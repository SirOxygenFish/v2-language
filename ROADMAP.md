# V2 Roadmap

The build plan for the rebooted V2, and the one place that says **where we are**.
[DOCS.md](DOCS.md) says *what* V2 does, [INTERNALS.md](INTERNALS.md) says *how*, and this file
says *in what order* and *what's done*.

## Where we are

| | |
| --- | --- |
| **Current phase** | Pre-publish (approved by the owner 2026-11-14), before Phase 2 |
| **Next task** | The pre-publish boxes below; merging, the `v0.1.0-alpha.2` tag and publishing the draft release wait for the owner's OK |
| **Last update** | 2026-11-14: pre-publish newcomer round 1 found and fixed (progress log) |

## Rules

1. **A box is checked only when its tests pass.** No "it should work". Run it, see it pass, then
   check the box.
2. **Phases go in order.** A phase is done when its tests pass on Windows and Linux, and on the
   whole CI matrix (§22.5) once CI runs.
   Nothing from a later phase is stubbed in early: a builtin that doesn't exist yet is a
   `NameError`, an engine that isn't registered yet is `EngineUnknown`. The registry holds py, js
   and ts in Phase 1 (so `@c` is `EngineUnknown` there); c, cpp and rust join in Phase 2 with
   their toolchains, and custom engines and the built-in run engines in Phase 3. EngineUnknown's
   list of known engines names only the registered ones (a tag of a later phase says it isn't in
   this version yet, §7.3), and its `declare it in [engines.<tag>] in v2.toml, or with
   define_engine` hint comes with custom engines. The only placeholders: a
   block of a registered engine, `@import` and `extern` raise `Error: … isn't in this version of
   V2 yet` until their milestone, and `v2 cache clean` prints "not implemented yet" until 2.3.
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
DOCS chapter 3 (as it was then) runs, and a block like `@py { }` fails cleanly (`EngineUnknown`
until 1.1 registered py, the placeholder of rule 2 since).

- [x] **0.1 Project skeleton:** the crate (`[workspace] exclude = ["old"]`), the §1.4
      dependencies (the vendored libffi builds under MSVC), the §1.2 module tree, the §1.3
      threads, the §19.1 argument grammar, the `tests/run.rs` harness (§22.2)
  - [~] CI on GitHub Actions (§22.5): written, runs once the branch is pushed; Phase 1's
        Python and Node columns, `V2_TEST_REQUIRE` and the minimum-versions job are in, the
        C/C++ and Rust columns come with Phase 2
- [x] **0.2 Walking skeleton:** source map, spans and diagnostics (§2.1, §2.2, §2.4, §2.5);
      `print("hi")` end to end
- [x] **0.3 Lexer:** tokens, numbers, strings, bytes, f-strings, comments (§3.1–3.5)
- [x] **0.4 Block scanning and directives:** `UnitLoc`, block heads, fences, scan profiles,
      `@export`/`@import` extraction and dedent, `extern` bodies (§2.3, §3.6–3.9, §4.4)
- [x] **0.5 Parser, AST and static checks** (§4.1–4.5), with `ffi/cdecl`, which the parser
      needs for `extern` (§16.3, §16.4)
- [x] **0.6 Values and operators:** values, equality, arithmetic, indexing, optional chains,
      the methods of str, list, dict and bytes (§5.1–5.5, §5.11; 0.11 adds `split()`,
      `push(...items)` and `sort(key, reverse)`)
- [x] **0.7 Variables, functions, control flow, errors** (§5.6–5.9)
- [x] **0.8 Modules:** `import "./x.v2" as m` and the import graph (§6.1, §6.4)
- [x] **0.9 Builtins and JSON:** every engine-free builtin DOCS 3.11 had then (0.11 adds the rest), `codec/json` plain mode
      (§5.10, §10.1, §10.2)
- [x] **0.10 Config, CLI and exit paths:** `v2.toml` and `V2_ENGINE_<TAG>` (ch. 18),
      `v2 check FILE` (§19.3), the shutdown paths and exit codes (§6.5, §19.1)

**Phase 0 complete when:** all of `tests/host`, the unit tests, R07, R09, R28, R39 (host part),
R45, R48 (host part), R49, the ch. 21 Host rows and Setup's config-file row pass — **done
2026-10-02**: every listed test and ch. 21 row has a test, every DOCS chapter 3 example was run
against the binary, and the suite passes on Windows and Linux (macOS untested until CI runs).

- [x] **0.11 Small standard library** (added 2026-10-24, after Phase 0 closed; built first in
      Phase 1): `exec`, `script_dir`, `exists`, `list_dir`, `make_dir`, `remove_file`,
      `write_file(…, append:)`, `sleep`, `time`, `min`, `max`, `sum`, `abs`, `round`, `div`,
      `assert` (the `AssertionError` kind), `print(…, stderr:)`; `sort(key, reverse)`, `split()`
      on whitespace, `push(...items)` (DOCS 3.5, 3.6, 3.11; §5.9–5.11). `exec` finds `argv[0]`
      like an engine command (§7.5) and runs on `engines/process` (§8.3)
  - **Done when:** every new builtin and method has `tests/host` cases, its errors included, on
    Windows and Linux; the ch. 21 Host row for a failed `print(stderr: true)` has a test; the
    DOCS chapter 3 examples run against the binary — **passed 2026-10-24** (Windows and Linux)

---

## Phase 1: Python, JavaScript, TypeScript

The polyglot core: worker processes, the VEP/1 protocol, `@export`/`@import`, packages, handles,
callbacks and errors that cross languages.

### 1.0 Child processes and the watchdog
- [x] `engines/process`: child rules (own session on Unix with `PR_SET_PDEATHSIG` on Linux;
      `CREATE_SUSPENDED` + process group + kill-on-close Job Object on Windows), `kill_tree`,
      exit observed without reaping, reaping, `run_captured` with concurrent pipes (§8.3)
- [x] The watchdog: timer stack with pause/resume, timers that kill a child's tree, every
      running child killed on the interrupt path (§1.3, §6.5, §8.5)
- **Done when:** the `engines/process` unit tests (§22.3) pass on Windows and Linux and the
  exit-path tests still pass — **passed 2026-10-02**

### 1.1 Engine registry and toolchain discovery
- [x] `EngineSpec`, the py/js/ts table, aliases, runtime tag resolution with did-you-mean
      (§7.1–7.3); blocks resolve their tag through the registry (EngineUnknown /
      EngineNotFound, exit 3 when uncaught: R48's EngineNotFound part)
- [x] Finding executables: PATH walk without relative entries, `PATHEXT`, Windows Store python
      stub, `py -3`; configured `command`, `[python] venv` and `V2_ENGINE_<TAG>` (§7.5, §7.6,
      §18.2, §18.4)
- [x] Version probes and the probe cache `~/.v2/cache/toolchains.json` (§7.7); `EngineNotFound`
      messages (§7.8)
- [x] `engine_available(tag)` (§7.3); harness `requires:` headers and SKIP (§22.2)
- [x] `v2 doctor` table (§19.2)
- **Done when:** R41 and the `engines/spec`, `engines/registry` and `engines/resolve` unit tests
  (§22.3) pass, except their c/cpp/rust rows and `cl` cases (2.3, 2.5) and their custom-engine
  table, run-engine rows and run-engine lookup cases (Phase 3); `v2 doctor`
  shows py/js/ts on this PC — **passed 2026-10-02** (Windows and Linux; doctor
  here: py 3.11.6, js and ts on Node 24.11.1 with built-in type stripping, the WindowsApps
  `python3.exe` skipped as the Microsoft Store placeholder)

### 1.2 The VEP/1 wire
- [x] Framing: NDJSON, 64 MiB limit both ways, pre-`hello` noise, one `\r` dropped (§9.1, §9.3)
- [x] Frame types and their checks, `ProtocolError` for every fatal case (§9.2–9.6, §9.11)
- [x] Codec VEP mode: numbers, `$v2` tags, base64, depth and cycle checks (§10.1–10.5)
- [x] Host `HandleRef`, release queue and the callable table (§11.2, §11.8)

### 1.3 Sessions
- [x] Session state machine, spawn, env (`PYTHONUTF8`), cwd (§8.1–8.3); starting a session at
      first use comes with blocks (1.4)
- [x] Handshake (§8.4); the re-entrant request loop with depth limit 64 (§9.7); when a worker may
      call V2, host side (§9.8); `release`/`bye` (§9.10)
- [x] Call and startup timers, paused during reverse calls (§8.5)
- [x] Crash detection → EngineCrashed; shutdown with `bye`, grace, kill (§8.6, §8.7)
- [x] Private run directory: create, owner-only permissions, removed by main and by the
      interrupt path (§20.3); 1.4 tests every exit path once adapters fill it
- **Done when (1.2 + 1.3):** the `engines/vep` and `engines/session` protocol tests against the
  scripted adapter, in process and as a real child (§22.3), pass on Windows and Linux —
  **passed 2026-10-02**

### 1.4 Python engine
- [x] `adapters/py.py`: launch, fd rewiring, `out` frames, dispatch loop (§14.1–14.3)
- [x] Reaching a block (its session starts at first use), the `load` frame, Python load:
      module per unit, line offsets, last-expression value (`want_value`), block args (§6.2,
      §6.3, §12.1–12.3)
- [x] Action vs definition blocks: load once, failed loads re-raise (§12.1)
- [x] Python marshaling and the worker handle table (§10.6, §11.1); worker stdio rules (§9.9);
      `init` (§9.3); coroutines awaited on the session's loop (§9.8)
- [x] V2 functions passed into Python (block args) as stubs, and `V2Error` with `type`,
      `message`, `data`, `lang` and `foreign_type` when such a call fails (§11.8, §14.2); errors
      from Python with chained causes as `e.cause` (§9.6)
- **Done when:** R05, R06 (py), R34 and R40 pass; the run dir is removed on every exit path —
  **passed 2026-10-25** (Windows and Linux)

### 1.5 JavaScript and TypeScript engine
- [x] `adapters/js.js`: sync stdin reads, wrapped stdout/console/fs/child_process (§14.4)
- [x] JavaScript load with `vm`, completion value, no `import` statements (§12.4)
- [x] TypeScript via Node's built-in `module.stripTypeScriptTypes` (one Node rule for js and ts:
      22.13+, not 23.0 or 23.1; §7.2, §12.5)
- [x] JS marshaling (§10.7); Promises awaited only for an outermost request (stack depth 1, §9.8)
- [x] V2 functions passed into JS as stubs and `V2Error`; `Error.cause` as `e.cause` (§9.6,
      §11.8, §14.2)
- **Done when:** R06 (js) and R32 pass — **passed 2026-10-25** (Windows and Linux)

### 1.6 Handles
- [x] Operations (old-style JS constructors called with `new`), iteration (mapping handles like
      dicts) and `to_v2(h)` with its generous conversion (§11.3–11.5)
- [x] Callable handles, identity, invalid uses (§11.6, §11.7)
- [x] Named args → Python kwargs; ArityError for JS (§10.8)
- **Done when:** R04, R15 and R53 pass — **passed 2026-10-25** (Windows and Linux)

### 1.7 Imports, packages and cross-language calls
- [x] Selectors (`@js` and `@ts` match each other), per-file export registry, resolving names
      (ambiguity from declared exports, whatever has loaded), binding (§13.1–13.4)
- [x] Package imports `py.x` / `js.x`, `*`, module handles; ES modules through `require` (§13.5)
- [x] Routes; imports inside blocks (same-engine stubs looked up at call time, constants
      refused); the `v2` selector (§13.6, §13.7)
- [x] Callable handles passed to another engine as `fn` tags, the MarshalError for other
      handles (§11.6–11.8); leaks (§11.9); idle-callback errors (§9.8)
- **Done when:** R08, R10, R12, R13, R14 (first two parts), R18, R19, R26, R27, R38, R43, R44,
  R46, R52, R54 and R55 pass, plus `tests/cross` cases for a callable handle crossing engines and
  a non-callable one refused — **passed 2026-10-25** (Windows and Linux)

### 1.8 Errors and output across languages
- [x] Foreign tracebacks mapped to `.v2` lines and columns (§2.3, §12.6)
- [x] Output order with captured stdout (§9.9)
- **Done when:** R16, R39 (`@py` line part), R14 (V2 error part) and R17 (py/js) pass — **passed 2026-10-25** (Windows and Linux)

### 1.9 Seed test
- [x] Port lines 1–111 of `old/v2/testsuite/test_engines.v2` as
      `tests/engines/py/seed_engines.v2`: `var` → `let`, the probe `try`/`catch` → `// requires: py`,
      `caught.contains(x)` → `x in caught`, `str(e)` → `e.message`; the missing-export check
      (`@import { nonexistent } from @py`) asserts `e.type == "ImportError"` and §13.3 step 6's
      message. No other old file is ported
- **Done when:** the seed passes — **passed 2026-10-25** (Windows and Linux)

### 1.10 Phase 1 test sweep
- [x] R01, R02, R03, R11, R29, R30, R31, R33, R42, and every R of 1.4–1.9 again
- [x] ch. 21 rows for bridged engines: Setup, Blocks and imports, Calls and values

**Phase 1 complete when:** `tests/engines/{py,js,ts}`, `tests/cross`, `tests/imports`, the seed
and protocol tests, and every R listed in 1.1–1.10 pass. — **complete 2026-10-25** on Windows and Linux (CI has not run
yet: nothing is pushed; macOS untested).

The owner reopened Phase 1 on 2026-10-30, before Phase 2, for how languages live in memory and how
fast V2 runs.

### 1.11 Language lifetime
- [x] `Callable::Waiting`: a reference that exists before its process does (§5.1, §12.7)
- [x] The waiting line: `UnitState::Waiting`, one line in program order, first need runs it,
      failures, `xdepth` around each place (§12.7)
- [x] Which `@import`s are eligible: the per-file use walk (§13.4, §6.2)
- [x] `@import { * }` removed everywhere, `Slot.star` and the adapters' `"*"` paths with it (§4.5,
      §5.6, §13.5)
- **Done when:** R60–R64 and every existing test pass, and DOCS 5's example behaves as written —
  **passed 2026-10-30** (Windows and Linux)

### 1.12 Parking and startup
- [x] Parking: idle tracking and `K32EmptyWorkingSet` on the Job's processes (§8.8)
- [x] `NtResumeProcess` instead of the Toolhelp snapshot (§8.3)
- [x] The Store placeholder remembered in `toolchains.json` (§7.7)
- [x] Node's first context: lazy globals forwarded, not copied (§12.4)
- **Done when:** R65 and R67 pass, and first-block times are recorded against 190 ms (Python)
  and 93 ms (Node) — **passed 2026-10-30**: 79 ms and 42 ms (Windows and Linux; parking is Windows
  only)

### 1.13 Variable slots
- [x] `syntax/resolve`: layouts and name positions, frame elision (§5.6)
- [x] `Frame` by position, by-name fallback for reads and writes (§5.6)
- [x] `eval` and `exec` slimmed: cold arms split, int fast path, direct argument binding
- **Done when:** R66 and every existing test pass unchanged, and `fib(32)` and the 10M loop are
  recorded against 3.5 s and 3.2 s — **passed 2026-10-30**: 1.23 s and 1.15 s on Windows, 1.11 s and
  1.13 s on Linux. About 3x faster; the estimate of 5-10x was not reached

### 1.14 The `.v2` test-and-fix loop
- [x] `tests/sweep/`, one folder per DOCS area, expected output taken from DOCS (§22.1)
- [x] Every mismatch fixed at its root, with a regression test (rules 3 and 4)
- [~] Reviewers' hostile programs against a snapshot binary, until two rounds in a row find nothing:
      18 rounds and a phase audit ran, and every one found something; the owner closed 1.14 after
      round 18
- **Done when:** both rounds are clean on Windows and Linux. Compiled blocks, `extern`, run engines
  and custom engines aren't built yet, so they aren't covered — **closed by the owner 2026-11-10**
  (Windows and Linux green; the two clean rounds were not reached)

---

## Release gate (before Phase 2)

The owner's decision of 2026-11-10: Phases 0-1 go public as a pre-release before Phase 2 starts,
so CI, the minimum versions and macOS are proven on what exists.

- [x] `s += piece` appends in place when V2 holds the string alone, and `dict.remove` doesn't shift
      every later entry: both stay as the docs say, only no longer quadratic (DOCS 3.3, 3.6). Release:
      100,000 appends 9.8 s to 0.02 s, removing 100,000 keys 14.2 s to 0.13 s
- [x] Every runnable DOCS example of Phases 0-1 runs as a test, with fixtures: 25 of DOCS.md's 41
      and README's run (run.rs `docs_examples_run_as_documented`, tests/docs/), `skip.txt` says why
      the other 17 don't
- [x] CI green on Windows and Linux, the minimum versions (Python 3.10, Node 22.13) included, with
      `V2_TEST_REQUIRE=py,js,ts` (run #3, 2026-11-11); macOS on the owner's Mac Mini (macOS 26.5, Apple
      Silicon, Python 3.14, Node 22.23): all green
- [x] A short `README.md`; DOCS and INTERNALS say plainly which chapters aren't built yet
- [x] VEP/1 reserves room for zero-copy data: `init` carries the host's caps, and the `buffer` cap
      and `$v2` tag names are reserved, nothing more (§9.3, §10.3)
- [x] On the owner's explicit OK (2026-11-12): `old/` removed, `v2-reboot` merged into `main`, tag
      `v0.1.0-alpha.1` (`v0.1.0` stays for the release after Phase 3), CI green on `main`
- After the gate: `llms.txt`, a condensed spec of a few hundred lines for coding agents, its
  examples run as tests like DOCS's

---

## Pre-publish (before Phase 2)

The owner's goal of 2026-11-14: a friend who opens the repository can read it and succeed without
asking anyone. No bug in a newcomer's first 15 minutes, and docs that answer every question before
it's asked. Merging, new tags and publishing the release wait for the owner's OK; the release is
prepared as a draft.

- [x] README.md for a newcomer: what V2 is in three lines, what it needs, install per OS, a
      quickstart of four examples that run as written (plain V2, Python, JavaScript, a program of
      your own; tests/docs), a Python/JavaScript-to-V2 table, packages, troubleshooting, FAQ,
      known limitations, uninstall, the alpha label
- [~] Prebuilt binaries for Windows, Linux and macOS from CI (`release.yml`, §22.6), with
      `install.sh` and `install.ps1` one-liners; nobody needs Rust
- [x] `v2 doctor` says what is missing, how to install it, and what is ready; every first-run
      error says what to do next: a missing script or a folder (§19.1), a file that isn't UTF-8
      (§3.1), a Python or JavaScript file given to V2, an engine not found or too old (§7.8), a
      tag of a later phase (§7.3), Python and JavaScript habits (§2.4, §5.9), a V2 variable named
      in a block (§12.1), a package that isn't installed (§13.5), pasted look-alike characters
- [~] Clean machines follow the README literally: a fresh Linux (a new WSL distro), Windows (a
      fresh profile and PATH; the CI runners), macOS (a temp HOME on the owner's Mac Mini); the
      release workflow's install check on each fresh runner (§22.6)
- [~] A newcomer test: fresh agents that only get the README install V2, run the quickstart and
      try to break it, round after round until a round finds nothing; every question they have to
      ask is a docs bug, every crash or cryptic message a code bug, each fixed with a test
- [x] A bug-report issue template and CONTRIBUTING.md
- [ ] The draft release `v0.1.0-alpha.2`; on the owner's OK: merge, tag, publish

---

## Phase 2: Compiled blocks and native libraries

C, C++ and Rust blocks compiled to cached libraries, and `extern` for existing libraries.

**Order** (the owner, 2026-11-10): 2.2 `extern` first (no toolchain); then 2.1 with 2.3, C blocks on
gcc and clang, with `cl` right after (Windows accepts only `cl`, 7.6); then 2.5 Rust and 2.6; 2.4
C++ last; 2.7.

### 2.1 Process helper
- [ ] `engines/runonly` on `engines/process`: what compiled programs use (§17.3 steps 3–5 and
      7): spawn, stdout inherited or captured, 4 KiB stderr tail, timeout, EngineExitError
      (§6.3, §15.5, §17.3); the temp file (steps 1, 2, 6) comes with run engines in Phase 3
- **Done when:** its unit tests (statement and expression output, the stderr tail, timeout and
  tree kill, EngineExitError's message and `data`) pass on Windows and Linux

### 2.2 Native FFI (`extern`)
- [ ] The `extern` statement and library resolution per platform, `SetDllDirectoryW(L"")` on
      Windows (§16.1, §16.2)
- [ ] Argument and return conversion with range checks, strings, opaque pointers (§16.5, §16.6)
- [ ] libffi call path and lazy native flush (§16.7)
- **Done when:** R23 and R25 pass

### 2.3 C blocks
- [ ] C/C++ toolchains: `cl` via vswhere + vcvars on Windows, gcc/clang elsewhere (§7.6,
      §15.7); `[engines.c|cpp] command/flags/call_timeout` (§18.2); their spec rows and `v2
      doctor` rows (§7.2, §19.2)
- [ ] Pipeline, exports and inner imports (why `@export { * }` left a function out),
      signature extraction (attributes and calling conventions skipped), generated sources and
      shims with the `v2.h` prelude (`V2_EXPORT`, `V2_EXTERN_C`, `__v2_init`/`free`/`flush`),
      `#line` mapping (§15.1–15.4)
- [ ] Files next to the script: the `.v2` folder as include directory, relative paths in `flags`,
      local headers in the cache key, the `.v2` folder in the library's dependency search
      (§15.6–15.8, §18.1)
- [ ] Program mode (no `@export`): `main` wrapping, run, capture (§15.5)
- [ ] Build cache in `~/.v2/cache/build`, looked up before any toolchain, so a hit touches no
      compiler (§15.1, §15.6); `v2 cache clean` empties all of `~/.v2/cache` (the build cache and
      `toolchains.json`) (§19.4)
- [ ] Definition blocks wait for their first call (§12.7)
- [ ] Loading and binding: evict-and-rebuild only for a damaged library, FFILoadError for a
      missing dependency (§15.8)
- **Done when:** R06 (C), R20, R56 and the `cl` cases of the `engines/resolve` unit tests (§22.3) pass

### 2.4 C++ blocks
- [ ] `std::string`, `extern "C"` shims with `try`/`catch` (§15.3, §15.4)
- **Done when:** R22 passes

### 2.5 Rust blocks
- [ ] Rust toolchain, on a cache miss only: `rustc -vV`, rustup sysroot pin, target check (§7.6, §15.7);
      `[engines.rust] command/flags/call_timeout` (§18.2); its spec row and `v2 doctor` row
- [ ] Signature parser on tokens (lifetimes ignored), `catch_unwind` shims with the Rust `v2`
      module's prelude, `String` returns (§15.3, §15.4); program blocks may define helpers
      without `main` (§15.5)
- **Done when:** R21 passes

### 2.6 Calling other languages from compiled code
- [ ] The host API in `v2.h` and the Rust `v2` module: per-import functions, `v2_last_error`
      (§15.4); `host_call` reverse calls (§15.9)
- **Done when:** R50 passes (the DOCS 8.3 example, Rust calling `py.statistics`, among it)

### 2.7 Phase 2 test sweep
- [ ] R17 (native part), R24, R51, R57
- [ ] ch. 21 rows: Build, FFI, and the compiled and native parts of Setup, Blocks and imports,
      Calls and values, and Host

**Phase 2 complete when:** `tests/compiled`, `tests/ffi`, R06 (C), R17 (native part), R20–R25,
R50, R51, R56 and R57 pass.

---

## Phase 3: Custom engines

How any other language gets in.

**Order** (the owner, 2026-11-10): VEP/1 is frozen, the minimal adapter profile and its conformance
kit come first and are proven by a Lua adapter written only from the spec; the built-in run engines
come after.

- [ ] VEP/1 frozen before any third-party adapter is invited: frames, tags, caps and their
      meanings change only in a VEP/2
- [ ] The minimal adapter profile: what a bridged adapter must do, with every cap optional
      (modules, handles, kwargs, reverse), and the stdio rules it must keep (§9.9, §14.5, §17.5)
- [ ] A conformance kit: a test runner any adapter can be checked with, beside
      `examples/fake_adapter.rs` (§17.6, §22.3)
- [ ] A Lua bridged adapter written only from the spec, passing the kit

- [ ] Custom `[engines.<tag>]` tables in `v2.toml`: spec and tag validation, registration,
      resolving only `command[0]`, `V2_ENGINE_<TAG>` replacing `command[0]`, config errors, one
      doctor row each (§7.3, §7.4, §17.1, §17.2, §18.1–18.5, §19.2). The config unit test that
      declares `ruby` as a custom engine changes when `ruby` becomes built in
- [ ] `run` engines on the 2.1 helper plus the temp file (§17.3 steps 1, 2, 6): `{file}`,
      `V2_ARG_*`, CRLF for `.bat`/`.cmd`, tree kill (§17.3, §17.4)
- [ ] `bridged` engines through the normal session code, capability checks (§14.5, §17.5)
- [ ] `define_engine(…)`: a custom engine declared from a script, through the same validation,
      registered at run time, `base_dir` the caller's folder (§17.1, §17.2, §18.1, §18.4); R59
- [ ] `examples/fake_adapter.rs` (§17.6, §22.3)
- [ ] Built-in run engines, no `v2.toml` needed: `bash` (`sh`; Git Bash on Windows), `pwsh`
      (`powershell`), `cmd`, `lua`, `ruby`, `go`, `java`, `zig`, `cs` (`csharp`); their lookups,
      probes and version patterns, not-found messages, `{file}` in a configured command, and
      `v2 doctor` rows (§7.2, §7.6–7.8, §17.3, §18.2, §19.2); R58
- [ ] Tests: R17 (run part), R35, R37, R47; the custom-engine-table and run-engine cases of the
      `engines/spec`, `engines/registry` and `engines/resolve` unit tests (§22.3); the ch. 21 rows
      for run and custom engines

**Phase 3 complete when:** `tests/custom`, R17 (run part), R35, R37, R47, R58 and R59 pass.

---

## Release checklist (after Phase 3)

- [ ] Full CI matrix from §22.5 green, with `V2_TEST_REQUIRE` set so missing engines fail
      instead of skipping
- [ ] Every runnable DOCS.md example runs as a test, with fixtures (cookbook included);
      fragments, and examples that need software outside the CI matrix or a GUI (MessageBoxA), are
      checked by hand
- [ ] Drop "being built" and the ROADMAP pointer from the status lines of DOCS.md and INTERNALS.md
      (keep DOCS.md's pointer to INTERNALS.md and INTERNALS.md's "DOCS.md wins" sentence)
- [ ] New Windows installer, and DOCS 2 "Install" describing it
- [ ] A short `README.md`: what V2 is, the taste example from DOCS 1, install, links to DOCS and
      this roadmap
- [ ] Merge `v2-reboot` into `main`, tag `v0.1.0`

---

## Later (not designed yet)

These need a DOCS + INTERNALS design pass **before** any code:

- Full `@export`/`@import` (bridged adapters) for the Phase 3 built-in run engines: `bash`,
  `pwsh`, `cmd`, `lua`, `ruby`, `go`, `java`, `zig`, `cs` (DOCS 5)
- FFI structs, callbacks, variadic functions, `#include` headers (DOCS 12)
- Cargo crates in `@rust` blocks; lists/dicts/callbacks as direct compiled-function
  parameters (DOCS 11.7)
- Standard library beyond 0.11: additions need a design pass

---

## Progress log

| Date | What happened |
| --- | --- |
| 2026-09-30 | Old V2 evaluated; decision to reboot around the polyglot system |
| 2026-10-01 | Old project archived in `old/`; DOCS.md, INTERNALS.md and this roadmap written; branch `v2-reboot` |
| 2026-10-01 | 0.1: crate, dependencies, module tree, threads, CLI parser, test harness, CI file |
| 2026-10-01 | 0.2 + 0.3: source map, error rendering, the lexer; `print` end to end |
| 2026-10-01 | 0.4: block scanning, `@export`/`@import` directives, dedent, `UnitLoc`, `extern` bodies |
| 2026-10-01 | 0.5: parser, AST, static checks, `ffi/cdecl` |
| 2026-10-01 | 0.6 + 0.7: the interpreter core (values, operators, methods, closures, control flow, errors, the stack guard) |
| 2026-10-01 | 0.8: V2 modules (`import`, R49) |
| 2026-10-02 | 0.9: the JSON codec (plain mode) and the engine-free builtins |
| 2026-10-02 | 0.10: config, CLI and exit paths; the Phase 0 exit audit. **Phase 0 complete** (Windows; Linux in WSL) |
| 2026-10-02 | 1.0: `engines/process` and the watchdog's timer stack; Phase 1 re-ordered |
| 2026-10-02 | 1.1: engine registry (py, js, ts), executable lookup, version probes, `engine_available`, `requires:`/SKIP, the `v2 doctor` table |
| 2026-10-02 | 1.0 reviewed: 12 findings fixed with regression tests |
| 2026-10-02 | 1.2 + 1.3: the VEP/1 wire and sessions, tested with scripted workers in process and as real children |
| 2026-10-02 | Reviews of 1.0–1.3: 29 confirmed findings fixed or settled, each fix with a regression test |
| 2026-10-16 | Docs trimmed to the glue-language scope: `register_engine`, `v2 run`, `v2 doctor TAG`, check's inventory, the global flags, `~/.v2/config.toml`, the ts transpiler and smaller items cut; INTERNALS lost its decision log, salvage map and duplicated text; the code follows in the same change |
| 2026-10-24 | Docs review, 43 changes approved by the owner. Added: the 0.11 small standard library (docs only), built-in run engines (Phase 3), callable handles that cross engines, a generous `to_v2(h)` (was `value(h)`), `e.cause`, `V2Error` details in foreign code. Changed in code: `func` replaces `lambda`, `index_of` and `pop(i)` replace `find` and `remove_at`, Python 3.10+ and Node 22.13+ (one Node rule), `command` must be an array, canonical `V2_ENGINE_<TAG>` only, narrower path rewriting in `command`, `in` on a dict, positional `range`, dict keys holding handles; new static errors for unused expression values, chained `==`, code after a multi-line block's `{` and compiled-block directives. Spec fixes for in-block imports, ambiguity, mapping handles, JS constructors and compiled blocks using files next to the script; machinery cut before it was built; INTERNALS shortened |
| 2026-10-24 | `define_engine` added at the owner's request (DOCS 13.3, §17.2): a script declares its own custom engines without a `v2.toml`; built with Phase 3 |
| 2026-10-24 | ROADMAP checked against INTERNALS and DOCS: Phase 1 re-cut so each milestone can pass its own tests (V2 function stubs and `V2Error` into 1.4/1.5, named args into 1.6, cross-engine callable handles, leaks, idle callbacks and R54 into 1.7), Phase 2/3 items and citations completed, stale claims fixed; the owner gave the go-ahead for 0.11 and Phase 1 |
| 2026-10-24 | 0.11: the small standard library (`exec`, `script_dir`, the file and folder functions, `sleep`, `time`, `min`/`max`/`sum`/`abs`/`round`/`div`, `assert`, `print(stderr:)`, `split()`, `push(...items)`, `sort(key, reverse)`); a review found 12 issues, each fixed with a test (drive-relative and `%` program paths, cwd-relative paths, pipes serviced at once, tree kill) |
| 2026-10-25 | 1.4: the Python engine (`adapters/py.py`, blocks reached and loaded, values, V2 functions as stubs, `V2Error`, causes, coroutines, uncaught foreign errors printed with their class and causes); a review confirmed 34 findings, fixed with tests (error data with functions or handles through Python, a finalizer printing mid-send, replies that can't fail, handle counts given back, sys.path order, 8 causes, split UTF-8) |
| 2026-10-25 | 1.5: the JavaScript and TypeScript engine (`adapters/js.js`, one vm context per unit, Promises awaited for outermost requests, ts through Node's type stripping); the 1.4 lessons applied up front; a review confirmed 24 findings, fixed with tests (Node's fs shortcuts and `fork` past the stdio wrappers, `import()` without a flag, chunks splitting surrogate pairs, a Promise that can never settle, accessor globals like `crypto`, quoted missing exports, ts error codes) |
| 2026-10-25 | 1.6: handles (attributes, items, calls and method calls in one round trip, `len`, `str`, iteration with batching, `to_v2` with the key rule and hooks, JS `new` for constructors, bound methods, named args only with `kwargs`); a review confirmed 23 findings, fixed with tests (depth limits in `to_v2` and iteration replies, natives across JS realms, Map order, i64 keys, batching only for built-in iteration, batches that stop before a bad item, hook cycles, hints) |
| 2026-10-25 | 1.7: imports, packages and cross-language calls (`engines/imports.rs`: selectors, 13.3 resolution and its messages, `@import` binding with Proxies, module units for `py.x`/`js.x` with `*` and whole-module handles, routes, imports inside blocks with same-engine stubs and constants refused, the `v2` selector; callable handles cross engines as functions); a review confirmed 11 findings, fixed with tests that fail without the fix: names bound by `*` could be declared again, a worker started before its block's import checks, Windows `.\x.js` specifiers shared one unit, JS stubs broke `extends`/`instanceof`, a CommonJS module ran twice on a nested top-level await, a subpath missing from `exports` and a null module gave the wrong errors, a dead session said "cannot import packages", plus tests for `@js`/`@ts` selectors, leak releases reaching V2 and two 13.7 paths. Found while building: the JS idle-turn collection never freed stubs, since V8 keeps `WeakRef` targets until the job ends |
| 2026-10-25 | 1.8: errors and output across languages (`engines/traces.rs`: a worker error's frames mapped to `.v2` lines and columns and printed as 2.5 says, located at the innermost block frame with that block's tag as `e.lang`; a V2 error crossing foreign code keeps its frames with the foreign ones between; output order checked with stdout captured); a review confirmed 7 findings, fixed with tests that fail without the fix: TS strip errors past a block's first line lost their position on Node 24, JS syntax errors had no column, a V2 error first raised by foreign code lost V2's frames, Python compiler errors counted bytes, `<module>` was dropped for library frames, a huge column from a worker panicked the host, and frames went to the wrong block or line (shared lines across engines, U+2028 in JS, `@@export` lines) |
| 2026-10-25 | 1.9: the seed test, lines 1-111 of the old `test_engines.v2` ported as `tests/engines/py/seed_engines.v2` (15 checks, the missing-export check asserting 13.3 step 6's message) |
| 2026-10-25 | 1.10: the Phase 1 sweep: R01-R03, R11, R29-R31 and R33 got their tests (`tests/engines/{py,js}`), R42 its js part in `ts/strip.v2`; every R of 1.1-1.10 and the ch. 21 rows for bridged engines have tests; an audit confirmed 3 test gaps, all closed (a failing action block reached again, the idle turn after a call reply, the dead state after a startup timeout or exit 9009). Phase 1 complete on Windows and Linux |
| 2026-10-30 | Owner paused Phase 2 for a redesign of language lifetime, exports and calls. Measured: unused languages cost nothing, a parked worker keeps everything at under 0.1 MB, V2 itself is about 15x slower than Python. Three review rounds (29 findings) moved the design back towards the existing rules: a language starts at first need, its top-level definition blocks and eligible imports wait in line, everything else acts as before. Compiled-only languages rejected. `@import { * }` removed; a cache hit touches no compiler. Spec updated; milestones 1.11-1.14 added |
| 2026-10-30 | 1.11: a language starts at first need. `Callable::Waiting` exists before its process does; one line in program order across languages holds top-level definition blocks and eligible `@import`s; `@import { * }` is gone. The review confirmed 22 findings with 5 root causes, all fixed: per-language lines ran blocks out of order, a block looked at its imports before the line ran, a failed line stored its error on the wrong block, a waiting `@import` saw blocks below it, and a name no block exported went unnoticed. A failure no longer stops the line. Also fixed, from Phase 1: a language refused at the depth limit could never start. A differential fuzz (each program against a copy that starts both languages first) gave 0 differences in 1,200 programs when blocks call only their own language |
| 2026-10-30 | 1.12: an idle worker gives back its RAM, and starting languages takes half the time. First Python block 181 ms to 79 ms, first Node block 92 ms to 42 ms: child start no longer snapshots every thread on the machine, the Store placeholder is remembered, and Node loads no lazy global until a block uses it. The review found 3 problems, all fixed: a trim stalled the watchdog, a worker with over 255 processes was never trimmed, and a replaced global became enumerable. Also fixed, older: `@export { * }` skipped a function named like a Node global |
| 2026-10-30 | 1.13: names go by position. `syntax/resolve` gives each scope a layout and each use a frame and slot; a body that needs no frame gets none; an empty slot falls back to the by-name walk, so behaviour is unchanged. `eval` and `exec` keep only the common cases, with an int fast path and arguments going straight into the frame. `fib(32)` 3.5 s to 1.23 s, 10M loop 3.2 s to 1.15 s: about 3x, short of the 5-10x estimated; link-time optimization and a smaller `Flow` gave nothing and were dropped. Old and new binaries gave byte-identical output on 3,000 random programs |
| 2026-10-30 to 11-03 | 1.14 sweep rounds 1-15: `tests/sweep/` written from DOCS, then hostile reviewers on snapshot binaries each round. Fixed with tests that fail without them, among many smaller ones: a frame cycle collector replacing per-call checks (closures over their own frame leaked up to 312 MB per 100,000 calls; INTERNALS 5.1), errors with no V2 call to attach to printed with the user's frames, a forked Python child closing the protocol, Node's console and writes kept in program order with Node's own semantics, a worker daemon hiding a crash, `exec` 141 ms to 1 ms on WSL, interrupts that keep what was printed, probe limits that follow `startup_timeout` |
| 2026-11-03 | The phase 0-1 audit: every R-test and ch. 21 row of Phases 0-1 has a test that checks what it says, and every DOCS example and promise was run on Windows and Linux. One wrong result (a dict key holding a non-callable foreign object overrode the dict method), five DOCS sentences fixed, 25 tests added for promises that held but had none; CI got Phase 1's columns |
| 2026-11-03 to 11-10 | Rounds 16-18 with oracle fuzzers written from the docs: the host language (62,500 programs, nothing found), values and errors across the boundary (a V2 error passing through JS came back altered; errors now count as a nesting level), traces and the waiting line (10: JS frames of a waiting request, a failed start's line, columns and carets), output order and modules (3: Python's search order across module folders, half characters held past a reply) |
| 2026-11-10 | 1.14 closed by the owner after round 18: every round still found something, lately rare edges. Windows: 232 unit, 318 `.v2`, 70 CLI tests; Linux: 230, 321, 75; clippy clean on both. Release numbers on 2026-11-10: `fib(32)` 1.30 s, 10M loop 1.04 s (3.5 s and 3.2 s before 1.13); first Python block 106 ms, first Node block 63 ms (every build since round 12 measures the same today, so it is the machine, not the code: 1.12 recorded 79 ms and 42 ms); an idle worker 19 MB (Python) and 52 MB (Node), 0 once parked, state kept. **Phases 0 and 1 complete**; Phase 2 waits for the owner |
| 2026-11-10 | The owner's review of Phases 0-1, with a second assistant from the owner's chat: processes stay the only transport (no embedded runtimes), DOCS 9.1's value model is the boundary type system (no new record or result types), no restart. Approved: a release gate before Phase 2 (the two quadratic operations, DOCS examples as tests, CI on Windows, Linux and the minimum versions, macOS on the owner's Mac Mini, a README, labels on what isn't built; pushing `v2-reboot` for CI; merging and the `v0.1.0-alpha.1` tag on the owner's OK, `old/` removed at the merge); VEP/1 reserves the `buffer` cap and tag and is frozen before Phase 3 invites adapters; Phase 2 order `extern`, C (gcc and clang, then `cl`), Rust, C++ last; Phase 3 starts with the adapter profile, a conformance kit and a Lua adapter, then the built-in run engines; `llms.txt` after the gate |
| 2026-11-11 | Release gate: everything but the merge is done. The first CI runs, and the owner's Mac Mini, found what Windows and WSL couldn't: Node 22.13 reports TypeScript strip errors with a snippet and says nothing about which module awaited in a `require()` of a TLA graph (a CommonJS module ran twice), Python 3.10 has no columns, the CI runner is an Administrator, and on macOS the adapter's own folder stayed on `sys.path` (a symlinked temp folder). All fixed with tests; `s +=` and `dict.remove` are linear (9.8 s to 0.02 s, 14.2 s to 0.13 s); the DOCS examples run as tests; MIT license. Merging, removing `old/` and the `v0.1.0-alpha.1` tag wait for the owner |
| 2026-11-12 | On the owner's OK: `v2-reboot` fast-forwarded into `main`, `old/` removed (it stays in git history before this tag), the crate's version set to `0.1.0-alpha.1` to match the tag, CI green on `main`, tagged `v0.1.0-alpha.1`. Phase 2 waits for the owner |
| 2026-11-14 | Pre-publish, newcomer round 1: two fresh agents with only the README, on a fresh Ubuntu 24.04 (a new WSL distro, root only) and on Windows (a sandbox profile), about 400 scripts, no crash or internal error. Fixed with tests: the README couldn't teach a program of one's own (a fourth example, a Python/JavaScript table, packages on Ubuntu/Debian, installer settings, exact uninstall lines); V2 functions passed to JavaScript's `map`/`forEach` failed on the extra arguments (now dropped, as JS does); `v2 x.v2 | head` ended with an IOError, and a printing block never stopped (a closed pipe now stops the run quietly, exit 141; other failed writes keep their IOError); `v2 data.csv` read 18 MB into 925 MB of RAM (refused); a V2 variable inside a block, Python/JavaScript habits, f"{x}", `@import x from`, missing packages, `npm.x`, curly quotes and no-break spaces got no hint; temp folders of killed runs stayed; the default Windows build needed VCRUNTIME140.dll (releases link the C runtime in); install.sh installed a binary before checking it ran; `/bin` and `/usr/bin` probed twice; a Linux build on Ubuntu 24.04 needs glibc 2.39 (releases build on 22.04, and the workflow checks it). Not decided, for the owner: `#!` lines and static members of imported classes (both agents met them) |
