// V2's JavaScript adapter: the worker side of VEP/1 for js and ts blocks (INTERNALS ch. 9-14,
// 14.4). V2 writes this file into its private run folder and starts it as `node js.js`. Node
// 22.13 is the floor (not 23.0 or 23.1).
'use strict';

// 1. The built-ins the adapter uses, before anything touches process.stdout or process.stdin:
// their getters create libuv handles on the protocol fds (14.4).
const fs = require('fs');
const vm = require('vm');
const v8 = require('v8');
const path = require('path');
const Module = require('module');
const stream = require('stream');
const { StringDecoder } = require('string_decoder');
const { MAX_STRING_LENGTH } = require('buffer').constants;
const url = require('url');
const util = require('util');
const child_process = require('child_process');

v8.setFlagsFromString('--expose-gc');
const gc = vm.runInNewContext('gc');

const VEP = 1;
const LIMIT = 64 * 1024 * 1024; // bytes per frame, both directions (9.1)
const MAX_DEPTH = 256; // nested lists/dicts (10.5)
const OUT_CHUNK = 1024 * 1024; // characters per `out` frame, well below LIMIT
const FLUSH_AT = 64 * 1024; // buffered characters that force a flush during a request
// fn stubs made since the last collection that force one at the next idle turn: 200, or one per
// 128 KB of heap, so a collection of a big heap costs each stub no more than of a small one (11.9)
const GC_AFTER = 200;
const GC_HEAP_PER_STUB = 128 * 1024;
const KINDS = ['ForeignError', 'ImportError', 'NameError', 'ValueError', 'MarshalError', 'ProtocolError', 'Error'];
const MAX_CAUSES = 8; // chained causes sent (9.6)
const ADAPTER = __filename;

// The protocol's own channel: frames go out only through the original fs.writeSync(1, ...) and
// come in through fs.readSync(0, ...); everything else that reaches fds 0-2 is wrapped below.
const rawWriteSync = fs.writeSync;
const rawReadSync = fs.readSync;
const rawExit = process.exit.bind(process);
const rawKill = process.kill.bind(process);

// ---------------------------------------------------------------------------------------------
// Errors

class V2Error extends Error {
  // Raised when a call into V2 fails or is refused (9.6, 9.8, 14.2).
  constructor(type = 'Error', message = '', data = null, lang = null, foreign_type = null) {
    super(message);
    this.name = 'V2Error';
    this.type = type;
    this.data = data;
    this.lang = lang;
    this.foreign_type = foreign_type;
    Object.defineProperty(this, '_v2', { value: null, writable: true, enumerable: false });
  }
}

// A global, so `e instanceof V2Error` works in blocks and in what they require (DOCS 8.4).
Object.defineProperty(globalThis, 'V2Error', { value: V2Error, writable: true, configurable: true, enumerable: false });

class Fail extends Error {
  // A failure the adapter detects itself, sent with its own kind (9.6).
  constructor(kind, message) {
    super(message);
    this.kind = kind;
    this.steps = []; // where inside a value, innermost first (10.5)
    this.located = false;
  }
}

const wellFormed = (s) => (typeof s === 'string' ? s.toWellFormed() : String(s));

function gone() {
  // The host is gone: end of input or a broken pipe (9.1).
  leave();
}

function leave() {
  // Exits; a `process.on('exit')` handler that throws is the user's error, not the adapter's. On
  // Windows Node's own teardown can fail a libuv assertion while a `fetch` winds down, so the
  // process ends without it, once everything is written, the exit handlers' output too (9.10).
  const end = process.platform === 'win32' ? () => rawKill(process.pid, 'SIGKILL') : process.reallyExit.bind(process);
  process.reallyExit = (code) => {
    process.reallyExit = end;
    try {
      drainOut(true);
    } catch {}
    end(code);
  };
  try {
    rawExit(0);
  } catch (e) {
    rawWriteSync(2, `uncaught error in a process 'exit' handler: ${e && e.stack ? e.stack : e}\n`);
    rawExit(0);
  }
}

function fatal(what) {
  // A line the adapter can't parse, or a reply it isn't waiting for (9.1).
  try {
    rawWriteSync(2, 'v2 js adapter: ' + what + '\n');
  } finally {
    rawExit(2);
  }
}

// ---------------------------------------------------------------------------------------------
// Framing (9.1)

let rbuf = Buffer.alloc(0);
const chunk = Buffer.allocUnsafe(65536);

function recv() {
  // The next frame; exits at end of input.
  for (;;) {
    const i = rbuf.indexOf(10);
    if (i >= 0) {
      const line = rbuf.subarray(0, i).toString('utf8');
      rbuf = rbuf.subarray(i + 1);
      let m;
      try {
        m = JSON.parse(line);
      } catch (e) {
        fatal('cannot parse a frame from V2: ' + e.message);
      }
      if (m === null || typeof m !== 'object' || Array.isArray(m) || typeof m.t !== 'string') {
        fatal("a frame from V2 that is not an object with a 't'");
      }
      return m;
    }
    let n;
    try {
      n = rawReadSync(0, chunk, 0, chunk.length, null);
    } catch (e) {
      if (e.code === 'EAGAIN') continue;
      n = 0; // EOF (Windows reports a closed pipe as an error)
    }
    if (n === 0) gone();
    rbuf = Buffer.concat([rbuf, chunk.subarray(0, n)]);
  }
}

class TooBig extends Error {}

class OrderedPairs {
  // A dict whose key order JSON.stringify must not change (to_v2 of a Map, 11.5).
  constructor(pairs) {
    this.pairs = pairs;
  }
}
let orderedPending = false;

function stringify(v) {
  if (v instanceof OrderedPairs) return '{' + v.pairs.map(([k, x]) => JSON.stringify(k) + ':' + stringify(x)).join(',') + '}';
  if (Array.isArray(v)) return '[' + v.map((x) => (x === undefined ? 'null' : stringify(x))).join(',') + ']';
  if (v !== null && typeof v === 'object') return '{' + Object.keys(v).map((k) => JSON.stringify(k) + ':' + stringify(v[k])).join(',') + '}';
  return JSON.stringify(v);
}

function writeFrame(obj) {
  let text;
  try {
    text = orderedPending ? stringify(obj) : JSON.stringify(obj);
  } catch (e) {
    if (e instanceof RangeError) throw new TooBig(String(MAX_STRING_LENGTH)); // longer than a JS string
    throw e;
  }
  const data = Buffer.from(text + '\n', 'utf8');
  if (data.length - 1 > LIMIT) throw new TooBig(String(data.length - 1)); // the newline isn't part of the frame
  let off = 0;
  while (off < data.length) {
    try {
      off += rawWriteSync(1, data, off, data.length - off);
    } catch (e) {
      if (e.code === 'EAGAIN') continue;
      gone();
    }
  }
}

const tooBigMessage = (n) => `value too large to send to V2 (${Math.ceil(n / (1 << 20))} MiB, limit ${LIMIT >> 20} MiB)`;

class Walk extends Set {
  // The containers open on the way down, and the least the frame can come to: a value too big
  // stops here, before the time and memory of encoding all of it (9.1).
  cost = 0;
  hooks = 0; // to_v2 hooks open on the way down (11.5)

  spend(n) {
    this.cost += n;
    if (this.cost > LIMIT) {
      const f = new Fail('MarshalError', `value too large to send to V2 (more than ${LIMIT >> 20} MiB)`);
      f.located = true;
      throw f;
    }
  }
}

// ---------------------------------------------------------------------------------------------
// The request stack (14.2) and output (9.9)

const stack = []; // {kind: 'request'|'waiting', id}
let closing = false;
let outBuf = []; // [stream, text]
let outSize = 0;
const fnReleases = []; // callable ids whose stubs died (11.8)
let pinned = []; // stubs encoded into the frame being built (11.8)
let counted = []; // handle ids whose counts the frame being built added (10.1)

function rollbackCounts(mark = 0) {
  // The counts added since `mark` were for tags that won't be sent (10.1).
  releaseHandles(counted.splice(mark));
}

const active = () => stack.length > 0 || closing;

function drainOut(final = false) {
  let items = outBuf;
  outBuf = [];
  outSize = 0;
  // A high surrogate at the very end may be completed by the next write.
  const last = items[items.length - 1];
  if (!final && last && /[\ud800-\udbff]$/.test(last[1])) {
    items = items.slice(0, -1).concat([[last[0], last[1].slice(0, -1)]]);
    outBuf = [[last[0], last[1].slice(-1)]];
    outSize = 1;
  }
  const merged = [];
  for (const [s, text] of items) {
    if (merged.length && merged[merged.length - 1][0] === s) merged[merged.length - 1][1].push(text);
    else merged.push([s, [text]]);
  }
  for (const [s, parts] of merged) {
    const text = parts.join('').toWellFormed();
    for (let i = 0; i < text.length; ) {
      let j = Math.min(i + OUT_CHUNK, text.length);
      if (j < text.length && (text.charCodeAt(j - 1) & 0xfc00) === 0xd800) j--; // keep a pair whole
      if (j > i) writeFrame({ t: 'out', stream: s, text: text.slice(i, j) });
      i = j;
    }
  }
}

function send(frame) {
  // Buffered output, then queued releases as one frame, then `frame` (14.2).
  drainOut();
  if (fnReleases.length) writeFrame({ t: 'release', fn: fnReleases.splice(0) });
  try {
    writeFrame(frame);
  } catch (e) {
    if (e instanceof TooBig) rollbackCounts();
    throw e;
  } finally {
    pinned = [];
    orderedPending = false;
  }
  counted = [];
}

function writeOut(s, text) {
  if (!text) return;
  outBuf.push([s, text]);
  outSize += text.length;
  if (active() && (text.endsWith('\n') || outSize >= FLUSH_AT)) drainOut();
}

function outputStream(name) {
  // process.stdout and process.stderr: text sent as `out` frames in program order (9.9).
  const decoder = new StringDecoder('utf8');
  const w = new stream.Writable({
    decodeStrings: false,
    write(data, encoding, cb) {
      writeOut(name, typeof data === 'string' ? data : decoder.write(data));
      cb();
    },
  });
  // Writes complete at once, so they keep their order with frames and with console.log.
  w.write = function (data, encoding, cb) {
    if (typeof encoding === 'function') [cb, encoding] = [encoding, undefined];
    const text = typeof data === 'string' ? data : decoder.write(Buffer.isBuffer(data) ? data : Buffer.from(data));
    writeOut(name, text);
    if (typeof cb === 'function') process.nextTick(cb);
    return true;
  };
  w.isTTY = false;
  w.fd = name === 'stdout' ? 1 : 2;
  return w;
}

const stdoutStream = outputStream('stdout');
const stderrStream = outputStream('stderr');

function noInput() {
  // process.stdin: V2's input() owns the user's stdin (9.9, DOCS 4.6).
  const r = new stream.Readable({
    read() {
      this.destroy(new Error("blocks cannot read standard input: V2's input() owns it; pass the data in as a block arg"));
    },
  });
  r.isTTY = false;
  r.fd = 0;
  return r;
}
let stdinStream = null;

// 2. Stdio exactly as 9.9: the getters, fs on fds 0-2, child_process stdio.
Object.defineProperty(process, 'stdout', { configurable: true, enumerable: true, get: () => stdoutStream });
Object.defineProperty(process, 'stderr', { configurable: true, enumerable: true, get: () => stderrStream });
Object.defineProperty(process, 'stdin', { configurable: true, enumerable: true, get: () => (stdinStream ??= noInput()) });

const stdinError = () => Object.assign(new Error("blocks cannot read standard input: V2's input() owns it; pass the data in as a block arg"), { code: 'EBADF' });
const textOf = (buffer, offset, length) => {
  if (typeof buffer === 'string') return buffer;
  const b = Buffer.from(buffer.buffer ?? buffer, buffer.byteOffset ?? 0, buffer.byteLength ?? buffer.length);
  const start = typeof offset === 'number' ? offset : 0;
  const end = typeof length === 'number' ? start + length : b.length;
  return b.subarray(start, end).toString('utf8');
};
fs.writeSync = function (fd, buffer, offset, length, position) {
  if (fd === 1 || fd === 2) {
    const text = textOf(buffer, offset, length);
    writeOut(fd === 1 ? 'stdout' : 'stderr', text);
    return typeof buffer === 'string' ? Buffer.byteLength(buffer, typeof offset === 'string' ? offset : 'utf8') : (typeof length === 'number' ? length : buffer.byteLength - (offset || 0));
  }
  return rawWriteSync.apply(fs, arguments);
};
const rawWrite = fs.write;
fs.write = function (fd, buffer, ...rest) {
  if (fd === 1 || fd === 2) {
    const cb = rest.findLast((a) => typeof a === 'function');
    try {
      const n = fs.writeSync(fd, buffer, ...rest.filter((a) => typeof a !== 'function'));
      if (cb) process.nextTick(cb, null, n, buffer);
    } catch (e) {
      if (cb) process.nextTick(cb, e);
    }
    return;
  }
  return rawWrite.call(fs, fd, buffer, ...rest);
};
fs.readSync = function (fd, ...rest) {
  if (fd === 0) throw stdinError();
  return rawReadSync.call(fs, fd, ...rest);
};
const rawRead = fs.read;
fs.read = function (fd, ...rest) {
  if (fd === 0) {
    const cb = rest.findLast((a) => typeof a === 'function');
    if (cb) process.nextTick(cb, stdinError());
    return;
  }
  return rawRead.call(fs, fd, ...rest);
};
// Node's fast paths for these call its native binding directly, past the wrappers above.
const isOutFd = (fd) => fd === 1 || fd === 2;
const dataText = (data, options) => {
  if (typeof data === 'string') return data;
  const enc = typeof options === 'string' ? options : options && options.encoding;
  if (Buffer.isBuffer(data) || ArrayBuffer.isView(data)) return textOf(data);
  return enc ? Buffer.from(String(data), enc).toString('utf8') : String(data);
};
for (const name of ['readFileSync', 'readvSync']) {
  const raw = fs[name];
  fs[name] = function (fd, ...rest) {
    if (fd === 0) throw stdinError();
    return raw.call(fs, fd, ...rest);
  };
}
for (const name of ['readFile', 'readv']) {
  const raw = fs[name];
  fs[name] = function (fd, ...rest) {
    if (fd === 0) {
      const cb = rest.findLast((a) => typeof a === 'function');
      if (cb) process.nextTick(cb, stdinError());
      return;
    }
    return raw.call(fs, fd, ...rest);
  };
}
for (const name of ['writeFileSync', 'appendFileSync']) {
  const raw = fs[name];
  fs[name] = function (fd, data, options) {
    if (isOutFd(fd)) return void writeOut(fd === 1 ? 'stdout' : 'stderr', dataText(data, options));
    return raw.apply(fs, arguments);
  };
}
for (const name of ['writeFile', 'appendFile']) {
  const raw = fs[name];
  fs[name] = function (fd, data, ...rest) {
    if (isOutFd(fd)) {
      writeOut(fd === 1 ? 'stdout' : 'stderr', dataText(data, typeof rest[0] === 'function' ? undefined : rest[0]));
      const cb = rest.findLast((a) => typeof a === 'function');
      if (cb) process.nextTick(cb, null);
      return;
    }
    return raw.call(fs, fd, data, ...rest);
  };
}
const rawWritevSync = fs.writevSync;
fs.writevSync = function (fd, buffers, position) {
  if (isOutFd(fd)) {
    const text = buffers.map((b) => textOf(b)).join('');
    writeOut(fd === 1 ? 'stdout' : 'stderr', text);
    return buffers.reduce((n, b) => n + b.byteLength, 0);
  }
  return rawWritevSync.apply(fs, arguments);
};
const rawWritev = fs.writev;
fs.writev = function (fd, buffers, ...rest) {
  if (isOutFd(fd)) {
    const cb = rest.findLast((a) => typeof a === 'function');
    try {
      const n = fs.writevSync(fd, buffers);
      if (cb) process.nextTick(cb, null, n, buffers);
    } catch (e) {
      if (cb) process.nextTick(cb, e);
    }
    return;
  }
  return rawWritev.call(fs, fd, buffers, ...rest);
};

// Children never inherit fd 0 or 1: stdin becomes the null device and stdout goes to stderr.
function rewriteStdio(options, fork) {
  const o = options && typeof options === 'object' ? { ...options } : {};
  let stdio = o.stdio;
  // fork's own default: the parent's fds plus an IPC channel, unless `silent`.
  if (stdio === undefined && fork) stdio = o.silent ? ['pipe', 'pipe', 'pipe', 'ipc'] : ['inherit', 'inherit', 'inherit', 'ipc'];
  if (typeof stdio === 'string') stdio = [stdio, stdio, stdio];
  if (Array.isArray(stdio)) {
    stdio = stdio.map((x, i) => {
      const fd = x === 'inherit' ? i : x === process.stdin ? 0 : x === process.stdout ? 1 : x === process.stderr ? 2 : x;
      return fd === 0 ? 'ignore' : fd === 1 ? 2 : fd;
    });
    if (fork && !stdio.includes('ipc')) stdio.push('ipc');
    o.stdio = stdio;
  }
  return o;
}
function withOptions(fn, at, fork) {
  // `at`: the index of the options argument when every optional argument is given.
  const wrapped = function (...args) {
    let i = args.findIndex((a, k) => k > 0 && a && typeof a === 'object' && !Array.isArray(a));
    if (i < 0) {
      const cb = args.findIndex((a) => typeof a === 'function');
      i = Math.min(at, cb >= 0 ? cb : args.length);
      while (args.length < i) args.push(undefined);
      args.splice(i, 0, {});
    }
    args[i] = rewriteStdio(args[i], fork);
    return fn.apply(this, args);
  };
  for (const key of Reflect.ownKeys(fn)) {
    if (key === 'length' || key === 'name' || key === 'prototype') continue;
    Object.defineProperty(wrapped, key, Object.getOwnPropertyDescriptor(fn, key));
  }
  Object.defineProperty(wrapped, 'name', { value: fn.name });
  return wrapped;
}
for (const [name, at] of [['spawn', 2], ['spawnSync', 2], ['execFile', 2], ['execFileSync', 2], ['fork', 2], ['exec', 1], ['execSync', 1]]) {
  child_process[name] = withOptions(child_process[name], at, name === 'fork');
}
Module.syncBuiltinESMExports();

// 3. A block can't end the worker; the original exit is kept for `bye` (9.10).
process.exit = function (code) {
  throw new Error(`process.exit(${code === undefined ? '' : code}) can't end V2's JavaScript engine; throw an error instead`);
};

// 4. The host owns Ctrl+C; errors with no call to attach to are printed and the worker goes on.
process.on('SIGINT', () => {});
if (process.platform === 'win32') process.on('SIGBREAK', () => {});
function printUnattached(prefix, e) {
  // With the frames 12.6 keeps; an error from V2 also says its type and where V2 raised it.
  let lines;
  try {
    const fromV2 = e instanceof V2Error;
    lines = [`${prefix}${errorName(e)}: ${fromV2 ? `${e.type}: ` : ''}${errorMessage(e)}`];
    const raised = fromV2 && e._v2 && Array.isArray(e._v2.trace) ? e._v2.trace[0] : null;
    if (typeof raised === 'string') lines.push(`    at ${raised}`);
    for (const f of frames(e)) {
      const at = `${f.file}:${f.line}${f.col === null ? '' : `:${f.col}`}`;
      lines.push(`    at ${f.func ? `${f.func} (${at})` : at}`);
    }
  } catch {
    lines = [`${prefix}<an error that can't be printed>`]; // a revoked Proxy, a throwing trap
  }
  writeOut('stderr', lines.join('\n') + '\n');
}
process.on('uncaughtException', (e) => printUnattached('', e));
process.on('unhandledRejection', (e) => printUnattached('unhandled rejection: ', e));
// Node prints an ExperimentalWarning the first time types are stripped (12.5).
const rawEmitWarning = process.emitWarning;
process.emitWarning = function (warning, ...rest) {
  const type = typeof rest[0] === 'string' ? rest[0] : rest[0] && rest[0].type;
  const text = warning && warning.message ? warning.message : String(warning);
  if (type === 'ExperimentalWarning' && /Type Stripping|stripTypeScriptTypes|vm\.USE_MAIN_CONTEXT_DEFAULT_LOADER|importModuleDynamically/i.test(text)) return;
  return rawEmitWarning.call(process, warning, ...rest);
};

// ---------------------------------------------------------------------------------------------
// Handles (11.1) and V2 function stubs (11.8)

const handles = new Map(); // hid -> {obj, count, batch}
const byObj = new Map(); // obj -> hid (objects by identity, symbols and BigInts by value)
let nextHid = 0;

function typeName(obj) {
  if (typeof obj === 'symbol') return 'symbol';
  if (typeof obj === 'bigint') return 'bigint';
  if (typeof obj === 'function') return /^class[\s{]/.test(Function.prototype.toString.call(obj)) ? 'class' : 'function';
  try {
    const c = obj && obj.constructor;
    if (c && typeof c.name === 'string' && c.name) return c.name;
  } catch {}
  return 'Object';
}

function handleTag(obj) {
  let hid = byObj.get(obj);
  if (hid === undefined) {
    hid = ++nextHid;
    handles.set(hid, { obj, count: 0, batch: false });
    byObj.set(obj, hid);
  }
  handles.get(hid).count += 1;
  counted.push(hid);
  const tag = { $v2: 'h', id: hid, type: wellFormed(typeName(obj)) };
  if (typeof obj === 'function') tag.callable = true;
  return tag;
}

function releaseHandles(ids) {
  for (const hid of ids) {
    const entry = handles.get(hid);
    if (!entry) continue;
    entry.count -= 1;
    if (entry.count <= 0) {
      handles.delete(hid);
      byObj.delete(entry.obj);
      pendingItems.delete(hid);
    }
  }
}

const stubs = new Map(); // callable id -> WeakRef to its one stub
const stubCells = new WeakMap(); // stub -> {id, n}
let stubsSinceGc = 0;
const finalizer = new FinalizationRegistry((cell) => {
  if (stubs.get(cell.id)?.deref() === undefined) stubs.delete(cell.id);
  for (let i = 0; i < cell.n; i++) fnReleases.push(cell.id);
});

function stubFor(id, name) {
  let s = stubs.get(id)?.deref();
  if (s === undefined) {
    const cell = { id, n: 0 };
    const label = name || 'function';
    s = { [label]: function (...args) { return hostCall({ fn: id }, args, label); } }[label];
    stubCells.set(s, cell);
    stubs.set(id, new WeakRef(s));
    finalizer.register(s, cell);
    stubsSinceGc += 1;
  }
  stubCells.get(s).n += 1;
  return s;
}

const routeStubs = new WeakMap(); // route stub -> route id (13.7)

function routeStub(route, name) {
  // A function an inner @import brings in through V2: another engine's, or V2's (13.7).
  const s = { [name]: function (...args) { return hostCall({ route }, args, name); } }[name];
  routeStubs.set(s, route);
  return s;
}

const isV2Function = (x) => typeof x === 'function' && (stubCells.has(x) || routeStubs.has(x));

// ---------------------------------------------------------------------------------------------
// Marshaling (10.2-10.5, 10.7)

const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;

function surrogateCheck(s) {
  if (!s.isWellFormed()) {
    let i = 0;
    for (; i < s.length; i++) {
      const c = s.charCodeAt(i);
      if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length && (s.charCodeAt(i + 1) & 0xfc00) === 0xdc00) {
        i++;
        continue;
      }
      if (c >= 0xd800 && c <= 0xdfff) break;
    }
    const cp = [...s.slice(0, i)].length;
    throw new Fail('MarshalError', `string contains an unpaired surrogate (U+${s.charCodeAt(i).toString(16).toUpperCase()}) at index ${cp}`);
  }
  return s;
}

const ownFunction = (d) => d !== undefined && typeof d.value === 'function' && !stubCells.has(d.value) && !routeStubs.has(d.value);

function isPlainObject(x) {
  const p = Object.getPrototypeOf(x);
  if (p === null) return true;
  if (Object.getPrototypeOf(p) !== null) return false;
  try {
    return typeof p.constructor === 'function' && p.constructor.name === 'Object';
  } catch {
    return false;
  }
}

function encode(x, depth = 0, open = new Walk()) {
  // A JS value for V2 (10.7): plain data copied, anything else a handle.
  if (x === undefined || x === null) return null;
  const t = typeof x;
  if (t === 'boolean') return x;
  if (t === 'number') {
    if (Number.isNaN(x)) return { $v2: 'float', v: 'nan' };
    if (!Number.isFinite(x)) return { $v2: 'float', v: x > 0 ? 'inf' : '-inf' };
    if (Number.isSafeInteger(x)) return x === 0 ? 0 : x;
    if (Number.isInteger(x) && x >= -(2 ** 63) && x < 2 ** 63) return { $v2: 'int', v: BigInt(x).toString() };
    return x;
  }
  if (t === 'bigint') return x >= I64_MIN && x <= I64_MAX ? { $v2: 'int', v: x.toString() } : handleTag(x);
  if (t === 'string') {
    open.spend(x.length);
    return surrogateCheck(x);
  }
  if (t === 'function' && stubCells.has(x)) {
    pinned.push(x);
    return { $v2: 'fn', id: stubCells.get(x).id };
  }
  if (t === 'function' && routeStubs.has(x)) return { $v2: 'fn', route: routeStubs.get(x) };
  if (t !== 'object' || util.types.isProxy(x)) return handleTag(x);
  if (x instanceof V2Error) return errorTag(x, depth, open);
  if (util.types.isUint8Array(x)) {
    open.spend(Math.ceil(x.byteLength / 3) * 4);
    return { $v2: 'bytes', b64: Buffer.from(x.buffer, x.byteOffset, x.byteLength).toString('base64') };
  }
  if (Array.isArray(x)) {
    if (depth >= MAX_DEPTH || open.has(x)) return handleTag(x);
    open.spend(2 * x.length);
    open.add(x);
    try {
      const out = new Array(x.length);
      for (let i = 0; i < x.length; i++) {
        try {
          out[i] = encode(x[i], depth + 1, open);
        } catch (e) {
          if (e instanceof Fail) e.steps.push(`[${i}]`);
          throw e;
        }
      }
      return out;
    } finally {
      open.delete(x);
    }
  }
  if (isPlainObject(x)) {
    if (depth >= MAX_DEPTH || open.has(x)) return handleTag(x);
    const keys = Object.keys(x);
    // One that carries its own functions is behaviour, not data: copied, they'd lose `this`.
    if (Reflect.ownKeys(x).some((k) => ownFunction(Object.getOwnPropertyDescriptor(x, k)))) return handleTag(x);
    open.spend(2 * keys.length);
    open.add(x);
    try {
      const out = {};
      let tagged = false;
      for (const k of keys) {
        surrogateCheck(k);
        if (k === '$v2') tagged = true;
        let v;
        try {
          v = encode(x[k], depth + 1, open);
        } catch (e) {
          if (e instanceof Fail) e.steps.push(`["${k}"]`);
          throw e;
        }
        Object.defineProperty(out, k, { value: v, enumerable: true, writable: true, configurable: true });
      }
      return tagged ? { $v2: 'dict', v: out } : out;
    } finally {
      open.delete(x);
    }
  }
  return handleTag(x);
}

function encodeValue(x, root, depth = 0) {
  // encode(x) for the frame member `root` (`value`, `args[0]`), `depth` levels down in the value
  // V2 decodes; a MarshalError says where (10.5).
  try {
    return encode(x, depth);
  } catch (e) {
    if (e instanceof Fail && !e.located) {
      e.located = true;
      e.message += ` (at ${root}${e.steps.slice().reverse().join('')})`;
    }
    throw e;
  }
}

const textOrNull = (v) => (typeof v === 'string' ? wellFormed(v) : null);

function errorTag(e, depth = 0, open = new Walk(), causes = 0) {
  // A V2Error as an `error` tag (10.3). One that came from V2 keeps its members (location,
  // trace), but its data and cause are encoded again, so their handles and stubs belong to this
  // frame (11.1, 11.8). An error tag is not a nesting level (10.5), but one inside its own data
  // is sent as a handle.
  if (open.has(e)) return handleTag(e);
  const tag = e._v2
    ? { ...e._v2 }
    : {
        $v2: 'error', type: textOrNull(e.type) || 'Error', message: wellFormed(String(e.message)), lang: textOrNull(e.lang),
        foreign_type: textOrNull(e.foreign_type), file: null, line: null, col: null, trace: [],
      };
  open.add(e);
  try {
    tag.data = encode(e.data, depth, open);
    tag.cause = e._cause instanceof V2Error && causes < MAX_CAUSES ? errorTag(e._cause, depth, open, causes + 1) : null;
  } finally {
    open.delete(e);
  }
  return tag;
}

function decode(j) {
  // A V2 value in JavaScript (10.7), top down (10.1), in the adapter's realm.
  if (Array.isArray(j)) return j.map(decode);
  if (j !== null && typeof j === 'object') {
    if (Object.prototype.hasOwnProperty.call(j, '$v2')) return decodeTag(j);
    const out = {};
    for (const k of Object.keys(j)) Object.defineProperty(out, k, { value: decode(j[k]), enumerable: true, writable: true, configurable: true });
    return out;
  }
  return j;
}

function decodeTag(j) {
  const tag = j.$v2;
  switch (tag) {
    case 'h': {
      const entry = handles.get(j.id);
      if (!entry) throw new Fail('ProtocolError', `unknown handle id ${j.id}`);
      return entry.obj;
    }
    case 'fn':
      return 'unit' in j ? realTarget(j.unit, j.name) : stubFor(j.id, j.name);
    case 'int': {
      const n = BigInt(j.v);
      return n >= -(2n ** 53n - 1n) && n <= 2n ** 53n - 1n ? Number(n) : n;
    }
    case 'bytes':
      return Buffer.from(j.b64, 'base64');
    case 'float':
      return { nan: NaN, inf: Infinity, '-inf': -Infinity }[j.v];
    case 'error': {
      const e = new V2Error(j.type, j.message, decode(j.data ?? null), j.lang ?? null, j.foreign_type ?? null);
      Object.defineProperty(e, '_cause', { value: j.cause && typeof j.cause === 'object' ? decode(j.cause) : null, enumerable: false });
      e._v2 = j;
      return e;
    }
    case 'dict': {
      const out = {};
      for (const k of Object.keys(j.v)) Object.defineProperty(out, k, { value: decode(j.v[k]), enumerable: true, writable: true, configurable: true });
      return out;
    }
  }
  throw new Fail('ProtocolError', `an unknown '$v2' tag '${tag}'`);
}

// ---------------------------------------------------------------------------------------------
// Errors as `err` payloads (9.6, 12.6)

const sources = new Map(); // .v2 path -> Map(line -> body text line), kept by rememberSource
// .v2 path -> Map(body's first line -> rows) for bodies holding U+2028 or U+2029, which V8 counts
// as line breaks: row k is V8's line first + k as [.v2 line, UTF-16 offset of its segment].
const shifted = new Map();

function rememberSource(file, first, source) {
  let lines = sources.get(file);
  if (!lines) sources.set(file, (lines = new Map()));
  source.split('\n').forEach((text, i) => lines.set(first + i, text));
  if (!/[\u2028\u2029]/.test(source)) return;
  const rows = [];
  source.split('\n').forEach((text, i) => {
    rows.push([first + i, 0]);
    for (let j = 0; j < text.length; j++) if (text[j] === '\u2028' || text[j] === '\u2029') rows.push([first + i, j + 1]);
  });
  if (!shifted.has(file)) shifted.set(file, new Map());
  shifted.get(file).set(first, rows);
}

function realPosition(file, line, utf16col) {
  // V8's line and 1-based UTF-16 column (or null) as the .v2 line and the code-point column
  // within it; columns only for .v2 files (12.6).
  let real = line;
  let offset = 0;
  for (const [first, rows] of shifted.get(file) || []) {
    const row = rows[line - first];
    if (row) {
      [real, offset] = row;
      break;
    }
  }
  const text = sources.get(file)?.get(real);
  return [real, text === undefined || utf16col === null ? null : codePointCol(text, utf16col + offset)];
}

function codePointCol(text, utf16col) {
  // A 1-based column in UTF-16 units as 1-based code points (12.6).
  return [...text.slice(0, Math.max(0, utf16col - 1))].length + 1;
}

const FRAME = /^\s*at (?:(.*?) \()?(.+?):(\d+):(\d+)\)?$/;

function frames(e) {
  let stack = '';
  try {
    if (e && typeof e.stack === 'string') stack = e.stack;
  } catch {} // a `message` getter that throws, read by Node to format the stack
  const out = [];
  for (const line of stack.split('\n')) {
    const m = FRAME.exec(line);
    if (!m) continue;
    let [, func, file, lineNo, col] = m;
    if (file.startsWith('node:') || file === ADAPTER || file.startsWith('evalmachine.')) continue;
    if (file.startsWith('file://')) file = url.fileURLToPath(file);
    const [real, cp] = realPosition(file, Number(lineNo), Number(col));
    out.push({
      file: wellFormed(file), line: real, col: cp,
      func: func ? wellFormed(func.replace(/^(?:async |new )/, '')) : null,
    });
  }
  return out;
}

function errorName(e) {
  if (e === null || (typeof e !== 'object' && typeof e !== 'function')) return typeof e;
  let name;
  try {
    name = e.name;
    if (typeof name !== 'string') return typeName(e);
  } catch {
    return 'Error';
  }
  // A class that sets no `name` of its own still says what it is, as a Python class does.
  try {
    const C = e.constructor;
    const own = typeof C === 'function' && typeof C.name === 'string' ? C.name : '';
    if (own && own !== name && !Object.hasOwn(e, 'name') && C.prototype && !Object.hasOwn(C.prototype, 'name')) return own;
  } catch {} // a `constructor` getter or Proxy trap that throws
  return name;
}

function errorMessage(e) {
  try {
    if (e !== null && typeof e === 'object' && typeof e.message === 'string') return e.message;
    return String(e);
  } catch {
    return '<unprintable>';
  }
}

function serialize(e, depth = 0) {
  // An exception, or any thrown value, as `err.error` (9.6).
  if (e instanceof Fail) return { type: e.kind, message: wellFormed(e.message), foreign_type: null, trace: [] };
  const trace = e && e.v2Frame ? [e.v2Frame, ...frames(e)] : frames(e);
  if (e instanceof V2Error) {
    const kind = KINDS.includes(e.type) ? e.type : 'ForeignError';
    const out = { type: kind, message: wellFormed(String(e.message)), foreign_type: kind !== e.type ? 'V2Error' : textOrNull(e.foreign_type), trace };
    // An error from V2 goes back as it was; one made in JS is reported like any exception, so
    // its kind is filtered and it can't make V2 exit 3.
    if (e._v2) {
      try {
        out.v2 = errorTag(e);
      } catch (err) {
        if (!(err instanceof Fail)) throw err;
        rollbackCounts();
        out.v2 = { ...e._v2, data: null, cause: null };
      }
    }
    return out;
  }
  const out = { type: 'ForeignError', message: wellFormed(errorMessage(e)), foreign_type: wellFormed(errorName(e)), trace };
  if (e !== null && typeof e === 'object' && e.cause !== undefined && depth < MAX_CAUSES) out.cause = serialize(e.cause, depth + 1);
  return out;
}

// ---------------------------------------------------------------------------------------------
// Calls into V2 (14.2)

let nextOutId = 0;

function hostCall(target, args, name) {
  const top = stack[stack.length - 1];
  if (!top || top.kind !== 'request') throw new V2Error('Error', `V2 function '${name}' called outside an active V2 call`);
  let frame;
  try {
    frame = { t: 'call', id: 0, target, args: args.map((a, i) => encodeValue(a, `args[${i}]`)), kwargs: {} };
  } catch (e) {
    rollbackCounts();
    if (e instanceof Fail) throw new V2Error(e.kind, e.message);
    throw e;
  }
  frame.id = ++nextOutId;
  try {
    send(frame);
  } catch (e) {
    if (e instanceof TooBig) throw new V2Error('MarshalError', tooBigMessage(Number(e.message)));
    throw e;
  }
  stack.push({ kind: 'waiting', id: frame.id });
  try {
    return serveSync(frame.id);
  } finally {
    stack.pop();
  }
}

// ---------------------------------------------------------------------------------------------
// Units (12.4, 12.5)

// Own names of the main realm's globalThis that a fresh context lacks: Node's globals.
const NODE_GLOBALS = (() => {
  const fresh = new Set(Object.getOwnPropertyNames(vm.runInNewContext('globalThis')));
  return Object.getOwnPropertyNames(globalThis).filter((k) => !fresh.has(k) && k !== 'global' && k !== 'console');
})();

const NODE_NAMES = new Set(NODE_GLOBALS);
const NODE_ENUMERABLE = new Set(NODE_GLOBALS.filter((k) => Object.prototype.propertyIsEnumerable.call(globalThis, k)));

const IDENT = /^[\p{ID_Start}$_][\p{ID_Continue}$‌‍]*$/u;
const RESERVED = new Set(('break case catch class const continue debugger default delete do else enum export extends false finally ' +
  'for function if import in instanceof new null return super switch this throw true try typeof var void while with ' +
  'yield let static implements interface package private protected public await').split(' '));
const PROVIDED = new Set(['undefined', 'NaN', 'Infinity', 'globalThis', '__proto__']);

function checkName(k, what = 'block arg') {
  if (!IDENT.test(k) || RESERVED.has(k) || PROVIDED.has(k)) throw new Fail('ValueError', `${what} '${k}' is not a valid JavaScript name`);
  return k;
}

function entryUrl(spec, from) {
  if (spec.startsWith('node:') || Module.builtinModules.includes(spec)) return spec;
  return url.pathToFileURL(Module.createRequire(from).resolve(spec)).href;
}


function newContext(file) {
  // One vm context (one realm) per unit: private top-level names (12.4).
  const sb = {};
  const module = { exports: {} };
  // Forwarded, not copied: reading a descriptor loads a lazy global (`FormData` is all of undici),
  // so nothing loads until a block uses it.
  for (const k of NODE_GLOBALS) {
    const enumerable = NODE_ENUMERABLE.has(k);
    Object.defineProperty(sb, k, {
      configurable: true, enumerable,
      get: () => globalThis[k],
      set(v) { Object.defineProperty(this, k, { value: v, writable: true, configurable: true, enumerable }); },
    });
  }
  Object.assign(sb, { console, module, exports: module.exports, require: Module.createRequire(file), __filename: file, __dirname: path.dirname(file) });
  const ctx = vm.createContext(sb);
  sb.global = vm.runInContext('Error.stackTraceLimit = 50; globalThis', ctx);
  return ctx;
}

const EXPLAIN = [
  ['Cannot use import statement outside a module', (lang) => `import statements are not allowed in @${lang} blocks — use require(...), await import(...) inside an async function, or @import { x } from js.<package>`],
  ["Unexpected token 'export'", (lang) => `export statements are not allowed in @${lang} blocks — use @export { name }`],
  ['await is only valid in async functions', (lang) => `top-level await is not allowed in @${lang} blocks — end the block with a Promise (it is awaited) or export an async function`],
];

function compileOrExplain(src, m) {
  try {
    return new vm.Script(src, { filename: m.file, lineOffset: m.line - 1, importModuleDynamically: vm.constants.USE_MAIN_CONTEXT_DEFAULT_LOADER });
  } catch (e) {
    if (e && e.name === 'SyntaxError') {
      const hit = EXPLAIN.find(([needle]) => String(e.message).includes(needle));
      if (hit) e.message = hit[1](m.lang);
      // V8 puts the location on the stack's first line, `file:line`, then the source line and
      // a caret under the column.
      const loc = /^(.*):(\d+)\r?\n(?:[^\n]*\r?\n([ \t]*)\^)?/.exec(e.stack || '');
      if (loc && loc[1] === m.file) {
        const [line, col] = realPosition(m.file, Number(loc[2]), loc[3] === undefined ? null : loc[3].length + 1);
        e.v2Frame = { file: m.file, line, col, func: null };
      }
    }
    throw e;
  }
}

function stripTypes(src, m) {
  try {
    return Module.stripTypeScriptTypes(src, { mode: 'strip' });
  } catch (e) {
    const message = String(e && e.message ? e.message : e);
    const err = new SyntaxError(e && e.code ? `${message} (${e.code})` : message);
    // The body position: a `[line:col]` marker, or (Node 24) the stack's first line `:line`,
    // then source lines (the one before too) and a caret under the column.
    const stack = e && typeof e.stack === 'string' ? e.stack : '';
    const marker = /\[(\d+):(\d+)\]/.exec(message);
    const header = /^[^\n]*:(\d+)\r?\n(?:[^\n]*\r?\n)*?([ \t]*)\^+[ \t]*(?:\r?\n|$)/.exec(stack);
    let line = null;
    let col = null;
    if (marker) [line, col] = [Number(marker[1]), Number(marker[2])];
    else if (header) [line, col] = [Number(header[1]), [...header[2]].length + 1];
    if (line !== null) err.v2Frame = { file: m.file, line: m.line + line - 1, col, func: null };
    err.stack = stack || 'SyntaxError: ' + err.message;
    throw err;
  }
}

const units = new Map(); // wire unit id -> {ctx, get, kinds} for definitions, {mod} for modules (14.2)

function inheritedBuiltin(o, n) {
  // `n` found on Object.prototype or Function.prototype only (13.5).
  for (let p = o; p !== null && p !== undefined; p = Object.getPrototypeOf(p)) {
    if (Object.prototype.hasOwnProperty.call(p, n)) return p === Object.prototype || p === Function.prototype;
  }
  return false;
}

function holds(o, n) {
  if (o === null || (typeof o !== 'object' && typeof o !== 'function')) return false;
  try {
    return n in o && !inheritedBuiltin(o, n);
  } catch {
    return false;
  }
}

function moduleAttr(mod, n) {
  // [value, the object it is read from] of a module's `n`, or null (13.5).
  if (mod === null || mod === undefined) return null;
  if (n === 'default') return [mod.default ?? mod, mod];
  if (holds(mod, n)) return [mod[n], mod];
  const d = mod.default;
  if (holds(d, n)) return [d[n], d];
  return null;
}

function unitAttr(u, name) {
  // [value, this] of `name` in a unit of this session, looked up now (12.4, 13.5).
  if ('mod' in u) {
    const r = moduleAttr(u.mod, name);
    if (!r) throw new Fail('NameError', `'${name}' is no longer defined`);
    return r;
  }
  try {
    return [exportLookup(u, name), undefined];
  } catch (e) {
    if (e && e.name === 'ReferenceError') throw new Fail('NameError', `'${name}' is no longer defined`);
    throw e;
  }
}

const withThis = new WeakMap(); // a module's function -> its module object -> what stands for it

function realTarget(unit, name) {
  // The function or class itself. One read from a module object keeps that `this`, as a call
  // through V2 does (13.5): only a plain call differs, everything else is the function's own.
  const [f, self] = unitAttr(units.get(unit), name);
  if (typeof f !== 'function' || self === undefined || isConstructor(f)) return f;
  if (!withThis.has(f)) withThis.set(f, new WeakMap());
  const stands = withThis.get(f);
  if (!stands.has(self)) stands.set(self, new Proxy(f, { apply: (t, that, args) => Reflect.apply(t, that === undefined ? self : that, args) }));
  return stands.get(self);
}

function unitStub(unit, name, route, alias) {
  // An inner @import from a block of this session that hasn't loaded yet (13.7). A Proxy, so calls,
  // `new`, `instanceof`, `class X extends` and static members reach the target, or say why not.
  const current = () => {
    const u = units.get(unit);
    if (!u) hostCall({ route }, [], alias); // V2 says why it isn't here (13.6)
    if (u.kinds && u.kinds.get(name) === 'value') throw new V2Error('ImportError', `'${name}' is a constant \u2014 pass it in as a block arg`);
    try {
      return unitAttr(u, name);
    } catch (e) {
      if (e instanceof Fail) throw new ReferenceError(e.message);
      throw e;
    }
  };
  const stub = new Proxy({ [alias]: function () {} }[alias], {
    apply(_, _this, args) {
      const [f, self] = current();
      if (typeof f !== 'function') throw new TypeError(`${name} is not a function`);
      return invoke(f, self, args);
    },
    construct(_, args, newTarget) {
      const [f] = current();
      if (typeof f !== 'function') throw new TypeError(`${name} is not a constructor`);
      return Reflect.construct(f, args, newTarget === stub ? f : newTarget);
    },
    get(_, k) {
      const [f] = current();
      return f === null || f === undefined ? undefined : Reflect.get(Object(f), k);
    },
  });
  return stub;
}

const importers = new Map(); // unit -> {ctx, alias, name, stub} of the blocks waiting for it (13.7)

function bindImporters(unit) {
  // Block `unit` has loaded, for V2 too: what imported from it gets the real thing.
  const u = units.get(unit);
  const waiting = (u && importers.get(unit)) || [];
  importers.delete(unit);
  for (const { ctx, alias, name, stub } of waiting) {
    if (ctx[alias] === stub && u.kinds.get(name) === 'func') defineData(ctx, alias, realTarget(unit, name));
  }
}

function bindImports(ctx, imports, keeps) {
  for (const e of imports) {
    const alias = checkName(e.as, '@import name');
    if (!('unit' in e)) defineData(ctx, alias, routeStub(e.route, alias));
    else if (units.has(e.unit)) defineData(ctx, alias, realTarget(e.unit, e.name));
    else {
      const stub = unitStub(e.unit, e.name, e.route, alias);
      defineData(ctx, alias, stub);
      if (keeps) importers.set(e.unit, [...(importers.get(e.unit) || []), { ctx, alias, name: e.name, stub }]);
    }
  }
}

function defineData(ctx, k, v) {
  Object.defineProperty(ctx, k, { value: v, writable: true, configurable: true, enumerable: true });
}

function exportLookup(unit, name) {
  let get = unit.get.get(name);
  if (!get) {
    get = IDENT.test(name) && !RESERVED.has(name)
      ? new vm.Script(`(() => ${name})`).runInContext(unit.ctx)
      : () => {
          if (!(name in unit.ctx)) throw new ReferenceError(`${name} is not defined`);
          return unit.ctx[name];
        };
    unit.get.set(name, get);
  }
  return get();
}

// The block replaced this global with its own (a `function fetch`): no longer the forwarding accessor.
function replacedGlobal(ctx, k) {
  return NODE_NAMES.has(k) && Object.getOwnPropertyDescriptor(ctx, k).get === undefined;
}

function exportReply(unit, before, requested) {
  const exports = [];
  const missing = [];
  const seen = new Set();
  const add = (n, v) => {
    seen.add(n);
    exports.push(typeof v === 'function' ? { name: n, kind: 'func' } : { name: n, kind: 'value', value: encodeValue(v, n) });
  };
  for (const n of requested) {
    if (n === '*' || seen.has(n)) continue;
    let v;
    try {
      v = exportLookup(unit, n);
    } catch (e) {
      if (e && e.name === 'ReferenceError') {
        missing.push(n);
        continue;
      }
      throw e;
    }
    add(n, v);
  }
  if (requested.includes('*')) {
    for (const k of Object.getOwnPropertyNames(unit.ctx)) {
      if ((before.has(k) && !replacedGlobal(unit.ctx, k)) || seen.has(k) || k.startsWith('_') || !k.isWellFormed()) continue;
      let v;
      try {
        v = unit.ctx[k];
      } catch {
        continue;
      }
      if (typeof v === 'function') add(k, v);
    }
  }
  return { value: null, exports, missing };
}

function isThenable(v) {
  try {
    return v !== null && (typeof v === 'object' || typeof v === 'function') && typeof v.then === 'function';
  } catch {
    return false;
  }
}

function settle(v, k, what) {
  // Now, or once a thenable settles; only an outermost request can wait (9.8).
  if (!isThenable(v)) return k(v);
  if (stack.length !== 1) {
    Promise.resolve(v).catch(() => {});
    throw new Fail('Error', `${what} during a nested cross-language call; only an outermost call can be awaited`);
  }
  return Promise.resolve(v).then(k);
}

function doLoad(m) {
  const src = m.lang === 'ts' ? stripTypes(m.source, m) : m.source;
  rememberSource(m.file, m.line, m.source); // stripping keeps every UTF-16 offset (12.6)
  const ctx = newContext(m.file);
  for (const [k, v] of Object.entries(m.args)) defineData(ctx, checkName(k), decode(v));
  bindImports(ctx, m.imports || [], m.kind !== 'action');
  const script = compileOrExplain(src, m);
  const before = new Set(Object.getOwnPropertyNames(ctx));
  return settle(script.runInContext(ctx), (v) => {
    if (m.kind === 'action') return { value: m.want_value ? encodeValue(v, 'value') : null, exports: [], missing: [] };
    const unit = { ctx, get: new Map() };
    const reply = exportReply(unit, before, m.exports);
    unit.kinds = new Map(reply.exports.map((e) => [e.name, e.kind]));
    if (reply.missing.length === 0) units.set(m.unit, unit);
    return reply;
  }, "the block's value is a Promise");
}

function doInit() {
  return { value: null };
}

function moduleReply(mod, names) {
  const nothing = mod === null || mod === undefined;
  if (names === null || names === undefined) return { value: nothing ? null : handleTag(mod), exports: [], missing: [] };
  const exports = [];
  const missing = [];
  const seen = new Set();
  for (const n of names) {
    if (typeof n !== 'string' || !n.isWellFormed() || seen.has(n)) continue;
    seen.add(n);
    const r = moduleAttr(mod, n);
    if (!r) missing.push(n);
    else if (typeof r[0] === 'function') exports.push({ name: n, kind: 'func' });
    else exports.push({ name: n, kind: 'value', value: encodeValue(r[0], n) });
  }
  return { value: null, exports, missing };
}

function doModule(m) {
  // A module unit (13.5): loaded once per unit through require (ES modules too), then the names
  // asked for.
  const u = units.get(m.unit);
  if (u) return moduleReply(u.mod, m.names);
  const loaded = (mod) => {
    units.set(m.unit, { mod });
    return moduleReply(mod, m.names);
  };
  const req = Module.createRequire(m.from);
  try {
    req.resolve(m.name);
  } catch (e) {
    // Not found: resolving looks only for the module asked for, not the ones it requires.
    if (e && NOT_FOUND.has(e.code)) throw new Fail('ImportError', String(e.message).split('\n')[0]);
    throw e;
  }
  let mod;
  try {
    mod = req(m.name);
  } catch (e) {
    // Top-level await in the module or its ES module graph; when a CommonJS module it loads
    // requires one, Node names that module on the "From" line, and the module has already run.
    const from = /\n\s*From (.*\S)\s*(\n|$)/.exec(String(e && e.message));
    if (e && e.code === 'ERR_REQUIRE_ASYNC_MODULE' && (!from || from[1] === m.from)) {
      return settle(import(entryUrl(m.name, m.from)), loaded, `the module '${m.name}' uses top-level await, so its import is a Promise`);
    }
    throw e;
  }
  return loaded(mod);
}

const NOT_FOUND = new Set(['MODULE_NOT_FOUND', 'ERR_PACKAGE_PATH_NOT_EXPORTED', 'ERR_UNKNOWN_BUILTIN_MODULE']);

// ---------------------------------------------------------------------------------------------
// Handles: calls and operations (9.5, 11.3-11.5)

function handleEntry(hid) {
  const entry = handles.get(hid);
  if (!entry) throw new Fail('ProtocolError', `unknown handle id ${hid}`);
  return entry;
}

// Natives that are functions first: calling them converts rather than constructs (11.3). Every
// block has its own realm, so they are recognized by name and native code, not identity.
const PLAIN_NATIVES = new Set(['Number', 'String', 'Boolean', 'Symbol', 'BigInt']);
const NATIVE_CODE = /\{\s*\[native code\]\s*\}\s*$/;

function isNative(f) {
  try {
    return typeof f === 'function' && NATIVE_CODE.test(Function.prototype.toString.call(f));
  } catch {
    return false;
  }
}

function isConstructor(f) {
  // A class, or a function whose prototype has own properties besides `constructor` (ES5
  // constructors such as EventEmitter, natives such as Map and Date) (11.3).
  if (typeof f !== 'function' || (PLAIN_NATIVES.has(f.name) && isNative(f))) return false;
  let src = '';
  try {
    src = Function.prototype.toString.call(f);
  } catch {}
  if (/^class[\s{]/.test(src)) return true;
  let p;
  try {
    p = f.prototype;
  } catch {
    return false;
  }
  if (p === null || (typeof p !== 'object' && typeof p !== 'function')) return false;
  return Reflect.ownKeys(p).some((k) => k !== 'constructor');
}

function invoke(f, self, args) {
  return isConstructor(f) ? Reflect.construct(f, args) : Reflect.apply(f, self, args);
}

function doCall(m) {
  const target = m.target || {};
  const args = (m.args || []).map(decode);
  let name;
  let result;
  if ('unit' in target) {
    const u = units.get(target.unit);
    if (!u) throw new Fail('ProtocolError', `unknown unit ${target.unit}`);
    name = target.name;
    const [f, self] = unitAttr(u, name);
    if (typeof f !== 'function') throw new TypeError(`${name} is not a function`);
    result = invoke(f, self, args);
    return settle(result, (v) => ({ value: encodeValue(v, 'value') }), `'${name}' returned a Promise`);
  }
  if (!('h' in target)) throw new Fail('ProtocolError', "a call target V2's JavaScript adapter does not serve");
  const obj = handleEntry(target.h).obj;
  if ('method' in target) {
    name = target.method;
    const f = obj[name];
    if (typeof f !== 'function') throw new TypeError(`${name} is not a function`);
    result = invoke(f, obj, args);
  } else {
    name = (typeof obj === 'function' && obj.name) || typeName(obj);
    if (typeof obj !== 'function') throw new TypeError(`${typeName(obj)} is not a function`);
    result = invoke(obj, undefined, args);
  }
  return settle(result, (v) => ({ value: encodeValue(v, 'value') }), `'${name}' returned a Promise`);
}

// One bound function per (object, method), so `let f = h.m; f()` keeps `this` (11.3).
const boundMethods = new WeakMap();

function boundMethod(obj, f) {
  const make = () => new Proxy(f, { apply: (t, _this, args) => Reflect.apply(t, obj, args) });
  if (obj === null || (typeof obj !== 'object' && typeof obj !== 'function')) return make();
  let inner = boundMethods.get(obj);
  if (!inner) boundMethods.set(obj, (inner = new WeakMap()));
  let p = inner.get(f);
  if (!p) inner.set(f, (p = make()));
  return p;
}

function inheritsObjectToString(obj) {
  // Whether toString comes from some realm's Object.prototype (a root prototype).
  let p = obj;
  while (p !== null && p !== undefined) {
    if (Object.prototype.hasOwnProperty.call(p, 'toString')) return Object.getPrototypeOf(p) === null;
    p = Object.getPrototypeOf(p);
  }
  return true;
}

function strOf(obj) {
  if (typeof obj === 'function' || (obj !== null && typeof obj === 'object' && inheritsObjectToString(obj))) {
    return util.inspect(obj, { depth: 2, breakLength: Infinity });
  }
  return String(obj);
}

function floatText(x) {
  // V2's str() of a float: Rust's shortest form (`1e21`, `0.0001`, `1e-5`, `2.0`) (5.10).
  if (Number.isNaN(x)) return 'nan';
  if (!Number.isFinite(x)) return x > 0 ? 'inf' : '-inf';
  if (x === 0) return Object.is(x, -0) ? '-0.0' : '0.0';
  const a = Math.abs(x);
  if (a >= 1e16 || a < 1e-4) return x.toExponential().replace('e+', 'e');
  const t = String(x);
  return t.includes('.') ? t : t + '.0';
}

function keyText(k, mapping) {
  if (typeof k === 'string') return surrogateCheck(k);
  if (k === null || k === undefined) return 'null';
  if (typeof k === 'boolean') return k ? 'true' : 'false';
  if (typeof k === 'number') return Number.isInteger(k) && k >= -(2 ** 63) && k < 2 ** 63 ? BigInt(k).toString() : floatText(k);
  if (typeof k === 'bigint' && k >= I64_MIN && k <= I64_MAX) return k.toString();
  throw new Fail('MarshalError', `to_v2(): can't convert ${typeName(mapping)} to V2 data (key of type ${typeName(k)})`);
}

function toV2(x, depth = 0, open = new Walk()) {
  // `op value`: the object as plain data, converted here (11.5). `depth` is the level the value
  // would sit at, so a container at 256 or deeper is too deep, as for the host (10.5).
  if (x === undefined || x === null || typeof x === 'boolean' || typeof x === 'number' || typeof x === 'string') return encode(x, depth, open);
  if (typeof x === 'bigint') {
    if (x < I64_MIN || x > I64_MAX) throw new Fail('MarshalError', "to_v2(): can't convert bigint to V2 data (it needs more than 64 bits)");
    return encode(x);
  }
  if (util.types.isUint8Array(x)) return encode(x, depth, open);
  if (isV2Function(x) || x instanceof V2Error) return encode(x, depth, open); // V2's own values (10.7)
  if (depth >= MAX_DEPTH) throw new Fail('MarshalError', `to_v2(): value nested deeper than ${MAX_DEPTH} levels`);
  const isMap = util.types.isMap(x);
  const list = Array.isArray(x) || util.types.isSet(x);
  if (isMap || list || (typeof x === 'object' && isPlainObject(x))) {
    if (open.has(x)) throw new Fail('MarshalError', `to_v2(): can't convert a ${typeName(x)} that contains itself`);
    const keys = isMap || list ? null : Object.keys(x);
    open.spend(2 * (keys ? keys.length : isMap || !Array.isArray(x) ? x.size : x.length));
    open.add(x);
    try {
      if (list) return Array.from(x, (v) => toV2(v, depth + 1, open));
      const pairs = [];
      const seen = new Set();
      const entries = isMap ? x.entries() : keys.map((k) => [k, x[k]]);
      for (const [k, v] of entries) {
        const key = keyText(k, x);
        if (seen.has(key)) throw new Fail('MarshalError', `to_v2(): can't convert ${typeName(x)} to V2 data (two keys become "${key}")`);
        seen.add(key);
        pairs.push([key, toV2(v, depth + 1, open)]);
      }
      orderedPending = true;
      const out = new OrderedPairs(pairs);
      return seen.has('$v2') ? { $v2: 'dict', v: out } : out;
    } finally {
      open.delete(x);
    }
  }
  for (const hook of ['to_dict', 'tolist']) {
    const f = x[hook];
    if (typeof f === 'function') {
      // The object stays on the path, so a hook that returns it (or holds it) is a cycle.
      if (open.has(x)) throw new Fail('MarshalError', `to_v2(): can't convert a ${typeName(x)} that contains itself`);
      if (open.hooks >= MAX_DEPTH) throw new Fail('MarshalError', `to_v2(): value nested deeper than ${MAX_DEPTH} levels`);
      open.add(x);
      open.hooks += 1;
      try {
        return toV2(Reflect.apply(f, x, []), depth, open);
      } finally {
        open.hooks -= 1;
        open.delete(x);
      }
    }
  }
  throw new Fail('MarshalError', `to_v2(): can't convert ${typeName(x)} to V2 data`);
}

function isEager(obj) {
  // Iterating it runs no user code, so `next` may return many items (11.4): a built-in
  // container whose iteration methods are the built-in ones (a subclass may override them).
  if (typeof obj === 'string') return true;
  if (!(Array.isArray(obj) || ArrayBuffer.isView(obj) || util.types.isMap(obj) || util.types.isSet(obj))) return false;
  return isNative(obj[Symbol.iterator]) && (!util.types.isMap(obj) || isNative(obj.entries));
}

const pendingItems = new Map(); // iterator handle id -> items taken but not sent yet (11.4)

function doOp(m) {
  const entry = handleEntry(m.h);
  const obj = entry.obj;
  switch (m.op) {
    case 'getattr': {
      const v = obj[m.name];
      return { value: encodeValue(typeof v === 'function' && !isConstructor(v) ? boundMethod(obj, v) : v, 'value') };
    }
    case 'setattr':
      obj[m.name] = decode(m.value);
      return { value: null };
    case 'getitem': {
      const k = decode(m.key);
      return { value: encodeValue(util.types.isMap(obj) ? obj.get(k) : obj[k], 'value') };
    }
    case 'setitem': {
      const k = decode(m.key);
      const v = decode(m.value);
      if (util.types.isMap(obj)) obj.set(k, v);
      else obj[k] = v;
      return { value: null };
    }
    case 'len': {
      const length = obj.length;
      const size = typeof length === 'number' ? null : obj.size;
      const n = typeof length === 'number' ? length : typeof size === 'number' ? size : null;
      if (n === null) throw new TypeError('object has no length');
      if (!Number.isSafeInteger(n) || n < 0) throw new TypeError(`object has an invalid length (${n})`);
      return { value: n };
    }
    case 'str':
      return { value: wellFormed(strOf(obj)) };
    case 'value':
      return { value: toV2(obj) };
    case 'iter': {
      const mapping = util.types.isMap(obj);
      let it;
      if (mapping) it = obj.entries();
      else {
        const f = obj == null ? undefined : obj[Symbol.iterator];
        if (typeof f !== 'function') throw new TypeError(`${typeName(obj)} is not iterable`);
        it = Reflect.apply(f, obj, []);
      }
      const tag = handleTag(it);
      handles.get(tag.id).batch = isEager(obj);
      return { value: { it: tag, mapping } };
    }
    case 'next': {
      // Items one at a time at their real depth; one that fails, or would make the reply too
      // big, waits for the next call so the items before it still arrive (11.4).
      const n = entry.batch ? Math.max(1, Number(m.n) || 1) : 1;
      let pending = pendingItems.get(m.h);
      if (!pending) pendingItems.set(m.h, (pending = []));
      const items = [];
      let done = false;
      let size = 0;
      while (items.length < n) {
        let x;
        if (pending.length) x = pending.shift();
        else {
          const r = obj.next();
          if (r === null || (typeof r !== 'object' && typeof r !== 'function')) throw new TypeError(`Iterator result ${String(r)} is not an object`);
          if (r.done) {
            done = true;
            break;
          }
          x = r.value;
        }
        const mark = counted.length;
        let j;
        try {
          j = encodeValue(x, 'item', 2); // inside {"items": [...]} an item sits two levels down
        } catch (e) {
          if (!(e instanceof Fail) || !items.length) throw e;
          rollbackCounts(mark);
          pending.unshift(x);
          break;
        }
        if (n > 1) size += Buffer.byteLength(JSON.stringify(j) || 'null');
        if (items.length && size > LIMIT / 2) {
          rollbackCounts(mark);
          pending.unshift(x);
          break;
        }
        items.push(j);
      }
      if (done) pendingItems.delete(m.h);
      return { value: { items, done } };
    }
  }
  throw new Fail('ProtocolError', `unknown op '${m.op}'`);
}

const HANDLERS = { init: doInit, load: doLoad, module: doModule, call: doCall, op: doOp };

// ---------------------------------------------------------------------------------------------
// The loop (14.2)

const plainErr = (id, kind, message) => ({ t: 'err', id, error: { type: kind, message, foreign_type: null, trace: [] } });

function trySend(m, frame) {
  try {
    send(frame);
    if (m.t === 'load') bindImporters(m.unit);
  } catch (e) {
    rollbackCounts();
    if (m.t === 'load') units.delete(m.unit); // a failed block for V2, so it's gone here too (12.1)
    if (e instanceof TooBig) send(plainErr(m.id, 'MarshalError', tooBigMessage(Number(e.message))));
    else send(plainErr(m.id, 'Error', `the reply could not be sent (${errorName(e)})`));
  }
}

function reply(m, result) {
  trySend(m, { t: 'ret', id: m.id, ...result });
}

function replyError(m, e) {
  rollbackCounts();
  let error;
  try {
    error = serialize(e);
  } catch (e2) {
    rollbackCounts();
    error = plainErr(m.id, 'Error', `the error could not be sent (${errorName(e2)})`).error;
  }
  trySend(m, { t: 'err', id: m.id, error });
}

async function handle(m) {
  // A request; at depth 1 its Promise may be awaited, deeper it can't (9.8).
  const entry = { kind: 'request', id: m.id };
  stack.push(entry);
  let onDry = null;
  try {
    let result = HANDLERS[m.t](m);
    if (isThenable(result)) {
      const dry = new Promise((_, reject) => {
        const what = m.t === 'load' ? "the block's value" : 'the result';
        onDry = () => reject(new Fail('Error', `${what} is a Promise that can never settle: nothing is left for it to wait on`));
        process.once('beforeExit', onDry);
      });
      result = await Promise.race([result, dry]);
    }
    reply(m, result);
  } catch (e) {
    replyError(m, e);
  } finally {
    if (onDry) process.removeListener('beforeExit', onDry);
    stack.splice(stack.lastIndexOf(entry), 1);
  }
}

function handleSync(m) {
  const entry = { kind: 'request', id: m.id };
  stack.push(entry);
  try {
    const result = HANDLERS[m.t](m);
    reply(m, result);
  } catch (e) {
    replyError(m, e);
  } finally {
    stack.pop();
  }
}

function bye() {
  closing = true;
  try {
    drainOut(true);
  } catch {}
  leave();
}

function dispatchOther(m) {
  // Frames other than requests and replies; false when there is nothing to do.
  if (m.t === 'release') {
    releaseHandles(Array.isArray(m.h) ? m.h : []);
  } else if (m.t === 'bye') {
    bye();
  } else if ('id' in m) {
    send({ t: 'err', id: m.id, error: { type: 'ProtocolError', message: `unknown frame type '${m.t}'`, foreign_type: null, trace: [] } });
  } else {
    fatal(`an unknown frame type '${m.t}' from V2`);
  }
}

function serveSync(waitId) {
  // A stub's wait (14.2): nested requests are served synchronously until the reply.
  for (;;) {
    const m = recv();
    if (m.t === 'ret' || m.t === 'err') {
      if (m.id !== waitId) fatal(`a reply to request ${m.id}, which V2's adapter is not waiting for`);
      if (m.t === 'ret') {
        try {
          return decode(m.value ?? null);
        } catch (e) {
          if (e instanceof Fail) throw new V2Error(e.kind, e.message);
          throw e;
        }
      }
      const error = m.error || {};
      if (error.v2 && typeof error.v2 === 'object') throw decodeTag(error.v2);
      throw new V2Error(error.type || 'Error', error.message || '');
    } else if (Object.prototype.hasOwnProperty.call(HANDLERS, m.t)) {
      handleSync(m);
    } else {
      dispatchOther(m);
    }
  }
}

async function main() {
  send({ t: 'hello', vep: VEP, version: process.versions.node, pid: process.pid, caps: ['modules', 'handles', 'reverse'] });
  for (;;) {
    const m = recv();
    if (m.t === 'ret' || m.t === 'err') fatal(`a reply to request ${m.id}, which V2's adapter is not waiting for`);
    if (Object.prototype.hasOwnProperty.call(HANDLERS, m.t)) {
      await handle(m);
      // One event-loop turn after each outermost reply: timers, finalizers (14.4, 11.9). V8
      // keeps WeakRef targets (every new stub) until the job ends, so a collection for many new
      // stubs comes after the turn, and its finalizers run in one more.
      await new Promise((r) => setImmediate(r));
      if (stubsSinceGc > GC_AFTER && stubsSinceGc > v8.getHeapStatistics().used_heap_size / GC_HEAP_PER_STUB) {
        gc();
        await new Promise((r) => setImmediate(r));
        stubsSinceGc = 0;
      }
    } else {
      dispatchOther(m);
    }
  }
}

main().catch((e) => fatal('internal error: ' + (e && e.stack ? e.stack : e)));
