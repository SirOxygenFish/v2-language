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

### macOS and Linux

Open a terminal, paste this line and press Enter:

```bash
curl -fsSL https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.sh | sh
```

It puts `v2` in `~/.v2/bin` and adds that folder to your PATH in your shell's startup files
(it says which). Then open a new terminal. No `curl` (`curl: command not found`)? Install it
(`sudo apt update && sudo apt install curl` on Ubuntu and Debian), or use `wget` instead:

```bash
wget -qO- https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.sh | sh
```

### What the install scripts do, and their settings

Both download from this repository's
[Releases](https://github.com/SirOxygenFish/v2-language/releases), check the file's SHA-256, run
`v2 --version` to make sure it works on your computer before it replaces anything, and tell you if
another `v2` comes first on your PATH. Running one again updates V2. Settings:

| Setting               | Does                                                          |
| --------------------- | ------------------------------------------------------------- |
| `V2_INSTALL_DIR`      | install into this folder instead                              |
| `V2_NO_MODIFY_PATH=1` | leave PATH alone (`true` and `yes` work too)                  |
| `V2_VERSION`          | install another release, such as `v0.1.0-alpha.2`             |
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
the same folder. Then, in that folder:

```bash
cd ~/Downloads
V2_ARCHIVE=v2-x86_64-unknown-linux-gnu.tar.gz sh install.sh
```

```powershell
cd $HOME\Downloads
$env:V2_ARCHIVE = "v2-x86_64-pc-windows-msvc.zip"
powershell -ExecutionPolicy Bypass -File install.ps1
```

Run as a file, `install.ps1` shows the line that makes `v2` work in that same window; new windows
have it anyway.

### Install by hand

Download the file for your computer from
[Releases](https://github.com/SirOxygenFish/v2-language/releases):

| Computer                         | File                                 |
| -------------------------------- | ------------------------------------ |
| Windows (64-bit)                 | `v2-x86_64-pc-windows-msvc.zip`      |
| macOS (Apple Silicon)            | `v2-aarch64-apple-darwin.tar.gz`     |
| Linux (64-bit Intel/AMD)         | `v2-x86_64-unknown-linux-gnu.tar.gz` |

Unpack it: the folder inside has `v2` (`v2.exe` on Windows). Put that in a folder that is on
your PATH. On macOS, a file downloaded with a browser is blocked the first time ("can't be
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

`v2 doctor` shows whether Python and Node.js were found, with the command that installs whatever
is missing. Its last lines sum it up, for example `ready: py, js, ts`.

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
mkdir ~/v2-examples && cd ~/v2-examples
nano hello.v2
```

Paste, then Ctrl+O and Enter to save, Ctrl+X to quit. Any plain-text editor works, as long as it
saves UTF-8.

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
    print(e.type, e.foreign_type)
}
```

Run it with `v2 tour.v2`. It prints:

```text
ada: A
grace: B
linus: keep going
["ada", "grace"] 2 true
best: ada
ForeignError ZeroDivisionError
```

### Coming from Python or JavaScript

| Python, JavaScript                        | V2                                              |
| ----------------------------------------- | ----------------------------------------------- |
| `def f(x, y=2):`, `function f(x, y = 2) { }` | `func f(x, y = 2) { ... }`                   |
| `lambda x: x * 2`, `x => x * 2`           | `func(x) => x * 2`                              |
| `x = 1`, `var x = 1`, `let x = 1`         | `let x = 1` (and `const` for a constant)        |
| `True`, `False`, `None`, `null`, `undefined` | `true`, `false`, `null`                      |
| `and`, `or`, `not`                        | `&&`, `\|\|`, `!`                                |
| `f"{x}"`, `` `${x}` ``                    | `f"${x}"`                                       |
| `elif`, `else if`                         | `elif`                                          |
| `i += 1`, `i++`                           | `i += 1`                                        |
| `while x < 3:`, `while (x < 3) { }`       | `while x < 3 { ... }`                           |
| `for i in range(3)`                       | `for i in 0..3` (0, 1, 2)                       |
| `xs.append(x)`, `xs.push(x)`              | `xs.push(x)`                                    |
| `len(xs)`, `xs.length`                    | `len(xs)`                                       |
| `", ".join(xs)`, `xs.join(", ")`          | `xs.join(", ")`                                 |
| `s.strip()`, `s.trim()`                   | `s.trim()`                                      |
| `a // b`, `Math.floor(a / b)`             | `div(a, b)` (`//` starts a comment)             |
| `f(x, key=1)`                             | `f(x, key: 1)`                                  |
| `try: … except E as e:`, `catch (e) { }`  | `try { ... } catch (e) { ... }`                 |
| `raise ValueError("bad")`, `throw new Error("bad")` | `throw error("ValueError", "bad")`, or `throw "bad"` |
| `print(x, end="")`, `process.stdout.write(x)` | `print(x, end: "")`                         |
| `open(f).read()`, `fs.readFileSync(f, "utf8")` | `read_file(f)`, and `write_file(f, text)`  |
| `sys.exit(1)`, `process.exit(1)`          | `exit(1)`                                       |
| `sys.argv`, `process.argv`                | `args()` (what follows the file: `v2 x.v2 a b`) |
| `input()`                                 | `input()` (`null` at the end of input)          |

A block can't see V2's variables: pass them in, as in `@py(data) { sum(data) }`, and its last
expression is its value. File paths are relative to the folder you run `v2` from; for a file next
to the script, `read_file(f"${script_dir()}/data.csv")`. `v2 check FILE` checks a script's V2
syntax without running it. The
[docs](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md) have the whole language:
[the host language](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#3-the-host-language),
[blocks](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#4-embedded-blocks),
[sharing functions](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#6-sharing-functions-export-and-import),
[packages](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#7-importing-foreign-packages)
and a [cookbook](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#17-cookbook).

## Python and npm packages

V2 doesn't install packages: it uses the Python and Node.js that `v2 doctor` shows, with the
packages installed for them.

- **npm:** in your project folder, `npm install lodash`, then
  `@import { chunk } from js.lodash`. Names with other characters are quoted (`js."date-fns"`,
  `js."@scope/pkg"`), a default export is `@import { "default" as dayjs } from js.dayjs`, Node's
  own modules are `js.fs`, `js.path` and so on, and a file of your own next to the script is
  `@import { f } from js."./helpers.js"`.
- **Python on Windows:** `py -m pip install numpy`, then `@import py.numpy as np`. If
  `v2 doctor` shows another Python, use its path, with `&` in front in PowerShell:
  `& "C:\Program Files\Python313\python.exe" -m pip install numpy`. A virtual environment works
  as below, with `py -m venv .venv` and `.venv\Scripts\pip install numpy`. A `helpers.py` next
  to the script is `py.helpers`.
- **Python on Linux and macOS:** the system's and Homebrew's Python refuse `pip install` outside a
  virtual environment ("externally-managed-environment"), so make one in your project folder:

  ```bash
  python3 -m venv .venv          # on Ubuntu and Debian, first: sudo apt install python3-venv
  .venv/bin/pip install numpy
  ```

  and tell V2 to use it with a file named `v2.toml` next to your script:

  ```toml
  [python]
  venv = ".venv"
  ```

  (Activating it, `source .venv/bin/activate`, before running `v2` works too.)

## Troubleshooting

**`v2` is not recognized, or `command not found`.** Open a new terminal: the PATH change reaches
only terminals opened after the install. Still not found? Check that the folder is on your PATH
(Windows: `$env:Path -split ';'`; macOS and Linux: `echo $PATH`), or run V2 by its full path
(`~/.v2/bin/v2`; in PowerShell, `& "$env:USERPROFILE\.v2\bin\v2.exe"`).

**`EngineNotFound: engine 'py' (Python) not found`** (or `'js'`, Node.js). Install it with the
command the message shows, open a new terminal, and run `v2 doctor`. On Windows, if Python is
installed but still not found, run its installer again and tick "Add python.exe to PATH".

**`... is too old: found 3.9.6, V2 needs 3.10+`.** macOS's own `python3` is 3.9: install a newer
one (`brew install python`, or [python.org](https://www.python.org/downloads/)). For Node.js,
Linux distributions often ship older versions than 22.13 (`apt install nodejs` gives 18 on Ubuntu
24.04): get Node.js 22 or newer from [nodejs.org](https://nodejs.org/en/download). `v2 doctor`
shows which version V2 found, and where.

**`ImportError: No module named 'numpy'`** (or `Cannot find module 'lodash'`). Install the package:
[Python and npm packages](#python-and-npm-packages).

**`NameError: name 'x' is not defined` from a block** (Python), or `x is not defined`
(JavaScript). Blocks don't see V2's variables: pass `x` in, `@py(x) { ... }`.

**`SyntaxError: file is not valid UTF-8`.** Save the file as UTF-8. In Windows PowerShell 5, `>`
and `Out-File` write UTF-16: use an editor, or `Set-Content -Encoding utf8`.

**`cannot find 'hello.v2'`.** Your terminal is in another folder: `cd` to the folder with the file,
or give its full path. The message says where V2 looked and which `.v2` files are there.

**`unexpected character U+00A0 (a no-break space)`, `'“' (a curly quote)`.** Code copied from a
web page or a chat can bring characters that look like spaces or quotes: retype them.

**`v2 doctor` says Node.js `doesn't run`, with `libatomic.so.1`.** Node.js's own Linux download
needs a library that minimal systems lack: `sudo apt install libatomic1`.

**`v2` works in a terminal but not in cron or `ssh host v2 ...`.** Those may not read your shell's
startup files: use the full path, `~/.v2/bin/v2`.

**macOS says `v2` "can't be opened".** Only files downloaded with a browser are blocked:
`xattr -d com.apple.quarantine ~/.v2/bin/v2`.

**PowerShell says running scripts is disabled.** The `irm ... | iex` line works anyway. If you
saved `install.ps1` as a file, run it with `powershell -ExecutionPolicy Bypass -File install.ps1`.
For `npm` ("npm.ps1 cannot be loaded"), type `npm.cmd` instead. A virtual environment needs no
`Activate.ps1`: the `v2.toml` above finds it.

**A script hangs.** Press Ctrl+C: V2 stops it, and its Python and Node.js workers, with exit
code 130.

## FAQ

**Do I need Python or Node.js?** Only for the blocks and packages of that language. `v2 doctor`
says what's ready.

**Which Python and Node.js does V2 use?** The first suitable `python3`, `python` (on Windows also
`py -3`) and `node` on your PATH; `v2 doctor` shows them. To pick one, put a `v2.toml` in your
project folder ([docs, chapter 14](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#14-project-file-v2toml)):
`[python]` `venv = ".venv"` for a virtual environment, or `[engines.py]`
`command = ["C:/Python313/python.exe"]` (on macOS, for example, `["/opt/homebrew/bin/python3"]`).
The variable `V2_ENGINE_PY` (or `V2_ENGINE_JS`) set to a
program's path, without quotes, does the same for one run.

**Do TypeScript blocks check my types?** No: `@ts` blocks run on Node.js with the types removed,
so `add("2", 3)` runs. Run `tsc` yourself for checking. `enum` and `namespace` aren't supported
there.

**Does V2 leave files around?** No. Temporary files go to a private folder in your temp directory
and are removed when V2 exits (if V2 was killed, the next script that runs a block removes
them); caches live in
`~/.v2/cache`. Nothing is written to your project.

**Is a `.v2` file safe to run?** Like any script, it runs with your rights, every block in every
language: only run scripts you trust ([docs, chapter 16](https://github.com/SirOxygenFish/v2-language/blob/main/DOCS.md#16-security-and-trust)).

**Is it fast?** V2's own code is a small interpreter, fine for gluing things together. Heavy
loops belong in a Python or JavaScript block (and, later, in C or Rust blocks). Each call from one
language to another takes about a tenth of a millisecond: pass a whole list to one call rather than
calling once per item.

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
  the foreign object; one value can be at most 64 MiB, and a list or dict nest at most 256 deep.
  V2's integers are 64-bit: a bigger Python or JavaScript integer arrives as a handle, so do big
  number math inside the block. A handle works only in its own language: `to_v2(h)` makes plain
  data that any language takes (functions are the exception: they can be passed anywhere).
- **Blocks can't read standard input:** V2's `input()` owns it. Read the input in V2 and pass it
  in.
- **Output from blocks is text:** bytes that aren't UTF-8 come out as U+FFFD, so write binary data
  to a file.
- **A class imported from a block is a function here:** you can make objects and call their
  methods, but not reach the class's own attributes; export a function that uses them.
- **Calls between languages nest at most 64 deep.**
- **`v2 check` checks V2's syntax only:** names, engine tags and the code inside blocks are checked
  when the script runs.
- **A `#!` first line is an error:** delete it, and run scripts with `v2 file.v2`.
- **No REPL, no package manager of its own, no editor plugins.**
- **It's an alpha:** syntax and behavior may still change before 0.1.0.

## Uninstall

1. Delete the folder `.v2` in your home folder (`%USERPROFILE%\.v2` on Windows): it holds V2
   and its caches. If you installed with `V2_INSTALL_DIR`, delete just `v2` (`v2.exe`) from that
   folder, not the folder.
2. Take it off your PATH. Windows: Settings, search "Edit environment variables for your account",
   select `Path`, delete the `.v2\bin` entry. macOS and Linux: in each startup file `install.sh`
   named, delete the line `# V2 (added by install.sh)` and the line under it; with fish, delete
   `~/.config/fish/conf.d/v2.fish`.

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
