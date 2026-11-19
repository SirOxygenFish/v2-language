//! Scopes and name positions (INTERNALS 5.6): each scope that declares names gets a layout, and each
//! use of a name the frame and slot it reads. Every name V2 can declare is known here, so frames are
//! arrays. A slot can still be empty at run time (`let` hasn't run yet); then the interpreter walks
//! by name, which is what makes this pass invisible.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use super::ast::*;
use super::check::declarations;
use crate::runtime::builtins::BUILTINS;

/// The names a body declares directly, in the order a frame gets them: hoisted `func`s first.
fn declared(stmts: &[Stmt]) -> Vec<Rc<str>> {
    let funcs = stmts.iter().filter(|s| matches!(s.kind, StmtKind::Func(_)));
    let others = stmts.iter().filter(|s| !matches!(s.kind, StmtKind::Func(_)));
    funcs.chain(others).flat_map(|s| declarations(s).into_iter().map(|(name, _)| name)).collect()
}

/// A file's module frame.
pub fn module_layout(program: &[Stmt]) -> Layout {
    declared(program).into()
}

fn builtin(name: &str) -> Option<u32> {
    static INDEX: OnceLock<HashMap<&'static str, u32>> = OnceLock::new();
    INDEX.get_or_init(|| BUILTINS.iter().enumerate().map(|(i, b)| (b.name, i as u32)).collect()).get(name).copied()
}

/// Resolve a whole file, after its static checks.
pub fn resolve(program: &mut [Stmt]) {
    let mut r = Resolver { scopes: Vec::new(), holders: 0 };
    r.enter(declared(program), true);
    r.stmts(program);
}

struct Scope {
    names: HashMap<Rc<str>, u32>,
    /// Whether this scope is a frame at run time. A scope without one declares nothing.
    frame: bool,
}

struct Resolver {
    scopes: Vec<Scope>,
    /// Functions, engine blocks and imports seen so far: what could keep a frame (`Body.reusable`).
    holders: u32,
}

impl Resolver {
    fn enter(&mut self, names: Vec<Rc<str>>, frame: bool) -> Layout {
        let index = names.iter().enumerate().map(|(i, n)| (n.clone(), i as u32)).collect();
        self.scopes.push(Scope { names: index, frame });
        names.into()
    }

    fn leave(&mut self) {
        self.scopes.pop();
    }

    /// Where a use of `name` looks first: the innermost scope that declares it.
    fn find(&self, name: &str) -> Loc {
        let mut hops = 0;
        for scope in self.scopes.iter().rev() {
            if let Some(&index) = scope.names.get(name) {
                return Loc::Slot { hops, index };
            }
            hops += u32::from(scope.frame);
        }
        builtin(name).map_or(Loc::Unresolved, Loc::Builtin)
    }

    /// `name`'s slot in the scope that declares it, the innermost one.
    fn bind(&self, name: &mut Name) {
        if let Some(&index) = self.scopes.last().and_then(|s| s.names.get(&name.text)) {
            name.slot = index;
        }
    }

    fn stmts(&mut self, list: &mut [Stmt]) {
        list.iter_mut().for_each(|s| self.stmt(s));
    }

    /// A body in a frame of its own, unless it needs none (5.6).
    fn body(&mut self, b: &mut Body) {
        let names = declared(&b.stmts);
        let frame = !names.is_empty() || b.stmts.iter().any(creates_function);
        let layout = self.enter(names, frame);
        b.layout = frame.then_some(layout);
        let holders = self.holders;
        self.stmts(&mut b.stmts);
        b.reusable = self.holders == holders;
        self.leave();
    }

    /// A body whose frame also holds `first` (loop variables, the catch name).
    fn body_with(&mut self, b: &mut Body, first: Vec<&mut Name>) {
        let mut names: Vec<Rc<str>> = first.iter().map(|n| n.text.clone()).collect();
        names.extend(declared(&b.stmts));
        b.layout = Some(self.enter(names, true));
        for n in first {
            self.bind(n);
        }
        let holders = self.holders;
        self.stmts(&mut b.stmts);
        b.reusable = self.holders == holders;
        self.leave();
    }

    fn func(&mut self, f: &mut FuncDecl) {
        let mut names: Vec<Rc<str>> = f.params.iter().map(|(n, _)| n.text.clone()).chain(f.rest.iter().map(|r| r.text.clone())).collect();
        if let FuncBody::Block(b) = &f.body {
            names.extend(declared(&b.stmts));
        }
        f.layout = self.enter(names, true);
        for (n, default) in &mut f.params {
            self.bind(n);
            if let Some(d) = default {
                self.expr(d);
            }
        }
        if let Some(r) = &mut f.rest {
            self.bind(r);
        }
        match &mut f.body {
            FuncBody::Block(b) => self.stmts(&mut b.stmts),
            FuncBody::Expr(e) => self.expr(e),
        }
        self.leave();
    }

    fn block(&mut self, b: &mut Rc<EngineBlock>) {
        self.holders += 1;
        let b = Rc::get_mut(b).expect("nothing else holds a block during resolution");
        for (_, e) in &mut b.args {
            self.expr(e);
        }
    }

    fn stmt(&mut self, s: &mut Stmt) {
        match &mut s.kind {
            StmtKind::Let(n, value) => {
                self.bind(n);
                if let Some(e) = value {
                    self.expr(e);
                }
            }
            StmtKind::Const(n, e) => {
                self.bind(n);
                self.expr(e);
            }
            StmtKind::Expr(e) | StmtKind::Throw(e) | StmtKind::Return(Some(e)) => self.expr(e),
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
            StmtKind::EngineImport(_) | StmtKind::Extern(_) => self.holders += 1,
            StmtKind::Assign { target, value, .. } => {
                match target {
                    Target::Name(n, loc) => *loc = self.find(&n.text),
                    Target::Member(o, _) => self.expr(o),
                    Target::Index(o, i, _) => {
                        self.expr(o);
                        self.expr(i);
                    }
                }
                self.expr(value);
            }
            StmtKind::If { arms, else_ } => {
                for (cond, body) in arms {
                    self.expr(cond);
                    self.body(body);
                }
                if let Some(b) = else_ {
                    self.body(b);
                }
            }
            StmtKind::While { cond, body } => {
                self.expr(cond);
                self.body(body);
            }
            StmtKind::For { first, second, iter, body } => {
                match iter {
                    ForIter::Value(e) => self.expr(e),
                    ForIter::Range(a, b) => {
                        self.expr(a);
                        self.expr(b);
                    }
                }
                self.body_with(body, std::iter::once(first).chain(second.as_mut()).collect());
            }
            StmtKind::Try { body, catch, finally } => {
                self.body(body);
                if let Some((name, b)) = catch {
                    self.body_with(b, name.iter_mut().collect());
                }
                if let Some(b) = finally {
                    self.body(b);
                }
            }
            StmtKind::Func(f) => {
                self.holders += 1;
                let f = Rc::get_mut(f).expect("nothing else holds a function during resolution");
                if let Some(n) = &mut f.name {
                    self.bind(n);
                }
                self.func(f);
            }
            StmtKind::Import { alias, .. } => {
                self.holders += 1;
                self.bind(alias);
            }
            StmtKind::Block(b) => self.block(b),
        }
    }

    fn expr(&mut self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Null | ExprKind::Bool(_) | ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bytes(_) => {}
            ExprKind::Name(n, loc) => *loc = self.find(n),
            ExprKind::FStr(parts) => {
                for p in parts {
                    if let FPart::Expr { expr, .. } = p {
                        self.expr(expr);
                    }
                }
            }
            ExprKind::List(items) => items.iter_mut().for_each(|x| self.expr(x)),
            ExprKind::Dict(entries) => {
                for (k, v) in entries {
                    if let DictKey::Computed(k) = k {
                        self.expr(k);
                    }
                    self.expr(v);
                }
            }
            ExprKind::Unary(_, _, x) | ExprKind::OptChain(x) | ExprKind::Member { obj: x, .. } => self.expr(x),
            ExprKind::Binary(_, _, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Ternary(c, a, b) => {
                self.expr(c);
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Call { callee, args, .. } => {
                self.expr(callee);
                for a in args {
                    match a {
                        Arg::Pos(x) | Arg::Named(_, x) | Arg::Spread(x) => self.expr(x),
                    }
                }
            }
            ExprKind::Index { obj, index, .. } => {
                self.expr(obj);
                self.expr(index);
            }
            ExprKind::Slice { obj, lo, hi, .. } => {
                self.expr(obj);
                for x in [lo, hi].into_iter().flatten() {
                    self.expr(x);
                }
            }
            ExprKind::Func(f) => {
                self.holders += 1;
                self.func(Rc::get_mut(f).expect("nothing else holds a function during resolution"));
            }
            ExprKind::Block(b) => self.block(b),
        }
    }
}

/// A statement that makes a closure over the body it sits in, not counting bodies nested in it,
/// which have their own frames (5.6's cycle trimming goes by frame).
fn creates_function(s: &Stmt) -> bool {
    let block = |b: &EngineBlock| b.args.iter().any(|(_, e)| has_function(e));
    match &s.kind {
        StmtKind::Let(_, Some(e)) | StmtKind::Const(_, e) | StmtKind::Expr(e) | StmtKind::Throw(e) | StmtKind::Return(Some(e)) => has_function(e),
        StmtKind::Assign { target, value, .. } => {
            let t = match target {
                Target::Name(..) => false,
                Target::Member(o, _) => has_function(o),
                Target::Index(o, i, _) => has_function(o) || has_function(i),
            };
            t || has_function(value)
        }
        StmtKind::If { arms, .. } => arms.iter().any(|(cond, _)| has_function(cond)),
        StmtKind::While { cond, .. } => has_function(cond),
        StmtKind::For { iter: ForIter::Value(e), .. } => has_function(e),
        StmtKind::For { iter: ForIter::Range(a, b), .. } => has_function(a) || has_function(b),
        StmtKind::Func(_) => true,
        StmtKind::Block(b) => block(b),
        _ => false,
    }
}

fn has_function(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Func(_) => true,
        ExprKind::Null | ExprKind::Bool(_) | ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bytes(_) | ExprKind::Name(..) => false,
        ExprKind::FStr(parts) => parts.iter().any(|p| matches!(p, FPart::Expr { expr, .. } if has_function(expr))),
        ExprKind::List(items) => items.iter().any(has_function),
        ExprKind::Dict(entries) => entries.iter().any(|(k, v)| matches!(k, DictKey::Computed(k) if has_function(k)) || has_function(v)),
        ExprKind::Unary(_, _, x) | ExprKind::OptChain(x) | ExprKind::Member { obj: x, .. } => has_function(x),
        ExprKind::Binary(_, _, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) => has_function(a) || has_function(b),
        ExprKind::Ternary(c, a, b) => has_function(c) || has_function(a) || has_function(b),
        ExprKind::Call { callee, args, .. } => {
            has_function(callee) || args.iter().any(|a| matches!(a, Arg::Pos(x) | Arg::Named(_, x) | Arg::Spread(x) if has_function(x)))
        }
        ExprKind::Index { obj, index, .. } => has_function(obj) || has_function(index),
        ExprKind::Slice { obj, lo, hi, .. } => has_function(obj) || [lo, hi].into_iter().flatten().any(|x| has_function(x)),
        ExprKind::Block(b) => b.args.iter().any(|(_, x)| has_function(x)),
    }
}
