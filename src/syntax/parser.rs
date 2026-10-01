//! Recursive-descent parser (INTERNALS 4.1, 4.2, 4.5).
//!
//! Roadmap 0.2 subset: a program is a sequence of expression statements; expressions are
//! literals, names, f-strings, parentheses and calls with positional and named arguments.
//! The rest of the grammar (operators, statements, blocks) arrives in 0.5.

use super::ast::{Arg, Expr, ExprKind, FPart, Name, Stmt, StmtKind};
use super::span::Span;
use super::token::{FPartTok, Kw, Op, Tok, Token};
use super::Diag;

/// Past this many nested expressions the parser stops (4.1), protecting the host stack.
const MAX_DEPTH: u32 = 256;

pub fn parse_program(toks: Vec<Token>) -> Result<Vec<Stmt>, Diag> {
    let mut p = Parser { toks, pos: 0, depth: 0 };
    let mut stmts = Vec::new();
    loop {
        p.skip_separators();
        if matches!(p.peek(), Tok::Eof) {
            return Ok(stmts);
        }
        stmts.push(p.statement()?);
        match p.peek() {
            Tok::Newline | Tok::Semi | Tok::Eof => {}
            other => {
                return Err(Diag::new(format!("expected end of line, found {other}"), p.span()));
            }
        }
    }
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    depth: u32,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn at_op(&self, op: Op) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }

    fn skip_separators(&mut self) {
        while matches!(self.peek(), Tok::Newline | Tok::Semi) {
            self.bump();
        }
    }

    fn statement(&mut self) -> Result<Stmt, Diag> {
        let expr = self.expr()?;
        let span = expr.span;
        Ok(Stmt { kind: StmtKind::Expr(expr), span })
    }

    fn expr(&mut self) -> Result<Expr, Diag> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Diag::new("nested too deeply", self.span()));
        }
        let r = self.postfix();
        self.depth -= 1;
        r
    }

    fn postfix(&mut self) -> Result<Expr, Diag> {
        let mut e = self.primary()?;
        while self.at_op(Op::LParen) {
            let paren = self.bump().span;
            let args = self.args()?;
            let close = self.toks[self.pos - 1].span;
            let span = e.span.to(close);
            e = Expr { kind: ExprKind::Call { callee: Box::new(e), args, paren }, span };
        }
        Ok(e)
    }

    /// Arguments after `(`, through the closing `)`. Named args come after positional ones (4.1).
    fn args(&mut self) -> Result<Vec<Arg>, Diag> {
        let mut args = Vec::new();
        let mut named_seen: Option<Span> = None;
        loop {
            if self.at_op(Op::RParen) {
                self.bump();
                return Ok(args);
            }
            let is_named = matches!(self.peek(), Tok::Ident(_))
                && matches!(self.toks.get(self.pos + 1).map(|t| &t.tok), Some(Tok::Op(Op::Colon)));
            if is_named {
                let t = self.bump();
                let Tok::Ident(text) = t.tok else { unreachable!() };
                self.bump(); // :
                let value = self.expr()?;
                if args.iter().any(|a| matches!(a, Arg::Named(n, _) if n.text == text)) {
                    return Err(Diag::new(format!("argument '{text}' is given twice"), t.span));
                }
                named_seen.get_or_insert(t.span);
                args.push(Arg::Named(Name { text, span: t.span }, value));
            } else {
                let value = self.expr()?;
                if let Some(first) = named_seen {
                    return Err(Diag::new("a positional argument can't follow a named one", value.span)
                        .note("the first named argument", first));
                }
                args.push(Arg::Pos(value));
            }
            if self.at_op(Op::Comma) {
                self.bump();
            } else if !self.at_op(Op::RParen) {
                return Err(Diag::new(format!("expected `,` or `)`, found {}", self.peek()), self.span()));
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, Diag> {
        let t = self.bump();
        let kind = match t.tok {
            Tok::Int(n) => match i64::try_from(n) {
                Ok(v) => ExprKind::Int(v),
                Err(_) => return Err(Diag::new("integer literal does not fit in 64 bits", t.span)),
            },
            Tok::Float(x) => ExprKind::Float(x),
            Tok::Str(s) => ExprKind::Str(s),
            Tok::Bytes(b) => ExprKind::Bytes(b),
            Tok::FStr(parts) => ExprKind::FStr(self.fstring(parts)?),
            Tok::Ident(n) => ExprKind::Name(n),
            Tok::Kw(Kw::True) => ExprKind::Bool(true),
            Tok::Kw(Kw::False) => ExprKind::Bool(false),
            Tok::Kw(Kw::Null) => ExprKind::Null,
            Tok::Op(Op::LParen) => {
                let inner = self.expr()?;
                if !self.at_op(Op::RParen) {
                    return Err(Diag::new(format!("expected `)`, found {}", self.peek()), self.span()));
                }
                self.bump();
                // The inner expression keeps its own span, so error carets land on the name or
                // operator inside the parentheses (2.2), not on the `(`.
                return Ok(inner);
            }
            other => return Err(Diag::new(format!("expected an expression, found {other}"), t.span)),
        };
        Ok(Expr { kind, span: t.span })
    }

    /// Each `${…}` holds exactly one expression (3.5).
    fn fstring(&mut self, parts: Vec<FPartTok>) -> Result<Vec<FPart>, Diag> {
        let mut out = Vec::with_capacity(parts.len());
        for part in parts {
            match part {
                FPartTok::Lit(s) => out.push(FPart::Lit(s)),
                FPartTok::Expr { mut toks, fixed, span } => {
                    let end = Span { lo: span.hi, ..span };
                    toks.push(Token { tok: Tok::Eof, span: end });
                    let mut sub = Parser { toks, pos: 0, depth: self.depth };
                    let expr = sub.expr()?;
                    if !matches!(sub.peek(), Tok::Eof) {
                        return Err(Diag::new(
                            format!("expected `}}` to close the ${{…}}, found {}", sub.peek()),
                            sub.span(),
                        ));
                    }
                    out.push(FPart::Expr { expr, fixed });
                }
            }
        }
        Ok(out)
    }
}

/// Convenience for tests and callers holding source text.
#[cfg(test)]
pub fn parse_str(src: &str) -> Result<Vec<Stmt>, String> {
    let mut sm = super::span::SourceMap::new();
    let id = sm.add(std::path::PathBuf::from("t.v2"), "t.v2".into(), src);
    let toks = super::lexer::lex(&sm, id).map_err(|d| d.message)?;
    parse_program(toks).map_err(|d| d.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_and_args() {
        let prog = parse_str("print(\"hi\", 1, sep: \"-\")\nprint()").unwrap();
        assert_eq!(prog.len(), 2);
        let StmtKind::Expr(Expr { kind: ExprKind::Call { args, .. }, .. }) = &prog[0].kind else { panic!() };
        assert_eq!(args.len(), 3);
        assert!(matches!(&args[2], Arg::Named(n, _) if &*n.text == "sep"));
    }

    #[test]
    fn errors() {
        assert_eq!(parse_str("print(sep: 1, 2)").unwrap_err(), "a positional argument can't follow a named one");
        assert_eq!(parse_str("print(a: 1, a: 2)").unwrap_err(), "argument 'a' is given twice");
        assert_eq!(parse_str("print(1 2)").unwrap_err(), "expected `,` or `)`, found number `2`");
        assert_eq!(parse_str("print(1) print(2)").unwrap_err(), "expected end of line, found name `print`");
        assert_eq!(parse_str("9223372036854775808").unwrap_err(), "integer literal does not fit in 64 bits");
        assert_eq!(parse_str("let x = 1").unwrap_err(), "expected an expression, found keyword `let`");
    }

    #[test]
    fn fstrings_parse_their_parts() {
        assert!(parse_str("print(f\"${greet(\"x\")}!\")").is_ok());
        assert_eq!(parse_str("print(f\"${a b}\")").unwrap_err(), "expected `}` to close the ${…}, found name `b`");
    }
}
