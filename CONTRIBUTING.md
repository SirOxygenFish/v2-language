# Contributing to V2

Thanks for helping. V2 is a small project, so the rules are short.

## Reporting a bug

Open an issue and pick "Bug report". The most useful report has:

- the smallest `.v2` script that shows the problem (and any `.py` or `.js` files it imports),
- what you expected, and what happened, with the whole error message,
- the output of `v2 doctor`: it shows V2's version, your system, and which Python and Node.js
  V2 found.

`v2: internal error: …` (exit code 101) is always a bug in V2, whatever the script does. So is a
message that doesn't tell you what went wrong or what to do about it. A question about how to do
something is welcome as an issue too.

## Building and testing

You need Rust (stable) and a C compiler, as in [README.md](README.md#build-from-source), and for
the engine tests Python 3.10+ and Node.js 22.13+ (tests that need a missing one are skipped and
listed as `SKIP`).

```bash
cargo build                    # target/debug/v2
cargo test                     # everything below
cargo clippy --all-targets     # must print no warnings
```

`cargo test` runs the unit tests and `tests/run.rs`: every `.v2` file under `tests/`, which must
print its `.out` file; the command-line tests; and every example in DOCS.md and README.md, which
must print what the docs say (`tests/docs/`). CI runs the same on Windows and Linux, and with the
oldest supported Python and Node.js, for every push. `tests/install/smoke.sh` and `smoke.ps1`
install a release archive into a fresh home folder and follow README.md's quickstart word for word
(the release workflow runs them).

## Changing V2

- [DOCS.md](DOCS.md) says what V2 does, [INTERNALS.md](INTERNALS.md) how it does it, and
  [ROADMAP.md](ROADMAP.md) in what order. The docs are the spec: when the code and the docs
  disagree, fix one of them on purpose, in the same change.
- Every fix comes with a test that fails without it.
- Keep V2 small. It is a glue language: what Python or npm already does well belongs in a block.
- Before starting something big, check the roadmap: phases go in order.
