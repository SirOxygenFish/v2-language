# Not Yet Implemented in V2

Authoritative status of documented features vs. the actual implementation
(interpreter is the reference engine; the bytecode VM is a secondary path).
Last reconciled: 2026-07-02.

## Fully implemented (built into the toolchain)

- **Core language**: variables/consts, all operators (incl. `&&`/`||`, `//`, `**`, `?.`, `??`,
  `?:`, ranges, spread, pipe), arbitrary-precision `int` (never overflows), exact `decimal`,
  Unicode-aware strings, lists/dicts/tuples/sets, comprehensions (incl. over generators),
  control flow (incl. C-style `for`, labeled loops, `if let`/`while let`, chained `if let && let`),
  functions/closures/lambdas, classes/inheritance/static methods, traits + impl, enums with data,
  structs, pattern matching (literals, ranges, Ok/Err/Some/None, tuple/list/struct, guards),
  error handling (typed `catch`, error objects with `.message`, re-raise), generators (lazy,
  infinite, `yield*` delegation), async/await + `for await` (synchronous model), defer,
  decorators, macros (runtime), destructuring (incl. `{a, b ?? default}`).
- **Stdlib (real)**: `std.math`, `std.io`, `std.collections`, `std.fmt` (printf/`sprintf`),
  `std.fs` (paths + local file I/O), `std.regex` (full engine with capture groups), `std.crypto` (SHA-1/256/512, MD5,
  HMAC, base64, hex), `std.hash`, `std.uuid`, `std.semver`, `std.csv`, `std.decimal`, `std.money`,
  `std.diff`, `std.serialize`/JSON, `std.log` (leveled structured logging), `std.toml` (parse/
  stringify), `std.dotenv` (parse/load/env).
- **Macros**: recursive/self-referential expansion is Turing-complete, guarded by a tunable depth
  limit (`ct_set_macro_limit`/`ct_get_macro_limit`, default 256) so infinite expansion errors
  cleanly. Pattern macros (`macro pattern`) are not yet implemented.
- **Tooling**: package manager (`v2 init/add/remove/install/update/list/run/test/build/publish/
  search`), `v2.toml` manifest, `v2.lock`, git/path/registry dependency sources, import resolution
  from `v2_modules/`.

## Partially implemented

- `std.os` (getenv, platform, arch work; process/user APIs partial), `std.time` (now/timestamp/
  parse partial; full tz/formatting incomplete), `std.rand` (basic PRNG; not cryptographic),
  `std.jwt`/`std.oauth2` (encode/verify shapes present; crypto-signature paths incomplete).
- `std.regex` is a full backtracking engine (anchors `^`/`$`, alternation `|`, numbered + named
  `(?P<name>...)` capture groups, non-capturing `(?:...)`, `{n}`/`{n,}`/`{n,m}` with lazy `?`
  variants, `\b`/`\B`, `$1`-`$9` replacement backrefs, lambda replacements, `regex.compile`
  objects). Not supported: in-pattern backreferences (`\1`) and lookaround (`(?=...)`).
- Concurrency: `thread_spawn`/`thread_join` and channels run on a **single-threaded** model
  (threads execute eagerly; channels are synchronous buffers). Actors/agents/isolates are modeled,
  not truly parallel — `actor_spawn`/`agent_create` return handle objects, but
  `actor_send`/`actor_receive`/`actor_call` are no-op stubs returning null.
  `--async-workers > 1` is accepted but not parallel.
- Macros are runtime-expanded, not hygienic compile-time expansion.
- Bytecode backend (`-c -r`) supports the core language incl. bignum, but not every newer feature
  (e.g. tagged templates) — use the interpreter (default) for full coverage.
- **Embedded language engines**: `@py` and `@js` blocks fully execute (run-in-place, `@export`,
  `@import { ... } from @lang / block / @lang.block`, `py.module` imports, JSON value round-trip,
  persistent worker state, foreign exceptions as catchable V2 errors) via `python`/`node` on PATH.
  `@bash`/`@sh`/`@os`/`@ps`/`@lua` blocks run in place (no exports). Not yet: compiled-language
  blocks (`@c`, `@rust`, `@go`, …) which need a build-toolchain bridge, `@import` *inside* embedded
  blocks (cross-engine calls), JS module imports (`js.lodash`), managed engine bundles
  (`v2 install --engines`), and `register_engine()` (accepted, but custom tags don't execute).
- **Native FFI (`extern` / `cimport` / `std.ffi`)**: `extern func` declarations **parse only** —
  calling one returns `null`; `cimport` is a no-op; `std.ffi` is a reference-spec stub. There is
  no dlopen/libffi bridge yet. To reach another language today, use the embedded engine bridge
  (`@py`/`@js`) — native C/Rust calls need a real FFI backend.
- **WASM backend**: the entire WASM chapter (`--target wasm`, `--wasm-cap`, `extern wasm_host`,
  component-model host adapters) is design-only; `v2 -c --target wasm` reports it as unavailable.
- Soft signal handlers (`signal.on/once/off/reset/ignore/raise/alarm`) are stubs; the hardware
  fault-classification APIs in `std.signal` are real.

## Reference specification only (typed stubs; need a native backend or a package)

Importing these succeeds and returns callable stubs (so code type-checks and runs), but the
operations are not backed by real implementations. They are intended to be delivered as installable
packages (see `PACKAGES.md`) or wired to native backends:

- **Networking**: `std.http`, `std.net`, `std.grpc`, `std.mqtt`, `std.dns`, `std.ssh`, `std.webrtc`.
- **Databases**: `std.db`.
- **GUI/desktop**: `std.ui`, `std.tray`, `std.notify`, `std.clipboard`, `std.hotkey`, `std.accessibility`.
- **Hardware**: `std.gpu`, `std.serial`, `std.usb`, `std.bluetooth`, `std.camera`, `std.hal`, `std.iot`.
- **Media/graphics**: `std.audio`, `std.video`, `std.image`, `std.gfx3d`, `std.game`, `std.2d`,
  `std.speech`, `std.pdf`, `std.excel`, `std.office`, `std.qr`, `std.barcode`.
- **ML/AI**: `std.ai`, `std.ml.vision`, `std.ml.audio`.
- **Other I/O-bound**: `std.mail`, `std.scrape`, `std.phone`, `std.blockchain`, `std.geo`, `std.map`,
  `std.watch`, `std.compress`, `std.archive`, `std.yaml` (parsing not implemented), `std.cli`
  (arg-parser builder not implemented).

## Runtime-safety / platform (design-only)

- `signal.set_recovery_point()` / `signal.recover()` and `signal.dump_core()` are designed
  (`FAULT_HANDLING_DESIGN.md`) but not implemented — Milestone 2.
- POSIX alt-stack (`sigaltstack`), Windows minidump, and setjmp/longjmp recovery are not implemented.
- LeakSanitizer/ASan-style native heap instrumentation is simulated inside the interpreter only.
- Sized-integer overflow **modes** (`--overflow wrap/saturate/panic` for `u8`/`i32`/…) are parsed
  but not enforced; only the default unsized-`int` arbitrary-precision path is implemented.
- LSP mode, step debugger, coverage, and some profiling flags are parsed but partial/placeholder.
