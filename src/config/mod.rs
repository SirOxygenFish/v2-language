//! Configuration: project root, layered config files and precedence (INTERNALS ch. 18).
//!
//! Layers, lowest first: built-in defaults, `~/.v2/config.toml`, the project's `v2.toml`,
//! `V2_ENGINE_<TAG>` and `--engine TAG=PATH`. The files are read into raw values first; per key
//! the highest layer wins, and only the winning values are validated (18.3). A custom engine's
//! table comes whole from the highest layer that declares it, and is validated by `engines/spec`
//! (17.1) when custom engines arrive (roadmap Phase 3).

#![allow(dead_code)] // most settings are read by the engines (roadmap Phase 1)

pub mod rundir;

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use indexmap::IndexMap;

use crate::cli::Flags;
use crate::runtime::error::did_you_mean;
use crate::runtime::value::float_str;
use crate::syntax::scan::canonical_tag;
use crate::syntax::span::absolute_clean;

/// Where a value came from: doctor's FROM column (18.3).
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigSource {
    Default,
    User(PathBuf),
    Project(PathBuf),
    Env(String),
    Cli,
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
    pub transpiler: Option<Sourced<Vec<String>>>,
}

/// A custom engine's table as declared, with the folder its relative paths resolve against.
#[derive(Clone, Debug)]
pub struct CustomEngine {
    pub table: toml::Table,
    pub base_dir: PathBuf,
    pub from: ConfigSource,
}

#[derive(Debug, Default)]
pub struct Config {
    /// The project root: the folder of the nearest `v2.toml`, else the start folder (18.1).
    pub root: PathBuf,
    /// The config files read, lowest layer first.
    pub files: Vec<PathBuf>,
    pub project_name: Option<Sourced<String>>,
    pub entry: Option<Sourced<PathBuf>>,
    /// Built-in engines by canonical tag.
    pub engines: IndexMap<String, EngineCfg>,
    pub custom: IndexMap<String, CustomEngine>,
    pub python_venv: Option<Sourced<PathBuf>>,
    pub python_paths: Option<Sourced<Vec<PathBuf>>>,
    pub node_modules: Option<Sourced<PathBuf>>,
    pub node_paths: Option<Sourced<Vec<PathBuf>>>,
    pub ffi_search_paths: Option<Sourced<Vec<PathBuf>>>,
    /// `V2_ENGINE_*` / `--engine` overrides for tags unknown at startup (18.4).
    pub pending: Vec<(String, PathBuf, ConfigSource)>,
    /// Printed only by check and doctor (18.5).
    pub warnings: Vec<String>,
    pub no_cache: bool,
}

/// A configuration error: printed to stderr, exit 2 (18.5).
#[derive(Debug, PartialEq)]
pub struct ConfigError(pub String);

pub const BUILTIN_TAGS: &[&str] = &["py", "js", "ts", "c", "cpp", "rust"];

const SECTIONS: &[&str] = &["project", "engines", "python", "node", "ffi"];
const ENGINE_KEYS: &[&str] = &["command", "flags", "startup_timeout", "call_timeout", "transpiler"];

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
/// cwd for `v2 run` and `v2 doctor`).
pub fn load(start: &Path, flags: &Flags) -> Result<Config, ConfigError> {
    load_from(start, flags, home_dir(), std::env::vars_os())
}

/// One config file: how messages name it, the folder its relative paths resolve against, and
/// doctor's FROM for what it sets.
struct Layer {
    shown: String,
    base: PathBuf,
    from: ConfigSource,
}

/// A schema value as written, kept until every layer is read (18.3).
struct RawValue {
    /// How messages name the key: `[engines.python] command`.
    what: String,
    value: toml::Value,
    layer: Rc<Layer>,
}

/// Raw values by key (`project.entry`, `engines.py.command`, `python.paths`, ...); a higher
/// layer's value replaces a lower one's.
type Raw = IndexMap<String, RawValue>;

/// `load` with the home folder and the environment passed in.
fn load_from(
    start: &Path,
    flags: &Flags,
    home: Option<PathBuf>,
    env: impl Iterator<Item = (OsString, OsString)>,
) -> Result<Config, ConfigError> {
    let start_abs = absolute_clean(start);
    let root = find_root(&start_abs).unwrap_or_else(|| start_abs.clone());
    let mut cfg = Config { root: root.clone(), no_cache: flags.no_cache, ..Config::default() };
    let mut raw = Raw::new();
    match home {
        Some(home) => {
            let base = home.join(".v2");
            let user = base.join("config.toml");
            if user.is_file() {
                read_layer(&mut cfg, &mut raw, &user, base, ConfigSource::User(user.clone()), true)?;
            }
        }
        None => cfg.warnings.push(format!(
            "warning: no home folder ({HOME_VAR} is not set): ~/.v2/config.toml is skipped and the ~/.v2 caches are off"
        )),
    }
    let project = root.join("v2.toml");
    if project.is_file() {
        read_layer(&mut cfg, &mut raw, &project, root.clone(), ConfigSource::Project(project.clone()), false)?;
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    // L3: V2_ENGINE_<TAG>, one executable path each, never split (18.4).
    let prefix_len = "V2_ENGINE_".len();
    let mut seen: IndexMap<String, String> = IndexMap::new();
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
        let tag = canonical_tag(&k[prefix_len..].to_lowercase()).to_string();
        if let Some(other) = seen.get(&tag) {
            return Err(ConfigError(format!("config error: {other} and {k} both set the {tag} engine")));
        }
        seen.insert(tag.clone(), k.clone());
        override_engine(&mut cfg, &mut raw, &tag, from_cwd(&cwd, Path::new(&v)), ConfigSource::Env(k))?;
    }
    // L4: --engine TAG=PATH, the last one per engine wins.
    for (tag, path) in &flags.engines {
        let tag = canonical_tag(&tag.to_lowercase()).to_string();
        override_engine(&mut cfg, &mut raw, &tag, from_cwd(&cwd, Path::new(path)), ConfigSource::Cli)?;
    }
    validate(&mut cfg, &raw)?;
    Ok(cfg)
}

/// An executable named on the command line or in `V2_ENGINE_*`: with a separator it is a path
/// against the cwd; a bare name is left for the PATH walk (18.1, 17.1).
fn from_cwd(cwd: &Path, v: &Path) -> PathBuf {
    let text = v.to_string_lossy();
    if text.contains(['/', '\\']) || v.is_absolute() {
        cwd.join(v)
    } else {
        v.to_path_buf()
    }
}

/// L3/L4: a built-in engine gets `command = [PATH]`, replacing (and never validating) the
/// files' command; a custom one `command[0] = PATH`; an unknown tag waits in `pending` (18.4).
fn override_engine(cfg: &mut Config, raw: &mut Raw, tag: &str, path: PathBuf, from: ConfigSource) -> Result<(), ConfigError> {
    if tag == "ts" {
        return Err(ConfigError("config error: ts has no executable of its own; use --engine js=PATH or [engines.ts] transpiler".into()));
    }
    let path_text = path.to_string_lossy().into_owned();
    if BUILTIN_TAGS.contains(&tag) {
        raw.shift_remove(&format!("engines.{tag}.command"));
        cfg.engines.entry(tag.to_string()).or_default().command = Some(Sourced { value: vec![path_text], from });
    } else if let Some(custom) = cfg.custom.get_mut(tag) {
        match custom.table.get_mut("command") {
            Some(toml::Value::Array(items)) if !items.is_empty() => items[0] = toml::Value::String(path_text),
            _ => {
                custom.table.insert("command".into(), toml::Value::Array(vec![toml::Value::String(path_text)]));
            }
        }
    } else {
        cfg.pending.retain(|(t, _, _)| t != tag);
        cfg.pending.push((tag.to_string(), path, from));
    }
    Ok(())
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

/// `unknown key [python] pathz (did you mean 'paths'?)`.
fn unknown(place: &str, key: &str, known: &[&str]) -> String {
    match did_you_mean(key, known.iter().copied()) {
        Some(s) => format!("unknown {place} (did you mean '{s}'?)"),
        None => format!("unknown {place}"),
    }
}

/// One config file: parse it, report structural errors and unknown keys, and keep its schema
/// values in `raw` over those of lower layers (18.3, 18.5).
fn read_layer(cfg: &mut Config, raw: &mut Raw, path: &Path, base: PathBuf, from: ConfigSource, user: bool) -> Result<(), ConfigError> {
    let shown = path.display().to_string();
    let bytes = std::fs::read(path).map_err(|e| ConfigError(format!("{shown}: config error: cannot read the file: {}", crate::syntax::span::os_message(&e))))?;
    let text = String::from_utf8(bytes).map_err(|_| ConfigError(format!("{shown}: config error: the file is not valid UTF-8")))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        let message = e.message().trim().to_string();
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
    cfg.files.push(path.to_path_buf());
    let layer = Rc::new(Layer { shown: shown.clone(), base, from });
    let error = |text: String| ConfigError(format!("{shown}: config error: {text}"));
    let mut keep = |key: String, what: String, value: &toml::Value| {
        raw.insert(key, RawValue { what, value: value.clone(), layer: Rc::clone(&layer) });
    };
    let mut warnings = Vec::new();

    for (section, value) in &table {
        let body = match value {
            toml::Value::Table(body) => body,
            v if SECTIONS.contains(&section.as_str()) => {
                return Err(error(format!("[{section}]: expected a table, got {}", describe(v))))
            }
            _ => {
                warnings.push(unknown(&format!("key {section}"), section, SECTIONS));
                continue;
            }
        };
        let mut keys = |known: &[&str], keep: &mut dyn FnMut(String, String, &toml::Value)| {
            for (k, v) in body {
                if known.contains(&k.as_str()) {
                    keep(format!("{section}.{k}"), format!("[{section}] {k}"), v);
                } else {
                    warnings.push(unknown(&format!("key [{section}] {k}"), k, known));
                }
            }
        };
        match section.as_str() {
            "project" if user => warnings.push("[project] is ignored in config.toml".into()),
            "project" => keys(&["name", "entry"], &mut keep),
            "python" => keys(&["venv", "paths"], &mut keep),
            "node" => keys(&["modules", "paths"], &mut keep),
            "ffi" => keys(&["search_paths"], &mut keep),
            "engines" => {
                let mut claimed: IndexMap<String, String> = IndexMap::new();
                for (name, v) in body {
                    let tag = canonical_tag(name).to_string();
                    if let Some(first) = claimed.get(&tag) {
                        return Err(error(format!("[engines.{first}] and [engines.{name}] name the same engine")));
                    }
                    claimed.insert(tag.clone(), name.clone());
                    let toml::Value::Table(t) = v else {
                        return Err(error(format!("[engines.{name}]: expected a table, got {}", describe(v))));
                    };
                    if !BUILTIN_TAGS.contains(&tag.as_str()) {
                        // A custom engine: its whole table, from the highest layer (18.3, 17.2).
                        let base_dir = layer.base.clone();
                        cfg.custom.insert(tag, CustomEngine { table: t.clone(), base_dir, from: layer.from.clone() });
                        continue;
                    }
                    for (k, kv) in t {
                        match k.as_str() {
                            "kind" | "aliases" => {
                                return Err(error(format!("built-in engine '{tag}' can't change 'kind' or 'aliases'")))
                            }
                            "command" if tag == "ts" => {
                                return Err(error(
                                    "ts has no executable of its own; use --engine js=PATH or [engines.ts] transpiler".into(),
                                ))
                            }
                            key if ENGINE_KEYS.contains(&key) => {
                                keep(format!("engines.{tag}.{k}"), format!("[engines.{name}] {k}"), kv)
                            }
                            other => {
                                // [engines.ts] takes only `transpiler` (18.2): never suggest the others.
                                let known = if tag == "ts" { &["transpiler"][..] } else { ENGINE_KEYS };
                                warnings.push(unknown(&format!("key [engines.{name}] {other}"), other, known))
                            }
                        }
                    }
                }
            }
            other => warnings.push(unknown(&format!("section [{other}]"), other, SECTIONS)),
        }
    }
    cfg.warnings.extend(warnings.into_iter().map(|w| format!("warning: {shown}: {w}")));
    Ok(())
}

impl RawValue {
    fn error(&self, text: String) -> ConfigError {
        ConfigError(format!("{}: config error: {}: {text}", self.layer.shown, self.what))
    }

    fn expected<T>(&self, what: &str, v: &toml::Value) -> Result<T, ConfigError> {
        Err(self.error(format!("expected {what}, got {}", describe(v))))
    }

    fn warn(&self, cfg: &mut Config, text: &str) {
        cfg.warnings.push(format!("warning: {}: {} {text}", self.layer.shown, self.what));
    }

    fn sourced<T>(&self, value: T) -> Option<Sourced<T>> {
        Some(Sourced { value, from: self.layer.from.clone() })
    }

    fn string_of(&self, v: &toml::Value) -> Result<String, ConfigError> {
        match v {
            toml::Value::String(s) => Ok(s.clone()),
            other => self.expected("a string", other),
        }
    }

    /// A Windows drive-relative path (`C:main.v2`) would resolve against that drive's cwd, so
    /// it is an error (18.1).
    fn not_drive_relative(&self, text: &str) -> Result<(), ConfigError> {
        let p = Path::new(text);
        if matches!(p.components().next(), Some(Component::Prefix(_))) && !p.has_root() {
            return Err(self.error(format!("'{text}' is a drive-relative path: write it relative to the file, or absolute")));
        }
        Ok(())
    }

    /// A path: relative ones resolve against the declaring file's folder (18.1).
    fn path_of(&self, v: &toml::Value) -> Result<PathBuf, ConfigError> {
        let text = self.string_of(v)?;
        self.not_drive_relative(&text)?;
        Ok(self.layer.base.join(text))
    }

    fn path(&self) -> Result<PathBuf, ConfigError> {
        self.path_of(&self.value)
    }

    fn paths(&self) -> Result<Vec<PathBuf>, ConfigError> {
        match &self.value {
            toml::Value::Array(items) => items.iter().map(|i| self.path_of(i)).collect(),
            other => self.expected("an array of paths", other),
        }
    }

    /// An array of strings; `single` also takes a string as one element, never split (18.2).
    fn strings(&self, single: bool, non_empty: bool) -> Result<Vec<String>, ConfigError> {
        let list = match &self.value {
            toml::Value::String(s) if single => vec![s.clone()],
            toml::Value::Array(items) => items.iter().map(|i| self.string_of(i)).collect::<Result<_, _>>()?,
            other => return self.expected("an array of strings", other),
        };
        if non_empty && (list.is_empty() || list.iter().any(String::is_empty)) {
            return Err(self.error("expected a non-empty array of non-empty strings".into()));
        }
        Ok(list)
    }

    fn seconds(&self) -> Result<f64, ConfigError> {
        let x = match &self.value {
            toml::Value::Integer(n) => *n as f64,
            toml::Value::Float(x) => *x,
            other => return self.expected("a non-negative number", other),
        };
        if !x.is_finite() || x < 0.0 {
            return self.expected("a non-negative number", &self.value);
        }
        Ok(x)
    }

    /// Folders that don't exist are a warning (18.5).
    fn dirs(&self, cfg: &mut Config) -> Result<Option<Sourced<Vec<PathBuf>>>, ConfigError> {
        let dirs = self.paths()?;
        for d in dirs.iter().filter(|d| !d.is_dir()) {
            cfg.warnings.push(format!("warning: {}: {}: folder not found: {}", self.layer.shown, self.what, d.display()));
        }
        Ok(self.sourced(dirs))
    }

    /// 18.1: `argv[0]` with a separator is joined to the declaring folder (without one it is left
    /// for the PATH walk); with `all`, so are later elements that are relative, contain a
    /// separator, don't start with `-` and contain no `{` (D38).
    fn argv(&self, mut argv: Vec<String>, all: bool) -> Result<Vec<String>, ConfigError> {
        for (i, a) in argv.iter_mut().enumerate() {
            let path_like = a.contains(['/', '\\']);
            let relative = !Path::new(a.as_str()).has_root();
            if i == 0 || (all && path_like && relative && !a.starts_with('-') && !a.contains('{')) {
                self.not_drive_relative(a)?;
                if path_like && Path::new(a.as_str()).is_relative() {
                    *a = self.layer.base.join(a.as_str()).to_string_lossy().into_owned();
                }
            }
        }
        Ok(argv)
    }
}

/// Type-check the winning values and fill the typed fields (18.2, 18.3, 18.5).
fn validate(cfg: &mut Config, raw: &Raw) -> Result<(), ConfigError> {
    for (key, rv) in raw {
        let (section, name) = key.split_once('.').expect("raw keys are section.key");
        match (section, name) {
            ("project", "name") => cfg.project_name = rv.sourced(rv.string_of(&rv.value)?),
            ("project", _) => cfg.entry = rv.sourced(rv.path()?),
            ("python", "venv") => cfg.python_venv = rv.sourced(rv.path()?),
            ("python", _) => cfg.python_paths = rv.dirs(cfg)?,
            ("node", "modules") => cfg.node_modules = rv.sourced(rv.path()?),
            ("node", _) => cfg.node_paths = rv.dirs(cfg)?,
            ("ffi", _) => cfg.ffi_search_paths = rv.dirs(cfg)?,
            (_, name) => {
                let (tag, k) = name.split_once('.').expect("engine keys are engines.tag.key");
                match k {
                    "command" => {
                        let argv = rv.argv(rv.strings(true, true)?, true)?;
                        cfg.engines.entry(tag.into()).or_default().command = rv.sourced(argv);
                    }
                    "flags" => {
                        let list = rv.strings(false, false)?;
                        if !matches!(tag, "c" | "cpp" | "rust") {
                            rv.warn(cfg, "only applies to c, cpp and rust");
                        }
                        cfg.engines.entry(tag.into()).or_default().flags = rv.sourced(list);
                    }
                    "transpiler" => {
                        let argv = rv.strings(true, true)?;
                        let argv = if argv == ["node"] { argv } else { rv.argv(argv, false)? };
                        if tag != "ts" {
                            rv.warn(cfg, "only applies to ts");
                        }
                        cfg.engines.entry(tag.into()).or_default().transpiler = rv.sourced(argv);
                    }
                    _ => {
                        let x = rv.seconds()?;
                        if tag == "ts" {
                            cfg.warnings.push(format!(
                                "warning: {}: {}: ts runs in the js session; set it in [engines.js]",
                                rv.layer.shown, rv.what
                            ));
                        } else if k == "startup_timeout" && !matches!(tag, "py" | "js") {
                            rv.warn(cfg, "only applies to py and js");
                        }
                        let e = cfg.engines.entry(tag.into()).or_default();
                        if k == "startup_timeout" {
                            e.startup_timeout = rv.sourced(x);
                        } else {
                            e.call_timeout = rv.sourced(x);
                        }
                    }
                }
            }
        }
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

    /// Load from `start` with an empty home folder (unless given) and no environment overrides.
    fn load_at(start: &Path, flags: &Flags, home: Option<PathBuf>) -> Result<Config, ConfigError> {
        load_from(start, flags, home, std::iter::empty())
    }

    /// A project holding `v2.toml`, loaded from its `src` folder.
    fn project(name: &str, toml_text: &str) -> Result<Config, ConfigError> {
        let d = temp(name);
        std::fs::write(d.join("v2.toml"), toml_text).unwrap();
        let sub = d.join("src");
        std::fs::create_dir_all(&sub).unwrap();
        let home = d.join("home");
        std::fs::create_dir_all(&home).unwrap();
        load_at(&sub, &Flags::default(), Some(home))
    }

    /// A project plus a user layer `~/.v2/config.toml`.
    fn with_user(name: &str, user_toml: &str, project_toml: &str, flags: &Flags) -> Result<Config, ConfigError> {
        let d = temp(name);
        std::fs::write(d.join("v2.toml"), project_toml).unwrap();
        let home = d.join("home");
        std::fs::create_dir_all(home.join(".v2")).unwrap();
        std::fs::write(home.join(".v2").join("config.toml"), user_toml).unwrap();
        load_at(&d, flags, Some(home))
    }

    #[test]
    fn finds_the_root_and_reads_values() {
        let cfg = project("ok", "[project]\nname = \"demo\"\nentry = \"main.v2\"\n[engines.python]\ncommand = \"py.exe\"\ncall_timeout = 5\n[engines.c]\nflags = [\"-O2\"]\n[engines.ruby]\nkind = \"run\"\ncommand = [\"ruby\", \"{file}\"]\n").unwrap();
        assert!(cfg.root.join("v2.toml").is_file());
        assert_eq!(cfg.project_name.unwrap().value, "demo");
        assert_eq!(cfg.entry.unwrap().value, cfg.root.join("main.v2"));
        assert_eq!(cfg.engines["py"].command.as_ref().unwrap().value, vec!["py.exe"]);
        assert_eq!(cfg.engines["py"].call_timeout.as_ref().unwrap().value, 5.0);
        assert_eq!(cfg.engines["c"].flags.as_ref().unwrap().value, vec!["-O2"]);
        assert!(cfg.custom.contains_key("ruby"));
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
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
        // Errors and warnings follow the file's order, not the keys' alphabetical order.
        let e = project("order", "[python]\nvenv = 5\n[engines.py]\ncall_timeout = \"x\"\n").unwrap_err().0;
        assert!(e.ends_with("[python] venv: expected a string, got integer 5"), "{e}");
        let e = project("tscmd", "[engines.ts]\ncommand = \"tsc\"\n").unwrap_err().0;
        assert!(e.ends_with("ts has no executable of its own; use --engine js=PATH or [engines.ts] transpiler"), "{e}");
        let e = project("kind", "[engines.py]\nkind = \"run\"\n").unwrap_err().0;
        assert!(e.ends_with("built-in engine 'py' can't change 'kind' or 'aliases'"), "{e}");
        let e = project("empty", "[engines.py]\ncommand = []\n").unwrap_err().0;
        assert!(e.ends_with("[engines.py] command: expected a non-empty array of non-empty strings"), "{e}");
        let e = project("section", "engines = 1\n").unwrap_err().0;
        assert!(e.ends_with("v2.toml: config error: [engines]: expected a table, got integer 1"), "{e}");
    }

    #[test]
    fn flags_take_only_an_array() {
        // 18.2: only `command` (and `transpiler`) take a string as one element.
        let e = project("flagstr", "[engines.c]\nflags = \"-O2 -Wall\"\n").unwrap_err().0;
        assert!(e.ends_with("[engines.c] flags: expected an array of strings, got string \"-O2 -Wall\""), "{e}");
    }

    #[test]
    fn warnings_for_unknown_and_inapplicable_keys() {
        let cfg = project("warn", "foo = \"x\"\npythn = 1\n[engines.py]\ncomand = \"x\"\nflags = []\n[engines.ts]\ncall_timeout = 1\n[pyton]\n[python]\npaths = [\"nope\"]\n").unwrap();
        let w = cfg.warnings.join("\n");
        assert!(w.contains("unknown key [engines.py] comand (did you mean 'command'?)"), "{w}");
        assert!(w.contains("[engines.py] flags only applies to c, cpp and rust"), "{w}");
        assert!(w.contains("ts runs in the js session; set it in [engines.js]"), "{w}");
        assert!(w.contains("unknown section [pyton] (did you mean 'python'?)"), "{w}");
        assert!(w.contains("[python] paths: folder not found:"), "{w}");
        // A top-level key that is not a section is an unknown key, not an error (18.5).
        assert!(w.contains("v2.toml: unknown key foo\n"), "{w}");
        assert!(w.contains("unknown key pythn (did you mean 'python'?)"), "{w}");
        // [engines.ts] takes only `transpiler`, so `command` is never suggested there.
        let cfg = project("warnts", "[engines.ts]\ncomand = \"x\"\ntranspilr = \"y\"\n").unwrap();
        let w = cfg.warnings.join("\n") + "\n";
        assert!(w.contains("unknown key [engines.ts] comand\n"), "{w}");
        assert!(w.contains("unknown key [engines.ts] transpilr (did you mean 'transpiler'?)"), "{w}");
    }

    #[test]
    fn validation_runs_on_the_merged_result() {
        // 18.3: a bad value that a higher layer replaces is never validated.
        let cfg = with_user("merged", "[engines.py]\nstartup_timeout = \"30s\"\n", "[engines.py]\nstartup_timeout = 10\n", &Flags::default()).unwrap();
        let t = cfg.engines["py"].startup_timeout.as_ref().unwrap();
        assert_eq!(t.value, 10.0);
        assert!(matches!(t.from, ConfigSource::Project(_)));
        // Only the winning array's folders are checked.
        let d = temp("mergeddirs");
        std::fs::create_dir_all(d.join("lib2")).unwrap();
        std::fs::write(d.join("v2.toml"), "[python]\npaths = [\"lib2\"]\n").unwrap();
        let home = d.join("home");
        std::fs::create_dir_all(home.join(".v2")).unwrap();
        std::fs::write(home.join(".v2").join("config.toml"), "[python]\npaths = [\"nolib1\"]\n").unwrap();
        let cfg = load_at(&d, &Flags::default(), Some(home)).unwrap();
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
        assert_eq!(cfg.python_paths.unwrap().value, vec![d.join("lib2")]);
        // An --engine override replaces a bad command from the files.
        let flags = Flags { engines: vec![("py".into(), "python".into())], no_cache: false };
        let cfg = with_user("mergedcli", "", "[engines.py]\ncommand = []\n", &flags).unwrap();
        assert_eq!(cfg.engines["py"].command.as_ref().unwrap().value, vec!["python"]);
        // The winning bad value is still an error, naming its own file.
        let e = with_user("mergedbad", "[engines.js]\ncall_timeout = 1\n", "[engines.js]\ncall_timeout = \"x\"\n", &Flags::default()).unwrap_err().0;
        assert!(e.contains("v2.toml: config error: [engines.js] call_timeout"), "{e}");
    }

    #[test]
    fn no_home_folder_is_a_warning() {
        // 18.1: the user layer is skipped and check and doctor warn.
        let d = temp("nohome");
        let cfg = load_at(&d, &Flags::default(), None).unwrap();
        assert_eq!(cfg.warnings.len(), 1, "{:?}", cfg.warnings);
        assert!(cfg.warnings[0].starts_with("warning: no home folder ("), "{:?}", cfg.warnings);
    }

    #[test]
    fn engine_commands_resolve_against_the_declaring_folder() {
        // 18.1 and D38: argv[0] with a separator, and later path-like elements.
        let cfg = project("argv", "[engines.py]\ncommand = [\"tools/python\", \"adapters/x.py\", \"-X\", \"-a/b\", \"{file}/x\", \"utf8\"]\n[engines.js]\ncommand = \"node\"\n[engines.ts]\ntranspiler = [\"bin/tsx\", \"lib/x.js\"]\n").unwrap();
        let py = &cfg.engines["py"].command.as_ref().unwrap().value;
        assert_eq!(py[0], cfg.root.join("tools/python").to_string_lossy());
        assert_eq!(py[1], cfg.root.join("adapters/x.py").to_string_lossy());
        assert_eq!(&py[2..], ["-X", "-a/b", "{file}/x", "utf8"]);
        assert_eq!(cfg.engines["js"].command.as_ref().unwrap().value, vec!["node"]);
        // Later elements with a root (Windows switches such as /c) are not paths to rewrite.
        let ts = &cfg.engines["ts"].transpiler.as_ref().unwrap().value;
        assert_eq!(ts, &vec![cfg.root.join("bin/tsx").to_string_lossy().into_owned(), "lib/x.js".to_string()]);
        let switches = project("argvroot", "[engines.py]\ncommand = [\"cmd\", \"/c\", \"python\", \"/tools/x.py\"]\n").unwrap();
        assert_eq!(switches.engines["py"].command.as_ref().unwrap().value, vec!["cmd", "/c", "python", "/tools/x.py"]);
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_relative_path_is_a_config_error() {
        // `C:main.v2` would resolve against drive C's cwd, not the project root.
        let e = project("driverel", "[project]\nentry = \"C:main.v2\"\n").unwrap_err().0;
        assert!(e.ends_with("v2.toml: config error: [project] entry: 'C:main.v2' is a drive-relative path: write it relative to the file, or absolute"), "{e}");
        // Engine commands too: argv[0] with or without a separator, and later path-like elements.
        for (toml_text, what, bad) in [
            ("[engines.py]\ncommand = \"C:python.exe\"\n", "[engines.py] command", "C:python.exe"),
            ("[engines.py]\ncommand = [\"C:tools/python\"]\n", "[engines.py] command", "C:tools/python"),
            ("[engines.py]\ncommand = [\"python\", \"C:adapters/x.py\"]\n", "[engines.py] command", "C:adapters/x.py"),
            ("[engines.ts]\ntranspiler = [\"C:tools/tsx\"]\n", "[engines.ts] transpiler", "C:tools/tsx"),
        ] {
            let e = project("driverelcmd", toml_text).unwrap_err().0;
            assert!(e.ends_with(&format!("config error: {what}: '{bad}' is a drive-relative path: write it relative to the file, or absolute")), "{e}");
        }
        // A later element that isn't path-like is left alone.
        assert!(project("driverelok", "[engines.py]\ncommand = [\"python\", \"-X\", \"C:x\"]\n").is_ok());
    }

    #[test]
    fn command_line_overrides_win() {
        let d = temp("cli");
        std::fs::write(d.join("v2.toml"), "[engines.py]\ncommand = [\"py\", \"-3\"]\n[engines.ruby]\nkind = \"run\"\ncommand = [\"ruby\", \"{file}\"]\n").unwrap();
        let flags = Flags { engines: vec![("python".into(), "py311".into()), ("ruby".into(), "r3".into()), ("lua".into(), "lua5".into())], no_cache: true };
        let cfg = load_at(&d, &flags, Some(d.clone())).unwrap();
        let py = &cfg.engines["py"].command.as_ref().unwrap();
        assert_eq!((py.value.clone(), py.from.clone()), (vec!["py311".to_string()], ConfigSource::Cli));
        let ruby = cfg.custom["ruby"].table["command"].as_array().unwrap();
        assert!(ruby[0].as_str() == Some("r3") && ruby[1].as_str() == Some("{file}"));
        assert_eq!(cfg.pending[0].0, "lua");
        assert!(cfg.no_cache);
        let ts = Flags { engines: vec![("typescript".into(), "x".into())], no_cache: false };
        assert!(load_at(&d, &ts, None).unwrap_err().0.contains("ts has no executable of its own"));
    }

    #[test]
    fn environment_overrides() {
        // 18.4: the prefix test compares bytes, so a multibyte char across byte 10 is skipped.
        let d = temp("env");
        let env = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect::<Vec<_>>();
        let vars = env(&[("我的环境变量", "x"), ("AБВГДЕЖЗИ", "x"), ("V2_ENGINEé", "x"), ("V2_ENGINE_PYTHON", "bin/py"), ("V2_ENGINE_JS", "")]);
        let cfg = load_from(&d, &Flags::default(), Some(d.clone()), vars.into_iter()).unwrap();
        let py = cfg.engines["py"].command.as_ref().unwrap();
        assert_eq!(py.value, vec![std::env::current_dir().unwrap().join("bin/py").to_string_lossy().into_owned()]);
        assert_eq!(py.from, ConfigSource::Env("V2_ENGINE_PYTHON".into()));
        assert!(!cfg.engines.contains_key("js"), "an empty variable is unset");
        let vars = env(&[("V2_ENGINE_PY", "a"), ("V2_ENGINE_PYTHON", "b")]);
        let e = load_from(&d, &Flags::default(), Some(d.clone()), vars.into_iter()).unwrap_err().0;
        assert_eq!(e, "config error: V2_ENGINE_PY and V2_ENGINE_PYTHON both set the py engine");
    }
}
