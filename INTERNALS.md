# V2 Internals Documentation

A compiler/runtime implementation reference for V2. This document focuses on execution behavior, compiler lowering, runtime semantics, diagnostics, safety, and conformance.

This is implementation-only documentation. User-facing syntax examples belong in DOCS.md, while INTERNALS.md defines how features and APIs must work.

> **Specification vs. current implementation.** This document is largely a *specification of how
> features must work*, and in places describes machinery (multi-backend codegen, HIR/MIR, native/
> WASM backends, parallel async workers, native sanitizers, hardware/network stdlib) that is
> **designed but not fully built** in the reference implementation. The shipping engine is a
> tree-walking interpreter (with a secondary bytecode VM) that fully implements the core language
> and the pure-computation standard library. For the authoritative, up-to-date list of what is
> implemented, partial, or stub, see `NOT_YET_IMPLEMENTED.md`; for how optional/native modules are
> delivered, see `PACKAGES.md`. Sections below that describe unbuilt backends or native subsystems
> should be read as target design, not a claim about the current binary.

---

## Internals-Only Chapters

### 1) Compiler Pipeline

- Stages: lexing -> parsing -> AST -> name resolution -> typing/effects -> lowering -> optimization -> codegen.
- Errors: recover where possible and emit stable diagnostic categories with source spans and actionable notes.
- Artifacts: preserve symbol IDs and source mapping continuity across all lowering stages.

### 2) IR and Codegen

- HIR carries normalized semantics and explicit bindings; MIR carries control flow and optimization-friendly form.
- Backends: bytecode VM, native backend, and WASM backend with capability-aware lowering.
- Parity: behavior-equivalence tests are required across backends where feature parity is expected.

### 3) Runtime Architecture

- Subsystems: scheduler, allocator/GC, module loader, effect runtime, FFI bridge, engine bridge, diagnostics bridge.
- Concurrency: async event loop + worker pools + thread APIs + isolate boundaries.
- Safety: safe-by-default with explicit unsafe boundaries and optional sanitizer instrumentation.

### 4) Security and Determinism

- Capability gates control privileged host access in sandboxed and WASM contexts.
- Boundary hygiene is required for all FFI and engine bridges (validation, normalization, wrapping).
- Determinism profile defines reproducible behavior, deterministic test seeds, and stable diagnostics.

### 5) Conformance Strategy

- Every feature/API requires parser, semantic, runtime, negative, and regression coverage.
- Performance-sensitive APIs require benchmark baselines and regression thresholds.
- Security-critical APIs require adversarial tests and misuse-path diagnostics validation.

## Chapter Status Matrix

Status key:

- `Draft`: implementation contract exists, but concrete function-level internals records are minimal or missing.
- `Partial`: core internals path records are documented; exhaustive API-by-API records are still in progress.
- `Complete`: chapter-level internals and function behavior records are comprehensive and cross-referenced.

| Chapter                                       | Status      | Notes                                                                                                                                                        |
| --------------------------------------------- | ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| CLI Usage                                     | Complete    | Full command execution paths, diagnostics, and failure matrix documented.                                                                                    |
| WASM Target                                   | Complete    | Lowering, memory/import contracts, runtime bridge, and failure matrix documented.                                                                            |
| Step Debugger                                 | Complete    | Breakpoint/step/eval runtime contracts and failure matrix documented.                                                                                        |
| Data Types                                    | Complete    | Canonicalization, coercion, layout, and type failure matrix documented.                                                                                      |
| Operators                                     | Complete    | Parse/resolve/lower semantics and operator failure matrix documented.                                                                                        |
| Control Flow                                  | Complete    | CFG lowering paths and control-flow failure matrix documented.                                                                                               |
| Structs                                       | Complete    | Layout/init/update internals and failure matrix documented.                                                                                                  |
| Enums                                         | Complete    | Tag/payload/match internals and failure matrix documented.                                                                                                   |
| Error Handling                                | Complete    | Propagation/unwind/effects coupling and failure matrix documented.                                                                                           |
| Effects System                                | Complete    | Inference/check/lowering coupling and failure matrix documented.                                                                                             |
| Variable Scoping Rules                        | Complete    | Scope stack, capture analysis, and scope failure matrix documented.                                                                                          |
| Strings                                       | Complete    | UTF-8 storage, slicing/concat behavior, and failure matrix documented.                                                                                       |
| Lists                                         | Complete    | Allocation/growth, mutation/iteration semantics, and failure matrix documented.                                                                              |
| Dictionaries                                  | Complete    | Hash/equality contracts, resize/iteration semantics, and failure matrix documented.                                                                          |
| Tuples                                        | Complete    | Product-type layout/access/destructure behavior and failure matrix documented.                                                                               |
| Sets                                          | Complete    | Uniqueness/hash contracts, set operations, and failure matrix documented.                                                                                    |
| Variables & Constants                         | Complete    | Binding init/mutability/const-eval semantics and failure matrix documented.                                                                                  |
| Functions                                     | Complete    | Define/call/return ABI behavior and function failure matrix documented.                                                                                      |
| Defer                                         | Complete    | Cleanup stack registration/ordering and defer failure matrix documented.                                                                                     |
| Lambdas & Closures                            | Complete    | Capture/lowering/invocation behavior and closure failure matrix documented.                                                                                  |
| Decorators                                    | Complete    | Expansion ordering, transform contracts, and failure matrix documented.                                                                                      |
| Lazy Expressions                              | Complete    | Thunk force/memoization behavior and failure matrix documented.                                                                                              |
| Classes                                       | Complete    | Layout/constructor/dispatch/destructor behavior and failure matrix documented.                                                                               |
| Pattern Matching                              | Complete    | Decision-tree lowering, guards, and failure matrix documented.                                                                                               |
| Generics                                      | Complete    | Bound solving, specialization cache, and generic failure matrix documented.                                                                                  |
| `using` Keyword                               | Complete    | Alias/import opening semantics and failure matrix documented.                                                                                                |
| Traits                                        | Complete    | Trait declaration/impl/dispatch semantics and failure matrix documented.                                                                                     |
| Trait Associated Types                        | Complete    | Associated-type projection/normalization and failure matrix documented.                                                                                      |
| Const Generics                                | Complete    | Const-parameter evaluation/specialization and failure matrix documented.                                                                                     |
| Pipe and Spread                               | Complete    | Pipe lowering and spread expansion semantics and failure matrix documented.                                                                                  |
| Runtime Introspection                         | Complete    | Reflection metadata/query semantics and failure matrix documented.                                                                                           |
| Modules & Imports                             | Complete    | Graph resolution, selector binding, and import failure matrix documented.                                                                                    |
| Module Visibility (`pub(crate)`/`pub(super)`) | Complete    | Visibility lattice enforcement and failure matrix documented.                                                                                                |
| Embedded Language Engines                     | Complete    | Embedded engine bridge internals and failure matrix documented.                                                                                              |
| Custom Language Engines                       | Complete    | Engine registration/compile/execute semantics and failure matrix documented.                                                                                 |
| Inline Assembly                               | Complete    | Constraint-aware asm lowering, safety gates, and failure matrix documented.                                                                                  |
| Actors & Agents                               | Complete    | Mailbox/supervision runtime semantics and failure matrix documented.                                                                                         |
| Isolates                                      | Complete    | Isolate lifecycle and cross-isolate transfer failure matrix documented.                                                                                      |
| Manual Allocation                             | Complete    | Unsafe alloc API semantics and memory-safety failure matrix documented.                                                                                      |
| Vectors & Tensors                             | Complete    | Shape/dtype/layout execution semantics and failure matrix documented.                                                                                        |
| Testing                                       | Complete    | Discovery/execution/reporting semantics and failure matrix documented.                                                                                       |
| Builtins Reference                            | Complete    | Intrinsic resolution/lowering semantics and failure matrix documented.                                                                                       |
| Method Reference                              | Complete    | Method resolution/dispatch semantics and failure matrix documented.                                                                                          |
| Operator Overloading                          | Complete    | Overload registration/selection semantics and failure matrix documented.                                                                                     |
| Keywords                                      | Complete    | Lexing/parser keyword contracts and keyword failure matrix documented.                                                                                       |
| Importing Standard Libraries                  | Complete    | Std import resolution/binding semantics and failure matrix documented.                                                                                       |
| Stdlib Module Catalog                         | Complete    | Catalog metadata/resolution semantics and failure matrix documented.                                                                                         |
| std.fs — Filesystem                           | Complete    | Filesystem API validation/I-O semantics and failure matrix documented.                                                                                       |
| std.fmt — Formatting                          | Complete    | Format-parse/render semantics and failure matrix documented.                                                                                                 |
| std.regex — Regular Expressions               | Complete    | Compile/match/replace semantics and failure matrix documented.                                                                                               |
| std.iter — Iterator Combinators               | Complete    | Iterator pipeline evaluation semantics and failure matrix documented.                                                                                        |
| std.time — Date & Time                        | Complete    | Clock/timezone/format semantics and failure matrix documented.                                                                                               |
| std.proc — Process Management                 | Complete    | Spawn/wait/pipe semantics and failure matrix documented.                                                                                                     |
| std.log — Structured Logging                  | Complete    | Structured sink/flush/filter semantics and failure matrix documented.                                                                                        |
| Memory Safety and Borrowing                   | Complete    | Borrow/lifetime checking and memory safety failure matrix documented.                                                                                        |
| Move Semantics                                | Complete    | Ownership transfer, implicit-move-on-last-use, closure capture, and failure matrix documented.                                                               |
| Compiler Diagnostics                          | Complete    | Diagnostic pipeline, span mapping, error codes, suggestion engine, and output format documented.                                                             |
| Structured Concurrency                        | Complete    | TaskGroup lifecycle, cancellation propagation, error strategies, and failure matrix documented.                                                              |
| Data Classes (`@data`)                        | Complete    | Auto-generated method synthesis, exclusion, destructuring, and failure matrix documented.                                                                    |
| Sealed Classes                                | Complete    | Subclass registration, exhaustiveness checking, and failure matrix documented.                                                                               |
| Copy-on-Write Classes (`@cow`)                | Complete    | COW refcount, mutation-triggered copy, and failure matrix documented.                                                                                        |
| Trait Composition & Supertraits               | Complete    | Supertrait resolution, diamond dedup, embedding, and failure matrix documented.                                                                              |
| Hardware Fault Signals                        | Milestone 1 | `signal.on_fault` registration, OS handler, `FaultInfo` capture, and `dump_json` implemented. Recovery, core dumps, and backtrace symbolication are planned. |
| Channels and Threads                          | Complete    | Channel/thread lifecycle semantics and concurrency failure matrix documented.                                                                                |
| Remaining mirrored chapters                   | Complete    | All mirrored chapters now include implementation records, API execution records, and failure matrices.                                                       |

---

## Mirrored Chapters from DOCS.md

Chapters below mirror DOCS.md headings, but content here is implementation behavior only.

## Table of Contents

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - TOC indexing, anchor resolution, and navigation internals and failure behavior are documented.

### Implemented Internal Records

- Heading extraction builds a stable section graph from parsed markdown AST nodes with persistent section IDs.
- Anchor normalization applies deterministic slug rules and collision suffixing to guarantee unique intra-page targets.
- Mode-partitioned indexes (docs vs internals) are materialized once and reused by render/search/navigation paths.

### API Execution Records (Complete)

- toc.build(sourceTrees): traverse heading nodes -> assign depth/order -> emit compact TOC graph artifact.
- toc.resolve(anchor, mode): normalize anchor -> lookup mode-specific index -> return section span + title metadata.
- toc.search(query, mode): tokenize query -> run prefix/fuzzy ranking over TOC index -> emit ranked section hits.
- toc.render(tree, viewport): apply collapse/expand policies -> emit paginated TOC view model.

### Failure Mode Matrix

- Duplicate normalized anchors in same scope: deterministic anchor-collision diagnostic with both source headings.
- Missing anchor target on navigation request: not-found diagnostic including nearest anchor suggestions.
- Malformed heading hierarchy (depth jump violations): structure diagnostic with offending heading span.
- TOC index build budget exceeded on huge docs: bounded indexing diagnostic with fallback linear mode.

## Getting Started

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - startup bootstrap sequencing, environment checks, and onboarding internals and failure behavior are documented.

### Implemented Internal Records

- Bootstrap flow composes template generation, environment probes, and first-run validation into an ordered pipeline.
- Toolchain detection caches probe results (compiler/runtime/backend) to avoid repeated startup cost.
- Onboarding checklist generation derives missing prerequisites from manifest + workspace capability profile.

### API Execution Records (Complete)

- bootstrap.initProject(path, template): validate destination -> scaffold baseline layout -> write starter manifest.
- bootstrap.detectToolchain(profile): probe required executables/backends -> normalize capability report.
- bootstrap.firstRunCheck(workspace): run prerequisite checks -> aggregate actionable setup tasks.
- bootstrap.generateChecklist(report): emit ordered remediation checklist with command hints.

### Failure Mode Matrix

- Invalid project destination (non-empty/conflicting files): bootstrap path diagnostic with safe-merge guidance.
- Required toolchain component missing: setup diagnostic with version requirement and install hint.
- Template write failure due permissions: filesystem/permission diagnostic with failed path context.
- Incompatible profile/backend combo for first run: capability diagnostic with supported combinations.

## Docs Modes

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - docs mode routing, rendering, and mode-specific internals and failure behavior are documented.

### Implemented Internal Records

- Mode router maps CLI/UI requests to docs, internals, and parity validation execution paths.
- Mode-specific renderers share a common markdown/heading cache while applying distinct visibility filters.
- Search indexes are keyed by mode to prevent cross-surface leakage of internal-only content.

### API Execution Records (Complete)

- docs.mode.select(args): parse mode flags -> resolve active mode and fallback policy.
- docs.mode.loadIndex(mode): load or build mode-partitioned heading/search index.
- docs.mode.renderPage(mode, route): fetch page AST -> apply mode filters -> render output surface.
- docs.mode.search(query, mode): execute mode-scoped search over cached index -> return ranked snippets.

### Failure Mode Matrix

- Unknown mode selector: mode-resolution diagnostic with allowed mode list.
- Route not available in selected mode: visibility diagnostic with suggested alternate mode.
- Stale mode index artifact: cache-invalidation diagnostic and automatic rebuild path.
- Search query budget exceeded in constrained mode: bounded-search diagnostic with narrowed-query hint.

## Project Manifest

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - manifest parsing, profile resolution, and manifest internals and failure behavior are documented.

### Implemented Internal Records

- Manifest loader parses schema-versioned configuration into canonical runtime/compiler option models.
- Profile inheritance and CLI overrides merge deterministically with explicit precedence metadata.
- Compiler target resolution keeps mode selection (`native`/`wasm`/`bytecode`) separate from platform triples (`os`/`arch`), with mobile modeled as `target=native` plus `os=android|ios`.
- Library profile normalization enforces publish metadata semantics (`entry=src/lib.vt`, package identity fields, optional discovery metadata).
- Path fields are normalized relative to workspace root with sandbox escape checks before use.

### API Execution Records (Complete)

- manifest.load(path): read + parse manifest bytes -> decode schema version -> emit typed manifest model.
- manifest.validateSchema(model): enforce required fields/types/ranges and deprecation policies.
- manifest.validatePublishMetadata(model): validate package identity/discovery metadata and library entrypoint constraints.
- manifest.mergeProfiles(base, profile, cli): apply layered override precedence -> emit effective configuration.
- manifest.resolvePaths(model, root): normalize relative paths -> validate workspace containment and existence policy.
- manifest.fingerprint(model): hash effective config for cache keys and incremental invalidation.

### Failure Mode Matrix

- Manifest syntax/schema mismatch: parse/schema diagnostic with exact key path and expected type.
- Circular profile inheritance: manifest-merge diagnostic with cycle chain rendering.
- Invalid package identity/metadata for publish (name/scope/version/entry): publish-metadata diagnostic.
- Path escape outside workspace root: security diagnostic with rejected normalized path.
- Unsupported manifest schema version: compatibility diagnostic with supported version range.

## CLI Usage

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - command routing, build/run/test orchestration, and CLI internals and failure behavior are documented.

### Implemented Internal Records

- CLI argument parser resolves command/flag topology and rejects ambiguous combinations before filesystem I/O.
- Command dispatcher compiles immutable execution plans (run/build/check/test/repl/docs/internals modes).
- Init templates support both app and lib scaffolds with deterministic starter layouts and manifest defaults.
- Publish pipeline has explicit preflight stage shared by `vt publish` and `vt publish --dry-run`.
- Exit normalization maps internal error categories to stable process exit codes for tooling integration.

### API Execution Records (Complete)

- vt.init(path, template): scaffold app/lib project layout and emit starter manifest + source tree.
- vt.run(file, opts): parse args -> resolve manifest/profile -> compile pipeline -> backend launch -> exit map.
- vt.build(opts): resolve target/profile -> incremental cache keying -> artifact emission + metadata write.
- vt.check(opts): run parse/type/effect/borrow validation only -> emit diagnostics without artifact output.
- vt.test(opts): discover tests -> isolate runtime contexts -> execute with timeout policy -> aggregate report.
- vt.publish(opts): execute publish preflight (manifest/tests/package) -> dry-run package or registry upload.
- vt.repl(opts): initialize incremental session -> evaluate cells -> persist context across submissions.
- vt.docs or vt.internals(opts): load mode-specific indexes -> render/search docs surfaces.

### Failure Mode Matrix

- Invalid command/flag combination: deterministic CLI parser diagnostic with valid combination hints.
- Missing or invalid manifest/profile: config diagnostic with failing key path and fallback behavior.
- Publish preflight failed (metadata/tests/package build): publish-preflight diagnostic with blocking checks.
- Registry authentication missing/expired for upload mode: publish-auth diagnostic.
- Backend initialization or launch failure: runtime diagnostic with backend selection context.
- Test session timeout/resource cap breach: bounded execution diagnostic with per-test attribution.

## WASM Target

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - wasm lowering, import/memory wiring, and wasm backend internals and failure behavior are documented.

### Implemented Internal Records

- MIR-to-WASM lowering emits typed opcode streams with deterministic source map anchors for debug parity.
- Import tables are resolved through capability-scoped host bindings (WASI/JS/native bridge profiles).
- Linear memory planning places static data, stack, and heap segments with relocation metadata for startup.

### API Execution Records (Complete)

- wasm.emitModule(mir, target): lower typed MIR into wasm sections (type/import/function/code/data).
- wasm.linkImports(module, hostCaps): resolve declared imports to allowed host symbols and signatures.
- wasm.initMemory(module, dataSegments): materialize linear memory layout and apply data initializers.
- wasm.start(module, argv): invoke entry trampoline and normalize exit/status mapping.

### Failure Mode Matrix

- Unsupported language feature for wasm backend: lowering diagnostic with unsupported construct witness.
- Unresolved or signature-mismatched import: link diagnostic with expected vs provided function type.
- Linear memory/data segment overflow: layout diagnostic with segment bounds and required memory growth.
- Host capability denied for imported operation: capability diagnostic with blocked import symbol details.

## Step Debugger

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - breakpoint/step/eval internals and debugger failure behavior are documented.

### Implemented Internal Records

- Breakpoint registry indexes source spans to IR offsets and runtime instruction pointers.
- Step controls (into/over/out) are implemented by temporary trap instrumentation and frame-depth guards.
- In-frame evaluation executes in a sandboxed expression VM with side-effect policy enforcement.

### API Execution Records (Complete)

- dbg.setBreakpoint(file, line, cond): resolve source location -> install breakpoint with optional condition.
- dbg.step(mode): arm stepping trap policy (into/over/out) -> resume execution until next stop event.
- dbg.inspect(frame, expr): evaluate read-only watch expression in selected frame context.
- dbg.eval(frame, expr, policy): execute interactive expression under side-effect policy constraints.
- dbg.stack(): capture normalized stack frames, locals, and source mappings.

### Failure Mode Matrix

- Breakpoint location not mappable to executable span: debug-location diagnostic with nearest valid span.
- Step operation on optimized frame without mapping: debug-step diagnostic with optimization context.
- Eval blocked by side-effect policy: debug-eval policy diagnostic with allowed evaluation modes.
- Debug session detached/stale target handle: session-state diagnostic with reconnection requirement.

## Comments & Doc Comments

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - comment lexing, doc binding, and doc-comment internals and failure behavior are documented.

### Implemented Internal Records

- Lexer strips non-doc comments before parser consumption while preserving source map offsets.
- Doc comments are parsed into markdown/doc-tag AST nodes and attached to symbol IDs during binding.
- Doc index emission stores normalized summaries, tags, and links for docs/internals render pipelines.

### API Execution Records (Complete)

- comments.strip(tokenStream): remove line/block comments -> preserve token span mapping.
- doc.parseBlocks(rawDocTokens): parse markdown + structured tags into doc AST.
- doc.bindSymbol(symbolId, docAst): attach parsed docs to symbol table entry with visibility checks.
- doc.emitIndex(module): serialize bound doc metadata for docs/internals search/render.

### Failure Mode Matrix

- Unterminated block comment: lexer diagnostic with opening delimiter span.
- Malformed doc tag syntax: doc-parser diagnostic with expected tag grammar.
- Duplicate/conflicting docs for same symbol: binding diagnostic with both doc source locations.
- Unsafe raw HTML/doc payload in restricted docs profile: sanitization diagnostic with rejected node details.

## Variables & Constants

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Variables & Constants lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Variable binding (`let`) creates a named slot in the current frame's scope dict with mutable storage; `const` bindings are resolved at compile time and prevent reassignment.
- `comptime const` declarations force evaluation during compilation and bake the result directly into bytecode as an immediate value.
- Underscore literals in numeric values (`1_000_000`) are lexically stripped during tokenization — they have no runtime representation.
- Type inference propagates from the initializer expression; explicit annotations (`let x: int = ...`) are checked against the inferred type.

### API Execution Records (Complete)

- let.bind(name, init_expr, scope): evaluate init_expr -> allocate slot in scope frame -> bind name to value -> slot is mutable.
- const.bind(name, init_expr, scope): evaluate init_expr at declaration time -> allocate immutable slot -> reject any subsequent assignment at compile time.
- comptime_const.bind(name, expr): force compile-time evaluation of expr -> bake result as immediate in bytecode -> no runtime allocation.
- type(val): query runtime type tag -> return human-readable string ("int", "str", "list", etc.).
- typeof(val): query raw runtime type tag -> return internal sized type descriptor.
- variable.access(name, scope_stack): search scope stack from innermost to outermost -> return value or raise NameError.
- variable.assign(name, value, scope_stack): search scope stack for existing binding -> update value in found slot -> raise NameError if not found.

### Failure Mode Matrix

- Re-declaring variable with `let` in same scope: compile error "name already declared in this scope".
- Attempting reassignment of `const` binding: compile error "cannot reassign const".
- Variable used before declaration: runtime NameError "x is not defined".
- `comptime const` with expression not resolvable at compile time: compile error "expression cannot be evaluated at compile time".
- Shadowing outer variable without `@suppress shadow`: compiler warning with inner/outer binding locations.

## Variable Scoping Rules

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Variable Scoping Rules lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Scope is a stack of frame dictionaries — new scope created on `{`, destroyed on `}`. Each scope level holds its own binding set.
- Loop variables in `for (item in list)` are implicitly `let`-bound with a fresh slot per iteration — mutations do not carry across iterations.
- Closures capture variables by reference (live binding) — the capturing function holds a pointer to the original scope slot.
- `@suppress shadow` inline directive disables the shadowing warning for a single redeclaration.

### API Execution Records (Complete)

- scope.enter(parent_scope): allocate new lexical frame -> push onto scope stack -> new bindings isolated from parent.
- scope.exit(frame): destroy frame -> pop from scope stack -> all bindings in frame become unreachable.
- for_in.bind(var_name, iterable, body): create iteration scope -> bind var_name to each element -> re-bind fresh slot per iteration.
- for_loop.bind(init, cond, update, body): create loop scope -> bind init variable -> evaluate cond/update/body in loop scope.
- closure.capture(var_name, outer_scope): record reference to var_name slot in outer scope -> mutations in closure affect outer binding.
- closure.freeze(var_name, value): bind var_name as default parameter with current value -> capture by value at definition time.
- shadow.check(name, outer_scope, inner_scope): detect inner `let` that redefines outer binding -> emit shadow warning unless suppressed.

### Failure Mode Matrix

- Accessing variable after block exits: NameError "x is not defined".
- Loop variable captured by closure without freeze: all closures see final loop value (classic closure gotcha).
- Mutable borrow conflict when loop variable is captured by closure and reassigned in loop body.
- Using loop variable after loop in @borrow_check context: compile error — binding is out of scope.
- Shadowing without @suppress: compiler emits shadow warning with inner/outer binding locations.

## Data Types

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Data Types lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Primitive types (`int`, `float`, `str`, `bool`) are unboxed and stack-allocated; `int` is arbitrary-precision by default (no overflow unless `--overflow` mode configured).
- `null` is a singleton value with special equality semantics — `null == null` is true, but `null` fails `is` type checks for other types.
- Collection types (`list`, `dict`, `set`) use reference semantics — assignment shares the reference, not a copy.
- `any` type disables static type checking for one binding — runtime type discovery via `type()` or `is` narrowing.

### API Execution Records (Complete)

- int(val, base?): parse val as integer with optional base -> return int or raise ParseError.
- float(val): parse val as IEEE 754 double -> return float or raise ParseError.
- str(val): convert val to string representation -> call **str**() if available, else default formatting.
- bool(val): convert val to boolean using truthiness rules -> falsy set is [null, false, 0, 0.0, "", [], {}, #{}].
- type(val): query runtime type tag -> return human-readable string name.
- typeof(val): query raw type descriptor -> return sized type tag (platform-dependent).
- is_some(opt): check if Option is Some -> return true/false.
- is_none(opt): check if Option is None -> return true/false.
- val is TypeName: runtime type check -> narrow value to TypeName in subsequent scope.

### Failure Mode Matrix

- Calling string methods on `null`: runtime TypeError "Cannot call method on null".
- Indexing list out of bounds: IndexError "list index out of range".
- Using `any`-typed value in type-specific operation without narrowing: runtime TypeError at operation time.
- Mixing numeric types: implicit coercion may lose precision (int + float -> float).
- Hash collision on dict/set keys: follows open-addressing probe chain.

## Operators

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Operators lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Binary operators are left-associative by default except `**` (right-associative). Precedence follows standard mathematical convention.
- Operator overloading via `__add__`, `__mul__`, etc. methods on user types; compiler rewrites operator expressions to method calls.
- Short-circuit evaluation for `&&` and `||` — right operand not evaluated if left determines result.
- Bitwise operators (`band`, `bor`, `bxor`, `bnot`, `<<`, `>>`) operate on sized integers at bit level.

### API Execution Records (Complete)

- op.add(a, b): addition (number) or concatenation (string/list) -> result with type coercion.
- op.sub(a, b): subtraction (number only) -> numeric result.
- op.mul(a, b): multiplication (number) or repetition (string _ int, list _ int) -> result.
- op.div(a, b): float division (always returns float) -> raise ZeroDivisionError if b is 0.
- op.floordiv(a, b): floor division (rounds toward negative infinity) -> int result.
- op.mod(a, b): modulo (same sign as divisor) -> numeric result.
- op.pow(a, b): exponentiation (right-associative) -> result type depends on operands.
- op.eq(a, b): deep equality comparison -> bool (collections compared recursively).
- op.lt/gt/le/ge(a, b): comparison operators -> bool.
- op.and(a, b): logical AND (short-circuit) -> return left if falsy, else right.
- op.or(a, b): logical OR (short-circuit) -> return left if truthy, else right.
- op.not(a): logical NOT -> invert truthiness.
- op.band/bor/bxor(a, b): bitwise operations on integers.
- op.shl/shr(a, b): bit shift operations -> integer result.
- op.in(a, b): membership test -> substring in string, item in list/set, key in dict.
- op.ternary(cond, t, f): evaluate cond -> return t if truthy, else f.
- op.nullcoal(a, b): null coalescing -> return b if a is null.
- op.optchain(a, prop): optional chaining -> access prop only if a is not null, else null.

### Failure Mode Matrix

- Division by zero: runtime panic (OverflowError or ValueError depending on --overflow mode).
- Comparing incompatible types (e.g., `5 > "hello"`): runtime TypeError.
- Using bitwise operators on non-integer types: compile error or runtime TypeError.
- Integer overflow: wrap mode silently wraps, saturate mode clamps, panic mode (default) panics.
- Operator on user type without **add**/**lt**/etc.: "operator not supported" TypeError.

## Strings

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Strings lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Strings are immutable UTF-8; indexing and slicing operate on Unicode code points, not UTF-8 byte offsets.
- String escape sequences processed at parse time — `\n`, `\u0041`, etc. become single characters in the literal.
- Raw strings (`r"..."`) disable all escape processing — backslashes remain literal characters.
- F-strings parse `${expr}` interpolation fragments at compile time and emit concatenation bytecode.

### API Execution Records (Complete)

- str.literal(tokens): parse string with escape processing -> emit string constant.
- str.fstring(parts, exprs): evaluate each expr -> convert to string via **str**() -> concatenate with static parts.
- str.len(s): return code point count (not byte count).
- str.upper(s) / str.lower(s): case conversion -> return new string.
- str.trim(s) / str.trim_start(s) / str.trim_end(s): strip whitespace -> return new string.
- str.split(s, sep, max?): split by separator -> return list of parts.
- str.replace(s, old, new): replace all occurrences -> return new string.
- str.contains(s, sub) / str.starts_with(s, pre) / str.ends_with(s, suf): substring checks -> bool.
- str.index(s, i): return code point at position i -> raise IndexError if out of bounds.
- str.slice(s, start, end, step): return substring with optional step.
- str.repeat(s, n): repeat string n times -> return new string.
- str.to_bytes(s): return UTF-8 byte list.
- str_from_bytes(bytes): construct string from byte list -> raise ValueError if invalid UTF-8.
- str.graphemes(s): return list of grapheme clusters.
- str.byte_len(s): return byte length of UTF-8 encoding.
- tag_fn f"template": pass static parts and interpolated values to tag function -> return tag function result.

### Failure Mode Matrix

- Index out of bounds: IndexError "string index out of range".
- String mutation attempt (s[0] = "x"): TypeError "strings are immutable".
- Invalid UTF-8 in str_from_bytes(): ValueError "invalid UTF-8 sequence".
- Interpolating null or non-Printable type in f-string: runtime lookup calls **str**() or TypeError.
- Tag function returning unexpected type: type propagation from tag function determines result type.

## Lists

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Lists lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Lists are dynamic arrays with O(1) amortized append, O(n) insertion/deletion at arbitrary positions.
- List slicing returns a new list (shallow copy of references, not deep copy of elements).
- List iteration via `for-in` implicitly calls `.iter()` which returns an iterator.
- Empty list `[]` has inferred element type `any` until context requires a specific type.

### API Execution Records (Complete)

- list.literal(elements): allocate dynamic array -> store elements contiguously.
- list.push(item): append to end (amortized O(1)) -> mutates list in place.
- list.pop(i?): remove and return element at index (default: last) -> raise IndexError if empty.
- list.insert(i, val): insert before index -> shift elements right (O(n)).
- list.remove(val): remove first occurrence by equality -> no-op if not found.
- list.extend(other): append all elements from other -> mutates list in place.
- list.index(i): access element at index -> raise IndexError if out of bounds.
- list.assign(i, val): mutate element at index -> raise IndexError if out of bounds.
- list.slice(start, end, step): return new list from slice -> shallow copy.
- list.len(): return element count.
- list.contains(val): membership test using == equality -> bool.
- list.map(fn): apply fn to each element -> return new list.
- list.filter(fn): keep elements where fn returns truthy -> return new list.
- list.reduce(fn, init): fold over elements with accumulator -> return accumulated result.
- list.sort(): in-place TimSort (stable) -> return self for chaining.
- list.sort_by(fn): in-place sort by key function -> return self.
- list.reverse(): return reversed copy (non-mutating).
- list.join(sep): join elements to string with separator.
- list.flat_map(fn): map then flatten one level -> return new list.
- list.enumerate(): return list of (index, element) tuples.
- list.zip(other): pair elements with other list -> list of tuples.
- list.chunks(n): split into groups of n -> list of lists.
- list.unique(): remove duplicates preserving order -> new list.

### Failure Mode Matrix

- Indexing out of bounds: IndexError.
- Iterating list while mutating: RuntimeError "collection modified during iteration".
- Sorting list with non-comparable types: TypeError.
- Memory exhaustion on very large append: MemoryError.
- Calling methods on null list: TypeError.

## Dictionaries

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Dictionaries lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Dicts maintain insertion order for iteration. Keys must be hashable (strings, numbers, tuples).
- Collision handling via open addressing with linear probing (implementation detail).
- Missing key access via `dict[key]` returns `null` (no error); use `.has()` to check existence.
- Dict comprehension syntax `{k: v for ...}` desugars to equivalent iterator pipeline.

### API Execution Records (Complete)

- dict.literal(pairs): allocate hash table -> insert key-value pairs in order.
- dict.get(key, default?): lookup key -> return value or default (null if no default).
- dict.set(key, val) / dict[key] = val: insert or update entry -> mutates dict.
- dict.has(key): check key existence -> bool.
- dict.remove(key): delete entry -> no-op if key missing.
- dict.keys(): return list of keys in insertion order.
- dict.values(): return list of values in insertion order.
- dict.items(): return list of (key, value) tuples.
- dict.update(other): merge other dict in-place -> other's keys win on conflict.
- dict.merge(other): return new merged dict -> non-mutating.
- dict.len(): return entry count.
- dict(obj): convert struct/class to dict of field names and values.
- for (k in dict): iterate keys in insertion order.
- for ([k, v] in dict.items()): iterate pairs.

### Failure Mode Matrix

- Using mutable object as key (list, dict): TypeError "unhashable type".
- Modifying dict during iteration: RuntimeError "dictionary modified during iteration".
- Memory pressure on very large dicts: MemoryError or degraded performance.
- Non-string keys with f-string interpolation: implicit conversion via str().

## Tuples

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Tuples lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Tuples are immutable, fixed-length, and stack-allocated or compact heap-allocated.
- Tuple indexing via dot notation `.0`, `.1` is resolved at compile time (not runtime lookup).
- Empty tuple `()` is truthy (value exists, just no elements).
- Tuple unpacking in `let` and `for` is syntactic sugar for indexed access with compile-time bounds check.

### API Execution Records (Complete)

- tuple.literal(elements): allocate immutable fixed-length tuple -> elements stored contiguously.
- tuple.index(i) / tuple.dot(i): access element at compile-time-known position.
- tuple.len(): return element count.
- tuple.unpack(pattern): destructure tuple into bindings -> compile error if pattern arity mismatches.
- tuple(list): convert list to tuple -> immutable copy.
- for ((x, y) in list_of_tuples): destructure each tuple during iteration.

### Failure Mode Matrix

- Attempting assignment tuple[0] = val: TypeError "tuple is immutable".
- Out-of-bounds access tuple[10]: IndexError.
- Mismatched destructuring pattern: ValueError "not enough values to unpack".
- Using tuple as dict key with mutable elements: TypeError "unhashable".

## Sets

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Sets lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Sets use hash table internally — O(1) average add/remove/lookup.
- Empty set requires `#{}` syntax (distinct from empty dict `{}`).
- Set operations (`|`, `&`, `-`, `^`) are routed to method equivalents via operator overloading on the set type.
- Elements must be hashable; mutable types (list, dict) cannot be set elements.

### API Execution Records (Complete)

- set.literal(elements): allocate hash set -> insert unique elements.
- set(list): convert list to set, removing duplicates.
- set.add(val): insert element (no-op if already present) -> mutates set.
- set.remove(val): delete element (no-op if absent) -> mutates set.
- set.contains(val): membership test -> bool.
- set.len(): return element count.
- set.union(other) / a | b: all elements from both sets -> new set.
- set.intersect(other) / a & b: elements in both -> new set.
- set.difference(other) / a - b: elements in self but not other -> new set.
- set.sym_difference(other) / a ^ b: elements in either but not both -> new set.
- set.is_subset(other): every element of self in other -> bool.
- set.is_superset(other): every element of other in self -> bool.
- set.is_disjoint(other): no shared elements -> bool.
- set.to_list(): convert to list (unordered).
- set.clear(): remove all elements.

### Failure Mode Matrix

- Using unhashable type as element (list, dict): TypeError.
- Modifying set during iteration: RuntimeError "set changed size during iteration".
- Comparing sets with incompatible element types: may fail at hash time.
- Empty set literal `#{}` vs empty dict `{}`: parser distinguishes by `#` prefix.

## Comprehensions

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - comprehension parsing, desugaring, type inference, and lowering internals documented.

### Implemented Internal Records

- Parser recognizes `[expr for ident in iterable if cond]` as a ComprehensionExpr AST node with kind (list/dict/set), body expression, iterator bindings, and optional filter.
- Desugaring pass converts ComprehensionExpr into equivalent iterator pipeline: `iterable.iter().filter(cond).map(body).collect()` — preserving source spans for diagnostics.
- Type inference propagates element type from the iterable through the body expression; the result type is `list<T>`, `dict<K,V>`, or `set<T>` depending on comprehension kind.
- Nested comprehensions (`for x in ... for y in ...`) desugar to `flat_map` chains with inner `filter`/`map` stages.

### API Execution Records (Complete)

- comprehension.parse(tokens): recognize `[` expr `for` pattern `in` expr (`if` expr)? (`for` pattern `in` expr)\* `]` -> emit ComprehensionExpr node with source spans.
- comprehension.desugar(ast): walk ComprehensionExpr -> emit equivalent iter().filter().map().collect() pipeline AST -> attach original spans for error mapping.
- comprehension.typecheck(desugared, env): infer element type from iterable -> propagate through filter/map -> unify result type -> emit typed IR node.
- comprehension.lower(typed_ir): emit MIR loop with pre-allocated result buffer (size hint from iterable length if known) -> append/insert operations for each yielded element.
- comprehension.async_desugar(ast): for `for await` comprehensions -> emit async iterator pipeline with `await` at each yield point.

### Failure Mode Matrix

- Non-iterable expression after `in`: type diagnostic with "expected Iterable, found T" and suggestion to implement Iterator trait.
- Filter expression not boolean: type diagnostic with "filter condition must be bool, found T".
- Dict comprehension body not a key:value pair: parse diagnostic with expected syntax hint.
- Nested comprehension variable shadowing: warning diagnostic with inner/outer binding locations.
- Async comprehension in non-async context: effect diagnostic requiring async function boundary.

## Control Flow

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Control Flow lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- if/elif/else blocks are compiled to conditional jump instructions with branch prediction hints.
- Loop variables in `for-in` are lexically scoped; each iteration re-binds the variable in a fresh slot.
- `break` and `continue` compile to jump targets identified by label or closest enclosing loop.
- `match` on enum performs exhaustiveness checking at compile time; non-exhaustive match emits a warning.

### API Execution Records (Complete)

- if.exec(cond, then_block, else_block?): evaluate cond -> execute then_block if truthy, else_block otherwise.
- elif.chain(conds, blocks, else_block?): evaluate conditions in order -> execute first matching block.
- while.exec(cond, body): evaluate cond -> execute body -> repeat until cond is falsy.
- for_classic.exec(init, cond, update, body): execute init -> loop (cond -> body -> update) until cond is falsy.
- for_in.exec(var, iterable, body): call iterable.iter() -> bind var to each element -> execute body.
- break.exec(label?): jump to loop exit -> unwind to labeled loop if label provided.
- continue.exec(label?): jump to loop header -> skip to next iteration of labeled loop if provided.
- match.exec(val, arms): evaluate val -> test each arm's pattern in order -> execute first matching arm's body.
- match.guard(pattern, guard_expr): test pattern first -> if pattern matches, evaluate guard -> proceed only if both pass.
- match.or_pattern(p1, p2): try p1 first -> if no match, try p2 -> proceed if either matches.
- ternary.exec(cond, true_expr, false_expr): evaluate cond -> return true_expr if truthy, false_expr otherwise.

### Failure Mode Matrix

- `break`/`continue` outside loop context: compile error.
- Non-exhaustive match without default: compiler warning (with --warn exhaustive).
- Infinite loop (no exit conditions, no break): not an error, but program hangs.
- Loop variable mutation in for-in: compile error if loop variable reassigned (binding is read-only).
- Guard expression that throws: exception propagates (guards don't catch errors).

## Functions

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Functions lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Function calls create new stack frame; arguments are evaluated left-to-right before the call.
- Return without value implicitly returns `null`.
- Default parameters are evaluated at call time (lazy), not at function definition time.
- Variadic parameters (`...args`) are collected into a list at the call site.

### API Execution Records (Complete)

- func.declare(name, params, body, return_type?, effects?): register function in module namespace.
- func.call(name, args): create new stack frame -> bind args to params -> execute body -> return result.
- func.default_param(name, default_expr): at call time, evaluate default_expr for any unprovided argument.
- func.variadic(name, fixed_params, rest_name): collect extra positional args into rest_name list.
- func.named_call(name, kwargs): match named arguments to parameters -> reorder as needed.
- func.return(value?): exit function -> return value (or null if omitted).
- func.generator(name, params, body): declare generator function (func\*) -> returns iterator on call.
- func.async(name, params, body): declare async function -> returns Promise on call.
- func.apply(fn, args_list): invoke fn with args_list spread as positional arguments.
- func.effect_annotate(name, effects): attach effect list [io, net, ...] to function metadata.
- func.rename(old_name, new_name): unbind old_name from function object → register under new_name → old_name becomes unresolved. Raises NameConflictError if new_name is already bound. Scope-local: renaming an imported function only affects the current file's binding table.
- func.check*name_unique(name, scope): before registering a new function (define or import), verify name is not already bound in scope → raise NameConflictError if it is. Applies to func declarations, selective imports (`import { f }`), and glob imports (`as *`).
- func.qualified_resolve(module, name): resolve `module.name` to the function object via the module's export table. Available for all import styles; aliased imports use the alias as the module prefix.
- func.unqualified_resolve(name): resolve bare `name` to the function object via the current scope's flat binding table. Available for `import "mod"` (default) and `import { name } from "mod"`, but NOT for `import "mod" as alias`.

### Failure Mode Matrix

- Missing required parameter at call: TypeError "missing argument".
- Too many positional arguments: TypeError "too many arguments".
- Named argument with unknown name: TypeError "unexpected keyword argument".
- Calling non-function value: TypeError "not callable".
- Return type mismatch (with explicit annotation): type error at return site.
- Recursive call without base case: stack overflow RecursionError.
- rename with already-bound new_name: NameConflictError "function `new_name` already exists in this scope".
- rename with unbound old_name: NameError "`old_name` is not defined".
- Defining or importing a function whose name is already bound: NameConflictError with diagnostic pointing to the existing definition.
- Unqualified call ambiguous (same name from two modules): NameConflictError "ambiguous — `name` exists in `mod_a` and `mod_b`".

## Defer

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Defer lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- `defer` blocks are registered on a cleanup stack in declaration order and executed in LIFO (reverse) order on function exit.
- Defer runs even on early `return`, `throw`, or normal end of function.
- Variables captured in `defer` block are evaluated at exit time (live binding), not at registration time.
- Return value mutation inside `defer` preserves the mutation for the caller — the defer body can alter what gets returned.

### API Execution Records (Complete)

- defer.register(block, cleanup_stack): push block onto cleanup stack -> will execute on function exit.
- defer.execute_all(cleanup_stack): pop and execute all blocks in LIFO order -> runs after function body completes.
- defer.with_return(block, return_val): defer block can read and mutate return value before it is returned to caller.
- defer.with_throw(block, exception): defer block runs before exception propagates -> exception continues after all defers complete.

### Failure Mode Matrix

- Exception thrown inside defer block: propagates up, replacing earlier exception if one was in flight.
- Infinite loop inside defer: function never exits (hang).
- Memory allocation in defer without corresponding free: potential leak.
- Deferred block referencing already-freed resource: operation succeeds but has no effect (safe no-op).

## Lambdas & Closures

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Lambdas & Closures lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Lambdas are anonymous functions with implicit scope capture from their definition site.
- Closures capture variables by reference (live binding) — reassignments in closure affect outer scope and vice versa.
- Arrow form `lambda(a) => expr` has single-expression body with implicit return.
- Async lambdas return Promise when called; generator lambdas (rare) return iterators.

### API Execution Records (Complete)

- lambda.arrow(params, expr): create single-expression lambda -> implicit return of expr.
- lambda.block(params, body): create multi-statement lambda -> explicit return required.
- lambda.capture(var_name, outer_scope): record reference to var_name in enclosing scope -> live binding.
- lambda.freeze(var_name, value): bind var_name as default parameter with current value -> capture by value.
- lambda.async(params, body): create async lambda -> returns Promise on invocation.
- lambda.call(args): invoke lambda -> create frame with captured + bound variables -> execute body.

### Failure Mode Matrix

- Loop-spawned lambdas all seeing final loop value: use default param freeze to capture per-iteration.
- Modifying outer variable from lambda: surprise side effects due to reference semantics.
- Non-serializable captured binding passed across async boundary: SerializationError.
- Returning lambda that captures local variable, calling after outer function exits: captured vars may be stale.

## Decorators

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Decorators lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Decorator is a higher-order function applied at declaration time — `@d func f` is equivalent to `f = d(f)`.
- Multiple decorators applied bottom-up: `@a @b func` means `a(b(func))`.
- Decorator with arguments: `@retry(3)` calls `retry(3)` which returns a decorator function.
- Built-in decorators (`@memo`, `@inline`, `@deprecated`, `@comptime`) have special compiler support.

### API Execution Records (Complete)

- decorator.apply(decorator_fn, target_fn): call decorator_fn(target_fn) -> result replaces target_fn in namespace.
- decorator.stack(decorators, target_fn): apply decorators bottom-up -> innermost first, outermost last.
- decorator.with_args(factory, args): call factory(args) -> return decorator function -> apply to target.
- @memo.apply(fn): wrap fn with cache keyed by argument tuple -> return cached result on repeat calls.
- @deprecated.apply(fn, msg): wrap fn to emit deprecation warning on first call -> proceed with original function.
- @inline.apply(fn): hint to compiler to inline fn at call sites -> actual inlining is optimizer decision.
- @comptime.apply(fn): mark fn for compile-time-only evaluation -> fn callable only in comptime context.

### Failure Mode Matrix

- Decorator returns non-function value: runtime error when decorated name is called.
- @memo with unhashable parameter types: cache key fails, TypeError.
- Circular decorator dependencies: infinite loop at declaration time.
- Decorator that throws during application: module fails to load.

## Lazy Expressions

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Lazy Expressions lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- `lazy` creates a thunk — expression wrapped in a zero-argument closure, re-evaluated on each read.
- Lazy variables are stored as closures internally; `is_lazy(val)` checks if binding holds a thunk vs. concrete value.
- Re-assigning a lazy binding replaces the thunk entirely with the new value.
- No automatic caching — each read re-evaluates (use `@memo` or manual cache for memoization).

### API Execution Records (Complete)

- lazy.create(expr): wrap expr in a thunk closure -> store thunk in binding slot.
- lazy.force(binding): check if binding is thunk -> if so, call thunk to get value -> return value.
- lazy.assign(binding, new_val): replace thunk with concrete value -> binding is no longer lazy.
- is_lazy(binding): check if binding holds a thunk -> return bool.
- lazy.field(struct, field_name, expr): attach lazy expression to struct field -> re-evaluated on each field access.

### Failure Mode Matrix

- Lazy expression with side effects fires side effect on every read (unexpected repeated I/O).
- Lazy variable referencing reassigned outer binding: sees updated value (live reference).
- Infinite lazy recursion (`let x = lazy x + 1`): stack overflow on read.
- Performance degradation if lazy expression is expensive and accessed many times without caching.

## Classes

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Classes lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Constructor dispatch: `new ClassName(args)` resolves the constructor method signature at compile time and generates heap allocation + vtable initialization bytecode.
- Method dispatch: instance method calls validate `self` binding at compile time; runtime uses vtable for polymorphic lookup.
- Inheritance chain: base class methods resolved during compilation; superclass lookup walks the inheritance tree at runtime via class metadata.
- Field layout: instance fields stored contiguously in heap-allocated object; `@fixed` decorator locks the schema for fixed-layout optimization.

### API Execution Records (Complete)

- class.construct(name, args): allocate heap object -> initialize vtable -> call constructor body -> return instance.
- class.method_dispatch(instance, method_name, args): look up method in vtable -> call with self bound to instance.
- class.super_call(args): in child constructor, dispatch to parent constructor -> parent fields initialized first.
- class.field_access(instance, field_name): resolve field offset -> read from heap object.
- class.field_assign(instance, field_name, val): resolve field offset -> write to heap object.
- class.extends(child, parent): copy parent vtable -> overlay child methods -> child instances participate in parent type checks.
- class.impl_block(class_name, methods): register methods in class namespace -> callable as instance.method().
- class.fixed_check(class_name, field_set): lock field schema at end of class definition -> reject dynamic field addition.
- class.access_modifier(member, level): enforce pub/priv/internal visibility -> compile error on violation.

### Failure Mode Matrix

- Constructor recursion without base case: stack overflow at runtime.
- Accessing undeclared field in @fixed class: compile error "field not found".
- Calling super() in a class without parent: runtime error "no superclass".
- Method override with incompatible signature: may cause runtime type errors at call site.
- Accessing private member from outside class: compile error "private member".

## Computed Properties

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - computed property parsing, accessor lowering, lazy caching, and diagnostics documented.

### Implemented Internal Records

- Parser recognizes `get name -> Type { body }` and `set name(param: Type) { body }` inside class/struct bodies as ComputedPropertyDecl AST nodes.
- Semantic analysis ensures getter has no side effects violations and setter validates the assigned value type matches the declared property type.
- Lowering converts `get` to a hidden method `__get_name(self)` and `set` to `__set_name(self, value)` — field access/assignment syntax is rewritten to call these methods.
- `lazy get` inserts a backing cache field (`__lazy_name: Option<T>`) and wraps the getter body in a check-or-compute pattern with atomic initialization for thread safety.

### API Execution Records (Complete)

- computed.parse_getter(tokens): recognize `get` ident `->` type `{` body `}` -> emit GetterDecl with return type annotation and body AST.
- computed.parse_setter(tokens): recognize `set` ident `(` param `)` `{` body `}` -> emit SetterDecl with parameter binding and body AST.
- computed.typecheck_getter(decl, class_env): verify body expression type matches declared return type -> verify no mutation of self (purity check for non-lazy getters).
- computed.typecheck_setter(decl, class_env): verify parameter type -> verify body assigns to backing field or performs valid mutations.
- computed.lower_access(field_access, class_info): if field has getter -> rewrite to **get_name(self) call; if assignment and field has setter -> rewrite to **set_name(self, value) call.
- computed.lower_lazy(getter_decl): insert \_\_lazy_cache field -> emit check-or-compute MIR with atomic CAS for thread-safe initialization -> cache result.
- computed.invalidate_lazy(setter_call): after any setter executes on the owning object -> clear all lazy caches on that instance.

### Failure Mode Matrix

- Assignment to getter-only property: compile error "property 'name' is read-only (no setter defined)" with source span.
- Read of setter-only property: compile error "property 'name' is write-only (no getter defined)" with source span.
- Getter body has side effects (mutation, I/O): warning diagnostic suggesting `lazy get` or extracting to a method.
- Lazy getter circular dependency (getter reads another lazy property that reads this one): runtime deadlock detection -> panic with cycle trace.
- Type mismatch between getter return and declared type: standard type diagnostic.

## Structs

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Structs lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Value semantics: assignment copies the struct by value; no heap reference sharing unless explicitly borrowed.
- Field layout: fields laid out in declaration order; memory layout is deterministic for FFI interop.
- Struct literals `StructName { field: value, ... }` are expressions that construct instances on the stack.
- Update syntax (`...base`): desugars to field-by-field copy where only modified fields differ from source.

### API Execution Records (Complete)

- struct.construct(name, fields): allocate struct on stack -> initialize fields in declaration order.
- struct.named_construct(name, field_map, base?): resolve field names -> apply base spread if present -> initialize.
- struct.field_access(instance, field_name): resolve field offset at compile time -> read value.
- struct.field_assign(instance, field_name, val): resolve field offset -> write value.
- struct.impl_block(struct_name, methods): register methods in struct impl namespace.
- struct.update(base, overrides): copy all fields from base -> override specified fields -> return new struct.
- struct.mem_size_of(struct_name): compute byte size from field types with alignment -> return int.
- struct.to_dict(instance): convert fields to dict of name-value pairs.

### Failure Mode Matrix

- Field type mismatch in construction: compile error.
- Accessing non-existent field: compile error "field not found".
- Assigning to field of frozen struct: runtime error "cannot mutate frozen value".
- Struct with borrowed reference cannot be moved until borrow ends: compile error in borrow-checked code.

## C Structs (`cstruct`)

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - C-ABI layout/marshalling and cstruct internals and failure behavior are documented.

### Implemented Internal Records

- C struct layout honors ABI-specific alignment/packing rules and emits explicit field offset tables.
- FFI marshalling converts between managed values and C memory representations with ownership tags.
- Pointer field policies encode mutability/nullability and lifetime expectations at boundary crossings.

### API Execution Records (Complete)

- cstruct.define(def, abi): validate field ABI compatibility -> emit C layout descriptor and offsets.
- cstruct.marshalIn(value, layout): convert managed struct value to ABI-compliant C memory block.
- cstruct.marshalOut(ptr, layout): decode C memory into managed representation with safety checks.
- cstruct.offsetOf(layout, field): query stabilized field offset for interop codegen.

### Failure Mode Matrix

- Field ABI mismatch (size/alignment/incompatible type): cstruct-abi diagnostic with field diff.
- Unsupported packing/bitfield combination for active target ABI: layout diagnostic with target details.
- Unsafe pointer/lifetime violation crossing FFI boundary: ownership diagnostic with boundary trace.
- Unaligned access during marshal path: runtime alignment diagnostic with offending field offset.

## `using` Keyword

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - `using` Keyword lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Scope projection: `using obj { }` creates a local context where all fields of obj are accessible without the `obj.` prefix.
- Read-only binding: fields brought into scope via `using` are read-only aliases to the original object's fields.
- Namespace flattening: `using module` at file scope imports all exported symbols directly into current namespace.
- Stack-based scope: `using obj { }` block is stack-managed; projected names disappear when block exits.

### API Execution Records (Complete)

- using.block(obj, body): project obj fields into block scope -> field names accessible without prefix.
- using.module(module_name): flat import all exported symbols -> available without module prefix.
- using.alias(module, "\_"): wildcard alias disables prefix -> all symbols imported directly.
- using.field_access(projected_name): resolve to original obj.field -> read-only access.
- using.scope_exit(block): remove projected names from scope -> no longer accessible.

### Failure Mode Matrix

- Name collision: two modules imported with `using` both export same name -> compile error.
- Name shadowing in `using` block: outer scope names hidden -> compiler warns about shadowing.
- Write to using-projected field: runtime or compile error "cannot mutate using-projected field".
- Using a module not yet imported: compile error "must import before using".

## Traits

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Traits lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Trait object vtable: each trait implementation on a type generates a vtable with method pointers; `dyn Trait` values carry a pointer to this vtable.
- Static dispatch (impl Trait): trait bounds resolved at compile time; code generated per concrete type without runtime indirection.
- Dynamic dispatch (dyn Trait): trait bounds resolved at runtime via vtable indirection + heap allocation for trait objects.
- Coherence checking: no two implementations of the same trait for the same type are allowed; the orphan rule prevents ambiguous impls.

### API Execution Records (Complete)

- trait.declare(name, methods, assoc_types?): register trait definition with method signatures and optional associated types.
- trait.impl(trait_name, type_name, methods): register trait implementation -> method pointers stored in vtable.
- trait.static_dispatch(val, method_name, args): resolve concrete type at compile time -> generate direct call.
- trait.dynamic_dispatch(trait_obj, method_name, args): vtable lookup -> indirect call through method pointer.
- trait.object_box(val, trait_name): box value with trait object descriptor -> heap-allocated dyn Trait.
- trait.blanket_impl(trait_name, bound, methods): register impl for all T satisfying bound.
- trait.coherence_check(trait_name, type_name): verify no conflicting impls exist across crate graph.
- val is TraitName: runtime type check -> determine if val's type implements trait.

### Failure Mode Matrix

- Calling trait method not implemented by concrete type: compile error (static) or runtime "method not found" (dyn).
- Conflicting blanket implementations: compile error "overlapping implementations".
- Trait with generic method used as dyn: compile error "not object-safe".
- Orphan rule violation: compile error "cannot implement foreign trait for foreign type".

## Trait Associated Types

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Trait Associated Types lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Each trait defines named associated types as unresolved placeholders; each implementation must concretely resolve them.
- Associated type resolution occurs during monomorphization; code generation happens per concrete type.
- `T::AssocType` syntax allows code to reference the associated type in generic bounds and return types.
- Default associated types may be provided in the trait; implementations can override or accept the default.

### API Execution Records (Complete)

- assoc.declare(trait_name, type_name, default?): register abstract associated type in trait definition.
- assoc.resolve(trait_name, impl_type): look up concrete associated type from impl -> substitute in signatures.
- assoc.project(T, assoc_name): resolve T::AssocName to concrete type via impl -> monomorphize.
- assoc.bound_check(T, assoc_name, expected_bound): verify resolved associated type satisfies expected bound.
- assoc.default_fallback(trait_name, assoc_name): if impl omits associated type -> use trait default.

### Failure Mode Matrix

- Using T::AssocType where T has no matching impl: compile error "no associated type found".
- Mismatched associated type bounds in where clause: compile error if concrete impl violates bound.
- Multiple impls with different associated type values for same (trait, type): compile error.
- Ambiguous associated type without qualifying trait: compile error "ambiguous associated type".

## Const Generics

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Const Generics lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Each unique const parameter value generates distinct code during compilation; no runtime overhead.
- Const parameters must be resolvable at compile time (literals, const expressions, comptime results).
- Monomorphization per const: `func<const N: int>` produces a different compiled version for each unique N.
- `static_assert` can validate const parameters, failing compilation if the assertion is false.

### API Execution Records (Complete)

- const_generic.declare(struct_name, const_param, type): register struct/func with const type parameter.
- const_generic.instantiate(name, const_value): specialize code for concrete const value -> generate distinct type/function.
- const_generic.assert(const_expr, msg): evaluate const_expr at compile time -> fail compilation if false.
- const_generic.sizeof(type_with_const): compute type size with const parameter substituted -> compile-time constant.
- const_generic.compatible_check(a, b): verify two const-generic types have same const value -> type error if different.

### Failure Mode Matrix

- Non-compile-time-resolvable const value: compile error "const parameter must be a compile-time constant".
- Const parameter in trait object (`dyn`): compile error "const parameters cannot be part of dynamic dispatch".
- Integer overflow in const expression: compile error at instantiation site.
- Large const value generating massive code: may exceed compilation budget.

## Pipe and Spread

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Pipe and Spread lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Pipe desugaring: `a |> f(_, b)` desugars to `f(a, b)` at compile time; `_` marks the position of the piped value.
- Left-to-right evaluation: pipe operations chain sequentially; result of each stage becomes input to the next.
- Spread unpacking (`...list`) in collection literals unpacks elements into parent collection during construction.
- Rest collection (`...rest`) in destructuring gathers remaining elements into a list.

### API Execution Records (Complete)

- pipe.desugar(left, fn_call, placeholder): replace placeholder with left -> emit rewritten function call.
- pipe.chain(stages): evaluate left-to-right -> result of each stage becomes input to next.
- spread.list(target_list, source_lists): unpack each source -> concatenate into target.
- spread.dict(target_dict, source_dicts): merge each source -> later keys overwrite earlier.
- spread.call(fn, args_list): unpack args_list as separate positional arguments.
- rest.pattern(prefix, rest_name, list): bind prefix elements by position -> collect remainder into rest_name.

### Failure Mode Matrix

- Pipe placeholder `_` in wrong position or multiple times: parse error or type error.
- Spread on non-iterable value: TypeError "cannot spread non-iterable".
- Multiple rest patterns in one destructuring: compile error "only one rest pattern allowed".
- Spread in function call with wrong argument count after expansion: TypeError.

## Runtime Introspection

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Runtime Introspection lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Every value carries a runtime type tag; `type(val)` and `typeof(val)` query this tag.
- Compiler embeds reflection metadata for classes, structs, enums into the binary.
- `eval()` and `exec()` parse and execute V2 code at runtime; code generation happens on the fly.
- `isolate_exec()` runs code in a separate interpreter instance with isolated scope.

### API Execution Records (Complete)

- introspect.type(val): query runtime type tag -> return human-readable string.
- introspect.dir(obj): enumerate attributes and methods -> return list of names.
- introspect.hasattr(obj, name): check attribute existence -> bool.
- introspect.getattr(obj, name): retrieve attribute value -> value or error if not found.
- introspect.setattr(obj, name, val): set attribute value -> obj mutated.
- introspect.callable(val): check if val is a function/method -> bool.
- introspect.vars(): get current scope as dict -> dict of all local bindings.
- eval(code_str): parse expression string -> execute -> return result.
- exec(code_str): parse statement string -> execute -> no return value.
- isolate_exec(code_str, opts): execute in sandbox -> return Result<value, error>.

### Failure Mode Matrix

- Reflecting on non-object primitive with getattr: TypeError "primitive has no attributes".
- eval() with invalid V2 syntax: parse error.
- Sandbox escape in isolate_exec: prevented by isolation; accessing host globals throws "not in scope".
- eval() with side effects modifying global state: mutations persist in calling scope.

## Enums

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Enums lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Each enum variant is a distinct runtime tag; `match` on enum uses tag dispatch to select branch.
- Enum variants may carry associated data; constructor desugars to variant tag + data payload.
- Exhaustiveness checking at compile time verifies all variants are handled or a default exists.
- Generic enums are monomorphized per concrete type parameter.

### API Execution Records (Complete)

- enum.declare(name, variants): register enum type with variant tags in type registry.
- enum.construct(variant_name, data?): create tagged value with optional data payload.
- enum.match_dispatch(val, arms): read variant tag -> dispatch to matching arm.
- enum.destructure(variant, pattern): extract data payload from variant -> bind to pattern variables.
- enum.impl_block(enum_name, methods): register methods that can match on self's variant.
- enum.generic_instantiate(enum_name, type_params): monomorphize enum for concrete type parameters.
- enum.exhaustiveness_check(match_arms, all_variants): verify all variants covered -> warn if missing.

### Failure Mode Matrix

- Missing case in match without default: compile warning/error.
- Accessing variant data without destructuring: TypeError.
- Incompatible variant name in pattern: compile error.
- Generic enum with incompatible type parameter: compile error.

## Generics

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Generics lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Monomorphization: each unique generic instantiation generates distinct compiled code for each concrete type.
- Trait bounds constrain type parameters; compiler verifies all trait methods available at compile time.
- Generic specialization: multiple implementations can coexist; most specific impl selected.
- Generic type parameters are compile-time only; no runtime type information remains after compilation (type erasure).

### API Execution Records (Complete)

- generic.declare(name, type_params, bounds?): register generic function/struct with type parameters.
- generic.instantiate(name, concrete_types): monomorphize code for concrete type arguments.
- generic.bound_check(T, trait_bounds): verify T implements all required traits -> compile error if not.
- generic.where_clause(constraints): additional constraints on type parameters and associated types.
- generic.blanket_impl(trait, bound, methods): register impl for all T satisfying bound.
- generic.specialization_select(candidates, concrete_type): select most specific impl.

### Failure Mode Matrix

- Calling generic method on T without trait bound: compile error "method not found for type T".
- Infinite generic recursion in impl resolution: compile error.
- Specialization ambiguity (two impls equally specific): compile error.
- Generic struct with incompatible type parameter usage: compile error.

## Pattern Matching

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Pattern Matching lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Dispatch table generation: `match` compiles to a jump tree or dispatch table; simple matches become branch instructions.
- Type narrowing: pattern matching with type patterns narrows the value's type in the matched branch.
- Guard evaluation: `if` guards in pattern arms evaluated at runtime; if false, proceeds to next arm.
- Exhaustiveness verification: compiler statically checks that all variant/value paths are covered.

### API Execution Records (Complete)

- match.dispatch(val, arms): evaluate val -> test each arm's pattern in declaration order -> execute first match.
- match.literal(val, literal): exact equality comparison -> branch if equal.
- match.binding(val, var_name): capture-all pattern -> bind var_name to val.
- match.type_check(val, type_name, binding): check val is type_name -> narrow type and bind.
- match.guard(pattern, guard_expr): test pattern -> if matches, evaluate guard -> proceed only if both pass.
- match.destructure_list(val, patterns): decompose list and match element patterns.
- match.destructure_struct(val, field_patterns): extract fields and match against field patterns.
- match.or_pattern(p1, p2): try p1 -> if no match, try p2 -> proceed if either matches.
- match.exhaustiveness(arms, type_info): verify all possible values covered -> warn if gaps exist.

### Failure Mode Matrix

- Non-exhaustive match without default: compiler warning (with exhaustiveness checking enabled).
- Guard expression that throws: exception propagates (guards don't catch errors).
- Overlapping patterns: compiler warning about unreachable code.
- Pattern with undefined destructuring variable: compile error.

## Error Handling

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Error Handling lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Exception stack unwinding: `throw` propagates up the call stack; `try`/`catch` intercepts and handles.
- Result monad: `Result<T, E>` carries either success or error; `?` operator unwraps or early-returns.
- Option type: `Option<T>` carries presence or absence; `?` operator unwraps or early-returns None.
- Error conversion: `?` uses `From` trait to convert error types across function boundaries.

### API Execution Records (Complete)

- error.throw(val): unwind call stack -> search for nearest catch -> terminate if uncaught.
- error.try_catch(body, catch_clauses, finally?): execute body -> on throw, match catch clause by type -> execute finally always.
- error.result_unwrap(result): if Ok(val) return val; if Err(e) early-return Err(e) from enclosing function.
- error.option_unwrap(option): if Some(val) return val; if None early-return None from enclosing function.
- Ok(val): construct Result in success state.
- Err(e): construct Result in error state.
- Some(val): construct Option in present state.
- None: construct Option in absent state.
- try_wrap(fn): call fn() -> return Ok(result) on success, Err(error) on throw.
- result.map(fn): transform Ok value -> Ok(fn(val)) or Err passthrough.
- result.or_else(fn): recover from Err -> fn(err) returns new Result.

### Failure Mode Matrix

- Unhandled exception: program terminates with stack trace.
- `?` operator in non-Result/non-Option function: compile error.
- Catch clause with wrong error type: exception continues propagating.
- Error type conversion via From fails if impl not found: compile error at `?` site.

## `defer` and Exceptions

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - defer stack + exception unwind ordering internals and failure behavior are documented.

### Implemented Internal Records

- Defer operations are stacked per lexical frame and executed in LIFO order for all exit paths.
- Exception payloads carry normalized category, message, and trace metadata through unwind propagation.
- Catch/finally lowering merges cleanup and handler edges while preserving deterministic execution order.

### API Execution Records (Complete)

- defer.push(frame, cleanup): register cleanup closure onto frame-local defer stack.
- exception.throw(err): package error payload and begin unwind traversal across active frames.
- exception.catch(pattern, handler): match normalized exception payload and execute selected handler.
- unwind.runCleanups(frame): execute deferred cleanups/finally blocks during unwind or normal return.

### Failure Mode Matrix

- Throw across no-exception boundary/profile: exception-boundary diagnostic with required effect declaration.
- Cleanup failure while unwinding: nested-unwind diagnostic with primary and secondary fault chaining.
- Rethrow outside active catch context: control-flow diagnostic with nearest valid handler span.
- Exception payload type mismatch with catch pattern: handler-match diagnostic with payload shape details.

## Generators

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - generator state machine internals and generator failure behavior are documented.

### Implemented Internal Records

- Generator lowering transforms yield points into resumable state machine blocks with explicit program counters.
- Generator frames store locals/captures in heap-backed suspension records for resume continuity.
- Borrow/lifetime checks extend across suspension points to prevent invalid captured references.

### API Execution Records (Complete)

- gen.create(fn): lower generator body to state machine and allocate initial suspension frame.
- gen.next(handle): resume at saved state -> run until next yield/return -> emit yielded value/state.
- gen.send(handle, value): inject value into suspended generator input slot and resume execution.
- gen.close(handle): terminate generator, execute pending cleanups, and release frame resources.

### Failure Mode Matrix

- Resume/send on completed generator: generator-state diagnostic with terminal state metadata.
- Borrowed temporary captured across yield illegally: lifetime diagnostic at yield boundary.
- Panic/fault during generator resume: runtime generator-fault diagnostic with state index context.
- Close requested while generator actively executing: concurrent-state diagnostic with owner task info.

## Async / Await

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - async future state machines, wake/cancel flow, and async internals and failure behavior are documented.

### Implemented Internal Records

- Async functions lower to poll-based state machines implementing future contracts.
- Scheduler wake queues track waker tokens with deduplicated wake semantics.
- Cancellation is cooperative and inserts cancellation checkpoints at await/yield-safe boundaries.

### API Execution Records (Complete)

- async.spawn(task, opts): register future with scheduler and return task handle.
- await.poll(future, cx): poll future state machine and return pending/ready outcome.
- async.join(handles): await completion of multiple futures with deterministic result ordering.
- async.cancel(handle, mode): request cooperative cancellation and run cleanup continuation path.

### Failure Mode Matrix

- Await on non-future/non-awaitable value: type/effect diagnostic with required awaitable contract.
- Double wake/race on completed future: scheduler-state diagnostic with task lifecycle details.
- Cancellation denied in non-cancellable critical region: cancellation-policy diagnostic.
- Task handle leak or orphaned future in strict mode: async-resource diagnostic with spawn origin.

## Macros

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - macro parsing/expansion/hygiene and macro internals and failure behavior are documented.

### Implemented Internal Records

- Macro parser builds token-tree IR preserving delimiters, spans, and repetition operators.
- Expansion pipeline applies deterministic expansion order with hygiene context propagation.
- Expansion cache keys macro input + environment to support incremental rebuild stability.

### API Execution Records (Complete)

- macro.parse(invocation): parse invocation token tree and validate macro signature/shape.
- macro.expand(tree, ctx): execute expansion rules -> produce transformed syntax tree output.
- macro.resolveHygiene(ast, ctx): apply hygiene renaming and scope marks to generated identifiers.
- macro.emit(ast): hand expanded AST to normal parser/semantic pipeline with span remapping.

### Failure Mode Matrix

- Recursive macro expansion exceeds limit: expansion-depth diagnostic with expansion chain trace.
- Hygiene collision causing unresolved/generated identifier conflict: hygiene diagnostic with symbol marks.
- Macro symbol unresolved or inaccessible in scope: macro-resolution diagnostic with import hint.
- Non-deterministic expansion in strict mode: determinism diagnostic with cache-key mismatch evidence.

## Compile-Time Execution

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - constexpr evaluation pipeline and compile-time execution internals and failure behavior are documented.

### Implemented Internal Records

- Const-eval engine executes side-effect-restricted IR in a deterministic sandbox with capability gates.
- Folded results are cached by expression hash + environment fingerprint for incremental rebuild reuse.
- Dependency edges from constexpr artifacts to source symbols drive invalidation on upstream edits.

### API Execution Records (Complete)

- cte.eval(expr, env): execute constexpr-safe expression graph in compile-time sandbox.
- cte.foldConstants(ir): replace foldable IR subgraphs with evaluated literal nodes.
- cte.materialize(value, targetType): cast/normalize constexpr output for downstream lowering.
- cte.invalidate(changedSymbols): invalidate affected constexpr cache entries for recompilation.

### Failure Mode Matrix

- Non-deterministic/impure operation in constexpr context: compile-time purity diagnostic.
- Capability-restricted call attempted during constexpr evaluation: capability diagnostic with blocked intrinsic.
- Compile-time recursion/step limit exceeded: constexpr budget diagnostic with evaluation trace.
- Runtime-only value required in constexpr path: context diagnostic with required runtime fallback.

## Integer Overflow

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - overflow mode selection, lowering, and integer-overflow internals and failure behavior are documented.

### Implemented Internal Records

- Arithmetic lowering selects checked/wrapping/saturating semantics from profile and local annotations.
- Range analysis precomputes provable non-overflow spans to remove redundant runtime checks.
- Trap-on-overflow mode emits explicit trap edges with source-span-aware diagnostics hooks.

### API Execution Records (Complete)

- overflow.inferRange(expr): infer operand/result ranges for static overflow elimination.
- overflow.lowerAddSubMul(op, mode): lower arithmetic op with profile-selected overflow semantics.
- overflow.insertGuards(expr): inject runtime guards where static proof is unavailable.
- overflow.report(site, lhs, rhs): emit overflow diagnostic payload for trap paths.

### Failure Mode Matrix

- Checked arithmetic overflow at runtime: trap diagnostic with operand values and source span.
- Narrowing cast overflow without explicit policy: cast-overflow diagnostic with target width details.
- Conflicting overflow modes between profile and local annotation: mode-coherence diagnostic.
- Backend missing saturating intrinsic for target: lowering diagnostic with fallback strategy note.

## Tail-Call Optimization (TCO)

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - tail-position analysis, call rewriting, and TCO internals and failure behavior are documented.

### Implemented Internal Records

- Tail-position analysis annotates eligible call sites after control-flow and cleanup edge normalization.
- TCO transform rewrites eligible calls to frame-reuse jumps or loop trampolines by backend policy.
- Debug metadata preserves logical call-chain attribution even when physical frames are reused.

### API Execution Records (Complete)

- tco.markTailSites(cfg): detect syntactic + semantic tail positions in normalized CFG.
- tco.rewriteCall(site): replace terminal call+return with frame-reuse jump sequence.
- tco.emitLoopTrampoline(fn): generate trampoline form for self or mutual recursion sets.
- tco.verifySemantics(before, after): assert return/effect equivalence after optimization pass.

### Failure Mode Matrix

- Candidate call rejected due pending cleanup/defer edge: non-tail diagnostic with blocking edge info.
- ABI/varargs mismatch preventing frame reuse: tco-eligibility diagnostic with ABI constraints.
- Debug stack reconstruction failure in strict debug profile: debug-mapping diagnostic.
- Mutual-recursion set exceeds trampoline limit: optimization-budget diagnostic with SCC details.

## Warnings System

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - warning rule evaluation, filtering, and warning-system internals and failure behavior are documented.

### Implemented Internal Records

- Warning registry stores rule metadata, default severities, and language-edition applicability.
- Lint pass pipeline runs rule evaluators over AST/HIR/MIR phases with deduplicated spans.
- Severity and suppression configuration is resolved by module, scope, and command-line override precedence.

### API Execution Records (Complete)

- warn.registerRule(rule): register warning rule descriptor and phase binding.
- warn.analyze(unit, phase): execute enabled warning rules and collect findings.
- warn.applyConfig(findings, cfg): apply allow/warn/deny/suppress filters and effective severities.
- warn.emitReport(findings, format): output structured warning report for CLI/IDE tooling.

### Failure Mode Matrix

- Unknown warning code in config/suppression: config diagnostic with known code suggestions.
- Conflicting warning directives at same scope: precedence diagnostic with winning directive.
- Malformed inline suppression annotation: parser diagnostic with expected suppression grammar.
- Warning flood guard triggered in constrained mode: diagnostics-budget warning with truncation metadata.

## Source Directives

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - directive parsing/scoping/application and source-directive internals and failure behavior are documented.

### Implemented Internal Records

- Directives are parsed in pre-AST pass and bound to scoped directive tables with span provenance.
- Directive evaluator applies frontend/backend toggles without mutating source semantic intent.
- Conflicting directive sets are resolved via policy precedence and explicit incompatibility checks.

### API Execution Records (Complete)

- directive.parse(tokenStream): parse directive tokens into normalized directive AST nodes.
- directive.bindScope(node, scope): attach directives to lexical/module scope tables.
- directive.apply(unit, policy): apply directive effects to parser/lowerer/backend configuration.
- directive.finalize(unit): emit effective directive manifest for diagnostics/tooling parity.

### Failure Mode Matrix

- Invalid directive syntax or argument shape: directive parse diagnostic with expected grammar.
- Directive used outside allowed scope: scope diagnostic with permitted directive locations.
- Incompatible directive combination: coherence diagnostic listing conflicting directives.
- Directive disabled by active profile/capabilities: policy diagnostic with required profile hint.

## Modules & Imports

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Modules & Imports lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Each module creates a namespace; symbols are addressable via module-qualified names.
- Import resolution: `import "path"` resolves to a file or directory with `mod.vt`; symbols registered in current namespace.
- Visibility enforcement: `pub`, `private`, `internal` checked at compile time; violations are compile errors.
- Circular import detection: compiler errors or applies deduplication logic.
- Dual calling convention: `import "mod"` (no alias) registers symbols in both the qualified table (`mod.func`) and the unqualified flat table (`func`). `import "mod" as alias` registers only in the qualified table under `alias.func`. `import "mod" as _` registers only in the unqualified flat table. `import { f } from "mod"` registers `f` in both qualified (`mod.f`) and unqualified (`f`).
- Name collision at import: before inserting into the unqualified table, check for an existing binding. If a collision is detected, the import is rejected with `NameConflictError` unless the colliding name has been `rename`d away first.
- Ambiguous unqualified calls: if two modules provide the same unqualified name (both imported without alias), the unqualified name is marked ambiguous. Calls to it produce a compile error; qualified calls (`mod.name`) remain valid.

### API Execution Records (Complete)

- module.import(path): resolve path to source file -> parse -> register namespace. Register all exported symbols in both qualified (mod.name) and unqualified (name) binding tables. Reject on name collision.
- module.selective_import(path, symbols): import only specified symbols from module.
- module.alias(path, alias): register module namespace under alias name. Symbols only accessible as alias.name (no unqualified binding).
- module.export(symbol, visibility): mark symbol with visibility level for importing modules.
- module.circular_check(import_graph): detect cycles -> error or deduplicate.
- module.on_import(hook_fn): register initialization hook -> executed when module is first imported.
- module.cimport(header): parse C header -> generate extern declarations and cstruct definitions.
- module.inline_mod(name, body): create inline submodule namespace within current module.
- module.check_unqualified_collision(name, scope): before adding to flat table, verify no existing binding → raise NameConflictError.

### Failure Mode Matrix

- Importing non-existent module: compile error "module not found".
- Circular imports: compile error or infinite loop.
- Visibility violation (accessing non-pub symbol): compile error.
- on_import() throwing: module fails to load.
- Import causes name collision in unqualified scope: NameConflictError with diagnostic showing both the existing definition and the import source.
- Ambiguous unqualified call (two modules export same name): compile error "ambiguous — exists in both `mod_a` and `mod_b`; use qualified form".

## Module Visibility — `pub(crate)` and `pub(super)`

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Module Visibility — `pub(crate)` and `pub(super)` lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Four visibility levels: `pub` (everywhere), `pub(crate)` (within crate), `pub(super)` (parent module), `internal` (current module + children), `private` (current file).
- Visibility enforced at compile time during name resolution; violations produce compile errors.
- `pub(super)` applies only to the direct parent module; grandparent and higher cannot access.
- Crate boundary is the unit of compilation declared by `vt.toml`.

### API Execution Records (Complete)

- visibility.pub(symbol): accessible everywhere including other crates.
- visibility.pub_crate(symbol): accessible within same crate only.
- visibility.pub_super(symbol): accessible in direct parent module only.
- visibility.internal(symbol): accessible in current module and submodules.
- visibility.private(symbol): accessible in current file only.
- visibility.check(symbol, access_site): verify access_site has sufficient visibility -> compile error if not.

### Failure Mode Matrix

- Accessing pub(super) from sibling module: compile error.
- Accessing pub(crate) from external crate: compile error.
- pub(super) in module with no parent: compile error.
- Mismatched visibility in impl blocks: verification error.

## Embedded Language Engines

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Embedded Language Engines lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Embedded `@py { }` blocks: body extracted and sent to the Python interpreter via stdin/stdout protocol.
- Cross-language FFI: `@export` and `@import` directives establish name bindings between V2 and embedded language scopes.
- Block caching: compiled embedded code cached using hash of source; re-run reuses cache.
- Variable interop: V2 values serialized to embedded language format; results deserialized back to V2.

### API Execution Records (Complete)

- engine.py_exec(code): send Python code to interpreter -> execute in Python context -> return exported symbols.
- engine.js_exec(code): send JavaScript code to Node.js -> execute -> return exports.
- engine.export(block, symbols): extract named symbols from embedded block -> register in V2 namespace.
- engine.export_wildcard(block): export all callable symbols from embedded block.
- engine.import(symbol, source_block): import symbol from named block or all blocks of engine type.
- engine.import_from_module(symbol, py_module): import from Python module via Python import system.
- engine.interop_serialize(v2_value): convert V2 value to JSON/MsgPack -> pass to engine.
- engine.interop_deserialize(engine_value): convert engine result back to V2 value.
- register_engine(path, name): register custom engine executable for @name blocks.

### Failure Mode Matrix

- Engine executable not found: runtime error "engine not found".
- Embedded code syntax error: error reported from target language interpreter.
- Exporting non-callable symbol: error when V2 tries to call it.
- Circular import between embedded blocks and V2 code: infinite loop potential.

## Custom Language Engines

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Custom Language Engines lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Engine registration: `register_engine(path, name)` registers an executable for `@name { }` blocks.
- I/O protocol: engine receives source on stdin; writes results to stdout; V2 parses stdout.
- Name uniqueness: engine name must not conflict with builtin engines.
- Per-program scope: registration is per-program instance; not persisted globally.

### API Execution Records (Complete)

- custom_engine.register(path, name): validate executable exists -> register for @name blocks.
- custom_engine.exec(name, code): serialize code -> send to engine stdin -> read result from stdout -> deserialize.
- custom_engine.named_block(name, block_name, code): execute named block -> available for import directives.
- custom_engine.export(block, symbols): extract symbols from engine output -> register in V2 namespace.
- custom_engine.import(symbol, engine_name): import from custom engine blocks -> symbol callable in V2.
- custom_engine.re_register(name, new_path): replace existing engine registration -> last registration wins.

### Failure Mode Matrix

- Engine executable not found or not executable: permission error.
- Engine crashes or returns invalid output: parsing error in V2.
- Name collision with builtin engine: registration error.
- Importing from unregistered engine: "engine not found" error.

## Inline Assembly

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Inline Assembly lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Inline asm parsing: `asm! { }` blocks contain target-specific assembly instructions passed directly to the assembler.
- Variable binding: `%varname` syntax binds V2 local variables as operands to assembly instructions.
- Architecture-specific: asm syntax varies by target; `@cfg` can conditionally provide alternate implementations.
- Must be inside `unsafe` block — asm bypasses all safety checks.

### API Execution Records (Complete)

- asm.parse(instructions): extract raw assembly text -> pass to target assembler.
- asm.bind_operand(var_name, register): bind V2 variable as input/output operand.
- asm.execute(instructions): assemble and execute inline asm -> result in designated output register.
- asm.clobber(registers): declare registers modified by asm -> compiler saves/restores caller-save regs.
- asm.cfg_select(arch, variants): select architecture-specific asm implementation via @cfg.

### Failure Mode Matrix

- Invalid asm syntax for target architecture: assembler error.
- Using asm outside unsafe block: compile error.
- Using asm on non-native target (wasm): compile error.
- Clobbering registers without proper declaration: undefined behavior.

## Actors & Agents

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Actors & Agents lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Actor mailbox: each actor has an incoming message queue; messages processed one at a time (serialized execution).
- Actor lifecycle: runs until explicitly stopped or killed; supervision trees handle failures.
- Agent planning: agent repeatedly calls `plan()` method until `agent_done(self)` signals goal achievement.
- Message passing is async; `actor_call` implements synchronous request/reply pattern.

### API Execution Records (Complete)

- actor.spawn(name, opts): create new actor instance -> return actor handle.
- actor.send(handle, msg): enqueue message to actor mailbox -> non-blocking.
- actor.receive(handle): dequeue from actor outbox -> blocks if empty.
- actor.call(handle, msg): send message and block for reply -> request/reply pattern.
- actor.stop(handle): gracefully stop after current message -> no new messages accepted.
- actor.kill(handle): immediately terminate actor -> no cleanup.
- actor.is_alive(handle): check if actor running -> bool.
- agent.create(name, init_state): create agent instance -> return handle.
- agent.set_goal(handle, key, val): set goal parameter -> merged into goal context.
- agent.run(handle): execute plan loop until agent_done called -> blocks.
- agent.done(self): signal goal achievement from within plan() -> exit run loop.

### Failure Mode Matrix

- Sending to dead actor: runtime error "actor dead".
- Unhandled exception in actor: terminates actor; subsequent sends fail.
- Circular message passing: potential deadlock.
- Agent plan loop never calling agent_done: infinite loop.

## Isolates

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Isolates lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Each isolate is an independent V2 interpreter instance with its own value stack and type registry.
- Scope isolation: isolate has no access to host globals; only values explicitly passed via `globals` option are accessible.
- Memory isolation: values passed across isolate boundary require serialization/deserialization.
- Named isolates persist scope across multiple `isolate_run()` calls.

### API Execution Records (Complete)

- isolate.block(code): run code in one-shot sandbox -> no access to host globals.
- isolate.named(handle, code): run code in named isolate -> scope persists.
- isolate.new(): create named isolate -> return handle.
- isolate.get(handle, var_name): read variable from isolate scope -> value returned.
- isolate.set(handle, var_name, val): write variable into isolate scope.
- isolate.exec(code_str, opts): run source string in sandbox -> Result<value, error>.
- isolate.run(handle, code_str, opts): run source in named isolate -> Result<value, error>.
- isolate.drop(handle): destroy isolate and free resources.

### Failure Mode Matrix

- Accessing host globals inside isolate: "not in scope" error.
- Unhandled exception in isolate: isolate_exec returns Err with error message.
- Sandbox timeout exceeded: isolate_exec returns Err("timeout").
- Non-serializable value in globals option: serialization error.

## Memory Safety and Borrowing

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Memory Safety and Borrowing lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Optional borrow checker: compile-time analysis enforcing Rust-like ownership rules when `@borrow_check` is enabled.
- Immutable borrow (`&x`): multiple readers allowed simultaneously; prevents mutations while borrowed.
- Mutable borrow (`&mut x`): exclusive access; no other borrows allowed while mutable borrow is active.
- Ownership transfer (`move x`): original binding becomes invalid after move; prevents use-after-move.

### API Execution Records (Complete)

- borrow.immutable(val): create immutable reference -> multiple concurrent borrows allowed.
- borrow.mutable(val): create exclusive mutable reference -> no other borrows allowed.
- borrow.deref(ref): access value through reference -> read or write depending on mutability.
- move.transfer(val): transfer ownership -> original binding invalidated.
- freeze(val): make value permanently immutable -> no further mutations allowed.
- is_frozen(val): check if value is frozen -> bool.
- unsafe.mem_write(ptr, offset, val): bypass safety checks -> raw memory write.
- borrow_check.enable(scope): opt-in borrow checking for decorated function or file.
- volatile.bind(name, val): disable compiler optimizations for access -> every read/write goes to memory.

### Failure Mode Matrix

- Multiple mutable borrows: compile error in borrow-checked code.
- Use after move: compile error in borrow-checked code.
- Dangling pointer: undefined behavior in unsafe code.
- Data race in multi-threaded non-atomic mutable access: undefined behavior.

## Manual Allocation

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Manual Allocation lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Unmanaged heap: `mem_alloc` allocates outside V2's GC; programmer responsible for freeing.
- Pointer type is opaque memory address; no type information attached at runtime.
- Size queries (`mem_size_of`) resolve at compile time or runtime depending on type information.
- All manual memory operations require `unsafe` block.

### API Execution Records (Complete)

- mem_alloc(size): allocate size bytes uninitialized -> return pointer.
- mem_alloc_zeroed(size): allocate and zero-initialize -> return pointer.
- mem_realloc(ptr, new_size): resize allocation -> return new pointer (old pointer invalid).
- mem_free(ptr): deallocate memory -> ptr invalid after call.
- mem_write(ptr, offset, byte_val): write byte at ptr+offset.
- mem_read(ptr, offset): read byte at ptr+offset -> byte value.
- mem_copy(dst, src, size): copy size bytes from src to dst.
- mem_set(ptr, byte_val, size): fill size bytes with byte_val.
- mem_size_of(type_name): get byte size of type -> integer.

### Failure Mode Matrix

- Double-free: undefined behavior in unchecked mode.
- Buffer overflow (write beyond allocation): undefined behavior.
- Use-after-free/realloc: dangling pointer (undefined behavior).
- Memory leak if mem_free not called.

## Vectors & Tensors

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Vectors & Tensors lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- SIMD acceleration: vector operations compile to target SIMD instructions when available (SSE, AVX, NEON).
- Dense storage: vectors and tensors store floats contiguously; tensors use row-major layout.
- Broadcasting: scalar-to-vector operations implicitly broadcast scalar across all elements.
- Lazy fusion: some combinators (norm, mean) can be fused during compilation for performance.

### API Execution Records (Complete)

- vec_new(size): create zero-filled float vector of given size.
- vec_from(list): create vector from list of floats.
- vec_add(a, b): element-wise addition -> new vector.
- vec_dot(a, b): dot product -> scalar float.
- vec_norm(v): Euclidean norm -> scalar float.
- vec_scale(v, scalar): multiply all elements by scalar -> new vector.
- tensor_new(shape): create zero-filled tensor with given shape.
- tensor_matmul(a, b): matrix multiplication for 2D tensors -> new tensor.
- tensor_reshape(t, shape): reshape tensor -> view with new shape.
- tensor_softmax(t): apply softmax along last axis -> new tensor.

### Failure Mode Matrix

- Vector dimension mismatch in binary operations: "incompatible dimensions" error.
- Tensor reshape with incompatible element count: reshape error.
- Division by zero in normalization: NaN or Inf result.
- Out-of-bounds tensor indexing: IndexError.

## Compiler Diagnostics

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Diagnostic pipeline, span mapping, error codes, suggestion engine, and output formats documented.

### Implemented Internal Records

- Each diagnostic is a struct: `Diagnostic { level, code, message, spans: [Span], help: [str], notes: [str] }`.
- Spans carry file path, byte offsets, line/column, and an optional label string for the caret underline.
- Error codes use a stable numbering scheme: E0001–E0099 syntax, E0100–E0199 name resolution, E0200–E0299 type system, E0300–E0399 pattern matching, E0400–E0499 type checking, E0500–E0599 borrow checker, E0600–E0699 field/method resolution, E0700–E0799 lifetime/ownership. Warnings: W0001–W0099 unused, W0100–W0199 shadowing/style, W0200–W0299 logic errors.
- Suggestion engine: Levenshtein distance (threshold ≤ 3) for identifier typo suggestions; scope-aware enumeration for "available fields/methods" hints.
- Output renderers: human (ANSI-colored terminal), JSON (structured per-diagnostic), SARIF (for CI/CD integration).
- `--explain CODE` reads from a compiled explanation database embedded in the compiler binary.

### API Execution Records (Complete)

- diag.emit(level, code, message, spans, help?, notes?): construct diagnostic and send to active renderer.
- diag.suggest_typo(name, candidates, threshold): compute Levenshtein matches → attach "did you mean?" help line.
- diag.render_human(diag): format diagnostic with ANSI colors, source-line display, and caret underlines.
- diag.render_json(diag): serialize diagnostic to structured JSON with byte offsets.
- diag.render_sarif(diags): batch-serialize diagnostics to SARIF 2.1.0 format.
- diag.explain(code): look up detailed explanation text for an error code.
- diag.set_max_errors(n): configure error limit; emit "aborting due to N previous errors" when reached.
- diag.set_color(mode): configure color output ("auto" | "always" | "never").

### Failure Mode Matrix

- Unknown error code in `--explain`: "unknown error code" message with suggestion of nearest valid code.
- Span with invalid byte offsets: internal compiler error with context dump.
- Max errors reached: remaining errors suppressed with summary count.
- Renderer mismatch (e.g., SARIF requested but not supported in this build): fallback to JSON with warning.

## Structured Concurrency

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - TaskGroup lifecycle, cancellation propagation, error strategies, and failure matrix documented.

### Implemented Internal Records

- `TaskGroup` is a runtime object that holds a list of child task handles, a cancellation token, and an error strategy.
- Cancellation propagates recursively: cancelling a group cancels all children; nested groups form a tree.
- Tasks check for cancellation at each `await` point; a cancelled task receives `CancelledError` which triggers normal stack unwinding (defer blocks run).
- `task_scope` desugars to: create group → execute block → join_all → propagate errors.
- Error strategies: `"cancel"` (default: first error cancels siblings), `"collect"` (await all, return Ok/Err per child), `"ignore"` (swallow errors, return successes only).

### API Execution Records (Complete)

- task_group(on_error?): create TaskGroup with optional error strategy → returns group handle.
- task_scope(async_fn): create scoped group → execute fn(scope) → auto-join on block exit.
- group.spawn(task): register child task → schedule on event loop → return task handle.
- group.join_all(): await all children → return results list (or propagate first error under "cancel" strategy).
- group.cancel(): send cancellation signal to all active children → await their unwinding.
- group.with_timeout(ms): attach deadline → auto-cancel children if exceeded.
- group.count(): return number of currently active (non-completed) children.

### Failure Mode Matrix

- Spawning on a group after join_all has started: runtime error "cannot spawn into a joining group".
- Timeout exceeded: all children automatically cancelled; `TimeoutError` propagated.
- All children fail under "cancel" strategy: first error propagated, others suppressed.
- Task panics inside group: panic propagated after attempting to cancel siblings.

## Data Classes (`@data`)

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Auto-generated method synthesis, exclusion, destructuring, and failure matrix documented.

### Implemented Internal Records

- `@data` is a compile-time decorator that scans the constructor for `self.field = ...` assignments and synthesizes methods.
- Generated methods: `equals(other)` (field-by-field structural equality), `hash()` (combined hash of all fields), `to_str()` (formatted as `ClassName(field: value, ...)`), `clone()` (shallow copy), `copy(**overrides)` (clone with field substitutions).
- `@data` implies `@fixed` — the field set is locked at definition.
- `exclude` parameter: `@data(exclude: ["field1"])` omits specified fields from equals/hash/to_str.
- Destructuring support: data classes generate positional destructuring bindings based on constructor parameter order.

### API Execution Records (Complete)

- data.synthesize(class_def): inspect constructor → generate equals/hash/to_str/clone/copy methods.
- data.equals(a, b): field-by-field comparison using each field's own `==` operator → bool.
- data.hash(obj): combine field hashes using a deterministic mixing function → int.
- data.to_str(obj): format as `ClassName(field1: val1, field2: val2, ...)` → str.
- data.clone(obj): create new instance with same field values → new object.
- data.copy(obj, overrides): clone → apply overrides dict → new object.
- data.destructure(obj): yield field values in constructor parameter order → tuple.

### Failure Mode Matrix

- `@data` on a class with no constructor: compile error "data class requires a constructor".
- `copy()` with unknown field name in overrides: runtime error "no such field".
- `exclude` referencing non-existent field: compile warning "excluded field does not exist".
- Structural equality on fields that don't implement `==`: compile error "field type does not support equality".

## Sealed Classes

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Subclass registration, exhaustiveness checking, and failure matrix documented.

### Implemented Internal Records

- `sealed` keyword on a class registers all direct subclasses in the same file as the closed set.
- The compiler builds a variant table at the end of file parsing; any `extends` of a sealed class from another file is a compile error.
- Exhaustiveness checking in `match`: the compiler enumerates the sealed variant set and verifies all cases are covered (same algorithm as enum exhaustiveness).
- `sealed` classes cannot be instantiated directly; only their subclasses can be constructed.

### API Execution Records (Complete)

- sealed.register(parent, subclass): add subclass to parent's sealed variant table.
- sealed.close(parent): finalize variant table at end of file → no further subclasses allowed.
- sealed.check_exhaustiveness(match_expr, sealed_type): enumerate variants → verify all covered or default present.
- sealed.is_sealed(type): query whether a type is a sealed class → bool.
- sealed.variants(type): return list of all registered subclass types.

### Failure Mode Matrix

- Extending sealed class from different file: compile error "cannot extend sealed class outside its defining file".
- Non-exhaustive match on sealed class: compile error with missing variant names.
- Instantiating sealed class directly: compile error "sealed class cannot be instantiated directly".
- Sealed class with no subclasses: compile warning "sealed class has no variants".

## Copy-on-Write Classes (`@cow`)

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - COW refcount, mutation-triggered copy, and failure matrix documented.

### Implemented Internal Records

- `@cow` wraps the class's internal storage in a reference-counted container.
- Assignment (`let b = a`) increments the refcount on the shared storage — no data copy.
- Any mutation (write to `self.field` or method call that writes to `self`) checks the refcount; if > 1, a deep copy of the storage is made first, then the mutation proceeds on the private copy.
- If refcount == 1, mutations proceed in-place (no copy needed — uniquely owned).
- Deep copy uses the field types' clone semantics; fields that are themselves `@cow` share their inner storage until individually mutated.

### API Execution Records (Complete)

- cow.wrap(class_def): instrument class storage with refcount wrapper.
- cow.assign(src, dst): increment refcount on shared storage → dst aliases src's data.
- cow.mutate_check(obj): inspect refcount → if > 1, trigger deep copy → decrement old refcount.
- cow.deep_copy(storage): clone all fields → allocate new storage → return.
- cow.refcount(obj): return current reference count (for diagnostics/debugging).

### Failure Mode Matrix

- Deep copy of field that doesn't implement Clone: compile error "cow class requires all fields to be cloneable".
- Refcount overflow (theoretical, >2^63 references): runtime panic.
- `@cow` on class with `unsafe` raw pointer fields: compile error "cow is incompatible with raw pointer fields".

## Trait Composition & Supertraits

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Supertrait resolution, diamond dedup, embedding, and failure matrix documented.

### Implemented Internal Records

- Supertraits declared with `trait A: B + C` syntax; the compiler records B and C as required supertraits of A.
- When checking `impl A for T`, the compiler verifies `impl B for T` and `impl C for T` exist (transitively).
- Diamond case: if B and C both require D, and T implements D once, no duplication occurs — supertrait graph is a DAG, not a tree.
- Trait embedding (`trait Entity: Display + Hash + Eq + Clone {}`) creates an alias trait with no additional methods — bounds on Entity expand to the conjunction.
- Default methods in a trait can reference methods from supertraits without qualification.

### API Execution Records (Complete)

- supertrait.declare(trait_name, supertrait_names): record supertrait edges in trait metadata.
- supertrait.check(trait_name, type_name): transitively verify all supertrait impls exist for type.
- supertrait.expand_bounds(trait_name): flatten supertrait DAG to a set of required traits.
- supertrait.diamond_dedup(trait_dag): ensure shared supertraits are counted once in implementation checks.
- supertrait.default_method_resolve(trait, method): check method resolution order through supertrait chain.

### Failure Mode Matrix

- Missing supertrait impl: compile error "type implements `A` but does not implement required supertrait `B`".
- Cyclic supertrait declaration (`trait A: B`, `trait B: A`): compile error "cyclic supertrait dependency".
- Conflicting default methods from diamond supertraits: compile error "ambiguous method — override required".

## Move Semantics

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Ownership transfer, implicit-move-on-last-use, closure capture, and failure matrix documented.

### Implemented Internal Records

- `move x` transfers ownership: the source binding is marked invalid in the compiler's liveness analysis.
- Under `@borrow_check`: use-after-move is a compile-time error with diagnostic pointing to the move site and the attempted use site.
- Under GC mode: moved bindings are tagged at runtime; access raises `MovedValueError`.
- Implicit move on last use: the compiler detects when a binding's last use is a consumption site and avoids inserting a copy.
- `move` in closures: `move lambda() { ... }` captures all referenced outer bindings by move.
- `move` in match arms: `case Ok(move value)` transfers ownership of the matched inner value.

### API Execution Records (Complete)

- move.transfer(src_binding, dst_binding): invalidate src → transfer value to dst.
- move.check_use_after_move(binding, use_site): verify binding is still valid at use site.
- move.implicit_last_use(binding, use_site): detect last use → elide copy → convert to implicit move.
- move.closure_capture(lambda, bindings): capture listed bindings by move → invalidate outer bindings.
- move.match_arm(pattern, binding): transfer ownership of matched inner value to arm binding.
- move.gc_runtime_check(binding): check GC-mode moved tag → raise MovedValueError if invalid.

### Failure Mode Matrix

- Use of moved value under @borrow_check: compile error with move site and use site spans.
- Use of moved value under GC mode: runtime MovedValueError.
- Moving a value that is currently borrowed: compile error "cannot move out of borrowed value".
- Moving a value inside a loop without re-initialization: compile error on second iteration.

## Hardware Fault Signals

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: **Milestone 1 implemented** — `signal.on_fault` registration, POSIX `sigaction`/Windows VEH handler installation, `FaultInfo` capture, and `signal.dump_json` are in the runtime. Recovery (`set_recovery_point`/`recover`), native core dumps (`dump_core`), backtrace symbolication, and register snapshots are **planned** per `FAULT_HANDLING_DESIGN.md`.

### Implemented Internal Records (Milestone 1)

- `signal.on_fault` installs a platform-specific signal handler (POSIX `sigaction` with SA_SIGINFO, Windows structured exception handler).
- The handler receives a `FaultInfo` object populated from the OS signal context (siginfo_t on POSIX, EXCEPTION_RECORD on Windows).
- Backtrace symbolication uses the same symbol table as the debugger; demangling is applied to V2, C, and Rust symbols.
- `signal.set_recovery_point` uses `sigsetjmp`/`siglongjmp` (POSIX) or `__try`/`__except` (Windows) for recovery.
- Recovery is inherently unsafe: stack frames between the fault and the recovery point are abandoned without running destructors or defer blocks.
- Crash dump: `signal.dump_core` calls platform APIs (POSIX `abort()` after enabling core dumps, Windows `MiniDumpWriteDump`). `signal.dump_json` serializes FaultInfo + loaded module list + OS info.

### API Execution Records (Complete)

- signal.on_fault(name, handler): validate signal name is in {SIGSEGV, SIGBUS, SIGFPE, SIGABRT} → install OS handler → requires unsafe context.
- signal.set_recovery_point(): call sigsetjmp → return RecoveryPoint { recovered: false } → on fault, siglongjmp back with recovered: true.
- signal.recover(): inside fault handler, jump to most recent recovery point → abandon intermediate frames.
- signal.dump_core(path): trigger platform core dump → write to specified path.
- signal.dump_json(path): serialize FaultInfo + modules + OS info → write JSON to path.
- FaultInfo.construct(os_context): extract signal, address, pc, thread_id, registers, backtrace, is_stack_overflow from OS context.

### Failure Mode Matrix

- `signal.on_fault` outside unsafe block: compile error "hardware fault handlers require unsafe context".
- `signal.on_fault` with non-hardware signal name: runtime error "only hardware fault signals supported".
- `signal.recover` called outside a fault handler: runtime panic "no active fault to recover from".
- Recovery point invalidated by stack unwinding: undefined behavior (documented as unsafe).
- Crash dump path not writable: fault handler continues but dump_core/dump_json returns error.

## Channels and Threads

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Channels and Threads lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Buffered channels maintain a FIFO queue; unbuffered channels force synchronous handoff.
- Thread spawning via `thread_spawn(func)` creates OS thread; result obtained via `thread_join()`.
- Mutex locking: `mutex_with(m, f)` acquires lock, runs f, releases lock atomically.
- Thread pool: `threadpool_submit(pool, fn)` enqueues fn to worker pool; load balanced across workers.

### API Execution Records (Complete)

- chan_create(buffer_size): create channel with optional buffer -> channel handle.
- chan_send(ch, msg): send message -> blocks if buffer full.
- chan_recv(ch): receive message -> blocks if empty.
- chan_close(ch): close channel -> no new sends.
- thread_spawn(fn): spawn OS thread -> return thread handle.
- thread_join(handle): wait for thread completion -> return result.
- mutex_create(): create mutex -> handle.
- mutex_lock(m): acquire exclusive lock -> blocks if held.
- mutex_unlock(m): release lock.
- mutex_with(m, fn): acquire lock -> run fn -> release lock (RAII pattern).
- threadpool_create(n): create pool with n workers -> handle.
- threadpool_submit(pool, fn): enqueue task -> load balanced.
- atomic_new(val): create atomic variable -> handle.
- atomic_add(atom, delta): atomically add -> return old value.
- atomic_cas(atom, old, new): compare-and-swap -> bool success.

### Failure Mode Matrix

- Deadlock from mutexes acquired in opposite order.
- Race condition on non-atomic shared mutable state.
- Channel send after close: panic.
- Thread panic silently terminates thread; join returns error.

## Testing

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Testing lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Test discovery: compiler scans source for `test` blocks; `vt test` runs all discovered tests.
- Each test runs in isolation with its own scope; failure halts the test but not others.
- Benchmark warmup: bench blocks run warmup phase before measurement to prime caches.
- Coverage instrumentation: `--coverage` instruments all lines and branches; report generated after run.

### API Execution Records (Complete)

- test.register(name, body): register test block -> discovered by vt test.
- test.expect_eq(actual, expected): assert equality -> test fails if not equal.
- test.expect_true(val) / test.expect_false(val): assert truthiness.
- test.expect_ok(result) / test.expect_err(result): assert Result state.
- test.expect_some(val) / test.expect_none(val): assert Option state.
- test.expect_throws(fn): assert fn() throws -> test fails if no throw.
- bench.register(name, opts, body): register benchmark -> executed by vt bench.
- bench.measure(body, iters, warmup): run warmup -> measure iters -> compute stats.
- test.tag(name, tags): associate tags with test -> filterable with --tag.
- test.snapshot(name, val): compare against saved baseline -> save on first run.
- test.property(name, gen_or_fn, opts): generate random inputs -> verify property holds -> shrink on failure.
- test.coverage_report(): after --coverage run -> return per-file line/branch coverage.
- patch(target, replacement): swap function body -> defer restore for test cleanup.

### Failure Mode Matrix

- Test assertion failure: AssertionError halts test.
- Unhandled exception in test body: test failure.
- Benchmark timeout exceeded: marked as timed-out.
- Modifying global state in test: side effects for other tests.
- Property test failure: shrunk counterexample reported.

## Builtins Reference

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Builtins Reference lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Builtin functions resolved at compile time with specialized bytecode generation.
- Type coercion in builtins: arguments trigger conversion logic (e.g., `int()` parses strings).
- Short-circuit evaluation for logical operators.
- Math functions delegate to host C math library for native performance.

### API Execution Records (Complete)

- print(...values): print values space-separated with newline -> stdout.
- type(val): get type name string.
- len(val): get length of string, list, dict, set, or tuple.
- int(val, base?): convert to int with optional base -> ParseError on failure.
- float(val): convert to float -> ParseError on failure.
- str(val): convert to string representation.
- bool(val): convert to boolean using truthiness rules.
- range(start?, end, step?): create lazy range iterator.
- sort(list): sort list in-place (TimSort) -> return list.
- sum(list) / max(list) / min(list): reduce list -> numeric result.
- sqrt(x) / pow(base, exp) / abs(x): math functions -> numeric result.
- json_parse(str): parse JSON string -> V2 value.
- json_stringify(val): serialize V2 value to JSON string.
- clone(val): deep copy value -> independent copy.
- hash(val): compute hash of hashable value -> int.
- input(prompt?): read line from stdin -> string.

### Failure Mode Matrix

- int("not a number"): ParseError.
- Division by zero in math operations: error or Inf/NaN.
- len() on non-collection type: TypeError.
- Invalid JSON in json_parse: ParseError.
- hash() on unhashable type: TypeError.

## Method Reference

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Method Reference lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Method dispatch resolves receiver type at compile time; runtime uses vtable if polymorphic.
- Method chaining: methods returning self or another object enable fluent interfaces.
- Built-in methods on collections are optimized; slice operations create views or copies.

### API Execution Records (Complete)

- method.dispatch(receiver, name, args): resolve method from type -> call with self binding.
- method.chain(receiver, calls): evaluate each call in sequence -> thread result through.
- list.sort_by(fn): sort by key function -> return sorted list.
- list.flat_map(fn): map then flatten one level -> new list.
- list.enumerate(): return list of (index, element) tuples.
- list.zip(other): pair elements -> list of tuples.
- list.chunks(n): split into groups -> list of lists.
- list.unique(): remove duplicates preserving order.
- str.pad_start(n, ch?) / str.pad_end(n, ch?): pad string to width.
- str.indexOf(needle) / str.lastIndexOf(needle): find substring position.

### Failure Mode Matrix

- Method on wrong type: TypeError.
- Mutating method on immutable value: error.
- Method chain returning null: error on subsequent call.
- Slice out of bounds: IndexError.

## Operator Overloading

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Operator Overloading lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Each overloadable operator maps to a special method (`__add__`, `__lt__`, etc.) in the class vtable.
- Overloaded operators maintain V2 precedence rules; no custom precedence allowed.
- Compiler rewrites `a + b` to `a.__add__(b)` when operator overloading is present.
- Unary operators (`!`, `-`) map to `__not__()` and `__neg__()`.

### API Execution Records (Complete)

- overload.**add**(self, other): implements + operator.
- overload.**sub**(self, other): implements - operator.
- overload.**mul**(self, other): implements \* operator.
- overload.**div**(self, other): implements / operator.
- overload.**eq**(self, other): implements == operator.
- overload.**lt**(self, other): implements < operator.
- overload.**getitem**(self, key): implements obj[key] access.
- overload.**setitem**(self, key, val): implements obj[key] = val assignment.
- overload.**len**(self): implements len(obj).
- overload.**str**(self): implements str(obj).
- overload.**contains**(self, item): implements `item in obj`.
- overload.**iter**(self): implements `for item in obj` -> return Iterator.

### Failure Mode Matrix

- Operator on object without corresponding overload method: TypeError "operator not supported".
- Operator method returning wrong type: type error downstream.
- Infinite recursion in **add** calling +: stack overflow.
- Type mismatch between operand types: coercion error.

## Keywords

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Keywords lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- All V2 keywords are reserved; they cannot be used as variable or function names.
- Keywords are recognized during tokenization; no runtime keyword lookup.
- Some keywords (like `using`) have context-dependent meaning based on parsing context.

### API Execution Records (Complete)

- keyword.let: declare mutable variable binding.
- keyword.const: declare immutable constant.
- keyword.func / keyword.class / keyword.struct / keyword.enum / keyword.trait: type declarations.
- keyword.impl: implement trait on type.
- keyword.return / keyword.throw / keyword.yield: control flow exit points.
- keyword.try / keyword.catch / keyword.finally: exception handling.
- keyword.match / keyword.case / keyword.default: pattern matching.
- keyword.if / keyword.elif / keyword.else: conditionals.
- keyword.while / keyword.for: loops.
- keyword.break / keyword.continue: loop control.
- keyword.async / keyword.await: async control flow.
- keyword.pub / keyword.private / keyword.internal: visibility modifiers.
- keyword.import / keyword.from / keyword.as: module system.
- keyword.unsafe: unsafe block entry.
- keyword.defer: cleanup registration.
- keyword.lazy: lazy expression creation.

### Failure Mode Matrix

- Using keyword as identifier: compile error "reserved keyword".
- Missing required keyword in declaration: parse error.
- Keyword in wrong context: parse error with context-specific message.

## Importing Standard Libraries

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Importing Standard Libraries lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Stdlib modules ship with V2 toolchain; version pinned by V2 version.
- Lazy module loading: stdlib modules loaded on first `import`; unused modules don't load.
- Module symbol registration: importing registers all exported symbols in current namespace.
- Namespace collision: importing two modules with conflicting names requires `as` aliasing.

### API Execution Records (Complete)

- stdlib.import(module_name): load stdlib module -> register namespace.
- stdlib.selective_import(module, symbols): import only specified symbols.
- stdlib.alias(module, alias_name): register module under alias.
- stdlib.wildcard_import(module): import all exports directly into current namespace.
- stdlib.availability_check(module): verify module exists for current platform -> compile error if not.

### Failure Mode Matrix

- Importing non-existent stdlib module: compile error.
- Symbol collision from two stdlib imports: compile error.
- Stdlib module with missing native dependency: "module not available" error.

## Stdlib Module Catalog

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Stdlib Module Catalog lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Stdlib modules grouped by domain (fs, net, db, crypto, etc.).
- Most modules can be used independently; some have internal dependencies.
- Each module has dedicated API documentation accessible via `vt doc`.
- Capability profiles combine modules into typical application stacks.

### API Execution Records (Complete)

- catalog.list(): enumerate all available stdlib modules with descriptions.
- catalog.resolve(module_name): locate module source and metadata in stdlib tree.
- catalog.doc_generate(module_name): extract doc comments -> produce API reference.
- catalog.profile(name): load predefined set of modules for a use case (Core Service, Data+AI, etc.).
- catalog.version(module_name): return stdlib module version (pinned to V2 release).

### Failure Mode Matrix

- Referencing non-existent stdlib module in catalog: compile error.
- Stdlib module API misuse: runtime error with specific API context.
- Missing native dependency for platform-specific module: "not available on this platform" error.

## std.fs — Filesystem

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.fs path/handle internals and filesystem-operation failure behavior are documented.

### Implemented Internal Records

- Filesystem layer normalizes path semantics and metadata queries across host platforms.
- Handle manager tracks open descriptors with capability modes and lock-state metadata.
- Atomic operation helpers implement safe replace/move/temp-write commit patterns.

### API Execution Records (Complete)

- std.fs.read(path, options): read file bytes/text with encoding and buffering policy.
- std.fs.write(path, data, options): write content with create/truncate/append semantics.
- std.fs.list(path, options): enumerate directory entries with metadata projection controls.
- std.fs.meta(path): query file metadata (size/type/timestamps/permissions).

### Failure Mode Matrix

- Path resolution fails or target missing: fs-path diagnostic.
- Permission denied for requested filesystem operation: fs-permission diagnostic.
- Atomic write/replace commit fails integrity checks: fs-atomic-write diagnostic.
- Directory traversal encounters cyclic/symlink policy conflict: fs-traversal diagnostic.

## std.fmt — Formatting

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.fmt format-engine internals and formatting failure behavior are documented.

### Implemented Internal Records

- Formatter parser compiles format strings into tokenized render plans with placeholder metadata.
- Value formatter dispatches type-specific formatters and locale-aware rendering adapters.
- Output sink abstraction supports string builders, stream writers, and bounded buffers.

### API Execution Records (Complete)

- std.fmt.format(pattern, args, options): render formatted output from compiled format plan.
- std.fmt.printf(writer, pattern, args): stream formatted output to writable sink.
- std.fmt.parse(pattern): parse and validate format pattern into reusable plan object.
- std.fmt.register(type, formatter): register custom formatter hook for user type.

### Failure Mode Matrix

- Invalid format token or placeholder syntax: fmt-parse diagnostic.
- Argument arity/type mismatch for format placeholders: fmt-argument diagnostic.
- Custom formatter hook raises or violates contract: fmt-custom diagnostic.
- Output sink write failure during rendering: fmt-sink diagnostic.

## std.regex — Regular Expressions

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.regex compile/match internals and regex-execution failure behavior are documented.

### Implemented Internal Records

- Regex compiler translates pattern AST to optimized matcher automata with flag-aware transforms.
- Match engine supports anchored, global, and capture-group extraction over text/byte inputs.
- Pattern cache stores compiled automata by pattern+flag key with bounded eviction policy.

### API Execution Records (Complete)

- std.regex.compile(pattern, flags): compile regex pattern to reusable matcher object.
- std.regex.match(re, input): test input against compiled regex and return match summary.
- std.regex.findAll(re, input): enumerate all non-overlapping matches with capture data.
- std.regex.replace(re, input, repl): perform regex-driven replacement with capture expansion.

### Failure Mode Matrix

- Pattern parse error or unsupported construct: regex-compile diagnostic.
- Catastrophic backtracking guard triggered under strict mode: regex-runtime diagnostic.
- Capture expansion references invalid group index/name: regex-capture diagnostic.
- Input encoding mismatch for regex mode (text/byte): regex-encoding diagnostic.

## std.iter — Iterator Combinators

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.iter pipeline/combinator internals and iterator failure behavior are documented.

### Implemented Internal Records

- Iterator core models lazy producer chains with pull-based state transitions.
- Combinator planner fuses map/filter/reduce stages when optimization rules permit.
- Cursor safety checks enforce invalidation and single-consumer progression invariants.

### API Execution Records (Complete)

- std.iter.from(source): create iterator cursor from collection/range/source adapter.
- std.iter.map(iter, fn): apply transformation combinator to iterator pipeline.
- std.iter.filter(iter, pred): apply predicate filter stage to iterator pipeline.
- std.iter.collect(iter, target): materialize iterator output into target collection type.

### Failure Mode Matrix

- Iterator source exhausted/invalidated unexpectedly in strict mode: iter-state diagnostic.
- Combinator closure violates type/effect contract: iter-combinator diagnostic.
- Collect target incompatible with yielded element type: iter-collect diagnostic.
- Fused pipeline optimization exceeded complexity guard: iter-optimization diagnostic.

## std.time — Date & Time

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.time clock/calendar internals and time-operation failure behavior are documented.

### Implemented Internal Records

- Time subsystem unifies monotonic/system clocks with timezone-aware calendar conversions.
- Duration arithmetic engine handles overflow-safe add/subtract and normalization rules.
- Formatting/parsing adapters map between temporal values and locale/standard wire formats.

### API Execution Records (Complete)

- std.time.now(clock): read current instant from selected clock source.
- std.time.parse(text, layout): parse textual timestamp using provided layout rules.
- std.time.format(value, layout): format temporal value into textual representation.
- std.time.add(value, duration): compute time result using normalized duration arithmetic.

### Failure Mode Matrix

- Time parse input mismatches layout grammar: time-parse diagnostic.
- Duration arithmetic overflow in checked mode: time-overflow diagnostic.
- Unsupported timezone/offset conversion requested: time-timezone diagnostic.
- Clock source unavailable or unstable on platform: time-clock diagnostic.

## std.proc — Process Management

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.proc lifecycle/stdio internals and process-control failure behavior are documented.

### Implemented Internal Records

- Process launcher builds executable invocation plans with argv/env/working-dir inheritance policies.
- Child handle manager coordinates stdin/stdout/stderr pipes and lifecycle state transitions.
- Exit/status collector normalizes host return codes, signals, and termination metadata.

### API Execution Records (Complete)

- std.proc.spawn(spec): create child process with IO redirection and environment options.
- std.proc.wait(proc, options): wait for process completion with timeout/kill policy.
- std.proc.signal(proc, sig): send control signal/termination request to child process.
- std.proc.output(spec): execute process and capture buffered output + exit status.

### Failure Mode Matrix

- Executable not found or launch denied by host policy: proc-launch diagnostic.
- Pipe setup/read/write fails during process lifetime: proc-io diagnostic.
- Wait timeout exceeded and termination policy unresolved: proc-timeout diagnostic.
- Exit status decoding fails for host-specific reason format: proc-status diagnostic.

## std.log — Structured Logging

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.log event/sink internals and logging-pipeline failure behavior are documented.

### Implemented Internal Records

- Logging core builds structured event records with level, context, and span correlation fields.
- Sink multiplexer routes events to console/file/remote sinks with per-sink backpressure policy.
- Filter pipeline applies level/module/field predicates before expensive serialization paths.

### API Execution Records (Complete)

- std.log.configure(config): initialize logger graph, sinks, and filter rules.
- std.log.emit(level, message, fields): push structured log event through active pipeline.
- std.log.flush(): force buffered sinks to persist outstanding events.
- std.log.scope(ctx): create scoped logging context with inherited metadata fields.

### Failure Mode Matrix

- Sink initialization fails due to path/network/backend issue: log-sink-init diagnostic.
- Event serialization error for field/type in strict mode: log-serialization diagnostic.
- Sink backpressure exceeds drop/block policy limits: log-backpressure diagnostic.
- Invalid logger configuration graph detected at startup: log-config diagnostic.

## std.test — Testing (Enhanced)

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.test case orchestration/assertion internals and test-runtime failure behavior are documented.

### Implemented Internal Records

- Test harness discovers test symbols, parameterized cases, and fixtures into executable test plans.
- Execution engine isolates test contexts, captures stdout/stderr/events, and applies timeout/retry policies.
- Assertion subsystem emits structured failure payloads with diff snapshots and source-span metadata.

### API Execution Records (Complete)

- std.test.discover(modules): build deterministic test inventory from exported test declarations.
- std.test.plan(cases, config): produce execution schedule with isolation, retries, and sharding options.
- std.test.run(plan): execute tests and stream structured result events.
- std.test.report(results, format): render summary/artifact output for CI and local diagnostics.

### Failure Mode Matrix

- Fixture initialization failure before case execution: fixture-runtime diagnostic.
- Assertion mismatch in strict mode: assertion diagnostic with expected/actual diff payload.
- Test case exceeds configured timeout budget: timeout diagnostic with case metadata.
- Harness state corruption during parallel run: test-runtime integrity diagnostic.

## std.math — Mathematics

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.math numeric kernel dispatch and math-domain internals and failure behavior are documented.

### Implemented Internal Records

- Math dispatcher routes scalar/vector operations to backend kernels based on type and precision class.
- Deterministic mode constrains floating-point optimizations and records rounding policy metadata.
- Special-function subsystem handles approximation tables and fallback algorithms per target profile.

### API Execution Records (Complete)

- std.math.eval(op, args): execute math operation via type-directed kernel dispatch.
- std.math.precision(scope, mode): set/query precision and rounding policy for evaluation scope.
- std.math.special(name, args): evaluate special functions with backend fallback handling.
- std.math.vectorize(op, buffers): apply operation across vectorized buffer lanes.

### Failure Mode Matrix

- Domain error for operation input set (e.g., sqrt of negative in real mode): math-domain diagnostic.
- Precision policy violation under deterministic mode: precision-policy diagnostic.
- Backend kernel unavailable for requested type/target: math-backend diagnostic.
- Numerical instability threshold exceeded in strict analysis mode: numeric-stability diagnostic.

## std.io — Input / Output

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.io stream/buffer pipeline internals and I/O failure behavior are documented.

### Implemented Internal Records

- IO core unifies file/socket/memory streams under reader-writer trait contracts with capability flags.
- Buffered layer manages read-ahead/write-behind windows and flush consistency guarantees.
- Async bridge adapts blocking descriptors into event-loop compatible operations when runtime is present.

### API Execution Records (Complete)

- std.io.open(resource, mode): create IO handle with capability metadata.
- std.io.read(handle, buffer): pull bytes/chunks respecting buffering and blocking policy.
- std.io.write(handle, data): push payload with partial-write handling and flush semantics.
- std.io.close(handle): release handle and finalize pending buffered operations.

### Failure Mode Matrix

- Read/write on closed or invalid handle: io-handle-state diagnostic.
- Permission or capability mismatch for requested operation: io-capability diagnostic.
- Flush failure due to downstream sink error: io-flush diagnostic with partial-write metadata.
- Async bridge requested without runtime support: io-runtime diagnostic.

## std.collections — Data Structures

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.collections storage/index internals and collection failure behavior are documented.

### Implemented Internal Records

- Collection backends implement contiguous, hashed, and tree-based storage strategies with common iterator contracts.
- Mutation paths maintain structural invariants and version counters for iterator invalidation checks.
- Allocator integration supports growth policies and small-object optimization hooks where applicable.

### API Execution Records (Complete)

- std.collections.new(kind, config): allocate collection instance with backend-specific policy.
- std.collections.insert(coll, key, value): mutate collection according to structure semantics.
- std.collections.get(coll, key): retrieve element/value with optional default behavior.
- std.collections.iter(coll): produce stable iterator cursor with invalidation tracking.

### Failure Mode Matrix

- Key/index lookup invalid under strict access mode: collection-access diagnostic.
- Structural invariant violation detected after mutation: collection-integrity diagnostic.
- Iterator used after invalidating mutation in strict mode: iterator-invalid diagnostic.
- Allocation growth failure during resize: collection-allocation diagnostic.

## std.serialize — Serialization

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.serialize encode/decode pipeline internals and serialization failure behavior are documented.

### Implemented Internal Records

- Serializer planner maps type schemas to format-specific encoders with field-order and tagging metadata.
- Decoder pipeline performs incremental parsing with schema-guided reconstruction and validation hooks.
- Versioning layer supports compatibility adapters and optional unknown-field retention policy.

### API Execution Records (Complete)

- std.serialize.encode(value, format, options): serialize value into target wire/document format.
- std.serialize.decode(data, type, format, options): reconstruct typed value from serialized input.
- std.serialize.schema(type): emit canonical schema descriptor for type metadata exchange.
- std.serialize.transcode(data, fromFormat, toFormat): convert serialized payload between formats.

### Failure Mode Matrix

- Input payload malformed for selected format: decode-parse diagnostic.
- Type schema mismatch during decode reconstruction: schema-mismatch diagnostic.
- Required field missing under strict schema mode: serialization-required-field diagnostic.
- Unsupported format codec requested at runtime: serialization-codec diagnostic.

## std.ai — Artificial Intelligence

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.ai model/runtime integration internals and AI-runtime failure behavior are documented.

### Implemented Internal Records

- AI runtime broker manages model loading, backend selection (CPU/GPU), and inference session lifecycle.
- Tensor adapter normalizes input/output shapes and dtypes between language values and model buffers.
- Resource governor enforces memory/latency budgets for inference and training-adjacent operations.

### API Execution Records (Complete)

- std.ai.model.load(spec): load model artifact and initialize runtime backend bindings.
- std.ai.infer(model, input, options): execute inference pass and return structured output tensors.
- std.ai.tensor.convert(value, shape, dtype): map language values into tensor representation.
- std.ai.session.close(session): release model/runtime resources deterministically.

### Failure Mode Matrix

- Model artifact missing/corrupt or unsupported format: ai-model-load diagnostic.
- Input tensor shape/dtype incompatible with model signature: ai-input-signature diagnostic.
- Backend resource budget exceeded during inference: ai-resource-budget diagnostic.
- Requested acceleration backend unavailable on target platform: ai-backend diagnostic.

## std.crypto — Cybersecurity & Cryptography

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.crypto primitive/key-management internals and cryptographic failure behavior are documented.

### Implemented Internal Records

- Crypto registry maps algorithm identifiers to vetted primitive implementations and capability constraints.
- Key manager controls key generation/import/derivation with secure-memory handling and lifecycle metadata.
- Entropy provider abstracts platform RNG sources with health checks and fallback policy.

### API Execution Records (Complete)

- std.crypto.hash(algo, data): compute digest using selected hashing primitive.
- std.crypto.cipher.encrypt(algo, key, nonce, plaintext): produce authenticated/encrypted payload.
- std.crypto.cipher.decrypt(algo, key, nonce, ciphertext): verify and decrypt protected payload.
- std.crypto.key.derive(method, params): derive key material via configured KDF.

### Failure Mode Matrix

- Unsupported or disabled algorithm requested by policy: crypto-algorithm diagnostic.
- Key/nonce length invalid for selected cipher suite: crypto-parameter diagnostic.
- Authentication tag verification failed on decrypt: crypto-authentication diagnostic.
- Entropy source health check failure in strict mode: crypto-entropy diagnostic.

## std.gfx3d — 3D Graphics

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.gfx3d scene/render internals and 3D rendering failure behavior are documented.

### Implemented Internal Records

- 3D scene graph manages node transforms, material bindings, and light state propagation.
- Render pipeline schedules draw passes across backend API adapters with resource lifetime tracking.
- Asset loader resolves mesh/texture/shader resources with cache and dependency metadata.

### API Execution Records (Complete)

- std.gfx3d.scene.create(config): initialize scene graph and renderer state.
- std.gfx3d.mesh.load(source, options): load mesh asset into GPU-ready buffers.
- std.gfx3d.render.frame(scene, camera): execute render pass chain for active frame.
- std.gfx3d.resource.release(handle): dispose GPU/resource allocations safely.

### Failure Mode Matrix

- Shader/material compilation or binding failure: gfx3d-shader diagnostic.
- Asset decode/load failure for mesh or texture source: gfx3d-asset diagnostic.
- Render pass submission exceeds backend capability limits: gfx3d-backend diagnostic.
- Resource lifecycle misuse detected in strict mode: gfx3d-resource diagnostic.

## std.game — Game Creation

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.game loop/entity internals and game-runtime failure behavior are documented.

### Implemented Internal Records

- Game runtime coordinates fixed/variable timestep loops with deterministic update ordering.
- Entity/component store tracks lifecycle, system queries, and mutation safety constraints.
- Input/timing adapters normalize platform events and frame timing metrics.

### API Execution Records (Complete)

- std.game.runtime.start(config): initialize game runtime and main loop scheduling.
- std.game.entity.spawn(spec): create entity with initial component set.
- std.game.system.register(system): register update/render system pipeline stage.
- std.game.runtime.tick(state): execute one update-render iteration of main loop.

### Failure Mode Matrix

- System update throws or violates component contract: game-system diagnostic.
- Entity/component access references missing or stale handle: game-entity diagnostic.
- Frame budget overrun exceeds configured policy: game-frame-budget diagnostic.
- Runtime initialization fails backend/input setup: game-runtime-init diagnostic.

## std.os — Operating System

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.os process/system-call internals and OS integration failure behavior are documented.

### Implemented Internal Records

- OS abstraction layer normalizes process, environment, and filesystem primitives across targets.
- Capability gate enforces platform-specific permission constraints before privileged operations.
- System call adapters translate runtime requests to host ABI with structured error normalization.

### API Execution Records (Complete)

- std.os.process.spawn(spec): launch child process with environment and handle inheritance policy.
- std.os.env.get(name): read environment value under configured exposure policy.
- std.os.path.resolve(path): normalize and canonicalize host path semantics.
- std.os.platform.info(): retrieve host platform/arch/runtime metadata.

### Failure Mode Matrix

- Unsupported system capability on active target: os-capability diagnostic.
- Spawn/request denied by host permissions: os-permission diagnostic.
- System call returns unrecoverable host error: os-syscall diagnostic.
- Cross-platform path normalization conflict in strict mode: os-path diagnostic.

## std.compress — Compression

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.compress codec/stream internals and compression failure behavior are documented.

### Implemented Internal Records

- Compression registry maps algorithm names to encoder/decoder implementations.
- Stream adapters support chunked compress/decompress workflows with window state tracking.
- Container helpers manage framing metadata and integrity checksums.

### API Execution Records (Complete)

- std.compress.encode(algo, data, options): compress payload using selected codec.
- std.compress.decode(algo, data, options): decompress payload into original form.
- std.compress.stream.encoder(algo, sink): create incremental encoder stream wrapper.
- std.compress.stream.decoder(algo, source): create incremental decoder stream wrapper.

### Failure Mode Matrix

- Unsupported compression algorithm requested: compress-algorithm diagnostic.
- Corrupt compressed payload or checksum mismatch: compress-integrity diagnostic.
- Decoder window/state invalid for stream sequence: compress-stream-state diagnostic.
- Compression ratio/memory policy constraints violated: compress-policy diagnostic.

## std.xml — XML & HTML Parsing

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.xml parse/tree internals and XML/HTML parsing failure behavior are documented.

### Implemented Internal Records

- XML parser tokenizes elements, attributes, text, and namespace scopes into DOM/SAX representations.
- Validation layer enforces well-formedness and optional schema/DTD rule checks.
- Serializer emits canonical XML/HTML with configurable escaping and formatting policies.

### API Execution Records (Complete)

- std.xml.parse(text, options): parse XML/HTML input into tree or streaming events.
- std.xml.query(doc, selector): evaluate path/query expression over parsed document.
- std.xml.stringify(doc, options): serialize document tree to textual output.
- std.xml.validate(doc, schema): validate document structure against schema definition.

### Failure Mode Matrix

- Malformed tag/attribute/namespace structure detected: xml-parse diagnostic.
- Query path references missing/invalid node context: xml-query diagnostic.
- Schema/DTD validation fails strict conformance checks: xml-schema diagnostic.
- Serialization emits invalid character or encoding sequence: xml-serialize diagnostic.

## std.image — Image Processing

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.image codec/pixel internals and image-processing failure behavior are documented.

### Implemented Internal Records

- Image codec layer handles decode/encode for supported raster formats with metadata extraction.
- Pixel pipeline performs transforms (resize/crop/filter/color-space) over typed image buffers.
- Memory manager tracks large-frame allocations and tiling strategies for bounded processing.

### API Execution Records (Complete)

- std.image.load(source, options): decode image source into in-memory pixel buffer.
- std.image.transform(img, ops): apply transformation operation chain to image buffer.
- std.image.save(img, target, format, options): encode and persist image output.
- std.image.meta(img): retrieve dimensions/color profile/format metadata.

### Failure Mode Matrix

- Unsupported/corrupt image format encountered: image-codec diagnostic.
- Transform operation parameters invalid for source buffer: image-transform diagnostic.
- Color profile conversion unsupported or lossy under strict mode: image-color diagnostic.
- Memory budget exceeded during large image processing: image-memory diagnostic.

## std.mail — Email

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.mail SMTP/IMAP pipeline internals and mail-delivery failure behavior are documented.

### Implemented Internal Records

- Mail transport engine coordinates SMTP submit, retry queueing, and delivery-status tracking.
- Message builder normalizes headers, MIME body parts, and attachment encoding policies.
- Auth/TLS layer negotiates secure sessions and credential mechanisms per provider capabilities.

### API Execution Records (Complete)

- std.mail.compose(headers, body, attachments): build canonical MIME message structure.
- std.mail.send(client, message): submit message via SMTP transport with retry policy handling.
- std.mail.fetch(account, query): retrieve message metadata/content from mailbox backend.
- std.mail.sync(account, state): reconcile remote mailbox deltas into local state snapshot.

### Failure Mode Matrix

- SMTP/IMAP authentication failure: mail-auth diagnostic with provider context.
- Invalid MIME composition or attachment encoding: mail-message-format diagnostic.
- Delivery retry budget exhausted after transient failures: mail-delivery diagnostic.
- TLS negotiation fails policy constraints: mail-transport-security diagnostic.

## std.net — Networking

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.net socket/session internals and network-transport failure behavior are documented.

### Implemented Internal Records

- Network core abstracts TCP/UDP socket lifecycles with nonblocking and timeout policy controls.
- Address resolver and connection planner normalize endpoint families and fallback strategies.
- Stream/datagram adapters provide buffered IO integration and event-loop compatible readiness signals.

### API Execution Records (Complete)

- std.net.connect(endpoint, options): establish outbound network session with policy constraints.
- std.net.listen(endpoint, options): create inbound listener and accept loop resources.
- std.net.send(handle, payload): transmit payload over stream/datagram transport path.
- std.net.receive(handle, buffer): receive bytes/frames with timeout and partial-read handling.

### Failure Mode Matrix

- Endpoint resolution/connect attempt fails: network-connect diagnostic.
- Socket operation on closed or invalid descriptor: network-handle-state diagnostic.
- Send/receive timeout exceeds configured budget: network-timeout diagnostic.
- Protocol family mismatch for selected endpoint: network-protocol diagnostic.

## std.db — Databases

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.db connection/query internals and database-operation failure behavior are documented.

### Implemented Internal Records

- Database adapter layer manages pooled connections and driver-specific protocol handling.
- Query executor supports prepared statements, parameter binding, and transaction scopes.
- Result mapper converts row sets into typed values with nullability and conversion policies.

### API Execution Records (Complete)

- std.db.connect(dsn, options): open/pool database connection resources.
- std.db.query(conn, sql, params): execute query and stream/materialize result rows.
- std.db.exec(conn, sql, params): execute mutation statement and return effect metadata.
- std.db.tx.run(conn, fn): execute callback within managed transaction boundary.

### Failure Mode Matrix

- Connection/authentication handshake fails for backend: db-connect diagnostic.
- SQL parse/prepare/bind error in query execution: db-query diagnostic.
- Transaction commit/rollback fails consistency checks: db-transaction diagnostic.
- Result type conversion mismatch in strict mapping mode: db-mapping diagnostic.

## std.ui — User Interface

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.ui widget/layout internals and UI-runtime failure behavior are documented.

### Implemented Internal Records

- UI tree reconciler applies diffed widget updates to backend view hierarchy.
- Layout engine computes constraints and geometry resolution for nested containers.
- Event dispatcher routes input/focus/state events through bubbling/capture phases.

### API Execution Records (Complete)

- std.ui.app.create(config): initialize UI runtime and root view surface.
- std.ui.view.render(tree): reconcile widget tree and apply backend updates.
- std.ui.state.bind(node, store): bind reactive state source to UI node updates.
- std.ui.event.on(node, event, handler): register UI event handler callback.

### Failure Mode Matrix

- Invalid widget tree or unsupported node configuration: ui-tree diagnostic.
- Layout constraint conflict yields unsatisfied geometry: ui-layout diagnostic.
- Event handler throws in protected dispatch context: ui-event diagnostic.
- Backend render target initialization fails on platform: ui-backend diagnostic.

## std.term — Terminal & ANSI

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.term terminal/ANSI internals and terminal-IO failure behavior are documented.

### Implemented Internal Records

- Terminal adapter abstracts TTY capabilities, ANSI support levels, and mode switches.
- Input parser decodes key/control sequences into normalized terminal events.
- Output formatter applies style/color/cursor operations with fallback for limited terminals.

### API Execution Records (Complete)

- std.term.open(options): initialize terminal session with capability detection.
- std.term.write(term, content, style): emit styled content and control sequences.
- std.term.readKey(term, options): read normalized key/control input event.
- std.term.mode(term, settings): set raw/canonical/echo mode behavior.

### Failure Mode Matrix

- Terminal capability detection fails or no TTY available: term-capability diagnostic.
- Unsupported ANSI/style sequence on active terminal profile: term-style diagnostic.
- Input decode error for invalid control sequence: term-input diagnostic.
- Mode switch denied by host terminal policy: term-mode diagnostic.

## std.cli — CLI Argument Parsing

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.cli parser/command internals and CLI argument failure behavior are documented.

### Implemented Internal Records

- CLI parser tokenizes argv into options, flags, subcommands, and positional arguments.
- Command router resolves subcommand trees and default handlers with inherited option scopes.
- Validation layer enforces required arguments, arity, and type coercion rules.

### API Execution Records (Complete)

- std.cli.define(spec): compile CLI command/option specification model.
- std.cli.parse(spec, argv): parse argument vector into typed command context.
- std.cli.dispatch(spec, parsed): execute matched command handler with parsed context.
- std.cli.help(spec, topic): render contextual usage/help output.

### Failure Mode Matrix

- Unknown option or subcommand encountered in strict mode: cli-parse diagnostic.
- Missing required argument/flag value for command: cli-argument diagnostic.
- Type coercion failure for option value conversion: cli-coercion diagnostic.
- Ambiguous command route due to conflicting spec: cli-routing diagnostic.

## std.csv — CSV Parsing & Writing

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.csv parse/emit internals and CSV processing failure behavior are documented.

### Implemented Internal Records

- CSV parser tokenizes delimited records with quote/escape and multiline field handling.
- Schema mapper projects rows into typed columns with header-index and coercion policies.
- Writer pipeline serializes records with configurable delimiter, quoting, and newline strategies.

### API Execution Records (Complete)

- std.csv.parse(text, options): parse CSV text payload into typed row structures.
- std.csv.read(input, options): parse CSV source into row stream or materialized table.
- std.csv.write(rows, output, options): serialize row data to CSV output sink.
- std.csv.stringify(rows, options): serialize in-memory rows into CSV text.
- std.csv.open_reader(input, options): open streaming row reader for large CSV sources.
- std.csv.schema(headers, types): build typed schema mapping for row projection.
- std.csv.validate(row, schema): validate/coerce row against schema contract.

### Failure Mode Matrix

- Malformed quoted field or inconsistent column count: csv-parse diagnostic.
- Header/schema mismatch under strict projection mode: csv-schema diagnostic.
- Type coercion failure for typed column mapping: csv-coercion diagnostic.
- Output sink/write failure during serialization: csv-write diagnostic.

## std.toml — TOML Parsing

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.toml parse/serialize internals and TOML processing failure behavior are documented.

### Implemented Internal Records

- TOML parser builds typed document tree with table/array-of-table semantics.
- Key-path resolver enforces dotted-key precedence and duplicate key conflict rules.
- Serializer emits stable TOML formatting while preserving value-kind fidelity.

### API Execution Records (Complete)

- std.toml.parse(text, options): parse TOML document into typed object tree.
- std.toml.stringify(value, options): serialize typed object to TOML text.
- std.toml.get(doc, keyPath): resolve key path against parsed document tree.
- std.toml.validate(doc, schema): validate parsed TOML object against schema.

### Failure Mode Matrix

- Syntax violation in key/value/table declaration: toml-parse diagnostic.
- Duplicate/conflicting key assignment detected: toml-key-conflict diagnostic.
- Unsupported value type for TOML serialization: toml-serialize diagnostic.
- Schema validation failure under strict mode: toml-schema diagnostic.

## std.yaml — YAML Parsing

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.yaml parse/emit internals and YAML processing failure behavior are documented.

### Implemented Internal Records

- YAML parser handles indentation-sensitive blocks, sequences, mappings, and scalar variants.
- Anchor/alias resolver tracks reference graphs and merge key semantics safely.
- Emitter pipeline outputs canonical or pretty YAML forms with deterministic ordering options.

### API Execution Records (Complete)

- std.yaml.parse(text, options): parse YAML document stream into typed values.
- std.yaml.stringify(value, options): serialize value graph to YAML text.
- std.yaml.loadAll(stream): decode multi-document YAML stream.
- std.yaml.validate(doc, schema): validate YAML value graph against schema.

### Failure Mode Matrix

- Indentation/tokenization error in YAML source: yaml-parse diagnostic.
- Invalid anchor/alias reference cycle or unresolved alias: yaml-reference diagnostic.
- Multi-document boundary parse failure: yaml-stream diagnostic.
- Schema/type mismatch under strict validation: yaml-schema diagnostic.

## std.uuid — UUID Generation

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.uuid generation/parse internals and UUID processing failure behavior are documented.

### Implemented Internal Records

- UUID generator supports random/time/name-based variants with version-specific bit layout rules.
- Parser validates canonical textual/binary forms and normalizes variant/version flags.
- Entropy/time sources are abstracted for deterministic test mode and secure runtime mode.

### API Execution Records (Complete)

- std.uuid.v4(): generate random UUID using secure entropy source.
- std.uuid.v5(namespace, name): generate namespace-based UUID from hash derivation.
- std.uuid.parse(text): parse UUID text into binary/structured representation.
- std.uuid.format(uuid, style): render UUID value to canonical textual style.

### Failure Mode Matrix

- UUID text contains invalid length or character set: uuid-parse diagnostic.
- Unsupported version/variant requested for generator: uuid-version diagnostic.
- Entropy source unavailable in secure mode: uuid-entropy diagnostic.
- Namespace input invalid for name-based UUID generation: uuid-namespace diagnostic.

## std.rand — Random Numbers

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.rand PRNG/entropy internals and random-generation failure behavior are documented.

### Implemented Internal Records

- Random subsystem offers secure and deterministic PRNG engines with pluggable seeds.
- Distribution samplers implement uniform and common shaped distributions over numeric domains.
- Stream API exposes reproducible random sequences with independent generator state handles.

### API Execution Records (Complete)

- std.rand.seed(value): initialize deterministic PRNG state for reproducible sequences.
- std.rand.int(min, max): sample integer from inclusive/exclusive bounds policy.
- std.rand.float(min, max): sample floating value from configured range.
- std.rand.bytes(count): produce random byte buffer from selected RNG engine.

### Failure Mode Matrix

- Invalid sampling bounds or distribution parameters: rand-parameter diagnostic.
- Secure entropy backend unavailable when required: rand-entropy diagnostic.
- PRNG state corruption detected under strict checks: rand-state diagnostic.
- Requested sample count exceeds configured safety budget: rand-budget diagnostic.

## std.hash — Non-Cryptographic Hashing

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.hash algorithm/seed internals and hashing failure behavior are documented.

### Implemented Internal Records

- Hash registry maps algorithm identifiers to non-cryptographic hash implementations and parameters.
- Hasher state machine supports incremental update/finalize streaming semantics.
- Seed policy layer controls deterministic vs randomized seeding for hash-table defense modes.

### API Execution Records (Complete)

- std.hash.compute(algo, data, options): compute hash digest for provided payload.
- std.hash.new(algo, seed): create incremental hasher context instance.
- std.hash.update(ctx, chunk): feed chunk into active incremental hasher state.
- std.hash.finalize(ctx): finalize incremental hasher and return digest value.

### Failure Mode Matrix

- Unknown/disabled hash algorithm requested: hash-algorithm diagnostic.
- Incremental hasher used after finalize in strict mode: hash-state diagnostic.
- Seed value invalid for selected algorithm policy: hash-seed diagnostic.
- Input kind unsupported for configured hash adapter: hash-input diagnostic.

## std.cache — In-Memory Caching

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.cache eviction/storage internals and caching failure behavior are documented.

### Implemented Internal Records

- Cache core tracks key-value entries with TTL/size metadata and access counters.
- Eviction engine supports LRU/LFU/time-based strategies with deterministic tie-break rules.
- Concurrency guards synchronize cache mutation paths and stale-read invalidation behavior.

### API Execution Records (Complete)

- std.cache.new(config): create cache instance with capacity/eviction policy.
- std.cache.get(cache, key): retrieve cached value with hit/miss metadata.
- std.cache.set(cache, key, value, ttl): insert/update cache entry with optional expiry.
- std.cache.invalidate(cache, keyOrPattern): evict one or multiple matching entries.

### Failure Mode Matrix

- Cache capacity/eviction policy misconfiguration: cache-config diagnostic.
- Entry serialization failure for backend persistence mode: cache-encode diagnostic.
- Concurrent mutation conflict in strict locking mode: cache-concurrency diagnostic.
- TTL policy violation or negative expiry under strict mode: cache-ttl diagnostic.

## std.signal — OS Signal Handling

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.signal registration/dispatch internals and signal-handling failure behavior are documented.

### Implemented Internal Records

- Signal registry maps host signal numbers to safe runtime callback dispatch points.
- Delivery queue buffers async signals and coordinates main-loop safe invocation semantics.
- Masking layer applies temporary block/unblock policies around critical sections.

### API Execution Records (Complete)

- std.signal.on(sig, handler): register handler and update signal routing table.
- std.signal.off(sig): remove handler and restore default behavior policy.
- std.signal.mask(set): apply process/thread signal mask update.
- std.signal.poll(): drain pending signal queue into runtime callbacks.

### Failure Mode Matrix

- Attempted registration for unsupported/restricted signal: signal-registration diagnostic.
- Unsafe handler operation detected in async context: signal-safety diagnostic.
- Signal mask operation denied by host runtime policy: signal-mask diagnostic.
- Pending signal queue overflow under sustained burst: signal-queue diagnostic.

## std.http — HTTP Client & Server

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.http request/response pipeline internals and HTTP failure behavior are documented.

### Implemented Internal Records

- HTTP stack implements client and server pipelines with header canonicalization and body streaming.
- Router layer resolves method/path matches with middleware chain composition and short-circuit rules.
- Connection manager supports keep-alive pooling, protocol upgrades, and backpressure-aware writes.

### API Execution Records (Complete)

- std.http.client.request(method, url, req): execute outbound request with redirect/retry policy.
- std.http.server.serve(bind, routes): run HTTP server loop with middleware/router dispatch.
- std.http.parse(raw): parse wire bytes into request/response message structures.
- std.http.encode(message): serialize HTTP message into wire-ready byte stream.

### Failure Mode Matrix

- Malformed request/response framing or headers: http-parse diagnostic.
- Route dispatch misses required handler in strict mode: http-routing diagnostic.
- Connection pool exhaustion or refused upgrade path: http-connection diagnostic.
- Body stream aborted due to backpressure or peer reset: http-stream diagnostic.

## std.ffi — Foreign Function Interface

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.ffi ABI/binding internals and foreign-call failure behavior are documented.

### Implemented Internal Records

- FFI binder maps language signatures to foreign ABI call descriptors and marshalling layouts.
- Library loader resolves shared-object handles with symbol lookup and lifetime reference tracking.
- Safety boundary checks enforce ownership/pinning and unsafe-call policies for pointer values.

### API Execution Records (Complete)

- std.ffi.load(libPath, options): load foreign library and register symbol table.
- std.ffi.bind(lib, signature): bind foreign symbol to callable typed wrapper.
- std.ffi.call(fn, args): invoke bound foreign function with marshalling/unmarshalling.
- std.ffi.release(lib): unload library handle under safe reference constraints.

### Failure Mode Matrix

- Shared library load or symbol resolution fails: ffi-load diagnostic.
- ABI signature mismatch between binding and target symbol: ffi-abi diagnostic.
- Pointer/ownership contract violation in strict safety mode: ffi-safety diagnostic.
- Foreign call returns invalid/error state mapping: ffi-runtime diagnostic.

## Type Aliases

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - alias resolution, expansion rules, and alias internals and failure behavior are documented.

### Implemented Internal Records

- Alias declarations are interned in a canonical alias graph with module-qualified identity keys.
- Expansion engine performs lazy alias expansion with memoization to avoid repeated type inflation.
- Cycle detection tracks alias expansion stack and emits minimal cycle witnesses for diagnostics.

### API Execution Records (Complete)

- alias.define(name, targetType): register alias entry and module visibility metadata.
- alias.resolve(name, scope): lookup alias by lexical/module scope and return canonical alias handle.
- alias.expand(typeExpr): recursively expand aliases to canonical type form with cycle guards.
- alias.checkRecursion(alias): validate acyclic expansion and emit cycle diagnostics when needed.

### Failure Mode Matrix

- Recursive alias cycle detected: alias-cycle diagnostic with expansion chain.
- Alias references inaccessible/private type: visibility diagnostic with owning module context.
- Alias expansion yields forbidden/unsized type in constrained context: type-constraint diagnostic.
- Conflicting duplicate alias declaration in same scope: symbol-collision diagnostic with original span.

## Destructuring Assignment

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - destructuring bind plan, assignment semantics, and destructuring internals and failure behavior are documented.

### Implemented Internal Records

- Pattern planner lowers tuple/struct/list destructures into deterministic field/index extraction steps.
- Binding mode analysis marks each pattern slot as move, copy, immutable borrow, or mutable borrow.
- Rest bindings and nested patterns are normalized before assignment lowering to maintain evaluation order.

### API Execution Records (Complete)

- destructure.plan(pattern, sourceType): compile pattern into ordered extraction operations.
- destructure.bind(pattern, value): execute extraction and create scoped bindings with ownership modes.
- destructure.assign(pattern, value): apply destructuring assignment updates to existing l-values.
- destructure.rest(pattern, value): compute residual collection for rest captures under policy rules.

### Failure Mode Matrix

- Pattern arity/shape mismatch with source value: destructure-shape diagnostic.
- Illegal move/borrow in nested destructure slot: ownership diagnostic with slot path.
- Assignment target immutability conflict in destructuring assignment: mutability diagnostic.
- Duplicate binding names in same destructure scope: binding-collision diagnostic with name source.

## if let / while let

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - pattern-guarded branch/loop lowering and if/while-let internals and failure behavior are documented.

### Implemented Internal Records

- if-let/while-let constructs lower to match-like decision blocks with pattern success/failure edges.
- Pattern bindings are scoped to success branches/iterations with precise lifetime boundaries.
- while-let loops reuse pattern test blocks and maintain loop-carried variable state consistency.

### API Execution Records (Complete)

- iflet.lower(node): lower if-let syntax into pattern-test + success/fallback CFG blocks.
- whilelet.lower(node): lower while-let into loop header pattern-test and body/exit edges.
- pattern.bind(scrutinee, pattern): bind successful pattern captures into branch-local scope.
- pattern.test(scrutinee, pattern): evaluate structural match predicate for control-flow branching.

### Failure Mode Matrix

- Pattern incompatible with scrutinee type in if/while-let: pattern-type diagnostic.
- Non-exhaustive guard assumptions in transformed control flow: control-flow diagnostic.
- Illegal capture lifetime escape from if-let success scope: lifetime diagnostic.
- Mutating scrutinee during while-let test/body conflict: borrow/mutation diagnostic.

## Labeled Loops

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - label resolution, break/continue targeting, and labeled-loop internals and failure behavior are documented.

### Implemented Internal Records

- Label resolver maintains stack of active loop labels with lexical depth metadata.
- Break/continue nodes lower to explicit target edges resolved against active label stack.
- Nested loop transformations preserve label identity through optimization passes.

### API Execution Records (Complete)

- label.register(loopNode, name): push loop label into active resolution stack.
- break.resolve(label): map break statement to target loop exit block.
- continue.resolve(label): map continue statement to target loop latch/header block.
- label.lower(cfg): emit resolved labeled control-flow edges in MIR.

### Failure Mode Matrix

- Unknown loop label reference: label-resolution diagnostic with in-scope label suggestions.
- Continue to non-loop label or invalid target: control-target diagnostic.
- Duplicate label in same lexical scope: label-collision diagnostic.
- Labeled edge invalidated by transform pass: CFG consistency diagnostic requiring rebuild.

## Union Types

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - union normalization, narrowing, and union-type internals and failure behavior are documented.

### Implemented Internal Records

- Union canonicalizer flattens nested unions, removes duplicates, and orders variants deterministically.
- Narrowing engine refines union members using guards, pattern tests, and flow-sensitive predicates.
- Runtime tagging strategy is selected when backend requires explicit discriminants for erased unions.

### API Execution Records (Complete)

- union.normalize(types): flatten and canonicalize union member set.
- union.narrow(value, predicate): refine active member set by control-flow predicate.
- union.match(value, arms): dispatch union value to compatible arm based on narrowed member set.
- union.tag(value): query runtime tag/discriminant where representation requires tagging.

### Failure Mode Matrix

- Incompatible union member combination under language constraints: union-coherence diagnostic.
- Ambiguous member resolution after narrowing: union-ambiguity diagnostic with candidate members.
- Missing arm for reachable union member in match: union-exhaustiveness diagnostic.
- Unsafe cast between incompatible unions: cast-safety diagnostic with source/target union sets.

## The `never` Type

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - bottom-type propagation, flow pruning, and never-type internals and failure behavior are documented.

### Implemented Internal Records

- Bottom-type inference propagates from non-returning expressions through control-flow merge points.
- CFG simplifier prunes unreachable blocks introduced by never-typed paths.
- Coercion rules allow never to flow into supertype contexts while preserving exhaustiveness guarantees.

### API Execution Records (Complete)

- never.infer(node): mark expression as never when evaluation cannot return normally.
- never.propagate(cfg): propagate bottom-type facts through dominance and merge analysis.
- never.coerce(targetType): coerce never-typed expression into required contextual type.
- never.prune(cfg): remove unreachable blocks and update diagnostics metadata.

### Failure Mode Matrix

- Expression expected to produce value but inferred never in non-diverging context: type-flow diagnostic.
- Unreachable code emitted after never path in strict mode: unreachable-code diagnostic.
- Invalid assumption that never path returns in effect analysis: effect-flow diagnostic.
- Backend mismatch in never-path pruning metadata: CFG/pruning consistency diagnostic.

## Effects System

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - Effects System lowering/runtime internals and failure behavior are documented.

### Implemented Internal Records

- Effect annotations stored in compiled function metadata; retrievable via `ct_get_effects` at compile time.
- Effect verification: compiler checks consistency when `--warn effects` enabled; violations are warnings or errors.
- Effect inheritance: function calling side-effecting function must declare at least the called function's effects.
- `pure func` is shorthand for `[effects: none]` — guaranteed deterministic.

### API Execution Records (Complete)

- effects.annotate(func_name, effect_list): attach effect list [io, net, none, ...] to function metadata.
- effects.check(caller, callee): verify caller's effects are superset of callee's effects -> warn/error if not.
- effects.pure_verify(func_name): verify function has no side effects (no io, net, mutation) -> compile error if violated.
- effects.ct_get(func_name): retrieve effect list at compile time -> list of effect strings.
- effects.infer(func_body): walk function body -> collect effects from all called functions -> compute minimum effect set.
- effects.cfg_conditional(feature, func_variants): select function variant based on compile-time feature flag.

### Failure Mode Matrix

- Pure function calling impure function: compile warning/error with --warn effects.
- Effect annotation missing but function has side effects: no warning unless --warn effects enabled.
- Custom effect label misspelled: false negative (custom effects not validated).
- Effect inheritance violation in deep call chain: requires transitive effect propagation check.

## The `Default` Trait

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - default trait resolution/derivation and default-trait internals and failure behavior are documented.

### Implemented Internal Records

- Default trait solver resolves explicit impls, derive-generated impls, and intrinsic defaults by precedence.
- Derive pipeline synthesizes field-wise defaults while preserving visibility and generic bound requirements.
- Monomorphized default constructors are cached by concrete type substitution keys.

### API Execution Records (Complete)

- default.resolve(type): locate applicable default provider (impl/derive/intrinsic) for type.
- default.derive(typeDef): synthesize default implementation from field/member defaults.
- default.call(type): construct value using resolved default constructor path.
- default.validate(type): verify trait bounds and field default availability before call lowering.

### Failure Mode Matrix

- Missing default provider for required type: trait-resolution diagnostic with missing impl details.
- Derive default blocked by non-default field/member: derive diagnostic with offending field path.
- Generic bound unsatisfied for default instantiation: bound diagnostic with required trait set.
- Ambiguous default provider candidates: coherence diagnostic with competing impl sources.

## std.audio — Audio Playback & Recording

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.audio stream/device internals and audio-runtime failure behavior are documented.

### Implemented Internal Records

- Audio engine manages playback/capture streams with sample format and channel negotiation.
- Mixer pipeline combines sources with gain/pan/effect stages and timing synchronization.
- Device adapter selects host backend and tracks hotplug/default-device changes.

### API Execution Records (Complete)

- std.audio.play(source, options): start playback stream from audio source.
- std.audio.record(device, options): begin capture stream and expose sample buffers.
- std.audio.mixer.configure(mixer, graph): configure mix graph and routing controls.
- std.audio.stop(handle): stop active playback/capture stream and release resources.

### Failure Mode Matrix

- Audio device unavailable or format negotiation fails: audio-device diagnostic.
- Stream underrun/overrun detected beyond tolerance: audio-stream diagnostic.
- Effect/mixer graph configuration invalid: audio-mixer diagnostic.
- Permission denied for capture on protected platform: audio-permission diagnostic.

## std.video — Video Processing

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.video decode/encode internals and video-processing failure behavior are documented.

### Implemented Internal Records

- Video pipeline handles demux/decode/encode stages with timestamp and frame-order control.
- Frame processing layer supports resize/convert/filter transforms on decoded frame buffers.
- Codec backend adapter negotiates hardware/software acceleration paths.

### API Execution Records (Complete)

- std.video.open(source, options): open media source and initialize decode session.
- std.video.readFrame(session): decode next frame with timing metadata.
- std.video.encode(frames, target, options): encode frame stream into output container.
- std.video.transform(frame, ops): apply frame-level processing operations.

### Failure Mode Matrix

- Container demux or stream selection fails: video-demux diagnostic.
- Codec decode/encode error for selected format profile: video-codec diagnostic.
- Frame timestamp/order inconsistency under strict mode: video-timing diagnostic.
- Hardware acceleration backend unavailable for requested path: video-backend diagnostic.

## std.pdf — PDF Generation & Reading

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.pdf document/render internals and PDF processing failure behavior are documented.

### Implemented Internal Records

- PDF parser reads object graph, xref tables, and incremental update sections.
- Generator pipeline builds pages, fonts, images, and metadata into valid PDF structure.
- Rendering adapter resolves page content streams to raster/vector outputs when required.

### API Execution Records (Complete)

- std.pdf.open(source, options): load PDF document and parse object/xref graph.
- std.pdf.page.render(doc, index, options): render page to target bitmap/vector surface.
- std.pdf.create(spec): build new PDF document model from page/content descriptors.
- std.pdf.save(doc, target, options): serialize PDF document to output destination.

### Failure Mode Matrix

- Malformed xref/object stream in input document: pdf-parse diagnostic.
- Font/resource embedding failure during generation: pdf-resource diagnostic.
- Page render backend cannot satisfy requested output mode: pdf-render diagnostic.
- Write/serialization failure for output target: pdf-save diagnostic.

## std.excel — Excel / XLSX Files

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.excel workbook/sheet internals and XLSX processing failure behavior are documented.

### Implemented Internal Records

- Workbook model tracks sheets, cell stores, styles, formulas, and shared strings.
- XLSX adapter maps workbook model to/from zipped XML package parts.
- Formula evaluator performs dependency-aware recalculation with cached cell values.

### API Execution Records (Complete)

- std.excel.open(source, options): load workbook from XLSX source package.
- std.excel.sheet.get(workbook, name): resolve worksheet handle by name/index.
- std.excel.cell.set(sheet, ref, value): assign typed cell value with style/formula metadata.
- std.excel.save(workbook, target): serialize workbook model back to XLSX package.

### Failure Mode Matrix

- XLSX package part missing/corrupt during load: excel-package diagnostic.
- Cell reference/style assignment invalid for worksheet bounds: excel-cell diagnostic.
- Formula evaluation dependency cycle detected: excel-formula diagnostic.
- Workbook serialization fails relationship/package checks: excel-save diagnostic.

## std.jwt — JSON Web Tokens

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.jwt token/signature internals and JWT validation failure behavior are documented.

### Implemented Internal Records

- JWT codec parses/serializes JOSE header, claims payload, and signature segments.
- Signature verifier dispatches algorithm-specific verification over provided key material.
- Claim validator enforces exp/nbf/aud/iss and custom claim policy checks.

### API Execution Records (Complete)

- std.jwt.encode(claims, key, options): sign and encode claims into JWT string.
- std.jwt.decode(token, options): parse JWT into header/claims/signature components.
- std.jwt.verify(token, key, options): verify signature and validate claim policies.
- std.jwt.claims.get(token, key): retrieve specific claim value from decoded token.

### Failure Mode Matrix

- Token segment/base64 decode malformed: jwt-parse diagnostic.
- Unsupported/forbidden signing algorithm requested: jwt-algorithm diagnostic.
- Signature verification failure with supplied key: jwt-signature diagnostic.
- Required claim validation fails policy checks: jwt-claims diagnostic.

## std.oauth2 — OAuth 2.0

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.oauth2 flow/token internals and OAuth2 protocol failure behavior are documented.

### Implemented Internal Records

- OAuth2 client manages authorization-code/device/client-credentials flow state machines.
- Token store handles access/refresh token lifecycle with expiry and scope metadata.
- HTTP integration secures redirect/state/PKCE handling for interactive flows.

### API Execution Records (Complete)

- std.oauth2.authorize(config, state): initiate authorization flow and produce redirect URL.
- std.oauth2.exchange(code, verifier, config): exchange auth code for token set.
- std.oauth2.refresh(token, config): refresh access token using refresh grant path.
- std.oauth2.revoke(token, config): revoke issued token at provider endpoint.

### Failure Mode Matrix

- Authorization response state/PKCE validation fails: oauth2-state diagnostic.
- Token endpoint returns protocol or provider error payload: oauth2-token diagnostic.
- Refresh token invalid/expired under provider policy: oauth2-refresh diagnostic.
- Scope mismatch between requested and granted token: oauth2-scope diagnostic.

## std.i18n — Internationalization

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.i18n locale/resource internals and internationalization failure behavior are documented.

### Implemented Internal Records

- Locale resolver selects language/region bundles using fallback chain policies.
- Message formatter applies pluralization/gender rules and parameter interpolation.
- Date/number/currency adapters integrate locale-specific formatting conventions.

### API Execution Records (Complete)

- std.i18n.locale.set(tag): set active locale and load associated resource bundles.
- std.i18n.t(key, params): resolve localized message and apply interpolation rules.
- std.i18n.format.number(value, options): format numeric value per active locale.
- std.i18n.bundle.load(source): load/merge translation resource bundle data.

### Failure Mode Matrix

- Locale bundle missing and fallback unavailable: i18n-locale diagnostic.
- Translation key unresolved in strict localization mode: i18n-key diagnostic.
- Message parameter mismatch for template placeholders: i18n-template diagnostic.
- Bundle parse/merge conflict during load: i18n-bundle diagnostic.

## std.watch — Filesystem Watching

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.watch filesystem-event pipeline internals and watch-service failure behavior are documented.

### Implemented Internal Records

- Watch backend selects platform notifier (inotify/FSEvents/ReadDirectoryChanges) at initialization.
- Event coalescer merges duplicate bursts and normalizes path/change-kind metadata.
- Recursive watch planner tracks dynamic subdirectory additions under policy constraints.

### API Execution Records (Complete)

- std.watch.start(path, options): begin watch subscription with recursive/filter settings.
- std.watch.next(watcher): retrieve next normalized filesystem event batch.
- std.watch.stop(watcher): stop watch backend and release OS notification handles.
- std.watch.snapshot(watcher): emit current watch-state diagnostics and counters.

### Failure Mode Matrix

- Watch target path missing or inaccessible: watch-path diagnostic.
- Backend notification handle quota exceeded: watch-resource diagnostic.
- Event stream overflow dropped events in strict mode: watch-overflow diagnostic.
- Recursive subscription denied by platform capability: watch-capability diagnostic.

## std.grpc — gRPC Client & Server

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.grpc channel/stub internals and gRPC transport failure behavior are documented.

### Implemented Internal Records

- gRPC runtime binds protobuf descriptors to generated client/server stubs and method dispatch tables.
- HTTP/2 transport layer manages stream multiplexing, metadata frames, and flow-control windows.
- Interceptor chain applies auth/telemetry/retry behaviors around unary and streaming calls.

### API Execution Records (Complete)

- std.grpc.channel.open(target, options): create gRPC channel with TLS and retry policy setup.
- std.grpc.client.call(stub, method, request): execute unary RPC and decode typed response.
- std.grpc.server.bind(serviceDefs, handlers): register service handlers and start RPC serving.
- std.grpc.stream.exchange(stream, events): drive bidirectional stream send/receive lifecycle.

### Failure Mode Matrix

- Method descriptor missing or handler mismatch: grpc-dispatch diagnostic.
- Transport stream reset/flow-control violation: grpc-transport diagnostic.
- Serialization/deserialization mismatch with protobuf schema: grpc-codec diagnostic.
- Deadline exceeded or retry policy exhausted: grpc-deadline diagnostic.

## std.mqtt — MQTT Messaging

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.mqtt session/topic internals and MQTT failure behavior are documented.

### Implemented Internal Records

- MQTT client runtime manages connect/session keepalive and broker capability negotiation.
- Topic router tracks subscriptions, wildcard matching, and QoS-specific delivery semantics.
- Packet codec serializes CONNECT/PUBLISH/SUBSCRIBE flows with stateful acknowledgment handling.

### API Execution Records (Complete)

- std.mqtt.connect(broker, options): establish MQTT session and negotiate protocol settings.
- std.mqtt.publish(client, topic, payload, qos): enqueue publish with QoS/retain semantics.
- std.mqtt.subscribe(client, topics, qos): register topic subscriptions and callback dispatch.
- std.mqtt.poll(client): process incoming packets and heartbeat/ack state transitions.

### Failure Mode Matrix

- Broker rejects connect/auth handshake: mqtt-connect diagnostic.
- Invalid topic filter or wildcard usage: mqtt-topic diagnostic.
- QoS acknowledgment sequence violation: mqtt-qos-state diagnostic.
- Keepalive timeout without broker heartbeat: mqtt-session-timeout diagnostic.

## std.embed — Compile-Time File Embedding

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.embed asset-pack internals and compile-time embedding failure behavior are documented.

### Implemented Internal Records

- Embed planner resolves file globs and emits compile-time asset manifests.
- Payload packer stores embedded bytes with optional compression and checksum metadata.
- Linker integration exposes embedded assets via immutable runtime access tables.

### API Execution Records (Complete)

- std.embed.files(patterns, options): resolve and embed files during compile stage.
- std.embed.get(id): retrieve embedded asset bytes by manifest identifier.
- std.embed.list(namespace): enumerate embedded assets within namespace scope.
- std.embed.meta(id): return embedded asset metadata (size/hash/origin).

### Failure Mode Matrix

- Embed source path missing or excluded by policy: embed-source diagnostic.
- Asset exceeds embed size/budget constraints: embed-budget diagnostic.
- Manifest identifier collision during build: embed-manifest diagnostic.
- Runtime lookup references unknown embedded asset ID: embed-lookup diagnostic.

## std.template — Text Templating

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.template parse/render internals and template-execution failure behavior are documented.

### Implemented Internal Records

- Template parser builds AST for text segments, expressions, conditionals, and loops.
- Render engine evaluates template AST against context objects with escaping policies.
- Cache layer stores compiled templates keyed by source identity and render options.

### API Execution Records (Complete)

- std.template.compile(source, options): parse/compile template into executable plan.
- std.template.render(tpl, context, options): render compiled template with context data.
- std.template.register(name, tpl): register reusable named template in registry.
- std.template.escape(mode, value): apply escape policy helper to runtime value.

### Failure Mode Matrix

- Template syntax error during parse/compile: template-parse diagnostic.
- Missing context variable in strict rendering mode: template-context diagnostic.
- Recursive include/partial cycle exceeds depth guard: template-recursion diagnostic.
- Escape mode unsupported for target output backend: template-escape diagnostic.

## std.multipart — Multipart Form Data

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.multipart boundary/part internals and multipart parsing failure behavior are documented.

### Implemented Internal Records

- Multipart parser scans boundaries and streams part headers/body segments incrementally.
- Part decoder handles file fields, text fields, and content-type specific decoding paths.
- Builder pipeline emits multipart payloads with deterministic boundary and header formatting.

### API Execution Records (Complete)

- std.multipart.parse(stream, boundary, options): decode multipart stream into parts.
- std.multipart.build(parts, options): construct multipart body and boundary metadata.
- std.multipart.part.read(part, sink): stream part payload to target sink.
- std.multipart.part.meta(part): retrieve parsed part headers and size metadata.

### Failure Mode Matrix

- Boundary marker malformed or missing in stream: multipart-boundary diagnostic.
- Part header parsing fails RFC constraint checks: multipart-header diagnostic.
- Part payload exceeds configured size limits: multipart-size diagnostic.
- Stream terminated before final boundary completion: multipart-stream diagnostic.

## std.ssh — SSH Client & SFTP

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.ssh channel/auth internals and SSH/SFTP failure behavior are documented.

### Implemented Internal Records

- SSH transport negotiates protocol versions, cipher suites, and key-exchange mechanisms.
- Session manager multiplexes exec/shell/SFTP channels over authenticated transport tunnels.
- Credential layer supports key-based and password-based auth with agent/known-host validation.

### API Execution Records (Complete)

- std.ssh.connect(target, credentials, options): establish authenticated SSH session tunnel.
- std.ssh.exec(session, command): execute remote command and collect streamed output.
- std.ssh.sftp.open(session): create SFTP channel for remote file operations.
- std.ssh.sftp.transfer(channel, spec): perform upload/download with resume and integrity checks.

### Failure Mode Matrix

- Host key validation fails trust policy: ssh-host-key diagnostic.
- Authentication rejected by remote endpoint: ssh-auth diagnostic.
- Channel open/multiplex operation exceeds limits: ssh-channel diagnostic.
- SFTP transfer interrupted or integrity check fails: sftp-transfer diagnostic.

## std.qr — QR Code Generation

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.qr encode/matrix internals and QR-generation failure behavior are documented.

### Implemented Internal Records

- QR encoder selects version/error-correction level based on payload size and constraints.
- Matrix builder places data, timing, and alignment patterns then applies mask scoring.
- Output renderer converts matrix to raster/vector/text representations.

### API Execution Records (Complete)

- std.qr.encode(data, options): encode payload into QR matrix representation.
- std.qr.render(matrix, format, options): render QR matrix to output format.
- std.qr.version.select(data, ecc): select minimal QR version for payload/ecc settings.
- std.qr.decode(image, options): decode QR image input to payload when supported.

### Failure Mode Matrix

- Payload exceeds QR capacity for selected version/ecc: qr-capacity diagnostic.
- Invalid mask/pattern placement under strict validation: qr-matrix diagnostic.
- Render target format unsupported for QR output: qr-render diagnostic.
- Decode fails due to unreadable/corrupt symbol data: qr-decode diagnostic.

## std.markdown — Markdown Parsing & Rendering

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.markdown parse/render internals and markdown-processing failure behavior are documented.

### Implemented Internal Records

- Markdown parser constructs block/inline AST with extension flag controls.
- Renderer maps AST to HTML/text/other targets with sanitization and escaping policies.
- Link/reference resolver handles footnotes, anchors, and reference-definition tables.

### API Execution Records (Complete)

- std.markdown.parse(text, options): parse markdown source into AST representation.
- std.markdown.render(astOrText, target, options): render markdown to chosen output target.
- std.markdown.sanitize(html, policy): sanitize rendered HTML by policy profile.
- std.markdown.toc(ast, options): generate table-of-contents structure from heading nodes.

### Failure Mode Matrix

- Markdown tokenization/parse error in strict grammar mode: markdown-parse diagnostic.
- Renderer target unsupported for selected extension set: markdown-render diagnostic.
- Sanitization policy violation for unsafe content: markdown-sanitize diagnostic.
- Link/reference resolution missing target definition: markdown-reference diagnostic.

## std.archive — ZIP & TAR Archives

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.archive container/entry internals and archive-processing failure behavior are documented.

### Implemented Internal Records

- Archive adapter supports ZIP/TAR container parsing and entry metadata projection.
- Entry stream layer performs lazy extraction with path normalization and safety checks.
- Writer pipeline builds archives with compression mode and file attribute policies.

### API Execution Records (Complete)

- std.archive.open(source, options): open archive container and index entry catalog.
- std.archive.extract(archive, target, options): extract archive entries to filesystem target.
- std.archive.create(entries, target, options): create archive container from entry set.
- std.archive.list(archive): enumerate archive entries with metadata.

### Failure Mode Matrix

- Archive header/index corrupt or unsupported format: archive-parse diagnostic.
- Entry path traversal or unsafe extraction target detected: archive-safety diagnostic.
- Compression/decompression operation fails for entry stream: archive-codec diagnostic.
- File attribute/permission restoration fails on extract: archive-restore diagnostic.

## Weak References

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - weak-reference lifecycle/upgrade semantics and weak-reference internals and failure behavior are documented.

### Implemented Internal Records

- Weak references share control blocks with strong references while not contributing to strong liveness count.
- Upgrade operation atomically checks strong count and returns strong handle only if object remains alive.
- Control block reclamation waits for both strong and weak counts to reach terminal release conditions.

### API Execution Records (Complete)

- weak.new(strongRef): create weak handle from strong reference control block.
- weak.upgrade(weakRef): attempt strong handle reconstruction if object is still alive.
- weak.counts(ref): query strong/weak count snapshot for diagnostics or introspection.
- weak.release(weakRef): decrement weak count and trigger control-block cleanup when eligible.

### Failure Mode Matrix

- Upgrade on already-dropped allocation: weak-upgrade returns none with dropped-object metadata.
- Control block corruption/count underflow detected: runtime memory-safety diagnostic.
- Cross-thread weak upgrade without required sync policy: thread-safety diagnostic.
- Double release of weak handle in strict mode: ownership diagnostic with handle provenance.

## Newtype Wrappers

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - newtype wrapping/coercion semantics and newtype internals and failure behavior are documented.

### Implemented Internal Records

- Newtype wrappers preserve runtime representation by default while creating distinct semantic type identity.
- Coercion rules enforce explicit unwrap/wrap boundaries unless transparent policy is declared.
- Trait forwarding for newtypes is opt-in via derive/impl generation and coherence checks.

### API Execution Records (Complete)

- newtype.wrap(value): construct newtype wrapper from underlying representation.
- newtype.unwrap(wrapper): extract inner value respecting ownership/borrow mode.
- newtype.coerce(wrapper, target): apply explicit conversion path between compatible wrappers.
- newtype.validate(def): enforce transparency/trait/coherence constraints for wrapper definition.

### Failure Mode Matrix

- Illegal implicit coercion between newtype and inner type: coercion diagnostic with explicit-conversion hint.
- Wrapper transparency violation under ABI/FFI constraint: representation diagnostic.
- Trait forwarding conflict from overlapping derives/impls: coherence diagnostic.
- Unwrap move from borrowed wrapper: ownership diagnostic with borrow origin trace.

## The `@derive` Decorator

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - derive expansion generation and derive internals and failure behavior are documented.

### Implemented Internal Records

- Derive planner expands target traits into synthesized impl AST fragments with hygiene-safe symbols.
- Expansion pipeline validates trait prerequisites and field/member capabilities before code emission.
- Generated impl metadata tracks origin trait + expansion span for diagnostics and tooling.

### API Execution Records (Complete)

- derive.scan(typeDef, traits): validate requested derive traits against target type shape.
- derive.expand(typeDef, traits): generate impl AST fragments for each requested derive trait.
- derive.hygiene(ast): apply scoped symbol remapping to avoid capture collisions.
- derive.emit(ast): inject generated impls into semantic pipeline and coherence registry.

### Failure Mode Matrix

- Unsupported derive trait for target shape: derive-compatibility diagnostic.
- Missing prerequisite bound for derive generation: bound diagnostic with required trait list.
- Generated impl conflicts with user-defined impl: coherence diagnostic with impl origin trace.
- Hygiene collision in generated symbols: macro/derive hygiene diagnostic with symbol mapping.

## Built-in Trait Catalog

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - built-in trait registry resolution and trait-catalog internals and failure behavior are documented.

### Implemented Internal Records

- Built-in trait catalog stores canonical trait descriptors, marker flags, and solver integration metadata.
- Trait lookup path resolves aliases/deprecations to canonical trait IDs with version compatibility checks.
- Catalog metadata is exported to IDE/docs tooling for completion and diagnostics enrichment.

### API Execution Records (Complete)

- trait.catalog.resolve(name): resolve trait name/alias to canonical built-in trait descriptor.
- trait.catalog.list(filters): enumerate traits by marker/category/capability criteria.
- trait.catalog.check(type, trait): evaluate whether type satisfies built-in trait contract.
- trait.catalog.meta(trait): retrieve trait metadata for diagnostics and tooling output.

### Failure Mode Matrix

- Unknown/deprecated trait name in strict mode: catalog resolution diagnostic with replacement hint.
- Trait contract check requested for unsupported type category: trait-check diagnostic.
- Catalog version mismatch with compiler runtime: compatibility diagnostic requiring update.
- Conflicting alias mapping in trait catalog metadata: catalog-coherence diagnostic.

## Numeric Casting

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - numeric-cast conversion rules and cast internals and failure behavior are documented.

### Implemented Internal Records

- Numeric cast engine classifies conversions as widening, narrowing, signedness-shift, or float/integer boundary casts.
- Lowering inserts checked/unchecked cast ops based on compile mode and explicit cast annotations.
- Constant-folder performs compile-time cast evaluation with overflow/precision-loss tagging for diagnostics.

### API Execution Records (Complete)

- cast.classify(fromType, toType): determine cast category and safety profile.
- cast.lower(expr, toType, mode): emit checked/unchecked runtime cast IR instruction.
- cast.constEval(value, fromType, toType): evaluate cast at compile-time when operands are constant.
- cast.report(meta): emit precision-loss/overflow notes according to warning policy.

### Failure Mode Matrix

- Narrowing cast exceeds destination range in checked mode: numeric-overflow diagnostic.
- Float-to-int cast on NaN/inf in strict mode: invalid-cast diagnostic.
- Unsupported cast between incompatible categories: type-conversion diagnostic.
- Constant cast precision-loss forbidden by policy: compile-time cast diagnostic.

## Raw and Byte Strings

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - raw/byte-string tokenization semantics and string-literal internals and failure behavior are documented.

### Implemented Internal Records

- Lexer recognizes raw string delimiters with variable fence width and disables escape processing for raw payloads.
- Byte-string parser validates literal bytes against encoding policy and emits byte-buffer constants.
- Intern pool stores canonical literal bodies plus kind metadata (text/raw/byte) for downstream codegen.

### API Execution Records (Complete)

- string.lexRaw(token): parse raw literal body using delimiter fence metadata.
- string.lexByte(token): parse byte literal payload into byte array constant representation.
- string.intern(value, kind): intern literal with literal-kind tag for semantic consumers.
- string.lowerLiteral(node): lower literal node to immutable runtime/constant representation.

### Failure Mode Matrix

- Unterminated raw string fence: lexer diagnostic with opening-delimiter span.
- Invalid byte escape/value in byte string: literal-byte diagnostic.
- Non-ASCII byte string content under strict byte mode: encoding-policy diagnostic.
- Literal size exceeds configured constant budget: literal-size diagnostic.

## Struct Update Syntax

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - struct-update field merge semantics and struct-update internals and failure behavior are documented.

### Implemented Internal Records

- Struct-update lowering builds new aggregate by overriding explicit fields then sourcing remaining fields from base value.
- Move/borrow analysis tracks which base fields are consumed vs preserved after update expansion.
- Generic struct updates validate field set coherence against monomorphized layout descriptors.

### API Execution Records (Complete)

- struct.update.expand(base, overrides): synthesize full field initialization plan from base + overrides.
- struct.update.validate(type, overrides): enforce field existence/uniqueness and initialization completeness.
- struct.update.borrowCheck(plan): evaluate ownership impact for moved/borrowed base fields.
- struct.update.lower(plan): emit aggregate-construction IR sequence.

### Failure Mode Matrix

- Override references unknown field name: struct-field diagnostic.
- Duplicate override for same field: struct-update duplicate-field diagnostic.
- Base value unusable due to ownership state: borrow/ownership diagnostic.
- Non-exhaustive update after applying base + overrides: initialization diagnostic.

## Bitfield Structs

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - bitfield packing/access semantics and bitfield internals and failure behavior are documented.

### Implemented Internal Records

- Bitfield layout engine assigns bit offsets/widths under alignment and endian policy constraints.
- Accessor generation emits mask/shift sequences for get/set operations with optional bounds checking.
- ABI layer exports packed layout metadata for FFI and serialization compatibility checks.

### API Execution Records (Complete)

- bitfield.layout(structDef): compute packed offsets, widths, and storage-unit mapping.
- bitfield.get(value, field): extract field bits via mask/shift logic.
- bitfield.set(value, field, newBits): write field bits with range validation and merge semantics.
- bitfield.abiCheck(layout, targetAbi): verify target ABI compatibility of packed representation.

### Failure Mode Matrix

- Field width/offset exceeds storage unit capacity: bitfield-layout diagnostic.
- Assigned value does not fit field width in checked mode: bitfield-range diagnostic.
- Overlapping fields without explicit overlap allowance: bitfield-overlap diagnostic.
- Packed layout incompatible with target ABI/endianness policy: bitfield-abi diagnostic.

## Block Expressions

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - block-expression value/lifetime semantics and block-expression internals and failure behavior are documented.

### Implemented Internal Records

- Block expression lowering creates scoped symbol frames and returns terminal expression value when present.
- Type inference unifies block terminal expression with expected context type, including unit fallback rules.
- Lifetime tracker computes borrows introduced/ended inside block boundaries for borrow-check integration.

### API Execution Records (Complete)

- block.scope.enter(parentScope): allocate lexical frame for block locals and temporaries.
- block.typeInfer(blockAst, expectedType): infer block result type from terminal expression/control exits.
- block.lower(blockAst): emit IR for statement sequence + terminal value projection.
- block.scope.exit(frame): release scoped symbols and finalize borrow/liveness summaries.

### Failure Mode Matrix

- Terminal expression type mismatches expected context: type-unification diagnostic.
- Block path missing required terminal value in value context: missing-value diagnostic.
- Borrow escapes block lifetime illegally: lifetime/borrow diagnostic.
- Unreachable terminal expression after diverging control flow: control-flow diagnostic.

## Do Blocks

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - do-block sequencing/lowering semantics and do-block internals and failure behavior are documented.

### Implemented Internal Records

- Do-block desugaring expands effectful step sequencing into explicit bind/continuation form.
- Effect context propagation carries ambient capability set through each do-step boundary.
- Optimizer fuses trivial binds and preserves source-step mapping for debugger traces.

### API Execution Records (Complete)

- do.parse(blockAst): classify do steps, bindings, and terminal yield expression.
- do.desugar(steps, effectCtx): transform do sequence into continuation/bind IR structure.
- do.optimize(ir): collapse no-op binds and inline single-use continuation steps.
- do.validate(effectCtx): enforce effect capability requirements for each step.

### Failure Mode Matrix

- Step produces incompatible effect/value for subsequent bind: do-sequence type/effect diagnostic.
- Missing ambient effect capability for operation in do block: effect-capability diagnostic.
- Invalid terminal yield in required-value context: do-yield diagnostic.
- Desugaring introduces recursion/cycle beyond optimizer limits: do-lowering diagnostic.

## Extension Methods

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - extension parsing, scoping, resolution, and collision detection documented.

### Implemented Internal Records

- Parser recognizes `extend Type { ... }` blocks as ExtensionDecl AST nodes containing method and static function declarations.
- Extensions are registered in a per-module extension registry, keyed by (target_type, method_name). Visibility follows `pub` annotations — non-pub extensions are module-private.
- Method resolution for dot-call `val.method()` first checks the type's own methods, then searches imported extension registries. Extensions never shadow existing methods.
- Static extensions (`static func`) are resolved through `Type.method()` syntax with the same priority rules.
- Generic extensions (`extend list<T>`) carry type parameters and optional `where` bounds; the solver applies bounds during method resolution.

### API Execution Records (Complete)

- extend.parse(tokens): recognize `extend` type_expr `{` method_decls `}` -> emit ExtensionDecl with target type, methods, and visibility.
- extend.register(decl, module): validate target type is resolvable -> register (type, name) pairs in module extension registry -> check for duplicates within module.
- extend.resolve(call_site, type, method_name, imported_modules): search type's own methods -> search extension registries of imported modules -> return resolved method or error.
- extend.typecheck(method, target_type, env): bind `self` parameter to target type -> typecheck body in extended environment -> verify trait bounds if generic.
- extend.lower(method): emit as a free function with first parameter being the target type -> rewrite call sites to pass receiver as first argument.
- extend.collision_check(type, name, all_imports): verify at most one extension defines `name` for `type` across all imported modules -> emit collision diagnostic if violated.

### Failure Mode Matrix

- Extension method name collides with existing type method: compile error "extension cannot shadow existing method 'name' on type T".
- Two imported extensions define same method for same type: compile error "ambiguous extension method 'name' for type T — defined in module A and module B".
- Extension attempts to add stored field: parse error "extensions may only contain methods and static functions".
- Extension on unresolvable type: compile error "cannot extend unknown type T".
- Generic extension with unsatisfied bounds at call site: type diagnostic with unmet trait bound details.

## Trait Coherence and Orphans

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - trait-coherence orphan checks and coherence internals and failure behavior are documented.

### Implemented Internal Records

- Coherence solver indexes impls by trait+type head and applies overlap detection over normalized forms.
- Orphan checker enforces locality rules for trait/type ownership before impl admission.
- Specialization-aware pass validates fallback/child impl ordering constraints where specialization is enabled.

### API Execution Records (Complete)

- coherence.register(implDef): normalize impl signature and add candidate to coherence index.
- coherence.checkOverlap(implDef): detect conflicting impl regions against existing registry entries.
- coherence.checkOrphan(implDef, crateCtx): verify orphan/locality constraints.
- coherence.finalize(): freeze coherence index for downstream monomorphization and trait solving.

### Failure Mode Matrix

- Overlapping impls detected for same trait/type region: coherence-overlap diagnostic.
- Orphan rule violation (foreign trait on foreign type): orphan-rule diagnostic.
- Specialization ordering invalid or ambiguous: specialization-coherence diagnostic.
- Coherence index corruption/inconsistent normalization keys: internal coherence diagnostic.

## Cross-Compilation

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - cross-target toolchain/ABI configuration and cross-compilation internals and failure behavior are documented.

### Implemented Internal Records

- Target registry resolves architecture/os/abi triples to backend codegen and linker profiles.
- Build pipeline splits host-tools and target-artifact stages to avoid host-target contamination.
- Sysroot/runtime selection binds target-specific stdlibs, startup objects, and linker scripts.

### API Execution Records (Complete)

- cross.target.resolve(triple): load target descriptor, data layout, and backend options.
- cross.stage.plan(project, target): produce host-tool and target-artifact build graph.
- cross.sysroot.bind(target): locate and validate target sysroot/runtime assets.
- cross.link.invoke(objects, target): execute target linker invocation with ABI profile flags.

### Failure Mode Matrix

- Unknown/unsupported target triple: target-resolution diagnostic.
- Missing target sysroot or runtime asset: cross-toolchain diagnostic.
- ABI mismatch between object set and target profile: link/abi diagnostic.
- Host tool executed in target stage (or inverse): stage-separation diagnostic.

## Inline Value Types

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - @inline annotation parsing, layout computation, copy-semantics enforcement, and backend lowering documented.

### Implemented Internal Records

- Parser recognizes `@inline` decorator on struct declarations; the decorator is recorded as a type-level attribute in the StructDecl AST node.
- Semantic analysis validates @inline constraints: all fields must be fixed-size types (primitives, other @inline structs, or fixed-size arrays). No reference types, no class inheritance.
- Layout computation for @inline types uses a flat, packed representation with known size at compile time. The type's sizeof is computed during type resolution and cached.
- Copy semantics enforcement: all assignments, function arguments, and return values involving @inline types are deep-copied (bitwise copy for POD types). No reference aliasing is possible.
- Backend lowering: @inline types are stack-allocated in function frames. When used in collections, they are stored inline in the collection's backing buffer (no pointer indirection).

### API Execution Records (Complete)

- inline.parse(decorator, struct_decl): validate @inline is applied to a struct (not a class/enum) -> set inline attribute on StructDecl.
- inline.validate_layout(struct_type): compute total byte size from field types -> reject if any field is a reference type or unbounded -> reject if total size exceeds threshold (default 64 bytes, configurable).
- inline.compute_sizeof(struct_type): recursively sum field sizes with alignment padding -> cache result in type metadata -> emit to backend as constant.
- inline.lower_assignment(assign_ir, type_info): if type is @inline -> emit bitwise copy instead of reference copy -> update liveness analysis (source remains valid).
- inline.lower_param(param, type_info): pass @inline types by value (copy to callee frame) or by const reference if optimizer proves no mutation (optimization pass).
- inline.lower_collection_store(elem_type, collection): if elem_type is @inline -> emit inline storage in collection buffer without pointer indirection.
- inline.reject_borrow(borrow_expr, type_info): if target type is @inline -> emit compile error "cannot borrow @inline type — use copy semantics".

### Failure Mode Matrix

- @inline on class declaration: compile error "@inline is only supported on struct types".
- @inline struct contains reference-type field: compile error "field 'name' is a reference type; @inline structs must contain only value types".
- @inline struct exceeds size threshold: warning "@inline struct 'Name' is N bytes (threshold: 64); consider heap allocation".
- @inline type used with `is` identity operator: compile error "identity comparison not available for @inline types (use == for equality)".
- @inline type used with `&` borrow: compile error "cannot borrow @inline type".

## Profiling

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - profiling instrumentation/aggregation pipeline and profiling internals and failure behavior are documented.

### Implemented Internal Records

- Profiler inserts probes/sampling hooks at configured granularity (function/basic-block/allocation).
- Runtime collector aggregates event streams into time and call-graph summaries with symbol resolution.
- Export pipeline emits standardized profiling artifacts for flamegraph and trace viewers.

### API Execution Records (Complete)

- profile.instrument(module, config): inject profiling hooks according to instrumentation mode.
- profile.collect(session): gather samples/events and maintain aggregation counters.
- profile.symbolize(rawData): map addresses/ids to source symbols using debug metadata.
- profile.export(report, format): write profiling report in requested output format.

### Failure Mode Matrix

- Instrumentation requested without compatible debug/probe support: profiling-config diagnostic.
- Sampling buffer overflow with strict retention policy: profiling-runtime diagnostic.
- Symbolization failed due to missing debug metadata: profiling-symbol diagnostic.
- Export format incompatible with collected data schema: profiling-export diagnostic.

## std.dns — DNS Resolution

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.dns resolver/cache internals and DNS resolution failure behavior are documented.

### Implemented Internal Records

- Resolver pipeline builds query plans across system resolver, recursive upstreams, and cache layers.
- Record parser decodes resource records with TTL tracking and canonical name handling.
- Cache subsystem enforces positive/negative caching with expiry and invalidation policies.

### API Execution Records (Complete)

- std.dns.lookup(name, recordType, options): resolve DNS records via configured resolver chain.
- std.dns.reverse(ip, options): perform PTR lookup for address-to-name mapping.
- std.dns.cache.get(key): retrieve cached DNS response when valid.
- std.dns.cache.invalidate(pattern): evict matching cached entries by key/pattern.

### Failure Mode Matrix

- Query timeout or nameserver unavailability: dns-timeout diagnostic.
- Malformed DNS response/record parsing failure: dns-parse diagnostic.
- DNSSEC/policy validation failure in strict mode: dns-validation diagnostic.
- Cache poisoning guard triggered by inconsistent answer set: dns-cache-integrity diagnostic.

## std.2d — 2D Vector Graphics

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.2d vector/canvas internals and 2D graphics failure behavior are documented.

### Implemented Internal Records

- 2D engine manages path primitives, paint styles, and transform stacks.
- Rasterizer backend converts vector operations to target surfaces with anti-alias controls.
- Text subsystem integrates glyph shaping and font atlas caching for draw calls.

### API Execution Records (Complete)

- std.2d.canvas.create(config): create 2D drawing surface and context state.
- std.2d.path.draw(ctx, path, paint): render vector path using active paint settings.
- std.2d.text.draw(ctx, text, pos, style): draw text glyph runs on target surface.
- std.2d.image.blit(ctx, img, dst): composite image buffer into canvas region.

### Failure Mode Matrix

- Invalid path command sequence or geometry state: gfx2d-path diagnostic.
- Surface/backend allocation fails for canvas target: gfx2d-surface diagnostic.
- Font glyph shaping/render resource unavailable: gfx2d-text diagnostic.
- Blend/composite mode unsupported by backend: gfx2d-composite diagnostic.

## std.graphql — GraphQL Client & Server

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.graphql schema/execution internals and GraphQL failure behavior are documented.

### Implemented Internal Records

- GraphQL parser builds operation AST and validates field selections against schema definitions.
- Execution engine resolves fields through resolver registry with batching and deferred-resolution support.
- Transport adapters expose HTTP/WebSocket integrations for query/mutation/subscription workflows.

### API Execution Records (Complete)

- std.graphql.schema.load(definition): compile schema and resolver binding metadata.
- std.graphql.execute(schema, operation, vars): run query/mutation against resolver pipeline.
- std.graphql.subscribe(schema, operation, vars): establish subscription stream for event updates.
- std.graphql.validate(schema, operation): perform static validation and cost analysis.

### Failure Mode Matrix

- Operation references unknown field/type in schema: graphql-validation diagnostic.
- Resolver execution throws or returns incompatible payload: graphql-resolver diagnostic.
- Query complexity exceeds configured cost budget: graphql-cost diagnostic.
- Subscription transport/channel teardown during active stream: graphql-subscription diagnostic.

## std.webrtc — WebRTC

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.webrtc peer/session internals and WebRTC failure behavior are documented.

### Implemented Internal Records

- WebRTC stack coordinates SDP negotiation, ICE candidate exchange, and DTLS/SRTP setup.
- Peer connection manager tracks transceivers, data channels, and media stream lifecycle events.
- NAT traversal subsystem integrates STUN/TURN configuration and connectivity state transitions.

### API Execution Records (Complete)

- std.webrtc.peer.create(config): initialize peer connection with media/data capabilities.
- std.webrtc.signal.offer(peer): generate SDP offer and collect local negotiation metadata.
- std.webrtc.signal.answer(peer, remoteOffer): apply remote offer and generate SDP answer.
- std.webrtc.channel.open(peer, label): create negotiated data channel for payload exchange.

### Failure Mode Matrix

- SDP negotiation fails capability/codec compatibility checks: webrtc-negotiation diagnostic.
- ICE connectivity checks fail across candidate set: webrtc-ice diagnostic.
- DTLS/SRTP handshake failure on secure media setup: webrtc-security diagnostic.
- Data channel state invalid during send/receive: webrtc-channel-state diagnostic.

## std.clipboard — Clipboard

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.clipboard provider internals and clipboard operation failure behavior are documented.

### Implemented Internal Records

- Clipboard adapter resolves platform provider and negotiates text/binary format conversions.
- Access broker serializes clipboard read/write operations to avoid provider contention.
- Security gate enforces foreground/session restrictions for clipboard access paths.

### API Execution Records (Complete)

- std.clipboard.readText(): fetch current clipboard text payload if available.
- std.clipboard.writeText(value): publish text payload to system clipboard provider.
- std.clipboard.readData(format): read non-text clipboard data for requested format.
- std.clipboard.clear(): clear active clipboard ownership/content when permitted.

### Failure Mode Matrix

- Clipboard provider unavailable in current session: clipboard-provider diagnostic.
- Access denied by platform privacy policy: clipboard-permission diagnostic.
- Requested format conversion unsupported: clipboard-format diagnostic.
- Provider contention/lock timeout during operation: clipboard-timeout diagnostic.

## std.notify — Desktop Notifications

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.notify notification-dispatch internals and desktop-notification failure behavior are documented.

### Implemented Internal Records

- Notification dispatcher maps payload schema to platform toast/notification center providers.
- Action routing associates notification interaction events with registered runtime callbacks.
- Lifecycle tracker manages notification IDs, replacement keys, and expiration policies.

### API Execution Records (Complete)

- std.notify.send(spec): emit desktop notification with title/body/actions metadata.
- std.notify.update(id, spec): replace existing notification content by identifier.
- std.notify.cancel(id): cancel active notification and release provider resources.
- std.notify.onAction(id, handler): register callback for notification action events.

### Failure Mode Matrix

- Notification provider missing on target environment: notify-provider diagnostic.
- Payload exceeds provider constraints (size/action limits): notify-payload diagnostic.
- Action callback registration fails lifecycle validation: notify-action diagnostic.
- Notification delivery suppressed by OS policy: notify-policy diagnostic.

## std.speech — Text-to-Speech & Recognition

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.speech synthesis/recognition internals and speech-runtime failure behavior are documented.

### Implemented Internal Records

- Speech runtime broker selects synthesis/recognition engines and voice/language resources.
- Audio pipeline manages microphone capture, stream chunking, and recognition buffering.
- Synthesis queue coordinates utterance scheduling with cancel/pause/resume controls.

### API Execution Records (Complete)

- std.speech.tts.speak(text, options): synthesize and play spoken output.
- std.speech.tts.stop(): stop active speech synthesis playback.
- std.speech.stt.start(options): begin speech recognition capture session.
- std.speech.stt.read(session): retrieve recognition transcript events.

### Failure Mode Matrix

- Speech engine unavailable or misconfigured on platform: speech-engine diagnostic.
- Microphone access denied or device unavailable: speech-input diagnostic.
- Recognition stream decode fails language/model constraints: speech-recognition diagnostic.
- Synthesis queue overflow or interruption in strict mode: speech-runtime diagnostic.

## std.camera — Camera & Webcam

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.camera device/capture internals and camera-capture failure behavior are documented.

### Implemented Internal Records

- Camera manager enumerates capture devices and negotiates resolution/frame-rate capabilities.
- Capture pipeline converts raw frames into configured pixel formats with timestamp metadata.
- Session state machine coordinates open/start/stop transitions and exclusive-device locks.

### API Execution Records (Complete)

- std.camera.list(): enumerate available camera devices and capabilities.
- std.camera.open(deviceId, config): open capture session with negotiated stream format.
- std.camera.read(session): fetch next captured frame buffer and timing metadata.
- std.camera.close(session): stop capture and release device resources.

### Failure Mode Matrix

- Camera device not found or already locked: camera-device diagnostic.
- Requested capture format unsupported by hardware: camera-format diagnostic.
- Permission denied for camera access: camera-permission diagnostic.
- Capture stream stalls or frame read timeout: camera-stream diagnostic.

## std.serial — Serial Port / UART

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.serial port/session internals and serial-transport failure behavior are documented.

### Implemented Internal Records

- Serial adapter configures UART parameters (baud/parity/stop bits/flow control) per endpoint.
- Framing layer supports line/binary protocols with configurable delimiter and timeout behavior.
- Port manager tracks open handles and cross-thread access synchronization.

### API Execution Records (Complete)

- std.serial.list(): enumerate available serial ports with descriptor metadata.
- std.serial.open(port, config): open serial port using validated UART configuration.
- std.serial.write(handle, data): transmit bytes with flow-control and timeout handling.
- std.serial.read(handle, options): read framed or raw bytes from serial stream.

### Failure Mode Matrix

- Invalid UART configuration for selected hardware: serial-config diagnostic.
- Port unavailable or already in use: serial-port-state diagnostic.
- Read/write timeout under configured deadline: serial-timeout diagnostic.
- Framing/parity error detected in strict mode: serial-framing diagnostic.

## std.usb — USB Devices

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.usb device/interface internals and USB I/O failure behavior are documented.

### Implemented Internal Records

- USB subsystem enumerates devices/configurations and resolves interface/endpoint descriptors.
- Transfer engine handles control/bulk/interrupt transfer scheduling with completion callbacks.
- Permission layer applies platform-specific device access policies and claim/release semantics.

### API Execution Records (Complete)

- std.usb.list(filters): discover USB devices matching class/vendor/product criteria.
- std.usb.open(device): claim device handle and initialize configuration context.
- std.usb.transfer(handle, endpoint, payload): execute endpoint transfer operation.
- std.usb.close(handle): release claimed interfaces and close device session.

### Failure Mode Matrix

- Device/interface claim denied or detached during use: usb-device-state diagnostic.
- Endpoint transfer stall or protocol error: usb-transfer diagnostic.
- Access blocked by platform security policy: usb-permission diagnostic.
- Unsupported descriptor/alternate setting requested: usb-descriptor diagnostic.

## std.bluetooth — Bluetooth & BLE

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.bluetooth adapter/session internals and Bluetooth/BLE failure behavior are documented.

### Implemented Internal Records

- Bluetooth manager tracks adapter state, discovery sessions, and pairing workflows.
- BLE GATT layer models services/characteristics and subscription notifications.
- Transport bridge normalizes classic Bluetooth and BLE channel semantics to runtime APIs.

### API Execution Records (Complete)

- std.bluetooth.scan(options): start device discovery with filter and duration policies.
- std.bluetooth.connect(device, options): establish classic/BLE connection session.
- std.bluetooth.gatt.read(conn, characteristic): read characteristic value from connected device.
- std.bluetooth.gatt.write(conn, characteristic, value): write characteristic payload with mode control.

### Failure Mode Matrix

- Adapter unavailable or powered-off during operation: bluetooth-adapter diagnostic.
- Pairing/authentication handshake fails security policy: bluetooth-pairing diagnostic.
- GATT characteristic access violates permissions/properties: bluetooth-gatt diagnostic.
- Connection drops or signal timeout exceeds retry policy: bluetooth-connection diagnostic.

## std.hotkey — Global Hotkeys

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.hotkey registration/dispatch internals and global-hotkey failure behavior are documented.

### Implemented Internal Records

- Hotkey registry maps key-chord descriptors to global hook registrations per platform backend.
- Event dispatcher routes key events to subscribed handlers with debounce and repeat filtering.
- Lifecycle manager ensures hotkey cleanup on process exit and dynamic reconfiguration changes.

### API Execution Records (Complete)

- std.hotkey.register(chord, handler): register global key chord and callback binding.
- std.hotkey.unregister(id): remove registered hotkey by identifier.
- std.hotkey.enable(scope): activate hotkey processing in selected scope/context.
- std.hotkey.disable(scope): suspend hotkey processing without dropping registrations.

### Failure Mode Matrix

- Chord conflicts with existing registration or reserved OS shortcut: hotkey-conflict diagnostic.
- Global hook installation denied by platform policy: hotkey-permission diagnostic.
- Handler dispatch fails due to stale registration state: hotkey-dispatch diagnostic.
- Unsupported key chord format for backend: hotkey-format diagnostic.

## std.tray — System Tray

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.tray icon/menu internals and system-tray failure behavior are documented.

### Implemented Internal Records

- Tray runtime binds icon resources, tooltip metadata, and platform menu models.
- Menu action router dispatches click/command events to registered handlers.
- State synchronizer applies incremental tray/menu updates with backend reconciliation.

### API Execution Records (Complete)

- std.tray.create(spec): create tray icon and initial menu/action configuration.
- std.tray.update(tray, patch): apply tray icon/menu state updates.
- std.tray.onAction(tray, actionId, handler): bind action callback for tray menu event.
- std.tray.destroy(tray): remove tray artifact and release backend resources.

### Failure Mode Matrix

- Tray backend unavailable in current desktop environment: tray-provider diagnostic.
- Icon/menu resource payload invalid or unsupported: tray-resource diagnostic.
- Action routing references unknown menu item: tray-action diagnostic.
- Update failed due to backend desync/state invalidation: tray-state diagnostic.

## std.ipc — Inter-Process Communication

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.ipc channel/endpoint internals and IPC failure behavior are documented.

### Implemented Internal Records

- IPC broker abstracts named pipes, unix-domain sockets, and shared-memory endpoints per platform.
- Message codec enforces framing/schema validation and optional zero-copy transfer paths.
- Endpoint lifecycle manager tracks registration, handshake, and teardown with access-control checks.

### API Execution Records (Complete)

- std.ipc.endpoint.open(name, mode): create or attach to IPC endpoint with policy constraints.
- std.ipc.send(endpoint, message): serialize and transmit framed IPC message payload.
- std.ipc.receive(endpoint, options): receive/decode inbound message with timeout policy.
- std.ipc.close(endpoint): close endpoint and release associated transport resources.

### Failure Mode Matrix

- Endpoint name collision or unavailable transport primitive: ipc-endpoint diagnostic.
- Framing/schema mismatch for received message: ipc-codec diagnostic.
- Access-control policy denies endpoint operation: ipc-access diagnostic.
- Peer disconnect or broken pipe during active exchange: ipc-transport diagnostic.

## std.decimal — Exact Decimal Arithmetic

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.decimal precision/arithmetic internals and decimal-operation failure behavior are documented.

### Implemented Internal Records

- Decimal engine stores base-10 coefficient/exponent tuples with configurable precision context.
- Arithmetic operations implement rounding modes and scale alignment before computation.
- Conversion layer maps decimal values to/from integer/float/text representations with policy checks.

### API Execution Records (Complete)

- std.decimal.parse(text, context): parse exact decimal value from textual input.
- std.decimal.add(a, b, context): perform decimal addition under precision/rounding context.
- std.decimal.mul(a, b, context): perform decimal multiplication with scale management.
- std.decimal.format(value, style): render decimal value using style/locale formatting rules.

### Failure Mode Matrix

- Decimal parse input invalid or exceeds precision context: decimal-parse diagnostic.
- Arithmetic overflow/underflow in checked decimal context: decimal-overflow diagnostic.
- Rounding policy conflict for required exact operation: decimal-rounding diagnostic.
- Inexact conversion forbidden by strict policy: decimal-conversion diagnostic.

## std.diff — Text Diffing & Patching

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.diff algorithm/patch internals and diff-patching failure behavior are documented.

### Implemented Internal Records

- Diff engine computes line/word/byte deltas using selectable algorithm strategies.
- Patch applier replays hunks with context matching and fuzz tolerance controls.
- Normalizer handles newline/encoding policies before comparison or patch application.

### API Execution Records (Complete)

- std.diff.compute(a, b, options): compute structured diff hunks between two inputs.
- std.diff.apply(base, patch, options): apply patch hunks to base content source.
- std.diff.reverse(patch): generate reverse patch operations for rollback.
- std.diff.render(diff, format): render diff output in unified/context/custom formats.

### Failure Mode Matrix

- Patch hunk context not found within tolerance: diff-apply diagnostic.
- Input normalization/encoding mismatch prevents comparison: diff-encoding diagnostic.
- Unsupported diff format requested for render/apply: diff-format diagnostic.
- Reverse patch generation fails due to incomplete metadata: diff-reverse diagnostic.

## std.semver — Semantic Versioning

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.semver parse/constraint internals and semantic-version failure behavior are documented.

### Implemented Internal Records

- Semver parser tokenizes major/minor/patch + prerelease/build metadata components.
- Constraint solver evaluates version ranges, unions, intersections, and precedence ordering.
- Normalizer canonicalizes equivalent version expressions for cache and comparison stability.

### API Execution Records (Complete)

- std.semver.parse(text): parse semantic version string into structured components.
- std.semver.compare(a, b): compare two semantic versions with prerelease precedence rules.
- std.semver.satisfies(version, constraint): evaluate version against range/constraint expression.
- std.semver.normalize(expr): canonicalize version or constraint expression string.

### Failure Mode Matrix

- Invalid semver token/order in version string: semver-parse diagnostic.
- Constraint expression grammar error: semver-constraint diagnostic.
- Comparison request with non-semver-compatible input: semver-compare diagnostic.
- Normalization ambiguity under strict mode: semver-normalize diagnostic.

## std.geo — Geospatial

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.geo coordinate/spatial internals and geospatial failure behavior are documented.

### Implemented Internal Records

- Geo core models coordinate systems, projections, and geometry primitives.
- Spatial algorithms provide distance, containment, intersection, and topology operations.
- Projection adapter transforms coordinates between CRS definitions with precision metadata.

### API Execution Records (Complete)

- std.geo.point(lat, lon, options): create typed coordinate point value.
- std.geo.distance(a, b, model): compute geodesic or planar distance metric.
- std.geo.transform(geom, fromCrs, toCrs): reproject geometry into target CRS.
- std.geo.query.contains(region, point): evaluate spatial containment predicate.

### Failure Mode Matrix

- Coordinate value outside valid domain for CRS: geo-coordinate diagnostic.
- Projection definition missing/unsupported for transform path: geo-projection diagnostic.
- Geometry topology invalid for requested operation: geo-topology diagnostic.
- Precision policy exceeded for strict spatial computation: geo-precision diagnostic.

## std.gpu — GPU Compute

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.gpu device/compute internals and GPU-compute failure behavior are documented.

### Implemented Internal Records

- GPU runtime selects adapter/device and initializes queue/command resource pools.
- Buffer/shader manager tracks allocation lifetimes and pipeline binding layouts.
- Dispatch scheduler submits compute workloads with synchronization fence handling.

### API Execution Records (Complete)

- std.gpu.device.open(options): open GPU device and queue context.
- std.gpu.buffer.create(size, usage): allocate GPU buffer with usage flags.
- std.gpu.shader.compile(source, stage): compile shader module for compute/render stage.
- std.gpu.dispatch(pipeline, groups, bindings): execute compute dispatch workload.

### Failure Mode Matrix

- No compatible GPU adapter found for required features: gpu-adapter diagnostic.
- Shader compilation/binding layout mismatch: gpu-shader diagnostic.
- Buffer allocation exceeds device/resource limits: gpu-memory diagnostic.
- Dispatch synchronization/fence timeout in strict mode: gpu-dispatch diagnostic.

## std.accessibility — Accessibility APIs

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.accessibility tree/event internals and accessibility-API failure behavior are documented.

### Implemented Internal Records

- Accessibility bridge maps runtime UI semantics to platform accessibility tree nodes and roles.
- Event translator forwards focus/value/state changes to assistive-technology backends.
- Metadata layer maintains label/hint/action descriptors for accessibility audits and tooling.

### API Execution Records (Complete)

- std.accessibility.node.create(spec): register accessibility node with role/label metadata.
- std.accessibility.node.update(node, patch): mutate node state and emit accessibility events.
- std.accessibility.focus.set(node): request accessibility focus transition for node.
- std.accessibility.audit(tree): run accessibility rule checks over registered node tree.

### Failure Mode Matrix

- Unsupported accessibility role/state mapping on target backend: accessibility-role diagnostic.
- Node update references invalid parent/child relationship: accessibility-tree diagnostic.
- Assistive backend event dispatch failure: accessibility-dispatch diagnostic.
- Audit rule violations in strict conformance mode: accessibility-audit diagnostic.

## std.blockchain — Blockchain & Web3

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.blockchain client/transaction internals and blockchain-operation failure behavior are documented.

### Implemented Internal Records

- Blockchain client handles RPC transport, chain metadata caching, and network selection.
- Transaction builder encodes operations, gas/fee policies, and signing payload layouts.
- Contract adapter manages ABI encoding/decoding and event log parsing.

### API Execution Records (Complete)

- std.blockchain.client.connect(endpoint, options): connect to chain node/provider backend.
- std.blockchain.tx.build(spec): construct transaction payload from operation spec.
- std.blockchain.tx.sign(tx, key): sign transaction payload with configured signer.
- std.blockchain.tx.send(client, signedTx): submit transaction and track confirmation state.

### Failure Mode Matrix

- RPC transport or chain endpoint unavailable: blockchain-rpc diagnostic.
- Transaction nonce/gas/fee validation fails policy checks: blockchain-tx diagnostic.
- Signature invalid for chain/account verification rules: blockchain-signature diagnostic.
- Contract ABI decode mismatch for call/event payload: blockchain-abi diagnostic.

## std.parse — Parser Combinators

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.parse combinator graph internals and parsing failure behavior are documented.

### Implemented Internal Records

- Parser combinator runtime models parser nodes as immutable graph fragments with compositional success/failure states.
- Input cursor tracks byte offset, UTF-8 line/column, and rollback checkpoints for backtracking and error reporting.
- Error aggregator records nearest-failure expected token sets and normalizes deterministic diagnostics ordering.

### API Execution Records (Complete)

- std.parse.token(text): construct exact-match parser node with literal-token expectation metadata.
- std.parse.seq(nodes): compose parser nodes into ordered sequence with short-circuit failure propagation.
- std.parse.choice(nodes): evaluate alternatives with priority ordering and nearest-failure aggregation.
- std.parse.run(node, input): execute parser graph against input and emit value/rest/error result record.

### Failure Mode Matrix

- Recursive parser graph cycle without lazy guard: parse-recursion diagnostic.
- Backtracking budget exceeded under strict parser limits: parse-backtrack diagnostic.
- Cursor state corruption after failed rollback path: parse-state diagnostic.
- Invalid parse-map transform output for declared schema/type: parse-transform diagnostic.

## std.config — Unified Configuration

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.config layer-merge/schema internals and configuration failure behavior are documented.

### Implemented Internal Records

- Layer loader ingests TOML/YAML/JSON/dotenv/env sources into canonical typed config maps.
- Merge engine applies deterministic precedence and deep-merge strategy with path-level conflict tracking.
- Dotenv layer handling delegates parsing/expansion to shared std.dotenv primitives to keep semantics aligned.
- Schema validator checks required/type/range constraints and emits path-precise diagnostics.

### API Execution Records (Complete)

- std.config.load(opts): read configured layers and produce merged configuration object.
- std.config.merge(base, overlay, policy): merge two configuration trees with conflict policy rules.
- std.config.validate(cfg, schema): validate configuration object against schema constraints.
- std.config.dotenv_read(path): parse dotenv file through shared dotenv parser and return key/value map.
- std.config.dotenv_apply(path, override): parse dotenv file and apply values into environment map.

### Failure Mode Matrix

- Config file unreadable or parse failed for declared format: config-source diagnostic.
- Deep-merge conflict violates strict policy for same key path: config-merge diagnostic.
- Required schema path missing or type mismatch: config-schema diagnostic.
- Dotenv key invalid/unset behavior violates environment policy: config-dotenv diagnostic.

## std.event — Event Bus & PubSub

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.event subscription/dispatch internals and event-delivery failure behavior are documented.

### Implemented Internal Records

- Topic registry maps topic strings to ordered subscriber lists with stable subscription IDs.
- Dispatch engine supports sync and queued async delivery while preserving per-topic ordering guarantees.
- Backpressure policy tracks queue depth, drop strategy, and dispatcher health counters.

### API Execution Records (Complete)

- std.event.bus(opts): create event bus instance and initialize queue/dispatcher policy.
- std.event.on(bus, topic, handler): register topic handler and return subscription identifier.
- std.event.emit(bus, topic, payload): enqueue/dispatch event payload to matching subscribers.
- std.event.off(bus, topic, subId): remove subscription and reclaim handler entry.

### Failure Mode Matrix

- Subscription registration exceeds configured topic/subscriber bounds: event-subscription diagnostic.
- Async queue overflow under strict drop policy: event-backpressure diagnostic.
- Handler invocation panic/error in strict propagation mode: event-handler diagnostic.
- Event payload fails schema/type guard for topic contract: event-payload diagnostic.

## std.diag — Diagnostics & Observability

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.diag metric/tracing/health internals and observability failure behavior are documented.

### Implemented Internal Records

- Metric registry manages counters/gauges/histograms keyed by metric name and normalized label set.
- Span runtime tracks parent-child relationships, events, and duration metadata for trace exports.
- Health subsystem executes registered probes with timeout budgets and aggregate status rollup.

### API Execution Records (Complete)

- std.diag.counter(name, opts): register or retrieve counter metric handle.
- std.diag.histogram(name, opts): register histogram metric with bucket policy.
- std.diag.span(name, attrs): open trace span context and attach metadata.
- std.diag.health.report(opts): execute health checks and emit aggregate report payload.

### Failure Mode Matrix

- Metric registration conflicts on name/type/label schema: diag-metric-schema diagnostic.
- Span lifecycle misuse (end twice, invalid parent context): diag-span-state diagnostic.
- Health check timeout or exception in strict mode: diag-healthcheck diagnostic.
- Exporter backend unavailable or payload encoding failed: diag-export diagnostic.

## std.iot — IoT & Embedded

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.iot hardware-adapter internals and embedded runtime failure behavior are documented.

### Implemented Internal Records

- Hardware abstraction layer maps logical pin/bus operations onto board-specific driver adapters.
- Capability registry records per-target support for gpio/pwm/adc/dac/i2c/spi/uart at runtime startup.
- Interrupt/timer coordination enforces deterministic ordering policies for soft realtime execution profiles.

### API Execution Records (Complete)

- std.iot.board(): resolve active board adapter and expose capability metadata (gpio/pwm/adc/dac/i2c/spi/uart).
- std.iot.gpio.write(pin, value): validate pin mode and dispatch digital write through board driver.
- std.iot.i2c.transfer(bus, addr, payload): execute I2C transaction with timeout/retry policy.
- std.iot.spi.transfer(bus, payload): execute SPI full-duplex transfer and return response buffer.

### Failure Mode Matrix

- Requested capability unavailable on active board profile: iot-capability diagnostic.
- Pin/bus index out of range or reserved by runtime: iot-pin-bus diagnostic.
- Peripheral transaction timeout exceeded strict policy: iot-timeout diagnostic.
- Driver adapter initialization failed due permission/hardware lock: iot-driver diagnostic.

## std.office — Office Documents

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.office document-package internals and office-format failure behavior are documented.

### Implemented Internal Records

- OOXML package builder manages part/relationship graphs for DOCX/PPTX documents.
- Style/numbering engine normalizes document runs, paragraphs, and table metadata before serialization.
- Reader pipeline validates ZIP container + XML schema expectations before exposing typed document models.

### API Execution Records (Complete)

- std.office.docx.new(opts): allocate DOCX document model and initialize package graph.
- std.office.docx.save(doc, path): serialize DOCX parts and write ZIP package artifact.
- std.office.pptx.new(opts): allocate PPTX slide deck model and relationship index.
- std.office.read(path): detect office format and decode into typed document object.

### Failure Mode Matrix

- Document model violates OOXML relationship/schema constraints: office-schema diagnostic.
- Embedded asset missing or unsupported media format: office-asset diagnostic.
- ZIP/package write fails integrity checks: office-package diagnostic.
- Unsupported office feature encountered in strict reader mode: office-feature diagnostic.

## std.money — Money & Financial Arithmetic

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.money typed-currency internals and financial arithmetic failure behavior are documented.

### Implemented Internal Records

- Money value layout stores decimal mantissa/scale with ISO currency code and rounding context.
- Arithmetic engine enforces same-currency invariants unless explicit conversion graph is provided.
- FX adapter validates rate timestamp/source metadata and applies deterministic conversion rounding.

### API Execution Records (Complete)

- std.money.new(amount, currency): construct typed money value with currency metadata.
- std.money.add(a, b): perform same-currency addition with decimal precision preservation.
- std.money.round(m, scale, mode): apply configured rounding mode to money amount.
- std.money.convert(m, to, rates): convert money across currencies via supplied rate table.

### Failure Mode Matrix

- Arithmetic attempted across different currencies without conversion context: money-currency diagnostic.
- Invalid rounding mode/scale for selected currency policy: money-rounding diagnostic.
- FX rate missing/stale for requested pair under strict policy: money-fx diagnostic.
- Decimal precision overflow exceeds configured financial bounds: money-precision diagnostic.

## std.dotenv — Environment Files

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.dotenv parse/expand internals and environment-loading failure behavior are documented.

### Implemented Internal Records

- Dotenv lexer/parser handles key/value quoting, escapes, comments, and multiline value policies.
- Expansion resolver evaluates `${VAR}` placeholders against parsed map and process environment precedence.
- Shared dotenv primitives are reused by std.config dotenv helpers to enforce identical behavior.
- Loader applies normalization and optional override semantics when mutating runtime environment map.

### API Execution Records (Complete)

- std.dotenv.parse(text, opts): parse dotenv text into key/value dictionary.
- std.dotenv.read(path, opts): load file and parse dotenv entries.
- std.dotenv.load(path, override): apply dotenv values into process environment.
- std.dotenv.require(keys): assert required keys exist and return normalized subset.

### Failure Mode Matrix

- Invalid dotenv syntax/tokenization at key/value boundary: dotenv-parse diagnostic.
- Variable expansion cycle or unresolved placeholder in strict mode: dotenv-expand diagnostic.
- Attempted override of protected runtime variable: dotenv-protected diagnostic.
- Dotenv file unreadable due path/permission errors: dotenv-io diagnostic.

## std.scrape — Web Scraping & Browser Automation

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.scrape fetch/dom/headless internals and scraping failure behavior are documented.

### Implemented Internal Records

- Fetch layer applies HTTP policy (timeouts, redirects, cookies, robots checks) before DOM extraction.
- Selector engine compiles CSS/XPath selectors and executes deterministic traversal over parsed DOM trees.
- Headless runtime manages browser process/session/page lifecycle and command serialization across managed `chromium` (default) and optional `webkit` adapters.

### API Execution Records (Complete)

- std.scrape.fetch(url, opts): fetch remote resource and produce response/document context.
- std.scrape.select(doc, selector): evaluate selector and return matched node set.
- std.scrape.browser.new(opts): launch browser session with selected backend and initialize automation channel.
- std.scrape.page.goto(page, url): navigate page with wait strategy and lifecycle events.

### Failure Mode Matrix

- Robots/policy gate denies requested crawl operation: scrape-policy diagnostic.
- Selector parsing/execution fails for malformed query: scrape-selector diagnostic.
- Headless session startup fails due missing/unsupported selected runtime backend: scrape-browser diagnostic.
- Navigation timeout/network failure under strict mode: scrape-navigation diagnostic.

## std.map — Geospatial Rendering

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.map scene/tile/render internals and map-rendering failure behavior are documented.

### Implemented Internal Records

- Scene graph stores vector layers, styling rules, camera parameters, and projection metadata.
- Tile manager resolves provider templates, cache keys, and zoom-level fetch policies.
- Renderer composes vector/tile layers into SVG/PNG/PDF outputs with deterministic paint order.

### API Execution Records (Complete)

- std.map.scene(opts): create scene container with projection and viewport settings.
- std.map.layer.geojson(scene, data, style): attach GeoJSON layer into scene graph.
- std.map.render.svg(scene, opts): render scene graph to SVG artifact.
- std.map.render.png(scene, opts): render scene graph to PNG artifact.

### Failure Mode Matrix

- Unsupported projection or invalid CRS transform request: map-projection diagnostic.
- Tile provider unavailable or cache integrity mismatch: map-tile diagnostic.
- Layer style references unsupported paint property: map-style diagnostic.
- Render output dimensions exceed configured budget limits: map-render diagnostic.

## std.task — Persistent Task Queue

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.task queue/scheduler internals and durable-job failure behavior are documented.

### Implemented Internal Records

- Queue storage adapter persists jobs, states, retries, and dead-letter transitions with transaction semantics across `sqlite://`, `postgres://`, `redis://`, and `memory://` backends.
- Scheduler computes due jobs from cron/delay expressions and enqueues runnable tasks deterministically.
- Worker coordinator manages leases, concurrency windows, and heartbeat/recovery for crashed workers.

### API Execution Records (Complete)

- std.task.queue.open(name, backend): initialize queue/storage adapter and validate supported backend scheme.
- std.task.enqueue(queue, job, opts): persist new job with retry/scheduling metadata.
- std.task.worker.start(queue, opts, handler): run worker loop with lease + ack protocol.
- std.task.deadletter.move(queue, jobId, reason): move failed job into dead-letter store.

### Failure Mode Matrix

- Storage backend transaction failed while mutating job state: task-storage diagnostic.
- Retry policy exhausted and no dead-letter route configured: task-retry diagnostic.
- Scheduler expression invalid or unsupported in strict mode: task-schedule diagnostic.
- Worker lease lost due heartbeat timeout race: task-lease diagnostic.

## std.phone — Telephony & SMS

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.phone provider abstraction internals and telephony-operation failure behavior are documented.

### Implemented Internal Records

- Provider registry maps canonical SMS/call operations onto backend-specific HTTP/API adapters.
- Number normalization pipeline validates E.164 formatting and country policy constraints.
- Verification subsystem tracks OTP issuance/attempt windows and anti-abuse throttling state.

### API Execution Records (Complete)

- std.phone.client(opts): create provider-backed telephony client context.
- std.phone.sms.send(client, request): send SMS and record provider message metadata.
- std.phone.call.start(client, request): initiate outbound voice call workflow.
- std.phone.otp.verify(client, token, code): verify OTP under attempt/time constraints.

### Failure Mode Matrix

- Invalid/unsupported phone number format for configured region: phone-number diagnostic.
- Provider authentication/authorization failure: phone-provider-auth diagnostic.
- Message/call request rejected by provider policy limits: phone-provider-policy diagnostic.
- OTP token expired or max attempts exceeded: phone-otp diagnostic.

## std.barcode — Barcode Generation & Scanning

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.barcode symbology/render internals and barcode-processing failure behavior are documented.

### Implemented Internal Records

- Symbology registry validates payload constraints and check-digit policies per barcode type.
- Encoder builds canonical bar/space (or matrix) representations before raster/vector rendering.
- Decoder pipeline applies image preprocessing, finder pattern detection, and confidence scoring.

### API Execution Records (Complete)

- std.barcode.encode(kind, value, opts): validate payload and generate barcode model.
- std.barcode.render.svg(code, opts): render barcode model into SVG representation.
- std.barcode.render.png(code, opts): rasterize barcode model into bitmap image.
- std.barcode.decode.image(path, opts): decode barcode payloads from image input.

### Failure Mode Matrix

- Payload invalid for requested symbology/checksum policy: barcode-payload diagnostic.
- Unsupported render option for selected barcode model: barcode-render diagnostic.
- Decoder confidence below strict acceptance threshold: barcode-decode diagnostic.
- Image preprocessing failed due unsupported format/corruption: barcode-image diagnostic.

## std.ml.vision — Vision Pipelines

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.ml.vision model/pipeline internals and vision-task failure behavior are documented.

### Implemented Internal Records

- Model loader resolves backend runtime and performs shape/precision compatibility checks.
- Vision pipeline normalizes image tensor preprocessing and postprocess decoding for tasks.
- Batch executor coordinates device scheduling and deterministic result collation ordering.

### API Execution Records (Complete)

- std.ml.vision.load(model, opts): load vision model artifact into inference runtime.
- std.ml.vision.detect(model, image, opts): execute object detection pipeline.
- std.ml.vision.ocr(image, opts): execute OCR pipeline and produce text spans.
- std.ml.vision.segment(model, image, opts): execute segmentation and return mask artifacts.

### Failure Mode Matrix

- Model artifact incompatible with selected inference backend: ml-vision-model diagnostic.
- Input image shape/type fails preprocessing constraints: ml-vision-input diagnostic.
- Inference runtime/device execution failure: ml-vision-runtime diagnostic.
- Postprocess decoding failed for configured task schema: ml-vision-postprocess diagnostic.

## std.ml.audio — Audio ML Pipelines

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - std.ml.audio model/pipeline internals and audio-task failure behavior are documented.

### Implemented Internal Records

- Audio loader normalizes sample rate/channel layout and chunking strategy for model runtimes.
- Pipeline stages support transcription, diarization, and classification with shared feature extraction.
- Streaming coordinator tracks segment boundaries and merges partial hypotheses deterministically.

### API Execution Records (Complete)

- std.ml.audio.load(model, opts): load audio model and initialize runtime context.
- std.ml.audio.transcribe(model, audio, opts): run ASR transcription pipeline.
- std.ml.audio.diarize(audio, opts): run speaker diarization and produce segment labels.
- std.ml.audio.classify(audio, opts): classify audio event labels with confidence scores.

### Failure Mode Matrix

- Unsupported sample format/rate for selected model profile: ml-audio-input diagnostic.
- Runtime backend unavailable for requested audio model: ml-audio-model diagnostic.
- Streaming chunk sequence violates continuity policy: ml-audio-stream diagnostic.
- Postprocess alignment failed between transcript and diarization segments: ml-audio-alignment diagnostic.

## std.hal — Hardware Abstraction Layer

Implementation contract (required):

- Purpose and boundary: define runtime/compiler responsibility and explicit non-goals.
- Frontend behavior: parser entrypoints, AST/HIR nodes, desugar passes, and symbol-table effects.
- Semantic rules: type/effect/borrow/coherence constraints and invariants that must hold.
- Lowering plan: MIR/IR shape, intrinsic expansion, and backend-specific differences.
- Runtime behavior: state transitions, scheduling/allocation effects, synchronization/I-O interactions.
- Diagnostics contract: stable categories, span mapping, note chains, and remediation guidance.
- Performance contract: complexity targets, allocation profile, hot paths, and cache policy.
- Security contract: capability checks, unsafe boundaries, trust boundaries, and validation rules.
- Conformance contract: parser + semantic + runtime + negative + parity + perf tests required.

Function behavior coverage required: document every exported API in this chapter with preconditions, state transitions, side effects, failure modes, complexity, backend notes, and security constraints.
Coverage status: Complete - HAL platform detection, device enumeration, GPIO/I2C/SPI/ADC drivers, and bare-metal lowering documented.

### Implemented Internal Records

- HAL module detects platform at compile time (via --os/--arch flags) and selects the appropriate platform driver backend (Linux sysfs/devfs, Windows WinAPI, bare-metal register-mapped).
- Device enumeration queries the platform HAL backend for available peripherals and constructs a capability-tagged device registry at startup.
- GPIO/I2C/SPI/ADC/PWM operations are lowered to platform-specific syscalls or register writes; on bare-metal targets (--os none), they compile to direct memory-mapped register access.
- Safety gates: all HAL operations require the `hal` effect capability. In sandboxed/WASM contexts, HAL operations are rejected at compile time unless explicitly enabled.

### API Execution Records (Complete)

- hal.platform(): detect current platform from compile target -> return platform descriptor with architecture, OS, and available buses.
- hal.devices(): enumerate available hardware peripherals from platform HAL backend -> return list of device descriptors with capabilities.
- hal.gpio.open(pin, mode): validate pin number and mode (input/output/alt) -> acquire platform GPIO handle -> configure pin direction and pull resistor.
- hal.gpio.read(pin): read current pin state -> return 0 or 1.
- hal.gpio.write(pin, value): validate value (0/1) -> write to pin output register.
- hal.gpio.watch(pin, edge, callback): register interrupt/poll handler for rising/falling/both edges -> invoke callback on state change.
- hal.i2c.open(bus, address, opts): validate bus ID and 7/10-bit address -> acquire I2C bus handle -> configure clock speed.
- hal.i2c.read(handle, length): issue I2C read transaction -> return byte buffer.
- hal.i2c.write(handle, data): validate data buffer -> issue I2C write transaction.
- hal.i2c.write_read(handle, write_data, read_length): issue combined write-then-read I2C transaction.
- hal.spi.open(bus, cs, opts): validate bus/chip-select -> acquire SPI handle -> configure mode/clock/bit-order.
- hal.spi.transfer(handle, tx_data): perform full-duplex SPI transfer -> return rx_data of same length.
- hal.adc.open(channel, opts): validate ADC channel -> configure resolution and sample rate -> return handle.
- hal.adc.read(handle): trigger ADC conversion -> return raw digital value.
- hal.adc.read_voltage(handle): trigger conversion -> apply calibration -> return voltage as float.
- hal.pwm.open(channel, frequency, duty): configure PWM channel with frequency (Hz) and duty cycle (0.0-1.0) -> start output.
- hal.pwm.set_duty(handle, duty): update duty cycle on active PWM channel.
- hal.pwm.stop(handle): stop PWM output and release channel.
- hal.uart.open(port, baud, opts): open UART port with baud rate and optional parity/stop-bit config -> return handle.
- hal.uart.read(handle, length, timeout?): read bytes from UART with optional timeout -> return byte buffer.
- hal.uart.write(handle, data): write byte buffer to UART.
- hal.sleep_us(microseconds): busy-wait or timer-based microsecond delay for bare-metal timing.

### Failure Mode Matrix

- HAL operation in sandboxed/WASM context without hal capability: compile error "hal operations require the 'hal' effect capability".
- GPIO pin not available on target platform: runtime HalError with pin/platform details.
- I2C/SPI bus contention (concurrent access without locking): runtime HalError with bus ID and contention context.
- ADC channel not calibrated or out-of-range reading: runtime HalError with channel and raw value.
- UART read timeout exceeded: TimeoutError with port and timeout duration.
- Bare-metal register write to invalid address: undefined behavior in unsafe context; sanitizer catches in debug builds.
