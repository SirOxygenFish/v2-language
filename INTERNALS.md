# V2 Internals — how everything works

> **Status: being built;** [`ROADMAP.md`](ROADMAP.md) "Where we are" says which parts work. Phases 0 and 1 are: the host, py, js and ts. Chapters 15 (compiled blocks), 16 (`extern`) and 17 (custom engines) and the parts of others about them specify Phases 2 and 3. [`DOCS.md`](DOCS.md) defines what V2 does and this file how it works; where the two disagree, DOCS.md wins and this file is fixed.

## Contents

- [0. Principles](#0-principles)
- [1. Architecture and crate layout](#1-architecture-and-crate-layout)
- [2. Source, spans and diagnostics](#2-source-spans-and-diagnostics)
- [3. Lexer](#3-lexer)
- [4. Grammar and AST](#4-grammar-and-ast)
- [5. Host runtime](#5-host-runtime)
- [6. Program execution and modules](#6-program-execution-and-modules)
- [7. Engine registry and resolution](#7-engine-registry-and-resolution)
- [8. Sessions and worker lifecycle](#8-sessions-and-worker-lifecycle)
- [9. The VEP/1 protocol](#9-the-vep1-protocol)
- [10. Codec and marshaling](#10-codec-and-marshaling)
- [11. Handles](#11-handles)
- [12. Block execution](#12-block-execution)
- [13. Import resolution and routing](#13-import-resolution-and-routing)
- [14. Built-in adapters](#14-built-in-adapters)
- [15. Compiled blocks: the build bridge](#15-compiled-blocks-the-build-bridge)
- [16. Native FFI layer](#16-native-ffi-layer)
- [17. Custom engines](#17-custom-engines)
- [18. Configuration](#18-configuration)
- [19. CLI, doctor and check](#19-cli-doctor-and-check)
- [20. Security model](#20-security-model)
- [21. Failure-mode matrix](#21-failure-mode-matrix)
- [22. Testing and conformance](#22-testing-and-conformance)

---

## 0. Principles
"ch. N" and "N.M" refer to this file; "DOCS N" refers to DOCS.md.
### 0.1 Rules
| Rule | What it means in code |
| ---- | --------------------- |
| **One backend** | One tree-walking interpreter (`runtime/interp.rs`) runs the AST. There is no bytecode, no alternate mode and no `--target`. Every feature exists once, so no execution path can drop an engine statement. |
| **Polyglot is the product** | The host is small on purpose (DOCS 3). Sessions, units and handles are core runtime objects, not plugins. When host convenience conflicts with crossing quality (exact numbers, handles instead of lossy copies, foreign lines mapped to `.v2` lines), crossing quality wins. |
| **Fail loudly** | Every unsupported situation raises an error of a listed kind (5.9) with a location. Forbidden: printing a warning and continuing; skipping a block whose engine is missing; stripping a directive; dropping named arguments; a catch-all builtin or method that fabricates `null`/`""`/`[]`; stringifying a value that cannot cross. A value that cannot cross exactly becomes a handle or an error. It is never rounded, truncated or replaced. Deliberate exceptions: `getenv`, `args` and the output of child processes (`exec`, a program's output as a block value, stderr tails) decode invalid UTF-8 leniently (U+FFFD); a foreign exception with no V2 call to attach to (a JS `uncaughtException` or `unhandledRejection`, an exception in a Python thread or `__del__`) is printed to stderr, with the frames 12.6 keeps, and the worker keeps running. |
| **One codec, one protocol** | Everything that leaves the process as data goes through `codec/` (serde_json; the `$v2` tags only in its VEP mode, `codec/vep`, ch. 10): VEP frames, `json_parse`/`json_stringify`, JSON reverse calls from compiled code (15.9) and `V2_ARG_*` values (17.4). Built-in and custom bridged adapters speak the same VEP/1 (ch. 9). There are no side channels. |
| **Spans everywhere** | Every token, AST node, thrown `ErrorValue` and block unit carries a `Span` or a `UnitLoc` (ch. 2). A thrown error without a location is a bug. |
| **Explicit tables** | Tags and aliases (7.2), scanning profiles (3.8), marshaling (ch. 10), C types (16.4) and error kinds (including `SyntaxError`, 2.4) are data in the source, and a test walks every row. Nothing converts "by best effort" outside a table. |
| **Data, never source** | V2 values are never inserted into foreign code. Block args and call arguments travel as data (DOCS 4.4, 16). A body is passed as written, with its common indentation removed and its directive lines blanked (4.4). The only foreign text V2 generates is compiled-block wrappers and shims (ch. 15). That text depends on declarations, never on runtime values. |
| **Machine-independent syntax** | Lexing and parsing never read config, the environment or toolchains, so a file gives the same AST and the same static errors on every machine. Tags are resolved at runtime (ch. 7). |
| **Nothing in the project** | Per-run files live in one private run directory. It is created on first use and removed on every exit path but one Windows case (20.3, 6.5). User-level files live only in `~/.v2/` (18.1). V2 writes nothing into the project; only user code does. |

---

## 1. Architecture and crate layout
`v2` is one binary crate (edition 2021, stable Rust). It targets 64-bit x86_64 and aarch64 on Windows (MSVC), Linux and macOS (ch. 16); 22.5 defines the CI matrix.
### 1.1 Processes and run pipeline
```text
 v2 host process
   main thread          argv, config; starts the watchdog and interrupt handler; runs and joins the interpreter (6.1)
   interpreter thread   syntax -> runtime::interp -> engines::units -> engines::session + vep (bridged engines)
                          | engines::runonly (run engines, compiled programs) | build + ffi (in process, never unloaded)
   watchdog thread      deadlines (8.5) and interrupts (6.5); never touches values, never writes frames
 workers, one per bridged engine, started at first need (12.7): python py.py (@py, py.<module>) | node js.js (@js, @ts, js.<pkg>) | custom adapter
   stdin: host->worker frames; stdout: worker->host frames; stderr inherited
 per-use children (stdin null, or exec's input): run engines, compiled programs, exec (5.10), cl/gcc/clang/rustc, probes
```
The run's steps are 6.1 (startup, syntax, run) and 6.5 (its end). Sessions and workers: ch. 8 and 9 (worker stdio: 9.9). Per-use children: 6.3 and 17.3. The rules every child process follows: 8.3.

### 1.2 Module map
| Module | Responsibility |
| ------ | -------------- |
| `main.rs` | Starts the threads (1.3), owns the run-dir guard and the exit paths (6.5), and maps outcomes to exit codes (19.1). |
| `cli/` | Hand-written argv parsing (19.1): `v2 FILE [args]`, `check FILE`, `doctor`, `cache clean`, `--version`, `--help`. |
| `config/` | Project root discovery, layered config (ch. 18), and the paths of `~/.v2` and the run dir. |
| `syntax/` | `span`: FileId, SourceMap, Span, UnitLoc (ch. 2). `token` and `lexer` (ch. 3). `scan`: raw bodies, profiles, fences, line states (3.7, 3.8). `directives`: body lines, `@export`/`@import`, dedent (4.4). `parser`: recursive descent (4.1); it and `check` run the static checks (4.5). `resolve`: scope layouts and name positions (5.6). `ast` (4.3). |
| `runtime/` | `value`, `env`, `interp`, `call`, `ops`, `methods`, `strings`, `builtins` (including the `Out` writer), `error` (ErrorValue, error kinds, traces, rendering per 2.4), and `module` (`import "…"`, the run-once cache, cycles). See ch. 5 and 6. |
| `codec/` | The single codec (ch. 10), one serde_json value model in two modes: `json`, the plain mode of the JSON builtins (10.1); `vep`, the VEP mode with the `$v2` tags, handles, callables and base64 (10.1-10.5). |
| `engines/` | `process`: the child rules (8.3: own session or process group and Job Object, `kill_tree`, exit observed before reaping, piped streams), the watchdog thread with its timers and registered children, and the interrupt path (1.3, 6.5, 8.5); every process V2 starts goes through it. `spec` (EngineSpec and the built-in table), `registry` and `resolve` (discovery, probes, `toolchains.json`): ch. 7. `session` and `vep`: ch. 8, 9. `units`: ch. 12. `imports`: ch. 13. `handles`: ch. 11. `runonly`: 17.3, 15.5. |
| `adapters/` | `py.py` and `js.js`, embedded with `include_str!` and installed before launch (14.1). |
| `build/` | `mod`, `cache`, `c`, `cpp`, `rust`, `shims` (ch. 15). |
| `ffi/` | `cdecl` parses C declarations: `extern` bodies for the parser, and definition heads for 15.3. Also `types`, `lib`, `call` (ch. 16). |
| `doctor/` | `v2 doctor` and `v2 check` (19.2, 19.3). |

### 1.3 Threads
- **Main thread.** Parses argv, loads config, starts the watchdog, installs the interrupt handler, then spawns the interpreter thread (stack size: ch. 5) and joins it (6.1). When the system won't reserve its 256 MiB stack (a virtual memory limit, `ulimit -v`), 64 MiB and then 16 MiB are tried, so recursion stops sooner (5.8's guard measures the stack it got). Under such a limit (`RLIMIT_AS`) no stack above a quarter of it is tried (`stack_cap`; 16 MiB always is), and on Linux with glibc main first sets `M_ARENA_MAX` to 1 (glibc gives each thread that allocates an arena of its own, each holding 64 MiB of address space: they used up a 120 MB limit before the program did, so even 30 MB now ends deep recursion in the error below), so the heap keeps room: a 256 MiB stack under 400 MB ended deep recursion in Rust's abort (`memory allocation of 104 bytes failed`, exit 134). On a smaller stack the guard's error says why it came early: `RecursionError: maximum call depth exceeded at N calls deep: a memory limit for this shell (ulimit -v) left V2 a smaller stack`. With none, `v2: cannot start the interpreter thread: <why>` and `a limit set for this shell may be too low: on memory (ulimit -v; V2 needs about 50 MB, deep recursion about 300 MB) or on processes and threads (ulimit -u)` (both give the same error), exit 1. On Unix SIGXFSZ is ignored, so a write past the file size limit (`ulimit -f`) fails with EFBIG, an IOError with `hint: the file size limit (ulimit -f) stops it: raise the limit, or write less`, instead of killing V2; the children V2 starts get the default back (8.3).
- **Interpreter thread.** The logical main thread: the only thread that touches `Value` (`Rc`/`RefCell`, not `Send`), session pipes, native libraries and `Out`, and the only one that spawns processes (8.3). It serves a worker's calls into V2 while it waits for that worker's reply (9.7, 9.8), and compiled code calls V2 on it, during the native call (15.9). Compiled and `extern` code runs on it, so APIs that insist on the process main thread (macOS AppKit) do not work in v1; host functions that native code calls wrap their bodies in `catch_unwind`.
- **Watchdog.** Enforces the startup, call, probe and grace deadlines by killing child trees (8.5), and carries out the interrupt path (6.5).
- **Interrupt handler.** Ctrl+C, Ctrl+Break, console close, SIGINT, SIGTERM, SIGHUP and SIGQUIT are handled by the host alone: the handler sets `INTERRUPTED` (on Windows it also wakes the watchdog and then blocks), and the watchdog ends the run (6.5).
- **Output order.** One writer, `Out` (5.10), carries `print` and worker `out` text and is flushed before native calls and child spawns, so language-level output of every engine interleaves in program order (9.9, 16.7).

### 1.4 Dependencies
| Crate | Used for | Why a crate |
| ----- | -------- | ----------- |
| `serde_json` (`preserve_order`, `float_roundtrip`, `unbounded_depth`) | codec, VEP frames, JSON builtins | Strict parsing (it rejects lone-surrogate escapes), escaping of every control character, exact float round-trip. The codec bounds nesting itself (10.1). |
| `toml` (`preserve_order`) | `v2.toml` | Full TOML. The file is read as `toml::Table` with hand-written key checks, so "unknown key" errors are exact (18.5); `preserve_order` keeps the first error in file order. |
| `indexmap` | `v2.toml`'s tables (ch. 18) | Insertion order with O(1) lookup, so engines and errors keep file order. |
| `libloading` | compiled blocks, `extern` | Portable `dlopen`/`LoadLibraryExW` that reports the OS error text (15.8, 16.2). |
| `libffi` | calls whose signature is known only at runtime | Per-ABI call frames (SysV, Win64, AAPCS64). The crate builds a vendored libffi, so building v2 needs a C compiler (and `ml64` on MSVC); the binary has no runtime dependency. |
| `sha2` | build and probe cache keys (15.6, 7.7) | Standard SHA-256. |
| `unicode-normalization` | a file name in another Unicode form (6.4, 19.1, 5.10), did-you-mean's accents (5.9), an `@export` name Python reads in its NFKC form (12.2) | Unicode's NFC, NFD and NFKC tables. |
| `libc` | `kill`, `setsid`, `sigaction`, `waitid`, `prctl(PR_SET_PDEATHSIG)`, `getentropy`, `fflush`, `_exit`, `tcgetattr`/`tcsetattr`, `ioctl(TIOCGWINSZ)` | Direct OS calls. |
| `windows-sys` | Job Objects; `CREATE_NEW_PROCESS_GROUP`; `SetConsoleCtrlHandler`; `SetConsoleMode` (VT processing, 9.9); the run-dir DACL and `BCryptGenRandom` (20.3); `SetDllDirectoryW` and `LoadLibraryExW` flags (16.2, 15.8) | Direct OS calls. |

Not used: an async runtime, a CLI or regex crate (the few patterns in ch. 7 and 15 are hand-written matchers), `rand`, serde derive.

---

## 2. Source, spans and diagnostics
### 2.1 Source files
```rust
pub struct FileId(pub u32);        // index into SourceMap::files; the entry script is 0
pub struct SourceFile {
    pub path: PathBuf,             // span::canonicalize with `\\?\` stripped: the dedupe key, and the path adapters get (12.2)
    pub display: Rc<str>,          // what messages and e.file print
    pub text: Rc<str>,             // normalized text (3.1); every offset refers to it
    pub line_starts: Vec<u32>,     // byte offset of each line start
}
pub struct SourceMap { files: Vec<SourceFile>, by_path: HashMap<PathBuf, FileId> }
```
- **Ids** follow the startup parse order (6.1 step 2): the entry file is 0, then the files its `import "…"` literals name, followed transitively and depth-first in source order, as `v2 check` does (19.3). A file first seen at run time (6.4 step 1) gets the next id then. A path that is already known returns its existing id, so each file is read once per run and `UnitId`s stay stable.
- **Display path.** Under the canonicalized cwd, it is relative, with `/` on every OS (`src/util.v2`), unless the cwd is a filesystem root (`/`, `C:\`), where the path stays absolute. Otherwise it is the absolute path with `\\?\` stripped (`\\?\UNC\` becomes `\\`), but kept when a part ends in a dot or a space, which Windows would drop without it (`\\?\C:\x\run.v2.`, 5.10; `span::strip_verbatim`); so `script_dir()` can start with `\\?\`, where Windows reads no `/` and leaves `.` and `..` in, and 5.10's file builtins and `exec` (its program, and each argument that starts with `\\?\`, but not cmd's command line after `/c`: `builtins::verbatim_arguments`) read such a path's `/` as `\` and take out its `.` and `..` parts (`builtins::verbatim_parts`; no Windows name is `.` or `..` or holds `/`). `e.file` is `display`. Control characters in it are escaped as 5.10's paths are (`\n`, `\r`, `\t`, others as `\u{1b}`: `span::shown_path`), and so are the bidi embeddings, overrides and isolates (U+202A to U+202E, U+2066 to U+2069: `\u{202e}`, `span::is_bidi_format`), so a file name can't split a message, act on the terminal or show reversed (`rapor\u{202e}fdp.exe`), and so are WSL's stand-ins (U+F001 to U+F01F, U+F022, U+F02A, U+F03A, U+F03C, U+F03E, U+F03F, U+F05C and U+F07C, how Windows shows a Linux name's control characters and `" * : < > ? \ |`: `mod\u{f03a}x.v2`, `span::is_stand_in`), which show as nothing or as another character (the rest of U+F000 to U+F0FF, Nerd Fonts' icons among it, stays as it is); the paths V2 prints of its own files go through `shown_path`: diagnostic headers and `at` frames (2.4, 2.5), `ImportError` messages (`module::locate`, 6.4), check's summary (19.3), 19.1's messages about FILE (each line, so the listing of a folder's `.v2` files and a did-you-mean too), usage errors, and the `v2.toml` path in config errors (`config::read_file`, 18.5), in an engine's `(set by …)` (7.6) and on doctor's `config file:` line (19.2).
- **Only `.v2` files** are in the map. Config errors name the file and TOML key instead (18.5). Foreign files appear only inside frame strings (2.5).

### 2.2 Spans
```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span { pub file: FileId, pub lo: u32, pub hi: u32, pub line: u32, pub col: u32 }
```
- `lo..hi` is a byte range in the normalized text. `line` and `col` are the 1-based position of `lo`; `col` counts Unicode scalar values, and a tab counts as 1.
- A node's span runs from its first token's `lo` to its last token's `hi`, and copies the first token's `line`/`col`.
- `SourceMap::line_col(file, offset)` serves any other offset: a binary search in `line_starts`, then a char count.

The error caret points at:

| Raised by | Span |
| --------- | ---- |
| binary or unary operator | the operator token |
| call: arity, not callable, builtin failure, foreign call without a mapped frame | the callee's last name (`c` in `a.b.c(x)`), else the `(` |
| index or slice / member or `?.` | the `[` / the member name |
| `throw` | the `throw` keyword |
| block load, or an engine error without a mapped frame | the block's `@tag` |
| `@import` | the failing item, or the selector for selector errors |
| lexer, parser, static check | the offending character, token or declaration |

`error(...)` makes a value with no location. `throw` fills in the span and trace only when they are empty. So a rethrow keeps them, `e.line` is the first throw site, and an uncaught error's header and caret point at that `throw` (5.9).
### 2.3 Unit locations
```rust
pub struct UnitLoc {               // one per block, recorded by the parser (4.4); indexed per file by body line range
    pub file: FileId,
    pub body_first_line: u32,      // .v2 line of body line 1; sent as `line` in `load` (12.2)
    pub body_col: u32,             // .v2 column of body line 1, column 1
    pub dedent_width: u32,         // chars removed from each dedented line (4.4)
    pub line_count: u32,           // lines in the processed body
    pub kept: Rc<[u32]>,           // body lines left undedented (4.4 step 3), sorted
    pub escaped: Rc<[(u32, u32)]>, // `@@` escape lines (4.4): body line, column where the removed `@` was
}
impl UnitLoc { pub fn col(&self, body_line: u32, fcol: u32) -> Option<u32>; }  // None: past any line
```
- **Lines.** Adapters compile every unit with the `.v2` path as the file name and `body_first_line - 1` as the line offset (12.3, 12.4, 14.5). A frame inside a block therefore already carries its `.v2` file and line. Directive lines are blanked, never removed (4.4), so line numbers never drift.
- **Columns** arrive relative to the processed body line, in Unicode scalar values, or as `null` (12.6). Adapters convert V8's UTF-16 units and CPython's UTF-8 byte offsets first.
  - The host finds the block of the reporting engine whose body covers the frame's line. If several of its one-line blocks share that line, it takes the first.
  - It maps the column as follows: `body_col + fcol - 1` on body line 1, `fcol` on a `kept` line, `dedent_width + fcol` on any other line; on an escape line, one more from the removed `@` on. A column past any line counts as none.
  - Without a column, the result is the first non-blank column of the `.v2` line.
- **Everything else.** A frame outside every body (generated text), or in a foreign file, keeps its location as reported.
### 2.4 Diagnostics
An uncaught error, and every static diagnostic of the entry file, is printed to stderr as plain UTF-8, without colors:
```text
<file>:<line>:<col>: <Type>: [<foreign_type>: ]<first message line>
    <source line, leading whitespace removed>
    <spaces>^
    <other message lines, indented 4>
  note: <text> (<file>:<line>:<col>)
  hint: <text>
  at <frame>
  caused by: <Type>: [<foreign_type>: ]<first message line of the cause>
    at <frame of the cause>
```
- **Location.** Normally the error's span. For a `ForeignError`, it is the innermost foreign frame that maps into a `.v2` file, or the V2 call site if none maps. With no location at all (startup errors), the header is `v2: <Type>: …`.
- **A line of its own.** Before an uncaught error, and before `v2: cannot write to stdout` (6.5), V2 writes `\n` to stderr when the program left that line unfinished (`print("x", end: "")`): when the last byte written to stderr isn't `\n`, or, with stdout and stderr one terminal, pipe or file (`2>&1`), the last byte written to either (`builtins::start_line`; `Out` notes what it wrote, `stderr!` and `print(stderr: true)` theirs). The same stream is, on Unix, the same `fstat` device and inode of fds 1 and 2; on Windows, two console handles, one kernel object (`CompareObjectHandles`), or one file (`GetFileInformationByHandle`'s volume serial and file index).
- **Excerpt.** Printed whenever the location is in the SourceMap, for every error kind, `ForeignError` included.
  - Leading whitespace is removed, the rest is indented 4, and each tab prints as one space; other C0 control characters and DEL print as their Control Pictures symbol (U+2400 + code, U+2421), C1 controls as U+FFFD, so a stray escape sequence can't act on the terminal, and the bidi formatting characters as in 2.1 (`\u{202e}`), so a comment or string holding one can't reorder the line shown. The caret follows 4 spaces plus the terminal columns of what the characters before it (`col - 1 - removed` of them) print as: 2 for East Asian wide and fullwidth characters and the emoji terminals draw wide (✅, ⭐, 🚀), 0 for combining marks (Latin, Cyrillic, Hebrew, Arabic, Devanagari, Thai and the general blocks), zero-width characters and bidi marks (U+200E, U+200F), variation selectors and skin tones, else 1 (`error.rs` `columns`, a table of the common ranges, not all of Unicode), and 0 for a character right after a zero-width joiner (`👩🏽‍💻` is one glyph, two columns).
  - A line longer than 120 chars is cut to a 120-char window around the caret, with `...` at each cut end.
  - In multi-line messages (compiler output, foreign messages), the first line goes in the header and the rest follows the caret, indented 4.
- **Notes and hints.** Static diagnostics (`SyntaxError`, config errors) have notes and hints. A runtime error has a `hint` of its own only where the hint is about what the user wrote, not about the error (a V2 variable named inside a block, 12.1; a package that isn't installed, 13.5): it prints after the message lines and is never part of `e.message`, so a program that catches the error sees it as it was raised. Other runtime errors carry their hint in the message.
- **Habits.** A syntax error with no hint of its own gets one when the tokens where the parser stopped are another language's spelling (`parser.rs` `habit`, which tracks the innermost bracket still open): a `,` inside an index (`[` after a name, `)`, `]` or a string: `df.loc[r, "col"]`) → `an index takes one key: write x[a][b], or, for a pandas or numpy handle, do it in a block, as in @py(df) { df.loc[r, "col"] }`; `def`/`fn`/`function`/`fun`/`sub` NAME → `V2 writes functions as func NAME(a, b) { ... }`; `var x` → `V2 declares variables with let: let x = 1`; `await f()` → `V2 has no await: when a call returns a Promise or a coroutine, V2 waits for its result, so write f(…)`; `let mein name` → `a name is one word, without spaces: mein_name` (`let mein-name` → `a name can't have a - in it: mein_name`); `elif` starting a statement → `elif goes right after an if's }: if x { ... } elif y { ... }` (`else` → `else goes right after an if's }: …`), a second `else` after an `}` → `an if has one else, at its end: for more cases, put elif x { ... } before it`; `quadrat(x) {` at a line's start → `a function is written with func: func quadrat(…) { ... }`; two int literals around a `,` with no space, where one expression ends and not inside `( )` or `[ ]` (`let preis = 19,99`, `x = 2,5`, `return 2,5`, `if x > 2,5 {`, and the `${3,5:.2f}` of an f-string, whose error is `` expected `}` to close the ${…} ``) → `decimal numbers use a point: write 19.99, not 19,99` (the number as written), but when the whole run of literals joined so may be thousands (a first one of one to three digits that doesn't start with 0, then ones of exactly three: `1,500`, `1,000,000`; `lexer::decimal_comma_hint`) → `numbers are written without thousands commas: 1500 or 1_500 (a decimal one has a point: 1.5)` (the decimal one without its trailing zeros, `1.0` for `1,000`; past one comma only the thousands reading: `… 1000000 or 1_000_000`; the last literal may be a float of three digits, a point and digits, for the thousands reading alone: `1,234,567.89` → `… 1234567.89 or 1_234_567.89`), and another run of three or more (`1,5,2`), or a run with any other float (`19,99.5`, `1.5,000`, `1,000e5`, `1,000.5e3`), gets neither; `0,05`, `1,000` and `1,000,000.50`, whose `05` and `000` are the lexer's leading-zero error, get them too, and so does an `ArityError` of a builtin given more arguments than it takes, two or more of them written so: `sleep(0,5)`, `len(1,500)`, `sleep(1,234,567.89)`; `round(2,5)` is a call; `and`/`or` → `V2 writes and, or and not as &&, || and !`; `is` → `V2 compares with == and !=, as in x == null`; `=>` → `V2 writes an anonymous function as func(x) => x * 2`; `++` → `V2 has no ++: write x += 1`; a keyword with a capital at a statement's start (`Let x = 1`, `} Else {`) → `V2's own words are written in small letters: let`; a `,` inside `( )` that isn't a call's (`(1, 2)`) → `V2 has no tuples: write a list, as in [1, 2]`; `for` inside `[ ]` → `V2 has no comprehensions: write xs.map(func(x) => x * 2), or xs.filter(func(x) => x > 0)`; a `,` after `let a` → `V2 declares one name per let: let a = 1, then let b = 2 (or one list, let ab = [1, 2])`; after `return 1` → `V2 returns one value: return a list, as in return [a, b]`; a keyword where a name is declared (`let if = 1`) → `if is one of V2's own words: pick another name, as in if_`; a `{` that starts a statement and whose first entry has no `:` → the bare-block error below; JavaScript's `a ??= 5` → `V2 has no ??=: write a = a ?? …`; `} elsif` (`elseif`) → `V2 writes elif: } elif x > 0 { ... }`; `x := 1` → `V2 declares a name with let x = …, and assigns to it with x = …`; `--` at the end of a line → `V2 has no --: write x -= 1`; `===`/`!==` → `V2 compares with == (there is no ===)`; `..=` → `a..b leaves out b: for 1, 2 and 3 write 1..4`; `name=` in a call → `V2 writes a named argument as name: value, as in f(x, name: 2)`; `catch e` → `V2 writes catch (e) { ... }`; a `:` that ends a line → `V2 puts a block in { }, not after a colon: if x > 1 { ... }`; `function(x) {`/`fn(x) {` → `V2 writes an anonymous function as func(x) { ... }`; `lambda x` → `V2 writes an anonymous function as func(x) => x * 2`; an `=` in the condition of `if`, `elif` or `while` → `V2 compares with ==, as in if x == 2 { ... }`; `class NAME` → `V2 has no classes: keep the data in a dict and write functions for the rest, or write the class in a @py or @js block`; a second `:` inside `[ ]` (`xs[::2]`, `xs[1:5:2]`) → `V2 slices have no step: xs[a:b] takes the items from a up to b (a @py block has xs[::2])`; `...` after `[` or `,` (`[0, ...a]`) → `... spreads a list into a call's arguments only: join lists with +, as in [0] + a`; `**` right after the `(` or a `,` of a call (Python's `f(x, **opts)`; a call's `(` comes right after a name, `)`, `]` or a string) → `V2 has no ** for named arguments: write them by name, or call it in a block, as in @py(f, opts) { f(**opts) }`, of a `func` parameter list (Python's `def f(a, **kwargs)`: `func f(a, **kw)`, `func(**kw)`) → `V2 has no **kwargs parameter: take the options as one dict parameter, as in func f(a, opts) { … }`, and of a grouping (`(**x)`) no hint; `switch x`, Python's `match x`, or a line that starts with `switch` stopped at `{` (`switch (x) {`) → `V2 has no switch or match: write if x == 1 { ... } elif x == 2 { ... } else { ... }`; `raise ValueError("bad")` → `V2 throws an error with throw error("ValueError", "bad")` (the message as written, else `"text"`); `throw new Error("bad")` → `V2 has no new: throw error("Error", "bad"), or throw "bad" for a plain Error`, another `new Date()` → `V2 has no new: call it, as in Date(…)`; Python's `"big" if x > 2 else "small"` (an `if` in the middle of a line, not after `else`) → `V2 writes a conditional value as cond ? a : b, as in x > 2 ? "big" : "small"`; `print` followed by a string, name or number → `print is a function in V2: print("hello")`; Python's `from os.path import join, exists as e` at the start of a statement → `V2 imports names with @import { join, exists as e } from py.os.path` (another shape: `@import { mean } from py.statistics`); `js.date-fns` (also `py.`, `ts.`) → `a package name with - goes in quotes: js."date-fns"` (the name as written up to a space or punctuation, digits and keywords included: `js."utf-8-validate"`). Errors with hints of their own: `import` followed by a name: Python's `import os.path as p` → `a Python module is imported with @import py.os.path as p; another .v2 file with import "./util.v2" as util` (the alias as written, else the last part), JavaScript's `import dayjs from "dayjs"` → `V2 imports a JavaScript default export with @import { "default" as dayjs } from js.dayjs`; `import { a, b as c } from "x"` → `V2 imports names with @import { a, b as c } from js.x`; `import * as m from "x"` → `V2 imports a whole package with @import js.x as m`; any of these from a `.v2` file (`import { bump } from "./lib/side.v2"`, `import util from "./util.v2"`, `import * as u from "./util.v2"`) → `another .v2 file is imported with import "./lib/side.v2" as side, then side.bump(…)` (the alias as written, else the file's name; the `then` part for a `{ }` list's first name); a `:` after a parameter → `V2 has no type annotations; a default is written with =, as in func f(x, key = 1)`; Python's `*args` as a parameter → `V2 collects extra arguments with ...args` (the name as written; Python's keyword-only `*` alone, `func f(a, *, b)`, gets none); a third loop name (`for root, dirs, files in walk(d)`), a `(` or `[` where a loop name goes (`for i, (a, b) in …`, `for [a, b] in …`) and `for (a, b) in …` → `a for loop takes one name, or two (index or key, then item): for e in walk(d) { let root = e[0] … }, or loop in a @py block`; a `:` or a name after the error's name (`catch (e: Error)`, `catch (Exception e)`) → `V2 catches every error: catch (e) { … }, then test e.type`; `func by_len(s) => len(s)` → `a named function has a { } body: func by_len(s) { return … }, or write let by_len = func(s) => …`; `try { … }` followed by `except` (on its line or the next) → `V2 writes except as catch: try { ... } catch (e) { ... }`; `@import NAME from SEL`, SEL as written up to a space: a package (`py.…`, `js.…`, `ts.…`, or a quoted name, JavaScript's `from "dayjs"`, as `js.dayjs`) → `write @import { NAME } from SEL, or @import SEL as NAME for the whole module`, for `js.`/`ts.` with `(a default export: @import { "default" as NAME } from SEL)` before the comma; anything else (a label, `@py.stats`) → `write @import { NAME } from SEL`; `@import js.date-fns as m` → `a package name with - goes in quotes: @import js."date-fns" as m`. A package name in these hints is quoted unless it is a plain name.
- **Cause.** When `cause` is set, it prints as `caused by:` blocks, at most 8 levels deep.
- **Frames** (2.5) print innermost first. A run of the same frame, or of the same 2 to 4 frames (a recursion through another language), that comes three or more times in a row prints once, then `  ... the frame above repeated N more times` (`the 3 frames above`). When more than 100 lines are left, the first 50 print, then `  ... N frames omitted`, then the last 20. `e.trace` keeps all of them (5.9).
- **BuildError** prints up to 20 of its `data.diagnostics` after the excerpt, one per line, as `file:line:col: severity: message` (15.7).
- **EngineExitError** does not repeat the child's stderr, because that already streamed live (6.3, 17.3).

```text
main.v2:3:16: ForeignError: ZeroDivisionError: division by zero
    return a / b
           ^
  at ratio (main.v2:3, in @py risky)
  at main.v2:9
```
**`SyntaxError`** is the kind of every lexer, parser and static-check diagnostic, for example `main.v2:3:1: SyntaxError: @py block is never closed`. It is in the error-kind table, but it can never be caught in the file that contains it, because that file never runs.
- **In the entry file**, every diagnostic is printed in source order and the run exits with 1 before anything runs (6.1).
- **In a file loaded by `import`**, the diagnostics are not printed; the `import` raises `ImportError` (6.4).
### 2.5 Frames and messages
| Frame | `e.trace` entry (the renderer prefixes `  at `) |
| ----- | ----------------------------------------------- |
| V2 named / anonymous `func` | `parse_port (main.v2:4)` / `func@main.v2:7 (main.v2:7)` (names: 5.7) |
| top level of a file | `main.v2:10` |
| foreign frame in a block | `ratio (main.v2:3, in @py risky)`; for an unlabeled block, `(main.v2:3, in @py block at main.v2:1)` (12.6) |
| foreign frame in a foreign file | `mean (/usr/lib/python3.12/statistics.py:430, in py)`; `in ts` for a `.ts`, `.mts` or `.cts` file, which runs on the js engine |

- **Frame lines.** The innermost frame's line is where the error was raised; every other frame's line is the call it is executing.
- **Names.** Tags always print in canonical form (`@py`, even when the source says `@python`). Foreign function names print as the engine reports them (`<module>`).
- **Repeats.** A foreign frame without a name that reads exactly like the one before it is dropped: V8 writes an anonymous function's frame in a block (`[1].map(v => v.x.y)`) as it writes the block's own top level.
- **Parse errors** read `expected <what>, found <token>`, with tokens named as: name `x`; keyword `let`; number `42`; string / f-string / bytes literal; an operator or bracket in backticks; end of line; `;`; end of file; block `@py stats`; block body; `@import`; `@export`; a selector tag `@py`.
- **Message style.** Messages start lower-case and have no final period. They put names in `'…'` and code in backticks, as DOCS does. Rust `Debug` output never appears.

---

## 3. Lexer
### 3.1 Input
`SourceMap::load` refuses a file whose canonical path isn't UTF-8 (`v2 x.v2` inside a folder named in another encoding, where main's check of FILE saw only `x.v2`): `v2: the path '/root/dir\u{FFFD}/x.v2' is not valid UTF-8, which V2 needs: a folder on the way has a name in another encoding; rename it`, exit 2. Then it does three things:
- It rejects invalid UTF-8 at the line and column of the first bad byte: `file is not valid UTF-8: it may be in an older encoding, such as Windows-1252 (what Windows PowerShell's Set-Content writes): save it as UTF-8` (on Linux and macOS, `… such as Windows-1252 or Latin-1: save it as UTF-8, as in iconv -f cp1252 -t utf-8 old.v2 > new.v2, then check the text`, as iconv converts from a wrong code page without an error: the code page and its name come from the locale's language, the first of `LC_ALL`, `LC_CTYPE` and `LANG` that is set and not empty, `tr` → cp1254 (`such as Windows-1254`), `ru`, `uk`, `bg` → cp1251, `pl`, `cs`, `sk`, `hu`, `ro`, `hr`, `sl` → cp1250, `el` → cp1253, `he` → cp1255, `ar` → cp1256, `ja` → cp932 (`Shift-JIS`), `zh` → gbk (`GBK`), `ko` → cp949 (`EUC-KR`), any other → cp1252: `span.rs` `older_encoding`, `locale_code_page`); with a NUL byte in its first 8 KiB, `file is not valid UTF-8: it looks like a binary file, not a V2 script`; or, when the file starts with a UTF-16 byte-order mark (Windows PowerShell 5's `>` and `Out-File` write UTF-16), `file is not valid UTF-8: it is UTF-16 (what Windows PowerShell's > and Out-File write); save it as UTF-8`, and with a UTF-32 one (`FF FE 00 00` or `00 00 FE FF`, looked for first, as the first starts with UTF-16's), `file is not valid UTF-8: it is UTF-32 (what Out-File -Encoding utf32 writes); save it as UTF-8`. The same at 1:1 for UTF-16 without the mark, which is valid UTF-8 when its text is Latin: at least three in four of the first 8 KiB's byte pairs are a printable ASCII byte, tab or line break and a NUL, the same way round (`span.rs` `bomless_utf16`).
- It strips one leading U+FEFF, which Windows editors and PowerShell add.
- It turns every CRLF and every lone CR into LF.

The normalized text is what gets stored, spanned, scanned and handed to engines, so block bodies always use LF. (Run files with ext `bat`/`cmd` are written with CRLF, 17.3.) The lexer turns the whole file into one `Vec<Token>` before parsing starts.
### 3.2 Tokens
Every token carries a `Span` (2.2). Besides literals, names, keywords and operators, the lexer emits `Newline` and `Semi` (3.3), `Eof`, and the `@` tokens `DirImport`, `DirExport`, `AtTag`, `BlockHead`, `BlockBody` and `ExternBody` (3.6–3.9).
- **Keywords** (DOCS 18, case-sensitive): `let const func return if elif else while for in not break continue try catch finally throw import as from extern true false null`. A keyword directly after `.` or `?.` lexes as `Ident` (`h.from`, `result.in`), so every rule below sees a member name there.
- **Operators**, longest match first: `** ... .. ?. ?? => == != <= >= && || += -= *= /= %=`, then `+ - * / % = < > ! ? : . , ( ) [ ] ;`.
  - `&`, `|`, `^`, `~`, `<<` and `>>` are errors: "V2 has no bitwise operators" (for `^`, hint `V2 writes a power as 2 ** 3`; for `|`, Python's merge of two dicts, `to merge two dicts, copy and set keys: let c = a.copy(), then for k, v in b { c[k] = v }`, and for `|=`, `to add one dict's keys to another, set them: for k, v in b { a[k] = v }`). Where only a format can be (3.5: `${x:^10}`, `${x:*^10}`), one is the format's error instead.
  - Any other character is `unexpected character '#'`; for `#` the hint says comments start with `//`, and for `#!` at the very start of the file, `V2 doesn't read #! lines: delete it, and run the script with v2 FILE`. Characters that copying code from a web page, a chat or a word processor brings in are named, with a hint: a no-break space (U+00A0, U+2007, U+202F: `unexpected character U+00A0 (a no-break space)`, hint `type a plain space instead`) or another Unicode space (U+1680, U+2000 to U+200A, U+205F: `U+2003 (a Unicode space)`; U+3000, an input method's: `U+3000 (an ideographic space)`; the same hint), a zero-width one (U+200B to U+200D, U+2060, U+FEFF: `(an invisible zero-width character)`, `delete it`), a curly quote (`'“' (a curly quote)`, `use a straight quote: " or '`), a dash (U+2010 to U+2015, U+2212: `'–' (a dash)`, `use a plain minus sign: -`), a multiplication sign (× U+00D7, ⋅ U+22C5, · U+00B7: `'×' (a multiplication sign)`, `multiply with *: 5 * 3`) or a division sign (÷ U+00F7, ∶ U+2236: `'÷' (a division sign)`, `divide with /: 6 / 3`), a backtick (`` '`' ``, `` V2 writes a template string as f"text ${x}" ``; on a line that is only ```` ``` ```` or ```` ```v2 ````, ```` ```v2 and ``` mark an example in a Markdown file: copy only the lines between them ````), Ctrl+Z (`U+001A (Ctrl+Z, an old end-of-file mark)`, `delete it: V2 files end where their text ends`), NUL (`U+0000 (a NUL byte)`, `is this a text file? V2 scripts are plain UTF-8 text`); other control and bidi characters are shown by code point only (`unexpected character U+001B`). A Japanese or Chinese input method left on brings full-width forms (U+FF01 to U+FF5E), named with their ASCII form (`'（' (a full-width '(' from an input method)`; `＇` is `a full-width apostrophe`), hint `type ( with the input method off`, the whole run of them as ASCII up to a `＃`, `｀` or `～` (`１２` → `type 12 with the input method off`; of a longer run 20 characters, then `…`); `＃`, `｀` and `～` get the hints of `#`, `` ` `` and `~`, which V2 doesn't read either (`comments start with //`, the template string's, `V2 has no bitwise operators`), and corner quotes (「」『』: `'「' (a corner quote)`, `use a straight quote: " or '`); its letters are letters, so `ｘ` is a name. A `$` in a V2-level `@import { … }` list (JavaScript's `$x`) gets `hint: write a name that isn't a V2 name in quotes, with a V2 name after as: @import { "$x" as dx } from …` (the alias as written, else the name without its `$`, else `other`), and one right after `as` `after as goes a plain name, without $: as s`. Where only a format can be (3.5), `#` and any other character is the format's error. Directives name characters the same way. Once a directive's `{` has been read, in its list or after it (`@import { f } from　@py`), a space, a zero-width character or a quote gets the same hint, and so does a full-width form whose ASCII fits where it stands (`directives::fits`): `，`, `＂`, `＇`, `＿`, `＄` (whose `$` then gets the quoting hint above) and `｝` in the list, and `＊` in an `@export`'s; `＠` after an `@import`'s `from`, then a selector's `．` and `＿`, and `＂` or `＇` after a module path's dot (`py.＂os.path＂`, not after `from` or `@py.`); `＃` where the comment leaves the directive whole (in a list that goes on below the line, after an `@export`'s list, after an `@import`'s selector: `type # with the input method off`); `｛` right after the word (`type { and } with the input method off` when a `｝` follows on the line, as a brace block's braces are ASCII ones). `；` in a list gets `separate names with a comma: ,`. Any other character or form (`（`, `｀`, `～`, a `．` or a digit in the list), and anything before the `{` (a docstring's `@export – marks it`), gets 4.4's `@@export` or `@@import` hint instead.
- **Identifiers** start with `_` or a char for which `char::is_alphabetic` is true, and continue with those or ASCII digits. They are compared by code point, with no normalization.
- **Comments.** `//` runs to the end of the line. One `//` is Python's floor division instead, which as a comment would quietly drop the divisor (`n // 2` would be `n`): a `//` right after an operand on its line (a name or a number with the calls, indexes and members chained to it, `len(xs)`, `xs[0]`, `d.n`, or a parenthesized expression), with nothing or one space between them, followed by text that starts with a number and holds no words (after one optional space: digits, `_`, `.`, spaces and ``)]},;+-*``, as in `2`, `2)`, `2, 3)`, `2 - 1`), is the error ``// 2` starts a comment in V2, so this line doesn't divide`` at the `//`, hint `V2 divides rounding down with div(a, b), as in div(len(xs), 2)` (the operand as written). An operand that starts its statement (`print(x) // 2`) has no value to divide, so that `//` is a remark; so is a comment after more space (`print(x)    // 3`), with words (`// 3 retries`), a name (`// half`) or other signs (`// 100%`). `/* */` does not nest, and an unterminated one is an error at its start. A block comment that contains a line break counts as one line break.
### 3.3 Where statements end
The lexer keeps a stack of open brackets: `Paren`, `Bracket`, `Dict`, `Body` and `Interp` (an f-string `${`).

**Body and Dict braces.** A `{` is a **Body** brace when the previous token is `)`, `]`, `}`, a name, a literal (number, string, f-string, bytes, `true`, `false`, `null`), or one of `else`, `try`, `catch`, `finally`. Any other `{` is a **Dict** brace: after an operator, `(`, `[`, `,`, `:`, `=`, `=>`, `return`, `throw`, `in`, `Newline`, `;` or `@import`, or at the start of the file or of an interpolation.
- So `if x {`, `} else {`, `func f() {` and `func(s) {` open bodies.
- `let d = {`, `f({`, `@import {` and the first brace of `for k, v in {a: 1} {` open dicts.
- Block and `extern` braces never reach this rule (3.7, 3.9).
- Closers must match their openers (`found ] but ( opened at 3:5 is still open`). A bracket still open at end of file is an error at its opener.

**Line breaks.** A line break becomes a `Newline` token unless one of these holds:
1. The stack top is `Paren`, `Bracket`, `Dict` or `Interp`.
2. The previous token is `,` `=>` `=` `+=` `-=` `*=` `/=` `%=` `+` `-` `*` `/` `%` `**` `==` `!=` `<` `>` `<=` `>=` `&&` `||` `??` `?` `:` `!` `.` `?.` `..` `...`, or the keyword `in`, `not`, `as` or `from`.
3. The next line that is not blank or comment-only starts with `.` (but not `..`) or with `?.`.

Other rules:
- Inside a `Body` the stack top is `Body`, so a `func(x) { … }` inside a call still separates its statements.
- A run of line breaks produces one `Newline`, and none comes before the first token. `;` is a `Semi` token.
- The parser ends a statement at `Newline`, at `Semi`, before `}` and at `Eof`. It skips `Newline`s before `elif`, `else`, `catch` and `finally`.
- A line that starts with a binary-only operator (`+ * / % ** == != < > <= >= && || ?? ?`, `in`, `not in`) does not continue the previous one: `+ b` fails with the hint "end the previous line with the operator". A line starting with `-` or `!` starts a new statement, which 4.5's expression-statement check rejects. After an `if`, `elif`, `while` or `for` head the error is `expected {, found `&&` on the next line`, with the same hint.
- **A body `{` must be on its header's line.** This applies to `if`, `elif`, `else`, `while`, `for`, `func` (both forms), `try`, `catch` and `finally`. After a `Newline`, a `{` is a Dict brace, so the parser reports `expected {` with the hint "put { at the end of the if line". A `{` never closed whose block holds a later line starting with `else`, `elif`, `catch` or `finally`, no further in than the `{`'s line, gets `hint: a } is probably missing before the else on line 9: write } else` (`lexer.rs` `missing_close`). After `js.` or `ts.`, an `@` (`js.@sindresorhus/slugify`) is `a package name that starts with @ goes in quotes: js."@sindresorhus/slugify"`, and `@import *` is `@import needs a list of names, or a module and a name for it`, `hint: V2 has no import *: list the names, @import { sqrt, pi } from py.math, or import the module, @import py.math as math`. A body on the header's own line without braces (`if (x == 1) print(x)`: a name followed by `(`, `=`, `.` or `+=`, other than `and`, `or`, `is`, `not`, `then` and `do`, or `return`, `break`, `continue`, `throw` or `let`) is `expected {, found name `print`` with the hint "the body goes in braces, even for one statement: if … { ... }". A `{` right after `if`, `elif` or `while` is `while needs a condition before its {` (hint `as in while x < 3 { ... }`), and `func hallo {` is `expected `(`, found `{`` with `hint: a function has brackets for its parameters, even none: func hallo() { ... }`.
### 3.4 Numbers
- **Integers.** `0`, `[1-9][0-9_]*`, or `0x` followed by hex digits. `_` is allowed only between two digits. These are errors: leading zeros (`007`), `0b`, `0o`, and a number followed directly by an identifier character (`12abc`, `0x`, `0X1`, `1f`).
- **Floats.** Digits `.` digits with an optional exponent, or digits followed by an exponent (`e|E [+|-] digits`).
  - A `.` joins the number only when a digit follows it. So `0..3` lexes as `Int .. Int`, `1.x` is a member access, and no float starts with `.`.
  - A second `.` with a digit after it (`1.000.000`, thousands as German writes them) is `a number has one decimal point at most`, with `hint: for thousands, write 1000000 or 1_000_000: a point starts the decimals`.
  - Floats are parsed with `str::parse::<f64>` (correctly rounded) after removing `_`. A result of ±inf is an error: "float literal is out of range".
- **Integer range.** `Int` holds the magnitude; anything above 2^63 is an error: "integer literal does not fit in 64 bits". Exactly 2^63 is accepted only as the entire operand of a unary minus (`-9223372036854775808`, which gives `i64::MIN`). Anywhere else, including `-9223372036854775808 ** 1`, it is the same error.
### 3.5 Strings, bytes, f-strings
| Form | Escapes | Line breaks inside |
| ---- | ------- | ------------------ |
| `"…"` `'…'` | yes | no: "unterminated string" at the opening quote, the hint from the line: a curly quote on it (`the line has a curly quote ” where the string should end: …`), three `'` or more in a `'…'` line (`an apostrophe inside '…' ends the string: put the text in "…"`), `\"` at its end (`\" is a quote inside the string, so it goes on: end a Windows path with / or \\`), else `end the string with " on this line; for text over several lines, write \n …, or use a """ string` and how one is laid out |
| `"""…"""` | yes, decoded after dedent | yes, see below |
| `r` + any form | none: `\` is literal and the first matching quote ends the string | as its form |
| `f` + any form / `b` + any form | in literal parts / the byte subset | as its form |

- **Prefixes.** A prefix is a word that is exactly `r`, `f` or `b`, directly followed by a quote. A literal takes at most one: `rf`, `br` and upper-case prefixes are errors.
- **Escapes.** `\n \t \r \\ \" \' \0 \$`, `\xNN` and `\u{…}`.
- **End of file.** Its token sits right after the last token that isn't a line break, so an error there (`let total = 1 +`) shows that line, with the caret where more was expected, not the empty line after it.
  - `\xNN` takes exactly 2 hex digits. It is U+00NN in a string and the byte NN in bytes.
  - `\u{…}` takes 1–6 hex digits naming a Unicode scalar value.
  - Any other escape, including `\` before a line break, is an error at the backslash: `unknown escape \d` (hint: `\\` or a raw string). So are a `\x` without two hex digits and a `\u` without `{`, with the same hint: a Windows path (`"C:\xampp"`, `"data\updates.csv"`) is likelier than an escape.
- **Bytes literals** must be ASCII. They take the same escapes, except `\u{}`.
- **Python's placeholders.** `{name}`, `{a.b}` or `{x:.2f}` (a name, then a format of letters, digits and `.,<>^=+-#_%`) in an f-string's text, its braces not doubled and not `${`, is a SyntaxError: `{x:.2f} in an f-string is Python's placeholder, which V2 doesn't use`, hint `write ${x:.2f} for its value, or ${"{x:.2f}"} for the text {x:.2f}`.

**Triple-quoted strings** (DOCS 3.5), for every prefix:
1. The opening `"""` ends its line: only spaces and tabs may follow it. Otherwise the error is `the text of a """ string starts on the line after the opening """`. The line break after the opener is not part of the content.
2. The closing `"""` must have only whitespace W before it on its line (error: `the closing """ must be on its own line`). Ordinary tokens may follow it, as in `""")`.
3. Each content line either starts with W, which is removed, or is whitespace-only, which becomes empty. Any other line is an error at that line: `indented less than the closing """, or with different tabs/spaces`. In f-strings, lines that begin inside `${…}` are neither checked nor dedented.
4. The line break before the closing line is dropped. So a literal whose four source lines are `"""`, `    a`, `    b` and `    """` is `"a\nb"`; to end with `\n`, leave an empty line before the closing `"""`. Escapes (none for `r"""`) are decoded after dedent, so an escaped `\n` never starts a line.

**f-strings**
- Literal text runs until `${`. A `$` anywhere else is literal, and so is `\$`.
- `${` starts an interpolation. The lexer lexes ordinary tokens recursively, on a fresh stack whose bottom is `Interp`, until the `}` that closes it. So nested strings, even with the same quote, and nested f-strings work: `f"${d["k"]}"`.
- **Format spec.** While no bracket inside the interpolation is open, the exact characters `:` `.` one or two digits `f` `}` end it with `fixed = Some(N)` (DOCS 3.5 `:.Nf`). Any other `:` is an ordinary token, as in a ternary. A `:` left over after the expression (`${x:>5}`), and what the lexer can't read where only a format can be (right after a top-level `:` with no `?` before it in the interpolation, past nothing but digits, spaces and `,.+-_<>^=#`, or one fill character before an alignment: `${x:.3e}`, `${x:^10}`, `${x:*^10}`, `${x:#x}`), is `SyntaxError: the only format in ${…} is :.Nf, with N up to 99`. Its hint goes by what follows the `:` (`lexer.rs` `format_hint`): a width or an alignment, with at most `.N` and `f`, `d` or `s` after it (`>8`, `<5`, `8.3f`, `*^10`), `hint: pad with " " * n, as in let s = str(x), then " " * (5 - len(s)) + s for 5 wide`, and with `.N` and `f` (N of one or two digits) `let s = f"${x:.Nf}"` in place of `let s = str(x)`, with the expression as written when it is on one line of at most 40 characters, else `x` (`${price:8.2f}`: `let s = f"${price:.2f}"`); `.Nf` with spaces around it, `write the format without spaces: ${x:.2f}`; anything else (`.3e`, `,`, `%`, `+.2f`, `.100f`), `hint: format it in a block, as in @py(x) { f"{x:.3e}" }`, with the format as written and the expression passed in: a name as itself (`@py(loss) { f"{loss:.3e}" }`), another expression on one line of at most 40 characters as `x` (`@py(x: d.rate) { f"{x:.2%}" }`), else just `x`. The expression is the text from the last `${` before the `:`, used only when it has no quotes and its brackets balance, else `x` (a nested f-string, `${float(f"${a}") * 2:8.2f}`).
- `${}` is an error. Line breaks inside an interpolation are allowed only in triple-quoted f-strings.
### 3.6 The `@` sigil
- `@import` and `@export` (the word ends there) lex as `DirImport` and `DirExport`.
- `@` TAG right after `from` or `DirImport` is `AtTag(TAG)`, a selector (`@py`, `@py.stats`).
- `@` TAG anywhere else is a `BlockHead`, continued by 3.7.
- `@` not followed by an ASCII letter or `_` is the error `expected an engine tag after @`.

TAG is ASCII `[A-Za-z_][A-Za-z0-9_]*` and case-sensitive. The lexer never checks it: `@perl` lexes whether or not an engine exists. A tag that is unknown, or not a valid engine tag (`[a-z][a-z0-9_]{0,31}`, 17.1; so `@PY` too), raises `EngineUnknown` only when its block is reached (ch. 7). V2 has no decorators: `@memo` followed by `func` fails as a block header, with a hint that says so.
### 3.7 Block heads and raw bodies
The header is `'@' TAG [LABEL] ['(' args ')'] ( '{' | FENCE )`. Outside the args, only spaces and tabs may appear between its parts.
- **LABEL** is an identifier that is not a keyword. A keyword there is the error ``a block label can't be the keyword `for` ``. A string is not a label: `@py "x" {` is the error `a block label can't be a string` (hint: a label is a name, like stats or calc_2), and so is a string where a selector has a label (`from "odd name"`, `@py."odd name"`, 4.1), whose hint for a string ending `.v2` (`from "./util.v2"`) is `another .v2 file is imported with import "./util.v2" as util (a statement of its own, not @import)`; `from "py.statistics"`, a package selector quoted whole, gets `a package is written py.statistics, without quotes`, and `from "js.date-fns"` gets `a package is written js."date-fns": the quotes go around the name only`. Quoted package segments (`js."date-fns"`) are not labels.
- **args** are ordinary tokens under a `Paren` entry, so they may span lines. The `{` or fence must be on the line of the header's last token.
- **Brace on the next line.** If the next non-blank line starts with `{`, the error is `the { that opens a block must be on the same line as @py`, at that `{`, with a note at the header and the hint "move { to the end of line N". Anything else there gives `expected { or a ``` fence after @py stats`.

**Brace form.** The body starts after `{`, and the profile scanner (3.8) finds the matching `}`. When that `}` is on a later line, only whitespace may follow the `{` on the header line: other text there is the error `a multi-line block starts on the line after its {`, at that text, with the hint `move this code to the next line, or write the whole block on one line`, checked before C1 and C2 below: it is the first mistake on the way down. Two guards catch a scanner that closed early on a brace hidden in a construct it does not understand. Both raise a `SyntaxError` whose hint names the fence form ("if that } belongs to the foreign code, use the fence form"):
- **C1.** If the body spans lines, the closing `}` must be the first non-blank character of its line: `@py block closes at a } that does not start its line`. It is reported at the block's `@` (DOCS 4 Errors: "at the opening line"), with a note at the offending `}`, and the hint `put the } that ends the block on a line of its own` when nothing follows that `}` on its line (else the fence form's).
- **C2.** After the closing `}`, the rest of its line must be blank, a `//` or `/*` comment, or text starting with `)`, `]`, `}`, `,` or `;`. Otherwise the error is `unexpected text after the } that closes the @py block`; the hint also shows `(@py { 1 }) + 2` (block values as operands: 4.1).

**Fence form.** The header ends with N ≥ 3 backticks followed by nothing but whitespace (```` ```python ```` is an error). The body is every following line up to the first line whose trimmed text is exactly N backticks; that closing line is consumed. Nothing inside a fence is scanned for its end.

**Unterminated block.** If the end of the file comes first, the error is `@py block 'stats' is never closed` (unlabeled: `@py block is never closed`), at the block's `@`. For the brace form, notes point at the innermost `{` still open inside the body and at any foreign string, comment or template that never closed; the hint names the fence form.

**Output.** Otherwise the lexer emits `BlockHead`, the arg tokens, then `BlockBody`. Its `text` is the exact text between `{` and `}` (or the lines between the fences), directives and indentation included (4.4).
### 3.8 Scanning profiles
A tag's profile depends only on the tag's text, never on config. Built-in ids and aliases take their profile from `syntax/scan`'s static table, which matches the kind / profile column of 7.2 (a test keeps the two in step, 7.1):
- **Python:** `py`, `python`.
- **CLike**, with a dialect: `js`, `javascript`, `ts`, `typescript` (Js); `c` (C); `cpp`, `cxx` (C++); `rust`, `rs` (Rust).
- **Plain:** `bash`, `sh`, `pwsh`, `powershell`, `cmd`, `lua`, `ruby`, `go`, `java`, `zig`, `cs`, `csharp` (the built-in `run` engines, 7.2), and every other tag, meaning every custom engine. Plain counts every `{` and `}`.

The scanner works in state Code, where `{` adds 1 to the depth, `}` subtracts 1, and depth 0 ends the body. It skips these constructs:

| Construct | Python | C | C++ | Rust | Js |
| --------- | ------ | - | --- | ---- | -- |
| line comment | `#` | `//` | `//` | `//` | `//` |
| block comment | none | `/* */` | `/* */` | `/* */`, nesting | `/* */` |
| `"…"` | one line | one line | one line | may span lines | one line |
| `'…'` | string | char | char | char or lifetime | string |
| `'''…'''` `"""…"""` | span lines | none | none | none | none |
| raw strings | (prefixes need no care) | none | `R"d(…)d"` | `r"…"`, `r#"…"#` | none |
| template | none | none | none | none | `` `…${…}…` `` |

- **Escapes.** In every string or char form, `\` skips the next char, even a line break; this is also right for Python raw strings. A one-line string or char that reaches a line break ends there, and the foreign compiler reports it, so a stray quote damages at most one line.
- **C/C++.** A `'` inside a word that starts with a digit (`1'000'000`) is a digit separator. In C++, a word that is exactly `R`, `u8R`, `uR`, `UR` or `LR` followed by `"` starts a raw string. Its delimiter is the text up to `(` (at most 16 chars), and the string ends at `)`, the delimiter, then `"`.
- **Rust raw strings.** A word that is exactly `r`, `br` or `cr`, followed by zero or more `#` and then `"`, starts a raw string. It ends at `"` plus the same number of `#` (`r#ident` has no `"`).
- **Rust chars and lifetimes.** `'\` starts a char that runs to the next unescaped `'`, so `'\u{7D}'` is skipped whole. `'x'` (a `'` two chars on) is a char. Anything else is a lifetime or label, and only the `'` is consumed.
- **Js templates.** A backtick opens a template, and `\` escapes inside it. `${` pushes a Code context with its own depth; its `}` at depth 0 returns to the template. Templates nest.
- **JS regex literals** are recognized where a `/` can't be a division, as in JS itself: at the start, after an operator, an opening bracket, `,`, `;`, `}`, or a keyword such as `return` or `typeof` (not one read as a property, `o.in`); not after a name, a literal, `)`, `]`, postfix `++`/`--` or TypeScript's `x!`, and a block comment in between is skipped; one runs to its unescaped `/` outside a `[…]` class, on the same line.
- **Not recognized:** a regex anywhere else (after `)`, say), preprocessor lines with unbalanced braces, heredocs, unusual string forms. A brace or quote inside one of them can end the block early, or never. C1, C2 and the unterminated error make that failure loud, and the fence form is the fix (DOCS 4.2).
- **Python f-strings** are scanned as plain strings. PEP 701 same-quote nesting still scans correctly whenever its quotes pair up.
- **Line states.** For every body line, the scanner records whether it starts in Code or inside a multi-line string, raw string, template or block comment (`RawBody.starts_in_code`, used by dedent in 4.4). A fenced body is scanned only for these states, never for its end. A Plain body marks every line as Code.
### 3.9 `extern` bodies
After `extern`, the lexer expects a string literal, an optional `as` NAME, and `{` on the same line. It captures the body with the C dialect of CLike, under C1, C2 and the unterminated rule, and emits `ExternBody`. The parser hands the body to `ffi::cdecl` at once, so a declaration V2 cannot read is a `SyntaxError` at the offending token (16.1, 16.3), with the hint `an extern body holds C declarations, as in double cos(double x); (extern itself isn't in this version of V2 yet)` (the parenthesis goes when `extern` runs). `extern` has no fence form.

---

## 4. Grammar and AST
### 4.1 Grammar
EBNF over the tokens of ch. 3. Notation:
- `SEP` is `Newline` or `;`.
- `RAW` is a `BlockBody` or `ExternBody`.
- `BLOCK_HEAD` and `AT_TAG` are the tokens of 3.6–3.7.
- `&x` is lookahead.
- Keywords after `.`/`?.` are already `NAME` tokens (3.2).
```ebnf
program   = stmts EOF                          body = "{" stmts "}"                  (* Body brace *)
stmts     = { SEP } { statement ( SEP { SEP } | &"}" | &EOF ) }
statement = let | const | if | while | for | "break" | "continue" | return | throw | try | func
          | import | engine_import | extern | block | expr [ assign_op expr ]   (* a bare expr: 4.5 *)
let       = "let" NAME [ "=" expr ]            const = "const" NAME "=" expr
assign_op = "=" | "+=" | "-=" | "*=" | "/=" | "%="                     throw = "throw" expr
if        = "if" expr body { "elif" expr body } [ "else" body ]        while = "while" expr body
for       = "for" NAME [ "," NAME ] "in" expr [ ".." expr ] body
func      = "func" NAME "(" params ")" body    params = [ param { "," param } [ "," ] ]
param     = NAME [ "=" expr ] | "..." NAME     return = "return" [ expr ]  (* bare before SEP, "}", EOF *)
try       = "try" body [ "catch" [ "(" NAME ")" ] body ] [ "finally" body ]
import    = "import" STRING "as" NAME          extern = "extern" STRING [ "as" NAME ] RAW
engine_import = "@import" ( "{" items "}" "from" selector | selector "as" NAME )
items     = item { "," item } [ "," ]          item = ( NAME | STRING ) [ "as" NAME ]
selector  = AT_TAG [ "." label ] | label | NAME "." segment { "." segment } | "v2"
label     = NAME                               segment = NAME | STRING
block     = BLOCK_HEAD [ "(" [ block_arg { "," block_arg } [ "," ] ] ")" ] RAW
block_arg = NAME [ ":" expr ]                  expr = block | ternary      (* see "Blocks" below *)
ternary   = coalesce [ "?" expr ":" expr ]     coalesce = or { "??" or }
or        = and { "||" and }                   and = equality { "&&" equality }
equality  = compare [ ( "==" | "!=" ) compare ]
compare   = additive [ ( "<" | ">" | "<=" | ">=" | "in" | "not" "in" ) additive ]
additive  = term { ( "+" | "-" ) term }        term = unary { ( "*" | "/" | "%" ) unary }
unary     = ( "-" | "!" ) unary | power        power = postfix [ "**" unary ]
postfix   = primary { "(" [ arg { "," arg } [ "," ] ] ")" | "[" subscript "]" | ( "." | "?." ) NAME }
arg       = expr | WORD ":" expr | "..." expr  subscript = expr | [ expr ] ":" [ expr ]
primary   = INT | FLOAT | STRING | FSTRING | BYTES | "true" | "false" | "null" | NAME
          | "(" expr ")" | list | dict | func_expr
list      = "[" [ expr { "," expr } [ "," ] ] "]"      func_expr = "func" "(" params ")" ( "=>" expr | body )
dict      = "{" [ entry { "," entry } [ "," ] ] "}"    (* Dict brace *)
entry     = ( WORD | STRING | "[" expr "]" ) ":" expr      (* WORD = NAME | KEYWORD *)
```
- **Blocks.** A statement that starts with `@TAG` is a block statement, ended by `SEP`, `}` or `EOF`.
  - Anywhere else, a block is a whole `expr` (a `let` value, an argument, an element, a parenthesized operand), and it must be an action block (4.5).
  - An operator or postfix after a block value is rejected. On the same line (`@py { 1 } + 2`, `c ? @py { 1 } : 2`), C2 catches it (3.7). On the next line, where `.m()` continues the statement by 3.3 rule 3, the parser reports `a block value must be in parentheses here: (@py { … }).m()`.
  - A block in operand position (`1 + @py { 2 }`, `-@py { 2 }`, `a ?? @py { 1 }`, a `for` head) reaches `primary`, which has no block: `a block value must be in parentheses here`, at the `@`.
- **`@import`.** `engine_import` is the V2-level statement, with both forms. An `@import` directive inside a block takes only the first form, `{ items } from selector` (4.4).
- **`func`.** A statement that starts with `func` and a name declares a function; `func` followed by `(` starts a `func_expr`, an anonymous function. `lambda` is an ordinary name: `lambda` followed by a parenthesized list and then `=>` or a body `{` is the error `V2 has no lambda keyword` (hint: `anonymous functions are written func(x) => …`).
- **Assignment** is a statement, not an expression. Targets are NAME, `postfix "." NAME` and `postfix "[" expr "]"`, in parentheses or not. Any other target is `cannot assign to this expression`, and a target chain that contains `?.` is `cannot assign through ?.`.
- **Optional chains** (`OptChain`): 5.5.
- **Operands and arguments.**
  - `-2 ** 2` is `-(2 ** 2)`, and `2 ** -1` is valid.
  - Named args come after all positional and spread args. A named arg is `NAME` or a keyword then `:` (two-token lookahead), so `f(c ? a : b)` is positional and `df.agg(func: "sum")` names Python's `func`; a keyword then `:` is a dict key the same way (`{from: a}`), which nothing else can start.
  - The block arg `nums` is short for `nums: nums`.
  - Past 256 levels of nested expressions, bodies and interpolations, the parser stops with `nested too deeply`. A body is a level and so is each expression inside another, a statement's own expression included: 255 parentheses fit, and 256 nested `if`s. Left-associative chains (`a + b + …`, `a && b && …`, `a.b.c…`, `f()()…`) build a tree as deep as the chain is long, so the tree's binary-operator and postfix nodes are counted separately: when a node would sit more than 10,000 such nodes deep, counting the operands already under it (a parenthesized group, a tighter chain) and the expression it is part of (an enclosing chain, an expression in a `func_expr`'s body), the parser stops with `expression too long` (hint: split it into several statements with let). Both limits protect the host stack from generated input; every later pass (4.5, ch. 5) may recurse over the tree.

### 4.2 Precedence
DOCS 3.4's table; 4.1's grammar encodes it.
### 4.3 AST
`syntax/ast` follows 4.1's grammar, and every node carries its `Span` (2.2). Other chapters rely on these nodes:
- **`EngineBlock`.** Its `UnitId` (the file and the byte offset of the block's `@`), tag and label as written, args, the processed `body` with its `UnitLoc` (4.4, 2.3), and its directives: `exports` (`Exports { items, star }`; present means a definition block) and the inner `@import`s.
- **`Selector`** keeps tags and path segments exactly as written: `@py`, `@py.stats`, `stats`, `py.os.path` or `js."@scope/pkg"`, `v2`. Ch. 13 canonicalizes them (`@python.stats` is `@py.stats`) and joins module paths.
- **`Extern`.** The library string, the optional alias, and `decls`: the `Sig`s that `ffi::cdecl` parses at parse time (3.9, 16.4). A compiled block's function signatures are read only when the block is reached (15.2, 15.3).
### 4.4 Block bodies: lines, directives, dedent
`syntax/directives.rs` turns a `RawBody` into `body`, `loc` (2.3) and `directives`. The result is what engines receive: the code as written, with directive lines blanked and the common indentation removed (DOCS 4.1).

**Step 1: lines.**
- *One-line brace block:* the body is the text, trimmed. `body_first_line` is the header line and `body_col` is the column of the body's first char. It takes no part in dedent.
- *Multi-line brace block:* the body is the lines between the header line, which has only whitespace after its `{` (3.7), and the closing line, whose text before `}` is blank (C1) and is dropped. `body_first_line` is the header line + 1.
- *Fence form:* the body is the lines between the fences, and `body_first_line` is the fence line + 1.

**Step 2: directives.** A body line is a directive when, after leading spaces and tabs, it starts with `@export` or `@import` followed by a space, a tab, `{` or the end of the line. The match is on the exact token: `@exporter` and `@import_hook` are foreign code. Matching is line-based on purpose, whatever the foreign string or comment state, so an `@export` line inside a foreign string is still a directive. A line starting with `@@export` or `@@import` is the escape: it stays foreign code, minus one `@` (DOCS 6.1).
- **Directive-mode lexing.** The V2 lexer lexes from that `@` in directive mode. Spans are real file spans, and a line break ends the directive except inside `{ }` (3.3 rules 2 and 3 are off).
- **Parsing.** The parser reads one `@export` list or one `engine_import`. A list spans lines until its braces balance; if the body ends first, the error is `unterminated @export list`. The directive's last line may then hold only whitespace or a comment starting with `#` or `//`.
- **Blanking.** Every line the directive touched becomes an empty line, so foreign line numbers never shift.
- **Escapes.** A directive line that doesn't parse, with no hint of its own, may be the block's own code (a CSS `@import url("theme.css");` in a template literal or a docstring): its error gets `hint: a line that starts with @import is V2's; for code of the block's own language that starts with @import (a CSS @import in a string), write @@import` (for `@export`, the hint below). JavaScript's `@export { show as pyshow }` is `expected `,` or `}` in the @export list, found keyword `as``, hint `@export can't rename: export show and rename it where it is imported, as in @import { show as other } from …`, the item as each list writes it: a quoted one keeps its quotes (`export "$x" and …, as in @import { "$x" as other } from …`), and a V2 keyword, bare in `@export`, is quoted in `@import` (`@import { "from" as other }`); `@export { show as $s }` gets the same hint at the `$`. Another `$` in an `@export { }` list gets `hint: write a name that isn't a V2 name in quotes: @export { "$x" }`, and one in an `@import { }` list `write a name that isn't a V2 name in quotes, with a plain name after as: @import { "$x" as x } from …`, always with an alias, since Python binds an imported name only as a Python name (12.2): the alias as written (`{ "$x" as dx }`), else the name without its `$` when that is a V2 name that every adapter takes (no Python keyword or dunder, none of the words js.js refuses: `this`, `delete`, `yield`, 12.2), else `other` (`{ "$class" as other }`, `{ "$this" as other }`); right after `as`, `after as goes a plain name, without $: as s` (`as other` by the same rule).
- **`@export` grammar:** `"@export" "{" ( "*" | xitem { "," xitem } [ "," ] ) "}"`, with `xitem = NAME | KEYWORD | STRING`. Without the `{` the error is `expected { after @export`, hint `a line that starts with @export is V2's; for code of the block's own language that starts with @export, write @@export` (a Python decorator named `export`).
  - Keywords are accepted bare, because they name foreign functions.
  - Several `@export` lines merge into one list, which may combine `*` with explicit names (12.3).
  - In `c`, `cpp` and `rust` blocks (and their aliases), items name functions only; a quoted item is a `SyntaxError` (4.5).
  - Elsewhere, a quoted item is a name that is not a V2 identifier (JS `"$helper"`).
- **`@import` inside a block** takes only the `{ items } from selector` form of `engine_import`; the whole-module form is a `SyntaxError` (4.5). It may use the `v2` selector. Its quoted items need no `as`, because the name binds in the foreign namespace. Directive mistakes in compiled blocks are static (4.5); which directives a `run`-engine block accepts is checked when the block is reached (12.1 step 3).

**Python indentation.** In a Python-profile block, a code line whose leading whitespace holds anything but spaces, tabs and form feeds, which Python takes (a no-break space pasted from a web page, another Unicode space) is the error `unexpected character U+00A0 (a no-break space) in the block's indentation` at that character, named and hinted as 3.2 does: Python rejects it, and may report another line. Other profiles leave it to their language (JavaScript takes a no-break space as whitespace).

**Step 3: dedent.** A line is a *code line* unless the scanner recorded that it starts inside a multi-line string, template or comment (3.8).
1. Whitespace-only code lines become empty and are ignored, and so are lines holding only a comment (`#` for py; `//` or `/*` for js, ts and the compiled languages), whose indentation the language doesn't care about. m is the smallest number of leading spaces and tabs over the remaining code lines.
2. Their first m chars must be identical. If they are not, the error is `block indentation mixes tabs and spaces`, at the first line that differs, with a note at the first code line (DOCS 4.1) and, since the shown lines leave out their indentation, `hint: this line starts with 1 tab, the block's first code line with 4 spaces: indent every line the same way`. Chars after the first m are the foreign language's business.
3. m chars are removed from every code line, and from every other line that starts with the same m chars. The remaining lines (string or comment text indented less than the code) stay exactly as written and are listed in `UnitLoc.kept`. So a Python triple-quoted string whose text starts at column 1 keeps its content, and the code around it is still dedented.

`dedent_width` is m. These are ASCII chars, so byte slicing is char-safe. A dedented line 1 has `body_col` = m + 1. The body is the lines joined with `\n`, plus a final `\n`; it is `""` when no line has text.
```text
line 10  @py stats {                      header line: not part of the body
line 11      def spread(xs):              body line 1
line 12          return max(xs) - min(xs)
line 13      @export { spread }           becomes "", exports = [spread]
line 14  }
=> body "def spread(xs):\n    return max(xs) - min(xs)\n\n", loc { body_first_line: 11, body_col: 5, dedent_width: 4, line_count: 3, kept: [] }
```
### 4.5 Static checks
- **Lexing and parsing** stop at the first error; there is no error recovery.
- **Static checks.** A file that parses gets every whole-tree check (`syntax/check`: labels, `v2`, placement, redeclarations, unused values), all reported in source order (2.4). `v2 check` reports the same (19.3).
- **Kind.** Every error below is a `SyntaxError` with a span.
- **Known tags.** This pass knows only the built-in tags and aliases, by their text (3.8); `c`, `cpp`, `cxx`, `rust` and `rs` are always compiled (7.2, 17.2).

| Check | Message |
| ----- | ------- |
| the same label on two blocks in one file, in any languages (DOCS 4.7) | `duplicate block label 'stats'`, with a note at the first one |
| label `v2`, or selector `v2.x` | `'v2' is reserved for this file's V2 functions` |
| `@export` outside a block; `from v2` outside a block | `@export is only valid inside an embedded block` (hint: modules export every top-level name not starting with `_`) / `the v2 selector is only valid inside a block` |
| a definition block used as a value (DOCS 4.5) | `a block with @export is a definition block and cannot be used as a value`, with a note at the `@export` |
| args on a `c`, `cpp`, `cxx`, `rust` or `rs` block (DOCS 4.4) | `compiled blocks take no args — @export a function and call it with arguments` |
| a block arg given twice; a block arg named like one of the block's inner `@import` aliases | `block arg 'a' is given twice` / `'m' is already declared in this block` |
| a block arg that isn't a name, or a name followed by more than `,`, `)` or `: expr` (`@py(1 + 2)`, `@py(data.copy())`) | `expected a block arg name, found number 1` / ``expected `,` or `)`, found `.` ``, hint ``block args are `name` or `name: expr`, as in @py(n: len(xs))`` (the tag as written) |
| `@export {}`, a name exported twice, text after the list, an unterminated list (4.4) | the specific reason |
| a quoted `@export` item in a `c`, `cpp`, `cxx`, `rust` or `rs` block (4.4) | `compiled blocks export functions by name` (hint: export a wrapper function with a plain signature) |
| an `@import` in a compiled block without `@export` (program mode, 15.5) | `a compiled block without @export runs as a program and can't @import` (hint: `export a function from this block, or call the other language from V2 code`) |
| `@import { * } from …`, at V2 level or in any block | `@import needs a list of names` (hint: `list the names: @import { mean } from …`) |
| in a compiled block, an `@import`'s local name (the alias, else the name) or an exported name that is not a valid, non-keyword identifier of the block's language (C, C++ or Rust, by tag) | import: `'x' is not a valid C identifier — rename it with 'as'`; export: `'x' is not a valid C identifier` (hint: `rename the function, or export a wrapper with a plain name`) |
| a V2-level `@import sel as x` where sel is not a package selector `lang.module` | `only a package selector (py.name, js.name) can be imported as a whole module` |
| `@import sel as x` inside a block (4.4) | `the whole-module form of @import is only valid in V2 code`, with a hint naming the block language's own import when the selector is a `py.` module in a py block (`use import numpy as np inside @py blocks`, `use import os` for `py.os as os`) or a `js.`/`ts.` module in a js/ts block (`use const dfns = require("date-fns") inside @js blocks`); otherwise the hint is `import names instead: @import { … } from …` |
| a quoted V2-level import item that is not a plain name (a V2 identifier that is not a keyword, 13.4) and has no `as` | `quoted name 'from' needs an alias: "from" as NAME` |
| `a..b` outside a `for` head; `for i, x in a..b`; `for (x in xs)` | `a..b is only allowed in a for head; use range(a, b)` / `a range loop takes one variable` / `a for head takes no parentheses` |
| `break` or `continue` outside a loop (a function body starts outside any loop); `return` outside a function | `break outside a loop`, and so on |
| a name declared twice directly in one body: `let`, `const`, `func`, named `@import` items, `import` and `extern` aliases, `extern` functions, and the parameters, loop variables or catch name that the body introduces (DOCS 3.3; inner bodies may shadow) | `'x' is already declared in this block`, with a note at the first declaration |
| a duplicate parameter, a non-default parameter after a default one, `...rest` not last; a positional argument after a named one, a duplicate named argument | the specific reason |
| `try` with neither `catch` nor `finally` | `try needs a catch or a finally` |
| `a < b < c`; `a == b == c` (`!=` too); `not` without `in` | `comparisons cannot be chained; write a < b && b < c` / `comparisons cannot be chained; write a == b && b == c` / `not is only used in 'not in'; negate with !x` |
| an expression statement that is not a call (a postfix chain whose last step is a call, `?.` chains included) or an action block, either one optionally in parentheses (DOCS 3.1) | `this expression's value is not used`, with the hint `to continue the previous line, end that line with the operator` for a unary `-` or `!` expression, `did you mean '='?` for a top-level `==`, `to give it back from the function, write return in front of it` for a function body's last statement, `to call it, write hallo(); to show it, print(hallo)` for a bare name, `to call it, add the brackets: sort()` for a member, `to change i, write i += … (or i = i + …)` for `NAME + - * / …`, else `keep it with let x = …, print it, or remove the line` |
| a direct statement of a `while` or `for` body `let x = e` where `e` reads `x` (not inside a function it makes) and an enclosing body declared `x` before the loop | `this let makes a new 'i' for one turn of the loop, so the 'i' outside it never changes`, with `note: declared here` and `hint: to change that one, leave out let: i = …` (`while i < 5 { let i = i + 1 }` never ends) |
| a `{` that starts a statement, then a keyword (JavaScript's bare block) | `V2 has no bare { } blocks: a { that starts a statement is a dict`, hint `write the statements without the braces, or in a function` |
| a dict key that is a number / a keyword / `...` or `**` (JavaScript's `{...a, ...b}`, Python's `{**a, **b}`) | `dict keys are names, strings or [expr]`, hint `dict keys are strings: write "1": or [str(1)]:` / ``keyword `in` can't be a bare dict key: quote it`` (followed by `:`, hint `write "in": as the key`; otherwise, after `=> {`, a hint to write `func(…) { … }`) / `dict keys are names, strings or [expr]`, hint `copy and set keys: let c = a.copy(), then for k, v in b { c[k] = v }` |
| an invalid assignment target; a target through `?.` (4.1) | `cannot assign to this expression` / `cannot assign through ?.` |
| an `extern` declaration V2 cannot read (3.9) | the messages of 16.3 |

Checked at run time instead: unknown tags (`EngineUnknown`, 7.3) and the directives that `run`-engine blocks accept (12.1 step 3).

---

## 5. Host runtime

`runtime/` is a tree-walking interpreter over the span-annotated AST (ch. 4). It runs only on the interpreter thread (256 MiB stack, 1.3); `Value` is `Rc`/`RefCell` and never crosses threads.

### 5.1 Values

```rust
pub enum Value {
    Null, Bool(bool), Int(i64), Float(f64), Str(Str), Bytes(Rc<[u8]>),  // Str: Rc<str>, or Rc<String> once grown (below)
    List(List), Dict(Dict), Func(Func),        // Rc<RefCell<Vec<Value>>>, Rc<RefCell<DictMap>>,
    Handle(Rc<HandleRef>), Error(Rc<ErrorValue>),  // Rc<Callable>, each freed iteratively (below)
}
pub enum Callable {
    Closure { decl: Rc<FuncDecl>, env: Env, name: Rc<str> },         // name: `greet` or `func@main.v2:12` (5.7)
    Builtin(&'static BuiltinDef),                                    // 5.10
    Proxy { session: SessionId, unit: Rc<str>, name: Rc<str> },      // bridged export; unit = wire unit id (13.4)
    Waiting { import: Rc<EngineImport>, local: Rc<str>, seen: Rc<[UnitId]>, resolved: OnceCell<Value> },  // a name an `@import` bound without running (12.7)
    Route { route: usize, name: Rc<str> },                           // a route stub passed back unresolved (11.8)
    Native { lib: LibId, sym: Rc<str>, sig: Rc<Sig> },               // compiled export (15.8) or extern (16.1)
    HandleMethod { handle: Rc<HandleRef>, method: Rc<str> },         // built only as the callee of h.m(...) (11.3)
}
pub struct ErrorValue {
    pub kind: Rc<str>, pub message: Rc<str>, pub data: Value,     // kind: the `type` field
    pub lang: Option<Rc<str>>, pub foreign_type: Option<Rc<str>>,
    pub cause: Value,                                              // an error, or null
    pub span: OnceCell<Span>, pub trace: OnceCell<Rc<[Rc<str>]>>,  // set by the first raise (5.9)
}
```

- `Value::clone` is O(1): assigning, passing, returning or storing a list or dict shares it; `str`, `bytes`, `func`, `handle` and `error` are immutable. `.copy()` is shallow: a new `Vec`/`DictMap`, nested containers shared.
- **`Str`** is an `Rc<str>` (one allocation; literals, dict keys and error texts are shared, not copied) until `+=` grows it (5.3): `Str::append` then copies it into an `Rc<String>` with exact capacity, and the next appends push onto that `String` in place while `Rc::get_mut` allows it (no other `Rc`, and no `Weak` from the char cache, 5.4), else copy again. So a string only one name holds grows in amortized O(1), and every other holder keeps the old text. `==`, ordering and printing see only the text; `Str::addr` is the allocation's address either way.
- **`DictMap`** keeps a dict's entries in insertion order: `slots: Vec<Option<(Rc<str>, Value)>>` (`None` is a removed entry) and, past 8 slots, `index: HashMap<Rc<str>, usize>` (up to 8, a lookup walks the slots). `remove` leaves a hole instead of shifting the later entries; holes at the end go at once, and the rest are compacted (with the index rebuilt) once the slots exceed twice the entries plus 8, so removing is O(1) amortized. Setting a key that is there keeps its place; a new key, or a removed one put back, goes at the end. Iteration skips holes, so printing, `keys()`, JSON, the wire codec and the collector see the entries in order.
- **Cycles through frames.** A closure over a frame that the frame holds (in a slot, or in a list or dict in one) makes an `Rc` cycle, and so does one over a frame below it (a closure made in an `if` or a loop body, kept in the function's variables). Leaving a frame at or below which closures were made walks what its slots reach: each frame, list, dict, function and error is counted with its references from the walked values, and one with more holders than that is held from outside, so it lives, with all it reaches (followed along each value's own references, so linear). The frame is garbage if none of that reaches it, and its slots are cleared, with those of every other frame found garbage. A value is walked only once all its holders are found among walked ones, so what the rest of the program holds (data passed in, a running frame, another call's closure) costs nothing. Nor is a frame made before the one checked, or one checked before whose walk looked at more than 64 items unless it is below the one checked (its own nested call): it was checked when it was left, and a chain of closures, each over the frame before (built by a loop or by recursion), would take every walk back to the first. A reference the walk misses only makes more look held, so a wrong answer keeps memory but never frees what is in use. A frame still held when it is left is remembered, and what it holds counts toward the next collection as if made then: list items and dict entries, and strings and bytes by their size (16 bytes an item) and a handle as 1,000 (its object, elsewhere, could be big), each once until the next collection however many frames reach it (by address; one shared when the frame is left may be the garbage's alone later); a collection counts each once in what it found alive too. Dropping a closure that leaves at most one other holder rechecks each remembered frame from the closure's own up, at the next scope exit, once the frames left since that frame's last check, times 8, reach the items that check looked at: a big group costs each call a few items, not itself. A collection walks all remembered frames together and everything they reach, with no budget: it frees cycles through several frames, cycles made after a frame was left, cycles a dropped list or dict let go of (no closure drop announces those), and groups too big for a scope exit's walk (10,000,000 items or 1,000,000 values). It runs at a scope exit once the program has made, since the last one, as many list items, dict entries and frame slots as the last one found alive, and at least 100,000: its work stays in proportion to the program's, and garbage to what lives. Other cycles, such as lists and dicts that point at each other, or one through another language (a frame holding a handle whose object keeps a closure over that frame, DOCS 8.2), are not collected.
- **Freeing is iterative.** The `List`, `Dict` and `Func` handles wrap their `Rc` in a type whose `Drop` moves the contents of the last reference (a closure's frame slots, when nothing else holds the frame) into a thread-local queue that the outermost drop empties one value at a time; `ErrorValue`'s `Drop` queues its `data` and `cause` the same way. So a value nested a million levels deep (`for i in 0..1000000 { v = [v] }`) never overflows the stack when it goes away.
- `type(x)` is one of `null bool int float str bytes list dict func handle error`. Every `Callable` is `"func"`, including names bound by `@import`; foreign functions, bound methods and classes *returned as values* are callable handles, type `"handle"` (11.6). Native pointers are handles in the native pseudo-session (16.6).
- Blocks and `@import`s in a function body resolve against the declaring file (`decl.span.file` and its `FileScope`, 6.2), whichever file calls it.

### 5.2 Equality, ordering, truthiness

`eq(a, b)` implements `==`/`!=`; `in` on lists and `index_of` reuse it.

| Operands | Rule |
| --- | --- |
| same scalar type | value equality; floats follow IEEE (`nan != nan`, `0.0 == -0.0`) |
| `Int` vs `Float` | exact: `f.fract() == 0.0 && -2^63 <= f < 2^63 && f as i64 == i` (never `i as f64 == f`, wrong beyond 2^53) |
| `Str`, `Bytes` | byte equality |
| `List`, `Dict` | `Rc::ptr_eq`, else pairwise `eq` (list) or the same key set with `eq` values (dict, order ignored) |
| `Handle` | same session and id (11.6); native pointers: same address (16.6) |
| `Func` | `Closure`/`Builtin`: `Rc::ptr_eq`; `Proxy`: same session, unit, name; `Route`: same route; `Native`: same library and symbol; `HandleMethod`: equal handles, same method |
| `Error` | `Rc::ptr_eq` |
| different types (`Int` vs `Bool` too) | `false`, never an error |

- **Depth.** `eq` and `str()` recurse; past 1,000 levels they raise `RecursionError: value nested too deeply (or cyclic)`, and `str()` prints a container already on its current path as `[...]`/`{...}`. The codec has its own limits: `json_parse` and `json_stringify` reject nesting over 1,000 levels with `ValueError` (10.1), and the 256-level container limit applies only to VEP frames and compiled reverse-call JSON (10.5).
- **Ordering** (`< > <= >=`, `sort`, `min`, `max`) exists only for number–number and str–str. Int vs float is exact: NaN compares `false`; `f >= 2^63` or `f < -2^63` decides at once; otherwise compare `i` with `f.trunc() as i64`, a tie decided by the sign of `f - f.trunc()`. Strings compare by code point (Rust `str` order). Other pairs → `TypeError: cannot compare list and int with '<'` (a str and a number add `— to compare text such as input() gives with a number, convert it: int(x) or float(x)`); a handle operand gets the TypeError of 11.7.
- **Truthiness.** Falsy: `null`, `false`, `0`, `0.0`, `-0.0` and an empty str, bytes, list or dict. Everything else is truthy, including NaN, functions, errors and every handle (no round trip). `!x` and `bool(x)` use this rule.

### 5.3 Operators

| Op | int ∘ int | with a float | other operands |
| --- | --- | --- | --- |
| `+` | `checked_add` | f64 | str+str, bytes+bytes, list+list → new value; else `TypeError: unsupported operand types for +: 'str' and 'int'`, plus `— to join text, use f"...${x}" or str(x); to count with text such as input() gives, convert it: int(x) or float(x)` when one side is a str (for `-`, `*` and the rest with a str and a number, `— to count with text such as input() gives, convert it: int(x) or float(x)`; a str or list `*` a float, `— repeat with a whole number, as in "ab" * 2`; a list `+` something that isn't one, `— add an item with xs.push(x), or join lists with xs + [x]`; two dicts, `— to merge two dicts, copy and set keys: let c = a.copy(), then for k, v in b { c[k] = v }`). With a `null` operand none of those: the interpreter adds a hint saying where the null came from (`null_hint`): a call `f(…)` → `f(…) gave null: a function without return gives null, so end it with return …` (a builtin: `print() gives null: it does its work and has no value to use`; `getenv("API_KEY") gave null: that environment variable isn't set: pass a default, as in getenv("API_KEY", "…")`, the variable as the call names it when that is a literal (else `getenv(…) gave null: … as in getenv(name, "…")`), and on Windows for `HOME`, which only Git Bash sets, `getenv("HOME") gave null: Windows sets USERPROFILE for the home folder, not HOME (Git Bash sets both): write getenv("USERPROFILE")` (elsewhere `getenv("USERPROFILE")`: `USERPROFILE is Windows' name for the home folder: write getenv("HOME")`; `getenv_advice`); `.get(…)`: `get() gave null: the key or index isn't there: pass a default, as in d.get(k, 0)`; a block: `the block gave null: a block's value is its last line, and that line isn't an expression`, and a name set from any of them says so), `xs.sort()` (`reverse`, `push`, `insert`) → `sort() changes the list itself and gives null: use the list`, `input()` → its end-of-input hint, a name → what the text after its `let NAME =` on an earlier line shows (`'name' is null: it was set from input(), …`, `… from xs.sort(), and sort() changes the list itself …`, `… from doppelt(…), which gave null (a function without return does)`), else `'name' is null here: a function without return gives null, and so does input() when the input has ended`; the same for `cannot index null` |
| `-` `*` | `checked_sub` / `checked_mul` | f64 | `*`: str or list × int, either order; count ≤ 0 → empty; size via `checked_mul`, buffer via `try_reserve_exact` (failure → `ValueError: repetition result too large`); a result over 1 GiB of text or 100,000,000 items is refused first, `ValueError: repetition result too large: 3000000000 bytes of text (1 GiB at most)` (`… items (100000000 at most)`), as more would take the machine's memory |
| `/` | both to f64; always a float | f64 | divisor `0` or `0.0` → `ValueError: division by zero` |
| `%` | floor: `r = a.wrapping_rem(b); if r != 0 && (r < 0) != (b < 0) { r += b }` | same fix-up on f64 `%`; a zero result takes the divisor's sign (`-6.0 % 3` is `0.0`, `6.0 % -3` is `-0.0`) | divisor `0` or `0.0` → `ValueError: division by zero` |
| `**` | `e >= 0`: `checked_pow` (`e > u32::MAX` overflows unless the base is 0, 1 or -1); `e < 0`: a float | `powf` | zero base (int or float) with a negative exponent → `ValueError: division by zero` |
| `-x` | `checked_neg` | negate | `TypeError: bad operand type for unary -: 'str'` |

- Integer overflow raises `ValueError: integer overflow in '*'` (the operator). Float `+ - * **` follow IEEE and may give `inf`/`nan`.
- `x op= y` always means `x = x op y`, with the target's container and key evaluated once: `xs += [1]` makes a new list and rebinds `xs` (other names keep the old list). `s += piece` with two strs, on a variable, a dict entry (`d.k`, `d[k]`) or a list item, reads `s`, evaluates `piece`, then, if the target still holds the very string it read, drops that read and appends in place (`env::grow`, 5.1's `Str::append`); if the target changed meanwhile (or is a constant, or a handle's item) it is the plain `x = x + piece` with its usual errors. `&&`/`||` short-circuit and return an operand; `??` returns its left operand unless it is `null`, and evaluates its right operand only when the left one is `null`.
- `x in y`: list → an element `eq` to `x`; dict → key lookup (a non-`Str` `x` is `false`); str → substring, `x` must be a `Str`; bytes → a `Bytes` subsequence or an `Int` byte (outside 0–255 → `false`); otherwise TypeError (`'in' needs a list, dict, str or bytes on the right, got int`; `'in' on a str needs a str, got int`). `not in` negates.
- Handles take only `==`/`!=`: `in`, slicing, arithmetic, unary `-` and ordering raise the TypeErrors of 11.7, whose hint says to do it inside a block of its language or call a method, and that `to_v2(h)` converts plain data.

### 5.4 Strings and bytes

A `Str` is always valid UTF-8 (lone surrogates never enter, 10.4); user-visible positions are **code-point** indexes. `Interp` keeps an 8-slot round-robin cache of `CharIndex { s: WeakStr, ascii: bool, chars: usize, marks: Vec<usize> }` (`marks`: byte offsets of chars 0, 64, 128, …) so `s[i]`, `len(s)`, slices and `index_of` are not O(n). A lookup compares the `Weak`'s address with `Str::addr` (the `Weak` keeps the allocation, so its address cannot be reused, and it stops `+=` from growing that string in place under the entry, 5.1). Char → byte is `i` when `ascii`, else `marks[i / 64]` plus `i % 64` steps; byte → char binary-searches `marks`. Strings under 64 bytes and `for ch in s` use no index. An entry for a string no one holds any more is dropped at the next lookup: its `Weak` would keep the whole allocation.

- `s[i]` is a one-char `Str`; negative `i` adds the length; out of range → `IndexError: string index 9 out of range (length 5)` (lists and bytes use the same wording).
- `x[a:b]` on str, bytes and list follows Python: a missing bound is 0 or the length, a negative bound adds the length, both clamp to `[0, len]`, `a >= b` gives an empty value, the result is new (shallow for lists). No step, no slice assignment; slicing anything else is a TypeError.
- f-strings: `${e}` inserts `str(e)`; `${e:.Nf}` needs an int or float (else TypeError); an int prints exactly, its digits then N zeros (`${9223372036854775807:.2f}`, not through `f64`), a float is `format!("{:.N$}", x)`, rounding the exact binary value (`${19.999:.2f}` → `20.00`); NaN prints `nan`, not Rust's `NaN`.
- `Bytes` supports `len(b)`, `b[i]` (an `Int` 0–255), slices, `+`, `==` and `in`, but no ordering. `b.decode()` is strict UTF-8 (else `ValueError: invalid UTF-8 at byte offset N`), and so is `s.encode()`: that ValueError, and the ArityError of either given any argument (`b.decode("latin-1")`, `encoding:`, `errors:`), get `hint: V2's decode() reads UTF-8 only: for another encoding, decode in a block, as in @py(b) { b.decode("cp1254") }` (`V2's encode() writes UTF-8 only: for another encoding, encode in a block, as in @py(s) { s.encode("cp1254") }`; `methods::utf8_only`), except a call whose one argument (by position or `encoding:`) is `"utf-8"` or `"utf8"` in any case, which gets `hint: V2's decode() reads UTF-8: write b.decode()` (`V2's encode() writes UTF-8: write s.encode()`; `methods::names_utf8`); `b.hex()` is lowercase without separators.

### 5.5 Members, indexing and optional chains

| Receiver | `r.name` | `r.name(args)` | `r[k]` | `r.name = v` / `r[k] = v` |
| --- | --- | --- | --- | --- |
| dict | key; missing → `KeyError: key 'name' not found` + did-you-mean over the keys | key `name` holding a `Func` or a handle → call it; otherwise the dict method; neither → `TypeError: dict has no key or method 'g'`, with a did-you-mean over the keys and the methods (an imported `.v2` file is a dict of its names) | `Str` key (else `TypeError: dict keys must be str, got int`); missing → KeyError, with `hint: when a key may be missing, read it with d.get(k, default), as in counts[w] = counts.get(w, 0) + 1, or test it first with k in d` when there is no did-you-mean | insert or replace (a replaced key keeps its position) |
| list | `TypeError: list has no field 'push'` | list method | `Int` (else `TypeError: list index must be an int, got float`, for a float with `hint: V2's / always gives a float: write div(a, b) for a whole number, as in xs[div(len(xs), 2)]`), negative from the end; IndexError, `hint: xs.get(i, default) gives a default past the end; or check len(xs) first` (on `args()[i]`, `hint: the script was given fewer arguments: use args().get(0, "default") for one that may be missing, or check len(args()) first`) | index assignment; IndexError, never grows; `r.name = v` is the field TypeError |
| str, bytes | `TypeError: str has no field 'x'` (a method's name: `…: upper is a method, so call it, as in x.upper()`, for every type with methods) | method (5.11) | code point / byte (`string index …`, `bytes index …`) | `TypeError: str is immutable`, `hint: build a new string instead, as in s = s[0:i] + "a" + s[i + 1:]` |
| handle | `op getattr` | `call {h, method}` | `op getitem` | `op setattr` / `op setitem` (11.3; native pointers: TypeError, 16.6) |
| error | field (5.9) | `TypeError: error has no method 'x'` | `TypeError: cannot index error` | `TypeError: error values are read-only` |
| null | `TypeError: cannot read 'name' of null (use ?.)` | same (on `input()`'s result, `hint: input() gives null when the input has ended: check it first, as in let line = input(); if line == null { break }`) | `TypeError: cannot index null` | `TypeError: cannot set 'name' of null` / `TypeError: cannot index null` |
| other | `TypeError: int has no field 'x'` | `TypeError: int has no method 'x'` | `TypeError: cannot index int` | `TypeError: int has no field 'x'` / `TypeError: cannot index int` |

- A dict dispatches `d.name(args)` on its key first **only when that key holds a `Func` or a callable handle** (a foreign function got as a value; `HandleRef.callable`, 11.1): module namespaces (6.4), `extern … as ns` dicts (16.1) and dicts of functions work like objects (`ns.keys(…)` calls a C function named `keys`), while JSON data with a `"values"` key keeps the dict methods. A float index (even `1.0`) or bool index is a TypeError; a computed literal key `[e]: v` must be a `Str`.
- **Optional chains.** The parser wraps a postfix chain containing `?.` in `OptChain` (4.1). A `?.` step whose receiver is `null` ends the **whole chain** at once with `null`, as in JS: `a?.b.c()` with a null `a` evaluates neither `.b`, `.c` nor the call's arguments. Parentheses end the chain, so `(a?.b).c` fails when `a` is null. Only a null receiver of `?.` is guarded: `a?.b.c` with a non-null `a` and a null `b` fails at `.c`, and a missing dict key is still a KeyError. Assigning through `?.` is a SyntaxError (4.1). `eval_chain` returns `Ok(None)` for the short-circuit, and only `OptChain` turns it into `null`.

### 5.6 Environments and declarations

`Env = Rc<RefCell<Frame>>`; `Frame { layout: Rc<[Rc<str>]>, slots: Vec<Option<Slot>>, parent: Option<Env>, closures: u32 }`; `Slot { value, is_const: bool }`.

Every name V2 can declare is known statically, so `syntax/resolve` (run by `parse_file`, also for files parsed late, 6.4) gives each scope a **layout**, its names in insertion order (parameters, rest, loop variables, the catch name, hoisted `func`s, then the statements in order), and each `Name` and name assignment a position: how many frames up, and which index. An empty slot means that name is not declared yet. A condition, a `for` iterable and block args resolve in the enclosing scope. A name no scope declares resolves in the builtins frame or raises 5.6's `NameError`.

- **Frames:** builtins frame (shared, read-only: assigning to a builtin's name is `TypeError: cannot assign to builtin 'print' — shadow it with let`) ← one module frame per file ← call frames ← block frames. Each `if`/`elif`/`else` arm and `try`/`catch`/`finally` body gets a new frame; loops get **one fresh frame per iteration**, holding a `for`'s variables, in which the body runs directly; a call frame holds the parameters and is the body's top-level frame; a catch body's frame holds the catch name. Parameters, loop variables and the catch name are therefore declared in the body they introduce (4.5); inner blocks may shadow. A body that declares nothing and directly creates no function gets no frame (a frame there would only change when a closure's cycle is trimmed, below); module frames always exist. Reads and writes go by position; on an empty slot, either one falls back to walking `parent` by name through the layouts, skipping empty slots, so `x = 1` or `print(x)` before a local `let x` acts on the outer `x`, or raises. A `RefCell` borrow is held only while one slot is read or written.
- **Declarations:** `let x = e` evaluates `e`, then declares `x` (`let x = x + 1` in an inner block reads the outer `x`); `let x` binds `null`. `const`, `@import` names (13.4) and `extern` names (16.1) are const slots: assigning → `TypeError: cannot assign to constant 'LIMIT'` (shallow: a const list can still `push`). A name declared twice in one body is a static SyntaxError (4.5). An undeclared name → `NameError: 'x' is not declared` + did-you-mean (an assignment adds `— declare it with let`). One whose scope declares it further on (an empty slot in the innermost layout that has it) is `NameError: 'total' is used before its let on line 12` when that scope is the file's (`const`, `@import`, `import` likewise; `FileScope.top` keeps where), else `'step' is used before it is declared: its declaration comes later`, with no did-you-mean. A name a nested body declares (`let`, `const`, `func` in an `if`, a loop, a `try` or a function; `FileScope.inner`), used outside that body: `NameError: 'summe' is not declared here: its let on line 2 is inside the for loop, so it exists only there — declare it with let before the for loop, and change it inside with summe = …` (a function's: `… inside the function zeige, so it exists only there — return it from the function, or declare it with let outside it`), the nearest such declaration before the use, else the first after it, ahead of the did-you-mean.
- **Hoisting:** entering a body turns each `func` declaration directly inside it into a `Closure` over that frame before the first statement runs; reaching the statement later does nothing. **Closures** capture the `Env`, so `func() => i` made in a loop keeps its iteration's `i`.
- **Cycle trimming:** a closure stored in the frame it captures (every local `func` declaration, `let f = func(…) …`) is an `Rc` cycle. `Frame.closures` counts the closures made at or below a frame; leaving one with any runs 5.1's check ("Cycles through frames"), which clears the slots of a frame nothing outside reaches (releasing held handles). Cycles through containers are not trimmed.

### 5.7 Calls

The callee is evaluated first, then the arguments left to right; for `r.name(args)` the callee is resolved first too (the `Func` or handle under a dict key, or the method, or the null-receiver / unknown-method TypeError of 5.5), so an argument that changes `r` can't change what is called; `...xs` must be a list (`TypeError: spread argument must be a list, got dict`; a dict, JavaScript's spread for named arguments, also gets `hint: V2 has no ** for named arguments: write them by name, or call it in a block, as in @py(f, opts) { f(**opts) }`) and expands in place. `Closure` and `Builtin` (a `&'static [ParamSpec]`) share one binder:

1. Positional args fill the parameters before the rest parameter; extras go to the rest list, or without one → `ArityError: greet() takes 2 positional arguments but 3 were given`.
2. Named args match by name: unknown → `ArityError: greet() got an unexpected argument 'greting' — did you mean 'greeting'?` (among the parameters not given yet; `exec`'s `env`, `cwd`, `dir` or `environment` adds `hint: exec takes argv and input only: for a variable or another folder, start a shell, as in exec(["sh", "-c", "cd web && NODE_ENV=production npm run build"])`, on Windows `exec(["cmd", "/c", "cd /d web && set \"NODE_ENV=production\" && npm run build"])`, `builtins.rs` `named_hint`); already bound → `… got multiple values for argument 'name'`; the rest parameter → `… cannot take rest parameter 'nums' by name`.
3. Unfilled parameters without a default → `ArityError: greet() missing required argument 'name'` (all listed), checked before any default runs.
4. Push the `CallRecord` (5.8), check the depth (5.9), create the call frame (parent `Closure.env`) and walk the parameters in order: declare the bound value or the rest list (possibly empty), else evaluate the default **in the call frame** (it sees earlier parameters; re-evaluated per call). Parameters after the rest parameter are named-only (builtins only, such as `print`'s `sep` and `end`; in V2 code `...rest` must be last, 4.5).
5. Hoist the body's `func`s and run it; falling off the end returns `null`.

- `Proxy`, handle calls `h(…)` and `HandleMethod` forward positional and named args as given (`call`, 9.5). Named args to a session without the `kwargs` cap (js, ts; custom engines per 17.5) → `ArityError: {tag} functions take positional arguments only (got '{kw}')`, raised before sending (10.8, 17.5).
- `Native` (compiled exports, `extern`) takes positional args only (`ArityError: {name}() takes positional arguments only`), counted against the C signature (16.5), and runs in-process (16.7).
- A non-function → `TypeError: 'int' is not callable`; called by a builtin's name (`let str = ""`, then `str(i)`): `TypeError: 'str' is a str here, not a function` (`an int`, `null`), hint `a variable named 'str' hides V2's str(): give the variable another name`. An anonymous function is named `func@main.v2:12` in messages, `str()` and its trace frame (`func@main.v2:12 (main.v2:14)`, 2.5).
- V2 functions called from foreign code (`fn` and `route` calls, 11.8, 13.6; compiled `host_call`, 15.9) go through the same binder on the interpreter thread, inside the host's wait for that engine (9.7). They can arrive only while V2 has a request outstanding to that engine (9.8); a stored callback stays valid for later requests.

### 5.8 Control flow and frame discipline

Statements return `Exec<()>`, expressions `Exec<Value>`: `type Exec<T> = Result<T, Flow>`, `enum Flow { Error(Rc<ErrorValue>), Return(Value), Break, Continue, Exit(i32) }`. `exec(stmt, env)` and `eval(e, env)` take the environment as an **argument**, never interpreter state: a child frame is a local `Env` dropped on every exit path, including `?`, so a `catch` always runs with its own function's locals (R09, 22.4).

- The only interpreter-level stack is `calls: Vec<CallRecord { name: Option<Rc<str>>, call_site: Span }>`. Calls of non-builtin callables and module imports push a record and pop it with the result held in a local (`let r = self.run_call(..); self.calls.pop(); r`), never with a `?` in between.
- A call turns `Return(v)` into `Ok(v)`; `break`/`continue` never cross a function (static, 4.5). Loops: `Ok`/`Continue` → next iteration, `Break` → exit, anything else propagates.
- `try`: the body runs in a new frame; on `Flow::Error(e)` the `catch` body runs in a new frame with `e` bound; `Return`/`Break`/`Continue` are never caught. `finally` runs last for every flow except `Exit`: if it completes normally the pending outcome (value, flow, or a new error from `catch`) continues, else its own flow replaces it. Every error kind is catchable, `RecursionError` and engine errors included.
- `Exit(code)` (from `exit()`, 5.10) is caught by nothing and skips every `finally`; it unwinds through foreign frames (9.7) and native frames (15.9) to the top, where 6.5 ends the run. Interrupts never become a `Flow` (6.5).
- **Iteration** evaluates the iterable once: list → a snapshot of its elements; dict → a snapshot of its `(key, value)` pairs (one variable gets the key); str → chars; bytes → `Int`s; `a..b` → a lazy `i64` counter (both ends `Int`, else TypeError; `b <= a` → no iterations); handle → `op iter`/`next` (11.4); `range(…)` with positional arguments only, where `range` is the builtin, → a lazy counter like `a..b` (its step and count from `range_bounds`, as the list would hold), so `for i in range(10 ** 9)` makes no list. Two variables get `(index, item)`, or `(key, value)` for a dict or a mapping handle (11.4). Otherwise `TypeError: cannot iterate over int`, with `hint: to count, write for i in 0..10 (0 up to 9)` for an int, and 5.3's null hint for `null`.

### 5.9 Errors, traces and limits

- **Kinds.** Host: `Error TypeError ValueError KeyError IndexError NameError ArityError IOError RecursionError ImportError AssertionError`. Engines and crossings (ch. 7–17): `EngineUnknown EngineNotFound EngineCrashed EngineTimeout EngineExitError MarshalError BuildError ForeignError ProtocolError FFILoadError`. These 21 are catchable. `SyntaxError` is a diagnostic kind (2.4): it is reported before its file runs, so it never reaches a `catch` in that file (imported files: 6.4). `error(type, …)` accepts any non-empty type string.
- **Raising.** `throw v` throws an `Error` as is, turns a `Str` into `error("Error", v)`, else `TypeError: throw expects an error or a str, got int`. `span` and `trace` are filled when the error is first raised: by `throw` (the `throw` keyword), by a failing node (operator, call, index, member; spans per 2.2), or on surfacing from an engine (the mapped foreign location, 12.6). `error()` leaves both empty; a rethrow keeps them (`OnceCell`), so `==` still holds and the uncaught header points at the first `throw`.
- **Hint.** `e.hint` is what an uncaught error prints after `hint:` (2.4), or `null`: what to do, set where the error is made, so a program that catches it can show it too.
- **Trace** (innermost first): `loc` = raise span; for each `CallRecord` from the top, emit `(record.name, loc)`, then `loc = record.call_site`; finally `(None, loc)`. A proxy call's record emits the reply's mapped foreign frames (12.6) instead. Frame strings are 2.5; rendering and elision of long traces are 2.4 (`e.trace` keeps every frame).
- **Fields:** `type message file line col trace data lang foreign_type cause`; any other → `TypeError: error has no field 'x'` + did-you-mean. `file`/`line`/`col` are `null` until raised (`file` is the display path, 2.1); `trace` is a new list of strings on each read. `cause` is the error this one was raised from (a foreign exception's chained cause, 9.6), or `null`; `error()` can't set it, and an uncaught error prints it as `caused by:` (2.4).
- **RecursionError:** a call that takes `calls.len()` past **10,000** → `RecursionError: maximum call depth (10000) exceeded` at its span. A stack guard on calls and the less common expressions, at 7/8 of the stack, also covers deep expressions and nested foreign callbacks: each call and each `eval` of an expression node compares a local's address (`std::hint::black_box`) with the one recorded at thread start and raises the same error past 7/8 of the stack size. Cross-language nesting has its own limit of 64 (9.7).
- **Did-you-mean** (names, keys, fields, methods, named args; tags use 7.3): optimal string alignment distance on lowercased names, accepted when `d <= 1` for a name of up to 5 chars (`nope` is no typo of `type`), else `d <= min(3, (len + 2) / 3)`, and `d` is less than the longer name's length (so `y` never suggests `f`); a candidate that is the name typed without its accents (`error::folded`: the NFD form without the combining diacritics U+0300 to U+036F, lowercased, `ß` as `ss` and `ı` as `i`: `grosse` for `größe`, `buyut` for `büyüt`) counts as `d = 0`, as one in another case does; one that is the name in another Unicode form (the same NFC form: `café` decomposed, which looks the same) is never suggested, as the name itself isn't (for a file or a folder, 6.4's `unicode_twin` names those; a key gets no hint); the smallest wins, ties → one that starts with the name (`stat`: `stats`, not `star`), then innermost frame, then declaration order. An assignment's leaves out the builtins, which can't be assigned (`count = 1` isn't a typo of `round`).
- **Habits** come before did-you-mean. Names: `True`/`False` → `'True' is not declared — V2 writes true`; `None`, `undefined`, `nil`, `NULL` → `V2 writes null`; `console`, `System` → `V2 prints with print(...)`; `require` → `V2 imports a package with @import { name } from js.package`; `open`, `readFile`, `readFileSync` → `V2 reads a file with read_file(path) and writes one with write_file(path, text)`; `sorted` → `V2 sorts a list with xs.sort()`; `process` → `V2 gets the script's arguments with args() and prints without a newline with print(x, end: "")`; `sys` → `V2 gets the script's arguments with args() and stops with exit(n); a Python module is imported with @import py.sys as sys`; `Math` → `V2 has min, max, abs, round, sum and div(a, b) built in; JavaScript's Math works in a @js block` (so no `did you mean 'max'?`); `json`, `JSON` → `V2 reads JSON with json_parse(text) and writes it with json_stringify(value)`; `os` → `V2 has getenv(name), list_dir(path), exists(path) and exec([...]) built in; a Python module is imported with @import py.os as os`; `env`, `environ`, `ENV` → `V2 reads an environment variable with getenv(name)`; `map` → `V2 maps a list with xs.map(f)`; `filter` → `V2 filters a list with xs.filter(f)`; `enumerate` → `V2 loops with an index with for i, x in xs { ... }`; `zip` → `V2 has no zip: loop with for i in 0..len(xs) and read xs[i] and ys[i]`; `list`, `Array` → `V2 writes a list as [1, 2], and copies one with xs.copy()`; `dict`, `Object` → `V2 writes a dict as {a: 1}, and copies one with d.copy()`; `parseInt`, `parseFloat`, `Number` → `V2 converts with int(s) and float(s)`; `String` → `V2 converts with str(x)`; `floor`, `ceil`, `trunc` → `V2 has int(x) (toward zero), round(x) and div(a, b); Python's math and JavaScript's Math work in a block`; `isinstance` → `V2 checks a type with type(x) == "int"`; `random`, `randint`, `randrange`, `choice`, `shuffle`, `uniform` → `V2 gets random numbers from Python: @import { randint, choice } from py.random`; `math` → `V2 has min, max, abs, round, sum and div(a, b) built in; for more, @import py.math as math`; `sqrt`, `pi`, `pow`, `log`, `sin`, `cos`, `tan`, `exp`, `factorial`, `gcd` → `Python's math has it: @import { sqrt } from py.math`; `datetime`, `date`, `Date`, `now`, `today` → `V2's time() gives the seconds since 1970; dates come from Python: @import py.datetime as dt, then dt.datetime.now()`. A name a nested body declares comes before these (5.6). The did-you-mean takes the file's own names declared further on before the builtins: `'totl' is not declared — did you mean 'total'? (its let is on line 9, further on)`. `time.time()` and `time.sleep(1)`: `func has no method 'time' — time() is V2's own function: call it, as in time() for the seconds, and sleep(1) to wait`. Then, before did-you-mean, a name that a definition block of the file exports, or a block's label (`imports.rs` `import_hint`): `'spread' is not declared — block 'tools' exports it: add @import { spread } from tools` (an unlabeled block: `@py block at main.v2:9 exports it: add @import { f } from @py`), `'tools' is not declared — it is a block's label: bring the block's names in with @import { … } from tools`, and a name a block's top level defines (a line that starts `def NAME`, `async def`, `class NAME`; in JavaScript also `function`, `const`, `let`, `var`): `'double' is not declared — @py block at main.v2:1 defines it without exporting it: add @export { double } inside it, then @import { double } from @py` (`… exports it: add @import …` when the block has `@export { * }` and the name doesn't start with `_`). Members of V2's own values (`interp.rs` `habit`): `length`, `size`, `len` (str, list, dict, bytes) → `V2 writes len(x)`; `slice`, `substring`, `substr`, `subList` (str, list) → `V2 slices with x[a:b], as in xs[0:3]`; on a str `isdigit`, `isnumeric`, `isdecimal` → `V2 checks for a number by converting it: try { int(s) } catch (e) { ... }`, `count` → `V2 counts a piece of text with len(s.split(piece)) - 1`, `capitalize`, `title`, `isalpha`, `isalnum`, `isupper`, `islower`, `isspace`, `swapcase`, `zfill`, `casefold` → `V2's strings don't have it: do it in a block, as in @py(s) { s.title() }` (the method called: `s.zfill(5)` for `zfill`), `chars`, `toCharArray` → `V2 loops over a string's characters with for ch in s`, `charAt` → `V2 reads a character with s[i]`; `includes`, `contains` (str, list) → `V2 writes item in x`; on a list `find`, `findIndex`, `some`, `every` → `V2 writes xs.filter(f) (its first item: xs.filter(f)[0]), or xs.index_of(x) for a value`; `indexOf`, `index`, `find` → `did you mean 'index_of'?`; `toString` → `V2 writes str(x)`; on a list `append`, `add` → `'push'`, `extend`, `concat` → `V2 joins lists with +`, `forEach`, `each` → `V2 loops with for item in x { ... }`, `sorted` → `'sort'`, `reversed` → `'reverse'`, `remove`, `splice` → `V2 removes the item at an index with xs.pop(i) (xs.index_of(x) finds it), or keeps the others with xs.filter(func(y) => y != x)`, `count` → `V2 counts with len(xs.filter(func(y) => y == x))`, `clear` (list or dict) → `V2 starts over with a new one: xs = [] (or d = {})`; on a str `ljust`, `rjust`, `center`, `padEnd`, `padStart` → `V2 pads with " " * n, as in s + " " * (10 - len(s))`, `toUpperCase` → `'upper'`, `toLowerCase` → `'lower'`, `strip`, `lstrip`, `rstrip`, `trimStart`, `trimEnd` → `'trim'`, `startsWith`/`startswith` → `'starts_with'`, `endsWith`/`endswith` → `'ends_with'`, `splitlines` → `'lines'`, `format` → `V2 formats with f-strings`, `join` → `V2 joins a list with xs.join(sep)`; on a dict `items`, `entries` → `V2 loops over a dict with for key, value in d { ... }`, `has`, `has_key`, `hasOwnProperty`, `containsKey` → `V2 writes key in d`, `pop`, `delete` → `'remove'`, `set`, `put`, `update`, `setdefault` → `V2 sets a key with d[key] = value`. A member read of a dict key that starts with `_` and isn't there (`util._helper`) adds ` (an imported .v2 file keeps names that start with _ to itself)` to the `KeyError`; one named like a dict method (`d.keys` without the call) gets `hint: keys is a method of dicts, so call it, as in d.keys()`.

### 5.10 Builtins

`BuiltinDef { name, params: &'static [ParamSpec], f: fn(&mut Interp, Args, Span) -> Exec<Value> }` entries live in the builtins frame, so user names can shadow them. Notes beyond DOCS 3.11:

| Builtin | Notes |
| --- | --- |
| `print` | `str()` of each value joined by `sep`, then `end` (both `Str`, else TypeError); one write to `Out`, or with `stderr: true` (a `Bool`) one write to the host's stderr. |
| `input` | Writes `str(prompt)`, flushes (see Output), reads one line from the host's stdin and strips `\n` and one `\r` before it. `echo` must be a `Bool` (else TypeError, before the prompt). With `echo: false` and stdin a terminal (`builtins::EchoOff`), echo goes off before the prompt is written, so nothing typed after it shows: Unix clears `ECHO` on fd 0 with `tcsetattr`, keeping `ICANON` and the rest (`TCSANOW`, not getpass's `TCSAFLUSH`: what was typed ahead is read, not dropped, as on Windows); Windows clears `ENABLE_ECHO_INPUT` on the console input handle. Unix, first: a v2 in the background of that terminal (`tcgetpgrp(0)` another process group: `v2 x.v2 &`) calls `tcdrain(0)`, which stops it with SIGTTOU until `fg` makes it the foreground job, so the settings it then reads are the ones the shell gives a job, not its line editor's raw mode (which echo off would keep, and the end of the line put back); still in the background after it (SIGTTOU ignored, an orphaned job), it reads as plain input and leaves the terminal alone. While echo is off, the settings before are kept in `ECHO_SAVED` and `process::TERMINAL_CHANGED` is set, so Ctrl+C puts the terminal back even when stdout isn't one (6.5). The settings before come back once the read returns, and before a failed prompt write is reported (a closed pipe ends the run at once with 141, 6.5); after a line (not at end of input) `\n` goes to stdout, where the prompt went, as getpass writes one; Ctrl+C meanwhile puts echo back on the interrupt path (6.5: on Unix `restore_terminal`, on Windows `builtins::echo_back` with the mode kept in `ECHO_MODE`); Ctrl+Z gives the terminal its settings back while v2 is stopped and turns echo off again when it goes on in the foreground (`builtins::echo_around_stop`, 6.5). With stdin not a terminal (a pipe, a file) `echo: false` reads as usual and writes no `\n`. End of input before any byte → `null`, and the 1000th `null` in a row (no line read in between) → `IOError: the input has ended, and input() has given null 1000 times in a row`, `hint: check for null and stop, as in let line = input(); if line == null { break }`, so a menu loop that never checks stops; a last line without `\n` is returned; invalid UTF-8 → `IOError: input line 2 is not valid UTF-8: it may be in an older encoding, such as Windows-1252: send UTF-8 text` (lines counted over the run; what it is as for `read_file`, UTF-16 for the first line only), with `hint: the file sent as input isn't UTF-8: save it as UTF-8, or read it in a @py block with the code page of the Windows that saved it, as in @py(path) { open(path, encoding="cp1252").read() }: cp1252 for Western European text, cp1250 Central European, cp1251 Cyrillic, cp1253 Greek, cp1254 Turkish, cp1255 Hebrew, cp1256 Arabic, gbk Chinese, cp932 Japanese` (as for `read_file`) when stdin is a file (`v2 x.v2 < data.txt`, `stdin_is_file`: on Windows its handle's `GetFileType` is `FILE_TYPE_DISK`, since a pipe's handle reports a file's metadata there), else on Windows `hint: in cmd.exe, run chcp 65001 first; for Windows PowerShell 5.1, README.md's Troubleshooting has the line to run; a program that sends v2 its input must send UTF-8 (in Python, encoding="utf-8")` when standard error is a console (a shell, or a program run there: Python's `input=` with `text=True` sends its ANSI code page whatever chcp says; `builtins::piped_input_hint`), and when it isn't (a program runs v2 and reads what it says) `hint: the program that sends v2 its input must send UTF-8 (in Python, encoding="utf-8")`. A line that is UTF-8 up to a character cut at its end (`cut_utf8`, as for `read_file`: `head -c` cut it, or its writer stopped there) is `IOError: input line 4 is UTF-8 but ends inside a character: send whole characters`, with none of those hints, which would garble it. Every UTF-8 BOM at the start of the run's first line is dropped (Windows PowerShell 5 pipes start with one, two when `$OutputEncoding` is UTF-8 with its BOM, and Notepad's files with one). |
| `len`, `type`, `bool` | `len`: chars (5.4), bytes, list items, dict entries, a handle's `op len` (11.3), else `TypeError: len() needs a str, bytes, list or dict, got int`. `type`: 5.1. `bool`: 5.2 truthiness (no round trip). |
| `str` | A `Str` is returned unchanged; others get the canonical form: `null`, `true`, `42`; floats by Rust `{:?}` (`2.0`, `0.1`, `1e20`) plus `nan`, `inf`, `-inf`; strings inside containers double-quoted with `\" \\ \n \r \t` and `\u{..}` for other control chars, the invisible ones (U+00AD, U+200B–U+200F, U+2028–U+202E, U+2060–U+2064, U+FEFF: `["\u{feff}name"]`), WSL's stand-ins (2.1's `span::is_stand_in`: `["mod\u{f03a}x.v2"]`, as `list_dir` gives a Linux `mod:x.v2` on Windows) and the spaces other than U+0020 (U+00A0, U+1680, U+2000–U+200A, U+202F, U+205F, U+3000: `"a\u{a0}b"`, a no-break space from a web page), but a ZWJ or ZWNJ between two characters that are neither ASCII nor whitespace is kept (a family emoji, a Persian word) (`[1, "a"]`, `{"k": 1}`); bytes as `b"\x00A\xff"` (printable ASCII as is, `\" \\ \n \r \t`, other bytes as lowercase `\xNN`); `<func greet>`, `<func func@main.v2:3>`, `<func spread (py)>`, `<func fast_sum (rust)>`, `<func cos (extern)>`, `<builtin print>`; errors `ValueError: msg` or `ForeignError: ZeroDivisionError: msg`; handles via `op str` (11.3; a dead session raises EngineCrashed, 8.2, 11.7); native pointers `<native sqlite3* 0x…>` (16.6). |
| `int`, `float` | `int`: a float truncates toward zero (NaN, inf, out of range → `ValueError: cannot convert nan to int`); a str must be `[+-]?[0-9]+` after trimming (else `ValueError: invalid int: 'abc'`; beyond i64 → `ValueError: int out of range: '…'`); bool → 0/1. `float`: int → nearest f64, bool → 0.0/1.0, a trimmed str via `f64::from_str` (also `nan`, `inf`, `infinity`, any case), failure → ValueError. A str that isn't a number gets a hint by what it holds (`number_hint`), each one a change that makes it a number when followed, checked in this order: empty → `the text is empty: an empty line from input() is ""`; `0x1F` → `int() reads decimal digits: for hex text, convert it in a block, as in @py(x) { int(x, 16) }` (`float()` alike); a minus sign U+2212 (`−5`, as JavaScript's Intl writes it for sv-SE and fi-FI) → `the text has a minus sign U+2212, as Intl writes it: int(x.replace("−", "-"))`, or when the text needs more (`−0,5`), `… as Intl writes it: x = x.replace("−", "-") changes it; then ` and the hint for the text with `-`; `1_000` → `int() reads digits without _: int(x.replace("_", ""))`; U+066B, the Arabic decimal separator, with ASCII digits (`12٫5`) → `the text has U+066B, the Arabic decimal separator: float(x.replace("٫", "."))`, and with U+066C, the Arabic thousands separator, too (`1٬234٫5`) → `the text has U+066C and U+066B, the Arabic thousands and decimal separators: float(x.replace("٬", "").replace("٫", "."))` (U+066C alone: `the text has U+066C, the Arabic thousands separator: int(x.replace("٬", ""))`; `arabic_marks`); one to three digits, the first not 0, then groups of three with one separator before each (a space, U+00A0, U+202F, U+2009, `'`, U+2019, a comma or a point) and perhaps a decimal part, as locales write numbers (`1 234,5` fr-FR, `1’234.5` de-CH with U+2019, as Intl writes it, `1,234.56` en, `1.234,56` de, `1.000.000`; `grouped_hint`) → `the text has narrow no-break spaces (U+202F) between thousands and a decimal comma: float(x.replace("\u{202f}", "").replace(",", "."))` (the separator named, `spaces`, `no-break spaces (U+00A0)`, `thin spaces (U+2009)`, `apostrophes`, `apostrophes (U+2019)`, `commas` or `points`, and an invisible one written as its escape; `int(…)` without a decimal part), but not one point or comma before three digits and no more (`1.234`); other spaces between digits (`1 2`) → `int() reads digits without spaces between them: int(x.replace(" ", ""))` (a replace for each kind of space there); points and then a decimal comma not in groups of three → `the text has points between thousands and a decimal comma: float(x.replace(".", "").replace(",", "."))`, several points → `the text has points between thousands: int(x.replace(".", ""))`; for `int`, one point or comma before exactly three digits, after one to three digits that don't start with 0 (`1.234`: a decimal to some, 1234 to others; `0.123` is no thousand; `thousands`) → `if the point is between thousands, int(x.replace(".", "")) reads 1234; if it is a decimal point, use float(x), or int(float(x)) to drop what is after the point` (a comma: `if the comma is between thousands, int(x.replace(",", "")) reads 1234; if it is a decimal comma, int(float(x.replace(",", "."))) drops what is after it`); a decimal comma (`3,7`) → `write the decimal point as a point, as in 3.7: x.replace(",", ".") changes it`; `13.5` for `int` → `int() reads whole numbers, as in 13: for 13.5, use float(x), or int(float(x)) to drop what is after the point`; digits of another script (`١٢٣`, `１２`) → `int() reads the digits 0 to 9 only: for other digits, convert in a block, as in @py(x) { int(x) }`, with what Python refuses taken out first (`other_digits`): the bidi mark Intl writes before a sign (U+061C ar-EG, U+200E fa-IR and ps-AF, U+200F ckb), as a Python escape since it is invisible; ASCII commas between thousands, as bn-BD, mr-IN, ne-NP and my-MM write them with their digits (`১,২৩৪.৫`, the Indian `১২,৩৪,৫৬৭.৫`; as `grouped_hint` reads ASCII ones: after the sign and bidi marks one to three digits, the first not its script's zero (`zero_digit`), then two or three digits to each next comma and three after the last, none after the point: `between_thousands`; so a decimal comma stays, as in `৩,৫`, `०,१२३` (de-DE-u-nu-deva's 0.123) and `१२३४,५६७` (es-u-nu-deva's 1234.567), and the block, which Python refuses, never reads 123 or 1234567; one comma before three digits, `১,২৩৪`, is taken as between thousands, as in the locales that write these digits by default); U+066C; and U+066B as a point. For `int()`, U+066B makes the block `float`, and so does a point that can't be between thousands by the same rule (`१२.५`, `०.१२३`, mr-IN's `-१,२३४.५`); one that can (`१.२३४`, de-DE-u-nu-deva's 1234) keeps `int(x)`, which Python refuses instead of reading 1.234. So ar-EG's `١٬٢٣٤٫٥` gives `@py(x) { float(x.replace("٬", "").replace("٫", ".")) }`, its negative (U+061C, then `-`) `@py(x) { float(x.replace("\u061c", "").replace("٬", "").replace("٫", ".")) }`, and bn-BD's `১,২৩৪.৫` `@py(x) { float(x.replace(",", "")) }`; a number, then other text without digits (`13 Jahre`) → `int() reads the number alone, without other text: '13' would work`; no digits → `the text has no digits: a number is written with digits, as in 13`. `null` → `TypeError: int() can't convert null`, `hint: input() gives null when the input has ended, and a function without return gives null: check for null first`. Other types → `TypeError: int() can't convert list`; a handle (a `Decimal`, a `Fraction`) gets `hint: convert it in its language, as in @py(h) { int(h) }` (`float(h)`; js `Math.trunc(Number(h))`, `Number(h)`), and so does `${h:.2f}` (`float(h)`). For `int()` of an integer too big for V2 (a Python `int` or a JavaScript `bigint` handle, 9.1), which that would give back as the same handle, the hint is `this integer needs more than 64 bits, which V2's int doesn't have: keep the math in the block, or use @py(h) { str(h) } for its digits or @py(h) { float(h) }` (js `h.toString()`, `Number(h)`). |
| `range` | `range(end)` or `range(start, end, step = 1)`, all `Int`s, positional only (a named argument → `ArityError: range() takes positional arguments only`; none → `ArityError: range() missing required argument 'end'`); the length is computed first with checked arithmetic (over 100,000,000 → `ValueError: range too large: a list of 1000000000 items (100000000 at most)`, `hint: for a long loop, write for i in 0..n, which makes no list`) and the list reserved with `try_reserve_exact` (failure → `ValueError: range too large`); `step == 0` → `ValueError: range() step must not be zero`. |
| `json_parse` | The codec in plain mode (10.1) straight into `Value`, after one leading U+FEFF, which it skips as RFC 8259 allows and `read_file` does (text a block read from a file Windows tools wrote keeps it; columns count after it): integer tokens that fit i64 → `Int` (`-0` is the float `-0.0`, 10.2); integer tokens outside i64 → `ValueError: JSON at line L column C holds an integer beyond V2's 64-bit range (the JSON itself is fine): parse it in a @py block, as in @py(text) { import json; json.loads(text) }, or have the number written as a string`, raised by the 10.1 pre-parse byte scan, since serde_json would already have made them `f64` (never the nearest `Float`, 0.1; 10.2); tokens with `.`, `e` or `E` → `Float` (`1.0` stays a float); duplicate keys: the last value wins at the first key's position; `$v2` keys are plain data. Syntax errors, lone `\uD800` escapes and nesting past 1,000 levels (10.1) → `ValueError: invalid JSON at line L column C: …` (a lone surrogate: `a \u escape of half a surrogate pair (\ud800 to \udfff) without its other half, which isn't text`); only whitespace → `ValueError: invalid JSON: the text is empty`. |
| `json_stringify` | null, bool, int, finite float (keeps `.0`), str, list, dict. bytes, func, handle, error and non-finite floats → TypeError naming the path (`TypeError: cannot convert handle to JSON at [2]["k"] (use to_v2(h) first)`, 10.1; for a handle whose type `to_v2` can't convert, 11.5's way instead: `(a datetime.datetime isn't data: make plain data of it in a block, as in @py(h) { h.isoformat() })`); the others get a hint of what to store: a non-finite float `JSON has no NaN or infinity: store null instead (x != x is true for NaN), or write it with Python's json in a block, as in @py(v) { __import__("json").dumps(v) }`, bytes `JSON holds text, not bytes: store b.hex(), or b.decode() for UTF-8 text, or base64 from a block, as in @py(b) { __import__("base64").b64encode(b).decode() }`, a func `a function isn't data: store what it returns, or leave it out`, an error `an error isn't data: store its fields, as in {type: e.type, message: e.message}`; a cycle → `ValueError: cannot convert a cyclic list to JSON`; nesting past 1,000 levels → ValueError (10.1). `indent: null` → compact (`{"a":1}`); an int ≥ 0 → pretty with that many spaces, one item per line, `": "` after keys, empty containers as `[]`/`{}` (as in Python and JS); a negative int → ValueError. |
| `read_file`, `write_file` | Relative paths resolve against the process working directory (unlike `import` and `extern`, which use the `.v2` file's folder). Text reads are strict UTF-8 without one leading BOM (Windows tools write one; `binary: true` keeps it), no newline translation. Invalid → `IOError: 'x' is not valid UTF-8: <what it is>`, `hint: save it as UTF-8, or read it in a @py block with the code page of the Windows that saved it, as in @py(path) { open(path, encoding="cp1252").read() }: cp1252 for Western European text, cp1250 Central European, cp1251 Cyrillic, cp1253 Greek, cp1254 Turkish, cp1255 Hebrew, cp1256 Arabic, gbk Chinese, cp932 or euc-jp Japanese` (euc-jp is Linux's older Japanese; `code_page_way`; on a Windows whose own code page, `GetACP`, isn't cp1252, the example names that one and adds `(this Windows saves cp1254)`, as a Turkish Excel's CSV is in it), where it is `it is UTF-32 (what Out-File -Encoding utf32 writes)` (a UTF-32 BOM, `FF FE 00 00` or `00 00 FE FF`, looked for first, as the first starts with UTF-16's; then `encoding="utf-32"`), `it is UTF-16 (what Windows PowerShell's > and Out-File write)` (a UTF-16 BOM; then `encoding="utf-16"`), `it looks like binary data` (a NUL in the first 8 KiB; then only `read it with binary: true`), else `it may be in an older encoding, such as Windows-1252`. Text that is UTF-8 up to a character cut at its end (a log another program is still writing: `from_utf8`'s error is at the end, `error_len` `None`, and the valid part before it, a UTF-8 BOM counted, holds a byte past ASCII, so it isn't one byte of a code page at the end, as `caf\xE9` is: `cut_utf8`; after the UTF-16, UTF-32 and binary checks) is `IOError: 'growing.log' is UTF-8 but ends inside a character (a program may still be writing it)`, `hint: read it again once it is written, or without the cut character: @py(path) { open(path, encoding="utf-8", errors="ignore", newline="").read() }`, as a code page's advice would garble it. `write_file` takes `Str` (UTF-8) or `Bytes` (raw), else TypeError, and creates or replaces the file (no parent folders): via `fs::write`, but on Windows by `replace_file`, as `fs::write` cuts a file before it writes and another program's lock of a part of it (a database's) doesn't stop the cut, only the write, so a failed write left it empty. There the file is opened as it is (`OPEN_ALWAYS`, as `fs::write` does, so a hidden or a system file works), locked whole with `File::try_lock` (another program's lock anywhere in it, past the new data too, as SQLite's lock bytes are, refuses it on purpose before anything changed, as `ERROR_LOCK_VIOLATION` with its message and hint, below; a file system without locks goes on without one), cut with `set_len(0)` (a file another program has mapped refuses that, `ERROR_USER_MAPPED_FILE`, still whole), written and unlocked; never written first and cut after, which would leave a mapped file part old, part new. A handle that isn't a disk file's (`GetFileType` isn't `FILE_TYPE_DISK`: a named pipe, a console) is only written; `append: true` (a `Bool`, like `binary`) appends instead, creating a missing file. On Unix `write_file` to `/dev/stdout`, `/dev/fd/1` or `/proc/self/fd/1` writes through `Out` (`Out::write_bytes`), and to `/dev/stderr`, `/dev/fd/2` or `/proc/self/fd/2` as `print(stderr: true)` does: opened again, they would write before what `Out` holds, and a redirected file would start over at its beginning. Likewise `read_file` of `/dev/stdin`, `/dev/fd/0` or `/proc/self/fd/0` reads V2's own stdin (`read_path`): what `input()`'s buffer holds, then fd 0 to its end from where it is, so it gives what is left after `input()` (and after what a shell read before v2: `{ read -r h; v2 x.v2; } < data.csv`); opened again, a pipe lost up to 8 KiB and a file was read from its start. At most 1 GiB is read (`process::read_capped` with `read_cap()`: `MAX_READ`, the most text `*` makes, 5.3), in 64 KiB pieces, so memory stays bounded: a regular file whose size is more is refused before it is read, and a device or a pipe that holds more (`/dev/zero`) is dropped there: `IOError: cannot read '/dev/zero': it holds more than 1 GiB, the most V2 reads at once: read it in pieces in a block`, `hint: for its first MiB: @py(path) { open(path, "rb").read(1 << 20) }; for its last: @py(path) { f = open(path, "rb"); f.seek(-(1 << 20), 2); f.read() }` (forms that give a block's value, which a `with` statement doesn't; a device takes the first only). On Windows a pipe or a device WSL made, seen through `\\wsl.localhost` (its reparse tag `IO_REPARSE_TAG_LX_FIFO`, `LX_CHR`, `LX_BLK` or `AF_UNIX`: 6.4's `module::wsl_file`, looked at for a path on a `\\server\share` or a network drive only, or a relative one while the current folder is: `on_a_share`, as the look makes a small local read a quarter slower), which Windows takes for a file, is refused before it is opened: `IOError: cannot read 'fifo.v2': it is a pipe or a device, not a file`, `hint: Windows can't read WSL's pipes and devices: run V2 inside WSL`, and by `write_file`, `append: true` too, `cannot write 'fifo.v2': …`, `hint: Windows can't write to WSL's pipes and devices: run V2 inside WSL` (`wsl_special`); opened, a pipe waits 10 s for its other end and leaves a thread of WSL's file server waiting for it, and a device node reads Linux's device. On Windows a path in the device namespace that names no file (`\\.\pipe\x`, `\\.\COM1`, `\\?\pipe\x`, either slash: not a drive's, `UNC`'s or `Volume{…}`'s path after `\\.\` or `\\?\`; `device_path`) is looked at by nothing else, as each look opens it and each open of a named pipe takes one of its server's instances (`write_file` took four, none with the data): `on_a_share` is false for it, and a failure is the OS's reason alone (`root_error`). OS failure → `IOError: cannot read 'x': <os message>` (or `write`; for every file builtin), `it is a folder` instead when `read_file` is given one (`remove_file`: `it is a folder, and remove_file removes files only`; `write_file`: `it is a folder`; `list_dir` given a file: `it is a file, not a folder`; `make_dir` where a file is: `a file has that name`; a path that goes on past a file, `'notes.txt' is a file, not a folder`; one whose folder isn't there, `there is no folder 'out'`, with `hint: make the folder first, with make_dir("out")` for `write_file`; Windows's own words for these are "Access is denied", "The directory name is invalid", "The system cannot find the path specified" and "Cannot create a file when that file already exists"). Before those, in every file builtin, a link on the path that leads nowhere or in a loop (the path itself or a folder on the way, a junction too: `module::link_error`, 6.4's `link_problem`) is named, with what fixes it, not a `make_dir` hint that would fail there: `IOError: cannot write 'j/new.txt': 'j' is a link to C:\x\gone, which isn't there`, `hint: remove the link (rmdir j), or make C:\x\gone` (`it is a link …` for the path itself; the link as written, on Windows with `\` (`rmdir sub\j`), as cmd takes the `/j` of `sub/j` for a switch; `del` for a link to a file on Windows; `rm` elsewhere, and in Git Bash (`MSYSTEM` is set) with `/` (`rm sub/j`), as its bash drops a `\` and its rmdir refuses a junction: `module::remove_command`; a relative link's target is shown from the link's own folder, absolute, as `ln -s gone sub/j` points at `sub/gone`, and a `\\?\Volume{…}\…` target (a junction into a volume) as it is, having no other name: `module::link_target`; a loop: `'j' is a link in a loop`, `hint: remove the link (rmdir j), or point it somewhere else`). A Linux symlink WSL made (`IO_REPARSE_TAG_LX_SYMLINK`: seen through `\\wsl.localhost`, or made by WSL on a Windows drive; `module::wsl_file` reads the tag with `FILE_FLAG_OPEN_REPARSE_POINT` and `FileAttributeTagInfo`, after one `GetFileAttributesW` that finds no reparse point for any other file, and from the folder's listing, `FindFirstFileW`'s `dwReserved0`, for a link on a drive that Windows may not even open, `ERROR_CANT_ACCESS_FILE`), which Windows can't follow wherever it points, is no link that leads nowhere: `IOError: cannot read 'links/rel.v2': it is a Linux (WSL) symlink, which Windows can't follow`, `hint: use the path it points to (ls -l in WSL shows it), or run V2 inside WSL` (the share doesn't give Windows a link's target). On Windows, in every file builtin, a path on a drive that isn't there (`GetDriveTypeW` finds no root, which doesn't touch the drive: an unplugged USB drive, `x:y.txt` with no `X:`; also after `\\?\`) is `IOError: cannot write 'Q:\reports\out.csv': there is no drive Q: (is it plugged in or connected?)`, a UNC path whose share can't be reached (`ERROR_BAD_NETPATH`, `ERROR_BAD_NET_NAME`, `ERROR_CONNECTION_UNAVAIL` and the network's other errors: `network_error`) `IOError: cannot read '\\srv\docs\x.txt': the share \\srv\docs can't be reached (is that computer on and connected, and is the share's name right?)`, a `\\wsl.localhost` or `\\wsl$` path whose distribution isn't there (Windows' `ERROR_LOGON_FAILURE`, "The user name or password is incorrect", or a network's error, `network_error`: `ERROR_BAD_NET_NAME`, under load `ERROR_NETNAME_DELETED`) `IOError: cannot read '\\wsl.localhost\Ubuntu-22.04\x.txt': there is no WSL distribution named Ubuntu-22.04: wsl --list --quiet shows them`, and one that names none (`\\wsl.localhost` alone) `IOError: cannot list '\\wsl.localhost': \\wsl.localhost needs a WSL distribution's name after it, as in \\wsl.localhost\Ubuntu: wsl --list --quiet shows them` (`wsl_root`), and a network drive with one of those errors (a mapped drive whose computer isn't reached: `GetDriveTypeW` still knows its letter, as `DRIVE_REMOTE`; on a local drive such an error comes from a link to a share on the path, and gets the messages below: `drive_problem`) `IOError: cannot write 'Z:\reports\out.csv': the drive Z: can't be reached (is it connected?)`, with no hint (no folder to make; the escapes' hint for a path with a control character) (`unreachable_root`). On Windows a path under `/tmp` or `/var/tmp` where this drive has no such folder gets `hint: /tmp is Linux's and macOS's temp folder: on Windows use getenv("TEMP"), as in getenv("TEMP") + "/x.txt"` (`tmp_hint`) instead of the `make_dir` hint, as `make_dir("/tmp")` would make `C:\tmp`, a folder every user can write to (DOCS 16). A write that the OS refuses (PermissionDenied) to a file that is read-only gets `hint: 'ro.txt' is read-only: clear that with attrib -r ro.txt (or in its Properties)` on Windows (`FILE_ATTRIBUTE_READONLY`; the path with `\`, in quotes when it has a space or another character a shell reads, and `.\` before a name that starts with `-`, which attrib would read as an attribute: `attrib -r .\-ro.txt`), elsewhere, for a file of this user without the owner's write bit, `hint: 'ro.txt' is read-only: make it writable with chmod u+w ro.txt` (`./-ro.txt` for a name that starts with `-`, which chmod would read as an option) (`read_only_hint`). On Windows, `ERROR_SHARING_VIOLATION` (32), `ERROR_LOCK_VIOLATION` (33) and `ERROR_USER_MAPPED_FILE` (1224), a file another process opened without sharing (Excel keeps an open file so), locked a part of or mapped, get `hint: another program has it open (Excel keeps an open file locked; so does a block of this script that opened it): close it there` in every file builtin, a block's worker being another process, and `write_file`'s adds `, or write to another file name` (`busy_hint`); `remove_file` refused with `ERROR_ACCESS_DENIED` (5) for a `.exe` or a `.dll` (`program_file`: a running program's, which Windows keeps) gets `hint: if that program is running, stop it first` (5 also comes from permissions, hence the if). Paths in these messages show control characters escaped (`\n`, `\t`, `\u{1b}`) and bidi ones (`\u{202e}`, 2.1), and an uncaught one whose path holds a control character gets `hint: in "…", a backslash starts an escape (\n is a line break, \t a tab): write Windows paths with / or \\, as in "notes/new.txt"`. A relative path that isn't found but names a file next to the `.v2` file that calls `read_file` gets `hint: relative paths start in the folder you ran v2 from; for the file next to the script: read_file(f"${script_dir()}/data.csv")` (the path as written, `\` as `/`). A path whose first part is `~`, on Unix `~` and a user up to the first `/` (`~root`: when `getpwnam` finds that user, as bash asks before it puts that user's home folder there; a name no user has, Word's `~WRL0001.tmp` or a folder `~old`, is taken as written, and on Windows, whose shells expand no `~user`, so is every `~name`: `other_home`) or a variable a shell would expand (`%NAME%`, `${NAME}`, `$env:NAME`, `$NAME` in capitals: `config::unexpanded`) is refused before the OS sees it, by `read_file`, `write_file`, `list_dir`, `make_dir`, `remove_file` and `exists` (where `false` would hide a file that is there), as making a folder named `~` here, which a later `rm -rf ~` would follow home, is never what was meant: `IOError: cannot create '~/out': V2 doesn't expand ~ in paths`, hint `write getenv("USERPROFILE") + "/out", or ./~ for a folder that is really named ~` (`HOME` off Windows; for a variable, `write getenv("USERPROFILE") in its place, or ./%USERPROFILE% …`; for `~root/x`, `V2 doesn't expand ~root in paths`, hint `write the path of root's home folder (what ~root is in a shell), or ./~root for a folder that is really named ~root`). A path that isn't there and holds such a variable later on gets `hint: V2 doesn't expand %USERPROFILE% in paths: write getenv("USERPROFILE") in its place`. On Windows `$HOME`, `${HOME}` and `%HOME%` where `HOME` isn't set (PowerShell's `$HOME` is its own variable and cmd has none; Git Bash sets it) name `getenv("USERPROFILE")` in both hints, as `~` does (`home_var`). On Windows a `:` after the drive, or after a UNC path's `\\server\share` with either slash (`module::after_root`), is refused first (`notes 12:30.txt` names a hidden stream of `notes 12`, so a write would seem to work; WSL's share takes it for a Linux name's, which only the `\\` spelling reached): `IOError: cannot write 'notes 12:30.txt': a Windows file name can't have ':' in it`, `hint: use another character, as in 12-30 for a time`, or, when the name with WSL's stand-ins is there (`module::stand_in_twin`: a Linux `mod:x.v2`, `mod\u{f03a}x.v2` to Windows), `hint: WSL made one of that name, which Windows shows with \u{f03a} for ':': reach it as read_file("mod\u{f03a}x.v2"), or rename it in WSL` (`stand_in_hint`; the same after the OS refuses a name with `* ? " < > |`, before the hint below) (a `\\?\` or `\\.\` path is taken as written). Then, also before the OS sees it (`windows_name`; a `\\?\` or `\\.\` path is taken as written), a path that Windows would take for another: one with a part other than `.` and `..` that ends in a dot or a space, which Windows drops (`remove_file("data.txt.")` deleted `data.txt`, `write_file("report.")` wrote `report`), is `IOError: cannot remove 'data.txt.': a Windows file name can't end in a dot or a space ('data.txt.' would be 'data.txt')`, and one that `GetFullPathNameW` makes a device (`\\.\…`: on Windows 11 a bare `CON`, `PRN`, `AUX`, `CONIN$`, `COM1` to `COM9` or `LPT1` to `LPT9`, and `NUL` in any folder, a relative path's too (from a cwd on a share, `weird\NUL` is the device), while in a path written as `\\server\share\…` these are ordinary names; `read_file("CON")` waited for typed input, `write_file("nul")` threw the data away) is `IOError: cannot read 'CON': CON is the name of a device on Windows`, while `sub\aux` and `aux\x.txt` are ordinary paths there (below). Both get `hint: Windows doesn't allow this file name (…): choose another name` (below), or, when a file of that name is there (WSL and Macs make them: the path through its last part that ends in a dot or a space, `through_last_dotted`, so for `write_file("dir./new.txt")` the folder `dir.`; for a device the whole path; the path made absolute by its text, `.` and `..` taken out, with `\\?\` in front, also from a cwd on a share: `verbatim`; with `dotted_name`, which an import and FILE use too, 6.4, 19.1), `hint: Windows doesn't allow this file name (…), but one of that name is there (WSL and Macs can make one): reach it by its full path with \\?\ in front, as in read_file("\\\\?\\C:\\data\\notes.txt ")` (the builtin that was called, and that path). On Windows, `/dev/stdin`, `/dev/stdout` and `/dev/stderr` get `hint: /dev/stdin, /dev/stdout and /dev/stderr are Linux's and macOS's: on Windows read standard input with input(), a line at a time, and write with print() and print(…, stderr: true)`, whatever the current folder (on a `\\wsl.localhost` share they are WSL's links, and get no link message). In every file builtin, a path that isn't found whose folder has its last name in another Unicode form (6.4's `unicode_twin`: `daten_ü.csv` copied from a Mac) gets `hint: there is a 'daten_ü.csv' whose name looks the same but is written in decomposed Unicode (letters plus combining accents, as macOS writes names): rename it`; so does a folder on the way that isn't there, by the first missing one (`module::folder_twin`: `IOError: cannot write 'ördner/new.txt': there is no folder 'ördner'`, `hint: there is a 'ördner' whose name looks the same …: rename it`), before the `make_dir` hint, which would make a second folder that looks the same. On Windows, in every file builtin: a path that isn't found whose first part is one letter (`/c/Users/me/x.txt`, Git Bash's spelling, when the drive `C:` exists and the current drive has no folder `\c`; or `/mnt/c/…`, WSL's, with no `\mnt\c`: `unix_drive`, `msys_hint`) gets `hint: on Windows, /c/… is Git Bash's spelling of C:/…: write "C:/Users/me/x.txt"` (`/mnt/c/… is WSL's spelling of C:/…`; before the `make_dir` hint, which would make `C:\c\Users\…`); one whose first folder is Linux's own (`/root`, `/home`, `/usr`, `/etc`, `/opt`, `/srv`, `/var`) and not on this drive gets `hint: a Linux path: from Windows, WSL's files are \\wsl.localhost\<distro>\…` (`linux_path_hint`, also for exec's path and 19.1's FILE); and a path with a part Windows refuses as a file name (`bad_windows_name`: a device, `CON`, `PRN`, `AUX`, `NUL`, `COM1` to `COM9`, `LPT1` to `LPT9`, when `GetFullPathNameW` makes the path through that part `\\.\…`: on Windows 11 a bare name, and `NUL` in any folder of a path not written as `\\server\share\…`, while `sub\aux` and `.\con` are folders there, as is a bare first part with more after it (`aux\x.txt`) when `.\aux` is a folder, which also keeps `there is no folder 'aux'` from its message; Windows 10 also in a folder and with an extension, `con.txt`; one that ends in a dot or a space; one holding `< > " | ? *`; a `\\?\` or `\\.\` path is taken as written) gets `hint: Windows doesn't allow this file name (CON, PRN, AUX, NUL, COM1 to COM9 and LPT1 to LPT9 are devices; a name can't end in a dot or a space, or hold < > " | ? *): choose another name`, where the OS says only `The filename, directory name, or volume label syntax is incorrect` or that the path isn't there. On Windows, in every file builtin, after a link's and an unreachable root's messages and before the others, a path with a part longer than a Windows file or folder name can be (255 UTF-16 units, an emoji 2: `long_name`), which Windows says as a bad name or, for a folder on the way, as one that isn't there (whose `make_dir` hint would fail too), is `IOError: cannot write 'nnn….txt': the name 'nnnnnnnnnnnnnnnnnnnn…' is 256 characters long, and a Windows file or folder name can have at most 255` (the name's first 20 characters; `… long as Windows counts them (an emoji is 2), …` when its UTF-16 units aren't its characters), `hint: shorten it`. `write_file` of a list → `TypeError: write_file() argument 'data' must be a str or bytes, got list`, `hint: write one item per line with xs.join("\n") (read it back with read_file(f).lines()), or the whole list with json_stringify(xs)` (a dict: `write it as JSON with json_stringify(d) (read it back with json_parse(read_file(f)))`; a number or bool: `write it as text with str(x)`). |
| `exists`, `list_dir`, `make_dir`, `remove_file` | Paths as for `read_file`. `exists`: whether `fs::metadata` succeeds (symlinks followed; any error → `false`; `""` → `false`, the refusals above raise). An empty path is `IOError: cannot create '': the path is empty` in the others (`create_dir_all("")` succeeds, making nothing). `list_dir`: a new list of the entry names, sorted by code point; a name that is not valid UTF-8 → `IOError: cannot list 'x': the name 'bad�.txt' is not valid UTF-8 (� marks what isn't), and V2's strings are: rename it` (the name shown lossily, U+FFFD for each bad byte, or on Windows each lone surrogate), with `hint: to list such names as they are, keep them as bytes in a @py block: @py(d) { import os; os.listdir(os.fsencode(d)) }, and open one there with os.path.join(os.fsencode(d), name)`. `make_dir`: `fs::create_dir_all` (missing parents too; an existing folder is fine); returns `null`. On Windows a Git Bash or WSL path (`unix_drive`, as for `read_file`) is refused first, as it would make `\c\Users\me\out` (`\mnt\c\…`) on the current drive: `IOError: cannot create '/c/Users/me/out': there is no folder '/c'` (`'/mnt/c'`), with that hint; so is `/tmp/…` or `/var/tmp/…` where this drive has no such folder (`tmp_hint`): `IOError: cannot create '/tmp/work': on Windows that makes \tmp at the top of the drive, a folder other users can write to` (for `/var/tmp/…`, `\var\tmp`: `tmp_refusal`; `unix_tmp` finds which of them the path starts with), with the `/tmp` hint above; and so is another Linux path whose top folder this drive doesn't have (`linux_path_hint`'s, `/root/out`): `IOError: cannot create '/root/out': on Windows that makes \root at the top of the drive, a folder other users can write to`, with `hint: a Linux path: from Windows, WSL's files are \\wsl.localhost\<distro>\…` (`linux_dir_refusal`). `create_dir_all`'s own text for a root it can't make, `failed to create whole tree` (no OS code; `whole_tree_error`), is never shown: the root's own error is used instead (`fs::metadata` of the path's root, its last ancestor), so a drive or share that isn't there says so as above; newer Rust versions give the OS's error there instead, or none for the root itself (`Z:/`), so a folder still not there after `create_dir_all` succeeded gets its own `fs::metadata` error, the same message. Other errors without an OS code are Rust's true reason and stay as they are, as in the other file builtins: a NUL in the path is `IOError: cannot create 'a\u{0}b': strings passed to WinAPI cannot contain NULs` (`file name contained an unexpected NUL byte` elsewhere). `remove_file`: `fs::remove_file`, so a folder is an IOError, with `hint: a @py block removes a folder with what is in it: @py(path) { import shutil; shutil.rmtree(path) }`; a link goes, not what it points to, as `rm` does: on Windows, which removes a link to a folder as a folder, one that leads to a folder (`FileTypeExt::is_symlink_dir`: a junction or a directory symlink) is removed with `fs::remove_dir`, which takes the link alone, unless it is written with a separator at its end (`live/`, `live\`, `live/.`: `module::trim_separators`), which names the folder; there, on every OS, a link to a folder (`folder_link`) is `IOError: cannot remove 'live/': it is a link to C:\x\target, a folder, and remove_file removes files only` (its target as `module::link_target` shows it), `hint: to remove the link alone, leave out the slash at its end: remove_file("live")`, never the rmtree hint, as rmtree refuses a link and, given `live/` on Linux, would empty the folder (another failure on such a link is the OS's reason); on Windows a link to a folder that leads nowhere keeps `link_error`'s message (on Linux and macOS `unlink` removes it); a read-only file is removed on every OS, as `rm` does (on Windows Rust's `remove_file` ignores the read-only attribute, unlike `del`). OS failure → `IOError: cannot list 'x': <os message>` (or `create`, `remove`). |
| `script_dir` | The parent of `SourceFile.path` (2.1) of the call's span: the folder of the file containing the call, not the entry file's. |
| `exec` | `argv`: a non-empty list of `Str`s (else TypeError, or ValueError when empty or when one holds a NUL: `exec() argument 'argv' holds a NUL character at [1]: "a\u{0}b": a program's name and arguments can't contain one`; a `Str`'s TypeError shows the list, its words split on whitespace: `exec() argument 'argv' must be a list, got str — as in exec(["git", "status"]): exec runs a program, without a shell`; when the first word is a command of the shell, as below, the message ends at `got str` and the hint is that command's: `exec("dir")` → `hint: 'dir' is a command of cmd.exe, not a program: write exec(["cmd", "/c", "dir"]) …`). `argv[0]` is found by 7.5 with the cwd as base. Cwd and environment are the host's, plus `PYTHONIOENCODING=utf-8` when it isn't set: a Python program writes a pipe in the locale's encoding, the ANSI code page on Windows, which the console code page below would misread (`café` as `cafÚ`) and which can't hold CJK at all. `input`: `null` → the null device; a `Str` or `Bytes` is piped to the child's stdin (8.3). `process::run_captured` collects stdout and stderr completely (no timeout), each up to 1 GiB (`read_cap()`, as `read_file`): a reader past that kills the tree, so the wait ends, and drops what it read, and `exec` raises `IOError: exec: 'yes' wrote more than 1 GiB of output: send it to a file in the command, or read it in pieces in a block` (an endless program, a zip bomb unpacked to stdout). It decodes them as UTF-8 leniently (U+FFFD), after UTF-16: output that starts with a byte order mark, or on Windows has NULs in at least half of its odd bytes and in at most a quarter as many even ones (Latin text in UTF-16LE, as `cmd /u` and `wsl --list` write it; text in UTF-8 or a code page has none), is decoded as UTF-16 (`utf16_text`); on Windows, other output that isn't UTF-8 is first decoded with the console's output code page (`GetConsoleOutputCP`; the OEM code page without a console; none for a UTF-8 console), as `cmd /c dir` and other console programs write it; the child's tree is killed when `exec` returns (8.3) and on an interrupt (6.5). Returns a new dict `{code, stdout, stderr}`: `code` is the exit code (Unix, killed by a signal: 128 + its number; Windows: the unsigned 32-bit code, as 8.6 prints it); a non-zero code is not an error. Not started → `IOError: exec: cannot start 'gitt': <reason>` (`not found on PATH`, or with `PATH` unset or empty `exec: cannot start 'ls': not found, as PATH isn't set: set it, or give the program's full path`; `python` when only `python3` is on PATH gets `hint: this system names it python3: write exec(["python3", …])`; `there is no such file` for a path that isn't there, which for a batch file cmd.exe would only report as its output, with the Git Bash hint of the file builtins for `/c/tools/x.exe` (WSL's for `/mnt/c/…`, and the Linux path one for `/usr/bin/python3`), or the OS message). When `argv[0]` is `cmd.exe` and `/c` or `/k` (any case) is the last argument but one, that last one goes on the command line as written (`raw_arg`, `Spawn.raw_tail`): it is a command line for cmd, whose quotes Rust's quoting for a program (`\"`) would break, so `exec(["cmd", "/c", "cd /d \"my dir\" && dir"])` works as typed. On Unix the program's `argv[0]` is its name as written (7.5). Every other argument reaches the program as given, on Windows by 8.3's command line (Git Bash's `find . -name "*.v2"` gets `*.v2`, not the names it matches), except that on Windows one that starts with `\\?\` is read as 2.1 says (`/` as `\`, `.` and `..` taken out: `verbatim_arguments`), so `exec(["python", f"${script_dir()}/x.py"])` works in a script run by its `\\?\` path. A name not found that is a command of the shell, not a program (Windows, cmd.exe's: `assoc call cd chdir cls color copy date del dir echo erase ftype md mkdir mklink move path pause popd pushd rd ren rename rmdir set start time title type ver vol`; elsewhere: `alias cd eval export set source ulimit umask unalias unset`), gets a hint: `'copy' is a command of cmd.exe, not a program: write exec(["cmd", "/c", "copy", "a b", "c"])` (the arguments as given; elsewhere `exec(["sh", "-c", "copy a b c"])`, `the shell`; `dir` adds ` (or list_dir(path) for the names in a folder)`), and for `cd`/`chdir`, `… not a program: to run a program in another folder, write exec(["cmd", "/c", "cd /d build && program"])` (`sh`, `-c`, `cd build && program` elsewhere). Otherwise a name not found that is a file in the cwd (with 7.5's extensions on Windows, whose own shells run it so) gets `hint: V2 doesn't look in the current folder for a bare name: write exec(["./build.bat"])`, or on Unix, when that file has no execute bit, `hint: V2 doesn't look in the current folder for a bare name, and ./job.sh isn't marked as a program: write exec(["sh", "./job.sh"]), or mark it once with chmod +x ./job.sh and write exec(["./job.sh"])` (a name with no known extension: `… mark it once with chmod +x ./tool and write exec(["./tool"]), or run it through its program, as in exec(["sh", "./tool"])`; a path that can't start for that reason, `Permission denied`, gets the same without the first clause, `unmarked_hint`); one in a `node_modules/.bin` of the cwd or a folder above it (`NAME`, or on Windows `NAME.cmd`), `hint: prettier is in node_modules/.bin, where npm puts a package's programs: run it with npx, as in exec(["npx", "prettier", …])`; on Windows, else, `ls`, `grep`, `sed`, `awk` or `sh`, `hint: run Linux programs with exec(["wsl.exe", "-e", "ls", "-la"])` (the arguments as given, which `-e`, unlike `--`, passes on unchanged: `linux_program_hint`). A path that is a folder: `exec: cannot start './tools': it is a folder, not a program`. A script that doesn't start (Windows: "not a valid Win32 application"; elsewhere no `#!` or execute bit) gets the program that runs it by its extension: `hint: a .ps1 file runs through its program: write exec(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "./build.ps1"])` (`pwsh -File` elsewhere; `.py`: `python` (`python3` elsewhere) and `, or run its code in a @py block`; `.js`, `.mjs`, `.cjs`, `.ts`, `.mts`, `.cts`: `node`; `.sh`: `sh`, but on Windows Git for Windows' bash by its full path, found as 7.6 finds the bash engine's (`git_bash`): `exec(["C:/Program Files/Git/bin/bash.exe", "./build.sh"])`, never a `bash` on PATH, which is often WSL's launcher and runs Linux, and without one `hint: a .sh file needs a shell to run it, such as Git for Windows' bash (winget install -e --id Git.Git): then write exec(["C:/Program Files/Git/bin/bash.exe", "./build.sh"])`; `.v2`: `v2`). |
| `sleep`, `time` | `sleep`: an `Int` or `Float` (else TypeError); negative, NaN or beyond `Duration` → ValueError; `thread::sleep` on the interpreter thread, so an interrupt still ends the run through the watchdog (6.5). `time`: `SystemTime` since `UNIX_EPOCH` as a `Float` of seconds. |
| `min`, `max` | Exactly one argument that is a list → its items; otherwise the arguments. All numbers or all strings (else TypeError), compared by 5.2's ordering; an item replaces the current best only when strictly smaller (larger), so the first wins ties. No values → `ValueError: min() of an empty list` (`max()`). |
| `sum`, `abs` | `sum`: a list of `Int`s and `Float`s (else TypeError), folded from `0` left to right with 5.3's `+` (int overflow → ValueError; any float makes the result a float); `sum([])` is `0`. `abs`: an `Int` via `checked_abs` (overflow → ValueError) or a `Float`; else TypeError. |
| `round` | `digits`: an `Int` (negative → ValueError). An `Int` `x` is returned unchanged; a `Float` is formatted as `${x:.Nf}` does (5.4, ties to even on the exact binary value) and parsed back: `digits` 0 → an `Int` (NaN, inf or beyond i64 → ValueError, as `int()`), > 0 → a `Float`; from 324 on, `x` unchanged (no f64 needs more decimals). |
| `div` | Two `Int`s (else TypeError): `checked_div`, minus 1 when the remainder is non-zero and the signs differ (floor, so `div(a, b) * b + a % b == a`). `b == 0` → `ValueError: division by zero`; `div(-2^63, -1)` → ValueError. |
| `getenv`, `args` | `var_os`/`args_os`, decoded leniently (U+FFFD; on purpose, 0.1). A name that is empty or contains `=` → `ValueError: invalid environment variable name: 'A=B'` (std may panic on it); one with a NUL is shown quoted, `invalid environment variable name: "A\u{0}B": a name can't contain a NUL character`. `args()` returns a new list each call. A file builtin's IOError for a path holding U+FFFD, while an argument of the run (else an environment variable) isn't UTF-8, gets `hint: the path holds U+FFFD where an argument wasn't UTF-8: rename the file to a UTF-8 name, or use a UTF-8 locale` (`an environment variable`; `lossy_hint`), after the control-character one. |
| `exit` | `code`: an `Int` in i32 range, and on Windows also 2147483648 to 4294967295 (`exit_code`), passed on as the same 32 bits, so `exit(r.code)` passes on `exec`'s unsigned code of a crashed program (else TypeError / `ValueError: exit code 1099511627776 is out of range` with `hint: exit() takes a code from -2147483648 to 4294967295 on Windows`, elsewhere `hint: exit() takes a code from -2147483648 to 2147483647, and a shell sees only 0-255: use those for a code a shell checks`; POSIX keeps the low 8 bits; Git Bash on Windows, which V2 hands the whole code, shows the low 8 bits too, but a code from -1073741824 to -1 as 127, except three crash codes it reads as 128 + a signal: an access violation (-1073741819) as 139, an illegal instruction (-1073741795) as 132 and Ctrl+C's code (-1073741510) as 130 (`exit(-1)` and a stack overflow's -1073741571 read as 127, `exit(-2147483648)` as 0): DOCS 3.11). Returns `Flow::Exit(code)` (5.8, 6.5). |
| `error`, `assert` | `error`: `type` (non-empty) and `message` must be `Str`, `data` anything; no location until raised. With no `message` (or `null`), JavaScript's `new Error("text")` habit: `ArityError: error() needs a type and a message, as in error("ValueError", "text"); throw "text" throws a plain Error`. `assert`: a falsy `cond` (5.2) throws `error("AssertionError", message)` (`message` a `Str`), raised at the call (2.2). |
| `to_v2` | A handle → `op value` (11.5; native pointer → MarshalError, 16.6); anything else is returned unchanged (containers are not walked). |
| `define_engine` | 17.2: one custom engine, for the rest of the run. |
| `engine_available` | 7.3. |

**Output.** `Out` (`BufWriter<StdoutLock<'static>>`, 64 KiB, owned by `Interp`) is the only writer of the host's stdout: `print` and the stdout text of worker `out` frames (9.9). `Out` is flushed after every write when stdout is a terminal (`IsTerminal`, checked once), otherwise at 64 KiB, at a write that comes 100 ms or more after the last flush (so a reader that went away is met soon), or by a thread of its own 100 ms after a write it held (`flusher`: it writes `PENDING` to the descriptor under the same lock, so nothing changes order, and leaves the bytes to `Out` when that fails), so a file or a pipe gets a slow program's output as it comes, the line before a long computation included, and before every in-process native call (16.7), every frame written to a worker and every wait for a worker's next frame (9.9), every child spawn (a statement-position compiled or `run` block inherits stdout, 6.3), every `sleep()`, every `input()` read, every write to stderr (diagnostics, `out` stderr text, `print(stderr: true)`), and at the end of the run; dirty native stdio is flushed before every write to `Out` (16.7). A failed write of `print` to either stream raises IOError at the `print`; a closed pipe (Rust ignores SIGPIPE, so it is `EPIPE`) stops the run instead (6.5). A flush on the way (before `sleep`, `exec`, stderr text, a frame to a worker) that meets a closed pipe stops the run there, as such a `print` does (6.5), so `v2 x.v2 | head -1` doesn't run the rest of a script that prints and sleeps; another failure waits for the next write or the end of the run. With fd 1 closed (`v2 x.v2 >&-`), what is printed is dropped without a word, as Python's `print` drops it: Rust's stdout takes a closed handle as a sink, and a script run for what it does, not what it prints, keeps working. Stdin belongs to `input()` alone (workers: 9.9; other children: 8.3).

### 5.11 Methods

Arguments bind as for builtins, and messages name the method with its type: `ArityError: list.get() takes 2 positional arguments but 3 were given`, `TypeError: list.join() argument 'sep' must be a str, got int`. `push`, `insert`, `sort`, `reverse` return `null`; `pop` and dict `remove` return the removed item. Unknown method → `TypeError: list has no method 'append'` + did-you-mean. Methods are not values: `xs.push` without a call is the field TypeError of 5.5. There is no `len()` method: lengths come from `len(x)` (5.10).

| Type | Methods |
| --- | --- |
| str | `upper()`, `lower()` (full Unicode mapping); `trim()` (`char::is_whitespace`); `split(sep = null)` (`null`: `split_whitespace`, so no empty parts; a `Str` must be non-empty, else `ValueError: str.split() separator must not be empty`, and keeps empty parts); `lines()` (at `\n`, `\r\n` and a lone `\r`, an old Mac file's, as Python's `splitlines` for those; no trailing empty line: `methods.rs` `text_lines`); `replace(old, new)` (all matches, left to right; empty `old` → `ValueError: str.replace() argument 'old' must not be empty`); `starts_with(p)`, `ends_with(p)`; `index_of(sub)` (code-point index or `-1`); `encode()` |
| bytes | `decode()`, `hex()` (5.4) |
| list | `push(...items)` (appended in order; none is fine); `pop(i = -1)` (negative from the end; empty → `IndexError: pop from empty list`; out of range → `IndexError: list index 9 out of range (length 5)`); `get(i, default = null)` (`Int`, negative from the end; out of range → `default`); `insert(i, x)` (negative `i` adds the length, then clamps to `[0, len]`); `sort(key = null, reverse = false)` (a key function that changes the list's length: `ValueError: list modified during sort()`, as Python says); `reverse()`; `map(f)`, `filter(f)` (over a snapshot; `f` gets exactly one argument; new list); `join(sep)` (`str()` of each item); `index_of(x)` (first `eq`, or `-1`); `copy()` (shallow) |
| dict | `keys()`, `values()` (new lists, insertion order); `get(k, default = null)`; `remove(k)` (a hole in `DictMap`, 5.1: the rest keeps its order, nothing shifts; missing → KeyError); `copy()` (shallow). `get`/`remove` keys must be `Str` (TypeError). |

`sort` is stable and in place. A `key` is called first, once per item, in order, with exactly one argument. Then the keys (without `key`, the elements) must be all numbers or all strings (else `TypeError: cannot sort a list mixing int and str`, or `… containing list (only numbers or strings)`); numbers use 5.2's exact order with NaN last, so the comparator is total and runs no user code; the sorted items replace the list's contents. `reverse: true` reverses the comparator, so equal items keep their order. **Borrow rule:** no `RefCell` borrow of a list, dict or frame is held across user code or a session round trip (a callback may mutate the same list; `op str` for `str(h)` may call back into V2), so `map`, `filter`, `join`, `sort` with a `key` and `str()` of containers clone the element values out first.

---

## 6. Program execution and modules

### 6.1 Startup and the run

1. **Startup** (1.3): parse argv (19.1) and load the config (ch. 18); a usage or config error, or a missing or unreadable script, prints a message and exits 2. On Windows the cwd is first dropped from the DLL search (16.2). The interpreter thread runs steps 2 and 3.
2. **Syntax:** the entry file and, transitively, every file an `import "…"` literal names are lexed, parsed and checked, each file once, depth-first in source order (ch. 3, 4; the parser stops at a file's first error, 4.5). A `SyntaxError` in the **entry** file prints as `file:line:col: SyntaxError: msg` with an excerpt (2.4) and exits 1 before anything runs or any engine is resolved. The diagnostics of an **imported** file are stored with that file for its `import` (6.4). Tags are not checked here (syntax never reads config, 0.1): an unknown tag raises `EngineUnknown` when its block is reached (7.3). `v2 check` stops after this step and prints the diagnostics and a summary (19.3).
3. **Run:** build the entry file's `FileScope` (6.2), hoist its top-level `func`s, execute its statements in order, then end the run (6.5).

### 6.2 FileScope

```rust
pub struct FileScope {                        // Interp.files, by FileId
    frame: Env,                               // module frame; parent: the builtins frame
    blocks: Vec<Rc<EngineBlock>>,             // the definition blocks, in source order (13.2, 13.3)
    top: HashSet<Rc<str>>,                    // top-level names, for `v2` (13.7)
    waiting: HashSet<UnitId>, eligible: HashSet<u32>,  // what may wait: 12.7, 13.4 (`@import`s by span start)
}
```

A file's `FileScope` is built when its top level starts and kept for the run (5.1, 13.6). Reaching a block runs `exec_block`: 12.1 defines what "reached" means and the lifecycle of definition and action blocks.

- A V2-level `@import` runs when reached, or waits (12.7), and can bind only blocks reached by then (13.3, 13.4); an `@import` inside a block resolves when called (13.6, 13.7), so blocks may import functions from each other in any order.
- Blocks, labels and selectors belong to one `FileScope`: a block in `util.v2` never makes `@import … from @py` in `main.v2` ambiguous, and `from v2` inside `util.v2` means `util.v2`.

### 6.3 Blocks as statements and expressions

An action block is an expression; as a statement its value is discarded, and the engine is told so. Definition blocks are statements only (4.5).

| Engine kind | Expression position | Statement position |
| --- | --- | --- |
| py, js, ts, custom bridged | py: the last statement's value if it is an expression statement, else `null`; js, ts: the script's completion value (what `eval` would return; `undefined` → `null`; 12.2, DOCS 4.5); custom bridged: the block's last expression's value, or `null`, sent as the `value` of the adapter's reply to the action's `load` (12.2). Each is awaited if awaitable (9.8), marshaled per ch. 10 | `want_value: false`: the value is computed (and awaited) but never encoded, so no handle is created (12.2) |
| compiled action program, `run` engine | stdout piped and collected; one trailing `\n` or `\r\n` stripped; decoded as UTF-8 leniently (invalid bytes → U+FFFD), like the stderr tail (17.3) | the host flushes `Out` and dirty native stdio (16.7), and the child inherits stdout, so its output streams in program order |

Compiled programs and `run` engines, in both positions: the working directory and environment are the host's (a `run` engine's environment also gets 17.4's args); stdin is the null device; stderr streams live and its last 4 KiB are kept; a non-zero exit raises `EngineExitError` whose `data.stderr` holds that tail, not repeated in the message (17.3); the block's whole process tree is killed when the block finishes or times out (`call_timeout`, 8.5) or V2 exits, so nothing it started outlives it.

Blocks inside larger expressions keep left-to-right order: in `f(@py { 1 }, g())` the block runs before `g`.

### 6.4 V2 modules

`import "PATH" as name` may appear in any body and declares `name` in the current frame:

1. **Resolve** `PATH` as written (no extension added) against the **importing file's** folder (absolute paths as is), then `fs::canonicalize` (`span::canonicalize`, also the loader's and so FILE's: on Windows, where that fails though the file opens, as for a name on WSL's share that holds a stand-in, `mod\u{f03a}x.v2`, which Windows can't normalize, the path the file was opened by, `GetFinalPathNameByHandleW`'s `FILE_NAME_OPENED`). On Windows a PATH with a part (not `.` or `..`) that ends in a dot or a space, which Windows drops (`./util.v2.` would load `util.v2`), is refused first, as 5.10 refuses it (`builtins::dotted_name`): `ImportError: cannot import './util.v2.': a Windows file name can't end in a dot or a space ('util.v2.' would be 'util.v2')`, plus ` — did you mean './util.v2'?` when Windows' reading of it is there (`module::without_dots`), or, when a file of that name is there (WSL and Macs make them), `hint: one of that name is there (WSL and Macs can make one): reach it by its full path with \\?\ in front, as in import "\\\\?\\C:\\proj\\lib.\\x.v2" as x` (a folder on the way: such a path is kept with its `\\?\`, 2.1), or for the file itself, whose name doesn't end in `.v2`, `hint: one of that name is there (WSL and Macs can make one), but a module's name ends in .v2: rename it` (`module::dotted_import`). Missing → `ImportError: cannot find module './util.v2' (looked for C:\proj\util.v2)` (paths shown per 2.1, with `..` resolved and the existing part canonicalized), plus ` — did you mean './util.v2'?` when PATH doesn't end in `.v2` and PATH`.v2` is a file (this hint and those below take PATH without the `/` it ends with, on Windows `\` too: `./util/` gets `./util.v2`, `module::trim_separators`). PATH that is a `.v2` file with a separator after it (`./util.v2/`, which Windows 11 and macOS find, and Linux and Windows Server don't) gets ` — did you mean './util.v2'?`. Else, for a relative PATH ending in `.v2`, ` — did you mean './Util.v2'?` (PATH with the name found) for a `.v2` file of its folder within 5.9's did-you-mean distance, the names compared without `.v2` as for 19.1's FILE, or one whose name differs only in its `.v2`'s case (`./lib/Util.V2` for `Util.v2`), plus ` (Linux matches the case of file names)` when only the case differs (` (this file system …)` elsewhere: `module::nearby_module`). On Windows, first, a PATH with `: * ? " < > |` whose name with their stand-ins is there (`module::stand_in_twin`: WSL's `q\u{f03f}.v2` for a Linux `q?.v2`) gets ` — did you mean './q\u{f03f}.v2'?`, and every did-you-mean that offers a name with a stand-in gets `hint: \u{f03f} is how Windows shows the '?' of the Linux name 'q?.v2': rename it in WSL` (`module::stand_in_note`). A name of that folder whose NFC form is that of PATH's last name (a file copied from a Mac, which writes names decomposed: `o` plus U+0308 for `ö`) comes first, as the hint `there is a 'größe.v2' whose name looks the same but is written in decomposed Unicode (letters plus combining accents, as macOS writes names): rename it` (`module::unicode_twin`, also 19.1's and 5.10's; a decomposed name given for a composed file: `…, but the name given is written in decomposed Unicode (…): type it again`), looked for only once the file is missing; so does PATH`.v2`'s name when PATH doesn't end in `.v2` (`./büyüt` for a Mac's `büyüt.v2`), the hint then ending with the import that works, PATH with its last name composed (NFC) plus `.v2`: `…: rename it, and import './büyüt.v2'` (`module::composed_v2`; `…: type it again, and import …` alike), and, for a folder on the way that isn't there, the first missing one's (`module::folder_twin`: `there is a 'größe' whose name looks the same …: rename it`). There but not to be followed (`symlink_metadata` finds it, `canonicalize` fails), as 19.1 says for FILE: a link to nothing is `ImportError: cannot import './d.v2': it is a link to C:\proj\gone.v2, which isn't there` (a relative target from the link's folder, 5.10's `link_target`), with `hint: remove the link (del d.v2), or make C:\proj\gone.v2` (`rm d.v2` elsewhere), one in a loop (`ELOOP`, Windows' `ERROR_CANT_RESOLVE_FILENAME`, 1921: `module::is_link_loop`) `cannot import './l.v2': it is a link in a loop`, with `hint: remove the link (del l.v2), or point it somewhere else` (the link as messages show its path, as below), a Linux symlink WSL made (5.10's, which Windows can't follow wherever it points; a folder on the way too) `cannot import './rel.v2': it is a Linux (WSL) symlink, which Windows can't follow`, with `hint: use the path it points to (ls -l in WSL shows it), or run V2 inside WSL`, anything else the OS's reason. Not there on a `\\wsl.localhost` or `\\wsl$` path with no WSL distribution of that name (5.10's `wsl_root`): `cannot import '//wsl.localhost/Ubuntu-22.04/m.v2': there is no WSL distribution named Ubuntu-22.04: wsl --list --quiet shows them`; a server with no distribution's name (`\\wsl.localhost`, `//wsl.localhost/`, `\\wsl$`), which `Path` reads as a folder at the top of the drive, is looked at as written, before it is resolved: `cannot import '\\wsl.localhost': \\wsl.localhost needs a WSL distribution's name after it, as in \\wsl.localhost\Ubuntu: wsl --list --quiet shows them`. Not even `symlink_metadata` finds it: a link on the way that leads nowhere or in a loop (a folder part; on Windows a junction too, the one link a non-administrator can make) is named, `cannot import './j/lib.v2': 'j' is a link to C:\proj\gone, which isn't there`, with 5.10's `hint: remove the link (rmdir j), or make C:\proj\gone` (the link as messages show its path; or `'j' is a link in a loop`, `hint: remove the link (rmdir j), or point it somewhere else`; `module::link_problem` looks at each part with `symlink_metadata` and follows each link with `metadata`), in a run and in `v2 check` (`locate` gives the message and the hint); a file in a folder V2 may not read (`canonicalize` or `symlink_metadata` fails with `PermissionDenied`, as when a deny is inherited from the folder) is the OS's reason, `cannot import './noread/lib.v2': Access is denied`; only the rest is "cannot find module". A pipe or a device (a FIFO, a link to `/dev/zero`; also `/dev/stdin` from a pipe, `/proc/self/fd/0` or a `<(…)`'s `/dev/fd/N`, links to an anonymous pipe that `canonicalize` can't follow and `metadata` can, checked before the link messages; on Windows one WSL made, seen through `\\wsl.localhost`, which Windows takes for a file: 5.10's `module::wsl_file`, asked only of a reparse point, `module::reparse_point`), whose read would never end, is `ImportError: cannot import './f.v2': it is a pipe or a device, not a file`, as 19.1 refuses such a FILE. `module::locate` makes these checks, so the startup parse (6.1, which skips the file), `v2 check` (19.3) and the `import` itself all refuse such a file and none reads it; its messages show control characters escaped (2.1). A file found with another extension (any case; a file with none is read as V2) isn't parsed: `ImportError: cannot import './data.csv': import loads .v2 files; read a data file with read_file(f"${script_dir()}/data.csv")` (an absolute path as it is: `read_file("C:/data/x.csv")`), or for `.py` `a Python file is imported with @import { name } from py.lib.helper`, for `.js`, `.mjs`, `.cjs` `a JavaScript file is imported with @import { name } from js."./helper.js"`, for `.ts`, `.mts`, `.cts` the same with `ts.` and TypeScript. A folder is `ImportError: cannot import './lib': it is a folder, and import loads a .v2 file, as in import "./lib/x.v2" as x` (its first `.v2` file by name and that file's name as the alias, or `FILE.v2` and `m`; Windows would say "Access is denied"). A Windows drive-relative path (`C:m.v2`) is neither relative to the file nor absolute: `ImportError: cannot find module 'C:m.v2' (a drive-relative path: write it relative to the file, or absolute)`. A file that did not exist at startup but exists now is parsed now. One that can't be read is `cannot import './m.v2': cannot read 'm.v2': <os message>`, with 19.1's hint for a file another program holds open. The written path's last names are compared with the canonical path's (`module::case_note`): when they differ only in case (a file system that ignores case, Windows' or macOS's, found the file), the import works, and stderr gets, once per import site in a run (in `v2 check` at the site, among its messages, not an error), `note: './REAL/Counter.V2' is spelled './real/counter.v2' on disk: a case-sensitive file system, as Linux's, won't find it` (the written path up to its first name, then the names on disk; a link among those parts, or an 8.3 short name, is no case problem).
2. **Look up** `Interp.modules` (by canonical path; a path not loaded yet whose `file_identity`, the volume serial number and 128-bit file ID on Windows (`FILE_ID_INFO`, as the 64-bit file index isn't unique on ReFS; an ID of all zeros or all ones is none), the device and inode elsewhere, is a loaded file's, while that file's own path still has it, is that file's path: `SourceMap::same_file`, as a share of a local folder, `\\localhost\C$\…`, and its `C:\…` canonicalize apart; before that, on Windows, a `\\server\share\…` path that is a loaded one's but for the case of its server and share, or with `wsl$` for `wsl.localhost` (`span::unc_key`: WSL's share gives no file IDs), is that file's path; a file system gives a deleted file's ID to a new one, ext4 its inode at once, so the ID alone would take a new file for an imported one that was deleted), whose `ModuleState` is `Running | Done(Value) | Failed(Rc<ErrorValue>)` (the entry file starts `Running`):
   - `Done(ns)` → declare `name = ns`. `Failed(e)` → throw `e` again: a module's top level never runs twice.
   - `Running` → `ImportError: circular import: a.v2 -> b.v2 -> a.v2` (the chain from the path's first entry in `import_stack`).
   - Absent, with stored diagnostics (6.1) → `Failed` with `ImportError: cannot import './util.v2': util.v2:3:9: SyntaxError: <message>` (the first diagnostic) at the `import`, `data` = all of them as `{file, line, col, message}`. Uncaught, it also prints that diagnostic's line and caret, after `  in util.v2:`, and its hint as the error's hint (2.4): what `v2 check` would show. Neither is part of `e.message`.
   - Absent otherwise → check the call depth and the stack guard as for a call (5.9), push onto `import_stack`, mark `Running`, push a nameless `CallRecord` for the import site, build a new `FileScope` whose module frame's parent is the builtins frame, hoist and run it.
3. **Success:** the namespace is a new `Dict` (type `"dict"`) of the module frame's bindings in layout order, minus names starting with `_`: values, `func`s, `@import`ed proxies and module handles, `extern` functions and `extern … as` dicts, imported module dicts. It is stored as `Done(ns)` and shared by every importer. It is a snapshot: rebinding a module-level `let` later is not visible through it. `util.slug(…)` works by dict-key dispatch (5.5).
4. **Failure:** `Failed(err)`; the error propagates unchanged with the import site in its trace.

Re-exported Proxies keep their `session` and `unit` and work from any file. Modules share the entry file's project root and config (18.1).

### 6.5 Ending the run: exit codes and shutdown

Each outcome's exit code is in 19.1's table.

**End of run.** A normal end, an uncaught error and `exit(code)` (`Flow::Exit`, 5.8) take one path. The interpreter thread then:
1. Flushes dirty native stdio and `Out`; an uncaught error is written to stderr (2.4).
2. Shuts the sessions down (8.7): `bye`, one shared 1 s grace period that keeps forwarding `out` frames, then `kill_tree` on survivors.
3. Kills and reaps, as trees, any child still running: `run` engines, compiled programs, compilers (8.3).
4. Flushes `Out` again (late `out` frames) and returns the exit code (19.1) to main.

Main removes the run dir (20.3) and exits with the code. Native libraries are never unloaded (16.2).

**Ctrl+Z** (Unix, SIGTSTP, unless ignored when v2 starts): the handler only sets `SUSPEND`; the watchdog then sends SIGSTOP to every child's process group (workers and `exec`'s children have their own), stops v2 with `raise(SIGSTOP)`, and when the job goes on (`fg`, `bg`, SIGCONT) sends them SIGCONT, so the job stops and goes on as a whole. While `input(echo: false)` waits (5.10), `builtins::echo_around_stop` wraps the stop, holding `ECHO_SAVED`'s lock so the line can't end in between: before it, when v2 is the terminal's foreground job, the terminal gets back the settings it had before echo went off (dash leaves the terminal as it finds it, so its prompt would show nothing typed); after it, in the background (`bg`), `tcdrain(0)` stops v2 with SIGTTOU until `fg` (its read would stop it with SIGTTIN, and bash's settings would then show the password), the children still stopped; once v2 is the foreground job, echo goes off again (bash has put its own settings back meanwhile). A `kill -STOP` of v2 alone can't be seen, so its children run on.

**Interrupts** (Ctrl+C, Ctrl+Break, console close, SIGINT, SIGTERM, SIGHUP, SIGQUIT) are handled by the host alone; 8.3's child rules decide which of them reach a child. The interrupt handler sets `INTERRUPTED` (an `AtomicBool`); the watchdog does the rest. Unix: a `sigaction` handler for SIGINT, SIGTERM and SIGHUP that does nothing else (only async-signal-safe work); a signal already ignored when v2 starts (`nohup`, background jobs of a shell without job control) stays ignored. If the watchdog thread cannot be started, no handler is installed, so the OS default still ends the process. Windows: the `SetConsoleCtrlHandler` routine sets `INTERRUPTED`, wakes the watchdog (8.5), then blocks until the process ends, because returning early would let `CTRL_CLOSE_EVENT` terminate the process before cleanup. The watchdog (1.3) checks `INTERRUPTED` on every wake (8.5). Once it is set, the watchdog `kill_tree`s every registered child, workers included (8.5), writes what `Out` holds straight to the descriptor (from a thread of its own, at most 300 ms, so a reader that stopped reading can't hold the exit), waits a bounded time for those trees to end, gives the terminal back (`restore_terminal`, below: a block stopped after it turned echo off, or while a live display hid the cursor, never ran its own cleanup, and neither did `input(echo: false)`), on Windows the console's echo when Ctrl+C came during `input(echo: false)` (`builtins::echo_back`, 5.10), removes `<run>` (20.3) and ends the process with `_exit(130)` / `ExitProcess(130)`: no `bye`, `catch` or `finally`. The interpreter thread never handles an interrupt: once `INTERRUPTED` is set it prints no diagnostic (neither the EOF of a killed worker as `EngineCrashed` nor the failure of a per-use child ended by the same event), spawns no process and runs no `catch` or `finally`; where it would, it stops (`process::stop_if_interrupted`) and the watchdog ends the process. It also stops right after `exec` and each request to a worker return, and before each file builtin, as a signal can land while they run and the watchdog acts only at its next wake: `exec(["sh", "-c", "kill -TERM $PPID"])`, or a block's `os.kill(os.getppid(), signal.SIGTERM)`, would otherwise let the next statements (a `write_file`) run first. For those checks to see such a signal, the handler must have run by then: on Unix main blocks the four signals before it starts any thread, the interpreter thread unblocks them for itself, and helper threads start with them blocked (`process::spawn_helper`; `take_interrupts`), so the kernel runs the handler on the interpreter thread, ending its wait, before the call returns (on another thread it could run later: 3 runs in 40 under load printed the next line). Main unblocks them again once the interpreter thread has ended. Children start with an empty mask (Rust's `Command` resets it). A run that ends between the interrupt and the watchdog's next wake is caught by main, which checks `INTERRUPTED` after joining the interpreter thread and takes the same path. The terminal: on Unix `Watchdog::start` saves the settings (`tcgetattr`) of each of stdin, stdout and stderr that is a terminal (`remember_terminal`); the interrupt path (`restore_terminal`) sets them again (`tcsetattr`, `TCSANOW`, on a terminal whose foreground process group is v2's only: a background v2 would change the settings of the shell reading it now; SIGTTOU ignored) and, on every OS, shows the cursor (`show_cursor`: `ESC[?25h` once to each terminal among stdout and stderr, on Unix one whose foreground process group is v2's, on Windows a console with VT processing on, before its mode is put back; from a thread of its own for at most 100 ms, as a terminal whose output is stopped, Ctrl+S or a console's selection, blocks the write). It does so only when v2's stdout is a terminal, as v2 is then what the screen shows (in `v2 x.v2 | less` Ctrl+C reaches the pager too, which still runs and has the terminal in a mode of its own), or when `TERMINAL_CHANGED` is set (v2 itself changed the terminal's settings and hasn't put them back yet: input read with echo off). The other ends that aren't the normal one, `exit(n)`, an uncaught error and a closed pipe, show the cursor too (`cursor_back`: a block's live display may never have ended, where plain Python's `sys.exit` would have run its cleanup), when v2's stdout is a terminal and a worker was told it has one (`TOLD_TERMINAL`, set by the handshake, 9.3). Windows: the console modes `console_vt_on` changed at startup (9.9) are put back on every exit, this one, a closed pipe's and the end of the run (`restore_console`, from `hard_exit` and main's exit), each only while it is still the mode v2 set; a console whose VT processing was on already (another v2 on it, `v2 a.v2 | v2 b.v2`) isn't recorded, so the later run doesn't put back the earlier one's mode; the earlier one, ending first, still turns it off under the later.

**A closed pipe.** When stdout's or stderr's reader goes away (`v2 x.v2 | head`, `v2 x.v2 2>&1 | head`: a write of `Out`, of `print(stderr: true)` or of a block's stderr text, `builtins::write_stderr`, fails with `BrokenPipe`, which on Windows includes "the pipe is being closed"), nothing more can be shown: the run stops at once and quietly, the way a shell's SIGPIPE stops a command. Every child is killed as a tree, the run dir removed, and the exit code is 141; no `catch` or `finally` runs and nothing is printed (`process::exit_broken_pipe`). Found only at the final flush, it ends the run as usual with nothing said, and 0 becomes 141.

**Closed streams.** Diagnostics, check's summary, `--help` and `--version` are written with writes whose errors are ignored, so a closed stdout or stderr never turns an exit path into a panic or an abort: the exit code is the one the run would have had. Program output is different: any other failed write (a full disk, a device error) is 5.10's `IOError` at the `print`, and what it couldn't write goes with it (a program that catches it has handled that output). A loss no `IOError` told the program about (a block's own error won over it, 9.9, or late output at the end) is said at the end like a failed final flush: when `Out`'s final flush (step 1 above) fails, `v2: cannot write to stdout: <os message>` goes to stderr and an exit code of 0 becomes 1 (any other code is kept), so lost output never looks like success.

**Panics.** A panic in the interpreter thread prints `v2: internal error: <panic message>`; unwinding drops the sessions (killing their workers), main removes the run dir and exits 101. Host functions called from native code wrap their bodies in `catch_unwind` (15.9). If the host dies with no path at all (SIGKILL, a native crash), 8.3's child rules still end its children: on Windows their whole trees, on Linux the direct children only, since PDEATHSIG doesn't reach what they started; the run dir is left behind, owner-only (20.3).

---

## 7. Engine registry and resolution

Code: `engines/spec.rs` (specs, the built-in table), `engines/registry.rs` (tags, the resolution memo, `engine_available`), `engines/resolve.rs` (discovery, probes, `toolchains.json`).

### 7.1 Data model

```rust
pub enum EngineKind { Bridged, Compiled, Run }
pub struct EngineSpec {                                    // py: "py", "Python", ["python"], Bridged
    pub id: Rc<str>, pub language: Rc<str>, pub aliases: Vec<Rc<str>>, pub kind: EngineKind,
    pub session_of: Option<Rc<str>>,                       // Some("js") for ts: no process of its own
    pub candidates: Vec<Vec<Rc<str>>>, pub fixed_args: Vec<Rc<str>>,  // argv prefixes tried in order; appended args ({adapter}, {file})
    pub ext: Option<Rc<str>>,                              // built-in run engines' temp-file extension (7.2, 17.3)
    pub env: Vec<(Rc<str>, Rc<str>)>, pub min_version: Option<Version>,
    pub probe: Option<(Vec<Rc<str>>, VersionPattern)>,     // probe args and how to read the version (7.2, 7.7)
    pub adapter: Option<(&'static str, &'static str)>,     // (file name, include_str! source), 14.1
    pub custom: Option<Rc<CustomFields>>,                  // custom engines: 17.1's validated fields
}
pub enum Origin { Path, Vswhere, Venv, Git, Config(ConfigSource) }   // doctor's FROM column (19.2); Git: Windows bash (7.6); ConfigSource: 18.3
pub struct ResolvedEngine {                                // ts: js's argv, version and origin under the ts spec (7.6)
    pub spec: Rc<EngineSpec>, pub origin: Origin,
    pub argv: Vec<OsString>,                               // [C:\Windows\py.exe, -3]: the program, absolute, then its prefix args
    pub version: Option<Version>,                          // None: custom engine, cmd
    pub skipped: Vec<(PathBuf, String)>,                   // "Microsoft Store placeholder", "too old: 3.9.6 < 3.10"
    pub target: Option<String>, pub env: Vec<(OsString, OsString)>,  // compiled (7.6): cl's x64|arm64 or rustc's triple; cl's vcvars env
}   // shown(): doctor's COMMAND (19.2); ts: "(js engine, built-in type stripping)"; rust: the rustup proxy (7.6)
```

Scanning profiles are not part of `EngineSpec`: the lexer takes them from `syntax/scan`'s static table (3.8), and a test keeps that table in step with 7.2's.

`Registry` holds the specs (built-ins, then the custom engines of `v2.toml` in declaration order), the config, the probe cache and `resolved: RefCell<HashMap<Rc<str>, Result<Rc<ResolvedEngine>, Rc<NotFound>>>>` (`NotFound`: the `EngineNotFound` message plus doctor's STATUS, COMMAND and FROM). It is built from the config (ch. 18) at startup and does not change during the run; the lexer never consults it (3.8). Resolution is lazy (the first block, selector, `engine_available` or doctor that needs it) and memoized per id, failures included.

### 7.2 Built-in engines

| id | aliases | kind / profile | candidates, in order | fixed args | env | minimum | probe |
|---|---|---|---|---|---|---|---|
| `py` | `python` | Bridged / Python | `python3`, `python`, Windows: `py -3` | `-B -u {adapter}` (`-B`: no `__pycache__` in the project; `-u`: unbuffered) | `PYTHONUTF8=1`, `PYTHONIOENCODING=utf-8` | 3.10 | `--version`: `Python (\d+)\.(\d+)\.(\d+)` |
| `js` | `javascript` | Bridged / CLike(Js) | `node` | `--disable-warning=MODULE_TYPELESS_PACKAGE_JSON {adapter}` (a `.ts` file that imports another, under a `package.json` without `"type"`, makes Node advise `"type": "module"`, which would break the CommonJS files there); before them, for a Node before 22.18 on the 22 line or 23.6 on the 23 line, `--experimental-strip-types`, so it loads the `.ts`, `.mts` and `.cts` files an `@import` names, as later ones do by default (their syntax errors come as 12.5's snippet, read the same way), and `--disable-warning=ExperimentalWarning`, since such a Node warns about the flag at startup, before the adapter could drop that one warning | | 22.13, not 23.0 or 23.1 (7.6) | `--version`: `^v(\d+)\.(\d+)\.(\d+)` |
| `ts` | `typescript` | Bridged / CLike(Js), session `js` | (js) | | | (js) | js's probe |
| `c` | | Compiled / CLike(C) | Windows: `cl`; else `cc`, `gcc`, `clang` | | | gcc 7, clang 5 (full C++17), cl 19.28 (VS 2019 16.8, first with `/std:c11`) | 7.7 |
| `cpp` | `cxx` | Compiled / CLike(Cpp) | Windows: `cl`; else `c++`, `g++`, `clang++` | | | as `c` | 7.7 |
| `rust` | `rs` | Compiled / CLike(Rust) | `rustc` | | | 1.56 (edition 2021) | `-vV` (7.6) |
| `bash` | `sh` | Run / Plain | `bash`; Windows: Git Bash only (7.6) | `{file}` | | | `--version` |
| `pwsh` | `powershell` | Run / Plain | `pwsh`; Windows: then `powershell` (7.6) | `-NoProfile -File {file}`; Windows: `-NoProfile -ExecutionPolicy Bypass -File {file}` | | | `--version` (7.6) |
| `cmd` | | Run / Plain | Windows: `cmd`; elsewhere none (7.6) | `/d /q /c {file}` | | | none |
| `lua` | | Run / Plain | `lua`, `lua5.4` | `{file}` | | | `-v` |
| `ruby` | | Run / Plain | `ruby` | `{file}` | | | `--version` |
| `go` | | Run / Plain | `go` | `run {file}` | | | `version` |
| `java` | | Run / Plain | `java` | `{file}` (single-file source launch) | | 11 | `--version` |
| `zig` | | Run / Plain | `zig` | `run {file}` | | | `version` |
| `cs` | `csharp` | Run / Plain | `dotnet` | `run {file}` (file-based apps) | | 10.0 (the SDK) | `--version` |

The Run rows are the built-in run engines (17.3, 17.4; configuration 18.2), with the temp-file extensions `sh`, `ps1`, `bat` (CRLF), `lua`, `rb`, `go`, `java`, `zig` and `cs`.

### 7.3 Tags, EngineUnknown and `engine_available`

- **Canonical tags.** `canonical(tag)` maps an id or alias to `spec.id` everywhere: block headers (`@python stats` is a `py` block), selectors (`@python.stats` = `@py.stats`, `python.os.path` = `py.os.path`), `engine_available` and `[engines.<tag>]`; `V2_ENGINE_<TAG>` takes canonical tags only (18.4). Messages print the canonical form (2.5).
- **Profiles are static.** The lexer takes a tag's scanning profile from the fixed built-in table; every other tag, declared or not, is Plain (3.8). The parser never rejects a tag as unknown.
- **Later tags.** Until their milestone registers them, the tags of DOCS 5 that this build doesn't have (`c`, `cpp`/`cxx`, `rust`/`rs` in Phase 2; the built-in run engines' tags and aliases in Phase 3) raise `EngineUnknown: unknown engine '@c': C blocks aren't in this version of V2 yet — known engines: py, js, ts`, checked before did-you-mean (so `@rs` isn't taken for a typo of `@ts`). Each row goes when its engine joins the registry (`registry.rs` `later_language`).
- **EngineUnknown.** `resolve_tag(tag, span)` runs when a block is reached or a selector is bound. A miss raises `EngineUnknown: unknown engine '@pyhton' (did you mean '@py'?)` (did-you-mean per 5.9, over ids and aliases); with no close match, `unknown engine '@perl' — known engines: py, js, ts, c, cpp, rust, bash, pwsh, cmd, lua, ruby, go, java, zig, cs; declare it in [engines.perl] in v2.toml` (the list is the registry's engines); the text ends `in v2.toml, or with define_engine` (17.2). Until custom engines are built (Phase 3) it ends `(custom engines aren't in this version of V2 yet)` instead, and `define_engine` is `Error: define_engine (custom engines) isn't in this version of V2 yet`, as `extern` is. Custom tags have 17.1's lowercase shape, so a tag the lexer accepts outside it (`@PY`) is simply unknown.
- **`engine_available(tag)`:** `false` for an unknown tag (no error); otherwise `true` iff resolution succeeds (7.4, 7.6). It may run probes (cached, 7.7) but never starts a session or compiles, and a later crash (8.2) does not change its answer.

### 7.4 Custom engines in the registry

- Custom engines come from `[engines.<tag>]` tables in `v2.toml` and from `define_engine` calls: spec, validation and tag rules 17.1, registration 17.2, `V2_ENGINE_<TAG>` 18.4.
- Resolving one only finds `command[0]` (7.5) and runs nothing (`version` is `None`; doctor VERSION `-`). A miss is `EngineNotFound: 'perl' not found on PATH (engine declared in C:\proj\v2.toml)`, or `(engine declared at main.v2:3)` for a `define_engine` call.

### 7.5 Finding an executable

Children are always spawned by absolute path (Windows `Command` would search the application and system folders before `PATH`). On Unix `exec`'s program gets its name as written as `argv[0]` (`CommandExt::arg0`, `process::run_captured_as`), as a shell passes it, so its messages start `ls:`, not `/usr/bin/ls:`; on Windows the command line starts with the path. `find_program(name, base)`, also used for `exec`'s `argv[0]` with the cwd as `base` (5.10):
1. `name` containing `/` or `\`: `base.join(name)` (`base` per 18.1); on Windows an extensionless name is also tried with each accepted extension. No `access(X_OK)` here: a named file that cannot run fails its probe with the OS message (`is not usable: could not start …: Permission denied`) instead of looking missing.
2. Otherwise each `std::env::split_paths(PATH)` entry in order, skipping empty and relative entries such as `.` (the cwd is never searched, so a cloned project cannot shadow `python`). One that starts with `~` (`~/.nvm/versions/node/v24/bin`, from a quoted `export PATH="~/…:$PATH"`) is relative too, and skipped, as execvp, dash and Python skip it; only bash's own lookup expands it, so the shell may run another program than V2. So on Unix an engine's search (7.6) lists such an entry whose folder, `~` expanded (`$HOME`; `~ann`, that user's home), holds a program it looks for among its skipped hits, which EngineNotFound (7.8) and doctor's notes (19.2) show: `skipped: ~/.nvm/versions/node/v24/bin/node (its PATH entry starts with ~, which only bash expands: write $HOME/.nvm/versions/node/v24/bin in your startup file)` (`write /home/ann/bin` for `~ann/bin`; `resolve::tilde_skips`). Only one before the folder of the program it uses (after it, bash runs that one too), or any when it found none, and not one whose file it met through another entry. Windows: `dir\name` if it already has an accepted extension, then `dir\name{ext}` per `PATHEXT` entry (unset: `.COM;.EXE;.BAT;.CMD`; case-insensitive), accepting only `.com`, `.exe`, `.bat`, `.cmd` (`CreateProcess` cannot start `.py` or `.ps1`). Unix: `dir/name` passing `access(X_OK)`.
3. The hit must be a regular file (`symlink_metadata` when `fs::metadata` fails, as for `WindowsApps` aliases) and stays absolute, not canonicalized. `.bat`/`.cmd` hits (which cmd.exe runs, so an engine's search passes over them while the working folder is a `\\server\share`, 7.6) are spawned directly (Rust ≥ 1.77.2 quotes arguments for `cmd.exe`, kept for them instead of 8.3's Windows command line; its `InvalidInput` for an unquotable argument is a spawn failure, 8.3); one whose path contains `%` is a spawn failure too (`a batch file whose path contains '%' can't be started safely: move it to a folder whose path has no %, or start the program it runs directly`), because `cmd.exe` would expand it. So is an argument with a line break (Rust's own refusal, "batch file arguments are invalid", is worded `a batch file can't take an argument with a line break: pass the text as input, as in exec(argv, input: text)`), as `cmd.exe` would end the command line there.

A Windows drive-relative name (`C:tool`, relative to that drive's own current folder) is never found; `exec` says `IOError: exec: cannot start 'C:tool': a drive-relative path: write it relative to the working directory, or absolute`.

cmd.exe can't work in a `\\server\share` folder (a project in WSL, `\\wsl.localhost\Ubuntu\…`; also as `\\?\UNC\…`): it says `UNC paths are not supported` on its stderr, works in the Windows folder instead and exits 0. So on Windows, when V2's working folder is one, `exec` raises before starting `cmd.exe` (but for a `/c` or `/k` command line that starts with `pushd `, `cd /d ` or `chdir /d ` to an absolute folder, `X:\`, `X:/` or `\\…`, quoted or not, which moves first; a relative one, `cd /d logs`, is looked for in the Windows folder) or a `.bat` or `.cmd` file (`npm.cmd`), which cmd.exe runs: `IOError: exec: cmd.exe can't work in a \\server\share folder and would run in C:\Windows: run the program itself, or exec(["cmd", "/c", "pushd \"\\\\wsl.localhost\\Ubuntu\\home\\ann\\p\" && …"])` (`exec: npm.cmd runs in cmd.exe, which can't work in …`; the Windows folder is `SystemRoot`'s; the folder in a V2 string, as `pushd` takes it, which maps a drive letter to the share; `builtins::cmd_in_share`). Other programs start there as anywhere.

### 7.6 Per-engine rules

**Order.** The configured command or `[python] venv` (whichever 18.3 picks), else the candidates. A configured value never falls back to auto-detection: a missing one is `EngineNotFound: engine 'py' command 'C:/Python312/python.exe' not found (set by C:\proj\v2.toml)` (a path holding a variable a shell would expand, as PowerShell's single quotes and a `REG_SZ` variable keep it, adds `; V2 doesn't expand %ProgramFiles%: write the path out`; a `V2_ENGINE_<TAG>` value that is `~` or starts with `~/` or `~\`, which a shell keeps in quotes and 18.4 joins to the cwd, adds `; V2 doesn't expand ~ in V2_ENGINE_PY: write the path out, as in V2_ENGINE_PY="$HOME/bin/python3"`, the value's own path after `$HOME`, which a shell expands in double quotes (on Windows `$env:V2_ENGINE_PY = "$env:USERPROFILE\bin\python3"`: `resolve::env_tilde`), and a bare `~` (or `~/`, `~\`, the home folder, not a program) only `; V2 doesn't expand ~ in V2_ENGINE_PY: write the path out`; one from `v2.toml` holding a control character, which a `\n`, `\t`, `\b`, `\f` or `\r` in `"…"` makes, as in `"C:/Program Files/nodejs\node.exe"`, adds 18.5's `; in "…", a backslash starts an escape (\n is a line break, \t a tab): write the path with / or in single quotes, as in command = ['C:\Program Files\nodejs\node.exe']` instead (`config::configured_escape_hint`, which looks only at what `v2.toml` wrote, not at the folder a relative path is joined to)), and a failing probe is an error too (doctor `broken`; too old: `old`): `engine 'py' command 'C:\x\python.exe' (set by C:\proj\v2.toml) is not usable: version probe failed (exit code 1)`. "Set by" names the file's full path or the `V2_ENGINE_<TAG>` variable. On Windows a configured program that is there but that Windows can't start (7.5 finds no `.exe`, `.com`, `.bat` or `.cmd`) is never called missing: a Linux one (an ELF file, a WSL link Windows can't follow, also one on a drive, made under `/mnt/c`, which Windows can't even stat, `ERROR_CANT_ACCESS_FILE`, and its folder lists as a link; any file on `\\wsl.localhost` or `\\wsl$`) is `engine 'py' command '\\wsl.localhost\Ubuntu\usr\bin\python3.12' (set by …) isn't a Windows program (a Linux one): the Windows v2 runs Windows programs: name a python.exe, or run the Linux v2 inside WSL` (`node.exe` for js), another `… isn't a Windows program (an .exe, .com, .bat or .cmd file): name a python.exe` (`resolve::not_windows_program`); a path that mixes `/` and `\`, as `/usr/bin/python3` or `../p/.venv/bin/python` joined to the folder of a `v2.toml` on a share (18.1), is shown in these messages and in doctor with `\` only (`one_separator`). A configured program that is a `.bat` or `.cmd` (pyenv-win's shim, `conda.bat`) runs in cmd.exe, which in a `\\server\share` working folder would start the worker in the Windows folder (7.5): there it is refused before any probe, `engine 'py' command 'C:\Users\ann\.pyenv\pyenv-win\shims\python.bat' (set by …) runs in cmd.exe, which can't work in a \\server\share folder and would run in C:\Windows: name the program itself by its full path, as in V2_ENGINE_PY=C:\…\python.exe (for pyenv-win, pyenv which python shows it), or run v2 from a folder on a drive` (doctor `broken`; for js `as in V2_ENGINE_JS=C:\…\node.exe`; a bare `python.exe` would be looked for on `PATH`, which may have only the shim; `resolve::program_itself`). A command that runs `wsl.exe`, or WSL's `bash.exe` (in `System32`, not Git for Windows'), answers `--version`, but its Linux interpreter can't open the worker's adapter, a Windows path: doctor's notes say `py: wsl.exe runs a Linux python, which the Windows v2 can't use, as its adapter is a Windows path: name a python.exe, or run the Linux v2 inside WSL` (`a Linux node`, `node.exe` for js; 19.2), and a crash at startup ends with it (8.4; `resolve::linux_interpreter_note`). Skip reasons: `Microsoft Store placeholder`, `a py launcher with no Python installed` (the `py` launcher's `No suitable Python runtime found`), `too old: 3.9.6 < 3.10`, `version probe failed (exit code 1)` (then `: ` and the first non-empty line the program printed, as `: node: error while loading shared libraries: libatomic.so.1: …`; past 160 characters it loses its middle, marked `…`, so the end of a path in it, where the folder that matters is, stays: `resolve::clip_middle`), `version probe timed out (10 s)`, `no Node.js version in its --version output (it printed: Python 3.12.1)` (the first non-empty line, past 80 characters cut in its middle the same way: a `V2_ENGINE_JS` naming Python), `targets x86 but V2 is x86_64`, 8.3's `could not start '…': …`, or, on Windows while V2's working folder is a `\\server\share`, `runs in cmd.exe, which can't work in a \\server\share folder` for a `.bat` or `.cmd` hit (pyenv-win's `shims\python.bat`), which is passed over unprobed, like the placeholder, and so never cached (the verdict depends on the working folder), but counts as one that doesn't run (7.8; from a drive folder it is judged as any hit). Each auto-detected hit is probed (7.7); a hit whose probe fails, times out or matches no version, that is below the minimum, or that has the wrong target is skipped (kept in `skipped`) and the search goes on. Candidates without arguments are searched together, folder by folder: each `PATH` folder is tried with every such name, in the table's order, before the next folder, so an activated venv's `python` comes before a system `python3`; candidates with arguments (`py -3`) come after them all. Each hit is judged as it is found and the search stops at the first usable one, so later folders are never looked in (WSL's PATH holds dozens of slow Windows folders). A hit that is the same file as an earlier one (both canonicalized; a path that can't be, as itself) with the same prefix arguments is passed over unprobed and unlisted: on most Linux systems `/bin` is a link to `/usr/bin`, and `python` may be a link to `python3`. Probes of `PATH` hits always pass arguments: an App Execution Alias run without arguments opens the Microsoft Store.

- **py.** venv: `<venv>\Scripts\python.exe` on Windows, `<venv>/bin/python` then `bin/python3` elsewhere, used only when the folder holds a `pyvenv.cfg` (a conda environment and a Python installation have a `bin/python` too, and get the messages below on every system); missing: `EngineNotFound: [python] venv 'C:\proj\.venv' has no Scripts\python.exe (create it: py -m venv C:\proj\.venv)` (elsewhere `python3 -m venv`; `py` and `python3` only when `PATH` has them, else `(create it with the Python you want: its python.exe -m venv C:\proj\.venv)`, `its python3` elsewhere, as in every message below that makes a venv, where one made again uses the Python its `pyvenv.cfg`'s `home` names while that is there, `make the venv again, & "C:\Py 3.12\python.exe" -m venv --clear C:\proj\.venv, …` (`home\python.exe` on Windows; elsewhere the first there of `python3`, `python3.X` of the venv's version and `python`; `& ` on Windows before a quoted path, which PowerShell runs only after it; never Python's embeddable package, below, which has no venv module; for a venv virtualenv made, a `virtualenv` line in its `pyvenv.cfg`, `& "C:\Tools\py312embed\python.exe" -m virtualenv --clear C:\proj\.venv` while that Python has virtualenv, in `<home>\Lib\site-packages`, elsewhere `<home>/../lib/python3*/site-packages` or `dist-packages`; `resolve::venv_maker`, `with_maker`); a path with a space is quoted, so the command can be pasted, and control characters show escaped, as every path in these messages does, 2.1); a venv from `v2.toml` holding a control character (`venv = "C:/work\new"`) gets no command, which would make that name, but 18.5's hint: `[python] venv 'C:\work\new' has no Scripts\python.exe: in "…", a backslash starts an escape (\n is a line break, \t a tab): write the path with / or in single quotes, as in venv = 'C:\proj\.venv'`; when a folder next to it holds a `pyvenv.cfg`: `… has no Scripts\python.exe; there is a virtual environment in C:\proj\.venv: did you mean venv = ".venv"?` (the folder as `v2.toml`'s folder sees it). A venv that names a file: `[python] venv 'C:\proj\.venv\Scripts\python.exe' is a file: venv names the virtual environment's folder: write venv = ".venv"` (when the file's grandparent holds a `pyvenv.cfg`; else `… is a file: venv names a virtual environment's folder; for a Python that isn't in one, write [engines.py] command = ['C:\Python311\python.exe'] instead of venv`, when the file's name starts with `py`; another file, `venv = "v2.toml"`, is `… is a file: venv names a virtual environment's folder, as in venv = ".venv"`). `-m venv` is never suggested into a folder that holds anything, since over a Python installation it would make a venv of another Python there: a folder with a Python and no `pyvenv.cfg` (Windows: `python.exe`; elsewhere `bin/python3` or `bin/python`, with no `bin/activate`) is `[python] venv 'C:\Program Files\Python311' is a Python installation, not a virtual environment: name its python instead, [engines.py] command = ['C:\Program Files\Python311\python.exe'], or make a virtual environment with it: & "C:\Program Files\Python311\python.exe" -m venv .venv` (for Python's embeddable package, which has no venv module, below, `-m virtualenv .venv` when its `Lib\site-packages` has virtualenv, else no such part), or, with a `conda-meta` folder, `… is a conda environment, not a virtual environment: run v2 where it is activated (conda activate sales) and leave venv out, or name its python instead: [engines.py] command = ['…\envs\sales\python.exe']` (`conda activate base` for the base environment, the folder with `condabin`); its `bin` or `Scripts` folder, `… is a folder of a Python installation, …` (`/usr/bin`: `[engines.py] command = ["/usr/bin/python3"]`); a venv the other system made, which that system may still use (one project folder used from Windows and from WSL: on Windows a `bin/python` or `bin/python3` and no `bin/python.exe`, which MSYS2's has, or a `pyvenv.cfg` whose `home` is a POSIX path; elsewhere a `Scripts/python.exe`, or a `home` with a drive letter or `\\`), is kept, never made again with `--clear` (on a `\\wsl.localhost` path that fails halfway, leaving neither system a venv): `[python] venv '\\wsl.localhost\Ubuntu\home\ann\p\.venv' was made on Linux or macOS: keep it, make one for this system beside it, py -m venv \\wsl.localhost\Ubuntu\home\ann\p\.venv-win, and name its python in V2_ENGINE_PY, which wins over [python] venv ($env:V2_ENGINE_PY = "\\wsl.localhost\Ubuntu\home\ann\p\.venv-win\Scripts\python.exe"), or run V2 on that system` (elsewhere `was made on Windows`, `python3 -m venv /mnt/c/p/.venv-linux` (`.venv-macos` on macOS) and `(export V2_ENGINE_PY=/mnt/c/p/.venv-linux/bin/python)`; `resolve::foreign_venv`); another venv (a `pyvenv.cfg`) without the python this system needs, `… has no Scripts\python.exe, so it was made on another system or is damaged: make it again here, py -m venv --clear C:\proj\.venv`; a venv that lost its `pyvenv.cfg` (Windows: a `Scripts\python.exe`, a launcher that can't run without it, `No pyvenv.cfg file`, and no `python.exe` beside `Scripts`; elsewhere a `bin/activate`; never a folder with `conda-meta`, as conda's base has a `bin/activate` too), `… has no pyvenv.cfg, so it is damaged: make it again, py -m venv --clear C:\proj\.venv`, and for its `Scripts` (`bin`) folder or its python named instead, `[python] venv 'C:\proj\.venv\Scripts' is inside a virtual environment that has no pyvenv.cfg, so it is damaged: make it again, py -m venv --clear C:\proj\.venv, and write venv = ".venv"` (`is a file in` for the python), never its launcher as a command; another folder that isn't empty, `… has no Scripts\python.exe, and it isn't a virtual environment (it has no pyvenv.cfg): write the folder of one, or make one: py -m venv .venv`. A path inside one, its `bin` or `Scripts` folder or a `python…` in one that isn't there: `[python] venv 'C:\proj\.venv\Scripts' is inside a virtual environment, and venv names the environment's folder: write venv = ".venv"` (the folder above, with ` (create it: py -m venv C:\proj\.venv)` when it is missing or empty). A venv whose `python` links to one that is gone (an upgrade of the system's Python): `[python] venv '/p/.venv' has a python that links to /usr/bin/python3.99, which isn't there (was Python upgraded?): make the venv again, python3 -m venv --clear /p/.venv, and install its packages again` (the end of the chain of links: a uv venv's `python3` links to `python`). A venv whose python runs another Python than the one it was made with, its `pyvenv.cfg`'s `version_info` (uv, virtualenv), else `version`, major and minor compared with the probed version (its python links to an unversioned `python3` that an upgrade repointed: `uv python install --default --force`, a release upgrade, Homebrew; so the packages in its `lib/python3.13` are gone for it), is broken: `[python] venv '/p/.venv' was made with Python 3.13 (its pyvenv.cfg says so), but its python now runs 3.11 (was Python upgraded?): make the venv again, python3 -m venv --clear /p/.venv, and install its packages again`. The same check holds for a `python` the search finds on `PATH` in a venv (the folder above its folder holds a `pyvenv.cfg`, as an activated venv's does), which is then broken (doctor: COMMAND that python, FROM `PATH`), not passed over: `the venv /p/.venv on PATH was made with Python 3.13 (its pyvenv.cfg says so), …`. For a venv uv made (its `pyvenv.cfg` has a `uv` line; it has no pip), these messages and the next one make it again with uv: `make the venv again, uv venv --clear /p/.venv, and install its packages again`, or, for the `.venv` of a uv project (a `pyproject.toml` and a `uv.lock` beside it), `make the venv again and install its packages: run uv sync in /p`. `venv = ""` is a configuration error (18.2): `[python] venv is empty: name the virtual environment's folder, as in venv = ".venv"`, and so is a `~/…` venv or command with no home folder set (`HOME`, `USERPROFILE` on Windows): `[python] venv: '~/venv-home' starts with ~, but HOME isn't set, so there is no home folder for it: write the full path`. A venv whose python is there but doesn't run (its probe fails) ends `; make the venv again, py -m venv --clear C:\proj\.venv, and install its packages again`; when the Python its `pyvenv.cfg`'s `home` names isn't there (moved, as when a portable Python's folder is renamed, or uninstalled; Windows' copied launcher then says `No Python at '…'`), that is the message, in place of the launcher's words: `engine 'py' [python] venv 'C:\proj\.venv' (set by C:\proj\v2.toml) is not usable: the Python this venv was made from, C:\Tools\py312\python.exe, isn't there any more: put it back, or make the venv again, py -m venv --clear C:\proj\.venv, and install its packages again` (`resolve::base_gone`). A path holding a variable a shell would expand gets no command to create it: `… has no Scripts\python.exe: V2 doesn't expand %USERPROFILE%: write the path out (a ~ at its start is your home folder)`. With no venv or command configured, an activated venv (`VIRTUAL_ENV`) whose `bin/python3` and `bin/python` (Windows `Scripts\python.exe`) link to nothing or are missing is never met by the search (7.5 needs a file), and one whose python is there but fails its probe is skipped (Windows' copied launcher, once the Python its `pyvenv.cfg`'s `home` names is uninstalled or `home` is edited, says `No Python at '…'`, exit 103); the search then finds another Python, and doctor's notes (19.2) and an `ImportError`'s hint (13.5) say so (`resolve::activated_venv_problem`): `the activated venv /p/.venv (VIRTUAL_ENV) has a python that links to /usr/bin/python3.99, which isn't there (was Python upgraded?), so V2 can't use it: make the venv again, python3 -m venv --clear /p/.venv, and install its packages again` (uv's wording for uv's; `… has no bin/python, so V2 can't use it: …`; a folder that is gone, `… isn't there, so V2 can't use it: run deactivate, or make it again, python3 -m venv /p/.venv`; a python of the venv's `bin` (`Scripts`) folder among the search's skipped hits with a failed probe, `… has a python that doesn't run (No Python at '"C:\Python311\python.exe'), so V2 can't use it: make the venv again, py -m venv --clear C:\proj\.venv, and install its packages again`, with what it printed, else the skip reason, `(version probe failed (exit code 103))`, or, when its `home` names a Python that isn't there, `… has a python that doesn't run, so V2 can't use it: the Python this venv was made from, C:\Python311\python.exe, isn't there any more: put it back, or make the venv again, …`; a slow probe isn't one). A hit under `%LOCALAPPDATA%\Microsoft\WindowsApps`, or in any other folder named `Microsoft\WindowsApps` (`LOCALAPPDATA` moved, another user's; case-insensitive), is an alias: a real Store or Install Manager Python, or the placeholder that prints "Python was not found" and exits 9009 (skipped: `Microsoft Store placeholder`). `py -3` (`find_program("py")` + `-3`) is Windows-only and last. Python's embeddable package (python.org's zip: a `pythonXY._pth` beside its `python.exe`, `python313._pth` for 3.13, the only one looked for once the version is probed; `resolve::embeddable`) runs blocks, but that file decides `sys.path`, it ignores `PYTHONPATH` and `PYTHONHOME`, and it has no venv module (its standard library is a zip without it) and no pip until `import site` is uncommented in that file and get-pip.py is run, which puts pip in its `Lib\site-packages` (`resolve::pth_python`; a full Python given a `._pth` has `venv` in its `Lib` and is read the same way, without "embeddable package"): 13.5's install line, doctor's notes (19.2) and 8.4's startup crash say what holds for it instead. It and a Windows venv `python.exe` are launchers that run the real interpreter as a child, and aliases change target without their file changing; so the version is re-checked from `hello` (8.4) and every kill is a tree kill (8.3).
- **macOS.** `/usr/bin/python3`, `cc`, `gcc`, `g++`, `clang` and `clang++` are `xcrun` shims that open an install dialog when the Command Line Tools are missing; V2 first runs `/usr/bin/xcode-select -p`, and a non-zero exit skips them (`Command Line Tools missing`).
- **js** is `node`. One minimum covers the Node family: ts needs Node's built-in `module.stripTypeScriptTypes`, which 22.13+ has except 23.0 and 23.1, so those two count as too old (worded `not supported`). Skip reasons: `too old: 20.11.1 < 22.13`, `not supported: 23.1.0 has no type stripping; V2 needs 22.13+, not 23.0 or 23.1`; `hello` is checked the same way (8.4). **ts** has no process, settings or version check of its own (an `[engines.ts]` table or `V2_ENGINE_TS` is a configuration error, 18.2, 18.4): `resolve(ts)` = `resolve(js)`. A configured js program that prints no Node.js version and is npm's (the stem of its file `npm`, `npx` or `corepack`, or another with a `node.exe` beside it, `node` off Windows), as `V2_ENGINE_JS` set to the Node.js zip's `npm.cmd`, ends `; that is npm, not Node.js: point it at node.exe in the same folder (C:\node\node.exe)` (by its stem, `that is yarn, …`; `point it at Node.js's node.exe` with none beside it; `resolve::npm_fix`). Without js: `EngineNotFound: engine 'ts' (TypeScript) not found — @ts blocks run on the js engine`, then js's tried, skipped and install lines; with a configured js that can't run (its command missing or broken): `engine 'ts' (TypeScript) can't run — @ts blocks run on the js engine, and <js's first line>`, then js's other lines. Stripping: 12.5.
- **c, cpp.** One compiler family per OS. Linux and macOS: the candidates `cc`, `gcc`, `clang` (c) or `c++`, `g++`, `clang++` (cpp), in that order, or the configured command; they are not target-checked. Windows: MSVC `cl` only, the candidate or a configured `argv[0]`, resolved specially:
  1. `find_program("cl")` hits and `INCLUDE` is non-empty (a Developer Prompt): use it with the current environment; FROM `PATH`.
  2. Else the probe cache's `vcvars` entry if still valid (7.7), else a fresh capture (FROM `vswhere`): `%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe -nologo -utf8 -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath` (`VC.Tools.ARM64` on arm64; no output: not found), then `cmd.exe /u /d /s /c ""<install>\VC\Auxiliary\Build\vcvarsall.bat" x64 >nul && set"` (`arm64` on arm64; `/u` makes `set` write UTF-16LE, so non-ASCII user folders survive). From the `NAME=VALUE` lines (names starting with `=` skipped) keep `INCLUDE` (empty: broken, `vcvarsall.bat did not set INCLUDE`), `LIB`, `LIBPATH` and the `PATH` entries not already in `PATH` (prepended at use); `cl.exe` is `find_program("cl")` over that `PATH`.
  3. A configured absolute `cl.exe` gets the current environment if `INCLUDE` is set, else the vswhere capture.

  Any other configured command on Windows (file stem not `cl`, compared case-insensitively: gcc, clang, clang-cl, …) is not usable and is not probed: `EngineNotFound: engine 'c' command 'C:\msys64\ucrt64\bin\gcc.exe' (set by C:\proj\v2.toml) is not usable: C and C++ on Windows use MSVC cl` (exit 3 if uncaught).

  Target: the folder holding `cl.exe` (`x64`, `x86`, `arm64`), normalized (`amd64` and `x64` → `x86_64`, `arm64` → `aarch64`), must equal `std::env::consts::ARCH`. Else an auto-detected hit is skipped (`targets x86 but V2 is x86_64`) and a configured one is not usable: `EngineNotFound: engine 'c' command 'C:\BuildTools\VC\Tools\MSVC\14.40.33807\bin\Hostx64\x86\cl.exe' (set by C:\proj\v2.toml) is not usable: targets x86 but V2 is x86_64` (exit 3 if uncaught).
- **rust.** `rustc -vV`: `release: (\d+)\.(\d+)\.(\d+)`, `host: (\S+)`; the host triple must be v2's own target triple, else the hit is skipped (`targets i686-pc-windows-msvc but V2 is x86_64-pc-windows-msvc`) or the configured command is not usable, as above. The `rustc` on `PATH` is usually the rustup proxy, which picks a toolchain from `RUSTUP_TOOLCHAIN` or a `rust-toolchain.toml` in the cwd while its own file never changes; so a run that builds Rust (never a cache hit, 15.1) runs `<proxy> --print sysroot` once (cwd = the user's) and sets `argv[0] = <sysroot>/bin/rustc[.exe]`, which keys the probe cache and is what ch. 15 compiles with. `shown()` stays the proxy (DOCS 2). rustc finds MSVC `link.exe` itself, so `@rust` on Windows needs the same Build Tools as `@c` (7.8).
- **Run engines** (7.2's Run rows). A configured `command` replaces candidate and fixed args together; its `command[0]` is what is found and probed, and `V2_ENGINE_<TAG>` replaces only that (18.4). The resolved command goes to 17.3.
  - **bash** on Windows is Git Bash only, never a `PATH` hit for `bash` (`System32\bash.exe` is the WSL launcher, which can't open the Windows temp path): `bin\bash.exe` in the parent of the folder holding `git` (found per 7.5; `C:\Program Files\Git\cmd\git.exe` gives `C:\Program Files\Git\bin\bash.exe`), then `%ProgramFiles%\Git\bin\bash.exe`. Either hit has origin `Git` (doctor FROM `git`). `exec`'s hint for a `.sh` names the same bash (`resolve::git_bash`, 5.10).
  - **pwsh** on Windows falls back to `powershell` (Windows PowerShell). Both get 7.2's Windows template (`-ExecutionPolicy Bypass`: Windows PowerShell's default policy blocks scripts), so `V2_ENGINE_PWSH`, which replaces only the program, always gets matching args. Windows PowerShell has no `--version`, so `powershell` (and a configured `command[0]` whose file stem is `powershell`) is probed with `-NoProfile -Command $PSVersionTable.PSVersion.ToString()`.
  - **cmd** is not probed (`version` `None`, doctor VERSION `-`). Outside Windows it is never found: `EngineNotFound: cmd is Windows only`.

Ch. 15 receives the compiled engines' `argv`, `version`, `target` (cl's folder name or rustc's triple; `None` for cc, gcc and clang) and `env` (cl's `INCLUDE`, `LIB`, `LIBPATH` and `PATH` prefix).

### 7.7 Version probes and the probe cache

- **Running:** `argv + probe args`, stdin null, stdout and stderr are read concurrently (8.3) and joined stdout first (lossy UTF-8), cwd the user's, host env plus `spec.env`, 8.3's child rules; a 10 s timer (60 s for vcvarsall, 8.5; a configured command or venv waits as long as its engine's `startup_timeout` (30 s unless set; 0 is no limit) when that is longer, since the user's own command may start slowly, and a timeout says to raise it; set below 10 s, it says `a probe waits at least 10 s: raise [engines.py] startup_timeout above that if the program is only slow`), then `kill_tree`. A run (not doctor) that skipped a PATH hit whose probe timed out and found another says so on stderr, since every run waits for it again: `note: /opt/slow/python3 didn't answer --version in 10 s, so V2 used /usr/bin/python3 (v2 doctor lists what it skips)`. **Parsing:** 7.2's patterns; cc, gcc, clang and their C++ names: the first `(\d+)\.(\d+)(?:\.(\d+))?` of the first `--version` line, held to clang's minimum when the output contains `clang`, else to gcc's; cl, run without arguments: the first `\d+\.\d+\.\d+` of its localized banner; run engines: the first `(\d+)\.(\d+)(?:\.(\d+))?` (`go version go1.22.1 linux/amd64` is 1.22.1), except java: `^\S+ (\d+)(?:\.(\d+))?(?:\.(\d+))?` on the first line (`openjdk 21 2023-09-19` is 21.0.0). A missing part is 0.
- **Cache** `~/.v2/cache/toolchains.json` = `{"format": 1, "entries": {…}, "vcvars": {…}}`. Entry key: sha256(argv, i.e. the path and any prefix arguments such as `py`'s `-3`, canonical path if `canonicalize` succeeds, size, mtime ns, spec hash), spec hash = sha256(engine id, probe args, pattern, V2 version); an alias is stat'ed as the link itself. For py, a python in a venv (a `pyvenv.cfg` in its folder's parent) is also keyed on that `pyvenv.cfg`'s size and mtime and on the path, size and mtime of the program its `home` names (`home\python.exe` on Windows, else `home/python3`; a missing one, or no `home`, as a value of its own; `resolve::venv_stamps`): a Windows venv's python is a copied launcher whose own file doesn't change when that Python is uninstalled or `home` is edited, and a stale entry would start a worker that exits at once (`No Python at`, exit 103) instead of 7.6's message. Value: `{"path", "ok": true, "version", "output"}`, plus `"target"` for cl and rustc. Only usable hits are stored, and the Store placeholder (7.6), as `{"path", "placeholder": true}`: it is no version manager's shim, and its verdict can change only with the alias file. An entry with neither is ignored, and a probe that finds a hit no longer usable removes its entry. `vcvars.<arch>` = `{"install", "vcvarsall_size", "vcvarsall_mtime_ns", "cl", "include", "lib", "libpath", "path_prefix"}`, valid while `vcvarsall.bat` keeps its size and mtime and `cl` and every `include` folder exist (a VS update deletes the old toolset).
- **Access:** runs read entries and write the usable hits they probe. A skipped or too-old hit, the placeholder excepted, is never stored, so every run probes it again: a version-manager shim (pyenv, asdf, volta) runs another interpreter in another project or environment, and a timeout may have been passing (a stale usable entry is caught by `hello`'s version check, 8.4). `v2 doctor` probes afresh and rewrites the entries of the hits it probes, dropping those no longer usable. Writes go to `toolchains.json.<pid>.tmp`, then rename (a failed rename is ignored); an unreadable file or unknown `format` counts as empty.

### 7.8 EngineNotFound messages

`EngineNotFound` (exit 3 when uncaught, 19.1) means there is no usable toolchain: none installed, only too-old versions, an executable that cannot start (8.3), a configured command or venv that does not exist, or `cmd` outside Windows (7.6). Custom engines: 7.4.

```text
EngineNotFound: engine 'py' (Python) not found
  tried: python3, python, py -3 on PATH
  skipped: C:\Users\me\AppData\Local\Microsoft\WindowsApps\python.exe (Microsoft Store placeholder)
  install: winget install -e --id Python.Python.3.13 (or https://www.python.org/downloads/), then open a new terminal
  or set [engines.py] command in v2.toml, or V2_ENGINE_PY
```

With `PATH` unset or empty (cron, `env -i`) the tried line ends `on PATH, which isn't set`, and the install lines give way to `fix: set PATH, or name the program with V2_ENGINE_PY or in v2.toml, [engines.py] command`: the engine may well be installed.

When every hit was too old, the first line says what was found and what V2 needs instead of "not found": `engine 'py' (Python) is too old: found 3.9.6, V2 needs 3.10+` (the newest version found; Node: `V2 needs 22.13+, not 23.0 or 23.1`; a newest Node of 23.0 or 23.1: `engine 'js' (JavaScript) is not supported: found 23.1.0, which has no type stripping; V2 needs 22.13+, not 23.0 or 23.1`), and ts's is `engine 'ts' (TypeScript) can't run — @ts blocks run on the js engine, and engine 'js' (JavaScript) is too old: …`. When no hit was too old but one was installed and didn't run (any skip other than the Store placeholder and a py launcher with no Python, which both mean no Python is installed: its probe failed or timed out, no version, the wrong target), the engine is `broken`, not missing: `engine 'js' (JavaScript) was found but doesn't run (see why below)`; when there are both, the too-old line ends `; another doesn't run (see why below)`. The other lines are as above, plus, after the skipped lines, one `fix:` line for each hit that couldn't load a shared library (its probe said `error while loading shared libraries: libX.so.N: …`): `fix: /usr/local/bin/node needs libatomic.so.1: on Debian and Ubuntu, sudo apt install libatomic1; on Fedora, sudo dnf install libatomic`, or for another library `fix: … needs libX.so.N, which isn't installed: install it with your system's package manager`, and, when a batch file was passed over in a `\\server\share` folder (7.6), `fix: name the program itself by its full path, as in V2_ENGINE_PY=C:\…\python.exe (for pyenv-win, pyenv which python shows it), or run v2 from a folder on a drive` (7.6's words; `resolve::batch_fix`), and when those batch files are all it skipped, no install lines: the program is installed. Then, before the install line, when a hit's probe failed (an exit status or a signal, not a timeout) for another reason while a startup variable that `--version` reads too is set (`NODE_OPTIONS`: Node checks it before it answers; Python answers before it reads `PYTHONHOME` or `PYTHONPATH`; `spec::probe_variables`), `fix: node didn't start: NODE_OPTIONS is set (--bogus-flag), which it reads as it starts: unset it or fix it` (8.4's value, whole; `resolve::probe_fix`). A configured command that isn't usable gets the same `fix:` line (the `NODE_OPTIONS` one as `; node didn't start: …` on its first line), and one that is the Store placeholder (`V2_ENGINE_PY=python3`, a Linux habit, where Windows' Python is `python`) ends, when the search of 7.6 finds a usable Python, with that one, named the way the command was set: `; Python is installed as C:\Program Files\Python311\python.exe: name that one, as in V2_ENGINE_PY=python` (`… as in [engines.py] command = ["python"]` for `v2.toml`; `py`'s `-3` among the words), by its name only when a lookup of that name (7.5) finds that same file, otherwise by its path, `V2_ENGINE_PY=C:\Program Files\Python311\python.exe` or `command = ['C:\Program Files\Python311\python.exe']` (a `WindowsApps` folder first on `PATH` has a placeholder for `python` too); when the search finds none, `; install Python and name its full path, or leave command out: <install hint>`. A configured program that is a folder (`V2_ENGINE_<TAG>`, or `[engines.<tag>] command`, named so in the message) ends, for a virtual environment (the folder holds `pyvenv.cfg`), `; V2_ENGINE_PY names a virtual environment: write [python] venv = '.venv' in v2.toml, or point V2_ENGINE_PY at its python, .venv/Scripts/python.exe` (that part when the program is there); for another folder, `; V2_ENGINE_PY names a folder: point it at the program in it, C:/Python311/python.exe` (on Windows with `/`, which pastes into a TOML `"…"` too, where `\n` of `nodejs\node.exe` is an escape), naming the first that exists of `python.exe`, `Scripts\python.exe` (Windows) or `python3`, `python`, `bin/python3`, `bin/python` (`node.exe`; `node`, `bin/node`), else `; … names a folder: point it at the program, not at its folder`. A configured command that is too old is worded as one found on `PATH`, then what to do: `engine 'py' command 'C:\x\python.exe' (set by V2_ENGINE_PY) is too old: found 3.9.6, V2 needs 3.10+; point it at a newer Python, or install one: <install hint>` (for js, `a newer Node.js`) (Node 23.0 and 23.1: `is not supported: found 23.1.0, which has no type stripping; V2 needs …`). One with a space that names no program, when a run of its first words does (the shortest: a path with spaces, `C:\Program Files\Python311\python.exe -X utf8`, is one program), ends `; V2_ENGINE_PY names one program, without arguments: for a command with arguments, write [engines.py] command = ["py", "-3"] in v2.toml` (a word with a backslash in single quotes).

| engine | Windows | Linux | macOS |
|---|---|---|---|
| py | `winget install -e --id Python.Python.3.13 (or https://www.python.org/downloads/), then open a new terminal` | `sudo apt install python3 / sudo dnf install python3 (or https://www.python.org/downloads/)` | `brew install python (or https://www.python.org/downloads/)` |
| js, ts | `winget install -e --id OpenJS.NodeJS.LTS (or https://nodejs.org), then open a new terminal` | `https://nodejs.org/en/download (distro packages may be older than 22.13)` | `brew install node (or https://nodejs.org)` |
| c, cpp | `winget install -e --id Microsoft.VisualStudio.2022.BuildTools` (workload "Desktop development with C++") | `sudo apt install build-essential` / `sudo dnf install gcc gcc-c++` | `xcode-select --install` |
| rust | `winget install -e --id Rustlang.Rustup` | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` | as Linux |

Run engines' messages have no `install:` line. On Linux, the py line and the libatomic `fix:` line name one package manager when `/etc/os-release`'s `ID` or `ID_LIKE` says which (`debian`, `ubuntu`: apt; `fedora`, `rhel`, `centos`: dnf), as `sudo apt install python3 (or https://www.python.org/downloads/)` and `fix: … needs libatomic.so.1: sudo apt install libatomic1`, and leave out `sudo` for root (a container, a fresh WSL distro), where apt's starts with `apt update && ` (such a system has no package lists yet: `apt update && apt install python3`); otherwise they are as above.

---

## 8. Sessions and worker lifecycle

`engines/session.rs`, on the child rules and watchdog of `engines/process.rs` (8.3, 8.5). A session is the one worker process of a bridged engine, from its language's first need (12.7) to the end of the run: `py`, `js` (which also runs every ts unit) and one per custom bridged tag. Frames: ch. 9; adapters: ch. 14.

### 8.1 Session

```rust
pub struct Session {                                // Interp.sessions: Vec<Session> in creation order (SessionId = index)
    pub key: Rc<str>, pub engine: Rc<ResolvedEngine>, pub state: State,   // "py", "js" (also ts), custom
    child: Option<ChildProc>,                       // 8.3; None for the tests' in-process scripted workers
    reader: Option<FrameReader<Box<dyn Read + Send>>>, writer: Option<Box<dyn Write>>,   // the worker's stdout and stdin (9.1); Send: 8.7's drain reads on a thread per worker
    pub hello: Option<Hello>, caps: Caps,           // 8.4
    next_id: u64, pending: Vec<Pending>,            // host request ids; open requests, innermost last, each with its timer (9.7)
    pub releases: ReleaseQueue, pub callables: CallableTable, timeouts: Timeouts,   // 11.2, 11.8; startup and call (8.4, 8.5)
}   // unit state: 12.1. Sessions are created when their worker is spawned and never removed, so a SessionId
    // stays valid while a callback re-enters the interpreter and starts another engine
pub enum State { Handshaking, Ready, Busy(u32), Closing, Closed, Dead(DeadReason) }
pub enum DeadReason { SpawnFailed(Rc<ErrorValue>), StartupTimeout(Duration), CallTimeout(Duration), Protocol,
                      Exited { exit: Option<ExitInfo>, during: Option<Rc<str>>, at: Option<Span> } }
```

### 8.2 State machine

| From | Event | To |
|---|---|---|
| (no session) | first need (12.7; never `engine_available`): spawn ok / spawn error | Handshaking / Dead(SpawnFailed): `EngineNotFound` |
| Handshaking | `hello`, then `ret` for `init` / deadline / EOF or `err` for `init` / bad frame / `hello` version too old | Ready / Dead: `EngineTimeout` / `EngineCrashed` / `ProtocolError` / `EngineNotFound` (8.4) |
| Ready, Busy(n) | host sends a request (load, module, call, op), also nested ones while serving the worker's `call` | Busy(n+1) |
| Busy(n) | `ret` or `err` for the innermost request | Busy(n−1), Ready at 0 |
| Ready, Busy | EOF, pipe error, child exited, call timeout, protocol violation / program exit (8.7) | Dead(reason) / Closing, then Closed |

Once Dead, every use (loads, its Proxies, handle operations (11.7), imports from its units) raises `EngineCrashed: engine 'py' is not running (stopped earlier: exited with code 1 during a call to 'ratio' at main.v2:14); engines are not restarted`; the other reasons read `was killed by signal 9 (SIGKILL)`, `exceeded call_timeout (5 s)`, `did not start within 30 s` and `broke the protocol`. The first use after an unobserved idle death raises 8.6's "exited while idle" instead, and Dead(SpawnFailed) re-raises its `EngineNotFound`. Releases for a Dead session are dropped (11.2); definition blocks it loaded stay `Loaded` on the host, and their Proxies fail as above. v1 never restarts a session.

### 8.3 Spawning and child processes

1. py, js: the embedded adapter is materialized per 14.1 and its path substituted for `{adapter}`.
2. `Command::new(argv[0]).args(rest + fixed_args)`, cwd inherited (the host never calls `set_current_dir`), env = the host's plus `spec.env` (an inherited `NODE_PATH` or `PYTHONPATH` passes untouched). The project root travels in `init` (9.3). stdin and stdout are piped (VEP), stderr is inherited. Before any user code runs, the adapter keeps user code off the protocol streams (9.9), so the user's stdin belongs to `input()` alone.
3. A spawn error is `EngineNotFound: could not start 'C:\...\python.exe': <OS message>` (Windows: a `%1` in the message reads `the program`, and error 193 adds `(not a valid program for this machine)`). Too many open files (EMFILE, a low `ulimit -n`) adds `hint: the open-files limit (ulimit -n) is too low for V2 and its workers: raise it, as in ulimit -n 1024`.

**Child rules**, for every process V2 starts (workers, probes, compilers, run engines, compiled programs, `exec`, 5.10):
- **Own session or group:** Unix: `pre_exec` calls `setsid()` (not `process_group(0)`), so the child leads a new session and process group and has no controlling terminal: terminal SIGINT and SIGHUP never reach it, and a `/dev/tty` prompt (git credentials, ssh, sudo, `getpass`) fails at once with `ENXIO` instead of stopping the child on SIGTTIN/SIGTTOU (DOCS 4.6). Windows: `CREATE_NEW_PROCESS_GROUP` (+ `CREATE_NO_WINDOW` when the host has no console, `GetConsoleCP() == 0`, so console children share one hidden console instead of opening a window each), which blocks Ctrl+C only; Ctrl+Break and console close reach every process on the console. Children stay attached to the host's console (never `DETACHED_PROCESS`, whose descendants would open new console windows) and only stdin is replaced (null or a pipe), so a program that reads the console directly (`CONIN$`: an ssh password prompt, Python `getpass`) waits for keyboard input (DOCS 4.6). Adapters ignore SIGINT and SIGBREAK (14.3, 14.4), so a keyboard interrupt never makes a worker die mid-frame or print a `KeyboardInterrupt` trace; the host's console control handler sets `INTERRUPTED` as its first action, so failures of other children caused by the same event are never reported (6.5). stdin: a pipe for workers and for `exec` with `input`, null for the rest (DOCS 4.6).
- **Windows:** created suspended, put in its own Job Object (`KILL_ON_JOB_CLOSE | BREAKAWAY_OK`) and resumed with `NtResumeProcess` on its process handle, so every descendant is in the job unless created with `CREATE_BREAKAWAY_FROM_JOB`. **Linux:** after `setsid()`, `pre_exec` runs `prctl(PR_SET_PDEATHSIG, SIGKILL)` and `_exit(1)` if `getppid()` is no longer the host; PDEATHSIG fires when the spawning *thread* exits, so spawns and shutdown run on the interpreter thread (1.3). **All:** adapters exit on stdin EOF (macOS has only this, so a busy worker outlives a SIGKILLed host until its next read). These three also cover a host that dies with no exit path (6.5).
- **Windows command lines:** a program parses its own command line, so each argument goes on it by `command_line_arg` (`raw_arg`), which both kinds of parser read back as given: the C runtime's (MSVCRT's rules, also Rust's, Python's and Node's) and the MSYS runtime of Git Bash's programs (`find`, `grep`, `printf`, `sh`), which, in a word it sees unquoted, globs (`*.v2`), expands braces (`{a,b}`) and a leading `~`, reads a leading `@` as a file of arguments (`@data.json`) and takes `'` as a quote. A word with no space, tab or other control character, none of `" ' * ? [ ] { } ( )` and no leading `~` or `@` goes as it is (`--format=%s`, `C:\dir\x`, `%PATH%`); any other goes in double quotes, `"` as `\"`, the backslashes before a quote or before the closing one doubled. MSYS reads `\\` in quotes as one backslash where the C runtime keeps both, so a run of two or more before another character ends the quotes (doubled, as before a quote) and the word goes on unquoted up to its next backslash or character that needs them (`\\server\share\My Docs` is `"\\\\"server"\share\My Docs"`); only such a run right before a space, a tab or another control character, a `'` or one of `* ? [ {` still reaches an MSYS program halved. A `.bat` or `.cmd` file and `cmd.exe` keep Rust's own quoting, which is cmd.exe's (7.5; quoting every argument would hand `%1` its quotes), and cmd's `/c` tail goes as written (5.10).
- **`kill_tree`** is every kill V2 performs: Windows `TerminateJobObject(job, 1)`, Unix `kill(-pid, SIGKILL)` (the session leader's pid is its pgid; `kill(pid)` on `ESRCH`), then on Linux every process still in the child's session, found through `/proc` (one that only left the process group, `setpgrp`, to keep Ctrl+C away; one that called `setsid` has detached and lives on, DOCS 16). Launchers (`py -3`, a Windows venv `python.exe`, `.cmd` shims, `bash`) run the real work in a descendant that also holds the pipes.
- **Lifetime:** workers are killed as a tree at shutdown (8.7); a run-engine, compiled-program or `exec` child is killed as a tree, with everything it started, when its block or `exec` call finishes, times out or V2 exits; probes when they exit or time out.
- **Pipes** (every child but workers, whose stdin and stdout carry frames, 8.5): every piped stream is serviced at the same time as the others. Each piped output (expression-position stdout, the stderr tail, probe, compiler and `exec` output) gets its own helper thread that collects into memory (`run_captured`: 1 GiB at most, past which it kills the tree, 5.10) or forwards, and `exec`'s `input` is written by another, which then closes the pipe. The interpreter thread waits only for the exit (or the timer's kill), then runs `kill_tree`, then joins the output helpers, which reach EOF because the tree is dead, then reaps (below). The input writer is not joined: its writes fail once the tree is dead, and a process that escaped the tree could keep it blocked.
- **Reaping:** exit is observed without reaping (Unix `waitid(P_PID, pid, WEXITED|WNOHANG|WNOWAIT)`, Windows the process handle), then `kill_tree`, then `wait()` and the job handle closed: nothing a child started without detaching (`setsid`, `CREATE_BREAKAWAY_FROM_JOB`) outlives it, and the unreaped leader keeps its pgid from being reused.

`<run>` is the private per-run temp dir (0.1, 20.3).

### 8.4 Handshake

One deadline covers spawn → `hello` → `init` reply: `startup_timeout` (`[engines.js]`'s for ts; default 30 s; 0 = none), one timer (8.5). No worker is started through a `.bat` or `.cmd` while V2's working folder is a `\\server\share` (7.6's `EngineNotFound`): cmd.exe would run it in the Windows folder.
- **Before `hello`:** preload noise and a first frame other than `hello`: 9.3.
- **`hello`** must carry `vep: 1` (else 9.3's `ProtocolError`). Its `caps` are recorded and gate features (17.5). For built-in engines its `version` must meet 7.2's minimum (js's for the js session, also when a ts block started it), because a launcher or alias may run another interpreter than the one probed: else kill, Dead(SpawnFailed), `EngineNotFound: Python 3.9.6 at C:\...\python.exe is too old (needs 3.10+)`, or `JavaScript 23.1.0 at C:\...\node.exe is too old (needs 22.13+, not 23.0 or 23.1)` (7.6).
- **`init`** (9.3): `ret` → Ready; `err` (of any type) → kill, `EngineCrashed: engine 'py' failed to start: <message>`. A session is not started at the cross-language depth limit, where `init` would be the 65th crossing (`RecursionError`, 9.7).
- **EOF first:** Windows exit status 9009 (a Store placeholder the probe did not catch) → Dead(SpawnFailed), `EngineNotFound: engine 'py' could not start: C:\...\python3.exe is the Microsoft Store placeholder (exit code 9009)`; otherwise `EngineCrashed: engine 'py' exited during startup (exit code 1); see its error output above, if any`. Exit code 0 (a program that ended without a word, as one `conda run` starts: it keeps the program's standard input and output for itself) adds `; it ended without a word to V2, which talks to the program over its standard input and output: a command that runs it through another program must pass them through`; an exit with code 0 after the `hello`, during `init` (capture-mode `conda run` writes the program's output, its `hello` included, only when the program has ended), `; it ended before V2 could talk to it over its standard input and output: …`. js on Unix talks over two pipes it inherits (9.9), which a wrapper can close while it passes standard input and output through (Python's `subprocess` closes descriptors above 2 by default): `over two pipes it inherits (named in V2_VEP_FDS): a command that runs it through another program must leave them open`. When the command's words have `conda` (a program of that name), then `run`, without `--no-capture-output` or `--live-stream` after it, it ends `; conda run passes them through only with --no-capture-output: add it after run, or name the environment's python (envs/sales/bin/python in conda's folder) in [engines.py] command` (the name after `-n` or `--name`; `envs\sales\python.exe` on Windows; without a name, no path; for js on Windows, `the environment's node in [engines.js] command`); for js on Unix, with those options too, `; conda run closes them, even with --no-capture-output: name the environment's node (envs/web/bin/node in conda's folder) in [engines.js] command`, and the same for `mamba run` and `micromamba run` (`; micromamba run closes them: name the environment's node …`), whose `run` passes standard input and output through and has no `--no-capture-output`, so py through them gets no advice, but for `mamba` (mamba 1 hands `run` to conda's) `; if this is mamba 1, whose run is conda's, it passes them through only with --no-capture-output: add it after run, or name …`, which doctor doesn't say, as mamba 2's works (`session::wrapper_note`, `conda_run_advice`; doctor gives that advice as a note, 19.2). On Windows, whatever the exit code, a command that runs `wsl.exe` or WSL's `bash.exe` adds 7.6's `; wsl.exe runs a Linux python, which the Windows v2 can't use, as its adapter is a Windows path: name a python.exe, or run the Linux v2 inside WSL`. Then, for the variables the program reads as it starts that are set (`PYTHONHOME`, `PYTHONPATH`; `NODE_OPTIONS`; a session a ts block started is js's), `; PYTHONHOME is set (C:\nope), which it reads as it starts: unset it or fix it` (the value whole, its control characters escaped as a path's (2.1), as the wrong folder is often its last entry; none for Python's embeddable package, which ignores them (7.6; `resolve::read_at_start`); for two, `PYTHONHOME is set (…) and PYTHONPATH is set (…), which it reads as it starts: unset them or fix them`, as doctor's fix line, 19.2): `--version` runs without them (only a bad `NODE_OPTIONS` option stops it, 7.8), so a stale one shows only here, and in doctor, which starts the program to see (19.2); a js crash after startup names `NODE_OPTIONS` too (8.6). On Unix under an address-space limit it also ends `; a memory limit (ulimit -v) of 400 MB is set for this shell, and Node.js reserves about 1 GB as it starts: raise it` (`the engine may need more` for another engine): Node's own `Fatal process out of memory` comes before it.
- **Deadline:** kill, `EngineTimeout: engine 'py' did not start within 30 s (C:\...\python.exe); raise [engines.py] startup_timeout if it is only slow`.

### 8.5 Requests and call_timeout

- **Before each request:** check whether the child exited (without reaping; if so, Dead, 8.6), check the cross-language depth limit (64, 9.7) and send the session's queued releases first, but those for handles the request names (11.2).
- **Frame I/O:** compact `serde_json` plus `\n` in one `write_all`, then flush; reads split on `\n` only and drop one `\r` before it; frames are at most 64 MiB in either direction (9.1). A session's stdout is read only while the host awaits a reply from it (and in 6.5's shutdown drain), so a worker can call V2 only while the host has a request outstanding to it; adapters reject calls from idle code themselves (9.8).
- **Watchdog** (the thread: 1.3; `engines/process.rs`): a stack of timers (limit `None` = none), each with its child's `KillHandle` (the Job Object, or the pid = pgid), plus every running child's `KillHandle` (for the interrupt path, 6.5) and the run dir. Only the top timer's clock runs, and not while it is paused: a timer pushed onto another stops that one's clock until popped, and a pause lasts until its resume wherever the timer is in the stack. Each wait is bounded (a short tick, or less when the top timer's deadline or the next exit check comes first), a wake-up cuts it short (6.5's Windows handler), and every wake checks `INTERRUPTED` (6.5).
- **Timers:** each host request after `init` (load, module, call, op) pushes a timer with `limit = call_timeout` (`None` for 0); the handshake pushes one with `startup_timeout`, probes one with 7.7's limit. Timers have no kind: the code that pushed one words its message. An inbound `call` from that worker pauses the top timer, sending its reply resumes it, and the reply to the request pops it. Only the top timer runs, so exactly the innermost pending host→worker request is timed, never V2 code or another engine: V2 → py (A runs) → V2 callback (A paused) → js (B runs, popped) → py (A resumes). Run engines and compiled programs use the same timers, with `call_timeout` (message: 17.3); in-process calls (compiled exports, `extern`) cannot be interrupted and are never timed. Every 250 ms the watchdog also checks whether the top timer's child exited (`WaitForSingleObject(h, 0)`; `waitid` + `WNOWAIT`) and then runs `kill_tree`, so a descendant holding the stdout pipe cannot keep the read from reaching EOF (on Unix a descendant that called `setsid` is out of that kill's reach, so the read itself ends once the child has exited and what it left in the pipe has been read; whatever a descendant writes after that is not waited for).
- **Suspension:** time the process spends stopped (Ctrl+Z, SIGSTOP) or the computer asleep runs no call, so the running clock skips it: after Ctrl+Z the time stopped; otherwise when the watchdog wakes at least 1 s later than it asked to.
- **Expiry:** the watchdog runs `kill_tree` and marks the timer fired; the blocked read gets EOF, sees it, and the session becomes Dead(CallTimeout): `EngineTimeout: a call to 'slow' in engine 'py' exceeded call_timeout (5 s); the engine was stopped` (v1 cannot cancel a call any other way). The engine stays dead for the rest of the run; later uses raise 8.2's `EngineCrashed`. A child's timers and kill registration are removed before it is reaped, so a reused pid is never signalled or waited on.

### 8.6 Crash detection

EOF on stdout, a pipe error (`EPIPE`, `ERROR_BROKEN_PIPE`, `ERROR_NO_DATA`; Rust ignores SIGPIPE), the child found exited, or a fatal protocol violation (9.11): `kill_tree`, reap, record the `DeadReason`.
- The pending request fails with `EngineCrashed: engine 'py' (pid 4242) exited during a call to 'ratio' (exit code 3221225477 = 0xC0000005); see its error output above, if any` (Windows codes ≥ 0x80000000 also in hex; Unix: `killed by signal 11 (SIGSEGV)`, or `killed by signal 77` for a signal without a known name), or with 9.11's `ProtocolError`. The worker's last output is already on stderr. A js session's (a ts block's too) ends with 8.4's `; NODE_OPTIONS is set (--max-old-space-size=8), which it reads as it starts: unset it or fix it` when that is set, as a heap limit there stops Node later (`node_options_note`).
- A death while idle is noticed at the next use: `EngineCrashed: engine 'py' exited while idle (exit code 1)` (for js with the `NODE_OPTIONS` clause above); later uses get 8.2's "not running".
- Raised inside another engine's callback, the error travels back through that engine like any V2 error (9.6). When a callback's nested request to the worker that called it kills that session, no reply goes out: the outer call gets the callback's error, or 8.2's "not running" if V2 code caught it. EOFs caused by an interrupt are not reported (6.5).

### 8.7 Shutdown and interrupts

6.5 owns the shutdown order (exit codes: 19.1); the session part:
1. On every orderly exit (end of program, `exit(code)` (5.8) even inside a callback, an uncaught error after it is printed), each Ready or Busy session, newest first, gets `{"t":"bye"}`, its stdin is closed, state Closing. An adapter blocked in a stub exits without unwinding user code (9.10).
2. 6.5's shared 1 s drain reads every closing worker at once (a reader thread each), so one that ignores `bye` cannot hold up another's exit output behind a full pipe; it forwards `out` frames as they arrive (exit-handler output is kept) and ignores all others; worker exit codes are ignored.
3. Survivors are `kill_tree`d and every worker is reaped (8.3), state Closed; 6.5 then kills the remaining children and removes `<run>`.

Interrupts and panics take 6.5's interrupt and panic paths instead (no `bye`; workers are killed as trees).

### 8.8 Parking

A worker with no request in flight for 10 s gives back its RAM. Nothing else about it changes: same process, state, handles and background threads. Its pages leave the working set and fault back in on use.

- **Idle** means no timer for that child on the stack (8.5 pushes one for every request, with or without a limit); each pop restarts the clock. A worker parks once per idle period.
- **Who:** the watchdog decides, and a short thread of its own does the work, because a trim takes about 40 ms per GB and must never delay a timer. On Windows it lists the Job Object's processes (`QueryInformationJobObject`, `JobObjectBasicProcessIdList`), which covers launchers (8.3), and calls `K32EmptyWorkingSet` on each; errors are ignored, because a process may have gone.
- **Linux and macOS:** nothing. A normal user can't do this to a child (`process_madvise` gives `EPERM`), and the kernel reclaims idle pages under pressure when swap or compression exists. Linux without swap keeps them.
- **Never** a session with `call_timeout` set, because bringing its memory back would count against the timer. Every other session registers once its handshake is done (`process::allow_parking`).
- It releases the working set, not commit: the memory stays charged to the system, and without a page file modified pages stay in RAM.

---

## 9. The VEP/1 protocol

VEP/1 (V2 Engine Protocol, version 1) connects the host to every bridged engine: the `py` session, the `js` session (which also runs `ts` blocks) and custom `bridged` engines (17.5). Host side: `engines/vep.rs` on top of `engines/session.rs` (ch. 8); values go through `codec/vep` (ch. 10); the built-in adapters are `adapters/py.py` and `adapters/js.js` (ch. 14). This chapter owns the wire format and the call discipline: the member names and shapes of every frame are defined in 9.2 and 9.4; their semantics are in 12.2 (`load`), 13.5 (`module`), 13.7 (`imports` entries) and ch. 11 (handle ops).

### 9.1 Transport and framing

- **Channels.** Host→worker frames travel on the worker's stdin, worker→host frames on its stdout. Worker stderr is inherited and never parsed.
- **Frames.** One UTF-8 JSON object per line, ended by a single `\n` (0x0A) and written in binary mode (no newline translation). Writers use a real JSON encoder (serde_json, `json.dumps`, `JSON.stringify`) with compact separators and put `t` first; the encoder escapes every U+0000–U+001F inside strings, so a frame never contains a raw 0x0A. Raw U+2028/U+2029 inside strings are allowed.
- **Splitting.** Both sides read raw bytes, split on 0x0A only, then decode each complete line. Nothing splits on `\r`, U+0085 or U+2028: no Python text-mode `readline` or `for line in sys.stdin`, no Node `readline` module. The host drops one `\r` before the `\n`; adapter parsers accept it as JSON whitespace.
- **Size limit: 64 MiB per frame, both directions**, not counting its newline. The host refuses to send a larger frame: `MarshalError: value too large to send to engine 'py' (70 MiB, limit 64 MiB)`, nothing sent. A longer incoming line is fatal: `ProtocolError: frame from engine 'py' exceeds 64 MiB`. An adapter never sends a larger frame: a reply becomes `err` `MarshalError`, a stub call raises `MarshalError` in the foreign code, `out` text is chunked (9.9). The adapters' encoders keep a running lower bound of the frame's size (2 bytes per list or dict item, the length of each string, the base64 length of bytes) and stop with `MarshalError: value too large to send to V2 (more than 64 MiB)` as soon as it passes the limit, so a huge or heavily shared value costs neither the time nor the memory of encoding it all. What the bound misses is caught when the frame is written, as is JSON text too long for a JS string.
- **Host I/O.** Reading gives up as soon as a pending line passes the limit; writing: 8.5. A write error (EPIPE, `ERROR_BROKEN_PIPE`, `ERROR_NO_DATA`) is handled like EOF: the worker is gone (8.6).
- **Adapter I/O.** Stdin EOF or a broken stdout pipe means the host is gone: the adapter exits quietly with status 0. A host line it cannot parse, or a reply it is not waiting for, is reported on stderr and the adapter exits with status 2 (14.2).
- **Evolution.** Both sides ignore unknown members. The set of `t` values is fixed for VEP/1: an unknown `t` from a worker is fatal (9.11); an adapter answers an unknown `t` that carries an `id` with `err` `ProtocolError` and keeps running.

### 9.2 Frame catalogue

| `t` | Direction | `id` | Members |
| --- | --- | --- | --- |
| `hello` | W→H | – | `vep`, `version`, `pid`, `caps` (9.3) |
| `init` | H→W | yes | `project_root` (9.3) |
| `load` | H→W | yes | `unit`, `kind`, `lang`, `file`, `line`, `source`, `args`, `imports`, `exports`, `want_value` (9.4; 12.2) |
| `module` | H→W | yes | `unit`, `name`, `from`, `names` (9.4; 13.5) |
| `call` | both | yes | `target`, `args`, `kwargs` (9.5) |
| `op` | H→W | yes | `op`, `h`, plus `name`/`key`/`value`/`n` as the op needs (9.5, 11.3) |
| `ret` | both | echoed | `value`; plus `exports` and `missing` when answering `load`/`module` |
| `err` | both | echoed | `error` (9.6) |
| `out` | W→H | – | `stream` (`"stdout"` or `"stderr"`), `text` (9.9) |
| `release` | both | – | `h` (H→W: handle ids) or `fn` (W→H: callable ids) (9.10) |
| `bye` | H→W | – | – (9.10) |

`init`, `load`, `module`, `call` and `op` are requests: each gets exactly one `ret` or `err`. A session is `hello`, `init`, any number of requests (with nested traffic), then `bye`. In `op` frames and `call` targets, `h` is the bare handle id, not an `h` tag (10.3).

Example: a `py` block imports the V2 function `log_line` (route 1) and exports `ratio`. The worker's `call` carries the worker's own id 1; the host's `call` has host id 3.

```json
{"t":"hello","vep":1,"version":"3.12.4","pid":4242,"caps":["modules","handles","kwargs","reverse"]}
{"t":"init","id":1,"project_root":"C:\\work","caps":[]}
{"t":"ret","id":1,"value":null}
{"t":"load","id":2,"unit":"0:812","kind":"definition","lang":"py","file":"C:\\work\\main.v2","line":3,"source":"\ndef ratio(a, b):\n    log_line(\"dividing\")\n    return a / b\n\n","args":{},"imports":[{"as":"log_line","route":1}],"exports":["ratio"],"want_value":false}
{"t":"ret","id":2,"value":null,"exports":[{"name":"ratio","kind":"func"}],"missing":[]}
{"t":"call","id":3,"target":{"unit":"0:812","name":"ratio"},"args":[1,0],"kwargs":{}}
{"t":"call","id":1,"target":{"route":1},"args":["dividing"],"kwargs":{}}
{"t":"ret","id":1,"value":null}
{"t":"err","id":3,"error":{"type":"ForeignError","message":"division by zero","foreign_type":"ZeroDivisionError","trace":[{"file":"C:\\work\\main.v2","line":6,"col":12,"func":"ratio"}],"text":"Traceback (most recent call last):\n..."}}
```

### 9.3 Handshake: `hello` and `init`

**`hello`** is written by the adapter before any user code runs. Ch. 8.4 owns its deadline (`startup_timeout`), the minimum-version check and the startup error messages.
- `vep` must be `1`; anything else is fatal (9.11): `ProtocolError: engine '{tag}' speaks VEP/{n}; this V2 speaks VEP/1`. `version` is the interpreter's version (`"3.12.4"`, `"22.13.1"`), which 8.4 checks against 7.2's minimum. `pid` is the interpreter's own pid, not a `py -3` launcher's; crash messages use it (8.6).
- `caps` is a list of strings. The host gates features on `modules`, `handles`, `kwargs` and `reverse` (what each enables and the error without it: 17.5) and ignores unknown caps. The py adapter sends `["modules","handles","kwargs","reverse"]`; the js adapter sends `["modules","handles","reverse"]` (no `kwargs`: JS functions take positional arguments only, 10.8).
- **Preload noise.** Code can run before the adapter owns fd 1 (`sitecustomize`, `.pth` files, `NODE_OPTIONS=--require`). Before `hello`, a stdout line that is not a JSON object is copied to the host's stderr and startup continues. Noise with no newline of its own shares `hello`'s line, so anything in front of `{"t":"hello"` on a line is noise too (`t` is the frame's first member). The first JSON object must be a valid `hello`; any other frame, an `out` frame included, is fatal: `ProtocolError: engine 'py' sent 'ret' before hello`. From `hello` on, every line must be a frame (9.11). EOF before `hello` is a failed start (8.4).

**`init`** is always the first request; the adapter answers `ret` with `value: null` (an `err` kills the worker, 8.4). Its `caps` lists what the host supports beyond VEP/1's core, as `hello`'s do for the worker, so a side sends an optional tag only to one that announced its cap; this host sends `[]`. **Reserved:** the cap `buffer` and the `$v2` tag `buffer`, for data shared without copying (an array's memory); VEP/1 defines neither yet, and no side sends them. Its `project_root` is the canonical folder (2.1) holding `v2.toml`, or the entry script's folder. py prepends it to `sys.path`, and at each `load`/`module` of a `.v2` file puts that file's folder first and the root second (14.3), moving entries an inherited `PYTHONPATH` or another file's load already put there, which gives DOCS 7's order for every file. js does not use it: Node's own lookup from each `.v2` file's folder (12.4, 13.5), plus any inherited `NODE_PATH`, finds packages. Its `terminals` says which of V2's stdout and stderr are terminals as the worker starts, `{"stdout": [columns, rows], "stderr": null}` (`null`: not one; `process::terminal_size`: Unix `isatty` and `TIOCGWINSZ`, 0 x 0 when the size can't be read; Windows a console and its window), which blocks see as theirs (9.9).

### 9.4 Units: `load` and `module`

`load` members are those of 9.2 (there is no `col`); 12.2 says what each holds and how the adapter runs the unit, and 12.1 when a definition is loaded (once, never retried). The wire rules:
- **`unit`** is the wire UnitId `"<FileId>:<byte offset of the block's @>"`. Module units are `"<canonical tag>:<name>"` (`py:statistics`, `js:date-fns`; `ts.pkg` is `js:pkg`): one unit per module per session (13.5).
- **`imports`**: each entry has exactly one of two shapes (when each is used: 13.7): `{"as","unit","name"}` (plus `route` when the unit is a block) binds a stub that looks `name` up in that unit of this session at each call, whether or not the unit has loaded yet; `{"as","route"}` binds a stub for that route (13.6).

The `ret` of a `load` or `module` carries `value`, `exports` and `missing`. `exports` is a list of `{"name","kind","value"?}`: `kind` is `"func"` or `"value"` (whether a call constructs is decided at call time, 11.3), and `value` (the constant, copied at load) is present only for `"value"`. `missing` lists the requested names the unit does not define; the host raises `ImportError` (12.2, 13.5). A module's `ret` with names in `missing` may add `names`, the module's public names, for that error's did-you-mean (13.5); anything but a list of strings counts as none. It may also add `hint`, which the host gives that error when no hint of its own applies (the js adapter's for a dual package, 13.5); anything but a non-empty string counts as none. An exception while running the block is answered with `err`; a foreign syntax error is `ForeignError` with `foreign_type: "SyntaxError"` at the `.v2` line (12.3, 12.4, 12.6).

**`module`** imports a foreign package into its module unit; its members, the adapter's lookup and the not-found mapping to `ImportError` are 13.5's. Without the `modules` cap the host raises before sending (17.5).

### 9.5 `call` and `op`

| `target` | Sent by | Meaning |
| --- | --- | --- |
| `{"unit","name"}` | host | a Proxy call (13.4): `name` is looked up in the unit's namespace when the call arrives (gone since load: `err` `NameError`, 12.3); a JS constructor is called with `new` (11.3) |
| `{"h"}` / `{"h","method"}` | host | calls the handle's object, or one of its methods, in one round trip (11.3) |
| `{"route"}` | worker | calls a route created by an inner `@import`; a `Pending` route resolves when called (13.6) |
| `{"fn"}` | worker | calls a callable the worker received as a value: a V2 function, or another session's callable handle (11.8) |

- `args` (array) and `kwargs` (object) are always present, empty when unused. The host expands a spread `...xs` into `args`. Named arguments go only to sessions with the `kwargs` cap (10.8).
- A worker `call` with a `unit` or `h` target, or from a worker without the `reverse` cap, is fatal. An adapter answers a host `call` with a `route` or `fn` target with `err` `ProtocolError`.
- An unknown `route` or `fn` id is answered with `err` `ProtocolError: unknown callable id 42`; the session stays usable.

`op` goes host→worker only, and never to a session without the `handles` cap: `getattr {h,name}`, `setattr {h,name,value}`, `getitem {h,key}`, `setitem {h,key,value}`, `len {h}`, `iter {h}`, `next {h,n}`, `str {h}`, `value {h}` (`to_v2(h)`; semantics: 11.3–11.5). `setattr` and `setitem` answer `null`; `iter` answers `{"it":<h tag>,"mapping":bool}`; `next` answers `{"items":[...],"done":bool}`.

### 9.6 `ret` and `err`

`ret.value` is an encoded value (ch. 10). `err.error` has these members:

| Member | Content |
| --- | --- |
| `type` | a V2 error kind: `ForeignError` for a foreign exception; `ImportError`, `NameError`, `ValueError`, `MarshalError`, `ProtocolError` or `Error` for failures the adapter detects itself. These seven are the only kinds an adapter may report |
| `message` | the message text |
| `foreign_type` | the foreign exception class, such as `"ZeroDivisionError"`, or `null` |
| `trace` | frames `{"file","line","col","func"}` (mapped per 12.6), innermost first, adapter-internal frames left out; `col` in code points or `null`; `func: null` at a block's top level |
| `cause` | optional: the chained exception (`__cause__`, `e.cause`) in the same shape, at most 8 deep |
| `text` | optional: the engine's own formatted traceback |
| `v2` | optional: the `error` tag (10.3) of a V2 error that propagated through foreign code |
| `data` | optional, for a foreign exception: an object of its own simple fields, at most 32, with strings, booleans, `null` and numbers V2 holds exactly (JS: its own enumerable properties but `message`, `stack`, `cause`, a safe integer or a float; Python: `exit_code` for `SystemExit` (its `code`, `0` for `None`), `errno`, `strerror`, `filename`, `filename2` of an `OSError` when set, then its `__dict__`'s names without a leading `_`, an int within 64 bits); never a key `$v2`. The adapters' own `process.exit(n)` and `sys.exit(n)` errors carry `exit_code` |
| `hint` | optional, for a foreign exception or an `ImportError`: the adapter's hint for it, which becomes the V2 error's when V2 has none of its own, and wins over the host's for an `ImportError` (Python: a file of the user's named like a standard module, met by another module's import or attribute, `your C:\x\random.py hides Python's own random module, which other code needs: give your file another name`; a `UnicodeDecodeError` of the `'utf-8'` codec, below; an `ImportError`, or an `OSError` raised by a module's top level as it is imported, whose message names a shared library, `libGL.so.1: cannot open shared object file: …` or `PortAudio library not found`, `libGL.so.1 is a system library the package needs, not a Python package: install it with your system's package manager` (`library_hint`); a `module`'s `ImportError`, 13.5) |

**How the host converts `err`:**
- With `v2`, the host restores that error (type, message, data, location) and puts the foreign frames into its trace, between the frames of the callback and those below the request; an error never raised before (made with `error()` and raised by foreign code) gets V2's frames at the call site after them, as its first raise (5.9). So a V2 error thrown in a callback comes back with its original type, whatever its kind (DOCS 10).
- Without `v2`, the kind is `type` if it is one of the seven kinds above; any other `type`, a host-only kind such as `EngineNotFound` included, becomes `ForeignError` with `foreign_type = type`, so a worker's own error can't make V2 exit 3. The location is the innermost frame that maps into a `.v2` file, else the V2 call site (2.4, 12.6); `e.lang` follows 12.6. If `trace` is empty, the lines of `text` become the trace.
- A `ForeignError`'s `data` becomes `e.data`, decoded as a value (ch. 10). One that V2's setup causes gets the adapter's hint: Python's `UnicodeDecodeError` from the `'utf-8'` codec, `hint: blocks run Python in UTF-8 mode, so open() reads UTF-8: for a file in another encoding, name it, as in open(path, encoding="cp1252")`, except when the bytes at position 0 are a UTF-16 byte-order mark, FF FE or FE FF (which Excel's Unicode Text and PowerShell's `Export-Csv -Encoding Unicode` write and cp1252 would read as garbage; UTF-32's starts the same): `hint: blocks run Python in UTF-8 mode, so open() reads UTF-8: text that starts with byte 0xff or 0xfe is probably UTF-16 (what Windows PowerShell's > and Out-File write): name it, as in open(path, encoding="utf-16"), or pd.read_csv(path, encoding="utf-16"); a binary file, such as an image, opens with open(path, "rb")`. A 0xff or 0xfe at position 0 without the mark is a code page's letter (`ş` is 0xfe in cp1254, `я` 0xff in cp1251) or a binary file's byte (a JPEG starts FF D8): the code-page hint, then `; a binary file, such as an image, opens with open(path, "rb")` (`py.py` `decode_hint`, which reads the bytes from the exception; Python's message names the first byte only). A JavaScript one located at a block line that starts with `(`, `[` or `` ` ``, whose code line above ends with a letter, digit, `_`, `$`, `)`, `]` or a quote (`const a = [1, 2]` then `[3, 4].forEach(…)`), gets 4.4's advice as its hint: `JavaScript reads this line as part of the one above, as in a(…) or a[…]: end the line above with ;` (`joined_line`).
- Adapters make every error string well-formed first (10.4).

**The other direction.** When a worker's `call` fails (a V2 function, or another language's function behind a route or `fn`), the host's `err` always carries `v2`. Adapters raise it as `V2Error` (14.2), which keeps the tag, so if foreign code does not catch it, it goes back in `v2` unchanged. The adapters encode its data and cause again (their handles and stubs belong to the new frame, 11.1, 11.8), so the tag the host sends also has a `ref`, a number per session, and an error that comes back with a `ref` the host sent during the outermost request to that engine gets the data, cause and hint it had when sent: what the foreign language can't keep (JS's `1.0`, key order, a function of that engine) is not lost. The worker's copy is still decoded, so its handle counts balance; later, or with an unknown `ref`, that copy is the data.

### 9.7 Ids, depth and the re-entrant loop

- **Ids.** Each side numbers its own requests 1, 2, 3 … per session (u64, never reused). `ret`/`err` echo the other side's id, so the two counters never collide.
- **LIFO replies.** A reply must answer the receiver's innermost outstanding request. Anything else is fatal: `ProtocolError: engine 'py' broke the protocol: reply to request 7, which is not the innermost pending request`.
- **Depth limit (DOCS 8.4).** `xdepth` counts the crossings in progress across all engines: each outstanding host request, each worker `call` being served, each in-process native call (16.7) and each `host_call` (15.9). The crossing that would be the 65th raises `RecursionError: cross-language call depth limit (64) exceeded`: at the V2 expression before a request is sent, or, for a worker `call`, as its `err` reply, so it unwinds through the foreign code back to V2 with its kind.

```rust
fn request(&mut self, s: SessionId, f: Frame, at: Span) -> Result<Value, Flow> {  // init, load, module, call, op
    let _level = self.xdepth.enter(at)?;                     // RecursionError past 64; left on every path
    let id = self.send(s, f)?;                               // queued releases first, not the frame's own handles (11.2); size check (9.1)
    loop {
        match self.read_frame(s)? {                          // EOF → EngineCrashed, deadline → EngineTimeout (8.5, 8.6)
            Frame::Ret { id: r, value, .. } if r == id => return self.wire(s).decode(&value).map_err(|w| self.fatal(s, w)),  // 10.1
            Frame::Err { id: r, error } if r == id => return Err(self.foreign_error(s, error)),
            Frame::Call { id: c, target, args, kwargs } => {
                self.watchdog.pause(s);                      // the worker waits on us; it is not computing (8.5)
                let res = self.serve_call(s, target, args, kwargs);  // enters a level; may re-enter request()
                self.watchdog.resume(s);
                let reply = self.reply_frame(c, res);        // an encode failure becomes err MarshalError
                self.send(s, reply)?;
            }
            Frame::Out { stream, text } => self.out.write(stream, &text),      // 9.9
            Frame::Release { fns, .. } => self.release_fns(s, &fns),           // 11.8
            other => return Err(self.fatal(s, other.describe())),              // 9.11
        }
    }
}
```

An `exit()` inside a served call sends no reply to the pending `call`: the process shuts down (6.5), and the waiting worker gets `bye` like every other session (9.10).

**The adapter side** (its loop `serve(wait_id)`, its request stack and the stubs that send `call`) is 14.2's.

**User code cannot end the worker.** Request handlers catch everything (Python `BaseException`, any thrown JS value) and reply `err`: `sys.exit()` becomes a `ForeignError` with `foreign_type: "SystemExit"` and the message `sys.exit(3) can't end V2's Python engine; raise an error instead, or end the program with V2's exit(3)` (the argument's `repr`, none for no code), and JS `process.exit()` throws an `Error` worded the same way, `process.exit(3) can't end V2's JavaScript engine; throw an error instead, or end the program with V2's exit(3)` (14.3, 14.4). Adapters ignore SIGINT and SIGBREAK; the host owns interrupts (6.5).

### 9.8 When a worker may call V2

**Only while V2 is waiting on this engine.** A stub may send `call` only while the innermost entry on the adapter's stack is a host request that is executing: during a synchronous call, while that request's coroutine or Promise is awaited, or during a nested request served inside a stub. A callback stored earlier stays valid and may be called during any later request. A timer, event handler, thread or `__del__` that fires while the worker is idle, or while it is blocked in a stub (V2 is computing then), gets a `V2Error` (type `Error`) raised in the foreign code, and nothing is sent: `V2 function 'progress' called outside an active V2 call: Python called it while V2 wasn't waiting on Python (a thread, a timer or a finalizer after its call returned); call V2 from the code V2 called, and return to V2 when done` (JavaScript: `…: JavaScript called it while V2 wasn't waiting on JavaScript (a server, timer or event after its call returned); keep V2 in a call while it serves, as in an exported async function that resolves when the server should stop`) (DOCS 8.4). In JavaScript, timers and event handlers run only right after a call returns (the event-loop turn the adapter yields after every outermost reply, 14.4), or while V2 is waiting on a JS Promise. One that calls V2 right after a call fails; one that fires while V2 awaits a JS Promise is allowed.

**Only from the dispatch thread.** Python stubs compare `threading.get_ident()` with the dispatch thread's id: `V2 function 'progress' called from another thread: Python may call V2 only from the thread V2 called it on: do the threaded work in Python and call V2 with the results` (a thread pool's `map` over a V2 function, for one; still a `V2Error` of type `Error`, as above). JS stubs always run on the main thread, because functions cannot be sent to `worker_threads`.

The host cannot enforce either rule: it reads a worker's frames only while a request to that worker is pending.

**Awaiting (the async rule).** The result of a `call`, and the completion value of a `load` even with `want_value: false`, are awaited when awaitable (`inspect.isawaitable`; a thenable). `op` results never are.
- **py:** `loop.run_until_complete(x)` on the session's event loop (one per session, created on first use), at any depth. If an event loop is already running in the thread, the reply is `err` type `Error`: `'fetch' returned a coroutine while another coroutine is being awaited; call it directly from V2` (for a block's value: `the block's value is a coroutine while another coroutine is being awaited in this thread`).
- **js:** only a request served from the idle listener (stack depth 1) is awaited, and the event loop keeps running meanwhile (14.4). Inside a stub's synchronous wait the event loop cannot run, so the reply is `err` type `Error`: `'fetch_json' returned a Promise during a nested cross-language call; only an outermost call can be awaited: await it in JavaScript itself, or return a value, not a Promise` (DOCS 8.4; for a block's value: `the block's value is a Promise during …`).
- Before either error reply the adapter calls `coro.close()` or attaches a no-op rejection handler, so no "never awaited" or unhandled-rejection warning appears.

### 9.9 Output and worker stdio

**Stdio contract** (implementation: 14.3, 14.4). Before any user code runs, the adapter keeps user code off the protocol stream:
- **Protocol descriptors.** The protocol moves to private non-inheritable descriptors, then fd 0 points at the null device and fd 1 at fd 2. Python does this with `os.dup` of fds 0 and 1, then `dup2` of the null device to fd 0 and of fd 2 to fd 1 (14.3). Node has no `dup2`: on Unix the host starts it with the protocol on two pipes of its own, whose numbers are in `V2_VEP_FDS` (the adapter removes it), and with fd 0 on the null device and fd 1 on the host's stderr (`EngineSpec.protocol_fds`); on Windows the protocol stays on fds 0 and 1. Either way it writes frames with a saved `fs.writeSync` and reads with `fs.readSync`, and wraps everything else that reaches fds 0 and 1: `process.stdout`, `console` and `fs` writes to fd 1 are captured, `process.stdin` and `fs` reads of fd 0 are replaced, and `child_process` stdio is rewritten (14.4). An adapter for another language without `dup2` does the same as Node.
- **Language-level output** (`print`, `sys.stdout.write`, `console.log`, `process.stdout.write`) is sent as `out` frames, in program order; output a request produces goes out before that request's `ret` or `err`.
- **Fd-level output goes to stderr:** C extensions, `os.write(1, …)`, and `os.system`/`subprocess`/`child_process` children that inherit stdout. It isn't ordered with language-level output, which goes the longer way (an `out` frame, then the host's write), so it can come before what the block printed earlier (DOCS 4.6). A forked Python child (`multiprocessing`'s default on Linux) is one of them: an `os.register_at_fork` hook closes the protocol descriptors in it, and from then on the adapter's `sys.stdout` and `sys.stderr` (also one a library saved before the fork, such as a logging handler's) write to fds 1 and 2, a whole line per write (what is left at a flush, or past 64 KiB), so children printing at once, a `Pool`'s, don't split each other's lines, the line held under the stream's lock, so what its threads print comes out once, none lost or doubled (the fork hook makes the lock anew, as a thread may hold it at the fork, and drops a line the parent, itself a forked child, held), nothing is sent at exit, calls into V2 are refused, and an error that reaches the adapter ends the child as it would end a Python script (printed without adapter frames, exit 1, or `sys.exit`'s code); a request that ends normally ends it with 0.
- **Warnings.** A warning whose location is the adapter's own file (a library's `warnings.warn(…, stacklevel=2)` about a call V2 made, matplotlib's missing glyphs) is shown as `UserWarning: <message> (in a call from V2)`, not with the adapter's temp path and line (`warnings.showwarning` wrapped); others as Python shows them.
- **Stdin.** Language-level stdin is replaced by a stream whose reads raise (14.3); fd-level and child reads see EOF. On Unix `getpass.getpass` is replaced too: it raises `OSError: getpass can't ask in a block: …: ask in V2 with input(prompt, echo: false) and pass the answer in as a block arg` at once, where Python's own would warn that the input may be echoed and print its prompt before failing (the worker has no terminal, 8.3); on Windows it reads V2's console (DOCS 4.6). Python's `sys.stdin.fileno()` is 0 and `sys.stderr.fileno()` 2; `sys.stdout.fileno()` raises `UnsupportedOperation`, since V2's stdout has no descriptor in the worker (fd 1 is stderr, and saying 1 would send console-aware libraries such as click there), and its message is for the caller (the calling frame's module): to `subprocess` or `os` (`subprocess.run(…, stdout=sys.stdout)`) it says what to write instead, `fileno: a block's stdout is V2's and has no file descriptor: leave stdout= out (the output then goes to stderr), or capture it (capture_output=True, text=True) and print(r.stdout), or use V2's exec([…])`; to any other (`curses.wrapper`, a direct call) `fileno: a block has no terminal of its own: full-screen libraries such as curses can't run in a block`. The user's stdin belongs to V2's `input()` (DOCS 4.6).
- **Terminals.** A block sees where V2 writes its output, from `init`'s `terminals` (9.3), not the worker's fds 1 and 2 (both V2's stderr; in Node on Windows fd 1 is the protocol's pipe, below). py: `sys.stdout.isatty()` and `sys.stderr.isatty()`, `os.isatty(1)` and `(2)` (other fds as the OS says), `os.get_terminal_size(1)` and `(2)` (`OSError` ENOTTY for a stream that isn't a terminal) and `shutil.get_terminal_size()` (`COLUMNS` and `LINES` first, as shutil's own, then V2's stdout's size, else its fallback) answer for V2's stdout and stderr. shutil is changed when a block first imports it (`AfterImport`, a finder first on `sys.meta_path` that runs a function on a module once it has run; `after_import`), as importing it at startup (bz2, lzma) cost every run's first block about 2 ms on Linux and 8 ms on Windows. On Windows colorama's `ansitowin32.winapi_test` answers False the same way, so colorama (whose init tqdm, click and others call) passes a block's escape sequences through, or strips them when the stream isn't a terminal, instead of turning them into console calls on the worker's handle out of order with the text. js: `process.stdout` and `process.stderr` of a terminal get `isTTY`, `columns`, `rows` and `tty.WriteStream.prototype`'s `getColorDepth`, `hasColors`, `cursorTo`, `moveCursor`, `clearLine`, `clearScreenDown` and `getWindowSize`, which write their escape sequences into that stream's output (`seeTerminal`); `tty.isatty(1)` and `(2)` answer for V2's stdout and stderr (ESM imports too: `Module.syncBuiltinESMExports`). So libraries that decide on colors and live displays (chalk's supports-color, rich, ora, cli-progress, `console.log` of an object) decide as in plain Python or Node there. Where the bytes go doesn't change (above). A resize isn't passed on: the size is the one at the worker's start. On Windows the host turns on `ENABLE_VIRTUAL_TERMINAL_PROCESSING` for the consoles of its stdout and stderr at startup (`Watchdog::start`, `console_vt_on`) and puts their modes back at every exit (6.5), so those escape sequences render in conhost.
- **Remaining gap.** In Node on Windows, code that bypasses the wrappers (a native addon) can still write fd 1 and corrupt the stream (fatal `ProtocolError`, 9.11), or read fd 0 and steal protocol bytes.

**Adapter side.**
- Output is buffered and flushed (as `out` frames, by 14.2's `send`) at: a write ending in `\n`, the size threshold, an explicit flush, right before any other frame, and at `bye`. JavaScript has no flush, so during a request its writes go out at once, as Node's own stdout does. `out` text is sent in chunks below the frame limit. Queued releases go out as one `release` frame just before the next frame.
- An adapter writes frames only while it is executing a host request or handling `bye`; during a request any thread may flush `out` under the send lock.
- Output produced while idle (Python non-dispatch threads, signal handlers, JS timers) stays buffered and goes out with the next frame. Once 1 MiB of it is buffered, a Python non-dispatch thread that writes more waits until the next request starts.
- Lone surrogates in output become U+FFFD (10.4). Output still unfinished when a request answers or calls (a JS write ending in a high surrogate, bytes written to Python's `sys.stdout.buffer` that are not a whole UTF-8 sequence) goes out then as U+FFFD, not with a later request's text; so do held bytes before a text write to the same stream.

**Host side.**
- An `out` frame may carry `"error": true`: its text reports an error no V2 call could raise (a JS `uncaughtException` or `unhandledRejection`, an exception in a Python thread or `__del__`, 0.1), printed while `sys.stderr` is the adapter's own. The host prints it as any stderr text and remembers it: a run that would end with 0 ends with 1 (6.5).
- `out` text for `"stdout"` goes through `Out`, the writer of V2's `print` (5.10); `"stderr"` text goes to the host's stderr after `Out` is flushed (a closed pipe stops the run, 6.5; another failure is ignored, as for V2's diagnostics). A failed write to stdout (other than a closed pipe, which stops the run, 6.5) is raised as `print`'s `IOError` when the request that wrote it gets its `ret`, so a program whose reader has gone stops there like one using `print`; if that request gets an `err`, that error wins. The worker can't be stopped mid-request, so a block runs to its end first.
- While a worker runs, the host is blocked in `request()` for it and handles its frames in arrival order, so block output and V2 output appear in program order, also when redirected (DOCS 4.6). The host flushes `Out` before writing any frame, before `input()` blocks, and before it waits for a worker's next frame when it has handled every byte read so far, so a file or a pipe gets a long block's output as it comes (a chatty block costs a flush per 64 KiB read, not per line). A closed pipe there stops the run; another failure may be an earlier request's output, so it waits for the next write (the startup of another engine inside a callback must not take it).
- Idle output appears before the worker's next frame; its position relative to V2 output printed in between is not guaranteed.

### 9.10 `release` and `bye`

**`release`** carries refcount decrements (ch. 11).
- `h` (host→worker) lists handle ids the host dropped; `fn` (worker→host) lists callable ids whose stubs died. An id appears once per dropped reference. No reply; applied at once; unknown ids are ignored. Sessions without the `handles` cap never get `release`.
- Neither side writes it from a finalizer or on its own: finalizers only append to a queue, which goes out as one `release` right before that side's next frame to the other; the host holds back a release for a handle that frame names (11.2).

**`bye`** is sent on every orderly exit path (end of program, `exit()`, uncaught error; 6.5, 8.7), possibly while the worker is blocked in a stub; interrupts kill workers instead. The adapter flushes its output, runs the language's exit handlers, flushes what they wrote, and exits with status 0 (on Windows it ends itself after the `exit` handlers, without Node's teardown, whose libuv assertion can fire while a `fetch` winds down; the host ignores the status of a worker it said `bye` to) without running user `except`/`finally` code:
- **Python at depth 0:** `sys.exit(0)` from the main loop, so normal finalization and `atexit` handlers run.
- **Python inside a stub:** `atexit._run_exitfuncs()`, a flush, then `os._exit(0)`; raising `SystemExit` there would run through user code.
- **Node at depth 0:** what is still waiting runs first, as Node runs it after a script: the adapter stops reading its input, and when the event loop is empty (`beforeExit`) it ends with the saved `process.exit(0)`. Timers that keep it busy are cut off by the grace period (8.7), as a Python thread that doesn't end is; a V2 function called then raises 9.8's error, as between calls, and an error printed then ends the run with exit code 1 (9.9).
- **Node inside a stub:** the saved `process.exit(0)` at once.

After `bye` the host drains the worker's stdout for the grace period, forwarding `out` frames and ignoring everything else (8.7).

### 9.11 Malformed input and `ProtocolError`

From `hello` on, every stdout line must parse as a JSON object whose `t` is a string; there is no stray-output recovery. A fatal protocol error kills the worker and marks the session Dead (8.2). The current operation raises `ProtocolError: engine 'py' broke the protocol: <what> (got: <first 80 bytes, escaped>)`; later uses raise `EngineCrashed` (8.2).

**Fatal:**
- a line that is not a JSON object, invalid UTF-8, a lone-surrogate `\u` escape (serde_json rejects them), or a line over 64 MiB;
- an invalid first frame, or `vep` ≠ 1 (9.3);
- an unknown `t`, or a required member that is missing or has the wrong type;
- a `ret` or `err` that does not answer the innermost host request;
- a frame a worker may not send: `init`, `load`, `module`, `op`, `bye`, a `call` with a `unit`/`h` target, a `call` without the `reverse` cap, an `h` tag without the `handles` cap;
- an unknown or malformed `$v2` tag, an `int` tag outside i64, or value nesting deeper than 256 (10.5).

**Not fatal:** a worker `call` with an unknown `route`/`fn` id (the host answers `err`); an adapter's `err` of type `ProtocolError` (raised in V2).

### 9.12 Why it cannot deadlock

- **One thread of control** spans all processes: exactly one party computes at a time, and every other party in the call chain is blocked in its loop, reading its input.
- **Every write has a reader.** The host, the only router, writes only to workers that are reading (idle, or blocked in a stub's `serve`, 14.2), and a worker writes only while executing a request, when the host is in `request()` reading it (9.9). Two cases only wait: js's post-reply event-loop turn (14.4), and a Python thread that flushes `out` while the host serves one of that worker's calls (14.3). A hang inside user code is not a deadlock and is covered by `call_timeout` (8.5).

---

## 10. Codec and marshaling

### 10.1 One codec

`codec/` is the only JSON implementation (0.1): one serde_json value model in two modes, the plain mode (`codec/json`) and the VEP mode with the `$v2` tags (`codec/vep`). It serves VEP frames, `json_parse`/`json_stringify` (5.10), the JSON of reverse calls from compiled blocks (15.9) and `V2_ARG_*` values (17.4). It wraps `serde_json` with three features:
- `preserve_order`: objects keep insertion order, like V2 dicts; with duplicate keys the last value wins.
- `float_roundtrip`: exact float parsing.
- `unbounded_depth`: serde_json's own limit of 128 is below V2's limits, so the codec calls `disable_recursion_limit()` and bounds nesting itself. Before parsing, a byte scan rejects input whose bracket nesting outside strings exceeds 1,000 (fatal `ProtocolError` on the VEP path, `ValueError` in `json_parse` per 5.10, `MarshalError` for compiled-block JSON), which bounds the parser's recursion. Conversion then enforces the container limit of 256 on the VEP and compiled paths (10.5). So `json_parse` accepts nesting up to 1,000, and `json_stringify` has the same bound: encoding a value nested deeper is a `ValueError`.

`arbitrary_precision` is not enabled, so serde_json turns an integer token above u64::MAX or below i64::MIN into an f64 while parsing, and the parsed value no longer shows it was an integer. The same byte scan therefore also checks each number token outside strings: in `json_parse`, a token with no `.`/`e`/`E` whose value is outside i64 is `ValueError: invalid JSON at line L column C: integer does not fit in 64 bits` (5.10). On the VEP and compiled-block paths such a token decodes as a `Float` per 10.2, as serde already does; a serde `u64` above i64::MAX becomes a `Float` there too.

Base64 (standard alphabet, padded, strict decoding) is about 40 hand-written lines in `codec/vep`.

```rust
// codec/json: the plain mode
pub fn parse(text: &str) -> Exec<Value>;                              // json_parse
pub fn stringify(v: &Value, indent: Option<usize>) -> Exec<String>;   // json_stringify
// codec/vep: the VEP mode. A Wire is one session's side: whose handles and callables are in play
pub struct Wire<'a> { pub session: usize, pub tag: Rc<str>, pub caps: Caps, pub releases: &'a ReleaseQueue,
                      pub callables: &'a mut CallableTable, pub sm: &'a SourceMap,
                      pub route: &'a dyn Fn(u64) -> Option<Value> }   // route: an inner @import's target (13.6)
impl Wire<'_> {
    pub fn encode(&mut self, v: &Value, root: &str) -> Exec<Json>;   // MarshalError naming `root` (`args[0]`) and the path (10.5)
    pub fn decode(&mut self, j: &Json) -> Result<Value, String>;     // Err: what is wrong, for a fatal ProtocolError (9.11)
}
```

| Mode | Used for | `$v2` tags (10.3) |
| --- | --- | --- |
| VEP (`codec/vep`) | VEP frames | all |
| plain (`codec/json`) | `json_parse`, `json_stringify`, `V2_ARG_*`, `args_json` and results of compiled-block reverse calls (15.9) | none: `"$v2"` is an ordinary key. `bytes`, `func`, handles, errors and non-finite floats are rejected with an error that names the path (`json_stringify`: 5.10; `V2_ARG_*`: 17.4; compiled-block reverse calls: `MarshalError`, 15.9) |

- **Top-down decoding.** The content of a `dict` tag is never reinterpreted. Adapters do the same: plain `json.loads`/`JSON.parse` first, then a top-down walk; never `object_hook` or a `reviver`, which work bottom-up.
- **Failed encodes.** If an encode fails part-way, the callable-table counts it added (11.8) are rolled back; so are the counts of every `fn` tag in a frame refused as too large (9.1), since nothing was sent.

### 10.2 Numbers

| Step | Rule |
| --- | --- |
| host encodes `Int` | a JSON integer if abs(n) ≤ 2^53−1 or the mode is plain; else `{"$v2":"int","v":"<decimal>"}` |
| host encodes `Float` | finite: serde_json's shortest round-trip text, which always has a `.` or an exponent (`1.0`, `0.1`, `1e+21`, `-0.0`), so `json_stringify(2.0)` is `"2.0"`; NaN/±∞: the `float` tag (serde_json alone would write `null`) |
| host decodes a literal | no `.`/`e`/`E` and fits i64 → `Int`; no `.`/`e`/`E` but outside i64 → `Float`, the nearest double (`json_parse`: `ValueError` from the 10.1 scan, 5.10); anything else → `Float`. serde_json reads `-0` as the float `-0.0` |
| host decodes an `int` tag | parse as i64; out of range → fatal `ProtocolError` |

- Adapters for languages with unbounded integers send integers outside i64 as handles, never as bare literals, which the host would read as floats.
- The py adapter tags non-finite floats, then encodes with `allow_nan=False` as a safety net.
- So the int/float kind survives every crossing: `1.0` stays a float in Python. JavaScript has only `number`, so an integral JS number that fits i64 arrives as `int` (DOCS 9.1).

### 10.3 `$v2` tags

An object with a `"$v2"` member is a tag in a value position: `args`, `kwargs` values, `value`, export and `op` values, and anything nested inside them. Frame envelopes are never tags.

| Tag | Members | Meaning |
| --- | --- | --- |
| `h` | `id`, `type`; W→H also `callable` (`true` or absent, 11.1) | a handle (ch. 11). Worker→host: a new reference. Host→worker: only to the owning session; a callable handle goes to another session as an `fn` tag (11.8) |
| `fn` | H→W: `id`, `name`. W→H: `id` or `route` | a V2 callable or another session's callable handle (11.8); `route` marks an inner `@import`'s route stub (13.6, 13.7) |
| `int` | `v`: decimal string | V2 ints beyond ±(2^53−1) going out; JS `BigInt`s within i64, and integral JS `number`s beyond ±(2^53−1) but within i64 (10.7), coming back |
| `bytes` | `b64` | `bytes`, as standard padded base64 |
| `float` | `v`: `"nan"`, `"inf"` or `"-inf"` | a non-finite float |
| `error` | `type`, `message`, `data`, `lang`, `foreign_type`, `file`, `line`, `col`, `trace` (strings), `cause` (tag or `null`); in an `err`'s `v2`, also `ref` (9.6), which adapters keep and decoders ignore | a V2 error value |
| `dict` | `v`: object | a real dict that has a `"$v2"` key: the keys of `v` are literal, its values are decoded normally |
| `buffer` | reserved | not sent in VEP/1; only to a side whose `hello` or `init` announces the cap `buffer` once it is defined (9.3) |

An unknown tag or a missing member is a fatal `ProtocolError` on the host (9.11); adapters answer it with `err` `ProtocolError`.

### 10.4 Strings

Host strings are always valid UTF-8. Foreign strings can hold lone surrogates: in Python any code point U+D800–U+DFFF (for example `surrogateescape` file names), in JS an unpaired UTF-16 half. Handling depends on where the string appears:
- **Values** (arguments, results, attribute values, dict keys): `MarshalError: string contains an unpaired surrogate (U+DC80) at index 12`. Python detects it through the `UnicodeEncodeError` of `s.encode("utf-8")`; JS through `s.isWellFormed()`.
- **Hints.** A `MarshalError` V2 raises or receives gets one, uncaught: for a value too large (9.1), `write it to a file and pass the file's path, or send it in parts`; for a lone surrogate in Python, U+DC80 to U+DCFF (what `surrogateescape` makes of bytes that aren't UTF-8), `the text holds bytes that aren't UTF-8, which Python keeps as U+DC80 to U+DCFF (file names, environment variables and program output can): send it as bytes, os.fsencode(s), or with those bytes replaced, s.encode("utf-8", "surrogateescape").decode("utf-8", "replace")`, another one `replace it in the block: s.encode("utf-8", "replace").decode()`, and in JavaScript `replace it in the block: s.toWellFormed()`; for a `to_v2()` key of another type (a dict keyed by dates; a key of what the object's `to_dict()` gave has a block in its Python message, 11.5), `make its keys strings in the block, as in @py(h) { {str(k): v for k, v in h.items()} }` (JavaScript: `@js(h) { Object.fromEntries([...h].map(([k, v]) => [String(k), v])) }`).
- **Text** (`out`, error messages, `text`, traces, `str` op results, the `type` of `h` tags): each lone surrogate becomes U+FFFD (Python `re.sub("[\ud800-\udfff]", "\ufffd", s)`, JS `s.toWellFormed()`), so printing and error reporting never fail.

### 10.5 Containers

- **Copying.** Lists and dicts are copied in both directions, so mutations never propagate (DOCS 9.1). Substructure shared inside one value is copied once per occurrence.
- **Dict keys** on the wire are strings. Whoever encodes a dict that has a `"$v2"` key wraps it in the `dict` tag.
- **Depth limit: 256 nested lists, dicts and errors** (an error's data and cause are one level below it). Host encode fails with `MarshalError: value nested deeper than 256 levels`; host decode fails with a fatal `ProtocolError`; adapter encode sends the container or error at depth 257 as a handle.
- **Cycles.** Host encode tracks the `Rc` pointers on the current path; a revisit fails with `MarshalError: cannot send a list that contains itself`. Adapter encode tracks object identities on the current path and sends a revisited container as a handle: Python `a = [1]; a.append(a)` arrives as `[1, <handle>]`.
- **Error paths.** Errors inside containers name the path, e.g. `… (at args[0]["when"])`; an adapter's error about a whole `value` (a block's value, a result) names none. A path of more than 12 steps shows its first 4 and last 4: `args[0][0][0][0]…(249 more)…[0][0][0][0]`.

### 10.6 Python

**V2 → Python:** `null`, `bool`, `int`, `float`, `str`, `bytes` become `None`, `bool`, `int`, `float`, `str`, `bytes`; `list` and `dict` a new `list` and `dict` in the same order; `func` a stub (11.8); a handle of this session its original object; `error` a `V2Error` instance, not raised.

**Python → V2**, checked in this order, with `type(x) is …` fast paths first:
1. `None` → `null`. `bool` → `bool` (before `int`, because `bool` subclasses `int`); numpy `bool_` (type module `numpy`, type name `bool_` or `bool`) → `bool`.
2. `int` and its subclasses (`IntEnum`), any `numbers.Integral` (numpy integer scalars register themselves) → `int(x)`; outside i64 → handle.
3. `float` and its subclasses (`numpy.float64`), numpy floating scalars (`numbers.Real` from module `numpy`) → `float(x)`, non-finite through the `float` tag. `Fraction`, `Decimal`, `complex` → handles.
4. `str` and its subclasses → `str`, after the surrogate check. Exactly `bytes` → `bytes`; `bytearray` and `memoryview` → handles.
5. Exactly `list` or `tuple` → `list`, and exactly `dict` with only `str` keys → `dict`, recursively. Subclasses (namedtuples, `OrderedDict`, `Counter`, custom lists) and dicts with other keys → handles, so `row.region` works on an `itertuples()` row (DOCS 9.2).
6. A V2 stub → `fn` tag. A `V2Error` → `error` tag.
7. Anything else → handle: functions, bound methods and classes (callable handles, 11.6), modules, sets, `datetime`, `DataFrame`, generators.

### 10.7 JavaScript

**V2 → JS:**
- `null`, `bool`, `str`, `float` become `null`, `boolean`, `string`, `number` (`1.0` and `1` are the same JS value). `int` becomes a `number` when abs(n) ≤ 2^53−1, otherwise a `BigInt` (from the `int` tag).
- `bytes` becomes a `Buffer`, `list` an `Array`, `func` a stub (11.8), `error` a `V2Error`; a handle of this session its original value.
- `dict` becomes an ordinary object whose prototype is `Object.prototype`. Its entries are own data properties defined with `Object.defineProperty`, never by assignment, so a `"__proto__"` key stays an ordinary key. It is not a null-prototype object, because those break `String(obj)`, template literals and `obj.hasOwnProperty`.
- Values are decoded in the adapter's realm, not the unit's (12.4): blocks use `Array.isArray(x)`, not `x instanceof Array`. JS lists integer-like keys first, in ascending order: `{b: 1, "2": 2}` comes back as `{"2": 2, b: 1}`.

**JS → V2**, checked in this order:
1. `undefined` and `null` → `null`. `boolean` → `bool`. A `Proxy` → handle, whatever it wraps: copying it would lose its traps.
2. `number`: NaN and ±Infinity → `float` tag. A number for which `Number.isSafeInteger(x)` is true is written as a JSON integer: `3` → `int`, `-0` (written `0`) → `int` 0. An integral number outside ±(2^53−1) but inside i64 (−2^63 ≤ x < 2^63) is sent as the `int` tag with `v: BigInt(x).toString()`, the double's exact value, so `2**60` arrives as `int` 1152921504606846976 (`JSON.stringify` would write `1152921504606847000`). Any other finite number is written by `JSON.stringify`, and the host reads it as a `float` (10.2): non-integral numbers (`3.5`) and integral numbers outside i64 (`1e20`, written `100000000000000000000`).
3. `bigint` within i64 → `int` tag, any other `bigint` → handle. `string` → `str`, after the surrogate check.
4. `Buffer` or any `Uint8Array` (`util.types.isUint8Array`, which works across realms) → `bytes`; other typed arrays → handles.
5. `Array.isArray(x)` → `list`, recursively; holes become `null`.
6. A plain object → `dict` over `Object.keys`; symbol keys are ignored. One with a JS function among its own data properties, enumerable or not, symbol keys included (a V2 function's stub doesn't count), is a handle instead: it carries behaviour, and copied, its methods would lose `this`. Plain means prototype `null`, or a prototype `p` with `Object.getPrototypeOf(p) === null` and `p.constructor.name === "Object"` (any realm; `instanceof Object` fails across `vm` contexts).
7. A V2 stub → `fn` tag. A `V2Error` → `error` tag.
8. Anything else → handle: functions and classes (callable handles, 11.6), symbols, `Map`, `Set`, `Date`, `RegExp`, class instances, other `Error`s, and Promises that are not awaited (9.8).

### 10.8 Named arguments

- Named arguments travel in `call.kwargs`; a spread `...xs` is expanded into `args` first. Block args are not kwargs: they travel in `load.args`.
- Before sending, the host checks the destination session's `kwargs` cap (9.3, 17.5). The py adapter has it and calls `f(*args, **kwargs)`. For js and ts functions (and custom engines without the cap), a non-empty `kwargs` raises `ArityError: js functions take positional arguments only (got 'color')` and nothing is sent. This applies to imported functions, handle calls and method calls alike.
- In the other direction, Python stubs forward keyword arguments as `kwargs`, and the host binds them to V2 parameter names like any named call (5.7). An unknown name is an `ArityError`, raised back into Python as a `V2Error`.

---

## 11. Handles

A handle is V2's reference to an object that stays inside a worker: V2 holds `(session, id)`, the worker holds the object. The host side is `engines/handles.rs`; the worker side is each adapter's handle table (14.2). Only sessions whose `hello` declares `handles` produce them (17.5). Native pointers from `extern` and compiled code are handles of a reserved native pseudo-session with their own rules (16.6); this chapter is about worker handles.

### 11.1 Worker-side table

```python
_handles = {}   # hid -> [obj, count, batch]   count = references the host holds; batch: 11.4
_by_obj  = {}   # id(obj) -> hid               identity map; safe because the table keeps obj alive
```

JS uses a `Map<number, {obj, count, batch}>` plus a `Map<any, number>` identity map; objects are keyed by identity, symbols and `BigInt`s by value.
- **Sending** an object reuses its live id or allocates the next one (per session, never reused), then does `count += 1`: every `h` tag sent is one host reference.
- **Releasing** does `count -= 1` per occurrence of the id; at 0 both entries are removed and the object can be collected. Unknown ids in `release` are ignored; an unknown id in an `op` or a `call` target is answered with `err` `ProtocolError: unknown handle id 7`.
- **`type`**: in Python, `module.qualname` of `type(obj)` without the `builtins.` prefix (`datetime.date`, `pandas.core.frame.DataFrame`, `int` for a big int); in JS, the constructor name, or `function`, `class`, `symbol`, `bigint`.
- **`callable`**: `true` in the `h` tag of a function-like object (functions, bound methods, classes, callable objects: Python `callable(obj)`, JS `typeof obj === "function"`); absent otherwise (11.6).

### 11.2 Host side and releases

```rust
pub type ReleaseQueue = Rc<RefCell<Vec<u64>>>;      // the session's queue (8.1)
pub struct HandleRef { pub session: usize, pub tag: Rc<str>, pub id: u64, pub type_name: Rc<str>,   // session: SessionId (8.1)
                       pub callable: bool,               // the `h` tag's `callable` (11.1, 11.6)
                       releases: Option<ReleaseQueue> }  // tag: the engine, for messages; type_name: `datetime.date`; None: a native pointer (16.6)
impl Drop for HandleRef {
    fn drop(&mut self) { if let Some(q) = &self.releases { if let Ok(mut q) = q.try_borrow_mut() { q.push(self.id) } } }
}
```

- **One `Rc` per tag.** Each decoded `h` tag creates a new `Rc<HandleRef>`, matching one count on the worker side; the host never deduplicates.
- **`Drop` only queues.** No code keeps the queue borrowed across other calls (the flush drains it inside one short borrow), so `Drop` is safe anywhere: mid-decode, during unwinding, at shutdown.
- **Flushing.** A session's queue goes out as one `release` frame right before the host's next frame to that session (9.10), so it can't fill the pipe while the worker writes its answer; but a release for a handle that frame names (an `h` tag, an `op`'s or a call target's `h`) waits for a later frame, since the frame may be its last holder, such as a V2 function's return value or a temporary block arg.
- **Dead sessions and shutdown.** Queued ids of dead or closed sessions are discarded; `bye` frees everything, so no releases are sent at shutdown.
- **Native pointers** (16.6) have no queue (`releases: None`), so dropping one queues and sends nothing. (The built `HandleRef` has a plain `ReleaseQueue` until native pointers exist, and no `callable` until roadmap 1.6.)

### 11.3 Operations

| V2 | Frame | Python | JavaScript |
| --- | --- | --- | --- |
| `h.name` | `op getattr` | `getattr(obj, name)` | `obj[name]`, a missing property is `null`. A function `f` that is not a constructor comes back as `new Proxy(f, {apply: (t, _this, args) => Reflect.apply(t, obj, args)})`, cached per `(obj, f)` in a two-level `WeakMap`: `let f = h.m; f()` keeps `this` like a Python bound method, `f`'s own properties stay readable (`chalk.red.bold`), and repeated reads give the same handle |
| `h.name = v` | `op setattr` | `setattr(obj, name, v)` | `obj[name] = v` |
| `h.m(a, k: b)` | `call {h, method}` | `getattr(obj, "m")(*args, **kwargs)` | `f = obj.m`; not a function → `TypeError`; a constructor → `Reflect.construct(f, args)`; else `Reflect.apply(f, obj, args)` |
| `h(a)` | `call {h}` | `obj(*args, **kwargs)` | a constructor → `Reflect.construct`; else `Reflect.apply(obj, undefined, args)` |
| `h[k]`, `h[k] = v` | `op getitem`, `op setitem` | `obj[k]`, `obj[k] = v` | a `Map`: `get`/`set`; else `obj[k]`, `obj[k] = v` |
| `len(h)` | `op len` → int | `len(obj)` | `obj.length`; else a numeric `obj.size`; else `TypeError("object has no length")` |
| `str(h)`, `print(h)`, `f"${h}"` | `op str` → str | `str(obj)` | `util.inspect(obj, {depth: 2, breakLength: Infinity})` for functions and for objects whose `toString` is inherited from a realm's `Object.prototype`; else `String(obj)` |
| `for x in h`, `to_v2(h)` | `op iter`/`next`, `op value` | 11.4, 11.5 | 11.4, 11.5 |

- **JS constructors.** A function `f` is a constructor when `Function.prototype.toString.call(f)` starts with `class`, or when `f.prototype` has own properties other than `constructor` (ES5 constructors such as Node's `EventEmitter`; natives such as `Map` and `Date`), except `Number`, `String`, `Boolean`, `Symbol` and `BigInt`, recognized in any realm by name and native code (each block has its own). Any other function is called plainly, so `function Point() {}` with no prototype methods is called without `new`.
- **Method calls.** In call position `h.m(...)` takes one round trip: the interpreter evaluates the callee to `Callable::HandleMethod{handle, method}` without a `getattr`, and calling it sends `call {h, method}`. Anywhere else, `h.m` is a `getattr`.
- **Results and errors.** Results are marshaled like any value: plain data is copied, anything else becomes a new handle. Exceptions become `ForeignError` with the foreign class (`AttributeError`, `KeyError`, `TypeError`, …) in `foreign_type`. Named arguments follow 10.8. `call` results are awaited (9.8), `op` results never.

### 11.4 Iteration

`for x in h` (DOCS 9.2):
1. The host sends `op iter {h, pairs?}` (`pairs: true` when the loop has two variables); the worker answers `{"it": <iterator handle>, "mapping": bool}`. `mapping` is true for a Python `collections.abc.Mapping` and a JS `Map` (`util.types.isMap`), whose iterator yields `[key, value]` pairs (`iter(obj.items())`, `obj.entries()`), and, with `pairs`, for a Python object (not a `str` or `bytes`) with a callable `items` (a pandas Series or DataFrame, whose own iteration gives values or column names). Otherwise the worker calls `iter(obj)` (Python) or `obj[Symbol.iterator]()` (JS; not iterable → `TypeError`). An async generator, which `for` can't step through, gets a hint from its type name (`handles.rs` `async_iter_hint`; JS names an object without a named constructor by its `Symbol.toStringTag`, `AsyncGenerator`): `an async generator can't be looped over in V2: collect it in JavaScript, as in let items = @js(h) { Array.fromAsync(h) } (V2 waits for the Promise)`, and for Python `… collect it with an async function that a block exports, as in async def collect(h): return [x async for x in h] (V2 waits for the coroutine it returns)`.
2. The host repeats `op next {h: it, n: 64}`; each answer is `{"items": [...], "done": bool}`.
3. The loop ends when `done` is true and the received items are used up. `break` or an error simply drops the iterator handle.

Over a **mapping handle** (`mapping` true), `for k in h` gives the keys and `for k, v in h` the key and the value, as for dicts. Over any other handle, one variable gets the item and two get the index and the item, as for lists. V2 never unpacks pairs, so a two-variable loop whose iterable is a call `x.items()`, `x.iteritems()`, `x.iterrows()` or `x.entries()` with no arguments, or `x.most_common(…)` (a Counter's pairs), that gives a handle or a list is `TypeError: V2 doesn't unpack pairs: for k, v in d.items() would give each pair's index and the pair`, hint `write for k, v in d: two variables loop over a mapping's keys and values, and over a Python object's items()` (`iterrows`: `loop over its rows with for row in df.itertuples() and read their fields, as in row.Index`; `most_common`: `loop with one variable and read its parts: for pair in c.most_common(…) { print(pair[0], pair[1]) }`), with `x` as written when it is one line of at most 40 bytes.

**Batching.** A `next` returns more than one item only if the iterator's `batch` flag is set. The adapter sets it at `iter` time when the source is an eager built-in container whose iteration runs no user code: Python `list`, `tuple`, `dict` and its views, `set`, `frozenset`, `str`, `bytes`, `range`, numpy `ndarray`; JS `Array`, typed arrays, `Map`, `Set`, strings, when their iteration methods are the built-in ones (a subclass may override them). Other iterators (generators, `map` objects, `itertuples()`) return one item per `next`, so foreign side effects such as prints stay in step with the loop body. Items are encoded at the depth they have inside the reply (two levels down, three for a mapping's pair, 10.5). A batch stops before an item that fails to encode or would take the reply past half the frame limit; that item comes first in the next `next`, so the items before it still arrive.

### 11.5 `to_v2(h)`

`op value {h}` converts the object to plain data inside the worker, recursively up to depth 256 (a container at level 257 is too deep, as for the host, 10.5); deeper nesting or a cycle is a `MarshalError`. At each node:
1. **Plain values** convert as in 10.6/10.7; V2 functions and error values held by the object go back as themselves.
2. **Containers** convert element by element: Python `list` and `tuple` (subclasses included, such as namedtuples), `set` and `frozenset` (→ list, in iteration order) and any `collections.abc.Mapping` (→ dict); JS arrays, plain objects, `Set` and typed arrays other than `Uint8Array` (→ list: a `Float64Array` of floats, a `BigInt64Array` of ints) and `Map` (→ dict, keeping the `Map`'s order). A mapping key that is a string stays; one that converts to `int` (within i64), `float`, `bool` or `null` becomes its V2 `str()` form, not the language's (5.10: `1` → `"1"`, Python `True`/`None` → `"true"`/`"null"`, Python `1e-05` → `"1e-5"`, JS `1e+21` → `"1e21"`); any other key, or two keys with the same form, is a `MarshalError`: `to_v2(): can't convert dict to V2 data (key of type tuple)`, `… (two keys become "1")`. In Python, a key of another type under the object's own hooks (step 3: a pandas Series or DataFrame indexed by dates) names the object instead, with a block that converts it: `to_v2(): a key of h has the type pandas.Timestamp: make plain data of h in a block first, as in @py(h) { __import__("json").loads(h.to_json(date_format="iso")) }` for a pandas object (its type's module, with a callable `to_json`), else `{str(k): v for k, v in h.to_dict().items()}` (what the level-0 hooks gave, as in step 4); deeper than what they gave, `a key inside h has the type …` (pandas: the same block; else `make plain data of h in a block first, with str(k) for each such key k`).
3. **Hooks.** Otherwise a callable `to_dict` is tried first, then a callable `tolist`, and the result converts from step 1 at the same level (an object whose `size`, an int, times 6 for `to_dict` or 2 for `tolist` passes the 64 MiB limit, a big DataFrame or array, is `MarshalError: value too large to send to V2 (12000000 items, more than 64 MiB)` before its hook runs, which would hold it all as Python objects first), with the object on the cycle path (a hook that returns it is a cycle). More than 256 hooks open at once is too deep, so a chain of hooks that return new objects ends. So a pandas `Series` becomes a dict (DOCS 17.2; with the default index `{"0": …, "1": …}`) and a numpy array a list.
4. **Anything else** is `MarshalError: to_v2(): can't convert datetime.date to V2 data: make plain data of it in a block first, as in @py(h) { h.isoformat() }`, with what works for the object: `h.isoformat()` (it has `isoformat`), `list(h)` (it iterates), `vars(h)` (it has fields), else `str(h)`; js: `h.toISOString()` for a `Date` (`util.types.isDate`: a block's `Date` is its own context's, 14.1, where `instanceof Date` fails), else `({ ...h })`: a `datetime`, a class instance without hooks, a `Decimal` or `Fraction`. A node that isn't the object itself (an item of it, or what its hook gave) is named so, with its type after "has the type" (no article to pick: `uuid.UUID`, `URL`), and a block that converts the whole object as fits it: `to_v2(): an item of h has the type datetime.date: make plain data of it in a block first, as in @py(h) { [x.isoformat() for x in h] }` for an item of a list, tuple or set at level 1, `{k: v.isoformat() for k, v in h.items()}` for a mapping's whose keys are all strings, else `{str(k): v.isoformat() for k, v in h.items()}` (a block's dict with other keys comes back as a handle, not data; `str` gives an int V2's form, `None` its Python one); js: `[...h].map((x) => x.toISOString())`, for a `Map`'s `Object.fromEntries([...h].map(([k, v]) => [k, v.toISOString()]))`, for an object's `Object.fromEntries(Object.entries(h).map(…))`. What level-0 hooks gave stands in for `h`: `to_v2(): h.to_dict() has the type datetime.date: … { h.to_dict().isoformat() }`, `[x.isoformat() for x in h.tolist()]`. Deeper, or what an item's hook gave: `to_v2(): an item inside h has the type datetime.date: make plain data of h in a block first, with x.isoformat() for each such item x`. When the object is pandas's or numpy's (its type's module, with a callable `astype`), the block makes text of it: `h.astype(str).tolist()` (a `Series`, an `Index`, an array, a `datetime64`); for a `DataFrame` (no `tolist`) `import json; json.loads(h.to_json(date_format="iso"))`, which keeps its numbers and its shape. An int outside i64 is `to_v2(): can't convert int to V2 data (it needs more than 64 bits): pass it as text, as in @py(h) { str(h) }` (js: `bigint`, `@js(h) { h.toString() }`).

A hook that raises gives a `ForeignError`. `to_v2(x)` on a non-handle returns `x` unchanged (5.10); on a native pointer it is a `MarshalError` (16.6).

### 11.6 Callable handles, identity, diagnostics

- **Callable handles.** Foreign functions, bound methods, classes and callable objects that come back as values (call results, `getattr`, attributes of a module handle) are handles marked `callable` (11.1): `type(h)` is `"handle"`, and they are called as `h(...)`. Names brought in with `@import` are `func`s (Proxies, 13.4), not handles. Sent to another session, a callable handle goes as an `fn` tag whose callable-table entry calls it through its own session (11.8); other handles stay with their session (11.7).
- **Equality.** `h1 == h2` is true exactly when session and id match. The identity map (11.1), together with removal only at count 0, makes this mean "the same foreign object" for all live handles. A handle equals nothing else.
- **No round trips.** `type(h)`, truthiness (always true) and diagnostics never contact the worker. Diagnostics show `<py datetime.date>`, built from `type_name`; a class a block defines is named as written, without the block's module (`<py Counter>`, `to_v2(): can't convert Box to V2 data`).

### 11.7 Invalid uses

| Use | Error |
| --- | --- |
| a non-callable handle sent to another session (in args, kwargs, block args, or inside lists/dicts) | `MarshalError: this handle belongs to engine 'py' and can't be passed to 'js' — convert it with to_v2(h) first`, plus the path (10.5); for what a function another language called returned (`at the result`), whose code has no `to_v2`, `— a Python object can't go to JavaScript: have the function return plain data instead (a dict or a list; in V2, to_v2(h)), or the object's methods, which go as functions`; for a type `to_v2` can't convert, the block that can instead (`HandleRef::plain_way`): Python's `datetime`, `date`, `time` and pandas `Timestamp` `— a datetime.datetime isn't data: make plain data of it in a block, as in @py(h) { h.isoformat() }`, `Decimal`, `UUID`, `Fraction` and `pathlib` paths `str(h)`, `timedelta` `h.total_seconds()`, JavaScript's `Date` `@js(h) { h.toISOString() }`; an int beyond 64 bits `— an int beyond 64 bits isn't V2 data: pass it as text, as in @py(h) { str(h) }` (a `bigint`: `@js(h) { h.toString() }`). The one exception is an error answering a worker's `call`: a handle in its data or cause goes as a function, so that foreign code that doesn't catch the error sends it back whole (9.6); called, that function raises V2's error for calling a handle that can't be called |
| a handle as an argument of a compiled export or `extern` function; returned to compiled code by a reverse call | `MarshalError: cannot pass a py handle (datetime.date) to native code; convert it with to_v2(h) first` (16.5); `MarshalError` (15.9) |
| any use after its session died | `EngineCrashed` (8.2) |
| arithmetic, ordering, `in`, slicing | `TypeError` with the hint `— do it inside a block of its language, as in @py(h) { h + 1 }, or call a method on it; to_v2(h) converts one that holds plain data (lists, dicts, numbers, text)` (5.2, 5.3), the example in the handle's own language and for the operation: `h + 1` (the operator used), `h < x`, `-h`, `x in h` (`h.includes(x)`), `h in text`, `h[1:3]` (`h.slice(1, 3)`), `sorted(h)` (`[...h].sort()`), `min(h)` (`Math.min(...h)`); for a handle to an integer past 64 bits (`type_name` `int` or `bigint`), which `to_v2` can't convert: `— this integer needs more than 64 bits, which V2's int doesn't have: keep the arithmetic in the block` |
| a member of an imported function or class (a Proxy, which is a function in V2) | for a block's export, `TypeError: func has no method 'hello' — an imported class is only a function here; @export a function from the block that uses C.hello`; for a package's, `… — an imported function or class brings none of its own properties; reach them through the module: @import py.pkg as m, then m.C.hello` (the package quoted unless a plain name; a local file: `@import its file as a module, as in @import js."./file.js" as m`; for a name imported as `default`: `then m.hello (for an ES module's default export, m.default.hello)`) |
| a handle as a dict key (`d[h]`, `{[h]: 1}`) | `TypeError: dict keys must be str, got handle` |
| `json_stringify(h)` | `TypeError` (5.10) |

### 11.8 V2 callables inside workers

When the host encodes for session S a V2 `func` (a closure, a builtin, a `Proxy` of another session, a `Native` function or a `HandleMethod`) or a callable handle of another session (11.6), it registers the value in S's callable table and sends `{"$v2":"fn","id":n,"name":"log_line"}`; a closure's tag also has `"params"`, the names of its parameters before the first default, and `"more": true` when defaults or a rest parameter follow, so its stub looks like it to code that counts parameters: in JS the stub's `length` is their number (Express calls a 4-parameter function as error middleware; lodash's `curry` waits for that many arguments), and in Python its `__signature__` (made on first use, the only time the adapter imports `inspect`) is `(err, req, res, next)`, with `*args, **kwargs` after them for `more` (and alone when a name can't be a Python parameter). A `Proxy` of S itself goes as `{"$v2":"fn","unit":u,"name":n}`, which the adapter decodes to the current target of that name (in JS, 13.7's `Proxy` for a function read from a module object), so a function or class imported from a language is the real thing back in that language, with no hop per call. A session without the `reverse` cap gets a `MarshalError` instead (17.5).

- **Reuse.** The table maps id → (value, count) and the value's `Rc` address → id. Sending the same `Rc` (a `Callable`, or a callable handle's `HandleRef`) again reuses its id and does `count += 1`. The table holds the `Rc`, so the address key stays valid.
- **Worker stubs.** The worker keeps one stub per id in a weak map (Python `weakref.WeakValueDictionary`, JS `Map<id, WeakRef>`), so one V2 function is one foreign object (`f is g`); its name is the V2 name (a callable handle's is its diagnostic form, `<py function>`, 11.6). Each stub owns a cell `{id, n}` counting the `fn` tags it absorbed.
- **Release.** When a stub is collected, its finalizer (Python `weakref.finalize`, JS `FinalizationRegistry`) only appends the id `n` times to the release queue, which goes out as `release {fn: [...]}` before the worker's next frame. A stub encoded into a frame stays referenced until that frame is written, so its release never goes out ahead of a frame that names it. The host subtracts once per occurrence and drops the entry at 0. A stub that dies just as its id is sent again is safe, because the new stub gets its own cell.
- **Calls.** Calling a stub sends `call {"fn": n}` (9.5, 14.2). The host calls the value (a callable handle: `call {h}` to its own session, 11.3), re-entering the interpreter, and encodes the result for S; a V2 error goes back as `err` with `v2` (9.6). A call from the js session to a V2 closure with no `...rest`, whether it came as a callback or through a route (`@import { f } from v2`, 13.6), drops the positional arguments past its parameters, as a JavaScript function ignores them (`xs.map(f)` passes item, index and array; DOCS 8.2); too few is still an `ArityError`, and calls from other sessions are bound strictly. A V2 closure a worker calls with fewer arguments than it declares (by position or by name, any session) gets, on that function's own `ArityError`, `hint: it was called with fewer arguments (Node's emit and callbacks often do): give x a default, as in func one(x = null)` (from the first one missing, each parameter without a default needs one: `give a and b defaults, as in func two(a = null, b = null, c = …)`; `func(…)` for an anonymous one; session.rs `fewer_args_hint`).
- **Passing back.** A stub passed to the host is sent as `{"$v2":"fn","id":n}` and decodes to the original V2 value; counts do not change. An inner `@import`'s route stub (13.7) is sent as `{"$v2":"fn","route":r}` and decodes to that route's target, or, while the route is `Pending` (and always for a `v2` route), to a `Callable::Route` that calls the route as 13.6 says. Routes live in `Interp.routes` (13.6), not in this table, and are never released. A same-session stub (13.7) is an ordinary worker function, so it arrives as a callable handle.

### 11.9 Leaks

- **Cross-process cycles are never collected.** A Python object that holds a V2 closure whose environment holds a handle to that same object keeps both alive until the session ends.
- **CPython** frees unreferenced stubs immediately through refcounting.
- **V8 finalizes late.** `FinalizationRegistry` callbacks run only after a garbage collection and an event-loop turn, so passing callbacks in a loop could grow the host's table without bound; the loop blocks in `fs.readSync` between requests (14.4), so the event loop does not turn on its own. The js adapter's idle turn and forced `gc()` (14.4) bound it.

---

## 12. Block execution

Host: `engines/units`; workers: the adapters (ch. 14). This chapter is the path every block takes, plus loading bridged
units; compiled blocks continue in ch. 15, run engines in 17.3.

### 12.1 Reaching a block

A block is an `EngineBlock` (4.3) whose `body` and `loc` come from 4.4 and 2.3; each execution of it is a **unit**, named
by its `UnitId`. Only definition blocks keep a state, in `Interp.units`: `Waiting` (12.7), `Loading`, `Loaded` or `Failed(err)`; an
action run leaves nothing on the host except its routes (13.6). Messages name a block with `block_name(b)`:
`block 'stats'`, or `@py block at main.v2:12` when it has no label; tags always print canonical.

A block whose code names a V2 variable fails in its language (Python `NameError: name 'n' is not defined`, JavaScript `ReferenceError: n is not defined`). When that name is visible in the block's scope (`env::lookup`) and isn't one of its args, the error gets the hint (2.4) `'n' is a V2 variable, and blocks don't see V2 variables: pass it in, @py(n) { ... }`; for a func of the file, `'helper' is a V2 function, and blocks don't see V2's names: import it in the block, @import { helper } from v2, or pass it in, @py(helper) { ... }`; for a function or handle the file imported, `'mean' is a name V2 imported, and blocks don't see V2's names: import it in the block, as its language does, or pass it in, @js(mean) { ... }`; for a V2 builtin, the language's own way: `print is V2's function, which blocks don't see: in JavaScript, write console.log(x)` (`len`: `x.length`; `read_file`: `require("fs").readFileSync(path, "utf8")`, in Python `open(path).read()`; `units.rs` `javascript_way`, `python_way`), else `… use JavaScript's own, or pass it in, @js(name) { ... }`.

A block is **reached** each time control executes its statement: a block inside a function is first reached by the first
call that runs it, one in a loop on the first iteration, and one in an `if` arm never taken is never reached. A
definition block (with `@export`) loads on its first reach, unless it waits (12.7); later reaches do nothing, or re-raise its stored load error
(step 1). An action block (no `@export`) goes through steps 2–8 on every reach, args included, each time in a fresh
namespace (bridged: a new unit, dropped after its reply; compiled: the program, built or reused, 15.5; run: 17.3). Inside `try` a load error is
catchable, so `try { @py probe { … @export { … } } } catch (e) { … }` is a probe; reaching it again re-raises the stored
error.

`exec_block(b, want_value)`:

1. **State** (definition blocks): `Loaded` or `Waiting` → return `null`, evaluating nothing, not even the args; a block that may wait and whose language isn't running goes to `Waiting` instead (12.7). `Failed(e)` →
   `ImportError: block 'stats' failed to load: <e's message>` at the block, with `e` as its `cause` (DOCS 4.3): no
   retry, no repeated side effects (an `@import` from it gets 13.3's `failed to load` too). `Loading` → `ImportError: block 'stats' is still loading (reached again during its own load)`. Otherwise set
   `Loading`.
2. **Engine:** `registry.resolve(tag)`, else `EngineUnknown` / `EngineNotFound` (ch. 7).
3. **Directive check:** a **run-engine** block (built-in or custom) with `@export` or an inner `@import` →
   `ImportError: @{tag} blocks cannot @export or @import — '{tag}' is a run engine`, and the block does not run (17.3).
   It runs after the tag is resolved and before args and inner imports.
4. **Args:** evaluated left to right in the current scope.
5. **Inner imports** → `load` entries, `{"as","unit","name"}` or `{"as","route"}` (13.7).
6. **Dispatch:** bridged → `load` (12.2) to the engine's session, spawned lazily (ch. 8; ts uses js's); compiled →
   ch. 15; run → 17.3.
7. **Definition:** register the exports (13.2), set `Loaded`. An error in steps 2–7 is stored as `Failed(err)`, then thrown.
8. **Value:** a bridged action with `want_value` → the decoded `ret.value`; a compiled or run action → its stdout as 6.3
   specifies; otherwise `null`.

The static block checks (a definition block used as a value, block args, the whole-module `@import` inside a block, the
directives a compiled block accepts) are 4.5's.

### 12.2 The `load` frame

The frame is 9.4's; the host fills it from the `EngineBlock`:

| Member | Value |
| --- | --- |
| `unit` | `"<FileId>:<byte offset of @>"` (`"0:812"`); not unique across concurrent runs of one action block (recursion), so adapters never key action state by it |
| `kind` | `definition`: sent once per unit (a failed load is never resent, 12.1); the adapter keeps the namespace until `bye` only if its reply's `missing` is empty. `action`: sent on every reach; fresh namespace, run, drop; objects that escaped as handles or stubs stay alive |
| `lang` | the canonical tag; `ts` makes the js adapter strip types first (12.5) |
| `file`, `line` | the canonical `.v2` path (2.1); `loc.body_first_line`. Adapters compile with them (12.6). No column member: the host maps columns itself (2.3, 12.6) |
| `source` | `EngineBlock.body`: dedented, directive lines blanked, nothing added; V2 never inserts values into foreign code (4.4) |
| `exports` | `[]` for an action; else the explicit `@export` names in order, plus the element `"*"` when the list contains `*` (4.4) |
| `imports` | 13.7 |
| `args` | object in written order of codec values (ch. 10), e.g. `{"nums":[4,8,15]}`; a V2 function or another session's callable handle goes as an `fn` tag (11.8); another session's other handles are a `MarshalError` before sending (11.7) |
| `want_value` | `true` only for an action block in expression position |

**Reply** (9.4). Action: `{"value": v, "exports": [], "missing": []}` (`v` is `null` unless `want_value`). Definition:
`{"value":null,"exports":[{"name":"area","kind":"func"},{"name":"VERSION","kind":"value","value":"1.2"}],"missing":["typo"]}`.
A non-empty `missing` → `ImportError: @export lists 'typo' but block 'geometry' does not define it`, at that `@export`
item; a py name whose NFKC form, an identifier, differs (Python reads `ｈｅｌｌｏ` in code as `hello`) and that the block's text, read
in NFKC (its directive lines blank), holds as a whole identifier (`units::mentions_read`: a mention counts, so each way Python
defines a name does, `a, NAME = …`, `import m as NAME`, `for NAME in`, `with … as NAME`, under an `if`; a block that only uses it,
or has `NAME: T` alone, gets this wording too, and the message above once it is followed; with no mention, the message above) is `@export lists 'ｈｅｌｌｏ' (U+FF48 …), which
Python reads as 'hello': export hello, and import it as @import { hello as ｈｅｌｌｏ } from p` (`units::missing_export`: the code
point of the first character that changes, with ` …` when others do too; `from @py` for a block without a label); the unit is `Failed` and the adapter has already dropped it. `func` entries (classes included) become Proxies
(13.4); a `value` entry is the copy taken at load (non-plain values arrive as handles). Constant exports exist for
bridged engines (py, js, ts, custom bridged), not compiled.

**Adapter side.** Arg names and import aliases must be identifiers of the adapter's language that are neither keywords
nor names the adapter provides (Python: `str.isidentifier()`, not `keyword.iskeyword()`, no dunders, and the same as
its NFKC form, the name Python reads in code, so `ﬁle` would be bound where the block can't reach it; JS: 12.4's identifier
pattern, not a keyword or a word strict code reserves (`yield let static implements interface package private protected
public`) or a module does (`await`), not `undefined NaN Infinity globalThis __proto__`), else `err ValueError: block arg 'pass' is not a valid
Python name: pass it under another name, as in @py(pass_: …)` (the name with `_` added; for a name that is no
identifier, Python: its NFKC form when that is one (`ⓐ`: `a`), else `name`; JavaScript: `name`; a JavaScript keyword:
`… is not a valid JavaScript name: … as in @js(this_: …)`). A dunder, a name the JS adapter provides, or a word strict
code reserves (a name in the scripts blocks run as, which aren't strict), is `block arg '__name__' is reserved in @py
blocks: pass it under another name, as in @py(name_: …)` (`undefined`: `@js(undefined_: …)`, `yield`: `@js(yield_: …)`,
`@ts` in a ts block); a Python name whose NFKC form differs, `block arg 'µ' (U+00B5) is read by Python as 'μ' (U+03BC):
pass it under that name, as in @py(μ: µ)`, with the code points of the first character that changes and of what it
becomes, since the two can look the same (`ﬁle`: `(U+FB01)`, `'file' (U+0066 U+0069)`). For an alias the message starts
`@import name 'pass' …` and ends `import it under another name, as in @import { … as pass_ } from …` (`import it under
that name, …`). The py adapter imports `unicodedata` only for a
non-ASCII name. Args bind first, then imports, then the code runs. The completion value (py: the last statement
if it is an expression; js/ts: the script's completion value from `runInContext`) is always computed and awaited if
awaitable (9.8), and encoded only if `want_value`. A py action block with `want_value` whose last statement is a `with`, `for`, `if`, `try`, `while` or `match` (its last line looks like the value, but is inside the statement) raises, once it has run to its end, `SyntaxError: the block's value is used, but its last statement is a `with` statement, which has no value: put the value on a last line of its own, outside the `with`` (`an `if``), at that statement. A js/ts action block with `want_value` whose code starts with an
object (`{` then `}`, `...`, or a name (any JavaScript identifier, `{şehir: 1}` too), string or number followed by `:`, `,` or `}`, as in `{a: 1}`) would be read as a
block of code with a label, so it is refused before it runs: `SyntaxError: an object as the block's value needs
parentheses, as in @js { ({a: 1}) }: at its start, JavaScript reads { as a block of code`, at the `{`. So would a block of several lines whose last line starts so, after a line that ended with `;`, `}`, a name or a quote (`{city: city}`): it is refused with the next message. Any block's
`SyntaxError: Unexpected token ':'` on a line that starts with `{` becomes `an object at the start of a statement needs
parentheses, as in ({a: 1}): JavaScript reads { there as a block of code`. Such a block whose last non-blank line starts
with `(`, `[` or a backtick, after a line that ends with a name, a digit, `)`, `]`, a quote or `$` (not `;` or `}`),
would be read as one line with it (`1` then `[a, 2]` is `1[a, 2]`): `SyntaxError: JavaScript reads this line as part of
the one above, as in a(…) or a[…]: end the line above with ;`, at that line. Any block's `Invalid or unexpected token`
on a line that starts with `@name` is `decorators aren't supported in @ts blocks: Node runs TypeScript by removing its
types, and a decorator is code` (js: `decorators aren't part of the JavaScript Node runs yet`). A `ReferenceError: number is not defined` from a `@ts` block whose source has `<number>` (TypeScript's old cast, left as code by the stripping: `5\n<number>a` reads as `5 < number > a`) gets `hint: TypeScript's <T>x cast is code to Node, not a type it removes: write x as T` (the adapter keeps it in `errorHints`, a WeakMap, so it isn't in `e.data`, and sends it as the `err` frame's `hint`).

### 12.3 Python load

```python
def do_load(m):
    path, first = m["file"], m["line"]
    add_file_dir(path); remember_source(path, first, m["source"])          # 9.3; 12.6
    name = unique_name("__v2_" + m["unit"].replace(":", "_"))             # "_2", "_3"… while taken
    mod = types.ModuleType(name); g = mod.__dict__
    g["__file__"], g["__builtins__"] = path, builtins
    for k, v in m["args"].items(): g[check_name(k)] = decode(v)
    bind_imports(g, m["imports"])                                          # 13.7
    tree = ast.parse("\n" * (first - 1) + m["source"], path, "exec")       # SyntaxError carries .v2 lines
    for node in tree.body:                                                 # if __name__ == "__main__":
        if main_guard(node): raise SyntaxError(...)                        # at its line, before any code runs
    tail = ast.Expression(tree.body.pop().value) if tree.body and isinstance(tree.body[-1], ast.Expr) else None
    sys.modules[name] = mod; keep = False
    try:
        exec(compile(tree, path, "exec", dont_inherit=True), g)
        value = eval(compile(tail, path, "eval", dont_inherit=True), g) if tail else None
        value = await_if_awaitable(value)                                  # 9.8
        if m["kind"] == "action":
            return {"value": encode(value) if m["want_value"] else None, "exports": [], "missing": []}
        reply = export_reply(g, name, m["exports"]); keep = not reply["missing"]
    finally:
        if not keep: sys.modules.pop(name, None)
    if keep: units[m["unit"]] = mod, {e["name"]: e["kind"] for e in reply["exports"]}  # kinds: 13.7 (14.2)
    return reply
```

- The prepended blank lines act like `ast.increment_lineno(tree, first - 1)` and also put `SyntaxError`/`SyntaxWarning`
  on the right `.v2` line, so `linecache` shows real `.v2` lines in tracebacks Python prints itself; the text V2 sent is
  unchanged. Columns stay body-relative (12.6); `from __future__` works; `dont_inherit=True` keeps the adapter's
  flags out.
- While it lives, the unit is in `sys.modules` (for `pickle`, `dataclasses`, `typing.get_type_hints`) under a name with
  no dot (`pickle` imports a dotted name's parent); `__name__` is never `"__main__"`, so a top-level
  `if __name__ == "__main__":` (either side of the `==`) is never true: it is
  ``SyntaxError: a block isn't the main module, so its `if __name__ == "__main__":` is never true: run that code directly
  (call main()), or remove the if``, at the `if`, before any of the block runs.
- `export_reply`: the union, each name once, of the explicit names and, when `"*"` is listed, every key of `g` (dict
  order) not starting with `_` whose value the block defined (DOCS 6.1): a class, or a function after `inspect.unwrap`
  (admits `functools.wraps`, `lru_cache`, named lambdas), whose `__module__` is the unit's name, so imports, stubs and
  other units' objects are left out; an exception while checking means no. Absent explicit names go to `missing`.
  Kind: `func` if `callable` (classes included), else `value` with `encode(v)`.
- A Proxy call looks the name up in `g` at call time, so rebinding is seen; a name deleted since →
  `err NameError: 'area' is no longer defined` (9.5).

### 12.4 JavaScript load

```js
// NODE_GLOBALS (computed once): own names of globalThis absent from a fresh context, minus 'global', 'console'
function newContext(file) {                      // one vm context (one realm) per unit: private top-level names
  const sb = {}, module = { exports: {} };
  for (const k of NODE_GLOBALS) forward(sb, k);                            // an accessor to globalThis[k]; see below
  Object.assign(sb, { console /* the adapter's (9.9); V8's per-context console prints nothing */, module,
    exports: module.exports, require: Module.createRequire(file), __filename: file, __dirname: path.dirname(file) });
  const ctx = vm.createContext(sb);
  sb.global = vm.runInContext('Error.stackTraceLimit = 50; globalThis', ctx);
  return ctx;
}
function doLoad(m) {
  const src = m.lang === 'ts' ? stripTypes(m.source) : m.source;           // 12.5
  rememberSource(m.file, m.line, src);
  const ctx = newContext(m.file);
  for (const [k, v] of Object.entries(m.args)) defineData(ctx, checkName(k), decode(v));  // adapter realm (10.7)
  bindImports(ctx, m.imports);                                              // 13.7
  const script = compileOrExplain(src, { filename: m.file, lineOffset: m.line - 1,
    importModuleDynamically: vm.constants.USE_MAIN_CONTEXT_DEFAULT_LOADER });
  const before = new Set(Object.getOwnPropertyNames(ctx));                 // for @export { * }
  return settle(script.runInContext(ctx), v => {  // settle: now, or once a thenable settles (9.8)
    if (m.kind === 'action') return { value: m.want_value ? encode(v) : null, exports: [], missing: [] };
    const unit = { ctx, get: new Map() }, reply = exportReply(unit, before, m.exports);
    unit.kinds = new Map(reply.exports.map(e => [e.name, e.kind]));          // kinds: 13.7 (14.2)
    if (reply.missing.length === 0) units.set(m.unit, unit);
    return reply;
  });
}
```

- `require` is created for the `.v2` file: `./x` resolves from its folder, bare names walk up `node_modules` folders
  from there (so the project root's is found), then an inherited `NODE_PATH` (14.1). `module`/`exports` only let
  CommonJS-style code run. No `columnOffset`: V8 applies it to line 1 only; the host maps columns (12.6).
- `import()` in a block goes to Node's own loader (`USE_MAIN_CONTEXT_DEFAULT_LOADER`), which resolves relative to
  the script's filename, the `.v2` file; a callback there would need `--experimental-vm-modules`. The filter of 12.5
  also drops this loader's one `ExperimentalWarning`.
- Every Node global is forwarded to the context as an accessor reading the main `globalThis` (the receiver some
  getters check), keeping its enumerability. Copying descriptors would load lazy globals (`FormData` is all of undici),
  so nothing loads until a block uses it. Assigning one gives the block its own data property. `@export { * }` counts a
  global the block replaced, such as its own `function fetch`, as the block's. Args are data properties (`defineData`),
  whatever the context has under that name.
- Decoded values (10.7), and values from `require`d modules and Node built-ins (an `fs` error, a package's array),
  belong to the adapter's realm, not the unit's, so `instanceof Array`/`instanceof Error` is false for them: blocks test
  `Array.isArray(x)` and `e.name`/`e.message`.
- `import`/`export` declarations and top-level `await` are SyntaxErrors in a Script. `compileOrExplain` keeps V8's
  location and sends a `ForeignError` (foreign_type `SyntaxError`) with a clearer message (`@ts` for ts blocks):

| V8 message contains | Message sent |
| --- | --- |
| `Cannot use import statement outside a module` | `import statements are not allowed in @js blocks — use require(...), await import(...) inside an async function, or @import { x } from js.<package>` |
| `Unexpected token 'export'` | `export statements are not allowed in @js blocks — use @export { name }` |
| `await is only valid in async functions` | `top-level await is not allowed in @js blocks — end the block with a Promise (it is awaited) or export an async function` |

**Export lookup** caches one getter per name in `unit.get`, returning the current binding. For an identifier
(`/^[\p{ID_Start}$_][\p{ID_Continue}$\u200c\u200d]*$/u`, not reserved) the getter is
``new vm.Script(`(() => ${name})`).runInContext(unit.ctx)``, which also sees `let`/`const`/`class`; any other name (a
quoted `@export` item, 4.4) reads the context's own property. A `ReferenceError` at load puts the name in `missing`.
Kind: `func` for functions (whether a call constructs: 11.3), else `value`.

**`@export { * }`** exports what the block's code added to its context. `before` holds the context's own property names
just before the script runs, after args and imports are bound; `exportReply` takes every own property name not in
`before`, in property order, that does not start with `_` and whose value is a function (an exception while reading it
means no). These are the top-level `function` declarations and the `var`s holding functions. `let`, `const` and `class`
declarations do not become context properties, so their names must be listed. The reply is the union of these names
and the explicit ones, each name once.

### 12.5 TypeScript

ts blocks run in the js session; the stripped text goes through 12.4 unchanged.

- `module.stripTypeScriptTypes(src, { mode: 'strip' })` blanks types with whitespace, so positions stay exact. `enum`, value `namespace` and parameter properties are
  not erasable and fail with `ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX` (`ERR_INVALID_TYPESCRIPT_SYNTAX` on Node 22); such code is written in erasable TypeScript or in
  `@js`. The stripper parses the whole block, so it also throws on any syntax error, before V8 compiles anything. On
  any stripping failure the adapter sends `err` `ForeignError` (foreign_type `SyntaxError`) with one trace frame
  `{file: m.file, line: m.line + L − 1, col: C}`, where L:C is the 1-based body position the stripper reports (on Node 22, the caret under the
  numbered source line of the snippet in its message, a tab drawn 4 wide, the message then being the snippet's first
  line `x …`; a `[line:col]` marker in its message; or, on Node 24, its stack's first line `:L` and a caret under column
  C); with no readable position it sends no frame, so the error stays at the block's `@ts` (12.6). The message ends with
  Node's code: `TypeScript enum is not supported in strip-only mode (ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX)`
  (`ERR_INVALID_TYPESCRIPT_SYNTAX` on Node 22). For TypeScript that is code, the error has a hint (js.js `stripHint`: one of those codes, a message
  saying `not supported`, and the feature it names), the same in a `.ts` file V2 imports or a block requires: `Node runs TypeScript by removing its
  types, and an enum is code: use a const object, as in const Color = { Red: 0, Green: 1 } as const`; likewise `a parameter property is code: declare
  the field and assign it in the constructor, as in who: string; constructor(who: string) { this.who = who }`, `import x = require(…) is code: write
  const x = require("…")` and `export = is code: write module.exports = x` in a `.cts` file, `import x from "…"` and `export default x` in an `.mts`
  file, `const x = require("…") in CommonJS, or import x from "…" in an ES module` and `module.exports = x in CommonJS, or export default x in an ES
  module` in a `.ts` file (or one whose name isn't known), `const x = require("…")` and `@export { x }` in a block (the file: the error's position,
  else the `file:line` first on its stack, else what was required), `a namespace with values is code (an older Node, such as 22.13, refuses even one
  with only types): use a module of its own (a file) or an object, as in const NS = { x: 1 }` and `a <T>x cast is code: write x as T`. Node 24's
  `` `module` keyword is not supported`` (Node 22 reports `module M` as a namespace) gets `hint: write namespace M for module M: Node's type stripping
  takes only that spelling, and only for a namespace of types (one with values is code: use a module of its own (a file) or an object, as in const NS = { x: 1 })`, so a module with values isn't refused a second time for its namespace. A `process.emitWarning` filter drops the one `ExperimentalWarning` it prints. Node's `Failed to load the ES module: …` warning for a file outside `node_modules`, which its loader sends to the `warning` listeners directly, is dropped by a listener that wraps Node's own: 13.5's `commonjs_hint` says it.

### 12.6 Foreign positions to .v2 positions

- **Adapters** compile with the `.v2` path as filename and the block's line offset (12.3, 12.4), so frames inside blocks
  already carry the `.v2` file and line. V8 also counts U+2028 and U+2029 as line breaks, even in strings, so the js
  adapter maps its lines in a body holding them back to `.v2` lines. Frames go innermost first, in 9.6's shape; adapter frames and engine
  internals (`<frozen importlib._bootstrap…>`, `node:internal/…`, `node:vm`) are dropped; library frames stay. An ES module
  that a block `require`s runs with nothing of the block on its stack (Node's require of an ES module), so the block's
  `require` (`blockRequire`) adds its own call site as the outermost frame of an error that has none of the block's
  (kept in a WeakMap, `errorCallers`): the error is placed at the `require` line, not at the block's first line. A
  worker sends only the frames of the request it answers: Python's traceback starts at its handler, and the js adapter
  stops at its request dispatch (`handle`, `handleSync`), so the frames of a request still waiting below a nested one
  are left to the host (9.6). JS frames come from the stack after the message, so another error's stack quoted in a
  message is not read as frames, and a line ending in `)` is `at NAME (LOC)`, any other `at LOC`, so a path holding
  ` (` stays a path.
- **`col`** is 1-based code points relative to the body text line (kept by `remember_source`), sent only for frames in
  `.v2` files, else `null`. Python 3.11+: `FrameSummary.colno` (0-based UTF-8 bytes) converted; 3.10: `null`. JS:
  parsed from `err.stack` (`at fn (file:line:col)`, UTF-16 units) and converted. Syntax errors: Python `e.offset` (the compiler's, unlike the
  parser's, is in UTF-8 bytes and converted; the parser runs under a name CPython can't open, since given the `.v2`
  path it counts the column on that file's line, not the body's); V8's caret line; ts type stripping: the stripper's
  position, which gives the line too (12.5), its caret laid out for a terminal (a tab and a wide character two columns,
  a combining mark none) and converted back.
- **Host:** a frame in a known `.v2` file whose line lies in a block body gets body line `line − loc.body_first_line + 1`
  and its column from `UnitLoc::col` (2.3; a `null` col becomes the line's first non-blank column), and prints as 2.5 says
  (`in @py risky`).
- **Module files.** A `SyntaxError` in a module file an `@import` loads gets a first frame at its file and line: Python's
  when its `filename` is a real path (not `<…>`), with the import machinery's frames (`importlib`'s folder) left out; JS's
  from the `file:line` Node puts first on its stack above a caret line (CommonJS and TypeScript files), else, for an ES
  module, whose error has no position, from `node --check FILE` (10 s limit). V8's `Module.f`, a function called through
  a module namespace, is `f`.
- **Error fields:** `e.file/line/col` come from the innermost mapped frame, else the V2 call site (2.4; the excerpt prints
  either way); `e.lang` is that frame's block tag (`ts` for ts blocks), else the session's tag.

---

### 12.7 The waiting line

A language has no process and costs nothing until something needs it. `Interp.line` holds what would have run in a language by now: one line for all languages, in program order, each place with its engine key (ts is js).

- **Running** means a session exists, `Dead` included, so a dead engine's blocks raise as 8.2 says and never wait.
- **What waits** while its language isn't running: a definition block that is a direct statement of its file, with no args and no inner `@import … from v2`, and an eligible `@import` (13.4) whose every name 13.3 finds in a `Waiting` block. One that names a block not reached yet, a failed or loading one, or nothing, runs at its statement and raises there, starting nothing (13.3). Everything else runs when reached: action blocks, and blocks and imports nested in `if`, `try`, loops or functions, so `try` around them works as it always did.
- **At reach**, 12.1's step 3 runs first, on the spec from `registry.canonical`, with no probe: an unknown tag raises `EngineUnknown`, a run engine with `@export` raises its `ImportError`. Then the unit becomes `Waiting` and joins the line.
- **First need:** a block reached that can't wait, a waiting reference used, a route resolved, or an `@import` that can't wait. Before a block counts as started (`Loading`), the depth limit is checked if its language isn't running, and whatever waits for that language runs, so neither failure is stored as that block's. `session_for` starts the session and runs the line before returning to what needed it.
- **Running the line.** Again and again, the first place in program order whose language is running. While a place runs, only places before it may go: a language started by that place runs what came earlier, and what comes after the place comes after it in the program too, so it runs once the place is done. `xdepth` is saved, zeroed and restored around each place, because the depth limit (9.7) belongs to the caller's chain and must not fail a block forever.
- **Once running**, every statement acts as if the line had never existed.
- **Failures.** A block that fails is `Failed` as at any load, and a failed `@import` leaves its references waiting, so they retry at their next use. Neither stops the line: the rest still runs, and the first error then goes to what needed the language. `exit()` ends the line at once. A use after a failure raises 13.3's `failed to load`.
- **References.** A waiting `@import` joins the line and binds each local name to a `Callable::Waiting`, which names no process, so it can exist before one does. The blocks reached by its statement are kept with it: it finds what it would have found there, so a block further down can't make it ambiguous or supply a name. A call, or the member and method steps of a chain (5.5), settle it: start the language at that use, then run the `@import` as 13.3 and 13.5 say, raising what it would have raised, and go on with what came back, which can be a Proxy, a constant, a handle or `null`. A failure leaves it waiting. Once a Proxy exists its session does too, and sessions are never replaced (8.1), so no reference can dangle. 13.4's rule guarantees nothing else meets a `Waiting`, so `==`, the encoders and printing never do.
- **Compiled blocks** have no process and no line: a definition block is `Waiting` until the first call into it (15.1).
- **The visible difference** (DOCS 5): a block in the line runs when its language starts, so its top-level effects come later than its position, or never, and it sees V2 state from then. Its errors surface at that later point, with the V2 frames of what needed the language below its own. A start that fails leaves its engine dead, which counts as running, so the line runs then and what waited for that engine fails at that need, not at another language's.

## 13. Import resolution and routing

`engines/imports` resolves selectors against the file's export registry; `runtime/interp` binds the results.

### 13.1 Selectors

| Written | `Selector` (4.3) | Resolves against |
| --- | --- | --- |
| `@py` | `Union` | every definition block of that engine in this file |
| `stats` | `Label` | the block with that label, any engine |
| `@py.stats` | `Exact` | the block with that label, which must be a `py` block |
| `py.statistics`, `js."date-fns"` | `Module` | a module unit (13.5); segments joined with `.` (JS subpaths and scopes are quoted: `js."lodash/fp"`) |
| `v2` | `V2` | this file's top-level names; inside blocks only (4.5) |

Tags canonicalize at run time through the registry (7.3): `@python.stats` is `@py.stats`; an unknown tag is 7.3's
`EngineUnknown`. js and ts are one engine, so `@js` and `@ts` (and `@js.x`, `@ts.x`) match blocks of either tag, and
`ts.x` is the module `js.x` (13.5). `sel as NAME` needs a `Module` selector and exists only at V2 level (4.5).
Selectors are file-scoped: an `@import` in `util.v2` or its blocks sees only `util.v2`'s blocks (6.2). Messages quote
selectors as written. A selector whose first word is a V2 module's alias in that file (`import "./lib.v2" as lib`
at its top level, `FileScope.modules`; one imported only in a nested body, such as a function's, gets no hint) is no engine and no label:
its `EngineUnknown` (`lib.wrap`, `@lib.wrap`, `@lib`) or 13.3 step 1's `no block in this file is labeled 'lib'`
(without the py./js. advice) gets `hint: lib is a V2 module (import "./lib.v2" as lib): use lib.total directly; @import
brings in names from other languages' blocks and packages` (the first name imported; `use lib directly` for `@import
lib.x as y`; `alias_hint`). A block's `@import`, whose code can't name `lib`, gets `hint: lib is a V2 module (import
"./lib.v2" as lib): a block gets its names through V2, as in let total = lib.total at the file's top level, then @import { total } from v2 in
the block` (13.7).

### 13.2 ExportRegistry

The registry is `Interp.units`: when a definition unit becomes `Loaded`, it keeps the unit's exports in reply order,
each once, as `Proxy{session, unit, name}` for a bridged function, a `Native` callable for a compiled one (15.8), or the
copy of a `value` export. Entries are never removed. The file's definition blocks (`FileScope.blocks`, collected from
the AST: labels, tags, declared names, `*`) are the static side that 13.3 takes its candidates from; a selector's
tags compare by engine, `js` for ts blocks too (13.1).

### 13.3 Resolving a name from a block selector

A use that can't wait first starts the language of every `Waiting` block `sel` matches (12.7), unless what follows already fails for one of its names without a loaded block: nothing starts for an error. `cands` = the file's definition blocks that `sel` matches, in source order, reached or not. Blocks are named by
`block_name` (12.1). A candidate **provides** `name` when its `@export` list names it, loaded or not, or when it is
`Loaded` and its `*` exported `name`, so ambiguity between explicit exports does not depend on what has run.

1. `cands` empty → `ImportError`: when the file has an action block with that label (and, for `@tag.label`, that engine), `block 'stats' exports nothing: add @export { mean } inside it`, with the names being imported; for `@py` alone, `no @py block in this file has an @export`; otherwise `no block in this file is labeled 'stat'` (`no @py block …` for `@py.stat`), then ` — did you mean 'stats'?` over the labels of the file's blocks (of that engine), or, for a bare label that is an engine's tag (`from py`), ` — did you mean @py? (from @py takes the names this file's @py blocks export)`, or, for a bare label with none close, `: an npm package is imported with js.stat` when `node_modules/stat` is in the file's folder or above it, else `: a Python module is imported with py.stat, an npm package with js.stat` (for a V2 module's alias, 13.1's hint instead).
2. `hits` = the candidates that provide `name`.
3. One hit, `Loaded` → its entry's `value`.
4. More → `ImportError: Import of 'mean' from '@py' is ambiguous (exported by blocks 'stats' and 'calc') — use @py.stats
   or @py.calc` (DOCS 6.3). Three or more read `'a', 'b' and 'c'` / `@py.a, @py.b or @py.c`; an unlabeled block is listed
   as `@py block at main.v2:12`, and the advice becomes `— label the blocks to choose one`.
5. One hit, not `Loaded` (with no hit: the first candidate with `*` that is not `Loaded`) → by its state: never reached →
   `ImportError: block 'stats' has not run yet — move this @import below it`, or, for a block that isn't a statement of its file (`FileScope.top_blocks`) and so sits in an `if`, a loop or a function that hasn't reached it, `ImportError: block 'helpers' (line 3) is inside an `if`, a loop or a function, which hasn't run it: a block runs when the code it is in reaches it — move it to the top level, or this @import into that code`; `Loading` → `ImportError: block
   'stats' is still loading — its exports exist once its top-level code finishes`; `Failed(e)` → `ImportError: block
   'stats' failed to load: <e.message>`.
6. Otherwise → `ImportError: 'x' is not exported by 'stats' — did you mean 'spread'?` (5.9's did-you-mean over what the candidates export; over their `@export` lists as written, it goes on ` (or should its @export say 'x'?)`), else `… (did the block @export it?)` (a candidate with `@export { * }`: `… (its @export { * } exports only functions, and no name that starts with _: list a const, let or class by name, as in @export { x })`, for py `… only the functions and classes it defines, and no name that starts with _: list others by name, …`), or, when a candidate is a
   `Loaded` compiled block with `*`, 15.2's message saying why it was left out.

### 13.4 Binding into the V2 scope

`@import` declares each alias in the current scope like `const` (assigning it is an error; a name already declared there
is 4.5's redeclaration `SyntaxError`) and re-resolves each time it runs, unless it waits (12.7).

**Eligible to wait**, computed once per file for `FileScope`: the `@import` is a direct statement of its file; by name
text across the file (so ignoring shadowing, which only errs towards running at the statement), every use of each
local name is a direct call `n(…)`, or for `sel as n` a plain member read or method call `n.y`, `n.y(…)` (not `?.`, not
`n.y = 1`, not `n(…)`); no block's inner `@import … from v2` names it; and outside the entry file its local name starts
with `_` (V2 modules export the others, 6.4). So a waiting reference can only be called or have a member read: constants,
printing, comparing and passing see exactly what they would if the `@import` had run at its statement.

Imported functions are `func`s: a foreign function becomes `Callable::Proxy { session, unit, name }` (5.1; `unit` is the wire unit id, 9.4),
and calling it sends `call` with target `{"unit","name"}` plus `args`/`kwargs` (9.5, 10.8). Proxies are ordinary
values: stored, passed, re-exported by V2 modules (6.4), sent to other engines as `fn` tags (11.8). `sel as NAME` binds
a module handle (13.5).

### 13.5 Module units

A module that isn't installed is the adapter's `ImportError` (Python `No module named 'x'`, Node `Cannot find module 'x'`) with a hint (2.4), read from that message (`imports.rs` `missing_package_hint`):
- A top-level name Python has no module of gets the adapter's hint (9.6), in place of the host's below (py.py `package_hint`): a `my-helper.py`, or a `my-helper` folder with an `__init__.py`, in the `.v2` file's folder or the project root, for `py.my_helper` or `py."my-helper"`, `Python module names use _: rename my-helper.py to my_helper.py and import py.my_helper`; else, for a name of Python's standard library (`sys.stdlib_module_names`, the same on every system) that this Python doesn't have (py.py `stdlib_hint`): one only another system's Python has (on Windows `fcntl`, `grp`, `posix`, `pwd`, `readline`, `resource`, `syslog` and `termios`; elsewhere `msvcrt`, `nt`, `winreg`, `winsound`, `_winapi` and `_overlapped`), `'fcntl' is only in Python on Linux and macOS` (`'winreg' is only in Python on Windows`; for readline, `; pyreadline3 is a substitute` after it); on Linux, whose distributions package parts of it apart, `'tkinter' is part of Python's standard library, but this Python was installed without it: on Debian and Ubuntu, sudo apt install python3-tk` (for root, 7.8's `apt update && apt install python3-tk`; another module, `…installed without it; pip can't add it`); elsewhere (tkinter, which python.org's embeddable package leaves out) `'tkinter' is part of Python's standard library, which this Python lacks: pip can't add it; use a full Python`; else a distribution that Python has installed, found by pip's normalized name (`pyyaml`, `PyYAML`, `opencv_python`, `py."opencv-python"`), whose modules are named otherwise, `PyYAML is installed, and its module is named yaml: @import py.yaml as yaml` (`its modules are named a and b`, at most three, the closest to the distribution's name first (2.4's distance, `-` and `.` read as `_`), the first in the example, by its last part for a dotted one: `protobuf is installed, and its module is named google.protobuf: @import py.google.protobuf as protobuf`; the names of its `top_level.txt` that Python finds (`importlib.util.find_spec`, which asks every finder, a `pip install -e` one's too, and imports nothing for a top-level name; a `setup.py develop` egg-info's too), one whose files its `RECORD` lists without its `__init__.py` by what is below it, as a namespace's (google-auth's `google` is `google.auth` and `google.oauth2`, PyQt5_sip's `PyQt5` is `PyQt5.sip`), and none for a namespace it has no file in (google-auth's `scripts`); without a `top_level.txt` (hatchling, scikit-build-core), from its `RECORD`: each file's first folder with an `__init__.py`, so a namespace package's is its package (`google.protobuf`, never `google`), or a `.py`, `.pyd` or tagged `.so` file (`x.abi3.so`) at the top, never a folder of data (llama-cpp-python's `bin`, `include`, `lib`) or an editable install's own module (`__editable__…`, `_editable_impl_…`); a name with a part starting with `_` only when there are no others, and no hint when no name qualifies: `installed_as`, `dist_modules`, with `importlib.metadata`), as `pip install pyyaml` would only say it is installed.
- Python names the first part it couldn't find, so a dotted name (`os.pathh`, `xml.etreee`) means the rest was there: `'xml' is there, but has no 'etreee': check the name after 'xml.'` (`'os' is a module, not a package, so it has no 'pathh': …` when Python says so). A name a folder below the `.v2` file's holds as `NAME.py` (three levels at most, the nearest first; hidden folders, `node_modules`, `venv`, `__pycache__` and `site-packages` aren't looked in) gets `there is a stages/clean.py in a folder below: import it as py.stages.clean`, when each folder's name is a part of a module name (as `module_unit` checks it); when only a path through one that isn't (`2024`, `class`, `my-lib`) has it, `there is a 2024/clean.py in a folder below, but 2024 isn't a Python name, so no py. name reaches it: rename the folder, or put it on sys.path in a @py block first`. A `.v2` file under a folder with a `v2.toml` that isn't the run's project root (a run uses the entry script's project, 18.1), when that project has the module (a `NAME.py` or a `NAME` folder at its root, or an entry `NAME`, `NAME.…` or `NAME-…` in the site-packages of the venv its `v2.toml` names, as pip's `NAME-1.0.dist-info`; `project_has`), gets `C:\mono\lib.v2 belongs to another project (C:\mono, with its own v2.toml), but a run uses the entry script's project (C:\glue): run it from that project, or put the module where this one finds it` (the file as its position shows it). Else a `NAME.py` in a folder up to three above: in a folder of the project, below its root, `there is a C:\proj\lib\rooth.py in a folder above: from the project folder, C:\proj, it is py.lib.rooth` (when each folder's name is a part of a module name, as `module_unit` checks it: an identifier that isn't a keyword, so not `2024` or `class`); in the entry script's folder or one above it, with no `v2.toml` found, `there is a C:\proj\rooth.py in a folder above: a v2.toml in C:\proj (it may be empty) makes that the project folder, where py.rooth is found`; in any other folder but the root, `there is a C:\shared\h.py in a folder above, which isn't on Python's path: move it next to the .v2 file or into the project folder, or put its folder on sys.path in a @py block first`, never the install line, except above the root of the project the `.v2` file is in, where it gets the install line as any other. Else `if 'x' isn't misspelled, install it with pip for the Python that v2 doctor shows; README.md, "Python and npm packages", has the steps` (for a well-known module whose package on PyPI is named otherwise, the package, there as `install its package, PyYAML, with pip …` and in every command below: yaml → PyYAML, PIL → Pillow, cv2 → opencv-python, sklearn → scikit-learn, bs4 → beautifulsoup4, slugify → python-slugify, dateutil → python-dateutil, docx → python-docx, dotenv → python-dotenv, jwt → PyJWT, serial → pyserial, Crypto → pycryptodome, as `pip install yaml` finds nothing and `pip install slugify` installs another, Python 2 package: `pypi_name`; for Python's embeddable package (7.6), whether V2 was told to use it or found it on `PATH`, while it has no pip (none in its `Lib\site-packages`; with pip, the lines below), `this is Python's embeddable package, which has no pip: for packages, use a full Python (python.org's installer, or the nuget package) and a venv`, joined to the `there is a virtual environment in …` hint below by `; and `; for a Python V2 was told to use, a venv, a command or `V2_ENGINE_PY`, a command that works for it: `if 'numpy' isn't misspelled, install it for the Python V2 uses: C:\proj\.venv\Scripts\python.exe -m pip install numpy`, the whole command with its arguments, `/root/.pyenv/bin/pyenv exec python -m pip install numpy`, each word quoted when it needs it (`cli::shell_word`, 19.1), after `& ` on Windows when the first one is; `: uv add numpy` for a command that runs `uv run`; for a venv uv made, which has no pip (7.6), `: uv add numpy in /p` for a uv project's `.venv`, else `: uv pip install --python /p/.venv numpy`; for another venv whose site-packages has no `pip`, `…, whose venv has no pip: /p/.venv/bin/python -m ensurepip, then /p/.venv/bin/python -m pip install numpy`, or, when the Python it was made with has no `ensurepip` (the standard library of its `pyvenv.cfg`'s `home`, `<home>/../lib/python3.X`, Windows `<home>\Lib`, holds `os.py` but no `ensurepip` folder: Debian and Ubuntu leave it out without python3-venv, and `python3 -m venv` then stops after making a venv without pip), `…, whose venv has no pip, and its Python has no ensurepip to add it (on Debian and Ubuntu: sudo apt install python3-venv): then make the venv again, python3 -m venv --clear /p/.venv, and install its packages, as in /p/.venv/bin/python -m pip install numpy` (for root, 7.8's `apt update && apt install python3-venv`; on Windows `…to add it: make the venv again, py -m venv --clear …`; with 7.6's Python, the one its `home` names while it is there); for one found on `PATH` in a venv (an activated one: the folder above its folder holds a `pyvenv.cfg`) or in a conda environment (an activated one: its prefix, the folder above its folder, on Windows its own folder, holds `conda-meta`), which V2 uses, its own command as above, and the `there is a virtual environment in …` hint below only when that is another venv; for one found on `PATH` while the activated venv can't be used (7.6), that venv's message instead, also in place of the `there is a virtual environment in …` hint below when that is the same venv, and after that hint, joined by `; and ` instead of `; or `, when it is another), after, when no venv or py command is configured and a `.venv` or `venv` holding `pyvenv.cfg` is in the `.v2` file's folder or up to three above it, `there is a virtual environment in C:\proj\.venv, which V2 isn't using: name it in a v2.toml next to it, [python] venv = ".venv"; or `. When `V2_ENGINE_PY` wins over the project's `[python] venv` (18.3) and that venv's site-packages has the module (an entry of its name, as `project_has` reads them: `venv_has_module`), the hint starts with 19.2's note (unless unsetting it wouldn't help, as there), `V2_ENGINE_PY is set, so [python] venv 'C:\p\.venv' (v2.toml) isn't used: to use it, Remove-Item Env:V2_ENGINE_PY` (`unset V2_ENGINE_PY` off Windows), then `; or ` and the rest (`; and ` before the embeddable package's line; `imports::unused_venv_hint`).
- A top-level Python name of six or more characters one edit from an importable one gets Python's own wording for suggestions in the message, from the adapter (`near_module`: the standard library's names, the built-in ones and what `pkgutil.iter_modules()` finds on `sys.path`, names starting with `_` only for a name that does, and none that isn't an identifier, such as a `my-helper.py`; none with the first bullet's hint): `No module named 'statistcs'. Did you mean: 'statistics'?`. Stricter than 2.4's rule, as most such names are packages not installed yet, and short ones sit one edit from a standard module (`lxml`, `xml`; `httpx`, `http`; `PIL`, `pip`).
- An ES module imported whole is its namespace (`@import js.chalk as chalk`): calling a name it lacks but its default export has (`chalk.red(…)`) is `TypeError: red is not a function of the module itself, but of its default export: write m.default.red(…), or import the default export with @import { "default" as m }`, and calling the namespace when its default export is a function is `TypeError: the module is not a function, but its default export is: call it through .default, as in m.default(…), or import it with @import { "default" as f }` (js.js `notAFunction`).
- A package whose `exports` offer only `import` (`No "exports" main defined in …/node_modules/esm-only/package.json`, which `require` can't load) gets `hint: 'esm-only' can be loaded only with import(), which a @js block can do: async function load() { return await import("esm-only") }, then @export { load } and call load()`.
- A requested name a bare-name package lacks, when its `exports` give `import()` another file than the one `require` loaded (a dual package, its builds told apart by the `import` and `require` conditions), gets the reply's `hint` (9.4), unless a name is near (5.9): `hint: V2 loaded C:\proj\node_modules\dual\cjs\index.cjs, the package's "require" build, as require() does; its "import" build, C:\proj\node_modules\dual\esm\index.js, is loaded with import(), which a @js block can do: async function load() { return await import("dual") }, then @export { load } and call load()` (js.js `importBuild`: the package.json is the first above the loaded file with the package's name; the subpath's entry, exact or a one-`*` pattern, read with the conditions `node`, `import`, `module-sync`, `node-addons` and `default` in its order).
- Node has no JSX. A syntax error in a `.jsx` or `.tsx` file (the file of its position, else the one a block required) and Node's `ERR_UNKNOWN_FILE_EXTENSION` for one (a `.ts` file importing it), from a module V2 imports or a block's `require`, get `hint: Node has no JSX, so it can't run .jsx or .tsx files: compile comp.jsx first and import the .js your build writes (TypeScript without JSX can be a .ts file)` (js.js `jsxHint`); a syntax error gets none when `require.extensions` has a handler for its file's extension (a loader that `NODE_OPTIONS` registers, `--require @babel/register`), as the error is then the code's own.
- A local file that uses `import` or `export` (`Unexpected token 'export'`, `Cannot use import statement outside a module`) when the nearest `package.json` above it says `"type": "commonjs"`: `hint: C:\proj\package.json says "type": "commonjs" (npm init writes that since npm 11), so Node loads this file as CommonJS, without import and export: set "type": "module" there, or rename the file to .mts` (`.mjs` for JavaScript; `commonjs_hint`).
- A TypeScript file's own imports, which Node reports as a `ForeignError` (`ts_import_hint`): a bare specifier that a `paths` alias of the tsconfig.json nearest that file matches (`Cannot find package '@lib/math' imported from …\app.ts`, where Node names only the package part, `~` for `~/utils`; a `.cts` file's `Cannot find module '~/utils'` with the file first on its `Require stack`) gets the alias hint of the npm bullet below, first; `Cannot find module '…\math.js' imported from …\app.ts`, for a `.js`, `.mjs` or `.cjs` (or extensionless) path with a `.ts`, `.mts` or `.cts` file of that stem, gets `hint: Node runs .ts files as they are written, so imports between them name the .ts file: write math.ts for math.js in app.ts`; `The requested module './types.ts' does not provide an export named 'Shape'` (a `.ts` module) gets `hint: if 'Shape' is a type or an interface, import it with import type { Shape } from './types.ts': Node removes types as it runs a .ts file, so they aren't exports`. Before it: when that module is a `.js` or `.ts` file next to the imported one whose nearest `package.json` says `"type": "commonjs"`, the cause is that (`nested_commonjs_hint`): `hint: C:\mods\package.json says "type": "commonjs", so Node loads ./vec.ts as CommonJS, whose exports an import can't name: set "type": "module" there, or rename it to .mts` (`.mjs` for `.js`).
- npm: the package is the specifier's first part (`@scope/name` for a scoped one). A package in a `node_modules` that is a Linux symlink seen through `\\wsl.localhost` (`npm link`, workspaces), which Windows' Node can't follow, is `hint: '\\wsl.localhost\Ubuntu\proj\node_modules\pkg' is a Linux (WSL) symlink, which Windows can't follow: use the path it points to (ls -l in WSL shows it), or run V2 inside WSL` (`module::wsl_link_line`). A subpath of an installed one (in a `node_modules` from the `.v2` file's folder up) is `'lodash' is installed, but has no 'chunck': check the path after 'lodash/'`; else, when a `compilerOptions.paths` key of the tsconfig.json nearest the `.v2` file's folder (comments and trailing commas allowed; a relative `extends` followed when it has no `paths`) matches the name (`@lib/*` matches what starts with `@lib/`, or a package part `@lib`; `utils` matches `utils` and `utils/…`; the catch-all `*` matches nothing), `hint: "@lib/*" is a "paths" alias in C:\proj\tsconfig.json, and Node doesn't read tsconfig paths: import by a relative path, or use package.json "imports" (#alias)`; else, when the name (with `.js`, `.mjs`, `.cjs`, `.ts`, `.mts`, `.cts`, `.jsx` or `.tsx`, or the name itself when it ends in one) is a file of the `.v2` file's folder, its folders kept (`lib/math` for lib/math.ts, as a tsconfig `"baseUrl": "."` lets TypeScript write it), `your own helpers.js is imported by its path from the .v2 file's folder: @import { … } from js."./helpers.js" (js.helpers, without ./, names an npm package)` (`your own lib/math.ts …: @import { … } from ts."./lib/math.ts"`; `ts.` for TypeScript, `.tsx` included; the name as written, `js."helpers.js"` for one that isn't a plain name), and a name with such an extension that isn't there, `a file of your own is imported by its path from the .v2 file's folder, as in js."./chart.js": check its name (js."chart.js", without ./, names an npm package); or, for the npm package chart.js, npm install chart.js in C:\proj` (some packages' names end in `.js`: decimal.js, chart.js), without the install line when its first part is a folder of the script's own, with `; did you mean js."bn.js"? if 'big.js' isn't misspelled, npm install big.js in C:\proj` in its place when the name nearly spells an installed package, and the Yarn line below in place of `npm install …` in a Yarn Plug'n'Play project (`; or, for the npm package chart.js: this project uses Yarn …`); else, when the name's first part is a folder next to the `.v2` file, the script's own code, never `npm install lib`: `lib is a folder of your own, whose files are imported by their paths from the .v2 file's folder, as in js."./lib/mathh" (js."lib/mathh", without ./, names an npm package)` (`./lib/…` for the folder's name alone; for the folder's name alone when the folder has no `package.json` and no file with those extensions in it or in its folders two levels down (`holds_code`: node_modules and hidden folders aside), a folder of data as node-config's `config/` or Prisma's `prisma/` is, the same ending as for `chart.js`, `; or, for the npm package config, npm install config in C:\proj`, except from a block's `import()`, whose message names only the package part); else, when the `.v2` file's folder or one above it has a `.pnp.cjs` (or `.pnp.js`), `this project uses Yarn Plug'n'Play (C:\proj\.pnp.cjs), which Node can't read without Yarn's loader: set nodeLinker: node-modules in .yarnrc.yml and run yarn install`; else `if 'lodsh' isn't misspelled, install it with npm install lodsh in C:\proj or a folder above it, where Node looks for this file` (the `.v2` file's folder: a shared module's is not the project's), after `did you mean js.lodash? ` over the nearest `node_modules`' packages (`@scope/name` for a scope's, none of its dot entries such as `.bin`; the name as written, `js."bignumber.js"` for one that isn't a plain name). A relative or absolute specifier: `a local file is found from the .v2 file's folder: check the path`, or, for a relative one that is a file with one of those extensions added, `did you mean ts."./money.ts"? a file is named with its extension`. A name a bare `js.NAME` (or `ts.NAME`) module doesn't export while the `.v2` file's folder has NAME with one of those extensions (`js.util` and util.js: Node's built-in wins) gets `hint: js.util is a package (Node's own or one from npm), not your util.js: a file of yours is imported by its path, as in js."./util.js"` (`shadowed_hint`). Tool names given as the language (`npm.x`, `node.x`, `pip.x`, `@node`) are `EngineUnknown: unknown engine '@npm' (did you mean '@js'? npm packages are imported with js.NAME, as in @import { chunk } from js.lodash)` (7.3).
- A block's own `require()` or `import()` of a bare specifier Node doesn't find is a `ForeignError` (its data's `code` `MODULE_NOT_FOUND`, the `.v2` file first on its `Require stack`; or `ERR_MODULE_NOT_FOUND`, `Cannot find package '@lib/math' imported from C:\proj\main.v2`, where Node names the package part) that gets the npm bullet's hints in its order, from the `.v2` file's folder, where a block's `require` looks, in a block's words (`imports.rs` `block_module_hint`, from `session.rs` as the error arrives, and `npm_hint`, which both use): the installed package without that subpath, the `paths` alias, a file of the script's own, `your own helpers.js is loaded by its path from the .v2 file's folder: require("./helpers.js") ("helpers", without ./, names an npm package)` (`require("./lib/math.ts")` for `lib/math`; `import("./helpers.js")` for an `import()`), one with such an extension that isn't there, `a file of your own is loaded by its path from the .v2 file's folder, as in require("./utils.js"): check its name ("utils.js", without ./, names an npm package); or, for the npm package utils.js, npm install utils.js in C:\proj`, a folder of the script's own, `lib is a folder of your own, whose files are loaded by their paths from the .v2 file's folder, as in import("./lib/…") ("lib", without ./, names an npm package)` (an `import()`'s message names only the package part, `lib` for `lib/math`; a `require()` names it whole, `require("./lib/mathh")`), Yarn Plug'n'Play and the install line, whose did-you-mean is `did you mean "lodash"? `. A relative or absolute specifier, and one that a module the block required couldn't find (its own file first on the stack), get none: Node's message names the path.

A `Module` selector names one unit per module per session, `"<canonical tag>:<name>"` (9.4): `py:statistics`,
`py:os.path`, `js:date-fns`; for a relative (`./`, `../`; on Windows also `.\`, `..\`) or absolute JS specifier `<name>` is the absolute path joined from the
importing file's folder. `ts.X` and `js.X` share the js session's unit `js:X`. Only bridged engines take module
selectors (`ImportError: module imports need a bridged engine — 'c' is compiled` / `— 'lua' is a run engine`), and only
with the `modules` cap (17.5). Python names must be dotted identifiers, each segment a letter or `_` followed by letters, digits or `_`, and not a Python keyword, checked before anything is sent, except a name of one part that is one with `_` for each `-` (`py."opencv-python"`, `py."my-helper"`; `pip_style`), which goes to the worker, whose `module` answers it with the same error and the first bullet's hint (`ImportError: 'my-mod' is not a valid Python module name`; a file's path, one ending in `.py` or holding `/` or `\`, adds how to name it (`py_path_hint`): below the `.v2` file's folder, `py."./lib/helper.py"`, `: a Python file next to the .v2 file is imported as a module, py.lib.helper`; below the project root, `py."../src/acme/core.py"`, `: the project folder, C:\proj, is on Python's path, and from there this is py.src.acme.core`; anywhere else, or with a part that isn't a name, `: Python finds a module by its dotted name in the .v2 file's folder, the project folder or its own path (a venv's packages): write that name, or put the file's folder on sys.path in a @py block before this @import`); a JS name is any non-empty specifier
(package, `node:` built-in, `./path` from the `.v2` file), a `.jsx` or `.tsx` file's too: a loader that `NODE_OPTIONS` registers runs it, and without one loading it fails with the JSX hint above. Every `@import` from a module sends one `module` frame; the
adapter imports once per unit, so a bare name's first import wins for the session, as in `sys.modules` (a JS package
resolves from the folder of the first `.v2` file that imports it; when a later importer's folder resolves the bare name to another file, its own copy, the
adapter writes, once per such file, `note: js.legacy from C:\b resolves to C:\b\node_modules\legacy\index.js, but this run already loaded
C:\a\node_modules\legacy\index.js: a package is loaded once per run (DOCS 7)` to stderr in an `out` frame, js.js `otherCopy`):

```json
{"t":"module","id":7,"unit":"py:statistics","name":"statistics","from":"C:\\proj\\main.v2","names":["mean"]}
```

`name` is the module name as joined (13.1) or the JS specifier as written. `from` is the absolute path of the importing
`.v2` file; the adapters use it for `sys.path` and `createRequire`. `names` is a list, or `null` for the whole
module as a handle (`@import py.numpy as np`). Reply: `ret {"value": <module handle or null>, "exports":[{name, kind,
value?}], "missing":[...]}`.

| Step | Python | JavaScript |
| --- | --- | --- |
| load | `importlib.import_module(name)`, the folder of `from` on `sys.path` (9.3) | `createRequire(from)(name)`, which loads ES modules too (require(esm)); only a module with top-level await in itself or its ES module graph (`ERR_REQUIRE_ASYNC_MODULE` whose `From` line, when Node prints one, names the importing `.v2` file; a CommonJS module that requires one has already run) goes through `import(entryUrl(name, from))` (14.4), awaited as 9.8 allows |
| failure | the module (or a parent package) not found → `err ImportError` with the foreign message (`No module named 'pandas'`); any other exception, a missing dependency of the module included → `ForeignError` (9.4) | `require.resolve(name)` runs first, so not found is decided for the module itself: `MODULE_NOT_FOUND`, `ERR_PACKAGE_PATH_NOT_EXPORTED` or `ERR_UNKNOWN_BUILTIN_MODULE` there → `ImportError` with the first line of Node's message (`Cannot find module 'pandas'`); errors while loading → `ForeignError` |
| name `n` | `getattr(mod, n)`, else the submodule `import_module(name + "." + n)` | `mod[n]` unless inherited from `Object.prototype`/`Function.prototype`; `"default"` → `mod.default ?? mod`; else `mod.default[n]` |
| kind | callable (classes too) → `func`, else `value` (copied; non-plain values arrive as handles) | function (classes too; constructed or called per 11.3) → `func`, else `value` |

- `@import js.pkg as x` binds the module object as a handle; for a CommonJS package that exports a function it is a
  callable handle (`@import js.slugify as slugify`). `@import { "default" as x } from js.pkg` takes `mod.default ?? mod`.
- A module the importing `.v2` file's folders hold (`NAME.py` or `NAME/__init__.py` in that file's folder or the project root),
  while Python already has another under that name (one the worker loaded itself, such as `json`, or another
  folder's), is `ImportError: py.json is C:\…\json\__init__.py, which was loaded first, not your C:\proj\json.py: give
  your file another name (one Python serves the whole run, with one module per name)`, at every `@import` of it: Python
  itself would have loaded the file. The two are compared by real path (`realpath`, then `normcase`), then by
  `os.path.samefile`, so a `helpers.py` that links to the module loaded is that module, and so is one reached another
  way (a share of a local folder, `\\localhost\C$\…`, and its `C:\…`). A module built into Python (`sys.builtin_module_names`, or a spec origin `built-in`
  or `frozen`: `math`, `time`, `sys` on Windows and Linux) has no file, so the message names it as `Python's built-in math
  module`: Python finds built-in modules before it looks in folders, so it never reads a `math.py` of yours. Where
  `math` is a file of its own that isn't loaded yet (macOS), Python reads the script's `math.py`, and so does V2.
- A missing requested name of a module that is a file of the importing file's folders and has a name of Python's own
  (`sys.stdlib_module_names`) → `ImportError: py.statistics is your C:\proj\statistics.py, which hides Python's own
  statistics module, and it has no 'mean': give your file another name`.
- A missing requested name → `ImportError: 'x' is not exported by 'py.statistics'`, with ` — did you mean 'mean'?` when
  one of the reply's `names` (9.4) is close (5.9); else, for a `js.`/`ts.` module whose path's
  last part is that name (`.js`, `.mjs`, `.cjs`, `.ts`, `.mts`, `.cts` dropped) it goes on `; a module that is one function
  is imported with @import { "default" as chunk } from js."lodash/chunk"`. Its hint: `shadowed_hint`'s; else, for a module that is a `.ts`,
  `.mts` or `.cts` file whose lines declare the name as a type or an interface (`export type Shape`, `export interface Point`, with `declare` or without
  `export` too; `ts_type_hint`), `hint: Shape is a type, which TypeScript removes before it runs: there is nothing to import; use it in TypeScript
  only`; else the reply's (9.4). A `js.`/`ts.`/`py.` selector of one segment
  ending in `.v2` is the SyntaxError `a .v2 file isn't a JavaScript module` (`Python`), with 3.7's `.v2` hint. A module Proxy targets
  `{"unit":"py:statistics","name":"mean"}`; the adapter repeats the lookup on every call (JS calls the function with the
  object it was read from as `this`).

### 13.6 Routes

Foreign code cannot hold a V2 `Value`, so what a block imports through the host is reached by numbered routes. A route
is an entry of `Interp.routes`, numbered separately from the callable table of 11.8; `{"route": r}` carries its
index, and routes are never released. Each holds:

```rust
pub enum Route {
    Target(Value),                                           // a resolved Func: Proxy, Native or Closure
    Pending { block: Rc<EngineBlock>, import: usize, name: Rc<str> },  // that directive's selector, resolved when
                                                             // called; stays Pending while that fails
    V2 { file: FileId, name: Rc<str> },                      // `v2` selector: looked up at every call
}
```

One route per (importing unit, directive item), allocated at the importer's first load; each later load (a re-run action
block) resets a resolved route to `Pending`, so the table stays bounded by the import directives. On
`call {"route": r}` (9.5): `Target(f)` → call `f`; `Pending` → resolve now with 13.3 or 13.5: found, it becomes
`Target` and is called; otherwise its `ImportError` is raised into the caller and the route stays `Pending`, so a later
call looks again (a block not run yet and a constant give 13.7's messages, not 13.3 step 5's advice to move the
`@import`); `V2` → call the file's current top-level `name` (`NameError: 'log_line' is not defined at the top level of
main.v2`; `TypeError: 'x' is not a function`). Compiled blocks keep their `Route`s per loaded library, keyed by alias
(15.2).

### 13.7 Imports inside blocks

Inside a block only `@import { … } from SEL` exists; the whole-module form `SEL as h` is a `SyntaxError` there that
points at the language's own import (4.5). At step 5 of 12.1 each directive item becomes one `imports` entry (shapes:
9.4). In-block imports name functions. Resolution is lazy, so blocks may import from each other in any order: an entry
names its target, and the function is looked up only when called. `as` is the name bound in the block (the alias, else
the imported name).

| Target | Entry | The adapter binds |
| --- | --- | --- |
| an export of a block of **this session** (py→py, js↔ts), loaded or not, or an attribute of a module of this session (imported first with a `module` frame, 13.5) | `{"as","unit","name"}`; for a block also `route` | the target itself once the unit is `Loaded`: at load if it already is, else replaced in the block's namespace right after the unit's `ret` is written, unless the block rebound the name (a `*` name or a constant stays as it was). Like Python's `from m import f`, a later rebinding in the target isn't seen. Until then, a stub that looks `name` up at every use and calls it in the worker (JS: per 11.3; a `Proxy` whose `new`, `extends`, `instanceof` and property reads also reach the target; Python: an object whose calls, attributes, `isinstance` and subclassing do). In JS, a function read from a module object is a `Proxy` of it, one per function, whose plain calls get that object as `this` (13.5); everything else is the function's own: no host hop, exceptions propagate natively |
| anything else: another session's block or module, a compiled export, `v2` | `{"as","route"}` | a stub that sends `call {"route": r}` (14.2) |

- Errors that no later load can fix are raised at load: a selector matching no block (13.3 step 1); an ambiguous name
  (step 4); a target block that is already `Failed` (step 5's `failed to load`); no candidate providing the name (step
  6); an invalid module name (13.5); a `v2` name not declared at the
  file's top level (`func`, `let`, `const`, `import`, `extern`, `@import`): `ImportError: 'x' is not a top-level name in
  main.v2`; a constant one (`const`, `@import`; 5.6) that holds something other than a function:
  `ImportError: 'x' is a constant — pass it in as a block arg`; a `let` that holds something else, not `null`: `ImportError: 'k' is an int, not a function — pass it in as a block arg` (one that holds `null` or isn't declared yet may still get a function before the first call; a call when it holds data is `TypeError: 'k' is a list now, not a function — pass data in as a block arg`).
- A name the target exports as a constant → `ImportError: 'VERSION' is a constant — pass it in as a block arg`: at load
  when the target is `Loaded` or a module of this session (imported then), else at the first call. A module of this
  session's (zod's `z`) adds `hint: a block imports only functions; in the block, import the package the language's own way: const { z } = require("zod")` (Python: `from zod import z`). A JavaScript route stub (an import whose block hasn't run yet, so V2 can't tell a constant from a function) used as a value, its `Symbol.toPrimitive` or `toString` (`VERSION + "!"`), throws `V2Error` ImportError `'VERSION' is imported as a function, and used here as a value: a block imports only functions; for a constant, pass it in as a block arg`, instead of giving the stub's code.
- Calling a stub whose block has not loaded → `ImportError: block 'b' has not run yet — 'f' can't be called before it
  loads`, `f` being the imported name. Through a route: 13.6 (still loading or failed: 13.3 step 5). A same-session stub
  whose unit is not in the adapter's `units` (not reached, still loading, or failed after this block loaded) calls its
  entry's `route`, so the answer is the host's, the same as any route's. It raises the constant error itself, as a
  `V2Error` (14.2), when the unit's load reply listed the name as `value`.
- A name that only a not yet loaded `*` block could provide (13.3 step 5) gets a route, even in this session.
- Compiled blocks take no imports in program mode (4.5); their routes: 13.6.

---

## 14. Built-in adapters

`adapters/py.py` and `adapters/js.js` are the worker side of ch. 9–13; custom bridged adapters follow the same shape
(14.5).

### 14.1 Distribution and launch

- Both files are embedded with `include_str!` and written create-new (0600 on Unix) to `<run>/adapters/` when their
  session first spawns (8.3); they are never read from the project, `PATH` or `~/.v2` (20.2).
- Launch argv and environment are 7.2's: `<python> -B -u <run>/adapters/py.py` with `PYTHONUTF8=1` and
  `PYTHONIOENCODING=utf-8` (with `PYTHONINSPECT` set, py.py's `bye` ends with `os._exit(0)` after the atexit handlers, as `python -i` would print the `SystemExit` and wait for input); `<node> <run>/adapters/js.cjs` with nothing added (`.cjs`, so it is CommonJS whatever `NODE_OPTIONS` or a `package.json` says the default is), so an inherited `NODE_PATH` reaches Node
  untouched.
- Output from code that runs before the adapter owns fd 1 (`sitecustomize`, `.pth` files, `NODE_OPTIONS=--require`) is
  9.3's preload noise.

### 14.2 Shared skeleton

```text
setup()            14.3 / 14.4: import hygiene, protocol fds and stdio (9.9), signals
send(hello)        vep 1, version, pid (9.3); caps: py ["modules","handles","kwargs","reverse"],
                   js ["modules","handles","reverse"] (JS functions take positional arguments only, 10.8)
serve(None)        the top-level loop; it ends only by exiting
serve(wait_id):    for each incoming frame m:
  ret, err         must carry wait_id: return the decoded value / raise the decoded V2Error; else stderr, exit 2
  release          apply (11.1); no reply
  bye              flush output, exit 0 (9.10)
  init load module call op
                   push request; try: reply ret(handler(m)), awaiting per 9.8
                   except any exception e (Python BaseException, any thrown JS value): reply err(serialize(e))
                   finally: pop
  other t, with id → err ProtocolError, keep serving (9.1);  unparseable → stderr, exit 2 (9.1)
  EOF              exit 0 quietly (9.1)
host_call(target, args, kwargs)                     route stubs (13.7) and V2 function stubs (11.8)
  top != request   raise V2Error (type Error): V2 function 'progress' called outside an active V2 call  (9.8)
  Python, not the dispatch thread → raise V2Error (type Error): V2 function 'progress' called from another thread: …  (9.8)
  id = next_out_id(); send(call {id, target, args, kwargs}); push waiting; try: return serve(id)  finally: pop
send(f)            under the send lock: buffered output as out frames (9.9), queued releases as one release frame
                   (9.10), then f; a reply over 64 MiB becomes err MarshalError, an oversize call raises
                   MarshalError in the caller (9.1)
```

- **The loop** is `serve(wait_id)`. Every request gets exactly one `ret` or `err`, and requests nest: a stub's
  `serve(id)` handles the requests the host sends while it waits, so replies always answer the innermost pending request
  (9.7).
- **The stack** holds a `request` entry per host request being handled and a `waiting` entry per stub blocked in
  `serve(id)`. `host_call` proceeds only when the top entry is a `request`: a synchronous call, a request whose coroutine
  or Promise is being awaited, or a nested request served inside a stub (9.8). A callback stored during one request
  may be called during any later one; from a timer, an idle turn or a finalizer, or while the adapter is blocked in a
  stub, it raises.
- **Tables** live in the adapter's scope, never in user globals: `units` (wire unit id → namespace, plus a definition's
  export kinds for 13.7's stubs; definition blocks and modules, never actions), handles (11.1), V2 function stubs (11.8)
  and the release queue. Finalizers only append to that queue, because they can run in the middle of `send`.
- **Errors** (`err.error`, 9.6): `type` is `ForeignError`, or the kind the adapter detected itself (`ImportError`,
  `NameError`, `ValueError`, `MarshalError`, `ProtocolError`, `Error`); `foreign_type` is Python `type(e).__name__`, JS
  `e.name` (the class's name when neither the error nor its class sets `name` and the inherited one is a plain value, as for `class AppErr extends TypeError {}`), else its constructor's name, or `typeof`
  for a thrown primitive; `trace` per 12.6, which can be empty (a call that fails before any of
  the user's code runs); no `text`, whose frames would be the adapter's own. A
  `V2Error` that escapes user code goes back with its `v2` tag, so it round-trips unchanged.
- **`V2Error`** (Python `class V2Error(Exception)`, a builtin; JS `class V2Error extends Error`, a non-enumerable global) is raised in foreign code when a
  call through `host_call` fails (9.6) or is refused (9.8), and by a same-session stub that refuses a call (13.7). It
  exposes `type`, `message`, `data`, `lang` and `foreign_type` from the `error` tag (10.3). One decoded from
  the host reads as a log shows it: in JavaScript its `stack` is `V2Error: IOError: disk hiccup` (no type for
  `Error`), `    at ` and the tag's first `trace` entry (where V2 raised it), then the stack's frames of this
  request without the adapter's, `node:` and `evalmachine.` ones; in Python, a failed `host_call` raises it
  with its traceback starting there (the serve loop's frames dropped), and on 3.11+ with the note
  `raised in V2 as IOError, at flaky (logs.v2:3)`. 12.6's frames ignore both (they aren't V8 or Python frames).
- **Output** follows 9.9. Every string sent as text is first made well-formed (10.4).

### 14.3 py.py

All code runs under `if __name__ == "__main__":`, so a `multiprocessing` spawn child (the default on Windows and macOS)
that re-imports `__main__` starts no second adapter. Such a child (spawn or forkserver) runs the file as `__mp_main__`,
which only adds a finder to `sys.meta_path`: a function or class it was sent from a block is pickled by its block's
module, which a new Python can't import, so `__v2_…` names fail with `ModuleNotFoundError: No module named '__v2_0_142':
it is a @py block's code, which a new Python process can't import (multiprocessing's spawn and forkserver start one):
put the functions and classes you send to other processes in a .py file next to your .v2 file, and import that in the
block` (a `Pool` then waits forever, as Python's does for any worker that dies; a `ProcessPoolExecutor` breaks). The
worker's own finder (setup 3) says the same for a `__v2_…` name it hasn't loaded, as a pickle of another run's block
names: `… that is how V2 names a @py block's code, for its place in one run, so nothing else can import it (pickle saves
where each class it saved came from): define what you pickle in a .py file next to your .v2 file, and import that in
the block`. Setup, in order:

1. Import every stdlib module the adapter uses (`sys os io json ast types builtins inspect importlib keyword traceback
   linecache weakref threading signal base64 codecs collections`, plus `msvcrt ctypes` on Windows) before the project root or any `.v2`
   file's folder is on `sys.path`, so a project's `json.py` cannot replace them (an inherited `PYTHONPATH` is already
   there: the user's own setting, 20.2); `asyncio`, `importlib.metadata` and `importlib.util` (13.5's package hint) are
   imported on first need with the original path (9.3).
2. Protocol, stdio and signals exactly as 9.9: the protocol moves to non-inheritable `os.dup` copies of fds 0 and 1
   (binary mode on Windows); fd 0 becomes `os.devnull` and `os.dup2(2, 1)` points fd 1 at stderr (plus `SetStdHandle` on
   Windows); `sys.stdout` and `sys.stdin` are replaced; SIGINT (and SIGBREAK) are ignored, since the host owns Ctrl+C
   (ch. 8). C extensions, `os.write(1, …)`, `os.system` and inheriting `subprocess` children therefore write to stderr
   and read EOF. `threading.excepthook` and `sys.unraisablehook` print as Python's own do, with the frames 12.6 keeps.
3. Remove the adapter's own folder (`sys.path[0]`, i.e. `<run>/adapters`). `init` then puts the project root on
   `sys.path`, and each `.v2` file's first `load` or `module` inserts that file's folder before it (9.3); the venv's
   packages and an inherited `PYTHONPATH` are Python's own. Append the `BlockNames` finder to `sys.meta_path` (above).

- **Framing.** `recv` reads `os.read(IN, 65536)` into a bytearray and splits on `b"\n"` only, so a `\r` inside a string
  cannot split a frame. `send` writes `json.dumps(obj, ensure_ascii=False, allow_nan=False,
  separators=(",", ":")).encode() + b"\n"` in an `os.write` loop under 9.9's lock. Only `send` takes that lock. While the host serves one of this worker's calls it is not reading, so a non-dispatch thread that flushes `out` then may block on a full pipe while holding the lock; the dispatch thread keeps reading stdin in its stub's `serve(id)` meanwhile and needs the lock only to send its next frame, after the host has written to it and is reading again.
- **Calls** run `f(*args, **kwargs)`. `BaseException` is caught per request, so `sys.exit()` in a call or a block is an
  `err` (`ForeignError`, foreign_type `SystemExit`), not a dead worker. Awaitables run per 9.8 (one kept session
  loop; `err` when a loop already runs in the thread).
- **Python 3.10** is the floor for the adapter itself: nothing from 3.11+ (`except*`, `tomllib`, PEP 701 f-strings).
  `FrameSummary.colno` is 3.11+, so on 3.10 traceback frames map lines only (`col: null`, 12.6).

### 14.4 js.js

Setup, in order, before anything touches `process.stdout` or `process.stdin`: their getters create libuv handles on the
protocol fds, which makes a POSIX pipe non-blocking (`writeSync` would then fail with `EAGAIN`) and may read ahead on
fd 0 (9.9).

1. `require` the built-ins used: `fs vm v8 path module stream string_decoder url util child_process`; get `gc` once,
   through `v8.setFlagsFromString("--expose-gc")` and `vm.runInNewContext("gc")`.
2. Stdio exactly as 9.9: the `process.stdout`, `stderr` and `stdin` getters are replaced (`console`'s
   `_stdout` and `_stderr` are set to the adapter's streams: Node's console binds to `process.stdout` at its first use,
   which a `NODE_OPTIONS` preload may already have made, and this keeps it the one `require('console')` gives, with
   methods a preload took for itself following; `trace`, its own and `Console.prototype`'s, prints the frames 12.6 keeps
   through `error`), and a write takes what Node's does: bytes of any view and strings in any encoding go through the
   stream's UTF-8 decoder, a sequence it still holds ending as U+FFFD before a UTF-8 string or a reply, `fs` calls on fds 0–2 are wrapped (also the ones Node routes past `readSync`/`writeSync`:
   `readFile(Sync)`, `readv(Sync)`, `writeFile(Sync)`, `appendFile(Sync)`, `writev(Sync)`), `child_process` stdio is
   rewritten at every index, `fork`'s default (`inherit` plus `ipc`) included, so children never inherit fd 0 or 1
   (their output goes to stderr); the wrappers keep their originals' properties (`util.promisify.custom`). Then
   `module.syncBuiltinESMExports()`. Frames are written only through the
   saved `fs.writeSync` on the protocol fd (9.9), the adapter's private channel, which stays blocking. Frames are read
   in 1 MiB chunks, only new bytes are searched for the line break, and a frame's chunks are joined once, so a large
   frame costs time in step with its size. On Windows, native
   addons that write fd 1 are not covered; after `hello` their lines are a `ProtocolError` (9.11).
3. `process.exit` is replaced by a function that throws an `Error` (9.7's wording), so a block cannot end the worker;
   the original is kept for `bye` (9.10). A thrown value that isn't an error gets its message as `str(h)` shows it
   (11.3: `{ code: 42 }`, not `[object Object]`).
4. `process.on('SIGINT' | 'SIGBREAK', () => {})`; `uncaughtException` and `unhandledRejection` print the error to stderr,
   after `uncaught exception: ` or `unhandled rejection: `,
   with the frames 12.6 keeps, by line, then an `AggregateError`'s errors (from any realm, 8 levels at most) and up to 8 causes (`caused by:`), each error once (`(shown above)` after that); a `V2Error`
   from V2 adds its type and the V2 frame that raised it (a plain `Error` isn't named twice);
   the same text again right after it is not printed: once `(the same error again; further repeats are not shown)`, then nothing,
   and the worker keeps running (0.1).

The loop is 14.2's: an async top level that blocks in `fs.readSync` between requests, with nested `serve(id)` fully
synchronous; event-loop turns and Promise awaiting follow 9.8. An awaited Promise that can never settle (the
event loop runs dry, Node's `beforeExit`) is answered with `err` type `Error`: `the block's value is a Promise that
can never settle: nothing is left for it to wait on` (for a call, `the result is a Promise…`), and the worker keeps serving. The event loop cannot turn while `fs.readSync` blocks,
so after the reply to every outermost (stack depth 1) request, before the next `fs.readSync`, the loop yields one
event-loop turn (`await new Promise(r => setImmediate(r))`). If more than 200 `fn` stubs were created since the last
collection, it then calls `gc()` synchronously (it takes longer the more the heap holds; a minor collection would not
free the stubs, and fewer collections would let V2 keep more of what the stubs' functions hold) and yields one more turn: V8 keeps a new `WeakRef`'s target (every new stub,
through the stub map) alive until the job that made it ends, and runs `FinalizationRegistry` callbacks only after a
collection (11.9). During this turn the stack is empty, so a timer or event handler that calls V2 raises 9.8's idle-call `V2Error`; the
`FinalizationRegistry` cleanups run, and their queued `release` goes out with the next frame. The turn sends nothing and never waits on the host; a frame the host writes during it is read by the next `fs.readSync`. Calls use
`Reflect.apply`/`Reflect.construct` as 11.3 decides; named arguments never arrive, since js declares no `kwargs` cap and
the host raises `ArityError` before sending (10.8).

- `entryUrl(spec, from)` (13.5): a built-in (`node:` or in `Module.builtinModules`) stays as is; anything else →
  `pathToFileURL(createRequire(from).resolve(spec))`, the file `require` would load.
- **Node 22.13** (not 23.0 or 23.1, 7.2) is the floor for js and ts alike: `isWellFormed`/`toWellFormed` (10.4),
  require(esm) (13.5) and `module.stripTypeScriptTypes` (12.5) need no fallback.

### 14.5 Custom bridged adapters

A custom `bridged` engine (DOCS 13.2) can port either file. The host relies on 17.6's checklist. The adapter declares in
`hello` only the caps it implements, and the host raises 17.5's defined errors for the rest, so an adapter may skip
`module` (no `modules`), `op`/`release` (no `handles`), kwargs, or stubs (no `reverse`). Units keep 12.2's contract: a
fresh namespace per action run, a persistent one per definition, `exports`/`missing` in the reply. Line mapping: the
`.v2` path as filename and `line - 1` as the line offset where the language allows it (Ruby:
`eval(src, binding, file, line)`), else `line - 1` prepended blank lines; `col: null` when unknown.

---

## 15. Compiled blocks: the build bridge

A compiled block (`@c`, `@cpp`, `@rust` and their aliases; kind Compiled, 7.2) never speaks VEP. With `@export` it is in **library mode**: built into a native library, cached under `~/.v2/cache/build`, loaded into the host with `libloading` and called through `ffi/call` (16.7). Without `@export` it is in **program mode** (15.5). Code: `build/{mod, cache, c, cpp, rust, shims}`; `mod` holds the pipeline, the per-run memo, loading, binding and `host_call`. Messages name a block with 12.1's `block_name`: `block 'fast'`, or `@rust block at main.v2:12` when it has no label.

### 15.1 Pipeline

This is 12.1's step 6: 12.1 has already checked the unit state, resolved the tag and routed the inner imports, and afterwards registers the exports and produces the value. Compiled blocks take no block args, and their directive mistakes (15.2) are static `SyntaxError`s (4.5).
1. **Body** (4.4): the dedented text with directive lines blanked, its `UnitLoc` (2.3), the export items and the inner imports.
2. **Signatures** (15.2, 15.3): text analysis only, no toolchain.
3. **Generation** (15.4, 15.5), then the **cache** (15.6). A hit goes straight to step 5 and touches no compiler, so a cached block works with none installed.
4. On a miss, the **toolchain** (7.6, including the target check; none usable: `EngineNotFound`), then the **build** (15.7).
5. Library mode: **load and bind** (15.8). Program mode: **run** (15.5).

Steps 1–4 are memoized for the run in `Memo: HashMap<UnitId, Result<Rc<Artifact>, Rc<ErrorValue>>>`, failures included: a program block in a loop builds once and runs on every reach, and a failed build is re-raised on later reaches without rebuilding (a definition block is already in 12.1's `Failed` state).

### 15.2 Exports and inner imports

The items are 4.3's `Exports { items, star }`: each explicit item follows its row below, and when `star` is set the `*` row adds every other function, so each function is exported once. Each item resolves to a name, a `Sig` (16.4) and its span. Compiled blocks export **functions only**, by name: a quoted item is a static `SyntaxError` (4.5), a function V2 cannot read or bind needs a wrapper function with a plain signature, written in the block, that calls it and is exported instead, and a constant's name counts as "not found" (constant exports are bridged engines only, 12.2):

| Item | Found, supported | Unsupported type | Several definitions | Head unreadable | Not found |
|---|---|---|---|---|---|
| `name` | exported | `BuildError` naming the parameter and its type | `BuildError: 'f' has several definitions — write a wrapper with a plain signature` | `BuildError: cannot read the signature of 'f' — write a wrapper with a plain signature` | `ImportError: block 'fastmath' exports 'x', but does not define it as a function` |
| `*` | exported, except `main` and `static`/`inline` functions | skipped | skipped | skipped | — |

- *Several definitions*: a C++ overload set, or C/C++ definitions that differ under `#if`/`#else`. Definitions with identical lowered signatures (the usual `#ifdef _WIN32` pair) count as one.
- *Head unreadable*: a definition head naming the function exists (its name found as 15.3 says), but `cdecl` rejected it: macros, templates, generics.
- *Skipped by `*`*: signature extraction (15.1 step 2) records why each function was skipped: `parameter 1 has unsupported type int*`, `the result has unsupported type int*`, `it has several definitions`, `its signature cannot be read`, `it is static`, `it is inline`, `it is main`. When an `@import` finds no export for a name (13.3) and a candidate is a loaded compiled block with `*`, the first such block decides: `ImportError: 'f' was left out of @export { * } in block 'fast': parameter 1 has unsupported type int*`, or, if it skipped no function of that name, `ImportError: block 'fast' has no top-level function 'f'`.
- A name exported twice, or one that is not a valid, non-keyword identifier of the block's language, is a static `SyntaxError` (4.5).

**Inner imports.** Compiled blocks take only `@import { a, b as c } from SEL` (the wrapper list must be fixed before building, and compiled code cannot hold handles), and only in library mode (a program runs as a separate process); the other forms, and local names that are not valid identifiers of the block's language, are static `SyntaxError`s (4.5).
- **Library mode:** one generated wrapper per local name (15.4).
- **Routing:** 12.1 step 5 gives each item a route (13.6), so blocks may import each other in any order. `CompiledUnit.routes: HashMap<Rc<str>, RouteId>` is keyed by local name: wrappers pass the name, never a route id, because a cached library outlives the run.
- **Reserved names** (a clash is an ordinary compile error): C/C++ identifiers starting with `v2_`, `V2_` or `__v2`; the Rust module `v2` and Rust items starting with `__v2`.

### 15.3 Signature extraction

**C/C++** (`build/c.rs`). The body goes through the cdecl tokenizer (16.3); the C++ dialect adds `::`, `R"d(…)d"` and `[[…]]`. Comments are dropped, and a preprocessor line with its `\` continuations is one token. A depth-0 walk collects `head` tokens while tracking parentheses:
- A `;` at paren depth 0, or a preprocessor token, clears `head` (prototypes, globals, `#if` lines).
- A `{` at paren depth 0 ends a **definition head** when `head`, with attribute groups (`__declspec(…)`, `__attribute__((…))`, `[[…]]`, `alignas(…)`) and calling conventions (16.3's `callconv`: `__cdecl`, `__stdcall`, `__fastcall`, …) left out, has a depth-0 `(` and no depth-0 `=` (so `struct S {`, `namespace n {`, `extern "C" {` and initializers are not heads), the identifier just before its first depth-0 `(` (the function's name) is not a C/C++ keyword, and at least one token (a return type or specifier) comes before that identifier. A head starting with `if`, `else`, `for`, `while`, `do`, `switch`, `return`, `try` or `catch` is never a definition head (so `for (int i = 0; i < 3; i++) {`, `else if (x) {` and a range-`for` are statements, whose `{…}` is skipped like any other). A definition head goes to `cdecl::parse_definition_head`: parameter names are optional; `static`, `inline`, `template<…>` and C++ `extern "C"` are recorded; `const`, `noexcept` and attributes after the `)` are allowed.
- After every `{` (a head or not), the matching `}` is skipped by counting braces and `head` is cleared, so bodies, statement blocks and the contents of `struct`, `namespace` and `extern "C" { }` are never scanned; to export a function inside them, write a top-level wrapper function in the block that calls it, and export the wrapper.

**Rust** (`build/rust.rs`). A small tokenizer handles `//`, nested `/* */`, `"…"`, `r#"…"#`, byte and char literals (`'a` not followed by `'` is a lifetime) and feeds a depth-0 item walk: items start after `;` or `}`, and `#[…]`/`#![…]` groups are skipped.
- An item whose first non-qualifier token is `fn` is read as `fn NAME ( params ) [-> TYPE] {`. Each param is `IDENT: TYPE`, `mut IDENT: TYPE` or `_: TYPE`; a type is the tokens up to the next depth-0 `,` (counting `()`, `[]`, `<>`; `->` is not a bracket). Types, `-> TYPE` included, are matched against the table below as token lists, not joined strings: a lifetime after `&` is ignored (`&'static str` is `&str`), and `&mut str` is unsupported.
- The qualifiers `pub`, `pub(…)`, `const` and `extern "C"` are allowed; `unsafe`, `async`, `<…>` generics and `where` make the head unreadable. Any other depth-0 `{` (`impl`, `mod`, `macro_rules!`) is skipped by brace matching.

Supported types (DOCS 11.2) and their lowering to `ffi::CType` (16.4):

| C / C++ | Rust | Parameter | Result |
|---|---|---|---|
| `char` `short` `int` `long` `long long` `unsigned …` `int8_t`…`uint64_t` `size_t` `ssize_t` `intptr_t` `uintptr_t` `ptrdiff_t` | `i8`…`i64` `u8`…`u64` `isize` `usize` | `Int{size,signed}` | same |
| `float`, `double` | `f32`, `f64` | `F32` / `F64` | same |
| `bool` | `bool` | `Bool` | `Bool` |
| `const char*` | `&str` (any lifetime) | `Str` (C/C++: `null` allowed as NULL; Rust: non-null) | C/C++: `Str` (copied, never freed); Rust `&str`: unsupported |
| `char*` | — | unsupported | `OwnedStr` (copied, then `__v2_free`d) |
| `std::string`, `const std::string&` | `String` | `Str`, non-null (the shim builds the string) | `OwnedStr` (the shim returns a heap copy) |
| `void` | `()` or none | — | `Void` |

Everything else is unsupported: other pointers, references, arrays, structs, `long double`, `wchar_t`, Rust `char`/`i128`/`Option`/`Vec`/slices, and `-> !`. A non-null parameter given `null` is `MarshalError: argument 1 ('s', std::string) of 'reversed': null is not a string`.

### 15.4 Library mode: generated sources and shims

Generated text is **position-independent**: it never contains the `.v2` path, absolute line numbers or any runtime value, so moving a block, or editing lines above it, keeps its cache key. Positions are applied when diagnostics or panics are reported, through the block's current `UnitLoc` (2.3).
- **C/C++** (`unit.c`/`unit.cpp`): `#include "v2.h"`, `#line 1 "__v2_body"`, the body, `#line 1 "__v2_gen"`, in C++ `<string>` `<exception>` `<typeinfo>` `<cstdlib>` `<cstring>`, the shims, then `__v2_init`, `__v2_free` and `__v2_flush`.
- **Rust** (`lib.rs`): the body on lines 1..N (Rust has no `#line`, and item order is free), then one crate-root wrapper per inner import, `mod v2`, the shims and the three `__v2_*` functions. Generated Rust uses absolute paths (`::std::ffi::CString`), so user `use` lines cannot clash.

**Shims.** Each export `f` becomes `RET __v2x_f(P1 v2_a1, …, Pn v2_an, char** v2_err)` with the C ABI and the lowered types:
- **C:** a `V2_EXTERN_C V2_EXPORT` function returning `f(v2_a1, …)`; it is in the same translation unit, so `static` functions work. `V2_EXPORT` is `__declspec(dllexport)` on Windows and `__attribute__((visibility("default")))` elsewhere, where builds pass `-fvisibility=hidden`, so only `__v2x_*` and `__v2_*` are exported.
- **C++:** an `extern "C"` shim whose body is `try { … } catch (const std::exception& e) { … } catch (...) { … }`; a catch fills `*v2_err` and returns a zero value. `std::string` parameters are built from the `const char*`; `std::string` results are copied into a `malloc` buffer (one holding a NUL byte sets `V2:MarshalError` instead).
- **Rust:** `#[no_mangle] pub extern "C" fn` (edition 2021, where plain `#[no_mangle]` is valid on every stable rustc; `#[unsafe(no_mangle)]` needs 1.82), its body inside `std::panic::catch_unwind(AssertUnwindSafe(…))`. `&str` parameters come from `CStr::from_ptr(p).to_str()` (`String` adds `.to_owned()`); `String` results leave as `CString::new(s)` then `.into_raw()` (a NUL byte gives `V2:MarshalError`).

**Error slot.** `*v2_err` is a library-allocated string `TYPE "\n" LINE "\n" MESSAGE`:

| Source | TYPE | MESSAGE |
|---|---|---|
| C++ `std::exception` | `typeid(e).name()` as is: readable on MSVC (`class std::runtime_error`), mangled on gcc/clang (`St13runtime_error`) | `e.what()` |
| C++ `catch (...)` | `unknown` | `unknown C++ exception` |
| Rust panic | `panic` | the `&str`/`String` payload, else `Box<dyn Any>` |
| returned string containing a NUL byte | `V2:MarshalError` | the NUL's position |

`LINE` is a Rust panic's body line, or empty. The host copies the string, releases it with the library's `__v2_free`, and raises the named V2 kind or `ForeignError{lang: "cpp"|"rust", foreign_type: TYPE, message}` with the frame `at f (main.v2:L, in @rust fast)` (2.5), where L is the mapped panic line, else the block's first line. The library stays loaded and usable.

**Rust panics.** `__v2_init` installs a panic hook (`set_hook` affects only this cdylib's copy of std). While the thread is inside a shim, the hook prints nothing and records `location().line()` when `location().file()` is the generated `lib.rs`; elsewhere it prints the usual message. A user flag `-C panic=abort` defeats `catch_unwind`, and the process then aborts.

**String ownership** (DOCS 11.2). Parameters are owned by the host and valid only during the call. A `const char*` result is copied and never freed, so it must stay live (a literal or a static buffer). A `char*` result is copied, then passed to `__v2_free` (`free` of the library's own CRT), so it must come from `malloc`. `std::string`/`String` results are heap copies made by the shim, freed the same way (Rust's `__v2_free` is `drop(CString::from_raw(p))`). NULL becomes `null`; invalid UTF-8 is `MarshalError`.

**`v2.h`** is generated per unit. Besides the code below, it includes `<stdint.h>`, `<stddef.h>`, `<stdlib.h>`, `<string.h>`, `<stdio.h>` and, in C, `<stdbool.h>`, and it defines `V2_EXPORT` and `V2_EXTERN_C`.

```c
typedef struct v2_host_api {
    uint64_t unit;                        /* host token for this library's unit */
    const char* (*call)(uint64_t unit, const char* route, const char* args_json);
    const char* (*last_error)(void);
} v2_host_api;
static const v2_host_api* v2__api;
static inline const char* v2_call(const char* name, const char* args_json) {
    return v2__api ? v2__api->call(v2__api->unit, name, args_json) : NULL; }
static inline const char* v2_last_error(void) {
    return v2__api ? v2__api->last_error() : "V2 host API not initialized"; }
/* one per inner @import item, named by its local name: */
static inline const char* mean(const char* args_json) { return v2_call("mean", args_json); }
```

The generated tail defines `void __v2_init(const v2_host_api*)`, which stores the pointer, `void __v2_free(char*)`, and `void __v2_flush(void)`, which calls `fflush(NULL)` in the library's own CRT. There is no version handshake: a change to `v2.h` changes the prelude hash, and any other change to the host contract bumps `v2abi` (15.6), so either one rebuilds. Rust's `mod v2` mirrors this:
- a `#[repr(C)] HostApi` and an `AtomicPtr<HostApi>` set by `__v2_init`; `__v2_flush` flushes `std::io::stdout()`, ignoring errors;
- `pub fn call(name: &str, args_json: &str) -> Result<String, String>`: a NUL in either argument is `Err`, NULL becomes `Err(last_error)`, otherwise the result is copied at once;
- per import, a crate-root `fn mean(args_json: &str) -> Result<String, String> { v2::call("mean", args_json) }`.

The per-import functions and `v2_last_error` are what DOCS 11.6 documents; `v2_call` and `v2::call` are internal helpers of the per-import functions.

### 15.5 Program mode (no `@export`)

A block has a `main` when 15.3's scan finds a top-level function named `main` (a C/C++ definition head, or Rust `fn main`); it is then compiled as is (C/C++ behind `#line 1 "__v2_body"`). A C/C++ block without `main` may contain statements only: if the scan finds any other top-level function definition (a 15.3 definition head; braced `if`/`for`/`while`/`switch` statements are not), it is `BuildError: @c block at main.v2:12 defines functions but no main — write main yourself to use helper functions`. A Rust block without `main` may also define helper functions and other items, which are legal inside `fn main`. V2 wraps a block without `main`:
- **C/C++:** `#include <stdio.h>`, `<stdlib.h>`, `<string.h>`; each top-level `#include` line of the body, preceded by `#line K "__v2_body"`; `int main(void) {` and `#line 1 "__v2_body"`; the body with those `#include` lines blanked; `return 0; }`.
- **Rust:** `fn main() {`, the body, `}`; the line map is "generated line − 1". `use` declarations are legal inside a function body, which matches keeping `use` lines at the top (DOCS 11.4).

The executable runs from the cache entry through the run-engine process helper (17.3 steps 3–5 and 7), under 8.3's child rules, with the engine's `call_timeout`. Stdin, stdout (streamed in statement position, the value in expression position), the stderr tail, the tree kill and `EngineExitError` (`@c block exited with status 3`) follow 6.3 and 17.3.

### 15.6 The build cache

The key is `k = hex(sha256(serde_json::to_string(&key)))` over a struct, so the field order is fixed and there is no whitespace:

```json
{"v2abi":1,"engine":"rust","mode":"lib","target":"x86_64-pc-windows-msvc","command":null,
 "flags":["--crate-type","cdylib","--edition","2021","-C","opt-level=2","-C","target-feature=+crt-static",
          "--crate-name","v2block","--error-format=json","{src}","-C","opt-level=3","-o","{out}"],
 "prelude":"<sha256 of all generated text except the body>","source":"<dedented body>",
 "ordinal":0,"headers":[]}
```

`target` is V2's own target triple, and `command` the engine's configured `command` as written, or `null`. No compiler path or version is in the key, so a lookup runs nothing, and updating a compiler rebuilds nothing until `v2 cache clean` (19.4). `flags` is the exact argv including user flags, with the input and output paths replaced by `{src}`/`{out}`. `prelude` covers the shims and wrappers, so a changed export or import list, or a generator change, rebuilds; exports are re-derived from the source on every run. `ordinal` counts the units loaded earlier in this run, from any file, whose key would otherwise be equal, so two identical blocks never share a library: each gets its own entry, `static`s (DOCS 11.3) and host-API token. `headers` (C/C++; `[]` for Rust) is the sha256 of each local header's contents, in first-include order, each once: every quoted `#include "x"` that resolves against the including file's folder (for the body, the `.v2` folder), then the `.v2` folder, followed recursively; other includes are the compiler's business. The entry directory `~/.v2/cache/build/<k>/` holds only:
- the artifact: `v2_<k[0..16]>.dll|.so|.dylib`, or `prog[.exe]`;
- `build.log`: the argv, the full compiler output and any warnings.

Lookup and build:
1. It is a hit if the artifact exists.
2. On a miss, build in `~/.v2/cache/build/.tmp-<k>-<pid>-<rand>/` (created 0700 on Unix), delete the generated sources and the other compiler outputs (cl's `.obj`, `.lib`, `.exp`) so only the artifact and `build.log` remain, and rename it to `<k>`; if `<k>` appeared in the meantime, keep it and delete ours. A failed build's temp dir is deleted: failed builds are never cached.

There is no lock: an entry appears only through one atomic rename, so concurrent builders may both compile, and the first rename wins. A cached library with a damaged image or missing symbols is evicted and rebuilt once (15.8).
- **Sweeping.** `.tmp-*` dirs older than a day are deleted, best effort, at the start of every build. `v2 cache clean` (19.4) is the one reset; it deletes entries one by one, and an entry it cannot delete (on Windows, a library a running V2 has loaded) is skipped and listed.
- **No usable `~/.v2`** (18.1): builds run in `<run>/build/<k>/` (20.3), libraries load from there, and nothing is cached.

### 15.7 Toolchains, command lines, diagnostics

**Process.** The compiler is 7.6's resolved engine (candidate order, target check, cl's captured environment) and arrives as 7.1's `argv`, `version`, `target` and `env` (cl's `INCLUDE`, `LIB`, `LIBPATH` and `PATH` prefix). Compilers run in the build dir (15.6) with relative file names and null stdin, under 8.3's child rules; stdout and stderr are read concurrently (8.3) and both kept (cl writes diagnostics to stdout). cl gets `env`, and diagnostics are forced to English with `LC_ALL=C` (gcc/clang) or `VSLANG=1033` (cl). A compiler that cannot be spawned is `EngineNotFound: could not start '<path>': <os error>` (8.3). In the table, `OUT` is `v2_<k[0..16]>`, `DIR` the absolute path of the `.v2` file's folder and `FLAGS` the user's `[engines.<tag>] flags`:

| Toolchain | Library mode | Program mode |
|---|---|---|
| `c`, Linux (`cc`, `gcc`, `clang`) | `-std=gnu11 -O2 -shared -fPIC -fvisibility=hidden -Wl,--no-undefined -IDIR -Wl,-rpath,DIR unit.c FLAGS -o OUT.so -lm` | `-std=gnu11 -O2 -IDIR unit.c FLAGS -o prog -lm` |
| `c`, macOS (`cc`, `gcc`, `clang`) | `-std=gnu11 -O2 -dynamiclib -fvisibility=hidden -IDIR -Wl,-rpath,DIR unit.c FLAGS -o OUT.dylib` | `-std=gnu11 -O2 -IDIR unit.c FLAGS -o prog` |
| `cpp`, Linux and macOS (`c++`, `g++`, `clang++`) | that OS's `c` row with `-std=gnu++17`, `unit.cpp`, no `-lm` | same |
| `c`, `cpp`, Windows (`cl`) | `/nologo /LD /O2 /utf-8 /std:c11 /TC /IDIR /Fe:OUT.dll unit.c FLAGS`; C++: `/std:c++17 /EHs /TP` and `unit.cpp` | no `/LD`; `/Fe:prog.exe` |
| `rust` (`rustc`) | `--crate-type cdylib --edition 2021 -C opt-level=2 [-C target-feature=+crt-static] [-C link-arg=-Wl,-rpath,DIR] --crate-name v2block --error-format=json lib.rs FLAGS -o OUT.<ext>` | `--crate-type bin`, `main.rs`, `-o prog[.exe]`, no rpath |

- User flags come after V2's own flags, so they win (`-O3`, `-C opt-level=3`), and after the source, so `-lfoo` links. For cl, FLAGS come last because a `/link …` part must end the command line.
- Relative paths in FLAGS (`./inc`, `-I../include`) are made absolute against the `v2.toml`'s folder first (18.1).
- `-Wl,--no-undefined` turns a missing symbol into a link error instead of a later `dlopen` failure.
- `DIR` makes files next to the script work: `#include "foo.h"` finds them, and a library build's own dependencies (a DLL or shared library linked through FLAGS) load from there: on Linux and macOS through the rpath (rustc gets its `-C link-arg` only there), on Windows through 15.8's `SetDllDirectoryW`.
- cl keeps its default static CRT (`/MT`), and rustc on `*-windows-msvc` gets `-C target-feature=+crt-static`, so no artifact needs a redistributable DLL. Each library therefore has its own heap and stdio, which is why `__v2_free` and `__v2_flush` are per library.
- `/EHs` (not `/EHsc`) keeps the shim's `catch` effective around a user function declared `extern "C"`. `/std:c11` needs Visual Studio 2019 16.8 or later (7.2's minimum).

**Diagnostics.** The full output always goes to `build.log`. For the error, V2 maps:
- gcc/clang lines matching `^(__v2_body|__v2_gen):(\d+):(?:(\d+):)? (fatal error|error|warning|note): (.*)`;
- cl lines matching `^(__v2_body|__v2_gen)\((\d+)(?:,(\d+))?\): (fatal error|error|warning) (\w+): (.*)`;
- rustc stderr lines that parse as JSON with `"$message_type":"diagnostic"`: `level`, `message`, `code.code`, `rendered`, and the `is_primary` span's `file_name`/`line_start`/`column_start`.

A body position maps to a `.v2` position as 2.3 says: line `body_first_line + line − 1`, column through `UnitLoc::col`. `__v2_gen` positions, and Rust lines past the body, are reported at the `@export`/`@import` item that produced them, as `in generated wrapper for 'f': …`. Lines about other files (system headers), span-less diagnostics and linker lines (`undefined reference to 'sqrt'`) are kept verbatim. Warnings are never printed. The resulting `BuildError` has the first mapped error as its span, the message `{block_name} failed to compile: <first error>` (`block 'fast' failed to compile: …`), and `data = {diagnostics: [{file, line, col, severity, message}], output: <raw output, at most 64 KiB>}`; uncaught, it prints up to 20 diagnostics (2.4).

### 15.8 Loading, binding and calling

```rust
struct LoadedLib { path: PathBuf, lib: libloading::Library, compiled: Option<CompiledLib> }
struct CompiledLib { token: u32, free: unsafe extern "C" fn(*mut c_char), flush: unsafe extern "C" fn() }
```

1. **Load** by absolute path as a full-path library (16.2). On Windows, V2 calls `SetDllDirectoryW(DIR)` (15.7) just before the load and `SetDllDirectoryW(L"")` right after, whatever the outcome, so dependencies are searched in the standard order with DIR in the slot the cwd would have: the library's folder, DIR, the system folders, then `PATH`. If a library served from the cache has a damaged image (Windows error 193 or 1006; dlopen's `invalid ELF header` or `file too short`; macOS `not a mach-o file`), V2 evicts the entry (deletes `<k>/`) and rebuilds once. Any other load failure, such as a missing dependency (Windows error 126, `cannot open shared object file`), and any failure after a fresh build, is `FFILoadError` at once with the OS text (16.2).
2. **Resolve** `__v2_init`, `__v2_free`, `__v2_flush` and every `__v2x_<name>`. Missing symbols evict and rebuild once like a damaged image; after a fresh build they are `FFILoadError` naming them in 16.1's `not found in library …` form.
3. **Init.** `Box::leak` a `HostApi{unit: token, call: host_call, last_error: host_last_error}` and call `__v2_init`. `token` indexes `Vec<CompiledUnit{routes, loc: UnitLoc, name, active: Cell<u32>}>`.
4. **Bind** each export as `Value::Func(Rc<Callable::Native{lib, sym, sig}>)` (5.1) with `sig.err_slot = true`, registered like a bridged export (13.2). Calls take positional arguments only (5.7).

Libraries are never unloaded. Because `lib.compiled` is `Some`, the call path (16.7) adds the error slot, `ACTIVE` and the unit's `active` count (15.9), the library's flush target and `__v2_free` for `OwnedStr` results.

### 15.9 Reverse calls (`host_call`)

`extern "C" fn host_call(unit, route, args_json) -> *const c_char` is wrapped whole in `catch_unwind` and runs these steps:
1. **Guards** (DOCS 8.4). V2 may be called only from the thread that called into this library, while such a call is running: the thread-local `ACTIVE: Cell<*mut Interp>` must be non-null (it is null on another thread, and once the V2→native call has returned: an atexit handler, a stored pointer) and `units[unit].active` must be non-zero. Otherwise set the error `V2 can only be called from the thread that called into this library, while that call is running` and return NULL. If `PENDING_EXIT` is set, return NULL with `exit() was called`.
2. Find `route` in `units[unit].routes`; if it is missing: `'x' is not imported by this block`.
3. Decode `args_json` in the plain mode (`codec/json`, 10.1), NULL meaning `[]`: invalid JSON is `MarshalError: args_json is not valid JSON: <detail>`, a non-array `MarshalError: args_json must be a JSON array`; `"$v2"` is an ordinary key.
4. Call the route as 13.6 does for `call {route}` (a `Pending` route resolves now or fails with its ImportError), with the span of the enclosing native call. This counts one level of 9.7's cross-language depth limit (64). No flush is needed first: the library is already dirty (16.7), and any V2 output flushes it.
5. On `Ok(v)`, encode `v` in the plain mode; bytes, functions, handles, errors and non-finite floats fail as 10.1 says (`MarshalError: a handle cannot be returned to compiled code — convert it with to_v2(h) first`) and go to step 6. Store the text in the thread-local `LAST_RESULT: Option<CString>`, replacing the previous one, and return its pointer; it stays valid until the next V2 call on this thread (DOCS 11.6).
6. On `Err(e)`, set `LAST_ERROR = "Type: message"` (e.g. `ForeignError: ZeroDivisionError: division by zero`) and return NULL. `exit(code)` also sets `PENDING_EXIT`, and a Rust panic inside the interpreter sets `PENDING_PANIC`.

`host_last_error` returns `LAST_ERROR`, or `""`. After `Cif::call` returns, `call_native` re-raises a pending exit (the interpreter's exit signal, 5.8) or panic (`resume_unwind`, exit 101). A `host_call` further out turns it into NULL again, so neither crosses C frames and both reach the top.

**Why this is sound.** The interpreter is single-threaded. Immediately before `Cif::call`, `call_native` saves `ACTIVE` and sets it to a raw pointer derived from its own `&mut Interp`, restoring it right after (a stack for nested calls). By then all arguments are in owned storage, every `RefCell` borrow is released and the callee's `Rc` is cloned, so no reference into interpreter state lives across the call, and the caller's `&mut Interp` stays unused until `Cif::call` returns. So the `&mut *ACTIVE` created in `host_call` is the only live access to interpreter state, and it ends before `host_call` returns. Other threads read null and never touch the interpreter.

---

## 16. Native FFI layer

`extern` binds functions of an existing native library, described with C declarations, and calls them through libffi. Code: `ffi/cdecl.rs` (the declaration parser, shared with 15.3), `ffi/types.rs` (`CType` and platform sizes), `ffi/lib.rs` (library resolution and loading) and `ffi/call.rs` (the call path, shared with compiled blocks). Only the 64-bit targets of ch. 1 are supported: `ffi/types.rs` has `#[cfg(not(target_pointer_width = "64"))] compile_error!`.

### 16.1 The `extern` statement

The lexer raw-captures the body with the CLike(C) profile (3.9); the parser runs `cdecl::parse_extern` at once, filling 4.3's `Extern.decls`, and lowers each declaration to a `Sig` (16.4). So every declaration error, including a function declared twice in one body, is a `SyntaxError` at the exact token (2.4), reported before anything runs and by `v2 check`. The first reach runs steps 1–3 and caches their callables on the statement node (a failure caches nothing, so the next reach tries again); every reach runs step 4:
1. Resolve and load the library (16.2).
2. Look up every symbol. All missing names go into one error: `FFILoadError: not found in library 'm' (/lib/x86_64-linux-gnu/libm.so.6): cosf128, foo`. On any failure nothing is bound.
3. Make one `Callable::Native{lib, sym, sig}` (5.1) per declaration (`lib.compiled` is `None`).
4. Bind. Without `as`, each name is declared like a `const` in the current block; a same-block redeclaration is a static `SyntaxError` (4.5). With `as ns`, `const ns` is a new dict `{name: func}` in declaration order, the module-namespace form (6.4): `win.MessageBoxA(…)` is member access plus a call, and because a key holding a function wins over dict methods (5.5), `ns.keys()` calls a C function named `keys`.

`extern` is not hoisted: a name used before its `extern` ran is a `NameError`.

### 16.2 Library resolution

| Spec | Meaning | Candidates |
|---|---|---|
| `"c"` | C runtime | Windows `ucrtbase.dll`, `msvcrt.dll`; Linux `libc.so.6`, `libc.so`; macOS `/usr/lib/libSystem.B.dylib` |
| `"m"` | math library | Windows, macOS: as `"c"`; Linux `libm.so.6`, `libm.so`, then the `"c"` list (musl keeps libm in libc) |
| contains `/` or `\` | path | `file_dir.join(spec)` if relative (the `.v2` file's folder, never the cwd), else as is; loaded directly, no search, no suffixes |
| ends in `.dll`, `.dylib`, `.so`, `.so.N…` | file name | `[spec]` |
| otherwise | bare name `N` | Windows `N.dll`, `libN.dll`; Linux `libN.so`, `N.so`; macOS `libN.dylib`, `N.dylib` |

`"c"` and `"m"` go straight to step 2; file names and bare names go through both steps:
1. **The `.v2` file's folder** (D). The first `D/candidate` that is a file is loaded by full path, and the search **stops**: if that load fails, that failure is the error, so a present but broken library (e.g. a 32-bit one) is never silently replaced by another. On Linux, when no exact candidate exists in D, the highest-versioned `libN.so.<digits>[.<digits>…]` in D matches too.
2. **System default search.**
   - **Windows:** `LoadLibraryExW(candidate, NULL, 0)`: the standard order (the v2.exe folder, System32, the Windows folders, then `PATH`) without the current directory: `main` calls `SetDllDirectoryW(L"")` first thing at startup, so the cwd never joins any DLL search.
   - **Linux:** `dlopen(candidate)` (`LD_LIBRARY_PATH`, ld.so.cache, the default dirs), then the versioned match over `/lib/<triple>`, `/usr/lib/<triple>`, `/lib64`, `/usr/lib64`, `/lib`, `/usr/lib` and `/usr/local/lib`, so `sqlite3` finds `libsqlite3.so.0` without the `-dev` symlink.
   - **macOS:** `dlopen` of `/opt/homebrew/lib/<c>`, `/usr/local/lib/<c>` and `/usr/lib/<c>`, without existence checks, since system libraries live in the dyld shared cache. Bare-name `dlopen` is never used there: it searches the current directory.

Paths are made absolute with `std::path::absolute`, not `canonicalize` (which adds a `\\?\` prefix on Windows). Full-path loads on Windows (a path spec, a hit in D, a compiled library, 15.8) use `LoadLibraryExW(path, NULL, LOAD_WITH_ALTERED_SEARCH_PATH)`, so dependencies are searched in the library's own folder first (in place of the v2.exe folder), then System32, the Windows folders and `PATH`; a compiled library also searches its `.v2` folder right after its own (15.8); every Windows load runs under `SetThreadErrorMode(SEM_FAILCRITICALERRORS)` (libloading does this), so a bad image never opens a dialog. Unix loads use `RTLD_NOW | RTLD_LOCAL`, so missing dependencies fail at load time. Every attempt is recorded and listed on failure:

```text
FFILoadError: cannot load library 'sqlite3'
  C:\proj\sqlite3.dll: not found
  C:\proj\libsqlite3.dll: not found
  sqlite3.dll (system): The specified module could not be found. (os error 126)
  libsqlite3.dll (system): The specified module could not be found. (os error 126)
  hint: put the library next to the script or give its path
```

On Windows, error 126 for a file that exists is reworded `found, but a library it depends on could not be loaded (os error 126)`, and error 193 `not a library for this machine — 32-bit or another architecture? (os error 193)`. On Unix the `dlerror()` text is kept as is (e.g. `wrong ELF class: ELFCLASS32`).

Loaded libraries (`LIBS: HashMap<LibKey, Rc<LoadedLib>>`, keyed by absolute path or `system:<candidate>`) are **never unloaded**: function values outlive scopes, and libraries hold atexit handlers and thread-locals.

### 16.3 The C declaration parser (`ffi/cdecl`)

Two callers: `extern` bodies (a list of prototypes) at parse time (4.3), and compiled C/C++ definition heads at build time (15.3: parameter names optional; `static`, `inline` and `template` recorded). The C++ dialect adds `std::string`, `const std::string&`, `noexcept`, `[[…]]`, `extern "C"` and `::`. The tokenizer yields identifiers, integers, punctuation, `...` and literals; it skips comments and returns a preprocessor line as one token.

```text
decls      = { proto ";" }
proto      = specs declarator
specs      = { qual | ignored } base { qual | ignored }
base       = "void" | "bool" | "_Bool" | int_words | "float" | "double"
           | TYPEDEF | IDENT | ("struct" | "union" | "enum") IDENT
int_words  = any C-valid mix of signed unsigned char short int long ("long long")
declarator = { "*" { qual } } [ callconv ] IDENT "(" params ")"
params     = "void" | "" | param { "," param }
param      = specs { "*" { qual } } [ IDENT ] [ "[" [ INT ] "]" ]
qual       = "const" | "volatile" | "restrict" | "__restrict" | "__restrict__"
ignored    = "extern" | "static" | "inline" | "__inline" | "__extension__" | callconv
           | "__declspec" "(" … ")" | "__attribute__" "((" … "))" | "WINBASEAPI" | "WINUSERAPI"
callconv   = "__cdecl" | "_cdecl" | "__stdcall" | "_stdcall" | "__fastcall" | "_fastcall" | "WINAPI" | "WINAPIV" | "APIENTRY" | "CALLBACK"
```

Rules on top of the grammar:
- Calling-convention words are ignored: x64 and arm64 each have a single C calling convention. `()` means no parameters, and array parameters decay to pointers.
- An unknown `IDENT`, or `struct/union/enum X`, is allowed **only before `*`**, as an opaque pointee such as `sqlite3*` or `FILE*`.
- Known typedefs are `int8_t`…`uint64_t`, `size_t`, `ssize_t`, `intptr_t`, `uintptr_t` and `ptrdiff_t`, plus these Windows names on every OS:

| Typedef | Meaning |
|---|---|
| `BOOL`, `INT`, `LONG`, `HRESULT` | `int32_t` |
| `DWORD`, `UINT`, `ULONG` | `uint32_t` |
| `BYTE`, `UCHAR`; `CHAR` | `uint8_t`; `char` |
| `WORD`, `USHORT`; `SHORT` | `uint16_t`; `int16_t` |
| `LONGLONG`; `ULONGLONG`, `DWORD64` | `int64_t`; `uint64_t` |
| `SIZE_T`, `WPARAM`, `UINT_PTR`, `ULONG_PTR`, `DWORD_PTR` | `uintptr_t` |
| `LPARAM`, `LRESULT`, `INT_PTR`, `LONG_PTR` | `intptr_t` |
| `HANDLE`, `HWND`, `HMODULE`, `HINSTANCE`, `LPVOID`, `LPCVOID`, `LPWSTR`, `LPCWSTR` | opaque pointer |
| `LPSTR`; `LPCSTR` | `char*`; `const char*` |

Errors are `SyntaxError`s in `extern` bodies; in a definition head the same rejection makes the head unreadable (15.2):

| Input | Message |
|---|---|
| struct/union by value, or a `{` | `structs are not supported in extern — wrap the function in a @c block` |
| `enum X` by value | `enum types are not supported by value — declare it as int` |
| `...` | `variadic functions (like printf) are not supported — wrap the call in a @c block` |
| `(*name)(…)` | `function pointers (callbacks) are not supported` |
| `typedef` | `typedef is not supported in extern — use the underlying type` |
| preprocessor line | `#include/#define are not supported in extern — copy the prototypes` |
| no `(` (`extern int errno;`, `struct tm;`) | `only functions can be declared in extern` |
| unknown identifier by value | `unknown type 'SQLITE_API' — remove macros from copied declarations` |
| `long double`, `wchar_t`, `char16_t`, `_Complex`, `__int128` by value | `type 'long double' is not supported` |
| `int a(void), b(void);` | `declare one function per declaration` |
| type words that don't make one type (`int int`, `unsigned double`, `int DWORD`) | `cannot read the type 'unsigned double'` |
| K&R definitions, references, qualified names (`A::f`) | `cannot read this declaration` |

### 16.4 `CType` and platform sizes

```rust
pub enum CType {
    Void, Bool,                     // Bool: C _Bool / C++ bool / Rust bool, 1 byte
    Int { size: u8, signed: bool }, // size 1, 2, 4, 8
    F32, F64,
    Str,      // const char*: V2 str in (temporary copy); result copied, never freed
    OwnedStr, // compiled exports only: result copied, then freed by the library (15.4)
    Ptr,      // any other pointer: opaque native pointer
}
pub struct Param { pub ty: CType, pub spelling: Rc<str>, pub name: Option<Rc<str>>, pub non_null: bool }
pub struct Sig { pub name: Rc<str>, pub ret: CType, pub ret_spelling: Rc<str>,
                 pub params: Vec<Param>, pub err_slot: bool, pub cif: OnceCell<Cif> }
```

| C spelling | Windows (LLP64) | Linux, macOS (LP64) |
|---|---|---|
| `char`, `signed char`, `unsigned char`, `bool` | 1 | 1 |
| `short` / `int` | 2 / 4 | 2 / 4 |
| `long`, `unsigned long` | **4** | **8** |
| `long long`, `int64_t` | 8 | 8 |
| `size_t`, `ssize_t`, `intptr_t`, `uintptr_t`, `ptrdiff_t`, any pointer | 8 | 8 |
| `float` / `double` | 4 / 8 | 4 / 8 |

- Plain `char` is unsigned on aarch64 Linux and signed on every other supported target.
- `const char*` is `Str`, and so are `char const*` and `const char* const`. Every other pointer is `Ptr` (`char*`, `unsigned char*`, `void*`, `char**`, `T*`), so a non-const `char*` in `extern` is an opaque pointer in both directions (DOCS 12). To get a copied string back, declare the function with `const char*`, e.g. `const char* getenv(const char*)`; this is ABI-identical.
- `non_null` is set only for the compiled-block string parameters of 15.3 (`std::string`, `&str`, `String`); `const char*` takes `null` as NULL.

### 16.5 Argument conversion (V2 → C)

Arity is checked first: `ArityError: cos() takes 1 argument (2 given)`; named arguments are `ArityError: cos() takes positional arguments only` (5.7). An engine handle anywhere in the arguments, callable or not, is a `MarshalError` (11.7): `MarshalError: cannot pass a py handle (datetime.date) to native code; convert it with to_v2(h) first`. A conversion failure is `MarshalError: argument 2 ('exponent', double) of 'pow': <reason>` (an unnamed parameter omits the name).

| CType | Accepts | Rejects with |
|---|---|---|
| `Int{size,signed}` | `int` in range (unsigned: `0..=min(2^(8·size)−1, i64::MAX)`) | `300 does not fit in uint8_t (0..255)`; any other type |
| `F32`, `F64` | `float`; `int` (converted `as f64`) | F32: finite values beyond `f32::MAX` (NaN and ±inf pass) |
| `Bool` | `bool` | anything else |
| `Str` | `str` → `CString` copy; `null` → NULL unless `non_null` | `string contains a NUL byte at index 5`; `null is not a string` |
| `Ptr` | native pointer handle → its address; `null` → NULL | `expected a native pointer or null, got int`; for `char*`: `got str — declare the parameter const char* to pass a string` |

Converted values live in `ArgStore { slots: Vec<Slot>, strings: Vec<CString> }`, filled completely before any `libffi::middle::Arg` is taken and dropped after the call returns. So strings are valid **for the call only**: an API that keeps the pointer (`sqlite3_bind_text` with `SQLITE_STATIC`, `setvbuf`) ends up with a dangling pointer, and avoiding that is the declaration author's responsibility (DOCS 16).

### 16.6 Return conversion and native pointers

| CType | V2 value |
|---|---|
| `Void` | `null` |
| `Bool` | `bool` (non-zero = `true`) |
| `Int` | `int`; a `uint64_t`/`size_t` above `i64::MAX` is `MarshalError: return value 18446744073709551615 of 'f' does not fit in a V2 int` (never wrapped) |
| `F32`, `F64` | `float` (F32 widened) |
| `Str` | NULL → `null`; otherwise the bytes up to NUL, copied into a `str`; invalid UTF-8 → `MarshalError`; never freed |
| `OwnedStr` | as `Str`, then the library's `__v2_free(p)` (also on the UTF-8 error path) |
| `Ptr` | NULL → `null`; otherwise a native pointer handle |

A **native pointer handle** is a `Value::Handle(Rc<HandleRef>)` whose session is the reserved `SessionId::NATIVE`; no worker owns it, so nothing of ch. 11 that needs a worker (ops, releases) ever reaches it. Its `id` is the address, its `type_name` the declared pointee (`sqlite3*`), its repr `<native sqlite3* 0x000001d2c3e4f5a0>`, and `type(p)` is `"handle"`.
- `==` compares addresses only. NULL is always `null`, so native handles stay truthy.
- Dropping a handle sends nothing: V2 never frees native memory.
- `.attr`, calls, indexing, `len` and iteration raise `TypeError: native pointers support no operations — pass them to extern functions`; `to_v2(h)`, and passing a native handle to py or js code, are `MarshalError`.
- Any native handle fits any `Ptr` parameter. There is no pointee check, because `void*` round trips need that.

### 16.7 The call path (`ffi/call.rs`)

The libffi types are: `Void` → void, `Bool` → u8, `Int` → i8…u64 by size and sign, `F32`/`F64` → f32/f64, and pointer for `Str`, `OwnedStr`, `Ptr` and the error slot. Each `Sig` builds its `Cif` at its first call and keeps it in `Sig.cif`.

**Output order (DOCS 4.6)** uses a lazy flush, so tight loops pay nothing per call:
- `NATIVE_DIRTY` records flush targets: a compiled library's `__v2_flush`, or "C runtime" for `extern`.
- `flush_native()` runs every dirty target and clears the set. "C runtime" means libc `fflush(NULL)` on Unix, and on Windows `fflush(NULL)` in `ucrtbase.dll` and `msvcrt.dll` if they are loaded (`GetModuleHandleW`, pointers cached).
- It runs before every write to the host's stdout (`print`, forwarded `out` frames), before `input()` reads, before spawning a process that inherits stdout, and at exit.

`call_native(interp, f, args, span)` runs these steps:
1. Check arity and convert the arguments (16.5). Compiled exports get a trailing pointer to a local `err = NULL`.
2. Flush the host's stdout buffer if it is non-empty. Add the callee's flush target to `NATIVE_DIRTY` before the call, so V2 output during a reverse call flushes it. Enter one level of 9.7's depth count. Compiled exports also save and set `ACTIVE` and increment their unit's `active` (15.9).
3. Call `unsafe { cif.call::<R>(CodePtr(sym), &args) }`. For `Bool`/`Int`, `R = u64`, then truncate or sign-extend to the declared size: libffi widens small integer results to a full `ffi_arg`, so a narrower `R` would be overrun. Otherwise `R` is `f32`, `f64`, `*mut c_void` or `()`.
4. Restore `ACTIVE`, decrement `active` and leave the depth level.
5. Handle the error slot (15.4), convert the return value (16.6), then re-raise a pending panic or exit (15.9).

In-process calls have no timeout (`call_timeout` cannot interrupt a thread running native code) and no crash guard: a fault ends the process with the OS status (DOCS 11.7, 16). Ctrl+C during a long native call ends the process with 130 through 6.5's interrupt path. A library that calls `exit()` ends the process without V2's shutdown; workers then see EOF on stdin, and the kill-on-close Job Objects or `PR_SET_PDEATHSIG` end the other children (8.3).

---

## 17. Custom engines

A custom engine is any engine that isn't built in. It comes from an `[engines.<tag>]` table in the project's `v2.toml` (18.2) or a `define_engine` call (17.2) and has one of two kinds:

- **`run`** (`engines/runonly`): writes the block to a temp file, runs a command and returns its stdout (DOCS 13.1).
- **`bridged`**: an adapter process that speaks VEP/1 through the same `engines/session` code as `py` and `js` (DOCS 13.2).

Every custom tag is scanned with the Plain profile (3.8): braces are counted and foreign strings and comments are not understood, so code with unbalanced braces needs the fence form. Profiles come from the built-in table only; lexing never reads config (ch. 0).

### 17.1 Specs and validation

A custom spec is a 7.1 `EngineSpec` of kind `Run` or `Bridged` with no candidates, minimum or probe, and with `custom` set: its `command`, `ext`, timeouts, the `base_dir` used for path rewriting (the project root; for `define_engine`, the calling file's folder) and its origin, the declaring `v2.toml` or call site, which messages and doctor print. The table (18.2) goes through one validator, `engines/spec::validate`:

| Key | Kinds | Rule | Default |
| --- | --- | --- | --- |
| `kind` | both | `"run"` or `"bridged"` | required |
| `command` | both | non-empty array of non-empty strings (a string is 18.2's error). `run`: some element contains `{file}`; `bridged`: none may | required |
| `ext` | run | `[A-Za-z0-9_]+` after dropping one leading `.` | the tag |
| `startup_timeout` | bridged | seconds (int or float), finite, `>= 0`; 0 = no limit | 30 |
| `call_timeout` | both | as `startup_timeout` | 0 |

- **Tags** match `[a-z][a-z0-9_]{0,31}`; `v2`, `export` and `import` are reserved. A table named after a built-in tag or alias configures that built-in engine instead (17.2). Custom engines have no aliases.
- **Problems** in a table are configuration errors in 18.5's format (exit 2, before anything runs); from `define_engine` they are 17.2's `ValueError`s. The texts: `command must contain {file}`, `invalid engine tag '{t}'`, `'{t}' is reserved`, `'{k}' does not apply to {kind} engines`, `'{k}' must be {expected}, got {type}`. Unknown keys are errors with 18.5's did-you-mean text.
- **Command paths** are rewritten per 18.1, against `base_dir`; `V2_ENGINE_<TAG>` replaces `command[0]` only (18.4).

### 17.2 Registration

The registry (7.1) maps every tag to its spec; built-in aliases canonicalize everywhere (7.3). It is filled from config at startup:

- A table named after a built-in tag or alias is an override table (18.2); `kind` in it is the config error `built-in engine '{tag}' can't change 'kind'`, and `ext` is an unknown key (18.5).
- Every other table defines a custom engine (17.1); `V2_ENGINE_<TAG>` then replaces its `command[0]` (18.4).

**`define_engine(tag, kind, command, ext = null, startup_timeout = null, call_timeout = null)`** (DOCS 13.3) registers a custom engine at run time; `null` means the key is not given. The arguments are 17.1's keys and go through the same validator, with the calling file's folder as `base_dir` and the call site as origin. Problems raise `ValueError` with 17.1's texts after `define_engine: ` (`define_engine: command must contain {file}`); an unknown key is the binder's `ArityError` (5.7). Refused, also as `ValueError`:
- a built-in tag or alias: `define_engine: 'py' is a built-in engine — configure it in [engines.py] in v2.toml`;
- a tag already declared by `v2.toml` or by an earlier call, from any file: `define_engine: engine 'r' is already declared (main.v2:3)`, or `(C:\proj\v2.toml)`. The registry is per run, so each tag is declared once.

A `V2_ENGINE_<TAG>` kept from startup for that tag replaces `command[0]` now (18.4). The engine exists from the call on: a block reached before it raised EngineUnknown (7.3). The call returns `null`. `v2 doctor` runs no script, so it never lists these engines (19.2).

Nothing is resolved or started at registration. `engine_available(tag)` is `false` for an unknown tag and, for a custom tag, `true` when `command[0]` resolves to an executable file; it never spawns anything.

### 17.3 Run engines

Custom run engines and 7.2's built-in run engines share this code; a built-in one takes its command and `ext` from 7.2 (command configurable, 18.2). A reached run-engine block goes through 12.1: tag resolution, the step 3 check (no `@export` or inner `@import`), and args left to right. Then:

1. **Source.** The processed body (4.4: dedented, directive lines blanked, `@@export`/`@@import` unescaped) with `\n` line endings, UTF-8 without BOM, and nothing added. Line *k* of the file is `.v2` line `body_first_line + k − 1` (2.3). `bat`/`cmd` files get `\r\n` instead (cmd.exe's label search misbehaves in LF files), which moves no lines. `ps1` files start with a UTF-8 BOM, because Windows PowerShell (7.6's fallback) reads a BOM-less script as ANSI; pwsh accepts the BOM, and it moves no lines.
2. **File.** Create `<run>/run/{tag}-{n}.{ext}` with `create_new` and mode 0600 (20.3), write it, close it, and replace every `{file}` in `command` with its absolute path. The source reaches the command only through `{file}`; there is no stdin mode.
3. **Spawn**, under the child rules of 8.3 (own process group or Job Object), after flushing host stdout and dirty native stdio (16.7). The working directory and environment are inherited, plus 17.4.
   - **stdin** is the null device: `input()` owns user stdin (DOCS 4.6).
   - **stdout** is inherited in statement position, which keeps program order, and piped and read by a second helper thread in expression position.
   - **stderr** is piped; a helper thread copies each chunk to host stderr at once and keeps the last 4096 bytes.
   - A spawn failure is EngineNotFound `could not start '{path}': {os error}` (8.3).
4. **Timeout.** With `call_timeout` set, a timer with `call_timeout` (8.5) kills the process tree at the deadline and raises EngineTimeout `@{tag} block ran longer than call_timeout ({n}s) and was stopped`. `startup_timeout` doesn't apply.
5. **Finish.** Observe the exit without reaping, `kill_tree`, join the reader threads, then reap (8.3). Background processes the block started therefore stop with it (DOCS 13.1) and can't hold stdout open; only a process that detached itself (`setsid`, `CREATE_BREAKAWAY_FROM_JOB`) escapes.
6. **Cleanup.** Delete the temp file on every path, ignoring errors.
7. **Result.** A non-zero status raises EngineExitError. Otherwise a statement gives `null`, and an expression gives the collected bytes minus one trailing `\n` or `\r\n`, decoded as UTF-8 leniently (invalid bytes → U+FFFD), like the stderr tail.

**EngineExitError** (shared with compiled programs, 15.5):

- **Message:** `@{tag} block exited with status {code}`; Windows statuses `>= 0x80000000` print in hex (`0xC0000005`). A POSIX signal gives `@{tag} block was killed by signal {n} ({SIGNAME})`.
- **`lang`** is the tag; **`data`** is `{status, signal, stderr}`. `stderr` is the kept tail, decoded lossily; when more than 4096 bytes were written it starts after the first line break inside the window. The message doesn't repeat it, because it has already streamed live.

**Traps.** Scripts that resolve paths from their own location (`require("./x")`, `$(dirname "$0")`) see the run dir, not the project (DOCS 13.1). On Windows a configured `command` naming plain `bash` may find `System32\bash.exe`, the WSL launcher (7.6); the built-in `bash` never does.

### 17.4 Block args as environment variables

The child's environment is the inherited one, minus every `V2_ARG_*` variable (matched case-insensitively on Windows, so a nested `v2` can't leak its args), plus one `V2_ARG_<name>` per arg, named as written: `@sh(target: x)` sets `V2_ARG_target`. A `str` passes unchanged; `null`, `bool`, `int`, `float`, `list` and `dict` pass as compact `json_stringify` text (`null`, `42`, `1.0`, `[1,2]`, `{"a":1}`).

| Problem | Error |
| --- | --- |
| name not `[A-Za-z_][A-Za-z0-9_]*` | ValueError `block arg '{name}' can't be used as an environment variable name` |
| two names equal ignoring ASCII case (Windows only) | ValueError `block args '{a}' and '{b}' differ only in case; run engines pass args as environment variables, which are case-insensitive on Windows` |
| `bytes`, `func`, `handle` or `error` at any depth | MarshalError `block arg '{name}': a {type} can't be passed to a run engine` |
| NaN or ±inf at any depth | MarshalError `block arg '{name}': NaN and infinity have no JSON form` |
| a NUL character in a `str` | MarshalError `block arg '{name}' contains a NUL character` |

Sizes are not checked: an environment the OS refuses is 17.3's spawn failure.

### 17.5 Bridged engines

A bridged engine runs exactly like `py` and `js` (ch. 8, 9, 11, 13): one worker per tag per run, started lazily, with argv from the spec, the inherited working directory, environment and stderr, and stdin/stdout as the VEP pipes. Units, selectors, handles, re-entrancy, timeouts, crashes and line mapping behave as for built-in engines. The differences: nothing is written to the run dir, no environment variables are added, and the host gates features on the `caps` of `hello`, which must arrive within `startup_timeout` with `vep: 1` and `caps` a list of strings (8.4, 9.3; unknown caps are ignored). The built-in py adapter declares all four caps; js declares all but `kwargs`.

| Cap | The adapter supports | Without it, the host |
| --- | --- | --- |
| `modules` | `module` frames: `@import {…} from {tag}.name`, `@import {tag}.name as m` | raises ImportError `engine '{tag}' cannot import packages` and sends nothing |
| `handles` | `h` tags, `op` and `release` | never sends `op` or `release`; an `h` tag from the adapter is ProtocolError, so the adapter answers an unmappable result with `err` MarshalError |
| `kwargs` | named args in `call` | raises ArityError `{tag} functions take positional arguments only (got '{kw}')` and sends nothing |
| `reverse` | adapter-sent `call` frames: inner `@import`, V2 functions and other sessions' callable handles passed as args (11.8) | raises ImportError `@import inside @{tag} blocks needs the 'reverse' capability` before `load`, and MarshalError `engine '{tag}' can't receive V2 functions (no 'reverse' capability)` for function args, callable handles included; a `call` frame from the adapter is ProtocolError |

### 17.6 Adapter checklist

What the host relies on (porting a built-in adapter: 14.5). Always: framing and the 64 MiB limit, and an unknown `t` answered with `err` (9.1); exactly one `ret` or `err` per request, innermost first (9.2, 9.7); every exception caught per request (9.7); user stdin and stdout kept off the protocol (9.9); values per ch. 10 (with `handles`, an `h` tag marks a function-like object `callable`, 11.1).

| Message | Direction | Needed | § |
| --- | --- | --- | --- |
| `hello` | W→H | always; the first frame | 9.3, 8.4 |
| `init` | H→W | always | 9.3 |
| `load` | H→W | always | 9.4, 12.2, 13.7 (an action's value: 6.3) |
| `module` | H→W | `modules` | 9.4, 13.5 |
| `call` | H→W | always | 9.5, 13.4 |
| `op` | H→W | `handles` | 9.5, 11.3–11.5 |
| `ret`, `err` | both | always | 9.6 |
| `out` | W→H | always | 9.9 |
| `release` (`h`) | H→W | `handles` | 9.10, 11.2 |
| `release` (`fn`) | W→H | `reverse` | 9.10, 11.8 |
| `bye` | H→W | always | 9.10 |
| reverse `call` | W→H | `reverse` | 9.5, 9.8, 14.2 |

---

## 18. Configuration

Module `config/`. Config is loaded before any script is parsed; the lexer and parser never read it (ch. 0).

### 18.1 Files, project root and relative paths

- **File:** the project's `v2.toml`, the only config file; without one, the built-in defaults apply. `~` is `%USERPROFILE%` on Windows and `$HOME` elsewhere, read from exactly these variables (22.2); `~/.v2` holds only caches (7.7, 15.6). With no home folder, or one that isn't a folder (`/nonexistent`, a service account's), the caches are off.
- **No ownership checks:** V2 never refuses `v2.toml` or `~/.v2` because of owner or permissions; it is a same-user tool (20.1).
- **Project root:** start at the entry file's folder (`v2 FILE`, `v2 check FILE`; the real file's, a symlink followed, as `script_dir()` and imports see it) or the cwd (`v2 doctor`; canonicalized too, so a folder reached through a junction or a symlink shows the `v2.toml` that a run of a script there uses), made absolute with `.` and `..` applied to the text (as Windows does; POSIX `std::path::absolute` keeps `..`, and walking up `/proj/../other` would visit `/proj`), and walk up; the first folder holding a regular file `v2.toml` (an empty one counts, and so does a link to one; a broken link, a folder or a pipe of that name is passed over, `find_root`'s `is_file`) is the root, else the start folder. One project per run: V2 modules imported from other folders use the entry's config.
- **Quotes.** One pair of `"` around `command[0]` or `[python] venv` is taken off, as for `V2_ENGINE_<TAG>` (18.4): Explorer's "Copy as path" pasted into a TOML string brings them.
- **Relative paths** resolve against the folder of whatever declared them: `v2.toml` → the project root (DOCS 14); `V2_ENGINE_*` → the cwd. A `[python] venv` is then cleaned as the root is (`../venvs/.venv` is shown and used without the `..`). This covers `[python] venv`, engine commands and compiler `flags`. In `v2.toml`, `[python] venv` and `command[0]` that are `~`, or start with `~/` or `~\`, are in the home folder instead, as a shell reads them:
  - `command[0]`: with a separator, joined to that folder; without one, the `PATH` walk (7.5).
  - A later `command` element is made absolute against that folder only if it starts with `./` or `../` (also `.\`, `..\`), or if it doesn't start with `-`, contains no `{`, and `<folder>/<element>` exists. So `["ruby", "adapters/v2_adapter.rb"]` works from any cwd and `["sh", "-c", "cd src/x && make"]` is left alone. Nothing else is rewritten: there is no shell, `{file}` is the only placeholder (17.3), and a later element's `~` and any `$VAR` or `%VAR%` are not expanded (a program or venv not found whose path holds one says so, 7.6).
  - `flags` (`c`, `cpp`, `rust`): an element starting with `./` or `../` (also `.\`, `..\`), or such a path right after `-I`, `-L`, `/I` or `/LIBPATH:` (`-I./include`), is made absolute against that folder.
  - Paths used by scripts are separate: `read_file`, `exec` and the other file builtins resolve against the cwd (5.10); `import` and `extern` against the `.v2` file's folder.
  - A Windows drive-relative path in `v2.toml` (`C:.venv`) would resolve against that drive's cwd, so it is a configuration error: `[python] venv: 'C:.venv' is a drive-relative path: write it relative to the file, or absolute` (as 6.4 does for imports). `"C:\bin\node.exe"` is one too, its `\b` and `\n` being escapes: the path shows them escaped (`'C:\u{8}in\node.exe'`), and a second line gives 18.5's hint for a path holding a control character.

### 18.2 Schema

`[engines.<tag>]` names are canonicalized (`[engines.python]` is `[engines.py]`; both in one file is an error). A built-in tag accepts only the built-in keys below; any other tag declares a custom engine, whose keys, defaults and messages are 17.1's.

| Key | Type | Default | Applies to; meaning and validation |
|---|---|---|---|
| `[engines.T] command` | array of strings | auto (7.2, 7.6) | non-empty, skipping auto-detection. Built-in bridged or compiled: interpreter or compiler argv prefix; built-in run engine: the whole argv, which must contain `{file}` (17.1's message); custom: 17.1 |
| `[engines.T] flags` | array of strings | `[]` | `c`, `cpp`, `rust`: extra compiler flags, after V2's own (15.7); paths per 18.1 |
| `[engines.T] startup_timeout` | number, s | 30 | `py`, `js` (also for ts), custom bridged: spawn to `init` reply (8.4); finite, ≥ 0, 0 = none |
| `[engines.T] call_timeout` | number, s | 0 = none | each request to a `py`/`js` worker or custom bridged engine, each run of a `run` engine (built-in or custom) or a `c`/`cpp`/`rust` program (8.5); in-process calls (compiled exports, `extern`) can't be interrupted and ignore it |
| `[engines.T] kind`, `ext` | 17.1 | 17.1 | custom engines only |
| `[python] venv` | path | none | venv whose interpreter and packages are used (7.6) |

A `command` given as a string is a configuration error that spells out the array: `[engines.lua] command must be an array: write ["lua", "{file}"]`. Its elements are the string's words, split on whitespace, each in double quotes, or in TOML literal quotes when it contains a backslash (`write ['C:\Git\bin\bash.exe', "{file}"]`). A program path with spaces stays one element, the first rule that applies: a program in quotes (`"C:\Program Files\…\python.exe" -X utf8`), without them; the shortest run of words with a separator that is an existing file (from the file's folder, `~` as 18.1); a run of words with a separator and a space that ends in `.exe`, `.cmd` or `.bat`; a first word with a separator and the words after it that continue the path (a separator, and not starting with `-`, `{`, `/`, `\`, `.`, `~` or a drive). So `write ["C:/Program Files/Python311/python.exe", "-X", "utf8"]`, `write ["/opt/my tools/python3", "-X", "utf8"]`, but `write ["/usr/bin/env", "python3"]`. A word with a backslash or a `"` goes in literal quotes, or, with a `'` too, in escaped double quotes, so the array can be pasted.

Any `[engines.ts]` table is a configuration error: `[engines.ts]: ts runs on the js engine; configure [engines.js]`. So is a table named for a built-in engine's program (`node`, `nodejs`; `python3`, `py3`, `pypy`, `pypy3`, `cpython`; `gcc`, `clang`, `cc`, `cl`; `rustc`), which would otherwise declare a custom engine: `[engines.node] names a program, not an engine: it runs the js engine, so write [engines.js] command = ["node"]`. A table whose name is within 5.9's did-you-mean distance of this version's engine tags and aliases (`py`, `python`, `js`, `javascript`, `c`, `cpp`, `cxx`, `rust`, `rs`), case included, would configure nothing: `[engines.pyy] isn't an engine of this version: did you mean [engines.py]? (custom engines aren't in this version of V2 yet)`. A later engine's tag (7.3: `ruby` is not a misspelling of `rust`), a table with `kind`, which only a custom engine has (`[engines.r]`), and other names are kept for custom engines.

### 18.3 Layers and merging

Built-in defaults < the project's `v2.toml` < `V2_ENGINE_<TAG>` (DOCS 14).
- One file is read, so nothing merges across files. `V2_ENGINE_<TAG>` replaces a built-in bridged or compiled engine's whole `command` (which is then never validated) and any other engine's `command[0]` (18.4).
- `V2_ENGINE_PY` wins over `[python] venv`. Both `[python] venv` and `[engines.py] command` in `v2.toml` is a configuration error, since one of them would do nothing: `[python] venv and [engines.py] command both choose the Python: keep one (venv for a virtual environment, command for any other Python)`.
- Every configured value records its source, `ConfigSource = Project(PathBuf) | Env(String)` (a built-in default has none); doctor's FROM column shows it, or the resolver's own origin (`PATH`, `venv`, `vswhere`, `git`; 7.1, 19.2).

### 18.4 Environment overrides

- `V2_ENGINE_<X>` holds one executable path, never split; empty = unset. Spaces around it are trimmed (cmd's `set V2_ENGINE_PY=C:\…\python.exe && v2 x.v2` keeps the one before `&&`), and one pair of `"` around the whole value is taken off (cmd's `set V2_ENGINE_PY="C:\Program Files\…\python.exe"` keeps them). The name is matched case-insensitively on Windows (as Windows matches variable names) and must be upper case elsewhere (`V2_ENGINE_Py` is ignored), so two variables never name one engine. `X`, lower-cased, must be a canonical tag (`V2_ENGINE_PY`, not `V2_ENGINE_PYTHON`): an alias is ignored like any unknown tag. A path with a separator resolves against the cwd; a bare name (`node`) is left for the `PATH` walk (18.1, 7.5).
- Effect: a built-in bridged or compiled engine gets `command = [PATH]`, dropping any prefix such as `py -3` (a `cl.exe` path follows 7.6 step 3); a built-in run engine or a custom engine gets `command[0] = PATH`, the rest kept (a configured `command`'s, else 7.2's fixed args). A tag that no engine has is ignored, because environments hold unrelated variables; one a later `define_engine` declares gets it then (17.2). `V2_ENGINE_TS` is a configuration error that names the variable as written: `V2_ENGINE_TS: ts runs on the js engine; set V2_ENGINE_JS`.

### 18.5 Parsing and validation

- A leading UTF-8 BOM is dropped (invalid UTF-8 is an error, worded as 3.1's: `config error: the file is not valid UTF-8: it is UTF-16 (…); save it as UTF-8`, and off Windows iconv's line names this file, `… as in iconv -f cp1252 -t utf-8 v2.toml > new.toml && mv new.toml v2.toml, then check the text` run in its folder, and from another folder by its path, as 18.1 finds it from the script's folder up: `/p/v2.toml > /p/new.toml && mv /p/new.toml /p/v2.toml`, each path quoted as 19.1 says, `config::iconv_files`); the `toml` crate parses the file into a `toml::Table`; validation is hand-written (1.4). A syntax error about an escape on a line with a backslash (a Windows path in `"…"`) gets `hint: in "…", a backslash starts an escape: write Windows paths with / or in single quotes, as in venv = 'C:\proj\.venv'`. An escape TOML accepts makes a control character (`\n` a line break, `\t` a tab, `\b`, `\f`, `\r`), so a `command[0]` or `[python] venv` that holds one, after parsing, says so where it is reported (the drive-relative error of 18.1, `EngineNotFound` and doctor's note, 7.6; `config::escape_hint`): `in "…", a backslash starts an escape (\n is a line break, \t a tab): write the path with / or in single quotes, as in command = ['C:\Program Files\nodejs\node.exe']` (`venv = 'C:\proj\.venv'` for a venv, `command = ['C:\Program Files\Python311\python.exe']` for py). A syntax error on a line that holds a section and a key (`[engines.js] command = ["node"]`, as a sentence shows them) gets `hint: a section's name goes on a line of its own, its keys on the lines below it:` and the two lines. TOML's `invalid unquoted key` at a character that isn't ASCII (`[pythın]`, where its "letters" are ASCII ones) gets `hint: 'ı' (U+0131) can't be in a bare key: TOML takes A-Z, a-z, 0-9, - and _ there` (the character named as 3.2 names it, a letter with its code point, a bidi or control one by code point alone: `U+202E can't be …`, `U+00A0 (a no-break space) can't be …`), and, when the bare key read whole as an ASCII keyboard writes it (lower case, ı and İ as i, ß as ss, Latin-1's and Latin Extended-A's letters without their diacritics, combining marks dropped: `config::folded_key`) is a known name or within 5.9's did-you-mean distance of one, ` — did you mean [python]?`: in a `[…]` line a section, with the rest of the header folded the same way (`[ëngines.pÿ]`, `[ENGİNES.PY]` → `[engines.py]`; a rest still not ASCII gets no suggestion), or after `[engines.` an engine (`[engines.py]`; ts's names, `[engines.tş]`, `[ëngines.ts]` and `[ENGİNES.TS]`, get ` — ts runs on the js engine: configure [engines.js]`), and on a key's line a key of the section above it (`'venv'`, `'startup_timeout'`). A character that ends the key instead (a no-break space before the `=`) gets 3.2's hint for it: ` — type a plain space instead`; so does one without which the key as written is the name suggested, unless that suggestion is ts's line (a zero-width character in `venv`: ` — delete it`; in `Venv` the did-you-mean stays). A key of a section outside it: `venv goes in the [python] section: put a line [python] above it` (`command`, `flags`, `startup_timeout`, `call_timeout`: `… goes in an engine's section: put a line such as [engines.py] or [engines.js] above it`). Any other syntax error on a `key = value` line whose value starts with none of `"`, `'`, `[`, `{`, `-`, `+` or a digit and isn't `true` or `false` gets `hint: text goes in quotes: venv = ".venv"` (single quotes when the value has a backslash).
- Errors exit 2 before anything runs. Syntax: `C:\proj\v2.toml:12:5: config error: <toml message>` (position from the error span). Wrong type or value (a `toml::Table` keeps no positions): `C:\proj\v2.toml: config error: [engines.py] startup_timeout: expected a non-negative number, got string "30s"`; custom engine tables use the same form with 17.1's problem text.
- A known section (`engines`, `python`) that is not a table is a type error (`[engines]: expected a table, got integer 1`); any other top-level key is an unknown key.
- Unknown sections and keys, also in custom engine tables, are errors with did-you-mean: `C:\proj\v2.toml: config error: unknown key [engines.py] comand (did you mean 'command'?)`, also for a known key of 4 letters or more with more after it past a non-letter (`venv_dir`: `did you mean 'venv'?`). So are keys that do not apply: `C:\proj\v2.toml: config error: [engines.py] flags only applies to c, cpp and rust`. Config has no warnings.

---

## 19. CLI, doctor and check

`cli/` is a hand-written argv parser; `doctor/` holds doctor and check.

### 19.1 Commands and exit codes

| Command | Does |
|---|---|
| `v2 FILE [ARGS...]` | run FILE; ARGS become `args()` |
| `v2 check FILE` | 19.3 |
| `v2 doctor` | 19.2 |
| `v2 cache clean` | 19.4 |
| `v2 --version`, `v2 --help` | `v2 <crate version> (VEP/1)`; the command and exit-code tables |

`--version` and `--help` (also `-h`) are the only flags. A first word that is exactly `check`, `doctor` or `cache` is a subcommand, else the script (`v2 ./doctor` runs a file named doctor); everything after FILE goes to `args()` untouched. No arguments, any other first word starting with `-`, a missing or extra argument after a subcommand (`v2 doctor py`), or a missing or unreadable FILE, or one whose name isn't valid UTF-8 (`v2: the file name 'J\u{FFFD}rgen.v2' is not valid UTF-8, which V2 needs: the name is in another encoding; rename it`, as a folder's, 2.1): a message on stderr, exit 2.

`--help` or `-h` right after a command (`v2 check --help`, `v2 doctor -h`) prints the help, and anything after `--version` or `--help` is a usage error (`unexpected argument `x` after `v2 --version``). The help ends with where the docs are.

Each of these messages ends with what to do next (`cli/entry.rs` for FILE):
- No arguments: `no script given: run one with `v2 FILE`, for example `v2 hello.v2``; `-help`, `-?`, `--h`, `-H` add `(did you mean `--help`?)`, `-v`, `-V`, `-version`, `--v` add `(did you mean `--version`?)`; `-` is `V2 doesn't read a script from standard input: save it as a .v2 file and run v2 FILE`, and `--` is ``unknown flag `--`: to run a file whose name starts with -, write v2 ./-name.v2``. A usage error's second line is `run `v2 --help` for usage`.
- FILE missing (`v2 FILE` and `v2 check FILE`): a relative FILE while the current folder no longer exists: `cannot find 'hello.v2': the current folder no longer exists (it was deleted or moved)`, then `cd to another folder, or give the script's full path`. FILE and the arguments after it, joined by spaces, naming a file there (the longest such run: `v2 mein spiel.v2`, `v2 my mod [v1].v2 --fast`): `the file's name has a space, so the shell split it: put it in quotes, as in v2 "mein spiel.v2"`, and a did-you-mean whose name has one, `did you mean "mein spiel.v2"? a name with a space goes in quotes: v2 "mein spiel.v2"`. A link on the way that leads nowhere or in a loop (6.4's `link_problem`, a junction too; also when the OS's error isn't "not found", as with a loop): `cannot find 'j/main.v2'` and `'j' is a link to C:\x\gone, which isn't there: remove the link (rmdir j), or make C:\x\gone` (`it is …` for FILE itself; a loop: `'j' is a link in a loop: remove the link (rmdir j), or point it somewhere else`; the command and the target as 5.10 words them: `rmdir` or `del` with `\` on Windows, `rm` with `/` in Git Bash and elsewhere); a Linux symlink WSL made (5.10's) is `'rel.v2' is a Linux (WSL) symlink, which Windows can't follow: use the path it points to (ls -l in WSL shows it), or run V2 inside WSL`. A `\\wsl.localhost` or `\\wsl$` FILE with no WSL distribution of that name, or none named (5.10's `wsl_root`): `cannot find '\\wsl.localhost\Ubuntu-22.04\x.v2'`, then `there is no WSL distribution named Ubuntu-22.04: wsl --list --quiet shows them` (`\\wsl.localhost needs a WSL distribution's name after it, as in \\wsl.localhost\Ubuntu: …`). On Windows, Git Bash's spelling pasted into cmd or PowerShell (`/c/Users/me/x.v2`, read the way the file builtins' `msys_hint` reads it, `builtins::unix_drive`): `cannot find '/c/Users/me/x.v2'` and `on Windows, /c/… is Git Bash's spelling of C:/…: run v2 "C:/Users/me/x.v2"` (`run v2 check …`; quoted as below), and WSL's, `on Windows, /mnt/c/… is WSL's spelling of C:/…: run v2 …`; another Linux path (5.10's `linux_path_hint`: `/root/proj/main.v2`), `cannot find '/root/proj/main.v2'` and `a Linux path: from Windows, WSL's files are \\wsl.localhost\<distro>\…`; FILE with `: * ? " < > |` whose name with WSL's stand-ins is there (6.4's `stand_in_twin`; also when the OS refuses FILE's name, before the wildcard message below), `cannot find 'q?.v2'` and `did you mean 'q\u{f03f}.v2'? \u{f03f} is how Windows shows the '?' of the Linux name 'q?.v2': rename it in WSL` (6.4's `stand_in_note`, which the did-you-mean and the listing of `.v2` files below add for a name with a stand-in too, the listing after a `; `). When a folder on its way isn't there, `cannot find 'FILE'` and `there is no folder <the first missing one, absolute>`, plus `: there is a 'größe' whose name looks the same but is written in decomposed Unicode (letters plus combining accents, as macOS writes names): rename it` when a name next to it is that one in another Unicode form (6.4's `unicode_twin`), else `: did you mean 'examples'?` for a folder next to it within 5.9's did-you-mean distance. Else `cannot find 'NAME' in <folder>` (FILE's own name in the canonical folder it would be in, so `sub/x.v2` is `x.v2` in `…/sub`; an absolute FILE: just `cannot find 'FILE'`), then, for a link to a file that isn't there, `'NAME' is a link to /x/real.v2, which isn't there: remove the link (rm sub/NAME), or make /x/real.v2` (5.10's wording: the target absolute, from the link's folder; the link as FILE was written), else one hint, the first that applies: for a bare word, `run`/`exec`/`start` → `there is no 'run' command: to run hello.v2, type: v2 hello.v2` (the next argument, quoted when it has a space; none: `type: v2 FILE (for example: v2 hello.v2)`), `help` → `for help, type: v2 --help`, `version` → `for the version, type: v2 --version`, a word people expect to be a command (`build`, `compile`, `format`, `fmt`, `init`, `install`, `lint`, `new`, `repl`, `setup`, `test`, `uninstall`, `update`, `upgrade`) → `'install' isn't a V2 command; the commands are check, doctor and cache clean (see v2 --help)`; a name there that is NAME in another Unicode form (6.4's `unicode_twin`; also NAME`.v2`, for FILE without an extension when NAME`.v2` itself isn't there, which a folder can hold beside its twin: `v2 größe` for a Mac's `größe.v2`) → `there is a 'größe.v2' whose name looks the same but is written in decomposed Unicode (letters plus combining accents, as macOS writes names): rename it`, for NAME`.v2` followed by the run that works then, `, and run v2 größe.v2` (FILE with its last name composed, NFC, plus `.v2`: `module::composed_v2`; `v2 check …` for `v2 check`, quoted as below; also after `type it again`); FILE without an extension and FILE`.v2` there → `did you mean 'hello.v2'?` (FILE without the `/` it ends with, on Windows `\` too, here, in the `, and run …` above and in the did-you-mean of a `.v2` file below: `v2 hello/` gets `hello.v2`, 6.4's `trim_separators`); a bare word within 5.9's did-you-mean distance of `check`, `doctor` or `cache` → `did you mean 'v2 doctor'?` (`'v2 check x.v2'` with the next argument, else `'v2 check FILE'`; `'v2 cache clean'`), and one that is a command apart from case → `did you mean 'v2 check echo.v2'? (commands are lower case)`; FILE or its name without `.v2` with `.txt` there, as Notepad saves `hello` (`hello.txt` for `v2 hello.v2` or `v2 hello`) → `there is a 'hello.txt': your editor saved it as a .txt file, so rename it to 'hello.v2'`; FILE`.txt` there → `there is a 'hello.v2.txt': your editor added '.txt', so rename it to 'hello.v2'`; a `.v2` file of that folder within 5.9's did-you-mean distance, the names compared without `.v2` (so `nope.v2` isn't taken for `tour.v2`) → `did you mean 'hello.v2'?`; else the folder's `.v2` files, sorted, five at most (`the .v2 files there: a.v2, b.v2, …`), or `there are no .v2 files there: cd to the folder that holds your script, or give its full path`, or, when V2 can't list the folder (a deny on it), `V2 can't list that folder: Access is denied (check the name, or the folder's permissions)`. `/?` is `'/?' isn't a V2 command` with the help hint.
- An unknown flag that names a file in the cwd (`v2 -dash.v2`; also `-dash` then `.v2`, as PowerShell splits it, or `-dash` for `-dash.v2`): the usage error is followed by `to run the file -dash.v2, write: v2 ./-dash.v2`; after `v2 check` (`v2 check -dash .v2`), `to check the file -dash.v2, write: v2 check ./-dash.v2`.
- Several files after `v2 check`, each ending `.v2` (a shell's `*.v2`): `unexpected argument `b.v2` after `v2 check FILE`: it checks one file; to check several, loop in the shell: ok=0; for f in *.v2; do v2 check "$f" || ok=1; done` (on Windows outside Git Bash, whose `MSYSTEM` is set, PowerShell's: `loop in PowerShell: $ok = 0; Get-ChildItem *.v2 | ForEach-Object { v2 check $_.Name; if ($LASTEXITCODE -ne 0) { $ok = 1 } }`), as the README shows. On Windows a FILE with `*` or `?`, which cmd and PowerShell pass on as written: `cannot find '*.v2': cmd and PowerShell don't expand * and ?, and v2 check takes one file`, then `to check several, loop in PowerShell: …` over FILE's pattern (`$_.FullName` when it names a folder); for `v2 FILE`, `… and v2 runs one script`, then the folder's `.v2` files, as above. In Git Bash a `*` that reaches V2 matched nothing: the missing-FILE message.
- On Windows, FILE (`v2 FILE` and `v2 check FILE`) with a part that ends in a dot or a space, which Windows would read as another file (`run.v2.` as `run.v2`), is refused before anything else, as 5.10 refuses it (`builtins::dotted_name`): `cannot read 'run.v2.': a Windows file name can't end in a dot or a space ('run.v2.' would be 'run.v2')`, then, when a file of that name is there (WSL and Macs make them), `one of that name is there (WSL and Macs can make one): run it by its full path with \\?\ in front: v2 "\\?\C:\proj\run.v2."` (`check it …: v2 check …`; such a path keeps its `\\?\`, 2.1), else `did you mean 'run.v2'?` when Windows' reading of it is there (6.4's `without_dots`), else the folder's `.v2` files as below (`entry::dotted_file`).
- FILE V2 can't read is `cannot read 'x.v2': <os message>`; on Windows one another program holds open without sharing (`ERROR_SHARING_VIOLATION`, `ERROR_LOCK_VIOLATION`, as Excel keeps a file) adds `another program has it open (Excel keeps an open file locked): close it there` (`span::read_hint`), in `v2 check` too, as the hint of 6.4's `ImportError` for a module, and for the project's `v2.toml`, whose config error (`…v2.toml: config error: cannot read the file: …`, exit 2, 18.5) adds it as `  hint: …`.
- FILE a pipe or a device (`/dev/stdin` from a pipe, `<(…)`, `/dev/zero`; on Windows one WSL made, seen through `\\wsl.localhost`, which Windows takes for a file and shows as a reparse point: 6.4's `module::wsl_file`): `'/dev/stdin' is a pipe or a device, not a file: V2 runs scripts saved as .v2 files`. FILE over 1 MiB with an extension other than `.v2` (`v2 data.csv` for `v2 script.v2 data.csv`): `'data.csv' is 18 MB and isn't a .v2 script`, then `to give it to a script, type: v2 SCRIPT.v2 data.csv`; neither is read.
- FILE without an extension, with only FILE`.v2.txt` there: `there is a 'hello.v2.txt': your editor added '.txt', so rename it to 'hello.v2'`.
- FILE a folder: `'proj' is a folder, not a script`, (with only Notepad's `hello.v2.txt` in it: `, and it has no .v2 files`, then `there is a 'hello.v2.txt': your editor added '.txt', so rename it to 'hello.v2'`), then `run one of its .v2 files, for example: v2 proj\main.v2` (after `v2 check`: `check one of its .v2 files, for example: v2 check proj\main.v2`) (`main.v2` if there, else the first by name; the platform's separator), or `, and it has no .v2 files`; a folder V2 can't list (a deny on it) is `'proj' is a folder, not a script, and V2 can't list it: Access is denied`, then `run a .v2 file in it by its name: v2 proj\FILE.v2` (`check …`). A path these messages suggest, as the venv's `create it:` command and doctor's COMMAND items do, is in double quotes unless it has only letters, digits and `._-+~\/:` (`cli::shell_word`), so a folder named `R&D` or `it's` pastes into cmd, PowerShell and sh; off Windows one with a `$`, `` ` ``, `"`, `\` or `!`, which sh (or bash's history) reads inside double quotes, is in single quotes instead, a `'` as `'\''` (`'/w/a$b/v2.toml'`).
- FILE ending `.py`, `.js`/`.mjs`/`.cjs`, `.ts`/`.mts`/`.cts`, `.c`/`.h`, `.cpp`/`.cc`/`.cxx`/`.hpp` or `.rs`: after the error that ends the run (a SyntaxError, a static-check error or an uncaught error), one more line: `note: 'app.js' is a JavaScript file, and V2 runs .v2 scripts: put JavaScript code in a @js { } block in a .v2 file`; for the compiled languages, `… V2 runs .v2 scripts (C blocks aren't in this version of V2 yet)`.

| Exit | Meaning |
|---|---|
| 0 | success: the entry file ran to its end; check found no errors; doctor and `cache clean` finished |
| n | `exit(n)`: an int in i32 range (on Windows up to 4294967295 too); POSIX keeps the low 8 bits; no `finally` runs (5.8) |
| 1 | an uncaught error, printed per 2.4 (`EngineUnknown` included), or a `SyntaxError` in the entry file (6.1); check found errors; a run whose final stdout flush fails, or during which a worker reported an error no V2 call could raise (9.9), turns 0 into 1 (6.5) |
| 2 | usage or configuration error (18.5), before any V2 code runs; `v2 cache clean` without a home folder |
| 3 | uncaught `EngineNotFound` |
| 141 | stdout's or stderr's reader went away (a closed pipe, 6.5); nothing more is printed |
| 130 | interrupted: Ctrl+C, Ctrl+Break, console close, SIGINT, SIGTERM, SIGHUP, SIGQUIT; always 130, never 128+signo; no `finally`, engines stopped, what was printed written, the terminal's settings and cursor back (Unix, when stdout is the terminal, 6.5), `<run>` removed (6.5, 8.7) |
| 101 | internal panic, a V2 bug (6.5) |

### 19.2 v2 doctor

1. Load config from the cwd (18.1; error: exit 2); the registry is the built-ins (7.2, the built-in run engines included), then the config's custom engines, one row each (doctor runs no script, so `define_engine` engines are not listed). Resolve every engine with fresh probes (7.7), rewriting the probe cache. Doctor executes toolchain commands, including project-configured ones (`v2.toml` commands, the venv python, vswhere and vcvarsall), so it is not safe on an untrusted project (20.2).
2. Print a table to stdout, columns `ENGINE STATUS VERSION FROM COMMAND`, each as wide as its longest cell + 2 (DOCS 2); COMMAND goes last, so a long path (a venv deep in a project) runs on past its row instead of pushing the other columns apart. COMMAND: `shown` plus prefix args (a built-in run engine's: the rest of its template, `{file}` included), space-joined, with an item that has a space in double quotes when there is more than one item (`"C:\Program Files\Python313\python.exe" -X utf8`, so it can be pasted); ts: `(js engine, built-in type stripping)`; custom: the argv template. VERSION: the probed version (ts: Node's); `-` for `cmd` and custom engines, which have no probe. FROM: `PATH`, `vswhere`, `venv` (a `[python] venv`, also one that is broken), `git`, `v2.toml`, `V2_ENGINE_<TAG>` or `-` (ts: js's; bash on Windows: `git`, 7.6). STATUS: `ok`; `missing` (other columns `-`; also `cmd` off Windows); `old` (only too-old versions, 7.6; VERSION the newest, and when found on `PATH`, COMMAND its path and FROM `PATH`); `unsupported` (the same for a newest version at or above the minimum, Node 23.0 and 23.1, whose note says `not supported: found 23.1.0, which has no type stripping; …`); `broken` (probe failed or timed out, wrong target, a configured command or venv missing, a venv whose python runs another Python than it was made with (7.6), a program that answers `--version` but doesn't start with a startup variable set, a command that runs `conda run` so that no worker can start, step 3, or, in a `\\server\share` folder, a configured `.bat` or `.cmd`, 7.6; found on `PATH`, COMMAND is the first such program, never a Store placeholder, and FROM `PATH`; a venv that names a file shows that file). The ts row shows js's COMMAND and FROM when js found something.
3. Then `notes:`, engine by engine in table order: for py, first, when `V2_ENGINE_PY` wins over the `[python] venv` of the project's `v2.toml` (18.3), `py: V2_ENGINE_PY is set, so [python] venv 'C:\p\.venv' (v2.toml) isn't used: to use it, Remove-Item Env:V2_ENGINE_PY` (`unset V2_ENGINE_PY` off Windows; `resolve::unused_venv`), but not when unsetting it wouldn't help: for a venv the other system made, which 7.6 keeps (DOCS 14's project used from Windows and WSL names it in `v2.toml` and this system's in `V2_ENGINE_PY`), one without a python for this system, or `V2_ENGINE_PY` naming that venv's own python; for an `ok` engine (or one broken by its `conda run`, below), a line per variable its program reads as it starts that is set (8.4: `py: PYTHONHOME is set (C:\nope), which Python reads as it starts`, the value whole; none for Python's embeddable package, which ignores them, so it isn't started for them, but `py: this is Python's embeddable package: its python313._pth decides sys.path (PYTHONPATH is ignored), and it has no pip or venv` (`…, and it has no venv` once it has pip; for a full Python given a `._pth`, `py: its python312._pth decides sys.path (PYTHONPATH is ignored)`, with `, and it has no pip` when it has none; `resolve::pth_note`), 7.6), then on Windows, for a command through `wsl.exe` or WSL's `bash.exe`, 7.6's `py: wsl.exe runs a Linux python, …` (the row stays `ok`: a wrapper could translate the adapter's path), then, when its command runs `conda run` (for js on Unix also `mamba run` or `micromamba run`) so that no worker can start, 8.4's advice, read from the command's words with no process started (`--version` works through it, a worker doesn't), and the row is `broken`, not in `ready:` (ts on such a js too, with `ts: @ts blocks run on the js engine: see js above`), so DOCS 15's `grep -q '^ready:.* py'` fails as the scripts would: `py: V2 talks to the program over its standard input and output, and conda run passes them through only with --no-capture-output: add it after run, or name the environment's python (envs/sales/bin/python in conda's folder) in [engines.py] command` (js on Unix: `js: V2 talks to the program over two pipes it inherits (named in V2_VEP_FDS), and conda run closes them, even with --no-capture-output: name the environment's node (envs/web/bin/node in conda's folder) in [engines.js] command`; none for ts, whose command is js's; `session::conda_run_note`). With one set, or on Unix under an address-space limit (`ulimit -v`, `RLIMIT_AS`), doctor also starts the program, under 7.7's rules and 10 s limit, since `--version` answers before Python reads them and before Node runs a `--require` preload or reserves its memory (`resolve::start_failure`, `spec::start_args`): Python imports what V2's worker imports as it starts (py.py's step 1, `-c "import sys; sys.path[:1] = [p for p in sys.path[:1] if p]; import os, io, json, ast, …"`: without the current folder, which `-c` puts first on `sys.path` and the worker, a script in the run folder, doesn't have), as a stale `PYTHONPATH`, another version's standard library, starts Python but breaks `re` (`AssertionError: SRE module mismatch`); Node runs `node -e 0`. When that exits with an error or a signal (a timeout, or a spawn failure, tells nothing), the row is `broken` with its VERSION, COMMAND and FROM, and the notes are `py: C:\…\python.exe can't import the standard modules V2's worker uses (exit code 1): Fatal Python error: init_fs_encoding: …` (`js: /usr/bin/node -e 0 failed (killed by signal 5 (SIGTRAP))`; after a Python traceback, its exception, the first line after it that isn't indented, else the first output line that has `rror` in it, else the first, past 160 characters cut in its middle as 7.6's skip reasons are), then, for the variables that are set, `py: fix: python didn't start: PYTHONHOME is set (C:\nope), which it reads as it starts: unset it or fix it` (`PYTHONHOME is set (…) and PYTHONPATH is set (…), …: unset them or fix them` for both), and under a limit 8.4's words, `js: fix: a memory limit (ulimit -v) of 585 MB is set for this shell, and Node.js reserves about 1 GB as it starts: raise it` (`the engine may need more` for py); and ts, on a js that didn't start, is `broken` with `ts: @ts blocks run on the js engine: see js above`; a line for an engine that is not `ok`, then the hits it skipped, such as Store placeholders (so a broken engine's `(see why below)` points at them; for `old` and `unsupported`, not the one the row and the line already name; ts, whose reasons are js's, has only `ts: @ts blocks run on the js engine: see js above` when js was found but can't be used, and js's own note when js is missing), then a `fix:` line for each that couldn't load a library (7.8), and cl's and rustc's targets; for py found by the search (or not found) while the activated venv can't be used (7.6), `py: the activated venv /p/.venv (VIRTUAL_ENV) has a python that links to …`; then, for each `[engines.<tag>]` table of `v2.toml` this version doesn't run, `r: [engines.r] in v2.toml is a custom engine, and custom engines aren't in this version of V2 yet: @r blocks won't run` (a later built-in engine's: `sh: [engines.sh] in v2.toml configures @sh, which isn't in this version of V2 yet`); then, when the temp folder isn't one, `temp: ` and 20.3's problem, which adds `blocks` to `not ready`; then the config file used (`config file: <path>`, or `config file: none`), the version line for bug reports (`--version`'s text, then the OS and architecture), and a summary: `ready: ` and the `ok` engines in table order (none: `none`), then, if any engine is not `ok`, `not ready: js, ts (see the notes); scripts that don't use them still run` (`plain V2 scripts still run` when none is ready). Exit 0: missing engines are information, not failure. For example:
   ```text
   notes:
     py: skipped C:\Users\me\AppData\Local\Microsoft\WindowsApps\python3.exe (Microsoft Store placeholder)
     js: not found; install: winget install -e --id OpenJS.NodeJS.LTS (or https://nodejs.org), then open a new terminal; installed where PATH doesn't look: [engines.js] command in v2.toml, or V2_ENGINE_JS
     ts: not found; install: winget install -e --id OpenJS.NodeJS.LTS (or https://nodejs.org), then open a new terminal; installed where PATH doesn't look: [engines.js] command in v2.toml, or V2_ENGINE_JS
   config file: none
   v2 0.1.0 (VEP/1), windows x86_64
   ready: py
   not ready: js, ts (see the notes); scripts that don't use them still run
   ```
   A `missing` engine's note is `not found; install: …` with 7.8's hint, then `; installed where PATH doesn't look: [engines.js] command in v2.toml, or V2_ENGINE_JS` (js's for ts; with PATH unset or empty: `not found: PATH isn't set, so nothing was searched; set it, or name the program in v2.toml, [engines.py] command`, js's table for ts) (run engines have none: `not found`; `cmd` off Windows: `cmd is Windows only`); an auto-detected `old` one is `too old: found 3.9.6, V2 needs 3.10+; install: …` (ts with js's minimum); a `broken` one, or a configured command that is too old, gets the first line of its `EngineNotFound` message. A `broken` one found on PATH without a library `fix:` line ends with `py: fix: repair that Python, or install one: …` (`Node.js` for js) and 7.8's hint, or, when a batch file was passed over in a `\\server\share` folder (7.6), the program being fine, with 7.8's `py: fix: name the program itself by its full path, as in V2_ENGINE_PY=C:\…\python.exe (for pyenv-win, pyenv which python shows it), or run v2 from a folder on a drive`, or, when 7.8's `NODE_OPTIONS` line applies, with that one: `js: fix: node didn't start: NODE_OPTIONS is set (--bogus-flag), which it reads as it starts: unset it or fix it`.

### 19.3 v2 check

Check parses and runs the static checks; it executes nothing (no engine, compiler, probe, `xcode-select`, `vswhere`, or library load, which would run initializers) and writes nothing (no probe cache, no `<run>`); it only reads files (20.1).
1. Load config (18.1, from FILE's folder; error: exit 2). Parse FILE and, transitively, every `import "…"` target once (paths are literals, so imports inside `if` count too); a cycle or a missing target gives the run-time `ImportError` text (6.4) at the `import`; a file that fails to parse is reported and its imports are skipped.
2. **Errors per file:** every `SyntaxError` (format 2.4) of chapters 3, 4 and 16.1, such as directive syntax, duplicate labels, block args on a compiled tag, `@import SEL as NAME` without a package selector, a definition block used as a value, and `extern` declarations.
3. Diagnostics go to stderr (2.4), then a summary to stdout: `check main.v2: 2 files, 0 errors`; when a file's parse stopped at an error (so its other errors weren't looked for), it ends ` (a file with a syntax error shows only that error: fix it, then check again)`. Exit 1 on any error, else 0.

### 19.4 v2 cache clean

`v2 cache clean` deletes everything under `~/.v2/cache/` (`build/`, 15.6; `toolchains.json`, 7.7), entry by entry and best effort: what it cannot delete (on Windows, a library a running V2 has loaded) is skipped and listed. It is the one cache reset: a damaged cached library is rebuilt on its own (15.8), and `v2 doctor` probes afresh (19.2). Exit 0, or 2 if the home folder is unknown.

---

## 20. Security model

### 20.1 Trust boundary

V2 is a same-user tool. Running a `.v2` file runs all its code with the user's rights: its blocks, compiled blocks, `extern` libraries and the engine commands its config names. `v2 check` only parses (19.3). The mitigations in 20.2 keep two promises:

- **Data stays data.** V2 values never become code: block args and call arguments travel as data, and V2 never inserts values into foreign source (DOCS 4.1, 4.4).
- **No code the user didn't choose.** A downloaded project or another local user can't get code run through the working directory, `PATH`, DLL planting or the build cache.

Out of scope: sandboxing, permission prompts, signature checks, defending against the user's own other processes, and the config file itself. A `v2.toml` found by the upward search is trusted like the script (no ownership checks, 18.1; `v2 doctor` names the config file it read, 19.2), and so are the modules Python imports from each `.v2` file's folder and the project root (9.3) and what Node's own lookup finds from those folders up (`node_modules`, `package.json`, 12.4). DOCS 16 lists them, and says not to keep or run scripts under folders other users can write.

### 20.2 Threats and mitigations

| Threat | Promise | § |
| --- | --- | --- |
| Running an untrusted `.v2` file or project | None: no sandbox (DOCS 16); a run runs all its code and `v2 doctor` runs the toolchain commands a project configures; `v2 check` only parses | 19.2, 19.3 |
| Temp-file and symlink races | Temp files only in the private run dir or the owner-only build cache, always created new (a planted file or symlink is refused); nothing written into the project | 20.3, 15.6 |
| Tampered adapter | Built-in adapters come only from the `v2` binary | 14.1 |
| Data breaking the frame stream | No value can split or forge a frame | 9.1, 9.11 |
| User code or its children reaching the protocol | Kept off the protocol descriptors (Node code that bypasses the wrappers can still corrupt the stream: fatal ProtocolError) | 9.9 |
| Evaluating names | Names from frames are looked up, never evaluated; block args are bound as values | 12.4, 13.5, DOCS 4.4 |
| Module shadowing | A documented search order; adapters load their own dependencies first; the cwd is never added | 9.3, 14.3, DOCS 7 |
| Build-cache poisoning | Owner-only, outside the project, keyed on everything that shapes a build, filled only by atomic rename | 15.6 |
| Command and library hijack | Programs and libraries are found only where documented, never through the cwd or a relative `PATH` entry; children are spawned by absolute path | 7.5, 15.8, 16.2 |
| Processes outliving their block or the run | The child rules, also when the host dies with no exit path | 8.3 |
| Environment inheritance | Engines inherit the user's own environment and cwd (DOCS 16); V2 adds only documented variables, which the user's other processes can read, so no secret travels in them | 7.2, 15.7, 17.4 |
| Memory safety of FFI and compiled code | None: trusted and unsandboxed (DOCS 11.7, 12); V2 checks only what it converts | 16.5, 16.6, 15.8 |

### 20.3 The private run directory

- **Path.** `<run>` = `temp_dir()/v2-{pid}-{32 hex}`, the hex being 16 bytes from the OS RNG. It is created on first need, so a pure-host script never makes one.
- **POSIX.** Create it with `mkdir` and mode 0700. If the name exists, draw a new one; after 8 tries, raise IOError `cannot create a private temp folder in {base}`. The message (both systems) adds the OS's reason and what to do: when `temp_dir()` isn't a folder, `; TMP names C:\nope, which doesn't exist: blocks can't run until it names a folder you can write to` (the variable it came from: `TMP`, then `TEMP` on Windows, `TMPDIR` elsewhere; none: `; the temp folder /tmp doesn't exist: blocks can't run until it does`; `isn't a folder` for a file), else, with a variable set, `; TMP must name a folder you can write to`, or, for a folder made but not private (a Windows drive in WSL, a FAT or network drive, which show every folder as everyone's), `: its file system can't keep a folder private (a Windows drive in WSL, a FAT or network drive); set TMPDIR to a folder on the computer's own file system, such as /tmp`. Writing a worker's adapter into a temp folder that is full: `cannot start Python: the temp folder /mnt/tiny is full (No space left on device): free some space there, or set TMPDIR to a folder with room` (`TEMP` on Windows); another failure, `cannot start Python: V2 couldn't write its worker for it into …: <why>`. `{base}` is shown without a trailing separator. Then `lstat` must show a real directory (not a symlink) owned by the euid, with mode 0700.
- **Windows.** Create it with `CreateDirectoryW` and a protected DACL that grants full control only to the current user's SID (from `GetTokenInformation(TokenUser)`): SDDL `D:P(A;OICI;FA;;;{sid})`, always by its `\\?\` path, so a deep temp folder can't fail it. Its longest file, `\v2-{pid}-{32 hex}\adapters\py.py` (62 characters at most), must still fit in 259 characters, which Python opens at most when Windows' long paths are off (`LongPathsEnabled` in `HKLM\SYSTEM\CurrentControlSet\Control\FileSystem`): a temp folder that leaves too little room is used by its short (8.3) name (`GetShortPathNameW`) when that fits, else it is 20.3's problem, in the message and doctor's `temp:` note: `TMP names C:\…\Temp, which is 237 characters long: too long for the files blocks need there, as Windows paths stop at 260 characters; blocks can't run until TMP and TEMP name a shorter folder, such as C:\Temp` (`the temp folder C:\… is 237 characters long: …` with neither variable set). With none of `TMP`, `TEMP` and `USERPROFILE` set, as a program that starts v2 with a small environment of its own leaves them, Windows offers its own folder, where a user can't write: that is 20.3's problem too (`unset_problem`), `TEMP and TMP aren't set: a program that starts v2 with an environment of its own must pass them on (in Python, env=dict(os.environ, …))`, after the OS's reason in the message (`cannot create a private temp folder in C:\WINDOWS: Access is denied; TEMP and TMP aren't set: …`) and in doctor's `temp:` note.
- **Contents.** `run/{tag}-{n}.{ext}` (17.3); `adapters/` (14.1); `build/<k>/` only when `~/.v2` is unusable: builds then happen and libraries load there (15.6). Every file is created with `create_new`.
- **Leftovers.** A run killed outright (SIGKILL, Task Manager) can't remove its folder. So a run that has just made its own `<run>` removes, best effort, every entry of the same base named `v2-{pid}-{32 hex}` that is a real folder (on POSIX owned by the euid), unchanged for 5 s (`MARGIN`, by its mtime: a run that is starting may have just made it), whose pid isn't running (`kill(pid, 0)` failing with `ESRCH`; Windows: `OpenProcess` failing other than with access denied, or an exit code other than `STILL_ACTIVE`). A running pid keeps its folder; on Windows, which reuses pids soon, not when that process started more than 5 s after the folder was made (`GetProcessTimes` against the folder's creation time, `reused`), as it can't be the run that made it. Seconds, not ms: a run makes its folder a few ms after it starts, and file times are coarser than process times (NTFS takes the clock's last tick), so a run's folder can look older than the run (round 20: runs started together removed each other's). POSIX has no such race: a folder's pid is its maker's, which runs from before the folder is made until after it is removed, so `kill` can't miss a live run, and a reused pid only keeps a folder longer; only runs in separate pid namespaces that share one temp folder (containers) can't see each other's pids, so each needs its own `TMPDIR`.
- **Removal.** On every exit path (normal end, uncaught error, `exit()`, interrupts), after workers and child trees have stopped: by main once the interpreter thread ends, or by the interrupt path (6.5). On Windows each makes up to 3 more tries, 50 ms apart, while killed processes release their files; then errors are ignored. A host killed outright (SIGKILL, `TerminateProcess`, a native crash) leaves the folder behind. So does, on Windows, a run that loaded a compiled library from `<run>/build` (no usable `~/.v2`, e.g. no home folder): libraries are never unloaded (15.8), so their files can't be deleted. A leftover folder stays in the temp dir, owner-only.

---

## 21. Failure-mode matrix

### 21.1 Conventions

- **Rows.** Failures whose outcome goes beyond the error raised (session, unit, cache or process state, what is sent, exit code). Static `SyntaxError`s (ch. 3, 4.4, 4.5, 16.3) stop the file before it runs and are not listed.
- **Columns.** Kind is the error kind raised (`none`: nothing is raised). § is the section that defines the message and the behavior. Dead is 8.2's session state. Exit codes: 19.1.

### 21.2 Matrix

| Trigger | Kind | § | State after |
| --- | --- | --- | --- |
| **Setup** | | | |
| No usable toolchain: none found, only too-old ones, or only the Store placeholder `python.exe` (exits 9009); a configured command or venv missing or not usable (another target; on Windows a C/C++ compiler other than `cl`) | EngineNotFound | 7.4, 7.6, 7.8 | failure memoized for the run (7.1); never falls back to auto-detection; exit 3 if uncaught |
| Worker can't start; exits with Windows status 9009 before `hello` (a Store placeholder the probe missed); `hello` reports a version below the minimum (killed) | EngineNotFound | 8.3, 8.4 | Dead (SpawnFailed); later uses re-raise this EngineNotFound |
| No `hello` and `init` reply within `startup_timeout` | EngineTimeout | 8.4 | killed; Dead |
| Worker exits before `hello` with any other status; `init` answered with `err` | EngineCrashed | 8.4 | Dead |
| A JSON frame other than a valid `hello` (including `out`) before `hello`; `vep` other than 1 | ProtocolError | 8.4, 9.3 | killed; Dead |
| Invalid config file (unknown sections and keys included) or custom engine table | configuration error | 18.5 | exit 2 |
| **Blocks and imports** | | | |
| Exception while a block runs, foreign syntax errors included | ForeignError (`foreign_type` e.g. `SyntaxError`) or the reported kind | 9.6, 12.1 | definition block: unit `Failed`, later reaches re-raise the stored error; action block: nothing kept, the next reach runs it again |
| `@export` lists a name the block doesn't define (compiled: as a function, so a constant counts too) | ImportError | 12.2, 15.2 | unit `Failed` |
| Module selector on a compiled or run engine; adapter without `modules` | ImportError | 13.5, 17.5 | nothing sent |
| A waiting block or `@import` fails when its language starts | the block's error, or ImportError | 12.7 | the rest of the line still runs, then the first error goes to what needed the language; a block is `Failed`, an `@import` retries at its next use |
| Inner `@import` called before its target block loads | ImportError, raised in the calling language as a `V2Error`; compiled: NULL and `v2_last_error()` | 13.6, 13.7, 15.9 | a later call looks the target up again |
| **Calls and values** | | | |
| Foreign exception, including Python `sys.exit()` (`foreign_type` `SystemExit`) and JS `process.exit()` (which throws an `Error`) | ForeignError | 9.6, 9.7 | session fine; the worker keeps running |
| Named args to js/ts, compiled or `extern` functions, or a bridged engine without `kwargs` | ArityError | 5.7, 10.8, 17.5 | nothing sent |
| Callback into V2 while its engine is idle (timer, thread, late event handler), or from another thread | Error raised in the foreign language; compiled: NULL and `v2_last_error()` | 9.8, 15.9 | V2 function not run; session fine; the callback stays valid for later requests |
| Worker dies during a call, or while idle (raised by the next use) | EngineCrashed | 8.6 | Dead |
| Any later use of a Dead session or its handles | EngineCrashed; Dead (SpawnFailed): its EngineNotFound | 8.2, 11.7 | Dead |
| Bridged call exceeds `call_timeout` | EngineTimeout | 8.5 | killed; Dead |
| Non-frame line after `hello`, corrupt frame, unknown `t`, bad `$v2` tag, out-of-order reply, frame over 64 MiB | ProtocolError | 9.7, 9.11 | killed; Dead |
| Block output below the language level (C extension, `os.write(1, …)`, a child inheriting stdout) | none | 9.9 | text appears on stderr, not stdout (DOCS 4.6); protocol intact |
| Rust panic or C++ exception in an exported function | ForeignError | 15.4 | library stays loaded |
| Native crash, or native code calling `exit()`/`abort()` | none | 16.7 | process gone without V2's shutdown; children stop (8.3); run dir left |
| Run engine or compiled program exits non-zero | EngineExitError | 17.3 | `data.stderr` holds the tail |
| Run engine or compiled program exceeds `call_timeout` | EngineTimeout | 17.3 | process tree killed |
| **Build** | | | |
| Compiler error; an export with an unreadable signature, an unsupported type or several definitions | BuildError | 15.2, 15.7 | nothing cached; memoized for the run (15.1); a definition unit is `Failed` (12.1) |
| Compiled library fails to load or lacks a `__v2_*` or `__v2x_*` symbol | FFILoadError | 15.8 | from the cache with a damaged image or missing symbols: the entry is evicted and rebuilt once, and only a second failure raises; any other cause (a missing dependency): raised at once, no rebuild |
| **FFI** | | | |
| `extern` library not found or fails to load; declared symbols missing | FFILoadError | 16.1, 16.2 | no names bound |
| Unsigned result above `i64::MAX`; returned string not valid UTF-8 | MarshalError | 16.6 | an owned string is still freed |
| **Host** | | | |
| Circular `import` | ImportError | 6.4 | no module of the cycle runs twice |
| Missing module file | ImportError | 6.4 | a later import retries |
| stdout's or stderr's reader went away (a closed pipe: `| head`, `2>&1 | head`) | the run stops quietly, exit 141 | 6.5 | children killed, run dir removed; at the final flush, exit 0 becomes 141 |
| Failed write of `print` to stdout or, with `stderr: true`, stderr (a full disk, a device error) | IOError at the `print`; none at stdout's final flush | 5.10, 6.5 | a failed final flush turns exit 0 into 1 |
| A script path that isn't valid UTF-8 (Linux) | usage error, exit 2 | 2.1 | argv is read as text |
| The interpreter thread can't start (a process or thread limit) | `v2: cannot start the interpreter thread: <reason>`, exit 1 | 1.3 | not an internal error: nothing ran |
| Failed write of a worker's output to stdout (a full disk, a device error; a closed pipe stops the run) | IOError at the block or call that wrote it, once it returns | 9.9 | the worker keeps running |
| Out of memory in the host, other than `*` and `range` (a `ValueError`) | the process aborts, as any Rust program does | 5.3 | workers die with it (8.3); the run dir stays, as after a hard kill |
| Failed write of a diagnostic, summary, `--help` or `--version` (closed stdout or stderr) | none: the write is dropped | 6.5 | exit code unchanged; never 101 or an abort |
| Interrupt: Ctrl+C, Ctrl+Break, console close, SIGINT, SIGTERM, SIGHUP, SIGQUIT | none (not catchable) | 6.5, 8.3 | child trees killed, run dir removed, exit 130 (Unix: after that, v2 ends by the signal that came, its handler reset to the default and the signal raised again, so the parent sees it killed by SIGINT and a shell stops a loop of v2 runs; SIGQUIT with no core file); no `finally` runs |
| `exit(code)`, even inside a nested cross-language or native call | none | 6.5, 15.9 | no `finally` runs (5.8); pending foreign calls abandoned; sessions get `bye`, then are killed after 1 s (8.7); run dir removed; exit `code` |
| Host panic (a V2 bug) | none | 6.5 | children killed; exit 101 |

---

## 22. Testing and conformance

### 22.1 Layout

```text
tests/
  run.rs                  harness: every .v2 test plus the multi-step cases (cargo test --test run)
  v2.toml                 comments only; stops the upward config search inside the repo
  host/                   host language, modules, builtins; host/syntax/ for diagnostics
  engines/py/ js/ ts/     bridged engines, each with fixtures/
  cross/  imports/        multi-engine programs; selectors, ambiguity, aliases, the waiting line
  sweep/                  one folder per DOCS area: expected output taken from DOCS, not the binary (roadmap 1.14)
  docs/                   expected output of the DOCS.md and README.md examples (run.rs)
  install/                smoke.sh, smoke.ps1: install a release archive and follow README.md;
                          paths.sh, paths.ps1: the install scripts' choices (22.6)
  compiled/c/ cpp/ rust/
  ffi/
  custom/                 run and bridged custom engines; custom/builtin/ for the built-in run engines
examples/fake_adapter.rs  scripted VEP/1 adapter for tests/custom (a Cargo example; not shipped)
src/**                    #[cfg(test)] unit and protocol tests
```

### 22.2 The `.v2` harness

- **Discovery.** Every `tests/**/*.v2` outside a `fixtures/` folder is a test. It runs as `v2 <file>` (`CARGO_BIN_EXE_v2`) with its own folder as the cwd, stdin from `<name>.in` or an empty closed pipe, and stdout and stderr captured, so output is always redirected. A test that needs settings has its own `v2.toml` beside it.
- **Headers.** Leading `//` comments: `// requires: py js`, `// exit: 1` (default 0), `// stderr: <text>` (repeatable; each must appear, in order), `// args: a b`, `// platform: windows|unix`, `// timeout: 60`.
- **Expected output.** Stdout must equal `<name>.out` after CRLF-to-LF conversion. A test with neither a `.out` file nor a `stderr:` line is rejected.
- **SKIP.** Once per harness run, a generated probe (run in an isolated home like the tests) prints `engine_available(t)` for every tag named in `requires:`; if it fails, every test that needs an engine fails with its error. A test that needs a missing tag reports `SKIP {file} (engine {tag} not available)` and never counts as a pass; when `V2_TEST_REQUIRE=py,js,…` is set (CI), a SKIP for a listed tag is a FAIL.
- **Isolation.** `HOME` and `USERPROFILE` point at a fresh temporary home (18.1 reads `~` from exactly these), so the `~/.v2` caches start empty; `RUSTUP_HOME` and `CARGO_HOME` keep their original values so `rustc` still works, and `V2_TEST_RUST_TOOLCHAIN`, when set, becomes `RUSTUP_TOOLCHAIN` (22.5). Inherited `V2_ENGINE_*` variables are removed, and `V2_ENGINE_FAKE` is set to the built `fake_adapter` for `tests/custom`. So are `VIRTUAL_ENV` and, from `PATH`, a folder whose parent has a `pyvenv.cfg` (an activated venv's `bin` or `Scripts`), worked out once per test process, so a developer's venv can't change which Python the runs find and so their hints; a test that wants one sets `VIRTUAL_ENV` or `PATH` itself.
- **Panic trigger.** Debug builds only (`cfg(debug_assertions)`, so never in a release binary): with `V2_TEST_PANIC` set, the interpreter thread panics at once, so a test can check 6.5's panic path (`v2: internal error: …`, exit 101). Likewise `V2_TEST_MAX_READ` (bytes) replaces the 1 GiB that `read_file` and `exec` read at most (5.10), so a test needn't fill 1 GiB.
- **WSL share.** On Windows the tests of the Windows v2 on a `\\wsl.localhost` share (run.rs `WslDir`) run only when `V2_TEST_WSL_DISTRO` names a WSL distribution, and SKIP otherwise (CI has none); each makes its folder there with `wsl.exe`, under `V2_TEST_WSL_DIR` (default `/tmp`), and removes it, a reader still waiting on one of its pipes let go first.
- **Summary.** `{p} passed, {f} failed, {s} skipped`, listing every skip with its reason; `cargo test` fails when f > 0.

### 22.3 Rust unit and protocol tests

The `engines/process` and `engines/session` tests need no Python or Node: the unit-test binary re-runs itself as the child, its role chosen by `V2_HELPER` (process tests) or `V2_WORKER` (a scripted worker; session tests also run one in process over `std::io::pipe()`). Tests that push timers take turns through `engines::process::tests::serial()`, because the watchdog is process-wide and only its top timer runs.

| Module | Cases |
| --- | --- |
| `syntax/scan` | each profile's strings, chars, comments, raw strings and templates (3.8); the fence form; C1 and C2; an unterminated block reported at its opening line |
| `syntax/directives` | exact-token `@export`/`@import` matching, `@@` escapes, lists spanning lines, preserved line counts, dedent and mixed indentation (4.4) |
| `codec/json`, `codec/vep` | int and float kinds and i64 limits, `-0.0`, non-finite floats, control characters, `"$v2"` keys, `bytes`, lone surrogates (ch. 10) |
| `ffi/cdecl` | the accepted declarations, and every 16.3 rejection with its message |
| `engines/spec`, `engines/registry` | the built-in table, aliases and version patterns; the spec, `syntax/scan` and config tables agree; unknown-tag messages; memoized failures; `engine_available` never errors; custom engine tables: every 17.1 message, reserved and invalid tags |
| `engines/resolve` (injected probes) | R41; the `PATH` walk (7.5); skip reasons and the probe cache (7.6, 7.7); configured commands and venvs, with no fallback; ts from js; the Node rule for js and ts (22.12, 23.0 and 23.1 skipped; 22.13 and 23.2 kept); 7.6's run-engine lookups (Git Bash, never `System32\bash.exe`; `powershell` after `pwsh`; `cmd` outside Windows); the not-found message (7.8); Windows `cl` from a Developer Prompt environment, else vswhere (7.6) |
| `engines/process` | the timer stack, tree kills, the interrupt path, concurrent pipes, reaping and the per-OS child rules (6.5, 8.3, 8.5). Not tested, because they matter only in races or with a console event: `CREATE_NEW_PROCESS_GROUP`, the `getppid` check, the `ESRCH` fallback |
| `engines/vep`, `codec/vep` (pure) | framing and the size limit (9.1), noise before `hello`, every 9.11 fatal frame, call targets, the `$v2` tags both ways, callable ids and releases |
| `engines/session` | handshake, `init` and startup failures; reply order and `out`; crashes, timeouts and Dead sessions; callbacks and the depth limit; releases; `bye` and the shared grace (ch. 8, 9) |
| `examples/fake_adapter.rs` | a real child process for `tests/custom`. Its "language" is one JSON object per block, e.g. `{"answer": 42, "echo": "$0", "fail": "!boom", "say": "@out:hi"}`; its first argument picks the mode (default `ok`): `ok`, `garbage`, `vep2`, `nohello`, `noise` (startup lines before `hello`), `crash`, `slow` (each call sleeps 30 s) or `nocaps`. Each mode's test folder has a `v2.toml` with `[engines.fake]` `kind = "bridged"`, `command = ["fake_adapter", "<mode>"]`; the harness's `V2_ENGINE_FAKE` replaces only `command[0]` (18.4) |

### 22.4 Mandatory regression list

| ID | Test | Asserts |
| --- | --- | --- |
| R01 | `engines/{py,js}/cr_args.v2` | `echo("a\rb\r\nc\u{2028}d")` returns the identical 8-character string; the next 50 calls `echo(i)` return `i` (no reply off by one) |
| R02 | `engines/{py,js}/float_kind.v2` | through py, `type(echo(1.0)) == "float"` and `@py(x: 1.0) { type(x).__name__ }` is `"float"`; through js, `echo(1.0)` is the int `1` and `@js { 1.5 }` is a float |
| R03 | `engines/{py,js}/nonfinite.v2` | NaN from `@py { float("nan") }` and from `@js { NaN }` satisfies `x != x`; ±inf round-trips through `echo` unchanged |
| R04 | `engines/py/nonstr_keys.v2` | `let h = @py { {1: "a"} }` gives `type(h) == "handle"` and `h[1] == "a"`; `to_v2(h)` is `{"1": "a"}`, and `to_v2` of `{None: 0, True: 1, 2.5: 2}` is `{"null": 0, "true": 1, "2.5": 2}`; `to_v2` of `{(1, 2): "a"}` or of `{1: "a", "1": "b"}` raises MarshalError (11.5's key rule) |
| R05 | `engines/py/decorators.v2` | decorator lines `@exporter` and `@export_json` reach Python untouched and the block stays an action block; lines `@@export { x }` and `@@import y` inside a triple-quoted string print as `@export { x }` and `@import y` |
| R06 | `engines/py/braces.v2`, `engines/js/braces.v2`, `compiled/c/braces.v2` | `@py { print("}") }` prints `}`; a JS template literal containing `${"}"}` works; `@c { char c = '{'; printf("%c", c); }` has value `"{"` |
| R07 | `host/syntax/unterminated.v2` | exit 1; stderr starts `unterminated.v2:3:` and contains `SyntaxError: @py block is never closed`, then the source line and a caret; stdout is empty although line 1 prints |
| R08 | `engines/py/def_in_loop.v2` | a definition block, its `@import` and `print(bump())` inside `for i in 0..3` print `loaded` once, then `1`, `2`, `3` |
| R09 | `host/try_locals.v2` | inside a function, locals declared before `try` survive a caught `throw`: with `f` throwing, `func g() { let local = 7; try { f() } catch (e) { }; return local }` gives `g() == 7`, and the same holds in a `for` body and an anonymous `func`; a name declared inside `try` is gone after it; `return` and `break` out of `try … finally` work; name lookup still works after 1000 caught throws |
| R10 | `engines/{py,js}/kwargs.v2` | `f(1, sep: "-")` arrives as a Python keyword argument; an unknown keyword raises ForeignError `TypeError`; a named arg to a JS function raises ArityError `js functions take positional arguments only (got 'sep')` |
| R11 | `engines/{js,py}/async.v2` | an exported `async function` returning 42 gives `42`; a rejected Promise raises ForeignError; a Python coroutine is awaited; a Promise returned in a nested call raises 9.8's Error |
| R12 | `engines/js/packages/` | `@import { join } from js.path` works; `@import { twice } from js."v2-test-pkg"` loads the committed package in `tests/engines/js/packages/node_modules/`, found by Node's own walk (no network); a missing package raises ImportError carrying Node's message |
| R13 | `cross/tour.v2` | DOCS 17.1 verbatim, with `math_ops.py`, prints `22`, `20`, `44`, `47` |
| R14 | `engines/py/callbacks.v2` | DOCS 8.2 prints `["Linus", "Ada", "Grace"]`; DOCS 17.5 prints `1/3`, `2/3`, `3/3` before `process` returns; a ValueError thrown in the V2 callback crosses the Python frame and is caught in V2 as ValueError, with the catching function's locals intact |
| R15 | `engines/py/datetime.v2` | with `d = @py { import datetime; datetime.date(2024, 2, 29) }`: `type(d) == "handle"`, `d.year == 2024`, `str(d) == "2024-02-29"`, `d.isoformat()` equal; `d.replace(year: 2025)` raises ForeignError `ValueError`; `d == d`; `d + 1` raises TypeError; `len(d)` raises ForeignError `TypeError` |
| R16 | `engines/{py,js}/trace_lines.v2` | an exception raised on `.v2` line 12, inside a block opened on line 10, gives `e.line == 12` and a frame `(trace_lines.v2:12, in @py risky)`; uncaught, the header, source excerpt and caret have DOCS 10's shape |
| R17 | `cross/order/` | with stdout captured, program order holds for all of: V2 `print`, `@py` `print`, `@py` `sys.stdout.write("x")` with no newline, `@js` `console.log`, a `@c` action block, an exported `@c` function that calls `printf`, and a run-engine block |
| R18 | `engines/py/crash.v2` | a call running `os._exit(3)` raises EngineCrashed naming status 3; the next call raises EngineCrashed `is not running`; the script continues |
| R19 | `engines/py/timeout/` | with `call_timeout = 1`, `time.sleep(30)` raises EngineTimeout in under 5 s; the next call raises EngineCrashed |
| R20 | `compiled/c/cache/` (run.rs: two runs with one HOME) | DOCS 11.1 `hypot3(1, 2, 2)` prints `3.0`; the second run leaves every file under `~/.v2/cache/build` unchanged (paths, sizes, mtimes), and a run with no compiler on `PATH` still prints `3.0`; with the cached library overwritten by 4096 zero bytes (a damaged image, 15.8), the next run evicts the entry, rebuilds and still prints `3.0` |
| R21 | `compiled/rust/panic.v2` | `count_primes(100000) == 9592`; an export that panics with `boom` raises ForeignError with `lang == "rust"`, `foreign_type == "panic"` and `boom` in the message; the next call works |
| R22 | `compiled/cpp/exception.v2` | `reversed("stressed") == "desserts"`; `reversed(null)` raises MarshalError; `throw std::runtime_error("bad")` raises ForeignError with message `bad` and a `foreign_type` containing `runtime_error`; `throw 42` gives `foreign_type == "unknown"` |
| R23 | `ffi/libc_libm.v2` | through `extern "m"`, `cos(0) == 1.0` and `pow(2, 10) == 1024.0`; through `extern "c"`, `strlen("héllo") == 6` |
| R24 | `ffi/range.v2` | through `extern "c"`, `abs(2147483648)` raises MarshalError naming argument 1, `int` and `-2147483648..2147483647`; a `@c` `unsigned char id8(unsigned char x)` called with 300 raises MarshalError; `strlen("a\0b")` raises the NUL MarshalError; a `@c` export returning `UINT64_MAX` as `uint64_t` raises MarshalError `return value 18446744073709551615 of '…' does not fit in a V2 int` |
| R25 | `ffi/not_found.v2` | `extern "v2_no_such_lib"` raises FFILoadError listing exactly this platform's 16.2 search order (the `.v2` file's folder, then the system search); on Windows (run.rs), a DLL present only in the cwd, with the script elsewhere, is not found |
| R26 | `imports/ambiguity.v2` | with two `@py` blocks exporting `mean`, a named import raises the DOCS 6.3 message; `@py.stats` resolves; an `@export { * }` block can be imported selectively |
| R27 | `imports/aliases.v2` | `@python stats` is reachable as `stats`, `@py.stats` and `@python.stats`; `python.statistics` is `py.statistics`; a `@python` block and a `@py` block share one process (a `sys` attribute set in one is visible in the other) |
| R28 | `host/syntax/dup_label.v2` | `@py stats` followed by `@js stats` is a SyntaxError at the second block, with a note at the first |
| R29 | `engines/js/bigint.v2` | `echo(2**60) == 2**60` through JS; `@js(x: 2**60) { typeof x }` is `"bigint"`; `@js(x: 2**53 - 1) { typeof x }` is `"number"`; `@js { 1e20 }` follows chapter 10 without overflow |
| R30 | `engines/js/proto_key.v2` | `@js(d: {"__proto__": {"x": 1}}) { d.x === undefined && Object.keys(d).length === 1 }` is `true`; `echo` returns an equal dict |
| R31 | `engines/{py,js}/surrogates.v2` | a lone surrogate from `@py { "\ud800" }` or `@js { "\ud800" }` follows chapter 10, with no panic; the session keeps working |
| R32 | `cross/stdin_owner.v2` with `.in` = `a\nb\n` | `@py { import sys; sys.stdin.read() }` raises ForeignError (9.9's stdin error) and `@js { require("fs").readFileSync(0) }` throws; then `input()` returns `a`, then `b`, then `null` |
| R33 | `engines/js/idle_timer.v2` | a `setImmediate` callback scheduled during a call, calling an imported V2 function, runs in the event-loop turn after that call's reply (14.4) and throws 9.8's idle-call Error in JS; the V2 function never runs; the stored callback still works during a later call |
| R34 | `engines/py/subprocess.v2` | in a block, `subprocess.run([sys.executable, "-c", "print('child')"])` and `os.write(1, b"x\n")` put `child` and `x` on stderr, not stdout; the next block runs (protocol intact) |
| R35 | `custom/run/` (requires js) | with `[engines.nd]` set to `kind = "run"`, `command = ["node", "{file}"]`, `ext = "js"`: statement output is in order; `@nd(t: "x y", n: [1, 2]) { process.stdout.write(process.env.V2_ARG_t + process.env.V2_ARG_n + "\r\n") }` has value `x y[1,2]`; `console.error("bad"); process.exit(3)` raises EngineExitError with `e.data.status == 3` and `bad` in `e.data.stderr`; on Windows, `@nd(p: 1, P: 2) {}` raises ValueError; `@export` inside raises ImportError |
| R37 | `custom/bridged/` | export, call, `out` and `err` work; the fake adapter's modes (22.3) `garbage`, `vep2`, `nohello`, `crash`, `slow` (with `call_timeout = 1`) and `nocaps` each produce their chapter 21 row; mode `noise` still starts, with the noise on stderr |
| R38 | `cross/depth.v2` | V2↔py mutual recursion reaching depth 64 succeeds; depth 65 raises RecursionError, which V2 catches |
| R39 | `host/bom_crlf.v2`, `engines/py/bom_crlf.v2` | stored with a BOM and CRLF line endings (`.gitattributes`: `-text`); it runs, and a `@py` error in such a file reports the right line |
| R40 | run.rs `an_interrupt_during_a_py_block_exits_130_and_cleans_up` | SIGINT, and separately SIGTERM (Ctrl+Break on Windows), sent to the host during a sleeping `@py` block exits 130 within 2 s; the worker pid is gone and the run dir is removed |
| R41 | `engines/resolve` unit test | a WindowsApps `python.exe` whose probe exits 9009 is skipped and the next candidate is used; one whose probe prints `Python 3.12.1` is kept |
| R42 | `engines/ts/strip.v2` | a `@ts t` block exporting `function dbl(x: number): number` gives `dbl(21) == 42`, and a `@js` block imports `dbl` from `t` |
| R43 | `engines/{py,js}/export_star.v2` | `@export { * }` exports a py block's `def`s and classes, but not a name brought in by `from math import sqrt`; in a js block it exports a top-level `function`, but not a `const f = () => …` unless it is listed |
| R44 | `imports/module_names.v2` | `@import { x } from py."os; print(1)"` raises the invalid-module-name ImportError and prints nothing |
| R45 | `host/optchain.v2`, `host/syntax/optchain_assign.v2` | with `let a = null`: `a?.b.c(f())` is `null` and `f` never runs; `(a?.b).c` raises TypeError; `{"b": null}?.b.c` raises TypeError; `{}?.x` raises KeyError; `a?.b = 1` is a SyntaxError |
| R46 | `engines/py/def_fail.v2` | a definition block that prints `loading` and then raises ValueError, reached twice in a loop inside `try`: both catches get ForeignError `ValueError` and the same error value (`e1 == e2`), and `loading` prints once; `@import` from it raises ImportError `block 'bad' failed to load: …` |
| R47 | `custom/run/tree_kill.v2` (requires js) | a run-engine block that starts a non-detached child which would write a marker file after 1 s returns at once; 2 s later the marker doesn't exist |
| R48 | `host/exit_codes/` (run.rs) | `exit(7)` exits 7 without running an enclosing `finally`; a SyntaxError exits 1; an invalid `v2.toml` exits 2; an uncaught EngineNotFound (`[engines.py] command` naming a missing file) exits 3; `exit(2**40)` raises ValueError |
| R49 | `host/modules/import_syntax.v2` | importing a module with two syntax errors raises ImportError at the `import`, with the first diagnostic as its message and both in `e.data` |
| R50 | `compiled/rust/calls_py.v2`, `compiled/c/reverse_error.v2`, `compiled/c/reverse_exit.v2` (requires py) | DOCS 8.3 prints `3.5`; in a `@c` export, an imported py function that raises `ZeroDivisionError` returns `NULL` and `v2_last_error()` is `ForeignError: ZeroDivisionError: division by zero` (15.9); a V2 function imported from `v2` that runs `exit(4)`, called from a `@c` export, exits 4 without running the `finally` around the export's call |
| R51 | `compiled/{c,cpp,rust}/build_error.v2` | a block opened on line 3 with an undeclared name on `.v2` line 6 raises BuildError `block 'bad' failed to compile: …` with `e.line == 6` and `e.data.diagnostics[0].line == 6` |
| R52 | `engines/py/inner_lazy.v2`, `engines/js/inner_lazy.v2`, `cross/inner_lazy.v2` (requires py js) | block `a` imports `f` from block `b` further down (py→py; js→ts): calling `f` before `b` runs raises, inside `a`, a `V2Error` with `type` `ImportError` and message `block 'b' has not run yet — 'f' can't be called before it loads`; once `b` has run, the same import calls `f`, `f`'s own exception reaches `a` unwrapped (Python `except ValueError`; JS `e.name == "RangeError"`), and the same import is then the real `f`; importing `b`'s constant `VERSION` raises ImportError `'VERSION' is a constant — pass it in as a block arg`, at `a`'s load when `b` has run, else at the first call (13.7); across sessions (py→js, a route), the early call gets the same not-run-yet message and a call after `b` has run works |
| R53 | `engines/{py,js}/mapping_iter.v2` | for a handle `h` to a py `collections.OrderedDict(a=1, b=2)` or a js `new Map([["a", 1], ["b", 2]])`, `for k in h` gives `a`, `b` and `for k, v in h` gives `a 1`, `b 2`; for a py `collections.deque([7, 8])` or a js `new Set([7, 8])`, `for x in h` gives `7`, `8` and `for i, x in h` gives `0 7`, `1 8` (11.4) |
| R54 | `engines/js/constructors.v2` | an imported `class` and an ES5 constructor (`function P() { this.n = 1 }` plus `P.prototype.get`) are called with `new` (`P().get() == 1`); `EventEmitter()`, imported from `js.events`, gives an object whose `listenerCount("x")` is `0`; a plain function sees `new.target === undefined` (11.3) |
| R55 | `imports/ambiguity_order.v2` | two `@py` blocks list `mean` in `@export`: `@import { mean } from @py` between them (only the first has run) raises the DOCS 6.3 ambiguity message; with a single declaring block that hasn't run, it raises `block 'stats' has not run yet — move this @import below it`; an `@export { * }` block defining `mean` makes the import ambiguous only once it has run |
| R56 | `compiled/c/local_files/` (run.rs) | with the cwd elsewhere: `#include "helper.h"` finds `helper.h` beside the `.v2` file; `flags = ["-I./inc"]` finds `inc/` beside the `v2.toml` (18.1); a block linked through `flags` against a library that run.rs first builds into the `.v2` folder loads; after `helper.h` changes, the next run (same HOME) rebuilds and prints the new value |
| R57 | `compiled/{c,cpp,rust}/signatures.v2` | exports are found behind attributes and calling conventions: C `__declspec(noinline) int __cdecl twice(int x)` (Windows) or `__attribute__((noinline)) int twice(int x)`, C++ `[[nodiscard]] int thrice(int x)`; Rust `fn chars(s: &'static str) -> usize` gives `chars("héllo") == 5`; a Rust export taking `&mut str` raises BuildError naming `&mut str` |
| R58 | `custom/builtin/{bash,pwsh,cmd}.v2` (each `requires:` its tag), `custom/builtin/cmd_unix.v2` (`// platform: unix`), run.rs (doctor) | with no `v2.toml`: `@sh { echo hi }` prints `hi` and `let b = @bash { printf ok }` gives `"ok"`, through Git Bash on Windows (never `System32\bash.exe`); `@pwsh { Write-Output 5 }` gives `"5"`; on Windows `@cmd(x: 1) { echo %V2_ARG_x% }` gives `"1"`; in `cmd_unix.v2`, `@cmd { }` raises EngineNotFound `cmd is Windows only`; `v2 doctor` has a row for each built-in run engine (run.rs `doctor_rows`; 7.2, 19.2) |
| R59 | `custom/define_engine/` (requires js) | with no `v2.toml`, `define_engine("nd", kind: "run", command: ["node", "{file}"], ext: "js")` then `@nd { console.log(1 + 2) }` prints `3`; a block `@nq` reached before its `define_engine` raises EngineUnknown ending `or with define_engine`; declaring `nd` again, or `py`, raises ValueError (17.2); `command: ["node"]` raises `ValueError: define_engine: command must contain {file}`; in `sub/lib.v2`, imported from `main.v2`, a command `["node", "./runner.js", "{file}"]` runs `sub/runner.js` (the caller's folder, 18.1) |
| R60 | `imports/lazy/unused/`, `imports/lazy/nothing/` | with a missing Node configured, a program with js and ts blocks and imports that calls only a py function works, and calling the js one raises EngineNotFound at that call; with Python missing too, a program that calls neither runs |
| R61 | `imports/lazy/line_order.v2` (requires py) | `random.seed(1)` in a waiting definition block, and in a module a waiting `@import` names, is in effect for a later action block; definition blocks print in source order, before the action block's output |
| R62 | `imports/lazy/{as_today,not_reached,statement,star}.v2` | a constant, a printed name and a passed function behave as if the `@import` ran at its statement; `try` around a nested definition block or `@import` catches its error; an `@import` from a block further down raises `block 'b' has not run yet — move this @import below it`; a waiting `@import` isn't changed by a block further down; a block of the running language sees a `*` block as loaded; two `*` blocks are ambiguous; an unknown name raises when the language starts |
| R63 | `imports/lazy/{files,public}.v2`, `imports/lazy/cross_order.v2` | definition blocks in a V2 module imported before, and another imported after, the language starts run in the same relative order as statements; a public imported name of a module doesn't wait; a language started by a block in the line runs what came before that block, and what came after waits for it |
| R64 | `imports/lazy/{failures,dead_at_reach,depth,depth_line}.v2` | with a line `[bad, seed]`, `seed` runs and then the first use raises `bad`'s error; calling `bad`'s function raises `block 'bad' failed to load: …` every time; a nested block that triggered the line isn't stored as failed; a block reached after `EngineCrashed` raises `EngineCrashed`; a first use at the depth limit raises RecursionError for a function and for a nested block, and a shallow use afterwards works; a line block that crosses languages at its top level loads even when started near the limit |
| R65 | run.rs `a_parked_worker_keeps_everything`, `an_idle_worker_gives_back_its_ram` and `a_worker_with_a_call_timeout_keeps_its_ram` with `V2_TEST_PARK_MS`, `engines/process` unit test (Windows) | after a park, a counter, a handle and a stored callback all still work; the working set drops; a session with `call_timeout` is not parked |
| R66 | `host/scopes.v2` | a read and a write before a local `let` act on the outer name; `x += 1` before `let x` likewise; `for x in x` reads the outer `x`; closures keep their own iteration; a module's namespace order is unchanged; did-you-mean skips names not declared yet |
| R67 | `engines/resolve` unit test | a Store placeholder is probed once across two runs sharing a cache file; a `doctor` refresh probes it again; a real alias is never cached as a placeholder |

### 22.5 CI matrix

| Job | Python | Node | C / C++ | `@rust` | `V2_TEST_REQUIRE` |
| --- | --- | --- | --- | --- | --- |
| windows-latest, MSVC | 3.13 | 22 (≥ 22.13) | `cl` found via vswhere, outside a Developer Prompt | stable | py,js,ts,c,cpp,rust |
| ubuntu-latest | 3.13 | 22 (≥ 22.13) | gcc/g++ | stable | py,js,ts,c,cpp,rust |
| macOS: the owner's Mac (Apple Silicon), run by hand over SSH, not a CI job | 3.10+ | 22 (≥ 22.13) | Apple clang | stable | py,js,ts,c,cpp,rust |
| ubuntu-22.04, minimum versions (7.2) | 3.10 | 22.13 | gcc/g++ | 1.56 | py,js,ts,c,cpp,rust |

- **Build and test.** Every job builds V2 with stable Rust and runs `cargo test`. On Windows the host target is `x86_64-pc-windows-msvc`, which also proves that libffi builds with MSVC. `@rust` blocks compile with that column's toolchain: the minimum-versions job installs Rust 1.56 beside stable and sets `V2_TEST_RUST_TOOLCHAIN=1.56` (22.2).
- **Unusual paths.** A run.rs test runs a script from a folder whose path has a space and a non-ASCII character (`v2 tést`).

### 22.6 Release builds and the install check

- **The workflow.** `.github/workflows/release.yml` runs by hand with the release's tag, when a `v*` tag is pushed, or on a push to `v2-reboot` whose commit message contains `[release]` (GitHub offers "Run workflow" only for workflows on the default branch; the tag is then `v` + the crate version). The tag must be `v` + the crate version, and `install.sh` and `install.ps1` must name it as their default `V2_VERSION`; otherwise nothing is built.
- **Builds**, `--release --locked`: `x86_64-pc-windows-msvc` on windows-latest with `-C target-feature=+crt-static` (the C runtime linked in, because a fresh Windows may have no `VCRUNTIME140.dll`); `x86_64-unknown-linux-gnu` on ubuntu-22.04, the oldest runner, so the binary needs only glibc 2.35; `aarch64-apple-darwin` on macos-latest. Each is packed as `v2-<target>.zip` (Windows) or `v2-<target>.tar.gz`, with `v2` (`v2.exe`), README.md, DOCS.md and LICENSE in a folder `v2-<target>` (so unpacking by hand makes one folder, with the guide README's links point to), plus a `.sha256` file in `sha256sum` format. The install scripts also take the alpha.1 layout, with the files at the top. Tar archives store owner 0/0 (GNU tar `--owner=0 --group=0 --numeric-owner`, bsdtar `--uid 0 --gid 0 --numeric-owner`), so unpacking one by hand as root gives files root owns.
- **The install check**, on each fresh runner before anything is uploaded: `tests/install/smoke.sh` (`smoke.ps1` in Windows PowerShell) installs the archive with the install script (`V2_ARCHIVE`) into a fresh home whose path has a space and a non-ASCII letter, checks the PATH change (the shell startup file; the user `Path` in the registry), then saves each README quickstart file (a "Save this as `X.v2`:" line and the next `v2` fence), runs it from a new shell with plain `v2`, and compares what it prints with the next `text` fence. `tests/install/paths.sh` and `paths.ps1` then check the scripts' choices below, each case in a fresh home with only the system folders on PATH: the startup files per shell, once per folder, and that a new shell then finds v2 (zsh and fish where installed; CI installs them on Linux); `V2_NO_MODIFY_PATH`; `~` and relative paths; a folder that is a file; both archive layouts; a damaged archive; a v2.exe that runs while it is replaced, twice; another v2 that comes first; `V2_VERSION` with `V2_ARCHIVE`; on Windows a temp folder deep enough that the new v2.exe's path in it passes 259 characters, and an install folder too long for v2.exe; an install folder with the PATH separator (`;`, in `paths.sh` `:` and a line break). `paths.ps1` sets `V2_NO_MODIFY_PATH` in every case but three, and fails if the user `Path` changed: two run `install.ps1` in a PowerShell whose `HKEY_CURRENT_USER` is a registry hive file of its own (`RegLoadAppKey`, `RegOverridePredefKey`), one checking that the command it gives for this window, pasted into a new PowerShell, puts a folder named with `$`, a backtick and `'` first in `Path`, the other that through `iex` in a PowerShell started with `-Command` (cmd.exe's and Git Bash's way) it says to open a new window; its `-ModifyPath` case (CI only) changes the real user `Path` and restores it.
- **The draft.** Then the six files go on a draft prerelease named `V2 <version>` that targets the built commit; an existing draft gets its files replaced and its target moved; a published release is never touched. Publishing it is a person's step, and creates the tag.
- **The install scripts.** `install.sh` (POSIX sh; Linux x86-64 and macOS arm64) and `install.ps1` (Windows PowerShell 5.1 and 7) download the pinned release's archive and `.sha256` (curl or wget; `Invoke-WebRequest` with TLS 1.2), or take `V2_ARCHIVE` (with `V2_VERSION` set too: `Note: V2_VERSION is ignored: V2_ARCHIVE names the archive to install`); check the SHA-256; run `v2 --version` before anything is replaced: `install.sh` runs the copy it puts in the install folder as `v2.new` (a temp folder may not run programs, as a server's /tmp mounted noexec doesn't, and the install folder has to), through a `sh` started with `LC_ALL=C LANGUAGE=`, so the shell's words it reads are English under any locale (bash, Fedora's sh, says them in the user's language, and an assignment in front of a command doesn't change bash's own), renamed to `v2` once it runs, and removed, with the folder if the script made it, when it doesn't; `Permission denied` there is `<dir> doesn't let programs run (its file system is mounted noexec): install into a folder that does, with V2_INSTALL_DIR` (a glibc error names the version needed; the shell's own error is shown only when no such line explains it, before `doesn't run here (see above)`; `install.ps1` unpacks only `v2.exe`, into `%TEMP%\v2-<8 hex>`, and runs it by its short (8.3) path when its own passes 259 characters, which Windows can't start, else stops: `the temp folder … is N characters long, too long for Windows to start the new v2.exe from it: set $env:TEMP to a shorter folder (such as C:\Temp), then run this again`; an install folder whose `v2.exe` would have such a path stops it first: `… is N characters long, too long a folder for v2.exe: Windows starts no program whose path has 260 characters or more; set V2_INSTALL_DIR to a shorter folder`, and one with a `;`, which PATH would split in two (only cmd reads a quoted entry), the same way: `… has a ';' in it, which PATH uses between folders: set V2_INSTALL_DIR to a folder without one`); install into `V2_INSTALL_DIR` (default `~/.v2/bin`, `%USERPROFILE%\.v2\bin`; a leading `~` and a relative path are expanded as a shell would, against the current folder, for `V2_ARCHIVE` too (install.ps1 run as a file from a folder whose path has `[` or `]`: Windows PowerShell can't enter it and starts in `$PSHOME`, so a relative path then starts at the process's own folder, `[IO.Directory]::GetCurrentDirectory()`); the folder is then used as PATH spells it, absolute and without `.`, `..` or a trailing separator; a path that is a file is an error, and so is a folder named `v2` (`v2.exe`) inside it, which is left alone) by renaming, so a running v2 is never overwritten: on Windows the old `v2.exe` moves to `v2.exe.old`, or `v2.exe.old-<8 hex>` while an older one still runs, and every `v2.exe.old*` that no longer runs is removed. Unless `V2_NO_MODIFY_PATH` is `1`, `true` or `yes` (any case), the folder goes on PATH. `install.sh` does nothing more when the folder is on PATH already and isn't in the home folder (`/usr/local/bin`: `<dir> is already on your PATH, so no startup file was changed`); a home folder's (`~/.v2/bin`) gets the lines even then, since this terminal's PATH may come from an `export` or from lines an uninstall deleted (`New terminals will find v2 too (this one has <dir> on PATH already)` when it added some). Else it appends a `# V2 (added by install.sh)` line and `case ":$PATH:" in *":<dir>:"*) ;; *) export PATH="<dir>:$PATH" ;; esac` (`$HOME/…` when under home; `\`, `"`, `$` and the backtick escaped, so any folder name works; a shell that reads two of the files gets the folder once) to each startup file of `$SHELL`, unless a line of the file is exactly that line (a commented-out one isn't): zsh `$ZDOTDIR/.zshenv` (or `~/.zshenv`); bash `.bashrc` plus the login file, `.bash_profile` if it exists or on macOS, else `.profile`; fish `${XDG_CONFIG_HOME:-~/.config}/fish/conf.d/v2.fish` with `contains "<dir>" $PATH; or set -gx PATH "<dir>" $PATH` (escaped the same, without the backtick; global, so deleting the file undoes it); else `.profile`. It names the files it changed (`~/…`); one it can't write is named on stderr with the line to add by hand, and the script then exits 1 after finishing the rest. When this terminal doesn't have the folder on PATH, it shows the command that puts it there, escaped as the line is, so it works when pasted: `Open a new terminal to use v2 (or, in this one, run: export PATH="$HOME/.v2/bin:$PATH")` (fish: `set -gx PATH "<dir>" $PATH`), or `To use v2 in this terminal, run: …` when no file could be written. A folder with a line break is refused, and so is one with a `:`, which PATH uses between folders (`<dir> has a ':' in it, which PATH uses between folders: set V2_INSTALL_DIR to a folder without one`), before anything is made; `V2_VERSION=latest` is refused with what to do (`leave V2_VERSION unset …`); Ctrl+C (and TERM, HUP) still removes its temporary folder. `install.ps1` puts the folder in front of the user `Path` in the registry, kept unexpanded, and in front of `$env:Path`; run through `iex` it says `v2` works in this window (but in a PowerShell started with `-Command` (`-c`) and without `-NoExit`, as `powershell -ExecutionPolicy Bypass -c "irm … | iex"` from cmd.exe or Git Bash, whose window keeps its PATH: `Open a new window to use v2 (cmd, PowerShell and Git Bash all find it there).`), run as a file it says to open a new window or gives the `$env:Path = '<dir>;' + $env:Path` line (the folder in single quotes, each quote in it doubled, so a `$` or a backtick in its name stays as is when pasted), since a file may run in another PowerShell. Then each says if another v2 comes first: on PATH before the folder (`install.sh`), in new windows, where the system `Path` comes before the user `Path` (`install.ps1`), or, with `V2_NO_MODIFY_PATH`, the one a terminal finds now; `install.sh` also names one this terminal runs that new terminals won't (`Note: this terminal runs another v2, ~/.local/bin/v2; new terminals run this one. To update that one instead, run this again with V2_INSTALL_DIR="$HOME/.local/bin"`, the folder escaped as the startup line's is). An archive whose `v2` doesn't run, unchecked because no `.sha256` came with it, says `if the file may be damaged, download it again with its .sha256, so it gets checked; else build it from source`. `install.ps1` says when the old `v2.exe` still runs, so `v2.exe.old` stays (the next install removes it). A failure names the file the user gave (`V2_ARCHIVE`'s name) and says to download it and its `.sha256` again, except for an archive for another computer, which names the one this computer needs: a `.zip` for `install.sh` (`… is the download for Windows: this computer needs v2-x86_64-unknown-linux-gnu.tar.gz`), an archive whose folder is `v2-<another target>` (`… is the download for another computer (aarch64-apple-darwin): this one needs …`), a `.tar.gz` for `install.ps1` (`… is the download for Linux or macOS: Windows needs v2-x86_64-pc-windows-msvc.zip`). `install.ps1` runs in a script block and never calls `exit` when run through `iex`, so `irm … | iex` can't close the user's window.
