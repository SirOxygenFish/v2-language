//! Tokens (INTERNALS 3.2).

use std::fmt;
use std::rc::Rc;

use super::span::Span;

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Tok {
    /// Magnitude of an integer literal; at most 2^63 (only valid under unary minus, 3.4).
    Int(u64),
    Float(f64),
    Str(Rc<str>),
    Bytes(Rc<[u8]>),
    FStr(Vec<FPartTok>),
    Ident(Rc<str>),
    Kw(Kw),
    Op(Op),
    LBrace(#[allow(dead_code)] BraceKind), // read by the parser from roadmap 0.5
    RBrace,
    /// `@import` / `@export` (3.6).
    DirImport,
    DirExport,
    /// `@py` in a selector, after `from` or `@import` (3.6).
    AtTag(Rc<str>),
    /// `@tag [label]`, followed by arg tokens (if any) and a `BlockBody` (3.7).
    BlockHead { tag: Rc<str>, label: Option<(Rc<str>, Span)> },
    BlockBody(Box<RawBody>),
    /// The `{ … }` of `extern "lib" [as name]` (3.9).
    ExternBody(#[allow(dead_code)] Box<RawBody>), // parsed by `extern` support (roadmap 0.5)
    Newline,
    Semi,
    Eof,
}

/// A raw block body exactly as written between its delimiters (3.7).
#[derive(Clone, Debug)]
pub struct RawBody {
    pub text: Rc<str>,
    pub text_span: Span,
    /// The `{` or the opening fence.
    pub open: Span,
    /// The `}` or the closing fence line.
    #[allow(dead_code)] // read by static checks (roadmap 0.5)
    pub close: Span,
    pub delim: Delim,
    /// For each line of `text`: does it start in code (3.8)?
    pub starts_in_code: Vec<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Delim {
    Brace,
    /// A fence of this many backticks.
    Fence(u8),
}

#[derive(Clone, Debug)]
pub enum FPartTok {
    Lit(Rc<str>),
    Expr { toks: Vec<Token>, fixed: Option<u8>, span: Span },
}

/// A `{` opens a statement body or a dict literal (3.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BraceKind {
    Body,
    Dict,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kw {
    Let,
    Const,
    Func,
    Return,
    If,
    Elif,
    Else,
    While,
    For,
    In,
    Not,
    Break,
    Continue,
    Try,
    Catch,
    Finally,
    Throw,
    Import,
    As,
    From,
    Extern,
    True,
    False,
    Null,
}

impl Kw {
    pub fn from_word(w: &str) -> Option<Kw> {
        Some(match w {
            "let" => Kw::Let,
            "const" => Kw::Const,
            "func" => Kw::Func,
            "return" => Kw::Return,
            "if" => Kw::If,
            "elif" => Kw::Elif,
            "else" => Kw::Else,
            "while" => Kw::While,
            "for" => Kw::For,
            "in" => Kw::In,
            "not" => Kw::Not,
            "break" => Kw::Break,
            "continue" => Kw::Continue,
            "try" => Kw::Try,
            "catch" => Kw::Catch,
            "finally" => Kw::Finally,
            "throw" => Kw::Throw,
            "import" => Kw::Import,
            "as" => Kw::As,
            "from" => Kw::From,
            "extern" => Kw::Extern,
            "true" => Kw::True,
            "false" => Kw::False,
            "null" => Kw::Null,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kw::Let => "let",
            Kw::Const => "const",
            Kw::Func => "func",
            Kw::Return => "return",
            Kw::If => "if",
            Kw::Elif => "elif",
            Kw::Else => "else",
            Kw::While => "while",
            Kw::For => "for",
            Kw::In => "in",
            Kw::Not => "not",
            Kw::Break => "break",
            Kw::Continue => "continue",
            Kw::Try => "try",
            Kw::Catch => "catch",
            Kw::Finally => "finally",
            Kw::Throw => "throw",
            Kw::Import => "import",
            Kw::As => "as",
            Kw::From => "from",
            Kw::Extern => "extern",
            Kw::True => "true",
            Kw::False => "false",
            Kw::Null => "null",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    StarStar,
    Ellipsis,
    DotDot,
    QDot,
    QQ,
    FatArrow,
    EqEq,
    Ne,
    Le,
    Ge,
    AndAnd,
    OrOr,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    Lt,
    Gt,
    Bang,
    Question,
    Colon,
    Dot,
    Comma,
    LParen,
    RParen,
    LBracket,
    RBracket,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::StarStar => "**",
            Op::Ellipsis => "...",
            Op::DotDot => "..",
            Op::QDot => "?.",
            Op::QQ => "??",
            Op::FatArrow => "=>",
            Op::EqEq => "==",
            Op::Ne => "!=",
            Op::Le => "<=",
            Op::Ge => ">=",
            Op::AndAnd => "&&",
            Op::OrOr => "||",
            Op::PlusEq => "+=",
            Op::MinusEq => "-=",
            Op::StarEq => "*=",
            Op::SlashEq => "/=",
            Op::PercentEq => "%=",
            Op::Plus => "+",
            Op::Minus => "-",
            Op::Star => "*",
            Op::Slash => "/",
            Op::Percent => "%",
            Op::Eq => "=",
            Op::Lt => "<",
            Op::Gt => ">",
            Op::Bang => "!",
            Op::Question => "?",
            Op::Colon => ":",
            Op::Dot => ".",
            Op::Comma => ",",
            Op::LParen => "(",
            Op::RParen => ")",
            Op::LBracket => "[",
            Op::RBracket => "]",
        }
    }
}

/// Human-readable token names for "expected …, found …" messages (INTERNALS 2.5).
impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Int(n) => write!(f, "number `{n}`"),
            Tok::Float(x) => write!(f, "number `{x:?}`"),
            Tok::Str(_) => write!(f, "string"),
            Tok::Bytes(_) => write!(f, "bytes literal"),
            Tok::FStr(_) => write!(f, "f-string"),
            Tok::Ident(n) => write!(f, "name `{n}`"),
            Tok::Kw(k) => write!(f, "keyword `{}`", k.as_str()),
            Tok::Op(o) => write!(f, "`{}`", o.as_str()),
            Tok::LBrace(_) => write!(f, "`{{`"),
            Tok::RBrace => write!(f, "`}}`"),
            Tok::DirImport => write!(f, "`@import`"),
            Tok::DirExport => write!(f, "`@export`"),
            Tok::AtTag(t) => write!(f, "a selector tag `@{t}`"),
            Tok::BlockHead { tag, label: Some((l, _)) } => write!(f, "block `@{tag} {l}`"),
            Tok::BlockHead { tag, label: None } => write!(f, "block `@{tag}`"),
            Tok::BlockBody(_) => write!(f, "block body"),
            Tok::ExternBody(_) => write!(f, "extern body"),
            Tok::Newline => write!(f, "end of line"),
            Tok::Semi => write!(f, "`;`"),
            Tok::Eof => write!(f, "end of file"),
        }
    }
}
