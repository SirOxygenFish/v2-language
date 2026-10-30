# V2 — one script, every language

> **Status:** being built; [ROADMAP.md](ROADMAP.md) "Where we are" says which parts work. How it
> works under the hood is specified in [INTERNALS.md](INTERNALS.md).

---

## Contents

1. [What V2 is](#1-what-v2-is)
2. [Getting started](#2-getting-started)
3. [The host language](#3-the-host-language)
4. [Embedded blocks](#4-embedded-blocks)
5. [Engines](#5-engines)
6. [Sharing functions: `@export` and `@import`](#6-sharing-functions-export-and-import)
7. [Importing foreign packages](#7-importing-foreign-packages)
8. [Cross-language calls and callbacks](#8-cross-language-calls-and-callbacks)
9. [Values across the boundary](#9-values-across-the-boundary)
10. [Errors across the boundary](#10-errors-across-the-boundary)
11. [Compiled blocks: C, C++, Rust](#11-compiled-blocks-c-c-rust)
12. [Native libraries: `extern`](#12-native-libraries-extern)
13. [Custom engines](#13-custom-engines)
14. [Project file: `v2.toml`](#14-project-file-v2toml)
15. [Command line](#15-command-line)
16. [Security and trust](#16-security-and-trust)
17. [Cookbook](#17-cookbook)
18. [Appendix](#18-appendix)

---

## 1. What V2 is

V2 is a small **glue language**. You write the *orchestration* in V2, and the heavy lifting in
whatever language already has the best library for the job:

- **Python** for data, science, machine learning and scripting libraries,
- **JavaScript / TypeScript** for the npm ecosystem,
- **C, C++ and Rust** for speed and native libraries.

V2 lets those languages live in **one file**, call each other, and share functions with
`@export` / `@import`. The host language itself is deliberately tiny: if Python or npm already
does something well, V2 borrows it instead of reinventing it.

### A taste

```v2
// Pull a function straight out of Python's standard library.
@import { mean } from py.statistics

// A Python block that defines functions for the rest of the file.
@py stats {
    def spread(xs):
        return max(xs) - min(xs)
    @export { spread }
}

// A Rust block, compiled once and cached, for the hot path.
@rust fast {
    fn sum_squares(n: i64) -> i64 {
        (1..=n).map(|i| i * i).sum()
    }
    @export { sum_squares }
}

@import { spread } from stats
@import { sum_squares } from fast

let data = [3, 8, 1, 9, 4]
print(f"mean ${mean(data)}, spread ${spread(data)}")
print(f"sum of squares to 1000: ${sum_squares(1000)}")

// Blocks are expressions too: the last expression is the value.
let top = @py(data) { sorted(data, reverse=True)[:2] }
print(top)   // [9, 8]
```

### Not in V2

V2 has no classes, structs, enums, traits, `match`, destructuring, generators, async/await,
macros, decorators, type annotations, tuples, sets, comprehensions, pipes, bitwise operators or
`defer`: write that part in a language that has it — that is exactly what V2 is for.

---

## 2. Getting started

### Install

```bash
cargo install --path .       # from the repo root
```

This puts `v2` in `~/.cargo/bin`, which rustup already added to your `PATH`. The binary is called
`v2`. Building it from source needs Rust and a C compiler (V2 bundles libffi, which is written in
C); the built binary has no other dependencies. Each language you embed
needs its own toolchain (`python`, `node`, `rustc`, and a C/C++ compiler: MSVC `cl` on Windows,
`gcc` or `clang` elsewhere; see [chapter 5](#5-engines)). `v2 doctor` shows what V2 can find.

### Hello, world

```v2
print("Hello, world!")
```

```bash
v2 hello.v2
```

A V2 file runs top to bottom. There is no `main()` function. Arguments after the file name are
available through `args()`:

```bash
v2 greet.v2 Ada
```

```v2
let who = args().get(0, "stranger")
print(f"Hello, ${who}!")
```

### Check your toolchains

```text
$ v2 doctor
ENGINE  STATUS  COMMAND                                   VERSION   FROM
py      ok      C:\Program Files\Python311\python.exe     3.11.6    PATH
js      ok      C:\Program Files\nodejs\node.exe          22.14.0   PATH
ts      ok      (js engine, built-in type stripping)      22.14.0   PATH
c       ok      ...\MSVC\14.40\bin\Hostx64\x64\cl.exe     19.40     vswhere
cpp     ok      ...\MSVC\14.40\bin\Hostx64\x64\cl.exe     19.40     vswhere
rust    ok      C:\Users\me\.cargo\bin\rustc.exe          1.91.1    PATH
...
config file: none
```

### A project

A folder with a `v2.toml` is a V2 project (see [chapter 14](#14-project-file-v2toml)). Projects
are optional — a single `.v2` file works on its own.

```toml
[python]
venv = ".venv"      # use this virtual environment's python and packages
```

```bash
v2 main.v2      # V2 finds the v2.toml in the script's folder or a parent folder
```

---

## 3. The host language

The host language is small on purpose: variables, functions, control flow, lists, dicts,
strings, errors and modules. Everything else comes from the languages you embed.

### 3.1 Lexical basics

```v2
// line comment
/* block comment */

let a = 1
let b = 2; let c = 3        // `;` is optional; use it to put statements on one line

let total = a +
            b +             // a line ending in an operator continues
            c
let names = [
    "ada",                  // open brackets continue across lines
    "grace",
]
let loud = names
    .filter(func(s) => s.starts_with("a"))     // a line starting with `.` continues
    .map(func(s) => s.upper())
```

A statement ends at the end of a line **unless** a `(`, `[` or dict `{` is still open, the line
ends with an operator, a comma or `=>`, or the next line starts with `.` or `?.`. A line that is
only an expression must be a call or a block, so `x == 1` meant as `x = 1`, or a line starting
with `-` meant to continue the previous one, is caught as a `SyntaxError`.

Source files are UTF-8 (a BOM is fine). Identifiers are letters, digits and `_`, not starting
with a digit. Keywords are listed in the [appendix](#keywords).

### 3.2 Values

| Type     | Examples                         | Notes                                                    |
| -------- | -------------------------------- | -------------------------------------------------------- |
| `null`   | `null`                           | "nothing"                                                |
| `bool`   | `true`, `false`                  |                                                          |
| `int`    | `42`, `-7`, `1_000_000`, `0xff`  | 64-bit; overflow is a `ValueError`, never silent wrap-around. Decimal and hex (`0x`) literals only |
| `float`  | `3.14`, `1e-3`, `2.0`            | 64-bit IEEE                                              |
| `str`    | `"hi"`, `'hi'`, `f"${x}"`        | Unicode text, immutable                                  |
| `bytes`  | `b"\x00\xff"`                    | raw binary data, immutable                               |
| `list`   | `[1, "two", 3.0]`                | ordered, mutable                                         |
| `dict`   | `{name: "Ada", "age": 36}`       | string keys, keeps insertion order, mutable              |
| `func`   | `func(x) => x * 2`               | V2 functions, and foreign functions you imported         |
| `handle` | (from foreign code)              | a live object inside another language — see [9](#9-values-across-the-boundary) |
| `error`  | `error("ValueError", "bad")`     | structured error value — see [3.9](#39-errors)           |

`type(x)` returns the type name as a string: `type(3)` is `"int"`, `type([])` is `"list"`.

`str(x)` and `print` show strings inside containers quoted, and whole-number floats keep their
`.0`: `print({k: [1, "a", 2.0]})` shows `{"k": [1, "a", 2.0]}`, while `print("a")` shows `a`.

**Truthiness.** `null`, `false`, `0`, `0.0`, `""`, `b""`, `[]` and `{}` are *falsy*. Everything
else, including every handle, is *truthy*.

**Lists and dicts are shared, not copied.** Assigning or passing a list gives another name for the
*same* list, like in Python and JavaScript. Use `.copy()` for a copy (a *shallow* one: lists
inside the copy are still shared).

```v2
let a = [1, 2]
let b = a
b.push(3)
print(a)            // [1, 2, 3]
let c = a.copy()
c.push(4)
print(a)            // [1, 2, 3]
```

### 3.3 Variables and constants

```v2
let count = 0          // variable
let nothing            // same as `let nothing = null`
const LIMIT = 10       // cannot be reassigned

count = count + 1
count += 1             // also -=, *=, /=, %=
```

- Variables live in the block (`{ ... }`) where they are declared.
- Declaring the same name twice in the same block is an error; an inner block may *shadow* it.
  A function's parameters, a loop's variables and a `catch` name count as declared in the body they
  belong to, so `func f(x) { let x = 1 }` is an error.
- `x += y` always means `x = x + y`. For lists that makes a **new** list: other names for the old
  list don't see the change (use `.push()` to change a list in place).
- `const` stops reassignment of the name. A `const` list can still be changed with `.push()`.
- Assigning to a name that was never declared is an error — there are no accidental globals.

### 3.4 Operators

From lowest to highest precedence:

| Level | Operators                          | Meaning                                             |
| ----- | ---------------------------------- | --------------------------------------------------- |
| 1     | `c ? a : b`                        | conditional                                         |
| 2     | `a ?? b`                           | `a` unless it is `null`, then `b`                   |
| 3     | `\|\|`                             | or (returns the first truthy operand)               |
| 4     | `&&`                               | and (returns the first falsy operand)               |
| 5     | `==`  `!=`                         | equality                                            |
| 6     | `<` `>` `<=` `>=` `in` `not in`    | comparison, membership                              |
| 7     | `+` `-`                            | add, subtract, concatenate                          |
| 8     | `*` `/` `%`                        | multiply, divide, remainder                         |
| 9     | `-x` `!x`                          | negate, not                                         |
| 10    | `**`                               | power (right-associative: `2 ** 3 ** 2` is `2 ** 9`) |
| 11    | `f(x)` `x[i]` `x[a:b]` `x.y` `x?.y` | call, index, slice, member, null-safe member        |

Rules worth knowing:

- `/` always produces a `float`: `7 / 2` is `3.5`. `%` follows the sign of the divisor:
  `-7 % 3` is `2`. Dividing by zero (with `/` or `%`) is a `ValueError`, and so is an `int` result
  that doesn't fit in 64 bits.
- `+` joins two strings, two lists or two bytes values. `"a" + 1` is a `TypeError` — use
  `f"a${1}"` or `"a" + str(1)`.
- `"ab" * 3` is `"ababab"`; `[0] * 3` is `[0, 0, 0]`.
- `==` compares values deeply (`[1, [2]] == [1, [2]]` is `true`). `1 == 1.0` is `true`.
  Values of different types are simply not equal. Two handles are equal when they point at the
  same foreign object.
- `<`, `>` and friends work on numbers and on strings. Comparing anything else is a `TypeError`.
- Comparisons don't chain: `a < b < c` and `a == b == c` are a `SyntaxError`; write
  `a < b && b < c`.
- `x in list` checks elements, `k in dict` checks keys (`false` when `k` isn't a string),
  `s in str` checks substrings (`s` must be a string), and `x in bytes` checks for a byte value
  (an int) or a sub-sequence (bytes).
- `user?.name` is `null` when `user` is `null`, instead of an error. The rest of the chain is
  skipped too: `user?.address.city` is `null` when `user` is `null`. (A missing dict key is still a
  `KeyError` — `?.` only guards against `null`.)

### 3.5 Strings

```v2
let s1 = "double quotes"
let s2 = 'single quotes'
let path = r"C:\Users\ada\data.csv"      // raw: backslashes are kept as-is
let name = "Ada"
let hello = f"Hello, ${name}! 2 + 2 = ${2 + 2}"
let price = f"${19.999:.2f}"              // "20.00" — `:.Nf` rounds a number to N decimals

let poem = """
    Roses are red,
    V2 glues them all.
    """
```

In a triple-quoted string the text starts on the line after the opening `"""`, the closing `"""`
sits on its own line, and its indentation is removed from every line. The result above is
`"Roses are red,\nV2 glues them all."` (no trailing newline). `f"""…"""` and `r"""…"""` work too.

Escapes: `\n` `\t` `\r` `\\` `\"` `\'` `\0` `\$` `\xNN` `\u{1F600}`. Any other escape is a syntax
error (so a typo like `"\d"` is caught instead of silently kept).

Strings are indexed by character: `"héllo"[1]` is `"é"`, `"hello"[-1]` is `"o"`,
`"hello"[1:3]` is `"el"`, and `len("héllo")` is `5`.

String methods:

| Method                       | Result                                           |
| ---------------------------- | ------------------------------------------------ |
| `s.upper()`, `s.lower()`     | changed case                                     |
| `s.trim()`                   | without leading/trailing whitespace              |
| `s.split(sep = null)`        | list of parts; without `sep`, split on runs of whitespace with no empty parts: `" a  b ".split()` is `["a", "b"]` |
| `s.lines()`                  | list of lines                                    |
| `s.replace(old, new)`        | every `old` replaced                             |
| `s.starts_with(p)`, `s.ends_with(p)` | `bool`                                   |
| `s.index_of(sub)`            | character index of the first match, or `-1`      |
| `s.encode()`                 | UTF-8 `bytes`                                    |

`bytes` has `len(b)`, indexing (gives an `int` 0–255), slicing, `b.decode()` (UTF-8 to `str`) and
`b.hex()`.

### 3.6 Lists and dicts

```v2
let xs = [5, 3, 8]
xs.push(1)                  // [5, 3, 8, 1]
let last = xs.pop()         // 1
xs.sort()                   // sorts in place: [3, 5, 8]
print(xs[0], xs[-1], xs[0:2])   // 3 8 [3, 5]

let user = {name: "Ada", "favorite number": 7}
print(user.name)            // Ada — `d.key` works for simple keys
print(user["favorite number"])
user.email = "ada@example.com"
print("email" in user)      // true
print(user.get("phone", "none"))   // none
```

Dict keys are always strings. In a literal, a bare word (`name:`) is a string key; write
`[expr]: value` to compute a key. Reading a missing key with `d[k]` or `d.k` is a `KeyError`; use
`d.get(k, default)` when a key may be missing. `len(x)` is the number of items in a list or dict.

| List method              | Effect                                        |
| ------------------------ | --------------------------------------------- |
| `push(...items)`         | add items to the end (`xs.push(...ys)` appends a whole list) |
| `pop(i = -1)`            | remove and return the item at index `i` (default: the last) |
| `insert(i, x)`           | insert at index                               |
| `sort(key = null, reverse = false)` | sort in place, stable; the items (or `key(item)` results) must be all numbers or all strings; `reverse: true` sorts descending |
| `reverse()`              | in place                                      |
| `map(f)` / `filter(f)`   | new list                                      |
| `join(sep)`              | string made of the items                      |
| `index_of(x)`            | index or `-1`                                 |
| `get(i, default = null)` | item at index, or `default` if out of range   |
| `copy()`                 | shallow copy                                  |

| Dict method              | Effect                                        |
| ------------------------ | --------------------------------------------- |
| `keys()` / `values()`    | lists                                         |
| `get(k, default = null)` | value or default                              |
| `remove(k)`              | remove and return the value                   |
| `copy()`                 | shallow copy                                  |

If a dict holds a function (or a foreign function you got as a value) under a key, `d.name(args)`
calls it — even when a dict method has the same name. Otherwise `d.name(args)` calls the dict
method.

### 3.7 Control flow

```v2
if score >= 90 {
    print("A")
} elif score >= 75 {
    print("B")
} else {
    print("keep going")
}

while len(queue) > 0 {
    handle(queue.pop())
}

for name in ["ada", "grace"] { print(name) }
for i in 0..3 { print(i) }                 // 0 1 2  (end is excluded)
for i, name in ["ada", "grace"] { print(i, name) }   // index and item
for key, value in {a: 1, b: 2} { print(key, value) }
for ch in "héllo" { print(ch) }            // characters
```

Parentheses around `if` and `while` conditions are optional: `if (x > 0) { }` is fine too; a
`for` head takes none. Braces are always required, and the opening `{` goes on the same line as
`if`, `while`, `for`, `func`, `try` and so on. `elif`, `else`, `catch` and `finally` may start a
new line. `break` and `continue` work in `while` and `for`. `a..b` is only allowed in a `for`
head; elsewhere use `range(a, b)`, which returns a list. A `for` loop walks over a snapshot of the
list or dict taken when the loop starts, so changing it inside the loop doesn't affect which items
the loop visits. Iterating a handle iterates the foreign object (see
[chapter 9](#9-values-across-the-boundary)).

### 3.8 Functions

```v2
func greet(name, greeting = "Hello") {
    return f"${greeting}, ${name}!"
}

print(greet("Ada"))                          // Hello, Ada!
print(greet("Ada", greeting: "Welcome"))     // named argument

func total(...nums) {                        // collects extra arguments into a list
    let s = 0
    for n in nums { s += n }
    return s
}
print(total(1, 2, 3))                        // 6
print(total(...[4, 5]))                      // spread a list into arguments: 9

let double = func(x) => x * 2                // an anonymous function
let shout = func(s) {
    return s.upper() + "!"
}
```

- `func` followed by a name declares a function; `func` followed by `(` makes an anonymous one.
  Messages and stack traces name an anonymous function after where it was written:
  `func@main.v2:12`.
- A named `func` can be called before the line where it is declared (declarations are hoisted
  within their block).
- A function without `return` returns `null`.
- Calls are checked: missing arguments, extra arguments and unknown argument names are all an
  `ArityError`.
- Functions are values: pass them around, store them in lists and dicts, return them.
- Closures capture variables, not copies. Each loop iteration gets its own variable, so
  `func() => i` made inside `for i in 0..3` remembers that iteration's `i`.
- Deep recursion stops with a `RecursionError` instead of crashing.

### 3.9 Errors

```v2
func parse_port(text) {
    let port = int(text)                     // throws ValueError if text is not a number
    if port < 1 || port > 65535 {
        throw error("ValueError", f"port out of range: ${port}")
    }
    return port
}

try {
    parse_port("99999")
} catch (e) {
    print(e.type, "-", e.message)            // ValueError - port out of range: 99999
    print(f"at ${e.file}:${e.line}")
} finally {
    print("done")
}
```

- `error(type, message, data = null)` makes an error value; `throw` raises it. `throw "text"` is
  shorthand for `throw error("Error", "text")`.
- `catch (e)` binds the error; `catch { }` without a name is allowed. Rethrow with `throw e`.
- Every error carries its location and a stack, so you always know **where** it happened.

| Field          | Meaning                                                           |
| -------------- | ----------------------------------------------------------------- |
| `e.type`       | kind, e.g. `"ValueError"`, `"KeyError"`, `"ForeignError"`         |
| `e.message`    | human-readable text                                               |
| `e.file`, `e.line`, `e.col` | where it was raised                                  |
| `e.trace`      | list of stack frames as strings, innermost first                  |
| `e.data`       | optional extra value                                              |
| `e.lang`       | `null` for V2 errors, or the engine tag (`"py"`, `"js"`, …)       |
| `e.foreign_type` | the foreign exception class, e.g. `"ZeroDivisionError"`         |
| `e.cause`      | the error this one was raised from (a foreign exception's cause, e.g. Python `raise … from …`, JS `cause`), or `null`; `error()` can't set it |

Built-in error types: `Error`, `TypeError`, `ValueError`, `KeyError`, `IndexError`, `NameError`,
`ArityError`, `IOError`, `RecursionError`, `ImportError`, `AssertionError`, plus the engine
errors in [chapter 10](#10-errors-across-the-boundary).

A mistake in the code itself (a missing `}`, a misspelled keyword, declaring a name twice) is a
`SyntaxError`. It is reported **before anything runs**, so it can't be caught by `try` in the same
file; in a file you `import`, it becomes an `ImportError` in the importing file.

An uncaught error prints like this and exits with code 1:

```text
main.v2:4:9: ValueError: port out of range: 99999
    throw error("ValueError", f"port out of range: ${port}")
    ^
  at parse_port (main.v2:4)
  at main.v2:10
```

The location is where the error was first thrown; rethrowing with `throw e` keeps it.

### 3.10 Modules

```v2
// util.v2
func slug(text) {
    return text.lower().replace(" ", "-")
}
let _cache = {}          // names starting with `_` stay private
```

```v2
// main.v2
import "./util.v2" as util
print(util.slug("Hello World"))    // hello-world
```

- Paths are relative to the importing file.
- A module runs once, the first time it is imported; later imports reuse it.
- `util` is a dict of the module's exported names, taken when the module finishes running.
  Reassigning a module variable later does not change what importers see — export a function
  if you need the current value.
- Every top-level name not starting with `_` is exported — including functions a module imported
  from other languages, so a module can wrap foreign code for the rest of your program.
- Circular imports are an `ImportError` that shows the cycle.

### 3.11 Builtins

The host has only these built-in functions. Dates, HTTP, regex, math beyond these and anything
bigger come from the languages you embed.

| Function                                | Does                                                        |
| --------------------------------------- | ----------------------------------------------------------- |
| `print(...values, sep: " ", end: "\n", stderr: false)` | print values separated by `sep`, followed by `end`; `stderr: true` writes to standard error |
| `input(prompt = "")`                    | read one line from standard input (without the newline); `null` at end of input |
| `len(x)`                                | length of a str, bytes, list, dict or handle                |
| `type(x)`                               | type name as a string                                       |
| `str(x)`, `int(x)`, `float(x)`, `bool(x)` | conversions (`int("42")`, `str(3.5)`, …)                  |
| `range(end)`, `range(start, end, step = 1)` | list of ints; positional arguments only                 |
| `min(...values)`, `max(...values)`      | smallest / largest of the arguments, or of a list's items when the only argument is a list (`max(xs)`); all numbers or all strings; on a tie the first wins; no values is a `ValueError` |
| `sum(xs)`                               | sum of a list of numbers: a float if any item is one; `sum([])` is `0` |
| `abs(x)`                                | absolute value of an int or float                           |
| `round(x, digits = 0)`                  | `x` rounded to `digits` decimals, ties to even, exactly as `f"${x:.Nf}"` prints it: `round(2.5)` is `2`, `round(2.675, 2)` is `2.67`. `digits` 0 gives an int (so NaN and infinity are a `ValueError`), more gives a float, fewer is a `ValueError`; an int `x` comes back unchanged |
| `div(a, b)`                             | integer division of two ints, rounded down: `div(-7, 2)` is `-4`, and `div(a, b) * b + a % b == a` (V2 has no `//`: it starts a comment) |
| `json_parse(text)`, `json_stringify(value, indent = null)` | JSON in and out                      |
| `read_file(path, binary: false)`        | file contents as `str` (or `bytes` with `binary: true`)     |
| `write_file(path, data, append: false)` | write a `str` or `bytes`, replacing the file; `append: true` adds to its end (creating it if missing) |
| `exists(path)`                          | `true` if a file or folder is there (`false` on any error)  |
| `list_dir(path)`                        | sorted list of the names in a folder                        |
| `make_dir(path)`                        | create a folder and any missing parents; an existing folder is fine |
| `remove_file(path)`                     | delete a file (not a folder)                                |
| `script_dir()`                          | absolute path of the folder of the `.v2` file containing the call |
| `exec(argv, input = null)`              | run a program without a shell (below)                       |
| `getenv(name, default = null)`          | environment variable                                        |
| `args()`                                | command-line arguments after the script name                |
| `sleep(seconds)`                        | wait (an int or float ≥ 0); Ctrl+C still stops the program  |
| `time()`                                | seconds since 1970-01-01 UTC, as a float                    |
| `exit(code = 0)`                        | stop the program now; engines are shut down cleanly, but `finally` blocks don't run |
| `error(type, message, data = null)`     | make an error value                                         |
| `assert(cond, message = "assertion failed")` | throw `error("AssertionError", message)` if `cond` is falsy |
| `to_v2(h)`                              | convert a foreign handle to plain V2 data ([9.2](#92-handles-everything-else)) |
| `define_engine(tag, kind, command, …)`  | declare a custom engine for the rest of the run ([13.3](#133-declaring-an-engine-in-the-script)) |
| `engine_available(tag)`                 | `true` if that engine's toolchain was found (`false` for unknown tags); never starts the engine |

Relative paths in the file functions and in `exec` are relative to the folder you run `v2` from
(the working directory), like in Python and Node. For a file next to your script, use
`script_dir()`: `read_file(f"${script_dir()}/data.csv")` (forward slashes work on Windows too).
`import` paths and `extern` library paths are relative to the `.v2` file instead. The file
functions raise `IOError` when they fail (`exists` just returns `false`). `getenv` and `args()`
turn invalid UTF-8 into U+FFFD instead of failing.

`exec` runs a program directly, without a shell, so its arguments reach it exactly as given:

```v2
let r = exec(["git", "log", "-1", "--format=%s"])
if r.code == 0 { print(r.stdout.trim()) } else { print(r.stderr, stderr: true) }
```

`argv` is a non-empty list of strings; a program name without a path separator is searched on
`PATH` like engine commands. A `str` or `bytes` `input` becomes the program's standard input
(with `null` it is empty). The result is a dict `{code, stdout, stderr}`, both outputs captured
whole and decoded as UTF-8 (invalid bytes become U+FFFD). A non-zero `code` is not an error; on
Linux and macOS a program killed by a signal has `code` 128 + the signal number. A program that
can't start raises `IOError: exec: cannot start 'gitt': <reason>`. It runs in V2's working
directory and environment with no timeout; what it started is stopped when `exec` returns or on
Ctrl+C.

---

## 4. Embedded blocks

An **embedded block** is a piece of another language written directly in a `.v2` file.

```v2
@py {
    import platform
    print("Python running on", platform.system())
}
```

### 4.1 Syntax

```text
@tag [label] [(args)] { foreign code }
```

- `tag` picks the language: `py`, `js`, `ts`, `c`, `cpp`, `rust`, a run engine such as `bash` or
  `go` (see [chapter 5](#5-engines)), or a custom engine ([chapter 13](#13-custom-engines)).
- `label` is an optional name, so other code can import from this block: `@py stats { ... }`.
- `(args)` passes V2 values into the block (see 4.4).
- The opening `{` must be on the **same line** as the end of the header (a long args list may wrap
  across lines; the `{` goes after its closing `)`). In a block that spans several lines, nothing
  but whitespace may follow that `{`: the code starts on the next line.

Your code is handed to the other language **as written**. V2 only removes the common indentation
(so you can indent blocks to match the surrounding V2 code; mixing tabs and spaces in that
indentation is an error) and blanks out its own `@export`/`@import` lines, keeping line numbers
intact. V2 never pastes V2 values into foreign code. The only code V2 ever adds is the small
wrappers it generates around compiled blocks (chapter 11).

### 4.2 Braces inside foreign code

For `py`, `js`, `ts`, `c`, `cpp` and `rust` blocks, V2 finds the closing `}` while understanding
that language's strings and comments, so code like `print("}")` or `char c = '{';` is fine. For
run engines and custom engines (chapter 13) V2 simply counts braces.

Two rules catch mismatched braces early:

- In a block that spans several lines, the closing `}` must be the first thing on its line.
- After the closing `}` of **any** block, even a one-line one, only `)`, `]`, `}`, `,`, `;` or a
  comment may follow on that line.

If a block contains braces V2 cannot match — for example inside a JavaScript regex literal, a C
`#define`, a heredoc or an unusual string form — use the **fence form**: three or more backticks
after the header, closed by a line holding only the same backticks.

````v2
@js report ```
    const opener = /\{+/;
    function render(x) { return String(x).replace(opener, "("); }
    @export { render }
```
````

An unterminated block is a syntax error that points at the line where the block started.

### 4.3 Action blocks and definition blocks

There are two kinds of block, decided by whether the block contains `@export`:

| Kind                 | Contains `@export`? | Runs                                  | Keeps its state? |
| -------------------- | ------------------- | ------------------------------------- | ---------------- |
| **Action block**     | no                  | every time the program reaches it     | no — fresh each run |
| **Definition block** | yes                 | once ([when it runs](#when-a-language-starts)) | yes — functions and globals live on |

```v2
for i in 0..3 {
    @py(i) { print("action run", i) }     // runs three times
}

@py counter {
    count = 0
    def bump():
        global count
        count += 1
        return count
    @export { bump }
}
@import { bump } from counter
bump()
print(bump())    // 2 — the definition block's state persists
```

Blocks run **in program order**, like any other statement, with one exception: a language nothing
needs yet doesn't start just because V2 passed one of its definition blocks (see
[When a language starts](#when-a-language-starts)). In V2 code, an `@import` must come after the
block it imports from; otherwise V2 reports which block is missing. (`@import` *inside* a block
looks the function up when it is called, so blocks can import from each other in any order — see
chapter 8.)

If a definition block fails while loading (say, a Python exception in its top-level code), it
stays failed: reaching it again raises the same error instead of running its code a second time.
Block args of a definition block are evaluated once, when it loads.

Each block has its **own namespace**: a variable defined in one `@py` block is not visible in
another. Blocks of the same language do share one engine process, so a library imported in one
block loads fast in the next. To use one block's functions in another block, `@export` and
`@import` them (see [chapter 8](#8-cross-language-calls-and-callbacks)).

### 4.4 Passing values in: block args

```v2
let nums = [4, 8, 15, 16, 23, 42]
@py(nums, label: "lucky numbers") {
    print(label, "->", sum(nums))
}
```

- `@py(nums)` makes a foreign variable `nums` holding a copy of V2's `nums`.
- `@py(label: expr)` names the foreign variable explicitly and can pass any expression.
- Args work for `py`, `js` and `ts` blocks, run engines and custom engines. Compiled blocks take
  their inputs through exported functions instead (chapter 11); `@c(x) { }` is a `SyntaxError`.

Values are **passed as data, never pasted into source code**, so there is no quoting or
injection problem.

### 4.5 Getting values out: blocks as expressions

An action block can be used as an expression. Its value is:

- for `py`: the value of the **last expression** in the block,
- for `js`, `ts`: JavaScript's **completion value** — what `eval` of the block would return
  (`undefined` becomes `null`),
- for compiled languages and `run` engines (built-in or custom): the program's **standard
  output** as a `str` (invalid UTF-8 becomes U+FFFD), with one trailing newline removed.

When a compiled or `run` block is used as a plain statement instead, its output is printed
normally, in order.

```v2
let data = [3, 1, 2]
let avg = @py(data) { sum(data) / len(data) }            // 2.0
let upper = @js(name: "ada") { name.toUpperCase() }      // "ADA"
let when = @py {
    import datetime
    datetime.date.today()                                # a handle — see chapter 9
}
print(when.year)
```

In a `py` block, if the last statement is not an expression, the block's value is `null`.
Definition blocks are statements only; using one as an expression is a syntax error.

A block value used as an operand, or followed by `.method()`, must be in parentheses:
`(@py { 1 }) + 2`, `(@py { "ada" }).upper()`.

### 4.6 Output and input

Output from blocks (`print` in Python, `console.log` in JavaScript, `printf` in C, …) goes to the
same place as V2's `print`, **in program order** — even when output is redirected to a file.

One exception: output written *below* the language level — by a Python C extension, `os.system`,
or a child process a Python/JS block starts — goes to standard error instead, so it can't mix
into V2's internal communication with the engine.

Blocks cannot read standard input: V2's `input()` owns it. Read input in V2 and pass it in as a
block arg. On Linux and macOS, terminal prompts (git, ssh, sudo, `getpass`) fail at once. On
Windows, blocks share V2's console, so a program that reads the console directly (an ssh password
prompt, Python `getpass`) waits for keyboard input — don't prompt from blocks.

### 4.7 Labels

Labels are identifiers and must be unique within a file across all languages. Aliases of a tag
mean the same thing: `@python stats` and `@py stats` are the same block kind.

### Errors

| Situation                                         | Error                                   |
| ------------------------------------------------- | --------------------------------------- |
| `{` on the line after the tag                     | `SyntaxError` with a hint               |
| code after the `{` of a multi-line block          | `SyntaxError` with a hint               |
| block never closed, or closing `}` mid-line       | `SyntaxError` at the opening line, suggesting the fence form |
| args on a compiled block (`@c(x) { }`)            | `SyntaxError`                           |
| two blocks with the same label                    | `SyntaxError`                           |
| unknown tag, e.g. `@pyhton`                       | `EngineUnknown` (did you mean `py`?)    |
| engine not installed or too old                   | `EngineNotFound`, saying how to fix it  |
| foreign syntax error in a py/js/ts block          | `ForeignError` pointing at the `.v2` line |
| compile error in a c/cpp/rust block               | `BuildError` pointing at the `.v2` line |
| reaching a definition block that failed to load   | the same error again                    |

---

## 5. Engines

An **engine** is what runs one language. V2 ships six built-in engines that share functions with
`@export` / `@import`, plus the [built-in run engines](#built-in-run-engines) below.

| Tag (aliases)          | Language   | Needs                                  | Kind        |
| ---------------------- | ---------- | -------------------------------------- | ----------- |
| `py` (`python`)        | Python     | Python 3.10+                           | interpreted |
| `js` (`javascript`)    | JavaScript | Node.js 22.13+ (not 23.0 or 23.1)      | interpreted |
| `ts` (`typescript`)    | TypeScript | Node.js 22.13+ (not 23.0 or 23.1)      | interpreted |
| `c`                    | C          | MSVC `cl` on Windows; gcc or clang     | compiled    |
| `cpp` (`cxx`)          | C++        | MSVC `cl` on Windows; g++ or clang++   | compiled    |
| `rust` (`rs`)          | Rust       | rustc (stable)                         | compiled    |

Interpreted engines run in a **worker process** that starts the first time your program needs
that language and stays until your program ends, so everything in it survives. Compiled engines
build a native library the first time a block is needed, cache it, and call it directly inside the
V2 process.

### When a language starts

A language costs nothing until your program needs it: when an action block runs, or when a function
imported from it is called. A script with Python, JavaScript and Rust blocks that only calls the
Python ones never starts Node or a compiler.

While a language isn't running, its definition blocks and `@import`s at the top level of a file wait
in line. When the language starts, the line runs first, in order, and from then on everything runs
in program order as usual.

```v2
@py stats {                      // waits: Python has not started
    def spread(xs):
        return max(xs) - min(xs)
    @export { spread }
}
@import { spread } from stats    // waits too

print("hello")                   // still no Python
print(spread([1, 2]))            // Python starts, the line runs, then the call
```

Only what can't show the difference waits. These run when reached, as usual: a definition block
with args, one that imports from `v2`, and blocks and imports inside `if`, `try`, loops or functions.
An `@import` waits only if the file just calls the name (for a whole-module import, just uses
`m.name`). Read the name as a value (a constant, `print(x)`, passing it to a function) and the
`@import` runs at its own line, starting the language. In a V2 module, public names don't wait
either, because other files can use them.

What this means for you:

- **A definition block's top-level effects can come later than its position, or never**, if its
  language isn't needed: its output, files it writes, anything other code would see. Do that work in
  an action block, which always runs when reached.
- An error from something that waited (a missing package, a failing block) appears when the
  language starts or at the first call, not at its own line.

A worker that has been idle for 10 seconds gives its memory back: on Windows at once, on Linux and
macOS when the system needs it (Linux without swap keeps it in RAM). Nothing in it is lost.
Variables, loaded libraries and handles are all still there, and the next call brings it back,
usually in about a millisecond, longer if its memory has to return from disk. A worker with a
`call_timeout` keeps its memory, so coming back can never trip the timeout.

### Python — `@py`

- Found as `python3`, `python`, or the `py -3` launcher on Windows (the Microsoft Store
  placeholder `python.exe` is skipped). Override with `[engines.py] command`.
- If `[python] venv` is set, that virtual environment's Python and packages are used
  ([14](#14-project-file-v2toml)).
- Where Python modules are found: [chapter 7](#7-importing-foreign-packages).
- Named arguments become Python keyword arguments: `plot(xs, color: "red")`.
- If a Python function returns a coroutine, V2 waits for it (see [8.4](#84-rules) for the one
  exception).
- Output is UTF-8 on every platform.
- The value of a block is its last expression, like in a Jupyter cell
  ([4.5](#45-getting-values-out-blocks-as-expressions)).
- `sys.exit()` inside Python raises a `ForeignError` (with `foreign_type` `"SystemExit"`) instead
  of ending your program. In blocks, `__name__` is not `"__main__"`.
- `multiprocessing` can't pickle functions defined in `@py` blocks: put them in a `.py` file next
  to the `.v2` file and import them from there.

### JavaScript — `@js`

- Found as `node`. Override with `[engines.js] command`.
- Blocks run as scripts with `require` available; `import` *statements* are not allowed inside
  blocks — use `require(...)`, `await import(...)` in an exported async function, or
  `@import { x } from js.package` ([chapter 7](#7-importing-foreign-packages)).
- `require` finds packages the way Node does ([chapter 7](#7-importing-foreign-packages)).
- If a function returns a Promise, V2 waits for it.
- JavaScript has no keyword arguments: calling a JS function with named arguments is an
  `ArityError`.
- A JS function is called with `new` when it is a `class`, or an old-style constructor whose
  `prototype` has its own properties besides `constructor` (like Node's `EventEmitter`); other
  functions are called plainly.
- `process.exit()` inside a block throws an error instead of stopping the engine.
- Each block runs in its own JavaScript realm, so values that come from packages or from V2 may
  fail `instanceof Array` / `instanceof Error`; use `Array.isArray(x)` and check `e.name` /
  `e.message`.

### TypeScript — `@ts`

- Runs on the same Node worker as `@js`, with the types stripped first by Node's built-in type
  stripping. Settings go in `[engines.js]`; `ts` has none of its own.
- Types are **not checked** — run `tsc` yourself if you want checking.
- Stripping only removes type syntax. `enum`, `namespace` and constructor parameter properties
  raise a `ForeignError` (`ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX`): write TypeScript without them, or
  use `@js`.
- `@js` and `@ts` blocks can import from each other directly.

### C — `@c`, C++ — `@cpp`, Rust — `@rust`

- On Linux and macOS, found as `cc`/`gcc`/`clang` (C) and `c++`/`g++`/`clang++` (C++). On
  Windows, C and C++ use MSVC `cl` only (found even outside a Developer Prompt, through the Visual
  Studio installer). Rust is `rustc`; on Windows it needs the same Visual Studio Build Tools.
  Override with `[engines.c] command`, add flags with `[engines.c] flags`.
- Standards: C11, C++17, Rust edition 2021.
- See [chapter 11](#11-compiled-blocks-c-c-rust) for how compiled blocks work.

### Built-in run engines

These work exactly like a custom `run` engine ([13.1](#131-run-engines-execute-a-file): output
printed, or the block's value as a `str`; args as `V2_ARG_<name>` variables; no `@export` or
`@import`), but need no `v2.toml`.

| Tag (aliases)          | Runs                            | File   | Needs                       |
| ---------------------- | ------------------------------- | ------ | --------------------------- |
| `bash` (`sh`)          | `bash {file}`                   | `.sh`  | bash; on Windows Git Bash   |
| `pwsh` (`powershell`)  | `pwsh -NoProfile -File {file}`  | `.ps1` | PowerShell                  |
| `cmd`                  | `cmd /d /q /c {file}`           | `.bat` | Windows                     |
| `lua`                  | `lua {file}`                    | `.lua` | Lua (`lua` or `lua5.4`)     |
| `ruby`                 | `ruby {file}`                   | `.rb`  | Ruby                        |
| `go`                   | `go run {file}`                 | `.go`  | Go                          |
| `java`                 | `java {file}`                   | `.java`| Java 11+                    |
| `zig`                  | `zig run {file}`                | `.zig` | Zig                         |
| `cs` (`csharp`)        | `dotnet run {file}`             | `.cs`  | .NET 10+                    |

```v2
let branch = @sh { git branch --show-current }
print(f"on branch ${branch}")

@sh(target: "build") { mkdir -p "$V2_ARG_target" }
```

- On Windows, `bash` is Git Bash (found through `git` on `PATH`, else in `C:\Program Files\Git`),
  never WSL's `System32\bash.exe`; `pwsh` runs with `-ExecutionPolicy Bypass`, falling back to
  `powershell.exe`. `cmd` is Windows only (elsewhere: `EngineNotFound: cmd is Windows only`).
- A `java` block is one source file run directly; a `cs` block is a .NET file-based app.
- Configure them in `[engines.<tag>]` (`command`, `call_timeout`) or with `V2_ENGINE_<TAG>`
  ([14](#14-project-file-v2toml)); `v2 doctor` lists them.
- For `@export` / `@import` in one of these languages, declare a bridged engine under another tag
  ([13.2](#132-bridged-engines-full-importexport)).

---

## 6. Sharing functions: `@export` and `@import`

`@export` makes names from a block available. `@import` brings them into V2 — or into another
block.

```v2
@py geometry {
    import math
    def area(r):
        return math.pi * r * r
    def circumference(r):
        return 2 * math.pi * r
    @export { area, circumference }
}

@import { area, circumference as circ } from geometry
print(area(2), circ(2))
```

### 6.1 `@export`

Inside a block, on its own line:

```text
@export { name1, name2 }
@export { * }
```

- The list may span several lines.
- `@export { * }` exports every function (and, in Python, class) the block **defines**. In `py`,
  `js` and `ts` blocks it skips names starting with `_`; in Python it also leaves out names the
  block imported. In `js` and `ts` blocks it exports the top-level `function` declarations and
  every top-level `var` holding a function, including one set from `require(...)` (names from the
  block's `@import` lines are not included); list `const`, `let` and `class` names explicitly. In
  compiled blocks it exports every non-`static`, non-`inline` function except `main` whose
  signature V2 can convert; the others are left out, and an `@import` of one says why.
- In `py`, `js` and `ts` blocks you may also export a constant, e.g. `@export { VERSION }`; V2
  copies its value when the block loads. Compiled blocks export functions only.
- Exporting a class (Python) or constructor (JS) gives you a function that returns handles.
- Every exported name is checked when the block loads; a missing name is an `ImportError`.
- A name that isn't a valid V2 name (JavaScript's `$x`) can be written in quotes: `@export { "$x" }`.
  Compiled blocks export functions by plain name; a quoted item there is a `SyntaxError`.
- If your foreign code has a line that really starts with `@export` or `@import` (for example a
  Python decorator with that name), write `@@export` / `@@import` and V2 passes it through with a
  single `@`.

### 6.2 `@import`

```text
@import { name [as alias], ... } from SELECTOR
@import MODULE_SELECTOR as alias
```

The **selector** says where to look:

| Selector           | Means                                                           |
| ------------------ | --------------------------------------------------------------- |
| `@py`              | all `@py` blocks in this file                                   |
| `stats`            | the block labeled `stats` (any language)                        |
| `@py.stats`        | exactly the `@py` block labeled `stats`                         |
| `py.statistics`    | the Python module `statistics` ([chapter 7](#7-importing-foreign-packages)) |
| `js.lodash`        | the npm package `lodash` (`ts.lodash` means the same)           |
| `v2`               | this file's V2 functions — only inside blocks ([chapter 8](#8-cross-language-calls-and-callbacks)) |

Aliases of a tag work in selectors: `@python.stats` is `@py.stats`, `python.os` is `py.os`. `@js`
and `@ts` blocks run on one engine, so `@js`, `@ts`, `@js.x` and `@ts.x` match blocks of either
tag. The whole-module form `@import MODULE_SELECTOR as alias` only works with package selectors
like `py.numpy`, and only in V2 code. Inside a block it is a `SyntaxError`: use the language's own
import there (`import numpy as np` in a `@py` block).

An imported name is an ordinary V2 function in the scope where the `@import` appears. Call it,
pass it around, store it, or re-export it from a V2 module. List every name you import: there is
no `@import { * }`.

Inside a block, `@import` brings in functions only: naming a constant that a block or package
exports is an `ImportError` (`'VERSION' is a constant — pass it in as a block arg`).

### 6.3 Ambiguity rules

If a name is exported by two blocks and you import it from `@py` (all Python blocks), V2 cannot
guess which one you mean:

```text
ImportError: Import of 'mean' from '@py' is ambiguous (exported by blocks 'stats' and 'calc')
             — use @py.stats or @py.calc
```

- It doesn't matter which blocks have run: a block counts as soon as its `@export` list names the
  name, and a block with `@export { * }` counts once it has run.
- Naming a specific block (`stats` or `@py.stats`) always resolves the ambiguity.
- A block exported with `@export { * }` can still be imported selectively.

### Errors

| Situation                              | Message                                                     |
| -------------------------------------- | ----------------------------------------------------------- |
| name not exported                      | `'x' is not exported by 'stats' (did the block @export it?)` |
| selector matches nothing               | `No engine exports found for selector 'stats'`              |
| block not loaded yet (V2 code)         | `block 'stats' has not run yet — move this @import below it` |
| block not loaded yet when a block calls `x` | `block 'stats' has not run yet — 'x' can't be called before it loads` |
| block failed to load                   | `block 'stats' failed to load: <the original error>`        |
| ambiguous name                         | see 6.3                                                     |

Blocks without a label appear in messages as `@py block at main.v2:12`.

---

## 7. Importing foreign packages

You don't need a block to use a library. Import it directly:

```v2
@import { mean, median } from py.statistics
@import { chunk } from js.lodash

print(mean([1, 2, 3, 4]))           // 2.5
print(chunk([1, 2, 3, 4, 5], 2))    // [[1, 2], [3, 4], [5]]
```

- `py.name` imports a Python module; dotted names work: `@import { join } from py.os.path`.
- `js.name` imports an npm package (or Node built-in, e.g. `js.fs`, `js.path`). Package names with
  unusual characters are quoted: `@import { format } from js."date-fns"`,
  `@import { z } from js."@scope/pkg"`. A local JavaScript file is a quoted relative path, from
  the `.v2` file's folder: `@import { render } from js."./helpers.js"`.
- A JavaScript default export is imported by name: `@import { "default" as dayjs } from js.dayjs`.
- **Whole module as a handle:** `@import py.numpy as np` gives you the module itself. If a
  JavaScript package *is* a function (`module.exports = function …`), the handle can be called
  directly.

```v2
@import py.numpy as np
let a = np.array([1, 2, 3])
print(a.mean())                      // 2.0
print(to_v2(np.multiply(a, 2)))      // [2, 4, 6] — use functions, not operators, on handles
```

Names that are V2 keywords can be imported with quotes: `@import { "from" as rx_from } from js.rxjs`.

**Where packages come from.** V2 does not install packages for you. Install them with the
language's own tools and tell V2 where they are:

```bash
python -m venv .venv
.venv\Scripts\pip install pandas      # Windows
.venv/bin/pip install pandas          # Linux/macOS
npm install lodash                    # creates node_modules/ in the project root
```

```toml
[python]
venv = ".venv"
```

Python looks in the `.v2` file's folder, the project root, then its normal module path (your
`PYTHONPATH`, the standard library and installed packages — the venv's, when one is set). Node
looks the way Node always does: in the `node_modules` folders from the `.v2` file's folder upwards
(so your project's is found), then in your own `NODE_PATH` if you set one. Packages that only ship
as ES modules work too, except the rare one whose `package.json` `exports` offers only an
`import` entry. A local `helpers.py` next to your `.v2` file is importable as `py.helpers`.

One Python engine serves every `.v2` file in a run, so two local modules with the same name in
different folders collide — the first one imported wins. Likewise a JavaScript package imported by
bare name is loaded once per run, from the folder of the first `.v2` file that imports it.

A module that doesn't exist is an `ImportError` carrying the foreign message
(`No module named 'pandas'`), raised at the `@import`, or when the language starts or at the first
call if the `@import` [waited](#when-a-language-starts).

---

## 8. Cross-language calls and callbacks

`@import` also works **inside** blocks, so every language can call every other — and V2 itself.

```v2
@py calc {
    def square(x):
        return x * x
    @export { square }
}

@js report {
    @import { square } from calc
    @import { mean } from py.statistics
    function summary(xs) {
        return `squares: ${xs.map(x => square(x))}, mean: ${mean(xs)}`;
    }
    @export { summary }
}

@import { summary } from report
print(summary([1, 2, 3]))
```

Inside a block, imported names are ordinary functions of that language. Calls are synchronous:
the JavaScript above simply gets the Python result back. (Note `xs.map(x => square(x))` rather
than `xs.map(square)`: JavaScript's `map` passes three arguments, and the Python function only
takes one.) An imported function is looked up when it is called, so the block it comes from only
has to have run by then.

### 8.1 Calling V2 from a block

The selector `v2` imports this file's top-level V2 functions into a block:

```v2
func log_line(msg) {
    print(f"[log] ${msg}")
}

@py {
    @import { log_line } from v2
    log_line("hello from Python")
}
```

### 8.2 Passing functions as callbacks

V2 functions, named or anonymous, can be passed to foreign code, and foreign functions to other
languages — including ones you got back as values (callable handles, 9.2):

```v2
@py sorter {
    def sort_by(items, key):
        return sorted(items, key=key)
    @export { sort_by }
}
@import { sort_by } from sorter

let people = [{name: "Ada", age: 36}, {name: "Grace", age: 85}, {name: "Linus", age: 21}]
let by_age = sort_by(people, func(p) => p.age)
print(by_age.map(func(p) => p.name))      // ["Linus", "Ada", "Grace"]
```

Python calls the V2 function once per item; each call crosses back into V2 and returns.

### 8.3 Compiled code calling other languages

Compiled blocks can import too. Because C, C++ and Rust have no common dynamic data type,
imported functions there take and return **JSON text** (see [11.6](#116-calling-other-languages-from-compiled-code)):

```v2
@rust stats {
    @import { mean } from py.statistics
    fn average(a: f64, b: f64) -> f64 {
        let json = mean(&format!("[[{}, {}]]", a, b)).expect("python failed");
        json.parse().unwrap()
    }
    @export { average }
}
@import { average } from stats
print(average(3.0, 4.0))   // 3.5 — Rust called a Python library
```

### 8.4 Rules

- Calls may nest (V2 → Python → V2 → JavaScript …) up to 64 levels deep.
- Each hop between processes costs roughly a tenth of a millisecond: keep tight loops inside one
  language.
- A JavaScript function that returns a Promise is awaited, except while JavaScript is itself
  waiting on a cross-language call it made (JS → … → this JS function): its event loop can't run
  then, so returning a Promise is an error. Likewise a Python coroutine is awaited unless Python is
  already running an event loop on that thread (an async Python function called into V2, which
  called back into Python).
- Foreign code can call into V2 (an imported function or a V2 callback) only while V2 is waiting
  on that engine — during a call or block V2 started. A call from a timer, background thread or
  event handler that fires while the engine is idle fails with an error inside the foreign
  language. A callback can be stored and used again during a later call.
- JavaScript timers, sockets and other async work run only while V2 is waiting on a JS Promise
  (and for a moment right after each call returns): to let them finish, export an async function
  and call it.
- A foreign function may only call V2 from the thread V2 called it on.
- When an imported function (another language's, or V2's) throws, the foreign code that called it
  gets a `V2Error` exception with `type`, `message`, `data`, `lang` and `foreign_type`.

---

## 9. Values across the boundary

### 9.1 What gets copied

Plain data is **copied** when it crosses between languages:

| V2      | Python              | JavaScript                         | C / C++                   | Rust            |
| ------- | ------------------- | ---------------------------------- | ------------------------- | --------------- |
| `null`  | `None`              | `null` (`undefined` becomes `null`) | `NULL` for `const char*` and pointer parameters; a NULL result is `null` | — |
| `bool`  | `bool`              | `boolean`                          | `bool`                    | `bool`          |
| `int`   | `int`               | `number` (beyond ±2⁵³: `BigInt`)   | `int`, `long`, `int64_t`… | `i8`…`i64`, `u8`…`u64` |
| `float` | `float`             | `number`                           | `float`, `double`         | `f32`, `f64`    |
| `str`   | `str`               | `string`                           | `const char*` / `std::string` | `&str` / `String` |
| `bytes` | `bytes`             | `Buffer`                           | —                         | —               |
| `list`  | `list` (tuples come back as lists) | `Array`             | — (pass `json_stringify(x)` as a `str`, 11.7) | — (pass `json_stringify(x)` as a `str`, 11.7) |
| `dict`  | `dict` (string keys) | plain object                      | — (pass `json_stringify(x)` as a `str`, 11.7) | — (pass `json_stringify(x)` as a `str`, 11.7) |
| `func`  | callable            | function                           | —                         | —               |

JSON text is used automatically only when compiled code calls other languages through `@import`
([11.6](#116-calling-other-languages-from-compiled-code)).

Only plain `list`, `tuple` and `dict` values convert; subclasses such as `namedtuple`,
`OrderedDict` or `Counter` come back as handles (9.2). Functions go *into* foreign code as
callables. Foreign functions, methods and classes that come *back* as values are callable handles
(`type()` says `"handle"`); names you bring in with `@import` are ordinary `func` values.

Because data is copied, a Python function that changes a list you passed in does **not** change
your V2 list. Return the new value instead. One value crossing between processes can't exceed
64 MiB (`MarshalError`).

Numbers keep their kind: `1.0` stays a float in Python. JavaScript cannot tell `1.0` from `1`, so
a whole-number JS `number` arrives in V2 as an `int`. Integers are range-checked when they go into
compiled code (`MarshalError` if 300 is passed as a `uint8_t`).

### 9.2 Handles: everything else

A value that isn't plain data — a `datetime`, a class instance, a pandas `DataFrame`, a set, a
Python int too big for 64 bits, a JS `Map` — comes back as a **handle**: a reference to the live
object inside the other language.

```v2
@import py.pandas as pd

let df = pd.read_csv("sales.csv")
print(len(df))                       // number of rows
print(df.columns)                    // attribute access
let top = df.nlargest(3, "revenue")  // method call returns another handle
for row in top.itertuples() {        // iterate a foreign iterable
    print(row.region, row.revenue)
}
print(df["revenue"].sum())           // indexing, then a method
```

What you can do with a handle:

| Operation              | Example                    |
| ---------------------- | -------------------------- |
| read / write attribute | `h.name`, `h.name = "x"`   |
| call a method          | `h.method(1, key: 2)`      |
| call it                | `h(3)`                     |
| index                  | `h["col"]`, `h[0]`, `h[k] = v` |
| length                 | `len(h)`                   |
| iterate                | `for x in h { }`, `for i, x in h { }` |
| print / `str(h)`       | the foreign text form      |
| convert to data        | `to_v2(h)`                 |

Iterating a Python mapping or a JS `Map` works like a dict: `for k in h` gives keys,
`for k, v in h` keys and values. Other handles give items, or index and item with two variables.

`to_v2(h)` asks the engine to turn the object into plain data. Containers are converted
recursively; Python sets and JS `Set`s become lists, JS `Map`s become dicts, and mapping keys that
are numbers, booleans or `None`/`null` become strings in V2's `str()` form (`1` → `"1"`, `True` → `"true"`, so a
pandas Series with the default index gives `{"0": …, "1": …}`). Other objects use their
`to_dict()` method if they have one, otherwise `tolist()`. Anything else — a `datetime`, a class
instance without those methods, an int beyond 64 bits, other key types, two keys that become the
same string (`1` and `"1"`) — is a `MarshalError`.

**Handle rules:**

- Arithmetic and comparison operators (`+`, `*`, `<`, …), `in` and slicing don't apply to handles
  (`TypeError`); `==` only checks whether two handles are the same object. Do the work inside a
  block — `@py(df) { df["revenue"] * 1.2 }` — call a function (`np.multiply(a, 2)`), or convert
  with `to_v2(h)`.
- A handle belongs to its engine. Passing a Python handle to a JavaScript function is a
  `MarshalError` — convert it with `to_v2(h)` first. A callable handle (a function, method,
  class or callable object) is the exception: the other language gets a function that calls it.
- A handle stays valid as long as V2 keeps a reference to it. V2 tells the engine when it can let
  the object go.
- Handles are always truthy and cannot be dict keys.

---

## 10. Errors across the boundary

A foreign exception becomes a normal V2 error you can catch:

```v2
@py risky {
    def ratio(a, b):
        return a / b
    @export { ratio }
}
@import { ratio } from risky

try {
    ratio(1, 0)
} catch (e) {
    print(e.type)          // ForeignError
    print(e.foreign_type)  // ZeroDivisionError
    print(e.message)       // division by zero
    print(e.lang)          // py
}
```

Uncaught, it prints the foreign stack **with `.v2` line numbers**, mixed into the V2 stack:

```text
main.v2:3:16: ForeignError: ZeroDivisionError: division by zero
    return a / b
           ^
  at ratio (main.v2:3, in @py risky)
  at main.v2:9
```

A V2 error thrown inside a callback travels through the foreign code and comes back to V2 with
its original type.

### Engine errors

| Error              | When                                                                  |
| ------------------ | --------------------------------------------------------------------- |
| `EngineUnknown`    | a block or selector uses a tag that is neither built in nor declared (in `v2.toml` or with `define_engine`) |
| `EngineNotFound`   | the engine's toolchain isn't installed, is too old, or can't start — including a configured command or venv that doesn't exist (message includes how to fix it) |
| `EngineCrashed`    | the engine process died; every later use of that engine fails too     |
| `EngineTimeout`    | a call ran longer than `call_timeout`; a worker engine (py, js, bridged) is stopped for the rest of the run, so later uses of it raise `EngineCrashed` |
| `EngineExitError`  | a compiled program or `run` engine block exited with a non-zero status |
| `MarshalError`     | a value couldn't cross (e.g. a non-callable handle sent to another engine, or a value over 64 MiB), or doesn't fit a C type (range, NUL inside a string, `null` for a `std::string`/`&str`, …), or a native result doesn't fit V2 (an unsigned value above the `int` range, a returned string that isn't valid UTF-8) |
| `BuildError`       | a compiled block failed to compile (compiler output, `.v2` line numbers) |
| `ForeignError`     | the foreign code threw an exception                                   |
| `ProtocolError`    | an engine adapter broke the protocol (custom engines)                 |
| `FFILoadError`     | a native library couldn't be loaded, or a declared function isn't in it |

### Skip if missing

```v2
if engine_available("rust") {
    // use the Rust block
} else {
    print("Rust not installed; using the slow path")
}
```

---

## 11. Compiled blocks: C, C++, Rust

A compiled block is built into a native library the first time it is needed, cached, loaded into
the V2 process and called directly — no worker process, no per-call overhead beyond converting
arguments.

### 11.1 You write normal functions

```v2
@c fastmath {
    #include <math.h>
    double hypot3(double x, double y, double z) {
        return sqrt(x * x + y * y + z * z);
    }
    @export { hypot3 }
}

@cpp words {
    #include <string>
    #include <algorithm>
    std::string reversed(std::string s) {
        std::reverse(s.begin(), s.end());
        return s;
    }
    @export { reversed }
}

@rust primes {
    fn count_primes(limit: u32) -> u32 {
        (2..limit).filter(|n| (2..*n).take_while(|d| d * d <= *n).all(|d| n % d != 0)).count() as u32
    }
    @export { count_primes }
}

@import { hypot3 } from fastmath
@import { reversed } from words
@import { count_primes } from primes

print(hypot3(1, 2, 2))          // 3.0
print(reversed("stressed"))     // desserts
print(count_primes(100000))     // 9592
```

You don't write `extern "C"`, `#[no_mangle]`, `__declspec(dllexport)` or any other export
boilerplate: V2 generates a small C-compatible wrapper for each exported function. The wrapper
also catches Rust panics and C++ exceptions and turns them into a `ForeignError` instead of
crashing your program.

**Files next to the script.** The `.v2` file's folder is an include directory for C and C++, so
`#include "helpers.h"` finds a header there, and native libraries the block links against are
found there when it loads. In `flags`, a path starting with `./` or `../` (or `.\`, `..\`) — on
its own or right after `-I`, `-L`, `/I` or `/LIBPATH:` — is relative to the folder of the
`v2.toml`.

### 11.2 Types

V2 reads each exported function's signature from the source. Supported types:

| Kind      | C / C++                                                   | Rust                      | V2 value |
| --------- | --------------------------------------------------------- | ------------------------- | -------- |
| integers  | `char`, `short`, `int`, `long`, `long long`, `unsigned …`, `int8_t`…`uint64_t`, `size_t` | `i8`…`i64`, `u8`…`u64`, `isize`, `usize` | `int` |
| floats    | `float`, `double`                                         | `f32`, `f64`              | `float` (an `int` argument is converted) |
| bool      | `bool`                                                    | `bool`                    | `bool`   |
| text in   | `const char*`, `std::string`                              | `&str`, `String`          | `str`    |
| text out  | `const char*`, `char*`, `std::string`                     | `String`                  | `str`    |
| nothing   | `void`                                                    | `()` / no return type     | `null`   |

For anything else, pass JSON text as a string and parse it on the other side, or export a
function with a simpler signature. If V2 can't read a signature (heavy macros, templates,
overloads), export a small wrapper function with a plain signature that calls it.

**Who frees returned strings:** a returned `const char*` must point to memory that stays valid
(a literal or a static buffer); V2 copies it. A returned `char*` must come from `malloc`; V2
copies it and frees it. A Rust `String` or C++ `std::string` is handled for you.

**`null` and named arguments:** a `const char*` parameter accepts `null` (passed as `NULL`);
`std::string`, `&str` and `String` parameters don't (`MarshalError`). Native functions take
positional arguments only — named arguments are an `ArityError`.

### 11.3 State

The library stays loaded until the program ends, so `static` variables and Rust `static`s keep
their values between calls.

### 11.4 Compiled action blocks: run a program

A compiled block **without** `@export` is built as a small program and run, as a statement or as
an expression ([4.5](#45-getting-values-out-blocks-as-expressions)).

```v2
@rust {
    fn main() {
        println!("Hello from Rust!");
    }
}

let answer = @c {
    printf("%d", 6 * 7);
}
print(answer)    // 42
```

If there is no `main` function, V2 wraps the block in one: `#include` and `use` lines stay at the
top, and C/C++ wrappers also include `<stdio.h>`, `<stdlib.h>` and `<string.h>`. A wrapped C or
C++ block can only contain statements — to define helper functions, write `main` yourself (Rust
allows functions inside `fn main`, so a wrapped Rust block may define them).

The program's error output is shown live. A non-zero exit status raises `EngineExitError`; the
last part of the error output is in `e.data.stderr`.

### 11.5 The build cache

The first run of a compiled block compiles it (Rust takes a second or two); later runs load the
cached library instantly without touching the compiler, so a cached block keeps working even if
the compiler is gone. The cache key covers the source and the local headers it includes
(`#include "x.h"` found beside the including file or the `.v2` file, not through `-I` flags), the
flags and any configured `command`, so changing any of them rebuilds. Updating a compiler rebuilds
nothing: run `v2 cache clean` to rebuild with the new one. A cached library that is damaged is
rebuilt automatically. The cache lives in your user
folder (`~/.v2/cache/build`), never inside the project, and keeps every version it built until
`v2 cache clean` empties it (along with the toolchain probe cache).

Compiler errors are reported as a `BuildError` with line numbers pointing into your `.v2` file.

### 11.6 Calling other languages from compiled code

An `@import` inside a compiled block generates, for each name, a function that takes the
arguments as a **JSON array** and returns the result as **JSON text**:

| Language | Generated function for `@import { mean } from py.statistics`          |
| -------- | ---------------------------------------------------------------------- |
| C / C++  | `const char* mean(const char* args_json)` — returns `NULL` on error; `v2_last_error()` gives the message; the result stays valid until the next V2 call |
| Rust     | `fn mean(args_json: &str) -> Result<String, String>`                   |

See the example in [8.3](#83-compiled-code-calling-other-languages).

Inside compiled blocks `@import { names } from SELECTOR` works only in blocks that `@export`
something — a program-style block runs as a separate process and can't call
back into V2. Each imported name (its alias, if it has one) must be a valid identifier of the
block's language; rename it with `as` otherwise. Breaking these rules is a `SyntaxError`.

### 11.7 Limits

- A crash in C or C++ code (a bad pointer, a segfault) takes down the whole V2 process — native
  code is not sandboxed. So does native code calling `exit()` or `abort()`.
- Exported functions run inside the V2 process, so `call_timeout` doesn't apply to them
  ([14](#14-project-file-v2toml)), and they share standard input with V2's `input()` — don't read
  stdin from them.
- Lists, dicts and callbacks as direct parameters of compiled functions are not supported; use
  JSON text.
- Cargo crates in `@rust` blocks are not supported; only the standard library.

---

## 12. Native libraries: `extern`

`extern` calls functions in an **existing** native library (a `.dll`, `.so` or `.dylib`) with no
compiler involved. You describe the functions with ordinary C declarations — copy them straight
from the library's header.

```v2
extern "m" {
    double cos(double x);
    double pow(double base, double exponent);
}
print(cos(0))          // 1.0
print(pow(2, 10))      // 1024.0

extern "user32" as win {
    int MessageBoxA(void* hwnd, const char* text, const char* caption, unsigned int type);
}
win.MessageBoxA(null, "Hello from V2", "V2", 0)
```

- `extern "lib" { ... }` puts the functions in the current scope; `extern "lib" as name { ... }`
  puts them under `name.`.
- **Library names:**
  - `"c"` means the C runtime and `"m"` the math library, on every OS.
  - A bare name is searched per platform: `user32` → `user32.dll`; `sqlite3` → `sqlite3.dll` /
    `libsqlite3.so` / `libsqlite3.dylib`.
  - A path (`"./native/libfoo.so"`) is used as given (relative to the `.v2` file).
  - Search order: the `.v2` file's folder, then the system's normal search. On Windows that
    includes `PATH` but never the current folder, so a downloaded folder can't slip in a fake DLL.
- **Types:** the same scalar types as compiled blocks (11.2), plus pointers:
  - `const char*` parameters take a V2 `str` (converted to a NUL-terminated copy for the
    duration of the call) or `null`; a returned `const char*` is copied into a `str`.
  - Any other pointer (`void*`, `FILE*`, `sqlite3*`, and also non-const `char*`) is an **opaque
    handle**: you get it from one function and pass it to another. `null` is the NULL pointer.
- Positional arguments only. Like compiled code, `extern` functions run inside the V2 process, so
  `call_timeout` doesn't apply to them ([14](#14-project-file-v2toml)).

```v2
extern "sqlite3" as sq {
    const char* sqlite3_libversion(void);
}
print(sq.sqlite3_libversion())    // 3.45.1
```

Not supported: structs, callbacks (function pointers), variadic functions like `printf`, and
reading C headers directly with `#include`. Wrap those in a small `@c` block instead.

`extern` is trusted: a wrong declaration can crash the program. See
[chapter 16](#16-security-and-trust).

### Errors

`FFILoadError` (library not found — the message lists every place V2 looked — or a declared
function not in the library), `MarshalError` (argument or result doesn't fit), and a
`SyntaxError` for a declaration V2 can't read.

---

## 13. Custom engines

Any language can be added as a custom engine, declared in an `[engines.<tag>]` table in
`v2.toml` ([chapter 14](#14-project-file-v2toml)) or with `define_engine` in the script
([13.3](#133-declaring-an-engine-in-the-script)). Tags are lowercase letters, digits and `_`,
starting with a letter; `v2`, `export` and `import` are reserved. Built-in tags and their aliases
can't be redefined: a table named after one configures that built-in engine, except `ts`
(and `typescript`), which runs on the js engine and is configured in `[engines.js]`
([chapter 14](#14-project-file-v2toml)). There are two kinds.

### 13.1 `run` engines: execute a file

A `run` engine writes the block to a temporary file and runs a command; its output is printed, or
is the block's value when the block is used as an expression
([4.5](#45-getting-values-out-blocks-as-expressions)).

```toml
# v2.toml
[engines.r]
kind    = "run"
command = ["Rscript", "{file}"]
ext     = "R"

[engines.perl]
kind    = "run"
command = ["perl", "{file}"]
ext     = "pl"
```

```v2
@r { cat("hello from R\n") }

let version = @perl { print $^V }
print(f"perl ${version}")

@perl(name: "Ada") { print "hi $ENV{V2_ARG_name}\n" }
```

- `{file}` is replaced with the temporary file's path (`.bat`/`.cmd` files are written with
  Windows line endings). That file lives in V2's private temp folder, so a script that looks for
  files next to itself sees that folder: use paths relative to the working directory, or pass
  them in as block args.
- Block args arrive as environment variables named `V2_ARG_<name>` (lists and dicts as JSON).
  They are never pasted into the code.
- Error output is shown live. A non-zero exit status raises `EngineExitError`, with the last part
  of the error output in `e.data.stderr`.
- Processes the block starts in the background are stopped when the block finishes.
- `run` engines can't `@export` or `@import` — they run and return output.

### 13.2 `bridged` engines: full import/export

A `bridged` engine is a small **adapter** program that speaks V2's engine protocol (VEP) — the
same protocol the built-in Python and JavaScript engines use. A bridged engine supports
`@export`, `@import`, block values and errors, plus whichever extras its adapter declares (module
imports, handles, named arguments, calls back into V2); using an extra it doesn't declare is a
clear error.

```toml
[engines.rb]
kind    = "bridged"
command = ["ruby", "adapters/v2_adapter.rb"]   # relative to this v2.toml's folder
```

Relative paths in `command` are resolved against the folder of the `v2.toml` that declares them:
the program when it contains a path separator, and a later element when it starts with `./` or
`../` (or `.\`, `..\`), or when it names a file that exists there (`adapters/v2_adapter.rb`).
Options (`-c`) and elements containing `{` are left alone. Custom blocks are scanned by counting
braces (4.2), so use the fence form when the code has unbalanced braces. The protocol is specified
in [INTERNALS.md](INTERNALS.md), with a checklist of messages an adapter must handle. To test an
adapter, run a two-line `.v2` file that uses it.

### 13.3 Declaring an engine in the script

`define_engine` declares a custom engine from V2 code, so a single `.v2` file can bring its own
languages without a `v2.toml`:

```v2
define_engine("r", kind: "run", command: ["Rscript", "{file}"], ext: "R")
define_engine("rb", kind: "bridged", command: ["ruby", "adapters/v2_adapter.rb"])

@r { cat("hello from R\n") }
```

- It takes the keys of an `[engines.<tag>]` table, with the same rules. A mistake is a
  `ValueError` (an unknown key is an `ArityError`).
- The engine exists from the call on, so declare engines at the top of the file: a block that
  uses the tag must be reached after the call.
- Relative paths in `command` are relative to the `.v2` file that makes the call.
- Each tag can be declared once per run (in `v2.toml` or with one call), and built-in tags can't
  be declared: configure those in `v2.toml`. `V2_ENGINE_<TAG>` replaces the program as it does
  for a `v2.toml` engine.
- `v2 doctor` doesn't run your script, so it doesn't list these engines.

---

## 14. Project file: `v2.toml`

V2 looks for `v2.toml` in the script's folder and its parents (`v2 doctor` starts from the
current folder); the folder where it is found is the **project root**. Without one, the script's
folder is the project root. Every key is optional, except that a custom engine needs `kind` and
`command` (a `run` command, custom or built-in, must contain `{file}`; `ext` defaults to the tag).
`command` is always an array of strings; a plain string is a configuration error
(`[engines.lua] command must be an array: write ["lua", "{file}"]`). Relative paths are relative
to the folder of the `v2.toml`.

```toml
[engines.py]
# command = ["C:/Python312/python.exe"]   # skips auto-detection
startup_timeout = 30              # seconds to wait for the engine to start (default 30)
call_timeout    = 0               # seconds per call; 0 = no limit (default). Applies to worker
                                  # engines, run engines and compiled programs — not to in-process
                                  # compiled functions or extern calls, which can't be interrupted

[engines.rust]
flags = ["-C", "opt-level=3"]

[python]
venv = ".venv"                    # use this virtual environment
```

A call that runs past `call_timeout` raises `EngineTimeout`. For a worker engine that stops the
whole engine for the rest of the run, so later uses of it raise `EngineCrashed`.

If `[engines.py] command` (or `V2_ENGINE_PY`) is set, it is used and `[python] venv` is ignored.

`@ts` blocks run on the js engine, so they are configured in `[engines.js]`: an `[engines.ts]`
table, or `V2_ENGINE_TS`, is a configuration error. So is an unknown section or key, or a key that
doesn't apply to that engine (such as `flags` in `[engines.py]`). A configuration error stops V2
before anything runs, with exit code 2:

```text
C:\proj\v2.toml: config error: unknown key [engines.py] comand (did you mean 'command'?)
```

Settings are applied in this order, later ones winning: built-in defaults → the project's
`v2.toml` → `V2_ENGINE_<TAG>` environment variables, which set one engine's program
(`V2_ENGINE_PY=C:/Python312/python.exe`). `<TAG>` is the canonical tag in upper case
(`V2_ENGINE_PY`, not `V2_ENGINE_PYTHON`); a variable naming an alias, or a tag
that is neither built in nor declared (in `v2.toml` or with `define_engine`), is ignored.

V2 writes nothing into your project: temporary files go to a private folder in your system's
temp directory, and caches live in `~/.v2/`.

---

## 15. Command line

| Command                        | Does                                                                 |
| ------------------------------ | -------------------------------------------------------------------- |
| `v2 file.v2 [args...]`         | run a script                                                         |
| `v2 check file.v2`             | parse and check the script and every file it `import`s, without running or writing anything; prints the errors |
| `v2 doctor`                    | show every engine: found or not, command, version, and where the setting came from |
| `v2 cache clean`               | delete V2's caches (compiled blocks and toolchain probe results)     |
| `v2 --version`, `v2 --help`    |                                                                      |

There are no other flags. A first word that is exactly `check`, `doctor` or `cache` is a command;
to run a script with such a name, write it as a path: `v2 ./doctor`.

| Exit code | Meaning                                                                  |
| --------- | ------------------------------------------------------------------------ |
| `0`       | success                                                                  |
| `1`       | uncaught error, or a `SyntaxError`; `v2 check` found an error            |
| `2`       | usage or configuration error                                             |
| `3`       | a required engine is missing (uncaught `EngineNotFound`)                 |
| `130`     | interrupted (Ctrl+C or the process was told to stop); `finally` blocks don't run, engines are stopped |
| `101`     | internal error in V2 itself (a bug — please report it)                   |

---

## 16. Security and trust

- **Running a `.v2` file runs all the code in it** — every block, in every language, with your
  user rights. V2 has no sandbox. Only run scripts you trust.
- `v2 check file.v2` only parses and checks the code; it runs nothing. `v2 doctor`, by contrast,
  runs toolchain commands — including ones a project's `v2.toml` configures — so don't run it on a
  project you don't trust.
- When V2 searches `PATH` for toolchains or `exec` programs it skips relative entries like `.`,
  so a project folder can't slip in its own fake `python` or `gcc`.
- Processes started by `exec`, run-engine blocks and compiled programs (and everything they start)
  are stopped when the call or block finishes or times out; processes started from Python or
  JavaScript code are stopped when V2 exits.
- `extern` and compiled blocks run native code inside the V2 process. A mistake there can crash
  the program or corrupt memory; V2 checks the values it converts, but cannot check what native
  code does.
- Compiled-block caches live in your user folder, not in the project, so a downloaded project
  cannot ship a pre-built library that V2 would load.
- Engines inherit your environment variables and working directory.
- V2 never pastes V2 values into foreign source code; block args and call arguments are always
  passed as data.

---

## 17. Cookbook

### 17.1 The full tour

`math_ops.py`, next to the script:

```python
def bonus(x):
    return x * 3 + 7
```

`main.v2`:

```v2
@import { bonus } from py.math_ops           // a local Python file
@import { mean } from py.statistics          // Python's standard library

print(bonus(5))                              // 22
print(mean([10, 20, 30]))                    // 20

@py {
    from math_ops import bonus
    def py_bonus(x):
        return bonus(x)
    @export { py_bonus }
}

@py calculations {
    def py_square(x):
        return x * x
    @export { py_square }
}

@import { py_bonus } from @py                // all @py blocks
@import { py_square } from calculations      // by label
@import { py_square as square } from @py.calculations   // exact

@js {
    @import { py_bonus } from @py
    function js_bonus_twice(x) {
        return py_bonus(x) * 2;
    }
    @export { js_bonus_twice }
}

@js reports {
    @import { py_bonus } from @py
    @import { py_square } from @py.calculations
    function js_combo(x) {
        return py_bonus(x) + py_square(x);
    }
    @export { js_combo }
}

@import { js_bonus_twice } from @js
@import { js_combo } from reports

print(js_bonus_twice(5))     // 44
print(js_combo(5))           // 47
```

### 17.2 A spreadsheet through a pandas handle

```v2
@import py.pandas as pd

let df = pd.read_csv("sales.csv")
let by_region = df.groupby("region")["revenue"].sum()
for region, total in to_v2(by_region) {      // to_dict() -> V2 dict
    print(f"${region}: ${total:.2f}")
}
```

### 17.3 A hot loop in Rust

```v2
@rust sim {
    fn monte_carlo_pi(samples: u64) -> f64 {
        let mut inside = 0u64;
        let mut x: u64 = 12345;
        for _ in 0..samples {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let a = (x % 10000) as f64 / 10000.0;
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let b = (x % 10000) as f64 / 10000.0;
            if a * a + b * b <= 1.0 { inside += 1; }
        }
        4.0 * inside as f64 / samples as f64
    }
    @export { monte_carlo_pi }
}
@import { monte_carlo_pi } from sim
print(monte_carlo_pi(10_000_000))
```

### 17.4 An npm package

```v2
@import js.slugify as slugify                // after `npm install slugify`; the package is a function
print(slugify("Hello World from V2"))        // Hello-World-from-V2

@import { kebabCase } from js.lodash         // after `npm install lodash`
print(kebabCase("Hello World"))              // hello-world
```

### 17.5 Python calling back into V2

```v2
func progress(done, total) {
    print(f"${done}/${total}")
}

@py worker {
    @import { progress } from v2
    def process(items):
        for i, item in enumerate(items, 1):
            progress(i, len(items))
        return len(items)
    @export { process }
}
@import { process } from worker
process(["a", "b", "c"])
```

---

## 18. Appendix

### Grammar summary

```text
program     = { statement }                   (a statement ends at a line end or ";", 3.1)
statement   = let | const | assign | if | while | for | "break" | "continue"
            | return | throw | try | func | import | engine_import | extern
            | block | call
call        = an expr whose last step is a call: f(), a.b(), x?.y()
              (parentheses may surround a call or a block; any other expression is not a
              statement, 3.1)
let         = "let" NAME [ "=" expr ]
const       = "const" NAME "=" expr
assign      = target ( "=" | "+=" | "-=" | "*=" | "/=" | "%=" ) expr
target      = NAME | expr "." NAME | expr "[" expr "]"     (never through "?.")
if          = "if" expr body { "elif" expr body } [ "else" body ]
while       = "while" expr body
for         = "for" NAME [ "," NAME ] "in" expr [ ".." expr ] body
func        = "func" NAME "(" params ")" body
params      = [ param { "," param } [ "," ] ]      param = NAME [ "=" expr ] | "..." NAME
return      = "return" [ expr ]
throw       = "throw" expr
try         = "try" body [ "catch" [ "(" NAME ")" ] body ] [ "finally" body ]
import      = "import" STRING "as" NAME
engine_import = "@import" ( "{" items "}" "from" selector | selector "as" NAME )
              (the "as" form takes a package selector and is not allowed inside blocks)
items       = item { "," item } [ "," ]           item = ( NAME | STRING ) [ "as" NAME ]
selector    = "@" TAG [ "." NAME ] | NAME | package | "v2"
package     = LANG "." segment { "." segment }     segment = NAME | STRING
extern      = "extern" STRING [ "as" NAME ] "{" C-declarations "}"
block       = "@" TAG [ NAME ] [ "(" [ block_arg { "," block_arg } [ "," ] ] ")" ]
              ( "{" raw "}" | fence raw fence )
block_arg   = NAME [ ":" expr ]
body        = "{" { statement } "}"           (the "{" on the same line as its header)
expr        = block | ternary                 (a block used as a value must be an action block)
ternary     = the operator table in 3.4 over primaries: literals, NAME, "(" expr ")", list,
              dict, f-string and func_expr — so a block used as an operand, or followed by a
              postfix, needs parentheses; comparisons and equality don't chain
func_expr   = "func" "(" params ")" ( "=>" expr | body )
```

### Keywords

`let` `const` `func` `return` `if` `elif` `else` `while` `for` `in` `not` `break`
`continue` `try` `catch` `finally` `throw` `import` `as` `from` `extern` `true` `false` `null`

Keywords may still be used after a `.` (`h.from`, `result.in`) and, quoted, in import lists.
