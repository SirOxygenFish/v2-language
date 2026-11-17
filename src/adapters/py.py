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
    import errno

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

    # A library's warning about a call V2 made (`warnings.warn(..., stacklevel=2)`, matplotlib's
    # missing glyphs) would name this file's own line in the temp folder: it says where it came
    # from instead (12.6).
    import warnings
    _show_warning = warnings.showwarning

    def show_warning(message, category, filename, lineno, file=None, line=None):
        if isinstance(filename, str) and os.path.normcase(os.path.abspath(filename)) == ADAPTER:
            try:
                (file if file is not None else sys.stderr).write("%s: %s (in a call from V2)\n" % (category.__name__, message))
            except Exception:
                pass
            return
        _show_warning(message, category, filename, lineno, file, line)

    warnings.showwarning = show_warning

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
    # resolves symlinks in sys.path[0] (macOS's temp folder is one), so both sides are resolved
    # (`same` is also shadowed()'s test of one file: one reached two ways that resolve apart, a
    # share of a local folder, \\localhost\C$\..., and its C:\... path, is one file too).
    def same(a, b):
        if os.path.normcase(os.path.realpath(a)) == os.path.normcase(os.path.realpath(b)):
            return True
        try:
            return os.path.samefile(a, b)
        except (OSError, ValueError):
            return False

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

    class AfterImport:
        """Changes made to a module when it is first imported, not at startup, where importing it
        would cost every run its time (shutil pulls in bz2 and lzma): `hooks` maps a module's name
        to a function given the module once it has run (9.9)."""

        hooks = {}

        @classmethod
        def find_spec(cls, name, path=None, target=None):
            hook = cls.hooks.get(name)
            if hook is None:
                return None
            for finder in sys.meta_path:
                find = getattr(finder, "find_spec", None) if finder is not cls else None
                spec = find(name, path, target) if find is not None else None
                if spec is not None:
                    break
            else:
                return None
            real = spec.loader
            if real is None or not hasattr(real, "exec_module"):
                return spec

            class Loader:
                def create_module(self, spec):
                    return real.create_module(spec)

                def exec_module(self, module):
                    real.exec_module(module)
                    hook(module)

                def __getattr__(self, attr):
                    return getattr(real, attr)

            spec.loader = Loader()
            return spec

    def after_import(name, hook):
        if name in sys.modules:
            hook(sys.modules[name])
        else:
            AfterImport.hooks[name] = hook

    sys.meta_path.insert(0, AfterImport)

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

        def __init__(self, kind, message, hint=None):
            Exception.__init__(self, message)
            self.kind = kind
            self.message = message
            self.hint = hint  # an ImportError's, sent as err.hint (9.6, 13.5)
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
        # to_v2's object, the hooks run on it (h.to_dict()...), and the container at level 0:
        # what a node that can't convert is an item of (11.5).
        top = base = None
        chain = ()

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
            self._held = ""  # a forked child's unfinished line, under _lock (its threads print too)
            self._lock = threading.RLock()
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
            # Where V2 writes this stream: a terminal or not (9.9).
            return _terminals[self._stream] is not None

        def fileno(self):
            # stderr is fd 2. stdout is V2's, with no descriptor here: fd 1 is stderr, and
            # console-aware libraries (click) would write to it (9.9). What to do instead depends
            # on who asks: subprocess (stdout=sys.stdout) or anyone else, such as curses.
            if self._fd == 2:
                return 2
            try:
                caller = sys._getframe(1).f_globals.get("__name__")
            except (AttributeError, ValueError):
                caller = None
            if caller in ("subprocess", "os"):
                raise io.UnsupportedOperation("fileno: a block's stdout is V2's and has no file descriptor: leave stdout= out "
                                              "(the output then goes to stderr), or capture it (capture_output=True, text=True) "
                                              "and print(r.stdout), or use V2's exec([…])")
            raise io.UnsupportedOperation("fileno: a block has no terminal of its own: full-screen libraries such as curses "
                                          "can't run in a block")

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
                # through a stream saved before the fork (a logging handler), a whole line per
                # write, so children printing at once (a Pool) don't split each other's lines.
                with self._lock:
                    held = self._held + s
                    cut = held.rfind("\n") + 1 if len(held) < FLUSH_AT else len(held)
                    self._held = held[cut:]
                    self._write_fd(held[:cut])
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
                return
            with self._lock:
                held, self._held = self._held, ""
                self._write_fd(held)

        def _write_fd(self, s):
            data = s.encode("utf-8", "backslashreplace")
            while data:
                data = data[os.write(self._fd, data):]

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

    if os.name != "nt":
        import getpass

        def no_getpass(prompt="Password: ", stream=None):
            """The worker has no terminal (8.3): getpass's fallback would warn that the input
            may be echoed and print the prompt, then fail on stdin (DOCS 4.6). On Windows it
            reads V2's console, as DOCS says."""
            raise OSError("getpass can't ask in a block: a block has no terminal, and V2's input() owns standard input: ask in V2 with input(prompt, echo: false) and pass the answer in as a block arg")

        getpass.getpass = no_getpass

    # Which of V2's stdout and stderr are terminals, and their (columns, rows), from init (9.3): a
    # block's sys.stdout and sys.stderr, os.isatty(1) and (2) and the terminal sizes answer for
    # where V2 writes them, as Python's own would there, so colors and live bars show on a terminal
    # and stay out of a file (9.9, DOCS 4.6). The size is the one at the worker's start.
    _terminals = {"stdout": None, "stderr": None}
    _os_isatty = os.isatty
    _os_terminal_size = os.get_terminal_size

    def _terminal_of(fd):
        """V2's stream behind a block's fd 1 or 2 (or None for another fd), and its size."""
        if isinstance(fd, int) and fd in (1, 2):
            return True, _terminals["stdout" if fd == 1 else "stderr"]
        return False, None

    def isatty(fd):
        ours, size = _terminal_of(fd)
        return size is not None if ours else _os_isatty(fd)

    def get_terminal_size(fd=1):
        ours, size = _terminal_of(fd)
        if not ours:
            return _os_terminal_size(fd)
        if size is None:
            raise OSError(errno.ENOTTY, os.strerror(errno.ENOTTY))
        return os.terminal_size(size)

    def shutil_terminal_size(fallback=(80, 24)):
        """shutil.get_terminal_size, with V2's stdout as the terminal it asks about."""
        def env(name):
            try:
                return int(os.environ[name])
            except (KeyError, ValueError):
                return 0
        columns, lines = env("COLUMNS"), env("LINES")
        if columns <= 0 or lines <= 0:
            size = _terminals["stdout"] or fallback
            columns = columns if columns > 0 else size[0] or fallback[0]
            lines = lines if lines > 0 else size[1] or fallback[1]
        return os.terminal_size((columns, lines))

    os.isatty = isatty
    os.get_terminal_size = get_terminal_size
    after_import("shutil", lambda m: setattr(m, "get_terminal_size", shutil_terminal_size))
    if os.name == "nt":
        # colorama (tqdm, click and others call its init) would turn a block's ANSI codes into
        # console calls on the worker's console handle, out of order with the text, which goes to
        # V2 as frames: V2 turned on VT processing for its consoles, so the codes pass through.
        after_import("colorama.ansitowin32", lambda m: setattr(m, "winapi_test", lambda: False))

    _streams = (Output("stdout", 1), Output("stderr", 2))
    sys.stdout, sys.stderr = _streams
    sys.stdin = NoInput()
    sys.__stdout__, sys.__stderr__, sys.__stdin__ = sys.stdout, sys.stderr, sys.stdin

    forked = [False]

    def forked_child():
        """A forked child (multiprocessing's default on Linux) is not the worker: it closes the
        protocol, writes to fds 1 and 2 like any child of a block, fd 1 being stderr, and ends
        as a Python process does (9.9)."""
        forked[0] = True
        # A lock another thread held at the fork stays held here; a line the parent (a forked
        # child itself) held is its own to write.
        for o in _streams:
            o._lock = threading.RLock()
            o._held = ""
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
        """A V2 function inside Python: calling it calls V2 (DOCS 8.2)."""

        def __init__(self, cell, name, params=None, more=False):
            self._cell = cell
            self.__name__ = name
            self.__qualname__ = name
            self._params = params
            self._more = more

        def __call__(self, /, *args, **kwargs):
            return host_call({"fn": self._cell.id}, args, kwargs, self.__name__)

        @property
        def __signature__(self):
            """The V2 func's parameters, for code that reads them (inspect.signature, 11.8);
            inspect is imported only then."""
            import inspect

            P = inspect.Parameter
            rest = [P("args", P.VAR_POSITIONAL), P("kwargs", P.VAR_KEYWORD)]
            try:
                named = [P(p, P.POSITIONAL_OR_KEYWORD) for p in self._params or ()]
                return inspect.Signature(named + rest if self._more or self._params is None else named)
            except ValueError:
                # A V2 name that is a Python keyword, or clashes with args/kwargs.
                return inspect.Signature(rest)

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

    def stub_for(n, name, params=None, more=False):
        s = _stubs.get(n)
        if s is None:
            cell = _Cell(n)
            s = V2Function(cell, name or "function", params, more)
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
                return unit_attr(j["unit"], j["name"]) if "unit" in j else stub_for(j["id"], j.get("name"), j.get("params"), bool(j.get("more")))
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
            out = {"type": e.kind, "message": well_formed(e.message), "foreign_type": None, "trace": []}
            if e.hint:
                out["hint"] = well_formed(e.hint)
            return out
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
        data = own_data(e)
        if data:
            out["data"] = data
        hint = shadow_hint(e) or decode_hint(e) or library_hint(e)
        if hint:
            out["hint"] = hint
        return out

    def library_hint(e):
        """A system library a package needs, missing (9.6): an ImportError, or an OSError raised
        while importing, `libGL.so.1: cannot open shared object file: …` (Linux's loader) or
        `PortAudio library not found` (ctypes.util's search, as sounddevice reports it)."""
        try:
            if not isinstance(e, (ImportError, OSError)):
                return None
            if not isinstance(e, ImportError):
                # Raised by a module's top level (Python trims the import machinery's frames).
                tb, importing = e.__traceback__, False
                while tb is not None and not importing:
                    code = tb.tb_frame.f_code
                    importing = code.co_name == "<module>" and code.co_filename.endswith(".py")
                    tb = tb.tb_next
                if not importing:
                    return None
            message = str(e)
            for mark in (": cannot open shared object file", " library not found"):
                if mark in message:
                    lib = os.path.basename(message.split(mark)[0].strip().split(" ")[-1].strip("'\""))
                    if lib:
                        return "%s is a system library the package needs, not a Python package: install it with your system's package manager" % lib
            return None
        except Exception:
            return None

    def shadow_hint(e):
        """A file of the user's named like a standard module, which some other module needed (9.6):
        `cannot import name 'Random' from 'random' (C:\\x\\random.py)`, or an AttributeError of
        that module."""
        try:
            if isinstance(e, ImportError):
                name, path = e.name, e.path
            elif isinstance(e, AttributeError) and isinstance(getattr(e, "obj", None), types.ModuleType):
                name, path = e.obj.__name__, getattr(e.obj, "__file__", None)
            else:
                return None
            if not isinstance(name, str) or name not in getattr(sys, "stdlib_module_names", ()) or not isinstance(path, str):
                return None
            where = os.path.normcase(os.path.abspath(path))
            import sysconfig  # only on this error path

            paths = sysconfig.get_paths()
            own = [paths.get(k) for k in ("stdlib", "platstdlib", "purelib", "platlib")]
            if any(isinstance(p, str) and where.startswith(os.path.normcase(os.path.abspath(p)) + os.sep) for p in own):
                return None
            return "your %s hides Python's own %s module, which other code needs: give your file another name" % (path, name)
        except Exception:
            return None

    def decode_hint(e):
        """A UnicodeDecodeError that V2's setup causes (9.6): blocks run Python in its UTF-8 mode
        (8.4), so open() reads UTF-8 where a plain Windows Python reads the ANSI code page. A UTF-16
        byte-order mark first (Excel's Unicode Text, PowerShell's Export-Csv -Encoding Unicode)
        wants encoding="utf-16", as a code page reads it as garbage; a first 0xff or 0xfe without
        one is a code page's letter ('ş' is 0xfe in cp1254) or a binary file's byte (a JPEG starts
        FF D8)."""
        try:
            if not isinstance(e, UnicodeDecodeError) or e.encoding != "utf-8":
                return None
            way = "blocks run Python in UTF-8 mode, so open() reads UTF-8: "
            first = bytes(e.object[:2]) if e.start == 0 else b""
            if first in (b"\xff\xfe", b"\xfe\xff"):
                return way + "text that starts with byte 0xff or 0xfe is probably UTF-16 (what Windows PowerShell's > and Out-File write): name it, as in open(path, encoding=\"utf-16\"), or pd.read_csv(path, encoding=\"utf-16\"); a binary file, such as an image, opens with open(path, \"rb\")"
            other = way + "for a file in another encoding, name it, as in open(path, encoding=\"cp1252\")"
            return other + "; a binary file, such as an image, opens with open(path, \"rb\")" if first[:1] in (b"\xff", b"\xfe") else other
        except Exception:
            return None

    def own_data(e):
        """An exception's own simple fields, as e.data in V2 (9.6): sys.exit(n)'s n as exit_code,
        an OSError's errno, strerror and filename, and the plain attributes it was given.
        Strings, ints within 64 bits, finite floats, booleans and None only; at most 32."""
        fields = []
        if isinstance(e, SystemExit):
            fields.append(("exit_code", 0 if e.code is None else e.code))
        if isinstance(e, OSError):
            fields += [("errno", e.errno), ("strerror", e.strerror), ("filename", e.filename), ("filename2", e.filename2)]
        try:
            fields += [(k, v) for k, v in vars(e).items() if isinstance(k, str) and not k.startswith("_")]
        except TypeError:
            pass
        data = {}
        for k, v in fields:
            if len(data) >= 32 or k in data or k == "$v2":
                continue
            if isinstance(v, str):
                data[well_formed(k)] = well_formed(v)
            elif v is None or isinstance(v, bool) or (type(v) is int and I64_MIN <= v <= I64_MAX):
                data[well_formed(k)] = v
            elif type(v) is float and v == v and abs(v) != float("inf"):
                data[well_formed(k)] = v
        # Fields the OS left unset say nothing.
        for k in ("errno", "strerror", "filename", "filename2"):
            if k in data and data[k] is None:
                del data[k]
        return data

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
            raise V2Error("Error", "V2 function '%s' called outside an active V2 call: Python called it while V2 wasn't waiting on Python (a thread, a timer or a finalizer after its call returned); call V2 from the code V2 called, and return to V2 when done" % name)
        if threading.get_ident() != DISPATCH:
            raise V2Error("Error", "V2 function '%s' called from another thread: Python may call V2 only from the thread V2 called it on: do the threaded work in Python and call V2 with the results" % name)
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
        except V2Error as e:
            # Its traceback starts at the call, not in the adapter's serve loop, and V2's frame
            # where it was raised is a note in it (Python 3.11+; 14.2).
            where = (e._v2 or {}).get("trace") or []
            if where and isinstance(where[0], str) and hasattr(e, "add_note") and not getattr(e, "__notes__", None):
                e.add_note("raised in V2 as %s, at %s" % (e.type, where[0]))
            raise e.with_traceback(None)
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
            # `math`, `time`, `sys`: built into Python, so a math.py of yours is never read.
            origin = getattr(getattr(sys.modules.get(top), "__spec__", None), "origin", None)
            if top in sys.modules and (top in sys.builtin_module_names or origin in ("built-in", "frozen")):
                for p in lead:
                    mine = os.path.join(p, top + ".py")
                    if os.path.isfile(mine):
                        return (top, "Python's built-in %s module" % top, mine)
            return None
        for p in lead:
            for mine in (os.path.join(p, top + ".py"), os.path.join(p, top, "__init__.py")):
                if os.path.isfile(mine):
                    # Real paths: a helpers.py that links to the one loaded is that module.
                    return None if same(mine, loaded) else (top, loaded, mine)
        return None

    def check_name(k, what="block arg"):
        """A block arg or an inner @import's alias, which the block's code reaches by name (12.2):
        else a ValueError that names one it can reach."""
        n = k
        if not k.isascii():
            import unicodedata  # only for a non-ASCII name, so startup stays lean

            n = unicodedata.normalize("NFKC", k)  # the name Python reads in the block's code
        dunder = n.startswith("__") and n.endswith("__")
        if n == k and k.isidentifier() and not keyword.iskeyword(k) and not dunder:
            return k
        arg = what == "block arg"

        def other(name):
            if arg:
                return "pass it under another name, as in @py(%s: …)" % name
            return "import it under another name, as in @import { … as %s } from …" % name

        if not (k.isidentifier() and n.isidentifier()):
            # A letter V2 takes and Python doesn't (ⓐ, which Python's NFKC form makes a).
            fit = n.isidentifier() and not keyword.iskeyword(n) and not dunder
            raise Fail("ValueError", "%s '%s' is not a valid Python name: %s" % (what, k, other(n if fit else "name")))
        if keyword.iskeyword(n):
            raise Fail("ValueError", "%s '%s' is not a valid Python name: %s" % (what, k, other(n + "_")))
        if dunder:  # __name__, __file__, __builtins__: the adapter's, or Python's
            s = n.strip("_") + "_"
            raise Fail("ValueError", "%s '%s' is reserved in @py blocks: %s" % (what, k, other(s if s[1:] and s.isidentifier() else "name")))
        how = "pass it under that name, as in @py(%s: %s)" % (n, k) if arg else "import it under that name, as in @import { … as %s } from …" % n
        was, now = changed(k, n)
        raise Fail("ValueError", "%s '%s' (%s) is read by Python as '%s' (%s): %s" % (what, k, was, n, now, how))

    def changed(k, n):
        """The code points of the first character NFKC changes in `k`, and of what it becomes in
        `n`: µ (U+00B5) and μ (U+03BC) look the same (12.2)."""
        import unicodedata

        i = 0
        while i < min(len(k), len(n)) and k[i] == n[i]:
            i += 1
        j = 0
        while j < min(len(k), len(n)) - i and k[-1 - j] == n[-1 - j]:
            j += 1
        a, b = k[i : len(k) - j], n[i : len(n) - j]
        # One character that changes on its own (µ of µµ); else the whole change (two Hangul
        # jamo that NFKC makes one syllable).
        c = a[:1]
        f = unicodedata.normalize("NFKC", c)
        if f != c and b.startswith(f):
            a, b = c, f
        return tuple(" ".join("U+%04X" % ord(ch) for ch in s) for s in (a, b))

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
        """An importable top-level module one edit from `name`, when `name` has six or more
        characters (13.5): the standard library's, the built-in ones, and what sys.path holds."""
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
        # Stricter than V2's own rule: most such names are packages not installed yet, and short
        # package names sit one edit from a standard module (lxml, xml; httpx, http; PIL, pip).
        if len(target) < 6:
            return None
        best = None
        for c in sorted(names):
            # Not my-helper.py, which no module name reaches (package_hint says so).
            if c == name or not c.isidentifier() or (c.startswith("_") and not name.startswith("_")) or abs(len(c) - len(target)) > 1:
                continue
            if osa(target, c.lower()) <= 1:
                best = best or c
        return best

    # Modules only one platform's Python has (13.5): sys.stdlib_module_names lists every
    # platform's, so another platform's module isn't one this Python lacks.
    UNIX_ONLY = ("fcntl", "grp", "posix", "pwd", "readline", "resource", "syslog", "termios")
    WINDOWS_ONLY = ("msvcrt", "nt", "winreg", "winsound", "_winapi", "_overlapped")

    def stdlib_hint(name):
        """The hint for a module of Python's standard library this Python doesn't have (13.5): one
        only another platform's Python has; on Linux, one the distribution packages apart
        (tkinter: python3-tk on Debian and Ubuntu); else one this Python lacks, as python.org's
        embeddable package lacks tkinter."""
        windows = sys.platform == "win32"
        if name in (UNIX_ONLY if windows else WINDOWS_ONLY):
            hint = "'%s' is only in Python on %s" % (name, "Linux and macOS" if windows else "Windows")
            return hint + ("; pyreadline3 is a substitute" if name == "readline" else "")
        if sys.platform.startswith("linux"):
            hint = "'%s' is part of Python's standard library, but this Python was installed without it" % name
            if name != "tkinter":
                return hint + "; pip can't add it"
            geteuid = getattr(os, "geteuid", None)
            apt = "apt update && apt install" if geteuid is not None and geteuid() == 0 else "sudo apt install"
            return hint + ": on Debian and Ubuntu, %s python3-tk" % apt
        return "'%s' is part of Python's standard library, which this Python lacks: pip can't add it; use a full Python" % name

    def package_hint(name, lead):
        """An ImportError's hint for a name Python has no module of (13.5): your own my-helper.py
        in the .v2 file's folders (`lead`), which no module name reaches; a module of the standard
        library this Python doesn't have (stdlib_hint); else a distribution of that name,
        installed, whose module is named otherwise (installed_as)."""
        if "." in name:
            return None
        under = name.replace("-", "_")
        if under.isidentifier() and not keyword.iskeyword(under):
            for d in lead:
                for entry in (name + ".py", under.replace("_", "-") + ".py", name, under.replace("_", "-")):
                    path = os.path.join(d, entry)
                    stem = entry[:-3] if entry.endswith(".py") else entry
                    mine = os.path.isfile(path) if entry.endswith(".py") else os.path.isfile(os.path.join(path, "__init__.py"))
                    if "-" in stem and mine:
                        new = under + (".py" if entry.endswith(".py") else "")
                        return "Python module names use _: rename %s to %s and import py.%s" % (entry, new, under)
        if name in getattr(sys, "stdlib_module_names", ()):
            return stdlib_hint(name)
        return installed_as(name)

    def installed_as(name):
        """`PyYAML is installed, and its module is named yaml: @import py.yaml as yaml`, for
        py.pyyaml, py.PyYAML or py."opencv-python": a distribution this Python has, found by pip's
        normalized name, whose modules are named otherwise (13.5)."""
        try:
            if "importlib.metadata" not in sys.modules or "importlib.util" not in sys.modules:
                saved = sys.path[:]
                sys.path[:] = ORIGINAL_PATH  # the stdlib's modules, not a project file (9.3)
                try:
                    importlib.import_module("importlib.metadata")
                    importlib.import_module("importlib.util")
                finally:
                    sys.path[:] = saved
            dist = sys.modules["importlib.metadata"].distribution(name)
            shown = dist.metadata.get("Name") or name
            mods = dist_modules(dist, shown)[:3]
            if not mods or name in mods:
                return None
            listed = mods[0] if len(mods) == 1 else ", ".join(mods[:-1]) + " and " + mods[-1]
            return "%s is installed, and its module%s named %s: @import py.%s as %s" % (
                shown, " is" if len(mods) == 1 else "s are", listed, mods[0], mods[0].rsplit(".", 1)[-1])
        except Exception:
            return None

    def dist_modules(dist, shown):
        """The modules a distribution provides, the closest to its name `shown` first (13.5): the
        names of its top_level.txt that Python finds (importlib.util.find_spec asks every finder,
        a pip install -e one's too, and imports nothing for a top-level name), one whose files the
        RECORD lists without its __init__.py by what is below it (a namespace: google-auth's
        google is google.auth and google.oauth2, PyQt5_sip's PyQt5 is PyQt5.sip), and none for a
        namespace it has no file in; without a top_level.txt (hatchling, scikit-build), its
        RECORD's: each file's first folder with an __init__.py, or a .py or extension module at
        the top, never a folder of data (bin, lib) or an editable install's own module
        (__editable__…). A name with a part starting with _ only when there are no others."""
        files = [tuple(f.parts) for f in dist.files or ()]
        files = [p for p in files if p and p[0] not in ("..", "__pycache__") and not p[0].endswith((".dist-info", ".data", ".egg-info"))]

        def module_file(last):
            return last.endswith((".py", ".pyd")) or (last.endswith(".so") and last.count(".") > 1)  # libllama.so is a library

        found, have = set(), set(files)
        top = (dist.read_text("top_level.txt") or "").split()
        for n in top:
            try:
                spec = importlib.util.find_spec(n) if n.isidentifier() else None
            except Exception:
                spec = None
            if spec is None:
                continue
            mine = [p for p in files if len(p) > 1 and p[0] == n and p[1] != "__pycache__"]
            if mine and (n, "__init__.py") not in have:
                below = {(n, p[1]) if len(p) > 2 else (n, p[1].split(".")[0]) for p in mine if len(p) > 2 or module_file(p[1])}
                found |= below or {(n,)}
            elif mine or spec.origin not in (None, "namespace"):
                found.add((n,))
        if not top:
            for parts in files:
                if parts[0].startswith(("__editable__", "_editable_impl_")):
                    continue
                dirs, last = parts[:-1], parts[-1]
                package = next((dirs[:i] for i in range(1, len(dirs) + 1) if dirs[:i] + ("__init__.py",) in have), None)
                if package is not None:
                    found.add(package)
                elif not dirs and module_file(last):
                    found.add((last.split(".")[0],))
        names = [".".join(p) for p in found if all(x.isidentifier() and not keyword.iskeyword(x) for x in p)]
        names = [n for n in names if not any(x.startswith("_") for x in n.split("."))] or names
        key = shown.lower().replace("-", "_").replace(".", "_")
        return sorted(names, key=lambda n: (osa(key, n.lower().replace(".", "_")), n))

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
        terminals = m.get("terminals")
        for name in _terminals:
            size = terminals.get(name) if isinstance(terminals, dict) else None
            if isinstance(size, list) and len(size) == 2 and all(isinstance(n, int) and n >= 0 for n in size):
                _terminals[name] = tuple(size)
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
        # A value asked for from a block that ends in a compound statement: its last line looks
        # like the value, but is inside the statement, so the value would be None (DOCS 4.5).
        compound = {ast.With: "with", ast.AsyncWith: "async with", ast.For: "for", ast.AsyncFor: "async for",
                    ast.If: "if", ast.Try: "try", ast.While: "while"}
        if hasattr(ast, "Match"):
            compound[ast.Match] = "match"
        if hasattr(ast, "TryStar"):
            compound[ast.TryStar] = "try"
        last = tree.body[-1] if tree.body else None
        no_value = m["kind"] == "action" and m.get("want_value") and type(last) in compound
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
            if no_value:
                # Once it has run to its end: a block that raises in its `try` says that instead.
                word = compound[type(last)]
                e = SyntaxError("the block's value is used, but its last statement is %s `%s` statement, which has no value: "
                                "put the value on a last line of its own, outside the `%s`" % ("an" if word == "if" else "a", word, word))
                e.filename, e.lineno, e.offset = path, last.lineno, last.col_offset + 1
                e.text = _lines.get(path, {}).get(last.lineno)
                raise e.with_traceback(None)
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
            if not all(p.isidentifier() and not keyword.iskeyword(p) for p in name.split(".")):
                # `py."opencv-python"`: a pip-style name, sent for package_hint (13.5).
                raise Fail("ImportError", "'%s' is not a valid Python module name" % name, package_hint(name, lead))
            try:
                mod = importlib.import_module(name)
            except ModuleNotFoundError as e:
                # Only the module asked for (or a parent of it) missing is an ImportError; a
                # missing dependency is the module's own failure.
                if e.name is not None and (name == e.name or name.startswith(e.name + ".")):
                    message = str(e)
                    hint = package_hint(e.name, lead)
                    near = near_module(e.name) if "." not in e.name and not hint else None
                    if near:
                        message += ". Did you mean: %r?" % near  # as Python's own suggestions read
                    raise Fail("ImportError", message, hint)
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
        # Your statistics.py next to the script, which Python found before its own (13.5).
        mine = getattr(mod, "__file__", None)
        if m["name"] in getattr(sys, "stdlib_module_names", ()) and isinstance(mine, str):
            where = os.path.normcase(os.path.abspath(mine))
            if any(where.startswith(os.path.normcase(os.path.abspath(p)) + os.sep) for p in lead):
                raise Fail("ImportError", "py.%s is your %s, which hides Python's own %s module, and it has no '%s': give your file another name"
                           % (m["name"], mine, m["name"], missing[0]))
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

    def key_text(k, mapping, depth, path):
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
        raise Fail("MarshalError", key_message(k, mapping, depth, path))

    def key_message(k, mapping, depth, path):
        """to_v2's message for a key that can't convert. Under the object's own to_dict() (a
        pandas Series indexed by dates) it names the object, with a block that converts it (11.5);
        a mapping's own key names the mapping, and V2 adds the hint (session.rs)."""
        name = type_name(k)
        if not path.chain:
            return "to_v2(): can't convert %s to V2 data (key of type %s)" % (type_name(mapping), name)
        top = path.top
        mod = getattr(type(top), "__module__", None)
        what = "a key of h" if depth == 0 else "a key inside h"
        if isinstance(mod, str) and mod.split(".")[0] == "pandas" and callable(getattr(top, "to_json", None)):
            how = '__import__("json").loads(h.to_json(date_format="iso"))'
        elif depth == 0:
            how = "{str(k): v for k, v in %s.items()}" % ("h" + "".join(".%s()" % c for c in path.chain))
        else:
            return "to_v2(): a key inside h has the type %s: make plain data of h in a block first, with str(k) for each such key k" % name
        return "to_v2(): %s has the type %s: make plain data of h in a block first, as in @py(h) { %s }" % (what, name, how)

    def to_v2(x, depth=0, path=None):
        """`op value`: the object as plain data, converted here (11.5). `depth` is the level the
        value would sit at, so a container at 256 or deeper is too deep, as for the host (10.5)."""
        if path is None:
            path = Walk()
            path.top = x
        t = type(x)
        if x is None or t is bool:
            return x
        if isinstance(x, bool) or is_numpy(x, "bool_", "bool"):
            return bool(x)
        if isinstance(x, numbers.Integral):
            n = int(x)
            if not I64_MIN <= n <= I64_MAX:
                raise Fail("MarshalError", "to_v2(): can't convert int to V2 data (it needs more than 64 bits): pass it as text, as in @py(h) { str(h) }")
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
            if depth == 0:
                path.base = x
            try:
                if not mapping:
                    return [to_v2(v, depth + 1, path) for v in x]
                out = {}
                for k, v in x.items():
                    key = key_text(k, x, depth, path)
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
                # A pandas or numpy object whose items alone pass the limit is refused before it
                # converts, which would hold all of it as Python objects (gigabytes) to fail anyway:
                # each item takes at least 2 bytes as sent ("0,"), 6 in to_dict's form ({"0":0,}).
                size = getattr(x, "size", None)
                if isinstance(size, numbers.Integral) and not isinstance(size, bool) and int(size) * (6 if hook == "to_dict" else 2) > LIMIT:
                    raise Fail("MarshalError", "value too large to send to V2 (%d items, more than %d MiB)" % (size, LIMIT >> 20))
                path.add(id(x))
                path.hooks += 1
                chain = path.chain
                if depth == 0:
                    path.chain = chain + (hook,)
                try:
                    return to_v2(f(), depth, path)
                finally:
                    path.hooks -= 1
                    path.chain = chain
                    path.discard(id(x))
        # A way that works for this object: its ISO text, its items, its fields, else its text.
        if callable(getattr(x, "isoformat", None)):
            form = "%s.isoformat()"
        elif hasattr(x, "__iter__"):
            form = "list(%s)"
        elif hasattr(x, "__dict__"):
            form = "vars(%s)"
        else:
            form = "str(%s)"
        if depth == 0 and not path.chain:
            raise Fail("MarshalError", "to_v2(): can't convert %s to V2 data: make plain data of it in a block first, as in @py(h) { %s }" % (type_name(x), form % "h"))
        raise Fail("MarshalError", item_message(x, form, depth, path))

    def item_message(x, form, depth, path):
        """to_v2's message for a node that can't convert and isn't the object asked for: an item
        of it, or what its to_dict() or tolist() gave. The block it names converts the object, as
        fits its kind: a pandas or numpy object as text, a container item by item (11.5)."""
        top, name = path.top, type_name(x)  # "has the type X": no article to pick (a uuid.UUID)
        src = "h" + "".join(".%s()" % c for c in path.chain)
        what = "an item of h" if depth else src
        mod = getattr(type(top), "__module__", None)
        if isinstance(mod, str) and mod.split(".")[0] in ("pandas", "numpy") and callable(getattr(top, "astype", None)):
            if callable(getattr(top, "tolist", None)):  # a Series, an Index, an array
                how = "h.astype(str).tolist()"
            else:  # a DataFrame: its numbers stay numbers
                how = 'import json; json.loads(h.to_json(date_format="iso"))'
        elif depth == 0:
            how = form % src
        elif depth == 1 and path.hooks == len(path.chain):
            if isinstance(path.base, collections.abc.Mapping):
                # A block's dict comes back as data only with str keys: other keys become their
                # text (an int's is V2's too), else the block's value would be a handle.
                try:
                    keys = "k" if all(isinstance(k, str) for k in path.base) else "str(k)"
                except Exception:
                    keys = "str(k)"
                how = "{%s: %s for k, v in %s.items()}" % (keys, form % "v", src)
            else:
                how = "[%s for x in %s]" % (form % "x", src)
        else:
            return "to_v2(): an item inside h has the type %s: make plain data of h in a block first, with %s for each such item x" % (name, form % "x")
        return "to_v2(): %s has the type %s: make plain data of it in a block first, as in @py(h) { %s }" % (what, name, how)

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
            # Two loop variables: a mapping, or anything with an items() method (a pandas
            # Series or DataFrame), gives each key and value (11.4).
            mapping = isinstance(obj, collections.abc.Mapping) or (
                bool(m.get("pairs")) and not isinstance(obj, (str, bytes)) and callable(getattr(obj, "items", None))
            )
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
        # PYTHONINSPECT (python -i) would print the SystemExit and wait for input instead.
        if not stack and not sys.flags.inspect:
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
