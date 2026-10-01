//! The lexer (INTERNALS ch. 3): normalized text in, one `Vec<Token>` out.
//!
//! Covers 3.1-3.5: tokens, keywords, operators, comments, where statements end, numbers and
//! every string form. Embedded blocks and `extern` bodies (3.6-3.9) arrive in roadmap 0.4;
//! until then `@` is an unexpected character.

use super::span::{FileId, SourceMap, Span};
use super::token::{BraceKind, FPartTok, Kw, Op, Tok, Token};
use super::Diag;

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
}

impl Open {
    fn opener(self) -> &'static str {
        match self {
            Open::Paren => "(",
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
                '#' => {
                    return Err(Diag::new("unexpected character '#'", self.span_here(1)).hint("comments start with //"))
                }
                _ => self.operator(m)?,
            }
        }
    }

    fn end_of_file(&mut self) -> Result<Option<u8>, Diag> {
        if let Some(&(open, span)) = self.stack.last() {
            return Err(Diag::new(format!("`{}` is never closed", open.opener()), span));
        }
        let m = self.mark();
        self.push(Tok::Eof, m);
        Ok(None)
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
        if matches!(self.stack.last(), Some((Open::Paren | Open::Bracket | Open::Dict | Open::Interp, _))) {
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
            ')' => open == Open::Paren,
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
                return Err(Diag::new("V2 has no bitwise operators", self.span_here(bad.chars().count())));
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
        let mut d = Diag::new(format!("unexpected character '{c}'"), self.span_here(1));
        if c == '@' {
            d = d.hint("embedded blocks (@py { … }) are not available in this build yet");
        }
        Err(d)
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
        self.push(tok, m);
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

    /// `"…"` / `'…'`: no line breaks inside.
    fn one_line_body(&mut self, kind: StrKind, quote: char) -> Result<Vec<RawPart>, Diag> {
        let q = self.mark(); // the opening quote: where "unterminated string" points
        self.bump();
        let mut parts = Vec::new();
        let mut lit = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => {
                    return Err(Diag::new("unterminated string", Span { hi: q.pos as u32 + 1, ..self.span_from(q) })
                        .hint("write \\n for a line break, or use a \"\"\" string"));
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
        let bad = |what: &str| {
            Diag::new(format!("unknown escape {what}"), at).hint("write \\\\ for a backslash, or use a raw string r\"…\"")
        };
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
                    return Err(Diag::new("\\x takes exactly 2 hex digits", at));
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
                    return Err(Diag::new("\\u must be followed by {hex digits}, as in \\u{1F600}", at));
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

/// `_` only between two digits (3.4).
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
    .filter(lambda(s) => s.starts_with("a"))   // a line starting with `.` continues
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
    fn operators() {
        assert_eq!(kinds("a ** b ?? c ?. d ... e"), "a ** b ?? c ?. d ... e EOF");
        assert_eq!(kinds("x += 1"), "x += 1 EOF");
        assert_eq!(err("a & b"), "V2 has no bitwise operators");
        assert_eq!(err("a << 1"), "V2 has no bitwise operators");
        assert_eq!(kinds("a && b || c"), "a && b || c EOF");
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
