//! The AST (INTERNALS 4.3). Grows with the parser: roadmap 0.2 covers expression statements,
//! literals, names, f-strings and calls; 0.5 adds the rest of 4.3.

use std::rc::Rc;

use super::span::Span;

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
