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
/// The names `[engines.…]` takes for the built-in engines a did-you-mean offers (18.2).
const ENGINE_NAMES: &[&str] = &["py", "python", "js", "javascript", "c", "cpp", "cxx", "rust", "rs"];
/// The engine names a bare key with another keyboard's letter may have meant (`[engines.tş]`):
/// ts's too, which `bare_key_hint` sends to `[engines.js]` (18.5).
const KEY_ENGINE_NAMES: &[&str] = &["py", "python", "js", "javascript", "ts", "typescript", "c", "cpp", "cxx", "rust", "rs"];

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

/// A path from a `"…"` TOML string holding what its backslash escapes made (`\n` a line break,
/// `\t` a tab, `\b`, `\f`, `\r`, `\e`): the hint that says so and what to write (18.5), with an
/// example for `key` (`venv`, `py`, or any other engine's `command`).
pub(crate) fn escape_hint(written: &str, key: &str) -> Option<String> {
    if !written.contains(char::is_control) {
        return None;
    }
    let example = match key {
        "venv" => r"venv = 'C:\proj\.venv'",
        "py" => r"command = ['C:\Program Files\Python311\python.exe']",
        _ => r"command = ['C:\Program Files\nodejs\node.exe']",
    };
    Some(format!(r#"in "…", a backslash starts an escape (\n is a line break, \t a tab): write the path with / or in single quotes, as in {example}"#))
}

/// `escape_hint` for a configured path, by the part its `v2.toml` wrote: a relative one is
/// joined to the file's folder (18.1), whose own name isn't the user's escape. A
/// `V2_ENGINE_<TAG>` path has no escapes.
pub(crate) fn configured_escape_hint(path: &Path, from: &ConfigSource, key: &str) -> Option<String> {
    let ConfigSource::Project(file) = from else { return None };
    let written = file.parent().and_then(|base| path.strip_prefix(base).ok()).unwrap_or(path);
    escape_hint(&written.to_string_lossy(), key)
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
    // `venv_dir`, `command_line`: a known key with more after it.
    let longer = || known.iter().copied().find(|k| k.len() >= 4 && key.len() > k.len() && key.starts_with(k) && !key[k.len()..].starts_with(char::is_alphanumeric));
    match did_you_mean(key, known.iter().copied()).or_else(longer) {
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
    /// is an error (18.1). `"C:\bin\node.exe"` is one too, `\b` and `\n` being escapes: shown
    /// escaped, with the hint that says so.
    fn not_drive_relative(&self, what: &str, text: &str) -> Result<(), ConfigError> {
        let p = Path::new(text);
        if matches!(p.components().next(), Some(Component::Prefix(_))) && !p.has_root() {
            let shown = crate::syntax::span::shown_path(text);
            let key = if what.ends_with("venv") { "venv" } else if what.starts_with("[engines.py") { "py" } else { "js" };
            let hint = escape_hint(text, key).map(|h| format!("\n  hint: {h}")).unwrap_or_default();
            return Err(self.error(format!("{what}: '{shown}' is a drive-relative path: write it relative to the file, or absolute{hint}")));
        }
        Ok(())
    }

    /// `~/x` with no home folder set (`env -i`, a service without one): an error, not a folder
    /// named `~` next to the file (18.1).
    fn no_home(&self, what: &str, text: &str) -> Result<(), ConfigError> {
        let tilde = text == "~" || text.starts_with("~/") || text.starts_with("~\\");
        if tilde && home_dir().is_none() {
            let text = crate::syntax::span::shown_path(text);
            return Err(self.error(format!("{what}: '{text}' starts with ~, but {HOME_VAR} isn't set, so there is no home folder for it: write the full path")));
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
        self.no_home(what, &text)?;
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
            if i == 0 {
                self.no_home(what, a)?;
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

/// What iconv's line converts for a `v2.toml` at `path` that isn't UTF-8 (2.1, 18.5): the file
/// is found from the script's folder up (18.1), so one outside the current folder is named by
/// its path, with `new.toml` beside it.
fn iconv_files(path: &Path) -> String {
    let here = std::env::current_dir().ok().map(|d| absolute_clean(&d));
    if path.parent().is_some_and(|p| Some(p) == here.as_deref()) {
        return "v2.toml > new.toml && mv new.toml v2.toml".to_string();
    }
    let word = |p: &Path| crate::cli::shell_word(&crate::syntax::span::shown_path(&p.display().to_string()));
    let (file, new) = (word(path), word(&path.with_file_name("new.toml")));
    format!("{file} > {new} && mv {new} {file}")
}

/// The project's `v2.toml`, in the file's order: its structure, known sections and keys, keys
/// that apply, and each value's type (18.2, 18.5). A built-in `command` that `V2_ENGINE_<TAG>`
/// replaces is not validated (18.3).
fn read_file(cfg: &mut Config, path: &Path, overrides: &Overrides) -> Result<(), ConfigError> {
    // A folder's name can't act on the terminal (2.1).
    let shown = crate::syntax::span::shown_path(&path.display().to_string());
    let bytes = std::fs::read(path).map_err(|e| {
        // Another program has it open without sharing: what FILE gets (19.1).
        let hint = crate::syntax::span::read_hint(&e).map(|h| format!("\n  hint: {h}")).unwrap_or_default();
        ConfigError(format!("{shown}: config error: cannot read the file: {}{hint}", crate::syntax::span::os_message(&e)))
    })?;
    let text = String::from_utf8(bytes).map_err(|e| {
        // iconv's line names this file (2.1).
        let why = match crate::syntax::span::not_utf8_named(e.as_bytes()) {
            Some(m) => m.trim_start_matches("file is not valid UTF-8").to_string(),
            None => format!(": {}", crate::syntax::span::older_encoding(&iconv_files(path))),
        };
        ConfigError(format!("{shown}: config error: the file is not valid UTF-8{why}"))
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        let mut message = e.message().trim().to_string();
        let line_text = e.span().map(|span| {
            let start = text[..span.start.min(text.len())].rfind('\n').map_or(0, |i| i + 1);
            text[start..].lines().next().unwrap_or_default()
        });
        let one_line = line_text.and_then(|l| l.trim_start().strip_prefix('[')).and_then(|l| l.split_once(']')).filter(|(_, rest)| rest.contains('='));
        // A Windows path in "…" is read with backslash escapes (18.5).
        if let Some((section, rest)) = one_line {
            message.push_str(&format!("\n  hint: a section's name goes on a line of its own, its keys on the lines below it:\n    [{}]\n    {}", section.trim(), rest.trim()));
        } else if (message.contains("escape") || message.contains("unicode")) && line_text.is_some_and(|l| l.contains('\\')) {
            message.push_str("\n  hint: in \"…\", a backslash starts an escape: write Windows paths with / or in single quotes, as in venv = 'C:\\proj\\.venv'");
        } else if let Some(hint) = e.span().and_then(|span| bare_key_hint(text, span.start, &message)) {
            message.push_str(&format!("\n  hint: {hint}"));
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
            // A key without its section: name the section it belongs in.
            _ if PYTHON_KEYS.contains(&section.as_str()) => return Err(f.error(format!("{section} goes in the [python] section: put a line [python] above it"))),
            _ if ENGINE_KEYS.contains(&section.as_str()) => {
                return Err(f.error(format!("{section} goes in an engine's section: put a line such as [engines.py] or [engines.js] above it")))
            }
            // A top-level key that is not a section is an unknown key (18.5).
            _ => return Err(f.error(unknown(&format!("key {section}"), section, SECTIONS))),
        };
        match section.as_str() {
            "python" => {
                for (k, v) in body {
                    if k != "venv" {
                        return Err(f.error(unknown(&format!("key [python] {k}"), k, PYTHON_KEYS)));
                    }
                    // `venv = ""` would be the project folder itself (18.3).
                    if v.as_str().is_some_and(|s| s.trim().is_empty()) {
                        return Err(f.error("[python] venv is empty: name the virtual environment's folder, as in venv = \".venv\"".into()));
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

/// `[pythın]`, `vënv = …`: TOML's "invalid unquoted key" at byte `at`, a character a bare key
/// can't hold (18.5). The hint names it as the lexer does (3.2: a bidi or control character by
/// code point), and the section, engine or key the key read whole is close to, compared as an
/// ASCII keyboard writes it (`folded_key`); a character after the key (`venv\u{a0}=`) gets the
/// lexer's hint for it instead.
fn bare_key_hint(text: &str, at: usize, message: &str) -> Option<String> {
    let c = text.get(at..)?.chars().next()?;
    if c.is_ascii() || !message.contains("unquoted key") {
        return None;
    }
    let (shown, instead) = crate::syntax::lexer::shown_char(c);
    let shown = if shown == format!("'{c}'") { format!("{shown} (U+{:04X})", c as u32) } else { shown };
    let hint = format!("{shown} can't be in a bare key: TOML takes A-Z, a-z, 0-9, - and _ there");
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let stop = |c: char| c.is_whitespace() || "[].=\"'#,".contains(c);
    let lo = text[start..at].char_indices().rev().find(|&(_, c)| stop(c)).map_or(start, |(i, c)| start + i + c.len_utf8());
    let hi = text[at..].find(stop).map_or(text.len(), |i| at + i);
    if hi == at {
        return Some(match instead {
            Some(h) => format!("{hint} — {h}"),
            None => hint,
        });
    }
    let head: String = text[start..lo].split_whitespace().collect();
    // In a `[…]` line, a section or an engine; on a key's line, a key of the section above it.
    let section = || {
        let line = text[..start].lines().rev().map(str::trim).find(|l| l.starts_with('['))?;
        Some(line.trim_start_matches('[').split(']').next()?.split_whitespace().collect::<String>())
    };
    // What follows the key in its `[…]` (`[ëngines.py]`: `.py`) stays in the suggestion, folded
    // too (`.pÿ`, `.PY`: `.py`); one still not ASCII then gets none.
    let written: String = text[hi..].lines().next().unwrap_or_default().split(']').next().unwrap_or_default().split_whitespace().collect();
    let rest = folded_key(&written);
    // The suggestion for a known name, given what follows the key.
    type Near = fn(&str, &str) -> String;
    let (known, near): (&[&str], Near) = match head.as_str() {
        "[" if !rest.is_ascii() => return Some(hint),
        "[" => (SECTIONS, |s, rest| match rest.strip_prefix('.').map(canonical_tag) {
            Some("ts") if s == "engines" => "ts runs on the js engine: configure [engines.js]".into(),
            _ => format!("did you mean [{s}{rest}]?"),
        }),
        "[engines." => (KEY_ENGINE_NAMES, |s, _| match canonical_tag(s) {
            "ts" => "ts runs on the js engine: configure [engines.js]".into(),
            _ => format!("did you mean [engines.{s}]?"),
        }),
        "" => match section() {
            Some(s) if s == "python" => (PYTHON_KEYS, |s, _| format!("did you mean '{s}'?")),
            Some(s) if s.starts_with("engines.") => (ENGINE_KEYS, |s, _| format!("did you mean '{s}'?")),
            _ => return Some(hint),
        },
        _ => return Some(hint),
    };
    let folded = folded_key(&text[lo..hi]);
    // `venv\u{200b}`: a did-you-mean of the key read without the character names what the user
    // sees already; the character's own hint (`delete it`) says what to do. `Venv\u{200b}` keeps
    // the did-you-mean: deleting it leaves the capital, and so does `[engines\u{200b}.PY]`, whose
    // rest the suggestion folds.
    let without = format!("{}{}", &text[lo..at], &text[at + c.len_utf8()..hi]);
    match known.iter().copied().find(|k| *k == folded).or_else(|| did_you_mean(&folded, known.iter().copied())) {
        Some(k) => Some(match (near(k, &rest), instead) {
            (said, Some(h)) if k == without && written == rest && said.starts_with("did you mean") => format!("{hint} — {h}"),
            (said, _) => format!("{hint} — {said}"),
        }),
        None => Some(hint),
    }
}

/// A key as an ASCII keyboard writes it (18.5): lower case, ı and İ as i, ß as ss, the Latin
/// letters of Latin-1 and Latin Extended-A without their diacritics (ö as o, ş as s, ł as l),
/// combining marks dropped.
fn folded_key(key: &str) -> String {
    // Each character's base letter, 16 to a row; `*` keeps the character.
    const LATIN_1: &[u8] = concat!("aaaaaaaceeeeiiii", "dnooooo*ouuuuy*s", "aaaaaaaceeeeiiii", "dnooooo*ouuuuy*y").as_bytes();
    const EXTENDED_A: &[u8] = concat!(
        "aaaaaaccccccccdd", // U+0100
        "ddeeeeeeeeeegggg",
        "gggghhhhiiiiiiii",
        "iiiijjkkklllllll", // U+0130: İ, ı
        "lllnnnnnnnnnoooo",
        "oooorrrrrrssssss",
        "ssttttttuuuuuuuu",
        "uuuuwwyyyzzzzzzs",
    )
    .as_bytes();
    let mut out = String::new();
    for c in key.chars() {
        let base = match c as u32 {
            n @ 0x00C0..=0x00FF => Some(LATIN_1[(n - 0xC0) as usize]),
            n @ 0x0100..=0x017F => Some(EXTENDED_A[(n - 0x100) as usize]),
            0x0300..=0x036F => continue,
            _ => None,
        };
        match base.filter(|b| *b != b'*') {
            Some(b) if c == 'ß' => out.extend([b as char, b as char]),
            Some(b) => out.push(b as char),
            None => out.extend(c.to_lowercase()),
        }
    }
    out
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
        // no word (18.2); a later engine's name (`ruby`) isn't a misspelling, nor is a table with
        // a `kind`, which only a custom engine has (`[engines.r]`, DOCS 13).
        let custom = t.contains_key("kind");
        if !BUILTIN_TAGS.contains(&tag.as_str()) && crate::engines::registry::later_language(&tag).is_none() && !custom {
            if let Some(near) = crate::runtime::error::did_you_mean(name, ENGINE_NAMES.iter().copied()) {
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
    fn an_empty_venv_is_an_error() {
        // 18.3: `venv = ""` would name the project folder itself.
        let e = project("emptyvenv", "[python]\nvenv = \"\"\n").unwrap_err().0;
        assert!(e.ends_with("[python] venv is empty: name the virtual environment's folder, as in venv = \".venv\""), "{e}");
    }

    #[test]
    fn a_non_ascii_bare_key_is_named_with_the_key_meant() {
        // 18.5 (round 20): TOML's "expected letters" means ASCII ones; a Turkish or German
        // keyboard's letter is named, and the section, engine or key it was close to suggested.
        let rule = "can't be in a bare key: TOML takes A-Z, a-z, 0-9, - and _ there";
        let cases = [
            ("[pyth\u{131}n]\nvenv = \".venv\"\n", "1:6", format!("'\u{131}' (U+0131) {rule} — did you mean [python]?")),
            ("[PYTH\u{130}N]\n", "1:6", format!("'\u{130}' (U+0130) {rule} — did you mean [python]?")),
            ("[python]\nv\u{eb}nv = \".venv\"\n", "2:2", format!("'\u{eb}' (U+00EB) {rule} — did you mean 'venv'?")),
            ("[engines.p\u{131}]\ncommand = [\"python3\"]\n", "1:11", format!("'\u{131}' (U+0131) {rule} — did you mean [engines.py]?")),
            ("[engines.js]\nst\u{e4}rt\u{fc}p_timeout = 5\n", "2:3", format!("'\u{e4}' (U+00E4) {rule} — did you mean 'startup_timeout'?")),
            ("[gr\u{f6}\u{df}e]\n", "1:4", format!("'\u{f6}' (U+00F6) {rule}")),
            ("\u{e7}ay = 1\n", "1:1", format!("'\u{e7}' (U+00E7) {rule}")),
            // Round 20: ts's names, which configure [engines.js]; the rest of a header kept.
            ("[engines.t\u{15f}]\ncommand = [\"x\"]\n", "1:11", format!("'\u{15f}' (U+015F) {rule} — ts runs on the js engine: configure [engines.js]")),
            ("[engines.typescr\u{131}pt]\n", "1:17", format!("'\u{131}' (U+0131) {rule} — ts runs on the js engine: configure [engines.js]")),
            ("[\u{eb}ngines.py]\ncommand = [\"python3\"]\n", "1:2", format!("'\u{eb}' (U+00EB) {rule} — did you mean [engines.py]?")),
            ("[eng\u{ef}nes . js ]\n", "1:5", format!("'\u{ef}' (U+00EF) {rule} — did you mean [engines.js]?")),
            // Named as the lexer names it: a bidi or control character by code point, so it can't
            // act on the terminal; one after the key gets what to type instead.
            ("[py\u{202e}thon]\n", "1:4", format!("U+202E {rule} — did you mean [python]?")),
            ("[py\u{85}thon]\n", "1:4", format!("U+0085 {rule}")),
            ("[python]\nvenv\u{a0}= \".venv\"\n", "2:5", format!("U+00A0 (a no-break space) {rule} — type a plain space instead")),
            ("[python]\n\u{ff56}env = \".venv\"\n", "2:1", format!("'\u{ff56}' (a full-width 'v' from an input method) {rule} — did you mean 'venv'?")),
            // A section's rest is folded too, ts's names in it get ts's line, and a rest still
            // not ASCII no suggestion.
            ("[\u{eb}ngines.ts]\n", "1:2", format!("'\u{eb}' (U+00EB) {rule} — ts runs on the js engine: configure [engines.js]")),
            ("[\u{eb}ngines.typescript]\n", "1:2", format!("'\u{eb}' (U+00EB) {rule} — ts runs on the js engine: configure [engines.js]")),
            ("[\u{eb}ngines.p\u{ff}]\ncommand = [\"python3\"]\n", "1:2", format!("'\u{eb}' (U+00EB) {rule} — did you mean [engines.py]?")),
            ("[\u{eb}ngines.p\u{3b1}]\n", "1:2", format!("'\u{eb}' (U+00EB) {rule}")),
            // A key that reads as the name meant without its character: the character's hint.
            ("[python]\nvenv\u{200b} = \".venv\"\n", "2:5", format!("U+200B (an invisible zero-width character) {rule} — delete it")),
            ("[python]\nve\u{200b}nv = \".venv\"\n", "2:3", format!("U+200B (an invisible zero-width character) {rule} — delete it")),
            ("[engines\u{feff}.py]\n", "1:9", format!("U+FEFF (an invisible zero-width character) {rule} — delete it")),
            ("[engines.ts\u{200b}]\n", "1:12", format!("U+200B (an invisible zero-width character) {rule} — ts runs on the js engine: configure [engines.js]")),
            // Round 20d: deleting it would leave the capital, so the name meant; an upper-case
            // rest is folded as well.
            ("[python]\nVenv\u{200b} = \".venv\"\n", "2:5", format!("U+200B (an invisible zero-width character) {rule} — did you mean 'venv'?")),
            ("[Engines\u{200b}.py]\n", "1:9", format!("U+200B (an invisible zero-width character) {rule} — did you mean [engines.py]?")),
            // Deleting the character leaves `.PY`, which the suggestion folds: the did-you-mean
            // (the fourth review of round 20).
            ("[engines\u{200b}.PY]\n", "1:9", format!("U+200B (an invisible zero-width character) {rule} — did you mean [engines.py]?")),
            ("[ENG\u{130}NES.PY]\n", "1:5", format!("'\u{130}' (U+0130) {rule} — did you mean [engines.py]?")),
            ("[ENG\u{130}NES.TS]\n", "1:5", format!("'\u{130}' (U+0130) {rule} — ts runs on the js engine: configure [engines.js]")),
        ];
        for (toml_text, at, hint) in cases {
            let e = project("bareascii", toml_text).unwrap_err().0;
            assert!(e.contains(&format!("v2.toml:{at}: config error: ")) && e.ends_with(&format!("\n  hint: {hint}")), "{toml_text:?}: {e}");
        }
        // What those hints name loads.
        for followed in ["[python]\nvenv = \".venv\"\n", "[engines.py]\ncommand = [\"python3\"]\n", "[engines.js]\ncommand = [\"node\"]\n"] {
            assert!(project("bareascii", followed).is_ok(), "{followed:?}");
        }
        assert_eq!(folded_key("\u{130}stanbul-\u{15f}\u{131}\u{11f}-\u{142}\u{f8}d\u{17a}-stra\u{df}e-e\u{301}"), "istanbul-sig-lodz-strasse-e");
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
    fn only_a_regular_file_named_v2_toml_counts() {
        // 18.1, DOCS 14: a folder named v2.toml (off Windows also a broken link, a link loop, a
        // pipe or a link to a device) is passed over, and the search goes on to the parent's
        // file; a link to a regular file counts.
        let d = temp("notafile");
        std::fs::write(d.join("v2.toml"), "[engines.py]\ncall_timeout = 7\n").unwrap();
        let used = |start: &Path| {
            let cfg = load_from(start, vars(&[])).unwrap();
            let timeout = cfg.engines.get("py").and_then(|e| e.call_timeout.as_ref()).map(|t| t.value);
            (cfg.root, cfg.file, timeout)
        };
        let parent = (d.clone(), Some(d.join("v2.toml")), Some(7.0));
        let folder = d.join("folder");
        std::fs::create_dir_all(folder.join("v2.toml")).unwrap();
        assert_eq!(used(&folder), parent, "a folder named v2.toml");
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let child = |name: &str| {
                let c = d.join(name);
                std::fs::create_dir_all(&c).unwrap();
                c
            };
            let broken = child("broken");
            std::os::unix::fs::symlink("../missing/v2.toml", broken.join("v2.toml")).unwrap();
            let looped = child("loop");
            std::os::unix::fs::symlink("v2.toml", looped.join("v2.toml")).unwrap();
            let device = child("device");
            std::os::unix::fs::symlink("/dev/null", device.join("v2.toml")).unwrap();
            let pipe = child("pipe");
            let path = std::ffi::CString::new(pipe.join("v2.toml").as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0, "mkfifo");
            for c in [&broken, &looped, &device, &pipe] {
                assert_eq!(used(c), parent, "{}", c.display());
            }
            let linked = child("linked");
            std::os::unix::fs::symlink("../v2.toml", linked.join("v2.toml")).unwrap();
            assert_eq!(used(&linked), (linked.clone(), Some(linked.join("v2.toml")), Some(7.0)), "a link to a file");
        }
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
        // A table with `kind` is a custom engine's (DOCS 13.1), not a misspelling: `r` isn't `rs`.
        let cfg = project("custom", "[engines.r]\nkind = \"run\"\ncommand = [\"Rscript\", \"{file}\"]\next = \"R\"\n").unwrap();
        assert!(cfg.custom.contains_key("r"));
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
            // A key without its section line.
            ("venv = \".venv\"\n", "venv goes in the [python] section: put a line [python] above it"),
            ("command = [\"node\"]\n", "command goes in an engine's section: put a line such as [engines.py] or [engines.js] above it"),
            ("[engines.py]\ncomand = \"x\"\n", "unknown key [engines.py] comand (did you mean 'command'?)"),
            ("[engines.py]\naliases = [\"p\"]\n", "unknown key [engines.py] aliases"),
            ("[pyton]\n", "unknown section [pyton] (did you mean 'python'?)"),
            ("[project]\nname = \"x\"\n", "unknown section [project]"),
            ("[node]\nmodules = \"m\"\n", "unknown section [node]"),
            ("[ffi]\nsearch_paths = []\n", "unknown section [ffi]"),
            ("[python]\npaths = [\"lib\"]\n", "unknown key [python] paths"),
            ("[python]\nvenvv = \".venv\"\n", "unknown key [python] venvv (did you mean 'venv'?)"),
            // A known key with more after it.
            ("[python]\nvenv_dir = \".venv\"\n", "unknown key [python] venv_dir (did you mean 'venv'?)"),
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
        // A section and its key on one line, as a sentence shows them.
        let e = project("oneline", "[engines.js] command = [\"node\"]\n").unwrap_err().0;
        assert!(e.ends_with("\n  hint: a section's name goes on a line of its own, its keys on the lines below it:\n    [engines.js]\n    command = [\"node\"]"), "{e}");
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
        // An older encoding: off Windows, iconv's line names this v2.toml, not old.v2 (round 20),
        // by its path, as the tests' folder isn't the file's (tests/run.rs runs it in both).
        std::fs::write(d.join("v2.toml"), b"# Gr\xfc\xdfe\n").unwrap();
        let e = load_from(&d, vars(&[])).unwrap_err().0;
        let want = if cfg!(windows) {
            "v2.toml: config error: the file is not valid UTF-8: it may be in an older encoding, such as Windows-1252 (what Windows PowerShell's Set-Content writes): save it as UTF-8".to_string()
        } else {
            let (page, name) = crate::syntax::span::locale_code_page();
            let word = |name: &str| crate::cli::shell_word(&d.join(name).display().to_string());
            let (file, new) = (word("v2.toml"), word("new.toml"));
            format!("v2.toml: config error: the file is not valid UTF-8: it may be in an older encoding, such as {name}: save it as UTF-8, as in iconv -f {page} -t utf-8 {file} > {new} && mv {new} {file}, then check the text")
        };
        assert!(e.ends_with(&want), "{e}");
        assert_eq!(iconv_files(&std::env::current_dir().unwrap().join("v2.toml")), "v2.toml > new.toml && mv new.toml v2.toml");
        if cfg!(unix) {
            assert_eq!(iconv_files(Path::new("/my proj/v2.toml")), "\"/my proj/v2.toml\" > \"/my proj/new.toml\" && mv \"/my proj/new.toml\" \"/my proj/v2.toml\"");
        }
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
        // Round 19: "C:\bin\node.exe" is one too, its \b and \n being escapes: shown escaped,
        // with the hint that says so (18.5).
        let hint = |example: &str| format!("\n  hint: in \"…\", a backslash starts an escape (\\n is a line break, \\t a tab): write the path with / or in single quotes, as in {example}");
        let e = project("driverelesc", "[engines.js]\ncommand = [\"C:\\bin\\node.exe\"]\n").unwrap_err().0;
        let want = format!("config error: [engines.js] command: 'C:\\u{{8}}in\\node.exe' is a drive-relative path: write it relative to the file, or absolute{}", hint(r"command = ['C:\Program Files\nodejs\node.exe']"));
        assert!(e.ends_with(&want), "{e}");
        let e = project("driverelvenv", "[python]\nvenv = \"C:\\new\"\n").unwrap_err().0;
        let want = format!("config error: [python] venv: 'C:\\new' is a drive-relative path: write it relative to the file, or absolute{}", hint(r"venv = 'C:\proj\.venv'"));
        assert!(e.ends_with(&want), "{e}");
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
        let cfg = project_env("custom-env", "[engines.ruby]\nkind = \"run\"\ncommand = [\"ruby\", \"{file}\"]\n", &env).unwrap();
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
