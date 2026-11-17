# V2

> **Alpha (0.1.0-alpha.2).** What's here works and is tested on Windows, macOS and Linux, but V2
> is young: things can still change, and some chapters of the docs aren't built yet
> ([Known limitations](#known-limitations)). Found a bug? [Tell us](#reporting-bugs).

V2 is a small scripting language for gluing other languages together. One `.v2` file holds
Python, JavaScript and TypeScript code in blocks, and its functions call each other across
languages as if they were one program. V2 itself stays tiny: the libraries come from Python and npm.

## What you need

- **V2:** a single program, `v2` (`v2.exe` on Windows). The install command below downloads it;
  you don't need Rust or anything else to install it.
- **Python 3.10 or newer**, only for `@py` blocks and Python packages.
- **Node.js 22.13 or newer** (but not 23.0 or 23.1), only for `@js` and `@ts` blocks and npm
  packages.

A script without those blocks needs only V2. The ready-made downloads are for Windows 10 and 11
(64-bit), macOS on Apple Silicon (M1 or newer), and 64-bit Intel/AMD Linux with glibc 2.35 or newer
(Ubuntu 22.04, Debian 12, Fedora 36 or later). On anything else,
[build it from source](#build-from-source).

## Install

### Windows

Open **PowerShell** (press Start, type `PowerShell`, press Enter), paste this line and press Enter:

```powershell
irm https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.ps1 | iex
```

It puts `v2.exe` in `%USERPROFILE%\.v2\bin` and adds that folder to your user PATH. `v2` works in
that window right away; windows that were already open need to be closed and opened again.

From **cmd.exe** or **Git Bash**, run the same line through PowerShell:

```text
powershell -ExecutionPolicy Bypass -c "irm https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.ps1 | iex"
```

Then open a new window: cmd, PowerShell and Git Bash all find `v2` there (Git Bash reads the
Windows PATH when it starts), but the window you ran it in doesn't.

### macOS and Linux

Open a terminal, paste this line and press Enter:

```bash
curl -fsSL https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.sh | sh
```

It puts `v2` in `~/.v2/bin` and adds that folder to your PATH in your shell's startup files
(it says which). Then open a new terminal. No `curl` (`curl: command not found`)? Install it
(`sudo apt update && sudo apt install curl` on Ubuntu and Debian), or use `wget`, if you have it,
instead. (As root, as in a container or a fresh WSL distro, leave out `sudo` in every command in
this README: such a system has none. It has no package lists yet either: run `apt update` once
before its first `apt install`.)

```bash
wget -qO- https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.sh | sh
```

### What the install scripts do, and their settings

Both download from this repository's
[Releases](https://github.com/SirOxygenFish/v2-language/releases), check the file's SHA-256, run
`v2 --version` to make sure it works on your computer before it replaces anything, and tell you if
another `v2` comes first on your PATH. Running one again updates V2 (with the same `V2_INSTALL_DIR`,
if you used one). Settings:

| Setting               | Does                                                          |
| --------------------- | ------------------------------------------------------------- |
| `V2_INSTALL_DIR`      | install into this folder instead                              |
| `V2_NO_MODIFY_PATH=1` | leave PATH alone (`true` and `yes` work too)                  |
| `V2_VERSION`          | install another release, such as `v0.1.0-alpha.2` (not with `V2_ARCHIVE`) |
| `V2_ARCHIVE`          | install from a file you downloaded (a `.sha256` file next to it is checked) |

With the one-liner, a setting goes after the pipe, where `sh` sees it:
`curl -fsSL https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.sh | V2_INSTALL_DIR=~/bin sh`.
In PowerShell, set it first in the same window: `$env:V2_INSTALL_DIR = "C:\Tools\v2"`, then the
`irm` line.

To install from files you downloaded: get the archive for your computer from
[Releases](https://github.com/SirOxygenFish/v2-language/releases) (and its `.sha256`, to have it
checked), and save the script from the address in the one-liner
([install.sh](https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.sh) or
[install.ps1](https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.ps1)) into
the same folder (in WSL, a Windows browser saves to `/mnt/c/Users/<you>/Downloads`). Then, in
that folder:

```bash
cd ~/Downloads        # in WSL: cd /mnt/c/Users/<you>/Downloads
V2_ARCHIVE=v2-x86_64-unknown-linux-gnu.tar.gz sh install.sh
```

```powershell
cd $HOME\Downloads
$env:V2_ARCHIVE = "v2-x86_64-pc-windows-msvc.zip"
powershell -ExecutionPolicy Bypass -File install.ps1
```

Run as a file, `install.ps1` shows the line that makes `v2` work in that same PowerShell window
(from cmd.exe or Git Bash, open a new window instead); new windows have it anyway.

### Install by hand

Download the file for your computer from
[Releases](https://github.com/SirOxygenFish/v2-language/releases):

| Computer                         | File                                 |
| -------------------------------- | ------------------------------------ |
| Windows (64-bit)                 | `v2-x86_64-pc-windows-msvc.zip`      |
| macOS (Apple Silicon)            | `v2-aarch64-apple-darwin.tar.gz`     |
| Linux (64-bit Intel/AMD)         | `v2-x86_64-unknown-linux-gnu.tar.gz` |

Unpack it: the folder inside has `v2` (`v2.exe` on Windows), this README and DOCS.md, the whole
language. Put `v2` in a folder that is on your PATH. On macOS, a file downloaded with a browser is blocked the first time ("can't be
opened"): run `xattr -d com.apple.quarantine v2` on it once.

### Build from source

You need Rust (stable, from [rustup.rs](https://rustup.rs)) and a C compiler: on Windows, Visual
Studio Build Tools with "Desktop development with C++"; on macOS, `xcode-select --install`; on
Linux, `sudo apt install build-essential` (or your distribution's equivalent). Then:

```bash
git clone https://github.com/SirOxygenFish/v2-language
cd v2-language
cargo install --path .
```

That puts `v2` in `~/.cargo/bin`, which rustup already put on your PATH.

### Check that it works

```bash
v2 --version
v2 doctor
```

`v2 doctor` shows whether Python and Node.js were found, with the command or the link that installs
whatever is missing (for Node.js on Linux it is a link, as distributions' own packages are often too
old: [Troubleshooting](#troubleshooting) has the steps). Its last lines sum it up, for example
`ready: py, js, ts`. The install scripts put only `v2` on your computer; to read the whole
language's guide offline, DOCS.md is in the release archive, next to `v2`.

## Quickstart

Make a folder for the examples, go into it, and create the first file. In PowerShell:

```powershell
mkdir $HOME\v2-examples
cd $HOME\v2-examples
notepad hello.v2
```

Notepad asks whether to create `hello.v2`: say yes, paste the example, save and close. (Saving
from a new Notepad window adds `.txt` unless you choose "All files" under "Save as type".) On
macOS and Linux:

```bash
mkdir -p ~/v2-examples && cd ~/v2-examples
nano hello.v2
```

Paste, then Ctrl+O and Enter to save, Ctrl+X to quit (a bare system may have no `nano`:
`apt install nano`). Any plain-text editor works, as long as it saves UTF-8.

**1. Plain V2** (needs nothing else). Save this as `hello.v2`:

```v2
let name = "world"
print(f"Hello, ${name}!")
for i in 1..4 {
    print(f"${i} squared is ${i * i}")
}
```

Run it with `v2 hello.v2`. It prints:

```text
Hello, world!
1 squared is 1
2 squared is 4
3 squared is 9
```

**2. A Python block** (needs Python). Save this as `stats.v2`:

```v2
// A function from Python's standard library, and one written in Python right here.
@import { mean, median } from py.statistics

@py tools {
    def spread(xs):
        return max(xs) - min(xs)
    @export { spread }
}
@import { spread } from tools

let data = [3, 8, 1, 9, 4]
print(f"mean ${mean(data)}, median ${median(data)}, spread ${spread(data)}")
```

Run it with `v2 stats.v2`. It prints:

```text
mean 5, median 4, spread 8
```

**3. A JavaScript block** (needs Node.js). Save this as `shout.v2`:

```v2
// A JavaScript function, called from V2 like any other.
@js text {
    function shout(s) { return s.toUpperCase() + "!" }
    @export { shout }
}
@import { shout } from text

print(shout("hello from javascript"))

// A block is also an expression: its last value comes back to V2.
let lengths = @js { ["one", "two", "three"].map(w => w.length) }
print(lengths)
```

Run it with `v2 shout.v2`. It prints:

```text
HELLO FROM JAVASCRIPT!
[3, 3, 5]
```

**4. Your own program** (needs Python). Save this as `tour.v2`:

```v2
// Functions, if/elif/else, lists, dicts and loops are V2's own.
func grade(score) {
    if score >= 90 {
        return "A"
    } elif score >= 75 {
        return "B"
    }
    return "keep going"
}

let scores = {ada: 95, grace: 82, linus: 60}
let passed = []
for name, score in scores {
    print(f"${name}: ${grade(score)}")
    if score >= 75 && name != "nobody" {
        passed.push(name)
    }
}
print(passed, len(passed), scores.get("bob") == null)

// A block sees only what you pass in: @py(name) { ... } or @js(name: value) { ... }.
let best = @py(scores) { max(scores, key=scores.get) }
print(f"best: ${best}")

// Errors from every language are caught the same way.
try {
    @py { 1 / 0 }
} catch (e) {
    print(e.type, e.foreign_type, e.message)
}
```

Run it with `v2 tour.v2`. It prints:

```text
ada: A
grace: B
linus: keep going
["ada", "grace"] 2 true
best: ada
ForeignError ZeroDivisionError division by zero
```

### Coming from Python or JavaScript

| Python, JavaScript                        | V2                                              |
| ----------------------------------------- | ----------------------------------------------- |
| `def f(x, y=2):`, `function f(x, y = 2) { }` | `func f(x, y = 2) { ... }`                   |
| `lambda x: x * 2`, `x => x * 2`           | `func(x) => x * 2` (anonymous only: a named func has a `{ }` body) |
| `function (x) { ... }` (several statements) | `func(x) { ... }`                             |
| `import util`, `import ... from "./util.js"` (a file of yours) | `import "./util.v2" as util`, then `util.f()` |
| `x = 1`, `var x = 1`, `let x = 1`         | `let x = 1` (and `const` for a constant)        |
| `True`, `False`, `None`, `null`, `undefined` | `true`, `false`, `null`                      |
| `and`, `or`, `not`                        | `&&`, `\|\|`, `!`                                |
| `f"{x}"`, `` `${x}` ``                    | `f"${x}"` (without the `f`, `${x}` is plain text) |
| `elif`, `else if`                         | `elif`                                          |
| `i += 1`, `i++`                           | `i += 1`                                        |
| `while x < 3:`, `while (x < 3) { }`       | `while x < 3 { ... }`                           |
| `for i in range(3)`                       | `for i in 0..3` (0, 1, 2), or `for i in range(3)` |
| `xs.append(x)`, `xs.push(x)`              | `xs.push(x)`                                    |
| `len(xs)`, `xs.length`                    | `len(xs)`                                       |
| `", ".join(xs)`, `xs.join(", ")`          | `xs.join(", ")`                                 |
| `s.strip()`, `s.trim()`                   | `s.trim()`                                      |
| `a // b`, `Math.floor(a / b)`             | `div(a, b)` (`//` starts a comment)             |
| `f(x, key=1)`                             | `f(x, key: 1)` (a V2 word works as the name too: `df.agg(func: "sum")`) |
| `f(**opts)`, `f(...opts)` with a dict     | named arguments come by name; for a dict of them, call it in a block: `@py(f, opts) { f(**opts) }` (`...xs` spreads a list only) |
| `{**a, **b}`, `{...a, ...b}`              | `let c = a.copy()`, then `for k, v in b { c[k] = v }` |
| `for k, v in d.items():`                  | `for k, v in d`, for a dict or a handle with `items()` such as a pandas Series (V2 never unpacks pairs: over a list, two variables are the index and the item) |
| `try: … except E as e:`, `catch (e) { }`  | `try { ... } catch (e) { ... }`                 |
| `raise ValueError("bad")`, `throw new Error("bad")` | `throw error("ValueError", "bad")`, or `throw "bad"` |
| `print(x, end="")`, `process.stdout.write(x)` | `print(x, end: "")`                         |
| `print(x, file=sys.stderr)`, `console.error(x)` | `print(x, stderr: true)`                  |
| `open(f).read()`, `fs.readFileSync(f, "utf8")` | `read_file(f)`, and `write_file(f, text)`  |
| `sys.exit(1)`, `process.exit(1)`          | `exit(1)`                                       |
| `sys.argv`, `process.argv`                | `args()` (what follows the file: `v2 x.v2 a b`); `args().get(0, "out.txt")` for one that may be missing |
| `input()`                                 | `input()` (`null` at the end of input); a number: `int(input("Age: "))` |

More of V2, in short:

- **Functions:** `int`, `float`, `str`, `round` (ties to even, as in Python: `round(2.5)` is
  `2`), `abs`, `min`, `max`, `sum`, `type`, `len`,
  `range` (a list, except as what a `for` loop goes over), `json_parse` and
  `json_stringify(x, indent: 2)`, `sleep(seconds)`, `time()` (seconds, as a float),
  `print(a, b, sep: ", ")`.
- **Files and programs:** `read_file(path)`, or `read_file(path, binary: true)` for bytes;
  `write_file(path, data)`, with `append: true` to add to the end; `exists`, `list_dir`,
  `make_dir`, `remove_file`, `getenv` (V2's own environment: a variable a block sets, with
  dotenv's `config()` or `os.environ`, stays in that language). `exec(["git", "status"], input: text)` runs a program
  without a shell and returns `{code, stdout, stderr}`, its output as text and all of it in memory.
  A program that can't start is an `IOError`. To walk folders, or tell a file from a folder, use
  Python's `os.walk` in a block. File paths are relative to the folder you run `v2` from; for a
  file next to the script, `read_file(f"${script_dir()}/data.csv")`. They are taken as written:
  `~` and `%USERPROFILE%` are a shell's, so write `getenv("HOME")` (`getenv("USERPROFILE")` on
  Windows) instead; a path whose first part is one (or, on Linux and macOS, `~name` for a user
  that exists) is refused rather than read or made as a folder named `~` (`./~` reaches a folder
  that is really named so).
- **Strings** have `split`, `lines` (Windows and old Mac line endings too), `upper`, `lower`, `replace`, `trim`,
  `starts_with`, `ends_with` and `index_of`; **lists** `push`, `pop`, `insert`,
  `sort(key: f, reverse: true)` and `reverse` (they change the list and return `null`), `map` and
  `filter` (they return new lists), `join`, `index_of`, `get(i, default)` (the default past the
  end, where `xs[i]` is an `IndexError`) and `copy`;
  **dicts**, whose keys are strings, `keys`, `values`, `get`, `remove` and `copy`. Lists and
  strings slice as `xs[1:3]`, and `x in xs`, `key in d` and `"lo" in "hello"` test membership.
- **Syntax:** `cond ? a : b` picks a value, `a ?? b` is `b` when `a` is `null`, `x?.y` is `null`
  when `x` is, `**` is a power; `f"${x:.2f}"` formats a number (`:.Nf` is the only format: pad with
  `" " * n`, and for another, such as `:.3e` or `:,`, format it in a block:
  `@py(x) { f"{x:.3e}" }`); in `"…"`, `\n`, `\t`, `\\`, `\"` and `\u{e9}` are escapes, and
  `r"C:\new"` keeps its backslashes; a `"""` at the end of a line starts a string of several lines;
  a caught error has `e.type` and `e.message`.
- **What V2 doesn't have built in** comes from the languages: random numbers with
  `@import { randint, choice } from py.random`; a program with a time limit with
  `@py(cmd) { __import__("subprocess").run(cmd, capture_output=True, text=True, timeout=5).stdout }`
  (exec has none: a program that never ends holds the script). To check a folder of scripts, loop
  in the shell: `ok=0; for f in *.v2; do v2 check "$f" || ok=1; done`, in PowerShell
  `$ok = 0; Get-ChildItem *.v2 | ForEach-Object { v2 check $_.Name; if ($LASTEXITCODE -ne 0) { $ok = 1 } }`;
  a CI job then ends with `exit $ok`, so an error in any file fails it.
- **Other `.v2` files:** their top-level names not starting with `_` are imported as a snapshot
  taken when that file finished running: read state that changes through its functions.
- **Blocks** can't see V2's variables: pass them in, as in `@py(data) { sum(data) }`, and a block's
  last expression is its value (one that ends in an assignment, `def` or `import` has the value
  `null`; using the value of one that ends in a `with`, `for`, `if` or `try` statement is an
  error: put the value on a last line of its own). A block is one line, `@py { 1 + 2 }`, or its code starts on the
  line after the `{` and its `}` stands on a line of its own; inside a larger expression it goes in
  parentheses, `(@py { 1 }) + 2`. A `@js` block whose value is an object literal needs them too,
  `@js { ({a: 1}) }`, or JavaScript reads the braces as code; and in a `@js` block of several
  lines, end a line with `;` when the next one starts with `(`, `[` or a backtick, or JavaScript
  joins the two. What a block exports, another block imports with the same `@import` line, written
  inside it. A block with `@export` runs when its language is first needed, so what its top level
  prints comes then, or never: do such work in a block without `@export`. `sys.argv` and
  `process.argv` in a block are V2's worker's: pass the script's in, `@py(a: args()) { ... }`.
  For code whose braces V2 can't match, a block can use a fence instead: three backticks after
  the header, and three backticks alone on the line that closes it.
- **From the shell:** `$1` is `args()[0]`, `$(cmd)` is `exec([...]).stdout.trim()` and `$?` its
  `.code`. `exec` starts one program, not a shell, so for a pipe, a glob, `cd dir && make` or
  `VAR=x cmd`, start a shell: `exec(["sh", "-c", "cd build && make"])`, on Windows
  `exec(["cmd", "/c", "cd /d build && make"])` (the one string after `/c` reaches cmd.exe as
  written, so quote a path as cmd does: `"cd /d \"my build\" && make"`, and a variable as
  `"set \"NODE_ENV=production\" && npm run build"`, or it keeps the space before `&&`). It returns when the program ends, with all its
  output; to see the output as it comes, run the program from a `@py` block with `subprocess.run`
  (what it prints goes to standard error). In a `@py` block, `shutil.rmtree(path)` removes a
  folder, `os.replace(a, b)` renames or moves a file, and `shutil.copy2(a, b)` copies one. On
  Linux and macOS, `read_file("/dev/stdin")` reads the rest of standard input at once, after what `input()` read (anywhere:
  `input()` in a loop until it gives `null`), and `write_file("/dev/stdout", data)` writes text
  or bytes after what `print` wrote. A program `exec` starts has no terminal, so it can't ask
  for a password (`sudo -n`, ssh keys and git's credential helpers don't need one) or show a
  full-screen program such as `vi`. What it starts stops when it ends: to leave a job running,
  `exec(["setsid", "-f", "sh", "-c", "job > job.log 2>&1 < /dev/null"])` (Linux). Pass values to
  `sh -c` as arguments, not inside the command: `exec(["sh", "-c", "cd \"$1\" && make", "sh", dir])`.

`v2 check FILE` checks a script's V2 syntax without running it. The
[docs](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md) have the whole language:
[the host language](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#3-the-host-language),
[blocks](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#4-embedded-blocks),
[sharing functions](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#6-sharing-functions-export-and-import),
[packages](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#7-importing-foreign-packages)
and a [cookbook](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#17-cookbook).

## Python and npm packages

V2 doesn't install packages: it uses the Python and Node.js that `v2 doctor` shows, with the
packages installed for them. A Python package is imported with `@import py.numpy as np` (or
`@import { mean } from py.statistics`), and a `helpers.py` next to the `.v2` file that imports it,
or in the project folder (where the `v2.toml` of the script you run is), is `py.helpers`, and a
`stages/clean.py` there is `py.stages.clean` (name yours unlike Python's own
modules, such as `json` or `numbers`: one Python serves the whole run, with one module per name,
and V2 says so when another came first). One project serves the run, too: a `.v2` file imported
from another project uses your script's `v2.toml`, venv and project folder, and only its own
folder is added for its `.py` files (npm packages are still found from each `.v2` file's own
folder upward, as Node does).

- **npm:** in your project folder, `npm install lodash` (in a Windows PowerShell that blocks
  `npm`, `npm.cmd install lodash`; in WSL, check that `type -P npm` names Linux's npm, as the
  Windows PATH that WSL adds can bring Windows' npm), then
  `@import { chunk } from js.lodash`. Names with other characters are quoted (`js."date-fns"`,
  `js."@scope/pkg"`), a default export is `@import { "default" as dayjs } from js.dayjs`, Node's
  own modules are `js.fs`, `js.path` and so on, and a file of your own next to the script is
  `@import { f } from js."./helpers.js"` (a TypeScript one, `ts."./helpers.ts"`; `js.helpers`,
  without `./`, means an npm package named helpers). If that file uses `import` or `export`
  and the folder has a `package.json` that says `"type": "commonjs"` (what `npm init` writes since
  npm 11), it needs `"type": "module"` there, or the extension `.mjs` (`.mts`). A function
  imported by name brings none of its own properties: for `dayjs.extend` or lodash's `_.chunk`,
  import the module itself, `@import js.dayjs as dayjs`. For an ES module imported whole, what
  its default export holds is under `.default`: `chalk.default.red("x")`. `js.lodash` loads a
  package the way `require()` does (a package with separate `import` and `require` builds gives
  its `require` one), and once per run, like a Python module: a script in another folder with its
  own copy in its `node_modules` gets the first one too, and V2 says so.
- **Python on Windows:** `py -m pip install numpy`. If `v2 doctor` shows another Python, use its
  path, with `&` in front in PowerShell: `& "C:\Program Files\Python313\python.exe" -m pip install
  numpy`. For a virtual environment: `py -m venv .venv`, then
  `.venv\Scripts\python -m pip install numpy`, and a file named `v2.toml` next to your script
  that says which venv to use, two lines: `[python]`, then `venv = ".venv"` (as shown below).
- **Python on Linux and macOS:** the system's and Homebrew's Python refuse `pip install` outside a
  virtual environment ("externally-managed-environment"), so make one in your project folder:

  ```bash
  python3 -m venv .venv          # on Ubuntu and Debian, first: sudo apt install python3-venv
  .venv/bin/pip install numpy
  ```

  and tell V2 to use it with a file named `v2.toml` next to your script (V2 uses the nearest
  `v2.toml` in the script's folder or a folder above it; `venv` names the folder, not its
  `python`):

  ```toml
  [python]
  venv = ".venv"
  ```

  (Activating it, `source .venv/bin/activate`, before running `v2` works too.)
- **uv:** `uv add numpy` in your project (or `uv venv`, then `uv pip install numpy`), and the same
  `v2.toml` (`[python]`, then `venv = ".venv"`); or run `uv run v2 main.v2`. uv's venvs have no
  pip: install with `uv add` or `uv pip install`.
- **conda (Anaconda, Miniconda):** run `v2` where the environment is activated
  (`conda activate sales`): V2 finds its `python` first on `PATH`, as for an activated virtual
  environment. Or name that Python in `v2.toml`, without `venv`, which is for virtual
  environments only: `[engines.py]` and `command = ['C:\Users\you\miniconda3\envs\sales\python.exe']`
  (single quotes keep the backslashes). Activated is the safer way: some conda packages find
  their DLLs through what activation puts on `PATH`. A `conda run` command needs
  `--no-capture-output` after `run`, as V2 talks to Python over its input and output:
  `command = ["conda", "run", "--no-capture-output", "-n", "sales", "python"]`.

Python and Node.js start when the script first needs them, and then every package a file imports
from that language loads, in order: at the first call of an imported name (for
`@import py.numpy as np`, the first use of `np.something`), not at the `@import` lines. Once that
language runs, a later `@import` loads its package at its own line. A name used
as a value (`let f = mean`, `print(mean)`) loads its package at its `@import` line instead. What a
package prints when it loads, and the error for a misspelled package name, come at that point.

## Troubleshooting

**An `if` that compares what `input()` gave with a number is never true.** `input()` gives text,
and values of different types are never equal: `"7" == 7` is `false`. Compare `int(answer) == 7`,
or `answer == "7"`.

**`v2` is not recognized, or `command not found`.** Open a new terminal: the PATH change reaches
only terminals opened after the install. Still not found? Check that the folder is on your PATH
(Windows: `$env:Path -split ';'`; macOS and Linux: `echo $PATH`), or run V2 by its full path
(`~/.v2/bin/v2`; in PowerShell, `& "$env:USERPROFILE\.v2\bin\v2.exe"`). Changed your shell (to zsh
or fish) after installing? Run the install command again in a new terminal of that shell, once
it is your login shell (after `chsh`, log out and in first): it sets up the shell that `$SHELL`
names.

**`EngineNotFound: engine 'py' (Python) not found`** (or `'js'`, Node.js). Install it with the
command the message shows, open a new terminal, and run `v2 doctor`. On Windows, if Python is
installed but still not found, run its installer again and tick "Add python.exe to PATH". For
Node.js on Linux, the steps are in the next entry: distributions' own packages are often too old.

**`... is too old: found 3.9.6, V2 needs 3.10+`.** macOS's own `python3` is 3.9: install a newer
one (`brew install python`, or [python.org](https://www.python.org/downloads/)). For Node.js,
Linux distributions often ship older versions than 22.13 (`apt install nodejs` gives 18 on Ubuntu
24.04): get Node.js 22 or newer from [nodejs.org](https://nodejs.org/en/download). One way there:
its Linux `.tar.xz`, checked and unpacked into `/opt`, from a terminal (leave out `sudo` if you
are root):

```bash
sudo apt install curl xz-utils
curl -fsSLO https://nodejs.org/dist/latest-v22.x/SHASUMS256.txt
f=$(grep -o 'node-v22[^ ]*-linux-x64.tar.xz' SHASUMS256.txt) && echo "$f"
curl -fsSLO "https://nodejs.org/dist/latest-v22.x/$f"
grep " $f\$" SHASUMS256.txt | sha256sum -c -
sudo tar -xf "$f" -C /opt
```

Then put its `bin` folder (the name `echo` showed, without `.tar.xz`) on PATH in your
shell's startup files (bash: `echo 'export PATH="/opt/node-v22.21.0-linux-x64/bin:$PATH"' |
tee -a ~/.bashrc ~/.profile`, with the version you unpacked; zsh: `~/.zshenv`; fish:
`fish_add_path /opt/node-v22.21.0-linux-x64/bin`), then a new terminal. Or, whatever the shell,
name it in a `v2.toml` next to your script (a section's name on a line of its own, its keys below):

```toml
[engines.js]
command = ["/opt/node-v22.21.0-linux-x64/bin/node"]
```

nvm, which the site also offers, sets up interactive shells only. `v2 doctor` shows which version
V2 found, and where.

**`ImportError: No module named 'numpy'`** (or `Cannot find module 'lodash'`). Install the package:
[Python and npm packages](#python-and-npm-packages).

**`NameError: name 'x' is not defined` from a block** (Python), or `x is not defined`
(JavaScript). Blocks don't see V2's variables: pass `x` in, `@py(x) { ... }`.

**`SyntaxError: file is not valid UTF-8`.** Save the file as UTF-8. Windows PowerShell 5.1's `>`
and `Out-File` write UTF-16, and its `Set-Content` the old ANSI code page: use an editor, or add
`-Encoding utf8`.

**Non-ASCII text turns into `├╝`, `Ã©` or `?` when piped in Windows PowerShell 5.1, or `input()`
says `input line 1 is not valid UTF-8`.** It decodes and encodes what passes through pipes with
the console's old code page. Run
`[Console]::OutputEncoding = $OutputEncoding = [Text.UTF8Encoding]::new()` first in that window,
or use PowerShell 7; and read a file with `Get-Content -Encoding UTF8 file.txt`, as 5.1 reads one
without a byte-order mark in the old code page. In cmd.exe, run `chcp 65001` first. V2's standard
input, output and error are UTF-8 on every OS, whatever the console's code page, so a program that
runs v2 reads and sends UTF-8 (in Python, `encoding="utf-8"`). On Linux and
macOS, V2 and its blocks always read and write UTF-8, whatever `LANG` says: use a UTF-8 locale
(`C.UTF-8`, `de_DE.UTF-8`), or convert with `iconv`.

**`UnicodeDecodeError` when a `@py` block reads a file that plain Python reads.** Blocks run in
Python's UTF-8 mode, so `open()` reads UTF-8 unless told otherwise: for an Excel CSV, pass the
code page of the Windows that saved it, `open(path, encoding="cp1252")` for Western European
text (`cp1250` Central European, `cp1251` Cyrillic, `cp1254` Turkish, `cp1255` Hebrew, `cp1256`
Arabic, `gbk` Chinese, `cp932` Japanese).

**`cannot find 'hello.v2'`.** Your terminal is in another folder: `cd` to the folder with the file,
or give its full path. The message says where V2 looked and which `.v2` files are there.

**`unexpected character U+00A0 (a no-break space)`, `'“' (a curly quote)`.** Code copied from a
web page or a chat can bring characters that look like spaces or quotes: retype them.

**`v2 doctor` says node `needs libatomic.so.1`.** Node.js's own Linux download needs a library
that minimal systems lack: `sudo apt install libatomic1`. On such a system (a container, a fresh
WSL distro) you may be root without `sudo`: leave it out; unpacking Node.js's `.tar.xz` also needs
`apt install xz-utils`.

**`v2` works in a terminal but not in cron or `ssh host v2 ...`.** Those may not read your shell's
startup files: use the full path, `~/.v2/bin/v2`. A Python or Node.js that your startup files put
on PATH isn't found there either: name it in a `v2.toml` as in the entry on Node.js above
(`command = ["/opt/node/bin/node"]` on the line under `[engines.js]`), or with `V2_ENGINE_JS`.

**macOS says `v2` "can't be opened".** Only files downloaded with a browser are blocked:
`xattr -d com.apple.quarantine ~/.v2/bin/v2`.

**PowerShell says running scripts is disabled.** The `irm ... | iex` line works anyway. If you
saved `install.ps1` as a file, run it with `powershell -ExecutionPolicy Bypass -File install.ps1`.
For `npm` ("npm.ps1 cannot be loaded"), type `npm.cmd` instead. A virtual environment needs no
`Activate.ps1`: the `v2.toml` above finds it.

**`v2: Permission denied`, or the installer says a folder "doesn't let programs run".** That
folder's file system is mounted `noexec`, as servers often have it: install into another folder
with `V2_INSTALL_DIR`.

**`Fatal process out of memory`, `memory allocation of … bytes failed`, or `left V2 a smaller
stack`.** A memory limit is set for this shell (`ulimit -v`, common on shared servers). Plain
scripts need little, but deep recursion needs about 300 MB, and `@js` blocks about 1 GB, as
Node.js reserves that much when it starts. Raise the limit (`ulimit -v unlimited`, where that's
allowed) or ask the server's admin.

**A script hangs.** Press Ctrl+C: V2 stops it, and its Python and Node.js workers, with exit
code 130 (on Linux and macOS, v2 ends by the signal, as other programs do, so a shell loop of
`v2` runs stops too; `kill` shows 143).

## FAQ

**Do I need Python or Node.js?** Only for the blocks and packages of that language. `v2 doctor`
says what's ready.

**Which Python and Node.js does V2 use?** The first suitable `python3`, `python` (on Windows also
`py -3`) and `node` on your PATH, unless a `v2.toml` says otherwise; `v2 doctor`, run in the
script's folder, shows them. To pick one, put a `v2.toml` in your
project folder ([docs, chapter 14](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#14-project-file-v2toml)),
each section's name on a line of its own, its keys on the lines below it:

```toml
[python]
venv = ".venv"                              # a virtual environment, or else:

[engines.py]
command = ["C:/Python313/python.exe"]       # on macOS, for example, ["/opt/homebrew/bin/python3"]
```

(one of the two: a venv and a command for Python together are an error).
The variable `V2_ENGINE_PY` (or `V2_ENGINE_JS`) set to a
program's path does the same while it is set: in PowerShell, for that window,
`$env:V2_ENGINE_PY = "C:\Program Files\Python313\python.exe"` (and `Remove-Item Env:V2_ENGINE_PY`
ends it); in a shell, `export V2_ENGINE_PY=/opt/homebrew/bin/python3`.

V2 doesn't read `.python-version`, `.nvmrc` or `.tool-versions`. A version manager's shim on PATH
(pyenv, asdf, fnm) picks the version for the folder you run `v2` from, not the script's folder:
run `v2` from the project folder, or pin the program in `v2.toml`; `v2 doctor`'s COMMAND column
shows the shim, and VERSION the version it picks there.

**Do TypeScript blocks check my types?** No: `@ts` blocks run on Node.js with the types removed,
so `add("2", 3)` runs. Run `tsc` yourself for checking. Only TypeScript whose types can simply be
removed runs: not decorators, and not `enum`, `namespace`, `constructor(public x)` parameter
properties, `import x = require(…)` or `<T>x` casts, whose errors' hints say what to write
instead (`x as T` for a cast). Imports between your `.ts` files name the `.ts` file
(`./math.ts`, not tsc's `./math.js` or `./math`), and a type is imported with `import type`.
Node reads no `tsconfig.json`, so a `"paths"` alias needs a relative path instead, and it has no
JSX: compile `.tsx` and `.jsx` files first.

**Does V2 leave files around?** No. Temporary files go to a private folder in your temp directory
and are removed when V2 exits (if V2 was killed, the next script that runs a block removes
them); caches live in
`~/.v2/cache`. Nothing is written to your project.

**Is a `.v2` file safe to run?** Like any script, it runs with your rights, every block in every
language: only run scripts you trust ([docs, chapter 16](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#16-security-and-trust)).
A run trusts some files near the script as much: the nearest `v2.toml` in its folder or a folder
above (it can name the programs that run the blocks), the modules Python imports from the
script's folder and the project folder, and the `node_modules` and `package.json` Node finds
there and in the folders above. So don't keep or run scripts under a folder that other users can
write to, such as `/tmp`, or on Windows `C:\Users\Public` and folders made directly under `C:\`
(such as `C:\work`), which every user can write to by default: keep projects under your user folder.

**Is it fast?** V2's own code is a small interpreter, fine for gluing things together. Heavy
loops belong in a Python or JavaScript block (and, later, in C or Rust blocks). Each call from one
language to another takes about a tenth of a millisecond: pass a whole list to one call rather than
calling once per item. Each run of a `@js` block without `@export` also makes a new JavaScript
realm, about 0.4 ms (a `@py` one takes about 0.06 ms): in a loop, export a function from a `@js`
block and call it, or pass the whole list in once, `@js(xs) { xs.map(x => x * 2) }`. Each line
a block prints crosses over to V2 too (a Python block prints about a hundred thousand lines a
second): for a lot of output, build the text in the block and print it once, or return it.

**Can blocks use async JavaScript?** Yes: a function that returns a Promise (an `async` function),
or a block whose value is a Promise, is awaited, and you get its result; a rejection is an error you
can catch. A block can't use `await` at its top level: end it with a Promise, or export an `async`
function. One exception: while JavaScript is itself waiting on V2, as when it calls a V2
function from a route handler or a callback, that V2 function can't wait on async JavaScript;
write such handlers in JavaScript, and call V2 for the parts that don't wait. An HTTP server in a
block serves only while V2 waits on it:
[the cookbook has one](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#176-an-http-server-in-a-javascript-block).

**How do I catch a V2 error in Python or JavaScript?** It arrives as a `V2Error`
(`except Exception as e`, `catch (e)`): `e.type` is its V2 type (`"ValueError"`), `e.message` its
text. Uncaught, it goes back to V2 as it was.

**What is `VEP/1` in `v2 --version`?** The version of the protocol V2 speaks with its Python and
Node.js workers.

**Is there editor support?** Not yet: `.v2` files are plain text in any editor.

## Known limitations

- **Not built yet:** C, C++ and Rust blocks (`@c`, `@cpp`, `@rust`), `extern` for native
  libraries, custom engines, the shell and other run engines (`@sh`, `@pwsh`, `@lua`, ...), and
  `v2 cache clean`. The docs describe them and mark each one "Not built yet"; using one says so.
- **Ready-made downloads** exist for the three computers listed above. Others, Intel Macs
  included, build from source.
- **The downloads aren't code-signed**, so a browser may warn about them. The install commands
  don't get those warnings.
- **Values cross between languages as copies** (numbers, strings, lists, dicts) or as handles to
  the foreign object; one value can be at most 64 MiB, and a list or dict nest at most 256 deep
  (deeper parts of a value from Python or JavaScript arrive as handles). `to_v2` of a large
  DataFrame or array converts all of it before it finds it too big, which takes time and memory:
  try a slice first (`df.head(1000)`), and keep the rest in its block.
  V2's integers are 64-bit: a bigger Python or JavaScript integer arrives as a handle, so do big
  number math inside the block. In JavaScript, a V2 integer from ±2⁵³ on is a `BigInt` (`x + 1`
  fails there: write `x + 1n`), a whole number comes back as an integer (`2.0` as `2`), and dict
  keys that look like numbers come back first, as JavaScript orders them. A handle works only in its own language: `to_v2(h)` makes plain
  data that any language takes. Operators don't work on handles (call a method, or do it in the
  block), a handle is always true, and `==` on two handles means the same object; a handle is
  never equal to a number or a string, so a `Decimal` from a database is never `== 0`: compare
  in its block, `@py(amount) { amount == 0 }`.
- **A V2 function given to Python or JavaScript** can be called whenever V2 is in a call to that
  language (one kept for later works in later calls), on its main thread: not from another
  thread, and not from a timer or an event that fires while V2 runs its own code.
- **JavaScript's event loop runs only while V2 is in a call to JavaScript:** a server, timer or
  socket in a `@js` block waits in between. Output a Python or JavaScript thread prints in
  between shows at the next call, or when the program ends. When the script ends, what
  JavaScript timers and promises and Python threads still have to do runs, for at most a second.
- **A block's functions and classes can't go to another process:** `multiprocessing` with spawn
  (the default on Windows and macOS) or forkserver (on Linux from Python 3.14) can't import them:
  each worker prints why, and a `Pool` using them hangs (press Ctrl+C). A pickle of a block's
  class loads only in the same run. Put them in a `.py` file next to the script, and import that
  in the block.
- **Each block has its own variables**, also in JavaScript: share them with `@export`. Each `@js`
  block is a JavaScript realm of its own, so a list or dict from V2 isn't `instanceof Array` or
  `Object` there, nor a V2 error `instanceof Error`: test with `Array.isArray(x)` and `e.type`.
- **Blocks can't read standard input:** V2's `input()` owns it. Read the input in V2 and pass it
  in.
- **Programs a block starts print to standard error**, not standard output (`subprocess.run`,
  `os.system`, `child_process`), so `v2 x.v2 > out.txt` doesn't catch what they print. Capture
  their output in the block and print it, or run them with V2's own `exec(["git", "status"])`,
  which returns `{code, stdout, stderr}`.
- **Output from blocks is text:** bytes that aren't UTF-8 come out as U+FFFD, so write binary data
  to a file.
- **A class imported from a block is a function here:** you can make objects and call their
  methods, but not reach the class's own attributes; export a function that uses them.
- **Calls between languages nest at most 64 deep.**
- **`v2 check` checks V2's syntax only:** names, engine tags and the code inside blocks are checked
  when the script runs.
- **A `#!` first line is an error:** delete it, and run scripts with `v2 file.v2`.
- **Ctrl+C and `exit(n)` stop at once:** `finally` blocks don't run, in any language. After
  `exit(n)` Python and Node.js still end cleanly (open files are flushed, `atexit` runs). Ctrl+C,
  or a kill such as `docker stop`, stops them on the spot: what an open file still buffers is
  lost and `atexit` doesn't run, so close files as you go (`with open(...)` in each call).
- **An error V2 can't raise in your program** (from a JavaScript timer, a Promise nobody waits
  for, a Python thread) is printed, and the run then ends with exit code 1, not 0.
- **No REPL, no package manager of its own, no editor plugins.**
- **It's an alpha:** syntax and behavior may still change before 0.1.0.

## Uninstall

1. Delete the folder `.v2` in your home folder (`%USERPROFILE%\.v2` on Windows): it holds V2's
   caches, and V2 itself unless you installed with `V2_INSTALL_DIR`. If you did, also delete just
   `v2` (`v2.exe`, and a `v2.exe.old` if there is one) from that folder, not the folder. If V2
   was ever killed, also delete the `v2-…` folders it left in your temp folder (`%TEMP%` on
   Windows, `/tmp` or `$TMPDIR` elsewhere).
2. Take it off your PATH. Windows: Settings, search "Edit environment variables for your account",
   select `Path`, delete the `.v2\bin` entry. macOS and Linux: in each startup file `install.sh`
   changed (`~/.bashrc`, `~/.profile` or `~/.bash_profile`, `~/.zshenv`; `grep -ls 'added by
   install.sh' ~/.bashrc ~/.profile ~/.bash_profile ~/.zshenv` lists them), delete the line
   `# V2 (added by install.sh)` and the line under it (and `~/.zshenv`, if that leaves it empty);
   with fish, delete
   `~/.config/fish/conf.d/v2.fish` (under `$XDG_CONFIG_HOME` instead, if you set it).

## Reporting bugs

[Open an issue](https://github.com/SirOxygenFish/v2-language/issues/new/choose) and pick "Bug
report". Include the smallest script that shows the problem, what you expected, what happened,
and the output of `v2 doctor` (it includes V2's version and your system).
[CONTRIBUTING.md](https://github.com/SirOxygenFish/v2-language/blob/main/CONTRIBUTING.md) explains
how to build V2 and run its tests.

## Read more

- [DOCS.md](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md): the language, from
  the first script to the cookbook
- [INTERNALS.md](https://github.com/SirOxygenFish/v2-language/blob/main/INTERNALS.md): how it
  works, for contributors
- [ROADMAP.md](https://github.com/SirOxygenFish/v2-language/blob/main/ROADMAP.md): what's done and
  what's next

V2 is under the [MIT license](https://github.com/SirOxygenFish/v2-language/blob/main/LICENSE).
