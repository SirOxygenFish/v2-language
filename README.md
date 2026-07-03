# V2

V2 is a from-scratch, general-purpose programming language with a "small", fully-implemented
core and a package-manager-driven standard library. It has a tree-walking interpreter (the
reference engine) and a bytecode VM, both written in Rust.

## Install (Windows x64)

Run the installer — no manual copying or PATH editing:

```bat
:: double-click install.bat, or:
powershell -ExecutionPolicy Bypass -File install.ps1
```

It installs `v2` to `%LOCALAPPDATA%\Programs\v2\bin`, adds it to `PATH`, bundles the docs,
and installs the reference package registry. Open a new terminal, then:

```bash
v2 --version
v2 --help
```

Uninstall: `powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall`

### Build from source (any platform)

```bash
cd v2
cargo build --release      # -> v2/target/release/v2
```

## Hello, world

```v2
func main() {
    print("Hello, World!")
}
main()
```

```bash
v2 hello.v2        # run a file
v2 run             # run the project entry from v2.toml
```

## Packages

```bash
v2 init                          # create a project (v2.toml)
v2 add mathutils --git <url>     # add a git dependency
v2 add std.http                  # install a library from the registry
v2 list / v2 remove / v2 publish / v2 search
```

The [`registry/`](registry/) folder in this repo _is_ the reference registry — an `index/`
mapping package names to sources plus 62 `packages/std-*` implementations of the I/O-, network-,
and hardware-bound modules that aren't baked into the `v2` binary (see [`registry/README.md`](registry/README.md)).
See [`PACKAGES.md`](PACKAGES.md) for the full package-manager guide.

## Project layout

```
v2-new/
├── v2/              # the language itself (Rust): interpreter, bytecode VM, parser, CLI
│   ├── src/         # compiler/runtime source
│   ├── tests/       # Rust integration tests
│   └── testsuite/   # .v2 sample/regression programs
├── registry/        # reference package registry (index/ + packages/std-*)
├── install.ps1 / install.bat   # Windows installer
└── *.md             # docs (see below)
```

## Documentation

- [`DOCS.md`](DOCS.md) — language reference (also `v2 --docs`)
- [`INTERNALS.md`](INTERNALS.md) — compiler/runtime internals (`v2 --internals`)
- [`IMPLEMENTATION.md`](IMPLEMENTATION.md) — implementation checklists per language feature
- [`PACKAGES.md`](PACKAGES.md) — packages & package manager (`v2 --packages`)
- [`LIST.md`](LIST.md) — chapter-by-chapter status map of every documented feature
- [`NOT_YET_IMPLEMENTED.md`](NOT_YET_IMPLEMENTED.md) — **authoritative implementation status;
  read this before assuming a documented feature works**
- [`AUDIT.md`](AUDIT.md) — feature-by-feature audit (historical snapshot, Apr 2026; superseded
  by `NOT_YET_IMPLEMENTED.md`)
- [`FAULT_HANDLING_DESIGN.md`](FAULT_HANDLING_DESIGN.md) — design for signal/fault recovery (not yet implemented)

## Testing

```bash
cd v2
cargo test                              # Rust unit tests (lexer/parser/bigint/decimal/regex/engines)
./target/release/v2 testsuite/ch_test.v2         # chapter tests (and every other testsuite/*.v2)
./target/release/v2 testsuite/test_hardening.v2  # regression suite pinning past bug fixes
./target/release/v2 testsuite/test_engines.v2    # @py/@js bridge tests (needs python on PATH)
cd ..
python test_docs_compliance.py          # runs curated DOCS.md examples against the binary
```

A note on what the suites prove: `test_docs_compliance.py` checks that documented examples
*parse and run* — it does not prove every documented behavior end-to-end. The `testsuite/*.v2`
files assert actual values and are the stronger signal. For any feature not covered by a test,
treat [`NOT_YET_IMPLEMENTED.md`](NOT_YET_IMPLEMENTED.md) as the source of truth.

## What's implemented

**Working and tested:** the core language (arbitrary-precision `int`, exact `decimal`,
classes/traits/enums, pattern matching with dict/list/range patterns, generators, async,
error handling with `?` propagation, Turing-complete macros, catchable recursion limits), the
pure-computation standard library (`std.math/io/collections/fmt/fs/regex/crypto/hash/uuid/
semver/csv/decimal/money/diff/serialize/log/toml/dotenv`, …, with a full backtracking regex
engine incl. named capture groups), and polyglot `@py`/`@js` blocks — `@export`ed Python/JS
functions become callable V2 functions (worker state persists between calls), and
`@import { mean } from py.statistics` pulls Python libraries straight into V2 (see "Embedded
Language Engines" in [`DOCS.md`](DOCS.md)).

**Documented but NOT implemented yet** (the docs double as a design spec — these parse but
don't execute): native FFI (`extern`/`cimport`/`std.ffi` return `null`), the WASM backend,
compiled-language blocks (`@c`/`@rust`/`@go`), real parallelism (threads/actors run on a
single-threaded model), pattern macros, and sized-int overflow modes. The 62 larger I/O-,
network-, and hardware-bound modules (`std.http`, `std.db`, `std.ui`, `std.gpu`, …) ship as
installable [`registry/`](registry/) packages rather than being baked into the binary.
[`NOT_YET_IMPLEMENTED.md`](NOT_YET_IMPLEMENTED.md) is the authoritative list — when in doubt,
trust it over any other doc (including this one).
