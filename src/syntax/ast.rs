//! The AST (INTERNALS 4.3). Grows with the parser: roadmap 0.2 covers expression statements,
//! literals, names, f-strings and calls; 0.5 adds the rest of 4.3.

use std::rc::Rc;

use super::span::{FileId, Span, UnitLoc};
use super::token::Delim;

#[derive(Debug, Clone)]
pub struct Name {
    pub text: Rc<str>,
    #[allow(dead_code)] // read by static checks and messages from roadmap 0.5
    pub span: Span,
}

#[derive(Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    #[allow(dead_code)] // read by static checks from roadmap 0.5
    pub span: Span,
}

#[derive(Debug)]
pub enum StmtKind {
    Expr(Expr),
    /// A block in statement position: an action or definition block.
    Block(Rc<EngineBlock>),
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

#[derive(Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug)]
pub enum ExprKind {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Bytes(Rc<[u8]>),
    FStr(Vec<FPart>),
    Name(Rc<str>),
    /// `paren` is the `(`, the caret position when the callee has no name (2.2).
    Call { callee: Box<Expr>, args: Vec<Arg>, paren: Span },
    /// An action block used as a value (4.1).
    Block(Rc<EngineBlock>),
}

#[derive(Debug)]
pub enum FPart {
    Lit(Rc<str>),
    Expr { expr: Expr, fixed: Option<u8> },
}

#[derive(Debug)]
pub enum Arg {
    Pos(Expr),
    Named(Name, Expr),
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

#[allow(dead_code)] // read by the parser and import resolution (roadmap 0.5, 1.6)
#[derive(Debug)]
pub struct ExportItem {
    pub text: Rc<str>,
    /// In c/cpp/rust blocks a quoted item is a C prototype (15.2); elsewhere a non-V2 name.
    pub quoted: bool,
    pub span: Span,
}

/// `@import { items } from SELECTOR` or `@import PACKAGE as NAME` (4.1, 4.3).
#[allow(dead_code)] // read by the parser and import resolution (roadmap 0.5, 1.6)
#[derive(Debug)]
pub struct EngineImport {
    pub items: ImportItems,
    pub selector: Selector,
    pub span: Span,
}

#[allow(dead_code)] // read by the parser and import resolution (roadmap 0.5, 1.6)
#[derive(Debug)]
pub enum ImportItems {
    All(Span),
    Names(Vec<ImportItem>),
    /// `SELECTOR as name`: the whole module as a handle.
    Module(Name),
}

#[allow(dead_code)] // read by the parser and import resolution (roadmap 0.5, 1.6)
#[derive(Debug)]
pub struct ImportItem {
    pub name: Rc<str>,
    pub quoted: bool,
    pub alias: Option<Name>,
    pub span: Span,
}

/// A selector exactly as written; chapter 13 canonicalizes it (4.3).
#[allow(dead_code)] // read by the parser and import resolution (roadmap 0.5, 1.6)
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
