//! V2 modules: parsing the import graph at startup and loading files (INTERNALS 6.1, 6.4).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::syntax::ast::*;
use crate::syntax::span::{absolute_clean, display_path, FileId, LoadError, SourceMap, Span};
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

/// The file `import "spec"` in `importer` loads, canonical, or the `ImportError` message (6.4).
/// A drive-relative Windows path (`C:m.v2`) is neither relative to the file nor absolute.
pub fn locate(sm: &SourceMap, importer: FileId, spec: &str) -> Result<PathBuf, String> {
    let p = Path::new(spec);
    if matches!(p.components().next(), Some(std::path::Component::Prefix(_))) && !p.has_root() {
        return Err(format!("cannot find module '{spec}' (a drive-relative path: write it relative to the file, or absolute)"));
    }
    let target = resolve(sm, importer, spec);
    let found = canonical(&target).ok_or_else(|| {
        // `import "./util" as util` for util.v2.
        let with = target.with_file_name(format!("{}.v2", target.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
        let hint = if spec.ends_with(".v2") || !with.is_file() { String::new() } else { format!(" — did you mean '{spec}.v2'?") };
        format!("cannot find module '{spec}' (looked for {}){hint}", shown(&target))
    })?;
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
