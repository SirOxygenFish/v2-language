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
    /// A file's parse stopped at an error, so the rest of it wasn't checked.
    stopped: bool,
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
    fn not_utf8(&mut self, display: &str, line: u32, col: u32, message: &str) {
        self.files += 1;
        self.errors += 1;
        let text = format!("{display}:{line}:{col}: SyntaxError: {message}\n");
        self.problems.push(Problem { met: self.files, lo: 0, text });
    }

    /// An import problem, shown as the `ImportError` the run would raise there (6.4), control
    /// characters escaped (2.1).
    fn import_error(&mut self, message: String, at: Span) {
        self.errors += 1;
        let e = ErrorValue::new("ImportError", crate::syntax::span::shown_path(&message));
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
                self.stopped = true;
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
                Err(LoadError::NotUtf8 { display, line, col, message }) => {
                    self.done.insert(canonical);
                    self.not_utf8(&display, line, col, message);
                }
            }
        }
    }
}

/// py found by the search (7.6) while the activated venv's python links to nothing or is missing,
/// which the search can't see: doctor says so, as a broken `[python] venv` would be (19.2).
fn activated_venv_note(id: &str) -> Option<String> {
    let venv = std::env::var_os("VIRTUAL_ENV").filter(|v| id == "py" && !v.is_empty())?;
    crate::engines::resolve::activated_venv_problem(Path::new(&venv)).map(|problem| format!("py: {problem}"))
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// `v2 check FILE` (19.3): diagnostics to stderr, the summary to stdout; exit 1 on any error.
pub fn check(file: &str) -> i32 {
    if let Some(msg) = crate::cli::entry_error(file, &[], "v2 check") {
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
        Err(LoadError::NotUtf8 { display, line, col, message }) => {
            c.not_utf8(&display, line, col, message);
            display
        }
    };
    // After an interrupt nothing is reported (6.5).
    stop_if_interrupted();
    c.problems.sort_by_key(|p| (p.met, p.lo));
    for p in &c.problems {
        stderr!("{}", p.text);
    }
    let stopped = if c.stopped { " (a file with a syntax error shows only that error: fix it, then check again)" } else { "" };
    stdoutln!("check {display}: {}, {}{stopped}", plural(c.files, "file"), plural(c.errors, "error"));
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
    // The engines whose program answered --version but didn't start.
    let mut no_start: HashSet<String> = HashSet::new();
    for spec in reg.specs() {
        let id = spec.id.to_string();
        let skipped = |list: &[(std::path::PathBuf, String)], notes: &mut Vec<String>| {
            notes.extend(list.iter().map(|(p, why)| format!("{id}: skipped {} ({why})", p.display())));
        };
        match reg.resolve(spec) {
            Ok(e) => {
                let version = e.version.map_or_else(|| "-".to_string(), |v| v.to_string());
                // Read as it starts, after the version probe: one that is wrong stops every block,
                // so with one set the program has to start (importing what the worker imports).
                // So under an address-space limit, which Node.js needs about 1 GB of (8.4).
                let vars = crate::engines::spec::startup_variables(&id);
                let limit = crate::engines::spec::limit_fix(&id);
                let host_failed = spec.session_of.as_ref().is_some_and(|h| no_start.contains(&**h));
                let failure = if (vars.is_empty() && limit.is_none()) || spec.session_of.is_some() { None } else { reg.start_failure(&e) };
                if host_failed || failure.is_some() {
                    rows.push([id.clone(), "broken".into(), e.shown(), version, e.origin.label()]);
                    match (&spec.session_of, failure) {
                        (Some(host), _) => notes.push(format!("{id}: @{id} blocks run on the {host} engine: see {host} above")),
                        (None, why) => {
                            skipped(&e.skipped, &mut notes);
                            notes.push(format!("{id}: {} {}", e.shown(), why.unwrap_or_default()));
                            if !vars.is_empty() {
                                notes.push(format!("{id}: fix: {}", crate::engines::spec::startup_fix(&id, &vars)));
                            }
                            notes.extend(limit.map(|fix| format!("{id}: fix: {fix}")));
                            no_start.insert(id.clone());
                        }
                    }
                    not_ready.push(id);
                    continue;
                }
                rows.push([id.clone(), "ok".into(), e.shown(), version, e.origin.label()]);
                skipped(&e.skipped, &mut notes);
                notes.extend(activated_venv_note(&id).filter(|_| e.origin == crate::engines::resolve::Origin::Path));
                for (name, value) in vars {
                    notes.push(format!("{id}: {name} is set ({value}), which {} reads as it starts", if id == "py" { "Python" } else { "Node.js" }));
                }
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
                    // `env -i v2 doctor`: nothing was searched, so installing wouldn't help.
                    (Status::Missing, _, _) if std::env::var_os("PATH").is_none_or(|p| p.is_empty()) => {
                        let key = spec.session_of.as_deref().unwrap_or(&id);
                        format!("not found: PATH isn't set, so nothing was searched; set it, or name the program in v2.toml, [engines.{key}] command")
                    }
                    // Installed where PATH doesn't look (cron's PATH, /opt): name it instead.
                    (Status::Missing, _, _) => {
                        let key = spec.session_of.as_deref().unwrap_or(&id);
                        format!("not found; install: {}; installed where PATH doesn't look: [engines.{key}] command in v2.toml, or V2_ENGINE_{}", install_hint(&id), key.to_uppercase())
                    }
                    (Status::Old(v), Some(min), None) if v >= min => {
                        format!("not supported: found {v}, which has no type stripping; V2 needs {}; install: {}", rule.needs(min), install_hint(&id))
                    }
                    (Status::Old(v), Some(min), None) => format!("too old: found {v}, V2 needs {}; install: {}", rule.needs(min), install_hint(&id)),
                    _ => nf.message.lines().next().unwrap_or_default().to_string(),
                };
                // The note, then the programs skipped, which its "(see why below)" points at. ts's
                // reasons are js's: when js was found but can't be used, its notes above say what
                // to do (a missing js has the install line for both).
                let host_found = spec.session_of.as_ref().and_then(|h| rows.iter().find(|r| *r[0] == **h)).is_some_and(|r| r[1] != "ok" && r[1] != "missing");
                if let (Some(host), true) = (&spec.session_of, host_found) {
                    notes.push(format!("{id}: @{id} blocks run on the {host} engine: see {host} above"));
                    not_ready.push(id);
                    continue;
                }
                notes.push(format!("{id}: {note}"));
                let others: Vec<(std::path::PathBuf, String)> =
                    nf.skipped.iter().filter(|(p, _)| shown_old.as_deref() != Some(&*p.display().to_string())).cloned().collect();
                skipped(&others, &mut notes);
                notes.extend(activated_venv_note(&id).filter(|_| nf.command.is_none()));
                // A program that can't load a library: the package that has it (7.8). One that
                // stopped at a NODE_OPTIONS its probe reads: that variable. Any other that doesn't
                // run: repair it, or install one, as for a missing engine.
                let fixes: Vec<String> = nf.skipped.iter().filter_map(|(p, why)| crate::engines::resolve::library_fix(p, why)).map(|fix| format!("{id}: fix: {fix}")).collect();
                if fixes.is_empty() && matches!(nf.status, Status::Broken) && nf.command.is_none() {
                    let program = if id == "py" { "Python" } else { "Node.js" };
                    notes.push(match crate::engines::resolve::probe_fix(&id, &nf.skipped, &crate::engines::spec::startup_variables(&id)) {
                        Some(fix) => format!("{id}: fix: {fix}"),
                        None => format!("{id}: fix: repair that {program}, or install one: {}", install_hint(&id)),
                    });
                }
                notes.extend(fixes);
                not_ready.push(id);
            }
        }
    }
    // An engine table in v2.toml for a language this version doesn't run yet (17.1).
    for tag in cfg.custom.keys() {
        notes.push(match crate::engines::registry::later_language(tag) {
            Some(_) => format!("{tag}: [engines.{tag}] in v2.toml configures @{tag}, which isn't in this version of V2 yet"),
            None => format!("{tag}: [engines.{tag}] in v2.toml is a custom engine, and custom engines aren't in this version of V2 yet: @{tag} blocks won't run"),
        });
    }
    // Blocks also need a temp folder (20.3).
    if let Some(problem) = crate::config::rundir::temp_problem() {
        notes.push(format!("temp: {problem}"));
        not_ready.push("blocks".into());
    }
    // A program's path, as every note, is shown with its control characters escaped (2.1): a
    // project folder named with ESC can't drive the terminal.
    for row in rows.iter_mut().skip(1) {
        row[2] = crate::syntax::span::shown_path(&row[2]);
    }
    // COMMAND goes last, so a long path (a venv deep in a project) runs on past the row instead of
    // pushing the other columns apart.
    const ORDER: [usize; 5] = [0, 1, 3, 4, 2];
    let widths: Vec<usize> = (0..5).map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0) + 2).collect();
    for row in &rows {
        let mut line = String::new();
        for (i, &c) in ORDER.iter().enumerate() {
            if i + 1 < ORDER.len() {
                line.push_str(&format!("{:<width$}", row[c], width = widths[c]));
            } else {
                line.push_str(&row[c]);
            }
        }
        stdoutln!("{}", line.trim_end());
    }
    if !notes.is_empty() {
        stdoutln!("notes:");
        for n in &notes {
            stdoutln!("  {}", crate::syntax::span::shown_path(n));
        }
    }
    match &cfg.file {
        Some(f) => stdoutln!("config file: {}", crate::syntax::span::shown_path(&f.display().to_string())),
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
