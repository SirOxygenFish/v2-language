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
- `old/` (the previous implementation) was removed at the `v0.1.0-alpha.1` release; it is in git
  history before that tag. Never build on it.
- Work on branch `v2-reboot` (fast-forwarded to `main` at the release). Never push or merge to
  main unless the owner asks.
- Status: ROADMAP "Where we are". Phase 2 needs the owner's go-ahead.

## In flight (update at every step boundary; trim when a milestone closes)
- Released `v0.1.0-alpha.1` (2026-11-12) and `v0.1.0-alpha.2` (2026-11-19, on the owner's OK: a
  published GitHub pre-release with the binaries; `main` = its tag, 80cee2c; CI green; the README
  one-liners install it on Windows, Linux and macOS). Pre-publish and milestone 1.15 (interpreter
  speed) are done: ROADMAP.
- Next, only when the owner says go: `llms.txt`, then Phase 2 in the ROADMAP's order. Pushing
  `v2-reboot` and CI are fine; merging, tags and publishing wait for the owner's OK. Ask before
  creating OS user accounts.
- A `[release]` push now fails (release.yml leaves a published release alone): the next release
  first bumps the version in Cargo.toml, install.sh and install.ps1 (release.yml checks all three).
- Owner decisions still open: `#!` lines, static members of imported classes, `v2 check` finding
  undeclared names.
- Newcomer rounds ended after round 23 (2026-11-19; about 4M tokens a round, and the owner stopped
  round 23's agents over usage): no round 24 unless the owner asks, and then a lean one (one
  newcomer, one implementer, one review). How they ran is in the ROADMAP log. Agents always get
  this file and the memory index, so newcomers are told to ignore them; they work as root only,
  with no OS user accounts (the owner's rule). Clean Linux: `wsl --import` of the official Ubuntu
  base images in scratchpad/distro-images; remove with `wsl --unregister`.
- The owner's Windows PATH has an old v2 (`%LOCALAPPDATA%\Programs\v2\bin`): never touch it
  (install.ps1 warns that it shadows a new install). `C:\tmp` and `C:\var` are round 19's test
  leftovers: leave them; tmp_on_windows_points_to_temp skips while they are there.
- Installer changes run on Windows, WSL and the Mac before they are pushed (tests/install/
  paths.{sh,ps1}; paths.ps1 changes the user PATH only with -ModifyPath, in CI). WSL-share tests:
  V2_TEST_WSL_DISTRO=v2win21, set from PowerShell, never Git Bash. CI's minimum Node for local
  checks: WSL's ~/node2213/node-v22.13.0-linux-x64/bin first on PATH. A swap-out proof in WSL:
  `touch` the restored files after the rsync (rsync -a keeps Windows' older mtimes).
- `готово/` in the repo root is the owner's folder (empty now, not V2): never touch, delete or
  commit it; it is in `.git/info/exclude`.
- macOS: the owner's Mac Mini, `ssh -o BatchMode=yes siroxygen@192.168.100.2` (key, owner-approved
  2026-11-10); run tests in `zsh -lic` (Homebrew's PATH: cargo, python3 3.14) from
  `~/Desktop/v2/v2-test`, copied with tar excluding target, .git, .claude. Every V2 file on the Mac
  (scripts, logs, test folders) goes inside `~/Desktop/v2`, never loose in the home folder (the
  owner's request, 2026-11-20); a test's temp HOME goes in `mktemp -d` and is removed after. Node 22 (owner-approved brew `node@22`) is
  keg-only: `export PATH=/opt/homebrew/opt/node@22/bin:$PATH` first.
- Reviews that worked best: oracle fuzzers written from DOCS/INTERNALS (not old-vs-new), two
  finders a round, each with a refuter. Keep them lean.
- Reviewer agents run every program through scratchpad `capped.py EXE FILE 2048 [TIMEOUT_S]` (kills
  v2 and its workers past 2 GB together or the time limit; a test ran away to 47 GB on
  2026-11-02; in WSL `~/v2snap/capped.py`, a /proc version, with snapshots `~/v2snap/v2*`), never
  kill by name, and stop and report when an action is refused.
- Per milestone: read its § sections, ch. 21 rows and R-tests → build + tests → cargo test +
  clippy on Windows and WSL → run its DOCS examples → review workflow (finders + refuters) → fix
  with regression tests (swap-out proofs) → check boxes, ROADMAP log, code map → commit.
- Open proposals from 2026-11-10 (not decided): imported classes have no static members, `#!`
  lines, narrower closure capture. (`s +=` and `dict.remove` were fixed at the release gate.)
- Scratchpad helpers are session-local and can vanish (the app crash of 2026-11-02 wiped them):
  bench/measure.py re-measures release numbers; rewrite fuzzers if needed. Never generate a sweep
  test's .out from the binary: its output comes from DOCS.

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
  `wsl.exe -e bash -lc "rsync -a --delete --exclude target --exclude .git --exclude .claude '/mnt/c/Users/SirOxygen/Desktop/My Stuff/v2-language/' ~/v2/ && cd ~/v2 && cargo test"`
  (the repo lives in `Desktop\My Stuff\v2-language`; quote the path, it has a space)
  (also `cargo clippy --all-targets` there: nightly has extra lints, and cfg(unix) code only
  compiles there).
- WSL's `Catastrophic failure ... E_UNEXPECTED` during heavy runs: most likely the VM running out
  of memory (2026-11-19: generated programs of 5 GB each, an OOM kill in `dmesg`). Cap each
  program's address space (RLIMIT_AS, 3 GB at most 8 at a time) and send output to files, not
  pipes; `wsl --shutdown` recovers.
- For review workflows, copy `target/debug/v2.exe` to the scratchpad and let agents use that
  snapshot (never cargo in the repo while agents run).
- `cargo test` = unit tests + `tests/run.rs` (the .v2 harness + CLI tests). Clippy must be clean.
- Never name a file `nul`, `con`, `aux`, `prn`, `com1`… (any extension): Windows device names;
  git can't add them.

## Code map (src/)
- `main.rs`: threads (interp thread 256 MiB stack), exit codes, panic hook → 101. Defines the
  output macros `stdout!`/`stdoutln!`/`stderr!`/`stderrln!` (ignore write errors; the stderr
  ones first call `stop_if_interrupted`). Never use `print!`/`eprint!` in src. Debug builds:
  `V2_TEST_PANIC` panics the interpreter thread (for the 101 test).
- `cli/`: argv parser (`v2 FILE`, `check FILE`, `doctor`, `cache clean` (parsed; main prints
  "not implemented yet" until 2.3), `--version`, `--help`; no other flags), HELP text;
  `entry.rs`: what to do next for a missing FILE or a folder, the note for a .py/.js file (19.1).
  Release side (not src): `install.sh`, `install.ps1`, `.github/workflows/release.yml`,
  `tests/install/smoke.{sh,ps1}` and `paths.{sh,ps1}` (§22.6).
  `config/`: the project's `v2.toml` read in file order, then
  `V2_ENGINE_<TAG>` overrides (canonical tags only; a replaced `command` is not validated);
  `command` must be an array; unknown sections/keys and keys that don't apply are config
  errors (no warnings). `load_from(start, env)` is the test seam. Custom engine tables kept
  raw until Phase 3 (17.1). `unexpanded(text)`: the shell variable (`%X%`, `$X`) a hint names.
- `syntax/`: `span` (SourceMap, FileId, display_path; `shown_path`, the one escaper of control and bidi
  characters in printed paths; `same_file`: one module per file identity), `lexer`, `scan` (block bodies, profiles,
  `canonical_tag`), `directives` (@export/@import, dedent), `resolve` (layouts, `Loc`, after
  `check`, in `parse_file`, the entry path and `parse_str`), `parser` (MAX_DEPTH 256, expression
  height MAX_LINKS 10,000; anonymous functions are `func(…)`, there is no `lambda`), `check`
  (static checks, 4.5, incl. "this expression's value is not used" and a loop's `let i = i + 1`), `ast`.
- `runtime/`: `interp` (exec/eval take `&Env` and keep only the common cases, the rest in
  `exec_other`/`eval_other`; 1.15's fast paths (5.7): `leaf` (an int or a variable without eval),
  `peek_int`/`int_op` (`binary`'s ints read in place), `direct_call` (+ `direct_call_named`,
  `fill_params`) and call_value's `fill_params_pos` bind without the binder, `run_body`'s tail
  `return` and guard clause, `compound_slot` (`x op= y`: peek, never a copy, or `s +=` stops
  growing in place), loops' shared frames (`Body.reusable`, `LoopVars`, `exec_while`; `item_iteration` copies a list's, dict's or handle's loop variables to keep their release order); stacks: `calls` and `import_stack`; `guard()` at 7/8 of the stack, on calls and `eval_other`; MAX_CALLS 10,000; `files` (FileScope per file), `units`, `routes`;
  `call_value(f, pos, named, at)` calls any V2 value from Rust; `line`: 12.7), `value` (`Callable` = Closure |
  Builtin | Proxy {session, unit, name} | Route {route, name} | Waiting {import, local, seen,
  resolved}: an `@import` that hasn't run, settled at first use by `interp.settle`/`settled`;
  `Str`: an `Rc<str>` until `+=` grows it in place (`env::grow`); `DictMap`: dicts in order with
  holes, O(1) remove),
  `env` (`Frame` = layout + slots, by position; an empty slot means not declared yet, so reads
  and writes fall back by name; a spare-array pool and `POOL` of whole frames (`pool`,
  `new_frame`), `shared_frame`/`end_iteration`/`release_shared` for a loop's shared frame, `read_int`/`update_int`, `GC_ACTIVE`
  (the collector's lists looked at only once it has kept a frame); the frame cycle collector of 5.1: `walk`
  (lazy for `leave` and `recheck`, everything for `collect`); `closure_dropped` from `Func::drop`
  queues rechecks; `made()` counts list items, dict entries and frame slots, which start
  collections), `call` (binder: `ParamSpec::req/opt/rest`, `bind_spec`, its fast path `bind_plain`), `ops`, `methods` (tables of
  `MethodDef`), `strings` (char cache), `builtins` (`BuiltinDef` table, `str_arg`/`bool_arg`; `Out`, the only stdout
  writer, its pending bytes in `PENDING` so the interrupt path's `write_pending` can write them,
  `flusher`, a thread that writes held output 100 ms later; `Out::capture()` in tests; `POSITIONAL_ONLY`; Windows names and drives: `windows_name`, `unreachable_root`/`drive_problem`; `write_stderr` (a closed stderr is 141); `input(echo: false)`: `EchoOff`, Unix `ECHO_SAVED`/`background`/`echo_around_stop` (Ctrl+Z), Windows `ECHO_MODE`/`echo_back`; files (round 23): `replace_file` (Windows write_file: lock held through cut and write), `write_to_stream`, `device_path`, `busy_hint`, `program_file`, `folder_link`/`remove_dir_link`, `long_name`, `slash_after_a_file`, `cut_utf8` (read_file, input) and exec's `cut_output`; exec output: `by_lines`, `other_text`, `single_byte`, `pick_code_page`, `other_code_page`, `code_page_text`/`strict_code_page_text`; exec start errors: `start_error`, `with_commas`), `error` (ErrorValue, Flow, `fail`,
  rendering, did_you_mean), `module` (import graph, `locate`, `shown`; `link_problem`/`link_error`:
  links that lead nowhere or in a loop, 3.10; `unicode_twin`/`folder_twin`: a Mac's decomposed names;
  `nearby_module`: an import's did-you-mean). Number hints: builtins' `number_text`, `between_thousands`. A builtin returns
  `fail(..)`; the caller places it at the call site.
- `Flow { Error, Return, Break, Continue, Exit }`, `Exec<T> = Result<T, Flow>`. Interrupts are
  never a Flow.
- Deep values are freed iteratively (Drop impls feed a thread-local queue).
- `codec/json`: plain mode (serde_json, pre-scan for i64 overflow + nesting 1,000).
  `codec/vep`: the `$v2` wire codec (`Wire` context: session, tag, caps, release queue,
  callables, sm, `routes`; a route `fn` tag decodes via `Route::value`).
- `doctor/`: `v2 check` (parse + static checks of the file and its imports, explicit-stack walk;
  prints `check X: N files, M errors`) and `doctor::doctor` (the table, notes, version line,
  `ready:`/`not ready:` summary).
- `engines/process.rs`: child rules (setsid/PDEATHSIG; suspended + Job Object), `ChildProc`,
  `KillHandle` (`release_memory`: 8.8), `UntilExit` (a worker's stdout on Unix: ends with the child), `run_captured(argv, env, limit, input)` (stdin null or fed by an unjoined thread;
  `SpawnError { message, reason }`; batch paths with `%` refused), the watchdog (only the top timer
  runs, not while paused; exit check every 250 ms; SIGCHLD reset),
  `INTERRUPTED`/`stop_if_interrupted`/`exit_interrupted` (kills trees, waits ≤ 500 ms, removes
  the run dir), `take_interrupts`/`spawn_helper` (Unix: only the interpreter thread takes
  SIGINT/TERM/HUP/QUIT; **start every helper thread with `spawn_helper`**), parking (`allow_parking`, `Watch::due_to_park`; a thread per trim; debug builds
  take `V2_TEST_PARK_MS`), the terminal (6.5: `remember_terminal`/`restore_terminal` only when stdout is a terminal or `TERMINAL_CHANGED`, `show_cursor`, `cursor_back` + `TOLD_TERMINAL`, `terminal_size` for init's `terminals`; Windows `console_vt_on`/`restore_console`), Windows starts: `SpawnError.code`/`short`, `long_program` (259; a batch file 246), `short_program`/`short_path` (8.3), `command_line_units`. Tests re-run the test binary as the child (`V2_HELPER`); **tests that push
  timers must hold `engines::process::tests::serial()`** (the watchdog is global).
- `engines/spec.rs` (py/js/ts table, `Version`, `too_old`: Python 3.10+, one Node rule for js
  and ts: 22.13+, not 23.0/23.1), `engines/resolve.rs` (`Lookup`, `find_program`,
  `Prober`/`RealProber`, `ProbeCache` (usable entries only) + `CacheMode`, `Resolver`,
  `ResolvedEngine`, `NotFound`; venvs, 7.6: `venv_mismatch`, `activated_venv_problem`, `venv_stamps`
  in the cache key; `clip_middle`, `venv_maker` (the remake line from the base Python), `PthPython` (embeddable), `unused_venv`, `made_elsewhere`, `npm_fix`; `library_fix(id, …)`/`library_fixes`; spec `limit_fix_at`), `engines/registry.rs` (`Registry::real`, `resolve_tag`,
  `engine_available`; `later_language`: tags of Phases 2-3 say "not in this version yet", drop
  each row when its engine registers). Uncaught EngineNotFound exits 3.
- `engines/vep.rs` (framing, `Frame`, `parse_frame`, builders; `hello` = vep, version, pid,
  caps), `engines/handles.rs` (`HandleRef`: Drop only queues the release; `CallableTable`),
  `engines/session.rs` (`Interp.sessions: Vec<Session>`, `start_session`/`attach_session` (Unix, an
  engine with `protocol_fds` (js): the protocol on two pipes named in `V2_VEP_FDS`, fd 0 null,
  fd 1 the host's stderr),
  `worker_command(e, adapter)`, handshake, `request` re-entrant loop serving reverse calls with
  the timer paused, `wrapper_note`/`conda_run_advice` (8.4), `shutdown_sessions`: one reader thread per worker, so readers are
  `Box<dyn Read + Send>`; a worker `err` may only use 9.6's kinds, others become ForeignError).
  Session state stays Handshaking until `init`'s `ret`. Reaping a child also drops its timers
  (`process::unregister`). `session/tests.rs`: scripted workers over `std::io::pipe()` or as a
  real child (`V2_WORKER`; `worker_main` writes a newline first because libtest's "test ... "
  line has none). `Reply` has typed `exports`/`missing`; `wire()` is pub(crate); `foreign_error`
  returns a V2 error from a `v2` tag as it was and reads `cause` (8 levels); foreign frames and
  `text` (custom engines only) are mapped by `engines/traces.rs` (1.8). `engines/vep.rs` builds `load` (no module/call/op builders yet). Errors
  render as `Kind: foreign_type: msg` plus `caused by:` (error.rs `kind_text`).
- `engines/units.rs`: `exec_block` (12.1: definition units Waiting/Loading/Loaded/Failed in
  `Interp.units`; action blocks every reach), `Line` (12.7: one queue in program order across
  languages), `before_block`, `run_line`, `block_name`, `session_for` (one session per key, adapter
  written by `adapters::materialize`, then the line), `start_engine` (by tag), timeouts from
  config. `adapters/py.py` (embedded
  via `adapters/mod.rs`, `for_session(key)`): the whole Python worker; its internals are module
  globals of `__main__`, so white-box tests reach them with `sys.modules["__main__"]` (units,
  HANDLERS; `AfterImport` changes shutil and colorama at their first import; `stdlib_hint`; `member_error`; `flush_files` (bye in a stub); `refuse_big`/`sampled_size`/`table_hint`). `adapters/js.js`: the Node worker (js and ts); its internals are module-scoped, so
  JS white-box tests use `process.*` and `require("vm").runInNewContext("gc")`; `seeTerminal` gives process.stdout/stderr V2's terminal (9.9); `noInput` refuses stdin listeners; `builtinMember`.
- `engines/handles.rs`: `HandleRef` (+ `callable`, `text()`, `plain_way`: the block for a date or Decimal), `CallableTable` (Values: functions
  and other sessions' callable handles, keyed by Rc address), and the host's handle ops on
  `Interp`: `handle_getattr/setattr/getitem/setitem/len/str/value/call/iter/next`; `foreign_call`
  (handles and Proxies; `check_alive` first, then the kwargs cap).
  interp.rs routes members, indexing, calls (`MethodTarget::Handle`), `for` and `str()` to them;
  `str_of(v, at)` and `index(o, k, at)` take a span for that.
- `engines/imports.rs` (ch. 13): `FileScope` (frame, definition blocks collected from the AST,
  top-level names, what may wait: the `Walk` over the tree records each name's uses),
  `candidates`/`provides`/`provider` (13.3 steps; `Seen` = blocks a waiting `@import`'s statement
  saw), `exec_import` (V2 level: `import_waits`, `wait_import`, or `resolve_import`), `settle`,
  `module_unit` (ids, py name check) + `import_module` (one `module` frame), `inner_imports`
  (13.7 entries: same-session `{as,unit,name,route}`, the real object once that unit has loaded, or
  routes), `call_route`/`resolve_route`, hints: `npm_hint` (+ `block_module_hint`), `pip_for`/`install_line`,
  `export()` builds Proxies. units.rs: `Export {name, value, func}`, `wire_unit`.
  Adapters: py `do_module`/`module_attr`/`UnitStub`/`RouteFunction`; js `doModule` (resolve
  first; TLA via import() only for the module itself)/`moduleAttr`/`unitStub` (a Proxy)/`routeStub`.
- `engines/traces.rs` (2.3, 2.5, 12.6): `foreign_frames(s, err)` → frame texts + the innermost
  block position and tag; `block_at` uses `FileScope.bodies` (every block) limited to the
  reporting engine. `UnitLoc::col` returns Option (checked) and knows `@@` escape lines.
  js.js `realPosition` maps V8 lines past U+2028/2029 back (`shifted`).
- Empty stubs (a doc comment only): `engines/runonly.rs`, `build/*`, `ffi/lib.rs`.
- `config/rundir.rs`: the private run dir (lazy; 0700 + lstat / protected DACL, made by its `\\?\`
  path; a temp folder too deep on Windows: its 8.3 name via `choose`, else `temp_problem`); main
  removes it.
- `tests/run.rs`: `requires:` → one `engine_available` probe per run; SKIP, or FAIL under
  `V2_TEST_REQUIRE`. `Pty` (Unix: `wait`/`reap` keep reading, as macOS holds a session leader's exit until its
  output is read; `flags` through the master, as macOS revokes the slave). `run_with_files` / `run_with_files_env` / `doctor_rows` helpers for CLI
  tests. `tests/v2.toml` is comment-only (it just stops the upward config search).
- `bench/`: the interpreter's benchmarks (fib, loop, while, calls, closures, lists, dicts, strings,
  errors, and engine ones), `measure.py` (wall-time medians) and `callgrind.sh` (instruction counts
  in WSL, `--top NAME`); 1.15's baseline and numbers are in ROADMAP. Compare full-size wall times
  too: a quadratic cost hides at callgrind's small sizes.
- `ffi/`: `cdecl` (C declarations; `parse_definition_head` waits for 15.3 (roadmap 2.3)),
  `types` (CType, Sig), `call` (libffi smoke test).

## Reminders for later phases
- Interrupt path (6.5): once `INTERRUPTED` is set the interpreter prints nothing, runs no
  catch/finally, spawns nothing and sends no `bye` (`shutdown_sessions` skips it).
- CI runs on every push (Windows, Linux, minimum versions); read results in the built-in browser,
  which loses the owner's GitHub sign-in when the pane reopens (ask them to sign in again).
  macOS is the owner's Mac, not CI, except release.yml's macOS build and install check.
