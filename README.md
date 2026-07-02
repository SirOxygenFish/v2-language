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
- [`NOT_YET_IMPLEMENTED.md`](NOT_YET_IMPLEMENTED.md) — authoritative implementation status
- [`AUDIT.md`](AUDIT.md) — feature-by-feature audit against the actual source
- [`FAULT_HANDLING_DESIGN.md`](FAULT_HANDLING_DESIGN.md) — design for signal/fault recovery (not yet implemented)

## Testing

```bash
cd v2
cargo test                              # Rust unit/integration tests
cd ..
python test_docs_compliance.py          # runs every DOCS.md example against v2/target/debug/v2.exe
```

`test_docs_compliance.py` builds its checks straight from the documented examples in `DOCS.md`,
so it's the fastest way to see whether the docs still match the implementation.

## What's implemented

The core language (arbitrary-precision `int`, exact `decimal`, classes/traits/enums, pattern
matching, generators, async, error handling, Turing-complete macros) and the pure-computation
standard library (`std.math/io/collections/fmt/fs/regex/crypto/hash/uuid/semver/csv/decimal/
money/diff/serialize/log/toml/dotenv`, …) are fully implemented and tested. The 62 larger I/O-,
network-, and hardware-bound modules (`std.http`, `std.db`, `std.ui`, `std.gpu`, `std.image`, …)
ship as installable [`registry/`](registry/) packages rather than being baked into the binary —
see [`NOT_YET_IMPLEMENTED.md`](NOT_YET_IMPLEMENTED.md) for the authoritative implemented/partial/
stub status of every module.
