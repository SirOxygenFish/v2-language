//! Finding executables, version probes and the probe cache (INTERNALS 7.5-7.8).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use serde_json::{json, Map, Value as Json};
use sha2::{Digest, Sha256};

use super::process::{self, ExitInfo};
use super::spec::{install_hint, EngineSpec, Version};
use crate::config::{Config, ConfigSource};

/// How long a version probe may run (7.7).
pub const PROBE_LIMIT: Duration = Duration::from_secs(10);

/// The only extensions `CreateProcess` can start (7.5).
const ACCEPTED: [&str; 4] = [".com", ".exe", ".bat", ".cmd"];

/// Where executables are looked up; the tests inject their own (7.5).
#[derive(Clone, Debug, Default)]
pub struct Lookup {
    pub path: Option<OsString>,
    pub pathext: Option<OsString>,
    /// `%LOCALAPPDATA%\Microsoft\WindowsApps`: hits there are App Execution Aliases (7.6).
    pub windows_apps: Option<PathBuf>,
    /// The base of a name with a separator (18.1: already absolute when it came from config).
    pub cwd: PathBuf,
    /// macOS: `xcrun` shims that open an install dialog when the Command Line Tools are
    /// missing, so `xcode-select -p` is asked first (7.6).
    pub xcrun_shims: Vec<PathBuf>,
}

impl Lookup {
    pub fn from_env() -> Lookup {
        let windows_apps = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Microsoft").join("WindowsApps"))
        } else {
            None
        };
        Lookup {
            path: std::env::var_os("PATH"),
            pathext: std::env::var_os("PATHEXT"),
            windows_apps,
            cwd: std::env::current_dir().unwrap_or_default(),
            xcrun_shims: if cfg!(target_os = "macos") { vec![PathBuf::from("/usr/bin/python3")] } else { Vec::new() },
        }
    }

    fn is_alias(&self, path: &Path) -> bool {
        let Some(apps) = &self.windows_apps else { return false };
        let (p, a) = (path.to_string_lossy().to_lowercase(), apps.to_string_lossy().to_lowercase());
        p.starts_with(&a) && p[a.len()..].starts_with(['\\', '/'])
    }

    /// `PATHEXT` entries that `CreateProcess` can start, in order (unset: `.COM;.EXE;.BAT;.CMD`).
    fn extensions(&self) -> Vec<String> {
        let raw = self.pathext.as_ref().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.trim().is_empty());
        let raw = raw.unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
        let mut out: Vec<String> = Vec::new();
        for e in raw.split(';').map(|e| e.trim().to_lowercase()) {
            if ACCEPTED.contains(&e.as_str()) && !out.contains(&e) {
                out.push(e);
            }
        }
        out
    }
}

/// A regular file: `symlink_metadata` when `metadata` fails, as for WindowsApps aliases (7.5).
fn is_regular_file(p: &Path) -> bool {
    match std::fs::metadata(p) {
        Ok(m) => m.is_file(),
        Err(_) => std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file()),
    }
}

#[cfg(unix)]
fn executable(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return false };
    // SAFETY: a NUL-terminated path.
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

#[cfg(not(unix))]
fn executable(_: &Path) -> bool {
    true
}

fn has_accepted_extension(name: &Path) -> bool {
    name.extension().is_some_and(|e| ACCEPTED.contains(&format!(".{}", e.to_string_lossy().to_lowercase()).as_str()))
}

fn with_extension_appended(p: &Path, ext: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(ext);
    PathBuf::from(s)
}

/// Windows `C:tool`: relative to that drive's own current folder, so neither a `PATH` name nor
/// a file under the base, and never found (7.5).
pub fn is_drive_relative(p: &Path) -> bool {
    matches!(p.components().next(), Some(std::path::Component::Prefix(_))) && !p.has_root()
}

/// Every hit for `name`, in `PATH` order, so the search can go on past a skipped one (7.5).
/// With a separator, `name` is joined to `l.cwd`; otherwise each absolute `PATH` entry is
/// tried (never the cwd).
pub fn find_program(name: &str, l: &Lookup) -> Vec<PathBuf> {
    let mut hits: Vec<PathBuf> = Vec::new();
    // `X_OK` only on the PATH walk: a named file that can't run fails its probe with the OS
    // message instead of looking missing (7.5).
    let mut consider = |p: PathBuf, walk: bool| {
        if is_regular_file(&p) && (!walk || executable(&p)) && !hits.contains(&p) {
            hits.push(p);
        }
    };
    let as_path = Path::new(name);
    if is_drive_relative(as_path) {
        return hits;
    }
    if name.contains(['/', '\\']) {
        let p = l.cwd.join(as_path);
        if cfg!(windows) && !has_accepted_extension(as_path) {
            for ext in ACCEPTED {
                consider(with_extension_appended(&p, ext), false);
            }
        } else {
            consider(p, false);
        }
        return hits;
    }
    let path = l.path.clone().unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() || dir.is_relative() {
            continue;
        }
        if cfg!(windows) {
            if has_accepted_extension(as_path) {
                consider(dir.join(name), true);
            }
            for ext in l.extensions() {
                consider(with_extension_appended(&dir.join(name), &ext), true);
            }
        } else {
            consider(dir.join(name), true);
        }
    }
    hits
}

// ---------------------------------------------------------------------------------------------
// Probes (7.7)

/// What running a probe gave.
#[derive(Clone, Debug)]
pub enum ProbeOutcome {
    Exited { code: Option<i32>, output: String },
    /// Killed by this signal (Unix).
    Killed(i32),
    TimedOut,
    SpawnFailed(String),
}

/// Runs version probes; the tests inject fakes.
pub trait Prober {
    fn probe(&self, argv: &[OsString], env: &[(OsString, OsString)]) -> ProbeOutcome;
}

/// Runs the probe under 8.3's child rules with a 10 s timer; stdout then stderr, lossy UTF-8.
pub struct RealProber;

impl Prober for RealProber {
    fn probe(&self, argv: &[OsString], env: &[(OsString, OsString)]) -> ProbeOutcome {
        match process::run_captured(argv, env, Some(PROBE_LIMIT), None) {
            Err(e) => ProbeOutcome::SpawnFailed(e.message),
            Ok(c) if c.timed_out => ProbeOutcome::TimedOut,
            Ok(c) => match c.status.signal {
                Some(sig) => ProbeOutcome::Killed(sig),
                None => {
                    let mut output = String::from_utf8_lossy(&c.stdout).into_owned();
                    output.push_str(&String::from_utf8_lossy(&c.stderr));
                    ProbeOutcome::Exited { code: c.status.code, output }
                }
            },
        }
    }
}

/// What a probe says about one hit (7.6).
#[derive(Clone, Debug, PartialEq)]
enum Judged {
    Usable(Version),
    TooOld(Version, String),
    Skipped(String),
}

impl Judged {
    fn reason(&self) -> Option<&str> {
        match self {
            Judged::Usable(_) => None,
            Judged::TooOld(_, r) | Judged::Skipped(r) => Some(r),
        }
    }
}

/// The skip reason of the Store's fake `python.exe` (7.6): the one skip the cache remembers (7.7).
const PLACEHOLDER: &str = "Microsoft Store placeholder";

fn judge(spec: &EngineSpec, alias: bool, outcome: &ProbeOutcome) -> Judged {
    match outcome {
        ProbeOutcome::SpawnFailed(msg) => Judged::Skipped(msg.clone()),
        ProbeOutcome::TimedOut => Judged::Skipped(format!("version probe timed out ({} s)", PROBE_LIMIT.as_secs())),
        ProbeOutcome::Killed(sig) => Judged::Skipped(format!("version probe failed ({})", ExitInfo { code: None, signal: Some(*sig) }.describe())),
        ProbeOutcome::Exited { code, output } => {
            // The placeholder prints "Python was not found" and exits 9009 (7.6).
            if alias && (*code == Some(9009) || output.contains("Python was not found")) {
                return Judged::Skipped(PLACEHOLDER.into());
            }
            if *code != Some(0) {
                return Judged::Skipped(format!("version probe failed ({})", ExitInfo { code: *code, signal: None }.describe()));
            }
            let Some((args, pattern)) = &spec.probe else { return Judged::Skipped("no version probe".into()) };
            let Some(version) = pattern.find(output) else {
                return Judged::Skipped(format!("no version in its {} output", args.join(" ")));
            };
            match spec.too_old(version) {
                Some(why) => Judged::TooOld(version, why),
                None => Judged::Usable(version),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The probe cache (7.7)

/// How a run uses `~/.v2/cache/toolchains.json` (7.7; doctor 19.2; no home folder 18.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CacheMode {
    /// Runs: read, and write misses.
    ReadWrite,
    /// Doctor: probe afresh and overwrite.
    Refresh,
    /// No home folder: no cache at all.
    Off,
}

pub struct ProbeCache {
    file: Option<PathBuf>,
    mode: CacheMode,
    entries: Map<String, Json>,
    dirty: bool,
}

impl ProbeCache {
    /// The cache under `home`; an unreadable file or an unknown `format` counts as empty.
    pub fn open(home: Option<&Path>, mode: CacheMode) -> ProbeCache {
        let file = home.filter(|_| mode != CacheMode::Off).map(|h| h.join(".v2").join("cache").join("toolchains.json"));
        let entries = file.as_deref().and_then(read_cache).unwrap_or_default();
        ProbeCache { file, mode, entries, dirty: false }
    }

    /// A cache that neither reads nor writes.
    #[cfg(test)]
    pub fn off() -> ProbeCache {
        ProbeCache::open(None, CacheMode::Off)
    }

    /// A cached usable version, or the Store placeholder. A stale version is caught by `hello`'s
    /// version check (8.4); a placeholder's key changes with its alias file.
    fn get(&self, key: &str) -> Option<Judged> {
        if self.mode != CacheMode::ReadWrite {
            return None;
        }
        let e = self.entries.get(key)?;
        if e.get("placeholder").and_then(Json::as_bool) == Some(true) {
            return Some(Judged::Skipped(PLACEHOLDER.into()));
        }
        if e.get("ok").and_then(Json::as_bool) != Some(true) {
            return None;
        }
        e.get("version").and_then(Json::as_str).and_then(Version::parse).map(Judged::Usable)
    }

    /// The cache holds usable entries and the Store placeholder (7.7): any other skipped hit is
    /// probed again by every run, because a version-manager shim (pyenv, asdf, volta) runs another
    /// interpreter in another project, and a timeout may have been passing. A skip drops an entry
    /// doctor re-probed.
    fn put(&mut self, key: String, path: &Path, j: &Judged, output: &str) {
        if self.mode == CacheMode::Off {
            return;
        }
        if matches!(j, Judged::Skipped(reason) if reason == PLACEHOLDER) {
            self.entries.insert(key, json!({"path": path.to_string_lossy(), "placeholder": true}));
            self.dirty = true;
            return;
        }
        let Judged::Usable(version) = j else {
            self.dirty |= self.entries.shift_remove(&key).is_some();
            return;
        };
        let entry = json!({
            "path": path.to_string_lossy(),
            "ok": true,
            "version": version.to_string(),
            "output": output,
        });
        self.entries.insert(key, entry);
        self.dirty = true;
    }

    /// Write the file if anything changed: a temp file, then a rename (a failed rename is ignored).
    pub fn save(&mut self) {
        let Some(file) = &self.file else { return };
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let Some(dir) = file.parent() else { return };
        if create_private_dir(dir).is_err() {
            return;
        }
        let text = json!({"format": 1, "entries": Json::Object(self.entries.clone())}).to_string();
        let tmp = dir.join(format!("toolchains.json.{}.tmp", std::process::id()));
        if std::fs::write(&tmp, text).is_err() || std::fs::rename(&tmp, file).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

fn read_cache(file: &Path) -> Option<Map<String, Json>> {
    let v: Json = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    if v.get("format").and_then(Json::as_u64) != Some(1) {
        return None;
    }
    v.get("entries")?.as_object().cloned()
}

/// `~/.v2/cache`, owner-only on Unix.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

/// sha256 over the argv, the canonical path, size, mtime and the spec hash (7.7). An alias is
/// stat'ed as the link itself.
fn cache_key(spec: &EngineSpec, argv: &[OsString], path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).or_else(|_| std::fs::symlink_metadata(path)).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    let mut h = Sha256::new();
    let mut field = |bytes: &[u8]| {
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    };
    for a in argv {
        field(a.as_encoded_bytes());
    }
    let canonical = std::fs::canonicalize(path).ok();
    field(canonical.as_deref().map_or(&[][..], |c| c.as_os_str().as_encoded_bytes()));
    field(&meta.len().to_le_bytes());
    field(&mtime.to_le_bytes());
    field(&spec_hash(spec));
    Some(hex(&h.finalize()))
}

/// sha256(engine id, probe args, pattern, V2 version): a new V2 or rule re-probes (7.7).
fn spec_hash(spec: &EngineSpec) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(spec.id.as_bytes());
    if let Some((args, pattern)) = &spec.probe {
        for a in args {
            h.update([0]);
            h.update(a.as_bytes());
        }
        h.update([0]);
        h.update(pattern.name());
    }
    h.update([0]);
    h.update(env!("CARGO_PKG_VERSION"));
    h.finalize().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------------------------
// Resolution (7.6, 7.8)

/// Where a resolved engine came from: doctor's FROM column (19.2).
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    Path,
    Venv,
    Config(ConfigSource),
}

impl Origin {
    pub fn label(&self) -> String {
        match self {
            Origin::Path => "PATH".into(),
            Origin::Venv => "venv".into(),
            Origin::Config(from) => source_label(from),
        }
    }
}

/// `v2.toml` or `V2_ENGINE_PY` (19.2's FROM column).
pub fn source_label(from: &ConfigSource) -> String {
    match from {
        ConfigSource::Project(_) => "v2.toml".into(),
        ConfigSource::Env(var) => var.clone(),
    }
}

/// What "(set by …)" names: the file's full path, or the variable (7.6).
fn set_by(from: &ConfigSource) -> String {
    match from {
        ConfigSource::Project(p) => p.display().to_string(),
        ConfigSource::Env(var) => var.clone(),
    }
}

#[derive(Debug)]
pub struct ResolvedEngine {
    pub spec: Rc<EngineSpec>,
    pub origin: Origin,
    /// `[C:\Windows\py.exe, -3]`: the program, absolute, then its prefix arguments. ts: js's.
    pub argv: Vec<OsString>,
    pub version: Option<Version>,
    pub skipped: Vec<(PathBuf, String)>,
}

impl ResolvedEngine {
    /// Doctor's COMMAND column: the argv, space-joined, unquoted (19.2).
    pub fn shown(&self) -> String {
        if self.spec.session_of.is_some() {
            "(js engine, built-in type stripping)".into()
        } else {
            join(&self.argv)
        }
    }
}

fn join(argv: &[OsString]) -> String {
    argv.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
}

/// Doctor's STATUS for an engine that is not usable (19.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    Missing,
    /// Only versions below the minimum (the newest shown).
    Old(Version),
    /// A configured command or venv missing, or a probe that failed.
    Broken,
}

/// Why an engine has no usable toolchain: the `EngineNotFound` message and doctor's row.
#[derive(Clone, Debug)]
pub struct NotFound {
    pub status: Status,
    pub message: String,
    /// Doctor's COMMAND and FROM for a configured value that does not work.
    pub command: Option<String>,
    pub from: Option<String>,
    pub skipped: Vec<(PathBuf, String)>,
}

impl NotFound {
    fn broken(message: String, command: String, from: &ConfigSource) -> NotFound {
        NotFound { status: Status::Broken, message, command: Some(command), from: Some(source_label(from)), skipped: Vec::new() }
    }
}

/// What resolution needs besides the spec: config, lookup, prober and cache.
pub struct Resolver<'a> {
    pub cfg: &'a Config,
    pub lookup: &'a Lookup,
    pub prober: &'a dyn Prober,
    pub cache: &'a mut ProbeCache,
}

impl Resolver<'_> {
    fn env(spec: &EngineSpec) -> Vec<(OsString, OsString)> {
        spec.env.iter().map(|(k, v)| (OsString::from(&**k), OsString::from(&**v))).collect()
    }

    /// Probe one hit, through the cache.
    fn probe(&mut self, spec: &EngineSpec, argv: &[OsString], path: &Path) -> Judged {
        let key = cache_key(spec, argv, path);
        if let Some(j) = key.as_deref().and_then(|k| self.cache.get(k)) {
            return j;
        }
        let mut full = argv.to_vec();
        if let Some((args, _)) = &spec.probe {
            full.extend(args.iter().map(|a| OsString::from(&**a)));
        }
        let outcome = self.prober.probe(&full, &Self::env(spec));
        let judged = judge(spec, self.lookup.is_alias(path), &outcome);
        if let Some(key) = key {
            let output = match &outcome {
                ProbeOutcome::Exited { output, .. } => output.as_str(),
                _ => "",
            };
            self.cache.put(key, path, &judged, output);
        }
        judged
    }

    /// py or js: the configured command, else `[python] venv`, never falling back to
    /// auto-detection; else the candidates (7.6).
    pub fn resolve(&mut self, spec: &Rc<EngineSpec>) -> Result<ResolvedEngine, NotFound> {
        let command = self.cfg.engines.get(&*spec.id).and_then(|e| e.command.as_ref());
        let venv = if &*spec.id == "py" { self.cfg.python_venv.as_ref() } else { None };
        match (command, venv) {
            (Some(c), _) => self.configured(spec, &c.value, &c.from),
            (None, Some(v)) => self.venv(spec, &v.value, &v.from),
            (None, None) => self.search(spec),
        }
    }

    fn usable(&mut self, spec: &Rc<EngineSpec>, argv: Vec<OsString>, origin: Origin, what: &str, from: &ConfigSource) -> Result<ResolvedEngine, NotFound> {
        let path = PathBuf::from(&argv[0]);
        match self.probe(spec, &argv, &path) {
            Judged::Usable(v) => Ok(ResolvedEngine { spec: spec.clone(), origin, argv, version: Some(v), skipped: Vec::new() }),
            other => {
                let reason = other.reason().unwrap_or_default();
                let mut nf = NotFound::broken(
                    format!("engine '{}' {what} (set by {}) is not usable: {reason}", spec.id, set_by(from)),
                    join(&argv),
                    from,
                );
                if let Judged::TooOld(v, _) = other {
                    nf.status = Status::Old(v);
                }
                Err(nf)
            }
        }
    }

    fn configured(&mut self, spec: &Rc<EngineSpec>, command: &[String], from: &ConfigSource) -> Result<ResolvedEngine, NotFound> {
        let name = &command[0];
        let Some(path) = find_program(name, self.lookup).into_iter().next() else {
            return Err(NotFound::broken(
                format!("engine '{}' command '{name}' not found (set by {})", spec.id, set_by(from)),
                command.join(" "),
                from,
            ));
        };
        let mut argv = vec![path.into_os_string()];
        argv.extend(command[1..].iter().map(OsString::from));
        self.usable(spec, argv, Origin::Config(from.clone()), &format!("command '{name}'"), from)
    }

    fn venv(&mut self, spec: &Rc<EngineSpec>, venv: &Path, from: &ConfigSource) -> Result<ResolvedEngine, NotFound> {
        let candidates: Vec<PathBuf> = if cfg!(windows) {
            vec![venv.join("Scripts").join("python.exe")]
        } else {
            vec![venv.join("bin").join("python"), venv.join("bin").join("python3")]
        };
        let Some(exe) = candidates.iter().find(|p| is_regular_file(p)) else {
            let want = if cfg!(windows) { r"Scripts\python.exe" } else { "bin/python" };
            return Err(NotFound::broken(
                format!("[python] venv '{}' has no {want} (create it: python -m venv {})", venv.display(), venv.display()),
                candidates[0].display().to_string(),
                from,
            ));
        };
        let what = format!("[python] venv '{}'", venv.display());
        self.usable(spec, vec![exe.clone().into_os_string()], Origin::Venv, &what, from)
    }

    fn search(&mut self, spec: &Rc<EngineSpec>) -> Result<ResolvedEngine, NotFound> {
        let mut skipped: Vec<(PathBuf, String)> = Vec::new();
        let mut newest_old: Option<Version> = None;
        for candidate in &spec.candidates {
            for hit in find_program(&candidate[0], self.lookup) {
                if self.lookup.xcrun_shims.contains(&hit) && !self.command_line_tools() {
                    skipped.push((hit, "Command Line Tools missing".into()));
                    continue;
                }
                let mut argv = vec![hit.clone().into_os_string()];
                argv.extend(candidate[1..].iter().map(|a| OsString::from(&**a)));
                match self.probe(spec, &argv, &hit) {
                    Judged::Usable(v) => {
                        return Ok(ResolvedEngine { spec: spec.clone(), origin: Origin::Path, argv, version: Some(v), skipped })
                    }
                    Judged::TooOld(v, reason) => {
                        newest_old = newest_old.max(Some(v));
                        skipped.push((hit, reason));
                    }
                    Judged::Skipped(reason) => skipped.push((hit, reason)),
                }
            }
        }
        let status = newest_old.map_or(Status::Missing, Status::Old);
        Err(NotFound { status, message: not_found_message(spec, &skipped), command: None, from: None, skipped })
    }

    /// macOS: whether the Command Line Tools are installed, so the `xcrun` shims can run
    /// without opening an install dialog (7.6).
    fn command_line_tools(&self) -> bool {
        let argv = [OsString::from("/usr/bin/xcode-select"), OsString::from("-p")];
        matches!(self.prober.probe(&argv, &[]), ProbeOutcome::Exited { code: Some(0), .. })
    }

    /// ts: the js engine, if its Node has built-in type stripping (7.6).
    pub fn resolve_ts(&mut self, spec: &Rc<EngineSpec>, js: Result<&ResolvedEngine, &NotFound>) -> Result<ResolvedEngine, NotFound> {
        let js = js.map_err(|nf| {
            let rest: Vec<&str> = nf.message.lines().skip(1).collect();
            let mut message = "engine 'ts' (TypeScript) not found — @ts blocks run on the js engine".to_string();
            if rest.is_empty() {
                message.push_str(&format!("\njs: {}", nf.message));
            }
            for line in rest {
                message.push('\n');
                message.push_str(line);
            }
            NotFound { message, skipped: Vec::new(), ..nf.clone() }
        })?;
        Ok(ResolvedEngine { spec: spec.clone(), origin: js.origin.clone(), argv: js.argv.clone(), version: js.version, skipped: Vec::new() })
    }
}

/// 7.8's message: what was tried, what was skipped and why, how to install, how to configure.
fn not_found_message(spec: &EngineSpec, skipped: &[(PathBuf, String)]) -> String {
    let tried: Vec<String> = spec.candidates.iter().map(|c| c.join(" ")).collect();
    let mut m = format!("engine '{}' ({}) not found\ntried: {} on PATH", spec.id, spec.language, tried.join(", "));
    for (path, reason) in skipped {
        m.push_str(&format!("\nskipped: {} ({reason})", path.display()));
    }
    let id = &spec.id;
    m.push_str(&format!("\ninstall: {}\nor set [engines.{id}] command in v2.toml, or V2_ENGINE_{}", install_hint(id), id.to_uppercase()));
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EngineCfg, Sourced};
    use std::cell::{Cell, RefCell};

    /// A fresh folder under the system temp dir.
    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("v2res-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// An "executable" named `name` (`.exe` added on Windows) in `dir`.
    fn exe(dir: &Path, name: &str) -> PathBuf {
        let file = if cfg!(windows) && Path::new(name).extension().is_none() { format!("{name}.exe") } else { name.to_string() };
        let p = dir.join(file);
        std::fs::write(&p, b"").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        p
    }

    /// Answers probes by program path; counts and records them.
    struct Fake {
        answer: Box<dyn Fn(&Path) -> ProbeOutcome>,
        calls: Rc<Cell<usize>>,
        argvs: Rc<RefCell<Vec<Vec<OsString>>>>,
    }

    impl Prober for Fake {
        fn probe(&self, argv: &[OsString], _: &[(OsString, OsString)]) -> ProbeOutcome {
            self.calls.set(self.calls.get() + 1);
            self.argvs.borrow_mut().push(argv.to_vec());
            (self.answer)(Path::new(&argv[0]))
        }
    }

    fn says(text: &str) -> ProbeOutcome {
        ProbeOutcome::Exited { code: Some(0), output: text.into() }
    }

    fn fake(answer: impl Fn(&Path) -> ProbeOutcome + 'static) -> Fake {
        Fake { answer: Box::new(answer), calls: Rc::new(Cell::new(0)), argvs: Rc::new(RefCell::new(Vec::new())) }
    }

    fn lookup(dirs: &[&Path], apps: Option<&Path>) -> Lookup {
        Lookup {
            path: Some(std::env::join_paths(dirs).unwrap()),
            pathext: None,
            windows_apps: apps.map(Path::to_path_buf),
            cwd: std::env::temp_dir(),
            xcrun_shims: Vec::new(),
        }
    }

    fn spec(id: &str) -> Rc<EngineSpec> {
        Rc::new(crate::engines::spec::builtins().into_iter().find(|s| &*s.id == id).unwrap())
    }

    fn resolve_with(cfg: &Config, l: &Lookup, p: &Fake, cache: &mut ProbeCache, id: &str) -> Result<ResolvedEngine, NotFound> {
        Resolver { cfg, lookup: l, prober: p, cache }.resolve(&spec(id))
    }

    fn project_source() -> ConfigSource {
        ConfigSource::Project(PathBuf::from("C:/proj/v2.toml"))
    }

    #[test]
    fn r41_store_placeholder_skipped_next_used() {
        let root = temp("r41a");
        let (apps, real) = (root.join("WindowsApps"), root.join("Python312"));
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        let alias = exe(&apps, "python3");
        let python = exe(&real, "python");
        let apps2 = apps.clone();
        let p = fake(move |prog| {
            if prog.starts_with(&apps2) {
                ProbeOutcome::Exited { code: Some(9009), output: "Python was not found; run without arguments to install from the Microsoft Store".into() }
            } else {
                says("Python 3.12.1")
            }
        });
        let l = lookup(&[&apps, &real], Some(&apps));
        let e = resolve_with(&Config::default(), &l, &p, &mut ProbeCache::off(), "py").expect("the real python");
        assert_eq!(e.argv, [python.into_os_string()]);
        assert_eq!(e.version, Some(Version(3, 12, 1)));
        assert_eq!(e.origin, Origin::Path);
        assert_eq!(e.skipped, [(alias, "Microsoft Store placeholder".to_string())]);
    }

    #[test]
    fn r67_the_placeholder_is_probed_once() {
        let home = temp("r67home");
        let root = temp("r67");
        let (apps, real) = (root.join("WindowsApps"), root.join("Python312"));
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        let alias = exe(&apps, "python3");
        exe(&real, "python");
        let apps2 = apps.clone();
        let p = fake(move |prog| {
            if prog.starts_with(&apps2) {
                ProbeOutcome::Exited { code: Some(9009), output: "Python was not found".into() }
            } else {
                says("Python 3.12.1")
            }
        });
        let l = lookup(&[&apps, &real], Some(&apps));
        let cfg = Config::default();
        for _ in 0..2 {
            let mut c = ProbeCache::open(Some(&home), CacheMode::ReadWrite);
            let e = resolve_with(&cfg, &l, &p, &mut c, "py").expect("the real python");
            assert_eq!(e.skipped, [(alias.clone(), "Microsoft Store placeholder".to_string())]);
            c.save();
        }
        assert_eq!(p.calls.get(), 2, "the placeholder and python, once each, across two runs");
        // doctor probes afresh.
        let mut c = ProbeCache::open(Some(&home), CacheMode::Refresh);
        resolve_with(&cfg, &l, &p, &mut c, "py").expect("the real python");
        assert_eq!(p.calls.get(), 4);
        // Other skips are never remembered (7.7).
        let bin = temp("r67other");
        let other = exe(&bin, "python");
        let broken = fake(|_| ProbeOutcome::Exited { code: Some(1), output: String::new() });
        let l = lookup(&[&bin], None);
        for _ in 0..2 {
            let mut c = ProbeCache::open(Some(&home), CacheMode::ReadWrite);
            assert!(resolve_with(&cfg, &l, &broken, &mut c, "py").is_err());
            c.save();
        }
        assert_eq!(broken.calls.get(), 2, "{}", other.display());
    }

    #[test]
    fn r41_store_alias_with_real_python_kept() {
        let root = temp("r41b");
        let apps = root.join("WindowsApps");
        std::fs::create_dir_all(&apps).unwrap();
        let alias = exe(&apps, "python3");
        let p = fake(|_| says("Python 3.12.1"));
        let l = lookup(&[&apps], Some(&apps));
        let e = resolve_with(&Config::default(), &l, &p, &mut ProbeCache::off(), "py").expect("the alias");
        assert_eq!(e.argv, [alias.into_os_string()]);
        assert!(e.skipped.is_empty());
    }

    #[test]
    fn relative_and_empty_path_entries_skipped() {
        // A relative entry is relative to the cwd, which is never searched (7.5).
        let rel = PathBuf::from("target").join(format!("v2res-rel-{}", std::process::id()));
        std::fs::create_dir_all(&rel).unwrap();
        exe(&rel, "node");
        let abs = temp("relabs");
        let good = exe(&abs, "node");
        let path = std::env::join_paths([PathBuf::new(), PathBuf::from("."), rel.clone(), abs.clone()]).unwrap();
        let l = Lookup { path: Some(path), pathext: None, windows_apps: None, cwd: std::env::current_dir().unwrap(), xcrun_shims: Vec::new() };
        assert_eq!(find_program("node", &l), [good]);
        let _ = std::fs::remove_dir_all(&rel);
    }

    #[cfg(windows)]
    #[test]
    fn pathext_order_and_accepted_extensions() {
        let d = temp("pathext");
        for name in ["tool.cmd", "tool.exe", "tool.py", "tool.js", "tool"] {
            std::fs::write(d.join(name), b"").unwrap();
        }
        let mut l = lookup(&[&d], None);
        l.pathext = Some(".JS;.CMD;.EXE;.PY".into());
        assert_eq!(find_program("tool", &l), [d.join("tool.cmd"), d.join("tool.exe")]);
        // A name that already has an accepted extension is tried as is first.
        assert_eq!(find_program("tool.exe", &l)[0], d.join("tool.exe"));
        assert!(find_program("tool.py", &l).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn requires_x_ok() {
        let d = temp("xok");
        std::fs::write(d.join("node"), b"").unwrap();
        let l = lookup(&[&d], None);
        assert!(find_program("node", &l).is_empty());
        let good = exe(&d, "node");
        assert_eq!(find_program("node", &l), [good]);
    }

    #[test]
    fn too_old_skipped_search_continues() {
        let root = temp("old");
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let old = exe(&a, "python3");
        let new = exe(&b, "python3");
        let a2 = a.clone();
        let p = fake(move |prog| says(if prog.starts_with(&a2) { "Python 3.6.8" } else { "Python 3.11.6" }));
        let l = lookup(&[&a, &b], None);
        let e = resolve_with(&Config::default(), &l, &p, &mut ProbeCache::off(), "py").unwrap();
        assert_eq!(e.argv, [new.into_os_string()]);
        assert_eq!(e.skipped, [(old.clone(), "too old: 3.6.8 < 3.10".to_string())]);
        // Only old ones: status Old with the newest version.
        exe(&a, "python");
        let p = fake(|prog| says(if prog.to_string_lossy().contains("python3") { "Python 3.7.9" } else { "Python 3.6.8" }));
        let nf = resolve_with(&Config::default(), &lookup(&[&a], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.status, Status::Old(Version(3, 7, 9)));
        assert!(nf.message.contains("too old: 3.7.9 < 3.10") && nf.message.contains("too old: 3.6.8 < 3.10"), "{}", nf.message);
    }

    #[test]
    fn probe_failures_are_skips() {
        let d = temp("fail");
        exe(&d, "node");
        for (outcome, reason) in [
            (ProbeOutcome::TimedOut, "version probe timed out (10 s)"),
            (ProbeOutcome::SpawnFailed("could not start 'x': boom".into()), "could not start 'x': boom"),
            (ProbeOutcome::Exited { code: Some(1), output: String::new() }, "version probe failed (exit code 1)"),
            (says("node, but no version"), "no version in its --version output"),
        ] {
            let p = fake(move |_| outcome.clone());
            let nf = resolve_with(&Config::default(), &lookup(&[&d], None), &p, &mut ProbeCache::off(), "js").unwrap_err();
            assert_eq!(nf.status, Status::Missing);
            assert_eq!(nf.skipped[0].1, reason);
        }
    }

    fn with_command(id: &str, argv: &[&str], from: ConfigSource) -> Config {
        let mut cfg = Config::default();
        let command = Some(Sourced { value: argv.iter().map(|s| s.to_string()).collect(), from });
        cfg.engines.insert(id.into(), EngineCfg { command, ..EngineCfg::default() });
        cfg
    }

    #[test]
    fn configured_command_missing_no_fallback() {
        let d = temp("cfgmiss");
        exe(&d, "python3");
        let missing = d.join("nowhere").join("python.exe");
        let cfg = with_command("py", &[&missing.to_string_lossy()], project_source());
        let p = fake(|_| says("Python 3.12.1"));
        let nf = resolve_with(&cfg, &lookup(&[&d], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.status, Status::Broken);
        assert_eq!(nf.message, format!("engine 'py' command '{}' not found (set by C:/proj/v2.toml)", missing.display()));
        assert_eq!(nf.from.as_deref(), Some("v2.toml"));
        assert_eq!(p.calls.get(), 0, "no fallback to the PATH python");
        // A bare configured name is looked up on PATH; prefix arguments are kept.
        let cfg = with_command("py", &["python3", "-X", "utf8"], ConfigSource::Env("V2_ENGINE_PY".into()));
        let e = resolve_with(&cfg, &lookup(&[&d], None), &p, &mut ProbeCache::off(), "py").unwrap();
        assert_eq!(e.argv[1..], [OsString::from("-X"), OsString::from("utf8")]);
        assert_eq!(e.origin.label(), "V2_ENGINE_PY");
        assert_eq!(p.argvs.borrow()[0][1..], [OsString::from("-X"), OsString::from("utf8"), OsString::from("--version")]);
    }

    #[test]
    fn configured_probe_failure_is_error() {
        let d = temp("cfgfail");
        let node = exe(&d, "node");
        let cfg = with_command("js", &[&node.to_string_lossy()], ConfigSource::Env("V2_ENGINE_JS".into()));
        let p = fake(|_| ProbeOutcome::Exited { code: Some(1), output: String::new() });
        let nf = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "js").unwrap_err();
        assert_eq!(nf.status, Status::Broken);
        assert_eq!(
            nf.message,
            format!("engine 'js' command '{}' (set by V2_ENGINE_JS) is not usable: version probe failed (exit code 1)", node.display())
        );
        let p = fake(|_| says("v16.20.0"));
        let nf = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "js").unwrap_err();
        assert_eq!(nf.status, Status::Old(Version(16, 20, 0)));
    }

    #[test]
    fn venv_interpreter_and_precedence() {
        let d = temp("venv");
        let venv = d.join(".venv");
        let missing = Config { python_venv: Some(Sourced { value: venv.clone(), from: project_source() }), ..Config::default() };
        let p = fake(|_| says("Python 3.12.1"));
        let nf = resolve_with(&missing, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        let want = if cfg!(windows) { r"Scripts\python.exe" } else { "bin/python" };
        assert!(nf.message.starts_with(&format!("[python] venv '{}' has no {want} (create it: python -m venv ", venv.display())), "{}", nf.message);
        let bin = venv.join(if cfg!(windows) { "Scripts" } else { "bin" });
        std::fs::create_dir_all(&bin).unwrap();
        let python = exe(&bin, "python");
        let e = resolve_with(&missing, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap();
        assert_eq!((e.argv, e.origin), (vec![python.into_os_string()], Origin::Venv));
        // A configured command wins over the venv, from v2.toml or V2_ENGINE_PY (7.6).
        let other = exe(&d, "python3");
        for command_from in [project_source(), ConfigSource::Env("V2_ENGINE_PY".into())] {
            let mut cfg = with_command("py", &[&other.to_string_lossy()], command_from);
            cfg.python_venv = Some(Sourced { value: venv.clone(), from: project_source() });
            let e = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap();
            assert_ne!(e.origin, Origin::Venv);
        }
    }

    #[cfg(windows)]
    #[test]
    fn py_launcher_last() {
        let d = temp("pylauncher");
        let py = exe(&d, "py");
        let p = fake(|_| says("Python 3.12.1"));
        let e = resolve_with(&Config::default(), &lookup(&[&d], None), &p, &mut ProbeCache::off(), "py").unwrap();
        assert_eq!(e.argv, [py.into_os_string(), OsString::from("-3")]);
        assert_eq!(e.shown(), format!("{} -3", d.join("py.exe").display()));
    }

    #[test]
    fn ts_follows_js() {
        let d = temp("ts");
        exe(&d, "node");
        let l = lookup(&[&d], None);
        let mut cache = ProbeCache::off();
        let cfg = Config::default();
        // A Node without type stripping is too old for js too, so ts never sees one (7.2).
        let p = fake(|_| says("v22.6.0"));
        let mut r = Resolver { cfg: &cfg, lookup: &l, prober: &p, cache: &mut cache };
        let js = r.resolve(&spec("js"));
        assert_eq!(js.as_ref().unwrap_err().status, Status::Old(Version(22, 6, 0)));
        let nf = r.resolve_ts(&spec("ts"), js.as_ref()).unwrap_err();
        assert!(nf.message.starts_with("engine 'ts' (TypeScript) not found — @ts blocks run on the js engine\n"), "{}", nf.message);
        assert!(nf.message.contains("too old: 22.6.0 < 22.13"), "{}", nf.message);
        let p = fake(|_| says("v24.11.1"));
        let mut r = Resolver { cfg: &cfg, lookup: &l, prober: &p, cache: &mut cache };
        let js = r.resolve(&spec("js")).unwrap();
        let ts = r.resolve_ts(&spec("ts"), Ok(&js)).unwrap();
        assert_eq!((ts.shown(), ts.version, &ts.argv), ("(js engine, built-in type stripping)".to_string(), js.version, &js.argv));
        assert_eq!(js.shown(), join(&js.argv));
        // Without js there is no ts.
        let js_missing = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "js").unwrap_err();
        let mut r = Resolver { cfg: &cfg, lookup: &l, prober: &p, cache: &mut cache };
        let nf = r.resolve_ts(&spec("ts"), Err(&js_missing)).unwrap_err();
        assert!(nf.message.starts_with("engine 'ts' (TypeScript) not found — @ts blocks run on the js engine\ntried: node on PATH"), "{}", nf.message);
    }

    #[test]
    fn not_found_message_lines() {
        let root = temp("message");
        let apps = root.join("WindowsApps");
        std::fs::create_dir_all(&apps).unwrap();
        let alias = exe(&apps, "python3");
        let p = fake(|_| ProbeOutcome::Exited { code: Some(9009), output: String::new() });
        let nf = resolve_with(&Config::default(), &lookup(&[&apps], Some(&apps)), &p, &mut ProbeCache::off(), "py").unwrap_err();
        let tried = if cfg!(windows) { "python3, python, py -3" } else { "python3, python" };
        let want = format!(
            "engine 'py' (Python) not found\ntried: {tried} on PATH\nskipped: {} (Microsoft Store placeholder)\ninstall: {}\nor set [engines.py] command in v2.toml, or V2_ENGINE_PY",
            alias.display(),
            install_hint("py")
        );
        assert_eq!(nf.message, want);
        assert_eq!(nf.status, Status::Missing);
    }

    #[test]
    fn cache_hit_skips_probe() {
        let home = temp("cachehome");
        let d = temp("cachebin");
        let node = exe(&d, "node");
        let l = lookup(&[&d], None);
        let cfg = Config::default();
        let p = fake(|_| says("v24.11.1"));
        let mut cache = ProbeCache::open(Some(&home), CacheMode::ReadWrite);
        resolve_with(&cfg, &l, &p, &mut cache, "js").unwrap();
        cache.save();
        assert_eq!(p.calls.get(), 1);
        let file = home.join(".v2").join("cache").join("toolchains.json");
        let saved: Json = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["format"], 1);
        let entry = saved["entries"].as_object().unwrap().values().next().unwrap().clone();
        assert_eq!((entry["ok"].clone(), entry["version"].clone()), (json!(true), json!("24.11.1")));
        let mut again = ProbeCache::open(Some(&home), CacheMode::ReadWrite);
        let e = resolve_with(&cfg, &l, &p, &mut again, "js").unwrap();
        assert_eq!((p.calls.get(), e.version), (1, Some(Version(24, 11, 1))), "served from the cache");
        // A changed file is probed again. Skips are not stored: every run probes them again (a
        // version-manager shim answers per project, 7.7).
        let p_old = fake(|_| says("v16.0.0"));
        std::fs::write(&node, b"changed").unwrap();
        let mut c = ProbeCache::open(Some(&home), CacheMode::ReadWrite);
        assert!(resolve_with(&cfg, &l, &p_old, &mut c, "js").is_err());
        assert_eq!(p_old.calls.get(), 1);
        c.save();
        let saved: Json = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        let entries = saved["entries"].as_object().unwrap();
        assert!(entries.values().all(|e| e["ok"] == json!(true) && e["version"] == json!("24.11.1")), "usable entries only: {saved}");
        let mut c = ProbeCache::open(Some(&home), CacheMode::ReadWrite);
        let nf = resolve_with(&cfg, &l, &p_old, &mut c, "js").unwrap_err();
        assert_eq!((p_old.calls.get(), nf.status), (2, Status::Old(Version(16, 0, 0))));
    }

    #[test]
    fn xcrun_shims_wait_for_the_command_line_tools() {
        // macOS: /usr/bin/python3 opens an install dialog without the Command Line Tools (7.6).
        let d = temp("xcrun");
        let shim = exe(&d, "python3");
        let mut l = lookup(&[&d], None);
        l.xcrun_shims = vec![shim.clone()];
        // (The Command Line Tools' own Python is 3.9, below the minimum; a newer one stands in.)
        for (clt, expect) in [(1, None), (0, Some(Version(3, 12, 4)))] {
            let p = fake(move |prog| {
                if prog.ends_with("xcode-select") {
                    ProbeOutcome::Exited { code: Some(clt), output: String::new() }
                } else {
                    says("Python 3.12.4")
                }
            });
            let r = resolve_with(&Config::default(), &l, &p, &mut ProbeCache::off(), "py");
            match expect {
                Some(v) => assert_eq!(r.unwrap().version, Some(v)),
                None => {
                    assert_eq!(r.unwrap_err().skipped, [(shim.clone(), "Command Line Tools missing".to_string())]);
                    assert_eq!(p.argvs.borrow().len(), 1, "the shim itself never ran");
                }
            }
        }
    }

    #[test]
    fn a_probe_killed_by_a_signal_says_so() {
        let j = judge(&spec("py"), false, &ProbeOutcome::Killed(9));
        let text = ExitInfo { code: None, signal: Some(9) }.describe();
        assert_eq!(j, Judged::Skipped(format!("version probe failed ({text})")));
        assert!(text.starts_with("killed by signal 9"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn a_named_file_need_not_be_executable_to_be_found() {
        // 7.5: X_OK is the PATH walk's test; a configured path that can't run fails its probe.
        let d = temp("noexec");
        let f = d.join("python3");
        std::fs::write(&f, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&f, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        let l = Lookup { cwd: d.clone(), ..lookup(&[&d], None) };
        assert_eq!(find_program("./python3", &l), [d.join("./python3")]);
        assert!(find_program("python3", &l).is_empty(), "not on the PATH walk");
    }

    #[test]
    fn bad_cache_files_count_as_empty_and_modes() {
        let home = temp("cachebad");
        let d = temp("cachebadbin");
        exe(&d, "node");
        let l = lookup(&[&d], None);
        let cfg = Config::default();
        let file = home.join(".v2").join("cache").join("toolchains.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let p = fake(|_| says("v24.11.1"));
        for bad in ["not json", r#"{"format": 2, "entries": {}}"#, r#"{"format": 1}"#] {
            std::fs::write(&file, bad).unwrap();
            let mut c = ProbeCache::open(Some(&home), CacheMode::ReadWrite);
            resolve_with(&cfg, &l, &p, &mut c, "js").unwrap();
            c.save();
        }
        assert_eq!(p.calls.get(), 3);
        // Refresh probes again even with a good entry, and rewrites the file.
        let mut c = ProbeCache::open(Some(&home), CacheMode::Refresh);
        resolve_with(&cfg, &l, &p, &mut c, "js").unwrap();
        assert_eq!(p.calls.get(), 4);
        // A Refresh probe that now skips drops the entry: the cache holds usable entries only.
        c.save();
        let skips = fake(|_| ProbeOutcome::Exited { code: Some(1), output: String::new() });
        let mut c = ProbeCache::open(Some(&home), CacheMode::Refresh);
        assert!(resolve_with(&cfg, &l, &skips, &mut c, "js").is_err());
        c.save();
        let saved: Json = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["entries"], json!({}), "{saved}");
        // Off never writes.
        std::fs::remove_file(&file).unwrap();
        let mut c = ProbeCache::open(Some(&home), CacheMode::Off);
        resolve_with(&cfg, &l, &p, &mut c, "js").unwrap();
        c.save();
        assert!(!file.exists());
    }

    #[test]
    fn the_minimum_itself_is_usable() {
        let d = temp("kminimum");
        exe(&d, "python3");
        exe(&d, "node");
        let p = fake(|prog| says(if prog.to_string_lossy().contains("node") { "v22.13.0" } else { "Python 3.10.0" }));
        let l = lookup(&[&d], None);
        assert_eq!(resolve_with(&Config::default(), &l, &p, &mut ProbeCache::off(), "py").unwrap().version, Some(Version(3, 10, 0)));
        assert_eq!(resolve_with(&Config::default(), &l, &p, &mut ProbeCache::off(), "js").unwrap().version, Some(Version(22, 13, 0)));
    }

    #[test]
    fn old_shows_the_newest_even_when_found_last() {
        let d = temp("koldorder");
        exe(&d, "python3");
        exe(&d, "python");
        let p = fake(|prog| says(if prog.file_stem().is_some_and(|s| s == "python3") { "Python 3.6.8" } else { "Python 3.7.9" }));
        let nf = resolve_with(&Config::default(), &lookup(&[&d], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.status, Status::Old(Version(3, 7, 9)));
    }

    #[test]
    fn a_same_size_replacement_is_probed_again() {
        let home = temp("kcachemtime");
        let d = temp("kcachemtimebin");
        let node = exe(&d, "node");
        std::fs::write(&node, b"v1").unwrap();
        let l = lookup(&[&d], None);
        let cfg = Config::default();
        let p = fake(|_| says("v24.11.1"));
        let mut c = ProbeCache::open(Some(&home), CacheMode::ReadWrite);
        resolve_with(&cfg, &l, &p, &mut c, "js").unwrap();
        c.save();
        // Same size, another mtime: say a patch upgrade that kept the launcher's size (7.7).
        std::fs::write(&node, b"v2").unwrap();
        let f = std::fs::File::options().write(true).open(&node).unwrap();
        f.set_modified(std::time::SystemTime::now() + Duration::from_secs(120)).unwrap();
        drop(f);
        let p2 = fake(|_| says("v16.0.0"));
        let mut c = ProbeCache::open(Some(&home), CacheMode::ReadWrite);
        let nf = resolve_with(&cfg, &l, &p2, &mut c, "js").unwrap_err();
        assert_eq!((p2.calls.get(), nf.status), (1, Status::Old(Version(16, 0, 0))));
    }

    #[test]
    fn ts_origin_follows_js() {
        let d = temp("kts");
        let node = exe(&d, "node");
        let l = lookup(&[&d], None);
        let mut cache = ProbeCache::off();
        let p = fake(|_| says("v24.11.1"));
        // js set by V2_ENGINE_JS: ts shows js's FROM (19.2).
        let cfg = with_command("js", &[&node.to_string_lossy()], ConfigSource::Env("V2_ENGINE_JS".into()));
        let mut r = Resolver { cfg: &cfg, lookup: &l, prober: &p, cache: &mut cache };
        let js = r.resolve(&spec("js")).unwrap();
        let ts = r.resolve_ts(&spec("ts"), Ok(&js)).unwrap();
        assert_eq!(ts.origin.label(), "V2_ENGINE_JS");
    }

    #[test]
    fn ts_names_a_one_line_js_failure() {
        let cfg = with_command("js", &["missing/node.exe"], ConfigSource::Env("V2_ENGINE_JS".into()));
        let p = fake(|_| says("v24.11.1"));
        let l = lookup(&[], None);
        let mut cache = ProbeCache::off();
        let mut r = Resolver { cfg: &cfg, lookup: &l, prober: &p, cache: &mut cache };
        let js = r.resolve(&spec("js")).unwrap_err();
        let nf = r.resolve_ts(&spec("ts"), Err(&js)).unwrap_err();
        assert_eq!(nf.message, format!("engine 'ts' (TypeScript) not found — @ts blocks run on the js engine\njs: {}", js.message));
    }

    #[cfg(windows)]
    #[test]
    fn a_configured_path_without_extension_gets_one() {
        let d = temp("knoext");
        let python = exe(&d, "python");
        let bare = d.join("python");
        assert_eq!(find_program(&bare.to_string_lossy(), &lookup(&[], None)), [python]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_apps_is_matched_case_insensitively() {
        let root = temp("kalias");
        let apps = root.join("WindowsApps");
        std::fs::create_dir_all(&apps).unwrap();
        exe(&apps, "python3");
        let p = fake(|_| ProbeOutcome::Exited { code: Some(9009), output: String::new() });
        let upper = root.join("WINDOWSAPPS");
        let nf = resolve_with(&Config::default(), &lookup(&[&upper], Some(&apps)), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.skipped[0].1, "Microsoft Store placeholder");
    }

    #[test]
    fn a_folder_listed_twice_is_one_hit() {
        let d = temp("ktwice");
        let node = exe(&d, "node");
        assert_eq!(find_program("node", &lookup(&[&d, &d], None)), [node]);
    }

    #[cfg(windows)]
    #[test]
    fn unset_pathext_accepts_cmd() {
        let d = temp("kpathext");
        std::fs::write(d.join("tsx.cmd"), b"").unwrap();
        assert_eq!(find_program("tsx", &lookup(&[&d], None)), [d.join("tsx.cmd")]);
    }

    #[cfg(unix)]
    #[test]
    fn venv_prefers_bin_python() {
        let d = temp("kvenv");
        let bin = d.join(".venv").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let python = exe(&bin, "python");
        exe(&bin, "python3");
        let cfg = Config { python_venv: Some(Sourced { value: d.join(".venv"), from: project_source() }), ..Config::default() };
        let p = fake(|_| says("Python 3.12.1"));
        let e = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap();
        assert_eq!(e.argv, [python.into_os_string()]);
    }
}
