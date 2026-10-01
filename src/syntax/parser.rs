//! Recursive-descent parser (INTERNALS 4.1, 4.2, 4.5).
//!
//! Roadmap 0.2-0.4 subset: a program is a sequence of expression statements and blocks;
//! expressions are literals, names, f-strings, parentheses, calls with positional and named
//! arguments, and action blocks. The rest of the grammar (operators, statements, `extern`,
//! `@import`) arrives in 0.5.

use std::rc::Rc;

use super::ast::{Arg, EngineBlock, Expr, ExprKind, FPart, ImportItems, Name, Stmt, StmtKind, UnitId};
use super::directives;
use super::span::{SourceMap, Span};
use super::token::{FPartTok, Kw, Op, Tok, Token};
use super::Diag;

/// Past this many nested expressions the parser stops (4.1), protecting the host stack.
const MAX_DEPTH: u32 = 256;

/// The fixed compiled tags; their blocks take no args (4.5, DOCS 4.4).
const COMPILED_TAGS: &[&str] = &["c", "cpp", "cxx", "rust", "rs"];

pub fn parse_program(sm: &SourceMap, toks: Vec<Token>) -> Result<Vec<Stmt>, Diag> {
    let mut p = Parser { sm, toks, pos: 0, depth: 0 };
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

struct Parser<'a> {
    sm: &'a SourceMap,
    toks: Vec<Token>,
    pos: usize,
    depth: u32,
}

impl<'a> Parser<'a> {
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
        if matches!(self.peek(), Tok::BlockHead { .. }) {
            let b = self.block()?;
            if self.at_op(Op::LParen) || self.at_op(Op::Dot) || self.at_op(Op::LBracket) || self.at_op(Op::QDot) {
                return Err(Diag::new("a block value must be in parentheses here: (@py { … }).m()", self.span()));
            }
            let span = b.span;
            return Ok(Stmt { kind: StmtKind::Block(b), span });
        }
        let expr = self.expr()?;
        let span = expr.span;
        Ok(Stmt { kind: StmtKind::Expr(expr), span })
    }

    fn expr(&mut self) -> Result<Expr, Diag> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Diag::new("nested too deeply", self.span()));
        }
        let r = if matches!(self.peek(), Tok::BlockHead { .. }) { self.block_value() } else { self.postfix() };
        self.depth -= 1;
        r
    }

    /// A block used as a value must be an action block, and nothing may be applied to it
    /// without parentheses (4.1, 4.5).
    fn block_value(&mut self) -> Result<Expr, Diag> {
        let b = self.block()?;
        if let Some(ex) = &b.directives.exports {
            let at = ex.at.unwrap_or(b.span);
            return Err(Diag::new("a block with @export is a definition block and cannot be used as a value", b.tag.span)
                .note("the @export that makes it one", at));
        }
        if self.at_op(Op::LParen) || self.at_op(Op::Dot) || self.at_op(Op::LBracket) || self.at_op(Op::QDot) {
            return Err(Diag::new("a block value must be in parentheses here: (@py { … }).m()", self.span()));
        }
        let span = b.span;
        Ok(Expr { kind: ExprKind::Block(b), span })
    }

    /// `@tag [label] [( args )] body` (4.1).
    fn block(&mut self) -> Result<Rc<EngineBlock>, Diag> {
        let head = self.bump();
        let Tok::BlockHead { tag, label } = head.tok else { unreachable!("called at a block head") };
        let at = head.span;
        let tag_hi = at.lo + 1 + tag.len() as u32;
        let tag_name = Name { text: tag.clone(), span: Span { hi: tag_hi, ..at } };
        let label = label.map(|(text, span)| Name { text, span });
        let mut args: Vec<(Name, Expr)> = Vec::new();
        if self.at_op(Op::LParen) {
            let open = self.bump().span;
            loop {
                if self.at_op(Op::RParen) {
                    self.bump();
                    break;
                }
                let t = self.bump();
                let Tok::Ident(text) = t.tok else {
                    return Err(Diag::new(format!("expected a block arg name, found {}", t.tok), t.span)
                        .hint("block args are `name` or `name: expr`"));
                };
                let name = Name { text, span: t.span };
                let value = if self.at_op(Op::Colon) {
                    self.bump();
                    self.expr()?
                } else {
                    Expr { kind: ExprKind::Name(name.text.clone()), span: name.span }
                };
                if let Some((prev, _)) = args.iter().find(|(n, _)| n.text == name.text) {
                    return Err(Diag::new(format!("block arg '{}' is given twice", name.text), name.span)
                        .note("first given here", prev.span));
                }
                args.push((name, value));
                if self.at_op(Op::Comma) {
                    self.bump();
                } else if !self.at_op(Op::RParen) {
                    return Err(Diag::new(format!("expected `,` or `)`, found {}", self.peek()), self.span()));
                }
            }
            if COMPILED_TAGS.contains(&&*tag) && !args.is_empty() {
                return Err(Diag::new("compiled blocks take no args — @export a function and call it with arguments", open));
            }
        }
        let body_tok = self.bump();
        let Tok::BlockBody(raw) = body_tok.tok else {
            return Err(Diag::new(format!("expected a block body, found {}", body_tok.tok), body_tok.span));
        };
        let processed = directives::process(self.sm, &raw, &tag)?;
        // A block arg may not share a name with what an inner @import binds (12.1, 13.7).
        for imp in &processed.directives.imports {
            let bound: Vec<(Rc<str>, Span)> = match &imp.items {
                ImportItems::Names(items) => items
                    .iter()
                    .map(|i| match &i.alias {
                        Some(a) => (a.text.clone(), a.span),
                        None => (i.name.clone(), i.span),
                    })
                    .collect(),
                ImportItems::Module(alias) => vec![(alias.text.clone(), alias.span)],
                ImportItems::All(_) => Vec::new(),
            };
            for (name, at) in bound {
                if let Some((arg, _)) = args.iter().find(|(n, _)| n.text == name) {
                    return Err(Diag::new(format!("'{name}' is already declared in this block"), arg.span)
                        .note("also bound by this @import", at));
                }
            }
        }
        Ok(Rc::new(EngineBlock {
            unit_id: UnitId { file: at.file, offset: at.lo },
            tag: tag_name,
            label,
            args,
            body: processed.body,
            loc: processed.loc,
            body_span: raw.text_span,
            delimiter: raw.delim,
            directives: processed.directives,
            span: at.to(raw.close),
        }))
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
                    let mut sub = Parser { sm: self.sm, toks, pos: 0, depth: self.depth };
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

/// Convenience for tests holding source text.
#[cfg(test)]
pub fn parse_str(src: &str) -> Result<Vec<Stmt>, String> {
    let mut sm = SourceMap::new();
    let id = sm.add(std::path::PathBuf::from("t.v2"), "t.v2".into(), src);
    let toks = super::lexer::lex(&sm, id).map_err(|d| d.message)?;
    parse_program(&sm, toks).map_err(|d| d.message)
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

    #[test]
    fn blocks() {
        let prog = parse_str("@py stats {\n    def f(): pass\n    @export { f }\n}\nprint(@py(n, k: 3) { n * k })").unwrap();
        let StmtKind::Block(b) = &prog[0].kind else { panic!("statement block") };
        assert_eq!(&*b.tag.text, "py");
        assert_eq!(b.label.as_ref().unwrap().text.as_ref(), "stats");
        assert!(b.directives.exports.is_some());
        let StmtKind::Expr(Expr { kind: ExprKind::Call { args, .. }, .. }) = &prog[1].kind else { panic!() };
        let Arg::Pos(Expr { kind: ExprKind::Block(v), .. }) = &args[0] else { panic!("block value") };
        assert_eq!(v.args.len(), 2);
        assert!(matches!(&v.args[0].1.kind, ExprKind::Name(n) if &**n == "n"));
    }

    #[test]
    fn block_static_checks() {
        assert_eq!(
            parse_str("print(@py {\n  def f(): pass\n  @export { f }\n})").unwrap_err(),
            "a block with @export is a definition block and cannot be used as a value"
        );
        assert_eq!(parse_str("@c(x) { int y; }").unwrap_err(), "compiled blocks take no args — @export a function and call it with arguments");
        assert_eq!(parse_str("@py(a, a: 1) { a }").unwrap_err(), "block arg 'a' is given twice");
        assert_eq!(
            parse_str("@py(m) {\n  @import { mean as m } from py.statistics\n}").unwrap_err(),
            "'m' is already declared in this block"
        );
        assert_eq!(parse_str("@py(np) {\n  @import py.numpy as np\n}").unwrap_err(), "'np' is already declared in this block");
        assert_eq!(parse_str("@py { 1 }\n.x").unwrap_err(), "a block value must be in parentheses here: (@py { … }).m()");
    }
}
