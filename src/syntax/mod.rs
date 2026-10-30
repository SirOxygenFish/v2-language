//! Source files, lexer, block scanning, directives, parser and AST (INTERNALS ch. 2-4).

pub mod ast;
pub mod check;
pub mod directives;
pub mod lexer;
pub mod parser;
pub mod resolve;
pub mod scan;
pub mod span;
pub mod token;

use span::Span;

/// A static diagnostic: kind `SyntaxError` (INTERNALS 2.4, 4.5).
#[derive(Debug, Clone)]
pub struct Diag {
    pub message: String,
    pub span: Span,
    pub notes: Vec<(String, Span)>,
    pub hint: Option<String>,
}

impl Diag {
    pub fn new(message: impl Into<String>, span: Span) -> Diag {
        Diag { message: message.into(), span, notes: Vec::new(), hint: None }
    }

    pub fn note(mut self, text: impl Into<String>, span: Span) -> Diag {
        self.notes.push((text.into(), span));
        self
    }

    pub fn hint(mut self, text: impl Into<String>) -> Diag {
        self.hint = Some(text.into());
        self
    }
}
