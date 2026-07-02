#!/usr/bin/env python3
"""Generate the V2 package registry for the non-essential (reference-spec) stdlib
modules. Each becomes a real, installable V2 package under registry/packages/,
with an index entry under registry/index/. Run from the v2/ directory."""
import io, os, re

HERE = os.path.dirname(os.path.abspath(__file__))
V2_DIR = os.path.dirname(HERE)
REPO = os.path.dirname(V2_DIR)
REGISTRY = os.path.join(REPO, "registry")

# Modules that are genuinely implemented in the core — NOT published as packages.
CORE_REAL = {
    "math", "io", "collections", "fmt", "fs", "regex", "crypto", "hash", "uuid",
    "semver", "csv", "decimal", "money", "diff", "serialize", "log", "toml",
    "dotenv", "os", "time", "rand", "test", "signal", "cache", "parse", "iter",
    "config", "event",
}

def stub_modules():
    src = open(os.path.join(V2_DIR, "src", "interpreter.rs"), encoding="utf-8").read()
    pat = re.compile(r'make_stub_module\(\s*"([a-z0-9_.]+)"\s*,\s*&\[(.*?)\]\s*\)', re.S)
    mods = {}
    for m in pat.finditer(src):
        name = m.group(1)
        fns = re.findall(r'"([a-z0-9_]+)"', m.group(2))
        mods[name] = fns
    return mods

def write(path, content):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with io.open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(content)

def main():
    mods = stub_modules()
    index_dir = os.path.join(REGISTRY, "index")
    pkgs_dir = os.path.join(REGISTRY, "packages")
    published = []
    for name in sorted(mods):
        if name in CORE_REAL:
            continue
        fns = mods[name]
        modname = "std." + name.replace("_", ".")   # ml_vision -> std.ml.vision
        pkgdir = os.path.join(pkgs_dir, "std-" + name)
        # v2.toml for the package
        write(os.path.join(pkgdir, "v2.toml"),
              f'[project]\n'
              f'name       = "{modname}"\n'
              f'version    = "0.1.0"\n'
              f'entry      = "src/lib.v2"\n'
              f'description = "Reference package for {modname} (community-maintainable)."\n'
              f'license    = "MIT"\n\n'
              f'[dependencies]\n')
        # src/lib.v2 — documented placeholder surface for each function.
        lines = [
            f"// {modname} — reference package.",
            f"//",
            f"// This is the community/package home for {modname}. The functions below",
            f"// declare the documented API surface; replace the bodies with a real",
            f"// implementation (or a native binding) and publish a new version.",
            "",
        ]
        for fn in fns:
            lines.append(f"func {fn}(...args) {{")
            lines.append(f"    // TODO: implement {modname}.{fn}")
            lines.append(f"    return null")
            lines.append(f"}}")
            lines.append("")
        write(os.path.join(pkgdir, "src", "lib.v2"), "\n".join(lines))
        # registry index entry (path source, relative to the registry root).
        write(os.path.join(index_dir, f"{modname}.toml"),
              f'# registry entry for {modname}\n'
              f'name = "{modname}"\n'
              f'path = "packages/std-{name}"\n'
              f'version = "0.1.0"\n')
        published.append(modname)
    # Registry README
    write(os.path.join(REGISTRY, "README.md"),
          "# V2 Package Registry\n\n"
          "This directory is a V2 package registry index. Each `index/<name>.toml` maps a\n"
          "package name to its source; each `packages/<name>/` is a V2 package.\n\n"
          "It hosts the **reference (non-core) standard library** — the modules that are not\n"
          "built into the `v2` binary and are instead delivered as installable packages.\n\n"
          "## Use it\n\n"
          "```bash\n"
          "export V2_REGISTRY=/path/to/this/registry   # or a GitHub URL of this repo\n"
          "v2 add std.http\n"
          "v2 search http\n"
          "```\n\n"
          f"## Packages ({len(published)})\n\n"
          + "\n".join(f"- `{p}`" for p in published) + "\n")
    print(f"Generated {len(published)} packages into {REGISTRY}")

if __name__ == "__main__":
    main()
