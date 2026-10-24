# Notes for Claude (working notes on the V2 reboot)

The owner allowed this file so I keep my bearings across sessions. Keep it short and current;
it is loaded automatically into every session and every subagent.

## What this is
- V2: a small glue language with a small standard library (Rust, tree-walking interpreter)
  whose point is the polyglot system: `@py`/`@js`/`@ts`/`@c`/`@cpp`/`@rust` blocks,
  `@export`/`@import`, VEP/1 workers, compiled blocks via libffi, `extern`, custom engines.
- **DOCS.md** = what the language does (user spec). **INTERNALS.md** = how (implementation spec,
  `§n.m` = its sections). **ROADMAP.md** = order + what's done ("Where we are", progress log).
- The docs were trimmed to the glue-language scope on 2026-10-16, then reviewed on 2026-10-24
  (43 owner-approved changes; ROADMAP progress log lists both). Don't bring cut features back
  without the owner's OK.
- **Never propose cutting how other languages get in**: built-in engines, custom engines (`run`
  and `bridged`), `extern`, compiled blocks, cross-language `@import`/`@export`, handles. That
  reach is the language (owner, 2026-10-24: "2 and 3 are the main base of the language").
  Bloat to trim is elsewhere: host-language extras, tooling, config, over-detailed spec text.
- `old/` = the archived previous implementation. Reference only; never build on it.
- Branch `v2-reboot`. Never push or merge to main unless the owner asks.
- Status: ROADMAP "Where we are". The owner gave the go-ahead on 2026-10-24 for 0.11 and Phase 1;
  build them in order, one milestone per commit (see "In flight").

## In flight (update at every step boundary; trim when a milestone closes)
- Plan: alignment commit (done 2026-10-24), then 0.11, then 1.4 → 1.10. Per milestone: read its
  § sections, ch. 21 rows and R-tests → build + tests → cargo test + clippy on Windows and WSL →
  run its DOCS examples → review workflow (finders + refuters) → fix with regression tests →
  check boxes, ROADMAP log, code map below → commit.
- Now: 1.5 (not started). A draft `js.js` sits in the session scratchpad (not in the repo); it
  was driven by hand over VEP (loads, values, BigInt, Promises, causes, ts stripping,
  callbacks). If the scratchpad is gone, write it again from 12.4, 12.5, 14.4 and py.py.
- Lessons from 1.4: the same bugs py.py had will apply to js.js (pin encoded stubs, re-encode
  error tags, roll back handle counts, never re-enter the send path, every request gets a reply).

## Working rules (from the owner / ROADMAP)
- Ultracode: workflows for substantive work; adversarial review per milestone (reviewers find,
  independent refuters verify); fix every confirmed finding with a regression test, and prove
  the test fails without the fix when it is cheap (swap the fix out, run, restore).
- Before checking off a phase: audit that every ch. 21 row / R-test of that phase has a test,
  and run the DOCS examples of that phase against the binary.
- A roadmap box is checked only when its tests pass. Docs are the spec: if code and docs
  disagree, fix one of them on purpose in the same change.
- Phases go in order; nothing from a later phase is stubbed in early.
- Commit per milestone; message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Verify end to end before claiming anything works.
- Owner: casual hobbyist, hates bloat, wants me to make the technical calls.

## Environment gotchas
- Windows 11, MSVC Rust 1.91. Bash tool is Git Bash: **heredocs (even quoted) and `sed`
  mangle backslashes** — for any edit containing `\` write a Python script with the Write tool
  and run it, or use the Edit tool.
- Linux check: WSL Ubuntu has cargo (nightly 1.93):
  `wsl.exe -e bash -lc "rsync -a --delete --exclude target --exclude old --exclude .git '/mnt/c/Users/SirOxygen/Desktop/My Stuff/v2-language/' ~/v2/ && cd ~/v2 && cargo test"`
  (the repo lives in `Desktop\My Stuff\v2-language`; quote the path, it has a space)
  (also `cargo clippy --all-targets` there: nightly has extra lints, and cfg(unix) code only
  compiles there).
- For review workflows, copy `target/debug/v2.exe` to the scratchpad and let agents use that
  snapshot (never cargo in the repo while agents run).
- `cargo test` = unit tests + `tests/run.rs` (the .v2 harness + CLI tests). Clippy must be clean.

## Code map (src/)
- `main.rs`: threads (interp thread 256 MiB stack), exit codes, panic hook → 101. Defines the
  output macros `stdout!`/`stdoutln!`/`stderr!`/`stderrln!` (ignore write errors; the stderr
  ones first call `stop_if_interrupted`). Never use `print!`/`eprint!` in src. Debug builds:
  `V2_TEST_PANIC` panics the interpreter thread (for the 101 test).
- `cli/`: argv parser (`v2 FILE`, `check FILE`, `doctor`, `cache clean` (parsed; main prints
  "not implemented yet" until 2.3), `--version`, `--help`; no other flags), HELP text.
  `config/`: the project's `v2.toml` read in file order, then
  `V2_ENGINE_<TAG>` overrides (canonical tags only; a replaced `command` is not validated);
  `command` must be an array; unknown sections/keys and keys that don't apply are config
  errors (no warnings). `load_from(start, env)` is the test seam. Custom engine tables kept
  raw until Phase 3 (17.1).
- `syntax/`: `span` (SourceMap, FileId, display_path), `lexer`, `scan` (block bodies, profiles,
  `canonical_tag`), `directives` (@export/@import, dedent), `parser` (MAX_DEPTH 256, expression
  height MAX_LINKS 10,000; anonymous functions are `func(…)`, there is no `lambda`), `check`
  (static checks, 4.5, incl. "this expression's value is not used"), `ast`.
- `runtime/`: `interp` (exec/eval take `&Env`; stacks: `calls` and `import_stack`; `guard()` at
  7/8 of the stack; MAX_CALLS 10,000; no FileScope/units/export registry yet; `call_value(f,
  pos, named, at)` calls any V2 value from Rust), `value` (`Callable` = Closure | Builtin),
  `env`, `call` (binder: `ParamSpec::req/opt/rest`, `bind_spec`), `ops`, `methods` (tables of
  `MethodDef`), `strings` (char cache), `builtins` (`BuiltinDef` table, `str_arg`/`bool_arg`; `Out`, the only stdout
  writer, `Out::capture()` in tests; `POSITIONAL_ONLY`), `error` (ErrorValue, Flow, `fail`,
  rendering, did_you_mean), `module` (import graph, `locate`, `shown`). A builtin returns
  `fail(..)`; the caller places it at the call site.
- `Flow { Error, Return, Break, Continue, Exit }`, `Exec<T> = Result<T, Flow>`. Interrupts are
  never a Flow.
- Deep values are freed iteratively (Drop impls feed a thread-local queue).
- `codec/json`: plain mode (serde_json, pre-scan for i64 overflow + nesting 1,000).
  `codec/vep`: the `$v2` wire codec (`Wire` context: session, tag, caps, release queue,
  callables, sm, `route` (always `|_| None` until 1.7)).
- `doctor/`: `v2 check` (parse + static checks of the file and its imports, explicit-stack walk;
  prints `check X: N files, M errors`) and `doctor::doctor` (the table).
- `engines/process.rs`: child rules (setsid/PDEATHSIG; suspended + Job Object), `ChildProc`,
  `KillHandle`, `run_captured(argv, env, limit, input)` (stdin null or fed by an unjoined thread;
  `SpawnError { message, reason }`; batch paths with `%` refused), the watchdog (only the top timer
  runs, not while paused; exit check every 250 ms; SIGCHLD reset),
  `INTERRUPTED`/`stop_if_interrupted`/`exit_interrupted` (kills trees, waits ≤ 500 ms, removes
  the run dir). Tests re-run the test binary as the child (`V2_HELPER`); **tests that push
  timers must hold `engines::process::tests::serial()`** (the watchdog is global).
- `engines/spec.rs` (py/js/ts table, `Version`, `too_old`: Python 3.10+, one Node rule for js
  and ts: 22.13+, not 23.0/23.1), `engines/resolve.rs` (`Lookup`, `find_program`,
  `Prober`/`RealProber`, `ProbeCache` (usable entries only) + `CacheMode`, `Resolver`,
  `ResolvedEngine`, `NotFound`), `engines/registry.rs` (`Registry::real`, `resolve_tag`,
  `engine_available`). A reached block resolves its tag, then `not_yet` until 1.4. Uncaught
  EngineNotFound exits 3.
- `engines/vep.rs` (framing, `Frame`, `parse_frame`, builders; `hello` = vep, version, pid,
  caps), `engines/handles.rs` (`HandleRef`: Drop only queues the release; `CallableTable`),
  `engines/session.rs` (`Interp.sessions: Vec<Session>`, `start_session`/`attach_session`,
  `worker_command(e, adapter)`, handshake, `request` re-entrant loop serving reverse calls with
  the timer paused, `shutdown_sessions`: one reader thread per worker, so readers are
  `Box<dyn Read + Send>`; a worker `err` may only use 9.6's kinds, others become ForeignError).
  Session state stays Handshaking until `init`'s `ret`. Reaping a child also drops its timers
  (`process::unregister`). `session/tests.rs`: scripted workers over `std::io::pipe()` or as a
  real child (`V2_WORKER`; `worker_main` writes a newline first because libtest's "test ... "
  line has none). `Reply` has typed `exports`/`missing`; `wire()` is pub(crate); `foreign_error`
  returns a V2 error from a `v2` tag as it was and reads `cause` (8 levels); foreign frames and
  `text` wait for 1.8. `engines/vep.rs` builds `load` (no module/call/op builders yet). Errors
  render as `Kind: foreign_type: msg` plus `caused by:` (error.rs `kind_text`).
- `engines/units.rs`: `exec_block` (12.1: definition units Loading/Loaded/Failed in
  `Interp.units`; action blocks every reach), `block_name`, `session_for` (one session per key,
  adapter written by `adapters::materialize`), timeouts from config. `adapters/py.py` (embedded
  via `adapters/mod.rs`): the whole Python worker; adapter internals are module globals of
  `__main__`, so white-box tests reach them with `sys.modules["__main__"]` (units, HANDLERS).
- Empty stubs (a doc comment only): `engines/imports.rs`, `engines/runonly.rs`, `build/*`,
  `ffi/lib.rs`.
- `config/rundir.rs`: the private run dir (lazy; 0700 + lstat / protected DACL); main removes it.
- `tests/run.rs`: `requires:` → one `engine_available` probe per run; SKIP, or FAIL under
  `V2_TEST_REQUIRE`. `run_with_files` / `run_with_files_env` / `doctor_rows` helpers for CLI
  tests. `tests/v2.toml` is comment-only (it just stops the upward config search).
- `ffi/`: `cdecl` (C declarations; `parse_definition_head` waits for 15.3 (roadmap 2.3)),
  `types` (CType, Sig), `call` (libffi smoke test).

## Reminders for later phases
- Interrupt path (6.5): once `INTERRUPTED` is set the interpreter prints nothing, runs no
  catch/finally, spawns nothing and sends no `bye` (`shutdown_sessions` skips it).
- CI (`.github/workflows/ci.yml`) has never run (nothing pushed); macOS is untested.
