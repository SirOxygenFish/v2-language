use crate::ast::*;
use crate::token::{Token, TokenKind};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    pending_doc_comment: Option<String>,
    /// When true, a `{` following an identifier is NOT treated as a struct
    /// literal — it is a block delimiter instead. Set while parsing the header
    /// expressions of `if`/`while`/`for`/`match` and the RHS of `if let`/`while let`,
    /// mirroring Rust's `NO_STRUCT_LITERAL` restriction. Reset inside any
    /// parenthesized/bracketed sub-expression so nested struct literals still parse.
    no_struct_literal: bool,
    /// `newline_before[i]` is true when one or more newline tokens preceded the
    /// retained token at index `i` in the original lexer stream. Newlines are
    /// filtered out for parsing, but this preserves statement-boundary info used
    /// to disambiguate the postfix try operator `expr?` from the ternary `a ? b : c`.
    newline_before: Vec<bool>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        // Filter out newlines for simpler parsing (V2 is not newline-sensitive),
        // but record whether a newline preceded each retained token.
        let mut filtered: Vec<Token> = Vec::with_capacity(tokens.len());
        let mut newline_before: Vec<bool> = Vec::with_capacity(tokens.len());
        let mut pending_newline = false;
        for t in tokens.into_iter() {
            if matches!(t.kind, TokenKind::Newline) {
                pending_newline = true;
            } else {
                newline_before.push(pending_newline);
                filtered.push(t);
                pending_newline = false;
            }
        }
        Self {
            tokens: filtered,
            pos: 0,
            pending_doc_comment: None,
            no_struct_literal: false,
            newline_before,
        }
    }

    /// True if a newline separated the token at `idx` from the previous one.
    fn newline_before(&self, idx: usize) -> bool {
        self.newline_before.get(idx).copied().unwrap_or(true)
    }

    /// Parse an expression with struct-literal syntax suppressed (header context).
    fn parse_cond_expr(&mut self) -> Result<Expr, String> {
        let prev = self.no_struct_literal;
        self.no_struct_literal = true;
        let r = self.parse_expr();
        self.no_struct_literal = prev;
        r
    }

    /// Parse a single chain operand for `if let`/`while let`: struct literals are
    /// suppressed and parsing stops below `&&`/`||` so a following `&& let ...`
    /// chain element is not swallowed into this expression.
    fn parse_cond_operand(&mut self) -> Result<Expr, String> {
        let prev = self.no_struct_literal;
        self.no_struct_literal = true;
        let r = self.parse_bitwise_or();
        self.no_struct_literal = prev;
        r
    }

    /// Parse an expression with struct-literal syntax re-enabled (inside delimiters).
    fn parse_expr_allow_struct(&mut self) -> Result<Expr, String> {
        let prev = self.no_struct_literal;
        self.no_struct_literal = false;
        let r = self.parse_expr();
        self.no_struct_literal = prev;
        r
    }

    pub fn parse(&mut self) -> Result<Program, String> {
        let mut stmts = Vec::new();
        while !self.is_at_end() {
            self.consume_doc_comments();
            if self.is_at_end() {
                break;
            }
            stmts.push(self.parse_stmt()?);
        }
        Ok(Program { stmts })
    }

    /// Public entry point for parsing a single expression (used by f-string eval).
    pub fn parse_expr_public(&mut self) -> Result<Expr, String> {
        self.parse_expr()
    }

    // ── Helpers ──────────────────────────────────────────

    fn peek(&self) -> &TokenKind {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos].kind
        } else {
            &TokenKind::Eof
        }
    }

    fn peek_token(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn current_line(&self) -> usize {
        self.peek_token().line
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos];
        self.pos += 1;
        tok
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn check(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.peek()) == std::mem::discriminant(kind)
    }

    fn match_tok(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<&Token, String> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            Err(format!(
                "[line {}] Error: Expected {:?}, found {:?}",
                self.current_line(),
                kind,
                self.peek()
            ))
        }
    }

    fn expect_ident(&mut self) -> Result<Ident, String> {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                Ok(name)
            }
            // Contextual (soft) keywords: usable as ordinary identifiers in name
            // positions (function/method/param/field names), e.g. `func from(..)`
            // for the `From` conversion trait, or a field named `type`/`error`.
            other => {
                if let Some(name) = Self::soft_keyword_text(&other) {
                    self.advance();
                    Ok(name.to_string())
                } else {
                    Err(format!(
                        "[line {}] Error: Expected identifier, found {:?}",
                        self.current_line(),
                        self.peek()
                    ))
                }
            }
        }
    }

    /// Keywords that may also serve as identifiers when a name is required.
    /// Conservative list: only tokens that appear as names in the docs and can
    /// never begin a statement/expression in the positions that call expect_ident.
    fn soft_keyword_text(tok: &TokenKind) -> Option<&'static str> {
        match tok {
            TokenKind::From => Some("from"),
            TokenKind::Type => Some("type"),
            TokenKind::Default => Some("default"),
            TokenKind::Label => Some("label"),
            _ => None,
        }
    }

    /// Parse a field or method name after `.`/`?.`. Because the leading dot
    /// removes ambiguity, most keywords are accepted as ordinary member names
    /// (e.g. `Type.new(..)`, `x.match(..)`, `opt.default()`, `t.type`).
    fn expect_field_name(&mut self) -> Result<Ident, String> {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                Ok(name)
            }
            other => {
                if let Some(txt) = Self::keyword_field_text(&other) {
                    self.advance();
                    Ok(txt.to_string())
                } else {
                    Err(format!(
                        "[line {}] Error: Expected field name, found {:?}",
                        self.current_line(),
                        self.peek()
                    ))
                }
            }
        }
    }

    /// Keywords that may appear as a member name after `.`/`?.`.
    fn keyword_field_text(tok: &TokenKind) -> Option<&'static str> {
        Some(match tok {
            TokenKind::New => "new",
            TokenKind::From => "from",
            TokenKind::Type => "type",
            TokenKind::Default => "default",
            TokenKind::Match => "match",
            TokenKind::As => "as",
            TokenKind::In => "in",
            TokenKind::For => "for",
            TokenKind::If => "if",
            TokenKind::Async => "async",
            TokenKind::Await => "await",
            TokenKind::Label => "label",
            _ => return None,
        })
    }

    fn expect_decorator_name(&mut self) -> Result<Ident, String> {
        match self.peek().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                Ok(name)
            }
            TokenKind::Pure => {
                self.advance();
                Ok("pure".to_string())
            }
            TokenKind::Sealed => {
                self.advance();
                Ok("sealed".to_string())
            }
            TokenKind::TestBlock => {
                self.advance();
                Ok("test".to_string())
            }
            TokenKind::BenchBlock => {
                self.advance();
                Ok("bench".to_string())
            }
            _ => Err(format!(
                "[line {}] Error: Expected decorator name, found {:?}",
                self.current_line(),
                self.peek()
            )),
        }
    }

    fn expect_string(&mut self) -> Result<String, String> {
        match self.peek().clone() {
            TokenKind::Str(s) => {
                self.advance();
                Ok(s)
            }
            _ => Err(format!(
                "[line {}] Error: Expected string literal, found {:?}",
                self.current_line(),
                self.peek()
            )),
        }
    }

    fn current_stmt_can_take_doc_comment(&self) -> bool {
        match self.peek() {
            TokenKind::Const
            | TokenKind::Func
            | TokenKind::Pure
            | TokenKind::Class
            | TokenKind::Sealed
            | TokenKind::Struct
            | TokenKind::Enum
            | TokenKind::Trait
            | TokenKind::Type
            | TokenKind::Macro
            | TokenKind::Newtype
            | TokenKind::CStruct
            | TokenKind::At => true,
            TokenKind::Async => self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&TokenKind::Func),
            _ => false,
        }
    }

    fn push_pending_doc_comment(&mut self, doc: String) {
        if doc.is_empty() {
            return;
        }
        if let Some(existing) = &mut self.pending_doc_comment {
            existing.push('\n');
            existing.push_str(&doc);
        } else {
            self.pending_doc_comment = Some(doc);
        }
    }

    fn consume_doc_comments(&mut self) {
        while let TokenKind::DocComment(text) = self.peek().clone() {
            self.advance();
            self.push_pending_doc_comment(text);
        }
    }

    fn take_doc_comment(&mut self) -> Option<String> {
        self.pending_doc_comment.take()
    }

    fn skip_generic_params_if_present(&mut self) -> Result<(), String> {
        if !self.check(&TokenKind::Lt) {
            return Ok(());
        }

        self.advance();
        let mut depth = 1;
        while depth > 0 {
            if self.is_at_end() {
                return Err(format!(
                    "[line {}] Error: Unterminated generic parameter list",
                    self.current_line()
                ));
            }
            match self.peek() {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt => depth -= 1,
                _ => {}
            }
            self.advance();
        }
        Ok(())
    }

    fn skip_where_clause_if_present(&mut self) {
        if !self.match_tok(&TokenKind::Where) {
            return;
        }
        while !self.check(&TokenKind::LBrace) && !self.is_at_end() {
            self.advance();
        }
    }

    fn parse_type_annotation(&mut self) -> Result<String, String> {
        let mut result = String::new();
        let mut angle_depth = 0usize;
        let mut delim_depth = 0usize; // depth of () and [] in the type
        let mut saw_any = false;

        loop {
            // A type annotation never spans a statement boundary unless we are
            // inside a delimiter — stop at a newline once we have a type.
            if saw_any && angle_depth == 0 && delim_depth == 0 && self.newline_before(self.pos) {
                break;
            }
            match self.peek().clone() {
                // Function type: func(int, int) -> bool
                TokenKind::Func => {
                    result.push_str("func");
                    self.advance();
                    saw_any = true;
                }
                TokenKind::Arrow => {
                    result.push_str(" -> ");
                    self.advance();
                    saw_any = true;
                }
                TokenKind::LParen => {
                    delim_depth += 1;
                    result.push('(');
                    self.advance();
                    saw_any = true;
                }
                TokenKind::RParen if delim_depth > 0 => {
                    delim_depth -= 1;
                    result.push(')');
                    self.advance();
                }
                TokenKind::LBracket => {
                    delim_depth += 1;
                    result.push('[');
                    self.advance();
                    saw_any = true;
                }
                TokenKind::RBracket if delim_depth > 0 => {
                    delim_depth -= 1;
                    result.push(']');
                    self.advance();
                }
                TokenKind::Comma if delim_depth > 0 => {
                    result.push_str(", ");
                    self.advance();
                }
                TokenKind::Ident(name) => {
                    result.push_str(&name);
                    self.advance();
                    saw_any = true;
                }
                TokenKind::Self_ => {
                    result.push_str("self");
                    self.advance();
                    saw_any = true;
                }
                TokenKind::Dyn => {
                    if !result.is_empty() {
                        result.push(' ');
                    }
                    result.push_str("dyn");
                    self.advance();
                    saw_any = true;
                }
                TokenKind::Ref => {
                    if !result.is_empty() && !result.ends_with('&') {
                        result.push(' ');
                    }
                    result.push_str("ref");
                    self.advance();
                    saw_any = true;
                }
                TokenKind::Mut => {
                    if !result.is_empty() && !result.ends_with('&') {
                        result.push(' ');
                    }
                    result.push_str("mut");
                    self.advance();
                    saw_any = true;
                }
                TokenKind::BitAnd => {
                    result.push('&');
                    self.advance();
                    saw_any = true;
                }
                TokenKind::ColonColon => {
                    result.push_str("::");
                    self.advance();
                    saw_any = true;
                }
                TokenKind::Lt => {
                    angle_depth += 1;
                    result.push('<');
                    self.advance();
                    saw_any = true;
                }
                TokenKind::Gt if angle_depth > 0 => {
                    angle_depth -= 1;
                    result.push('>');
                    self.advance();
                    saw_any = true;
                }
                TokenKind::Comma if angle_depth > 0 => {
                    result.push_str(", ");
                    self.advance();
                }
                TokenKind::Plus => {
                    if !result.ends_with(' ') && !result.is_empty() {
                        result.push(' ');
                    }
                    result.push('+');
                    result.push(' ');
                    self.advance();
                    saw_any = true;
                }
                TokenKind::BitOr => {
                    // Union type: int | str
                    if !result.ends_with(' ') && !result.is_empty() {
                        result.push(' ');
                    }
                    result.push('|');
                    result.push(' ');
                    self.advance();
                    saw_any = true;
                }
                TokenKind::Never => {
                    result.push_str("never");
                    self.advance();
                    saw_any = true;
                }
                // `null` as a union member: int | str | null
                TokenKind::Null => {
                    result.push_str("null");
                    self.advance();
                    saw_any = true;
                }
                _ => break,
            }
        }

        if saw_any {
            Ok(result)
        } else {
            Err(format!(
                "[line {}] Error: Expected type annotation, found {:?}",
                self.current_line(),
                self.peek()
            ))
        }
    }

    // ── Statement Parsing ────────────────────────────────

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        self.consume_doc_comments();
        if !self.current_stmt_can_take_doc_comment() {
            self.pending_doc_comment = None;
        }
        match self.peek().clone() {
            TokenKind::Let => self.parse_let(),
            TokenKind::Const => self.parse_const(),
            TokenKind::Func | TokenKind::Pure => self.parse_func_decl(),
            TokenKind::Async => {
                if self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&TokenKind::Func) {
                    self.parse_func_decl()
                } else {
                    self.parse_expr_stmt()
                }
            }
            TokenKind::Return => self.parse_return(),
            TokenKind::If => self.parse_if(),
            TokenKind::While => self.parse_while(),
            TokenKind::For => self.parse_for(),
            TokenKind::Break => {
                self.advance();
                if let TokenKind::Ident(lbl) = self.peek().clone() {
                    self.advance();
                    Ok(Stmt::BreakLabel(lbl))
                } else {
                    Ok(Stmt::Break)
                }
            }
            TokenKind::Continue => {
                self.advance();
                if let TokenKind::Ident(lbl) = self.peek().clone() {
                    self.advance();
                    Ok(Stmt::ContinueLabel(lbl))
                } else {
                    Ok(Stmt::Continue)
                }
            }
            // A bare `...` at statement position is an elision placeholder
            // (common in documentation to stand in for omitted code) — a no-op.
            TokenKind::Ellipsis => {
                self.advance();
                if matches!(self.peek(), TokenKind::Ident(_)) {
                    self.advance();
                }
                Ok(Stmt::Multi(Vec::new()))
            }
            TokenKind::Match => self.parse_match(),
            TokenKind::Throw => self.parse_throw(),
            TokenKind::Try => self.parse_try_catch(),
            TokenKind::Defer => self.parse_defer(),
            TokenKind::Class | TokenKind::Sealed => self.parse_class_decl(),
            TokenKind::Struct => self.parse_struct_decl(),
            TokenKind::Enum => self.parse_enum_decl(),
            TokenKind::Trait => self.parse_trait_decl(),
            TokenKind::Impl => self.parse_impl_block(),
            TokenKind::Extend => self.parse_extend_block(),
            TokenKind::Import => self.parse_import(),
            TokenKind::Label => self.parse_label(),
            TokenKind::Goto => self.parse_goto(),
            TokenKind::Yield => self.parse_yield(),
            // `test "name" { }` is a test block; `test.hook(..)` / `test(..)` uses
            // `test` as the std.test module object — parse as an expression instead.
            TokenKind::TestBlock
                if matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Dot) | Some(TokenKind::LParen) | Some(TokenKind::QuestionDot)
                ) =>
            {
                self.parse_expr_stmt()
            }
            TokenKind::TestBlock => self.parse_test_block(),
            TokenKind::BenchBlock => self.parse_bench_block(),
            TokenKind::At => self.parse_decorated_func(),
            TokenKind::Type => self.parse_type_alias(),
            TokenKind::Using => self.parse_using(),
            TokenKind::StaticAssert => self.parse_static_assert(),
            TokenKind::Macro => self.parse_macro_decl(),
            TokenKind::Newtype => self.parse_newtype_decl(),
            TokenKind::Comptime => self.parse_comptime_block(),
            TokenKind::CStruct => self.parse_cstruct_decl(),
            TokenKind::Unsafe => self.parse_unsafe_block(),
            TokenKind::Actor => self.parse_actor_decl(false),
            TokenKind::Agent => self.parse_actor_decl(true),
            TokenKind::Isolate => self.parse_isolate_block(),
            TokenKind::Bitfield => self.parse_bitfield_struct(),
            TokenKind::Enable => self.parse_enable_langs(),
            TokenKind::Extern => self.parse_extern(),
            TokenKind::Cimport => self.parse_cimport(),
            // Visibility modifiers: consume and parse next statement
            TokenKind::Pub | TokenKind::Private | TokenKind::Internal => {
                self.advance(); // skip pub/private/internal
                // Handle pub(crate) and pub(super)
                if self.check(&TokenKind::LParen) {
                    self.advance(); // skip (
                    if let TokenKind::Ident(_) = self.peek().clone() {
                        self.advance(); // skip crate/super
                    }
                    self.expect(&TokenKind::RParen)?; // skip )
                }
                self.parse_stmt()
            }
            TokenKind::LBrace => {
                self.advance();
                let stmts = self.parse_block_body()?;
                Ok(Stmt::Block(stmts))
            }
            // Check for labeled loop: `name: for ...` or `name: while ...`
            TokenKind::Ident(_) => {
                if self.pos + 2 < self.tokens.len() {
                    let next = &self.tokens[self.pos + 1].kind;
                    let after = &self.tokens[self.pos + 2].kind;
                    if *next == TokenKind::Colon
                        && (*after == TokenKind::For || *after == TokenKind::While)
                    {
                        let label_name = self.expect_ident()?;
                        self.advance(); // skip ':'
                        // Parse the loop statement
                        let loop_stmt = self.parse_stmt()?;
                        // Wrap: label then loop, so exec_block_no_scope catches it
                        return Ok(Stmt::Block(vec![
                            Stmt::Label(label_name),
                            loop_stmt,
                        ]));
                    }
                }
                self.parse_expr_stmt()
            }
            _ => self.parse_expr_stmt(),
        }
    }

    fn parse_let(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'let'

        // Handle destructuring: let [a, b] = expr  or  let {a, b} = expr
        if self.check(&TokenKind::LBracket) {
            return self.parse_let_destructure();
        }
        if self.check(&TokenKind::LBrace) {
            return self.parse_let_dict_destructure();
        }
        // Handle tuple destructuring: let (a, b) = expr
        // But NOT let Ok(x) = ... or let Some(x) = ...
        if self.check(&TokenKind::LParen) {
            // Lookahead: if next is Ident and then Comma, it's tuple destructure
            // let (a, b, c) = expr
            if self.pos + 2 < self.tokens.len() {
                let next1 = &self.tokens[self.pos + 1].kind;
                let next2 = &self.tokens[self.pos + 2].kind;
                if matches!(next1, TokenKind::Ident(_)) && matches!(next2, TokenKind::Comma | TokenKind::RParen) {
                    return self.parse_let_tuple_destructure();
                }
            }
        }

        // Check for `let Ok(var) = expr else { }` or `let Some(var) = expr else { }`
        if let TokenKind::Ident(ref pat) = self.peek().clone() {
            if (pat == "Ok" || pat == "Some" || pat == "Err")
                && self.pos + 1 < self.tokens.len()
                && self.tokens[self.pos + 1].kind == TokenKind::LParen
            {
                let pattern = self.expect_ident()?;
                self.expect(&TokenKind::LParen)?;
                let var = self.expect_ident()?;
                self.expect(&TokenKind::RParen)?;
                self.expect(&TokenKind::Assign)?;
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::Else)?;
                self.expect(&TokenKind::LBrace)?;
                let else_body = self.parse_block_body()?;
                return Ok(Stmt::LetElse { pattern, var, expr, else_body });
            }
        }

        let name = self.expect_ident()?;

        let type_ann = if self.match_tok(&TokenKind::Colon) {
            Some(self.parse_type_annotation()?)
        } else {
            None
        };

        let value = if self.match_tok(&TokenKind::Assign) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        Ok(Stmt::Let {
            name,
            type_ann,
            value,
        })
    }

    fn parse_let_tuple_destructure(&mut self) -> Result<Stmt, String> {
        // let (a, b, c) = expr
        self.expect(&TokenKind::LParen)?;
        let mut names = Vec::new();
        while !self.check(&TokenKind::RParen) {
            names.push(self.expect_ident()?);
            if !self.match_tok(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Assign)?;
        let value = self.parse_expr()?;

        let tmp = format!("__destruct_{}", self.pos);
        let mut stmts = vec![Stmt::Let {
            name: tmp.clone(),
            type_ann: None,
            value: Some(value),
        }];
        for (i, n) in names.into_iter().enumerate() {
            stmts.push(Stmt::Let {
                name: n,
                type_ann: None,
                value: Some(Expr::Index {
                    object: Box::new(Expr::Ident(tmp.clone())),
                    index: Box::new(Expr::Int(i as i64)),
                }),
            });
        }
        Ok(Stmt::Multi(stmts))
    }

    fn parse_let_destructure(&mut self) -> Result<Stmt, String> {
        // let [a, [b, c], ...rest] = expr
        let tmp = format!("__destruct_{}", self.pos);
        let mut stmts = Vec::new();
        self.parse_list_destruct_body(&Expr::Ident(tmp.clone()), &mut stmts, &mut 0)?;
        self.expect(&TokenKind::Assign)?;
        let value = self.parse_expr()?;
        // Prepend the binding of tmp
        let mut all_stmts = vec![Stmt::Let { name: tmp, type_ann: None, value: Some(value) }];
        all_stmts.extend(stmts);
        Ok(Stmt::Multi(all_stmts))
    }

    fn parse_list_destruct_body(&mut self, base: &Expr, stmts: &mut Vec<Stmt>, counter: &mut usize) -> Result<(), String> {
        // Parses [pat1, pat2, ...rest] and pushes let-stmts into stmts
        self.expect(&TokenKind::LBracket)?;
        let mut idx: usize = 0;
        loop {
            if self.check(&TokenKind::RBracket) { break; }
            if self.check(&TokenKind::Ellipsis) {
                self.advance(); // skip ...
                let name = self.expect_ident()?;
                // rest = base.slice(idx)
                stmts.push(Stmt::Let {
                    name,
                    type_ann: None,
                    value: Some(Expr::MethodCall {
                        object: Box::new(base.clone()),
                        method: "slice".into(),
                        args: vec![CallArg { name: None, value: Expr::Int(idx as i64), is_spread: false }],
                        optional: false,
                    }),
                });
                // rest pattern must be last
                if !self.check(&TokenKind::RBracket) { self.match_tok(&TokenKind::Comma); }
                break;
            } else if self.check(&TokenKind::LBracket) {
                // Nested list pattern
                *counter += 1;
                let nested_tmp = format!("__nestd_{}", counter);
                stmts.push(Stmt::Let {
                    name: nested_tmp.clone(),
                    type_ann: None,
                    value: Some(Expr::Index {
                        object: Box::new(base.clone()),
                        index: Box::new(Expr::Int(idx as i64)),
                    }),
                });
                self.parse_list_destruct_body(&Expr::Ident(nested_tmp), stmts, counter)?;
                idx += 1;
            } else {
                let name = self.expect_ident()?;
                if name != "_" {
                    stmts.push(Stmt::Let {
                        name,
                        type_ann: None,
                        value: Some(Expr::Index {
                            object: Box::new(base.clone()),
                            index: Box::new(Expr::Int(idx as i64)),
                        }),
                    });
                }
                idx += 1;
            }
            if !self.match_tok(&TokenKind::Comma) { break; }
        }
        self.expect(&TokenKind::RBracket)?;
        Ok(())
    }

    fn parse_let_dict_destructure(&mut self) -> Result<Stmt, String> {
        // let {a, b} = expr           → extract keys "a","b" (missing key errors)
        // let {a, b ?? default} = expr → fall back to `default` when "b" is missing
        self.expect(&TokenKind::LBrace)?;
        let mut fields: Vec<(String, Option<Expr>)> = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let name = self.expect_ident()?;
            let default = if self.match_tok(&TokenKind::QuestionQuestion) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            fields.push((name, default));
            if !self.match_tok(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace)?;
        self.expect(&TokenKind::Assign)?;
        let value = self.parse_expr()?;

        let tmp = format!("__ddestr_{}", self.pos);
        let mut stmts = vec![Stmt::Let {
            name: tmp.clone(),
            type_ann: None,
            value: Some(value),
        }];
        for (n, default) in fields {
            let field_expr = match default {
                // `field ?? default` → tmp.get("field", default)
                Some(def) => Expr::MethodCall {
                    object: Box::new(Expr::Ident(tmp.clone())),
                    method: "get".to_string(),
                    args: vec![
                        CallArg { name: None, value: Expr::Str(n.clone()), is_spread: false },
                        CallArg { name: None, value: def, is_spread: false },
                    ],
                    optional: false,
                },
                None => Expr::Index {
                    object: Box::new(Expr::Ident(tmp.clone())),
                    index: Box::new(Expr::Str(n.clone())),
                },
            };
            stmts.push(Stmt::Let {
                name: n,
                type_ann: None,
                value: Some(field_expr),
            });
        }
        Ok(Stmt::Multi(stmts))
    }

    fn parse_const(&mut self) -> Result<Stmt, String> {
        let doc_comment = self.take_doc_comment();
        self.advance(); // skip 'const'
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Assign)?;
        let value = self.parse_expr()?;
        Ok(Stmt::Const {
            name,
            value,
            doc_comment,
        })
    }

    fn parse_func_decl(&mut self) -> Result<Stmt, String> {
        let doc_comment = self.take_doc_comment();
        let mut is_pure = false;
        let mut is_async = false;
        let mut is_generator = false;

        if self.match_tok(&TokenKind::Pure) {
            is_pure = true;
        }
        if self.match_tok(&TokenKind::Async) {
            is_async = true;
        }
        self.expect(&TokenKind::Func)?;

        // Check for generator: func*
        if self.match_tok(&TokenKind::Star) {
            is_generator = true;
        }

        let name = self.expect_ident()?;
        self.skip_generic_params_if_present()?;
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen)?;

        // Optional effects annotation: [effects: net, io] (can appear before or after return type)
        if self.check(&TokenKind::LBracket) {
            self.advance(); // skip [
            while !self.check(&TokenKind::RBracket) && !self.check(&TokenKind::Eof) {
                self.advance();
            }
            if self.check(&TokenKind::RBracket) {
                self.advance();
            }
        }

        // Optional return type annotation
        if self.match_tok(&TokenKind::Arrow) {
            let _ret_type = self.parse_type_annotation()?; // consume but don't enforce yet
        }

        // Effects annotation can also come after return type
        if self.check(&TokenKind::LBracket) {
            self.advance(); // skip [
            while !self.check(&TokenKind::RBracket) && !self.check(&TokenKind::Eof) {
                self.advance();
            }
            if self.check(&TokenKind::RBracket) {
                self.advance();
            }
        }

        self.skip_where_clause_if_present();

        // Allow functions without body (abstract trait methods)
        let body = if self.check(&TokenKind::LBrace) {
            self.expect(&TokenKind::LBrace)?;
            self.parse_block_body()?
        } else {
            vec![] // abstract method — no body
        };

        // A function whose body yields is a generator even without the `func*`
        // star (nested function bodies don't count — they yield for themselves).
        if !is_generator && Self::stmts_contain_yield(&body) {
            is_generator = true;
        }

        Ok(Stmt::FuncDecl {
            name,
            params,
            body,
            is_pure,
            is_async,
            is_generator,
            decorators: Vec::new(),
            doc_comment,
        })
    }

    /// Recursively scan statements (but not nested function declarations)
    /// for a `yield`, so plain `func` generators work like `func*`.
    fn stmts_contain_yield(stmts: &[Stmt]) -> bool {
        stmts.iter().any(|s| match s {
            Stmt::Yield(_) => true,
            Stmt::If { body, else_ifs, else_body, .. } => {
                Self::stmts_contain_yield(body)
                    || else_ifs.iter().any(|(_, b)| Self::stmts_contain_yield(b))
                    || else_body.as_deref().map(Self::stmts_contain_yield).unwrap_or(false)
            }
            Stmt::While { body, .. } => Self::stmts_contain_yield(body),
            Stmt::IfLet { body, else_body, .. } => {
                Self::stmts_contain_yield(body)
                    || else_body.as_deref().map(Self::stmts_contain_yield).unwrap_or(false)
            }
            Stmt::WhileLet { body, .. } => Self::stmts_contain_yield(body),
            Stmt::LetElse { else_body, .. } => Self::stmts_contain_yield(else_body),
            Stmt::ForIn { body, .. } => Self::stmts_contain_yield(body),
            Stmt::ForInDestructure { body, .. } => Self::stmts_contain_yield(body),
            Stmt::ForClassic { body, .. } => Self::stmts_contain_yield(body),
            Stmt::Match { arms, .. } => {
                arms.iter().any(|a| Self::stmts_contain_yield(&a.body))
            }
            Stmt::TryCatch { body, catch_body, catch_clauses, finally_body, .. } => {
                Self::stmts_contain_yield(body)
                    || catch_body.as_deref().map(Self::stmts_contain_yield).unwrap_or(false)
                    || catch_clauses.iter().any(|(_, _, b)| Self::stmts_contain_yield(b))
                    || finally_body.as_deref().map(Self::stmts_contain_yield).unwrap_or(false)
            }
            Stmt::Defer(body) => Self::stmts_contain_yield(body),
            _ => false,
        })
    }

    fn parse_decorated_func(&mut self) -> Result<Stmt, String> {
        let mut decorators = Vec::new();
        while self.match_tok(&TokenKind::At) {
            let name = self.expect_decorator_name()?;

            // ── Source directives ──
            match name.as_str() {
                "replace" | "insert" | "borrow_check" | "cfg" | "suppress" | "suppress_start" | "suppress_end"
                | "wasm_export" | "wasm_import" => {
                    let mut args = Vec::new();
                    // Consume rest of line as arguments (simplified)
                    while !self.check(&TokenKind::Newline) && !self.check(&TokenKind::Eof)
                          && !self.check(&TokenKind::At) && !self.check(&TokenKind::Func)
                          && !self.check(&TokenKind::Class) && !self.check(&TokenKind::Struct)
                          && !self.check(&TokenKind::LBrace) {
                        match self.peek().clone() {
                            TokenKind::Str(s) => { args.push(s.clone()); self.advance(); }
                            TokenKind::Ident(s) => { args.push(s.clone()); self.advance(); }
                            _ => { self.advance(); }
                        }
                    }
                    return Ok(Stmt::SourceDirective { kind: name, args });
                }
                // ── Embedded language blocks: @py { code }, @js { code }, etc ──
                "py" | "js" | "lua" | "go" | "rust" | "bash" | "sql" | "wasm" => {
                    let label = if !self.check(&TokenKind::LBrace) && !self.is_at_end() {
                        if let TokenKind::Ident(l) = self.peek().clone() {
                            self.advance();
                            Some(l)
                        } else { None }
                    } else { None };
                    self.expect(&TokenKind::LBrace)?;
                    // Collect everything until matching } as raw text
                    let mut code = String::new();
                    let mut depth = 1;
                    while depth > 0 && !self.is_at_end() {
                        match self.peek().clone() {
                            TokenKind::LBrace => { depth += 1; code.push('{'); self.advance(); }
                            TokenKind::RBrace => {
                                depth -= 1;
                                if depth > 0 { code.push('}'); }
                                self.advance();
                            }
                            _ => {
                                code.push_str(&format!("{:?} ", self.peek()));
                                self.advance();
                            }
                        }
                    }
                    return Ok(Stmt::EmbeddedLangBlock { lang: name, label, code });
                }
                "inline" => {
                    // @inline struct
                    if self.check(&TokenKind::Struct) {
                        let mut stmt = self.parse_struct_decl()?;
                        if let Stmt::StructDecl { name, fields, doc_comment, .. } = stmt {
                            return Ok(Stmt::InlineStructDecl { name, fields, doc_comment });
                        }
                        return Ok(stmt);
                    }
                    // Otherwise treat as regular decorator
                    decorators.push(Expr::Ident(name));
                    continue;
                }
                _ => {}
            }

            if self.match_tok(&TokenKind::LParen) {
                // @decorator(args) — supports positional, named (key: value),
                // and spread arguments, e.g. @data(exclude: ["cache"]).
                let call_args = self.parse_call_args()?;
                self.expect(&TokenKind::RParen)?;
                decorators.push(Expr::Call {
                    callee: Box::new(Expr::Ident(name)),
                    args: call_args,
                });
            } else {
                decorators.push(Expr::Ident(name));
            }
        }
        // Now parse the func, class, struct, or enum decl
        if self.check(&TokenKind::Class) {
            let mut stmt = self.parse_class_decl()?;
            if let Stmt::ClassDecl { decorators: ref mut decs, .. } = stmt {
                let dec_names: Vec<String> = decorators.iter().map(|d| match d {
                    Expr::Ident(n) => n.clone(),
                    Expr::Call { callee, args } => {
                        if let Expr::Ident(name) = callee.as_ref() {
                            let arg_names: Vec<String> = args.iter().map(|a| match &a.value {
                                Expr::Ident(n) => n.clone(),
                                _ => String::new(),
                            }).collect();
                            format!("{}({})", name, arg_names.join(", "))
                        } else {
                            String::new()
                        }
                    }
                    _ => String::new(),
                }).filter(|s| !s.is_empty()).collect();
                *decs = dec_names;
            }
            return Ok(stmt);
        }
        if self.check(&TokenKind::Struct) {
            // @derive(...) struct or other decorators on struct
            let mut stmt = self.parse_struct_decl()?;
            if let Stmt::StructDecl { decorators: ref mut decs, .. } = stmt {
                let dec_names: Vec<String> = decorators.iter().map(|d| match d {
                    Expr::Ident(n) => n.clone(),
                    Expr::Call { callee, args } => {
                        if let Expr::Ident(name) = callee.as_ref() {
                            let arg_names: Vec<String> = args.iter().map(|a| match &a.value {
                                Expr::Ident(n) => n.clone(),
                                _ => String::new(),
                            }).collect();
                            format!("{}({})", name, arg_names.join(", "))
                        } else {
                            String::new()
                        }
                    }
                    _ => String::new(),
                }).filter(|s| !s.is_empty()).collect();
                *decs = dec_names;
            }
            return Ok(stmt);
        }
        if self.check(&TokenKind::Enum) {
            let stmt = self.parse_enum_decl()?;
            return Ok(stmt);
        }
        if self.check(&TokenKind::Newtype) {
            let stmt = self.parse_newtype_decl()?;
            return Ok(stmt);
        }
        let mut stmt = self.parse_func_decl()?;
        if let Stmt::FuncDecl { decorators: ref mut decs, .. } = stmt {
            *decs = decorators;
        }
        Ok(stmt)
    }

    fn parse_params(&mut self) -> Result<Vec<Param>, String> {
        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) {
            let is_variadic = self.match_tok(&TokenKind::Ellipsis);
            // Allow `self` as a parameter name
            let name = if self.check(&TokenKind::Self_) {
                self.advance();
                "self".to_string()
            } else {
                self.expect_ident()?
            };

            let type_ann = if self.match_tok(&TokenKind::Colon) {
                Some(self.parse_type_annotation()?)
            } else {
                None
            };

            let default = if self.match_tok(&TokenKind::Assign) {
                Some(self.parse_expr()?)
            } else {
                None
            };

            params.push(Param {
                name,
                type_ann,
                default,
                is_variadic,
            });

            if !self.match_tok(&TokenKind::Comma) {
                break;
            }
        }
        Ok(params)
    }

    fn parse_block_body(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            stmts.push(self.parse_stmt()?);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(stmts)
    }

    fn parse_return(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'return'
        // return can be bare or with a value
        if self.check(&TokenKind::RBrace) || self.is_at_end() {
            Ok(Stmt::Return(None))
        } else {
            Ok(Stmt::Return(Some(self.parse_expr()?)))
        }
    }

    fn parse_if(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'if'

        // Check for `if let Pattern(var) = expr { body }`
        if self.check(&TokenKind::Let) {
            return self.parse_if_let();
        }

        // Condition parens are optional: `if (x) { }` and `if x { }` both work.
        // parse_cond_expr suppresses struct literals so the bare `{` opens the body.
        let condition = self.parse_cond_expr()?;
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;

        let mut else_ifs = Vec::new();
        let mut else_body = None;

        while self.match_tok(&TokenKind::Else) {
            if self.match_tok(&TokenKind::If) {
                let cond = self.parse_cond_expr()?;
                self.expect(&TokenKind::LBrace)?;
                let b = self.parse_block_body()?;
                else_ifs.push((cond, b));
            } else {
                self.expect(&TokenKind::LBrace)?;
                else_body = Some(self.parse_block_body()?);
                break;
            }
        }
        // Also handle `elif` as standalone keyword
        while self.match_tok(&TokenKind::Elif) {
            let cond = self.parse_cond_expr()?;
            self.expect(&TokenKind::LBrace)?;
            let b = self.parse_block_body()?;
            else_ifs.push((cond, b));
        }
        if else_body.is_none() && self.match_tok(&TokenKind::Else) {
            self.expect(&TokenKind::LBrace)?;
            else_body = Some(self.parse_block_body()?);
        }

        Ok(Stmt::If {
            condition,
            body,
            else_ifs,
            else_body,
        })
    }

    fn parse_while(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'while'

        // Check for `while let Pattern(var) = expr { body }`
        if self.check(&TokenKind::Let) {
            return self.parse_while_let();
        }

        // Condition parens are optional, as with `if`.
        let condition = self.parse_cond_expr()?;
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;
        Ok(Stmt::While { condition, body })
    }

    fn parse_if_let(&mut self) -> Result<Stmt, String> {
        // Already consumed 'if', now at 'let'. Supports chaining with `&&`:
        //   if let Some(x) = a && let Some(y) = b && cond { body } else { alt }
        // Chained conditions all must hold; desugars to nested if/if-let, each
        // level sharing the same else branch (only one branch ever runs).
        self.advance(); // skip 'let'

        enum Elem {
            Bind(String, String, Expr),
            Cond(Expr),
        }

        let parse_binding = |p: &mut Self| -> Result<(String, String, Expr), String> {
            let pattern = p.expect_ident()?; // "Some", "Ok", "Err", ...
            p.expect(&TokenKind::LParen)?;
            let var = p.expect_ident()?;
            p.expect(&TokenKind::RParen)?;
            p.expect(&TokenKind::Assign)?;
            let expr = p.parse_cond_operand()?;
            Ok((pattern, var, expr))
        };

        let (pattern, var, expr) = parse_binding(self)?;
        let mut elems = vec![Elem::Bind(pattern, var, expr)];
        while self.match_tok(&TokenKind::And) {
            if self.match_tok(&TokenKind::Let) {
                let (p, v, e) = parse_binding(self)?;
                elems.push(Elem::Bind(p, v, e));
            } else {
                elems.push(Elem::Cond(self.parse_cond_operand()?));
            }
        }

        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;
        let else_body = if self.match_tok(&TokenKind::Else) {
            self.expect(&TokenKind::LBrace)?;
            Some(self.parse_block_body()?)
        } else {
            None
        };

        // Build from the innermost element outward; each level reuses else_body.
        let mut current = body;
        for elem in elems.into_iter().rev() {
            let stmt = match elem {
                Elem::Bind(pattern, var, expr) => Stmt::IfLet {
                    pattern,
                    var,
                    expr,
                    body: current,
                    else_body: else_body.clone(),
                },
                Elem::Cond(condition) => Stmt::If {
                    condition,
                    body: current,
                    else_ifs: Vec::new(),
                    else_body: else_body.clone(),
                },
            };
            current = vec![stmt];
        }
        Ok(current.into_iter().next().unwrap())
    }

    fn parse_while_let(&mut self) -> Result<Stmt, String> {
        // Already consumed 'while', now at 'let'
        self.advance(); // skip 'let'
        let pattern = self.expect_ident()?; // "Some" or "Ok"
        self.expect(&TokenKind::LParen)?;
        let var = self.expect_ident()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Assign)?;
        let expr = self.parse_cond_expr()?;
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;
        Ok(Stmt::WhileLet { pattern, var, expr, body })
    }

    fn parse_for(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'for'
        // `for await (x in asyncIterable)` — async iteration. Async runs
        // synchronously here, so it behaves like an ordinary for-in.
        self.match_tok(&TokenKind::Await);

        // Head parens are optional for for-in: `for x in it { }`,
        // `for a, b in it { }`, and `for [a, b] in it { }` all work.
        // C-style `for (init; cond; step)` still requires parens.
        if !self.check(&TokenKind::LParen) {
            if self.match_tok(&TokenKind::LBracket) {
                let mut vars = Vec::new();
                loop {
                    vars.push(self.expect_ident()?);
                    if !self.match_tok(&TokenKind::Comma) { break; }
                }
                self.expect(&TokenKind::RBracket)?;
                self.expect(&TokenKind::In)?;
                let iter = self.parse_cond_expr()?;
                self.expect(&TokenKind::LBrace)?;
                let body = self.parse_block_body()?;
                return Ok(Stmt::ForInDestructure { vars, iter, body });
            }
            let mut vars = vec![self.expect_ident()?];
            while self.match_tok(&TokenKind::Comma) {
                vars.push(self.expect_ident()?);
            }
            self.expect(&TokenKind::In)?;
            let iter = self.parse_cond_expr()?;
            self.expect(&TokenKind::LBrace)?;
            let body = self.parse_block_body()?;
            return Ok(if vars.len() == 1 {
                Stmt::ForIn { var: vars.remove(0), iter, body }
            } else {
                Stmt::ForInDestructure { vars, iter, body }
            });
        }

        self.expect(&TokenKind::LParen)?;

        // Check for destructuring for-in: for ([a, b] in expr) or for ((a, b) in expr)
        if self.check(&TokenKind::LBracket) || 
           (self.check(&TokenKind::LParen) && {
               // Look ahead to distinguish tuple destructure from grouped expr
               // Simple heuristic: (ident, ident) in ...
               let mut j = self.pos + 1;
               let mut looks_like_destructure = false;
               while j < self.tokens.len() {
                   match &self.tokens[j].kind {
                       TokenKind::RParen => { 
                           looks_like_destructure = j + 1 < self.tokens.len() && self.tokens[j + 1].kind == TokenKind::In;
                           break; 
                       }
                       TokenKind::Ident(_) | TokenKind::Comma => { j += 1; }
                       _ => break,
                   }
               }
               looks_like_destructure
           })
        {
            let is_bracket = self.check(&TokenKind::LBracket);
            self.advance(); // skip [ or (
            let mut vars = Vec::new();
            loop {
                vars.push(self.expect_ident()?);
                if !self.match_tok(&TokenKind::Comma) { break; }
            }
            if is_bracket {
                self.expect(&TokenKind::RBracket)?;
            } else {
                self.expect(&TokenKind::RParen)?;
            }
            self.expect(&TokenKind::In)?;
            let iter = self.parse_expr()?;
            self.expect(&TokenKind::RParen)?;
            self.expect(&TokenKind::LBrace)?;
            let body = self.parse_block_body()?;
            return Ok(Stmt::ForInDestructure { vars, iter, body });
        }

        // Single-paren tuple destructure: `for (i, part) in iter { }` — the
        // `(` we already consumed wraps the variables, and `in iter` follows the
        // matching `)`. Detect `ident (, ident)* ) in`.
        {
            let mut j = self.pos;
            let mut saw_comma = false;
            let mut ok = false;
            while j < self.tokens.len() {
                match &self.tokens[j].kind {
                    TokenKind::Ident(_) => j += 1,
                    TokenKind::Comma => { saw_comma = true; j += 1; }
                    TokenKind::RParen => {
                        ok = saw_comma
                            && self.tokens.get(j + 1).map(|t| &t.kind) == Some(&TokenKind::In);
                        break;
                    }
                    _ => break,
                }
            }
            if ok {
                let mut vars = Vec::new();
                loop {
                    vars.push(self.expect_ident()?);
                    if !self.match_tok(&TokenKind::Comma) { break; }
                }
                self.expect(&TokenKind::RParen)?;
                self.expect(&TokenKind::In)?;
                let iter = self.parse_cond_expr()?;
                self.expect(&TokenKind::LBrace)?;
                let body = self.parse_block_body()?;
                return Ok(Stmt::ForInDestructure { vars, iter, body });
            }
        }

        // Distinguish for-in from C-style for
        if let TokenKind::Ident(name) = self.peek().clone() {
            if self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&TokenKind::In) {
                self.advance(); // skip ident
                self.advance(); // skip 'in'
                let iter = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                self.expect(&TokenKind::LBrace)?;
                let body = self.parse_block_body()?;
                return Ok(Stmt::ForIn {
                    var: name,
                    iter,
                    body,
                });
            }
        }

        // C-style for
        let init = if self.check(&TokenKind::Let) {
            Some(Box::new(self.parse_let()?))
        } else if !self.check(&TokenKind::Semicolon) {
            Some(Box::new(self.parse_expr_stmt()?))
        } else {
            None
        };
        self.expect(&TokenKind::Semicolon)?;

        let condition = if !self.check(&TokenKind::Semicolon) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(&TokenKind::Semicolon)?;

        let update = if !self.check(&TokenKind::RParen) {
            Some(Box::new(self.parse_expr_stmt()?))
        } else {
            None
        };
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;

        Ok(Stmt::ForClassic {
            init,
            condition,
            update,
            body,
        })
    }

    fn parse_match(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'match'
        // Subject parens are optional: `match (x) { }` and `match x { }` are both
        // valid. parse_cond_expr suppresses struct literals so a bare `{` opens the
        // arm block; explicit `( )` are handled as a normal grouped expression.
        let subject = self.parse_cond_expr()?;
        self.expect(&TokenKind::LBrace)?;

        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            if self.match_tok(&TokenKind::Case) {
                self.expect(&TokenKind::LParen)?;
                let pattern = self.parse_pattern()?;
                self.expect(&TokenKind::RParen)?;
                let guard = if self.match_tok(&TokenKind::If) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                self.expect(&TokenKind::LBrace)?;
                let body = self.parse_block_body()?;

                arms.push(MatchArm {
                    pattern,
                    guard,
                    body,
                });
            } else if self.match_tok(&TokenKind::Default) {
                self.expect(&TokenKind::LBrace)?;
                let body = self.parse_block_body()?;
                arms.push(MatchArm {
                    pattern: Pattern::Default,
                    guard: None,
                    body,
                });
            } else {
                return Err(format!(
                    "[line {}] Error: Expected 'case' or 'default' in match",
                    self.current_line()
                ));
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Stmt::Match { subject, arms })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, String> {
        let pat = self.parse_single_pattern()?;

        // Check for or pattern: pat | pat
        if self.check(&TokenKind::BitOr) {
            let mut pats = vec![pat];
            while self.match_tok(&TokenKind::BitOr) {
                pats.push(self.parse_single_pattern()?);
            }
            return Ok(Pattern::Or(pats));
        }

        Ok(pat)
    }

    fn parse_single_pattern(&mut self) -> Result<Pattern, String> {
        match self.peek().clone() {
            TokenKind::Int(n) => {
                self.advance();
                // Check for range pattern: 1..10 or 1..=10
                if self.check(&TokenKind::DotDot) {
                    self.advance();
                    let end = match self.peek().clone() {
                        TokenKind::Int(e) => { self.advance(); Expr::Int(e) }
                        _ => return Err(format!("[line {}] Expected integer in range pattern", self.current_line())),
                    };
                    return Ok(Pattern::Range { start: Box::new(Expr::Int(n)), end: Box::new(end), inclusive: false });
                }
                if self.check(&TokenKind::DotDotEq) {
                    self.advance();
                    let end = match self.peek().clone() {
                        TokenKind::Int(e) => { self.advance(); Expr::Int(e) }
                        _ => return Err(format!("[line {}] Expected integer in range pattern", self.current_line())),
                    };
                    return Ok(Pattern::Range { start: Box::new(Expr::Int(n)), end: Box::new(end), inclusive: true });
                }
                Ok(Pattern::Literal(Expr::Int(n)))
            }
            TokenKind::Float(f) => {
                self.advance();
                Ok(Pattern::Literal(Expr::Float(f)))
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Pattern::Literal(Expr::Str(s)))
            }
            TokenKind::Bool(b) => {
                self.advance();
                Ok(Pattern::Literal(Expr::Bool(b)))
            }
            TokenKind::Null => {
                self.advance();
                Ok(Pattern::Literal(Expr::Null))
            }
            TokenKind::Underscore => {
                self.advance();
                Ok(Pattern::Wildcard)
            }
            TokenKind::LBracket => {
                self.advance();
                let mut pats = Vec::new();
                while !self.check(&TokenKind::RBracket) {
                    // Check for rest pattern: ...name
                    if self.check(&TokenKind::Ellipsis) {
                        self.advance(); // skip ...
                        let rest_name = self.expect_ident()?;
                        pats.push(Pattern::Rest(rest_name));
                        self.match_tok(&TokenKind::Comma);
                        break; // rest must be last
                    }
                    pats.push(self.parse_pattern()?);
                    if !self.match_tok(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RBracket)?;
                Ok(Pattern::List(pats))
            }
            TokenKind::LBrace => {
                // Dict/struct pattern without a type name: { key: pat, shorthand }.
                // Keys may be idents, string literals, or the `type` keyword
                // (common in event dicts: case ({type: "click", x, y})).
                self.advance();
                let mut fields = Vec::new();
                while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
                    let field_name = match self.peek().clone() {
                        TokenKind::Ident(n) => { self.advance(); n }
                        TokenKind::Str(s) => { self.advance(); s }
                        TokenKind::Type => { self.advance(); "type".to_string() }
                        other => {
                            return Err(format!(
                                "[line {}] Expected field name in dict pattern, found {:?}",
                                self.current_line(), other
                            ))
                        }
                    };
                    let field_pat = if self.match_tok(&TokenKind::Colon) {
                        Some(self.parse_pattern()?)
                    } else {
                        None // shorthand: binds the value at this key
                    };
                    fields.push((field_name, field_pat));
                    if !self.match_tok(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RBrace)?;
                Ok(Pattern::StructPat { type_name: None, fields })
            }
            TokenKind::LParen => {
                self.advance();
                // Typed binding pattern: (name: type) — binds `name` when the
                // value has the given type.
                if let TokenKind::Ident(name) = self.peek().clone() {
                    if self.tokens.get(self.pos + 1).map(|t| t.kind == TokenKind::Colon).unwrap_or(false) {
                        self.advance(); // name
                        self.advance(); // :
                        let type_name = self.parse_type_annotation()?;
                        self.expect(&TokenKind::RParen)?;
                        return Ok(Pattern::TypedBind { name, type_name });
                    }
                }
                // Tuple pattern: (a, b, ...)
                let mut pats = Vec::new();
                while !self.check(&TokenKind::RParen) {
                    pats.push(self.parse_pattern()?);
                    if !self.match_tok(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen)?;
                Ok(Pattern::Tuple(pats))
            }
            TokenKind::Ident(name) => {
                self.advance();
                // Handle Ok/Err/Some/None as special patterns
                match name.as_str() {
                    "None" => Ok(Pattern::None),
                    "Ok" if self.check(&TokenKind::LParen) => {
                        self.expect(&TokenKind::LParen)?;
                        let inner = self.parse_pattern()?;
                        self.expect(&TokenKind::RParen)?;
                        Ok(Pattern::Ok(Box::new(inner)))
                    }
                    "Err" if self.check(&TokenKind::LParen) => {
                        self.expect(&TokenKind::LParen)?;
                        let inner = self.parse_pattern()?;
                        self.expect(&TokenKind::RParen)?;
                        Ok(Pattern::Err(Box::new(inner)))
                    }
                    "Some" if self.check(&TokenKind::LParen) => {
                        self.expect(&TokenKind::LParen)?;
                        let inner = self.parse_pattern()?;
                        self.expect(&TokenKind::RParen)?;
                        Ok(Pattern::Some(Box::new(inner)))
                    }
                    // Type patterns: bare type-name keywords
                    "int" | "float" | "str" | "bool" | "list" | "dict" | "tuple" | "set"
                    | "bytes" | "null" | "generator" | "pointer" => {
                        // Could be `case (int)` — type pattern
                        // But only if there's no ( following (which would make it a call/destructure)
                        if !self.check(&TokenKind::LParen) && !self.check(&TokenKind::Dot) {
                            Ok(Pattern::TypePat(name))
                        } else {
                            Ok(Pattern::Ident(name))
                        }
                    }
                    _ => {
                        // Check for struct field pattern: Name { x, y } or Name { x: pat, y: pat }
                        if self.check(&TokenKind::LBrace) {
                            self.advance(); // skip {
                            let mut fields = Vec::new();
                            while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
                                let field_name = self.expect_ident()?;
                                let field_pat = if self.match_tok(&TokenKind::Colon) {
                                    Some(self.parse_pattern()?)
                                } else {
                                    None // shorthand: field_name binds the field value
                                };
                                fields.push((field_name, field_pat));
                                if !self.match_tok(&TokenKind::Comma) {
                                    break;
                                }
                            }
                            self.expect(&TokenKind::RBrace)?;
                            return Ok(Pattern::StructPat { type_name: Some(name), fields });
                        }
                        // Check for destructure: Name.Variant(fields) or Name(fields)
                        if self.check(&TokenKind::Dot) || self.check(&TokenKind::LParen) {
                            let mut path = vec![name];
                            while self.match_tok(&TokenKind::Dot) {
                                path.push(self.expect_ident()?);
                            }
                            if self.match_tok(&TokenKind::LParen) {
                                let mut fields = Vec::new();
                                while !self.check(&TokenKind::RParen) {
                                    fields.push(self.parse_pattern()?);
                                    if !self.match_tok(&TokenKind::Comma) {
                                        break;
                                    }
                                }
                                self.expect(&TokenKind::RParen)?;
                                Ok(Pattern::Destructure { path, fields })
                            } else {
                                Ok(Pattern::Destructure {
                                    path,
                                    fields: vec![],
                                })
                            }
                        } else {
                            Ok(Pattern::Ident(name))
                        }
                    }
                }
            }
            _ => Err(format!(
                "[line {}] Error: Expected pattern, found {:?}",
                self.current_line(),
                self.peek()
            )),
        }
    }

    fn parse_throw(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'throw'
        let expr = self.parse_expr()?;
        Ok(Stmt::Throw(expr))
    }

    fn parse_try_catch(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'try'
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;

        let mut catch_clauses: Vec<(Option<String>, Option<String>, Vec<Stmt>)> = Vec::new();
        let mut finally_body = None;

        // Parse one or more catch clauses
        while self.check(&TokenKind::Catch) {
            self.advance(); // skip 'catch'
            let mut catch_var = None;
            let mut catch_type = None;
            if self.match_tok(&TokenKind::LParen) {
                if !self.check(&TokenKind::RParen) {
                    let var_name = self.expect_ident()?;
                    catch_var = Some(var_name);
                    // Check for typed catch: catch (e: TypeName)
                    if self.match_tok(&TokenKind::Colon) {
                        catch_type = Some(self.expect_ident()?);
                    }
                }
                self.expect(&TokenKind::RParen)?;
            }
            self.expect(&TokenKind::LBrace)?;
            let cb = self.parse_block_body()?;
            catch_clauses.push((catch_var, catch_type, cb));
        }

        if self.match_tok(&TokenKind::Finally) {
            self.expect(&TokenKind::LBrace)?;
            finally_body = Some(self.parse_block_body()?);
        }

        // For backward compatibility, extract first clause into legacy fields
        let (catch_var, catch_type, catch_body) = if let Some((cv, ct, cb)) = catch_clauses.first().cloned() {
            (cv, ct, Some(cb))
        } else {
            (None, None, None)
        };

        Ok(Stmt::TryCatch {
            body,
            catch_var,
            catch_body,
            catch_type,
            catch_clauses,
            finally_body,
        })
    }

    fn parse_defer(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'defer'
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;
        Ok(Stmt::Defer(body))
    }

    fn parse_class_decl(&mut self) -> Result<Stmt, String> {
        let doc_comment = self.take_doc_comment();
        let is_sealed = self.match_tok(&TokenKind::Sealed);
        self.expect(&TokenKind::Class)?;
        let name = self.expect_ident()?;
        self.skip_generic_params_if_present()?;

        let parent = if self.match_tok(&TokenKind::Extends) {
            Some(self.expect_ident()?)
        } else {
            None
        };

        self.expect(&TokenKind::LBrace)?;
        let mut body = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            body.push(self.parse_class_member()?);
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(Stmt::ClassDecl {
            name,
            parent,
            body,
            decorators: Vec::new(),
            is_sealed,
            doc_comment,
        })
    }

    fn parse_class_member(&mut self) -> Result<Stmt, String> {
        self.consume_doc_comments();
        let doc_comment = self.take_doc_comment();
        let is_lazy = self.match_tok(&TokenKind::Lazy);

        let accessor = match self.peek().clone() {
            TokenKind::Ident(name) if name == "get" || name == "set" => Some(name),
            _ => None,
        };

        if let Some(accessor) = accessor {
            self.advance();
            let prop_name = self.expect_ident()?;
            let params = if accessor == "set" {
                self.expect(&TokenKind::LParen)?;
                let params = self.parse_params()?;
                self.expect(&TokenKind::RParen)?;
                params
            } else {
                Vec::new()
            };

            if self.match_tok(&TokenKind::Arrow) {
                let _ret_type = self.expect_ident()?;
            }

            self.expect(&TokenKind::LBrace)?;
            let body = self.parse_block_body()?;
            let mut decorators = Vec::new();
            if is_lazy {
                decorators.push(Expr::Ident("lazy".to_string()));
            }

            return Ok(Stmt::FuncDecl {
                name: format!("{}_{}", accessor, prop_name),
                params,
                body,
                is_pure: false,
                is_async: false,
                is_generator: false,
                decorators,
                doc_comment,
            });
        }

        if is_lazy {
            return Err(format!(
                "[line {}] Error: 'lazy' is only supported before a computed property getter",
                self.current_line()
            ));
        }

        self.pending_doc_comment = doc_comment;
        self.parse_stmt()
    }

    fn parse_struct_decl(&mut self) -> Result<Stmt, String> {
        let doc_comment = self.take_doc_comment();
        self.advance(); // skip 'struct'
        let name = self.expect_ident()?;
        self.skip_generic_params_if_present()?;
        self.expect(&TokenKind::LBrace)?;

        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            let fname = self.expect_ident()?;
            let type_ann = if self.match_tok(&TokenKind::Colon) {
                Some(self.parse_type_annotation()?)
            } else {
                None
            };
            let default = if self.match_tok(&TokenKind::Assign) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            fields.push(StructField {
                name: fname,
                type_ann,
                default,
            });
            self.match_tok(&TokenKind::Comma);
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(Stmt::StructDecl {
            name,
            fields,
            decorators: Vec::new(),
            doc_comment,
        })
    }

    fn parse_enum_decl(&mut self) -> Result<Stmt, String> {
        let doc_comment = self.take_doc_comment();
        self.advance(); // skip 'enum'
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;

        let mut variants = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            let vname = self.expect_ident()?;
            let mut fields = Vec::new();
            if self.match_tok(&TokenKind::LParen) {
                while !self.check(&TokenKind::RParen) {
                    fields.push(self.expect_ident()?);
                    if !self.match_tok(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen)?;
            }
            variants.push(EnumVariant {
                name: vname,
                fields,
            });
            self.match_tok(&TokenKind::Comma);
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(Stmt::EnumDecl {
            name,
            variants,
            doc_comment,
        })
    }

    fn parse_trait_decl(&mut self) -> Result<Stmt, String> {
        let doc_comment = self.take_doc_comment();
        self.advance(); // skip 'trait'
        let name = self.expect_ident()?;
        self.skip_generic_params_if_present()?;

        let mut supertraits = Vec::new();
        if self.match_tok(&TokenKind::Colon) {
            supertraits.push(self.parse_type_annotation()?);
            while self.match_tok(&TokenKind::Plus) {
                supertraits.push(self.parse_type_annotation()?);
            }
        }

        self.expect(&TokenKind::LBrace)?;
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            methods.push(self.parse_stmt()?);
        }
        self.expect(&TokenKind::RBrace)?;

        Ok(Stmt::TraitDecl {
            name,
            supertraits,
            methods,
            doc_comment,
        })
    }

    fn parse_impl_block(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'impl'
        self.skip_generic_params_if_present()?;
        let first = self.parse_type_annotation()?;

        if self.match_tok(&TokenKind::For) {
            // impl Trait for Type
            let target = self.parse_type_annotation()?;
            self.expect(&TokenKind::LBrace)?;
            let mut methods = Vec::new();
            while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
                methods.push(self.parse_stmt()?);
            }
            self.expect(&TokenKind::RBrace)?;
            Ok(Stmt::ImplBlock {
                trait_name: Some(first),
                target,
                methods,
            })
        } else {
            // impl Type { ... }
            self.expect(&TokenKind::LBrace)?;
            let mut methods = Vec::new();
            while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
                methods.push(self.parse_stmt()?);
            }
            self.expect(&TokenKind::RBrace)?;
            Ok(Stmt::ImplBlock {
                trait_name: None,
                target: first,
                methods,
            })
        }
    }

    fn parse_extend_block(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'extend'
        self.skip_generic_params_if_present()?;
        let target = self.parse_type_annotation()?;
        // Optional 'where' clause
        self.skip_where_clause_if_present();
        self.expect(&TokenKind::LBrace)?;
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            methods.push(self.parse_stmt()?);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Stmt::ImplBlock {
            trait_name: None,
            target,
            methods,
        })
    }

    fn parse_import(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'import'

        // import { a, b } from "mod"  OR  import { std.io, std.math } (grouped modules)
        if self.check(&TokenKind::LBrace) {
            self.advance();
            let mut names = Vec::new();
            while !self.check(&TokenKind::RBrace) {
                // A name may itself be a dotted module path (std.io) when this is
                // a grouped module import rather than a named-symbol import.
                let mut name = self.expect_ident()?;
                while self.match_tok(&TokenKind::Dot) {
                    name.push('.');
                    name.push_str(&self.expect_ident()?);
                }
                names.push(name);
                if !self.match_tok(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace)?;
            // `import { std.io, std.math }` (grouped modules) has no `from` clause —
            // desugar to one plain module import per entry so each resolves normally.
            if !self.match_tok(&TokenKind::From) {
                let imports: Vec<Stmt> = names
                    .into_iter()
                    .map(|p| Stmt::Import { path: p, names: None, alias: None })
                    .collect();
                return Ok(Stmt::Multi(imports));
            }
            let path = self.expect_string()?;
            let alias = if self.match_tok(&TokenKind::As) {
                Some(self.expect_ident()?)
            } else {
                None
            };
            return Ok(Stmt::Import {
                path,
                names: Some(names),
                alias,
            });
        }

        // import "path" or import "path" as alias
        // OR path-based import: import std.math / import crate::utils::Config /
        // import super::shared::helpers / import a.b.* (glob)
        let path = if let TokenKind::Str(_) = self.peek() {
            self.expect_string()?
        } else {
            self.parse_import_path()?
        };
        let alias = if self.match_tok(&TokenKind::As) {
            if self.check(&TokenKind::Underscore) {
                self.advance();
                Some("_".to_string())
            } else {
                Some(self.expect_ident()?)
            }
        } else {
            None
        };

        Ok(Stmt::Import {
            path,
            names: None,
            alias,
        })
    }

    /// Parse an unquoted module path such as `std.math`, `crate::utils::Config`,
    /// `super::shared::helpers`, or a glob `a.b.*`. Components are joined with the
    /// separators as written (`.` or `::`).
    fn parse_import_path(&mut self) -> Result<String, String> {
        let mut path = String::new();
        loop {
            match self.peek().clone() {
                TokenKind::Ident(s) => { self.advance(); path.push_str(&s); }
                TokenKind::Super => { self.advance(); path.push_str("super"); }
                TokenKind::Self_ => { self.advance(); path.push_str("self"); }
                TokenKind::Star => { self.advance(); path.push('*'); }
                other => {
                    if let Some(s) = Self::soft_keyword_text(&other) {
                        self.advance();
                        path.push_str(s);
                    } else {
                        break;
                    }
                }
            }
            if self.check(&TokenKind::Dot) {
                self.advance();
                path.push('.');
            } else if self.check(&TokenKind::ColonColon) {
                self.advance();
                path.push_str("::");
            } else {
                break;
            }
        }
        if path.is_empty() {
            return Err(format!(
                "[line {}] Error: Expected module path or string after import",
                self.current_line()
            ));
        }
        Ok(path)
    }

    fn parse_label(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'label'
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        Ok(Stmt::Label(name))
    }

    fn parse_goto(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'goto'
        let name = self.expect_ident()?;
        Ok(Stmt::Goto(name))
    }

    fn parse_test_block(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'test'
        // test "name" { ... }
        let name = if let TokenKind::Str(s) = self.peek() {
            let s = s.clone();
            self.advance();
            s
        } else {
            self.expect_ident()?
        };
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;
        Ok(Stmt::TestBlock { name, body })
    }

    fn parse_bench_block(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'bench'
        let name = if let TokenKind::Str(s) = self.peek() {
            let s = s.clone();
            self.advance();
            s
        } else {
            self.expect_ident()?
        };
        // Optional: `with { warmup: N, iters: N }` — skip for now
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;
        Ok(Stmt::BenchBlock { name, body })
    }

    fn parse_type_alias(&mut self) -> Result<Stmt, String> {
        let doc_comment = self.take_doc_comment();
        self.advance(); // skip 'type'
        let name = self.expect_ident()?;
        self.skip_generic_params_if_present()?;
        // '= Type' is optional (for trait associated type declarations: `type Item`)
        let value = if self.match_tok(&TokenKind::Assign) {
            self.parse_type_annotation()?
        } else {
            // Associated type declaration without concrete type
            name.clone()
        };
        Ok(Stmt::TypeAlias {
            name,
            value,
            doc_comment,
        })
    }

    fn parse_using(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'using'
        let expr = self.parse_expr()?;
        // Check for block form: `using expr { ... }`
        if self.check(&TokenKind::LBrace) {
            self.advance();
            let body = self.parse_block_body()?;
            Ok(Stmt::Using { expr, body: Some(body) })
        } else {
            // Flat form: `using expr` — applies to rest of scope
            Ok(Stmt::Using { expr, body: None })
        }
    }

    fn parse_static_assert(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'static_assert'
        self.expect(&TokenKind::LParen)?;
        let condition = self.parse_expr()?;
        self.expect(&TokenKind::Comma)?;
        let message = if let TokenKind::Str(s) = self.peek() {
            let s = s.clone();
            self.advance();
            s
        } else {
            return Err("static_assert requires a string message".into());
        };
        self.expect(&TokenKind::RParen)?;
        Ok(Stmt::StaticAssert { condition, message })
    }

    fn parse_macro_decl(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'macro'
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Not)?;
        self.expect(&TokenKind::LParen)?;
        let mut params = Vec::new();
        while !self.check(&TokenKind::RParen) {
            params.push(self.expect_ident()?);
            if !self.match_tok(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;
        Ok(Stmt::MacroDecl { name, params, body })
    }

    fn parse_newtype_decl(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'newtype'
        let doc = self.take_doc_comment();
        let name = self.expect_ident()?;
        self.expect(&TokenKind::Assign)?;
        let inner_type = self.expect_ident()?;
        Ok(Stmt::NewtypeDecl { name, inner_type, doc_comment: doc })
    }

    /// Parse a foreign-function declaration:
    ///   extern func name(params) -> ret            (V2-style, no body)
    ///   extern <abi> func name(params) -> ret       (abi = c, wasm_host, js, ...)
    ///   extern c <ret-type> name(<c-params>)        (C-style signature)
    /// No real FFI exists in the tree-walk runtime, so the function is registered
    /// as a stub that returns null; C-style/`cimport` forms parse to a no-op.
    fn parse_extern(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'extern'
        // Optional ABI qualifier (an identifier that is not `func`).
        if matches!(self.peek(), TokenKind::Ident(_)) && !self.check(&TokenKind::Func) {
            // Peek: only treat as ABI if a `func` or a C-style signature follows.
            self.advance();
        }
        if self.match_tok(&TokenKind::Func) {
            // V2-style: func name(params) [-> ret]
            let name = self.expect_ident()?;
            self.expect(&TokenKind::LParen)?;
            let params = self.parse_params()?;
            self.expect(&TokenKind::RParen)?;
            if self.match_tok(&TokenKind::Arrow) {
                let _ = self.parse_type_annotation()?;
            }
            return Ok(Stmt::FuncDecl {
                name,
                params,
                body: Vec::new(),
                is_pure: false,
                is_async: false,
                is_generator: false,
                decorators: Vec::new(),
                doc_comment: None,
            });
        }
        // C-style: <ret-type ...> name(<c-params>). Consume the return type and
        // any pointer markers, capture the name (last ident before `(`), then
        // skip the balanced parameter list.
        let mut last_ident: Option<String> = None;
        while !self.check(&TokenKind::LParen) && !self.is_at_end() {
            if let TokenKind::Ident(n) = self.peek().clone() {
                last_ident = Some(n);
            }
            // Stop if we hit a statement boundary without a paren list.
            if self.newline_before(self.pos) && last_ident.is_some() && self.check_next_is_stmt_start() {
                break;
            }
            self.advance();
        }
        let params: Vec<Param> = Vec::new();
        if self.check(&TokenKind::LParen) {
            self.advance();
            let mut depth = 1;
            while depth > 0 && !self.is_at_end() {
                match self.peek() {
                    TokenKind::LParen => depth += 1,
                    TokenKind::RParen => depth -= 1,
                    _ => {}
                }
                self.advance();
            }
        }
        if let Some(name) = last_ident {
            return Ok(Stmt::FuncDecl {
                name,
                params,
                body: Vec::new(),
                is_pure: false,
                is_async: false,
                is_generator: false,
                decorators: Vec::new(),
                doc_comment: None,
            });
        }
        Ok(Stmt::Multi(Vec::new()))
    }

    /// True if the current token can begin a new statement (used to bound a
    /// C-style extern signature that has no parameter list).
    fn check_next_is_stmt_start(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::Extern | TokenKind::Cimport | TokenKind::Func | TokenKind::Let
                | TokenKind::Const | TokenKind::Struct | TokenKind::Class | TokenKind::Enum
        )
    }

    /// Parse `cimport "header.h"` — a C header import. No real FFI, so this is a no-op.
    fn parse_cimport(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'cimport'
        if let TokenKind::Str(_) = self.peek() {
            self.advance();
        }
        Ok(Stmt::Multi(Vec::new()))
    }

    fn parse_comptime_block(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'comptime'
        // `comptime { ... }` is a compile-time block. `comptime func/const/let/...`
        // uses comptime as a modifier on a declaration; since the interpreter
        // evaluates compile-time code as ordinary runtime code, the declaration is
        // parsed and executed normally (in the current scope, so it stays visible).
        if !self.check(&TokenKind::LBrace) {
            return self.parse_stmt();
        }
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;
        Ok(Stmt::ComptimeBlock { body })
    }

    fn parse_cstruct_decl(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'cstruct'
        let doc = self.take_doc_comment();
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            let fname = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let ftype = self.parse_type_annotation()?;
            fields.push(StructField { name: fname, type_ann: Some(ftype), default: None });
            let _ = self.match_tok(&TokenKind::Comma);
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Stmt::CStructDecl { name, fields, doc_comment: doc })
    }

    fn parse_unsafe_block(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'unsafe'
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;
        Ok(Stmt::UnsafeBlock { body })
    }

    fn parse_actor_decl(&mut self, is_agent: bool) -> Result<Stmt, String> {
        self.advance(); // skip 'actor' or 'agent'
        let name = self.expect_ident()?;
        let mut goal = None;
        // Check for goal string (agents)
        if is_agent {
            if let TokenKind::Str(s) = self.peek().clone() {
                self.advance();
                goal = Some(s);
            }
        }
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;
        Ok(Stmt::ActorDecl { name, is_agent, goal, body })
    }

    fn parse_isolate_block(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'isolate'
        let name = if self.check(&TokenKind::LParen) {
            self.advance();
            let expr = self.parse_expr()?;
            self.expect(&TokenKind::RParen)?;
            Some(expr)
        } else {
            None
        };
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;
        Ok(Stmt::IsolateBlock { name, body })
    }

    fn parse_bitfield_struct(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'bitfield'
        self.expect(&TokenKind::Struct)?;
        let name = self.expect_ident()?;
        let backing = if self.check(&TokenKind::Colon) {
            self.advance();
            Some(self.expect_ident()?)
        } else {
            None
        };
        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            let field_name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let bits = match self.peek().clone() {
                TokenKind::Int(n) => {
                    let b = n as u8;
                    self.advance();
                    b
                }
                _ => return Err("Expected bit count".into()),
            };
            fields.push((field_name, bits));
            if self.check(&TokenKind::Comma) { self.advance(); }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Stmt::BitfieldStructDecl { name, backing, fields, doc_comment: None })
    }

    fn parse_enable_langs(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'enable'
        self.expect(&TokenKind::LBrace)?;
        let mut langs = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
            langs.push(self.expect_ident()?);
            if self.check(&TokenKind::Comma) { self.advance(); }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Stmt::EnableLangs { langs })
    }

    fn parse_yield(&mut self) -> Result<Stmt, String> {
        self.advance(); // skip 'yield'
        // `yield* iterable` — delegation. Desugar to `for (v in iterable) { yield v }`
        // so each value of the inner generator/iterable is yielded in turn.
        if self.check(&TokenKind::Star) {
            self.advance();
            let iter = self.parse_expr()?;
            let var = "__yield_from".to_string();
            let body = vec![Stmt::Yield(Some(Expr::Ident(var.clone())))];
            return Ok(Stmt::ForIn { var, iter, body });
        }
        if self.check(&TokenKind::RBrace) || self.is_at_end() {
            Ok(Stmt::Yield(None))
        } else {
            Ok(Stmt::Yield(Some(self.parse_expr()?)))
        }
    }

    fn parse_expr_stmt(&mut self) -> Result<Stmt, String> {
        let expr = self.parse_expr()?;

        // Check for assignment operators
        let op = match self.peek() {
            TokenKind::Assign => Some(AssignOp::Assign),
            TokenKind::PlusAssign => Some(AssignOp::PlusAssign),
            TokenKind::MinusAssign => Some(AssignOp::MinusAssign),
            TokenKind::StarAssign => Some(AssignOp::StarAssign),
            TokenKind::SlashAssign => Some(AssignOp::SlashAssign),
            TokenKind::PercentAssign => Some(AssignOp::PercentAssign),
            TokenKind::DoubleStarAssign => Some(AssignOp::DoubleStarAssign),
            TokenKind::ShlAssign => Some(AssignOp::ShlAssign),
            TokenKind::ShrAssign => Some(AssignOp::ShrAssign),
            TokenKind::BitAndAssign => Some(AssignOp::BitAndAssign),
            TokenKind::BitOrAssign => Some(AssignOp::BitOrAssign),
            TokenKind::BitXorAssign => Some(AssignOp::BitXorAssign),
            TokenKind::IntDivAssign => Some(AssignOp::IntDivAssign),
            // ++ and -- are sugar for += 1, -= 1
            TokenKind::PlusPlus => {
                self.advance();
                return Ok(Stmt::Assign {
                    target: expr,
                    op: AssignOp::PlusAssign,
                    value: Expr::Int(1),
                });
            }
            TokenKind::MinusMinus => {
                self.advance();
                return Ok(Stmt::Assign {
                    target: expr,
                    op: AssignOp::MinusAssign,
                    value: Expr::Int(1),
                });
            }
            _ => None,
        };

        if let Some(op) = op {
            self.advance(); // skip assignment operator
            let value = self.parse_expr()?;
            Ok(Stmt::Assign {
                target: expr,
                op,
                value,
            })
        } else {
            Ok(Stmt::Expr(expr))
        }
    }

    // ── Expression Parsing (Pratt / precedence climbing) ──

    pub fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_ternary()
    }

    fn parse_ternary(&mut self) -> Result<Expr, String> {
        let expr = self.parse_pipe()?;

        if self.match_tok(&TokenKind::Question) {
            let then_expr = self.parse_expr()?;
            self.expect(&TokenKind::Colon)?;
            let else_expr = self.parse_expr()?;
            return Ok(Expr::Ternary {
                condition: Box::new(expr),
                then_expr: Box::new(then_expr),
                else_expr: Box::new(else_expr),
            });
        }

        Ok(expr)
    }

    fn parse_pipe(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_or()?;
        while self.match_tok(&TokenKind::Pipe) {
            let right = self.parse_or()?;
            expr = Expr::Pipe {
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_null_coalesce()?;
        while self.match_tok(&TokenKind::Or) {
            let right = self.parse_null_coalesce()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op: BinOp::Or,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_null_coalesce(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while self.match_tok(&TokenKind::QuestionQuestion) {
            let right = self.parse_and()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op: BinOp::NullCoalesce,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_bitwise_or()?;
        while self.match_tok(&TokenKind::And) {
            let right = self.parse_bitwise_or()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op: BinOp::And,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_bitwise_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_bitwise_xor()?;
        while self.check(&TokenKind::BitOr) && !self.check(&TokenKind::Pipe) {
            self.advance();
            let right = self.parse_bitwise_xor()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op: BinOp::BitOr,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_bitwise_xor(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_bitwise_and()?;
        while self.match_tok(&TokenKind::BitXor) {
            let right = self.parse_bitwise_and()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op: BinOp::BitXor,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_bitwise_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_equality()?;
        while self.match_tok(&TokenKind::BitAnd) {
            let right = self.parse_equality()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op: BinOp::BitAnd,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_comparison()?;
        loop {
            let op = match self.peek() {
                TokenKind::Eq => BinOp::Eq,
                TokenKind::NotEq => BinOp::NotEq,
                _ => break,
            };
            self.advance();
            let right = self.parse_comparison()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_shift()?;
        loop {
            let op = match self.peek() {
                TokenKind::Lt => BinOp::Lt,
                TokenKind::Gt => BinOp::Gt,
                TokenKind::LtEq => BinOp::LtEq,
                TokenKind::GtEq => BinOp::GtEq,
                TokenKind::In => BinOp::In,
                TokenKind::NotIn => BinOp::NotIn,
                TokenKind::Is => BinOp::Is,
                _ => break,
            };
            self.advance();
            let right = self.parse_shift()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }
        // Check for `as` cast
        if self.match_tok(&TokenKind::As) {
            let target = self.expect_ident()?;
            left = Expr::Cast {
                expr: Box::new(left),
                target,
            };
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_addition()?;
        loop {
            let op = match self.peek() {
                TokenKind::Shl => BinOp::Shl,
                TokenKind::Shr => BinOp::Shr,
                _ => break,
            };
            self.advance();
            let right = self.parse_addition()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_addition(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_multiplication()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplication()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_multiplication(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_power()?;
        loop {
            let op = match self.peek() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Mod,
                TokenKind::DoubleSlash => BinOp::IntDiv,
                _ => break,
            };
            self.advance();
            let right = self.parse_power()?;
            left = Expr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_power(&mut self) -> Result<Expr, String> {
        let base = self.parse_unary()?;
        if self.match_tok(&TokenKind::DoubleStar) {
            // Right-associative
            let exp = self.parse_power()?;
            Ok(Expr::BinOp {
                left: Box::new(base),
                op: BinOp::Pow,
                right: Box::new(exp),
            })
        } else {
            Ok(base)
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            TokenKind::Yield => {
                self.advance();
                // In expression position, lower `yield expr` to an internal call.
                let has_value = !self.check(&TokenKind::Comma)
                    && !self.check(&TokenKind::RParen)
                    && !self.check(&TokenKind::RBracket)
                    && !self.check(&TokenKind::RBrace)
                    && !self.check(&TokenKind::Semicolon)
                    && !self.check(&TokenKind::Eof);
                let value_expr = if has_value {
                    self.parse_expr()?
                } else {
                    Expr::Null
                };
                Ok(Expr::Call {
                    callee: Box::new(Expr::Ident("__yield_expr".to_string())),
                    args: vec![CallArg {
                        name: None,
                        value: value_expr,
                        is_spread: false,
                    }],
                })
            }
            TokenKind::Minus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                })
            }
            TokenKind::Not => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                })
            }
            TokenKind::BitNot => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::BitNot,
                    expr: Box::new(expr),
                })
            }
            TokenKind::Ellipsis => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Spread(Box::new(expr)))
            }
            TokenKind::Await => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Await(Box::new(expr)))
            }
            TokenKind::Move => {
                self.advance();
                let expr = self.parse_unary()?;
                // move just evaluates to the inner expression in tree-walk
                Ok(expr)
            }
            // `async lambda(...)` / `async func(...)` expressions. The runtime
            // executes async synchronously (single-worker), so an async closure
            // is parsed as an ordinary closure whose body may contain `await`.
            TokenKind::Async
                if matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Lambda) | Some(TokenKind::Func)
                ) =>
            {
                self.advance(); // skip 'async'
                self.parse_unary()
            }
            TokenKind::BitAnd => {
                self.advance();
                // &mut x or &x — borrow syntax, evaluates to inner expr in tree-walk
                if self.check(&TokenKind::Mut) {
                    self.advance(); // skip mut
                }
                let expr = self.parse_unary()?;
                Ok(expr)
            }
            TokenKind::Star => {
                self.advance();
                // *r — deref, evaluates to inner expr in tree-walk
                let expr = self.parse_unary()?;
                Ok(expr)
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_primary()?;

        // Tagged template literal: an identifier/field access immediately followed
        // by an f-string, e.g. `sql f"..."` or `tags.html f"..."`.
        if matches!(expr, Expr::Ident(_) | Expr::FieldAccess { .. }) {
            if let TokenKind::FStr(template) = self.peek().clone() {
                self.advance();
                expr = Expr::TaggedTemplate { tag: Box::new(expr), template };
            }
        }

        loop {
            match self.peek() {
                TokenKind::Not => {
                    // `ident!(...)` is a macro call; any other `expr!` is the
                    // postfix force-unwrap operator (unwrap Ok/Some, panic on
                    // Err/None), desugared to `.unwrap()`.
                    let is_macro = matches!(&expr, Expr::Ident(_))
                        && self
                            .tokens
                            .get(self.pos + 1)
                            .map(|t| t.kind == TokenKind::LParen)
                            .unwrap_or(false);
                    if is_macro {
                        if let Expr::Ident(name) = &expr {
                            let name = name.clone();
                            self.advance(); // skip '!'
                            self.expect(&TokenKind::LParen)?;
                            let args = self.parse_call_args()?;
                            self.expect(&TokenKind::RParen)?;
                            expr = Expr::MacroCall { name, args };
                        }
                    } else {
                        self.advance(); // skip '!'
                        expr = Expr::MethodCall {
                            object: Box::new(expr),
                            method: "unwrap".to_string(),
                            args: vec![],
                            optional: false,
                        };
                    }
                }
                TokenKind::LParen => {
                    // Function call
                    self.advance();
                    let args = self.parse_call_args()?;
                    self.expect(&TokenKind::RParen)?;
                    expr = Expr::Call {
                        callee: Box::new(expr),
                        args,
                    };
                }
                TokenKind::LBracket => {
                    // Index or Slice
                    self.advance();
                    // Check for [::step] (ColonColon)
                    if self.match_tok(&TokenKind::ColonColon) {
                        let step = if self.check(&TokenKind::RBracket) {
                            None
                        } else {
                            Some(Box::new(self.parse_expr()?))
                        };
                        self.expect(&TokenKind::RBracket)?;
                        expr = Expr::Slice {
                            object: Box::new(expr),
                            start: None,
                            end: None,
                            step,
                        };
                        continue;
                    }
                    let start = if self.check(&TokenKind::Colon) {
                        None
                    } else {
                        Some(Box::new(self.parse_expr()?))
                    };
                    if self.match_tok(&TokenKind::Colon) {
                        // This is a slice
                        let end = if self.check(&TokenKind::RBracket) || self.check(&TokenKind::Colon) || self.check(&TokenKind::ColonColon) {
                            None
                        } else {
                            Some(Box::new(self.parse_expr()?))
                        };
                        let step = if self.match_tok(&TokenKind::Colon) {
                            if self.check(&TokenKind::RBracket) {
                                None
                            } else {
                                Some(Box::new(self.parse_expr()?))
                            }
                        } else {
                            None
                        };
                        self.expect(&TokenKind::RBracket)?;
                        expr = Expr::Slice {
                            object: Box::new(expr),
                            start,
                            end,
                            step,
                        };
                    } else if self.match_tok(&TokenKind::ColonColon) {
                        // start::step
                        let step = if self.check(&TokenKind::RBracket) {
                            None
                        } else {
                            Some(Box::new(self.parse_expr()?))
                        };
                        self.expect(&TokenKind::RBracket)?;
                        expr = Expr::Slice {
                            object: Box::new(expr),
                            start,
                            end: None,
                            step,
                        };
                    } else {
                        let index = start.unwrap();
                        self.expect(&TokenKind::RBracket)?;
                        expr = Expr::Index {
                            object: Box::new(expr),
                            index,
                        };
                    }
                }
                TokenKind::Dot | TokenKind::QuestionDot => {
                    let is_optional = self.peek_token().kind == TokenKind::QuestionDot;
                    self.advance();
                    // Allow numeric fields for tuple/newtype access: x.0
                    let field = if let TokenKind::Int(n) = self.peek().clone() {
                        self.advance();
                        n.to_string()
                    } else {
                        self.expect_field_name()?
                    };
                    if self.check(&TokenKind::LParen) {
                        self.advance();
                        let args = self.parse_call_args()?;
                        self.expect(&TokenKind::RParen)?;
                        expr = Expr::MethodCall {
                            object: Box::new(expr),
                            method: field,
                            args,
                            optional: is_optional,
                        };
                    } else {
                        expr = Expr::FieldAccess {
                            object: Box::new(expr),
                            field,
                            optional: is_optional,
                        };
                    }
                }
                TokenKind::DotDot => {
                    // Range
                    self.advance();
                    let end = self.parse_addition()?;
                    expr = Expr::Range {
                        start: Box::new(expr),
                        end: Box::new(end),
                        inclusive: false,
                    };
                }
                TokenKind::DotDotEq => {
                    // Inclusive range
                    self.advance();
                    let end = self.parse_addition()?;
                    expr = Expr::Range {
                        start: Box::new(expr),
                        end: Box::new(end),
                        inclusive: true,
                    };
                }
                TokenKind::Question => {
                    // Postfix ? (try operator) vs ternary `a ? b : c`.
                    // It is the try operator when the token after `?` closes the
                    // expression (`)`, `}`, `]`, `,`, `;`, EOF), starts a new
                    // statement (a newline separates them), or is a statement
                    // keyword that cannot begin a ternary's "then" branch.
                    let next_after = self.tokens.get(self.pos + 1).map(|t| &t.kind).cloned();
                    let newline_after = self.newline_before(self.pos + 1);
                    let is_try = newline_after || matches!(next_after,
                        Some(TokenKind::RParen) | Some(TokenKind::RBrace) | Some(TokenKind::RBracket) |
                        Some(TokenKind::Comma) | Some(TokenKind::Semicolon) | Some(TokenKind::Newline) |
                        Some(TokenKind::Eof) | None |
                        Some(TokenKind::Return) | Some(TokenKind::Let) | Some(TokenKind::Const)
                    );
                    if is_try {
                        self.advance();
                        expr = Expr::TryUnwrap(Box::new(expr));
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_call_args(&mut self) -> Result<Vec<CallArg>, String> {
        // Arguments live inside `( )`, so struct literals are unambiguous here
        // even when the surrounding header suppressed them.
        let saved_no_struct = self.no_struct_literal;
        self.no_struct_literal = false;
        let mut args = Vec::new();
        while !self.check(&TokenKind::RParen) && !self.is_at_end() {
            let is_spread = self.match_tok(&TokenKind::Ellipsis);

            // Check for named argument: name: value. The name may be a plain
            // identifier or a soft keyword (e.g. `type: "int"`, `default: 10`).
            let arg_name = match self.peek().clone() {
                TokenKind::Ident(name) => Some(name),
                other => Self::soft_keyword_text(&other).map(|s| s.to_string()),
            };
            if let Some(name) = arg_name {
                if self.tokens.get(self.pos + 1).map(|t| &t.kind) == Some(&TokenKind::Colon) {
                    self.advance(); // skip name
                    self.advance(); // skip colon
                    let value = self.parse_expr()?;
                    args.push(CallArg {
                        name: Some(name),
                        value,
                        is_spread,
                    });
                    if !self.match_tok(&TokenKind::Comma) {
                        break;
                    }
                    continue;
                }
            }

            let value = self.parse_expr()?;
            args.push(CallArg {
                name: None,
                value,
                is_spread,
            });
            if !self.match_tok(&TokenKind::Comma) {
                break;
            }
        }
        self.no_struct_literal = saved_no_struct;
        Ok(args)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            TokenKind::Int(n) => {
                self.advance();
                Ok(Expr::Int(n))
            }
            TokenKind::BigIntLit(s) => {
                self.advance();
                Ok(Expr::BigIntLit(s))
            }
            TokenKind::Float(f) => {
                self.advance();
                Ok(Expr::Float(f))
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr::Str(s))
            }
            TokenKind::FStr(s) => {
                self.advance();
                Ok(Expr::FStr(s))
            }
            TokenKind::ByteStr(bytes) => {
                self.advance();
                Ok(Expr::ByteStr(bytes))
            }
            TokenKind::Bool(b) => {
                self.advance();
                Ok(Expr::Bool(b))
            }
            TokenKind::Null => {
                self.advance();
                Ok(Expr::Null)
            }
            TokenKind::Self_ => {
                self.advance();
                Ok(Expr::Self_)
            }
            TokenKind::Lazy => {
                self.advance();
                let expr = self.parse_expr()?;
                Ok(Expr::Lazy(Box::new(expr)))
            }
            TokenKind::Super => {
                self.advance();
                Ok(Expr::Ident("super".to_string()))
            }
            TokenKind::Ident(name) => {
                self.advance();
                // Check for struct literal: Name { field: val, ... } or Name { ...spread, field: val }
                // Disambiguate: must be { followed by Ident+Colon, Ellipsis, or empty {}
                // Suppressed in header contexts (if/while/for/match) where `{` opens a block.
                if self.check(&TokenKind::LBrace) && !self.no_struct_literal {
                    let look1 = self.tokens.get(self.pos + 1).map(|t| &t.kind);
                    let look2 = self.tokens.get(self.pos + 2).map(|t| &t.kind);
                    let is_struct_lit = match look1 {
                        Some(TokenKind::RBrace) => true, // empty: Name {}
                        Some(TokenKind::Ellipsis) => true, // spread: Name { ...base }
                        Some(TokenKind::Ident(_)) => matches!(look2, Some(TokenKind::Colon)),
                        _ => false,
                    };
                    if is_struct_lit {
                        self.advance(); // consume {
                        let mut fields = Vec::new();
                        let mut spread = None;
                        while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
                            if self.match_tok(&TokenKind::Ellipsis) {
                                spread = Some(Box::new(self.parse_expr()?));
                            } else {
                                let fname = self.expect_ident()?;
                                self.expect(&TokenKind::Colon)?;
                                let fval = self.parse_expr()?;
                                fields.push((fname, fval));
                            }
                            if !self.match_tok(&TokenKind::Comma) {
                                break;
                            }
                        }
                        self.expect(&TokenKind::RBrace)?;
                        return Ok(Expr::StructLit { name, fields, spread });
                    }
                }
                Ok(Expr::Ident(name))
            }
            TokenKind::LParen => {
                self.advance();
                if self.check(&TokenKind::RParen) {
                    self.advance();
                    return Ok(Expr::Tuple(vec![]));
                }
                let expr = self.parse_expr_allow_struct()?;
                // Generator comprehension: (expr for x in iter ...)
                if self.check(&TokenKind::For) {
                    let clauses = self.parse_comp_clauses()?;
                    self.expect(&TokenKind::RParen)?;
                    return Ok(Expr::GenComp {
                        expr: Box::new(expr),
                        clauses,
                    });
                }
                if self.check(&TokenKind::Comma) {
                    // Tuple literal: (a, b, c)
                    let mut items = vec![expr];
                    while self.match_tok(&TokenKind::Comma) {
                        if self.check(&TokenKind::RParen) {
                            break; // trailing comma
                        }
                        items.push(self.parse_expr_allow_struct()?);
                    }
                    self.expect(&TokenKind::RParen)?;
                    Ok(Expr::Tuple(items))
                } else {
                    self.expect(&TokenKind::RParen)?;
                    Ok(Expr::Grouped(Box::new(expr)))
                }
            }
            TokenKind::LBracket => {
                // List literal or list comprehension
                self.advance();
                if self.check(&TokenKind::RBracket) {
                    self.advance();
                    return Ok(Expr::List(vec![]));
                }
                let first = self.parse_expr_allow_struct()?;
                // Check for list comprehension: [expr for var in iterable ...]
                if self.check(&TokenKind::For) {
                    let clauses = self.parse_comp_clauses()?;
                    self.expect(&TokenKind::RBracket)?;
                    return Ok(Expr::ListComp {
                        expr: Box::new(first),
                        clauses,
                    });
                }
                let mut elements = vec![first];
                while self.match_tok(&TokenKind::Comma) {
                    if self.check(&TokenKind::RBracket) { break; }
                    elements.push(self.parse_expr_allow_struct()?);
                }
                self.expect(&TokenKind::RBracket)?;
                Ok(Expr::List(elements))
            }
            TokenKind::LBrace => {
                // Block expression, dict literal or dict comprehension
                // { let x = 5; x } is a block expression (statement inside => DoBlock)
                // { key: value } is a dict literal
                // { ...base, key: value } supports dict spread
                // { } is an empty dict
                // Detect block expression: first token inside is a statement keyword
                let look1 = self.tokens.get(self.pos + 1).map(|t| &t.kind);
                let is_block_expr = matches!(look1,
                    Some(TokenKind::Let) | Some(TokenKind::Const) | Some(TokenKind::If) |
                    Some(TokenKind::While) | Some(TokenKind::For) | Some(TokenKind::Return) |
                    Some(TokenKind::Throw) | Some(TokenKind::Func)
                );
                if is_block_expr {
                    self.advance(); // consume {
                    let body = self.parse_block_body()?;
                    return Ok(Expr::DoBlock(body));
                }
                self.advance();
                if self.check(&TokenKind::RBrace) {
                    self.advance();
                    return Ok(Expr::Dict(vec![]));
                }
                let mut pairs = Vec::new();
                while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
                    if self.match_tok(&TokenKind::Ellipsis) {
                        let spread_expr = self.parse_expr()?;
                        if self.check(&TokenKind::For) {
                            return Err(format!(
                                "[line {}] Error: Dict comprehensions cannot use spread entries",
                                self.current_line()
                            ));
                        }
                        // Reuse Expr::Spread for dict spread entries; value is ignored by evaluator.
                        pairs.push((Expr::Spread(Box::new(spread_expr)), Expr::Null));
                    } else {
                        let key = self.parse_expr()?;
                        self.expect(&TokenKind::Colon)?;
                        let value = self.parse_expr()?;
                        // Check for dict comprehension
                        if pairs.is_empty() && self.check(&TokenKind::For) {
                            let clauses = self.parse_comp_clauses()?;
                            self.expect(&TokenKind::RBrace)?;
                            return Ok(Expr::DictComp {
                                key_expr: Box::new(key),
                                val_expr: Box::new(value),
                                clauses,
                            });
                        }
                        pairs.push((key, value));
                    }

                    if !self.match_tok(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RBrace)?;
                Ok(Expr::Dict(pairs))
            }
            TokenKind::Hash => {
                // Set literal or set comprehension: #{a, b} or #{expr for var in iter}
                self.advance(); // skip #
                self.expect(&TokenKind::LBrace)?;
                if self.check(&TokenKind::RBrace) {
                    self.advance();
                    return Ok(Expr::Set(vec![]));
                }
                let first = self.parse_expr()?;
                // Check for set comprehension: #{expr for var in iterable}
                if self.check(&TokenKind::For) {
                    let clauses = self.parse_comp_clauses()?;
                    self.expect(&TokenKind::RBrace)?;
                    return Ok(Expr::SetComp {
                        expr: Box::new(first),
                        clauses,
                    });
                }
                let mut elements = vec![first];
                while self.match_tok(&TokenKind::Comma) {
                    if self.check(&TokenKind::RBrace) { break; }
                    elements.push(self.parse_expr()?);
                }
                self.expect(&TokenKind::RBrace)?;
                Ok(Expr::Set(elements))
            }
            TokenKind::Lambda => {
                self.parse_lambda()
            }
            // Anonymous function expression: func(params) { body } or
            // func(params) => expr — used as callbacks. An optional name is
            // accepted and ignored. `async func(...)` reaches here via parse_unary.
            TokenKind::Func => self.parse_func_expr(),
            TokenKind::New => {
                self.advance();
                let class = self.expect_ident()?;
                self.expect(&TokenKind::LParen)?;
                let args = self.parse_call_args()?;
                self.expect(&TokenKind::RParen)?;
                Ok(Expr::New { class, args })
            }
            TokenKind::Do => {
                self.advance();
                self.expect(&TokenKind::LBrace)?;
                let body = self.parse_block_body()?;
                Ok(Expr::DoBlock(body))
            }
            TokenKind::Match => {
                // Match expression — subject parens optional (match x { } / match (x) { }).
                self.advance();
                let subject = self.parse_cond_expr()?;
                self.expect(&TokenKind::LBrace)?;
                let mut arms = Vec::new();
                while !self.check(&TokenKind::RBrace) && !self.is_at_end() {
                    if self.match_tok(&TokenKind::Default) {
                        self.expect(&TokenKind::LBrace)?;
                        let body = self.parse_block_body()?;
                        arms.push(MatchArm { pattern: Pattern::Default, guard: None, body });
                        self.match_tok(&TokenKind::Comma);
                        continue;
                    }
                    self.expect(&TokenKind::Case)?;
                    self.expect(&TokenKind::LParen)?;
                    let pattern = self.parse_pattern()?;
                    self.expect(&TokenKind::RParen)?;
                    let guard = if self.match_tok(&TokenKind::If) {
                        Some(self.parse_expr()?)
                    } else {
                        None
                    };
                    self.expect(&TokenKind::LBrace)?;
                    let body = self.parse_block_body()?;
                    arms.push(MatchArm { pattern, guard, body });
                    self.match_tok(&TokenKind::Comma);
                }
                self.expect(&TokenKind::RBrace)?;
                Ok(Expr::MatchExpr { subject: Box::new(subject), arms })
            }
            TokenKind::TypeOf => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::TypeOf(Box::new(expr)))
            }
            // `type` is a keyword for type aliases at statement level, but in
            // expression position `type(x)` is the type-introspection builtin
            // (returns the type name string). Route it to the `type_of` builtin.
            TokenKind::Type if self.tokens.get(self.pos + 1).map(|t| t.kind == TokenKind::LParen).unwrap_or(false) => {
                self.advance();
                Ok(Expr::Ident("type_of".to_string()))
            }
            // Soft keywords used as a value reference (e.g. a variable named
            // `label` or `from`). Safe here because statement-level uses of these
            // keywords are dispatched before reaching expression parsing.
            TokenKind::Label | TokenKind::From | TokenKind::Default | TokenKind::Type => {
                let name = Self::soft_keyword_text(self.peek()).unwrap().to_string();
                self.advance();
                Ok(Expr::Ident(name))
            }
            TokenKind::TestBlock => {
                self.advance();
                Ok(Expr::Ident("test".to_string()))
            }
            _ => Err(format!(
                "[line {}] Error: Expected expression, found {:?}",
                self.current_line(),
                self.peek()
            )),
        }
    }

    /// Parse one or more `for var in iter [if cond]` clauses for comprehensions.
    fn parse_comp_clauses(&mut self) -> Result<Vec<CompClause>, String> {
        let mut clauses = Vec::new();
        while self.check(&TokenKind::For) {
            self.advance(); // skip 'for'
            // `for await item in asyncIterable` — async runs synchronously here.
            self.match_tok(&TokenKind::Await);
            // Support destructuring: for (a, b) in iter
            let (var, destructure) = if self.check(&TokenKind::LParen) {
                self.advance();
                let mut names = Vec::new();
                while !self.check(&TokenKind::RParen) && !self.is_at_end() {
                    names.push(self.expect_ident()?);
                    if !self.match_tok(&TokenKind::Comma) { break; }
                }
                self.expect(&TokenKind::RParen)?;
                let first = names.first().cloned().unwrap_or_default();
                (first, if names.len() > 1 { Some(names) } else { None })
            } else {
                (self.expect_ident()?, None)
            };
            self.expect(&TokenKind::In)?;
            let iter = self.parse_expr()?;
            let cond = if self.match_tok(&TokenKind::If) {
                Some(Box::new(self.parse_expr()?))
            } else {
                None
            };
            clauses.push(CompClause { var, destructure, iter: Box::new(iter), cond });
        }
        Ok(clauses)
    }

    /// Parse an anonymous `func` expression: `func(params) { body }` or
    /// `func(params) => expr`. An optional name (as in `func handler(x) {}`) is
    /// accepted and discarded. Produces the same AST as a lambda.
    fn parse_func_expr(&mut self) -> Result<Expr, String> {
        self.advance(); // skip 'func'
        // Optional name — ignored for anonymous function expressions.
        if matches!(self.peek(), TokenKind::Ident(_)) {
            self.advance();
        }
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen)?;
        // Optional return type annotation: func(x) -> T { ... }
        if self.match_tok(&TokenKind::Arrow) {
            let _ = self.parse_type_annotation()?;
        }
        if self.match_tok(&TokenKind::FatArrow) {
            let body = self.parse_expr()?;
            return Ok(Expr::Lambda {
                params,
                body: Box::new(body),
                is_move: false,
            });
        }
        self.expect(&TokenKind::LBrace)?;
        let body = self.parse_block_body()?;
        Ok(Expr::LambdaBlock {
            params,
            body,
            is_move: false,
        })
    }

    fn parse_lambda(&mut self) -> Result<Expr, String> {
        self.advance(); // skip 'lambda'
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen)?;

        if self.match_tok(&TokenKind::FatArrow) {
            // Expression lambda: lambda(x) => x * 2
            let body = self.parse_expr()?;
            Ok(Expr::Lambda {
                params,
                body: Box::new(body),
                is_move: false,
            })
        } else {
            // Block lambda: lambda(x) { ... }
            self.expect(&TokenKind::LBrace)?;
            let body = self.parse_block_body()?;
            Ok(Expr::LambdaBlock {
                params,
                body,
                is_move: false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;

    fn parse(src: &str) -> Program {
        let mut lexer = Lexer::new(src);
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        parser.parse().unwrap()
    }

    #[test]
    fn test_let_statement() {
        let prog = parse("let x = 42");
        assert_eq!(prog.stmts.len(), 1);
        assert!(matches!(&prog.stmts[0], Stmt::Let { name, .. } if name == "x"));
    }

    #[test]
    fn test_const_statement() {
        let prog = parse("const PI = 3.14");
        assert_eq!(prog.stmts.len(), 1);
        assert!(matches!(&prog.stmts[0], Stmt::Const { name, .. } if name == "PI"));
    }

    #[test]
    fn test_function_decl() {
        let prog = parse("func add(a, b) { return a + b }");
        assert_eq!(prog.stmts.len(), 1);
        assert!(matches!(&prog.stmts[0], Stmt::FuncDecl { name, params, .. } if name == "add" && params.len() == 2));
    }

    #[test]
    fn test_if_else() {
        let prog = parse("if (x > 0) { print(x) } else { print(0) }");
        assert!(matches!(&prog.stmts[0], Stmt::If { .. }));
    }

    #[test]
    fn test_for_in() {
        let prog = parse("for (x in items) { print(x) }");
        assert!(matches!(&prog.stmts[0], Stmt::ForIn { var, .. } if var == "x"));
    }

    #[test]
    fn test_match() {
        let prog = parse(r#"match (x) { case (1) { print("one") } case (2) { print("two") } default { print("other") } }"#);
        assert!(matches!(&prog.stmts[0], Stmt::Match { arms, .. } if arms.len() == 3));
    }

    #[test]
    fn test_class_decl() {
        let prog = parse("class Dog extends Animal { func bark() { print(\"woof\") } }");
        assert!(matches!(&prog.stmts[0], Stmt::ClassDecl { name, parent, .. } if name == "Dog" && parent.as_deref() == Some("Animal")));
    }

    #[test]
    fn test_doc_comment_attaches_to_function() {
        let prog = parse("/// Adds two integers\nfunc add(a, b) { return a + b }");
        assert!(matches!(&prog.stmts[0], Stmt::FuncDecl { doc_comment: Some(doc), .. } if doc == "Adds two integers"));
    }

    #[test]
    fn test_doc_block_comment_attaches_to_struct() {
        let prog = parse("/**\n * Represents a 2D point.\n * @field x horizontal\n */\nstruct Point { x, y }");
        assert!(matches!(&prog.stmts[0], Stmt::StructDecl { doc_comment: Some(doc), .. } if doc.contains("Represents a 2D point.") && doc.contains("@field x horizontal")));
    }

    #[test]
    fn test_computed_property_in_class_body() {
        let prog = parse("class Circle { get area -> float { return self.radius * self.radius } }");
        assert!(matches!(&prog.stmts[0], Stmt::ClassDecl { body, .. } if matches!(&body[0], Stmt::FuncDecl { name, .. } if name == "get_area")));
    }

    #[test]
    fn test_generic_function_decl() {
        let prog = parse("func identity<T: Comparable>(value: T) -> T { return value }");
        assert!(matches!(&prog.stmts[0], Stmt::FuncDecl { name, params, .. } if name == "identity" && params.len() == 1));
    }

    #[test]
    fn test_generic_struct_decl() {
        let prog = parse("struct Pair<T, U> { first: T, second: U }");
        assert!(matches!(&prog.stmts[0], Stmt::StructDecl { name, fields, .. } if name == "Pair" && fields.len() == 2));
    }

    #[test]
    fn test_where_clause_function_decl() {
        let prog = parse("func process<T, U>(items: T, handler: U) -> list<U> where T: Iterable, U: Comparable + Printable { return handler }");
        assert!(matches!(&prog.stmts[0], Stmt::FuncDecl { name, params, .. } if name == "process" && params.len() == 2));
    }

    #[test]
    fn test_macro_decl_and_call() {
        let prog = parse("macro debug!(expr) { print(expr) } debug!(42)");
        assert!(matches!(&prog.stmts[0], Stmt::MacroDecl { name, params, .. } if name == "debug" && params.len() == 1));
        assert!(matches!(&prog.stmts[1], Stmt::Expr(Expr::MacroCall { name, args }) if name == "debug" && args.len() == 1));
    }

    #[test]
    fn test_struct_decl() {
        let prog = parse("struct Point { x, y }");
        assert!(matches!(&prog.stmts[0], Stmt::StructDecl { name, fields, .. } if name == "Point" && fields.len() == 2));
    }

    #[test]
    fn test_binary_ops() {
        let prog = parse("let x = 2 + 3 * 4");
        // Should parse as 2 + (3 * 4) due to precedence
        if let Stmt::Let { value: Some(Expr::BinOp { op: BinOp::Add, right, .. }), .. } = &prog.stmts[0] {
            assert!(matches!(right.as_ref(), Expr::BinOp { op: BinOp::Mul, .. }));
        } else {
            panic!("Expected binary op");
        }
    }

    #[test]
    fn test_ternary() {
        let prog = parse("let x = a > b ? a : b");
        if let Stmt::Let { value: Some(Expr::Ternary { .. }), .. } = &prog.stmts[0] {
            // ok
        } else {
            panic!("Expected ternary");
        }
    }

    #[test]
    fn test_lambda() {
        let prog = parse("let f = lambda(x) => x * 2");
        if let Stmt::Let { value: Some(Expr::Lambda { params, .. }), .. } = &prog.stmts[0] {
            assert_eq!(params.len(), 1);
        } else {
            panic!("Expected lambda");
        }
    }
}
