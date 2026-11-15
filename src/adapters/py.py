# V2's Python adapter: the worker side of VEP/1 (INTERNALS ch. 9-14, 14.3). V2 writes this file
# into its private run folder and starts it as `python -B -u py.py`. Python 3.10 is the floor.
#
# Everything runs under the __main__ guard, so a multiprocessing spawn child that re-imports
# __main__ starts no second adapter (14.3); such a child only learns why it can't import a
# block's code (the end of this file).

if __name__ == "__main__":
    # 1. Every stdlib module the adapter uses, before any project folder is on sys.path (14.3).
    import sys, os, io, json, ast, types, builtins, inspect, importlib, keyword, traceback
    import linecache, weakref, threading, signal, base64, codecs, collections, collections.abc, numbers, atexit

    if os.name == "nt":
        import msvcrt, ctypes

    VEP = 1
    LIMIT = 64 * 1024 * 1024  # bytes per frame, both directions (9.1)
    MAX_DEPTH = 256  # nested lists, dicts and errors (10.5)
    OUT_CHUNK = 1024 * 1024  # characters per `out` frame, well below LIMIT
    FLUSH_AT = 64 * 1024  # buffered characters that force a flush during a request
    IDLE_CAP = 1024 * 1024  # buffered characters a non-dispatch thread may add while idle (9.9)
    KINDS = ("ForeignError", "ImportError", "NameError", "ValueError", "MarshalError", "ProtocolError", "Error")
    MAX_CAUSES = 8  # chained causes sent (9.6)
    ADAPTER = os.path.normcase(os.path.abspath(__file__))
    # The import machinery's own frames, between an @import and the module it loads (12.6).
    IMPORTLIB = os.path.normcase(os.path.dirname(os.path.abspath(importlib.__file__)))
    DISPATCH = threading.get_ident()

    # 2. The protocol moves to private copies of fds 0 and 1; fd 0 becomes the null device and
    # fd 1 points at stderr, so C extensions, os.write(1, ...) and inheriting children write to
    # stderr and read end of input (9.9).
    IN = os.dup(0)
    OUT = os.dup(1)
    if os.name == "nt":
        msvcrt.setmode(IN, os.O_BINARY)
        msvcrt.setmode(OUT, os.O_BINARY)
    _null = os.open(os.devnull, os.O_RDONLY)
    os.dup2(_null, 0)
    os.close(_null)
    os.dup2(2, 1)
    if os.name == "nt":
        _k32 = ctypes.WinDLL("kernel32", use_last_error=True)
        _k32.SetStdHandle.argtypes = (ctypes.c_uint32, ctypes.c_void_p)
        _k32.SetStdHandle(ctypes.c_uint32(-10 & 0xFFFFFFFF), ctypes.c_void_p(msvcrt.get_osfhandle(0)))
        _k32.SetStdHandle(ctypes.c_uint32(-11 & 0xFFFFFFFF), ctypes.c_void_p(msvcrt.get_osfhandle(2)))
    # The host owns Ctrl+C (6.5).
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    if hasattr(signal, "SIGBREAK"):
        signal.signal(signal.SIGBREAK, signal.SIG_IGN)

    # 3. The adapter's own folder leaves sys.path; init puts the project root on it (9.3). Python
    # resolves symlinks in sys.path[0] (macOS's temp folder is one), so both sides are resolved.
    same = lambda a, b: os.path.normcase(os.path.realpath(a)) == os.path.normcase(os.path.realpath(b))
    if sys.path and same(sys.path[0] or ".", os.path.dirname(ADAPTER)):
        del sys.path[0]
    ORIGINAL_PATH = list(sys.path)

    class BlockNames:
        """A block's module is named for its place in this run (12.1), so a `__v2_` name that isn't
        loaded comes from somewhere else, such as a pickle another run saved (14.3)."""

        @staticmethod
        def find_spec(name, path=None, target=None):
            if name.startswith("__v2_"):
                raise ModuleNotFoundError(
                    "No module named %r: that is how V2 names a @py block's code, for its place in one run, so "
                    "nothing else can import it (pickle saves where each class it saved came from): define what you "
                    "pickle in a .py file next to your .v2 file, and import that in the block" % name, name=name)
            return None

    sys.meta_path.append(BlockNames)

    # ----------------------------------------------------------------------------------------
    # Errors

    class V2Error(Exception):
        """Raised when a call into V2 fails or is refused (9.6, 9.8, 14.2)."""

        def __init__(self, type="Error", message="", data=None, lang=None, foreign_type=None):
            Exception.__init__(self, message)
            self.type = type
            self.message = message
            self.data = data
            self.lang = lang
            self.foreign_type = foreign_type
            self._v2 = None  # the `error` tag it came as, sent back unchanged (9.6)

        def __reduce__(self):
            return (V2Error, (self.type, self.message, self.data, self.lang, self.foreign_type))

    # A builtin, so `except V2Error` works in blocks and in the modules they use (DOCS 8.4).
    builtins.V2Error = V2Error

    class Fail(Exception):
        """A failure the adapter detects itself, sent with its own kind (9.6)."""

        def __init__(self, kind, message):
            Exception.__init__(self, message)
            self.kind = kind
            self.message = message
            self.steps = []  # where inside a value, innermost first (10.5)
            self.located = False

    def well_formed(s):
        """Text with each lone surrogate replaced by U+FFFD (10.4)."""
        try:
            s.encode("utf-8")
            return s
        except UnicodeEncodeError:
            return "".join("�" if 0xD800 <= ord(c) <= 0xDFFF else c for c in s)

    def gone():
        """The host is gone: end of input or a broken pipe (9.1)."""
        os._exit(0)

    def fatal(what):
        """A line the adapter can't parse, or a reply it isn't waiting for (9.1)."""
        try:
            os.write(2, ("v2 py adapter: " + what + "\n").encode("utf-8", "replace"))
        finally:
            os._exit(2)

    # ----------------------------------------------------------------------------------------
    # Framing (9.1, 14.3)

    _rbuf = bytearray()

    def recv():
        """The next frame as a dict; None at end of input."""
        while True:
            i = _rbuf.find(b"\n")
            if i >= 0:
                line = bytes(_rbuf[:i])
                del _rbuf[: i + 1]
                try:
                    m = json.loads(line)
                except ValueError as e:
                    fatal("cannot parse a frame from V2: %s" % e)
                if not isinstance(m, dict) or not isinstance(m.get("t"), str):
                    fatal("a frame from V2 that is not an object with a 't'")
                return m
            try:
                chunk = os.read(IN, 65536)
            except OSError:
                return None
            if not chunk:
                return None
            _rbuf.extend(chunk)

    def dumps(obj):
        return json.dumps(obj, ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode("utf-8") + b"\n"

    def write_all(data):
        view = memoryview(data)
        while view:
            try:
                n = os.write(OUT, view)
            except OSError:
                gone()
            view = view[n:]

    class TooBig(Exception):
        pass

    def write_frame(obj):
        data = dumps(obj)
        if len(data) - 1 > LIMIT:  # the newline isn't part of the frame
            raise TooBig(len(data) - 1)
        write_all(data)

    def too_big_message(n):
        return "value too large to send to V2 (%d MiB, limit %d MiB)" % ((n + (1 << 20) - 1) >> 20, LIMIT >> 20)

    class Walk(set):
        """The containers open on the way down, and the least the frame can come to: a value too
        big stops here, before the time and memory of encoding all of it (9.1)."""

        cost = 0
        hooks = 0  # to_v2 hooks open on the way down (11.5)

        def spend(self, n):
            self.cost += n
            if self.cost > LIMIT:
                f = Fail("MarshalError", "value too large to send to V2 (more than %d MiB)" % (LIMIT >> 20))
                f.located = True
                raise f

    # ----------------------------------------------------------------------------------------
    # The request stack (14.2) and output (9.9)

    stack = []  # ("request", id) per host request being handled, ("waiting", id) per stub
    closing = [False]  # set while handling `bye`

    _send_lock = threading.Lock()
    # The thread inside send(): its own writes (a finalizer that prints mid-send) only buffer and
    # go out with the next frame, since taking the lock again would deadlock (14.2).
    _send_owner = [None]
    _out_cond = threading.Condition()
    _out_buf = []  # (stream, text)
    _out_size = [0]
    _fn_releases = collections.deque()  # callable ids whose stubs died (11.8)
    # Stubs encoded into the frame being built stay alive until it is written, so their release
    # can never go out ahead of a frame that names them (11.8).
    _pinned = []
    # Handle ids whose counts the frame being built added: given back if it is never sent (10.1).
    _counted = []

    def rollback_counts(mark=0):
        """The counts added since `mark` were for tags that won't be sent (10.1)."""
        ids = _counted[mark:]
        del _counted[mark:]
        release_handles(ids)

    def active():
        return bool(stack) or closing[0]

    def drain_out():
        """Under the send lock: buffered output as `out` frames, in order."""
        with _out_cond:
            items = _out_buf[:]
            del _out_buf[: len(items)]
            _out_size[0] = sum(len(t) for _, t in _out_buf)
            _out_cond.notify_all()
        merged = []
        for stream, text in items:
            if merged and merged[-1][0] == stream:
                merged[-1][1].append(text)
            else:
                merged.append((stream, [text]))
        for stream, parts in merged:
            text = well_formed("".join(parts))
            for i in range(0, len(text), OUT_CHUNK):
                # "stderr!": text that reports an error no V2 call could raise (9.9).
                frame = {"t": "out", "stream": stream.rstrip("!"), "text": text[i : i + OUT_CHUNK]}
                if stream.endswith("!"):
                    frame["error"] = True
                write_frame(frame)

    def send(frame):
        """Buffered output, then queued releases as one frame, then `frame` (14.2)."""
        with _send_lock:
            _send_owner[0] = threading.get_ident()
            try:
                for b in _byte_streams:
                    b.finish()
                drain_out()
                ids = []
                while _fn_releases:
                    ids.append(_fn_releases.popleft())
                if ids:
                    write_frame({"t": "release", "fn": ids})
                try:
                    write_frame(frame)
                except TooBig:
                    rollback_counts()
                    raise
                del _counted[:]
            finally:
                del _pinned[:]
                _send_owner[0] = None

    def flush_out():
        if not active() or _send_owner[0] == threading.get_ident():
            return
        with _send_lock:
            _send_owner[0] = threading.get_ident()
            try:
                drain_out()
            finally:
                _send_owner[0] = None

    class Output(io.TextIOBase):
        """sys.stdout and sys.stderr: text sent as `out` frames in program order (9.9)."""

        def __init__(self, stream, fd):
            self._stream = stream
            self._fd = fd
            self.buffer = BytesOutput(self)

        @property
        def encoding(self):
            return "utf-8"

        @property
        def errors(self):
            return "strict"

        @property
        def name(self):
            return "<" + self._stream + ">"

        def writable(self):
            return True

        def isatty(self):
            return False

        def fileno(self):
            # stderr is fd 2. stdout is V2's, with no descriptor here: fd 1 is stderr, and
            # console-aware libraries (click) would write to it (9.9).
            if self._fd == 2:
                return 2
            raise io.UnsupportedOperation("fileno: a block's stdout is V2's and has no file descriptor; "
                                          "a child process of a block writes to stderr (DOCS 4.6)")

        @property
        def line_buffering(self):
            return True

        def reconfigure(self, *, encoding=None, errors=None, newline=None, line_buffering=None, write_through=None):
            # V2's output is UTF-8 and goes out line by line: only that can be asked for.
            if encoding is not None and codecs.lookup(encoding).name != "utf-8":
                raise ValueError("a block's %s is V2's, which is UTF-8, not %s" % (self._stream, encoding))

        def write(self, s):
            if not isinstance(s, str):
                raise TypeError("write() argument must be str, not " + type(s).__name__)
            if not s:
                return 0
            self.buffer.finish()  # bytes before this text that never became a character
            return self._put(s)

        def _put(self, s):
            if forked[0]:
                # A forked child writes at the fd level, like any child of a block (9.9), also
                # through a stream saved before the fork (a logging handler).
                data = s.encode("utf-8", "backslashreplace")
                while data:
                    data = data[os.write(self._fd, data):]
                return len(s)
            with _out_cond:
                if threading.get_ident() != DISPATCH:
                    while not active() and _out_size[0] >= IDLE_CAP:
                        _out_cond.wait()
                _out_buf.append((self._stream, s))
                _out_size[0] += len(s)
                big = _out_size[0] >= FLUSH_AT
            if s.endswith("\n") or big:
                flush_out()
            return len(s)

        def flush(self):
            if not forked[0]:
                flush_out()

    class BytesOutput(io.RawIOBase):
        """sys.stdout.buffer: bytes decoded as UTF-8 (invalid bytes become U+FFFD)."""

        def __init__(self, text):
            self._text = text
            self._decoder = codecs.getincrementaldecoder("utf-8")("replace")
            _byte_streams.append(self)

        def writable(self):
            return True

        def write(self, b):
            data = bytes(b)
            text = self._decoder.decode(data)
            if text:
                self._text._put(text)
            return len(data)

        def flush(self):
            self._text.flush()

        def finish(self):
            """A sequence left incomplete becomes U+FFFD: before a text write to the stream, a
            reply or call, and at exit, so it stays in program order (9.9)."""
            if self._decoder.getstate()[0]:
                self._text._put(self._decoder.decode(b"", final=True))

    _byte_streams = []

    class NoInput(io.TextIOBase):
        """sys.stdin: V2's input() owns the user's stdin (9.9, DOCS 4.6)."""

        encoding = "utf-8"

        def readable(self):
            return True

        def isatty(self):
            return False

        def fileno(self):
            return 0  # the null device: a child given it reads end of input (9.9)

        def _refuse(self, *args, **kwargs):
            raise OSError("blocks cannot read standard input: V2's input() owns it; pass the data in as a block arg")

        read = readline = readlines = _refuse

        def __iter__(self):
            self._refuse()

    sys.stdout = Output("stdout", 1)
    sys.stderr = Output("stderr", 2)
    sys.stdin = NoInput()
    sys.__stdout__, sys.__stderr__, sys.__stdin__ = sys.stdout, sys.stderr, sys.stdin

    forked = [False]

    def forked_child():
        """A forked child (multiprocessing's default on Linux) is not the worker: it closes the
        protocol, writes to fds 1 and 2 like any child of a block, fd 1 being stderr, and ends
        as a Python process does (9.9)."""
        forked[0] = True
        for fd in (IN, OUT):
            try:
                os.close(fd)
            except OSError:
                pass
        sys.excepthook = lambda t, e, tb: print_unattached("", e)

    if hasattr(os, "register_at_fork"):
        os.register_at_fork(after_in_child=forked_child)

    def final_flush():
        """At exit (bye, after atexit handlers that print): the last output (9.10)."""
        if forked[0]:
            return
        closing[0] = True
        try:
            for b in _byte_streams:
                b.finish()
            with _send_lock:
                _send_owner[0] = threading.get_ident()
                drain_out()
        except Exception:
            pass

    atexit.register(final_flush)  # registered first, so it runs last

    # ----------------------------------------------------------------------------------------
    # Handles (11.1) and V2 function stubs (11.8)

    _handles = {}  # hid -> [obj, count, batch]
    _by_obj = {}  # id(obj) -> hid
    _next_hid = [0]

    def type_name(obj):
        t = type(obj)
        mod = getattr(t, "__module__", None)
        mod = mod if isinstance(mod, str) else ""
        qual = getattr(t, "__qualname__", t.__name__)
        # A class a block defines is named as written, not with the block's module (11.6).
        return qual if mod == "builtins" or not mod or mod.startswith("__v2_") else mod + "." + qual

    def handle_tag(obj):
        hid = _by_obj.get(id(obj))
        if hid is None:
            _next_hid[0] += 1
            hid = _next_hid[0]
            _handles[hid] = [obj, 0, False]
            _by_obj[id(obj)] = hid
        _handles[hid][1] += 1
        _counted.append(hid)
        tag = {"$v2": "h", "id": hid, "type": well_formed(type_name(obj))}
        try:
            if callable(obj):
                tag["callable"] = True
        except Exception:
            pass
        return tag

    def release_handles(ids):
        for hid in ids:
            entry = _handles.get(hid)
            if entry is None:
                continue
            entry[1] -= 1
            if entry[1] <= 0:
                del _handles[hid]
                _by_obj.pop(id(entry[0]), None)
                _pending.pop(hid, None)

    class _Cell(object):
        __slots__ = ("id", "n")

        def __init__(self, n):
            self.id = n
            self.n = 0

    class V2Function(object):
        """A V2 function inside Python: calling it calls V2 (11.8)."""

        def __init__(self, cell, name):
            self._cell = cell
            self.__name__ = name
            self.__qualname__ = name

        def __call__(self, /, *args, **kwargs):
            return host_call({"fn": self._cell.id}, args, kwargs, self.__name__)

        def __repr__(self):
            return "<V2 function " + self.__name__ + ">"

    class RouteFunction(object):
        """A function an inner @import brings in through V2: another engine's, or V2's (13.7)."""

        def __init__(self, route, name):
            self._route = route
            self.__name__ = name
            self.__qualname__ = name

        def __call__(self, /, *args, **kwargs):
            return host_call({"route": self._route}, args, kwargs, self.__name__)

        def __repr__(self):
            return "<V2 function " + self.__name__ + ">"

    _stubs = weakref.WeakValueDictionary()  # callable id -> its one stub

    def _release_fn(cell):
        for _ in range(cell.n):
            _fn_releases.append(cell.id)

    def stub_for(n, name):
        s = _stubs.get(n)
        if s is None:
            cell = _Cell(n)
            s = V2Function(cell, name or "function")
            _stubs[n] = s
            weakref.finalize(s, _release_fn, cell)
        s._cell.n += 1
        return s

    # ----------------------------------------------------------------------------------------
    # Marshaling (10.2-10.6)

    I64_MIN, I64_MAX = -(1 << 63), (1 << 63) - 1

    def surrogate_check(s):
        try:
            s.encode("utf-8")
        except UnicodeEncodeError as e:
            raise Fail("MarshalError", "string contains an unpaired surrogate (U+%04X) at index %d" % (ord(s[e.start]), e.start))
        return s

    def is_numpy(x, *names):
        t = type(x)
        return getattr(t, "__module__", "") == "numpy" and (not names or t.__name__ in names)

    def float_json(f):
        if f != f:
            return {"$v2": "float", "v": "nan"}
        if f in (float("inf"), float("-inf")):
            return {"$v2": "float", "v": "inf" if f > 0 else "-inf"}
        return f

    def encode(x, depth=0, path=None):
        """A Python value for V2 (10.6): plain data copied, anything else a handle."""
        t = type(x)
        if x is None:
            return None
        if t is bool:
            return x
        if t is int:
            return x if I64_MIN <= x <= I64_MAX else handle_tag(x)
        if t is float:
            return float_json(x)
        if path is None:
            path = Walk()
        if t is str:
            path.spend(len(x))
            return surrogate_check(x)
        if t is list or t is tuple:
            return encode_list(x, depth, path)
        if t is dict:
            return encode_dict(x, depth, path)
        if t is bytes:
            path.spend((len(x) + 2) // 3 * 4)
            return {"$v2": "bytes", "b64": base64.b64encode(x).decode("ascii")}
        if isinstance(x, bool) or is_numpy(x, "bool_", "bool"):
            return bool(x)
        if isinstance(x, numbers.Integral):
            n = int(x)
            return n if I64_MIN <= n <= I64_MAX else handle_tag(x)
        if isinstance(x, float) or (is_numpy(x) and isinstance(x, numbers.Real)):
            return float_json(float(x))
        if isinstance(x, str):
            path.spend(str.__len__(x))
            return surrogate_check(str.__str__(x))  # the content, not a subclass's __str__
        if isinstance(x, V2Function):
            _pinned.append(x)
            return {"$v2": "fn", "id": x._cell.id}
        if isinstance(x, RouteFunction):
            return {"$v2": "fn", "route": x._route}
        if isinstance(x, V2Error):
            return error_tag(x, depth, path)
        return handle_tag(x)

    def encode_value(x, root, depth=0):
        """encode(x) for the frame member `root` (`value`, `args[0]`), `depth` levels down in the
        value V2 decodes; a MarshalError says where (10.5)."""
        try:
            return encode(x, depth)
        except Fail as f:
            if not f.located:
                f.located = True
                # The whole value needs no path (10.5).
                if root != "value" or f.steps:
                    f.message += " (at %s%s)" % (root, "".join(reversed(f.steps)))
            raise

    def encode_list(x, depth, path):
        if depth >= MAX_DEPTH or id(x) in path:
            return handle_tag(x)
        path.spend(2 * len(x))
        path.add(id(x))
        out = []
        try:
            for i, v in enumerate(x):
                try:
                    out.append(encode(v, depth + 1, path))
                except Fail as f:
                    f.steps.append("[%d]" % i)
                    raise
        finally:
            path.discard(id(x))
        return out

    def encode_dict(x, depth, path):
        if depth >= MAX_DEPTH or not all(type(k) is str for k in x) or id(x) in path:
            return handle_tag(x)
        path.spend(2 * len(x))
        path.add(id(x))
        out = {}
        try:
            for k, v in x.items():
                try:
                    out[surrogate_check(k)] = encode(v, depth + 1, path)
                except Fail as f:
                    f.steps.append('["%s"]' % well_formed(k))
                    raise
        finally:
            path.discard(id(x))
        return {"$v2": "dict", "v": out} if "$v2" in out else out

    def error_tag(e, depth=0, path=None):
        """A V2Error as an `error` tag (10.3). One that came from V2 keeps its members (location,
        trace, the host's `ref`), but its data and cause are encoded again, so their handles and
        stubs belong to this frame (11.1, 11.8; the host restores the ones it sent, 9.6). An error
        is a nesting level, like a list or a dict (10.5)."""
        path = path if path is not None else Walk()
        if depth >= MAX_DEPTH or id(e) in path:  # too deep, or an error inside its own data
            return handle_tag(e)
        text = lambda v: well_formed(v) if isinstance(v, str) else None
        if e._v2 is not None:
            tag = dict(e._v2)
        else:
            tag = {"$v2": "error", "type": text(e.type) or "Error", "message": well_formed(str(e.message)),
                   "lang": text(e.lang), "foreign_type": text(e.foreign_type), "file": None, "line": None,
                   "col": None, "trace": []}
        path.add(id(e))
        try:
            tag["data"] = encode(e.data, depth + 1, path)
            cause = getattr(e, "_cause", None)
            tag["cause"] = error_tag(cause, depth + 1, path) if isinstance(cause, V2Error) else None
        finally:
            path.discard(id(e))
        return tag

    def decode(j):
        """A V2 value in Python (10.6), top down (10.1)."""
        if isinstance(j, list):
            return [decode(v) for v in j]
        if isinstance(j, dict):
            if "$v2" in j:
                return decode_tag(j)
            return {k: decode(v) for k, v in j.items()}
        return j

    def decode_tag(j):
        tag = j["$v2"]
        try:
            if tag == "h":
                entry = _handles.get(j["id"])
                if entry is None:
                    raise Fail("ProtocolError", "unknown handle id %s" % j["id"])
                return entry[0]
            if tag == "fn":
                return unit_attr(j["unit"], j["name"]) if "unit" in j else stub_for(j["id"], j.get("name"))
            if tag == "int":
                return int(j["v"])
            if tag == "bytes":
                return base64.b64decode(j["b64"], validate=True)
            if tag == "float":
                return {"nan": float("nan"), "inf": float("inf"), "-inf": float("-inf")}[j["v"]]
            if tag == "error":
                e = V2Error(j["type"], j["message"], decode(j.get("data")), j.get("lang"), j.get("foreign_type"))
                cause = j.get("cause")
                e._cause = decode(cause) if isinstance(cause, dict) else None
                e._v2 = j
                return e
            if tag == "dict":
                return {k: decode(v) for k, v in j["v"].items()}
        except Fail:
            raise
        except (KeyError, TypeError, ValueError, AttributeError):
            raise Fail("ProtocolError", "a malformed '%s' tag" % tag)
        raise Fail("ProtocolError", "an unknown '$v2' tag '%s'" % tag)

    # ----------------------------------------------------------------------------------------
    # Errors as `err` payloads (9.6, 12.6)

    _lines = {}  # .v2 path -> {line number: body text line}, kept by remember_source (12.6)

    def remember_source(path, first, source):
        lines = _lines.setdefault(path, {})
        for i, text in enumerate(source.split("\n")):
            lines[first + i] = text

    def frames(tb):
        """Innermost first; adapter frames and engine internals (the import machinery) left out
        (12.6)."""
        out = []
        for fs in reversed(traceback.extract_tb(tb)):
            name = fs.filename or ""
            if name.startswith("<frozen ") or os.path.normcase(os.path.abspath(name)) == ADAPTER:
                continue
            if os.path.normcase(os.path.dirname(os.path.abspath(name))) == IMPORTLIB:
                continue
            col = None
            text = _lines.get(name, {}).get(fs.lineno)
            colno = getattr(fs, "colno", None)  # Python 3.11+
            if text is not None and colno is not None:
                col = len(text.encode("utf-8")[:colno].decode("utf-8", "ignore")) + 1
            # A block's top level has no name; a module's keeps Python's `<module>` (2.5, 9.6).
            func = None if fs.name == "<module>" and name in _lines else well_formed(fs.name)
            out.append({"file": well_formed(name), "line": fs.lineno, "col": col, "func": func})
        return out

    def internal(fs):
        return fs.filename.startswith("<frozen ") or os.path.normcase(os.path.abspath(fs.filename)) == ADAPTER

    def by_line(fs):
        # A block's frame loses its column: Python would put the caret under the dedented text.
        if fs.filename in _lines:
            return traceback.FrameSummary(fs.filename, fs.lineno, fs.name, line=fs.line)
        return fs

    def print_unattached(header, e):
        """An exception with no V2 call to attach to (1.1), as Python prints it, with the frames
        12.6 keeps."""
        te = traceback.TracebackException.from_exception(e)
        todo, seen = [(te, e)], set()
        while todo:
            t, x = todo.pop()
            if t is not None and id(t) not in seen:
                seen.add(id(t))
                t.stack = traceback.StackSummary.from_list([by_line(fs) for fs in t.stack if not internal(fs)])
                # One from V2 says its V2 type and where V2 raised it, as on the way back (9.6).
                if isinstance(x, V2Error) and isinstance(x._v2, dict) and hasattr(t, "_str"):
                    t._str = "%s: %s" % (x.type, t._str)
                    trace = x._v2.get("trace") or []
                    if trace and isinstance(trace[0], str):
                        t.__notes__ = list(getattr(t, "__notes__", None) or []) + ["raised in V2 at " + trace[0]]
                todo += [(t.__cause__, getattr(x, "__cause__", None)), (t.__context__, getattr(x, "__context__", None))]
                todo += list(zip(getattr(t, "exceptions", None) or (), getattr(x, "exceptions", None) or ()))
        report(header + "".join(te.format()))

    def report(text):
        """An error no V2 call could raise, on V2's stderr and marked, so the run ends with exit 1
        (6.5, 9.9); a stderr of the program's own gets it as it is."""
        if forked[0] or not isinstance(sys.stderr, Output):
            sys.stderr.write(text)
            return
        with _out_cond:
            _out_buf.append(("stderr!", text))
            _out_size[0] += len(text)
        flush_out()

    def thread_hook(a):
        if a.exc_type is not SystemExit:
            name = a.thread.name if a.thread is not None else threading.get_ident()
            print_unattached("Exception in thread %s:\n" % name, a.exc_value)

    def unraisable_hook(u):
        try:
            what = "%s: %r" % (u.err_msg or "Exception ignored in", u.object) if u.object is not None else u.err_msg
        except Exception:
            what = u.err_msg or "Exception ignored in"
        if u.exc_value is None:
            report("%s\n%s\n" % (what or "Exception ignored", getattr(u.exc_type, "__name__", "")))
        else:
            print_unattached((what or "Exception ignored") + "\n", u.exc_value)

    threading.excepthook = thread_hook
    sys.unraisablehook = unraisable_hook

    def serialize(e, depth=0):
        """An exception as `err.error` (9.6)."""
        if isinstance(e, Fail):
            return {"type": e.kind, "message": well_formed(e.message), "foreign_type": None, "trace": []}
        trace = frames(e.__traceback__)
        # A SyntaxError's place is the error itself: a block's line, or the line of a module file
        # it couldn't compile (12.6, 13.5).
        if isinstance(e, SyntaxError) and isinstance(e.filename, str) and e.lineno and not e.filename.startswith("<"):
            trace.insert(0, {"file": well_formed(e.filename), "line": e.lineno, "col": e.offset or None, "func": None})
        if isinstance(e, V2Error):
            kind = e.type if e.type in KINDS else "ForeignError"
            out = {"type": kind, "message": well_formed(str(e.message)), "trace": trace,
                   "foreign_type": "V2Error" if kind != e.type else e.foreign_type}
            if e._v2 is not None:
                # An error from V2 goes back as it was (9.6). One made in Python is reported like
                # any exception, so its kind is filtered and it can't make V2 exit 3.
                try:
                    out["v2"] = error_tag(e)
                except Fail:
                    rollback_counts()
                    out["v2"] = dict(e._v2, data=None, cause=None)
            return out
        try:
            # A SyntaxError's location is in its frame, not repeated in the message.
            message = e.msg if isinstance(e, SyntaxError) and isinstance(e.msg, str) else str(e)
        except Exception:
            message = "<unprintable %s>" % type(e).__name__
        if isinstance(e, SystemExit):
            # As for JavaScript's process.exit() (DOCS 5).
            arg = "" if e.code is None else repr(e.code)
            message = "sys.exit(%s) can't end V2's Python engine; raise an error instead, or end the program with V2's exit(%s)" % (arg, arg)
        out = {"type": "ForeignError", "message": well_formed(message), "foreign_type": type(e).__name__,
               "trace": trace}
        if e.__cause__ is not None and depth < MAX_CAUSES:
            out["cause"] = serialize(e.__cause__, depth + 1)
        return out

    # ----------------------------------------------------------------------------------------
    # Awaiting (9.8)

    _loop = [None]

    def get_asyncio():
        if "asyncio" in sys.modules:
            return sys.modules["asyncio"]
        saved = sys.path[:]
        sys.path[:] = ORIGINAL_PATH  # the stdlib's asyncio, not a project file (9.3)
        try:
            import asyncio
        finally:
            sys.path[:] = saved
        return asyncio

    def await_if_awaitable(x, refusal):
        """x, awaited on the session's loop if awaitable; `refusal` is the error when a loop is
        already running in this thread (9.8)."""
        if not inspect.isawaitable(x):
            return x
        asyncio = get_asyncio()
        try:
            asyncio.get_running_loop()
            running = True
        except RuntimeError:
            running = False
        if running:
            if inspect.iscoroutine(x):
                x.close()
            raise Fail("Error", refusal)
        if _loop[0] is None:
            _loop[0] = asyncio.new_event_loop()
        return _loop[0].run_until_complete(x)

    # ----------------------------------------------------------------------------------------
    # Calls into V2 (14.2)

    _next_out_id = [0]

    def host_call(target, args, kwargs, name):
        if forked[0]:
            raise V2Error("Error", "V2 function '%s' called from a forked process" % name)
        if not stack or stack[-1][0] != "request":
            raise V2Error("Error", "V2 function '%s' called outside an active V2 call" % name)
        if threading.get_ident() != DISPATCH:
            raise V2Error("Error", "V2 function '%s' called from another thread" % name)
        if _send_owner[0] == DISPATCH:  # a finalizer that runs while a frame is being sent
            raise V2Error("Error", "V2 function '%s' called outside an active V2 call" % name)
        try:
            frame = {"t": "call", "id": 0, "target": target,
                     "args": [encode_value(a, "args[%d]" % i) for i, a in enumerate(args)],
                     "kwargs": {surrogate_check(k): encode_value(v, 'kwargs["%s"]' % k) for k, v in kwargs.items()}}
        except Fail as f:
            rollback_counts()
            raise V2Error(f.kind, f.message)
        _next_out_id[0] += 1
        frame["id"] = cid = _next_out_id[0]
        try:
            send(frame)
        except TooBig as t:
            raise V2Error("MarshalError", too_big_message(t.args[0]))
        stack.append(("waiting", cid))
        try:
            return serve(cid)
        finally:
            stack.pop()

    # ----------------------------------------------------------------------------------------
    # Requests

    _root = [None]
    units = {}  # wire unit id -> (namespace, {name: kind}); modules: (module, None) (14.2)

    def add_file_dir(path):
        """A .v2 file's folder, then the project root, lead sys.path at each load and module of
        that file, so its own folder wins over another .v2 file's (9.3, DOCS 7)."""
        d = os.path.dirname(path)
        root = _root[0]
        lead = [d] if root is None or d == root else [d, root]
        for p in lead:
            while p in sys.path:
                sys.path.remove(p)
        sys.path[0:0] = lead
        return lead

    def shadowed(name, lead):
        """A module the .v2 file's folders hold, while another of that name was loaded first (one
        V2 loaded itself, or another folder's): what Python would have used, unnoticed (13.5)."""
        top = name.split(".")[0]
        loaded = getattr(sys.modules.get(top), "__file__", None)
        if not isinstance(loaded, str):
            return None
        for p in lead:
            for mine in (os.path.join(p, top + ".py"), os.path.join(p, top, "__init__.py")):
                if os.path.isfile(mine):
                    same = os.path.normcase(os.path.abspath(mine)) == os.path.normcase(os.path.abspath(loaded))
                    return None if same else (top, loaded, mine)
        return None

    def check_name(k, what="block arg"):
        if not k.isidentifier() or keyword.iskeyword(k) or (k.startswith("__") and k.endswith("__")):
            raise Fail("ValueError", "%s '%s' is not a valid Python name" % (what, k))
        return k

    _absent = object()

    def module_attr(mod, n):
        """`n` of a module: an attribute, else its submodule (13.5); _absent when neither."""
        try:
            return getattr(mod, n)
        except AttributeError:
            pass
        sub = "%s.%s" % (mod.__name__, n)
        try:
            return importlib.import_module(sub)
        except ModuleNotFoundError as e:
            if e.name == sub:
                return _absent
            raise

    def unit_attr(unit, name):
        """`name` in a unit of this session, looked up now (12.3, 13.5)."""
        ns, kinds = units[unit]
        if kinds is None:
            v = module_attr(ns, name)
            if v is _absent:
                raise Fail("NameError", "'%s' is no longer defined" % name)
            return v
        g = ns.__dict__
        if name not in g:
            raise Fail("NameError", "'%s' is no longer defined" % name)
        return g[name]

    class UnitStub(object):
        """An inner @import from a block of this session that hasn't loaded yet (13.7). Its calls,
        attributes, isinstance and subclassing reach the target, or say why they can't."""

        def __init__(self, unit, name, route, alias):
            self._unit, self._name, self._route = unit, name, route
            self.__name__ = self.__qualname__ = alias

        def _current(self):
            u = units.get(self._unit)
            if u is None:
                host_call({"route": self._route}, (), {}, self.__name__)  # V2 says why it isn't here (13.6)
            if u[1] is not None and u[1].get(self._name) == "value":
                raise V2Error("ImportError", "'%s' is a constant — pass it in as a block arg" % self._name)
            try:
                return unit_attr(self._unit, self._name)
            except Fail as e:
                raise NameError(e.message)

        def __call__(self, /, *args, **kwargs):
            return self._current()(*args, **kwargs)

        def __getattr__(self, k):
            if "_unit" not in self.__dict__:  # half built by copy or pickle
                raise AttributeError(k)
            return getattr(self._current(), k)

        def __instancecheck__(self, x):
            return isinstance(x, self._current())

        def __subclasscheck__(self, c):
            return issubclass(c, self._current())

        def __mro_entries__(self, bases):
            return (self._current(),)

    _importers = {}  # unit -> (globals, alias, name, stub) of the blocks waiting for it (13.7)

    def bind_importers(unit):
        """Block `unit` has loaded, for V2 too: what imported from it gets the real thing."""
        waiting = _importers.pop(unit, ()) if unit in units else ()
        for g, alias, name, stub in waiting:
            if g.get(alias) is stub and units[unit][1].get(name) == "func":
                g[alias] = unit_attr(unit, name)

    def reply_lost(m):
        """A load whose reply can't go is a failed block for V2, so it's gone here too (12.1)."""
        u = units.pop(m.get("unit"), None) if m["t"] == "load" else None
        if u is not None:
            sys.modules.pop(u[0].__name__, None)

    def bind_imports(g, imports, keeps):
        for e in imports:
            alias = check_name(e["as"], "@import name")
            if "unit" not in e:
                g[alias] = RouteFunction(e["route"], alias)
            elif e["unit"] in units:
                g[alias] = unit_attr(e["unit"], e["name"])
            else:
                g[alias] = UnitStub(e["unit"], e["name"], e.get("route"), alias)
                if keeps:
                    _importers.setdefault(e["unit"], []).append((g, alias, e["name"], g[alias]))

    def osa(a, b):
        """Optimal string alignment distance, as V2's own did-you-mean counts it (2.4)."""
        d = [[i + j if i * j == 0 else 0 for j in range(len(b) + 1)] for i in range(len(a) + 1)]
        for i in range(1, len(a) + 1):
            for j in range(1, len(b) + 1):
                d[i][j] = min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + (a[i - 1] != b[j - 1]))
                if i > 1 and j > 1 and a[i - 1] == b[j - 2] and a[i - 2] == b[j - 1]:
                    d[i][j] = min(d[i][j], d[i - 2][j - 2] + 1)
        return d[-1][-1]

    def near_module(name):
        """The importable top-level module closest to `name`, within V2's did-you-mean limit (13.5):
        the standard library's, the built-in ones, and what sys.path holds."""
        if "pkgutil" not in sys.modules:
            saved = sys.path[:]
            sys.path[:] = ORIGINAL_PATH  # the stdlib's pkgutil, not a project file (9.3)
            try:
                import pkgutil
            finally:
                sys.path[:] = saved
        try:
            found = {m.name for m in sys.modules["pkgutil"].iter_modules()}
        except Exception:
            found = set()
        names = set(getattr(sys, "stdlib_module_names", ())) | set(sys.builtin_module_names) | found
        target = name.lower()
        limit = max(1, min(3, (len(target) + 2) // 3))
        best = None
        for c in sorted(names):
            if c == name or (c.startswith("_") and not name.startswith("_")) or abs(len(c) - len(target)) > limit:
                continue
            dist = osa(target, c.lower())
            if dist <= limit and (best is None or dist < best[0]):
                best = (dist, c)
        return best and best[1]

    def main_guard(node):
        """`if __name__ == "__main__":` (either way round), which a block's code never passes."""
        t = node.test if isinstance(node, ast.If) else None
        if not (isinstance(t, ast.Compare) and len(t.ops) == 1 and isinstance(t.ops[0], ast.Eq)):
            return False
        sides = [t.left, t.comparators[0]]
        name = any(isinstance(s, ast.Name) and s.id == "__name__" for s in sides)
        main = any(isinstance(s, ast.Constant) and s.value == "__main__" for s in sides)
        return name and main

    def unique_name(base):
        name, n = base, 1
        while name in sys.modules:
            n += 1
            name = "%s_%d" % (base, n)
        return name

    def defined_here(v, modname):
        """Whether `@export { * }` takes v: a class or a function the block defined (12.3)."""
        try:
            if inspect.isclass(v):
                return v.__module__ == modname
            f = inspect.unwrap(v) if callable(v) else None
            return inspect.isfunction(f) and f.__module__ == modname
        except Exception:
            return False

    def export_reply(g, modname, requested):
        names = [n for n in requested if n != "*"]
        exports, missing, seen = [], [], set()

        def add(n):
            seen.add(n)
            v = g[n]
            if callable(v):
                exports.append({"name": n, "kind": "func"})
            else:
                exports.append({"name": n, "kind": "value", "value": encode_value(v, n)})

        for n in names:
            if n in seen:
                continue
            if n in g:
                add(n)
            else:
                missing.append(n)
        if "*" in requested:
            for k, v in list(g.items()):
                if k not in seen and not k.startswith("_") and k == well_formed(k) and defined_here(v, modname):
                    add(k)
        return {"value": None, "exports": exports, "missing": missing}

    def do_init(m):
        root = m.get("project_root")
        if isinstance(root, str) and root:
            _root[0] = root
            while root in sys.path:
                sys.path.remove(root)
            sys.path.insert(0, root)
        return {"value": None}

    def do_load(m):
        path, first = m["file"], m["line"]
        add_file_dir(path)
        remember_source(path, first, m["source"])
        name = unique_name("__v2_" + m["unit"].replace(":", "_"))
        mod = types.ModuleType(name)
        g = mod.__dict__
        g["__file__"], g["__builtins__"] = path, builtins
        for k, v in m["args"].items():
            g[check_name(k)] = decode(v)
        bind_imports(g, m.get("imports") or [], m["kind"] != "action")
        try:
            # Under a name CPython can't open: given the .v2 path it rereads the error's line from
            # that file and counts the column on it, not on the body line it parsed (2.3).
            tree = ast.parse("\n" * (first - 1) + m["source"], "<v2 block>", "exec")
        except SyntaxError as e:
            e.filename = path
            raise e.with_traceback(None)  # its frame is the .v2 line, not ast's or ours (12.6)
        for node in tree.body:
            if main_guard(node):
                # A script pasted into a block would quietly run nothing (DOCS 4).
                e = SyntaxError("a block isn't the main module, so its `if __name__ == \"__main__\":` is never true: "
                                "run that code directly (call main()), or remove the if")
                e.filename, e.lineno, e.offset = path, node.lineno, node.col_offset + 1
                e.text = _lines.get(path, {}).get(node.lineno)
                raise e.with_traceback(None)
        tail = None
        if tree.body and isinstance(tree.body[-1], ast.Expr):
            tail = ast.Expression(tree.body.pop().value)
        try:
            code = compile(tree, path, "exec", dont_inherit=True)
            tail_code = compile(tail, path, "eval", dont_inherit=True) if tail is not None else None
        except SyntaxError as e:
            # The compiler's columns are UTF-8 byte offsets, the parser's are characters (12.6).
            text = _lines.get(path, {}).get(e.lineno)
            if text is not None and isinstance(e.offset, int) and e.offset > 0:
                e.offset = len(text.encode("utf-8")[:e.offset - 1].decode("utf-8", "ignore")) + 1
            raise e.with_traceback(None)
        sys.modules[name] = mod
        keep = False
        try:
            exec(code, g)
            value = eval(tail_code, g) if tail_code is not None else None
            value = await_if_awaitable(value, "the block's value is a coroutine while another coroutine is being awaited in this thread")
            if m["kind"] == "action":
                return {"value": encode_value(value, "value") if m["want_value"] else None, "exports": [], "missing": []}
            reply = export_reply(g, name, m["exports"])
            keep = not reply["missing"]
        finally:
            if not keep:
                sys.modules.pop(name, None)
        if keep:
            units[m["unit"]] = (mod, {e["name"]: e["kind"] for e in reply["exports"]})
        return reply

    # ----------------------------------------------------------------------------------------
    # Handles: calls and operations (9.5, 11.3-11.5)

    _pending = {}  # iterator handle id -> items taken but not sent yet (11.4)

    def handle_entry(hid):
        entry = _handles.get(hid)
        if entry is None:
            raise Fail("ProtocolError", "unknown handle id %s" % hid)
        return entry

    def eager(obj):
        """Iterating it runs no user code, so `next` may return many items (11.4)."""
        t = type(obj)
        if t in (list, tuple, dict, set, frozenset, str, bytes, range):
            return True
        if t.__module__ == "builtins" and t.__name__ in ("dict_keys", "dict_values", "dict_items"):
            return True
        return t.__module__ == "numpy" and t.__name__ == "ndarray"

    def do_module(m):
        """A module unit (13.5): imported once per unit, then the names asked for."""
        lead = add_file_dir(m["from"])
        unit = m["unit"]
        if unit not in units:
            name = m["name"]
            try:
                mod = importlib.import_module(name)
            except ModuleNotFoundError as e:
                # Only the module asked for (or a parent of it) missing is an ImportError; a
                # missing dependency is the module's own failure.
                if e.name is not None and (name == e.name or name.startswith(e.name + ".")):
                    message = str(e)
                    near = near_module(e.name) if "." not in e.name else None
                    if near:
                        message += ". Did you mean: %r?" % near  # as Python's own suggestions read
                    raise Fail("ImportError", message)
                raise
            units[unit] = (mod, None)
        clash = shadowed(m["name"], lead)
        if clash:
            raise Fail("ImportError", "py.%s is %s, which was loaded first, not your %s: give your file another name "
                       "(one Python serves the whole run, with one module per name)" % clash)
        mod = units[unit][0]
        names = m.get("names")
        if names is None:
            return {"value": handle_tag(mod), "exports": [], "missing": []}
        exports, missing, seen = [], [], set()
        for n in names:
            if not isinstance(n, str) or n != well_formed(n) or n in seen:
                continue
            seen.add(n)
            v = module_attr(mod, n)
            if v is _absent:
                missing.append(n)
            elif callable(v):
                exports.append({"name": n, "kind": "func"})
            else:
                exports.append({"name": n, "kind": "value", "value": encode_value(v, n)})
        if not missing:
            return {"value": None, "exports": exports, "missing": missing}
        # What it has, for V2's did-you-mean (9.4, 13.5).
        try:
            have = sorted(n for n in dir(mod) if isinstance(n, str) and not n.startswith("_") and n == well_formed(n))
        except Exception:  # a module whose __dir__ raises
            have = []
        return {"value": None, "exports": exports, "missing": missing, "names": have[:5000]}

    def do_call(m):
        target = m.get("target") or {}
        if "unit" in target:
            if target["unit"] not in units:
                raise Fail("ProtocolError", "unknown unit %s" % target["unit"])
            name = target["name"]
            f = unit_attr(target["unit"], name)
        elif "h" in target:
            obj = handle_entry(target["h"])[0]
            method = target.get("method")
            f = getattr(obj, method) if method is not None else obj
            name = method if method is not None else getattr(obj, "__name__", type_name(obj))
        else:
            raise Fail("ProtocolError", "a call target V2's Python adapter does not serve")
        args = [decode(a) for a in m.get("args") or []]
        kwargs = {k: decode(v) for k, v in (m.get("kwargs") or {}).items()}
        value = f(*args, **kwargs)
        value = await_if_awaitable(value, "'%s' returned a coroutine while another coroutine is being awaited; call it directly from V2" % name)
        return {"value": encode_value(value, "value")}

    def float_text(x):
        """V2's str() of a float: Rust's shortest form, `1e-5` not Python's `1e-05` (5.10)."""
        r = repr(x)
        if "e" in r:
            mant, exp = r.split("e")
            r = mant + "e" + str(int(exp))
        return r

    def key_text(k, mapping):
        if isinstance(k, str):
            return surrogate_check(str.__str__(k))
        if k is None:
            return "null"
        if isinstance(k, bool) or is_numpy(k, "bool_", "bool"):
            return "true" if k else "false"
        if isinstance(k, numbers.Integral):
            # Outside i64 it is a MarshalError, never a rounded float (numpy ints are Real too).
            if I64_MIN <= int(k) <= I64_MAX:
                return str(int(k))
        elif isinstance(k, float) or (is_numpy(k) and isinstance(k, numbers.Real)):
            return float_text(float(k))
        raise Fail("MarshalError", "to_v2(): can't convert %s to V2 data (key of type %s)" % (type_name(mapping), type_name(k)))

    def to_v2(x, depth=0, path=None):
        """`op value`: the object as plain data, converted here (11.5). `depth` is the level the
        value would sit at, so a container at 256 or deeper is too deep, as for the host (10.5)."""
        path = path if path is not None else Walk()
        t = type(x)
        if x is None or t is bool:
            return x
        if isinstance(x, bool) or is_numpy(x, "bool_", "bool"):
            return bool(x)
        if isinstance(x, numbers.Integral):
            n = int(x)
            if not I64_MIN <= n <= I64_MAX:
                raise Fail("MarshalError", "to_v2(): can't convert int to V2 data (it needs more than 64 bits)")
            return n
        if isinstance(x, float) or (is_numpy(x) and isinstance(x, numbers.Real)):
            return float_json(float(x))
        if isinstance(x, str):
            return encode(str.__str__(x), depth, path)
        if t is bytes:
            return encode(x, depth, path)
        if isinstance(x, (V2Function, RouteFunction, V2Error)):
            return encode(x, depth, path)  # V2's own values go back as they came (10.6)
        if depth >= MAX_DEPTH:
            raise Fail("MarshalError", "to_v2(): value nested deeper than %d levels" % MAX_DEPTH)
        mapping = isinstance(x, collections.abc.Mapping)
        if mapping or isinstance(x, (list, tuple, set, frozenset)):
            if id(x) in path:
                raise Fail("MarshalError", "to_v2(): can't convert a %s that contains itself" % type_name(x))
            path.spend(2 * len(x))
            path.add(id(x))
            try:
                if not mapping:
                    return [to_v2(v, depth + 1, path) for v in x]
                out = {}
                for k, v in x.items():
                    key = key_text(k, x)
                    if key in out:
                        raise Fail("MarshalError", 'to_v2(): can\'t convert %s to V2 data (two keys become "%s")' % (type_name(x), key))
                    out[key] = to_v2(v, depth + 1, path)
                return {"$v2": "dict", "v": out} if "$v2" in out else out
            finally:
                path.discard(id(x))
        for hook in ("to_dict", "tolist"):
            f = getattr(x, hook, None)
            if callable(f):
                # The object stays on the path, so a hook that returns it (or holds it) is a cycle.
                if id(x) in path:
                    raise Fail("MarshalError", "to_v2(): can't convert a %s that contains itself" % type_name(x))
                if path.hooks >= MAX_DEPTH:
                    raise Fail("MarshalError", "to_v2(): value nested deeper than %d levels" % MAX_DEPTH)
                path.add(id(x))
                path.hooks += 1
                try:
                    return to_v2(f(), depth, path)
                finally:
                    path.hooks -= 1
                    path.discard(id(x))
        raise Fail("MarshalError", "to_v2(): can't convert %s to V2 data: make plain data of it in a block first, as in @py(h) { list(h) } or @py(h) { vars(h) }" % type_name(x))

    def do_op(m):
        op = m.get("op")
        entry = handle_entry(m.get("h"))
        obj = entry[0]
        if op == "getattr":
            return {"value": encode_value(getattr(obj, m["name"]), "value")}
        if op == "setattr":
            setattr(obj, m["name"], decode(m.get("value")))
            return {"value": None}
        if op == "getitem":
            return {"value": encode_value(obj[decode(m.get("key"))], "value")}
        if op == "setitem":
            obj[decode(m.get("key"))] = decode(m.get("value"))
            return {"value": None}
        if op == "len":
            return {"value": len(obj)}
        if op == "str":
            return {"value": well_formed(str(obj))}
        if op == "value":
            return {"value": to_v2(obj)}
        if op == "iter":
            mapping = isinstance(obj, collections.abc.Mapping)
            it = iter(obj.items()) if mapping else iter(obj)
            tag = handle_tag(it)
            _handles[tag["id"]][2] = eager(obj)
            return {"value": {"it": tag, "mapping": mapping}}
        if op == "next":
            n = max(1, int(m.get("n") or 1)) if entry[2] else 1
            pending = _pending.setdefault(m.get("h"), collections.deque())
            items, done, size = [], False, 0
            while len(items) < n:
                if pending:
                    x = pending.popleft()
                else:
                    try:
                        x = next(obj)
                    except StopIteration:
                        done = True
                        break
                # Inside {"items": [...]} an item sits two levels down (10.5).
                mark = len(_counted)
                try:
                    j = encode_value(x, "item", 2)
                except Fail:
                    if not items:
                        raise
                    rollback_counts(mark)
                    pending.appendleft(x)
                    break
                size += len(dumps(j)) if n > 1 else 0
                if items and size > LIMIT // 2:
                    rollback_counts(mark)
                    pending.appendleft(x)
                    break
                items.append(j)
            if done:
                _pending.pop(m.get("h"), None)
            return {"value": {"items": items, "done": done}}
        raise Fail("ProtocolError", "unknown op '%s'" % op)

    HANDLERS = {"init": do_init, "load": do_load, "module": do_module, "call": do_call, "op": do_op}

    def plain_err(rid, kind, message):
        return {"t": "err", "id": rid, "error": {"type": kind, "message": message, "foreign_type": None, "trace": []}}

    def handle(m):
        rid = m.get("id")
        stack.append(("request", rid))
        with _out_cond:
            _out_cond.notify_all()  # idle writers may go on (9.9)
        try:
            try:
                reply = {"t": "ret", "id": rid}
                reply.update(HANDLERS[m["t"]](m))
            except BaseException:
                # A forked child has no one to answer: the error ends it, as in a Python script.
                if forked[0]:
                    raise
                e = sys.exc_info()[1]
                rollback_counts()
                try:
                    reply = {"t": "err", "id": rid, "error": serialize(e)}
                except BaseException as e2:
                    rollback_counts()
                    reply = plain_err(rid, "Error", "the error could not be sent (%s)" % type(e2).__name__)
            if forked[0]:
                raise SystemExit(0)  # its work is done; the worker answers, not it
            try:
                send(reply)
                if m["t"] == "load":
                    bind_importers(m["unit"])
            except TooBig as t:
                reply_lost(m)
                send(plain_err(rid, "MarshalError", too_big_message(t.args[0])))
            except BaseException as e:
                rollback_counts()
                reply_lost(m)
                send(plain_err(rid, "Error", "the reply could not be sent (%s)" % type(e).__name__))
        finally:
            stack.pop()

    def bye():
        closing[0] = True
        if not stack:
            sys.exit(0)  # normal finalization and atexit handlers run (9.10)
        # Inside a stub: no unwinding through user code.
        try:
            atexit._run_exitfuncs()
        finally:
            final_flush()
            os._exit(0)

    def serve(wait_id):
        """The loop (14.2): handles requests until the reply to `wait_id` (None: forever)."""
        while True:
            m = recv()
            if m is None:
                gone()
            t = m["t"]
            if t in ("ret", "err"):
                if wait_id is None or m.get("id") != wait_id:
                    fatal("a reply to request %s, which V2's adapter is not waiting for" % m.get("id"))
                if t == "ret":
                    try:
                        return decode(m.get("value"))
                    except Fail as f:
                        raise V2Error(f.kind, f.message)
                error = m.get("error") or {}
                tag = error.get("v2")
                if isinstance(tag, dict):
                    raise decode_tag(tag)
                raise V2Error(error.get("type", "Error"), error.get("message", ""))
            elif t == "release":
                release_handles(m.get("h") or [])
            elif t == "bye":
                bye()
            elif t in HANDLERS:
                handle(m)
            elif "id" in m:
                send({"t": "err", "id": m["id"], "error": {"type": "ProtocolError",
                      "message": "unknown frame type '%s'" % t, "foreign_type": None, "trace": []}})
            else:
                fatal("an unknown frame type '%s' from V2" % t)

    send({"t": "hello", "vep": VEP, "version": "%d.%d.%d" % sys.version_info[:3], "pid": os.getpid(),
          "caps": ["modules", "handles", "kwargs", "reverse"]})
    serve(None)

elif __name__ == "__mp_main__":
    # A child that multiprocessing started with spawn or forkserver runs this file as __mp_main__.
    # A function or class it was sent from a block is named by the block's module, which a new
    # Python can't import, so it says why and what to do instead (14.3).
    import sys

    class BlockNames:
        @staticmethod
        def find_spec(name, path=None, target=None):
            if name.startswith("__v2_"):
                raise ModuleNotFoundError(
                    "No module named %r: it is a @py block's code, which a new Python process can't import "
                    "(multiprocessing's spawn and forkserver start one): put the functions and classes you send to "
                    "other processes in a .py file next to your .v2 file, and import that in the block" % name, name=name)
            return None

    sys.meta_path.append(BlockNames)
