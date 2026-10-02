# Notes for Claude (working notes on the V2 reboot)

The owner allowed this file so I keep my bearings across sessions. Keep it short and current;
it is loaded automatically into every session and every subagent.

## What this is
- V2: a small glue language (Rust, tree-walking interpreter) whose point is the polyglot system:
  `@py`/`@js`/`@ts`/`@c`/`@cpp`/`@rust` blocks, `@export`/`@import`, VEP/1 workers, compiled
  blocks via libffi, `extern`.
- **DOCS.md** = what the language does (user spec). **INTERNALS.md** = how (implementation spec,
  `§n.m` = its sections). **ROADMAP.md** = order + what's done ("Where we are", progress log).
- `old/` = the archived previous implementation. Reference only; never build on it.
- Branch `v2-reboot`. Never push or merge to main unless the owner asks.
- Status: Phase 0 (host core, 0.1–0.10) is complete. Next: Phase 1, starting with 1.1.

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
  `wsl.exe -e bash -lc "rsync -a --delete --exclude target --exclude old --exclude .git /mnt/c/Users/SirOxygen/Desktop/v2-language/ ~/v2/ && cd ~/v2 && cargo test"`
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
- `cli/`: argv parser, HELP text. `config/`: files → raw values (`Raw`, keyed
  `section.key`/`engines.tag.key`) → L3/L4 overrides → `validate()` of the winners only.
  `load_from(start, flags, home, env)` is the test seam. Custom engine tables kept raw (17.1 =
  Phase 3).
- `syntax/`: `span` (SourceMap, FileId, display_path), `lexer`, `scan` (block bodies, profiles,
  `canonical_tag`), `directives` (@export/@import, dedent), `parser` (MAX_DEPTH 256, expression
  height MAX_LINKS 10,000), `check` (static checks, 4.5), `ast`.
- `runtime/`: `interp` (exec/eval take `&Env`; only stack = `calls`; `guard()` at 7/8 of the
  stack; MAX_CALLS 10,000), `value`, `env`, `call` (binder), `ops`, `methods`, `strings`
  (char cache), `builtins` (+ `Out`, the only stdout writer), `error` (ErrorValue, Flow,
  rendering, did_you_mean), `module` (import graph, `locate`, `shown`).
- `Flow { Error, Return, Break, Continue, Exit }`, `Exec<T> = Result<T, Flow>`. Interrupts are
  never a Flow.
- Deep values are freed iteratively (Drop impls feed a thread-local queue).
- `codec/json`: plain mode only (serde_json, pre-scan for i64 overflow + nesting 1,000);
  `$v2` tags and base64 come in 1.3.
- `doctor/`: host-only `v2 check` (explicit-stack import walk; problems sorted by the order
  files were met). Doctor itself = Phase 1.1.
- `engines/session.rs`: watchdog + `INTERRUPTED`, `interrupted()`, `stop_if_interrupted()`
  (interp thread stops where it would run catch/finally or print), `exit_interrupted()`
  (`_exit(130)` on Unix). Handler only installed if the watchdog spawned; inherited SIG_IGN kept.
- `ffi/cdecl` is done (parser needs it); rest of `ffi/`, `build/`, `engines/*` mostly empty.

## Where I stopped (2026-10-02, usage limit)
- Plan: `C:\Users\SirOxygen\.claude\plans\wild-wobbling-creek.md` (Phase 1 foundation = 1.0–1.3,
  then report to the owner). 1.0 (`engines/process.rs`) is committed; its adversarial review
  was stopped before finishing — re-run it, fix findings, then do 1.1.
- 1.1 drafts (not yet in the repo): `%TEMP%\claude\C--Users-SirOxygen-Desktop-v2-language\
  01e93e21-6252-4f4b-b948-9df871d7b845\scratchpad\draft11\` — spec.rs, resolve.rs (+
  resolve_tests.rs to append), registry.rs, doctor_fn.rs, edit_harness.py (requires:/SKIP).
  Still to write: interp/builtins/main/run_script wiring, exit 3, .v2 + run.rs tests, docs.
- Node 24.11.1 is installed in WSL (`~/.local/node`, on PATH via `~/.local/bin`).

## Reminders for later phases
- Interrupt path (6.5): once `INTERRUPTED` is set the interpreter prints nothing, runs no
  catch/finally, spawns nothing, and must NOT send `bye` (the watchdog kill_trees). When
  sessions/children arrive (1.2): register children for the watchdog's kill_tree, call
  `stop_if_interrupted()` before every spawn, make the end-of-run shutdown skip `bye` after an
  interrupt, and have `exit_interrupted()` kill trees + remove the run dir.
- Check's ENGINES/PACKAGES/LIBRARIES/COMPILED sections and the pending-override warning
  (18.4) come with 1.1 / Phase 3.
- CI (`.github/workflows/ci.yml`) has never run (nothing pushed); macOS is untested.
