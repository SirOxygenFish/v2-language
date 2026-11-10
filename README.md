# V2

One script, every language. V2 is a small glue language whose blocks run Python, JavaScript and
TypeScript, with functions, values, callbacks and errors crossing between them. C, C++ and Rust
blocks come next.

> **Pre-release.** Built: the host language, `@py`, `@js` and `@ts` blocks, `@export`/`@import`
> across them, packages, handles, callbacks and errors. Not built yet: compiled blocks, `extern`
> and custom engines. [ROADMAP.md](ROADMAP.md) has the plan.

```v2
@import { mean } from py.statistics

@py stats {
    def spread(xs):
        return max(xs) - min(xs)
    @export { spread }
}
@js text {
    function shout(s) { return s.toUpperCase() + "!" }
    @export { shout }
}
@import { spread } from stats
@import { shout } from text

let data = [3, 8, 1, 9, 4]
print(f"mean ${mean(data)}, spread ${spread(data)}")   // mean 5, spread 8
print(shout("done"))                                   // DONE!
```

## Install

Build it with Rust (stable):

```bash
cargo build --release
```

The binary is `target/release/v2` (`v2.exe` on Windows). Blocks need Python 3.10+ and Node.js
22.13+ (not 23.0 or 23.1); `v2 doctor` shows what V2 found. Run a script with `v2 hello.v2`.

## Read more

- [DOCS.md](DOCS.md): the language, from the first script to the cookbook
- [INTERNALS.md](INTERNALS.md): how it works, for contributors
- [ROADMAP.md](ROADMAP.md): what's done and what's next
