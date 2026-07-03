# V2 Language Documentation

A complete reference for the V2 programming language — a modern, multi-paradigm language with bytecode compilation, memory safety, and embedded language interop.

**File extension:** `.v2`

---

## Table of Contents

- [Getting Started](#getting-started)
- [Docs Modes](#docs-modes)
  - [Interactive Docs](#interactive-docs)
- [Project Manifest](#project-manifest)
- [CLI Usage](#cli-usage)
- [WASM Target](#wasm-target)
- [Step Debugger](#step-debugger)
- [Comments & Doc Comments](#comments--doc-comments)
- [Variables & Constants](#variables--constants)
  - [Variable Scoping Rules](#variable-scoping-rules)
- [Data Types](#data-types)
- [Operators](#operators)
  - [F-String Reference](#f-string-reference)
  - [Operator Precedence](#operator-precedence)
- [Strings](#strings)
- [Lists](#lists)
- [Dictionaries](#dictionaries)
- [Tuples](#tuples)
- [Sets](#sets)
- [Control Flow](#control-flow)
- [Functions](#functions)
  - [Local Labels & Goto](#local-labels--goto)
- [Defer](#defer)
- [Decorators](#decorators)
- [Lambdas & Closures](#lambdas--closures)
- [Lazy Expressions](#lazy-expressions)
- [Classes](#classes)
  - [Fixed-Field Classes (`@fixed`)](#fixed-field-classes-fixed)
  - [Data Classes (`@data`)](#data-classes-data)
  - [Sealed Classes](#sealed-classes)
  - [Copy-on-Write Classes (`@cow`)](#copy-on-write-classes-cow)
- [Structs](#structs)
- [C Structs (`cstruct`)](#c-structs-cstruct)
- [`using` Keyword](#using-keyword)
- [Traits](#traits)
- [Trait Composition & Supertraits](#trait-composition--supertraits)
- [Trait Associated Types](#trait-associated-types)
- [Const Generics](#const-generics)
- [Pipe and Spread](#pipe-and-spread)
- [Runtime Introspection](#runtime-introspection)
- [Enums](#enums)
- [Generics](#generics)
- [Pattern Matching](#pattern-matching)
- [Error Handling](#error-handling)
- [Generators](#generators)
- [Async / Await](#async--await)
  - [Async and Threads Model](#async-and-threads-model)
- [Structured Concurrency](#structured-concurrency)
- [Macros](#macros)
- [Compile-Time Execution](#compile-time-execution)
- [Integer Overflow](#integer-overflow)
- [Tail-Call Optimization (TCO)](#tail-call-optimization-tco)
- [Warnings System](#warnings-system)
- [Compiler Diagnostics](#compiler-diagnostics)
- [Source Directives](#source-directives)
- [Modules & Imports](#modules--imports)
  - [Module Visibility — `pub(crate)` and `pub(super)`](#module-visibility--pubcrate-and-pubsuper)
- [Embedded Language Engines](#embedded-language-engines)
- [Custom Language Engines](#custom-language-engines)
- [Inline Assembly](#inline-assembly)
- [Actors & Agents](#actors--agents)
- [Isolates](#isolates)
- [Memory Safety and Borrowing](#memory-safety-and-borrowing)
- [Move Semantics](#move-semantics)
- [Manual Allocation](#manual-allocation)
- [Vectors & Tensors](#vectors--tensors)
- [Channels and Threads](#channels-and-threads)
- [Testing](#testing)
- [Builtins Reference](#builtins-reference)
- [Method Reference](#method-reference)
- [Operator Overloading](#operator-overloading)
- [Keywords](#keywords)
- [Standard Libraries](#standard-libraries)
  - [Stdlib Module Catalog](#stdlib-module-catalog)
  - [Universal Capability Profiles](#universal-capability-profiles)
  - [std.fs — Filesystem](#stdfs--filesystem)
  - [std.fmt — Formatting](#stdfmt--formatting)
  - [std.regex — Regular Expressions](#stdregex--regular-expressions)
  - [std.iter — Iterator Combinators](#stditer--iterator-combinators)
  - [std.time — Date & Time](#stdtime--date--time)
  - [std.proc — Process Management](#stdproc--process-management)
  - [std.log — Structured Logging](#stdlog--structured-logging)
  - [std.test — Testing (Enhanced)](#stdtest--testing-enhanced)
  - [std.math — Mathematics](#stdmath--mathematics)
  - [std.io — Input / Output](#stdio--input--output)
  - [std.collections — Data Structures](#stdcollections--data-structures)
  - [std.serialize — Serialization](#stdserialize--serialization)
  - [std.ai — Artificial Intelligence](#stdai--artificial-intelligence)
  - [std.crypto — Cybersecurity & Cryptography](#stdcrypto--cybersecurity--cryptography)
  - [std.compress — Compression](#stdcompress--compression)
  - [std.xml — XML & HTML Parsing](#stdxml--xml--html-parsing)
  - [std.image — Image Processing](#stdimage--image-processing)
  - [std.mail — Email](#stdmail--email)
  - [std.gfx3d — 3D Graphics](#stdgfx3d--3d-graphics)
  - [std.game — Game Creation](#stdgame--game-creation)
  - [std.net — Networking](#stdnet--networking)
  - [std.os — Operating System](#stdos--operating-system)
  - [std.db — Databases](#stddb--databases)
  - [std.ui — User Interface](#stdui--user-interface)
  - [std.term — Terminal & ANSI](#stdterm--terminal--ansi)
  - [std.cli — CLI Argument Parsing](#stdcli--cli-argument-parsing)
  - [std.csv — CSV Parsing & Writing](#stdcsv--csv-parsing--writing)
  - [std.toml — TOML Parsing](#stdtoml--toml-parsing)
  - [std.yaml — YAML Parsing](#stdyaml--yaml-parsing)
  - [std.uuid — UUID Generation](#stduuid--uuid-generation)
  - [std.rand — Random Numbers](#stdrand--random-numbers)
  - [std.hash — Non-Cryptographic Hashing](#stdhash--non-cryptographic-hashing)
  - [std.cache — In-Memory Caching](#stdcache--in-memory-caching)
  - [std.signal — OS Signal Handling](#stdsignal--os-signal-handling)
  - [std.http — HTTP Client & Server](#stdhttp--http-client--server)
  - [std.ffi — Foreign Function Interface](#stdffi--foreign-function-interface)
  - [std.audio — Audio Playback & Recording](#stdaudio--audio-playback--recording)
  - [std.video — Video Processing](#stdvideo--video-processing)
  - [std.pdf — PDF Generation & Reading](#stdpdf--pdf-generation--reading)
  - [std.excel — Excel / XLSX Files](#stdexcel--excel--xlsx-files)
  - [std.jwt — JSON Web Tokens](#stdjwt--json-web-tokens)
  - [std.oauth2 — OAuth 2.0](#stdoauth2--oauth-20)
  - [std.i18n — Internationalization](#stdi18n--internationalization)
  - [std.watch — Filesystem Watching](#stdwatch--filesystem-watching)
  - [std.grpc — gRPC Client & Server](#stdgrpc--grpc-client--server)
  - [std.mqtt — MQTT Messaging](#stdmqtt--mqtt-messaging)
  - [std.embed — Compile-Time File Embedding](#stdembed--compile-time-file-embedding)
  - [std.template — Text Templating](#stdtemplate--text-templating)
  - [std.multipart — Multipart Form Data](#stdmultipart--multipart-form-data)
  - [std.ssh — SSH Client & SFTP](#stdssh--ssh-client--sftp)
  - [std.qr — QR Code Generation](#stdqr--qr-code-generation)
  - [std.markdown — Markdown Parsing & Rendering](#stdmarkdown--markdown-parsing--rendering)
  - [std.archive — ZIP & TAR Archives](#stdarchive--zip--tar-archives)
  - [std.dns — DNS Resolution](#stddns--dns-resolution)
  - [std.2d — 2D Vector Graphics](#std2d--2d-vector-graphics)
  - [std.graphql — GraphQL Client & Server](#stdgraphql--graphql-client--server)
  - [std.webrtc — WebRTC](#stdwebrtc--webrtc)
  - [std.clipboard — Clipboard](#stdclipboard--clipboard)
  - [std.notify — Desktop Notifications](#stdnotify--desktop-notifications)
  - [std.speech — Text-to-Speech & Recognition](#stdspeech--text-to-speech--recognition)
  - [std.camera — Camera & Webcam](#stdcamera--camera--webcam)
  - [std.serial — Serial Port / UART](#stdserial--serial-port--uart)
  - [std.usb — USB Devices](#stdusb--usb-devices)
  - [std.bluetooth — Bluetooth & BLE](#stdbluetooth--bluetooth--ble)
  - [std.hotkey — Global Hotkeys](#stdhotkey--global-hotkeys)
  - [std.tray — System Tray](#stdtray--system-tray)
  - [std.ipc — Inter-Process Communication](#stdipc--inter-process-communication)
  - [std.decimal — Exact Decimal Arithmetic](#stddecimal--exact-decimal-arithmetic)
  - [std.diff — Text Diffing & Patching](#stddiff--text-diffing--patching)
  - [std.semver — Semantic Versioning](#stdsemver--semantic-versioning)
  - [std.geo — Geospatial](#stdgeo--geospatial)
  - [std.gpu — GPU Compute](#stdgpu--gpu-compute)
  - [std.accessibility — Accessibility APIs](#stdaccessibility--accessibility-apis)
  - [std.blockchain — Blockchain & Web3](#stdblockchain--blockchain--web3)
  - [std.parse — Parser Combinators](#stdparse--parser-combinators)
  - [std.config — Unified Configuration](#stdconfig--unified-configuration)
  - [std.event — Event Bus & PubSub](#stdevent--event-bus--pubsub)
  - [std.diag — Diagnostics & Observability](#stddiag--diagnostics--observability)
  - [std.iot — IoT & Embedded](#stdiot--iot--embedded)
  - [std.hal — Hardware Abstraction Layer](#stdhal--hardware-abstraction-layer)
  - [std.office — Office Documents](#stdoffice--office-documents)
  - [std.money — Money & Financial Arithmetic](#stdmoney--money--financial-arithmetic)
  - [std.dotenv — Environment Files](#stddotenv--environment-files)
  - [std.scrape — Web Scraping & Browser Automation](#stdscrape--web-scraping--browser-automation)
  - [std.map — Geospatial Rendering](#stdmap--geospatial-rendering)
  - [std.task — Persistent Task Queue](#stdtask--persistent-task-queue)
  - [std.phone — Telephony & SMS](#stdphone--telephony--sms)
  - [std.barcode — Barcode Generation & Scanning](#stdbarcode--barcode-generation--scanning)
  - [std.ml.vision — Vision Pipelines](#stdmlvision--vision-pipelines)
  - [std.ml.audio — Audio ML Pipelines](#stdmlaudio--audio-ml-pipelines)
- [Type Aliases](#type-aliases)
- [Destructuring Assignment](#destructuring-assignment)
- [if let / while let](#if-let--while-let)
- [Labeled Loops](#labeled-loops)
- [Union Types](#union-types)
- [The `never` Type](#the-never-type)
- [Effects System](#effects-system)
- [The `Default` Trait](#the-default-trait)
- [Weak References](#weak-references)
- [Newtype Wrappers](#newtype-wrappers)
- [The `@derive` Decorator](#the-derive-decorator)
- [Built-in Trait Catalog](#built-in-trait-catalog)
  - [`Sendable`](#sendable)
- [Numeric Casting](#numeric-casting)
- [Raw and Byte Strings](#raw-and-byte-strings)
- [Struct Update Syntax](#struct-update-syntax)
- [Bitfield Structs](#bitfield-structs)
- [Block Expressions](#block-expressions)
- [Comprehensions](#comprehensions)
- [Extension Methods](#extension-methods)
- [Computed Properties](#computed-properties)
- [Inline Value Types](#inline-value-types)
- [Trait Coherence and Orphans](#trait-coherence-and-orphans)
- [Do Blocks](#do-blocks)
- [Cross-Compilation](#cross-compilation)
- [Profiling](#profiling)

---

## Getting Started

### Installation

**Windows (x64).** Download the release, then run the installer — no manual file
copying or PATH editing required:

```bat
:: double-click install.bat, or from a terminal:
powershell -ExecutionPolicy Bypass -File install.ps1
```

The installer places `v2.exe` in `%LOCALAPPDATA%\Programs\v2\bin`, adds it to your
user `PATH`, bundles the offline documentation, and installs the reference package
registry (setting `V2_REGISTRY` so `v2 add <pkg>` works out of the box). Open a **new**
terminal afterward and verify:

```bash
v2 --version
v2 --help
```

To uninstall:

```bash
powershell -ExecutionPolicy Bypass -File install.ps1 -Uninstall
```

**Build from source** (any platform with a Rust toolchain):

```bash
cd v2
cargo build --release      # produces v2/target/release/v2
```

Once installed, `v2 --docs`, `v2 --internals`, and `v2 --packages` open the
documentation in your browser. See [`PACKAGES.md`](PACKAGES.md) for the package manager.

### Hello World

```v2
print("Hello, World!")
```

### Run a Program

```bash
v2 hello.v2
```

### Interactive REPL

```bash
v2
```

Type expressions or statements interactively. Use `exit` or `quit` to leave.

---

## Docs Modes

V2 documentation is split into two complementary tracks:

1. `DOCS.md` (normal docs): syntax, feature behavior, standard library usage, and day-to-day language reference.
2. `INTERNALS.md` (technical docs): compiler implementation, runtime architecture, feature internals, and execution model details.

### Interactive Docs

Use CLI flags to open a documentation track as a searchable HTML viewer in your browser:

```bash
v2 -D   # or  v2 --docs
v2 -I   # or  v2 --internals
v2 -P   # or  v2 --packages
```

- `--docs` opens the user-facing language documentation (`DOCS.md`).
- `--internals` opens the compiler/runtime internals documentation (`INTERNALS.md`).
- `--packages` opens the packages & package-manager guide (`PACKAGES.md`).

Each viewer renders the current Markdown source live with a table-of-contents sidebar and
full-text search, so it always reflects the latest documentation. These interactive viewers are
distinct from `v2 doc`, which generates API docs from source comments.

---

## Project Manifest

Every V2 project can have a `v2.toml` file at its root that declares metadata, dependencies, compiler settings, and build targets.

```toml
[project]
name    = "myapp"
version = "1.0.0"
entry   = "src/main.v2"
authors = ["Alice <alice@example.com>"]

[dependencies]
http-utils = "1.2.0"
json-extra = "0.9.1"
mylib      = { url = "https://github.com/user/mylib", version = "2.0.0" }
locallib   = { path = "../locallib" }

[dev-dependencies]
test-helpers = "0.3.0"

[compiler]
optimize     = true
target       = "native"     # "native" | "wasm" | "bytecode" (mobile uses "native" + [[target]].os)
overflow     = "panic"      # "wrap" | "saturate" | "panic"
tco          = true         # enable tail-call optimization
borrow_check = false        # set true to enable borrow checker project-wide
warn         = ["unused", "shadow", "unreachable"]
no_warn      = []
cfg          = { debug = false }

[runtime]
async_workers = 1           # number of async event-loop worker threads (default: 1)

[build]
out_dir  = "build"
script   = ""               # optional host-phase build script (e.g. "build.v2")
```

Mobile builds do not use a separate compiler `target` value. Keep `[compiler].target = "native"` and declare mobile outputs with `[[target]]` entries using `os = "android"` or `os = "ios"`.

#### `[runtime]` Keys

| Key             | Type  | Default | Description                                                                                                                                                                                                                                  |
| --------------- | ----- | ------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `async_workers` | `int` | `1`     | Number of worker threads for the async event loop. `1` = single-threaded (default). Values > 1 enable parallel async scheduling; values sent across workers must satisfy the `Sendable` trait. Equivalent to `--async-workers N` on the CLI. |

### `v2` Package Manager Commands

```bash
v2 init                  # Create a new v2.toml in the current directory
v2 init --lib            # Scaffold a library package (entry = src/lib.v2)
v2 add http-utils        # Add a dependency
v2 add http-utils@1.2.0  # Pin a specific version
v2 remove http-utils     # Remove a dependency
v2 install               # Install all dependencies from v2.toml
v2 install --frozen      # Install exactly what v2.lock declares (no resolution changes)
v2 install --engines     # Install managed embedded-language runtimes declared in v2.toml
v2 update                # Update dependencies to latest compatible versions
v2 lock                  # Generate or refresh v2.lock without installing
v2 login --token <token> # Authenticate with the package registry
v2 check                 # Type-check all .v2 source files without building or running
v2 profile               # Build and run with CPU & memory profiling enabled
v2 profile --out <file>  # Write profiling data to a file (default: profile.vtprof)
v2 profile --flame       # Generate a flamegraph SVG (requires v2-flamegraph tool)
v2 publish --dry-run     # Validate and package publish artifact without uploading
v2 publish               # Publish the package to the V2 package registry
v2 run                   # Build and run the project entry point
v2 build                 # Build without running
v2 test                  # Run all test blocks
v2 bench                 # Run all bench blocks
v2 clean                 # Remove build artifacts
v2 build --no-build-script  # Build without executing the configured host build script
v2 doc                   # Generate HTML documentation from doc comments
v2 fmt                   # Format all .v2 source files in the project
v2 fmt --check           # Check formatting without writing changes (exit 1 if any file differs)
v2 lint                  # Run the linter on all .v2 source files
v2 lint --fix            # Auto-fix all lint issues that have safe automatic fixes
```

### Package Registry

The V2 package registry is available at `pkg.v2.dev`. It hosts public packages published with `v2 publish`.

#### Publishing a Package

Before publishing, ensure your `v2.toml` has a `[project]` section with `name`, `version`, and `authors`:

```toml
[project]
name    = "my-utils"
version = "1.0.0"
authors = ["Alice <alice@example.com>"]
```

Authenticate once using your registry token (obtained from `pkg.v2.dev/settings`):

```bash
v2 login --token <your-token>
```

Then publish:

```bash
v2 publish
```

The CLI validates your `v2.toml`, runs all tests, and uploads the package. The version in `v2.toml` must be higher than any previously published version for the same package name.

Use `v2 publish --dry-run` first when releasing from CI or before tagging a version. It performs the same validation and artifact packaging steps without uploading anything.

#### Library Quickstart (Shareable Package)

The fastest path to publish a reusable library is:

1. Scaffold the library layout:

```bash
mkdir string-tools
cd string-tools
v2 init --lib
```

2. Fill package identity in `v2.toml`:

```toml
[project]
name        = "acme/string-tools"
version     = "0.1.0"
entry       = "src/lib.v2"
authors     = ["Acme Dev <dev@acme.dev>"]
description = "String helpers for V2 applications"
license     = "MIT"
repository  = "https://github.com/acme/string-tools"
readme      = "README.md"
keywords    = ["string", "text", "utility"]
```

3. Export your public API from `src/lib.v2` using `pub` declarations.
4. Add tests (`test` blocks) for every public behavior and run `v2 test`.
5. Run release preflight:

```bash
v2 publish --dry-run
```

6. Authenticate once and publish:

```bash
v2 login --token <your-token>
v2 publish
```

Consumers can then install the library with:

```bash
v2 add acme/string-tools
```

#### Yanking a Published Version

If a published version has a critical bug or security issue, you can **yank** it. Yanked versions are not shown in search results and cannot be newly installed, but existing lockfiles that reference the yanked version continue to work (preventing surprise breakage).

```bash
v2 yank acme/string-tools@0.1.0 --reason "Critical security vulnerability in auth module"
```

To un-yank (restore) a version:

```bash
v2 yank --undo acme/string-tools@0.1.0
```

Behavior:

- `v2 add acme/string-tools` will skip yanked versions when resolving.
- `v2 install --frozen` continues to work if the lockfile already pins the yanked version.
- `v2 install` (without `--frozen`) warns about yanked versions and suggests upgrading.

| Command                      | Description                                  |
| ---------------------------- | -------------------------------------------- |
| `v2 yank <pkg>@<ver>`        | Yank a version (soft-delete from resolution) |
| `v2 yank --undo <pkg>@<ver>` | Un-yank a version                            |
| `v2 yank --reason "<msg>"`   | Attach a reason (displayed in warnings)      |

#### Library Package Metadata

For discoverable and reusable libraries, keep this metadata in `[project]`:

| Key           | Required for publish | Purpose                                  |
| ------------- | -------------------- | ---------------------------------------- |
| `name`        | Yes                  | Registry identity (global or scoped)     |
| `version`     | Yes                  | SemVer release version                   |
| `authors`     | Yes                  | Maintainer identity                      |
| `entry`       | Yes for libraries    | Public library entrypoint (`src/lib.v2`) |
| `description` | Recommended          | Short package summary in registry/search |
| `license`     | Recommended          | Reuse/legal clarity for consumers        |
| `repository`  | Recommended          | Source and issue tracker link            |
| `readme`      | Recommended          | Rendered package landing page            |
| `keywords`    | Recommended          | Search/discovery tags                    |

#### Namespacing

Package names are globally unique on the registry. By convention, use a scope prefix for organizational packages:

```toml
name = "acme/http-client"     # scoped to the "acme" organization
```

#### Private Registries

Point V2 at a self-hosted registry by setting the `registry` key in `v2.toml`:

```toml
[registry]
url = "https://packages.internal.example.com"
token_env = "VT_REGISTRY_TOKEN"    # read token from this environment variable
```

All `v2 add`, `v2 install`, and `v2 publish` commands then target that registry. Multiple registries can be listed; the first one that resolves a package name wins:

```toml
[[registry]]
url = "https://packages.internal.example.com"

[[registry]]
url = "https://pkg.v2.dev"    # public registry as fallback
```

#### Version Conflict Resolution

When two dependencies require different versions of the same package, V2 applies these rules in order:

1. **Compatible range overlap** — if both requirements can be satisfied by a single version in the intersection of their ranges, that version is used.
2. **Duplication** — if no single version satisfies both, V2 links both versions simultaneously and each dependent gets the version it asked for. This is possible because packages are namespaced per-importer.
3. **Hard conflict** — if the dependency graph is unresolvable (e.g. two packages expose conflicting global state), `v2 install` errors and explains the conflict.

Run `v2 install --explain` to see the full dependency resolution graph.

### Version Pinning in URL Imports

When importing directly from a URL, pin a version using a `@version` suffix:

```v2
import "https://pkg.v2.dev/http-utils@1.2.0/mod.v2"
import "https://pkg.v2.dev/json-extra@^0.9.0/mod.v2"    // compatible range
```

Version specifiers:

| Spec       | Meaning                       |
| ---------- | ----------------------------- |
| `@1.2.0`   | Exact version                 |
| `@^1.2.0`  | Compatible (same major)       |
| `@~1.2.0`  | Patch-compatible (same minor) |
| `@>=1.0.0` | At least version              |

### Lockfiles & Workspaces

V2 supports reproducible installs via a lockfile and multi-package workspaces for monorepos.

#### `v2.lock` (Reproducible Dependency Graph)

`v2 lock` writes the fully resolved dependency graph to `v2.lock`.

```bash
v2 lock
v2 install --frozen    # fails if v2.toml and v2.lock are out of sync
```

Typical CI flow:

1. Commit both `v2.toml` and `v2.lock`.
2. Run `v2 install --frozen` in CI for deterministic builds.
3. Refresh lockfile only when dependencies intentionally change.

#### Workspace / Monorepo Projects

Define a workspace root with multiple local packages:

```toml
[workspace]
members = ["apps/api", "apps/web", "libs/core", "libs/net"]
```

Each member has its own `v2.toml`. Shared commands run from the workspace root:

```bash
v2 install              # resolves all members together
v2 test                 # runs tests across all members
v2 build                # builds all members
```

Member-local dependency reference:

```toml
[dependencies]
core = { path = "../../libs/core" }
```

This setup enables consistent versions, shared lockfile resolution, and atomic updates across the entire monorepo.

### Build Scripts (Host Phase)

V2 supports an explicit host build phase for tasks that must run before compilation (code generation, schema download, FFI binding generation, asset preprocessing).

Configure in `v2.toml`:

```toml
[build]
out_dir = "build"
script  = "build.v2"        # executed on host before compile
```

`build.v2` runs in a restricted host context and can use build-only APIs such as `build_read_file`, `build_write_file`, `build_http_get`, and `build_exec`.

```v2
// build.v2
let schema = build_http_get("https://api.example.com/schema")
build_write_file("generated/schema.json", schema)
```

Run behavior:

- `v2 build` / `v2 run` executes the build script by default when configured.
- `v2 build --no-build-script` skips it.

### Managed Embedded Runtimes

To remove system-level runtime dependency drift for embedded engines, projects can pin and install managed toolchains in `v2.toml`:

```toml
[engines]
policy = "managed"        # "system" | "managed" | "mixed"
python = "3.12"
node   = "20"
lua    = "5.4"
```

Install pinned runtimes:

```bash
v2 install --engines
```

When `policy = "managed"`, `@py`, `@js`, and other engine blocks resolve to the pinned toolchain first, then fail only if the pinned runtime is unavailable.

---

## CLI Usage

```
v2 [options] [file.v2]
```

| Flag                        | Long Flag     | Description                                                                                |
| --------------------------- | ------------- | ------------------------------------------------------------------------------------------ |
| (none)                      |               | Start interactive REPL                                                                     |
| `file.v2`                   |               | Run a V2 program                                                                           |
| `-c`                        | `--compile`   | Compile to bytecode                                                                        |
| `--coverage`                |               | Run with code coverage instrumentation; emit line/branch report                            |
| `--coverage --out <dir>`    |               | Write coverage report to directory (default: `./coverage/`)                                |
| `--coverage --format <f>`   |               | Coverage output format: `text` (default), `html`, `lcov`, `json`                           |
| `-r`                        | `--run`       | Run bytecode after compilation                                                             |
| `-i`                        | `--interpret` | Force interpreter mode                                                                     |
| `-d`                        | `--debug`     | Debug mode (show tokens, AST, then execute)                                                |
| `-O`                        | `--optimize`  | Enable bytecode optimizations (default)                                                    |
| `-O0`                       |               | Disable optimizations                                                                      |
| `-S`                        | `--disasm`    | Show bytecode disassembly                                                                  |
| `-o <file>`                 |               | Custom output filename                                                                     |
| `-V`                        | `--verbose`   | Verbose diagnostic output                                                                  |
| `-v`                        | `--version`   | Print version                                                                              |
| `-h`                        | `--help`      | Print help                                                                                 |
| `-D`                        | `--docs`      | Open interactive user documentation (`DOCS.md`) in the terminal                            |
| `-I`                        | `--internals` | Open interactive internals documentation (`INTERNALS.md`) in the terminal                  |
| `--dump-cfg <file?>`        |               | Export control-flow graph as Graphviz `.dot` file                                          |
| `--target <t>`              |               | Compile target: `native`, `wasm`, `bytecode`, `exe` (self-contained executable)            |
| `--arch <a>`                |               | Target architecture: `x86_64`, `arm64`, `wasm32`                                           |
| `--os <os>`                 |               | Target OS: `linux`, `windows`, `macos`, `android`, `ios`, `none` (bare-metal/freestanding) |
| `--wasm-cap <caps>`         |               | Enable WASM host capabilities (comma-separated): `ffi`, `proc`, `fs`, `net`                |
| `--incremental`             |               | Enable incremental compilation (cache unchanged files)                                     |
| `--cache-dir <dir>`         |               | Directory for incremental compilation cache                                                |
| `--async-workers <n>`       |               | Run async scheduler with `n` worker threads (default: 1)                                   |
| `--build-script <file>`     |               | Override build script path for this invocation                                             |
| `--embed-engines`           |               | Force managed embedded runtime resolution for this build                                   |
| `--strict-unsafe`           |               | Elevate unsafe diagnostics and reject unchecked pointer operations                         |
| `--sanitizer <s>`           |               | Enable sanitizer: `address`, `ub`, `thread`, `leak`                                        |
| `--lsp`                     |               | Start the V2 Language Server (LSP) on stdio                                                |
| `--lsp-port <n>`            |               | Start LSP on a TCP port instead of stdio                                                   |
| `--step-debug`              |               | Launch step debugger (breakpoints, watchpoints, call stack)                                |
| `--break <file:line>`       |               | Set a breakpoint at file:line for `--step-debug`                                           |
| `--test`                    |               | Run all `test` blocks in the file                                                          |
| `--test --tag <t>`          |               | Run only tests with a given tag                                                            |
| `--test --skip-tag <t>`     |               | Skip tests with a given tag                                                                |
| `--test --update-snapshots` |               | Regenerate snapshot baselines                                                              |
| `--bench`                   |               | Run all `bench` blocks                                                                     |
| `--overflow <mode>`         |               | Integer overflow: `wrap`, `saturate`, `panic` (default)                                    |
| `--no-tco`                  |               | Disable tail-call optimization                                                             |
| `--warn <w>`                |               | Enable specific warning category                                                           |
| `--no-warn <w>`             |               | Suppress specific warning category                                                         |
| `--doc`                     |               | Generate documentation from doc comments (`///`, `/** */`)                                 |
| `--doc --out <dir>`         |               | Custom output directory for generated docs                                                 |
| `--doc --format <f>`        |               | Doc output format: `html` (default), `markdown`                                            |
| `--profile`                 |               | Profile program and print summary                                                          |
| `--profile --flame`         |               | Generate flamegraph SVG                                                                    |

Flag distinction:

- `--docs` and `--internals` open interactive terminal viewers.
- `--doc` generates API documentation from source comments (equivalent in purpose to `v2 doc`).

### Examples

```bash
v2 hello.v2                            # Interpret and run
v2 -c hello.v2                         # Compile to bytecode
v2 -c -r hello.v2                      # Compile and run
v2 -c --target exe hello.v2            # Bundle into self-contained executable
v2 -c --target wasm hello.v2           # Compile to WebAssembly
v2 -c --target wasm --wasm-cap ffi,proc hello.v2  # WASM with host capability bridge
v2 --async-workers 8 app.v2            # Multi-worker async runtime
v2 -c --target native --arch arm64 --os linux hello.v2   # Cross-compile
v2 --sanitizer address --strict-unsafe app.v2  # Safety-instrumented run
v2 -c --incremental hello.v2           # Incremental compile
v2 --lsp                               # Start LSP server
v2 --step-debug --break hello.v2:10 hello.v2  # Step debugger
v2 --test hello.v2                     # Run all tests
v2 --overflow wrap hello.v2            # Wrapping integer arithmetic
v2 --docs                              # Open interactive user docs
v2 --internals                         # Open interactive internals docs
v2 --doc                               # Generate API docs from comments
v2 --doc --out site/api                # Write generated docs to a custom directory
v2 --doc --format markdown             # Generate docs as Markdown
v2 --coverage --test app.v2            # Run tests with code coverage
v2 --coverage --format html app.v2     # Generate HTML coverage report
v2 --profile app.v2                    # CPU profile and print summary
v2 --profile --flame app.v2            # Generate flamegraph SVG
```

---

## WASM Target

V2 can compile to WebAssembly for use in browsers, Node.js, and WASI runtimes.

### Compiling to WASM

```bash
v2 -c --target wasm hello.v2           # produces hello.wasm
v2 -c --target wasm -o myapp hello.v2  # custom output name
```

### Loading in a Browser

```html
<script type="module">
  import { load_vt } from "./vt_runtime.js";

  const v2 = await load_vt("hello.wasm");
  v2.call("main");
</script>
```

### WASM-Exported Functions

Mark functions `pub` and annotate with `@wasm_export` to expose them to JavaScript:

```v2
@wasm_export
pub func add(a: int, b: int) -> int {
    return a + b
}

@wasm_export
pub func greet(name: str) -> str {
    return f"Hello, ${name}!"
}
```

Call from JavaScript:

```js
const v2 = await load_vt("myapp.wasm");
console.log(v2.add(1, 2)); // 3
console.log(v2.greet("Alice")); // Hello, Alice!
```

### Importing from JavaScript

```v2
@wasm_import("env", "js_log")
extern func js_log(msg: str)

@wasm_import("env", "now")
extern func js_now() -> float

js_log("Hello from V2!")
let ts = js_now()
```

### WASI Support

Compile targeting WASI for server-side WASM runtimes (Wasmtime, WasmEdge):

```bash
v2 -c --target wasm --wasi hello.v2
wasmtime hello.wasm
```

WASI mode enables filesystem, stdio, and environment access through the WASI API. All `std.fs`, `std.io`, and `print` calls work transparently.

### WASM Limitations

| Feature         | Status in WASM                                                                |
| --------------- | ----------------------------------------------------------------------------- |
| `std.fs`        | WASI native; browser via host capability bridge (`--wasm-cap fs`)             |
| `std.net`       | Browser: fetch API native; sockets via host capability bridge or WASI-sockets |
| Threading       | Native in WASI; browser requires `SharedArrayBuffer` + COOP/COEP headers      |
| Inline assembly | Not native in WASM; host-side intrinsics via capability bridge                |
| `extern c` FFI  | Via `extern wasm_host` / Component-model host adapter (`--wasm-cap ffi`)      |
| `std.proc`      | Via capability-gated host process bridge (`--wasm-cap proc`)                  |

### WASM Host Capability Bridge

V2 can expose host-provided capabilities to WASM modules explicitly. This keeps browser/WASI sandboxes safe by default while allowing opt-in privileged features.

```v2
@wasm_host_import("proc_spawn")
extern wasm_host func proc_spawn(cmd: str, args: list<str>) -> int

@wasm_host_import("ffi_call")
extern wasm_host func ffi_call(lib: str, symbol: str, payload: bytes) -> bytes
```

Compile with explicit capability grants:

```bash
v2 -c --target wasm --wasm-cap proc,ffi app.v2
```

If a module calls a host capability that was not granted, the runtime raises a capability error at the call site.

---

## Step Debugger

Launch the step debugger with `--step-debug`:

```bash
v2 --step-debug hello.v2
v2 --step-debug --break hello.v2:10 hello.v2   # break at line 10 on start
```

### Debugger Commands

Once paused at a breakpoint the debugger accepts these commands:

| Command             | Shorthand | Description                                       |
| ------------------- | --------- | ------------------------------------------------- |
| `step`              | `s`       | Execute one line, stepping into function calls    |
| `next`              | `n`       | Execute one line, stepping over function calls    |
| `continue`          | `c`       | Resume until next breakpoint                      |
| `finish`            | `f`       | Run until current function returns                |
| `break <file:line>` | `b`       | Set a breakpoint                                  |
| `break <funcname>`  | `b`       | Break at function entry                           |
| `delete <id>`       | `d`       | Delete a breakpoint by ID                         |
| `breakpoints`       | `bp`      | List all breakpoints                              |
| `print <expr>`      | `p`       | Evaluate and print an expression in current scope |
| `watch <expr>`      | `w`       | Break whenever expression value changes           |
| `backtrace`         | `bt`      | Print the call stack                              |
| `frame <n>`         | `fr`      | Switch to stack frame N                           |
| `locals`            | `l`       | Print all local variables in current frame        |
| `up` / `down`       |           | Move up/down the call stack                       |
| `quit`              | `q`       | Exit the debugger                                 |

### Example Session

```
$ v2 --step-debug --break hello.v2:5 hello.v2

[BREAK] hello.v2:5 — func main()
  3 | func main() {
  4 |     let x = 10
> 5 |     let y = compute(x)
  6 |     print(y)
  7 | }

(v2-dbg) p x
x = 10
(v2-dbg) s
[BREAK] hello.v2:2 — func compute(n)
> 2 |     return n * n + 1
(v2-dbg) p n
n = 10
(v2-dbg) c
101
[Program exited with code 0]
```

---

## Comments & Doc Comments

```v2
// Single-line comment

/* Multi-line
   comment */
```

### Doc Comments

Doc comments begin with `///` for single-line or `/** ... */` for multi-line. They attach to the declaration immediately following them and are extracted by `v2 doc`.

```v2
/// Adds two integers and returns the result.
func add(a: int, b: int) -> int {
    return a + b
}

/**
 * Represents a 2D point in space.
 *
 * @field x  The horizontal coordinate.
 * @field y  The vertical coordinate.
 */
struct Point {
    x: float,
    y: float
}

/// Computes the Euclidean distance between two points.
/// @param a  The first point.
/// @param b  The second point.
/// @returns  Distance as a float.
func distance(a: Point, b: Point) -> float {
    let dx = a.x - b.x
    let dy = a.y - b.y
    return sqrt(dx * dx + dy * dy)
}
```

Doc tags recognized by the tooling:

| Tag                      | Description                         |
| ------------------------ | ----------------------------------- |
| `@param name desc`       | Documents a parameter               |
| `@returns desc`          | Documents the return value          |
| `@throws ErrorType desc` | Documents a thrown error            |
| `@deprecated msg`        | Marks the item as deprecated        |
| `@see other_func`        | Cross-reference to another item     |
| `@example`               | Begins an inline code example block |

Generate HTML documentation for a project:

```bash
v2 doc                        # generates docs/ from all .v2 files
v2 doc --out site/api         # custom output directory
v2 doc --format markdown      # output as Markdown instead of HTML
```

`v2 doc` respects visibility — only `pub` items appear in generated documentation by default. Pass `--all` to include `internal` and `private` items.

---

## Variables & Constants

### Main Variable Declaration Syntax

```v2
[const/let] [Data Type] (Variable Name) [":"Size Type] = (Value)
```

> [] = optional
> () = required
> The optional fields will be filled out automatically by the compiler

### Variable Declaration

```v2
let name = "Alice"
let age = 30
let pi = 3.14159
let active = true
let nothing = null
```

`var` is accepted as an alias for `let` (bindings are mutable either way):

```v2
var counter = 0
counter += 1
```

### Constants

```v2
const MAX_SIZE = 100
const PI = 3.14159
const myValue = 42       // lowercase is fine too
const greeting = "hello"
```

Constants cannot be reassigned after declaration.

> **Naming convention:** `const` only enforces immutability — it does **not** require uppercase names. `ALL_CAPS` (e.g. `MAX_SIZE`) is a common convention used in the V2 standard library to signal "this is a constant", but lowercase and camelCase constant names like `myValue` or `defaultTimeout` are perfectly valid.

### Compile-Time Constants (`comptime const`)

A `comptime const` is guaranteed to be evaluated at compile time — its value is baked into the binary, not computed at runtime. Expressions must be fully resolvable at compile time.

```v2
comptime const MAX_BUF = 1024 * 4          // 4096 — computed at compile time
comptime const PLATFORM = ct_platform()    // "linux" / "windows" / "macos"
comptime const IS_64BIT = mem_size_of("i64") == 8

// Can be used in static_assert
static_assert(MAX_BUF > 0, "Buffer must be positive")

// Conditionally compile based on comptime const
comptime {
    if (!IS_64BIT) {
        ct_error("V2 requires a 64-bit platform")
    }
}
```

`const` (without `comptime`) means immutable at runtime — the value may still be computed lazily. `comptime const` is the guarantee that it is a compile-time literal.

### Integer Literal Separators

Underscores can be used anywhere inside a numeric literal to improve readability. They are ignored by the compiler.

```v2
let million   = 1_000_000
let hex_color = 0xFF_AA_00
let big_float = 3.141_592_653
let bin_mask  = 0b1111_0000
```

### Type Annotations (Optional)

```v2
// Prefix style
let int x = 42
let str name = "Alice"
let bool active = true
let float pi = 3.14
let list nums = [1, 2, 3]
let dict person = {"name": "Bob"}

// Explicitly sized annotation forms
let int x:i32 = 1
const int y:u64 = 2
int z:f32 = 3.14
a:i8 = 4
```

Available type annotations: `int`, `float`, `str`, `bool`, `void`, `list`, `dict`, `pointer`, `tuple`, `set`, `any`.

**`void`** — used as the return type of a function that returns no meaningful value (returns `null` implicitly). Equivalent to omitting the return type annotation entirely. It is not a value you can store in a variable.

```v2
let handler: func(str) -> void = lambda(msg) { print(msg) }
// equivalent to:
let handler: func(str) = lambda(msg) { print(msg) }
```

**`pointer`** — an opaque raw-memory address, produced by `mem_alloc` or passed in from C via `extern c`. It holds no type information and can only be meaningfully used inside `unsafe` blocks with `mem_read` / `mem_write` / `mem_free`. It is distinct from a borrow (`&x`), which the borrow checker can reason about.

```v2
func write_hardware_reg(addr: pointer, val: u32) [effects: unsafe] {
    unsafe { mem_write(addr, 0, val) }
}
```

### Function Type Signatures

When a variable, parameter, or field holds a function, you can annotate it with a typed signature using `func(ParamTypes...) -> ReturnType`. This lets the compiler verify that the correct function shape is passed.

```v2
// A function that takes two ints and returns a bool
let predicate: func(int, int) -> bool = lambda(a, b) => a > b

// A callback with no arguments and no return value
let on_click: func() -> void = lambda() { print("clicked") }

// A transformer: takes a str, returns a str
func apply_transform(text: str, f: func(str) -> str) -> str {
    return f(text)
}

apply_transform("hello", lambda(s) => s.upper())    // "HELLO"
```

Function type signatures in parameters, fields, and return types:

```v2
// As a struct field
struct EventHandler {
    on_data:  func(str) -> void,
    on_error: func(Error) -> bool,
    on_close: func() -> void
}

// As a return type — returns a function
func make_adder(n: int) -> func(int) -> int {
    return lambda(x) => x + n
}

let add5: func(int) -> int = make_adder(5)
add5(10)    // 15
```

Variadic functions use `...` in the signature:

```v2
let logger: func(str, ...any) -> void = lambda(level, ...args) {
    print(f"[${level}]", ...args)
}
```

When the return type is `void`, it can be omitted from the annotation:

```v2
let handler: func(str)    // equivalent to func(str) -> void
```

The plain `func` annotation (without signature) still works when the shape doesn't need to be enforced — it opts into dynamic function typing for that binding.

### Union Types (Preview)

Use `|` to express values that may hold one of several types:

```v2
let id: int | str = 42
id = "user-42"
```

Use `is` checks or `match` to narrow before type-specific operations. For full syntax, narrowing rules, and exhaustiveness details, see [Union Types](#union-types).

### Type Aliases and Newtypes (Preview)

Use `type` for transparent aliases and `newtype` for strongly distinct wrappers:

```v2
type UserId = int
newtype OrderId = int
```

Aliases are interchangeable with the original type; newtypes are not. For complete guidance and trait derivation behavior, see [Type Aliases](#type-aliases) and [Newtype Wrappers](#newtype-wrappers).

### Generic Bounds (Preview)

Use trait bounds (`T: TraitA + TraitB`) and `where` clauses to constrain generic APIs.

```v2
func max_of<T: Comparable>(a: T, b: T) -> T {
    return a > b ? a : b
}
```

For full coverage, see [Generics](#generics), [Trait Associated Types](#trait-associated-types), and [Trait Coherence and Orphans](#trait-coherence-and-orphans).

### Sized Numeric Types

Sized numeric annotations are also supported in the runtime:

- Integers: `i8`, `i16`, `i32`, `i64`, `u8`, `u16`, `u32`, `u64`
- Floats: `f32`, `f64`
  Numeric assignments auto-adapt across compatible numeric sizes. For example, assigning a float result into an integer-sized variable promotes it to a float-sized type instead of failing.

### Destructuring Assignment (Preview)

V2 supports list, tuple, and struct/dict destructuring, including rest patterns (`...rest`) and ignore bindings (`_`).

```v2
let [head, ...tail] = [1, 2, 3, 4]
let (name, age) = ("Alice", 30)
let { x, y } = Point { x: 10, y: 20 }
```

For complete destructuring patterns in `let`, function parameters, loops, and `match`, see [Destructuring Assignment](#destructuring-assignment).

### Assignment

```v2
x = 42
x += 10        // Compound assignment
x -= 5
x *= 2
x /= 3

x++            // Post-increment statement (equivalent to x += 1)
x--            // Post-decrement statement (equivalent to x -= 1)
```

> `x++` and `x--` are **statement-only** — they cannot appear inside expressions. See [Assignment Operators](#assignment) for full semantics.

---

## Variable Scoping Rules

V2 uses **lexical block scoping**. A binding declared with `let` or `const` is visible from its declaration point to the end of its enclosing block (`{}`).

### Block Scope

```v2
{
    let x = 10
    print(x)    // OK
}
print(x)        // ERROR — x is out of scope
```

### Loop Scope

Variables declared in a loop header are scoped to the loop body:

```v2
for (let i = 0; i < 10; i++) {
    print(i)    // OK inside loop
}
print(i)        // ERROR — i is out of scope
```

`for-in` binding variables are also loop-scoped:

```v2
for (item in [1, 2, 3]) {
    print(item)    // OK
}
// item not visible here
```

**`for-in` binding semantics:** The loop variable (e.g. `item`) is implicitly declared with `let` at the start of each iteration — you do not write `let item` yourself. This means:

- The binding is **read-only by default** inside the loop body. Assigning `item = something_else` is a compile error; use a separate `let` if you need a mutable local copy.
- A **fresh slot** is created for each iteration, but closures capture variables **by reference** (live binding), not by value. This means closures defined inside the loop body all hold a reference to the same loop variable and will see its final value after the loop ends. Use a default-parameter freeze (`lambda(i = i) => ...`) to capture the current value per iteration — see [Lambdas & Closures](#lambdas--closures).
- The loop variable **shadows** any outer variable with the same name (a `shadow` warning is emitted by default — suppress with the inline directive comment `// @suppress shadow` if intentional).
- Destructuring is supported directly in the header: `for ([k, v] in pairs)` or `for ((x, y) in points)`.

```v2
let item = "outer"
for (item in ["a", "b", "c"]) {    // @suppress shadow — intentional
    print(item)    // "a", "b", "c" — loop binding, not outer
    // item = "x"  // ERROR — loop binding is immutable
    let copy = item    // OK — make a mutable local copy
}
print(item)    // "outer" — outer binding unaffected
```

### Shadowing

An inner `let` binding may shadow an outer one. The outer binding is unchanged; the inner binding is an independent variable with the same name, visible only within its own block:

```v2
let x = 1
{
    let x = 2     // shadows outer x
    print(x)      // 2
}
print(x)          // 1 — outer x is unaffected
```

The compiler emits a `shadow` warning by default. Suppress it for intentional shadowing with `// @suppress shadow` on the relevant line, or disable the warning globally with `--no-warn shadow`.

### Re-declaration

Re-declaring a variable with `let` in the **same scope** is a compile error:

```v2
let x = 1
let x = 2    // ERROR — x already declared in this scope
x = 2        // OK — reassignment (no let)
```

### Closures Capture by Reference

Lambdas and closures capture variables from the enclosing scope by reference (live binding), not by value at capture time:

```v2
let count = 0
let inc = lambda() { count += 1 }
inc()
inc()
print(count)    // 2 — the closure mutated the outer binding
```

To capture the current value instead, use an immediately-evaluated parameter default or an explicit copy:

```v2
let val = 10
let snapshot = lambda(v = val) { print(v) }
val = 99
snapshot()    // 10 — captured at definition time via default param
```

---

## Data Types

### Primitive Types

| Type    | Example                                     | Description                                             |
| ------- | ------------------------------------------- | ------------------------------------------------------- |
| `int`   | `42`, `0xFF`, `0b1010`, `0o77`, `1_000_000` | Integer (decimal, hex, binary, octal; `_` as separator) |
| `float` | `3.14`, `1.5e10`                            | Floating-point number                                   |
| `str`   | `"hello"`                                   | String                                                  |
| `bool`  | `true`, `false`                             | Boolean                                                 |
| `null`  | `null`                                      | Null / absence of value                                 |

### Collection Types

| Type    | Example              | Description                           |
| ------- | -------------------- | ------------------------------------- |
| `list`  | `[1, 2, 3]`          | Ordered, mutable collection           |
| `dict`  | `{"key": "value"}`   | Key-value mapping                     |
| `tuple` | `(1, "hello", true)` | Ordered, immutable collection         |
| `set`   | `#{1, 2, 3}`         | Unordered collection of unique values |

### Special Types

| Type               | Description                                  |
| ------------------ | -------------------------------------------- |
| `Option`           | `Some(value)` or `None` — optional values    |
| `Result`           | `Ok(value)` or `Err(error)` — error handling |
| `function`         | First-class function value                   |
| `generator`        | Generator instance from `func*`              |
| `class` / `object` | Class definition / instance                  |
| `struct`           | Struct instance                              |
| `enum`             | Enum variant                                 |
| `range`            | Lazy range iterator from `range()` or `..`   |

### The `any` Type

`any` is V2's dynamic escape hatch — a variable of type `any` can hold a value of any other type at any point in time. It opts that binding out of static type checking: the compiler accepts any operation on an `any` value, and type errors become runtime errors instead.

#### When `any` Is Used

The compiler infers `any` when it cannot determine a more specific type:

```v2
let x = []            // list of any — element type unknown
let y = {}            // dict with any values
let z                 // declared without value — inferred as any
```

You can also declare `any` explicitly:

```v2
let val: any = 42
val = "hello"         // fine — any accepts reassignment to a different type
val = [1, 2, 3]       // fine
```

#### `any` vs Union Types

`any` and union types (`int | str`) serve different purposes:

|                | `any`                                 | `int \| str`                          |
| -------------- | ------------------------------------- | ------------------------------------- |
| Allowed values | Literally anything                    | Only the listed types                 |
| Type errors    | Runtime                               | Compile time (when provable)          |
| Narrowable     | Yes, with `is` or `match (type(...))` | Yes, with `is` or `match (type(...))` |
| Intent         | Unknown / dynamic                     | Known set of possibilities            |

Prefer union types when you know the set of possible types. Use `any` only when the type is genuinely unknown — for example, when deserializing external data, working with heterogeneous collections, or interfacing with dynamically typed embedded language engines.

#### Narrowing `any`

An `any` value can be narrowed to a concrete type using `is` or `match (type(...))`:

```v2
func describe(val: any) {
    if (val is int) {
        print(f"integer: ${val + 1}")      // val is int here
    } elif (val is str) {
        print(f"string: ${val.upper()}")   // val is str here
    } elif (val is list) {
        print(f"list of ${val.len()} items")
    } else {
        print(f"something else: ${type(val)}")
    }
}

describe(42)         // integer: 43
describe("hello")    // string: HELLO
describe([1, 2, 3])  // list of 3 items
```

With `match`:

```v2
func process(val: any) {
    match (type(val)) {
        case ("int") { return val * 2 }
        case ("str") { return val.trim() }
        case ("list") { return val.len() }
        case ("null") { return 0 }
        default { throw new TypeError(f"Unsupported type: ${type(val)}") }
    }
}
```

#### `any` in Collections

Lists and dicts without explicit element type annotations hold `any` values:

```v2
let mixed: list = [1, "two", true, null]    // list of any — explicitly typed
let record = {}                              // dict with any values

mixed.push(3.14)    // fine
mixed.push([])      // fine
```

To get a more specific collection type, annotate the variable or use a typed struct:

```v2
let int_list: list = []       // still list of any — V2 does not have generic list<T> literals
                               // use a typed struct or Generics for strict element typing
```

> **`list<T>` in type annotations vs. literals:** While you cannot write a generic list _literal_ like `list<int>(range(5))`, you _can_ use `list<T>` as a type annotation or return type — for example `-> list<float>` or `let result: list<float> = ...`. The `<T>` annotation constrains what the compiler accepts going in and out; it does not change the runtime representation.

#### The `implicit_any` Warning

When the compiler infers `any` where a more specific type might have been intended, it emits an `implicit_any` warning. This is off by default. Enable it to catch unintentionally dynamic code:

```bash
v2 --warn implicit_any myapp.v2
```

Suppress per-line when `any` is intentional:

```v2
let data: any = fetch_dynamic_json()    // @suppress implicit_any
```

#### `any` and the Type System

- Calling a method on `any` that does not exist on the runtime value throws a `TypeError` at runtime.
- `any` is assignable to any type annotation — it effectively bypasses the type checker at the point of use.
- `comptime` code cannot operate on `any`-typed values; all types must be fully resolved at compile time.

### Truthiness

| Falsy                                                | Truthy          |
| ---------------------------------------------------- | --------------- |
| `null`, `false`, `0`, `0.0`, `""`, `[]`, `{}`, `#{}` | Everything else |

> **Empty set `#{}`** is falsy, consistent with `[]` (empty list) and `{}` (empty dict). Use `#{}` as the empty-set literal; `set([])` is equivalent.
>
> **Tuples `()`** — tuples are immutable and fixed-length. An empty tuple `()` is **truthy** (the value exists; it simply has no elements). This differs from empty collections because a tuple's length is part of its type, not a runtime property.

### Type Checking

```v2
type(42)          // "int"
type("hello")     // "str"
type([1, 2])      // "list"
typeof(value)     // raw type tag
```

**`type(val)`** returns a human-readable string name — always one of the language keywords like `"int"`, `"str"`, `"list"`, `"MyClass"`. This is what you use in `match` arms and logging.

**`typeof(val)`** returns the internal raw type tag — a low-level numeric or structured identifier used by the runtime's type system. It is mainly useful for performance-sensitive reflection, serialization engines, or FFI code that needs to distinguish between sized variants (`"i32"` vs `"i64"`) that `type()` both report as `"int"`. In most application code, prefer `type()` or `val is Type` — `typeof` is an advanced escape hatch.

Docs display tag values in atom/tag literal form with a leading colon (for example `:int`). The leading `:` means "tag value", not string text.

```v2
type(42)           // "int"      — human-readable, language-level
typeof(42)         // :int       — raw runtime type tag (tag type may vary by platform)

// Use type() for comparisons and match:
match (type(val)) {
    case ("int") { return val * 2 }
    case ("str") { return val.trim() }
}

// Use val is Type for type narrowing:
if (val is int) { return val + 1 }
```

### Type Conversion

```v2
int("42")       // 42
float("3.14")   // 3.14
str(42)         // "42"
bool(1)         // true
list("abc")     // ["a", "b", "c"]
dict(obj)       // dict representation
set([1,2,2])    // #{1, 2}
tuple([1,2])    // (1, 2)
resize(7, "f32") // 7.0 (numeric size conversion)
```

**`resize(value, type_str)`** converts a numeric value to a specific sized numeric type. The second argument is a type-string using the same notation as V2's sized numeric types:

| Type string                       | Meaning                             |
| --------------------------------- | ----------------------------------- |
| `"i8"`, `"i16"`, `"i32"`, `"i64"` | Signed integer of given bit width   |
| `"u8"`, `"u16"`, `"u32"`, `"u64"` | Unsigned integer of given bit width |
| `"f32"`, `"f64"`                  | Floating-point of given bit width   |

```v2
resize(255, "u8")    // 255  — fits in u8
resize(300, "u8")    // 44   — wraps (300 mod 256)
resize(3, "f32")     // 3.0  — integer promoted to f32
resize(3.9, "i32")   // 3    — float truncated to i32
```

These type strings are the same format used by `mem_size_of`, `static_assert`, and `cstruct` field declarations.

### Base Conversion

Parse integers from strings in any numeric base, and format integers as strings in any base.

```v2
// Parsing integers with an explicit base
int("ff", 16)       // 255  — hexadecimal
int("1010", 2)      // 10   — binary
int("77", 8)        // 63   — octal
int("z", 36)        // 35   — base-36

// Prefix-aware parsing (base inferred from prefix when base omitted)
int("0xFF")         // 255  — 0x prefix ? base 16
int("0b1010")       // 10   — 0b prefix ? base 2
int("0o77")         // 63   — 0o prefix ? base 8

// Formatting integers as strings in a given base
str(255, base: 16)  // "ff"
str(10,  base: 2)   // "1010"
str(63,  base: 8)   // "77"

// The built-in hex() and bin() shortcuts
hex(255)            // "ff"      — same as str(255, base: 16)
bin(10)             // "1010"    — same as str(10,  base: 2)
oct(63)             // "77"      — same as str(63,  base: 8)
```

`int(str, base)` throws `ValueError` if the string contains characters invalid for the given base.

---

## Operators

### Arithmetic

| Operator | Description                            | Example               |
| -------- | -------------------------------------- | --------------------- |
| `+`      | Addition / string concat / list concat | `3 + 4` ? `7`         |
| `-`      | Subtraction                            | `10 - 3` ? `7`        |
| `*`      | Multiplication / string repeat         | `4 * 3` ? `12`        |
| `/`      | Division (always float result)         | `10 / 3` ? `3.333...` |
| `//`     | Floor division (integer result)        | `10 // 3` ? `3`       |
| `%`      | Modulo                                 | `10 % 3` ? `1`        |
| `**`     | Exponentiation                         | `2 ** 8` ? `256`      |

```v2
10 / 3       // 3.333...  — float division
10 // 3      // 3         — floor division (rounds toward -8)
-7 // 2      // -4        — NOT -3 (floor, not truncation)
2 ** 10      // 1024
2.0 ** 0.5   // 1.4142...  — fractional exponents work too
```

Compound assignment:

```v2
x //= 3    // floor divide and assign
x **= 2    // exponentiate and assign
```

### Comparison

| Operator | Description           |
| -------- | --------------------- |
| `==`     | Equal                 |
| `!=`     | Not equal             |
| `<`      | Less than             |
| `>`      | Greater than          |
| `<=`     | Less than or equal    |
| `>=`     | Greater than or equal |

### Logical

| Operator | Description                 |
| -------- | --------------------------- |
| `&&`     | Logical AND (short-circuit) |
| `\|\|`   | Logical OR (short-circuit)  |
| `!`      | Logical NOT                 |

> **`not` is a keyword, not an operator.** It is used exclusively in the compound `not in` expression (`5 not in [1,2,3]`). It is **not** a standalone negation — use `!` for that. `not` does not appear in this table because it has no meaning outside of `not in`.

### Assignment

| Operator | Description                                              |
| -------- | -------------------------------------------------------- |
| `=`      | Assignment                                               |
| `+=`     | Add and assign                                           |
| `-=`     | Subtract and assign                                      |
| `*=`     | Multiply and assign                                      |
| `/=`     | Divide and assign                                        |
| `//=`    | Floor divide and assign                                  |
| `**=`    | Exponentiate and assign                                  |
| `%=`     | Modulo and assign                                        |
| `<<=`    | Left shift and assign                                    |
| `>>=`    | Right shift and assign                                   |
| `band=`  | Bitwise AND and assign                                   |
| `bor=`   | Bitwise OR and assign                                    |
| `bxor=`  | Bitwise XOR and assign                                   |
| `x++`    | Post-increment (statement only) — equivalent to `x += 1` |
| `x--`    | Post-decrement (statement only) — equivalent to `x -= 1` |

> **`x++` and `x--` are statements, not expressions.** They cannot be used as the right-hand side of an assignment or inside a larger expression (`let y = x++` is a compile error). They apply only to numeric types (`int`, `float`, and sized variants). Use `x += 1` if you need an expression form.

### Other Operators

| Operator | Description            | Example                     |
| -------- | ---------------------- | --------------------------- |
| `? :`    | Ternary conditional    | `x > 0 ? "yes" : "no"`      |
| `??`     | Null coalescing        | `value ?? "default"`        |
| `?.`     | Optional chaining      | `user?.name`                |
| `?`      | Try operator (postfix) | `result?`                   |
| `is`     | Type check             | `val is int`                |
| `in`     | Membership test        | `2 in [1,2,3]` ? `true`     |
| `not in` | Negated membership     | `5 not in [1,2,3]` ? `true` |
| `as`     | Type cast              | `val as int`                |
| `\|>`    | Pipe                   | `data \|> transform`        |
| `_`      | Pipe placeholder       | `data \|> process(opts, _)` |
| `..`     | Exclusive range        | `0..10`                     |
| `..=`    | Inclusive range        | `0..=10`                    |
| `...`    | Spread                 | `[...list1, ...list2]`      |
| `&`      | Address-of / borrow    | `&x`                        |
| `&mut`   | Mutable borrow         | `&mut x`                    |
| `*`      | Dereference (prefix)   | `*ptr`                      |

### Bitwise

| Operator | Description         | Example                         |
| -------- | ------------------- | ------------------------------- |
| `band`   | Bitwise AND         | `0b1100 band 0b1010` ? `0b1000` |
| `bor`    | Bitwise OR          | `0b1100 bor 0b1010` ? `0b1110`  |
| `bxor`   | Bitwise XOR         | `0b1100 bxor 0b1010` ? `0b0110` |
| `bnot`   | Bitwise NOT (unary) | `bnot 0b1010` ? `...11110101`   |
| `<<`     | Left shift          | `1 << 3` ? `8`                  |
| `>>`     | Right shift         | `16 >> 2` ? `4`                 |

> **Why `band`/`bor`/`bxor`/`bnot` instead of `&`/`|`/`^`/`~`?**
> V2 uses `&` for borrowing/address-of and `|` for union types (`int | str`) and OR patterns in `match`. Using the same symbols for bitwise operations would create ambiguity. Named bitwise keywords (`band`, `bor`, etc.) eliminate that ambiguity entirely. Note: the pipe operator is `|>` (two characters), which is distinct from the single `|` used in types and patterns.

```v2
let flags = 0b0000
let READ  = 0b0001
let WRITE = 0b0010
let EXEC  = 0b0100

flags = flags bor READ bor WRITE    // 0b0011
print(flags band READ != 0)         // true  — has READ
print(flags band EXEC != 0)         // false — no EXEC
flags = flags bxor WRITE            // toggle WRITE off ? 0b0001
```

Compound bitwise assignment:

```v2
x <<= 2    // left shift assign
x >>= 1    // right shift assign
```

### The `as` Cast Operator

`as` performs an explicit type cast at runtime. It is the preferred way to convert between compatible types when the compiler cannot infer the conversion automatically.

```v2
let x: any = 42
let n = x as int          // cast any ? int
let f = x as float        // cast any ? float

let val: int | str = "99"
let s = val as str        // narrow union type
let n2 = int(val as str)  // chain: union ? str ? int

// Numeric widening / narrowing
let big: i64 = 1_000_000
let small = big as i32    // explicit narrowing (may lose data)
let wider = small as f64  // widen int to float

// Upcast to base class / trait object
let dog = new Dog("Rex", "Lab")
let animal = dog as Animal    // upcast — always safe
```

`as` panics at runtime if the value is not compatible with the target type (e.g. `"hello" as int`). Use `is` to check first if the cast might fail:

```v2
if (val is int) {
    let n = val as int    // safe — guarded by is check
}
```

`as` does **not** call constructors or trait methods — it is a direct type reinterpretation. For semantic conversion, use `From`/`Into` or explicit conversion functions like `int()`, `str()`, `float()`.

### The `in` and `not in` Operators

`in` tests membership. `not in` is its negation. Both work on lists, strings, dicts, sets, ranges, and any type implementing `__contains__`. Note: `not` is a keyword used exclusively in the `not in` compound operator — it is not a standalone negation (`!` is used for that).

```v2
// Lists
2 in [1, 2, 3]          // true
5 not in [1, 2, 3]      // true

// Strings (substring check)
"ell" in "hello"         // true
"xyz" not in "hello"     // true

// Dicts (key check)
"name" in {"name": "Alice"}   // true

// Sets
3 in #{1, 2, 3}          // true

// Ranges
5 in 0..10               // true
10 in 0..10              // false (exclusive)
10 in 0..=10             // true  (inclusive)

// In conditions
for (x in [1, 2, 3]) {
    if (x not in bad_values) {
        process(x)
    }
}
```

---

### Pipe Operator (`|>`)

The pipe operator passes the result of the left-hand expression as an argument to the right-hand function. `_` is the **pipe placeholder** — it marks exactly where the piped value is inserted. See the dedicated [Pipe and Spread](#pipe-and-spread) section for full documentation.

```v2
let result = [1, 2, 3, 4, 5]
    |> filter(_, lambda(x) => x % 2 == 0)
    |> sort(_)
```

### F-String Reference

F-strings (formatted string literals) are the primary way to embed expressions inside strings. They are prefixed with `f` and use `${...}` for interpolation.

In embedded language blocks (for example `@py`, `@js`), interpolation syntax follows the embedded language, not V2.

#### Basic Interpolation

Any valid V2 expression can appear inside `${...}`:

```v2
let name = "Alice"
let age  = 30

f"Hello, ${name}!"                    // "Hello, Alice!"
f"Next year you will be ${age + 1}."  // "Next year you will be 31."
f"Type is ${type(name)}."             // "Type is str."
```

#### Method Calls and Property Access

```v2
let items = ["a", "b", "c"]

f"${items.len()} items"               // "3 items"
f"${items.join(', ')}"               // "a, b, c"
f"${name.upper()} — ${name.lower()}" // "ALICE — alice"
```

#### Nested Expressions

Any expression that returns a printable value is valid — conditionals, function calls, arithmetic, even nested f-strings:

```v2
let score = 73

f"Grade: ${score >= 90 ? 'A' : score >= 70 ? 'B' : 'C'}"   // "Grade: B"
f"${is_some(result) ? unwrap(result) : 'none'}"
f"Sum: ${[1, 2, 3].reduce(lambda(a, x) => a + x, 0)}"       // "Sum: 6"
```

#### Format Specifiers

F-strings support an optional format specifier after a `:` inside `${...}`:

```
${expression:[[fill]align][sign][0][width][.precision][type]}
```

```v2
let price = 9.5
let n     = 42
let label = "hi"

f"${price:.2f}"      // "9.50"       — 2 decimal places
f"${n:08d}"          // "00000042"   — zero-padded, width 8
f"${n:>10}"          // "        42" — right-aligned, width 10
f"${label:<10}"      // "hi        " — left-aligned, width 10
f"${label:^10}"      // "    hi    " — center-aligned, width 10
f"${n:+d}"           // "+42"        — always show sign
f"${255:x}"          // "ff"         — hex (lowercase)
f"${255:X}"          // "FF"         — hex (uppercase)
f"${255:b}"          // "11111111"   — binary
f"${1234567:.2e}"    // "1.23e+06"   — scientific notation
```

| Specifier | Meaning                                   |
| --------- | ----------------------------------------- |
| `<`       | Left-align                                |
| `>`       | Right-align                               |
| `^`       | Center-align                              |
| `0`       | Zero-pad                                  |
| `.N`      | Precision (floats) or max width (strings) |
| `d`       | Integer                                   |
| `f`       | Float                                     |
| `e` / `E` | Scientific notation (lower/upper)         |
| `x` / `X` | Hex (lower/upper)                         |
| `b`       | Binary                                    |
| `o`       | Octal                                     |
| `+`       | Always show sign                          |

#### Quotes Inside F-Strings

Use the opposite quote style inside `${...}`, or escape with `\"`:

```v2
f"${dict['key']}"            // single quotes inside double-quoted f-string — OK
f"${func(\"arg\")}"          // escaped double quotes — also OK
```

Triple-quoted f-strings sidestep this entirely, since the outer delimiter is `"""`:

```v2
let key = "name"
f"""Value: ${dict["${key}"]}"""    // double quotes freely usable inside
```

#### Multi-Line F-Strings

Use `f"""..."""` for multi-line formatted strings:

```v2
let user = "Alice"
let score = 97

let report = f"""
    Player:  ${user}
    Score:   ${score}
    Grade:   ${score >= 90 ? "A" : "B"}
"""
```

#### Calling `Printable` Objects

Any object implementing the `Printable` trait (with `to_str()`) is interpolable directly:

```v2
struct Point { x: float, y: float }

impl Printable for Point {
    func to_str(self) { return f"(${self.x}, ${self.y})" }
}

let p = Point { x: 1.0, y: 2.5 }
f"Location: ${p}"    // "Location: (1.0, 2.5)"
```

#### What Is NOT Valid

- Statements (e.g. `let`, `for`) are not valid inside `${...}` — only expressions.
- `${...}` blocks cannot span multiple lines in a single-quoted f-string (use `f"""` instead).

---

### Operator Precedence

> **Reading this table:** Numbers go from **lowest** (1) to **highest** (18) precedence. Operators with a higher number bind more tightly — they are evaluated first. For example, `*` (14) binds tighter than `+` (13), so `2 + 3 * 4` evaluates as `2 + (3 * 4)`. When two operators share the same level, they associate left-to-right unless noted otherwise.

1. `=`, `+=`, `-=`, `*=`, `/=`, `//=`, `**=`, `%=`, `<<=`, `>>=`, `band=`, `bor=`, `bxor=` — Assignment _(lowest)_
2. `|>` — Pipe
3. `? :` — Ternary
4. `??` — Null coalescing
5. `||` — Logical OR
6. `&&` — Logical AND
7. `==`, `!=` — Equality
8. `is`, `in`, `not in` — Type / membership check
9. `<`, `>`, `<=`, `>=` — Comparison
10. `..`, `..=` — Range
11. `band`, `bor`, `bxor` — Bitwise
12. `<<`, `>>` — Shift
13. `+`, `-` — Addition
14. `*`, `/`, `//`, `%` — Multiplication / division
15. `**` — Exponentiation _(right-associative)_
16. `as` — Type cast
17. Unary (`-`, `!`, `bnot`, `&`, `*`) — Prefix
18. Postfix (`()`, `[]`, `.`, `?.`, `?`) — Postfix _(highest)_

> `x++` and `x--` are statement-only update forms, not expression operators, so they are intentionally excluded from precedence evaluation.

---

## Strings

### String Literals

```v2
let plain = "Hello, World!"
let raw = r"No \n escapes here"
let formatted = f"Hello, ${name}! You are ${age} years old."
```

### Multi-Line Strings

Triple-quoted strings preserve newlines and indentation. Leading whitespace up to the indent of the closing `"""` is stripped.

```v2
let sql = """
    SELECT *
    FROM users
    WHERE active = true
    ORDER BY name
"""

let html = """
    <div class="card">
        <h1>Hello</h1>
    </div>
"""
```

Triple-quoted strings also support interpolation with `f"""`:

```v2
let name = "Alice"
let msg = f"""
    Hello, ${name}!
    Your score is ${score * 100}%.
"""
```

Raw triple-quoted strings use `r"""`:

```v2
let regex_pat = r"""
    (\d{4})   # year
    -(\d{2})  # month
    -(\d{2})  # day
"""
```

### Escape Sequences

| Sequence     | Character                         |
| ------------ | --------------------------------- |
| `\n`         | Newline                           |
| `\t`         | Tab                               |
| `\r`         | Carriage return                   |
| `\\`         | Backslash                         |
| `\"`         | Double quote                      |
| `\'`         | Single quote                      |
| `\b`         | Backspace                         |
| `\f`         | Form feed                         |
| `\0`         | Null byte                         |
| `\uXXXX`     | Unicode code point (4 hex digits) |
| `\UXXXXXXXX` | Unicode code point (8 hex digits) |
| `\xHH`       | Byte value (2 hex digits)         |

Examples:

```v2
let s = "line one\nline two"
let tab = "col1\tcol2"
let path = "C:\\Users\\Alice"
let smiley = "\u263A"    // ?
```

### String Interpolation (f-strings)

Use `${expression}` inside f-strings to embed any V2 expression. See the [F-String Reference](#f-string-reference) section for full format specifier syntax, multi-line strings, and advanced usage.

```v2
let name = "Alice"
let greeting = f"Hello, ${name}!"    // "Hello, Alice!"
```

### Raw String Literals

Prefix a string with `r` to disable all escape processing. Every character is taken literally — backslashes are plain characters.

```v2
let pattern = r"\d+\.\d+"       // regex: literally \d+\.\d+
let path    = r"C:\Users\Alice"  // Windows path with no escape issues
let raw     = r"line1\nline2"    // the \n is NOT a newline — it's two chars
```

Raw strings can use single or double quotes and can be multi-line with triple quotes:

```v2
let multiline = r"""
SELECT *
FROM users
WHERE name LIKE '\%alice\%'
"""
```

Raw strings cannot contain the closing quote character unescaped. Use a non-raw string or triple-quote form if the string content requires it.

### String Repetition

Use `*` with a string and an integer to repeat the string:

```v2
"hi" * 3    // "hihihi"
```

### String Indexing

```v2
let s = "Hello"
s[0]          // "H"
s[-1]         // "o" (negative indexing)
```

### String Slicing

Strings support the same slice syntax as lists. Slices return a new string.

```v2
let s = "Hello, World!"

s[0:5]        // "Hello"
s[7:]         // "World!"
s[:5]         // "Hello"
s[-6:]        // "World!"
s[::2]        // "Hlo ol!"  — every 2nd character (step)
s[::-1]       // "!dlroW ,olleH"  — reversed

// Slice syntax: s[start:end:step]
// start defaults to 0, end to len, step to 1
// Negative indices count from the end
```

Ranges also work as slice indexes, and out-of-range bounds clamp instead of
erroring. `*` repeats a string (negative counts give `""`):

```v2
s[1..4]          // "ell"  — same as s[1:4]
s[7..=11]        // "World" — inclusive range
"abcdef"[2..99]  // "cdef" — clamped, never errors
"ab" * 3         // "ababab"
[0] * 4          // [0, 0, 0, 0] — lists repeat the same way
```

String slices are read-only — you cannot assign to a string slice (`s[1:3] = "xy"` is a type error; strings are immutable). To modify, convert to a list of chars and back:

```v2
let chars = list(s)    // ["H", "e", "l", "l", "o", ...]
chars[0] = "h"
let modified = chars.join("")    // "hello, World!"
```

### String Methods

| Method                     | Description                         | Example                                  |
| -------------------------- | ----------------------------------- | ---------------------------------------- |
| `.len()`                   | String length                       | `"hello".len()` ? `5`                    |
| `.upper()`                 | Convert to uppercase                | `"hello".upper()` ? `"HELLO"`            |
| `.lower()`                 | Convert to lowercase                | `"HELLO".lower()` ? `"hello"`            |
| `.trim()`                  | Strip leading & trailing whitespace | `" hi ".trim()` ? `"hi"`                 |
| `.trim_start()`            | Strip leading whitespace only       | `" hi ".trim_start()` ? `"hi "`          |
| `.trim_end()`              | Strip trailing whitespace only      | `" hi ".trim_end()` ? `" hi"`            |
| `.contains(sub)`           | Check substring                     | `"hello".contains("ell")` ? `true`       |
| `.starts_with(pre)`        | Check prefix                        | `"hello".starts_with("he")` ? `true`     |
| `.ends_with(suf)`          | Check suffix                        | `"hello".ends_with("lo")` ? `true`       |
| `.split(sep)`              | Split by delimiter                  | `"a,b,c".split(",")` ? `["a","b","c"]`   |
| `.split(sep, n)`           | Split into at most n parts          | `"a,b,c".split(",", 2)` ? `["a","b,c"]`  |
| `.replace(old, new)`       | Replace all occurrences             | `"hello".replace("l","r")` ? `"herro"`   |
| `.replace_first(old, new)` | Replace only first occurrence       | `"aaa".replace_first("a","b")` ? `"baa"` |
| `.count(sub)`              | Count non-overlapping occurrences   | `"aaa".count("a")` ? `3`                 |
| `.indexOf(needle)`         | First position (-1 if absent)       | `"hello".indexOf("ll")` ? `2`            |
| `.lastIndexOf(needle)`     | Last position (-1 if absent)        | `"hello".lastIndexOf("l")` ? `3`         |
| `.charAt(i)`               | Character at index                  | `"hello".charAt(1)` ? `"e"`              |
| `.substr(s, e)`            | Extract substring                   | `"hello".substr(1,4)` ? `"ell"`          |
| `.slice(s, e)`             | Alias for `.substr(s, e)`           | `"hello".slice(0,3)` ? `"hel"`           |
| `.repeat(n)`               | Repeat string n times               | `"ab".repeat(3)` ? `"ababab"`            |
| `.pad_start(n, ch?)`       | Pad left to width n                 | `"5".pad_start(3, "0")` ? `"005"`        |
| `.pad_end(n, ch?)`         | Pad right to width n                | `"hi".pad_end(5, ".")` ? `"hi..."`       |
| `.reverse()`               | Reversed copy                       | `"hello".reverse()` ? `"olleh"`          |
| `.isalpha()`               | All alphabetic?                     | `"abc".isalpha()` ? `true`               |
| `.isdigit()`               | All digits?                         | `"123".isdigit()` ? `true`               |
| `.isalnum()`               | All alphanumeric?                   | `"abc123".isalnum()` ? `true`            |
| `.isspace()`               | All whitespace?                     | `"  \t".isspace()` ? `true`              |
| `.isupper()`               | All uppercase?                      | `"ABC".isupper()` ? `true`               |
| `.islower()`               | All lowercase?                      | `"abc".islower()` ? `true`               |

### Unicode & Encoding

V2 strings are UTF-8 encoded. All string operations work on **Unicode code points** by default — not raw bytes.

```v2
let s = "h—llo"
s.len()           // 5  — code points, not bytes
s[0]              // "h"
s[1]              // "—"  — one code point, not two bytes
```

For byte-level access (e.g. when working with binary data or network buffers), use the bytes view:

```v2
let bytes = s.to_bytes()       // list of raw byte values
let s2    = str_from_bytes(bytes)    // reconstruct from bytes
s.byte_len()                   // number of UTF-8 bytes (may differ from .len())
```

For grapheme cluster iteration (user-visible characters, including composed emoji and combining marks):

```v2
let emoji = "????????"
emoji.len()              // 5  — code points (family emoji is a ZWJ sequence)
emoji.graphemes()        // ["????????"]  — 1 user-visible character
emoji.grapheme_len()     // 1

let accented = "e\u0301"    // 'e' + combining acute accent
accented.len()              // 2  — two code points
accented.graphemes()        // ["—"]  — one grapheme cluster
```

String comparison uses Unicode code point order by default. For locale-aware collation use `std.fmt`:

```v2
import "std.fmt" as fmt
let sorted = fmt.collate_sort(["ź", "—", "b"], locale: "pl")
```

Available encoding helpers:

| Function                 | Description                                                           |
| ------------------------ | --------------------------------------------------------------------- |
| `s.to_bytes()`           | UTF-8 byte list                                                       |
| `s.byte_len()`           | Byte length of the UTF-8 encoding                                     |
| `s.graphemes()`          | List of grapheme clusters                                             |
| `s.grapheme_len()`       | Number of grapheme clusters                                           |
| `str_from_bytes(bytes)`  | Construct string from byte list (must be valid UTF-8)                 |
| `str_encode(s, enc)`     | Encode to `"utf-8"`, `"utf-16"`, `"latin-1"` etc. — returns byte list |
| `str_decode(bytes, enc)` | Decode byte list with given encoding ? str                            |

### Tagged Template Literals

Tagged template literals let you attach a function to an f-string. The tag function receives the static string parts and the interpolated values separately, giving you full control over how the string is assembled — useful for SQL escaping, HTML sanitization, internationalization, and DSL construction.

```v2
// A tag function receives (parts: list<str>, ...values: list<any>) -> any
func sql(parts, ...values) -> str {
    let result = ""
    for (i, part) in parts.enumerate() {
        result += part
        if (i < values.len()) {
            result += db_escape(values[i])    // safely escape each interpolated value
        }
    }
    return result
}

let name = "O'Reilly"
let query = sql f"SELECT * FROM books WHERE author = ${name}"
// SELECT * FROM books WHERE author = 'O''Reilly'
```

Tag functions can return any type, not just strings:

```v2
func html(parts, ...values) -> HtmlNode {
    // sanitize values, parse HTML fragments, return a DOM tree
    ...
}

let user_input = "<script>alert('xss')</script>"
let node = html f"<div>${user_input}</div>"
// user_input is HTML-escaped automatically by the tag function
```

Built-in tag functions:

| Tag     | Description                                              |
| ------- | -------------------------------------------------------- |
| `raw`   | Returns parts and values without interpolation (inspect) |
| `regex` | Compile interpolated string as a regex at runtime        |
| `css`   | Vendor-prefix and validate CSS (in `std.web`)            |

---

## Lists

### Creating Lists

```v2
let nums = [1, 2, 3, 4, 5]
let mixed = [1, "two", true, null]
let empty = []
let from_range = list(range(5))    // [0, 1, 2, 3, 4]
```

### Indexing

```v2
nums[0]       // 1  (first element)
nums[-1]      // 5  (last element)
nums[1] = 99  // assignment
```

### List Methods

| Method                  | Description                                         | Example                                                      |
| ----------------------- | --------------------------------------------------- | ------------------------------------------------------------ |
| `.len()`                | Length                                              | `[1,2,3].len()` ? `3`                                        |
| `.push(item)`           | Append item                                         | `list.push(4)`                                               |
| `.pop()`                | Remove & return last                                | `list.pop()` ? last item                                     |
| `.pop(i)`               | Remove & return at index                            | `list.pop(0)` ? first item                                   |
| `.insert(i, val)`       | Insert at index                                     | `list.insert(0, "first")`                                    |
| `.remove(val)`          | Remove first occurrence of val                      | `[1,2,3].remove(2)` ? `[1,3]`                                |
| `.clear()`              | Remove all elements                                 | `list.clear()`                                               |
| `.extend(other)`        | Append all items from other list                    | `[1,2].extend([3,4])` ? `[1,2,3,4]`                          |
| `.contains(val)`        | Element exists?                                     | `[1,2,3].contains(2)` ? `true`                               |
| `.index_of(val)`        | First index of val (-1 if absent)                   | `[1,2,3].index_of(2)` ? `1`                                  |
| `.count(val)`           | Count occurrences of val                            | `[1,2,2,3].count(2)` ? `2`                                   |
| `.find(pred)`           | First element matching predicate ? `None` if absent | `[1,2,3].find(lambda(x) => x > 1)` ? `2`                     |
| `.any(pred)`            | Any element matches predicate?                      | `[1,2,3].any(lambda(x) => x > 2)` ? `true`                   |
| `.all(pred)`            | All elements match predicate?                       | `[1,2,3].all(lambda(x) => x > 0)` ? `true`                   |
| `.unique()`             | Deduplicated copy (preserves order)                 | `[1,2,2,3].unique()` ? `[1,2,3]`                             |
| `.flat_map(lambda)`     | Map then flatten one level                          | `[[1,2],[3]].flat_map(lambda(x) => x)` ? `[1,2,3]`           |
| `.sort()`               | Sort in-place                                       | `[3,1,2].sort()` ? `[1,2,3]`                                 |
| `.sort_by(key_fn)`      | Sort in-place by key                                | `items.sort_by(lambda(x) => x["age"])`                       |
| `.reverse()`            | Reversed copy                                       | `[1,2,3].reverse()` ? `[3,2,1]`                              |
| `.join(sep)`            | Join to string                                      | `["a","b"].join(",")` ? `"a,b"`                              |
| `.map(lambda)`          | Transform elements                                  | `[1,2,3].map(lambda(x) => x*2)` ? `[2,4,6]`                  |
| `.filter(lambda)`       | Filter elements                                     | `[1,2,3,4].filter(lambda(x) => x>2)` ? `[3,4]`               |
| `.reduce(lambda, init)` | Reduce to value                                     | `[1,2,3].reduce(lambda(a,x) => a+x, 0)` ? `6`                |
| `.for_each(lambda)`     | Iterate (side effects)                              | `list.for_each(lambda(x) => print(x))`                       |
| `.slice(s, e)`          | Extract sub-list                                    | `[1,2,3,4].slice(1,3)` ? `[2,3]`                             |
| `.flatten()`            | Flatten one level of nesting                        | `[[1,2],[3,[4]]].flatten()` ? `[1,2,3,[4]]`                  |
| `.chunk(n)`             | Non-overlapping chunks of size n                    | `[1,2,3,4,5].chunk(2)` ? `[[1,2],[3,4],[5]]`                 |
| `.windows(n)`           | Sliding windows of size n                           | `[1,2,3].windows(2)` ? `[[1,2],[2,3]]`                       |
| `.partition(pred)`      | Split into two lists: `[matching, non_matching]`    | `[1,2,3,4].partition(lambda(x) => x%2==0)` ? `[[2,4],[1,3]]` |
| `.group_by(fn)`         | Group elements by key fn ? dict of lists            | `items.group_by(lambda(x) => x["dept"])`                     |
| `.zip(other)`           | Pair elements with another list ? list of tuples    | `[1,2].zip([3,4])` ? `[(1,3),(2,4)]`                         |
| `.take(n)`              | First n elements                                    | `[1,2,3,4].take(2)` ? `[1,2]`                                |
| `.drop(n)`              | Skip first n elements                               | `[1,2,3,4].drop(2)` ? `[3,4]`                                |
| `.sum()`                | Sum all elements                                    | `[1,2,3].sum()` ? `6`                                        |
| `.product()`            | Product of all elements                             | `[1,2,3,4].product()` ? `24`                                 |

### Slicing

```v2
let nums = [1, 2, 3, 4, 5]

// Read slices
nums[1:3]      // [2, 3]
nums[:2]       // [1, 2]
nums[3:]       // [4, 5]

// Slice assignment
nums[1:3] = [20, 30]
print(nums)    // [1, 20, 30, 4, 5]

// Replace with different-length slice
nums[1:3] = [99]
print(nums)    // [1, 99, 4, 5]

// Delete a slice
nums[1:2] = []
print(nums)    // [1, 4, 5]
```

---

## Dictionaries

### Creating Dicts

```v2
let person = {
    "name": "Alice",
    "age": 30,
    "active": true
}

let empty = {}
```

### Access

```v2
person["name"]            // "Alice"
person["city"] = "NYC"    // add/update key
```

### Dict Methods

| Method               | Description                                    | Example                                       |
| -------------------- | ---------------------------------------------- | --------------------------------------------- |
| `.keys()`            | List of keys                                   | `d.keys()` ? `["name", "age"]`                |
| `.values()`          | List of values                                 | `d.values()` ? `["Alice", 30]`                |
| `.items()`           | List of `[key, val]` pairs                     | `d.items()` ? `[["name","Alice"],["age",30]]` |
| `.has(key)`          | Key exists?                                    | `d.has("name")` ? `true`                      |
| `.get(key, default)` | Get with fallback                              | `d.get("city", "?")` ? `"?"`                  |
| `.set(key, val)`     | Set key-value                                  | `d.set("city", "NYC")`                        |
| `.remove(key)`       | Remove key                                     | `d.remove("age")`                             |
| `.len()`             | Number of entries                              | `d.len()` ? `2`                               |
| `.update(other)`     | Merge another dict in-place (other's keys win) | `d.update({"city": "NYC"})`                   |
| `.merge(other)`      | Return a new merged dict (other's keys win)    | `d.merge({"port": 443})`                      |
| `.clear()`           | Remove all entries                             | `d.clear()`                                   |

Aliases: `.has_key()` ? `.has()`, `.contains_key()` ? `.has()`, `.delete()` ? `.remove()`.

---

## Tuples

Immutable, ordered collections with dot-index access.

```v2
let point = (10, 20)
let record = ("Alice", 30, true)

point.0       // 10
point.1       // 20
record.2      // true

let () = ()   // empty tuple
```

Tuples cannot be modified after creation.

### Tuple Destructuring

Tuples can be unpacked directly in `let` bindings and `for-in` loops:

```v2
let point = (10, 20)
let (x, y) = point
print(x, y)    // 10  20

// In a for-in loop (e.g. from iter.zip)
let names = ["Alice", "Bob"]
let scores = [95, 87]
for ((name, score) in iter.zip(names, scores)) {
    print(f"${name}: ${score}")
}

// Ignoring elements with _
let record = ("Alice", 30, true)
let (name, _, active) = record
```

---

## Sets

Unordered collections of unique values.

```v2
let s = #{1, 2, 3}
let empty = #{}
let from_list = set([1, 2, 2, 3])   // #{1, 2, 3}
```

`set(list)` is the canonical constructor. `to_set(list)` remains available as a legacy alias.

Set literals deduplicate (`#{1, 1, 2}` is `#{1, 2}`), and set equality is
membership-based — `#{1, 2} == #{2, 1}` is `true`.

### Set Methods

| Method                   | Description                        | Example                                    |
| ------------------------ | ---------------------------------- | ------------------------------------------ |
| `.add(val)`              | Add a value                        | `s.add(4)` ? `#{1,2,3,4}`                  |
| `.remove(val)`           | Remove a value (no-op if absent)   | `s.remove(2)` ? `#{1,3}`                   |
| `.contains(val)`         | Check membership                   | `s.contains(2)` ? `true`                   |
| `.len()`                 | Number of elements                 | `s.len()` ? `3`                            |
| `.union(other)`          | All elements from both sets        | `#{1,2}.union(#{2,3})` ? `#{1,2,3}`        |
| `.intersect(other)`      | Elements present in both sets      | `#{1,2,3}.intersect(#{2,3,4})` ? `#{2,3}`  |
| `.difference(other)`     | Elements in self but not in other  | `#{1,2,3}.difference(#{2,3})` ? `#{1}`     |
| `.sym_difference(other)` | Elements in either but not both    | `#{1,2}.sym_difference(#{2,3})` ? `#{1,3}` |
| `.is_subset(other)`      | Is every element of self in other? | `#{1,2}.is_subset(#{1,2,3})` ? `true`      |
| `.is_superset(other)`    | Is every element of other in self? | `#{1,2,3}.is_superset(#{1,2})` ? `true`    |
| `.is_disjoint(other)`    | Do the sets share no elements?     | `#{1,2}.is_disjoint(#{3,4})` ? `true`      |
| `.to_list()`             | Convert to list (unordered)        | `#{1,2,3}.to_list()` ? `[1,2,3]`           |
| `.clear()`               | Remove all elements                | `s.clear()`                                |

### Set Operators

V2 special-cases `|`, `&`, `-`, and `^` on the `set` type. When the left-hand operand is a `set`, the compiler routes these operators to the set's built-in overloads instead of the pipe, borrow, or arithmetic interpretations. This does **not** conflict with `band`/`bor`/`bxor` (which operate on integers) or `|>` (pipe, which requires `|>` not bare `|`).

```v2
let a = #{1, 2, 3}
let b = #{2, 3, 4}

a | b    // union      ? #{1,2,3,4}   (set overload of |)
a & b    // intersect  ? #{2,3}       (set overload of &)
a - b    // difference ? #{1}
a ^ b    // sym diff   ? #{1,4}

// Membership
2 in a    // true
5 in a    // false
```

> **Why not just use methods?** Set operator syntax (`a | b`) is idiomatic and widely expected. The method equivalents (`.union()`, `.intersect()`, `.difference()`, `.sym_difference()`) are equally valid and preferred when the set context is not obvious from surrounding code.

### Example

```v2
let visited = #{}
let frontier = #{"https://example.com"}

while (frontier.len() > 0) {
    let url = frontier.to_list()[0]
    frontier.remove(url)
    if (visited.contains(url)) { continue }
    visited.add(url)
    // ... crawl url and add links to frontier
}
```

---

## Comprehensions

Comprehensions provide concise syntax for building lists, dicts, and sets from iteration with optional filtering and transformation — inspired by Python and Haskell.

### List Comprehensions

```v2
let squares = [x * x for x in 0..10]
// [0, 1, 4, 9, 16, 25, 36, 49, 64, 81]

let evens = [x for x in 0..20 if x % 2 == 0]
// [0, 2, 4, 6, 8, 10, 12, 14, 16, 18]
```

### Dict Comprehensions

```v2
let names = ["alice", "bob", "carol"]
let lookup = {name: name.upper() for name in names}
// {"alice": "ALICE", "bob": "BOB", "carol": "CAROL"}

let filtered = {k: v for (k, v) in scores if v >= 50}
```

### Set Comprehensions

```v2
let unique_lengths = #{word.len() for word in words}
```

### Nested Comprehensions

```v2
let pairs = [(x, y) for x in 0..3 for y in 0..3 if x != y]
// [(0,1), (0,2), (1,0), (1,2), (2,0), (2,1)]
```

### With Pattern Destructuring

```v2
let firsts = [name for (name, _score) in students if _score > 90]
```

### Async Comprehensions

Comprehensions also work with `for await` for async iterables:

```v2
let results = [await process(item) for await item in async_stream]
```

Comprehensions are syntactic sugar over `map` / `filter` chains. The compiler desugars them into equivalent iterator pipelines, so they receive the same optimizations.

---

## Control Flow

### If / Elif / Else

```v2
if (x > 0) {
    print("positive")
} elif (x < 0) {
    print("negative")
} else {
    print("zero")
}
```

Parentheses around the condition are optional — `if x > 0 { ... }` and
`if (x > 0) { ... }` are both valid. The same applies to `while`, `elif`, and
`for ... in` heads (C-style `for (init; cond; step)` keeps its parentheses):

```v2
if x > 0 { print("positive") }
while queue.len() > 0 { process(queue.pop()) }
for item in items { print(item) }
for k, v in scores { print(f"${k}: ${v}") }
```

### Ternary Operator

```v2
let result = x > 0 ? "positive" : "non-positive"
```

### While Loop

```v2
let i = 0
while (i < 10) {
    print(i)
    i += 1
}
```

### For Loop (C-style)

```v2
for (let i = 0; i < 10; i++) {
    print(i)
}
```

### For-In Loop

```v2
for (item in [1, 2, 3]) {
    print(item)
}

// Dict iteration: for-in yields KEYS (insertion order)
for (key in dict) {
    print(key, dict[key])
}

// To iterate key-value pairs together, use items():
for ([key, val] in dict.items()) {
    print(key, "?", val)
}

// To iterate only values:
for (val in dict.values()) {
    print(val)
}

for (ch in "hello") {
    print(ch)
}
```

> **Dict iteration order:** V2 dicts preserve insertion order. `for (k in d)` yields keys in the order they were first inserted.

### Range Iteration

```v2
for (i in 0..10) {       // 0 to 9
    print(i)
}

for (i in 0..=10) {      // 0 to 10 (inclusive)
    print(i)
}

for (i in range(0, 20, 2)) {   // 0, 2, 4, ..., 18
    print(i)
}
```

Ranges are lazy — they don't allocate a list. Call `.collect()` to materialise them:

```v2
let nums = (0..10).collect()      // [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
let evens = range(0, 20, 2).collect()   // [0, 2, 4, ..., 18]
```

### Break & Continue

```v2
while (true) {
    if (done) { break }
    if (skip) { continue }
    process()
}
```

### Labeled Break & Continue

When loops are nested, use a label on the outer loop to `break` or `continue` it directly from an inner loop:

```v2
outer: for (i in 0..5) {
    for (j in 0..5) {
        if (j == 2) { continue outer }   // skip to next i
        if (i == 3) { break outer }      // exit both loops entirely
        print(i, j)
    }
}
```

Labels are placed immediately before the loop keyword with a `:` suffix. Only `for` and `while` loops can be labelled.

### Do-While Loop

V2 does not have a dedicated `do-while` keyword. The idiomatic equivalent is a `while (true)` loop with a `break` at the bottom:

```v2
// do-while equivalent — body always runs at least once
while (true) {
    process()
    if (!should_continue()) { break }
}
```

---

## Functions

### Function Declaration

```v2
func greet(name) {
    print(f"Hello, ${name}!")
}

func add(a, b) {
    return a + b
}
```

### Default Parameters

```v2
func greet(name, greeting = "Hello") {
    print(f"${greeting}, ${name}!")
}

greet("Alice")            // Hello, Alice!
greet("Alice", "Hi")      // Hi, Alice!
```

### Return Values

```v2
func square(x) {
    return x * x
}

let result = square(5)    // 25
```

Functions without an explicit `return` return `null`.

### Multiple Return Values

```v2
func min_max(list) {
    return [min(list), max(list)]
}

let result = min_max([3, 1, 4, 1, 5])
print(result[0], result[1])    // 1 5
```

Use list destructuring to unpack the return value directly into named variables — this is the idiomatic style:

```v2
let [lo, hi] = min_max([3, 1, 4, 1, 5])
print(lo, hi)    // 1 5
```

### Named Arguments

When calling a function, arguments can be passed by name. This allows reordering and skipping parameters that have defaults.

```v2
func connect(host, port = 8080, timeout = 30, tls = false) {
    // ...
}

// Named call — any order, any subset
connect(host: "localhost")
connect(host: "example.com", tls: true, port: 443)
connect(port: 9000, host: "dev.local", timeout: 10)
```

- Named and positional arguments can be mixed: positional args must come first.
- Passing an unknown name is a compile-time error.

```v2
connect("localhost", tls: true)    // positional host, named tls — OK
```

### Variadic Functions

Use `...args` as the last parameter to accept any number of additional arguments. They are collected into a list.

```v2
func sum(...args) {
    return args.reduce(lambda(a, x) => a + x, 0)
}

sum(1, 2, 3)        // 6
sum(10, 20)         // 30
sum()               // 0

func log(level, ...messages) {
    for (msg in messages) {
        print(f"[${level}] ${msg}")
    }
}

log("INFO", "Server started", "Listening on port 8080")
```

Spread into a variadic call with `...`:

```v2
let nums = [1, 2, 3, 4]
sum(...nums)    // 10
```

### Higher-Order Functions

Functions are first-class values — they can be passed as arguments, returned from other functions, and stored in variables.

```v2
func apply(value, f) {
    return f(value)
}

let result = apply(5, lambda(x) => x * x)    // 25

// Returning a function
func multiplier(factor) {
    return lambda(x) => x * factor
}

let double = multiplier(2)
let triple = multiplier(3)
print(double(5))    // 10
print(triple(5))    // 15
```

### Recursion

```v2
func factorial(n) {
    if (n <= 1) { return 1 }
    return n * factorial(n - 1)
}
```

### Effect Annotations

Functions can be annotated with their side-effect categories using `[effects: ...]`. This is optional metadata used by the compiler, tooling, and documentation — it does not change runtime behavior.

```v2
func fetch(url) [effects: net] {
    return http_get(url)
}

func pure_add(a, b) [effects: none] {
    return a + b
}

pure func square(x) {
    return x * x
}

func log_write(msg) [effects: io] {
    write_file("app.log", msg)
}
```

`pure` is shorthand for `[effects: none]`.

Built-in effect tags: `none`, `io`, `net`, `env`, `rand`, `time`, `state`, `unsafe` (same set as [Built-in Effect Labels](#effects-system)). You may define additional project-specific custom tags.

Effect annotations can be queried at compile time (see [Compile-Time Execution](#compile-time-execution)).

### Local Labels & Goto

Within a function body, you can define named jump targets using `label` and jump to them with `goto`. This is intentionally scoped — `goto` cannot cross function boundaries.

```v2
func state_machine(input) {
    let state = 0

    label start:
        if (state == 0) {
            state = process_a(input)
            goto check
        }

    label check:
        if (state < 0) { goto error }
        if (state > 10) { goto done }
        state += 1
        goto start

    label done:
        return Ok(state)

    label error:
        return Err("invalid state")
}
```

`label` and `goto` are designed for generated code, state machines, and low-level loops where structured control flow would require excessive nesting. Prefer structured control flow (`if`/`while`/`match`) for ordinary code.

**Restrictions:**

| Rule                     | Detail                                                                             |
| ------------------------ | ---------------------------------------------------------------------------------- |
| Function scope only      | `goto` cannot cross function boundaries                                            |
| Forward jumps allowed    | `goto` may jump forward (past code not yet executed)                               |
| No skipping declarations | `goto` cannot jump _into_ a scope past a `let` binding — the compiler rejects this |
| No skipping `defer`      | A `goto` that would bypass a `defer` block's registration point is a compile error |
| Labels are not values    | You cannot store a label in a variable or pass it as an argument                   |

```v2
// ERROR — jumps past a let binding
func bad() {
    goto end
    let x = 10    // skipped — compile error
    label end:
    print("done")
}

// OK — jumping forward over non-declaration statements is fine
func ok() {
    goto skip
    print("skipped")    // not a declaration — allowed
    label skip:
    print("here")
}
```

### Runtime Function Patching

`patch(target, replacement)` swaps the body of one function for another's at runtime. After patching, all calls to `target` execute `replacement`'s body instead.

```v2
func greet() {
    print("Hello!")
}

func greet_loud() {
    print("HELLO!!!")
}

greet()              // Hello!
patch(greet, greet_loud)
greet()              // HELLO!!!
```

This is useful for hot-swapping behavior, mock injection in tests, and self-modifying program logic.

- `patch` takes function references (names), not call expressions.
- The swap is permanent for the duration of the program run unless patched again.
- Patching a function that is currently executing raises `PatchInProgressError`.

### Function Renaming

`rename(old_name, new_name)` changes the name under which a function is callable. After renaming, the original name is freed — it no longer refers to anything — and the function responds only to its new name.

```v2
func greet(name) {
    print(f"Hello, ${name}!")
}

greet("Alice")               // Hello, Alice!

rename(greet, welcome)       // greet is now called welcome

welcome("Bob")               // Hello, Bob!
// greet("Bob")              // ERROR: `greet` is not defined
```

Renaming works on any function — user-defined, imported, or built-in:

```v2
// Rename a builtin
rename(print, say)

say("hello")          // hello
// print("hello")     // ERROR: `print` is not defined
```

```v2
// Rename an imported function
import "std.math"

rename(math.sqrt, root)

root(16.0)              // 4.0
// math.sqrt(16.0)      // ERROR: `sqrt` is not defined in `math`
```

**Rules:**

| Rule                       | Detail                                                                                |
| -------------------------- | ------------------------------------------------------------------------------------- |
| Frees the old name         | After `rename(a, b)`, the name `a` is available for a new function                    |
| New name must be available | If `new_name` is already bound to a function, `rename` raises `NameConflictError`     |
| Applies to current scope   | Renaming a module function renames it within the importing file only                  |
| Not reversible by default  | To swap names, rename both explicitly: `rename(a, tmp); rename(b, a); rename(tmp, b)` |

#### Practical Use: Wrapping a Builtin

A common pattern is to rename a builtin out of the way, define your own version, and delegate to the original:

```v2
rename(print, _original_print)

func print(...args) {
    let timestamp = time_now()
    _original_print(f"[${timestamp}] ", ...args)
}

print("server started")    // [2026-04-16T12:00:00Z] server started
```

### Function Name Uniqueness

V2 enforces **unique function names** within a scope. You cannot define or import a function whose name collides with an existing function in the same scope.

```v2
func process(data) {
    return data + 1
}

func process(data) {       // ERROR: function `process` is already defined in this scope
    return data * 2
}
```

This also applies to imports:

```v2
func sqrt(x) {
    return x ** 0.5
}

import "std.math" as _     // ERROR: importing `sqrt` would shadow existing function `sqrt`
```

To resolve a collision, **rename the existing function first**, then import or define the new one:

```v2
func sqrt(x) {
    return x ** 0.5
}

rename(sqrt, my_sqrt)      // frees the name `sqrt`

import "std.math" as _     // OK — `sqrt` is now available for std.math's version

my_sqrt(9.0)               // 3.0 — your original
sqrt(9.0)                  // 3.0 — std.math version
```

The same rule applies to selective imports:

```v2
import { sqrt } from "std.math"    // ERROR if `sqrt` already exists
```

**Builtins are pre-defined names.** To replace a builtin with your own implementation, rename it first:

```v2
rename(print, builtin_print)

func print(msg) {
    builtin_print(f"[LOG] ${msg}")
}

print("hello")    // [LOG] hello
```

> **Why not allow shadowing?** Implicit shadowing of functions leads to subtle bugs — calling `sqrt()` after an import and getting a completely different function with no warning. V2 makes this explicit: if you want to replace a name, you must `rename` first. The compiler always tells you exactly which name collided.

---

## Defer

A `defer` block schedules code to run automatically when the enclosing function exits — whether it returns normally, early-returns, or throws. It is the cleanest way to handle cleanup without duplicating code across every exit path.

### Basic Usage

```v2
func open_file(path) {
    let f = file_open(path)
    defer {
        file_close(f)    // always runs when open_file returns
    }

    let data = file_read(f)
    return data
}
```

### Multiple Defer Blocks

Multiple `defer` blocks in the same function run in **reverse order** (last declared, first executed):

```v2
func example() {
    defer { print("third") }
    defer { print("second") }
    defer { print("first") }
    print("body")
}

example()
// body
// first
// second
// third
```

### Defer with Early Return

`defer` runs even if the function returns early:

```v2
func process(data) {
    let conn = db_connect("sqlite://app.db")
    defer {
        db_close(conn)    // runs even if we return early below
    }

    if (data == null) { return Err("no data") }

    db_exec(conn, "INSERT INTO log VALUES (?)", [data])
    return Ok(null)
}
```

### Notes

- `defer` blocks run when control leaves the function but before the function fully exits.
- When returning a local binding (for example `return out`), `defer` can still mutate that binding before the value is handed to the caller.
- `defer` is not a loop construct — it fires once per function call, not once per iteration.
- Variables captured inside a `defer` block reflect their values at the time `defer` executes (on exit), not at the time `defer` was declared.

---

## Lambdas & Closures

### Arrow Syntax

```v2
let square = lambda(x) => x * x
let add = lambda(a, b) => a + b
```

### Block Body

```v2
let process = lambda(x) {
    let result = x * 2 + 1
    return result
}
```

### Closures

Lambdas capture variables from their enclosing scope **by reference** — they hold a live reference to the original variable, not a copy. This means mutations to the captured variable are visible inside the closure, and mutations inside the closure are visible outside.

```v2
func make_counter() {
    let count = 0
    return lambda() {
        count += 1
        return count
    }
}

let counter = make_counter()
counter()    // 1
counter()    // 2
counter()    // 3
```

**Reference capture — mutation is shared:**

```v2
let x = 10
let f = lambda() => x * 2

x = 99
f()    // 198 — sees the updated x, not the x at definition time
```

**Forcing capture by value** — use a default parameter with the value you want frozen:

```v2
let x = 10
let f = lambda(x = x) => x * 2    // x is copied at definition time

x = 99
f()    // 20 — uses the captured copy (10), not the updated x
```

This pattern works for any variable: bind it as a default parameter with the same name and its current value is frozen at lambda creation time.

**Capture in loops** — a common gotcha: all loop iterations share the same `i` reference unless you freeze it.

```v2
// Wrong — all closures see the final i (5)
let fns = []
for (i in 0..5) {
    fns.push(lambda() => i)
}
fns[0]()    // 5 — not 0!

// Correct — freeze i per iteration using default param
let fns = []
for (i in 0..5) {
    fns.push(lambda(i = i) => i)
}
fns[0]()    // 0
fns[3]()    // 3
```

### Async Lambdas

Lambdas can be `async`. An async lambda returns a `Promise` and can use `await` inside its body.

```v2
let fetch_all = async lambda(urls) {
    let results = await Promise.all(urls.map(async lambda(url) {
        return await http_get(url)
    }))
    return results
}

let pages = await fetch_all(["https://a.com", "https://b.com"])
```

Async lambdas are first-class — they can be passed to higher-order functions, stored in variables, and returned from other functions exactly like regular lambdas.

```v2
// Async lambda as a callback
let handler = async lambda(req) {
    let data = await db_query(conn, "SELECT * FROM users")
    return http_response(200, json_stringify(data))
}

// http_serve is a convenience builtin — equivalent to std.http's http.serve()
// For production use, prefer: import "std.http" and call http.serve(8080, handler)
http_serve(8080, handler)
```

### With Higher-Order Functions

```v2
let nums = [1, 2, 3, 4, 5]

let doubled = nums.map(lambda(x) => x * 2)           // [2, 4, 6, 8, 10]
let evens = nums.filter(lambda(x) => x % 2 == 0)     // [2, 4]
let total = nums.reduce(lambda(a, x) => a + x, 0)    // 15
```

---

## Decorators

Decorators wrap a function declaratively using `@name` syntax placed above the function definition. A decorator is any function that takes a function and returns a function.

### Basic Usage

```v2
func memoize(f) {
    let cache = {}
    return lambda(...args) {
        let key = str(args)
        if (cache.has(key)) { return cache[key] }
        let result = f(...args)
        cache[key] = result
        return result
    }
}

@memoize
func fib(n) {
    if (n <= 1) { return n }
    return fib(n - 1) + fib(n - 2)
}

fib(40)    // fast — results are cached
```

### Multiple Decorators

Decorators are applied bottom-up (innermost first):

```v2
@log_calls
@memoize
func expensive(x) {
    return x * x
}
// equivalent to: log_calls(memoize(expensive))
```

### Decorators with Arguments

Return a decorator from a function to support arguments:

```v2
func retry(times) {
    return lambda(f) {
        return lambda(...args) {
            for (i in 0..times) {
                try { return f(...args) } catch (e) {
                    if (i == times - 1) { throw e }
                }
            }
        }
    }
}

@retry(3)
func unstable_request(url) {
    return http_get(url)
}
```

### Built-in Decorators

V2 provides a small set of built-in decorators available without import:

| Decorator                 | Effect                                                                                                                                                                                                                                                               |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `@memo`                   | Memoize the function (alias for `memo()`)                                                                                                                                                                                                                            |
| `@deprecated(msg?)`       | Emit a warning when the function is called                                                                                                                                                                                                                           |
| `@inline`                 | Hint to inline the function at call sites                                                                                                                                                                                                                            |
| `@comptime`               | Declare a comptime function (same as `comptime func`)                                                                                                                                                                                                                |
| `@gpu_kernel`             | Compile the function as a GPU kernel (requires `import std.gpu`). The function must accept only `gpu.Buffer<T>` parameters; `gpu.thread_id()` is available inside the body to identify the current thread index. See [std.gpu](#stdgpu--gpu-compute) for full usage. |
| `@wasm_export`            | Export a `pub` function to JavaScript/host when compiling with `--target wasm`. See [WASM Target](#wasm-target).                                                                                                                                                     |
| `@wasm_import(mod, name)` | Bind an imported host function from a WASM module namespace when compiling with `--target wasm`.                                                                                                                                                                     |
| `@wasm_host_import(name)` | Bind a capability-gated host bridge function (`extern wasm_host`) for WASM builds using `--wasm-cap`.                                                                                                                                                                |

> **No `@noreturn` decorator exists.** For functions that never return (they always throw, loop forever, or call `exit()`), declare the return type as `-> never`. See [The `never` Type](#the-never-type) for details and examples.

#### `@memo` — Automatic Memoization

```v2
@memo
func fib(n) {
    if (n <= 1) { return n }
    return fib(n - 1) + fib(n - 2)
}

fib(40)    // fast — results cached automatically
```

`@memo` uses the function's arguments (as a string key) to cache return values. It is equivalent to wrapping the function with `memo(fib)`.

#### `@deprecated` — Deprecation Warnings

```v2
@deprecated("Use connect_v2() instead")
func connect(host, port) {
    // ...
}

connect("localhost", 8080)
// Warning: connect is deprecated — Use connect_v2() instead
```

Calling a deprecated function emits a `deprecated` warning at the call site. Pass `--warn deprecated` to make it an error.

#### `@inline` — Inline Hint

```v2
@inline
func square(x: int) -> int {
    return x * x
}
```

`@inline` is a hint to the compiler to expand the function body at each call site instead of generating a function call. It can improve performance for tiny, frequently-called functions. The compiler may ignore the hint if inlining would increase code size significantly.

#### `@comptime` — Compile-Time Function

```v2
@comptime
func assert_positive<T>() {
    if (ct_platform() == "wasm32") {
        ct_warn("positive assertion may be slower on wasm32")
    }
}
```

`@comptime` on a function declaration is identical to `comptime func`. The function can only be called from `comptime` blocks or other `comptime` functions.

---

## Lazy Expressions

A lazy expression defers evaluation — instead of capturing the _value_ of a variable at assignment time, it captures a _live reference_ to the expression and re-evaluates it every time it is read.

### Syntax

```v2
let b = lazy a
```

`b` now always reflects the current value of `a`, no matter when you read `b`.

### Basic Example

```v2
let a = 7
let b = lazy a

a = 6
print(b)    // 6  — b re-evaluates a at read time, not at assignment time
```

### Lazy Expressions with Expressions

Any expression can be made lazy, not just variable references:

```v2
let x = 10
let y = 5
let sum = lazy x + y

x = 20
print(sum)    // 25  — re-evaluated with new x
```

### Lazy in Structs

```v2
struct Circle {
    radius: float,
    area: any
}

let c = Circle { radius: 3.0, area: null }
c.area = lazy 3.14159 * c.radius * c.radius

c.radius = 5.0
print(c.area)    // 78.53975 — computed from the current radius
```

### Notes

- `lazy` is evaluated every time the binding is _read_, not when it is declared.
- Lazy expressions do not cache — each read re-runs the expression.
- Assigning a new value to a lazy binding replaces the lazy expression entirely: `b = 99` makes `b` a normal value again.
- `is_lazy(val)` returns `true` if a variable holds a lazy expression rather than a concrete value.

---

## Classes

### Basic Class

```v2
class Person {
    func constructor(name, age) {
        self.name = name
        self.age = age
    }

    func greet() {
        print(f"Hello, I'm ${self.name}")
    }

    func birthday() {
        self.age += 1
    }
}

let alice = new Person("Alice", 30)
alice.greet()        // Hello, I'm Alice
alice.birthday()
print(alice.age)     // 31
```

### Inheritance

```v2
class Animal {
    func constructor(name) {
        self.name = name
    }

    func speak() {
        print("...")
    }
}

class Dog extends Animal {
    func constructor(name, breed) {
        super(name)
        self.breed = breed
    }

    func speak() {
        print(f"${self.name} says: Woof!")
    }
}

let dog = new Dog("Rex", "Labrador")
dog.speak()    // Rex says: Woof!
```

### Operator Overloading (Class Example)

```v2
class Vector {
    func constructor(x, y) {
        self.x = x
        self.y = y
    }

    func __add__(other) {
        return new Vector(self.x + other.x, self.y + other.y)
    }

    func __str__() {
        return f"Vector(${self.x}, ${self.y})"
    }
}

let v1 = new Vector(1, 2)
let v2 = new Vector(3, 4)
let v3 = v1 + v2            // Vector(4, 6)
print(str(v3))              // Vector(4, 6)
```

See [Operator Overloading](#operator-overloading) for the full list of overloadable operators.

### `impl` for Classes

Classes can have external `impl` blocks, identical to structs. This lets you split method definitions across files or implement traits on a class without modifying the class body.

```v2
class Shape {
    func constructor(color) {
        self.color = color
    }
}

// Defined elsewhere — e.g. in a separate file
impl Shape {
    func describe(self) {
        print(f"A ${self.color} shape")
    }
}

// Implement a trait on a class
impl Printable for Shape {
    func to_str(self) {
        return f"Shape(${self.color})"
    }
}

let s = new Shape("red")
s.describe()    // A red shape
s.display()     // Shape(red)
```

### Access Modifiers

Class members can be explicitly scoped:

```v2
class BankAccount {
    pub func constructor(balance) {
        self._balance = balance    // convention: _ prefix = private
    }

    pub func deposit(amount) {
        self._validate(amount)
        self._balance += amount
    }

    pub func balance() {
        return self._balance
    }

    private func _validate(amount) {
        if (amount <= 0) { throw new ValueError("Amount must be positive") }
    }
}
```

| Modifier   | Accessible from                               |
| ---------- | --------------------------------------------- |
| `pub`      | Everywhere                                    |
| `private`  | Inside the class only                         |
| `internal` | Inside the current module and its sub-modules |

### Fixed-Field Classes (`@fixed`)

The `@fixed` decorator locks a class's field set at definition time. Once applied, no code — including the class's own methods or external code — can add new fields to instances dynamically. Any attempt to assign to an undeclared field is a **compile-time error**.

**How fields are "declared" in `@fixed` classes:** The compiler performs a static pass over the constructor body and records every `self.field = ...` assignment at the top level of the constructor as a declared field. Only those fields — and fields inherited from a parent `@fixed` class — are part of the fixed schema. Assignments to `self.field` in any other method that were not declared in the constructor are compile errors, not new declarations.

```v2
@fixed
class Config {
    func constructor(host, port) {
        self.host = host    // declared — part of the fixed schema
        self.port = port    // declared — part of the fixed schema
    }

    func set_host(host) {
        self.host = "example.com"    // OK — declared field, just reassigning
        self.timeout = 30            // ERROR — not declared in constructor
    }
}
```

```v2
let cfg = new Config("localhost", 8080)
cfg.host = "example.com"    // OK — declared field
cfg.timeout = 30            // ERROR — Config is @fixed, no such field
```

**Why use `@fixed`?**

| Reason      | Detail                                                                           |
| ----------- | -------------------------------------------------------------------------------- |
| Safety      | Typos in field names become compile errors instead of silent `null` reads        |
| Performance | The compiler can use a fixed-layout object representation (no hash-map overhead) |
| Clarity     | The class definition is the single source of truth for its shape                 |

`@fixed` classes still support inheritance. A subclass may declare additional fields, but those fields are also fixed once the subclass definition is complete:

```v2
@fixed
class Point {
    func constructor(x, y) {
        self.x = x
        self.y = y
    }
}

@fixed
class Point3D extends Point {
    func constructor(x, y, z) {
        super(x, y)
        self.z = z    // OK — declared in this subclass
    }
}
```

> **Tip:** If you want both a fixed shape _and_ value semantics, prefer a `struct`. `@fixed` is most useful for classes that need inheritance or method dispatch alongside shape enforcement.

### Data Classes (`@data`)

The `@data` decorator auto-generates boilerplate methods that every "plain data holder" class needs. Inspired by Kotlin `data class`, Java `record`, and Python `@dataclass`.

```v2
@data
class User {
    func constructor(name: str, age: int, email: str) {
        self.name = name
        self.age = age
        self.email = email
    }
}
```

`@data` generates the following for free, based on the fields declared in the constructor:

| Generated Method    | Behavior                                                           |
| ------------------- | ------------------------------------------------------------------ |
| `equals(other)`     | Structural equality — compares each field by value                 |
| `hash()`            | Consistent hash based on all fields (usable as dict/set key)       |
| `to_str()`          | Human-readable representation: `User(name: "Alice", age: 30, ...)` |
| `clone()`           | Shallow copy — creates a new instance with the same field values   |
| `copy(**overrides)` | Copy with selective field overrides                                |

```v2
let a = User("Alice", 30, "alice@example.com")
let b = User("Alice", 30, "alice@example.com")

print(a == b)          // true — structural equality, not reference equality
print(a)               // User(name: "Alice", age: 30, email: "alice@example.com")

let c = a.copy(age: 31)
print(c)               // User(name: "Alice", age: 31, email: "alice@example.com")
print(a == c)           // false — age differs
```

`@data` implies `@fixed` — data classes cannot have fields added dynamically.

#### Destructuring Data Classes

Data classes automatically support positional destructuring:

```v2
let User(name, age, email) = a
print(name)    // "Alice"
```

#### Excluding Fields

Use `@data(exclude: ["cache"])` to omit specific fields from equality, hashing, and to_str:

```v2
@data(exclude: ["cache"])
class CachedQuery {
    func constructor(query: str, ttl: int) {
        self.query = query
        self.ttl = ttl
        self.cache = {}    // excluded from equals/hash/to_str
    }
}
```

### Sealed Classes

A `sealed` class restricts which types can extend it. All subclasses must be defined in the **same file** as the sealed parent. This gives you a closed type hierarchy — the compiler knows every possible variant at compile time, enabling exhaustive `match` checking.

```v2
sealed class Shape {
    // no instances of Shape itself — only its subclasses
}

class Circle extends Shape {
    func constructor(radius: float) {
        self.radius = radius
    }
}

class Rectangle extends Shape {
    func constructor(width: float, height: float) {
        self.width = width
        self.height = height
    }
}

class Triangle extends Shape {
    func constructor(a: float, b: float, c: float) {
        self.a = a
        self.b = b
        self.c = c
    }
}
```

#### Exhaustive Matching

Because the compiler knows every subclass, `match` on a sealed class is checked for exhaustiveness — just like enum matching:

```v2
func area(s: Shape) -> float {
    match (s) {
        case (Circle(r)) { return math.PI * r * r }
        case (Rectangle(w, h)) { return w * h }
        case (Triangle(a, b, c)) {
            let sp = (a + b + c) / 2.0
            return math.sqrt(sp * (sp - a) * (sp - b) * (sp - c))
        }
        // no default needed — compiler verifies all variants are covered
    }
}
```

If you add a new subclass and forget to update a `match`, the compiler emits:

```
error[E0305]: non-exhaustive match on sealed class `Shape`
  --> src/geometry.v2:14:5
   |
14 |     match (s) {
   |     ^^^^^ missing case for `Pentagon`
   |
   = help: add `case Pentagon(...)` or a `default` branch
```

#### Sealed + Data

Combine `sealed` with `@data` for algebraic data types with auto-generated methods:

```v2
sealed class Expr {}

@data class Lit extends Expr {
    func constructor(value: float) { self.value = value }
}

@data class Add extends Expr {
    func constructor(left: Expr, right: Expr) {
        self.left = left
        self.right = right
    }
}

@data class Mul extends Expr {
    func constructor(left: Expr, right: Expr) {
        self.left = left
        self.right = right
    }
}

func eval(e: Expr) -> float {
    match (e) {
        case (Lit(v)) { return v }
        case (Add(l, r)) { return eval(l) + eval(r) }
        case (Mul(l, r)) { return eval(l) * eval(r) }
    }
}
```

### Copy-on-Write Classes (`@cow`)

The `@cow` decorator gives a class **copy-on-write** value semantics — like Swift's COW types. Instances share their internal storage until one is mutated, at which point the mutated instance gets a private copy.

```v2
@cow
class Buffer {
    func constructor(data: list<u8>) {
        self.data = data
    }

    func append(byte: u8) {
        self.data.push(byte)    // triggers copy if shared
    }

    func len() -> int {
        return self.data.len()
    }
}
```

```v2
let a = Buffer([1, 2, 3])
let b = a              // b shares a's storage — no copy yet

print(a.len())         // 3
print(b.len())         // 3 — same underlying data

b.append(4)            // b is mutated → private copy is made here
print(a.len())         // 3 — a is unchanged
print(b.len())         // 4 — b has its own copy now
```

How it works:

1. Assignment (`let b = a`) bumps a reference count — no data is copied.
2. On any mutation through a method marked `mut` (or any method that writes to `self`), the runtime checks the reference count.
3. If the count is > 1, a deep copy of the underlying storage is made first, then the mutation proceeds on the private copy.
4. If the count is 1, no copy is needed — the value is uniquely owned.

`@cow` is ideal for large collections, buffers, and string builders where you want the convenience of value semantics without paying for copies on every assignment.

---

## Computed Properties

Computed properties let you define `get` and `set` accessors on class and struct fields. Instead of storing a raw value, the property runs a function on each read or write — useful for validation, lazy derivation, and encapsulation.

### Basic Getter

```v2
class Circle {
    pub radius: float

    get area -> float {
        return math.PI * self.radius * self.radius
    }

    func constructor(r: float) {
        self.radius = r
    }
}

let c = Circle(5.0)
print(c.area)          // 78.539...  — accessed like a field, computed on the fly
// c.area = 10.0       // compile error — no setter defined
```

### Getter + Setter

```v2
class Temperature {
    priv _celsius: float

    get celsius -> float {
        return self._celsius
    }

    set celsius(value: float) {
        if (value < -273.15) { throw ValueError("Below absolute zero") }
        self._celsius = value
    }

    get fahrenheit -> float {
        return self._celsius * 9.0 / 5.0 + 32.0
    }

    set fahrenheit(value: float) {
        self.celsius = (value - 32.0) * 5.0 / 9.0    // reuses the celsius setter validation
    }

    func constructor(c: float) {
        self.celsius = c
    }
}

let t = Temperature(100.0)
print(t.fahrenheit)        // 212.0
t.fahrenheit = 32.0
print(t.celsius)           // 0.0
```

### Computed Properties on Structs

```v2
struct Rect {
    width: float,
    height: float,

    get area -> float {
        return self.width * self.height
    }

    get perimeter -> float {
        return 2.0 * (self.width + self.height)
    }
}

let r = Rect { width: 10.0, height: 5.0 }
print(r.area)        // 50.0
print(r.perimeter)   // 30.0
```

### Lazy Computed Properties

Combine `get` with memoization for properties that are expensive to compute and should only be evaluated once:

```v2
class Report {
    data: list<int>

    lazy get summary -> dict {
        log.info("Computing summary...")
        return {
            "mean": math.mean(self.data),
            "median": math.median(self.data),
            "stddev": math.stddev(self.data)
        }
    }
}

let r = Report { data: list(range(1_000_000)) }
print(r.summary)    // computed on first access
print(r.summary)    // cached — not recomputed
```

`lazy get` stores the result after the first evaluation. The cached value is invalidated if the owning object is mutated through a `set` accessor on any field.

---

## Structs

Value-oriented types with named fields.

### Definition

```v2
struct Point {
    x,
    y
}
```

### Construction

```v2
// Positional
let p1 = Point(10, 20)

// Named fields (struct literal)
let p2 = Point { x: 30, y: 40 }
```

### Struct Update Syntax (Within Structs)

Copy a struct with some fields changed using `...source` inside a struct literal. All fields not listed explicitly are copied from the source value.

```v2
struct Config {
    host: str,
    port: int,
    tls: bool,
    timeout: int
}

let base = Config { host: "localhost", port: 8080, tls: false, timeout: 30 }

// Copy base, overriding only port and tls
let prod = Config { ...base, host: "example.com", tls: true }
// prod = Config { host: "example.com", port: 8080, tls: true, timeout: 30 }
```

`...source` must come first in the literal. Explicitly listed fields after it take precedence over the copied values. You can spread from any value of the same struct type.

```v2
struct Point {
    x: float,
    y: float,
    z: float
}

let origin = Point { x: 0.0, y: 0.0, z: 0.0 }
let above  = Point { ...origin, z: 10.0 }    // only z changes
let moved  = Point { ...above, x: 5.0, y: 3.0 }
```

This also works with classes that use struct-literal construction:

```v2
let updated_user = User { ...existing_user, email: "new@example.com" }
```

### Field Access

```v2
print(p1.x, p1.y)    // 10 20
```

### Impl Blocks

```v2
struct Rectangle {
    width,
    height
}

impl Rectangle {
    func area(self) {
        return self.width * self.height
    }

    func scale(self, factor) {
        return Rectangle {
            width: self.width * factor,
            height: self.height * factor
        }
    }
}

let rect = Rectangle { width: 10, height: 20 }
print(rect.area())             // 200
let big = rect.scale(3)
print(big.area())              // 1800
```

### Typed Fields

```v2
struct TypedPoint {
    x: int,
    y: int,
    label: str
}
```

**Positional construction with typed fields** works exactly the same as with untyped fields — arguments are matched to fields in declaration order:

```v2
let p = TypedPoint(10, 20, "origin")    // x=10, y=20, label="origin"
```

The compiler checks that each positional argument is assignable to the corresponding field's declared type. Named-field construction is always available as an alternative and is preferred when field order is not obvious:

```v2
let p = TypedPoint { x: 10, y: 20, label: "origin" }    // equivalent
```

### Field Default Values

Struct fields can have default values. When constructing a struct, any field with a default can be omitted:

```v2
struct Config {
    host: str = "localhost",
    port: int = 8080,
    tls:  bool = false,
    timeout: int = 30
}

// Use all defaults
let cfg = Config {}

// Override some
let prod = Config { host: "example.com", tls: true }
// prod.port == 8080, prod.timeout == 30

// Override all
let custom = Config { host: "api.example.com", port: 443, tls: true, timeout: 60 }
```

Fields without a default value are required in the struct literal — omitting them is a compile-time error.

### Optional Struct Fields

A field type of `T?` (shorthand for `Option<T>`) makes the field optional — it defaults to `None` if not provided:

```v2
struct User {
    name: str,
    email: str,
    phone?: str,     // optional — defaults to None
    age?:   int      // optional — defaults to None
}

let alice = User { name: "Alice", email: "alice@example.com" }
// alice.phone == None, alice.age == None

let bob = User { name: "Bob", email: "bob@example.com", phone: Some("555-1234") }
```

### Using — Field Scope Export

`using` exports all fields of a struct instance into the current local scope, letting you refer to fields directly by name without the `obj.` prefix.

```v2
struct Config {
    host: str,
    port: int,
    debug: bool
}

let cfg = Config { host: "localhost", port: 8080, debug: true }

using cfg
print(host)     // localhost
print(port)     // 8080
print(debug)    // true
```

`using` is local to the current scope. Fields exported by `using` do not leak into parent or sibling scopes.

```v2
func connect(cfg) {
    using cfg
    print(f"Connecting to ${host}:${port}")    // works inside the function
}

// host and port are not accessible here
```

### cstruct — C ABI Struct

`cstruct` defines a struct that follows C ABI alignment and padding rules. See the dedicated [C Structs (`cstruct`)](#c-structs-cstruct) section for full documentation.

```v2
cstruct Point {
    x: i32,
    y: i32
}
```

---

## C Structs (`cstruct`)

`cstruct` defines a struct whose memory layout is **guaranteed to match the C ABI**. This is required when passing struct pointers to C functions via `extern c` or `cimport`, because V2's regular `struct` may use a different internal layout.

### Definition

```v2
cstruct Point2D {
    x: f32,
    y: f32
}

cstruct Header {
    magic:   u32,
    version: u16,
    flags:   u16,
    length:  u64
}
```

All fields must use sized numeric types (`i8`, `i16`, `i32`, `i64`, `u8`, `u16`, `u32`, `u64`, `f32`, `f64`) or pointers to other `cstruct`s. Plain `int` or `float` are not allowed — the C size would be ambiguous.

### Construction

```v2
let p = Point2D { x: 1.0, y: 2.5 }
let h = Header { magic: 0xDEADBEEF, version: 1, flags: 0, length: 64 }
```

### Passing to C Functions

```v2
cimport "mylib.h"

cstruct Vec3 {
    x: f32,
    y: f32,
    z: f32
}

extern c float vec3_length(Vec3 v)

let v = Vec3 { x: 1.0, y: 2.0, z: 2.0 }
print(vec3_length(v))    // 3.0
```

### Pointers to `cstruct`

```v2
extern c void init_header(Header* hdr)

unsafe {
    let hdr = mem_alloc(mem_size_of("Header"))
    init_header(hdr as Header*)
    mem_free(hdr)
}
```

### Nested `cstruct`

```v2
cstruct RGBA {
    r: u8,
    g: u8,
    b: u8,
    a: u8
}

cstruct Pixel {
    pos:   Point2D,
    color: RGBA
}
```

### Notes

- `cstruct` types are value types — they are copied on assignment, not reference-shared.
- `cstruct` does not support inheritance, trait implementations, or `impl` blocks.
- Fields are laid out in declaration order with C-standard alignment and padding.
- Use `mem_size_of("StructName")` to query the ABI size at compile time.

---

## `using` Keyword

`using` brings the members of a struct, class, or module into the current scope without a prefix. It is the V2 equivalent of `using namespace` (C++) or `open` (OCaml).

### `using` with Structs and Classes

```v2
struct Color {
    r: float,
    g: float,
    b: float
}

let c = Color { r: 0.8, g: 0.2, b: 0.1 }

using c {
    print(r, g, b)    // fields accessible directly without c.r, c.g, c.b
}
```

`using obj { ... }` creates a local scope in which all fields of `obj` are accessible by name. Changes to those names do not affect the original object — it is read-only projection.

### `using` with Modules

```v2
import "std.math"

using std.math {
    let x = sin(PI / 2)    // no need for std.math.sin or math.sin
    let y = cos(0.0)
}
```

Outside the `using` block, the module prefix is required again.

### `using` as a Statement (Flat Import)

```v2
import "std.math"
using std.math    // bring all exported names into this file's scope

let x = sin(PI)    // works everywhere in this file after the using statement
```

This is a flat import — use sparingly to avoid name collisions.

### Notes

- `using` is scoped to the block it appears in (when used with `{ }`), or to the rest of the current file (when used without a block).
- Name conflicts between the outer scope and the `using` target are a compile-time error.
- `using` does not re-export names — symbols brought in via `using` are not visible to other files that import the current one.

---

## Traits

Interfaces / contracts that types can implement.

### Definition

```v2
trait Printable {
    func to_str()                // required (must be implemented)
    func display() {             // optional convenience default
        print(self.to_str())
    }
}
```

> `Printable` requires `to_str()`. `display()` is a default helper that prints `self.to_str()`. Override `display()` only when you need custom print behavior in addition to the required string conversion.

### Implementation

A type can implement any number of traits. Each gets its own `impl TraitName for Type` block:

```v2
struct User {
    name,
    age
}

impl Printable for User {
    func to_str(self) {
        return f"User(${self.name}, ${self.age})"
    }
}

impl Comparable for User {
    func compare(self, other) {
        return self.age - other.age    // sort by age
    }
}

impl Equatable for User {
    func equals(self, other) {
        return self.name == other.name
    }
}

impl Cloneable for User {
    func clone(self) -> Self {
        return User(self.name, self.age)
    }
}

let users = [User("Bob", 25), User("Alice", 30)]
sort(users)                    // works — Comparable
print(users[0])                // User(Bob, 25) — Printable
let copy = clone(users[0])    // works — Cloneable
let u = User("Alice", 30)
u.display()                    // User(Alice, 30)
```

All four traits are independently implemented — each block is separate, and you can add more at any time, even in different files.

V2 defines the following standard traits. Any type can implement them to unlock built-in behavior.

Compatibility aliases are supported for historical code:

- `Printable` is an alias of `Display`
- `Cloneable` is an alias of `Clone`
- `Equatable` is an alias of `Eq`
- `Hashable` is an alias of `Hash`

Method aliases:

- `to_str(self)` is the required string-conversion method; `display(self)` is an optional default helper that prints `self.to_str()`
- `equals(self, other)` and `eq(self, other)` are interchangeable for equality

| Trait          | Required Methods                      | Unlocks                                    |
| -------------- | ------------------------------------- | ------------------------------------------ |
| `Printable`    | `to_str(self) -> str`                 | `print()`, f-string interpolation, `str()` |
| `Comparable`   | `compare(self, other) -> int`         | `<`, `>`, `<=`, `>=`, `sort()`             |
| `Equatable`    | `equals(self, other) -> bool`         | `==`, `!=`, `contains()`                   |
| `Hashable`     | `hash(self) -> int`                   | Use as dict key, set membership            |
| `Iterable`     | `iter(self) -> Iterator`              | `for-in`, `iter.*` combinators             |
| `Iterator`     | `next(self)`, `is_done(self) -> bool` | Manual iteration, `for-in`                 |
| `Cloneable`    | `clone(self)`                         | `clone(val)` builtin                       |
| `Serializable` | `serialize(self) -> str`              | `std.serialize` integration                |
| `Parseable`    | `parse(s: str)` (static)              | `Type.parse(str)` pattern                  |
| `Default`      | `default()` (static)                  | `Type.default()`, `unwrap_or_default()`    |

```v2
struct Color {
    r: int, g: int, b: int
}

impl Printable for Color {
    func to_str(self) {
        return f"rgb(${self.r}, ${self.g}, ${self.b})"
    }
}

impl Comparable for Color {
    func compare(self, other) {
        // compare by luminance
        let lum_self  = self.r * 299 + self.g * 587 + self.b * 114
        let lum_other = other.r * 299 + other.g * 587 + other.b * 114
        return lum_self - lum_other
    }
}

impl Hashable for Color {
    func hash(self) {
        return self.r << 16 bor self.g << 8 bor self.b
    }
}

let colors = [Color {r:255,g:0,b:0}, Color {r:0,g:0,b:255}]
sort(colors)    // works because Comparable is implemented
print(colors[0])    // rgb(...) — works because Printable is implemented
```

### Generic Trait Implementations (Blanket Impls)

You can implement a trait for all types that satisfy a bound — a _blanket implementation_. This lets you add behavior across a whole family of types at once.

```v2
// Implement Printable for any type that is already Serializable
impl<T: Serializable> Printable for T {
    func to_str(self) -> str {
        return self.serialize()
    }
}
```

Now every type that implements `Serializable` automatically also satisfies `Printable`. The compiler selects this blanket impl unless a more specific `impl Printable for ConcreteType` exists (specific impls take precedence).

```v2
struct LogEntry {
    level: str,
    msg:   str
}

impl Serializable for LogEntry {
    func serialize(self) -> str {
        return json_stringify({"level": self.level, "msg": self.msg})
    }
}

// LogEntry.to_str() now works via the blanket impl above
let e = LogEntry { level: "INFO", msg: "started" }
print(e)    // {"level":"INFO","msg":"started"}
```

Blanket implementations must not conflict — two blanket impls that could apply to the same type are a compile-time error.

### The `Self` Type

Inside `impl` blocks and `trait` definitions, `Self` is an alias for the type being implemented or defined. It lets you write methods that refer to their own type without hardcoding the name — useful for method chaining, factory methods, and trait definitions that work across multiple types.

```v2
struct Builder {
    parts: list
}

impl Builder {
    func new() -> Self {
        return Builder { parts: [] }
    }

    func add(self, part: str) -> Self {
        self.parts.push(part)
        return self    // return Self for chaining
    }

    func build(self) -> str {
        return self.parts.join(", ")
    }
}

let result = Builder.new()
    .add("alpha")
    .add("beta")
    .add("gamma")
    .build()
// "alpha, beta, gamma"
```

`Self` in trait definitions refers to whatever type implements the trait:

```v2
trait Cloneable {
    func clone(self) -> Self    // each implementor returns its own type
}

impl Cloneable for Builder {
    func clone(self) -> Self {
        return Builder { parts: self.parts[:] }    // shallow copy
    }
}
```

`Self` is only valid inside `impl` and `trait` blocks. Using it outside is a compile-time error.

### From and Into Traits

`From` and `Into` are paired built-in traits that define how values convert between types. They are the standard mechanism for type conversion in V2 and underpin how the `?` operator propagates errors across different `Result` error types.

#### `From` — Defining a Conversion

Implement `From<T>` on a type to define how it is constructed from `T`:

```v2
trait From<T> {
    func from(val: T) -> Self    // static — called as Type.from(val)
}
```

```v2
struct Celsius { degrees: float }
struct Fahrenheit { degrees: float }

impl From<Celsius> for Fahrenheit {
    func from(c: Celsius) -> Fahrenheit {
        return Fahrenheit { degrees: c.degrees * 9.0 / 5.0 + 32.0 }
    }
}

let boiling = Fahrenheit.from(Celsius { degrees: 100.0 })
print(boiling.degrees)    // 212.0
```

`from` is a static method — call it as `TargetType.from(source_value)`.

#### `Into` — The Reverse Direction

`Into<T>` is the mirror of `From`. Implementing `From<S>` for `T` automatically gives `S` an `Into<T>` implementation for free — you never need to implement `Into` manually.

```v2
trait Into<T> {
    func into(self) -> T
}
```

```v2
// Because Fahrenheit implements From<Celsius>,
// Celsius automatically gets .into() -> Fahrenheit

let c = Celsius { degrees: 0.0 }
let f: Fahrenheit = c.into()
print(f.degrees)    // 32.0
```

The rule: **implement `From`, get `Into` for free.**

#### `From` for Error Types and `?`

The `?` operator uses `From` to convert between error types when propagating through `Result`. If a function returns `Result<T, AppError>` and calls a sub-function returning `Result<U, IoError>`, the `?` operator calls `AppError.from(IoError)` automatically — provided `From<IoError>` is implemented for `AppError`.

```v2
class AppError extends Error {
    func constructor(msg) { super(msg) }
}

impl From<IOError> for AppError {
    func from(e: IOError) -> AppError {
        return new AppError(f"IO failure: ${e.message}")
    }
}

func load(path) {    // returns Result<str, AppError>
    let text = read_file(path)?    // read_file returns Result<str, IOError>
                                   // ? calls AppError.from(IOError) automatically
    return Ok(text)
}
```

Without `From` implemented, using `?` to propagate an incompatible error type is a compile-time error.

#### Built-in `From` Implementations

V2's standard library ships `From` implementations for the most common conversions:

| From           | To      | Behavior                                  |
| -------------- | ------- | ----------------------------------------- |
| `int`          | `float` | Widening numeric conversion               |
| `float`        | `int`   | Truncating (towards zero)                 |
| `str`          | `int`   | Parses decimal string — panics on failure |
| `str`          | `float` | Parses float string — panics on failure   |
| `int`          | `str`   | Same as `str(n)`                          |
| `IOError`      | `Error` | Upcast to base                            |
| `NetworkError` | `Error` | Upcast to base                            |

#### Summary

|             | `From<T>`                       | `Into<T>`                     |
| ----------- | ------------------------------- | ----------------------------- |
| Defined by  | Target type                     | Source type (auto-derived)    |
| Call syntax | `Target.from(val)`              | `val.into()`                  |
| Implement   | Manually on target              | Never — derived automatically |
| Used by `?` | Yes — for error type conversion | No                            |

### The `Cloneable` Trait

`Cloneable` enables explicit deep copying of values via the `clone()` builtin.

#### Definition

```v2
trait Cloneable {
    func clone(self) -> Self
}
```

#### Default Behavior

Primitive values (`int`, `float`, `str`, `bool`) are always cloneable by value — they are copied on assignment anyway and do not need to implement `Cloneable`.

For collections and user-defined types, `clone()` performs a **deep copy** by default when `Cloneable` is derived:

> **Shortcut:** If all fields of your struct are themselves `Clone`, you can skip the manual implementation entirely and use `@derive(Clone)` — see [The `@derive` Decorator](#the-derive-decorator). The manual approach below is only needed when you want custom clone logic (e.g. shallow-copying a field intentionally).

```v2
struct Config {
    host: str,
    port: int,
    tags: list
}

impl Cloneable for Config {
    func clone(self) -> Config {
        return Config {
            host: self.host,       // str — copied by value
            port: self.port,       // int — copied by value
            tags: self.tags[:]     // list — explicit deep copy via slice
        }
    }
}

let base = Config { host: "localhost", port: 8080, tags: ["web", "api"] }
let copy = clone(base)

copy.tags.push("debug")
print(base.tags)    // ["web", "api"]  — unaffected
print(copy.tags)    // ["web", "api", "debug"]
```

#### Shallow vs Deep — Your Responsibility

V2 does not enforce deep vs shallow — your `clone()` implementation decides. The convention is:

- Primitive fields: copy directly.
- List fields: copy with `list[:]` (full slice) or `.map(clone)` for nested objects.
- Dict fields: copy with `dict({...val})` or iterate entries.
- Nested Cloneable structs: call `clone(field)` recursively.

```v2
struct Tree {
    value: int,
    children: list    // list of Tree
}

impl Cloneable for Tree {
    func clone(self) -> Tree {
        return Tree {
            value:    self.value,
            children: self.children.map(lambda(c) => clone(c))    // recursive deep clone
        }
    }
}
```

#### The `clone()` Builtin

```v2
let original = Config { host: "prod.example.com", port: 443, tags: ["prod"] }
let copy = clone(original)    // calls original.clone()
```

`clone()` calls `.clone()` on the value. If the type does not implement `Cloneable`, a runtime `TypeError` is thrown.

#### `unwrap_or_default()` and `Cloneable`

The `Default` trait's `unwrap_or_default()` pattern often pairs with `Cloneable` — you clone a default template rather than constructing from scratch:

```v2
let default_cfg = Config { host: "localhost", port: 8080, tags: [] }

func get_config(key) {
    let result = config_store.get(key)
    return result ?? clone(default_cfg)    // always give a fresh copy
}
```

### `self` in Classes vs Structs

V2 uses `self` consistently in both classes and structs, but the declaration style differs between the two.

#### In Classes — `self` Is Implicit

Inside a class method, `self` refers to the current instance and is available automatically — you do not declare it in the parameter list:

```v2
class Counter {
    func constructor(start) {
        self.count = start    // self available without declaration
    }

    func increment() {
        self.count += 1       // self available without declaration
    }

    func value() {
        return self.count
    }
}

let c = new Counter(0)
c.increment()
print(c.value())    // 1
```

#### In Structs / `impl` Blocks — `self` Is Explicit

In `impl` blocks (for both structs and classes), `self` must be declared as the first parameter. This makes the receiver explicit and lets you name it differently if needed — though `self` is the conventional name.

```v2
struct Rectangle {
    width: float,
    height: float
}

impl Rectangle {
    func area(self) -> float {
        return self.width * self.height
    }

    func scale(self, factor: float) -> Rectangle {
        return Rectangle {
            width:  self.width  * factor,
            height: self.height * factor
        }
    }
}
```

#### Why the Difference?

Classes use `self` implicitly because their methods are always instance methods — there is no concept of a free function inside a class body. `impl` blocks can define static methods (no receiver) alongside instance methods (with `self`), so the presence or absence of `self` as the first parameter is how the compiler distinguishes them:

```v2
impl Config {
    // Instance method — has self
    func describe(self) {
        print(f"${self.host}:${self.port}")
    }

    // Static method — no self
    func default() -> Config {
        return Config { host: "localhost", port: 8080, tls: false, timeout: 30 }
    }
}

let cfg = Config.default()    // static call
cfg.describe()                // instance call
```

#### `self` in Trait `impl` Blocks

The same explicit rule applies when implementing a trait on a class or struct:

```v2
impl Printable for Rectangle {
    func to_str(self) -> str {
        return f"Rect(${self.width}x${self.height})"
    }
}
```

#### Summary

| Context                | `self`      | How Declared          |
| ---------------------- | ----------- | --------------------- |
| Class method body      | Implicit    | Not in parameter list |
| `impl` instance method | Explicit    | First parameter       |
| `impl` static method   | Not present | No `self` parameter   |
| Trait `impl` method    | Explicit    | First parameter       |

### `impl Trait` Return Types

Use `-> impl TraitName` to return a value whose concrete type is hidden — callers only see the trait interface:

```v2
func make_counter() -> impl Iterable {
    return naturals()    // concrete type hidden
}

func make_greeter(lang: str) -> impl Printable {
    if (lang == "en") { return EnglishGreeter {} }
    return SpanishGreeter {}
}
```

This is useful when the concrete return type is complex, private, or varies between branches.

### `dyn Trait` — Dynamic Dispatch

While `impl Trait` resolves the concrete type at compile time (static dispatch), `dyn Trait` creates a **trait object** — a heap-allocated value whose concrete type is erased and looked up at runtime via a vtable. This is the mechanism for runtime polymorphism and heterogeneous collections.

```v2
// A list that holds any Drawable, regardless of concrete type
let shapes: list<dyn Drawable> = []
shapes.push(new Circle(5.0))
shapes.push(new Rectangle(3.0, 4.0))
shapes.push(new Triangle(3.0, 4.0, 5.0))

for (shape in shapes) {
    shape.draw()    // virtual dispatch — calls the correct draw() per type
}
```

`dyn Trait` can be used anywhere a type is expected:

```v2
// As a parameter type
func render_all(items: list<dyn Drawable>) {
    for (item in items) { item.draw() }
}

// As a return type — different concrete types from different branches
func create_shape(kind: str) -> dyn Shape {
    match (kind) {
        case ("circle") { return new Circle(1.0) }
        case ("rectangle") { return new Rectangle(2.0, 3.0) }
        default { return new Circle(0.5) }
    }
}

// As a struct field
struct Renderer {
    target: dyn RenderTarget
}
```

**`impl Trait` vs `dyn Trait` — when to use which:**

|                                     | `impl Trait`                                            | `dyn Trait`                                   |
| ----------------------------------- | ------------------------------------------------------- | --------------------------------------------- |
| Dispatch                            | Static — resolved at compile time                       | Dynamic — resolved at runtime via vtable      |
| Allocation                          | None — value stored inline                              | Heap allocation (boxing)                      |
| Heterogeneous collections           | Not possible                                            | ? `list<dyn Trait>`                           |
| Multiple concrete types in one list | Not possible                                            | ?                                             |
| Performance                         | Faster (no indirection)                                 | Slight overhead per call                      |
| Use when                            | Return type varies but caller doesn't need to mix types | You need a collection of mixed concrete types |

**Trait object limitations:** Not all traits can be made into trait objects. A trait is **object-safe** if all its methods have `self` as the first parameter and do not use generic type parameters in their signatures. Calling `dyn` on a non-object-safe trait is a compile-time error.

```v2
// Object-safe — can use dyn
trait Drawable {
    func draw(self)
    func bounds(self) -> (float, float, float, float)
}

// NOT object-safe — generic method prevents dyn use
trait Converter {
    func convert<T>(self) -> T    // generic — cannot be dyn
}
```

### Custom Iteration with `Iterable`

Any user-defined type can participate in `for-in` loops by implementing the `Iterable` trait.

```v2
trait Iterable {
    func iter(self)    // must return an object with .next() and .is_done()
}
```

The object returned by `.iter()` must implement:

```v2
trait Iterator {
    func next(self)     // returns the next value
    func is_done(self)  // returns true when exhausted
}
```

#### Example — Custom Range Type

```v2
struct StepRange {
    current: int,
    stop: int,
    step: int
}

impl Iterator for StepRange {
    func next(self) {
        let val = self.current
        self.current += self.step
        return val
    }
    func is_done(self) {
        return self.current >= self.stop
    }
}

struct MyRange {
    start: int,
    stop: int,
    step: int
}

impl Iterable for MyRange {
    func iter(self) {
        return StepRange { current: self.start, stop: self.stop, step: self.step }
    }
}

let r = MyRange { start: 0, stop: 10, step: 3 }
for (n in r) {
    print(n)    // 0 3 6 9
}
```

---

## Trait Composition & Supertraits

### Supertraits

A trait can require that implementing types also implement one or more other traits. This is declared with the `:` syntax:

```v2
trait Display {
    func to_str(self) -> str
}

trait Debug: Display {
    func debug_str(self) -> str
}
```

Any type implementing `Debug` must also implement `Display`. The compiler enforces this:

```v2
struct Point { x: int, y: int }

impl Debug for Point {
    func debug_str(self) -> str {
        return f"Point {{ x: ${self.x}, y: ${self.y} }}"
    }
}
// ERROR: `Point` implements `Debug` but does not implement required supertrait `Display`
```

Fix by implementing both:

```v2
impl Display for Point {
    func to_str(self) -> str { return f"(${self.x}, ${self.y})" }
}

impl Debug for Point {
    func debug_str(self) -> str { return f"Point {{ x: ${self.x}, y: ${self.y} }}" }
}
```

### Multiple Supertraits

Require multiple parents with `+`:

```v2
trait Serializable: Display + Hash + Eq {
    func serialize(self) -> bytes
}
```

A type implementing `Serializable` must also implement `Display`, `Hash`, and `Eq`.

### Trait Composition in Bounds

Use `+` in generic bounds to require multiple traits without creating a new named trait:

```v2
func process<T: Display + Hash + Clone>(item: T) {
    let s = item.to_str()
    let h = item.hash()
    let c = item.clone()
}
```

### Default Methods Using Supertraits

A trait's default methods can call methods from its supertraits:

```v2
trait PrettyPrint: Display {
    func pretty(self) -> str {
        return f"=== ${self.to_str()} ==="    // to_str() comes from Display
    }
}
```

### Trait Embedding

Compose a new trait from existing traits with no additional methods — useful for creating capability bundles:

```v2
trait Entity: Display + Hash + Eq + Clone {}

// Now `Entity` can be used as a single bound that requires all four traits
func store<T: Entity>(item: T) {
    // ...
}
```

### Diamond Inheritance

When a type implements multiple traits that share a common supertrait, the shared supertrait is implemented once:

```v2
trait A { func a(self) }
trait B: A { func b(self) }
trait C: A { func c(self) }

struct S {}
impl A for S { func a(self) { print("a") } }
impl B for S { func b(self) { print("b") } }
impl C for S { func c(self) { print("c") } }

// A.a() is implemented once — no ambiguity
```

---

## Trait Associated Types

Associated types let a trait declare an **abstract type placeholder** that implementing types must resolve concretely. This is more expressive than generic parameters when the relationship between the trait and the type is one-to-one.

### Declaring an Associated Type

```v2
trait Container {
    type Item          // abstract — implementors must fill this in

    func get(self, i: int) -> Item
    func len(self) -> int
}
```

### Implementing a Trait with an Associated Type

```v2
struct IntVec {
    data: list
}

impl Container for IntVec {
    type Item = int       // concrete resolution

    func get(self, i: int) -> int {
        return self.data[i]
    }

    func len(self) -> int {
        return self.data.len()
    }
}

struct StrBag {
    items: list
}

impl Container for StrBag {
    type Item = str

    func get(self, i: int) -> str {
        return self.items[i]
    }

    func len(self) -> int {
        return self.items.len()
    }
}
```

### Using Associated Types in Generic Functions

Reference the associated type via `T::Item`:

```v2
func first<T: Container>(c: T) -> T::Item {
    return c.get(0)
}

let v = IntVec { data: [10, 20, 30] }
let x = first(v)    // 10 — inferred as int
```

### Default Associated Types

A trait can supply a default for an associated type. Implementing types may override it or accept the default:

```v2
trait Transformer {
    type Output = str    // default: str

    func transform(self) -> Output
}

struct Upper { val: str }

impl Transformer for Upper {
    // Output defaults to str — no override needed
    func transform(self) -> str {
        return self.val.upper()
    }
}

struct Doubler { val: int }

impl Transformer for Doubler {
    type Output = int    // override default

    func transform(self) -> int {
        return self.val * 2
    }
}
```

### Associated Types vs Generic Parameters

|                           | Generic parameter `<T>`                        | Associated type `type T`                     |
| ------------------------- | ---------------------------------------------- | -------------------------------------------- |
| Number of implementations | One per `T` per type                           | One per type                                 |
| Caller specifies          | Yes, explicitly                                | No — resolved by impl                        |
| Use when                  | A type works with many different element types | The element type is fixed per implementation |

---

## Const Generics

Const generics allow **compile-time constant values** (typically integers or booleans) to be used as generic parameters. This lets you create types and functions parameterized over sizes, capacities, or flags — without any runtime overhead.

### Declaring Const Generic Parameters

Use `const N: int` (or `const FLAG: bool`, etc.) inside angle brackets:

```v2
struct FixedArray<T, const N: int> {
    data: list    // conceptually holds exactly N items of type T
}

impl<T, const N: int> FixedArray<T, N> {
    func new() -> FixedArray<T, N> {
        return FixedArray { data: list(range(N)).map(lambda(_) => null) }
    }

    func get(self, i: int) -> T {
        if (i < 0 || i >= N) { throw new IndexError(f"index ${i} out of bounds for N=${N}") }
        return self.data[i]
    }

    func capacity(self) -> int { return N }
}
```

### Instantiating with Const Generic Arguments

```v2
let buf: FixedArray<int, 16> = FixedArray.new()
let mat: FixedArray<float, 64> = FixedArray.new()

print(buf.capacity())    // 16
print(mat.capacity())    // 64
```

### Const Generic Functions

```v2
func repeat_str<const N: int>(s: str) -> str {
    let result = ""
    for (_ in 0..N) { result += s }
    return result
}

repeat_str<3>("ab")    // "ababab"
repeat_str<0>("ab")    // ""
```

### Static Assertions with Const Parameters

```v2
struct Matrix<const ROWS: int, const COLS: int> {
    data: list
}

impl Matrix {
    func mul<const K: int>(self: Matrix<ROWS, K>, other: Matrix<K, COLS>) -> Matrix<ROWS, COLS> {
        static_assert(K > 0, "inner dimension must be positive")
        // ... matrix multiply ...
    }
}
```

### Notes

- Const generic parameters are evaluated entirely at compile time — they emit no runtime values.
- Supported const types: `int`, `float`, `bool`, `str` (string literals only).
- Two instantiations with different const values are distinct types: `FixedArray<int, 4>` and `FixedArray<int, 8>` are incompatible.
- Const parameters can appear in `static_assert` to catch invalid configurations at compile time.

---

## Pipe and Spread

### Pipe Operator (`|>`)

The pipe operator passes the left-hand side as an argument to the right-hand side. It is designed for readable, left-to-right data transformation chains.

```v2
// Without pipe
let result = sort(filter(map(data, lambda(x) => x * 2), lambda(x) => x > 5))

// With pipe — same semantics, reads left to right
let result = data
    |> map(_, lambda(x) => x * 2)
    |> filter(_, lambda(x) => x > 5)
    |> sort(_)
```

`_` is the **pipe placeholder** — it marks where the piped value is inserted. If you omit `_`, the piped value is passed as the **first argument**:

```v2
"hello world"
    |> split(" ")         // split("hello world", " ")
    |> reverse(_)         // reverse(["hello", "world"])
    |> join(_, " ")       // join(["world", "hello"], " ")
```

#### Multi-Line Pipes

Pipes can span lines freely — the `|>` must appear at the end of the preceding line or the start of the continuation:

```v2
let processed = raw_data
    |> parse_records(_)
    |> filter(_, lambda(r) => r["active"])
    |> sort_by(_, lambda(r) => r["name"])
    |> take(_, 100)
```

#### Pipe with Method Calls

Pipe composes with dot-method chaining — use whichever is more readable:

```v2
// Method chain
let words = "  Hello, World!  ".trim().lower().split(",")

// Pipe chain
let words = "  Hello, World!  "
    |> trim(_)
    |> lower(_)
    |> split(_, ",")
```

#### Pipe and Async

Async functions work naturally in pipes with `await`:

```v2
let result = await (url
    |> http_get(_)
    |> then(_, lambda(resp) => json_parse(resp.body)))
```

---

### Spread Operator (`...`)

`...` has two related roles: **spreading** an iterable into a context that expects individual elements, and **rest collection** that gathers remaining elements.

#### Spreading into List Literals

```v2
let a = [1, 2, 3]
let b = [4, 5, 6]

let combined = [...a, ...b]          // [1, 2, 3, 4, 5, 6]
let extended = [0, ...a, 99, ...b]   // [0, 1, 2, 3, 99, 4, 5, 6]
```

#### Spreading into Dict Literals

```v2
let base = {"host": "localhost", "port": 8080}
let overrides = {"port": 443, "tls": true}

let config = {...base, ...overrides}
// {"host": "localhost", "port": 443, "tls": true}
// — later keys win on collision
```

#### Spreading into Function Calls

```v2
func add(a, b, c) { return a + b + c }

let args = [1, 2, 3]
add(...args)    // 6 — equivalent to add(1, 2, 3)
```

#### Rest Patterns in Destructuring

`...rest` collects the tail of a list into a variable during destructuring:

```v2
let [head, ...tail] = [1, 2, 3, 4, 5]
// head = 1, tail = [2, 3, 4, 5]

let [first, second, ...rest] = items
let [...init, last] = items           // everything but the last
```

#### Rest in Variadic Functions

`...args` in a parameter list gathers all extra arguments into a list:

```v2
func log(level, ...messages) {
    for (msg in messages) { print(f"[${level}] ${msg}") }
}

log("INFO", "Server started", "Port 8080", "PID 12345")
```

---

## Runtime Introspection

V2 provides a set of builtins for inspecting the structure and type of values at runtime. These enable metaprogramming, debugging utilities, serialization, and plugin systems without reaching for macros or compile-time tools.

### Type Inspection

```v2
type(42)            // "int"
type("hello")       // "str"
type([1, 2])        // "list"
type(null)          // "null"
type(lambda() {})   // "func"

// Check a type
42 is int           // true
"hi" is str         // true
42 is str           // false

// Get the class name of a class instance
class Dog { func constructor(name) { self.name = name } }
let d = new Dog("Rex")
type(d)             // "Dog"
```

### Attribute Inspection

```v2
class Point {
    func constructor(x, y) {
        self.x = x
        self.y = y
    }

    func distance_to_origin() {
        return sqrt(self.x ** 2 + self.y ** 2)
    }
}

let p = new Point(3, 4)

// List all attributes and methods
dir(p)                          // ["x", "y", "distance_to_origin", ...]

// Check existence
hasattr(p, "x")                 // true
hasattr(p, "z")                 // false

// Get by name
getattr(p, "x")                 // 3
getattr(p, "distance_to_origin")  // <func distance_to_origin>

// Set by name
setattr(p, "x", 99)
p.x                             // 99

// Check if a value is callable
callable(p.distance_to_origin)  // true
callable(p.x)                   // false
```

### Function Introspection

```v2
func add(a: int, b: int) -> int [effects: none] {
    return a + b
}

is_func(add)             // true
is_func(42)              // false

// Compile-time introspection of effects (in comptime context)
comptime {
    let effects = ct_get_effects("add")    // ["none"]
    let all_funcs = ct_list_funcs()        // list of all defined function names
}
```

### Scope Inspection

```v2
// Get the current scope as a dict
let scope = vars()
print(scope["x"])    // same as print(x)

// Check if a name is defined in current scope
defined("x")         // true if x exists
defined("z")         // false if z does not exist
```

### Dynamic Evaluation

```v2
// Evaluate an expression string — returns the result
let result = eval("1 + 2 + 3")     // 6
eval("print('hello from eval')")   // hello from eval

// Execute a code block — no return value
exec("""
    let x = 10
    let y = 20
    print(x + y)
""")
// 30
```

> **Security note:** `eval` and `exec` run arbitrary V2 code. Never pass untrusted user input to them directly. Use `isolate_exec` instead to sandbox untrusted code.

### `isolate_exec` — Sandboxed Execution

`isolate_exec` runs V2 code inside a lightweight sandbox — a separate memory space with no access to the host program's globals, file system, or network unless explicitly granted. It is the safe alternative to `exec` for untrusted input.

Supported forms:

- `isolate_exec(code, opts?)` — one-shot sandbox execution.
- `isolate_run(iso, code, opts?)` — execute inside an existing named isolate created with `isolate_new()`.

```v2
// Basic sandbox — no I/O, no globals
let result = isolate_exec("1 + 2 + 3")    // Ok(6)

// Sandbox with a timeout
let result = isolate_exec("heavy_work()", { timeout_ms: 500 })
// Ok(value) or Err("timeout")

// Expose specific values to the sandbox
let result = isolate_exec(
    "x * x",
    { globals: { x: 7 } }
)    // Ok(49)

// Run inside a reusable named isolate
let plugin = isolate_new()
isolate_run(plugin, "result = 2 * 21")
let out = isolate_get(plugin, "result")    // 42
```

| Option       | Type    | Description                                                    |
| ------------ | ------- | -------------------------------------------------------------- |
| `timeout_ms` | `int?`  | Kill the sandbox if it runs longer than this many milliseconds |
| `globals`    | `dict?` | Values to expose to the sandboxed code as read-only globals    |
| `allow_io`   | `bool`  | Allow file I/O inside the sandbox (default: `false`)           |
| `allow_net`  | `bool`  | Allow network access inside the sandbox (default: `false`)     |

`isolate_exec` always returns a `Result` — `Ok(value)` on success, `Err(message)` on timeout, sandbox violation, or runtime error. It never throws.

### Struct & Class Field Enumeration

```v2
struct Config {
    host: str,
    port: int,
    tls:  bool
}

let cfg = Config { host: "localhost", port: 8080, tls: false }

// Enumerate fields as a dict
let d = dict(cfg)
// {"host": "localhost", "port": 8080, "tls": false}

for (key in d.keys()) {
    print(f"${key} = ${d[key]}")
}
```

### Trait Membership Check

```v2
trait Printable {
    func display(self)
}

struct Cat {}
impl Printable for Cat {
    func display(self) { print("meow") }
}

let c = Cat {}
c is Printable    // true — runtime trait membership check
```

### Reference Summary

| Function / Operator             | Description                                            |
| ------------------------------- | ------------------------------------------------------ |
| `type(val)`                     | Human-readable type name                               |
| `typeof(val)`                   | Raw type tag                                           |
| `val is Type`                   | Runtime type/trait membership check                    |
| `dir(obj?)`                     | List attribute and method names                        |
| `hasattr(obj, name)`            | Check if attribute exists                              |
| `getattr(obj, name)`            | Get attribute by name                                  |
| `setattr(obj, name, val)`       | Set attribute by name                                  |
| `callable(val)`                 | Is value callable?                                     |
| `is_func(val)`                  | Is value a function?                                   |
| `is_lazy(val)`                  | Is value a lazy expression?                            |
| `vars()`                        | Current scope as dict                                  |
| `defined(name)`                 | Name defined in scope?                                 |
| `eval(code)`                    | Evaluate expression string                             |
| `exec(code)`                    | Execute code string                                    |
| `isolate_exec(code, opts?)`     | Execute code in a one-shot sandbox and return `Result` |
| `isolate_run(iso, code, opts?)` | Execute code in a named isolate and return `Result`    |
| `dict(obj)`                     | Convert struct/class instance to dict of fields        |

---

## Enums

### Basic Enum

```v2
enum Color {
    Red,
    Green,
    Blue
}

let c = Color.Red
print(c == Color.Red)     // true
print(c == Color.Blue)    // false
```

### Enum with Data

```v2
enum Shape {
    Circle(float),
    Rectangle(float, float),    // width, height
    Point
}
```

### Enum in Pattern Matching

Use `case (Variant(binding))` to destructure data carried by enum variants:

```v2
let s = Shape.Circle(3.14)

match (s) {
    case (Shape.Circle(r)) { print(f"circle with radius ${r}") }
    case (Shape.Rectangle(w, h)) { print(f"rect ${w}x${h}") }
    case (Shape.Point) { print("a point") }
}

// Guard on extracted value
match (s) {
    case (Shape.Circle(r)) if r > 1.0 { print("big circle") }
    case (Shape.Circle(r)) { print("small circle") }
    default { print("not a circle") }
}
```

### Enum Methods

Enums can have `impl` blocks:

```v2
impl Shape {
    func area(self) {
        match (self) {
            case (Shape.Circle(r)) { return 3.14159 * r * r }
            case (Shape.Rectangle(w, h)) { return w * h }
            case (Shape.Point) { return 0.0 }
        }
    }
}

let area = Shape.Circle(5.0).area()    // 78.53975
```

### Generic Enums

Enums can have type parameters, letting you define reusable data structures parameterized over a type.

```v2
enum Option<T> {
    Some(T),
    None
}

enum Result<T, E> {
    Ok(T),
    Err(E)
}

enum Tree<T> {
    Leaf(T),
    Branch(Tree<T>, Tree<T>)
}

enum Either<L, R> {
    Left(L),
    Right(R)
}
```

Construct and match on generic enum variants exactly like non-generic ones:

```v2
let t = Tree.Branch(
    Tree.Leaf(1),
    Tree.Branch(Tree.Leaf(2), Tree.Leaf(3))
)

func sum_tree(node: Tree<int>) -> int {
    match (node) {
        case (Tree.Leaf(v)) { return v }
        case (Tree.Branch(l, r)) { return sum_tree(l) + sum_tree(r) }
    }
}

print(sum_tree(t))    // 6
```

Generic enums can have `impl` blocks with the same type parameters:

```v2
impl Either<L, R> {
    func is_left(self) -> bool {
        match (self) {
            case (Either.Left(_)) { return true }
            default { return false }
        }
    }

    func left_or(self, default: L) -> L {
        match (self) {
            case (Either.Left(v)) { return v }
            default { return default }
        }
    }
}

let e: Either<int, str> = Either.Left(42)
print(e.is_left())          // true
print(e.left_or(0))         // 42
```

### Match Exhaustiveness

The compiler checks that `match` on an enum covers all variants. If a variant is not handled and there is no `default` clause, the compiler emits an **exhaustiveness warning** (`--warn exhaustive` to make it an error):

```v2
enum Color { Red, Green, Blue }

match (c) {
    case (Color.Red) { print("red") }
    case (Color.Green) { print("green") }
    // Warning: Color.Blue is not handled — add a case or default
}
```

Add a `default` or cover the missing variant to silence it:

```v2
match (c) {
    case (Color.Red) { print("red") }
    case (Color.Green) { print("green") }
    default {
        print("other")    // covers Blue and any future variants
    }
}
```

For union types (`int | str | null`), exhaustiveness is checked when all branches use typed patterns:

```v2
func handle(val: int | str | null) {
    match (type(val)) {
        case ("int") { return val * 2 }
        case ("str") { return val.upper() }
        // Warning: "null" is not handled
    }
}
```

## Generics

Generics let you write reusable, type-safe code across functions, structs, enums, and impl blocks.

Use type parameters with angle-bracket syntax (`<T, U, ...>`), then constrain them with trait bounds (`T: Comparable`) or `where` clauses when needed.

Const-value generic parameters are documented separately in [Const Generics](#const-generics).

### Generic Functions

```v2
func identity<T>(value: T) -> T {
    return value
}

func swap<T, U>(a, b) {
    return [b, a]
}
```

### Generic Structs

```v2
struct Box<T> {
    value: T
}

struct Pair<T, U> {
    first: T,
    second: U
}
```

Generic type parameters are declared with `<T, U, ...>` and are erased at runtime.

### Trait Bounds on Generics

Use `:` after a type parameter to constrain it to one or more traits. This lets the compiler verify that generic functions only call methods that the type is guaranteed to provide.

```v2
// T must implement Comparable — enables <, >, sort()
func max_of<T: Comparable>(a: T, b: T) -> T {
    return a > b ? a : b
}

// T must be both Printable and Comparable
func print_sorted<T: Printable + Comparable>(items: list) {
    sort(items)
    for (item in items) {
        print(item)
    }
}

// Multiple independent bounds
func transfer<S: Iterable, D: Comparable>(src: S, dst: D) {
    // ...
}
```

Multiple trait bounds on a single parameter are joined with `+`. Calling a generic function with a type that does not satisfy its bounds is a compile-time error.

```v2
struct Score {
    value: int
}

impl Comparable for Score {
    func compare(self, other) {
        return self.value - other.value
    }
}

let a = Score { value: 10 }
let b = Score { value: 20 }
print(max_of(a, b).value)    // 20
```

### Trait Bounds on Generic Structs

```v2
struct SortedList<T: Comparable> {
    items: list
}

impl SortedList {
    func insert<T: Comparable>(self, item: T) {
        self.items.push(item)
        sort(self.items)
    }

    func first(self) {
        return self.items[0]
    }
}
```

### Where Clauses

For complex bounds, use a `where` clause after the parameter list to keep signatures readable:

```v2
func process<T, U>(items: T, handler: U) -> list
    where T: Iterable,
          U: Comparable + Printable
{
    // ...
}
```

---

## Pattern Matching

### Basic Matching

```v2
match (value) {
    case (0) { print("zero") }
    case (1) { print("one") }
    case (2) { print("two") }
    default { print("other") }
}
```

### OR Patterns

```v2
match (day) {
    case ("Sat" | "Sun") { print("weekend") }
    default { print("weekday") }
}
```

### Guard Clauses

```v2
match (x) {
    case (n) if n > 100 { print("big") }
    case (n) if n > 0 { print("small positive") }
    case (0) { print("zero") }
    default { print("negative") }
}
```

### Wildcard

```v2
match (value) {
    case (42) { print("the answer") }
    case (_) { print("something else") }
}
```

### Range Patterns

```v2
match (score) {
    case (90..=100) { print("A") }
    case (80..90) { print("B") }
    case (70..80) { print("C") }
    default { print("F") }
}
```

### Struct Destructuring

```v2
match (point) {
    case (Point { x: 0, y: 0 }) { print("origin") }
    case (Point { x, y }) { print(f"(${x}, ${y})") }
}
```

### As Expression

`match` returns the value of the matched branch:

```v2
let label = match (code) {
    case (200) { "OK" }
    case (404) { "Not Found" }
    case (500) { "Server Error" }
    default { "Unknown" }
}
```

### Pattern Matching — List and Tuple Patterns

In addition to struct destructuring, `match` supports patterns over lists and tuples.

#### List Patterns

Match against the contents of a list using `[...]` patterns:

```v2
match (items) {
    case ([]) { print("empty list") }
    case ([x]) { print(f"one element: ${x}") }
    case ([x, y]) { print(f"two elements: ${x}, ${y}") }
    case ([first, ...rest]) { print(f"head ${first}, tail has ${rest.len()} items") }
    default { print("something else") }
}
```

- `[x]` matches a list with exactly one element, binding it to `x`.
- `[x, y]` matches a list with exactly two elements.
- `[first, ...rest]` matches a list with at least one element; `rest` is a list of the remaining elements.
- `[...init, last]` matches a list with at least one element; `init` is a list of all but the last.
- `[a, ...mid, z]` matches a list with at least two elements; `mid` captures everything between.

```v2
// Parse a simple command protocol: ["cmd", arg1, arg2, ...]
match (message) {
    case (["ping"]) {
        send("pong")
    }
    case (["echo", ...args]) {
        send(args.join(" "))
    }
    case (["set", key, value]) {
        store[key] = value
    }
    case (["quit"]) {
        exit(0)
    }
    default {
        send(f"unknown command: ${message[0]}")
    }
}
```

#### Tuple Patterns

Match against tuples using `(...)` patterns — elements must match exactly by position:

```v2
let point = (3, 0)

match (point) {
    case ((0, 0)) { print("origin") }
    case ((x, 0)) { print(f"on x-axis at ${x}") }
    case ((0, y)) { print(f"on y-axis at ${y}") }
    case ((x, y)) { print(f"at (${x}, ${y})") }
}
```

Tuples don't support rest (`...`) patterns — their length is fixed and must match exactly.

#### Guard Clauses on List and Tuple Patterns

Add `if` guards to patterns for additional constraints:

```v2
match (scores) {
    case ([only]) if only >= 90 { print("solo perfect score") }
    case ([a, b]) if a == b { print("tied pair") }
    case ([first, ...rest]) if first < 0 { print("starts negative") }
    default { print("other") }
}
```

#### Nested Patterns

List and tuple patterns can nest arbitrarily:

```v2
match (data) {
    case ([(x, y), ...rest]) { print(f"first pair: ${x}, ${y}") }
    case ([[inner], outer]) { print(f"nested: ${inner} in ${outer}") }
}
```

#### In `let` Bindings

List and tuple patterns also work in `let` destructuring (documented in Variables & Constants), but `match` is the appropriate tool when you need to handle multiple shapes:

```v2
// let — use when you know the shape
let [head, ...tail] = my_list

// match — use when the shape is unknown
match (my_list) {
    case ([]) { handle_empty() }
    case ([x]) { handle_single(x) }
    default { handle_many(my_list) }
}
```

### Type Pattern Matching

V2 supports two styles for matching on runtime types. Use whichever reads more clearly for your context.

#### Style 1 — `match (type(val))` with string cases

The traditional style. `type(val)` returns a string, and you match against string literals. Works for all types.

```v2
func describe(val: any) {
    match (type(val)) {
        case ("int") { print(f"integer: ${val}") }
        case ("str") { print(f"string: ${val}") }
        case ("list") { print(f"list with ${val.len()} items") }
        case ("null") { print("nothing") }
        default { print(f"other: ${type(val)}") }
    }
}
```

#### Style 2 — `case (TypeName)` direct type patterns

You can match directly against a type name (without calling `type()`). When a branch matches, the value is automatically narrowed to that type inside the branch body.

```v2
func describe(val: int | str | list | null) {
    match (val) {
        case (int) {
            print(f"integer: ${val}")      // val is int here
        }
        case (str) {
            print(f"string: ${val}")       // val is str here
        }
        case (list) {
            print(f"${val.len()} items")   // val is list here
        }
        case (null) { print("nothing") }
    }
}
```

#### Style 3 — `case ((binding: TypeName))` with binding

Combine the type check with a name binding in one step:

```v2
func process(val: any) {
    match (val) {
        case ((n: int)) { return n * 2 }
        case ((s: str)) { return s.upper() }
        case ((lst: list)) { return lst.len() }
        default { return null }
    }
}
```

This is equivalent to `case (int)` with an implicit binding of `val`, but lets you rename the bound value for clarity.

#### Mixing Type Patterns with Other Patterns

Type patterns can appear alongside value patterns and guards in the same `match` block:

```v2
match (val) {
    case ((n: int)) if n < 0 { print("negative int") }
    case ((n: int)) { print(f"positive int: ${n}") }
    case ((s: str)) if s == "" { print("empty string") }
    case (str) { print("non-empty string") }
    default { print("something else") }
}
```

**Summary of type-matching styles:**

| Style           | Syntax                                       | Notes                      |
| --------------- | -------------------------------------------- | -------------------------- |
| String dispatch | `match (type(val)) { case ("int") { ... } }` | Always works; no narrowing |
| Direct type     | `match (val) { case (int) { ... } }`         | Narrows type in branch     |
| Binding         | `match (val) { case ((n: int)) { ... } }`    | Narrows and renames        |

### Matching `Result` and `Option` Variants

`match` works directly on `Ok`, `Err`, `Some`, and `None` — this is the idiomatic way to branch on the outcome of a fallible operation without using `is_ok()` / `unwrap()`.

```v2
// Match on Result
let result = parse_int("42")

match (result) {
    case (Ok(n)) { print(f"parsed: ${n}") }
    case (Err(e)) { print(f"failed: ${e}") }
}

// Match on Option
let user = db_find_user(id)

match (user) {
    case (Some(u)) { print(f"found: ${u.name}") }
    case (None) { print("user not found") }
}
```

Guard clauses work on variant patterns too:

```v2
match (result) {
    case (Ok(n)) if n > 100 { print("big number") }
    case (Ok(n)) { print(f"small: ${n}") }
    case (Err(e)) if e.contains("timeout") { retry() }
    case (Err(e)) { print(f"error: ${e}") }
}
```

Nested `Result`/`Option` matching:

```v2
match (outer) {
    case (Ok(Some(val))) { print(f"got: ${val}") }
    case (Ok(None)) { print("ok but empty") }
    case (Err(e)) { print(f"error: ${e}") }
}
```

`match` on a `Result` or `Option` is exhaustive — the compiler warns if you omit a reachable variant and there is no `default` branch.

---

## Error Handling

V2 offers three complementary error models. Choose based on the context:

| Model                      | Best for                                                                               |
| -------------------------- | -------------------------------------------------------------------------------------- |
| `throw` / `try` / `catch`  | Unexpected failures, propagating errors across layers, interop with code you don't own |
| `Result` / `Ok` / `Err`    | Functions where failure is a normal, expected outcome; explicit error threading        |
| `Option` / `Some` / `None` | Optional values — absence is not an error, just a possibility                          |

They can be freely mixed. A function returning `Result` can internally use `try`/`catch`, and `?` works in any function returning `Result` or `Option`. To convert between models:

```v2
// Option ? Result
let r = some_option.ok_or("value was missing")   // Some(x) ? Ok(x), None ? Err("value was missing")

// Result ? Option
let o = some_result.ok()                          // Ok(x) ? Some(x), Err(_) ? None

// Exception ? Result (wrap a throwing call)
let result = try_wrap(lambda() => risky_call())   // Ok(value) or Err(error)
```

### `try_wrap` — Convert Exceptions to Result

`try_wrap(fn)` calls `fn` with no arguments. If `fn` returns normally, `try_wrap` returns `Ok(value)`. If `fn` throws, `try_wrap` catches the error and returns `Err(error)` — no exception propagates.

This is useful for adapting exception-throwing APIs (including builtins) into the `Result` style without a full `try`/`catch` block:

```v2
let result = try_wrap(lambda() => int("not a number"))
// Err(ParseError: ...) — int() would normally throw

let result = try_wrap(lambda() => json_parse(raw))
match (result) {
    case (Ok(data)) { process(data) }
    case (Err(e)) { print(f"Bad JSON: ${e.message}") }
}
```

`try_wrap` accepts any zero-argument callable, including async lambdas (in which case it returns a `Promise<Result<T, Error>>`, where `T` is the callable's resolved value type).

### Custom Error Types

Define typed errors using classes. Inherit from `Error` or any subclass:

```v2
class NetworkError extends Error {
    func constructor(msg, status_code) {
        super(msg)
        self.status_code = status_code
    }
}

class RequestTimeoutError extends NetworkError {
    func constructor(url) {
        super(f"Request to ${url} timed out", 408)
        self.url = url
    }
}
```

### Typed Catch

Catch specific error types using `catch (name: Type)`:

```v2
try {
    let resp = await http_get("https://example.com")
} catch (e: RequestTimeoutError) {
    print(f"Timeout on ${e.url}")
} catch (e: NetworkError) {
    print(f"Network error ${e.status_code}: ${e.message}")
} catch (e) {
    print(f"Unknown error: ${e}")    // fallback — catches anything
} finally {
    cleanup()
}
```

Catch clauses are checked top-to-bottom. The first matching type wins. A bare `catch (e)` with no type annotation catches everything.

### Error Hierarchy

V2 has a built-in error base class and common subtypes:

| Class                  | Description                                                                                                                        |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `Error`                | Base class for all errors                                                                                                          |
| `TypeError`            | Wrong type used                                                                                                                    |
| `ValueError`           | Value out of expected range                                                                                                        |
| `IndexError`           | List/string index out of bounds                                                                                                    |
| `KeyError`             | Dict key not found                                                                                                                 |
| `OverflowError`        | Integer overflow (in panic mode)                                                                                                   |
| `CancelledError`       | Async task was cancelled                                                                                                           |
| `IOError`              | File or stream operation failed                                                                                                    |
| `NetworkError`         | Network operation failed                                                                                                           |
| `TimeoutError`         | Operation exceeded its time limit                                                                                                  |
| `ParseError`           | Failed to parse a string (JSON, TOML, number, etc.)                                                                                |
| `NotImplementedError`  | Called a method that is declared but not yet implemented                                                                           |
| `AggregateError`       | Multiple errors collected into one — thrown by `Promise.any` when all promises fail; exposes `.errors` (list of individual errors) |
| `AssertionError`       | `assert()` failed                                                                                                                  |
| `PatchInProgressError` | `patch()` called on a function that is currently on the call stack                                                                 |

All built-in errors expose `.message` and `.stack` properties.

### Throw Typed Errors

```v2
func parse_age(s) {
    let n = int(s)
    if (n < 0 || n > 150) {
        throw new ValueError(f"Age out of range: ${n}")
    }
    return n
}
```

### Option Type

Represents an optional value — either `Some(value)` or `None`.

```v2
let found = Some(42)
let missing = None

if (is_some(found)) {
    print(unwrap(found))       // 42
}

// Unwrap with default
let val = unwrap_or(missing, -1)    // -1

// Null coalescing (same effect)
let val2 = missing ?? -1            // -1
```

### Result Type

Represents success or failure — either `Ok(value)` or `Err(error)`.

```v2
func parse_int(s) {
    if (s.isdigit()) {
        return Ok(int(s))
    }
    return Err(f"Invalid number: ${s}")
}

let result = parse_int("42")
if (is_ok(result)) {
    print(unwrap(result))          // 42
}

let bad = parse_int("abc")
if (is_err(bad)) {
    print(unwrap_err(bad))         // Invalid number: abc
}
```

### Option & Result Chaining

`Option` and `Result` support method chaining for ergonomic functional error handling.

#### Option Methods

```v2
let val = Some(5)

val.map(lambda(x) => x * 2)          // Some(10)
val.and_then(lambda(x) => Some(x+1)) // Some(6)
val.or_else(lambda() => Some(0))     // Some(5)
val.filter(lambda(x) => x > 3)       // Some(5)
val.unwrap_or(99)                     // 5

let nothing = None
nothing.map(lambda(x) => x * 2)      // None
nothing.unwrap_or(99)                 // 99
nothing.or_else(lambda() => Some(0)) // Some(0)
```

#### Result Methods

```v2
let ok = Ok(10)
let err = Err("oops")

ok.map(lambda(x) => x * 2)              // Ok(20)
ok.map_err(lambda(e) => f"wrapped: ${e}") // Ok(10) — passthrough
err.map(lambda(x) => x * 2)             // Err("oops") — passthrough
err.map_err(lambda(e) => f"wrapped: ${e}") // Err("wrapped: oops")

ok.and_then(lambda(x) => Ok(x + 1))    // Ok(11)
ok.or_else(lambda(e) => Ok(0))         // Ok(10) — passthrough
err.or_else(lambda(e) => Ok(0))        // Ok(0)

ok.unwrap_or(0)                         // 10
err.unwrap_or(0)                        // 0
```

#### Option/Result Method Matrix

| Method                      | `Option` | `Result` | Description                                                                                        |
| --------------------------- | -------- | -------- | -------------------------------------------------------------------------------------------------- |
| `.map(f)`                   | ?        | ?        | Transform the inner value if present/ok                                                            |
| `.map_err(f)`               | —        | ?        | Transform the error value if err                                                                   |
| `.and_then(f)`              | ?        | ?        | Chain — `f` must return same wrapper type                                                          |
| `.or_else(f)`               | ?        | ?        | Fallback — called only on `None`/`Err`                                                             |
| `.filter(f)`                | ?        | —        | `None` if predicate fails                                                                          |
| `.unwrap_or(default)`       | ?        | ?        | Value or default                                                                                   |
| `.ok_or(err)`               | ?        | —        | `Some(x)` ? `Ok(x)`, `None` ? `Err(err)`                                                           |
| `.ok()`                     | —        | ?        | `Ok(x)` ? `Some(x)`, `Err(_)` ? `None`                                                             |
| `.is_some()` / `.is_none()` | ?        | —        | Check variant                                                                                      |
| `.is_ok()` / `.is_err()`    | —        | ?        | Check variant                                                                                      |
| `.flatten()`                | ?        | ?        | `Some(Some(x))` ? `Some(x)`, `Some(None)` ? `None`; `Ok(Ok(x))` ? `Ok(x)`, `Ok(Err(e))` ? `Err(e)` |
| `.unwrap_or_default()`      | ?        | ?        | Unwrap or call `Type.default()` if None/Err                                                        |

### The `?` Try Operator

`?` is a postfix operator for short-circuit error propagation. It eliminates the boilerplate of manually checking and returning errors on every fallible call.

**In a function returning `Result`:** `expr?` unwraps `Ok(value)` to `value`, or immediately returns `Err(e)` to the caller if the result is an error.

**In a function returning `Option`:** `expr?` unwraps `Some(value)` to `value`, or immediately returns `None` to the caller.

```v2
// Without ?
func load_config(path) {
    let text = match (read_file(path)) {
        case (Ok(t)) { t }
        case (Err(e)) { return Err(e) }
    }
    let parsed = match (json_parse(text)) {
        case (Ok(p)) { p }
        case (Err(e)) { return Err(e) }
    }
    return Ok(parsed)
}

// With ? — identical semantics, much less noise
func load_config(path) {
    let text   = read_file(path)?
    let parsed = json_parse(text)?
    return Ok(parsed)
}
```

**Requirements:** The enclosing function must return `Result` or `Option`. Using `?` in a function returning a plain value is a compile-time error.

**Error conversion:** If `?` unwraps a `Result<T, E1>` in a function returning `Result<U, E2>`, E1 must be convertible to E2 — either by implementing the `From` trait or by being the same type.

**Mixing `Option` and `Result`:** In a `Result`-returning function, `?` cannot be applied directly to an `Option`. Convert with `.ok_or(...)` (or equivalent) first.

```v2
// Chaining multiple fallible steps cleanly
func process_order(id) {    // returns Result
    let order    = db_find_order(id).ok_or("order not found")?
    let user     = db_find_user(order.user_id).ok_or("user not found")?
    let receipt  = send_email(user.email)?
    return Ok(receipt)
}
```

### Optional Chaining (`?.`)

Safely access members that may be null:

```v2
let city = user?.address?.city       // None if any link is null
user?.greet()                        // call only if non-null
```

---

## `defer` and Exceptions

`defer` blocks run when a function exits — this includes all exit paths: normal `return`, early `return`, and unhandled `throw`.

### `defer` Runs on `throw`

If a `throw` propagates out of a function without being caught inside it, all pending `defer` blocks for that function still execute before the exception travels to the caller:

```v2
func risky(path) {
    let f = file_open(path)
    defer {
        file_close(f)    // runs even if throw below fires
    }

    let data = parse(read_file(path))    // might throw ParseError
    return data
}

// If parse() throws, file_close(f) still runs — no resource leak
try {
    let result = risky("bad.json")
} catch (e) {
    print(f"Caught: ${e.message}")
}
```

### `defer` Does Not Suppress Exceptions

A `defer` block does not catch or swallow the exception — it runs cleanup and then lets the exception continue propagating. To handle the exception, use `try`/`catch` inside or around the function.

### `defer` and the Return Value

`defer` runs after a return statement is reached and before the function exits.

If the return statement returns a local binding (`return out`), deferred code can still mutate that binding before it is handed to the caller:

```v2
func example() {
    let out = 41
    defer { out = 99 }
    return out     // returns 99
}
```

This is why named return-variable style works naturally with `defer`:

```v2
func with_result() {
    let out = compute()
    defer {
        out = transform(out)
    }
    return out
}
```

### Order: Multiple Defers with Exceptions

Multiple `defer` blocks still run in reverse order even when an exception is in flight:

```v2
func multi_cleanup() {
    defer { print("C — runs third") }
    defer { print("B — runs second") }
    defer { print("A — runs first") }
    throw "error"
}

try { multi_cleanup() } catch (_) {}
// prints: A — runs first
//         B — runs second
//         C — runs third
```

---

## Generators

Generator functions produce a sequence of values lazily using `yield`.

### Definition

```v2
func* countdown(n) {
    while (n > 0) {
        yield n
        n -= 1
    }
}
```

The `*` is optional: any plain `func` whose body contains a `yield` is
automatically treated as a generator (nested function bodies don't count —
they yield for themselves).

### Manual Iteration

```v2
let gen = countdown(3)

print(gen.next())     // {done: false, value: 3}
print(gen.next())     // {done: false, value: 2}
print(gen.next())     // {done: false, value: 1}
print(gen.next())     // {done: true, value: null}
```

### For-In Iteration

```v2
for (n in countdown(5)) {
    print(n)     // 5, 4, 3, 2, 1
}
```

### Collect to List

```v2
let all = countdown(5).collect()    // [5, 4, 3, 2, 1]
```

### Infinite Generators

```v2
func* naturals() {
    let n = 0
    while (true) {
        yield n
        n += 1
    }
}

let gen = naturals()
print(gen.next())    // {done: false, value: 0}
print(gen.next())    // {done: false, value: 1}
```

### Yield Delegation

```v2
func* inner() {
    yield 1
    yield 2
}

func* outer() {
    yield 0
    yield* inner()    // delegates to inner generator
    yield 3
}
// outer() yields: 0, 1, 2, 3
```

### Two-Way Communication — `.send(val)`

`yield` is an expression as well as a statement. When the caller calls `.send(value)` instead of `.next()`, the yielded value inside the generator becomes the result of the `yield` expression, allowing the caller to push data back into the generator.

```v2
func* accumulator() {
    let total = 0
    while (true) {
        let n = yield total    // yield current total; receive next addend
        if (n == null) { return }
        total += n
    }
}

let acc = accumulator()
acc.next()          // {done: false, value: 0}  — prime the generator
acc.send(10)        // {done: false, value: 10} — total is now 10
acc.send(5)         // {done: false, value: 15} — total is now 15
acc.send(20)        // {done: false, value: 35}
acc.send(null)      // {done: true, value: null} — generator returned
```

**Rules for `.send()`:**

- The generator must be primed first with `.next()` before `.send()` can be called — this advances to the first `yield`.
- Calling `.send(val)` resumes the generator and makes `val` the result of the `yield` expression that is currently suspended.
- Calling `.next()` is equivalent to `.send(null)`.
- Calling `.send()` on a finished generator (`.is_done() == true`) is a no-op returning `{done: true, value: null}`.

```v2
// Pipeline: a generator that transforms values sent to it
func* doubler() {
    while (true) {
        let x = yield
        if (x == null) { return }
        yield x * 2
    }
}

let d = doubler()
d.next()         // prime
d.send(5)        // {done: false, value: 10}
d.next()         // advance to next yield (inner loop)
d.send(7)        // {done: false, value: 14}
```

The `.send()` method is also in the Generator Methods reference table.

---

## Async / Await

V2 has a real event-loop-based async runtime. `async func` returns a `Promise`. `await` suspends the current task until the Promise resolves without blocking the thread — other tasks continue running in the meantime.

### Async Functions

```v2
async func fetch_user(id) {
    let resp = await http_get(f"https://api.example.com/users/${id}")
    return json_parse(resp.body)
}

async func main() {
    let user = await fetch_user(1)
    print(user["name"])
}

await main()
```

### Concurrent Execution — `Promise.all`

Run multiple async tasks concurrently and wait for all to finish:

```v2
async func main() {
    let [a, b, c] = await Promise.all([
        fetch_user(1),
        fetch_user(2),
        fetch_user(3),
    ])
    print(a["name"], b["name"], c["name"])
}
```

### Race — `Promise.race`

Settle with whichever Promise finishes first — whether it resolves successfully or rejects. If the first to finish is a rejection, `Promise.race` rejects immediately with that error. If you need the first _successful_ result instead, use `Promise.any`.

```v2
let result = await Promise.race([
    fetch_from_primary(),
    fetch_from_fallback(),
])
```

### All Settled — `Promise.allSettled`

Wait for all Promises to finish regardless of success or failure. Unlike `Promise.all`, this never rejects early — it gives you a result for every input:

```v2
let results = await Promise.allSettled([
    fetch_user(1),
    fetch_user(2),
    fetch_user(999),    // this one might fail
])

for (r in results) {
    match (r) {
        case (Ok(val)) { print(f"ok: ${val}") }
        case (Err(e)) { print(f"failed: ${e}") }
    }
}
```

Each element of the returned list is either `Ok(value)` or `Err(error)` — it never throws.

### Any — `Promise.any`

Resolve with the first Promise that succeeds. Only rejects if **all** Promises fail:

```v2
// Use the fastest mirror that actually works
let data = await Promise.any([
    fetch_from_mirror_1(),
    fetch_from_mirror_2(),
    fetch_from_mirror_3(),
])
// resolves with the first Ok result
// throws AggregateError if all three fail
```

### Timeouts & Cancellation

```v2
// Fail if not resolved within 5000ms
let result = await Promise.timeout(fetch_user(1), 5000)

// Manual cancellation token
let token = cancel_token()
let p = fetch_large_file(token)

sleep(1000)
token.cancel()    // abort the in-flight task

// Handle cancellation
try {
    let data = await p
} catch (e: CancelledError) {
    print("Task was cancelled")
}
```

### Async Iteration

Consume an async stream of values:

```v2
async func* stream_lines(url) {
    let conn = await tcp_connect(url, 80)
    while (!conn.is_done()) {
        yield await conn.read_line()
    }
}

async func main() {
    for await (line in stream_lines("example.com")) {
        print(line)
    }
}
```

### Error Handling in Async

Errors thrown inside async functions propagate through `await` normally:

```v2
async func risky() {
    throw "something went wrong"
}

async func main() {
    try {
        await risky()
    } catch (e) {
        print(f"Caught: ${e}")
    }
}
```

### Promise API

#### Creating a Promise Manually

Most of the time you get a `Promise` by calling an `async func` or using `Promise.resolve` / `Promise.reject`. When you need to wrap a callback-based API, create one explicitly:

```v2
import "std.time"

// Promise(resolver) — resolver receives (resolve, reject) callbacks
let p = Promise(lambda(resolve, reject) {
    time.set_timeout(1000, lambda() {
        resolve(42)           // fulfil the promise with a value
    })
})

let val = await p    // 42 — after ~1 second

// Reject example
let p2 = Promise(lambda(resolve, reject) {
    if (bad_condition) {
        reject(new NetworkError("connection refused", 503))
    } else {
        resolve(fetch_data())
    }
})
```

The `resolve` callback accepts a single value. The `reject` callback accepts any value — typically an `Error` instance. Calling either one after the promise has already settled is a no-op.

#### Promise Combinators and Methods

| Function / Method          | Description                                                                                                             |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `Promise.all(list)`        | Wait for all — returns list of results                                                                                  |
| `Promise.race(list)`       | First to settle wins — resolves or rejects with that value; rejects immediately if first-to-finish rejects              |
| `Promise.any(list)`        | First resolved (non-error) wins — throws `AggregateError` if all fail                                                   |
| `Promise.allSettled(list)` | Wait for all, each result is `Ok(val)` or `Err(e)` — never throws                                                       |
| `Promise.timeout(p, ms)`   | Reject if not resolved within `ms` milliseconds                                                                         |
| `Promise.resolve(val)`     | Wrap a value in an already-resolved Promise                                                                             |
| `Promise.reject(err)`      | Wrap an error in an already-rejected Promise                                                                            |
| `p.and_then(f)`            | Chain: when `p` resolves, call `f(value)` — `f` must return a Promise; if `p` rejects, skip `f` and propagate the error |
| `p.map(f)`                 | Transform the resolved value: `f(value)` returns a plain value (not a Promise)                                          |
| `p.catch(f)`               | Handle rejection: call `f(error)` if `p` rejects; resolved values pass through                                          |
| `p.finally(f)`             | Run `f()` when `p` settles regardless of outcome — useful for cleanup; does not alter the resolved value                |
| `cancel_token()`           | Create a cancellation token                                                                                             |
| `token.cancel()`           | Cancel tasks holding the token                                                                                          |
| `token.is_cancelled()`     | Check if cancelled                                                                                                      |

---

### Async and Threads Model

V2 has two independent concurrency models — the async event loop and OS threads — and understanding how they interact is important for correct programs.

### The Async Runtime

`async func` / `await` runs on a **single-threaded event loop by default** (`--async-workers 1`). All `await` points in your program share one worker unless multi-worker scheduling is enabled. This means:

- Two `await` expressions never run simultaneously on the same thread.
- CPU-bound work inside an `async func` blocks the event loop — all other async tasks stall until it completes.
- I/O-bound work (network, file, timers) suspends efficiently — the event loop serves other tasks while waiting.

### Running Async Inside a Thread

You can run the async runtime inside a thread to give it its own event loop:

```v2
let t = thread_spawn(lambda() {
    // This thread gets its own async event loop
    async func fetch_all() {
        let [a, b] = await Promise.all([http_get(url1), http_get(url2)])
        return [a, b]
    }

    return await fetch_all()
})

let results = thread_join(t)
```

Each thread that uses `await` operates its own independent event loop. Async tasks started in one thread do not migrate to another.

### Offloading CPU Work from Async

To avoid blocking the event loop with CPU-intensive work, spawn it into a thread and wrap the result in a Promise:

```v2
func async_compute(data) {
    // Returns a Promise that resolves when the thread finishes
    return Promise.resolve(null).and_then(lambda(_) {
        let t = thread_spawn(lambda() => heavy_computation(data))
        return thread_join(t)
    })
}

async func main() {
    let result = await async_compute(large_dataset)
    print(result)
}
```

### Sharing Data Between Async and Threads

Async tasks and threads cannot share mutable data directly — use channels for the async?thread boundary:

```v2
let ch = chan_create(1)

// Async side — sends into channel
async func producer() {
    let data = await http_get("https://api.example.com/data")
    chan_send(ch, data)
}

// Thread side — receives from channel
let worker = thread_spawn(lambda() {
    let data = chan_recv(ch)    // blocks until async produces
    return process(data)
})

await producer()
let result = thread_join(worker)
```

> **Thread-to-thread shared state:** If you need multiple _threads_ (not async tasks) to share mutable data — for example, a shared cache — use `mutex_create` / `mutex_with`, `rwmutex_create`, or `atomic_new`. These synchronisation primitives are fully documented in [Channels and Threads](#channels-and-threads).

### Summary

| Scenario                               | Recommended approach                                                                        |
| -------------------------------------- | ------------------------------------------------------------------------------------------- |
| I/O concurrency                        | `async` / `await` with `Promise.all`                                                        |
| CPU concurrency                        | `thread_spawn` + `thread_join`                                                              |
| CPU work inside async                  | Spawn to thread, join result                                                                |
| Data sharing between async and threads | Channels (`chan_create` / `chan_send` / `chan_recv`)                                        |
| Shared mutable state between threads   | `mutex_with` / `rwmutex_*` / `atomic_*` — see [Channels and Threads](#channels-and-threads) |
| Multiple independent event loops       | One async runtime per thread                                                                |

### Multi-Worker Async Runtime

Enable parallel async scheduling across multiple workers:

```bash
v2 --async-workers 8 app.v2
```

Or in `v2.toml`:

```toml
[runtime]
async_workers = 8
```

When `async_workers > 1`, ready tasks are scheduled across a worker pool. Values that cross worker boundaries must satisfy the `Sendable` trait; non-sendable captures are rejected at compile time.

---

## Structured Concurrency

V2 provides **structured concurrency** — every spawned task has an owner (a `TaskGroup`), and the group guarantees that all child tasks complete (or are cancelled) before the group scope exits. This eliminates leaked background tasks, a common source of bugs in async code.

### Task Groups

A `TaskGroup` is a scope that owns child tasks. When the scope ends, all children are awaited automatically:

```v2
async func fetch_dashboard() {
    let group = task_group()

    group.spawn(async lambda() { await fetch_user_profile() })
    group.spawn(async lambda() { await fetch_notifications() })
    group.spawn(async lambda() { await fetch_recent_activity() })

    let [profile, notifs, activity] = await group.join_all()
    return Dashboard { profile, notifs, activity }
}
```

If any child throws, `join_all` cancels the remaining children and propagates the first error.

### Scoped Syntax

For convenience, `task_scope` provides a block-based API that automatically joins on block exit:

```v2
async func load_all(ids) {
    let results = await task_scope(async func(scope) {
        for (id in ids) {
            scope.spawn(async lambda() { await fetch_item(id) })
        }
        // all tasks are joined here automatically when the block returns
    })
    return results
}
```

`task_scope` guarantees: when the block exits — whether normally, via `return`, or via `throw` — all spawned tasks are cancelled and awaited before execution continues past the `task_scope` call.

### Cancellation Propagation

Cancelling a group cancels all its children recursively. Groups can nest, forming a tree:

```v2
async func pipeline() {
    let outer = task_group()

    outer.spawn(async lambda() {
        let inner = task_group()
        inner.spawn(async lambda() { await step_a() })
        inner.spawn(async lambda() { await step_b() })
        await inner.join_all()
    })

    outer.spawn(async lambda() { await step_c() })

    // If outer is cancelled, the inner group is also cancelled
    await outer.join_all()
}
```

Tasks check for cancellation at every `await` point. A cancelled task receives a `CancelledError` which unwinds the stack, running `defer` blocks normally.

### Timeouts on Groups

```v2
let group = task_group()
group.spawn(async lambda() { await slow_operation() })
group.spawn(async lambda() { await another_operation() })

// If not finished within 5 seconds, cancel all children
let results = await group.with_timeout(5000).join_all()
```

### Error Strategies

By default, the first error cancels siblings. Alternative strategies:

```v2
// Collect all results, even failures
let group = task_group(on_error: "collect")
group.spawn(async lambda() { await might_fail_1() })
group.spawn(async lambda() { await might_fail_2() })

let results = await group.join_all()
// results is a list of Ok(value) | Err(error) — like Promise.allSettled
```

| Strategy    | Behavior                                             |
| ----------- | ---------------------------------------------------- |
| `"cancel"`  | First error cancels siblings (default)               |
| `"collect"` | Await all; return `Ok`/`Err` per child               |
| `"ignore"`  | Swallow child errors; return only successful results |

### Structured Concurrency API

| Function / Method         | Description                                 |
| ------------------------- | ------------------------------------------- |
| `task_group()`            | Create a new task group                     |
| `task_group(on_error: s)` | Create a group with an error strategy       |
| `task_scope(async fn)`    | Block-scoped group that auto-joins on exit  |
| `group.spawn(task)`       | Spawn a child task owned by the group       |
| `group.join_all()`        | Await all children; returns list of results |
| `group.cancel()`          | Cancel all unfinished children              |
| `group.with_timeout(ms)`  | Attach a timeout to the group scope         |
| `group.count()`           | Number of currently active children         |

---

## Macros

### Definition

```v2
macro debug!(expr) {
    print(f"[DEBUG] ${expr}")
}

macro max2!(a, b) {
    a > b ? a : b
}
```

### Usage

```v2
debug!("hello")           // [DEBUG] hello
let m = max2!(10, 20)     // 20
```

### Assert Macro

```v2
macro assert!(condition, message) {
    if (!condition) {
        throw message
    }
}

assert!(x > 0, "x must be positive")
```

Macros are hygienic — the macro body is evaluated with parameter substitution at the call site.

Macro expansion depth is guarded in the runtime. You can tune it with:

```v2
comptime {
    ct_set_macro_limit(128)
    print(ct_get_macro_limit())
}
```

> **Note:** `ct_set_macro_limit` and `ct_get_macro_limit` are compile-time intrinsics — they must be called from a `comptime` block or `comptime func`. See the [Compile-Time Intrinsics](#compile-time-execution) table for the full list of `ct_*` functions.

### Pattern Macros

Pattern macros match syntax patterns and rewrite them at compile time. They are more powerful than substitution macros because they operate on the structure of expressions, not just textual slots.

#### Syntax

```v2
macro pattern name {
    $pattern => $rewrite ;
    $pattern => $rewrite ;
}
```

Each clause is a rule: if the pattern matches at the call site, the corresponding rewrite is emitted instead. Clauses are tried top to bottom; the first match wins.

#### Type-Constrained Patterns

Pattern variables can be constrained to a type:

```v2
macro pattern simplify {
    $x:int + 0    => $x ;
    0 + $x:int    => $x ;
    $x:int * 1    => $x ;
    $x:int * 0    => 0  ;
}
```

Available constraints: `:int`, `:float`, `:str`, `:bool`, `:any`.

#### Example — Logging Wrapper

```v2
macro pattern timed {
    $name:str => $expr => {
        let __t0 = time()
        let __result = $expr
        print(f"[${$name}] took ${time() - __t0}ms")
        __result
    } ;
}

let val = timed "compute" => heavy_computation(data)
```

#### Example — Algebraic Simplification

```v2
macro pattern algebra {
    $x + $x        => 2 * $x ;
    $x - $x        => 0      ;
    $x * $x        => $x ** 2 ;
}
```

Pattern macros compile into grammar rewrite rules applied during the parse phase, before the AST is finalized. They have no runtime cost.

---

## Compile-Time Execution

V2 has a compile-time execution domain. Code marked with `comptime` runs during compilation — before the program starts — and can inspect and rewrite the program being compiled.

This is the most powerful metaprogramming feature in V2. It is distinct from macros: macros do textual/pattern substitution, while `comptime` runs full V2 code with access to the compiler's internal state.

### comptime Blocks

A `comptime { }` block runs its body at compile time. It has no effect at runtime.

```v2
comptime {
    print("this prints during compilation, not at runtime")
}
```

### comptime Functions

A function declared `comptime func` runs at compile time when called from a `comptime` context. It can also be called from `comptime` blocks inside normal code.

```v2
comptime func check_platform() {
    if (ct_platform() != "linux") {
        ct_error("This program only supports Linux")
    }
}

comptime {
    check_platform()
}
```

### static_assert

`static_assert` is a compile-time assertion. If the condition is false, compilation fails with the given message. No runtime code is emitted.

```v2
static_assert(8 == mem_size_of("i64"), "unexpected i64 size")
static_assert(true, "this always passes")

const MAX = 256
static_assert(MAX > 0, "MAX must be positive")
```

`static_assert` can appear at the top level or inside `comptime` blocks.

### Compile-Time Intrinsics

These functions are only available inside `comptime` contexts:

| Function                | Description                                                                                                                                |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `ct_platform()`         | Returns the target platform string (`"linux"`, `"windows"`, `"macos"`)                                                                     |
| `ct_arch()`             | Returns the target architecture string (`"x86_64"`, `"arm64"`)                                                                             |
| `ct_word_exists(name)`  | Returns `true` if a function named `name` is defined                                                                                       |
| `ct_list_funcs()`       | Returns a list of all currently defined function names                                                                                     |
| `ct_get_effects(name)`  | Returns the effect annotation list of a function                                                                                           |
| `ct_unregister(name)`   | Removes a function from the compiled output                                                                                                |
| `ct_emit(code_str)`     | Injects raw V2 source code into the compilation unit                                                                                       |
| `ct_error(msg)`         | Fails compilation with a message                                                                                                           |
| `ct_warn(msg)`          | Emits a compiler warning                                                                                                                   |
| `ct_set_macro_limit(n)` | Set macro expansion depth limit                                                                                                            |
| `ct_get_macro_limit()`  | Get current macro expansion depth limit                                                                                                    |
| `mem_size_of(type_str)` | Returns the ABI byte size of the named type (e.g. `"i64"`, `"f32"`, `"Header"`) — available both in `comptime` and as a builtin at runtime |

### Example — Conditional Compilation

```v2
comptime {
    if (ct_platform() == "windows") {
        ct_emit('func path_sep() { return "\\" }')
    } else {
        ct_emit('func path_sep() { return "/" }')
    }
}

print(path_sep())    // "/" on Linux/macOS, "\" on Windows
```

### Example — Verifying Effect Annotations

```v2
comptime {
    let effects = ct_get_effects("fetch")
    if (!effects.contains("net")) {
        ct_warn("fetch does not declare a net effect")
    }
}
```

### Example — Stripping Debug Functions in Release Builds

```v2
comptime {
    let funcs = ct_list_funcs()
    for (name in funcs) {
        if (name.starts_with("debug_")) {
            ct_unregister(name)
        }
    }
}
```

### Notes

- `comptime` code runs in a separate VM pass before bytecode generation. It cannot call runtime-only functions (e.g. I/O, `time()`, `random()`).
- `comptime func` and regular `func` with the same name are separate — a `comptime func` is not callable at runtime.
- `ct_emit` injects source text that is parsed and compiled as if it appeared at the call site.

### Host Build Phase for Runtime-Dependent Generation

For build logic that must perform runtime operations (network, filesystem, external commands), use a host build script instead of `comptime`.

```toml
[build]
script = "build.v2"
```

`build.v2` runs before parsing/compilation and can emit source files consumed by `comptime` and normal compilation. This keeps `comptime` deterministic while still supporting dynamic code generation pipelines.

### `@cfg` — Compile-Time Feature Flags

Feature flags let you declare optional capabilities in `v2.toml` and conditionally compile code based on whether a flag is enabled. This is the standard mechanism for shipping a library with optional backends, debug instrumentation, or platform-specific paths.

#### Declaring Features in `v2.toml`

```toml
[features]
default  = ["json", "net"]   # enabled by default
json     = []                # no dependencies
net      = []
tls      = ["net"]           # tls requires net
debug    = []
postgres = ["net"]
```

Features listed in `default` are active unless explicitly disabled. Users of your package can opt in or out:

```bash
v2 add mylib --features tls,postgres
v2 add mylib --no-default-features --features json
```

#### Querying Features at Compile Time

Use `ct_feature(name)` inside `comptime` blocks to check whether a feature is active:

```v2
comptime {
    if (ct_feature("tls")) {
        ct_emit('import "std.crypto" as crypto')
    }
}

@cfg(feature = "debug")
func debug_dump(val) {
    print(f"[DEBUG] ${val}")
}
```

The `@cfg` decorator conditionally includes the decorated function only when the feature is active. If the feature is disabled, calling the function is a compile-time error.

```v2
// Conditionally compile different implementations
comptime {
    if (ct_feature("postgres")) {
        ct_emit('
            func db_connect_default() {
                return db_connect("postgres://localhost/app")
            }
        ')
    } else {
        ct_emit('
            func db_connect_default() {
                return db_connect("sqlite://app.db")
            }
        ')
    }
}
```

#### Compile-Time Intrinsic

| Function           | Description                                                                |
| ------------------ | -------------------------------------------------------------------------- |
| `ct_feature(name)` | Returns `true` if the named feature is enabled for the current compilation |

`ct_feature` is only valid inside `comptime` blocks. Using it at runtime is a compile-time error.

### `comptime` with Generics

`comptime` functions can be generic. The type parameter is resolved at compile time, letting you generate different code per instantiated type.

```v2
comptime func assert_numeric<T>() {
    let allowed = ["int", "float", "i8", "i16", "i32", "i64",
                   "u8", "u16", "u32", "u64", "f32", "f64"]
    if (!allowed.contains(str(T))) {
        ct_error(f"Expected a numeric type, got ${T}")
    }
}

// Verified at compile time — ct_error fires if T is not numeric
comptime { assert_numeric<int>() }    // ok
comptime { assert_numeric<str>() }    // compile error: Expected a numeric type, got str
```

You can also use `static_assert` with type intrinsics to enforce bounds on generic parameters:

```v2
comptime func check_size<T>() {
    static_assert(mem_size_of(str(T)) <= 8, f"${T} must be at most 8 bytes")
}
```

`ct_emit` can generate type-specialized code for a set of concrete types, effectively producing manual monomorphization:

```v2
comptime {
    for (T in ["i32", "i64", "f32", "f64"]) {
        ct_emit(f'
            func zero_{T}() -> {T} {{
                return 0 as {T}
            }}
        ')
    }
}

// Now zero_i32(), zero_i64(), zero_f32(), zero_f64() all exist
print(zero_i32())    // 0
print(zero_f64())    // 0.0
```

Note that `comptime func` and ordinary generic functions (`func identity<T>`) are complementary — generics give you runtime-erased polymorphism, while `comptime` generics give you compile-time specialization with zero overhead.

---

## Integer Overflow

By default, integer overflow **panics** at runtime with an `OverflowError`. The behavior can be changed via `v2.toml` or the `--overflow` CLI flag:

| Mode       | Behavior                                             |
| ---------- | ---------------------------------------------------- |
| `panic`    | Runtime panic on overflow **(default)**              |
| `wrap`     | Wraps around (two's complement) — `u8: 255 + 1 == 0` |
| `saturate` | Clamps to min/max — `u8: 255 + 1 == 255`             |

Per-expression override:

```v2
let a: u8 = 200
let b: u8 = 100

let wrapped   = wrap_add(a, b)     // 44   (wrapping)
let saturated = sat_add(a, b)      // 255  (saturating)
let checked   = check_add(a, b)    // Ok(result) or Err("overflow")
```

Unsized `int` is arbitrary-precision and never overflows. Sized types (`u8`, `i32`, etc.) follow the configured mode.

---

## Tail-Call Optimization (TCO)

V2 optimizes tail-recursive calls by default — a function whose last action is a recursive call does not grow the call stack.

```v2
// Safe for any n — TCO eliminates stack growth
func sum_tail(n, acc = 0) {
    if (n == 0) { return acc }
    return sum_tail(n - 1, acc + n)    // tail call — optimized
}

sum_tail(1_000_000)    // no stack overflow
```

A call is a tail call only when it is the **last** operation before `return`:

```v2
return n * factorial(n - 1)    // NOT a tail call — multiply happens after
return factorial(n - 1)        // tail call — TCO applies
```

TCO is on by default. Disable with `--no-tco` (CLI) or `tco = false` in `v2.toml` — useful when you want full call stacks in stack traces.

---

## Warnings System

The V2 compiler emits named warning categories. Warnings do not stop compilation.

### Categories

| Category       | Triggers on                                                  |
| -------------- | ------------------------------------------------------------ |
| `unused`       | Unused variables, imports, functions                         |
| `shadow`       | Variable shadows an outer-scope binding                      |
| `unreachable`  | Code after unconditional `return` / `break`                  |
| `implicit_any` | Variable inferred as `any` type                              |
| `deprecated`   | Calling a function marked `@deprecated`                      |
| `overflow`     | Potential overflow detected statically                       |
| `effects`      | Missing or inconsistent effect annotations                   |
| `exhaustive`   | `match` on an enum or union that does not cover all variants |
| `all`          | All categories                                               |

### Enabling / Suppressing

Via CLI:

```bash
v2 --warn all hello.v2
v2 --no-warn unused hello.v2
```

Via `v2.toml`:

```toml
[compiler]
warn    = ["unused", "shadow", "unreachable"]
no_warn = ["implicit_any"]
```

#### Inline suppression — single line

Place `// @suppress category` at the end of any line to silence that warning for that line only:

```v2
let _x = compute()       // @suppress unused
let v: any = get()       // @suppress implicit_any
```

Multiple categories can be suppressed on the same line, comma-separated:

```v2
let x = legacy_api()     // @suppress unused, deprecated
```

#### Block suppression — range of lines

Wrap a region with `// @suppress-start category` and `// @suppress-end category` to silence a warning across multiple consecutive lines:

```v2
// @suppress-start unused
let a = 1
let b = 2
// @suppress-end unused
```

> These are two **distinct** syntaxes. The inline form (`// @suppress`) targets exactly one line. The block form (`// @suppress-start` / `// @suppress-end`) targets every line between the markers. They can be combined freely, and both accept the same category names listed in the [Warnings System](#warnings-system) table.

---

## Compiler Diagnostics

V2 produces rich, Rust-inspired compiler diagnostics by default. Every error and warning includes the source line, a caret underline highlighting the problematic span, an error code, and (where possible) a fix suggestion.

### Diagnostic Format

```
error[E0421]: type mismatch in assignment
  --> src/main.v2:14:9
   |
12 |     let name: str = "Alice"
13 |     let age: int = 30
14 |     let score: int = "high"
   |                      ^^^^^^ expected `int`, found `str`
   |
   = help: if you meant to parse the string as a number, use `int("high")`
```

Every diagnostic has five parts:

1. **Severity and code** — `error[E0421]`, `warning[W0012]`, or `note`
2. **Summary line** — one-sentence description of the problem
3. **Location** — file path, line, and column (`-->`)
4. **Source span** — the relevant lines with ASCII caret underlines (`^^^`)
5. **Suggestions** — `help:` lines with concrete fixes, `note:` lines with background context

### Multi-Span Diagnostics

When an error involves multiple locations, the compiler shows all of them:

```
error[E0502]: cannot borrow `items` as mutable because it is also borrowed as immutable
  --> src/data.v2:19:5
   |
17 |     let r = &items
   |             ------ immutable borrow occurs here
18 |     process(r)
19 |     items.push(42)
   |     ^^^^^ mutable borrow occurs here
20 |     print(r)
   |           - immutable borrow later used here
   |
   = note: a value cannot be mutably borrowed while any immutable borrow is still live
```

### "Did You Mean?" Suggestions

The compiler uses Levenshtein distance to suggest corrections for misspelled identifiers:

```
error[E0425]: cannot find value `prntln` in this scope
  --> src/main.v2:5:5
   |
 5 |     prntln("hello")
   |     ^^^^^^ not found in this scope
   |
   = help: did you mean `println`?
```

This works for variable names, function names, type names, trait names, module names, and field names:

```
error[E0609]: no field `naem` on type `User`
  --> src/main.v2:8:10
   |
 8 |     user.naem
   |          ^^^^ unknown field
   |
   = help: did you mean `name`?
   = help: available fields are: `name`, `age`, `email`
```

### Warning Format

Warnings follow the same format:

```
warning[W0001]: unused variable `x`
  --> src/main.v2:3:9
   |
 3 |     let x = compute()
   |         ^ this variable is never read
   |
   = help: prefix with `_` to silence: `let _x = compute()`
```

### Error Codes

Every diagnostic has a stable error code. Use `--explain` to get a detailed explanation:

```bash
v2 --explain E0421
```

Output:

```
E0421: type mismatch in assignment

A value was assigned to a variable or passed to a function where the
expected type does not match the actual type.

Example of erroneous code:

    let x: int = "hello"    // E0421: expected int, found str

The fix depends on context — either change the type annotation, convert
the value, or fix the expression producing the wrong type.
```

Error code ranges:

| Range       | Category                  |
| ----------- | ------------------------- |
| E0001–E0099 | Syntax errors             |
| E0100–E0199 | Name resolution           |
| E0200–E0299 | Type system               |
| E0300–E0399 | Pattern matching          |
| E0400–E0499 | Type checking / inference |
| E0500–E0599 | Borrow checker            |
| E0600–E0699 | Field / method resolution |
| E0700–E0799 | Lifetime / ownership      |
| W0001–W0099 | Unused code               |
| W0100–W0199 | Shadowing / style         |
| W0200–W0299 | Potential logic errors    |

### Output Modes

Control the diagnostic format with CLI flags:

```bash
v2 build src/main.v2                     # default — human-readable with colors
v2 build src/main.v2 --color=never       # plain text (no ANSI escapes)
v2 build src/main.v2 --error-format=json # machine-readable JSON
v2 build src/main.v2 --error-format=sarif # SARIF format for CI/CD integration
```

#### JSON Output

```json
{
  "level": "error",
  "code": "E0421",
  "message": "type mismatch in assignment",
  "spans": [
    {
      "file": "src/main.v2",
      "line_start": 14,
      "line_end": 14,
      "col_start": 19,
      "col_end": 25,
      "label": "expected `int`, found `str`"
    }
  ],
  "help": ["if you meant to parse the string as a number, use `int(\"high\")`"]
}
```

### `v2.toml` Configuration

```toml
[diagnostics]
color = "auto"            # "auto" | "always" | "never"
error_format = "human"    # "human" | "json" | "sarif"
max_errors = 50           # stop after this many errors (0 = unlimited)
```

### Diagnostic API for Tooling

The compiler exposes diagnostics programmatically via `v2 check --emit=json`, enabling editors and IDE plugins to consume structured error data. Each diagnostic includes byte offsets and UTF-8 column positions for precise overlay rendering.

---

## Source Directives

Source directives are processed before parsing begins, operating on raw source text rather than on the AST or compiled code. They begin with `@` at the start of a line.

### @replace — Source-Level Text Substitution

`@replace` performs a global find-and-replace across all source code that follows it in the same file. Both plain strings and regular expressions are supported.

#### Plain Substitution

```v2
@replace "debug" -> "release"

// All occurrences of the word "debug" in the rest of this file become "release"
```

#### Regex Substitution

```v2
@replace /[0-9]+/ -> "NUM"

print(42)      // becomes: print(NUM)
let x = 100    // becomes: let x = NUM
```

#### Multiple Directives

Multiple `@replace` directives stack — each one is applied in declaration order:

```v2
@replace "foo" -> "bar"
@replace "bar" -> "baz"

print("foo")    // prints: baz
```

#### Identifier Aliasing

A useful pattern is aliasing keywords or names to match a preferred style:

```v2
@replace "fn" -> "func"
@replace "nil" -> "null"

fn greet(name) {
    if (name == nil) { return }
    print(f"Hello, ${name}!")
}
```

#### Notes

- `@replace` applies to the entire remainder of the file from the point it is declared — it does not affect files already imported above it.
- Substitution happens before tokenization, so it can rename keywords, identifiers, string contents, and anything else.
- Use with care: broad replacements can break unintended parts of the source. Prefer regex anchors (`\b`) to match whole words when doing identifier substitution.
- `@replace` directives in imported modules do not bleed into the importing file.

### @insert — File Inclusion

`@insert` splices the contents of another file directly into the current source at the point of declaration, before parsing.

```v2
@insert "shared/constants.v2"
@insert "generated/bindings.v2"
```

This is a textual inclusion — the inserted file's source is treated as if it were written inline. Unlike `import`, `@insert` does not create a module boundary, does not apply namespacing, and does not deduplicate (inserting the same file twice inserts it twice).

### @borrow_check — Enable Borrow Checker

`@borrow_check` opts a file (or a single function) into static borrow checking. It is a compile-time directive processed before parsing and has no runtime cost.

Place it at the top of a file to borrow-check the entire file:

```v2
@borrow_check

func process_buffer(buf) {
    let r = &buf         // immutable borrow
    let val = *r
    return val
}
```

Place it directly above a single function declaration to scope it to only that function:

```v2
@borrow_check
func critical_path(data) {
    let owned = move data
    // borrow rules enforced here
    return owned
}

func relaxed_path(data) {
    // GC-managed as normal — no borrow rules
}
```

See [Memory Safety and Borrowing](#memory-safety-and-borrowing) for the full ownership and borrowing reference.

### @cfg — Conditional Compilation

`@cfg` conditionally includes or excludes a block of source code based on compile-time configuration flags. It is processed before parsing and has no runtime cost.

```v2
@cfg(target = "wasm")
func platform_info() -> str {
    return "running in WebAssembly"
}

@cfg(target = "native")
func platform_info() -> str {
    return "running natively"
}
```

`@cfg` can also wrap arbitrary blocks, not just functions:

```v2
@cfg(debug = true) {
    print("Debug mode active")
    log.set_level("DEBUG")
}
```

Configuration flags are resolved from the compiler target, `v2.toml` settings, and `--cfg` CLI flags:

```bash
v2 --cfg debug=true app.v2
v2 --cfg feature_x=true app.v2
```

Or in `v2.toml`:

```toml
[compiler]
cfg = { debug = true, feature_x = false }
```

**Available built-in cfg keys:**

| Key        | Values                                      | Source                    |
| ---------- | ------------------------------------------- | ------------------------- |
| `target`   | `"native"`, `"wasm"`, `"bytecode"`          | `--target` / `v2.toml`    |
| `os`       | `"linux"`, `"windows"`, `"macos"`, `"none"` | `--os` / host detection   |
| `arch`     | `"x86_64"`, `"arm64"`, `"wasm32"`           | `--arch` / host detection |
| `debug`    | `true` / `false`                            | `--cfg debug=true`        |
| `optimize` | `true` / `false`                            | `[compiler] optimize`     |

Custom keys can be freely defined via `--cfg` or `[compiler] cfg` and used in any `@cfg` expression.

**Logical combinations:**

```v2
@cfg(os = "linux" && arch = "x86_64") {
    // x86_64 Linux only
}

@cfg(target = "wasm" || target = "bytecode") {
    // sandboxed environments
}

@cfg(!debug) {
    // release-only code
}
```

---

## Modules & Imports

V2 uses a single unified `import` keyword for all module, library, path, and hierarchy resolution. There is no separate `use` keyword.

### Basic Import

```v2
import "path/to/module"
import { func1, func2 } from "module"
import "module" as alias
import "module" as _            // disable namespace wrapping for this import
```

### Import Aliases

When you import a module with `as name`, all exported symbols from that module are prefixed with `name.` in your scope. This prevents name collisions and makes the origin of each symbol clear.

```v2
import "std.math" as m
import "std.crypto" as crypto

let x = m.sqrt(16.0)           // 4.0
let h = crypto.sha256("hello") // hash string
```

You can combine alias imports with selective imports:

```v2
import { sqrt, pow } from "std.math" as m    // imports as m.sqrt, m.pow
```

Using `as _` disables automatic namespace prefixing, importing all symbols directly into the current scope:

```v2
import "std.math" as _    // sqrt, pow, etc. available without prefix
```

### Dual Calling Convention — Qualified and Unqualified

By default, importing a module makes its symbols available **both** with the module prefix (qualified) and without it (unqualified). Both forms work interchangeably:

```v2
import "std.math"

let a = math.sqrt(16.0)    // qualified — explicit module origin
let b = sqrt(16.0)         // unqualified — same function, shorter syntax
// a == b == 4.0
```

This applies to all imported symbols — functions, types, constants:

```v2
import "std.collections"

let q1 = collections.Deque()    // qualified
let q2 = Deque()                // unqualified — same type
```

When two modules export the same name, the **unqualified** form becomes ambiguous — the compiler raises a `NameConflictError` at import time. The qualified forms still work:

```v2
import "std.math"       // exports sqrt
import "my_lib"         // also exports sqrt

// sqrt(4.0)            // ERROR: ambiguous — `sqrt` exists in both `math` and `my_lib`
math.sqrt(4.0)          // OK — qualified, no ambiguity
my_lib.sqrt(4.0)        // OK — qualified, no ambiguity
```

To resolve the conflict and make one of them available unqualified, rename the other:

```v2
import "std.math"
import "my_lib"

rename(my_lib.sqrt, my_sqrt)    // free the unqualified `sqrt` for std.math

sqrt(4.0)            // OK — now unambiguously std.math.sqrt
my_sqrt(4.0)         // OK — the renamed my_lib version
```

#### Import with Alias — Qualified Only

When importing with `as name`, symbols are available **only** under the alias prefix:

```v2
import "std.math" as m

m.sqrt(16.0)        // OK — qualified under alias
// sqrt(16.0)       // ERROR — not available unqualified with aliased imports
// math.sqrt(16.0)  // ERROR — original module name not bound either
```

#### Summary Table

| Import Style                 | `mod.func()`   | `func()` |
| ---------------------------- | -------------- | -------- |
| `import "mod"`               | Yes            | Yes      |
| `import "mod" as alias`      | `alias.func()` | No       |
| `import "mod" as _`          | No             | Yes      |
| `import { func } from "mod"` | `mod.func()`   | Yes      |

### URL Imports

```v2
import "https://example.com/mod.v2"
```

For offline/native-limited environments, register source text first:

```v2
http_import_register("https://example.com/mod.v2", "let X = 42")
import "https://example.com/mod.v2"
```

### Hierarchical Path Imports

All crate-style and hierarchy-style imports use `import` with path notation:

```v2
import { std.io, std.math }           // import multiple stdlib modules

import crate::utils::helper           // import from crate root
import super::common::Config          // import from parent module
import self::local_item               // import from current module
import crate::prelude::*              // glob import
```

### Module Declaration

```v2
mod utils;                    // loads from utils.v2 or utils/mod.v2

mod helpers {                 // inline module
    pub func format(s) {
        return f"[${s}]"
    }
}
```

### Visibility

```v2
pub func public_api() { ... }         // accessible everywhere
private func internal() { ... }       // file-private
internal func module_only() { ... }   // module-private

pub(crate) func crate_only() { ... }  // crate-level visibility
pub(super) const CONFIG = { ... }     // parent module visibility
```

### on_import Hook

A module can define a special function named `on_import`. If present, it is called automatically the moment the module is imported, before any other code in the importing file runs. Use it for module initialization, registering handlers, or printing load-time diagnostics.

```v2
// mylib.v2
func on_import() {
    print("mylib loaded")
    register_engine("/usr/local/bin/mylang", "mylang")
}

pub func greet(name) {
    print(f"Hello, ${name}!")
}
```

```v2
// main.v2
import "mylib"     // prints "mylib loaded" immediately

mylib.greet("Alice")
```

`on_import` is not exported as part of the module's public API. It cannot be called manually.

### `on_import` Error Handling

If a module's `on_import()` function throws an unhandled error, the import fails immediately and the error propagates to the importing file as a thrown exception. The module is considered unloaded — subsequent `import` statements for the same module will attempt to load it again.

```v2
// fragile_module.v2
func on_import() {
    let conn = db_connect("postgres://...")    // might throw if DB is down
    register_engine("/usr/local/bin/myengine", "myengine")
}
```

```v2
// main.v2
try {
    import "fragile_module"    // on_import fires here
} catch (e) {
    print(f"Module failed to load: ${e.message}")
    // fragile_module is NOT available — its symbols are not in scope
}
```

If `on_import` succeeds, the module's exported symbols become available in the importing file's scope immediately after the `import` statement.

`on_import` cannot be called manually — it is reserved for the module loading system. Attempting to call it as a regular function is a compile-time error.

### cimport — C Header Import

`cimport` reads a C header file and automatically generates `extern` declarations and `cstruct` definitions for everything it finds. This eliminates the need to manually transcribe C APIs.

```v2
cimport "my_header.h"
cimport "/usr/include/stdio.h"
```

After `cimport`, all functions, constants, and struct types declared in the header are available directly:

```v2
cimport "unistd.h"
cimport "sys/socket.h"

let fd = open("file.txt", O_RDONLY)
let sock = socket(AF_INET, SOCK_STREAM, 0)
```

`cimport` runs at compile time. It uses the system C preprocessor to resolve `#include` chains and `#define` constants.

### Enable Engines

```v2
enable { py, js, c, malbolge }       // enable embedded language engines
```

### Extern (FFI)

```v2
extern c puts
extern c printf
extern c strlen

// C-style typed declarations also supported:
extern c int strlen(str s)
extern c void exit(int code)
extern c double atan2(double y, double x)

puts("hello")
print(strlen("abc"))
```

`extern c` declarations are safe by default. Calling a typed extern is also safe when arguments stay in safe V2 value space; wrap usage in `unsafe` when doing raw-pointer interop, manual ABI casting, or other operations the compiler cannot verify.

Variadic C functions are supported with `...`:

```v2
extern c int printf(str fmt, ...)
```

Block-style extern for multiple bindings at once:

```v2
extern c {
    malloc
    free
    puts
}
```

Wildcard — bind all symbols from a shared library on first use:

```v2
extern "libc.so.6" *
```

The runtime provides default bindings for `c.puts`, `c.printf`, and `c.strlen`. Unknown externs raise an `Unresolved extern` runtime error.

---

## Module Visibility — `pub(crate)` and `pub(super)`

V2's visibility system has four levels. The basic three (`pub`, `private`, `internal`) are covered in the main Modules section. The fine-grained modifiers `pub(crate)` and `pub(super)` offer more precise control in multi-module projects.

### What Is a "Crate"?

In V2, a **crate** is the unit of compilation declared by a `v2.toml` file — equivalently, a project or package. All `.v2` files under the same `v2.toml` belong to the same crate. When you `v2 publish`, you publish a crate. When you `v2 add http-utils`, you add a crate dependency.

A crate is not a directory — it is a compilation unit. Multiple source files in subdirectories all belong to the same crate as long as they share a root `v2.toml`.

### Visibility Levels — Complete Reference

| Modifier     | Accessible from                                               |
| ------------ | ------------------------------------------------------------- |
| `pub`        | Anywhere — including other crates that import this one        |
| `pub(crate)` | Anywhere within the same crate — but not from external crates |
| `pub(super)` | The immediate parent module only                              |
| `internal`   | The current module (file or `mod` block) and its sub-modules  |
| `private`    | The current file only                                         |

### `pub(crate)` — Crate-Internal API

Use `pub(crate)` to expose something across your own codebase without making it part of your public API:

```v2
// src/db/pool.v2
pub(crate) func acquire_connection() {
    // Used freely by other modules in this crate,
    // but not exported to crate consumers
}
```

```v2
// src/server/handler.v2
import "src/db/pool"

pool.acquire_connection()    // OK — same crate
```

```v2
// external_lib/main.v2
import "myapp"

myapp.acquire_connection()   // ERROR — pub(crate), not accessible outside
```

### `pub(super)` — Parent Module Only

Use `pub(super)` to expose something to the direct parent module, but not to the broader codebase. This is useful for implementation details that a parent module needs to orchestrate but that should not leak further up:

```v2
// src/auth/tokens.v2  (child module of src/auth)
pub(super) func generate_token(user_id: int) -> str {
    // accessible only from src/auth — not from src/ or root
}
```

```v2
// src/auth/mod.v2  (parent of tokens.v2)
import "src/auth/tokens"

pub func login(user) {
    let token = tokens.generate_token(user.id)    // OK — parent can access pub(super)
    return token
}
```

```v2
// src/main.v2
import "src/auth/tokens"

tokens.generate_token(1)    // ERROR — not visible outside auth module
```

### Module Hierarchy and `crate::`, `super::`, `self::`

Path prefixes work together with visibility modifiers:

```v2
import crate::utils::Config        // absolute path from crate root
import super::shared::helpers      // relative to parent module
import self::local_helper          // within the current module
```

These path prefixes are for import resolution — they do not change visibility. Visibility is set by the `pub(...)` modifier on the declaration, not on the import.

### Typical Project Structure

```
myapp/
+-- v2.toml
+-- src/
    +-- main.v2           // crate root
    +-- config.v2         // pub(crate) items here: visible project-wide, not exported
    +-- auth/
    —   +-- mod.v2        // pub items here: part of the public API
    —   +-- tokens.v2     // pub(super) items here: visible to auth/mod.v2 only
    +-- db/
        +-- mod.v2
        +-- pool.v2       // pub(crate) items: usable anywhere in myapp, not exported
```

---

## Embedded Language Engines

V2 can embed code from other programming languages using `@lang { ... }` blocks.

### Supported Languages

| Tags                 | Language   | Requires                  |
| -------------------- | ---------- | ------------------------- |
| `@py`, `@python`     | Python     | `python`                  |
| `@js`, `@javascript` | JavaScript | `node`                    |
| `@lua`               | Lua        | `lua`                     |
| `@go`                | Go         | `go`                      |
| `@c`                 | C          | `gcc`                     |
| `@cpp`, `@cxx`       | C++        | `g++`                     |
| `@ts`, `@typescript` | TypeScript | `npx ts-node`             |
| `@java`              | Java       | `javac` + `java`          |
| `@cs`, `@csharp`     | C#         | `dotnet-script`           |
| `@rust`, `@rs`       | Rust       | `rustc`                   |
| `@bash`, `@sh`       | Bash       | `bash`                    |
| `@ps`, `@powershell` | PowerShell | `powershell`              |
| `@os`, `@shell`      | OS Shell   | (system shell)            |
| `@asm`, `@assembly`  | Assembly   | `nasm` + `ld`             |
| `@mal`, `@malbolge`  | Malbolge   | runtime-simulated backend |

### Managed Engine Bundles

When reproducibility is required, configure embedded language toolchains in `v2.toml` and install them with `v2 install --engines`.

```toml
[engines]
policy = "managed"
python = "3.12"
node   = "20"
rust   = "1.80"
```

Resolution order:

1. Managed runtime matching the pinned version.
2. System runtime (only when `policy = "mixed"`).
3. Compile error with actionable install message.

This removes hidden host dependency drift in CI and production builds.

### Usage

```v2
enable { py, js }

@py {
    import math
    print(f"Pi = {math.pi}")   # Python syntax inside @py block
}

@js {
    const items = [1, 2, 3];
    console.log(items.map(x => x * 2));
}

@bash {
    echo "Hello from Bash"
    ls -la
}

@mal {
    (=<`:9876Z4321UT.-Q+*)M'
}
```

### Named Engine Blocks

Engine blocks support two forms:

- unnamed block: `@py { ... }`
- named block: `@py calculations { ... }`

Unnamed blocks are fully valid: they execute, can export symbols, and participate in language-wide imports such as `from @py`.

```v2
@py {
    def square(x):
        return x * x
}

@py calculations {
    def sub(x, y):
        return x - y
}

@py stats{
    from statistics import mean
}
```

Both `@py name { ... }` and `@py name{ ... }` are valid forms.

### Cross-Language Interop

V2 uses directive syntax with explicit source selectors:

```v2
@py {
    def square(x):
        return x * x

    def sub(x, y):
        return x - y

    @export { square, sub }
}

@py stats{
    from statistics import mean
    @export { mean }
}

@import { square, mean as mean_from_py_blocks } from @py
@import { mean as mean_from_stats_name } from stats
@import { mean as mean_exact } from @py.stats
@import { mean as py_mean } from py.statistics

@js {
    @import { square } from @py
    console.log(square(5));    // 25
}
```

These directives are parsed by V2 before embedded source is sent to the host engine. This avoids comment-style conflicts across languages (for example, Python does not use `//` comments).

- `@export { a, b, c }` — export one or more symbols from the current embedded block
- `@export { * }` — export all callable symbols from the current embedded block
- `@import { a, b as alias } from @lang` — import from all blocks of a language tag in the current file
- `@import { a, b } from block_name` — import from a specific named block
- `@import { a, b } from @lang.block_name` — import from one exact language + block pair
- `@import { a, b } from py.module` — import from an external Python module/library (for example `py.math_ops`, `py.statistics`)
- `@import { * } from selector` — import all exported symbols from any selector form
- Compiled results are cached using DJB2 hashing to avoid re-compilation

Source selector rules:

- `@py`, `@js`, etc. mean the union of exports from all blocks with that language tag in the current `.v2` file.
- bare names (for example `calculations`) resolve to a named engine block in the current `.v2` file.
- `@lang.name` is the most specific selector and should be used when names overlap.
- unnamed blocks are imported through `@lang` (for example `@py`) and cannot be targeted by bare-name selectors because they have no block identifier.
- if a symbol appears in multiple blocks and you import from `@lang`, the import is ambiguous and fails at compile time; use `name` or `@lang.name`.

Wildcard behavior:

- a block exported with `@export { * }` may still be consumed selectively (for example `@import { sub } from @py.calculations`).
- wildcard import and selective import can be mixed in the same file.

### Canonical Pattern: Optional Named Blocks, Python Modules, and Cross-Engine Reuse

When you want to reuse Python functions from local files or installed libraries in V2, combine module imports with optional block naming.

1. Import from Python modules into V2 with `@import { ... } from py.module`.
2. Use `@py { ... }` for quick unnamed bridges, or `@py block_name{ ... }` when you need exact targeting.
3. Import from all blocks with `@py`, or target exact named blocks with `name` / `@py.name`.
4. Reuse exported symbols in other engines with the same selector forms.

> Use `@import { ... } from py.module` rather than V2 module imports like `import "calc.py"`.

#### `math_ops.py`

```python
def bonus(x: int) -> int:
    return x * 3 + 7
```

#### `main.v2`

```v2
enable { py, js }

@import { bonus } from py.math_ops
@import { mean } from py.statistics

let a = bonus(5)
print(a)
print(mean([10, 20, 30]))

@py {
    from math_ops import bonus

    def py_bonus(x):
        return bonus(x)

    @export { py_bonus }
}

@py calculations{
    def py_square(x):
        return x * x

    @export { py_square }
}

@import { py_bonus } from @py
@import { py_square } from calculations
@import { py_square as py_square_exact } from @py.calculations

@js {
    @import { py_bonus, py_square } from @py

    function js_bonus_twice(x) {
        return py_bonus(x) * 2;
    }

    @export { js_bonus_twice }
}

@js reports{
    @import { py_bonus } from @py
    @import { py_square } from @py.calculations

    function js_combo(x) {
        return py_bonus(x) + py_square(x);
    }

    @export { js_combo }
}

@import { js_bonus_twice } from @js
@import { js_combo } from reports
@import { js_combo as js_combo_exact } from @js.reports

let b = js_bonus_twice(5)
print(b)
print(js_combo(5))
```

This single pattern covers all three interop directions:

- imported-into-language-engine: `@import { ... } from py.module`
- exported-from-language-engine: `@export { ... }` or `@export { * }` inside `@py` / `@js` / custom engine blocks
- cross-engine imports/exports: one engine imports from another and re-exports to V2 or peers

If a Python module is outside the working directory, use package imports or set `sys.path` inside `@py` before importing.

---

## Custom Language Engines

V2 allows you to register your own embedded language engines at runtime, in addition to the built-in ones. This lets you write V2 code that embeds any language whose interpreter or compiler you have a path to.

### Registering a Custom Engine

```v2
register_engine(path, name)
```

| Argument | Type  | Description                                                                          |
| -------- | ----- | ------------------------------------------------------------------------------------ |
| `path`   | `str` | Absolute or relative path to the executable that runs the language                   |
| `name`   | `str` | The tag name used to invoke the engine in `@name { }` or `@name block_id { }` blocks |

### Example

```v2
// Register a custom engine called "mylang" backed by an executable
register_engine("/usr/local/bin/mylang-run", "mylang")

// Now enable it like any other engine
enable { mylang }

// Use it in an embedded block
@mylang {
    say "Hello from MyLang!"
}
```

### Cross-Language Interop with Custom Engines

Custom engines support the same `@export { ... }` / `@import { ... } from ...` interop as built-in engines:

```v2
register_engine("/usr/local/bin/mylang-run", "mylang")
enable { mylang }

@mylang tools{
    @export { version }

    func version() => "1.0.0"
}

@mylang {
    @export { greet }

    func greet(name) => "Hello, " + name
}

@import { greet } from @mylang
@import { version } from tools
@import { version as version_exact } from @mylang.tools

let msg = greet("World")
print(msg)    // Hello, World
```

### Notes

- The executable at `path` receives the embedded block's source code via stdin and is expected to write output to stdout.
- Custom engines are registered per-program run; they are not persisted globally.
- If `path` does not point to a valid executable, a `EngineNotFound` runtime error is raised.
- Engine names must be unique. Registering a name that conflicts with a built-in engine raises a `EngineNameConflict` error.

---

## Inline Assembly

V2 supports inline assembly directly within V2 code using the `asm!` expression. This is distinct from the `@asm { }` embedded engine block (which shells out to an external assembler). Inline assembly is woven directly into the compiled output and has access to V2's local variable bindings.

### Syntax

```v2
unsafe {
    asm! {
        ; assembly instructions here
        ; use %varname to reference V2 local variables
    }
}
```

Use `asm!` as an expression to capture a return value:

```v2
let result = unsafe {
    asm! {
        mov eax, %x
        add eax, %y
        ; return value is whatever is in eax / rax
    }
}
```

### Example — Adding Two Integers

```v2
let x = 10
let y = 32

let sum = unsafe {
    asm! {
        mov eax, %x
        add eax, %y
    }
}

print(sum)    // 42
```

### Example — Direct System Call

```v2
unsafe {
    asm! {
        mov eax, 1          ; sys_write
        mov ebx, 1          ; stdout
        mov ecx, msg        ; message pointer
        mov edx, 13         ; length
        int 0x80
    }
}
```

### Constraints

- `%varname` binds a V2 local variable as an input operand.
- The return value of an `asm!` expression is the value left in `eax` (x86) or `rax` (x86-64) after execution.
- Inline assembly is only available when compiling with `-c` (bytecode compilation targeting native); it is silently ignored in interpreter mode unless `-d` debug mode is active, which will warn.
- Clobbered registers beyond `eax`/`rax` must be saved/restored manually.
- Inline assembly blocks are architecture-specific. V2 does not abstract over ISA differences.

### Portability with `@cfg`

Guard architecture-specific assembly and provide a non-assembly fallback:

```v2
@cfg(arch = "x86_64")
func checksum_fast(ptr, len) {
    return unsafe {
        asm! {
            ; x86_64-specific instructions
        }
    }
}

@cfg(!(arch = "x86_64"))
func checksum_fast(ptr, len) {
    return checksum_portable(ptr, len)
}
```

---

## Actors & Agents

### Actors

Message-passing concurrency model:

```v2
actor Counter {
    let count = 0

    func increment() {
        count += 1
    }

    func get_count() {
        return count
    }
}

let c = actor_spawn("Counter", {})
actor_send(c, "increment")
let msg = actor_receive(c)
```

### Actor API

| Function                     | Description                                                                    |
| ---------------------------- | ------------------------------------------------------------------------------ |
| `actor_spawn(name, opts?)`   | Spawn an actor instance by name ? actor handle                                 |
| `actor_send(handle, msg)`    | Send a message (any value) to the actor's mailbox (non-blocking)               |
| `actor_receive(handle)`      | Receive the next message from the actor's outbox (blocks until available)      |
| `actor_call(handle, msg)`    | Send a message and block until the actor sends a reply — request/reply pattern |
| `actor_stop(handle)`         | Gracefully stop the actor after it finishes processing its current message     |
| `actor_kill(handle)`         | Immediately terminate the actor                                                |
| `actor_is_alive(handle)`     | Returns `true` if the actor is still running                                   |
| `actor_mailbox_size(handle)` | Number of messages currently queued in the actor's inbox                       |

> `actor_spawn(name, opts?)` resolves by exact actor declaration identifier (case-sensitive). Use the declared casing (for example `"Counter"`) to avoid runtime lookup failures.

```v2
// Request/reply with actor_call
actor Math {
    func double(x) {
        return x * 2
    }
}

let m = actor_spawn("Math", {})
let result = actor_call(m, {"fn": "double", "args": [21]})
print(result)    // 42

// Graceful shutdown
actor_stop(m)
print(actor_is_alive(m))    // false
```

> **`actor_call` message format:** The `{"fn": "...", "args": [...]}` dict format used above is a **runtime convention**, not an enforced schema. `actor_call` passes any value to the actor's mailbox — the actor's internal dispatch logic determines how to interpret it. The `{"fn", "args"}` shape is the idiomatic standard for request/reply actors and is what the runtime uses when it auto-generates actor dispatch from method definitions. You may use any message format your actor handles, including plain strings, integers, or custom structs.

**Error handling:** If an actor throws an unhandled error, it terminates and subsequent `actor_send` / `actor_call` calls raise an `ActorDeadError`. Use `try` / `catch` around actor calls to handle this.

### Agents

Agents are goal-oriented autonomous entities. Unlike actors (which react to explicit messages), agents work toward a declared `goal` by repeatedly calling their `plan()` function until the goal is considered complete. They are useful for AI-style task planning, background optimization loops, and autonomous subsystems.

```v2
agent Planner {
    goal "find optimal path"

    func plan(self) {
        // planning logic — called repeatedly until goal is met
    }
}

let ag = agent_create("planner", {})
agent_set_goal(ag, "target", "destination")
let state = agent_get_state(ag)
```

### Agent API

| Function                          | Description                                                 |
| --------------------------------- | ----------------------------------------------------------- |
| `agent_create(name, opts)`        | Spawn a new agent instance                                  |
| `agent_set_goal(ag, key, value)`  | Set a goal parameter (merged into the agent's goal context) |
| `agent_get_goal(ag, key)`         | Read a goal parameter                                       |
| `agent_get_state(ag)`             | Return the agent's current internal state dict              |
| `agent_set_state(ag, key, value)` | Write into the agent's state dict                           |
| `agent_step(ag)`                  | Run one planning cycle manually                             |
| `agent_run(ag)`                   | Run planning cycles automatically until goal is met         |
| `agent_done(self)`                | Mark goal completion from inside the agent's `plan()`       |
| `agent_is_done(ag)`               | Returns `true` if the agent considers its goal achieved     |
| `agent_stop(ag)`                  | Stop a running agent                                        |
| `agent_send(ag, msg)`             | Send a message dict to the agent's `on_message` handler     |

### Declaring Goal Completion

Inside `plan()`, call `agent_done(self)` to signal the goal is achieved. After that, `agent_is_done` returns `true` and `agent_run` exits.

```v2
agent Crawler {
    goal "index all pages"

    func plan(self) {
        let state = agent_get_state(self)
        let queue = state["queue"]

        if (queue.len() == 0) {
            agent_done(self)
            return
        }

        let url = queue.pop()
        let links = crawl_page(url)
        for (link in links) {
            if (!state["visited"].contains(link)) {
                queue.push(link)
                state["visited"].push(link)
            }
        }
        agent_set_state(self, "queue", queue)
        agent_set_state(self, "visited", state["visited"])
    }

    func on_message(self, msg) {
        if (msg["type"] == "add_url") {
            let q = agent_get_state(self)["queue"]
            q.push(msg["url"])
            agent_set_state(self, "queue", q)
        }
    }
}

let crawler = agent_create("crawler", {
    "queue": ["https://example.com"],
    "visited": []
})

agent_run(crawler)
print(agent_get_state(crawler)["visited"])
```

### Agents vs Actors

|            | Actor                           | Agent                                     |
| ---------- | ------------------------------- | ----------------------------------------- |
| Driven by  | Incoming messages               | Internal goal + plan loop                 |
| State      | Encapsulated, message-protected | Dict accessible via `agent_get/set_state` |
| Completion | Never (runs until stopped)      | When `agent_done()` is called             |
| Best for   | Event-driven concurrency        | Autonomous background tasks               |

---

## Isolates

An isolate is a sandboxed, independent V2 interpreter instance. Code run inside an isolate has its own separate variable stack and cannot read or write variables in the parent program. Isolates are useful for sandboxing untrusted code, running experiments without side effects, and building plugin systems.

### Running Code in a New Isolate

```v2
isolate {
    let x = 10
    print(x)    // 10 — runs in the isolate's own scope
}

print(x)    // error: x does not exist here
```

### Isolate as an Expression

An `isolate` block can return a value. The value of the last expression (or an explicit `return`) in the isolate becomes the result:

```v2
let result = isolate {
    let a = 5
    let b = 6
    return a + b
}

print(result)    // 11 — a and b never existed in the outer scope
```

### Named / Reusable Isolates

Create a named isolate that persists its internal state between calls using `isolate_new()`. You can then run code inside it repeatedly, and later read values back out of it.

```v2
let iso = isolate_new()

isolate(iso) {
    let counter = 0
}

isolate(iso) {
    counter += 1
    counter += 1
}

let val = isolate_get(iso, "counter")
print(val)    // 2 — the isolate's internal state persisted
```

### Isolate API

| Function / Syntax                   | Description                                          |
| ----------------------------------- | ---------------------------------------------------- |
| `isolate { ... }`                   | Run a block in a fresh, one-shot isolate             |
| `isolate(iso) { ... }`              | Run a block in an existing named isolate `iso`       |
| `let x = isolate { ... }`           | Run in a fresh isolate and capture the return value  |
| `let x = isolate(iso) { ... }`      | Run in a named isolate and capture the return value  |
| `isolate_new()`                     | Create and return a new named isolate                |
| `isolate_get(iso, "name")`          | Read a variable from an isolate's internal scope     |
| `isolate_set(iso, "name", val)`     | Write a variable into an isolate's internal scope    |
| `isolate_exec(code_str, opts?)`     | Run a V2 source string in a one-shot sandbox isolate |
| `isolate_run(iso, code_str, opts?)` | Run a V2 source string inside an existing isolate    |
| `isolate_drop(iso)`                 | Destroy an isolate and free its resources            |

Use `isolate_exec` for one-shot sandboxed execution. Use `isolate_run` when reusing a named isolate created with `isolate_new()`.

### Example — Sandboxed Plugin

```v2
let plugin = isolate_new()
isolate_run(plugin, read_file("plugin.v2"))

let output = isolate_get(plugin, "result")
print(output)
```

### Notes

- Isolates share no memory with the parent. Values passed in or out must be serializable (primitives, lists, dicts, strings).
- An isolate cannot import modules that have side effects on global state unless those modules are loaded fresh within the isolate itself.
- `isolate_get` returns `null` if the named variable does not exist in the isolate.

---

## Memory Safety and Borrowing

V2 manages memory automatically by default using a garbage collector. The borrow checker is an **opt-in** static analysis layer that gives you Rust-style ownership guarantees on top — useful for systems code, performance-critical paths, or any code where you want the compiler to prove the absence of data races and use-after-free bugs.

### Opting In

Enable the borrow checker for a file via a source directive at the top:

```v2
@borrow_check
```

Or enable it for a single function:

```v2
@borrow_check
func process_buffer(buf) {
    // borrow rules enforced inside this function
}
```

Or project-wide in `v2.toml`:

```toml
[compiler]
borrow_check = true
```

When borrow checking is **off** (the default), the GC handles all memory automatically — you never call `mem_free` or think about ownership. When borrow checking is **on**, the compiler enforces ownership rules and the GC is bypassed for values under borrow-check scope.

#### GC / Borrow-Check Boundary

When borrow-checked code calls GC-managed code (or vice versa), the following rules apply at the boundary:

- **Borrow-checked ? GC-managed:** A value that is owned or immutably borrowed in borrow-checked code may be passed to GC-managed functions freely. The GC treats it as a normal reference-counted value on the receiving side. Mutable borrows (`&mut`) may not be passed across the boundary while any other borrow is live — this is a compile-time error.
- **GC-managed ? Borrow-checked:** A GC-managed value passed into a borrow-checked function is implicitly treated as an immutable borrow for the duration of that call. If the borrow-checked function requires ownership (`move`), the caller must explicitly `clone()` the value first — the GC cannot transfer ownership of a value that may still be referenced elsewhere.
- **`unsafe` at the boundary:** Passing raw pointers (`mem_alloc` results) across the boundary requires an `unsafe` block. The borrow checker cannot verify pointer validity for manually-allocated memory originating from GC-managed code.

```v2
@borrow_check
func process(data: list) {      // data arrives from GC-managed caller
    let r = &data               // immutable borrow — OK
    // let m = &mut data        // ERROR — cannot mutably borrow a value
                                //         whose ownership is held by the GC
    return r.len()
}

func gc_caller() {              // GC-managed code
    let items = [1, 2, 3]
    let n = process(items)      // items implicitly borrowed for the call
    print(items)                // still valid — GC retains ownership
}
```

### Borrowing

V2 has two ways to create borrows. The operator syntax (`&x`, `&mut x`) is idiomatic in `@borrow_check` code. The keyword functions (`borrow`, `borrow_mut`) are available anywhere and produce the same values.

| Syntax   | Keyword equivalent   | Meaning                                                |
| -------- | -------------------- | ------------------------------------------------------ |
| `&x`     | `borrow(x)`          | Immutable borrow — read-only reference to `x`          |
| `&mut x` | `borrow_mut(x)`      | Mutable borrow — exclusive read-write reference to `x` |
| `*r`     | `deref(r)`           | Dereference a borrow                                   |
| `move x` | _(no keyword alias)_ | Transfer ownership of `x`                              |

**`ref`** is used in pattern matching contexts to bind a match arm variable as a reference rather than by move. It is the borrow-check equivalent of `&` on the binding side of a `let` or `match`:

```v2
let x = 42

let r = &x              // immutable borrow — operator syntax
let m = &mut x          // mutable borrow — operator syntax
let val = *r            // dereference

let moved = move x      // transfer ownership

// ref in pattern binding
match some_value {
    ref v => print(v)   // v is a reference to the matched value, not a copy
}
```

**Builtin borrow functions:**

```v2
let b = borrow(value)          // create immutable borrow
let bm = borrow_mut(value)     // create mutable borrow
let v = deref(b)               // dereference
```

### Volatile

`volatile` is a variable modifier that prevents the compiler from caching or optimising away reads and writes to a variable. Every access to a `volatile` variable always goes to the underlying memory location. This is required for hardware-mapped registers, memory-mapped I/O, and any value that can be changed by an external agent (an interrupt handler, DMA controller, or another core).

```v2
volatile let sensor_value = 0   // every read goes to memory — no register caching

unsafe {
    // Hardware register mapped at a fixed address
    let reg = mem_alloc(4)
    volatile let status = mem_read(reg, 0)    // read hardware register atomically
}
```

`volatile` does **not** imply atomicity — if multiple threads access the same volatile variable, use a mutex or atomic instead. `volatile` only prevents the _compiler_ from optimising away the access.

### Immutability

```v2
let data = [1, 2, 3]
freeze(data)              // make immutable
is_frozen(data)           // true
data.push(4)              // error: frozen
```

### `unsafe { }` Blocks

An `unsafe` block explicitly marks a region of code that bypasses memory safety guarantees. Inside an `unsafe` block, the compiler permits operations that would otherwise be rejected: raw pointer arithmetic, manual ABI/pointer interop with external C functions, casting between incompatible pointer types, and accessing `volatile` hardware addresses.

```v2
unsafe {
    let ptr = mem_alloc(16)
    mem_write(ptr, 0, 0xFF)
    let val = mem_read(ptr, 0)    // raw pointer read — no bounds check
    mem_free(ptr)
}
```

`unsafe` does not disable the language — types, scoping, and modules still work normally. It selectively suspends the subset of checks that require the compiler to reason about pointer validity and aliasing.

**When `unsafe` is required:**

| Operation                                           | Requires `unsafe` |
| --------------------------------------------------- | ----------------- |
| `mem_alloc` / `mem_free` / `mem_read` / `mem_write` | ?                 |
| Raw pointer arithmetic (`ptr + n`)                  | ?                 |
| `extern c` declarations / bindings                  | ?                 |
| `extern c` calls with raw pointers or ABI casts     | ?                 |
| Casting a pointer to a different type               | ?                 |
| Reading a `volatile` hardware register              | ?                 |
| `asm! { }` inline assembly                          | ?                 |

**`unsafe` does not bypass:**

- Type checking, name resolution, or module visibility
- `@borrow_check` rules outside the unsafe block
- Normal error handling (`throw`, `Result`, `Option`)

Annotate functions that contain or call `unsafe` blocks with `[effects: unsafe]` so callers know:

```v2
func write_hardware_reg(addr: pointer, val: u32) [effects: unsafe] {
    unsafe {
        mem_write(addr, 0, val)
    }
}
```

Keep `unsafe` blocks as small as possible — they are a contract that **you**, the programmer, have verified correctness that the compiler cannot.

### Checked Unsafe Mode and Sanitizers

For hardened builds, enable checked unsafe execution and sanitizers:

```toml
[compiler]
unsafe_mode = "checked"                  # "unchecked" | "checked"
sanitizers  = ["address", "ub", "thread"]
```

Or via CLI:

```bash
v2 --strict-unsafe --sanitizer address app.v2
v2 --strict-unsafe --sanitizer ub app.v2
```

Checked mode adds runtime guards for pointer lifetime, bounds, and invalid cast patterns. Sanitizers provide diagnostics for use-after-free, out-of-bounds access, data races, and undefined-behavior hotspots during testing.

---

## Move Semantics

V2 supports explicit **move semantics** for transferring ownership of a value from one binding to another. After a move, the source binding is invalidated — attempting to use it is a compile-time error (under `@borrow_check`) or a runtime error (under GC mode).

### Basic Move

```v2
@borrow_check

let a = [1, 2, 3]
let b = move a       // ownership transfers from a to b

print(b)             // [1, 2, 3]
// print(a)          // ERROR: use of moved value `a`
```

### Move into Functions

Passing a value as `move` transfers ownership to the function — the caller can no longer use it:

```v2
@borrow_check

func consume(data: list<int>) {
    print(f"consumed ${data.len()} items")
    // data is dropped at the end of this function
}

let items = [10, 20, 30]
consume(move items)
// print(items)      // ERROR: use of moved value `items`
```

### Move vs. Borrow

| Operation        | Syntax     | Caller retains access? | Callee can mutate?  |
| ---------------- | ---------- | ---------------------- | ------------------- |
| Immutable borrow | `&x`       | Yes (read-only)        | No                  |
| Mutable borrow   | `&mut x`   | No (while borrowed)    | Yes                 |
| Move             | `move x`   | No (permanently)       | Yes (owns it)       |
| Copy             | `clone(x)` | Yes                    | Yes (separate copy) |

### Implicit Move on Last Use

Under `@borrow_check`, the compiler can automatically move a value on its last use — no explicit `move` keyword needed:

```v2
@borrow_check

let data = load_file("big.bin")
process(data)          // implicitly moved — this is the last use of `data`
// data is not accessible here
```

This optimization avoids unnecessary copies for large values when the compiler can prove the binding is never used again.

### Move in Pattern Matching

Use `move` in match arms to take ownership of matched values:

```v2
@borrow_check

match (result) {
    case (Ok(move value)) {
        consume(value)    // takes ownership of the inner value
    }
    case (Err(ref e)) {
        log.error(e)      // borrows the error
    }
}
```

### Move and Closures

Closures can capture variables by move using the `move` keyword before the lambda:

```v2
@borrow_check

let data = [1, 2, 3]

let task = move lambda() {
    // data has been moved into the closure — the outer scope can no longer access it
    print(data)
}

// print(data)    // ERROR: use of moved value `data`
task()            // [1, 2, 3]
```

### Move Semantics without `@borrow_check`

In default GC mode, `move` still works but enforcement is at runtime rather than compile time:

```v2
let a = [1, 2, 3]
let b = move a

print(b)    // [1, 2, 3]
print(a)    // runtime error: value has been moved
```

The GC marks moved bindings as invalid and raises a `MovedValueError` on access.

---

## Manual Allocation

By default, V2 manages memory automatically. The runtime allocates and frees memory for all values without programmer intervention. However, when you need precise control — for performance-critical code, interop with native libraries, or systems-level work — V2 exposes a manual allocation API.

Manually allocated memory lives outside V2's garbage collector. You are responsible for freeing it.

### Allocation Functions

| Function                      | Description                                                         |
| ----------------------------- | ------------------------------------------------------------------- |
| `mem_alloc(size)`             | Allocate `size` bytes of uninitialized memory. Returns a `pointer`. |
| `mem_alloc_zeroed(size)`      | Allocate `size` bytes, zero-initialized. Returns a `pointer`.       |
| `mem_realloc(ptr, new_size)`  | Resize a previously allocated block. Returns a new `pointer`.       |
| `mem_free(ptr)`               | Free a previously allocated block.                                  |
| `mem_copy(dst, src, size)`    | Copy `size` bytes from `src` pointer to `dst` pointer.              |
| `mem_set(ptr, byte, size)`    | Fill `size` bytes starting at `ptr` with `byte` value.              |
| `mem_read(ptr, offset)`       | Read a byte at `ptr + offset`.                                      |
| `mem_write(ptr, offset, val)` | Write `val` byte at `ptr + offset`.                                 |
| `mem_size_of(type_name)`      | Return the byte size of a primitive type name (e.g. `"i32"`).       |

### Example — Manual Buffer

```v2
// Allocate a 64-byte buffer
let buf = mem_alloc(64)

// Write some data into it
mem_set(buf, 0, 64)            // zero it out
mem_write(buf, 0, 72)          // write 'H' (ASCII 72)
mem_write(buf, 1, 105)         // write 'i'

// Read it back
let a = mem_read(buf, 0)       // 72
let b = mem_read(buf, 1)       // 105

// Always free when done
mem_free(buf)
```

### Example — Resizing

```v2
let p = mem_alloc(16)
// ... fill 16 bytes ...
let p2 = mem_realloc(p, 32)    // grow to 32 bytes
// p is now invalid — use p2
mem_free(p2)
```

### Notes

- Manual allocations are **not** tracked by V2's garbage collector. Failing to call `mem_free` causes a memory leak.
- Using a pointer after `mem_free` triggers `MemoryAccessError` in checked mode (`--strict-unsafe`); behavior is undefined only in unchecked mode.
- Manual allocation can be combined with `borrow` / `borrow_mut` to pass raw pointers safely into borrow-checked code.
- In interpreter mode, allocation is simulated with managed backing storage; behavior is identical but no native heap is used.

---

## Vectors & Tensors

### SIMD Vectors

```v2
let v = vec_new(4)               // 4-element float vector
vec_set(v, 0, 1.0)
vec_set(v, 1, 2.0)

let a = vec_new(4)
let b = vec_new(4)
let sum = vec_add(a, b)          // element-wise add
let product = vec_mul(a, b)      // element-wise multiply
let dp = vec_dot(a, b)           // dot product
```

### SIMD Vector API

| Function               | Description                                       |
| ---------------------- | ------------------------------------------------- |
| `vec_new(size)`        | Create a zero-filled float vector of given length |
| `vec_from(list)`       | Create a vector from a list of floats             |
| `vec_get(v, i)`        | Get element at index                              |
| `vec_set(v, i, val)`   | Set element at index                              |
| `vec_len(v)`           | Number of elements                                |
| `vec_add(a, b)`        | Element-wise addition ? new vector                |
| `vec_sub(a, b)`        | Element-wise subtraction ? new vector             |
| `vec_mul(a, b)`        | Element-wise multiplication ? new vector          |
| `vec_div(a, b)`        | Element-wise division ? new vector                |
| `vec_scale(v, scalar)` | Multiply every element by scalar ? new vector     |
| `vec_dot(a, b)`        | Dot product ? float                               |
| `vec_norm(v)`          | Euclidean norm (magnitude) ? float                |
| `vec_normalize(v)`     | Unit vector (length 1) ? new vector               |
| `vec_sum(v)`           | Sum of all elements ? float                       |
| `vec_min(v)`           | Minimum element ? float                           |
| `vec_max(v)`           | Maximum element ? float                           |
| `vec_clamp(v, lo, hi)` | Clamp every element to [lo, hi] ? new vector      |
| `vec_copy(v)`          | Shallow copy of a vector ? new vector             |
| `vec_to_list(v)`       | Convert to a plain list of floats                 |

### Tensors (ML)

```v2
let t = tensor_new([3, 3])       // 3x3 tensor
tensor_set(t, [0, 0], 1.0)
let val = tensor_get(t, [0, 0])

let c = tensor_add(a, b)         // add tensors
let m = tensor_matmul(a, b)      // matrix multiply
let shape = tensor_shape(t)      // [3, 3]
```

### Tensor API

| Function                           | Description                                                 |
| ---------------------------------- | ----------------------------------------------------------- |
| `tensor_new(shape)`                | Create a zero-filled tensor with given shape (list of ints) |
| `tensor_from(list, shape)`         | Create tensor from flat list with given shape               |
| `tensor_get(t, indices)`           | Get element at index list                                   |
| `tensor_set(t, indices, val)`      | Set element at index list                                   |
| `tensor_shape(t)`                  | Returns shape as list of ints                               |
| `tensor_rank(t)`                   | Number of dimensions                                        |
| `tensor_size(t)`                   | Total number of elements                                    |
| `tensor_add(a, b)`                 | Element-wise addition                                       |
| `tensor_sub(a, b)`                 | Element-wise subtraction                                    |
| `tensor_mul(a, b)`                 | Element-wise multiplication                                 |
| `tensor_div(a, b)`                 | Element-wise division ? new tensor                          |
| `tensor_scale(t, scalar)`          | Multiply every element by scalar ? new tensor               |
| `tensor_matmul(a, b)`              | Matrix multiplication (2D tensors)                          |
| `tensor_transpose(t)`              | Transpose a 2D tensor                                       |
| `tensor_reshape(t, shape)`         | Reshape to new shape (same total elements)                  |
| `tensor_slice(t, dim, start, end)` | Slice along a dimension                                     |
| `tensor_fill(t, val)`              | Fill all elements with a value                              |
| `tensor_copy(t)`                   | Deep-copy a tensor ? new independent tensor                 |
| `tensor_sum(t)`                    | Sum of all elements ? float                                 |
| `tensor_mean(t)`                   | Mean of all elements ? float                                |
| `tensor_min(t)`                    | Minimum element value ? float                               |
| `tensor_max(t)`                    | Maximum element value ? float                               |
| `tensor_argmin(t)`                 | Flat index of the minimum element ? int                     |
| `tensor_argmax(t)`                 | Flat index of the maximum element ? int                     |
| `tensor_softmax(t)`                | Apply softmax along the last axis ? new tensor              |
| `tensor_relu(t)`                   | Apply ReLU (max(0, x)) element-wise ? new tensor            |
| `tensor_to_list(t)`                | Convert to nested lists matching shape                      |
| `tensor_from_list(nested)`         | Construct tensor from nested lists                          |

---

## Channels and Threads

### Channels

Channels are typed, thread-safe pipes between concurrent tasks. They can be buffered or unbuffered.

```v2
// Unbuffered — sender blocks until receiver is ready
let ch = chan_create()

// Buffered — sender blocks only when buffer is full
let ch = chan_create(10)

chan_send(ch, "hello")
chan_send(ch, "world")

let msg = chan_recv(ch)        // "hello" — blocks while open and empty
chan_close(ch)
chan_is_closed(ch)             // true
```

`chan_recv(ch)` semantics:

- Blocks while the channel is open and empty.
- After `chan_close(ch)`, buffered values are still delivered.
- Once the channel is closed and drained, `chan_recv(ch)` returns `null` immediately.

### Select — Multi-Channel

`chan_select` waits on multiple channels and runs the first ready one:

```v2
let result = chan_select([
    {ch: input_ch,   recv: lambda(val) => f"got input: ${val}"},
    {ch: timeout_ch, recv: lambda(_)   => "timed out"},
])
print(result)
```

### Threading

V2 threads map to native OS threads. Each thread has its own stack. Share data via channels or mutexes.

```v2
// Spawn a thread
let t = thread_spawn(lambda() {
    print("running in thread")
    return 42
})

let result = thread_join(t)    // blocks until thread finishes ? 42

// Pass arguments
let t2 = thread_spawn(lambda() {
    let data = heavy_compute(1_000_000)
    return data
})
```

### Mutex

```v2
let m = mutex_create()

mutex_lock(m)
// critical section — only one thread here at a time
shared_counter += 1
mutex_unlock(m)

// RAII-style — auto-unlock on scope exit
mutex_with(m, lambda() {
    shared_counter += 1
})
```

### Read-Write Mutex

```v2
let rw = rwmutex_create()

rwmutex_read_lock(rw)      // multiple readers allowed simultaneously
let val = shared_data
rwmutex_read_unlock(rw)

rwmutex_write_lock(rw)     // exclusive — blocks all readers and writers
shared_data = new_val
rwmutex_write_unlock(rw)
```

### Atomic Operations

For simple counters and flags without full mutex overhead:

```v2
let counter = atomic_new(0)

atomic_add(counter, 1)         // fetch-and-add
atomic_sub(counter, 1)
atomic_load(counter)           // read atomically
atomic_store(counter, 99)      // write atomically
atomic_cas(counter, 99, 100)   // compare-and-swap ? bool
```

### Thread Pool

```v2
let pool = threadpool_create(4)    // 4 worker threads

threadpool_submit(pool, lambda() {
    return heavy_compute(data)
})

threadpool_wait(pool)    // wait for all submitted tasks
threadpool_destroy(pool)
```

### Thread Pool — Getting Return Values

Use `threadpool_submit_future` to get a handle back from each submitted task:

```v2
let pool = threadpool_create(4)

let f1 = threadpool_submit_future(pool, lambda() => process_chunk(data[0:500]))
let f2 = threadpool_submit_future(pool, lambda() => process_chunk(data[500:]))

// Block until each future resolves
let result1 = future_get(f1)
let result2 = future_get(f2)

threadpool_destroy(pool)
```

| Function                             | Description                                                |
| ------------------------------------ | ---------------------------------------------------------- |
| `threadpool_submit_future(pool, fn)` | Submit a task and return a future handle                   |
| `future_get(future)`                 | Block until the future resolves and return its value       |
| `future_is_done(future)`             | `true` if the task has completed                           |
| `future_try_get(future)`             | Non-blocking: `Some(val)` if done, `None` if still running |

### Non-Blocking Channel Operations

```v2
// Try to send without blocking — returns false if the buffer is full
let ok = chan_try_send(ch, "hello")    // bool

// Try to receive without blocking — returns None if the channel is empty
let msg = chan_try_recv(ch)    // Some(val) or None

// Drain all currently available messages without blocking
let msgs = chan_drain(ch)    // list of available messages (may be empty)
```

| Function                 | Description                                            |
| ------------------------ | ------------------------------------------------------ |
| `chan_try_send(ch, val)` | Non-blocking send ? `true` if sent, `false` if full    |
| `chan_try_recv(ch)`      | Non-blocking recv ? `Some(val)` or `None`              |
| `chan_drain(ch)`         | Collect all immediately available messages into a list |
| `chan_len(ch)`           | Number of messages currently buffered                  |

### WaitGroup

A `WaitGroup` waits for a collection of async tasks or threads to finish:

```v2
let wg = waitgroup_create()

for (i in 0..5) {
    waitgroup_add(wg, 1)
    thread_spawn(lambda(i = i) {
        process_item(i)
        waitgroup_done(wg)
    })
}

waitgroup_wait(wg)    // blocks until all 5 calls to waitgroup_done
print("all done")
```

| Function               | Description                                            |
| ---------------------- | ------------------------------------------------------ |
| `waitgroup_create()`   | Create a new WaitGroup                                 |
| `waitgroup_add(wg, n)` | Increment the counter by n                             |
| `waitgroup_done(wg)`   | Decrement the counter by 1 (call when a task finishes) |
| `waitgroup_wait(wg)`   | Block until the counter reaches zero                   |

### Semaphore

A semaphore limits concurrent access to a shared resource:

```v2
let sem = semaphore_create(3)    // allow at most 3 concurrent holders

func rate_limited_call(url) {
    semaphore_acquire(sem)
    defer { semaphore_release(sem) }
    return http_get(url)
}

// Launch 10 requests — at most 3 run simultaneously
let threads = (0..10).map(lambda(i) {
    return thread_spawn(lambda() => rate_limited_call(f"https://api.example.com/${i}"))
}).collect()

for (t in threads) { thread_join(t) }
```

| Function                     | Description                            |
| ---------------------------- | -------------------------------------- |
| `semaphore_create(n)`        | Create a semaphore with capacity n     |
| `semaphore_acquire(sem)`     | Decrement; block if count is already 0 |
| `semaphore_try_acquire(sem)` | Non-blocking acquire ? `bool`          |
| `semaphore_release(sem)`     | Increment (release one slot)           |

### `thread_detach`

Detach a thread so it runs independently without needing to be joined:

```v2
let t = thread_spawn(lambda() {
    background_cleanup()
})

thread_detach(t)    // fire-and-forget — no need to join
// t is no longer joinable; the thread runs until it finishes on its own
```

Detached threads cannot be joined. If the main program exits before a detached thread finishes, the thread is terminated immediately. Use `thread_detach` only for background tasks where the result is not needed.

### Example — Producer / Consumer

```v2
let ch = chan_create(32)

// Producer thread
let producer = thread_spawn(lambda() {
    for (i in 0..100) {
        chan_send(ch, i)
    }
    chan_close(ch)
})

// Consumer thread
let consumer = thread_spawn(lambda() {
    while (true) {
        let val = chan_recv(ch)
        if (val == null) { break }    // closed and drained
        print(f"got: ${val}")
    }
})

thread_join(producer)
thread_join(consumer)
```

---

## Testing

### Test Blocks

```v2
test "addition works" {
    expect_eq(1 + 1, 2)
}

test "string methods" {
    expect_true("hello".contains("ell"))
    expect_false("hello".starts_with("world"))
}
```

### Assertions

| Function             | Description                  |
| -------------------- | ---------------------------- |
| `expect_eq(a, b)`    | Assert `a == b`              |
| `expect_ne(a, b)`    | Assert `a != b`              |
| `expect_true(val)`   | Assert value is truthy       |
| `expect_false(val)`  | Assert value is falsy        |
| `expect_ok(val)`     | Assert `val` is `Ok(_)`      |
| `expect_err(val)`    | Assert `val` is `Err(_)`     |
| `expect_some(val)`   | Assert `val` is `Some(_)`    |
| `expect_none(val)`   | Assert `val` is `None`       |
| `assert(cond, msg?)` | Assert with optional message |

### Test Registration

```v2
test_register("my test", lambda() {
    expect_eq(2 + 2, 4)
})

test_run_all()    // run all registered tests
```

### Benchmarks

```v2
bench "sorting performance" {
    let data = range(1000).collect()
    sort(data)
}
```

### Benchmarks — Reference

`bench` blocks measure execution performance. Each block is run repeatedly to produce stable timing measurements.

#### What Gets Measured

The benchmark runner:

1. Runs a **warmup phase** (default: 100 iterations) to prime caches and JIT paths.
2. Runs a **measurement phase** (default: 1000 iterations) and records wall-clock time per iteration.
3. Reports **mean**, **min**, **max**, and **standard deviation** in nanoseconds per iteration, plus throughput in operations per second.

#### Output Format

```
bench "sorting 1000 items"
  mean:   84.3 —s/op   (11,864 ops/sec)
  min:    79.1 —s/op
  max:   103.7 —s/op
  stddev:  4.2 —s
  iters:  1000
```

#### Controlling Iterations

Pass options to the `bench` block with a second argument:

```v2
bench "heavy compute" with { warmup: 10, iters: 500 } {
    heavy_computation(data)
}
```

| Option       | Default | Description                                           |
| ------------ | ------- | ----------------------------------------------------- |
| `warmup`     | `100`   | Number of warmup iterations (not measured)            |
| `iters`      | `1000`  | Number of measured iterations                         |
| `time_limit` | `null`  | Stop after this many milliseconds (overrides `iters`) |

#### Setup and Teardown

Code that should not be measured can be placed before the first statement of the bench block or extracted into helpers. If setup is expensive, run it outside the bench body:

```v2
// Expensive setup — runs once, outside the measured loop
let data = generate_large_dataset(100_000)

bench "sort large dataset" {
    // Only this runs per iteration
    let copy = data[:]    // slice copy is cheap; sort is what we're measuring
    copy.sort()
}
```

#### Tags

Bench blocks support the same `@tag` annotation as test blocks:

```v2
@tag("perf", "sorting")
bench "timsort vs radix" {
    sort(sample_data)
}
```

Run only tagged benches:

```bash
v2 bench --tag perf myapp.v2
```

#### Comparing Benches

Multiple `bench` blocks in the same file are run sequentially and their results printed together. To compare implementations, name them clearly:

```v2
bench "sort: timsort (default)" {
    sort(data[:])
}

bench "sort: radix sort" {
    radix_sort(data[:])
}
```

### Code Coverage

V2 has built-in code coverage instrumentation. Run tests or any program with `--coverage` to measure which lines and branches were executed.

```bash
v2 --coverage --test app.v2              # run tests with coverage
v2 --coverage --test --tag unit app.v2   # coverage for tagged tests only
v2 --coverage app.v2                     # coverage for a normal run
```

#### Output Formats

```bash
v2 --coverage --format text app.v2       # print summary to stdout (default)
v2 --coverage --format html app.v2       # generate HTML report in ./coverage/
v2 --coverage --format lcov app.v2       # LCOV format for CI integration
v2 --coverage --format json app.v2       # machine-readable JSON
v2 --coverage --out reports/ app.v2      # custom output directory
```

#### Coverage Summary (text output)

```
Coverage Report — app.v2
========================
File              Lines     Covered    %       Branches   Covered   %
─────────────────────────────────────────────────────────────────────
src/main.v2       120       108        90.0%   24         20        83.3%
src/parser.v2     340       298        87.6%   68         52        76.5%
src/utils.v2      45        45         100%    8          8         100%
─────────────────────────────────────────────────────────────────────
Total             505       451        89.3%   100        80        80.0%
```

#### Programmatic Access

```v2
import "std.test"

let cov = test.coverage_report()    // after running with --coverage
print(cov["total"]["line_pct"])     // 89.3
print(cov["files"]["src/main.v2"]["uncovered_lines"])    // [42, 67, 89]
```

#### Coverage Thresholds

Set minimum coverage requirements in `v2.toml` — the test run fails if thresholds are not met:

```toml
[test.coverage]
line_threshold   = 80.0    # minimum line coverage %
branch_threshold = 70.0    # minimum branch coverage %
fail_under       = true    # exit with error if below threshold
exclude          = ["test_*.v2", "vendor/**"]
```

### Mocking with `patch()`

V2's built-in `patch(target, replacement)` function is the idiomatic way to mock dependencies in tests. It swaps a function's body for another's for the duration of the test, then you restore the original afterward using `defer`.

```v2
// Production function
func send_email(to, subject, body) {
    // ... real SMTP call ...
}

test "order confirmation sends email" {
    let sent = []

    // Mock: capture calls instead of sending real email
    func fake_send_email(to, subject, body) {
        sent.push({"to": to, "subject": subject})
    }

    patch(send_email, fake_send_email)
    defer { patch(send_email, send_email) }    // restore after test

    place_order("alice@example.com", [item1, item2])

    expect_eq(sent.len(), 1)
    expect_eq(sent[0]["to"], "alice@example.com")
    expect_true(sent[0]["subject"].contains("confirmation"))
}
```

For cleaner test suites, wrap the mock+restore pattern in a helper:

```v2
func with_mock(target, mock, body) {
    let original = target
    patch(target, mock)
    defer { patch(target, original) }
    body()
}

test "payment fails gracefully" {
    with_mock(charge_card, lambda(amount) => Err("card declined"), lambda() {
        let result = checkout(cart)
        expect_err(result)
        expect_true(unwrap_err(result).contains("declined"))
    })
}
```

- `patch` swaps permanently for the current run unless patched again — `defer` is the clean way to scope a mock to a single test.
- To restore the original function, save a reference before patching:

```v2
// Save the original before patching, then restore with defer
let real_send = send_email

test "mocked email" {
    patch(send_email, fake_send_email)
    defer { patch(send_email, real_send) }    // restore the saved original
    // ...
}
```

- Patching a function that is currently on the call stack raises `PatchInProgressError`, so mock before calling, not during.

---

## Builtins Reference

### I/O

| Function                         | Description                                                                                                                          |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| `print(...)`                     | Print values (space-separated, newline at end). Accepts optional `sep` and `end` keyword args: `print("a", "b", sep: ", ", end: "")` |
| `input(prompt?)`                 | Read a line from stdin (strips trailing newline)                                                                                     |
| `read_file(path)`                | Read file contents as string                                                                                                         |
| `write_file(path, content)`      | Write string to file                                                                                                                 |
| `append_file(path, content)`     | Append to file                                                                                                                       |
| `file_exists(path)`              | Check if file exists                                                                                                                 |
| `delete_file(path)`              | Delete a file                                                                                                                        |
| `file_open(path, mode?)`         | Open a file handle (`"r"`, `"w"`, `"a"`, `"rb"`, `"wb"`)                                                                             |
| `file_read(handle)`              | Read all contents from an open file handle                                                                                           |
| `file_close(handle)`             | Close an open file handle                                                                                                            |
| `http_import_register(url, src)` | Register source text for a URL import (offline/test use)                                                                             |

```v2
// print keyword arguments
print("a", "b", "c")              // a b c\n  (space-separated, default)
print("a", "b", "c", sep: ", ")   // a, b, c\n
print("a", "b", sep: "", end: "")  // ab       (no newline)
print()                            // empty line
```

### Math

| Function         | Description                   |
| ---------------- | ----------------------------- |
| `abs(x)`         | Absolute value                |
| `min(a, b, ...)` | Minimum (also accepts a list) |
| `max(a, b, ...)` | Maximum (also accepts a list) |
| `sum(list)`      | Sum of list elements          |
| `sqrt(x)`        | Square root                   |
| `pow(base, exp)` | Exponentiation                |
| `floor(x)`       | Floor                         |
| `ceil(x)`        | Ceiling                       |
| `round(x)`       | Round to nearest              |

### Type Inspection & Conversion

| Function                                   | Description                                                                        |
| ------------------------------------------ | ---------------------------------------------------------------------------------- |
| `type(val)`                                | Human-readable type name                                                           |
| `typeof(val)`                              | Raw type tag                                                                       |
| `len(val)`                                 | Length of string/list/dict/tuple/set                                               |
| `int(val)`                                 | Convert to int. Accepts an optional second arg for base: `int("ff", 16)` ? `255`   |
| `float(val)`                               | Convert to float                                                                   |
| `str(val)`                                 | Convert to string. Accepts optional `base:` keyword: `str(255, base: 16)` ? `"ff"` |
| `bool(val)`                                | Convert to bool                                                                    |
| `resize(val, size)` / `to_size(val, size)` | Convert numeric value to size tag (`u32`, `i32`, `f64`, etc.)                      |
| `list(val)`                                | Convert to list                                                                    |
| `dict(val)`                                | Convert to dict                                                                    |
| `set(val)`                                 | Convert to set                                                                     |
| `tuple(val)`                               | Convert to tuple                                                                   |

### Collections

| Function                                                       | Description                                                                  |
| -------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| `range(end)` / `range(start, end)` / `range(start, end, step)` | Create range                                                                 |
| `push(list, item)`                                             | Append to list                                                               |
| `pop(list)`                                                    | Remove last from list                                                        |
| `reverse(val)`                                                 | Reverse list or string                                                       |
| `sort(list)`                                                   | Sort list in-place (stable, TimSort). Returns the sorted list.               |
| `sort_by(list, key_fn)`                                        | Sort in-place by key function — e.g. `sort_by(items, lambda(x) => x["age"])` |
| `to_set(list)`                                                 | Convert list to set (removes duplicates; alias of `set(list)`)               |
| `join(list, sep?)`                                             | Join list to string                                                          |
| `split(str, sep?)`                                             | Split string                                                                 |
| `zip(a, b, ...)`                                               | Zip lists                                                                    |
| `enumerate(list, start?)`                                      | Enumerate with indices                                                       |
| `deque_new(list?)`                                             | Create deque                                                                 |
| `deque_push_front(deque, item)`                                | Push item to front                                                           |
| `deque_push_back(deque, item)`                                 | Push item to back                                                            |
| `deque_pop_front(deque)`                                       | Pop item from front                                                          |
| `deque_pop_back(deque)`                                        | Pop item from back                                                           |
| `deque_len(deque)`                                             | Number of items in deque                                                     |
| `keys(dict)`                                                   | Dict keys                                                                    |
| `values(dict)`                                                 | Dict values                                                                  |
| `items(dict)`                                                  | Dict key-value pairs                                                         |
| `contains(coll, val)`                                          | Element/substring check                                                      |
| `all(list)`                                                    | All truthy?                                                                  |
| `any(list)`                                                    | Any truthy?                                                                  |

### String Builtins

| Function                  | Description          |
| ------------------------- | -------------------- |
| `upper(str)`              | Uppercase            |
| `lower(str)`              | Lowercase            |
| `trim(str)`               | Strip whitespace     |
| `replace(str, old, new)`  | Replace all          |
| `startswith(str, prefix)` | Check prefix         |
| `endswith(str, suffix)`   | Check suffix         |
| `chr(int)`                | Int to character     |
| `ord(char)`               | Char to int          |
| `hex(int)`                | Int to hex string    |
| `bin(int)`                | Int to binary string |
| `oct(int)`                | Int to octal string  |

### Option & Result

| Function                  | Description                                                                                                                                   |
| ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `Some(val)`               | Create Some                                                                                                                                   |
| `None`                    | None constant                                                                                                                                 |
| `Ok(val)`                 | Create Ok result                                                                                                                              |
| `Err(val)`                | Create Err result                                                                                                                             |
| `is_some(val)`            | Check if Some                                                                                                                                 |
| `is_none(val)`            | Check if None                                                                                                                                 |
| `is_ok(val)`              | Check if Ok                                                                                                                                   |
| `is_err(val)`             | Check if Err                                                                                                                                  |
| `unwrap(val)`             | Unwrap value (error if None/Err)                                                                                                              |
| `unwrap_or(val, default)` | Unwrap with fallback                                                                                                                          |
| `unwrap_err(val)`         | Unwrap error value                                                                                                                            |
| `unwrap_or_default(val)`  | Unwrap value, or return the type's zero-value if `None`/`Err` — `0` for numeric, `""` for str, `[]` for list, `{}` for dict, `false` for bool |
| `clone(val)`              | Deep-copy a value — produces a fully independent copy with no shared references                                                               |
| `default_(val, fallback)` | Fallback for `null`/`None`/`Err`                                                                                                              |
| `try_wrap(lambda)`        | Call `lambda()`, return `Ok(result)` or `Err(thrown)` — bridges exceptions into `Result`                                                      |

### JSON

| Function              | Description         |
| --------------------- | ------------------- |
| `json_parse(str)`     | Parse JSON to value |
| `json_stringify(val)` | Serialize to JSON   |

### System

| Function                     | Description                                                               |
| ---------------------------- | ------------------------------------------------------------------------- |
| `exit(code?)`                | Exit program                                                              |
| `assert(cond, msg?)`         | Assert condition (runtime)                                                |
| `static_assert(cond, msg)`   | Assert condition at compile time — fails compilation if false             |
| `time()`                     | Current timestamp                                                         |
| `random()`                   | Random float [0, 1)                                                       |
| `random_int(min, max)`       | Random integer in range [min, max] inclusive                              |
| `random_choice(list)`        | Random element from a list                                                |
| `shuffle(list)`              | Fisher-Yates in-place shuffle                                             |
| `sleep(ms)`                  | Sleep milliseconds                                                        |
| `getenv(name)`               | Get environment variable                                                  |
| `defined(name)`              | `true` if a variable or function named `name` exists in the current scope |
| `is_func(val)`               | `true` if `val` is a function or function reference                       |
| `is_lazy(val)`               | `true` if `val` holds a lazy expression                                   |
| `patch(target, replacement)` | Swap the body of `target` function for `replacement`'s body               |
| `rename(old_name, new_name)` | Rename a function; frees the old name for reuse                           |
| `agent_done(self)`           | Signal from inside `plan()` that the agent's goal is achieved             |

### Manual Memory

| Function                      | Description                           |
| ----------------------------- | ------------------------------------- |
| `mem_alloc(size)`             | Allocate `size` bytes (uninitialized) |
| `mem_alloc_zeroed(size)`      | Allocate `size` bytes (zeroed)        |
| `mem_realloc(ptr, new_size)`  | Resize allocation                     |
| `mem_free(ptr)`               | Free allocation                       |
| `mem_copy(dst, src, size)`    | Copy bytes between pointers           |
| `mem_set(ptr, byte, size)`    | Fill bytes at pointer                 |
| `mem_read(ptr, offset)`       | Read byte at offset                   |
| `mem_write(ptr, offset, val)` | Write byte at offset                  |
| `mem_size_of(type_name)`      | Byte size of a type name              |

### Engine Registry

| Function                      | Description                                |
| ----------------------------- | ------------------------------------------ |
| `register_engine(path, name)` | Register a custom embedded language engine |

### Isolate Builtins

| Function                            | Description                                        |
| ----------------------------------- | -------------------------------------------------- |
| `isolate_new()`                     | Create and return a new named isolate              |
| `isolate_get(iso, name)`            | Read a variable from an isolate's scope            |
| `isolate_set(iso, name, val)`       | Write a variable into an isolate's scope           |
| `isolate_exec(code_str, opts?)`     | Run V2 source string in a one-shot sandbox isolate |
| `isolate_run(iso, code_str, opts?)` | Run V2 source string inside a named isolate        |
| `isolate_drop(iso)`                 | Destroy an isolate and free its resources          |

### Metaprogramming

| Function                  | Description           |
| ------------------------- | --------------------- |
| `eval(code)`              | Evaluate code string  |
| `exec(code)`              | Execute code string   |
| `getattr(obj, name)`      | Get attribute by name |
| `setattr(obj, name, val)` | Set attribute by name |
| `hasattr(obj, name)`      | Check attribute       |
| `callable(val)`           | Check if callable     |
| `vars()`                  | Current scope as dict |
| `dir(obj?)`               | List attribute names  |
| `memo(func)`              | Memoize a function    |

### Immutability

| Function         | Description     |
| ---------------- | --------------- |
| `freeze(val)`    | Make immutable  |
| `is_frozen(val)` | Check if frozen |

### Recursion Depth

Runaway recursion raises a catchable error instead of crashing the process.
Tail calls are optimized away (see Tail-Call Optimization) and don't count
toward the limit.

| Function                  | Description                                        |
| ------------------------- | -------------------------------------------------- |
| `get_recursion_limit()`   | Current maximum call depth (default 15000)         |
| `set_recursion_limit(n)`  | Set the maximum call depth (clamped to 64–20000)   |

```v2
try {
    deep_recursive_thing()
} catch (e) {
    print(e)    // "Maximum recursion depth of 15000 exceeded ..."
}
```

---

## Method Reference

### String Methods

| Method                     | Returns | Description                            |
| -------------------------- | ------- | -------------------------------------- |
| `.len()`                   | `int`   | Length                                 |
| `.upper()`                 | `str`   | Uppercase copy                         |
| `.lower()`                 | `str`   | Lowercase copy                         |
| `.trim()`                  | `str`   | Strip leading & trailing whitespace    |
| `.trim_start()`            | `str`   | Strip leading whitespace               |
| `.trim_end()`              | `str`   | Strip trailing whitespace              |
| `.contains(sub)`           | `bool`  | Has substring?                         |
| `.starts_with(pre)`        | `bool`  | Starts with prefix?                    |
| `.ends_with(suf)`          | `bool`  | Ends with suffix?                      |
| `.split(sep, n?)`          | `list`  | Split by separator, optional max parts |
| `.replace(old, new)`       | `str`   | Replace all occurrences                |
| `.replace_first(old, new)` | `str`   | Replace first occurrence               |
| `.count(sub)`              | `int`   | Count non-overlapping occurrences      |
| `.charAt(i)`               | `str`   | Char at index                          |
| `.substr(s, e)`            | `str`   | Extract range                          |
| `.slice(s, e)`             | `str`   | Alias for substr                       |
| `.indexOf(sub)`            | `int`   | First position (-1 = absent)           |
| `.lastIndexOf(sub)`        | `int`   | Last position (-1 = absent)            |
| `.repeat(n)`               | `str`   | Repeat string n times                  |
| `.pad_start(n, ch?)`       | `str`   | Pad left to width                      |
| `.pad_end(n, ch?)`         | `str`   | Pad right to width                     |
| `.reverse()`               | `str`   | Reversed copy                          |
| `.isalpha()`               | `bool`  | All alphabetic?                        |
| `.isdigit()`               | `bool`  | All digits?                            |
| `.isalnum()`               | `bool`  | All alphanumeric?                      |
| `.isspace()`               | `bool`  | All whitespace?                        |
| `.isupper()`               | `bool`  | All uppercase?                         |
| `.islower()`               | `bool`  | All lowercase?                         |

### List Methods

| Method                  | Returns | Description                             |
| ----------------------- | ------- | --------------------------------------- |
| `.len()`                | `int`   | Length                                  |
| `.push(item)`           | `null`  | Append item                             |
| `.pop(i?)`              | `any`   | Remove & return last (or index)         |
| `.insert(i, val)`       | `null`  | Insert at index                         |
| `.remove(val)`          | `null`  | Remove first occurrence                 |
| `.clear()`              | `null`  | Remove all elements                     |
| `.extend(other)`        | `null`  | Append all from other list              |
| `.contains(val)`        | `bool`  | Has element?                            |
| `.index_of(val)`        | `int`   | First index (-1 if absent)              |
| `.count(val)`           | `int`   | Count occurrences                       |
| `.find(pred)`           | `any`   | First matching element or `None`        |
| `.any(pred)`            | `bool`  | Any element matches?                    |
| `.all(pred)`            | `bool`  | All elements match?                     |
| `.unique()`             | `list`  | Deduplicated copy                       |
| `.flat_map(lambda)`     | `list`  | Map then flatten one level              |
| `.sort()`               | `list`  | Sort in-place, returns self             |
| `.sort_by(key_fn)`      | `list`  | Sort in-place by key fn                 |
| `.reverse()`            | `list`  | Reversed copy                           |
| `.join(sep)`            | `str`   | Join to string                          |
| `.map(lambda)`          | `list`  | Transform elements                      |
| `.filter(lambda)`       | `list`  | Filter by predicate                     |
| `.reduce(lambda, init)` | `any`   | Reduce to value                         |
| `.for_each(lambda)`     | `null`  | Iterate (side effects)                  |
| `.slice(s, e)`          | `list`  | Sub-list                                |
| `.flatten()`            | `list`  | Flatten one level of nesting            |
| `.partition(pred)`      | `list`  | `[matching, non_matching]`              |
| `.group_by(fn)`         | `dict`  | Group by key fn ? dict of lists         |
| `.zip(other)`           | `list`  | Pair with another list ? list of tuples |
| `.take(n)`              | `list`  | First n elements                        |
| `.drop(n)`              | `list`  | Skip first n elements                   |
| `.sum()`                | `any`   | Sum all elements                        |
| `.product()`            | `any`   | Product of all elements                 |

### Dict Methods

| Method                | Returns | Description                                              |
| --------------------- | ------- | -------------------------------------------------------- |
| `.keys()`             | `list`  | All keys                                                 |
| `.values()`           | `list`  | All values                                               |
| `.items()`            | `list`  | List of `[key, value]` pairs                             |
| `.has(key)`           | `bool`  | Key exists?                                              |
| `.get(key, default?)` | `any`   | Get with fallback                                        |
| `.set(key, val)`      | `null`  | Set pair                                                 |
| `.remove(key)`        | `null`  | Remove key                                               |
| `.len()`              | `int`   | Entry count                                              |
| `.update(other)`      | `null`  | Merge `other` into this dict in-place (other's keys win) |
| `.merge(other)`       | `dict`  | Return new merged dict (other's keys win)                |
| `.clear()`            | `null`  | Remove all entries                                       |

### Generator Methods

| Method       | Returns | Description                                                        |
| ------------ | ------- | ------------------------------------------------------------------ |
| `.next()`    | `dict`  | `{done: bool, value: any}` — advance one step                      |
| `.send(val)` | `dict`  | Resume and make `val` the result of the current `yield` expression |
| `.collect()` | `list`  | Drain all remaining values into a list                             |
| `.is_done()` | `bool`  | `true` if the generator has returned                               |

---

## Operator Overloading

Define these methods on a class to customize operator behavior:

| Operator       | Method                                                      |
| -------------- | ----------------------------------------------------------- |
| `a + b`        | `__add__(other)`                                            |
| `a - b`        | `__sub__(other)`                                            |
| `a * b`        | `__mul__(other)`                                            |
| `a / b`        | `__div__(other)`                                            |
| `a // b`       | `__floordiv__(other)`                                       |
| `a % b`        | `__mod__(other)`                                            |
| `a ** b`       | `__pow__(other)`                                            |
| `a == b`       | `__eq__(other)`                                             |
| `a != b`       | `__ne__(other)`                                             |
| `a < b`        | `__lt__(other)`                                             |
| `a <= b`       | `__le__(other)`                                             |
| `a > b`        | `__gt__(other)`                                             |
| `a >= b`       | `__ge__(other)`                                             |
| `a band b`     | `__band__(other)`                                           |
| `a bor b`      | `__bor__(other)`                                            |
| `a bxor b`     | `__bxor__(other)`                                           |
| `bnot a`       | `__bnot__()`                                                |
| `a << b`       | `__lshift__(other)`                                         |
| `a >> b`       | `__rshift__(other)`                                         |
| `str(a)`       | `__str__()`                                                 |
| `a[key]`       | `__getitem__(key)`                                          |
| `a[key] = val` | `__setitem__(key, val)`                                     |
| `a[s:e]`       | `__getslice__(start, end)`                                  |
| `a[s:e] = val` | `__setslice__(start, end, val)`                             |
| `len(a)`       | `__len__()`                                                 |
| `key in a`     | `__contains__(key)`                                         |
| `for x in a`   | `iter()` — return an object with `.next()` and `.is_done()` |
| `-a` (unary)   | `__neg__()`                                                 |
| `!a` (unary)   | `__not__()`                                                 |

### Example — Custom Collection with `[]`

```v2
class Grid {
    func constructor(w, h) {
        self.data = list(range(w * h)).map(lambda(_) => 0)
        self.w = w
    }

    func __getitem__(pos) {
        let [x, y] = pos
        return self.data[y * self.w + x]
    }

    func __setitem__(pos, val) {
        let [x, y] = pos
        self.data[y * self.w + x] = val
    }

    func __len__() {
        return self.data.len()
    }
}

let g = new Grid(3, 3)
g[[1, 2]] = 99
print(g[[1, 2]])    // 99
print(len(g))       // 9
```

### Example — Money Class

```v2
class Money {
    func constructor(amount, currency) {
        self.amount = amount
        self.currency = currency
    }

    func __add__(other) {
        if (self.currency != other.currency) {
            throw "Currency mismatch"
        }
        return new Money(self.amount + other.amount, self.currency)
    }

    func __lt__(other) {
        return self.amount < other.amount
    }

    func __eq__(other) {
        return self.amount == other.amount && self.currency == other.currency
    }

    func __str__() {
        return f"${self.amount} ${self.currency}"
    }
}
```

---

## Keywords

All reserved keywords in V2:

### Declaration & Control

`let`, `const`, `comptime`, `func`, `lambda`, `return`, `if`, `elif`, `else`, `while`, `for`, `in`, `break`, `continue`, `match`, `case`, `default`, `defer`, `label`, `goto`

### Types & OOP

`struct`, `cstruct`, `class`, `extends`, `super`, `new`, `self`, `enum`, `trait`, `impl`, `type`, `newtype`, `pub`, `private`, `internal`, `using`, `where`, `dyn`

### Modules

`import`, `from`, `as`, `enable`, `extern`, `cimport`, `mod`

> `enable` activates embedded language engines for the file. See [Embedded Language Engines](#embedded-language-engines), especially [Enable Engines](#enable-engines).
>
> Access modifiers (`pub`, `private`, `internal`) are listed under **Types & OOP** and apply to module items as well.

### Macro Keywords

`macro`, `pattern`

### Error-Handling Keywords

`try`, `catch`, `throw`, `finally`

### Async & Generators

`yield`, `async`, `await`

### Values

`true`, `false`, `null`, `Option`, `Some`, `None`, `Ok`, `Err`, `Result`, `lazy`

### Operators & Special Symbols

`is` (type check), `in` (membership), `not in` (negated membership), `not` (used in `not in` only), `as` (type cast), `_` (pipe placeholder / pattern wildcard)

### Memory & Safety

`volatile`, `mut`, `ref`, `borrow`, `move`, `asm`, `unsafe`

### Concurrency & Isolation

`actor`, `agent`, `spawn`, `send`, `receive`, `isolate`

### Effects

`effects`, `pure`

> `pure` is shorthand for `[effects: none]` — a function that is guaranteed to have no side effects. See [Effects System](#effects-system) for full documentation and examples.

### Testing Keywords

`test`, `bench`

### Advanced Types

`vector`, `tensor`

### Source-Directive Keywords

`@replace`, `@insert`, `@borrow_check`, `@cfg` — see [Source Directives](#source-directives) for full documentation of each.

### Type Annotations

`int`, `float`, `str`, `bool`, `void`, `list`, `dict`, `pointer`, `tuple`, `set`, `any`

---

# Standard Libraries

V2's standard libraries are separate modules that must be explicitly imported. They are not loaded by default. Each library covers a distinct domain and follows the same import conventions as any other V2 module.

To keep documentation predictable, many module chapters intentionally reuse subsection labels such as `Syntax`, `Basic Usage`, `API Reference`, and `Notes`. This is a standardized layout, not duplicated feature definitions.

## Importing Standard Libraries

```v2
import "std.math"
import "std.proc"
import "std.crypto"
import "std.http"
```

Equivalent module-path syntax is also supported:

```v2
import std.math
import std.proc
import std.crypto
import std.http
```

Use either pattern for any module in the catalog below: `import "std.<module>"` or `import std.<module>`.

You may import specific symbols from any library:

```v2
import { nn_model, nn_train } from "std.ai"
import { aes_encrypt, sha256 } from "std.crypto"
```

---

## Stdlib Module Catalog

The V2 standard library is organized into focused modules. Import any module with `import std.module_name` (quotes optional).

### Implementation Status

V2 ships a small, fully-implemented **core** built into the toolchain, and treats the larger
catalog below as a **reference specification** whose I/O-, network-, and hardware-bound modules
are delivered as installable packages (see [`PACKAGES.md`](PACKAGES.md)) rather than baked into
the binary. This keeps the installer lean and lets the ecosystem grow without bloating the core.

**Built-in and fully implemented** (available with no installation):

- Language runtime: arbitrary-precision `int`, exact `decimal`, all operators, pattern matching,
  generators, async (synchronous model), classes/traits/enums, error handling.
- `std.math`, `std.io`, `std.collections`, `std.fmt` (printf/`sprintf`), `std.fs`
  (path manipulation + local file I/O), `std.regex` (subset: literals, `. \d \w \s [..] ? * +`),
  `std.crypto` (SHA-1/256/512, MD5, HMAC, base64, hex), `std.hash` (FNV-1a, CRC32, Adler32,
  murmur3), `std.uuid` (v4/v7), `std.semver`, `std.csv`, `std.decimal`, `std.money`, `std.diff`,
  `std.serialize`/JSON, `std.log` (leveled structured logging), `std.toml` (parse/stringify),
  `std.dotenv` (parse/load/env), plus partial `std.os`, `std.time`, `std.rand`.
- Metaprogramming: recursive substitution macros are Turing-complete with a tunable expansion-depth
  guard (`ct_set_macro_limit`/`ct_get_macro_limit`); `comptime` blocks/functions and `static_assert`.

**Reference specification / delivered as packages** (API is designed and documented here, but the
built-in module is a typed stub pending a native backend or a published package): all networking
(`std.http`, `std.net`, `std.grpc`, `std.mqtt`, `std.dns`, `std.ssh`, `std.webrtc`), databases
(`std.db`), GUI/desktop (`std.ui`, `std.tray`, `std.notify`, `std.clipboard`, `std.hotkey`,
`std.accessibility`), hardware (`std.gpu`, `std.serial`, `std.usb`, `std.bluetooth`, `std.camera`,
`std.hal`, `std.iot`), media (`std.audio`, `std.video`, `std.image`, `std.gfx3d`, `std.game`,
`std.2d`, `std.speech`, `std.pdf`, `std.excel`, `std.office`, `std.qr`, `std.barcode`), ML
(`std.ai`, `std.ml.vision`, `std.ml.audio`), and other I/O-bound modules (`std.mail`, `std.oauth2`,
`std.scrape`, `std.phone`, `std.blockchain`, `std.geo`, `std.map`, `std.watch`, `std.compress`,
`std.archive`, etc.). Importing one of these succeeds and returns callable stubs so code type-checks
and runs, but the operations are not yet backed by real implementations.

See [`NOT_YET_IMPLEMENTED.md`](NOT_YET_IMPLEMENTED.md) for the authoritative, up-to-date status list.

| Module              | Purpose                                                        |
| ------------------- | -------------------------------------------------------------- |
| `std.fs`            | Filesystem: paths, dirs, file ops                              |
| `std.fmt`           | String formatting and printf-style output                      |
| `std.regex`         | Regular expressions                                            |
| `std.parse`         | Parser combinators and grammar helpers                         |
| `std.iter`          | Iterator combinators                                           |
| `std.time`          | Date, time, duration, timezone                                 |
| `std.proc`          | Subprocess spawning and management                             |
| `std.log`           | Structured logging with levels and sinks                       |
| `std.diag`          | Metrics, traces, and health diagnostics                        |
| `std.iot`           | GPIO, PWM, ADC/DAC, I2C, SPI, and board adapters               |
| `std.hal`           | Bare-metal hardware abstraction (timers, gpio, uart, spi, i2c) |
| `std.office`        | DOCX/PPTX/RTF generation and reading                           |
| `std.money`         | Typed money, rounding modes, and currency-safe arithmetic      |
| `std.dotenv`        | `.env` parsing, expansion, and environment loading             |
| `std.scrape`        | DOM extraction and headless browser automation                 |
| `std.map`           | Tile-based and vector map rendering                            |
| `std.task`          | Persistent jobs, scheduling, retries, dead-letter queues       |
| `std.phone`         | SMS, OTP, and voice call provider abstraction                  |
| `std.barcode`       | EAN/Code128/DataMatrix encoding and decoding                   |
| `std.ml.vision`     | OCR, object detection, and image pipelines                     |
| `std.ml.audio`      | Transcription, diarization, and audio classification           |
| `std.test`          | Enhanced testing: fixtures, snapshots, parametrize             |
| `std.math`          | Math functions, statistics, numeric utilities                  |
| `std.io`            | Buffered I/O and streams                                       |
| `std.collections`   | Queues, heaps, linked lists, sorted maps                       |
| `std.serialize`     | JSON, MessagePack serialization                                |
| `std.ai`            | Machine learning and LLM inference                             |
| `std.crypto`        | Cryptography and security                                      |
| `std.gfx3d`         | 3D graphics and rendering                                      |
| `std.game`          | Game creation utilities                                        |
| `std.os`            | Operating system utilities                                     |
| `std.compress`      | Compression: gzip, zstd, brotli, lz4                           |
| `std.xml`           | XML and HTML parsing                                           |
| `std.image`         | Image reading, writing, and processing                         |
| `std.mail`          | Email sending and receiving                                    |
| `std.net`           | TCP/UDP/TLS networking                                         |
| `std.db`            | Database access (SQL, NoSQL)                                   |
| `std.ui`            | User interface widgets                                         |
| `std.term`          | Terminal and ANSI output                                       |
| `std.cli`           | CLI argument parsing                                           |
| `std.csv`           | CSV parsing and writing                                        |
| `std.toml`          | TOML parsing                                                   |
| `std.yaml`          | YAML parsing                                                   |
| `std.config`        | Layered app configuration and dotenv helpers                   |
| `std.uuid`          | UUID generation                                                |
| `std.rand`          | Random number generation                                       |
| `std.hash`          | Non-cryptographic hashing                                      |
| `std.cache`         | In-memory caching                                              |
| `std.event`         | In-process event bus and pub/sub                               |
| `std.signal`        | OS signal handling                                             |
| `std.http`          | HTTP client and server                                         |
| `std.ffi`           | Foreign Function Interface helpers                             |
| `std.audio`         | Audio playback and recording                                   |
| `std.video`         | Video processing                                               |
| `std.pdf`           | PDF generation and reading                                     |
| `std.excel`         | Excel / XLSX file handling                                     |
| `std.jwt`           | JSON Web Tokens                                                |
| `std.oauth2`        | OAuth 2.0 client                                               |
| `std.i18n`          | Internationalization and localization                          |
| `std.watch`         | Filesystem watching                                            |
| `std.grpc`          | gRPC client and server                                         |
| `std.mqtt`          | MQTT messaging                                                 |
| `std.embed`         | Compile-time file embedding                                    |
| `std.template`      | Text templating engine                                         |
| `std.multipart`     | Multipart form data                                            |
| `std.ssh`           | SSH client and SFTP                                            |
| `std.qr`            | QR code generation                                             |
| `std.markdown`      | Markdown parsing and rendering                                 |
| `std.archive`       | ZIP and TAR archives                                           |
| `std.dns`           | DNS resolution                                                 |
| `std.2d`            | 2D vector graphics                                             |
| `std.graphql`       | GraphQL client and server                                      |
| `std.webrtc`        | WebRTC                                                         |
| `std.clipboard`     | Clipboard access                                               |
| `std.notify`        | Desktop notifications                                          |
| `std.speech`        | Text-to-speech and recognition                                 |
| `std.camera`        | Camera and webcam access                                       |
| `std.serial`        | Serial port / UART                                             |
| `std.usb`           | USB device access                                              |
| `std.bluetooth`     | Bluetooth and BLE                                              |
| `std.hotkey`        | Global hotkeys                                                 |
| `std.tray`          | System tray                                                    |
| `std.ipc`           | Inter-process communication                                    |
| `std.decimal`       | Exact decimal arithmetic                                       |
| `std.diff`          | Text diffing and patching                                      |
| `std.semver`        | Semantic versioning                                            |
| `std.geo`           | Geospatial utilities                                           |
| `std.gpu`           | GPU compute (CUDA / Metal / Vulkan)                            |
| `std.accessibility` | Accessibility APIs                                             |
| `std.blockchain`    | Blockchain and Web3 integrations                               |

### Universal Capability Profiles

V2 can be assembled into a universal stack by composing focused modules rather than relying on one monolithic framework.

| Profile               | Core Modules                                                             | Typical Scope                                               |
| --------------------- | ------------------------------------------------------------------------ | ----------------------------------------------------------- |
| Core Service          | `std.http`, `std.db`, `std.serialize`, `std.log`, `std.signal`           | REST APIs, background workers, internal tooling             |
| Data + AI             | `std.ai`, `std.db`, `std.http`, `std.embed`, `std.template`, `std.cache` | Retrieval pipelines, model serving, report generation       |
| Realtime              | `std.http`, `std.net`, `std.mqtt`, `std.grpc`, `std.watch`               | WebSocket/SSE gateways, stream processors, brokers          |
| Edge / Device         | `std.serial`, `std.usb`, `std.bluetooth`, `std.camera`, `std.speech`     | Robotics, IoT gateways, local assistants                    |
| Desktop / Operator UX | `std.ui`, `std.term`, `std.notify`, `std.tray`, `std.clipboard`          | Admin consoles, cross-platform operator tools               |
| Blockchain / Fintech  | `std.blockchain`, `std.crypto`, `std.http`, `std.db`, `std.serialize`    | Wallet backends, indexers, contract automation, token flows |

A practical baseline for universal applications combines these modules:

```v2
import "std.http"
import "std.db"
import "std.serialize"
import "std.log"
import "std.crypto"
import "std.signal"
import "std.cache"
import "std.test"
```

For production hardening, pair the module stack with strict runtime/compiler settings:

```toml
[runtime]
async_workers = 8

[compiler]
unsafe_mode = "checked"
sanitizers = ["address", "ub", "thread"]
```

---

## std.fs — Filesystem

Full filesystem access: directory walking, path manipulation, file watching, permissions, and more.

```v2
import "std.fs"
```

### Path Utilities

| Function             | Description                   |
| -------------------- | ----------------------------- |
| `fs.join(...parts)`  | Join path segments            |
| `fs.basename(path)`  | Filename from path            |
| `fs.dirname(path)`   | Parent directory              |
| `fs.ext(path)`       | File extension (e.g. `".v2"`) |
| `fs.stem(path)`      | Filename without extension    |
| `fs.abs(path)`       | Resolve to absolute path      |
| `fs.normalize(path)` | Collapse `.` and `..`         |
| `fs.is_abs(path)`    | Is path absolute?             |

```v2
fs.join("/home", "alice", "docs")    // "/home/alice/docs"
fs.basename("/home/alice/file.v2")   // "file.v2"
fs.ext("readme.md")                  // ".md"
fs.stem("readme.md")                 // "readme"
```

### File Operations

| Function                | Description             |
| ----------------------- | ----------------------- |
| `fs.read(path)`         | Read file as string     |
| `fs.write(path, data)`  | Write string to file    |
| `fs.append(path, data)` | Append to file          |
| `fs.copy(src, dst)`     | Copy file               |
| `fs.move(src, dst)`     | Move / rename file      |
| `fs.delete(path)`       | Delete file             |
| `fs.exists(path)`       | File or dir exists?     |
| `fs.is_file(path)`      | Is a file?              |
| `fs.is_dir(path)`       | Is a directory?         |
| `fs.size(path)`         | File size in bytes      |
| `fs.modified(path)`     | Last modified timestamp |

> **Symlink functions** (`fs.symlink`, `fs.readlink`, `fs.is_symlink`) are listed under [Symlinks & Permissions](#symlinks--permissions) below.

### Directory Operations

| Function                     | Description                           |
| ---------------------------- | ------------------------------------- |
| `fs.mkdir(path, recursive?)` | Create directory                      |
| `fs.rmdir(path, recursive?)` | Remove directory                      |
| `fs.ls(path)`                | List directory entries                |
| `fs.walk(path)`              | Recursive directory walk ? generator  |
| `fs.glob(pattern)`           | Glob pattern matching ? list of paths |
| `fs.cwd()`                   | Current working directory             |
| `fs.chdir(path)`             | Change working directory              |

```v2
for (entry in fs.walk("/src")) {
    if (entry.ends_with(".v2")) {
        print(entry)
    }
}

let vt_files = fs.glob("/src/**/*.v2")
```

### Symlinks & Permissions

| Function                   | Description                                                   |
| -------------------------- | ------------------------------------------------------------- |
| `fs.symlink(target, link)` | Create symlink                                                |
| `fs.readlink(path)`        | Resolve symlink target                                        |
| `fs.is_symlink(path)`      | Is a symlink?                                                 |
| `fs.chmod(path, mode)`     | Set permissions (octal int)                                   |
| `fs.stat(path)`            | Returns `{size, modified, is_file, is_dir, is_symlink, mode}` |

### File Watching

```v2
let watcher = fs.watch("/config", lambda(event) {
    print(f"${event.path} was ${event.kind}")    // kind: "created" | "modified" | "deleted"
})

// later...
watcher.stop()
```

`fs.watch(path, callback)` is a convenience wrapper over `std.watch` for quick single-path watching. Use `std.watch` when you need multiple watched paths, per-path recursion controls, debounce settings, or `watch.wait_for`.

---

## std.fmt — Formatting

Printf-style, table, and locale-aware number formatting.

```v2
import "std.fmt"
```

### When to Use Which Formatting Method

V2 has four main ways to produce formatted strings. Here's when to use each:

| Method                    | Best for                                                 | Example                                            |
| ------------------------- | -------------------------------------------------------- | -------------------------------------------------- |
| **F-strings** `f"${...}"` | Most everyday string interpolation                       | `f"Hello, ${name}!"`                               |
| **`fmt.sprintf`**         | Printf-style, fixed-width, numeric formatting            | `fmt.sprintf("%.2f", pi)`                          |
| **`fmt.template`**        | Reusable templates rendered with a data dict             | `tpl.render({"name": "Alice"})`                    |
| **`std.template`**        | Complex multi-block text generation (HTML, emails, etc.) | Full templating engine with loops and conditionals |

**Rule of thumb:**

- Default to f-strings — they're inline, readable, and fast.
- Use `fmt.sprintf` when you need format specifiers (`%08x`, `%.4f`, etc.) or printf-style control.
- Use `fmt.template` for a simple one-liner template you'll render multiple times with different data.
- Use `std.template` when the template is large, lives in a file, or needs logic (loops, conditionals, filters).

### Printf-Style Formatting

```v2
fmt.sprintf("%s is %d years old", "Alice", 30)    // "Alice is 30 years old"
fmt.sprintf("%.2f", 3.14159)                       // "3.14"
fmt.sprintf("%08x", 255)                           // "000000ff"
fmt.printf("Hello, %s!\n", "world")               // prints to stdout
```

| Verb        | Meaning                     |
| ----------- | --------------------------- |
| `%s`        | String                      |
| `%d`        | Decimal integer             |
| `%f`        | Float                       |
| `%.Nf`      | Float with N decimal places |
| `%e` / `%E` | Scientific notation         |
| `%x` / `%X` | Hex lower/upper             |
| `%b`        | Binary                      |
| `%o`        | Octal                       |
| `%v`        | Default representation      |
| `%q`        | Quoted string               |
| `%%`        | Literal `%`                 |

### Number Formatting

```v2
fmt.number(1234567.89, {
    "decimals": 2,
    "thousands_sep": ",",
    "decimal_sep": "."
})
// "1,234,567.89"

fmt.currency(49.99, "USD")    // "$49.99"
fmt.percent(0.857)            // "85.7%"
fmt.pad_left("42", 8, "0")   // "00000042"
fmt.pad_right("hi", 10)      // "hi        "
```

### Table Formatting

```v2
let rows = [
    ["Alice", "30", "Engineer"],
    ["Bob",   "25", "Designer"],
]
let headers = ["Name", "Age", "Role"]

print(fmt.table(rows, headers))
// +------------------------+
// — Name  — Age — Role     —
// +-------+-----+----------—
// — Alice — 30  — Engineer —
// — Bob   — 25  — Designer —
// +------------------------+
```

### String Templates

```v2
let tpl = fmt.template("Hello, {name}! You have {count} messages.")
print(tpl.render({"name": "Alice", "count": 3}))
// "Hello, Alice! You have 3 messages."
```

### Locale & Collation

| Function                             | Description                                               |
| ------------------------------------ | --------------------------------------------------------- |
| `fmt.collate_sort(list, locale?)`    | Sort a list of strings using locale-aware collation rules |
| `fmt.collate_compare(a, b, locale?)` | Compare two strings locale-aware ? `-1`, `0`, `1`         |

```v2
// Sort Polish strings correctly (ą, ć, ę... come after z in ASCII order but before in Polish)
let sorted = fmt.collate_sort(["źr—dło", "ąkacja", "banan"], locale: "pl")
```

---

## std.regex — Regular Expressions

Pattern matching, capture groups, and replacement with regex.

```v2
import "std.regex"
```

### Basic Matching

```v2
regex.match("hello world", r"\w+")          // true
regex.match("12345", r"^\d+$")              // true
regex.match("abc123", r"^\d+$")             // false
```

### Find

```v2
regex.find("hello world", r"\w+")           // "hello"
regex.find_all("one two three", r"\w+")     // ["one", "two", "three"]
```

### Capture Groups

```v2
let m = regex.capture("2025-04-11", r"(\d{4})-(\d{2})-(\d{2})")
m[0]    // "2025-04-11" — full match
m[1]    // "2025"
m[2]    // "04"
m[3]    // "11"

// Named captures
let m2 = regex.capture("2025-04-11", r"(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})")
m2["year"]     // "2025"
m2["month"]    // "04"
```

### Replace

```v2
regex.replace("hello world", r"\bworld\b", "V2")     // "hello V2"
regex.replace_all("a1b2c3", r"\d", "X")              // "aXbXcX"

// $1-$9 in the replacement refer to capture groups ($0 = whole match)
regex.replace_all("john smith", r"(\w+) (\w+)", "$2 $1")   // "smith john"

// Replace with lambda
regex.replace_all("hello world", r"\w+", lambda(m) => m.upper())
// "HELLO WORLD"
```

### Split

```v2
regex.split("one,  two,three", r",\s*")    // ["one", "two", "three"]
```

### Compiled Patterns

```v2
let pat = regex.compile(r"^\d{3}-\d{4}$")
pat.match("123-4567")     // true
pat.match("abc")          // false
pat.find_all(phone_list)  // reuse compiled pattern efficiently
```

---

## std.iter — Iterator Combinators

Lazy iterator combinators for expressive data pipelines.

```v2
import "std.iter"
```

All combinator functions accept any iterable (list, generator, range, custom `Iterable`).

### Combinators

| Function                     | Description                                        |
| ---------------------------- | -------------------------------------------------- |
| `iter.take(it, n)`           | First `n` elements                                 |
| `iter.skip(it, n)`           | Drop first `n` elements                            |
| `iter.take_while(it, pred)`  | Take while predicate holds                         |
| `iter.skip_while(it, pred)`  | Skip while predicate holds                         |
| `iter.map(it, f)`            | Transform each element                             |
| `iter.filter(it, pred)`      | Keep elements where predicate is true              |
| `iter.flat_map(it, f)`       | Map then flatten one level                         |
| `iter.flatten(it)`           | Flatten one level of nesting                       |
| `iter.zip(a, b)`             | Pair elements from two iterables ? `(a, b)` tuples |
| `iter.zip_with(a, b, f)`     | Zip and apply function                             |
| `iter.chain(a, b, ...)`      | Concatenate iterables lazily                       |
| `iter.enumerate(it, start?)` | Add index ? `(i, val)` tuples                      |
| `iter.window(it, n)`         | Sliding windows of size `n`                        |
| `iter.chunk(it, n)`          | Non-overlapping chunks of size `n`                 |
| `iter.step_by(it, n)`        | Every Nth element                                  |
| `iter.cycle(it)`             | Repeat infinitely                                  |
| `iter.repeat(val, n?)`       | Repeat a value (n times or infinitely)             |
| `iter.peekable(it)`          | Iterator with `.peek()`                            |

### Terminal Operations

| Function                        | Description                      |
| ------------------------------- | -------------------------------- |
| `iter.collect(it)`              | Collect to list                  |
| `iter.reduce(it, f, init)`      | Reduce to single value           |
| `iter.count(it)`                | Count elements                   |
| `iter.sum(it)`                  | Sum all elements                 |
| `iter.min(it)` / `iter.max(it)` | Min/max element                  |
| `iter.find(it, pred)`           | First matching element or `None` |
| `iter.any(it, pred)`            | Any element matches?             |
| `iter.all(it, pred)`            | All elements match?              |
| `iter.for_each(it, f)`          | Run side effects                 |

### Examples

```v2
import "std.iter"

// Lazy pipeline — nothing runs until collect()
let result = iter.collect(
    iter.map(
        iter.filter(iter.take(naturals(), 100), lambda(x) => x % 2 == 0),
        lambda(x) => x * x
    )
)
// First 100 even perfect squares

// Chaining with pipe operator
let result2 = naturals()
    |> iter.take(_, 100)
    |> iter.filter(_, lambda(x) => x % 2 == 0)
    |> iter.map(_, lambda(x) => x * x)
    |> iter.collect(_)

// Windows and chunks
let words = ["a", "b", "c", "d", "e"]
iter.collect(iter.window(words, 3))    // [["a","b","c"], ["b","c","d"], ["c","d","e"]]
iter.collect(iter.chunk(words, 2))     // [["a","b"], ["c","d"], ["e"]]
```

---

## std.time — Date & Time

Date/time parsing, formatting, timezones, durations, and calendar arithmetic.

```v2
import "std.time"
```

### Current Time

```v2
let now = time.now()           // DateTime object, local time
let utc = time.now_utc()      // DateTime object, UTC
let ts  = time.timestamp()    // Unix timestamp (float seconds)
```

### DateTime Object

```v2
now.year        // 2025
now.month       // 4
now.day         // 11
now.hour        // 14
now.minute      // 30
now.second      // 0
now.weekday()   // "Friday"
now.tz          // "Europe/Warsaw"
```

### Parsing & Formatting

```v2
let dt = time.parse("2025-04-11", "%Y-%m-%d")
let dt2 = time.parse("11/04/2025 14:30", "%d/%m/%Y %H:%M")

time.format(now, "%Y-%m-%d")           // "2025-04-11"
time.format(now, "%d %B %Y")          // "11 April 2025"
time.format(now, "%H:%M:%S")          // "14:30:00"
```

| Format Code | Meaning          |
| ----------- | ---------------- |
| `%Y`        | 4-digit year     |
| `%m`        | Month (01—12)    |
| `%d`        | Day (01—31)      |
| `%H`        | Hour 24h (00—23) |
| `%M`        | Minute (00—59)   |
| `%S`        | Second (00—59)   |
| `%A`        | Weekday name     |
| `%B`        | Month name       |
| `%Z`        | Timezone name    |

### Durations

```v2
let d = time.duration({"hours": 2, "minutes": 30})
let d2 = time.duration({"days": 7})

let later = time.add(now, d)
let earlier = time.sub(now, d2)

let diff = time.diff(later, now)    // duration
diff.total_seconds()                // 9000.0
diff.total_minutes()                // 150.0
diff.total_hours()                  // 2.5
```

### Timezones

```v2
let warsaw = time.in_tz(now, "Europe/Warsaw")
let ny     = time.in_tz(now, "America/New_York")

time.list_tz()    // list of all IANA timezone names
```

### Comparison & Calendar

```v2
time.before(dt, dt2)    // bool
time.after(dt, dt2)     // bool
time.equal(dt, dt2)     // bool

time.start_of_day(now)    // DateTime at 00:00:00
time.end_of_day(now)      // DateTime at 23:59:59
time.start_of_week(now)   // Monday of current week
time.days_in_month(2025, 2)    // 28
time.is_leap_year(2024)        // true
```

### Timers & Intervals

Schedule callbacks to run after a delay or on a repeating interval. All timers run on the async event loop — the callback fires asynchronously even if called from synchronous code.

```v2
// One-shot timer: run callback once after 2000 ms
let id = time.set_timeout(2000, lambda() {
    print("fired after 2 seconds")
})

// Cancel before it fires
time.clear_timer(id)

// Repeating interval: run callback every 500 ms
let tick_id = time.set_interval(500, lambda() {
    print("tick")
})

// Stop after 5 seconds
time.set_timeout(5000, lambda() {
    time.clear_timer(tick_id)
    print("interval stopped")
})
```

| Function                    | Description                                                                        |
| --------------------------- | ---------------------------------------------------------------------------------- |
| `time.set_timeout(ms, fn)`  | Schedule `fn()` to run once after `ms` milliseconds. Returns a timer ID.           |
| `time.set_interval(ms, fn)` | Schedule `fn()` to run repeatedly every `ms` milliseconds. Returns a timer ID.     |
| `time.clear_timer(id)`      | Cancel a pending timeout or interval by its ID. No-op if already fired or cleared. |

Timer IDs are opaque integer handles. A cleared timer ID is invalidated — passing it to `clear_timer` again is a safe no-op.

---

## std.proc — Process Management

Spawn and control subprocesses, pipe I/O, and manage exit codes.

```v2
import "std.proc"
```

### Running Commands

```v2
// Simple run — returns {code, stdout, stderr}
let r = proc.run(["ls", "-la"])
print(r.stdout)
print(r.code)      // exit code

// Run with options
let r2 = proc.run(["python3", "script.py"], {
    "cwd": "/home/alice",
    "env": {"DEBUG": "1"},
    "timeout": 10000    // ms
})
```

### Spawning (Non-Blocking)

```v2
let p = proc.spawn(["node", "server.js"])

// Write to stdin
proc.write(p, "input data\n")

// Read from stdout (blocking)
let line = proc.read_line(p)

// Wait for exit
let code = proc.wait(p)

// Or kill
proc.kill(p)
```

### Piping

```v2
// Pipe output of one command into another
let result = proc.pipe([
    ["echo", "hello world"],
    ["tr", "a-z", "A-Z"]
])
print(result.stdout)    // "HELLO WORLD"
```

### Shell

```v2
// Run in system shell (bash / cmd)
let r = proc.shell("ls -la | grep .v2")
print(r.stdout)
```

### Environment

```v2
proc.getenv("HOME")              // "/home/alice"
proc.setenv("MY_VAR", "hello")
proc.unsetenv("MY_VAR")
proc.env()                       // dict of all env vars
proc.args()                      // list of CLI args passed to the program
proc.pid()                       // current process ID
proc.exit(0)                     // exit with code — alias for the builtin exit(code)
```

> **`proc.getenv()` vs builtin `getenv()`:** These are equivalent lookups exposed in namespaced and top-level form. `std.os` also provides `os.getenv(name)`, which documents Option-style "unset" behavior explicitly.

> **`proc.exit()` vs builtin `exit()`:** These are the same operation. `proc.exit(code)` is an alias for the top-level builtin `exit(code)` provided by `std.proc` for convenience when you are already importing the module. Both flush all pending I/O and terminate the process with the given exit code. Use whichever reads more clearly in context.

---

## std.log — Structured Logging

Log levels, structured fields, sinks, and formatting.

```v2
import "std.log"
```

### Basic Logging

```v2
log.debug("Starting up")
log.info("Server ready", {"port": 8080})
log.warn("Slow query", {"duration_ms": 450, "query": "SELECT *"})
log.error("Connection failed", {"host": "db.local", "err": e})
```

Log levels in order: `DEBUG < INFO < WARN < ERROR < FATAL`.

### Fatal Logging

`log.fatal()` logs at the highest severity level. `log.fatal_and_exit()` additionally terminates the process immediately after flushing all sinks — useful when the error is unrecoverable and continuing would be unsafe.

```v2
log.fatal("Unrecoverable state detected", {"component": "auth", "reason": "key store corrupted"})

log.fatal_and_exit("Database connection pool exhausted — cannot continue", {"db": "primary"})
// process exits with code 1 after this line
```

| Function                                  | Description                                                           |
| ----------------------------------------- | --------------------------------------------------------------------- |
| `log.fatal(msg, fields?)`                 | Log at FATAL level — does NOT exit; lets you handle shutdown yourself |
| `log.fatal_and_exit(msg, fields?, code?)` | Log at FATAL level then call `exit(code)` (default code `1`)          |

When `log.set_level("ERROR")` or higher is set, FATAL messages still always appear — FATAL cannot be suppressed by the level filter.

### Configuration

```v2
log.set_level("WARN")      // suppress DEBUG and INFO globally
log.set_format("json")     // "text" (default) or "json"
log.set_output("app.log")  // write to file (default: stdout)
```

### Named Loggers

```v2
let db_log = log.new("database")
db_log.info("Query executed", {"rows": 42})
// [INFO] [database] Query executed rows=42

let net_log = log.new("network", {"level": "DEBUG", "output": "net.log"})
```

### Structured Fields

```v2
// Add persistent fields to a logger context
let ctx_log = log.with({"request_id": "abc123", "user": "alice"})
ctx_log.info("Handler called")
ctx_log.warn("Rate limit approaching")
// All messages carry request_id and user
```

### Sinks

```v2
// Custom log sink (receives every log entry as a dict)
log.add_sink(lambda(entry) {
    if (entry["level"] == "ERROR") {
        http_post("https://alerts.example.com", json_stringify(entry))
    }
})
```

### Context Propagation

Structured log context automatically propagates through async boundaries. Use `log.context` to set trace-level fields that are carried through `await` chains, spawned tasks, and thread handoffs without manual threading:

```v2
import "std.log"

func handle_request(req) {
    // Set context for this request — all downstream logs inherit these fields
    log.context({
        "request_id": req.headers["X-Request-Id"],
        "user_id": req.user?.id,
        "trace_id": req.headers.get("X-Trace-Id", uuid())
    })

    log.info("Handling request")                    // carries request_id, user_id, trace_id
    let result = await process_order(req.body)      // context propagates through await
    log.info("Request complete", {"result": result})
}

async func process_order(body) {
    log.info("Processing order")    // still carries request_id, user_id, trace_id
    await charge_payment(body)
    await send_confirmation(body)
}
```

Context is scoped to the current async task tree — sibling tasks from different requests do not interfere:

```v2
// Two concurrent requests get independent log contexts
spawn handle_request(req1)    // context: {request_id: "aaa"}
spawn handle_request(req2)    // context: {request_id: "bbb"}
// Logs from req1 handlers only carry "aaa", req2 only carries "bbb"
```

| Function              | Description                                             |
| --------------------- | ------------------------------------------------------- |
| `log.context(fields)` | Set fields that propagate to all downstream log calls   |
| `log.context_get()`   | Retrieve the current context fields as a dict           |
| `log.context_clear()` | Clear propagated context for the current task           |
| `log.with(fields)`    | Create a child logger with additional persistent fields |

---

## std.test — Testing (Enhanced)

```v2
import "std.test"
```

The basic `test` block and `expect_*` builtins still work. `std.test` adds fixtures, hooks, tagging, parameterized tests, and snapshot testing.

### Fixtures & Hooks

```v2
test.before_all(lambda() {
    db = db_connect("sqlite://test.db")
})

test.after_all(lambda() {
    db_close(db)
})

test.before_each(lambda() {
    db_exec(db, "DELETE FROM users")
})

test "insert user" {
    db_exec(db, "INSERT INTO users VALUES (1, 'Alice')")
    let rows = db_query(db, "SELECT * FROM users")
    expect_eq(rows.len(), 1)
}
```

### Parameterized Tests

```v2
test.each([
    [2, 4],
    [3, 9],
    [10, 100],
], lambda(input, expected) {
    expect_eq(input * input, expected)
})
```

### Tagging & Filtering

```v2
test "slow database test" [tags: "slow", "db"] {
    // ...
}

test "fast unit test" [tags: "unit"] {
    // ...
}
```

Run only tagged tests:

```bash
v2 --test --tag unit hello.v2
v2 --test --skip-tag slow hello.v2
```

### Snapshot Testing

```v2
test "render output" {
    let output = render_report(sample_data)
    test.snapshot("report_output", output)
    // First run: saves snapshot
    // Subsequent runs: compares against saved snapshot
}
```

Update snapshots:

```bash
v2 --test --update-snapshots hello.v2
```

### Additional Assertions

| Function                   | Description                      |
| -------------------------- | -------------------------------- |
| `expect_eq(a, b)`          | `a == b`                         |
| `expect_ne(a, b)`          | `a != b`                         |
| `expect_gt(a, b)`          | `a > b`                          |
| `expect_lt(a, b)`          | `a < b`                          |
| `expect_true(a)`           | `a` is truthy                    |
| `expect_false(a)`          | `a` is falsy                     |
| `expect_ok(a)`             | `a` is `Ok(_)`                   |
| `expect_err(a)`            | `a` is `Err(_)`                  |
| `expect_some(a)`           | `a` is `Some(_)`                 |
| `expect_none(a)`           | `a` is `None`                    |
| `expect_throws(f)`         | `f()` throws                     |
| `expect_type(a, t)`        | `type(a) == t`                   |
| `expect_match(s, pattern)` | String matches regex pattern     |
| `test.snapshot(name, val)` | Snapshot assert                  |
| `test.skip(reason?)`       | Skip current test                |
| `test.todo(desc)`          | Mark test as not yet implemented |

### Property-Based Testing

Property-based tests verify that a property holds for _all_ inputs by generating random test cases automatically — inspired by QuickCheck and Hypothesis. Import `std.test` for the `test.property` API.

```v2
import "std.test"

test.property("reversing a list twice yields the original", lambda(data: list<int>) {
    expect_eq(data.reverse().reverse(), data)
})

test.property("sorting is idempotent", lambda(data: list<int>) {
    let sorted = data.sorted()
    expect_eq(sorted.sorted(), sorted)
})
```

#### Generators

The `test.gen` module provides composable random data generators:

```v2
import "std.test"

let gen_name = test.gen.string(min_len: 1, max_len: 50)
let gen_age  = test.gen.int(min: 0, max: 150)
let gen_user = test.gen.struct_of({
    "name": gen_name,
    "age":  gen_age
})

test.property("user age is non-negative", gen_user, lambda(user) {
    expect_true(user["age"] >= 0)
})
```

Built-in generators:

| Generator                             | Produces                      |
| ------------------------------------- | ----------------------------- |
| `test.gen.int(min?, max?)`            | Random integers               |
| `test.gen.float(min?, max?)`          | Random floats                 |
| `test.gen.bool()`                     | Random booleans               |
| `test.gen.string(min_len?, max_len?)` | Random strings                |
| `test.gen.list_of(gen, min?, max?)`   | Random lists from a generator |
| `test.gen.dict_of(key_gen, val_gen)`  | Random dicts                  |
| `test.gen.one_of(list)`               | Pick from a fixed set         |
| `test.gen.struct_of(field_gens)`      | Random dict matching a shape  |
| `test.gen.optional(gen)`              | `Some(value)` or `None`       |
| `test.gen.tuple_of(...gens)`          | Random tuples                 |

#### Shrinking

When a property fails, the test runner automatically **shrinks** the failing input to the minimal counterexample:

```
FAIL: "sorting is idempotent"
  Original failing input:  [99, -3, 42, 0, 7]
  Shrunk to:               [-3, 0]
  After 23 shrink steps
```

#### Configuration

```v2
test.property("my prop", gen, lambda(x) { ... }, {
    "trials": 500,         // number of random inputs (default: 100)
    "seed":   12345,       // deterministic replay
    "max_shrinks": 1000    // shrink budget (default: 500)
})
```

#### Stateful Property Testing

For testing stateful systems, `test.property_stateful` generates sequences of operations:

```v2
test.property_stateful("stack behaves correctly", {
    "init": lambda() { return [] },
    "commands": [
        {"name": "push", "gen": test.gen.int(), "run": lambda(state, val) {
            state.push(val)
            return state
        }},
        {"name": "pop", "gen": test.gen.none(), "run": lambda(state, _) {
            if (state.len() > 0) {
                let top = state[-1]
                let popped = state.pop()
                expect_eq(popped, top)
            }
            return state
        }}
    ],
    "trials": 200
})
```

---

## std.math — Mathematics

```v2
import "std.math"
```

### Constants

```v2
math.PI       // 3.141592653589793
math.E        // 2.718281828459045
math.TAU      // 6.283185307179586  (2 * PI)
math.INF      // infinity
math.NAN      // not-a-number
```

### Trigonometry

| Function           | Description              |
| ------------------ | ------------------------ |
| `math.sin(x)`      | Sine (radians)           |
| `math.cos(x)`      | Cosine                   |
| `math.tan(x)`      | Tangent                  |
| `math.asin(x)`     | Arc sine                 |
| `math.acos(x)`     | Arc cosine               |
| `math.atan(x)`     | Arc tangent              |
| `math.atan2(y, x)` | Two-argument arc tangent |
| `math.sinh(x)`     | Hyperbolic sine          |
| `math.cosh(x)`     | Hyperbolic cosine        |
| `math.tanh(x)`     | Hyperbolic tangent       |
| `math.deg(r)`      | Radians to degrees       |
| `math.rad(d)`      | Degrees to radians       |

### Exponential & Logarithmic

| Function              | Description    |
| --------------------- | -------------- |
| `math.exp(x)`         | `e^x`          |
| `math.exp2(x)`        | `2^x`          |
| `math.log(x)`         | Natural log    |
| `math.log2(x)`        | Log base 2     |
| `math.log10(x)`       | Log base 10    |
| `math.pow(base, exp)` | Exponentiation |
| `math.sqrt(x)`        | Square root    |
| `math.cbrt(x)`        | Cube root      |
| `math.hypot(x, y)`    | `sqrt(x—+y—)`  |

### Rounding & Clamping

| Function                | Description          |
| ----------------------- | -------------------- |
| `math.floor(x)`         | Round down           |
| `math.ceil(x)`          | Round up             |
| `math.round(x)`         | Round to nearest     |
| `math.trunc(x)`         | Truncate to integer  |
| `math.clamp(x, lo, hi)` | Clamp to range       |
| `math.lerp(a, b, t)`    | Linear interpolation |
| `math.sign(x)`          | `-1`, `0`, or `1`    |
| `math.abs(x)`           | Absolute value       |

### Numeric Checks

```v2
math.is_nan(x)    // true if NaN
math.is_inf(x)    // true if —infinity
math.is_finite(x) // true if normal finite number
```

### Integer Math

| Function                        | Description                                   |
| ------------------------------- | --------------------------------------------- |
| `math.gcd(a, b)`                | Greatest common divisor                       |
| `math.lcm(a, b)`                | Least common multiple                         |
| `math.factorial(n)`             | n!                                            |
| `math.comb(n, k)`               | Binomial coefficient (n choose k)             |
| `math.perm(n, k)`               | Permutations                                  |
| `math.is_prime(n)`              | Primality test                                |
| `math.primes_up_to(n)`          | Sieve of Eratosthenes ? list of primes = n    |
| `math.digits(n, base?)`         | Digits of n in given base (default 10) ? list |
| `math.from_digits(list, base?)` | Reconstruct integer from digit list           |
| `math.bit_count(n)`             | Number of set bits (popcount)                 |
| `math.next_power_of_two(n)`     | Smallest power of 2 = n                       |

### Statistics

Descriptive statistics over a list of numeric values.

| Function                     | Description                                               |
| ---------------------------- | --------------------------------------------------------- |
| `math.mean(list)`            | Arithmetic mean (average)                                 |
| `math.median(list)`          | Middle value (sorts internally; does not modify original) |
| `math.mode(list)`            | Most frequent value — returns a list if there is a tie    |
| `math.variance(list)`        | Population variance (divides by N)                        |
| `math.std_dev(list)`         | Population standard deviation                             |
| `math.sample_variance(list)` | Sample variance (divides by N-1, Bessel's correction)     |
| `math.sample_std_dev(list)`  | Sample standard deviation                                 |
| `math.percentile(list, p)`   | p-th percentile (0—100) using linear interpolation        |
| `math.correlation(xs, ys)`   | Pearson correlation coefficient ? float in [-1, 1]        |
| `math.covariance(xs, ys)`    | Population covariance of two equal-length lists           |

```v2
let data = [2, 4, 4, 4, 5, 5, 7, 9]

math.mean(data)               // 5.0
math.median(data)             // 4.5
math.variance(data)           // 4.0
math.std_dev(data)            // 2.0
math.sample_std_dev(data)     // 2.138...
math.percentile(data, 75)     // 5.5

let xs = [1, 2, 3, 4, 5]
let ys = [2, 4, 5, 4, 5]
math.correlation(xs, ys)      // 0.8164...
math.covariance(xs, ys)       // 1.6
```

---

## std.io — Input / Output

Buffered I/O, streams, and standard file handles.

```v2
import "std.io"
```

### Standard Streams

```v2
io.stdout.write("hello")
io.stdout.write_line("world")
io.stdout.flush()

io.stderr.write_line("an error occurred")

let line = io.stdin.read_line()
let all  = io.stdin.read_all()
```

### Buffered Reader / Writer

```v2
let f = io.open("data.txt", "r")           // "r", "w", "a", "rb", "wb"
let reader = io.buffered_reader(f)

let line = reader.read_line()              // one line (no trailing \n)
let lines = reader.lines()                // generator of all lines
let chunk = reader.read(1024)             // read N bytes

io.close(f)
```

```v2
let f = io.open("out.txt", "w")
let writer = io.buffered_writer(f)

writer.write("hello")
writer.write_line("world")
writer.flush()
io.close(f)
```

### Binary I/O

```v2
let f = io.open("data.bin", "rb")
let bytes = io.read_bytes(f, 16)          // list of ints (0—255)
let uint32 = io.read_u32(f)              // read 4 bytes as u32
io.close(f)

let f = io.open("out.bin", "wb")
io.write_bytes(f, [0xFF, 0x00, 0xAB])
io.write_u32(f, 1234567)
io.close(f)
```

All typed binary read/write functions. All multi-byte reads default to **little-endian**; append `_be` for big-endian:

| Read                  | Write                     | Size    | Type            |
| --------------------- | ------------------------- | ------- | --------------- |
| `io.read_u8(f)`       | `io.write_u8(f, v)`       | 1 byte  | Unsigned 8-bit  |
| `io.read_u16(f)`      | `io.write_u16(f, v)`      | 2 bytes | Unsigned 16-bit |
| `io.read_u32(f)`      | `io.write_u32(f, v)`      | 4 bytes | Unsigned 32-bit |
| `io.read_u64(f)`      | `io.write_u64(f, v)`      | 8 bytes | Unsigned 64-bit |
| `io.read_i8(f)`       | `io.write_i8(f, v)`       | 1 byte  | Signed 8-bit    |
| `io.read_i16(f)`      | `io.write_i16(f, v)`      | 2 bytes | Signed 16-bit   |
| `io.read_i32(f)`      | `io.write_i32(f, v)`      | 4 bytes | Signed 32-bit   |
| `io.read_i64(f)`      | `io.write_i64(f, v)`      | 8 bytes | Signed 64-bit   |
| `io.read_f32(f)`      | `io.write_f32(f, v)`      | 4 bytes | 32-bit float    |
| `io.read_f64(f)`      | `io.write_f64(f, v)`      | 8 bytes | 64-bit float    |
| `io.read_bytes(f, n)` | `io.write_bytes(f, list)` | n bytes | Raw byte list   |

Big-endian variants: `io.read_u16_be(f)`, `io.write_u32_be(f, v)`, etc. — same signatures, `_be` suffix.

### Context Manager Style

```v2
io.with_file("data.txt", "r", lambda(f) {
    let reader = io.buffered_reader(f)
    for (line in reader.lines()) {
        print(line)
    }
})
// file is automatically closed after the lambda returns
```

---

## std.collections — Data Structures

Rich collection types beyond list and dict.

```v2
import "std.collections"
```

### Stack

```v2
let s = collections.stack()
s.push(1)
s.push(2)
s.pop()       // 2
s.peek()      // 1 — look without removing
s.is_empty()  // false
s.len()       // 1
```

### Queue / Deque

```v2
let q = collections.queue()
q.enqueue(1)
q.enqueue(2)
q.dequeue()   // 1 — FIFO
q.peek()      // 2

let dq = collections.deque()
dq.push_front(0)
dq.push_back(1)
dq.pop_front()   // 0
dq.pop_back()    // 1
```

### Priority Queue (Min-Heap)

```v2
let pq = collections.priority_queue()
pq.push(5)
pq.push(1)
pq.push(3)
pq.pop()     // 1 — always pops smallest
pq.peek()    // 3

// Custom comparator (max-heap)
let max_pq = collections.priority_queue(lambda(a, b) => b < a)
```

### Ordered Map

A dict that iterates in insertion order (default dict already does this), or sorted order:

```v2
let om = collections.sorted_map()    // keys always in sorted order
om.set("banana", 2)
om.set("apple", 1)
om.set("cherry", 3)

for (k in om.keys()) { print(k) }   // apple, banana, cherry
```

| Method                | Description                                  |
| --------------------- | -------------------------------------------- |
| `.set(key, val)`      | Insert or update a key                       |
| `.get(key, default?)` | Get value, optional fallback                 |
| `.has(key)`           | Key exists?                                  |
| `.remove(key)`        | Delete a key                                 |
| `.len()`              | Number of entries                            |
| `.keys()`             | Keys in sorted order                         |
| `.values()`           | Values in key-sorted order                   |
| `.items()`            | List of `[key, value]` pairs in sorted order |
| `.clear()`            | Remove all entries                           |

### Set Operations (extended)

> **Note:** `collections.set([...])` wraps a list in the same built-in `set` type as `#{}`. The constructor is a convenience helper; both produce identical types with identical methods.

```v2
let a = collections.set([1, 2, 3, 4])
let b = collections.set([3, 4, 5, 6])

a.union(b)         // #{1,2,3,4,5,6}
a.intersect(b)     // #{3,4}
a.difference(b)    // #{1,2}
a.is_subset(b)     // false
a.is_superset(b)   // false
```

### Linked List

```v2
let ll = collections.linked_list()
ll.push_front(1)
ll.push_back(2)
ll.pop_front()    // 1
ll.len()          // 1
```

### Multiset (Bag)

```v2
let bag = collections.multiset([1, 2, 2, 3, 3, 3])
bag.count(3)       // 3
bag.add(2)
bag.remove(3)
bag.most_common()  // [[3, 2], [2, 3], [1, 1]] — sorted by frequency
```

---

## std.serialize — Serialization

```v2
import "std.serialize"
```

### JSON (extended)

> **Note:** `json_encode` / `json_decode` are also available as global builtins (see [Builtins Reference](#builtins-reference)). `std.serialize` versions are identical but namespaced and support additional options such as streaming and indentation.

```v2
serialize.json_encode(value)              // ? str
serialize.json_decode(str)               // ? value
serialize.json_encode(value, indent: 2)  // pretty-print
```

### Streaming JSON

For large data that shouldn't be held in memory all at once:

```v2
// Stream-encode a large list without building the full string
let writer = serialize.json_stream_writer(io.open("out.json", "w"))
writer.begin_array()
for (row in db_query(conn, "SELECT * FROM logs")) {
    writer.write_value(row)
}
writer.end_array()
writer.close()

// Stream-parse — receive one top-level value at a time
let reader = serialize.json_stream_reader(io.open("large.json", "r"))
for (item in reader.items()) {
    process(item)
}
```

### TOML

```v2
let config = serialize.toml_decode(read_file("config.toml"))
let toml_str = serialize.toml_encode({"key": "value", "count": 42})
```

### YAML

```v2
let data = serialize.yaml_decode(read_file("data.yaml"))
let yaml_str = serialize.yaml_encode(my_dict)
```

### CSV

```v2
let rows = serialize.csv_decode("name,age\nAlice,30\nBob,25")
// [[name, age], [Alice, 30], [Bob, 25]]

serialize.csv_encode(rows)               // back to CSV string
serialize.csv_encode(rows, header: true) // first row treated as header
```

### MessagePack

```v2
let bytes = serialize.msgpack_encode(value)    // compact binary
let value = serialize.msgpack_decode(bytes)
```

### Protobuf (Protocol Buffers)

Define a schema inline or load from a `.proto` file:

```v2
// Define schema inline
let schema = serialize.proto_schema("""
message User {
    required string name = 1;
    required int32  age  = 2;
    optional string email = 3;
}
""")

// Encode
let bytes = serialize.proto_encode({"name": "Alice", "age": 30}, schema, "User")

// Decode
let user = serialize.proto_decode(bytes, schema, "User")
print(user["name"])    // "Alice"

// Load schema from a .proto file
let schema2 = serialize.proto_load("user.proto")
```

### Binary / Raw

```v2
// Serialize to compact binary using V2's native binary format
let bytes = serialize.binary_encode(value)
let value = serialize.binary_decode(bytes)
```

`serialize.binary_encode` produces a compact, non-human-readable binary representation. It is faster and smaller than JSON but not human-readable and is only compatible with other V2 programs.

---

## std.ai — Artificial Intelligence

Provides primitives for building, training, and running machine learning models directly in V2.

```v2
import "std.ai"
```

### Neural Networks

| Function                            | Description                                              |
| ----------------------------------- | -------------------------------------------------------- |
| `nn_model(layers)`                  | Create a neural network from a list of layer descriptors |
| `nn_layer(type, size, activation?)` | Define a layer (`"dense"`, `"conv"`, `"rnn"`, etc.)      |
| `nn_train(model, data, opts)`       | Train a model on a dataset                               |
| `nn_predict(model, input)`          | Run inference on input                                   |
| `nn_save(model, path)`              | Save model weights to file                               |
| `nn_load(path)`                     | Load a saved model                                       |
| `nn_loss(model, data)`              | Compute loss on a dataset                                |

### Example — Simple Classifier

```v2
import "std.ai"

let model = nn_model([
    nn_layer("dense", 64, "relu"),
    nn_layer("dense", 32, "relu"),
    nn_layer("dense", 10, "softmax")
])

nn_train(model, training_data, {
    "epochs": 10,
    "lr": 0.001,
    "loss": "cross_entropy"
})

let prediction = nn_predict(model, input_sample)
print(prediction)
```

### Embeddings & Tokenization

| Function                    | Description                                  |
| --------------------------- | -------------------------------------------- |
| `ai_tokenize(text, vocab?)` | Tokenize a string                            |
| `ai_embed(tokens, model)`   | Generate embeddings from tokens              |
| `ai_cosine_sim(a, b)`       | Cosine similarity between two vectors        |
| `ai_top_k(probs, k)`        | Return top-k indices from a probability list |

### Large Language Model Inference

| Function                                | Description                                                     |
| --------------------------------------- | --------------------------------------------------------------- |
| `ai_llm_load(path_or_name)`             | Load a local LLM (GGUF, safetensors) or reference a name        |
| `ai_llm_generate(model, prompt, opts?)` | Run text generation ? string                                    |
| `ai_llm_chat(model, messages, opts?)`   | Chat-style completion — `messages` is list of `{role, content}` |
| `ai_llm_embed(model, text)`             | Generate text embedding ? float list                            |
| `ai_llm_unload(model)`                  | Free model memory                                               |

```v2
import "std.ai"

let model = ai_llm_load("mistral-7b-instruct")

let reply = ai_llm_chat(model, [
    {"role": "system",    "content": "You are a helpful assistant."},
    {"role": "user",      "content": "What is the capital of Poland?"}
], {
    "max_tokens": 256,
    "temperature": 0.7
})

print(reply)    // "Warsaw is the capital of Poland."
```

**Generation options:**

| Key             | Default | Description                                                            |
| --------------- | ------- | ---------------------------------------------------------------------- |
| `"max_tokens"`  | `256`   | Maximum tokens to generate                                             |
| `"temperature"` | `1.0`   | Sampling temperature (0 = deterministic, higher = more random)         |
| `"top_p"`       | `1.0`   | Nucleus sampling cutoff                                                |
| `"stop"`        | `[]`    | List of stop strings                                                   |
| `"stream"`      | `false` | If `true`, returns a generator that yields tokens as they are produced |

```v2
// Streaming generation
let stream = ai_llm_generate(model, "Once upon a time", {"stream": true})
for (token in stream) {
    print(token, end: "")
}
```

### Data Utilities

| Function                   | Description                           |
| -------------------------- | ------------------------------------- |
| `ai_dataset(list)`         | Wrap a list of `{input, label}` dicts |
| `ai_split(dataset, ratio)` | Train/test split                      |
| `ai_normalize(data)`       | Normalize numeric data to [0, 1]      |
| `ai_shuffle(dataset)`      | Shuffle a dataset                     |
| `ai_batch(dataset, size)`  | Split dataset into batches            |

---

## std.crypto — Cybersecurity & Cryptography

Provides hashing, encryption, key generation, and common security utilities.

```v2
import "std.crypto"
```

### Hashing

| Function                 | Description                                       |
| ------------------------ | ------------------------------------------------- |
| `sha256(data)`           | SHA-256 hash ? hex string                         |
| `sha512(data)`           | SHA-512 hash ? hex string                         |
| `md5(data)`              | MD5 hash ? hex string (not for security use)      |
| `blake3(data)`           | BLAKE3 hash ? hex string                          |
| `hmac(data, key, algo?)` | HMAC with optional algorithm (`"sha256"` default) |
| `hash_file(path, algo?)` | Hash a file's contents                            |

### Symmetric Encryption

| Function                                | Description                                          |
| --------------------------------------- | ---------------------------------------------------- |
| `aes_encrypt(data, key, iv?)`           | AES-256-GCM encrypt. Returns `{ciphertext, iv, tag}` |
| `aes_decrypt(ciphertext, key, iv, tag)` | AES-256-GCM decrypt. Returns plaintext               |
| `chacha20_encrypt(data, key, nonce)`    | ChaCha20-Poly1305 encrypt                            |
| `chacha20_decrypt(data, key, nonce)`    | ChaCha20-Poly1305 decrypt                            |

### Asymmetric Encryption & Signing

| Function                         | Description                                        |
| -------------------------------- | -------------------------------------------------- |
| `rsa_keygen(bits?)`              | Generate RSA key pair `{pub, priv}` (default 2048) |
| `rsa_encrypt(data, pub_key)`     | RSA encrypt with public key                        |
| `rsa_decrypt(data, priv_key)`    | RSA decrypt with private key                       |
| `rsa_sign(data, priv_key)`       | Sign data, returns signature                       |
| `rsa_verify(data, sig, pub_key)` | Verify signature ? `bool`                          |
| `ec_keygen(curve?)`              | Generate EC key pair (default `"P-256"`)           |
| `ec_sign(data, priv_key)`        | ECDSA sign                                         |
| `ec_verify(data, sig, pub_key)`  | ECDSA verify ? `bool`                              |

### Key Derivation & Random

| Function                            | Description                                           |
| ----------------------------------- | ----------------------------------------------------- |
| `pbkdf2(pass, salt, iters?, algo?)` | Password-based key derivation                         |
| `bcrypt_hash(password, cost?)`      | Bcrypt hash for password storage                      |
| `bcrypt_verify(password, hash)`     | Verify bcrypt password ? `bool`                       |
| `argon2(password, salt, opts?)`     | Argon2id key derivation                               |
| `secure_random(size)`               | Cryptographically secure random bytes                 |
| `secure_random_int(min, max)`       | Cryptographically secure random int                   |
| `uuid4()`                           | Generate a random UUID v4 (alias for `std.uuid.v4()`) |

### Encoding

| Function              | Description   |
| --------------------- | ------------- |
| `base64_encode(data)` | Base64 encode |
| `base64_decode(str)`  | Base64 decode |
| `hex_encode(data)`    | Hex encode    |
| `hex_decode(str)`     | Hex decode    |

### Example — Password Hashing

```v2
import "std.crypto"

let password = "hunter2"
let hash = bcrypt_hash(password, 12)

let ok = bcrypt_verify("hunter2", hash)     // true
let bad = bcrypt_verify("wrong", hash)      // false
```

### Example — AES Encryption

```v2
import "std.crypto"

let key = secure_random(32)     // 256-bit key
let payload = aes_encrypt("secret message", key)

let plaintext = aes_decrypt(payload["ciphertext"], key, payload["iv"], payload["tag"])
print(plaintext)    // secret message
```

---

## std.gfx3d — 3D Graphics

Provides a 3D scene, mesh, camera, lighting, and shader system.

```v2
import "std.gfx3d"
```

### Initialization

| Function                          | Description                             |
| --------------------------------- | --------------------------------------- |
| `gfx_init(width, height, title?)` | Initialize a rendering window           |
| `gfx_close()`                     | Close the window and free GPU resources |
| `gfx_begin_frame()`               | Begin a render frame                    |
| `gfx_end_frame()`                 | Flush and present the frame             |
| `gfx_clear(color?)`               | Clear the framebuffer                   |

### Scene & Nodes

| Function                           | Description                          |
| ---------------------------------- | ------------------------------------ |
| `scene_new()`                      | Create a new scene graph             |
| `scene_add(scene, node)`           | Add a node to the scene              |
| `scene_remove(scene, node)`        | Remove a node                        |
| `node_new(mesh, material?)`        | Create a scene node                  |
| `node_set_pos(node, x, y, z)`      | Set position                         |
| `node_set_rot(node, x, y, z)`      | Set rotation (Euler angles, degrees) |
| `node_set_scale(node, x, y, z)`    | Set scale                            |
| `node_translate(node, dx, dy, dz)` | Move relative to current position    |
| `node_rotate(node, axis, degrees)` | Rotate around axis                   |

### Meshes

| Function                          | Description                                     |
| --------------------------------- | ----------------------------------------------- |
| `mesh_load(path)`                 | Load a mesh from file (`.obj`, `.gltf`, `.fbx`) |
| `mesh_cube(size?)`                | Generate a unit cube mesh                       |
| `mesh_sphere(radius?, segments?)` | Generate a UV sphere mesh                       |
| `mesh_plane(width?, depth?)`      | Generate a flat plane mesh                      |
| `mesh_custom(vertices, indices)`  | Create a mesh from raw vertex data              |

### Materials & Shaders

| Function                                | Description                       |
| --------------------------------------- | --------------------------------- |
| `material_new(opts)`                    | Create a PBR material             |
| `material_set(mat, key, value)`         | Set material property             |
| `shader_load(vert_path, frag_path)`     | Load GLSL vertex/fragment shaders |
| `shader_set_uniform(shader, name, val)` | Set a shader uniform              |

### Camera

| Function                        | Description                       |
| ------------------------------- | --------------------------------- |
| `camera_new(fov?, near?, far?)` | Create a perspective camera       |
| `camera_set_pos(cam, x, y, z)`  | Set camera position               |
| `camera_look_at(cam, x, y, z)`  | Point camera at target            |
| `camera_set_ortho(cam, size)`   | Switch to orthographic projection |

### Lighting

| Function                                     | Description                |
| -------------------------------------------- | -------------------------- |
| `light_directional(dir, color?, intensity?)` | Create a directional light |
| `light_point(pos, color?, radius?)`          | Create a point light       |
| `light_spot(pos, dir, angle?, color?)`       | Create a spot light        |
| `light_ambient(color?, intensity?)`          | Set ambient light          |

### Example — Spinning Cube

```v2
import "std.gfx3d"

gfx_init(1280, 720, "My 3D Window")

let scene = scene_new()
let cube = node_new(mesh_cube())
scene_add(scene, cube)

let cam = camera_new(60.0)
camera_set_pos(cam, 0, 1, 5)
camera_look_at(cam, 0, 0, 0)

light_directional([1, -1, -1], [1, 1, 1], 1.0)

let angle = 0.0
while (gfx_is_open()) {
    gfx_begin_frame()
    gfx_clear()

    node_rotate(cube, "y", 1.0)
    gfx_render(scene, cam)

    gfx_end_frame()
}

gfx_close()
```

---

## std.game — Game Creation

A higher-level game framework built on top of `std.gfx3d`, providing game loops, input, physics, audio, and 2D/3D entity management.

```v2
import "std.game"
```

### Game Lifecycle

| Function                       | Description                       |
| ------------------------------ | --------------------------------- |
| `game_init(opts)`              | Initialize game with options dict |
| `game_run(update_fn, draw_fn)` | Start the game loop               |
| `game_quit()`                  | Exit the game loop                |
| `game_delta()`                 | Time (seconds) since last frame   |
| `game_fps()`                   | Current frames per second         |

### Entities

| Function                        | Description                             |
| ------------------------------- | --------------------------------------- |
| `entity_new(name?)`             | Create a new entity (blank game object) |
| `entity_add_component(e, comp)` | Attach a component to an entity         |
| `entity_get_component(e, type)` | Get a component by type                 |
| `entity_destroy(e)`             | Remove entity from the world            |
| `entity_find(name)`             | Find entity by name                     |
| `world_entities()`              | List all active entities                |

### Input

| Function                        | Description                                   |
| ------------------------------- | --------------------------------------------- |
| `input_key_down(key)`           | `true` if key is held                         |
| `input_key_pressed(key)`        | `true` on the frame the key was first pressed |
| `input_key_released(key)`       | `true` on the frame the key was released      |
| `input_mouse_pos()`             | Mouse position `{x, y}`                       |
| `input_mouse_button(btn)`       | `true` if mouse button is held                |
| `input_mouse_delta()`           | Mouse movement since last frame `{dx, dy}`    |
| `input_gamepad_axis(id, axis)`  | Gamepad analog axis value [-1, 1]             |
| `input_gamepad_button(id, btn)` | Gamepad button state                          |

### Physics

| Function                            | Description                                      |
| ----------------------------------- | ------------------------------------------------ |
| `physics_init(gravity?)`            | Initialize physics world                         |
| `body_new(shape, mass?)`            | Create a rigid body                              |
| `body_set_pos(body, x, y, z)`       | Set body position                                |
| `body_apply_force(body, x, y, z)`   | Apply a force                                    |
| `body_apply_impulse(body, x, y, z)` | Apply an instant impulse                         |
| `body_get_vel(body)`                | Get velocity `{x, y, z}`                         |
| `collider_box(w, h, d)`             | Box collider                                     |
| `collider_sphere(r)`                | Sphere collider                                  |
| `collider_capsule(r, h)`            | Capsule collider                                 |
| `raycast(origin, dir, dist?)`       | Cast a ray, returns `{hit, point, normal, body}` |

### Audio

| Function                       | Description                              |
| ------------------------------ | ---------------------------------------- |
| `audio_load(path)`             | Load an audio clip                       |
| `audio_play(clip, opts?)`      | Play a clip (`loop`, `volume`, `pitch`)  |
| `audio_stop(clip)`             | Stop playback                            |
| `audio_set_listener(pos, dir)` | Set 3D audio listener position/direction |
| `audio_set_pos(clip, x, y, z)` | Set 3D position of a playing clip        |

### Tilemaps & 2D

| Function                              | Description                      |
| ------------------------------------- | -------------------------------- |
| `tilemap_load(path)`                  | Load a Tiled `.tmx` tilemap      |
| `tilemap_draw(map, cam?)`             | Draw the tilemap                 |
| `sprite_new(path)`                    | Load a sprite from an image file |
| `sprite_draw(sprite, x, y, opts?)`    | Draw a sprite at screen position |
| `sprite_animate(sprite, frames, fps)` | Set animation frames and speed   |

### Example — Simple 2D Game

```v2
import "std.game"

game_init({
    "title": "My Game",
    "width": 800,
    "height": 600,
    "mode": "2d"
})

let player = sprite_new("player.png")
let px = 400.0
let py = 300.0

func update() {
    let speed = 200.0 * game_delta()
    if (input_key_down("left"))  { px -= speed }
    if (input_key_down("right")) { px += speed }
    if (input_key_down("up"))    { py -= speed }
    if (input_key_down("down"))  { py += speed }
}

func draw() {
    sprite_draw(player, px, py)
}

game_run(update, draw)
```

---

## std.os — Operating System

Runtime OS information, environment variables, signals, and system-level utilities.

```v2
import "std.os"
```

### Platform & Runtime Info

| Function          | Description                                        |
| ----------------- | -------------------------------------------------- |
| `os.platform()`   | Returns OS name: `"linux"`, `"windows"`, `"macos"` |
| `os.arch()`       | Returns CPU architecture: `"x86_64"`, `"arm64"`    |
| `os.hostname()`   | System hostname                                    |
| `os.username()`   | Current user's login name                          |
| `os.home_dir()`   | Current user's home directory path                 |
| `os.pid()`        | Current process ID (integer)                       |
| `os.ppid()`       | Parent process ID                                  |
| `os.cpu_count()`  | Number of logical CPU cores                        |
| `os.uptime()`     | System uptime in seconds                           |
| `os.v2_version()` | V2 runtime version string                          |

### Environment Variables

| Function                 | Description                                           |
| ------------------------ | ----------------------------------------------------- |
| `os.getenv(name)`        | Get an environment variable (returns `None` if unset) |
| `os.setenv(name, value)` | Set an environment variable for the current process   |
| `os.unsetenv(name)`      | Remove an environment variable                        |
| `os.environ()`           | All environment variables as a dict                   |

```v2
import "std.os"

let path = os.getenv("PATH") ?? "/usr/bin"
os.setenv("MY_VAR", "hello")
let all = os.environ()
print(all["HOME"])
```

### Signal Handling

| Function                     | Description                                                                |
| ---------------------------- | -------------------------------------------------------------------------- |
| `os.on_signal(sig, handler)` | Register a handler for a signal (e.g. `"SIGTERM"`, `"SIGINT"`, `"SIGHUP"`) |
| `os.send_signal(pid, sig)`   | Send a signal to a process by PID                                          |
| `os.ignore_signal(sig)`      | Suppress the default behavior of a signal                                  |
| `os.reset_signal(sig)`       | Restore the default signal handler                                         |

These `os.*` signal helpers are low-level convenience APIs. For cross-platform lifecycle events (`shutdown`, `reload`, etc.), one-shot handlers, and richer handler management, prefer `std.signal`.

```v2
import "std.os"

os.on_signal("SIGINT", lambda() {
    print("Caught Ctrl+C — cleaning up...")
    exit(0)
})

os.on_signal("SIGHUP", lambda() {
    print("SIGHUP received — reloading config...")
    reload_config()
})
```

### Process Exit

| Function              | Description                                                |
| --------------------- | ---------------------------------------------------------- |
| `os.exit(code?)`      | Exit the process with an optional exit code (default `0`)  |
| `os.abort()`          | Terminate immediately without cleanup                      |
| `os.at_exit(handler)` | Register a function to run when the process exits normally |

```v2
os.at_exit(lambda() {
    print("Goodbye!")
    flush_logs()
})
```

### System Paths

| Function               | Description                                                   |
| ---------------------- | ------------------------------------------------------------- |
| `os.temp_dir()`        | System temporary directory path                               |
| `os.config_dir()`      | OS-appropriate user config directory                          |
| `os.data_dir()`        | OS-appropriate user data directory                            |
| `os.cache_dir()`       | OS-appropriate user cache directory                           |
| `os.executable_path()` | Absolute path to the currently running `.v2` or bytecode file |

### Example — Cross-Platform Config Path

```v2
import "std.os"
import "std.fs"

let config_path = fs.join(os.config_dir(), "myapp", "config.toml")
if (fs.exists(config_path)) {
    let config = serialize.toml_decode(fs.read(config_path))
    print(config)
}
```

---

## std.compress — Compression

Codec-first compression and decompression for gzip, zstd, brotli, and lz4, plus lightweight zip/tar helpers.

```v2
import "std.compress"
```

`std.compress` focuses on raw codec operations and convenience archive helpers. For archive-centric object workflows (open/edit/list/extract ZIP/TAR with richer container operations), use `std.archive`.

### Stream Compression / Decompression

All algorithms share a consistent API: `compress.algo_compress(data)` / `compress.algo_decompress(data)` for in-memory buffers, and `compress.algo_compress_file` / `compress.algo_decompress_file` for files.

### gzip

```v2
let compressed   = compress.gzip_compress(data)        // bytes ? compressed bytes
let decompressed = compress.gzip_decompress(compressed) // compressed bytes ? original bytes

// File helpers
compress.gzip_compress_file("input.bin", "output.bin.gz")
compress.gzip_decompress_file("output.bin.gz", "restored.bin")
```

### zstd (Zstandard)

```v2
let compressed   = compress.zstd_compress(data, level: 3)   // level 1—22, default 3
let decompressed = compress.zstd_decompress(compressed)

compress.zstd_compress_file("data.bin", "data.bin.zst", level: 9)
compress.zstd_decompress_file("data.bin.zst", "data.bin")
```

### brotli

```v2
let compressed   = compress.brotli_compress(data, quality: 6)   // quality 0—11
let decompressed = compress.brotli_decompress(compressed)
```

### lz4

```v2
let compressed   = compress.lz4_compress(data)
let decompressed = compress.lz4_decompress(compressed)

compress.lz4_compress_file("data.bin", "data.bin.lz4")
compress.lz4_decompress_file("data.bin.lz4", "data.bin")
```

### Zip Archives

```v2
// Create a zip archive
let zip = compress.zip_create("archive.zip")
compress.zip_add_file(zip, "readme.txt", read_file("readme.txt"))
compress.zip_add_file(zip, "src/main.v2", read_file("src/main.v2"))
compress.zip_close(zip)

// Read a zip archive
let entries = compress.zip_list("archive.zip")    // list of {name, size, compressed_size}
let content = compress.zip_read(zip, "readme.txt")  // extract one file to string

// Extract all entries to a directory
compress.zip_extract("archive.zip", "./output/")
```

### Tar Archives

```v2
// Create a tarball (uncompressed or gzip-compressed)
compress.tar_create("project.tar", ["src/", "README.md"])
compress.tar_create("project.tar.gz", ["src/", "README.md"], compress: "gzip")
compress.tar_create("project.tar.zst", ["src/", "README.md"], compress: "zstd")

// List contents
let entries = compress.tar_list("project.tar")     // list of {name, size, mode}

// Extract
compress.tar_extract("project.tar.gz", "./restore/")
```

### Full API Reference

| Function                                        | Description                                                      |
| ----------------------------------------------- | ---------------------------------------------------------------- |
| `compress.gzip_compress(data, level?)`          | Compress bytes with gzip (level 1—9)                             |
| `compress.gzip_decompress(data)`                | Decompress gzip bytes                                            |
| `compress.gzip_compress_file(src, dst, level?)` | Compress file to `.gz`                                           |
| `compress.gzip_decompress_file(src, dst)`       | Decompress `.gz` file                                            |
| `compress.zstd_compress(data, level?)`          | Compress bytes with zstd (level 1—22)                            |
| `compress.zstd_decompress(data)`                | Decompress zstd bytes                                            |
| `compress.zstd_compress_file(src, dst, level?)` | Compress file to `.zst`                                          |
| `compress.zstd_decompress_file(src, dst)`       | Decompress `.zst` file                                           |
| `compress.brotli_compress(data, quality?)`      | Compress bytes with brotli (quality 0—11)                        |
| `compress.brotli_decompress(data)`              | Decompress brotli bytes                                          |
| `compress.lz4_compress(data)`                   | Compress bytes with lz4                                          |
| `compress.lz4_decompress(data)`                 | Decompress lz4 bytes                                             |
| `compress.lz4_compress_file(src, dst)`          | Compress file to `.lz4`                                          |
| `compress.lz4_decompress_file(src, dst)`        | Decompress `.lz4` file                                           |
| `compress.zip_create(path)`                     | Create a new zip archive, returns handle                         |
| `compress.zip_add_file(zip, name, data)`        | Add a file entry to the zip                                      |
| `compress.zip_add_dir(zip, name)`               | Add a directory entry                                            |
| `compress.zip_close(zip)`                       | Finalise and write the zip                                       |
| `compress.zip_list(path)`                       | List entries in a zip file                                       |
| `compress.zip_read(path, entry)`                | Extract one entry from a zip as a string                         |
| `compress.zip_read_bytes(path, entry)`          | Extract one entry as raw bytes                                   |
| `compress.zip_extract(path, outdir)`            | Extract all entries to a directory                               |
| `compress.tar_create(path, sources, compress?)` | Create a tar archive; `compress` = `"gzip"`, `"zstd"`, or `null` |
| `compress.tar_list(path)`                       | List entries in a tar archive                                    |
| `compress.tar_extract(path, outdir)`            | Extract all entries to a directory                               |

---

## std.xml — XML & HTML Parsing

Parse, query, and build XML and HTML documents.

```v2
import "std.xml"
```

### Parsing XML

```v2
let doc = xml.parse("""
    <library>
        <book id="1">
            <title>V2 Programming</title>
            <author>Alice</author>
        </book>
        <book id="2">
            <title>Systems Design</title>
            <author>Bob</author>
        </book>
    </library>
""")

// Access the root element
let root = doc.root                        // <library> element
let books = root.children("book")         // list of <book> elements
let first = books[0]

first.attr("id")                          // "1"
first.child("title").text()               // "V2 Programming"
first.child("author").text()              // "Alice"
```

### Parsing HTML

`xml.parse_html` uses a lenient parser that handles malformed HTML, missing closing tags, and common browser quirks:

```v2
let doc = xml.parse_html(read_file("page.html"))

// CSS selector queries
let links = doc.query_all("a[href]")
for (link in links) {
    print(link.attr("href"), link.text())
}

let title = doc.query("title").text()
let headings = doc.query_all("h1, h2, h3")
```

### Navigating Elements

| Method                 | Description                                            |
| ---------------------- | ------------------------------------------------------ |
| `.tag()`               | Element tag name (e.g. `"div"`)                        |
| `.text()`              | All text content, concatenated (strips tags)           |
| `.inner_html()`        | Raw inner HTML as a string                             |
| `.outer_html()`        | Raw outer HTML including the element's own tags        |
| `.attr(name)`          | Get an attribute value, or `None` if absent            |
| `.attrs()`             | All attributes as a dict                               |
| `.has_attr(name)`      | `true` if the attribute exists                         |
| `.children(tag?)`      | Direct child elements, optionally filtered by tag name |
| `.child(tag)`          | First direct child with given tag, or `None`           |
| `.parent()`            | Parent element, or `None` for root                     |
| `.siblings()`          | All sibling elements at the same level                 |
| `.next_sibling()`      | Next element sibling                                   |
| `.prev_sibling()`      | Previous element sibling                               |
| `.query(selector)`     | First element matching CSS selector, or `None`         |
| `.query_all(selector)` | All elements matching CSS selector                     |

### CSS Selectors

`query` and `query_all` support a practical subset of CSS selectors:

```v2
doc.query("div")                    // by tag
doc.query("#main")                  // by id
doc.query(".card")                  // by class
doc.query("a[href]")                // has attribute
doc.query("a[href='https://...']")  // attribute equals
doc.query("div > p")                // direct child
doc.query("div p")                  // descendant
doc.query("h1, h2")                 // multiple selectors
doc.query("li:first-child")         // pseudo-class
doc.query("input[type='text']")     // attribute filter
```

### Building XML

```v2
let root = xml.element("library")
let book = xml.element("book", attrs: {"id": "1"})
book.append(xml.element("title", text: "V2 Programming"))
book.append(xml.element("author", text: "Alice"))
root.append(book)

let output = xml.stringify(root)
// <?xml version="1.0" encoding="UTF-8"?>
// <library><book id="1"><title>V2 Programming</title>...
```

### Parsing from File / URL

```v2
let doc = xml.parse_file("data.xml")
let doc = xml.parse_html_file("page.html")
```

### XPath Queries

```v2
let titles = doc.xpath("//book/title/text()")   // list of text nodes
let first  = doc.xpath_one("//book[@id='1']")  // first matching element
```

### Validation & Module-Level Helpers

In addition to element methods, std.xml also exposes module-level helpers that align with the runtime/internals API surface:

```v2
let nodes = xml.query(doc, "a[href]")                 // module-level query helper
let text  = xml.stringify(doc)                         // explicit stringify helper

let schema = read_file("bookstore.xsd")
let ok = xml.validate(doc, schema)                     // schema/DTD validation
if (!ok) {
    print("XML validation failed")
}
```

`xml.parse_html` remains lenient for malformed HTML input. `xml.validate` is the strict conformance gate for schema/DTD checks.

---

## std.image — Image Processing

Load, transform, and save raster images in common formats.

```v2
import "std.image"
```

### Loading & Saving

```v2
let img = image.load("photo.jpg")         // load PNG, JPG, WebP, GIF, BMP, TIFF
let img = image.load_bytes(raw_bytes)     // load from in-memory bytes

image.save(img, "output.png")             // save — format inferred from extension
image.save(img, "output.jpg", quality: 85) // JPEG quality 0—100
image.save_bytes(img, "png")              // serialize to bytes without writing a file
```

### Basic Properties

```v2
img.width       // pixel width
img.height      // pixel height
img.channels    // 1 (gray), 3 (RGB), 4 (RGBA)
img.format      // "png", "jpg", "webp", etc.
```

### Metadata & Transform Pipeline Helper

```v2
let info = image.meta(img)
print(info["width"], info["height"], info["format"], info["color_profile"])

let out = image.transform(img, [
    {op: "resize", width: 1200, fit: "contain"},
    {op: "rotate", angle: 90},
    {op: "contrast", factor: 1.1}
])
```

Most convenience helpers (`resize`, `rotate`, `blur`, etc.) map to this transform pipeline internally.

### Resizing & Cropping

```v2
// Resize — specify width, height, or both; set to null to scale proportionally
let resized = image.resize(img, width: 800)                     // scale to w=800, proportional height
let resized = image.resize(img, width: 800, height: 600)        // exact size (may distort)
let resized = image.resize(img, width: 800, fit: "cover")       // crop to fill — no distortion
let resized = image.resize(img, width: 800, fit: "contain")     // letterbox — no distortion
let resized = image.resize(img, width: 800, fit: "scale_down")  // only shrink, never enlarge

// Crop to a rectangle (x, y, width, height)
let cropped = image.crop(img, 100, 50, 400, 300)

// Smart crop — finds the most "interesting" region
let thumb = image.smart_crop(img, 200, 200)
```

### Pixel Access

```v2
// Get a pixel — returns {r, g, b, a} with values 0—255
let px = image.get_pixel(img, 10, 20)
print(px["r"], px["g"], px["b"], px["a"])

// Set a pixel
image.set_pixel(img, 10, 20, {r: 255, g: 0, b: 0, a: 255})

// Iterate all pixels
for (y in 0..img.height) {
    for (x in 0..img.width) {
        let px = image.get_pixel(img, x, y)
        // ... transform px ...
        image.set_pixel(img, x, y, px)
    }
}
```

### Transforms & Filters

```v2
let flipped   = image.flip_h(img)            // flip horizontally
let flipped   = image.flip_v(img)            // flip vertically
let rotated   = image.rotate(img, 90)        // rotate 90—, 180—, 270— (lossless)
let rotated   = image.rotate(img, 45)        // arbitrary angle (fills with transparent)
let gray      = image.grayscale(img)
let blurred   = image.blur(img, radius: 3)
let sharpened = image.sharpen(img, amount: 1.5)
let bright    = image.brightness(img, factor: 1.2)   // > 1 brighter, < 1 darker
let contrasted = image.contrast(img, factor: 1.3)
let inverted  = image.invert(img)
let clipped   = image.opacity(img, 0.5)              // 0.0—1.0
```

### Compositing

```v2
// Overlay one image on top of another at position (x, y)
let composite = image.composite(base, overlay, x: 50, y: 100)
let composite = image.composite(base, overlay, x: 50, y: 100, blend: "multiply")

// Draw text onto an image
let annotated = image.draw_text(img, "Hello!", x: 20, y: 20, {
    "font_size": 24,
    "color": {r: 255, g: 255, b: 255, a: 255}
})

// Draw a rectangle
let boxed = image.draw_rect(img, x: 10, y: 10, width: 100, height: 50, {
    "color": {r: 255, g: 0, b: 0},
    "stroke": 2,      // stroke width; 0 = filled
    "fill": false
})
```

### Conversion

```v2
let rgba   = image.to_rgba(img)     // ensure 4-channel RGBA
let rgb    = image.to_rgb(img)      // drop alpha channel
let gray   = image.grayscale(img)   // to single-channel grayscale
let thumb  = image.thumbnail(img, 128)  // square thumbnail, max 128—128
```

### Full API Table

| Function                                      | Description                   |
| --------------------------------------------- | ----------------------------- |
| `image.load(path)`                            | Load image from file          |
| `image.load_bytes(bytes)`                     | Load image from raw bytes     |
| `image.save(img, path, opts?)`                | Save image to file            |
| `image.save_bytes(img, format)`               | Serialize image to bytes      |
| `image.resize(img, opts)`                     | Resize with optional fit mode |
| `image.crop(img, x, y, w, h)`                 | Rectangular crop              |
| `image.smart_crop(img, w, h)`                 | Content-aware crop            |
| `image.flip_h(img)` / `flip_v(img)`           | Flip horizontal/vertical      |
| `image.rotate(img, degrees)`                  | Rotate                        |
| `image.grayscale(img)`                        | Convert to grayscale          |
| `image.blur(img, radius)`                     | Gaussian blur                 |
| `image.sharpen(img, amount)`                  | Unsharp mask                  |
| `image.brightness(img, factor)`               | Adjust brightness             |
| `image.contrast(img, factor)`                 | Adjust contrast               |
| `image.invert(img)`                           | Invert colours                |
| `image.opacity(img, alpha)`                   | Multiply alpha channel        |
| `image.composite(base, overlay, x, y, opts?)` | Composite images              |
| `image.draw_text(img, text, x, y, opts?)`     | Rasterize text onto image     |
| `image.draw_rect(img, x, y, w, h, opts?)`     | Draw a rectangle              |
| `image.get_pixel(img, x, y)`                  | Read pixel as `{r, g, b, a}`  |
| `image.set_pixel(img, x, y, px)`              | Write pixel                   |
| `image.to_rgba(img)`                          | Convert to 4-channel RGBA     |
| `image.to_rgb(img)`                           | Convert to 3-channel RGB      |
| `image.thumbnail(img, size)`                  | Fast square thumbnail         |

---

## std.mail — Email

Send email via SMTP with plain text, HTML, and attachments.

```v2
import "std.mail"
```

### Sending a Simple Email

```v2
let mailer = mail.connect({
    "host":     "smtp.example.com",
    "port":     587,
    "tls":      true,
    "username": "user@example.com",
    "password": os.getenv("SMTP_PASS") ?? ""
})

mail.send(mailer, {
    "from":    "sender@example.com",
    "to":      ["alice@example.com"],
    "subject": "Hello from V2",
    "text":    "This is the plain-text body."
})
```

### HTML Email with Attachments

```v2
mail.send(mailer, {
    "from":    "noreply@example.com",
    "to":      ["alice@example.com", "bob@example.com"],
    "cc":      ["manager@example.com"],
    "bcc":     ["audit@example.com"],
    "subject": "Monthly Report",
    "text":    "Please see the attached HTML report.",
    "html":    f"""
        <h1>Monthly Report</h1>
        <p>Figures for ${month}:</p>
        <table>...</table>
    """,
    "attachments": [
        {"name": "report.pdf", "data": read_file("report.pdf"), "type": "application/pdf"},
        {"name": "data.csv",   "data": read_file("data.csv"),   "type": "text/csv"}
    ]
})

mail.disconnect(mailer)
```

### Using a Template

```v2
let tpl = mail.template(read_file("email_templates/welcome.html"))

mail.send(mailer, {
    "from":    "welcome@example.com",
    "to":      [user.email],
    "subject": f"Welcome, ${user.name}!",
    "html":    tpl.render({"name": user.name, "link": activation_link})
})
```

### Full API Reference

| Function                          | Description                                                  |
| --------------------------------- | ------------------------------------------------------------ |
| `mail.connect(opts)`              | Open an SMTP connection. Returns a mailer handle             |
| `mail.send(mailer, msg)`          | Send an email. `msg` is a dict (see fields below)            |
| `mail.send_raw(mailer, raw_mime)` | Send a raw MIME message string                               |
| `mail.disconnect(mailer)`         | Close the SMTP connection                                    |
| `mail.template(html_str)`         | Create a template from an HTML string                        |
| `tpl.render(vars)`                | Render the template with variable substitution ? HTML string |

**`mail.connect` options:**

| Key          | Description                                               |
| ------------ | --------------------------------------------------------- |
| `"host"`     | SMTP server hostname                                      |
| `"port"`     | SMTP port (commonly `25`, `465`, `587`)                   |
| `"tls"`      | `true` for STARTTLS (port 587) or implicit TLS (port 465) |
| `"username"` | SMTP authentication username                              |
| `"password"` | SMTP authentication password                              |
| `"timeout"`  | Connection timeout in milliseconds (default: `10000`)     |

**`mail.send` message fields:**

| Key             | Required | Description                        |
| --------------- | -------- | ---------------------------------- |
| `"from"`        | ?        | Sender address (string)            |
| `"to"`          | ?        | Recipient list (list of strings)   |
| `"subject"`     | ?        | Email subject line                 |
| `"text"`        | ?        | Plain-text body                    |
| `"html"`        | ?        | HTML body                          |
| `"cc"`          | ?        | CC addresses (list of strings)     |
| `"bcc"`         | ?        | BCC addresses (list of strings)    |
| `"reply_to"`    | ?        | Reply-To address                   |
| `"attachments"` | ?        | List of `{name, data, type}` dicts |
| `"headers"`     | ?        | Extra MIME headers as a dict       |

At least one of `"text"` or `"html"` must be provided. If both are given, the message is sent as `multipart/alternative`.

---

## std.net — Networking

HTTP, WebSocket, TCP/UDP, and DNS utilities.

```v2
import "std.net"
```

### HTTP Client

| Function                         | Description                           |
| -------------------------------- | ------------------------------------- |
| `http_get(url, headers?)`        | HTTP GET ? `{status, body, headers}`  |
| `http_get(url, headers?, opts?)` | HTTP GET with options (timeout, TLS)  |
| `http_post(url, body, headers?)` | HTTP POST ? `{status, body, headers}` |
| `http_put(url, body, headers?)`  | HTTP PUT                              |
| `http_delete(url, headers?)`     | HTTP DELETE                           |
| `http_request(opts)`             | Custom HTTP request — see opts below  |

All shorthand functions (`http_get`, `http_post`, etc.) accept an optional trailing `opts` dict with the same keys as `http_request`. Use this for one-off timeout or TLS configuration without switching to `http_request`:

```v2
// GET with 5 second timeout
let resp = http_get("https://api.example.com/data", {}, {
    "timeout": 5000,
    "verify_tls": false    // skip TLS verification (dev only)
})

// POST with bearer auth and timeout
let resp2 = http_post(
    "https://api.example.com/items",
    json_stringify(payload),
    {"Content-Type": "application/json"},
    {"timeout": 3000, "auth": {"type": "bearer", "token": token}}
)
```

#### `http_request` Options

| Key                  | Type   | Description                                                                                          |
| -------------------- | ------ | ---------------------------------------------------------------------------------------------------- |
| `"method"`           | `str`  | HTTP method: `"GET"`, `"POST"`, `"PUT"`, `"DELETE"`, `"PATCH"`, `"HEAD"`, `"OPTIONS"`                |
| `"url"`              | `str`  | Request URL                                                                                          |
| `"headers"`          | `dict` | Request headers dict                                                                                 |
| `"body"`             | `str`  | Request body (string or bytes)                                                                       |
| `"timeout"`          | `int`  | Timeout in milliseconds (default: 30000)                                                             |
| `"follow_redirects"` | `bool` | Follow HTTP redirects (default: `true`)                                                              |
| `"verify_tls"`       | `bool` | Verify TLS certificate (default: `true`)                                                             |
| `"ca_bundle"`        | `str`  | Path to custom CA certificate bundle                                                                 |
| `"client_cert"`      | `str`  | Path to client certificate (mTLS)                                                                    |
| `"client_key"`       | `str`  | Path to client private key (mTLS)                                                                    |
| `"cookies"`          | `dict` | Cookies to send                                                                                      |
| `"auth"`             | `dict` | Auth dict: `{"type": "basic", "user": "...", "pass": "..."}` or `{"type": "bearer", "token": "..."}` |

```v2
let resp = http_request({
    "method": "POST",
    "url": "https://api.example.com/data",
    "headers": {"Content-Type": "application/json"},
    "body": json_stringify({"key": "value"}),
    "timeout": 5000,
    "auth": {"type": "bearer", "token": my_token}
})

print(resp["status"])    // 200
print(resp["body"])
print(resp["headers"])
print(resp["cookies"])   // dict of response cookies
```

### HTTP Server

| Function                                   | Description                           |
| ------------------------------------------ | ------------------------------------- |
| `http_serve(port, handler)`                | Start an HTTP server                  |
| `http_response(status, body, headers?)`    | Build an HTTP response dict           |
| `http_serve_tls(port, cert, key, handler)` | Start an HTTPS server with TLS        |
| `http_router()`                            | Create a route-based request router   |
| `router.get(path, handler)`                | Register a GET route                  |
| `router.post(path, handler)`               | Register a POST route                 |
| `router.put(path, handler)`                | Register a PUT route                  |
| `router.delete(path, handler)`             | Register a DELETE route               |
| `router.use(middleware)`                   | Register a middleware function        |
| `router.handle(req)`                       | Dispatch a request through the router |

> `http_serve` and `http_serve_tls` are convenience global builtins that delegate to `std.http`. For advanced server features (middleware stacks, HTTP/2 controls, server objects, lifecycle hooks), prefer `import "std.http"` and the namespaced API.

#### Request Dict Fields

The `req` dict passed to handlers contains:

| Field                | Type   | Description                                       |
| -------------------- | ------ | ------------------------------------------------- |
| `req["method"]`      | `str`  | HTTP method (`"GET"`, `"POST"`, etc.)             |
| `req["path"]`        | `str`  | URL path (e.g. `"/users/42"`)                     |
| `req["query"]`       | `dict` | Parsed query string params                        |
| `req["headers"]`     | `dict` | Request headers                                   |
| `req["body"]`        | `str`  | Raw request body                                  |
| `req["cookies"]`     | `dict` | Parsed cookies                                    |
| `req["params"]`      | `dict` | Route params (e.g. `{id: "42"}` for `/users/:id`) |
| `req["remote_addr"]` | `str`  | Client IP address                                 |

```v2
import "std.net"

let router = http_router()

router.get("/", lambda(req) {
    return http_response(200, "Hello!")
})

router.get("/users/:id", lambda(req) {
    let id = req["params"]["id"]
    return http_response(200, f"User ${id}")
})

router.post("/echo", lambda(req) {
    return http_response(200, req["body"])
})

// Middleware example
router.use(lambda(req, next) {
    log.info("Request", {"method": req["method"], "path": req["path"]})
    return next(req)
})

http_serve(8080, lambda(req) => router.handle(req))
```

### WebSocket

| Function                  | Description                   |
| ------------------------- | ----------------------------- |
| `ws_connect(url)`         | Connect to a WebSocket server |
| `ws_send(conn, msg)`      | Send a message                |
| `ws_recv(conn)`           | Receive a message (blocking)  |
| `ws_close(conn)`          | Close the connection          |
| `ws_serve(port, handler)` | Start a WebSocket server      |

### TCP / UDP

| Function                           | Description                |
| ---------------------------------- | -------------------------- |
| `tcp_connect(host, port)`          | Open a TCP connection      |
| `tcp_listen(port, handler)`        | Listen for TCP connections |
| `tcp_send(conn, data)`             | Send bytes over TCP        |
| `tcp_recv(conn, size?)`            | Receive bytes from TCP     |
| `udp_socket(port?)`                | Create a UDP socket        |
| `udp_send(sock, host, port, data)` | Send a UDP datagram        |
| `udp_recv(sock)`                   | Receive a UDP datagram     |

### DNS

| Function                     | Description                                     |
| ---------------------------- | ----------------------------------------------- |
| `dns_resolve(hostname)`      | Resolve hostname ? list of IP strings           |
| `dns_resolve_ipv4(hostname)` | Resolve hostname ? IPv4 address list            |
| `dns_resolve_ipv6(hostname)` | Resolve hostname ? IPv6 address list            |
| `dns_reverse(ip)`            | Reverse lookup IP ? hostname                    |
| `dns_lookup_mx(domain)`      | Look up MX records ? list of `{host, priority}` |
| `dns_lookup_txt(domain)`     | Look up TXT records ? list of strings           |

```v2
import "std.net"

let ips = dns_resolve("example.com")
print(ips)    // ["93.184.216.34"]

let host = dns_reverse("93.184.216.34")
print(host)   // "example.com"

let mxs = dns_lookup_mx("gmail.com")
for (mx in mxs) {
    print(f"${mx['priority']} ${mx['host']}")
}
```

### Example — Simple HTTP Server

```v2
import "std.net"

http_serve(8080, lambda(req) {
    if (req["path"] == "/") {
        return http_response(200, "Hello, World!", {"Content-Type": "text/plain"})
    }
    return http_response(404, "Not Found")
})
```

---

## std.db — Databases

SQL and key-value store access.

```v2
import "std.db"
```

### SQL

| Function                       | Description                                            |
| ------------------------------ | ------------------------------------------------------ |
| `db_connect(url)`              | Connect to a database (SQLite, Postgres, MySQL)        |
| `db_connect(url, opts)`        | Connect with options (pool size, timeout, etc.)        |
| `db_query(conn, sql, params?)` | Execute a SELECT ? list of row dicts                   |
| `db_exec(conn, sql, params?)`  | Execute INSERT/UPDATE/DELETE ? `{affected, last_id}`   |
| `db_transaction(conn, fn)`     | Run `fn` inside a transaction (auto-rollback on error) |
| `db_prepare(conn, sql)`        | Prepare a statement for repeated execution             |
| `db_run(stmt, params?)`        | Execute a prepared statement                           |
| `db_close(conn)`               | Close connection                                       |

#### `db_connect` Options

| Key           | Description                                        |
| ------------- | -------------------------------------------------- |
| `"pool_size"` | Max number of connections in the pool (default: 1) |
| `"timeout"`   | Connection timeout in milliseconds                 |
| `"max_idle"`  | Max idle connections to keep open                  |
| `"ssl"`       | `true` to require SSL/TLS                          |
| `"ssl_cert"`  | Path to client cert for mTLS                       |

```v2
// Connection pool for a web server
let pool = db_connect("postgres://user:pass@host/db", {
    "pool_size": 10,
    "timeout": 5000
})

// Prepared statement — compile once, run many times
let stmt = db_prepare(pool, "SELECT * FROM users WHERE id = ?")
let user = db_run(stmt, [42])

// Transaction with automatic rollback on error
db_transaction(pool, lambda(tx) {
    db_exec(tx, "UPDATE accounts SET balance = balance - ? WHERE id = ?", [100, 1])
    db_exec(tx, "UPDATE accounts SET balance = balance + ? WHERE id = ?", [100, 2])
    // if either throws, the whole transaction is rolled back
})
```

### Migrations

| Function                             | Description                                                   |
| ------------------------------------ | ------------------------------------------------------------- |
| `db_migrate(conn, dir)`              | Run all pending migration files in `dir` (sorted by filename) |
| `db_migrate_up(conn, dir, steps?)`   | Apply N migrations (default: all pending)                     |
| `db_migrate_down(conn, dir, steps?)` | Roll back N migrations (default: 1)                           |
| `db_migrate_status(conn, dir)`       | List migrations with `{name, applied, applied_at}`            |

Migration files are plain `.sql` files named with a numeric prefix (e.g. `001_create_users.sql`). Each file contains an `-- up` section and a `-- down` section:

```sql
-- up
CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT UNIQUE);

-- down
DROP TABLE users;
```

```v2
import "std.db"

let conn = db_connect("sqlite://app.db")
db_migrate(conn, "./migrations")    // applies all unapplied migrations
```

### Key-Value Store

| Function                    | Description                           |
| --------------------------- | ------------------------------------- |
| `kv_open(path)`             | Open or create a local KV store       |
| `kv_set(store, key, value)` | Set a key                             |
| `kv_get(store, key)`        | Get a key (returns `None` if missing) |
| `kv_delete(store, key)`     | Delete a key                          |
| `kv_keys(store, prefix?)`   | List keys                             |
| `kv_close(store)`           | Close the store                       |

### Example — SQLite Query

```v2
import "std.db"

let conn = db_connect("sqlite://app.db")

db_exec(conn, "CREATE TABLE IF NOT EXISTS users (id INTEGER PRIMARY KEY, name TEXT)")
db_exec(conn, "INSERT INTO users (name) VALUES (?)", ["Alice"])

let rows = db_query(conn, "SELECT * FROM users")
for (row in rows) {
    print(row["id"], row["name"])
}

db_close(conn)
```

---

## std.ui — User Interface

Cross-platform native GUI widgets.

```v2
import "std.ui"
```

### Window & App

| Function                       | Description                                         |
| ------------------------------ | --------------------------------------------------- |
| `ui_app(title, width, height)` | Create and run a UI application window              |
| `ui_set_layout(root, layout)`  | Set the root layout (`"column"`, `"row"`, `"grid"`) |
| `ui_render()`                  | Trigger a re-render of the UI                       |

### Widgets

| Function                                  | Description                                                                           |
| ----------------------------------------- | ------------------------------------------------------------------------------------- |
| `ui_label(text, opts?)`                   | Text label                                                                            |
| `ui_button(text, on_click)`               | Clickable button                                                                      |
| `ui_input(placeholder?, on_change)`       | Single-line text input field                                                          |
| `ui_textarea(placeholder?, on_change)`    | Multi-line text input                                                                 |
| `ui_checkbox(label, checked?, on_change)` | Checkbox                                                                              |
| `ui_radio(options, selected?, on_change)` | Radio button group                                                                    |
| `ui_slider(min, max, value?, on_change)`  | Slider                                                                                |
| `ui_dropdown(options, on_select)`         | Dropdown selector                                                                     |
| `ui_image(path_or_url, opts?)`            | Image widget                                                                          |
| `ui_divider()`                            | Horizontal separator                                                                  |
| `ui_container(children, opts?)`           | Layout container                                                                      |
| `ui_scroll(child, opts?)`                 | Scrollable container                                                                  |
| `ui_tabs(tabs, opts?)`                    | Tabbed view — `tabs` is list of `{label, content}`                                    |
| `ui_table(columns, rows, opts?)`          | Data table — `columns` is list of header strings, `rows` is list of lists             |
| `ui_progress(value, max?, opts?)`         | Progress bar (0.0—1.0 or 0—max)                                                       |
| `ui_modal(content, on_close?)`            | Modal overlay dialog                                                                  |
| `ui_tooltip(text, child)`                 | Wrap a widget with a hover tooltip                                                    |
| `ui_menu(items, opts?)`                   | Menu bar — `items` is list of `{label, on_click}` or `{label, children}` for submenus |
| `ui_spacer()`                             | Flexible space for pushing widgets apart                                              |

```v2
// Table example
ui_table(
    ["Name", "Age", "Role"],
    [
        ["Alice", "30", "Engineer"],
        ["Bob",   "25", "Designer"],
    ]
)

// Tabs example
ui_tabs([
    {"label": "Profile",  "content": profile_view()},
    {"label": "Settings", "content": settings_view()},
])

// Modal example
let modal = ui_modal(
    ui_container([
        ui_label("Are you sure?"),
        ui_button("Confirm", lambda() { confirm(); modal.close() }),
    ])
)
```

### Example — Counter App

```v2
import "std.ui"

let count = 0

ui_app("Counter", 300, 200)
ui_set_layout(ui_container([
    ui_label(f"Count: ${count}"),
    ui_button("Increment", lambda() {
        count += 1
        ui_render()
    }),
    ui_button("Reset", lambda() {
        count = 0
        ui_render()
    })
], {"align": "center"}))
```

---

## std.term — Terminal & ANSI

Terminal colors, styles, cursor control, and raw-mode input for CLI applications.

```v2
import "std.term"
```

### Colors & Styles

```v2
// Foreground colors
term.red("error!")           // prints in red
term.green("ok")
term.yellow("warning")
term.blue("info")
term.cyan("debug")
term.magenta("trace")
term.white("text")
term.gray("muted")

// Background colors
term.bg_red("alert")
term.bg_green("success")
term.bg_yellow("caution")
term.bg_blue("note")

// Text styles
term.bold("important")
term.italic("emphasis")
term.underline("link")
term.strikethrough("removed")
term.dim("secondary")

// Chaining styles
term.bold(term.red("CRITICAL ERROR"))

// Reset all styles
term.reset()
```

### Using Color in f-strings

```v2
print(f"${term.green('?')} All tests passed")
print(f"${term.red('?')} ${count} failures")
```

### Cursor Control

```v2
term.move_to(row, col)       // move cursor to row, col (1-indexed)
term.move_up(n)              // move cursor up N lines
term.move_down(n)
term.move_left(n)
term.move_right(n)
term.save_cursor()           // save cursor position
term.restore_cursor()        // restore saved position
term.hide_cursor()
term.show_cursor()
term.clear_screen()          // clear the terminal
term.clear_line()            // clear current line
term.clear_to_end()          // clear from cursor to end of line
```

### Terminal Size

```v2
let size = term.size()
print(size["cols"], size["rows"])    // e.g. 220 50
```

### Raw Mode & Key Input

```v2
term.raw_mode(true)    // disable line buffering and echo

let key = term.read_key()    // blocks until a key is pressed
// key is a string: "a", "A", "?", "?", "?", "?", "Enter", "Escape", "Ctrl+C", etc.

term.raw_mode(false)   // restore normal terminal mode
```

### Progress Bar

```v2
let bar = term.progress_bar(total: 100, width: 40)

for (i in 0..100) {
    bar.update(i + 1)
    sleep(20)
}
bar.finish("Done!")
```

### Spinner

```v2
let spinner = term.spinner("Loading...")
spinner.start()
// ... do work ...
spinner.stop("Complete!")
```

### Color Detection

```v2
term.supports_color()     // true if the terminal supports ANSI colors
term.color_depth()        // 1 (no color), 8, 256, or 16_000_000 (truecolor)
```

### Style Builder API

```v2
let style = term.style()
    .fg(term.Color.Red)
    .bg(term.Color.Black)
    .bold()
    .underline()

print(style.apply("ALERT"))
```

---

## std.cli — CLI Argument Parsing

Structured command-line argument parsing with subcommands, flags, validation, and help generation.

```v2
import "std.cli"
```

### Basic Usage

```v2
let app = cli.app("myapp", "A tool that does things")

app.flag("--verbose", "-v", "Enable verbose output", default: false)
app.option("--output", "-o", "Output file path", default: "out.txt")
app.arg("input", "Input file to process", required: true)

let args = app.parse()    // parses proc.args() automatically

if (args["verbose"]) {
    print("Verbose mode on")
}
let output = args["output"]
let input = args["input"]
```

### Argument Types

| Method                                            | Description                                   |
| ------------------------------------------------- | --------------------------------------------- |
| `app.flag(long, short?, help, default?)`          | Boolean flag (`--verbose` ? `true`)           |
| `app.option(long, short?, help, default?, type?)` | Named option with a value                     |
| `app.arg(name, help, required?)`                  | Positional argument                           |
| `app.multi_arg(name, help)`                       | Collect remaining positional args into a list |

### Type Coercion

```v2
app.option("--count", help: "Number of items", type: "int", default: 10)
app.option("--scale", help: "Scale factor",   type: "float", default: 1.0)
app.option("--mode",  help: "Run mode",       type: "str",   choices: ["fast", "safe"])

let args = app.parse()
print(args["count"] + 1)    // int arithmetic — no manual conversion needed
```

### Subcommands

```v2
let app = cli.app("git-like", "Version control tool")

let init_cmd = app.subcommand("init", "Initialize a repository")
init_cmd.option("--bare", help: "Create a bare repo", default: false)

let clone_cmd = app.subcommand("clone", "Clone a repository")
clone_cmd.arg("url", "Repository URL", required: true)
clone_cmd.option("--depth", help: "Shallow clone depth", type: "int")

let args = app.parse()

match (args["subcommand"]) {
    case ("init") { init_repo(args["bare"]) }
    case ("clone") { clone_repo(args["url"], args["depth"]) }
    case (null) { app.print_help() }
}
```

### Validation

```v2
app.option("--port", help: "Port number", type: "int", default: 8080,
    validate: lambda(v) => v >= 1 && v <= 65535 || "Port must be 1—65535")

// Custom validator — return true (valid) or a string (error message)
```

### Help Generation

`app.parse()` automatically exits with a formatted help message when `--help` or `-h` is passed. You can also call it manually:

```v2
app.print_help()    // prints help and exits 0
app.print_usage()   // prints one-line usage synopsis
```

### Parsing from a Custom Argv

```v2
let args = app.parse(["--verbose", "--output", "result.txt", "input.json"])
```

### Full Example

```v2
import "std.cli"

let app = cli.app("compress", "File compression tool")
app.flag("--verbose", "-v", "Show progress")
app.option("--level", "-l", "Compression level (1—9)", type: "int", default: 6,
    validate: lambda(n) => n >= 1 && n <= 9 || "Level must be 1—9")
app.option("--format", "-f", "Output format", choices: ["gz", "bz2", "xz"], default: "gz")
app.arg("input", "File to compress", required: true)

let args = app.parse()

if (args["verbose"]) {
    print(f"Compressing ${args['input']} at level ${args['level']} using ${args['format']}")
}
compress_file(args["input"], args["level"], args["format"])
```

---

## std.csv — CSV Parsing & Writing

Reading and writing Comma-Separated Value files with header support, custom delimiters, and streaming.

```v2
import "std.csv"
```

### Parsing a CSV String

```v2
let data = csv.parse("name,age,city\nAlice,30,Warsaw\nBob,25,London")

// data is a list of dicts (header row becomes keys)
data[0]    // {"name": "Alice", "age": "30", "city": "Warsaw"}
data[1]    // {"name": "Bob",   "age": "25", "city": "London"}
```

### Parsing a CSV File

```v2
let rows = csv.read("users.csv")

for (row in rows) {
    print(row["name"], row["age"])
}
```

### Writing CSV

```v2
let rows = [
    {"name": "Alice", "age": 30, "city": "Warsaw"},
    {"name": "Bob",   "age": 25, "city": "London"},
]

let text = csv.stringify(rows)
// "name,age,city\nAlice,30,Warsaw\nBob,25,London\n"

csv.write("output.csv", rows)
```

### Custom Delimiters and Options

```v2
// TSV (tab-separated)
let rows = csv.read("data.tsv", {"delimiter": "\t"})

// Semicolon delimiter, no header row
let rows = csv.parse(raw, {
    "delimiter": ";",
    "header": false    // returns list of lists instead of list of dicts
})

// Custom quote character
let rows = csv.parse(raw, {"quote": "'"})
```

### Schema Mapping & Validation

```v2
let schema = csv.schema(
    headers: ["name", "age", "city"],
    types: {"name": "str", "age": "int", "city": "str"}
)

let row = {"name": "Alice", "age": "30", "city": "Warsaw"}
let checked = csv.validate(row, schema)
// checked["age"] is now typed/coerced according to schema rules
```

### Streaming Large Files

```v2
let reader = csv.open_reader("huge.csv")
let header = reader.header()    // ["name", "age", "city"]

for (row in reader) {
    process(row)    // row is a dict; rows are streamed, not loaded all at once
}

reader.close()
```

### Writing with Custom Column Order

```v2
csv.write("out.csv", rows, columns: ["city", "name", "age"])
// writes columns in that order regardless of dict key order
```

### API Reference

| Function                       | Description                                       |
| ------------------------------ | ------------------------------------------------- |
| `csv.parse(text, opts?)`       | Parse CSV string ? list of dicts or list of lists |
| `csv.stringify(rows, opts?)`   | Serialize list of dicts/lists ? CSV string        |
| `csv.read(path, opts?)`        | Read a CSV file ? list of dicts                   |
| `csv.write(path, rows, opts?)` | Write list of dicts to CSV file                   |
| `csv.open_reader(path, opts?)` | Open a streaming reader                           |
| `csv.schema(headers, types)`   | Build typed column schema for row projection      |
| `csv.validate(row, schema)`    | Validate/coerce row using schema rules            |
| `reader.header()`              | Get column names                                  |
| `reader.close()`               | Close the stream                                  |

---

## std.toml — TOML Parsing

Parse and generate TOML configuration files (the same format used by `v2.toml`).

```v2
import "std.toml"
```

### Parsing TOML

```v2
let config = toml.parse("""
[server]
host = "localhost"
port = 8080
tls  = false

[database]
url  = "postgres://user:pass@host/db"
pool = 10
""")

config["server"]["host"]     // "localhost"
config["server"]["port"]     // 8080  (int)
config["database"]["pool"]   // 10
```

### Reading a TOML File

```v2
let cfg = toml.read("config.toml")
```

### Writing TOML

```v2
let data = {
    "project": {"name": "myapp", "version": "1.0.0"},
    "dependencies": {"http-utils": "1.2.0"}
}

let text = toml.stringify(data)
toml.write("v2.toml", data)
```

### Type Mapping

| TOML type | V2 type                 |
| --------- | ----------------------- |
| String    | `str`                   |
| Integer   | `int`                   |
| Float     | `float`                 |
| Boolean   | `bool`                  |
| Array     | `list`                  |
| Table     | `dict`                  |
| Datetime  | `str` (ISO 8601 format) |

### API Reference

| Function                | Description                  |
| ----------------------- | ---------------------------- |
| `toml.parse(text)`      | Parse TOML string ? dict     |
| `toml.read(path)`       | Read and parse a TOML file   |
| `toml.stringify(val)`   | Serialize dict ? TOML string |
| `toml.write(path, val)` | Write dict to a TOML file    |

---

## std.yaml — YAML Parsing

Parse and generate YAML documents, including multi-document files.

```v2
import "std.yaml"
```

### Parsing YAML

```v2
let config = yaml.parse("""
server:
  host: localhost
  port: 8080
  tags:
    - web
    - api
database:
  url: postgres://user:pass@host/db
  pool: 10
""")

config["server"]["host"]      // "localhost"
config["server"]["tags"]      // ["web", "api"]
config["database"]["pool"]    // 10
```

### Reading a YAML File

```v2
let cfg = yaml.read("config.yaml")
```

### Writing YAML

```v2
let data = {
    "server": {"host": "example.com", "port": 443},
    "tls": true
}

let text = yaml.stringify(data)
yaml.write("config.yaml", data)
```

### Multi-Document Files

A YAML file may contain multiple documents separated by `---`:

```v2
let docs = yaml.parse_all("""
---
name: Alice
role: admin
---
name: Bob
role: user
""")

docs[0]["name"]    // "Alice"
docs[1]["name"]    // "Bob"
```

### API Reference

| Function                       | Description                                |
| ------------------------------ | ------------------------------------------ |
| `yaml.parse(text)`             | Parse YAML string ? dict or list           |
| `yaml.parse_all(text)`         | Parse multi-document YAML ? list of values |
| `yaml.read(path)`              | Read and parse a YAML file                 |
| `yaml.stringify(val, opts?)`   | Serialize ? YAML string                    |
| `yaml.write(path, val, opts?)` | Write to a YAML file                       |

Stringify options:

| Key           | Default | Description                   |
| ------------- | ------- | ----------------------------- |
| `"indent"`    | `2`     | Indentation spaces            |
| `"flow"`      | `false` | Use inline (flow) style       |
| `"sort_keys"` | `false` | Sort dict keys alphabetically |

---

## std.uuid — UUID Generation

Generate and parse Universally Unique Identifiers (UUIDs).

```v2
import "std.uuid"
```

### Generating UUIDs

```v2
let id = uuid.v4()           // "f47ac10b-58cc-4372-a567-0e02b2c3d479"
let id2 = uuid.v4()          // different every call — cryptographically random

let id_v1 = uuid.v1()        // time-based UUID
let id_v7 = uuid.v7()        // time-ordered UUID (sortable, recommended for DB keys)
```

### Format Variants

```v2
uuid.v4()                    // "550e8400-e29b-41d4-a716-446655440000"
uuid.v4_compact()            // "550e8400e29b41d4a716446655440000" (no hyphens)
uuid.v4_urn()                // "urn:uuid:550e8400-e29b-41d4-a716-446655440000"
uuid.nil()                   // "00000000-0000-0000-0000-000000000000"
```

### Parsing and Validation

```v2
let parsed = uuid.parse("550e8400-e29b-41d4-a716-446655440000")
// Returns a UUID object, or throws ParseError if invalid

uuid.is_valid("not-a-uuid")  // false
uuid.is_valid(uuid.v4())     // true

let u = uuid.parse(raw_str)
u.version()    // 4
u.str()        // "550e8400-e29b-41d4-a716-446655440000"
u.compact()    // "550e8400e29b41d4a716446655440000"
```

### Comparison

```v2
let a = uuid.v4()
let b = uuid.v4()

uuid.equals(a, b)     // false (almost certainly)
uuid.compare(a, b)    // -1 | 0 | 1 — lexicographic order
```

### API Reference

| Function             | Description                                 |
| -------------------- | ------------------------------------------- | --- | --- |
| `uuid.v4()`          | Random UUID (recommended for general use)   |
| `uuid.v1()`          | Time-based UUID                             |
| `uuid.v7()`          | Time-ordered, monotonically increasing UUID |
| `uuid.nil()`         | All-zeros UUID                              |
| `uuid.v4_compact()`  | UUID v4 without hyphens                     |
| `uuid.v4_urn()`      | UUID v4 with URN prefix                     |
| `uuid.parse(s)`      | Parse UUID string ? UUID object             |
| `uuid.is_valid(s)`   | Validate UUID string                        |
| `uuid.equals(a, b)`  | Compare two UUIDs for equality              |
| `uuid.compare(a, b)` | Lexicographic comparison ? `-1              | 0   | 1`  |

---

## std.rand — Random Numbers

Comprehensive random number generation: distributions, seeding, secure randomness, and shuffling.

```v2
import "std.rand"
```

### Basic Random Values

```v2
rand.float()               // uniform float in [0.0, 1.0)
rand.float(min, max)       // uniform float in [min, max)
rand.int(min, max)         // uniform int in [min, max] inclusive
rand.bool()                // true or false with equal probability
rand.bool(p)               // true with probability p (0.0—1.0)
```

### Seeded Generator

```v2
let rng = rand.new(seed: 42)    // deterministic, reproducible

rng.float()
rng.int(0, 100)
rng.bool()
```

### Collections

```v2
rand.choice([1, 2, 3, 4, 5])           // random element
rand.choices([1, 2, 3], k: 2)          // sample 2 elements (with replacement)
rand.sample([1, 2, 3, 4, 5], k: 3)    // 3 unique elements (without replacement)

let deck = list(range(52))
rand.shuffle(deck)                      // Fisher-Yates in-place shuffle

rand.weighted_choice(
    ["apple", "banana", "cherry"],
    weights: [0.5, 0.3, 0.2]           // probabilities must sum to 1.0
)
```

### Distributions

```v2
rand.normal(mean: 0.0, std: 1.0)        // standard normal (Gaussian)
rand.exponential(rate: 1.0)             // exponential distribution
rand.poisson(lam: 5.0)                  // Poisson distribution ? int
rand.binomial(n: 10, p: 0.5)           // binomial ? int
rand.uniform(a: 0.0, b: 1.0)           // alias for rand.float(a, b)
rand.triangular(low, high, mode)        // triangular distribution
```

### Secure Random (Cryptographic)

```v2
// Cryptographically secure — uses OS entropy source
rand.secure_bytes(n)        // n random bytes as a list
rand.secure_int(min, max)   // cryptographically secure int
rand.secure_token(n: 32)    // URL-safe base64 random token of n bytes
```

### API Reference

| Function                              | Description                    |
| ------------------------------------- | ------------------------------ |
| `rand.float(min?, max?)`              | Uniform float                  |
| `rand.int(min, max)`                  | Uniform integer                |
| `rand.bool(p?)`                       | Random boolean                 |
| `rand.choice(list)`                   | Random element                 |
| `rand.choices(list, k)`               | k samples with replacement     |
| `rand.sample(list, k)`                | k unique samples               |
| `rand.shuffle(list)`                  | In-place shuffle               |
| `rand.weighted_choice(list, weights)` | Weighted random selection      |
| `rand.normal(mean, std)`              | Normal distribution            |
| `rand.exponential(rate)`              | Exponential distribution       |
| `rand.poisson(lam)`                   | Poisson distribution           |
| `rand.binomial(n, p)`                 | Binomial distribution          |
| `rand.secure_bytes(n)`                | Cryptographically secure bytes |
| `rand.secure_token(n)`                | URL-safe random token          |
| `rand.new(seed?)`                     | Create a seeded RNG instance   |

---

## std.hash — Non-Cryptographic Hashing

Fast, non-cryptographic hash functions for hash tables, checksums, deduplication, and content addressing. For cryptographic hashing (SHA-256, bcrypt, etc.), use `std.crypto`.

```v2
import "std.hash"
```

### One-Shot Hashing

```v2
hash.fnv1a("hello")          // 1335831723  — fast, good distribution
hash.djb2("hello")           // 210700827
hash.murmur3("hello")        // 613153351
hash.xxhash("hello")         // 2794345569  — extremely fast, excellent quality
hash.crc32("hello")          // 907060870
hash.crc32c("hello")         // hardware-accelerated variant (Castagnoli)
hash.adler32("hello")        // 103547413
```

### Bytes and Binary Data

```v2
hash.xxhash(data, seed: 0)            // hash a list of bytes
hash.xxhash64(data)                   // 64-bit variant
hash.crc32(data)                      // CRC-32 of bytes
```

### Streaming Hasher

For large files or data that arrives incrementally:

```v2
let h = hash.hasher("xxhash")

for (chunk in file_chunks) {
    h.update(chunk)
}

let result = h.digest()    // final hash as int
let hex    = h.hex()       // hex string
```

### Content-Based IDs

```v2
// Stable ID for a value — same content always produces the same hash
hash.content_id({"name": "Alice", "age": 30})    // deterministic dict hash
hash.content_id([1, 2, 3])
```

### Bloom Filter

```v2
let bloom = hash.bloom_filter(capacity: 10_000, error_rate: 0.01)

bloom.add("user:1")
bloom.add("user:2")

bloom.contains("user:1")    // true (definitely)
bloom.contains("user:99")   // false (probably — small false-positive rate)

bloom.estimated_count()     // approximate number of items added
```

### API Reference

| Function                                  | Description                                |
| ----------------------------------------- | ------------------------------------------ |
| `hash.fnv1a(data)`                        | FNV-1a 32-bit hash                         |
| `hash.djb2(data)`                         | DJB2 hash                                  |
| `hash.murmur3(data, seed?)`               | MurmurHash3 32-bit                         |
| `hash.xxhash(data, seed?)`                | xxHash 32-bit — fastest general-purpose    |
| `hash.xxhash64(data, seed?)`              | xxHash 64-bit                              |
| `hash.crc32(data)`                        | CRC-32 (IEEE)                              |
| `hash.crc32c(data)`                       | CRC-32C (Castagnoli, hardware-accelerated) |
| `hash.adler32(data)`                      | Adler-32                                   |
| `hash.content_id(val)`                    | Deterministic hash of any V2 value         |
| `hash.hasher(algo)`                       | Create a streaming hasher                  |
| `hash.bloom_filter(capacity, error_rate)` | Probabilistic set membership               |

---

## std.cache — In-Memory Caching

TTL-based key-value caching with LRU eviction, memoization helpers, and optional persistence.

```v2
import "std.cache"
```

### Basic Usage

```v2
let c = cache.new()

c.set("user:1", {"name": "Alice"})
c.get("user:1")              // {"name": "Alice"}
c.get("user:99")             // None

c.has("user:1")              // true
c.delete("user:1")
c.clear()
```

### TTL (Time-to-Live)

```v2
let c = cache.new()

c.set("session:abc", token, ttl: 3600)    // expires in 3600 seconds
sleep(3601_000)
c.get("session:abc")                       // None — expired

// Default TTL for all entries
let c2 = cache.new(ttl: 300)    // 5 minutes default
c2.set("key", value)            // inherits 300s TTL
c2.set("perm", value, ttl: 0)  // ttl=0 means no expiry
```

### LRU Eviction

```v2
// Cap at 1000 entries — oldest-accessed are evicted first
let c = cache.new(max_size: 1000)

c.set("a", 1)
// ... add many entries ...
// When max_size is exceeded, least-recently-used entries are dropped automatically
```

### Memoization Helper

```v2
// Automatically cache the results of a function call by its arguments
let cached_fetch = cache.memoize(fetch_user, ttl: 60)

let user = cached_fetch(42)    // fetches and caches
let user2 = cached_fetch(42)   // returns cached result instantly
```

### Namespace Support

```v2
let users  = cache.namespace(c, "users:")
let tokens = cache.namespace(c, "tokens:")

users.set("1", alice)
tokens.set("abc", token)
// Stored as "users:1" and "tokens:abc" in the backing cache
```

### Stats

```v2
let stats = c.stats()
stats["hits"]         // number of successful gets
stats["misses"]       // number of failed gets
stats["size"]         // current number of entries
stats["hit_rate"]     // float 0.0—1.0
```

### Persistence

```v2
// Persist cache to disk and reload across restarts
let c = cache.new(persist: "cache.db")    // backed by local KV store
```

### API Reference

| Function                     | Description                             |
| ---------------------------- | --------------------------------------- |
| `cache.new(opts?)`           | Create a new cache                      |
| `c.set(key, val, ttl?)`      | Set a value with optional TTL (seconds) |
| `c.get(key)`                 | Get a value or `None`                   |
| `c.has(key)`                 | Check if key exists and not expired     |
| `c.delete(key)`              | Remove a key                            |
| `c.clear()`                  | Remove all entries                      |
| `c.stats()`                  | Get hit/miss stats                      |
| `cache.memoize(fn, ttl?)`    | Return a memoized version of `fn`       |
| `cache.namespace(c, prefix)` | Namespaced view of an existing cache    |

Cache options:

| Key          | Default | Description                            |
| ------------ | ------- | -------------------------------------- |
| `"ttl"`      | `null`  | Default TTL in seconds for all entries |
| `"max_size"` | `null`  | Max entries before LRU eviction        |
| `"persist"`  | `null`  | File path for disk persistence         |

---

## std.signal — OS Signal Handling

Register handlers for POSIX signals (`SIGINT`, `SIGTERM`, `SIGHUP`, etc.) and gracefully manage process lifecycle.

```v2
import "std.signal"
```

`std.signal` is the canonical high-level signal API. The signal helpers in `std.os` (`os.on_signal`, `os.send_signal`, etc.) are thin convenience wrappers.

### Registering a Signal Handler

```v2
signal.on("SIGINT", lambda() {
    print("\nShutting down...")
    cleanup()
    exit(0)
})

// Keep the program running
while (true) {
    // do work...
    sleep(1000)
}
```

### Common Signals

| Signal    | Typical meaning                                |
| --------- | ---------------------------------------------- |
| `SIGINT`  | Ctrl+C — interactive interrupt                 |
| `SIGTERM` | Graceful shutdown requested (e.g. from `kill`) |
| `SIGHUP`  | Terminal disconnected / config reload          |
| `SIGPIPE` | Broken pipe (write to closed socket)           |
| `SIGUSR1` | User-defined signal 1                          |
| `SIGUSR2` | User-defined signal 2                          |
| `SIGCHLD` | Child process state changed                    |
| `SIGALRM` | Alarm clock (from `signal.alarm()`)            |

> **Windows note:** Windows only supports `SIGINT` and `SIGTERM` natively. All other signals are ignored on Windows builds.

### Portable Signal Events

Use portable lifecycle events when you need identical behavior across platforms:

```v2
signal.on_portable("shutdown", lambda() {
    save_state()
    exit(0)
})

signal.on_portable("reload", lambda() {
    reload_config()
})
```

Portable event mapping:

| Portable event | Linux/macOS mapping  | Windows mapping              |
| -------------- | -------------------- | ---------------------------- |
| `shutdown`     | `SIGTERM` / `SIGINT` | Console close / Ctrl+C       |
| `reload`       | `SIGHUP`             | Service reload control event |
| `child_exit`   | `SIGCHLD`            | Process wait notification    |
| `user1`        | `SIGUSR1`            | Named event channel          |
| `user2`        | `SIGUSR2`            | Named event channel          |

`signal.emit_portable(name)` is available for testing and local orchestration.

### Removing a Handler

```v2
let id = signal.on("SIGTERM", lambda() { graceful_stop() })

// Later — remove this specific handler
signal.off("SIGTERM", id)

// Or remove all handlers for a signal
signal.reset("SIGTERM")
```

### One-Shot Handler

```v2
signal.once("SIGUSR1", lambda() {
    reload_config()    // called exactly once, then auto-removed
})
```

### Default Actions

Signals that have no registered handler use the OS default (e.g. `SIGTERM` terminates the process, `SIGPIPE` terminates, `SIGHUP` hangs up). Call `signal.ignore(name)` to explicitly suppress a signal:

```v2
signal.ignore("SIGPIPE")    // common for network servers
```

### Alarm / Timed Signal

```v2
signal.alarm(seconds: 5)    // fire SIGALRM in 5 seconds
signal.on("SIGALRM", lambda() {
    print("5 seconds elapsed")
})
```

### Graceful Server Shutdown Pattern

```v2
import "std.signal"
import "std.net"

let server = http_serve(8080, handler)

signal.on("SIGTERM", lambda() {
    log.info("SIGTERM received — shutting down")
    server.stop(grace_period: 30)    // finish in-flight requests, timeout 30s
    exit(0)
})

signal.on("SIGINT", lambda() {
    log.info("SIGINT — immediate shutdown")
    exit(1)
})
```

### API Reference

| Function                     | Description                                      |
| ---------------------------- | ------------------------------------------------ |
| `signal.on(name, handler)`   | Register a handler ? returns handler ID          |
| `signal.once(name, handler)` | Register a one-shot handler                      |
| `signal.off(name, id)`       | Remove a specific handler by ID                  |
| `signal.reset(name)`         | Remove all handlers, restore OS default          |
| `signal.ignore(name)`        | Suppress a signal (no handler called)            |
| `signal.raise(name)`         | Send a signal to the current process             |
| `signal.alarm(seconds)`      | Schedule SIGALRM after `seconds` seconds         |
| `signal.list()`              | List all signal names available on this platform |

### Hardware Fault Signals

> **Implementation status — Milestone 1 (current):** `signal.on_fault` registration, OS-level handler installation (POSIX `sigaction` / Windows VEH), `FaultInfo` capture, `signal.dump_json`, and `signal.dump_core` are implemented. `signal.set_recovery_point` and `signal.recover` remain **planned** (Milestone 2) — see `FAULT_HANDLING_DESIGN.md`. The `backtrace` and `registers` fields in `FaultInfo` are captured as placeholders in Milestone 1.

V2 can trap hardware fault signals (`SIGSEGV`, `SIGBUS`, `SIGFPE`, `SIGABRT`) that normally cause immediate process termination. Because recovery from hardware faults is **inherently unsafe** — the process may be in a corrupt state — these handlers are registered through a separate API and require an `unsafe` acknowledgment.

#### Registering a Fault Handler

```v2
import std.signal

unsafe {
    signal.on_fault("SIGSEGV", func(info) {
        log.error(f"segfault at address ${info.address}")
        log.error(f"  instruction: ${info.pc}")
        log.error(f"  backtrace:\n${info.backtrace}")

        // Generate a crash dump before terminating
        signal.dump_core("crash.dump")
        exit(128 + 11)
    })
}
```

`signal.on_fault` differs from `signal.on` in three ways:

1. It only accepts the four hardware fault signals: `SIGSEGV`, `SIGBUS`, `SIGFPE`, `SIGABRT`.
2. The handler receives a `FaultInfo` object with crash context.
3. The call must be inside an `unsafe` block.

#### `FaultInfo` Object

The handler receives a `FaultInfo` with the following fields:

| Field               | Type   | Description                                         |
| ------------------- | ------ | --------------------------------------------------- |
| `signal`            | `str`  | Signal name (`"SIGSEGV"`, `"SIGBUS"`, etc.)         |
| `address`           | `int?` | Faulting memory address (null if not available)     |
| `pc`                | `int?` | Program counter / instruction pointer at fault time |
| `backtrace`         | `str`  | Symbolicated stack trace at the point of the fault  |
| `thread_id`         | `int`  | OS thread ID where the fault occurred               |
| `registers`         | `dict` | CPU register snapshot (`{"rax": ..., "rbx": ...}`)  |
| `is_stack_overflow` | `bool` | `true` if the signal was caused by stack exhaustion |

#### Fault Signal Table

| Signal    | Cause                                                    |
| --------- | -------------------------------------------------------- |
| `SIGSEGV` | Invalid memory access (null dereference, use-after-free) |
| `SIGBUS`  | Misaligned memory access, non-existent physical address  |
| `SIGFPE`  | Arithmetic fault (integer divide-by-zero, overflow trap) |
| `SIGABRT` | Explicit abort (assertion failure, `abort()` call)       |

#### Recovery vs. Termination

**Recovery from hardware faults is not generally safe.** The default behavior after a fault handler returns is to terminate the process. If you want to perform cleanup (flush logs, write crash dumps, notify a monitoring service), do so inside the handler.

To attempt recovery (re-execution after the faulting instruction), use `signal.recover()` inside the handler. This is **deeply unsafe** — it uses `longjmp` under the hood and can leave data structures in an inconsistent state:

```v2
import std.signal

unsafe {
    let checkpoint = signal.set_recovery_point()

    if (checkpoint.recovered) {
        print("recovered from a fault — continuing cautiously")
    } else {
        // Normal execution path
        let ptr = null
        deref(ptr)    // this will SIGSEGV
    }
}
```

`signal.set_recovery_point()` works like `setjmp` — it returns a `RecoveryPoint` whose `.recovered` field is `false` on the first call, and `true` when control returns after a fault. The fault handler must call `signal.recover()` to jump back.

#### Crash Dump Generation

```v2
unsafe {
    signal.on_fault("SIGSEGV", func(info) {
        // Write a minidump compatible with platform-native debuggers
        signal.dump_core("crash.dump")

        // Or a structured JSON crash report
        signal.dump_json("crash.json")

        exit(1)
    })
}
```

`signal.dump_core` is implemented and writes a `CORE_DUMP_V2` crash report payload (including captured `FaultInfo`) to the requested path. Full platform-native dump generation (ELF core on Linux, Mach-O on macOS, minidump on Windows) is still planned for a later milestone. `signal.dump_json` generates a JSON report and is implemented; the `backtrace`, `registers`, and module-list fields are placeholders until full symbolication is added.

#### Fault Handler API Reference

| Function                         | Description                                         |
| -------------------------------- | --------------------------------------------------- |
| `signal.on_fault(name, handler)` | Register a hardware fault handler (requires unsafe) |
| `signal.set_recovery_point()`    | Set a `setjmp`-style recovery point (unsafe)        |
| `signal.recover()`               | Jump back to recovery point from fault handler      |
| `signal.dump_core(path)`         | Write a platform-native core dump                   |
| `signal.dump_json(path)`         | Write a JSON crash report                           |

---

## std.http — HTTP Client & Server

A full-featured HTTP client and server with WebSockets, middleware, routing, and HTTP/2 support. For quick one-liner HTTP calls, the `http_get` / `http_post` builtins in `std.net` suffice. Import `std.http` for production-grade servers and clients.

```v2
import "std.http"
```

### HTTP Client

```v2
// GET
let resp = await http.get("https://api.example.com/users")
print(resp.status)          // 200
print(resp.body)            // raw string
print(resp.json())          // parsed JSON
print(resp.headers)         // dict of response headers

// POST with JSON body
let resp2 = await http.post("https://api.example.com/users", {
    "body": json_stringify({"name": "Alice"}),
    "headers": {"Content-Type": "application/json"}
})

// PUT, PATCH, DELETE
await http.put(url, opts)
await http.patch(url, opts)
await http.delete(url)
```

### Request Options

```v2
let resp = await http.get(url, {
    "headers":  {"Authorization": f"Bearer ${token}"},
    "timeout":  5000,           // ms; throws TimeoutError if exceeded
    "follow_redirects": true,   // default true
    "verify_ssl": true,         // default true
    "proxy": "http://proxy:8080"
})
```

### HTTP Client Object (Persistent Sessions)

```v2
let client = http.client({
    "base_url": "https://api.example.com",
    "headers":  {"Authorization": f"Bearer ${token}"},
    "timeout":  10_000
})

let users = await client.get("/users")
let user  = await client.get(f"/users/${id}")
await client.post("/users", body: json_stringify(new_user))
client.close()
```

### HTTP Server

```v2
let server = http.server(8080)

server.get("/", lambda(req, res) {
    res.send("Hello, World!")
})

server.get("/users/:id", lambda(req, res) {
    let id = req.params["id"]
    let user = db_find_user(int(id))
    match (user) {
        case (Some(u)) { res.json(u) }
        case (None) { res.status(404).send("Not found") }
    }
})

server.post("/users", lambda(req, res) {
    let body = req.json()
    let user = create_user(body)
    res.status(201).json(user)
})

server.start()
```

### Routing

```v2
// Route groups
let api = server.group("/api/v1", [auth_middleware])
api.get("/users", list_users_handler)
api.post("/users", create_user_handler)

// Wildcard and multi-segment
server.get("/files/*path", static_files_handler)
```

### Middleware

```v2
// Middleware is a function (req, res, next) => void
func auth_middleware(req, res, next) {
    let token = req.headers.get("Authorization", "")
    if (!verify_token(token)) {
        res.status(401).send("Unauthorized")
        return    // do NOT call next() — stops the chain
    }
    req.user = decode_token(token)    // attach to request
    next()
}

func logger_middleware(req, res, next) {
    let start = time()
    next()
    log.info(f"${req.method} ${req.path} ? ${res.status_code} (${time() - start:.1f}ms)")
}

// Apply globally
server.use(logger_middleware)
server.use(auth_middleware)

// Or per-route
server.get("/admin", [auth_middleware, admin_check], handler)
```

### Request & Response Objects

**Request fields:**

| Field         | Description                     |
| ------------- | ------------------------------- |
| `req.method`  | `"GET"`, `"POST"`, etc.         |
| `req.path`    | URL path                        |
| `req.params`  | Dict of URL path parameters     |
| `req.query`   | Dict of query string parameters |
| `req.headers` | Dict of request headers         |
| `req.body`    | Raw body string                 |
| `req.json()`  | Parse body as JSON              |
| `req.form()`  | Parse body as URL-encoded form  |

**Response methods:**

| Method                     | Description                           |
| -------------------------- | ------------------------------------- |
| `res.send(body)`           | Send a plain text response            |
| `res.json(val)`            | Serialize and send JSON               |
| `res.status(n)`            | Set HTTP status code (chainable)      |
| `res.header(k, v)`         | Set a response header (chainable)     |
| `res.redirect(url, code?)` | Redirect (default 302)                |
| `res.file(path)`           | Send a file with correct Content-Type |
| `res.stream(generator)`    | Send response as a stream             |

### Server-Sent Events (SSE)

```v2
// SSE Server — stream events to a connected client
server.get("/events", lambda(req, res) {
    res.sse_start()    // sets Content-Type: text/event-stream and begins stream

    for (i in 0..10) {
        res.sse_send({"data": f"update ${i}", "event": "progress"})
        sleep(1000)
    }
    res.sse_end()
})

// SSE Client — consume a server-sent event stream
let stream = await http.sse_connect("https://example.com/events")
for await (event in stream) {
    print(event["event"], event["data"])
}
stream.close()
```

| Method                         | Description                                                 |
| ------------------------------ | ----------------------------------------------------------- |
| `res.sse_start()`              | Begin an SSE response (sets headers, enters streaming mode) |
| `res.sse_send(msg)`            | Send one SSE event — `msg` is `{data, event?, id?, retry?}` |
| `res.sse_end()`                | Close the SSE stream                                        |
| `http.sse_connect(url, opts?)` | Connect to an SSE endpoint ? async generator of event dicts |

### WebSockets

```v2
server.ws("/chat", lambda(socket) {
    socket.on("message", lambda(msg) {
        print(f"Received: ${msg}")
        socket.send(f"Echo: ${msg}")
    })

    socket.on("close", lambda() {
        print("Client disconnected")
    })
})

// Client-side WebSocket
let ws = await http.ws_connect("wss://example.com/chat")
ws.send("hello")
let msg = await ws.recv()
ws.close()
```

### HTTP/2 & TLS

```v2
let server = http.server(443, {
    "tls": {
        "cert": "cert.pem",
        "key":  "key.pem"
    },
    "http2": true
})
```

### Server Lifecycle & API Reference

**Server object methods:**

| Method                               | Description                                                                                          |
| ------------------------------------ | ---------------------------------------------------------------------------------------------------- |
| `server.get(path, handler)`          | Register a GET route                                                                                 |
| `server.post(path, handler)`         | Register a POST route                                                                                |
| `server.put(path, handler)`          | Register a PUT route                                                                                 |
| `server.patch(path, handler)`        | Register a PATCH route                                                                               |
| `server.delete(path, handler)`       | Register a DELETE route                                                                              |
| `server.ws(path, handler)`           | Register a WebSocket upgrade route                                                                   |
| `server.use(middleware)`             | Register global middleware                                                                           |
| `server.group(prefix, middlewares?)` | Create a route group with optional middleware                                                        |
| `server.static(path, dir)`           | Serve static files from `dir` under `path`                                                           |
| `server.start()`                     | Start listening — blocks until stopped                                                               |
| `server.start_async()`               | Start in the background (non-blocking) — returns a Promise                                           |
| `server.stop(grace_period?)`         | Gracefully stop: finish in-flight requests, then shut down. `grace_period` is seconds (default `30`) |
| `server.address()`                   | Returns `{host, port}` of the bound socket                                                           |

**`http.server` constructor options:**

| Key          | Default     | Description                                   |
| ------------ | ----------- | --------------------------------------------- |
| `"host"`     | `"0.0.0.0"` | Interface to bind to                          |
| `"tls"`      | `null`      | TLS config dict `{cert, key}` — enables HTTPS |
| `"http2"`    | `false`     | Enable HTTP/2 (requires TLS)                  |
| `"max_body"` | `1MB`       | Max request body size in bytes                |
| `"timeout"`  | `30000`     | Request timeout in milliseconds               |

```v2
// Typical graceful-shutdown pattern
import "std.http"
import "std.signal"

let server = http.server(8080)
server.get("/", lambda(req, res) { res.send("ok") })

signal.on("SIGTERM", lambda() {
    log.info("Shutting down...")
    server.stop(grace_period: 10)
    exit(0)
})

server.start()
```

### Retry Policies

Configure automatic retries for transient failures on the HTTP client:

```v2
let client = http.client({
    "base_url": "https://api.example.com",
    "retry": {
        "max_attempts": 3,                         // total attempts (1 initial + 2 retries)
        "backoff": "exponential",                   // "fixed", "linear", "exponential"
        "base_delay_ms": 200,                       // initial delay
        "max_delay_ms": 5000,                       // cap on backoff
        "retry_on": [429, 500, 502, 503, 504],     // HTTP status codes to retry
        "retry_on_timeout": true                    // also retry on TimeoutError
    }
})

let resp = await client.get("/flaky-endpoint")    // retries automatically on 5xx
```

Per-request retry override:

```v2
let resp = await http.get(url, {
    "retry": {"max_attempts": 5, "backoff": "linear", "base_delay_ms": 1000}
})
```

### Rate Limiting

Built-in server-side rate limiter middleware:

```v2
import "std.http"

let limiter = http.rate_limiter({
    "window_ms": 60_000,      // 1-minute window
    "max_requests": 100,      // max 100 requests per window per key
    "key": lambda(req) { return req.headers.get("X-API-Key", req.ip) },
    "strategy": "sliding_window",    // "fixed_window", "sliding_window", "token_bucket"
    "on_limit": lambda(req, res) {
        res.status(429).json({"error": "Rate limit exceeded", "retry_after": 30})
    }
})

server.use(limiter)
```

Client-side rate limiting (throttle outbound requests):

```v2
let client = http.client({
    "base_url": "https://api.example.com",
    "rate_limit": {
        "requests_per_second": 10,    // max 10 requests/sec
        "burst": 20                    // allow burst of 20 before throttling
    }
})

// Requests exceeding the rate are queued and sent when the window allows
for (id in user_ids) {
    let resp = await client.get(f"/users/${id}")    // automatically throttled
}
```

| Function / Option              | Description                                        |
| ------------------------------ | -------------------------------------------------- |
| `http.rate_limiter(opts)`      | Create rate-limiting middleware for server         |
| `"strategy": "token_bucket"`   | Smooth rate limiting with burst allowance          |
| `"strategy": "sliding_window"` | Per-key sliding window counter                     |
| `"strategy": "fixed_window"`   | Simple fixed-interval counter                      |
| `client opts "rate_limit"`     | Client-side outbound request throttling            |
| `client opts "retry"`          | Client-side retry policy with configurable backoff |

---

## std.ffi — Foreign Function Interface

Comprehensive FFI beyond `extern c` for loading shared libraries at runtime, defining function signatures, mapping complex C types, and calling platform APIs.

```v2
import "std.ffi"
```

### Loading a Shared Library

```v2
let libc = ffi.load("libc.so.6")     // Linux
let libc = ffi.load("libc.dylib")    // macOS
let libc = ffi.load("msvcrt.dll")    // Windows

// Platform-agnostic (searches standard library paths)
let libcurl = ffi.load_lib("curl")   // finds libcurl.so / libcurl.dylib / libcurl.dll
```

### Declaring Function Signatures

```v2
let puts = ffi.func(libc, "puts", {
    "args":   ["pointer"],     // const char*
    "ret":    "int"
})

let strlen = ffi.func(libc, "strlen", {
    "args":   ["pointer"],
    "ret":    "u64"
})

let memcpy = ffi.func(libc, "memcpy", {
    "args":   ["pointer", "pointer", "u64"],
    "ret":    "pointer"
})
```

### Calling FFI Functions

```v2
let msg = ffi.cstring("Hello from V2!\n")    // allocate a null-terminated C string
puts(msg)
ffi.free(msg)

let n = strlen(ffi.cstring("hello"))
print(n)    // 5
```

### Type Mapping

| V2 FFI type | C equivalent                |
| ----------- | --------------------------- |
| `"i8"`      | `int8_t` / `char`           |
| `"i16"`     | `int16_t` / `short`         |
| `"i32"`     | `int32_t` / `int`           |
| `"i64"`     | `int64_t` / `long long`     |
| `"u8"`      | `uint8_t` / `unsigned char` |
| `"u16"`     | `uint16_t`                  |
| `"u32"`     | `uint32_t`                  |
| `"u64"`     | `uint64_t` / `size_t`       |
| `"f32"`     | `float`                     |
| `"f64"`     | `double`                    |
| `"pointer"` | `void*` / any pointer       |
| `"int"`     | platform `int`              |
| `"void"`    | void return                 |

### Struct Mapping (C Structs)

```v2
// Define a C-compatible struct layout
let stat_t = ffi.struct({
    "st_dev":   "u64",
    "st_ino":   "u64",
    "st_mode":  "u32",
    "st_nlink": "u64",
    "st_uid":   "u32",
    "st_gid":   "u32",
    "st_size":  "i64",
    // ... etc
})

let stat_fn = ffi.func(libc, "stat", {
    "args": ["pointer", "pointer"],
    "ret":  "int"
})

// Allocate and read a struct
let buf = ffi.alloc(stat_t.size())
let path = ffi.cstring("/etc/passwd")
stat_fn(path, buf)

let st = stat_t.read(buf)
print(st["st_size"])    // file size in bytes

ffi.free(buf)
ffi.free(path)
```

### Callbacks (V2 Function ? C Function Pointer)

```v2
let compare_fn = ffi.callback(
    lambda(a: pointer, b: pointer) -> int {
        let x = ffi.read_i32(a)
        let y = ffi.read_i32(b)
        return x - y
    },
    args: ["pointer", "pointer"],
    ret: "int"
)

let qsort = ffi.func(libc, "qsort", {
    "args": ["pointer", "u64", "u64", "pointer"],
    "ret": "void"
})

qsort(data_ptr, count, item_size, compare_fn)
ffi.free_callback(compare_fn)
```

### Memory Utilities

```v2
let ptr = ffi.alloc(64)               // allocate 64 bytes
let ptr2 = ffi.alloc_zeroed(128)      // zero-initialized allocation
ffi.write_i32(ptr, 0, 42)             // write int32 at offset 0
let val = ffi.read_i32(ptr, 0)        // read int32 at offset 0
let s = ffi.cstring("hello")          // allocate null-terminated string
let text = ffi.read_cstring(ptr)      // read null-terminated string from pointer
ffi.free(ptr)                         // free manually allocated memory
```

### API Reference

| Function                                  | Description                                       |
| ----------------------------------------- | ------------------------------------------------- |
| `ffi.load(path)`                          | Load a shared library by path                     |
| `ffi.load_lib(name)`                      | Load a shared library by name (platform-agnostic) |
| `ffi.func(lib, name, sig)`                | Bind a function from a library                    |
| `ffi.struct(fields)`                      | Define a C-compatible struct layout               |
| `ffi.callback(fn, args, ret)`             | Wrap a V2 lambda as a C function pointer          |
| `ffi.free_callback(ptr)`                  | Release a callback                                |
| `ffi.alloc(n)`                            | Allocate n bytes                                  |
| `ffi.alloc_zeroed(n)`                     | Allocate n bytes, zeroed                          |
| `ffi.free(ptr)`                           | Free allocation                                   |
| `ffi.cstring(s)`                          | Allocate a C string                               |
| `ffi.read_cstring(ptr)`                   | Read a null-terminated string from a pointer      |
| `ffi.read_i8/i16/i32/i64(ptr, off)`       | Read integer at offset                            |
| `ffi.write_i8/i16/i32/i64(ptr, off, val)` | Write integer at offset                           |
| `ffi.read_f32/f64(ptr, off)`              | Read float at offset                              |
| `ffi.write_f32/f64(ptr, off, val)`        | Write float at offset                             |

---

## Type Aliases

A type alias gives an existing type a new name. The alias and the original type are **fully interchangeable** — they refer to the same underlying type, not to a distinct newtype.

### Syntax

```v2
type Alias = ExistingType
```

### Basic Examples

```v2
type UserId   = int
type Username = str
type Tags     = list<str>
type Callback = func(int, str) -> bool
```

Anywhere `int` is valid, `UserId` is valid, and vice versa:

```v2
func find_user(id: UserId) -> str { ... }

let uid: UserId = 42
find_user(uid)       // works
find_user(42)        // also works — int and UserId are the same type
```

### Aliases for Generic Types

```v2
type StringMap<V> = dict<str, V>
type Matrix<T>    = list<list<T>>

let m: StringMap<int> = {"a": 1, "b": 2}
let grid: Matrix<float> = [[1.0, 2.0], [3.0, 4.0]]
```

### Aliases for Function Types

```v2
type Predicate<T>  = func(T) -> bool
type Transformer<A, B> = func(A) -> B

func filter_by<T>(items: list<T>, pred: Predicate<T>) -> list<T> {
    return items.filter(pred)
}
```

### Aliases for Complex Nested Types

```v2
type Result<T> = Ok(T) | Err(str)
type EventMap  = dict<str, list<func()>>
```

### Notes

- Type aliases are resolved at compile time — they have zero runtime overhead.
- Aliases do **not** create distinct types. If you need a fully distinct type (e.g. to prevent `UserId` from being accidentally used as a raw `int`), use a newtype pattern via a single-field `struct` instead.
- Recursive aliases (a type that directly references itself) are a compile-time error. Use `enum` or `struct` for recursive structures.

---

## Destructuring Assignment

V2 supports destructuring in `let` bindings, function parameters, `for` loops, and `match` arms. Destructuring lets you unpack the contents of lists, tuples, dicts, and structs directly into named variables.

### List / Tuple Destructuring

```v2
let [a, b, c] = [1, 2, 3]
// a = 1, b = 2, c = 3

let [x, y] = get_coords()    // function returning a two-element list

// Ignore elements with _
let [first, _, third] = [10, 20, 30]
// first = 10, third = 30 (middle element discarded)

// Nested
let [[r, g], b] = [[255, 128], 0]
// r = 255, g = 128, b = 0
```

### Rest Patterns

`...name` collects remaining elements into a list:

```v2
let [head, ...tail] = [1, 2, 3, 4, 5]
// head = 1, tail = [2, 3, 4, 5]

let [first, second, ...rest] = items
// first and second are singles, rest collects everything after

let [...init, last] = [10, 20, 30, 40]
// init = [10, 20, 30], last = 40
```

### Dict / Struct Destructuring

Destructure a dict by key names:

```v2
let {"name": n, "age": a} = {"name": "Alice", "age": 30}
// n = "Alice", a = 30

// Shorthand — use the key name directly as the variable
let {name, age} = user_dict
// equivalent to let {"name": name, "age": age} = user_dict

// Ignore extra keys
let {x, y} = {"x": 10, "y": 20, "z": 30}    // z is discarded
```

Struct fields can be destructured the same way:

```v2
struct Point { x: float, y: float }

let p = Point { x: 3.0, y: 4.0 }
let {x, y} = p
print(x, y)    // 3.0  4.0
```

### Renaming in Dict Destructuring

Use `"key": varname` to bind a key to a differently-named variable:

```v2
let {"first_name": first, "last_name": last} = record
```

### Default Values in Dict Destructuring

Provide a fallback for missing keys with `??`:

```v2
let {name, age ?? 0, active ?? true} = config
```

### Destructuring in Function Parameters

```v2
func print_point([x, y]) {
    print(f"(${x}, ${y})")
}

func greet({name, age}) {
    print(f"Hello ${name}, you are ${age}")
}

print_point([3, 4])
greet({"name": "Alice", "age": 30})
```

### Destructuring in `for` Loops

```v2
// Tuple/list
for ([key, val] in items(my_dict)) {
    print(f"${key} = ${val}")
}

// Dict
for ({name, score} in leaderboard) {
    print(f"${name}: ${score}")
}

// With index via enumerate
for ([i, [x, y]] in enumerate(points)) {
    print(f"Point ${i}: (${x}, ${y})")
}
```

### Destructuring in `match` Arms

```v2
match (event) {
    case ({type: "click", x, y}) { print(f"Click at (${x}, ${y})") }
    case ({type: "key", key}) { print(f"Key pressed: ${key}") }
    case ([first, ...rest]) { print(f"List starting with ${first}") }
    default { print("unknown event") }
}
```

### Notes

- Destructuring is always a compile-time static transform — no reflection or runtime type checks are involved.
- Assigning to a destructure binding where the source has fewer elements than the pattern is a **runtime error** (out-of-bounds).
- For dicts and structs, missing keys without a `??` default cause a **runtime key error**.

---

## if let / while let

`if let` and `while let` are ergonomic patterns for safely extracting values from `Option` (`Some`/`None`) and `Result` (`Ok`/`Err`) without boilerplate `if is_some(...)` / `unwrap(...)` chains.

### `if let`

`if let` matches a pattern and, if it succeeds, binds the inner value to a name in the `if` body:

```v2
let maybe_user = find_user(id)    // returns Some(user) or None

if let Some(user) = maybe_user {
    print(f"Found: ${user.name}")
} else {
    print("Not found")
}
```

The `else` branch runs if the pattern does not match. The bound variable (`user`) is only in scope inside the `if` block.

#### With `Result`

```v2
let cfg_result = read_file("config.toml")

if let Ok(data) = cfg_result {
    process(data)
} else {
    print("Could not read config.toml")
}

// More idiomatic for full Ok/Err handling (including the error value):
let cfg_result2 = read_file("config.toml")
match (cfg_result2) {
    case (Ok(data)) { process(data) }
    case (Err(e)) { print(f"Error: ${e}") }
}
```

#### Chaining with `&&`

Multiple `if let` conditions can be chained with `&&`. All must match for the body to run:

```v2
if let Some(user) = get_user(id) && let Some(perms) = get_permissions(user.id) {
    print(f"${user.name} has ${perms.len()} permissions")
}
```

#### Compared to `match`

`if let` is syntactic sugar for a `match` with one meaningful arm and a `default`:

```v2
// These are equivalent:
if let Some(x) = val { use(x) }

match (val) {
    case (Some(x)) { use(x) }
    default { () }
}
```

Prefer `match` when you need to handle multiple variants. Prefer `if let` for a single extraction with an optional `else`.

### `while let`

`while let` loops as long as a pattern keeps matching. When the pattern fails, the loop exits:

```v2
// Drain a list until empty
let stack = [1, 2, 3, 4, 5]

while let Some(top) = stack.pop_opt() {
    print(top)
}
// prints 5 4 3 2 1

// Process items from a channel until it closes
while let Some(msg) = chan_try_recv(ch) {
    handle(msg)
}
```

`.pop_opt()` returns `Some(val)` on success and `None` when the list is empty — this makes it the idiomatic method to use with `while let`.

```v2
// Collect results until the first error
let results = []
while let Ok(line) = reader.next_line() {
    results.push(line)
}
```

### `let else`

`let else` is the asserting counterpart: bind the value if the pattern matches, otherwise execute the `else` block (which must diverge — `return`, `break`, `continue`, or `throw`):

```v2
func process(raw) {
    let Ok(data) = parse(raw) else {
        print("parse failed")
        return
    }
    // data is available here — parse succeeded
    use(data)
}
```

This avoids deep nesting by handling the failure early and continuing with the success path.

---

## Labeled Loops

Labels allow `break` and `continue` to target a specific outer loop, rather than always affecting the innermost one. This is useful for nested loops where you need to exit or skip more than one level.

### Syntax

Attach a label to any `for` or `while` loop with `label_name:` before the loop keyword:

```v2
outer: for (i in 0..5) {
    inner: for (j in 0..5) {
        if (j == 3) { break outer }     // exit BOTH loops
        print(i, j)
    }
}
```

### `break` with a Label

`break label` exits the labeled loop immediately. Execution continues after the labeled loop's closing brace.

```v2
search: for (row in grid) {
    for (cell in row) {
        if (cell == target) {
            print("Found!")
            break search    // stop searching entirely
        }
    }
}
// continues here after break search
```

### `continue` with a Label

`continue label` skips the current iteration of the labeled loop (not the innermost one), moving on to the next iteration of the labeled loop:

```v2
outer: for (i in 0..5) {
    for (j in 0..5) {
        if (j == 2) { continue outer }    // skip rest of inner loop, go to next i
        print(i, j)
    }
}
```

### Labeled `while` Loops

Labels work on `while` loops too:

```v2
reading: while (true) {
    let line = input()
    for (ch in line) {
        if (ch == "q") { break reading }
    }
}
```

### Notes

- Label names follow the same identifier rules as variable names.
- Labels are local to the function — they cannot be referenced from a nested function or lambda.
- A label must immediately precede a `for` or `while` statement; attaching a label to anything else is a compile-time error.
- Unlabeled `break` and `continue` still target the innermost enclosing loop, as always.

---

## Union Types

A union type (also called a sum type or discriminated union) is a type that can hold a value of any one of several listed types. Union types are written with `|` between the alternatives.

### Syntax

```v2
type MaybeInt = int | null
type Id       = int | str
type JsonVal  = int | float | str | bool | null | list | dict
```

### Declaring Variables with Union Types

```v2
let x: int | str = 42
let y: int | str = "hello"

func process(val: int | str | null) {
    // ...
}
```

### Type Narrowing

Use `is` to narrow a union type within a conditional or `match`:

```v2
func show(val: int | str | null) {
    if (val is int) {
        print(f"Integer: ${val * 2}")    // val is treated as int here
    } else if (val is str) {
        print(f"String: ${val.upper()}")
    } else {
        print("null")
    }
}
```

Inside an `is` branch the compiler knows the concrete type and allows the corresponding methods and operations.

### Union Types in `match`

`match` is the idiomatic way to handle all variants exhaustively:

```v2
func describe(val: int | float | str | bool | null) -> str {
    match (val) {
        case (v: int) { return f"integer ${v}" }
        case (v: float) { return f"float ${v:.2f}" }
        case (v: str) { return f"string '${v}'" }
        case (v: bool) { return v ? "true" : "false" }
        case (null) { return "nothing" }
    }
}
```

The compiler enforces exhaustiveness on union type `match` — unhandled variants are a warning (use `--warn exhaustive` to make it an error).

### Union Types and `null`

`T | null` is a common pattern equivalent to `Option<T>`. It is idiomatic for values that might be absent:

```v2
type OptStr = str | null

func find_name(id: int) -> str | null {
    if (id == 1) { return "Alice" }
    return null
}

let name = find_name(99) ?? "unknown"
```

### Narrowing with `as`

After checking with `is`, use `as` for an explicit cast (no runtime check — only valid inside a narrowed branch):

```v2
let val: int | str = get_value()
if (val is str) {
    let s = val as str
    print(s.upper())
}
```

### Union Types as Function Parameters and Return Types

```v2
func parse_number(s: str) -> int | float | null {
    if (s.contains(".")) {
        return float(s)
    } else if (s.isdigit()) {
        return int(s)
    }
    return null
}
```

### Notes

- Union types are erased at runtime. The runtime representation is a tagged value (type tag + payload).
- You cannot call methods on a union type directly — you must narrow it first.
- Unions of the same type reduce to that type: `int | int` is `int`.
- `null` in a union does not automatically make other members optional — only the specific null variant is nullable.

---

## The `never` Type

`never` is the **bottom type** — it represents a computation that never produces a value because it always diverges: it throws an exception, loops forever, calls `exit()`, etc.

### When to Use `never`

A function that never returns should declare `-> never` as its return type:

```v2
func panic(msg: str) -> never {
    throw new RuntimeError(msg)
}

func infinite_loop() -> never {
    while (true) {
        // ...
    }
}

func abort_program(code: int) -> never {
    exit(code)
}
```

Declaring `-> never` tells the compiler that code after a call to this function is unreachable, enabling better flow analysis and dead-code warnings.

### `never` in Control Flow

`never` integrates with exhaustiveness checking and type inference. In a `match`, a branch that calls a `-> never` function is considered handled:

```v2
func require<T>(val: T | null, msg: str) -> T {
    match (val) {
        case (Some(v)) { return v }
        case (null) {
            panic(msg)    // panic returns never — branch is handled
        }
    }
}
```

### `never` as a Union Member

`never` in a union type drops out, since it can never contribute a value:

```v2
type Foo = int | never    // equivalent to just int
```

This is useful in generic code where conditional compilation may produce `never` for some type parameters.

### `never` vs `null` vs `void`

| Type    | Meaning                                        |
| ------- | ---------------------------------------------- |
| `void`  | Function returns, but produces no useful value |
| `null`  | The absence of a value — explicitly `null`     |
| `never` | Function never returns at all                  |

```v2
func greet() -> void  { print("hi") }          // returns, no value
func nothing() -> null { return null }          // returns null
func crash() -> never  { throw "bye" }          // never returns
```

---

## Effects System

V2 allows functions to be annotated with **effects** — a set of capabilities they use. Effect annotations are optional and informational by default, but the compiler can verify them when `--warn effects` is enabled. The `comptime` function `ct_get_effects` can inspect them at compile time.

### Syntax

Effects are declared in square brackets after the parameter list (and before the return type arrow):

```v2
func fetch_data(url: str) [effects: net] -> str {
    return http_get(url).body
}

func write_log(msg: str) [effects: io] {
    append_file("app.log", msg + "\n")
}

func add(a: int, b: int) [effects: none] -> int {
    return a + b
}

func mixed(url: str, path: str) [effects: net, io] -> str {
    let data = http_get(url).body
    write_file(path, data)
    return data
}
```

### Built-in Effect Labels

| Effect   | Meaning                                        |
| -------- | ---------------------------------------------- |
| `none`   | Pure — no side effects, no I/O, deterministic  |
| `io`     | Reads or writes files, stdin/stdout            |
| `net`    | Makes network requests                         |
| `env`    | Reads environment variables or system info     |
| `rand`   | Uses a random number generator                 |
| `time`   | Reads the system clock                         |
| `state`  | Mutates shared/global state                    |
| `unsafe` | Uses `unsafe` blocks or raw pointer operations |

Multiple effects are listed comma-separated: `[effects: net, io, state]`.

### `pure` Shorthand

A function annotated `[effects: none]` is **pure**. V2 provides the `pure` shorthand keyword:

```v2
pure func square(x: int) -> int {
    return x * x
}

// equivalent to:
func square(x: int) [effects: none] -> int {
    return x * x
}
```

Pure functions can be memoized automatically and are safe for compile-time evaluation.

### Effect Checking

Effect annotations are verified with `--warn effects`:

```bash
v2 --warn effects myapp.v2
```

The compiler emits a warning when:

- A function annotated `[effects: none]` calls a function with a declared side effect.
- A function annotated `[effects: io]` calls a function with `[effects: net]` without also declaring `net`.

Effect inference is **opt-in** — unannotated functions are unchecked.

### Compile-Time Effect Introspection

```v2
comptime {
    let effects = ct_get_effects("fetch_data")    // ["net"]
    if (!effects.contains("net")) {
        ct_warn("fetch_data missing net effect")
    }
}
```

### Effects in `v2.toml`

Enable effect checking project-wide:

```toml
[compiler]
warn = ["effects"]
```

### Custom Effect Labels

You can define your own effect labels — the compiler treats them as opaque tokens that appear in the `ct_get_effects` output:

```v2
func draw_frame() [effects: gfx] {
    // ...
}
```

Custom labels are not validated against built-in semantics — they are purely informational unless you write your own `comptime` checks that inspect them.

---

## The `Default` Trait

The `Default` trait defines a **zero-value constructor** for a type — a canonical "empty" or "identity" instance produced without arguments. Types implementing `Default` can be used with `Type.default()` and `unwrap_or_default()`.

### Definition

```v2
trait Default {
    func default() -> Self    // static — no self parameter
}
```

### Built-in `Default` Implementations

V2's standard types have built-in defaults:

| Type    | `default()` value |
| ------- | ----------------- |
| `int`   | `0`               |
| `float` | `0.0`             |
| `str`   | `""`              |
| `bool`  | `false`           |
| `list`  | `[]`              |
| `dict`  | `{}`              |
| `set`   | `#{}`             |

### Implementing `Default` for Custom Types

```v2
struct Config {
    host:    str,
    port:    int,
    timeout: int,
    tls:     bool
}

impl Default for Config {
    func default() -> Config {
        return Config {
            host:    "localhost",
            port:    8080,
            timeout: 30,
            tls:     false
        }
    }
}

let cfg = Config.default()
print(cfg.host)     // localhost
print(cfg.port)     // 8080
```

### Using `unwrap_or_default()`

`unwrap_or_default()` unwraps an `Option` or `Result`, and if it is `None` or `Err`, returns `Type.default()` instead:

```v2
let x: int | null = None
let val = unwrap_or_default(x)    // 0 — int default

let cfg = get_config() ?? Config.default()    // ?? also works for simple cases

// The builtin unwrap_or_default infers the type from context:
func load_settings() -> Config {
    let result = parse_config(read_file("settings.toml"))
    return unwrap_or_default(result)    // Config.default() if parse fails
}
```

### `Default` and `Cloneable` Together

A common pattern is combining `Default` and `Cloneable` to cheaply stamp out fresh copies of a template value:

```v2
let template = Config.default()

func fresh_config() -> Config {
    return clone(template)    // always a fresh independent copy
}
```

### Deriving `Default` Automatically

If all fields of a struct have `Default` implementations, V2 can derive `Default` automatically using the `@derive` decorator:

```v2
@derive(Default)
struct Point {
    x: float,    // defaults to 0.0
    y: float     // defaults to 0.0
}

let origin = Point.default()    // Point { x: 0.0, y: 0.0 }
```

`@derive(Default)` generates an `impl Default for Point` that calls `.default()` on every field. If any field does not implement `Default`, the derive is a compile-time error.

### Notes

- `Default` is a **static** method — call it as `Type.default()`, not `val.default()`.
- `Default` should return a **valid, usable** value — not a sentinel or invalid placeholder.
- For types where no sensible default exists, prefer not implementing `Default` and force callers to provide explicit values.

---

## std.audio — Audio Playback & Recording

```v2
import std.audio
```

### Playback

```v2
let player = audio.open("song.mp3")    // supports mp3, wav, ogg, flac, aac
player.play()
player.pause()
player.stop()
player.seek(30.0)                      // seek to 30 seconds
player.volume = 0.8                    // 0.0 — 1.0
player.speed  = 1.5                    // playback rate multiplier
let dur = player.duration()            // total duration in seconds
let pos = player.position()            // current position in seconds
player.on_end(func() { print("done") })
```

### Recording

```v2
let rec = audio.recorder(
    sample_rate: 44100,
    channels:    2,
    format:      "f32"      // "i16" | "i32" | "f32"
)

rec.start()
sleep(5000)    // record for 5 seconds
rec.stop()
rec.save("output.wav")

// Stream recorded chunks
let rec2 = audio.recorder(sample_rate: 16000, channels: 1, format: "i16")
rec2.on_chunk(func(samples: bytes) {
    process(samples)
})
rec2.start()
```

### DSP & Filters

```v2
let buf = audio.load_buffer("input.wav")    // AudioBuffer

// Built-in DSP
let loud  = buf.gain(2.0)
let quiet = buf.gain(0.5)
let cut   = buf.low_pass(cutoff: 800.0, q: 0.7)
let boost = buf.high_pass(cutoff: 200.0, q: 0.7)
let band  = buf.band_pass(center: 1000.0, q: 1.0)
let rev   = buf.reverb(room: 0.8, damp: 0.5, wet: 0.3)
let norm  = buf.normalize()
let mono  = buf.to_mono()
let trimmed = buf.trim_silence(threshold: 0.01)

// Concatenate & mix
let joined = audio.concat([buf1, buf2, buf3])
let mixed  = audio.mix([buf1, buf2], weights: [0.7, 0.3])

buf.save("output.mp3", bitrate: 192)
buf.save("output.wav")
```

### Streaming

```v2
let stream = audio.open_stream(url: "https://example.com/stream.mp3")
stream.play()
stream.on_buffer(func(seconds_buffered: float) {
    print("buffered:", seconds_buffered)
})
```

---

## std.video — Video Processing

```v2
import std.video
```

### Loading & Info

```v2
let v = video.open("movie.mp4")    // supports mp4, mkv, webm, avi, mov
print(v.width, v.height)
print(v.fps)
print(v.duration)       // seconds
print(v.codec)
print(v.audio_tracks)   // list of audio track info
```

### Frame Access

```v2
let frame = v.frame_at(10.0)     // get frame at 10 seconds ? Image
let frames = v.frames(0.0, 5.0, fps: 1)  // extract 1 frame/sec from 0—5s ? list<Image>

// Iterate all frames
for (frame in v.iter_frames()) {
    process(frame)
}

// Seek and read
v.seek(30.0)
let frame = v.next_frame()
```

### Editing & Encoding

```v2
let out = video.writer("output.mp4", {
    "width":  1920,
    "height": 1080,
    "fps":    30,
    "codec":  "h264",       // "h264", "h265", "vp9", "av1"
    "bitrate": 5_000_000    // bits per second
})

for (frame in v.iter_frames()) {
    let processed = image.resize(frame, width: 1920)
    out.write_frame(processed)
}
out.close()
```

### Trimming & Concatenation

```v2
// Trim a video to a time range
video.trim("input.mp4", "clip.mp4", start: 10.0, end: 30.0)

// Concatenate multiple videos
video.concat(["intro.mp4", "main.mp4", "outro.mp4"], "final.mp4")
```

### Audio Extraction

```v2
video.extract_audio("movie.mp4", "audio.mp3")
video.replace_audio("movie.mp4", "narration.mp3", "output.mp4")
video.mute("input.mp4", "silent.mp4")
```

### Thumbnails & GIF

```v2
video.thumbnail("movie.mp4", "thumb.jpg", at: 5.0)
video.to_gif("input.mp4", "output.gif", start: 0.0, end: 3.0, fps: 10, width: 320)
```

### API Reference

| Function                                    | Description                        |
| ------------------------------------------- | ---------------------------------- |
| `video.open(path)`                          | Open a video file                  |
| `v.frame_at(seconds)`                       | Extract a single frame as an Image |
| `v.frames(start, end, fps?)`                | Extract frames over a time range   |
| `v.iter_frames()`                           | Iterator over all frames           |
| `v.seek(seconds)`                           | Seek to a position                 |
| `v.next_frame()`                            | Read the next frame after a seek   |
| `video.writer(path, opts)`                  | Create a video writer              |
| `out.write_frame(image)`                    | Write one frame to the output      |
| `out.close()`                               | Finalize and close the output file |
| `video.trim(src, dst, start, end)`          | Trim video to a time range         |
| `video.concat(inputs, dst)`                 | Concatenate videos                 |
| `video.extract_audio(src, dst)`             | Extract audio track to a file      |
| `video.replace_audio(src, audio, dst)`      | Replace audio in a video           |
| `video.mute(src, dst)`                      | Remove audio from a video          |
| `video.thumbnail(src, dst, at)`             | Generate a thumbnail image         |
| `video.to_gif(src, dst, start, end, opts?)` | Convert a clip to animated GIF     |

---

## std.pdf — PDF Generation & Reading

```v2
import std.pdf
```

### Reading PDFs

```v2
let doc = pdf.open("report.pdf")
print(doc.page_count)
print(doc.metadata)          // {title, author, subject, creator, ...}

let text = doc.extract_text()             // full text, all pages
let page_text = doc.page(0).text()        // text from one page
let imgs = doc.page(0).images()           // list of embedded images

// Search
let matches = doc.search("revenue")       // [{page, x, y, text}, ...]
```

### Creating PDFs

```v2
let doc = pdf.new()

let page = doc.add_page(width: 595, height: 842)   // A4 in points

// Text
page.text("Hello, World!", x: 50, y: 780, font: "Helvetica", size: 24, color: "#000")
page.text("Body paragraph.", x: 50, y: 740, size: 12, line_height: 1.5, max_width: 400)

// Shapes
page.rect(x: 50, y: 700, w: 200, h: 50, fill: "#e0e0e0", stroke: "#000", stroke_width: 1)
page.circle(x: 300, y: 725, r: 25, fill: "#4488ff")
page.line(x1: 50, y1: 660, x2: 545, y2: 660, color: "#ccc", width: 0.5)

// Images
page.image("logo.png", x: 50, y: 600, width: 100, height: 50)

// Links
page.link(url: "https://example.com", x: 50, y: 580, width: 150, height: 20)
```

### Tables

```v2
let tbl = pdf.table(
    columns: ["Name", "Score", "Grade"],
    rows: [
        ["Alice", "95", "A"],
        ["Bob",   "82", "B"],
    ],
    x: 50, y: 500, width: 400,
    header_fill: "#3355aa", header_color: "#fff",
    row_fill: ["#fff", "#f5f5f5"],    // alternating
    border: true
)
page.add_table(tbl)
```

### Merge & Split

```v2
let merged = pdf.merge(["a.pdf", "b.pdf", "c.pdf"])
merged.save("combined.pdf")

let pages = pdf.split("big.pdf", ranges: [[0, 4], [5, 9]])
// pages[0] ? first 5 pages, pages[1] ? next 5 pages
pages[0].save("part1.pdf")
```

### Saving

```v2
doc.save("output.pdf")
doc.save_bytes()    // returns bytes — useful for HTTP responses
```

---

## std.excel — Excel / XLSX Files

```v2
import std.excel
```

### Reading

```v2
let wb = excel.open("data.xlsx")
let ws = wb.sheet("Sheet1")        // by name
let ws = wb.sheet(0)               // by index

let val = ws.cell("A1").value      // raw value (int | float | str | bool | null)
let text = ws.cell("A1").text      // string representation
let row  = ws.row(1)               // list of Cell
let col  = ws.column("A")         // list of Cell
let all  = ws.rows()               // list<list<Cell>>
let used = ws.used_range()         // (row_start, col_start, row_end, col_end)
```

### Writing

```v2
let wb = excel.new()
let ws = wb.add_sheet("Results")

ws.set("A1", "Name")
ws.set("B1", "Score")
ws.set("A2", "Alice")
ws.set("B2", 95)

// Ranges
ws.set_row(3, ["Bob", 82, "B"])
ws.set_column("C", ["Grade", "A", "B"])
```

### Formatting

```v2
let fmt = excel.format(
    bold: true, italic: false,
    font_size: 12, font_color: "#000",
    fill: "#ddeeff",
    border: "thin",
    number_format: "#,##0.00",
    align: "center"
)
ws.set_format("A1:C1", fmt)
ws.set_col_width("A", 20)
ws.set_row_height(1, 25)
ws.freeze_panes(row: 1, col: 0)    // freeze header row
```

### Charts

```v2
let chart = excel.chart(type: "bar")    // "bar" | "line" | "pie" | "scatter"
chart.add_series(
    name: "Scores",
    categories: "Sheet1!A2:A10",
    values: "Sheet1!B2:B10"
)
chart.title = "Student Scores"
ws.add_chart(chart, at: "E2")
```

### Saving

```v2
wb.save("output.xlsx")
```

---

## std.jwt — JSON Web Tokens

```v2
import std.jwt
```

### Signing

```v2
// HS256 (shared secret)
let token = jwt.sign(
    payload: {"sub": "user123", "role": "admin", "exp": time.now() + 3600},
    secret:  "mysecret",
    alg:     "HS256"     // default
)

// RS256 (RSA private key PEM)
let token = jwt.sign(
    payload: {"sub": "user123"},
    secret:  read_file("private.pem"),
    alg:     "RS256"
)

// ES256 (EC private key PEM)
let token = jwt.sign(
    payload: {"sub": "user123"},
    secret:  read_file("ec_private.pem"),
    alg:     "ES256"
)
```

### Verifying

```v2
let result = jwt.verify(token, secret: "mysecret", alg: "HS256")

match result {
    case (Ok(claims)) {
        print(claims["sub"])    // "user123"
    }
    case (Err(e)) {
        print("Invalid token:", e)    // "expired" | "invalid signature" | ...
    }
}

// RS256 verify with public key
let result = jwt.verify(token, secret: read_file("public.pem"), alg: "RS256")
```

### Decoding Without Verification

```v2
let claims = jwt.decode_unverified(token)    // no signature check — use with caution
print(claims["sub"])
```

### Common Claims

| Claim | Meaning                 |
| ----- | ----------------------- |
| `sub` | Subject (user ID)       |
| `exp` | Expiry timestamp (unix) |
| `iat` | Issued-at timestamp     |
| `iss` | Issuer                  |
| `aud` | Audience                |

---

## std.oauth2 — OAuth 2.0

```v2
import std.oauth2
```

### Authorization Code Flow

```v2
let client = oauth2.client(
    client_id:     "my_app",
    client_secret: "secret",
    auth_url:      "https://provider.example.com/oauth/authorize",
    token_url:     "https://provider.example.com/oauth/token",
    redirect_url:  "https://myapp.com/callback",
    scopes:        ["openid", "email", "profile"]
)

// Step 1 — redirect user to this URL
let auth_url = client.auth_url(state: "random_csrf_token")

// Step 2 — after redirect, exchange code for token
let token = client.exchange_code(code: "auth_code_from_query", state: "random_csrf_token")!
print(token.access_token)
print(token.refresh_token)
print(token.expires_in)
```

### Client Credentials Flow

```v2
let token = client.client_credentials()!
print(token.access_token)
```

### Refreshing Tokens

```v2
let new_token = client.refresh(token.refresh_token)!
```

### Making Authenticated Requests

```v2
let res = client.get("https://api.example.com/me", token: token)
let data = res.json()
```

---

## std.i18n — Internationalization

```v2
import std.i18n
```

### Loading Translations

```v2
// translations/en.toml
// greeting = "Hello, {name}!"
// items = "{count} item | {count} items"

i18n.load_dir("translations")    // loads all .toml / .json files by locale name
i18n.set_locale("fr")
```

### Translating

```v2
let msg = i18n.t("greeting", name: "Alice")    // "Bonjour, Alice!"
let msg = i18n.t("items", count: 1)            // "1 item"
let msg = i18n.t("items", count: 5)            // "5 items"
```

### Locale-Aware Formatting

```v2
// Numbers
i18n.format_number(1234567.89, locale: "de")   // "1.234.567,89"
i18n.format_number(1234567.89, locale: "en")   // "1,234,567.89"

// Currency
i18n.format_currency(49.99, currency: "EUR", locale: "fr")   // "49,99 —"
i18n.format_currency(49.99, currency: "USD", locale: "en")   // "$49.99"

// Dates
i18n.format_date(time.now(), style: "long", locale: "ja")     // "2025年6月1日"
i18n.format_date(time.now(), style: "short", locale: "en")    // "6/1/25"

// Relative time
i18n.relative_time(-300, locale: "en")    // "5 minutes ago"
i18n.relative_time(3600, locale: "fr")    // "dans 1 heure"
```

### Pluralization

Translation files use `|`-separated plural forms:

```toml
# translations/en.toml
apples = "one apple | {count} apples"

# translations/ar.toml  (Arabic has 6 plural forms)
apples = ——————————— واحدة |——————————————— | {count} تفاحات | {count}———————————"
```

```v2
i18n.t("apples", count: 1)    // "one apple"
i18n.t("apples", count: 7)    // "7 apples"
```

---

## std.watch — Filesystem Watching

```v2
import std.watch
```

`std.watch` is the full-featured watcher API. Use `fs.watch` from `std.fs` for quick one-path callbacks, and `std.watch` for advanced watcher lifecycle and multi-path control.

### Watching Files & Directories

```v2
let watcher = watch.new()

watcher.on_change("src/", func(event: watch.Event) {
    print(event.kind)   // "create" | "modify" | "delete" | "rename"
    print(event.path)
    print(event.old_path)   // set on rename events
})

watcher.add("config.toml")
watcher.add("src/", recursive: true)
watcher.remove("src/temp/")

watcher.start()
// ... non-blocking, runs callbacks on changes

watcher.stop()
```

### One-Shot Wait

```v2
// Block until a specific file changes
let event = watch.wait_for("build.lock", timeout: 10000)    // ms; null on timeout
```

### Debouncing

```v2
watcher.on_change("src/", debounce: 200, func(event: watch.Event) {
    // Only fires after 200ms of no additional changes — prevents rapid-fire rebuilds
    rebuild()
})
```

---

## std.grpc — gRPC Client & Server

```v2
import std.grpc
```

### Defining a Service

gRPC services are defined using V2's trait-like `service` blocks with Protobuf-compatible types:

```v2
service Greeter {
    func say_hello(req: HelloRequest) -> HelloReply
    func say_many(req: HelloRequest) -> stream HelloReply        // server streaming
    func collect(req: stream HelloRequest) -> HelloReply         // client streaming
    func chat(req: stream HelloRequest) -> stream HelloReply     // bidirectional
}

struct HelloRequest { name: str }
struct HelloReply   { message: str }
```

### Server

```v2
let server = grpc.server(port: 50051)

server.register(Greeter, impl: {
    say_hello: func(req: HelloRequest) -> HelloReply {
        return HelloReply { message: "Hello, " + req.name }
    },
    say_many: func(req: HelloRequest, stream: grpc.ServerStream<HelloReply>) {
        for i in 0..5 {
            stream.send(HelloReply { message: "Hello " + i.to_str() })
        }
        stream.close()
    }
})

server.start()
server.await()
```

### Client — Unary

```v2
let ch  = grpc.channel("localhost:50051")
let stub = grpc.stub(Greeter, channel: ch)

let reply = stub.say_hello(HelloRequest { name: "Alice" })!
print(reply.message)
```

### Client — Server Streaming

```v2
let stream = stub.say_many(HelloRequest { name: "Alice" })
for msg in stream {
    print(msg.message)
}
```

### Client — Bidirectional Streaming

```v2
let stream = stub.chat()
stream.send(HelloRequest { name: "Alice" })
stream.send(HelloRequest { name: "Bob" })
stream.close_send()

for reply in stream {
    print(reply.message)
}
```

---

## std.mqtt — MQTT Messaging

```v2
import std.mqtt
```

### Connecting

```v2
let client = mqtt.connect(
    host:     "broker.example.com",
    port:     1883,
    client_id: "myapp-001",
    username: "user",
    password: "pass",
    tls:      false,
    keep_alive: 60
)
```

### Publishing

```v2
client.publish("sensors/temp", payload: "23.5", qos: 1, retain: false)
client.publish("alerts/critical", payload: "fire!", qos: 2)
```

### Subscribing

```v2
client.subscribe("sensors/#", qos: 1, func(msg: mqtt.Message) {
    print(msg.topic)
    print(msg.payload)    // str
})

client.subscribe("alerts/+", qos: 2, func(msg: mqtt.Message) {
    handle_alert(msg)
})

// Unsubscribe
client.unsubscribe("sensors/#")
```

### QoS Levels

| Level | Meaning                         |
| ----- | ------------------------------- |
| `0`   | At most once (fire and forget)  |
| `1`   | At least once (acknowledged)    |
| `2`   | Exactly once (fully guaranteed) |

### Wildcards

| Wildcard | Matches                                                                 |
| -------- | ----------------------------------------------------------------------- |
| `+`      | Single level — `sensors/+` matches `sensors/temp` but not `sensors/a/b` |
| `#`      | Multi level — `sensors/#` matches everything under `sensors/`           |

### Disconnecting

```v2
client.disconnect()
```

---

## std.embed — Compile-Time File Embedding

```v2
import std.embed
```

Embed files, text, and directories directly into the compiled binary at build time. Embedded data is read-only and accessible without filesystem access at runtime.

### Embedding a File as Bytes

```v2
let data: bytes = embed.file("assets/logo.png")
```

### Embedding a File as Text

```v2
let html: str = embed.text("templates/index.html")
```

### Embedding an Entire Directory

```v2
let dir: embed.Dir = embed.dir("assets/")

let content: bytes = dir.read("logo.png")
let text:    str   = dir.read_text("style.css")
let names:   list<str> = dir.files()    // all embedded file paths
```

### The `bytes` Type

`bytes` is V2's built-in immutable byte sequence, distinct from `str`:

```v2
let b: bytes = embed.file("data.bin")
print(b.len())
let s: str = b.to_str(encoding: "utf8")
let hex: str = b.to_hex()
let b64: str = b.to_base64()
let sub: bytes = b.slice(0, 16)
```

### Notes

- Paths in `embed.file()`, `embed.text()`, and `embed.dir()` are **relative to the source file** that calls them.
- Embedding happens at compile time — the files must exist when `v2 build` is run.
- Use `--no-embed` flag to disable embedding and fall back to runtime filesystem reads (useful for hot-reload during development).

---

## std.template — Text Templating

```v2
import std.template
```

V2's template engine uses Jinja2-compatible syntax.

### Basic Rendering

```v2
let result = template.render("Hello, {{ name }}!", {name: "Alice"})
// "Hello, Alice!"
```

### Loading from String

```v2
let tpl = template.from_str("{% for item in items %}{{ item }}\n{% endfor %}")
let out = tpl.render({items: ["a", "b", "c"]})
```

### Loading from File

```v2
let tpl = template.from_file("views/email.html")
let html = tpl.render({user: "Alice", code: "XYZ123"})
```

### Environment (Multiple Templates)

```v2
let env = template.env(dir: "views/")    // loads all .html / .txt files

let tpl  = env.get("email.html")
let out  = tpl.render({name: "Alice"})

// Auto-reload in dev mode
let env = template.env(dir: "views/", auto_reload: true)
```

### Template Syntax

```jinja
{# Comment #}

{# Variables #}
{{ user.name }}
{{ scores[0] }}

{# Conditionals #}
{% if user.is_admin %}
  <b>Admin</b>
{% elif user.role == "mod" %}
  Moderator
{% else %}
  Guest
{% endif %}

{# Loops #}
{% for item in items %}
  {{ loop.index }} — {{ item.name }}
{% endfor %}

{# Filters #}
{{ name | upper }}
{{ price | round(2) }}
{{ bio   | truncate(100) }}
{{ html  | escape }}
{{ list  | join(", ") }}

{# Template Inheritance #}
{% extends "base.html" %}
{% block content %}
  <h1>Hello</h1>
{% endblock %}
```

### Built-in Filters

| Filter            | Description                     |
| ----------------- | ------------------------------- |
| `upper` / `lower` | Change case                     |
| `trim`            | Strip whitespace                |
| `truncate(n)`     | Limit to n characters           |
| `round(n)`        | Round to n decimal places       |
| `escape` / `e`    | HTML-escape                     |
| `safe`            | Mark as HTML-safe (no escaping) |
| `join(sep)`       | Join list with separator        |
| `default(val)`    | Fallback if null                |
| `length`          | List / string length            |
| `first` / `last`  | First or last element           |
| `sort`            | Sort a list                     |
| `reverse`         | Reverse a list                  |
| `keys` / `values` | Dict keys/values                |

---

## std.multipart — Multipart Form Data

```v2
import std.multipart
```

### Parsing Incoming Uploads (Server-Side)

```v2
// Inside an std.http request handler:
let form = multipart.parse(req)!    // parses Content-Type: multipart/form-data

// Access text fields
let username = form.field("username")!   // str

// Access uploaded files
let file = form.file("avatar")!
print(file.filename)       // "photo.jpg"
print(file.content_type)   // "image/jpeg"
print(file.size)           // bytes
let data: bytes = file.data()

// Save directly to disk
file.save("uploads/" + file.filename)

// Iterate all parts
for part in form.parts() {
    if part.is_file() {
        part.save("uploads/" + part.filename)
    } else {
        print(part.name, "=", part.value)
    }
}
```

### Building Outgoing Multipart Requests (Client-Side)

```v2
let form = multipart.new()

form.add_field("username", "alice")
form.add_file("avatar", path: "photo.jpg", content_type: "image/jpeg")
form.add_file_bytes("doc", data: pdf_bytes, filename: "report.pdf", content_type: "application/pdf")

let res = std.http.post(
    "https://api.example.com/upload",
    body:    form.body(),
    headers: {"Content-Type": form.content_type()}    // includes boundary
)
```

---

## std.ssh — SSH Client & SFTP

```v2
import std.ssh
```

### Connecting

```v2
// Password auth
let session = ssh.connect(
    host:     "server.example.com",
    port:     22,
    user:     "alice",
    password: "secret"
)!

// Key auth
let session = ssh.connect(
    host:       "server.example.com",
    user:       "alice",
    key_file:   "~/.ssh/id_ed25519",
    passphrase: ""    // leave empty if key is unencrypted
)!
```

### Remote Command Execution

```v2
let out = session.exec("ls -la /var/log")!
print(out.stdout)
print(out.stderr)
print(out.exit_code)

// Stream output
session.exec_stream("tail -f /var/log/syslog", func(line: str) {
    print(line)
})
```

### Interactive Shell

```v2
let shell = session.shell()
shell.write("cd /tmp\n")
shell.write("ls\n")
let output = shell.read_until("$")
shell.close()
```

### SFTP

```v2
let sftp = session.sftp()

// Upload
sftp.put(local: "build/app", remote: "/opt/myapp/app")

// Download
sftp.get(remote: "/var/log/app.log", local: "logs/app.log")

// List directory
let files = sftp.ls("/opt/myapp/")
for f in files {
    print(f.name, f.size, f.permissions)
}

// Other operations
sftp.mkdir("/opt/myapp/data")
sftp.rm("/opt/myapp/old.bin")
sftp.rename("/tmp/new.bin", "/opt/myapp/app")
sftp.chmod("/opt/myapp/app", 0o755)
```

### Port Tunneling

```v2
// Local port forwarding: localhost:8080 ? remote:80
let tunnel = session.forward_local(local_port: 8080, remote_host: "localhost", remote_port: 80)
tunnel.open()
// ... use localhost:8080 normally
tunnel.close()
```

### Closing

```v2
session.close()
```

---

## std.qr — QR Code Generation

```v2
import std.qr
```

### Generating a QR Code

```v2
// Generate as PNG image (returns bytes)
let png: bytes = qr.encode("https://example.com")

// Save directly to file
qr.save("https://example.com", path: "qr.png")

// Generate as SVG string
let svg: str = qr.encode_svg("https://example.com")
```

### Options

```v2
let png = qr.encode(
    "https://example.com",
    size:          400,          // image size in pixels
    margin:        4,            // quiet zone in modules
    error_correction: "H",      // "L" | "M" | "Q" | "H" (7% / 15% / 25% / 30%)
    foreground:    "#000000",
    background:    "#ffffff"
)
```

### Embedding a Logo

```v2
let logo = read_file("logo.png")
let png = qr.encode(
    "https://example.com",
    logo:       logo,
    logo_size:  0.25    // logo takes up 25% of center (must match error_correction capacity)
)
```

### Batch Generation

```v2
let codes = qr.encode_batch([
    "https://example.com/a",
    "https://example.com/b",
    "https://example.com/c",
])
// returns list<bytes>
```

---

## std.markdown — Markdown Parsing & Rendering

```v2
import std.markdown
```

### Parse to AST

```v2
let doc = markdown.parse("# Hello\n\nThis is **bold**.")
// returns a MarkdownDoc AST

for node in doc.nodes() {
    match node {
        case (Heading(level, text)) { print("H" + level, text) }
        case (Paragraph(inlines)) { print("p:", inlines) }
        case (Code(lang, src)) { print("code:", lang) }
        case (List(ordered, items)) { ... }
        case (BlockQuote(inner)) { ... }
        case (ThematicBreak) { ... }
        case (Table(head, rows)) { ... }
    }
}
```

### Render to HTML

```v2
let html: str = markdown.to_html("# Hello\n\nWorld")
// "<h1>Hello</h1>\n<p>World</p>"

// From parsed AST
let html = doc.to_html()

// With options
let html = markdown.to_html(src,
    sanitize:          true,     // strip raw HTML
    highlight_code:    true,     // syntax-highlight fenced code blocks
    smart_punctuation: true,     // smart quotes, em-dashes
    gfm:               true      // GitHub Flavored Markdown (tables, task lists, strikethrough)
)
```

### Render to Plain Text

```v2
let text: str = markdown.to_text("# Hello\n\n**World**")
// "Hello\n\nWorld"
```

### Render to ANSI Terminal

```v2
let ansi: str = markdown.to_ansi("# Hello\n\n**bold** text")
print(ansi)    // prints with colors and styles in terminal
```

### Frontmatter Extraction

```v2
let result = markdown.parse_frontmatter(src)    // YAML or TOML frontmatter
print(result.meta)    // dict of frontmatter keys
print(result.body)    // markdown without frontmatter
```

---

## std.archive — ZIP & TAR Archives

```v2
import std.archive
```

`std.archive` is archive-centric (open/edit/list/extract ZIP/TAR containers). It is not an alias of `std.compress`; use `std.compress` for raw gzip/zstd/brotli/lz4 codec operations and lightweight archive helper calls.

### Creating a ZIP

```v2
let zip = archive.zip_new()

zip.add_file("src/main.v2")
zip.add_file("src/utils.v2", archive_path: "utils.v2")    // rename inside zip
zip.add_bytes("README.txt", data: "Hello!".to_bytes())
zip.add_dir("assets/")      // adds entire directory recursively

zip.save("project.zip")
let data: bytes = zip.to_bytes()    // in-memory zip
```

### Extracting a ZIP

```v2
let zip = archive.zip_open("project.zip")

let names: list<str> = zip.list()    // all entry names
zip.extract("output/")               // extract all
zip.extract_file("main.v2", to: "extracted/main.v2")   // single file

let data: bytes = zip.read("main.v2")    // read without extracting
let text: str   = zip.read_text("README.txt")
```

### Creating a TAR

```v2
let tar = archive.tar_new(compression: "gz")   // "none" | "gz" | "bz2" | "xz"

tar.add_file("src/main.v2")
tar.add_dir("assets/")

tar.save("project.tar.gz")
```

### Extracting a TAR

```v2
let tar = archive.tar_open("project.tar.gz")   // auto-detects compression

let names = tar.list()
tar.extract("output/")
tar.extract_file("main.v2", to: "extracted/main.v2")

let data: bytes = tar.read("main.v2")
```

### Format Reference

| Format      | Extension          | Notes                            |
| ----------- | ------------------ | -------------------------------- |
| ZIP         | `.zip`             | Random access, widely compatible |
| TAR (raw)   | `.tar`             | Sequential, no compression       |
| TAR + gzip  | `.tar.gz` / `.tgz` | Common on Linux/macOS            |
| TAR + bzip2 | `.tar.bz2`         | Better compression than gz       |
| TAR + xz    | `.tar.xz`          | Best compression                 |

---

## Weak References

By default, V2's reference-counted heap values keep their target alive as long as a reference exists. **Weak references** hold a non-owning handle to a heap value — they do not prevent the value from being freed, and automatically become invalid once the value is collected. This is the primary tool for breaking reference cycles.

### Creating a Weak Reference

```v2
let strong = SomeClass { data: 42 }
let weak = weak_ref(strong)
```

### Accessing the Value

```v2
let maybe = weak.get()    // returns T | null

match maybe {
    case (Some(val)) {
        print(val.data)    // still alive
    }
    case (null) { print("freed") }
}
```

### Checking Liveness

```v2
if weak.is_alive() {
    let val = weak.get()!
    use(val)
}
```

### Breaking Reference Cycles

The classic cycle scenario — a parent holds children, children hold a back-reference to their parent:

```v2
class Node {
    value:    int,
    children: list<Node>,
    parent:   weak_ref<Node> | null    // weak — does not keep parent alive
}

let root  = Node { value: 1, children: [], parent: null }
let child = Node { value: 2, children: [], parent: weak_ref(root) }
root.children.push(child)

// When root goes out of scope, both root and child are freed —
// the weak back-reference from child ? root does not form a cycle.
```

### Weak References in Caches

```v2
// Cache that doesn't prevent GC of its entries
let cache: dict<str, weak_ref<ExpensiveObj>> = {}

func get_or_load(key: str) -> ExpensiveObj {
    if cache.has(key) and cache[key].is_alive() {
        return cache[key].get()!
    }
    let obj = load_expensive(key)
    cache[key] = weak_ref(obj)
    return obj
}
```

### Notes

- `weak_ref<T>` is a generic type — the type parameter matches the pointed-to type.
- Calling `.get()` is the only safe way to access the value; the returned `T | null` must be checked before use.
- Weak references are free-threaded — `.get()` is safe to call from any isolate or thread.
- Do not use weak references as a substitute for proper ownership design; use them specifically to break ownership cycles.

---

## Newtype Wrappers

The `newtype` keyword creates a **distinct named type** that wraps an existing type. Unlike a type alias (`type Foo = int`), a newtype is a fully separate type — you cannot accidentally pass a `UserId` where an `OrderId` is expected even though both wrap `int`.

### Declaring a Newtype

```v2
newtype UserId  = int
newtype OrderId = int
newtype Email   = str
```

### Constructing and Unwrapping

```v2
let uid: UserId  = UserId(42)
let oid: OrderId = OrderId(42)

// Type-safe: these are NOT interchangeable
func get_user(id: UserId) { ... }

get_user(uid)    // OK
get_user(oid)    // compile error: expected UserId, got OrderId
get_user(42)     // compile error: expected UserId, got int

// Unwrap with .0 or .inner()
let raw: int = uid.0          // 42
let raw2     = uid.inner()    // 42
```

### Methods on Newtypes

Newtypes can have `impl` blocks exactly like structs:

```v2
newtype Meters = float
newtype Feet   = float

impl Meters {
    func to_feet(self) -> Feet {
        return Feet(self.0 * 3.28084)
    }
}

let dist = Meters(10.0)
let ft   = dist.to_feet()    // Feet(32.8084)
```

### Implementing Traits for Newtypes

```v2
newtype Score = int

impl Comparable for Score {
    func compare(self, other: Score) -> int {
        return self.0 - other.0
    }
}

impl Default for Score {
    func default() -> Score { return Score(0) }
}
```

### Deriving Traits Automatically

```v2
@derive(Default, Comparable, Hash, Clone)
newtype PlayerId = int
```

### Newtypes vs Type Aliases

| Feature              | `type Alias = T`        | `newtype Wrap = T`            |
| -------------------- | ----------------------- | ----------------------------- |
| Distinct type?       | No — just another name  | Yes — separate type           |
| Can use T's methods? | Yes                     | No (must re-impl or delegate) |
| Accidental mixing    | Possible                | Compile-time error            |
| Performance overhead | None                    | None (zero-cost)              |
| Use when             | Readability, long names | Type-safe domain values       |

---

## The `@derive` Decorator

`@derive` automatically generates trait implementations for structs, newtypes, and enums whose fields all implement the requested traits. It eliminates boilerplate for common patterns.

### Syntax

```v2
@derive(Trait1, Trait2, ...)
struct Foo { ... }
```

### Derivable Traits

| Trait         | Generated behavior                                       |
| ------------- | -------------------------------------------------------- |
| `Default`     | `Foo.default()` — calls `.default()` on every field      |
| `Clone`       | `clone(val)` — deep-copies every field                   |
| `Eq`          | `==` and `!=` — field-by-field equality                  |
| `Ord`         | `<`, `>`, `<=`, `>=` — lexicographic field ordering      |
| `Hash`        | `hash(val)` — combines hashes of all fields              |
| `Display`     | `str(val)` — `"Foo { field: value, ... }"`               |
| `Comparable`  | `compare(a, b)` — lexicographic ordering                 |
| `Serialize`   | `serialize(val, fmt)` — encode to JSON/TOML/MessagePack  |
| `Deserialize` | `T.deserialize(data, fmt)` — decode from serialized form |

`@derive` accepts both canonical trait names and legacy aliases. For example, `@derive(Clone)` and `@derive(Cloneable)` are equivalent, but canonical names are preferred in new code.

### Example

```v2
@derive(Default, Clone, Eq, Hash, Display)
struct Point {
    x: float,
    y: float
}

let p1 = Point.default()          // Point { x: 0.0, y: 0.0 }
let p2 = clone(p1)                // independent copy
print(p1 == p2)                   // true
print(str(p1))                    // "Point { x: 0.0, y: 0.0 }"
```

### Enum Derive

```v2
@derive(Eq, Display)
enum Direction { North, South, East, West }

print(str(Direction.North))       // "North"
print(Direction.North == Direction.South)  // false
```

### Newtype Derive

```v2
@derive(Default, Clone, Eq, Ord, Hash, Display)
newtype Score = int
```

### Compile-Time Error on Missing Field Impl

If any field does not implement a required derived trait, `@derive` produces a **compile-time error** pointing at the offending field:

```v2
@derive(Hash)
struct Bad {
    name: str,
    data: list    // error: list does not implement Hash
}
```

### Custom Derive

You can write custom derivable traits using `comptime` macros:

```v2
comptime func derive_Serializable(T) {
    ct_emit(f'
        impl Serializable for {T} {{
            func serialize(self) -> str {{
                return serialize.json_encode(dict(self))
            }}
        }}
    ')
}

@derive(Serializable)
struct Config { host: str, port: int }
```

### Built-in Serialize / Deserialize

`@derive(Serialize, Deserialize)` generates automatic encoding and decoding for JSON, TOML, MessagePack, and any format supported by `std.serialize`. Field names map to keys by default; use `@field` annotations to customize.

```v2
import "std.serialize"

@derive(Serialize, Deserialize)
struct User {
    name: str,
    age: int,
    @field("email_address")    // rename in serialized form
    email: str,
    @skip                       // exclude from serialization
    cache: dict?
}

// Serialize
let u = User { name: "Alice", age: 30, email: "alice@example.com", cache: None }
let json = serialize.to_json(u)
// {"name": "Alice", "age": 30, "email_address": "alice@example.com"}

let toml = serialize.to_toml(u)
let msgpack = serialize.to_msgpack(u)

// Deserialize
let u2 = User.deserialize(json, "json")
let u3 = User.deserialize(toml, "toml")

// Round-trip
expect_eq(u.name, u2.name)
expect_eq(u.age, u2.age)
```

Annotation reference for `@derive(Serialize, Deserialize)`:

| Annotation                 | Effect                                                      |
| -------------------------- | ----------------------------------------------------------- |
| `@field("key")`            | Use `"key"` instead of the field name in serialized form    |
| `@skip`                    | Exclude the field from serialization entirely               |
| `@default(value)`          | Use `value` when the field is absent during deserialization |
| `@flatten`                 | Inline nested struct fields into the parent object          |
| `@rename_all("camelCase")` | Apply naming convention to all fields (class-level)         |

Supported naming conventions for `@rename_all`: `"camelCase"`, `"snake_case"`, `"PascalCase"`, `"SCREAMING_SNAKE_CASE"`, `"kebab-case"`.

---

## Built-in Trait Catalog

V2 has a set of built-in traits that the compiler, standard library, and language features depend on. Implementing these on your own types lets them participate in sorting, hashing, formatting, iteration, and more.

This section defines canonical trait names. Legacy aliases from earlier sections (`Printable`, `Cloneable`, `Equatable`, `Hashable`) remain fully supported.

---

### `Clone`

Produces an independent deep copy of a value.

```v2
trait Clone {
    func clone(self) -> Self
}
```

The global `clone(val)` function calls `val.clone()`. All primitive types and standard collections implement `Clone`. Use `@derive(Clone)` for structs where all fields are `Clone`.

```v2
let a = [1, 2, 3]
let b = clone(a)    // independent copy — mutating b does not affect a
```

---

### `Eq`

Defines structural equality (`==` and `!=`).

```v2
trait Eq {
    func eq(self, other: Self) -> bool
}
```

When you implement `Eq`, the `==` and `!=` operators call `eq`. The default `==` for classes is reference equality; implementing `Eq` makes it structural.

```v2
struct Point { x: float, y: float }

impl Eq for Point {
    func eq(self, other: Point) -> bool {
        return self.x == other.x and self.y == other.y
    }
}

Point{x:1.0, y:2.0} == Point{x:1.0, y:2.0}    // true
```

---

### `Ord` and `Comparable`

`Comparable` is the primary ordering trait used by `sort()` and comparison operators on custom types.

```v2
trait Comparable {
    func compare(self, other: Self) -> int
    // Return negative if self < other, 0 if equal, positive if self > other
}
```

Once `Comparable` is implemented, `<`, `>`, `<=`, `>=`, `sort()`, `min()`, `max()` all work.

`Ord` is a marker trait extending `Comparable` that asserts a **total** order (every pair is comparable). Implementing `Ord` allows the type to be used as a dict key and in ordered sets.

```v2
struct Version { major: int, minor: int, patch: int }

impl Comparable for Version {
    func compare(self, other: Version) -> int {
        if (self.major != other.major) { return self.major - other.major }
        if (self.minor != other.minor) { return self.minor - other.minor }
        return self.patch - other.patch
    }
}

let versions = [Version{2,0,0}, Version{1,9,0}, Version{1,10,0}]
sort(versions)    // [1.9.0, 1.10.0, 2.0.0]
```

---

### `Hash`

Required for a type to be used as a dict key or set element.

```v2
trait Hash {
    func hash(self) -> int    // return a non-cryptographic integer hash
}
```

All primitive types implement `Hash`. Custom types must implement it to be used as keys:

```v2
struct Point { x: int, y: int }

impl Hash for Point {
    func hash(self) -> int {
        return hash(self.x) ^ (hash(self.y) * 2654435761)
    }
}

impl Eq for Point { ... }    // Hash types must also implement Eq

let map: dict<Point, str> = {}
map[Point{x:1, y:2}] = "hello"
```

> Types used as dict keys or set elements must implement **both** `Hash` and `Eq`.

---

### `Display`

Controls how a value is converted to a human-readable string via `str(val)` and string interpolation.

```v2
trait Display {
    func to_str(self) -> str
}
```

When you implement `Display`, `str(val)`, `f"${val}"`, and `print(val)` all use string-conversion semantics.

`to_str(self)` is canonical and required for `Display`. Use `display(self)` as a convenience printer helper via `Printable`; it does not replace `to_str(self)` for trait conformance.

```v2
struct Color { r: int, g: int, b: int }

impl Display for Color {
    func to_str(self) -> str {
        return f"rgb(${self.r}, ${self.g}, ${self.b})"
    }
}

let c = Color { r: 255, g: 128, b: 0 }
print(c)              // rgb(255, 128, 0)
print(f"color: ${c}")  // color: rgb(255, 128, 0)
```

---

### `From` and `Into`

`From<T>` defines how to construct `Self` from a value of type `T`. `Into<T>` is the mirror — the compiler derives it automatically from any `From` impl.

```v2
trait From<T> {
    func from(val: T) -> Self    // static — no self parameter
}

trait Into<T> {
    func into(self) -> T
}
```

Implementing `From<T> for U` automatically gives `U` an `into()` method that produces a `T`.

```v2
newtype Celsius    = float
newtype Fahrenheit = float

impl From<Celsius> for Fahrenheit {
    func from(c: Celsius) -> Fahrenheit {
        return Fahrenheit(c.0 * 9.0 / 5.0 + 32.0)
    }
}

let boiling_c = Celsius(100.0)
let boiling_f = Fahrenheit.from(boiling_c)   // 212.0
let boiling_f2 = boiling_c.into()            // same — auto-derived Into
```

`From` / `Into` are also used by the `?` operator for automatic error conversion:

```v2
impl From<IOError> for AppError {
    func from(e: IOError) -> AppError {
        return AppError { message: f"IO error: ${e.message}" }
    }
}

func load() -> Result<str, AppError> {
    let data = read_file("data.txt")?    // IOError is auto-converted to AppError via From
    return Ok(data)
}
```

---

### `Iterable` and `Iterator`

Already documented in the Traits section; included here for catalog completeness.

| Trait      | Key method                    | Purpose                              |
| ---------- | ----------------------------- | ------------------------------------ |
| `Iterable` | `iter(self)`                  | Make a type usable in `for-in` loops |
| `Iterator` | `next(self)`, `is_done(self)` | The cursor returned by `iter()`      |

---

### `Sendable`

`Sendable` is a **marker trait** — it has no required methods. Implementing it declares that a value is safe to transfer across async worker boundaries or isolate boundaries without data races.

```v2
trait Sendable {}    // no methods required
```

The compiler enforces `Sendable` automatically when `async_workers > 1` or when passing values to an `isolate`. Any value captured by a cross-worker async task must be `Sendable`; non-`Sendable` captures are rejected at compile time.

**Types that are `Sendable` by default:**

- All primitive types (`int`, `float`, `bool`, `str`, `null`)
- `list`, `dict`, `set`, `tuple` — when all their element types are `Sendable`
- Structs — when all fields are `Sendable` (auto-derived)
- Classes — **not** `Sendable` by default; you must implement it explicitly

**Implementing `Sendable` on a class:**

```v2
class SafeCounter impl Sendable {
    // All internal state must be either immutable or protected
    // by atomic operations — the programmer's responsibility.
    let value: int = 0

    func increment() { self.value += 1 }
    func get() -> int { return self.value }
}
```

> Implementing `Sendable` on a class is a manual promise to the compiler that concurrent access to the type is safe. The compiler does not verify the internals — it trusts the declaration.

**Opting out of `Sendable` enforcement:**

If you need to pass a non-`Sendable` value to an async task and accept the responsibility, use `unsafe_send(val)`:

```v2
let handle = spawn(async lambda() {
    use(unsafe_send(my_non_sendable_val))
})
```

`unsafe_send` bypasses the trait check and is treated as an unsafe operation.

---

### Built-in Trait Implementation Summary

| Type                   | `Clone` | `Eq`             | `Comparable`      | `Hash`                   | `Display` |
| ---------------------- | ------- | ---------------- | ----------------- | ------------------------ | --------- |
| `int`, `float`, `bool` | ?       | ?                | ?                 | ?                        | ?         |
| `str`                  | ?       | ?                | ? (lexicographic) | ?                        | ?         |
| `list`                 | ?       | ? (element-wise) | ? (lexicographic) | ?                        | ?         |
| `dict`                 | ?       | ? (entry-wise)   | ?                 | ?                        | ?         |
| `set`                  | ?       | ?                | ?                 | ?                        | ?         |
| `tuple`                | ?       | ?                | ? (lexicographic) | ? (if elements hashable) | ?         |
| `null`                 | ?       | ?                | ?                 | ?                        | ?         |

---

## Numeric Casting

Use `as` to convert between numeric types explicitly. Casts are **value-converting** — the numeric value is adapted to fit the target type.

### Syntax

```v2
let x: int   = 42
let f: float = x as float      // 42.0
let i: int   = 3.99 as int     // 3  (truncates toward zero)
let b: u8    = 300 as u8       // 44 (wraps — 300 mod 256)
```

### Widening Casts (always safe)

```v2
let a: i32 = 1000
let b: i64 = a as i64       // always fits
let c: float = a as float   // may lose precision for very large ints
```

### Narrowing Casts (may truncate or wrap)

```v2
let big: i64 = 1_000_000_000_000
let small: i32 = big as i32   // wraps — only lower 32 bits kept

let f: float = 3.999
let i: int   = f as int       // 3 — truncates toward zero (not round)
```

### Float ? Int Rules

| Cast           | Behavior                               |
| -------------- | -------------------------------------- |
| `float as int` | Truncate toward zero (no rounding)     |
| `int as float` | Convert; large ints may lose precision |
| `f32 as f64`   | Always exact                           |
| `f64 as f32`   | May lose precision                     |

### Pointer Casts

```v2
let p: pointer = mem_alloc(4)
let addr: int = p as int       // cast pointer to integer address
let p2: pointer = addr as pointer   // cast integer back to pointer (unsafe)
```

Pointer-integer casts require an `unsafe` block:

```v2
unsafe {
    let addr: int = p as int
    let p2: pointer = addr as pointer
}
```

### Cast vs Coercion

`as` is an explicit cast — it never happens automatically. V2 does not silently coerce between numeric types. If you pass a `float` where an `int` is expected, the compiler will emit an error and you must add an explicit `as int`.

---

## Raw and Byte Strings

### Raw Strings

A raw string begins with `r"` and ends with `"`. Inside a raw string, **backslash sequences are not processed** — every character is literal. This is useful for regular expressions, Windows paths, and any content where escaping would be tedious.

```v2
let pattern = r"\d+\.\d+"         // literal backslashes — no need to write \\d
let path    = r"C:\Users\Alice"    // literal backslash — no need to write \\
let re      = r"^(\w+)\s+(\w+)$"
```

To include a double-quote inside a raw string, use a raw block with a `#` delimiter:

```v2
let s = r#"He said "hello" and left."#    // contains literal double quotes
let t = r##"ends with #" here"##           // use as many # as needed
```

The opening `r#"` must be closed by `"#` with the exact same number of `#` characters.

### Byte Strings

A byte string begins with `b"` and produces a `bytes` value (a list of unsigned 8-bit integers) rather than a `str`. Each character in the literal is stored as its ASCII/UTF-8 byte value.

```v2
let data: bytes = b"Hello"           // [72, 101, 108, 108, 111]
let header      = b"\x89PNG\r\n"     // hex and escape sequences work inside b""
```

Byte strings support the same escape sequences as regular strings (`\n`, `\r`, `\t`, `\xHH`, `\uHHHH`).

### Combining Raw and Byte Strings

```v2
let raw_bytes: bytes = rb"\xff\x00"   // rb"..." — raw bytes, no escape processing
// [92, 120, 102, 102, 92, 120, 48, 48]  — literal characters, not parsed escapes
```

### Multi-Line String Indentation Stripping

Triple-quoted strings (`"""..."""`) automatically strip **common leading indentation** from all lines (based on the indentation of the least-indented non-empty line). This lets you indent multi-line strings naturally in your code:

```v2
func make_html() {
    let html = """
        <html>
          <body>
            <p>Hello</p>
          </body>
        </html>
        """
    // Stripped result: "<html>\n  <body>\n    <p>Hello</p>\n  </body>\n</html>\n"
}
```

The closing `"""` may appear on its own line; its indentation sets the baseline. Characters to the left of the baseline are stripped from every line.

---

## Struct Update Syntax

You can create a new struct by copying an existing instance and overriding specific fields using `{ ...base, field: new_value }` syntax.

### Basic Usage

```v2
struct Config {
    host:    str,
    port:    int,
    tls:     bool,
    timeout: int
}

let defaults = Config { host: "localhost", port: 8080, tls: false, timeout: 30 }

// Override only port and tls — other fields copied from defaults
let prod = Config { ...defaults, port: 443, tls: true }

print(prod.host)     // "localhost"  (copied)
print(prod.port)     // 443          (overridden)
print(prod.tls)      // true         (overridden)
print(prod.timeout)  // 30           (copied)
```

### Rules

- Fields listed after `...base` override the base; fields not listed are copied verbatim.
- `...base` must appear **before** any overriding fields.
- Multiple base structs are not supported — only one `...` spread per literal.
- The base and the result must be the same struct type.
- Works on structs and `cstruct`, and on classes that support struct-literal construction.

```v2
// Chain multiple updates
let a = Config { ...defaults, port: 9000 }
let b = Config { ...a, tls: true }

// Works with variables as field values
let new_port = compute_port()
let c = Config { ...defaults, port: new_port }
```

---

## Bitfield Structs

Bitfield structs let you define a struct where individual fields occupy a specific **number of bits** within a packed integer. They are essential for working with hardware registers, binary protocols, and memory-mapped I/O.

### Syntax

Declare a bitfield struct with `bitfield struct`. Each field specifies its bit width after a colon:

```v2
bitfield struct Flags {
    read:     1,    // 1 bit
    write:    1,    // 1 bit
    execute:  1,    // 1 bit
    reserved: 5     // 5 bits — total = 8 bits (1 byte)
}
```

### Usage

```v2
let f = Flags { read: 1, write: 1, execute: 0, reserved: 0 }

print(f.read)       // 1
print(f.write)      // 1
print(f.execute)    // 0

f.execute = 1
```

### Specifying the Backing Integer Type

By default, the compiler packs fields into the smallest integer that fits. You can specify the backing type explicitly:

```v2
bitfield struct StatusReg: u32 {
    ready:    1,
    error:    1,
    overflow: 1,
    data:     13,    // 13-bit data payload
    reserved: 16
}
// total = 32 bits — backed by u32
```

### Reading & Writing Raw Integer Values

```v2
let raw: u32 = f.to_int()           // pack all fields into an integer
let g = StatusReg.from_int(0b101)   // unpack from integer
```

### Overlay on a Pointer (Hardware Registers)

```v2
bitfield struct ControlReg: u32 {
    enable:   1,
    mode:     2,
    reserved: 29
}

unsafe {
    let ctrl_reg = ControlReg.from_ptr(0xFFFE0000 as pointer)
    ctrl_reg.enable = 1
    ctrl_reg.mode   = 0b10
}
```

### Notes

- Bitfield field values are always unsigned integers unless the field name is prefixed with `s` (signed) — e.g. `soffset: 8` for a signed 8-bit field.
- Field order in memory follows declaration order (first field = lowest bits).
- Bitfield structs implement `cstruct`-compatible ABI by default and can be passed to C functions.

---

## Block Expressions

In V2, a `{ }` block can be used as an expression — the value of the block is the value of its **last expression** (without a trailing semicolon or `return`). This works anywhere an expression is expected.

### Basic Usage

```v2
let result = {
    let x = compute_a()
    let y = compute_b()
    x + y    // last expression — becomes the value of the block
}

print(result)    // value of x + y
```

### In Assignments and Return Positions

```v2
let label = {
    if (score >= 90) { "A" }
    elif (score >= 80) { "B" }
    else { "C" }
}

func classify(n: int) -> str {
    return {
        if (n < 0) { "negative" }
        elif (n == 0) { "zero" }
        else { "positive" }
    }
}
```

### Scoping

Variables declared inside a block expression are local to that block:

```v2
let x = {
    let temp = expensive_computation()
    let adjusted = temp * 1.1
    adjusted    // temp and adjusted go out of scope here
}
// temp and adjusted are not accessible here
```

### Combined with `match`

```v2
let msg = match (status) {
    case (200) {
        let body = parse(resp)
        f"OK: ${body}"
    }
    case (404) { "Not Found" }
    case (_) { "Unknown" }
}
```

---

## Do Blocks

A `do { ... }` block is an expression-oriented scope for multi-step computations. Like block expressions, the value of the last expression becomes the value of the whole block.

Use `do` when you want imperative local steps in places that expect a single expression (assignments, return values, pipeline stages, and arguments).

### Basic Usage

```v2
let total = do {
    let subtotal = price * qty
    let tax = subtotal * 0.07
    subtotal + tax
}
```

### In Return Positions

```v2
func classify(score: int) -> str {
    return do {
        if (score >= 90) { "A" }
        elif (score >= 80) { "B" }
        elif (score >= 70) { "C" }
        else { "F" }
    }
}
```

### With Pipelines

```v2
let normalized = raw
    |> do {
        let s = trim(_).lower()
        s.replace("_", "-")
    }
```

### Notes

- `do` creates a new scope; locals declared inside it do not leak out.
- `defer` works inside `do` and runs when the `do` scope exits.
- `break`/`continue` still target loops, not `do` blocks.
- Prefer `do` when a plain `{ ... }` expression becomes visually ambiguous inside larger expressions.

---

## Extension Methods

Extension methods let you add new methods to existing types — including foreign types from the standard library or third-party packages — without modifying the original definition or creating a wrapper. Inspired by Kotlin, Swift, and C#.

### Basic Extension

```v2
extend str {
    func word_count(self) -> int {
        return self.split(" ").filter(|w| w.len() > 0).len()
    }

    func is_palindrome(self) -> bool {
        let clean = self.lower().replace(" ", "")
        return clean == clean.reverse()
    }
}

print("hello world".word_count())       // 2
print("racecar".is_palindrome())         // true
```

### Extension with Generics

```v2
extend list<T> {
    func second(self) -> T? {
        return if (self.len() >= 2) Some(self[1]) else None
    }

    func window(self, size: int) -> list<list<T>> {
        return [self[i:i+size] for i in 0..self.len()-size+1]
    }
}

print([10, 20, 30].second())    // Some(20)
print([1,2,3,4,5].window(3))    // [[1,2,3], [2,3,4], [3,4,5]]
```

### Extension with Trait Bounds

You can constrain extensions to types that implement certain traits:

```v2
extend list<T> where T: Ord {
    func is_sorted(self) -> bool {
        for (i in 1..self.len()) {
            if (self[i] < self[i-1]) { return false }
        }
        return true
    }
}

print([1, 2, 3].is_sorted())    // true
print([3, 1, 2].is_sorted())    // false
// [1, 2, 3].is_sorted() compiles only because int implements Ord
```

### Static Extensions

```v2
extend int {
    static func from_hex(s: str) -> int {
        return int(s, base: 16)
    }
}

let val = int.from_hex("FF")    // 255
```

### Scoped Visibility

Extensions are scoped to the module where they are defined. They are visible to importing modules only when explicitly imported:

```v2
// string_helpers.v2
pub extend str {
    func shout(self) -> str { return self.upper() + "!" }
}

// main.v2
import "string_helpers"    // brings the extension into scope
print("hello".shout())     // "HELLO!"
```

### Rules and Restrictions

- Extensions cannot add stored fields — only methods and static functions.
- Extensions cannot override existing methods on the type.
- Extension methods are resolved at compile time (static dispatch), not at runtime. They do not participate in dynamic dispatch or trait object vtables.
- If two imported extensions define the same method name for the same type, the compiler raises a **name collision** error at the import site.

---

## Trait Coherence and Orphans

V2 enforces **coherence** — at most one implementation of a given trait for a given type may exist across the entire program. This prevents ambiguous method resolution.

### The Orphan Rule

You may implement a trait for a type only if **at least one of** — the trait or the type — is defined in the **current crate**:

| Trait                 | Type                  | Allowed?                     |
| --------------------- | --------------------- | ---------------------------- |
| Your crate            | Your crate            | ?                            |
| Your crate            | Foreign (stdlib, dep) | ? — you own the trait        |
| Foreign (stdlib, dep) | Your crate            | ? — you own the type         |
| Foreign               | Foreign               | ? — orphan impl, not allowed |

```v2
// Your crate defines MySummary
trait MySummary { func summarize(self) -> str }

// OK — you own MySummary, even though str is foreign
impl MySummary for str {
    func summarize(self) -> str { return self[:50] }
}

// NOT OK — both Display (foreign) and list (foreign) are external
impl Display for list {    // compile error: orphan implementation
    func display(self) -> str { ... }
}
```

### Blanket Implementations

A **blanket impl** implements a trait for all types satisfying a bound. These must also satisfy the orphan rule:

```v2
// Implement Display for anything that implements MySummary
impl<T: MySummary> Display for T {
    func display(self) -> str {
        return self.summarize()
    }
}
```

### Newtype Pattern for Orphan Workaround

When you need to implement a foreign trait for a foreign type, wrap the type in a newtype:

```v2
newtype MyList = list    // your crate owns MyList

impl Display for MyList {    // OK — MyList is yours
    func display(self) -> str {
        return self.0.join(", ")
    }
}
```

---

## Cross-Compilation

V2 supports cross-compilation to a range of targets via `--os` and `--arch` flags (or `v2.toml`).

### Supported Target Triples

| `--os` value | `--arch` values                        | Notes                                            |
| ------------ | -------------------------------------- | ------------------------------------------------ |
| `linux`      | `x86_64`, `arm64`, `riscv64`           | Full stdlib support                              |
| `windows`    | `x86_64`, `arm64`                      | Full stdlib support                              |
| `macos`      | `x86_64`, `arm64`                      | Full stdlib support                              |
| `android`    | `arm64`, `x86_64`                      | NDK required; `std.ui` uses Android Views        |
| `ios`        | `arm64`                                | Xcode SDK required; `std.ui` uses UIKit          |
| `none`       | `x86_64`, `arm64`, `riscv64`, `wasm32` | Bare-metal / freestanding — no OS, no stdlib I/O |
| `wasm`       | `wasm32`                               | Browser / WASI — see WebAssembly section         |

### Cross-Compile Examples

```bash
v2 -c --target native --os android --arch arm64 app.v2
v2 -c --target native --os ios --arch arm64 app.v2
v2 -c --target native --os none --arch riscv64 firmware.v2   # bare-metal
```

### Mobile Target Tier

Mobile targets are available as an explicit tier in V2's cross-compilation model:

- `android` and `ios` are supported native targets with dedicated SDK/NDK integration.
- ABI compatibility is stable within a major V2 release, but mobile runtime adapters may evolve between minors.
- `std.ui`, `std.http`, `std.net`, and `std.crypto` are expected to work on both targets; low-level host APIs depend on platform capabilities.

Recommended release process for mobile apps:

1. Build with pinned SDK/NDK/Xcode versions in CI.
2. Run device-level integration tests for both architectures (`arm64`, `x86_64` where applicable).
3. Verify runtime capability checks for permissions (camera, notifications, accessibility, etc.).

### Bare-Metal (`--os none`)

When targeting `none`, the standard library is reduced to:

- `std.math` — fully available
- `std.collections`, `std.serialize` — fully available
- `std.hal` — hardware abstraction (timers, gpio, uart, spi, i2c)
- `std.io`, `std.fs`, `std.net` — available when using `bare_profile = "hosted"` with board/runtime adapters
- `std.proc` — available in `hosted` profile via supervisor adapters; unavailable in `minimal` and `realtime` profiles
- `std.ffi` — available for calling C functions from a linked C runtime

The entry point for bare-metal targets is `func _start()` rather than `func main()`.

```v2
// firmware.v2 — bare-metal entry point
pub func _start() -> never {
    init_hardware()
    loop()
}
```

#### Bare-Metal Runtime Profiles

```toml
[[target]]
os = "none"
arch = "arm64"
bare_profile = "realtime"   # "minimal" | "hosted" | "realtime"
```

Profile behavior:

| Profile    | Goal                                   | Runtime behavior                                       |
| ---------- | -------------------------------------- | ------------------------------------------------------ |
| `minimal`  | Tiny footprint                         | No allocator, no async runtime                         |
| `hosted`   | Feature parity on boards with adapters | Enables adapter-backed `std.io/std.fs/std.net`         |
| `realtime` | Deterministic latency                  | Incremental bounded GC + `@no_alloc` critical sections |

### `v2.toml` Cross-Compile Configuration

```toml
[[target]]
os   = "android"
arch = "arm64"
out  = "build/android/libapp.so"

[[target]]
os   = "ios"
arch = "arm64"
out  = "build/ios/app.a"
```

Run `v2 build` to build all declared targets sequentially.

---

## Inline Value Types

The `@inline` annotation instructs the compiler to allocate a struct directly on the stack (or inline inside its parent object) instead of heap-allocating it. This is a performance hint for small, short-lived value types — similar to C# `struct`, Java's Project Valhalla value types, and Swift's value semantics.

### Basic Usage

```v2
@inline
struct Vec2 {
    x: float,
    y: float
}

let v = Vec2 { x: 1.0, y: 2.0 }    // allocated on the stack, no heap allocation
let w = v                              // copied by value, not by reference
```

### When to Use `@inline`

Use `@inline` for types that are:

- **Small** (1–4 fields, all primitive or other inline types)
- **Short-lived** (local variables, loop intermediates, return values)
- **Frequently allocated** (inner-loop math vectors, color values, coordinate pairs)

```v2
@inline
struct Color {
    r: byte, g: byte, b: byte, a: byte
}

@inline
struct Rect {
    x: float, y: float, width: float, height: float
}

// These are stack-allocated and copied by value — no GC pressure
func bounding_box(rects: list<Rect>) -> Rect {
    var min_x = float.MAX
    var min_y = float.MAX
    var max_x = float.MIN
    var max_y = float.MIN
    for (r in rects) {
        min_x = math.min(min_x, r.x)
        min_y = math.min(min_y, r.y)
        max_x = math.max(max_x, r.x + r.width)
        max_y = math.max(max_y, r.y + r.height)
    }
    return Rect { x: min_x, y: min_y, width: max_x - min_x, height: max_y - min_y }
}
```

### Rules

- `@inline` types are always passed and assigned **by value** (copy semantics).
- They cannot be used with reference-based features: no `&` borrowing, no identity comparison (`is`).
- `@inline` types cannot participate in class inheritance — use with `struct` only.
- The compiler may reject `@inline` on types that are too large (configurable threshold, default 64 bytes) with a warning suggesting heap allocation instead.
- `@inline` types can implement traits and be used as generic type parameters.

### Inline Arrays

For fixed-size arrays of inline types, use `@inline` on a wrapper:

```v2
@inline
struct Mat4 {
    data: [float; 16]    // fixed-size inline array — 128 bytes on stack
}
```

---

## Profiling

V2 has built-in CPU and memory profiling support, accessible via `v2 profile`.

### Running the Profiler

```bash
v2 profile app.v2                    # profile and print summary to stdout
v2 profile --out profile.vtprof app.v2   # write raw profile data to file
v2 profile --flame app.v2            # generate flamegraph.svg in current directory
v2 profile --mem app.v2              # memory allocation profiling only
v2 profile --cpu app.v2              # CPU time profiling only (default)
```

### In-Code Profiling Regions

Mark specific regions of code for profiling using the `@profile` decorator or the `profile_start` / `profile_end` builtins:

```v2
@profile("render")
func render_frame() {
    // This function's time is tracked under the "render" label
}

// Or manual start/stop
profile_start("parse")
let ast = parse_source(src)
profile_end("parse")
```

### Reading Profile Data Programmatically

```v2
import std.proc

let report = proc.profile_report()    // dict of label ? {calls, total_ms, avg_ms}
for (label in report.keys()) {
    print(f"${label}: ${report[label]['avg_ms']:.3f} ms avg over ${report[label]['calls']} calls")
}
```

### Flamegraph Output

`--flame` generates an interactive SVG flamegraph:

```bash
v2 profile --flame app.v2
# Opens flamegraph.svg — shows call stack depth and hot paths
```

---

## std.dns — DNS Resolution

```v2
import std.dns
```

### Forward Lookup (Name ? IP)

```v2
// Resolve to a list of IP addresses (IPv4 and/or IPv6)
let addrs = dns.lookup("example.com")!
// ["93.184.216.34", "2606:2800:220:1:248:1893:25c8:1946"]

// Resolve to first IPv4 only
let ipv4 = dns.lookup_ipv4("example.com")!    // "93.184.216.34"

// Resolve to first IPv6 only
let ipv6 = dns.lookup_ipv6("example.com")!
```

### Reverse Lookup (IP ? Name)

```v2
let hostname = dns.reverse("93.184.216.34")!   // "example.com"
```

### MX, TXT, CNAME, and Other Record Types

```v2
let mx    = dns.query("example.com", "MX")!    // list of MX records
let txt   = dns.query("example.com", "TXT")!   // list of TXT strings
let cname = dns.query("www.example.com", "CNAME")!
let ns    = dns.query("example.com", "NS")!
let a     = dns.query("example.com", "A")!     // IPv4 records
let aaaa  = dns.query("example.com", "AAAA")!  // IPv6 records
let srv   = dns.query("_http._tcp.example.com", "SRV")!
```

### Custom Resolver

```v2
let resolver = dns.resolver(
    nameservers: ["8.8.8.8", "1.1.1.1"],
    timeout_ms:  3000,
    retries:     2
)

let addrs = resolver.lookup("internal.corp")!
```

### Full API

| Function                     | Description                                        |
| ---------------------------- | -------------------------------------------------- |
| `dns.lookup(host)`           | Resolve hostname ? `list<str>` of IPs              |
| `dns.lookup_ipv4(host)`      | First IPv4 address ? `str`                         |
| `dns.lookup_ipv6(host)`      | First IPv6 address ? `str`                         |
| `dns.reverse(ip)`            | Reverse PTR lookup ? hostname `str`                |
| `dns.query(host, type)`      | Query a specific record type ? list of records     |
| `dns.resolver(opts)`         | Create a custom resolver with specific nameservers |
| `resolver.lookup(host)`      | Use custom resolver for forward lookup             |
| `resolver.query(host, type)` | Use custom resolver for record query               |

All functions return `Result` — use `!` to unwrap or `match` for explicit error handling.

---

## std.2d — 2D Vector Graphics

```v2
import std.2d
```

`std.2d` provides a 2D vector drawing API for creating SVGs, rendering to canvases, and compositing shapes. It targets both file output (SVG, PNG) and screen output (via `std.ui` or `std.game`).

### Creating a Canvas

```v2
let canvas = gfx2d.canvas(width: 800, height: 600, background: "#ffffff")
```

### Drawing Primitives

```v2
// Rectangle
canvas.rect(x: 10, y: 10, width: 200, height: 100, {
    fill:   "#3498db",
    stroke: "#2980b9",
    stroke_width: 2,
    radius: 8     // rounded corners
})

// Circle / Ellipse
canvas.circle(cx: 400, cy: 300, r: 50, { fill: "#e74c3c" })
canvas.ellipse(cx: 400, cy: 300, rx: 80, ry: 40, { fill: "#2ecc71" })

// Line
canvas.line(x1: 0, y1: 0, x2: 800, y2: 600, {
    stroke: "#000",
    stroke_width: 1,
    dash: [5, 3]    // dashed line: 5px dash, 3px gap
})

// Polygon
canvas.polygon(points: [(100, 10), (200, 190), (10, 190)], {
    fill: "#9b59b6",
    stroke: "#8e44ad"
})

// Polyline (open)
canvas.polyline(points: [(0,0),(100,50),(200,0),(300,50)], {
    stroke: "#1abc9c",
    fill: "none"
})
```

### Paths

```v2
let path = gfx2d.path()
path.move_to(50, 50)
path.line_to(150, 50)
path.curve_to(200, 50, 200, 150, 150, 150)  // cubic bezier
path.quad_to(100, 200, 50, 150)              // quadratic bezier
path.arc(cx: 100, cy: 100, r: 50, start: 0, end: 3.14159)
path.close()

canvas.draw(path, { fill: "#f39c12", stroke: "#d35400" })
```

### Text

```v2
canvas.text("Hello, V2!", x: 100, y: 200, {
    font:       "Arial",
    font_size:  24,
    font_weight: "bold",
    color:      "#2c3e50",
    align:      "center"    // "left" | "center" | "right"
})

// Multiline text box
canvas.text_box("Long text that wraps...", x: 50, y: 100, width: 300, {
    font_size: 14,
    line_height: 1.5
})
```

### Transforms

```v2
// Save/restore transform state
canvas.save()
canvas.translate(200, 200)
canvas.rotate(45)          // degrees
canvas.scale(1.5, 1.5)
canvas.rect(0, 0, 100, 100, { fill: "#e67e22" })
canvas.restore()
```

### Gradients and Patterns

```v2
let grad = gfx2d.linear_gradient(x1: 0, y1: 0, x2: 1, y2: 0)  // horizontal
grad.stop(0.0, "#3498db")
grad.stop(1.0, "#9b59b6")

canvas.rect(0, 0, 400, 200, { fill: grad })

let radial = gfx2d.radial_gradient(cx: 0.5, cy: 0.5, r: 0.5)
radial.stop(0.0, "#f39c12")
radial.stop(1.0, "#e74c3c")
```

### Image Compositing

```v2
let img = image.load("logo.png")
canvas.image(img, x: 10, y: 10, width: 100, height: 100)
canvas.image(img, x: 200, y: 200, opacity: 0.5)
```

### Output

```v2
canvas.save_svg("output.svg")           // save as SVG file
canvas.save_png("output.png")           // rasterize and save as PNG
canvas.save_png("output.png", scale: 2) // @2x retina resolution

let svg_str: str   = canvas.to_svg()    // SVG source as string
let png_bytes: bytes = canvas.to_bytes("png")
```

---

## std.graphql — GraphQL Client & Server

```v2
import std.graphql
```

### Client — Executing Queries

```v2
let client = graphql.client("https://api.example.com/graphql", {
    headers: {"Authorization": "Bearer " + token}
})

let result = client.query("""
    query GetUser($id: ID!) {
        user(id: $id) {
            name
            email
            posts { title }
        }
    }
""", vars: {"id": "42"})!

print(result["user"]["name"])
```

### Client — Mutations

```v2
let result = client.mutate("""
    mutation CreatePost($title: String!, $body: String!) {
        createPost(title: $title, body: $body) {
            id
            createdAt
        }
    }
""", vars: {"title": "Hello", "body": "World"})!
```

### Client — Subscriptions

```v2
let sub = client.subscribe("""
    subscription OnMessage($roomId: ID!) {
        message(roomId: $roomId) {
            id
            text
            sender { name }
        }
    }
""", vars: {"roomId": "main"})

for msg in sub {
    print(msg["message"]["text"])
}
sub.close()
```

### Server — Defining a Schema

```v2
let schema = graphql.schema("""
    type Query {
        user(id: ID!): User
        users: [User]
    }

    type Mutation {
        createUser(name: String!, email: String!): User
    }

    type User {
        id:    ID!
        name:  String!
        email: String!
    }
""")
```

### Server — Resolvers

```v2
schema.resolver("Query", "user", func(args, ctx) {
    return db_find_user(args["id"])
})

schema.resolver("Query", "users", func(args, ctx) {
    return db_list_users()
})

schema.resolver("Mutation", "createUser", func(args, ctx) {
    return db_create_user(args["name"], args["email"])
})
```

### Server — Serving

```v2
let server = graphql.server(schema, port: 4000)
server.start()
```

The GraphQL server integrates with `std.http` — you can mount it on an existing HTTP server:

```v2
import std.http

let http_server = http.server(port: 8080)
http_server.mount("/graphql", graphql.handler(schema))
http_server.start()
```

---

## std.webrtc — WebRTC

```v2
import std.webrtc
```

### Creating a Peer Connection

```v2
let pc = webrtc.peer_connection({
    ice_servers: [
        {"urls": "stun:stun.l.google.com:19302"},
        {"urls": "turn:turn.example.com", "username": "user", "credential": "pass"}
    ]
})
```

### Signaling (Offer / Answer)

```v2
// Caller side
let offer = pc.create_offer()!
pc.set_local_description(offer)
// Send offer.sdp to the remote peer via your signaling server

// Callee side — after receiving offer SDP
pc.set_remote_description({"type": "offer", "sdp": received_sdp})
let answer = pc.create_answer()!
pc.set_local_description(answer)
// Send answer.sdp back via signaling
```

### ICE Candidates

```v2
pc.on_ice_candidate(func(candidate) {
    if (candidate != null) {
        // Send candidate to remote via signaling
        signal_send(candidate)
    }
})

// When a remote candidate arrives via signaling:
pc.add_ice_candidate(received_candidate)
```

### Data Channels

```v2
let ch = pc.create_data_channel("chat")

ch.on_open(func() { ch.send("hello!") })
ch.on_message(func(msg: str) { print("received:", msg) })
ch.on_close(func() { print("channel closed") })

// On the remote side
pc.on_data_channel(func(ch) {
    ch.on_message(func(msg) { print("peer says:", msg) })
})
```

### Media Streams (Audio / Video)

```v2
// Capture local media
let stream = webrtc.get_user_media(audio: true, video: true)!

// Add to peer connection
pc.add_stream(stream)

// Receive remote stream
pc.on_track(func(track, stream) {
    // Render or process incoming audio/video track
})
```

---

## std.clipboard — Clipboard

```v2
import std.clipboard
```

### Reading

```v2
let text = clipboard.read_text()!    // read plain text from clipboard
let png  = clipboard.read_image()!   // read image as bytes (PNG format)

// Check what's available
let has_text  = clipboard.has_text()   // bool
let has_image = clipboard.has_image()  // bool
```

### Writing

```v2
clipboard.write_text("Hello from V2!")

let img_bytes = read_file("image.png")
clipboard.write_image(img_bytes)    // write PNG bytes as image

// Write multiple formats at once
clipboard.write({
    "text/plain": "plain text",
    "text/html":  "<b>rich text</b>"
})
```

### Monitoring

```v2
let watcher = clipboard.watch()

watcher.on_change(func(content) {
    if (content.has_text()) {
        print("clipboard changed:", content.read_text())
    }
})

watcher.start()
// ... later
watcher.stop()
```

---

## std.notify — Desktop Notifications

```v2
import std.notify
```

### Sending a Notification

```v2
notify.send("Build Complete", body: "Project compiled in 1.4s")

notify.send("Download Finished",
    body:  "file.zip is ready.",
    icon:  "path/to/icon.png",
    sound: true
)
```

### Actions (Buttons)

```v2
notify.send("New Message",
    body: "Alice says hi",
    actions: [
        {"id": "reply",   "label": "Reply"},
        {"id": "dismiss", "label": "Dismiss"}
    ],
    on_action: func(action_id: str) {
        if (action_id == "reply") { open_chat() }
    }
)
```

### Notification Levels

```v2
notify.send("Info",    body: "...", level: "info")     // default
notify.send("Warning", body: "...", level: "warning")
notify.send("Error",   body: "...", level: "critical")
```

### Persistent / Sticky Notifications

```v2
let n = notify.create("Progress", body: "0%", sticky: true)
n.show()

for (i in 0..=100) {
    n.update(body: f"${i}%")
    sleep(50)
}

n.close()
```

---

## std.speech — Text-to-Speech & Recognition

```v2
import std.speech
```

### Text-to-Speech

```v2
// Speak text using the system TTS engine
speech.say("Hello from V2!")

// With options
speech.say("Welcome back.", {
    voice:  "en-US-Jenny",    // voice name (system-dependent)
    rate:   1.0,              // speed: 0.5—2.0
    pitch:  1.0,              // pitch: 0.5—2.0
    volume: 0.8               // 0.0—1.0
})

// List available voices
let voices = speech.voices()    // list of {name, lang, gender}
```

### Saving Speech to File

```v2
speech.save("Hello, World!", path: "greeting.wav")
speech.save("Bonjour.", path: "fr.mp3", voice: "fr-FR-Denise")
```

### Speech Recognition

```v2
// Transcribe from microphone (blocking until silence)
let text = speech.recognize()!    // str

// Transcribe from audio file
let text = speech.recognize_file("audio.wav")!

// Streaming recognition — callback fires as words are recognized
let session = speech.recognize_stream(func(partial: str, final: bool) {
    if (final) {
        print("Recognized:", partial)
    }
})

session.start()
sleep(10000)
session.stop()
```

### Language Options

```v2
let text = speech.recognize(lang: "fr-FR")!
speech.say("Bonjour!", lang: "fr-FR")
```

---

## std.camera — Camera & Webcam

```v2
import std.camera
```

### Listing Devices

```v2
let devices = camera.devices()    // list of {id, name, is_default}
for d in devices {
    print(d["name"])
}
```

### Capturing a Still Frame

```v2
let cam = camera.open()           // open default camera
let frame = cam.capture()!        // capture one frame ? image (std.image compatible)

image.save(frame, "photo.jpg")
cam.close()

// Specific device
let cam2 = camera.open(device_id: "usb-001")!
```

### Video Stream

```v2
let cam = camera.open(width: 1280, height: 720, fps: 30)!

cam.on_frame(func(frame) {
    // frame is an image object (std.image compatible)
    let gray = image.grayscale(frame)
    process(gray)
})

cam.start()
sleep(5000)
cam.stop()
cam.close()
```

### Recording to File

```v2
let cam = camera.open()!
let recorder = cam.record("output.mp4", codec: "h264")!

recorder.start()
sleep(10000)   // record 10 seconds
recorder.stop()

cam.close()
```

---

## std.serial — Serial Port / UART

```v2
import std.serial
```

### Listing Ports

```v2
let ports = serial.list()    // list of {name, description, vendor_id, product_id}
// e.g. [{name: "/dev/ttyUSB0", description: "CP2102 USB to UART"}, ...]
```

### Opening a Port

```v2
let port = serial.open("/dev/ttyUSB0", {
    baud_rate:  115200,
    data_bits:  8,           // 5, 6, 7, or 8
    stop_bits:  1,           // 1 or 2
    parity:     "none",      // "none" | "even" | "odd"
    flow_control: "none"     // "none" | "software" | "hardware"
})!
```

### Reading and Writing

```v2
// Write bytes or string
port.write("AT\r\n")
port.write_bytes([0xFF, 0x01, 0x00])

// Read a line (blocks until \n or timeout)
let line = port.read_line(timeout_ms: 1000)!

// Read N bytes
let data: bytes = port.read_bytes(16, timeout_ms: 500)!

// Read all available bytes without blocking
let available = port.read_available()
```

### Event-Driven Reading

```v2
port.on_data(func(data: bytes) {
    process(data)
})

port.on_line(func(line: str) {
    print("received:", line)
})

port.start_listening()
// ... later
port.stop_listening()
port.close()
```

---

## std.usb — USB Devices

```v2
import std.usb
```

### Listing Devices

```v2
let devices = usb.list()    // list of {vendor_id, product_id, serial, manufacturer, product}

let filtered = usb.list(vendor_id: 0x1234)    // filter by vendor
```

### Opening a Device

```v2
let dev = usb.open(vendor_id: 0x1234, product_id: 0x5678)!

// Or by serial number
let dev = usb.open(serial: "ABC12345")!
```

### Bulk Transfer

```v2
// Write to endpoint
dev.write(endpoint: 0x01, data: [0xFF, 0x01, 0x02])

// Read from endpoint
let data: bytes = dev.read(endpoint: 0x81, length: 64, timeout_ms: 1000)!
```

### Control Transfer

```v2
dev.control_write(
    request_type: 0x40,
    request:      0x01,
    value:        0x0000,
    index:        0,
    data:         [0x01, 0x02]
)

let response: bytes = dev.control_read(
    request_type: 0xC0,
    request:      0x02,
    value:        0,
    index:        0,
    length:       8
)!
```

### Device Events

```v2
usb.on_connect(func(dev_info) {
    print("connected:", dev_info["product"])
})

usb.on_disconnect(func(dev_info) {
    print("disconnected:", dev_info["serial"])
})

usb.start_monitoring()
```

### Closing

```v2
dev.close()
```

---

## std.bluetooth — Bluetooth & BLE

```v2
import std.bluetooth
```

### Classic Bluetooth

```v2
// Scan for devices
let devices = bluetooth.scan(timeout_ms: 5000)!
// list of {address, name, rssi, paired}

// Connect
let conn = bluetooth.connect("AA:BB:CC:DD:EE:FF")!

// RFCOMM (serial-like) communication
conn.write("hello")
let data: bytes = conn.read(64)!

conn.close()
```

### Bluetooth Low Energy (BLE)

```v2
// Scan for BLE devices (filtered by service UUID)
let devices = bluetooth.ble_scan(
    timeout_ms:    5000,
    service_uuid:  "6E400001-B5A3-F393-E0A9-E50E24DCCA9E"   // optional filter
)!

// Connect
let dev = bluetooth.ble_connect(devices[0]["address"])!

// Discover services and characteristics
let services = dev.services()!
let chars    = dev.characteristics("6E400001-B5A3-F393-E0A9-E50E24DCCA9E")!

// Read / write characteristics
let value = dev.read_char("6E400003-B5A3-F393-E0A9-E50E24DCCA9E")!
dev.write_char("6E400002-B5A3-F393-E0A9-E50E24DCCA9E", data: [0x01])

// Subscribe to notifications
dev.notify("6E400003-B5A3-F393-E0A9-E50E24DCCA9E", func(data: bytes) {
    print("notification:", data)
})

dev.close()
```

### BLE Peripheral (Advertise)

```v2
let peripheral = bluetooth.ble_peripheral()

peripheral.add_service("6E400001-B5A3-F393-E0A9-E50E24DCCA9E", [
    {
        uuid:       "6E400002-B5A3-F393-E0A9-E50E24DCCA9E",
        properties: ["write"],
        on_write:   func(data: bytes) { print("written:", data) }
    },
    {
        uuid:       "6E400003-B5A3-F393-E0A9-E50E24DCCA9E",
        properties: ["notify", "read"],
        on_read:    func() -> bytes { return [0x01] }
    }
])

peripheral.start(name: "V2 Device")
```

---

## std.hotkey — Global Hotkeys

```v2
import std.hotkey
```

### Registering a Hotkey

```v2
// Register a global hotkey (works even when your app is not focused)
hotkey.register("Ctrl+Shift+S", func() {
    print("hotkey fired!")
    take_screenshot()
})

hotkey.register("Alt+F4", func() {
    exit(0)
})
```

### Modifier Keys

| Symbol                 | Key                           |
| ---------------------- | ----------------------------- |
| `Ctrl`                 | Control                       |
| `Alt`                  | Alt / Option                  |
| `Shift`                | Shift                         |
| `Meta` / `Win` / `Cmd` | Windows key / Command (macOS) |

Keys are joined with `+`. Examples: `"Ctrl+C"`, `"Alt+Shift+F"`, `"Meta+Space"`.

### Unregistering

```v2
let id = hotkey.register("Ctrl+Q", func() { quit() })
hotkey.unregister(id)
hotkey.unregister_all()
```

### Starting the Listener

```v2
hotkey.start()    // start listening (non-blocking)
// ... program continues
hotkey.stop()
```

---

## std.tray — System Tray

```v2
import std.tray
```

### Creating a Tray Icon

```v2
let tray = tray.icon(
    icon:    "assets/icon.png",    // path to 16x16 or 32x32 icon
    tooltip: "My V2 App"
)
```

### Context Menu

```v2
tray.menu([
    {label: "Open",   action: func() { open_window() }},
    {label: "---"},                         // separator
    {label: "Settings", action: func() { open_settings() }},
    {label: "---"},
    {label: "Quit",   action: func() { exit(0) }}
])
```

### Events

```v2
tray.on_click(func() { open_window() })
tray.on_right_click(func() { tray.show_menu() })
tray.on_double_click(func() { toggle_window() })
```

### Updating the Icon and Tooltip

```v2
tray.set_icon("assets/icon_alert.png")
tray.set_tooltip("My App — 3 pending items")
```

### Showing / Hiding

```v2
tray.show()
tray.hide()
```

### Starting

```v2
tray.start()    // registers the tray icon with the OS
```

---

## std.ipc — Inter-Process Communication

```v2
import std.ipc
```

### Named Pipes

```v2
// Server — create a named pipe and wait for connections
let server = ipc.pipe_server("/tmp/myapp.sock")!

server.on_connection(func(conn) {
    let msg = conn.read_line()!
    conn.write_line("echo: " + msg)
    conn.close()
})

server.start()

// Client — connect to named pipe
let client = ipc.pipe_connect("/tmp/myapp.sock")!
client.write_line("hello")
let response = client.read_line()!
print(response)    // "echo: hello"
client.close()
```

### Shared Memory

```v2
// Create a shared memory region (64 KB)
let shm = ipc.shm_create("myapp_shm", size: 65536)!

// Write into shared memory
shm.write(0, "hello from process A".to_bytes())

// In another process — attach to the same region
let shm2 = ipc.shm_open("myapp_shm")!
let data: bytes = shm2.read(0, 20)
print(str(data))    // "hello from process A"

shm.close()
shm2.close()
ipc.shm_delete("myapp_shm")
```

### Message Queues

```v2
// Create a message queue
let mq = ipc.mq_create("myapp_queue")!

// Send a message
mq.send("task payload", priority: 5)

// Receive a message (blocks until available)
let msg = mq.recv()!
print(msg.body)
print(msg.priority)

mq.close()
ipc.mq_delete("myapp_queue")
```

### Unix Domain Sockets (Stream & Datagram)

```v2
// Stream socket server
let srv = ipc.unix_server("/tmp/app.sock")!
let conn = srv.accept()!
let line = conn.read_line()!
conn.write_line("ack")

// Stream socket client
let sock = ipc.unix_connect("/tmp/app.sock")!
sock.write_line("ping")
let reply = sock.read_line()!
```

### Signals Between Processes

```v2
// Send a signal to another process by PID
ipc.signal_send(pid: 12345, signal: "SIGUSR1")

// Receive signals in this process (also available via std.signal)
import std.signal
signal.on("SIGUSR1", func() { reload_config() })
```

---

## std.decimal — Exact Decimal Arithmetic

```v2
import std.decimal
```

`std.decimal` provides arbitrary-precision decimal arithmetic with **no floating-point rounding errors**. Essential for financial calculations, tax, currency conversion, and any domain where `0.1 + 0.2 == 0.3` must be true.

### Creating Decimals

```v2
let a = decimal.new("0.1")          // from string — exact
let b = decimal.new("0.2")
let c = a + b                        // 0.3 — exact, no floating-point error
print(c)                             // "0.3"
print(c == decimal.new("0.3"))       // true

let price = decimal.new("19.99")
let tax   = decimal.new("0.21")     // 21%
let total = price * (decimal.new("1") + tax)   // 24.1879
```

### Rounding

```v2
let d = decimal.new("24.1879")

d.round(2)                          // 24.19 (HALF_UP by default)
d.round(2, mode: "half_up")        // 24.19
d.round(2, mode: "half_even")      // 24.19  (banker's rounding)
d.round(2, mode: "floor")          // 24.18
d.round(2, mode: "ceiling")        // 24.19
d.round(0, mode: "half_up")        // 24
```

### Arithmetic

```v2
let x = decimal.new("10.5")
let y = decimal.new("3.0")

x + y      // 13.5
x - y      // 7.5
x * y      // 31.5
x / y      // 3.5 (exact — no precision loss)
x % y      // 1.5
x ** 2     // 110.25
x.abs()    // 10.5
x.negate() // -10.5
```

### Comparison

```v2
let a = decimal.new("10.50")
let b = decimal.new("10.5")

a == b      // true — trailing zeros don't affect equality
a > b       // false
a.compare(b)  // 0
```

### Converting

```v2
decimal.new("3.14").to_float()    // 3.14 (float — may lose precision)
decimal.new("42.0").to_int()      // 42 (truncates decimal part)
decimal.new("1.5").to_str()       // "1.5"
decimal.from_float(3.14)          // decimal representation of the float
decimal.from_int(42)              // 42.0 as decimal
```

### Precision and Scale

```v2
let d = decimal.new("123.456")
d.precision()    // 6 (total significant digits)
d.scale()        // 3 (digits after decimal point)

// Set a global precision context (affects division and complex ops)
decimal.set_precision(28)    // default is 28 significant digits
```

---

## std.diff — Text Diffing & Patching

```v2
import std.diff
```

### Computing a Diff

```v2
let original = "line one\nline two\nline three\n"
let modified = "line one\nline TWO\nline three\nline four\n"

let d = diff.compute(original, modified)

// Unified diff (git-style)
print(diff.unified(original, modified))
// --- original
// +++ modified
// @@ -1,3 +1,4 @@
//  line one
// -line two
// +line TWO
//  line three
// +line four

// Side-by-side diff
print(diff.side_by_side(original, modified))
```

### Diffing Lines vs Characters vs Words

```v2
diff.compute(a, b)                     // line-level diff (default)
diff.compute(a, b, mode: "char")       // character-level diff
diff.compute(a, b, mode: "word")       // word-level diff
```

### Working with the Diff Result

```v2
let d = diff.compute(original, modified)

for op in d.ops() {
    match op {
        case (diff.Equal(text)) { print("  " + text) }
        case (diff.Insert(text)) { print("+ " + text) }
        case (diff.Delete(text)) { print("- " + text) }
    }
}

print("additions:", d.additions())   // number of added chars/lines
print("deletions:", d.deletions())   // number of deleted chars/lines
print("similarity:", d.similarity()) // 0.0—1.0 similarity ratio
```

### Applying a Patch

```v2
let patch_text = diff.unified(original, modified)

// Apply the patch to the original text
let result = diff.apply(original, patch_text)!
print(result == modified)    // true

// Fuzzy apply — tolerates small differences in the source
let result2 = diff.apply(slightly_different, patch_text, fuzzy: 3)!
```

### Saving and Loading Patches

```v2
// Save patch to file
write_file("changes.patch", diff.unified(a, b))

// Apply from file
let patched = diff.apply(original, read_file("changes.patch"))!
```

---

## std.semver — Semantic Versioning

```v2
import std.semver
```

### Parsing

```v2
let v = semver.parse("1.2.3")!
print(v.major)    // 1
print(v.minor)    // 2
print(v.patch)    // 3

let v2 = semver.parse("2.0.0-alpha.1+build.42")!
print(v2.pre)     // "alpha.1"
print(v2.build)   // "build.42"
```

### Comparison

```v2
let a = semver.parse("1.2.3")!
let b = semver.parse("1.10.0")!
let c = semver.parse("2.0.0-beta")!

a < b     // true  (minor 2 < 10)
b < c     // true  (1.x < 2.x)

semver.parse("1.0.0-alpha")! < semver.parse("1.0.0")!   // true — pre-release < release

sort([b, c, a])   // [1.2.3, 1.10.0, 2.0.0-beta]
```

### Version Range Matching

```v2
// Check if a version satisfies a constraint
semver.satisfies("1.5.0", "^1.2.0")    // true  — compatible range (same major)
semver.satisfies("2.0.0", "^1.2.0")    // false
semver.satisfies("1.2.3", "~1.2.0")    // true  — patch-compatible
semver.satisfies("1.3.0", "~1.2.0")    // false
semver.satisfies("1.5.0", ">=1.0.0 <2.0.0")  // true — range expression
semver.satisfies("3.0.0", "*")          // true  — any version
```

### Range Specifiers

| Spec             | Meaning                       |
| ---------------- | ----------------------------- |
| `^1.2.3`         | `>=1.2.3 <2.0.0` — same major |
| `~1.2.3`         | `>=1.2.3 <1.3.0` — same minor |
| `>=1.0.0`        | At least 1.0.0                |
| `>1.0.0 <=2.0.0` | Range                         |
| `1.2.x`          | `>=1.2.0 <1.3.0`              |
| `*`              | Any version                   |

### Incrementing Versions

```v2
let v = semver.parse("1.2.3")!

v.bump_major()    // 2.0.0
v.bump_minor()    // 1.3.0
v.bump_patch()    // 1.2.4
v.bump_pre("beta")   // 1.2.4-beta.1 (or increments pre-release)
```

### Maximum Satisfying Version

```v2
let versions = ["1.0.0", "1.5.0", "2.0.0", "2.1.0"].map(lambda(s) => semver.parse(s)!)
let best = semver.max_satisfying(versions, "^1.0.0")!   // 1.5.0
```

---

## std.geo — Geospatial

```v2
import std.geo
```

### Coordinate Types

```v2
let point = geo.point(lat: 52.2297, lng: 21.0122)   // Warsaw, Poland
let point2 = geo.point(lat: 48.8566, lng: 2.3522)   // Paris, France
```

### Distance Calculation

```v2
// Haversine distance (great-circle, accounts for Earth's curvature)
let dist_km = geo.distance(point, point2)                // km (default)
let dist_mi = geo.distance(point, point2, unit: "miles")
let dist_m  = geo.distance(point, point2, unit: "meters")

print(f"Warsaw to Paris: ${dist_km:.1f} km")
```

### Bearing and Navigation

```v2
let bearing = geo.bearing(point, point2)     // 0—360 degrees from north
let midpoint = geo.midpoint(point, point2)   // geo.Point halfway between

// Destination point from start + bearing + distance
let dest = geo.destination(point, bearing: 270.0, distance_km: 500.0)
```

### Bounding Box

```v2
let bbox = geo.bounding_box(center: point, radius_km: 50.0)
print(bbox.north, bbox.south, bbox.east, bbox.west)

// Check if a point is inside a bounding box
bbox.contains(some_point)   // bool
```

### Geocoding (requires network)

```v2
// Forward geocoding: address ? coordinates
let result = geo.geocode("Plac Zamkowy 4, Warszawa")!
print(result.lat, result.lng)    // 52.2477, 21.0142
print(result.formatted_address)

// Reverse geocoding: coordinates ? address
let addr = geo.reverse_geocode(52.2477, 21.0142)!
print(addr.city)     // "Warsaw"
print(addr.country)  // "Poland"
print(addr.formatted_address)
```

### GeoJSON

```v2
// Parse GeoJSON
let fc = geo.parse_geojson(read_file("regions.geojson"))

for feature in fc.features() {
    print(feature.properties["name"])
    print(feature.geometry.type)   // "Polygon", "Point", "LineString", ...
}

// Create GeoJSON
let point_geojson = geo.to_geojson(point)
let poly = geo.polygon([(0,0),(1,0),(1,1),(0,1),(0,0)])
let poly_geojson = geo.to_geojson(poly)
write_file("out.geojson", poly_geojson)
```

### Point-in-Polygon

```v2
let poly = geo.polygon([
    (52.20, 21.00),
    (52.25, 21.00),
    (52.25, 21.05),
    (52.20, 21.05),
    (52.20, 21.00)
])

poly.contains(geo.point(lat: 52.22, lng: 21.02))   // true
```

---

## std.gpu — GPU Compute

```v2
import std.gpu
```

`std.gpu` provides general-purpose GPU computation (GPGPU) via an abstraction over CUDA, Metal, and Vulkan Compute. It is designed for data-parallel numerical workloads — ML inference, simulations, image processing pipelines, etc.

### Querying Devices

```v2
let devices = gpu.devices()    // list of {name, vendor, vram_mb, api}

let dev = gpu.default_device()!
print(dev.name)     // "NVIDIA GeForce RTX 4090"
print(dev.vram_mb)  // 24576
```

### Buffers

```v2
// Upload data to GPU
let data = [1.0, 2.0, 3.0, 4.0]
let buf  = dev.buffer(data)!           // GPU buffer from list of floats

// Download back to CPU
let result: list<float> = buf.to_list()!

// Allocate uninitialized GPU buffer
let empty = dev.buffer_alloc(size: 1024, dtype: "f32")!
```

### Writing a Kernel

Kernels are written in V2 with the `@gpu_kernel` decorator and compiled to the target GPU API at build time:

```v2
@gpu_kernel
func add_arrays(a: gpu.Buffer<float>, b: gpu.Buffer<float>, out: gpu.Buffer<float>) {
    let i = gpu.thread_id()
    out[i] = a[i] + b[i]
}
```

### Running a Kernel

```v2
let a   = dev.buffer([1.0, 2.0, 3.0, 4.0])!
let b   = dev.buffer([10.0, 20.0, 30.0, 40.0])!
let out = dev.buffer_alloc(size: 4, dtype: "f32")!

dev.run(add_arrays, args: [a, b, out], threads: 4)!

let results = out.to_list()!   // [11.0, 22.0, 33.0, 44.0]
```

### Built-in GPU Operations

For common operations, `std.gpu` provides pre-compiled kernels:

```v2
// Matrix multiply on GPU (returns a new GPU buffer)
let c = gpu.matmul(dev, a_buf, b_buf, rows: 1024, cols: 1024, inner: 1024)!

// Element-wise ops
let sum  = gpu.add(dev, a_buf, b_buf)!
let prod = gpu.mul(dev, a_buf, b_buf)!

// Reduction
let total: float = gpu.reduce_sum(dev, a_buf)!
let max:   float = gpu.reduce_max(dev, a_buf)!

// Sort on GPU (returns sorted buffer)
let sorted = gpu.sort(dev, a_buf)!
```

### Integration with `std.ai`

When `std.gpu` is imported alongside `std.ai`, neural network operations automatically use GPU acceleration if a device is available:

```v2
import std.ai
import std.gpu

gpu.set_default(gpu.default_device()!)    // enable GPU for all std.ai ops

let model = ai_llm_load("mistral-7b-instruct")    // loads weights to GPU
let reply  = ai_llm_generate(model, "Hello!")      // inference runs on GPU
```

---

## std.accessibility — Accessibility APIs

```v2
import std.accessibility
```

`std.accessibility` provides programmatic access to the operating system's accessibility layer. It lets you read UI elements from other applications, drive UI automation, and build assistive tools.

### Reading UI Elements

```v2
// Get the focused application
let app = accessibility.focused_app()!
print(app.name)         // "Firefox"
print(app.pid)          // 12345

// Get all top-level windows
let windows = app.windows()!
for w in windows {
    print(w.title, w.role)
}

// Find elements by role or label
let buttons = app.find(role: "button")!
let search  = app.find(label: "Search", role: "text_field")!

// Read element properties
let btn = buttons[0]
print(btn.label)        // "Submit"
print(btn.role)         // "button"
print(btn.enabled)      // true
print(btn.focused)      // false
print(btn.bounds)       // {x, y, width, height}
```

### Performing Actions

```v2
btn.click()
search_field.set_value("hello")
search_field.focus()

// Keyboard simulation
accessibility.key_press("Return")
accessibility.key_combo("Ctrl+C")
accessibility.type_text("Hello, World!")
```

### Observing Events

```v2
let observer = accessibility.observe()

observer.on("focused", func(element) {
    print("focus moved to:", element.label)
})

observer.on("value_changed", func(element) {
    print(element.label, "changed to:", element.value)
})

observer.start()
// ... later
observer.stop()
```

### Screen Reader Mode

```v2
// Announce text to the system screen reader
accessibility.announce("File saved successfully.")

// Check if a screen reader is running
let active = accessibility.screen_reader_active()   // bool
```

### Permissions

On macOS and Windows, accessibility access requires user permission. V2 handles the permission request dialog automatically on first use, but you can check and prompt explicitly:

```v2
if (!accessibility.has_permission()) {
    accessibility.request_permission()   // shows OS permission dialog
}
```

---

## std.blockchain — Blockchain & Web3

```v2
import std.blockchain
```

`std.blockchain` provides chain clients, wallet/key tooling, transaction signing, contract interaction, and event streaming. It is designed to pair with `std.crypto`, `std.http`, and `std.db` for production services.

### Connecting to a Chain

```v2
let chain = blockchain.connect({
    chain: "ethereum",
    rpc_url: "https://rpc.ankr.com/eth"
})!

print(chain.chain_id())      // 1
print(chain.block_number())  // latest height
```

### Wallets and Keys

```v2
let wallet = blockchain.wallet_new()!
print(wallet.address)

let restored = blockchain.wallet_from_seed(
    "seed phrase words ...",
    account: 0,
    index: 0
)!

let from_pk = blockchain.wallet_from_private_key("0x...")!
```

### Native Transfer

```v2
let tx = chain.transfer_native({
    from: wallet,
    to: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e",
    amount: blockchain.units("0.05", "eth"),
    gas_policy: "auto"
})!

print(tx.hash)
print(chain.wait(tx.hash, confirmations: 1)!.status)   // "confirmed"
```

### Contract Calls

```v2
let erc20 = chain.contract(
    address: "0xA0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
    abi: read_file("erc20.abi.json")
)

let symbol = erc20.call("symbol", [])!
let balance = erc20.call("balanceOf", [wallet.address])!

let approve_tx = erc20.send(
    wallet,
    "approve",
    ["0xSpender...", blockchain.units("1000", "usdc")]
)!
```

### Event Streams

```v2
let sub = chain.subscribe({
    contract: erc20,
    event: "Transfer",
    from_block: "latest"
})

for evt in sub {
    print(evt.block_number, evt.args["from"], evt.args["to"], evt.args["value"])
}
```

### Notes

- Always verify chain IDs before signing transactions.
- Use hardware/KMS-backed signing in production where possible.
- Persist transaction intent/idempotency keys in `std.db` to avoid accidental replay.

---

## std.parse — Parser Combinators

```v2
import "std.parse"
```

`std.parse` provides reusable parser combinators for token streams and text-based grammars. It is intended for DSLs, config formats, protocol parsing, and compiler frontends.

### Core Combinators

| Function                    | Description                             |
| --------------------------- | --------------------------------------- |
| `parse.token(text)`         | Match an exact token/string             |
| `parse.regex(pattern)`      | Match regex at current input position   |
| `parse.seq(p1, p2, ...)`    | Run parsers in sequence                 |
| `parse.choice(p1, p2, ...)` | Try parsers in order until one succeeds |
| `parse.many(p, min?, max?)` | Repeat a parser                         |
| `parse.many1(p)`            | Repeat parser one or more times         |
| `parse.optional(p)`         | Optional parser (`None` on no match)    |
| `parse.map(p, fn)`          | Transform parser output                 |
| `parse.sep_by(p, sep)`      | Parse zero or more separated values     |
| `parse.sep_by1(p, sep)`     | Parse one or more separated values      |
| `parse.lazy(fn)`            | Recursive parser thunk                  |
| `parse.run(p, input)`       | Execute parser and return result record |

### Example — Comma-Separated Integers

```v2
let digit = parse.regex(r"\d")
let int_p = parse.map(parse.many1(digit), lambda(chars) {
    return int(chars.join(""))
})

let csv_ints = parse.sep_by1(int_p, parse.token(","))

let out = parse.run(csv_ints, "10,20,30")
out.ok        // true
out.value     // [10, 20, 30]
out.rest      // ""
```

### Whitespace Helpers

```v2
let ident = parse.regex(r"[A-Za-z_][A-Za-z0-9_]*")
let assign = parse.seq(
    parse.ws0(),
    ident,
    parse.ws0(),
    parse.token("="),
    parse.ws0(),
    parse.regex(r"\d+")
)
```

### Error Reporting

`parse.run` returns a structured result:

- `ok: bool`
- `value: any` (when `ok == true`)
- `rest: str` (unconsumed input)
- `error: { offset, line, col, expected }` (when `ok == false`)

```v2
let res = parse.run(csv_ints, "10,,30")
if (!res.ok) {
    print(res.error.line, res.error.col)
    print(res.error.expected)
}
```

---

## std.config — Unified Configuration

```v2
import "std.config"
```

`std.config` layers configuration from defaults, file formats, dotenv files, and environment overrides into one validated config object.
For dotenv support, `std.config` delegates parsing/loading primitives to `std.dotenv` so both modules share the same expansion and normalization behavior.

### Layered Loading

Precedence (lowest to highest):

1. defaults
2. config files (`.toml`, `.yaml`, `.json`)
3. dotenv values
4. process environment overrides
5. explicit CLI/runtime overrides

```v2
let cfg = config.load({
    defaults: {
        "server": {"host": "127.0.0.1", "port": 8080},
        "log": {"level": "info"}
    },
    files: ["config.toml", "config.yaml"],
    dotenv: ".env",
    env_prefix: "APP_",
    overrides: {
        "log.level": "debug"
    }
})!

print(cfg["server"]["host"])
```

### Dotenv Helpers

```v2
let env_map = config.dotenv_read(".env")!
print(env_map["DATABASE_URL"])

// Apply .env values into process environment (without replacing existing values)
config.dotenv_apply(".env", override: false)!
```

### Schema Validation

```v2
let schema = config.schema({
    "server.host": {type: "str", required: true},
    "server.port": {type: "int", min: 1, max: 65535, required: true},
    "log.level": {type: "str", one_of: ["debug", "info", "warn", "error"]}
})

config.validate(cfg, schema)!    // throws on validation errors
```

### API Reference

| Function                                | Description                                  |
| --------------------------------------- | -------------------------------------------- |
| `config.load(opts)`                     | Load and merge config layers                 |
| `config.merge(base, overlay, opts?)`    | Deep-merge config objects                    |
| `config.get(cfg, path, default?)`       | Read nested value by dot path                |
| `config.require(cfg, path, type?)`      | Read required value or throw                 |
| `config.schema(spec)`                   | Build schema descriptor                      |
| `config.validate(cfg, schema)`          | Validate config against schema               |
| `config.dotenv_read(path?)`             | Parse `.env` file into dict                  |
| `config.dotenv_apply(path?, override?)` | Apply `.env` values to process env           |
| `config.watch(path, fn)`                | Watch config file and invoke reload callback |

---

## std.event — Event Bus & PubSub

```v2
import "std.event"
```

`std.event` provides an in-process event bus for decoupled architecture: plugins, UI signals, domain events, and reactive workflows.

### Basic Publish/Subscribe

```v2
let bus = event.bus()

let sub = bus.on("user.created", lambda(evt) {
    print("new user:", evt["id"])
})

bus.emit("user.created", {"id": 42, "name": "alice"})
bus.off("user.created", sub)
```

### One-Time Handlers

```v2
bus.once("startup.ready", lambda(evt) {
    print("ready at", evt["ts"])
})

bus.emit("startup.ready", {"ts": time.now()})
bus.emit("startup.ready", {"ts": time.now()})   // second emit is ignored
```

### Buffered Async Delivery

```v2
let bus = event.bus({queue_size: 1024, delivery: "async"})

bus.on("email.send", lambda(job) {
    mail.send(job["to"], job["subject"], job["body"])
})

bus.emit("email.send", {
    "to": "ops@example.com",
    "subject": "Deploy complete",
    "body": "Build 182 shipped"
})
```

### API Reference

| Function                    | Description                                         |
| --------------------------- | --------------------------------------------------- |
| `event.bus(opts?)`          | Create event bus instance                           |
| `bus.on(topic, fn)`         | Subscribe handler; returns subscription id          |
| `bus.once(topic, fn)`       | Subscribe one-shot handler                          |
| `bus.off(topic, sub_id)`    | Remove one handler                                  |
| `bus.clear(topic?)`         | Remove handlers for one topic or all topics         |
| `bus.emit(topic, payload?)` | Publish event                                       |
| `bus.emit_many(events)`     | Publish batch of events                             |
| `bus.stats()`               | Queue depth, handler count, drops, dispatch latency |

---

## std.diag — Diagnostics & Observability

```v2
import "std.diag"
```

`std.diag` adds production observability primitives: metrics, spans, and health checks. It complements `std.log` and profiler tooling.

### Metrics

```v2
let requests = diag.counter("http_requests_total", labels: ["route", "method"])
let latency  = diag.histogram("http_latency_ms", buckets: [5, 10, 25, 50, 100, 250], labels: ["route"])
let in_flight = diag.gauge("http_in_flight", labels: ["route"])

in_flight.inc({"route": "/users"})
requests.inc({"route": "/users", "method": "GET"})
latency.observe(12.4, {"route": "/users"})
in_flight.dec({"route": "/users"})
```

### Timers

```v2
let db_timer = diag.timer("db_query_ms", labels: ["query"])

let stop = db_timer.start({"query": "users_by_id"})
let rows = db.query("SELECT * FROM users WHERE id = ?", [42])
stop()    // records elapsed time in histogram/timer metric
```

### Health Checks

```v2
diag.health("database", lambda() {
    return {"status": "ok", "details": {"pool": "ready"}}
})

diag.health("cache", lambda() {
    return cache.ping() ? {"status": "ok"} : {"status": "fail", "details": {"reason": "timeout"}}
})

let report = diag.health_report()
print(report["status"])     // "ok" or "degraded"
```

### Spans

```v2
let span = diag.span("http.request", {"route": "/users/:id", "method": "GET"})
span.event("handler.start")

// ... work ...

span.end(status: "ok")
```

### API Reference

| Function                      | Description                             |
| ----------------------------- | --------------------------------------- |
| `diag.counter(name, opts?)`   | Monotonic counter                       |
| `diag.gauge(name, opts?)`     | Gauge metric (up/down/set)              |
| `diag.histogram(name, opts?)` | Bucketed distribution metric            |
| `diag.timer(name, opts?)`     | Duration recorder helper                |
| `diag.span(name, attrs?)`     | Create tracing span                     |
| `diag.health(name, fn)`       | Register health-check callback          |
| `diag.health_report()`        | Run all checks and aggregate status     |
| `diag.exporter(kind, opts?)`  | Configure metrics/trace exporter        |
| `diag.reset()`                | Reset in-memory counters and registries |

### OpenTelemetry Export

`std.diag` natively supports the OpenTelemetry Protocol (OTLP) for exporting metrics, traces, and logs to observability backends — Jaeger, Grafana Tempo, Datadog, Honeycomb, or any OTLP-compatible collector.

```v2
import "std.diag"

// Configure OTLP exporter — traces and metrics
diag.exporter("otlp", {
    "endpoint": "http://localhost:4317",    // gRPC endpoint (default)
    "protocol": "grpc",                      // "grpc" or "http"
    "headers": {"Authorization": f"Bearer ${env("OTEL_TOKEN")}"},
    "service_name": "my-api",
    "service_version": "1.2.0",
    "environment": "production"
})

// Alternatively, export to specific backends
diag.exporter("jaeger", {"endpoint": "http://jaeger:14268/api/traces"})
diag.exporter("prometheus", {"port": 9090, "path": "/metrics"})
```

Once an exporter is configured, all `diag.span()`, `diag.counter()`, `diag.histogram()`, and other instrumentation calls automatically publish data to the configured backend.

#### Distributed Tracing with Context Propagation

Traces propagate across service boundaries via W3C Trace Context headers:

```v2
// Service A — outbound request
let span = diag.span("service_a.call_b", {"target": "service-b"})
let resp = await http.get("http://service-b/api", {
    "headers": diag.inject_trace_headers()    // injects traceparent, tracestate
})
span.end()

// Service B — inbound request handler
server.use(lambda(req, res, next) {
    diag.extract_trace_headers(req.headers)    // restores parent trace context
    let span = diag.span("service_b.handler")
    next()
    span.end()
})
```

| Function                              | Description                                             |
| ------------------------------------- | ------------------------------------------------------- |
| `diag.exporter(kind, opts)`           | Register exporter: `"otlp"`, `"jaeger"`, `"prometheus"` |
| `diag.inject_trace_headers()`         | Returns dict of W3C Trace Context headers               |
| `diag.extract_trace_headers(headers)` | Restore trace context from incoming request headers     |
| `diag.current_trace_id()`             | Get the active trace ID (or `None`)                     |
| `diag.current_span_id()`              | Get the active span ID (or `None`)                      |

---

## std.iot — IoT & Embedded

```v2
import "std.iot"
```

`std.iot` provides board-level hardware APIs for microcontroller and edge targets: GPIO, PWM, ADC/DAC, I2C, SPI, UART adapters, and device capability probing.

### Board and Capability Discovery

```v2
let board = iot.board()
print(board.name)
print(board.capabilities())   // ["gpio", "pwm", "i2c", ...]
```

### GPIO / PWM / ADC / DAC

```v2
iot.gpio_mode(13, "out")
iot.gpio_write(13, true)

let pwm = iot.pwm_open(pin: 12, freq_hz: 1000)
pwm.set_duty(0.5)

let temp_raw = iot.adc_read(pin: 0)
iot.dac_write(pin: 1, val: 2048)   // 12-bit mid-scale output
```

### I2C / SPI

```v2
let i2c = iot.i2c_open(bus: 1, freq_hz: 400_000)
let whoami = i2c.read(addr: 0x68, reg: 0x75, len: 1)

let spi = iot.spi_open(bus: 0, mode: 0, speed_hz: 8_000_000)
let rx = spi.transfer([0x9F, 0x00, 0x00, 0x00])
```

### API Reference

| Function                            | Description                                                 |
| ----------------------------------- | ----------------------------------------------------------- |
| `iot.board()`                       | Get board/platform metadata and capabilities                |
| `iot.gpio_mode(pin, mode)`          | Configure GPIO pin mode (`in`, `out`, `pullup`, `pulldown`) |
| `iot.gpio_write(pin, val)`          | Set digital pin output                                      |
| `iot.gpio_read(pin)`                | Read digital pin input                                      |
| `iot.pwm_open(pin, freq_hz)`        | Open PWM channel                                            |
| `iot.adc_read(pin)`                 | Read analog input value                                     |
| `iot.dac_write(pin, val)`           | Write analog output value                                   |
| `iot.i2c_open(bus, freq_hz?)`       | Open I2C bus adapter                                        |
| `iot.spi_open(bus, mode, speed_hz)` | Open SPI bus adapter                                        |

---

## std.hal — Hardware Abstraction Layer

```v2
import std.hal
```

`std.hal` is the low-level hardware abstraction layer for **bare-metal** targets (`--os none`). It provides direct access to timers, GPIO, UART, SPI, and I2C peripherals without an OS. Use `std.iot` for higher-level IoT workflows on hosted platforms; use `std.hal` when running on bare metal with no OS kernel.

> `std.hal` is only available when compiling with `--os none`. Importing it on a hosted target is a compile-time error.

### GPIO

```v2
let pin = hal.gpio(13)

pin.set_mode("output")        // "input" | "output" | "input_pullup" | "input_pulldown" | "analog"
pin.write(true)                // set high
pin.write(false)               // set low

pin.set_mode("input_pullup")
let state: bool = pin.read()   // digital read

// Interrupt on edge
pin.on_edge("rising", func() {     // "rising" | "falling" | "both"
    handle_interrupt()
})
```

### Timers

```v2
let timer = hal.timer(0)      // hardware timer index

timer.set_period_us(1000)     // 1 ms period
timer.on_tick(func() {
    toggle_led()
})
timer.start()

// One-shot delay
hal.delay_us(500)
hal.delay_ms(10)
```

### UART

```v2
let uart = hal.uart(1, baud: 115200, tx_pin: 17, rx_pin: 16)

uart.write("AT\r\n")
uart.write_bytes([0xFF, 0x01])

let line = uart.read_line(timeout_us: 100_000)!
let data: bytes = uart.read_bytes(8, timeout_us: 50_000)!

uart.on_rx(func(data: bytes) {
    process(data)
})
```

### SPI

```v2
let spi = hal.spi(0, {
    clock_hz:  8_000_000,
    mode:      0,           // 0, 1, 2, or 3 (CPOL/CPHA)
    bit_order: "msb",       // "msb" | "lsb"
    cs_pin:    10
})

let rx: bytes = spi.transfer([0x9F, 0x00, 0x00])   // simultaneous write+read
spi.write([0x06])                                    // write-only
let id: bytes = spi.read(3)                          // read-only
```

### I2C

```v2
let i2c = hal.i2c(0, freq_hz: 400_000, sda_pin: 21, scl_pin: 22)

// Write to a device register
i2c.write(addr: 0x68, data: [0x6B, 0x00])     // wake up MPU6050

// Read from a register
let who_am_i = i2c.read(addr: 0x68, reg: 0x75, len: 1)!

// Scan the bus for connected devices
let devices: list<int> = i2c.scan()    // list of responding addresses
```

### ADC / DAC

```v2
let raw: int = hal.adc_read(pin: 0, bits: 12)     // 12-bit ADC (0–4095)
let voltage: float = hal.adc_read_mv(pin: 0) / 1000.0

hal.dac_write(pin: 1, value: 2048, bits: 12)      // 12-bit DAC mid-scale
```

### DMA (Direct Memory Access)

```v2
let dma = hal.dma(channel: 0)

dma.mem_to_peripheral(
    src:  buffer.ptr(),
    dst:  spi.data_register(),
    len:  buffer.len(),
    on_complete: func() { print("transfer done") }
)
dma.start()
```

### Critical Sections

```v2
hal.critical(func() {
    // Interrupts are disabled inside this block
    shared_counter += 1
})

// Or use the @no_alloc directive for time-critical interrupt handlers
@no_alloc
func timer_isr() {
    hal.gpio(13).toggle()
}
```

### API Reference

| Function                           | Description                               |
| ---------------------------------- | ----------------------------------------- |
| `hal.gpio(pin)`                    | Get GPIO handle for a pin                 |
| `hal.timer(index)`                 | Get hardware timer handle                 |
| `hal.uart(index, opts)`            | Open UART peripheral                      |
| `hal.spi(index, opts)`             | Open SPI peripheral                       |
| `hal.i2c(index, opts)`             | Open I2C peripheral                       |
| `hal.adc_read(pin, bits?)`         | Read analog-to-digital converter          |
| `hal.adc_read_mv(pin)`             | Read ADC as millivolts                    |
| `hal.dac_write(pin, value, bits?)` | Write digital-to-analog converter         |
| `hal.dma(channel)`                 | Get DMA channel handle                    |
| `hal.delay_us(microseconds)`       | Busy-wait delay in microseconds           |
| `hal.delay_ms(milliseconds)`       | Busy-wait delay in milliseconds           |
| `hal.critical(fn)`                 | Execute function with interrupts disabled |
| `hal.reset()`                      | Software reset the microcontroller        |
| `hal.unique_id()`                  | Read the chip's unique hardware ID        |
| `hal.clock_hz()`                   | Current system clock frequency            |

---

## std.office — Office Documents

```v2
import "std.office"
```

`std.office` supports DOCX, PPTX, and RTF creation/read workflows for business reporting and document automation.

### DOCX Example

```v2
let doc = office.docx_new()
doc.heading("Quarterly Report", level: 1)
doc.paragraph("Revenue grew 18% quarter-over-quarter.")
doc.table([
    ["Region", "Revenue"],
    ["EMEA", "4.2M"],
    ["NA",   "5.8M"]
])
doc.save("report.docx")
```

### PPTX Example

```v2
let deck = office.pptx_new()
let slide = deck.slide(title: "Launch Metrics")
slide.bullets(["Daily active users +22%", "Crash rate -31%"])
deck.save("launch.pptx")
```

### API Reference

| Function                 | Description                      |
| ------------------------ | -------------------------------- |
| `office.docx_new(opts?)` | Create DOCX document builder     |
| `office.docx_read(path)` | Read DOCX document model         |
| `office.pptx_new(opts?)` | Create PPTX presentation builder |
| `office.pptx_read(path)` | Read PPTX presentation model     |
| `office.rtf_new(opts?)`  | Create RTF document builder      |
| `office.rtf_read(path)`  | Read RTF content model           |

---

## std.money — Money & Financial Arithmetic

```v2
import "std.money"
```

`std.money` provides currency-safe arithmetic on top of decimal precision, with explicit rounding policies and conversion helpers.

### Money Values

```v2
let a = money.new("10.25", currency: "USD")
let b = money.new("2.10", currency: "USD")
let total = money.add(a, b)        // USD 12.35
```

Mixed-currency operations throw unless a conversion is explicitly requested.

### Rounding Modes

```v2
money.round(total, scale: 2, mode: "HALF_EVEN")
money.round(total, scale: 2, mode: "HALF_UP")
money.round(total, scale: 0, mode: "DOWN")
```

### FX Conversion

```v2
let rates = money.rates({"USD/EUR": "0.92", "EUR/USD": "1.087"})
let eur = money.convert(total, to: "EUR", rates: rates, mode: "HALF_EVEN")
```

### API Reference

| Function                             | Description                               |
| ------------------------------------ | ----------------------------------------- |
| `money.new(amount, currency)`        | Create typed money value                  |
| `money.add(a, b)`                    | Add monetary values of same currency      |
| `money.sub(a, b)`                    | Subtract monetary values of same currency |
| `money.mul(m, scalar)`               | Multiply by scalar                        |
| `money.div(m, scalar, mode?)`        | Divide with explicit rounding policy      |
| `money.round(m, scale, mode)`        | Round amount with selected mode           |
| `money.convert(m, to, rates, mode?)` | Convert currency using provided rates     |
| `money.format(m, locale?)`           | Locale-aware money formatting             |

---

## std.dotenv — Environment Files

```v2
import "std.dotenv"
```

`std.dotenv` parses and loads `.env` files, including optional variable expansion. It is intentionally lightweight and pairs with `std.config` for layered configuration.
`std.config.dotenv_read` and `std.config.dotenv_apply` call the same parser/loader primitives under the hood, so dotenv behavior stays consistent across both modules.

### Loading `.env`

```v2
dotenv.load(".env", override: false)!
print(getenv("DATABASE_URL"))
```

### Parsing Text

```v2
let env = dotenv.parse("""
HOST=127.0.0.1
PORT=8080
URL=http://${HOST}:${PORT}
""", expand: true)

print(env["URL"])   // http://127.0.0.1:8080
```

### API Reference

| Function                        | Description                                      |
| ------------------------------- | ------------------------------------------------ |
| `dotenv.load(path?, override?)` | Load `.env` values into process env              |
| `dotenv.parse(text, expand?)`   | Parse dotenv string into dict                    |
| `dotenv.read(path?)`            | Read dotenv file into dict (no env mutation)     |
| `dotenv.set(key, val)`          | Set env variable with dotenv-style normalization |
| `dotenv.require(keys)`          | Ensure required env keys exist                   |

---

## std.scrape — Web Scraping & Browser Automation

```v2
import "std.scrape"
```

`std.scrape` offers extraction helpers for HTTP+HTML workflows and optional headless browser automation for JS-rendered pages and UI testing.

### Runtime Backend

`scrape.browser` runs on V2's managed headless backend layer:

- `chromium` is bundled and used by default.
- `webkit` is optional and enabled when the matching runtime bundle is present on the host.

Pick explicitly when needed:

```v2
let browser = scrape.browser(headless: true, backend: "chromium")
```

### HTTP + Selector Extraction

```v2
let page = scrape.fetch("https://example.com")!
let title = scrape.select_text(page.html, "title")
let links = scrape.select_attr(page.html, "a", "href")
```

### Headless Browser Flow

```v2
let browser = scrape.browser(headless: true)
let tab = browser.new_page()
tab.goto("https://example.com/login")
tab.fill("#email", "user@example.com")
tab.fill("#password", "secret")
tab.click("button[type=submit]")
tab.wait_for(".dashboard")
let html = tab.html()
browser.close()
```

### API Reference

| Function                                | Description                                             |
| --------------------------------------- | ------------------------------------------------------- | ---------- |
| `scrape.fetch(url, opts?)`              | Fetch page and return response + parsed metadata        |
| `scrape.select(html, css)`              | Select matching DOM nodes                               |
| `scrape.select_text(html, css)`         | Extract text for first matching node                    |
| `scrape.select_attr(html, css, attr)`   | Extract attribute values for matches                    |
| `scrape.browser(opts?)`                 | Start headless browser controller (`backend: "chromium" | "webkit"`) |
| `scrape.robots_allow(url, user_agent?)` | Check robots policy for path                            |

---

## std.map — Geospatial Rendering

```v2
import "std.map"
```

`std.map` renders geospatial layers (points, lines, polygons, GeoJSON) into static SVG/PNG/PDF map outputs and tile-backed previews.

### Static Map Rendering

```v2
let m = map.scene(center: [52.2297, 21.0122], zoom: 11)
m.add_geojson(read_file("districts.geojson"), style: {stroke: "#2a4", fill: "#a4d"})
m.add_marker([52.2297, 21.0122], label: "Center")

let svg = m.render_svg(width: 1200, height: 800)
write_file("warsaw.svg", svg)
```

### API Reference

| Function                          | Description                                      |
| --------------------------------- | ------------------------------------------------ |
| `map.scene(opts)`                 | Create map scene with projection/camera settings |
| `scene.add_geojson(data, style?)` | Add GeoJSON layer                                |
| `scene.add_marker(point, opts?)`  | Add point marker annotation                      |
| `scene.add_path(points, style?)`  | Add polyline/polygon layer                       |
| `scene.render_svg(opts?)`         | Render map scene to SVG                          |
| `scene.render_png(opts?)`         | Render map scene to PNG                          |
| `map.tile_source(name_or_url)`    | Configure tile source provider                   |

---

## std.task — Persistent Task Queue

```v2
import "std.task"
```

`std.task` provides durable background job execution with retry policies, scheduled runs, and dead-letter handling.

### Queue and Worker

```v2
let q = task.queue("emails", backend: "sqlite://tasks.db")

q.enqueue("send_welcome", {"user_id": 42}, retry: {max: 5, backoff: "exp"})

task.worker(q, concurrency: 4, lambda(job) {
    if (job.name == "send_welcome") {
        send_welcome_email(job.payload["user_id"])
    }
})
```

### Supported Backends

| Backend DSN      | Durability                      | Multi-worker | Notes                                      |
| ---------------- | ------------------------------- | ------------ | ------------------------------------------ |
| `sqlite://...`   | Durable local file              | Single host  | Default local backend                      |
| `postgres://...` | Durable remote DB               | Yes          | Recommended for distributed workers        |
| `redis://...`    | Durable when AOF/RDB is enabled | Yes          | High-throughput queue workloads            |
| `memory://`      | Process memory only             | No           | Testing/dev only; state is lost on restart |

### Scheduled Jobs

```v2
q.schedule("nightly_summary", cron: "0 2 * * *", payload: {"region": "all"})
```

### API Reference

| Function                                     | Description                                                                             |
| -------------------------------------------- | --------------------------------------------------------------------------------------- |
| `task.queue(name, backend)`                  | Open/create persistent task queue (`sqlite://`, `postgres://`, `redis://`, `memory://`) |
| `queue.enqueue(name, payload, opts?)`        | Push immediate job                                                                      |
| `queue.schedule(name, cron, payload, opts?)` | Register recurring scheduled job                                                        |
| `task.worker(queue, opts, fn)`               | Start worker loop for queue                                                             |
| `queue.ack(job_id)`                          | Mark job successful                                                                     |
| `queue.retry(job_id, reason?)`               | Retry failed job per policy                                                             |
| `queue.dead_letter(job_id, reason?)`         | Move job to dead-letter queue                                                           |

---

## std.phone — Telephony & SMS

```v2
import "std.phone"
```

`std.phone` exposes provider-neutral APIs for SMS messaging, OTP delivery, and basic voice call orchestration.

### SMS and OTP

```v2
let client = phone.client(provider: "twilio", api_key: getenv("PHONE_API_KEY"))

client.sms_send(to: "+12025550123", from: "+12025550999", body: "Your code is 348219")!
let token = client.otp_start(to: "+12025550123", ttl_sec: 300)!
```

### Voice

```v2
client.call_start(
    to: "+12025550123",
    from: "+12025550999",
    tts: "Your appointment is tomorrow at 09:00"
)!
```

### API Reference

| Function                         | Description                  |
| -------------------------------- | ---------------------------- |
| `phone.client(opts)`             | Create provider client       |
| `client.sms_send(opts)`          | Send SMS message             |
| `client.sms_status(id)`          | Query SMS delivery status    |
| `client.otp_start(opts)`         | Issue OTP verification token |
| `client.otp_verify(token, code)` | Verify OTP code              |
| `client.call_start(opts)`        | Start outbound voice call    |
| `client.call_status(id)`         | Query call status            |

---

## std.barcode — Barcode Generation & Scanning

```v2
import "std.barcode"
```

`std.barcode` handles 1D/2D barcode generation and decoding for retail, logistics, inventory, and ticketing workflows.

### Generate Barcodes

```v2
let ean = barcode.encode("EAN13", "5901234123457")!
write_file("ean13.svg", barcode.render_svg(ean))

let code128 = barcode.encode("Code128", "INV-2026-00017")!
image.save(barcode.render_png(code128, scale: 3), "label.png")
```

### Decode from Image

```v2
let decoded = barcode.decode_image("scan.jpg")!
print(decoded.type)
print(decoded.value)
```

### API Reference

| Function                             | Description                              |
| ------------------------------------ | ---------------------------------------- |
| `barcode.encode(kind, value, opts?)` | Encode value into barcode object         |
| `barcode.render_svg(code, opts?)`    | Render barcode to SVG                    |
| `barcode.render_png(code, opts?)`    | Render barcode to bitmap                 |
| `barcode.decode_image(path, opts?)`  | Decode barcode(s) from image file        |
| `barcode.is_valid(kind, value)`      | Validate payload against symbology rules |

---

## std.ml.vision — Vision Pipelines

```v2
import "std.ml.vision"
```

`std.ml.vision` provides high-level computer vision tasks: classification, object detection, OCR, and segmentation.

### Detection and OCR

```v2
let model = ml.vision.load("yolo-v8n")!
let boxes = ml.vision.detect(model, "street.jpg", threshold: 0.4)

for (b in boxes) {
    print(b.label, b.score, b.bbox)
}

let text = ml.vision.ocr("invoice.png", lang: "en")!
print(text)
```

### API Reference

| Function                                    | Description                    |
| ------------------------------------------- | ------------------------------ |
| `ml.vision.load(model_name_or_path, opts?)` | Load vision model              |
| `ml.vision.classify(model, image, opts?)`   | Image classification           |
| `ml.vision.detect(model, image, opts?)`     | Object detection               |
| `ml.vision.segment(model, image, opts?)`    | Semantic/instance segmentation |
| `ml.vision.ocr(image, opts?)`               | OCR text extraction            |

---

## std.ml.audio — Audio ML Pipelines

```v2
import "std.ml.audio"
```

`std.ml.audio` covers speech transcription, diarization, keyword spotting, and audio event classification.

### Transcription and Classification

```v2
let asr = ml.audio.load("whisper-small")!
let tr = ml.audio.transcribe(asr, "meeting.wav", lang: "en")!
print(tr.text)

let cls = ml.audio.classify("birdsong.wav")!
print(cls.label, cls.score)
```

### Diarization

```v2
let diar = ml.audio.diarize("podcast.wav")!
for (seg in diar.segments) {
    print(seg.speaker, seg.start_ms, seg.end_ms)
}
```

### API Reference

| Function                                   | Description                               |
| ------------------------------------------ | ----------------------------------------- |
| `ml.audio.load(model_name_or_path, opts?)` | Load audio ML model                       |
| `ml.audio.transcribe(model, audio, opts?)` | Speech-to-text transcription              |
| `ml.audio.diarize(audio, opts?)`           | Speaker diarization                       |
| `ml.audio.classify(audio, opts?)`          | Audio event classification                |
| `ml.audio.embed(audio, opts?)`             | Audio embeddings for retrieval/similarity |
