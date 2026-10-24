//! Statement and expression evaluation (INTERNALS 5.5-5.9, ch. 6).
//!
//! `exec`/`eval` take the environment as an argument, never interpreter state, so a child frame
//! is a local `Env` dropped on every exit path (5.8). The only interpreter-level stack is
//! `calls`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use indexmap::IndexMap;

use super::builtins::{Out, BUILTINS, POSITIONAL_ONLY};
use super::call::{bind, bind_spec, Named, Param};
use super::env::{self, declare, lookup, new_frame, AssignError, Env};
use super::error::{fail, type_error, with_suggestion, ErrorValue, Exec, Flow};
use super::methods::{self, index_error, key_error, normalize};
use super::module::{self, Parsed};
use super::ops::{self, MAX_VALUE_DEPTH};
use super::strings::CharCache;
use super::value::{bytes_repr, float_str, quoted, Callable, Value};
use crate::engines::handles::HandleRef;
use crate::engines::process::stop_if_interrupted;
use crate::engines::registry::Registry;
use crate::engines::session::Session;
use crate::engines::units::UnitState;
use crate::syntax::ast::*;
use crate::syntax::span::{FileId, SourceMap, Span};

/// Past this many nested calls a call raises `RecursionError` (5.9).
const MAX_CALLS: usize = 10_000;

/// What `r.name(args)` calls: a function held under a dict key, a method (5.5), or a handle's
/// method in one round trip (11.3).
enum MethodTarget {
    Func(Value),
    Method(&'static methods::MethodDef),
    Handle(Rc<HandleRef>),
}

/// One active call of a V2 function, or a module import (nameless) (5.8).
struct CallRecord {
    name: Option<Rc<str>>,
    call_site: Span,
}

/// A module's state for the run (6.4): its top level runs at most once.
enum ModuleState {
    Running,
    Done(Value),
    Failed(Rc<ErrorValue>),
}

pub struct Interp {
    pub sm: SourceMap,
    /// The engines of this run (7.1).
    pub registry: Registry,
    /// The worker sessions, by `SessionId` (8.1).
    pub sessions: Vec<Session>,
    /// Cross-language crossings in progress (9.7).
    pub xdepth: u32,
    /// Definition units by block (12.1).
    pub units: HashMap<UnitId, UnitState>,
    pub out: Out,
    pub chars: CharCache,
    calls: Vec<CallRecord>,
    builtins: Env,
    /// The script's arguments, for `args()` (5.10).
    pub args: Vec<String>,
    /// Files parsed at startup and not imported yet (6.1 step 2).
    parsed: HashMap<PathBuf, Parsed>,
    modules: HashMap<PathBuf, ModuleState>,
    import_stack: Vec<PathBuf>,
    /// Address of a local at the start of the interpreter thread, and how many bytes below it
    /// the stack guard allows (5.9).
    stack_base: usize,
    stack_limit: usize,
}

/// The address of a local in the caller's frame.
#[inline(never)]
fn stack_here() -> usize {
    let marker = 0u8;
    std::hint::black_box(&marker) as *const u8 as usize
}

impl Interp {
    /// `parsed` holds the files the entry file's imports name (6.1 step 2); `stack_size` is the
    /// size of the thread this runs on, and the guard trips at 7/8 of it.
    pub fn new(sm: SourceMap, parsed: HashMap<PathBuf, Parsed>, stack_size: usize, registry: Registry) -> Interp {
        let builtins = new_frame(None);
        for def in BUILTINS {
            declare(&builtins, def.name.into(), Value::func(Callable::Builtin(def)), false);
        }
        Interp {
            sm,
            registry,
            sessions: Vec::new(),
            xdepth: 0,
            units: HashMap::new(),
            out: Out::new(),
            chars: CharCache::default(),
            calls: Vec::new(),
            builtins,
            args: Vec::new(),
            parsed,
            modules: HashMap::new(),
            import_stack: Vec::new(),
            stack_base: stack_here(),
            stack_limit: stack_size / 8 * 7,
        }
    }

    /// Run the entry file's top level (6.1 step 3); it counts as a running module, so importing
    /// it is a circular import.
    pub fn run(&mut self, file: FileId, program: &[Stmt]) -> Exec<()> {
        let path = self.sm.file(file).path.clone();
        self.modules.insert(path.clone(), ModuleState::Running);
        self.import_stack.push(path);
        self.run_top_level(program).map(|_| ())
    }

    /// A file's top level in a new module frame whose parent is the builtins frame.
    fn run_top_level(&mut self, program: &[Stmt]) -> Exec<Env> {
        let module = new_frame(Some(&self.builtins));
        self.hoist(program.iter(), &module);
        for s in program {
            self.exec(s, &module)?;
        }
        Ok(module)
    }

    /// `import "spec" as name` (6.4): the module's namespace dict.
    fn import(&mut self, spec: &str, at: Span) -> Exec<Value> {
        let path = match module::locate(&self.sm, at.file, spec) {
            Ok(p) => p,
            Err(msg) => return self.fail_at("ImportError", msg, at),
        };
        match self.modules.get(&path) {
            Some(ModuleState::Done(ns)) => return Ok(ns.clone()),
            // A module's top level never runs twice: the same error again (it keeps its
            // location and trace).
            Some(ModuleState::Failed(e)) => return Err(Flow::Error(e.clone())),
            Some(ModuleState::Running) => {
                let first = self.import_stack.iter().position(|p| *p == path).unwrap_or(0);
                let chain: Vec<String> = self.import_stack[first..]
                    .iter()
                    .chain(std::iter::once(&path))
                    .map(|p| module::shown(p).to_string())
                    .collect();
                return self.fail_at("ImportError", format!("circular import: {}", chain.join(" -> ")), at);
            }
            None => {}
        }
        let parsed = match self.parsed.remove(&path) {
            Some(p) => p,
            // Not there at startup, there now: parse it now (6.4 step 1).
            None => match module::parse_file(&mut self.sm, &path) {
                Ok(p) => p,
                Err(e) => {
                    let msg = match e {
                        crate::syntax::span::LoadError::Io(m) => m,
                        crate::syntax::span::LoadError::NotUtf8 { .. } => "file is not valid UTF-8".into(),
                    };
                    return self.fail_at("ImportError", format!("cannot import '{spec}': {msg}"), at);
                }
            },
        };
        let program = match parsed {
            Parsed::Ok { program, .. } => program,
            Parsed::Failed(diags) => {
                let d = &diags[0];
                let msg = format!("cannot import '{spec}': {}:{}:{}: SyntaxError: {}", d.display, d.line, d.col, d.message);
                let data = diags
                    .iter()
                    .map(|d| {
                        let mut m = IndexMap::new();
                        m.insert(Rc::from("file"), Value::Str(d.display.clone()));
                        m.insert(Rc::from("line"), Value::Int(d.line as i64));
                        m.insert(Rc::from("col"), Value::Int(d.col as i64));
                        m.insert(Rc::from("message"), Value::str(&d.message));
                        Value::dict(m)
                    })
                    .collect();
                let e = Rc::new(ErrorValue::with_data("ImportError".into(), msg.into(), Value::list(data)));
                let flow = self.raise(Flow::Error(e.clone()), at);
                self.modules.insert(path, ModuleState::Failed(e));
                return Err(flow);
            }
        };
        if self.calls.len() >= MAX_CALLS {
            return self.fail_at("RecursionError", format!("maximum call depth ({MAX_CALLS}) exceeded"), at);
        }
        self.guard(at)?;
        self.modules.insert(path.clone(), ModuleState::Running);
        self.import_stack.push(path.clone());
        self.calls.push(CallRecord { name: None, call_site: at });
        let r = self.run_top_level(&program);
        self.calls.pop();
        self.import_stack.pop();
        match r {
            Ok(frame) => {
                // The namespace: a snapshot of the module's bindings, without `_` names (6.4 step 3).
                let ns: IndexMap<Rc<str>, Value> = frame
                    .borrow()
                    .vars
                    .iter()
                    .filter(|(k, _)| !k.starts_with('_'))
                    .map(|(k, slot)| (k.clone(), slot.value.clone()))
                    .collect();
                let ns = Value::dict(ns);
                self.modules.insert(path, ModuleState::Done(ns.clone()));
                env::leave(frame);
                Ok(ns)
            }
            Err(Flow::Error(e)) => {
                self.modules.insert(path, ModuleState::Failed(e.clone()));
                Err(Flow::Error(e))
            }
            Err(other) => Err(other),
        }
    }

    // -- errors ---------------------------------------------------------------------------------

    /// Fill in the location and trace of an error the first time it is raised (5.9).
    pub fn raise(&self, flow: Flow, span: Span) -> Flow {
        if let Flow::Error(e) = &flow {
            if e.span.get().is_none() {
                let _ = e.span.set(span);
                let _ = e.trace.set(self.trace_at(span));
            }
        }
        flow
    }

    fn at<T>(&self, r: Exec<T>, span: Span) -> Exec<T> {
        r.map_err(|f| self.raise(f, span))
    }

    fn fail_at<T>(&self, kind: &str, message: impl Into<Rc<str>>, span: Span) -> Exec<T> {
        self.at(fail(kind, message), span)
    }

    /// Innermost first: the raise location, then each call's site (5.9, 2.5).
    fn trace_at(&self, span: Span) -> Rc<[Rc<str>]> {
        let mut out: Vec<Rc<str>> = Vec::with_capacity(self.calls.len() + 1);
        let mut loc = span;
        for rec in self.calls.iter().rev() {
            out.push(self.frame_str(rec.name.as_deref(), loc));
            loc = rec.call_site;
        }
        out.push(self.frame_str(None, loc));
        out.into()
    }

    fn frame_str(&self, name: Option<&str>, loc: Span) -> Rc<str> {
        let display = &self.sm.file(loc.file).display;
        match name {
            Some(n) => format!("{n} ({display}:{})", loc.line).into(),
            None => format!("{display}:{}", loc.line).into(),
        }
    }

    /// The stack guard: deep expressions and deep recursion stop before the thread's stack
    /// does (5.9).
    fn guard(&self, span: Span) -> Exec<()> {
        if self.stack_base.saturating_sub(stack_here()) > self.stack_limit {
            return self.fail_at("RecursionError", format!("maximum call depth ({MAX_CALLS}) exceeded"), span);
        }
        Ok(())
    }

    /// Temporary until the roadmap milestone named in `what` lands.
    pub(crate) fn not_yet(&self, what: &str, span: Span) -> Flow {
        let msg = format!("{what} doesn't run yet in this build");
        self.raise(Flow::Error(Rc::new(ErrorValue::new("Error", msg))), span)
    }

    // -- statements -------------------------------------------------------------------------------

    /// Turn each `func` statement into a closure over `env` before the body runs (5.6).
    fn hoist<'s>(&mut self, stmts: impl Iterator<Item = &'s Stmt>, env: &Env) {
        for s in stmts {
            if let StmtKind::Func(decl) = &s.kind {
                let name = decl.name.as_ref().expect("a func statement has a name").text.clone();
                let f = self.closure(decl, env);
                declare(env, name, f, false);
            }
        }
    }

    fn closure(&mut self, decl: &Rc<FuncDecl>, env: &Env) -> Value {
        let name = match &decl.name {
            Some(n) => n.text.clone(),
            None => format!("func@{}:{}", self.sm.file(decl.span.file).display, decl.span.line).into(),
        };
        env.borrow_mut().closures += 1;
        Value::func(Callable::Closure { decl: decl.clone(), env: env.clone(), name })
    }

    /// A body in a new frame whose parent is `parent`.
    fn exec_body(&mut self, body: &Body, parent: &Env) -> Exec<()> {
        self.guard(body.span)?;
        let frame = new_frame(Some(parent));
        let r = self.exec_in(body, &frame);
        env::leave(frame);
        r
    }

    /// A body directly in `frame` (a call frame, a loop iteration's frame, a catch frame).
    fn exec_in(&mut self, body: &Body, frame: &Env) -> Exec<()> {
        self.hoist(body.funcs.iter().map(|&i| &body.stmts[i]), frame);
        for s in &body.stmts {
            self.exec(s, frame)?;
        }
        Ok(())
    }

    fn exec(&mut self, s: &Stmt, env: &Env) -> Exec<()> {
        match &s.kind {
            StmtKind::Let(name, value) => {
                let v = match value {
                    Some(e) => self.eval(e, env)?,
                    None => Value::Null,
                };
                declare(env, name.text.clone(), v, false);
            }
            StmtKind::Const(name, e) => {
                let v = self.eval(e, env)?;
                declare(env, name.text.clone(), v, true);
            }
            StmtKind::Expr(e) => {
                self.eval(e, env)?;
            }
            StmtKind::Break => return Err(Flow::Break),
            StmtKind::Continue => return Err(Flow::Continue),
            StmtKind::Return(e) => {
                let v = match e {
                    Some(e) => self.eval(e, env)?,
                    None => Value::Null,
                };
                return Err(Flow::Return(v));
            }
            StmtKind::Throw(e) => return Err(self.throw(e, s.span, env)),
            StmtKind::Assign { target, op, value } => self.assign(target, *op, value, env)?,
            StmtKind::If { arms, else_ } => {
                for (cond, body) in arms {
                    if self.eval(cond, env)?.truthy() {
                        return self.exec_body(body, env);
                    }
                }
                if let Some(body) = else_ {
                    return self.exec_body(body, env);
                }
            }
            StmtKind::While { cond, body } => {
                while self.eval(cond, env)?.truthy() {
                    match self.exec_body(body, env) {
                        Ok(()) | Err(Flow::Continue) => {}
                        Err(Flow::Break) => break,
                        Err(other) => return Err(other),
                    }
                }
            }
            StmtKind::For { first, second, iter, body } => self.exec_for(first, second.as_ref(), iter, body, env)?,
            StmtKind::Try { body, catch, finally } => self.exec_try(body, catch.as_ref(), finally.as_ref(), env)?,
            StmtKind::Func(_) => {} // hoisted on entry to its body
            StmtKind::Import { path, alias } => {
                let ns = self.import(path, s.span)?;
                declare(env, alias.text.clone(), ns, false);
            }
            StmtKind::EngineImport(imp) => return Err(self.not_yet("@import (roadmap phase 1)", imp.span)),
            StmtKind::Extern(_) => return Err(self.not_yet("extern (roadmap phase 2)", Span { hi: s.span.lo + 6, ..s.span })),
            StmtKind::Block(b) => {
                self.exec_block(b, env, false)?;
            }
        }
        Ok(())
    }

    /// `throw v`: an error as is, a str as `error("Error", v)`; located at the keyword unless
    /// already raised (5.9).
    fn throw(&mut self, e: &Expr, stmt: Span, env: &Env) -> Flow {
        let keyword = Span { hi: stmt.lo + 5, ..stmt };
        let v = match self.eval(e, env) {
            Ok(v) => v,
            Err(flow) => return flow,
        };
        let err = match v {
            Value::Error(err) => err,
            Value::Str(m) => Rc::new(ErrorValue::new("Error", m)),
            other => {
                let msg = format!("throw expects an error or a str, got {}", other.type_name());
                Rc::new(ErrorValue::new("TypeError", msg))
            }
        };
        self.raise(Flow::Error(err), keyword)
    }

    fn not_declared<T>(&self, name: &str, span: Span, env: &Env, assigning: bool) -> Exec<T> {
        let names = env::visible_names(env);
        let base = format!("'{name}' is not declared");
        let msg = match (super::error::did_you_mean(name, names.iter().map(|n| &**n)), assigning) {
            (Some(s), false) => format!("{base} — did you mean '{s}'?"),
            (Some(s), true) => format!("{base} — did you mean '{s}'? (or declare it with let)"),
            (None, false) => base,
            (None, true) => format!("{base} — declare it with let"),
        };
        self.fail_at("NameError", msg, span)
    }

    /// `target = value` / `target op= value`: the target's container and key are evaluated once,
    /// before the value (5.3).
    fn assign(&mut self, target: &Target, op: Option<(BinOp, Span)>, value: &Expr, env: &Env) -> Exec<()> {
        match target {
            Target::Name(n) => {
                let new = match op {
                    None => self.eval(value, env)?,
                    Some((op, op_span)) => {
                        let Some(old) = lookup(env, &n.text) else { return self.not_declared(&n.text, n.span, env, true) };
                        let rhs = self.eval(value, env)?;
                        self.at(ops::binop(op, &old, &rhs), op_span)?
                    }
                };
                match env::assign(env, &n.text, new) {
                    Ok(()) => Ok(()),
                    Err(AssignError::NotDeclared) => self.not_declared(&n.text, n.span, env, true),
                    Err(AssignError::Const) => self.fail_at("TypeError", format!("cannot assign to constant '{}'", n.text), n.span),
                    Err(AssignError::Builtin) => {
                        self.fail_at("TypeError", format!("cannot assign to builtin '{}' — shadow it with let", n.text), n.span)
                    }
                }
            }
            Target::Member(obj, name) => {
                let o = self.eval(obj, env)?;
                let new = match op {
                    None => self.eval(value, env)?,
                    Some((op, op_span)) => {
                        let old = self.member(&o, name);
                        let old = self.at(old, name.span)?;
                        let rhs = self.eval(value, env)?;
                        self.at(ops::binop(op, &old, &rhs), op_span)?
                    }
                };
                let r = self.set_member(&o, name, new);
                self.at(r, name.span)
            }
            Target::Index(obj, index, bracket) => {
                let o = self.eval(obj, env)?;
                let k = self.eval(index, env)?;
                let new = match op {
                    None => self.eval(value, env)?,
                    Some((op, op_span)) => {
                        let old = self.index(&o, &k, *bracket);
                        let old = self.at(old, *bracket)?;
                        let rhs = self.eval(value, env)?;
                        self.at(ops::binop(op, &old, &rhs), op_span)?
                    }
                };
                let r = self.set_index(&o, &k, new, *bracket);
                self.at(r, *bracket)
            }
        }
    }

    /// One loop iteration in a fresh frame holding the loop variables (5.6). `Ok(false)` ends
    /// the loop.
    fn iteration(&mut self, body: &Body, env: &Env, vars: &[(&Name, Value)]) -> Exec<bool> {
        let frame = new_frame(Some(env));
        for (n, v) in vars {
            declare(&frame, n.text.clone(), v.clone(), false);
        }
        let r = self.exec_in(body, &frame);
        env::leave(frame);
        match r {
            Ok(()) | Err(Flow::Continue) => Ok(true),
            Err(Flow::Break) => Ok(false),
            Err(other) => Err(other),
        }
    }

    /// `for` evaluates its iterable once and walks a snapshot of it (5.8).
    fn exec_for(&mut self, first: &Name, second: Option<&Name>, iter: &ForIter, body: &Body, env: &Env) -> Exec<()> {
        // With two variables, the first gets the index (the key for a dict).
        let pair = |a: Value, b: Value| -> Vec<(&Name, Value)> {
            match second {
                Some(s) => vec![(first, a), (s, b)],
                None => vec![(first, b)],
            }
        };
        match iter {
            ForIter::Range(a, b) => {
                let lo = self.eval(a, env)?;
                let hi = self.eval(b, env)?;
                let (lo, hi) = match (&lo, &hi) {
                    (Value::Int(x), Value::Int(y)) => (*x, *y),
                    _ => {
                        let (bad, at) = if matches!(lo, Value::Int(_)) { (&hi, b.span) } else { (&lo, a.span) };
                        return self.fail_at("TypeError", format!("a range needs int bounds, got {}", bad.type_name()), at);
                    }
                };
                let mut i = lo;
                while i < hi {
                    if !self.iteration(body, env, &[(first, Value::Int(i))])? {
                        break;
                    }
                    i += 1;
                }
            }
            ForIter::Value(e) => match self.eval(e, env)? {
                Value::List(l) => {
                    let items = l.borrow().clone();
                    for (i, x) in items.into_iter().enumerate() {
                        if !self.iteration(body, env, &pair(Value::Int(i as i64), x))? {
                            break;
                        }
                    }
                }
                Value::Dict(d) => {
                    let entries: Vec<(Rc<str>, Value)> = d.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    for (k, v) in entries {
                        let vars = match second {
                            Some(s) => vec![(first, Value::Str(k)), (s, v)],
                            None => vec![(first, Value::Str(k))],
                        };
                        if !self.iteration(body, env, &vars)? {
                            break;
                        }
                    }
                }
                Value::Str(s) => {
                    for (i, ch) in s.chars().enumerate() {
                        let mut buf = [0u8; 4];
                        if !self.iteration(body, env, &pair(Value::Int(i as i64), Value::str(ch.encode_utf8(&mut buf))))? {
                            break;
                        }
                    }
                }
                Value::Bytes(b) => {
                    for (i, byte) in b.iter().enumerate() {
                        if !self.iteration(body, env, &pair(Value::Int(i as i64), Value::Int(*byte as i64)))? {
                            break;
                        }
                    }
                }
                Value::Handle(h) => self.for_handle(&h, first, second, body, env, e.span)?,
                other => return self.fail_at("TypeError", format!("cannot iterate over {}", other.type_name()), e.span),
            },
        }
        Ok(())
    }

    /// `for` over a handle (11.4): batches of items from the worker's iterator. A mapping gives
    /// `[key, value]` pairs, so one variable is the key and two are key and value, as for a
    /// dict; anything else gives items, or index and item, as for a list.
    fn for_handle(&mut self, h: &HandleRef, first: &Name, second: Option<&Name>, body: &Body, env: &Env, at: Span) -> Exec<()> {
        let (it, mapping) = self.handle_iter(h, at)?;
        let mut index = 0;
        loop {
            let (items, done) = self.handle_next(&it, at)?;
            for item in items {
                let vars = if mapping {
                    let (k, v) = match &item {
                        Value::List(pair) if pair.borrow().len() == 2 => {
                            let pair = pair.borrow();
                            (pair[0].clone(), pair[1].clone())
                        }
                        _ => return self.fail_at("ProtocolError", format!("engine '{}' sent a mapping item that is not a pair", h.tag), at),
                    };
                    match second {
                        Some(s) => vec![(first, k), (s, v)],
                        None => vec![(first, k)],
                    }
                } else {
                    match second {
                        Some(s) => vec![(first, Value::Int(index)), (s, item)],
                        None => vec![(first, item)],
                    }
                };
                index += 1;
                if !self.iteration(body, env, &vars)? {
                    return Ok(());
                }
            }
            if done {
                return Ok(());
            }
        }
    }

    /// `try`/`catch`/`finally` (5.8): `catch` sees errors only; `finally` runs after every
    /// outcome and replaces it only if it does not complete normally.
    fn exec_try(&mut self, body: &Body, catch: Option<&(Option<Name>, Body)>, finally: Option<&Body>, env: &Env) -> Exec<()> {
        let mut outcome = self.exec_body(body, env);
        // After an interrupt no `catch` or `finally` runs (6.5).
        stop_if_interrupted();
        if let (Err(Flow::Error(e)), Some((name, cbody))) = (&outcome, catch) {
            let e = e.clone();
            let frame = new_frame(Some(env));
            if let Some(n) = name {
                declare(&frame, n.text.clone(), Value::Error(e), false);
            }
            outcome = self.exec_in(cbody, &frame);
            env::leave(frame);
        }
        stop_if_interrupted();
        if let Some(f) = finally {
            if !matches!(outcome, Err(Flow::Exit(_))) {
                self.exec_body(f, env)?;
            }
        }
        outcome
    }

    // -- expressions ------------------------------------------------------------------------------

    pub fn eval(&mut self, e: &Expr, env: &Env) -> Exec<Value> {
        self.guard(e.span)?;
        Ok(match &e.kind {
            ExprKind::Null => Value::Null,
            ExprKind::Bool(b) => Value::Bool(*b),
            ExprKind::Int(n) => Value::Int(*n),
            ExprKind::Float(x) => Value::Float(*x),
            ExprKind::Str(s) => Value::Str(s.clone()),
            ExprKind::Bytes(b) => Value::Bytes(b.clone()),
            ExprKind::Name(n) => match lookup(env, n) {
                Some(v) => v,
                None => return self.not_declared(n, e.span, env, false),
            },
            ExprKind::FStr(parts) => Value::Str(self.fstring(parts, env)?.into()),
            ExprKind::List(items) => {
                let mut out = Vec::with_capacity(items.len());
                for x in items {
                    out.push(self.eval(x, env)?);
                }
                Value::list(out)
            }
            ExprKind::Dict(entries) => {
                let mut map = IndexMap::with_capacity(entries.len());
                for (k, x) in entries {
                    let key = match k {
                        DictKey::Str(s, _) => s.clone(),
                        DictKey::Computed(ke) => match self.eval(ke, env)? {
                            Value::Str(s) => s,
                            other => return self.fail_at("TypeError", format!("dict keys must be str, got {}", other.type_name()), ke.span),
                        },
                    };
                    let v = self.eval(x, env)?;
                    map.insert(key, v);
                }
                Value::dict(map)
            }
            ExprKind::Unary(op, span, x) => {
                let v = self.eval(x, env)?;
                match op {
                    UnOp::Not => Value::Bool(!v.truthy()),
                    UnOp::Neg => self.at(ops::neg(&v), *span)?,
                }
            }
            ExprKind::Binary(op, span, a, b) => {
                let x = self.eval(a, env)?;
                let y = self.eval(b, env)?;
                self.at(ops::binop(*op, &x, &y), *span)?
            }
            ExprKind::And(a, b) => {
                let x = self.eval(a, env)?;
                if x.truthy() { self.eval(b, env)? } else { x }
            }
            ExprKind::Or(a, b) => {
                let x = self.eval(a, env)?;
                if x.truthy() { x } else { self.eval(b, env)? }
            }
            ExprKind::Coalesce(a, b) => match self.eval(a, env)? {
                Value::Null => self.eval(b, env)?,
                x => x,
            },
            ExprKind::Ternary(c, a, b) => {
                if self.eval(c, env)?.truthy() {
                    self.eval(a, env)?
                } else {
                    self.eval(b, env)?
                }
            }
            ExprKind::Call { .. } | ExprKind::Index { .. } | ExprKind::Slice { .. } | ExprKind::Member { .. } => {
                self.chain(e, env)?.unwrap_or(Value::Null)
            }
            ExprKind::OptChain(x) => self.chain(x, env)?.unwrap_or(Value::Null),
            ExprKind::Func(decl) => self.closure(decl, env),
            ExprKind::Block(b) => self.exec_block(b, env, true)?,
        })
    }

    /// A postfix step. `None` means a `?.` met `null`: the whole chain is `null` and nothing
    /// after it is evaluated, arguments included (5.5).
    fn chain(&mut self, e: &Expr, env: &Env) -> Exec<Option<Value>> {
        match &e.kind {
            ExprKind::Member { obj, name, optional } => {
                let Some(o) = self.chain(obj, env)? else { return Ok(None) };
                if *optional && matches!(o, Value::Null) {
                    return Ok(None);
                }
                let r = self.member(&o, name);
                Ok(Some(self.at(r, name.span)?))
            }
            ExprKind::Index { obj, index, bracket } => {
                let Some(o) = self.chain(obj, env)? else { return Ok(None) };
                let k = self.eval(index, env)?;
                let r = self.index(&o, &k, *bracket);
                Ok(Some(self.at(r, *bracket)?))
            }
            ExprKind::Slice { obj, lo, hi, bracket } => {
                let Some(o) = self.chain(obj, env)? else { return Ok(None) };
                let lo = match lo {
                    Some(x) => Some(self.eval(x, env)?),
                    None => None,
                };
                let hi = match hi {
                    Some(x) => Some(self.eval(x, env)?),
                    None => None,
                };
                let r = self.slice(&o, lo.as_ref(), hi.as_ref());
                Ok(Some(self.at(r, *bracket)?))
            }
            ExprKind::Call { callee, args, paren } => {
                self.guard(*paren)?;
                if let ExprKind::Member { obj, name, optional } = &callee.kind {
                    let Some(o) = self.chain(obj, env)? else { return Ok(None) };
                    if *optional && matches!(o, Value::Null) {
                        return Ok(None);
                    }
                    let target = self.method_target(&o, name)?;
                    let (pos, named) = self.args(args, env)?;
                    return Ok(Some(self.call_method(&o, name, target, pos, named)?));
                }
                let Some(f) = self.chain(callee, env)? else { return Ok(None) };
                let (pos, named) = self.args(args, env)?;
                // The caret goes at the callee's name, else at the `(` (2.2).
                let at = if matches!(callee.kind, ExprKind::Name(_)) { callee.span } else { *paren };
                Ok(Some(self.call_value(&f, pos, named, at)?))
            }
            _ => Ok(Some(self.eval(e, env)?)),
        }
    }

    /// Arguments left to right; `...xs` must be a list and expands in place (5.7).
    fn args(&mut self, args: &[Arg], env: &Env) -> Exec<(Vec<Value>, Named)> {
        let mut pos = Vec::with_capacity(args.len());
        let mut named = Vec::new();
        for a in args {
            match a {
                Arg::Pos(x) => pos.push(self.eval(x, env)?),
                Arg::Named(n, x) => named.push((n.text.clone(), self.eval(x, env)?)),
                Arg::Spread(x) => match self.eval(x, env)? {
                    Value::List(l) => pos.extend(l.borrow().iter().cloned()),
                    other => {
                        return self.fail_at("TypeError", format!("spread argument must be a list, got {}", other.type_name()), x.span)
                    }
                },
            }
        }
        Ok((pos, named))
    }

    fn fstring(&mut self, parts: &[FPart], env: &Env) -> Exec<String> {
        let mut s = String::new();
        for p in parts {
            match p {
                FPart::Lit(text) => s.push_str(text),
                FPart::Expr { expr, fixed } => {
                    let v = self.eval(expr, env)?;
                    match fixed {
                        None => {
                            let text = self.str_of(&v, expr.span);
                            s.push_str(&self.at(text, expr.span)?);
                        }
                        Some(n) => {
                            let n = *n as usize;
                            let x = match v {
                                Value::Int(i) => i as f64,
                                Value::Float(x) => x,
                                other => {
                                    let msg = format!("the :.{n}f format needs a number, got {}", other.type_name());
                                    return self.fail_at("TypeError", msg, expr.span);
                                }
                            };
                            if x.is_finite() {
                                s.push_str(&format!("{x:.n$}"));
                            } else {
                                s.push_str(&float_str(x));
                            }
                        }
                    }
                }
            }
        }
        Ok(s)
    }

    // -- calls ------------------------------------------------------------------------------------

    /// Call any value with already evaluated arguments; errors without a location get
    /// `call_site` (5.7).
    pub fn call_value(&mut self, f: &Value, pos: Vec<Value>, named: Vec<(Rc<str>, Value)>, call_site: Span) -> Exec<Value> {
        if let Value::Handle(h) = f {
            return self.handle_call(h, None, pos, named, call_site);
        }
        let Value::Func(c) = f else {
            return self.fail_at("TypeError", format!("'{}' is not callable", f.type_name()), call_site);
        };
        match &**c {
            Callable::Builtin(def) => {
                if !named.is_empty() && POSITIONAL_ONLY.contains(&def.name) {
                    return self.fail_at("ArityError", format!("{}() takes positional arguments only", def.name), call_site);
                }
                let args = self.at(bind_spec(def.name, def.params, pos, named), call_site)?;
                let r = (def.f)(self, args, call_site);
                self.at(r, call_site)
            }
            Callable::Closure { decl, env, name } => {
                let params: Vec<Param> = decl
                    .params
                    .iter()
                    .map(|(n, d)| Param { name: &n.text, has_default: d.is_some(), rest: false })
                    .chain(decl.rest.iter().map(|r| Param { name: &r.text, has_default: false, rest: true }))
                    .collect();
                let bound = self.at(bind(name, &params, pos, named), call_site)?;
                if self.calls.len() >= MAX_CALLS {
                    return self.fail_at("RecursionError", format!("maximum call depth ({MAX_CALLS}) exceeded"), call_site);
                }
                self.guard(call_site)?;
                self.calls.push(CallRecord { name: Some(name.clone()), call_site });
                let r = self.run_closure(decl, env, bound);
                self.calls.pop();
                r
            }
        }
    }

    /// 5.7 step 4-5: the call frame, parameters (defaults evaluated in it), then the body.
    fn run_closure(&mut self, decl: &FuncDecl, env: &Env, bound: Vec<Option<Value>>) -> Exec<Value> {
        let frame = new_frame(Some(env));
        let r = self.run_closure_in(decl, &frame, bound);
        env::leave(frame);
        r
    }

    fn run_closure_in(&mut self, decl: &FuncDecl, frame: &Env, mut bound: Vec<Option<Value>>) -> Exec<Value> {
        for (i, (pname, default)) in decl.params.iter().enumerate() {
            let v = match (bound[i].take(), default) {
                (Some(v), _) => v,
                (None, Some(d)) => self.eval(d, frame)?,
                (None, None) => unreachable!("the binder checked required parameters"),
            };
            declare(frame, pname.text.clone(), v, false);
        }
        if let Some(r) = &decl.rest {
            let v = bound.last_mut().and_then(Option::take).unwrap_or_else(|| Value::list(Vec::new()));
            declare(frame, r.text.clone(), v, false);
        }
        match &decl.body {
            FuncBody::Expr(e) => self.eval(e, frame),
            FuncBody::Block(b) => match self.exec_in(b, frame) {
                Ok(()) => Ok(Value::Null),
                Err(Flow::Return(v)) => Ok(v),
                Err(other) => Err(other),
            },
        }
    }

    /// `r.name(args)` (5.5): a dict key holding a function, or a handle (a foreign callable got as
    /// a value), wins over the dict methods. The callee is resolved before the arguments run (5.7).
    fn method_target(&self, o: &Value, name: &Name) -> Exec<MethodTarget> {
        if let Value::Handle(h) = o {
            return Ok(MethodTarget::Handle(h.clone()));
        }
        if let Value::Dict(d) = o {
            let f = d.borrow().get(&name.text).cloned();
            if let Some(f @ (Value::Func(_) | Value::Handle(_))) = f {
                return Ok(MethodTarget::Func(f));
            }
        }
        let Some(table) = methods::table(o) else {
            let msg = match o {
                Value::Null => format!("cannot read '{}' of null (use ?.)", name.text),
                other => format!("{} has no method '{}'", other.type_name(), name.text),
            };
            return self.fail_at("TypeError", msg, name.span);
        };
        let Some(def) = table.iter().find(|m| *m.name == *name.text) else {
            let msg = format!("{} has no method '{}'", o.type_name(), name.text);
            let msg = with_suggestion(msg, &name.text, table.iter().map(|m| m.name));
            return self.fail_at("TypeError", msg, name.span);
        };
        Ok(MethodTarget::Method(def))
    }

    fn call_method(&mut self, o: &Value, name: &Name, target: MethodTarget, pos: Vec<Value>, named: Named) -> Exec<Value> {
        let def = match target {
            MethodTarget::Func(f) => return self.call_value(&f, pos, named, name.span),
            MethodTarget::Handle(h) => return self.handle_call(&h, Some(&name.text), pos, named, name.span),
            MethodTarget::Method(def) => def,
        };
        let fname = format!("{}.{}", o.type_name(), def.name);
        let args = self.at(bind_spec(&fname, def.params, pos, named), name.span)?;
        let r = (def.f)(self, o, args, name.span);
        self.at(r, name.span)
    }

    // -- members, indexing, slicing (5.4, 5.5) --------------------------------------------------

    fn member(&mut self, o: &Value, name: &Name) -> Exec<Value> {
        let n = &name.text;
        match o {
            Value::Handle(h) => self.handle_getattr(h, n, name.span),
            Value::Dict(d) => {
                let found = d.borrow().get(n).cloned();
                match found {
                    Some(v) => Ok(v),
                    None => key_error(n, d),
                }
            }
            Value::Error(e) => self.error_field(e, n),
            Value::Null => type_error(format!("cannot read '{n}' of null (use ?.)")),
            other => type_error(format!("{} has no field '{n}'", other.type_name())),
        }
    }

    /// The fields of an error value (5.9).
    fn error_field(&self, e: &ErrorValue, n: &str) -> Exec<Value> {
        let span = e.span.get();
        Ok(match n {
            "type" => Value::Str(e.kind.clone()),
            "message" => Value::Str(e.message.clone()),
            "file" => span.map_or(Value::Null, |s| Value::Str(self.sm.file(s.file).display.clone())),
            "line" => span.map_or(Value::Null, |s| Value::Int(s.line as i64)),
            "col" => span.map_or(Value::Null, |s| Value::Int(s.col as i64)),
            "trace" => Value::list(e.trace.get().map_or_else(Vec::new, |t| t.iter().map(|f| Value::Str(f.clone())).collect())),
            "data" => e.data.clone(),
            "lang" => e.lang.clone().map_or(Value::Null, Value::Str),
            "foreign_type" => e.foreign_type.clone().map_or(Value::Null, Value::Str),
            "cause" => e.cause.clone(),
            _ => {
                const FIELDS: [&str; 10] = ["type", "message", "file", "line", "col", "trace", "data", "lang", "foreign_type", "cause"];
                return type_error(with_suggestion(format!("error has no field '{n}'"), n, FIELDS));
            }
        })
    }

    fn set_member(&mut self, o: &Value, name: &Name, v: Value) -> Exec<()> {
        let n = &name.text;
        match o {
            Value::Handle(h) => self.handle_setattr(h, n, &v, name.span),
            Value::Dict(d) => {
                d.borrow_mut().insert(n.clone(), v);
                Ok(())
            }
            Value::Str(_) | Value::Bytes(_) => type_error(format!("{} is immutable", o.type_name())),
            Value::Error(_) => type_error("error values are read-only"),
            Value::Null => type_error(format!("cannot set '{n}' of null")),
            other => type_error(format!("{} has no field '{n}'", other.type_name())),
        }
    }

    fn index_of(&self, k: &Value, what: &str) -> Exec<i64> {
        match k {
            Value::Int(i) => Ok(*i),
            other => type_error(format!("{what} index must be an int, got {}", other.type_name())),
        }
    }

    fn index(&mut self, o: &Value, k: &Value, at: Span) -> Exec<Value> {
        match o {
            Value::Handle(h) => self.handle_getitem(h, k, at),
            Value::List(l) => {
                let i = self.index_of(k, "list")?;
                let l = l.borrow();
                match normalize(i, l.len()) {
                    Some(n) => Ok(l[n].clone()),
                    None => index_error("list", i, l.len()),
                }
            }
            Value::Str(s) => {
                let i = self.index_of(k, "string")?;
                let len = self.chars.len(s);
                match normalize(i, len) {
                    Some(n) => {
                        let (a, b) = (self.chars.byte_of(s, n), self.chars.byte_of(s, n + 1));
                        Ok(Value::str(&s[a..b]))
                    }
                    None => index_error("string", i, len),
                }
            }
            Value::Bytes(b) => {
                let i = self.index_of(k, "bytes")?;
                match normalize(i, b.len()) {
                    Some(n) => Ok(Value::Int(b[n] as i64)),
                    None => index_error("bytes", i, b.len()),
                }
            }
            Value::Dict(d) => match k {
                Value::Str(key) => {
                    let found = d.borrow().get(key).cloned();
                    match found {
                        Some(v) => Ok(v),
                        None => key_error(key, d),
                    }
                }
                other => type_error(format!("dict keys must be str, got {}", other.type_name())),
            },
            Value::Null => type_error("cannot index null"),
            other => type_error(format!("cannot index {}", other.type_name())),
        }
    }

    fn set_index(&mut self, o: &Value, k: &Value, v: Value, at: Span) -> Exec<()> {
        match o {
            Value::Handle(h) => self.handle_setitem(h, k, &v, at),
            Value::List(l) => {
                let i = self.index_of(k, "list")?;
                let mut l = l.borrow_mut();
                let len = l.len();
                match normalize(i, len) {
                    Some(n) => {
                        l[n] = v;
                        Ok(())
                    }
                    None => index_error("list", i, len),
                }
            }
            Value::Dict(d) => match k {
                Value::Str(key) => {
                    d.borrow_mut().insert(key.clone(), v);
                    Ok(())
                }
                other => type_error(format!("dict keys must be str, got {}", other.type_name())),
            },
            Value::Str(_) | Value::Bytes(_) => type_error(format!("{} is immutable", o.type_name())),
            Value::Error(_) => type_error("error values are read-only"),
            Value::Null => type_error("cannot index null"),
            other => type_error(format!("cannot index {}", other.type_name())),
        }
    }

    /// `x[a:b]` like Python: missing bounds are 0 and the length, negative bounds add the
    /// length, both clamp (5.4).
    fn slice(&mut self, o: &Value, lo: Option<&Value>, hi: Option<&Value>) -> Exec<Value> {
        let bound = |v: Option<&Value>| -> Exec<Option<i64>> {
            match v {
                None => Ok(None),
                Some(Value::Int(i)) => Ok(Some(*i)),
                Some(other) => type_error(format!("slice bounds must be int, got {}", other.type_name())),
            }
        };
        let (lo, hi) = (bound(lo)?, bound(hi)?);
        let range = |len: usize| -> (usize, usize) {
            let fix = |b: Option<i64>, default: usize| -> usize {
                match b {
                    None => default,
                    Some(i) if i < 0 => (i + len as i64).max(0) as usize,
                    Some(i) => (i as u64).min(len as u64) as usize,
                }
            };
            let (a, b) = (fix(lo, 0), fix(hi, len));
            (a, b.max(a))
        };
        match o {
            Value::List(l) => {
                let l = l.borrow();
                let (a, b) = range(l.len());
                Ok(Value::list(l[a..b].to_vec()))
            }
            Value::Bytes(bytes) => {
                let (a, b) = range(bytes.len());
                Ok(Value::Bytes(bytes[a..b].into()))
            }
            Value::Str(s) => {
                let (a, b) = range(self.chars.len(s));
                let (x, y) = (self.chars.byte_of(s, a), self.chars.byte_of(s, b));
                Ok(Value::str(&s[x..y]))
            }
            Value::Handle(_) => type_error(format!("cannot slice a handle{}", ops::HANDLE_HINT)),
            other => type_error(format!("cannot slice {}", other.type_name())),
        }
    }

    // -- str() (5.10) -----------------------------------------------------------------------------

    /// `str(v)`: a str unchanged, anything else in its canonical form.
    pub fn str_of(&mut self, v: &Value, at: Span) -> Exec<String> {
        if let Value::Str(s) = v {
            return Ok(s.to_string());
        }
        let mut out = String::new();
        self.repr(v, &mut out, &mut Vec::new(), at)?;
        Ok(out)
    }

    /// The canonical text of `v` inside a container; `path` holds the containers being printed,
    /// so a cycle prints as `[...]`/`{...}` (5.2). A handle asks its worker (`op str`, 11.3).
    fn repr(&mut self, v: &Value, out: &mut String, path: &mut Vec<usize>, at: Span) -> Exec<()> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Float(x) => out.push_str(&float_str(*x)),
        Value::Str(s) => quoted(s, out),
        Value::Bytes(b) => out.push_str(&bytes_repr(b)),
        Value::Handle(h) => out.push_str(&self.handle_str(h, at)?),
        Value::List(l) => {
            let id = l.addr();
            if path.contains(&id) {
                out.push_str("[...]");
                return Ok(());
            }
            if path.len() >= MAX_VALUE_DEPTH {
                return ops::too_deep();
            }
            // Clone the items out: no borrow is held while printing (5.11 borrow rule).
            let items = l.borrow().clone();
            path.push(id);
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                self.repr(x, out, path, at)?;
            }
            out.push(']');
            path.pop();
        }
        Value::Dict(d) => {
            let id = d.addr();
            if path.contains(&id) {
                out.push_str("{...}");
                return Ok(());
            }
            if path.len() >= MAX_VALUE_DEPTH {
                return ops::too_deep();
            }
            let entries: Vec<(Rc<str>, Value)> = d.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            path.push(id);
            out.push('{');
            for (i, (k, x)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                quoted(k, out);
                out.push_str(": ");
                self.repr(x, out, path, at)?;
            }
            out.push('}');
            path.pop();
        }
        Value::Func(c) => match &**c {
            Callable::Closure { name, .. } => out.push_str(&format!("<func {name}>")),
            Callable::Builtin(def) => out.push_str(&format!("<builtin {}>", def.name)),
        },
        Value::Error(e) => out.push_str(&format!("{}: {}", super::error::kind_text(e), e.message)),
    }
    Ok(())
    }
}
