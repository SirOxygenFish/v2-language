//! Import resolution and routing (INTERNALS ch. 13): what selectors see of a file (6.2), resolving
//! names (13.3), binding at V2 level (13.4), module units (13.5), routes (13.6) and the `imports`
//! entries of a block's `load` (13.7).

use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use serde_json::{json, Value as Json};

use super::session::SessionId;
use super::units::{wire_unit, Export, UnitState};
use crate::runtime::env::{declare, declare_star, star_bound, Env};
use crate::runtime::error::{fail, ErrorValue, Exec, Flow};
use crate::runtime::interp::Interp;
use crate::runtime::value::{Callable, Value};
use crate::syntax::ast::*;
use crate::syntax::lexer::is_plain_name;
use crate::syntax::span::{absolute_clean, FileId, Span};

/// What import resolution needs of one file (6.2): its module frame and its blocks.
pub struct FileScope {
    pub frame: Env,
    /// The definition blocks, in source order, reached or not (13.3's candidates).
    blocks: Vec<Rc<EngineBlock>>,
    /// Every block, by body line, for mapping foreign frames (2.3, 12.6).
    pub bodies: Vec<Rc<EngineBlock>>,
    /// The names declared at the top level, for the `v2` selector (13.7).
    top: HashSet<Rc<str>>,
    /// The top-level `func`s in order, for `@import { * } from v2`.
    funcs: Vec<Rc<str>>,
    /// A top-level `@import { * }` may declare any name.
    star: bool,
}

impl FileScope {
    pub fn new(program: &[Stmt], frame: Env) -> FileScope {
        let mut bodies = Vec::new();
        stmts(program, &mut bodies);
        bodies.sort_by_key(|b| b.unit_id.offset);
        let blocks = bodies.iter().filter(|b| b.directives.exports.is_some()).cloned().collect();
        let mut top = HashSet::new();
        let mut funcs = Vec::new();
        let mut star = false;
        for s in program {
            match &s.kind {
                StmtKind::Func(f) => funcs.extend(f.name.as_ref().map(|n| n.text.clone())),
                StmtKind::EngineImport(imp) => star |= matches!(imp.items, ImportItems::All(_)),
                _ => {}
            }
            top.extend(crate::syntax::check::declarations(s).into_iter().map(|(n, _)| n));
        }
        FileScope { frame, blocks, bodies, top, funcs, star }
    }
}

// -- collecting a file's blocks ---------------------------------------------------------------------

fn stmts(list: &[Stmt], out: &mut Vec<Rc<EngineBlock>>) {
    list.iter().for_each(|s| stmt(s, out));
}

fn stmt(s: &Stmt, out: &mut Vec<Rc<EngineBlock>>) {
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
                Target::Name(_) => {}
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

fn func(f: &FuncDecl, out: &mut Vec<Rc<EngineBlock>>) {
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

fn block(b: &Rc<EngineBlock>, out: &mut Vec<Rc<EngineBlock>>) {
    out.push(b.clone());
    for (_, e) in &b.args {
        expr(e, out);
    }
}

fn expr(e: &Expr, out: &mut Vec<Rc<EngineBlock>>) {
    match &e.kind {
        ExprKind::Null | ExprKind::Bool(_) | ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bytes(_) | ExprKind::Name(_) => {}
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
        ExprKind::Unary(_, _, x) | ExprKind::OptChain(x) | ExprKind::Member { obj: x, .. } => expr(x, out),
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
            expr(callee, out);
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

/// The import list's (name, local name, span), or `*` from `v2`'s functions (13.7).
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

    /// Its `@export` list names `name`, or it has loaded and its `*` exported it (13.3).
    fn provides(&self, b: &EngineBlock, name: &str) -> bool {
        let Some(ex) = &b.directives.exports else { return false };
        ex.items.iter().any(|i| &*i.text == name) || (ex.star.is_some() && self.loaded(b).is_some_and(|l| find(l, name).is_some()))
    }

    /// 13.3 steps 1-4 and 6: the one candidate that provides `name`, or the first not yet loaded
    /// `*` candidate, which might (step 5).
    fn provider(&self, file: FileId, sel: &Selector, name: &str) -> Exec<Rc<EngineBlock>> {
        let cands = self.candidates(file, sel)?;
        let text = self.selector_text(sel);
        if cands.is_empty() {
            return Err(import_error(format!("No engine exports found for selector '{text}'")));
        }
        let hits: Vec<&Rc<EngineBlock>> = cands.iter().filter(|b| self.provides(b, name)).collect();
        match hits.as_slice() {
            [one] => Ok((*one).clone()),
            [] => cands
                .iter()
                .find(|b| b.directives.exports.as_ref().is_some_and(|e| e.star.is_some()) && self.loaded(b).is_none())
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
    fn resolve_name(&self, file: FileId, sel: &Selector, name: &str) -> Exec<Value> {
        let b = self.provider(file, sel, name)?;
        match self.loaded(&b) {
            Some(exports) => match find(exports, name) {
                Some(e) => Ok(e.value.clone()),
                None => Err(import_error(format!("'{name}' is not exported by '{}' (did the block @export it?)", self.selector_text(sel)))),
            },
            None => Err(self.state_error(&b, None)),
        }
    }

    /// `@import { * } from sel` (13.3): the union of the loaded candidates' exports, each with the
    /// block it comes from; a name two candidates provide is ambiguous.
    fn resolve_star(&self, file: FileId, sel: &Selector) -> Exec<Vec<(Rc<EngineBlock>, Export)>> {
        let cands = self.candidates(file, sel)?;
        let text = self.selector_text(sel);
        let Some(first) = cands.first() else {
            return Err(import_error(format!("No engine exports found for selector '{text}'")));
        };
        if cands.iter().all(|b| self.loaded(b).is_none()) {
            return Err(self.state_error(first, None));
        }
        let mut out: Vec<(Rc<EngineBlock>, Export)> = Vec::new();
        for b in &cands {
            for e in self.loaded(b).into_iter().flat_map(|l| l.iter()) {
                if out.iter().any(|(_, x)| x.name == e.name) {
                    continue;
                }
                let hits: Vec<&Rc<EngineBlock>> = cands.iter().filter(|c| self.provides(c, &e.name)).collect();
                if hits.len() > 1 {
                    return Err(self.ambiguous(&e.name, &text, &hits));
                }
                out.push((b.clone(), e.clone()));
            }
        }
        Ok(out)
    }

    /// `@import … from sel` at V2 level (13.4): each name a `const` of `env`, resolved again
    /// every time the statement runs.
    pub fn exec_import(&mut self, imp: &EngineImport, env: &Env) -> Exec<()> {
        let file = imp.span.file;
        // (local name, value, where an error points, bound by `*`)
        let mut bound: Vec<(Rc<str>, Value, Span, bool)> = Vec::new();
        match (&imp.selector, &imp.items) {
            (Selector::Module { .. }, items) => {
                let names = match items {
                    ImportItems::Names(list) => json!(list.iter().map(|i| &*i.name).collect::<Vec<_>>()),
                    ImportItems::All(_) => json!("*"),
                    ImportItems::Module(_) => Json::Null,
                };
                let (value, exports, missing) = self.import_module(file, &imp.selector, names, imp.span)?;
                let text = self.selector_text(&imp.selector);
                match items {
                    ImportItems::Module(alias) => bound.push((alias.text.clone(), value, alias.span, false)),
                    ImportItems::Names(list) => {
                        for (name, alias, span) in named_items(list) {
                            match find(&exports, &name) {
                                Some(e) if !missing.contains(&name) => bound.push((alias, e.value.clone(), span, false)),
                                _ => {
                                    let msg = format!("'{name}' is not exported by '{text}'");
                                    return Err(self.raise(import_error(msg), span));
                                }
                            }
                        }
                    }
                    ImportItems::All(span) => {
                        if let Some(name) = missing.first() {
                            return Err(self.raise(import_error(format!("'{name}' is not exported by '{text}'")), *span));
                        }
                        bound.extend(exports.into_iter().map(|e| (e.name, e.value, *span, true)));
                    }
                }
            }
            (sel, ImportItems::Names(list)) => {
                for (name, alias, span) in named_items(list) {
                    let v = self.resolve_name(file, sel, &name).map_err(|f| self.raise(f, span))?;
                    bound.push((alias, v, span, false));
                }
            }
            (sel, ImportItems::All(span)) => {
                let exports = self.resolve_star(file, sel).map_err(|f| self.raise(f, *span))?;
                bound.extend(exports.into_iter().map(|(_, e)| (e.name, e.value, *span, true)));
            }
            (_, ImportItems::Module(_)) => unreachable!("the parser allows `as` only with a package selector"),
        }
        for (name, value, span, star) in bound {
            // Only names V2 can write; import the others quoted, with `as` (13.4).
            if star && !is_plain_name(&name) {
                continue;
            }
            // Static checks see every other declaration, not the names `*` binds (5.6).
            let clash = if star { env.borrow().vars.contains_key(&name) } else { star_bound(env, &name) };
            if clash {
                return Err(self.raise(error("NameError", format!("'{name}' is already declared in this block")), span));
            }
            if star {
                declare_star(env, name, value);
            } else {
                declare(env, name, value, true);
            }
        }
        Ok(())
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
    /// `names` (a list, `"*"` or null) and the requested names the module lacks.
    fn import_module(&mut self, file: FileId, sel: &Selector, names: Json, at: Span) -> Exec<(Value, Vec<Export>, Vec<Rc<str>>)> {
        let Selector::Module { lang, path } = sel else { unreachable!("a module selector") };
        let key = self.selector_key(lang).map_err(|f| self.raise(f, at))?;
        let joined = path.iter().map(|n| &*n.text).collect::<Vec<_>>().join(".");
        let unit = self.module_unit(file, &key, &joined).map_err(|f| self.raise(f, at))?;
        let s = match self.sessions.iter().position(|x| x.key == key) {
            Some(s) => s,
            None => {
                let engine = self.registry.resolve_tag(&key).map_err(|f| self.raise(f, at))?;
                self.session_for(&engine, at)?
            }
        };
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
                    let scope = &self.files[&file];
                    let items: Vec<Item> = match &imp.items {
                        ImportItems::Names(list) => named_items(list),
                        ImportItems::All(span) => scope.funcs.iter().filter(|n| !n.starts_with('_')).map(|n| (n.clone(), n.clone(), *span)).collect(),
                        ImportItems::Module(_) => unreachable!("a static error in blocks"),
                    };
                    for (name, alias, span) in items {
                        let scope = &self.files[&file];
                        if !scope.top.contains(&name) && !scope.star {
                            let msg = format!("'{name}' is not a top-level name in {}", self.sm.file(file).display);
                            return Err(self.raise(import_error(msg), span));
                        }
                        let r = self.route(b.unit_id, i, &name, Route::V2 { file, name: name.clone() });
                        entries.push(json!({"as": &*alias, "route": r}));
                    }
                }
                Selector::Module { lang, path } => {
                    let target = self.selector_key(lang).map_err(|f| self.raise(f, imp.span))?;
                    let same = *target == *key;
                    let star = matches!(imp.items, ImportItems::All(_));
                    if !same && !star {
                        // Another session's module is imported when first called (13.6).
                        let joined = path.iter().map(|n| &*n.text).collect::<Vec<_>>().join(".");
                        self.module_unit(file, &target, &joined).map_err(|f| self.raise(f, imp.span))?;
                        let ImportItems::Names(list) = &imp.items else { unreachable!() };
                        for (name, alias, _) in named_items(list) {
                            let r = self.route(b.unit_id, i, &name, pending(&name));
                            entries.push(json!({"as": &*alias, "route": r}));
                        }
                        continue;
                    }
                    let names = match &imp.items {
                        ImportItems::Names(list) => json!(list.iter().map(|i| &*i.name).collect::<Vec<_>>()),
                        _ => json!("*"),
                    };
                    let (_, exports, missing) = self.import_module(file, &imp.selector, names, imp.span)?;
                    let text = self.selector_text(&imp.selector);
                    let items: Vec<(Item, &Export)> = match &imp.items {
                        ImportItems::Names(list) => {
                            let mut out = Vec::new();
                            for item in named_items(list) {
                                let found = find(&exports, &item.0).filter(|_| !missing.contains(&item.0));
                                let Some(e) = found else {
                                    let msg = format!("'{}' is not exported by '{text}'", item.0);
                                    return Err(self.raise(import_error(msg), item.2));
                                };
                                if !e.func {
                                    return Err(self.raise(constant_error(&item.0), item.2));
                                }
                                out.push((item, e));
                            }
                            out
                        }
                        _ => {
                            if let Some(name) = missing.first() {
                                return Err(self.raise(import_error(format!("'{name}' is not exported by '{text}'")), imp.span));
                            }
                            exports.iter().filter(|e| e.func).map(|e| ((e.name.clone(), e.name.clone(), imp.span), e)).collect()
                        }
                    };
                    for ((name, alias, _), e) in items {
                        if same {
                            let Value::Func(f) = &e.value else { unreachable!("a func export is a Proxy") };
                            let Callable::Proxy { unit, .. } = &**f else { unreachable!("a module export is a Proxy") };
                            entries.push(json!({"as": &*alias, "unit": &**unit, "name": &*name}));
                        } else {
                            let r = self.route(b.unit_id, i, &name, pending(&name));
                            entries.push(json!({"as": &*alias, "route": r}));
                        }
                    }
                }
                sel => {
                    let items: Vec<(Item, Rc<EngineBlock>)> = match &imp.items {
                        ImportItems::Names(list) => {
                            let mut out = Vec::new();
                            for item in named_items(list) {
                                let hit = self.provider(file, sel, &item.0).map_err(|f| self.raise(f, item.2))?;
                                match self.units.get(&hit.unit_id) {
                                    Some(UnitState::Failed(_)) => return Err(self.raise(self.state_error(&hit, None), item.2)),
                                    Some(UnitState::Loaded(ex)) if find(ex, &item.0).is_some_and(|e| !e.func) => {
                                        return Err(self.raise(constant_error(&item.0), item.2));
                                    }
                                    _ => {}
                                }
                                out.push((item, hit));
                            }
                            out
                        }
                        ImportItems::All(span) => {
                            let exports = self.resolve_star(file, sel).map_err(|f| self.raise(f, *span))?;
                            exports.into_iter().filter(|(_, e)| e.func).map(|(hit, e)| ((e.name.clone(), e.name, *span), hit)).collect()
                        }
                        ImportItems::Module(_) => unreachable!("a static error in blocks"),
                    };
                    for ((name, alias, _), hit) in items {
                        // A name only a not yet loaded `*` block might provide gets a route (13.7).
                        let known = self.loaded(&hit).is_some() || self.provides(&hit, &name);
                        if known && self.engine_key(&hit.tag.text).as_deref() == Some(key) {
                            let unit = wire_unit(hit.unit_id);
                            entries.push(json!({"as": &*alias, "unit": unit, "name": &*name, "block": self.block_name(&hit)}));
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
                let found = self.files[&file].frame.borrow().vars.get(&name).map(|slot| slot.value.clone());
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
        let hit = self.provider(file, &imp.selector, name)?;
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
