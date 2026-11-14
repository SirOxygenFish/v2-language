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

It puts `v2` in `~/.v2/bin` and adds that folder to your PATH in your shell's startup file
(`~/.zshrc`, `~/.bashrc`, `~/.bash_profile`, fish's `conf.d`, or `~/.profile`). Then open a new
terminal. No `curl` (`curl: command not found`)? Install it (`sudo apt install curl` on Ubuntu and
Debian), or use `wget` instead:

```bash
wget -qO- https://raw.githubusercontent.com/SirOxygenFish/v2-language/main/install.sh | sh
```

Both scripts download from this repository's
[Releases](https://github.com/SirOxygenFish/v2-language/releases), check the file's SHA-256, run
`v2 --version` to make sure it works on your computer, and tell you if an older `v2` comes first
on your PATH. Running one again updates V2. To change what they do, set these first:
`V2_INSTALL_DIR` (another folder), `V2_NO_MODIFY_PATH=1` (leave PATH alone), `V2_VERSION` (another
release), `V2_ARCHIVE` (install from a file you downloaded).

### Install by hand

Download the file for your computer from
[Releases](https://github.com/SirOxygenFish/v2-language/releases):

| Computer                         | File                                 |
| -------------------------------- | ------------------------------------ |
| Windows (64-bit)                 | `v2-x86_64-pc-windows-msvc.zip`      |
| macOS (Apple Silicon)            | `v2-aarch64-apple-darwin.tar.gz`     |
| Linux (64-bit Intel/AMD)         | `v2-x86_64-unknown-linux-gnu.tar.gz` |

Unpack it and put `v2` (`v2.exe` on Windows) in a folder that is on your PATH. On macOS, a file
downloaded with a browser is blocked the first time ("can't be opened"): run
`xattr -d com.apple.quarantine v2` on it once.

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

Make a folder for these examples and open a terminal in it. Save each file with a plain-text
editor (Notepad, VS Code, TextEdit in plain-text mode), as UTF-8, with exactly the name shown:
Notepad adds `.txt` unless you choose "All files" under "Save as type".

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

Next: [DOCS.md](DOCS.md) is the whole language, from the first script to a cookbook. Good places to
continue: [the host language](DOCS.md#3-the-host-language), [blocks](DOCS.md#4-embedded-blocks),
[sharing functions](DOCS.md#6-sharing-functions-export-and-import) and
[Python and npm packages](DOCS.md#7-importing-foreign-packages).

## Troubleshooting

**`v2` is not recognized, or `command not found`.** Open a new terminal: the PATH change reaches
only terminals opened after the install. Still not found? Check that the folder is on your PATH
(Windows: `$env:Path -split ';'`; macOS and Linux: `echo $PATH`), or run V2 by its full path
(`~/.v2/bin/v2`, or `%USERPROFILE%\.v2\bin\v2.exe`).

**`EngineNotFound: engine 'py' (Python) not found`** (or `'js'`, Node.js). Install it with the
command the message shows, open a new terminal, and run `v2 doctor`. On Windows, if Python is
installed but still not found, run its installer again and tick "Add python.exe to PATH".

**`... is too old: found 3.9.6, V2 needs 3.10+`.** macOS's own `python3` is 3.9: install a newer
one (`brew install python`, or [python.org](https://www.python.org/downloads/)). For Node.js,
Linux distributions often ship older versions than 22.13 (`apt install nodejs` gives 18 on Ubuntu
24.04): get Node.js 22 or newer from [nodejs.org](https://nodejs.org/en/download). `v2 doctor`
shows which version V2 found, and where.

**`ImportError: ... No module named 'numpy'`** (or a missing npm package). V2 doesn't install
packages. Install them for the Python that `v2 doctor` shows (`python -m pip install numpy`), or
with `npm install lodash` in your project folder. Chapter 7 of the docs covers virtual environments.

**`SyntaxError: file is not valid UTF-8`.** Save the file as UTF-8. In Windows PowerShell 5, `>`
and `Out-File` write UTF-16: use an editor, or `Set-Content -Encoding utf8`.

**`cannot find 'hello.v2'`.** Your terminal is in another folder: `cd` to the folder with the file,
or give its full path. The message says where V2 looked and which `.v2` files are there.

**macOS says `v2` "can't be opened".** Only files downloaded with a browser are blocked:
`xattr -d com.apple.quarantine ~/.v2/bin/v2`.

**PowerShell says running scripts is disabled.** The `irm ... | iex` line works anyway. If you
saved `install.ps1` as a file, run it with `powershell -ExecutionPolicy Bypass -File install.ps1`.

**A script hangs.** Press Ctrl+C: V2 stops it, and its Python and Node.js workers, with exit
code 130.

## FAQ

**Do I need Python or Node.js?** Only for the blocks and packages of that language. `v2 doctor`
says what's ready.

**Which Python and Node.js does V2 use?** The first suitable `python3`, `python` (on Windows also
`py -3`) and `node` on your PATH; `v2 doctor` shows them. To pick one, put a `v2.toml` in your
project folder ([docs, chapter 14](DOCS.md#14-project-file-v2toml)), for example
`[python]` `venv = ".venv"` for a virtual environment.

**Does V2 leave files around?** No. Temporary files go to a private folder in your temp directory
and are removed when V2 exits; caches live in `~/.v2/cache`. Nothing is written to your project.

**Is a `.v2` file safe to run?** Like any script, it runs with your rights, every block in every
language: only run scripts you trust ([docs, chapter 16](DOCS.md#16-security-and-trust)).

**Is it fast?** V2's own code is a small interpreter, fine for gluing things together. Heavy
loops belong in a Python or JavaScript block (and, later, in C or Rust blocks).

**Is there editor support?** Not yet: `.v2` files are plain text in any editor.

## Known limitations

- **Not built yet:** C, C++ and Rust blocks (`@c`, `@cpp`, `@rust`), `extern` for native
  libraries, custom engines, the shell and other run engines (`@sh`, `@pwsh`, `@lua`, ...), and
  `v2 cache clean`. The docs describe them and mark each one "Not built yet"; using one says so.
- **Ready-made downloads** exist for the three computers listed above. Others, Intel Macs
  included, build from source.
- **The downloads aren't code-signed**, so a browser or antivirus may warn about them. The install
  commands don't trigger those warnings.
- **Values cross between languages as copies** (numbers, strings, lists, dicts) or as handles to
  the foreign object; one value can be at most 64 MiB.
- **No REPL, no package manager of its own, no editor plugins.**
- **It's an alpha:** syntax and behavior may still change before 0.1.0.

## Uninstall

Delete the `.v2` folder in your home folder (`%USERPROFILE%\.v2` on Windows), then remove it from
PATH: on Windows, under Settings, "Edit environment variables for your account", delete the
`.v2\bin` entry of `Path`; on macOS and Linux, delete the two `# V2` lines that `install.sh` added
to your shell's startup file.

## Reporting bugs

[Open an issue](https://github.com/SirOxygenFish/v2-language/issues/new/choose) and pick "Bug
report". Include the smallest script that shows the problem, what you expected, what happened,
and the output of `v2 doctor` (it includes V2's version and your system).
[CONTRIBUTING.md](CONTRIBUTING.md) explains how to build V2 and run its tests.

## Read more

- [DOCS.md](DOCS.md): the language, from the first script to the cookbook
- [INTERNALS.md](INTERNALS.md): how it works, for contributors
- [ROADMAP.md](ROADMAP.md): what's done and what's next

V2 is under the [MIT license](LICENSE).
