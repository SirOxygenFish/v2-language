# V2 Roadmap

The build plan for the rebooted V2, and the one place that says **where we are**.
[DOCS.md](DOCS.md) says *what* V2 does, [INTERNALS.md](INTERNALS.md) says *how*, and this file
says *in what order* and *what's done*.

## Where we are

| | |
| --- | --- |
| **Current phase** | Pre-publish done: `v0.1.0-alpha.2` published 2026-11-19; Phase 2 is next |
| **Next task** | `llms.txt`, then Phase 2 in this file's order |
| **Last update** | 2026-11-19: `v0.1.0-alpha.2` published; milestone 1.15, the interpreter's speed (1.4-3.6× faster); pre-publish newcomer rounds 1 to 23 (progress log) |

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

I reopened Phase 1 on 2026-10-30, before Phase 2, for how languages live in memory and how
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
      18 rounds and a phase audit ran, and every one found something; I closed 1.14 after
      round 18
- **Done when:** both rounds are clean on Windows and Linux. Compiled blocks, `extern`, run engines
  and custom engines aren't built yet, so they aren't covered — **closed 2026-11-10**
  (Windows and Linux green; the two clean rounds were not reached)

### 1.15 Interpreter speed
Started 2026-11-19, before the next phase designs a compiler: the tree-walking
interpreter as fast as it goes, behavior unchanged (§5.6, §5.7).
- [x] `bench/`: nine interpreter benchmarks and the engine ones, `measure.py` (wall time) and
      `callgrind.sh` (instruction counts); the baseline is a9e3c9f
- [x] Frames reused (a pool; a loop body nothing can keep shares one frame across iterations),
      arguments bound without the binder when nothing can go wrong (V2 functions, callbacks,
      builtins, methods, `print`), ints read in place, guard clauses and tail returns without
      `Flow::Return`, method lookups cached, anonymous functions' names and method names for
      messages made only when needed, ints written as text without the formatting machinery
- [x] Measured and dropped: inline scalar clones and drops, `codegen-units = 1`, fat LTO (2-8%
      fewer instructions, no wall-time gain), `panic = "abort"` (it would change the 101 exit), a
      16-byte `Value` and slots without `RefCell` (≤ 4% estimated, not worth the risk); a faster
      allocator and PGO not tried (a new dependency or a build step each)
- **Done when:** every test passes on Windows, Linux and macOS, the baseline and new binaries agree
  byte for byte on every test file and on generated programs, and peak memory doesn't rise —
  **passed 2026-11-19**: 14,002 of 14,006 test files and generated programs alike (the other 4
  differ by pids, temp paths and pool scheduling only; 4 programs that grow a string or list past
  1.5 GB stop at the same allocation in both), peak memory the same. A review found four places
  where the order of releases or the collector's pacing had changed (a shared loop frame counted
  as no frame made, its values released before a pending recheck, a loop variable released before
  the frame's other values, a failed named call's arguments): all match the baseline now, with
  tests (tests/engines/py/release_order, collector_pacing, collector_no_iteration). A fifth, the
  stack guard's depth for pathological nesting, moved a few percent either way with Rust's frame
  sizes (same error, §5.9). Release, Linux (the lower median of two rounds of 5), before → after,
  and Python 3.12: `fib(32)` 912 → 494 ms (Python 191), a 10M `for` 592 → 258 (486), a 10M
  `while` 713 → 197 (666), calls 438 → 220, closures 357 → 247, lists 283 → 157, dicts 248 → 163,
  strings 276 → 166, try/catch 612 → 327. Windows (medians): 1193 → 511 ms (Python 3.11: 258),
  890 → 285 (504), 804 → 225 (764), 624 → 247, 567 → 350, 389 → 194, 350 → 209, 419 → 254,
  789 → 373. macOS (Apple M4, after; Python 3.14): 265 ms (158), 165 (396), 163 (550).
  Function calls stay about 1.7-2.6× slower than Python's, loops are 1.8-3.6× faster; going
  further needs a compiler (the next phase)

---

## Release gate (before Phase 2)

Decided 2026-11-10: Phases 0-1 go public as a pre-release before Phase 2 starts,
so CI, the minimum versions and macOS are proven on what exists.

- [x] `s += piece` appends in place when V2 holds the string alone, and `dict.remove` doesn't shift
      every later entry: both stay as the docs say, only no longer quadratic (DOCS 3.3, 3.6). Release:
      100,000 appends 9.8 s to 0.02 s, removing 100,000 keys 14.2 s to 0.13 s
- [x] Every runnable DOCS example of Phases 0-1 runs as a test, with fixtures: 25 of DOCS.md's 41
      and README's run (run.rs `docs_examples_run_as_documented`, tests/docs/), `skip.txt` says why
      the other 17 don't
- [x] CI green on Windows and Linux, the minimum versions (Python 3.10, Node 22.13) included, with
      `V2_TEST_REQUIRE=py,js,ts` (run #3, 2026-11-11); macOS on my Mac Mini (macOS 26.5, Apple
      Silicon, Python 3.14, Node 22.23): all green
- [x] A short `README.md`; DOCS and INTERNALS say plainly which chapters aren't built yet
- [x] VEP/1 reserves room for zero-copy data: `init` carries the host's caps, and the `buffer` cap
      and `$v2` tag names are reserved, nothing more (§9.3, §10.3)
- [x] 2026-11-12: `old/` removed, `v2-reboot` merged into `main`, tag
      `v0.1.0-alpha.1` (`v0.1.0` stays for the release after Phase 3), CI green on `main`
- After the gate: `llms.txt`, a condensed spec of a few hundred lines for coding agents, its
  examples run as tests like DOCS's

---

## Pre-publish (before Phase 2)

The goal (2026-11-14): a friend who opens the repository can read it and succeed without
asking anyone. No bug in a newcomer's first 15 minutes, and docs that answer every question before
it's asked. The release is prepared as a draft first.

- [x] README.md for a newcomer: what V2 is in three lines, what it needs, install per OS, a
      quickstart of four examples that run as written (plain V2, Python, JavaScript, a program of
      your own; tests/docs), a Python/JavaScript-to-V2 table, packages, troubleshooting, FAQ,
      known limitations, uninstall, the alpha label
- [x] Prebuilt binaries for Windows, Linux and macOS from CI (`release.yml`, §22.6), with
      `install.sh` and `install.ps1` one-liners; nobody needs Rust. The published
      `v0.1.0-alpha.2` installs with them on all three (2026-11-19)
- [x] `v2 doctor` says what is missing, how to install it, and what is ready; every first-run
      error says what to do next: a missing script or a folder (§19.1), a file that isn't UTF-8
      (§3.1), a Python or JavaScript file given to V2, an engine not found or too old (§7.8), a
      tag of a later phase (§7.3), Python and JavaScript habits (§2.4, §5.9), a V2 variable named
      in a block (§12.1), a package that isn't installed (§13.5), pasted look-alike characters
- [x] Clean machines follow the README literally: a fresh Linux (a new WSL distro), Windows (a
      fresh profile and PATH; the CI runners), macOS (a temp HOME on my Mac Mini); the
      release workflow's install check on each fresh runner (§22.6). Each newcomer round had a
      new distro and sandbox
- [x] A newcomer test: fresh agents that only get the README install V2, run the quickstart and
      try to break it, round after round; every question they have to ask is a docs bug, every
      crash or cryptic message a code bug, each fixed with a test. Paused after round 16
      (2026-11-15), continued from round 17 (2026-11-16), ended after round 23
      (2026-11-19), when the rounds found only edge cases
- [x] A bug-report issue template and CONTRIBUTING.md
- [x] The release `v0.1.0-alpha.2` (2026-11-19): `v2-reboot` merged into
      `main`, tagged and published as a pre-release (progress log)

---

## Phase 2: Compiled blocks and native libraries

C, C++ and Rust blocks compiled to cached libraries, and `extern` for existing libraries.

**Order** (2026-11-10): 2.2 `extern` first (no toolchain); then 2.1 with 2.3, C blocks on
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

**Order** (2026-11-10): VEP/1 is frozen, the minimal adapter profile and its conformance
kit come first and are proven by a Lua adapter written only from the spec; the built-in run engines
come after.

- [ ] VEP/1 frozen before any third-party adapter is invited: frames, tags, caps and their
      meanings change only in a VEP/2
- [ ] The minimal adapter profile: what a bridged adapter must do, with every cap optional
      (modules, handles, kwargs, reverse), and the stdio rules it must keep (§9.9, §14.5, §17.5)
- [ ] A conformance kit: a test runner any adapter can be checked with, beside
      `examples/fake_adapter.rs` (§17.6, §22.3)
- [ ] A Lua bridged adapter written only from the spec, passing the kit

- [x] Custom `[engines.<tag>]` tables in `v2.toml`: spec and tag validation, registration,
      resolving only `command[0]`, `V2_ENGINE_<TAG>` replacing `command[0]`, config errors, one
      doctor row each (§7.3, §7.4, §17.1, §17.2, §18.1–18.5, §19.2). The config unit test that
      declares `ruby` as a custom engine changes when `ruby` becomes built in
- [x] `run` engines on the 2.1 helper plus the temp file (§17.3 steps 1, 2, 6): `{file}`,
      `V2_ARG_*`, CRLF for `.bat`/`.cmd`, tree kill (§17.3, §17.4)
- [x] `bridged` engines through the normal session code, capability checks (§14.5, §17.5)
- [x] `define_engine(…)`: a custom engine declared from a script, through the same validation,
      registered at run time, `base_dir` the caller's folder (§17.1, §17.2, §18.1, §18.4); R59
- [x] `examples/fake_adapter.rs` (§17.6, §22.3)
- [x] Built-in run engines, no `v2.toml` needed: `bash` (`sh`; Git Bash on Windows), `pwsh`
      (`powershell`), `cmd`, `lua`, `ruby`, `go`, `java`, `zig`, `cs` (`csharp`); their lookups,
      probes and version patterns, not-found messages, `{file}` in a configured command, and
      `v2 doctor` rows (§7.2, §7.6–7.8, §17.3, §18.2, §19.2); R58
- [x] Tests: R17 (run part), R35, R37, R47; the custom-engine-table and run-engine cases of the
      `engines/spec`, `engines/registry` and `engines/resolve` unit tests (§22.3); the ch. 21 rows
      for run and custom engines

**Phase 3 complete when:** `tests/custom`, R17 (run part), R35, R37, R47, R58 and R59 pass.
Status (2026-11-26): all of these pass on Linux (R58's `pwsh` and `cmd` tests skip where those
engines aren't installed). The Windows-only lookups (Git Bash, the `powershell` fallback, `cmd`) are
written but not yet run on Windows. The four items above about third-party adapters are still open.

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
- [ ] Tag `v0.1.0`

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
| 2026-10-24 | Docs review, 43 changes approved. Added: the 0.11 small standard library (docs only), built-in run engines (Phase 3), callable handles that cross engines, a generous `to_v2(h)` (was `value(h)`), `e.cause`, `V2Error` details in foreign code. Changed in code: `func` replaces `lambda`, `index_of` and `pop(i)` replace `find` and `remove_at`, Python 3.10+ and Node 22.13+ (one Node rule), `command` must be an array, canonical `V2_ENGINE_<TAG>` only, narrower path rewriting in `command`, `in` on a dict, positional `range`, dict keys holding handles; new static errors for unused expression values, chained `==`, code after a multi-line block's `{` and compiled-block directives. Spec fixes for in-block imports, ambiguity, mapping handles, JS constructors and compiled blocks using files next to the script; machinery cut before it was built; INTERNALS shortened |
| 2026-10-24 | `define_engine` added (DOCS 13.3, §17.2): a script declares its own custom engines without a `v2.toml`; built with Phase 3 |
| 2026-10-24 | ROADMAP checked against INTERNALS and DOCS: Phase 1 re-cut so each milestone can pass its own tests (V2 function stubs and `V2Error` into 1.4/1.5, named args into 1.6, cross-engine callable handles, leaks, idle callbacks and R54 into 1.7), Phase 2/3 items and citations completed, stale claims fixed; go-ahead for 0.11 and Phase 1 |
| 2026-10-24 | 0.11: the small standard library (`exec`, `script_dir`, the file and folder functions, `sleep`, `time`, `min`/`max`/`sum`/`abs`/`round`/`div`, `assert`, `print(stderr:)`, `split()`, `push(...items)`, `sort(key, reverse)`); a review found 12 issues, each fixed with a test (drive-relative and `%` program paths, cwd-relative paths, pipes serviced at once, tree kill) |
| 2026-10-25 | 1.4: the Python engine (`adapters/py.py`, blocks reached and loaded, values, V2 functions as stubs, `V2Error`, causes, coroutines, uncaught foreign errors printed with their class and causes); a review confirmed 34 findings, fixed with tests (error data with functions or handles through Python, a finalizer printing mid-send, replies that can't fail, handle counts given back, sys.path order, 8 causes, split UTF-8) |
| 2026-10-25 | 1.5: the JavaScript and TypeScript engine (`adapters/js.js`, one vm context per unit, Promises awaited for outermost requests, ts through Node's type stripping); the 1.4 lessons applied up front; a review confirmed 24 findings, fixed with tests (Node's fs shortcuts and `fork` past the stdio wrappers, `import()` without a flag, chunks splitting surrogate pairs, a Promise that can never settle, accessor globals like `crypto`, quoted missing exports, ts error codes) |
| 2026-10-25 | 1.6: handles (attributes, items, calls and method calls in one round trip, `len`, `str`, iteration with batching, `to_v2` with the key rule and hooks, JS `new` for constructors, bound methods, named args only with `kwargs`); a review confirmed 23 findings, fixed with tests (depth limits in `to_v2` and iteration replies, natives across JS realms, Map order, i64 keys, batching only for built-in iteration, batches that stop before a bad item, hook cycles, hints) |
| 2026-10-25 | 1.7: imports, packages and cross-language calls (`engines/imports.rs`: selectors, 13.3 resolution and its messages, `@import` binding with Proxies, module units for `py.x`/`js.x` with `*` and whole-module handles, routes, imports inside blocks with same-engine stubs and constants refused, the `v2` selector; callable handles cross engines as functions); a review confirmed 11 findings, fixed with tests that fail without the fix: names bound by `*` could be declared again, a worker started before its block's import checks, Windows `.\x.js` specifiers shared one unit, JS stubs broke `extends`/`instanceof`, a CommonJS module ran twice on a nested top-level await, a subpath missing from `exports` and a null module gave the wrong errors, a dead session said "cannot import packages", plus tests for `@js`/`@ts` selectors, leak releases reaching V2 and two 13.7 paths. Found while building: the JS idle-turn collection never freed stubs, since V8 keeps `WeakRef` targets until the job ends |
| 2026-10-25 | 1.8: errors and output across languages (`engines/traces.rs`: a worker error's frames mapped to `.v2` lines and columns and printed as 2.5 says, located at the innermost block frame with that block's tag as `e.lang`; a V2 error crossing foreign code keeps its frames with the foreign ones between; output order checked with stdout captured); a review confirmed 7 findings, fixed with tests that fail without the fix: TS strip errors past a block's first line lost their position on Node 24, JS syntax errors had no column, a V2 error first raised by foreign code lost V2's frames, Python compiler errors counted bytes, `<module>` was dropped for library frames, a huge column from a worker panicked the host, and frames went to the wrong block or line (shared lines across engines, U+2028 in JS, `@@export` lines) |
| 2026-10-25 | 1.9: the seed test, lines 1-111 of the old `test_engines.v2` ported as `tests/engines/py/seed_engines.v2` (15 checks, the missing-export check asserting 13.3 step 6's message) |
| 2026-10-25 | 1.10: the Phase 1 sweep: R01-R03, R11, R29-R31 and R33 got their tests (`tests/engines/{py,js}`), R42 its js part in `ts/strip.v2`; every R of 1.1-1.10 and the ch. 21 rows for bridged engines have tests; an audit confirmed 3 test gaps, all closed (a failing action block reached again, the idle turn after a call reply, the dead state after a startup timeout or exit 9009). Phase 1 complete on Windows and Linux |
| 2026-10-30 | Phase 2 paused for a redesign of language lifetime, exports and calls. Measured: unused languages cost nothing, a parked worker keeps everything at under 0.1 MB, V2 itself is about 15x slower than Python. Three review rounds (29 findings) moved the design back towards the existing rules: a language starts at first need, its top-level definition blocks and eligible imports wait in line, everything else acts as before. Compiled-only languages rejected. `@import { * }` removed; a cache hit touches no compiler. Spec updated; milestones 1.11-1.14 added |
| 2026-10-30 | 1.11: a language starts at first need. `Callable::Waiting` exists before its process does; one line in program order across languages holds top-level definition blocks and eligible `@import`s; `@import { * }` is gone. The review confirmed 22 findings with 5 root causes, all fixed: per-language lines ran blocks out of order, a block looked at its imports before the line ran, a failed line stored its error on the wrong block, a waiting `@import` saw blocks below it, and a name no block exported went unnoticed. A failure no longer stops the line. Also fixed, from Phase 1: a language refused at the depth limit could never start. A differential fuzz (each program against a copy that starts both languages first) gave 0 differences in 1,200 programs when blocks call only their own language |
| 2026-10-30 | 1.12: an idle worker gives back its RAM, and starting languages takes half the time. First Python block 181 ms to 79 ms, first Node block 92 ms to 42 ms: child start no longer snapshots every thread on the machine, the Store placeholder is remembered, and Node loads no lazy global until a block uses it. The review found 3 problems, all fixed: a trim stalled the watchdog, a worker with over 255 processes was never trimmed, and a replaced global became enumerable. Also fixed, older: `@export { * }` skipped a function named like a Node global |
| 2026-10-30 | 1.13: names go by position. `syntax/resolve` gives each scope a layout and each use a frame and slot; a body that needs no frame gets none; an empty slot falls back to the by-name walk, so behaviour is unchanged. `eval` and `exec` keep only the common cases, with an int fast path and arguments going straight into the frame. `fib(32)` 3.5 s to 1.23 s, 10M loop 3.2 s to 1.15 s: about 3x, short of the 5-10x estimated; link-time optimization and a smaller `Flow` gave nothing and were dropped. Old and new binaries gave byte-identical output on 3,000 random programs |
| 2026-10-30 to 11-03 | 1.14 sweep rounds 1-15: `tests/sweep/` written from DOCS, then hostile reviewers on snapshot binaries each round. Fixed with tests that fail without them, among many smaller ones: a frame cycle collector replacing per-call checks (closures over their own frame leaked up to 312 MB per 100,000 calls; INTERNALS 5.1), errors with no V2 call to attach to printed with the user's frames, a forked Python child closing the protocol, Node's console and writes kept in program order with Node's own semantics, a worker daemon hiding a crash, `exec` 141 ms to 1 ms on WSL, interrupts that keep what was printed, probe limits that follow `startup_timeout` |
| 2026-11-03 | The phase 0-1 audit: every R-test and ch. 21 row of Phases 0-1 has a test that checks what it says, and every DOCS example and promise was run on Windows and Linux. One wrong result (a dict key holding a non-callable foreign object overrode the dict method), five DOCS sentences fixed, 25 tests added for promises that held but had none; CI got Phase 1's columns |
| 2026-11-03 to 11-10 | Rounds 16-18 with oracle fuzzers written from the docs: the host language (62,500 programs, nothing found), values and errors across the boundary (a V2 error passing through JS came back altered; errors now count as a nesting level), traces and the waiting line (10: JS frames of a waiting request, a failed start's line, columns and carets), output order and modules (3: Python's search order across module folders, half characters held past a reply) |
| 2026-11-10 | 1.14 closed after round 18: every round still found something, lately rare edges. Windows: 232 unit, 318 `.v2`, 70 CLI tests; Linux: 230, 321, 75; clippy clean on both. Release numbers on 2026-11-10: `fib(32)` 1.30 s, 10M loop 1.04 s (3.5 s and 3.2 s before 1.13); first Python block 106 ms, first Node block 63 ms (every build since round 12 measures the same today, so it is the machine, not the code: 1.12 recorded 79 ms and 42 ms); an idle worker 19 MB (Python) and 52 MB (Node), 0 once parked, state kept. **Phases 0 and 1 complete**; Phase 2 is next |
| 2026-11-10 | A second review of Phases 0-1: processes stay the only transport (no embedded runtimes), DOCS 9.1's value model is the boundary type system (no new record or result types), no restart. Approved: a release gate before Phase 2 (the two quadratic operations, DOCS examples as tests, CI on Windows, Linux and the minimum versions, macOS on my Mac Mini, a README, labels on what isn't built; pushing `v2-reboot` for CI; merging and the `v0.1.0-alpha.1` tag after that, `old/` removed at the merge); VEP/1 reserves the `buffer` cap and tag and is frozen before Phase 3 invites adapters; Phase 2 order `extern`, C (gcc and clang, then `cl`), Rust, C++ last; Phase 3 starts with the adapter profile, a conformance kit and a Lua adapter, then the built-in run engines; `llms.txt` after the gate |
| 2026-11-11 | Release gate: everything but the merge is done. The first CI runs, and my Mac Mini, found what Windows and WSL couldn't: Node 22.13 reports TypeScript strip errors with a snippet and says nothing about which module awaited in a `require()` of a TLA graph (a CommonJS module ran twice), Python 3.10 has no columns, the CI runner is an Administrator, and on macOS the adapter's own folder stayed on `sys.path` (a symlinked temp folder). All fixed with tests; `s +=` and `dict.remove` are linear (9.8 s to 0.02 s, 14.2 s to 0.13 s); the DOCS examples run as tests; MIT license. Merging, removing `old/` and the `v0.1.0-alpha.1` tag are next |
| 2026-11-12 | `v2-reboot` fast-forwarded into `main`, `old/` removed (it stays in git history before this tag), the crate's version set to `0.1.0-alpha.1` to match the tag, CI green on `main`, tagged `v0.1.0-alpha.1`. Phase 2 is next |
| 2026-11-14 | Pre-publish, newcomer round 1: two fresh agents with only the README, on a fresh Ubuntu 24.04 (a new WSL distro, root only) and on Windows (a sandbox profile), about 400 scripts, no crash or internal error. Fixed with tests: the README couldn't teach a program of one's own (a fourth example, a Python/JavaScript table, packages on Ubuntu/Debian, installer settings, exact uninstall lines); V2 functions passed to JavaScript's `map`/`forEach` failed on the extra arguments (now dropped, as JS does); `v2 x.v2 | head` ended with an IOError, and a printing block never stopped (a closed pipe now stops the run quietly, exit 141; other failed writes keep their IOError); `v2 data.csv` read 18 MB into 925 MB of RAM (refused); a V2 variable inside a block, Python/JavaScript habits, f"{x}", `@import x from`, missing packages, `npm.x`, curly quotes and no-break spaces got no hint; temp folders of killed runs stayed; the default Windows build needed VCRUNTIME140.dll (releases link the C runtime in); install.sh installed a binary before checking it ran; `/bin` and `/usr/bin` probed twice; a Linux build on Ubuntu 24.04 needs glibc 2.39 (releases build on 22.04, and the workflow checks it). Not decided yet: `#!` lines and static members of imported classes (both agents met them) |
| 2026-11-14 | Pre-publish, newcomer round 2: two new agents on new sandboxes (another fresh Ubuntu 24.04 distro; a Windows profile under a folder named `win2 ü`), no crash or internal error, about 40 findings. Fixed with tests: an activated virtual environment's `python` lost to a system `python3` (PATH is now searched folder by folder); a Node.js that fails to start (a missing libatomic) looked "not found", doctor didn't say where an old or broken program was, and Node 23.1 was called "too old"; in `v2.toml` a `command` string with a path with spaces was split, `~/` wasn't the home folder, and `venv = .venv` got no hint; a missing folder in the script's path, `-h`, `error("text")`, `input()` after a Windows PowerShell pipe (a BOM), the caret under wide characters, a script at a drive's root; hints for `import numpy`, `from x import y`, `lambda`, `print "x"`, a quoted selector, a type annotation, `js.date-fns`, backticks, `open()`, `sorted()`, `str.join`, and `@import` from a label with no exports. The installers: bash login shells (and zsh in ssh commands) didn't get the PATH line, a second install folder got none, fish set a universal variable, `V2_NO_MODIFY_PATH=true` was ignored, `~` and relative paths, a folder that is a file, a second upgrade while an old v2.exe still ran failed, "works in this window" was wrong when run as a file, and archives now unpack into one folder; the new `tests/install/paths.{sh,ps1}` (run by release.yml) then found that a home folder with a space broke install.sh's PATH lines. The README: installing from downloads, Windows Python and venv commands, npm names, more table rows, cron/ssh, libatomic, `npm.ps1`, stdin and handle limits, and an uninstall that never deletes a whole `V2_INSTALL_DIR`. Windows: 248 unit, 329 `.v2`, 76 CLI tests; Linux: 246, 332, 82; clippy clean on both. Kept as specified: SIGTERM exits 130, cross-language nesting stops at 64 |
| 2026-11-14 | Review of round 2's fixes (two finders, a refuter each): 19 findings, 15 confirmed, each fixed with a test that fails without the fix. Hints: `@import mean from stats` suggested a Python module `py.mean`, `import dayjs from "dayjs"` led to pip, dashed names stopped at a digit or keyword (`js."utf"` for `utf-8-validate`); `v2.toml`: a command string kept `/usr/bin/env python3` as one program and kept quotes around a path, and the quotes hint swallowed a `# comment`; doctor: "(see why below)" pointed up, a broken row could name a Store placeholder, a configured command with a space wasn't quoted; carets missed under ✅, ⭐ and combining marks; install.sh: an archive for another computer said "has no v2 in it", a startup file it couldn't write was reported as already set, a commented-out line counted as present, `dir/` and `.` weren't normalized, a folder named `v2` swallowed the binary; README: a Windows path PowerShell can't run. The first release run of `tests/install/paths.sh` failed on two of the test's own assumptions (fish 3.3 in the C locale; macOS's sh sets SHELL), fixed and checked on Ubuntu 22.04 with zsh and fish and on my Mac. Windows: 248 unit, 329 `.v2`, 78 CLI tests; Linux: 246, 332, 83; clippy clean on both |
| 2026-11-15 | Pre-publish, newcomer round 3: two new agents (another fresh Ubuntu 24.04 distro, root only; a Windows sandbox named `round 3 (ü)`), about 500 actions, 37 findings, no crash or internal error. Fixed with tests: on WSL a script with a block spent 0.3 s per language looking in dozens of Windows PATH folders (the search now stops at the first usable program: a one-line `@py` block 333 → 36 ms); `[engines.node]` and `[engines.python3]` were silently ignored; a venv naming its python, or a misnamed venv, got the wrong advice; Explorer's "Copy as path" quotes in a command; `V2_ENGINE_PY="py -3"`; the Store placeholder as a command; a missing libatomic got the generic install line; Node 23.1 showed as `old` and its skip twice; `read_file` kept the BOM of PowerShell's `Set-Content -Encoding utf8`; encoding errors from `read_file` and `input()` said nothing about what to do; a `\n` in a Windows path garbled the message; a folder read as "Access is denied"; invisible characters didn't show in lists; a syntax error in an imported file showed without its line or hint; hints for `raise`, `throw new`, `except`, `a if c else b`, `func f(x) => …`, `process`, `sys`, `Math`; `sys.exit(3)` now names V2's `exit(3)`. Installers: a folder name with `"`, `$` or a backtick broke the startup files; a folder already on PATH was added again, and bash got the folder two or three times; fish ignored `XDG_CONFIG_HOME`; an update into another folder didn't mention the v2 the terminal runs; Ctrl+C left the temp folder; `V2_VERSION=latest`; an unchecked damaged archive said to build from source; `v2.exe.old` stayed without a word; archives stored the builder's uid. README: V2's own functions, `e.message`, other `.v2` files, statement lambdas, update and uninstall steps, PowerShell 5.1 pipes, Python's UTF-8 mode, programs a block starts print to stderr (DOCS 4 says why), the nearest `v2.toml`. Kept as specified: `7 // 2` is a comment; every stop signal exits 130 (the help now says so). Not V2's: the round's Linux binary was a local build that needs glibc 2.39 (release builds are checked against 2.35). Windows: 249 unit, 330 `.v2`, 81 CLI tests; Linux: 247, 333, 86; clippy clean on both |
| 2026-11-15 | Pre-publish, newcomer round 4: two new agents (another fresh Ubuntu 24.04 distro, root only; a Windows sandbox named `round 4 d'Été`), 42 findings, no crash or internal error. Fixed with tests: a syntax error in a Python, JavaScript or TypeScript file an `@import` loads pointed at the `@import`, not at the file's line (now its own file and line come first, a TypeScript file's frames say `in ts`, and Python's import machinery and V8's `Module.` prefix are left out); Python's `n // 2` quietly meant `n` (now an error that points at `div(n, 2)`); a no-break space pasted into a Python block's indentation was reported on another line; `import "./util"` without `.v2`, `.v2` files named like packages (`from "./util.v2"`, `js."./util.v2"`, JavaScript's own `import … from "./util.v2"`) and a Python decorator named `export` got no advice; operators on handles and `to_v2` failures now say what to do instead; an imported function's own properties (`dayjs.extend`, `_.chunk`) and a module that is one function (`lodash/chunk`) point at the module form; `read_file("data.csv")` run from another folder points at `script_dir()`; `exec("git status")` shows the list; `json`, `os` and `env` habits; a script run through a symlink read the link's folder's `v2.toml`; a venv given with `..` was shown with it; a missing `TMP` folder didn't say which variable to fix (doctor notes it now); `V2_ENGINE_PY` naming a folder; a `py` launcher with no Python left counted as broken, not missing; "no version" didn't say what the program printed; a home folder that doesn't exist got a cache; `v2 -` and `v2 --`; Node advised `"type": "module"` for a TypeScript file that imports another, which would break the CommonJS files there. Found while writing the spec: a form feed in a Python block's indentation, which Python takes, was refused. README: V2's file and system functions, `map`, slices, f-string formats, multi-line strings, imported `.v2` files are a snapshot, TypeScript files and limits, when packages load, cmd's code page, a minimal Linux (no `sudo`, `xz-utils`), known limitations (handles, callbacks only during a call, JavaScript's event loop, each block's own variables, Ctrl+C and `finally`). Kept as specified: `v2 check` stops at a file's first error; `exit(256)` wraps on Linux and macOS (DOCS 3.11 says so). Windows: 249 unit, 335 `.v2`, 84 CLI tests; Linux: 247, 338, 90; macOS: 246, 338, 87; clippy clean on Windows and Linux |
| 2026-11-15 | Pre-publish, newcomer round 5: two new agents (another fresh Ubuntu 24.04 distro, root only; a Windows sandbox named `раунд 5 日本`, outside the ANSI code page), about 440 actions, 41 findings, no crash or internal error. First, CI's minimum-versions job (Node 22.13) found that importing a `.ts` file relied on Node 22.18's default: older Nodes now get `--experimental-strip-types`, with its startup warning off, and a `.ts` file's syntax error is placed there too (checked on Node 22.13 in WSL). Fixed with tests: a large value sent to JavaScript took time in the square of its size (60 MiB: 13.6 s, now 0.3 s); a block's output to a file or a pipe waited for the call's end, and was lost if V2 was killed; `v2 x.v2 | head -1` ran the rest of a script that prints and sleeps; a `numbers.py` or `json.py` next to the script silently lost to the module Python already had, and two folders' modules of one name collided in silence (both now an ImportError naming both files, DOCS 7 changed); `[engines.pyy]`, `[engines.Python]`, and `[python] venv` beside `[engines.py] command`, configured nothing without a word; `V2_ENGINE_PY` naming a venv got a path that doesn't exist; a forgotten `@import` gave a bare NameError (now it names the block that exports the name), and `from py` didn't point at `@py`; a misspelled imported name gets a did-you-mean (`meen`: `mean`); a thrown JS object showed as `[object Object]`, an uncaught JS exception had no label, `process.exit` didn't point at `exit(n)`; a block's classes showed as `__v2_0_38.Counter`; doctor's Linux line said `sudo apt … / sudo dnf …` even as root on Ubuntu; `input()` kept one of PowerShell 5.1's two BOMs; `V2_ENGINE_PY` with a trailing space; `remove_file` on a folder said "Access is denied"; a script that isn't UTF-8 was placed at 1:1 and didn't say what it probably is; a configured engine that is too old was worded unlike one on PATH; code after a multi-line block's `{` got the closing-brace error. README: root without `sudo`, WSL's downloads folder, no `nano`, `${x}` without the `f`, `sleep(seconds)`, the one format, block layout and parentheses, `({a: 1})` in `@js`, when packages load, another shell after installing, Set-Content's encoding, async JavaScript, V2 errors in Python and JavaScript, callbacks as they really behave, each `@js` block's own realm (`instanceof`; documented in DOCS 5 already), `exit()` and `finally`, temp folders of killed runs. Not done, nits the docs cover: a decorator's bare SyntaxError (the FAQ lists decorators), the KeyError for an imported file's `_name` (the README says `_` names aren't imported), `print` in a `@py` block about 20 times slower than plain Python's when redirected. Windows: 250 unit, 340 `.v2`, 86 CLI tests; Linux: 248, 343, 92 (the `.v2` tests also on Node 22.13); macOS: 247, 343, 89; clippy clean on Windows and Linux |
| 2026-11-15 | Pre-publish, newcomer round 6: two new agents (another fresh Ubuntu 24.04 distro, root only; a Windows sandbox named `r6 🚀 OneDrive - Contoso`), about 400 actions, 43 findings, one major, no crash or internal error. Fixed with tests: Python's `//` after a call, an index, a member or parentheses (`len(xs) // 2`) still quietly began a comment (the major one); `exec` output from Windows console programs lost its non-ASCII text (now read in the console's code page); an error a worker printed with no call to raise it in (a JS timer's, a Promise nobody waits for, a Python thread's) left exit code 0 (now 1: `out` frames mark such text); `v2 doctor` said ready for a Python that couldn't start (`PYTHONHOME`, `PYTHONPATH` and `NODE_OPTIONS` are named in doctor's notes and in the startup crash); install hints blamed the wrong name for a submodule, an npm subpath or a typo, and missed a `.venv` V2 wasn't using; `import "./data.csv"` was parsed as V2; `py."./helper.py"`, a typo in `@export`, a `_name` of an imported file, `map`, `list`, `enumerate`, `zip`, `parseInt`, `floor`, `xs.find`, `d.set`; `\u` and `\x` in Windows paths; the `{name}` f-string message read like a warning; file and folder mix-ups in the OS's words; a `.bat` under a `%` path; `V2_ENGINE_PY` with a spaced path and arguments; "a newer JavaScript"; a script named `-dash.v2`; two `install.ps1` messages. README: a short list of V2's functions, methods and syntax (binary files, `exec`'s `input:`, `sort(key:)`, `filter`, `lines`, `?.`, `??`, `**`, the fence form, …), when packages load, npm in Windows PowerShell, cron and the engines, `V2_ENGINE_PY` in PowerShell, deep values, errors V2 can't raise, an empty `~/.zshenv`. Not done, documented instead: a cap on `exec`'s output, `exit()` codes past 255, `v2 check` of tags and labels, hints for decorators and object literals in blocks. Windows: 251 unit, 344 `.v2`, 89 CLI tests; Linux: 249, 347, 94 (the `.v2` tests also on Node 22.13); macOS: 248, 347, 91; the installer tests on all three; clippy clean on Windows and Linux |
| 2026-11-15 | Pre-publish, newcomer round 7: two new agents (another fresh Ubuntu 24.04 distro, root only; a Windows sandbox whose home folder alone is over 200 characters), about 450 actions, 39 findings, four major, no crash or internal error. Fixed with tests: on Windows, blocks and the installer failed under a temp folder past about 200 characters (the run folder is now made by its `\\?\` path and used by its 8.3 name when its files wouldn't fit; `install.ps1` unpacks only `v2.exe` into a short folder and runs it by its 8.3 name; both say when a folder is too long); `multiprocessing` with spawn or forkserver and a block's function hung, naming an internal module (each child now says why and what to do, and so does a pickle of another run's block class); the Store's `python.exe` outside `LOCALAPPDATA` counted as a broken Python (a simulation artifact, made sturdy); a script's `if __name__ == "__main__":` in a block quietly ran nothing (now an error at that line); JavaScript timers and promises still waiting at the end never ran (now they do, within the grace second, as Python's threads); `@js { {a: 1} }` was quietly 1; TypeScript files' own `./x.js` and type imports, a misspelled Python module, `class`, `switch`, block args that are expressions, `~`, `%VAR%` and `$env:` in paths, `%VAR%` in `V2_ENGINE_*` and `v2.toml`, a venv's `bin` folder, `dir`, `cd` and `copy` in `exec`, `remove_file` on a folder, a `list_dir` name that isn't UTF-8, too-large and surrogate MarshalErrors, a duplicate JavaScript trace line, doctor's table with a long path (COMMAND is now last) and ts's notes, `V2_VERSION` with `V2_ARCHIVE`; and CI's UTF-8 console code page in `exec_windows.v2`. README: shell recipes, `sort` returning null, `round`, escapes, BigInt and key order, `.venv\Scripts\python -m pip`, `V2_ENGINE_PY` in PowerShell, a Node 22 route for Ubuntu, TypeScript imports, the multiprocessing limit, timers at the end, modules in the project folder, imports inside blocks, `DOCS.md` in the archives. Windows: 255 unit, 351 `.v2`, 92 CLI tests; Linux: 252, 354, 97 (the `.v2` tests also on Node 22.13); macOS: 251, 354, 93; the installer tests on all three; clippy clean on Windows and Linux |
| 2026-11-15 | Pre-publish, newcomer round 8: two new agents (another fresh Ubuntu 24.04 distro, root only; a Windows sandbox named `r8 Données & Co (2026)`, for a cmd.exe user), both now with the archive's DOCS.md, about 470 actions, 31 findings, none major, no crash or internal error. Fixed with tests: Ctrl+Z stopped v2 but not its workers or `exec`'s children (now the job stops and goes on as a whole), and time stopped (or asleep) counted toward `call_timeout`, killing the engine on `fg` (the clock skips it now; macOS's `waitid` reporting a continue as an exit is fixed too); a V2 function imported into JavaScript with `from v2` didn't drop extra arguments as a passed one does; `exec`'s Python children wrote the ANSI code page (`café` came back as `cafÚ`, CJK crashed them: now `PYTHONIOENCODING=utf-8` unless set); round 7's module did-you-mean suggested `runpy` for `numpy` (now one edit, six letters or more); run folders of killed runs stayed when Windows gave their pid to another program; suggested commands didn't quote `R&D`; `import "./lib"` on a folder said "Access is denied"; `define_engine`, `@r` and `[engines.r]` didn't say custom engines aren't here yet; `e.hint` (new) lets a program that catches an error show its hint; and messages and hints for `to_v2` of a date, a file or input that isn't UTF-8, `json_parse` of a big integer, `f"{x:.2f}"`, `xs[::2]`, `[0, ...a]`, `{1: "x"}`, a variable named `str`, a JS last line that joins the one above, decorators in `@ts`, an import-only npm package, a missing `sub/x.v2`, a dangling symlink, an unset PATH, Node given as Python. DOCS.md's first example runs and links to GitHub; README: `Get-Content -Encoding UTF8`, `sudo` as root, the Node route per shell, when definition blocks run, `sys.argv` in blocks, renaming and copying files, printing a lot. Documented instead: a closed stdout drops output, as Python's `print` does. Windows: 258 unit, 355 `.v2`, 93 CLI tests; Linux: 255, 358, 100 (the `.v2` tests also on Node 22.13); macOS: 254, 358, 96; clippy clean on Windows and Linux |
| 2026-11-15 | Pre-publish, newcomer round 9: two new agents (another fresh Ubuntu 24.04 distro, root only; a Windows sandbox named `r9 team tools`), each building tools for a team (a log analyzer over a million lines, an HTTP checker, a pandas report, a backup script). One major finding, no crash or internal error. Fixed with tests: `make_dir("~/out")` made a folder named `~` here, which a later `rm -rf ~` would follow home (a path that starts with `~` or `$HOME`, `%USERPROFILE%`, … is now refused with what to write, `./~` for a real one); a line printed just before a long V2 computation waited in the buffer when stdout was a pipe or a file (a thread writes it 100 ms later now); `exec` said only "not found on PATH" for a program in the current folder, gave the OS's words for a `.ps1` or `.py` file, a folder and a batch argument with a line break, and returned UTF-16 output (`cmd /u`, `wsl --list`) as NULs; `@import { k } from v2` of a `let` that holds data gave the block a fake function; a sort key that changed the list lost its changes (a `ValueError` now, as in Python); `f"${n:.2f}"` changed the digits of ints past 2^53; a name used before its `let` said only "is not declared" (now `used before its let on line 12`), and one-letter names got a did-you-mean for any other one (`y` → `f`, now none); and messages and hints for a CSS `@import` in a block's string, `x.upper` without the call, `2 ^ 3`, `if x = 2`, JavaScript's bare `{ }` block, `{from: x}`, a named argument already given, an ES module's default export (`chalk.red`), a nested Promise, a venv whose Python was upgraded away or set to `""`, a missing folder or a file in the way, `v2 check` stopping at a file's first syntax error, doctor with PATH unset or only a broken Python. Docs: README `args().get(0, …)`, list `get`, when packages load, `$SHELL`, `.default`; DOCS `f"{{name}}"`, keyword keys, `read_file` of a file that isn't UTF-8, how `exec` finds programs and runs scripts, `@@import` for lines in strings, what a failed block raises again, `define_engine`, `exit`'s range. Windows: 261 unit, 357 `.v2`, 95 CLI tests; Linux: 259, 360, 102 (the `.v2` tests also on Node 22.13); macOS: 258, 360, 98; clippy clean on all three |
| 2026-11-15 | Pre-publish, newcomer round 10: two new agents (another fresh Ubuntu 24.04 distro, root only, as a JavaScript and TypeScript developer who writes shell scripts; a Windows sandbox named `r10 dåta+analysis`, as a data analyst with pandas, openpyxl and matplotlib in a project venv). Two major findings, no crash or internal error. Fixed with tests: two loop variables over a pandas Series gave positions, and `for k, v in h.items()` each pair's index and the pair, silently (now a Python object with `items()` gives keys and values, and `.items()`, `.iterrows()` and `.entries()` loops are a TypeError that says what to write); `[python] venv` set to a conda environment or a Python installation suggested running `-m venv` over it (now it says what that folder is and what to write, and `-m venv` is never suggested into a folder that holds anything); `write_file("/dev/stdout", …)` came before what `print` wrote, and with `> file` started the file over (now it goes through V2's own stdout); `exec(["cmd", "/c", "…"])` broke on quoted paths, as Rust quotes for a program (now the command line goes to cmd.exe as written); a missing `.bat` wasn't an IOError; a foreign exception's own fields were lost (now `e.data`: Node's `code`, the n of `process.exit(n)` and `sys.exit(n)` as `exit_code`, an OSError's `errno` and `filename`); `to_v2` of a block's JS date gave the wrong advice, and typed arrays stayed handles (now lists); V2's own words couldn't name an argument (`df.agg(func: "sum")`) or a dict key (`{from: 1}`, as JavaScript allows); and messages and hints for `UnicodeDecodeError` in a block (Python's UTF-8 mode), a date-keyed Series in `to_v2`, a `Decimal` in `float()`, `int()` and `:.2f`, `input()` from a file that isn't UTF-8, `read_file`'s advice (now `e.hint`), a handle operation's example (now that operation, in the handle's language), `venv_dir`, `count = 1` suggesting `round`, `xs.remove`, `count`, `clear`, `s.ljust`, `d.keys` without the call, `??=`, `elsif`, `:=`, `@export { show as pyshow }`, `@export { * }` leaving out a `const`, `@import py.os as os` in a block, `@py(class: 1)`, a `pathlib.Path` as a path, `json_parse("")` and a lone surrogate, `nope.v2` suggesting `tour.v2`, an error at the end of the file. Docs: README conda, the loop row, keyword argument names, cmd's quotes, all of stdin and bytes to stdout, Node.js on Linux from the not-found entry, npm in WSL; DOCS keys and names, a block's own working directory, `e.data`, typed arrays. Not changed: `to_v2` of a frame far over 64 MiB still converts before it refuses (minor), and block output to `/dev/null` is slower than to a pipe (nit). Windows: 262 unit, 361 `.v2`, 96 CLI tests; Linux: 260, 365, 104 (the `.v2` tests also on Node 22.13); macOS: 259, 365, 100; clippy clean on all three |
| 2026-11-15 | Pre-publish, newcomer round 11: two new agents (another fresh Ubuntu 24.04 distro, root only, as a system administrator: cron-style runs, signals, a monorepo, the usual tools; a Windows sandbox named `r11 OneDrive - Jörg`, as a beginner with Notepad, pasted examples and first mistakes). Two major findings, no crash or internal error. Fixed with tests: an interrupted run exited 130 like any program, so one Ctrl+C didn't stop a shell loop of v2 runs and SIGTERM looked like a failure (on Linux and macOS v2 now ends by the signal it got once it has cleaned up); `@import { add } from js.helpers` with the script's own helpers.js said `npm install helpers` (now the file's path form, and never `npm install x.js`); `install.sh` run again in a terminal that had `~/.v2/bin` on PATH didn't restore deleted startup lines; a failed definition block reached again raised its first error instead of DOCS 4.3's `ImportError … failed to load`; `to_v2` of a big DataFrame took gigabytes before refusing (now refused by its size first); `venv = "~/x"` with HOME unset meant a folder named `~`; and messages and hints for a function a block defines but doesn't export, a local `statistics.py` hiding Python's own, a `v2.toml` section and key on one line or a key outside its section, unterminated strings (a curly quote, an apostrophe, a path's `\"`), text from `input()` used as a number, V2's builtins used in a block, a venv whose Python was uninstalled, a non-UTF-8 name in `list_dir`, `input()` at its end, a float index, `args()[0]` with none given, capitalized keywords, tuples, comprehensions, `let a, b = …`, `return 1, 2`, `let if`, a bare `{ }` block, Markdown fence lines, Ctrl+Z, Notepad's hello.txt, a block closed on its last code line, a missing npm package for a shared module, doctor under cron's PATH, a Latin-1 file on Linux (`iconv`). Docs: README two-line v2.toml examples, `js.helpers` means a package, `range()`, `int(input())`, a block ending in a statement, programs started by exec have no terminal, background jobs, `sh -c` with arguments, the exit status; DOCS symlinks and `script_dir()`, `write_file` isn't atomic. Not changed: `#!` lines (not decided yet), one npm package loaded per run across projects (DOCS 7), `exit(256)` is 0 on Unix (its low 8 bits, as documented). Windows: 264 unit, 364 `.v2`, 97 CLI tests; Linux: 262, 368, 105 (the `.v2` tests also on Node 22.13; installer tests on Linux and macOS); macOS: 261, 368, 101; clippy clean on all three |
| 2026-11-15 | Pre-publish, newcomer round 12: two new agents (another fresh Ubuntu 24.04 distro, root only, as a data engineer: an ETL pipeline split into modules over 3 million rows, with a Python generator, TypeScript validation, sqlite and a local HTTP API; a Windows sandbox named `r12 web dev (Zoë)`, as a web developer: an HTTP server in a `@js` block, fetch, TypeScript, ESM and CommonJS packages, npm scripts through exec). Two major findings, both docs, no crash or internal error. Fixed with tests: a `@js` server whose handler called V2 after the call returned said only "called outside an active V2 call", with a doubled `Error:`, once per request (now why and what to do, once, then one note); a local ES module under the `"type": "commonjs"` that `npm init` writes gave a bare `Unexpected token 'export'` (now a hint naming that package.json); on Windows `input()` from a pipe was taken for a file; `V2Error.stack` in JavaScript and its Python traceback showed the adapter's temp files (now its V2 type, where V2 raised it and the caller's frames); a datetime, Date or Decimal handle passed on said to use `to_v2`, which can't convert them (now the block that can); `apt install` advice for root without `apt update`; list_dir's hint for a non-UTF-8 name, whose code didn't run; and hints for `js.util` with a util.js next to the script, `ts."./money"` without its extension, a package's object imported in a block, `exec(..., env:/cwd:)`, `/dev/stdin` on Windows, a tool in `node_modules/.bin`, an async generator in `for`, `if (x) print(x)`, `await`, `${x:>5}`, UTF-16 without a BOM, a `.py` helper in a subfolder, a JavaScript line in a block that continues the one above, `v2 doctr`. Docs: README async FAQ (a V2 function JavaScript calls can't wait on async JavaScript), what Ctrl+C or a kill loses (buffered file writes, `atexit`; `exit(n)` ends engines cleanly), a handle is never `==` a number (a Decimal from a database), dotenv and `os.environ` stay in their language, cmd's `set "X=1"`, `"type": "module"` for your own ES modules, `py.stages.clean`, `apt update` on a fresh system, Node's PATH line in `~/.bashrc` too; DOCS 17.6 an HTTP server in a block, a timeout you can retry. Not changed: `handle == 0` stays `false` (DOCS 3.4: values of different types are simply not equal). Windows: 264 unit, 371 `.v2`, 98 CLI tests; Linux: 262, 374, 106 (the `.v2` tests also on Node 22.13); macOS: 261, 374, 102; clippy clean on all three |
| 2026-11-15 | Pre-publish, newcomer round 13: two new agents (another fresh Ubuntu 24.04 distro, root only, as a backend developer gluing Python and TypeScript: objects and callbacks both ways, long-held handles, memory over long loops, errors across three languages, the nesting limits, every "Not built yet" feature; a Windows sandbox named `r13 Klasse 7b – Übungen`, as a teacher preparing beginner exercises: 110 mistake scripts, German text, a folder of submissions checked in loops). Two major findings, both from the Windows agent; the Linux one found none, and neither met a crash or internal error. Fixed with tests: "this expression's value is not used" now hints by the line's shape (a function's last line: return; a bare name: call it; `i + 1`: `i += 1`) instead of "assign it with let", which in a loop gave a `let i = i + 1` that never ends; such a `let` in a loop body for a name declared outside it is now a static error, so `v2 check` finds it; a name used after the loop, if or function that declared it says so instead of "did you mean 'sum'?"; a null operand or index says where the null came from (a function without return, the end of input, `sort()`); `int()` and `float()` of "13.5", "3,7", "13 Jahre", "", "0x1F" and null; `for i in range(n)` makes no list, and `range()` lists and `*` repetition past 100,000,000 items or 1 GiB are refused instead of taking the machine's memory; a `:` in a Windows file name, which wrote a hidden stream; a user's random.py hiding Python's own module in a block's import; a Python object returned to JavaScript said to use `to_v2`; and hints for `1.000.000`, `2,50`, `×`, `÷`, `elif` or `else` without `if`, a second `else`, `while {`, `func hallo {`, `quadrat(x) {`, names with a space or `-`, `for i in 10`, `xs + 3`, `"ab" * 2.5`, `xs.slice`, a dict's missing key or method, mixed tabs and spaces (now said in words), `exec("dir")`, `v2 check -entwurf.v2` as PowerShell splits it, a `.py` helper in a folder above, did-you-mean ties (`stats`, not `star`), doctor notes for custom engine tables, Node's duplicate ES-module warning, the joined-line hint said twice, install.sh's "the downloaded v2" for a local archive. Docs: README troubleshooting for comparing `input()` with a number, random numbers, a time limit for a program, checking a folder of scripts, where DOCS.md is, npm 11's `"commonjs"`; DOCS string order, a JavaScript caller with fewer arguments, the loop `let` rule. Not changed: `v2 check` still doesn't look for undeclared names (README's known limitation; a static NameError would also stop a script whose typo sits in a branch never taken: not decided yet). Windows: 264 unit, 377 `.v2`, 100 CLI tests; Linux: 262, 380, 108 (the `.v2` tests also on Node 22.13; installer tests on Linux and macOS); macOS: 261, 380, 104; clippy clean on all three |
| 2026-11-15 | Pre-publish, newcomer round 14: two new agents (another fresh Ubuntu 24.04 distro, root only, as an operations engineer on locked-down machines: C and Latin-1 locales, HOME unset or read-only, noexec and full temp folders, `ulimit -v`, `-n` and `-f`, no terminal, a read-only project; a Windows sandbox named `r14 Ömer's Hausaufgaben`, as a 14-year-old beginner writing a quiz, a guessing game, a shopping list and a text adventure, fixing each mistake only from the messages). One major finding, from the Linux agent; none from the beginner; no crash or internal error. Fixed with tests: `install.sh` on a server whose /tmp is mounted noexec said to build from source (it now runs the copy it puts in the install folder, and names an install folder mounted noexec); under `ulimit -v` the 256 MiB interpreter stack couldn't be reserved, so even hello.v2 failed (64 and 16 MiB are tried); a write past `ulimit -f` killed v2 with SIGXFSZ (now an IOError); a menu loop that never checks `input()` for null spun forever once the input ended (the 1000th null in a row is an IOError); a constant imported into a `@js` block before its block ran printed V2's own code; `NODE_OPTIONS=--experimental-default-type=module` broke the JS worker (now `js.cjs`); `PYTHONINSPECT` ended runs with a traceback; and hints for a null from `getenv`, `.get()`, a block or a builtin, a missing key or index, `random`, `randint`, `math`, `sqrt`, `datetime` and `time.time()`, short names two edits away (no longer suggested), the file's own names declared further on, a `}` missing before `else`, a script name with a space, `js.@scope/name` unquoted, `@import *`, `write_file` of a list, Python's string methods, a string's character assigned, the exact pip command for a venv, a folder with only hello.v2.txt, a deleted current folder, a full TMPDIR, TMPDIR on a Windows drive in WSL, a python3 that never answers `--version`, too many open files, a decorator in the middle of a `@ts` line, install.sh's wording for unwritable startup files. Docs: README for locked-down machines (noexec, memory limits) and UTF-8 whatever LANG says; DOCS `startup_timeout = 0`. Windows: 264 unit, 383 `.v2`, 101 CLI tests; Linux: 262, 386, 113 (the `.v2` tests also on Node 22.13; installer tests on Linux, the noexec ones as root in a throwaway distro, and on macOS); macOS: 261, 386, 106; clippy clean on all three |
| 2026-11-15 | Pre-publish, newcomer round 15: two new agents (another fresh Ubuntu 24.04 distro, root only, as a meticulous reviewer testing every sentence, example, quoted message, limit and default of README.md and DOCS.md; a Windows sandbox named `r15 دفتر İstanbul 📊`, as a data journalist cleaning CSV and Excel files in Turkish, Arabic, Hebrew, Chinese and emoji, comparing text handling across V2, Python and JavaScript, and charting with matplotlib). One major finding, from the Windows agent: `@js { {şehir: 1} }`, an object literal with a non-ASCII key, silently gave the value instead of the documented error (any JavaScript identifier now counts, and a block whose last line is one, after a statement, is refused too); no crash or internal error. Fixed with tests: a signal that lands as `exec` or a worker's answer returns (`kill $PPID`, `os.kill(os.getppid(), SIGTERM)`) let the next statements run, a `write_file` too (checks after each, and on Unix only the interpreter thread takes the interrupt signals, so the handler has run before the call returns: without that, 3 runs in 40 under load printed the next line); under `ulimit -v` a 256 MiB stack left too little heap, so deep recursion ended in an abort (`memory allocation of 104 bytes failed`, exit 134): no stack above a quarter of the limit, and the depth error says a memory limit shortened it; a Python warning about a call from V2 named the adapter's temp file; a `@py` block used as a value whose last statement is a `with`, `for` or `if` gave null; a `math.py` (or another module built into Python) next to the script was silently ignored; `for a, b in c.most_common()` gave index and pair; `lines()` didn't split an old Mac file's lone `\r`; lists showed no-break and other spaces as plain spaces (now `\u{a0}`), and the error caret was off after a zero-width joiner; `exists("~/x")` was silently false (now refused as in the other file functions), `make_dir("")` made nothing silently, NULs in `exec`'s argv and `getenv`'s name gave raw messages; `getpass` in a Unix block warned about echo and printed its prompt before failing; install.sh showed the shell's raw `Permission denied` before its noexec line; and hints for `int()`/`float()` of `1_000`, `1 000`, `1.234,56`, `1.000.000` and non-ASCII digits, the code page of the Windows that saved a file (`cp1254` on a Turkish one, not always cp1252), `df.loc[r, "col"]` in V2, a script in the current folder that isn't marked as a program, a probe timeout under a `startup_timeout` below 10 s, `v2 check`'s note on a file with a syntax error. Docs: DOCS what a character is (code points, no normalization, JavaScript's UTF-16 lengths, case and order without a locale), small floats in exponent form, custom-engine tables not checked yet (Phase 3), `exists` and empty paths; README a memory-limit entry of its own with measured numbers, a multiprocessing `Pool` that hangs, code pages, the Windows venv `v2.toml`. Round 14's CI failed on Windows: two config unit tests shared a temp folder name and raced (renamed). Not changed: a closed stdout (`>&-`) is `/dev/null` to Rust's runtime, as to Python, so output is dropped with exit 0; a missing name of a Python module keeps the documented wording. Windows: 264 unit, 390 `.v2`, 101 CLI tests; Linux: 262, 394, 115 (the `.v2` tests also on Node 22.13; installer tests on Linux, the noexec ones as root in a throwaway distro, and on macOS); macOS: 261, 394, 108; clippy clean on all three |
| 2026-11-15 | Pre-publish, newcomer round 16, the last (later rounds found mostly polish, about one major each): two new agents (another fresh Ubuntu 24.04 distro, root only, as a DevOps engineer: pipelines between v2 runs and other tools, a 200 MB file and a million lines through `input()`, exit codes under `set -e`, `env -i`, 50 runs at once, `timeout`, stop and resume; a Windows sandbox named `r16 Mods [beta] 100% Ελλάδα`, as a game modder passing functions, classes, generators, async functions, errors, dates, big ints and nested data between Python, JavaScript and TypeScript, with ESM-only npm packages, Express and a local HTTP server). One major finding, from the Linux agent: `read_file("/dev/stdin")` after `input()` reopened the path, so from a pipe it lost what `input()` had buffered and a redirected file was read again from its start (it now reads V2's own stdin on). No crash or internal error. Fixed with tests: V2 functions in JavaScript had `length` 0 and in Python the signature `(*args, **kwargs)`, so Express never called a V2 error handler and `curry` called at once (the `fn` tag now carries the parameters); glibc's per-thread malloc arenas used up a `ulimit -v` limit (CI's red round-15 run: one arena under a limit, so even 30 MB ends deep recursion in V2's error); install.ps1 run as a file from a folder with `[ ]` resolved relative paths against `$PSHOME`; a script in a folder whose name isn't UTF-8 ran with a `script_dir()` that doesn't exist; and hints for a big int passed to another language (to_v2 can't take it either: pass it as text), TypeScript's `<T>x` cast, an ES module importing from a sibling file that package.json makes CommonJS, a V2 function or an imported name used in a block, a block above its `@import` but in an `if` that didn't run, a script name split at several spaces, `exec` with PATH unset or `python` where only `python3` is, an engine not found with PATH unset (no install line), Node.js crashing at startup under `ulimit -v`, a venv naming a file that isn't a Python, "an `if` statement". Docs: a block ending in `with`/`for`/`if`/`try` whose value is used is an error (README, DOCS 4.5), `read_file("/dev/stdin")` reads the rest of stdin, `exec` and batch files with `%`, `int`/`float` and spaces, exponents, `inf`, `nan`, `v2 doctor`'s exit code, a CI-safe `v2 check` loop, `mkdir -p`, Node.js's tar.xz from a terminal, async generators in `for` (V2's hint already said how; DOCS didn't), SIGKILL of v2 leaving what its children started. Not changed: error positions for a failure inside a file a block loads still point at the block (a nit); a closed stdout stays as `/dev/null`. Windows: 265 unit, 394 `.v2`, 101 CLI tests, and install.ps1's path tests; Linux: 263, 398, 118 (the `.v2` tests on Node 22.13; installer tests); macOS: 262, 398, 110, and installer tests; clippy clean on all three |
| 2026-11-16 | Pre-publish, newcomer round 17 (the rounds continued), run as workflows: two new agents (another fresh Ubuntu 24.04 distro, root only, as a security-minded reviewer feeding hostile input: huge and deep JSON, malformed UTF-8, a 100 MB line, file names with control characters, links out of the project, planted v2.toml, package.json and modules in parent folders, odd PATH entries and language variables; a Windows sandbox named `r17 Lab Ω (Δt=0.5)`, as a researcher automating experiments: numbers across the three languages, dates, seeds, CSV/JSON results, numpy and scipy, Ctrl+C mid-run), then refuters that reproduced each of the 27 findings with the error's hint printed and checked the docs (19 confirmed, 6 partly, 2 by design). One major: an imported .v2 file that is a FIFO hung `v2 check` and runs, and one linked to /dev/zero ate memory, though DOCS calls `v2 check` safe for code you don't trust (now an ImportError, as for the main file). The other reported major, a v2.toml in a shared parent folder trusted silently, is the documented trust model (INTERNALS 18.1, 20.1): DOCS 16 now says which nearby files a run trusts. No crash or internal error. Fixed with tests, by four implementers in their own worktrees, each reviewed, then the reviews' issues fixed and reviewed again: an import of a link loop or a dangling link or of /dev/stdin from a pipe said 'cannot find module'; control characters in file names reached the terminal raw (every path V2 prints of its own files is escaped now, the v2.toml, venv and program paths in messages and v2 doctor too); `~root/x` made a folder named `~root` (a `~name` is refused on Linux and macOS when that user exists, as bash expands it; Word's `~WRL0001.tmp` is a name); exec's output and read_file had no limit (1 GiB, then an IOError); an uncaught error was glued to an unfinished output line; a block arg Python reads under another name (NFKC: the `ﬁ` ligature, the micro sign) wasn't reachable, and reserved names got no next step; V2 functions called from a Python thread, to_v2 of a container of dates or a date-indexed pandas Series, `**opts` and dict merges in Python's and JavaScript's spellings, `@export` renames and `$x` imports, `int()` of a big int, a BOM before JSON (now skipped), NaN in json_stringify, `${x:.3e}` and other formats, a file Excel holds open, install.sh's and install.ps1's paste-in PATH commands for folder names with `$` or a backtick; an ES module a block requires was placed at the block's first line. Docs: what a run trusts, v2.toml links, JSON nesting and list_dir limits, the doctor link for Node.js, `**opts` and dict merges in the Python/JavaScript table, round's range, a @js action block's cost in a loop, names before their let. Left: a pandas Series with duplicate dates still names a block that fails; `a$b` quotes only its tail. Windows: 272 unit, 418 `.v2`, 107 CLI tests, and install.ps1's 13 path cases; Linux: 270, 422, 127 (the `.v2` tests on Node 22.13; installer tests as a user and, the noexec ones, as root in a throwaway distro); macOS: 269, 422, 119, and installer tests; clippy clean on all three |
| 2026-11-16 | Pre-publish, newcomer round 18, run as workflows: two new agents (another fresh Ubuntu 24.04 distro, root only, as the maintainer of a Python and TypeScript codebase bringing V2 in: a src/ package with subpackages and an editable install, npm workspaces, tsconfig paths, ESM and CommonJS side by side, .v2 files in several folders and projects; a Windows sandbox named `r18 git & bash — Čeština`, as a developer who lives in Git Bash and cmd.exe: MSYS paths, CRLF scripts, exec of .sh and .bat files and of Git Bash's tools, Ctrl+C and pipes in each shell), then refuters that reproduced each of the 22 findings with the error's hint printed and checked the docs (15 confirmed, 6 partly, 1 by design: a misspelled module gets no did-you-mean against installed ones). One major: exec's arguments were re-read by Git Bash's own programs (find, grep, printf, sh), which globbed `*.v2`, expanded `{a,b}` and `~`, read `@file` and ate quotes (each argument now goes on the command line so both the C runtime and MSYS read it back as given; batch files and cmd.exe keep their quoting). No crash or internal error. Fixed with tests, by three implementers in their own worktrees, each reviewed, then the reviews' issues fixed and reviewed again, then the second reviews' issues: a .v2 module of another project was told to make a v2.toml it had (one project serves the run, now said in the README and DOCS 7 and 14, and in the hint); a NAME.py in a folder above, a linked helpers.py, `py."../x/core.py"`, a folder below named `2024`, `@import` from a V2 module's alias, at V2 level and in a block; doctor said `ready` for a Python a broken PYTHONHOME or a NODE_OPTIONS stops (it starts the program when one is set); a dual package's names only in its import build, .jsx and .tsx files, a second copy of a package in another folder, Yarn Plug'n'Play, tsconfig paths, TypeScript that is code (enum, namespace, parameter properties, `import =`, `export =`, `<T>x`, Node 24's `module` keyword), an interface imported as a value; on Windows the .sh hint named a `bash` on PATH (WSL's, not Git Bash), `/c/Users/…` paths got a bare 'there is no folder', names Windows refuses (devices, a trailing dot) the raw OS text, and V2_ENGINE_PY=python3 that is the Store placeholder said to install Python. Docs: Git Bash keeps only an exit code's low 8 bits, which backslashes MSYS still halves, the install line for cmd.exe and Git Bash. Windows: 282 unit, 425 `.v2`, 116 CLI tests, and install.ps1's 14 path cases; Linux: 280, 429, 134 (the `.v2` tests on Node 22.13; installer tests as a user and, the noexec ones, as root in a throwaway distro); macOS: 279, 429, 126, and installer tests; clippy clean on all three |
| 2026-11-17 | Pre-publish, newcomer round 19, run as workflows: two new agents (another fresh Ubuntu 24.04 distro, root only, as a developer whose Python and Node.js come from version managers: uv's Pythons and venvs, pyenv and its shims, Miniforge with an activated environment, nvm, asdf and fnm, per-folder versions, versions switched and removed between runs; a Windows sandbox named `r19 Maître ~ files#1 (copy)`, as a power user in the file system's corners: junctions and symlinks, hard links, read-only, hidden and system files, 8.3 names, names that differ in case or end in a dot or a space, long paths, files another program holds open, a folder V2 may not read, PowerShell 5.1's encodings), then refuters that reproduced each of the 32 findings with the error's hint printed and checked the docs (25 confirmed, 5 partly, 2 already handled). Two majors: on Windows `remove_file("data.txt.")` deleted data.txt and `read_file("CON")` waited forever, as Windows drops a trailing dot or space and makes device names devices (every file builtin now refuses those names before the OS sees them, with the `\\?\` form for one that is really there); a venv whose python now runs another Python than it was made with (after an upgrade) was `ok` while every import of its packages failed (now `broken`, with how to make it again, uv's way for uv's venvs). No crash or internal error. Fixed with tests, by four implementers in their own worktrees, each reviewed, then the reviews' issues fixed and reviewed again, then the second reviews' issues, reviewed once more: install lines that can't work (uv's venvs have no pip, `pyenv exec python`, `uv run python`, a venv without pip whose Python has no ensurepip, an activated venv V2 uses), an activated venv whose python is gone or doesn't run, a venv launcher's stale probe, a conda environment or a Python installation named as the venv on Linux and macOS, a damaged venv, FROM for a broken venv; `conda run` without --no-capture-output (a doctor note too; js on Unix: its two pipes), a `~` entry on PATH, doctor's start check for a stale PYTHONPATH and under `ulimit -v`, a block's require() of an own file, a tsconfig alias or a misspelled package, a package named like chart.js; drives that aren't there or can't be reached, /tmp on Windows, read-only files, UTF-16 and UTF-32 files, $HOME on Windows; links that lead nowhere or in a loop (imports, `v2 check`, the CLI and the file builtins, with what fixes them), a module V2 may not read, a file another program holds open, Git Bash paths and `*` in cmd and PowerShell, TOML escapes in v2.toml paths, doctor's folder through a link, one file reached by two paths (by its identity, which a reused inode can't fool), import case on Windows and macOS, a ';' in install.ps1's folder and a ':' in install.sh's. Docs: uv in the README, version managers' shims, `func(x) => x * 2`, the folders every user can write to on Windows. Left: Python's UTF-16 hint can't tell UTF-32 LE apart; doctor keeps a `conda run` py row `ok`, with the note. Windows: 309 unit, 428 `.v2`, 138 CLI tests, and install.ps1's 15 path cases; Linux: 309, 432, 152 (the `.v2` tests on Node 22.13; installer tests as a user and, the noexec ones, as root in a throwaway distro); macOS: 308, 432, 143, and installer tests; clippy clean on all three. CI then found that newer Rust's create_dir_all quietly succeeds at a drive that isn't there (`make_dir("Z:/")` now checks the folder is there), and two tests that pinned Rust's and Python's own wording |
| 2026-11-17 | Pre-publish, newcomer round 20, run as workflows: two new agents (another fresh Ubuntu 24.04 distro, root only, as a developer who lives in a language other than English: Turkish, German and Japanese locales, UTF-8 and legacy, set and mixed, the Turkish i, decimal commas, Intl and Python's locale module in blocks, localized messages from exec, Japanese and emoji in source, names, file names, arguments and input(); a Windows sandbox named `r20 {Ñ} C++ ⚙ runner`, as a developer who runs V2 from other programs: Python's subprocess, Node's child_process, PowerShell and cmd, no console, closed stdin, redirected output, the caller killing v2 mid-run, many runs at once), then refuters that reproduced each of the 25 findings with the error's hint printed and checked the docs (17 confirmed, 8 partly). One major: runs started at the same moment deleted each other's private temp folder, so 2 to 5% of 8 runs at once failed with EngineCrashed (a folder now counts as a reused PID's only when its process started seconds after it, and a folder changed in the last seconds is never removed). No crash or internal error. Fixed with tests, by four implementers in their own worktrees, each reviewed, then three more passes on the reviews' issues, each reviewed, then the last reviews' issues: hints for numbers written in other locales (points, spaces, NBSP, NNBSP and apostrophes between thousands, U+2212, Arabic and other digits with their marks; each hint followed in Python), the iconv line's code page from the locale and the file's own path, euc-jp, UTF-16 only with its byte-order mark, decode() and encode() given an encoding, the decimal comma wherever it stops parsing and thousands commas (`1,000,000`, `1,234,567.89`), full-width characters and U+3000 from an input method (in directives too), bidi characters escaped in printed names, v2.toml bare keys with `ı` or `ö`, a Mac's decomposed file and folder names (the CLI, imports, the file builtins), did-you-mean across diacritics and case (a missing import on Linux too), @export of a full-width name, exec's argv[0] as written, `v2 CHECK`, pip's package names (`yaml` → PyYAML), `~` in V2_ENGINE_*, no TEMP in a caller's environment (doctor too), exit() taking Windows' unsigned exit codes, NODE_OPTIONS in a js crash, input()'s hint for a program feeding v2, the installer's checks under a German, Turkish or Japanese locale, single quotes for pasted paths with `$` or a backtick. Docs: v2's standard streams are UTF-8 on every OS, a language's own case and number formats through a block, list_dir's order, /tmp where `C:\tmp` exists. The Mac's Node writes de-CH with an ASCII apostrophe, newer ICU data than CI's U+2019: the test takes either. Windows: 322 unit, 440 `.v2`, 146 CLI tests, and install.ps1's 15 path cases; Linux: 322, 442, 164 (the `.v2` tests on Node 22.13; installer tests as a user and, the noexec ones, as root in a throwaway distro, under a German locale too); macOS: 321, 442, 155, and installer tests; clippy clean on all three |
| 2026-11-17 | Pre-publish, newcomer round 21 (run with agents, as workflows were off for the session): two new agents (another fresh Ubuntu 24.04 distro, root only, as a developer installing Python and Node.js packages every way a modern Linux offers: pip into the system Python and PEP 668, --user and --break-system-packages, apt's python3-* packages, pipx, uv, conda, editable installs, namespace packages, compiled extensions, npm -g, npm link, pnpm and Yarn Plug'n'Play; a Windows sandbox named `r21 wsl-bridge (Ľudo's)`, as a developer whose code lives in WSL, running the Windows v2 on `\\wsl.localhost\…` with a throwaway distro of its own), then refuters that reproduced each of the 23 findings with the error's hint printed and checked the docs (15 confirmed, 6 partly, 2 already handled). Four majors, all on Windows: write_file on a `\\server\share\…` path took `log 10:30.txt` as a file `log 10` with a hidden stream (the `:` check now runs after a share as after a drive); from a WSL folder, exec of cmd or a .bat ran in C:\Windows and reported success (now refused, unless the line first moves to an absolute folder with pushd); a venv made in WSL was told `py -m venv --clear`, which half-deleted it (a venv the other system made is now kept, and a venv beside it for this system named in V2_ENGINE_PY); every Linux symlink seen through `\\wsl.localhost` was called broken, with advice to delete it (now named a Linux (WSL) symlink, which Windows can't follow). No crash or internal error. Fixed with tests, by four implementers in their own worktrees, each reviewed and the reviews' issues fixed, then a second review of the merged whole and its issues: WSL's names with `:` or `?` (its private-use stand-ins) reached by imports and FILE, shown escaped; a name ending in a dot or a space in imports and FILE; a script run by its `\\?\` path, whose `/` the file builtins and exec now read; `/mnt/c/…` as Git Bash's `/c/…`, other Linux paths and programs named, make_dir of `/root/…` refused; one module through `\\wsl$` and `\\wsl.localhost`; a WSL distribution that isn't there; WSL's pipes and devices refused unread (they hung 10 s, and a /dev/zero node read without end); a Linux interpreter configured on Windows; a .bat engine, or a .bat found on PATH, in a share folder; doctor broken for `conda run` without --no-capture-output; a pip name used as the module's (`pyyaml` → `yaml`, namespace packages and editable installs too); a third loop name, `*args`, `catch (e: Type)`; whole lines from threads of a forked worker; a missing system library; the fileno message; an ArityError from a callback; an activated conda environment's install line. Docs: code in WSL (which v2 to use), apt's python3-<name> packages, pip names, os.walk's block form, devices on a share. Windows: 339 unit, 443 `.v2`, 162 CLI tests (the WSL-share tests with V2_TEST_WSL_DISTRO set); Linux: 334, 447, 165 (the `.v2` tests on Node 22.13; installer tests as a user); macOS: 333, 447, 156, and installer tests; clippy clean on all three |
| 2026-11-17 | Pre-publish, newcomer round 22 (run with agents): two new agents (another fresh Ubuntu 24.04 distro, root only, as a developer whose scripts draw on the terminal: progress bars and spinners from blocks, colors with and without NO_COLOR, FORCE_COLOR and TERM=dumb, isatty checks, the terminal's width, input() and getpass, curses, Ctrl+C in the middle of a bar, on a pseudo-terminal and piped; a Windows sandbox named `r22 portable =py+node= [x64]`, as a developer using portable runtimes: python.org's embeddable zip, a second Python, Node.js's zip, a venv made from a portable Python, runtimes moved or deleted), then refuters that reproduced each of the 16 findings with the error's hint printed and checked the docs (all 16 real, 4 smaller than reported). Six majors: blocks decided whether they had a terminal from the worker's own descriptors (V2's stderr), so chalk wrote escape codes into `v2 x.v2 > out.txt`, rich, ora and cli-progress drew nothing live on a real terminal, and the docs said neither (decided on 2026-11-17: blocks see V2's terminal: `init` tells each worker which of V2's stdout and stderr are terminals and their size, and Python's isatty and terminal sizes and Node's isTTY, columns, rows and tty.WriteStream's methods answer for them; on Windows v2 turns on VT processing for its consoles and puts their modes back at every exit); a script couldn't ask for a password, as getpass fails in blocks and its message pointed to an input() that echoes (decided on 2026-11-17: `input(prompt, echo: false)`, echo off on a terminal until the line ends, back on Ctrl+C and while Ctrl+Z has v2 stopped); the README's line for a job that outlives V2 held exec until the job ended (now `setsid -w sh -c "job … &"`, which also keeps the job under load); a venv whose portable base Python was moved had its cause cut mid-path and a fix that used `py`, another Python (the missing Python is named from pyvenv.cfg's home, and every remake line uses the venv's own base, py only when PATH has it). No crash or internal error. Fixed with tests, by three implementers in their own worktrees, each reviewed and the reviews' issues fixed: Ctrl+C gives the terminal back (the settings saved at startup, the cursor shown) when v2's stdout is a terminal or v2 itself changed it, so a pager keeps its mode; the cursor also comes back after exit(n), an uncaught error and a closed pipe; a closed stderr ends the run with 141 as stdout's does; colorama on Windows passes a block's escape codes through; shutil is changed at its first import, not at startup (the first @py block 2 ms faster on Linux, 8 on Windows); the fileno message gives subprocess's advice only to subprocess and os, and tells curses that a block has no terminal; python.org's embeddable package (no pip, no venv, its ._pth decides sys.path) is named in hints and doctor, with virtualenv where it has it; a missing standard module (tkinter, a module only another system's Python has) gets "pip can't add it"; PYTHONPATH, PYTHONHOME and NODE_OPTIONS shown whole; V2_ENGINE_PY over a project's venv gets a doctor note and an ImportError hint; npm.cmd as V2_ENGINE_JS is named npm. Docs: blocks and the terminal, the order of a block's low-level output, the embeddable zip, input's echo. The terminal tests read what v2 draws until it has ended, and the terminal's settings through the master, as on macOS the exit of a session leader waits until its output is read and revokes the slave. Windows: 351 unit, 448 `.v2`, 169 CLI tests (the WSL-share tests with V2_TEST_WSL_DISTRO set); Linux: 346, 453, 177 (the `.v2` tests on Node 22.13; installer tests as a user, and as root with the noexec mounts); macOS: 345, 453, 167, and installer tests; clippy clean on all three |
| 2026-11-19 | Pre-publish, newcomer round 23 (run with agents): two new agents (another fresh Ubuntu 24.04 distro, root only, as a developer doing heavy and parallel work across languages: multiprocessing pools with fork, spawn and forkserver, concurrent.futures, Python threads and Node worker_threads and child_process.fork calling back, async on both sides, 100 MB values, 100,000 calls, a run of minutes, call timeouts; a Windows sandbox named `r23 busy files ‹Łódź›`, as a developer automating file chores on a busy machine: files other programs hold open, lock in part or map, attributes, a 2.5 GB file, 100,000 files, junctions, long names, Windows' own tools through exec), then refuters that reproduced all 27 findings. Two majors: write_file over another program's byte-range lock emptied the file, then said it couldn't write; exit(n) inside a call from Python lost Python's buffered file data, though the README said open files are flushed. Fixed with tests: write_file on Windows opens a file without cutting it and holds a lock of the whole of it through the cut and the write (another program's lock, or a mapping, refuses it with the file whole; a review showed a lock only looked for let a program that locks on and off cut it); a pipe or a mailslot is written once; read_file and input() tell UTF-8 cut at its end (a log still being written) from a code page; remove_file removes a junction or a link to a folder, not what it points to (a dead or read-only one too), and refuses `f.txt/` on every OS (Windows deleted f.txt); names over 255 UTF-16 units, a mapped file, a running .exe, a held v2.toml and the 1 GiB limit get hints, and the busy-file hint fits the verb; exec reads Windows tools' output a line at a time in a single-byte code page (attrib's ANSI names at a UTF-8 console; a UTF-8 line next to them stays) and whole in a double-byte one; a command line over 32,766 characters and a program at a long path (started by its folder's 8.3 path when it has one, a batch file from 247) are said as such; a class or a function imported as a module is named so (Python, and Node's built-ins); Python's open files are flushed after exit(n) inside a call (JavaScript's pending stream writes are lost there, as with process.exit(): the README says so); a JavaScript block's stdin listener fails at once; a table far over 64 MiB is refused before converting it; the forked-process, nested event loop and route messages; a low ulimit -v is no longer called a missing library (and is named only when below what the engine needs; doctor and a run agree); a fence's closing line with more after it is pointed at; a note when a worker still running 1 s after the end is stopped; a waited import's error shows the line that used it. Docs: renaming over an open file on Windows, sizes and times, call_timeout, async calls across languages, pool callbacks, raw fd writes, fenced blocks in expressions. Four implementers in their own worktrees, each reviewed (files and exec twice); the round's agents were stopped on 2026-11-18, and the last review fixes, the merge (one cut_utf8 for read_file and exec) and the checks were done without agents; the workers' change had no review. Windows: 356 unit, 460 `.v2`, 185 CLI tests (the WSL-share tests with V2_TEST_WSL_DISTRO set); Linux: 350, 466, 184 (the `.v2` tests on Node 22.13; installer tests); macOS: 349, 466, 173, and installer tests; clippy clean on all three |
| 2026-11-19 | 1.15, the interpreter's speed (before the compiler phase): frames reused, loop iterations sharing one frame when nothing can keep it, arguments bound without the binder when nothing can go wrong, ints read in place, guard clauses and tail returns, cached method lookups, no formatting on the usual paths. Instructions against the baseline: while -60%, for -55%, calls -50%, lists -45%, fib -42%, strings -41%, errors -40%, dicts -38%, closures -33%; wall time 1.4-3.6x faster on Linux and Windows (numbers in 1.15). Two regressions of its own were caught by wall time, not instruction counts: a copy of a string held through `s +=` made each append copy it (append_cost.v2 now times each kind of target), and copies of every loop variable (needed only for items whose release can be seen, now made only for those) cost a `for` over a range 25%. One review agent wrote programs from 5.1 and 5.6 and found four changes in the order of releases or the collector's pacing, all fixed with tests (1.15). tests/host/functions/fast_paths.v2, its output from the rules (the baseline binary gives it too); the baseline and new binaries agree on every test file and on 13,500 generated programs. Windows: 356 unit, 464 `.v2`, 185 CLI tests; Linux: 350, 470, 184; macOS: 349, 470, 173; clippy clean |
| 2026-11-19 | `v0.1.0-alpha.2` published: `v2-reboot` fast-forwarded into `main` (151 commits), the tag `v0.1.0-alpha.2` at a096a49 (its release run rebuilt the binaries from the tag), and the draft published as a pre-release with the binaries for Windows, Linux (glibc 2.35+) and macOS (Apple Silicon). CI green on the tag, `main` and `v2-reboot`. The README's one-liners run against it, Windows into a scratch folder without changing the PATH and Linux and macOS with a throwaway HOME: each installed 0.1.0-alpha.2 from the release and ran a program and a Python block |
