//! The AST (INTERNALS 4.3).

use std::rc::Rc;

use super::span::{FileId, Span, UnitLoc};
use super::token::Delim;
use crate::ffi::types::Sig;

#[derive(Debug, Clone)]
pub struct Name {
    pub text: Rc<str>,
    pub span: Span,
    /// Where a declaration of this name goes in its frame (`syntax/resolve`, 5.6).
    pub slot: u32,
    /// A method name's last lookup (`interp::method_target`): which methods table, and where in
    /// it; 0 before the first.
    pub method: std::cell::Cell<u32>,
}

impl Name {
    pub fn new(text: Rc<str>, span: Span) -> Name {
        Name { text, span, slot: 0, method: std::cell::Cell::new(0) }
    }
}

/// A scope's names, in the order its frame holds them (5.6).
pub type Layout = Rc<[Rc<str>]>;

/// Where a use of a name looks first (`syntax/resolve`, 5.6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Loc {
    /// Declared nowhere in the file: by name, for its error.
    Unresolved,
    Slot { hops: u32, index: u32 },
    Builtin(u32),
}

#[derive(Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug)]
pub enum StmtKind {
    Let(Name, Option<Expr>),
    Const(Name, Expr),
    Expr(Expr),
    Break,
    Continue,
    Return(Option<Expr>),
    Throw(Expr),
    /// `op: None` is a plain `=`; the span is the operator (the error caret, 2.2).
    Assign { target: Target, op: Option<(BinOp, Span)>, value: Expr },
    If { arms: Vec<(Expr, Body)>, else_: Option<Body> },
    While { cond: Expr, body: Body },
    For { first: Name, second: Option<Name>, iter: ForIter, body: Body },
    Try { body: Body, catch: Option<(Option<Name>, Body)>, finally: Option<Body> },
    Func(Rc<FuncDecl>),
    Import { path: Rc<str>, alias: Name },
    EngineImport(Rc<EngineImport>),
    Extern(Rc<Extern>),
    /// A block in statement position: an action or definition block.
    Block(Rc<EngineBlock>),
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum ForIter {
    Value(Expr),
    Range(Expr, Expr),
}

#[derive(Debug)]
pub enum Target {
    Name(Name, Loc),
    Member(Box<Expr>, Name),
    /// The span is the `[`.
    Index(Box<Expr>, Box<Expr>, Span),
}

#[derive(Debug)]
pub struct Body {
    pub stmts: Vec<Stmt>,
    /// Indices of the `func` statements directly in this body, hoisted on entry (5.6).
    #[allow(dead_code)]
    pub funcs: Vec<usize>,
    /// Its frame's names; `None` if it needs no frame (5.6). A function's body uses `FuncDecl.layout`.
    pub layout: Option<Layout>,
    #[allow(dead_code)]
    pub span: Span,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct FuncDecl {
    /// `None` for anonymous functions.
    pub name: Option<Name>,
    pub params: Vec<(Name, Option<Expr>)>,
    pub rest: Option<Name>,
    pub body: FuncBody,
    /// The call frame's names (5.6).
    pub layout: Layout,
    pub span: Span,
    /// An anonymous function's name, `func@main.v2:12` (5.7), made the first time it is needed.
    pub anon_name: std::cell::OnceCell<Rc<str>>,
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum FuncBody {
    Block(Body),
    Expr(Expr),
}

#[derive(Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum ExprKind {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Bytes(Rc<[u8]>),
    Name(Rc<str>, Loc),
    FStr(Vec<FPart>),
    List(Vec<Expr>),
    Dict(Vec<(DictKey, Expr)>),
    /// The `Span` is the operator token (the error caret, 2.2).
    Unary(UnOp, Span, Box<Expr>),
    Binary(BinOp, Span, Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Coalesce(Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `paren` is the `(`, the caret position when the callee has no name (2.2).
    Call { callee: Box<Expr>, args: Vec<Arg>, paren: Span },
    Index { obj: Box<Expr>, index: Box<Expr>, bracket: Span },
    Slice { obj: Box<Expr>, lo: Option<Box<Expr>>, hi: Option<Box<Expr>>, bracket: Span },
    /// A method call is a `Call` whose callee is a `Member`.
    Member { obj: Box<Expr>, name: Name, optional: bool },
    /// A postfix chain containing `?.`: a `null` at any `?.` makes the whole chain `null` (4.1).
    OptChain(Box<Expr>),
    /// An anonymous function: `func(x) => x * 2` or `func(x) { … }` (4.1).
    Func(Rc<FuncDecl>),
    /// An action block used as a value (4.1).
    Block(Rc<EngineBlock>),
}

#[derive(Debug)]
pub enum FPart {
    Lit(Rc<str>),
    Expr { expr: Expr, fixed: Option<u8> },
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum DictKey {
    Str(Rc<str>, Span),
    Computed(Expr),
}

#[derive(Debug)]
pub enum Arg {
    Pos(Expr),
    Named(Name, Expr),
    Spread(Expr),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    In,
    NotIn,
}

/// Identifies a block for the whole run: file and byte offset of its `@` (6.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct UnitId {
    pub file: FileId,
    pub offset: u32,
}

/// An embedded block (4.3).
#[allow(dead_code)] // most fields are read by engine dispatch (roadmap Phase 1)
#[derive(Debug)]
pub struct EngineBlock {
    pub unit_id: UnitId,
    /// The tag as written (`python`, not canonicalized).
    pub tag: Name,
    pub label: Option<Name>,
    pub args: Vec<(Name, Expr)>,
    /// The processed body and its location (4.4, 2.3).
    pub body: Rc<str>,
    pub loc: UnitLoc,
    pub body_span: Span,
    pub delimiter: Delim,
    pub directives: Directives,
    /// From the `@` to the closing delimiter.
    pub span: Span,
}

/// The `@export`/`@import` directives found in a block body (4.4).
#[derive(Debug, Default)]
pub struct Directives {
    /// `Some` makes the block a definition block.
    pub exports: Option<Exports>,
    pub imports: Vec<EngineImport>,
}

#[derive(Debug, Default)]
pub struct Exports {
    pub items: Vec<ExportItem>,
    pub star: Option<Span>,
    /// The first `@export` directive (where "definition block" notes point).
    pub at: Option<Span>,
}

#[derive(Debug)]
pub struct ExportItem {
    /// Quoted items (a foreign name V2 cannot write bare, 4.4) are names like any other here.
    pub text: Rc<str>,
    pub span: Span,
}

/// `@import { items } from SELECTOR` or `@import PACKAGE as NAME` (4.1, 4.3).
#[derive(Debug)]
pub struct EngineImport {
    pub items: ImportItems,
    pub selector: Selector,
    pub span: Span,
}

#[derive(Debug)]
pub enum ImportItems {
    Names(Vec<ImportItem>),
    /// `SELECTOR as name`: the whole module as a handle.
    Module(Name),
}

#[derive(Debug)]
pub struct ImportItem {
    pub name: Rc<str>,
    pub alias: Option<Name>,
    pub span: Span,
}

/// A selector exactly as written; chapter 13 canonicalizes it (4.3).
#[derive(Debug)]
pub enum Selector {
    /// `@py`
    Union { tag: Name },
    /// `@py.stats`
    Exact { tag: Name, label: Name },
    /// `stats` or `"odd-name"`
    Label { label: Name },
    /// `py.os.path`, `js."@scope/pkg"`
    Module { lang: Name, path: Vec<Name> },
    /// `v2`
    V2 { span: Span },
}

/// `extern "lib" [as name] { C declarations }` (4.3, 16.1).
#[allow(dead_code)] // read by the FFI layer (roadmap 2.2)
#[derive(Debug)]
pub struct Extern {
    pub lib: Name,
    pub alias: Option<Name>,
    pub decls: Vec<Sig>,
    /// Span of each declaration, parallel to `decls` (for redeclaration checks).
    pub decl_spans: Vec<Span>,
    pub body_span: Span,
}
