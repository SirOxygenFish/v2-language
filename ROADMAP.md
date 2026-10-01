# V2 Roadmap

The build plan for the rebooted V2, and the one place that says **where we are**.
[DOCS.md](DOCS.md) says *what* V2 does, [INTERNALS.md](INTERNALS.md) says *how*, and this file
says *in what order* and *what's done*.

## Where we are

| | |
| --- | --- |
| **Current phase** | Phase 0: Host core |
| **Next task** | 0.5 Parser, AST and static checks |
| **Last update** | 2026-10-01: 0.4 block scanning and directives done (CI written, not yet run) |

## Rules

1. **A box is checked only when its tests pass.** No "it should work". Run it, see it pass, then
   check the box. (Old V2 claimed features its tests only *pretended* to test — never again.)
2. **Phases go in order.** Nothing from a later phase is stubbed in early: a builtin that doesn't
   exist yet is a `NameError`, an engine that isn't built yet is `EngineUnknown` (INTERNALS 23).
3. **The docs are the spec.** If code and docs disagree, decide which one is wrong and fix it on
   purpose, in the same change. Never let them drift.
4. **Every fixed bug gets a test** before its box goes back to checked.
5. **Keep this file current:** update "Where we are" and the progress log at the end of every
   work session.

Legend: `[ ]` not started · `[~]` in progress · `[x]` done (tests pass) · `Rnn` = regression
test from INTERNALS 22.4 · `§n.m` = INTERNALS section · "ch. 21 rows" = the failure-mode matrix
rows for that area, each of which needs a test.

---

## Phase 0: Host core

The small V2 language on its own, without any foreign engines. At the end, every example in
DOCS chapter 3 runs, and a block like `@py { }` fails cleanly with `EngineUnknown`.

### 0.1 Project skeleton
- [x] Cargo crate at the repo root (`Cargo.toml`, `src/`, `tests/`) with
      `[workspace] exclude = ["old"]` so the archived crate stays separate (§1). (`examples/`
      arrives in Phase 3 with its first file, `fake_adapter.rs`.)
- [x] Dependencies from §1.4: `serde_json` (preserve_order, float_roundtrip, unbounded_depth),
      `toml`, `indexmap`, `libloading`, `libffi`, `sha2`, `libc`, `windows-sys` — the vendored
      libffi builds under MSVC and a mixed int/float call passes (`ffi::call` smoke test)
- [x] Module tree from §1.2 (`cli/ config/ syntax/ runtime/ codec/ engines/ adapters/ build/
      ffi/ doctor/`), empty but compiling
- [x] Threads per §1.3: main thread, interpreter thread (256 MiB stack), watchdog; panic → 101
- [x] `v2 --version`, `v2 --help`, the full §19.1 argument grammar with usage errors → exit 2
- [x] Test harness `tests/run.rs` (§22.2): discovers `tests/**/*.v2`, headers (`exit`,
      `stderr`, `args`, `platform`, `timeout`), `.out` comparison, isolated `HOME`, tautology
      lint, visible summary; real `.v2` tests run through it since 0.2. (`requires:` and SKIP
      come in 1.1, when `engine_available` exists.)
- [~] CI on GitHub Actions: `cargo build` + `cargo test` on Windows, Ubuntu and macOS (§22.6).
      Written; runs once the branch is pushed. The engine columns of the matrix are added as
      Phases 1 and 2 land.
- **Done when:** `cargo build` and `cargo test` pass locally on Windows — **passed 2026-10-01**
  (14 tests, 0 warnings)

### 0.2 Walking skeleton: `print("hi")` end to end
- [x] `FileId`, `SourceMap`, `Span` (§2.1, §2.2); `UnitLoc` moved to 0.4, where block bodies need it
- [x] Diagnostics: `file:line:col: Kind: message`, excerpt, caret, stack frames, notes, hints,
      long-line windowing (§2.4, §2.5)
- [x] Just enough parser and interpreter for literals, names, f-strings, calls with named args
      and `print` via `Out` (§5.10), plus `v2 FILE` and exit codes 0/1/2 (§1.2, §19.1);
      NameError with did-you-mean (§5.6, §5.9), `TypeError: 'str' is not callable` (§5.7)
- [x] An uncaught error and a `SyntaxError` render per §2.4 and exit 1
- **Done when:** a harness test printing `hi` passes, and a harness test with a syntax error
  checks the exact stderr and exit code — **passed 2026-10-01** (`tests/host/hello.v2`,
  `tests/host/syntax/*.v2`)

### 0.3 Lexer
- [x] Tokens, keywords (DOCS 18), BOM strip, CRLF → LF (§3.1, §3.2)
- [x] Where statements end: open brackets, trailing operators, leading `.`/`?.` (§3.3)
- [x] Numbers: decimal with `_`, hex; i64 overflow and `0b`/`0o`/leading zeros are SyntaxErrors (§3.4)
- [x] Strings: strict escapes, raw `r""`, bytes `b""`, f-strings with `${expr}` and `:.Nf`,
      triple-quoted with the closing-line dedent rule (§3.5); `${…}` nesting limited to 256
- [x] Comments `//` and `/* */`
- **Done when:** lexer unit tests pass, including every DOCS 3.1 and 3.5 example — **passed
  2026-10-01** (reviewed; 6 findings fixed, each with a regression test)

### 0.4 Block scanning and directives
- [x] `UnitLoc` (§2.3)
- [x] Block heads `@tag [label] [(args)]` with `{` on the header's last line, and the fence
      form (§3.6, §3.7)
- [x] Scan profiles: python, c-like (c/cpp/rust/js/ts), plain for custom tags; guards C1/C2 (§3.8)
- [x] `@export` / `@import` directive extraction: exact tokens, multi-line lists, `@@` escapes,
      blank lines kept so line numbers hold; dedent with the mixed-indentation error (§4.4)
- [x] `extern "lib" { }` bodies captured with the c-like profile (§3.9)
- [x] Ahead of 0.5: blocks as statements and values in the parser, block args, the block rows of
      the §4.5 static checks; reaching a block raises `EngineUnknown` (no engines until Phase 1)
- **Done when:** the `syntax/scan` and `syntax/directives` unit tests (§22.3) and R07 pass —
  **passed 2026-10-01** (reviewed; 13 confirmed findings fixed, each with a regression test)

### 0.5 Parser, AST and static checks
- [ ] Recursive-descent parser for the DOCS 18 grammar, precedence table (§4.1, §4.2)
- [ ] AST incl. `EngineBlock`, `EngineImport`, `Extern`, selectors (§4.3)
- [ ] `?.` whole-chain short-circuit; assigning through `?.` is a SyntaxError (§4.1, §5.5)
- [ ] C declaration parser `ffi/cdecl` with its CType lowering, because the parser needs it:
      unreadable `extern` declarations and `@export` prototypes are SyntaxErrors (§16.3, §16.4)
- [ ] Static checks (§4.5): duplicate labels, redeclaration rules incl. params and loop variables,
      args on compiled blocks, `a..b` only in `for`, definition blocks not usable as
      expressions, whole-module `@import` only with package selectors
- **Done when:** R28, the parser unit tests and the `ffi/cdecl` unit tests (§22.3) pass

### 0.6 Values and operators
- [ ] `Value` enum: null, bool, int (i64), float, str, bytes, list, dict, func, handle, error (§5.1)
- [ ] Reference semantics for list/dict, shallow `.copy()`
- [ ] Equality, ordering, truthiness (§5.2)
- [ ] Arithmetic: `/` always float, floor `%`, `**`; overflow and division by zero → ValueError (§5.3)
- [ ] Strings indexed by character; indexing, slicing and members for str, list, bytes, dict (§5.4, §5.5)
- [ ] Method tables for str, list, dict, bytes, incl. `list.get` and the dict
      key-before-method rule (§5.11)
- **Done when:** the `tests/host` value, operator and method tests pass

### 0.7 Variables, functions, control flow, errors
- [ ] Scopes and frames, `let`/`const`, closures, fresh variable per loop iteration (§5.6)
- [ ] Functions: hoisting, defaults, named args, `...rest`, spread, ArityError, lambdas (§5.7)
- [ ] `if`/`elif`/`else`, `while`, `for` (lists, dicts, strings, bytes, `a..b`, `i, x` form),
      `break`/`continue`, snapshot iteration (§5.8)
- [ ] Errors: `throw`, `error()`, `try`/`catch`/`finally`, error fields, traces, rethrow keeps
      origin; recursion limit; NameError with did-you-mean (§5.9)
- **Done when:** R09, R45 and the `tests/host` control-flow and error tests pass

### 0.8 Modules
- [ ] `import "./x.v2" as m`: resolution relative to the file, run once, dict snapshot,
      `_private` names, cycle detection with the chain; import graph parsed at startup (§6.1, §6.4)
- [ ] Syntax errors in imported files surface as ImportError at the `import`
- **Done when:** R49 and the module tests pass

### 0.9 Builtins and JSON
- [ ] `codec/json` on serde_json with `$v2` tags (§10.1–10.3)
- [ ] All builtins of DOCS 3.11 that don't need engines: `print` (`sep`, `end`), `input` (null
      at EOF), `len`, `type`, conversions, `range`, `json_parse`/`json_stringify`,
      `read_file`/`write_file`, `getenv`, `args`, `exit`, `error` (§5.10)
- **Done when:** the builtin tests and the `codec/json` unit tests (§22.3) pass

### 0.10 Config, CLI and exit paths
- [ ] Config layers and precedence: defaults → `~/.v2/config.toml` → `v2.toml` → env → CLI;
      config errors exit 2 (§18.1–18.5)
- [ ] `v2 run`, host-only `v2 check FILE` (§19.1, §19.3)
- [ ] Shutdown paths (§6.5): normal end, uncaught error, `exit()` (no `finally`), interrupt
      → 130, internal panic → 101
- **Done when:** the host parts of R39 and R48 pass

**Phase 0 complete when:** all of `tests/host`, the unit tests, R07, R09, R28, R39 (host part),
R45, R48 (host part), R49 and the ch. 21 Syntax and Host rows pass (INTERNALS 23).

---

## Phase 1: Python, JavaScript, TypeScript

The polyglot core: worker processes, the VEP/1 protocol, `@export`/`@import`, packages, handles,
callbacks and errors that cross languages. Most regression tests need several of these pieces at
once, so they are collected in 1.10.

### 1.1 Engine registry and toolchain discovery
- [ ] `EngineSpec`, built-in table, aliases, runtime tag resolution with did-you-mean (§7.1–7.3)
- [ ] Finding executables: PATH walk without relative entries, Windows Store python stub,
      `py -3` (§7.5, §7.6)
- [ ] Version probes and `~/.v2/cache/toolchains.json` (§7.7); `EngineNotFound` messages (§7.8)
- [ ] `engine_available(tag)` (§7.3); harness `requires:` headers and SKIP (§22.2)
- [ ] `v2 doctor` table (§19.2)
- **Done when:** R41 and the `engines/resolve` and `engines/registry` unit tests pass; `v2 doctor`
  shows py/js/ts on this PC

### 1.2 Worker processes
- [ ] Private run directory: create, owner-only permissions, remove on every exit path (§20.3)
- [ ] Session state machine, lazy spawn, env (`PYTHONUTF8`, `NODE_PATH`), cwd (§8.1–8.3)
- [ ] Process trees: `setsid` on Unix, Job Objects on Windows, tree kill (§8.3)
- [ ] Handshake (§8.4), watchdog timers paused during reverse calls (§8.5)
- [ ] Crash detection → EngineCrashed; shutdown with `bye`, grace, kill (§8.6, §8.7)

### 1.3 The VEP/1 protocol and codec
- [ ] Framing: NDJSON, 64 MiB limit, pre-`hello` noise to stderr, one `\r` dropped (§9.1)
- [ ] All frame types, ids and the re-entrant wait loop with depth limit 64 (§9.2–9.7)
- [ ] When a worker may call V2 (§9.8); worker stdio rules (§9.9); `release`/`bye` (§9.10);
      `ProtocolError` (§9.11)
- [ ] Marshaling for Python and JS: numbers, `$v2` tags, strings, containers, named args (§10.2–10.8)
- **Done when (1.2 + 1.3):** the `engines/vep` and `engines/session` protocol tests against the
  scripted in-process adapter (§22.3) pass

### 1.4 Python engine
- [ ] `adapters/py.py`: launch, fd rewiring, `out` frames, dispatch loop (§14.1–14.3)
- [ ] Reaching a block, the `load` frame, Python load: module per unit, line offsets,
      last-expression value, block args (§12.1–12.3)
- [ ] Action vs definition blocks: load once, failed loads re-raise (§12.1)
- **Done when:** R05, R06 (py) and R34 pass

### 1.5 JavaScript and TypeScript engine
- [ ] `adapters/js.js`: sync stdin reads, wrapped stdout/console/fs/child_process (§14.4)
- [ ] JavaScript load with `vm`, completion value, no `import` statements (§12.4)
- [ ] TypeScript via `module.stripTypeScriptTypes` (Node 22.13+) or a transpiler (§12.5)
- [ ] Promises awaited only at depth 0; idle-callback errors (§9.8)
- **Done when:** R06 (js) and R32 pass

### 1.6 Imports, packages and cross-language calls
- [ ] Selectors, per-file export registry, resolving names, binding (§13.1–13.4)
- [ ] Package imports `py.x` / `js.x`, `*`, module handles, ESM fallback (§13.5)
- [ ] Routes, imports inside blocks, the `v2` selector (§13.6, §13.7)
- [ ] V2 callables inside workers (callbacks both ways) (§11.8)
- [ ] Named args → Python kwargs; ArityError for JS (§10.8)

### 1.7 Handles
- [ ] Worker table, host `HandleRef`, release batching (§11.1, §11.2, §9.10)
- [ ] Operations, iteration and `value(h)` (§11.3–11.5)
- [ ] Callable handles, identity, invalid uses, leaks (§11.6, §11.7, §11.9)

### 1.8 Errors and output across languages
- [ ] Foreign tracebacks mapped to `.v2` lines and columns (§2.3, §12.6)
- [ ] Output order with captured stdout (§1.3, §9.9)

### 1.9 Config, check and doctor for engines
- [ ] `[engines.py|js|ts]` (command, timeouts, transpiler), `[python] venv/paths`,
      `[node] modules/paths` (§18.2)
- [ ] Full `v2 check` inventory for engines (§19.3); `v2 doctor TAG` for py/js/ts (§19.2)
- [ ] Port `old/v2/testsuite/test_engines.v2` as `tests/engines/py/seed_engines.v2` (§22.5)

### 1.10 Phase 1 test sweep
- [ ] R01, R02, R03, R04, R08, R10, R11, R12, R13, R14, R15, R16, R17 (py/js), R18, R19, R26,
      R27, R29, R30, R31, R33, R38, R39 (`@py` line part), R40, R42, R43, R44, R46, R48 (engine
      part)
- [ ] ch. 21 rows: Setup, Blocks and imports, Calls and values

**Phase 1 complete when:** `tests/engines/{py,js,ts}`, `tests/cross`, `tests/imports`, the seed
and protocol tests, and every R listed in 1.4, 1.5 and 1.10 pass (INTERNALS 23).

---

## Phase 2: Compiled blocks and native libraries

C, C++ and Rust blocks compiled to cached libraries, and `extern` for existing libraries.

### 2.1 Process helper
- [ ] `engines/runonly`: spawn, stdout streaming or capture, stderr tail, tree kill,
      EngineExitError (§15.5, §17.3)

### 2.2 Native FFI (`extern`)
- [ ] The `extern` statement and library resolution per platform, `SetDllDirectoryW(L"")` on
      Windows, `[ffi] search_paths` (§16.1, §16.2, §18.2)
- [ ] Argument and return conversion with range checks, strings, opaque pointers (§16.5, §16.6)
- [ ] libffi call path and lazy native flush (§16.7)
- **Done when:** R23 and R25 pass

### 2.3 C blocks
- [ ] C/C++ toolchains: `cl` via vswhere + vcvars, MinGW gcc with static runtime, gcc/clang
      (§7.6, §15.7); `[engines.c|cpp] command/flags/call_timeout` (§18.2)
- [ ] Pipeline, exports and inner imports, signature extraction, generated sources and shims,
      `#line` mapping (§15.1–15.4)
- [ ] Program mode (no `@export`): `main` wrapping, run, capture (§15.5)
- [ ] Build cache in `~/.v2/cache/build`, `--no-cache`, `v2 cache clean` (§15.6, §19.4)
- [ ] Loading and binding: identical-block copies, ABI check, evict-and-rebuild (§15.8)
- **Done when:** R06 (C) and R20 pass

### 2.4 C++ blocks
- [ ] `std::string`, `extern "C"` shims with `try`/`catch` (§15.3, §15.4)
- **Done when:** R22 passes

### 2.5 Rust blocks
- [ ] Rust toolchain: `rustc -vV`, rustup sysroot pin, target check (§7.6, §15.7)
- [ ] Signature parser, `catch_unwind` shims, `String` returns (§15.3, §15.4)
- **Done when:** R21 passes

### 2.6 Calling other languages from compiled code
- [ ] Generated `v2.h` and Rust `v2` module (§15.4); `host_call` reverse calls (§15.9)
- **Done when:** the DOCS 8.3 example (Rust calling `py.statistics`) runs as a test

### 2.7 Phase 2 test sweep
- [ ] `v2 check` LIBRARIES/COMPILED sections and compiled-block checks (§19.3);
      `v2 doctor TAG` for c/cpp/rust (§19.2)
- [ ] R17 (native part), R24
- [ ] ch. 21 rows: Build and FFI

**Phase 2 complete when:** `tests/compiled`, `tests/ffi`, R06 (C), R17 (native part), R20–R25
pass (INTERNALS 23).

---

## Phase 3: Custom engines

How any other language gets in.

- [ ] Custom `[engines.<tag>]` tables: spec validation, config conflicts, layer merging, pending
      overrides (§17.1, §17.2, §18.3, §18.4)
- [ ] `run` engines on the 2.1 helper: `{file}`, `V2_ARG_*`, CRLF for `.bat`/`.cmd`, tree kill
      (§17.3, §17.4)
- [ ] `bridged` engines through the normal session code, capability checks (§17.5)
- [ ] `register_engine(tag, spec)` and its conflict rules (§17.2)
- [ ] `examples/fake_adapter.rs`; `v2 doctor TAG` for custom tags (§17.6, §17.7, §22.3)
- [ ] `v2 check`: run-engine directive check and `register_engine` literal tags (§19.3)
- [ ] Tests: R17 (run part), R35, R36, R37, R47; ch. 21 custom-engine rows

**Phase 3 complete when:** `tests/custom`, R17 (run part), R35–R37, R47 pass (INTERNALS 23).

---

## Release checklist (after Phase 3)

- [ ] Full CI matrix from §22.6 green, with `V2_TEST_REQUIRE` set so missing engines fail
      instead of skipping
- [ ] Every DOCS.md example runs as a test (cookbook included)
- [ ] Remove the "design specification, not implemented yet" banners from DOCS.md and INTERNALS.md
- [ ] New Windows installer (the old one is in `old/install.ps1`)
- [ ] Merge `v2-reboot` into `main`, tag `v0.1.0`

---

## Later (not designed yet)

These need a DOCS + INTERNALS design pass **before** any code:

- Built-in engines for shell (bash/sh/PowerShell/cmd), Go, Zig, Lua, Ruby, Java, C# (DOCS 5)
- FFI structs, callbacks, variadic functions, `#include` headers (DOCS 12)
- Cargo crates in `@rust` blocks; lists/dicts/callbacks as direct compiled-function
  parameters (DOCS 11.7)
- `v2 install` for Python/npm packages (DOCS 7)
- Operators on handles (DOCS 9.2)
- V2's own standard library — size still undecided ("we'll see")

---

## Progress log

| Date | What happened |
| --- | --- |
| 2026-09-30 | Evaluation of old V2; decision to reboot around the polyglot system |
| 2026-10-01 | Old project archived in `old/`; DOCS.md and INTERNALS.md written and cross-checked; branch `v2-reboot` created; this roadmap written |
| 2026-10-01 | 0.1: crate, dependencies (libffi verified on MSVC), module tree, threads, CLI parser, test harness, CI file; reviewed (6 findings fixed) |
| 2026-10-01 | 0.2 + 0.3: SourceMap, error rendering, full lexer, `print` end to end; `v2 hello.v2` runs; reviewed (6 findings fixed with regression tests) |
| 2026-10-01 | 0.4: embedded-block syntax (scan profiles for py/js/ts/c/cpp/rust, fences, C1/C2 guards, unterminated notes), `@export`/`@import` directives, dedent, `UnitLoc`, `extern` bodies; blocks parse and raise EngineUnknown when reached; reviewed (13 findings fixed) |
