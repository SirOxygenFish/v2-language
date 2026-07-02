# V2 Language Implementation Audit

This document audits every language feature mentioned in DOCS.md against the actual implementation in `v2/src/interpreter.rs`, `v2/src/ast.rs`, `v2/src/value.rs`, `v2/src/lexer.rs`, `v2/src/parser.rs`, and related source files.

**Legend:**
- `IMPLEMENTED` — fully works at runtime
- `PARTIAL` — parses/registers but behavior is incomplete, simulated, or unverified
- `MISSING` — documented feature with no implementation
- `STUB` — builtin or module entry is registered but always returns a trivial/fake value with no real logic

---

## SECTION: Arithmetic Operators

- Feature: `+, -, *, /, //, %, **`
- Status: IMPLEMENTED
- Evidence: `BinOp::{Add,Sub,Mul,Div,IntDiv,Mod,Pow}` in `ast.rs`; all arms handled in `binary_op()` in `interpreter.rs`

- Feature: `x++`, `x--` (post-increment/decrement)
- Status: IMPLEMENTED
- Evidence: `lexer.rs` emits `PlusPlus`/`MinusMinus`; `parser.rs` (lines 1789-1806) desugars to `Stmt::Assign { op: PlusAssign/MinusAssign, value: Int(1) }`

---

## SECTION: Comparison Operators

- Feature: `==, !=, <, >, <=, >=`
- Status: IMPLEMENTED
- Evidence: `BinOp::{Eq,NotEq,Lt,Gt,LtEq,GtEq}`; handled in `binary_op()`

- Feature: `is` type-check operator
- Status: IMPLEMENTED
- Evidence: `BinOp::Is`; dispatched in `binary_op()` via `value.type_name()` comparison

- Feature: `in`, `not in` membership
- Status: IMPLEMENTED
- Evidence: `BinOp::{In,NotIn}`; checks strings, lists, dicts, sets, ranges; dispatches `__contains__` on instances

---

## SECTION: Logical Operators

- Feature: `&&`, `||`, `!`
- Status: IMPLEMENTED
- Evidence: Short-circuit logic in `eval_expr()` for `And`/`Or`; `UnaryOp::Not` in `unary_op()`

- Feature: `not` keyword alias for `!`
- Status: IMPLEMENTED
- Evidence: Lexer maps `not` keyword to `Not` token; parser maps to `UnaryOp::Not`

---

## SECTION: Bitwise Operators

- Feature: `band`, `bor`, `bxor`, `bnot`, `<<`, `>>`
- Status: IMPLEMENTED
- Evidence: `BinOp::{BitAnd,BitOr,BitXor,Shl,Shr}`, `UnaryOp::BitNot`; all handled in `binary_op()` / `unary_op()` for `Int` values

---

## SECTION: Assignment Operators

- Feature: `=` (plain assignment)
- Status: IMPLEMENTED
- Evidence: `exec_assign()` handles `AssignOp::Assign`

- Feature: `+=, -=, *=, /=, //=, **=, %=, <<=, >>=, band=, bor=, bxor=`
- Status: IMPLEMENTED
- Evidence: All variants in `AssignOp` enum in `ast.rs`; `exec_assign()` applies the op then stores

- Feature: Chained field assignment (`a.b.c = x`) and index assignment (`a[i][j] = x`)
- Status: IMPLEMENTED
- Evidence: `exec_assign()` recursively resolves field-access and index-access chains

---

## SECTION: Other Operators

- Feature: `?:` ternary
- Status: IMPLEMENTED
- Evidence: `Expr::Ternary`; evaluated in `eval_expr()`

- Feature: `??` null coalescing
- Status: IMPLEMENTED
- Evidence: `BinOp::NullCoalesce`; returns right side when left is `Null`; unwraps `Value::Some`

- Feature: `?.` optional chaining
- Status: IMPLEMENTED
- Evidence: `Expr::MethodCall { optional: true }` and `Expr::FieldAccess { optional: true }`; returns `Null` on `Null` receiver

- Feature: `?` try operator
- Status: IMPLEMENTED
- Evidence: `Expr::TryUnwrap`; unwraps `Ok(v)` → `v`, `Some(v)` → `v`; propagates `Err`/`Null` as early return

- Feature: `as` type cast
- Status: IMPLEMENTED
- Evidence: `Expr::Cast`; handles `int`, `float`, `str`, `bool`, `bytes` targets

- Feature: `|>` pipe operator
- Status: IMPLEMENTED
- Evidence: `Expr::Pipe`; evaluates left, calls right with result as argument

- Feature: `_` pipe placeholder
- Status: PARTIAL
- Evidence: `_` in pipe call args is substituted; full expression-level placeholder substitution (e.g., `x |> f(_, y)`) is not fully verified

- Feature: `..`, `..=` range expressions
- Status: IMPLEMENTED
- Evidence: `Expr::Range { inclusive }`; produces `Value::Range(start, end, inclusive)`; iterated in `value_to_iter()`

- Feature: `...expr` spread in list literals
- Status: IMPLEMENTED
- Evidence: `Expr::Spread`; list builder iterates inner value

- Feature: `...args` spread in function calls
- Status: IMPLEMENTED
- Evidence: `CallArg { is_spread: true }`; `call_value()` expands the spread arg into positional args

- Feature: `&` address-of / borrow annotation
- Status: PARTIAL
- Evidence: Parsed in type annotations and call sites; `borrow()` builtin returns the value unchanged; no borrow semantics enforced

- Feature: `&mut` mutable borrow annotation
- Status: PARTIAL
- Evidence: Parsed; not enforced

- Feature: `*` dereference
- Status: PARTIAL
- Evidence: `deref()` builtin is an identity function; no pointer-level dereferencing for `Value::Pointer`

---

## SECTION: F-Strings

- Feature: Basic `f"${expr}"` interpolation
- Status: IMPLEMENTED
- Evidence: `Expr::FStr`; `eval_fstring()` parses and evaluates `${...}` blocks

- Feature: Format specifiers `${val:.2f}`, `${val:>10}`, etc.
- Status: IMPLEMENTED
- Evidence: `eval_fstring()` calls `format_value_with_spec()` when `:` is found inside `${}`

- Feature: Multi-line `f"""..."""`
- Status: IMPLEMENTED
- Evidence: Lexer handles triple-quoted F-strings; stored as `FStr`

- Feature: Nested expressions in `${}`
- Status: IMPLEMENTED
- Evidence: `eval_fstring()` tracks `{}`-depth to handle nested braces

- Feature: Tagged template literals (e.g., `html`...``)
- Status: MISSING
- Evidence: No `TaggedTemplate` AST node; no tag-dispatch path in `eval_expr()`

---

## SECTION: Raw and Byte Strings

- Feature: `r"..."` raw strings (no escape processing)
- Status: IMPLEMENTED
- Evidence: Lexer handles `r` prefix; backslashes taken literally

- Feature: `r"""..."""` raw triple-quoted strings
- Status: IMPLEMENTED

- Feature: `b"..."` byte strings
- Status: IMPLEMENTED
- Evidence: `Expr::ByteStr`; produces `Value::Bytes(Vec<u8>)`

---

## SECTION: String Methods

- Feature: `.len()`
- Status: IMPLEMENTED
- Evidence: `(Value::Str(s), "len")` arm in `call_builtin_method()`

- Feature: `.upper()` / `.lower()` (also `to_upper`, `to_lower`)
- Status: IMPLEMENTED
- Evidence: Both camelCase and snake_case aliases present

- Feature: `.trim()` / `.trim_start()` / `.trim_end()`
- Status: IMPLEMENTED

- Feature: `.contains(sub)`, `.starts_with(prefix)`, `.ends_with(suffix)`
- Status: IMPLEMENTED

- Feature: `.split(sep, n?)`
- Status: IMPLEMENTED
- Evidence: Uses `splitn` when `n` arg provided, else `split`

- Feature: `.replace(old, new)` / `.replace_first(old, new)`
- Status: IMPLEMENTED
- Evidence: `replace_first` uses `replacen(..., 1)`

- Feature: `.count(sub)`, `.index_of(sub)` / `.indexOf(sub)`, `.last_index_of(sub)` / `.lastIndexOf(sub)`
- Status: IMPLEMENTED
- Evidence: Both camelCase and snake_case aliases

- Feature: `.char_at(i)` / `.charAt(i)`
- Status: IMPLEMENTED

- Feature: `.substr(start, end)` / `.slice(start, end)`
- Status: IMPLEMENTED
- Evidence: Char-indexed, not byte-indexed

- Feature: `.repeat(n)`, `.pad_start(width, fill?)`, `.pad_end(width, fill?)`
- Status: IMPLEMENTED

- Feature: `.reverse()`
- Status: IMPLEMENTED

- Feature: `.is_alpha()` / `.isalpha()`, `.is_digit()` / `.isdigit()`, `.is_alnum()` / `.isalnum()`, `.is_space()` / `.isspace()`, `.is_upper()` / `.isupper()`, `.is_lower()` / `.islower()`
- Status: IMPLEMENTED
- Evidence: All six predicates with both naming conventions

- Feature: `.chars()`, `.bytes()`, `.to_bytes()`, `.byte_len()`
- Status: IMPLEMENTED

- Feature: `.capitalize()`, `.title()`, `.swapcase()`, `.center(width, fill?)`
- Status: IMPLEMENTED

- Feature: `.graphemes()` / `.grapheme_len()`
- Status: MISSING
- Evidence: No such arms in `call_builtin_method()`; no Unicode grapheme cluster library used

- Feature: `.encode(encoding)` / `.decode(bytes, encoding)`
- Status: MISSING
- Evidence: `__str_encode` / `__str_decode` stubs return empty string from catch-all handler

- Feature: String indexing `s[i]`
- Status: IMPLEMENTED

- Feature: String slicing `s[start:end:step]`
- Status: IMPLEMENTED
- Evidence: `eval_slice()` handles `Value::Str`; char-indexed with step support

---

## SECTION: Variables and Constants

- Feature: `let x = val`, `let x: Type = val`
- Status: IMPLEMENTED
- Evidence: `Stmt::Let`; type annotation stored (not enforced); `env.define()`

- Feature: `const X = val`
- Status: IMPLEMENTED
- Evidence: `Stmt::Const`; `env.define_const()`; reassignment raises an error

- Feature: `freeze(x)` / `is_frozen(x)`
- Status: IMPLEMENTED
- Evidence: Both in `call_builtin()`; uses a `frozen_values` set

- Feature: Variable scoping (block scope)
- Status: IMPLEMENTED
- Evidence: `env.push_scope()` / `env.pop_scope()` around every block

- Feature: Variable shadowing
- Status: IMPLEMENTED

---

## SECTION: Data Types

- Feature: `int` (i64), `float` (f64), `str`, `bool`, `null` / `None`
- Status: IMPLEMENTED

- Feature: `list`, `dict` (insertion-ordered), `tuple`, `set`, `bytes`
- Status: IMPLEMENTED
- Evidence: `Value::List`, `Value::Dict(Vec<(Value,Value)>)`, `Value::Tuple`, `Value::Set`, `Value::Bytes`

- Feature: `range`
- Status: IMPLEMENTED
- Evidence: `Value::Range(i64, i64, inclusive: bool)`; lazy iteration in `value_to_iter()`

- Feature: `pointer`
- Status: IMPLEMENTED
- Evidence: `Value::Pointer(i64)`; tracked allocation table in interpreter state

- Feature: `generator`
- Status: PARTIAL
- Evidence: `Value::Generator(Rc<RefCell<GeneratorState>>)`; items are pre-collected eagerly; not a true lazy coroutine

- Feature: `deque`
- Status: IMPLEMENTED
- Evidence: `Value::Deque`; `deque_*` builtins implemented in `call_builtin()`

- Feature: Sized integer types (i8, i16, i32, u8, u16, u32, u64)
- Status: MISSING
- Evidence: Only `i64` internally; size annotations parsed as strings but not enforced or range-checked

- Feature: Sized float types (f32)
- Status: MISSING
- Evidence: Only `f64` internally; `f32` annotation parsed but not enforced

---

## SECTION: Control Flow

- Feature: `if / else if / else`
- Status: IMPLEMENTED

- Feature: `while` loop
- Status: IMPLEMENTED

- Feature: `for (x in iter)` for-in loop
- Status: IMPLEMENTED
- Evidence: `Stmt::ForIn`; `value_to_iter()` handles lists, ranges, strings, dicts, sets, generators, instances with `iter()` protocol

- Feature: `for (init; cond; update)` classic C-style for
- Status: IMPLEMENTED
- Evidence: `Stmt::ForClassic`

- Feature: `for ([a, b] in iter)` destructuring for-in
- Status: IMPLEMENTED
- Evidence: `Stmt::ForInDestructure`

- Feature: `break` / `continue`
- Status: IMPLEMENTED

- Feature: Labeled loops (`break label` / `continue label`)
- Status: IMPLEMENTED
- Evidence: `Stmt::Label`, `Stmt::BreakLabel`, `Stmt::ContinueLabel`; `Value::BreakLabel(name)` signal

- Feature: `match` statement
- Status: IMPLEMENTED
- Evidence: `Stmt::Match`; `matches_pattern()` + `bind_pattern()` with guard evaluation

- Feature: `match` expression
- Status: IMPLEMENTED
- Evidence: `Expr::MatchExpr`; returns matched arm's result value

- Feature: `goto` / `label`
- Status: IMPLEMENTED
- Evidence: Scans forward in current block's statement list; limited to same block scope

- Feature: `defer`
- Status: IMPLEMENTED
- Evidence: `Stmt::Defer`; deferred stack executed LIFO at scope exit

- Feature: `return`
- Status: IMPLEMENTED
- Evidence: `Value::Return(val)` signal; propagates through call stack

---

## SECTION: Functions

- Feature: Function declaration `func name(params) { body }`
- Status: IMPLEMENTED

- Feature: Default parameter values
- Status: IMPLEMENTED
- Evidence: `Param { default: Some(Expr) }`; evaluated at call time if arg missing

- Feature: Named arguments `f(x: val)`
- Status: IMPLEMENTED
- Evidence: `CallArg { name: Some(String) }`; matched by name in `call_value()`

- Feature: Variadic `*args`
- Status: IMPLEMENTED
- Evidence: `Param { is_variadic: true }`; extra args collected into `Value::List`

- Feature: `pure` keyword
- Status: PARTIAL
- Evidence: `FuncDecl { is_pure: true }` parsed; no side-effect verification performed

- Feature: `async func`
- Status: PARTIAL
- Evidence: `FuncDecl { is_async: true }` parsed; executes synchronously; no event loop

- Feature: Generator `func*` / `yield`
- Status: PARTIAL
- Evidence: `FuncDecl { is_generator: true }`; body executed eagerly by `yield_collector`; `Value::Generator` wraps a pre-collected list, not a lazy coroutine

- Feature: Tail-call optimization (TCO)
- Status: IMPLEMENTED
- Evidence: `Value::TailCall(name, args)`; `call_value()` loops on self-calls

- Feature: Closures
- Status: IMPLEMENTED
- Evidence: `FuncValue { closure_env: usize }`; parent scope captured by index at definition time

---

## SECTION: Lambdas

- Feature: `lambda(params) => expr`, `lambda(params) { body }`
- Status: IMPLEMENTED
- Evidence: `Expr::Lambda` / `Expr::LambdaBlock`; stored as `Value::Func`

- Feature: `async lambda`
- Status: PARTIAL
- Evidence: `is_async` flag parsed; executes synchronously

- Feature: `move lambda`
- Status: PARTIAL
- Evidence: `is_move` flag parsed; no actual value-move enforcement

- Feature: Lambda as first-class value
- Status: IMPLEMENTED

---

## SECTION: Decorators

- Feature: `@memo`
- Status: IMPLEMENTED
- Evidence: `apply_memo_decorator()` inserts per-function cache; `call_value()` checks cache on invocation

- Feature: `@deprecated` / `@deprecated("msg")`
- Status: IMPLEMENTED
- Evidence: `apply_deprecated_decorator()`; warning printed on first call

- Feature: `@fixed` on classes
- Status: IMPLEMENTED
- Evidence: `ClassValue { is_fixed: true }`; undeclared field assignment raises an error

- Feature: `@data` on classes
- Status: IMPLEMENTED
- Evidence: `ClassValue { is_data: true }`; auto-generates field-equality `==` and `__str__`

- Feature: `@cow` on classes
- Status: IMPLEMENTED
- Evidence: `ClassValue { is_cow: true }`; instances stored as `Value::CowInstance`

- Feature: `@sealed` on classes
- Status: IMPLEMENTED
- Evidence: `ClassValue { is_sealed: true }`; subclass registration tracked

- Feature: Custom decorator functions
- Status: IMPLEMENTED
- Evidence: Any callable used as a decorator is called with the function as argument

- Feature: `@derive(Trait, ...)`
- Status: MISSING
- Evidence: Comment at `parser.rs:774` acknowledges `@derive` on struct; interpreter's decorator dispatch has no `"derive"` branch; no trait implementations are auto-generated

- Feature: `@inline struct`
- Status: PARTIAL
- Evidence: Parsed; treated identically to a regular struct at runtime

---

## SECTION: Classes

- Feature: `class Name { }` declaration
- Status: IMPLEMENTED

- Feature: `class Name extends Parent { }` single inheritance
- Status: IMPLEMENTED
- Evidence: `ClassValue { parent: Option<String> }`; methods and fields inherited

- Feature: `constructor` / `init` method
- Status: IMPLEMENTED

- Feature: `self`, `super(args)`
- Status: IMPLEMENTED

- Feature: Instance method calls, field access/mutation
- Status: IMPLEMENTED

- Feature: `@fixed`, `@data`, `@sealed`, `@cow` class decorators
- Status: IMPLEMENTED

- Feature: Computed properties (`get_propname()` / `set_propname(val)`)
- Status: IMPLEMENTED
- Evidence: Field access checks for `get_{name}` on the class; assignment checks `set_{name}`

- Feature: `__str__`
- Status: IMPLEMENTED
- Evidence: `value_to_string()` dispatches to `__str__`

- Feature: `__add__`, `__sub__`, `__mul__`, `__div__`, `__mod__`, `__pow__`
- Status: IMPLEMENTED
- Evidence: `binary_op()` checks for these methods on `Value::Instance`

- Feature: `__eq__`, `__ne__`, `__lt__`, `__gt__`, `__le__`, `__ge__`
- Status: IMPLEMENTED

- Feature: `__getitem__` / `__setitem__`
- Status: IMPLEMENTED

- Feature: `__len__`, `__contains__`
- Status: IMPLEMENTED

- Feature: Custom iteration via `iter()` method
- Status: IMPLEMENTED
- Evidence: `value_to_iter()` checks for an `iter` method returning list/range/generator

- Feature: `__neg__` (unary minus on instance)
- Status: MISSING
- Evidence: `unary_op()` handles `Neg` only for `Int`/`Float`; no instance method dispatch

- Feature: `__not__` (unary `!` on instance)
- Status: MISSING
- Evidence: `unary_op()` for `Not` only calls `.is_truthy()`; no instance dispatch

- Feature: `__floordiv__`, `__band__`, `__bor__`, `__bxor__`, `__bnot__`, `__lshift__`, `__rshift__`
- Status: MISSING
- Evidence: `binary_op()` instance-method dispatch section has no arms for these operators

- Feature: `__getslice__` / `__setslice__`
- Status: MISSING
- Evidence: `eval_slice()` and slice assignment do not invoke instance methods

---

## SECTION: Structs

- Feature: `struct Name { fields }` declaration
- Status: IMPLEMENTED
- Evidence: `Stmt::StructDecl`; stored as `Value::Class`; instances are `Value::Instance` or `Value::StructInstance`

- Feature: Struct literal `Name { field: val }`
- Status: IMPLEMENTED
- Evidence: `Expr::StructLit`; fields validated when class is `@fixed`

- Feature: Struct update syntax `Name { ...base, field: newval }`
- Status: IMPLEMENTED
- Evidence: `Expr::StructLit { spread: Some(base_expr) }`; base fields copied then overridden

- Feature: Methods via `impl Name { ... }`
- Status: IMPLEMENTED
- Evidence: `Stmt::ImplBlock`; methods added to `ClassValue.methods`

- Feature: `cstruct`
- Status: PARTIAL
- Evidence: `Stmt::CStructDecl` parsed; constructible; no C ABI layout, alignment, or FFI enforcement

- Feature: Bitfield struct
- Status: PARTIAL
- Evidence: `Stmt::BitfieldStructDecl` parsed; stored as dict with field metadata; no actual bit-packing

- Feature: `@inline struct`
- Status: PARTIAL
- Evidence: Treated identically to a regular struct

---

## SECTION: Enums

- Feature: Plain enum `enum Color { Red, Green, Blue }`
- Status: IMPLEMENTED
- Evidence: `Stmt::EnumDecl`; variants stored as `Value::EnumVariant(enum_name, variant_name, [])`

- Feature: Enum with data `enum Shape { Circle(float), Rect(float, float) }`
- Status: IMPLEMENTED
- Evidence: Data-carrying variants stored as constructor `Value::Func`; calling constructs `Value::EnumVariant(name, variant, data_vec)`

- Feature: Enum methods via `impl`
- Status: IMPLEMENTED

- Feature: Generic enums `enum Option<T>`
- Status: PARTIAL
- Evidence: Generic parameter list accepted; `T` erased at runtime

- Feature: Match exhaustiveness warning
- Status: MISSING
- Evidence: No exhaustiveness analysis anywhere in interpreter or compiler

---

## SECTION: Traits

- Feature: `trait Name { method signatures and defaults }`
- Status: IMPLEMENTED
- Evidence: `Stmt::TraitDecl`; stored as `Value::Class`-like dict

- Feature: `impl Trait for Type`
- Status: IMPLEMENTED
- Evidence: `Stmt::ImplBlock { trait_name: Some(...) }`; methods merged into target type's class

- Feature: Default trait method implementations
- Status: IMPLEMENTED
- Evidence: Default methods auto-inherited when not overridden

- Feature: Supertraits (`trait Child extends Parent`)
- Status: PARTIAL
- Evidence: Supertrait syntax accepted; parent trait method inheritance not fully enforced

- Feature: Trait associated types
- Status: PARTIAL
- Evidence: Syntax accepted; not runtime-enforced

- Feature: Trait bounds in generics `<T: Trait>`
- Status: PARTIAL
- Evidence: Parsed; not checked at call time

- Feature: Built-in standard traits (Comparable, Printable, Iterable, Sendable, etc.)
- Status: MISSING
- Evidence: No built-in trait objects pre-defined; users must declare these traits themselves

- Feature: `Default` trait / `Type.default()` dispatch
- Status: MISSING
- Evidence: No built-in `Default` trait; `default_()` builtin is a stub

- Feature: Trait coherence / orphan rules
- Status: MISSING
- Evidence: Not enforced

---

## SECTION: Generics

- Feature: Generic function `func identity<T>(val: T) -> T`
- Status: PARTIAL
- Evidence: `<T>` parameter list accepted; `T` erased and not tracked at runtime

- Feature: Generic struct `struct Box<T>`, generic enum `enum Result<T, E>`
- Status: PARTIAL
- Evidence: Type parameters parsed; erased at runtime

- Feature: Trait bounds `<T: Comparable>`, `where` clauses
- Status: PARTIAL
- Evidence: Parsed; not enforced

- Feature: Const generics
- Status: MISSING
- Evidence: No `const N: usize` syntax in generic parameter list; not in AST

---

## SECTION: Pattern Matching

- Feature: Literal patterns (int, float, str, bool, null)
- Status: IMPLEMENTED
- Evidence: `Pattern::Literal(Expr)`; `matches_pattern()` compares by value equality

- Feature: Variable binding patterns
- Status: IMPLEMENTED
- Evidence: `Pattern::Ident(name)`; `bind_pattern()` defines name in scope

- Feature: Wildcard `_`, OR patterns `case (A | B)`, guard clauses `if cond`
- Status: IMPLEMENTED

- Feature: Range patterns `case (90..=100)`
- Status: IMPLEMENTED
- Evidence: `Pattern::Range { start, end, inclusive }`; integer ranges only

- Feature: Tuple patterns `case ((x, 0))`
- Status: IMPLEMENTED
- Evidence: `Pattern::Tuple(Vec<Pattern>)`; matches `Value::Tuple`

- Feature: Enum variant destructure `case (Shape.Circle(r))`
- Status: IMPLEMENTED
- Evidence: `Pattern::Destructure { path, fields }`; matches `Value::EnumVariant`

- Feature: Result/Option patterns `case (Ok(n))`, `case (Some(v))`, `case (None)`
- Status: IMPLEMENTED
- Evidence: `Pattern::Ok`, `Pattern::Err`, `Pattern::Some`, `Pattern::None`

- Feature: Fixed-size list patterns `case ([a, b, c])`
- Status: IMPLEMENTED
- Evidence: `Pattern::List(Vec<Pattern>)`; matches `Value::List` of exact same length

- Feature: List rest patterns `case ([head, ...tail])`
- Status: MISSING
- Evidence: `Pattern::List` stores a flat `Vec<Pattern>`; no rest/spread element in `Pattern` enum or `parse_single_pattern()`; the `...` token is not handled inside list patterns

- Feature: Struct field patterns `case (Point { x, y })`
- Status: MISSING
- Evidence: No `Pattern::Struct` variant exists; `parse_single_pattern()` does not handle `{` after an identifier; `Pattern::Destructure` only matches `Value::EnumVariant`

- Feature: Type patterns `case (int)`, `case ((n: int))`
- Status: MISSING
- Evidence: No `Pattern::Type` variant in AST; parser has no branch for bare type-name keywords

- Feature: Nested patterns
- Status: IMPLEMENTED
- Evidence: `parse_pattern()` is recursive; `matches_pattern()` recurses into sub-patterns

- Feature: `default` clause
- Status: IMPLEMENTED
- Evidence: `Pattern::Default`; always matches

- Feature: Match exhaustiveness analysis
- Status: MISSING

---

## SECTION: Error Handling

- Feature: `throw expr`
- Status: IMPLEMENTED

- Feature: `try { } catch (e) { } finally { }`
- Status: IMPLEMENTED

- Feature: Typed catch `catch (e: TypeError)`
- Status: MISSING
- Evidence: `TryCatch { catch_var: Option<Ident> }` — no type annotation on catch variable; all errors caught unconditionally

- Feature: `Result` type (`Ok(v)` / `Err(e)`), `Option` type (`Some(v)` / `null`)
- Status: IMPLEMENTED

- Feature: `?` try operator, `try_wrap(fn)`
- Status: IMPLEMENTED

- Feature: Error class hierarchy (TypeError, ValueError, IndexError, etc. as built-in classes)
- Status: MISSING
- Evidence: Errors are plain strings; no built-in `Error` base class or typed exception hierarchy

- Feature: `.ok_or(err)` / `.ok()` methods on Result/Option
- Status: MISSING
- Evidence: No such arms in `call_builtin_method()`

---

## SECTION: Generators

- Feature: `func* gen() { yield val }` declaration
- Status: PARTIAL
- Evidence: Body runs synchronously via `yield_collector`; results collected eagerly

- Feature: `yield expr`
- Status: PARTIAL
- Evidence: `Stmt::Yield`; collected into `Vec<Value>`; not a suspension point

- Feature: `yield*` delegation
- Status: MISSING
- Evidence: No `YieldDelegate` AST node

- Feature: Generator `.next()`
- Status: PARTIAL
- Evidence: `GeneratorState { items, index }`; returns `items[index]` and advances

- Feature: Generator `.send(val)` (two-way communication)
- Status: MISSING
- Evidence: No `.send()` on `GeneratorState`

- Feature: Generator `.collect()`, `.is_done()`
- Status: PARTIAL
- Evidence: Both operate on the pre-collected item list

- Feature: `for (x in gen())` iteration
- Status: IMPLEMENTED
- Evidence: `value_to_iter()` handles `Value::Generator`

- Feature: Infinite generators
- Status: MISSING
- Evidence: Requires true lazy state machine; eager collection would hang

- Feature: `async func*` async generators
- Status: MISSING

---

## SECTION: Async / Await

- Feature: `async func` declaration
- Status: PARTIAL
- Evidence: `is_async` flag set; executes fully synchronously

- Feature: `await expr`
- Status: PARTIAL
- Evidence: `Expr::Await`; simply evaluates the inner expression; no scheduling

- Feature: `Promise` constructors and static methods (`.all`, `.race`, `.any`, `.allSettled`, `.timeout`, `.resolve`, `.reject`)
- Status: MISSING
- Evidence: No `Promise` class defined anywhere; no async event loop

- Feature: Promise instance methods (`.and_then()`, `.map()`, `.catch()`, `.finally()`)
- Status: MISSING

- Feature: `cancel_token()` / `.cancel()` / `.is_cancelled()`
- Status: MISSING

- Feature: `for await (x in asyncGen)` async iteration
- Status: MISSING
- Evidence: No `ForAwait` AST node

- Feature: Multi-worker async runtime
- Status: MISSING

---

## SECTION: Structured Concurrency

- Feature: `task_group()`, `task_scope(fn)`
- Status: STUB
- Evidence: `task_scope` calls the provided function directly; all `__task_group_*` stubs return null

- Feature: `group.spawn(task)`, `group.join_all()`, `group.cancel()`, `group.with_timeout(ms)`
- Status: STUB
- Evidence: All route to `__task_group_*` stubs

---

## SECTION: Channels and Threads

- Feature: `chan_create(size?)`, `chan_send`, `chan_recv`, `chan_close`, `chan_is_closed`
- Status: STUB
- Evidence: `chan_create` returns a dict; `chan_send` discards the value (returns null); `chan_recv` reads from the buffer dict (which is never populated); no OS-level channel

- Feature: `chan_try_send`, `chan_try_recv`, `chan_drain`, `chan_len`, `chan_select`
- Status: STUB

- Feature: `thread_spawn(fn)`, `thread_join(t)`
- Status: STUB
- Evidence: `thread_spawn` returns a random integer; function is NEVER run on another thread; `thread_join` returns null

- Feature: `mutex_create`, `mutex_lock`, `mutex_unlock`, `mutex_with`
- Status: STUB
- Evidence: Lock/unlock are no-ops; `mutex_with` just calls the function directly; no mutual exclusion

- Feature: `rwmutex_*`
- Status: STUB

- Feature: `atomic_new`, `atomic_load`, `atomic_store`, `atomic_add`, `atomic_sub`, `atomic_cas`
- Status: STUB
- Evidence: Operations simulate on dict fields but do not mutate them; no actual atomicity

- Feature: `threadpool_create`, `threadpool_submit`, `threadpool_wait`, `threadpool_destroy`
- Status: STUB
- Evidence: `submit` returns a random int; submitted tasks are never executed

- Feature: `waitgroup_create`, `waitgroup_add`, `waitgroup_done`, `waitgroup_wait`
- Status: STUB

- Feature: `future_get`, `future_is_done`, `future_try_get`
- Status: STUB
- Evidence: `future_is_done` always returns `true`; `future_get` returns null

- Feature: `unsafe_send`
- Status: STUB
- Evidence: Returns first argument unchanged

---

## SECTION: Actors and Agents

- Feature: `actor Name { }`, `agent Name { goal: "..." }` declarations
- Status: PARTIAL
- Evidence: `Stmt::ActorDecl` parsed; stored as dict; no mailbox, message queue, or OS thread

- Feature: `actor_spawn`, `actor_send`, `actor_receive`, `actor_call`, `actor_stop`, `actor_is_alive`
- Status: STUB

- Feature: `agent_create`, `agent_set_goal`, `agent_get_state`, `agent_set_state`, `agent_run`, `agent_done`
- Status: STUB

---

## SECTION: Isolates

- Feature: `isolate { body }` block
- Status: IMPLEMENTED
- Evidence: `Stmt::IsolateBlock`; executes body in a fresh interpreter instance

- Feature: `isolate_new`, `isolate_get`, `isolate_set`, `isolate_exec`, `isolate_run`
- Status: STUB

---

## SECTION: Macros

- Feature: `macro name!(params) { body }` declaration and invocation
- Status: IMPLEMENTED
- Evidence: `Stmt::MacroDecl`; `Expr::MacroCall`; textual parameter substitution + body evaluation

- Feature: Macro hygiene
- Status: PARTIAL
- Evidence: Parameters substituted textually; no hygienic renaming of internal variables

- Feature: `ct_set_macro_limit` / `ct_get_macro_limit`
- Status: IMPLEMENTED

---

## SECTION: Compile-Time Execution

- Feature: `comptime { body }` block
- Status: IMPLEMENTED
- Evidence: `Stmt::ComptimeBlock`; executes immediately during normal interpretation

- Feature: `ct_platform()` / `ct_arch()`
- Status: IMPLEMENTED
- Evidence: Returns OS/arch strings from `std::env::consts`

- Feature: `ct_word_exists(name)`, `ct_list_funcs()`
- Status: IMPLEMENTED

- Feature: `ct_get_effects()`
- Status: PARTIAL
- Evidence: Returns an empty list; no actual effect tracking

- Feature: `ct_unregister()`, `ct_emit()`, `ct_error()`, `ct_warn()`
- Status: PARTIAL
- Evidence: `ct_emit` prints to stdout; others are no-ops

- Feature: `ct_feature(name)`
- Status: STUB
- Evidence: Returns `false` unconditionally

- Feature: `mem_size_of(type)`
- Status: IMPLEMENTED
- Evidence: Returns hardcoded sizes for known type names

---

## SECTION: Destructuring

- Feature: `let [a, b] = list`, `let (a, b) = tuple`, `let { x, y } = struct_or_dict`
- Status: IMPLEMENTED

- Feature: `for ([a, b] in iter)` destructuring for-in
- Status: IMPLEMENTED
- Evidence: `Stmt::ForInDestructure`

- Feature: `if let Some(x) = expr`, `while let`, `let Some(x) = expr else { ... }`
- Status: IMPLEMENTED
- Evidence: `Stmt::IfLet` / `Stmt::WhileLet` / `Stmt::LetElse`

---

## SECTION: Comprehensions

- Feature: List comprehension `[expr for x in iter if cond]`
- Status: IMPLEMENTED
- Evidence: `Expr::ListComp`

- Feature: Dict comprehension `{k: v for x in iter if cond}`
- Status: IMPLEMENTED
- Evidence: `Expr::DictComp`

- Feature: Set comprehension `#{expr for x in iter if cond}`
- Status: IMPLEMENTED
- Evidence: `Expr::SetComp`

- Feature: Nested comprehensions (multiple `for` clauses)
- Status: MISSING
- Evidence: `ListComp`/`DictComp`/`SetComp` AST nodes each hold a single iterator variable; no chained `for` clauses

- Feature: Generator comprehension `(expr for x in iter)`
- Status: MISSING
- Evidence: No `GenComp` AST node

---

## SECTION: Extension Methods

- Feature: `impl TypeName { func ... }` on user-defined types
- Status: IMPLEMENTED
- Evidence: `Stmt::ImplBlock`; methods added to target's `ClassValue.methods`

- Feature: `impl` on primitive types (`str`, `int`, `list`, etc.)
- Status: MISSING
- Evidence: `ImplBlock.target` looked up as identifier in environment; primitive types not stored as `ClassValue`; impl blocks for primitives are silently ignored

---

## SECTION: Operator Overloading

- Feature: `__add__`, `__sub__`, `__mul__`, `__div__`, `__mod__`, `__pow__`
- Status: IMPLEMENTED

- Feature: `__eq__`, `__ne__`, `__lt__`, `__gt__`, `__le__`, `__ge__`, `__str__`, `__len__`, `__contains__`, `__getitem__`, `__setitem__`
- Status: IMPLEMENTED

- Feature: `__neg__` (unary minus on instances)
- Status: MISSING
- Evidence: `unary_op()` handles `Neg` only for `Int`/`Float`; no instance method dispatch

- Feature: `__not__` (unary `!` on instances)
- Status: MISSING
- Evidence: `unary_op()` for `Not` only calls `.is_truthy()`

- Feature: `__floordiv__`, `__band__`, `__bor__`, `__bxor__`, `__bnot__`, `__lshift__`, `__rshift__`
- Status: MISSING
- Evidence: `binary_op()` instance dispatch has no arms for bitwise or floor-div operators

- Feature: `__getslice__` / `__setslice__`
- Status: MISSING
- Evidence: `eval_slice()` does not invoke instance methods

---

## SECTION: Union Types

- Feature: `int | str` union type annotation syntax
- Status: PARTIAL
- Evidence: Parsed as type strings; not enforced at variable binding or function call

- Feature: `is` type narrowing, `T | null` nullable pattern
- Status: IMPLEMENTED

- Feature: Exhaustiveness in `match` on union types
- Status: MISSING

---

## SECTION: The `never` Type

- Feature: `-> never` return type annotation
- Status: PARTIAL
- Evidence: Accepted as annotation string; not enforced

- Feature: Dead-code detection after `never`-returning calls
- Status: MISSING

---

## SECTION: Effects System

- Feature: `[effects: net, io, fs]` annotation syntax
- Status: PARTIAL
- Evidence: Parsed; stored as list of strings; not checked at call time

- Feature: `pure` keyword enforcement
- Status: PARTIAL
- Evidence: `is_pure` flag set; no verification that body performs no side effects

- Feature: `--warn effects` CLI flag
- Status: MISSING
- Evidence: No effect-checking pass

- Feature: `ct_get_effects()`
- Status: PARTIAL
- Evidence: Returns empty list; no effect tracking

---

## SECTION: Type Aliases and Newtypes

- Feature: `type Name = OtherType`
- Status: PARTIAL
- Evidence: `Stmt::TypeAlias`; stored as a string in environment; no semantic aliasing

- Feature: `newtype Name = InnerType`
- Status: PARTIAL
- Evidence: `Stmt::NewtypeDecl`; stores `"newtype:InnerType"` string; `.0` accessor and `.inner()` method not implemented; type-safety between different newtypes not enforced

---

## SECTION: Weak References

- Feature: `weak_ref(val)` — create a weak reference
- Status: STUB
- Evidence: `"weak_ref"` routes to catch-all stub; returns a dict with `type="weakref"` but no actual weak semantics

- Feature: `weak.get()` / `weak.is_alive()`
- Status: MISSING
- Evidence: No weak-reference methods in `call_builtin_method()`

- Feature: `unwrap_or_default()`
- Status: STUB

---

## SECTION: Block Expressions and Lazy

- Feature: `do { stmts; last_expr }` block expression
- Status: IMPLEMENTED
- Evidence: `Expr::DoBlock`; executes body stmts, returns last expression value

- Feature: `lazy expr`
- Status: IMPLEMENTED
- Evidence: `Expr::Lazy` → `Value::Lazy(Box<Expr>)`; re-evaluated each time the value is read

- Feature: `is_lazy(val)`
- Status: IMPLEMENTED

---

## SECTION: Labels and Goto

- Feature: `label name:` / `goto name`
- Status: IMPLEMENTED
- Evidence: `goto` scans forward in current block's statement slice; limited to same block scope

---

## SECTION: `using` Keyword

- Feature: `using expr { block }` — inject fields into scope
- Status: IMPLEMENTED
- Evidence: `Stmt::Using`; each field of the expression defined as a local variable

---

## SECTION: Static Assertions

- Feature: `static_assert(cond, msg?)`
- Status: IMPLEMENTED
- Evidence: `"static_assert"` in `call_builtin()`; panics at runtime if condition is false

---

## SECTION: Module Visibility

- Feature: `pub`, `private`, `internal` access modifiers
- Status: MISSING
- Evidence: Parsed as annotations; not enforced — all names accessible from any file

- Feature: `pub(crate)` / `pub(super)` scoped visibility
- Status: MISSING

---

## SECTION: Imports

- Feature: `import "path"`, `import { names } from "path"`, `import "path" as alias`
- Status: IMPLEMENTED

- Feature: `import std.module`
- Status: IMPLEMENTED
- Evidence: `"std."` prefix recognized; returns pre-built module dict

- Feature: URL imports (`import "https://..."`)
- Status: MISSING
- Evidence: `exec_import()` does not handle URL schemes; only `http_import_register()` for testing

---

## SECTION: Inline Assembly and Embedded Languages

- Feature: `asm! { ... }` block
- Status: PARTIAL
- Evidence: `Stmt::AsmBlock` parsed; interpreter emits a warning and no-ops

- Feature: `@py { code }`, `@js { code }` embedded blocks
- Status: PARTIAL
- Evidence: `Stmt::EmbeddedLangBlock` parsed; stored as string; never executed

- Feature: `enable { py, js }` directive
- Status: PARTIAL
- Evidence: Parsed; no engine loading

- Feature: `register_engine(path, name)`
- Status: STUB

---

## SECTION: Source Directives

- Feature: `@insert "path"` — inline another source file
- Status: IMPLEMENTED
- Evidence: `Stmt::SourceDirective { kind: "insert" }`; reads and executes the file

- Feature: `@replace`, `@cfg`, `@borrow_check`
- Status: PARTIAL
- Evidence: Parsed; `@borrow_check` logs a message; all effectively no-ops

---

## SECTION: Memory Safety and Borrowing

- Feature: `borrow(x)`, `borrow_mut(x)`, `deref(x)`
- Status: STUB
- Evidence: All return first argument unchanged; no borrow-checker state

- Feature: Move semantics
- Status: MISSING
- Evidence: All values cloned on assignment/pass; no move tracking

- Feature: Borrow checker
- Status: MISSING

---

## SECTION: Manual Memory Management

- Feature: `mem_alloc(size)`, `mem_alloc_zeroed(size)`, `mem_realloc(ptr, size)`, `mem_free(ptr)`
- Status: IMPLEMENTED
- Evidence: Allocation table in interpreter state; `Value::Pointer` returned

- Feature: `mem_read(ptr, idx)`, `mem_write(ptr, idx, val)`, `mem_copy(dst, src, size)`, `mem_set(ptr, byte, size)`
- Status: IMPLEMENTED
- Evidence: Bounds-checked access into allocation table

- Feature: `mem_size_of(type)`
- Status: IMPLEMENTED

- Feature: Use-after-free detection (`--strict-unsafe`)
- Status: IMPLEMENTED
- Evidence: `fault.rs` + interpreter checks on `mem_read`/`mem_write` after `mem_free`

- Feature: Leak detection (`--sanitizer leak`)
- Status: IMPLEMENTED
- Evidence: Unreleased allocations reported at interpreter shutdown

---

## SECTION: Vectors and Tensors

- Feature: `vec_new`, `vec_from`, `vec_get`, `vec_set`, `vec_len`, `vec_add`, `vec_sub`, `vec_mul`, `vec_div`, `vec_scale`, `vec_dot`, `vec_norm`, `vec_normalize`, `vec_sum`, `vec_min`, `vec_max`, `vec_clamp`, `vec_copy`, `vec_to_list`
- Status: IMPLEMENTED
- Evidence: All in `call_builtin()`

- Feature: `tensor_new`, `tensor_from`, `tensor_get`, `tensor_set`, `tensor_shape`, `tensor_rank`, `tensor_size`, `tensor_add`, `tensor_sub`, `tensor_mul`, `tensor_div`, `tensor_scale`, `tensor_matmul`, `tensor_transpose`, `tensor_reshape`, `tensor_slice`, `tensor_fill`, `tensor_copy`, `tensor_sum`, `tensor_mean`, `tensor_min`, `tensor_max`, `tensor_argmin`, `tensor_argmax`, `tensor_softmax`, `tensor_relu`, `tensor_to_list`, `tensor_from_list`
- Status: IMPLEMENTED
- Evidence: All in `call_builtin()`

---

## SECTION: Testing

- Feature: `test "name" { body }` blocks
- Status: IMPLEMENTED
- Evidence: Recognized in `--test` mode; results printed

- Feature: `bench "name" { body }` blocks
- Status: IMPLEMENTED
- Evidence: Timed execution in `--test` mode

- Feature: `assert(cond, msg?)`, `assert_eq(a, b)`, `assert_ne(a, b)`
- Status: IMPLEMENTED

- Feature: `std.test` module hooks (`before_all`, `after_all`, `before_each`, `after_each`, `each`, `snapshot`, `skip`, `todo`, `property`)
- Status: STUB
- Evidence: All entries are `__test_*` stubs returning null

---

## SECTION: List Methods (Comprehensive)

- Feature: `.len()`, `.is_empty()`, `.first()`, `.last()`
- Status: IMPLEMENTED

- Feature: `.push(item)`, `.pop(i?)`
- Status: IMPLEMENTED
- Evidence: Mutation applied to list in-place via environment

- Feature: `.insert(i, val)`, `.remove(val)`, `.clear()`, `.extend(other)`
- Status: PARTIAL
- Evidence: Methods recognized; several return error "not yet implemented for in-place mutation"; list method calls on `Value::List` clones do not propagate back to the variable

- Feature: `.contains(val)`, `.index_of(val)`, `.count(val)`, `.find(pred)`, `.any(pred)`, `.all(pred)`
- Status: IMPLEMENTED

- Feature: `.unique()`, `.flatten()`, `.flat_map(fn)`, `.sort()`, `.sort_by(fn)`, `.reverse()`
- Status: IMPLEMENTED

- Feature: `.join(sep)`, `.map(fn)`, `.filter(fn)`, `.reduce(fn, init)`, `.for_each(fn)`, `.each(fn)`
- Status: IMPLEMENTED

- Feature: `.slice(start, end)`, `.take(n)`, `.drop(n)`
- Status: IMPLEMENTED

- Feature: `.sum()`, `.product()`, `.min()`, `.max()`
- Status: IMPLEMENTED

- Feature: `.enumerate()`, `.zip(other)`, `.partition(pred)`, `.group_by(fn)`
- Status: IMPLEMENTED

---

## SECTION: Dict Methods (Comprehensive)

- Feature: `.keys()`, `.values()`, `.items()`, `.len()`, `.is_empty()`
- Status: IMPLEMENTED

- Feature: `.has(key)`, `.get(key, default?)`, `.contains(key)`
- Status: IMPLEMENTED

- Feature: `.set(key, val)`, `.remove(key)`, `.update(other)`, `.merge(other)`, `.clear()`
- Status: PARTIAL
- Evidence: Same in-place mutation issue as list; calls on a `Value::Dict` clone do not propagate back to the variable

---

## SECTION: Set Methods

- Feature: Set union (`a | b`), difference (`a - b`)
- Status: IMPLEMENTED
- Evidence: `BinOp::BitOr` / `BinOp::Sub` arms for `Value::Set` in `binary_op()`

- Feature: Set intersection (`a & b`)
- Status: PARTIAL
- Evidence: `BinOp::BitAnd` on sets may or may not be implemented; needs verification

- Feature: `.sym_difference()`, `.is_subset()`, `.is_superset()`, `.is_disjoint()`
- Status: MISSING
- Evidence: No such arms in `call_builtin_method()` for `Value::Set`

---

## SECTION: Standard Library Modules

- Feature: `std.math`
- Status: IMPLEMENTED
- Evidence: Maps to real `abs`, `sqrt`, `pow`, `floor`, `ceil`, `round`, `sin`, `cos`, `tan`, `log`, `mean`, `median`, `stddev` builtins

- Feature: `std.io`
- Status: IMPLEMENTED
- Evidence: `stdout`, `stderr`, `stdin` objects and file operation functions mapped to real builtins

- Feature: `std.collections`
- Status: PARTIAL
- Evidence: `deque_*` fully implemented; `stack`, `queue`, `priority_queue`, `sorted_map`, `linked_list`, `multiset` are `__col_*` stubs

- Feature: `std.serialize`
- Status: PARTIAL
- Evidence: JSON encode/decode map to real builtins; toml/yaml/csv/msgpack/proto are stubs

- Feature: `std.rand`
- Status: PARTIAL
- Evidence: `float()`, `int()`, `choice()`, `shuffle()` mapped; `seed()`, `bytes()`, `uuid()`, `gauss()`, `sample()`, `weighted_choice()` are stubs

- Feature: `std.time`
- Status: PARTIAL
- Evidence: `now()`, `now_utc()`, `timestamp()` mapped; `parse()`, `format()`, `duration()`, `timezone()`, `timer_*` are stubs

- Feature: `std.os`
- Status: PARTIAL
- Evidence: `getenv()`, `exit()`, `platform()`, `arch()` mapped; `hostname()`, `pid()`, `setenv()`, `environ()`, `signal.*`, `path.*` are stubs

- Feature: `std.fs`
- Status: STUB
- Evidence: All functions (`read`, `write`, `append`, `exists`, `delete`, `ls`, `mkdir`, `rmdir`, `stat`, `walk`, `glob`, `copy`, `move`, `rename`, `abs_path`, `basename`, `dirname`, `ext`, `stem`) are `__fs_*` stubs

- Feature: `std.fmt`
- Status: STUB

- Feature: `std.regex`
- Status: STUB

- Feature: `std.iter`
- Status: STUB

- Feature: `std.proc`
- Status: STUB

- Feature: `std.log`
- Status: STUB

- Feature: `std.test`
- Status: STUB

- Feature: `std.hash`
- Status: STUB

- Feature: `std.cache`
- Status: STUB

- Feature: `std.uuid`
- Status: STUB

- Feature: `std.csv`
- Status: STUB

- Feature: `std.toml`
- Status: STUB

- Feature: `std.yaml`
- Status: STUB

- Feature: `std.term`
- Status: STUB

- Feature: `std.cli`
- Status: STUB

- Feature: `std.net`
- Status: STUB

- Feature: `std.http`
- Status: STUB

- Feature: `std.crypto`
- Status: STUB
- Evidence: Hash/encode functions return formatted input string from catch-all

- Feature: `std.signal`
- Status: PARTIAL
- Evidence: `on_fault()`, `dump_json()`, `list()` IMPLEMENTED; `set_recovery_point()`, `recover()`, `dump_core()` return explicit "not yet implemented" errors; `on()`, `once()`, `off()`, `reset()`, `ignore()`, `raise()`, `alarm()` are no-ops

- Feature: `std.ffi`
- Status: STUB

- Feature: `std.compress`, `std.xml`, `std.image`, `std.mail`, `std.gfx3d`, `std.game`, `std.db`, `std.ui`, `std.audio`, `std.video`, `std.pdf`, `std.excel`, `std.jwt`, `std.oauth2`, `std.i18n`, `std.watch`, `std.grpc`, `std.mqtt`, `std.embed`, `std.template`, `std.multipart`, `std.ssh`, `std.qr`, `std.markdown`, `std.archive`, `std.dns`, `std.2d`, `std.graphql`, `std.webrtc`, `std.clipboard`, `std.notify`, `std.speech`, `std.camera`, `std.serial`, `std.usb`, `std.bluetooth`, `std.hotkey`, `std.tray`, `std.ipc`, `std.decimal`, `std.diff`, `std.semver`, `std.geo`, `std.gpu`, `std.accessibility`, `std.blockchain`, `std.parse`, `std.config`, `std.event`, `std.diag`, `std.iot`, `std.hal`, `std.office`, `std.money`, `std.dotenv`, `std.scrape`, `std.map`, `std.task`, `std.phone`, `std.barcode`, `std.ml.vision`, `std.ml.audio`, `std.ai`
- Status: STUB
- Evidence: All entries are `__<module>_*` stubs routed through the catch-all handler

---

## SECTION: Core Builtins (Miscellaneous)

- Feature: `print`, `println`, `input`, `eprint`, `eprintln`
- Status: IMPLEMENTED

- Feature: `read_file`, `write_file`, `append_file`, `file_exists`, `delete_file`
- Status: IMPLEMENTED

- Feature: `file_open`, `file_read`, `file_close`
- Status: PARTIAL
- Evidence: `__io_open`/`__io_read`/`__io_close` route to catch-all stub returning empty dict/string

- Feature: `len`, `type`, `typeof`, `str`, `int`, `float`, `bool`, `list`, `dict`, `set`, `tuple`
- Status: IMPLEMENTED

- Feature: `hex(n)`, `bin(n)`, `oct(n)`, `chr(n)`, `ord(c)`
- Status: IMPLEMENTED

- Feature: `abs`, `min`, `max`, `sum`, `sqrt`, `pow`, `floor`, `ceil`, `round`
- Status: IMPLEMENTED

- Feature: `mean`, `median`, `stddev`
- Status: IMPLEMENTED

- Feature: `sorted`, `reversed`, `zip`, `enumerate`
- Status: IMPLEMENTED

- Feature: `random()`, `random_int(lo, hi)`, `random_choice(list)`, `shuffle(list)`
- Status: IMPLEMENTED

- Feature: `sleep(ms)`, `getenv(name, default?)`
- Status: IMPLEMENTED

- Feature: `json_parse`, `json_stringify`
- Status: IMPLEMENTED

- Feature: `clone`, `hash`, `callable`, `vars`, `dir`, `hasattr`, `getattr`, `setattr`
- Status: IMPLEMENTED

- Feature: `eval(code_str)`, `exec(code_str)`
- Status: IMPLEMENTED

- Feature: `Ok(v)`, `Err(e)`, `Some(v)`, `is_ok`, `is_err`, `is_some`, `is_none`, `is_func`
- Status: IMPLEMENTED

- Feature: `unwrap`, `unwrap_or`, `unwrap_err`, `try_wrap`
- Status: IMPLEMENTED

- Feature: `freeze`, `is_frozen`, `is_lazy`, `memo`, `patch`
- Status: IMPLEMENTED

- Feature: `from_pairs`, `items`, `str_from_bytes`, `chars`
- Status: IMPLEMENTED

- Feature: `sort_by(list, fn)` (standalone)
- Status: IMPLEMENTED

- Feature: `http_import_register(url, source)`
- Status: IMPLEMENTED

- Feature: `resize(val, n)` / `to_size(val, type_name)`
- Status: MISSING
- Evidence: Not found in `call_builtin()`

- Feature: `default_(type)` — construct default value for a type
- Status: STUB

- Feature: `weak_ref(val)`
- Status: STUB

- Feature: `static_assert(cond, msg?)`
- Status: IMPLEMENTED

---

## Summary Table

| Feature Category | IMPLEMENTED | PARTIAL | MISSING | STUB |
|---|:---:|:---:|:---:|:---:|
| Arithmetic / Comparison / Logical / Bitwise operators | 14 | 0 | 0 | 0 |
| Assignment operators | 3 | 0 | 0 | 0 |
| Other operators (ternary, ??, ?., pipe, cast, etc.) | 9 | 4 | 1 | 0 |
| F-strings and string literals | 5 | 0 | 1 | 0 |
| String methods | 23 | 0 | 2 | 0 |
| Variables / constants / scoping | 5 | 0 | 0 | 0 |
| Primitive types | 10 | 1 | 2 | 0 |
| Control flow | 12 | 0 | 0 | 0 |
| Functions | 7 | 3 | 0 | 0 |
| Lambdas | 3 | 2 | 0 | 0 |
| Decorators | 7 | 1 | 1 | 0 |
| Classes (OOP) | 14 | 0 | 5 | 0 |
| Structs | 3 | 3 | 0 | 0 |
| Enums | 3 | 1 | 1 | 0 |
| Traits | 4 | 3 | 3 | 0 |
| Generics | 0 | 4 | 1 | 0 |
| Pattern matching | 8 | 0 | 4 | 0 |
| Error handling | 5 | 0 | 3 | 0 |
| Generators | 1 | 4 | 3 | 0 |
| Async / Await | 0 | 2 | 5 | 0 |
| Structured concurrency | 0 | 0 | 0 | 2 |
| Channels / Threads / Sync primitives | 0 | 0 | 0 | 15 |
| Actors / Agents | 0 | 2 | 0 | 2 |
| Isolates | 1 | 0 | 0 | 1 |
| Macros | 2 | 1 | 0 | 0 |
| Compile-time execution | 4 | 3 | 0 | 1 |
| Destructuring | 4 | 0 | 0 | 0 |
| Comprehensions | 3 | 0 | 2 | 0 |
| Extension methods | 1 | 0 | 1 | 0 |
| Operator overloading | 7 | 0 | 5 | 0 |
| Union types / never type | 2 | 2 | 2 | 0 |
| Effects system | 0 | 3 | 1 | 0 |
| Type aliases / Newtypes | 0 | 2 | 0 | 0 |
| Weak references | 0 | 0 | 1 | 1 |
| Block expressions / lazy | 3 | 0 | 0 | 0 |
| Module visibility | 0 | 0 | 2 | 0 |
| Imports | 4 | 0 | 1 | 0 |
| Inline asm / Embedded langs | 0 | 3 | 0 | 1 |
| Memory management | 7 | 0 | 2 | 0 |
| Vectors / Tensors | 30 | 0 | 0 | 0 |
| Testing | 4 | 0 | 0 | 1 |
| List methods | 12 | 2 | 0 | 0 |
| Dict methods | 4 | 2 | 0 | 0 |
| Set methods | 2 | 1 | 1 | 0 |
| std.math / std.io | 2 | 0 | 0 | 0 |
| std.collections / std.serialize / std.rand / std.time / std.os | 0 | 5 | 0 | 0 |
| std.fs / std.fmt / std.regex / std.iter / std.proc / std.log | 0 | 0 | 0 | 6 |
| std.hash / std.cache / std.uuid / std.csv / std.toml / std.yaml | 0 | 0 | 0 | 6 |
| std.term / std.cli / std.net / std.http / std.crypto / std.signal | 0 | 1 | 0 | 5 |
| std.ffi + 60+ other stdlib modules | 0 | 0 | 0 | 60+ |
| Misc core builtins | 18 | 1 | 2 | 2 |
