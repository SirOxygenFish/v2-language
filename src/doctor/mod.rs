//! `v2 doctor` and `v2 check` (INTERNALS 19.2, 19.3).
//!
//! Check is host-only for now: every file the entry imports is parsed and checked, and import
//! problems are reported with their run-time text. The ENGINES, PACKAGES, LIBRARIES and COMPILED
//! sections join it with the engines (roadmap 1.9 and Phase 2). Doctor prints the engine table;
//! `v2 doctor TAG` arrives in roadmap 1.9.

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
    fn not_utf8(&mut self, display: &str, line: u32) {
        self.files += 1;
        self.errors += 1;
        let text = format!("{display}:{line}:1: SyntaxError: file is not valid UTF-8\n");
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
                Err(LoadError::NotUtf8 { display, line }) => {
                    self.done.insert(canonical);
                    self.not_utf8(&display, line);
                }
            }
        }
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// `v2 check FILE` (19.3): diagnostics to stderr, the summary to stdout; exit 1 on any error.
pub fn check(file: &str, cfg: &Config) -> i32 {
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
        Err(LoadError::NotUtf8 { display, line }) => {
            c.not_utf8(&display, line);
            display
        }
    };
    // After an interrupt nothing is reported (6.5).
    stop_if_interrupted();
    c.problems.sort_by_key(|p| (p.met, p.lo));
    for p in &c.problems {
        stderr!("{}", p.text);
    }
    for w in &cfg.warnings {
        stderrln!("{w}");
    }
    stdoutln!(
        "check {display}: {}, {}, {}",
        plural(c.files, "file"),
        plural(c.errors, "error"),
        plural(cfg.warnings.len(), "warning")
    );
    if c.errors > 0 {
        1
    } else {
        0
    }
}

/// `v2 doctor` (19.2): every engine resolved with fresh probes, as a table on stdout, then
/// notes, the config files read and the config warnings. Missing engines are information, not
/// failure: exit 0.
pub fn doctor(cfg: Rc<Config>) -> i32 {
    let reg = Registry::real(cfg.clone(), CacheMode::Refresh);
    let mut rows: Vec<[String; 5]> = vec![["ENGINE", "STATUS", "COMMAND", "VERSION", "FROM"].map(String::from)];
    let mut notes: Vec<String> = Vec::new();
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
            }
            Err(nf) => {
                let (status, version) = match nf.status {
                    Status::Missing => ("missing", "-".to_string()),
                    Status::Old(v) => ("old", v.to_string()),
                    Status::Broken => ("broken", "-".to_string()),
                };
                let dash = || "-".to_string();
                rows.push([id.clone(), status.into(), nf.command.clone().unwrap_or_else(dash), version, nf.from.clone().unwrap_or_else(dash)]);
                skipped(&nf.skipped, &mut notes);
                if nf.status == Status::Missing {
                    notes.push(format!("{id}: not found; install: {}", install_hint(&id)));
                } else {
                    notes.push(format!("{id}: {}", nf.message.lines().next().unwrap_or_default()));
                }
            }
        }
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
    if cfg.files.is_empty() {
        stdoutln!("config files: none");
    } else {
        stdoutln!("config files:");
        for f in &cfg.files {
            stdoutln!("  {}", f.display());
        }
    }
    for w in &cfg.warnings {
        stdoutln!("{w}");
    }
    0
}
