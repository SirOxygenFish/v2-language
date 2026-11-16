//! V2 modules: parsing the import graph at startup and loading files (INTERNALS 6.1, 6.4).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::syntax::ast::*;
use crate::syntax::span::{absolute_clean, display_path, os_message, shown_path, FileId, LoadError, SourceMap, Span};
use crate::syntax::{check, lexer, parser, Diag};

/// A static diagnostic of a module file, kept until an `import` of it runs (6.1 step 2).
pub struct ModuleDiag {
    pub display: Rc<str>,
    pub line: u32,
    pub col: u32,
    pub message: String,
    /// The line and caret, and the hint, as `v2 check` prints them (2.4, 6.4).
    pub excerpt: String,
    pub hint: Option<String>,
}

/// A file read at startup or on its first import: its program, or its diagnostics.
pub enum Parsed {
    Ok { file: FileId, program: Rc<Vec<Stmt>> },
    Failed(Vec<ModuleDiag>),
}

fn module_diag(sm: &SourceMap, d: &Diag) -> ModuleDiag {
    ModuleDiag {
        display: sm.file(d.span.file).display.clone(),
        line: d.span.line,
        col: d.span.col,
        message: d.message.lines().next().unwrap_or("").to_string(),
        excerpt: crate::runtime::error::excerpt(sm, d.span),
        hint: d.hint.clone(),
    }
}

/// Load, lex, parse and check one file. `Err` holds what to print for the entry file;
/// `Parsed::Failed` holds a module's diagnostics.
pub fn parse_file(sm: &mut SourceMap, path: &Path) -> Result<Parsed, LoadError> {
    let file = match sm.load(path) {
        Ok(id) => id,
        Err(LoadError::NotUtf8 { display, line, col, message }) => {
            return Ok(Parsed::Failed(vec![ModuleDiag { display, line, col, message: message.into(), excerpt: String::new(), hint: None }]))
        }
        Err(e) => return Err(e),
    };
    let mut program = match lexer::lex(sm, file).and_then(|toks| parser::parse_program(sm, toks)) {
        Ok(p) => p,
        Err(d) => return Ok(Parsed::Failed(vec![module_diag(sm, &d)])),
    };
    let diags = check::check(&program);
    if !diags.is_empty() {
        return Ok(Parsed::Failed(diags.iter().map(|d| module_diag(sm, d)).collect()));
    }
    crate::syntax::resolve::resolve(&mut program);
    Ok(Parsed::Ok { file, program: Rc::new(program) })
}

/// Where `import "spec"` in `importer` points: relative to the importing file's folder,
/// absolute paths as is; no extension is added (6.4 step 1).
pub fn resolve(sm: &SourceMap, importer: FileId, spec: &str) -> PathBuf {
    let base = sm.file(importer).path.parent().map(Path::to_path_buf).unwrap_or_default();
    base.join(spec)
}

/// The file `import "spec"` in `importer` loads, canonical, or the `ImportError` message (6.4),
/// with control characters escaped (2.1). A drive-relative Windows path (`C:m.v2`) is neither
/// relative to the file nor absolute.
pub fn locate(sm: &SourceMap, importer: FileId, spec: &str) -> Result<PathBuf, String> {
    locate_noted(sm, importer, spec).map(|(path, _)| path)
}

/// `locate`, and the note for a spelling that differs from the file's name on disk only in case
/// (`case_note`). A file already loaded under another canonical path (`SourceMap::same_file`) is
/// that one.
pub fn locate_noted(sm: &SourceMap, importer: FileId, spec: &str) -> Result<(PathBuf, Option<String>), String> {
    let found = locate_raw(sm, importer, spec).map_err(|m| shown_path(&m))?;
    let note = case_note(&resolve(sm, importer, spec), spec, &found);
    match sm.same_file(&found) {
        Some(id) => Ok((sm.file(id).path.clone(), note)),
        None => Ok((found, note)),
    }
}

thread_local! {
    /// Import sites whose case note was printed: one note per site in a run (6.4).
    static NOTED: std::cell::RefCell<std::collections::HashSet<(u32, u32)>> = Default::default();
}

/// Whether the case note for the import at `at` is still to be printed (once per site).
pub fn first_note(at: Span) -> bool {
    NOTED.with(|n| n.borrow_mut().insert((at.file.0, at.lo)))
}

/// `import "./REAL/Counter.V2"` that found `real/counter.v2`, on a file system that ignores case
/// (Windows', macOS's): `note: './REAL/Counter.V2' is spelled './real/counter.v2' on disk: …`,
/// since a case-sensitive one won't find it. The written names at its end are compared with the
/// canonical path's last ones; a link among them (whose target has its own name) or any other
/// difference (an 8.3 short name) is no case problem (6.4).
pub fn case_note(target: &Path, spec: &str, found: &Path) -> Option<String> {
    // The written names at the end, each with where it starts in `spec`.
    let mut parts: Vec<(usize, &str)> = Vec::new();
    let mut start = 0;
    for (i, c) in spec.char_indices().chain(std::iter::once((spec.len(), '/'))) {
        if i == spec.len() || std::path::is_separator(c) {
            parts.push((start, &spec[start..i]));
            start = i + c.len_utf8();
        }
    }
    let plain = |p: &str| !p.is_empty() && p != "." && p != ".." && !(p.len() == 2 && p.ends_with(':'));
    let k = parts.iter().rev().take_while(|(_, p)| plain(p)).count();
    let written = &parts[parts.len() - k..];
    let real: Vec<String> = found.components().rev().take(k).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if k == 0 || real.len() < k {
        return None;
    }
    let real: Vec<&String> = real.iter().rev().collect();
    let same_but_case = written.iter().zip(&real).all(|((_, w), r)| w.to_lowercase() == r.to_lowercase());
    if !same_but_case || written.iter().zip(&real).all(|((_, w), r)| *w == r.as_str()) {
        return None;
    }
    let mut at = target.to_path_buf();
    for _ in 0..k {
        if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            return None;
        }
        at.pop();
    }
    let sep = if spec.contains('/') || !spec.contains('\\') { "/" } else { "\\" };
    let spelled = format!("{}{}", &spec[..written[0].0], real.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(sep));
    Some(shown_path(&format!("note: '{spec}' is spelled '{spelled}' on disk: a case-sensitive file system, as Linux's, won't find it")))
}

/// A link on the way to a path that can't be followed (DOCS 3.10): one to something that isn't
/// there, or one in a loop. Windows' junctions are links too (`FileType::is_symlink` says so).
pub struct LinkProblem {
    /// The path up to and including the link, spelled as the path was.
    pub at: PathBuf,
    /// What a link that leads nowhere names (`None` when it can't be read); `None` for a loop.
    target: Option<PathBuf>,
    looped: bool,
    /// A link to a folder (a junction, a directory symlink): Windows removes it with rmdir.
    dir: bool,
}

impl LinkProblem {
    /// `'brokenjunc' is a link to C:\x\gone, which isn't there`, or `… is a link in a loop`, with
    /// `name` (`'brokenjunc'`, or `it` for the path itself).
    pub fn problem(&self, name: &str) -> String {
        match (&self.target, self.looped) {
            (_, true) => format!("{name} is a link in a loop"),
            (Some(to), false) => format!("{name} is a link to {}, which isn't there", crate::syntax::span::strip_verbatim(to).display()),
            (None, false) => format!("{name} is a link to something that isn't there"),
        }
    }

    /// What fixes it, for a message that ends with what to do: `remove the link (rmdir
    /// brokenjunc), or make C:\x\gone` (`del` for a file's link on Windows, `rm` elsewhere).
    pub fn fix(&self) -> String {
        let remove = match (cfg!(windows), self.dir) {
            (true, true) => "rmdir",
            (true, false) => "del",
            (false, _) => "rm",
        };
        let link = crate::cli::shell_word(&self.at.to_string_lossy());
        match (&self.target, self.looped) {
            (Some(to), false) => format!("remove the link ({remove} {link}), or make {}", crate::syntax::span::strip_verbatim(to).display()),
            _ => format!("remove the link ({remove} {link}), or point it somewhere else"),
        }
    }
}

/// The start of `path` as written that is the path `at`, one of its beginnings (`a/link` of
/// `a/link/x.txt`, where `at` would print `a\link` on Windows).
pub fn written_prefix<'a>(path: &'a str, at: &Path) -> std::borrow::Cow<'a, str> {
    let ends = path.char_indices().filter(|&(_, c)| std::path::is_separator(c)).map(|(i, _)| i).chain(std::iter::once(path.len()));
    match ends.map(|i| &path[..i]).find(|p| Path::new(p) == at) {
        Some(p) => p.into(),
        None => at.to_string_lossy().into_owned().into(),
    }
}

/// Whether `e` says that links lead in a loop: `ELOOP`, or Windows'
/// `ERROR_CANT_RESOLVE_FILENAME` (1921, "The name of the file cannot be resolved by the system").
pub fn is_link_loop(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    return e.raw_os_error() == Some(libc::ELOOP);
    #[cfg(windows)]
    return e.raw_os_error() == Some(1921);
}

/// The first link on the way to `path`, or `path` itself, that leads nowhere or in a loop: each
/// part is looked at with `symlink_metadata`, then a link followed with `metadata` (DOCS 3.10).
/// `None` when every link leads somewhere, or a part can't be looked at (missing, unreadable).
pub fn link_problem(path: &Path) -> Option<LinkProblem> {
    let mut at = PathBuf::new();
    for c in path.components() {
        at.push(c);
        if !matches!(c, std::path::Component::Normal(_)) {
            continue;
        }
        let kind = std::fs::symlink_metadata(&at).ok()?.file_type();
        if !kind.is_symlink() {
            continue;
        }
        #[cfg(windows)]
        let dir = std::os::windows::fs::FileTypeExt::is_symlink_dir(&kind);
        #[cfg(unix)]
        let dir = false;
        match std::fs::metadata(&at) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Some(LinkProblem { target: std::fs::read_link(&at).ok(), at, looped: false, dir });
            }
            Err(e) if is_link_loop(&e) => return Some(LinkProblem { at, target: None, looped: true, dir }),
            Err(_) => return None,
        }
    }
    None
}

/// A file builtin's failure on a path through a link that leads nowhere or in a loop (5.10):
/// `IOError: cannot write 'brokenjunc/new.txt': 'brokenjunc' is a link to C:\x\gone, which isn't
/// there`, `hint: remove the link (rmdir brokenjunc), or make C:\x\gone`, not a `make_dir` hint
/// that would fail, nor Windows' "Access is denied".
pub fn link_error(verb: &str, path: &str) -> Option<crate::runtime::error::ErrorValue> {
    let link = link_problem(Path::new(path))?;
    let name = if link.at == Path::new(path) { "it".to_string() } else { format!("'{}'", written_prefix(path, &link.at)) };
    let err = crate::runtime::error::ErrorValue::new("IOError", shown_path(&format!("cannot {verb} '{path}': {}", link.problem(&name))));
    let _ = err.hint.set(shown_path(&link.fix()).into());
    Some(err)
}

fn locate_raw(sm: &SourceMap, importer: FileId, spec: &str) -> Result<PathBuf, String> {
    let p = Path::new(spec);
    if matches!(p.components().next(), Some(std::path::Component::Prefix(_))) && !p.has_root() {
        return Err(format!("cannot find module '{spec}' (a drive-relative path: write it relative to the file, or absolute)"));
    }
    let target = resolve(sm, importer, spec);
    let found = match std::fs::canonicalize(&target) {
        Ok(found) => crate::syntax::span::strip_verbatim(&found),
        // There, but it can't be followed: a link to a file that isn't there, a link in a loop
        // (ELOOP, Windows' 1921), or the OS's reason, as for FILE (19.1).
        // A link to an anonymous pipe (/dev/stdin from a pipe, /proc/self/fd/0, <(…)) is there
        // to the OS, which follows /proc's links, though canonicalize can't name its target.
        Err(e) if std::fs::symlink_metadata(&target).is_ok() => {
            if std::fs::metadata(&target).is_ok_and(|m| !m.is_dir() && !m.is_file()) {
                return Err(format!("cannot import '{spec}': it is a pipe or a device, not a file"));
            }
            return Err(match std::fs::read_link(&target) {
                Ok(to) if e.kind() == std::io::ErrorKind::NotFound => {
                    format!("cannot import '{spec}': it is a symbolic link to {}, which isn't there", crate::syntax::span::strip_verbatim(&to).to_string_lossy())
                }
                _ if is_link_loop(&e) => format!("cannot import '{spec}': it is a link in a loop"),
                _ => format!("cannot import '{spec}': {}", os_message(&e)),
            });
        }
        Err(e) => {
            // A link on the way that leads nowhere or in a loop, a junction too (DOCS 3.10).
            if let Some(link) = link_problem(&target) {
                return Err(format!("cannot import '{spec}': {}", link.problem(&format!("'{}'", shown(&link.at)))));
            }
            // There, in a folder V2 may not read (a deny inherited from it): the OS's reason.
            let lstat = std::fs::symlink_metadata(&target).err();
            if e.kind() == std::io::ErrorKind::PermissionDenied || lstat.is_some_and(|l| l.kind() == std::io::ErrorKind::PermissionDenied) {
                return Err(format!("cannot import '{spec}': {}", os_message(&e)));
            }
            // `import "./util" as util` for util.v2.
            let with = target.with_file_name(format!("{}.v2", target.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
            let hint = if spec.ends_with(".v2") || !with.is_file() { String::new() } else { format!(" — did you mean '{spec}.v2'?") };
            return Err(format!("cannot find module '{spec}' (looked for {}){hint}", shown(&target)));
        }
    };
    // A pipe or a device (a FIFO, a link to /dev/zero) would never end being read; the entry
    // FILE is refused the same way (19.1).
    if std::fs::metadata(&found).is_ok_and(|m| !m.is_dir() && !m.is_file()) {
        return Err(format!("cannot import '{spec}': it is a pipe or a device, not a file"));
    }
    // A folder: one of its .v2 files is what to write (Windows would say "Access is denied").
    if found.is_dir() {
        let mut names: Vec<String> = std::fs::read_dir(&found)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.to_ascii_lowercase().ends_with(".v2"))
            .collect();
        names.sort();
        let file = names.first().map_or("FILE.v2", String::as_str);
        let stem = &file[..file.len() - 3];
        let plain = stem.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') && stem.chars().all(|c| c.is_alphanumeric() || c == '_');
        let alias = if plain && file != "FILE.v2" { stem } else { "m" };
        let joined = format!("{}/{file}", spec.trim_end_matches(['/', '\\']));
        return Err(format!("cannot import '{spec}': it is a folder, and import loads a .v2 file, as in import \"{joined}\" as {alias}"));
    }
    // `import` loads V2 files: another language's file, or data, is used another way.
    let ext = found.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    let Some(ext) = ext.filter(|e| e != "v2") else { return Ok(found) };
    let rel = spec.strip_prefix("./").or_else(|| spec.strip_prefix(".\\")).unwrap_or(spec);
    let next = match ext.as_str() {
        "py" if !rel.contains("..") && Path::new(rel).is_relative() => {
            let module = rel.trim_end_matches(".py").replace(['/', '\\'], ".");
            format!("a Python file is imported with @import {{ name }} from py.{module}")
        }
        "py" => "a Python file next to a .v2 file is imported with @import { name } from py.NAME".to_string(),
        "js" | "mjs" | "cjs" => format!("a JavaScript file is imported with @import {{ name }} from js.\"{spec}\""),
        "ts" | "mts" | "cts" => format!("a TypeScript file is imported with @import {{ name }} from ts.\"{spec}\""),
        // An absolute path is read as it is: script_dir() would come before it (round 17).
        _ if Path::new(rel).is_absolute() || Path::new(rel).has_root() => {
            let mut path = String::new();
            crate::runtime::value::quoted(&rel.replace('\\', "/"), &mut path);
            format!("read a data file with read_file({path})")
        }
        _ => format!("read a data file with read_file(f\"${{script_dir()}}/{}\")", rel.replace('\\', "/")),
    };
    Err(format!("cannot import '{spec}': import loads .v2 files; {next}"))
}

/// `fs::canonicalize` without the `\\?\` prefix, or `None` when the file is missing.
pub fn canonical(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok().map(|p| crate::syntax::span::strip_verbatim(&p))
}

/// 6.1 step 2: every file the `import` literals of `program` name, depth-first in source order.
/// Missing files are skipped (their `import` fails when it runs).
pub fn parse_graph(sm: &mut SourceMap, file: FileId, program: &[Stmt], parsed: &mut HashMap<PathBuf, Parsed>) {
    // Depth-first with an explicit stack: an import chain can be longer than the host stack.
    let entry = sm.file(file).path.clone();
    let mut stack = vec![(file, import_sites(program).into_iter())];
    while let Some((importer, sites)) = stack.last_mut() {
        let importer = *importer;
        let Some((spec, _)) = sites.next() else {
            stack.pop();
            continue;
        };
        let Ok(path) = locate(sm, importer, &spec) else { continue };
        if parsed.contains_key(&path) || path == entry {
            continue;
        }
        let Ok(result) = parse_file(sm, &path) else { continue };
        let next = match &result {
            Parsed::Ok { file, program } => Some((*file, import_sites(program).into_iter())),
            Parsed::Failed(_) => None,
        };
        parsed.insert(path, result);
        if let Some(frame) = next {
            stack.push(frame);
        }
    }
}

/// Every `import "…"` in a program with its statement's span, in source order, at any depth
/// (6.4: imports may appear in any body, including function bodies).
pub fn import_sites(program: &[Stmt]) -> Vec<(Rc<str>, Span)> {
    let mut out = Vec::new();
    collect_imports(program, &mut out);
    out
}

fn collect_imports(stmts: &[Stmt], out: &mut Vec<(Rc<str>, Span)>) {
    for s in stmts {
        stmt_imports(s, out);
    }
}

fn body_imports(b: &Body, out: &mut Vec<(Rc<str>, Span)>) {
    collect_imports(&b.stmts, out);
}

fn stmt_imports(s: &Stmt, out: &mut Vec<(Rc<str>, Span)>) {
    match &s.kind {
        StmtKind::Import { path, .. } => out.push((path.clone(), s.span)),
        StmtKind::Let(_, e) => e.iter().for_each(|e| expr_imports(e, out)),
        StmtKind::Const(_, e) | StmtKind::Expr(e) | StmtKind::Throw(e) => expr_imports(e, out),
        StmtKind::Return(e) => e.iter().for_each(|e| expr_imports(e, out)),
        StmtKind::Assign { target, value, .. } => {
            match target {
                Target::Name(..) => {}
                Target::Member(o, _) => expr_imports(o, out),
                Target::Index(o, i, _) => {
                    expr_imports(o, out);
                    expr_imports(i, out);
                }
            }
            expr_imports(value, out);
        }
        StmtKind::If { arms, else_ } => {
            for (c, b) in arms {
                expr_imports(c, out);
                body_imports(b, out);
            }
            else_.iter().for_each(|b| body_imports(b, out));
        }
        StmtKind::While { cond, body } => {
            expr_imports(cond, out);
            body_imports(body, out);
        }
        StmtKind::For { iter, body, .. } => {
            match iter {
                ForIter::Value(e) => expr_imports(e, out),
                ForIter::Range(a, b) => {
                    expr_imports(a, out);
                    expr_imports(b, out);
                }
            }
            body_imports(body, out);
        }
        StmtKind::Try { body, catch, finally } => {
            body_imports(body, out);
            catch.iter().for_each(|(_, b)| body_imports(b, out));
            finally.iter().for_each(|b| body_imports(b, out));
        }
        StmtKind::Func(f) => func_imports(f, out),
        StmtKind::Block(b) => b.args.iter().for_each(|(_, e)| expr_imports(e, out)),
        StmtKind::Break | StmtKind::Continue | StmtKind::EngineImport(_) | StmtKind::Extern(_) => {}
    }
}

fn func_imports(f: &FuncDecl, out: &mut Vec<(Rc<str>, Span)>) {
    for (_, d) in &f.params {
        d.iter().for_each(|d| expr_imports(d, out));
    }
    match &f.body {
        FuncBody::Block(b) => body_imports(b, out),
        FuncBody::Expr(e) => expr_imports(e, out),
    }
}

fn expr_imports(e: &Expr, out: &mut Vec<(Rc<str>, Span)>) {
    match &e.kind {
        ExprKind::Func(f) => func_imports(f, out),
        ExprKind::FStr(parts) => {
            for p in parts {
                if let FPart::Expr { expr, .. } = p {
                    expr_imports(expr, out);
                }
            }
        }
        ExprKind::List(items) => items.iter().for_each(|x| expr_imports(x, out)),
        ExprKind::Dict(entries) => {
            for (k, v) in entries {
                if let DictKey::Computed(k) = k {
                    expr_imports(k, out);
                }
                expr_imports(v, out);
            }
        }
        ExprKind::Unary(_, _, x) | ExprKind::OptChain(x) => expr_imports(x, out),
        ExprKind::Binary(_, _, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) => {
            expr_imports(a, out);
            expr_imports(b, out);
        }
        ExprKind::Ternary(c, a, b) => {
            expr_imports(c, out);
            expr_imports(a, out);
            expr_imports(b, out);
        }
        ExprKind::Call { callee, args, .. } => {
            expr_imports(callee, out);
            for a in args {
                match a {
                    Arg::Pos(x) | Arg::Named(_, x) | Arg::Spread(x) => expr_imports(x, out),
                }
            }
        }
        ExprKind::Index { obj, index, .. } => {
            expr_imports(obj, out);
            expr_imports(index, out);
        }
        ExprKind::Slice { obj, lo, hi, .. } => {
            expr_imports(obj, out);
            for x in [lo, hi].into_iter().flatten() {
                expr_imports(x, out);
            }
        }
        ExprKind::Member { obj, .. } => expr_imports(obj, out),
        ExprKind::Block(b) => b.args.iter().for_each(|(_, e)| expr_imports(e, out)),
        ExprKind::Null
        | ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bytes(_)
        | ExprKind::Name(..) => {}
    }
}

/// How a path that may not exist prints in messages (2.1): `.` and `..` resolved, and the
/// longest existing part canonicalized, so case and short names match the real folder.
pub fn shown(path: &Path) -> Rc<str> {
    let clean = absolute_clean(path);
    let mut existing = clean.clone();
    let mut rest = Vec::new();
    while canonical(&existing).is_none() {
        match existing.file_name() {
            Some(name) => {
                rest.push(name.to_os_string());
                existing.pop();
            }
            None => return display_path(&clean),
        }
    }
    let mut full = canonical(&existing).unwrap_or(existing);
    for name in rest.iter().rev() {
        full.push(name);
    }
    display_path(&full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spelling_that_differs_only_in_case_gets_a_note() {
        // 6.4 (round 19): the written names at the end against the canonical path's last ones.
        let base = std::env::temp_dir().join(format!("v2case-{}", std::process::id()));
        let note = |spec: &str, found: &str| case_note(&base.join(spec), spec, &base.join(found));
        assert_eq!(
            note("./REAL/Counter.V2", "real/counter.v2").as_deref(),
            Some("note: './REAL/Counter.V2' is spelled './real/counter.v2' on disk: a case-sensitive file system, as Linux's, won't find it")
        );
        // Only the parts written are compared, with the separator written.
        assert_eq!(
            note("../lib/X.v2", "lib/x.v2").as_deref(),
            Some("note: '../lib/X.v2' is spelled '../lib/x.v2' on disk: a case-sensitive file system, as Linux's, won't find it")
        );
        if cfg!(windows) {
            assert_eq!(
                note(r".\Lib\x.v2", "lib/x.v2").as_deref(),
                Some(r"note: '.\Lib\x.v2' is spelled '.\lib\x.v2' on disk: a case-sensitive file system, as Linux's, won't find it")
            );
            // An 8.3 short name is another name, not a case problem.
            assert_eq!(note("./REALDI~1/x.v2", "RealDirectory/x.v2"), None);
        }
        assert_eq!(note("./real/counter.v2", "real/counter.v2"), None);
        assert_eq!(note("./other.v2", "real.v2"), None);
    }

    /// A link to a folder: a junction on Windows (which needs no administrator), else a symlink.
    pub(crate) fn dir_link(link: &Path, target: &Path) {
        #[cfg(windows)]
        {
            let ok = std::process::Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(link).arg(target).stdout(std::process::Stdio::null()).status().unwrap().success();
            assert!(ok, "mklink /J {}", link.display());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn links_that_lead_nowhere_or_in_a_loop_are_found() {
        // DOCS 3.10 (round 19): a folder part of the path, a junction on Windows.
        let d = std::env::temp_dir().join(format!("v2links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("gone")).unwrap();
        std::fs::create_dir_all(d.join("real")).unwrap();
        dir_link(&d.join("broken"), &d.join("gone"));
        std::fs::remove_dir(d.join("gone")).unwrap();
        dir_link(&d.join("loop1"), &d.join("loop2"));
        dir_link(&d.join("loop2"), &d.join("loop1"));
        dir_link(&d.join("fine"), &d.join("real"));
        let broken = link_problem(&d.join("broken").join("lib.v2")).expect("the broken link");
        assert_eq!(broken.at, d.join("broken"));
        assert_eq!(broken.problem("'broken'"), format!("'broken' is a link to {}, which isn't there", d.join("gone").display()));
        let remove = if cfg!(windows) { "rmdir" } else { "rm" };
        let shown = |p: &Path| crate::cli::shell_word(&p.to_string_lossy());
        assert_eq!(broken.fix(), format!("remove the link ({remove} {}), or make {}", shown(&d.join("broken")), d.join("gone").display()));
        let looped = link_problem(&d.join("loop1").join("x").join("y.v2")).expect("the loop");
        assert_eq!((looped.at.clone(), looped.problem("it")), (d.join("loop1"), "it is a link in a loop".to_string()));
        assert!(looped.fix().ends_with("), or point it somewhere else"), "{}", looped.fix());
        // A link that leads somewhere, and a path that simply isn't there, have none.
        assert!(link_problem(&d.join("fine").join("nope.v2")).is_none());
        assert!(link_problem(&d.join("nope").join("x.v2")).is_none());
        assert_eq!(written_prefix("a/broken/x.txt", Path::new("a").join("broken").as_path()), "a/broken");
        let _ = std::fs::remove_dir_all(&d);
    }
}
