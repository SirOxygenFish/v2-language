//! Configuration: the project root, its `v2.toml` and `V2_ENGINE_<TAG>` (INTERNALS ch. 18).
//!
//! Layers, lowest first: built-in defaults, the project's `v2.toml`, `V2_ENGINE_<TAG>`. The file
//! is checked in its own order; an unknown section or key, a key that does not apply and a bad
//! value are configuration errors (18.5). A `V2_ENGINE_<TAG>` replaces the file's `command`, which
//! is then not validated (18.3). A custom engine's table is kept as written and is validated by
//! `engines/spec` (17.1) when custom engines arrive (roadmap Phase 3).

#![allow(dead_code)] // most settings are read by the engines (roadmap Phase 1)

pub mod rundir;

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use indexmap::IndexMap;

use crate::runtime::error::did_you_mean;
use crate::runtime::value::float_str;
use crate::syntax::scan::canonical_tag;
use crate::syntax::span::absolute_clean;

/// Where a value came from: doctor's FROM column (18.3).
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigSource {
    /// The project's `v2.toml`.
    Project(PathBuf),
    /// A `V2_ENGINE_<TAG>` variable, by name.
    Env(String),
}

#[derive(Clone, Debug)]
pub struct Sourced<T> {
    pub value: T,
    pub from: ConfigSource,
}

/// Settings of a built-in engine (18.2).
#[derive(Clone, Debug, Default)]
pub struct EngineCfg {
    pub command: Option<Sourced<Vec<String>>>,
    pub flags: Option<Sourced<Vec<String>>>,
    pub startup_timeout: Option<Sourced<f64>>,
    pub call_timeout: Option<Sourced<f64>>,
}

#[derive(Debug, Default)]
pub struct Config {
    /// The project root: the folder of the nearest `v2.toml`, else the start folder (18.1).
    pub root: PathBuf,
    /// The project's `v2.toml`, when there is one.
    pub file: Option<PathBuf>,
    /// Built-in engines by canonical tag.
    pub engines: IndexMap<String, EngineCfg>,
    /// Custom engines' tables as written, by canonical tag; their relative paths resolve against
    /// `root` (17.1).
    pub custom: IndexMap<String, toml::Table>,
    pub python_venv: Option<Sourced<PathBuf>>,
}

/// A configuration error: printed to stderr, exit 2 (18.5).
#[derive(Debug, PartialEq)]
pub struct ConfigError(pub String);

pub const BUILTIN_TAGS: &[&str] = &["py", "js", "ts", "c", "cpp", "rust"];

const SECTIONS: &[&str] = &["engines", "python"];
const ENGINE_KEYS: &[&str] = &["command", "flags", "startup_timeout", "call_timeout"];
const PYTHON_KEYS: &[&str] = &["venv"];

/// The variable `~` is read from (18.1).
const HOME_VAR: &str = if cfg!(windows) { "USERPROFILE" } else { "HOME" };

/// `~`: `%USERPROFILE%` on Windows, `$HOME` elsewhere, from exactly these variables (18.1).
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os(HOME_VAR).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The nearest folder at or above `start` holding a regular file `v2.toml` (18.1).
pub fn find_root(start: &Path) -> Option<PathBuf> {
    let start = absolute_clean(start);
    start.ancestors().find(|d| d.join("v2.toml").is_file()).map(Path::to_path_buf)
}

/// Load the configuration for a run that starts at `start` (the entry file's folder, or the
/// cwd for `v2 doctor`).
pub fn load(start: &Path) -> Result<Config, ConfigError> {
    load_from(start, std::env::vars_os())
}

/// A `V2_ENGINE_<TAG>` override: the executable and the variable's name.
type Overrides = IndexMap<String, (PathBuf, String)>;

/// `load` with the environment passed in.
fn load_from(start: &Path, env: impl Iterator<Item = (OsString, OsString)>) -> Result<Config, ConfigError> {
    let start = absolute_clean(start);
    let root = find_root(&start).unwrap_or(start);
    let overrides = engine_overrides(env)?;
    let mut cfg = Config { root, ..Config::default() };
    let file = cfg.root.join("v2.toml");
    if file.is_file() {
        read_file(&mut cfg, &file, &overrides)?;
        cfg.file = Some(file);
    }
    // A built-in engine gets `command = [PATH]`; a custom one `command[0] = PATH`. A variable for
    // a tag nobody declares is ignored: environments hold unrelated variables (18.4).
    for (tag, (path, var)) in overrides {
        let path_text = path.to_string_lossy().into_owned();
        if BUILTIN_TAGS.contains(&tag.as_str()) {
            cfg.engines.entry(tag).or_default().command = Some(Sourced { value: vec![path_text], from: ConfigSource::Env(var) });
        } else if let Some(table) = cfg.custom.get_mut(&tag) {
            match table.get_mut("command") {
                Some(toml::Value::Array(items)) if !items.is_empty() => items[0] = toml::Value::String(path_text),
                _ => {
                    table.insert("command".into(), toml::Value::Array(vec![toml::Value::String(path_text)]));
                }
            }
        }
    }
    Ok(cfg)
}

/// `V2_ENGINE_<TAG>` (18.4): one executable path each, never split; empty means unset. The prefix
/// is case-insensitive on Windows; the tag must be canonical.
fn engine_overrides(env: impl Iterator<Item = (OsString, OsString)>) -> Result<Overrides, ConfigError> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let prefix_len = "V2_ENGINE_".len();
    let mut found = Overrides::new();
    for (k, v) in env {
        let k = k.to_string_lossy().into_owned();
        // Compare bytes: a name may have a multibyte char across the prefix's end.
        let head = k.as_bytes().get(..prefix_len);
        let has_prefix = k.len() > prefix_len
            && if cfg!(windows) {
                head.is_some_and(|h| h.eq_ignore_ascii_case(b"V2_ENGINE_"))
            } else {
                head == Some(&b"V2_ENGINE_"[..])
            };
        if !has_prefix || v.is_empty() || !k.is_char_boundary(prefix_len) {
            continue;
        }
        // Canonical tags only (`V2_ENGINE_PY`, not `V2_ENGINE_PYTHON`): an alias is ignored like
        // any other unknown tag. Off Windows the tag is upper case, as the prefix is, so no two
        // variables can name one engine (18.4).
        let rest = &k[prefix_len..];
        if !cfg!(windows) && rest.bytes().any(|b| b.is_ascii_lowercase()) {
            continue;
        }
        let tag = rest.to_lowercase();
        if tag == "ts" {
            return Err(ConfigError(format!("config error: {k}: ts runs on the js engine; set V2_ENGINE_JS")));
        }
        if canonical_tag(&tag) != tag {
            continue;
        }
        // cmd's `set V2_ENGINE_PY="C:\Program Files\…\python.exe"` keeps the quotes: one pair
        // around the whole value is taken off; `set V2_ENGINE_PY=C:\…\python.exe && v2 x.v2`
        // keeps the space before `&&`: spaces around it are too (18.4).
        let v = v.to_string_lossy();
        found.insert(tag, (from_cwd(&cwd, Path::new(unquoted(v.trim()))), k));
    }
    Ok(found)
}

/// A path without one pair of double quotes around it: cmd's `set X="C:\…"` keeps them, and so
/// does Explorer's "Copy as path" pasted into a TOML string (18.1, 18.4).
fn unquoted(s: &str) -> &str {
    s.strip_prefix('"').and_then(|x| x.strip_suffix('"')).filter(|x| !x.is_empty()).unwrap_or(s)
}

/// An executable named in `V2_ENGINE_*`: with a separator it is a path against the cwd; a bare
/// name is left for the PATH walk (18.1, 18.4).
fn from_cwd(cwd: &Path, v: &Path) -> PathBuf {
    let text = v.to_string_lossy();
    if text.contains(['/', '\\']) || v.is_absolute() {
        cwd.join(v)
    } else {
        v.to_path_buf()
    }
}

/// `~`, `~/x` or `~\x` as a path in the home folder (18.1), as a shell would read it; `None` for
/// anything else or when there is no home folder.
fn home_relative(text: &str) -> Option<PathBuf> {
    let rest = text.strip_prefix('~')?;
    let rest = match rest.chars().next() {
        None => "",
        Some('/' | '\\') => &rest[1..],
        Some(_) => return None,
    };
    Some(home_dir()?.join(rest))
}

/// A `command` written as one string, as the array it means (18.2): the string's words, except
/// that a program path with spaces stays one element: in quotes (`"C:\Program Files\…\python.exe"
/// -X utf8`, without them), the shortest run of words that is a path to an existing file (`exists`),
/// a path with spaces up to `.exe`, `.cmd` or `.bat`, or a path and the words that continue it.
fn command_words(s: &str, exists: impl Fn(&str) -> bool) -> Vec<String> {
    let s = s.trim();
    let split = |program: &str, rest: &str| std::iter::once(program.to_string()).chain(rest.split_whitespace().map(str::to_string)).collect();
    for q in ['"', '\''] {
        if let Some(end) = s.strip_prefix(q).and_then(|inner| inner.find(q)) {
            return split(&s[1..=end], &s[end + 2..]);
        }
    }
    // Where each word ends, so a candidate program keeps the spaces as written.
    let ends: Vec<usize> = s
        .char_indices()
        .filter(|&(i, c)| !c.is_whitespace() && s[i + c.len_utf8()..].chars().next().is_none_or(char::is_whitespace))
        .map(|(i, c)| i + c.len_utf8())
        .collect();
    let path = |program: &str| program.contains(['/', '\\']);
    if let Some(&end) = ends.iter().find(|&&end| path(&s[..end]) && exists(&s[..end])) {
        return split(&s[..end], &s[end..]);
    }
    let windows_program = |program: &str| {
        let lower = program.to_ascii_lowercase();
        program.contains(char::is_whitespace) && path(program) && [".exe", ".cmd", ".bat"].iter().any(|e| lower.ends_with(e))
    };
    if let Some(&end) = ends.iter().find(|&&end| windows_program(&s[..end])) {
        return split(&s[..end], &s[end..]);
    }
    // A path that goes on after a space (`/opt/my tools/python3`): the next words that continue
    // it, with a separator but starting as no item does (`-`, `{`, `/`, `.`, `~`, a drive).
    let words: Vec<&str> = s.split_whitespace().collect();
    let continues = |w: &str| path(w) && !w.starts_with(['-', '{', '/', '\\', '.', '~']) && w.get(1..2).is_none_or(|c| c != ":");
    if words.first().is_some_and(|w| path(w)) {
        let n = 1 + words[1..].iter().take_while(|w| continues(w)).count();
        return std::iter::once(words[..n].join(" ")).chain(words[n..].iter().map(|w| w.to_string())).collect();
    }
    words.into_iter().map(str::to_string).collect()
}

/// A word as a TOML string the user can paste (18.2, 18.5): in double quotes, in literal quotes
/// when it has a backslash or a double quote, or escaped when it also has a single quote.
pub(crate) fn toml_string(w: &str) -> String {
    if !w.contains(['\\', '"']) {
        format!("\"{w}\"")
    } else if !w.contains('\'') {
        format!("'{w}'")
    } else {
        format!("\"{}\"", w.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// The first variable in `text` that a shell would have expanded and V2 takes as written
/// (18.1, 5.10): `%ProgramFiles%`, `${HOME}`, `$env:USERPROFILE`, or `$HOME` in capitals.
pub(crate) fn unexpanded(text: &str) -> Option<&str> {
    let b = text.as_bytes();
    let starts = |i: usize| b.get(i).is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_');
    let end = |mut i: usize, also: &[u8]| {
        while b.get(i).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || also.contains(c)) {
            i += 1;
        }
        i
    };
    for i in 0..b.len() {
        match b[i] {
            b'%' if starts(i + 1) => {
                let e = end(i + 1, b"()");
                if b.get(e) == Some(&b'%') {
                    return Some(&text[i..=e]);
                }
            }
            b'$' if text[i + 1..].starts_with("env:") && starts(i + 5) => return Some(&text[i..end(i + 5, b"")]),
            b'$' if b.get(i + 1) == Some(&b'{') && starts(i + 2) => {
                let e = end(i + 2, b"");
                if b.get(e) == Some(&b'}') {
                    return Some(&text[i..=e]);
                }
            }
            b'$' if starts(i + 1) => {
                let e = end(i + 1, b"");
                if text[i + 1..e].bytes().all(|c| !c.is_ascii_lowercase()) {
                    return Some(&text[i..e]);
                }
            }
            _ => {}
        }
    }
    None
}

/// How a TOML value is named in messages: `string "30s"`, `integer 5`, `array`.
fn describe(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => format!("string {s:?}"),
        toml::Value::Integer(n) => format!("integer {n}"),
        toml::Value::Float(x) => format!("float {}", float_str(*x)),
        toml::Value::Boolean(b) => format!("boolean {b}"),
        toml::Value::Datetime(d) => format!("datetime {d}"),
        toml::Value::Array(_) => "array".into(),
        toml::Value::Table(_) => "table".into(),
    }
}

/// `unknown key [python] venvv (did you mean 'venv'?)`.
fn unknown(place: &str, key: &str, known: &[&str]) -> String {
    match did_you_mean(key, known.iter().copied()) {
        Some(s) => format!("unknown {place} (did you mean '{s}'?)"),
        None => format!("unknown {place}"),
    }
}

/// The file being read: how messages name it, and the folder its relative paths resolve
/// against (18.1).
struct File {
    shown: String,
    base: PathBuf,
    from: ConfigSource,
}

impl File {
    fn error(&self, text: String) -> ConfigError {
        ConfigError(format!("{}: config error: {text}", self.shown))
    }

    /// `[engines.py] startup_timeout: expected a non-negative number, got string "30s"`.
    fn expected<T>(&self, what: &str, expected: &str, v: &toml::Value) -> Result<T, ConfigError> {
        Err(self.error(format!("{what}: expected {expected}, got {}", describe(v))))
    }

    fn sourced<T>(&self, value: T) -> Sourced<T> {
        Sourced { value, from: self.from.clone() }
    }

    fn string_of(&self, what: &str, v: &toml::Value) -> Result<String, ConfigError> {
        match v {
            toml::Value::String(s) => Ok(s.clone()),
            other => self.expected(what, "a string", other),
        }
    }

    /// A Windows drive-relative path (`C:venv`) would resolve against that drive's cwd, so it
    /// is an error (18.1).
    fn not_drive_relative(&self, what: &str, text: &str) -> Result<(), ConfigError> {
        let p = Path::new(text);
        if matches!(p.components().next(), Some(Component::Prefix(_))) && !p.has_root() {
            return Err(self.error(format!("{what}: '{text}' is a drive-relative path: write it relative to the file, or absolute")));
        }
        Ok(())
    }

    /// A path: a leading `~/` is the home folder; another relative one resolves against the
    /// file's folder (18.1).
    fn path(&self, what: &str, v: &toml::Value) -> Result<PathBuf, ConfigError> {
        let text = unquoted(&self.string_of(what, v)?).to_string();
        if let Some(home) = home_relative(&text) {
            return Ok(absolute_clean(&home));
        }
        self.not_drive_relative(what, &text)?;
        // `../venvs/.venv` shown and used without the `..` (doctor prints it).
        Ok(absolute_clean(&self.base.join(text)))
    }

    /// An array of strings. For a `command`, a string gets the array to write instead (18.2):
    /// `[engines.lua] command must be an array: write ["lua", "{file}"]`.
    fn strings(&self, what: &str, v: &toml::Value, command: bool, non_empty: bool) -> Result<Vec<String>, ConfigError> {
        let list: Vec<String> = match v {
            toml::Value::String(s) if command && !s.trim().is_empty() => {
                // A program path is looked up as 18.1 will: in the home folder, or from this file.
                let exists = |p: &str| home_relative(p).unwrap_or_else(|| self.base.join(p)).is_file();
                let words: Vec<String> = command_words(s, exists).iter().map(|w| toml_string(w)).collect();
                return Err(self.error(format!("{what} must be an array: write [{}]", words.join(", "))));
            }
            toml::Value::Array(items) => items.iter().map(|i| self.string_of(what, i)).collect::<Result<_, _>>()?,
            other => return self.expected(what, "an array of strings", other),
        };
        if non_empty && (list.is_empty() || list.iter().any(String::is_empty)) {
            return Err(self.error(format!("{what}: expected a non-empty array of non-empty strings")));
        }
        Ok(list)
    }

    fn seconds(&self, what: &str, v: &toml::Value) -> Result<f64, ConfigError> {
        let x = match v {
            toml::Value::Integer(n) => *n as f64,
            toml::Value::Float(x) => *x,
            other => return self.expected(what, "a non-negative number", other),
        };
        if !x.is_finite() || x < 0.0 {
            return self.expected(what, "a non-negative number", v);
        }
        Ok(x)
    }

    /// 18.1: `argv[0]` with a separator is joined to the file's folder (without one it is left
    /// for the PATH walk). A later element is joined only when it starts with `./` or `../`, or
    /// when it names something that exists in that folder (`adapters/x.rb`), so switches, inline
    /// scripts (`cd src/x && make`) and `{file}` patterns are left alone.
    fn argv(&self, what: &str, mut argv: Vec<String>) -> Result<Vec<String>, ConfigError> {
        for (i, a) in argv.iter_mut().enumerate() {
            if i == 0 {
                *a = unquoted(a).to_string();
            }
            if let Some(home) = home_relative(a).filter(|_| i == 0) {
                *a = home.to_string_lossy().into_owned();
                continue;
            }
            let p = Path::new(a.as_str());
            let path_like = a.contains(['/', '\\']);
            let candidate = !a.starts_with('-') && !a.contains('{') && !p.has_root();
            if i == 0 || (path_like && candidate) {
                self.not_drive_relative(what, a)?;
            }
            let dotted = ["./", "../", ".\\", "..\\"].iter().any(|d| a.starts_with(d));
            let join = if i == 0 {
                path_like && p.is_relative()
            } else {
                candidate && p.is_relative() && (dotted || (!a.is_empty() && self.base.join(p).exists()))
            };
            if join {
                *a = self.base.join(a.as_str()).to_string_lossy().into_owned();
            }
        }
        Ok(argv)
    }
}

/// The project's `v2.toml`, in the file's order: its structure, known sections and keys, keys
/// that apply, and each value's type (18.2, 18.5). A built-in `command` that `V2_ENGINE_<TAG>`
/// replaces is not validated (18.3).
fn read_file(cfg: &mut Config, path: &Path, overrides: &Overrides) -> Result<(), ConfigError> {
    let shown = path.display().to_string();
    let bytes = std::fs::read(path).map_err(|e| ConfigError(format!("{shown}: config error: cannot read the file: {}", crate::syntax::span::os_message(&e))))?;
    let text = String::from_utf8(bytes).map_err(|e| {
        let why = crate::syntax::span::not_utf8_message(e.as_bytes()).trim_start_matches("file is not valid UTF-8");
        ConfigError(format!("{shown}: config error: the file is not valid UTF-8{why}"))
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        let mut message = e.message().trim().to_string();
        let line_text = e.span().map(|span| {
            let start = text[..span.start.min(text.len())].rfind('\n').map_or(0, |i| i + 1);
            text[start..].lines().next().unwrap_or_default()
        });
        // A Windows path in "…" is read with backslash escapes (18.5).
        if (message.contains("escape") || message.contains("unicode")) && line_text.is_some_and(|l| l.contains('\\')) {
            message.push_str("\n  hint: in \"…\", a backslash starts an escape: write Windows paths with / or in single quotes, as in venv = 'C:\\proj\\.venv'");
        } else if let Some((key, value)) = line_text.and_then(|l| l.split_once('=')).map(|(k, v)| (k.trim(), v.trim())) {
            // `venv = .venv`: text needs quotes (single ones keep a Windows path's backslashes);
            // a `#` comment after it stays outside them.
            let value = value.split_once('#').map_or(value, |(v, _)| v).trim_end();
            let bare = !key.is_empty()
                && !value.is_empty()
                && !value.starts_with(['"', '\'', '[', '{', '-', '+', '='])
                && !value.starts_with(|c: char| c.is_ascii_digit())
                && !matches!(value, "true" | "false");
            if bare {
                message.push_str(&format!("\n  hint: text goes in quotes: {key} = {}", toml_string(value)));
            }
        }
        match e.span() {
            Some(span) => {
                let before = &text[..span.start.min(text.len())];
                let line = before.matches('\n').count() + 1;
                let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
                ConfigError(format!("{shown}:{line}:{col}: config error: {message}"))
            }
            None => ConfigError(format!("{shown}: config error: {message}")),
        }
    })?;
    let f = File { shown, base: cfg.root.clone(), from: ConfigSource::Project(path.to_path_buf()) };
    for (section, value) in &table {
        let body = match value {
            toml::Value::Table(body) => body,
            v if SECTIONS.contains(&section.as_str()) => return Err(f.error(format!("[{section}]: expected a table, got {}", describe(v)))),
            // A top-level key that is not a section is an unknown key (18.5).
            _ => return Err(f.error(unknown(&format!("key {section}"), section, SECTIONS))),
        };
        match section.as_str() {
            "python" => {
                for (k, v) in body {
                    if k != "venv" {
                        return Err(f.error(unknown(&format!("key [python] {k}"), k, PYTHON_KEYS)));
                    }
                    cfg.python_venv = Some(f.sourced(f.path("[python] venv", v)?));
                }
            }
            "engines" => read_engines(cfg, &f, body, overrides)?,
            other => return Err(f.error(unknown(&format!("section [{other}]"), other, SECTIONS))),
        }
    }
    // Both choose the Python, and the command would win without a word (18.3).
    if cfg.python_venv.is_some() && cfg.engines.get("py").is_some_and(|e| e.command.is_some()) {
        return Err(f.error("[python] venv and [engines.py] command both choose the Python: keep one (venv for a virtual environment, command for any other Python)".into()));
    }
    Ok(())
}

/// `[engines.<tag>]` tables: names canonicalized, built-in keys checked and typed, custom tables
/// kept as written (18.2).
fn read_engines(cfg: &mut Config, f: &File, body: &toml::Table, overrides: &Overrides) -> Result<(), ConfigError> {
    let mut claimed: IndexMap<String, String> = IndexMap::new();
    for (name, v) in body {
        let tag = canonical_tag(name).to_string();
        if let Some(first) = claimed.get(&tag) {
            return Err(f.error(format!("[engines.{first}] and [engines.{name}] name the same engine")));
        }
        claimed.insert(tag.clone(), name.clone());
        let toml::Value::Table(t) = v else {
            return Err(f.error(format!("[engines.{name}]: expected a table, got {}", describe(v))));
        };
        if tag == "ts" {
            return Err(f.error(format!("[engines.{name}]: ts runs on the js engine; configure [engines.js]")));
        }
        // `[engines.node]`, `[engines.python3]`: a program's name where the engine's goes (18.2).
        let engine = match tag.as_str() {
            "node" | "nodejs" => Some("js"),
            "python3" | "py3" | "pypy" | "pypy3" | "cpython" => Some("py"),
            "gcc" | "clang" | "cc" | "cl" => Some("c"),
            "rustc" => Some("rust"),
            _ => None,
        };
        if let Some(engine) = engine {
            return Err(f.error(format!("[engines.{name}] names a program, not an engine: it runs the {engine} engine, so write [engines.{engine}] command = [\"{name}\"]")));
        }
        // A misspelled engine (`[engines.pyy]`, `[engines.Python]`) would configure nothing, with
        // no word (18.2); a later engine's name (`ruby`) isn't a misspelling.
        if !BUILTIN_TAGS.contains(&tag.as_str()) && crate::engines::registry::later_language(&tag).is_none() {
            let names = ["py", "python", "js", "javascript", "c", "cpp", "cxx", "rust", "rs"];
            if let Some(near) = crate::runtime::error::did_you_mean(name, names) {
                return Err(f.error(format!("[engines.{name}] isn't an engine of this version: did you mean [engines.{near}]? (custom engines aren't in this version of V2 yet)")));
            }
        }
        if !BUILTIN_TAGS.contains(&tag.as_str()) {
            // A custom engine (17.1, roadmap Phase 3).
            cfg.custom.insert(tag, t.clone());
            continue;
        }
        let mut e = EngineCfg::default();
        for (k, kv) in t {
            let what = format!("[engines.{name}] {k}");
            match k.as_str() {
                "kind" => return Err(f.error(format!("built-in engine '{tag}' can't change 'kind'"))),
                "command" if overrides.contains_key(&tag) => {}
                "command" => e.command = Some(f.sourced(f.argv(&what, f.strings(&what, kv, true, true)?)?)),
                "flags" => {
                    if !matches!(tag.as_str(), "c" | "cpp" | "rust") {
                        return Err(f.error(format!("{what} only applies to c, cpp and rust")));
                    }
                    e.flags = Some(f.sourced(f.strings(&what, kv, false, false)?));
                }
                "startup_timeout" => {
                    if !matches!(tag.as_str(), "py" | "js") {
                        return Err(f.error(format!("{what} only applies to py and js")));
                    }
                    e.startup_timeout = Some(f.sourced(f.seconds(&what, kv)?));
                }
                "call_timeout" => e.call_timeout = Some(f.sourced(f.seconds(&what, kv)?)),
                other => return Err(f.error(unknown(&format!("key [engines.{name}] {other}"), other, ENGINE_KEYS))),
            }
        }
        cfg.engines.insert(tag, e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the system temp dir.
    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("v2cfg-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn vars(pairs: &[(&str, &str)]) -> std::vec::IntoIter<(OsString, OsString)> {
        pairs.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect::<Vec<_>>().into_iter()
    }

    /// A project holding `v2.toml`, loaded from its `src` folder with these variables.
    fn project_env(name: &str, toml_text: &str, env: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let d = temp(name);
        std::fs::write(d.join("v2.toml"), toml_text).unwrap();
        let sub = d.join("src");
        std::fs::create_dir_all(&sub).unwrap();
        load_from(&sub, vars(env))
    }

    fn project(name: &str, toml_text: &str) -> Result<Config, ConfigError> {
        project_env(name, toml_text, &[])
    }

    #[test]
    fn variables_a_shell_would_expand_are_found() {
        // 18.1, 5.10: what V2 takes as written, for the hints that say so.
        assert_eq!(unexpanded(r"C:\proj\%ProgramFiles%\Python311\python.exe"), Some("%ProgramFiles%"));
        assert_eq!(unexpanded(r"%ProgramFiles(x86)%\x"), Some("%ProgramFiles(x86)%"));
        assert_eq!(unexpanded("$HOME/.venv"), Some("$HOME"));
        assert_eq!(unexpanded("${HOME}/.venv"), Some("${HOME}"));
        assert_eq!(unexpanded(r"$env:USERPROFILE\x"), Some("$env:USERPROFILE"));
        // Not variables: a share, a folder that starts with $, a lone %.
        for plain in [r"\\server\C$\x", r"C:\$Recycle.Bin", "50% off", "100%", "a%%b", "~/x"] {
            assert_eq!(unexpanded(plain), None, "{plain}");
        }
    }

    #[test]
    fn finds_the_root_and_reads_values() {
        let cfg = project("ok", "[engines.python]\ncommand = [\"py.exe\"]\ncall_timeout = 5\n[engines.c]\nflags = [\"-O2\"]\n[engines.ruby]\nkind = \"run\"\ncommand = [\"ruby\", \"{file}\"]\n").unwrap();
        assert!(cfg.root.join("v2.toml").is_file());
        assert_eq!(cfg.file, Some(cfg.root.join("v2.toml")));
        let venv = project("venv", "[python]\nvenv = \".venv\"\n").unwrap();
        assert_eq!(venv.python_venv.as_ref().unwrap().value, venv.root.join(".venv"));
        assert_eq!(cfg.engines["py"].command.as_ref().unwrap().value, vec!["py.exe"]);
        assert_eq!(cfg.engines["py"].call_timeout.as_ref().unwrap().value, 5.0);
        assert_eq!(cfg.engines["c"].flags.as_ref().unwrap().value, vec!["-O2"]);
        assert!(cfg.custom.contains_key("ruby"));
        // An empty v2.toml stops the upward search and sets nothing.
        let d = temp("nofile").join("deeper");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.parent().unwrap().join("v2.toml"), "").unwrap();
        let cfg = load_from(&d, vars(&[])).unwrap();
        assert_eq!((cfg.root.as_path(), cfg.engines.len()), (d.parent().unwrap(), 0));
    }

    #[test]
    fn errors_name_the_file_and_the_problem() {
        let e = project("syntax", "[engines.py\ncommand = 1\n").unwrap_err().0;
        assert!(e.contains("v2.toml:1:") && e.contains(": config error: "), "{e}");
        let e = project("type", "[engines.py]\nstartup_timeout = \"30s\"\n").unwrap_err().0;
        assert!(e.ends_with("v2.toml: config error: [engines.py] startup_timeout: expected a non-negative number, got string \"30s\""), "{e}");
        let e = project("neg", "[engines.js]\ncall_timeout = -1\n").unwrap_err().0;
        assert!(e.ends_with("expected a non-negative number, got integer -1"), "{e}");
        let e = project("negf", "[engines.js]\ncall_timeout = -1e300\n").unwrap_err().0;
        assert!(e.ends_with("expected a non-negative number, got float -1e300"), "{e}");
        let e = project("nan", "[engines.js]\ncall_timeout = nan\n").unwrap_err().0;
        assert!(e.ends_with("expected a non-negative number, got float nan"), "{e}");
        let e = project("dup", "[engines.python]\n[engines.py]\n").unwrap_err().0;
        assert!(e.ends_with("[engines.python] and [engines.py] name the same engine"), "{e}");
        // Errors follow the file's order, not the keys' alphabetical order.
        let e = project("order", "[python]\nvenv = 5\n[engines.py]\ncall_timeout = \"x\"\n").unwrap_err().0;
        assert!(e.ends_with("[python] venv: expected a string, got integer 5"), "{e}");
        let e = project("order2", "[engines.py]\ncomand = \"x\"\n[python]\nvenv = 5\n").unwrap_err().0;
        assert!(e.ends_with("unknown key [engines.py] comand (did you mean 'command'?)"), "{e}");
        let e = project("kind", "[engines.py]\nkind = \"run\"\n").unwrap_err().0;
        assert!(e.ends_with("built-in engine 'py' can't change 'kind'"), "{e}");
        let e = project("empty", "[engines.py]\ncommand = []\n").unwrap_err().0;
        assert!(e.ends_with("[engines.py] command: expected a non-empty array of non-empty strings"), "{e}");
        let e = project("section", "engines = 1\n").unwrap_err().0;
        assert!(e.ends_with("v2.toml: config error: [engines]: expected a table, got integer 1"), "{e}");
    }

    #[test]
    fn commands_and_flags_take_only_an_array() {
        // 18.2: a string is never split; for a command the error shows the array to write.
        let e = project("flagstr", "[engines.c]\nflags = \"-O2 -Wall\"\n").unwrap_err().0;
        assert!(e.ends_with("[engines.c] flags: expected an array of strings, got string \"-O2 -Wall\""), "{e}");
        let e = project("cmdstr", "[engines.py]\ncommand = \"py -3\"\n").unwrap_err().0;
        assert!(e.ends_with("v2.toml: config error: [engines.py] command must be an array: write [\"py\", \"-3\"]"), "{e}");
        let e = project("cmdpath", "[engines.py]\ncommand = 'C:\\Py\\python.exe -X utf8'\n").unwrap_err().0;
        assert!(e.ends_with("[engines.py] command must be an array: write ['C:\\Py\\python.exe', \"-X\", \"utf8\"]"), "{e}");
        // A program path with spaces stays one item.
        let e = project("cmdspace", "[engines.py]\ncommand = \"C:/Program Files/Python311/python.exe\"\n").unwrap_err().0;
        assert!(e.ends_with("command must be an array: write [\"C:/Program Files/Python311/python.exe\"]"), "{e}");
        let e = project("cmdspace2", "[engines.py]\ncommand = \"/opt/my tools/python3 -X utf8\"\n").unwrap_err().0;
        assert!(e.ends_with("command must be an array: write [\"/opt/my tools/python3\", \"-X\", \"utf8\"]"), "{e}");
        let e = project("cmdnode", "[engines.js]\ncommand = \"node --no-warnings\"\n").unwrap_err().0;
        assert!(e.ends_with("command must be an array: write [\"node\", \"--no-warnings\"]"), "{e}");
        // A program without spaces, then words: each its own item (`env` exists or not).
        let e = project("cmdenv", "[engines.py]\ncommand = \"/usr/bin/env python3\"\n").unwrap_err().0;
        assert!(e.ends_with("command must be an array: write [\"/usr/bin/env\", \"python3\"]"), "{e}");
        let e = project("cmduv", "[engines.py]\ncommand = \"~/.local/bin/uv run python\"\n").unwrap_err().0;
        assert!(e.ends_with("command must be an array: write [\"~/.local/bin/uv\", \"run\", \"python\"]"), "{e}");
        let e = project("cmdscript", "[engines.py]\ncommand = \"python3 ./wrap.py\"\n").unwrap_err().0;
        assert!(e.ends_with("command must be an array: write [\"python3\", \"./wrap.py\"]"), "{e}");
        // A quoted program loses its quotes, with either slash.
        let e = project("cmdquoted", "[engines.py]\ncommand = '\"C:\\Program Files\\Python311\\python.exe\" -X utf8'\n").unwrap_err().0;
        assert!(e.ends_with("command must be an array: write ['C:\\Program Files\\Python311\\python.exe', \"-X\", \"utf8\"]"), "{e}");
        let e = project("cmdquoted2", "[engines.py]\ncommand = '\"C:/Program Files/Python311/python.exe\" -X utf8'\n").unwrap_err().0;
        assert!(e.ends_with("command must be an array: write [\"C:/Program Files/Python311/python.exe\", \"-X\", \"utf8\"]"), "{e}");
        // An existing program with spaces in a relative path, found from the file's folder.
        let d = temp("cmdexists");
        std::fs::create_dir_all(d.join("my tools")).unwrap();
        std::fs::write(d.join("my tools").join("py"), "").unwrap();
        std::fs::write(d.join("v2.toml"), "[engines.py]\ncommand = \"my tools/py -X utf8\"\n").unwrap();
        let e = load_from(&d, vars(&[])).unwrap_err().0;
        assert!(e.ends_with("command must be an array: write [\"my tools/py\", \"-X\", \"utf8\"]"), "{e}");
        assert_eq!(toml_string("it's \"x\"\\y"), "\"it's \\\"x\\\"\\\\y\"");
        // Explorer's "Copy as path" in a TOML string: the quotes are taken off (18.1).
        let exe = if cfg!(windows) { "C:\\Program Files\\Python311\\python.exe" } else { "/opt/my python/bin/python3" };
        let cfg = project("cmdpasted", &format!("[engines.py]\ncommand = ['\"{exe}\"', \"-X\"]\n")).unwrap();
        assert_eq!(cfg.engines["py"].command.as_ref().unwrap().value, [exe, "-X"]);
        let cfg = project("venvpasted", "[python]\nvenv = '\".venv\"'\n").unwrap();
        assert_eq!(cfg.python_venv.unwrap().value, cfg.root.join(".venv"));
        // A program's name where the engine's goes (18.2).
        for (table, engine) in [("node", "js"), ("nodejs", "js"), ("python3", "py"), ("pypy3", "py"), ("gcc", "c"), ("rustc", "rust")] {
            let e = project(&format!("prog{table}"), &format!("[engines.{table}]\ncommand = [\"x\"]\n")).unwrap_err().0;
            assert!(e.ends_with(&format!("[engines.{table}] names a program, not an engine: it runs the {engine} engine, so write [engines.{engine}] command = [\"{table}\"]")), "{e}");
        }
        // A misspelled engine configures nothing: an error, not silence (18.2). A later engine's
        // name (ruby, close to rust) and any other name are kept for custom engines.
        for (table, near) in [("pyy", "py"), ("Python", "python"), ("PY", "py"), ("jss", "js")] {
            let e = project(&format!("typo{table}"), &format!("[engines.{table}]\ncommand = [\"x\"]\n")).unwrap_err().0;
            assert!(e.ends_with(&format!("[engines.{table}] isn't an engine of this version: did you mean [engines.{near}]? (custom engines aren't in this version of V2 yet)")), "{e}");
        }
        assert!(project("later", "[engines.ruby]\ncommand = [\"ruby\"]\n[engines.perl]\ncommand = [\"perl\"]\n").is_ok());
        // [python] venv and [engines.py] command both choose the Python (18.3).
        let e = project("venvcmd", "[python]\nvenv = \".venv\"\n[engines.py]\ncommand = [\"python3\"]\n").unwrap_err().0;
        assert!(e.ends_with("v2.toml: config error: [python] venv and [engines.py] command both choose the Python: keep one (venv for a virtual environment, command for any other Python)"), "{e}");
        let e = project("cmdempty", "[engines.py]\ncommand = \" \"\n").unwrap_err().0;
        assert!(e.ends_with("[engines.py] command: expected an array of strings, got string \" \""), "{e}");
    }

    #[test]
    fn unknown_and_inapplicable_keys_are_errors() {
        // 18.5: every one is a configuration error, named with the file.
        for (toml_text, message) in [
            ("foo = \"x\"\n", "unknown key foo"),
            ("pythn = 1\n", "unknown key pythn (did you mean 'python'?)"),
            ("[engines.py]\ncomand = \"x\"\n", "unknown key [engines.py] comand (did you mean 'command'?)"),
            ("[engines.py]\naliases = [\"p\"]\n", "unknown key [engines.py] aliases"),
            ("[pyton]\n", "unknown section [pyton] (did you mean 'python'?)"),
            ("[project]\nname = \"x\"\n", "unknown section [project]"),
            ("[node]\nmodules = \"m\"\n", "unknown section [node]"),
            ("[ffi]\nsearch_paths = []\n", "unknown section [ffi]"),
            ("[python]\npaths = [\"lib\"]\n", "unknown key [python] paths"),
            ("[python]\nvenvv = \".venv\"\n", "unknown key [python] venvv (did you mean 'venv'?)"),
            ("[engines.py]\nflags = []\n", "[engines.py] flags only applies to c, cpp and rust"),
            ("[engines.rust]\nstartup_timeout = 5\n", "[engines.rust] startup_timeout only applies to py and js"),
        ] {
            let e = project("unknown", toml_text).unwrap_err().0;
            assert!(e.ends_with(&format!("v2.toml: config error: {message}")), "{toml_text:?}: {e}");
        }
    }

    #[test]
    fn windows_habits_get_hints() {
        // A Windows path in "…" reads its backslashes as escapes: the hint says what to write (18.5).
        let hint = "\n  hint: in \"…\", a backslash starts an escape: write Windows paths with / or in single quotes, as in venv = 'C:\\proj\\.venv'";
        for toml_text in ["[python]\nvenv = \"C:\\Users\\me\\proj\\.venv\"\n", "[python]\nvenv = \"D:\\projects\\new\\.venv\"\n"] {
            let e = project("backslash", toml_text).unwrap_err().0;
            assert!(e.contains(": config error: ") && e.ends_with(hint), "{toml_text:?}: {e}");
        }
        assert!(project("single", "[python]\nvenv = 'C:\\proj\\.venv'\n").is_ok());
        // `~/` is the home folder, as in a shell.
        let home = home_dir().expect("a home folder in tests");
        assert_eq!(project("tilde", "[python]\nvenv = \"~/venvs/x\"\n").unwrap().python_venv.unwrap().value, home.join("venvs/x"));
        assert_eq!(project("tilde2", "[engines.py]\ncommand = [\"~/py/bin/python3\", \"-X\"]\n").unwrap().engines["py"].command.as_ref().unwrap().value[0], home.join("py/bin/python3").to_string_lossy());
        // Text without quotes.
        let e = project("bare", "[python]\nvenv = .venv\n").unwrap_err().0;
        assert!(e.ends_with("\n  hint: text goes in quotes: venv = \".venv\""), "{e}");
        let e = project("barecomment", "[python]\nvenv = .venv   # the project's env\n").unwrap_err().0;
        assert!(e.ends_with("\n  hint: text goes in quotes: venv = \".venv\""), "{e}");
        let e = project("eqeq", "[python]\nvenv == \".venv\"\n").unwrap_err().0;
        assert!(!e.contains("hint:"), "{e}");
        // Windows PowerShell 5's `>` writes UTF-16.
        let d = temp("utf16");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend("[python]\n".encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(d.join("v2.toml"), bytes).unwrap();
        let e = load_from(&d, vars(&[])).unwrap_err().0;
        assert!(e.ends_with("v2.toml: config error: the file is not valid UTF-8: it is UTF-16 (what Windows PowerShell's > and Out-File write); save it as UTF-8"), "{e}");
        // cmd's `set V2_ENGINE_PY="C:\…\python.exe"` keeps its quotes; one pair is taken off (18.4).
        let exe = if cfg!(windows) { "C:\\Python 3\\python.exe" } else { "/opt/python 3/bin/python3" };
        let cfg = project_env("quoted", "", &[("V2_ENGINE_PY", &format!("\"{exe}\""))]).unwrap();
        assert_eq!(cfg.engines["py"].command.as_ref().unwrap().value, vec![exe]);
    }

    #[test]
    fn any_engines_ts_table_is_an_error() {
        // ts runs on the js engine, so any [engines.ts] (or [engines.typescript]) table is an error (18.2).
        for (toml_text, name) in [
            ("[engines.ts]\n", "ts"),
            ("[engines.ts]\ncall_timeout = 1\n", "ts"),
            ("[engines.ts]\ntranspiler = \"tsx\"\n", "ts"),
            ("[engines.typescript]\ncommand = \"tsc\"\n", "typescript"),
        ] {
            let e = project("enginests", toml_text).unwrap_err().0;
            assert!(e.ends_with(&format!("v2.toml: config error: [engines.{name}]: ts runs on the js engine; configure [engines.js]")), "{toml_text:?}: {e}");
        }
    }

    #[test]
    fn an_override_replaces_the_files_command_unvalidated() {
        // 18.3: a command that V2_ENGINE_<TAG> replaces is never validated.
        let cfg = project_env("overbad", "[engines.py]\ncommand = []\ncall_timeout = 2\n", &[("V2_ENGINE_PY", "python")]).unwrap();
        let py = &cfg.engines["py"];
        assert_eq!(py.command.as_ref().unwrap().value, vec!["python"]);
        assert_eq!(py.command.as_ref().unwrap().from, ConfigSource::Env("V2_ENGINE_PY".into()));
        assert_eq!(py.call_timeout.as_ref().unwrap().value, 2.0, "the file's other keys stay");
        assert!(matches!(py.call_timeout.as_ref().unwrap().from, ConfigSource::Project(_)));
        // Without the override the bad command is an error.
        let e = project("overbad2", "[engines.py]\ncommand = []\n").unwrap_err().0;
        assert!(e.ends_with("[engines.py] command: expected a non-empty array of non-empty strings"), "{e}");
    }

    #[test]
    fn engine_commands_resolve_against_the_declaring_folder() {
        // 18.1: argv[0] with a separator; later elements starting with ./ or ../, or naming
        // something that exists in the folder. Inline scripts and missing names stay as written.
        let d = temp("argv");
        std::fs::create_dir_all(d.join("adapters")).unwrap();
        std::fs::write(d.join("adapters/x.py"), "").unwrap();
        let toml_text = "[engines.py]\ncommand = [\"tools/python\", \"adapters/x.py\", \"./run.sh\", \"-X\", \"-a/b\", \"{file}/x\", \"utf8\", \"cd src/x && make\", \"lib/missing.py\"]\n[engines.js]\ncommand = [\"node\"]\n";
        std::fs::write(d.join("v2.toml"), toml_text).unwrap();
        let cfg = load_from(&d, vars(&[])).unwrap();
        let py = &cfg.engines["py"].command.as_ref().unwrap().value;
        assert_eq!(py[0], cfg.root.join("tools/python").to_string_lossy());
        assert_eq!(py[1], cfg.root.join("adapters/x.py").to_string_lossy());
        assert_eq!(py[2], cfg.root.join("./run.sh").to_string_lossy());
        assert_eq!(&py[3..], ["-X", "-a/b", "{file}/x", "utf8", "cd src/x && make", "lib/missing.py"]);
        assert_eq!(cfg.engines["js"].command.as_ref().unwrap().value, vec!["node"]);
        // Later elements with a root (Windows switches such as /c) are not paths to rewrite.
        let switches = project("argvroot", "[engines.py]\ncommand = [\"cmd\", \"/c\", \"python\", \"/tools/x.py\"]\n").unwrap();
        assert_eq!(switches.engines["py"].command.as_ref().unwrap().value, vec!["cmd", "/c", "python", "/tools/x.py"]);
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_relative_path_is_a_config_error() {
        // `C:venv` would resolve against drive C's cwd, not the project root.
        let e = project("driverel", "[python]\nvenv = \"C:venv\"\n").unwrap_err().0;
        assert!(e.ends_with("v2.toml: config error: [python] venv: 'C:venv' is a drive-relative path: write it relative to the file, or absolute"), "{e}");
        // Engine commands too: argv[0] with or without a separator, and later path-like elements.
        for (toml_text, what, bad) in [
            ("[engines.py]\ncommand = [\"C:python.exe\"]\n", "[engines.py] command", "C:python.exe"),
            ("[engines.py]\ncommand = [\"C:tools/python\"]\n", "[engines.py] command", "C:tools/python"),
            ("[engines.py]\ncommand = [\"python\", \"C:adapters/x.py\"]\n", "[engines.py] command", "C:adapters/x.py"),
        ] {
            let e = project("driverelcmd", toml_text).unwrap_err().0;
            assert!(e.ends_with(&format!("config error: {what}: '{bad}' is a drive-relative path: write it relative to the file, or absolute")), "{e}");
        }
        // A later element that isn't path-like is left alone.
        assert!(project("driverelok", "[engines.py]\ncommand = [\"python\", \"-X\", \"C:x\"]\n").is_ok());
    }

    #[test]
    fn environment_overrides() {
        // 18.4: the prefix test compares bytes, so a multibyte char across byte 10 is skipped.
        let d = temp("env");
        let env = vars(&[("我的环境变量", "x"), ("AБВГДЕЖЗИ", "x"), ("V2_ENGINEé", "x"), ("V2_ENGINE_PY", "bin/py"), ("V2_ENGINE_JS", "")]);
        let cfg = load_from(&d, env).unwrap();
        let py = cfg.engines["py"].command.as_ref().unwrap();
        assert_eq!(py.value, vec![std::env::current_dir().unwrap().join("bin/py").to_string_lossy().into_owned()]);
        assert_eq!(py.from, ConfigSource::Env("V2_ENGINE_PY".into()));
        assert!(!cfg.engines.contains_key("js"), "an empty variable is unset");
        // Only canonical tags count: an alias is ignored like an unknown tag.
        let cfg = load_from(&d, vars(&[("V2_ENGINE_PYTHON", "b"), ("V2_ENGINE_JAVASCRIPT", "n")])).unwrap();
        assert!(cfg.engines.is_empty(), "aliases are not tags here");
    }

    #[cfg(not(windows))]
    #[test]
    fn engine_variables_name_tags_in_upper_case() {
        // 18.4: off Windows the tag is upper case like the prefix, so no two variables name one
        // engine.
        let d = temp("envcase");
        let cfg = load_from(&d, vars(&[("V2_ENGINE_Py", "a"), ("V2_ENGINE_py", "b")])).unwrap();
        assert!(cfg.engines.is_empty(), "{:?}", cfg.engines.keys().collect::<Vec<_>>());
    }

    #[test]
    fn overrides_of_custom_and_undeclared_tags() {
        // A custom engine gets command[0]; a tag nobody declares is ignored (18.4).
        let env = [("V2_ENGINE_RUBY", "r3"), ("V2_ENGINE_LUA", "lua5")];
        let cfg = project_env("custom", "[engines.ruby]\nkind = \"run\"\ncommand = [\"ruby\", \"{file}\"]\n", &env).unwrap();
        let ruby = cfg.custom["ruby"]["command"].as_array().unwrap();
        assert!(ruby[0].as_str() == Some("r3") && ruby[1].as_str() == Some("{file}"));
        assert!(!cfg.custom.contains_key("lua") && !cfg.engines.contains_key("lua"));
    }

    #[test]
    fn v2_engine_ts_names_v2_engine_js() {
        // ts has no executable of its own (18.4).
        let d = temp("envts");
        let e = load_from(&d, vars(&[("V2_ENGINE_TS", "x")])).unwrap_err().0;
        assert_eq!(e, "config error: V2_ENGINE_TS: ts runs on the js engine; set V2_ENGINE_JS");
        assert!(load_from(&d, vars(&[("V2_ENGINE_TYPESCRIPT", "x")])).is_ok(), "an alias is ignored");
        assert!(load_from(&d, vars(&[("V2_ENGINE_TS", "")])).is_ok(), "an empty variable is unset");
    }
}
