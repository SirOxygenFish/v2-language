# V2 Internals — how everything works

> **Status: being built;** [`ROADMAP.md`](ROADMAP.md) "Where we are" says which parts work. [`DOCS.md`](DOCS.md) defines what V2 does and this file how it works; where the two disagree, DOCS.md wins and this file is fixed.

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
- **Main thread.** Parses argv, loads config, starts the watchdog, installs the interrupt handler, then spawns the interpreter thread (stack size: ch. 5) and joins it (6.1).
- **Interpreter thread.** The logical main thread: the only thread that touches `Value` (`Rc`/`RefCell`, not `Send`), session pipes, native libraries and `Out`, and the only one that spawns processes (8.3). It serves a worker's calls into V2 while it waits for that worker's reply (9.7, 9.8), and compiled code calls V2 on it, during the native call (15.9). Compiled and `extern` code runs on it, so APIs that insist on the process main thread (macOS AppKit) do not work in v1; host functions that native code calls wrap their bodies in `catch_unwind`.
- **Watchdog.** Enforces the startup, call, probe and grace deadlines by killing child trees (8.5), and carries out the interrupt path (6.5).
- **Interrupt handler.** Ctrl+C, Ctrl+Break, console close, SIGINT, SIGTERM, SIGHUP and SIGQUIT are handled by the host alone: the handler sets `INTERRUPTED` (on Windows it also wakes the watchdog and then blocks), and the watchdog ends the run (6.5).
- **Output order.** One writer, `Out` (5.10), carries `print` and worker `out` text and is flushed before native calls and child spawns, so language-level output of every engine interleaves in program order (9.9, 16.7).

### 1.4 Dependencies
| Crate | Used for | Why a crate |
| ----- | -------- | ----------- |
| `serde_json` (`preserve_order`, `float_roundtrip`, `unbounded_depth`) | codec, VEP frames, JSON builtins | Strict parsing (it rejects lone-surrogate escapes), escaping of every control character, exact float round-trip. The codec bounds nesting itself (10.1). |
| `toml` (`preserve_order`) | `v2.toml` | Full TOML. The file is read as `toml::Table` with hand-written key checks, so "unknown key" errors are exact (18.5); `preserve_order` keeps the first error in file order. |
| `indexmap` | dict storage | Insertion order with O(1) lookup (DOCS 3.2). |
| `libloading` | compiled blocks, `extern` | Portable `dlopen`/`LoadLibraryExW` that reports the OS error text (15.8, 16.2). |
| `libffi` | calls whose signature is known only at runtime | Per-ABI call frames (SysV, Win64, AAPCS64). The crate builds a vendored libffi, so building v2 needs a C compiler (and `ml64` on MSVC); the binary has no runtime dependency. |
| `sha2` | build and probe cache keys (15.6, 7.7) | Standard SHA-256. |
| `libc` | `kill`, `setsid`, `sigaction`, `waitid`, `prctl(PR_SET_PDEATHSIG)`, `getentropy`, `fflush`, `_exit` | Direct OS calls. |
| `windows-sys` | Job Objects; `CREATE_NEW_PROCESS_GROUP`; `SetConsoleCtrlHandler`; the run-dir DACL and `BCryptGenRandom` (20.3); `SetDllDirectoryW` and `LoadLibraryExW` flags (16.2, 15.8) | Direct OS calls. |

Not used: an async runtime, a CLI or regex crate (the few patterns in ch. 7 and 15 are hand-written matchers), `rand`, serde derive.

---

## 2. Source, spans and diagnostics
### 2.1 Source files
```rust
pub struct FileId(pub u32);        // index into SourceMap::files; the entry script is 0
pub struct SourceFile {
    pub path: PathBuf,             // fs::canonicalize with `\\?\` stripped: the dedupe key, and the path adapters get (12.2)
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
- **Excerpt.** Printed whenever the location is in the SourceMap, for every error kind, `ForeignError` included.
  - Leading whitespace is removed, the rest is indented 4, and each tab prints as one space. The caret follows `4 + (col - 1 - removed)` spaces.
  - A line longer than 120 chars is cut to a 120-char window around the caret, with `...` at each cut end.
  - In multi-line messages (compiler output, foreign messages), the first line goes in the header and the rest follows the caret, indented 4.
- **Notes and hints** come only from static diagnostics (`SyntaxError`, config errors). A runtime error that needs a hint carries it in its message.
- **Cause.** When `cause` is set, it prints as `caused by:` blocks, at most 8 levels deep.
- **Frames** (2.5) print innermost first. When there are more than 100, the first 50 print, then `  ... N frames omitted`, then the last 20. `e.trace` keeps all of them (5.9).
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
| foreign frame in a foreign file | `mean (/usr/lib/python3.12/statistics.py:430, in py)` |

- **Frame lines.** The innermost frame's line is where the error was raised; every other frame's line is the call it is executing.
- **Names.** Tags always print in canonical form (`@py`, even when the source says `@python`). Foreign function names print as the engine reports them (`<module>`).
- **Parse errors** read `expected <what>, found <token>`, with tokens named as: name `x`; keyword `let`; number `42`; string / f-string / bytes literal; an operator or bracket in backticks; end of line; `;`; end of file; block `@py stats`; block body; `@import`; `@export`; a selector tag `@py`.
- **Message style.** Messages start lower-case and have no final period. They put names in `'…'` and code in backticks, as DOCS does. Rust `Debug` output never appears.

---

## 3. Lexer
### 3.1 Input
`SourceMap::load` does three things:
- It rejects invalid UTF-8 (`file is not valid UTF-8`, at the line of the first bad byte).
- It strips one leading U+FEFF, which Windows editors and PowerShell add.
- It turns every CRLF and every lone CR into LF.

The normalized text is what gets stored, spanned, scanned and handed to engines, so block bodies always use LF. (Run files with ext `bat`/`cmd` are written with CRLF, 17.3.) The lexer turns the whole file into one `Vec<Token>` before parsing starts.
### 3.2 Tokens
Every token carries a `Span` (2.2). Besides literals, names, keywords and operators, the lexer emits `Newline` and `Semi` (3.3), `Eof`, and the `@` tokens `DirImport`, `DirExport`, `AtTag`, `BlockHead`, `BlockBody` and `ExternBody` (3.6–3.9).
- **Keywords** (DOCS 18, case-sensitive): `let const func return if elif else while for in not break continue try catch finally throw import as from extern true false null`. A keyword directly after `.` or `?.` lexes as `Ident` (`h.from`, `result.in`), so every rule below sees a member name there.
- **Operators**, longest match first: `** ... .. ?. ?? => == != <= >= && || += -= *= /= %=`, then `+ - * / % = < > ! ? : . , ( ) [ ] ;`.
  - `&`, `|`, `^`, `~`, `<<` and `>>` are errors: "V2 has no bitwise operators".
  - Any other character is `unexpected character '#'`; for `#` the hint says comments start with `//`.
- **Identifiers** start with `_` or a char for which `char::is_alphabetic` is true, and continue with those or ASCII digits. They are compared by code point, with no normalization.
- **Comments.** `//` runs to the end of the line. `/* */` does not nest, and an unterminated one is an error at its start. A block comment that contains a line break counts as one line break.
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
- **A body `{` must be on its header's line.** This applies to `if`, `elif`, `else`, `while`, `for`, `func` (both forms), `try`, `catch` and `finally`. After a `Newline`, a `{` is a Dict brace, so the parser reports `expected {` with the hint "put { at the end of the if line".
### 3.4 Numbers
- **Integers.** `0`, `[1-9][0-9_]*`, or `0x` followed by hex digits. `_` is allowed only between two digits. These are errors: leading zeros (`007`), `0b`, `0o`, and a number followed directly by an identifier character (`12abc`, `0x`, `0X1`, `1f`).
- **Floats.** Digits `.` digits with an optional exponent, or digits followed by an exponent (`e|E [+|-] digits`).
  - A `.` joins the number only when a digit follows it. So `0..3` lexes as `Int .. Int`, `1.x` is a member access, and no float starts with `.`.
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

TAG is ASCII `[A-Za-z_][A-Za-z0-9_]*` and case-sensitive. The lexer never checks it: `@perl` lexes whether or not an engine exists. A tag that is unknown, or not a valid engine tag (`[a-z][a-z0-9_]{0,31}`, 17.1; so `@PY` too), raises `EngineUnknown` only when its block is reached (ch. 7). V2 has no decorators: `@memo` followed by `func` fails as a block header, with a hint that says so.
### 3.7 Block heads and raw bodies
The header is `'@' TAG [LABEL] ['(' args ')'] ( '{' | FENCE )`. Outside the args, only spaces and tabs may appear between its parts.
- **LABEL** is an identifier that is not a keyword. A keyword there is the error ``a block label can't be the keyword `for` ``. A string is not a label: `@py "x" {` is the error `a block label can't be a string` (hint: a label is a name, like stats or calc_2), and so is a string where a selector has a label (`from "odd name"`, `@py."odd name"`, 4.1). Quoted package segments (`js."date-fns"`) are not labels.
- **args** are ordinary tokens under a `Paren` entry, so they may span lines. The `{` or fence must be on the line of the header's last token.
- **Brace on the next line.** If the next non-blank line starts with `{`, the error is `the { that opens a block must be on the same line as @py`, at that `{`, with a note at the header and the hint "move { to the end of line N". Anything else there gives `expected { or a ``` fence after @py stats`.

**Brace form.** The body starts after `{`, and the profile scanner (3.8) finds the matching `}`. When that `}` is on a later line, only whitespace may follow the `{` on the header line: other text there is the error `a multi-line block starts on the line after its {`, at that text, with the hint `move this code to the next line, or write the whole block on one line`. Two guards catch a scanner that closed early on a brace hidden in a construct it does not understand. Both raise a `SyntaxError` whose hint names the fence form ("if that } belongs to the foreign code, use the fence form"):
- **C1.** If the body spans lines, the closing `}` must be the first non-blank character of its line: `@py block closes at a } that does not start its line`. It is reported at the block's `@` (DOCS 4 Errors: "at the opening line"), with a note at the offending `}`.
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
arg       = expr | NAME ":" expr | "..." expr  subscript = expr | [ expr ] ":" [ expr ]
primary   = INT | FLOAT | STRING | FSTRING | BYTES | "true" | "false" | "null" | NAME
          | "(" expr ")" | list | dict | func_expr
list      = "[" [ expr { "," expr } [ "," ] ] "]"      func_expr = "func" "(" params ")" ( "=>" expr | body )
dict      = "{" [ entry { "," entry } [ "," ] ] "}"    (* Dict brace *)
entry     = ( NAME | STRING | "[" expr "]" ) ":" expr
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
  - Named args come after all positional and spread args. A named arg is `NAME` then `:` (two-token lookahead), so `f(c ? a : b)` is positional.
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
- **`@export` grammar:** `"@export" "{" ( "*" | xitem { "," xitem } [ "," ] ) "}"`, with `xitem = NAME | KEYWORD | STRING`.
  - Keywords are accepted bare, because they name foreign functions.
  - Several `@export` lines merge into one list, which may combine `*` with explicit names (12.3).
  - In `c`, `cpp` and `rust` blocks (and their aliases), items name functions only; a quoted item is a `SyntaxError` (4.5).
  - Elsewhere, a quoted item is a name that is not a V2 identifier (JS `"$helper"`).
- **`@import` inside a block** takes only the `{ items } from selector` form of `engine_import`; the whole-module form is a `SyntaxError` (4.5). It may use the `v2` selector. Its quoted items need no `as`, because the name binds in the foreign namespace. Directive mistakes in compiled blocks are static (4.5); which directives a `run`-engine block accepts is checked when the block is reached (12.1 step 3).

**Step 3: dedent.** A line is a *code line* unless the scanner recorded that it starts inside a multi-line string, template or comment (3.8).
1. Whitespace-only code lines become empty and are ignored, and so are lines holding only a comment (`#` for py; `//` or `/*` for js, ts and the compiled languages), whose indentation the language doesn't care about. m is the smallest number of leading spaces and tabs over the remaining code lines.
2. Their first m chars must be identical. If they are not, the error is `block indentation mixes tabs and spaces`, at the first line that differs, with a note at the first code line (DOCS 4.1). Chars after the first m are the foreign language's business.
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
| `@export {}`, a name exported twice, text after the list, an unterminated list (4.4) | the specific reason |
| a quoted `@export` item in a `c`, `cpp`, `cxx`, `rust` or `rs` block (4.4) | `compiled blocks export functions by name` (hint: export a wrapper function with a plain signature) |
| an `@import` in a compiled block without `@export` (program mode, 15.5) | `a compiled block without @export runs as a program and can't @import` (hint: `export a function from this block, or call the other language from V2 code`) |
| `@import { * } from …`, at V2 level or in any block | `@import needs a list of names` (hint: `list the names: @import { mean } from …`) |
| in a compiled block, an `@import`'s local name (the alias, else the name) or an exported name that is not a valid, non-keyword identifier of the block's language (C, C++ or Rust, by tag) | import: `'x' is not a valid C identifier — rename it with 'as'`; export: `'x' is not a valid C identifier` (hint: `rename the function, or export a wrapper with a plain name`) |
| a V2-level `@import sel as x` where sel is not a package selector `lang.module` | `only a package selector (py.name, js.name) can be imported as a whole module` |
| `@import sel as x` inside a block (4.4) | `the whole-module form of @import is only valid in V2 code`, with a hint naming the block language's own import when the selector is a `py.` module in a py block (`use import numpy as np inside @py blocks`) or a `js.`/`ts.` module in a js/ts block (`use const dfns = require("date-fns") inside @js blocks`); otherwise the hint is `import names instead: @import { … } from …` |
| a quoted V2-level import item that is not a plain name (a V2 identifier that is not a keyword, 13.4) and has no `as` | `quoted name 'from' needs an alias: "from" as NAME` |
| `a..b` outside a `for` head; `for i, x in a..b`; `for (x in xs)` | `a..b is only allowed in a for head; use range(a, b)` / `a range loop takes one variable` / `a for head takes no parentheses` |
| `break` or `continue` outside a loop (a function body starts outside any loop); `return` outside a function | `break outside a loop`, and so on |
| a name declared twice directly in one body: `let`, `const`, `func`, named `@import` items, `import` and `extern` aliases, `extern` functions, and the parameters, loop variables or catch name that the body introduces (DOCS 3.3; inner bodies may shadow) | `'x' is already declared in this block`, with a note at the first declaration |
| a duplicate parameter, a non-default parameter after a default one, `...rest` not last; a positional argument after a named one, a duplicate named argument | the specific reason |
| `try` with neither `catch` nor `finally` | `try needs a catch or a finally` |
| `a < b < c`; `a == b == c` (`!=` too); `not` without `in` | `comparisons cannot be chained; write a < b && b < c` / `comparisons cannot be chained; write a == b && b == c` / `not is only used in 'not in'; negate with !x` |
| an expression statement that is not a call (a postfix chain whose last step is a call, `?.` chains included) or an action block, either one optionally in parentheses (DOCS 3.1) | `this expression's value is not used`, with the hint `to continue the previous line, end that line with the operator` for a unary `-` or `!` expression, `did you mean '='?` for a top-level `==`, else `assign it with let, or remove it` |
| a dict key that is a number / a keyword | `dict keys are names, strings or [expr]` / ``keyword `in` can't be a bare dict key: quote it`` (after `=> {`, with a hint to write `func(…) { … }`) |
| an invalid assignment target; a target through `?.` (4.1) | `cannot assign to this expression` / `cannot assign through ?.` |
| an `extern` declaration V2 cannot read (3.9) | the messages of 16.3 |

Checked at run time instead: unknown tags (`EngineUnknown`, 7.3) and the directives that `run`-engine blocks accept (12.1 step 3).

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

- `Value::clone` is O(1): assigning, passing, returning or storing a list or dict shares it; `str`, `bytes`, `func`, `handle` and `error` are immutable. `.copy()` is shallow: a new `Vec`/`IndexMap`, nested containers shared.
- **Cycles through frames.** A closure over a frame that the frame holds (in a slot, or in a list or dict in one) makes an `Rc` cycle, and so does one over a frame below it (a closure made in an `if` or a loop body, kept in the function's variables). Leaving a frame at or below which closures were made walks what its slots reach: each frame, list, dict, function and error is counted with its references from the walked values, and one with more holders than that is held from outside, so it lives, with all it reaches (followed along each value's own references, so linear). The frame is garbage if none of that reaches it, and its slots are cleared, with those of every other frame found garbage. A value is walked only once all its holders are found among walked ones, so what the rest of the program holds (data passed in, a running frame, another call's closure) costs nothing. Nor is a frame made before the one checked, or one checked before whose walk looked at more than 64 items unless it is below the one checked (its own nested call): it was checked when it was left, and a chain of closures, each over the frame before (built by a loop or by recursion), would take every walk back to the first. A reference the walk misses only makes more look held, so a wrong answer keeps memory but never frees what is in use. A frame still held when it is left is remembered, and what it holds counts toward the next collection as if made then: list items and dict entries, and strings and bytes it alone holds by their size (16 bytes an item), and a handle it alone holds as 1,000 (its object, elsewhere, could be big); a collection counts them in what it found alive too. Dropping a closure that leaves at most one other holder rechecks each remembered frame from the closure's own up, at the next scope exit, once the frames left since that frame's last check, times 8, reach the items that check looked at: a big group costs each call a few items, not itself. A collection walks all remembered frames together and everything they reach, with no budget: it frees cycles through several frames, cycles made after a frame was left, cycles a dropped list or dict let go of (no closure drop announces those), and groups too big for a scope exit's walk (10,000,000 items or 1,000,000 values). It runs at a scope exit once the program has made, since the last one, as many list items, dict entries and frame slots as the last one found alive, and at least 100,000: its work stays in proportion to the program's, and garbage to what lives. Other cycles, such as lists and dicts that point at each other, or one through another language (a frame holding a handle whose object keeps a closure over that frame, DOCS 8.2), are not collected.
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
- **Ordering** (`< > <= >=`, `sort`, `min`, `max`) exists only for number–number and str–str. Int vs float is exact: NaN compares `false`; `f >= 2^63` or `f < -2^63` decides at once; otherwise compare `i` with `f.trunc() as i64`, a tie decided by the sign of `f - f.trunc()`. Strings compare by code point (Rust `str` order). Other pairs → `TypeError: cannot compare list and int with '<'`; a handle operand gets the TypeError of 11.7.
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
- `x op= y` always means `x = x op y`, with the target's container and key evaluated once: `xs += [1]` makes a new list and rebinds `xs` (other names keep the old list). `&&`/`||` short-circuit and return an operand; `??` returns its left operand unless it is `null`, and evaluates its right operand only when the left one is `null`.
- `x in y`: list → an element `eq` to `x`; dict → key lookup (a non-`Str` `x` is `false`); str → substring, `x` must be a `Str`; bytes → a `Bytes` subsequence or an `Int` byte (outside 0–255 → `false`); otherwise TypeError (`'in' needs a list, dict, str or bytes on the right, got int`; `'in' on a str needs a str, got int`). `not in` negates.
- Handles take only `==`/`!=`: `in`, slicing, arithmetic, unary `-` and ordering raise the TypeErrors of 11.7, whose hint says to do it inside a block or convert with `to_v2(h)`.

### 5.4 Strings and bytes

A `Str` is always valid UTF-8 (lone surrogates never enter, 10.4); user-visible positions are **code-point** indexes. `Interp` keeps an 8-slot round-robin cache of `CharIndex { s: Weak<str>, ascii: bool, chars: usize, marks: Vec<usize> }` (`marks`: byte offsets of chars 0, 64, 128, …) so `s[i]`, `len(s)`, slices and `index_of` are not O(n). A lookup compares `Weak::as_ptr` with `Rc::as_ptr` (the `Weak` keeps the allocation, so its address cannot be reused). Char → byte is `i` when `ascii`, else `marks[i / 64]` plus `i % 64` steps; byte → char binary-searches `marks`. Strings under 64 bytes and `for ch in s` use no index. An entry for a string no one holds any more is dropped at the next lookup: its `Weak` would keep the whole allocation.

- `s[i]` is a one-char `Str`; negative `i` adds the length; out of range → `IndexError: string index 9 out of range (length 5)` (lists and bytes use the same wording).
- `x[a:b]` on str, bytes and list follows Python: a missing bound is 0 or the length, a negative bound adds the length, both clamp to `[0, len]`, `a >= b` gives an empty value, the result is new (shallow for lists). No step, no slice assignment; slicing anything else is a TypeError.
- f-strings: `${e}` inserts `str(e)`; `${e:.Nf}` needs an int or float (else TypeError) and is `format!("{:.N$}", x as f64)`, rounding the exact binary value (`${19.999:.2f}` → `20.00`); NaN prints `nan`, not Rust's `NaN`.
- `Bytes` supports `len(b)`, `b[i]` (an `Int` 0–255), slices, `+`, `==` and `in`, but no ordering. `b.decode()` is strict UTF-8 (else `ValueError: invalid UTF-8 at byte offset N`); `b.hex()` is lowercase without separators.

### 5.5 Members, indexing and optional chains

| Receiver | `r.name` | `r.name(args)` | `r[k]` | `r.name = v` / `r[k] = v` |
| --- | --- | --- | --- | --- |
| dict | key; missing → `KeyError: key 'name' not found` + did-you-mean over the keys | key `name` holding a `Func` or a handle → call it; otherwise the dict method | `Str` key (else `TypeError: dict keys must be str, got int`); missing → KeyError | insert or replace (a replaced key keeps its position) |
| list | `TypeError: list has no field 'push'` | list method | `Int` (else `TypeError: list index must be an int, got float`), negative from the end; IndexError | index assignment; IndexError, never grows; `r.name = v` is the field TypeError |
| str, bytes | `TypeError: str has no field 'x'` | method (5.11) | code point / byte (`string index …`, `bytes index …`) | `TypeError: str is immutable` |
| handle | `op getattr` | `call {h, method}` | `op getitem` | `op setattr` / `op setitem` (11.3; native pointers: TypeError, 16.6) |
| error | field (5.9) | `TypeError: error has no method 'x'` | `TypeError: cannot index error` | `TypeError: error values are read-only` |
| null | `TypeError: cannot read 'name' of null (use ?.)` | same | `TypeError: cannot index null` | `TypeError: cannot set 'name' of null` / `TypeError: cannot index null` |
| other | `TypeError: int has no field 'x'` | `TypeError: int has no method 'x'` | `TypeError: cannot index int` | `TypeError: int has no field 'x'` / `TypeError: cannot index int` |

- A dict dispatches `d.name(args)` on its key first **only when that key holds a `Func` or a handle** (a foreign callable got as a value): module namespaces (6.4), `extern … as ns` dicts (16.1) and dicts of functions work like objects (`ns.keys(…)` calls a C function named `keys`), while JSON data with a `"values"` key keeps the dict methods. A float index (even `1.0`) or bool index is a TypeError; a computed literal key `[e]: v` must be a `Str`.
- **Optional chains.** The parser wraps a postfix chain containing `?.` in `OptChain` (4.1). A `?.` step whose receiver is `null` ends the **whole chain** at once with `null`, as in JS: `a?.b.c()` with a null `a` evaluates neither `.b`, `.c` nor the call's arguments. Parentheses end the chain, so `(a?.b).c` fails when `a` is null. Only a null receiver of `?.` is guarded: `a?.b.c` with a non-null `a` and a null `b` fails at `.c`, and a missing dict key is still a KeyError. Assigning through `?.` is a SyntaxError (4.1). `eval_chain` returns `Ok(None)` for the short-circuit, and only `OptChain` turns it into `null`.

### 5.6 Environments and declarations

`Env = Rc<RefCell<Frame>>`; `Frame { layout: Rc<[Rc<str>]>, slots: Vec<Option<Slot>>, parent: Option<Env>, closures: u32 }`; `Slot { value, is_const: bool }`.

Every name V2 can declare is known statically, so `syntax/resolve` (run by `parse_file`, also for files parsed late, 6.4) gives each scope a **layout**, its names in insertion order (parameters, rest, loop variables, the catch name, hoisted `func`s, then the statements in order), and each `Name` and name assignment a position: how many frames up, and which index. An empty slot means that name is not declared yet. A condition, a `for` iterable and block args resolve in the enclosing scope. A name no scope declares resolves in the builtins frame or raises 5.6's `NameError`.

- **Frames:** builtins frame (shared, read-only: assigning to a builtin's name is `TypeError: cannot assign to builtin 'print' — shadow it with let`) ← one module frame per file ← call frames ← block frames. Each `if`/`elif`/`else` arm and `try`/`catch`/`finally` body gets a new frame; loops get **one fresh frame per iteration**, holding a `for`'s variables, in which the body runs directly; a call frame holds the parameters and is the body's top-level frame; a catch body's frame holds the catch name. Parameters, loop variables and the catch name are therefore declared in the body they introduce (4.5); inner blocks may shadow. A body that declares nothing and directly creates no function gets no frame (a frame there would only change when a closure's cycle is trimmed, below); module frames always exist. Reads and writes go by position; on an empty slot, either one falls back to walking `parent` by name through the layouts, skipping empty slots, so `x = 1` or `print(x)` before a local `let x` acts on the outer `x`, or raises. A `RefCell` borrow is held only while one slot is read or written.
- **Declarations:** `let x = e` evaluates `e`, then declares `x` (`let x = x + 1` in an inner block reads the outer `x`); `let x` binds `null`. `const`, `@import` names (13.4) and `extern` names (16.1) are const slots: assigning → `TypeError: cannot assign to constant 'LIMIT'` (shallow: a const list can still `push`). A name declared twice in one body is a static SyntaxError (4.5). An undeclared name → `NameError: 'x' is not declared` + did-you-mean (an assignment adds `— declare it with let`).
- **Hoisting:** entering a body turns each `func` declaration directly inside it into a `Closure` over that frame before the first statement runs; reaching the statement later does nothing. **Closures** capture the `Env`, so `func() => i` made in a loop keeps its iteration's `i`.
- **Cycle trimming:** a closure stored in the frame it captures (every local `func` declaration, `let f = func(…) …`) is an `Rc` cycle. `Frame.closures` counts the closures made at or below a frame; leaving one with any runs 5.1's check ("Cycles through frames"), which clears the slots of a frame nothing outside reaches (releasing held handles). Cycles through containers are not trimmed.

### 5.7 Calls

The callee is evaluated first, then the arguments left to right; for `r.name(args)` the callee is resolved first too (the `Func` or handle under a dict key, or the method, or the null-receiver / unknown-method TypeError of 5.5), so an argument that changes `r` can't change what is called; `...xs` must be a list (`TypeError: spread argument must be a list, got dict`) and expands in place. `Closure` and `Builtin` (a `&'static [ParamSpec]`) share one binder:

1. Positional args fill the parameters before the rest parameter; extras go to the rest list, or without one → `ArityError: greet() takes 2 positional arguments but 3 were given`.
2. Named args match by name: unknown → `ArityError: greet() got an unexpected argument 'greting' — did you mean 'greeting'?`; already bound → `… got multiple values for argument 'name'`; the rest parameter → `… cannot take rest parameter 'nums' by name`.
3. Unfilled parameters without a default → `ArityError: greet() missing required argument 'name'` (all listed), checked before any default runs.
4. Push the `CallRecord` (5.8), check the depth (5.9), create the call frame (parent `Closure.env`) and walk the parameters in order: declare the bound value or the rest list (possibly empty), else evaluate the default **in the call frame** (it sees earlier parameters; re-evaluated per call). Parameters after the rest parameter are named-only (builtins only, such as `print`'s `sep` and `end`; in V2 code `...rest` must be last, 4.5).
5. Hoist the body's `func`s and run it; falling off the end returns `null`.

- `Proxy`, handle calls `h(…)` and `HandleMethod` forward positional and named args as given (`call`, 9.5). Named args to a session without the `kwargs` cap (js, ts; custom engines per 17.5) → `ArityError: {tag} functions take positional arguments only (got '{kw}')`, raised before sending (10.8, 17.5).
- `Native` (compiled exports, `extern`) takes positional args only (`ArityError: {name}() takes positional arguments only`), counted against the C signature (16.5), and runs in-process (16.7).
- A non-function → `TypeError: 'int' is not callable`. An anonymous function is named `func@main.v2:12` in messages, `str()` and its trace frame (`func@main.v2:12 (main.v2:14)`, 2.5).
- V2 functions called from foreign code (`fn` and `route` calls, 11.8, 13.6; compiled `host_call`, 15.9) go through the same binder on the interpreter thread, inside the host's wait for that engine (9.7). They can arrive only while V2 has a request outstanding to that engine (9.8); a stored callback stays valid for later requests.

### 5.8 Control flow and frame discipline

Statements return `Exec<()>`, expressions `Exec<Value>`: `type Exec<T> = Result<T, Flow>`, `enum Flow { Error(Rc<ErrorValue>), Return(Value), Break, Continue, Exit(i32) }`. `exec(stmt, env)` and `eval(e, env)` take the environment as an **argument**, never interpreter state: a child frame is a local `Env` dropped on every exit path, including `?`, so a `catch` always runs with its own function's locals (R09, 22.4).

- The only interpreter-level stack is `calls: Vec<CallRecord { name: Option<Rc<str>>, call_site: Span }>`. Calls of non-builtin callables and module imports push a record and pop it with the result held in a local (`let r = self.run_call(..); self.calls.pop(); r`), never with a `?` in between.
- A call turns `Return(v)` into `Ok(v)`; `break`/`continue` never cross a function (static, 4.5). Loops: `Ok`/`Continue` → next iteration, `Break` → exit, anything else propagates.
- `try`: the body runs in a new frame; on `Flow::Error(e)` the `catch` body runs in a new frame with `e` bound; `Return`/`Break`/`Continue` are never caught. `finally` runs last for every flow except `Exit`: if it completes normally the pending outcome (value, flow, or a new error from `catch`) continues, else its own flow replaces it. Every error kind is catchable, `RecursionError` and engine errors included.
- `Exit(code)` (from `exit()`, 5.10) is caught by nothing and skips every `finally`; it unwinds through foreign frames (9.7) and native frames (15.9) to the top, where 6.5 ends the run. Interrupts never become a `Flow` (6.5).
- **Iteration** evaluates the iterable once: list → a snapshot of its elements; dict → a snapshot of its `(key, value)` pairs (one variable gets the key); str → chars; bytes → `Int`s; `a..b` → a lazy `i64` counter (both ends `Int`, else TypeError; `b <= a` → no iterations); handle → `op iter`/`next` (11.4). Two variables get `(index, item)`, or `(key, value)` for a dict or a mapping handle (11.4). Otherwise `TypeError: cannot iterate over int`.

### 5.9 Errors, traces and limits

- **Kinds.** Host: `Error TypeError ValueError KeyError IndexError NameError ArityError IOError RecursionError ImportError AssertionError`. Engines and crossings (ch. 7–17): `EngineUnknown EngineNotFound EngineCrashed EngineTimeout EngineExitError MarshalError BuildError ForeignError ProtocolError FFILoadError`. These 21 are catchable. `SyntaxError` is a diagnostic kind (2.4): it is reported before its file runs, so it never reaches a `catch` in that file (imported files: 6.4). `error(type, …)` accepts any non-empty type string.
- **Raising.** `throw v` throws an `Error` as is, turns a `Str` into `error("Error", v)`, else `TypeError: throw expects an error or a str, got int`. `span` and `trace` are filled when the error is first raised: by `throw` (the `throw` keyword), by a failing node (operator, call, index, member; spans per 2.2), or on surfacing from an engine (the mapped foreign location, 12.6). `error()` leaves both empty; a rethrow keeps them (`OnceCell`), so `==` still holds and the uncaught header points at the first `throw`.
- **Trace** (innermost first): `loc` = raise span; for each `CallRecord` from the top, emit `(record.name, loc)`, then `loc = record.call_site`; finally `(None, loc)`. A proxy call's record emits the reply's mapped foreign frames (12.6) instead. Frame strings are 2.5; rendering and elision of long traces are 2.4 (`e.trace` keeps every frame).
- **Fields:** `type message file line col trace data lang foreign_type cause`; any other → `TypeError: error has no field 'x'` + did-you-mean. `file`/`line`/`col` are `null` until raised (`file` is the display path, 2.1); `trace` is a new list of strings on each read. `cause` is the error this one was raised from (a foreign exception's chained cause, 9.6), or `null`; `error()` can't set it, and an uncaught error prints it as `caused by:` (2.4).
- **RecursionError:** a call that takes `calls.len()` past **10,000** → `RecursionError: maximum call depth (10000) exceeded` at its span. A stack guard on calls and the less common expressions, at 7/8 of the stack, also covers deep expressions and nested foreign callbacks: each call and each `eval` of an expression node compares a local's address (`std::hint::black_box`) with the one recorded at thread start and raises the same error past 7/8 of the stack size. Cross-language nesting has its own limit of 64 (9.7).
- **Did-you-mean** (names, keys, fields, methods, named args; tags use 7.3): optimal string alignment distance on lowercased names, accepted when `d <= max(1, min(3, (len + 2) / 3))`; the smallest wins, ties → innermost frame, then declaration order.

### 5.10 Builtins

`BuiltinDef { name, params: &'static [ParamSpec], f: fn(&mut Interp, Args, Span) -> Exec<Value> }` entries live in the builtins frame, so user names can shadow them. Notes beyond DOCS 3.11:

| Builtin | Notes |
| --- | --- |
| `print` | `str()` of each value joined by `sep`, then `end` (both `Str`, else TypeError); one write to `Out`, or with `stderr: true` (a `Bool`) one write to the host's stderr. |
| `input` | Writes `str(prompt)`, flushes (see Output), reads one line from the host's stdin and strips `\n` and one `\r` before it. End of input before any byte → `null`; a last line without `\n` is returned; invalid UTF-8 → IOError. |
| `len`, `type`, `bool` | `len`: chars (5.4), bytes, list items, dict entries, a handle's `op len` (11.3), else `TypeError: len() needs a str, bytes, list or dict, got int`. `type`: 5.1. `bool`: 5.2 truthiness (no round trip). |
| `str` | A `Str` is returned unchanged; others get the canonical form: `null`, `true`, `42`; floats by Rust `{:?}` (`2.0`, `0.1`, `1e20`) plus `nan`, `inf`, `-inf`; strings inside containers double-quoted with `\" \\ \n \r \t` and `\u{..}` for other control chars (`[1, "a"]`, `{"k": 1}`); bytes as `b"\x00A\xff"` (printable ASCII as is, `\" \\ \n \r \t`, other bytes as lowercase `\xNN`); `<func greet>`, `<func func@main.v2:3>`, `<func spread (py)>`, `<func fast_sum (rust)>`, `<func cos (extern)>`, `<builtin print>`; errors `ValueError: msg` or `ForeignError: ZeroDivisionError: msg`; handles via `op str` (11.3; a dead session raises EngineCrashed, 8.2, 11.7); native pointers `<native sqlite3* 0x…>` (16.6). |
| `int`, `float` | `int`: a float truncates toward zero (NaN, inf, out of range → `ValueError: cannot convert nan to int`); a str must be `[+-]?[0-9]+` after trimming (else `ValueError: invalid int: 'abc'`; beyond i64 → `ValueError: int out of range: '…'`); bool → 0/1. `float`: int → nearest f64, bool → 0.0/1.0, a trimmed str via `f64::from_str` (also `nan`, `inf`, `infinity`, any case), failure → ValueError. Other types → `TypeError: int() can't convert list`. |
| `range` | `range(end)` or `range(start, end, step = 1)`, all `Int`s, positional only (a named argument → `ArityError: range() takes positional arguments only`; none → `ArityError: range() missing required argument 'end'`); the length is computed first with checked arithmetic and the list reserved with `try_reserve_exact` (failure → `ValueError: range too large`); `step == 0` → `ValueError: range() step must not be zero`. |
| `json_parse` | The codec in plain mode (10.1) straight into `Value`: integer tokens that fit i64 → `Int` (`-0` is the float `-0.0`, 10.2); integer tokens outside i64 → `ValueError: invalid JSON at line L column C: integer does not fit in 64 bits`, raised by the 10.1 pre-parse byte scan, since serde_json would already have made them `f64` (never the nearest `Float`, 0.1; 10.2); tokens with `.`, `e` or `E` → `Float` (`1.0` stays a float); duplicate keys: the last value wins at the first key's position; `$v2` keys are plain data. Syntax errors, lone `\uD800` escapes and nesting past 1,000 levels (10.1) → `ValueError: invalid JSON at line L column C: …`. |
| `json_stringify` | null, bool, int, finite float (keeps `.0`), str, list, dict. bytes, func, handle, error and non-finite floats → TypeError naming the path (`TypeError: cannot convert handle to JSON at [2]["k"] (use to_v2(h) first)`, 10.1); a cycle → `ValueError: cannot convert a cyclic list to JSON`; nesting past 1,000 levels → ValueError (10.1). `indent: null` → compact (`{"a":1}`); an int ≥ 0 → pretty with that many spaces, one item per line, `": "` after keys, empty containers as `[]`/`{}` (as in Python and JS); a negative int → ValueError. |
| `read_file`, `write_file` | Relative paths resolve against the process working directory (unlike `import` and `extern`, which use the `.v2` file's folder). Text reads are strict UTF-8, BOM kept, no newline translation (invalid → `IOError: 'x' is not valid UTF-8 (use binary: true)`). `write_file` takes `Str` (UTF-8) or `Bytes` (raw), else TypeError, via `fs::write` (creates or truncates, no parent folders); `append: true` (a `Bool`, like `binary`) appends instead, creating a missing file. OS failure → `IOError: cannot read 'x': <os message>` (or `write`). |
| `exists`, `list_dir`, `make_dir`, `remove_file` | Paths as for `read_file`. `exists`: whether `fs::metadata` succeeds (symlinks followed; any error → `false`). `list_dir`: a new list of the entry names, sorted by code point; a name that is not valid UTF-8 → `IOError: cannot list 'x': a name is not valid UTF-8`. `make_dir`: `fs::create_dir_all` (missing parents too; an existing folder is fine); returns `null`. `remove_file`: `fs::remove_file`, so a folder is an IOError. OS failure → `IOError: cannot list 'x': <os message>` (or `create`, `remove`). |
| `script_dir` | The parent of `SourceFile.path` (2.1) of the call's span: the folder of the file containing the call, not the entry file's. |
| `exec` | `argv`: a non-empty list of `Str`s (else TypeError, or ValueError when empty). `argv[0]` is found by 7.5 with the cwd as base. Cwd and environment are the host's. `input`: `null` → the null device; a `Str` or `Bytes` is piped to the child's stdin (8.3). `process::run_captured` collects stdout and stderr completely (no limit beyond memory, no timeout) and decodes them as UTF-8 leniently (U+FFFD); the child's tree is killed when `exec` returns (8.3) and on an interrupt (6.5). Returns a new dict `{code, stdout, stderr}`: `code` is the exit code (Unix, killed by a signal: 128 + its number; Windows: the unsigned 32-bit code, as 8.6 prints it); a non-zero code is not an error. Not started → `IOError: exec: cannot start 'gitt': <reason>` (`not found on PATH`, or the OS message). |
| `sleep`, `time` | `sleep`: an `Int` or `Float` (else TypeError); negative, NaN or beyond `Duration` → ValueError; `thread::sleep` on the interpreter thread, so an interrupt still ends the run through the watchdog (6.5). `time`: `SystemTime` since `UNIX_EPOCH` as a `Float` of seconds. |
| `min`, `max` | Exactly one argument that is a list → its items; otherwise the arguments. All numbers or all strings (else TypeError), compared by 5.2's ordering; an item replaces the current best only when strictly smaller (larger), so the first wins ties. No values → `ValueError: min() of an empty list` (`max()`). |
| `sum`, `abs` | `sum`: a list of `Int`s and `Float`s (else TypeError), folded from `0` left to right with 5.3's `+` (int overflow → ValueError; any float makes the result a float); `sum([])` is `0`. `abs`: an `Int` via `checked_abs` (overflow → ValueError) or a `Float`; else TypeError. |
| `round` | `digits`: an `Int` (negative → ValueError). An `Int` `x` is returned unchanged; a `Float` is formatted as `${x:.Nf}` does (5.4, ties to even on the exact binary value) and parsed back: `digits` 0 → an `Int` (NaN, inf or beyond i64 → ValueError, as `int()`), > 0 → a `Float`; from 324 on, `x` unchanged (no f64 needs more decimals). |
| `div` | Two `Int`s (else TypeError): `checked_div`, minus 1 when the remainder is non-zero and the signs differ (floor, so `div(a, b) * b + a % b == a`). `b == 0` → `ValueError: division by zero`; `div(-2^63, -1)` → ValueError. |
| `getenv`, `args` | `var_os`/`args_os`, decoded leniently (U+FFFD; on purpose, 0.1). A name that is empty or contains `=` or NUL → `ValueError: invalid environment variable name: 'A=B'` (std may panic on it). `args()` returns a new list each call. |
| `exit` | `code`: an `Int` in i32 range (else TypeError / `ValueError: exit code 1099511627776 is out of range`; POSIX keeps the low 8 bits). Returns `Flow::Exit(code)` (5.8, 6.5). |
| `error`, `assert` | `error`: `type` (non-empty) and `message` must be `Str`, `data` anything; no location until raised. `assert`: a falsy `cond` (5.2) throws `error("AssertionError", message)` (`message` a `Str`), raised at the call (2.2). |
| `to_v2` | A handle → `op value` (11.5; native pointer → MarshalError, 16.6); anything else is returned unchanged (containers are not walked). |
| `define_engine` | 17.2: one custom engine, for the rest of the run. |
| `engine_available` | 7.3. |

**Output.** `Out` (`BufWriter<StdoutLock<'static>>`, 64 KiB, owned by `Interp`) is the only writer of the host's stdout: `print` and the stdout text of worker `out` frames (9.9). `Out` is flushed after every write when stdout is a terminal (`IsTerminal`, checked once), and before every in-process native call (16.7), every frame written to a worker (9.9), every child spawn (a statement-position compiled or `run` block inherits stdout, 6.3), every `input()` read, every write to stderr (diagnostics, `out` stderr text, `print(stderr: true)`), and at the end of the run; dirty native stdio is flushed before every write to `Out` (16.7). A failed write of `print` to either stream (closed pipe; Rust ignores SIGPIPE, so it is `EPIPE`) raises IOError at the `print`. Stdin belongs to `input()` alone (workers: 9.9; other children: 8.3).

### 5.11 Methods

Arguments bind as for builtins, and messages name the method with its type: `ArityError: list.get() takes 2 positional arguments but 3 were given`, `TypeError: list.join() argument 'sep' must be a str, got int`. `push`, `insert`, `sort`, `reverse` return `null`; `pop` and dict `remove` return the removed item. Unknown method → `TypeError: list has no method 'append'` + did-you-mean. Methods are not values: `xs.push` without a call is the field TypeError of 5.5. There is no `len()` method: lengths come from `len(x)` (5.10).

| Type | Methods |
| --- | --- |
| str | `upper()`, `lower()` (full Unicode mapping); `trim()` (`char::is_whitespace`); `split(sep = null)` (`null`: `split_whitespace`, so no empty parts; a `Str` must be non-empty, else `ValueError: str.split() separator must not be empty`, and keeps empty parts); `lines()` (Rust `str::lines`: `\n` or `\r\n`, no trailing empty line); `replace(old, new)` (all matches, left to right; empty `old` → `ValueError: str.replace() argument 'old' must not be empty`); `starts_with(p)`, `ends_with(p)`; `index_of(sub)` (code-point index or `-1`); `encode()` |
| bytes | `decode()`, `hex()` (5.4) |
| list | `push(...items)` (appended in order; none is fine); `pop(i = -1)` (negative from the end; empty → `IndexError: pop from empty list`; out of range → `IndexError: list index 9 out of range (length 5)`); `get(i, default = null)` (`Int`, negative from the end; out of range → `default`); `insert(i, x)` (negative `i` adds the length, then clamps to `[0, len]`); `sort(key = null, reverse = false)`; `reverse()`; `map(f)`, `filter(f)` (over a snapshot; `f` gets exactly one argument; new list); `join(sep)` (`str()` of each item); `index_of(x)` (first `eq`, or `-1`); `copy()` (shallow) |
| dict | `keys()`, `values()` (new lists, insertion order); `get(k, default = null)`; `remove(k)` (`shift_remove`, the rest keeps its order; missing → KeyError); `copy()` (shallow). `get`/`remove` keys must be `Str` (TypeError). |

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

1. **Resolve** `PATH` as written (no extension added) against the **importing file's** folder (absolute paths as is), then `fs::canonicalize`. Missing → `ImportError: cannot find module './util.v2' (looked for C:\proj\util.v2)` (paths shown per 2.1, with `..` resolved and the existing part canonicalized). A Windows drive-relative path (`C:m.v2`) is neither relative to the file nor absolute: `ImportError: cannot find module 'C:m.v2' (a drive-relative path: write it relative to the file, or absolute)`. A file that did not exist at startup but exists now is parsed now.
2. **Look up** `Interp.modules` (by canonical path), whose `ModuleState` is `Running | Done(Value) | Failed(Rc<ErrorValue>)` (the entry file starts `Running`):
   - `Done(ns)` → declare `name = ns`. `Failed(e)` → throw `e` again: a module's top level never runs twice.
   - `Running` → `ImportError: circular import: a.v2 -> b.v2 -> a.v2` (the chain from the path's first entry in `import_stack`).
   - Absent, with stored diagnostics (6.1) → `Failed` with `ImportError: cannot import './util.v2': util.v2:3:9: SyntaxError: <message>` (the first diagnostic) at the `import`, `data` = all of them as `{file, line, col, message}`.
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

**Interrupts** (Ctrl+C, Ctrl+Break, console close, SIGINT, SIGTERM, SIGHUP, SIGQUIT) are handled by the host alone; 8.3's child rules decide which of them reach a child. The interrupt handler sets `INTERRUPTED` (an `AtomicBool`); the watchdog does the rest. Unix: a `sigaction` handler for SIGINT, SIGTERM and SIGHUP that does nothing else (only async-signal-safe work); a signal already ignored when v2 starts (`nohup`, background jobs of a shell without job control) stays ignored. If the watchdog thread cannot be started, no handler is installed, so the OS default still ends the process. Windows: the `SetConsoleCtrlHandler` routine sets `INTERRUPTED`, wakes the watchdog (8.5), then blocks until the process ends, because returning early would let `CTRL_CLOSE_EVENT` terminate the process before cleanup. The watchdog (1.3) checks `INTERRUPTED` on every wake (8.5). Once it is set, the watchdog `kill_tree`s every registered child, workers included (8.5), writes what `Out` holds straight to the descriptor (from a thread of its own, at most 300 ms, so a reader that stopped reading can't hold the exit), waits a bounded time for those trees to end, removes `<run>` (20.3) and ends the process with `_exit(130)` / `ExitProcess(130)`: no `bye`, `catch` or `finally`. The interpreter thread never handles an interrupt: once `INTERRUPTED` is set it prints no diagnostic (neither the EOF of a killed worker as `EngineCrashed` nor the failure of a per-use child ended by the same event), spawns no process and runs no `catch` or `finally`; where it would, it stops (`process::stop_if_interrupted`) and the watchdog ends the process. A run that ends between the interrupt and the watchdog's next wake is caught by main, which checks `INTERRUPTED` after joining the interpreter thread and takes the same path.

**Closed streams.** Diagnostics, check's summary, `--help` and `--version` are written with writes whose errors are ignored, so a closed stdout or stderr never turns an exit path into a panic or an abort: the exit code is the one the run would have had. Program output is different: a failed write is 5.10's `IOError` at the `print`, and what it couldn't write goes with it (a program that catches it has handled that output). A loss no `IOError` told the program about (a block's own error won over it, 9.9, or late output at the end) is said at the end like a failed final flush: when `Out`'s final flush (step 1 above) fails, `v2: cannot write to stdout: <os message>` goes to stderr and an exit code of 0 becomes 1 (any other code is kept), so lost output never looks like success.

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
| `js` | `javascript` | Bridged / CLike(Js) | `node` | `{adapter}` | | 22.13, not 23.0 or 23.1 (7.6) | `--version`: `^v(\d+)\.(\d+)\.(\d+)` |
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
- **EngineUnknown.** `resolve_tag(tag, span)` runs when a block is reached or a selector is bound. A miss raises `EngineUnknown: unknown engine '@pyhton' (did you mean '@py'?)` (did-you-mean per 5.9, over ids and aliases); with no close match, `unknown engine '@perl' — known engines: py, js, ts, c, cpp, rust, bash, pwsh, cmd, lua, ruby, go, java, zig, cs; declare it in [engines.perl] in v2.toml` (the list is the registry's engines); the text ends `in v2.toml, or with define_engine` (17.2). Custom tags have 17.1's lowercase shape, so a tag the lexer accepts outside it (`@PY`) is simply unknown.
- **`engine_available(tag)`:** `false` for an unknown tag (no error); otherwise `true` iff resolution succeeds (7.4, 7.6). It may run probes (cached, 7.7) but never starts a session or compiles, and a later crash (8.2) does not change its answer.

### 7.4 Custom engines in the registry

- Custom engines come from `[engines.<tag>]` tables in `v2.toml` and from `define_engine` calls: spec, validation and tag rules 17.1, registration 17.2, `V2_ENGINE_<TAG>` 18.4.
- Resolving one only finds `command[0]` (7.5) and runs nothing (`version` is `None`; doctor VERSION `-`). A miss is `EngineNotFound: 'perl' not found on PATH (engine declared in C:\proj\v2.toml)`, or `(engine declared at main.v2:3)` for a `define_engine` call.

### 7.5 Finding an executable

Children are always spawned by absolute path (Windows `Command` would search the application and system folders before `PATH`). `find_program(name, base)`, also used for `exec`'s `argv[0]` with the cwd as `base` (5.10):
1. `name` containing `/` or `\`: `base.join(name)` (`base` per 18.1); on Windows an extensionless name is also tried with each accepted extension. No `access(X_OK)` here: a named file that cannot run fails its probe with the OS message (`is not usable: could not start …: Permission denied`) instead of looking missing.
2. Otherwise each `std::env::split_paths(PATH)` entry in order, skipping empty and relative entries such as `.` (the cwd is never searched, so a cloned project cannot shadow `python`). Windows: `dir\name` if it already has an accepted extension, then `dir\name{ext}` per `PATHEXT` entry (unset: `.COM;.EXE;.BAT;.CMD`; case-insensitive), accepting only `.com`, `.exe`, `.bat`, `.cmd` (`CreateProcess` cannot start `.py` or `.ps1`). Unix: `dir/name` passing `access(X_OK)`.
3. The hit must be a regular file (`symlink_metadata` when `fs::metadata` fails, as for `WindowsApps` aliases) and stays absolute, not canonicalized. `.bat`/`.cmd` hits are spawned directly (Rust ≥ 1.77.2 quotes arguments for `cmd.exe`; its `InvalidInput` for an unquotable argument is a spawn failure, 8.3); one whose path contains `%` is a spawn failure too (`a batch file whose path contains '%' can't be started safely`), because `cmd.exe` would expand it.

A Windows drive-relative name (`C:tool`, relative to that drive's own current folder) is never found; `exec` says `IOError: exec: cannot start 'C:tool': a drive-relative path: write it relative to the working directory, or absolute`.

### 7.6 Per-engine rules

**Order.** The configured command or `[python] venv` (whichever 18.3 picks), else the candidates. A configured value never falls back to auto-detection: a missing one is `EngineNotFound: engine 'py' command 'C:/Python312/python.exe' not found (set by C:\proj\v2.toml)`, and a failing probe is an error too (doctor `broken`; too old: `old`): `engine 'py' command 'C:\x\python.exe' (set by C:\proj\v2.toml) is not usable: version probe failed (exit code 1)`. "Set by" names the file's full path or the `V2_ENGINE_<TAG>` variable. Skip reasons: `Microsoft Store placeholder`, `too old: 3.9.6 < 3.10`, `version probe failed (exit code 1)`, `version probe timed out (10 s)`, `no version in its --version output`, `targets x86 but V2 is x86_64`, or 8.3's `could not start '…': …`. Each auto-detected hit is probed (7.7); a hit whose probe fails, times out or matches no version, that is below the minimum, or that has the wrong target is skipped (kept in `skipped`) and the search goes on (next `PATH` entry, then next candidate). Probes of `PATH` hits always pass arguments: an App Execution Alias run without arguments opens the Microsoft Store.

- **py.** venv: `<venv>\Scripts\python.exe` on Windows, `<venv>/bin/python` then `bin/python3` elsewhere; missing: `EngineNotFound: [python] venv 'C:\proj\.venv' has no Scripts\python.exe (create it: python -m venv C:\proj\.venv)`. A hit under `%LOCALAPPDATA%\Microsoft\WindowsApps` (case-insensitive) is an alias: a real Store or Install Manager Python, or the placeholder that prints "Python was not found" and exits 9009 (skipped: `Microsoft Store placeholder`). `py -3` (`find_program("py")` + `-3`) is Windows-only and last. It and a Windows venv `python.exe` are launchers that run the real interpreter as a child, and aliases change target without their file changing; so the version is re-checked from `hello` (8.4) and every kill is a tree kill (8.3).
- **macOS.** `/usr/bin/python3`, `cc`, `gcc`, `g++`, `clang` and `clang++` are `xcrun` shims that open an install dialog when the Command Line Tools are missing; V2 first runs `/usr/bin/xcode-select -p`, and a non-zero exit skips them (`Command Line Tools missing`).
- **js** is `node`. One minimum covers the Node family: ts needs Node's built-in `module.stripTypeScriptTypes`, which 22.13+ has except 23.0 and 23.1, so those two count as too old. Skip reasons: `too old: 20.11.1 < 22.13`, `too old: 23.1.0 has no type stripping (needs 22.13+, not 23.0 or 23.1)`; `hello` is checked the same way (8.4). **ts** has no process, settings or version check of its own (an `[engines.ts]` table or `V2_ENGINE_TS` is a configuration error, 18.2, 18.4): `resolve(ts)` = `resolve(js)`. Without js: `EngineNotFound: engine 'ts' (TypeScript) not found — @ts blocks run on the js engine`, then js's tried, skipped and install lines; with a configured js that can't run (its command missing or broken): `engine 'ts' (TypeScript) can't run — @ts blocks run on the js engine, and <js's first line>`, then js's other lines. Stripping: 12.5.
- **c, cpp.** One compiler family per OS. Linux and macOS: the candidates `cc`, `gcc`, `clang` (c) or `c++`, `g++`, `clang++` (cpp), in that order, or the configured command; they are not target-checked. Windows: MSVC `cl` only, the candidate or a configured `argv[0]`, resolved specially:
  1. `find_program("cl")` hits and `INCLUDE` is non-empty (a Developer Prompt): use it with the current environment; FROM `PATH`.
  2. Else the probe cache's `vcvars` entry if still valid (7.7), else a fresh capture (FROM `vswhere`): `%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe -nologo -utf8 -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath` (`VC.Tools.ARM64` on arm64; no output: not found), then `cmd.exe /u /d /s /c ""<install>\VC\Auxiliary\Build\vcvarsall.bat" x64 >nul && set"` (`arm64` on arm64; `/u` makes `set` write UTF-16LE, so non-ASCII user folders survive). From the `NAME=VALUE` lines (names starting with `=` skipped) keep `INCLUDE` (empty: broken, `vcvarsall.bat did not set INCLUDE`), `LIB`, `LIBPATH` and the `PATH` entries not already in `PATH` (prepended at use); `cl.exe` is `find_program("cl")` over that `PATH`.
  3. A configured absolute `cl.exe` gets the current environment if `INCLUDE` is set, else the vswhere capture.

  Any other configured command on Windows (file stem not `cl`, compared case-insensitively: gcc, clang, clang-cl, …) is not usable and is not probed: `EngineNotFound: engine 'c' command 'C:\msys64\ucrt64\bin\gcc.exe' (set by C:\proj\v2.toml) is not usable: C and C++ on Windows use MSVC cl` (exit 3 if uncaught).

  Target: the folder holding `cl.exe` (`x64`, `x86`, `arm64`), normalized (`amd64` and `x64` → `x86_64`, `arm64` → `aarch64`), must equal `std::env::consts::ARCH`. Else an auto-detected hit is skipped (`targets x86 but V2 is x86_64`) and a configured one is not usable: `EngineNotFound: engine 'c' command 'C:\BuildTools\VC\Tools\MSVC\14.40.33807\bin\Hostx64\x86\cl.exe' (set by C:\proj\v2.toml) is not usable: targets x86 but V2 is x86_64` (exit 3 if uncaught).
- **rust.** `rustc -vV`: `release: (\d+)\.(\d+)\.(\d+)`, `host: (\S+)`; the host triple must be v2's own target triple, else the hit is skipped (`targets i686-pc-windows-msvc but V2 is x86_64-pc-windows-msvc`) or the configured command is not usable, as above. The `rustc` on `PATH` is usually the rustup proxy, which picks a toolchain from `RUSTUP_TOOLCHAIN` or a `rust-toolchain.toml` in the cwd while its own file never changes; so a run that builds Rust (never a cache hit, 15.1) runs `<proxy> --print sysroot` once (cwd = the user's) and sets `argv[0] = <sysroot>/bin/rustc[.exe]`, which keys the probe cache and is what ch. 15 compiles with. `shown()` stays the proxy (DOCS 2). rustc finds MSVC `link.exe` itself, so `@rust` on Windows needs the same Build Tools as `@c` (7.8).
- **Run engines** (7.2's Run rows). A configured `command` replaces candidate and fixed args together; its `command[0]` is what is found and probed, and `V2_ENGINE_<TAG>` replaces only that (18.4). The resolved command goes to 17.3.
  - **bash** on Windows is Git Bash only, never a `PATH` hit for `bash` (`System32\bash.exe` is the WSL launcher, which can't open the Windows temp path): `bin\bash.exe` in the parent of the folder holding `git` (found per 7.5; `C:\Program Files\Git\cmd\git.exe` gives `C:\Program Files\Git\bin\bash.exe`), then `%ProgramFiles%\Git\bin\bash.exe`. Either hit has origin `Git` (doctor FROM `git`).
  - **pwsh** on Windows falls back to `powershell` (Windows PowerShell). Both get 7.2's Windows template (`-ExecutionPolicy Bypass`: Windows PowerShell's default policy blocks scripts), so `V2_ENGINE_PWSH`, which replaces only the program, always gets matching args. Windows PowerShell has no `--version`, so `powershell` (and a configured `command[0]` whose file stem is `powershell`) is probed with `-NoProfile -Command $PSVersionTable.PSVersion.ToString()`.
  - **cmd** is not probed (`version` `None`, doctor VERSION `-`). Outside Windows it is never found: `EngineNotFound: cmd is Windows only`.

Ch. 15 receives the compiled engines' `argv`, `version`, `target` (cl's folder name or rustc's triple; `None` for cc, gcc and clang) and `env` (cl's `INCLUDE`, `LIB`, `LIBPATH` and `PATH` prefix).

### 7.7 Version probes and the probe cache

- **Running:** `argv + probe args`, stdin null, stdout and stderr are read concurrently (8.3) and joined stdout first (lossy UTF-8), cwd the user's, host env plus `spec.env`, 8.3's child rules; a 10 s timer (60 s for vcvarsall, 8.5; a configured command or venv waits as long as its engine's `startup_timeout` when that is longer, since the user's own command may start slowly), then `kill_tree`. **Parsing:** 7.2's patterns; cc, gcc, clang and their C++ names: the first `(\d+)\.(\d+)(?:\.(\d+))?` of the first `--version` line, held to clang's minimum when the output contains `clang`, else to gcc's; cl, run without arguments: the first `\d+\.\d+\.\d+` of its localized banner; run engines: the first `(\d+)\.(\d+)(?:\.(\d+))?` (`go version go1.22.1 linux/amd64` is 1.22.1), except java: `^\S+ (\d+)(?:\.(\d+))?(?:\.(\d+))?` on the first line (`openjdk 21 2023-09-19` is 21.0.0). A missing part is 0.
- **Cache** `~/.v2/cache/toolchains.json` = `{"format": 1, "entries": {…}, "vcvars": {…}}`. Entry key: sha256(argv, i.e. the path and any prefix arguments such as `py`'s `-3`, canonical path if `canonicalize` succeeds, size, mtime ns, spec hash), spec hash = sha256(engine id, probe args, pattern, V2 version); an alias is stat'ed as the link itself. Value: `{"path", "ok": true, "version", "output"}`, plus `"target"` for cl and rustc. Only usable hits are stored, and the Store placeholder (7.6), as `{"path", "placeholder": true}`: it is no version manager's shim, and its verdict can change only with the alias file. An entry with neither is ignored, and a probe that finds a hit no longer usable removes its entry. `vcvars.<arch>` = `{"install", "vcvarsall_size", "vcvarsall_mtime_ns", "cl", "include", "lib", "libpath", "path_prefix"}`, valid while `vcvarsall.bat` keeps its size and mtime and `cl` and every `include` folder exist (a VS update deletes the old toolset).
- **Access:** runs read entries and write the usable hits they probe. A skipped or too-old hit, the placeholder excepted, is never stored, so every run probes it again: a version-manager shim (pyenv, asdf, volta) runs another interpreter in another project or environment, and a timeout may have been passing (a stale usable entry is caught by `hello`'s version check, 8.4). `v2 doctor` probes afresh and rewrites the entries of the hits it probes, dropping those no longer usable. Writes go to `toolchains.json.<pid>.tmp`, then rename (a failed rename is ignored); an unreadable file or unknown `format` counts as empty.

### 7.8 EngineNotFound messages

`EngineNotFound` (exit 3 when uncaught, 19.1) means there is no usable toolchain: none installed, only too-old versions, an executable that cannot start (8.3), a configured command or venv that does not exist, or `cmd` outside Windows (7.6). Custom engines: 7.4.

```text
EngineNotFound: engine 'py' (Python) not found
  tried: python3, python, py -3 on PATH
  skipped: C:\Users\me\AppData\Local\Microsoft\WindowsApps\python.exe (Microsoft Store placeholder)
  install: winget install -e --id Python.Python.3.12
  or set [engines.py] command in v2.toml, or V2_ENGINE_PY
```

| engine | Windows | Linux | macOS |
|---|---|---|---|
| py | `winget install -e --id Python.Python.3.12` | `sudo apt install python3` / `sudo dnf install python3` | `brew install python` |
| js, ts | `winget install -e --id OpenJS.NodeJS.LTS` | https://nodejs.org (distro packages may be older than 22.13) | `brew install node` |
| c, cpp | `winget install -e --id Microsoft.VisualStudio.2022.BuildTools` (workload "Desktop development with C++") | `sudo apt install build-essential` / `sudo dnf install gcc gcc-c++` | `xcode-select --install` |
| rust | `winget install -e --id Rustlang.Rustup` | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` | as Linux |

Run engines' messages have no `install:` line.

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
3. A spawn error is `EngineNotFound: could not start 'C:\...\python.exe': <OS message>` (Windows: a `%1` in the message reads `the program`, and error 193 adds `(not a valid program for this machine)`).

**Child rules**, for every process V2 starts (workers, probes, compilers, run engines, compiled programs, `exec`, 5.10):
- **Own session or group:** Unix: `pre_exec` calls `setsid()` (not `process_group(0)`), so the child leads a new session and process group and has no controlling terminal: terminal SIGINT and SIGHUP never reach it, and a `/dev/tty` prompt (git credentials, ssh, sudo, `getpass`) fails at once with `ENXIO` instead of stopping the child on SIGTTIN/SIGTTOU (DOCS 4.6). Windows: `CREATE_NEW_PROCESS_GROUP` (+ `CREATE_NO_WINDOW` when the host has no console, `GetConsoleCP() == 0`, so console children share one hidden console instead of opening a window each), which blocks Ctrl+C only; Ctrl+Break and console close reach every process on the console. Children stay attached to the host's console (never `DETACHED_PROCESS`, whose descendants would open new console windows) and only stdin is replaced (null or a pipe), so a program that reads the console directly (`CONIN$`: an ssh password prompt, Python `getpass`) waits for keyboard input (DOCS 4.6). Adapters ignore SIGINT and SIGBREAK (14.3, 14.4), so a keyboard interrupt never makes a worker die mid-frame or print a `KeyboardInterrupt` trace; the host's console control handler sets `INTERRUPTED` as its first action, so failures of other children caused by the same event are never reported (6.5). stdin: a pipe for workers and for `exec` with `input`, null for the rest (DOCS 4.6).
- **Windows:** created suspended, put in its own Job Object (`KILL_ON_JOB_CLOSE | BREAKAWAY_OK`) and resumed with `NtResumeProcess` on its process handle, so every descendant is in the job unless created with `CREATE_BREAKAWAY_FROM_JOB`. **Linux:** after `setsid()`, `pre_exec` runs `prctl(PR_SET_PDEATHSIG, SIGKILL)` and `_exit(1)` if `getppid()` is no longer the host; PDEATHSIG fires when the spawning *thread* exits, so spawns and shutdown run on the interpreter thread (1.3). **All:** adapters exit on stdin EOF (macOS has only this, so a busy worker outlives a SIGKILLed host until its next read). These three also cover a host that dies with no exit path (6.5).
- **`kill_tree`** is every kill V2 performs: Windows `TerminateJobObject(job, 1)`, Unix `kill(-pid, SIGKILL)` (the session leader's pid is its pgid; `kill(pid)` on `ESRCH`), then on Linux every process still in the child's session, found through `/proc` (one that only left the process group, `setpgrp`, to keep Ctrl+C away; one that called `setsid` has detached and lives on, DOCS 16). Launchers (`py -3`, a Windows venv `python.exe`, `.cmd` shims, `bash`) run the real work in a descendant that also holds the pipes.
- **Lifetime:** workers are killed as a tree at shutdown (8.7); a run-engine, compiled-program or `exec` child is killed as a tree, with everything it started, when its block or `exec` call finishes, times out or V2 exits; probes when they exit or time out.
- **Pipes** (every child but workers, whose stdin and stdout carry frames, 8.5): every piped stream is serviced at the same time as the others. Each piped output (expression-position stdout, the stderr tail, probe, compiler and `exec` output) gets its own helper thread that collects into memory or forwards, and `exec`'s `input` is written by another, which then closes the pipe. The interpreter thread waits only for the exit (or the timer's kill), then runs `kill_tree`, then joins the output helpers, which reach EOF because the tree is dead, then reaps (below). The input writer is not joined: its writes fail once the tree is dead, and a process that escaped the tree could keep it blocked.
- **Reaping:** exit is observed without reaping (Unix `waitid(P_PID, pid, WEXITED|WNOHANG|WNOWAIT)`, Windows the process handle), then `kill_tree`, then `wait()` and the job handle closed: nothing a child started without detaching (`setsid`, `CREATE_BREAKAWAY_FROM_JOB`) outlives it, and the unreaped leader keeps its pgid from being reused.

`<run>` is the private per-run temp dir (0.1, 20.3).

### 8.4 Handshake

One deadline covers spawn → `hello` → `init` reply: `startup_timeout` (`[engines.js]`'s for ts; default 30 s; 0 = none), one timer (8.5).
- **Before `hello`:** preload noise and a first frame other than `hello`: 9.3.
- **`hello`** must carry `vep: 1` (else 9.3's `ProtocolError`). Its `caps` are recorded and gate features (17.5). For built-in engines its `version` must meet 7.2's minimum (js's for the js session, also when a ts block started it), because a launcher or alias may run another interpreter than the one probed: else kill, Dead(SpawnFailed), `EngineNotFound: Python 3.9.6 at C:\...\python.exe is too old (needs 3.10+)`, or `JavaScript 23.1.0 at C:\...\node.exe is too old (needs 22.13+, not 23.0 or 23.1)` (7.6).
- **`init`** (9.3): `ret` → Ready; `err` (of any type) → kill, `EngineCrashed: engine 'py' failed to start: <message>`. A session is not started at the cross-language depth limit, where `init` would be the 65th crossing (`RecursionError`, 9.7).
- **EOF first:** Windows exit status 9009 (a Store placeholder the probe did not catch) → Dead(SpawnFailed), `EngineNotFound: engine 'py' could not start: C:\...\python3.exe is the Microsoft Store placeholder (exit code 9009)`; otherwise `EngineCrashed: engine 'py' exited during startup (exit code 1); see its error output above, if any`.
- **Deadline:** kill, `EngineTimeout: engine 'py' did not start within 30 s (C:\...\python.exe); raise [engines.py] startup_timeout if it is only slow`.

### 8.5 Requests and call_timeout

- **Before each request:** check whether the child exited (without reaping; if so, Dead, 8.6), check the cross-language depth limit (64, 9.7) and send the session's queued releases first, but those for handles the request names (11.2).
- **Frame I/O:** compact `serde_json` plus `\n` in one `write_all`, then flush; reads split on `\n` only and drop one `\r` before it; frames are at most 64 MiB in either direction (9.1). A session's stdout is read only while the host awaits a reply from it (and in 6.5's shutdown drain), so a worker can call V2 only while the host has a request outstanding to it; adapters reject calls from idle code themselves (9.8).
- **Watchdog** (the thread: 1.3; `engines/process.rs`): a stack of timers (limit `None` = none), each with its child's `KillHandle` (the Job Object, or the pid = pgid), plus every running child's `KillHandle` (for the interrupt path, 6.5) and the run dir. Only the top timer's clock runs, and not while it is paused: a timer pushed onto another stops that one's clock until popped, and a pause lasts until its resume wherever the timer is in the stack. Each wait is bounded (a short tick, or less when the top timer's deadline or the next exit check comes first), a wake-up cuts it short (6.5's Windows handler), and every wake checks `INTERRUPTED` (6.5).
- **Timers:** each host request after `init` (load, module, call, op) pushes a timer with `limit = call_timeout` (`None` for 0); the handshake pushes one with `startup_timeout`, probes one with 7.7's limit. Timers have no kind: the code that pushed one words its message. An inbound `call` from that worker pauses the top timer, sending its reply resumes it, and the reply to the request pops it. Only the top timer runs, so exactly the innermost pending host→worker request is timed, never V2 code or another engine: V2 → py (A runs) → V2 callback (A paused) → js (B runs, popped) → py (A resumes). Run engines and compiled programs use the same timers, with `call_timeout` (message: 17.3); in-process calls (compiled exports, `extern`) cannot be interrupted and are never timed. Every 250 ms the watchdog also checks whether the top timer's child exited (`WaitForSingleObject(h, 0)`; `waitid` + `WNOWAIT`) and then runs `kill_tree`, so a descendant holding the stdout pipe cannot keep the read from reaching EOF (on Unix a descendant that called `setsid` is out of that kill's reach, so the read itself ends once the child has exited and what it left in the pipe has been read; whatever a descendant writes after that is not waited for).
- **Expiry:** the watchdog runs `kill_tree` and marks the timer fired; the blocked read gets EOF, sees it, and the session becomes Dead(CallTimeout): `EngineTimeout: a call to 'slow' in engine 'py' exceeded call_timeout (5 s); the engine was stopped` (v1 cannot cancel a call any other way). The engine stays dead for the rest of the run; later uses raise 8.2's `EngineCrashed`. A child's timers and kill registration are removed before it is reaped, so a reused pid is never signalled or waited on.

### 8.6 Crash detection

EOF on stdout, a pipe error (`EPIPE`, `ERROR_BROKEN_PIPE`, `ERROR_NO_DATA`; Rust ignores SIGPIPE), the child found exited, or a fatal protocol violation (9.11): `kill_tree`, reap, record the `DeadReason`.
- The pending request fails with `EngineCrashed: engine 'py' (pid 4242) exited during a call to 'ratio' (exit code 3221225477 = 0xC0000005); see its error output above, if any` (Windows codes ≥ 0x80000000 also in hex; Unix: `killed by signal 11 (SIGSEGV)`, or `killed by signal 77` for a signal without a known name), or with 9.11's `ProtocolError`. The worker's last output is already on stderr.
- A death while idle is noticed at the next use: `EngineCrashed: engine 'py' exited while idle (exit code 1)`; later uses get 8.2's "not running".
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
{"t":"init","id":1,"project_root":"C:\\work"}
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

**`init`** is always the first request; the adapter answers `ret` with `value: null` (an `err` kills the worker, 8.4). Its `project_root` is the canonical folder (2.1) holding `v2.toml`, or the entry script's folder. py prepends it to `sys.path` and inserts each `.v2` file's folder before it at that file's first `load`/`module` (14.3), moving an entry that an inherited `PYTHONPATH` already put there, which gives DOCS 7's order. js does not use it: Node's own lookup from each `.v2` file's folder (12.4, 13.5), plus any inherited `NODE_PATH`, finds packages.

### 9.4 Units: `load` and `module`

`load` members are those of 9.2 (there is no `col`); 12.2 says what each holds and how the adapter runs the unit, and 12.1 when a definition is loaded (once, never retried). The wire rules:
- **`unit`** is the wire UnitId `"<FileId>:<byte offset of the block's @>"`. Module units are `"<canonical tag>:<name>"` (`py:statistics`, `js:date-fns`; `ts.pkg` is `js:pkg`): one unit per module per session (13.5).
- **`imports`**: each entry has exactly one of two shapes (when each is used: 13.7): `{"as","unit","name"}` (plus `route` when the unit is a block) binds a stub that looks `name` up in that unit of this session at each call, whether or not the unit has loaded yet; `{"as","route"}` binds a stub for that route (13.6).

The `ret` of a `load` or `module` carries `value`, `exports` and `missing`. `exports` is a list of `{"name","kind","value"?}`: `kind` is `"func"` or `"value"` (whether a call constructs is decided at call time, 11.3), and `value` (the constant, copied at load) is present only for `"value"`. `missing` lists the requested names the unit does not define; the host raises `ImportError` (12.2, 13.5). An exception while running the block is answered with `err`; a foreign syntax error is `ForeignError` with `foreign_type: "SyntaxError"` at the `.v2` line (12.3, 12.4, 12.6).

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

**How the host converts `err`:**
- With `v2`, the host restores that error (type, message, data, location) and puts the foreign frames into its trace, between the frames of the callback and those below the request; an error never raised before (made with `error()` and raised by foreign code) gets V2's frames at the call site after them, as its first raise (5.9). So a V2 error thrown in a callback comes back with its original type, whatever its kind (DOCS 10).
- Without `v2`, the kind is `type` if it is one of the seven kinds above; any other `type`, a host-only kind such as `EngineNotFound` included, becomes `ForeignError` with `foreign_type = type`, so a worker's own error can't make V2 exit 3. The location is the innermost frame that maps into a `.v2` file, else the V2 call site (2.4, 12.6); `e.lang` follows 12.6. If `trace` is empty, the lines of `text` become the trace.
- Adapters make every error string well-formed first (10.4).

**The other direction.** When a worker's `call` fails (a V2 function, or another language's function behind a route or `fn`), the host's `err` always carries `v2`. Adapters raise it as `V2Error` (14.2), which keeps the tag, so if foreign code does not catch it, it goes back in `v2` unchanged.

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

**User code cannot end the worker.** Request handlers catch everything (Python `BaseException`, any thrown JS value) and reply `err`: `sys.exit()` becomes a `ForeignError` with `foreign_type: "SystemExit"`, and JS `process.exit()` throws an `Error` (14.3, 14.4). Adapters ignore SIGINT and SIGBREAK; the host owns interrupts (6.5).

### 9.8 When a worker may call V2

**Only while V2 is waiting on this engine.** A stub may send `call` only while the innermost entry on the adapter's stack is a host request that is executing: during a synchronous call, while that request's coroutine or Promise is awaited, or during a nested request served inside a stub. A callback stored earlier stays valid and may be called during any later request. A timer, event handler, thread or `__del__` that fires while the worker is idle, or while it is blocked in a stub (V2 is computing then), gets a `V2Error` (type `Error`) raised in the foreign code, and nothing is sent: `V2 function 'progress' called outside an active V2 call` (DOCS 8.4). In JavaScript, timers and event handlers run only right after a call returns (the event-loop turn the adapter yields after every outermost reply, 14.4), or while V2 is waiting on a JS Promise. One that calls V2 right after a call fails; one that fires while V2 awaits a JS Promise is allowed.

**Only from the dispatch thread.** Python stubs compare `threading.get_ident()` with the dispatch thread's id: `V2 function 'progress' called from another thread`. JS stubs always run on the main thread, because functions cannot be sent to `worker_threads`.

The host cannot enforce either rule: it reads a worker's frames only while a request to that worker is pending.

**Awaiting (the async rule).** The result of a `call`, and the completion value of a `load` even with `want_value: false`, are awaited when awaitable (`inspect.isawaitable`; a thenable). `op` results never are.
- **py:** `loop.run_until_complete(x)` on the session's event loop (one per session, created on first use), at any depth. If an event loop is already running in the thread, the reply is `err` type `Error`: `'fetch' returned a coroutine while another coroutine is being awaited; call it directly from V2` (for a block's value: `the block's value is a coroutine while another coroutine is being awaited in this thread`).
- **js:** only a request served from the idle listener (stack depth 1) is awaited, and the event loop keeps running meanwhile (14.4). Inside a stub's synchronous wait the event loop cannot run, so the reply is `err` type `Error`: `'fetch_json' returned a Promise during a nested cross-language call; only an outermost call can be awaited` (DOCS 8.4; for a block's value: `the block's value is a Promise during …`).
- Before either error reply the adapter calls `coro.close()` or attaches a no-op rejection handler, so no "never awaited" or unhandled-rejection warning appears.

### 9.9 Output and worker stdio

**Stdio contract** (implementation: 14.3, 14.4). Before any user code runs, the adapter keeps user code off the protocol stream:
- **Protocol descriptors.** The protocol moves to private non-inheritable descriptors, then fd 0 points at the null device and fd 1 at fd 2. Python does this with `os.dup` of fds 0 and 1, then `dup2` of the null device to fd 0 and of fd 2 to fd 1 (14.3). Node has no `dup2`: on Unix the host starts it with the protocol on two pipes of its own, whose numbers are in `V2_VEP_FDS` (the adapter removes it), and with fd 0 on the null device and fd 1 on the host's stderr (`EngineSpec.protocol_fds`); on Windows the protocol stays on fds 0 and 1. Either way it writes frames with a saved `fs.writeSync` and reads with `fs.readSync`, and wraps everything else that reaches fds 0 and 1: `process.stdout`, `console` and `fs` writes to fd 1 are captured, `process.stdin` and `fs` reads of fd 0 are replaced, and `child_process` stdio is rewritten (14.4). An adapter for another language without `dup2` does the same as Node.
- **Language-level output** (`print`, `sys.stdout.write`, `console.log`, `process.stdout.write`) is sent as `out` frames, in program order; output a request produces goes out before that request's `ret` or `err`.
- **Fd-level output goes to stderr:** C extensions, `os.write(1, …)`, and `os.system`/`subprocess`/`child_process` children that inherit stdout. A forked Python child (`multiprocessing`'s default on Linux) is one of them: an `os.register_at_fork` hook closes the protocol descriptors in it, points `sys.stdout` and `sys.stderr` at fds 1 and 2, sends nothing at exit, and refuses calls into V2.
- **Stdin.** Language-level stdin is replaced by a stream whose reads raise (14.3); fd-level and child reads see EOF. Python's `sys.stdin.fileno()` is 0 and `sys.stderr.fileno()` 2; `sys.stdout.fileno()` raises `UnsupportedOperation`, since V2's stdout has no descriptor in the worker (fd 1 is stderr, and saying 1 would send console-aware libraries such as click there), so `subprocess.run(…, stdout=sys.stdout)` fails with that message. The user's stdin belongs to V2's `input()` (DOCS 4.6).
- **Remaining gap.** In Node on Windows, code that bypasses the wrappers (a native addon) can still write fd 1 and corrupt the stream (fatal `ProtocolError`, 9.11), or read fd 0 and steal protocol bytes.

**Adapter side.**
- Output is buffered and flushed (as `out` frames, by 14.2's `send`) at: a write ending in `\n`, the size threshold, an explicit flush, right before any other frame, and at `bye`. JavaScript has no flush, so during a request its writes go out at once, as Node's own stdout does. `out` text is sent in chunks below the frame limit. Queued releases go out as one `release` frame just before the next frame.
- An adapter writes frames only while it is executing a host request or handling `bye`; during a request any thread may flush `out` under the send lock.
- Output produced while idle (Python non-dispatch threads, signal handlers, JS timers) stays buffered and goes out with the next frame. Once 1 MiB of it is buffered, a Python non-dispatch thread that writes more waits until the next request starts.
- Lone surrogates in output become U+FFFD (10.4).

**Host side.**
- `out` text for `"stdout"` goes through `Out`, the writer of V2's `print` (5.10); `"stderr"` text goes to the host's stderr after `Out` is flushed. A failed write to stdout (a closed pipe) is raised as `print`'s `IOError` when the request that wrote it gets its `ret`, so a program whose reader has gone stops there like one using `print`; if that request gets an `err`, that error wins. The worker can't be stopped mid-request, so a block runs to its end first.
- While a worker runs, the host is blocked in `request()` for it and handles its frames in arrival order, so block output and V2 output appear in program order, also when redirected (DOCS 4.6). The host flushes `Out` before writing any frame and before `input()` blocks.
- Idle output appears before the worker's next frame; its position relative to V2 output printed in between is not guaranteed.

### 9.10 `release` and `bye`

**`release`** carries refcount decrements (ch. 11).
- `h` (host→worker) lists handle ids the host dropped; `fn` (worker→host) lists callable ids whose stubs died. An id appears once per dropped reference. No reply; applied at once; unknown ids are ignored. Sessions without the `handles` cap never get `release`.
- Neither side writes it from a finalizer or on its own: finalizers only append to a queue, which goes out as one `release` right before that side's next frame to the other; the host holds back a release for a handle that frame names (11.2).

**`bye`** is sent on every orderly exit path (end of program, `exit()`, uncaught error; 6.5, 8.7), possibly while the worker is blocked in a stub; interrupts kill workers instead. The adapter flushes its output, runs the language's exit handlers, flushes what they wrote, and exits with status 0 (on Windows it ends itself after the `exit` handlers, without Node's teardown, whose libuv assertion can fire while a `fetch` winds down; the host ignores the status of a worker it said `bye` to) without running user `except`/`finally` code:
- **Python at depth 0:** `sys.exit(0)` from the main loop, so normal finalization and `atexit` handlers run.
- **Python inside a stub:** `atexit._run_exitfuncs()`, a flush, then `os._exit(0)`; raising `SystemExit` there would run through user code.
- **Node:** the saved `process.exit(0)`.

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
| `error` | `type`, `message`, `data`, `lang`, `foreign_type`, `file`, `line`, `col`, `trace` (strings), `cause` (tag or `null`) | a V2 error value |
| `dict` | `v`: object | a real dict that has a `"$v2"` key: the keys of `v` are literal, its values are decoded normally |

An unknown tag or a missing member is a fatal `ProtocolError` on the host (9.11); adapters answer it with `err` `ProtocolError`.

### 10.4 Strings

Host strings are always valid UTF-8. Foreign strings can hold lone surrogates: in Python any code point U+D800–U+DFFF (for example `surrogateescape` file names), in JS an unpaired UTF-16 half. Handling depends on where the string appears:
- **Values** (arguments, results, attribute values, dict keys): `MarshalError: string contains an unpaired surrogate (U+DC80) at index 12`. Python detects it through the `UnicodeEncodeError` of `s.encode("utf-8")`; JS through `s.isWellFormed()`.
- **Text** (`out`, error messages, `text`, traces, `str` op results, the `type` of `h` tags): each lone surrogate becomes U+FFFD (Python `re.sub("[\ud800-\udfff]", "\ufffd", s)`, JS `s.toWellFormed()`), so printing and error reporting never fail.

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
1. The host sends `op iter {h}`; the worker answers `{"it": <iterator handle>, "mapping": bool}`. `mapping` is true for a Python `collections.abc.Mapping` and a JS `Map` (`util.types.isMap`), whose iterator yields `[key, value]` pairs (`iter(obj.items())`, `obj.entries()`). Otherwise the worker calls `iter(obj)` (Python) or `obj[Symbol.iterator]()` (JS; not iterable → `TypeError`).
2. The host repeats `op next {h: it, n: 64}`; each answer is `{"items": [...], "done": bool}`.
3. The loop ends when `done` is true and the received items are used up. `break` or an error simply drops the iterator handle.

Over a **mapping handle** (`mapping` true), `for k in h` gives the keys and `for k, v in h` the key and the value, as for dicts. Over any other handle, one variable gets the item and two get the index and the item, as for lists.

**Batching.** A `next` returns more than one item only if the iterator's `batch` flag is set. The adapter sets it at `iter` time when the source is an eager built-in container whose iteration runs no user code: Python `list`, `tuple`, `dict` and its views, `set`, `frozenset`, `str`, `bytes`, `range`, numpy `ndarray`; JS `Array`, typed arrays, `Map`, `Set`, strings, when their iteration methods are the built-in ones (a subclass may override them). Other iterators (generators, `map` objects, `itertuples()`) return one item per `next`, so foreign side effects such as prints stay in step with the loop body. Items are encoded at the depth they have inside the reply (two levels down, three for a mapping's pair, 10.5). A batch stops before an item that fails to encode or would take the reply past half the frame limit; that item comes first in the next `next`, so the items before it still arrive.

### 11.5 `to_v2(h)`

`op value {h}` converts the object to plain data inside the worker, recursively up to depth 256 (a container at level 257 is too deep, as for the host, 10.5); deeper nesting or a cycle is a `MarshalError`. At each node:
1. **Plain values** convert as in 10.6/10.7; V2 functions and error values held by the object go back as themselves.
2. **Containers** convert element by element: Python `list` and `tuple` (subclasses included, such as namedtuples), `set` and `frozenset` (→ list, in iteration order) and any `collections.abc.Mapping` (→ dict); JS arrays, plain objects, `Set` (→ list) and `Map` (→ dict, keeping the `Map`'s order). A mapping key that is a string stays; one that converts to `int` (within i64), `float`, `bool` or `null` becomes its V2 `str()` form, not the language's (5.10: `1` → `"1"`, Python `True`/`None` → `"true"`/`"null"`, Python `1e-05` → `"1e-5"`, JS `1e+21` → `"1e21"`); any other key, or two keys with the same form, is a `MarshalError`: `to_v2(): can't convert dict to V2 data (key of type tuple)`, `… (two keys become "1")`.
3. **Hooks.** Otherwise a callable `to_dict` is tried first, then a callable `tolist`, and the result converts from step 1 at the same level, with the object on the cycle path (a hook that returns it is a cycle). More than 256 hooks open at once is too deep, so a chain of hooks that return new objects ends. So a pandas `Series` becomes a dict (DOCS 17.2; with the default index `{"0": …, "1": …}`) and a numpy array a list.
4. **Anything else** is `MarshalError: to_v2(): can't convert datetime.date to V2 data`: a `datetime`, a class instance without hooks, an int outside i64, a `Decimal` or `Fraction`.

A hook that raises gives a `ForeignError`. `to_v2(x)` on a non-handle returns `x` unchanged (5.10); on a native pointer it is a `MarshalError` (16.6).

### 11.6 Callable handles, identity, diagnostics

- **Callable handles.** Foreign functions, bound methods, classes and callable objects that come back as values (call results, `getattr`, attributes of a module handle) are handles marked `callable` (11.1): `type(h)` is `"handle"`, and they are called as `h(...)`. Names brought in with `@import` are `func`s (Proxies, 13.4), not handles. Sent to another session, a callable handle goes as an `fn` tag whose callable-table entry calls it through its own session (11.8); other handles stay with their session (11.7).
- **Equality.** `h1 == h2` is true exactly when session and id match. The identity map (11.1), together with removal only at count 0, makes this mean "the same foreign object" for all live handles. A handle equals nothing else.
- **No round trips.** `type(h)`, truthiness (always true) and diagnostics never contact the worker. Diagnostics show `<py datetime.date>`, built from `type_name`.

### 11.7 Invalid uses

| Use | Error |
| --- | --- |
| a non-callable handle sent to another session (in args, kwargs, block args, or inside lists/dicts) | `MarshalError: this handle belongs to engine 'py' and can't be passed to 'js' — convert it with to_v2(h) first`, plus the path (10.5). The one exception is an error answering a worker's `call`: a handle in its data or cause goes as a function, so that foreign code that doesn't catch the error sends it back whole (9.6); called, that function raises V2's error for calling a handle that can't be called |
| a handle as an argument of a compiled export or `extern` function; returned to compiled code by a reverse call | `MarshalError: cannot pass a py handle (datetime.date) to native code; convert it with to_v2(h) first` (16.5); `MarshalError` (15.9) |
| any use after its session died | `EngineCrashed` (8.2) |
| arithmetic, ordering, `in`, slicing | `TypeError` with the "do it inside a block / `to_v2(h)`" hint (5.2, 5.3) |
| a handle as a dict key (`d[h]`, `{[h]: 1}`) | `TypeError: dict keys must be str, got handle` |
| `json_stringify(h)` | `TypeError` (5.10) |

### 11.8 V2 callables inside workers

When the host encodes for session S a V2 `func` (a closure, a builtin, a `Proxy` of another session, a `Native` function or a `HandleMethod`) or a callable handle of another session (11.6), it registers the value in S's callable table and sends `{"$v2":"fn","id":n,"name":"log_line"}`. A `Proxy` of S itself goes as `{"$v2":"fn","unit":u,"name":n}`, which the adapter decodes to the current target of that name (in JS, 13.7's `Proxy` for a function read from a module object), so a function or class imported from a language is the real thing back in that language, with no hop per call. A session without the `reverse` cap gets a `MarshalError` instead (17.5).

- **Reuse.** The table maps id → (value, count) and the value's `Rc` address → id. Sending the same `Rc` (a `Callable`, or a callable handle's `HandleRef`) again reuses its id and does `count += 1`. The table holds the `Rc`, so the address key stays valid.
- **Worker stubs.** The worker keeps one stub per id in a weak map (Python `weakref.WeakValueDictionary`, JS `Map<id, WeakRef>`), so one V2 function is one foreign object (`f is g`); its name is the V2 name (a callable handle's is its diagnostic form, `<py function>`, 11.6). Each stub owns a cell `{id, n}` counting the `fn` tags it absorbed.
- **Release.** When a stub is collected, its finalizer (Python `weakref.finalize`, JS `FinalizationRegistry`) only appends the id `n` times to the release queue, which goes out as `release {fn: [...]}` before the worker's next frame. A stub encoded into a frame stays referenced until that frame is written, so its release never goes out ahead of a frame that names it. The host subtracts once per occurrence and drops the entry at 0. A stub that dies just as its id is sent again is safe, because the new stub gets its own cell.
- **Calls.** Calling a stub sends `call {"fn": n}` (9.5, 14.2). The host calls the value (a callable handle: `call {h}` to its own session, 11.3), re-entering the interpreter, and encodes the result for S; a V2 error goes back as `err` with `v2` (9.6).
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

A block is **reached** each time control executes its statement: a block inside a function is first reached by the first
call that runs it, one in a loop on the first iteration, and one in an `if` arm never taken is never reached. A
definition block (with `@export`) loads on its first reach, unless it waits (12.7); later reaches do nothing, or re-raise its stored load error
(step 1). An action block (no `@export`) goes through steps 2–8 on every reach, args included, each time in a fresh
namespace (bridged: a new unit, dropped after its reply; compiled: the program, built or reused, 15.5; run: 17.3). Inside `try` a load error is
catchable, so `try { @py probe { … @export { … } } } catch (e) { … }` is a probe; reaching it again re-raises the stored
error.

`exec_block(b, want_value)`:

1. **State** (definition blocks): `Loaded` or `Waiting` → return `null`, evaluating nothing, not even the args; a block that may wait and whose language isn't running goes to `Waiting` instead (12.7). `Failed(e)` →
   throw the stored `e` unchanged: no retry, no repeated side effects (an `@import` from it gets 13.3's `failed to
   load`). `Loading` → `ImportError: block 'stats' is still loading (reached again during its own load)`. Otherwise set
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
item; the unit is `Failed` and the adapter has already dropped it. `func` entries (classes included) become Proxies
(13.4); a `value` entry is the copy taken at load (non-plain values arrive as handles). Constant exports exist for
bridged engines (py, js, ts, custom bridged), not compiled.

**Adapter side.** Arg names and import aliases must be identifiers of the adapter's language that are neither keywords
nor names the adapter provides (Python: `str.isidentifier()`, not `keyword.iskeyword()`, no dunders; JS: 12.4's identifier
pattern, not reserved, not `undefined NaN Infinity globalThis __proto__`), else `err ValueError: block arg 'pass' is not
a valid Python name` (`@import name 'pass' …` for an alias). Args bind first, then imports, then the code runs. The completion value (py: the last statement
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
    if keep: units[m["unit"]] = mod, {e["name"]: e["kind"] for e in reply["exports"]}  # kinds: 13.7 (14.2)
    return reply
```

- The prepended blank lines act like `ast.increment_lineno(tree, first - 1)` and also put `SyntaxError`/`SyntaxWarning`
  on the right `.v2` line, so `linecache` shows real `.v2` lines in tracebacks Python prints itself; the text V2 sent is
  unchanged. Columns stay body-relative (12.6); `from __future__` works; `dont_inherit=True` keeps the adapter's
  flags out.
- While it lives, the unit is in `sys.modules` (for `pickle`, `dataclasses`, `typing.get_type_hints`) under a name with
  no dot (`pickle` imports a dotted name's parent); `__name__` is never `"__main__"`.
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
  not erasable and fail with `ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX`; such code is written in erasable TypeScript or in
  `@js`. The stripper parses the whole block, so it also throws on any syntax error, before V8 compiles anything. On
  any stripping failure the adapter sends `err` `ForeignError` (foreign_type `SyntaxError`) with one trace frame
  `{file: m.file, line: m.line + L − 1, col: C}`, where L:C is the 1-based body position the stripper reports (a
  `[line:col]` marker in its message, or, on Node 24, its stack's first line `:L` and a caret under column C); with no
  readable position it sends no frame, so the error stays at the block's `@ts` (12.6). The message ends with Node's
  code: `TypeScript enum is not supported in strip-only mode (ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX)`. A `process.emitWarning` filter drops the one `ExperimentalWarning` it prints.

### 12.6 Foreign positions to .v2 positions

- **Adapters** compile with the `.v2` path as filename and the block's line offset (12.3, 12.4), so frames inside blocks
  already carry the `.v2` file and line. V8 also counts U+2028 and U+2029 as line breaks, even in strings, so the js
  adapter maps its lines in a body holding them back to `.v2` lines. Frames go innermost first, in 9.6's shape; adapter frames and engine
  internals (`<frozen importlib._bootstrap…>`, `node:internal/…`, `node:vm`) are dropped; library frames stay.
- **`col`** is 1-based code points relative to the body text line (kept by `remember_source`), sent only for frames in
  `.v2` files, else `null`. Python 3.11+: `FrameSummary.colno` (0-based UTF-8 bytes) converted; 3.10: `null`. JS:
  parsed from `err.stack` (`at fn (file:line:col)`, UTF-16 units) and converted. Syntax errors: Python `e.offset` (the compiler's, unlike the
  parser's, is in UTF-8 bytes and converted); V8's caret line; ts type stripping: the stripper's position, which gives the line too (12.5).
- **Host:** a frame in a known `.v2` file whose line lies in a block body gets body line `line − loc.body_first_line + 1`
  and its column from `UnitLoc::col` (2.3; a `null` col becomes the line's first non-blank column), and prints as 2.5 says
  (`in @py risky`).
- **Error fields:** `e.file/line/col` come from the innermost mapped frame, else the V2 call site (2.4; the excerpt prints
  either way); `e.lang` is that frame's block tag (`ts` for ts blocks), else the session's tag.

---

### 12.7 The waiting line

A language has no process and costs nothing until something needs it. `Interp.line` holds what would have run in a language by now: one line for all languages, in program order, each place with its engine key (ts is js).

- **Running** means a session exists, `Dead` included, so a dead engine's blocks raise as 8.2 says and never wait.
- **What waits** while its language isn't running: a definition block that is a direct statement of its file, with no args and no inner `@import … from v2`, and an eligible `@import` (13.4). Everything else runs when reached: action blocks, and blocks and imports nested in `if`, `try`, loops or functions, so `try` around them works as it always did.
- **At reach**, 12.1's step 3 runs first, on the spec from `registry.canonical`, with no probe: an unknown tag raises `EngineUnknown`, a run engine with `@export` raises its `ImportError`. Then the unit becomes `Waiting` and joins the line.
- **First need:** a block reached that can't wait, a waiting reference used, a route resolved, or an `@import` that can't wait. Before a block counts as started (`Loading`), the depth limit is checked if its language isn't running, and whatever waits for that language runs, so neither failure is stored as that block's. `session_for` starts the session and runs the line before returning to what needed it.
- **Running the line.** Again and again, the first place in program order whose language is running. While a place runs, only places before it may go: a language started by that place runs what came earlier, and what comes after the place comes after it in the program too, so it runs once the place is done. `xdepth` is saved, zeroed and restored around each place, because the depth limit (9.7) belongs to the caller's chain and must not fail a block forever.
- **Once running**, every statement acts as if the line had never existed.
- **Failures.** A block that fails is `Failed` as at any load, and a failed `@import` leaves its references waiting, so they retry at their next use. Neither stops the line: the rest still runs, and the first error then goes to what needed the language. `exit()` ends the line at once. A use after a failure raises 13.3's `failed to load`.
- **References.** A waiting `@import` joins the line and binds each local name to a `Callable::Waiting`, which names no process, so it can exist before one does. The blocks reached by its statement are kept with it: it finds what it would have found there, so a block further down can't make it ambiguous or supply a name. A call, or the member and method steps of a chain (5.5), settle it: start the language at that use, then run the `@import` as 13.3 and 13.5 say, raising what it would have raised, and go on with what came back, which can be a Proxy, a constant, a handle or `null`. A failure leaves it waiting. Once a Proxy exists its session does too, and sessions are never replaced (8.1), so no reference can dangle. 13.4's rule guarantees nothing else meets a `Waiting`, so `==`, the encoders and printing never do.
- **Compiled blocks** have no process and no line: a definition block is `Waiting` until the first call into it (15.1).
- **The visible difference** (DOCS 5): a block in the line runs when its language starts, so its top-level effects come later than its position, or never, and it sees V2 state from then. Its errors surface at that later point.

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
selectors as written.

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

1. `cands` empty → `ImportError: No engine exports found for selector 'stats'`.
2. `hits` = the candidates that provide `name`.
3. One hit, `Loaded` → its entry's `value`.
4. More → `ImportError: Import of 'mean' from '@py' is ambiguous (exported by blocks 'stats' and 'calc') — use @py.stats
   or @py.calc` (DOCS 6.3). Three or more read `'a', 'b' and 'c'` / `@py.a, @py.b or @py.c`; an unlabeled block is listed
   as `@py block at main.v2:12`, and the advice becomes `— label the blocks to choose one`.
5. One hit, not `Loaded` (with no hit: the first candidate with `*` that is not `Loaded`) → by its state: never reached →
   `ImportError: block 'stats' has not run yet — move this @import below it`; `Loading` → `ImportError: block
   'stats' is still loading — its exports exist once its top-level code finishes`; `Failed(e)` → `ImportError: block
   'stats' failed to load: <e.message>`.
6. Otherwise → `ImportError: 'x' is not exported by 'stats' (did the block @export it?)`, or, when a candidate is a
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

A `Module` selector names one unit per module per session, `"<canonical tag>:<name>"` (9.4): `py:statistics`,
`py:os.path`, `js:date-fns`; for a relative (`./`, `../`; on Windows also `.\`, `..\`) or absolute JS specifier `<name>` is the absolute path joined from the
importing file's folder. `ts.X` and `js.X` share the js session's unit `js:X`. Only bridged engines take module
selectors (`ImportError: module imports need a bridged engine — 'c' is compiled` / `— 'lua' is a run engine`), and only
with the `modules` cap (17.5). Python names must be dotted identifiers, each segment a letter or `_` followed by letters, digits or `_`, and not a Python keyword, checked before anything is sent (`ImportError: 'my-mod' is not a valid Python module name`); a JS name is any non-empty specifier
(package, `node:` built-in, `./path` from the `.v2` file). Every `@import` from a module sends one `module` frame; the
adapter imports once per unit, so a bare name's first import wins for the session, as in `sys.modules` (a JS package
resolves from the folder of the first `.v2` file that imports it):

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
- A missing requested name → `ImportError: 'x' is not exported by 'py.statistics'`. A module Proxy targets
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
  main.v2`; a constant one (`const`, `import`, `@import`) that holds something other than a function:
  `ImportError: 'x' is a constant — pass it in as a block arg` (a `let` may still get a function before the first call).
- A name the target exports as a constant → `ImportError: 'VERSION' is a constant — pass it in as a block arg`: at load
  when the target is `Loaded` or a module of this session (imported then), else at the first call.
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
  `PYTHONIOENCODING=utf-8`; `<node> <run>/adapters/js.js` with nothing added, so an inherited `NODE_PATH` reaches Node
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
  Python, not the dispatch thread → raise V2Error (type Error): V2 function 'progress' called from another thread
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
  exposes `type`, `message`, `data`, `lang` and `foreign_type` from the `error` tag (10.3).
- **Output** follows 9.9. Every string sent as text is first made well-formed (10.4).

### 14.3 py.py

All code runs under `if __name__ == "__main__":`, so a `multiprocessing` spawn child (the default on Windows and macOS)
that re-imports `__main__` starts no second adapter. Setup, in order:

1. Import every stdlib module the adapter uses (`sys os io json ast types builtins inspect importlib keyword traceback
   linecache weakref threading signal base64 codecs collections`, plus `msvcrt ctypes` on Windows) before the project root or any `.v2`
   file's folder is on `sys.path`, so a project's `json.py` cannot replace them (an inherited `PYTHONPATH` is already
   there: the user's own setting, 20.2); `asyncio` is imported on first need with the original
   path (9.3).
2. Protocol, stdio and signals exactly as 9.9: the protocol moves to non-inheritable `os.dup` copies of fds 0 and 1
   (binary mode on Windows); fd 0 becomes `os.devnull` and `os.dup2(2, 1)` points fd 1 at stderr (plus `SetStdHandle` on
   Windows); `sys.stdout` and `sys.stdin` are replaced; SIGINT (and SIGBREAK) are ignored, since the host owns Ctrl+C
   (ch. 8). C extensions, `os.write(1, …)`, `os.system` and inheriting `subprocess` children therefore write to stderr
   and read EOF. `threading.excepthook` and `sys.unraisablehook` print as Python's own do, with the frames 12.6 keeps.
3. Remove the adapter's own folder (`sys.path[0]`, i.e. `<run>/adapters`). `init` then puts the project root on
   `sys.path`, and each `.v2` file's first `load` or `module` inserts that file's folder before it (9.3); the venv's
   packages and an inherited `PYTHONPATH` are Python's own.

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
   methods a preload took for itself following; its `trace` prints the frames 12.6 keeps), `fs` calls on fds 0–2 are wrapped (also the ones Node routes past `readSync`/`writeSync`:
   `readFile(Sync)`, `readv(Sync)`, `writeFile(Sync)`, `appendFile(Sync)`, `writev(Sync)`), `child_process` stdio is
   rewritten at every index, `fork`'s default (`inherit` plus `ipc`) included, so children never inherit fd 0 or 1
   (their output goes to stderr); the wrappers keep their originals' properties (`util.promisify.custom`). Then
   `module.syncBuiltinESMExports()`. Frames are written only through the
   saved `fs.writeSync` on the protocol fd (9.9), the adapter's private channel, which stays blocking. On Windows, native
   addons that write fd 1 are not covered; after `hello` their lines are a `ProtocolError` (9.11).
3. `process.exit` is replaced by a function that throws an `Error`, so a block cannot end the worker; the original is
   kept for `bye` (9.10).
4. `process.on('SIGINT' | 'SIGBREAK', () => {})`; `uncaughtException` and `unhandledRejection` print the error to stderr
   with the frames 12.6 keeps, by line, then an `AggregateError`'s errors (from any realm, 8 levels at most) and up to 8 causes (`caused by:`), each error once (`(shown above)` after that); a `V2Error`
   from V2 adds its type and the V2 frame that raised it
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

- **File:** the project's `v2.toml`, the only config file; without one, the built-in defaults apply. `~` is `%USERPROFILE%` on Windows and `$HOME` elsewhere, read from exactly these variables (22.2); `~/.v2` holds only caches (7.7, 15.6). With no home folder the caches are off.
- **No ownership checks:** V2 never refuses `v2.toml` or `~/.v2` because of owner or permissions; it is a same-user tool (20.1).
- **Project root:** start at the entry file's folder (`v2 FILE`, `v2 check FILE`) or the cwd (`v2 doctor`), made absolute with `.` and `..` applied to the text (as Windows does; POSIX `std::path::absolute` keeps `..`, and walking up `/proj/../other` would visit `/proj`), and walk up; the first folder holding a regular file `v2.toml` (an empty one counts) is the root, else the start folder. One project per run: V2 modules imported from other folders use the entry's config.
- **Relative paths** resolve against the folder of whatever declared them: `v2.toml` → the project root (DOCS 14); `V2_ENGINE_*` → the cwd. This covers `[python] venv`, engine commands and compiler `flags`:
  - `command[0]`: with a separator, joined to that folder; without one, the `PATH` walk (7.5).
  - A later `command` element is made absolute against that folder only if it starts with `./` or `../` (also `.\`, `..\`), or if it doesn't start with `-`, contains no `{`, and `<folder>/<element>` exists. So `["ruby", "adapters/v2_adapter.rb"]` works from any cwd and `["sh", "-c", "cd src/x && make"]` is left alone. Nothing else is rewritten: there is no shell, `{file}` is the only placeholder (17.3), and `~` and `$VAR` are not expanded.
  - `flags` (`c`, `cpp`, `rust`): an element starting with `./` or `../` (also `.\`, `..\`), or such a path right after `-I`, `-L`, `/I` or `/LIBPATH:` (`-I./include`), is made absolute against that folder.
  - Paths used by scripts are separate: `read_file`, `exec` and the other file builtins resolve against the cwd (5.10); `import` and `extern` against the `.v2` file's folder.
  - A Windows drive-relative path in `v2.toml` (`C:.venv`) would resolve against that drive's cwd, so it is a configuration error: `[python] venv: 'C:.venv' is a drive-relative path: write it relative to the file, or absolute` (as 6.4 does for imports).

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

A `command` given as a string is a configuration error that spells out the array: `[engines.lua] command must be an array: write ["lua", "{file}"]`. Its elements are the string's words, split on whitespace, each in double quotes, or in TOML literal quotes when it contains a backslash (`write ['C:\Git\bin\bash.exe', "{file}"]`).

Any `[engines.ts]` table is a configuration error: `[engines.ts]: ts runs on the js engine; configure [engines.js]`.

### 18.3 Layers and merging

Built-in defaults < the project's `v2.toml` < `V2_ENGINE_<TAG>` (DOCS 14).
- One file is read, so nothing merges across files. `V2_ENGINE_<TAG>` replaces a built-in bridged or compiled engine's whole `command` (which is then never validated) and any other engine's `command[0]` (18.4).
- A configured py command (`[engines.py] command` or `V2_ENGINE_PY`) wins over `[python] venv`.
- Every configured value records its source, `ConfigSource = Project(PathBuf) | Env(String)` (a built-in default has none); doctor's FROM column shows it, or the resolver's own origin (`PATH`, `venv`, `vswhere`, `git`; 7.1, 19.2).

### 18.4 Environment overrides

- `V2_ENGINE_<X>` holds one executable path, never split; empty = unset. The name is matched case-insensitively on Windows (as Windows matches variable names) and must be upper case elsewhere (`V2_ENGINE_Py` is ignored), so two variables never name one engine. `X`, lower-cased, must be a canonical tag (`V2_ENGINE_PY`, not `V2_ENGINE_PYTHON`): an alias is ignored like any unknown tag. A path with a separator resolves against the cwd; a bare name (`node`) is left for the `PATH` walk (18.1, 7.5).
- Effect: a built-in bridged or compiled engine gets `command = [PATH]`, dropping any prefix such as `py -3` (a `cl.exe` path follows 7.6 step 3); a built-in run engine or a custom engine gets `command[0] = PATH`, the rest kept (a configured `command`'s, else 7.2's fixed args). A tag that no engine has is ignored, because environments hold unrelated variables; one a later `define_engine` declares gets it then (17.2). `V2_ENGINE_TS` is a configuration error that names the variable as written: `V2_ENGINE_TS: ts runs on the js engine; set V2_ENGINE_JS`.

### 18.5 Parsing and validation

- A leading UTF-8 BOM is dropped (invalid UTF-8 is an error); the `toml` crate parses the file into a `toml::Table`; validation is hand-written (1.4).
- Errors exit 2 before anything runs. Syntax: `C:\proj\v2.toml:12:5: config error: <toml message>` (position from the error span). Wrong type or value (a `toml::Table` keeps no positions): `C:\proj\v2.toml: config error: [engines.py] startup_timeout: expected a non-negative number, got string "30s"`; custom engine tables use the same form with 17.1's problem text.
- A known section (`engines`, `python`) that is not a table is a type error (`[engines]: expected a table, got integer 1`); any other top-level key is an unknown key.
- Unknown sections and keys, also in custom engine tables, are errors with did-you-mean: `C:\proj\v2.toml: config error: unknown key [engines.py] comand (did you mean 'command'?)`. So are keys that do not apply: `C:\proj\v2.toml: config error: [engines.py] flags only applies to c, cpp and rust`. Config has no warnings.

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

`--version` and `--help` are the only flags. A first word that is exactly `check`, `doctor` or `cache` is a subcommand, else the script (`v2 ./doctor` runs a file named doctor); everything after FILE goes to `args()` untouched. No arguments, any other first word starting with `-`, a missing or extra argument after a subcommand (`v2 doctor py`), or a missing or unreadable FILE, or one whose name isn't valid UTF-8: a message on stderr, exit 2.

| Exit | Meaning |
|---|---|
| 0 | success: the entry file ran to its end; check found no errors; doctor and `cache clean` finished |
| n | `exit(n)`: an int in i32 range; POSIX keeps the low 8 bits; no `finally` runs (5.8) |
| 1 | an uncaught error, printed per 2.4 (`EngineUnknown` included), or a `SyntaxError` in the entry file (6.1); check found errors; a run whose final stdout flush fails turns 0 into 1 (6.5) |
| 2 | usage or configuration error (18.5), before any V2 code runs; `v2 cache clean` without a home folder |
| 3 | uncaught `EngineNotFound` |
| 130 | interrupted: Ctrl+C, Ctrl+Break, console close, SIGINT, SIGTERM, SIGHUP, SIGQUIT; always 130, never 128+signo; no `finally`, engines stopped, what was printed written, `<run>` removed (6.5, 8.7) |
| 101 | internal panic, a V2 bug (6.5) |

### 19.2 v2 doctor

1. Load config from the cwd (18.1; error: exit 2); the registry is the built-ins (7.2, the built-in run engines included), then the config's custom engines, one row each (doctor runs no script, so `define_engine` engines are not listed). Resolve every engine with fresh probes (7.7), rewriting the probe cache. Doctor executes toolchain commands, including project-configured ones (`v2.toml` commands, the venv python, vswhere and vcvarsall), so it is not safe on an untrusted project (20.2).
2. Print a table to stdout, columns `ENGINE STATUS COMMAND VERSION FROM`, each as wide as its longest cell + 2 (DOCS 2). COMMAND: `shown` plus prefix args (a built-in run engine's: the rest of its template, `{file}` included), space-joined, unquoted; ts: `(js engine, built-in type stripping)`; custom: the argv template. VERSION: the probed version (ts: Node's); `-` for `cmd` and custom engines, which have no probe. FROM: `PATH`, `vswhere`, `venv`, `git`, `v2.toml`, `V2_ENGINE_<TAG>` or `-` (ts: js's; bash on Windows: `git`, 7.6). STATUS: `ok`; `missing` (other columns `-`; also `cmd` off Windows); `old` (only too-old versions, 7.6; VERSION the newest); `broken` (probe failed or timed out, wrong target, or a configured command or venv missing).
3. Then `notes:` (skipped hits, such as Store placeholders; cl's and rustc's targets; a line for each engine that is not `ok`) and the config file used (`config file: <path>`, or `config file: none`). Exit 0: missing engines are information, not failure. For example:
   ```text
   notes:
     py: skipped C:\Users\me\AppData\Local\Microsoft\WindowsApps\python3.exe (Microsoft Store placeholder)
     js: not found; install: winget install -e --id OpenJS.NodeJS.LTS
     ts: not found; install: winget install -e --id OpenJS.NodeJS.LTS
   config file: none
   ```
   A `missing` engine's note is `not found; install: …` with 7.8's hint (run engines have none: `not found`; `cmd` off Windows: `cmd is Windows only`); a `broken` or `old` one gets the first line of its `EngineNotFound` message.

### 19.3 v2 check

Check parses and runs the static checks; it executes nothing (no engine, compiler, probe, `xcode-select`, `vswhere`, or library load, which would run initializers) and writes nothing (no probe cache, no `<run>`); it only reads files (20.1).
1. Load config (18.1, from FILE's folder; error: exit 2). Parse FILE and, transitively, every `import "…"` target once (paths are literals, so imports inside `if` count too); a cycle or a missing target gives the run-time `ImportError` text (6.4) at the `import`; a file that fails to parse is reported and its imports are skipped.
2. **Errors per file:** every `SyntaxError` (format 2.4) of chapters 3, 4 and 16.1, such as directive syntax, duplicate labels, block args on a compiled tag, `@import SEL as NAME` without a package selector, a definition block used as a value, and `extern` declarations.
3. Diagnostics go to stderr (2.4), then a summary to stdout: `check main.v2: 2 files, 0 errors`. Exit 1 on any error, else 0.

### 19.4 v2 cache clean

`v2 cache clean` deletes everything under `~/.v2/cache/` (`build/`, 15.6; `toolchains.json`, 7.7), entry by entry and best effort: what it cannot delete (on Windows, a library a running V2 has loaded) is skipped and listed. It is the one cache reset: a damaged cached library is rebuilt on its own (15.8), and `v2 doctor` probes afresh (19.2). Exit 0, or 2 if the home folder is unknown.

---

## 20. Security model

### 20.1 Trust boundary

V2 is a same-user tool. Running a `.v2` file runs all its code with the user's rights: its blocks, compiled blocks, `extern` libraries and the engine commands its config names. `v2 check` only parses (19.3). The mitigations in 20.2 keep two promises:

- **Data stays data.** V2 values never become code: block args and call arguments travel as data, and V2 never inserts values into foreign source (DOCS 4.1, 4.4).
- **No code the user didn't choose.** A downloaded project or another local user can't get code run through the working directory, `PATH`, DLL planting or the build cache.

Out of scope: sandboxing, permission prompts, signature checks, defending against the user's own other processes, and the config file itself. A `v2.toml` found by the upward search is trusted like the script (no ownership checks, 18.1), and `v2 doctor` names the config file it read (19.2).

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
- **POSIX.** Create it with `mkdir` and mode 0700. If the name exists, draw a new one; after 8 tries, raise IOError `cannot create a private temp folder in {base}`. Then `lstat` must show a real directory (not a symlink) owned by the euid, with mode 0700.
- **Windows.** Create it with `CreateDirectoryW` and a protected DACL that grants full control only to the current user's SID (from `GetTokenInformation(TokenUser)`): SDDL `D:P(A;OICI;FA;;;{sid})`.
- **Contents.** `run/{tag}-{n}.{ext}` (17.3); `adapters/` (14.1); `build/<k>/` only when `~/.v2` is unusable: builds then happen and libraries load there (15.6). Every file is created with `create_new`.
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
| Failed write of `print` to stdout or, with `stderr: true`, stderr (closed pipe) | IOError at the `print`; none at stdout's final flush | 5.10, 6.5 | a failed final flush turns exit 0 into 1 |
| A script path that isn't valid UTF-8 (Linux) | usage error, exit 2 | 2.1 | argv is read as text |
| The interpreter thread can't start (a process or thread limit) | `v2: cannot start the interpreter thread: <reason>`, exit 1 | 1.3 | not an internal error: nothing ran |
| Failed write of a worker's output to stdout (closed pipe) | IOError at the block or call that wrote it, once it returns | 9.9 | the worker keeps running |
| Out of memory in the host, other than `*` and `range` (a `ValueError`) | the process aborts, as any Rust program does | 5.3 | workers die with it (8.3); the run dir stays, as after a hard kill |
| Failed write of a diagnostic, summary, `--help` or `--version` (closed stdout or stderr) | none: the write is dropped | 6.5 | exit code unchanged; never 101 or an abort |
| Interrupt: Ctrl+C, Ctrl+Break, console close, SIGINT, SIGTERM, SIGHUP, SIGQUIT | none (not catchable) | 6.5, 8.3 | child trees killed, run dir removed, exit 130; no `finally` runs |
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
- **Isolation.** `HOME` and `USERPROFILE` point at a fresh temporary home (18.1 reads `~` from exactly these), so the `~/.v2` caches start empty; `RUSTUP_HOME` and `CARGO_HOME` keep their original values so `rustc` still works, and `V2_TEST_RUST_TOOLCHAIN`, when set, becomes `RUSTUP_TOOLCHAIN` (22.5). Inherited `V2_ENGINE_*` variables are removed, and `V2_ENGINE_FAKE` is set to the built `fake_adapter` for `tests/custom`.
- **Panic trigger.** Debug builds only (`cfg(debug_assertions)`, so never in a release binary): with `V2_TEST_PANIC` set, the interpreter thread panics at once, so a test can check 6.5's panic path (`v2: internal error: …`, exit 101).
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
| R40 | `engines/py/interrupt/` (run.rs, unix) | SIGINT, and separately SIGTERM, sent to the host during a sleeping `@py` block exits 130 within 2 s; the worker pid is gone and the run dir is removed |
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
| R65 | `engines/py/parking.v2` with `V2_TEST_PARK_MS`, `engines/process` unit test (Windows) | after a park, a counter, a handle and a stored callback all still work; the working set drops; a session with `call_timeout` is not parked |
| R66 | `host/scopes.v2` | a read and a write before a local `let` act on the outer name; `x += 1` before `let x` likewise; `for x in x` reads the outer `x`; closures keep their own iteration; a module's namespace order is unchanged; did-you-mean skips names not declared yet |
| R67 | `engines/resolve` unit test | a Store placeholder is probed once across two runs sharing a cache file; a `doctor` refresh probes it again; a real alias is never cached as a placeholder |

### 22.5 CI matrix

| Job | Python | Node | C / C++ | `@rust` | `V2_TEST_REQUIRE` |
| --- | --- | --- | --- | --- | --- |
| windows-latest, MSVC | 3.13 | 22 (≥ 22.13) | `cl` found via vswhere, outside a Developer Prompt | stable | py,js,ts,c,cpp,rust |
| ubuntu-latest | 3.13 | 22 (≥ 22.13) | gcc/g++ | stable | py,js,ts,c,cpp,rust |
| macos-latest | 3.13 | 22 (≥ 22.13) | Apple clang | stable | py,js,ts,c,cpp,rust |
| ubuntu-22.04, minimum versions (7.2) | 3.10 | 22.13 | gcc/g++ | 1.56 | py,js,ts,c,cpp,rust |

- **Build and test.** Every job builds V2 with stable Rust and runs `cargo test`. On Windows the host target is `x86_64-pc-windows-msvc`, which also proves that libffi builds with MSVC. `@rust` blocks compile with that column's toolchain: the minimum-versions job installs Rust 1.56 beside stable and sets `V2_TEST_RUST_TOOLCHAIN=1.56` (22.2).
- **Unusual paths.** A run.rs test runs a script from a folder whose path has a space and a non-ASCII character (`v2 tést`).
