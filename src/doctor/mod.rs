//! `v2 doctor` and `v2 check` (INTERNALS 19.2, 19.3).
//!
//! Check parses and statically checks the entry and every file it imports, reporting import
//! problems with their run-time text; it executes nothing and writes nothing. Doctor prints the
//! engine table.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::config::Config;
use crate::engines::registry::Registry;
use crate::engines::resolve::{CacheMode, Status};
use crate::engines::spec::install_hint;
use crate::engines::process::stop_if_interrupted;
use crate::runtime::error::{render_diag, render_error, ErrorValue};
use crate::runtime::module;
use crate::syntax::span::{FileId, LoadError, SourceMap, Span};
use crate::syntax::{check as static_check, lexer, parser, Diag};

/// A file being checked: its id, its canonical path, and the imports still to follow.
type Frame = (FileId, PathBuf, std::vec::IntoIter<(Rc<str>, Span)>);

/// One problem, rendered per 2.4, with where it sorts: file order (the order the files were
/// first met, 2.1), then source order.
struct Problem {
    met: usize,
    lo: u32,
    text: String,
}

#[derive(Default)]
struct Checker {
    sm: SourceMap,
    problems: Vec<Problem>,
    files: usize,
    errors: usize,
    done: HashSet<PathBuf>,
    /// When each loaded file was met, by `FileId`.
    met: HashMap<u32, usize>,
}

impl Checker {
    fn diag(&mut self, d: &Diag) {
        self.errors += 1;
        let text = render_diag(&self.sm, d);
        self.problems.push(Problem { met: self.met[&d.span.file.0], lo: d.span.lo, text });
    }

    /// A file that is not UTF-8 never gets a `FileId`; its one error sorts at its place.
    fn not_utf8(&mut self, display: &str, line: u32, message: &str) {
        self.files += 1;
        self.errors += 1;
        let text = format!("{display}:{line}:1: SyntaxError: {message}\n");
        self.problems.push(Problem { met: self.files, lo: 0, text });
    }

    /// An import problem, shown as the `ImportError` the run would raise there (6.4).
    fn import_error(&mut self, message: String, at: Span) {
        self.errors += 1;
        let e = ErrorValue::new("ImportError", message);
        let _ = e.span.set(at);
        let text = render_error(&self.sm, &e);
        self.problems.push(Problem { met: self.met[&at.file.0], lo: at.lo, text });
    }

    /// Parse and check one file; its import sites, or `None` when it doesn't parse (a file that
    /// fails to parse is reported and its imports are skipped, 19.3 step 1).
    fn enter(&mut self, id: FileId) -> Option<Vec<(Rc<str>, Span)>> {
        self.files += 1;
        self.met.insert(id.0, self.files);
        let program = match lexer::lex(&self.sm, id).and_then(|toks| parser::parse_program(&self.sm, toks)) {
            Ok(p) => p,
            Err(d) => {
                self.diag(&d);
                return None;
            }
        };
        for d in static_check::check(&program) {
            self.diag(&d);
        }
        Some(module::import_sites(&program))
    }

    /// The entry and every file it imports, depth-first with an explicit stack (an import
    /// chain can be longer than the host stack).
    fn walk(&mut self, entry: FileId) {
        // The files being checked, outermost first, and the same as a set.
        let mut frames: Vec<Frame> = Vec::new();
        let mut open: HashSet<PathBuf> = HashSet::new();
        let path = self.sm.file(entry).path.clone();
        match self.enter(entry) {
            Some(sites) => {
                open.insert(path.clone());
                frames.push((entry, path, sites.into_iter()));
            }
            None => {
                self.done.insert(path);
            }
        }
        while let Some((id, _, sites)) = frames.last_mut() {
            let id = *id;
            let Some((spec, at)) = sites.next() else {
                let (_, path, _) = frames.pop().expect("a frame");
                open.remove(&path);
                self.done.insert(path);
                continue;
            };
            let canonical = match module::locate(&self.sm, id, &spec) {
                Ok(p) => p,
                Err(msg) => {
                    self.import_error(msg, at);
                    continue;
                }
            };
            if open.contains(&canonical) {
                let first = frames.iter().position(|(_, p, _)| *p == canonical).unwrap_or(0);
                let chain: Vec<String> = frames[first..]
                    .iter()
                    .map(|(_, p, _)| p)
                    .chain(std::iter::once(&canonical))
                    .map(|p| module::shown(p).to_string())
                    .collect();
                self.import_error(format!("circular import: {}", chain.join(" -> ")), at);
                continue;
            }
            if self.done.contains(&canonical) {
                continue;
            }
            match self.sm.load(&canonical) {
                Ok(next) => match self.enter(next) {
                    Some(sites) => {
                        open.insert(canonical.clone());
                        frames.push((next, canonical, sites.into_iter()));
                    }
                    None => {
                        self.done.insert(canonical);
                    }
                },
                Err(LoadError::Io(msg)) => {
                    self.done.insert(canonical);
                    self.import_error(format!("cannot import '{spec}': {msg}"), at);
                }
                Err(LoadError::NotUtf8 { display, line, message }) => {
                    self.done.insert(canonical);
                    self.not_utf8(&display, line, message);
                }
            }
        }
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// `v2 check FILE` (19.3): diagnostics to stderr, the summary to stdout; exit 1 on any error.
pub fn check(file: &str) -> i32 {
    if let Some(msg) = crate::cli::entry_error(file, None, "v2 check") {
        stderrln!("v2: {msg}");
        return 2;
    }
    let mut c = Checker::default();
    let display: Rc<str> = match c.sm.load(Path::new(file)) {
        Ok(entry) => {
            c.walk(entry);
            c.sm.file(entry).display.clone()
        }
        Err(LoadError::Io(msg)) => {
            stderrln!("v2: {msg}");
            return 2;
        }
        Err(LoadError::NotUtf8 { display, line, message }) => {
            c.not_utf8(&display, line, message);
            display
        }
    };
    // After an interrupt nothing is reported (6.5).
    stop_if_interrupted();
    c.problems.sort_by_key(|p| (p.met, p.lo));
    for p in &c.problems {
        stderr!("{}", p.text);
    }
    stdoutln!("check {display}: {}, {}", plural(c.files, "file"), plural(c.errors, "error"));
    if c.errors > 0 {
        1
    } else {
        0
    }
}

/// `v2 doctor` (19.2): every engine resolved with fresh probes, as a table on stdout, then
/// notes and the config file read. Missing engines are information, not failure: exit 0.
pub fn doctor(cfg: Rc<Config>) -> i32 {
    let reg = Registry::real(cfg.clone(), CacheMode::Refresh);
    let mut rows: Vec<[String; 5]> = vec![["ENGINE", "STATUS", "COMMAND", "VERSION", "FROM"].map(String::from)];
    let mut notes: Vec<String> = Vec::new();
    let (mut ready, mut not_ready) = (Vec::new(), Vec::new());
    for spec in reg.specs() {
        let id = spec.id.to_string();
        let skipped = |list: &[(std::path::PathBuf, String)], notes: &mut Vec<String>| {
            notes.extend(list.iter().map(|(p, why)| format!("{id}: skipped {} ({why})", p.display())));
        };
        match reg.resolve(spec) {
            Ok(e) => {
                let version = e.version.map_or_else(|| "-".to_string(), |v| v.to_string());
                rows.push([id.clone(), "ok".into(), e.shown(), version, e.origin.label()]);
                skipped(&e.skipped, &mut notes);
                ready.push(id);
            }
            Err(nf) => {
                // ts is held to js's minimum (7.6).
                let rule = spec.session_of.as_ref().and_then(|host| reg.specs().iter().find(|s| &s.id == host)).unwrap_or(spec);
                let (status, version) = match nf.status {
                    Status::Missing => ("missing", "-".to_string()),
                    // Node 23.0 and 23.1: newer than the minimum, without type stripping (7.6).
                    Status::Old(v) if rule.min_version.is_some_and(|min| v >= min) => ("unsupported", v.to_string()),
                    Status::Old(v) => ("old", v.to_string()),
                    Status::Broken => ("broken", "-".to_string()),
                };
                let dash = || "-".to_string();
                // Found on PATH but not usable: the newest too-old one's place, or the first that
                // doesn't run; ts shows what js found (19.2).
                let found = match (nf.status, &nf.command) {
                    (Status::Old(v), None) => nf
                        .skipped
                        .iter()
                        .find(|(_, why)| why.starts_with(&format!("too old: {v} ")) || why.starts_with(&format!("not supported: {v} ")))
                        .map(|(p, _)| p.display().to_string()),
                    // A Store placeholder isn't a program that doesn't run (7.8).
                    (Status::Broken, None) => nf.skipped.iter().find(|(_, why)| why != "Microsoft Store placeholder").map(|(p, _)| p.display().to_string()),
                    _ => None,
                };
                let host = spec.session_of.as_ref().and_then(|h| rows.iter().find(|r| *r[0] == **h)).filter(|r| r[1] != "missing").map(|r| (r[2].clone(), r[4].clone()));
                // The row and the note already say why the too-old one shown can't be used.
                let shown_old = found.clone().filter(|_| matches!(nf.status, Status::Old(_)));
                let (command, from) = match (found, host) {
                    (Some(path), _) => (path, "PATH".to_string()),
                    (None, Some(js)) if nf.command.is_none() => js,
                    _ => (nf.command.clone().unwrap_or_else(dash), nf.from.clone().unwrap_or_else(dash)),
                };
                rows.push([id.clone(), status.into(), command, version, from]);
                let note = match (nf.status, rule.min_version, &nf.command) {
                    (Status::Missing, _, _) => format!("not found; install: {}", install_hint(&id)),
                    (Status::Old(v), Some(min), None) if v >= min => {
                        format!("not supported: found {v}, which has no type stripping; V2 needs {}; install: {}", rule.needs(min), install_hint(&id))
                    }
                    (Status::Old(v), Some(min), None) => format!("too old: found {v}, V2 needs {}; install: {}", rule.needs(min), install_hint(&id)),
                    _ => nf.message.lines().next().unwrap_or_default().to_string(),
                };
                // The note, then the programs skipped, which its "(see why below)" points at; ts's
                // reasons are js's, listed above it.
                let note = match (&spec.session_of, nf.skipped.is_empty()) {
                    (Some(host), true) => note.replace(" (see why below)", &format!(" (see {host} above)")),
                    _ => note,
                };
                notes.push(format!("{id}: {note}"));
                let others: Vec<(std::path::PathBuf, String)> =
                    nf.skipped.iter().filter(|(p, _)| shown_old.as_deref() != Some(&*p.display().to_string())).cloned().collect();
                skipped(&others, &mut notes);
                // A program that can't load a library: the package that has it (7.8).
                notes.extend(nf.skipped.iter().filter_map(|(p, why)| crate::engines::resolve::library_fix(p, why)).map(|fix| format!("{id}: fix: {fix}")));
                not_ready.push(id);
            }
        }
    }
    // Blocks also need a temp folder (20.3).
    if let Some(problem) = crate::config::rundir::temp_problem() {
        notes.push(format!("temp: {problem}"));
        not_ready.push("blocks".into());
    }
    let widths: Vec<usize> = (0..5).map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0) + 2).collect();
    for row in &rows {
        let mut line = String::new();
        for (c, cell) in row.iter().enumerate() {
            if c + 1 < row.len() {
                line.push_str(&format!("{cell:<width$}", width = widths[c]));
            } else {
                line.push_str(cell);
            }
        }
        stdoutln!("{}", line.trim_end());
    }
    if !notes.is_empty() {
        stdoutln!("notes:");
        for n in &notes {
            stdoutln!("  {n}");
        }
    }
    match &cfg.file {
        Some(f) => stdoutln!("config file: {}", f.display()),
        None => stdoutln!("config file: none"),
    }
    // What a bug report needs, then what the table means for the user's scripts.
    stdoutln!("{}, {} {}", crate::cli::version_line(), std::env::consts::OS, std::env::consts::ARCH);
    let list = |ids: &[String]| if ids.is_empty() { "none".to_string() } else { ids.join(", ") };
    stdoutln!("ready: {}", list(&ready));
    if !not_ready.is_empty() {
        let still = if ready.is_empty() { "plain V2 scripts still run" } else { "scripts that don't use them still run" };
        stdoutln!("not ready: {} (see the notes); {still}", list(&not_ready));
    }
    0
}
