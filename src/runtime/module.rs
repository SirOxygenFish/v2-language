//! V2 modules: parsing the import graph at startup and loading files (INTERNALS 6.1, 6.4).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::syntax::ast::*;
use crate::syntax::span::{display_path, FileId, LoadError, SourceMap};
use crate::syntax::{check, lexer, parser, Diag};

/// A static diagnostic of a module file, kept until an `import` of it runs (6.1 step 2).
pub struct ModuleDiag {
    pub display: Rc<str>,
    pub line: u32,
    pub col: u32,
    pub message: String,
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
    }
}

/// Load, lex, parse and check one file. `Err` holds what to print for the entry file;
/// `Parsed::Failed` holds a module's diagnostics.
pub fn parse_file(sm: &mut SourceMap, path: &Path) -> Result<Parsed, LoadError> {
    let file = match sm.load(path) {
        Ok(id) => id,
        Err(LoadError::NotUtf8 { display, line }) => {
            return Ok(Parsed::Failed(vec![ModuleDiag { display, line, col: 1, message: "file is not valid UTF-8".into() }]))
        }
        Err(e) => return Err(e),
    };
    let program = match lexer::lex(sm, file).and_then(|toks| parser::parse_program(sm, toks)) {
        Ok(p) => p,
        Err(d) => return Ok(Parsed::Failed(vec![module_diag(sm, &d)])),
    };
    let diags = check::check(&program);
    if !diags.is_empty() {
        return Ok(Parsed::Failed(diags.iter().map(|d| module_diag(sm, d)).collect()));
    }
    Ok(Parsed::Ok { file, program: Rc::new(program) })
}

/// Where `import "spec"` in `importer` points: relative to the importing file's folder,
/// absolute paths as is; no extension is added (6.4 step 1).
pub fn resolve(sm: &SourceMap, importer: FileId, spec: &str) -> PathBuf {
    let base = sm.file(importer).path.parent().map(Path::to_path_buf).unwrap_or_default();
    base.join(spec)
}

/// `fs::canonicalize` without the `\\?\` prefix, or `None` when the file is missing.
pub fn canonical(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok().map(|p| crate::syntax::span::strip_verbatim(&p))
}

/// 6.1 step 2: every file the `import` literals of `program` name, depth-first in source order.
/// Missing files are skipped (their `import` fails when it runs).
pub fn parse_graph(sm: &mut SourceMap, file: FileId, program: &[Stmt], parsed: &mut HashMap<PathBuf, Parsed>) {
    let mut specs = Vec::new();
    collect_imports(program, &mut specs);
    for spec in specs {
        let target = resolve(sm, file, &spec);
        let Some(path) = canonical(&target) else { continue };
        if parsed.contains_key(&path) || sm.file(file).path == path {
            continue;
        }
        let Ok(result) = parse_file(sm, &path) else { continue };
        let next = match &result {
            Parsed::Ok { file, program } => Some((*file, program.clone())),
            Parsed::Failed(_) => None,
        };
        parsed.insert(path, result);
        if let Some((f, p)) = next {
            parse_graph(sm, f, &p, parsed);
        }
    }
}

/// Every `import "…"` path in a program, in source order, at any depth (6.4: imports may
/// appear in any body, including function and lambda bodies).
fn collect_imports(stmts: &[Stmt], out: &mut Vec<Rc<str>>) {
    for s in stmts {
        stmt_imports(s, out);
    }
}

fn body_imports(b: &Body, out: &mut Vec<Rc<str>>) {
    collect_imports(&b.stmts, out);
}

fn stmt_imports(s: &Stmt, out: &mut Vec<Rc<str>>) {
    match &s.kind {
        StmtKind::Import { path, .. } => out.push(path.clone()),
        StmtKind::Let(_, e) => e.iter().for_each(|e| expr_imports(e, out)),
        StmtKind::Const(_, e) | StmtKind::Expr(e) | StmtKind::Throw(e) => expr_imports(e, out),
        StmtKind::Return(e) => e.iter().for_each(|e| expr_imports(e, out)),
        StmtKind::Assign { target, value, .. } => {
            match target {
                Target::Name(_) => {}
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

fn func_imports(f: &FuncDecl, out: &mut Vec<Rc<str>>) {
    for (_, d) in &f.params {
        d.iter().for_each(|d| expr_imports(d, out));
    }
    match &f.body {
        FuncBody::Block(b) => body_imports(b, out),
        FuncBody::Expr(e) => expr_imports(e, out),
    }
}

fn expr_imports(e: &Expr, out: &mut Vec<Rc<str>>) {
    match &e.kind {
        ExprKind::Lambda(f) => func_imports(f, out),
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
        | ExprKind::Name(_) => {}
    }
}

/// How a path that may not exist prints in messages (2.1): `.` segments dropped.
pub fn shown(path: &Path) -> Rc<str> {
    let clean: PathBuf = path.components().collect();
    display_path(&clean)
}
