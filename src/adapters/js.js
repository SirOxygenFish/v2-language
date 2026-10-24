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
const GC_AFTER = 1000; // fn stubs made since the last idle turn that force a collection (11.9)
const KINDS = ['ForeignError', 'ImportError', 'NameError', 'ValueError', 'MarshalError', 'ProtocolError', 'Error'];
const MAX_CAUSES = 8; // chained causes sent (9.6)
const ADAPTER = __filename;

// The protocol's own channel: frames go out only through the original fs.writeSync(1, ...) and
// come in through fs.readSync(0, ...); everything else that reaches fds 0-2 is wrapped below.
const rawWriteSync = fs.writeSync;
const rawReadSync = fs.readSync;
const rawExit = process.exit.bind(process);

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
  rawExit(0);
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

function writeFrame(obj) {
  const data = Buffer.from(JSON.stringify(obj) + '\n', 'utf8');
  if (data.length > LIMIT) throw new TooBig(String(data.length));
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

// ---------------------------------------------------------------------------------------------
// The request stack (14.2) and output (9.9)

const stack = []; // {kind: 'request'|'waiting', id}
let closing = false;
let outBuf = []; // [stream, text]
let outSize = 0;
const fnReleases = []; // callable ids whose stubs died (11.8)
let pinned = []; // stubs encoded into the frame being built (11.8)
let counted = []; // handle ids whose counts the frame being built added (10.1)

function rollbackCounts() {
  const ids = counted;
  counted = [];
  releaseHandles(ids);
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
process.on('uncaughtException', (e) => {
  writeOut('stderr', (e && e.stack ? e.stack : String(e)) + '\n');
});
process.on('unhandledRejection', (e) => {
  writeOut('stderr', 'unhandled rejection: ' + (e && e.stack ? e.stack : String(e)) + '\n');
});
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
    }
  }
}

const stubs = new Map(); // callable id -> WeakRef to its one stub
const stubCells = new WeakMap(); // stub -> {id, n}
let stubsSinceTurn = 0;
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
    stubsSinceTurn += 1;
  }
  stubCells.get(s).n += 1;
  return s;
}

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

function encode(x, depth = 0, open = new Set()) {
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
  if (t === 'string') return surrogateCheck(x);
  if (t === 'function' && stubCells.has(x)) {
    pinned.push(x);
    return { $v2: 'fn', id: stubCells.get(x).id };
  }
  if (t !== 'object') return handleTag(x);
  if (x instanceof V2Error) return errorTag(x, depth, open);
  if (util.types.isUint8Array(x)) return { $v2: 'bytes', b64: Buffer.from(x.buffer, x.byteOffset, x.byteLength).toString('base64') };
  if (Array.isArray(x)) {
    if (depth >= MAX_DEPTH || open.has(x)) return handleTag(x);
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
    open.add(x);
    try {
      const out = {};
      let tagged = false;
      for (const k of Object.keys(x)) {
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

function encodeValue(x, root) {
  // encode(x) for the frame member `root` (`value`, `args[0]`); a MarshalError says where (10.5).
  try {
    return encode(x);
  } catch (e) {
    if (e instanceof Fail && !e.located) {
      e.located = true;
      e.message += ` (at ${root}${e.steps.slice().reverse().join('')})`;
    }
    throw e;
  }
}

const textOrNull = (v) => (typeof v === 'string' ? wellFormed(v) : null);

function errorTag(e, depth = 0, open = new Set(), causes = 0) {
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
      return stubFor(j.id, j.name);
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

function rememberSource(file, first, source) {
  let lines = sources.get(file);
  if (!lines) sources.set(file, (lines = new Map()));
  source.split('\n').forEach((text, i) => lines.set(first + i, text));
}

function codePointCol(text, utf16col) {
  // A 1-based column in UTF-16 units as 1-based code points (12.6).
  return [...text.slice(0, Math.max(0, utf16col - 1))].length + 1;
}

const FRAME = /^\s*at (?:(.*?) \()?(.+?):(\d+):(\d+)\)?$/;

function frames(e) {
  const stack = e && typeof e.stack === 'string' ? e.stack : '';
  const out = [];
  for (const line of stack.split('\n')) {
    const m = FRAME.exec(line);
    if (!m) continue;
    let [, func, file, lineNo, col] = m;
    if (file.startsWith('node:') || file === ADAPTER || file.startsWith('evalmachine.')) continue;
    if (file.startsWith('file://')) file = url.fileURLToPath(file);
    const text = sources.get(file)?.get(Number(lineNo));
    out.push({
      file: wellFormed(file), line: Number(lineNo), col: text === undefined ? null : codePointCol(text, Number(col)),
      func: func ? wellFormed(func.replace(/^(?:async |new )/, '')) : null,
    });
  }
  return out;
}

function errorName(e) {
  if (e === null || (typeof e !== 'object' && typeof e !== 'function')) return typeof e;
  try {
    return typeof e.name === 'string' ? e.name : typeName(e);
  } catch {
    return 'Error';
  }
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
  if (depth === 0 && e && typeof e.stack === 'string') out.text = wellFormed(e.stack);
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

const IDENT = /^[\p{ID_Start}$_][\p{ID_Continue}$‌‍]*$/u;
const RESERVED = new Set(('break case catch class const continue debugger default delete do else enum export extends false finally ' +
  'for function if import in instanceof new null return super switch this throw true try typeof var void while with ' +
  'yield let static implements interface package private protected public await').split(' '));
const PROVIDED = new Set(['undefined', 'NaN', 'Infinity', 'globalThis', '__proto__']);

function checkName(k) {
  if (!IDENT.test(k) || RESERVED.has(k) || PROVIDED.has(k)) throw new Fail('ValueError', `block arg '${k}' is not a valid JavaScript name`);
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
  for (const k of NODE_GLOBALS) {
    const d = Object.getOwnPropertyDescriptor(globalThis, k);
    if (d.get || d.set) {
      Object.defineProperty(sb, k, {
        configurable: true, enumerable: d.enumerable,
        get: () => (d.get ? d.get.call(globalThis) : undefined),
        set(v) { Object.defineProperty(this, k, { value: v, writable: true, configurable: true, enumerable: true }); },
      });
    } else {
      Object.defineProperty(sb, k, d);
    }
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
      // V8 puts the location on the stack's first line: `file:line`.
      const loc = /^(.*):(\d+)\r?\n/.exec(e.stack || '');
      if (loc && loc[1] === m.file) e.v2Frame = { file: m.file, line: Number(loc[2]), col: null, func: null };
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
    // The body position: a `[line:col]` marker, or (Node 24) the stack's first line `:line`
    // with a caret under the column.
    const stack = e && typeof e.stack === 'string' ? e.stack : '';
    const marker = /\[(\d+):(\d+)\]/.exec(message);
    const header = /^[^\n]*:(\d+)\r?\n([^\n]*)\r?\n(\s*)\^/.exec(stack);
    let line = null;
    let col = null;
    if (marker) [line, col] = [Number(marker[1]), Number(marker[2])];
    else if (header) [line, col] = [Number(header[1]), [...header[3]].length + 1];
    if (line !== null) err.v2Frame = { file: m.file, line: m.line + line - 1, col, func: null };
    err.stack = stack || 'SyntaxError: ' + err.message;
    throw err;
  }
}

const units = new Map(); // wire unit id -> {ctx, get, kinds} for loaded definitions (14.2)

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
      if (before.has(k) || seen.has(k) || k.startsWith('_') || !k.isWellFormed()) continue;
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
  rememberSource(m.file, m.line, src);
  const ctx = newContext(m.file);
  for (const [k, v] of Object.entries(m.args)) {
    Object.defineProperty(ctx, checkName(k), { value: decode(v), writable: true, configurable: true, enumerable: true });
  }
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

const HANDLERS = { init: doInit, load: doLoad };

// ---------------------------------------------------------------------------------------------
// The loop (14.2)

const plainErr = (id, kind, message) => ({ t: 'err', id, error: { type: kind, message, foreign_type: null, trace: [] } });

function trySend(m, frame) {
  try {
    send(frame);
  } catch (e) {
    rollbackCounts();
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
        onDry = () => reject(new Fail('Error', "the block's value is a Promise that can never settle: nothing is left for it to wait on"));
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
  rawExit(0);
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
      // One event-loop turn after each outermost reply: timers, finalizers (14.4, 11.9).
      if (stubsSinceTurn > GC_AFTER) gc();
      stubsSinceTurn = 0;
      await new Promise((r) => setImmediate(r));
    } else {
      dispatchOther(m);
    }
  }
}

main().catch((e) => fatal('internal error: ' + (e && e.stack ? e.stack : e)));
