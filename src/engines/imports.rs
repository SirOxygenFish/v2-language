//! Import resolution and routing (INTERNALS ch. 13): what selectors see of a file (6.2), resolving
//! names (13.3), binding at V2 level (13.4), module units (13.5), routes (13.6) and the `imports`
//! entries of a block's `load` (13.7).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::{json, Value as Json};

use super::session::SessionId;
use super::units::{wire_unit, Export, UnitState, Waiting};
use crate::runtime::env::{declare, own_value, Env};
use crate::runtime::error::{did_you_mean, fail, ErrorValue, Exec, Flow};
use crate::runtime::interp::Interp;
use crate::runtime::value::{Callable, Func, Value};
use crate::syntax::scan::canonical_tag;
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
    /// The names declared at the top level, for the `v2` selector (13.7), with where and how
    /// (`let`, `const`, …), for a use before that line (5.9).
    top: HashMap<Rc<str>, (Span, &'static str)>,
    /// The definition blocks that may wait for their language (12.7).
    waiting: HashSet<UnitId>,
    /// The blocks that are statements of the file itself, not inside an `if`, a loop or a function.
    top_blocks: HashSet<UnitId>,
    /// The `@import` statements that may, by span start (13.4).
    eligible: HashSet<u32>,
    /// Names declared inside a nested body, by name: where, that body's span and what it is
    /// (`for loop`, `function show`), for a use outside it (5.9).
    inner: HashMap<Rc<str>, Vec<Inner>>,
    /// The V2 modules the file imports at its top level, by alias: `lib` for `import "./lib.v2"
    /// as lib`, whose names an `@import … from lib.x` can't reach (13.1).
    modules: HashMap<Rc<str>, Rc<str>>,
}

/// A declaration in a nested body.
struct Inner {
    at: Span,
    how: &'static str,
    body: Span,
    what: Rc<str>,
}

impl FileScope {
    /// `entry`: this is the file the run started with, so no other file can see its names (13.4).
    pub fn new(program: &[Stmt], frame: Env, entry: bool) -> FileScope {
        let mut walk = Walk::default();
        stmts(program, &mut walk);
        let mut bodies = std::mem::take(&mut walk.bodies);
        let inner = std::mem::take(&mut walk.inner);
        let mut modules = HashMap::new();
        bodies.sort_by_key(|b| b.unit_id.offset);
        let blocks = bodies.iter().filter(|b| b.directives.exports.is_some()).cloned().collect();
        let mut top = HashMap::new();
        let mut waiting = HashSet::new();
        let mut eligible = HashSet::new();
        let mut top_blocks = HashSet::new();
        for s in program {
            if let StmtKind::Block(b) = &s.kind {
                top_blocks.insert(b.unit_id);
            }
            match &s.kind {
                StmtKind::Block(b) if may_wait(b) => {
                    waiting.insert(b.unit_id);
                }
                StmtKind::EngineImport(imp) if walk.import_may_wait(imp, entry) => {
                    eligible.insert(imp.span.lo);
                }
                StmtKind::Import { path, alias } => {
                    modules.insert(alias.text.clone(), path.clone());
                }
                _ => {}
            }
            let how = match &s.kind {
                StmtKind::Let(..) => "let",
                StmtKind::Const(..) => "const",
                StmtKind::Func(_) => "func",
                StmtKind::Import { .. } => "import",
                StmtKind::EngineImport(_) => "@import",
                _ => "extern",
            };
            top.extend(crate::syntax::check::declarations(s).into_iter().map(|(n, at)| (n, (at, how))));
        }
        FileScope { frame, blocks, bodies, top, waiting, top_blocks, eligible, inner, modules }
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
    inner: HashMap<Rc<str>, Vec<Inner>>,
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

/// A nested body's statements, with the names it declares.
fn nested(body: &Body, what: &str, out: &mut Walk) {
    let what: Rc<str> = what.into();
    for s in &body.stmts {
        let how = match &s.kind {
            StmtKind::Let(..) => "let",
            StmtKind::Const(..) => "const",
            StmtKind::Func(_) => "func",
            _ => continue,
        };
        for (name, at) in crate::syntax::check::declarations(s) {
            out.inner.entry(name).or_default().push(Inner { at, how, body: body.span, what: what.clone() });
        }
    }
    stmts(&body.stmts, out);
}

fn stmt(s: &Stmt, out: &mut Walk) {
    match &s.kind {
        StmtKind::Let(_, Some(e)) | StmtKind::Const(_, e) | StmtKind::Expr(e) | StmtKind::Throw(e) | StmtKind::Return(Some(e)) => expr(e, out),
        StmtKind::Let(_, None)
        | StmtKind::Import { .. }
        | StmtKind::Return(None)
        | StmtKind::Break
        | StmtKind::Continue
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
                nested(body, "if", out);
            }
            if let Some(b) = else_ {
                nested(b, "if", out);
            }
        }
        StmtKind::While { cond, body } => {
            expr(cond, out);
            nested(body, "while loop", out);
        }
        StmtKind::For { iter, body, .. } => {
            match iter {
                ForIter::Value(e) => expr(e, out),
                ForIter::Range(a, b) => {
                    expr(a, out);
                    expr(b, out);
                }
            }
            nested(body, "for loop", out);
        }
        StmtKind::Try { body, catch, finally } => {
            nested(body, "try", out);
            if let Some((_, b)) = catch {
                nested(b, "try", out);
            }
            if let Some(b) = finally {
                nested(b, "try", out);
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
        FuncBody::Block(b) => match &f.name {
            Some(n) => nested(b, &format!("function {}", n.text), out),
            None => nested(b, "function", out),
        },
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

pub(crate) const PY_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else",
    "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
    "return", "try", "while", "with", "yield",
];

/// One part of a dotted Python module name (13.5): an identifier that isn't a keyword.
fn py_part(seg: &str) -> bool {
    let mut cs = seg.chars();
    cs.next().is_some_and(|c| c == '_' || c.is_alphabetic()) && cs.all(|c| c == '_' || c.is_alphanumeric()) && !PY_KEYWORDS.contains(&seg)
}

/// `opencv-python`, `my-helper`: a name of one part with `-` that is a module name with `_` for
/// each, which the worker refuses with a hint: an installed distribution's module, or a file of
/// yours to rename (13.5).
fn pip_style(name: &str) -> bool {
    name.contains('-') && py_part(&name.replace('-', "_"))
}

/// `lib.sub` for the relative folder `lib/sub`, when each folder's name is a `py_part`.
fn py_dotted(rel: &Path) -> Option<String> {
    let parts: Vec<String> = rel.iter().map(|c| c.to_string_lossy().into_owned()).collect();
    (!parts.is_empty() && parts.iter().all(|p| py_part(p))).then(|| parts.join("."))
}

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

/// A package that isn't installed (Python's `No module named 'x'`, Node's `Cannot find module
/// 'x'`): the hint says how to install it (7). V2 installs nothing itself.
/// `'chunk' is not exported by 'js."lodash/chunk"'`: and when the name is the package's own last
/// part, the form for a module that is one function (`module.exports = function`) (13.5).
fn not_exported(name: &str, text: &str, names: &[Rc<str>]) -> String {
    let base = format!("'{name}' is not exported by '{text}'");
    // `meen` for `mean`: the module's own names, which its worker sent (13.5).
    if let Some(near) = did_you_mean(name, names.iter().map(|n| &**n)) {
        return format!("{base} — did you mean '{near}'?");
    }
    let package = text.strip_prefix("js.").or_else(|| text.strip_prefix("ts.")).map(|p| p.trim_matches('"'));
    let stem = |last: &str| [".js", ".mjs", ".cjs", ".ts", ".mts", ".cts"].iter().find_map(|e| last.strip_suffix(e)).unwrap_or(last).to_string();
    match package.and_then(|p| p.rsplit('/').next()).map(stem) {
        Some(last) if last == name => format!("{base}; a module that is one function is imported with @import {{ \"default\" as {name} }} from {text}"),
        _ => base,
    }
}

/// `'f' is not exported by 'js.util'` while the script's own util.js sits next to it: `js.util`
/// is a package (Node's own, or one from npm), and a file of yours is imported by its path.
fn shadowed_hint(text: &str, dir: &Path) -> Option<String> {
    let (tag, name) = text.split_once('.')?;
    if !matches!(tag, "js" | "ts") || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return None;
    }
    let file = OWN_EXTS.iter().map(|e| format!("{name}.{e}")).find(|f| dir.join(f).is_file())?;
    Some(format!("{text} is a package (Node's own or one from npm), not your {file}: a file of yours is imported by its path, as in {}.\"./{file}\"", file_lang(&file)))
}

/// The extensions of a JavaScript or TypeScript file of your own.
const OWN_EXTS: [&str; 8] = ["js", "mjs", "cjs", "ts", "mts", "cts", "jsx", "tsx"];

/// `ts` for a TypeScript file's name (`.tsx` too), else `js`.
fn file_lang(file: &str) -> &'static str {
    if Path::new(file).extension().is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "ts" | "mts" | "cts" | "tsx")) {
        "ts"
    } else {
        "js"
    }
}

/// `js.name`, or `js."name"` when the name isn't a plain one (`js."util.js"`), as it is written.
fn written(name: &str) -> String {
    if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        format!("js.{name}")
    } else {
        format!("js.\"{name}\"")
    }
}

/// A `module` request's answer (13.5): the module or null, the names asked for that it has, the
/// ones it lacks, and, when it lacks some, what the reply says about them.
type ModuleAnswer = (Value, Vec<Export>, Vec<Rc<str>>, Have);

/// What a module's reply says when it lacks a name asked for (9.4): its public names, for the
/// did-you-mean, and the worker's hint (a dual package's, 13.5).
struct Have {
    names: Vec<Rc<str>>,
    hint: Option<Rc<str>>,
}

/// 13.3 step 6: `name` isn't among what the block exports, with the closest of those names.
/// `listed`: `exported` is the blocks' `@export` lists as written, where the typo may be. `star`:
/// the block's tag, when it has `@export { * }`, which leaves some names out (DOCS 6.1).
fn not_exported_by_block<'a>(name: &str, text: &str, exported: impl IntoIterator<Item = &'a str>, listed: bool, star: Option<&str>) -> String {
    match (did_you_mean(name, exported), star) {
        (Some(near), _) if listed => format!("'{name}' is not exported by '{text}' — did you mean '{near}'? (or should its @export say '{name}'?)"),
        (Some(near), _) => format!("'{name}' is not exported by '{text}' — did you mean '{near}'?"),
        (None, Some("py")) => format!("'{name}' is not exported by '{text}' (its @export {{ * }} exports only the functions and classes it defines, and no name that starts with _: list others by name, as in @export {{ {name} }})"),
        (None, Some(_)) => format!("'{name}' is not exported by '{text}' (its @export {{ * }} exports only functions, and no name that starts with _: list a const, let or class by name, as in @export {{ {name} }})"),
        (None, None) => format!("'{name}' is not exported by '{text}' (did the block @export it?)"),
    }
}

/// Whether a block's top level defines `name` as a function or a class (`def`, `class`; in
/// JavaScript also `function` and `const`/`let`/`var`), read from its lines as written.
fn defines(b: &EngineBlock, name: &str) -> bool {
    let py = canonical_tag(&b.tag.text) == "py";
    let forms: &[&str] = if py { &["def ", "async def ", "class "] } else { &["function ", "async function ", "class ", "const ", "let ", "var "] };
    b.body.lines().any(|line| {
        forms.iter().any(|f| {
            line.strip_prefix(f).and_then(|rest| rest.trim_start_matches('*').trim_start().strip_prefix(name)).is_some_and(|after| after.starts_with(['(', ':', ' ', '=', '{', '<']) || after.is_empty())
        })
    })
}

/// The tag of a block with `@export { * }`.
fn star_tag(b: &EngineBlock) -> Option<&str> {
    b.directives.exports.as_ref().filter(|x| x.star.is_some()).map(|_| canonical_tag(&b.tag.text))
}

/// Where an `@import` of a module is, for its hints (13.5).
struct Site<'a> {
    /// The importing .v2 file's folder, and that file as messages show it.
    dir: &'a Path,
    file: &'a str,
    /// The run's project root (18.1), in `dir`'s form (canonical), and whether a v2.toml is there.
    root: &'a Path,
    toml: bool,
}

/// Whom an `ImportError`'s install line is for (13.5).
enum PipFor {
    /// The Python V2 was told to use (a venv, a command, V2_ENGINE_PY): its argv.
    Told(Vec<std::ffi::OsString>),
    /// One found on PATH while the activated venv (VIRTUAL_ENV) has no python that runs: that
    /// venv, and what is wrong with it (`resolve::activated_venv_problem`).
    Passed(PathBuf, String),
    /// One found on PATH in a venv or a conda environment (an activated one): its argv, and that
    /// venv or environment, which V2 uses.
    OnPath(Vec<std::ffi::OsString>, PathBuf),
    /// Python's embeddable package without pip (`resolve::pth_python`): whether V2 was told
    /// to use it (a command, V2_ENGINE_PY), else it was found on PATH.
    Embeddable(bool),
}

/// What the install line of an engine's `ImportError` is for: the Python V2 was told to use; for
/// one found on PATH, the activated venv (`active`, VIRTUAL_ENV's value) when the search couldn't
/// use its python, else the venv that Python is in, if any. Python's embeddable package, either
/// way, while it has no pip (in its `Lib\site-packages`, where get-pip.py puts it).
fn pip_for(engine: &crate::engines::resolve::ResolvedEngine, active: Option<PathBuf>) -> Option<PipFor> {
    let pth = engine.argv.first().and_then(|p| crate::engines::resolve::pth_python(Path::new(p), engine.version));
    let embedded = pth.is_some_and(|p| p.package() && !p.pip);
    if engine.origin != crate::engines::resolve::Origin::Path {
        return Some(if embedded { PipFor::Embeddable(true) } else { PipFor::Told(engine.argv.clone()) });
    }
    let passed = active.and_then(|venv| {
        let l = crate::engines::resolve::Lookup::from_env();
        crate::engines::resolve::activated_venv_problem(&venv, &engine.skipped, &l).map(|problem| PipFor::Passed(venv, problem))
    });
    passed.or_else(|| {
        if embedded {
            return Some(PipFor::Embeddable(false));
        }
        let program = Path::new(engine.argv.first()?);
        let venv = crate::engines::resolve::venv_of(program).or_else(|| conda_prefix(program))?;
        Some(PipFor::OnPath(engine.argv.clone(), venv))
    })
}

/// The conda environment a python is in (an activated one found on PATH): its prefix, which
/// holds `conda-meta`, above `bin/python` (on Windows, beside `python.exe`).
fn conda_prefix(python: &Path) -> Option<PathBuf> {
    let dir = python.parent()?;
    let prefix = if cfg!(windows) { dir } else { dir.parent()? };
    prefix.join("conda-meta").is_dir().then(|| prefix.to_path_buf())
}

/// After "install it for the Python V2 uses": a command that works for `argv`, the Python V2 was
/// told to use. Its own pip (the whole command, each word quoted as a shell needs it: `pyenv exec
/// python -m pip install x`); `uv add x` for a command that runs uv (`uv run python`); for a venv
/// uv made, which has no pip, `uv add x` in its project, else `uv pip install --python DIR x`; for
/// another venv without pip, `-m ensurepip` first, or, when its Python has no ensurepip (Debian
/// and Ubuntu without python3-venv), the venv made again.
fn install_line(argv: &[std::ffi::OsString], name: &str) -> String {
    let words: Vec<String> = argv.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let program = Path::new(words.first().map_or("", String::as_str));
    let uv = program.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("uv"));
    if uv && words.get(1).is_some_and(|w| w == "run") {
        return format!(": uv add {name}");
    }
    let quoted: Vec<String> = words.iter().map(|w| crate::cli::shell_word(w)).collect();
    // PowerShell runs a quoted path only after `&`.
    let amp = if cfg!(windows) && quoted.first().is_some_and(|w| w.starts_with('"')) { "& " } else { "" };
    let run = format!("{amp}{}", quoted.join(" "));
    match crate::engines::resolve::venv_of(program).filter(|_| words.len() == 1) {
        Some(venv) if crate::engines::resolve::made_by_uv(&venv) => match crate::engines::resolve::uv_project(&venv) {
            Some(project) => format!(": uv add {name} in {}", project.display()),
            None => format!(": uv pip install --python {} {name}", crate::cli::shell_word(&venv.display().to_string())),
        },
        Some(venv) if !crate::engines::resolve::venv_has_pip(&venv) && !crate::engines::resolve::base_has_ensurepip(&venv) => no_ensurepip_line(&venv, &run, name),
        Some(venv) if !crate::engines::resolve::venv_has_pip(&venv) => format!(", whose venv has no pip: {run} -m ensurepip, then {run} -m pip install {name}"),
        _ => format!(": {run} -m pip install {name}"),
    }
}

/// The package on PyPI that provides the module `name`, for the well-known ones named otherwise
/// (13.5): `pip install yaml` finds nothing, and `pip install slugify` installs another,
/// Python 2 package.
fn pypi_name(name: &str) -> &str {
    match name {
        "yaml" => "PyYAML",
        "PIL" => "Pillow",
        "cv2" => "opencv-python",
        "sklearn" => "scikit-learn",
        "bs4" => "beautifulsoup4",
        "slugify" => "python-slugify",
        "dateutil" => "python-dateutil",
        "docx" => "python-docx",
        "dotenv" => "python-dotenv",
        "jwt" => "PyJWT",
        "serial" => "pyserial",
        "Crypto" => "pycryptodome",
        other => other,
    }
}

/// `install_line` for a venv without pip whose Python has no ensurepip to add it: Debian and
/// Ubuntu leave it out without python3-venv, and `python3 -m venv` then stops after making a venv
/// without pip. That package, then the venv made again. `run`: its python, as pasted.
fn no_ensurepip_line(venv: &Path, run: &str, name: &str) -> String {
    let dir = crate::cli::shell_word(&venv.display().to_string());
    let first = if cfg!(windows) {
        ": ".to_string()
    } else {
        let sudo = crate::engines::spec::this_linux().1;
        format!(" (on Debian and Ubuntu: {}): then ", crate::engines::spec::package_line(crate::engines::spec::Family::Apt, sudo, "python3-venv", "python3", None))
    };
    // With the Python it was made from, else py (python3) when PATH has it (7.6).
    let maker = crate::engines::resolve::venv_maker(Some(venv), &crate::engines::resolve::Lookup::from_env());
    let make = crate::engines::resolve::with_maker(maker, ", ", &format!("--clear {dir}"));
    format!(", whose venv has no pip, and its Python has no ensurepip to add it{first}make the venv again{make}, and install its packages, as in {run} -m pip install {name}")
}

/// The hint of a module that wasn't found (13.5), from the engine's own message: what to check,
/// or what to install, from the folder of the importing .v2 file.
/// `python`: the Python V2 was told to use (a venv, a command, V2_ENGINE_PY), for the exact
/// install command, the activated venv V2 couldn't use, or the venv of one found on PATH; `None`
/// for another found on PATH.
fn missing_package_hint(message: &str, site: &Site, python: Option<&PipFor>) -> Option<String> {
    let dir = site.dir;
    let venv_set = matches!(python, Some(PipFor::Told(_) | PipFor::Embeddable(true)));
    // A package whose `exports` offer only `import`, which `require` can't load: a block can.
    if let Some(rest) = message.split("No \"exports\" main defined in ").nth(1) {
        let parts: Vec<String> = Path::new(rest.trim()).iter().map(|c| c.to_string_lossy().into_owned()).collect();
        let at = parts.iter().rposition(|c| c == "node_modules")?;
        let package = parts[at + 1..parts.len().saturating_sub(1)].join("/");
        return Some(format!(
            "'{package}' can be loaded only with import(), which a @js block can do: async function load() {{ return await import(\"{package}\") }}, then @export {{ load }} and call load()"
        ));
    }
    let quoted = |after: &str| message.split(after).nth(1).and_then(|r| r.split('\'').next()).map(str::to_string);
    if let Some(name) = quoted("No module named '") {
        // Python names the first part it couldn't find: a dotted one means the rest was there.
        if let Some((parent, part)) = name.rsplit_once('.') {
            return Some(if message.contains("is not a package") {
                format!("'{parent}' is a module, not a package, so it has no '{part}': check the name after '{parent}.'")
            } else {
                format!("'{parent}' is there, but has no '{part}': check the name after '{parent}.'")
            });
        }
        // A helper of the project's own in a folder below: `py.stages.clean` for stages/clean.py.
        if let Some((file, dotted)) = module_below(dir, &name) {
            return Some(match dotted {
                Ok(dotted) => format!("there is a {file} in a folder below: import it as py.{dotted}"),
                Err(folder) => format!(
                    "there is a {file} in a folder below, but {folder} isn't a Python name, so no py. name reaches it: rename the folder, or put it on sys.path in a @py block first"
                ),
            });
        }
        // A .v2 file of another project's that has the module: the entry script's project serves
        // the whole run (18.1).
        if let Some(own) = crate::config::find_root(dir).filter(|r| r != site.root && project_has(r, &name)) {
            return Some(format!(
                "{} belongs to another project ({}, with its own v2.toml), but a run uses the entry script's project ({}): run it from that project, or put the module where this one finds it",
                site.file,
                own.display(),
                site.root.display()
            ));
        }
        // One in a folder above, which isn't on Python's path (the run's root is).
        if let Some(above) = dir.ancestors().skip(1).take(3).map(|d| d.join(format!("{name}.py"))).find(|p| p.is_file()) {
            let folder = above.parent().unwrap_or(dir);
            // In a folder of the project, below its root: `py.lib.rooth`, when that is a name.
            if let Some(dotted) = folder.strip_prefix(site.root).ok().filter(|_| folder != site.root).and_then(py_dotted) {
                return Some(format!("there is a {} in a folder above: from the project folder, {}, it is py.{dotted}.{name}", above.display(), site.root.display()));
            }
            if !site.toml && site.root.starts_with(folder) {
                // The entry script's folder or one above it: a v2.toml there makes it the root.
                return Some(format!(
                    "there is a {} in a folder above: a v2.toml in {} (it may be empty) makes that the project folder, where py.{name} is found",
                    above.display(),
                    folder.display()
                ));
            }
            // Above the run's project, for a .v2 file in it: likely another module than the one
            // meant, so the install line below (review of round 18).
            let above_project = site.root.starts_with(folder) && dir.starts_with(site.root);
            if folder != site.root && !above_project {
                return Some(format!(
                    "there is a {} in a folder above, which isn't on Python's path: move it next to the .v2 file or into the project folder, or put its folder on sys.path in a @py block first",
                    above.display()
                ));
            }
        }
        let package = pypi_name(&name);
        let install = match python {
            Some(PipFor::Told(argv) | PipFor::OnPath(argv, _)) => format!("if '{name}' isn't misspelled, install it for the Python V2 uses{}", install_line(argv, package)),
            // Not another Python's pip: the activated venv, which V2 couldn't use, is the one meant.
            Some(PipFor::Passed(_, problem)) => problem.clone(),
            // No pip, and no venv module to make one with.
            Some(PipFor::Embeddable(_)) => "this is Python's embeddable package, which has no pip: for packages, use a full Python (python.org's installer, or the nuget package) and a venv".to_string(),
            None if package != name => format!("if '{name}' isn't misspelled, install its package, {package}, with pip for the Python that v2 doctor shows; README.md, \"Python and npm packages\", has the steps"),
            None => format!("if '{name}' isn't misspelled, install it with pip for the Python that v2 doctor shows; README.md, \"Python and npm packages\", has the steps"),
        };
        // A virtual environment made in the project (the file's folder or up to three above it),
        // but not named in a v2.toml.
        let venv = (!venv_set)
            .then(|| dir.ancestors().take(4).flat_map(|d| [d.join(".venv"), d.join("venv")]).find(|v| v.join("pyvenv.cfg").is_file()))
            .flatten();
        let same = |a: &Path, b: &Path| a == b || std::fs::canonicalize(a).ok().is_some_and(|a| std::fs::canonicalize(b).ok() == Some(a));
        return Some(match (venv, python) {
            // That one is the activated venv V2 couldn't use: what is wrong with it.
            (Some(v), Some(PipFor::Passed(active, _))) if same(&v, active) => install,
            // That one is the venv of the Python V2 uses (found on PATH): its own install line.
            (Some(v), Some(PipFor::OnPath(_, used))) if same(&v, used) => install,
            (Some(v), _) => format!(
                "there is a virtual environment in {}, which V2 isn't using: name it in a v2.toml next to it, [python] venv = \"{}\"; {} {install}",
                v.display(),
                v.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
                if matches!(python, Some(PipFor::Passed(..) | PipFor::Embeddable(_))) { "and" } else { "or" }
            ),
            (None, _) => install,
        });
    }
    let name = quoted("Cannot find module '")?;
    if name.starts_with(['.', '/']) || name.contains(':') {
        // `ts."./money"` for money.ts: Node finds a file by its whole name.
        let relative = name.starts_with("./") || name.starts_with("../");
        if let Some(ext) = OWN_EXTS.iter().find(|e| relative && dir.join(format!("{name}.{e}")).is_file()) {
            let file = format!("{name}.{ext}");
            return Some(format!("did you mean {}.\"{file}\"? a file is named with its extension", file_lang(&file)));
        }
        return Some("a local file is found from the .v2 file's folder: check the path".to_string());
    }
    Some(npm_hint(&name, dir, None))
}

/// The hint of a bare specifier `name` Node didn't find (13.5), from the importing .v2 file's
/// folder `dir`, where Node starts looking: an installed package without that subpath, a
/// tsconfig `paths` alias, a file or folder of the script's own (by its path, never `npm
/// install`), Yarn PnP, else the install line. `call`: `require` or `import` for a block's own
/// call, in its words; `None` for an `@import`.
fn npm_hint(name: &str, dir: &Path, call: Option<&str>) -> String {
    // The package is the first part of the specifier (`@scope/name` for a scoped one).
    let mut parts = name.splitn(3, '/');
    let package = match (parts.next(), parts.next()) {
        (Some(scope), Some(rest)) if scope.starts_with('@') => format!("{scope}/{rest}"),
        (Some(first), _) => first.to_string(),
        _ => name.to_string(),
    };
    let modules: Vec<PathBuf> = dir.ancestors().map(|d| d.join("node_modules")).filter(|m| m.is_dir()).collect();
    // A package that is a Linux symlink (npm link, workspaces) seen through \\wsl.localhost, which
    // Windows' Node can't follow: so, not a folder of the script's own nor a missing package.
    if let Some(link) = modules.iter().map(|m| m.join(&package)).find(|p| crate::runtime::module::wsl_file(p) == Some(crate::runtime::module::WslFile::Link)) {
        return crate::runtime::module::wsl_link_line(&link);
    }
    if package != name && modules.iter().any(|m| m.join(&package).is_dir()) {
        let rest = name[package.len()..].trim_start_matches('/');
        return format!("'{package}' is installed, but has no '{rest}': check the path after '{package}/'");
    }
    // `@/lib`: an alias of the nearest tsconfig.json, which only TypeScript's own tools read.
    if let Some(hint) = alias_hint(name, dir) {
        return hint;
    }
    // A file of the script's own by its path, as the hint writes it: `require("./x.js")` in a
    // block, `js."./x.js"` for an @import; and the specifier as it was given.
    let by_path = |path: &str| match call {
        Some(call) => format!("{call}(\"{path}\")"),
        None => format!("{}.\"{path}\"", file_lang(path)),
    };
    let (loaded, statement) = if call.is_some() { ("loaded", "") } else { ("imported", "@import { … } from ") };
    let unnamed = format!("{}, without ./, names an npm package", if call.is_some() { format!("\"{name}\"") } else { written(name) });
    // `js.helpers` for the script's own helpers.js, `lib/math` for lib/math.ts: a file is imported
    // by its path (its folders kept), and `npm install helpers` would fetch whatever package has
    // that name.
    let with_ext = Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| OWN_EXTS.contains(&e.to_ascii_lowercase().as_str()));
    let own: Vec<String> = if with_ext { vec![name.to_string()] } else { OWN_EXTS.iter().map(|e| format!("{name}.{e}")).collect() };
    if let Some(file) = own.iter().find(|f| dir.join(f).is_file()) {
        return format!("your own {file} is {loaded} by its path from the .v2 file's folder: {statement}{} ({unnamed})", by_path(&format!("./{file}")));
    }
    // `lib/…` (`lib` alone from an import(), whose message names the package part) where lib is a
    // folder next to the .v2 file: the script's own code, never `npm install lib`.
    let first = name.split('/').next().unwrap_or(name);
    let folder = !first.is_empty() && dir.join(first).is_dir();
    // Yarn Plug'n'Play keeps packages where only Yarn's loader finds them, with no node_modules.
    let pnp = dir.ancestors().flat_map(|d| [d.join(".pnp.cjs"), d.join(".pnp.js")]).find(|p| p.is_file()).map(|p| {
        format!("this project uses Yarn Plug'n'Play ({}), which Node can't read without Yarn's loader: set nodeLinker: node-modules in .yarnrc.yml and run yarn install", p.display())
    });
    // The packages of the nearest node_modules: `@scope/name` for a scope's, none of its dot
    // entries (.bin, .package-lock.json), which are no packages (the third review of round 19).
    let entries = |m: &Path| -> Vec<(String, PathBuf)> {
        std::fs::read_dir(m).into_iter().flatten().flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).filter(|(n, _)| !n.starts_with('.')).collect()
    };
    let installed: Vec<String> = modules
        .first()
        .map(|m| entries(m))
        .unwrap_or_default()
        .into_iter()
        .flat_map(|(n, path)| if n.starts_with('@') { entries(&path).into_iter().map(|(s, _)| format!("{n}/{s}")).collect() } else { vec![n] })
        .collect();
    // The did-you-mean names what is written: `js.lodash` for an @import, `"lodash"` in a block.
    let near = did_you_mean(&package, installed.iter().map(String::as_str)).map(|n| if call.is_some() { format!("did you mean \"{n}\"? ") } else { format!("did you mean {}? ", written(n)) });
    // A name that may be the script's own or an npm package's: for the package of that name,
    // Yarn's fix or the install line, after the package it nearly spells when there is one, as the
    // general hint words it (the reviews of round 19: never `npm install` a typo unsaid).
    let or_package = || match (&near, &pnp) {
        (Some(near), Some(pnp)) => format!("; {near}if '{package}' isn't misspelled: {pnp}"),
        (Some(near), None) => format!("; {near}if '{package}' isn't misspelled, npm install {package} in {}", dir.display()),
        (None, Some(pnp)) => format!("; or, for the npm package {package}: {pnp}"),
        (None, None) => format!("; or, for the npm package {package}, npm install {package} in {}", dir.display()),
    };
    if with_ext {
        // `chart.js` may be an npm package's own name: that too, unless the first part is a folder
        // of the script's own.
        let npm = if folder { String::new() } else { or_package() };
        return format!("a file of your own is {loaded} by its path from the .v2 file's folder, as in {}: check its name ({unnamed}){npm}", by_path(&format!("./{name}")));
    }
    if folder {
        let example = if name.contains('/') { format!("./{name}") } else { format!("./{name}/…") };
        // `require("config")` beside a config folder of data (node-config's own layout, Cypress's,
        // Prisma's): the npm package too, unless the folder holds code of the script's own, or the
        // name came from import(), whose message names only the package part (the reviews of
        // round 19).
        let npm = if name == first && call != Some("import") && !holds_code(&dir.join(first), 2) { or_package() } else { String::new() };
        return format!("{first} is a folder of your own, whose files are {loaded} by their paths from the .v2 file's folder, as in {} ({unnamed}){npm}", by_path(&example));
    }
    if let Some(pnp) = pnp {
        return pnp;
    }
    // Node looks in node_modules from the importing .v2 file's folder up, which for a shared
    // module isn't the project's folder.
    format!(
        "{}if '{package}' isn't misspelled, install it with npm install {package} in {} or a folder above it, where Node looks for this file",
        near.unwrap_or_default(),
        dir.display()
    )
}

/// Whether the folder `f` holds code of the script's own (13.5): a package.json, or a file with
/// one of `OWN_EXTS` in it or in its folders `depth` levels down (node_modules and hidden folders
/// aside; at most 500 entries a folder).
fn holds_code(f: &Path, depth: usize) -> bool {
    if f.join("package.json").is_file() {
        return true;
    }
    std::fs::read_dir(f).into_iter().flatten().flatten().take(500).any(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            depth > 0 && !name.starts_with('.') && name != "node_modules" && holds_code(&e.path(), depth - 1)
        } else {
            Path::new(&*name).extension().and_then(|x| x.to_str()).is_some_and(|x| OWN_EXTS.contains(&x.to_ascii_lowercase().as_str()))
        }
    })
}

/// Whether the project at `root` has the Python module `name` (13.5): a `name.py` or `name`
/// folder at its root, or an entry of that name in the site-packages of the venv its v2.toml
/// names (`name/`, `name.py`, `name.….pyd`, `name-1.0.dist-info`).
fn project_has(root: &Path, name: &str) -> bool {
    if root.join(format!("{name}.py")).is_file() || root.join(name).is_dir() {
        return true;
    }
    let Some(venv) = crate::config::load(root).ok().and_then(|c| c.python_venv).map(|v| v.value) else { return false };
    venv_has_module(&venv, name)
}

/// Whether the site-packages of `venv` has an entry of the Python module `name` (13.5): `name/`,
/// `name.py`, `name.….pyd`, `name-1.0.dist-info`.
fn venv_has_module(venv: &Path, name: &str) -> bool {
    // Lib\site-packages on Windows, lib/python3.X/site-packages elsewhere.
    let unix = std::fs::read_dir(venv.join("lib")).into_iter().flatten().flatten().map(|e| e.path().join("site-packages"));
    let entry = |e: std::fs::DirEntry| e.file_name().to_string_lossy().split(['.', '-']).next().is_some_and(|n| n.eq_ignore_ascii_case(name));
    std::iter::once(venv.join("Lib").join("site-packages")).chain(unix).any(|sp| std::fs::read_dir(sp).into_iter().flatten().flatten().any(entry))
}

/// 13.5: when `V2_ENGINE_PY` wins over the project's `[python] venv` (`unused`, from
/// `resolve::unused_venv`) and that venv has the module Python didn't find, that comes first in
/// the hint, then `hint`, joined by `; or ` (`; and ` for what is true either way: `and`).
fn unused_venv_hint(message: &str, unused: &(PathBuf, String), hint: Option<String>, and: bool) -> Option<String> {
    let name = message.split("No module named '").nth(1).and_then(|r| r.split('\'').next());
    if !name.is_some_and(|n| !n.contains('.') && venv_has_module(&unused.0, n)) {
        return hint;
    }
    Some(match hint {
        Some(h) => format!("{}; {} {h}", unused.1, if and { "and" } else { "or" }),
        None => unused.1.clone(),
    })
}

/// `stages/clean.py` and `Ok("stages.clean")` when `dir`'s folders, up to three levels down,
/// hold that file (the nearest first), or `Err("2024")`, the first folder whose name is no
/// Python name, when only such a path has one. Hidden folders, virtual environments and
/// node_modules aren't looked in.
fn module_below(dir: &Path, name: &str) -> Option<(String, Result<String, String>)> {
    let mut level: Vec<(PathBuf, Vec<String>)> = vec![(dir.to_path_buf(), Vec::new())];
    let mut seen = 0;
    let mut unnamed = None;
    for _ in 0..3 {
        let mut next = Vec::new();
        for (folder, parts) in level {
            for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                seen += 1;
                if seen > 5000 {
                    return unnamed;
                }
                let Ok(sub) = entry.file_name().into_string() else { continue };
                let skip = sub.starts_with('.') || matches!(sub.as_str(), "node_modules" | "venv" | "__pycache__" | "site-packages");
                if skip || !entry.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let mut parts = parts.clone();
                parts.push(sub);
                if entry.path().join(format!("{name}.py")).is_file() {
                    let file = format!("{}/{name}.py", parts.join("/"));
                    // As module_unit checks a dotted name: an identifier that isn't a keyword.
                    match parts.iter().find(|p| !py_part(p)) {
                        None => return Some((file, Ok(format!("{}.{name}", parts.join("."))))),
                        Some(bad) => {
                            unnamed = unnamed.or_else(|| Some((file, Err(bad.clone()))));
                        }
                    }
                }
                next.push((entry.path(), parts));
            }
        }
        next.sort();
        level = next;
    }
    unnamed
}

/// A local file that uses `import`/`export` in a folder whose package.json says `"type":
/// "commonjs"` (what `npm init -y` writes), which turns off Node's own detection (13.5).
fn commonjs_hint(message: &str, spec: &str, dir: &Path) -> Option<String> {
    let esm = message.contains("Unexpected token 'export'") || message.contains("Cannot use import statement outside a module");
    let local = spec.trim_matches('"');
    if !esm || !(local.starts_with("./") || local.starts_with("../")) {
        return None;
    }
    let file = absolute_clean(&dir.join(local));
    let pkg = commonjs_package(&file)?;
    let ts = file.extension().is_some_and(|e| e.to_string_lossy().to_ascii_lowercase().ends_with("ts"));
    Some(format!(
        "{} says \"type\": \"commonjs\" (npm init writes that since npm 11), so Node loads this file as CommonJS, without import and export: set \"type\": \"module\" there, or rename the file to .{}",
        pkg.display(),
        if ts { "mts" } else { "mjs" }
    ))
}

/// The nearest package.json above `file`, when it says `"type": "commonjs"`.
fn commonjs_package(file: &Path) -> Option<PathBuf> {
    let pkg = file.ancestors().skip(1).map(|d| d.join("package.json")).find(|p| p.is_file())?;
    let text = std::fs::read_to_string(&pkg).ok()?;
    let json: Json = serde_json::from_str(&text).ok()?;
    (json.get("type").and_then(Json::as_str) == Some("commonjs")).then_some(pkg)
}

/// `import { add } from './vec.ts'` in an ES module V2 imported, where package.json makes vec.ts
/// CommonJS: Node says vec.ts has no export named 'add' (13.5, round 16).
fn nested_commonjs_hint(message: &str, spec: &str, dir: &Path) -> Option<String> {
    let module = message.split("The requested module '").nth(1)?.split('\'').next()?;
    if !(module.starts_with("./") || module.starts_with("../")) {
        return None;
    }
    let importer = absolute_clean(&dir.join(spec.trim_matches('"')));
    let file = absolute_clean(&importer.parent()?.join(module));
    let ext = file.extension()?.to_string_lossy().to_ascii_lowercase();
    if !matches!(ext.as_str(), "js" | "ts") {
        return None;
    }
    let pkg = commonjs_package(&file)?;
    Some(format!(
        "{} says \"type\": \"commonjs\", so Node loads {module} as CommonJS, whose exports an import can't name: set \"type\": \"module\" there, or rename it to .{}",
        pkg.display(),
        if ext == "ts" { "mts" } else { "mjs" }
    ))
}

/// A TypeScript file's own import that Node couldn't follow (13.5): Node runs a .ts file as it
/// is written, so imports between .ts files name the .ts file, and types are imported with
/// `import type` (they are gone when it runs).
fn ts_import_hint(message: &str) -> Option<String> {
    let typescript = |p: &str| ts_file(Path::new(p));
    if let Some(hint) = imported_alias_hint(message) {
        return Some(hint);
    }
    if let Some(rest) = message.split("Cannot find module '").nth(1) {
        let (missing, rest) = rest.split_once('\'')?;
        let importer = rest.strip_prefix(" imported from ")?.lines().next()?.trim();
        if !typescript(importer) {
            return None;
        }
        let missing = Path::new(missing);
        let as_js = matches!(missing.extension().and_then(|e| e.to_str()), Some("js" | "mjs" | "cjs") | None);
        let ts = ["ts", "mts", "cts"].iter().map(|e| missing.with_extension(e)).find(|p| p.is_file()).filter(|_| as_js)?;
        let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Some(format!(
            "Node runs .ts files as they are written, so imports between them name the .ts file: write {} for {} in {}",
            name(&ts),
            name(missing),
            name(Path::new(importer))
        ));
    }
    let export = message.split("does not provide an export named '").nth(1)?.split('\'').next()?;
    let module = message.split("The requested module '").nth(1)?.split('\'').next()?;
    typescript(module).then(|| {
        format!("if '{export}' is a type or an interface, import it with import type {{ {export} }} from '{module}': Node removes types as it runs a .ts file, so they aren't exports")
    })
}

/// A `.ts`, `.mts` or `.cts` file.
fn ts_file(p: &Path) -> bool {
    p.extension().is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "ts" | "mts" | "cts"))
}

/// `@import { Shape } from ts."./types.ts"` where types.ts declares Shape as a type or an
/// interface (`export type Shape`, `export interface Shape`, read from its lines), which
/// TypeScript removes before the file runs (13.5).
fn ts_type_hint(name: &str, module: &str, dir: &Path) -> Option<String> {
    let relative = module.starts_with("./") || module.starts_with("../") || (cfg!(windows) && (module.starts_with(".\\") || module.starts_with("..\\")));
    if !relative && !Path::new(module).is_absolute() {
        return None;
    }
    let file = absolute_clean(&dir.join(module));
    if !ts_file(&file) {
        return None;
    }
    let text = std::fs::read_to_string(&file).ok()?;
    /// What follows `word` and the space after it.
    fn after<'a>(s: &'a str, word: &str) -> Option<&'a str> {
        s.strip_prefix(word).filter(|r| r.starts_with(char::is_whitespace)).map(str::trim_start)
    }
    let declares = text.lines().any(|line| {
        let mut rest = line.trim_start();
        for word in ["export", "declare"] {
            rest = after(rest, word).unwrap_or(rest);
        }
        ["type", "interface"].iter().filter_map(|k| after(rest, k)).filter_map(|r| r.strip_prefix(name)).any(|r| !r.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$'))
    });
    declares.then(|| format!("{name} is a type, which TypeScript removes before it runs: there is nothing to import; use it in TypeScript only"))
}

/// A specifier a `paths` alias of the tsconfig.json nearest `dir` matches (13.5): `@/lib` for
/// `@/*`. Node doesn't read tsconfig.json.
fn alias_hint(spec: &str, dir: &Path) -> Option<String> {
    let (config, keys) = tsconfig_paths(dir)?;
    let key = keys.iter().find(|k| alias_matches(k, spec))?;
    Some(format!(
        "\"{key}\" is a \"paths\" alias in {}, and Node doesn't read tsconfig paths: import by a relative path, or use package.json \"imports\" (#alias)",
        config.display()
    ))
}

/// A TypeScript file's import of an alias: `Cannot find package '@lib/math' imported from
/// …\paths.ts` (Node names a bare specifier's package part, `~` for `~/utils`), or a `.cts`
/// file's `Cannot find module '~/utils'` with the file first on its require stack.
fn imported_alias_hint(message: &str) -> Option<String> {
    let (spec, rest) = ["Cannot find package '", "Cannot find module '"].iter().find_map(|p| message.split(p).nth(1))?.split_once('\'')?;
    let importer = match rest.strip_prefix(" imported from ") {
        Some(r) => r.lines().next()?.trim(),
        None => rest.split("Require stack:").nth(1)?.lines().find_map(|l| l.trim().strip_prefix("- "))?.trim(),
    };
    let importer = Path::new(importer);
    if spec.starts_with('.') || !ts_file(importer) {
        return None;
    }
    alias_hint(spec, importer.parent()?)
}

/// A block's own `require()` (`MODULE_NOT_FOUND`, the .v2 file first on its `Require stack`) or
/// `import()` (`ERR_MODULE_NOT_FOUND`, `imported from` the .v2 file) of a bare specifier that Node
/// didn't find (13.5): the hints an `@import` of it gets (`npm_hint`), from the .v2 file's folder,
/// where a block's `require` looks, in the block's words: an installed package without that
/// subpath, a `paths` alias of tsconfig.json, a file or folder of the script's own (by its path,
/// as in `require("./lib/helpers.js")`), Yarn PnP, `npm install`. `code`: the error's (its data).
pub(crate) fn block_module_hint(message: &str, code: &str) -> Option<String> {
    let (spec, importer, call) = match code {
        "MODULE_NOT_FOUND" => {
            let (spec, rest) = message.split("Cannot find module '").nth(1)?.split_once('\'')?;
            (spec, rest.split("Require stack:").nth(1)?.lines().find_map(|l| l.trim().strip_prefix("- "))?.trim(), "require")
        }
        // Node names a bare specifier's package part (`@/lib` for `@/lib/math`).
        "ERR_MODULE_NOT_FOUND" => {
            let (spec, rest) = message.split("Cannot find package '").nth(1)?.split_once('\'')?;
            (spec, rest.strip_prefix(" imported from ")?.lines().next()?.trim(), "import")
        }
        _ => return None,
    };
    let importer = Path::new(importer);
    let block = importer.extension().is_some_and(|e| e.eq_ignore_ascii_case("v2"));
    if !block || spec.is_empty() || spec.starts_with(['.', '/', '\\']) || spec.contains(':') {
        return None;
    }
    // As for an @import, in the block's words.
    Some(npm_hint(spec, importer.parent()?, Some(call)))
}

/// Whether the `paths` key `key` (`@lib/*`, `~/*`, `utils`) can match a specifier that is
/// `spec` or starts with `spec/` (Node names a package part); never the catch-all `*`.
fn alias_matches(key: &str, spec: &str) -> bool {
    let within = |k: &str| k.starts_with(&format!("{spec}/"));
    match key.split_once('*') {
        Some(("", _)) => false,
        Some((pre, _)) => spec.starts_with(pre) || within(pre),
        None => key == spec || within(key),
    }
}

/// The `compilerOptions.paths` keys of the tsconfig.json nearest `dir` (its folder or one above),
/// or of what it extends by a relative path, with the file that has them.
fn tsconfig_paths(dir: &Path) -> Option<(PathBuf, Vec<String>)> {
    let mut file = dir.ancestors().map(|d| d.join("tsconfig.json")).find(|p| p.is_file())?;
    for _ in 0..8 {
        let json: Json = serde_json::from_str(&plain_json(&std::fs::read_to_string(&file).ok()?)).ok()?;
        if let Some(paths) = json.pointer("/compilerOptions/paths").and_then(Json::as_object) {
            return Some((file, paths.keys().cloned().collect()));
        }
        let base = json.get("extends").and_then(Json::as_str).filter(|e| e.starts_with("./") || e.starts_with("../"))?;
        let next = absolute_clean(&file.parent()?.join(base));
        file = if next.is_file() { next } else { PathBuf::from(format!("{}.json", next.display())) };
    }
    None
}

/// tsconfig.json's text (comments, trailing commas, a byte order mark) as plain JSON.
fn plain_json(text: &str) -> String {
    // Each pass keeps strings as they are: `skip(chars, i)` is where the string at `i` ends.
    let skip = |chars: &[char], mut i: usize| {
        i += 1;
        while i < chars.len() && chars[i] != '"' {
            i += if chars[i] == '\\' { 2 } else { 1 };
        }
        (i + 1).min(chars.len())
    };
    let chars: Vec<char> = text.trim_start_matches('\u{feff}').chars().collect();
    let mut bare = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        match (chars[i], chars.get(i + 1)) {
            ('"', _) => {
                let end = skip(&chars, i);
                bare.extend_from_slice(&chars[i..end]);
                i = end;
            }
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
                bare.push(' ');
            }
            (c, _) => {
                bare.push(c);
                i += 1;
            }
        }
    }
    let mut out = String::with_capacity(bare.len());
    let mut i = 0;
    while i < bare.len() {
        match bare[i] {
            '"' => {
                let end = skip(&bare, i);
                out.extend(&bare[i..end]);
                i = end;
                continue;
            }
            ',' if matches!(bare[i + 1..].iter().find(|c| !c.is_whitespace()), Some('}' | ']')) => {}
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// The imported names as an `@export { … }` list would write them.
fn names_text(list: &[ImportItem]) -> String {
    list.iter().map(|i| i.name.to_string()).collect::<Vec<_>>().join(", ")
}

fn named_items(items: &[ImportItem]) -> Vec<Item> {
    items.iter().map(|i| (i.name.clone(), i.alias.as_ref().map_or_else(|| i.name.clone(), |a| a.text.clone()), i.span)).collect()
}

impl Interp {
    /// The selector as written, for messages (13.1).
    /// Where a selector is written: the caret for its own errors (2.2).
    fn selector_span(&self, sel: &Selector) -> Span {
        match sel {
            Selector::Union { tag } => tag.span,
            Selector::Exact { tag, label } => tag.span.to(label.span),
            Selector::Label { label } => label.span,
            Selector::Module { lang, path } => lang.span.to(path.last().map_or(lang.span, |p| p.span)),
            Selector::V2 { span } => *span,
        }
    }

    /// A block selector's own errors, before any name's: an unknown tag, or nothing it matches
    /// (13.3 steps 1-2), at the selector.
    fn check_selector(&self, file: FileId, sel: &Selector, names: &str) -> Exec<()> {
        let at = self.selector_span(sel);
        if self.candidates(file, sel).map_err(|f| self.raise(f, at))?.is_empty() {
            return Err(self.raise(import_error(self.no_exports(file, sel, Some(names))), at));
        }
        Ok(())
    }

    /// A selector with no candidates (13.3 step 2): a block with that label that has no
    /// `@export` (an action block) says what to add; otherwise nothing matches.
    fn no_exports(&self, file: FileId, sel: &Selector, name: Option<&str>) -> String {
        let (label, tag) = match sel {
            Selector::Label { label } => (Some(&label.text), None),
            Selector::Exact { tag, label } => (Some(&label.text), Some(canonical_tag(&tag.text))),
            _ => (None, None),
        };
        let bodies = &self.files[&file].bodies;
        let same_engine = |b: &EngineBlock| tag.is_none_or(|t| canonical_tag(&b.tag.text) == t);
        let action = label.filter(|l| {
            bodies.iter().any(|b| b.directives.exports.is_none() && b.label.as_ref().is_some_and(|n| &n.text == *l) && same_engine(b))
        });
        if let Some(l) = action {
            return format!("block '{l}' exports nothing: add @export {{ {} }} inside it", name.unwrap_or("…"));
        }
        let Some(l) = label else {
            // `@py`: no block of that engine has an @export.
            return format!("no {} block in this file has an @export", self.selector_text(sel));
        };
        let what = match tag {
            Some(t) => format!("no @{t} block in this file is labeled '{l}'"),
            None => format!("no block in this file is labeled '{l}'"),
        };
        let labels: Vec<&str> = bodies.iter().filter(|b| same_engine(b)).filter_map(|b| b.label.as_ref().map(|n| &*n.text)).collect();
        if let Some(close) = did_you_mean(l, labels) {
            return format!("{what} — did you mean '{close}'?");
        }
        if tag.is_some() {
            return what;
        }
        // `from py`: an engine's tag without its @ (the file's @py blocks).
        if matches!(canonical_tag(l), "py" | "js" | "ts") {
            return format!("{what} — did you mean @{l}? (from @{l} takes the names this file's @{l} blocks export)");
        }
        // `from lib` for `import "./lib.v2" as lib`: the hint says what lib is (`alias_hint`).
        if self.files[&file].modules.contains_key(l) {
            return what;
        }
        // A package written without its language: npm's when node_modules has it.
        let dir = self.sm.file(file).path.parent().map(Path::to_path_buf).unwrap_or_default();
        let npm = dir.ancestors().any(|d| d.join("node_modules").join(&**l).is_dir());
        if npm {
            format!("{what}: an npm package is imported with js.{l}")
        } else {
            format!("{what}: a Python module is imported with py.{l}, an npm package with js.{l}")
        }
    }

    /// A NameError's hint (5.9): a block of the file exports `name`, or `name` is a block's label,
    /// so the `@import` that would bring it in is missing.
    /// A name whose scope declares it, but further on (5.9): `'y' is used before its let on line
    /// 3`. `env` is where the lookup started.
    pub(crate) fn declared_later(&self, file: FileId, name: &str, env: &Env) -> Option<String> {
        let mut cur = Some(env);
        while let Some(f) = cur {
            if crate::runtime::env::scope_declares(f, name) {
                let scope = self.files.get(&file)?;
                if Rc::ptr_eq(f, &scope.frame) {
                    let (at, how) = scope.top.get(name)?;
                    return Some(format!("'{name}' is used before its {how} on line {}", at.line));
                }
                return Some(format!("'{name}' is used before it is declared: its declaration comes later"));
            }
            cur = f.parent.as_ref();
        }
        None
    }

    /// `print(summe)` after the loop whose body declared `summe` (5.9): where it is declared, and
    /// what to do. The nearest such declaration before the use, else the first after it.
    pub(crate) fn declared_inside(&self, file: FileId, name: &str, at: Span) -> Option<String> {
        let found = self.files.get(&file)?.inner.get(name)?;
        let outside = |d: &&Inner| !(d.body.lo <= at.lo && at.lo < d.body.hi);
        let d = found.iter().filter(outside).rfind(|d| d.at.lo < at.lo).or_else(|| found.iter().find(outside))?;
        let what = &*d.what;
        let fix = if what.starts_with("function") {
            "return it from the function, or declare it with let outside it".to_string()
        } else {
            format!("declare it with let before the {what}, and change it inside with {name} = …")
        };
        Some(format!("'{name}' is not declared here: its {} on line {} is inside the {what}, so it exists only there — {fix}", d.how, d.at.line))
    }

    /// The file's top-level names with their line and how they are declared, for a
    /// did-you-mean that includes the ones further on (5.9).
    pub(crate) fn top_names(&self, file: FileId) -> Vec<(Rc<str>, u32, &'static str)> {
        self.files.get(&file).map_or_else(Vec::new, |f| f.top.iter().map(|(n, (at, how))| (n.clone(), at.line, *how)).collect())
    }

    pub(crate) fn import_hint(&self, file: FileId, name: &str) -> Option<String> {
        let bodies = &self.files.get(&file)?.bodies;
        let exporter = bodies.iter().find(|b| b.directives.exports.as_ref().is_some_and(|x| x.items.iter().any(|i| &*i.text == name)));
        if let Some(b) = exporter {
            let from = match &b.label {
                Some(l) => l.text.to_string(),
                None => format!("@{}", canonical_tag(&b.tag.text)),
            };
            return Some(format!("{} exports it: add @import {{ {name} }} from {from}", self.block_name(b)));
        }
        if bodies.iter().any(|b| b.label.as_ref().is_some_and(|l| &*l.text == name)) {
            return Some(format!("it is a block's label: bring the block's names in with @import {{ … }} from {name}"));
        }
        // `@py { def double(x): ... }`, then `double(4)`: defined at a block's top level, so V2
        // can have it once the block exports it.
        let b = bodies.iter().find(|b| defines(b, name))?;
        let from = match &b.label {
            Some(l) => l.text.to_string(),
            None => format!("@{}", canonical_tag(&b.tag.text)),
        };
        Some(match b.directives.exports.as_ref().filter(|x| x.star.is_some() && !name.starts_with('_')) {
            Some(_) => format!("{} exports it: add @import {{ {name} }} from {from}", self.block_name(b)),
            None => format!("{} defines it without exporting it: add @export {{ {name} }} inside it, then @import {{ {name} }} from {from}", self.block_name(b)),
        })
    }

    /// 13.3: what follows already fails for `name` whatever loads now: its provider is neither
    /// loaded nor waiting (not reached, failed, loading), and no waiting `*` block could take
    /// its place. Nothing starts for that error.
    fn certain_failure(&self, file: FileId, sel: &Selector, name: &str, seen: Seen) -> Option<Rc<EngineBlock>> {
        let waits = |b: &EngineBlock| matches!(self.units.get(&b.unit_id), Some(UnitState::Waiting));
        let b = self.provider(file, sel, name, seen).ok()?;
        if self.loaded_for(&b, seen).is_some() || waits(&b) {
            return None;
        }
        let star = |b: &EngineBlock| b.directives.exports.as_ref().is_some_and(|e| e.star.is_some());
        let star_waits = self.candidates(file, sel).ok()?.iter().any(|c| star(c) && waits(c));
        (!star_waits).then_some(b)
    }

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

    /// `@import { total } from lib.wrap` (or `from lib`) where the file has `import "./lib.v2" as
    /// lib`: its first word reads as an engine's tag or a block's label, so the error's hint says
    /// what `lib` is (13.1). `block`: the `@import` is a block's, whose code can't name `lib`.
    fn alias_hint(&self, imp: &EngineImport, f: Flow, block: bool) -> Flow {
        let Flow::Error(e) = &f else { return f };
        let first = match &imp.selector {
            Selector::Union { tag } | Selector::Exact { tag, .. } | Selector::Module { lang: tag, .. } => tag,
            Selector::Label { label } => label,
            Selector::V2 { .. } => return f,
        };
        let a = &first.text;
        let about = match &*e.kind {
            "EngineUnknown" => e.message.starts_with(&format!("unknown engine '@{a}'")),
            "ImportError" => e.message.starts_with(&format!("no block in this file is labeled '{a}'")),
            _ => false,
        };
        if let Some(path) = self.files.get(&imp.span.file).and_then(|s| s.modules.get(a)).filter(|_| about) {
            // The first name asked for; the whole module for `@import lib.x as y`.
            let first = match &imp.items {
                ImportItems::Names(list) => list.first().map(|i| i.name.clone()),
                ImportItems::Module(_) => None,
            };
            // The path as a literal again: `"..\\lib.v2"` for a backslash.
            let mut literal = String::new();
            crate::runtime::value::quoted(path, &mut literal);
            let module = format!("{a} is a V2 module (import {literal} as {a})");
            let hint = if block {
                // A block's code reaches V2's names only through `from v2` (13.7).
                let name = first.as_deref().unwrap_or("name");
                format!("{module}: a block gets its names through V2, as in let {name} = {a}.{name} at the file's top level, then @import {{ {name} }} from v2 in the block")
            } else {
                let what = first.map_or_else(|| a.to_string(), |n| format!("{a}.{n}"));
                format!("{module}: use {what} directly; @import brings in names from other languages' blocks and packages")
            };
            let _ = e.hint.set(hint.into());
        }
        f
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
            return Err(import_error(self.no_exports(file, sel, Some(name))));
        }
        let hits: Vec<&Rc<EngineBlock>> = cands.iter().filter(|b| self.provides(b, name, seen)).collect();
        match hits.as_slice() {
            [one] => Ok((*one).clone()),
            [] => cands
                .iter()
                .find(|b| b.directives.exports.as_ref().is_some_and(|e| e.star.is_some()) && self.loaded_for(b, seen).is_none())
                .cloned()
                .ok_or_else(|| import_error(not_exported_by_block(name, &text, cands.iter().flat_map(|b| b.directives.exports.iter().flat_map(|x| x.items.iter().map(|i| &*i.text))), true, cands.iter().find_map(|b| star_tag(b))))),
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
                // `if use_py { @py helpers {…} }`: below it already, but that code didn't run.
                None if self.files.get(&b.unit_id.file).is_some_and(|f| !f.top_blocks.contains(&b.unit_id)) => {
                    let line = self.sm.line_col(b.unit_id.file, b.unit_id.offset).0;
                    format!("{who} (line {line}) is inside an `if`, a loop or a function, which hasn't run it: a block runs when the code it is in reaches it — move it to the top level, or this @import into that code")
                }
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
                None => Err(import_error(not_exported_by_block(name, &self.selector_text(sel), exports.iter().map(|e| &*e.name), false, star_tag(&b)))),
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
            None => self.resolve_import(imp, None).map_err(|f| self.alias_hint(imp, f, false))?,
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
            self.module_unit(imp.span.file, &key, &joined).map_err(|f| self.raise(f, self.selector_span(&imp.selector)))?;
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
                let (value, exports, missing, have) = self.import_module(file, &imp.selector, names, self.selector_span(&imp.selector))?;
                match items {
                    ImportItems::Module(alias) => bound.push((alias.text.clone(), value)),
                    ImportItems::Names(list) => {
                        for (name, alias, span) in named_items(list) {
                            match find(&exports, &name) {
                                Some(e) if !missing.contains(&name) => bound.push((alias, e.value.clone())),
                                _ => return Err(self.raise(self.not_exported_error(file, &name, &imp.selector, &have), span)),
                            }
                        }
                    }
                }
            }
            (sel, ImportItems::Names(list)) => {
                // A name nothing can provide, or whose block can't have loaded by then, needs no
                // language to find that out (13.3).
                self.check_selector(file, sel, &names_text(list))?;
                for (name, _, span) in named_items(list) {
                    self.provider(file, sel, &name, seen).map_err(|f| self.raise(f, span))?;
                    if let Some(b) = self.certain_failure(file, sel, &name, seen) {
                        return Err(self.raise(self.state_error(&b, None), span));
                    }
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
    /// language; a failure leaves it waiting (12.7). Its error, there or at a later use, is the
    /// `@import`'s, with the use's frames below (round 23).
    pub(crate) fn settle(&mut self, f: &Func, at: Span) -> Exec<Value> {
        let Callable::Waiting { import, local, seen, resolved } = &**f else { unreachable!("only a waiting reference settles") };
        if let Some(v) = resolved.get() {
            return Ok(v.clone());
        }
        if let Some(tag) = self.import_tag(import) {
            self.start_engine(&tag, at)?;
        }
        let value = self.run_for(at, |it| it.resolve_import(import, Some(seen)))?.into_iter().find(|(name, _)| name == local).map(|(_, v)| v);
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
            if !name.split('.').all(py_part) && !pip_style(name) {
                // `py."./helper.py"`, as js takes a file: Python's names are module names.
                let hint = if name.ends_with(".py") || name.contains(['/', '\\']) { self.py_path_hint(file, name) } else { String::new() };
                return Err(import_error(format!("'{name}' is not a valid Python module name{hint}")));
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

    /// `py."../src/acme/core.py"`, a file's path for a Python module (13.5): Python finds modules
    /// by dotted name in the .v2 file's folder, the project root and its own path. A path below
    /// one of those two folders is that name; any other folder has to be on `sys.path`.
    fn py_path_hint(&self, file: FileId, name: &str) -> String {
        let dir = self.sm.file(file).path.parent().map(Path::to_path_buf).unwrap_or_default();
        let target = absolute_clean(&dir.join(name.trim_end_matches(".py").replace('\\', "/")));
        // The dotted name of `target` from `base`, when each part is a name.
        let dotted = |base: &Path| target.strip_prefix(base).ok().and_then(py_dotted);
        if let Some(module) = dotted(&dir) {
            return format!(": a Python file next to the .v2 file is imported as a module, py.{module}");
        }
        let root = &self.registry.config().root;
        let root = crate::runtime::module::canonical(root).unwrap_or_else(|| root.clone());
        if let Some(module) = dotted(&root) {
            return format!(": the project folder, {}, is on Python's path, and from there this is py.{module}", root.display());
        }
        ": Python finds a module by its dotted name in the .v2 file's folder, the project folder or its own path (a venv's packages): write that name, or put the file's folder on sys.path in a @py block before this @import".into()
    }

    /// One `module` frame (13.5): the module handle (or null), the exports of the requested
    /// `names` (a list, or null) and the requested names the module lacks.
    fn import_module(&mut self, file: FileId, sel: &Selector, names: Json, at: Span) -> Exec<ModuleAnswer> {
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
        let dir = self.sm.file(file).path.parent().map(Path::to_path_buf).unwrap_or_default();
        let engine = &self.sessions[s].engine;
        let python = pip_for(engine, std::env::var_os("VIRTUAL_ENV").filter(|v| !v.is_empty()).map(PathBuf::from));
        // V2_ENGINE_PY, left set, wins over the project's venv (18.3).
        let unused = (&*engine.spec.id == "py").then(|| crate::engines::resolve::unused_venv(self.registry.config())).flatten();
        let (root, toml) = (self.registry.config().root.clone(), self.registry.config().file.is_some());
        let shown = self.sm.file(file).display.clone();
        let reply = self.request(s, frame, &what, at).inspect_err(|f| {
            if let Flow::Error(e) = f {
                let hint = match &*e.kind {
                    "ImportError" => {
                        let root = crate::runtime::module::canonical(&root).unwrap_or(root);
                        let hint = missing_package_hint(&e.message, &Site { dir: &dir, file: &shown, root: &root, toml }, python.as_ref());
                        match &unused {
                            Some(unused) => unused_venv_hint(&e.message, unused, hint, matches!(python, Some(PipFor::Embeddable(_)))),
                            None => hint,
                        }
                    }
                    "ForeignError" => nested_commonjs_hint(&e.message, &joined, &dir)
                        .or_else(|| ts_import_hint(&e.message))
                        .or_else(|| commonjs_hint(&e.message, &joined, &dir)),
                    _ => None,
                };
                if let Some(hint) = hint {
                    let _ = e.hint.set(hint.into());
                }
            }
        })?;
        let unit: Rc<str> = unit.into();
        let exports = reply.exports.into_iter().map(|(name, v)| export(s, &unit, name, v)).collect();
        Ok((reply.value, exports, reply.missing, Have { names: reply.names, hint: reply.hint }))
    }

    /// 13.5's not-exported error, with a hint: a file of the script's own has that name, the
    /// name is a TypeScript type, or the worker's (a dual package's), unless a name is near.
    fn not_exported_error(&self, file: FileId, name: &str, sel: &Selector, have: &Have) -> Flow {
        let text = self.selector_text(sel);
        let f = import_error(not_exported(name, &text, &have.names));
        let dir = self.sm.file(file).path.parent().map(Path::to_path_buf).unwrap_or_default();
        let module = match sel {
            Selector::Module { path, .. } => path.iter().map(|n| &*n.text).collect::<Vec<_>>().join("."),
            _ => String::new(),
        };
        let worker = || have.hint.as_ref().filter(|_| did_you_mean(name, have.names.iter().map(|n| &**n)).is_none()).map(|h| h.to_string());
        let hint = shadowed_hint(&text, &dir).or_else(|| ts_type_hint(name, &module, &dir)).or_else(worker);
        if let (Flow::Error(e), Some(hint)) = (&f, hint) {
            let _ = e.hint.set(hint.into());
        }
        f
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
                        if !self.files[&file].top.contains_key(&name) {
                            let msg = format!("'{name}' is not a top-level name in {}", self.sm.file(file).display);
                            return Err(self.raise(import_error(msg), span));
                        }
                        // A block imports functions; a constant that isn't one can't become one
                        // (DOCS 6.2). A `let` that holds null, or isn't declared yet, may still be
                        // given a function before the first call; one that holds data is data.
                        let frame = &self.files[&file].frame;
                        let not_func = |v: &Value| !matches!(v, Value::Func(_)) && !matches!(v, Value::Handle(h) if h.callable);
                        if crate::runtime::env::own_const(frame, &name).is_some_and(|v| not_func(&v)) {
                            return Err(self.raise(constant_error(&name), span));
                        }
                        if let Some(v) = own_value(frame, &name).filter(|v| not_func(v) && !matches!(v, Value::Null)) {
                            let msg = format!("'{name}' is {}, not a function — pass it in as a block arg", v.a_type_name());
                            return Err(self.raise(import_error(msg), span));
                        }
                        let r = self.route(b.unit_id, i, &name, Route::V2 { file, name: name.clone() });
                        entries.push(json!({"as": &*alias, "route": r}));
                    }
                }
                Selector::Module { lang, path } => {
                    let at = self.selector_span(&imp.selector);
                    let target = self.selector_key(lang).map_err(|f| self.raise(self.alias_hint(imp, f, true), at))?;
                    let ImportItems::Names(list) = &imp.items else { unreachable!("a static error in blocks") };
                    if *target != *key {
                        // Another session's module is imported when first called (13.6).
                        let joined = path.iter().map(|n| &*n.text).collect::<Vec<_>>().join(".");
                        self.module_unit(file, &target, &joined).map_err(|f| self.raise(f, at))?;
                        for (name, alias, _) in named_items(list) {
                            let r = self.route(b.unit_id, i, &name, pending(&name));
                            entries.push(json!({"as": &*alias, "route": r}));
                        }
                        continue;
                    }
                    let names = json!(list.iter().map(|i| &*i.name).collect::<Vec<_>>());
                    let (_, exports, missing, have) = self.import_module(file, &imp.selector, names, at)?;
                    for (name, alias, span) in named_items(list) {
                        let found = find(&exports, &name).filter(|_| !missing.contains(&name));
                        let Some(e) = found else {
                            return Err(self.raise(self.not_exported_error(file, &name, &imp.selector, &have), span));
                        };
                        if !e.func {
                            // A package's object (zod's `z`): the block's language imports it itself.
                            let f = constant_error(&name);
                            if let Flow::Error(err) = &f {
                                let module = path.iter().map(|n| &*n.text).collect::<Vec<_>>().join(".");
                                let own = if key == "py" { format!("from {module} import {name}") } else { format!("const {{ {name} }} = require(\"{}\")", module.trim_matches('"')) };
                                let _ = err.hint.set(format!("a block imports only functions; in the block, import the package the language's own way: {own}").into());
                            }
                            return Err(self.raise(f, span));
                        }
                        let Value::Func(f) = &e.value else { unreachable!("a func export is a Proxy") };
                        let Callable::Proxy { unit, .. } = &**f else { unreachable!("a module export is a Proxy") };
                        entries.push(json!({"as": &*alias, "unit": &**unit, "name": &*name}));
                    }
                }
                sel => {
                    let ImportItems::Names(list) = &imp.items else { unreachable!("a static error in blocks") };
                    self.check_selector(file, sel, &names_text(list)).map_err(|f| self.alias_hint(imp, f, true))?;
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
    /// `js`: the caller is JavaScript, whose extra arguments a V2 function ignores (DOCS 8.2).
    pub(crate) fn call_route(&mut self, n: usize, mut pos: Vec<Value>, named: Vec<(Rc<str>, Value)>, at: Span, js: bool) -> Exec<Value> {
        let f = match self.routes.get(n) {
            None => return fail("ProtocolError", format!("unknown callable id {n}")),
            Some(Route::Target(f)) => f.clone(),
            Some(Route::V2 { file, name }) => {
                let (file, name) = (*file, name.clone());
                let found = own_value(&self.files[&file].frame, &name);
                match found {
                    Some(f @ Value::Func(_)) => f,
                    Some(Value::Handle(h)) if h.callable => Value::Handle(h),
                    Some(v) => {
                        let msg = format!("'{name}' is {} now, not a function — pass data in as a block arg", v.a_type_name());
                        return Err(self.raise(error("TypeError", msg), at));
                    }
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
        if js {
            crate::engines::session::drop_js_extras(&f, &mut pos);
        }
        self.call_value(&f, pos, named, at)
    }

    /// What route `n` calls now, if it is settled (a target, or a V2 function's current value),
    /// without resolving it: for the hint of a call that failed (session.rs `fewer_args_hint`).
    pub(crate) fn route_value(&self, n: usize) -> Option<Value> {
        match self.routes.get(n)? {
            Route::Target(f) => Some(f.clone()),
            Route::V2 { file, name } => own_value(&self.files.get(file)?.frame, name),
            Route::Pending { .. } => None,
        }
    }

    /// A `Pending` route's target, found now (13.6); not loaded yet and constants give 13.7's
    /// messages.
    fn resolve_route(&mut self, block: &Rc<EngineBlock>, import: usize, name: &Rc<str>, at: Span) -> Exec<Value> {
        let imp = &block.directives.imports[import];
        let file = block.unit_id.file;
        if let Selector::Module { .. } = &imp.selector {
            let (_, exports, missing, have) = self.import_module(file, &imp.selector, json!([&**name]), at)?;
            return match find(&exports, name) {
                Some(e) if !missing.contains(name) && e.func => Ok(e.value.clone()),
                Some(_) if !missing.contains(name) => Err(constant_error(name)),
                _ => Err(self.not_exported_error(file, name, &imp.selector, &have)),
            };
        }
        // What can only fail starts nothing (13.3).
        self.provider(file, &imp.selector, name, None)?;
        if let Some(b) = self.certain_failure(file, &imp.selector, name, None) {
            return Err(self.state_error(&b, Some(name)));
        }
        self.start_waiting(file, &imp.selector, at)?;
        let hit = self.provider(file, &imp.selector, name, None)?;
        match self.loaded(&hit).map(|ex| find(ex, name).cloned()) {
            Some(Some(e)) if e.func => Ok(e.value),
            Some(Some(_)) => Err(constant_error(name)),
            Some(None) => Err(import_error(not_exported_by_block(name, &self.selector_text(&imp.selector), self.loaded(&hit).into_iter().flat_map(|l| l.iter()).map(|e| &*e.name), false, star_tag(&hit)))),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the system temp dir.
    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("v2imp-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A .v2 file in `dir` that is the entry script, with no v2.toml.
    fn site(dir: &Path) -> Site<'_> {
        Site { dir, file: "m.v2", root: dir, toml: false }
    }

    #[test]
    fn missing_package_hints_name_the_missing_part() {
        // 13.5: from the engine's own message, what to check or what to install.
        let d = temp("plain");
        let hint = |m: &str, dir: &Path| missing_package_hint(m, &site(dir), None).unwrap();
        assert_eq!(hint("No module named 'os.pathh'; 'os' is not a package", &d), "'os' is a module, not a package, so it has no 'pathh': check the name after 'os.'");
        assert_eq!(hint("No module named 'xml.etreee'", &d), "'xml' is there, but has no 'etreee': check the name after 'xml.'");
        let pip = "if 'numpy' isn't misspelled, install it with pip for the Python that v2 doctor shows; README.md, \"Python and npm packages\", has the steps";
        assert_eq!(hint("No module named 'numpy'", &d), pip);
        // A virtual environment V2 isn't told about, in the file's folder or above it.
        let proj = temp("venv");
        std::fs::create_dir_all(proj.join(".venv")).unwrap();
        std::fs::write(proj.join(".venv").join("pyvenv.cfg"), "home = x\n").unwrap();
        std::fs::create_dir_all(proj.join("sub")).unwrap();
        let expected = format!("there is a virtual environment in {}, which V2 isn't using: name it in a v2.toml next to it, [python] venv = \".venv\"; or {pip}", proj.join(".venv").display());
        assert_eq!(hint("No module named 'numpy'", &proj.join("sub")), expected);
        // A Python V2 was told to use (a venv in v2.toml): its own install command.
        let venv_python = proj.join(".venv").join("bin").join("python");
        assert_eq!(
            missing_package_hint("No module named 'numpy'", &site(&proj), Some(&PipFor::Told(vec![venv_python.clone().into_os_string()]))),
            Some(format!("if 'numpy' isn't misspelled, install it for the Python V2 uses: {} -m pip install numpy", venv_python.display()))
        );
        // A helper of the project's own in a folder below.
        std::fs::create_dir_all(proj.join("stages").join("deep")).unwrap();
        std::fs::write(proj.join("stages").join("deep").join("clean.py"), "").unwrap();
        assert_eq!(hint("No module named 'clean'", &proj), "there is a stages/deep/clean.py in a folder below: import it as py.stages.deep.clean");
        assert_eq!(hint("No module named 'clean'", &proj.join("stages")), "there is a deep/clean.py in a folder below: import it as py.deep.clean");
        // Review of round 18: a folder that is no Python name (`2024`, `class`, `my-lib`) gives no
        // py. name, which module_unit would refuse; one that is wins over it.
        for folder in ["2024", "class", "my-lib"] {
            std::fs::create_dir_all(proj.join(folder)).unwrap();
            std::fs::write(proj.join(folder).join("cleanzz.py"), "").unwrap();
            assert_eq!(
                hint("No module named 'cleanzz'", &proj),
                format!("there is a {folder}/cleanzz.py in a folder below, but {folder} isn't a Python name, so no py. name reaches it: rename the folder, or put it on sys.path in a @py block first")
            );
            std::fs::remove_dir_all(proj.join(folder)).unwrap();
        }
        std::fs::create_dir_all(proj.join("2024")).unwrap();
        std::fs::write(proj.join("2024").join("cleanzz.py"), "").unwrap();
        std::fs::create_dir_all(proj.join("stages").join("deep").join("old")).unwrap();
        std::fs::write(proj.join("stages").join("deep").join("old").join("cleanzz.py"), "").unwrap();
        assert_eq!(hint("No module named 'cleanzz'", &proj), "there is a stages/deep/old/cleanzz.py in a folder below: import it as py.stages.deep.old.cleanzz");
        std::fs::remove_dir_all(proj.join("2024")).unwrap();
        // One in a folder above the .v2 file's.
        std::fs::write(proj.join("rooth.py"), "").unwrap();
        let sub = proj.join("stages").join("deep");
        assert_eq!(
            hint("No module named 'rooth'", &sub),
            format!("there is a {} in a folder above: a v2.toml in {} (it may be empty) makes that the project folder, where py.rooth is found", proj.join("rooth.py").display(), proj.display())
        );
        // npm: a subpath of an installed package, a near name, a scoped package.
        let js = temp("npm");
        std::fs::create_dir_all(js.join("node_modules").join("lodash")).unwrap();
        assert_eq!(hint("Cannot find module 'lodash/chunck'", &js), "'lodash' is installed, but has no 'chunck': check the path after 'lodash/'");
        let place = |dir: &Path| format!("{} or a folder above it, where Node looks for this file", dir.display());
        assert_eq!(hint("Cannot find module 'lodsh'", &js), format!("did you mean js.lodash? if 'lodsh' isn't misspelled, install it with npm install lodsh in {}", place(&js)));
        assert_eq!(hint("Cannot find module '@scope/pkg/sub'", &d), format!("if '@scope/pkg' isn't misspelled, install it with npm install @scope/pkg in {}", place(&d)));
        assert_eq!(hint("Cannot find module './nope.js'", &d), "a local file is found from the .v2 file's folder: check the path");
        // The script's own file, named like a package: never npm install it.
        std::fs::write(js.join("helpers.js"), "").unwrap();
        let own = "your own helpers.js is imported by its path from the .v2 file's folder: @import { … } from js.\"./helpers.js\" (js.helpers, without ./, names an npm package)";
        // Round 18: a .jsx or .tsx file of the script's own, named bare, is no npm package either.
        std::fs::write(d.join("comp.jsx"), "").unwrap();
        std::fs::write(d.join("view.tsx"), "").unwrap();
        assert_eq!(
            missing_package_hint("Cannot find module 'comp.jsx'", &site_at(&d), None).as_deref(),
            Some("your own comp.jsx is imported by its path from the .v2 file's folder: @import { … } from js.\"./comp.jsx\" (js.\"comp.jsx\", without ./, names an npm package)")
        );
        assert!(missing_package_hint("Cannot find module 'view.tsx'", &site_at(&d), None).unwrap().contains("from ts.\"./view.tsx\""));
        assert_eq!(hint("Cannot find module 'helpers'", &js), own);
        // What was written: `js."helpers.js"`, quoted.
        assert_eq!(hint("Cannot find module 'helpers.js'", &js), own.replace("js.helpers,", "js.\"helpers.js\","));
        // Review of round 19: a name with such an extension may be an npm package's own
        // (chart.js), so its install line follows.
        assert_eq!(
            hint("Cannot find module 'util.ts'", &js),
            format!("a file of your own is imported by its path from the .v2 file's folder, as in ts.\"./util.ts\": check its name (js.\"util.ts\", without ./, names an npm package); or, for the npm package util.ts, npm install util.ts in {}", js.display())
        );
        // A local path without its extension, when the file is there.
        std::fs::write(js.join("money.ts"), "").unwrap();
        assert_eq!(hint("Cannot find module './money'", &js), "did you mean ts.\"./money.ts\"? a file is named with its extension");
        assert_eq!(hint("Cannot find module './mony'", &js), "a local file is found from the .v2 file's folder: check the path");
        // A package name that is also a file of the script's own.
        std::fs::write(js.join("util.js"), "").unwrap();
        assert_eq!(shadowed_hint("js.util", &js).as_deref(), Some("js.util is a package (Node's own or one from npm), not your util.js: a file of yours is imported by its path, as in js.\"./util.js\""));
        assert_eq!(shadowed_hint("js.path", &js), None, "no path.js there");
        assert_eq!(shadowed_hint("py.util", &js), None);
        assert_eq!(missing_package_hint("something else", &site(&d), None), None);
        // A package that offers only `import` (an ES module's `exports`).
        let m = format!("No \"exports\" main defined in {}", d.join("node_modules").join("@scope").join("esm-only").join("package.json").display());
        assert_eq!(
            hint(&m, &d),
            "'@scope/esm-only' can be loaded only with import(), which a @js block can do: async function load() { return await import(\"@scope/esm-only\") }, then @export { load } and call load()"
        );
    }

    #[test]
    fn the_install_line_is_a_command_that_works_for_that_python() {
        // Round 19 (L1, L2): for a Python V2 was told to use, a command that works there.
        let d = temp("pipfor");
        let told = |argv: &[&Path]| PipFor::Told(argv.iter().map(|p| p.as_os_str().to_os_string()).collect());
        let hint = |p: &PipFor| missing_package_hint("No module named 'tabulate'", &site(&d), Some(p)).unwrap();
        let head = "if 'tabulate' isn't misspelled, install it for the Python V2 uses";
        // A command with arguments: all of it, each word quoted as a shell needs it.
        let (exec, python) = (Path::new("exec"), Path::new("python"));
        let pyenv = d.join("pyenv");
        assert_eq!(hint(&told(&[&pyenv, exec, python])), format!("{head}: {} exec python -m pip install tabulate", pyenv.display()));
        let spaced = d.join("my tools").join("pyenv");
        let amp = if cfg!(windows) { "& " } else { "" };
        assert_eq!(hint(&told(&[&spaced, exec, python])), format!("{head}: {amp}\"{}\" exec python -m pip install tabulate", spaced.display()));
        // One that runs uv (uv run python): uv add.
        assert_eq!(hint(&told(&[&d.join("uv"), Path::new("run"), python])), format!("{head}: uv add tabulate"));
        // A venv uv made, which has no pip: uv pip install --python, or uv add in its uv project.
        let venv = d.join(".venv");
        let bin = venv.join(if cfg!(windows) { "Scripts" } else { "bin" });
        std::fs::create_dir_all(&bin).unwrap();
        let venv_python = bin.join(if cfg!(windows) { "python.exe" } else { "python" });
        std::fs::write(venv.join("pyvenv.cfg"), "uv = 0.13.8\nversion_info = 3.13.18\n").unwrap();
        assert_eq!(hint(&told(&[&venv_python])), format!("{head}: uv pip install --python {} tabulate", venv.display()));
        std::fs::write(d.join("pyproject.toml"), "").unwrap();
        std::fs::write(d.join("uv.lock"), "").unwrap();
        assert_eq!(hint(&told(&[&venv_python])), format!("{head}: uv add tabulate in {}", d.display()));
        // Another venv without pip (python -m venv --without-pip): ensurepip first.
        std::fs::write(venv.join("pyvenv.cfg"), "home = x\nversion = 3.13.18\n").unwrap();
        let site_packages = if cfg!(windows) { venv.join("Lib").join("site-packages") } else { venv.join("lib").join("python3.13").join("site-packages") };
        std::fs::create_dir_all(&site_packages).unwrap();
        let py = venv_python.display();
        assert_eq!(hint(&told(&[&venv_python])), format!("{head}, whose venv has no pip: {py} -m ensurepip, then {py} -m pip install tabulate"));
        std::fs::create_dir_all(site_packages.join("pip")).unwrap();
        assert_eq!(hint(&told(&[&venv_python])), format!("{head}: {py} -m pip install tabulate"));
    }

    #[test]
    fn the_install_line_names_the_package_on_pypi() {
        // Round 20 (L14): `pip install yaml` finds nothing; `pip install slugify` installs an
        // unrelated Python 2 package. Every form of the line names the package.
        let d = temp("pypi");
        let told = |argv: &[&Path]| PipFor::Told(argv.iter().map(|p| p.as_os_str().to_os_string()).collect());
        let hint = |module: &str, p: Option<&PipFor>| missing_package_hint(&format!("No module named '{module}'"), &site(&d), p).unwrap();
        let python = d.join("python3");
        assert_eq!(hint("yaml", Some(&told(&[&python]))), format!("if 'yaml' isn't misspelled, install it for the Python V2 uses: {} -m pip install PyYAML", python.display()));
        assert!(hint("slugify", Some(&told(&[&python]))).ends_with(" -m pip install python-slugify"));
        assert!(hint("PIL", Some(&told(&[&python]))).ends_with(" -m pip install Pillow"));
        assert!(hint("Crypto", Some(&told(&[&python]))).ends_with(" -m pip install pycryptodome"));
        assert_eq!(hint("cv2", Some(&told(&[&d.join("uv"), Path::new("run"), Path::new("python")]))), "if 'cv2' isn't misspelled, install it for the Python V2 uses: uv add opencv-python");
        // A Python found on PATH: the package by name.
        assert_eq!(
            hint("bs4", None),
            "if 'bs4' isn't misspelled, install its package, beautifulsoup4, with pip for the Python that v2 doctor shows; README.md, \"Python and npm packages\", has the steps"
        );
        // Others: the module's name, and no note.
        assert!(hint("tabulate", Some(&told(&[&python]))).ends_with(" -m pip install tabulate"));
        assert!(hint("tabulate", None).starts_with("if 'tabulate' isn't misspelled, install it with pip"));
        // A venv uv made: uv pip install.
        let venv = d.join(".venv");
        let bin = venv.join(if cfg!(windows) { "Scripts" } else { "bin" });
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "uv = 0.13.8\n").unwrap();
        let venv_python = bin.join(if cfg!(windows) { "python.exe" } else { "python" });
        let shown = crate::cli::shell_word(&venv.display().to_string());
        assert!(hint("sklearn", Some(&told(&[&venv_python]))).ends_with(&format!(": uv pip install --python {shown} scikit-learn")));
    }

    #[test]
    fn an_activated_venv_v2_cant_use_is_named_instead_of_pip() {
        // Round 19 (L4): VIRTUAL_ENV names a venv whose python is gone, so V2 found another
        // Python on PATH: the hint says what is wrong with the venv, never to pip install into
        // the other Python, nor that the venv is one "V2 isn't using".
        let d = temp("passed");
        let venv = d.join(".venv");
        std::fs::create_dir_all(&venv).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = x\n").unwrap();
        let problem = crate::engines::resolve::activated_venv_problem(&venv, &[], &crate::engines::resolve::Lookup::default()).unwrap();
        let passed = PipFor::Passed(venv.clone(), problem.clone());
        let hint = |dir: &Path| missing_package_hint("No module named 'requests'", &site(dir), Some(&passed)).unwrap();
        assert_eq!(hint(&d), problem, "run in the venv's own folder");
        assert_eq!(hint(&temp("passed-elsewhere")), problem);
        // Another venv nearby: that one, and the activated one's problem.
        let other = temp("passed-other");
        std::fs::create_dir_all(other.join(".venv")).unwrap();
        std::fs::write(other.join(".venv").join("pyvenv.cfg"), "home = x\n").unwrap();
        assert_eq!(
            hint(&other),
            format!("there is a virtual environment in {}, which V2 isn't using: name it in a v2.toml next to it, [python] venv = \".venv\"; and {problem}", other.join(".venv").display())
        );
    }

    /// A venv `dir/name` with this pyvenv.cfg, and where its python goes.
    fn venv_at(dir: &Path, name: &str, cfg: &str) -> (PathBuf, PathBuf) {
        let venv = dir.join(name);
        let bin = venv.join(if cfg!(windows) { "Scripts" } else { "bin" });
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), cfg).unwrap();
        (venv, bin.join(if cfg!(windows) { "python.exe" } else { "python3" }))
    }

    #[test]
    fn a_venv_v2_found_on_path_gets_its_own_install_line() {
        // Round 19 review (P2): an activated venv whose python V2 found on PATH is the one V2 uses:
        // its own install line, never "which V2 isn't using", run from its own folder too.
        let d = temp("onpath");
        let (venv, python) = venv_at(&d, ".venv", "home = x\n");
        let on_path = PipFor::OnPath(vec![python.clone().into_os_string()], venv.clone());
        let hint = |dir: &Path| missing_package_hint("No module named 'requests'", &site(dir), Some(&on_path)).unwrap();
        let install = format!("if 'requests' isn't misspelled, install it for the Python V2 uses: {} -m pip install requests", python.display());
        assert_eq!(hint(&d), install);
        assert_eq!(hint(&temp("onpath-elsewhere")), install);
        // Another venv nearby is one V2 isn't using: named, then the install line.
        let other = temp("onpath-other");
        venv_at(&other, ".venv", "home = x\n");
        assert_eq!(
            hint(&other),
            format!("there is a virtual environment in {}, which V2 isn't using: name it in a v2.toml next to it, [python] venv = \".venv\"; or {install}", other.join(".venv").display())
        );
    }

    #[test]
    fn pip_for_picks_whom_the_install_line_is_for() {
        // 13.5: the Python V2 was told to use; for one found on PATH, the activated venv it
        // couldn't use (P1: its python is among the skipped hits, its probe failed), else the
        // venv that python is in (P2), else nobody.
        let d = temp("pipfor-engine");
        let (venv, python) = venv_at(&d, ".venv", "home = x\n");
        std::fs::write(&python, "").unwrap();
        let spec = Rc::new(crate::engines::spec::builtins().into_iter().find(|s| &*s.id == "py").unwrap());
        let engine = |origin: crate::engines::resolve::Origin, program: &Path, skipped: Vec<(PathBuf, String)>| crate::engines::resolve::ResolvedEngine {
            spec: spec.clone(),
            origin,
            argv: vec![program.as_os_str().to_os_string()],
            version: None,
            skipped,
        };
        use crate::engines::resolve::Origin;
        let system = d.join("python3");
        let told = engine(Origin::Venv, &python, Vec::new());
        assert!(matches!(pip_for(&told, None), Some(PipFor::Told(argv)) if argv == told.argv));
        assert!(matches!(pip_for(&engine(Origin::Path, &python, Vec::new()), None), Some(PipFor::OnPath(_, v)) if v == venv));
        assert!(pip_for(&engine(Origin::Path, &system, Vec::new()), None).is_none());
        // VIRTUAL_ENV names that venv and its python runs: it is the one used.
        assert!(matches!(pip_for(&engine(Origin::Path, &python, Vec::new()), Some(venv.clone())), Some(PipFor::OnPath(..))));
        // Its python's probe failed, so the search found another: what is wrong with it.
        let failed = vec![(python.clone(), "version probe failed (exit code 103): No Python at 'C:\\gone\\python.exe'".to_string())];
        let passed = pip_for(&engine(Origin::Path, &system, failed.clone()), Some(venv.clone()));
        assert!(matches!(&passed, Some(PipFor::Passed(v, problem)) if *v == venv && problem.contains("has a python that doesn't run (No Python at 'C:\\gone\\python.exe')")), "{:?}", passed.map(|_| ()));
    }

    #[test]
    fn a_conda_python_found_on_path_gets_its_own_install_line() {
        // Round 21 (L9): an activated conda environment's python (conda-meta in its prefix) is the
        // one V2 uses, as an activated venv's is: its own install line, not "the Python that v2
        // doctor shows".
        let d = temp("conda");
        let prefix = d.join("envs").join("sales");
        let python = if cfg!(windows) { prefix.join("python.exe") } else { prefix.join("bin").join("python") };
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::fs::write(&python, "").unwrap();
        let spec = Rc::new(crate::engines::spec::builtins().into_iter().find(|s| &*s.id == "py").unwrap());
        let engine = crate::engines::resolve::ResolvedEngine { spec, origin: crate::engines::resolve::Origin::Path, argv: vec![python.clone().into_os_string()], version: None, skipped: Vec::new() };
        assert!(pip_for(&engine, None).is_none(), "no conda-meta yet");
        std::fs::create_dir_all(prefix.join("conda-meta")).unwrap();
        let p = pip_for(&engine, None);
        assert!(matches!(&p, Some(PipFor::OnPath(_, v)) if *v == prefix));
        assert_eq!(
            missing_package_hint("No module named 'cowsay'", &site(&d), p.as_ref()).unwrap(),
            format!("if 'cowsay' isn't misspelled, install it for the Python V2 uses: {} -m pip install cowsay", python.display())
        );
    }

    #[test]
    fn a_venv_whose_python_has_no_ensurepip_is_made_again() {
        // Round 19 review (P3): on Debian and Ubuntu without python3-venv, python3 -m venv stops
        // after making a venv without pip, and its Python has no ensurepip (<home>/../lib/
        // python3.X/ensurepip, Windows <home>\Lib\ensurepip) to add one: the package, then the
        // venv made again, never -m ensurepip.
        let d = temp("noensurepip");
        let (home, stdlib) = if cfg!(windows) { (d.join("base"), d.join("base").join("Lib")) } else { (d.join("base").join("bin"), d.join("base").join("lib").join("python3.12")) };
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&stdlib).unwrap();
        std::fs::write(stdlib.join("os.py"), "").unwrap();
        // Round 22 (0): made again with the Python it was made from, which is there.
        let base = home.join(if cfg!(windows) { "python.exe" } else { "python3" });
        std::fs::write(&base, "").unwrap();
        let (venv, python) = venv_at(&d, ".venv", &format!("home = {}\nversion = 3.12.3\n", home.display()));
        std::fs::create_dir_all(if cfg!(windows) { venv.join("Lib").join("site-packages") } else { venv.join("lib").join("python3.12").join("site-packages") }).unwrap();
        let hint = || missing_package_hint("No module named 'tabulate'", &site(&d), Some(&PipFor::Told(vec![python.clone().into_os_string()]))).unwrap();
        let head = "if 'tabulate' isn't misspelled, install it for the Python V2 uses";
        #[cfg(unix)]
        let first = format!(" (on Debian and Ubuntu: {}): then ", if unsafe { libc::geteuid() } == 0 { "apt update && apt install python3-venv" } else { "sudo apt install python3-venv" });
        #[cfg(not(unix))]
        let first = ": ".to_string();
        let (v, p, py) = (venv.display(), python.display(), base.display());
        let remake = format!("{head}, whose venv has no pip, and its Python has no ensurepip to add it{first}make the venv again, {py} -m venv --clear {v}, and install its packages, as in {p} -m pip install tabulate");
        assert_eq!(hint(), remake);
        if cfg!(unix) {
            // No version in pyvenv.cfg: the python3.X folders there.
            std::fs::write(venv.join("pyvenv.cfg"), format!("home = {}\n", home.display())).unwrap();
            assert_eq!(hint(), remake);
        }
        // Its Python has ensurepip: that first.
        std::fs::create_dir_all(stdlib.join("ensurepip")).unwrap();
        let ensurepip = format!("{head}, whose venv has no pip: {p} -m ensurepip, then {p} -m pip install tabulate");
        assert_eq!(hint(), ensurepip);
        // No standard library where home says (V2 can't tell): ensurepip, as before.
        std::fs::remove_dir_all(&stdlib).unwrap();
        assert_eq!(hint(), ensurepip);
    }

    #[test]
    fn an_embeddable_python_gets_no_pip_line() {
        // Round 22 (1.2): python.org's embeddable package (a python313._pth beside python.exe)
        // has no pip and no venv module, whether V2 was told to use it or found it on PATH.
        let d = temp("embeddable");
        let portable = d.join("py313embed");
        std::fs::create_dir_all(&portable).unwrap();
        let python = portable.join(if cfg!(windows) { "python.exe" } else { "python3" });
        std::fs::write(&python, "").unwrap();
        std::fs::write(portable.join("python313._pth"), "python313.zip\n.\n").unwrap();
        let spec = Rc::new(crate::engines::spec::builtins().into_iter().find(|s| &*s.id == "py").unwrap());
        use crate::engines::resolve::{Origin, ResolvedEngine};
        use crate::engines::spec::Version;
        let engine = |origin: Origin, version: Version| ResolvedEngine { spec: spec.clone(), origin, argv: vec![python.clone().into_os_string()], version: Some(version), skipped: Vec::new() };
        let told = engine(Origin::Config(crate::config::ConfigSource::Env("V2_ENGINE_PY".into())), Version(3, 13, 9));
        assert!(matches!(pip_for(&told, None), Some(PipFor::Embeddable(true))));
        assert!(matches!(pip_for(&engine(Origin::Path, Version(3, 13, 9)), None), Some(PipFor::Embeddable(false))));
        // Another version's python beside that file isn't the package.
        assert!(pip_for(&engine(Origin::Path, Version(3, 12, 10)), None).is_none());
        let none = "this is Python's embeddable package, which has no pip: for packages, use a full Python (python.org's installer, or the nuget package) and a venv";
        let hint = |dir: &Path, p: PipFor| missing_package_hint("No module named 'humanize'", &site(dir), Some(&p)).unwrap();
        assert_eq!(hint(&d, PipFor::Embeddable(true)), none);
        assert_eq!(hint(&d, PipFor::Embeddable(false)), none);
        // A venv nearby, V2 not told which Python to use: that venv, and why the embeddable can't.
        let proj = temp("embeddable-venv");
        std::fs::create_dir_all(proj.join(".venv")).unwrap();
        std::fs::write(proj.join(".venv").join("pyvenv.cfg"), "home = x\n").unwrap();
        let venv_line = format!("there is a virtual environment in {}, which V2 isn't using: name it in a v2.toml next to it, [python] venv = \".venv\"; and {none}", proj.join(".venv").display());
        assert_eq!(hint(&proj, PipFor::Embeddable(false)), venv_line);
        assert_eq!(hint(&proj, PipFor::Embeddable(true)), none, "V2_ENGINE_PY wins over a venv");
        // Round 22 review (1): with pip (import site uncommented in its ._pth, get-pip.py run),
        // its own pip line, which works there.
        let pip = portable.join("Lib").join("site-packages").join("pip");
        std::fs::create_dir_all(&pip).unwrap();
        assert!(matches!(pip_for(&told, None), Some(PipFor::Told(argv)) if argv == told.argv));
        assert!(pip_for(&engine(Origin::Path, Version(3, 13, 9)), None).is_none());
        // A full Python given a ._pth (its Lib has the venv module) isn't the package either.
        std::fs::remove_dir_all(&pip).unwrap();
        std::fs::create_dir_all(portable.join("Lib").join("venv")).unwrap();
        assert!(matches!(pip_for(&told, None), Some(PipFor::Told(_))));
    }

    #[test]
    fn v2_engine_py_over_a_venv_that_has_the_module_is_named_first() {
        // Round 22 (4): V2_ENGINE_PY, left set, wins over the project's [python] venv; when that
        // venv's site-packages has the module, the hint says so first.
        let d = temp("unused-venv");
        let venv = d.join(".venv");
        let sp = if cfg!(windows) { venv.join("Lib").join("site-packages") } else { venv.join("lib").join("python3.12").join("site-packages") };
        std::fs::create_dir_all(sp.join("tabulate-0.9.0.dist-info")).unwrap();
        std::fs::write(sp.join("tabulate.py"), "").unwrap();
        let unset = if cfg!(windows) { "Remove-Item Env:V2_ENGINE_PY" } else { "unset V2_ENGINE_PY" };
        let note = format!("V2_ENGINE_PY is set, so [python] venv '{}' (v2.toml) isn't used: to use it, {unset}", venv.display());
        let unused = (venv.clone(), note.clone());
        let pip = "if 'tabulate' isn't misspelled, install it for the Python V2 uses: C:\\py\\python.exe -m pip install tabulate".to_string();
        let message = "No module named 'tabulate'";
        assert_eq!(unused_venv_hint(message, &unused, Some(pip.clone()), false), Some(format!("{note}; or {pip}")));
        assert_eq!(unused_venv_hint(message, &unused, Some("this is Python's embeddable package".into()), true), Some(format!("{note}; and this is Python's embeddable package")));
        assert_eq!(unused_venv_hint(message, &unused, None, false), Some(note));
        // A module the venv lacks, or a dotted name (its first part was found): the hint as it was.
        assert_eq!(unused_venv_hint("No module named 'numpy'", &unused, Some(pip.clone()), false), Some(pip.clone()));
        assert_eq!(unused_venv_hint("No module named 'tabulate.x'", &unused, Some(pip.clone()), false), Some(pip.clone()));
        assert_eq!(unused_venv_hint("Cannot find module 'tabulate'", &unused, None, false), None);
    }

    #[test]
    fn a_module_above_is_named_for_the_runs_project() {
        // Round 18: one project serves the run (18.1), so the hint for a NAME.py above the .v2
        // file depends on where the run's project root is.
        let base = temp("projects");
        for (path, text) in [("mono/v2.toml", ""), ("mono/zzroot.py", ""), ("glue/v2.toml", ""), ("proj/v2.toml", ""), ("proj/lib/rooth.py", ""), ("other/h.py", "")] {
            let p = base.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        for d in ["mono/scripts/sub", "proj/lib/sub/deep", "other/x"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        // A .v2 file of another project (its own v2.toml): the run uses the entry's.
        let (mono, glue) = (base.join("mono"), base.join("glue"));
        let sub = mono.join("scripts").join("sub");
        let shared = Site { dir: &sub, file: "../mono/scripts/sub/rootlib.v2", root: &glue, toml: true };
        assert_eq!(
            missing_package_hint("No module named 'zzroot'", &shared, None).unwrap(),
            format!("../mono/scripts/sub/rootlib.v2 belongs to another project ({}, with its own v2.toml), but a run uses the entry script's project ({}): run it from that project, or put the module where this one finds it", mono.display(), glue.display())
        );
        // In a folder of the run's project, below its root: the dotted name from the root.
        let proj = base.join("proj");
        let deep = proj.join("lib").join("sub").join("deep");
        let inside = Site { dir: &deep, file: "lib/sub/deep/m.v2", root: &proj, toml: true };
        assert_eq!(
            missing_package_hint("No module named 'rooth'", &inside, None).unwrap(),
            format!("there is a {} in a folder above: from the project folder, {}, it is py.lib.rooth", proj.join("lib").join("rooth.py").display(), proj.display())
        );
        // Outside the run's project, which has a v2.toml, or outside any project: no v2.toml
        // there would change the run, but the file is named, never pip-installed (review of 18).
        let x = base.join("other").join("x");
        let off_path = |file: &Path| {
            format!("there is a {} in a folder above, which isn't on Python's path: move it next to the .v2 file or into the project folder, or put its folder on sys.path in a @py block first", file.display())
        };
        let outside = Site { dir: &x, file: "../other/x/m.v2", root: &proj, toml: true };
        assert_eq!(missing_package_hint("No module named 'h'", &outside, None).unwrap(), off_path(&base.join("other").join("h.py")));
        let noproj = base.join("noproj");
        let no_project = Site { dir: &x, file: "../other/x/m.v2", root: &noproj, toml: false };
        assert_eq!(missing_package_hint("No module named 'h'", &no_project, None).unwrap(), off_path(&base.join("other").join("h.py")));
        // A folder whose name is no Python name (`2024`, `class`) gives no dotted name, which
        // module_unit would reject.
        for (folder, module) in [("2024", "rooth"), ("class", "rootk")] {
            let file = proj.join(folder).join(format!("{module}.py"));
            std::fs::create_dir_all(proj.join(folder).join("sub")).unwrap();
            std::fs::write(&file, "").unwrap();
            let site = Site { dir: &proj.join(folder).join("sub"), file: "m.v2", root: &proj, toml: true };
            assert_eq!(missing_package_hint(&format!("No module named '{module}'"), &site, None).unwrap(), off_path(&file));
        }
        // Above the root of the project the .v2 file is in: the install line, as for any other.
        std::fs::write(base.join("zzabove.py"), "").unwrap();
        let lib_dir = proj.join("lib");
        let in_project = Site { dir: &lib_dir, file: "lib/m.v2", root: &proj, toml: true };
        assert!(missing_package_hint("No module named 'zzabove'", &in_project, None).unwrap().starts_with("if 'zzabove' isn't misspelled, install it"));
        // Another project's module that isn't there either: the install line, not that project.
        let lib = mono.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        let third = Site { dir: &lib, file: "../mono/lib/third.v2", root: &glue, toml: true };
        assert!(missing_package_hint("No module named 'requestz'", &third, None).unwrap().starts_with("if 'requestz' isn't misspelled, install it"));
        // One in the venv the other project's v2.toml names (pip's dist-info, Unix's and Windows'
        // layouts), or a package folder at its root: that project has it.
        let other = |sp: &str, entry: &str| {
            let root = base.join(format!("venv-{}", entry.replace(['/', '.'], "-")));
            std::fs::create_dir_all(root.join(sp).join(entry)).unwrap();
            std::fs::create_dir_all(root.join("scripts")).unwrap();
            std::fs::write(root.join("v2.toml"), "[python]\nvenv = \".venv\"\n").unwrap();
            let scripts = root.join("scripts");
            let hint = missing_package_hint("No module named 'acme'", &Site { dir: &scripts, file: "lib.v2", root: &glue, toml: true }, None).unwrap();
            assert!(hint.starts_with(&format!("lib.v2 belongs to another project ({}, ", root.display())), "{sp}/{entry}: {hint}");
        };
        other(".venv/lib/python3.12/site-packages", "acme-0.1.0.dist-info");
        other(".venv/Lib/site-packages", "acme");
        let pkg = base.join("pkgproj");
        std::fs::create_dir_all(pkg.join("acme")).unwrap();
        std::fs::create_dir_all(pkg.join("s")).unwrap();
        std::fs::write(pkg.join("v2.toml"), "").unwrap();
        let hint = missing_package_hint("No module named 'acme'", &Site { dir: &pkg.join("s"), file: "s.v2", root: &glue, toml: true }, None).unwrap();
        assert!(hint.starts_with("s.v2 belongs to another project"), "{hint}");
        assert!(missing_package_hint("No module named 'acmex'", &Site { dir: &pkg.join("s"), file: "s.v2", root: &glue, toml: true }, None).unwrap().starts_with("if 'acmex'"));
    }

    #[test]
    fn an_es_module_importing_from_a_commonjs_sibling_is_told_why() {
        // Round 16: `import { add } from './vec.ts'` in physics.mts, where package.json says
        // "type": "commonjs": Node says vec.ts has no export 'add'.
        let d = temp("cjs-sibling");
        std::fs::write(d.join("package.json"), "{\"type\": \"commonjs\"}").unwrap();
        let m = "The requested module './vec.ts' does not provide an export named 'add'";
        let hint = nested_commonjs_hint(m, "\"./physics.mts\"", &d).unwrap();
        assert!(hint.ends_with("says \"type\": \"commonjs\", so Node loads ./vec.ts as CommonJS, whose exports an import can't name: set \"type\": \"module\" there, or rename it to .mts"), "{hint}");
        assert!(nested_commonjs_hint(&m.replace("vec.ts", "vec.mts"), "\"./physics.mts\"", &d).is_none(), "an .mts is ESM anyway");
        std::fs::write(d.join("package.json"), "{\"type\": \"module\"}").unwrap();
        assert!(nested_commonjs_hint(m, "\"./physics.mts\"", &d).is_none(), "an ES module package: the import-type hint applies");
    }

    #[test]
    fn a_typescript_files_own_imports_get_hints() {
        // 13.5: tsc's `./math.js` or `./math` for a math.ts, and a type imported as a value.
        let d = temp("ts");
        std::fs::write(d.join("math.ts"), "export const x = 1\n").unwrap();
        let importer = d.join("app.ts");
        for missing in ["math.js", "math"] {
            let m = format!("Cannot find module '{}' imported from {}", d.join(missing).display(), importer.display());
            let want = format!("Node runs .ts files as they are written, so imports between them name the .ts file: write math.ts for {missing} in app.ts");
            assert_eq!(ts_import_hint(&m).as_deref(), Some(&*want));
        }
        let elsewhere = format!("Cannot find module '{}' imported from {}", d.join("other.js").display(), importer.display());
        assert_eq!(ts_import_hint(&elsewhere), None, "no math.ts for it");
        let from_js = format!("Cannot find module '{}' imported from {}", d.join("math.js").display(), d.join("app.js").display());
        assert_eq!(ts_import_hint(&from_js), None, "a .js file's import");
        let m = "The requested module './types.ts' does not provide an export named 'Shape'";
        assert_eq!(
            ts_import_hint(m).as_deref(),
            Some("if 'Shape' is a type or an interface, import it with import type { Shape } from './types.ts': Node removes types as it runs a .ts file, so they aren't exports")
        );
        assert_eq!(ts_import_hint("The requested module './x.js' does not provide an export named 'y'"), None);
    }

    /// A Site for a .v2 file in `dir`, outside any project (the merge of round 18's js and py work).
    fn site_at(dir: &Path) -> Site<'_> {
        Site { dir, file: "x.v2", root: dir, toml: false }
    }

    #[test]
    fn a_yarn_plug_n_play_project_is_named_in_the_hint() {
        // Round 18: Yarn's PnP keeps packages out of node_modules, so `npm install` isn't the fix.
        let d = temp("pnp");
        std::fs::write(d.join("package.json"), "{}").unwrap();
        std::fs::create_dir_all(d.join("sub")).unwrap();
        let npm = missing_package_hint("Cannot find module 'lodash'", &site_at(&d.join("sub")), None).unwrap();
        assert!(npm.starts_with("if 'lodash' isn't misspelled, install it with npm install lodash"), "{npm}");
        std::fs::write(d.join(".pnp.cjs"), "").unwrap();
        let want = format!(
            "this project uses Yarn Plug'n'Play ({}), which Node can't read without Yarn's loader: set nodeLinker: node-modules in .yarnrc.yml and run yarn install",
            d.join(".pnp.cjs").display()
        );
        for dir in [d.clone(), d.join("sub")] {
            assert_eq!(missing_package_hint("Cannot find module 'lodash'", &site_at(&dir), None).as_deref(), Some(&*want));
        }
        // A file of the script's own still gets its hint, and a local path its own.
        std::fs::write(d.join("helpers.js"), "").unwrap();
        assert!(missing_package_hint("Cannot find module 'helpers'", &site_at(&d), None).unwrap().starts_with("your own helpers.js"));
        assert_eq!(missing_package_hint("Cannot find module './x.js'", &site_at(&d), None).as_deref(), Some("a local file is found from the .v2 file's folder: check the path"));
    }

    #[test]
    fn a_file_or_folder_of_the_scripts_own_keeps_its_folders_in_the_hint() {
        // Review of round 19: `lib/math` for lib/math.ts is the script's own file, named with its
        // folders, and `lib/…` for a folder lib of its own is never `npm install lib`; a name
        // with an extension that isn't there may be an npm package's own (chart.js).
        let d = temp("own-folders");
        std::fs::create_dir_all(d.join("lib")).unwrap();
        std::fs::write(d.join("lib").join("math.ts"), "").unwrap();
        let import = |spec: &str| missing_package_hint(&format!("Cannot find module '{spec}'"), &site_at(&d), None).unwrap();
        assert_eq!(import("lib/math"), "your own lib/math.ts is imported by its path from the .v2 file's folder: @import { … } from ts.\"./lib/math.ts\" (js.\"lib/math\", without ./, names an npm package)");
        assert_eq!(import("lib/math.ts"), "your own lib/math.ts is imported by its path from the .v2 file's folder: @import { … } from ts.\"./lib/math.ts\" (js.\"lib/math.ts\", without ./, names an npm package)");
        assert_eq!(import("lib/mathh"), "lib is a folder of your own, whose files are imported by their paths from the .v2 file's folder, as in js.\"./lib/mathh\" (js.\"lib/mathh\", without ./, names an npm package)");
        assert_eq!(import("lib"), "lib is a folder of your own, whose files are imported by their paths from the .v2 file's folder, as in js.\"./lib/…\" (js.lib, without ./, names an npm package)");
        assert_eq!(import("lib/x.js"), "a file of your own is imported by its path from the .v2 file's folder, as in js.\"./lib/x.js\": check its name (js.\"lib/x.js\", without ./, names an npm package)");
        assert_eq!(
            import("chart.js"),
            format!("a file of your own is imported by its path from the .v2 file's folder, as in js.\"./chart.js\": check its name (js.\"chart.js\", without ./, names an npm package); or, for the npm package chart.js, npm install chart.js in {}", d.display())
        );
        assert!(import("libb/math").starts_with("if 'libb' isn't misspelled, install it with npm install libb in "), "no folder libb");
        // A block's require() names the whole specifier; its import() only the package part.
        let v2 = d.join("main.v2");
        let require = |spec: &str| block_module_hint(&format!("Cannot find module '{spec}'\nRequire stack:\n- {}", v2.display()), "MODULE_NOT_FOUND").unwrap();
        let dynamic = |package: &str| block_module_hint(&format!("Cannot find package '{package}' imported from {}", v2.display()), "ERR_MODULE_NOT_FOUND").unwrap();
        assert_eq!(require("lib/math"), "your own lib/math.ts is loaded by its path from the .v2 file's folder: require(\"./lib/math.ts\") (\"lib/math\", without ./, names an npm package)");
        assert_eq!(require("lib/mathh"), "lib is a folder of your own, whose files are loaded by their paths from the .v2 file's folder, as in require(\"./lib/mathh\") (\"lib/mathh\", without ./, names an npm package)");
        assert_eq!(dynamic("lib"), "lib is a folder of your own, whose files are loaded by their paths from the .v2 file's folder, as in import(\"./lib/…\") (\"lib\", without ./, names an npm package)");
        assert_eq!(
            require("chart.js"),
            format!("a file of your own is loaded by its path from the .v2 file's folder, as in require(\"./chart.js\"): check its name (\"chart.js\", without ./, names an npm package); or, for the npm package chart.js, npm install chart.js in {}", d.display())
        );
        assert_eq!(require("lib/x.js"), "a file of your own is loaded by its path from the .v2 file's folder, as in require(\"./lib/x.js\"): check its name (\"lib/x.js\", without ./, names an npm package)");
        // The second review of round 19: a folder of data named alone (node-config's config/) may
        // be the npm package's own layout: its install line too; never for import(), whose message
        // names only the package part, nor for a folder with code in it (lib/math.ts above).
        std::fs::create_dir_all(d.join("config")).unwrap();
        std::fs::write(d.join("config").join("default.json"), "{}").unwrap();
        let config = format!("; or, for the npm package config, npm install config in {}", d.display());
        assert_eq!(require("config"), format!("config is a folder of your own, whose files are loaded by their paths from the .v2 file's folder, as in require(\"./config/…\") (\"config\", without ./, names an npm package){config}"));
        assert!(import("config").ends_with(&config), "{}", import("config"));
        assert!(!dynamic("config").contains("npm install"), "{}", dynamic("config"));
        assert!(!require("config/default").contains("npm install"), "{}", require("config/default"));
        assert!(!import("lib").contains("npm install"), "{}", import("lib"));
        // A name with an extension that nearly spells an installed package: that one, never an
        // install of the typo; in a Yarn PnP project, Yarn's fix instead of npm install.
        std::fs::create_dir_all(d.join("node_modules").join("bignumber.js")).unwrap();
        let own = "a file of your own is loaded by its path from the .v2 file's folder, as in require(\"./bignumer.js\"): check its name (\"bignumer.js\", without ./, names an npm package)";
        let install = format!("if 'bignumer.js' isn't misspelled, npm install bignumer.js in {}", d.display());
        assert_eq!(require("bignumer.js"), format!("{own}; did you mean \"bignumber.js\"? {install}"));
        assert!(import("bignumer.js").ends_with(&format!("names an npm package); did you mean js.\"bignumber.js\"? {install}")), "{}", import("bignumer.js"));
        // The third review: code only in a subfolder is still the script's own; a scope folder
        // (@prisma) or a dot entry (.bin) is no package to suggest.
        std::fs::create_dir_all(d.join("src").join("util")).unwrap();
        std::fs::write(d.join("src").join("util").join("index.js"), "").unwrap();
        assert!(!require("src").contains("npm install"), "{}", require("src"));
        std::fs::create_dir_all(d.join("node_modules").join("@prisma").join("client")).unwrap();
        std::fs::create_dir_all(d.join("node_modules").join(".bin")).unwrap();
        std::fs::create_dir_all(d.join("prisma")).unwrap();
        std::fs::write(d.join("prisma").join("schema.prisma"), "").unwrap();
        assert!(require("prisma").ends_with(&format!("; or, for the npm package prisma, npm install prisma in {}", d.display())), "{}", require("prisma"));
        assert!(require("bin").starts_with("if 'bin' isn't misspelled"), "{}", require("bin"));
        assert!(require("@prisma/clinet").starts_with("did you mean \"@prisma/client\"? "), "{}", require("@prisma/clinet"));
        std::fs::write(d.join(".pnp.cjs"), "").unwrap();
        assert!(
            require("chart.js").ends_with(&format!("; or, for the npm package chart.js: this project uses Yarn Plug'n'Play ({}), which Node can't read without Yarn's loader: set nodeLinker: node-modules in .yarnrc.yml and run yarn install", d.join(".pnp.cjs").display())),
            "{}",
            require("chart.js")
        );
    }

    #[test]
    fn a_blocks_own_require_gets_the_installed_package_near_names_and_yarn_in_its_words() {
        // Review of round 19, 13.5: the hints an @import gets, with the block's did-you-mean.
        let d = temp("block-npm");
        std::fs::create_dir_all(d.join("node_modules").join("lodash")).unwrap();
        let v2 = d.join("main.v2");
        let require = |spec: &str| block_module_hint(&format!("Cannot find module '{spec}'\nRequire stack:\n- {}", v2.display()), "MODULE_NOT_FOUND");
        let place = format!("{} or a folder above it, where Node looks for this file", d.display());
        assert_eq!(require("lodsh").unwrap(), format!("did you mean \"lodash\"? if 'lodsh' isn't misspelled, install it with npm install lodsh in {place}"));
        assert_eq!(require("lodash/chunck").unwrap(), "'lodash' is installed, but has no 'chunck': check the path after 'lodash/'");
        let dynamic = block_module_hint(&format!("Cannot find package 'lodsh' imported from {}", v2.display()), "ERR_MODULE_NOT_FOUND").unwrap();
        assert!(dynamic.starts_with("did you mean \"lodash\"? if 'lodsh'"), "{dynamic}");
        std::fs::write(d.join(".pnp.cjs"), "").unwrap();
        assert_eq!(
            require("lodsh").unwrap(),
            format!("this project uses Yarn Plug'n'Play ({}), which Node can't read without Yarn's loader: set nodeLinker: node-modules in .yarnrc.yml and run yarn install", d.join(".pnp.cjs").display())
        );
        // Not a block's (a module it required is first on the stack), and relative paths: none.
        assert_eq!(block_module_hint(&format!("Cannot find module 'lodsh'\nRequire stack:\n- {}", d.join("x.js").display()), "MODULE_NOT_FOUND"), None);
        assert_eq!(require("./x.js"), None);
    }

    #[test]
    fn a_tsconfig_paths_alias_is_named_in_the_hint() {
        // Round 18: Node doesn't read tsconfig.json, whose "paths" a TypeScript project may use.
        let d = temp("tsconfig");
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("tsconfig.base.json"), "\u{feff}{\n  // shared\n  \"compilerOptions\": {\n    \"paths\": {\n      \"@lib/*\": [\"lib/*\"], /* a comment, \"*/\n      \"~/*\": [\"src/*\"],\n      \"utils\": [\"src/utils.ts\"],\n      \"*\": [\"types/*\"],\n    },\n  },\n}\n").unwrap();
        std::fs::write(d.join("tsconfig.json"), "{ \"extends\": \"./tsconfig.base\", \"include\": [\"src/**/*\", \"a,]\"], }").unwrap();
        let base = d.join("tsconfig.base.json");
        let (file, keys) = tsconfig_paths(&d.join("src")).unwrap();
        assert_eq!(file, base);
        assert_eq!(keys, ["@lib/*", "~/*", "utils", "*"]);
        let hint = |key: &str| format!("\"{key}\" is a \"paths\" alias in {}, and Node doesn't read tsconfig paths: import by a relative path, or use package.json \"imports\" (#alias)", base.display());
        // The .v2 file's own @import, which names the whole specifier.
        let missing = |spec: &str| missing_package_hint(&format!("Cannot find module '{spec}'"), &site_at(&d.join("src")), None).unwrap();
        assert_eq!(missing("@lib/math"), hint("@lib/*"));
        assert_eq!(missing("~/x/y"), hint("~/*"));
        assert_eq!(missing("utils"), hint("utils"));
        assert!(missing("lodash").starts_with("if 'lodash' isn't misspelled"), "the catch-all * is no alias of every name");
        // A .ts file's import: Node names a bare specifier's package part (`~` for `~/utils`); a
        // .cts file's require names it whole, with the file first on its require stack.
        let ts = d.join("src").join("app.ts");
        assert_eq!(ts_import_hint(&format!("Cannot find package '~' imported from {}", ts.display())), Some(hint("~/*")));
        assert_eq!(ts_import_hint(&format!("Cannot find package '@lib/math' imported from {}", ts.display())), Some(hint("@lib/*")));
        let cts = d.join("src").join("old.cts");
        assert_eq!(ts_import_hint(&format!("Cannot find module '~/utils'\nRequire stack:\n- {}\n- {}", cts.display(), d.join("x.js").display())), Some(hint("~/*")));
        assert_eq!(ts_import_hint(&format!("Cannot find package 'lodash' imported from {}", ts.display())), None);
        assert_eq!(ts_import_hint(&format!("Cannot find package '~' imported from {}", d.join("src").join("app.js").display())), None, "a .js file's import");
        // Without a tsconfig.json: none.
        assert_eq!(tsconfig_paths(&temp("no-tsconfig")), None);
    }

    #[test]
    fn tsconfig_text_becomes_plain_json() {
        assert_eq!(plain_json("{\"a\": \"// not a comment\", \"b\": [1, 2,], /* c */ \"d\": \"\\\",]\",}"), "{\"a\": \"// not a comment\", \"b\": [1, 2],   \"d\": \"\\\",]\"}");
        assert!(alias_matches("@/*", "@/lib") && alias_matches("~/*", "~") && alias_matches("@lib/*", "@lib/math"));
        assert!(!alias_matches("*", "lodash") && !alias_matches("@lib/*", "@li") && !alias_matches("utils", "util"));
    }

    #[test]
    fn a_typescript_type_imported_as_a_value_is_named_in_the_hint() {
        // Round 18: `@import { Shape } from ts."./types.ts"`, where Shape is a type.
        let d = temp("ts-type");
        std::fs::write(d.join("types.ts"), "export interface Point { x: number }\nexport declare type Shape = 'a';\n  type Local<T> = T\nexport const ShapeKinds = 2\n").unwrap();
        let hint = |name: &str| ts_type_hint(name, "./types.ts", &d);
        assert_eq!(hint("Point").as_deref(), Some("Point is a type, which TypeScript removes before it runs: there is nothing to import; use it in TypeScript only"));
        assert!(hint("Shape").is_some() && hint("Local").is_some());
        assert_eq!(hint("ShapeKinds"), None, "a value");
        assert_eq!(hint("Shap"), None);
        std::fs::write(d.join("types.js"), "export type Shape = 1\n").unwrap();
        assert_eq!(ts_type_hint("Shape", "./types.js", &d), None, "a .js file has no types");
        assert_eq!(ts_type_hint("Shape", "types", &d), None, "a package");
    }
}
