//! Import resolution and routing (INTERNALS ch. 13): what selectors see of a file (6.2), resolving
//! names (13.3), binding at V2 level (13.4), module units (13.5), routes (13.6) and the `imports`
//! entries of a block's `load` (13.7).

use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use serde_json::{json, Value as Json};

use super::session::SessionId;
use super::units::{wire_unit, Export, UnitState, Waiting};
use crate::runtime::env::{declare, own_value, Env};
use crate::runtime::error::{fail, ErrorValue, Exec, Flow};
use crate::runtime::interp::Interp;
use crate::runtime::value::{Callable, Func, Value};
use crate::syntax::ast::*;
use crate::syntax::span::{absolute_clean, FileId, Span};

/// The blocks a waiting `@import`'s statement saw, for what it finds later (12.7).
type Seen<'a> = Option<&'a [UnitId]>;

/// What import resolution needs of one file (6.2): its module frame and its blocks.
pub struct FileScope {
    pub frame: Env,
    /// The definition blocks, in source order, reached or not (13.3's candidates).
    blocks: Vec<Rc<EngineBlock>>,
    /// Every block, by body line, for mapping foreign frames (2.3, 12.6).
    pub bodies: Vec<Rc<EngineBlock>>,
    /// The names declared at the top level, for the `v2` selector (13.7).
    top: HashSet<Rc<str>>,
    /// The definition blocks that may wait for their language (12.7).
    waiting: HashSet<UnitId>,
    /// The `@import` statements that may, by span start (13.4).
    eligible: HashSet<u32>,
}

impl FileScope {
    /// `entry`: this is the file the run started with, so no other file can see its names (13.4).
    pub fn new(program: &[Stmt], frame: Env, entry: bool) -> FileScope {
        let mut walk = Walk::default();
        stmts(program, &mut walk);
        let mut bodies = std::mem::take(&mut walk.bodies);
        bodies.sort_by_key(|b| b.unit_id.offset);
        let blocks = bodies.iter().filter(|b| b.directives.exports.is_some()).cloned().collect();
        let mut top = HashSet::new();
        let mut waiting = HashSet::new();
        let mut eligible = HashSet::new();
        for s in program {
            match &s.kind {
                StmtKind::Block(b) if may_wait(b) => {
                    waiting.insert(b.unit_id);
                }
                StmtKind::EngineImport(imp) if walk.import_may_wait(imp, entry) => {
                    eligible.insert(imp.span.lo);
                }
                _ => {}
            }
            top.extend(crate::syntax::check::declarations(s).into_iter().map(|(n, _)| n));
        }
        FileScope { frame, blocks, bodies, top, waiting, eligible }
    }

    pub(crate) fn block_may_wait(&self, id: UnitId) -> bool {
        self.waiting.contains(&id)
    }
}

/// A direct statement of its file with no args and no inner `from v2`: its load depends on no V2
/// state at its position (12.7).
fn may_wait(b: &EngineBlock) -> bool {
    b.directives.exports.is_some() && b.args.is_empty() && !b.directives.imports.iter().any(|i| matches!(i.selector, Selector::V2 { .. }))
}

/// What the walk over a file's tree collects: every block, and how each name is used, by text
/// (13.4). Ignoring shadowing only makes an `@import` run at its statement.
#[derive(Default)]
struct Walk {
    bodies: Vec<Rc<EngineBlock>>,
    /// Names used as anything but the callee of a direct call.
    not_called: HashSet<Rc<str>>,
    /// Names used as anything but the receiver of a plain member read or method call.
    not_member: HashSet<Rc<str>>,
}

impl Walk {
    fn other_use(&mut self, name: &Rc<str>) {
        self.not_called.insert(name.clone());
        self.not_member.insert(name.clone());
    }

    /// 13.4's rule for one `@import`, given that it is a direct statement of its file.
    fn import_may_wait(&self, imp: &EngineImport, entry: bool) -> bool {
        let hidden = |local: &str| entry || local.starts_with('_');
        match &imp.items {
            ImportItems::Names(items) => items.iter().all(|i| {
                let local = i.alias.as_ref().map_or(&i.name, |a| &a.text);
                !self.not_called.contains(local) && hidden(local)
            }),
            ImportItems::Module(alias) => !self.not_member.contains(&alias.text) && hidden(&alias.text),
        }
    }
}

// -- collecting a file's blocks and name uses --------------------------------------------------------

fn stmts(list: &[Stmt], out: &mut Walk) {
    list.iter().for_each(|s| stmt(s, out));
}

fn stmt(s: &Stmt, out: &mut Walk) {
    match &s.kind {
        StmtKind::Let(_, Some(e)) | StmtKind::Const(_, e) | StmtKind::Expr(e) | StmtKind::Throw(e) | StmtKind::Return(Some(e)) => expr(e, out),
        StmtKind::Let(_, None)
        | StmtKind::Return(None)
        | StmtKind::Break
        | StmtKind::Continue
        | StmtKind::Import { .. }
        | StmtKind::EngineImport(_)
        | StmtKind::Extern(_) => {}
        StmtKind::Assign { target, value, .. } => {
            match target {
                Target::Name(n, _) => out.other_use(&n.text),
                Target::Member(o, _) => expr(o, out),
                Target::Index(o, i, _) => {
                    expr(o, out);
                    expr(i, out);
                }
            }
            expr(value, out);
        }
        StmtKind::If { arms, else_ } => {
            for (cond, body) in arms {
                expr(cond, out);
                stmts(&body.stmts, out);
            }
            if let Some(b) = else_ {
                stmts(&b.stmts, out);
            }
        }
        StmtKind::While { cond, body } => {
            expr(cond, out);
            stmts(&body.stmts, out);
        }
        StmtKind::For { iter, body, .. } => {
            match iter {
                ForIter::Value(e) => expr(e, out),
                ForIter::Range(a, b) => {
                    expr(a, out);
                    expr(b, out);
                }
            }
            stmts(&body.stmts, out);
        }
        StmtKind::Try { body, catch, finally } => {
            stmts(&body.stmts, out);
            if let Some((_, b)) = catch {
                stmts(&b.stmts, out);
            }
            if let Some(b) = finally {
                stmts(&b.stmts, out);
            }
        }
        StmtKind::Func(f) => func(f, out),
        StmtKind::Block(b) => block(b, out),
    }
}

fn func(f: &FuncDecl, out: &mut Walk) {
    for (_, d) in &f.params {
        if let Some(d) = d {
            expr(d, out);
        }
    }
    match &f.body {
        FuncBody::Block(b) => stmts(&b.stmts, out),
        FuncBody::Expr(e) => expr(e, out),
    }
}

fn block(b: &Rc<EngineBlock>, out: &mut Walk) {
    out.bodies.push(b.clone());
    for (_, e) in &b.args {
        expr(e, out);
    }
    // `from v2` reads a top-level name at run time, as a value (13.6).
    for imp in &b.directives.imports {
        if let (Selector::V2 { .. }, ImportItems::Names(items)) = (&imp.selector, &imp.items) {
            items.iter().for_each(|i| out.other_use(&i.name));
        }
    }
}

fn expr(e: &Expr, out: &mut Walk) {
    match &e.kind {
        ExprKind::Null | ExprKind::Bool(_) | ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bytes(_) => {}
        ExprKind::Name(n, _) => out.other_use(n),
        ExprKind::FStr(parts) => {
            for p in parts {
                if let FPart::Expr { expr: x, .. } = p {
                    expr(x, out);
                }
            }
        }
        ExprKind::List(items) => items.iter().for_each(|x| expr(x, out)),
        ExprKind::Dict(entries) => {
            for (k, v) in entries {
                if let DictKey::Computed(k) = k {
                    expr(k, out);
                }
                expr(v, out);
            }
        }
        ExprKind::Unary(_, _, x) | ExprKind::OptChain(x) => expr(x, out),
        ExprKind::Member { obj, optional, .. } => match &obj.kind {
            ExprKind::Name(n, _) if !optional => {
                out.not_called.insert(n.clone());
            }
            _ => expr(obj, out),
        },
        ExprKind::Binary(_, _, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) => {
            expr(a, out);
            expr(b, out);
        }
        ExprKind::Ternary(c, a, b) => {
            expr(c, out);
            expr(a, out);
            expr(b, out);
        }
        ExprKind::Call { callee, args, .. } => {
            match &callee.kind {
                ExprKind::Name(n, _) => {
                    out.not_member.insert(n.clone());
                }
                _ => expr(callee, out),
            }
            for a in args {
                match a {
                    Arg::Pos(x) | Arg::Named(_, x) | Arg::Spread(x) => expr(x, out),
                }
            }
        }
        ExprKind::Index { obj, index, .. } => {
            expr(obj, out);
            expr(index, out);
        }
        ExprKind::Slice { obj, lo, hi, .. } => {
            expr(obj, out);
            for x in [lo, hi].into_iter().flatten() {
                expr(x, out);
            }
        }
        ExprKind::Func(f) => func(f, out),
        ExprKind::Block(b) => block(b, out),
    }
}

// -- routes -------------------------------------------------------------------------------------------

/// What an inner `@import` reaches through the host (13.6). Routes are never released.
pub enum Route {
    /// A resolved function: a Proxy or a V2 function.
    Target(Value),
    /// Resolved when called, with the selector of the importing block's directive `import`;
    /// stays `Pending` while that fails.
    Pending { block: Rc<EngineBlock>, import: usize, name: Rc<str> },
    /// The `v2` selector: the file's top-level `name`, looked up at every call.
    V2 { file: FileId, name: Rc<str> },
}

impl Route {
    /// What a route stub the worker passes back decodes to: the target, or a function that
    /// resolves route `n` when called (11.8).
    pub fn value(&self, n: usize) -> Value {
        match self {
            Route::Target(f) => f.clone(),
            Route::Pending { name, .. } | Route::V2 { name, .. } => Value::func(Callable::Route { route: n, name: name.clone() }),
        }
    }
}

const PY_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else",
    "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
    "return", "try", "while", "with", "yield",
];

fn error(kind: &str, message: String) -> Flow {
    Flow::Error(Rc::new(ErrorValue::new(kind, message)))
}

fn import_error(message: String) -> Flow {
    error("ImportError", message)
}

fn constant_error(name: &str) -> Flow {
    import_error(format!("'{name}' is a constant — pass it in as a block arg"))
}

/// `a`, `a and b`, `a, b and c` (13.3's lists of blocks and advice).
fn join(items: &[String], last: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., end] => format!("{} {last} {end}", init.join(", ")),
    }
}

fn find<'e>(exports: &'e [Export], name: &str) -> Option<&'e Export> {
    exports.iter().find(|e| &*e.name == name)
}

/// The import list's (name, local name, span).
type Item = (Rc<str>, Rc<str>, Span);

fn named_items(items: &[ImportItem]) -> Vec<Item> {
    items.iter().map(|i| (i.name.clone(), i.alias.as_ref().map_or_else(|| i.name.clone(), |a| a.text.clone()), i.span)).collect()
}

impl Interp {
    /// The selector as written, for messages (13.1).
    fn selector_text(&self, sel: &Selector) -> String {
        let (first, last) = match sel {
            Selector::Union { tag } => (tag.span, tag.span),
            Selector::Exact { tag, label } => (tag.span, label.span),
            Selector::Label { label } => (label.span, label.span),
            Selector::Module { lang, path } => (lang.span, path.last().map_or(lang.span, |p| p.span)),
            Selector::V2 { span } => (*span, *span),
        };
        self.sm.file(first.file).text[first.lo as usize..last.hi as usize].to_string()
    }

    /// The session key of a tag (`ts` runs on `js`), or `None` for an unknown tag (13.1).
    pub(crate) fn engine_key(&self, tag: &str) -> Option<Rc<str>> {
        self.registry.canonical(tag).map(|s| s.session_of.clone().unwrap_or_else(|| s.id.clone()))
    }

    /// A selector's engine; an unknown tag is 7.3's `EngineUnknown`.
    fn selector_key(&self, tag: &Name) -> Exec<Rc<str>> {
        self.engine_key(&tag.text).map_or_else(|| fail("EngineUnknown", self.registry.unknown_message(&tag.text)), Ok)
    }

    fn loaded(&self, b: &EngineBlock) -> Option<&Rc<[Export]>> {
        match self.units.get(&b.unit_id) {
            Some(UnitState::Loaded(exports)) => Some(exports),
            _ => None,
        }
    }

    /// The file's definition blocks that a block selector matches, in source order (13.3).
    fn candidates(&self, file: FileId, sel: &Selector) -> Exec<Vec<Rc<EngineBlock>>> {
        let blocks = &self.files[&file].blocks;
        let labeled = |b: &EngineBlock, l: &Name| b.label.as_ref().is_some_and(|x| x.text == l.text);
        Ok(match sel {
            Selector::Union { tag } => {
                let key = self.selector_key(tag)?;
                blocks.iter().filter(|b| self.engine_key(&b.tag.text).as_ref() == Some(&key)).cloned().collect()
            }
            Selector::Exact { tag, label } => {
                let key = self.selector_key(tag)?;
                blocks.iter().filter(|b| labeled(b, label) && self.engine_key(&b.tag.text).as_ref() == Some(&key)).cloned().collect()
            }
            Selector::Label { label } => blocks.iter().filter(|b| labeled(b, label)).cloned().collect(),
            Selector::Module { .. } | Selector::V2 { .. } => unreachable!("not a block selector"),
        })
    }

    /// `b`'s exports, unless it came after the statement that saw `seen` (12.7).
    fn loaded_for(&self, b: &EngineBlock, seen: Seen) -> Option<&Rc<[Export]>> {
        self.loaded(b).filter(|_| seen.is_none_or(|s| s.contains(&b.unit_id)))
    }

    /// Its `@export` list names `name`, or it has loaded and its `*` exported it (13.3).
    fn provides(&self, b: &EngineBlock, name: &str, seen: Seen) -> bool {
        let Some(ex) = &b.directives.exports else { return false };
        ex.items.iter().any(|i| &*i.text == name) || (ex.star.is_some() && self.loaded_for(b, seen).is_some_and(|l| find(l, name).is_some()))
    }

    /// 13.3 steps 1-4 and 6: the one candidate that provides `name`, or the first not yet loaded
    /// `*` candidate, which might (step 5).
    fn provider(&self, file: FileId, sel: &Selector, name: &str, seen: Seen) -> Exec<Rc<EngineBlock>> {
        let cands = self.candidates(file, sel)?;
        let text = self.selector_text(sel);
        if cands.is_empty() {
            return Err(import_error(format!("No engine exports found for selector '{text}'")));
        }
        let hits: Vec<&Rc<EngineBlock>> = cands.iter().filter(|b| self.provides(b, name, seen)).collect();
        match hits.as_slice() {
            [one] => Ok((*one).clone()),
            [] => cands
                .iter()
                .find(|b| b.directives.exports.as_ref().is_some_and(|e| e.star.is_some()) && self.loaded_for(b, seen).is_none())
                .cloned()
                .ok_or_else(|| import_error(format!("'{name}' is not exported by '{text}' (did the block @export it?)"))),
            many => Err(self.ambiguous(name, &text, many)),
        }
    }

    /// 13.3 step 4 (DOCS 6.3).
    fn ambiguous(&self, name: &str, sel: &str, hits: &[&Rc<EngineBlock>]) -> Flow {
        let who: Vec<String> = hits
            .iter()
            .map(|b| match &b.label {
                Some(l) => format!("'{}'", l.text),
                None => self.block_name(b),
            })
            .collect();
        let advice = if hits.iter().all(|b| b.label.is_some()) {
            let picks: Vec<String> = hits
                .iter()
                .map(|b| {
                    let tag = self.registry.canonical(&b.tag.text).map_or_else(|| b.tag.text.clone(), |s| s.id.clone());
                    format!("@{tag}.{}", b.label.as_ref().map_or("", |l| &l.text))
                })
                .collect();
            format!("use {}", join(&picks, "or"))
        } else {
            "label the blocks to choose one".into()
        };
        import_error(format!("Import of '{name}' from '{sel}' is ambiguous (exported by blocks {}) — {advice}", join(&who, "and")))
    }

    /// 13.3 step 5: why a block's exports aren't there. `call`: the imported name, when a block's
    /// call through the import found it (13.7).
    fn state_error(&self, b: &EngineBlock, call: Option<&str>) -> Flow {
        let who = self.block_name(b);
        import_error(match self.units.get(&b.unit_id) {
            Some(UnitState::Failed(e)) => format!("{who} failed to load: {}", e.message),
            Some(UnitState::Loading) => format!("{who} is still loading — its exports exist once its top-level code finishes"),
            _ => match call {
                Some(name) => format!("{who} has not run yet — '{name}' can't be called before it loads"),
                None => format!("{who} has not run yet — move this @import below it"),
            },
        })
    }

    /// One name from a block selector at V2 level (13.3).
    fn resolve_name(&mut self, file: FileId, sel: &Selector, name: &str, seen: Seen, at: Span) -> Exec<Value> {
        self.start_waiting(file, sel, at)?;
        let b = self.provider(file, sel, name, seen)?;
        match self.loaded_for(&b, seen) {
            Some(exports) => match find(exports, name) {
                Some(e) => Ok(e.value.clone()),
                None => Err(import_error(format!("'{name}' is not exported by '{}' (did the block @export it?)", self.selector_text(sel)))),
            },
            None => Err(self.state_error(&b, None)),
        }
    }

    /// This use can't wait: the languages of the blocks `sel` matches start now, so every block
    /// that has been reached counts (12.7, 13.3).
    fn start_waiting(&mut self, file: FileId, sel: &Selector, at: Span) -> Exec<()> {
        for b in self.candidates(file, sel)? {
            if matches!(self.units.get(&b.unit_id), Some(UnitState::Waiting)) {
                self.start_engine(&b.tag.text, at)?;
            }
        }
        Ok(())
    }

    /// `@import … from sel` at V2 level (13.4): each name a `const` of `env`, resolved again every
    /// time the statement runs, unless it waits (12.7).
    pub fn exec_import(&mut self, imp: &Rc<EngineImport>, env: &Env) -> Exec<()> {
        let bound = match self.import_waits(imp) {
            Some(key) => self.wait_import(imp, key)?,
            None => self.resolve_import(imp, None)?,
        };
        for (name, value) in bound {
            declare(env, name, value, true);
        }
        Ok(())
    }

    /// The line `imp` joins instead of running now: it is eligible (13.4), its language isn't
    /// running, and whatever it names has been reached. Anything else, errors included, is for
    /// the statement itself.
    fn import_waits(&self, imp: &EngineImport) -> Option<Rc<str>> {
        let file = imp.span.file;
        if !self.files[&file].eligible.contains(&imp.span.lo) {
            return None;
        }
        let key = match (&imp.selector, &imp.items) {
            (Selector::Module { lang, .. }, _) => self.engine_key(&lang.text)?,
            (sel, ImportItems::Names(items)) => {
                let mut key = None;
                for item in items {
                    let b = self.provider(file, sel, &item.name, None).ok()?;
                    if !matches!(self.units.get(&b.unit_id), Some(UnitState::Waiting)) {
                        return None;
                    }
                    key = self.engine_key(&b.tag.text);
                }
                key?
            }
            _ => return None,
        };
        (!self.running(&key)).then_some(key)
    }

    /// `imp` waits: it joins the line, and each name is bound to a reference that settles at its
    /// first use (12.7). A name that can't be valid raises here, as it would have.
    fn wait_import(&mut self, imp: &Rc<EngineImport>, key: Rc<str>) -> Exec<Vec<(Rc<str>, Value)>> {
        if let Selector::Module { path, .. } = &imp.selector {
            let joined = path.iter().map(|n| &*n.text).collect::<Vec<_>>().join(".");
            self.module_unit(imp.span.file, &key, &joined).map_err(|f| self.raise(f, imp.span))?;
        }
        let blocks = &self.files[&imp.span.file].blocks;
        let seen: Rc<[UnitId]> = blocks.iter().map(|b| b.unit_id).filter(|id| self.units.contains_key(id)).collect();
        self.line.push(key, Waiting::Import(imp.clone(), seen.clone()));
        let locals: Vec<Rc<str>> = match &imp.items {
            ImportItems::Names(items) => named_items(items).into_iter().map(|(_, alias, _)| alias).collect(),
            ImportItems::Module(alias) => vec![alias.text.clone()],
        };
        let reference = |local: Rc<str>| Callable::Waiting { import: imp.clone(), local, seen: seen.clone(), resolved: Default::default() };
        Ok(locals.into_iter().map(|local| (local.clone(), Value::func(reference(local)))).collect())
    }

    /// What `imp` binds (13.4): each local name and its value. `seen`: the statement ran earlier
    /// and saw these blocks (12.7).
    pub(crate) fn resolve_import(&mut self, imp: &EngineImport, seen: Seen) -> Exec<Vec<(Rc<str>, Value)>> {
        let file = imp.span.file;
        let mut bound = Vec::new();
        match (&imp.selector, &imp.items) {
            (Selector::Module { .. }, items) => {
                let names = match items {
                    ImportItems::Names(list) => json!(list.iter().map(|i| &*i.name).collect::<Vec<_>>()),
                    ImportItems::Module(_) => Json::Null,
                };
                let (value, exports, missing) = self.import_module(file, &imp.selector, names, imp.span)?;
                let text = self.selector_text(&imp.selector);
                match items {
                    ImportItems::Module(alias) => bound.push((alias.text.clone(), value)),
                    ImportItems::Names(list) => {
                        for (name, alias, span) in named_items(list) {
                            match find(&exports, &name) {
                                Some(e) if !missing.contains(&name) => bound.push((alias, e.value.clone())),
                                _ => {
                                    let msg = format!("'{name}' is not exported by '{text}'");
                                    return Err(self.raise(import_error(msg), span));
                                }
                            }
                        }
                    }
                }
            }
            (sel, ImportItems::Names(list)) => {
                // A name nothing can provide needs no language to find that out.
                for (name, _, span) in named_items(list) {
                    self.provider(file, sel, &name, seen).map_err(|f| self.raise(f, span))?;
                }
                for (name, alias, span) in named_items(list) {
                    let v = self.resolve_name(file, sel, &name, seen, span).map_err(|f| self.raise(f, span))?;
                    bound.push((alias, v));
                }
            }
            (_, ImportItems::Module(_)) => unreachable!("the parser allows `as` only with a package selector"),
        }
        Ok(bound)
    }

    /// What a waiting reference stands for: found at its first use, at `at`, which starts its
    /// language; a failure leaves it waiting (12.7).
    pub(crate) fn settle(&mut self, f: &Func, at: Span) -> Exec<Value> {
        let Callable::Waiting { import, local, seen, resolved } = &**f else { unreachable!("only a waiting reference settles") };
        if let Some(v) = resolved.get() {
            return Ok(v.clone());
        }
        if let Some(tag) = self.import_tag(import) {
            self.start_engine(&tag, at)?;
        }
        let value = self.resolve_import(import, Some(seen))?.into_iter().find(|(name, _)| name == local).map(|(_, v)| v);
        let value = value.expect("an @import binds each of its local names");
        Ok(resolved.get_or_init(|| value).clone())
    }

    /// `v`, or what it stands for if it is a waiting reference (12.7).
    pub(crate) fn settled(&mut self, v: Value, at: Span) -> Exec<Value> {
        match &v {
            Value::Func(f) if matches!(&**f, Callable::Waiting { .. }) => self.settle(f, at),
            _ => Ok(v),
        }
    }

    /// The tag of the engine `imp` needs.
    fn import_tag(&self, imp: &EngineImport) -> Option<Rc<str>> {
        match (&imp.selector, &imp.items) {
            (Selector::Module { lang, .. }, _) => Some(lang.text.clone()),
            (sel, ImportItems::Names(items)) => Some(self.provider(imp.span.file, sel, &items.first()?.name, None).ok()?.tag.text.clone()),
            _ => None,
        }
    }

    /// A module unit's id (13.5), after checking the name: `py:os.path`, `js:date-fns`, or for a
    /// relative or absolute JS specifier the absolute path from the importing file's folder.
    fn module_unit(&self, file: FileId, key: &str, name: &str) -> Exec<String> {
        if key == "py" {
            let ident = |seg: &str| {
                let mut cs = seg.chars();
                cs.next().is_some_and(|c| c == '_' || c.is_alphabetic()) && cs.all(|c| c == '_' || c.is_alphanumeric())
            };
            if !name.split('.').all(|seg| ident(seg) && !PY_KEYWORDS.contains(&seg)) {
                return Err(import_error(format!("'{name}' is not a valid Python module name")));
            }
        } else if name.is_empty() {
            return Err(import_error("'' is not a valid module name".into()));
        }
        let relative = name == "." || name == ".." || name.starts_with("./") || name.starts_with("../")
            || (cfg!(windows) && (name.starts_with(".\\") || name.starts_with("..\\")));
        if key == "js" && (relative || Path::new(name).is_absolute()) {
            let dir = self.sm.file(file).path.parent().map(Path::to_path_buf).unwrap_or_default();
            return Ok(format!("js:{}", absolute_clean(&dir.join(name)).display()));
        }
        Ok(format!("{key}:{name}"))
    }

    /// One `module` frame (13.5): the module handle (or null), the exports of the requested
    /// `names` (a list, or null) and the requested names the module lacks.
    fn import_module(&mut self, file: FileId, sel: &Selector, names: Json, at: Span) -> Exec<(Value, Vec<Export>, Vec<Rc<str>>)> {
        let Selector::Module { lang, path } = sel else { unreachable!("a module selector") };
        let key = self.selector_key(lang).map_err(|f| self.raise(f, at))?;
        let joined = path.iter().map(|n| &*n.text).collect::<Vec<_>>().join(".");
        let unit = self.module_unit(file, &key, &joined).map_err(|f| self.raise(f, at))?;
        let s = self.start_engine(&key, at)?;
        self.check_alive(s, at)?;
        if !self.sessions[s].caps.modules {
            return Err(self.raise(import_error(format!("engine '{key}' cannot import packages")), at));
        }
        let from = self.sm.file(file).path.to_string_lossy().into_owned();
        let frame = json!({"t": "module", "unit": unit, "name": joined, "from": from, "names": names});
        let what = format!("@import from '{}'", self.selector_text(sel));
        let reply = self.request(s, frame, &what, at)?;
        let unit: Rc<str> = unit.into();
        let exports = reply.exports.into_iter().map(|(name, v)| export(s, &unit, name, v)).collect();
        Ok((reply.value, exports, reply.missing))
    }

    /// The route for one item of an inner `@import`; a later load of the importer resets it (13.6).
    fn route(&mut self, importer: UnitId, import: usize, name: &Rc<str>, route: Route) -> usize {
        let key = (importer, import, name.clone());
        if let Some(&n) = self.route_keys.get(&key) {
            self.routes[n] = route;
            return n;
        }
        self.routes.push(route);
        self.route_keys.insert(key, self.routes.len() - 1);
        self.routes.len() - 1
    }

    /// 13.7: the `imports` entries of block `b`'s `load` to the session of engine `key`. Errors
    /// no later load can fix are raised here.
    pub(crate) fn inner_imports(&mut self, b: &Rc<EngineBlock>, key: &str) -> Exec<Vec<Json>> {
        let file = b.unit_id.file;
        let mut entries = Vec::new();
        for (i, imp) in b.directives.imports.iter().enumerate() {
            let pending = |name: &Rc<str>| Route::Pending { block: b.clone(), import: i, name: name.clone() };
            match &imp.selector {
                Selector::V2 { .. } => {
                    let ImportItems::Names(list) = &imp.items else { unreachable!("a static error in blocks") };
                    for (name, alias, span) in named_items(list) {
                        if !self.files[&file].top.contains(&name) {
                            let msg = format!("'{name}' is not a top-level name in {}", self.sm.file(file).display);
                            return Err(self.raise(import_error(msg), span));
                        }
                        // A block imports functions; a value already there that isn't one is a
                        // constant (DOCS 6.2).
                        let value = own_value(&self.files[&file].frame, &name);
                        if value.is_some_and(|v| !matches!(&v, Value::Func(_)) && !matches!(&v, Value::Handle(h) if h.callable)) {
                            return Err(self.raise(constant_error(&name), span));
                        }
                        let r = self.route(b.unit_id, i, &name, Route::V2 { file, name: name.clone() });
                        entries.push(json!({"as": &*alias, "route": r}));
                    }
                }
                Selector::Module { lang, path } => {
                    let target = self.selector_key(lang).map_err(|f| self.raise(f, imp.span))?;
                    let ImportItems::Names(list) = &imp.items else { unreachable!("a static error in blocks") };
                    if *target != *key {
                        // Another session's module is imported when first called (13.6).
                        let joined = path.iter().map(|n| &*n.text).collect::<Vec<_>>().join(".");
                        self.module_unit(file, &target, &joined).map_err(|f| self.raise(f, imp.span))?;
                        for (name, alias, _) in named_items(list) {
                            let r = self.route(b.unit_id, i, &name, pending(&name));
                            entries.push(json!({"as": &*alias, "route": r}));
                        }
                        continue;
                    }
                    let names = json!(list.iter().map(|i| &*i.name).collect::<Vec<_>>());
                    let (_, exports, missing) = self.import_module(file, &imp.selector, names, imp.span)?;
                    let text = self.selector_text(&imp.selector);
                    for (name, alias, span) in named_items(list) {
                        let found = find(&exports, &name).filter(|_| !missing.contains(&name));
                        let Some(e) = found else {
                            let msg = format!("'{name}' is not exported by '{text}'");
                            return Err(self.raise(import_error(msg), span));
                        };
                        if !e.func {
                            return Err(self.raise(constant_error(&name), span));
                        }
                        let Value::Func(f) = &e.value else { unreachable!("a func export is a Proxy") };
                        let Callable::Proxy { unit, .. } = &**f else { unreachable!("a module export is a Proxy") };
                        entries.push(json!({"as": &*alias, "unit": &**unit, "name": &*name}));
                    }
                }
                sel => {
                    let ImportItems::Names(list) = &imp.items else { unreachable!("a static error in blocks") };
                    for (name, alias, span) in named_items(list) {
                        let hit = self.provider(file, sel, &name, None).map_err(|f| self.raise(f, span))?;
                        match self.units.get(&hit.unit_id) {
                            Some(UnitState::Failed(_)) => return Err(self.raise(self.state_error(&hit, None), span)),
                            Some(UnitState::Loaded(ex)) if find(ex, &name).is_some_and(|e| !e.func) => {
                                return Err(self.raise(constant_error(&name), span));
                            }
                            _ => {}
                        }
                        // A name only a not yet loaded `*` block might provide gets a route (13.7).
                        let known = self.loaded(&hit).is_some() || self.provides(&hit, &name, None);
                        if known && self.engine_key(&hit.tag.text).as_deref() == Some(key) {
                            let r = self.route(b.unit_id, i, &name, pending(&name));
                            entries.push(json!({"as": &*alias, "unit": wire_unit(hit.unit_id), "name": &*name, "route": r}));
                        } else {
                            let r = self.route(b.unit_id, i, &name, pending(&name));
                            entries.push(json!({"as": &*alias, "route": r}));
                        }
                    }
                }
            }
        }
        Ok(entries)
    }

    /// A worker's `call {"route": n}` (13.6), or a call of a route stub passed back to V2.
    pub(crate) fn call_route(&mut self, n: usize, pos: Vec<Value>, named: Vec<(Rc<str>, Value)>, at: Span) -> Exec<Value> {
        let f = match self.routes.get(n) {
            None => return fail("ProtocolError", format!("unknown callable id {n}")),
            Some(Route::Target(f)) => f.clone(),
            Some(Route::V2 { file, name }) => {
                let (file, name) = (*file, name.clone());
                let found = own_value(&self.files[&file].frame, &name);
                match found {
                    Some(f @ Value::Func(_)) => f,
                    Some(Value::Handle(h)) if h.callable => Value::Handle(h),
                    Some(_) => return Err(self.raise(error("TypeError", format!("'{name}' is not a function")), at)),
                    None => {
                        let msg = format!("'{name}' is not defined at the top level of {}", self.sm.file(file).display);
                        return Err(self.raise(error("NameError", msg), at));
                    }
                }
            }
            Some(Route::Pending { block, import, name }) => {
                let (block, import, name) = (block.clone(), *import, name.clone());
                let f = self.resolve_route(&block, import, &name, at).map_err(|f| self.raise(f, at))?;
                self.routes[n] = Route::Target(f.clone());
                f
            }
        };
        self.call_value(&f, pos, named, at)
    }

    /// A `Pending` route's target, found now (13.6); not loaded yet and constants give 13.7's
    /// messages.
    fn resolve_route(&mut self, block: &Rc<EngineBlock>, import: usize, name: &Rc<str>, at: Span) -> Exec<Value> {
        let imp = &block.directives.imports[import];
        let file = block.unit_id.file;
        if let Selector::Module { .. } = &imp.selector {
            let (_, exports, missing) = self.import_module(file, &imp.selector, json!([&**name]), at)?;
            return match find(&exports, name) {
                Some(e) if !missing.contains(name) && e.func => Ok(e.value.clone()),
                Some(_) if !missing.contains(name) => Err(constant_error(name)),
                _ => Err(import_error(format!("'{name}' is not exported by '{}'", self.selector_text(&imp.selector)))),
            };
        }
        self.start_waiting(file, &imp.selector, at)?;
        let hit = self.provider(file, &imp.selector, name, None)?;
        match self.loaded(&hit).map(|ex| find(ex, name).cloned()) {
            Some(Some(e)) if e.func => Ok(e.value),
            Some(Some(_)) => Err(constant_error(name)),
            Some(None) => Err(import_error(format!("'{name}' is not exported by '{}' (did the block @export it?)", self.selector_text(&imp.selector)))),
            None => Err(self.state_error(&hit, Some(name))),
        }
    }
}

/// A reply's export entry (9.4): a function becomes a Proxy into `unit` of session `s` (13.4).
pub fn export(s: SessionId, unit: &Rc<str>, name: Rc<str>, value: Option<Value>) -> Export {
    match value {
        None => Export { value: Value::func(Callable::Proxy { session: s, unit: unit.clone(), name: name.clone() }), func: true, name },
        Some(v) => Export { name, value: v, func: false },
    }
}
