//! The lexer (INTERNALS ch. 3): normalized text in, one `Vec<Token>` out.
//!
//! Covers 3.1-3.9: tokens, keywords, operators, comments, where statements end, numbers, every
//! string form, the `@` sigil, embedded block heads and raw bodies, and `extern` bodies.

use std::rc::Rc;

use super::scan::{self, Dialect, Profile};
use super::span::{FileId, SourceMap, Span};
use super::token::{BraceKind, Delim, FPartTok, Kw, Op, RawBody, Tok, Token};
use super::Diag;

/// What a `:` in an interpolation can start (3.5).
pub const ONLY_FIXED: &str = "the only format in ${…} is :.Nf, with N up to 99";
/// What to write for `${x:>5}` and other formats V2 doesn't have.
pub const PAD_HINT: &str = "pad with \" \" * n, as in let s = str(x), then \" \" * (5 - len(s)) + s for 5 wide";

/// Past this many nested `${…}`, lexing stops with `nested too deeply` (4.1).
const MAX_INTERP_DEPTH: u32 = 256;

/// What an open bracket on the stack is (3.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Open {
    Paren,
    Bracket,
    Dict,
    Body,
    Interp,
    /// The `( … )` args of a block head; closing it opens the block body (3.7).
    BlockArgs,
}

impl Open {
    fn opener(self) -> &'static str {
        match self {
            Open::Paren | Open::BlockArgs => "(",
            Open::Bracket => "[",
            Open::Dict | Open::Body => "{",
            Open::Interp => "${",
        }
    }
}

pub fn lex(sm: &SourceMap, file: FileId) -> Result<Vec<Token>, Diag> {
    let text = sm.file(file).text.clone();
    let mut lx = Lexer {
        file,
        src: &text,
        pos: 0,
        line: 1,
        col: 1,
        toks: Vec::new(),
        stack: Vec::new(),
        interp_newlines: true,
        interp_depth: 0,
        heads: Vec::new(),
    };
    lx.run()?;
    Ok(lx.toks)
}

struct Lexer<'a> {
    file: FileId,
    src: &'a str,
    pos: usize,
    line: u32,
    col: u32,
    toks: Vec<Token>,
    stack: Vec<(Open, Span)>,
    /// May a line break appear inside the current `${…}`? Only in triple-quoted f-strings.
    interp_newlines: bool,
    /// Nesting of `${…}`, limited like the parser's depth (4.1) to protect the host stack.
    interp_depth: u32,
    /// Block heads whose `( … )` args are being lexed.
    heads: Vec<Head>,
}

/// A block head waiting for its body (3.7).
#[derive(Clone)]
struct Head {
    tag: Rc<str>,
    label: Option<Rc<str>>,
    /// The `@`.
    at: Mark,
}

impl Head {
    /// `@py block 'stats'` / `@py block`, as messages name it.
    fn name(&self) -> String {
        match &self.label {
            Some(l) => format!("@{} block '{l}'", self.tag()),
            None => format!("@{} block", self.tag()),
        }
    }

    /// The tag in canonical form, as messages print it (2.5).
    fn tag(&self) -> &str {
        scan::canonical_tag(&self.tag)
    }
}

#[derive(Clone, Copy)]
struct Mark {
    pos: usize,
    line: u32,
    col: u32,
}

/// A piece of a string literal before escapes are decoded (3.5).
enum RawPart {
    Lit(String),
    Interp { toks: Vec<Token>, fixed: Option<u8>, span: Span },
}

#[derive(Clone, Copy, PartialEq)]
enum StrKind {
    Plain,
    Raw,
    Format,
    Bytes,
}

/// A name V2 can write bare: an identifier that is not a keyword (13.4).
pub fn is_plain_name(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(is_ident_start) && cs.all(is_ident_continue) && crate::syntax::token::Kw::from_word(s).is_none()
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphabetic() || c.is_ascii_digit()
}

/// Tokens after which a line break never ends the statement (3.3 rule 2).
fn continues_line(t: &Tok) -> bool {
    match t {
        Tok::Op(op) => !matches!(op, Op::RParen | Op::RBracket | Op::LParen | Op::LBracket),
        Tok::Kw(k) => matches!(k, Kw::In | Kw::Not | Kw::As | Kw::From),
        _ => false,
    }
}

impl<'a> Lexer<'a> {
    // -- position helpers ---------------------------------------------------------------------

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn bump_n(&mut self, n: usize) {
        for _ in 0..n {
            self.bump();
        }
    }

    fn mark(&self) -> Mark {
        Mark { pos: self.pos, line: self.line, col: self.col }
    }

    fn span_from(&self, m: Mark) -> Span {
        Span { file: self.file, lo: m.pos as u32, hi: self.pos as u32, line: m.line, col: m.col }
    }

    /// A span covering the next `n` chars from the current position.
    fn span_here(&self, n: usize) -> Span {
        let len: usize = self.rest().chars().take(n).map(char::len_utf8).sum();
        Span { file: self.file, lo: self.pos as u32, hi: (self.pos + len) as u32, line: self.line, col: self.col }
    }

    fn push(&mut self, tok: Tok, m: Mark) {
        let span = self.span_from(m);
        self.toks.push(Token { tok, span });
    }

    // -- main loop ----------------------------------------------------------------------------

    /// Lex until end of file, or (inside an interpolation) until the `}` that closes it.
    /// Returns the `:.Nf` format spec that ended an interpolation, if any.
    fn run(&mut self) -> Result<Option<u8>, Diag> {
        loop {
            while matches!(self.peek(), Some(' ' | '\t')) {
                self.bump();
            }
            if self.stack.len() == 1 && self.stack[0].0 == Open::Interp {
                if let Some(n) = self.format_spec() {
                    return Ok(Some(n));
                }
            }
            let Some(c) = self.peek() else {
                return self.end_of_file();
            };
            let m = self.mark();
            match c {
                '\n' => {
                    self.bump();
                    self.line_break(m)?;
                }
                '/' if self.peek_at(1) == Some('/') => {
                    if let Some(d) = self.floor_division(m) {
                        return Err(d);
                    }
                    while !matches!(self.peek(), None | Some('\n')) {
                        self.bump();
                    }
                }
                '/' if self.peek_at(1) == Some('*') => {
                    self.bump_n(2);
                    let mut saw_newline = false;
                    loop {
                        if self.rest().starts_with("*/") {
                            self.bump_n(2);
                            break;
                        }
                        match self.bump() {
                            Some('\n') => saw_newline = true,
                            Some(_) => {}
                            None => {
                                return Err(Diag::new("unterminated /* comment", Span { hi: m.pos as u32 + 2, ..self.span_from(m) }))
                            }
                        }
                    }
                    if saw_newline {
                        self.line_break(m)?;
                    }
                }
                '"' | '\'' => self.string(StrKind::Plain, m)?,
                c if is_ident_start(c) => self.word(m)?,
                '0'..='9' => self.number(m)?,
                '{' => self.open_brace(m),
                '}' => {
                    if let Some(fixed) = self.close(m, '}')? {
                        return Ok(fixed);
                    }
                }
                '(' | '[' => {
                    self.bump();
                    let (open, op) = if c == '(' { (Open::Paren, Op::LParen) } else { (Open::Bracket, Op::LBracket) };
                    self.push(Tok::Op(op), m);
                    let span = self.span_from(m);
                    self.stack.push((open, span));
                }
                ')' | ']' => {
                    self.close(m, c)?;
                }
                ';' => {
                    self.bump();
                    self.push(Tok::Semi, m);
                }
                // A `#!` first line, as Unix scripts start, isn't read either (3.2).
                '#' if self.pos == 0 && self.src[self.pos..].starts_with("#!") => {
                    return Err(Diag::new("unexpected character '#'", self.span_here(1)).hint("V2 doesn't read #! lines: delete it, and run the script with v2 FILE"))
                }
                '#' => {
                    return Err(Diag::new("unexpected character '#'", self.span_here(1)).hint("comments start with //"))
                }
                '@' => self.at_sign(m)?,
                _ => self.operator(m)?,
            }
        }
    }

    fn end_of_file(&mut self) -> Result<Option<u8>, Diag> {
        if let Some(&(open, span)) = self.stack.last() {
            let line = &self.src[span.lo as usize..];
            let line = &line[..line.find('\n').unwrap_or(line.len())];
            let d = Diag::new(format!("`{}` is never closed", open.opener()), span);
            // A `//` outside a string: an even number of quotes before it.
            let comment = line.find("//").is_some_and(|p| line[..p].matches('"').count().is_multiple_of(2) && line[..p].matches('\'').count().is_multiple_of(2));
            return Err(if comment {
                d.hint("`//` starts a comment; integer division is div(a, b)")
            } else {
                d
            });
        }
        // Right after the last code, not on the empty line past it: `let total = 1 +` shows its
        // line with the caret where more was expected (2.4).
        let span = match self.toks.iter().rev().find(|t| !matches!(t.tok, Tok::Newline)) {
            Some(last) => {
                let s = last.span;
                let text = &self.src[s.lo as usize..s.hi as usize];
                let (line, col) = match text.rfind('\n') {
                    Some(nl) => (s.line + text.matches('\n').count() as u32, text[nl + 1..].chars().count() as u32 + 1),
                    None => (s.line, s.col + text.chars().count() as u32),
                };
                Span { file: self.file, lo: s.hi, hi: s.hi, line, col }
            }
            None => self.span_from(self.mark()),
        };
        self.toks.push(Token { tok: Tok::Eof, span });
        Ok(None)
    }

    /// Right after a `:` at the top level of an interpolation, with only what a format holds
    /// between them.
    fn in_format(&self, pos: usize) -> bool {
        let Some(open) = self.src[..pos].rfind("${") else { return false };
        let text = &self.src[open..pos];
        let top = matches!(self.stack.last(), Some((Open::Interp, _)));
        top && !text.contains('?') && text.rfind(':').is_some_and(|c| text[c + 1..].bytes().all(|b| b.is_ascii_digit() || b",.+-_<>^=#".contains(&b)))
    }

    /// `:.Nf}` right at the top level of an interpolation (3.5).
    fn format_spec(&mut self) -> Option<u8> {
        let r = self.rest().as_bytes();
        if r.len() < 4 || r[0] != b':' || r[1] != b'.' {
            return None;
        }
        let digits = r[2..].iter().take_while(|b| b.is_ascii_digit()).count();
        if !(1..=2).contains(&digits) || r.get(2 + digits) != Some(&b'f') || r.get(3 + digits) != Some(&b'}') {
            return None;
        }
        let n: u8 = std::str::from_utf8(&r[2..2 + digits]).ok()?.parse().ok()?;
        self.bump_n(4 + digits);
        self.stack.pop();
        Some(n)
    }

    // -- line breaks (3.3) --------------------------------------------------------------------

    fn line_break(&mut self, m: Mark) -> Result<(), Diag> {
        let in_interp = self.stack.iter().any(|(o, _)| *o == Open::Interp);
        if in_interp && !self.interp_newlines {
            return Err(Diag::new(
                "a line break inside ${…} is only allowed in a triple-quoted f-string",
                Span { hi: m.pos as u32 + 1, ..self.span_from(m) },
            ));
        }
        let Some(last) = self.toks.last() else { return Ok(()) };
        if matches!(last.tok, Tok::Newline) {
            return Ok(());
        }
        if matches!(self.stack.last(), Some((Open::Paren | Open::Bracket | Open::Dict | Open::Interp | Open::BlockArgs, _))) {
            return Ok(());
        }
        if continues_line(&last.tok) {
            return Ok(());
        }
        if self.next_line_starts_with_dot() {
            return Ok(());
        }
        let span = Span { file: self.file, lo: m.pos as u32, hi: m.pos as u32 + 1, line: m.line, col: m.col };
        self.toks.push(Token { tok: Tok::Newline, span });
        Ok(())
    }

    /// Rule 3: the next line that isn't blank or comment-only starts with `.` (not `..`) or `?.`.
    fn next_line_starts_with_dot(&self) -> bool {
        let mut s = self.rest();
        loop {
            s = s.trim_start_matches([' ', '\t', '\n']);
            if let Some(after) = s.strip_prefix("//") {
                s = after.find('\n').map(|i| &after[i..]).unwrap_or("");
                continue;
            }
            if let Some(after) = s.strip_prefix("/*") {
                match after.find("*/") {
                    Some(i) => {
                        s = &after[i + 2..];
                        continue;
                    }
                    None => return false,
                }
            }
            break;
        }
        s.starts_with("?.") || (s.starts_with('.') && !s.starts_with(".."))
    }

    // -- brackets -----------------------------------------------------------------------------

    fn open_brace(&mut self, m: Mark) {
        let body = matches!(
            self.toks.last().map(|t| &t.tok),
            Some(
                Tok::Op(Op::RParen | Op::RBracket)
                    | Tok::RBrace
                    | Tok::Ident(_)
                    | Tok::Int(_)
                    | Tok::Float(_)
                    | Tok::Str(_)
                    | Tok::FStr(_)
                    | Tok::Bytes(_)
                    | Tok::Kw(Kw::True | Kw::False | Kw::Null | Kw::Else | Kw::Try | Kw::Catch | Kw::Finally)
            )
        );
        self.bump();
        let (kind, open) = if body { (BraceKind::Body, Open::Body) } else { (BraceKind::Dict, Open::Dict) };
        self.push(Tok::LBrace(kind), m);
        let span = self.span_from(m);
        self.stack.push((open, span));
    }

    /// Close `)`, `]` or `}`. Returns `Some(fixed)` when the `}` ends the current interpolation.
    fn close(&mut self, m: Mark, c: char) -> Result<Option<Option<u8>>, Diag> {
        let here = self.span_here(1);
        let Some(&(open, open_span)) = self.stack.last() else {
            return Err(Diag::new(format!("unmatched `{c}`"), here));
        };
        let matches = match c {
            ')' => matches!(open, Open::Paren | Open::BlockArgs),
            ']' => open == Open::Bracket,
            _ => matches!(open, Open::Body | Open::Dict | Open::Interp),
        };
        if !matches {
            return Err(Diag::new(
                format!(
                    "found `{c}` but `{}` opened at {}:{} is still open",
                    open.opener(),
                    open_span.line,
                    open_span.col
                ),
                here,
            )
            .note(format!("`{}` opened here", open.opener()), open_span));
        }
        self.bump();
        self.stack.pop();
        match (c, open) {
            ('}', Open::Interp) => return Ok(Some(None)),
            ('}', _) => self.push(Tok::RBrace, m),
            (')', Open::BlockArgs) => {
                self.push(Tok::Op(Op::RParen), m);
                let head = self.heads.pop().expect("a block head for these args");
                self.block_open(&head)?;
            }
            (')', _) => self.push(Tok::Op(Op::RParen), m),
            _ => self.push(Tok::Op(Op::RBracket), m),
        }
        Ok(None)
    }

    // -- operators ----------------------------------------------------------------------------

    fn operator(&mut self, m: Mark) -> Result<(), Diag> {
        const OPS: &[(&str, Op)] = &[
            ("...", Op::Ellipsis),
            ("**", Op::StarStar),
            ("..", Op::DotDot),
            ("?.", Op::QDot),
            ("??", Op::QQ),
            ("=>", Op::FatArrow),
            ("==", Op::EqEq),
            ("!=", Op::Ne),
            ("<=", Op::Le),
            (">=", Op::Ge),
            ("&&", Op::AndAnd),
            ("||", Op::OrOr),
            ("+=", Op::PlusEq),
            ("-=", Op::MinusEq),
            ("*=", Op::StarEq),
            ("/=", Op::SlashEq),
            ("%=", Op::PercentEq),
            ("+", Op::Plus),
            ("-", Op::Minus),
            ("*", Op::Star),
            ("/", Op::Slash),
            ("%", Op::Percent),
            ("=", Op::Eq),
            ("<", Op::Lt),
            (">", Op::Gt),
            ("!", Op::Bang),
            ("?", Op::Question),
            (":", Op::Colon),
            (".", Op::Dot),
            (",", Op::Comma),
        ];
        let r = self.rest();
        for bad in ["<<", ">>", "&", "|", "^", "~"] {
            if r.starts_with(bad) && !r.starts_with("&&") && !r.starts_with("||") {
                let d = Diag::new("V2 has no bitwise operators", self.span_here(bad.chars().count()));
                return Err(if bad == "^" { d.hint("V2 writes a power as 2 ** 3") } else { d });
            }
        }
        for &(text, op) in OPS {
            if r.starts_with(text) {
                self.bump_n(text.len());
                self.push(Tok::Op(op), m);
                return Ok(());
            }
        }
        let c = self.peek().unwrap_or(' ');
        let (shown, mut hint) = shown_char(c);
        // A Markdown example copied with its fence lines (```v2 and ```).
        let start = self.src[..self.pos].rfind('\n').map_or(0, |i| i + 1);
        let line = self.src[start..].lines().next().unwrap_or_default().trim();
        if c == '`' && line.starts_with("```") && line[3..].chars().all(|c| c.is_ascii_alphanumeric()) {
            hint = Some("```v2 and ``` mark an example in a Markdown file: copy only the lines between them");
        }
        let d = Diag::new(format!("unexpected character {shown}"), self.span_here(1));
        Err(match hint {
            Some(h) => d.hint(h),
            None => d,
        })
    }

    // -- words: identifiers, keywords, string prefixes ----------------------------------------

    fn word(&mut self, m: Mark) -> Result<(), Diag> {
        let start = self.pos;
        while self.peek().is_some_and(is_ident_continue) {
            self.bump();
        }
        let w = &self.src[start..self.pos];
        if matches!(self.peek(), Some('"' | '\'')) {
            let kind = match w {
                "r" => Some(StrKind::Raw),
                "f" => Some(StrKind::Format),
                "b" => Some(StrKind::Bytes),
                _ if w.len() <= 3 && w.chars().all(|c| "rRfFbBuU".contains(c)) => {
                    return Err(Diag::new(
                        format!("unknown string prefix `{w}`"),
                        self.span_from(m),
                    )
                    .hint("a string takes at most one prefix: r, f or b (lower-case)"));
                }
                _ => None,
            };
            if let Some(kind) = kind {
                return self.string(kind, m);
            }
        }
        let after_dot = matches!(self.toks.last().map(|t| &t.tok), Some(Tok::Op(Op::Dot | Op::QDot)));
        let tok = match Kw::from_word(w) {
            Some(k) if !after_dot => Tok::Kw(k),
            _ => Tok::Ident(w.into()),
        };
        let is_extern = matches!(tok, Tok::Kw(Kw::Extern));
        self.push(tok, m);
        if is_extern {
            self.extern_head(m)?;
        }
        Ok(())
    }

    // -- numbers (3.4) ------------------------------------------------------------------------

    fn number(&mut self, m: Mark) -> Result<(), Diag> {
        let start = self.pos;
        if self.rest().starts_with("0x") {
            self.bump_n(2);
            let digits_start = self.pos;
            while self.peek().is_some_and(|c| c.is_ascii_hexdigit() || c == '_') {
                self.bump();
            }
            let digits = &self.src[digits_start..self.pos];
            if digits.is_empty() || self.peek().is_some_and(is_ident_continue) {
                self.eat_word();
                return Err(Diag::new("`0x` must be followed by hex digits", self.span_from(m)));
            }
            check_underscores(digits).map_err(|msg| Diag::new(msg, self.span_from(m)))?;
            let value = u64::from_str_radix(&digits.replace('_', ""), 16);
            return self.push_int(value.ok(), m);
        }
        while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '_') {
            self.bump();
        }
        let int_part = self.src[start..self.pos].to_string();
        let mut is_float = false;
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            is_float = true;
            self.bump();
            while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '_') {
                self.bump();
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let sign = usize::from(matches!(self.peek_at(1), Some('+' | '-')));
            if self.peek_at(1 + sign).is_some_and(|c| c.is_ascii_digit()) {
                is_float = true;
                self.bump_n(1 + sign);
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '_') {
                    self.bump();
                }
            }
        }
        if self.peek().is_some_and(is_ident_continue) {
            let c = self.peek().unwrap_or(' ');
            self.eat_word();
            let span = self.span_from(m);
            if int_part == "0" && matches!(c, 'b' | 'o' | 'B' | 'O' | 'X') {
                return Err(Diag::new("V2 has only decimal and 0x hex integer literals", span));
            }
            if self.in_format(m.pos) {
                return Err(Diag::new(ONLY_FIXED, span).hint(PAD_HINT));
            }
            return Err(Diag::new("a number can't be followed directly by a letter", span));
        }
        let text = &self.src[start..self.pos];
        let span = self.span_from(m);
        for group in text.split(['.', 'e', 'E', '+', '-']) {
            if !group.is_empty() {
                check_underscores(group).map_err(|msg| Diag::new(msg, span))?;
            }
        }
        if int_part.len() > 1 && int_part.starts_with('0') {
            return Err(Diag::new("a number can't start with 0 (leading zeros)", span));
        }
        let clean = text.replace('_', "");
        if is_float {
            match clean.parse::<f64>() {
                Ok(x) if x.is_finite() => {
                    self.push(Tok::Float(x), m);
                    Ok(())
                }
                _ => Err(Diag::new("float literal is out of range", span)),
            }
        } else {
            self.push_int(clean.parse::<u64>().ok(), m)
        }
    }

    fn push_int(&mut self, value: Option<u64>, m: Mark) -> Result<(), Diag> {
        match value {
            Some(v) if v <= 1u64 << 63 => {
                self.push(Tok::Int(v), m);
                Ok(())
            }
            _ => Err(Diag::new("integer literal does not fit in 64 bits", self.span_from(m))),
        }
    }

    fn eat_word(&mut self) {
        while self.peek().is_some_and(is_ident_continue) {
            self.bump();
        }
    }

    // -- strings (3.5) ------------------------------------------------------------------------

    fn string(&mut self, kind: StrKind, m: Mark) -> Result<(), Diag> {
        let quote = self.peek().unwrap_or('"');
        let triple = quote == '"' && self.rest().starts_with("\"\"\"");
        let parts = if triple { self.triple_body(kind, m)? } else { self.one_line_body(kind, quote)? };
        let tok = match kind {
            StrKind::Bytes => {
                let mut out = Vec::new();
                for p in &parts {
                    if let RawPart::Lit(raw) = p {
                        decode_into_bytes(raw, &mut out);
                    }
                }
                Tok::Bytes(out.into())
            }
            StrKind::Format => {
                let mut fparts = Vec::new();
                for p in parts {
                    match p {
                        RawPart::Lit(raw) => {
                            let s = decode(&raw);
                            if !s.is_empty() {
                                fparts.push(FPartTok::Lit(s.into()));
                            }
                        }
                        RawPart::Interp { toks, fixed, span } => fparts.push(FPartTok::Expr { toks, fixed, span }),
                    }
                }
                Tok::FStr(fparts)
            }
            StrKind::Plain | StrKind::Raw => {
                let mut s = String::new();
                for p in &parts {
                    if let RawPart::Lit(raw) = p {
                        if kind == StrKind::Raw {
                            s.push_str(raw);
                        } else {
                            s.push_str(&decode(raw));
                        }
                    }
                }
                Tok::Str(s.into())
            }
        };
        self.push(tok, m);
        Ok(())
    }

    /// Why a one-line string that opens at `open` reached the end of its line (3.2): a curly
    /// quote where it should end, an apostrophe that ended a '…' string early, a path ending in
    /// `\` before its quote, or text that goes on over lines.
    fn unterminated_hint(&self, open: usize, quote: char) -> String {
        let start = self.src[..open].rfind('\n').map_or(0, |i| i + 1);
        let line = &self.src[start..];
        let line = &line[..line.find('\n').unwrap_or(line.len())];
        let rest = &self.src[open + quote.len_utf8()..start + line.len()];
        if let Some(c) = rest.chars().find(|c| matches!(c, '\u{201C}' | '\u{201D}' | '\u{2018}' | '\u{2019}')) {
            return format!("the line has a curly quote {c} where the string should end: V2's strings end with a straight {quote} (editors and chat apps curl them)");
        }
        if quote == '\'' && line.matches('\'').count() >= 3 {
            return "an apostrophe inside '…' ends the string: put the text in \"…\", as in \"it's me\"".into();
        }
        if rest.trim_end().ends_with(&format!("\\{quote}")) {
            return format!("\\{quote} is a quote inside the string, so it goes on: end a Windows path with / or \\\\, as in \"C:/temp/\"");
        }
        format!("end the string with {quote} on this line; for text over several lines, write \\n for a line break, or use a \"\"\" string: \"\"\" at the end of the line, the text on the lines after it, and \"\"\" alone on the last line")
    }

    /// `"…"` / `'…'`: no line breaks inside.
    fn one_line_body(&mut self, kind: StrKind, quote: char) -> Result<Vec<RawPart>, Diag> {
        let q = self.mark(); // the opening quote: where "unterminated string" points
        self.bump();
        let mut parts = Vec::new();
        let mut lit = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => {
                    let d = Diag::new("unterminated string", Span { hi: q.pos as u32 + 1, ..self.span_from(q) });
                    return Err(d.hint(self.unterminated_hint(q.pos, quote)));
                }
                Some(c) if c == quote => {
                    self.bump();
                    break;
                }
                Some('\\') if kind != StrKind::Raw => self.escape(kind, &mut lit)?,
                Some('$') if kind == StrKind::Format && self.peek_at(1) == Some('{') => {
                    parts.push(RawPart::Lit(std::mem::take(&mut lit)));
                    parts.push(self.interpolation(false)?);
                }
                Some(c) => {
                    if kind == StrKind::Bytes && !c.is_ascii() {
                        return Err(Diag::new("bytes literals must be ASCII (write \\xNN for other bytes)", self.span_here(1)));
                    }
                    lit.push(c);
                    self.bump();
                }
            }
        }
        parts.push(RawPart::Lit(lit));
        Ok(parts)
    }

    /// `"""…"""` with the closing-line dedent rule (3.5).
    fn triple_body(&mut self, kind: StrKind, m: Mark) -> Result<Vec<RawPart>, Diag> {
        self.bump_n(3);
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.bump();
        }
        if self.peek() != Some('\n') {
            return Err(Diag::new(
                "the text of a \"\"\" string starts on the line after the opening \"\"\"",
                Span { hi: m.pos as u32 + 3, ..self.span_from(m) },
            ));
        }
        self.bump();
        // Raw content up to the closing quotes, with interpolations already lexed.
        let mut parts: Vec<RawPart> = Vec::new();
        let mut lit = String::new();
        // Each content line's start (index into its Lit part and the line's mark), for dedent.
        let mut line_ws_only = true;
        loop {
            if self.rest().starts_with("\"\"\"") {
                if !line_ws_only {
                    return Err(Diag::new("the closing \"\"\" must be on its own line", self.span_here(3)));
                }
                self.bump_n(3);
                break;
            }
            match self.peek() {
                None => {
                    return Err(Diag::new("unterminated \"\"\" string", Span { hi: m.pos as u32 + 3, ..self.span_from(m) }))
                }
                Some('\n') => {
                    lit.push('\n');
                    self.bump();
                    line_ws_only = true;
                }
                Some('\\') if kind != StrKind::Raw => {
                    line_ws_only = false;
                    self.escape(kind, &mut lit)?;
                }
                Some('$') if kind == StrKind::Format && self.peek_at(1) == Some('{') => {
                    line_ws_only = false;
                    parts.push(RawPart::Lit(std::mem::take(&mut lit)));
                    parts.push(self.interpolation(true)?);
                }
                Some(c) => {
                    if kind == StrKind::Bytes && !c.is_ascii() {
                        return Err(Diag::new("bytes literals must be ASCII (write \\xNN for other bytes)", self.span_here(1)));
                    }
                    if c != ' ' && c != '\t' {
                        line_ws_only = false;
                    }
                    lit.push(c);
                    self.bump();
                }
            }
        }
        parts.push(RawPart::Lit(lit));
        self.dedent_triple(parts, m)
    }

    /// Apply 3.5 rules 2-4 to the raw parts of a triple-quoted string.
    fn dedent_triple(&self, mut parts: Vec<RawPart>, m: Mark) -> Result<Vec<RawPart>, Diag> {
        // W: the whitespace before the closing quotes, at the end of the last part.
        let Some(RawPart::Lit(last)) = parts.last_mut() else { unreachable!("last part is a literal") };
        let w_start = last.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let w = last[w_start..].to_string();
        // Drop W and the line break before the closing line (rule 4).
        last.truncate(w_start);
        if last.ends_with('\n') {
            last.pop();
        }
        // Rule 3: every content line starts with W or is whitespace-only.
        let mut at_line_start = true;
        let mut line_no = m.line + 1;
        let n = parts.len();
        for (pi, part) in parts.iter_mut().enumerate() {
            let text = match part {
                RawPart::Lit(text) => text,
                RawPart::Interp { span, .. } => {
                    // Line breaks inside `${…}` still advance the line count.
                    line_no += self.src[span.lo as usize..span.hi as usize].matches('\n').count() as u32;
                    at_line_start = false;
                    continue;
                }
            };
            let followed_by_interp = pi + 1 < n;
            let mut out = String::with_capacity(text.len());
            let mut rest: &str = text;
            loop {
                if at_line_start {
                    let line_end = rest.find('\n');
                    let line = &rest[..line_end.unwrap_or(rest.len())];
                    let whole_line = line_end.is_some() || !followed_by_interp;
                    if whole_line && line.chars().all(|c| c == ' ' || c == '\t') {
                        rest = &rest[line.len()..];
                    } else if let Some(stripped) = rest.strip_prefix(w.as_str()) {
                        rest = stripped;
                    } else {
                        let lo = self.line_offset(m, line_no) as u32;
                        let span = Span { file: self.file, lo, hi: lo + 1, line: line_no, col: 1 };
                        let start = Span { file: self.file, lo: m.pos as u32, hi: m.pos as u32 + 3, line: m.line, col: m.col };
                        return Err(Diag::new("indented less than the closing \"\"\", or with different tabs/spaces", span)
                            .note("the string starts here", start));
                    }
                }
                match rest.find('\n') {
                    Some(i) => {
                        out.push_str(&rest[..=i]);
                        rest = &rest[i + 1..];
                        at_line_start = true;
                        line_no += 1;
                    }
                    None => {
                        out.push_str(rest);
                        at_line_start = false;
                        break;
                    }
                }
                // A line that begins right before a `${` still gets its indentation checked.
                if rest.is_empty() && !(at_line_start && followed_by_interp) {
                    break;
                }
            }
            *text = out;
        }
        Ok(parts)
    }

    /// Byte offset where `line` starts, scanning forward from mark `m` (on an earlier line).
    fn line_offset(&self, m: Mark, line: u32) -> usize {
        let mut cur = m.line;
        let mut pos = m.pos;
        for (i, b) in self.src.as_bytes()[m.pos..].iter().enumerate() {
            if cur == line {
                break;
            }
            if *b == b'\n' {
                cur += 1;
                pos = m.pos + i + 1;
            }
        }
        pos
    }

    /// Validate one escape at a backslash and copy it raw into `lit` (decoded later, after
    /// dedent). Errors point at the backslash.
    fn escape(&mut self, kind: StrKind, lit: &mut String) -> Result<(), Diag> {
        let at = self.span_here(2);
        self.bump(); // backslash
        const BACKSLASH: &str = "write \\\\ for a backslash, or use a raw string r\"…\"";
        let bad = |what: &str| Diag::new(format!("unknown escape {what}"), at).hint(BACKSLASH);
        match self.peek() {
            Some(c @ ('n' | 't' | 'r' | '\\' | '"' | '\'' | '0' | '$')) => {
                self.bump();
                lit.push('\\');
                lit.push(c);
            }
            Some('x') => {
                self.bump();
                let h: String = self.rest().chars().take(2).collect();
                if h.len() != 2 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                    // `"C:\xampp\htdocs"`: a Windows path, more likely than an escape.
                    return Err(Diag::new("\\x takes exactly 2 hex digits", at).hint(BACKSLASH));
                }
                self.bump_n(2);
                lit.push_str("\\x");
                lit.push_str(&h);
            }
            Some('u') => {
                if kind == StrKind::Bytes {
                    return Err(Diag::new("\\u{…} is not allowed in bytes literals (use \\xNN)", at));
                }
                self.bump();
                if self.peek() != Some('{') {
                    // `"data\updates.csv"`: a Windows path, more likely than an escape.
                    return Err(Diag::new("\\u must be followed by {hex digits}, as in \\u{1F600}", at).hint(BACKSLASH));
                }
                self.bump();
                let h: String = self.rest().chars().take_while(|c| *c != '}').take(7).collect();
                let ok = (1..=6).contains(&h.len())
                    && h.chars().all(|c| c.is_ascii_hexdigit())
                    && u32::from_str_radix(&h, 16).ok().and_then(char::from_u32).is_some()
                    && self.rest()[h.len()..].starts_with('}');
                if !ok {
                    return Err(Diag::new("\\u{…} must name a Unicode scalar value in 1 to 6 hex digits", at));
                }
                self.bump_n(h.len() + 1);
                lit.push_str("\\u{");
                lit.push_str(&h);
                lit.push('}');
            }
            Some('\n') | None => return Err(bad("\\ before a line break")),
            Some(c) => return Err(bad(&format!("\\{c}"))),
        }
        Ok(())
    }

    /// Lex `${ … }` on a fresh stack whose bottom is `Interp` (3.5).
    fn interpolation(&mut self, allow_newlines: bool) -> Result<RawPart, Diag> {
        let m = self.mark();
        self.bump_n(2); // ${
        let open = self.span_from(m);
        if self.interp_depth >= MAX_INTERP_DEPTH {
            return Err(Diag::new("nested too deeply", open));
        }
        self.interp_depth += 1;
        let saved_toks = std::mem::take(&mut self.toks);
        let saved_stack = std::mem::take(&mut self.stack);
        let saved_nl = self.interp_newlines;
        self.interp_newlines = allow_newlines;
        self.stack.push((Open::Interp, open));
        let result = self.run();
        self.interp_depth -= 1;
        self.interp_newlines = saved_nl;
        self.stack = saved_stack;
        let toks = std::mem::replace(&mut self.toks, saved_toks);
        let fixed = result?;
        let span = self.span_from(m);
        if toks.is_empty() {
            return Err(Diag::new("empty ${} in an f-string", span));
        }
        Ok(RawPart::Interp { toks, fixed, span })
    }
}

const DECORATOR_HINT: &str = "V2 has no decorators: @tag starts an embedded block of another language";

/// Where a string stands for a block label (3.7, 4.1).
pub(crate) const LABEL_HINT: &str = "a label is a name, like stats or calc_2";

const FENCE_HINT: &str = "if the block's code has braces V2 can't match, use the fence form: put ``` after the header and close it with ``` on its own line";

/// Embedded blocks, selectors, directives and `extern` bodies (3.6, 3.7, 3.9).
impl<'a> Lexer<'a> {
    /// `@import`, `@export`, a selector tag, or a block head (3.6).
    fn at_sign(&mut self, m: Mark) -> Result<(), Diag> {
        for (word, tok) in [("@import", Tok::DirImport), ("@export", Tok::DirExport)] {
            let r = self.rest();
            if r.starts_with(word) && !r[word.len()..].chars().next().is_some_and(is_ident_continue) {
                self.bump_n(word.len());
                self.push(tok, m);
                return Ok(());
            }
        }
        let after = &self.rest()[1..];
        let tag_len = after
            .char_indices()
            .take_while(|&(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
            .count();
        if tag_len == 0 {
            return Err(Diag::new("expected an engine tag after @", self.span_here(1)));
        }
        let tag: Rc<str> = after[..tag_len].into();
        if matches!(self.toks.last().map(|t| &t.tok), Some(Tok::Kw(Kw::From) | Tok::DirImport)) {
            self.bump_n(1 + tag_len);
            self.push(Tok::AtTag(tag), m);
            return Ok(());
        }
        self.bump_n(1 + tag_len);
        self.block_head(m, tag)
    }

    /// `@` TAG [LABEL] [`(` args `)`] then `{` or a fence (3.7).
    fn block_head(&mut self, at: Mark, tag: Rc<str>) -> Result<(), Diag> {
        self.skip_blanks();
        let mut label = None;
        let lm = self.mark();
        match self.peek() {
            Some(c) if is_ident_start(c) => {
                while self.peek().is_some_and(is_ident_continue) {
                    self.bump();
                }
                let w = &self.src[lm.pos..self.pos];
                let shown_tag = scan::canonical_tag(&tag);
                if w == "func" {
                    return Err(Diag::new(format!("expected {{ or a ``` fence after @{shown_tag}"), self.span_of_mark(at, 1 + tag.len()))
                        .hint(DECORATOR_HINT));
                }
                if Kw::from_word(w).is_some() {
                    return Err(Diag::new(format!("a block label can't be the keyword `{w}`"), self.span_from(lm)));
                }
                label = Some((Rc::<str>::from(w), self.span_from(lm)));
            }
            // Labels are names (3.7); the string is read only to point at all of it.
            Some(q @ ('"' | '\'')) => {
                self.one_line_body(StrKind::Plain, q)?;
                return Err(Diag::new("a block label can't be a string", self.span_from(lm)).hint(LABEL_HINT));
            }
            _ => {}
        }
        let head = Head { tag: tag.clone(), label: label.as_ref().map(|(l, _)| l.clone()), at };
        self.push(Tok::BlockHead { tag, label }, at);
        self.skip_blanks();
        if self.peek() == Some('(') {
            let m = self.mark();
            self.bump();
            self.push(Tok::Op(Op::LParen), m);
            let span = self.span_from(m);
            self.stack.push((Open::BlockArgs, span));
            self.heads.push(head);
            return Ok(()); // the main loop lexes the args; their `)` calls block_open
        }
        self.block_open(&head)
    }

    /// After the head (and args): a `{` or a fence on the same line, then the raw body.
    fn block_open(&mut self, head: &Head) -> Result<(), Diag> {
        self.skip_blanks();
        let profile = scan::profile_for(&head.tag);
        if self.rest().starts_with("```") {
            let body = self.capture_fence(head, profile)?;
            let span = body.text_span;
            self.toks.push(Token { tok: Tok::BlockBody(Box::new(body)), span });
            return Ok(());
        }
        if self.peek() == Some('{') {
            let body = self.capture_brace(profile, &head.name(), head.at, false)?;
            let span = body.text_span;
            self.toks.push(Token { tok: Tok::BlockBody(Box::new(body)), span });
            return Ok(());
        }
        let shown = match &head.label {
            Some(l) => format!("@{} {l}", head.tag()),
            None => format!("@{}", head.tag()),
        };
        // Brace on the next line?
        if matches!(self.peek(), None | Some('\n')) || self.rest().starts_with("//") || self.rest().starts_with("/*") {
            let header_line = self.line;
            if let Some(target) = self.next_code_line_start() {
                let line = &self.src[target..];
                let head_span = self.span_of_mark(head.at, 1 + head.tag.len());
                if line.starts_with('{') {
                    return Err(Diag::new(
                        format!("the {{ that opens a block must be on the same line as @{}", head.tag()),
                        self.span_at(target, 1),
                    )
                    .note("the block starts here", head_span)
                    .hint(format!("move {{ to the end of line {header_line}")));
                }
                let starts_func = line.strip_prefix("func").is_some_and(|r| !r.chars().next().is_some_and(is_ident_continue));
                if starts_func {
                    return Err(Diag::new(format!("expected {{ or a ``` fence after {shown}"), head_span).hint(DECORATOR_HINT));
                }
            }
        }
        Err(Diag::new(format!("expected {{ or a ``` fence after {shown}"), self.span_here(1)))
    }

    /// Byte offset of the first non-blank char on the next line that isn't blank or a comment.
    fn next_code_line_start(&self) -> Option<usize> {
        let mut i = self.pos + self.rest().find('\n')? + 1;
        loop {
            let line_end = self.src[i..].find('\n').map(|k| i + k).unwrap_or(self.src.len());
            let line = &self.src[i..line_end];
            let trimmed = line.trim_start_matches([' ', '\t']);
            if !trimmed.is_empty() && !trimmed.starts_with("//") {
                return Some(i + (line.len() - trimmed.len()));
            }
            if line_end >= self.src.len() {
                return None;
            }
            i = line_end + 1;
        }
    }

    /// A brace-form body, `{` included; checks guards C1 and C2 (3.7). `is_extern`: an `extern`
    /// body, which has no fence form and is never a value (3.9).
    fn capture_brace(&mut self, profile: Profile, what: &str, at: Mark, is_extern: bool) -> Result<RawBody, Diag> {
        let open = self.span_here(1);
        self.bump();
        let body_start = self.pos;
        let body_mark = self.mark();
        let scanned = match scan::scan_braces(self.src, body_start, profile) {
            Ok(s) => s,
            Err(un) => {
                let mut d = Diag::new(format!("{what} is never closed"), self.span_of_mark(at, 1));
                if !is_extern {
                    d = d.hint(FENCE_HINT);
                }
                if let Some(off) = un.open_brace {
                    d = d.note("this { inside the block is still open", self.span_at(off, 1));
                }
                if let Some((off, name)) = un.open_construct {
                    d = d.note(format!("this {name} never closes"), self.span_at(off, 1));
                }
                return Err(d);
            }
        };
        let close = scanned.close;
        let text = &self.src[body_start..close];
        // C1: a multi-line body closes at a `}` that starts its line. Reported at the opening
        // line (DOCS 4 Errors), with a note at the `}`.
        let what_short = what.split(" '").next().unwrap_or(what);
        if text.contains('\n') {
            // A multi-line block's code starts on the line after its `{` (3.7), so its first line
            // dedents like the others. Checked first: it is the first mistake on the way down.
            let first = text.split('\n').next().unwrap_or("");
            let lead = first.len() - first.trim_start_matches([' ', '\t']).len();
            if !is_extern && !first.trim_matches([' ', '\t', '\r']).is_empty() {
                return Err(Diag::new("a multi-line block starts on the line after its {", self.span_at(body_start + lead, 1))
                    .hint("move this code to the next line, or write the whole block on one line"));
            }
            let line_start = self.src[..close].rfind('\n').map(|i| i + 1).unwrap_or(0);
            if !self.src[line_start..close].trim_matches([' ', '\t']).is_empty() {
                let d = Diag::new(format!("{what_short} closes at a }} that does not start its line"), self.span_of_mark(at, 1))
                    .note("the } that closes it", self.span_at(close, 1));
                // `    print(x)}`: the block's last line, with its } at the end.
                let ends_line = self.src[close + 1..].split('\n').next().is_some_and(|rest| rest.trim().is_empty());
                return Err(if is_extern {
                    d.hint("put the } that closes an extern block on its own line")
                } else if ends_line {
                    d.hint("put the } that ends the block on a line of its own")
                } else {
                    d.hint(FENCE_HINT)
                });
            }
        }
        self.advance_to(close);
        let text_span = self.span_from(body_mark);
        let close_span = self.span_here(1);
        self.bump();
        // C2: only blanks, a comment, or `) ] } , ;` may follow on the closing line.
        let line_rest = self.rest().split('\n').next().unwrap_or("");
        let after = line_rest.trim_start_matches([' ', '\t']);
        let ok = after.is_empty()
            || after.starts_with("//")
            || after.starts_with("/*")
            || after.starts_with([')', ']', '}', ',', ';']);
        if !ok {
            let d = Diag::new(format!("unexpected text after the }} that closes the {what_short}"), close_span);
            return Err(if is_extern {
                d.hint("an extern block is a statement: end the line after its }")
            } else {
                d.hint(format!(
                    "{FENCE_HINT}; to use a block value in an expression, put it in parentheses: ({} {{ 1 }}) + 2",
                    what_short.trim_end_matches(" block")
                ))
            });
        }
        Ok(RawBody {
            text: text.into(),
            text_span,
            open,
            close: close_span,
            delim: Delim::Brace,
            starts_in_code: scanned.starts_in_code,
        })
    }

    /// A fenced body: N >= 3 backticks, then every line up to a line holding exactly N backticks.
    fn capture_fence(&mut self, head: &Head, profile: Profile) -> Result<RawBody, Diag> {
        let n = self.rest().chars().take_while(|c| *c == '`').count();
        let open = self.span_here(n);
        self.bump_n(n);
        let line_rest = self.rest().split('\n').next().unwrap_or("");
        if !line_rest.trim_matches([' ', '\t']).is_empty() {
            return Err(Diag::new("nothing may follow the opening ``` fence on its line", self.span_here(1))
                .hint("the block's tag already names the language"));
        }
        self.advance_to(self.pos + line_rest.len());
        if self.peek().is_none() {
            return Err(Diag::new(format!("{} is never closed", head.name()), self.span_of_mark(head.at, 1)));
        }
        self.bump(); // the line break after the fence
        let body_mark = self.mark();
        let fence = "`".repeat(n);
        let mut line_start = self.pos;
        loop {
            if line_start >= self.src.len() {
                return Err(Diag::new(format!("{} is never closed", head.name()), self.span_of_mark(head.at, 1))
                    .note("the fence opens here", open));
            }
            let line_end = self.src[line_start..].find('\n').map(|i| line_start + i).unwrap_or(self.src.len());
            if self.src[line_start..line_end].trim_matches([' ', '\t']) == fence {
                let text = &self.src[body_mark.pos..line_start];
                self.advance_to(line_start);
                let text_span = Span { hi: line_start as u32, ..self.span_from(body_mark) };
                let close_mark = self.mark();
                self.advance_to(line_end);
                let close = self.span_from(close_mark);
                return Ok(RawBody {
                    text: text.into(),
                    text_span,
                    open,
                    close,
                    delim: Delim::Fence(n.min(255) as u8),
                    starts_in_code: scan::line_states(text, profile),
                });
            }
            line_start = line_end + 1;
        }
    }

    /// After `extern`: a library string, optional `as NAME`, and `{` on the same line (3.9).
    fn extern_head(&mut self, kw: Mark) -> Result<(), Diag> {
        self.skip_blanks();
        let m = self.mark();
        let quote_next = matches!(self.peek_at(1), Some('"' | '\''));
        match self.peek() {
            Some('"' | '\'') => self.string(StrKind::Plain, m)?,
            // A raw string suits Windows paths: extern r"C:\libs\x.dll" (16.2).
            Some('r') if quote_next => {
                self.bump();
                self.string(StrKind::Raw, m)?
            }
            Some('f' | 'b') if quote_next => {
                return Err(Diag::new("the library name must be a plain \"…\" or raw r\"…\" string", self.span_here(1)))
            }
            _ => return Err(Diag::new("expected a library name in quotes after extern, as in extern \"m\" { … }", self.span_here(1))),
        }
        self.skip_blanks();
        if self.rest().starts_with("as") && !self.rest()[2..].chars().next().is_some_and(is_ident_continue) {
            let am = self.mark();
            self.bump_n(2);
            self.push(Tok::Kw(Kw::As), am);
            self.skip_blanks();
            let nm = self.mark();
            if !self.peek().is_some_and(is_ident_start) {
                return Err(Diag::new("expected a name after `as`", self.span_here(1)));
            }
            while self.peek().is_some_and(is_ident_continue) {
                self.bump();
            }
            let name = &self.src[nm.pos..self.pos];
            if Kw::from_word(name).is_some() {
                return Err(Diag::new(format!("expected a name after `as`, found keyword `{name}`"), self.span_from(nm)));
            }
            self.push(Tok::Ident(name.into()), nm);
            self.skip_blanks();
        }
        if self.peek() != Some('{') {
            return Err(Diag::new("expected { on the same line as extern", self.span_here(1)));
        }
        let body = self.capture_brace(Profile::CLike(Dialect::C), "extern block", kw, true)?;
        let span = body.text_span;
        self.toks.push(Token { tok: Tok::ExternBody(Box::new(body)), span });
        Ok(())
    }

    /// Python's floor division, `n // 2`: in V2 the rest of the line is a comment, so it would
    /// silently mean `n` (3.2). A comment that starts with a number and holds no words (`2`,
    /// `2)`, `2, 3)`, `2 - 1`), right after an operand (a name, a number, a call, an index, a
    /// member, parentheses) with at most one space on each side, is that habit; a result written
    /// after code (`print(x)    // 3`) or a remark (`// 3 retries`) is not.
    fn floor_division(&self, m: Mark) -> Option<Diag> {
        let prev = self.toks.last().filter(|t| t.span.line == m.line)?;
        let start = self.operand_start()?;
        // An operand that starts its statement (`print(x) // 2`) has its value unused: the `//`
        // after it is a remark, not a division (in Python too it would compute nothing).
        if start == 0 || matches!(self.toks[start - 1].tok, Tok::Newline | Tok::Semi | Tok::LBrace(_) | Tok::RBrace) {
            return None;
        }
        let gap = &self.src[prev.span.hi as usize..m.pos];
        let after = self.rest()[2..].split('\n').next().unwrap_or("");
        let text = after.strip_prefix(' ').unwrap_or(after).trim_end();
        let number_len = text.find(|c: char| !(c.is_ascii_digit() || c == '_' || c == '.')).unwrap_or(text.len());
        let number = &text[..number_len];
        let code_after = text[number_len..].chars().all(|c| c.is_ascii_digit() || " _.)],;+-*".contains(c));
        if !matches!(gap, "" | " ") || !number.starts_with(|c: char| c.is_ascii_digit()) || !code_after {
            return None;
        }
        let left = &self.src[self.toks[start].span.lo as usize..prev.span.hi as usize];
        Some(
            Diag::new(format!("`// {number}` starts a comment in V2, so this line doesn't divide"), self.span_of_mark(m, 2))
                .hint(format!("V2 divides rounding down with div(a, b), as in div({left}, {number})")),
        )
    }

    /// The first token of the operand that ends with the last token: a name or a number, with
    /// the calls, indexes and members chained to it (`len(xs)`, `xs[0]`, `d.n`), or a
    /// parenthesized expression; `None` when the last token ends no operand.
    fn operand_start(&self) -> Option<usize> {
        let toks = &self.toks;
        let mut i = toks.len().checked_sub(1)?;
        loop {
            match toks[i].tok {
                Tok::Op(Op::RParen | Op::RBracket) => {
                    let mut depth = 0i32;
                    loop {
                        match toks[i].tok {
                            Tok::Op(Op::RParen | Op::RBracket) => depth += 1,
                            Tok::Op(Op::LParen | Op::LBracket) => depth -= 1,
                            _ => {}
                        }
                        if depth == 0 {
                            break;
                        }
                        i = i.checked_sub(1)?;
                    }
                    // `len(xs)`, `xs[0]`, `f()(x)`: what is called or indexed is part of it.
                    if i > 0 && matches!(toks[i - 1].tok, Tok::Ident(_) | Tok::Op(Op::RParen | Op::RBracket)) {
                        i -= 1;
                        continue;
                    }
                }
                Tok::Ident(_) => {
                    if i >= 2 && matches!(toks[i - 1].tok, Tok::Op(Op::Dot | Op::QDot)) {
                        i -= 2;
                        continue;
                    }
                }
                Tok::Int(_) | Tok::Float(_) => {}
                _ => return None,
            }
            return Some(i);
        }
    }

    fn skip_blanks(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.bump();
        }
    }

    /// Move forward to byte offset `target`, keeping line and column right.
    fn advance_to(&mut self, target: usize) {
        while self.pos < target && self.bump().is_some() {}
    }

    /// A span of `len` chars at byte offset `offset` (at or after the current position, or
    /// anywhere: line and column are computed from the start of the text).
    fn span_at(&self, offset: usize, len: usize) -> Span {
        let before = &self.src[..offset];
        let line = before.matches('\n').count() as u32 + 1;
        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let col = self.src[line_start..offset].chars().count() as u32 + 1;
        let hi: usize = offset + self.src[offset..].chars().take(len).map(char::len_utf8).sum::<usize>();
        Span { file: self.file, lo: offset as u32, hi: hi as u32, line, col }
    }

    fn span_of_mark(&self, m: Mark, len: usize) -> Span {
        let hi: usize = m.pos + self.src[m.pos..].chars().take(len).map(char::len_utf8).sum::<usize>();
        Span { file: self.file, lo: m.pos as u32, hi: hi as u32, line: m.line, col: m.col }
    }
}

/// `_` only between two digits (3.4).
/// A character as `unexpected character …` names it (3.2), with a hint for the ones that come
/// from copying code out of a web page, a chat or a word processor: invisible ones by code point,
/// look-alikes of a quote, a minus or a space with what to type instead.
pub fn shown_char(c: char) -> (String, Option<&'static str>) {
    let code = format!("U+{:04X}", c as u32);
    match c {
        '\u{00A0}' | '\u{2007}' | '\u{202F}' => (format!("{code} (a no-break space)"), Some("type a plain space instead")),
        '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' => (format!("{code} (an invisible zero-width character)"), Some("delete it")),
        '\u{0000}' => (format!("{code} (a NUL byte)"), Some("is this a text file? V2 scripts are plain UTF-8 text")),
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => {
            (format!("'{c}' (a curly quote)"), Some("use a straight quote: \" or '"))
        }
        '\u{2010}'..='\u{2015}' | '\u{2212}' => (format!("'{c}' (a dash)"), Some("use a plain minus sign: -")),
        '`' => ("'`'".to_string(), Some("V2 writes a template string as f\"text ${x}\"")),
        // `copy con file` and old editors end a file with Ctrl+Z.
        '\u{001A}' => (format!("{code} (Ctrl+Z, an old end-of-file mark)"), Some("delete it: V2 files end where their text ends")),
        c if c.is_control() || matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}') => (code, None),
        c => (format!("'{c}'"), None),
    }
}

fn check_underscores(digits: &str) -> Result<(), &'static str> {
    if digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return Err("`_` in a number must sit between two digits");
    }
    Ok(())
}

/// Decode validated escapes in a raw string part.
fn decode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('x') => {
                let h: String = chars.by_ref().take(2).collect();
                out.push(char::from(u8::from_str_radix(&h, 16).unwrap_or(0)));
            }
            Some('u') => {
                let h: String = chars.by_ref().skip(1).take_while(|c| *c != '}').collect();
                out.push(u32::from_str_radix(&h, 16).ok().and_then(char::from_u32).unwrap_or('\u{fffd}'));
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn decode_into_bytes(raw: &str, out: &mut Vec<u8>) {
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c as u8);
            continue;
        }
        match chars.next() {
            Some('n') => out.push(b'\n'),
            Some('t') => out.push(b'\t'),
            Some('r') => out.push(b'\r'),
            Some('0') => out.push(0),
            Some('x') => {
                let h: String = chars.by_ref().take(2).collect();
                out.push(u8::from_str_radix(&h, 16).unwrap_or(0));
            }
            Some(other) => out.push(other as u8),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn lex_str(src: &str) -> Result<Vec<Tok>, String> {
        let mut sm = SourceMap::new();
        let id = sm.add(PathBuf::from("t.v2"), "t.v2".into(), src);
        lex(&sm, id).map(|ts| ts.into_iter().map(|t| t.tok).collect()).map_err(|d| d.message)
    }

    fn kinds(src: &str) -> String {
        let toks = lex_str(src).unwrap();
        toks.iter()
            .map(|t| match t {
                Tok::Int(n) => format!("{n}"),
                Tok::Float(x) => format!("{x:?}"),
                Tok::Str(s) => format!("{s:?}"),
                Tok::Bytes(b) => format!("b{b:?}"),
                Tok::FStr(_) => "F".into(),
                Tok::Ident(n) => n.to_string(),
                Tok::Kw(k) => k.as_str().to_uppercase(),
                Tok::Op(o) => o.as_str().into(),
                Tok::LBrace(BraceKind::Body) => "{B".into(),
                Tok::LBrace(BraceKind::Dict) => "{D".into(),
                Tok::RBrace => "}".into(),
                Tok::DirImport => "@import".into(),
                Tok::DirExport => "@export".into(),
                Tok::AtTag(t) => format!("@{t}"),
                Tok::BlockHead { tag, label: Some((l, _)) } => format!("HEAD({tag},{l})"),
                Tok::BlockHead { tag, label: None } => format!("HEAD({tag})"),
                Tok::BlockBody(b) => format!("BODY({:?})", &*b.text),
                Tok::ExternBody(b) => format!("EXTERN({:?})", &*b.text),
                Tok::Newline => "NL".into(),
                Tok::Semi => ";".into(),
                Tok::Eof => "EOF".into(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn err(src: &str) -> String {
        lex_str(src).unwrap_err()
    }

    /// Message, line and column of a lexer error.
    fn err_at(src: &str) -> (String, u32, u32) {
        let mut sm = SourceMap::new();
        let id = sm.add(PathBuf::from("t.v2"), "t.v2".into(), src);
        let d = lex(&sm, id).unwrap_err();
        let (line, col) = sm.line_col(id, d.span.lo);
        assert_eq!((line, col), (d.span.line, d.span.col), "span lo must match its line/col");
        (d.message, d.span.line, d.span.col)
    }

    #[test]
    fn review_regressions() {
        let dedent = "indented less than the closing \"\"\", or with different tabs/spaces".to_string();
        // Line breaks inside a multi-line ${…} count toward the reported line.
        assert_eq!(err_at("print(f\"\"\"\n  ${1 +\n  2}\n  ok\n bad\n  \"\"\")"), (dedent.clone(), 5, 1));
        // A line that starts with ${ at column 1 is still checked against the closing indentation.
        assert_eq!(err_at("print(f\"\"\"\n  a\n${1}\n  \"\"\")"), (dedent.clone(), 3, 1));
        assert_eq!(err_at("print(f\"\"\"\n  ${1}\n${2}\n  \"\"\")"), (dedent, 3, 1));
        // ...and passes when the closing quotes aren't indented.
        assert!(lex_str("print(f\"\"\"\n${1}\n${2}\n\"\"\")").is_ok());
        // "unterminated string" points at the quote, not the prefix.
        assert_eq!(err_at("print(f\"abc\n"), ("unterminated string".into(), 1, 8));
        assert_eq!(err_at("print(r'abc"), ("unterminated string".into(), 1, 8));
        // Deeply nested f-strings stop with an error instead of overflowing the stack. V2 lexes
        // on its 256 MiB interpreter thread; test threads are small, so give this one room too.
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                let deep = format!("print({}1{})", "f\"${".repeat(300), "}\"".repeat(300));
                assert_eq!(err(&deep), "nested too deeply");
                let ok = format!("print({}1{})", "f\"${".repeat(200), "}\"".repeat(200));
                assert!(lex_str(&ok).is_ok());
            })
            .unwrap()
            .join()
            .unwrap();
    }

    fn string_value(src: &str) -> String {
        match &lex_str(src).unwrap()[0] {
            Tok::Str(s) => s.to_string(),
            other => panic!("not a string: {other:?}"),
        }
    }

    #[test]
    fn statements_and_newlines() {
        assert_eq!(kinds("let a = 1\nlet b = 2; let c = 3"), "LET a = 1 NL LET b = 2 ; LET c = 3 EOF");
        // A line ending in an operator continues; open brackets continue.
        assert_eq!(kinds("let t = a +\n  b"), "LET t = a + b EOF");
        assert_eq!(kinds("f(\n1,\n2\n)"), "f ( 1 , 2 ) EOF");
        // A next line starting with `.` or `?.` continues; `..` doesn't.
        assert_eq!(kinds("xs\n  .len()"), "xs . len ( ) EOF");
        assert_eq!(kinds("xs\n // c\n ?.a"), "xs ?. a EOF");
        assert_eq!(kinds("a\n..b"), "a NL .. b EOF");
        // Runs of line breaks collapse; none before the first token.
        assert_eq!(kinds("\n\na\n\n\nb\n"), "a NL b NL EOF");
    }

    /// The DOCS 3.1 example lexes into exactly its six statements.
    #[test]
    fn docs_3_1_example() {
        let src = r#"// line comment
/* block comment */

let a = 1
let b = 2; let c = 3        // `;` is optional; use it to put statements on one line

let total = a +
            b +             // a line ending in an operator continues
            c
let names = [
    "ada",                  // open brackets continue across lines
    "grace",
]
let n = names
    .filter(func(s) => s.starts_with("a"))   // a line starting with `.` continues
    .len()
"#;
        let toks = lex_str(src).unwrap();
        let separators = toks.iter().filter(|t| matches!(t, Tok::Newline | Tok::Semi)).count();
        let lets = toks.iter().filter(|t| matches!(t, Tok::Kw(Kw::Let))).count();
        assert_eq!(lets, 6);
        // One separator after each of the six statements (the last line ends with a newline).
        assert_eq!(separators, 6);
        assert_eq!(
            kinds("let total = a +\n            b +\n            c"),
            "LET total = a + b + c EOF"
        );
    }

    #[test]
    fn block_heads_and_bodies() {
        assert_eq!(kinds("@py { x }"), r#"HEAD(py) BODY(" x ") EOF"#);
        assert_eq!(kinds("@py stats {\n  x\n}\nprint(1)"), r#"HEAD(py,stats) BODY("\n  x\n") NL print ( 1 ) EOF"#);
        assert_eq!(
            kinds("let v = @py(nums, k: 3) { sum(nums) }"),
            r#"LET v = HEAD(py) ( nums , k : 3 ) BODY(" sum(nums) ") EOF"#
        );
        // Args may span lines; the `{` goes after their `)`.
        assert_eq!(kinds("@py(\n  a,\n  b\n) { x }"), r#"HEAD(py) ( a , b ) BODY(" x ") EOF"#);
        assert_eq!(kinds("@py stats{ x }"), r#"HEAD(py,stats) BODY(" x ") EOF"#);
        // Fence form: nothing inside is scanned for its end.
        assert_eq!(kinds("@py ```\nx = '{'\n```\nprint(1)"), r#"HEAD(py) BODY("x = '{'\n") NL print ( 1 ) EOF"#);
        // Block values inside calls end at `,` and `)` (C2 allows them).
        assert_eq!(kinds("f(@py { 1 }, 2)"), r#"f ( HEAD(py) BODY(" 1 ") , 2 ) EOF"#);
        // Selectors and directives.
        assert_eq!(kinds("@import { a } from @py.stats"), "@import {D a } FROM @py . stats EOF");
        assert_eq!(kinds("@import py.numpy as np"), "@import py . numpy AS np EOF");
        // `@exporter` is not the `@export` directive: it is a block head (with no body here).
        assert_eq!(err("@exporter"), "expected { or a ``` fence after @exporter");
    }

    #[test]
    fn extern_bodies() {
        assert_eq!(
            kinds("extern \"m\" as m { double cos(double x); }"),
            r#"EXTERN "m" AS m EXTERN(" double cos(double x); ") EOF"#
        );
        assert_eq!(kinds("extern 'c' {\n  int puts(const char* s); // }\n}"), "EXTERN \"c\" EXTERN(\"\\n  int puts(const char* s); // }\\n\") EOF");
        assert_eq!(err("extern \"m\"\n{ }"), "expected { on the same line as extern");
        assert_eq!(err("extern m {}"), "expected a library name in quotes after extern, as in extern \"m\" { … }");
        assert_eq!(err("extern \"m\" {\n int f(); }"), "extern block closes at a } that does not start its line");
        assert_eq!(kinds("h.extern"), "h . extern EOF");
    }

    #[test]
    fn block_errors() {
        assert_eq!(err("@py\n{ x }"), "the { that opens a block must be on the same line as @py");
        assert_eq!(err("@py stats"), "expected { or a ``` fence after @py stats");
        assert_eq!(err("@memo\nfunc f() {}"), "expected { or a ``` fence after @memo");
        assert_eq!(err("@ {"), "expected an engine tag after @");
        assert_eq!(err("@py for { }"), "a block label can't be the keyword `for`");
        // Labels are names, never strings (3.7).
        let diag = |src: &str| {
            let mut sm = SourceMap::new();
            let id = sm.add(PathBuf::from("t.v2"), "t.v2".into(), src);
            lex(&sm, id).unwrap_err()
        };
        for src in ["@py \"\" { }", "@py \"odd name\" { }", "@py 'stats' { }", "let x = @js \"a\"(n: 1) { n }"] {
            let d = diag(src);
            assert_eq!((d.message.as_str(), d.hint.as_deref()), ("a block label can't be a string", Some(LABEL_HINT)), "{src}");
        }
        let d = diag("@py \"odd name\" { }");
        assert_eq!((d.span.col, d.span.hi - d.span.lo), (5, 10), "the whole string");
        assert_eq!(err("@py {\n x }"), "@py block closes at a } that does not start its line");
        // Code after the `{` too: that is the first mistake, so it is the one reported (3.7).
        assert_eq!(err("print(@py { import json\n    json.dumps([1]) })"), "a multi-line block starts on the line after its {");
        assert_eq!(err("@py { 1 } + 2"), "unexpected text after the } that closes the @py block");
        assert_eq!(err("@py ```python\n```"), "nothing may follow the opening ``` fence on its line");
        assert_eq!(err("@py stats ```\nx\n"), "@py block 'stats' is never closed");
    }

    #[test]
    fn review_0_4_regressions() {
        // A decorator on the same line as func, or before it with a comment between.
        assert_eq!(err("@memo func f() {}"), "expected { or a ``` fence after @memo");
        assert_eq!(err("@memo\n// note\nfunc f() {}"), "expected { or a ``` fence after @memo");
        let mut sm = SourceMap::new();
        let id = sm.add(PathBuf::from("t.v2"), "t.v2".into(), "@memo func f() {}");
        let d = lex(&sm, id).unwrap_err();
        assert!(d.hint.as_deref().unwrap_or("").contains("no decorators"));
        // Messages print canonical tags; C1 points at the opening line with a note at the `}`.
        let (msg, line, col) = err_at("let x = 1\n@python stats {\n  x = 1 }");
        assert_eq!((msg.as_str(), line, col), ("@py block closes at a } that does not start its line", 2, 1));
        assert_eq!(err("@javascript\n{ 1 }"), "the { that opens a block must be on the same line as @js");
        // extern: raw-string paths, no fence advice, no keyword alias.
        assert!(lex_str("extern r\"C:\\libs\\foo.dll\" { int f(void); }").is_ok());
        assert_eq!(err("extern b\"m\" { }"), "the library name must be a plain \"…\" or raw r\"…\" string");
        assert_eq!(err("extern \"m\" as for { }"), "expected a name after `as`, found keyword `for`");
        let id = sm.add(PathBuf::from("e.v2"), "e.v2".into(), "extern \"m\" {\n  double cos(double x); }");
        let d = lex(&sm, id).unwrap_err();
        assert!(!d.hint.as_deref().unwrap_or("").contains("fence"), "extern has no fence form");
        let id = sm.add(PathBuf::from("e2.v2"), "e2.v2".into(), "extern \"m\" { double cos(double x); } junk");
        let d = lex(&sm, id).unwrap_err();
        assert!(!d.hint.as_deref().unwrap_or("").contains("parentheses"));
    }

    #[test]
    fn unterminated_block_points_at_the_at_with_notes() {
        let mut sm = SourceMap::new();
        let id = sm.add(PathBuf::from("t.v2"), "t.v2".into(), "print(1)\n\n@py {\n    d = {\n    s = \"\"\"\n");
        let d = lex(&sm, id).unwrap_err();
        assert_eq!(d.message, "@py block is never closed");
        assert_eq!((d.span.line, d.span.col), (3, 1));
        let notes: Vec<&str> = d.notes.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(notes, vec!["this { inside the block is still open", "this string never closes"]);
        assert!(d.hint.as_deref().unwrap_or("").contains("fence form"));
    }

    #[test]
    fn braces_body_or_dict() {
        assert_eq!(kinds("if x {\n}"), "IF x {B NL } EOF");
        assert_eq!(kinds("let d = {a: 1}"), "LET d = {D a : 1 } EOF");
        assert_eq!(kinds("if a {\n} else {\n}"), "IF a {B NL } ELSE {B NL } EOF");
        assert_eq!(kinds("f({})"), "f ( {D } ) EOF");
        assert_eq!(kinds("for k, v in {a: 1} {}"), "FOR k , v IN {D a : 1 } {B } EOF");
        // After a line break a `{` is a dict brace (the parser then asks for the body `{`).
        assert_eq!(kinds("if x\n{}"), "IF x NL {D } EOF");
    }

    #[test]
    fn bracket_errors() {
        assert_eq!(err("f(1]"), "found `]` but `(` opened at 1:2 is still open");
        assert_eq!(err("f(1"), "`(` is never closed");
        assert_eq!(err("x)"), "unmatched `)`");
    }

    #[test]
    fn keywords_after_dot_are_names() {
        assert_eq!(kinds("h.from"), "h . from EOF");
        assert_eq!(kinds("r?.in"), "r ?. in EOF");
        assert_eq!(kinds("x in xs"), "x IN xs EOF");
    }

    #[test]
    fn comments() {
        assert_eq!(kinds("a // c\nb"), "a NL b EOF");
        assert_eq!(kinds("a /* c */ b"), "a b EOF");
        assert_eq!(kinds("a /* c\n */ b"), "a NL b EOF");
        assert_eq!(err("/* never"), "unterminated /* comment");
        assert_eq!(err("# c"), "unexpected character '#'");
    }

    #[test]
    fn pasted_characters_are_named() {
        // What copying code out of a web page, a chat or a word processor brings in (3.2).
        assert_eq!(err("print(\u{201C}hi\u{201D})"), "unexpected character '\u{201C}' (a curly quote)");
        assert_eq!(err("let\u{A0}x = 1"), "unexpected character U+00A0 (a no-break space)");
        assert_eq!(err("let x = 5 \u{2013} 3"), "unexpected character '\u{2013}' (a dash)");
        assert_eq!(err("x\u{200B}"), "unexpected character U+200B (an invisible zero-width character)");
        assert_eq!(err("\u{0}"), "unexpected character U+0000 (a NUL byte)");
        assert_eq!(err("x = 1\u{1B}[31m"), "unexpected character U+001B");
        assert_eq!(err("\u{202E}"), "unexpected character U+202E");
        assert_eq!(shown_char('\u{201C}').1, Some("use a straight quote: \" or '"));
        assert_eq!(shown_char('\u{A0}').1, Some("type a plain space instead"));
        assert_eq!(shown_char('$'), ("'$'".to_string(), None));
    }

    #[test]
    fn operators() {
        assert_eq!(kinds("a ** b ?? c ?. d ... e"), "a ** b ?? c ?. d ... e EOF");
        assert_eq!(kinds("x += 1"), "x += 1 EOF");
        assert_eq!(err("a & b"), "V2 has no bitwise operators");
        assert_eq!(err("a << 1"), "V2 has no bitwise operators");
        // `^` is Python's and JavaScript's xor, and a power in other languages.
        let mut sm = SourceMap::new();
        let id = sm.add(PathBuf::from("t.v2"), "t.v2".into(), "print(2 ^ 3)");
        assert_eq!(lex(&sm, id).unwrap_err().hint.as_deref(), Some("V2 writes a power as 2 ** 3"));
        assert_eq!(kinds("a && b || c"), "a && b || c EOF");
    }

    #[test]
    fn copied_markdown_and_ctrl_z_say_what_they_are() {
        let diag = |src: &str| {
            let mut sm = SourceMap::new();
            let id = sm.add(PathBuf::from("t.v2"), "t.v2".into(), src);
            lex(&sm, id).unwrap_err()
        };
        assert_eq!(diag("```v2\nprint(1)\n```\n").hint.as_deref(), Some("```v2 and ``` mark an example in a Markdown file: copy only the lines between them"));
        assert_eq!(diag("print(`x`)").hint.as_deref(), Some("V2 writes a template string as f\"text ${x}\""));
        let z = diag("print(1)\n\u{1A}");
        assert_eq!((z.message.as_str(), z.hint.as_deref()), ("unexpected character U+001A (Ctrl+Z, an old end-of-file mark)", Some("delete it: V2 files end where their text ends")));
    }

    #[test]
    fn an_unterminated_string_says_why() {
        // 3.2: the cause on the line itself, before the advice for text over several lines.
        let hint = |src: &str| {
            let mut sm = SourceMap::new();
            let id = sm.add(PathBuf::from("t.v2"), "t.v2".into(), src);
            lex(&sm, id).unwrap_err().hint.unwrap_or_default()
        };
        assert!(hint("print(\"hello\u{201D})").starts_with("the line has a curly quote \u{201D} where the string should end"));
        assert!(hint("print('it's me')").starts_with("an apostrophe inside '…' ends the string"));
        assert!(hint("let p = \"C:\\temp\\\"").starts_with("\\\" is a quote inside the string, so it goes on"));
        assert!(hint("print(\"hello)").starts_with("end the string with \" on this line; for text over several lines"));
    }

    #[test]
    fn numbers() {
        assert_eq!(kinds("0 42 1_000_000 0xff 3.14 1e-3 2.0 1E5"), "0 42 1000000 255 3.14 0.001 2.0 100000.0 EOF");
        assert_eq!(kinds("0..3"), "0 .. 3 EOF");
        assert_eq!(kinds("1.len()"), "1 . len ( ) EOF");
        assert_eq!(kinds("9223372036854775808"), "9223372036854775808 EOF");
        assert_eq!(err("9223372036854775809"), "integer literal does not fit in 64 bits");
        assert_eq!(err("007"), "a number can't start with 0 (leading zeros)");
        assert_eq!(err("0b101"), "V2 has only decimal and 0x hex integer literals");
        assert_eq!(err("0o17"), "V2 has only decimal and 0x hex integer literals");
        assert_eq!(err("12abc"), "a number can't be followed directly by a letter");
        assert_eq!(err("1f"), "a number can't be followed directly by a letter");
        assert_eq!(err("0x"), "`0x` must be followed by hex digits");
        assert_eq!(err("0X1"), "V2 has only decimal and 0x hex integer literals");
        assert_eq!(err("1__0"), "`_` in a number must sit between two digits");
        assert_eq!(err("1e999"), "float literal is out of range");
    }

    #[test]
    fn plain_strings_and_escapes() {
        assert_eq!(string_value(r#""double quotes""#), "double quotes");
        assert_eq!(string_value("'single quotes'"), "single quotes");
        assert_eq!(string_value(r#""a\tb\n\\\"\'\0\$""#), "a\tb\n\\\"'\0$");
        assert_eq!(string_value(r#""\x41\u{1F600}""#), "A\u{1F600}");
        assert_eq!(string_value(r#"r"C:\Users\ada\data.csv""#), r"C:\Users\ada\data.csv");
        assert_eq!(err(r#""\d""#), "unknown escape \\d");
        assert_eq!(err(r#""\x4""#), "\\x takes exactly 2 hex digits");
        assert_eq!(err(r#""\u{110000}""#), "\\u{…} must name a Unicode scalar value in 1 to 6 hex digits");
        assert_eq!(err("\"abc\nx\""), "unterminated string");
        // Python's `n // 2` (3.2), and the comments that are not that habit.
        assert_eq!(err("let half = n // 2"), "`// 2` starts a comment in V2, so this line doesn't divide");
        assert_eq!(err("let x = 7//2\n"), "`// 2` starts a comment in V2, so this line doesn't divide");
        // The other operands of Python's habit: calls, parentheses, indexes, members, floats, and
        // a number with closers or arithmetic after it.
        for (src, left) in [
            ("let h = len(xs) // 2", "len(xs)"),
            ("let h = (a + b) // 2", "(a + b)"),
            ("let h = xs[0] // 2", "xs[0]"),
            ("let h = d.n // 2", "d.n"),
            ("let h = 7.5 // 2", "7.5"),
            ("print(n // 2)", "n"),
            ("f(len(xs) // 2, 3)", "len(xs)"),
            ("let h = n // 2 - 1", "n"),
        ] {
            let mut sm = SourceMap::new();
            let id = sm.add(PathBuf::from("t.v2"), "t.v2".into(), src);
            let d = lex(&sm, id).unwrap_err();
            assert_eq!(d.message, "`// 2` starts a comment in V2, so this line doesn't divide", "{src}");
            assert_eq!(d.hint.as_deref(), Some(&*format!("V2 divides rounding down with div(a, b), as in div({left}, 2)")), "{src}");
        }
        for src in ["print(x)    // 3", "let n = count    // 3", "let n = 3 // 3 retries", "let d = n // half", "let a = n\n// 2", "let p = x // 100%", "foo() // 2 things", "print(\"a\") // 2", "n // 2", "if x { f(y) // 2\n}"] {
            assert!(lex_str(src).is_ok(), "{src}");
        }
        assert_eq!(err("rf\"x\""), "unknown string prefix `rf`");
        assert_eq!(err("B\"x\""), "unknown string prefix `B`");
    }

    #[test]
    fn bytes_literals() {
        assert_eq!(kinds(r#"b"\x00\xffA""#), "b[0, 255, 65] EOF");
        assert_eq!(err("b\"\u{e9}\""), "bytes literals must be ASCII (write \\xNN for other bytes)");
        assert_eq!(err(r#"b"\u{41}""#), "\\u{…} is not allowed in bytes literals (use \\xNN)");
    }

    #[test]
    fn triple_quoted() {
        // DOCS 3.5: indentation up to the closing """ is removed; no trailing newline.
        let src = "let poem = \"\"\"\n    Roses are red,\n    V2 glues them all.\n    \"\"\"";
        match &lex_str(src).unwrap()[3] {
            Tok::Str(s) => assert_eq!(&**s, "Roses are red,\nV2 glues them all."),
            other => panic!("{other:?}"),
        }
        // An empty line before the closing quotes gives a trailing newline.
        assert_eq!(string_value("\"\"\"\n  a\n\n  \"\"\""), "a\n");
        // Whitespace-only lines become empty; deeper indentation is kept.
        assert_eq!(string_value("\"\"\"\n  a\n      \n    b\n  \"\"\""), "a\n\n  b");
        // Escapes are decoded after dedent.
        assert_eq!(string_value("\"\"\"\n  a\\nb\n  \"\"\""), "a\nb");
        assert_eq!(string_value("\"\"\"\n\"\"\""), "");
        assert_eq!(err("\"\"\"text\n\"\"\""), "the text of a \"\"\" string starts on the line after the opening \"\"\"");
        assert_eq!(err("\"\"\"\n  a \"\"\""), "the closing \"\"\" must be on its own line");
        assert_eq!(err("\"\"\"\n a\n  \"\"\""), "indented less than the closing \"\"\", or with different tabs/spaces");
        assert_eq!(err("\"\"\"\n a\n"), "unterminated \"\"\" string");
    }

    #[test]
    fn f_strings() {
        let toks = lex_str(r#"f"Hello, ${name}! 2 + 2 = ${2 + 2}""#).unwrap();
        let Tok::FStr(parts) = &toks[0] else { panic!("{:?}", toks[0]) };
        assert_eq!(parts.len(), 4);
        assert!(matches!(&parts[0], FPartTok::Lit(s) if &**s == "Hello, "));
        assert!(matches!(&parts[1], FPartTok::Expr { toks, fixed: None, .. } if toks.len() == 1));
        // :.Nf format spec
        let toks = lex_str(r#"f"${19.999:.2f}""#).unwrap();
        let Tok::FStr(parts) = &toks[0] else { panic!() };
        assert!(matches!(&parts[0], FPartTok::Expr { fixed: Some(2), .. }));
        // A ternary's `:` is an ordinary token.
        let toks = lex_str(r#"f"${a ? b : c}""#).unwrap();
        let Tok::FStr(parts) = &toks[0] else { panic!() };
        assert!(matches!(&parts[0], FPartTok::Expr { fixed: None, toks, .. } if toks.len() == 5));
        // Nested strings with the same quote work.
        assert!(lex_str(r#"f"${d["k"]}""#).is_ok());
        assert!(lex_str(r#"f"a ${f"b ${c}"} d""#).is_ok());
        // `$` without `{` and `\$` are literal.
        let toks = lex_str(r#"f"cost: $5 \${x}""#).unwrap();
        let Tok::FStr(parts) = &toks[0] else { panic!() };
        assert!(matches!(&parts[0], FPartTok::Lit(s) if &**s == "cost: $5 ${x}"));
        assert_eq!(err(r#"f"${}""#), "empty ${} in an f-string");
        assert_eq!(err("f\"${a\n}\""), "a line break inside ${…} is only allowed in a triple-quoted f-string");
        assert!(lex_str("f\"\"\"\n  ${a +\n  b}\n  \"\"\"").is_ok());
    }
}
