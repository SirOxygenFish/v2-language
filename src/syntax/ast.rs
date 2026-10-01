//! The AST (INTERNALS 4.3).

use std::rc::Rc;

use super::span::{FileId, Span, UnitLoc};
use super::token::Delim;
use crate::ffi::types::Sig;

#[derive(Debug, Clone)]
pub struct Name {
    pub text: Rc<str>,
    pub span: Span,
}

#[derive(Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[allow(dead_code)] // the interpreter covers these as roadmap 0.6-0.8 land
#[derive(Debug)]
pub enum StmtKind {
    Let(Name, Option<Expr>),
    Const(Name, Expr),
    Expr(Expr),
    Break,
    Continue,
    Return(Option<Expr>),
    Throw(Expr),
    /// `op: None` is a plain `=`.
    Assign { target: Target, op: Option<BinOp>, value: Expr },
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

#[allow(dead_code)]
#[derive(Debug)]
pub enum Target {
    Name(Name),
    Member(Box<Expr>, Name),
    Index(Box<Expr>, Box<Expr>),
}

#[derive(Debug)]
pub struct Body {
    pub stmts: Vec<Stmt>,
    /// Indices of the `func` statements directly in this body, hoisted on entry (5.6).
    #[allow(dead_code)]
    pub funcs: Vec<usize>,
    #[allow(dead_code)]
    pub span: Span,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct FuncDecl {
    /// `None` for lambdas.
    pub name: Option<Name>,
    pub params: Vec<(Name, Option<Expr>)>,
    pub rest: Option<Name>,
    pub body: FuncBody,
    pub span: Span,
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
    Name(Rc<str>),
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
    Lambda(Rc<FuncDecl>),
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

#[allow(dead_code)] // read by import resolution (roadmap 1.6)
#[derive(Debug)]
pub struct ExportItem {
    pub text: Rc<str>,
    /// In c/cpp/rust blocks a quoted item is a C prototype (15.2); elsewhere a non-V2 name.
    pub quoted: bool,
    pub span: Span,
}

/// `@import { items } from SELECTOR` or `@import PACKAGE as NAME` (4.1, 4.3).
#[allow(dead_code)] // read by import resolution (roadmap 1.6)
#[derive(Debug)]
pub struct EngineImport {
    pub items: ImportItems,
    pub selector: Selector,
    pub span: Span,
}

#[allow(dead_code)] // read by import resolution (roadmap 1.6)
#[derive(Debug)]
pub enum ImportItems {
    All(Span),
    Names(Vec<ImportItem>),
    /// `SELECTOR as name`: the whole module as a handle.
    Module(Name),
}

#[allow(dead_code)] // read by import resolution (roadmap 1.6)
#[derive(Debug)]
pub struct ImportItem {
    pub name: Rc<str>,
    pub quoted: bool,
    pub alias: Option<Name>,
    pub span: Span,
}

/// A selector exactly as written; chapter 13 canonicalizes it (4.3).
#[allow(dead_code)] // read by import resolution (roadmap 1.6)
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
