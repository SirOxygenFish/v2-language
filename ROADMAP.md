# V2 Roadmap

The build plan for the rebooted V2, and the one place that says **where we are**.
[DOCS.md](DOCS.md) says *what* V2 does, [INTERNALS.md](INTERNALS.md) says *how*, and this file
says *in what order* and *what's done*.

## Where we are

| | |
| --- | --- |
| **Current phase** | Phase 1: Python, JavaScript, TypeScript |
| **Next task** | 1.1 Engine registry and toolchain discovery |
| **Last update** | 2026-10-02: 1.0 child processes and the watchdog done (CI written, not yet run) |

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
- [x] Recursive-descent parser for the DOCS 18 grammar, precedence table (§4.1, §4.2); nesting
      (256) and expression-height (10,000) limits, so generated input never overflows the stack
- [x] AST incl. `EngineBlock`, `EngineImport`, `Extern`, selectors (§4.3)
- [x] `?.` whole-chain short-circuit in the AST (`OptChain`); assigning through `?.` is a
      SyntaxError (§4.1, §5.5). Evaluating the chain is 0.6.
- [x] C declaration parser `ffi/cdecl` with its CType lowering, because the parser needs it:
      unreadable `extern` declarations and `@export` prototypes are SyntaxErrors with the 16.3
      messages; `@cpp` prototype strings use the C++ dialect (§16.3, §16.4)
- [x] Static checks (§4.5): duplicate labels, redeclaration rules incl. params and loop variables,
      args on compiled blocks, `a..b` only in `for`, definition blocks not usable as
      expressions, whole-module `@import` only with package selectors; all reported in source order
- **Done when:** R28, the parser unit tests and the `ffi/cdecl` unit tests (§22.3) pass —
  **passed 2026-10-01** (reviewed twice: 14 confirmed findings, then 11 in the fixes; all fixed
  with regression tests)

### 0.6 Values and operators
- [x] `Value` enum: null, bool, int (i64), float, str, bytes, list, dict, func, error (§5.1);
      `handle` arrives with the engines (Phase 1). Deep values free iteratively.
- [x] Reference semantics for list/dict, shallow `.copy()`
- [x] Equality, ordering, truthiness (§5.2)
- [x] Arithmetic: `/` always float, floor `%`, `**`; overflow and division by zero → ValueError (§5.3)
- [x] Strings indexed by character (with the §5.4 char-index cache); indexing, slicing and members
      for str, list, bytes, dict; optional chains (§5.4, §5.5)
- [x] Method tables for str, list, dict, bytes, incl. `list.get` and the dict
      key-before-method rule (§5.11)
- **Done when:** the `tests/host` value, operator and method tests pass — **passed 2026-10-01**
  (`tests/host/{values,ops,strings,lists}`, R45 `optchain.v2`; reviewed together with 0.7)

### 0.7 Variables, functions, control flow, errors
- [x] Scopes and frames, `let`/`const`, closures, fresh variable per loop iteration, closure-cycle
      trimming, read-only builtins frame (§5.6)
- [x] Functions: hoisting, defaults, named args, `...rest`, spread, ArityError, lambdas (§5.7)
- [x] `if`/`elif`/`else`, `while`, `for` (lists, dicts, strings, bytes, `a..b`, `i, x` form),
      `break`/`continue`, snapshot iteration (§5.8)
- [x] Errors: `throw`, `error()`, `try`/`catch`/`finally`, error fields, traces, rethrow keeps
      origin; call-depth limit and stack guard; NameError with did-you-mean (§5.9)
- **Done when:** R09, R45 and the `tests/host` control-flow and error tests pass — **passed
  2026-10-01** (`try_locals.v2`, `optchain.v2`, `tests/host/{control,functions,errors}`; reviewed:
  6 distinct findings fixed with regression tests)

### 0.8 Modules
- [x] `import "./x.v2" as m`: resolution relative to the file, run once, dict snapshot,
      `_private` names, cycle detection with the chain; import graph parsed at startup (§6.1, §6.4)
- [x] Syntax errors in imported files surface as ImportError at the `import`
- **Done when:** R49 and the module tests pass — **passed 2026-10-01** (`tests/host/modules`;
  reviewed together with 0.9)

### 0.9 Builtins and JSON
- [x] `codec/json` on serde_json, plain mode: the 10.1 pre-scan (nesting limit 1,000, integers
      outside i64), number kinds, pretty printing, path-naming errors (§10.1, §10.2). The VEP
      mode's `$v2` tags and base64 (§10.3) come with the protocol in 1.3
- [x] All builtins of DOCS 3.11 that don't need engines: `print` (`sep`, `end`), `input` (null
      at EOF), `len`, `type`, conversions, `range`, `json_parse`/`json_stringify`,
      `read_file`/`write_file`, `getenv`, `args`, `exit`, `error` (§5.10)
- **Done when:** the builtin tests and the `codec/json` unit tests (§22.3) pass — **passed
  2026-10-02** (`tests/host/builtins`, `tests/host/exit_codes`, codec unit tests; reviewed
  together with 0.8)

### 0.10 Config, CLI and exit paths
- [x] Config layers and precedence: defaults → `~/.v2/config.toml` → `v2.toml` → env → CLI;
      validation on the merged result; config errors exit 2 (§18.1–18.5)
- [x] `v2 run`, host-only `v2 check FILE` (§19.1, §19.3)
- [x] Shutdown paths (§6.5): normal end, uncaught error, `exit()` (no `finally`), interrupt
      → 130 (also when the run ends before the watchdog wakes), internal panic → 101; a closed
      stdout or stderr never turns an exit path into a panic or an abort
- **Done when:** the host parts of R39 and R48 pass — **passed 2026-10-02** (also on Linux
  in WSL; three review rounds, 28 findings fixed with regression tests)

**Phase 0 complete when:** all of `tests/host`, the unit tests, R07, R09, R28, R39 (host part),
R45, R48 (host part), R49 and the ch. 21 Syntax and Host rows pass (INTERNALS 23) — **done
2026-10-02**: every listed test and ch. 21 row has a test (audited row by row), every DOCS
chapter 3 example was run against the binary, and the suite passes on Windows and Linux
(macOS untested until CI runs).

**Phase 0 complete when:** all of `tests/host`, the unit tests, R07, R09, R28, R39 (host part),
R45, R48 (host part), R49 and the ch. 21 Syntax and Host rows pass (INTERNALS 23).

---

## Phase 1: Python, JavaScript, TypeScript

The polyglot core: worker processes, the VEP/1 protocol, `@export`/`@import`, packages, handles,
callbacks and errors that cross languages. The registry holds py, js and ts; c, cpp and rust
join in Phase 2 with their toolchains, so until then `@c` is `EngineUnknown` (INTERNALS 23).
Order note (2026-10-02): 1.0 was added because probes, workers and transpilers all need the
child-process rules first; the wire (1.2) comes before sessions (1.3) because the handshake
needs frames; handles come before imports because R04/R15 need only action blocks.

### 1.0 Child processes and the watchdog
- [x] `engines/process`: child rules (own session on Unix with `PR_SET_PDEATHSIG` on Linux;
      `CREATE_SUSPENDED` + process group + kill-on-close Job Object on Windows), `kill_tree`,
      exit observed without reaping, reaping, `run_captured` with concurrent pipes (§8.3)
- [x] The watchdog moved here from `session`: timer stack with pause/resume, timers that kill
      a child's tree, every running child killed on the interrupt path (§1.3, §6.5, §8.5)
- **Done when:** the `engines/process` unit tests (§22.3) pass on Windows and Linux and the
  exit-path tests still pass — **passed 2026-10-02**

### 1.1 Engine registry and toolchain discovery
- [ ] `EngineSpec`, the py/js/ts table, aliases, runtime tag resolution with did-you-mean
      (§7.1–7.3); blocks resolve their tag through the registry (EngineUnknown /
      EngineNotFound, exit 3 when uncaught: R48's run half)
- [ ] Finding executables: PATH walk without relative entries, `PATHEXT`, Windows Store python
      stub, `py -3`; configured `command`, `[python] venv`, `[engines.ts] transpiler`, `V2_ENGINE_*`
      and `--engine` (§7.5, §7.6, §18.2)
- [ ] Version probes and `~/.v2/cache/toolchains.json`, also `--no-cache` for probes (§7.7, §19.4);
      `EngineNotFound` messages (§7.8)
- [ ] `engine_available(tag)` (§7.3); harness `requires:` headers and SKIP (§22.2)
- [ ] `v2 doctor` table (§19.2)
- **Done when:** R41 and the `engines/spec`, `engines/registry` and `engines/resolve` unit tests
  pass; `v2 doctor` shows py/js/ts on this PC

### 1.2 The VEP/1 wire
- [ ] Framing: NDJSON, 64 MiB limit both ways, pre-`hello` noise, one `\r` dropped (§9.1)
- [ ] Frame types and their checks, `ProtocolError` for every fatal case (§9.2–9.6, §9.11)
- [ ] Codec VEP mode: numbers, `$v2` tags, base64, depth and cycle checks (§10.1–10.5)
- [ ] Host `HandleRef`, release queue and the callable table (§11.2, §11.8)

### 1.3 Sessions
- [ ] Session state machine, lazy spawn, env (`PYTHONUTF8`, `NODE_PATH`), cwd (§8.1–8.3)
- [ ] Handshake (§8.4); the re-entrant request loop with depth limit 64 (§9.7); when a worker may
      call V2, host side (§9.8); `release`/`bye` (§9.10)
- [ ] Call and startup timers, paused during reverse calls (§8.5)
- [ ] Crash detection → EngineCrashed; shutdown with `bye`, grace, kill (§8.6, §8.7)
- [ ] Private run directory: create, owner-only permissions, remove on every exit path (§20.3)
- **Done when (1.2 + 1.3):** the `engines/vep` and `engines/session` protocol tests against the
  scripted adapter, in process and as a real child (§22.3), pass on Windows and Linux

### 1.4 Python engine
- [ ] `adapters/py.py`: launch, fd rewiring, `out` frames, dispatch loop (§14.1–14.3)
- [ ] Reaching a block, the `load` frame, Python load: module per unit, line offsets,
      last-expression value, block args (§12.1–12.3)
- [ ] Action vs definition blocks: load once, failed loads re-raise (§12.1)
- [ ] Python marshaling and the worker handle table (§10.6, §11.1); worker stdio rules (§9.9);
      `[python] paths` in `init`
- **Done when:** R05, R06 (py), R34 and R40 pass; the run dir is removed on every exit path

### 1.5 JavaScript and TypeScript engine
- [ ] `adapters/js.js`: sync stdin reads, wrapped stdout/console/fs/child_process (§14.4)
- [ ] JavaScript load with `vm`, completion value, no `import` statements (§12.4)
- [ ] TypeScript via `module.stripTypeScriptTypes` (Node 22.13+) or a transpiler (§12.5)
- [ ] JS marshaling (§10.7); promises awaited only at depth 0; idle-callback errors (§9.8)
- **Done when:** R06 (js) and R32 pass

### 1.6 Handles
- [ ] Operations, iteration and `value(h)` (§11.3–11.5)
- [ ] Callable handles, identity, invalid uses, leaks (§11.6, §11.7, §11.9)
- **Done when:** R04 and R15 pass

### 1.7 Imports, packages and cross-language calls
- [ ] Selectors, per-file export registry, resolving names, binding (§13.1–13.4)
- [ ] Package imports `py.x` / `js.x`, `*`, module handles, ESM fallback (§13.5)
- [ ] Routes, imports inside blocks, the `v2` selector (§13.6, §13.7)
- [ ] V2 callables inside workers (callbacks both ways) (§11.8)
- [ ] Named args → Python kwargs; ArityError for JS (§10.8)
- **Done when:** R08, R10, R12, R13, R14 (first two parts), R18, R19, R26, R27, R38, R43, R44
  and R46 pass

### 1.8 Errors and output across languages
- [ ] Foreign tracebacks mapped to `.v2` lines and columns (§2.3, §12.6)
- [ ] Output order with captured stdout (§1.3, §9.9)
- **Done when:** R16, R39 (`@py` line part), R14 (V2 error part) and R17 (py/js) pass

### 1.9 Check and doctor for engines
- [ ] Full `v2 check` inventory for engines (§19.3); `v2 doctor TAG` for py/js/ts (§19.2)
- [ ] Port `old/v2/testsuite/test_engines.v2` as `tests/engines/py/seed_engines.v2` (§22.5)
- **Done when:** the seed passes, plus R48's check half and the `doctor TAG` tests

### 1.10 Phase 1 test sweep
- [ ] R01, R02, R03, R11, R29, R30, R31, R33, R42, and every R of 1.4–1.9 again
- [ ] ch. 21 rows for bridged engines: Setup, Blocks and imports, Calls and values

**Phase 1 complete when:** `tests/engines/{py,js,ts}`, `tests/cross`, `tests/imports`, the seed
and protocol tests, and every R listed in 1.1–1.10 pass (INTERNALS 23).

---

## Phase 2: Compiled blocks and native libraries

C, C++ and Rust blocks compiled to cached libraries, and `extern` for existing libraries.

### 2.1 Process helper
- [ ] `engines/runonly` on `engines/process`: temp file, stdout inherited or captured, 4 KiB
      stderr tail, EngineExitError (§15.5, §17.3)

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
| 2026-10-01 | 0.5: full parser and AST, static checks (all violations, in source order), `ffi/cdecl` with the 16.3 messages and the C++ dialect for `@cpp` prototype strings, expression-height limit; reviewed, fixes re-reviewed (25 findings fixed with regression tests) |
| 2026-10-01 | 0.6 + 0.7: the interpreter core: values, operators, string/list/dict/bytes methods, frames and closures, the call binder, control flow, try/catch/finally, error values with traces, the stack guard, iterative freeing of deep values; 0.5's third review round fixed (C++ prototype references, K&R, nested f-string cost); reviewed (6 findings fixed) |
| 2026-10-01 | 0.8: `import` with the startup parse of the import graph, run-once modules, namespace snapshots, circular-import chains, ImportError for syntax errors in modules (R49) |
| 2026-10-02 | 0.9: the JSON codec (plain mode) and every engine-free builtin; reviewed together with 0.8 (about 9 findings fixed) |
| 2026-10-02 | 0.10: config layers with validation on the merged result, `v2 run`, host-only `v2 check`, exit paths (closed streams, late interrupts, ignored signals, panic → 101); three review rounds, 28 findings fixed with regression tests (each new test shown to fail without its fix); the Phase 0 exit audit (ch. 21 rows, R-tests, DOCS ch. 3 examples) closed 8 test gaps and found `range(end: n)` and float `-6.0 % 3` mismatches, both fixed. **Phase 0 complete.** Linux verified in WSL |
| 2026-10-02 | 1.0: `engines/process` (child rules, Job Objects / sessions, tree kill, exit before reaping, captured runs) and the watchdog's timer stack; Phase 1 re-ordered (1.0 added, wire before sessions, handles before imports); Node 24.11.1 installed in WSL for Linux JS tests |
