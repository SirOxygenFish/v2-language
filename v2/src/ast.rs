/// AST node definitions for V2.

pub type Ident = String;

#[derive(Debug, Clone)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}

// ── Statements ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Stmt {
    /// `let x = expr` or `let x: type = expr`
    Let {
        name: Ident,
        type_ann: Option<String>,
        value: Option<Expr>,
    },
    /// `const X = expr`
    Const {
        name: Ident,
        value: Expr,
        doc_comment: Option<String>,
    },
    /// Expression statement (e.g. a function call on its own line)
    Expr(Expr),
    /// `func name(params) { body }`
    FuncDecl {
        name: Ident,
        params: Vec<Param>,
        body: Vec<Stmt>,
        is_pure: bool,
        is_async: bool,
        is_generator: bool,
        decorators: Vec<Expr>,
        doc_comment: Option<String>,
    },
    /// `return expr?`
    Return(Option<Expr>),
    /// `if (cond) { body } else if ... else { ... }`
    If {
        condition: Expr,
        body: Vec<Stmt>,
        else_ifs: Vec<(Expr, Vec<Stmt>)>,
        else_body: Option<Vec<Stmt>>,
    },
    /// `while (cond) { body }`
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },
    /// `if let Pattern(var) = expr { body } else { else_body }`
    IfLet {
        pattern: Ident,   // "Some" or "Ok"
        var: Ident,
        expr: Expr,
        body: Vec<Stmt>,
        else_body: Option<Vec<Stmt>>,
    },
    /// `while let Pattern(var) = expr { body }`
    WhileLet {
        pattern: Ident,   // "Some" or "Ok"
        var: Ident,
        expr: Expr,
        body: Vec<Stmt>,
    },
    /// `let Pattern(var) = expr else { diverging_body }`
    LetElse {
        pattern: Ident,   // "Some" or "Ok"
        var: Ident,
        expr: Expr,
        else_body: Vec<Stmt>,
    },
    /// `test "name" { body }` — inline test block (only runs in test mode)
    TestBlock {
        name: String,
        body: Vec<Stmt>,
    },
    /// `bench "name" { body }` — benchmark block (only runs in test mode)
    BenchBlock {
        name: String,
        body: Vec<Stmt>,
    },
    /// `for (ident in expr) { body }` or `for (let i = 0; i < n; i += 1) { body }`
    ForIn {
        var: Ident,
        iter: Expr,
        body: Vec<Stmt>,
    },
    /// for ([a, b] in expr) { body } -- destructuring for-in
    ForInDestructure {
        vars: Vec<Ident>,
        iter: Expr,
        body: Vec<Stmt>,
    },
    /// Classic C-style for: for (init; cond; update) { body }
    ForClassic {
        init: Option<Box<Stmt>>,
        condition: Option<Expr>,
        update: Option<Box<Stmt>>,
        body: Vec<Stmt>,
    },
    /// break or break label
    Break,
    BreakLabel(Ident),
    /// continue or continue label
    Continue,
    ContinueLabel(Ident),
    /// `match (expr) { case (pattern) { body } ... }`
    Match {
        subject: Expr,
        arms: Vec<MatchArm>,
    },
    /// `throw expr`
    Throw(Expr),
    /// `try { body } catch (e) { handler } finally { cleanup }`
    /// catch_clauses: Vec of (var_name, optional_type_filter, body)
    TryCatch {
        body: Vec<Stmt>,
        catch_var: Option<Ident>,
        catch_body: Option<Vec<Stmt>>,
        catch_type: Option<Ident>,
        catch_clauses: Vec<(Option<Ident>, Option<Ident>, Vec<Stmt>)>,
        finally_body: Option<Vec<Stmt>>,
    },
    /// `defer { body }`
    Defer(Vec<Stmt>),
    /// `class Name extends Parent? { body }`
    ClassDecl {
        name: Ident,
        parent: Option<Ident>,
        body: Vec<Stmt>,
        decorators: Vec<Ident>,
        is_sealed: bool,
        doc_comment: Option<String>,
    },
    /// `struct Name { fields }`
    StructDecl {
        name: Ident,
        fields: Vec<StructField>,
        decorators: Vec<String>,
        doc_comment: Option<String>,
    },
    /// `enum Name { variants }`
    EnumDecl {
        name: Ident,
        variants: Vec<EnumVariant>,
        doc_comment: Option<String>,
    },
    /// `trait Name { methods }`
    TraitDecl {
        name: Ident,
        supertraits: Vec<Ident>,
        methods: Vec<Stmt>,
        doc_comment: Option<String>,
    },
    /// `impl Trait for Type { methods }`
    ImplBlock {
        trait_name: Option<Ident>,
        target: Ident,
        methods: Vec<Stmt>,
    },
    /// `import "path"` or `import { names } from "path"` etc.
    Import {
        path: String,
        names: Option<Vec<Ident>>,
        alias: Option<Ident>,
    },
    /// Assignment: `target = value` or `target += value` etc.
    Assign {
        target: Expr,
        op: AssignOp,
        value: Expr,
    },
    /// `label name:`
    Label(Ident),
    /// `goto name`
    Goto(Ident),
    /// `yield expr`
    Yield(Option<Expr>),
    /// Block: `{ stmts }`
    Block(Vec<Stmt>),
    /// Multiple statements without a new scope (for destructuring)
    Multi(Vec<Stmt>),
    /// `type Name = Type`
    TypeAlias {
        name: Ident,
        value: Ident,
        doc_comment: Option<String>,
    },
    /// `using expr { block }` or `using expr` (flat)
    Using {
        expr: Expr,
        body: Option<Vec<Stmt>>,
    },
    /// `static_assert(condition, message)`
    StaticAssert {
        condition: Expr,
        message: String,
    },
    /// `macro name!(a, b) { body }`
    MacroDecl {
        name: Ident,
        params: Vec<Ident>,
        body: Vec<Stmt>,
    },
    /// `newtype Name = Type`
    NewtypeDecl {
        name: Ident,
        inner_type: Ident,
        doc_comment: Option<String>,
    },
    /// `comptime { body }`
    ComptimeBlock {
        body: Vec<Stmt>,
    },
    /// `cstruct Name { fields }`
    CStructDecl {
        name: Ident,
        fields: Vec<StructField>,
        doc_comment: Option<String>,
    },
    /// `unsafe { body }`
    UnsafeBlock {
        body: Vec<Stmt>,
    },
    /// `actor Name { body }` or `agent Name { body }`
    ActorDecl {
        name: Ident,
        is_agent: bool,
        goal: Option<String>,
        body: Vec<Stmt>,
    },
    /// `isolate { body }` or `isolate(name) { body }`
    IsolateBlock {
        name: Option<Expr>,
        body: Vec<Stmt>,
    },
    /// `bitfield struct Name { field: bits, ... }`
    BitfieldStructDecl {
        name: Ident,
        backing: Option<String>,
        fields: Vec<(Ident, u8)>,
        doc_comment: Option<String>,
    },
    /// `@inline struct` (parsed same as struct but with inline flag)
    InlineStructDecl {
        name: Ident,
        fields: Vec<StructField>,
        doc_comment: Option<String>,
    },
    /// `enable { lang1, lang2 }` or `register_engine(path, name)`
    EnableLangs {
        langs: Vec<String>,
    },
    /// `@py { code }`, `@js { code }` etc
    EmbeddedLangBlock {
        lang: String,
        label: Option<String>,
        code: String,
    },
    /// `@import { a, b as c } from <selector>` — import symbols exported by
    /// embedded engine blocks or foreign modules (e.g. `py.statistics`).
    EngineImport {
        names: Vec<(Ident, Option<Ident>)>,
        wildcard: bool,
        selector: String,
    },
    /// `asm! { code }`
    AsmBlock {
        code: String,
    },
    /// Source directives: `@replace`, `@insert`, `@borrow_check`, `@cfg`
    SourceDirective {
        kind: String,
        args: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub enum AssignOp {
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    PercentAssign,
    DoubleStarAssign,
    ShlAssign,
    ShrAssign,
    BitAndAssign,
    BitOrAssign,
    BitXorAssign,
    IntDivAssign,
}

// ── Expressions ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Expr {
    /// Integer literal
    Int(i64),
    /// Arbitrary-precision integer literal (decimal digit string, too big for i64)
    BigIntLit(String),
    /// Float literal
    Float(f64),
    /// String literal
    Str(String),
    /// F-string template: f"hello ${name}"
    FStr(String),
    /// Tagged template literal: `tag f"...${x}..."`. The tag function receives
    /// the literal string parts as a list and the interpolated values as varargs.
    TaggedTemplate { tag: Box<Expr>, template: String },
    ByteStr(Vec<u8>),
    /// Boolean literal
    Bool(bool),
    /// null
    Null,
    /// Identifier reference
    Ident(Ident),
    /// self
    Self_,

    /// Binary operation: left op right
    BinOp {
        left: Box<Expr>,
        op: BinOp,
        right: Box<Expr>,
    },
    /// Unary operation: op expr
    UnaryOp {
        op: UnaryOp,
        expr: Box<Expr>,
    },

    /// Function call: callee(args)
    Call {
        callee: Box<Expr>,
        args: Vec<CallArg>,
    },
    /// Macro call: name!(args)
    MacroCall {
        name: Ident,
        args: Vec<CallArg>,
    },
    /// Method call: obj.method(args)
    MethodCall {
        object: Box<Expr>,
        method: Ident,
        args: Vec<CallArg>,
        optional: bool,
    },
    /// Field access: obj.field
    FieldAccess {
        object: Box<Expr>,
        field: Ident,
        optional: bool,
    },
    /// Index: obj[index]
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    /// Slice: obj[start:end] or obj[start:end:step]
    Slice {
        object: Box<Expr>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
    },

    /// List literal: [a, b, c]
    List(Vec<Expr>),
    /// List comprehension: [expr for var in iterable if cond] (supports multiple for clauses)
    ListComp {
        expr: Box<Expr>,
        clauses: Vec<CompClause>,
    },
    /// Dict comprehension: {kexpr: vexpr for var in iterable if cond}
    DictComp {
        key_expr: Box<Expr>,
        val_expr: Box<Expr>,
        clauses: Vec<CompClause>,
    },
    /// Set comprehension: #{expr for var in iterable if cond}
    SetComp {
        expr: Box<Expr>,
        clauses: Vec<CompClause>,
    },
    /// Generator comprehension: (expr for var in iterable if cond)
    GenComp {
        expr: Box<Expr>,
        clauses: Vec<CompClause>,
    },
    /// Dict literal: { "key": val, ... }
    Dict(Vec<(Expr, Expr)>),
    /// Tuple literal: (a, b, c)  — only when 2+ elements or trailing comma
    Tuple(Vec<Expr>),
    /// Set literal: {a, b, c} (context-sensitive)
    Set(Vec<Expr>),

    /// Lambda: lambda(params) => body  or  lambda(params) { body }
    Lambda {
        params: Vec<Param>,
        body: Box<Expr>,
        is_move: bool,
    },
    /// Lambda with block body (returns last expression or null)
    LambdaBlock {
        params: Vec<Param>,
        body: Vec<Stmt>,
        is_move: bool,
    },

    /// Ternary: cond ? then : else
    Ternary {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },

    /// Range: start..end or start..=end
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        inclusive: bool,
    },

    /// Spread: ...expr
    Spread(Box<Expr>),

    /// `new ClassName(args)`
    New {
        class: Ident,
        args: Vec<CallArg>,
    },

    /// `await expr`
    Await(Box<Expr>),

    /// Struct literal: StructName { field: val, ... }
    StructLit {
        name: Ident,
        fields: Vec<(Ident, Expr)>,
        spread: Option<Box<Expr>>,
    },

    /// `typeof expr` — at compile time
    TypeOf(Box<Expr>),

    /// `do { stmts }` — block expression, value is last expression
    DoBlock(Vec<Stmt>),

    /// Match expression: `match (val) { case (P) { expr } }`
    MatchExpr {
        subject: Box<Expr>,
        arms: Vec<MatchArm>,
    },

    /// Pipe chain stored during parsing (desugared to nested calls)
    Pipe {
        left: Box<Expr>,
        right: Box<Expr>,
    },

    /// Type cast: expr as type
    Cast {
        expr: Box<Expr>,
        target: Ident,
    },

    /// Try unwrap: expr? — unwrap Ok/Some or early-return Err/None
    TryUnwrap(Box<Expr>),

    /// Grouped expression (parenthesized)
    Grouped(Box<Expr>),

    /// Lazy expression: `lazy expr`
    Lazy(Box<Expr>),
}

#[derive(Debug, Clone)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,        // **
    IntDiv,     // //
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    In,         // `x in list`
    NotIn,      // `x not in list`
    Is,         // `x is Type`
    NullCoalesce, // `x ?? y`
}

#[derive(Debug, Clone)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
}

/// A function/lambda parameter.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: Ident,
    pub type_ann: Option<String>,
    pub default: Option<Expr>,
    pub is_variadic: bool,    // ...args
}

/// A call argument — can be positional or named.
#[derive(Debug, Clone)]
pub struct CallArg {
    pub name: Option<Ident>,  // None for positional
    pub value: Expr,
    pub is_spread: bool,      // ...value
}

/// A match arm.
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    /// Literal value: 1, "hello", true, null
    Literal(Expr),
    /// Variable binding: x
    Ident(Ident),
    /// Wildcard: _
    Wildcard,
    /// Enum/struct destructure: Shape.Circle(r)
    Destructure {
        path: Vec<Ident>,
        fields: Vec<Pattern>,
    },
    /// List pattern: [a, b, ...rest]
    List(Vec<Pattern>),
    /// Rest pattern inside a list: ...tail
    Rest(Ident),
    /// Struct field pattern: Point { x, y } or { x, y }
    StructPat {
        type_name: Option<String>,
        fields: Vec<(String, Option<Pattern>)>,
    },
    /// Type pattern: case (int), case (str), case (float), etc.
    TypePat(String),
    /// Typed binding pattern: case ((n: int)) — binds `name` when the value is
    /// of the given type, otherwise no match.
    TypedBind { name: Ident, type_name: String },
    /// Tuple pattern: (a, b)
    Tuple(Vec<Pattern>),
    /// Or pattern: a | b
    Or(Vec<Pattern>),
    /// Range pattern: 1..10 or 1..=10
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        inclusive: bool,
    },
    /// Ok(pattern)
    Ok(Box<Pattern>),
    /// Err(pattern)
    Err(Box<Pattern>),
    /// Some(pattern)
    Some(Box<Pattern>),
    /// None
    None,
    /// Default/else
    Default,
}

/// A single clause in a comprehension: `for var in iter` or `for (a, b) in iter` with optional `if cond`
#[derive(Debug, Clone)]
pub struct CompClause {
    pub var: Ident,
    pub destructure: Option<Vec<Ident>>,  // for tuple destructuring like (a, b)
    pub iter: Box<Expr>,
    pub cond: Option<Box<Expr>>,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub name: Ident,
    pub type_ann: Option<String>,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: Ident,
    pub fields: Vec<Ident>, // data-carrying variant
}
