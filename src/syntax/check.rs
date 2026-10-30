//! Static checks that need the whole tree (INTERNALS 4.5): block labels, the reserved `v2`
//! name, `break`/`continue`/`return` placement, redeclarations and unused expression values.
//! A file that parses gets every check, and all violations are reported in source order.
//! (Checks that the grammar decides locally are parse errors, raised by the parser.)

use std::collections::HashMap;
use std::rc::Rc;

use super::ast::*;
use super::span::Span;
use super::Diag;

pub fn check(program: &[Stmt]) -> Vec<Diag> {
    let mut c = Checker { diags: Vec::new(), labels: HashMap::new() };
    c.body(&program.iter().collect::<Vec<_>>(), Ctx { in_loop: false, in_func: false }, &[]);
    c.diags.sort_by_key(|d| (d.span.file.0, d.span.lo));
    c.diags
}

#[derive(Clone, Copy)]
struct Ctx {
    in_loop: bool,
    in_func: bool,
}

struct Checker {
    diags: Vec<Diag>,
    /// Block labels seen so far in the file, with where.
    labels: HashMap<Rc<str>, Span>,
}

impl Checker {
    /// One body: names declared directly in it (seeded with what the body introduces:
    /// parameters, loop variables, the catch name) must be unique (DOCS 3.3).
    fn body(&mut self, stmts: &[&Stmt], ctx: Ctx, introduced: &[&Name]) {
        let mut declared: HashMap<Rc<str>, Span> = HashMap::new();
        for n in introduced {
            self.declare(&mut declared, n.text.clone(), n.span);
        }
        for s in stmts {
            for (name, span) in declarations(s) {
                self.declare(&mut declared, name, span);
            }
            self.stmt(s, ctx);
        }
    }

    fn declare(&mut self, declared: &mut HashMap<Rc<str>, Span>, name: Rc<str>, span: Span) {
        match declared.get(&name) {
            Some(&first) => self
                .diags
                .push(Diag::new(format!("'{name}' is already declared in this block"), span).note("first declared here", first)),
            None => {
                declared.insert(name, span);
            }
        }
    }

    /// A statement that is only an expression must be a call or an action block (4.5): any other
    /// value is computed and dropped, which is almost always a mistake (`x == 1` meant as
    /// `x = 1`, or a line starting with `-` meant to continue the line before).
    fn unused_value(&mut self, e: &Expr) {
        let last = match &e.kind {
            ExprKind::OptChain(chain) => &chain.kind,
            k => k,
        };
        if matches!(last, ExprKind::Call { .. } | ExprKind::Block(_)) {
            return;
        }
        let hint = match &e.kind {
            ExprKind::Unary(..) => "to continue the previous line, end that line with the operator",
            ExprKind::Binary(BinOp::Eq, ..) => "did you mean '='?",
            _ => "assign it with let, or remove it",
        };
        self.diags.push(Diag::new("this expression's value is not used", e.span).hint(hint));
    }

    fn body_of(&mut self, body: &Body, ctx: Ctx, introduced: &[&Name]) {
        let stmts: Vec<&Stmt> = body.stmts.iter().collect();
        self.body(&stmts, ctx, introduced);
    }

    fn stmt(&mut self, s: &Stmt, ctx: Ctx) {
        match &s.kind {
            StmtKind::Let(_, e) => {
                if let Some(e) = e {
                    self.expr(e, ctx);
                }
            }
            StmtKind::Const(_, e) | StmtKind::Throw(e) => self.expr(e, ctx),
            StmtKind::Expr(e) => {
                self.unused_value(e);
                self.expr(e, ctx);
            }
            StmtKind::Break if !ctx.in_loop => self.diags.push(Diag::new("break outside a loop", s.span)),
            StmtKind::Continue if !ctx.in_loop => self.diags.push(Diag::new("continue outside a loop", s.span)),
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::Return(e) => {
                if !ctx.in_func {
                    self.diags.push(Diag::new("return outside a function", s.span));
                }
                if let Some(e) = e {
                    self.expr(e, ctx);
                }
            }
            StmtKind::Assign { target, value, .. } => {
                match target {
                    Target::Name(..) => {}
                    Target::Member(obj, _) => self.expr(obj, ctx),
                    Target::Index(obj, index, _) => {
                        self.expr(obj, ctx);
                        self.expr(index, ctx);
                    }
                }
                self.expr(value, ctx);
            }
            StmtKind::If { arms, else_ } => {
                for (cond, body) in arms {
                    self.expr(cond, ctx);
                    self.body_of(body, ctx, &[]);
                }
                if let Some(b) = else_ {
                    self.body_of(b, ctx, &[]);
                }
            }
            StmtKind::While { cond, body } => {
                self.expr(cond, ctx);
                self.body_of(body, Ctx { in_loop: true, ..ctx }, &[]);
            }
            StmtKind::For { first, second, iter, body } => {
                match iter {
                    ForIter::Value(e) => self.expr(e, ctx),
                    ForIter::Range(a, b) => {
                        self.expr(a, ctx);
                        self.expr(b, ctx);
                    }
                }
                let mut vars = vec![first];
                if let Some(s) = second {
                    vars.push(s);
                }
                self.body_of(body, Ctx { in_loop: true, ..ctx }, &vars);
            }
            StmtKind::Try { body, catch, finally } => {
                self.body_of(body, ctx, &[]);
                if let Some((name, b)) = catch {
                    let names: Vec<&Name> = name.iter().collect();
                    self.body_of(b, ctx, &names);
                }
                if let Some(b) = finally {
                    self.body_of(b, ctx, &[]);
                }
            }
            StmtKind::Func(f) => self.func(f),
            StmtKind::Import { .. } | StmtKind::Extern(_) => {}
            StmtKind::EngineImport(imp) => {
                if let Selector::V2 { span } = &imp.selector {
                    self.diags.push(Diag::new("the v2 selector is only valid inside a block", *span));
                }
                self.reserved_selector(&imp.selector);
            }
            StmtKind::Block(b) => self.block(b, ctx),
        }
    }

    fn func(&mut self, f: &FuncDecl) {
        let ctx = Ctx { in_loop: false, in_func: true };
        for (_, default) in &f.params {
            if let Some(d) = default {
                self.expr(d, ctx);
            }
        }
        let mut introduced: Vec<&Name> = f.params.iter().map(|(n, _)| n).collect();
        if let Some(r) = &f.rest {
            introduced.push(r);
        }
        match &f.body {
            FuncBody::Block(b) => self.body_of(b, ctx, &introduced),
            FuncBody::Expr(e) => self.expr(e, ctx),
        }
    }

    fn block(&mut self, b: &EngineBlock, ctx: Ctx) {
        if let Some(label) = &b.label {
            if &*label.text == "v2" {
                self.diags.push(Diag::new("'v2' is reserved for this file's V2 functions", label.span));
            } else if let Some(&first) = self.labels.get(&label.text) {
                self.diags.push(
                    Diag::new(format!("duplicate block label '{}'", label.text), label.span).note("first used here", first),
                );
            } else {
                self.labels.insert(label.text.clone(), label.span);
            }
        }
        for (_, e) in &b.args {
            self.expr(e, ctx);
        }
        for imp in &b.directives.imports {
            self.reserved_selector(&imp.selector);
        }
    }

    /// `v2.x` is not a package selector: `v2` is reserved (4.5).
    fn reserved_selector(&mut self, sel: &Selector) {
        let reserved = match sel {
            Selector::Module { lang, .. } if &*lang.text == "v2" => Some(lang.span),
            Selector::Label { label } if &*label.text == "v2" => Some(label.span),
            _ => None,
        };
        if let Some(span) = reserved {
            self.diags.push(Diag::new("'v2' is reserved for this file's V2 functions", span));
        }
    }

    fn expr(&mut self, e: &Expr, ctx: Ctx) {
        match &e.kind {
            ExprKind::Null
            | ExprKind::Bool(_)
            | ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bytes(_)
            | ExprKind::Name(..) => {}
            ExprKind::FStr(parts) => {
                for p in parts {
                    if let FPart::Expr { expr, .. } = p {
                        self.expr(expr, ctx);
                    }
                }
            }
            ExprKind::List(items) => items.iter().for_each(|x| self.expr(x, ctx)),
            ExprKind::Dict(entries) => {
                for (k, v) in entries {
                    if let DictKey::Computed(k) = k {
                        self.expr(k, ctx);
                    }
                    self.expr(v, ctx);
                }
            }
            ExprKind::Unary(_, _, x) | ExprKind::OptChain(x) => self.expr(x, ctx),
            ExprKind::Binary(_, _, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) => {
                self.expr(a, ctx);
                self.expr(b, ctx);
            }
            ExprKind::Ternary(c, a, b) => {
                self.expr(c, ctx);
                self.expr(a, ctx);
                self.expr(b, ctx);
            }
            ExprKind::Call { callee, args, .. } => {
                self.expr(callee, ctx);
                for a in args {
                    match a {
                        Arg::Pos(x) | Arg::Named(_, x) | Arg::Spread(x) => self.expr(x, ctx),
                    }
                }
            }
            ExprKind::Index { obj, index, .. } => {
                self.expr(obj, ctx);
                self.expr(index, ctx);
            }
            ExprKind::Slice { obj, lo, hi, .. } => {
                self.expr(obj, ctx);
                for x in [lo, hi].into_iter().flatten() {
                    self.expr(x, ctx);
                }
            }
            ExprKind::Member { obj, .. } => self.expr(obj, ctx),
            ExprKind::Func(f) => self.func(f),
            ExprKind::Block(b) => self.block(b, ctx),
        }
    }
}

/// The names a statement declares directly in its body (4.5).
pub(crate) fn declarations(s: &Stmt) -> Vec<(Rc<str>, Span)> {
    match &s.kind {
        StmtKind::Let(n, _) | StmtKind::Const(n, _) => vec![(n.text.clone(), n.span)],
        StmtKind::Func(f) => f.name.iter().map(|n| (n.text.clone(), n.span)).collect(),
        StmtKind::Import { alias, .. } => vec![(alias.text.clone(), alias.span)],
        StmtKind::EngineImport(imp) => match &imp.items {
            ImportItems::Names(items) => items
                .iter()
                .map(|i| match &i.alias {
                    Some(a) => (a.text.clone(), a.span),
                    None => (i.name.clone(), i.span),
                })
                .collect(),
            ImportItems::Module(alias) => vec![(alias.text.clone(), alias.span)],
        },
        StmtKind::Extern(x) => match &x.alias {
            Some(a) => vec![(a.text.clone(), a.span)],
            None => x.decls.iter().zip(&x.decl_spans).map(|(d, sp)| (d.name.clone(), *sp)).collect(),
        },
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parser::parse_str;

    fn diags(src: &str) -> Vec<String> {
        let prog = parse_str(src).unwrap();
        check(&prog).into_iter().map(|d| d.message).collect()
    }

    #[test]
    fn clean_programs_pass() {
        assert!(diags("let x = 1\nif x { let x = 2 }\nfunc f(a) { return a }\nwhile true { break }").is_empty());
        // Inner bodies may shadow; separate functions have separate bodies.
        assert!(diags("func f(x) { if x { let x = 1 } }\nfunc g(x) { return x }").is_empty());
    }

    #[test]
    fn redeclarations() {
        assert_eq!(diags("let a = 1\nlet a = 2"), vec!["'a' is already declared in this block"]);
        assert_eq!(diags("func f(x) { let x = 1 }"), vec!["'x' is already declared in this block"]);
        assert_eq!(diags("for i in xs { let i = 2 }"), vec!["'i' is already declared in this block"]);
        assert_eq!(diags("try { } catch (e) { let e = 1 }"), vec!["'e' is already declared in this block"]);
        assert_eq!(diags("func f() { }\nconst f = 1"), vec!["'f' is already declared in this block"]);
        assert_eq!(diags("import \"./u.v2\" as u\n@import { mean as u } from py.statistics"), vec!["'u' is already declared in this block"]);
        assert_eq!(diags("extern \"m\" { double cos(double x); }\nlet cos = 1"), vec!["'cos' is already declared in this block"]);
        assert_eq!(diags("let f = func(a) { let a = 1 }"), vec!["'a' is already declared in this block"]);
        // The two loop variables are both declared in the loop body.
        assert_eq!(diags("for i, i in xs { }"), vec!["'i' is already declared in this block"]);
        let prog = parse_str("for i, i in xs { }").unwrap();
        let d = &check(&prog)[0];
        assert_eq!((d.span.col, d.notes[0].1.col), (8, 5));
    }

    #[test]
    fn placement_and_reserved_names() {
        assert_eq!(diags("break\ncontinue\nreturn 1"), vec!["break outside a loop", "continue outside a loop", "return outside a function"]);
        assert_eq!(diags("while true { func f() { break } }"), vec!["break outside a loop"]);
        assert!(diags("func f() { while true { return 1 } }").is_empty());
        assert_eq!(diags("@import { log } from v2"), vec!["the v2 selector is only valid inside a block"]);
        assert_eq!(diags("@import { x } from v2.thing"), vec!["'v2' is reserved for this file's V2 functions"]);
        assert_eq!(diags("@py v2 { x }"), vec!["'v2' is reserved for this file's V2 functions"]);
        assert!(diags("@py {\n  @import { log } from v2\n}").is_empty());
    }

    #[test]
    fn duplicate_labels_across_languages_all_reported() {
        let ds = diags("@py stats { 1 }\n@js stats { 1 }\nlet a = 1\nlet a = 2\n@c stats { int x; }");
        assert_eq!(ds, vec!["duplicate block label 'stats'", "'a' is already declared in this block", "duplicate block label 'stats'"]);
    }
}
