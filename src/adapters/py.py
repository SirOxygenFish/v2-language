# V2's Python adapter: the worker side of VEP/1 (INTERNALS ch. 9-14, 14.3). V2 writes this file
# into its private run folder and starts it as `python -B -u py.py`. Python 3.10 is the floor.
#
# Everything runs under the __main__ guard, so a multiprocessing spawn child that re-imports
# __main__ starts no second adapter (14.3).

if __name__ == "__main__":
    # 1. Every stdlib module the adapter uses, before any project folder is on sys.path (14.3).
    import sys, os, io, json, ast, types, builtins, inspect, importlib, keyword, traceback
    import linecache, weakref, threading, signal, base64, codecs, collections, collections.abc, numbers, atexit

    if os.name == "nt":
        import msvcrt, ctypes

    VEP = 1
    LIMIT = 64 * 1024 * 1024  # bytes per frame, both directions (9.1)
    MAX_DEPTH = 256  # nested lists/dicts (10.5)
    OUT_CHUNK = 1024 * 1024  # characters per `out` frame, well below LIMIT
    FLUSH_AT = 64 * 1024  # buffered characters that force a flush during a request
    IDLE_CAP = 1024 * 1024  # buffered characters a non-dispatch thread may add while idle (9.9)
    KINDS = ("ForeignError", "ImportError", "NameError", "ValueError", "MarshalError", "ProtocolError", "Error")
    MAX_CAUSES = 8  # chained causes sent (9.6)
    ADAPTER = os.path.normcase(os.path.abspath(__file__))
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

    # 3. The adapter's own folder leaves sys.path; init puts the project root on it (9.3).
    if sys.path and os.path.normcase(os.path.abspath(sys.path[0] or ".")) == os.path.dirname(ADAPTER):
        del sys.path[0]
    ORIGINAL_PATH = list(sys.path)

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
        if len(data) > LIMIT:
            raise TooBig(len(data))
        write_all(data)

    def too_big_message(n):
        return "value too large to send to V2 (%d MiB, limit %d MiB)" % ((n + (1 << 20) - 1) >> 20, LIMIT >> 20)

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
                write_frame({"t": "out", "stream": stream, "text": text[i : i + OUT_CHUNK]})

    def send(frame):
        """Buffered output, then queued releases as one frame, then `frame` (14.2)."""
        with _send_lock:
            _send_owner[0] = threading.get_ident()
            try:
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
            raise io.UnsupportedOperation("fileno")

        def write(self, s):
            if not isinstance(s, str):
                raise TypeError("write() argument must be str, not " + type(s).__name__)
            if not s:
                return 0
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
            self._text.write(self._decoder.decode(data))
            return len(data)

        def flush(self):
            self._text.flush()

        def finish(self):
            """At exit: a sequence left incomplete becomes U+FFFD."""
            self._text.write(self._decoder.decode(b"", final=True))

    _byte_streams = []

    class NoInput(io.TextIOBase):
        """sys.stdin: V2's input() owns the user's stdin (9.9, DOCS 4.6)."""

        encoding = "utf-8"

        def readable(self):
            return True

        def isatty(self):
            return False

        def fileno(self):
            raise io.UnsupportedOperation("fileno")

        def _refuse(self, *args, **kwargs):
            raise OSError("blocks cannot read standard input: V2's input() owns it; pass the data in as a block arg")

        read = readline = readlines = _refuse

        def __iter__(self):
            self._refuse()

    sys.stdout = Output("stdout", 1)
    sys.stderr = Output("stderr", 2)
    sys.stdin = NoInput()
    sys.__stdout__, sys.__stderr__, sys.__stdin__ = sys.stdout, sys.stderr, sys.stdin

    def final_flush():
        """At exit (bye, after atexit handlers that print): the last output (9.10)."""
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
        mod = getattr(t, "__module__", None) or ""
        qual = getattr(t, "__qualname__", t.__name__)
        return qual if mod == "builtins" or not mod else mod + "." + qual

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

        def __call__(self, *args, **kwargs):
            return host_call({"fn": self._cell.id}, args, kwargs, self.__name__)

        def __repr__(self):
            return "<V2 function " + self.__name__ + ">"

    class RouteFunction(object):
        """A function an inner @import brings in through V2: another engine's, or V2's (13.7)."""

        def __init__(self, route, name):
            self._route = route
            self.__name__ = name
            self.__qualname__ = name

        def __call__(self, *args, **kwargs):
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
        if t is str:
            return surrogate_check(x)
        if t is list or t is tuple:
            return encode_list(x, depth, path)
        if t is dict:
            return encode_dict(x, depth, path)
        if t is bytes:
            return {"$v2": "bytes", "b64": base64.b64encode(x).decode("ascii")}
        if isinstance(x, bool) or is_numpy(x, "bool_", "bool"):
            return bool(x)
        if isinstance(x, numbers.Integral):
            n = int(x)
            return n if I64_MIN <= n <= I64_MAX else handle_tag(x)
        if isinstance(x, float) or (is_numpy(x) and isinstance(x, numbers.Real)):
            return float_json(float(x))
        if isinstance(x, str):
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
                f.message += " (at %s%s)" % (root, "".join(reversed(f.steps)))
            raise

    def encode_list(x, depth, path):
        if depth >= MAX_DEPTH:
            return handle_tag(x)
        path = path if path is not None else set()
        if id(x) in path:
            return handle_tag(x)
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
        if depth >= MAX_DEPTH or not all(type(k) is str for k in x):
            return handle_tag(x)
        path = path if path is not None else set()
        if id(x) in path:
            return handle_tag(x)
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
        trace), but its data and cause are encoded again, so their handles and stubs belong to
        this frame (11.1, 11.8). An error tag is not a nesting level (10.5)."""
        path = path if path is not None else set()
        if id(e) in path:  # an error inside its own data
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
            tag["data"] = encode(e.data, depth, path)
            cause = getattr(e, "_cause", None)
            tag["cause"] = error_tag(cause, depth, path) if isinstance(cause, V2Error) else None
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
                return stub_for(j["id"], j.get("name"))
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
        """Innermost first; adapter frames and engine internals left out (12.6)."""
        out = []
        for fs in reversed(traceback.extract_tb(tb)):
            name = fs.filename or ""
            if name.startswith("<frozen ") or os.path.normcase(os.path.abspath(name)) == ADAPTER:
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

    def serialize(e, depth=0):
        """An exception as `err.error` (9.6)."""
        if isinstance(e, Fail):
            return {"type": e.kind, "message": well_formed(e.message), "foreign_type": None, "trace": []}
        trace = frames(e.__traceback__)
        if isinstance(e, SyntaxError) and e.filename in _lines and e.lineno:
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
        out = {"type": "ForeignError", "message": well_formed(message), "foreign_type": type(e).__name__,
               "trace": trace}
        if e.__cause__ is not None and depth < MAX_CAUSES:
            out["cause"] = serialize(e.__cause__, depth + 1)
        if depth == 0:
            try:
                out["text"] = well_formed("".join(traceback.format_exception(type(e), e, e.__traceback__)))
            except Exception:
                pass
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
    _file_dirs = set()
    units = {}  # wire unit id -> (namespace, {name: kind}); modules: (module, None) (14.2)

    def add_file_dir(path):
        """A .v2 file's folder goes before the project root on sys.path at its first load (9.3)."""
        d = os.path.dirname(path)
        if d in _file_dirs:
            return
        _file_dirs.add(d)
        root = _root[0]
        if d == root:
            return
        while d in sys.path:
            sys.path.remove(d)
        sys.path.insert(sys.path.index(root) if root in sys.path else 0, d)

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

    def unit_stub(unit, name, block, alias):
        """An inner @import from this session: calls it here, looked up at every call (13.7)."""

        def stub(*args, **kwargs):
            u = units.get(unit)
            if u is None:
                raise V2Error("ImportError", "%s has not run yet — '%s' can't be called before it loads" % (block, name))
            if u[1] is not None and u[1].get(name) == "value":
                raise V2Error("ImportError", "'%s' is a constant — pass it in as a block arg" % name)
            try:
                f = unit_attr(unit, name)
            except Fail as e:
                raise NameError(e.message)
            return f(*args, **kwargs)

        stub.__name__ = stub.__qualname__ = alias
        return stub

    def bind_imports(g, imports):
        for e in imports:
            alias = check_name(e["as"], "@import name")
            if "route" in e:
                g[alias] = RouteFunction(e["route"], alias)
            else:
                g[alias] = unit_stub(e["unit"], e["name"], e.get("block"), alias)

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
        bind_imports(g, m.get("imports") or [])
        try:
            tree = ast.parse("\n" * (first - 1) + m["source"], path, "exec")
        except SyntaxError as e:
            raise e.with_traceback(None)  # its frame is the .v2 line, not ast's or ours (12.6)
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
        add_file_dir(m["from"])
        unit = m["unit"]
        if unit not in units:
            name = m["name"]
            try:
                mod = importlib.import_module(name)
            except ModuleNotFoundError as e:
                # Only the module asked for (or a parent of it) missing is an ImportError; a
                # missing dependency is the module's own failure.
                if e.name is not None and (name == e.name or name.startswith(e.name + ".")):
                    raise Fail("ImportError", str(e))
                raise
            units[unit] = (mod, None)
        mod = units[unit][0]
        names = m.get("names")
        if names is None:
            return {"value": handle_tag(mod), "exports": [], "missing": []}
        if names == "*":
            public = getattr(mod, "__all__", None)
            names = list(public) if public is not None else [k for k in list(vars(mod)) if not k.startswith("_")]
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
        return {"value": None, "exports": exports, "missing": missing}

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
        if isinstance(k, numbers.Integral) and I64_MIN <= int(k) <= I64_MAX:
            return str(int(k))
        if isinstance(k, float) or (is_numpy(k) and isinstance(k, numbers.Real)):
            return float_text(float(k))
        raise Fail("MarshalError", "to_v2(): can't convert %s to V2 data (key of type %s)" % (type_name(mapping), type_name(k)))

    def to_v2(x, depth=0, path=None):
        """`op value`: the object as plain data, converted here (11.5). `depth` is the level the
        value would sit at, so a container at 256 or deeper is too deep, as for the host (10.5)."""
        path = path if path is not None else set()
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
            return surrogate_check(str.__str__(x))
        if t is bytes:
            return {"$v2": "bytes", "b64": base64.b64encode(x).decode("ascii")}
        if isinstance(x, (V2Function, RouteFunction, V2Error)):
            return encode(x, depth, path)  # V2's own values go back as they came (10.6)
        if depth >= MAX_DEPTH:
            raise Fail("MarshalError", "to_v2(): value nested deeper than %d levels" % MAX_DEPTH)
        mapping = isinstance(x, collections.abc.Mapping)
        if mapping or isinstance(x, (list, tuple, set, frozenset)):
            if id(x) in path:
                raise Fail("MarshalError", "to_v2(): can't convert a %s that contains itself" % type_name(x))
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
                path.add(id(x))
                try:
                    return to_v2(f(), depth, path)
                finally:
                    path.discard(id(x))
        raise Fail("MarshalError", "to_v2(): can't convert %s to V2 data" % type_name(x))

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
            except BaseException as e:
                rollback_counts()
                try:
                    reply = {"t": "err", "id": rid, "error": serialize(e)}
                except BaseException as e2:
                    rollback_counts()
                    reply = plain_err(rid, "Error", "the error could not be sent (%s)" % type(e2).__name__)
            try:
                send(reply)
            except TooBig as t:
                send(plain_err(rid, "MarshalError", too_big_message(t.args[0])))
            except BaseException as e:
                rollback_counts()
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
