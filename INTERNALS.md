# V2 Internals — how everything works

> **Status: design specification.** This file specifies how the rebooted V2 must be built: data
> structures, algorithms, protocols, platform rules and failure modes. **Nothing here is implemented
> yet.** User-visible behavior is defined by [`DOCS.md`](DOCS.md); where the two disagree, DOCS.md
> wins and this file gets fixed. The previous implementation is archived in [`old/`](old/).

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
- [23. Build order](#23-build-order)
- [Appendix: salvage map from old/](#appendix-salvage-map-from-old)
- [Decision log (D1–D75)](#decision-log-d1d75)

---

## 0. Principles
DOCS.md defines what V2 does; this file defines how. Where they disagree, DOCS.md wins and this file is corrected. "ch. N" and "N.M" refer to this file; "DOCS N" refers to DOCS.md.
### 0.1 Rules
| Rule | What it means in code |
| ---- | --------------------- |
| **One backend** | One tree-walking interpreter (`runtime/interp.rs`) runs the AST. There is no bytecode, no alternate mode and no `--target`. Every feature exists once, so no execution path can drop an engine statement (old/'s VM compiled every block to a no-op). |
| **Polyglot is the product** | The host is small on purpose (DOCS 3). Sessions, units and handles are core runtime objects, not plugins. When host convenience conflicts with crossing quality (exact numbers, handles instead of lossy copies, foreign lines mapped to `.v2` lines), crossing quality wins. |
| **Fail loudly** | Every unsupported situation raises an error of a listed kind (ch. 21) with a location. old/ did each of the following, so all are forbidden: printing a warning and continuing; skipping a block whose engine is missing; stripping a directive; dropping named arguments; a catch-all builtin or method that fabricates `null`/`""`/`[]`; stringifying a value that cannot cross. A value that cannot cross exactly becomes a handle or an error. It is never rounded, truncated or replaced. |
| **One codec, one protocol** | Everything that leaves the process as data goes through `codec/json` (serde_json plus `$v2` tags, ch. 10): VEP frames, `json_parse`/`json_stringify`, JSON reverse calls from compiled code (15.9) and `V2_ARG_*` values (17.4). Built-in and custom bridged adapters speak the same VEP/1 (ch. 9). There are no side channels. |
| **Spans everywhere** | Every token, AST node, thrown `ErrorValue` and block unit carries a `Span` or a `UnitLoc` (ch. 2). A thrown error without a location is a bug. |
| **Explicit tables** | Tags and aliases (7.2), scanning profiles (3.8), marshaling (ch. 10), C types (16.4) and error kinds (including `SyntaxError`, 2.4) are data in the source, and a test walks every row. Nothing converts "by best effort" outside a table. |
| **Data, never source** | V2 values are never inserted into foreign code. Block args and call arguments travel as data (DOCS 4.4, 16). A body is passed as written, with its common indentation removed and its directive lines blanked (4.4). The only foreign text V2 generates is compiled-block wrappers and shims (ch. 15). That text depends on declarations, never on runtime values. |
| **Machine-independent syntax** | Lexing and parsing never read config, the environment or toolchains, so a file gives the same AST and the same static errors on every machine. Tags are resolved at runtime (ch. 7). |
| **Nothing in the project** | Per-run files live in one private run directory. It is created on first use and removed on every exit path (20.3, 6.5). User-level files live only in `~/.v2/` (18.1). V2 writes nothing into the project; only user code does. |

### 0.2 Baseline: what old/ did
`old/` is a reference, not a base. Nothing is inherited; the appendix lists the pieces worth porting.
- **Processes.** Blocks without `@export` ran as one-shot temp-file processes. `@bash`, `@sh`, `@ps`, `@os` and `@lua` blocks ran that way and could not export. A block with `@export` started a JSON-lines worker for each *execution* of the block. A re-run block therefore spawned a second worker and made later imports ambiguous.
- **Imports.** `@import { a, b as c, * } from @lang | label | @lang.label` resolved at runtime, with the ambiguity and missing-export messages that DOCS keeps. `from py.module` worked; other languages were refused.
- **Values and errors.** Values crossed as hand-rolled JSON. `1.0` arrived as `1`, a raw `\r` desynchronized the protocol permanently, and non-JSON values failed with "not JSON serializable". Foreign exceptions arrived as bare strings, and the AST had no spans, so no runtime error had a line number.
- **Documented but missing or broken:** compiled blocks (skipped with a warning), `@import` inside blocks (stripped with a warning), JS module imports, custom tags (a lexer constant), `register_engine` (a stub), the `[engines]` config, any build cache, and `extern`/`cimport`/`std.ffi` (parse-only, or with fabricated results). Under the bytecode VM, every polyglot statement compiled to nothing. The only polyglot tests covered Python (`old/v2/testsuite/test_engines.v2`).

---

## 1. Architecture and crate layout
`v2` is one binary crate (edition 2021, stable Rust). It targets 64-bit x86_64 and aarch64 on Windows (MSVC), Linux and macOS (ch. 16); CI covers the matrix in 22.6.
### 1.1 Processes and run pipeline
```text
 v2 host process
   main thread          argv, config; starts the watchdog and interrupt handler; runs and joins the interpreter (6.1)
   interpreter thread   syntax -> runtime::interp -> engines::units -> engines::session + vep (bridged engines)
                          | engines::runonly (run engines, compiled programs) | build + ffi (in process, never unloaded)
   watchdog thread      deadlines (8.5) and interrupts (6.5); never touches values, never writes frames
 workers, one per bridged engine: python py.py (@py, py.<module>) | node js.js (@js, @ts, js.<pkg>) | custom adapter
   stdin: host->worker frames; stdout: worker->host frames; stderr inherited
 per-use children (stdin null): run engines, compiled programs, cl/gcc/clang/rustc, probes
 per-use children (stdin piped): transpilers (TS source in, JS out; 12.5)
```
**Pipeline**
1. `cli` parses argv (19.1). `config` finds the project root and merges the config layers (ch. 18). Errors here exit with 2.
2. `syntax` loads the entry file: it lexes, parses and runs the static checks (ch. 3, 4). A `SyntaxError` in the entry file is printed (2.4) and the run exits with 1, before any code runs or any tag is resolved.
   - The `import "…"` literals are followed transitively at startup: each named file is parsed once, depth-first in source order, as `v2 check` does (6.1, 19.3).
   - A diagnostic in an imported file is stored with that file and raised as `ImportError` when an `import` of it runs (6.4). A file first seen at run time is parsed then (6.4).
3. `runtime::interp` runs the entry file. `engines::units` resolves each block's tag through `engines::registry` (ch. 7) and dispatches the block (12.1):
   - bridged engines go to their session (ch. 8, 9);
   - `run` engines and compiled action blocks go to `engines::runonly` (17.3, 15.5);
   - compiled definition blocks go to `build` + `ffi` (ch. 15, 16).

   A definition block loads once. If its load fails, every later reach re-raises the stored error; the load is never retried (12.1).
4. A normal end, an uncaught error and `exit(code)` take the orderly shutdown path. Interrupts and panics take the hard path. Both are in 6.5.

**Processes**
- **Sessions.** Each bridged engine gets one worker per run. It is spawned on first use and kept until exit (ch. 8); `ts` units run in the `js` session. A worker's stdin and stdout are the VEP pipes, so `input()` is the only reader of the user's stdin (DOCS 4.6).
- **Worker output.**
  - Language-level output (`print`, `console.log`, `process.stdout.write`) travels as `out` frames, in program order.
  - Fd-level output (C extensions, `os.write(1, …)`, children that inherit stdout) goes to stderr (9.9).
    - The Python adapter moves the protocol to private non-inheritable fds, then points fd 1 at fd 2 and fd 0 at the null device (14.3).
    - Node has no `dup2`, so the Node adapter keeps the protocol on fds 0 and 1. It writes frames with a saved `fs.writeSync(1, …)` and reads with `fs.readSync(0, …)`, and wraps `process.stdout`/`stdin`/`console`, `fs` calls on fds 0–2 and `child_process` stdio (14.4, D73).
    - Native addons, and code that reopens `/dev/stdout` or bypasses the wrappers, can still corrupt the stream (fatal `ProtocolError`, 9.11).
  - Output produced while a worker is idle goes out with its next frame.
  - Before `hello`, the host forwards non-JSON stdout lines to stderr (noise from `sitecustomize`, `.pth` files or `--require`). After `hello`, any line that is not a frame is a `ProtocolError` (9.11).
- **Per-use children** get a null stdin, except transpilers, which read the TS source on a pipe (8.3, 12.5). For run engines and compiled programs:
  - as a statement, the block's stdout streams to V2's stdout in program order (the host flushes first);
  - as an expression, stdout is captured as the value, with one trailing `\n` or `\r\n` stripped;
  - stderr always streams live, and its last 4 KiB go to `e.data.stderr` of `EngineExitError` (12.1, 15.5, 17.3).
- **Spawning and killing.** Only the interpreter thread spawns, because Linux `PR_SET_PDEATHSIG` fires when the spawning *thread* exits. Every kill takes the whole process tree. Processes that a block started are killed as a tree when the block finishes or times out. Workers are killed at shutdown, and everything is killed when V2 exits (8.3).
  - **Unix:** every child starts with `setsid()` in `pre_exec`: a new session and process group with no controlling terminal. Terminal SIGINT and SIGHUP never reach it, and a `/dev/tty` prompt (git, ssh, sudo, `getpass`) fails at once (`ENXIO`) instead of stopping the child.
  - **Windows:** `CREATE_NEW_PROCESS_GROUP` plus a kill-on-close Job Object. This blocks Ctrl+C only; Ctrl+Break and console close reach every process on the console. Adapters ignore SIGBREAK (14.3, 14.4), and the host's console control handler sets `INTERRUPTED` as its first action, so failures of children caused by the same event are never reported (6.5).

### 1.2 Module map
| Module | Responsibility |
| ------ | -------------- |
| `main.rs` | Starts the threads (1.3), owns the run-dir guard and the exit paths (6.5), and maps outcomes to exit codes: **0** success; **n** for `exit(n)` (an i32; POSIX keeps the low 8 bits); **1** uncaught error or `SyntaxError`; **2** usage or config error; **3** uncaught `EngineNotFound`; **130** interrupt, always (never 128+signal); **101** internal panic. `check` and `doctor` codes: 19.1. |
| `cli/` | Hand-written argv parsing (19.1): `v2 FILE [args]`, `run`, `check FILE`, `doctor [TAG]`, `cache clean`, `--engine TAG=PATH`, `--no-cache`, `--version`, `--help`. |
| `config/` | Project root discovery, layered config (ch. 18), and the paths of `~/.v2` and the run dir. |
| `syntax/` | `span`: FileId, SourceMap, Span, UnitLoc (ch. 2). `token` and `lexer` (ch. 3). `scan`: raw bodies, profiles, fences, line states (3.7, 3.8). `directives`: body lines, `@export`/`@import`, dedent (4.4). `parser`: recursive descent and static checks (4.1, 4.5). `ast` (4.3). |
| `runtime/` | `value`, `env`, `interp`, `call`, `builtins` (including the `Out` writer), `error` (ErrorValue, error kinds, traces, rendering per 2.4), and `module` (`import "…"`, the run-once cache, cycles). See ch. 5 and 6. |
| `codec/` | The single codec (ch. 10), one serde_json value model in two modes: `json`, the plain mode of the JSON builtins (10.1); `vep`, the VEP mode with the `$v2` tags, handles, callables and base64 (10.1-10.5). |
| `engines/` | `process`: the child rules (8.3: own session or process group and Job Object, `kill_tree`, exit observed before reaping, piped streams), the watchdog thread with its timers and registered children, and the interrupt path (1.3, 6.5, 8.5); every process V2 starts goes through it. `spec` (EngineSpec and the built-in table), `registry` and `resolve` (discovery, probes, `toolchains.json`): ch. 7. `session` and `vep`: ch. 8, 9. `units`: ch. 12. `imports`: ch. 13. `handles`: ch. 11. `runonly`: 17.3, 15.5. |
| `adapters/` | `py.py` and `js.js`, embedded with `include_str!` and installed before launch (14.1). |
| `build/` | `mod`, `cache`, `c`, `cpp`, `rust`, `shims` (ch. 15). |
| `ffi/` | `cdecl` parses C declarations: `extern` bodies for the parser, prototypes and definition heads for ch. 15. Also `types`, `lib`, `call` (ch. 16). |
| `doctor/` | `v2 doctor`, the deep test `v2 doctor TAG`, and `v2 check` (19.2, 19.3). |

### 1.3 Threads
- **Main thread.** Parses argv, loads config, starts the watchdog, installs the interrupt handler, then spawns the interpreter thread (stack size: ch. 5) and joins it (6.1).
- **Interpreter thread.** The logical main thread. It is the only thread that touches `Value` (`Rc`/`RefCell`, not `Send`), session pipes, native libraries and `Out`.
  - Foreign calls are synchronous. While it waits for a reply, it serves the worker's re-entrant calls itself (9.7).
  - A worker may call into V2 only while the host has a request outstanding to that worker (9.8). Compiled code may call into V2 only from this thread, while the V2→native call is running (15.9).
  - Compiled and `extern` code runs on this thread, so APIs that insist on the process main thread (macOS AppKit) do not work in v1. Host functions that native code calls wrap their bodies in `catch_unwind`.
- **Watchdog.** Enforces the startup, call, probe and grace deadlines by killing child trees (8.5), and carries out the interrupt path (6.5).
- **Interrupts** (Ctrl+C, Ctrl+Break, console close, SIGINT, SIGTERM, SIGHUP) are handled by the host alone (1.1). The interrupt handler only sets `INTERRUPTED` (an `AtomicBool`).
  - **Unix:** a `sigaction` handler for SIGINT, SIGTERM and SIGHUP that does nothing else (no lock, no Condvar, no allocation). A signal ignored at startup (`nohup`) stays ignored.
  - **Windows:** the `SetConsoleCtrlHandler` routine runs on its own thread. It sets `INTERRUPTED`, notifies the watchdog's Condvar (8.5), then blocks until the process ends, because returning early would let `CTRL_CLOSE_EVENT` terminate the process before cleanup.
  - The watchdog waits on its Condvar with a timeout of at most 100 ms and checks `INTERRUPTED` on every wake. Once it is set, the watchdog `kill_tree`s every registered child (8.7), removes `<run>` (best effort) and ends the process with `_exit(130)` or `ExitProcess(130)`.
  - `finally` blocks do not run on this path, and they do not run for `exit(code)` either. In both cases engines are still stopped and the run dir is removed (6.5).
- **Output order.** One writer, `Out` (5.10), carries `print`, worker `out` text and forwarded child stdout. Native stdio is flushed lazily before host output (16.7). Language-level output of every engine therefore interleaves in program order, also when output is redirected (DOCS 4.6). Fd-level output inside workers goes to stderr (1.1). A failed write to stdout raises `IOError`.

### 1.4 Dependencies
| Crate | Used for | Why a crate |
| ----- | -------- | ----------- |
| `serde_json` (`preserve_order`, `float_roundtrip`, `unbounded_depth`) | codec, VEP frames, JSON builtins | Strict parsing (it rejects lone-surrogate escapes), escaping of every control character, exact float round-trip. The hand-rolled old codec caused the `\r` desync and sent `1.0` as `1`. The codec bounds nesting itself (10.1). |
| `toml` (`preserve_order`) | `v2.toml`, `~/.v2/config.toml` | Full TOML; the old reader had no arrays or numbers. Files are read as `toml::Table` with hand-written key checks, so "unknown key" errors are exact (18.5); `preserve_order` keeps warnings and the first error in file order. |
| `indexmap` | dict storage | Insertion order with O(1) lookup (DOCS 3.2). |
| `libloading` | compiled blocks, `extern` | Portable `dlopen`/`LoadLibraryExW` that reports the OS error text (15.8, 16.2). |
| `libffi` | calls whose signature is known only at runtime | Per-ABI call frames (SysV, Win64, AAPCS64). The crate builds a vendored libffi, so building v2 needs a C compiler (and `ml64` on MSVC); the binary has no runtime dependency. |
| `sha2` | build and probe cache keys (15.6, 7.7) | Standard SHA-256. |
| `libc` | `kill`, `setsid`, `sigaction`, `waitid`, `prctl(PR_SET_PDEATHSIG)`, `getentropy`, `fflush`, `_exit` | Direct OS calls. |
| `windows-sys` | Job Objects; `CREATE_NEW_PROCESS_GROUP`; `SetConsoleCtrlHandler`; the run-dir DACL and `BCryptGenRandom` (20.3); `SetDllDirectoryW(L"")` at startup, so the cwd never joins the DLL search (16.2); `LoadLibraryExW` flags | Direct OS calls. |

Not used: an async runtime, a CLI or regex crate (the few patterns in ch. 7 and 15 are hand-written matchers), `rand`, serde derive.

---

## 2. Source, spans and diagnostics
### 2.1 Source files
```rust
pub struct FileId(pub u32);        // index into SourceMap::files; the entry script is 0
pub struct SourceFile {
    pub path: PathBuf,             // fs::canonicalize with `\\?\` stripped: the dedupe key, and the path adapters get (9.4)
    pub display: Rc<str>,          // what messages and e.file print
    pub text: Rc<str>,             // normalized text (3.1); every offset refers to it
    pub line_starts: Vec<u32>,     // byte offset of each line start
}
pub struct SourceMap { files: Vec<SourceFile>, by_path: HashMap<PathBuf, FileId> }
```
- **Ids** follow the startup parse order (6.1 step 2): the entry file is 0, then the files its `import "…"` literals name, followed transitively and depth-first in source order, as `v2 check` does (19.3). A file first seen at run time (6.4 step 1) gets the next id then. A path that is already known returns its existing id, so each file is read once per run and `UnitId`s stay stable.
- **Display path.** Under the canonicalized cwd, it is relative, with `/` on every OS (`src/util.v2`). Otherwise it is the absolute path with `\\?\` stripped (`\\?\UNC\` becomes `\\`). `e.file` is `display`.
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
    pub body_first_line: u32,      // .v2 line of body line 1; sent as `line` in `load` (9.4)
    pub body_col: u32,             // .v2 column of body line 1, column 1
    pub dedent_width: u32,         // chars removed from each dedented line (4.4)
    pub line_count: u32,           // lines in the processed body
    pub kept: Rc<[u32]>,           // body lines left undedented (4.4 step 3), sorted
}
impl UnitLoc { pub fn col(&self, body_line: u32, fcol: Option<u32>) -> u32; }
```
- **Lines.** Adapters compile every unit with the `.v2` path as the file name and `body_first_line - 1` as the line offset (12.3, 12.4, 14.5). A frame inside a block therefore already carries its `.v2` file and line. Directive lines are blanked, never removed (4.4), so line numbers never drift.
- **Columns** arrive relative to the processed body line, in Unicode scalar values, or as `null` (12.6). Adapters convert V8's UTF-16 units and CPython's UTF-8 byte offsets first.
  - The host finds the block whose body covers the frame's line. If several one-line blocks share that line, it takes the first.
  - It maps the column as follows: `body_col + fcol - 1` on body line 1, `fcol` on a `kept` line, `dedent_width + fcol` on any other line.
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
- **Excerpt.** Printed whenever the location is in the SourceMap, for every error kind, `ForeignError` included.
  - Leading whitespace is removed, the rest is indented 4, and each tab prints as one space. The caret follows `4 + (col - 1 - removed)` spaces.
  - A line longer than 120 chars is cut to a 120-char window around the caret, with `...` at each cut end.
  - In multi-line messages (compiler output, foreign messages), the first line goes in the header and the rest follows the caret, indented 4.
- **Notes and hints** come only from static diagnostics (`SyntaxError`, config errors). A runtime error that needs a hint carries it in its message.
- **Cause.** When `cause` is set, it prints as `caused by:` blocks, at most 8 levels deep.
- **Frames** (2.5) print innermost first. When there are more than 100, the first 50 print, then `  ... N frames omitted`, then the last 20. `e.trace` keeps all of them (5.9).
- **BuildError** prints up to 20 of its `data.diagnostics` after the excerpt, one per line, as `file:line:col: severity: message` (15.7).
- **EngineExitError** does not repeat the child's stderr, because that already streamed live (1.1).

```text
main.v2:3:16: ForeignError: ZeroDivisionError: division by zero
    return a / b
           ^
  at ratio (main.v2:3, in @py risky)
  at main.v2:9
```
**`SyntaxError`** is the kind of every lexer, parser and static-check diagnostic, for example `main.v2:3:1: SyntaxError: @py block is never closed`. It is in the error-kind table, but it can never be caught in the file that contains it, because that file never runs.
- **In the entry file**, every diagnostic is printed in source order and the run exits with 1 (1.1).
- **In a file loaded by `import`**, the diagnostics are not printed. The importer raises `ImportError` instead: its message is the first diagnostic and its `data` holds all of them (6.4).
### 2.5 Frames and messages
| Frame | `e.trace` entry (the renderer prefixes `  at `) |
| ----- | ----------------------------------------------- |
| V2 `func` / lambda | `parse_port (main.v2:4)` / `lambda@main.v2:7 (main.v2:7)` (lambda names: 5.7) |
| top level of a file | `main.v2:10` |
| foreign frame in a block | `ratio (main.v2:3, in @py risky)`; for an unlabeled block, `(main.v2:3, in @py block at main.v2:1)` (12.6) |
| foreign frame in a foreign file | `mean (/usr/lib/python3.12/statistics.py:430, in py)` |

- **Frame lines.** The innermost frame's line is where the error was raised; every other frame's line is the call it is executing.
- **Names.** Tags always print in canonical form (`@py`, even when the source says `@python`). Foreign function names print as the engine reports them (`<module>`).
- **Parse errors** read `expected <what>, found <token>`, with tokens named as: name `x`; keyword `let`; number `42`; string / f-string / bytes literal; an operator or bracket in backticks; end of line; `;`; end of file; block `@py stats`; block body; `@import`; `@export`; a selector tag `@py`.
- **Message style.** Messages start lower-case and have no final period. They put names in `'…'` and code in backticks, as DOCS does. Rust `Debug` output never appears (old/ printed things like `Ident("hi")`).

---

## 3. Lexer
### 3.1 Input
`SourceMap::load` does three things:
- It rejects invalid UTF-8 (`file is not valid UTF-8`, at the line of the first bad byte).
- It strips one leading U+FEFF, which Windows editors and PowerShell add.
- It turns every CRLF and every lone CR into LF.

The normalized text is what gets stored, spanned, scanned and handed to engines, so block bodies always use LF. (Run files with ext `bat`/`cmd` are written with CRLF, 17.3.) The lexer turns the whole file into one `Vec<Token>` before parsing starts.
### 3.2 Tokens
```rust
pub struct Token { pub tok: Tok, pub span: Span }
pub enum Tok {
    Int(u64), Float(f64), Str(Rc<str>), Bytes(Rc<[u8]>), FStr(Vec<FPartTok>),
    Ident(Rc<str>), Kw(Kw), Op(Op), LBrace(BraceKind /* Body | Dict, 3.3 */), RBrace,
    DirImport, DirExport, AtTag(Rc<str>),               // `@import`, `@export`, `@py` in a selector
    BlockHead { tag: Rc<str>, label: Option<(Rc<str>, Span)> },
    BlockBody(Box<RawBody>), ExternBody(Box<RawBody>), Newline, Semi, Eof,
}
pub enum FPartTok { Lit(Rc<str>, Span), Expr { toks: Vec<Token>, fixed: Option<u8>, span: Span } }
pub struct RawBody { pub text: Rc<str>, pub text_span: Span, pub open: Span, pub close: Span,
                     pub delim: Delim /* Brace | Fence(u8): n backticks */, pub starts_in_code: Vec<bool> /* 3.8 */ }
```
- **Keywords** (DOCS 18, case-sensitive): `let const func lambda return if elif else while for in not break continue try catch finally throw import as from extern true false null`. A keyword directly after `.` or `?.` lexes as `Ident` (`h.from`, `result.in`), so every rule below sees a member name there.
- **Operators**, longest match first: `** ... .. ?. ?? => == != <= >= && || += -= *= /= %=`, then `+ - * / % = < > ! ? : . , ( ) [ ] ;`.
  - `&`, `|`, `^`, `~`, `<<` and `>>` are errors: "V2 has no bitwise operators".
  - Any other character is `unexpected character '#'`; for `#` the hint says comments start with `//`.
- **Identifiers** start with `_` or a char for which `char::is_alphabetic` is true, and continue with those or ASCII digits. They are compared by code point, with no normalization.
- **Comments.** `//` runs to the end of the line. `/* */` does not nest, and an unterminated one is an error at its start. A block comment that contains a line break counts as one line break.
### 3.3 Where statements end
The lexer keeps a stack of open brackets: `Paren`, `Bracket`, `Dict`, `Body` and `Interp` (an f-string `${`).

**Body and Dict braces.** A `{` is a **Body** brace when the previous token is `)`, `]`, `}`, a name, a literal (number, string, f-string, bytes, `true`, `false`, `null`), or one of `else`, `try`, `catch`, `finally`. Any other `{` is a **Dict** brace: after an operator, `(`, `[`, `,`, `:`, `=`, `=>`, `return`, `throw`, `in`, `Newline`, `;` or `@import`, or at the start of the file or of an interpolation.
- So `if x {`, `} else {`, `func f() {` and `lambda(s) {` open bodies.
- `let d = {`, `f({`, `@import {` and the first brace of `for k, v in {a: 1} {` open dicts.
- Block and `extern` braces never reach this rule (3.7, 3.9).
- Closers must match their openers (`found ] but ( opened at 3:5 is still open`). A bracket still open at end of file is an error at its opener.

**Line breaks.** A line break becomes a `Newline` token unless one of these holds:
1. The stack top is `Paren`, `Bracket`, `Dict` or `Interp`.
2. The previous token is `,` `=>` `=` `+=` `-=` `*=` `/=` `%=` `+` `-` `*` `/` `%` `**` `==` `!=` `<` `>` `<=` `>=` `&&` `||` `??` `?` `:` `!` `.` `?.` `..` `...`, or the keyword `in`, `not`, `as` or `from`.
3. The next line that is not blank or comment-only starts with `.` (but not `..`) or with `?.`.

Other rules:
- Inside a `Body` the stack top is `Body`, so a `lambda(x) { … }` inside a call still separates its statements.
- A run of line breaks produces one `Newline`, and none comes before the first token. `;` is a `Semi` token.
- The parser ends a statement at `Newline`, at `Semi`, before `}` and at `Eof`. It skips `Newline`s before `elif`, `else`, `catch` and `finally`.
- A line that starts with a binary-only operator (`+ * / % ** == != < > <= >= && || ?? ?`, `in`, `not in`) does not continue the previous one: `+ b` fails with the hint "end the previous line with the operator". After an `if`, `elif`, `while` or `for` head the error is `expected {, found `&&` on the next line`, with the same hint.
- **A body `{` must be on its header's line.** This applies to `if`, `elif`, `else`, `while`, `for`, `func`, `lambda`, `try`, `catch` and `finally`. After a `Newline`, a `{` is a Dict brace, so the parser reports `expected {` with the hint "put { at the end of the if line".
### 3.4 Numbers
- **Integers.** `0`, `[1-9][0-9_]*`, or `0x` followed by hex digits. `_` is allowed only between two digits. These are errors: leading zeros (`007`), `0b`, `0o`, and a number followed directly by an identifier character (`12abc`, `0x`, `0X1`, `1f`).
- **Floats.** Digits `.` digits with an optional exponent, or digits followed by an exponent (`e|E [+|-] digits`).
  - A `.` joins the number only when a digit follows it. So `0..3` lexes as `Int .. Int`, `1.len()` is a member call, and no float starts with `.`.
  - Floats are parsed with `str::parse::<f64>` (correctly rounded) after removing `_`. A result of ±inf is an error: "float literal is out of range".
- **Integer range.** `Int` holds the magnitude; anything above 2^63 is an error: "integer literal does not fit in 64 bits". Exactly 2^63 is accepted only as the entire operand of a unary minus (`-9223372036854775808`, which gives `i64::MIN`). Anywhere else, including `-9223372036854775808 ** 1`, it is the same error.
### 3.5 Strings, bytes, f-strings
| Form | Escapes | Line breaks inside |
| ---- | ------- | ------------------ |
| `"…"` `'…'` | yes | no: "unterminated string" at the opening quote (hint: `\n` or `"""`) |
| `"""…"""` | yes, decoded after dedent | yes, see below |
| `r` + any form | none: `\` is literal and the first matching quote ends the string | as its form |
| `f` + any form / `b` + any form | in literal parts / the byte subset | as its form |

- **Prefixes.** A prefix is a word that is exactly `r`, `f` or `b`, directly followed by a quote. A literal takes at most one: `rf`, `br` and upper-case prefixes are errors.
- **Escapes.** `\n \t \r \\ \" \' \0 \$`, `\xNN` and `\u{…}`.
  - `\xNN` takes exactly 2 hex digits. It is U+00NN in a string and the byte NN in bytes.
  - `\u{…}` takes 1–6 hex digits naming a Unicode scalar value.
  - Any other escape, including `\` before a line break, is an error at the backslash: `unknown escape \d` (hint: `\\` or a raw string).
- **Bytes literals** must be ASCII. They take the same escapes, except `\u{}`.

**Triple-quoted strings** (DOCS 3.5), for every prefix:
1. The opening `"""` ends its line: only spaces and tabs may follow it. Otherwise the error is `the text of a """ string starts on the line after the opening """`. The line break after the opener is not part of the content.
2. The closing `"""` must have only whitespace W before it on its line (error: `the closing """ must be on its own line`). Ordinary tokens may follow it, as in `""")`.
3. Each content line either starts with W, which is removed, or is whitespace-only, which becomes empty. Any other line is an error at that line: `indented less than the closing """, or with different tabs/spaces`. In f-strings, lines that begin inside `${…}` are neither checked nor dedented.
4. The line break before the closing line is dropped. So a literal whose four source lines are `"""`, `    a`, `    b` and `    """` is `"a\nb"`; to end with `\n`, leave an empty line before the closing `"""`. Escapes (none for `r"""`) are decoded after dedent, so an escaped `\n` never starts a line.

**f-strings**
- Literal text runs until `${`. A `$` anywhere else is literal, and so is `\$`.
- `${` starts an interpolation. The lexer lexes ordinary tokens recursively, on a fresh stack whose bottom is `Interp`, until the `}` that closes it. So nested strings, even with the same quote, and nested f-strings work: `f"${d["k"]}"`.
- **Format spec.** While no bracket inside the interpolation is open, the exact characters `:` `.` one or two digits `f` `}` end it with `fixed = Some(N)` (DOCS 3.5 `:.Nf`). Any other `:` is an ordinary token, as in a ternary.
- `${}` is an error. Line breaks inside an interpolation are allowed only in triple-quoted f-strings.
### 3.6 The `@` sigil
- `@import` and `@export` (the word ends there) lex as `DirImport` and `DirExport`.
- `@` TAG right after `from` or `DirImport` is `AtTag(TAG)`, a selector (`@py`, `@py.stats`).
- `@` TAG anywhere else is a `BlockHead`, continued by 3.7.
- `@` not followed by an ASCII letter or `_` is the error `expected an engine tag after @`.

TAG is ASCII `[A-Za-z_][A-Za-z0-9_]*` and case-sensitive. The lexer never checks it: `@lua` lexes whether or not an engine exists. A tag that is unknown, or not a valid engine tag (`[a-z][a-z0-9_]{0,31}`, 17.1; so `@PY` too), raises `EngineUnknown` only when its block is reached (ch. 7). V2 has no decorators: `@memo` followed by `func` fails as a block header, with a hint that says so.
### 3.7 Block heads and raw bodies
The header is `'@' TAG [LABEL] ['(' args ')'] ( '{' | FENCE )`. Outside the args, only spaces and tabs may appear between its parts.
- **LABEL** is an identifier that is not a keyword, or a plain (unprefixed), one-line, non-empty string literal. A keyword is not taken as a label; the error hints `@py "for" {`.
- **args** are ordinary tokens under a `Paren` entry, so they may span lines. The `{` or fence must be on the line of the header's last token.
- **Brace on the next line.** If the next non-blank line starts with `{`, the error is `the { that opens a block must be on the same line as @py`, at that `{`, with a note at the header and the hint "move { to the end of line N". Anything else there gives `expected { or a ``` fence after @py stats`.

**Brace form.** The body starts after `{`, and the profile scanner (3.8) finds the matching `}`. Two guards catch a scanner that closed early on a brace hidden in a construct it does not understand. Both raise a `SyntaxError` whose hint names the fence form ("if that } belongs to the foreign code, use the fence form"):
- **C1.** If the body spans lines, the closing `}` must be the first non-blank character of its line: `@py block closes at a } that does not start its line`. It is reported at the block's `@` (DOCS 4 Errors: "at the opening line"), with a note at the offending `}`.
- **C2.** After the closing `}`, the rest of its line must be blank, a `//` or `/*` comment, or text starting with `)`, `]`, `}`, `,` or `;`. Otherwise the error is `unexpected text after the } that closes the @py block`. So `@py { 1 } + 2` and `c ? @py { 1 } : 2` are errors; the hint also shows `(@py { 1 }) + 2` (4.1).

**Fence form.** The header ends with N ≥ 3 backticks followed by nothing but whitespace (```` ```python ```` is an error). The body is every following line up to the first line whose trimmed text is exactly N backticks; that closing line is consumed. Nothing inside a fence is scanned for its end.

**Unterminated block.** If the end of the file comes first, the error is `@py block 'stats' is never closed` (unlabeled: `@py block is never closed`), at the block's `@`. For the brace form, notes point at the innermost `{` still open inside the body and at any foreign string, comment or template that never closed; the hint names the fence form.

**Output.** Otherwise the lexer emits `BlockHead`, the arg tokens, then `BlockBody`. Its `text` is the exact text between `{` and `}` (or the lines between the fences), directives and indentation included (4.4).
### 3.8 Scanning profiles
A tag's profile depends only on the tag's text, never on config or `register_engine`. Built-in ids and aliases take the profile of their static spec (7.1, 7.2):
- **Python:** `py`, `python`.
- **CLike**, with a dialect: `js`, `javascript`, `ts`, `typescript` (Js); `c` (C); `cpp`, `cxx` (C++); `rust`, `rs` (Rust).
- **Plain:** every other tag, meaning every custom engine. Plain counts every `{` and `}`.

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
- **Not recognized:** regex literals, preprocessor lines with unbalanced braces, heredocs, unusual string forms. A brace or quote inside one of them can end the block early, or never. C1, C2 and the unterminated error make that failure loud, and the fence form is the fix (DOCS 4.2).
- **Python f-strings** are scanned as plain strings. PEP 701 same-quote nesting still scans correctly whenever its quotes pair up.
- **Line states.** For every body line, the scanner records whether it starts in Code or inside a multi-line string, raw string, template or block comment (`RawBody.starts_in_code`, used by dedent in 4.4). A fenced body is scanned only for these states, never for its end. A Plain body marks every line as Code.
### 3.9 `extern` bodies
After `extern`, the lexer expects a string literal, an optional `as` NAME, and `{` on the same line. It captures the body with the C dialect of CLike, under C1, C2 and the unterminated rule, and emits `ExternBody`. The parser hands the body to `ffi::cdecl` at once, so a declaration V2 cannot read is a `SyntaxError` at the offending token (16.1, 16.3). `extern` has no fence form.

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
          | import | engine_import | extern | block | expr [ assign_op expr ]
let       = "let" NAME [ "=" expr ]            const = "const" NAME "=" expr
assign_op = "=" | "+=" | "-=" | "*=" | "/=" | "%="                     throw = "throw" expr
if        = "if" expr body { "elif" expr body } [ "else" body ]        while = "while" expr body
for       = "for" NAME [ "," NAME ] "in" expr [ ".." expr ] body
func      = "func" NAME "(" params ")" body    params = [ param { "," param } [ "," ] ]
param     = NAME [ "=" expr ] | "..." NAME     return = "return" [ expr ]  (* bare before SEP, "}", EOF *)
try       = "try" body [ "catch" [ "(" NAME ")" ] body ] [ "finally" body ]
import    = "import" STRING "as" NAME          extern = "extern" STRING [ "as" NAME ] RAW
engine_import = "@import" ( "{" items "}" "from" selector | selector "as" NAME )
items     = "*" | item { "," item } [ "," ]    item = ( NAME | STRING ) [ "as" NAME ]
selector  = AT_TAG [ "." label ] | label | NAME "." segment { "." segment } | "v2"
label     = NAME | STRING                      segment = NAME | STRING
block     = BLOCK_HEAD [ "(" [ block_arg { "," block_arg } [ "," ] ] ")" ] RAW
block_arg = NAME [ ":" expr ]                  expr = block | ternary      (* see "Blocks" below *)
ternary   = coalesce [ "?" expr ":" expr ]     coalesce = or { "??" or }
or        = and { "||" and }                   and = equality { "&&" equality }
equality  = compare { ( "==" | "!=" ) compare }
compare   = additive [ ( "<" | ">" | "<=" | ">=" | "in" | "not" "in" ) additive ]
additive  = term { ( "+" | "-" ) term }        term = unary { ( "*" | "/" | "%" ) unary }
unary     = ( "-" | "!" ) unary | power        power = postfix [ "**" unary ]
postfix   = primary { "(" [ arg { "," arg } [ "," ] ] ")" | "[" subscript "]" | ( "." | "?." ) NAME }
arg       = expr | NAME ":" expr | "..." expr  subscript = expr | [ expr ] ":" [ expr ]
primary   = INT | FLOAT | STRING | FSTRING | BYTES | "true" | "false" | "null" | NAME
          | "(" expr ")" | list | dict | lambda
list      = "[" [ expr { "," expr } [ "," ] ] "]"      lambda = "lambda" "(" params ")" ( "=>" expr | body )
dict      = "{" [ entry { "," entry } [ "," ] ] "}"    (* Dict brace *)
entry     = ( NAME | STRING | "[" expr "]" ) ":" expr
```
- **Blocks.** A statement that starts with `@TAG` is a block statement, ended by `SEP`, `}` or `EOF`.
  - Anywhere else, a block is a whole `expr` (a `let` value, an argument, an element, a parenthesized operand), and it must be an action block (4.5).
  - An operator or postfix after a block value is rejected. On the same line, C2 catches it (3.7). On the next line, where `.m()` continues the statement by 3.3 rule 3, the parser reports `a block value must be in parentheses here: (@py { … }).m()`.
  - A block in operand position (`1 + @py { 2 }`, `-@py { 2 }`, `a ?? @py { 1 }`, a `for` head) reaches `primary`, which has no block: `a block value must be in parentheses here`, at the `@`.
- **Assignment** is a statement, not an expression. Targets are NAME, `postfix "." NAME` and `postfix "[" expr "]"`. Any other target is `cannot assign to this expression`, and a target chain that contains `?.` is `cannot assign through ?.`.
- **Optional chains.** The parser wraps a postfix chain that contains `?.` in `OptChain`. The chain runs to the end of the postfix sequence, and parentheses end it: `(a?.b).c` leaves `.c` outside.
  - When a `?.` finds `null`, the whole chain is `null`, and its remaining steps are not evaluated, arguments included. So `a?.b.c()` with `a == null` is `null`, as in JS.
  - Other steps behave normally: `.c` on a `null` `a.b` is a `TypeError`, and a missing dict key is a `KeyError` (5.5).
- **Operands and arguments.**
  - `-2 ** 2` is `-(2 ** 2)`, and `2 ** -1` is valid.
  - Named args come after all positional and spread args. A named arg is `NAME` then `:` (two-token lookahead), so `f(c ? a : b)` is positional.
  - The block arg `nums` is short for `nums: nums`.
  - Past 256 levels of nested expressions, bodies and interpolations, the parser stops with `nested too deeply`. Left-associative chains (`a + b + …`, `a && b && …`, `a.b.c…`, `f()()…`) build a tree as deep as the chain is long, so the tree's binary-operator and postfix nodes are counted separately: when a node would sit more than 10,000 such nodes deep, counting the operands already under it (a parenthesized group, a tighter chain) and the expression it is part of (an enclosing chain, a lambda body's expression), the parser stops with `expression too long` (hint: split it into several statements with let). Both limits protect the host stack from generated input; every later pass (4.5, ch. 5) may recurse over the tree.

### 4.2 Precedence
The grammar encodes DOCS 3.4's table, from lowest to highest:

| Level | Operators | Associativity | AST |
| ----- | --------- | ------------- | --- |
| 1 | `c ? a : b` | right | `Ternary` |
| 2–4 | `??`, `\|\|`, `&&` | left, short-circuit | `Coalesce`, `Or`, `And` |
| 5 | `==` `!=` | left | `Binary` |
| 6 | `<` `>` `<=` `>=` `in` `not in` | none: chaining is an error | `Binary` |
| 7–8 | `+` `-`; `*` `/` `%` | left | `Binary` |
| 9 | prefix `-` `!` | prefix | `Unary` |
| 10 | `**` | right | `Binary(Pow)` |
| 11 | call, index, slice, `.`, `?.` | postfix | `Call`, `Index`, `Slice`, `Member`, `OptChain` |
### 4.3 AST
```rust
pub struct Name { pub text: Rc<str>, pub span: Span }      pub struct Stmt { pub kind: StmtKind, pub span: Span }
pub enum StmtKind {
    Let(Name, Option<Expr>), Const(Name, Expr), Expr(Expr), Break, Continue, Return(Option<Expr>), Throw(Expr),
    Assign { target: Target, op: Option<(BinOp, Span)>, value: Expr },  // op None = plain `=`; Span: the operator
    If { arms: Vec<(Expr, Body)>, else_: Option<Body> }, While { cond: Expr, body: Body },
    For { first: Name, second: Option<Name>, iter: ForIter /* Value(Expr) | Range(Expr, Expr) */, body: Body },
    Try { body: Body, catch: Option<(Option<Name>, Body)>, finally: Option<Body> },
    Func(Rc<FuncDecl>), Import { path: Rc<str>, alias: Name }, EngineImport(Rc<EngineImport>), Extern(Rc<Extern>), Block(Rc<EngineBlock>),
}
pub enum Target { Name(Name), Member(Box<Expr>, Name), Index(Box<Expr>, Box<Expr>, Span /* the `[` */) }
pub struct Body { pub stmts: Vec<Stmt>, pub funcs: Vec<usize> /* hoisted */, pub span: Span }
pub struct FuncDecl { pub name: Option<Name>, pub params: Vec<(Name, Option<Expr>)>, pub rest: Option<Name>,
                      pub body: FuncBody /* Block(Body) | Expr(Expr) */, pub span: Span }
pub struct Expr { pub kind: ExprKind, pub span: Span }
pub enum ExprKind {
    Null, Bool(bool), Int(i64), Float(f64), Str(Rc<str>), Bytes(Rc<[u8]>), Name(Rc<str>),
    FStr(Vec<FPart>),                     // FPart = Lit(Rc<str>) | Expr { expr, fixed: Option<u8> }
    List(Vec<Expr>), Dict(Vec<(DictKey, Expr)>),     // DictKey = Str(Rc<str>, Span) | Computed(Expr)
    Unary(UnOp, Span, Box<Expr>), Binary(BinOp, Span, Box<Expr>, Box<Expr>),    // Span: the operator
    And(Box<Expr>, Box<Expr>), Or(Box<Expr>, Box<Expr>), Coalesce(Box<Expr>, Box<Expr>), Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Call { callee: Box<Expr>, args: Vec<Arg>, paren: Span },  // Arg = Pos | Named(Name, Expr) | Spread
    Index { obj: Box<Expr>, index: Box<Expr>, bracket: Span }, Slice { obj: Box<Expr>, lo: Option<Box<Expr>>, hi: Option<Box<Expr>>, bracket: Span },
    Member { obj: Box<Expr>, name: Name, optional: bool },    // method call = Call with a Member callee
    OptChain(Box<Expr>), Lambda(Rc<FuncDecl>), Block(Rc<EngineBlock>),
}
// UnOp = Neg | Not;   BinOp = Add Sub Mul Div Rem Pow Eq Ne Lt Gt Le Ge In NotIn
pub struct UnitId { pub file: FileId, pub offset: u32 }                 // byte offset of the `@` (6.2)
pub struct EngineBlock {
    pub unit_id: UnitId, pub tag: Name /* as written */, pub label: Option<Name>, pub args: Vec<(Name, Expr)>,
    pub body: Rc<str>, pub loc: UnitLoc,   // processed text and its location (4.4, 2.3)
    pub body_span: Span, pub delimiter: Delim, pub directives: Directives, pub span: Span,  // span: `@` .. closing delimiter
}
pub struct Directives { pub exports: Option<Exports>, pub imports: Vec<EngineImport> }   // Some => definition block
pub struct Exports { pub items: Vec<ExportItem>, pub star: Option<Span> }
pub struct ExportItem { pub text: Rc<str>, pub quoted: bool, pub span: Span }   // quoted, in c/cpp/rust: a prototype (15.2)
pub struct EngineImport { pub items: ImportItems, pub selector: Selector, pub span: Span }
pub enum ImportItems { All(Span), Names(Vec<ImportItem>), Module(Name) }         // Module: `sel as name`
pub struct ImportItem { pub name: Rc<str>, pub quoted: bool, pub alias: Option<Name>, pub span: Span }
pub enum Selector {            // as written: @py | @py.stats | stats, "odd-name" | py.os.path, js."@scope/pkg" | v2
    Union { tag: Name }, Exact { tag: Name, label: Name }, Label { label: Name }, Module { lang: Name, path: Vec<Name> }, V2 { span: Span },
}
pub struct Extern { pub lib: Name, pub alias: Option<Name>, pub decls: Vec<ffi::Sig>, pub body_span: Span }
```
- **Selectors.** `Selector` keeps tags and path segments exactly as written. Ch. 13 canonicalizes them (`@python.stats` is `@py.stats`) and joins module paths.
- **When C declarations are parsed.** `extern` declarations are parsed at parse time (3.9). The quoted `@export` items of `c`, `cpp`, `cxx`, `rust` and `rs` blocks are parsed as prototypes by `ffi::cdecl` at parse time too, `cpp` and `cxx` ones in its C++ dialect (16.3: `std::string`, `const std::string&`); one it cannot read is a `SyntaxError` with the messages of 16.3 (4.5). An unsupported type or a missing function is still a `BuildError` when the block is reached (15.2).
### 4.4 Block bodies: lines, directives, dedent
`syntax/directives.rs` turns a `RawBody` into `body`, `loc` (2.3) and `directives`. The result is what engines receive: the code as written, with directive lines blanked and the common indentation removed (DOCS 4.1).

**Step 1: lines.**
- *One-line brace block:* the body is the text, trimmed. `body_first_line` is the header line and `body_col` is the column of the body's first char.
- *Multi-line brace block:* the body is L1 (the text after `{` on the header line) plus the middle lines. The text before the closing `}` is blank (C1) and is dropped.
  - A blank L1 is dropped too, and the body starts on the next line.
  - Otherwise L1 is trimmed and becomes body line 1, with `body_col` its column. It takes no part in dedent.
- *Fence form:* the body is the lines between the fences, and `body_first_line` is the fence line + 1.

**Step 2: directives.** A body line is a directive when, after leading spaces and tabs, it starts with `@export` or `@import` followed by a space, a tab, `{` or the end of the line. The match is on the exact token: `@exporter` and `@import_hook` are foreign code. Matching is line-based on purpose, whatever the foreign string or comment state, so an `@export` line inside a foreign string is still a directive. A line starting with `@@export` or `@@import` is the escape: it stays foreign code, minus one `@` (DOCS 6.1).
- **Directive-mode lexing.** The V2 lexer lexes from that `@` in directive mode. Spans are real file spans, and a line break ends the directive except inside `{ }` (3.3 rules 2 and 3 are off).
- **Parsing.** The parser reads one `@export` list or one `engine_import`. A list spans lines until its braces balance; if the body ends first, the error is `unterminated @export list`. The directive's last line may then hold only whitespace or a comment starting with `#` or `//`.
- **Blanking.** Every line the directive touched becomes an empty line, so foreign line numbers never shift.
- **`@export` grammar:** `"@export" "{" ( "*" | xitem { "," xitem } [ "," ] ) "}"`, with `xitem = NAME | KEYWORD | STRING`.
  - Keywords are accepted bare, because they name foreign functions.
  - Several `@export` lines merge into one list, which may combine `*` with explicit names (12.3).
  - In `c`, `cpp` and `rust` blocks (and their aliases), items name functions only, and a quoted item is a C prototype (15.2).
  - Elsewhere, a quoted item is a name that is not a V2 identifier (JS `"$helper"`).
- **`@import` inside a block** uses `engine_import` and may use the `v2` selector. Its quoted items need no `as`, because the name binds in the foreign namespace.
  - Compiled blocks allow only `{ names } from SEL`, and only together with `@export`. Anything else is a `BuildError` (15.2).
  - A run-engine block with any directive raises `ImportError` when it is reached (17.3).

**Step 3: dedent.** A line is a *code line* unless the scanner recorded that it starts inside a multi-line string, template or comment (3.8). An L1 kept from the header line takes no part.
1. Whitespace-only code lines become empty and are ignored. m is the smallest number of leading spaces and tabs over the remaining code lines.
2. Their first m chars must be identical. If they are not, the error is `block indentation mixes tabs and spaces`, at the first line that differs, with a note at the first code line (DOCS 4.1). Chars after the first m are the foreign language's business.
3. m chars are removed from every code line, and from every other line that starts with the same m chars. The remaining lines (string or comment text indented less than the code) stay exactly as written and are listed in `UnitLoc.kept`. So a Python triple-quoted string whose text starts at column 1 keeps its content, and the code around it is still dedented.

`dedent_width` is m. These are ASCII chars, so byte slicing is char-safe. A dedented line 1 has `body_col` = m + 1. The body is the lines joined with `\n`, plus a final `\n`; it is `""` when no line has text.
```text
line 10  @py stats {                      blank L1: dropped
line 11      def spread(xs):              body line 1
line 12          return max(xs) - min(xs)
line 13      @export { spread }           becomes "", exports = [spread]
line 14  }
=> body "def spread(xs):\n    return max(xs) - min(xs)\n\n", loc { body_first_line: 11, body_col: 5, dedent_width: 4, line_count: 3, kept: [] }
```
### 4.5 Static checks
- **Lexing and parsing** stop at the first error; there is no error recovery.
- **Static checks.** A file that parses gets every check below, and all violations are reported in source order (2.4). `v2 check` reports the same (19.3).
- **Kind.** Every error below is a `SyntaxError` with a span.
- **Known tags.** The only tags this pass knows are the fixed compiled tags `c`, `cpp`, `cxx`, `rust` and `rs`, which cannot be redefined (DOCS 13.3).

| Check | Message |
| ----- | ------- |
| the same label on two blocks in one file, in any languages (DOCS 4.7) | `duplicate block label 'stats'`, with a note at the first one |
| label `v2`, or selector `v2.x` | `'v2' is reserved for this file's V2 functions` |
| `@export` outside a block; `from v2` outside a block | `@export is only valid inside an embedded block` (hint: modules export every top-level name not starting with `_`) / `the v2 selector is only valid inside a block` |
| a definition block used as a value (DOCS 4.5); an operator or postfix after a block value (4.1) | `a block with @export is a definition block and cannot be used as a value`, with a note at the `@export` / `a block value must be in parentheses here` |
| args on a `c`, `cpp`, `cxx`, `rust` or `rs` block (DOCS 4.4) | `compiled blocks take no args — @export a function and call it with arguments` |
| a block arg given twice, or named like one of the block's inner `@import` aliases (12.1) | the specific reason |
| `@export {}`, a name exported twice, text after the list, an unterminated list (4.4) | the specific reason |
| `@import sel as x` where sel is not a package selector `lang.module` | `only a package selector (py.name, js.name) can be imported as a whole module` |
| a quoted V2-level import item that is not a plain name (a V2 identifier that is not a keyword, 13.4) and has no `as` | `quoted name 'from' needs an alias: "from" as NAME` |
| `a..b` outside a `for` head; `for i, x in a..b`; `for (x in xs)` | `a..b is only allowed in a for head; use range(a, b)` / `a range loop takes one variable` / `a for head takes no parentheses` |
| `break` or `continue` outside a loop (a function body starts outside any loop); `return` outside a function | `break outside a loop`, and so on |
| a name declared twice directly in one body: `let`, `const`, `func`, named `@import` items, `import` and `extern` aliases, `extern` functions, and the parameters, loop variables or catch name that the body introduces (DOCS 3.3; inner bodies may shadow) | `'x' is already declared in this block`, with a note at the first declaration |
| a duplicate parameter, a non-default parameter after a default one, `...rest` not last; a positional argument after a named one, a duplicate named argument | the specific reason |
| `try` with neither `catch` nor `finally` | `try needs a catch or a finally` |
| `a < b < c`; `not` without `in` | `comparisons cannot be chained; write a < b && b < c` / `not is only used in 'not in'; negate with !x` |
| a dict key that is a number or a keyword | `dict keys are names, strings or [expr]` |
| an invalid assignment target; a target through `?.` (4.1) | `cannot assign to this expression` / `cannot assign through ?.` |
| an `extern` declaration V2 cannot read (3.9) | the messages of 16.3 |
| a quoted `@export` prototype in a `c`, `cpp`, `cxx`, `rust` or `rs` block that V2 cannot read (4.3) | the messages of 16.3 |

These are checked later, not here:
- tags: `EngineUnknown` when the block is reached (ch. 7);
- names bound by `@import { * }`: known only at runtime, and duplicates get the runtime error of 13.4;
- compiled-block directive rules: `BuildError` (15.2);
- directives in run-engine blocks: `ImportError` (17.3).

---

## 5. Host runtime

`runtime/` is a tree-walking interpreter over the span-annotated AST (ch. 4). It runs only on the interpreter thread (256 MiB stack, 1.3); `Value` is `Rc`/`RefCell` and never crosses threads.

### 5.1 Values

```rust
pub enum Value {
    Null, Bool(bool), Int(i64), Float(f64), Str(Rc<str>), Bytes(Rc<[u8]>),
    List(List), Dict(Dict), Func(Func),        // Rc<RefCell<Vec<Value>>>, Rc<RefCell<IndexMap<Rc<str>, Value>>>,
    Handle(Rc<HandleRef>), Error(Rc<ErrorValue>),  // Rc<Callable>, each freed iteratively (below)
}
pub enum Callable {
    Closure { decl: Rc<FuncDecl>, env: Env, file: Rc<FileScope> },  // file: the declaring file (6.2)
    Builtin(&'static BuiltinDef),                                    // 5.10
    Proxy { session: SessionId, unit: Rc<str>, name: Rc<str> },      // bridged export; unit = wire unit id (13.4)
    Native { lib: LibId, sym: Rc<str>, sig: Rc<Sig> },               // compiled export (15.8) or extern (16.1)
    HandleMethod { handle: Rc<HandleRef>, method: Rc<str> },         // built only as the callee of h.m(...) (11.3)
}
pub struct ErrorValue {
    pub type_: Rc<str>, pub message: Rc<str>, pub data: Value, pub lang: Option<Rc<str>>,
    pub foreign_type: Option<Rc<str>>, pub cause: Option<Rc<ErrorValue>>,
    pub span: OnceCell<Span>, pub trace: OnceCell<Rc<[Rc<str>]>>,  // set by the first raise (5.9)
}
```

- `Value::clone` is O(1): assigning, passing, returning or storing a list or dict shares it; `str`, `bytes`, `func`, `handle` and `error` are immutable. `.copy()` is shallow: a new `Vec`/`IndexMap`, nested containers shared.
- **Freeing is iterative.** The `List`, `Dict` and `Func` handles wrap their `Rc` in a type whose `Drop` moves the contents of the last reference (a closure's frame slots, when nothing else holds the frame) into a thread-local queue that the outermost drop empties one value at a time; `ErrorValue`'s `Drop` queues its `data` the same way. So a value nested a million levels deep (`for i in 0..1000000 { v = [v] }`) never overflows the stack when it goes away.
- `type(x)` is one of `null bool int float str bytes list dict func handle error`. Every `Callable` is `"func"`, including names bound by `@import`; foreign functions, bound methods and classes *returned as values* are callable handles, type `"handle"` (11.6). Native pointers are handles in the native pseudo-session (16.6).
- `Closure.file` makes blocks and `@import`s in a function body resolve against the declaring file, whichever file calls it.

### 5.2 Equality, ordering, truthiness

`eq(a, b)` implements `==`/`!=`; `in` on lists and `index_of` reuse it.

| Operands | Rule |
| --- | --- |
| same scalar type | value equality; floats follow IEEE (`nan != nan`, `0.0 == -0.0`) |
| `Int` vs `Float` | exact: `f.fract() == 0.0 && -2^63 <= f < 2^63 && f as i64 == i` (never `i as f64 == f`, wrong beyond 2^53) |
| `Str`, `Bytes` | byte equality |
| `List`, `Dict` | `Rc::ptr_eq`, else pairwise `eq` (list) or the same key set with `eq` values (dict, order ignored) |
| `Handle` | same session and id (11.6); native pointers: same address (16.6) |
| `Func` | `Closure`/`Builtin`: `Rc::ptr_eq`; `Proxy`: same session, unit, name; `Native`: same library and symbol; `HandleMethod`: equal handles, same method |
| `Error` | `Rc::ptr_eq` |
| different types (`Int` vs `Bool` too) | `false`, never an error |

- **Depth.** `eq` and `str()` recurse; past 1,000 levels they raise `RecursionError: value nested too deeply (or cyclic)`, and `str()` prints a container already on its current path as `[...]`/`{...}`. The codec has its own limits: `json_parse` and `json_stringify` reject nesting over 1,000 levels with `ValueError` (10.1), and the 256-level container limit applies only to VEP frames and compiled reverse-call JSON (10.5).
- **Ordering** (`< > <= >=`, `sort`) exists only for number–number and str–str. Int vs float is exact: NaN compares `false`; `f >= 2^63` or `f < -2^63` decides at once; otherwise compare `i` with `f.trunc() as i64`, a tie decided by the sign of `f - f.trunc()`. Strings compare by code point (Rust `str` order). Other pairs → `TypeError: cannot compare list and int with '<'`; a handle operand gets the TypeError of 11.7.
- **Truthiness.** Falsy: `null`, `false`, `0`, `0.0`, `-0.0` and an empty str, bytes, list or dict. Everything else is truthy, including NaN, functions, errors and every handle (no round trip). `!x` and `bool(x)` use this rule.

### 5.3 Operators

| Op | int ∘ int | with a float | other operands |
| --- | --- | --- | --- |
| `+` | `checked_add` | f64 | str+str, bytes+bytes, list+list → new value; else `TypeError: unsupported operand types for +: 'str' and 'int'`, plus `— use f"...${x}" or str(x)` when one side is a str |
| `-` `*` | `checked_sub` / `checked_mul` | f64 | `*`: str or list × int, either order; count ≤ 0 → empty; size via `checked_mul`, buffer via `try_reserve_exact` (failure → `ValueError: repetition result too large`) |
| `/` | both to f64; always a float | f64 | divisor `0` or `0.0` → `ValueError: division by zero` |
| `%` | floor: `r = a.wrapping_rem(b); if r != 0 && (r < 0) != (b < 0) { r += b }` | same fix-up on f64 `%`; a zero result takes the divisor's sign (`-6.0 % 3` is `0.0`, `6.0 % -3` is `-0.0`) | divisor `0` or `0.0` → `ValueError: division by zero` |
| `**` | `e >= 0`: `checked_pow` (`e > u32::MAX` overflows unless the base is 0, 1 or -1); `e < 0`: a float | `powf` | zero base (int or float) with a negative exponent → `ValueError: division by zero` |
| `-x` | `checked_neg` | negate | `TypeError: bad operand type for unary -: 'str'` |

- Integer overflow raises `ValueError: integer overflow in '*'` (the operator). Float `+ - * **` follow IEEE and may give `inf`/`nan`.
- `x op= y` always means `x = x op y`, with the target's container and key evaluated once: `xs += [1]` makes a new list and rebinds `xs` (other names keep the old list). `&&`/`||` short-circuit and return an operand; `??` returns its left operand unless it is `null`.
- `x in y`: list → an element `eq` to `x`; dict → key lookup, `x` must be a `Str`; str → substring, `x` must be a `Str`; bytes → a `Bytes` subsequence or an `Int` byte (outside 0–255 → `false`); otherwise TypeError (`'in' needs a list, dict, str or bytes on the right, got int`; `'in' on a str needs a str, got int`). `not in` negates.
- Handles take only `==`/`!=`: `in`, slicing, arithmetic, unary `-` and ordering raise the TypeErrors of 11.7, whose hint says to do it inside a block or convert with `value(h)`.

### 5.4 Strings and bytes

A `Str` is always valid UTF-8 (lone surrogates never enter, 10.4); user-visible positions are **code-point** indexes. `Interp` keeps an 8-slot round-robin cache of `CharIndex { s: Weak<str>, ascii: bool, chars: usize, marks: Vec<usize> }` (`marks`: byte offsets of chars 0, 64, 128, …) so `s[i]`, `len(s)`, slices and `find` are not O(n). A lookup compares `Weak::as_ptr` with `Rc::as_ptr` (the `Weak` keeps the allocation, so its address cannot be reused). Char → byte is `i` when `ascii`, else `marks[i / 64]` plus `i % 64` steps; byte → char binary-searches `marks`. Strings under 64 bytes and `for ch in s` use no index.

- `s[i]` is a one-char `Str`; negative `i` adds the length; out of range → `IndexError: string index 9 out of range (length 5)` (lists and bytes use the same wording).
- `x[a:b]` on str, bytes and list follows Python: a missing bound is 0 or the length, a negative bound adds the length, both clamp to `[0, len]`, `a >= b` gives an empty value, the result is new (shallow for lists). No step, no slice assignment; slicing anything else is a TypeError.
- f-strings: `${e}` inserts `str(e)`; `${e:.Nf}` needs an int or float (else TypeError) and is `format!("{:.N$}", x as f64)`, rounding the exact binary value (`${19.999:.2f}` → `20.00`); NaN prints `nan`, not Rust's `NaN`.
- `Bytes` supports `len(b)`, `b[i]` (an `Int` 0–255), slices, `+`, `==` and `in`, but no ordering. `b.decode()` is strict UTF-8 (else `ValueError: invalid UTF-8 at byte offset N`); `b.hex()` is lowercase without separators.

### 5.5 Members, indexing and optional chains

| Receiver | `r.name` | `r.name(args)` | `r[k]` | `r.name = v` / `r[k] = v` |
| --- | --- | --- | --- | --- |
| dict | key; missing → `KeyError: key 'name' not found` + did-you-mean over the keys | key `name` holding a `Func` → call it; otherwise the dict method | `Str` key (else `TypeError: dict keys must be str, got int`); missing → KeyError | insert or replace (a replaced key keeps its position) |
| list | `TypeError: list has no field 'len'` | list method | `Int` (else `TypeError: list index must be an int, got float`), negative from the end; IndexError | index assignment; IndexError, never grows; `r.name = v` is the field TypeError |
| str, bytes | `TypeError: str has no field 'x'` | method (5.11) | code point / byte (`string index …`, `bytes index …`) | `TypeError: str is immutable` |
| handle | `op getattr` | `call {h, method}` | `op getitem` | `op setattr` / `op setitem` (11.3; native pointers: TypeError, 16.6) |
| error | field (5.9) | `TypeError: error has no method 'x'` | `TypeError: cannot index error` | `TypeError: error values are read-only` |
| null | `TypeError: cannot read 'name' of null (use ?.)` | same | `TypeError: cannot index null` | `TypeError: cannot set 'name' of null` / `TypeError: cannot index null` |
| other | `TypeError: int has no field 'x'` | `TypeError: int has no method 'x'` | `TypeError: cannot index int` | `TypeError: int has no field 'x'` / `TypeError: cannot index int` |

- A dict dispatches `d.name(args)` on its key first **only when that key holds a `Func`**: module namespaces (6.4), `extern … as ns` dicts (16.1) and dicts of lambdas work like objects (`ns.keys(…)` calls a C function named `keys`), while JSON data with a `"values"` key keeps the dict methods. A float index (even `1.0`) or bool index is a TypeError; a computed literal key `[e]: v` must be a `Str`.
- **Optional chains.** The parser wraps a postfix chain containing `?.` in `OptChain` (4.1). A `?.` step whose receiver is `null` ends the **whole chain** at once with `null`, as in JS: `a?.b.c()` with a null `a` evaluates neither `.b`, `.c` nor the call's arguments. Parentheses end the chain, so `(a?.b).c` fails when `a` is null. Only a null receiver of `?.` is guarded: `a?.b.c` with a non-null `a` and a null `b` fails at `.c`, and a missing dict key is still a KeyError. Assigning through `?.` is a SyntaxError (4.1). `eval_chain` returns `Ok(None)` for the short-circuit, and only `OptChain` turns it into `null`.

### 5.6 Environments and declarations

`Env = Rc<RefCell<Frame>>`; `Frame { vars: IndexMap<Rc<str>, Slot { value, is_const: bool }>, parent: Option<Env>, closures: u32 }`.

- **Frames:** builtins frame (shared, read-only: assigning to a builtin's name is `TypeError: cannot assign to builtin 'print' — shadow it with let`) ← one module frame per file ← call frames ← block frames. Each `if`/`elif`/`else` arm and `try`/`catch`/`finally` body gets a new frame; loops get **one fresh frame per iteration**, holding a `for`'s variables, in which the body runs directly; a call frame holds the parameters and is the body's top-level frame; a catch body's frame holds the catch name. Parameters, loop variables and the catch name are therefore declared in the body they introduce (4.5); inner blocks may shadow. Lookup walks `parent`; a `RefCell` borrow is held only while one slot is read or written.
- **Declarations:** `let x = e` evaluates `e`, then declares `x` (`let x = x + 1` in an inner block reads the outer `x`); `let x` binds `null`. `const`, `@import` names (13.4) and `extern` names (16.1) are const slots: assigning → `TypeError: cannot assign to constant 'LIMIT'` (shallow: a const list can still `push`). A name declared twice in one body is a static SyntaxError (4.5). Only `@import { * }` declares names the parser cannot see; there `Frame::declare` raises `NameError: 'x' is already declared in this block`. An undeclared name → `NameError: 'x' is not declared` + did-you-mean (an assignment adds `— declare it with let`).
- **Hoisting:** entering a body turns each `func` statement directly inside it into a `Closure` over that frame before the first statement runs; reaching the statement later does nothing. **Closures** capture the `Env`, so `lambda() => i` made in a loop keeps its iteration's `i`.
- **Cycle trimming:** a closure stored in the frame it captures (every local `func`, `let f = lambda …`) is an `Rc` cycle, counted in `Frame.closures`. When such a frame goes out of scope, let `k` = its slots holding a `Closure` over it with `Rc::strong_count == 1`; if `Rc::strong_count(&frame) == 1 + k`, nothing else reaches it and its `vars` are cleared (releasing held handles). Cycles through containers are not trimmed.

### 5.7 Calls

The callee is evaluated first, then the arguments left to right; for `r.name(args)` the callee is resolved first too (the `Func` under a dict key, or the method, or the null-receiver / unknown-method TypeError of 5.5), so an argument that changes `r` can't change what is called; `...xs` must be a list (`TypeError: spread argument must be a list, got dict`) and expands in place. `Closure` and `Builtin` (a `&'static [ParamSpec]`) share one binder:

1. Positional args fill the parameters before the rest parameter; extras go to the rest list, or without one → `ArityError: greet() takes 2 positional arguments but 3 were given`.
2. Named args match by name: unknown → `ArityError: greet() got an unexpected argument 'greting' — did you mean 'greeting'?`; already bound → `… got multiple values for argument 'name'`; the rest parameter → `… cannot take rest parameter 'nums' by name`.
3. Unfilled parameters without a default → `ArityError: greet() missing required argument 'name'` (all listed), checked before any default runs.
4. Push the `CallRecord` (5.8), check the depth (5.9), create the call frame (parent `Closure.env`) and walk the parameters in order: declare the bound value or the rest list (possibly empty), else evaluate the default **in the call frame** (it sees earlier parameters; re-evaluated per call). Parameters after the rest parameter are named-only (builtins only, such as `print`'s `sep` and `end`; in V2 code `...rest` must be last, 4.5).
5. Hoist the body's `func`s and run it; falling off the end returns `null`.

- `Proxy`, handle calls `h(…)` and `HandleMethod` forward positional and named args as given (`call`, 9.5). Named args to a session without the `kwargs` cap (js, ts; custom engines per 17.5) → `ArityError: {tag} functions take positional arguments only (got '{kw}')`, raised before sending (10.8, 17.5).
- `Native` (compiled exports, `extern`) takes positional args only (`ArityError: {name}() takes positional arguments only`, D63), counted against the C signature (16.5), and runs in-process (16.7).
- A non-function → `TypeError: 'int' is not callable`. A lambda is named `lambda@main.v2:12` in messages, `str()` and its trace frame (`lambda@main.v2:12 (main.v2:14)`, 2.5).
- V2 functions called from foreign code (`fn` and `route` calls, 11.8, 13.6; compiled `host_call`, 15.9) go through the same binder on the interpreter thread, inside the host's wait for that engine (9.7). They can arrive only while V2 has a request outstanding to that engine (9.8); a stored callback stays valid for later requests.

### 5.8 Control flow and frame discipline

Statements return `Exec<()>`, expressions `Exec<Value>`: `type Exec<T> = Result<T, Flow>`, `enum Flow { Error(Rc<ErrorValue>), Return(Value), Break, Continue, Exit(i32) }`. `exec_block(body, env, file)` and `eval(e, env, file)` take the environment as an **argument**, never interpreter state: a child frame is a local `Env` dropped on every exit path, including `?`.

- The only interpreter-level stack is `calls: Vec<CallRecord { name: Option<Rc<str>>, call_site: Span }>`. Calls of non-builtin callables and module imports push a record and pop it with the result held in a local (`let r = self.run_call(..); self.calls.pop(); r`), never with a `?` in between.
- A call turns `Return(v)` into `Ok(v)`; `break`/`continue` never cross a function (static, 4.5). Loops: `Ok`/`Continue` → next iteration, `Break` → exit, anything else propagates.
- `try`: the body runs in a new frame; on `Flow::Error(e)` the `catch` body runs in a new frame with `e` bound; `Return`/`Break`/`Continue` are never caught. `finally` runs last for every flow except `Exit`: if it completes normally the pending outcome (value, flow, or a new error from `catch`) continues, else its own flow replaces it. Every error kind is catchable, `RecursionError` and engine errors included.
- `Exit(code)` (from `exit()`, 5.10) is caught by nothing and skips every `finally`; it unwinds through foreign frames (9.7) and native frames (15.9) to the top, where 6.5 ends the run. Interrupts never become a `Flow` (6.5).
- **Iteration** evaluates the iterable once: list → a snapshot of its elements; dict → a snapshot of its `(key, value)` pairs (one variable gets the key); str → chars; bytes → `Int`s; `a..b` → a lazy `i64` counter (both ends `Int`, else TypeError; `b <= a` → no iterations); handle → `op iter`/`next` (11.4). Two variables get `(index, item)`, or `(key, value)` for a dict. Otherwise `TypeError: cannot iterate over int`.

> **Regression (old `interpreter.rs:4956-4971`).** The old call ran `push_scope_with_parent(…); exec_block_no_scope(&body)?; set_scope(saved)`; an error skipped the restore, so the caller's `catch` ran in the callee's scope and `func g() { let local = 7; try { f() } catch (e) { }; return local }` failed with `Undefined variable 'local'` (for-in bodies leaked the same way). Required tests (R09): `g()` returns 7; the same in a `for` body and a lambda; `return`/`break` out of `try … finally`; a V2 error thrown in a callback crosses a Python frame and is caught with locals intact.

### 5.9 Errors, traces and limits

- **Kinds.** Host: `Error TypeError ValueError KeyError IndexError NameError ArityError IOError RecursionError ImportError`. Engines and crossings (ch. 21): `EngineUnknown EngineNotFound EngineNameConflict EngineCrashed EngineTimeout EngineExitError MarshalError BuildError ForeignError ProtocolError FFILoadError FFISymbolError FFIValueError`. These 23 are catchable. `SyntaxError` is a diagnostic kind (2.4): it is reported before its file runs and never reaches a `catch` in that file; an importer sees it as `ImportError` (6.4). `error(type, …)` accepts any non-empty type string.
- **Raising.** `throw v` throws an `Error` as is, turns a `Str` into `error("Error", v)`, else `TypeError: throw expects an error or a str, got int`. `span` and `trace` are filled when the error is first raised: by `throw` (the `throw` keyword), by a failing node (operator, call, index, member; spans per 2.2), or on surfacing from an engine (the mapped foreign location, 12.6). `error()` leaves both empty; a rethrow keeps them (`OnceCell`), so `==` still holds and the uncaught header points at the first `throw`.
- **Trace** (innermost first): `loc` = raise span; for each `CallRecord` from the top, emit `(record.name, loc)`, then `loc = record.call_site`; finally `(None, loc)`. A proxy call's record emits the reply's mapped foreign frames (12.6) instead. Frame strings are 2.5; rendering and elision of long traces are 2.4 (`e.trace` keeps every frame).
- **Fields:** `type message file line col trace data lang foreign_type`; any other → `TypeError: error has no field 'x'` + did-you-mean. `file`/`line`/`col` are `null` until raised (`file` is the display path, 2.1); `trace` is a new list of strings on each read. `cause` is not a field; an uncaught error prints it as `caused by:` (2.4).
- **RecursionError:** a call that takes `calls.len()` past **10,000** → `RecursionError: maximum call depth (10000) exceeded` at its span. A stack guard also covers deep expressions and nested foreign callbacks: each call and each `eval` of an expression node compares a local's address (`std::hint::black_box`) with the one recorded at thread start and raises the same error past 7/8 of the stack size. Cross-language nesting has its own limit of 64 (9.7).
- **Did-you-mean** (names, keys, fields, methods, named args; tags use 7.3): optimal string alignment distance on lowercased names, accepted when `d <= max(1, min(3, (len + 2) / 3))`; the smallest wins, ties → innermost frame, then declaration order.

### 5.10 Builtins

`BuiltinDef { name, params: &'static [ParamSpec], f: fn(&mut Interp, Args, Span) -> Exec<Value> }` entries live in the builtins frame, so user names can shadow them. Notes beyond DOCS 3.11:

| Builtin | Notes |
| --- | --- |
| `print` | `str()` of each value joined by `sep`, then `end` (both `Str`, else TypeError); one write to `Out`. |
| `input` | Writes `str(prompt)`, flushes (see Output), reads one line from the host's stdin and strips `\n` and one `\r` before it. End of input before any byte → `null`; a last line without `\n` is returned; invalid UTF-8 → IOError. |
| `len`, `type`, `bool` | `len`: chars (5.4), bytes, list items, dict entries, a handle's `op len` (11.3), else `TypeError: len() needs a str, bytes, list or dict, got int`. `type`: 5.1. `bool`: 5.2 truthiness (no round trip). |
| `str` | A `Str` is returned unchanged; others get the canonical form: `null`, `true`, `42`; floats by Rust `{:?}` (`2.0`, `0.1`, `1e20`) plus `nan`, `inf`, `-inf`; strings inside containers double-quoted with `\" \\ \n \r \t` and `\u{..}` for other control chars (`[1, "a"]`, `{"k": 1}`); bytes as `b"\x00A\xff"` (printable ASCII as is, `\" \\ \n \r \t`, other bytes as lowercase `\xNN`); `<func greet>`, `<func lambda@main.v2:3>`, `<func spread (py)>`, `<func fast_sum (rust)>`, `<func cos (extern)>`, `<builtin print>`; errors `ValueError: msg` or `ForeignError: ZeroDivisionError: msg`; handles via `op str` (11.3; a dead session raises EngineCrashed, 8.2, 11.7); native pointers `<native sqlite3* 0x…>` (16.6). |
| `int`, `float` | `int`: a float truncates toward zero (NaN, inf, out of range → `ValueError: cannot convert nan to int`); a str must be `[+-]?[0-9]+` after trimming (else `ValueError: invalid int: 'abc'`; beyond i64 → `ValueError: int out of range: '…'`); bool → 0/1. `float`: int → nearest f64, bool → 0.0/1.0, a trimmed str via `f64::from_str` (also `nan`, `inf`, `infinity`, any case), failure → ValueError. Other types → `TypeError: int() can't convert list`. |
| `range` | Parameters `start = null`, `end = null`, `step = 1`, all `Int`s; one of `start` and `end` alone is the end, so `range(a)` = `range(end: a)` = `range(0, a)` (neither: `ArityError: range() missing required argument 'end'`); the length is computed first with checked arithmetic and the list reserved with `try_reserve_exact` (failure → `ValueError: range too large`); `step == 0` → `ValueError: range() step must not be zero`. |
| `json_parse` | The codec in plain mode (10.1) straight into `Value`: integer tokens that fit i64 → `Int`; integer tokens outside i64 → `ValueError: invalid JSON at line L column C: integer does not fit in 64 bits`, raised by the 10.1 pre-parse byte scan, since serde_json would already have made them `f64` (never the nearest `Float`, 0.1; 10.2); tokens with `.`, `e` or `E` → `Float` (`1.0` stays a float); duplicate keys: the last value wins at the first key's position; `$v2` keys are plain data. Syntax errors, lone `\uD800` escapes and nesting past 1,000 levels (10.1) → `ValueError: invalid JSON at line L column C: …`. |
| `json_stringify` | null, bool, int, finite float (keeps `.0`), str, list, dict. bytes, func, handle, error and non-finite floats → TypeError naming the path (`TypeError: cannot convert handle to JSON at [2]["k"] (use value(h) first)`, 10.1); a cycle → `ValueError: cannot convert a cyclic list to JSON`; nesting past 1,000 levels → ValueError (10.1). `indent: null` → compact (`{"a":1}`); an int ≥ 0 → pretty with that many spaces, one item per line, `": "` after keys, empty containers as `[]`/`{}` (as in Python and JS); a negative int → ValueError. |
| `read_file`, `write_file` | Relative paths resolve against the process working directory (unlike `import` and `extern`, which use the `.v2` file's folder). Text reads are strict UTF-8, BOM kept, no newline translation (invalid → `IOError: 'x' is not valid UTF-8 (use binary: true)`). `write_file` takes `Str` (UTF-8) or `Bytes` (raw), else TypeError, via `fs::write` (creates or truncates, no parent folders). OS failure → `IOError: cannot read 'x': <os message>` (or `write`). |
| `getenv`, `args` | `var_os`/`args_os`, decoded lossily. A name that is empty or contains `=` or NUL → `ValueError: invalid environment variable name: 'A=B'` (std may panic on it). `args()` returns a new list each call. |
| `exit` | `code`: an `Int` in i32 range (else TypeError / `ValueError: exit code 1099511627776 is out of range`; POSIX keeps the low 8 bits). Returns `Flow::Exit(code)` (5.8): no `catch` or `finally` runs, engines are still shut down and the run dir removed (6.5). |
| `error`, `value` | `error`: `type` (non-empty) and `message` must be `Str`, `data` anything; no location until raised. `value`: a handle → `op value` (11.5; native pointer → MarshalError, 16.6); anything else is returned unchanged (containers are not walked). |
| `engine_available` | 7.3: canonicalizes the tag, resolves it (may run cached probes, 7.7), never starts a session or compiles; an unknown tag → `false`; a later crash does not change the answer. `register_engine`: 17.2. |

**Output.** `Out` (`BufWriter<StdoutLock<'static>>`, 64 KiB, owned by `Interp`) is the only writer of the host's stdout: `print` and the stdout text of worker `out` frames (9.9). Worker `out` frames carry language-level output only; fd-level output in workers goes to stderr (9.9). `Out` is flushed after every write when stdout is a terminal (`IsTerminal`, checked once), and before every in-process native call (16.7), every frame written to a worker (9.9), every child spawn (a statement-position compiled or `run` block inherits stdout, 6.3), every `input()` read, every write to stderr (diagnostics, `out` stderr text), and at the end of the run; dirty native stdio is flushed before every write to `Out` (16.7). A failed write (closed pipe; Rust ignores SIGPIPE, so it is `EPIPE`) raises IOError at the `print`. Stdin belongs to `input()` alone: a worker's stdin is its VEP channel, and compiled and `run` processes get the null device.

### 5.11 Methods

Arguments bind as for builtins, and messages name the method with its type: `ArityError: list.get() takes 2 positional arguments but 3 were given`, `TypeError: str.split() argument 'sep' must be a str, got int`. `push`, `insert`, `sort`, `reverse` return `null`; `pop`, `remove_at` and dict `remove` return the removed item. Unknown method → `TypeError: list has no method 'append'` + did-you-mean. Methods are not values: `xs.len` without a call is the field TypeError of 5.5.

| Type | Methods |
| --- | --- |
| str | `len()`; `upper()`, `lower()` (full Unicode mapping); `trim()` (`char::is_whitespace`); `split(sep)` (non-empty `Str`, else `ValueError: str.split() separator must not be empty`; keeps empty parts); `lines()` (Rust `str::lines`: `\n` or `\r\n`, no trailing empty line); `replace(old, new)` (all matches, left to right; empty `old` → `ValueError: str.replace() argument 'old' must not be empty`); `starts_with(p)`, `ends_with(p)`; `find(sub)` (code-point index or `-1`); `encode()` |
| bytes | `decode()`, `hex()` (5.4) |
| list | `push(x)`; `pop()` (empty → `IndexError: pop from empty list`); `get(i, default = null)` (`Int`, negative from the end; out of range → `default`); `insert(i, x)` (negative `i` adds the length, then clamps to `[0, len]`); `remove_at(i)` (negative from the end; IndexError); `sort()`; `reverse()`; `map(f)`, `filter(f)` (over a snapshot; `f` gets exactly one argument; new list); `join(sep)` (`str()` of each item); `index_of(x)` (first `eq`, or `-1`); `len()`; `copy()` (shallow) |
| dict | `keys()`, `values()` (new lists, insertion order); `get(k, default = null)`; `remove(k)` (`shift_remove`, the rest keeps its order; missing → KeyError); `len()`; `copy()` (shallow). `get`/`remove` keys must be `Str` (TypeError). |

`sort()` is stable and in place; it first checks that the elements are all numbers or all strings (else `TypeError: cannot sort a list mixing int and str`, or `… containing list (only numbers or strings)`); numbers use 5.2's exact order with NaN last, so the comparator is total and runs no user code. **Borrow rule:** no `RefCell` borrow of a list, dict or frame is held across user code or a session round trip (a callback may mutate the same list; `op str` for `str(h)` may call back into V2), so `map`, `filter`, `join` and `str()` of containers clone the element values out first.

---

## 6. Program execution and modules

### 6.1 Startup and the run

1. **Startup** (1.1, 1.3): parse argv (19.1) and load the config (ch. 18); a usage or config error, or a missing or unreadable script, prints a message and exits 2. On Windows, `SetDllDirectoryW(L"")` runs first, so the cwd never joins the DLL search (16.2, 20.2). The interpreter thread runs steps 2 and 3.
2. **Syntax** (1.1 step 2): the entry file and every file its `import "…"` literals name are lexed, parsed and checked (ch. 3, 4; the parser stops at a file's first error, 4.5). A `SyntaxError` in the **entry** file prints as `file:line:col: SyntaxError: msg` with an excerpt (2.4) and exits 1 before anything runs or any engine is resolved. A diagnostic in an **imported** file is stored with that file and raised as `ImportError` when an `import` of it runs (6.4). Tags are not checked here: `register_engine` adds them at runtime, so an unknown tag raises `EngineUnknown` when its block is reached (7.3). `v2 check` stops after this step and prints its inventory (19.3).
3. **Run:** build the entry file's `FileScope` (6.2), hoist its top-level `func`s, execute its statements in order, then end the run (6.5).

### 6.2 FileScope and interpreter state

```rust
pub struct FileScope {
    file: FileId, path: PathBuf,              // canonical path, the dedupe key (2.1)
    frame: Env,                               // module frame; parent: the builtins frame
    blocks: Rc<[BlockInfo]>,                  // static block list: labels, tags, declared exports (12.1)
    exports: RefCell<ExportRegistry>,         // exports of loaded definition units (13.2)
}
pub struct Interp {
    calls: Vec<CallRecord>,                                      // 5.8
    units: HashMap<UnitId, UnitState>,                           // definition units (12.1)
    modules: HashMap<PathBuf, ModuleState>, import_stack: Vec<PathBuf>,   // 6.4
    routes: Vec<Route>, sessions: SessionTable, registry: Registry,       // 13.6, ch. 8, ch. 7
    out: Out, chars: [Option<CharIndex>; 8],                     // 5.10, 5.4
}
```

Reaching a block runs `exec_block` (12.1); the rules that shape a program:

| Block | When reached |
| --- | --- |
| definition (has `@export`) | First reach: resolve the tag, evaluate the block args, load (bridged: `load`, 12.2; compiled: ch. 15), register the exports, `Loaded`. Later reaches do nothing and do **not** evaluate the args again. A failed load is `Failed(err)`: every later reach re-raises the same stored error (no retry, no repeated side effects), and `@import` from it gives 13.3's `block 'stats' failed to load: …`. Reached again during its own load → 12.1's `still loading` ImportError. |
| action (no `@export`) | Every reach: resolve the tag, evaluate the args left to right, run in a fresh namespace (bridged: a new unit dropped after its reply; compiled: build or reuse the program, 15.5; `run`: 17.3). |

- "Reached" means control executes the statement: inside a function, on the first call that runs it; in a loop, on the first iteration; in an `if` arm never taken, never. Inside `try` a load error is catchable, so `try { @py probe { … @export { … } } } catch (e) { … }` is a probe, but reaching it again re-raises the stored error.
- Messages name a block by its label, or `@py block at main.v2:12` when unlabeled (12.1 `block_name`).
- A V2-level `@import` runs when reached: it resolves its selector against `exports` at that moment (13.3) and declares the `Proxy`/`Native` values or module handle in the current frame like `const` (13.4). It must come after the block has run, else `ImportError: block 'stats' has not run yet — move this @import below it`. `@import` inside a block resolves lazily at first call (13.7), so blocks may import from each other in any order; the `v2` selector looks the file's top-level name up at every call (13.6).
- Blocks, labels and selectors belong to one `FileScope`: a block in `util.v2` never makes `@import … from @py` in `main.v2` ambiguous, and `from v2` inside `util.v2` means `util.v2`.

### 6.3 Blocks as statements and expressions

An action block is an expression; as a statement its value is discarded, and the engine is told so. Definition blocks are statements only (4.5).

| Engine kind | Expression position | Statement position |
| --- | --- | --- |
| py, js, ts, custom bridged | py: the last statement's value if it is an expression statement, else `null`; js, ts: the script's completion value (what `eval` would return; `undefined` → `null`; DOCS 4.5, 12.2); custom bridged: whatever the adapter returns for the action (17.6 item 5). Each is awaited if awaitable (9.8), marshaled per ch. 10 | `want_value: false`: the value is computed (and awaited) but never encoded, so no handle is created (12.2) |
| compiled action program, custom `run` | stdout piped and collected; one trailing `\n` or `\r\n` stripped; strict UTF-8, else MarshalError (17.3) | the host flushes `Out` and dirty native stdio, and the child inherits stdout, so its output streams in program order |

In both positions: stdin is the null device; stderr streams live and its last 4 KiB are kept; a non-zero exit raises `EngineExitError` whose `data.stderr` holds that tail, not repeated in the message (17.3); the block's whole process tree is killed when the block finishes or times out (`call_timeout`, 8.5), so nothing it started outlives it. Blocks inside larger expressions keep left-to-right order: in `f(@py { 1 }, g())` the block runs before `g`.

### 6.4 V2 modules

`import "PATH" as name` may appear in any body and declares `name` in the current frame:

1. **Resolve** `PATH` as written (no extension added) against the **importing file's** folder (absolute paths as is), then `fs::canonicalize`. Missing → `ImportError: cannot find module './util.v2' (looked for C:\proj\util.v2)` (paths shown per 2.1, with `..` resolved and the existing part canonicalized). A Windows drive-relative path (`C:m.v2`) is neither relative to the file nor absolute: `ImportError: cannot find module 'C:m.v2' (a drive-relative path: write it relative to the file, or absolute)`. A file that did not exist at startup but exists now is parsed now.
2. **Look up** `modules`, whose `ModuleState` is `Running | Done(Value) | Failed(Rc<ErrorValue>)` (the entry file starts `Running`):
   - `Done(ns)` → declare `name = ns`. `Failed(e)` → throw `e` again: a module's top level never runs twice.
   - `Running` → `ImportError: circular import: a.v2 -> b.v2 -> a.v2` (the chain from the path's first entry in `import_stack`).
   - Absent, with a stored diagnostic (6.1) → `Failed` with `ImportError: cannot import './util.v2': util.v2:3:9: SyntaxError: <message>` at the `import`, `data` = the list of diagnostics as `{file, line, col, message}`.
   - Absent otherwise → check the call depth and the stack guard as for a call (5.9), push onto `import_stack`, mark `Running`, push a nameless `CallRecord` for the import site, build a new `FileScope` whose module frame's parent is the builtins frame, hoist and run it.
3. **Success:** the namespace is a new `Dict` (type `"dict"`) of the module frame's bindings in declaration order, minus names starting with `_`: values, `func`s, `@import`ed proxies and module handles, `extern` functions and `extern … as` dicts, imported module dicts. It is stored as `Done(ns)` and shared by every importer. It is a snapshot: rebinding a module-level `let` later is not visible through it. `util.slug(…)` works by dict-key dispatch (5.5).
4. **Failure:** `Failed(err)`; the error propagates unchanged with the import site in its trace.

Re-exported proxies keep their `session` and `unit` and work from any file. Modules share the entry file's project root and config (18.1).

### 6.5 Ending the run: exit codes and shutdown

| Code | When |
| --- | --- |
| 0 | the entry file ran to its end |
| n | `exit(n)` (i32; POSIX keeps the low 8 bits) |
| 1 | an uncaught error (printed per 2.4), or a `SyntaxError` in the entry file (6.1) |
| 2 | a usage or configuration error, before any V2 code runs (18.5, 19.1) |
| 3 | an uncaught `EngineNotFound` |
| 130 | an interrupt: Ctrl+C, Ctrl+Break, console close, SIGINT, SIGTERM or SIGHUP; always 130, never 128+signo |
| 101 | an internal panic (a V2 bug) |

`v2 check` and `v2 doctor` codes: 19.1.

**End of run.** A normal end, an uncaught error and `exit(code)` take one path (1.1 step 4); `exit` is `Flow::Exit`, which unwinds to the top without running any `catch` or `finally` (5.8), abandoning pending foreign calls (9.7) and native frames (15.9). The interpreter thread then:
1. Flushes dirty native stdio and `Out`; an uncaught error is written to stderr (2.4).
2. Shuts the sessions down (8.7): `bye`, one shared 1 s grace period that keeps forwarding `out` frames, then `kill_tree` on survivors.
3. Kills and reaps, as trees, any child still running: `run` engines, compiled programs, compilers, transpilers (8.3).
4. Flushes `Out` again (late `out` frames) and returns the code to main.

Main removes the run dir (20.3; on Windows up to 3 retries 50 ms apart while killed processes release their locks, then errors are ignored) and exits with the code. Native libraries are never unloaded (16.2).

**Interrupts** are handled by the host alone (1.3). Unix: every child starts in its own session (`setsid()`, 8.3) with no controlling terminal, so terminal SIGINT and SIGHUP never reach it. Windows: `CREATE_NEW_PROCESS_GROUP` keeps only Ctrl+C from children; Ctrl+Break and console close reach every process on the console, and adapters ignore SIGBREAK (14.3, 14.4). The interrupt handler only sets `INTERRUPTED` (an `AtomicBool`). Unix: a `sigaction` handler for SIGINT, SIGTERM and SIGHUP that does nothing else (no lock, no Condvar, no allocation: only async-signal-safe work); a signal already ignored when v2 starts (`nohup`, background jobs of a shell without job control) stays ignored. If the watchdog thread cannot be started, no handler is installed, so the OS default still ends the process. Windows: the `SetConsoleCtrlHandler` routine runs on its own thread; it sets `INTERRUPTED`, notifies the watchdog's Condvar (8.5), then blocks until the process ends, because returning early would let `CTRL_CLOSE_EVENT` terminate the process before cleanup. The watchdog (1.3) waits on its Condvar with a timeout of at most 100 ms and checks `INTERRUPTED` on every wake. Once it is set, the watchdog `kill_tree`s every registered child, workers included (8.7), removes `<run>` (best effort) and ends the process with `_exit(130)` / `ExitProcess(130)`: no `bye`, `catch` or `finally`. The interpreter thread never handles an interrupt: once `INTERRUPTED` is set it prints no diagnostic (neither the EOF of a killed worker as `EngineCrashed` nor the failure of a per-use child ended by the same event), spawns no process and runs no `catch` or `finally`; where it would, it stops (`session::stop_if_interrupted`) and the watchdog ends the process. A run that ends between the interrupt and the watchdog's next wake is caught by main, which checks `INTERRUPTED` after joining the interpreter thread and takes the same path.

**Closed streams.** Diagnostics, check's summary, `--help` and `--version` are written with writes whose errors are ignored, so a closed stdout or stderr never turns an exit path into a panic or an abort: the exit code is the one the run would have had. Program output is different: a failed write is 5.10's `IOError` at the `print`, and when `Out`'s final flush (step 1 above) fails, `v2: cannot write to stdout: <os message>` goes to stderr and an exit code of 0 becomes 1 (any other code is kept), so lost output never looks like success.

**Panics.** A panic in the interpreter thread prints `v2: internal error: <panic message>`; unwinding drops the sessions (killing their workers), main removes the run dir and exits 101. Host functions called from native code wrap their bodies in `catch_unwind` (1.3, 15.9). If the host dies with no path at all (SIGKILL, a native crash), Windows' kill-on-close Job Objects, Linux's `PR_SET_PDEATHSIG` and adapters' exit on stdin EOF still end every child (8.3); the run dir is left behind, owner-only (20.3).

---

## 7. Engine registry and resolution

Code: `engines/spec.rs` (specs, the built-in table), `engines/registry.rs` (tags, `register_engine`), `engines/resolve.rs` (discovery, probes, `toolchains.json`).

### 7.1 Data model

```rust
pub enum EngineKind { Bridged, Compiled, Run }   pub enum Profile { Python, CLike(Dialect), Plain }   // 3.8
pub enum Family { Python, Node, Gcc, Clang, Msvc, Rustc, Custom }
pub struct EngineSpec {                                    // py: "py", "Python", ["python"], Bridged, builtin, Python
    pub id: Rc<str>, pub language: Rc<str>, pub aliases: Vec<Rc<str>>, pub kind: EngineKind, pub builtin: bool,
    pub profile: Profile, pub session_of: Option<Rc<str>>, // Some("js") for ts: no process of its own
    pub candidates: Vec<Vec<Rc<str>>>, pub fixed_args: Vec<Rc<str>>,  // argv prefixes tried in order; appended args ({adapter})
    pub env: Vec<(Rc<str>, Rc<str>)>, pub min_version: Option<Version>,
    pub adapter: Option<(&'static str, &'static str)>,     // (file name, include_str! source), 14.1
    pub custom: Option<Rc<CustomFields>>,                  // custom engines: 17.1's validated fields
}
pub struct ResolvedEngine {
    pub spec: Rc<EngineSpec>, pub from: ConfigSource,      // 18.3; doctor FROM column
    pub argv: Vec<OsString>, pub shown: OsString,          // [C:\Windows\py.exe, -3]; shown: doctor COMMAND (rust: 7.6)
    pub version: Option<Version>, pub family: Family,      // None: custom engine, or "not probed" (v2 check)
    pub target: Option<String>, pub env: Vec<(OsString, OsString)>,  // triple, or cl x64|x86|arm64; spec env (+ vcvars env)
    pub skipped: Vec<(PathBuf, String)>,                   // "Microsoft Store placeholder", "too old: 3.6.8 < 3.8"
}
```

These are the finished shapes; the code grows into them phase by phase, adding fields with the code that reads them. Phase 1's `EngineSpec` has `id`, `language`, `aliases`, `session_of`, `candidates`, `env`, `min_version` and `probe` (args and version pattern); the lexer's scanning profiles stay in `syntax/scan`'s static table (3.8), and a test keeps the two tables in step. Its `ResolvedEngine` has `spec`, `origin` (`Path`, `Venv` or the config source: doctor's FROM), `argv`, `version`, `skipped` and `ts` (`Strip` or `Transpiler(argv)`).

`Registry` holds the specs (built-ins, then config-declared custom engines in declaration order, then `register_engine` additions), `by_tag` (id or alias → id) and `resolved: RefCell<HashMap<Rc<str>, Result<Rc<ResolvedEngine>, Rc<ErrorValue>>>>`. It is built from the merged config (ch. 18) at startup; the lexer never consults it (3.8). Resolution is lazy (the first block, selector, `engine_available` or doctor that needs it) and memoized per id, failures included; unknown tags are not memoized, so a later `register_engine` can still add them.

### 7.2 Built-in engines

| id | aliases | kind / profile | candidates, in order | fixed args | env | minimum | probe |
|---|---|---|---|---|---|---|---|
| `py` | `python` | Bridged / Python | `python3`, `python`, Windows: `py -3` | `-B -u {adapter}` (`-B`: no `__pycache__` in the project; `-u`: unbuffered) | `PYTHONUTF8=1`, `PYTHONIOENCODING=utf-8` | 3.8 | `--version`: `Python (\d+)\.(\d+)\.(\d+)` |
| `js` | `javascript` | Bridged / CLike(Js) | `node` | `{adapter}` | `NODE_PATH` (8.3) | 18.0 | `--version`: `^v(\d+)\.(\d+)\.(\d+)` |
| `ts` | `typescript` | Bridged / CLike(Js), session `js` | (js) | | | Node 22.13+ on 22.x, or 23.2+, for stripping (7.6) | js's probe |
| `c` | | Compiled / CLike(C) | Windows: `cl`, `gcc`, `clang`; else `cc`, `gcc`, `clang` (D35) | | | gcc 7, clang 5 (full C++17), cl 19.28 (VS 2019 16.8, first with `/std:c11`) | 7.7 |
| `cpp` | `cxx` | Compiled / CLike(Cpp) | Windows: `cl`, `g++`, `clang++`; else `c++`, `g++`, `clang++` | | | as `c` | 7.7 |
| `rust` | `rs` | Compiled / CLike(Rust) | `rustc` | | | 1.56 (edition 2021) | `-vV` (7.6) |

### 7.3 Tags, EngineUnknown and `engine_available`

- **Canonical tags.** `canonical(tag)` maps an id or alias to `spec.id` everywhere: block headers (`@python stats` is a `py` block), selectors (`@python.stats` = `@py.stats`, `python.os.path` = `py.os.path`), `engine_available`, `[engines.<tag>]`, `V2_ENGINE_*`, `--engine` and `v2 doctor TAG`. Messages print the canonical form (2.5).
- **Profiles are static.** The lexer takes a tag's scanning profile from the fixed built-in table; every other tag, registered or not, is Plain (3.8, D3). The parser never rejects a tag as unknown.
- **EngineUnknown.** `resolve_tag(tag, span)` runs when a block is reached or a selector is bound. A miss raises `EngineUnknown: unknown engine '@pyhton' (did you mean '@py'?)` (did-you-mean per 5.9, over ids and aliases); with no close match, `unknown engine '@lua' — known engines: py, js, ts, c, cpp, rust; declare it in [engines.lua] or call register_engine` (the list is the registered engines: py, js and ts in Phase 1; the `declare it…` hint appears with custom engines, Phase 3). Custom tags have the shape `[a-z][a-z0-9_]{0,31}` (D42), so a tag the lexer accepts outside it (`@PY`) is simply unknown.
- **`engine_available(tag)`** (D41): `false` for an unknown tag (no error); otherwise `true` iff resolution succeeds (7.4, 7.6). It may run probes (cached, 7.7) but never starts a session or compiles, and a later crash (8.2) does not change its answer.

### 7.4 Custom engines in the registry

- `[engines.<tag>]` tables with a non-built-in tag and `register_engine(tag, spec)` share 17.1's validator. Names (D42, 17.2): `v2`, `export` and `import` are reserved; a tag or alias that is built-in, a built-in alias, declared in config, or already registered with a different spec raises `EngineNameConflict`; an identical re-registration is a no-op.
- A custom spec gets profile Plain, no candidates, minimum or probe, `ext` defaulting to the tag (D37), and 18.1's path rules for its `command` (D38). `V2_ENGINE_<TAG>` and `--engine` replace `command[0]` (18.4), also for a tag registered later.
- Resolving it only finds `command[0]` (7.5) and runs nothing (`version` is `None`; doctor VERSION `-`). A miss is `EngineNotFound: 'lua' not found on PATH (engine declared in C:\proj\v2.toml)`.

### 7.5 Finding an executable

Children are always spawned by absolute path (Windows `Command` would search the application and system folders before `PATH`). `find_program(name, base)`:
1. `name` containing `/` or `\`: `base.join(name)` (`base` per 18.1); on Windows an extensionless name is also tried with each accepted extension.
2. Otherwise each `std::env::split_paths(PATH)` entry in order, skipping empty and relative entries such as `.` (D43: the cwd is never searched, so a cloned project cannot shadow `python`). Windows: `dir\name` if it already has an accepted extension, then `dir\name{ext}` per `PATHEXT` entry (unset: `.COM;.EXE;.BAT;.CMD`; case-insensitive), accepting only `.com`, `.exe`, `.bat`, `.cmd` (`CreateProcess` cannot start `.py` or `.ps1`). Unix: `dir/name` passing `access(X_OK)`.
3. The hit must be a regular file (`symlink_metadata` when `fs::metadata` fails, as for `WindowsApps` aliases) and stays absolute, not canonicalized. `.bat`/`.cmd` hits are spawned directly (Rust ≥ 1.77.2 quotes arguments for `cmd.exe`; its `InvalidInput` for an unquotable argument is a spawn failure, 8.3).

### 7.6 Per-engine rules

**Order.** The configured command or `[python] venv` (whichever 18.3 picks), else the candidates. A configured value never falls back to auto-detection: a missing one is `EngineNotFound: engine 'py' command 'C:/Python312/python.exe' not found (set by C:\proj\v2.toml)`, and a failing probe is an error too (doctor `broken`; too old: `old`): `engine 'py' command 'C:\x\python.exe' (set by C:\proj\v2.toml) is not usable: version probe failed (exit code 1)`. "Set by" names the file's full path, the `V2_ENGINE_<TAG>` variable or `--engine`. Skip reasons: `Microsoft Store placeholder`, `too old: 3.6.8 < 3.8`, `version probe failed (exit code 1)`, `version probe timed out (10 s)`, `no version in its --version output`, or 8.3's `could not start '…': …`. Each auto-detected hit is probed (7.7); a hit whose probe fails, times out or matches no version, that is below the minimum, or that has the wrong target is skipped (kept in `skipped`) and the search goes on (next `PATH` entry, then next candidate). Probes of `PATH` hits always pass arguments: an App Execution Alias run without arguments opens the Microsoft Store.

- **py.** venv: `<venv>\Scripts\python.exe` on Windows, `<venv>/bin/python` then `bin/python3` elsewhere; missing: `EngineNotFound: [python] venv 'C:\proj\.venv' has no Scripts\python.exe (create it: python -m venv .venv)`. A hit under `%LOCALAPPDATA%\Microsoft\WindowsApps` (case-insensitive) is an alias: a real Store or Install Manager Python, or the placeholder that prints "Python was not found" and exits 9009 (skipped: `Microsoft Store placeholder`). `py -3` (`find_program("py")` + `-3`) is Windows-only and last. It and a Windows venv `python.exe` are launchers that run the real interpreter as a child, and aliases change target without their file changing; so the version is re-checked from `hello` (8.4) and every kill is a tree kill (8.3).
- **macOS.** `/usr/bin/python3`, `cc`, `gcc`, `g++`, `clang` and `clang++` are `xcrun` shims that open an install dialog when the Command Line Tools are missing; V2 first runs `/usr/bin/xcode-select -p`, and a non-zero exit skips them (`Command Line Tools missing`).
- **js** is `node`. **ts** has no process of its own: `resolve(ts)` = `resolve(js)` plus a type-stripping method; without js: `EngineNotFound: engine 'ts' (TypeScript) not found — @ts blocks run on the js engine`, then js's tried, skipped and install lines:
  - `transpiler = "node"` (default, D33): decided from the probed Node version, since only 22.13+ on the 22 line and 23.2+ have `module.stripTypeScriptTypes`. Older: `EngineNotFound: @ts blocks need Node 22.13+ (or 23.2+) for built-in type stripping (found 22.6.0) — upgrade Node or set [engines.ts] transpiler`; the running worker is re-checked at the first ts load (12.5). Strip mode blanks types with spaces, so positions stay exact; `enum`, `namespace` and parameter properties need a transpiler.
  - `transpiler = <command>` (D36): a string is one program path (never split), an array an argv; `argv[0]` via 7.5, missing: `EngineNotFound: [engines.ts] transpiler 'tsx' not found`. The host runs it once per distinct ts source per run (memoized by sha256), bounded by the js `startup_timeout` (a `Probe` timer, 8.5): TypeScript on stdin, JavaScript from stdout; the stdin write and the stdout and stderr reads are concurrent (8.3). A non-zero exit is a `ForeignError` (lang `ts`) carrying its stderr, at the block's span. Lines are taken 1:1 (no source maps), so they are approximate with a transpiler that reformats code.
- **c, cpp** (D35). `cl` (Windows only; a candidate or a configured `argv[0]`) is resolved specially:
  1. `find_program("cl")` hits and `INCLUDE` is non-empty (a Developer Prompt): use it with the current environment; FROM `PATH`.
  2. Else the probe cache's `vcvars` entry if still valid (7.7), else a fresh capture (FROM `vswhere`): `%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe -nologo -utf8 -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath` (`VC.Tools.ARM64` on arm64; no output: not found), then `cmd.exe /u /d /s /c ""<install>\VC\Auxiliary\Build\vcvarsall.bat" x64 >nul && set"` (`arm64` on arm64; `/u` makes `set` write UTF-16LE, so non-ASCII user folders survive). From the `NAME=VALUE` lines (names starting with `=` skipped) keep `INCLUDE` (empty: broken, `vcvarsall.bat did not set INCLUDE`), `LIB`, `LIBPATH` and the `PATH` entries not already in `PATH` (prepended at use); `cl.exe` is `find_program("cl")` over that `PATH`.
  3. A configured absolute `cl.exe` gets the current environment if `INCLUDE` is set, else the vswhere capture.

  Family: file stem `cl` is Msvc; probe output containing `clang` is Clang; else Gcc (gcc, cc, MinGW). Target: `-dumpmachine`, or for cl the folder holding `cl.exe` (`x64`, `x86`, `arm64`). It must pass 15.7's target check (V2's arch, with `x86_64` = `x64` = `amd64` and `aarch64` = `arm64`, and an accepted OS; on Windows gcc and clang only with a MinGW target, `*-w64-mingw32` or `*-windows-gnu`), else the hit is skipped (`targets i686-w64-mingw32 but V2 is x86_64`; a clang reporting `*-windows-msvc`: `clang targets MSVC — use cl`). Ch. 15 receives `family`, `target`, `argv` and `env`.
- **rust.** `rustc -vV`: `release: (\d+)\.(\d+)\.(\d+)`, `host: (\S+)`, target checked as above. The `rustc` on `PATH` is usually the rustup proxy, which picks a toolchain from `RUSTUP_TOOLCHAIN` or a `rust-toolchain.toml` in the cwd while its own file never changes; so a run that uses Rust runs `<proxy> --print sysroot` once (cwd = the user's) and sets `argv[0] = <sysroot>/bin/rustc[.exe]`, which keys the probe cache and is what ch. 15 compiles with. `shown` stays the proxy (DOCS 2). rustc finds MSVC `link.exe` itself.

### 7.7 Version probes and the probe cache

- **Running:** `argv + probe args`, stdin null, stdout and stderr are read concurrently (8.3) and joined stdout first (lossy UTF-8), cwd the user's, host env plus `spec.env`, 8.3's child rules; a 10 s `Probe` timer (60 s for vcvarsall), then `kill_tree`. **Parsing:** 7.2's patterns; gcc/clang: the first `(\d+)\.(\d+)(?:\.(\d+))?` of the first `--version` line; cl, run without arguments: the first `\d+\.\d+\.\d+` of its localized banner.
- **Cache** `~/.v2/cache/toolchains.json` = `{"format": 1, "entries": {…}, "vcvars": {…}}`. Entry key: sha256(argv, i.e. the path and any prefix arguments such as `py`'s `-3`, canonical path if `canonicalize` succeeds, size, mtime ns, spec hash), spec hash = sha256(engine id, probe args, pattern, V2 version); an alias is stat'ed as the link itself. Value: `{"path", "ok", "reason", "version", "output"}`, plus `"family"` and `"target"` for compiled engines (Phase 2); a too-old hit keeps its `version`; skips are stored too (`ok: false`) so `v2 check` can reuse them. `vcvars.<arch>` = `{"install", "vcvarsall_size", "vcvarsall_mtime_ns", "cl", "include", "lib", "libpath", "path_prefix"}`, valid while `vcvarsall.bat` keeps its size and mtime and `cl` and every `include` folder exist (a VS update deletes the old toolset).
- **Access:** runs read, and write misses; `v2 doctor` re-probes and overwrites; `v2 check` only reads; `--no-cache` skips reads (19.4). Writes go to `toolchains.json.<pid>.tmp`, then rename (a failed rename is ignored); an unreadable file or unknown `format` counts as empty.

### 7.8 EngineNotFound messages

`EngineNotFound` (exit 3 when uncaught) means there is no usable toolchain (D40): none installed, only too-old versions, an executable that cannot start (8.3), a configured command, venv or transpiler that does not exist, or a Node too old for ts stripping (7.6). Custom engines: 7.4.

```text
EngineNotFound: engine 'py' (Python) not found
  tried: python3, python, py -3 on PATH
  skipped: C:\Users\me\AppData\Local\Microsoft\WindowsApps\python.exe (Microsoft Store placeholder)
  install: winget install -e --id Python.Python.3.12
  or set [engines.py] command in v2.toml, V2_ENGINE_PY, or --engine py=PATH
```

| engine | Windows | Linux | macOS |
|---|---|---|---|
| py | `winget install -e --id Python.Python.3.12` | `sudo apt install python3` / `sudo dnf install python3` | `brew install python` |
| js, ts | `winget install -e --id OpenJS.NodeJS.LTS` | https://nodejs.org (distro packages may be older than 18, or than 22.13 for ts) | `brew install node` |
| c, cpp | `winget install -e --id Microsoft.VisualStudio.2022.BuildTools` (workload "Desktop development with C++"), or MSYS2 + `pacman -S mingw-w64-ucrt-x86_64-gcc` | `sudo apt install build-essential` / `sudo dnf install gcc gcc-c++` | `xcode-select --install` |
| rust | `winget install -e --id Rustlang.Rustup` | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` | as Linux |

---

## 8. Sessions and worker lifecycle

`engines/session.rs`, on the child rules and watchdog of `engines/process.rs` (8.3, 8.5). A session is the one worker process of a bridged engine for the whole run: `py`, `js` (which also runs every ts unit) and one per custom bridged tag. Frames: ch. 9; adapters: ch. 14.

### 8.1 Session

```rust
pub struct Session {                                // SessionTable: by key, plus creation order (SessionId = index)
    pub key: Rc<str>, pub engine: Rc<ResolvedEngine>, pub state: RefCell<SessionState>,   // "py", "js" (also ts), custom
    child: RefCell<Option<ChildProc>>,              // std Child + Windows job handle or Unix pgid (8.3)
    to_worker: RefCell<Option<BufWriter<ChildStdin>>>, from_worker: RefCell<Option<BufReader<ChildStdout>>>,
    pub hello: RefCell<Option<Hello>>, next_id: Cell<u64>,   // host request ids (9.7)
}   // unit state: 12.1; release queue: 11.2. No RefCell borrow is held across frame dispatch: a callback may re-enter
pub enum SessionState { NotStarted, Spawning, Handshaking, Ready, Busy(u32), Closing, Closed, Dead(DeadReason) }
pub enum DeadReason { SpawnFailed(Rc<ErrorValue>), StartupTimeout(Duration), CallTimeout(Duration), Protocol(String),
                      Exited { code: Option<i32>, signal: Option<i32>, during: Option<Rc<str>>, at: Span } }
```

### 8.2 State machine

| From | Event | To |
|---|---|---|
| NotStarted | first use: block reached, module import, `v2 doctor TAG` (never `engine_available`) | Spawning |
| Spawning | spawn ok / spawn error | Handshaking / Dead(SpawnFailed): `EngineNotFound` |
| Handshaking | `hello`, then `ret` for `init` / deadline / EOF or `err` for `init` / bad frame / `hello` version too old | Ready / Dead: `EngineTimeout` / `EngineCrashed` / `ProtocolError` / `EngineNotFound` (8.4) |
| Ready, Busy(n) | host sends a request (load, module, call, op), also nested ones while serving the worker's `call` | Busy(n+1) |
| Busy(n) | `ret` or `err` for the innermost request | Busy(n−1), Ready at 0 |
| Ready, Busy | EOF, pipe error, child exited, call timeout, protocol violation / program exit (8.7) | Dead(reason) / Closing, then Closed |

Once Dead, every use (loads, its Proxies, handle operations (11.7), imports from its units) raises `EngineCrashed: engine 'py' is not running (stopped earlier: exited with code 1 during a call to 'ratio' at main.v2:14); engines are not restarted`. The first use after an unobserved idle death raises 8.6's "exited while idle" instead, and Dead(SpawnFailed) re-raises its `EngineNotFound`. Releases for a Dead session are dropped (11.2); definition blocks it loaded stay `Loaded` on the host, and their Proxies fail as above. v1 never restarts a session.

### 8.3 Spawning and child processes

1. py, js: the embedded adapter is materialized per 14.1 and its path substituted for `{adapter}`.
2. `Command::new(argv[0]).args(rest + fixed_args)`, cwd inherited (the host never calls `set_current_dir`), env = the host's plus `spec.env`; js also gets `NODE_PATH` = absolute `[node] modules`, then `[node] paths` (D54, 14.1). Python paths travel in `init` (9.3), never in `PYTHONPATH`. stdin and stdout are piped (VEP), stderr is inherited. Before any user code the adapter moves the protocol to a private fd, sends fd-level output to stderr and makes language-level stdin raise (D45, D73; 9.9, 14.3, 14.4), so the user's stdin belongs to `input()` alone.
3. A spawn error is `EngineNotFound: could not start 'C:\...\python.exe': <OS message>` (Windows error 193 adds `(not a valid program for this machine)`).

**Child rules**, for every process V2 starts (workers, probes, transpilers, compilers, run engines, compiled programs):
- **Own session or group:** Unix: `pre_exec` calls `setsid()` (not `process_group(0)`), so the child leads a new session and process group and has no controlling terminal: terminal SIGINT and SIGHUP never reach it, and a `/dev/tty` prompt (git credentials, ssh, sudo, `getpass`) fails at once with `ENXIO` instead of stopping the child on SIGTTIN/SIGTTOU (DOCS 4.6). Windows: `CREATE_NEW_PROCESS_GROUP` (+ `CREATE_NO_WINDOW` when the host has no console), which blocks Ctrl+C only; Ctrl+Break and console close reach every process on the console. Children stay attached to the host's console (never `DETACHED_PROCESS`, whose descendants would open new console windows) and only stdin is replaced (null or a pipe), so a program that reads the console directly (`CONIN$`: an ssh password prompt, Python `getpass`) waits for keyboard input (DOCS 4.6). Adapters ignore SIGINT and SIGBREAK (14.3, 14.4), so a keyboard interrupt never makes a worker die mid-frame or print a `KeyboardInterrupt` trace; the host's console control handler sets `INTERRUPTED` as its first action, so failures of other children caused by the same event are never reported (8.7). stdin: a pipe for workers and transpilers, null for the rest (DOCS 4.6).
- **Windows:** created `CREATE_SUSPENDED`, assigned to its own Job Object (`KILL_ON_JOB_CLOSE | BREAKAWAY_OK`), then resumed (its thread found with `CreateToolhelp32Snapshot`, 17.3), so every descendant is in the job unless created with `CREATE_BREAKAWAY_FROM_JOB`. **Linux:** after `setsid()`, `pre_exec` runs `prctl(PR_SET_PDEATHSIG, SIGKILL)` and `_exit(1)` if `getppid()` is no longer the host; PDEATHSIG fires when the spawning *thread* exits, so spawns and shutdown run on the interpreter thread (1.3). **All:** adapters exit on stdin EOF (macOS has only this, so a busy worker outlives a SIGKILLed host until its next read).
- **`kill_tree`** is every kill V2 performs: Windows `TerminateJobObject(job, 1)`, Unix `kill(-pid, SIGKILL)` (the session leader's pid is its pgid; `kill(pid)` on `ESRCH`). Launchers (`py -3`, a Windows venv `python.exe`, `.cmd` shims, `bash`) run the real work in a descendant that also holds the pipes.
- **Lifetime (D44):** workers are killed as a tree at shutdown (8.7); a run-engine or compiled-program child is killed as a tree, with everything it started, when its block finishes, times out or V2 exits; probes and transpilers when they exit or time out.
- **Pipes** (every child but workers, whose stdin and stdout carry frames, 8.5): every piped stream is serviced at the same time as the others. Each piped output (expression-position stdout, the stderr tail, probe, compiler and transpiler output) gets its own helper thread that collects into memory or forwards; a transpiler's stdin is written from its own thread. The interpreter thread waits only for the exit (or the timer's kill), then runs `kill_tree`, then joins the helper threads, which reach EOF because the tree is dead, then reaps (below).
- **Reaping:** exit is observed without reaping (Unix `waitid(P_PID, pid, WEXITED|WNOHANG|WNOWAIT)`, Windows the process handle), then `kill_tree`, then `wait()` and the job handle closed: nothing a child started without detaching (`setsid`, `CREATE_BREAKAWAY_FROM_JOB`) outlives it, and the unreaped leader keeps its pgid from being reused.

`<run>` is the private per-run temp dir (0, 20.3).

### 8.4 Handshake

One deadline covers spawn → `hello` → `init` reply: `startup_timeout` (`[engines.js]`'s for ts; default 30 s; 0 = none), a `Startup` timer (8.5).
- **Before `hello`** (D74), lines that are not JSON objects (preload noise from `sitecustomize`, `.pth` files, `NODE_OPTIONS=--require`) are forwarded to the host's stderr; any other first frame is `ProtocolError: engine 'py' sent 'ret' before hello`. After `hello`, a line that is not a frame is a `ProtocolError` (9.11).
- **`hello`** must carry `vep: 1` (else 9.3's `ProtocolError`). Its `caps` are recorded and gate features (17.5, D68). For built-in engines its `version` must meet 7.2's minimum, because a launcher or alias may run another interpreter than the one probed: else kill, Dead(SpawnFailed), `EngineNotFound: Python 3.6.8 at C:\...\python.exe is too old (needs 3.8+)`.
- **`init`** (9.3): `ret` → Ready; `err` → kill, `EngineCrashed: engine 'py' failed to start: <message>`.
- **EOF first:** Windows exit status 9009 (a Store placeholder the probe did not catch) → Dead(SpawnFailed) with 7.8's `EngineNotFound`; otherwise `EngineCrashed: engine 'py' exited during startup (exit code 1); see its error output above`.
- **Deadline:** kill, `EngineTimeout: engine 'py' did not start within 30 s (C:\...\python.exe); raise [engines.py] startup_timeout if it is only slow`.

### 8.5 Requests and call_timeout

- **Before each request:** check whether the child exited (without reaping; if so, Dead, 8.6), check the cross-language depth limit (64, 9.7) and put pending releases on the frame (11.2).
- **Frame I/O:** compact `serde_json` plus `\n` in one `write_all`, then flush; reads split on `\n` only and drop one `\r` before it (D74); frames are at most 64 MiB in either direction (D72, 9.1). A session's stdout is read only while the host awaits a reply from it (and in 6.5's shutdown drain), so a worker can call V2 only while the host has a request outstanding to it (D46); adapters reject calls from idle code themselves (9.8).
- **Watchdog state** (the thread: 1.3; `engines/process.rs`): one `Mutex<Watch>` and a `Condvar`. `Watch` holds the timer stack (`Timer { id, limit: Option<Duration>, used, running_since: Option<Instant>, auto_paused, fired }`; `None` = no limit), the `KillHandle` of each timer, the ids of timers that fired, every running child's `KillHandle` (for the interrupt path, 6.5) and the run dir. `KillHandle` = the child's Job Object and a duplicated process handle, or its pid (= pgid). `push_timer(limit, kill)` returns a `TimerGuard` with `pause`, `resume` and `fired`; dropping it pops the timer. A timer pushed onto a running one pauses it until popped; an explicit `pause` lasts until `resume`. The watchdog waits at most 100 ms, or until the top timer's deadline.
- **Timers (D39):** each host request after `init` (load, module, call, op) pushes a running `Call` timer with `remaining = call_timeout` (`None` for 0); probes and transpilers push `Probe`. An inbound `call` from that worker pauses the top timer, sending its reply resumes it, and the reply to the request pops it. Only the top timer runs, so exactly the innermost pending host→worker request is timed, never V2 code or another engine: V2 → py (A runs) → V2 callback (A paused) → js (B runs, popped) → py (A resumes). Run engines and compiled programs use the same `Call` timers (message: 17.3); in-process calls (compiled exports, `extern`) cannot be interrupted and are never timed. Every 250 ms the watchdog also checks whether the top timer's child exited (`WaitForSingleObject(h, 0)`; `waitid` + `WNOWAIT`) and then runs `kill_tree`, so a descendant holding the stdout pipe cannot keep the read from reaching EOF.
- **Expiry:** holding the mutex, the watchdog runs `kill_tree` and sets `fired`; the blocked read gets EOF, sees `fired`, and the session becomes Dead(CallTimeout): `EngineTimeout: call to 'slow' in engine 'py' exceeded call_timeout (5 s); the engine was stopped` (v1 cannot cancel a call any other way). A child's timers and kill registration are removed under the mutex before it is reaped, so a reused pid is never signalled.

### 8.6 Crash detection

EOF on stdout, a pipe error (`EPIPE`, `ERROR_BROKEN_PIPE`, `ERROR_NO_DATA`; Rust ignores SIGPIPE), the child found exited, or a fatal protocol violation (9.11): `kill_tree`, reap, record the `DeadReason`.
- The pending request fails with `EngineCrashed: engine 'py' (pid 4242) exited during a call to 'ratio' (exit code 3221225477 = 0xC0000005); see its error output above` (Windows codes ≥ 0x80000000 also in hex; Unix: `killed by signal 11 (SIGSEGV)`), or with 9.11's `ProtocolError`. The worker's last output is already on stderr.
- A death while idle is noticed at the next use: `EngineCrashed: engine 'py' exited while idle (exit code 1)`; later uses get 8.2's "not running".
- Raised inside another engine's callback, the error travels back through that engine like any V2 error (9.6). EOFs caused by an interrupt are not reported (8.7).

### 8.7 Shutdown and interrupts

6.5 owns the shutdown order and the exit codes; the session part:
1. On every orderly exit (end of program, `exit(code)` even inside a callback, an uncaught error after it is printed; `exit` runs no `finally`, D15), each Ready or Busy session, newest first, gets `{"t":"bye"}`, its stdin is closed, state Closing. An adapter blocked in a stub exits without unwinding user code (9.10).
2. 6.5's shared 1 s drain forwards `out` frames (exit-handler output is kept) and ignores all others; worker exit codes are ignored.
3. Survivors are `kill_tree`d and every worker is reaped (8.3), state Closed; 6.5 then kills the remaining children and removes `<run>`.

Interrupts (Ctrl+C, Ctrl+Break, console close, SIGINT, SIGTERM, SIGHUP) are the host's (8.3): on Unix no terminal signal reaches a child, which runs in its own session; on Windows Ctrl+Break and console close reach every process on the console, adapters ignore SIGBREAK (14.3, 14.4), and because the host's console control handler sets `INTERRUPTED` first, child failures the same event causes are never reported. On 6.5's interrupt path the watchdog `kill_tree`s every registered child, workers included; the resulting EOFs are not reported as `EngineCrashed`, no `finally` runs, `<run>` is still removed, and the exit code is 130 on every OS (D15). A panic takes 6.5's hard path and exits 101.

---

## 9. The VEP/1 protocol

VEP/1 (V2 Engine Protocol, version 1) connects the host to every bridged engine: the `py` session, the `js` session (which also runs `ts` blocks) and custom `bridged` engines (17.5). Host side: `engines/vep.rs` on top of `engines/session.rs` (ch. 8); values go through `codec/json` (ch. 10); the built-in adapters are `adapters/py.py` and `adapters/js.js` (ch. 14). This chapter owns the wire format and the call discipline: the member names and shapes of every frame are defined in 9.2 and 9.4; their semantics are in 12.2 (`load`), 13.5 (`module`), 13.7 (`imports` entries) and ch. 11 (handle ops).

### 9.1 Transport and framing

- **Channels.** Host→worker frames travel on the worker's stdin, worker→host frames on its stdout. Worker stderr is inherited and never parsed.
- **Frames.** One UTF-8 JSON object per line, ended by a single `\n` (0x0A). Writers use a real JSON encoder (serde_json, `json.dumps`, `JSON.stringify`) with compact separators; it escapes every U+0000–U+001F inside strings, so a frame never contains a raw 0x0A. Raw U+2028/U+2029 inside strings are allowed.
- **Splitting.** Both sides read raw bytes, split on 0x0A only, then decode each complete line. Nothing splits on `\r`, U+0085 or U+2028: no Python text-mode `readline` or `for line in sys.stdin`, no Node `readline` module (splitting on a lone `\r` caused the old desync). The host drops one `\r` before the `\n` (D74); adapter parsers accept it as JSON whitespace.
- **Size limit: 64 MiB per frame, both directions (D72).** The host refuses to send a larger frame: `MarshalError: value too large to send to engine 'py' (70 MiB, limit 64 MiB)`, nothing sent. A longer incoming line is fatal: `ProtocolError: frame from engine 'py' exceeds 64 MiB`. An adapter never sends a larger frame: a reply becomes `err` `MarshalError`, a stub call raises `MarshalError` in the foreign code, `out` text is chunked (9.9).
- **Host I/O.** Reading is a bounded `read_until`: the `BufReader<ChildStdout>` is scanned with `fill_buf`/`consume` and gives up once the pending line passes the limit. Each outgoing frame is serialized into one `Vec<u8>` and written with one `write_all` + `flush`. A write error (EPIPE, `ERROR_BROKEN_PIPE`, `ERROR_NO_DATA`) is handled like EOF: the worker is gone (8.6).
- **Adapter I/O.** Stdin EOF or a broken stdout pipe means the host is gone: the adapter exits quietly with status 0. A host line it cannot parse, or a reply it is not waiting for, is reported on stderr and the adapter exits with status 2 (14.2).
- **Evolution.** Both sides ignore unknown members. The set of `t` values is fixed for VEP/1: an unknown `t` from a worker is fatal (9.11); an adapter answers an unknown `t` that carries an `id` with `err` `ProtocolError` and keeps running (17.6).

### 9.2 Frame catalogue

| `t` | Direction | `id` | Members |
| --- | --- | --- | --- |
| `hello` | W→H | – | `vep`, `engine`, `impl`, `version`, `pid`, `caps` (9.3) |
| `init` | H→W | yes | `project_root`, `paths` (9.3) |
| `load` | H→W | yes | `unit`, `kind`, `lang`, `label`, `file`, `line`, `source`, `args`, `imports`, `exports`, `want_value` (9.4; 12.2) |
| `module` | H→W | yes | `unit`, `name`, `from`, `names`, `as_handle` (9.4; 13.5) |
| `call` | both | yes | `target`, `args`, `kwargs` (9.5) |
| `op` | H→W | yes | `op`, `h`, plus `name`/`key`/`value`/`n` as the op needs (9.5, 11.3) |
| `ret` | both | echoed | `value`; plus `exports` and `missing` when answering `load`/`module` |
| `err` | both | echoed | `error` (9.6) |
| `out` | W→H | – | `stream` (`"stdout"` or `"stderr"`), `text` (9.9) |
| `release` | both | – | `h` (H→W: handle ids) or `fn` (W→H: callable ids) (9.10) |
| `bye` | H→W | – | – (9.10) |

`init`, `load`, `module`, `call` and `op` are requests: each gets exactly one `ret` or `err`. A session is `hello`, `init`, any number of requests (with nested traffic), then `bye`. In `op` frames and `call` targets, `h` is the bare handle id, not an `h` tag (10.3).

Example: a `py` block labeled `risky` imports the V2 function `log_line` (route 1) and exports `ratio`. The worker's `call` carries the worker's own id 1; the host's `call` has host id 3.

```json
{"t":"hello","vep":1,"engine":"py","impl":"CPython","version":"3.12.4","pid":4242,"caps":["modules","handles","kwargs","reverse"]}
{"t":"init","id":1,"project_root":"C:\\work","paths":["C:\\work"]}
{"t":"ret","id":1,"value":null}
{"t":"load","id":2,"unit":"0:812","kind":"definition","lang":"py","label":"risky","file":"C:\\work\\main.v2","line":3,"source":"\ndef ratio(a, b):\n    log_line(\"dividing\")\n    return a / b\n\n","args":{},"imports":[{"as":"log_line","route":1}],"exports":["ratio"],"want_value":false}
{"t":"ret","id":2,"value":null,"exports":[{"name":"ratio","kind":"func"}],"missing":[]}
{"t":"call","id":3,"target":{"unit":"0:812","name":"ratio"},"args":[1,0],"kwargs":{}}
{"t":"call","id":1,"target":{"route":1},"args":["dividing"],"kwargs":{}}
{"t":"ret","id":1,"value":null}
{"t":"err","id":3,"error":{"type":"ForeignError","message":"division by zero","foreign_type":"ZeroDivisionError","trace":[{"file":"C:\\work\\main.v2","line":6,"col":12,"func":"ratio"}],"text":"Traceback (most recent call last):\n..."}}
```

### 9.3 Handshake: `hello` and `init`

**`hello`** is written by the adapter before any user code runs. Ch. 8.4 owns its deadline (`startup_timeout`), the minimum-version check and the startup error messages.
- `vep` must be `1`; anything else is fatal (9.11): `ProtocolError: engine '{tag}' speaks VEP/{n}; this V2 speaks VEP/1`. `engine` is the adapter's tag. `impl` and `version` (`"CPython"`/`"3.12.4"`, `"node"`/`"22.13.1"`) are shown by `v2 doctor` and `v2 doctor TAG` (D34). `pid` is the interpreter's own pid, not a `py -3` launcher's; crash messages use it (8.6).
- `caps` is a list of strings. The host gates features on `modules`, `handles`, `kwargs` and `reverse` (D68; what each enables and the error without it: 17.5) and ignores unknown caps. The py adapter sends `["modules","handles","kwargs","reverse"]`; the js adapter sends `["modules","handles","reverse"]` (no `kwargs`, D63).
- **Preload noise (D74).** Before `hello`, a stdout line that is not a JSON object (output of `sitecustomize`, `.pth` files, `NODE_OPTIONS=--require`) is copied to the host's stderr. The first JSON object must be a valid `hello`, else fatal `ProtocolError`. From `hello` on, every line must be a frame (9.11). EOF before `hello` is a failed start (8.4).

**`init`** is always the first request; the adapter answers `ret` with `value: null` (an `err` kills the worker, 8.4).
- `project_root`: the absolute folder holding `v2.toml`, or the entry script's folder.
- `paths`: ordered, de-duplicated absolute folders for the engine's module search. py: `project_root`, then `[python] paths`, prepended to `sys.path`; the adapter also inserts each `.v2` file's folder before the project root at that file's first `load`/`module` (14.3), which gives DOCS 5's order. js: `[]`, because Node gets `[node] modules` and `[node] paths` through `NODE_PATH` at spawn (D54, 14.1). Custom engines: `[]`.

### 9.4 Units: `load` and `module`

`load` members are those of 9.2 (there is no `col`); the host fills them as 12.2 says. The protocol rules are:
- **`unit`** is the wire UnitId `"<FileId>:<byte offset of the block's @>"`. Module units are `"<canonical tag>:<name>"` (`py:statistics`, `js:date-fns`; `ts.pkg` is `js:pkg`, D52): one unit per module per session, shared by every importing file (13.5). Concurrent action runs of one block share its id, so adapters never key action state by it.
- **`kind: "definition"`** is sent once per UnitId. The adapter keeps the namespace until `bye` only if its reply's `missing` is empty. A failed definition load is never resent: the host marks the unit Failed and re-raises the stored error on every later reach (D31, 12.1). **`kind: "action"`** is sent on every execution: fresh namespace, run, drop; objects that escaped as handles or stubs stay alive.
- **`lang`** is the canonical tag; `"ts"` makes the js adapter strip types first (12.5). **`label`** is the block's label as written, or `null`. **`file`** (absolute `.v2` path) and **`line`** (the `.v2` line of `source` line 1) are what adapters compile with, so frames inside blocks already carry `.v2` files and lines; the host maps columns itself (12.6). **`source`** is the dedented body with directive lines blanked (4.4); V2 never inserts values into it (D13).
- **`args`** are bound as unit globals, then **`imports`**, then the code runs. An `imports` entry has exactly one of four shapes (when each is used: 13.7): `{"as","unit","name"}` binds an export or module attribute already in this session; `{"as","unit","module":true}` binds a module object of this session; `{"as","value"}` binds a decoded constant; `{"as","route"}` binds a stub for that route (13.6). **`want_value`** asks for the completion value (12.2).

The `ret` of a `load` or `module` carries `value`, `exports` and `missing`. `exports` is a list of `{"name","kind","value"?}`: `kind` is `"func"`, `"class"` or `"value"`, and `value` (the constant, copied at load) is present only for `"value"`. `missing` lists the requested names the unit does not define; the host raises `ImportError` (12.2, 13.5). An exception while running the block is answered with `err`; a foreign syntax error is `ForeignError` with `foreign_type: "SyntaxError"` at the `.v2` line (12.3, 12.4, 12.6).

**`module`** imports a foreign package into its module unit (13.5). `from` is the absolute path of the importing `.v2` file; adapters resolve against it (`createRequire`, the `sys.path` folder of 9.3). `names` is a list, `"*"`, or `null` with `as_handle: true`, in which case `ret.value` is the module object as a handle. A second `module` for the same unit reuses the imported module. The not-found mapping to `ImportError` is 13.5's. Without the `modules` cap the host raises before sending (17.5).

### 9.5 `call` and `op`

| `target` | Sent by | Meaning |
| --- | --- | --- |
| `{"unit","name"}` | host | a Proxy call (13.4): `name` is looked up in the unit's namespace when the call arrives (gone since load: `err` `NameError`, 12.3); a JS class is constructed (11.3) |
| `{"h"}` / `{"h","method"}` | host | calls the handle's object, or one of its methods, in one round trip (11.3) |
| `{"route"}` | worker | calls a route created by an inner `@import` (13.6); a `Pending` route resolves at this first call (D53) |
| `{"fn"}` | worker | calls a V2 callable the worker received as a value (11.8) |

- `args` (array) and `kwargs` (object) are always present, empty when unused. The host expands a spread `...xs` into `args`. Named arguments go only to sessions with the `kwargs` cap (10.8).
- A worker `call` with a `unit` or `h` target, or from a worker without the `reverse` cap, is fatal. An adapter answers a host `call` with a `route` or `fn` target with `err` `ProtocolError`.
- An unknown `route` or `fn` id is answered with `err` `ProtocolError: unknown callable id 42`; the session stays usable.

`op` goes host→worker only, and never to a session without the `handles` cap: `getattr {h,name}`, `setattr {h,name,value}`, `getitem {h,key}`, `setitem {h,key,value}`, `len {h}`, `iter {h}`, `next {h,n}`, `str {h}`, `value {h}` (semantics: 11.3–11.5). `setattr` and `setitem` answer `null`; `next` answers `{"items":[...],"done":bool}`.

### 9.6 `ret` and `err`

`ret.value` is an encoded value (ch. 10). `err.error` has these members:

| Member | Content |
| --- | --- |
| `type` | a V2 error kind: `ForeignError` for a foreign exception; `ImportError`, `NameError`, `ArityError`, `ValueError`, `MarshalError`, `ProtocolError` or `Error` for failures the adapter detects itself |
| `message` | the message text |
| `foreign_type` | the foreign exception class, such as `"ZeroDivisionError"`, or `null` |
| `trace` | frames `{"file","line","col","func"}`, innermost first, adapter-internal frames left out; `col` in code points or `null`; `func: null` at a block's top level (12.6) |
| `cause` | optional: the chained exception (`__cause__`, `e.cause`) in the same shape, at most 8 deep |
| `text` | optional: the engine's own formatted traceback |
| `v2` | optional: the `error` tag (10.3) of a V2 error that propagated through foreign code |

**How the host converts `err`:**
- With `v2`, the host restores that error (type, message, data, location) and appends the foreign frames to its trace, so a V2 error thrown in a callback comes back with its original type (DOCS 10).
- Without `v2`, the kind is `type` if it names a runtime error kind (DOCS 3.9, 10; `SyntaxError` is static-only, D1), otherwise `ForeignError` with `foreign_type = type`. The location is the innermost frame that maps into a `.v2` file, else the V2 call site (2.4, 12.6); `e.lang` follows 12.6. If `trace` is empty, the lines of `text` become the trace.
- Adapters make every error string well-formed first (10.4).

**The other direction.** When a V2 callback fails, the host's `err` always carries `v2`. Adapters raise it as `V2Error` (`class V2Error(Exception)` in Python, `class V2Error extends Error` in JS), which exposes `.type`, `.message` and `.data` and keeps the tag, so if foreign code does not catch it, it goes back in `v2` unchanged.

### 9.7 Ids, depth and the re-entrant loop

- **Ids.** Each side numbers its own requests 1, 2, 3 … per session (u64, never reused). `ret`/`err` echo the other side's id, so the two counters never collide.
- **LIFO replies.** A reply must answer the receiver's innermost outstanding request. Anything else is fatal: `ProtocolError: engine 'py' broke the protocol: reply to request 7, which is not the innermost pending request`.
- **Depth limit (DOCS 8.4).** `xdepth` counts the crossings in progress across all engines: each outstanding host request, each worker `call` being served, each in-process native call (16.7) and each `host_call` (15.9). The crossing that would be the 65th raises `RecursionError: cross-language call depth limit (64) exceeded`: at the V2 expression before a request is sent, or, for a worker `call`, as its `err` reply, so it unwinds through the foreign code back to V2 with its kind.

```rust
fn request(&mut self, s: SessionId, f: Frame, at: Span) -> Result<Value, Flow> {  // init, load, module, call, op
    let _level = self.xdepth.enter(at)?;                     // RecursionError past 64; left on every path
    let id = self.send(s, f)?;                               // queued releases go first (9.10); size check (9.1)
    loop {
        match self.read_frame(s)? {                          // EOF → EngineCrashed, deadline → EngineTimeout (8.5, 8.6)
            Frame::Ret { id: r, value, .. } if r == id => return codec::decode(value, Mode::Vep(s), self),
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

**The adapter loop** (skeleton: 14.2) is `serve(wait_id)`; the top level runs `serve(None)`. For each incoming frame:
- `ret` or `err`: it must carry `wait_id` and ends the loop; otherwise the adapter reports it on stderr and exits with status 2.
- `release`: applied. `bye`: the adapter exits (9.10). EOF: it exits quietly.
- Anything else is a request: push it on the adapter's stack, handle it (possibly recursively), send exactly one `ret` or `err`, pop it.

A **stub** (a route or `fn` function; 14.2's `host_call`) checks 9.8, sends `call` with a fresh id (buffered output and queued releases go first, 9.9), marks itself waiting on the stack, runs `serve(id)`, and returns the decoded `value` or raises `V2Error`.

**User code cannot end the worker.** Request handlers catch everything (Python `BaseException`, any thrown JS value) and reply `err`: `sys.exit()` becomes a `ForeignError` with `foreign_type: "SystemExit"`, and JS `process.exit()` throws an `Error` (D55; 14.3, 14.4). Adapters ignore SIGINT and SIGBREAK; the host owns interrupts (6.5).

### 9.8 When a worker may call V2

**Only while V2 is waiting on this engine (D46).** A stub may send `call` only while the innermost entry on the adapter's stack is a host request that is executing: during a synchronous call, while that request's coroutine or Promise is awaited, or during a nested request served inside a stub. A callback stored earlier stays valid and may be called during any later request. A timer, event handler, thread or `__del__` that fires while the worker is idle, or while it is blocked in a stub (V2 is computing then), gets a `V2Error` (type `Error`) raised in the foreign code, and nothing is sent: `V2 function 'progress' called outside an active V2 call` (DOCS 8.4). In JavaScript, timers and event handlers run only right after a call returns (the event-loop turn the adapter yields after every outermost reply, 14.4), or while V2 is waiting on a JS Promise. One that calls V2 right after a call fails; one that fires while V2 awaits a JS Promise is allowed.

**Only from the dispatch thread.** Python stubs compare `threading.get_ident()` with the dispatch thread's id: `V2 function 'progress' called from another thread`. JS stubs always run on the main thread, because functions cannot be sent to `worker_threads`.

The host cannot enforce either rule: it reads a worker's frames only while a request to that worker is pending (17.6).

**Awaiting (the async rule).** The result of a `call`, and the completion value of a `load` even with `want_value: false`, are awaited when awaitable (`inspect.isawaitable`; a thenable). `op` results never are.
- **py:** `loop.run_until_complete(x)` on the session's event loop (one per session, created on first use), at any depth. If an event loop is already running in the thread, the reply is `err` type `Error`: `'fetch' returned a coroutine while another coroutine is being awaited; call it directly from V2`.
- **js:** only a request served from the idle listener (stack depth 1) is awaited, and the event loop keeps running meanwhile (14.4). Inside a stub's synchronous wait the event loop cannot run, so the reply is `err` type `Error`: `'fetch_json' returned a Promise during a nested cross-language call; only an outermost call can be awaited` (DOCS 8.4).
- Before either error reply the adapter calls `coro.close()` or attaches a no-op rejection handler, so no "never awaited" or unhandled-rejection warning appears.

### 9.9 Output and worker stdio

**Stdio contract (D45, D73; implementation: 14.3, 14.4).** User code is kept off the protocol stream:
- **Protocol descriptors.** Python moves the protocol to private non-inheritable fds (`os.dup` of fds 0 and 1), then `dup2`s the null device to fd 0 and fd 2 to fd 1 (14.3). Node has no `dup2`, so it keeps the protocol on fds 0 and 1 (D73's "private fd" applies to Python only): it writes frames with a saved `fs.writeSync(1, …)` and reads with `fs.readSync(0, …)`, and wraps everything else that reaches those fds: `process.stdout`, `console` and `fs` writes to fd 1 are captured, `process.stdin` and `fs` reads of fd 0 are replaced, and `child_process` stdio is rewritten (14.4).
- **Language-level output** (`print`, `sys.stdout.write`, `console.log`, `process.stdout.write`) is sent as `out` frames, in program order.
- **Fd-level output goes to stderr:** C extensions, `os.write(1, …)`, and `os.system`/`subprocess`/`child_process` children that inherit stdout.
- **Stdin.** Language-level stdin is replaced by a stream whose reads raise (14.3); fd-level and child reads see EOF. The user's stdin belongs to V2's `input()` (DOCS 4.6).
- **Remaining gap.** In Node, code that bypasses the wrappers (a native addon, or reopening `/dev/stdout`) can still write fd 1 and corrupt the stream (fatal `ProtocolError`, 9.11), or read fd 0 and steal protocol bytes.

**Adapter side.**
- Buffering and flush points follow 14.2: a write ending in `\n`, the size threshold, an explicit flush, right before any other frame, and at `bye`. `out` text is sent in chunks below the frame limit. Queued releases go out as one `release` frame just before the next frame.
- An adapter writes frames only while it is executing a host request or handling `bye`; during a request any thread may flush `out` under the send lock.
- Output produced while idle (Python non-dispatch threads, signal handlers, JS timers) stays buffered and goes out with the next frame (D45). Once 1 MiB of it is buffered, a Python non-dispatch thread that writes more waits until the next request starts.
- Lone surrogates in output become U+FFFD (10.4).

**Host side.**
- `out` text for `"stdout"` goes through `Out`, the writer of V2's `print` (5.10); `"stderr"` text goes to the host's stderr after `Out` is flushed.
- While a worker runs, the host is blocked in `request()` for it and handles its frames in arrival order, so block output and V2 output appear in program order, also when redirected (DOCS 4.6). The host flushes `Out` before writing any frame and before `input()` blocks.
- Idle output appears before the worker's next frame; its position relative to V2 output printed in between is not guaranteed.

### 9.10 `release` and `bye`

**`release`** carries refcount decrements (ch. 11).
- `h` (host→worker) lists handle ids the host dropped; `fn` (worker→host) lists callable ids whose stubs died. An id appears once per dropped reference. No reply; applied at once; unknown ids are ignored.
- Neither side writes it from a finalizer: finalizers only append to a queue, which goes out as one `release` right before the next frame that side sends.
- At V2 statement boundaries the host also sends a standalone `release` to any session whose queue holds 256 or more ids. This is always safe: while V2 code runs, every worker is idle or blocked in a stub, so it is reading. Sessions without the `handles` cap never get `release`.

**`bye`** is sent on every orderly exit path (end of program, `exit()`, uncaught error; 6.5, 8.7), possibly while the worker is blocked in a stub; interrupts kill workers instead. The adapter flushes its output and exits with status 0 without running user `except`/`finally` code:
- **Python at depth 0:** `sys.exit(0)` from the main loop, so normal finalization and `atexit` handlers run.
- **Python inside a stub:** `atexit._run_exitfuncs()`, a flush, then `os._exit(0)`; raising `SystemExit` there would run through user code.
- **Node:** the saved `process.exit(0)`.

After `bye` the host drains the worker's stdout for the grace period, forwarding `out` frames and ignoring everything else (8.7).

### 9.11 Malformed input and `ProtocolError`

From `hello` on, every stdout line must parse as a JSON object whose `t` is a string (D74); there is no stray-output recovery. A fatal protocol error kills the worker and marks the session Dead (8.2). The current operation raises `ProtocolError: engine 'py' broke the protocol: <what> (got: <first 80 bytes, escaped>)`; later uses raise `EngineCrashed` (8.2).

**Fatal:**
- a line that is not a JSON object, invalid UTF-8, a lone-surrogate `\u` escape (serde_json rejects them), or a line over 64 MiB;
- an invalid first frame (8.4), or `vep` ≠ 1 (9.3);
- an unknown `t`, or a required member that is missing or has the wrong type;
- a `ret` or `err` that does not answer the innermost host request;
- a frame a worker may not send: `init`, `load`, `module`, `op`, `bye`, a `call` with a `unit`/`h` target, a `call` without the `reverse` cap, an `h` tag without the `handles` cap;
- an unknown or malformed `$v2` tag, an `int` tag outside i64, or value nesting deeper than 256 (10.5).

**Not fatal:** a worker `call` with an unknown `route`/`fn` id (the host answers `err`); an adapter's `err` of type `ProtocolError` (raised in V2).

### 9.12 Why it cannot deadlock

- **One thread of control** spans all processes. Exactly one party computes at a time; every other party in the call chain is blocked in its loop, reading its input.
- **The host writes only to readers.** It is the only router (workers never talk to each other) and writes only to workers whose input is being read: requests to idle workers, replies to workers blocked in a stub, standalone `release` frames to either. The js adapter reads stdin with a blocking `fs.readSync` whenever it is idle or waiting in a stub (14.4), except during the one event-loop turn it yields after every outermost reply; that turn sends nothing and does not wait on the host, so a frame written meanwhile is read once it ends. While it awaits a depth-1 Promise the host is blocked reading its output, so nothing is written to it then. In JavaScript, timers and event handlers run only right after a call returns, or while V2 is waiting on a JS Promise (9.8).
- **Workers write only while the host reads.** Adapters write only while executing a request (9.9); the host is then in `request()` for that session and drains the pipe. A Python non-dispatch thread may block on a full pipe while the host serves one of that worker's calls; the dispatch thread keeps reading stdin meanwhile and needs the send lock only once the host reads again.
- **Hangs are not deadlocks.** A hang inside user code is covered by `call_timeout` (8.5). A custom adapter that follows 9.1–9.11 and the 17.6 checklist gets the same guarantee.

---

## 10. Codec and marshaling

### 10.1 One codec

`codec/json.rs` is the only JSON implementation (0.1). It serves VEP frames, `json_parse`/`json_stringify` (5.10), the JSON of reverse calls from compiled blocks (15.9) and `V2_ARG_*` values (17.4). It wraps `serde_json` with three features:
- `preserve_order`: objects keep insertion order, like V2 dicts; with duplicate keys the last value wins.
- `float_roundtrip`: exact float parsing.
- `unbounded_depth`: serde_json's own limit of 128 is below V2's limits, so the codec calls `disable_recursion_limit()` and bounds nesting itself. Before parsing, a byte scan rejects input whose bracket nesting outside strings exceeds 1,000 (fatal `ProtocolError` on the VEP path, `ValueError` in `json_parse` per 5.10, `MarshalError` for compiled-block JSON), which bounds the parser's recursion. Conversion then enforces the container limit of 256 on the VEP and compiled paths (10.5). So `json_parse` accepts nesting up to 1,000, and `json_stringify` has the same bound: encoding a value nested deeper is a `ValueError`.

`arbitrary_precision` is not enabled, so serde_json turns an integer token above u64::MAX or below i64::MIN into an f64 while parsing, and the parsed value no longer shows it was an integer. The same byte scan therefore also checks each number token outside strings: in `json_parse`, a token with no `.`/`e`/`E` whose value is outside i64 is `ValueError: invalid JSON at line L column C: integer does not fit in 64 bits` (5.10). On the VEP and compiled-block paths such a token decodes as a `Float` per 10.2, as serde already does; a serde `u64` above i64::MAX becomes a `Float` there too.

Base64 (standard alphabet, padded, strict decoding) is about 40 hand-written lines in the same file.

```rust
pub enum Mode { Vep(SessionId), Plain }   // Vep: the session whose handles and callables are in play
pub fn encode(v: &Value, m: Mode, rt: &mut Runtime) -> Result<serde_json::Value, ErrorValue>;
pub fn decode(j: serde_json::Value, m: Mode, rt: &mut Runtime) -> Result<Value, ErrorValue>;
```

| Mode | Used for | `$v2` tags (10.3) |
| --- | --- | --- |
| `Vep` | VEP frames | all |
| `Plain` | `json_parse`, `json_stringify`, `V2_ARG_*`, `args_json` and results of compiled-block reverse calls (15.9) | none: `"$v2"` is an ordinary key. `bytes`, `func`, handles, errors and non-finite floats are rejected with an error that names the path (`json_stringify`: 5.10; `V2_ARG_*`: 17.4; compiled-block reverse calls: `MarshalError`, 15.9) |

- **Top-down decoding.** The content of a `dict` tag is never reinterpreted. Adapters do the same: plain `json.loads`/`JSON.parse` first, then a top-down walk; never `object_hook` or a `reviver`, which work bottom-up.
- **Failed encodes.** If an encode fails part-way, the callable-table counts it added (11.8) are rolled back.

### 10.2 Numbers

| Step | Rule |
| --- | --- |
| host encodes `Int` | a JSON integer if abs(n) ≤ 2^53−1 or the mode is not `Vep`; else `{"$v2":"int","v":"<decimal>"}` |
| host encodes `Float` | finite: serde_json's shortest round-trip text, which always has a `.` or an exponent (`1.0`, `0.1`, `1e+21`, `-0.0`), so `json_stringify(2.0)` is `"2.0"`; NaN/±∞: the `float` tag (serde_json alone would write `null`) |
| host decodes a literal | no `.`/`e`/`E` and fits i64 → `Int`; no `.`/`e`/`E` but outside i64 → `Float`, the nearest double (`json_parse`: `ValueError` from the 10.1 scan, 5.10); anything else → `Float`. serde_json reads `-0` as the float `-0.0` |
| host decodes an `int` tag | parse as i64; out of range → fatal `ProtocolError` |

- Adapters for languages with unbounded integers send integers outside i64 as handles, never as bare literals, which the host would read as floats.
- The py adapter tags non-finite floats, then encodes with `allow_nan=False` as a safety net.
- So the int/float kind survives every crossing: `1.0` stays a float in Python. JavaScript has only `number`, so an integral JS number that fits i64 arrives as `int` (DOCS 9.1).

### 10.3 `$v2` tags

An object with a `"$v2"` member is a tag in a value position: `args`, `kwargs` values, `value`, export, import and `op` values, and anything nested inside them. Frame envelopes are never tags.

| Tag | Members | Meaning |
| --- | --- | --- |
| `h` | `id`, `type` | a handle (ch. 11). Worker→host: a new reference. Host→worker: only to the owning session |
| `fn` | H→W: `id`, `name`. W→H: `id` or `route` | a V2 callable (11.8); `route` marks a stub made by an inner `@import` (13.6) |
| `int` | `v`: decimal string | V2 ints beyond ±(2^53−1) going out; JS `BigInt`s within i64, and integral JS `number`s beyond ±(2^53−1) but within i64 (10.7), coming back |
| `bytes` | `b64` | `bytes`, as standard padded base64 |
| `float` | `v`: `"nan"`, `"inf"` or `"-inf"` | a non-finite float |
| `error` | `type`, `message`, `data`, `lang`, `foreign_type`, `file`, `line`, `col`, `trace` (strings), `cause` (tag or `null`) | a V2 error value |
| `dict` | `v`: object | a real dict that has a `"$v2"` key: the keys of `v` are literal, its values are decoded normally |

An unknown tag or a missing member is a fatal `ProtocolError` on the host (9.11); adapters answer it with `err` `ProtocolError`.

### 10.4 Strings

Host strings are always valid UTF-8. Foreign strings can hold lone surrogates: in Python any code point U+D800–U+DFFF (for example `surrogateescape` file names), in JS an unpaired UTF-16 half. Handling depends on where the string appears:
- **Values** (arguments, results, attribute values, dict keys): `MarshalError: string contains an unpaired surrogate (U+DC80) at index 12`. Python detects it through the `UnicodeEncodeError` of `s.encode("utf-8")`; JS through `s.isWellFormed()` (Node ≥ 20) or `/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/`.
- **Text** (`out`, error messages, `text`, traces, `str` op results, the `type` of `h` tags): each lone surrogate becomes U+FFFD (Python `re.sub("[\ud800-\udfff]", "\ufffd", s)`, JS `s.toWellFormed()` or the regex), so printing and error reporting never fail.

### 10.5 Containers

- **Copying.** Lists and dicts are copied in both directions, so mutations never propagate (DOCS 9.1). Substructure shared inside one value is copied once per occurrence.
- **Dict keys** on the wire are strings. Whoever encodes a dict that has a `"$v2"` key wraps it in the `dict` tag.
- **Depth limit: 256 nested lists/dicts.** Host encode fails with `MarshalError: value nested deeper than 256 levels`; host decode fails with a fatal `ProtocolError`; adapter encode sends the container at depth 257 as a handle.
- **Cycles.** Host encode tracks the `Rc` pointers on the current path; a revisit fails with `MarshalError: cannot send a list that contains itself`. Adapter encode tracks object identities on the current path and sends a revisited container as a handle: Python `a = [1]; a.append(a)` arrives as `[1, <handle>]`.
- **Error paths.** Errors inside containers name the path, e.g. `… (at args[0]["when"])`.

### 10.6 Python

**V2 → Python:** `null`, `bool`, `int`, `float`, `str`, `bytes` become `None`, `bool`, `int`, `float`, `str`, `bytes`; `list` and `dict` a new `list` and `dict` in the same order; `func` a stub (11.8); a handle of this session its original object; `error` a `V2Error` instance, not raised.

**Python → V2**, checked in this order, with `type(x) is …` fast paths first:
1. `None` → `null`. `bool` → `bool` (before `int`, because `bool` subclasses `int`); numpy `bool_` (type module `numpy`, type name `bool_` or `bool`) → `bool`.
2. `int` and its subclasses (`IntEnum`), any `numbers.Integral` (numpy integer scalars register themselves) → `int(x)`; outside i64 → handle.
3. `float` and its subclasses (`numpy.float64`), numpy floating scalars (`numbers.Real` from module `numpy`) → `float(x)`, non-finite through the `float` tag. `Fraction`, `Decimal`, `complex` → handles.
4. `str` and its subclasses → `str`, after the surrogate check. Exactly `bytes` → `bytes`; `bytearray` and `memoryview` → handles.
5. Exactly `list` or `tuple` → `list`, and exactly `dict` with only `str` keys → `dict`, recursively (D48). Subclasses (namedtuples, `OrderedDict`, `Counter`, custom lists) and dicts with other keys → handles, so `row.region` works on an `itertuples()` row (DOCS 9.2).
6. A V2 stub → `fn` tag. A `V2Error` → `error` tag.
7. Anything else → handle: functions, bound methods and classes (callable handles, 11.6), modules, sets, `datetime`, `DataFrame`, generators.

### 10.7 JavaScript

**V2 → JS:**
- `null`, `bool`, `str`, `float` become `null`, `boolean`, `string`, `number` (`1.0` and `1` are the same JS value). `int` becomes a `number` when abs(n) ≤ 2^53−1, otherwise a `BigInt` (from the `int` tag).
- `bytes` becomes a `Buffer`, `list` an `Array`, `func` a stub (11.8), `error` a `V2Error`; a handle of this session its original value.
- `dict` becomes an ordinary object whose prototype is `Object.prototype`. Its entries are own data properties defined with `Object.defineProperty`, never by assignment, so a `"__proto__"` key stays an ordinary key. It is not a null-prototype object, because those break `String(obj)`, template literals and `obj.hasOwnProperty`.
- Values are decoded in the adapter's realm, not the unit's (12.4): blocks use `Array.isArray(x)`, not `x instanceof Array`. JS lists integer-like keys first, in ascending order: `{b: 1, "2": 2}` comes back as `{"2": 2, b: 1}`.

**JS → V2**, checked in this order:
1. `undefined` and `null` → `null`. `boolean` → `bool`.
2. `number`: NaN and ±Infinity → `float` tag. A number for which `Number.isSafeInteger(x)` is true is written as a JSON integer: `3` → `int`, `-0` (written `0`) → `int` 0. An integral number outside ±(2^53−1) but inside i64 (−2^63 ≤ x < 2^63) is sent as the `int` tag with `v: BigInt(x).toString()`, the double's exact value, so `2**60` arrives as `int` 1152921504606846976 (`JSON.stringify` would write `1152921504606847000`). Any other finite number is written by `JSON.stringify`, and the host reads it as a `float` (10.2): non-integral numbers (`3.5`) and integral numbers outside i64 (`1e20`, written `100000000000000000000`).
3. `bigint` within i64 → `int` tag, any other `bigint` → handle. `string` → `str`, after the surrogate check.
4. `Buffer` or any `Uint8Array` (`util.types.isUint8Array`, which works across realms) → `bytes`; other typed arrays → handles.
5. `Array.isArray(x)` → `list`, recursively; holes become `null`.
6. A plain object → `dict` over `Object.keys`; symbol keys are ignored. Plain means prototype `null`, or a prototype `p` with `Object.getPrototypeOf(p) === null` and `p.constructor.name === "Object"` (any realm; `instanceof Object` fails across `vm` contexts).
7. A V2 stub → `fn` tag. A `V2Error` → `error` tag.
8. Anything else → handle: functions and classes (callable handles, 11.6), symbols, `Map`, `Set`, `Date`, `RegExp`, class instances, other `Error`s, and Promises that are not awaited (9.8).

### 10.8 Named arguments

- Named arguments travel in `call.kwargs`; a spread `...xs` is expanded into `args` first. Block args are not kwargs: they travel in `load.args`.
- Before sending, the host checks the destination session's `kwargs` cap (9.3, 17.5). The py adapter has it and calls `f(*args, **kwargs)`. For js and ts functions (and custom engines without the cap), a non-empty `kwargs` raises `ArityError: js functions take positional arguments only (got 'color')` and nothing is sent (D63). This applies to imported functions, handle calls and method calls alike.
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

### 11.2 Host side and releases

```rust
pub struct HandleRef { session: SessionId, id: u64, type_name: Rc<str>, repr: Rc<str>,   // repr: native pointers only (16.6)
                       releases: Option<Rc<RefCell<Vec<u64>>>> }   // the session's queue (8.1); None: native pointer
impl Drop for HandleRef {
    fn drop(&mut self) { if let Some(q) = &self.releases { if let Ok(mut q) = q.try_borrow_mut() { q.push(self.id) } } }
}
```

- **One `Rc` per tag.** Each decoded `h` tag creates a new `Rc<HandleRef>`, matching one count on the worker side; the host never deduplicates.
- **`Drop` only queues.** No code keeps the queue borrowed across other calls (the flush drains it inside one short borrow), so `Drop` is safe anywhere: mid-decode, during unwinding, at shutdown.
- **Flushing.** A session's queue goes out as one `release` frame right before the host's next frame to that session, or as a standalone frame at a statement boundary once it holds 256 ids (9.10).
- **Dead sessions and shutdown.** Queued ids of dead or closed sessions are discarded; `bye` frees everything, so no releases are sent at shutdown.

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
| `for x in h`, `value(h)` | `op iter`/`next`, `op value` | 11.4, 11.5 | 11.4, 11.5 |

- **JS constructors.** A function `f` is a constructor when `Function.prototype.toString.call(f)` starts with `class`, or when `f` is native (`{ [native code] }`), has a non-writable own `prototype`, and is not `Number`, `String`, `Boolean`, `Symbol` or `BigInt`. So classes, `Map`, `Date` and `URL` are constructed, while ES5-style `function Point() {}` is called without `new` (as in 12.4).
- **Method calls.** In call position `h.m(...)` takes one round trip: the interpreter evaluates the callee to `Callable::HandleMethod{handle, method}` without a `getattr`, and calling it sends `call {h, method}`. Anywhere else, `h.m` is a `getattr`.
- **Results and errors.** Results are marshaled like any value: plain data is copied, anything else becomes a new handle. Exceptions become `ForeignError` with the foreign class (`AttributeError`, `KeyError`, `TypeError`, …) in `foreign_type`. Named arguments follow 10.8. `call` results are awaited (9.8), `op` results never.

### 11.4 Iteration

`for x in h` (DOCS 9.2, D28):
1. The host sends `op iter {h}`. The worker calls `iter(obj)` (Python) or `obj[Symbol.iterator]()` (JS; not iterable → `TypeError`) and returns the iterator as a new handle.
2. The host repeats `op next {h: it, n: 64}`; each answer is `{"items": [...], "done": bool}`.
3. The loop ends when `done` is true and the received items are used up. `break` or an error simply drops the iterator handle.

`for i, x in h` gives the index and the item, as for lists; this holds for mapping-like handles too.

**Batching.** A `next` returns more than one item only if the iterator's `batch` flag is set. The adapter sets it at `iter` time when the source is an eager built-in container whose iteration runs no user code: Python `list`, `tuple`, `dict` and its views, `set`, `frozenset`, `str`, `bytes`, `range`, numpy `ndarray`; JS `Array`, typed arrays, `Map`, `Set`, strings. Other iterators (generators, `map` objects, `itertuples()`) return one item per `next`, so foreign side effects such as prints stay in step with the loop body.

### 11.5 `value(h)`

`op value {h}` converts the object to plain data inside the worker (D49), recursively up to depth 256; deeper nesting or a cycle is a `MarshalError`. At each node:
1. **Plain values** convert as in 10.6/10.7.
2. **JSON-convertible containers** convert element by element: Python `list` and `tuple` (subclasses included, such as namedtuples) and `Mapping`s whose keys are all `str` (`dict`, `OrderedDict`, `Counter`); JS arrays and plain objects.
3. **Hooks.** Otherwise a callable `to_dict` is tried first, then a callable `tolist`, and the result converts from step 1. So a pandas `Series` becomes a dict (DOCS 17.2) and a numpy array a list.
4. **Anything else** is `MarshalError: value(): can't convert datetime.date to V2 data`: a `datetime`, a class instance, an int outside i64, a set, a JS `Map` or `Set`. A mapping with a non-`str` key reads `value(): can't convert dict to V2 data (key 1 is not a str)`.

A hook that raises gives a `ForeignError`. `value(x)` on a non-handle returns `x` unchanged (5.10); on a native pointer it is a `MarshalError` (16.6).

### 11.6 Callable handles, identity, diagnostics

- **Callable handles (D47).** Foreign functions, bound methods, classes and callable objects that come back as values (call results, `getattr`, attributes of a module handle) are handles: `type(h)` is `"handle"`, and they are called as `h(...)`. Names brought in with `@import` are `func`s (Proxies, 13.4), not handles. A callable handle obeys every handle rule, so it cannot be passed to another engine; wrap it instead: `js_fn(lambda(x) => h(x))`.
- **Equality.** `h1 == h2` is true exactly when session and id match. The identity map (11.1), together with removal only at count 0, makes this mean "the same foreign object" for all live handles. A handle equals nothing else.
- **No round trips.** `type(h)`, truthiness (always true) and diagnostics never contact the worker. Diagnostics show `<py datetime.date>`, built from `type_name`.

### 11.7 Invalid uses

| Use | Error |
| --- | --- |
| a handle sent to another session (in args, kwargs, block args, or inside lists/dicts) | `MarshalError: this handle belongs to engine 'py' and can't be passed to 'js' — convert it with value(h) first`, plus the path (10.5) |
| a handle as an argument of a compiled export or `extern` function; returned to compiled code by a reverse call | `MarshalError: cannot pass a py handle (datetime.date) to native code; convert it with value(h) first` (16.5); `MarshalError` (15.9) |
| any use after its session died | `EngineCrashed` (8.2) |
| arithmetic, ordering, `in`, slicing | `TypeError` with the "do it inside a block / `value(h)`" hint (5.2, 5.3; D28) |
| a handle as a dict key (`d[h]`, `{[h]: 1}`) | `TypeError: dict keys must be str, not handle` |
| `json_stringify(h)` | `TypeError` (5.10) |

### 11.8 V2 callables inside workers

When the host encodes a V2 `func` for session S (a closure, a builtin, a `Proxy` of any session, a `Native` function or a `HandleMethod`), it registers the function in S's callable table and sends `{"$v2":"fn","id":n,"name":"log_line"}`. A session without the `reverse` cap gets a `MarshalError` instead (17.5).

```rust
struct CallableTable { next: u64, entries: HashMap<u64, Entry>, by_ptr: HashMap<*const Callable, u64> }
struct Entry { func: Value, count: u32 }
```

- **Reuse.** Sending the same `Rc<Callable>` again reuses its id and does `count += 1`. The table holds the `Rc`, so the pointer key stays valid.
- **Worker stubs.** The worker keeps one stub per id in a weak map (Python `weakref.WeakValueDictionary`, JS `Map<id, WeakRef>`), so one V2 function is one foreign object (`f is g`); its name is the V2 name. Each stub owns a cell `{id, n}` counting the `fn` tags it absorbed.
- **Release.** When a stub is collected, its finalizer (Python `weakref.finalize`, JS `FinalizationRegistry`) only appends the id `n` times to the release queue, which goes out as `release {fn: [...]}` before the worker's next frame. The host subtracts once per occurrence and drops the entry at 0. A stub that dies just as its id is sent again is safe, because the new stub gets its own cell.
- **Calls.** Calling a stub sends `call {"fn": n}` (9.7). The host calls the value, re-entering the interpreter, and encodes the result for S; a V2 error goes back as `err` with `v2` (9.6).
- **Passing back.** A stub passed to the host is sent as `{"$v2":"fn","id":n}` and decodes to the original V2 value; counts do not change. A stub made by an inner `@import` is sent as `{"$v2":"fn","route":r}` and decodes to that route's target (a `Pending` route resolves first). Routes live in `Interp.routes` (13.6), not in this table, and are never released.

### 11.9 Leaks

- **Cross-process cycles are never collected.** A Python object that holds a V2 closure whose environment holds a handle to that same object keeps both alive until the session ends.
- **CPython** frees unreferenced stubs immediately through refcounting.
- **V8 finalizes late.** `FinalizationRegistry` callbacks run only after a garbage collection and an event-loop turn, so passing callbacks in a loop could grow the host's table without bound; the loop blocks in `fs.readSync` between requests (14.4), so the event loop does not turn on its own. After the reply to every outermost (stack depth 1) request, before the next `fs.readSync`, the js adapter therefore yields one event-loop turn (`await new Promise(r => setImmediate(r))`). If more than 1,000 `fn` stubs were created since the last turn, it calls `gc()` first. The `FinalizationRegistry` cleanups then run, and their queued `release` goes out with the next frame. During this turn the stack is empty, so a callback that calls V2 raises 9.8's idle-call `V2Error`. `gc` is obtained once, via `v8.setFlagsFromString("--expose-gc")` and `vm.runInNewContext("gc")`.

---

## 12. Block execution

Host: `engines/units`; workers: the adapters (ch. 14). This chapter is the path every block takes, plus loading bridged
units; compiled blocks continue in ch. 15, run engines in 17.3.

### 12.1 Reaching a block

A block is an `EngineBlock` (4.3) whose `body` and `loc` come from 4.4 and 2.3; each execution of it is a **unit**, named
by its `UnitId`. Only definition blocks keep a state, in `Interp.units` (6.2): `Loading`, `Loaded` or `Failed(err)`; an
action run leaves nothing on the host except its routes (13.6). Messages name a block with `block_name(b)`:
`block 'stats'`, or `@py block at main.v2:12` when it has no label (D31); tags always print canonical.

`exec_block(b, want_value)`:

1. **State** (definition blocks): `Loaded` → return `null`, evaluating nothing, not even the args (D30). `Failed(e)` →
   throw the stored `e` unchanged: no retry, no repeated side effects (D31). `Loading` → `ImportError: block 'stats' is
   still loading (reached again during its own load)`. Otherwise set `Loading`.
2. **Engine:** `registry.resolve(tag)`, else `EngineUnknown` / `EngineNotFound` (ch. 7).
3. **Directive check:** a **run-engine** block with `@export` or an inner `@import` → 17.3's `ImportError`; a compiled
   block (`c`/`cpp`/`rust` or an alias) with an inner `@import` but no `@export` (program mode), or with `@import { * }`
   or `@import SEL as h` → 15.2's `BuildError` (D57). It runs after the tag is resolved, before args and inner imports.
4. **Args:** evaluated left to right in the current scope.
5. **Inner imports** → `load` entries (13.7).
6. **Dispatch:** bridged → `load` (12.2) to the engine's session, spawned lazily (ch. 8; ts uses js's); compiled →
   ch. 15; run → 17.3.
7. **Definition:** register the exports (13.2), set `Loaded`. An error in steps 2–7 is stored as `Failed(err)`, then thrown.
8. **Value:** a bridged action with `want_value` → the decoded `ret.value`; a compiled or run action → its stdout as 6.3
   specifies (D18: captured in expression position with one trailing `\n` or `\r\n` stripped, streamed in program order
   in statement position); otherwise `null`.

Static checks (4.5; `SyntaxError`, D1; also reported by `v2 check`): a definition block in expression position; duplicate
arg names; args on a built-in compiled tag (`compiled blocks take no args — @export a function and call it with
arguments`, D32); an arg name equal to an inner-import alias of the same block (`'x' is already declared in this block`).

### 12.2 The `load` frame

The frame is 9.4's; the host fills it from the `EngineBlock`:

| Member | Value |
| --- | --- |
| `unit` | `"<FileId>:<byte offset of @>"` (`"0:812"`); not unique across concurrent runs of one action block (recursion), so adapters never key action state by it |
| `kind`, `lang` | `definition` or `action`; the canonical tag (`ts`, or `js` once a transpiler ran, 12.5) |
| `label` | the block's label as written, or `null` |
| `file`, `line` | the canonical `.v2` path (2.1); `loc.body_first_line`. No column member: the host maps columns itself (2.3, 12.6) |
| `source` | `EngineBlock.body`: dedented, directive lines blank, nothing added (4.4, D13) |
| `exports` | `[]` for an action; else the explicit `@export` names in order, plus the element `"*"` when the list contains `*` (4.4) |
| `imports` | 13.7 |
| `args` | object in written order of codec values (ch. 10), e.g. `{"nums":[4,8,15]}`; a V2 function goes as an `fn` tag (11.8); another session's handle is a `MarshalError` before sending (11.7) |
| `want_value` | `true` only for an action block in expression position |

**Reply** (9.4). Action: `{"value": v, "exports": [], "missing": []}` (`v` is `null` unless `want_value`). Definition:
`{"value":null,"exports":[{"name":"area","kind":"func"},{"name":"Point","kind":"class"},{"name":"VERSION","kind":"value","value":"1.2"}],"missing":["typo"]}`.
A non-empty `missing` → `ImportError: @export lists 'typo' but block 'geometry' does not define it`, at that `@export`
item; the unit is `Failed` and the adapter has already dropped it. `func`/`class` entries become Proxies (13.4); a `value`
entry is the copy taken at load (non-plain values arrive as handles; constant exports exist for py/js/ts only, D56).

**Adapter side.** Arg names and import aliases must be identifiers of the adapter's language that are neither keywords
nor names the adapter provides (Python: `str.isidentifier()`, not `keyword.iskeyword()`, no dunders; JS: 12.4's identifier
pattern, not reserved, not `undefined NaN Infinity globalThis __proto__`), else `err ValueError: block arg 'pass' is not
a valid Python name`. Args bind first, then imports, then the code runs. The completion value (py: the last statement
if it is an expression; js/ts: the script's completion value from `runInContext`) is always computed and awaited if
awaitable (9.8), and encoded only if `want_value`.

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
    units[m["unit"]] = mod
    return reply
```

- The prepended blank lines act like `ast.increment_lineno(tree, first - 1)` and also put `SyntaxError`/`SyntaxWarning`
  on the right `.v2` line, so `linecache` shows real `.v2` lines in tracebacks Python prints itself; the text V2 sent is
  unchanged (D13). Columns stay body-relative (12.6); `from __future__` works; `dont_inherit=True` keeps the adapter's
  flags out.
- While it lives, the unit is in `sys.modules` (for `pickle`, `dataclasses`, `typing.get_type_hints`) under a name with
  no dot (`pickle` imports a dotted name's parent); `__name__` is never `"__main__"` (D55).
- `export_reply`: the union, each name once, of the explicit names and, when `"*"` is listed, every key of `g` (dict
  order) not starting with `_` whose value the block defined (DOCS 6.1): a class, or a function after `inspect.unwrap`
  (admits `functools.wraps`, `lru_cache`, named lambdas), whose `__module__` is the unit's name, so imports, stubs and
  other units' objects are left out; an exception while checking means no. Absent explicit names go to `missing`. Kind: `class` if `inspect.isclass`, `func` if `callable`, else
  `value` with `encode(v)`.
- A Proxy call looks the name up in `g` at call time, so rebinding is seen; a name deleted since →
  `err NameError: 'area' is no longer defined in block 'geometry'` (9.5).

### 12.4 JavaScript load

```js
// NODE_GLOBALS (computed once): own names of globalThis absent from a fresh context, minus 'global', 'console'
function newContext(file) {                      // one vm context (one realm) per unit: private top-level names
  const sb = {}, module = { exports: {} };
  for (const k of NODE_GLOBALS) Object.defineProperty(sb, k, Object.getOwnPropertyDescriptor(globalThis, k));
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
  for (const [k, v] of Object.entries(m.args)) ctx[checkName(k)] = decode(v);  // adapter realm (10.7)
  bindImports(ctx, m.imports);                                              // 13.7
  const script = compileOrExplain(src, { filename: m.file, lineOffset: m.line - 1,
    importModuleDynamically: /\bimport\s*\(/.test(src) ? dynImport(m.file) : undefined });
  return settle(script.runInContext(ctx), v => {  // settle: now, or once a thenable settles (9.8)
    if (m.kind === 'action') return { value: m.want_value ? encode(v) : null, exports: [], missing: [] };
    const unit = { ctx, get: new Map() }, reply = exportReply(unit, src, m.exports);
    if (reply.missing.length === 0) units.set(m.unit, unit);
    return reply;
  });
}
```

- `require` is created for the `.v2` file (D54): `./x` resolves from its folder, bare names walk up `node_modules`
  folders from there (so the project root's is found), then `NODE_PATH` (14.1). `module`/`exports` only let
  CommonJS-style code run. No `columnOffset`: V8 applies it to line 1 only; the host maps columns (12.6).
- `dynImport(file)` = `spec => import(entryUrl(spec, file))` (14.4); without it `import()` fails with
  `ERR_VM_DYNAMIC_IMPORT_CALLBACK_MISSING`. It is attached only to sources containing `import(`: some Node versions never
  collect a script compiled with it, and per-run action contexts would leak.
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
``new vm.Script(`(() => ${name})`).runInContext(unit.ctx)``, which also sees `let`/`const`/`class`; other quoted names
(D11) read the context's own property. A `ReferenceError` at load puts the name in `missing`. Kind: `class` if
`Function.prototype.toString` starts with `class`, `func` for other functions (calls: 11.3), else `value`.

**`@export { * }`** scans the top level of `src` (after type stripping): context properties would miss
`const`/`let`/`class` and include `var x = require('x')`. The tokenizer skips whitespace, comments, `'…'`/`"…"` strings,
template literals (a stack tracks `${…}`) and regex literals (`/` starts one at input start, after any punctuator except
`)` `]` `}`, and after `return typeof instanceof in of new delete void throw case do else yield await`), and tracks
`()[]{}` depth. At depth 0 it collects `function [*] NAME`, `async function [*] NAME`, `class NAME`, and each
`const`/`let`/`var` declarator `NAME = INIT` whose INIT starts with `function`, `async`, `class`, `IDENT =>` or a balanced
`(…)` then `=>`. Declarators split at depth-0 commas; a declaration ends at a depth-0 `;` or at a line break whose
previous token is not `,`, `=` or `=>`. `_` names are dropped; the rest stay if their getter returns a function without
throwing. `exportReply` returns the union of these names and the explicit ones, each name once.

### 12.5 TypeScript

ts blocks run in the js session; the stripped text goes through 12.4 unchanged.

- `[engines.ts] transpiler = "node"` (default): `module.stripTypeScriptTypes(src, { mode: 'strip' })` blanks types with
  whitespace, so positions stay exact; `enum`, value `namespace` and parameter properties fail with
  `ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX`. The stripper parses the whole block, so it also throws on any syntax error, before
  V8 compiles anything. On any stripping failure the adapter sends `err` `ForeignError` (foreign_type `SyntaxError`) with
  one trace frame `{file: m.file, line: m.line + L − 1, col: C}`, where L:C is the 1-based body position the stripper's
  message reports (its `[line:col]` marker); with no readable position it sends no frame, so the error stays at the
  block's `@ts` (12.6). A `process.emitWarning` filter drops the one
  `ExperimentalWarning` it prints. It needs Node 22.13+ or 23.2+ (D33): the host decides from the probed Node version
  (confirmed by `hello.version`) and raises 7.6's `EngineNotFound` before the first ts load (D40); `engine_available("ts")`
  and doctor apply the same test.
- A transpiler command runs on the host as 7.6 specifies (once per distinct source; TypeScript on stdin, JavaScript on
  stdout; a non-zero exit is a `ForeignError`, D36). Its output is sent with `lang: "js"`; error lines are right only if
  the transpiler keeps lines.

### 12.6 Foreign positions to .v2 positions

- **Adapters** compile with the `.v2` path as filename and the block's line offset (12.3, 12.4), so frames inside blocks
  already carry the `.v2` file and line. Frames go innermost first, in 9.6's shape plus `col`; adapter frames and engine
  internals (`<frozen importlib._bootstrap…>`, `node:internal/…`, `node:vm`) are dropped; library frames stay.
- **`col`** is 1-based code points relative to the body text line (kept by `remember_source`), sent only for frames in
  `.v2` files, else `null`. Python 3.11+: `FrameSummary.colno` (0-based UTF-8 bytes) converted; 3.8–3.10: `null`. JS:
  parsed from `err.stack` (`at fn (file:line:col)`, UTF-16 units) and converted. Syntax errors: Python `e.offset`; V8's
  caret line; ts type stripping: the stripper's `[line:col]` marker, which gives the line too (12.5).
- **Host:** a frame in a known `.v2` file whose line lies in a block body gets body line `line − loc.body_first_line + 1`
  and its column from `UnitLoc::col` (2.3; a `null` col becomes the line's first non-blank column), and prints as 2.5 says
  (`in @py risky`).
- **Error fields:** `e.file/line/col` come from the innermost mapped frame, else the V2 call site (2.4; the excerpt prints
  either way, D2); `e.lang` is that frame's block tag (`ts` for ts blocks), else the session's tag.

---

## 13. Import resolution and routing

`engines/imports` resolves selectors against the file's export registry; `runtime/interp` binds the results.

### 13.1 Selectors

| Written | `Selector` (4.3) | Resolves against |
| --- | --- | --- |
| `@py` | `Union` | every definition block of that engine in this file |
| `stats`, `"odd name"` | `Label` | the block with that label, any engine |
| `@py.stats` | `Exact` | the block with that label, which must be a `py` block |
| `py.statistics`, `js."date-fns"` | `Module` | a module unit (13.5); segments joined with `.` (JS subpaths and scopes are quoted: `js."lodash/fp"`) |
| `v2` | `V2` | this file's top-level names; inside blocks only (4.5) |

Tags canonicalize at run time through the registry (7.3): `@python.stats` is `@py.stats`, `ts.x` is `js.x` (D52); an
unknown tag is 7.3's `EngineUnknown`. `sel as NAME` needs a `Module` selector (4.5, D10). Selectors are file-scoped: an
`@import` in `util.v2` or its blocks sees only `util.v2`'s blocks (6.4). Messages quote selectors as written.

### 13.2 ExportRegistry

```rust
pub struct ExportEntry { name: Rc<str>, unit: UnitId, value: Value }    // Func(Proxy | Native) or a copied constant
pub struct ExportRegistry { keys: HashMap<Rc<str>, Vec<ExportEntry>> }  // FileScope.exports (6.2)
```

When a definition unit becomes `Loaded`, each export is pushed under `"@<canonical tag>"` and, if labeled, under
`"<label>"` and `"@<tag>.<label>"` (duplicates within a block collapse; entries are never removed): bridged blocks register
`Proxy{session, unit, name}`, compiled blocks `Native` callables (15.8), `value` exports their copy. The file's
`EngineBlock` list is the static side (labels, tags, declared names, `*`) for "not run yet" checks.

### 13.3 Resolving a name from a block selector

`cands` = the file's definition blocks that `sel` matches, in source order, reached or not. Blocks are named by
`block_name` (12.1).

1. `cands` empty → `ImportError: No engine exports found for selector 'stats'`.
2. `hits` = the distinct `Loaded` units in `cands` whose entries under `sel`'s key contain `name`.
3. One hit → its `value`.
4. More → `ImportError: Import of 'mean' from '@py' is ambiguous (exported by blocks 'stats' and 'calc') — use @py.stats
   or @py.calc` (DOCS 6.3). Three or more read `'a', 'b' and 'c'` / `@py.a, @py.b or @py.c`; an unlabeled block is listed
   as `@py block at main.v2:12`, and the advice becomes `— label the blocks to choose one`.
5. None → the first candidate that is not `Loaded` and declares `name` or `*` decides: never reached →
   `ImportError: block 'stats' has not run yet — move this @import below it` (D53); `Loading` → `ImportError: block
   'stats' is still loading — its exports exist once its top-level code finishes`; `Failed(e)` → `ImportError: block
   'stats' failed to load: <e.message>` (D31).
6. Otherwise → `ImportError: 'x' is not exported by 'stats' (did the block @export it?)`.

`@import { * } from sel`: step 1; with no `Loaded` candidate, step 5 on the first; otherwise the union of the loaded
candidates' entries, where a name exported by two distinct units fails as in step 4, used or not (DOCS 6.3).

### 13.4 Binding into the V2 scope

`@import` declares each alias in the current scope like `const` (assigning it is an error; a name already declared there
is 4.5's redeclaration `SyntaxError`, or 5.6's `NameError` for names bound by `*`) and re-resolves each time it runs. `*`
binds only names that are V2 identifiers and not keywords; import others quoted, with `as`. Imported names are `func`
(D47): a foreign function becomes `Callable::Proxy { session, unit, name }` (5.1; `unit` is the wire unit id, 9.4), and
calling it sends `call` with target `{"unit","name"}` plus `args`/`kwargs` (9.5, 10.8). Proxies are ordinary values:
stored, passed, re-exported by V2 modules (6.4), sent to other engines as `fn` tags (11.8). `sel as NAME` binds a module
handle (13.5).

### 13.5 Module units

A `Module` selector names one unit per module per session, `"<canonical tag>:<name>"` (9.4): `py:statistics`,
`py:os.path`, `js:date-fns`; for a relative or absolute JS specifier `<name>` is the absolute path joined from the
importing file's folder. `ts.X` and `js.X` share the js session's unit `js:X` (D52). Only bridged engines take module
selectors (`ImportError: module imports need a bridged engine — 'c' is compiled` / `— 'lua' is a run engine`), and only
with the `modules` cap (17.5). Python names must be dotted identifiers (`ImportError: 'my-mod' is not a valid Python module name`); a JS name is any non-empty specifier
(package, `node:` built-in, `./path` from the `.v2` file). Every `@import` from a module sends one `module` frame; the
adapter imports once per unit, so a bare name's first import wins for the session, as in `sys.modules`:

```json
{"t":"module","id":7,"unit":"py:statistics","name":"statistics","from":"C:\\proj\\main.v2","names":["mean"],"as_handle":false}
```

`from` is the absolute path of the importing `.v2` file; the adapters use it for `sys.path` and `createRequire`. `names`
is a list, `"*"`, or `null` with `as_handle: true` (`@import py.numpy as np`). Reply:
`ret {"value": <module handle or null>, "exports":[{name, kind, value?}], "missing":[...]}`.

| Step | Python | JavaScript |
| --- | --- | --- |
| load | `importlib.import_module(name)`, the folder of `from` on `sys.path` (9.3) | `createRequire(from)(name)` (Node 20.19+/22.12+ `require` also loads synchronous ES modules); on `ERR_REQUIRE_ESM`, `ERR_REQUIRE_ASYNC_MODULE` or `ERR_PACKAGE_PATH_NOT_EXPORTED`, `import(entryUrl(name, from))` (14.4), awaited as 9.8 allows |
| failure | not found → `err ImportError` with the foreign message (`No module named 'pandas'`); any other exception → `ForeignError` (9.4) | the same, with Node's message |
| name `n` | `getattr(mod, n)`, else the submodule `import_module(name + "." + n)` | `mod[n]` unless inherited from `Object.prototype`/`Function.prototype`; `"default"` → `mod.default ?? mod`; else `mod.default[n]` |
| `*` | `__all__` if present (absent entries go to `missing`), else the names in `vars(mod)` not starting with `_`, in dict order | own enumerable keys of `mod` (and of an object `mod.default`), minus `default`, `__esModule` and `_` names |
| kind | class → `class`, callable → `func`, else `value` (copied; non-plain values arrive as handles) | function → `func`/`class` (12.4), else `value` |

- `@import js.pkg as x` binds the module object as a handle; for a CommonJS package that exports a function it is a
  callable handle (`@import js.slugify as slugify`, D51, D54). `@import { "default" as x } from js.pkg` takes
  `mod.default ?? mod`.
- A missing requested name → `ImportError: 'x' is not exported by 'py.statistics'`. A module Proxy targets
  `{"unit":"py:statistics","name":"mean"}`; the adapter repeats the lookup on every call.

### 13.6 Routes

Foreign code cannot hold a V2 `Value`, so what a block imports through the host is reached by numbered routes. A route
is an entry of `Interp.routes` (6.2), numbered separately from the callable table of 11.8; `{"route": r}` carries its
index, and routes are never released. Each holds:

```rust
pub enum Route {
    Target(Value),                                           // a resolved Func: Proxy, Native or Closure
    Pending { file: FileId, sel: Selector, name: Rc<str> },  // resolved at its first call (D53)
    V2 { file: FileId, name: Rc<str> },                      // `v2` selector: looked up at every call
}
```

One route per (importing unit, directive item), allocated at the importer's first load; each later load (a re-run action
block) resets a resolved route to `Pending`, so the table stays bounded by the import directives. On
`call {"route": r}` (9.5): `Target(f)` → call `f`; `Pending` → resolve now with 13.3 or 13.5 (it becomes `Target`) or raise
that `ImportError` into the caller; `V2` → call the file's current top-level `name` (`NameError: 'log_line' is not
defined at the top level of main.v2`; `TypeError: 'x' is not a function`). Compiled blocks keep the same `Route`s per
loaded library, keyed by alias (13.7).

### 13.7 Imports inside blocks

At step 5 of 12.1 each directive item becomes one `imports` entry (shapes: 9.4). Resolution is lazy (D53): an item is
bound at load only when its target is already available in the same session; everything else becomes a route resolved at
its first call, so blocks may import from each other in any order. `as` is the name bound in the block (the alias, else
the imported name).

| Target | Entry | The adapter binds |
| --- | --- | --- |
| an export of a `Loaded` block of **this session** (py→py, js↔ts), or an attribute of a module of this session (imported first with a `module` frame, 13.5) | `{"as","unit","name"}` | the real object, fetched once at load (`g[name]`, the JS getter, or the module attribute): no host hop, exceptions propagate natively; later rebinding in the source is not seen, as with Python's `from m import f` |
| a whole module of this session (`@import py.numpy as np` in a py block) | `{"as","unit","module":true}` | the module object |
| a `value` export of a `Loaded` block | `{"as","value"}` | a decoded copy (a same-session handle decodes to the live object) |
| anything else: another session's block or module, a compiled export, a block not loaded yet, `v2` | `{"as","route"}` | a stub that sends `call {"route": r}` (14.2) |

- Errors that no later load can fix are raised at load: a selector matching no block (13.3 step 1); no candidate
  declaring the name or `*` (step 6); an invalid module name (13.5); a whole-module handle of another engine
  (`ImportError: a py module can't be imported as a handle into a @js block (handles belong to their engine)`); a `v2`
  name not declared at the file's top level (`func`, `let`, `const`, `import`, `extern`, `@import`):
  `ImportError: 'x' is not a top-level name in main.v2`.
- Calling a route whose block has not loaded → `ImportError: block 'b' has not run yet — 'f' can't be called before it
  loads` (still loading or failed: 13.3 step 5). A route that resolves to a constant raises `TypeError: 'VERSION' is not
  a function` at that call; import constants from loaded blocks.
- `*` expands at load into explicit entries, so its targets must be loaded (a module is imported then); `*` from `v2`
  means the file's top-level `func` declarations not starting with `_`.
- Compiled blocks accept only `{ names } from SEL`, and only in library mode (D57; the BuildErrors are in 15.2). The host
  keeps alias → `Route` per loaded library; the generated wrappers pass the alias to `host_call` (15.9), never a route
  id, because a cached library outlives a run.

---

## 14. Built-in adapters

`adapters/py.py` and `adapters/js.js` are the worker side of ch. 9–13; custom bridged adapters follow the same shape
(14.5).

### 14.1 Distribution and launch

- Both files are embedded with `include_str!` and written create-new (0600 on Unix) to `<run>/adapters/` when their
  session first spawns (8.3); they are never read from the project, `PATH` or `~/.v2` (20.2).
- Launch argv and environment are 7.2's: `<python> -B -u <run>/adapters/py.py` with `PYTHONUTF8=1` and
  `PYTHONIOENCODING=utf-8`; `<node> <run>/adapters/js.js`. The js session also gets `NODE_PATH` = the absolute
  `[node] modules`, then `[node] paths`, then any inherited value (D54); Node reads it at startup.
- Code can run before the adapter owns fd 1 (`sitecustomize`, `.pth` files, `NODE_OPTIONS=--require`): until `hello` the
  host forwards non-JSON stdout lines to stderr; after it, any non-frame line is a `ProtocolError` (D74).

### 14.2 Shared skeleton

```text
setup()            14.3 / 14.4: import hygiene, protocol fds and stdio (9.9), signals
send(hello)        vep 1, engine, impl, version, pid (9.3); caps: py ["modules","handles","kwargs","reverse"],
                   js ["modules","handles","reverse"] (D68; JS takes no named arguments, D63)
serve(None)        the top-level loop (9.7); it ends only by exiting
serve(wait_id):    for each incoming frame m:
  ret, err         must carry wait_id: return the decoded value / raise the decoded V2Error; else stderr, exit 2
  release          apply (11.1, 11.8); no reply
  bye              flush output, exit 0 (9.10)
  init load module call op
                   push request; try: reply ret(handler(m)), awaiting per 9.8
                   except any exception e: reply err(serialize(e));  finally: pop
  other t, with id → err ProtocolError, keep serving (17.6);  unparseable → stderr, exit 2 (9.1)
  EOF              exit 0 quietly (9.1)
host_call(target, args, kwargs)                     route stubs (13.7) and V2 function stubs (11.8)
  top != request   raise V2Error (type Error): V2 function 'progress' called outside an active V2 call  (D46)
  Python, not the dispatch thread → raise V2Error (type Error): V2 function 'progress' called from another thread
  id = next_out_id(); send(call {id, target, args, kwargs}); push waiting; try: return serve(id)  finally: pop
send(f)            under the send lock: buffered output as out frames (9.9), queued releases as one release frame
                   (9.10), then f; a reply over 64 MiB becomes err MarshalError, an oversize call raises
                   MarshalError in the caller (9.1, D72)
```

- **The stack** (9.7) holds a `request` entry per host request being handled and a `waiting` entry per stub blocked in
  `serve(id)`. `host_call` proceeds only when the top entry is a `request`: a synchronous call, a request whose coroutine
  or Promise is being awaited, or a nested request served inside a stub (D46, 9.8). A callback stored during one request
  may be called during any later one; from a timer, an idle turn or a finalizer, or while the adapter is blocked in a
  stub, it raises.
- **Tables** live in the adapter's scope, never in user globals: `units` (wire unit id → namespace, for definition blocks
  and modules, never actions), handles (11.1), V2 function stubs (11.8) and the release queue. Finalizers only append to
  that queue, because they can run in the middle of `send`.
- **Errors** (`err.error`, 9.6): `type` is `ForeignError`, or the kind the adapter detected itself (`ImportError`,
  `NameError`, `ValueError`, `MarshalError`, `ProtocolError`, `Error`); `foreign_type` is Python `type(e).__name__`, JS
  `e.name`, or `typeof` for a thrown non-Error; `trace` per 12.6; `text` the engine's own formatted traceback. A
  `V2Error` that escapes user code goes back with its `v2` tag, so it round-trips unchanged.
- **Output** follows 9.9: output produced while idle waits for the next frame (D45). Every string sent as text is first
  made well-formed (10.4).

### 14.3 py.py

All code runs under `if __name__ == "__main__":`, so a `multiprocessing` spawn child (the default on Windows and macOS)
that re-imports `__main__` starts no second adapter. Setup, in order:

1. Import every stdlib module the adapter uses (`sys os io json ast types builtins inspect importlib keyword traceback
   linecache weakref threading signal base64 codecs collections`, plus `msvcrt ctypes` on Windows) while no user folder
   is on `sys.path`, so a user's `json.py` cannot replace them; `asyncio` is imported on first need with the original
   path (9.3).
2. Protocol, stdio and signals exactly as 9.9 (D73): the protocol moves to non-inheritable `os.dup` copies of fds 0 and 1
   (binary mode on Windows); fd 0 becomes `os.devnull` and `os.dup2(2, 1)` points fd 1 at stderr (plus `SetStdHandle` on
   Windows); `sys.stdout` and `sys.stdin` are replaced; SIGINT (and SIGBREAK) are ignored, since the host owns Ctrl+C
   (ch. 8). C extensions, `os.write(1, …)`, `os.system` and inheriting `subprocess` children therefore write to stderr
   and read EOF (D45).
3. Remove the adapter's own folder (`sys.path[0]`, i.e. `<run>/adapters`); `init` and each `load`/`module` then order
   the path per 9.3.

- **Framing.** `recv` reads `os.read(IN, 65536)` into a bytearray and splits on `b"\n"` only, so a `\r` inside a string
  cannot split a frame. `send` writes `json.dumps(obj, ensure_ascii=False, allow_nan=False,
  separators=(",", ":")).encode() + b"\n"` in an `os.write` loop under 9.9's lock.
- **Calls** run `f(*args, **kwargs)`. `BaseException` is caught per request, so `sys.exit()` in a call or a block is an
  `err` (`ForeignError`, foreign_type `SystemExit`, D55), not a dead worker. Awaitables run per 9.8 (one kept session
  loop; `err` when a loop already runs in the thread).
- **Python 3.8** is the floor for the adapter itself: no `match`, `X | Y`, `list[int]`, `str.removeprefix`, dict `|` or
  parenthesized `with`; `FrameSummary.colno` is 3.11+ and `SyntaxError.end_lineno` 3.10+.

### 14.4 js.js

Setup, in order, before anything touches `process.stdout` or `process.stdin`: their getters create libuv handles on the
protocol fds, which makes a POSIX pipe non-blocking (`writeSync` would then fail with `EAGAIN`) and may read ahead on
fd 0 (9.9).

1. `require` the built-ins used: `fs vm v8 path module stream string_decoder url util child_process`.
2. Stdio exactly as 9.9 (D73): the `process.stdout`, `stderr` and `stdin` getters are replaced (`console` binds lazily,
   so it follows), `fs` calls on fds 0–2 are wrapped, `child_process` stdio is rewritten so children never inherit fd 0
   or 1 (their output goes to stderr, D45), then `module.syncBuiltinESMExports()`. Frames are written only through the
   saved `fs.writeSync(1, …)`, the adapter's private channel, and fd 1 stays blocking. Native addons that write fd 1 are
   not covered; after `hello` their lines are a `ProtocolError` (D74).
3. `process.exit` throws 9.7's error (D55); the original is kept for `bye` (9.10).
4. `process.on('SIGINT' | 'SIGBREAK', () => {})`; `uncaughtException` and `unhandledRejection` print the stack to stderr
   and the worker keeps running (9.8).

The loop is 9.7's: an async top level that blocks in `fs.readSync` between requests, with nested `serve(id)` fully
synchronous; event-loop turns and Promise awaiting follow 9.8. After the reply to every outermost (stack depth 1)
request, before the next `fs.readSync`, the loop yields one event-loop turn (`await new Promise(r => setImmediate(r))`);
if more than 1,000 `fn` stubs were created since the last turn, it calls `gc()` synchronously first (11.9). During this
turn the stack is empty, so a timer or event handler that calls V2 raises 9.8's idle-call `V2Error`; the
`FinalizationRegistry` cleanups run, and their queued `release` goes out with the next frame. Calls use
`Reflect.apply`/`Reflect.construct` as 11.3 decides; named arguments never arrive, since js declares no `kwargs` cap and
the host raises `ArityError` before sending (10.8, D63).

- `entryUrl(spec, from)` (12.4, 13.5): a built-in (`node:` or in `Module.builtinModules`) stays as is; `./`, `../` or an
  absolute path → `pathToFileURL(path.resolve(path.dirname(from), spec))`; a bare name → `createRequire(from)
  .resolve(spec)`, else walk up from `from` (then `NODE_PATH`) to `node_modules/<pkg>/package.json` and take
  `exports["."]` (or `["./sub"]`, or a string) with conditions `import` → `node` → `default`, else `module`, `main`,
  `index.js`.
- **Node 18** is the floor: `isWellFormed`/`toWellFormed` (20+) have 10.4's regex fallback, `require` of ES modules needs
  20.19+/22.12+ (else 13.5's `import()` fallback), and `stripTypeScriptTypes` 22.13+ (12.5).

### 14.5 Custom bridged adapters

A custom `bridged` engine (DOCS 13.2) can port either file. The host relies on 17.6's checklist. The adapter declares in
`hello` only the caps it implements, and the host raises 17.5's defined errors for the rest (D68), so an adapter may skip
`module` (no `modules`), `op`/`release` (no `handles`), kwargs, or stubs (no `reverse`). Units keep 12.2's contract: a
fresh namespace per action run, a persistent one per definition, `exports`/`missing` in the reply. Line mapping: the
`.v2` path as filename and `line - 1` as the line offset where the language allows it (Ruby:
`eval(src, binding, file, line)`), else `line - 1` prepended blank lines; `col: null` when unknown. `v2 doctor TAG` (D34)
runs 17.7's handshake checks.

---

## 15. Compiled blocks: the build bridge

A compiled block (`@c`, `@cpp`, `@rust` and their aliases; kind Compiled, 7.2) never speaks VEP. With `@export` it is in **library mode**: built into a native library, cached under `~/.v2/cache/build`, loaded into the host with `libloading` and called through `ffi/call` (16.7). Without `@export` it is in **program mode** (15.5). Code: `build/{mod, cache, c, cpp, rust, shims}`; `mod` holds the pipeline, the per-run memo, loading, binding and `host_call`. Messages name a block with 12.1's `block_name`: `block 'fast'`, or `@rust block at main.v2:12` when it has no label.

### 15.1 Pipeline

This is 12.1's step 6: 12.1 has already checked the unit state, resolved the tag, run the directive check (step 3; 15.2's `BuildError`s) and routed the inner imports, and afterwards registers the exports and produces the value. Compiled blocks take no block args; args on a built-in compiled tag are a `SyntaxError` (4.5): `compiled blocks take no args — @export a function and call it with arguments`.
1. **Body** (4.4): the dedented text with directive lines blanked, its `UnitLoc` (2.3), the export items and the inner imports.
2. **Signatures** (15.2, 15.3): text analysis only, no toolchain.
3. **Toolchain** (7.6; target check in 15.7). None usable: `EngineNotFound`.
4. **Generation** (15.4, 15.5), then the **cache** (15.6): look the build up, build on a miss (15.7).
5. Library mode: **load and bind** (15.8). Program mode: **run** (15.5).

Steps 1–4 are memoized for the run in `Memo: HashMap<UnitId, Result<Rc<Artifact>, Rc<ErrorValue>>>`, failures included: a program block in a loop builds once and runs on every reach, and a failed build is re-raised on later reaches without rebuilding (for a definition block this is 12.1's `Failed` state: no retry, no repeated side effects). `v2 check` runs steps 1–2 only (19.3).

### 15.2 Exports and inner imports

The items are 4.3's `Exports { items, star }`: each explicit item follows its row below, and when `star` is set the `*` row adds every other function, so each function is exported once. A quoted item is a prototype that `ffi::cdecl` already parsed at parse time (an unreadable one is a `SyntaxError`, 4.3, 16.3); here it is only matched against the body. Each resolves to `struct Export { name: Rc<str>, sig: ffi::Sig, via_prototype: bool, span: Span }`. Compiled blocks export **functions only** (constant exports are py/js/ts only), so a constant's name counts as "not found":

| Item | Found, supported | Unsupported type | Several definitions | Head unreadable | Not found |
|---|---|---|---|---|---|
| `name` | exported | `BuildError` naming the parameter and its type | `BuildError: 'f' has several definitions — export one as a prototype string` | `BuildError: cannot read the signature of 'f' — give its C prototype in @export` | `ImportError: block 'fastmath' exports 'x', but does not define it as a function` |
| `*` | exported, except `main` and `static`/`inline` functions | skipped | skipped | skipped | — |

- *Several definitions*: a C++ overload set, or C/C++ definitions that differ under `#if`/`#else`. Definitions with identical lowered signatures (the usual `#ifdef _WIN32` pair) count as one.
- *Head unreadable*: a definition head naming the function exists (the name is the identifier just before the head's first depth-0 `(`), but `cdecl` rejected it: macros, templates, generics.
- Skipped functions and their reasons are kept in `Artifact.skipped`, which `v2 check` lists (19.3 step 6). A name exported twice, directly or through a prototype, is a `SyntaxError` (4.5).

**Prototype strings** (`"int mul(int a, int b)"`; in `@cpp` also `std::string` and `const std::string&`) name a function defined in the body or declared by an `#include`; a type outside 15.3's table is a `BuildError`, as for a name. The shim calls *through the prototype*, so the compiler checks that the function exists and fits:
- C calls `mul(v2_a1, v2_a2)`, so C argument conversions apply.
- C++ calls `static_cast<int(*)(int, int)>(&mul)(v2_a1, v2_a2)`, which selects an overload or instantiates a template.
- Rust maps `int`→`i32`, `long`→`::std::os::raw::c_long`, `size_t`→`usize`, a `const char*` parameter→`&str` and a `char*`/`const char*` result→`String`, then calls `mul(v2_a1, v2_a2)`.

**Inner imports.** Compiled blocks take only `@import { a, b as c } from SEL`; `{ * }` and `SEL as h` are `BuildError: compiled blocks only support @import { names } from SEL`, raised by the directive check at 12.1 step 3 when the block is reached (after tag resolution, before args and inner-import routing, so `*` is never expanded) and reported by `v2 check` (19.3), because the wrapper list must be fixed before building and compiled code cannot hold handles.
- **Library mode:** one generated wrapper per local name (15.4). The name must be a valid, non-keyword identifier of the block's language: `BuildError: 'x' is not a valid C identifier — rename it with 'as'`.
- **Program mode:** any `@import` is `BuildError: @import in a compiled block needs @export — a compiled program runs as a separate process`, raised by the directive check at 12.1 step 3, after tag resolution and before args and inner-import routing.
- **Routing:** 12.1 step 5 gives each item a route (13.6, 13.7). A target block that has not loaded yet gets a `Pending` route, resolved at the first call, so blocks may import each other in any order. `CompiledUnit.routes: HashMap<Rc<str>, RouteId>` is keyed by local name: wrappers pass the name, never a route id, because a cached library outlives the run.
- **Reserved names** (a clash is an ordinary compile error): C/C++ identifiers starting with `v2_`, `V2_` or `__v2`; the Rust module `v2` and Rust items starting with `__v2`.

### 15.3 Signature extraction

**C/C++** (`build/c.rs`). The body goes through the cdecl tokenizer (16.3); the C++ dialect adds `::`, `R"d(…)d"` and `[[…]]`. Comments are dropped, and a preprocessor line with its `\` continuations is one token. A depth-0 walk collects `head` tokens while tracking parentheses:
- A `;` at paren depth 0, or a preprocessor token, clears `head` (prototypes, globals, `#if` lines).
- A `{` at paren depth 0 ends a **definition head** when `head` has a depth-0 `(` and no depth-0 `=` (so `struct S {`, `namespace n {`, `extern "C" {` and initializers are not heads), the identifier just before its first depth-0 `(` is not a C/C++ keyword, and at least one token (a return type or specifier) comes before that identifier. A head starting with `if`, `else`, `for`, `while`, `do`, `switch`, `return`, `try` or `catch` is never a definition head (so `for (int i = 0; i < 3; i++) {`, `else if (x) {` and a range-`for` are statements, whose `{…}` is skipped like any other). A definition head goes to `cdecl::parse_definition_head`: parameter names are optional; `static`, `inline`, `template<…>` and C++ `extern "C"` are recorded; `const`, `noexcept` and attributes after the `)` are allowed.
- After every `{` (a head or not), the matching `}` is skipped by counting braces and `head` is cleared, so bodies, statement blocks and the contents of `struct`, `namespace` and `extern "C" { }` are never scanned; functions inside them need a prototype string.

**Rust** (`build/rust.rs`). A small tokenizer handles `//`, nested `/* */`, `"…"`, `r#"…"#`, byte and char literals (`'a` not followed by `'` is a lifetime) and feeds a depth-0 item walk: items start after `;` or `}`, and `#[…]`/`#![…]` groups are skipped.
- An item whose first non-qualifier token is `fn` is read as `fn NAME ( params ) [-> TYPE] {`. Each param is `IDENT: TYPE`, `mut IDENT: TYPE` or `_: TYPE`; a type is the tokens up to the next depth-0 `,` (counting `()`, `[]`, `<>`; `->` is not a bracket), joined without spaces.
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

Everything else is unsupported: other pointers, references, arrays, structs, `long double`, `wchar_t`, Rust `char`/`i128`/`Option`/`Vec`/slices, and `-> !`. A non-null parameter given `null` is `FFIValueError: argument 1 ('s', std::string) of 'reversed': null is not a string`.

### 15.4 Library mode: generated sources and shims

Generated text is **position-independent**: it never contains the `.v2` path, absolute line numbers or any runtime value, so moving a block, or editing lines above it, keeps its cache key. Positions are applied when diagnostics or panics are reported, through the block's current `UnitLoc` (2.3).
- **C/C++** (`unit.c`/`unit.cpp`): `#include "v2.h"`, `#line 1 "__v2_body"`, the body, `#line 1 "__v2_gen"`, in C++ `<string>` `<exception>` `<typeinfo>` `<cstdlib>` `<cstring>`, the shims, then `__v2_init`, `__v2_free` and `__v2_flush`.
- **Rust** (`lib.rs`): the body on lines 1..N (Rust has no `#line`, and item order is free), then one crate-root wrapper per inner import, `mod v2`, the shims and the three `__v2_*` functions. Generated Rust uses absolute paths (`::std::ffi::CString`), so user `use` lines cannot clash.

**Shims.** Each export `f` becomes `RET __v2x_f(P1 v2_a1, …, Pn v2_an, char** v2_err)` with the C ABI and the lowered types:
- **C:** a `V2_EXTERN_C V2_EXPORT` function returning `f(v2_a1, …)`; it is in the same translation unit, so `static` functions work. `V2_EXPORT` is `__declspec(dllexport)` on Windows and `__attribute__((visibility("default")))` elsewhere, where builds pass `-fvisibility=hidden`, so only `__v2x_*` and `__v2_*` are exported.
- **C++:** an `extern "C"` shim whose body is `try { … } catch (const std::exception& e) { … } catch (...) { … }`; a catch fills `*v2_err` and returns a zero value. `std::string` parameters are built from the `const char*`; `std::string` results are copied into a `malloc` buffer (one holding a NUL byte sets `V2:FFIValueError` instead).
- **Rust:** `#[no_mangle] pub extern "C" fn` (edition 2021, where plain `#[no_mangle]` is valid on every stable rustc; `#[unsafe(no_mangle)]` needs 1.82), its body inside `std::panic::catch_unwind(AssertUnwindSafe(…))`. `&str` parameters come from `CStr::from_ptr(p).to_str()` (`String` adds `.to_owned()`); `String` results leave as `CString::new(s)` then `.into_raw()` (a NUL byte gives `V2:FFIValueError`).

**Error slot.** `*v2_err` is a library-allocated string `TYPE "\n" LINE "\n" MESSAGE`:

| Source | TYPE | MESSAGE |
|---|---|---|
| C++ `std::exception` | its type, demangled with `abi::__cxa_demangle` (libstdc++/libc++), or MSVC's `type_info::name()` minus `class `/`struct ` | `e.what()` |
| C++ `catch (...)` | `unknown` | `unknown C++ exception` |
| Rust panic | `panic` | the `&str`/`String` payload, else `Box<dyn Any>` |
| returned string containing a NUL byte | `V2:FFIValueError` | the NUL's position |

`LINE` is a Rust panic's body line, or empty. The host copies the string, releases it with the library's `__v2_free`, and raises the named V2 kind or `ForeignError{lang: "cpp"|"rust", foreign_type: TYPE, message}` with the frame `at f (main.v2:L, in @rust fast)` (2.5), where L is the mapped panic line, else the block's first line. The library stays loaded and usable.

**Rust panics.** `__v2_init` installs a panic hook (`set_hook` affects only this cdylib's copy of std). While the thread is inside a shim, the hook prints nothing and records `location().line()` when `location().file()` is the generated `lib.rs`; elsewhere it prints the usual message. A user flag `-C panic=abort` defeats `catch_unwind`, and the process then aborts.

**String ownership** (DOCS 11.2). Parameters are owned by the host and valid only during the call. A `const char*` result is copied and never freed, so it must stay live (a literal or a static buffer). A `char*` result is copied, then passed to `__v2_free` (`free` of the library's own CRT), so it must come from `malloc`. `std::string`/`String` results are heap copies made by the shim, freed the same way (Rust's `__v2_free` is `drop(CString::from_raw(p))`). NULL becomes `null`; invalid UTF-8 is `FFIValueError`.

**`v2.h`** is generated per unit. Besides the code below, it includes `<stdint.h>`, `<stddef.h>`, `<stdlib.h>`, `<string.h>`, `<stdio.h>` and, in C, `<stdbool.h>`, and it defines `V2_EXPORT` and `V2_EXTERN_C`.

```c
#define V2_ABI_VERSION 1
typedef struct v2_host_api {
    uint32_t abi_version, reserved;       /* abi_version == V2_ABI_VERSION */
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

The generated tail defines `int32_t __v2_init(const v2_host_api*)` (returns 1 when `abi_version` differs, otherwise stores the pointer and returns 0), `void __v2_free(char*)`, and `void __v2_flush(void)`, which calls `fflush(NULL)` in the library's own CRT. Rust's `mod v2` mirrors this:
- a `#[repr(C)] HostApi` and an `AtomicPtr<HostApi>` set by `__v2_init`; `__v2_flush` flushes `std::io::stdout()`, ignoring errors;
- `pub fn call(name: &str, args_json: &str) -> Result<String, String>`: a NUL in either argument is `Err`, NULL becomes `Err(last_error)`, otherwise the result is copied at once;
- per import, a crate-root `fn mean(args_json: &str) -> Result<String, String> { v2::call("mean", args_json) }`.

### 15.5 Program mode (no `@export`)

A block has a `main` when 15.3's scan finds a top-level function named `main` (a C/C++ definition head, or Rust `fn main`); it is then compiled as is (C/C++ behind `#line 1 "__v2_body"`). A block without `main` may contain statements only: if the scan finds any other top-level function definition (a 15.3 definition head; braced `if`/`for`/`while`/`switch` statements are not), it is `BuildError: @c block at main.v2:12 defines functions but no main — write main yourself to use helper functions`. Otherwise V2 wraps it:
- **C/C++:** `#include <stdio.h>`, `<stdlib.h>`, `<string.h>`; each top-level `#include` line of the body, preceded by `#line K "__v2_body"`; `int main(void) {` and `#line 1 "__v2_body"`; the body with those `#include` lines blanked; `return 0; }`.
- **Rust:** `fn main() {`, the body, `}`; the line map is "generated line − 1". `use` declarations are legal inside a function body, which matches keeping `use` lines at the top (DOCS 11.4).

The executable runs from the cache entry through the run-engine process helper (17.3 steps 3–5 and 7), under 8.3's child rules:
- stdin is null (DOCS 4.6); the working directory and environment are the host's.
- **Statement position:** stdout streams to V2's stdout in program order, after the host has flushed its own stdout and all dirty native stdio (16.7). **Expression position:** stdout is captured and becomes the value, with one trailing `\n` or `\r\n` removed, decoded as in 17.3.
- stderr streams live to the host's stderr, and the last 4 KiB is kept. The engine's `call_timeout` applies (watchdog, `EngineTimeout`, 8.5).
- The program and everything it started are killed as a tree (8.3 `kill_tree`) when the block finishes, times out or V2 exits.

A non-zero status raises `EngineExitError` exactly as for a run engine (17.3): `@c block exited with status 3`, the signal and hex-NTSTATUS forms, and `data = {status, signal, stderr}` holding the kept tail, which the message does not repeat.

### 15.6 The build cache

The key is `k = hex(sha256(serde_json::to_string(&key)))` over a struct, so the field order is fixed and there is no whitespace:

```json
{"v2abi":1,"engine":"rust","mode":"lib",
 "compiler":{"kind":"rustc","path":"C:/Users/me/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin/rustc.exe",
             "version":"rustc 1.91.1 (ed61e7d7e 2025-11-07)","target":"x86_64-pc-windows-msvc"},
 "flags":["--crate-type","cdylib","--edition","2021","-C","opt-level=2","-C","target-feature=+crt-static",
          "--crate-name","v2block","--error-format=json","{src}","-C","opt-level=3","-o","{out}"],
 "prelude":"<sha256 of all generated text except the body>","source":"<dedented body>"}
```

`path` is 7.6's resolved `argv[0]` (for Rust the sysroot's `rustc`, not the rustup proxy); `version` is the first line of its version output; `target` comes from 7.6. `flags` is the exact argv including user flags, with the input and output paths replaced by `{src}`/`{out}`. `prelude` makes a generator change rebuild. The entry directory `~/.v2/cache/build/<k[0..2]>/<k>/` (created 0700 on Unix) holds:
- the artifact: `v2_<k[0..16]>.dll|.so|.dylib`, or `prog[.exe]`, plus any load copies `v2_<k[0..16]>_<n>.<ext>` (15.8);
- `exports.json`: `{"v2abi", "mode", "exports": [{"name", "symbol", "params", "ret"}], "imports"}`;
- `build.log` (the argv, the full compiler output and any warnings) and `src/` (the generated sources).

Lookup and build:
1. Unless `--no-cache` is given, it is a hit if the artifact exists and `exports.json` matches the current exports.
2. On a miss, build in `<run>/build/<k>/` (20.3). Copy the outputs into `<k[0..2]>/.tmp-<k>-<pid>-<rand>/`, on the entry's volume, and rename that to `<k>`; if `<k>` appeared in the meantime, keep it and delete ours. Failed builds are never cached.

There is no lock: an entry appears only through one atomic rename, so concurrent builders may both compile, and the first rename wins.
- **`--no-cache`** rebuilds and replaces: step 2 first renames an existing `<k>` to `.old-<k>-<rand>` and deletes it. On Windows that rename fails while another process has the DLL loaded; V2 then installs the new build as `.fresh-<k>-<pid>` in the same `<k[0..2]>` dir, loads it from there, and leaves the old entry alone.
- **Sweeping.** `.tmp-*`, `.old-*` and `.fresh-*` dirs older than a day are deleted, best effort, at the start of every build in their `<k[0..2]>` dir. `v2 cache clean` (19.4) deletes entries one by one and reports those still in use.
- **No usable `~/.v2`** (18.1): libraries load from `<run>/build/` and nothing is cached; `check` and `doctor` warn.

### 15.7 Toolchains, command lines, diagnostics

Resolution (7.2, 7.6) picks the compiler — on Windows MSVC `cl` first (from a Developer Prompt environment, else via vswhere with the captured vcvars environment), then gcc, then clang; elsewhere `cc`, `gcc`, `clang` for C and `c++`, `g++`, `clang++` for C++; `[engines.<tag>] command` overrides — and hands over 7.1's `ResolvedEngine`: `family` (Gcc, Clang, Msvc, Rustc), `argv`, `version`, `target` (a triple, or cl's `x64`/`arm64`) and `env` (cl's `INCLUDE`, `LIB`, `LIBPATH` and `PATH` prefix).

**Target check** (part of 7.6's probe). The arch (normalized: `amd64`/`x64` → `x86_64`, `arm64` → `aarch64`) must equal `std::env::consts::ARCH`, and the OS must match: on Windows cl as is, gcc and clang only with a MinGW target (`*-w64-mingw32`, `*-windows-gnu`), rustc with `*-windows-msvc` or `*-windows-gnu*` (a clang targeting `*-windows-msvc` is skipped as `clang targets MSVC — use cl`; `*-cygwin` and `*-msys` are rejected, because their DLLs need `msys-2.0.dll`); on Linux `*-linux-*`; on macOS `*-apple-darwin*`. Auto-detected candidates that fail are skipped; a configured command that fails is `EngineNotFound: gcc at C:\mingw32\bin\gcc.exe targets i686, but v2 is x86_64 — install a 64-bit toolchain` (exit 3 if uncaught).

**Process.** Compilers run in `<run>/build/<k>/` with relative file names and null stdin, under 8.3's child rules; stdout and stderr are read concurrently (8.3) and both kept (cl writes diagnostics to stdout). The compiler's own folder is prepended to `PATH` (MSYS2's `cc1.exe` loads DLLs from there), cl gets `env`, and diagnostics are forced to English with `LC_ALL=C` (gcc/clang) or `VSLANG=1033` (cl). A compiler that cannot be spawned is `EngineNotFound: could not start '<path>': <os error>` (8.3). In the table, `OUT` is `v2_<k[0..16]>` and `FLAGS` are the user's `[engines.<tag>] flags`:

| Toolchain | Library mode | Program mode |
|---|---|---|
| gcc/clang, Linux | `-std=gnu11 -O2 -shared -fPIC -fvisibility=hidden -Wl,--no-undefined unit.c FLAGS -o OUT.so -lm` | `-std=gnu11 -O2 unit.c FLAGS -o prog -lm` |
| clang, macOS | `-std=gnu11 -O2 -dynamiclib -fvisibility=hidden unit.c FLAGS -o OUT.dylib` | `-std=gnu11 -O2 unit.c FLAGS -o prog` |
| gcc/clang, MinGW | `-std=gnu11 -O2 -shared unit.c FLAGS -o OUT.dll -static` | `-std=gnu11 -O2 unit.c FLAGS -o prog.exe -static` |
| g++/clang++ | the C row with `-std=gnu++17`, `unit.cpp`, no `-lm` | same |
| cl | `/nologo /LD /O2 /utf-8 /std:c11 /TC /Fe:OUT.dll unit.c FLAGS`; C++: `/std:c++17 /EHs /TP` and `unit.cpp` | no `/LD`; `/Fe:prog.exe` |
| rustc | `--crate-type cdylib --edition 2021 -C opt-level=2 [-C target-feature=+crt-static] --crate-name v2block --error-format=json lib.rs FLAGS -o OUT.<ext>` | `--crate-type bin`, `main.rs`, `-o prog[.exe]` |

- User flags come after V2's own flags, so they win (`-O3`, `-C opt-level=3`), and after the source, so `-lfoo` links. For cl, FLAGS come last because a `/link …` part must end the command line.
- `-Wl,--no-undefined` turns a missing symbol into a link error instead of a later `dlopen` failure.
- On MinGW, `-static` links libgcc, libstdc++ and winpthread into the artifact (`-static-libgcc -static-libstdc++` alone would leave a `libwinpthread-1.dll` import); system DLLs stay dynamic. A user DLL is linked through its import library with `-Wl,-Bdynamic -lfoo -Wl,-Bstatic`; at load it is found by 16.2's dependency search (the cache folder, then the system order including `PATH`), not in the `.v2` folder.
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

1. **Load** by absolute path as a full-path library (16.2): Windows `LoadLibraryExW(path, NULL, LOAD_WITH_ALTERED_SEARCH_PATH)`, Unix `dlopen(RTLD_NOW | RTLD_LOCAL)`. If another unit already loaded this artifact in this run (two blocks with identical text), V2 loads the byte copy `v2_<k[0..16]>_<n>.<ext>` from the same entry (written through a temp name and a rename if absent; the bytes are identical, so an existing copy is reused): a distinct file is a distinct module, so each block keeps its own `static`s (DOCS 11.3) and host-API token. A load failure is `FFILoadError` with the OS text (16.2).
2. **Resolve** `__v2_init`, `__v2_free`, `__v2_flush` and every `__v2x_<name>`. If one is missing, evict the entry and rebuild once; if it is still missing, raise `FFISymbolError`.
3. **Init.** `Box::leak` a `HostApi{abi_version: 1, unit: token, call: host_call, last_error: host_last_error}` and call `__v2_init`; a non-zero result is `BuildError: library ABI mismatch`. `token` indexes `Vec<CompiledUnit{routes, loc: UnitLoc, name, active: Cell<u32>}>`.
4. **Bind** each export as `Value::Func(Rc<Callable::Native{lib, sym, sig}>)` (5.1) with `sig.err_slot = true`, registered like a bridged export (13.2). Calls take positional arguments only (5.7).

Libraries are never unloaded. Because `lib.compiled` is `Some`, the call path (16.7) adds the error slot, `ACTIVE` and the unit's `active` count (15.9), the library's flush target and `__v2_free` for `OwnedStr` results.

**MinGW libraries inside an MSVC-built v2** are safe because the two share only the x64 C ABI: scalars and pointers follow the Win64 convention; memory never crosses allocators (the host frees only its own argument copies and reverse-call results, and library strings go back through that library's `__v2_free`); stdio buffers are per CRT, hence the per-library `__v2_flush`; exceptions and panics never leave the shim.

### 15.9 Reverse calls (`host_call`)

`extern "C" fn host_call(unit, route, args_json) -> *const c_char` is wrapped whole in `catch_unwind` and runs these steps:
1. **Guards** (DOCS 8.4). V2 may be called only from the thread that called into this library, while such a call is running: the thread-local `ACTIVE: Cell<*mut Interp>` must be non-null (it is null on another thread, and once the V2→native call has returned: an atexit handler, a stored pointer) and `units[unit].active` must be non-zero. Otherwise set the error `V2 can only be called from the thread that called into this library, while that call is running` and return NULL. If `PENDING_EXIT` is set, return NULL with `exit() was called`.
2. Find `route` in `units[unit].routes`; if it is missing: `'x' is not imported by this block`.
3. Decode `args_json` in `Mode::Plain` (10.1), NULL meaning `[]`: invalid JSON is `MarshalError: args_json is not valid JSON: <detail>`, a non-array `MarshalError: args_json must be a JSON array`; `"$v2"` is an ordinary key.
4. Call the route as 13.6 does for `call {route}` (a `Pending` route resolves now or fails with its ImportError), with the span of the enclosing native call. This counts one level of 9.7's cross-language depth limit (64). No flush is needed first: the library is already dirty (16.7), and any V2 output flushes it.
5. On `Ok(v)`, encode `v` in Plain mode; bytes, functions, handles, errors and non-finite floats fail as 10.1 says (`MarshalError: a handle cannot be returned to compiled code — convert it with value()`) and go to step 6. Store the text in the thread-local `LAST_RESULT: Option<CString>`, replacing the previous one, and return its pointer; it stays valid until the next V2 call on this thread (DOCS 11.6).
6. On `Err(e)`, set `LAST_ERROR = "Type: message"` (e.g. `ForeignError: ZeroDivisionError: division by zero`) and return NULL. `exit(code)` also sets `PENDING_EXIT`, and a Rust panic inside the interpreter sets `PENDING_PANIC`.

`host_last_error` returns `LAST_ERROR`, or `""`. After `Cif::call` returns, `call_native` re-raises a pending exit (the interpreter's exit signal, 6.5; no `finally` runs) or panic (`resume_unwind`, exit 101). A `host_call` further out turns it into NULL again, so neither crosses C frames and both reach the top.

**Why this is sound.** The interpreter is single-threaded. Immediately before `Cif::call`, `call_native` saves `ACTIVE` and sets it to a raw pointer derived from its own `&mut Interp`, restoring it right after (a stack for nested calls). By then all arguments are in owned storage, every `RefCell` borrow is released and the callee's `Rc` is cloned, so no reference into interpreter state lives across the call, and the caller's `&mut Interp` stays unused until `Cif::call` returns. So the `&mut *ACTIVE` created in `host_call` is the only live access to interpreter state, and it ends before `host_call` returns. Other threads read null and never touch the interpreter.

---

## 16. Native FFI layer

`extern` binds functions of an existing native library, described with C declarations, and calls them through libffi. Code: `ffi/cdecl.rs` (the declaration parser, shared with 15.3), `ffi/types.rs` (`CType` and platform sizes), `ffi/lib.rs` (library resolution and loading) and `ffi/call.rs` (the call path, shared with compiled blocks). Only the 64-bit targets of ch. 1 are supported: `ffi/types.rs` has `#[cfg(not(target_pointer_width = "64"))] compile_error!`.

### 16.1 The `extern` statement

The lexer raw-captures the body with the CLike(C) profile (3.9); the parser runs `cdecl::parse_extern` at once, filling 4.3's `Extern.decls`, and lowers each declaration to a `Sig` (16.4). So every declaration error, including a function declared twice in one body, is a `SyntaxError` at the exact token (2.4), reported before anything runs and by `v2 check`. Each time the statement is reached:
1. Resolve and load the library (16.2), cached per (declaring file, spec) for the run.
2. Look up every symbol (cached per library and name). All missing names go into one error: `FFISymbolError: not found in library 'm' (/lib/x86_64-linux-gnu/libm.so.6): cosf128, foo`. On any failure nothing is bound.
3. Make one `Callable::Native{lib, sym, sig}` (5.1) per declaration (`lib.compiled` is `None`), cached per statement node.
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
1. **Directories**, in order: each `[ffi] search_paths` entry (resolved per 18.1), the `.v2` file's folder, then the project root if there is one and it differs. The first `D/candidate` that is a file is loaded by full path, and the search **stops**: if that load fails, that failure is the error, so a present but broken library (e.g. a 32-bit one) is never silently replaced by another. On Linux, when no exact candidate exists in D, the highest-versioned `libN.so.<digits>[.<digits>…]` in D matches too.
2. **System default search.**
   - **Windows:** `LoadLibraryExW(candidate, NULL, 0)`: the standard order (the v2.exe folder, System32, the Windows folders, then `PATH`) without the current directory, which `main` removes at startup with `SetDllDirectoryW(L"")` (20.2).
   - **Linux:** `dlopen(candidate)` (`LD_LIBRARY_PATH`, ld.so.cache, the default dirs), then the versioned match over `/lib/<triple>`, `/usr/lib/<triple>`, `/lib64`, `/usr/lib64`, `/lib`, `/usr/lib` and `/usr/local/lib`, so `sqlite3` finds `libsqlite3.so.0` without the `-dev` symlink.
   - **macOS:** `dlopen` of `/opt/homebrew/lib/<c>`, `/usr/local/lib/<c>` and `/usr/lib/<c>`, without existence checks, since system libraries live in the dyld shared cache. Bare-name `dlopen` is never used there: it searches the current directory.

Paths are made absolute with `std::path::absolute`, not `canonicalize` (which adds a `\\?\` prefix on Windows). Full-path loads on Windows use `LoadLibraryExW(path, NULL, LOAD_WITH_ALTERED_SEARCH_PATH)`, so dependencies are searched in the library's own folder first, then in the system order above; every Windows load runs under `SetThreadErrorMode(SEM_FAILCRITICALERRORS)` (libloading does this), so a bad image never opens a dialog. Unix loads use `RTLD_NOW | RTLD_LOCAL`, so missing dependencies fail at load time. Every attempt is recorded and listed on failure:

```text
FFILoadError: cannot load library 'sqlite3'
  C:\proj\native\bin\sqlite3.dll: not found
  C:\proj\sqlite3.dll: not found
  sqlite3.dll (system): The specified module could not be found. (os error 126)
  libsqlite3.dll (system): The specified module could not be found. (os error 126)
  hint: put the library next to the script or add its folder to [ffi] search_paths
```

On Windows, error 126 for a file that exists is reworded `found, but a library it depends on could not be loaded (os error 126)`, and error 193 `not a library for this machine — 32-bit or another architecture? (os error 193)`. On Unix the `dlerror()` text is kept as is (e.g. `wrong ELF class: ELFCLASS32`).

Loaded libraries (`LIBS: HashMap<LibKey, Rc<LoadedLib>>`, keyed by absolute path or `system:<candidate>`) are **never unloaded**: function values outlive scopes, and libraries hold atexit handlers and thread-locals. `v2 check` walks step 1 as existence checks only and never loads anything, because loading runs library initializers (19.3 step 5).

### 16.3 The C declaration parser (`ffi/cdecl`)

Three callers: `extern` bodies (a list of prototypes) and `@export` prototype strings (one prototype, `;` optional), both at parse time (4.3), and compiled C/C++ definition heads at build time (15.3: parameter names optional; `static`, `inline` and `template` recorded). The C++ dialect adds `std::string`, `const std::string&`, `noexcept`, `[[…]]`, `extern "C"` and `::`. The tokenizer yields identifiers, integers, punctuation, `...` and literals; it skips comments and returns a preprocessor line as one token.

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
callconv   = "__cdecl" | "_cdecl" | "__stdcall" | "_stdcall" | "WINAPI" | "WINAPIV" | "APIENTRY" | "CALLBACK"
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

Errors are `SyntaxError`s in `extern` bodies and prototype strings; in a definition head the same rejection makes the head unreadable (15.2):

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
                 pub params: Vec<Param>, pub err_slot: bool, pub cif: OnceCell<Rc<Cif>> }
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

Arity is checked first: `ArityError: cos() takes 1 argument (2 given)`; named arguments are `ArityError: cos() takes positional arguments only` (5.7). An engine handle anywhere in the arguments is a `MarshalError`, as for any other engine (11.7): `MarshalError: cannot pass a py handle (datetime.date) to native code; convert it with value(h) first`. A conversion failure is `FFIValueError: argument 2 ('exponent', double) of 'pow': <reason>` (an unnamed parameter omits the name).

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
| `Int` | `int`; a `uint64_t`/`size_t` above `i64::MAX` is `FFIValueError: return value 18446744073709551615 of 'f' does not fit in a V2 int` (never wrapped) |
| `F32`, `F64` | `float` (F32 widened) |
| `Str` | NULL → `null`; otherwise the bytes up to NUL, copied into a `str`; invalid UTF-8 → `FFIValueError`; never freed |
| `OwnedStr` | as `Str`, then the library's `__v2_free(p)` (also on the UTF-8 error path) |
| `Ptr` | NULL → `null`; otherwise a native pointer handle |

A **native pointer handle** is a `Value::Handle(Rc<HandleRef>)` whose session is the reserved `SessionId::NATIVE`; no worker owns it, so nothing of ch. 11 that needs a worker (ops, releases) ever reaches it. Its `id` is the address, its `type_name` the declared pointee (`sqlite3*`), its repr `<native sqlite3* 0x000001d2c3e4f5a0>`, and `type(p)` is `"handle"`.
- `==` compares addresses only. NULL is always `null`, so native handles stay truthy.
- Dropping a handle sends nothing: V2 never frees native memory.
- `.attr`, calls, indexing, `len` and iteration raise `TypeError: native pointers support no operations — pass them to extern functions`; `value(h)`, and passing a native handle to py or js code, are `MarshalError`.
- Any native handle fits any `Ptr` parameter. There is no pointee check, because `void*` round trips need that.

### 16.7 The call path (`ffi/call.rs`)

The libffi types are: `Void` → void, `Bool` → u8, `Int` → i8…u64 by size and sign, `F32`/`F64` → f32/f64, and pointer for `Str`, `OwnedStr`, `Ptr` and the error slot. `Cif`s are cached in `CIFS: HashMap<(Vec<FfiKind>, FfiKind), Rc<Cif>>`, so `int f(int)` and `int32_t g(int32_t)` share one; the lookup is memoized in `Sig.cif`.

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

In-process calls have no timeout (`call_timeout` cannot interrupt a thread running native code) and no crash guard: a fault ends the process with the OS status (DOCS 11.7, 16). Ctrl+C during a long native call ends the process with 130 through 6.5's hard path. A library that calls `exit()` ends the process without V2's shutdown; workers then see EOF on stdin, and the kill-on-close Job Objects or `PR_SET_PDEATHSIG` end the other children (8.3).

---

## 17. Custom engines

A custom engine is any engine that isn't built in. It comes from an `[engines.<tag>]` table in `~/.v2/config.toml` or `v2.toml` (18.2), or from `register_engine(tag, spec)` at run time, and has one of two kinds:

- **`run`** (`engines/runonly`): writes the block to a temp file, runs a command and returns its stdout (DOCS 13.1).
- **`bridged`**: an adapter process that speaks VEP/1 through the same `engines/session` code as `py` and `js` (DOCS 13.2).

Every custom tag is scanned with the Plain profile (3.8): braces are counted and foreign strings and comments are not understood, so code with unbalanced braces needs the fence form. Profiles come from the built-in table only; lexing never reads config (ch. 0).

### 17.1 Specs and validation

A custom spec is a 7.1 `EngineSpec` with `builtin: false`, profile `Plain`, no candidates and no minimum version, plus its `command`, its timeouts, the `base_dir` used for path rewriting and an origin (`UserConfig(path) | ProjectConfig(path) | Runtime(span)`) that messages and doctor print. TOML tables (the table of the winning layer, 18.3) and `register_engine` dicts go through one validator, `engines/spec::validate`:

| Key | Kinds | Rule | Default |
| --- | --- | --- | --- |
| `kind` | both | `"run"` or `"bridged"` | required |
| `command` | both | non-empty list of non-empty strings (in TOML a string is one element, 18.2). `run`: some element contains `{file}`; `bridged`: none may | required |
| `ext` | run | `[A-Za-z0-9_]+` after dropping one leading `.` | the tag |
| `aliases` | both | each a valid tag; no repeats; none equal to the tag | `[]` |
| `startup_timeout` | bridged | seconds (int or float), finite, `>= 0`; 0 = no limit | 30 |
| `call_timeout` | both | as `startup_timeout` | 0 |

- **Tags and aliases** match `[a-z][a-z0-9_]{0,31}`; `v2`, `export` and `import` are reserved.
- **`register_engine`:** a tag that isn't a `str`, or a spec that isn't a `dict`, is TypeError. Every other problem is ValueError `engine '{tag}': {problem}` (7.4), e.g. `command must contain {file}`, `invalid engine tag '{t}'`, `'{t}' is reserved`, `unknown key '{k}' (did you mean '{m}'?)`, `'{k}' does not apply to {kind} engines`, `'{k}' must be {expected}, got {type}`.
- **Config:** the same problems are configuration errors in 18.5's format (exit 2, before anything runs), except unknown keys, which are warnings (18.5).

**Command paths.** `base_dir` is the folder of the declaring config file, or of the `.v2` file that called `register_engine` (18.1).

- A `command[0]` without a separator is found by V2's PATH walk (7.5), which skips relative entries such as `.`.
- Every relative element that looks like a path (contains `/` or `\`, doesn't start with `-`, contains no `{`) is resolved against `base_dir`, so `["ruby", "adapters/v2_adapter.rb"]` works from any working directory.
- Nothing else is rewritten: there is no shell, `{file}` is the only placeholder, and `~` and `$VAR` are not expanded.
- `V2_ENGINE_<TAG>` and `--engine TAG=PATH` replace `command[0]` only (18.4).

### 17.2 Registration

The registry (7.1) maps every tag and alias to its spec; aliases canonicalize everywhere, as built-in ones do (7.3).

**From config, at startup:**

- A table named after a built-in tag or alias is an override table (18.2); `kind` or `aliases` in it is the config error `built-in engine '{tag}' can't change 'kind' or 'aliases'`. Every other table defines a custom engine; its table comes whole from the highest layer that declares it, with no key-by-key merge (18.3); `V2_ENGINE_<TAG>`/`--engine` then replace `command[0]` (18.4).
- A name that two tables claim, as tag or alias, is the config error `engine name '{n}' is defined by both [engines.{a}] and [engines.{b}]`.

**`register_engine(tag, spec)`:**

1. Validate the spec into `S` (17.1); its origin is the call span.
2. Check every name `N` of `S` (tag and aliases) before changing anything. It is EngineNameConflict when `N` is:
   - a built-in tag or alias: `'{N}' is a built-in engine tag; configure it with [engines.{builtin}]`;
   - declared in a config file, even with an identical spec: `engine tag '{N}' is already registered ({path})`;
   - owned by an earlier registration `E` that differs from `S` (another tag, or a different kind, command after rewriting, ext, alias set or timeouts): `engine tag '{N}' is already registered ({E.origin}) with a different spec`.
3. Insert the new names, apply any pending `V2_ENGINE_<TAG>`/`--engine` override (18.4) and return `null`. An identical re-registration inserts nothing. Nothing is resolved or started.

A registration lasts for the rest of the run, applies in every module, is never persisted, and affects only blocks reached afterwards (an earlier reach already raised EngineUnknown, 7.3). `engine_available(tag)` is `false` for an unknown tag and, for a custom tag, `true` when `command[0]` resolves to an executable file; it never spawns anything. `v2 check` knows the tags declared in config and those passed as string literals to `register_engine` (shown `runtime`, 19.3).

### 17.3 Run engines

A reached run-engine block goes through 12.1: tag resolution, the step 3 check (a block with `@export` or an inner `@import` raises ImportError `@{tag} blocks cannot @export or @import — '{tag}' is a run engine` and does not run), and args left to right. Then:

1. **Source.** The processed body (4.4: dedented, directive lines blanked, `@@export`/`@@import` unescaped) with `\n` line endings, UTF-8 without BOM, and nothing added. Line *k* of the file is `.v2` line `body_first_line + k − 1` (2.3). `bat`/`cmd` files get `\r\n` instead (D67; cmd.exe's label search misbehaves in LF files), which moves no lines.
2. **File.** Create `<run>/run/{tag}-{n}.{ext}` with `create_new` and mode 0600 (20.3), write it, close it, and replace every `{file}` in `command` with its absolute path. The source reaches the command only through `{file}`; there is no stdin mode.
3. **Spawn**, under the child rules of 8.3 (own process group or Job Object), after flushing host stdout and dirty native stdio (16.7). The working directory and environment are inherited, plus 17.4.
   - **stdin** is the null device: `input()` owns user stdin (DOCS 4.6).
   - **stdout** is inherited in statement position, which keeps program order, and piped and read by a second helper thread in expression position.
   - **stderr** is piped; a helper thread copies each chunk to host stderr at once and keeps the last 4096 bytes.
   - A spawn failure is EngineNotFound `could not start '{path}': {os error}` (8.3).
4. **Timeout.** With `call_timeout` set, a `Call` timer (8.5) kills the process tree at the deadline and raises EngineTimeout `@{tag} block ran longer than call_timeout ({n}s) and was stopped`. `startup_timeout` doesn't apply.
5. **Finish.** Observe the exit without reaping, `kill_tree`, join the reader threads, then reap (8.3). Background processes the block started therefore stop with it (DOCS 13.1) and can't hold stdout open; only a process that detached itself (`setsid`, `CREATE_BREAKAWAY_FROM_JOB`) escapes.
6. **Cleanup.** Delete the temp file on every path, ignoring errors.
7. **Result.** A non-zero status raises EngineExitError. Otherwise a statement gives `null`, and an expression gives the collected bytes minus one trailing `\n` or `\r\n`, decoded as strict UTF-8 (else MarshalError `output of @{tag} block is not valid UTF-8 (byte {k})`).

**EngineExitError** (shared with compiled programs, 15.5):

- **Message:** `@{tag} block exited with status {code}`; Windows statuses `>= 0x80000000` print in hex (`0xC0000005`). A POSIX signal gives `@{tag} block was killed by signal {n} ({SIGNAME})`.
- **`lang`** is the tag; **`data`** is `{status, signal, stderr}`. `stderr` is the kept tail, decoded lossily; when more than 4096 bytes were written it starts after the first line break inside the window. The message doesn't repeat it, because it has already streamed live.

**Traps.** Scripts that resolve paths from their own location (`require("./x")`, `$(dirname "$0")`) see the run dir, not the project. On Windows a bare `bash` usually resolves to `C:\Windows\System32\bash.exe`, the WSL launcher, which can't open the Windows temp path; configure Git Bash's full path (DOCS 13.1). Doctor check RUN2 exposes this.

### 17.4 Block args as environment variables

The child's environment is the inherited one, minus every `V2_ARG_*` variable (matched case-insensitively on Windows, so a nested `v2` can't leak its args), plus one `V2_ARG_<name>` per arg, named as written: `@sh(target: x)` sets `V2_ARG_target`. A `str` passes unchanged; `null`, `bool`, `int`, `float`, `list` and `dict` pass as compact `json_stringify` text (`null`, `42`, `1.0`, `[1,2]`, `{"a":1}`).

| Problem | Error |
| --- | --- |
| name not `[A-Za-z_][A-Za-z0-9_]*` | ValueError `block arg '{name}' can't be used as an environment variable name` |
| two names equal ignoring ASCII case (checked on every OS) | ValueError `block args '{a}' and '{b}' differ only in case; run engines pass args as environment variables, which are case-insensitive on Windows` |
| `bytes`, `func`, `handle` or `error` at any depth | MarshalError `block arg '{name}': a {type} can't be passed to a run engine` |
| NaN or ±inf at any depth | MarshalError `block arg '{name}': NaN and infinity have no JSON form` |
| a NUL character in a `str` | MarshalError `block arg '{name}' contains a NUL character` |
| `V2_ARG_{name}={value}` over 32767 bytes (the Windows per-variable limit, applied on every OS) | ValueError `block arg '{name}' is too large for an environment variable ({n} bytes, limit 32767)` |

### 17.5 Bridged engines

A bridged engine runs exactly like `py` and `js` (ch. 8, 9, 11, 13): one worker per tag per run, started lazily, with argv from the spec, the inherited working directory, environment and stderr, and stdin/stdout as the VEP pipes. Units, selectors, handles, re-entrancy, timeouts, crashes and line mapping behave as for built-in engines. The differences: nothing is written to the run dir, no environment variables are added, and the host gates features on the `caps` of `hello`, which must arrive within `startup_timeout` with `vep: 1` and `caps` a list of strings (8.4, 9.3; unknown caps are ignored). The built-in py adapter declares all four caps; js declares all but `kwargs`.

| Cap | The adapter supports | Without it, the host |
| --- | --- | --- |
| `modules` | `module` frames: `@import {…} from {tag}.name`, `@import {tag}.name as m` | raises ImportError `engine '{tag}' cannot import packages` and sends nothing |
| `handles` | `h` tags, `op` and `release` | never sends `op` or `release`; an `h` tag from the adapter is ProtocolError, so the adapter answers an unmappable result with `err` MarshalError |
| `kwargs` | named args in `call` | raises ArityError `{tag} functions take positional arguments only (got '{kw}')` and sends nothing |
| `reverse` | adapter-sent `call` frames: inner `@import`, V2 functions passed as args | raises ImportError `@import inside @{tag} blocks needs the 'reverse' capability` before `load`, and MarshalError `engine '{tag}' can't receive V2 functions (no 'reverse' capability)` for function args; a `call` frame from the adapter is ProtocolError |

### 17.6 Adapter checklist

What the host relies on (frame shapes: ch. 9; porting a built-in adapter: 14.5):

1. **Framing.** One compact JSON object per line, `"t"` first, UTF-8, every control character escaped (use a real JSON encoder), ending in `\n`, written in binary mode; the host drops one `\r` before the `\n`. Split input on the `\n` byte only: never universal newlines, a `readline` that breaks on `\r`, or `splitlines`. Frames are at most 64 MiB in each direction; answer `err` MarshalError instead of sending a larger one.
2. **Stdout is the protocol.** Before `hello` the host forwards non-JSON lines (interpreter startup noise) to stderr; after `hello` any line that isn't a frame is a fatal ProtocolError.
3. **Replies.** Send `hello` before running user code. Answer every `init`, `load`, `module`, `call` and `op` with exactly one `ret` or `err` echoing its `id`, innermost first (9.7); `release` and `bye` get no reply. An unknown `t` gets `err` ProtocolError and the adapter keeps running. Catch every exception per request, so user code can't end the worker.
4. **User I/O.** Before user code runs, move the protocol to a private non-inheritable descriptor, point fd 0 at the null device and fd 1 at fd 2 (where the language can't, keep user code off the protocol descriptors as 9.9 does for Node), and replace the language's stdin with one that raises and its stdout with a writer that makes `out` frames. Language-level output then reaches the host as `out` frames before the `ret`/`err` of the request that produced it; fd-level output and inheriting child processes land on stderr (DOCS 4.6). Output produced while idle is buffered and sent before the next frame (9.9).
5. **Loading.** Each unit gets a fresh namespace with the block args bound. An action's `ret` carries its last expression's value (or `null`) with empty `exports` and `missing`; a definition reports undefined exports in `missing` (9.4) and keeps its namespace until `bye`.
6. **Calls.** Look the target up by exact name in the unit's export table; never evaluate a name. A foreign exception becomes `err` with its foreign type, its message and a trace in `.v2` positions (12.6).
7. **Values.** Encode per ch. 10. Never stringify an unmappable value: send a handle (with `handles`) or `err` MarshalError.
8. **Re-entrancy (with `reverse`).** Serve incoming requests while waiting for your own reply. Call into V2 only while a host request to this adapter is executing, and only from the thread it arrived on; otherwise raise an error in the foreign language (DOCS 8.4, 9.8). Stored callbacks stay valid for later requests. The host can't enforce this rule, because it reads a worker's frames only while a request to that worker is pending.
9. **Releases.** Finalizers only queue releases; the queue goes out just before the next outgoing frame (9.10).
10. **Shutdown.** On `bye`, even while waiting for a reply, flush pending output and exit 0 within 1 s (9.10); on stdin EOF, exit quietly.

### 17.7 `v2 doctor TAG` for custom engines

This is 19.2's deep test for a custom tag. Doctor prints one line per check (`PASS` or `FAIL`, the check id, the check text; a failure adds the offending bytes or the stderr tail), then 19.2's last line, `{tag}: ok` or `{tag}: FAILED: <error>`. It exits 0 if every check passes, 1 on any FAIL, 3 if `command[0]` isn't found, and 2 for an unknown tag (19.2). BR2 is limited by `startup_timeout` and every other step by 10 s. After `hello`, a stdout line that isn't a frame fails the current step; noise before `hello` is listed, not failed.

| Id | Check |
| --- | --- |
| RUN1 | the spec is valid (its origin is printed) and `command[0]` resolves (its path is printed) |
| RUN2 | an empty block, run as an empty `<run>/probe.{ext}`, exits 0 with empty stdout |
| BR1 | `command[0]` resolves |
| BR2 | the first frame is a JSON object with `t = "hello"`; its fields and the handshake time are printed |
| BR3 | `vep == 1` and `caps` is a list of strings; unknown caps are listed but don't fail |
| BR4 | `init` gets a `ret` with the same `id` |
| BR5 | loading an empty action unit gets a `ret` with `value` `null` and empty `exports` and `missing` |
| BR6 | calling `__v2_doctor_missing` in that unit gets an `err` with a non-empty type and message, and the adapter stays alive |
| BR7 | a request with `t = "x-doctor"` gets an `err` ProtocolError echoing its `id` |
| BR8 | with `handles` only: a `release` of an unknown id gets no reply, and BR5 still passes afterwards |
| BR9 | `bye` makes the adapter exit 0 within 1 s |

Doctor can't exercise exports, callbacks or handles, which need code in the adapter's own language. Adapter authors cover checklist items 4–9 with their own `.v2` tests; the V2 repository covers the host side with `tests/custom` and the fake adapter (22.3).

---

## 18. Configuration

Module `config/`. Config is loaded before any script is parsed; the lexer and parser never read it (D3).

### 18.1 Files, project root and relative paths

- **Files:** `~/.v2/config.toml` (user defaults) and the project's `v2.toml`; a missing file is an empty layer. `~` is `%USERPROFILE%` on Windows and `$HOME` elsewhere, read from exactly these variables (22.2). With no home folder the user layer is skipped and the `~/.v2` caches are off (7.7, 15.6); check and doctor warn.
- **No ownership checks** (D70): V2 never refuses `v2.toml`, `config.toml` or `~/.v2` because of owner or permissions; it is a same-user tool (20.1).
- **Project root:** start at the entry file's folder (`v2 FILE`, `v2 check FILE`) or the cwd (`v2 run`, `v2 doctor`), made absolute with `.` and `..` applied to the text (as Windows does; POSIX `std::path::absolute` keeps `..`, and walking up `/proj/../other` would visit `/proj`), and walk up; the first folder holding a regular file `v2.toml` is the root, else the start folder. One project per run: V2 modules imported from other folders use the entry's config.
- **Relative paths** resolve against the folder of whatever declared them (DOCS 14): `v2.toml` → the project root; `config.toml` → `~/.v2`; `V2_ENGINE_*` and `--engine` → the cwd; a `register_engine` spec → the calling `.v2` file's folder. This covers `[project] entry`, `[python] venv`/`paths`, `[node] modules`/`paths`, `[ffi] search_paths` and engine commands:
  - `command[0]` and a transpiler's `argv[0]`: with a separator, joined to that folder; without one, the `PATH` walk (7.5).
  - Later `command` elements (D38) that are relative (no root and no drive, so Windows switches such as `/c` stay), contain `/` or `\`, do not start with `-` and contain no `{` are made absolute against that folder, so `["ruby", "adapters/v2_adapter.rb"]` works from any cwd. Nothing else is rewritten; `~` and `$VAR` are not expanded.
  - Paths used by scripts are separate (D26): `read_file`/`write_file` use the cwd; `import` and `extern` the `.v2` file's folder.
  - A Windows drive-relative path in a config file (`C:main.v2`) would resolve against that drive's cwd, so it is a configuration error: `[project] entry: 'C:main.v2' is a drive-relative path: write it relative to the file, or absolute` (as 6.4 does for imports).

### 18.2 Schema

`[engines.<tag>]` names are canonicalized (`[engines.python]` is `[engines.py]`; both in one file is an error). A built-in tag accepts only the built-in keys below; any other tag declares a custom engine, whose keys and messages are 17.1's (every key optional except `kind` and `command`, D37).

| Key | Type | Default | Applies to; meaning and validation |
|---|---|---|---|
| `[project] name` | string | root folder name | shown by check and doctor |
| `[project] entry` | path | none | what `v2 run` runs; must exist then |
| `[engines.T] command` | array of strings (a string = one element, never split) | auto (7.6) | non-empty. Built-in: interpreter or compiler argv prefix, skipping auto-detection; custom: 17.1 (a `run` command must contain `{file}`; there is no `source` option, D65) |
| `[engines.T] flags` | array of strings | `[]` | `c`, `cpp`, `rust`: extra compiler flags, after V2's own (15.7) |
| `[engines.T] startup_timeout` | number, s | 30 | `py`, `js` (also for ts), custom bridged: spawn to `init` reply (8.4); finite, ≥ 0, 0 = none |
| `[engines.T] call_timeout` | number, s | 0 = none | each request to a `py`/`js` worker or custom bridged engine, each run of a custom `run` engine or a `c`/`cpp`/`rust` program (8.5, D39); in-process calls (compiled exports, `extern`) ignore it |
| `[engines.ts] transpiler` | `"node"`, string, array | `"node"` | 7.6 (D36) |
| `[engines.T] kind`, `ext`, `aliases` | per 17.1 | 17.1 (`ext` = the tag) | custom engines only |
| `[python] venv` | path | none | venv whose interpreter and packages are used (7.6) |
| `[python] paths` | array of paths | `[]` | module folders after the `.v2` file's folder and the project root (9.3) |
| `[node] modules` | path | `node_modules` | package folder, passed via `NODE_PATH` (8.3, D54) |
| `[node] paths` | array of paths | `[]` | more folders after `modules` |
| `[ffi] search_paths` | array of paths | `[]` | first step of the `extern` search, before the `.v2` file's folder, the project root and the system (16.2, D60) |

`[engines.ts]` takes only `transpiler`: `command` there is an error with 18.4's message; `startup_timeout` and `call_timeout` are warnings (`ts runs in the js session; set it in [engines.js]`).

### 18.3 Layers and merging

L0 built-in < L1 `~/.v2/config.toml` < L2 `v2.toml` < L3 `V2_ENGINE_<TAG>` < L4 `--engine` (DOCS 14).
- Per key the highest layer wins; arrays are replaced, never concatenated. Tables of built-in engines merge key by key (a `command` from L1 and a `call_timeout` from L2 both apply); a custom engine's table comes whole from the highest layer that declares it (17.2), and L3/L4 then replace its `command[0]`.
- `[engines.py] command` and `[python] venv`: the higher layer wins; on a tie, `command`.
- Validation runs on the merged result and names the file of the bad value: the files are read into raw values, the winners are type-checked, and a value a higher layer replaces (including a `command` replaced by L3/L4) is never validated. Folder warnings (18.5) are for the winning arrays only. Structural problems (TOML syntax, duplicate engine tables, `kind`/`aliases` on a built-in, a `command` in `[engines.ts]`) are errors of the file that has them.
- Values are `Sourced<T> { value, from: ConfigSource }`, `ConfigSource = Default | Path | Vswhere | Venv | User(PathBuf) | Project(PathBuf) | Env(String) | Cli | Runtime(FileId)` (doctor's FROM column). `Config` holds the root and the files read, `[project]`, `engines: IndexMap<canonical tag, EngineCfg>` (fields `Option<Sourced<_>>`), `[python]`, `[node]`, `[ffi]`, `pending` overrides (18.4), `warnings` and `no_cache`.

### 18.4 Environment and command-line overrides

- `V2_ENGINE_<X>` (prefix case-insensitive on Windows; `X` lower-cased and canonicalized, so `V2_ENGINE_PYTHON` is py) holds one executable path, never split; empty = unset; two variables naming one engine are a configuration error. `--engine TAG=PATH` is repeatable (last wins) and TAG may be an alias. A PATH with a separator resolves against the cwd; a bare name (`node`) is left for the `PATH` walk (18.1, 7.5).
- Effect: a built-in engine gets `command = [PATH]`, dropping any prefix such as `py -3` (a `cl.exe` path follows 7.6 step 3); a custom engine gets `command[0] = PATH`, the rest kept; ts is a configuration error: `ts has no executable of its own; use --engine js=PATH or [engines.ts] transpiler`. A tag unknown at startup goes to `pending`: applied if `register_engine` later registers it, else ignored (check and doctor warn).

### 18.5 Parsing and validation

- A leading UTF-8 BOM is dropped (invalid UTF-8 is an error); the `toml` crate parses each file into a `toml::Table`; validation is hand-written (1.4).
- Errors exit 2 before anything runs. Syntax: `C:\proj\v2.toml:12:5: config error: <toml message>` (position from the error span). Wrong type or value (a `toml::Table` keeps no positions): `C:\proj\v2.toml: config error: [engines.py] startup_timeout: expected a non-negative number, got string "30s"`; custom engine tables use the same form with 17.1's problem text.
- A known section (`project`, `engines`, `python`, `node`, `ffi`) that is not a table is a type error (`[engines]: expected a table, got integer 1`); any other top-level key is an unknown key.
- Warnings (`Config.warnings`, printed only by check and doctor): no home folder (18.1); unknown sections and keys, also in custom engine tables, with did-you-mean (`warning: C:\proj\v2.toml: unknown key [engines.py] comand (did you mean 'command'?)`); keys that do not apply (`flags` on py, `transpiler` on js, `[project]` in `config.toml`); listed folders that do not exist.

---

## 19. CLI, doctor and check

`cli/` is a hand-written argv parser; `doctor/` holds doctor and check.

### 19.1 Commands and exit codes

| Command | Does |
|---|---|
| `v2 [FLAGS] FILE [ARGS...]` | run FILE; ARGS become `args()` |
| `v2 [FLAGS] run [ARGS...]` | run `[project] entry` (18.1); `no v2.toml found in C:\x or its parents` or `v2.toml has no [project] entry`: exit 2 |
| `v2 [FLAGS] check FILE` | 19.3 |
| `v2 [FLAGS] doctor [TAG]` | 19.2; the positional TAG (an alias works) selects the deep test (D34) |
| `v2 cache clean` | 19.4 |
| `v2 --version`, `v2 --help` | `v2 <crate version> (VEP/1)`; the command, flag and exit-code tables |

FLAGS are `--engine TAG=PATH` (repeatable, 18.4) and `--no-cache` (19.4), read from `argv[1]` up to the first other word (`--` ends them). That word is a subcommand if it is exactly `run`, `check`, `doctor` or `cache`, else the script (`v2 ./doctor` runs a file named doctor); everything after FILE or `run` goes to `args()` untouched. check and doctor also take FLAGS after the subcommand. `--engine` without `=PATH` is a usage error (hint: `v2 doctor TAG` tests one engine). No arguments, an unknown flag, or a missing or unreadable FILE: a message on stderr, exit 2.

| Exit | Meaning (D15) |
|---|---|
| 0 | success |
| n | `exit(n)`: an int in i32 range; POSIX keeps the low 8 bits |
| 1 | uncaught error or `SyntaxError` (`EngineUnknown` included); check found errors; a `doctor TAG` test failed |
| 2 | usage or configuration error (18.5), before any V2 code runs |
| 3 | uncaught `EngineNotFound`; check found a required engine missing (19.3); `doctor TAG` did not find the engine |
| 130 | interrupted: Ctrl+C, Ctrl+Break, console close, SIGINT, SIGTERM, SIGHUP; always 130, never 128+signo; no `finally`, engines stopped, `<run>` removed (6.5, 8.7) |
| 101 | internal panic, a V2 bug (6.5) |

### 19.2 v2 doctor

1. Load config from the cwd (18.1; error: exit 2); the registry is the built-ins, then the config's custom engines. Resolve every engine with fresh probes (7.7), rewriting the probe cache. Doctor executes toolchain commands, including project-configured ones (`v2.toml` commands, the venv python, vswhere and vcvarsall) (D69), so it is not an audit tool; `v2 check` is.
2. Print a table to stdout, columns `ENGINE STATUS COMMAND VERSION FROM`, each as wide as its longest cell + 2 (DOCS 2). COMMAND: `shown` plus prefix args, space-joined, unquoted; ts: `(js engine, built-in type stripping)` or `(js engine, transpiler: <argv>)`; custom: the argv template, VERSION `-`. VERSION: the probed version (ts: Node's). FROM: `PATH`, `vswhere`, `venv`, `v2.toml`, `config.toml`, `V2_ENGINE_<TAG>`, `--engine` or `-` (ts: js's, or where its transpiler is set). STATUS: `ok`; `missing` (other columns `-`); `old` (only versions below the minimum; VERSION the newest; also ts on a Node without stripping); `broken` (probe failed or timed out, wrong target, or a configured command, venv or transpiler missing).
3. Then `notes:` (skipped hits such as Store placeholders, 7.8 install hints, compiler family and target), the config files used and config warnings. Exit 0: missing engines are information, not failure. For example:
   ```text
   notes:
     py: skipped C:\Users\me\AppData\Local\Microsoft\WindowsApps\python3.exe (Microsoft Store placeholder)
     js: not found; install: winget install -e --id OpenJS.NodeJS.LTS
     ts: not found; install: winget install -e --id OpenJS.NodeJS.LTS
   config files: none
   ```
   An engine that is `broken` or `old` gets the first line of its `EngineNotFound` message as its note.

`v2 doctor TAG`: an unknown tag prints the `EngineUnknown` message, exit 2; an engine not found prints the 7.8 message, exit 3. Otherwise:
- **py, js, ts:** start the session (8.3, 8.4), print the `hello` fields and the handshake time, run an action unit whose value must be the int `2` (`1 + 1`; ts: `const x: number = 1; x + 1` in the js session), shut down (8.7).
- **c, cpp, rust:** compile, never served from the cache (15.6's `--no-cache` path), a synthetic block exporting `add` (`int add(int a, int b)` / `fn add(a: i32, b: i32) -> i32`), call `add(2, 3)` through libffi and require `5`; build an action program printing `ok` and compare its stdout; print each compiler command line and its time.
- **Custom engines:** 17.7's checks (RUN1–RUN2 for run engines, BR1–BR9 for bridged), with 17.7's output and exit codes.

The last line is `py: ok` or `py: FAILED: <error>`; exit 0, or 1 on any failure.

### 19.3 v2 check

Check executes nothing (no engine, compiler, probe, transpiler, `xcode-select`, `vswhere`, or library load, which would run initializers) and writes nothing (no probe cache, no `<run>`); it only reads files and metadata (D69, 20.1).
1. Load config (18.1, from FILE's folder; error: exit 2). Parse FILE and, transitively, every `import "…"` target once (paths are literals, so imports inside `if` count too); a cycle or a missing target gives the run-time `ImportError` text (6.4) at the `import`; a file that fails to parse is reported and its imports are skipped.
2. **Errors per file:** every `SyntaxError` (D1; format 2.4) of chapters 3, 4 and 16.1, such as directive syntax, duplicate labels, block args on a compiled tag (D32), `@import SEL as NAME` without a package selector (D10), a definition block used as a value, `extern` declarations, and quoted `@export` prototypes in compiled blocks that `ffi::cdecl` cannot read (16.3). Check adds: `@export`/`@import` in a block of a config-declared `run` engine (17.3); in a compiled block, `@import` without `@export` (D57) and `@import { * }` or `@import SEL as h` (15.2); explicit export items whose function cannot be exported (15.2).
3. **Engines:** every tag in block headers and selectors, plus `register_engine("<literal>", …)` tags (shown `runtime`). Unknown: an `EngineUnknown` error with did-you-mean (a warning instead if a checked file calls `register_engine` with a non-literal tag). Known: resolved by 7.6 without executing (a configured command or venv interpreter must exist; `PATH` is walked); versions and skips come from matching probe-cache entries (ts from the cached Node version, D33), else VERSION is `not probed` and a `WindowsApps` alias `unverified`.
4. **Packages:** each module selector, its first use, and a location if a stat finds one (py: `x.py` or `x/__init__.py` in the script folder, the project root, `[python] paths` or the venv's `site-packages`; js: `node_modules/<name>/package.json` walking up from the script folder, then `[node] modules` and `[node] paths`), else `(resolved by python/node at run time)`. Never an error.
5. **Libraries:** each `extern "lib"` through 16.2's search order (D60) as existence checks. A missing path library is an error (the `FFILoadError` text); `"c"` and `"m"` show `system (not verified)`; a bare name with no hit is a warning (`not found in <folders>; the system loader may still find it`): ld.so.cache, the dyld cache and system DLLs cannot be checked without loading.
6. **Compiled blocks:** export signatures are parsed (15.2, 15.3) without compiling; check lists each export with its V2 types and the functions `@export { * }` leaves out, with the reason (D56).
7. Diagnostics go to stderr (2.4); the inventory to stdout: `check main.v2: 2 files, 0 errors, 1 warning`, then sections `ENGINES` (tag, status, command, version, uses like `main.v2:12 stats`), `PACKAGES`, `LIBRARIES`, `COMPILED`.
8. Exit 1 on any error; else 3 if a used engine is missing, not counting `runtime` tags and tags named by a literal `engine_available("<tag>")` in the checked files (DOCS 10's skip pattern, shown `optional`); else 0.

### 19.4 v2 cache clean and --no-cache

- `v2 cache clean` deletes everything under `~/.v2/cache/` (`build/`, `toolchains.json`) entry by entry, never `config.toml`, and prints `removed 118 files (43.2 MB) from C:\Users\me\.v2\cache`; files it cannot delete (on Windows, libraries a running V2 has loaded) are skipped and counted (15.6). Exit 0, or 2 if the home folder is unknown.
- `--no-cache` applies to runs: compiled blocks are always rebuilt, never served from the build cache, and the fresh build replaces the cache entry (15.6); the probe cache is not read, so probes run fresh (7.7). Doctor already probes fresh; check ignores the flag.

---

## 20. Security model

### 20.1 Trust boundary

V2 is a same-user tool. Everything a `.v2` file contains runs with the user's rights: its blocks, compiled blocks, `extern` libraries and the engine commands its config names. The mitigations in 20.2 keep three promises:

- **Data stays data.** V2 values never become code: block args and call arguments travel as data, and V2 never inserts values into foreign source (DOCS 4.1, 4.4).
- **No code the user didn't choose.** A downloaded project or another local user can't get code run through the working directory, `PATH`, DLL planting or the build cache.
- **Inert audit.** `v2 check` executes nothing and writes nothing (19.3).

Out of scope: sandboxing, permission prompts, signature checks, defending against the user's own other processes, and the config files themselves. A `v2.toml` found by the upward search, or `~/.v2/config.toml`, is trusted like the script: V2 checks neither its owner nor its permissions, and `v2 doctor` lists the config files it read (19.2).

### 20.2 Threats and mitigations

| Threat | Mitigation | Where |
| --- | --- | --- |
| Running an untrusted `.v2` file or project | No sandbox (DOCS 16). The audit path is `v2 check` (19.3): it parses and resolves commands by path lookup only (stat, no `--version`), and never starts a worker, runs a compiler or transpiler, loads an `extern` library (loading runs its initializers) or calls `register_engine`. `v2 doctor` *does* run toolchain commands, including those a project's `v2.toml` configures (19.2) | cli/, doctor/ |
| Temp-file and symlink races | Temporary files live only in the private run dir (20.3) or in a temp dir inside the owner-only build cache (15.6). Files are created with `create_new` (`O_CREAT` + `O_EXCL`, which also refuses a planted symlink) and mode 0600. V2 never opens a predictable path in the shared temp dir and never writes into the project | engines/runonly, engines/session, build/ |
| Tampered adapter | `py.py` and `js.js` are embedded (`include_str!`) and written create-new into `<run>/adapters/` when their session first spawns (14.1); they are never read from the project, `PATH` or `~/.v2` | adapters/ |
| Data breaking the frame stream (the old `\r` desync) | Frames are written only by serde_json and each language's JSON encoder, which escape every control character. Readers split on the `\n` byte only and drop one `\r` before it, so `\r`, U+0085, U+2028 or `\x1c` inside a string can't end a frame. Frames over 64 MiB are refused in both directions; after `hello` a line that isn't a frame is a fatal ProtocolError (9.11) | codec/json, engines/vep |
| User code or its children reaching the protocol | Before user code runs, each adapter guards the protocol (9.9). Python moves it to private non-inheritable fds, points fd 0 at the null device and fd 1 at fd 2 (14.3). Node has no `dup2`, so the protocol stays on fds 0 and 1: frames are written with a saved `fs.writeSync(1, …)` and read with `fs.readSync(0, …)`, and the adapter wraps `process.stdout`, `process.stdin`, `console`, `fs` calls on fds 0–2 and `child_process` stdio (14.4). Child processes can neither read nor write frames, and fd-level output lands on stderr (DOCS 4.6). Remaining gap: a native addon, or code that reopens `/dev/stdout` or bypasses the wrappers, can still corrupt the stream (fatal ProtocolError, 9.11) | adapters/ |
| Evaluating names | Calls resolve through the unit's export table, never by evaluating text from a frame (the old JS worker did `eval(req.fn)`); the only generated code is 12.4's `(() => NAME)` getter, built for a name that passed the identifier test. Python names: `str.isidentifier()` and not `keyword.iskeyword()`, per dotted segment for module names (13.5). JS export names: `/^[\p{ID_Start}$_][\p{ID_Continue}$\u200C\u200D]*$/u` and not reserved; JS specifiers reach `require`/`import()` as strings. Block args are bound as values (DOCS 4.4) | adapters/, engines/units |
| Module shadowing | The search order is documented (DOCS 5, 7), and a local `statistics.py` shadowing the stdlib is intended, as in plain Python. Adapters import their own dependencies before user folders join the module path and remove their own folder from it (9.3, 14.3), so user files can't replace adapter internals. The cwd is never added as such | adapters/ |
| Build-cache poisoning | The cache lives only in `~/.v2/cache/build` (mode 0700), never in the project, so a cloned repo can't ship a library that V2 would load (DOCS 11.5, 16). The key covers the source, the compiler's path, version and target, the flags and the generated prelude; an entry appears only through one atomic rename of a same-volume temp dir (15.6) | build/cache |
| Command and library hijack | V2's own PATH walk skips relative and empty entries such as `.` (7.5), and children are spawned by absolute path. `extern` libraries are searched in `[ffi] search_paths`, the `.v2` file's folder, the project root, then the system default (16.2). Windows: `SetDllDirectoryW(L"")` at startup removes the current directory from the DLL search order while `PATH` stays in it, and a library resolved to a full path is loaded with `LoadLibraryExW(path, LOAD_WITH_ALTERED_SEARCH_PATH)`, so its dependencies come from its own folder first. POSIX: relative library paths are made absolute against the `.v2` folder before `dlopen`; macOS never uses bare-name `dlopen`, which searches the cwd | ffi/lib, engines/resolve |
| Processes outliving their block or the run | Every child runs in its own session on Unix (`setsid()` in `pre_exec`: no controlling terminal, so terminal signals never reach it and a `/dev/tty` prompt fails at once instead of stopping it), and in its own process group (`CREATE_NEW_PROCESS_GROUP`, which blocks Ctrl+C only: Ctrl+Break and console close reach every process on the console; adapters ignore SIGBREAK, and the host's handler sets `INTERRUPTED` first, so child failures that event causes are never reported) plus a Job Object on Windows (8.3). Run-engine and compiled-program trees are killed when their block finishes or times out (17.3), workers as trees at shutdown (8.7); kill-on-close Job Objects (Windows) and `PR_SET_PDEATHSIG` (Linux) also cover a host that dies | engines/session, engines/runonly |
| Environment inheritance | Engines inherit the environment and cwd (DOCS 16), including `PYTHONPATH`, `NODE_OPTIONS` and `LD_PRELOAD`: the user's own settings, trusted. V2 adds only `PYTHONUTF8=1` and `PYTHONIOENCODING=utf-8` (py), `NODE_PATH` (js, 14.1), compiler locale and vcvars variables (15.7) and `V2_ARG_*` (run engines, after removing inherited ones). No channel token or other secret travels in the environment. Args passed as env vars can be read by the user's other processes (`/proc/<pid>/environ`) | engines/session, engines/runonly |
| Memory safety of FFI and compiled code | Trusted and unsandboxed (DOCS 11.7, 12). V2 checks only what it converts: integer ranges, NUL in strings, `null` for non-null string parameters, unsigned results above `i64::MAX`, UTF-8 of returned strings (16.5, 16.6). Pointers are opaque values that come only from native returns or `null`; there is no int-to-pointer conversion. `const char*` arguments live for the call only. V2 never frees memory returned by `extern` functions; strings returned by a compiled block are freed with that library's own `__v2_free` (same CRT). A native crash kills the process: there is no crash handler | ffi/, build/shims |

### 20.3 The private run directory

- **Path.** `<run>` = `temp_dir()/v2-{pid}-{32 hex}`, the hex being 16 bytes from the OS RNG. It is created on first need, so a pure-host script never makes one.
- **POSIX.** Create it with `mkdir` and mode 0700. If the name exists, draw a new one; after 8 tries, raise IOError `cannot create a private temp folder in {base}`. Then `lstat` must show a real directory (not a symlink) owned by the euid, with mode 0700.
- **Windows.** Create it with `CreateDirectoryW` and a protected DACL that grants full control only to the current user's SID (from `GetTokenInformation(TokenUser)`): SDDL `D:P(A;OICI;FA;;;{sid})`.
- **Contents.** `run/{tag}-{n}.{ext}` (17.3); `adapters/` (14.1); `build/<k>/` for every build (15.6), from which libraries and their 15.8 copies are loaded only when `~/.v2` is unusable; otherwise the copies live in the cache entry (15.6, 15.8). Every file is created with `create_new`.
- **Removal.** On every exit path (normal end, uncaught error, `exit()`, interrupts), after workers and child trees have stopped (6.5); errors are ignored. A host killed outright (SIGKILL, `TerminateProcess`, a native crash), or a library that still pins a file, leaves the folder behind; it stays in the temp dir, owner-only.

---

## 21. Failure-mode matrix

### 21.1 Conventions

- **Placeholders.** `{tag}` is the canonical tag, `{name}` the called or imported name, `{sel}` the selector as written, and `{block}` the full `block_name(b)` output (12.1, D31): `block 'stats'` when labeled, `@py block at main.v2:12` otherwise. A section number in the Message column points at the defining text.
- **Dead.** The worker has exited or been killed; every unit, Proxy and handle of its session is invalid, and engines are never restarted. The use that observes the death raises the matching "exited", "timeout" or "protocol" row; every later use raises 8.2's `engine '{tag}' is not running (…); engines are not restarted` (8.2, 9.11), except a session that never started (Dead(SpawnFailed): spawn failure, a `hello` version below the minimum, Windows status 9009 at startup), whose later uses re-raise the stored EngineNotFound (exit 3 if uncaught).
- **SyntaxError** is a diagnostic, not a value: `{file}:{line}:{col}: SyntaxError: {msg}` with the source line and a caret (2.4), reported before anything runs, exit 1, uncatchable in its own file. In an `import`ed module it reaches the importer as ImportError (message: the first diagnostic; `data`: all of them; 6.4).
- **Exit codes** (6.5, 19.1): 0 success; `n` for `exit(n)`; 1 uncaught error (EngineUnknown included) or SyntaxError; 2 usage or configuration error, before anything runs; 3 uncaught EngineNotFound, `v2 check` finding a required engine missing, or `v2 doctor TAG` not finding its engine; 130 any interrupt, never 128 + signal; 101 internal panic.

### 21.2 Matrix

| Trigger | Detected by | Kind | Message | State after |
| --- | --- | --- | --- | --- |
| **Setup** | | | | |
| Unknown tag (`@pyhton`) | registry when the block is reached; `v2 check` statically | EngineUnknown | 7.3 `unknown engine '@pyhton' (did you mean '@py'?)` | nothing started; later reaches fail the same way unless `register_engine` runs first |
| Toolchain missing or too old; only the Store placeholder `python.exe` (exits 9009) found | engines/resolve at first use (7.6, 7.7) | EngineNotFound | 7.8 `engine 'py' (Python) not found` plus `tried:`, `skipped:` and `install:` lines | failure memoized for the run (7.1); exit 3 if uncaught |
| Configured command, venv or transpiler missing; Node too old for `@ts` stripping | engines/resolve | EngineNotFound | 7.6 (`engine 'py' command 'C:/Python312/python.exe' not found (set by C:\proj\v2.toml)`, `@ts blocks need Node 22.13+ (or 23.2+) for built-in type stripping (found …) — upgrade Node or set [engines.ts] transpiler`); custom: 7.4 `'lua' not found on PATH (engine declared in C:\proj\v2.toml)` | as above; never falls back to auto-detection |
| Executable can't start | spawn | EngineNotFound | 8.3 `could not start '{path}': {os error}` | Dead (SpawnFailed); later uses re-raise this EngineNotFound |
| `hello` reports a version below the minimum | engines/session | EngineNotFound | 8.4 `Python 3.6.8 at {path} is too old (needs 3.8+)` | killed; Dead (SpawnFailed); later uses re-raise this EngineNotFound |
| No `hello` and `init` reply within `startup_timeout` | watchdog | EngineTimeout | 8.4 `engine 'py' did not start within 30 s ({path}); raise [engines.py] startup_timeout if it is only slow` | killed; Dead |
| Worker exits before `hello`; `init` answered with `err` | engines/session | EngineCrashed | 8.4 `engine 'py' exited during startup (exit code 1); see its error output above` / `engine 'py' failed to start: {message}` | Dead |
| Non-JSON lines before `hello` (`sitecustomize`, `NODE_OPTIONS` preloads) | engines/vep | none | forwarded to stderr | startup continues |
| A JSON frame other than a valid `hello` (including `out`) before `hello`; `vep` other than 1 | engines/session | ProtocolError | 8.4 `engine '{tag}' sent '{t}' before hello`; 9.3 `engine '{tag}' speaks VEP/{n}; this V2 speaks VEP/1` | killed; Dead |
| Invalid config file or custom engine table | config/, engines/spec | configuration error | 18.5 | exit 2 |
| Bad `register_engine` arguments | engines/spec | TypeError, ValueError | 17.1 | registry unchanged |
| Built-in, config-declared or differently registered engine name | engines/registry | EngineNameConflict | 17.2 | registry unchanged |
| **Blocks and imports** | | | | |
| Exception while a definition block loads, foreign syntax errors included | adapter at `load` | ForeignError (`foreign_type` e.g. `SyntaxError`) or the reported kind | the foreign message at its `.v2` line, with a source excerpt (2.4) | unit `Failed`: every later reach re-raises the same stored error without running code or evaluating args (12.1) |
| The same in an action block | adapter | ForeignError | as above | nothing kept; the next reach runs the block again |
| `@export` lists a name the block doesn't define | adapter (`missing`, 9.4) | ImportError | 12.2 `@export lists 'typo' but block 'geometry' does not define it` | unit `Failed` |
| Definition block reached again during its own load | engines/units | ImportError | 12.1 `block 'stats' is still loading (reached again during its own load)` | the load continues |
| `@export` or `@import` in a run-engine block | engines/units (12.1 step 3); `v2 check` for config engines | ImportError | `@{tag} blocks cannot @export or @import — '{tag}' is a run engine` | block not run |
| Name not exported by the selected blocks | engines/imports | ImportError | `'{name}' is not exported by '{sel}' (did the block @export it?)` | nothing bound |
| Ambiguous name, named or `*` | engines/imports | ImportError | 13.3 `Import of 'mean' from '@py' is ambiguous (exported by blocks 'stats' and 'calc') — use @py.stats or @py.calc` | nothing from that `@import` bound |
| Selector matches no definition block | engines/imports | ImportError | `No engine exports found for selector '{sel}'` | nothing bound |
| V2-level `@import` from a block not reached yet / still loading | engines/imports | ImportError | `{block} has not run yet — move this @import below it` / 13.3 `… is still loading …` | nothing bound |
| `@import` from a `Failed` block | engines/imports | ImportError | `{block} failed to load: {message}` | nothing bound |
| Inner `@import` stub called before its target block loads | host on `call {route}` (13.6) | ImportError, raised into the calling engine | 13.7 `block 'b' has not run yet — 'f' can't be called before it loads` | route stays pending |
| Foreign package not found | adapter on `module` | ImportError (`lang`, `foreign_type` set) | the foreign message (`No module named 'pandas'`) | session fine |
| Invalid Python module name | engines/imports, before sending | ImportError | 13.5 `'my-mod' is not a valid Python module name` | nothing sent |
| Module selector on a compiled or run engine; adapter without `modules` | engines/imports | ImportError | 13.5 `module imports need a bridged engine — 'c' is compiled`; 17.5 `engine '{tag}' cannot import packages` | nothing sent |
| Whole-module handle into another engine's block | engines/imports | ImportError | 13.7 | block not loaded |
| Constant export from a compiled block | build/ | ImportError | 15.2 `block 'fastmath' exports 'x', but does not define it as a function` | unit `Failed` |
| **Calls and values** | | | | |
| Foreign exception | adapter `err` | ForeignError | the foreign message; uncaught: `ForeignError: {foreign_type}: {message}` at the innermost mapped `.v2` frame, with an excerpt (2.4) | session fine |
| Python `sys.exit()` / JS `process.exit()` in a block | adapter (9.7) | ForeignError `SystemExit` / a JS `Error` thrown in the block | 9.7 | worker keeps running |
| V2 error thrown inside a callback | host replies `err` with `v2`; the adapter re-raises it | its original kind | its original message | session fine |
| Named args to js/ts, compiled or `extern` functions, or a bridged engine without `kwargs` | host, before sending | ArityError | `{tag} functions take positional arguments only (got '{kw}')`; native: 5.7 `{name}() takes positional arguments only` | nothing sent |
| Handle passed to another engine, compiled code or `extern` | codec | MarshalError | another engine: 11.7 `this handle belongs to engine 'py' and can't be passed to 'js' — convert it with value(h) first`; compiled code and `extern`: 16.5 `cannot pass a py handle (datetime.date) to native code; convert it with value(h) first` | nothing sent |
| Arithmetic, ordering, `in` or slicing on a handle | runtime | TypeError | 5.2, 11.7, with the "do it inside a block / `value(h)`" hint | handle still valid |
| `value(h)` can't convert | adapter on `op value` | MarshalError | 11.5 `value(): can't convert datetime.date to V2 data` | handle still valid |
| Value nested deeper than 256, cyclic, or holding an unpaired surrogate | codec, adapter | MarshalError | 10.4, 10.5 | nothing sent |
| Outgoing frame over 64 MiB | codec | MarshalError | 9.1 `value too large to send to engine 'py' (70 MiB, limit 64 MiB)` | nothing sent |
| Coroutine or Promise returned inside a nested cross-language call | adapter | Error | 9.8 | session fine |
| Callback into V2 while its engine is idle (timer, thread, late event handler), or from another thread | adapter stub (9.8); compiled code: `host_call` (15.9) | Error raised in the foreign language; compiled: NULL and `v2_last_error()` | 9.8; 15.9 `V2 can only be called from the thread that called into this library, while that call is running` | V2 function not run; session fine; the callback stays valid for later requests |
| 65th nested cross-language call | host, before sending or when an inbound `call` arrives | RecursionError | `cross-language call depth limit (64) exceeded` | unwinds back to V2 with its kind |
| Worker dies during a call | engines/session: EOF, pipe error or exit | EngineCrashed | 8.6 `engine 'py' (pid 4242) exited during a call to 'ratio' (exit code 3221225477 = 0xC0000005); see its error output above`; Unix `killed by signal 11 (SIGSEGV)` | Dead |
| Worker died while idle | the next write (`EPIPE`) or read | EngineCrashed | 8.6, raised by the next use | Dead |
| Any later use of a Dead session or its handles (Dead(SpawnFailed): 21.1) | host | EngineCrashed | 8.2 `engine 'py' is not running (…); engines are not restarted`; handles: 11.7 | Dead |
| Bridged call exceeds `call_timeout` | watchdog, timing the innermost pending host→worker request (paused while V2 serves a reverse call, 8.5) | EngineTimeout | 8.5 `call to 'slow' in engine 'py' exceeded call_timeout (5 s); the engine was stopped` | killed; Dead |
| Non-frame line after `hello`, corrupt frame, unknown `t`, bad `$v2` tag, out-of-order reply, frame over 64 MiB | engines/vep (9.11) | ProtocolError | 9.11 `engine 'py' broke the protocol: <what> (got: <first 80 bytes, escaped>)`; 9.7 `engine 'py' broke the protocol: reply to request 7, which is not the innermost pending request` | killed; Dead (later uses: 9.11) |
| Worker `call` with an unknown `route` or `fn` id | host | `err` ProtocolError to the worker | 9.5 `unknown callable id 42` | session fine |
| Block output below the language level (C extension, `os.write(1, …)`, a child inheriting stdout) | adapter fd setup (9.9) | none | none | text appears on stderr, not stdout (DOCS 4.6); protocol intact |
| Block code reads stdin | adapter stdin replacement (9.9) | ForeignError | 9.9 | session fine |
| Rust panic or C++ exception in an exported function | generated shim (15.4) | ForeignError | the panic payload (`foreign_type` `panic`) / `what()` (`foreign_type` the exception type, `unknown` for `catch (...)`) | library stays loaded |
| Native crash, or native code calling `exit()`/`abort()` | OS | none | the OS message and status | process gone without V2's shutdown; workers see stdin EOF; kill-on-close jobs and `PDEATHSIG` stop children; run dir left |
| Run engine or compiled program exits non-zero | engines/runonly | EngineExitError | 17.3 | `data.stderr` holds the tail |
| Run engine or compiled program exceeds `call_timeout` | watchdog | EngineTimeout | 17.3 | process tree killed |
| Captured block output isn't UTF-8 | engines/runonly | MarshalError | `output of @{tag} block is not valid UTF-8 (byte {k})` | temp file removed |
| Block arg unusable as a `V2_ARG_*` variable | engines/runonly | ValueError, MarshalError | 17.4 | nothing spawned |
| **Build** | | | | |
| Compiler error | build/ | BuildError | 15.7 `{block} failed to compile: {first error}` (`block 'fast' failed to compile: …`), positions mapped to the `.v2` file | nothing cached; a definition unit is `Failed` |
| No compiler | engines/resolve | EngineNotFound | 7.8 (c, cpp and rust hints) | exit 3 if uncaught |
| Export with an unreadable signature, an unsupported type or several definitions | build/, before compiling | BuildError | 15.2 | block not built |
| `@import` inside a compiled program-mode block | engines/units (12.1 step 3); `v2 check` | BuildError | 15.2 `@import in a compiled block needs @export — a compiled program runs as a separate process` | block not built |
| Configured compiler targets another architecture; compiler can't be spawned | engines/resolve (15.7's target check); the spawn | EngineNotFound | 15.7 `gcc at C:\mingw32\bin\gcc.exe targets i686, but v2 is x86_64 — install a 64-bit toolchain`; 8.3 `could not start '{path}': {os error}` | block not built; exit 3 if uncaught |
| Library ABI mismatch | build/ | BuildError | 15.8 `library ABI mismatch` | block not loaded |
| **FFI** | | | | |
| Library not found, or found but fails to load | ffi/lib when the `extern` statement runs | FFILoadError | 16.2 `cannot load library '{lib}'`, one line per attempt, and a hint; the first existing candidate's failure is final | no names bound |
| Declared symbols missing | ffi/lib, all declarations eagerly | FFISymbolError | 16.1 `not found in library 'm' ({path}): cosf128, foo` | no names bound |
| Argument doesn't fit: range, NUL, wrong type, `null` for `std::string`/`&str`/`String` | ffi/types, on each call | FFIValueError | 16.5 `argument 2 ('exponent', double) of 'pow': {reason}`; 15.3 | call not made |
| Unsigned result above `i64::MAX`; returned string not valid UTF-8 | ffi/types | FFIValueError | 16.6 `return value 18446744073709551615 of 'f' does not fit in a V2 int` | an owned string is still freed |
| Unreadable C declaration | ffi/cdecl, at parse time | SyntaxError (`extern` bodies and `@export` prototype strings) | 16.3 | file doesn't run |
| Any operation on a native pointer | runtime | TypeError | 16.6 `native pointers support no operations — pass them to extern functions` | pointer still valid |
| **Syntax** (all SyntaxError; the file doesn't run) | | | | |
| Block never closed | syntax/scan | SyntaxError at the opening line | 3.7 `@py block 'stats' is never closed` (unlabeled `@py block is never closed`), with the fence hint | |
| `{` or fence not on the line of the header's last token | syntax/lexer | SyntaxError | 3.7 `the { that opens a block must be on the same line as @py` | |
| Multi-line block whose closing `}` isn't first on its line, or is followed by anything but `)` `]` `}` `,` `;` or a comment | syntax/scan | SyntaxError | 3.7 (C1, C2), with the fence hint | |
| Duplicate block label | parser | SyntaxError | 4.5 `duplicate block label 'stats'`, with a note at the first | |
| Tabs and spaces mixed in block indentation | syntax/directives | SyntaxError | 4.4 `block indentation mixes tabs and spaces` | |
| Definition block used as a value | parser | SyntaxError | 4.5 | |
| Block args on `c`, `cpp`, `rust` or an alias | parser | SyntaxError | `compiled blocks take no args — @export a function and call it with arguments` | |
| Bad number literal (`0b1`, `007`, `12abc`, beyond i64) | lexer | SyntaxError | 3.4 | |
| Assignment through `?.`; `@import SEL as x` without a package selector; a name declared twice in one body (parameters, loop variables and catch names included) | parser | SyntaxError | 4.1, 4.5 | |
| **Host** | | | | |
| Circular `import` | runtime/module | ImportError | 6.4 `circular import: a.v2 -> b.v2 -> a.v2` | no module of the cycle runs twice |
| Missing module file | runtime/module | ImportError | 6.4 `cannot find module './util.v2' (looked for …)` | a later import retries |
| Call depth over 10,000, or the stack guard | runtime/interp | RecursionError | 5.9 `maximum call depth (10000) exceeded` | unwinds to the nearest `catch` |
| Integer overflow | runtime/interp | ValueError | `integer overflow in '{op}'` | operands unchanged |
| Division or `%` by zero; zero to a negative power | runtime/interp | ValueError | `division by zero` | |
| Member access on `null` without `?.` | runtime/interp | TypeError | 5.5 `cannot read 'name' of null (use ?.)`; with `?.` the whole postfix chain is `null` (4.1) | |
| Failed write to stdout (closed pipe) | runtime/builtins | IOError at the `print`; at the end of the run `v2: cannot write to stdout: …` | 5.10, 6.5 | exit 0 becomes 1 |
| Failed write of a diagnostic, summary, `--help` or `--version` (closed stdout or stderr) | main, runtime, doctor | none: the write is dropped | 6.5 | exit code unchanged; never 101 or an abort |
| Interrupt: Ctrl+C, Ctrl+Break, console close, SIGINT, SIGTERM, SIGHUP | signal handler / `SetConsoleCtrlHandler` sets `INTERRUPTED`; the watchdog does the rest (6.5). Unix children run in their own session (`setsid`, 8.3), so terminal signals never reach them; Windows `CREATE_NEW_PROCESS_GROUP` keeps only Ctrl+C from children: Ctrl+Break and console close reach every process on the console (adapters ignore SIGBREAK, 14.3, 14.4), and child failures they cause are never reported | none (not catchable) | none | 6.5: child trees killed, temp dir removed, exit 130; no `finally` runs |
| `exit(code)`, even inside a nested cross-language or native call | runtime/builtins (from native code via 15.9's `PENDING_EXIT`) | none | none | no `finally` runs; pending foreign calls abandoned; sessions get `bye`, then are killed after 1 s (8.7); temp dir removed; exit `code` |
| Host panic (a V2 bug) | panic hook | none | 6.5 `v2: internal error: <panic message>` | children killed; exit 101 |

---

## 22. Testing and conformance

### 22.1 Layout

```text
tests/
  run.rs                  harness: every .v2 test plus the multi-step cases (cargo test --test run)
  v2.toml                 empty [project]; stops the upward config search inside the repo
  host/                   host language, modules, builtins; host/syntax/ for diagnostics
  engines/py/ js/ ts/     bridged engines, each with fixtures/
  cross/  imports/        multi-engine programs; selectors, ambiguity, aliases
  compiled/c/ cpp/ rust/
  ffi/
  custom/                 run and bridged custom engines
examples/fake_adapter.rs  scripted VEP/1 adapter for tests/custom (a Cargo example; not shipped)
src/**                    #[cfg(test)] unit and protocol tests
```

### 22.2 The `.v2` harness

- **Discovery.** Every `tests/**/*.v2` outside a `fixtures/` folder is a test. It runs as `v2 <file>` (`CARGO_BIN_EXE_v2`) with its own folder as the cwd, stdin from `<name>.in` or an empty closed pipe, and stdout and stderr captured, so output is always redirected. A test that needs settings has its own `v2.toml` beside it.
- **Headers.** Leading `//` comments: `// requires: py js`, `// exit: 1` (default 0), `// stderr: <text>` (repeatable; each must appear, in order), `// args: a b`, `// platform: windows|unix`, `// timeout: 60`.
- **Expected output.** Stdout must equal `<name>.out` after CRLF-to-LF conversion. A test with neither a `.out` file nor a `stderr:` line is rejected.
- **SKIP.** Once per harness run, a generated probe (run in an isolated home like the tests) prints `engine_available(t)` for every tag named in `requires:`; if it fails, every test that needs an engine fails with its error. A test that needs a missing tag reports `SKIP {file} (engine {tag} not available)` and never counts as a pass; when `V2_TEST_REQUIRE=py,js,…` is set (CI), a SKIP for a listed tag is a FAIL.
- **Isolation.** `HOME` and `USERPROFILE` point at a fresh temporary home (18.1 reads `~` from exactly these), so `~/.v2` config and caches start empty; `RUSTUP_HOME` and `CARGO_HOME` keep their original values so `rustc` still works. Inherited `V2_ENGINE_*` variables are removed, and `V2_ENGINE_FAKE` is set to the built `fake_adapter` for `tests/custom`.
- **Panic trigger.** Debug builds only (`cfg(debug_assertions)`, so never in a release binary): with `V2_TEST_PANIC` set, the interpreter thread panics at once, so a test can check 6.5's panic path (`v2: internal error: …`, exit 101).
- **Summary.** `{p} passed, {f} failed, {s} skipped`, listing every skip with its reason; `cargo test` fails when f > 0.
- **Lint.** A constant assertion matching `check\([^,]+,\s*true,\s*true\)` fails the run.

### 22.3 Rust unit and protocol tests

| Module | Cases |
| --- | --- |
| `syntax/scan` | per profile: Python `"}"`, `'{'`, `"""}"""`, `# }`, `f"{x}"`, raw strings; C-like `'{'`, `"}"`, `// }`, `/* } */`, C++ `R"(})"`, Rust `r#"}"#` and lifetimes `'a` (not char literals), JS templates containing `${…}`; Plain and the fence form; C1 and C2 violations; an unterminated block reported at its opening line |
| `syntax/directives` | `@export` matches as an exact token (`@exporter`, `@export_json` untouched); `@@export`/`@@import` lose one `@`; lists spanning lines; line counts preserved; mixed indentation is an error; NBSP indentation doesn't panic |
| `codec/json`, `codec/vep` | `1.0` stays `1.0`; `-0.0`; `i64::MIN`, `i64::MAX`; NaN and ±inf as `$v2` tags; `"\r\n\u2028\0"`; a dict with a `"$v2"` key; `bytes`; lone surrogates (10.4); the old `json_quote` cases |
| `ffi/cdecl` | accepts `double cos(double x);`, `const char* sqlite3_libversion(void);`, unnamed parameters, `unsigned long long`, `void*`; rejects structs by value, `...`, function pointers and `#include`, each with its 16.3 message |
| `engines/spec`, `engines/registry` | the built-in rows; the spec, `syntax/scan` and config tables agree; version patterns; ts stripping versions; aliases; unknown-tag messages (`@pyhton`, `@PY`, `@c` in Phase 1); failures memoized, unknown tags not; `engine_available` never errors. Phase 3: every 17.1 message; the 17.2 conflicts (built-in, alias, config-declared, different spec); identical re-registration; reserved and invalid tags |
| `engines/resolve` (injected probes) | R41; relative and empty `PATH` entries skipped; `PATHEXT` order and accepted extensions; Unix `X_OK`; too old, failed, timed-out and unparsable probes skipped and recorded; configured command missing (no fallback) or not usable; venv paths and command/venv precedence; `py -3` last; ts from js (stripping versions, transpiler found or missing, no js); the not-found message; probe cache hits, keys, bad files and modes. Phase 2: Windows C/C++ order `cl` (Developer Prompt environment, else vswhere) → gcc → clang |
| `engines/process` (the unit-test binary re-runs itself as the child, its role chosen by `V2_HELPER`, so no Python or Node is needed) | the timer stack: only the top runs, paused time does not count, a fired timer fires once; `kill_tree` kills grandchildren; a timer kills a probe's tree and reports it; stdout and stderr read concurrently (1 MiB each); a grandchild holding stdout does not block; stdin input on its own thread; exit status text; spawn error text; Unix: exit observed without reaping, children lead their own session; Windows: children are in their own job |
| `engines/vep`, `engines/session` (generic over `BufRead`/`Write`, driven by an in-process scripted adapter thread) | replies echo ids and `out` arrives before `ret`; nesting to depth 64, depth 65 → RecursionError; releases batched onto the next frame, never sent from a drop path; non-JSON lines before `hello` forwarded to stderr, a non-frame line after it → ProtocolError; one `\r` before `\n` dropped; frames over 64 MiB refused in both directions; EOF → EngineCrashed; the watchdog paused while V2 serves a reverse call |
| `examples/fake_adapter.rs` | a real child process for crash and timeout cases. Its "language" is one JSON object per block, e.g. `{"answer": 42, "echo": "$0", "fail": "!boom", "say": "@out:hi"}`; `FAKE_MODE` selects `ok`, `garbage`, `vep2`, `nohello`, `noise` (startup lines before `hello`), `crash`, `slow`, `nocaps` or `idlecall` |

### 22.4 Mandatory regression list

| ID | Test | Asserts |
| --- | --- | --- |
| R01 | `engines/{py,js}/cr_args.v2` | `echo("a\rb\r\nc\u{2028}d")` returns the identical 8-character string; the next 50 calls `echo(i)` return `i` (no reply off by one) |
| R02 | `engines/{py,js}/float_kind.v2` | through py, `type(echo(1.0)) == "float"` and `@py(x: 1.0) { type(x).__name__ }` is `"float"`; through js, `echo(1.0)` is the int `1` and `@js { 1.5 }` is a float |
| R03 | `engines/{py,js}/nonfinite.v2` | NaN from `@py { float("nan") }` and from `@js { NaN }` satisfies `x != x`; ±inf round-trips through `echo` unchanged |
| R04 | `engines/py/nonstr_keys.v2` | `let h = @py { {1: "a"} }` gives `type(h) == "handle"` and `h[1] == "a"`; `value(h)` raises MarshalError `value(): can't convert dict to V2 data (key 1 is not a str)` (11.5 key rule); `value` of `{(1, 2): "a"}` raises MarshalError |
| R05 | `engines/py/decorators.v2` | decorator lines `@exporter` and `@export_json` reach Python untouched and the block stays an action block; lines `@@export { x }` and `@@import y` inside a triple-quoted string print as `@export { x }` and `@import y` |
| R06 | `engines/py/braces.v2`, `engines/js/braces.v2`, `compiled/c/braces.v2` | `@py { print("}") }` prints `}`; a JS template literal containing `${"}"}` works; `@c { char c = '{'; printf("%c", c); }` has value `"{"` |
| R07 | `host/syntax/unterminated.v2` | exit 1; stderr starts `unterminated.v2:3:` and contains `SyntaxError: @py block is never closed`, then the source line and a caret; stdout is empty although line 1 prints |
| R08 | `engines/py/def_in_loop.v2` | a definition block, its `@import` and `print(bump())` inside `for i in 0..3` print `loaded` once, then `1`, `2`, `3` |
| R09 | `host/try_locals.v2` | inside a function, locals declared before `try` survive a caught `throw`; a name declared inside `try` is gone after it; name lookup still works after 1000 caught throws |
| R10 | `engines/{py,js}/kwargs.v2` | `f(1, sep: "-")` arrives as a Python keyword argument; an unknown keyword raises ForeignError `TypeError`; a named arg to a JS function raises ArityError `js functions take positional arguments only (got 'sep')` |
| R11 | `engines/{js,py}/async.v2` | an exported `async function` returning 42 gives `42`; a rejected Promise raises ForeignError; a Python coroutine is awaited; a Promise returned in a nested call raises 9.8's Error |
| R12 | `engines/js/packages/` | `@import { join } from js.path` works; with `[node] modules = "fixtures/node_modules"`, `@import { twice } from js."v2-test-pkg"` loads the committed package (no network); a missing package raises ImportError carrying Node's message |
| R13 | `cross/tour.v2` | DOCS 17.1 verbatim, with `math_ops.py`, prints `22`, `20`, `44`, `47` |
| R14 | `engines/py/callbacks.v2` | DOCS 8.2 prints `["Linus", "Ada", "Grace"]`; DOCS 17.5 prints `1/3`, `2/3`, `3/3` before `process` returns; a ValueError thrown in the V2 callback is caught in V2 as ValueError |
| R15 | `engines/py/datetime.v2` | with `d = @py { import datetime; datetime.date(2024, 2, 29) }`: `type(d) == "handle"`, `d.year == 2024`, `str(d) == "2024-02-29"`, `d.isoformat()` equal; `d.replace(year: 2025)` raises ForeignError `ValueError`; `d == d`; `d + 1` raises TypeError; `len(d)` raises ForeignError `TypeError` |
| R16 | `engines/{py,js}/trace_lines.v2` | an exception raised on `.v2` line 12, inside a block opened on line 10, gives `e.line == 12` and a frame `(trace_lines.v2:12, in @py risky)`; uncaught, the header, source excerpt and caret have DOCS 10's shape |
| R17 | `cross/order/` | with stdout captured, program order holds for all of: V2 `print`, `@py` `print`, `@py` `sys.stdout.write("x")` with no newline, `@js` `console.log`, a `@c` action block, an exported `@c` function that calls `printf`, and a run-engine block |
| R18 | `engines/py/crash.v2` | a call running `os._exit(3)` raises EngineCrashed naming status 3; the next call raises EngineCrashed `is not running`; the script continues |
| R19 | `engines/py/timeout/` | with `call_timeout = 1`, `time.sleep(30)` raises EngineTimeout in under 5 s; the next call raises EngineCrashed |
| R20 | `compiled/c/cache/` (run.rs: two runs with one HOME) | DOCS 11.1 `hypot3(1, 2, 2)` prints `3.0`; the second run leaves every file under `~/.v2/cache/build` unchanged (paths, sizes, mtimes); with the cached library truncated to 0 bytes, a `--no-cache` run still prints `3.0` |
| R21 | `compiled/rust/panic.v2` | `count_primes(100000) == 9592`; an export that panics with `boom` raises ForeignError with `lang == "rust"`, `foreign_type == "panic"` and `boom` in the message; the next call works |
| R22 | `compiled/cpp/exception.v2` | `reversed("stressed") == "desserts"`; `reversed(null)` raises FFIValueError; `throw std::runtime_error("bad")` raises ForeignError with message `bad` and a `foreign_type` containing `runtime_error`; `throw 42` gives `foreign_type == "unknown"` |
| R23 | `ffi/libc_libm.v2` | through `extern "m"`, `cos(0) == 1.0` and `pow(2, 10) == 1024.0`; through `extern "c"`, `strlen("héllo") == 6` |
| R24 | `ffi/range.v2` | `abs(2147483648)` raises FFIValueError naming argument 1, `int` and `-2147483648..2147483647`; a `@c` `unsigned char id8(unsigned char x)` called with 300 raises FFIValueError; `strlen("a\0b")` raises the NUL FFIValueError; a `@c` export returning `UINT64_MAX` as `uint64_t` raises FFIValueError `return value 18446744073709551615 of '…' does not fit in a V2 int` |
| R25 | `ffi/not_found.v2` | `extern "v2_no_such_lib"` raises FFILoadError listing exactly this platform's 16.2 search order; on Windows (run.rs), a DLL present only in the cwd, with the script elsewhere, is not found |
| R26 | `imports/ambiguity.v2` | with two `@py` blocks exporting `mean`, a named import and a `*` import from `@py` both raise the DOCS 6.3 message; `@py.stats` resolves; an `@export { * }` block can be imported selectively |
| R27 | `imports/aliases.v2` | `@python stats` is reachable as `stats`, `@py.stats` and `@python.stats`; `python.statistics` is `py.statistics`; a `@python` block and a `@py` block share one process (a `sys` attribute set in one is visible in the other) |
| R28 | `host/syntax/dup_label.v2` | `@py stats` followed by `@js stats` is a SyntaxError at the second block, with a note at the first |
| R29 | `engines/js/bigint.v2` | `echo(2**60) == 2**60` through JS; `@js(x: 2**60) { typeof x }` is `"bigint"`; `@js(x: 2**53 - 1) { typeof x }` is `"number"`; `@js { 1e20 }` follows chapter 10 without overflow |
| R30 | `engines/js/proto_key.v2` | `@js(d: {"__proto__": {"x": 1}}) { d.x === undefined && Object.keys(d).length === 1 }` is `true`; `echo` returns an equal dict |
| R31 | `engines/{py,js}/surrogates.v2` | a lone surrogate from `@py { "\ud800" }` or `@js { "\ud800" }` follows chapter 10, with no panic; the session keeps working |
| R32 | `host/stdin_owner.v2` with `.in` = `a\nb\n` | `@py { import sys; sys.stdin.read() }` raises ForeignError (9.9's stdin error) and `@js { require("fs").readFileSync(0) }` throws; then `input()` returns `a`, then `b`, then `null` |
| R33 | `engines/js/idle_timer.v2` | a `setImmediate` callback scheduled during a call, calling an imported V2 function, runs in the event-loop turn after that call's reply (14.4) and throws 9.8's idle-call Error in JS; the V2 function never runs; the stored callback still works during a later call |
| R34 | `engines/py/subprocess.v2` | in a block, `subprocess.run([sys.executable, "-c", "print('child')"])` and `os.write(1, b"x\n")` put `child` and `x` on stderr, not stdout; the next call works (protocol intact) |
| R35 | `custom/run/` (requires js) | with `[engines.nd]` set to `kind = "run"`, `command = ["node", "{file}"]`, `ext = "js"`: statement output is in order; `@nd(t: "x y", n: [1, 2]) { process.stdout.write(process.env.V2_ARG_t + process.env.V2_ARG_n + "\r\n") }` has value `x y[1,2]`; `console.error("bad"); process.exit(3)` raises EngineExitError with `e.data.status == 3` and `bad` in `e.data.stderr`; `@nd(p: 1, P: 2) {}` raises ValueError; `@export` inside raises ImportError |
| R36 | `custom/register.v2` | `register_engine` with `"py"`, `"python"` or a tag declared in the test's `v2.toml` raises EngineNameConflict; `"v2"` and `"Lua"` raise ValueError; an identical re-registration returns `null`; a different spec raises EngineNameConflict; a block reached before registration raises EngineUnknown, one reached after it runs; a misspelled key raises ValueError with a did-you-mean |
| R37 | `custom/bridged/` (run.rs) | `v2 doctor fake` passes every check; export, call, `out` and `err` work; `FAKE_MODE` values `garbage`, `vep2`, `nohello`, `crash` and `nocaps` each produce their chapter 21 row; `noise` still starts, with the noise on stderr |
| R38 | `cross/depth.v2` | V2↔py mutual recursion reaching depth 64 succeeds; depth 65 raises RecursionError, which V2 catches |
| R39 | `host/bom_crlf.v2` | stored with a BOM and CRLF line endings (`.gitattributes`: `-text`); it runs, and a `@py` error in it reports the right line |
| R40 | `engines/py/interrupt/` (run.rs, unix) | SIGINT, and separately SIGTERM, sent to the host during a sleeping py call exits 130 within 2 s; the worker pid is gone and the run dir is removed |
| R41 | `engines/resolve` unit test | a WindowsApps `python.exe` whose probe exits 9009 is skipped and the next candidate is used; one whose probe prints `Python 3.12.1` is kept |
| R42 | `engines/ts/strip.v2`, `engines/ts/old_node.v2` | a `@ts t` block exporting `function dbl(x: number): number` gives `dbl(21) == 42`, and a `@js` block imports `dbl` from `t`; on Node below 22.13 without a transpiler (minimum-version jobs), `engine_available("ts")` is `false` and a `@ts` block raises EngineNotFound naming 22.13 |
| R43 | `engines/{py,js}/export_star.v2` | `@export { * }` exports the block's `def`s/`function`s and classes, but not a name brought in by `from math import sqrt` (py) |
| R44 | `imports/module_names.v2` | `@import { x } from py."os; print(1)"` raises the invalid-module-name ImportError and prints nothing |
| R45 | `host/optchain.v2`, `host/syntax/optchain_assign.v2` | with `let a = null`: `a?.b.c(f())` is `null` and `f` never runs; `(a?.b).c` raises TypeError; `{"b": null}?.b.c` raises TypeError; `{}?.x` raises KeyError; `a?.b = 1` is a SyntaxError |
| R46 | `engines/py/def_fail.v2` | a definition block that prints `loading` and then raises ValueError, reached twice in a loop inside `try`: both catches get ForeignError `ValueError` and the same error value (`e1 == e2`), and `loading` prints once; `@import` from it raises ImportError `block 'bad' failed to load: …` |
| R47 | `custom/run/tree_kill.v2` (requires js) | a run-engine block that starts a non-detached child which would write a marker file after 1 s returns at once; 2 s later the marker doesn't exist |
| R48 | `host/exit_codes/` (run.rs) | `exit(7)` exits 7 without running an enclosing `finally`; a SyntaxError exits 1; an invalid `v2.toml` exits 2; an uncaught EngineNotFound (`[engines.py] command` naming a missing file) exits 3, and so does `v2 check` on that file; `exit(2**40)` raises ValueError |
| R49 | `host/modules/import_syntax.v2` | importing a module with two syntax errors raises ImportError at the `import`, with the first diagnostic as its message and both in `e.data` |

### 22.5 Seed and cleanup

- Port `old/v2/testsuite/test_engines.v2` to `tests/engines/py/seed_engines.v2`; the appendix lists the changes.
- Don't port the other old test files. They test removed features and contain tautologies such as `check("custom lang engines", true, true)` (`old/v2/testsuite/ch_test2.v2:245`) and `check("embedded lang engines", true, true)` (`test_batch21.v2:515`); the 22.2 lint keeps new ones out.

### 22.6 CI matrix

| Job | Python | Node | C / C++ | `V2_TEST_REQUIRE` |
| --- | --- | --- | --- | --- |
| windows-latest, MSVC | 3.13 | 22 (≥ 22.13) | `cl` found via vswhere, outside a Developer Prompt | py,js,ts,c,cpp,rust |
| windows-latest, MinGW | 3.8 | 18 | MinGW-w64 UCRT gcc/g++, set by `V2_ENGINE_C`/`V2_ENGINE_CPP` (`cl` would win otherwise) | py,js,c,cpp,rust |
| ubuntu-latest | 3.13 | 22 (≥ 22.13) | gcc/g++ | py,js,ts,c,cpp,rust |
| ubuntu-22.04, minimum versions | 3.8 | 18 | clang/clang++, set by `V2_ENGINE_C`/`V2_ENGINE_CPP` | py,js,c,cpp,rust |
| macos-latest | 3.13 | 22 (≥ 22.13) | Apple clang | py,js,ts,c,cpp,rust |

- **Build and test.** Every job builds V2 with stable Rust and runs `cargo test`. On Windows the host target is `x86_64-pc-windows-msvc`, which also proves that libffi builds with MSVC. `@rust` blocks use stable `rustc`.
- **Unusual paths.** The MSVC job checks out the repo and sets `HOME`/`USERPROFILE` under a path with a space and a non-ASCII character (`C:\v2 tést\`).
- **MinGW runtime.** The MinGW job runs `tests/compiled` with MinGW's `bin` folder removed from `PATH`, proving the static runtime linking of 15.7.
- **`ts` on minimum versions.** Built-in type stripping needs Node 22.13+ (or 23.2+), so Node 18 jobs don't require `ts` and run R42's old-Node case instead.

---

## 23. Build order

Each phase is done when its chapter 22 tests pass on the whole CI matrix. Nothing from a later phase is stubbed: a builtin that doesn't exist yet is a NameError, and an unregistered tag is EngineUnknown.

| Phase | Build | Done when |
| --- | --- | --- |
| 0. Host core | `syntax/*` (spans, tokens, lexer with BOM/CRLF, scan profiles, fence, directives, parser, AST, static checks); `runtime/*` (values, environments, interpreter, call binding, errors and their rendering, modules, builtins); `codec/json`, which also backs `json_parse` and `json_stringify`; `config/` (TOML layers and precedence); `cli/` (`v2 FILE`, `run`, host-only `check`, `--version`, `--help`, exit codes and the interrupt path of 6.5). Reaching a block raises EngineUnknown, because the registry is empty | `tests/host`; the scan, directives and codec unit tests; R07, R09, R28, R39 (host part), R45, R48 (host part), R49; the chapter 21 Syntax and Host rows |
| 1. py, js, ts | `engines/{process, spec, registry, resolve, session, vep, units, imports, handles}` (the registry holds py, js and ts; c, cpp and rust join in Phase 2); `adapters/{py.py, js.js}`; the watchdog timers (8.5); error line mapping; `[engines.*]`, `[python]`, `[node]`; `doctor/` (the table and `v2 doctor TAG` for built-in bridged engines); the full `check` inventory; `value` and `engine_available` | `tests/engines/{py,js,ts}`, `tests/cross`, `tests/imports`, the seed and the protocol tests; R01–R05, R06 (py/js), R08, R10–R16, R17 (py/js), R18, R19, R26, R27, R29–R34, R38, R40–R44, R46, R48; the chapter 21 Setup, Blocks and imports, and Calls and values rows |
| 2. Compiled blocks and `extern` | `build/{mod, cache, c, cpp, rust, shims}` (MSVC and MinGW); `ffi/{cdecl, types, lib, call}`; `v2.h` and the Rust `v2` module with `host_call` reverse calls (15.9); the `engines/runonly` process helper that program mode uses (spawn, capture, tree kill, EngineExitError, 15.5); `v2 doctor TAG` for c, cpp, rust; `v2 cache clean` and `--no-cache` | `tests/compiled` and `tests/ffi`; R06 (C), R17 (native part), R20–R25; the Build and FFI rows |
| 3. Custom engines | run engines on that helper (17.3, 17.4); bridged custom specs through `engines/session`; `register_engine`; `v2 doctor TAG` for custom tags; `examples/fake_adapter.rs` | `tests/custom`; R17 (run-engine part), R35–R37, R47; the custom-engine rows |

---

## Appendix: salvage map from old/

All paths are under `old/v2/src/` unless they say otherwise.

**Port, with changes:**

| Old code | New home | Changes |
| --- | --- | --- |
| `engines.rs:64-80` `dedent` | syntax/directives | Computes the common whitespace prefix by characters instead of slicing by a byte count, so multibyte whitespace (NBSP) no longer panics. The mixed-indentation error and the trailing newline follow 4.4. Runs after CRLF-to-LF |
| `engines.rs:86-119` `extract_directives` | syntax/directives | Matches exact tokens only: `@export`/`@import` followed by whitespace, `{` or the end of the line, so `@exporter` is left untouched. Lists may span lines. `@@export`/`@@import` lose one `@`. `@import` is parsed, not stripped with a warning. Names are validated. Each directive line becomes an empty line, so line numbering holds |
| `engines.rs:21-43` `normalize_lang` | engines/spec | Becomes the built-in data table (7.2): py/python, js/javascript, ts/typescript, c, cpp/cxx, rust/rs only. `canonical_tag` (46-60) and its `"other"` bucket are dropped |
| `engines.rs:396-404` `Drop for EngineWorker` | engines/session | Same order (announce shutdown, stop, wait, delete files), but it sends `bye`, waits up to 1 s and only then runs `kill_tree` (8.7). The old code killed right after writing `exit` |
| `engines.rs:185`, `engines.rs:340` Python env | engines/session (py spawn) | `PYTHONUTF8=1`, `PYTHONIOENCODING=utf-8`, unchanged |
| `lexer.rs:13-14` BOM strip | syntax/lexer | Adds CRLF and lone-CR to LF normalization (3.1). Spans are computed on the normalized text |
| `native.rs:14-34` `find_cc`, plus the `cl` branch at `native.rs:44-60` | engines/resolve, build/c, build/cpp | The probe parses `--version` output (it feeds the cache key) instead of only checking `is_ok()`. Candidate order follows 7.2: on Windows `cl` first, then gcc, then clang. `cl` comes from a Developer Prompt environment, else from vswhere with the captured vcvars environment (7.6), not bare PATH. Library mode builds a shared library (`/LD`, `-shared`; 15.7) instead of an exe. `tcc` is dropped. `.obj` cleanup is kept, inside the build's work dir (15.6) |
| `environment.rs:169-208` `did_you_mean`, `levenshtein` | runtime/error | Becomes optimal-string-alignment distance over any iterator of candidate names, serving NameError, EngineUnknown tags, config keys and engine-spec keys; thresholds per 5.9 (tags: 7.3) |
| `engines.rs:406-422` `json_quote`, `interpreter.rs:6938-6995` `json_unescape` | none (serde_json) | Not ported. Their inputs (`\r`, `\u0001`, quotes, surrogate pairs) become `codec/json` unit tests |
| `hashing.rs:361-434` `sha256` | none (`sha2` crate) | Not ported |
| `old/v2/testsuite/test_engines.v2:1-111` | tests/engines/py/seed_engines.v2 | `var` becomes `let`. The probe `try`/`catch` becomes `// requires: py`. `caught.contains(x)` becomes `x in caught`, and `str(e)` becomes `e.message`. The missing-export check asserts `e.type == "ImportError"` and the 12.2 message |

**Do not port:**

| Old code | Why |
| --- | --- |
| `compiler.rs`, `vm.rs`, `bytecode.rs`, `serialize.rs` | The bytecode VM is removed (DOCS 1, 18); V2 has one tree-walking interpreter |
| `interpreter.rs:12014-12063` `__*` stub catch-all, `interpreter.rs:633-637` `make_stub_module`, and the `build_std_*_module` builders | They fabricate return values, and `std.*` is removed. V2 fails loudly instead |
| `interpreter.rs:7089-7110` `value_to_json`, `interpreter.rs:6997-7087` `parse_json_value` and its helpers | Lossy: `1.0` becomes `1`, NaN breaks, `\r` is sent raw. Replaced by `codec/json` on serde_json |
| `engines.rs:426-444` `extract_json_string_array` | A substring scan. `hello` is parsed by serde_json |
| `parser.rs:1061-1088` legacy embedded-block path; `lexer.rs:24-99` `ENGINE_TAGS` and naive brace capture | A fixed tag list and brace counting. Replaced by runtime tag lookup (7.3) and scan profiles (3.8) |
| `engines.rs:232-279` worker suffixes; `engines.rs:121-230` temp paths, commands and run-only prelude | An id-less protocol using `eval(fn)`, predictable temp names, hard-coded commands (including `-ExecutionPolicy Bypass`), and a cwd `sys.path` insert. Replaced by the VEP/1 adapters, the private run dir and the resolver |
| `native.rs:72-893` V2-to-C transpiler; `fault.rs` | Not polyglot, and v1 has no native crash handler (DOCS 11.7) |

---

## Decision log (D1–D75)

The decisions that settled every open question while these documents were written. Chapters cite
them as `Dnn`. They are a record of *why*: where a chapter refines a decision with more detail, the
chapter is normative.

### Syntax & lexing
- D1 SyntaxError: parse/static errors are diagnostics of kind `SyntaxError` (`file:line:col: SyntaxError: msg`
  + caret excerpt), reported before anything runs, exit 1. In an `import`ed V2 module they surface in the
  importer as `ImportError` (message = first diagnostic, data = all). SyntaxError is listed with the error
  kinds but cannot be caught in the file that contains it.
- D2 An uncaught error's header location and caret point at the `throw` (first throw site; `error()` leaves the
  span empty, `throw` fills it only if empty, rethrow keeps the origin). Diagnostics for `.v2` locations always
  show a source excerpt, including ForeignError.
- D3 Built-in tags get language-aware body scanning (python profile for py; c-like for c/cpp/rust/js/ts).
  EVERY other tag (custom engines) uses the Plain profile (brace counting only). Lexing never reads config.
  The fence form is the escape hatch. Reasons to use a fence include regex literals, preprocessor lines with
  unbalanced braces, heredocs, unusual string forms.
- D4 Scanner guards: in a multi-line block the closing `}` must be the first non-blank token on its line; after
  the closing `}` only `)` `]` `}` `,` `;` or a comment may follow on that line. Violations are SyntaxErrors with
  a fence hint.
- D5 `@@export` and `@@import` at line start are unescaped to literal `@export`/`@import` lines.
- D6 The body `{` of if/elif/else/while/for/func/lambda/try/catch/finally must be on the same line as its header.
  Newlines before `elif`/`else`/`catch`/`finally` are allowed.
- D7 Triple-quoted strings (Swift-style): text starts on the line after the opening `"""`; the closing `"""` is on
  its own line and its indentation is removed from every line; the newline before the closing line is dropped
  (no trailing newline). Escapes are processed (except r"""). f""" allowed.
- D8 Integer literals: decimal (with `_`) and hex `0x` only; `0b`/`0o` and leading zeros are SyntaxErrors; a number
  immediately followed by an identifier char is a SyntaxError; overflow of i64 is a SyntaxError. In str `\xNN`
  is U+00NN; in bytes it is the byte.
- D9 Block header: the `{` or fence must be on the same line as the header's LAST token (args may wrap lines).
- D10 `@import SEL as NAME` (whole-module handle) is only valid with a package selector `lang.module`;
  otherwise SyntaxError.
- D11 Quoted items in `@export`: in compiled blocks a quoted item is a C prototype; in py/js/ts blocks a quoted
  item is a name that is not a V2 identifier (e.g. JS `$x`). V2 keywords may appear bare in @export lists.
- D14 `?.` short-circuits the whole postfix chain like JS: in `a?.b.c()` a null `a` makes the whole chain null;
  parentheses end the chain; assigning through `?.` is a SyntaxError. Missing dict keys are still KeyError.
- D17 A function's parameters, a for-loop's variables and a catch name are declared in the body they introduce,
  so `let x` of the same name directly in that body is a redeclaration SyntaxError (inner blocks may shadow).

### Host runtime
- D15 Exit codes: 0 ok; 1 uncaught error or SyntaxError; 2 usage/config error; 3 uncaught EngineNotFound (also
  `check` finding a required engine missing, and `doctor TAG` not finding it); 130 interrupt (Ctrl+C, Ctrl+Break,
  console close, SIGINT/SIGTERM/SIGHUP — always 130, not 128+signo); 101 internal panic (bug). `exit(code)` and
  interrupts do NOT run `finally` blocks; engines are still shut down and temp dirs removed. exit code must be
  an int in i32 range (POSIX keeps the low 8 bits).
- D20 `import "./m.v2" as m` binds a dict snapshot of the module's exported names (type "dict"), shared by all
  importers; rebinding a module-level `let` after import is not visible through it.
- D21 Calling `d.name(args)` on a dict: if key `name` exists and holds a func, that func is called; otherwise the
  dict method. (So `ns.keys(...)` from `extern ... as ns` works when the C function is named keys.)
- D22 `.copy()` is shallow.
- D23 Division or `%` by zero (int or float) and zero to a negative power raise `ValueError: division by zero`.
  Integer overflow raises `ValueError: integer overflow in '<op>'`.
- D24 Compound assignment `x op= y` always means `x = x op y` (so `xs += [1]` makes a new list).
- D25 `input()` returns null at end of input.
- D26 Relative paths in read_file/write_file resolve against the process working directory; `import` and
  `extern` library paths resolve relative to the .v2 file.
- D28 `in`, slicing, arithmetic and ordering on handles raise TypeError with a "do it inside a block / value(h)"
  hint. `for x in h` and `for i, x in h` work.
- D29 `for` over a list/dict iterates a snapshot taken when the loop starts.
- D41 `engine_available(tag)`: unknown tag -> false (no error); may run (cached) version probes; never starts a
  session or compiles; later crashes don't change it.
- D75 Lists get `get(i, default = null)` (negative i allowed; out of range -> default).

### Blocks & imports
- D13 V2 never inserts V2 VALUES into foreign code. Your code is passed as written (common indentation removed,
  directive lines blanked); the only text V2 adds is generated wrappers/shims for compiled blocks.
- D18 Compiled program-mode blocks and `run`-engine blocks: in statement position their stdout streams to V2's
  stdout in order (host flushes first); in expression position stdout is captured as the value (one trailing
  `\n` or `\r\n` stripped). stderr always streams live; the last 4 KiB are kept in `e.data.stderr` of
  EngineExitError (not repeated in the message).
- D30 Block args of a definition block are evaluated only when it loads (first reach); later reaches ignore them.
- D31 A definition block whose load fails is marked Failed: reaching it again re-raises the same stored error
  (no retry, no repeated side effects); `@import` from it says `block 'x' failed to load: <message>`.
  Unlabeled blocks are named `@py block at file:line` in messages.
- D32 Block args on built-in compiled tags (c, cpp, rust + aliases) are a SyntaxError:
  `compiled blocks take no args — @export a function and call it with arguments`.
- D53 V2-level `@import` must come after the block it imports from has run (else ImportError "block 'x' has not
  run yet — move this @import below it"). `@import` INSIDE blocks resolves lazily at first call, so blocks may
  import from each other in any order.
- D56 Constant exports (`@export { VERSION }`) apply to py/js/ts only; compiled blocks export functions only. In
  compiled blocks `@export { * }` exports non-static, non-inline functions except `main`, whose signatures V2 can
  convert; others are listed by `v2 check`.
- D57 `@import` inside a compiled program-mode block (no @export) is a BuildError. Inside compiled blocks only the
  `{ names } from SEL` form is supported (no `*`, no module-as-handle).
- D58 A compiled action block without `main` may contain statements only (no helper functions); to define helpers,
  write `main` yourself.

### Engines & tooling
- D33 TypeScript needs Node 22.13+ (or 23.2+) for built-in type stripping of in-memory source
  (`module.stripTypeScriptTypes`), or a configured transpiler. Doctor/engine_available decide ts from the probed
  Node version.
- D34 The deep engine test is `v2 doctor TAG` (positional), not `v2 doctor --engine TAG`. `--engine TAG=PATH`
  remains the global override flag.
- D35 Windows C/C++ toolchain order: MSVC `cl` (from the environment, else found via vswhere with captured vcvars
  env) first, then gcc, then clang. Elsewhere: cc/gcc/clang (c), c++/g++/clang++ (cpp). `[engines.c] command`
  overrides.
- D36 `[engines.ts] transpiler`: `"node"` (built-in stripping) or a command; a string other than "node" is one
  program path (never split), an array is an argv; TS source on stdin, JS on stdout; non-zero exit -> ForeignError.
- D37 In v2.toml every key is optional EXCEPT that a custom `[engines.<tag>]` needs `kind` and `command` (a run
  command must contain `{file}`). `ext` defaults to the tag.
- D38 Relative path-like elements in an engine `command` (contain a separator, don't start with `-`, contain no
  `{`) resolve against the folder of the config file that declares them.
- D39 `call_timeout` applies to bridged sessions (load/module/call requests), run engines and compiled programs.
  In-process calls (compiled exports, extern) cannot be interrupted and ignore it.
- D40 EngineNotFound also covers: too-old versions, executables that cannot start, missing configured
  command/venv/transpiler, Node too old for ts stripping.
- D42 Custom tag syntax `[a-z][a-z0-9_]{0,31}`; reserved: `v2`, `export`, `import`. Registering a tag that is
  built-in, an alias, declared in config, or already registered with a different spec -> EngineNameConflict;
  identical re-registration is a no-op.
- D43 PATH walking skips relative PATH entries (e.g. `.`), so a project can't shadow toolchains via the cwd.
- D44 Processes started by a block (run engines, compiled programs, and their children) are killed as a tree when
  the block finishes, times out, or V2 exits. Workers are killed as a tree at shutdown.
- D45 Output caveat: language-level output (print, console.log, process.stdout.write) from py/js/ts goes through
  `out` frames in program order. Fd-level output (C extensions, os.write(1), os.system/subprocess or
  child_process children inheriting stdout) goes to stderr: the Python adapter points fd 1 at fd 2 after moving
  the protocol to a private fd; the Node adapter wraps fd 1/fd 0 users instead (9.9, 14.4). Adapters buffer
  output produced while idle and send it with their next frame.
- D46 Callback rule: a foreign stub/callback may call into V2 only while the host has a request outstanding to
  that engine (i.e. V2 is currently waiting on it). Calls attempted while the engine is idle (timers, threads,
  event handlers firing later) fail inside the foreign language with an error. Stored callbacks remain valid
  for later requests.
- D47 Names brought in by `@import` are `func`. Foreign functions, bound methods and classes RETURNED as values
  become callable handles (type "handle").
- D48 Only exact tuple/list/dict convert; namedtuples and list/dict subclasses (OrderedDict, Counter, ...) come back
  as handles.
- D49 `value(h)`: JSON-convertible containers convert recursively; else `to_dict()` is tried first, then `tolist()`;
  else MarshalError.
- D52 `ts.pkg` selectors are the same as `js.pkg` (same session, same module key).
- D54 Node resolution: `require` is created for the .v2 file's path (standard Node lookup walks up node_modules from
  the script folder, so a project-root node_modules is found), plus `[node] modules` and `[node] paths` via
  NODE_PATH; ESM-only packages fall back to import(). `@import { "default" as x } from js.pkg` and
  `@import js.pkg as x` (callable module handle for CommonJS function exports) are supported.
- D55 `process.exit()` inside a JS block throws an Error (the worker survives); Python `sys.exit()` becomes a
  ForeignError with foreign_type SystemExit; Python blocks' `__name__` is not "__main__".
- D60 Native library search (extern): `[ffi] search_paths`, the .v2 file's folder, the project root, then the system
  default search. On Windows the system step includes PATH but never the current directory
  (SetDllDirectoryW(L"") at startup; full paths via LoadLibraryExW with LOAD_WITH_ALTERED_SEARCH_PATH).
- D61 In `extern`, a non-const `char*` is an opaque pointer; in compiled blocks a returned `char*` is a malloc'd
  string V2 copies and frees.
- D62 `const char*` parameters accept `null` as NULL; `std::string`, `&str`, `String` parameters reject null with
  FFIValueError.
- D63 Named arguments to native functions (compiled exports, extern) and to JS/TS functions -> ArityError
  ("... take positional arguments only").
- D64 Unsigned results above i64::MAX and returned strings that aren't valid UTF-8 -> FFIValueError.
- D65 No `source = "stdin"` option for run engines in v1 (only `{file}`).
- D66 Stdout captured as a block value has one trailing `\n` or `\r\n` stripped.
- D67 Windows note for bash custom engines: bare `bash` may resolve to the WSL launcher; configure Git Bash's full
  path. Run engines with ext bat/cmd are written with CRLF line endings.
- D68 A bridged custom engine supports whatever capabilities its adapter declares in hello (modules, handles,
  kwargs, reverse); missing caps give defined errors.
- D69 `v2 doctor` runs toolchain commands (including project-configured ones); only `v2 check` executes nothing.
- D70 NO POSIX ownership/permission refusal for v2.toml or ~/.v2 (drop that rule).
- D71 = D18 stderr shape for EngineExitError, for both run engines and compiled programs.
- D72 Maximum VEP frame size 64 MiB in both directions.
- D73 = D45: Python adapter dup2(2, 1) after moving the protocol to a private non-inheritable fd; fd 0 -> devnull;
  language-level stdin replaced with a stream that raises. Node has no dup2, so its adapter keeps the protocol on
  fds 0/1 (saved fs.writeSync(1)/fs.readSync(0)) and wraps process.stdout/stdin, console, fs and child_process
  (9.9, 14.4).
- D74 Before `hello`, the host tolerates non-JSON lines on a worker's stdout (sitecustomize/.pth/--require
  preload noise) and forwards them to stderr; after hello any non-frame line is a ProtocolError. Host drops one
  CR before LF when reading.

### DOCS example fixes (for reference)
- D50 DOCS 8 example: `xs.map(x => square(x))`.
- D51 DOCS 17.4: `@import js.slugify as slugify` (callable module handle).
- D27 DOCS 3.6 comments without quotes.
- D2 DOCS 3.9 caret under `throw`; DOCS 10 ForeignError example shows an excerpt.
- D75 DOCS 2 hello: `args().get(0, "stranger")`.
