# V2 Packages & the `v2` Package Manager

V2 is designed so the language core stays small and **capabilities are added by installing
libraries** — written in V2 and shared as git repositories. This guide explains how to create,
publish, and use packages.

There is **no server to run**. Packages live in git repos (e.g. on GitHub), and the registry
that maps names to repos is itself just a git repo. This is the same decentralized model as Go
modules.

---

## 1. Quick start

```bash
v2 init                       # create v2.toml + src/main.v2 in the current folder
v2 add mathutils --git https://github.com/you/mathutils
v2 run                        # runs [project].entry from v2.toml
```

In code, import by name (no quotes needed):

```v2
import mathutils
import stringutils as stru    // import the whole module as an object

print(square(5))              // a function exported by mathutils
print(stru.shout("hi"))       // via the module alias
```

---

## 2. The manifest: `v2.toml`

Every project (app or library) has a `v2.toml` at its root:

```toml
[project]
name       = "myapp"
version    = "1.0.0"
entry      = "src/main.v2"
repository = "https://github.com/you/myapp"   # required to `v2 publish`

[dependencies]
mathutils   = "1.2.0"                                   # by name, via the registry
stringutils = { git = "https://github.com/you/su" }     # directly from git
locallib    = { path = "../locallib" }                  # a local folder

[dev-dependencies]
test-helpers = { git = "https://github.com/you/th" }
```

A **dependency** can be:

| Form                          | Meaning                                        | Needs a registry? |
| ----------------------------- | ---------------------------------------------- | ----------------- |
| `name = "1.2.0"`              | resolve `name` through the registry index      | yes               |
| `name = { git = "URL", version = "1.2.0" }` | clone the repo, check out tag `1.2.0` | no                |
| `name = { path = "../dir" }`  | use a local folder                             | no                |

Installed packages are placed in **`v2_modules/`** and recorded in **`v2.lock`**. Add
`v2_modules/` to your `.gitignore`; commit `v2.toml` and `v2.lock`.

---

## 3. Commands

```bash
v2 init [--lib]           # scaffold an app (or a library with entry = src/lib.v2)
v2 add <name> [--path P | --git URL]   # add + install a dependency (name@1.2.0 pins a version)
v2 remove <name>          # remove a dependency and its installed copy
v2 install [--frozen]     # install everything from v2.toml (--frozen: don't re-resolve)
v2 update                 # re-fetch dependencies
v2 list                   # show declared/installed dependencies
v2 run | test | build     # run / test / build the project entry from v2.toml
v2 publish [--git URL]    # register this package in your registry index
v2 search <query>         # find packages in the registry
```

Path dependencies need nothing installed. Git and registry dependencies use the `git`
command under the hood, so `git` must be on your PATH.

---

## 4. Writing a library

A library is just a package whose `entry` exports functions (and classes, etc.):

```bash
v2 init --lib             # creates entry = src/lib.v2
```

```v2
// src/lib.v2
func greet(name) {
    return f"Hello, ${name}!"
}

class Stack {
    func init() { self.items = [] }
    func push(x) { self.items.push(x) }
    func pop() { return self.items.pop() }
}
```

Everything defined at the top level of the entry file is importable. Consumers do:

```v2
import mylib
print(greet("world"))

import mylib as m
let s = m.Stack()
```

Publish it by pushing the folder to a git repo. That's the whole distribution step.

---

## 5. The registry (optional, for install-by-name)

The Windows installer already installs a **bundled reference registry** (the non-core
standard library — `std.http`, `std.db`, `std.image`, …) and sets `V2_REGISTRY` to it, so
`v2 add std.http` works immediately after install. An installed package overrides the
built-in stub of the same name, so installing `std.http` replaces the placeholder with the
package's implementation.

Git and path dependencies work with zero setup. If you want the nicer
`name = "1.2.0"` experience, point V2 at a **registry index** — a folder (typically a
git repo on GitHub) containing one small file per package:

```
registry/
  index/
    mathutils.toml     ->  git = "https://github.com/you/mathutils"
    stringutils.toml   ->  git = "https://github.com/you/stringutils"
```

Tell V2 where the index is with the `V2_REGISTRY` environment variable — either a local
path or a git URL (which V2 clones and caches):

```bash
export V2_REGISTRY=https://github.com/you/v2-registry     # or a local path
```

Register your own package (writes `index/<name>.toml`, which you then commit + push):

```bash
v2 publish --git https://github.com/you/mathutils
```

Because the index and every package are ordinary git repos, **GitHub hosts all of it** — you
never operate a server. A central website like npmjs.com is optional and only adds discovery
polish; the mechanism above is complete on its own.

---

## 6. How resolution works (mental model)

1. `v2 install` reads `[dependencies]` from `v2.toml`.
2. For each dependency it figures out a **source**: a local path, a git URL, or a name it
   looks up in the registry index to get a git URL.
3. It fetches the source into `v2_modules/<name>/` and records the resolved source in `v2.lock`.
4. At runtime, `import <name>` finds `v2_modules/<name>/` and loads that package's `entry`
   (from the package's own `v2.toml`, falling back to `src/lib.v2` / `src/main.v2`).

That's the entire system. Packages are code in git, the registry is a lookup table in git,
and the client is `v2`.
