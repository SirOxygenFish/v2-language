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

    /// In `windows_apps`, or in any folder named `Microsoft\WindowsApps` (another user's, or
    /// with `LOCALAPPDATA` moved): Windows keeps App Execution Aliases there (7.6).
    fn is_alias(&self, path: &Path) -> bool {
        let Some(apps) = &self.windows_apps else { return false };
        let (p, a) = (path.to_string_lossy().to_lowercase(), apps.to_string_lossy().to_lowercase());
        let named = |p: &Path| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("WindowsApps")) && p.parent().and_then(Path::file_name).is_some_and(|n| n.eq_ignore_ascii_case("Microsoft"));
        (p.starts_with(&a) && p[a.len()..].starts_with(['\\', '/'])) || path.parent().is_some_and(named)
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

/// Every hit for `name`, in `PATH` order (7.5): what the resolver's folder-by-folder search
/// meets one at a time. With a separator, `name` is joined to `l.cwd`; otherwise each absolute
/// `PATH` entry is tried (never the cwd).
#[cfg(test)]
pub fn find_program(name: &str, l: &Lookup) -> Vec<PathBuf> {
    search(name, l, usize::MAX)
}

/// The first of `find_program`'s hits, with no walk past it: a long `PATH` can be slow to stat.
pub fn first_program(name: &str, l: &Lookup) -> Option<PathBuf> {
    search(name, l, 1).pop()
}

fn search(name: &str, l: &Lookup, wanted: usize) -> Vec<PathBuf> {
    let mut hits: Vec<PathBuf> = Vec::new();
    // `X_OK` only on the PATH walk: a named file that can't run fails its probe with the OS
    // message instead of looking missing (7.5).
    let consider = |hits: &mut Vec<PathBuf>, p: PathBuf, walk: bool| {
        if hits.len() < wanted && is_regular_file(&p) && (!walk || executable(&p)) && !hits.contains(&p) {
            hits.push(p);
        }
    };
    let as_path = Path::new(name);
    if is_drive_relative(as_path) {
        return hits;
    }
    if name.contains(['/', '\\']) {
        // `./x.bat` without the `.`, as cmd.exe shows a batch file's path.
        let p: PathBuf = l.cwd.join(as_path).components().collect();
        if cfg!(windows) && !has_accepted_extension(as_path) {
            for ext in ACCEPTED {
                consider(&mut hits, with_extension_appended(&p, ext), false);
            }
        } else {
            consider(&mut hits, p, false);
        }
        return hits;
    }
    let path = l.path.clone().unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        if hits.len() >= wanted {
            break;
        }
        if dir.as_os_str().is_empty() || dir.is_relative() {
            continue;
        }
        if cfg!(windows) {
            if has_accepted_extension(as_path) {
                consider(&mut hits, dir.join(name), true);
            }
            for ext in l.extensions() {
                consider(&mut hits, with_extension_appended(&dir.join(name), &ext), true);
            }
        } else {
            consider(&mut hits, dir.join(name), true);
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
    /// `limit`: `None` is no time limit.
    fn probe(&self, argv: &[OsString], env: &[(OsString, OsString)], limit: Option<Duration>) -> ProbeOutcome;
}

/// Runs the probe under 8.3's child rules with a timer; stdout then stderr, lossy UTF-8.
pub struct RealProber;

impl Prober for RealProber {
    fn probe(&self, argv: &[OsString], env: &[(OsString, OsString)], limit: Option<Duration>) -> ProbeOutcome {
        match process::run_captured(argv, env, limit, None, false) {
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

/// What to write instead of a folder where the program goes (7.8): a virtual environment's
/// `[python] venv`, else the program in the folder, when there is one.
fn folder_hint(who: &str, id: &str, dir: &Path) -> String {
    let names: &[&str] = match (id, cfg!(windows)) {
        ("py", true) => &["python.exe", "Scripts\\python.exe"],
        ("py", false) => &["python3", "python", "bin/python3", "bin/python"],
        (_, true) => &["node.exe"],
        _ => &["node", "bin/node"],
    };
    let inside = names.iter().map(|n| dir.join(n)).find(|p| p.is_file());
    if id == "py" && dir.join("pyvenv.cfg").is_file() {
        let its = inside.map(|p| format!(", or point {who} at its python, {}", p.display())).unwrap_or_default();
        let venv = crate::config::toml_string(&dir.display().to_string());
        return format!("; {who} names a virtual environment: write [python] venv = {venv} in v2.toml{its}");
    }
    match inside {
        Some(p) => format!("; {who} names a folder: point it at the program in it, {}", p.display()),
        None => format!("; {who} names a folder: point it at the program, not at its folder"),
    }
}

/// The skip reason of a `py` launcher left with no Python (7.6): like the placeholder, no Python
/// is installed, so the engine is missing, not broken (7.8).
const NO_RUNTIME: &str = "a py launcher with no Python installed";

fn judge(spec: &EngineSpec, alias: bool, outcome: &ProbeOutcome, limit: Option<Duration>) -> Judged {
    match outcome {
        ProbeOutcome::SpawnFailed(msg) => Judged::Skipped(msg.clone()),
        ProbeOutcome::TimedOut => Judged::Skipped(format!("version probe timed out ({} s)", limit.unwrap_or(PROBE_LIMIT).as_secs_f64())),
        ProbeOutcome::Killed(sig) => Judged::Skipped(format!("version probe failed ({})", ExitInfo { code: None, signal: Some(*sig) }.describe())),
        ProbeOutcome::Exited { code, output } => {
            // The placeholder prints "Python was not found" and exits 9009 (7.6).
            if alias && (*code == Some(9009) || output.contains("Python was not found")) {
                return Judged::Skipped(PLACEHOLDER.into());
            }
            // What the launcher says once Python is uninstalled.
            if *code != Some(0) && output.contains("No suitable Python runtime found") {
                return Judged::Skipped(NO_RUNTIME.into());
            }
            if *code != Some(0) {
                // What the program said, first line, so a missing library shows (7.6).
                let said = output.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| {
                    let cut: String = l.chars().take(160).collect();
                    format!(": {cut}")
                });
                return Judged::Skipped(format!("version probe failed ({}){}", ExitInfo { code: *code, signal: None }.describe(), said.unwrap_or_default()));
            }
            let Some((args, pattern)) = &spec.probe else { return Judged::Skipped("no version probe".into()) };
            let Some(version) = pattern.find(output) else {
                // `V2_ENGINE_JS` naming Python: what it printed says so, and whose version it isn't.
                let said = output.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| format!(" (it printed: {})", l.chars().take(80).collect::<String>()));
                let program = if &*spec.id == "js" { "Node.js" } else { &*spec.language };
                return Judged::Skipped(format!("no {program} version in its {} output{}", args.join(" "), said.unwrap_or_default()));
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
    /// This run's changes (`None`: removed), laid over the file as it is when saving, so runs
    /// at the same time keep each other's.
    changed: Vec<(String, Option<Json>)>,
}

impl ProbeCache {
    /// The cache under `home`; an unreadable file or an unknown `format` counts as empty. A home
    /// folder that doesn't exist (`/nonexistent`, a service account's) has no cache, rather than
    /// one V2 would create it for (18.1).
    pub fn open(home: Option<&Path>, mode: CacheMode) -> ProbeCache {
        let file = home.filter(|h| mode != CacheMode::Off && h.is_dir()).map(|h| h.join(".v2").join("cache").join("toolchains.json"));
        let entries = file.as_deref().and_then(read_cache).unwrap_or_default();
        ProbeCache { file, mode, entries, changed: Vec::new() }
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
            let entry = json!({"path": path.to_string_lossy(), "placeholder": true});
            self.entries.insert(key.clone(), entry.clone());
            self.changed.push((key, Some(entry)));
            return;
        }
        let Judged::Usable(version) = j else {
            if self.entries.shift_remove(&key).is_some() {
                self.changed.push((key, None));
            }
            return;
        };
        let entry = json!({
            "path": path.to_string_lossy(),
            "ok": true,
            "version": version.to_string(),
            "output": output,
        });
        self.entries.insert(key.clone(), entry.clone());
        self.changed.push((key, Some(entry)));
    }

    /// Write the file if anything changed: a temp file, then a rename (a failed rename is ignored).
    pub fn save(&mut self) {
        let Some(file) = &self.file else { return };
        if self.changed.is_empty() {
            return;
        }
        let Some(dir) = file.parent() else { return };
        if create_private_dir(dir).is_err() {
            return;
        }
        let mut entries = read_cache(file).unwrap_or_default();
        for (key, entry) in self.changed.drain(..) {
            match entry {
                Some(entry) => entries.insert(key, entry),
                None => entries.shift_remove(&key),
            };
        }
        let text = json!({"format": 1, "entries": Json::Object(entries)}).to_string();
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

/// A command as doctor and messages show it: space-joined, and when it has more than one item,
/// the items with a space quoted, so `["C:/Program", "Files/…"]` can't pass for one path (19.2).
fn join(argv: &[OsString]) -> String {
    let items: Vec<String> = argv.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    if items.len() > 1 {
        items.iter().map(|i| crate::cli::shell_word(i)).collect::<Vec<_>>().join(" ")
    } else {
        items.join(" ")
    }
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

    /// Probe one hit, through the cache, for at most `limit`.
    fn probe(&mut self, spec: &EngineSpec, argv: &[OsString], path: &Path, limit: Option<Duration>) -> Judged {
        let key = cache_key(spec, argv, path);
        if let Some(j) = key.as_deref().and_then(|k| self.cache.get(k)) {
            return j;
        }
        let mut full = argv.to_vec();
        if let Some((args, _)) = &spec.probe {
            full.extend(args.iter().map(|a| OsString::from(&**a)));
        }
        let outcome = self.prober.probe(&full, &Self::env(spec), limit);
        let judged = judge(spec, self.lookup.is_alias(path), &outcome, limit);
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
        // The user's own command may start slowly: its probe waits as long as starting it may
        // (`startup_timeout`, 30 s unless set; 0 is no limit).
        let start = self.cfg.engines.get(&*spec.id).and_then(|e| e.startup_timeout.as_ref()).map_or(crate::engines::units::STARTUP_DEFAULT, |t| t.value);
        // Too big for a Duration is no limit, like 0.
        let limit = (start > 0.0).then(|| Duration::try_from_secs_f64(start).ok()).flatten().map(|t| t.max(PROBE_LIMIT));
        match self.probe(spec, &argv, &path, limit) {
            Judged::Usable(v) => Ok(ResolvedEngine { spec: spec.clone(), origin, argv, version: Some(v), skipped: Vec::new() }),
            other => {
                let reason = other.reason().unwrap_or_default();
                let hint = if reason.starts_with("version probe timed out") {
                    format!("; raise [engines.{}] startup_timeout if it is only slow", spec.id)
                } else if reason == PLACEHOLDER {
                    format!("; install {} and name its full path, or leave command out: {}", spec.language, install_hint(&spec.id))
                } else if let Some(fix) = library_fix(&path, reason) {
                    format!("\nfix: {fix}")
                } else if origin == Origin::Venv {
                    // A venv's python that doesn't run: the Python it was made with is gone (an
                    // upgrade that removed it: Windows says "No Python at …").
                    let venv = path.parent().and_then(Path::parent).unwrap_or(&path);
                    let python = if cfg!(windows) { "py" } else { "python3" };
                    format!("; make the venv again, {python} -m venv --clear {}, and install its packages again", crate::cli::shell_word(&venv.display().to_string()))
                } else {
                    String::new()
                };
                let mut message = format!("engine '{}' {what} (set by {}) is not usable: {reason}{hint}", spec.id, set_by(from));
                if let (Judged::TooOld(v, _), Some(min)) = (&other, spec.min_version) {
                    // Worded as for one found on PATH (7.8), with what to do.
                    let found = if *v >= min {
                        format!("is not supported: found {v}, which has no type stripping; V2 needs {}", spec.needs(min))
                    } else {
                        format!("is too old: found {v}, V2 needs {}", spec.needs(min))
                    };
                    // The program, not the language: a newer Node.js.
                    let program = if &*spec.id == "js" { "Node.js" } else { &*spec.language };
                    let next = format!("point it at a newer {program}, or install one: {}", install_hint(&spec.id));
                    message = format!("engine '{}' {what} (set by {}) {found}; {next}", spec.id, set_by(from));
                }
                let mut nf = NotFound::broken(message, join(&argv), from);
                if let Judged::TooOld(v, _) = other {
                    nf.status = Status::Old(v);
                }
                Err(nf)
            }
        }
    }

    fn configured(&mut self, spec: &Rc<EngineSpec>, command: &[String], from: &ConfigSource) -> Result<ResolvedEngine, NotFound> {
        let name = &command[0];
        let Some(path) = first_program(name, self.lookup) else {
            // `V2_ENGINE_PY="C:\Python311"`: a folder; `V2_ENGINE_PY="py -3"`: the variable holds
            // one program, never split (18.4).
            let quote = |w: &str| if w.contains('\\') { format!("'{w}'") } else { format!("\"{w}\"") };
            // The shortest run of words that is a program, then its arguments: a path with spaces
            // (`C:\Program Files\…\python.exe -X utf8`) is one program.
            let words: Vec<&str> = name.split_whitespace().collect();
            let split = matches!(from, ConfigSource::Env(_))
                .then(|| (1..words.len()).find(|&k| first_program(&words[..k].join(" "), self.lookup).is_some()))
                .flatten();
            let hint = match from {
                _ if Path::new(name).is_dir() => {
                    let who = match from {
                        ConfigSource::Env(var) => var.clone(),
                        ConfigSource::Project(_) => format!("[engines.{}] command", spec.id),
                    };
                    folder_hint(&who, &spec.id, Path::new(name))
                }
                ConfigSource::Env(var) if split.is_some() => {
                    let k = split.unwrap_or(1);
                    let array: Vec<String> = std::iter::once(words[..k].join(" ")).chain(words[k..].iter().map(|w| w.to_string())).map(|w| quote(&w)).collect();
                    format!(
                        "; {var} names one program, without arguments: for a command with arguments, write [engines.{}] command = [{}] in v2.toml",
                        spec.id,
                        array.join(", ")
                    )
                }
                // `%ProgramFiles%\…` from PowerShell's single quotes, or a REG_SZ variable.
                _ if crate::config::unexpanded(name).is_some() => {
                    format!("; V2 doesn't expand {}: write the path out", crate::config::unexpanded(name).unwrap_or_default())
                }
                _ => String::new(),
            };
            return Err(NotFound::broken(
                format!("engine '{}' command '{name}' not found (set by {}){hint}", spec.id, set_by(from)),
                join(&command.iter().map(OsString::from).collect::<Vec<_>>()),
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
            let shown = venv.display().to_string();
            // What to write instead, as v2.toml's folder sees it (18.1).
            let base = match from {
                ConfigSource::Project(file) => file.parent().map(Path::to_path_buf),
                ConfigSource::Env(_) => None,
            };
            let write = |p: &Path| {
                let r = base.as_ref().and_then(|b| p.strip_prefix(b).ok()).filter(|r| !r.as_os_str().is_empty()).unwrap_or(p).display().to_string();
                if r.contains('\\') { format!("venv = '{r}'") } else { format!("venv = \"{r}\"") }
            };
            let is_venv = |p: &Path| p.join("pyvenv.cfg").is_file();
            // A command to paste: the path quoted as a shell needs it; the `py` launcher on
            // Windows (`python` may be the Store's stand-in), `python3` elsewhere.
            let python = if cfg!(windows) { "py" } else { "python3" };
            let pasted = |p: &Path| crate::cli::shell_word(&p.display().to_string());
            // `.venv/bin`, `.venv\Scripts`, or a python in one that isn't there: a path inside
            // the virtual environment, whose folder is the one above.
            let bin = |p: &Path| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("bin") || n.eq_ignore_ascii_case("Scripts"));
            let a_python = venv.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().starts_with("python"));
            let inside = if bin(venv) {
                venv.parent()
            } else if a_python && venv.parent().is_some_and(bin) {
                venv.parent().and_then(Path::parent)
            } else {
                None
            };
            // A conda environment's or an installation's Python (`C:\Program Files\Python311`,
            // `/usr`): never a place to run `-m venv` in, which would turn it into a venv of
            // another Python. Nor is a folder that holds anything else.
            let installed = |dir: &Path| -> Option<PathBuf> {
                let names: &[&str] = if cfg!(windows) { &["python.exe"] } else { &["bin/python3", "bin/python"] };
                names.iter().map(|n| dir.join(n)).find(|p| is_regular_file(p)).filter(|_| !is_venv(dir))
            };
            let free = |dir: &Path| std::fs::read_dir(dir).map_or(true, |mut d| d.next().is_none());
            let command = |p: &Path| {
                let p = p.display().to_string();
                if p.contains('\\') { format!("[engines.py] command = ['{p}']") } else { format!("[engines.py] command = [\"{p}\"]") }
            };
            let not_a_venv = |what: &str, py: &Path| {
                format!("[python] venv '{shown}' is {what}, not a virtual environment: name its python instead, {}, or make a virtual environment with it: {} -m venv .venv", command(py), pasted(py))
            };
            // Its python links to one that is gone, as after the system's Python was upgraded.
            let dangling = candidates.iter().find_map(|c| std::fs::read_link(c).ok().filter(|_| !c.exists()));
            let message = if let Some(target) = dangling {
                format!(
                    "[python] venv '{shown}' has a python that links to {}, which isn't there (was Python upgraded?): make the venv again, {python} -m venv --clear {}, and install its packages again",
                    target.display(),
                    pasted(venv)
                )
            } else if is_regular_file(venv) {
                // `.venv/bin/python`, `.venv\Scripts\python.exe`: the interpreter, not its folder.
                match venv.parent().and_then(Path::parent).filter(|f| is_venv(f)) {
                    Some(folder) => format!("[python] venv '{shown}' is a file: venv names the virtual environment's folder: write {}", write(folder)),
                    None => format!("[python] venv '{shown}' is a file: venv names a virtual environment's folder; for a Python that isn't in one, write {} instead of venv", command(venv)),
                }
            } else if let Some(folder) = inside.filter(|f| is_venv(f)) {
                format!("[python] venv '{shown}' is inside a virtual environment, and venv names the environment's folder: write {}", write(folder))
            } else if let Some(py) = installed(venv) {
                if venv.join("conda-meta").is_dir() {
                    let name = venv.file_name().map_or_else(|| shown.clone(), |n| n.to_string_lossy().into_owned());
                    format!("[python] venv '{shown}' is a conda environment, not a virtual environment: run v2 where it is activated (conda activate {name}) and leave venv out, or name its python instead: {}", command(&py))
                } else {
                    not_a_venv("a Python installation", &py)
                }
            } else if let Some(py) = inside.and_then(installed) {
                not_a_venv("a folder of a Python installation", &py)
            } else if let Some(folder) = inside {
                let create = if !free(folder) { String::new() } else { format!(" (create it: {python} -m venv {})", pasted(folder)) };
                format!("[python] venv '{shown}' is inside a virtual environment, and venv names the environment's folder: write {}{create}", write(folder))
            } else if is_venv(venv) {
                // A venv made on another system (`bin` where Windows wants `Scripts`), or damaged.
                format!("[python] venv '{shown}' has no {want}, so it was made on another system or is damaged: make it again here, {python} -m venv --clear {}", pasted(venv))
            } else if let Some(var) = crate::config::unexpanded(&shown) {
                format!("[python] venv '{shown}' has no {want}: V2 doesn't expand {var}: write the path out (a ~ at its start is your home folder)")
            } else if let Some(near) = venv
                .parent()
                .and_then(|dir| std::fs::read_dir(dir).ok())
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() && is_venv(p))
                .min()
            {
                format!("[python] venv '{shown}' has no {want}; there is a virtual environment in {}: did you mean {}?", near.display(), write(&near))
            } else if !free(venv) {
                format!("[python] venv '{shown}' has no {want}, and it isn't a virtual environment (it has no pyvenv.cfg): write the folder of one, or make one: {python} -m venv .venv")
            } else {
                format!("[python] venv '{shown}' has no {want} (create it: {python} -m venv {})", pasted(venv))
            };
            // COMMAND: the venv as given when it names a file, not a path inside it (19.2).
            let shown_command = if is_regular_file(venv) { shown.clone() } else { candidates[0].display().to_string() };
            return Err(NotFound::broken(message, shown_command, from));
        };
        let what = format!("[python] venv '{}'", venv.display());
        self.usable(spec, vec![exe.clone().into_os_string()], Origin::Venv, &what, from)
    }

    fn search(&mut self, spec: &Rc<EngineSpec>) -> Result<ResolvedEngine, NotFound> {
        let mut skipped: Vec<(PathBuf, String)> = Vec::new();
        let mut newest_old: Option<Version> = None;
        // A file met again, through another PATH entry (on most Linux systems /bin is a link to
        // /usr/bin) or another candidate name, with the same arguments, isn't probed or listed
        // twice (7.6).
        let mut seen: std::collections::HashSet<(PathBuf, Vec<String>)> = std::collections::HashSet::new();
        // A hit that is installed but doesn't run (its probe failed or timed out, the wrong
        // target), as opposed to a placeholder: the engine is then broken, not missing (7.8).
        let mut failed = false;
        // The names that take no arguments (`python3`, `python`) are looked for folder by folder,
        // so the first PATH folder that has any of them wins: an activated virtual environment's
        // `python.exe` before a `python3.exe` further down. Then the others (`py -3`) (7.6). Each
        // folder is looked in only when the ones before it had nothing usable: on WSL, PATH holds
        // dozens of Windows folders, each slow to look in.
        let path = self.lookup.path.clone().unwrap_or_default();
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).filter(|d| !d.as_os_str().is_empty() && !d.is_relative()).collect();
        let plain = spec.candidates.iter().filter(|c| c.len() == 1);
        let with_args = spec.candidates.iter().filter(|c| c.len() > 1);
        let order = dirs
            .iter()
            .flat_map(|d| plain.clone().map(move |c| (d, c)))
            .chain(with_args.flat_map(|c| dirs.iter().map(move |d| (d, c))));
        for (dir, candidate) in order {
            let one = Lookup { path: Some(dir.clone().into_os_string()), ..self.lookup.clone() };
            for hit in search(&candidate[0], &one, usize::MAX) {
                let file = std::fs::canonicalize(&hit).unwrap_or_else(|_| hit.clone());
                if !seen.insert((file, candidate[1..].iter().map(|a| a.to_string()).collect())) {
                    continue;
                }
                if self.lookup.xcrun_shims.contains(&hit) && !self.command_line_tools() {
                    skipped.push((hit, "Command Line Tools missing".into()));
                    continue;
                }
                let mut argv = vec![hit.clone().into_os_string()];
                argv.extend(candidate[1..].iter().map(|a| OsString::from(&**a)));
                match self.probe(spec, &argv, &hit, Some(PROBE_LIMIT)) {
                    Judged::Usable(v) => {
                        return Ok(ResolvedEngine { spec: spec.clone(), origin: Origin::Path, argv, version: Some(v), skipped })
                    }
                    Judged::TooOld(v, reason) => {
                        newest_old = newest_old.max(Some(v));
                        skipped.push((hit, reason));
                    }
                    Judged::Skipped(reason) => {
                        failed |= reason != PLACEHOLDER && reason != NO_RUNTIME;
                        skipped.push((hit, reason));
                    }
                }
            }
        }
        let status = match newest_old {
            Some(v) => Status::Old(v),
            None if failed => Status::Broken,
            None => Status::Missing,
        };
        let mut message = not_found_message(spec, &skipped, newest_old, failed);
        // `env -i v2 x.v2`: nothing to search (7.5).
        if self.lookup.path.as_ref().is_none_or(|p| p.is_empty()) {
            message = message.replacen(" on PATH", " on PATH, which isn't set", 1);
        }
        Err(NotFound { status, message, command: None, from: None, skipped })
    }

    /// macOS: whether the Command Line Tools are installed, so the `xcrun` shims can run
    /// without opening an install dialog (7.6).
    fn command_line_tools(&self) -> bool {
        let argv = [OsString::from("/usr/bin/xcode-select"), OsString::from("-p")];
        matches!(self.prober.probe(&argv, &[], Some(PROBE_LIMIT)), ProbeOutcome::Exited { code: Some(0), .. })
    }

    /// ts: the js engine, if its Node has built-in type stripping (7.6).
    pub fn resolve_ts(&mut self, spec: &Rc<EngineSpec>, js: Result<&ResolvedEngine, &NotFound>) -> Result<ResolvedEngine, NotFound> {
        let js = js.map_err(|nf| {
            let rest: Vec<&str> = nf.message.lines().skip(1).collect();
            // A configured js that can't run is named for what is wrong with it.
            let mut message = match nf.status {
                Status::Missing => "engine 'ts' (TypeScript) not found — @ts blocks run on the js engine".to_string(),
                Status::Old(_) | Status::Broken => {
                    format!("engine 'ts' (TypeScript) can't run — @ts blocks run on the js engine, and {}", nf.message.lines().next().unwrap_or_default())
                }
            };
            if rest.is_empty() && nf.status == Status::Missing {
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

/// What to do when `path` couldn't start because a shared library is missing (its probe said
/// `error while loading shared libraries: libatomic.so.1: …`, as Node.js's own Linux build does
/// on a minimal system): the package for libatomic, else the library to install.
pub fn library_fix(path: &Path, reason: &str) -> Option<String> {
    let (_, rest) = reason.split_once("error while loading shared libraries: ")?;
    let lib = rest.split(':').next()?.trim();
    Some(if lib.starts_with("libatomic.so") {
        let (family, sudo) = crate::engines::spec::this_linux();
        format!("{} needs {lib}: {}", path.display(), crate::engines::spec::package_line(family, sudo, "libatomic1", "libatomic", Some(())))
    } else {
        format!("{} needs {lib}, which isn't installed: install it with your system's package manager", path.display())
    })
}

/// 7.8's message: what was found (nothing, or only versions that are too old), what was tried,
/// what was skipped and why, how to install, how to configure.
fn not_found_message(spec: &EngineSpec, skipped: &[(PathBuf, String)], newest_old: Option<Version>, failed: bool) -> String {
    let tried: Vec<String> = spec.candidates.iter().map(|c| c.join(" ")).collect();
    let head = match (newest_old, spec.min_version) {
        // Node 23.0 and 23.1: newer than the minimum, but without type stripping (7.6).
        (Some(v), Some(min)) if v >= min => {
            format!("engine '{}' ({}) is not supported: found {v}, which has no type stripping; V2 needs {}", spec.id, spec.language, spec.needs(min))
        }
        (Some(v), Some(min)) => {
            // Another one, perhaps the newer one just installed, doesn't run: say so up front.
            let also = if failed { "; another doesn't run (see why below)" } else { "" };
            format!("engine '{}' ({}) is too old: found {v}, V2 needs {}{also}", spec.id, spec.language, spec.needs(min))
        }
        (None, _) if failed => format!("engine '{}' ({}) was found but doesn't run (see why below)", spec.id, spec.language),
        _ => format!("engine '{}' ({}) not found", spec.id, spec.language),
    };
    let mut m = format!("{head}\ntried: {} on PATH", tried.join(", "));
    for (path, reason) in skipped {
        m.push_str(&format!("\nskipped: {} ({reason})", path.display()));
    }
    for fix in skipped.iter().filter_map(|(path, reason)| library_fix(path, reason)) {
        m.push_str(&format!("\nfix: {fix}"));
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
        limits: Rc<RefCell<Vec<Option<Duration>>>>,
    }

    impl Prober for Fake {
        fn probe(&self, argv: &[OsString], _: &[(OsString, OsString)], limit: Option<Duration>) -> ProbeOutcome {
            self.calls.set(self.calls.get() + 1);
            self.argvs.borrow_mut().push(argv.to_vec());
            self.limits.borrow_mut().push(limit);
            (self.answer)(Path::new(&argv[0]))
        }
    }

    fn says(text: &str) -> ProbeOutcome {
        ProbeOutcome::Exited { code: Some(0), output: text.into() }
    }

    fn fake(answer: impl Fn(&Path) -> ProbeOutcome + 'static) -> Fake {
        Fake { answer: Box::new(answer), calls: Rc::new(Cell::new(0)), argvs: Rc::new(RefCell::new(Vec::new())), limits: Rc::default() }
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

    #[cfg(unix)]
    #[test]
    fn a_venv_whose_python_is_gone_says_so() {
        // 7.6: an upgrade of the system's Python leaves a venv's bin/python linking to nothing.
        let d = temp("goneven");
        let bin = d.join(".venv").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(d.join(".venv").join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        std::os::unix::fs::symlink("/nonexistent/python3.99", bin.join("python")).unwrap();
        let cfg = Config { python_venv: Some(Sourced { value: d.join(".venv"), from: project_source() }), ..Config::default() };
        let nf = resolve_with(&cfg, &lookup(&[], None), &fake(|_| says("Python 3.12.1")), &mut ProbeCache::off(), "py").unwrap_err();
        let want = format!(
            "[python] venv '{}' has a python that links to /nonexistent/python3.99, which isn't there (was Python upgraded?): make the venv again, python3 -m venv --clear {}, and install its packages again",
            d.join(".venv").display(),
            d.join(".venv").display()
        );
        assert_eq!(nf.message, want);
    }

    #[test]
    fn an_unset_path_is_named() {
        // 7.8: `env -i v2 x.v2` has no PATH to search, and the message says so.
        let p = fake(|_| says("Python 3.12.1"));
        let nf = resolve_with(&Config::default(), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.contains(" on PATH, which isn't set\n"), "{}", nf.message);
    }

    #[test]
    fn r41_a_placeholder_outside_localappdata_is_still_one() {
        // 7.6: a Microsoft\WindowsApps folder that isn't LOCALAPPDATA's (moved, or another
        // user's) holds aliases too, so with only placeholders py is missing, not broken (7.8).
        let root = temp("r41moved");
        let apps = root.join("Microsoft").join("WindowsApps");
        std::fs::create_dir_all(&apps).unwrap();
        let alias = exe(&apps, "python3");
        let p = fake(|_| ProbeOutcome::Exited { code: Some(9009), output: "Python was not found; run without arguments to install from the Microsoft Store".into() });
        let l = lookup(&[&apps], Some(&root.join("elsewhere").join("WindowsApps")));
        let nf = resolve_with(&Config::default(), &l, &p, &mut ProbeCache::off(), "py").expect_err("no Python");
        assert_eq!(nf.status, Status::Missing, "{}", nf.message);
        assert!(nf.skipped.contains(&(alias, "Microsoft Store placeholder".to_string())), "{:?}", nf.skipped);
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
    fn r67_a_real_alias_is_never_cached_as_a_placeholder() {
        // R67's last part: an alias that runs a real Python is cached as usable, one that fails
        // another way isn't cached, and a cached placeholder whose alias file changed (the Store
        // app was installed) is probed again and used.
        let apps = temp("r67alias").join("WindowsApps");
        std::fs::create_dir_all(&apps).unwrap();
        let alias = exe(&apps, "python3");
        let l = lookup(&[&apps], Some(&apps));
        let cfg = Config::default();
        let runs = |home: &Path, p: &Fake| {
            (0..2)
                .map(|_| {
                    let mut c = ProbeCache::open(Some(home), CacheMode::ReadWrite);
                    let ok = resolve_with(&cfg, &l, p, &mut c, "py").is_ok();
                    c.save();
                    ok
                })
                .collect::<Vec<_>>()
        };
        let home = temp("r67alias-real");
        let real = fake(|_| says("Python 3.12.1"));
        assert_eq!(runs(&home, &real), [true, true]);
        assert_eq!(real.calls.get(), 1, "the second run is served from the cache");
        let saved: Json = serde_json::from_slice(&std::fs::read(home.join(".v2/cache/toolchains.json")).unwrap()).unwrap();
        let entry = saved["entries"].as_object().unwrap().values().next().unwrap();
        assert_eq!((&entry["ok"], &entry["version"], entry.get("placeholder")), (&Json::Bool(true), &Json::from("3.12.1"), None), "{saved}");
        let broken = fake(|_| ProbeOutcome::Exited { code: Some(1), output: String::new() });
        assert_eq!(runs(&temp("r67alias-broken"), &broken), [false, false]);
        assert_eq!(broken.calls.get(), 2, "a failure other than 9009 is never remembered");
        let home = temp("r67alias-installed");
        let placeholder = fake(|_| ProbeOutcome::Exited { code: Some(9009), output: "Python was not found".into() });
        assert_eq!(runs(&home, &placeholder), [false, false]);
        assert_eq!(placeholder.calls.get(), 1);
        std::fs::write(&alias, b"installed").unwrap();
        assert_eq!(runs(&home, &real), [true, true]);
        assert_eq!(real.calls.get(), 2, "the changed alias is probed again, once");
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

    #[test]
    fn the_first_program_stops_the_walk() {
        let (a, b) = (temp("first-a"), temp("first-b"));
        let (first, second) = (exe(&a, "tool"), exe(&b, "tool"));
        let path = std::env::join_paths([a, b]).unwrap();
        let l = Lookup { path: Some(path), pathext: None, windows_apps: None, cwd: std::env::current_dir().unwrap(), xcrun_shims: Vec::new() };
        assert_eq!(find_program("tool", &l), [first.clone(), second]);
        assert_eq!(first_program("tool", &l), Some(first));
        assert_eq!(first_program("no-such-tool", &l), None);
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
    fn the_first_folder_with_either_name_wins() {
        // 7.6: an activated virtual environment puts its folder, with only `python` (Windows:
        // python.exe), first on PATH; a `python3` further down must not win over it.
        let root = temp("pathorder");
        let (venv, other) = (root.join("venv"), root.join("other"));
        std::fs::create_dir_all(&venv).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let venv_python = exe(&venv, "python");
        exe(&other, "python3");
        let p = fake(|_| says("Python 3.12.1"));
        let e = resolve_with(&Config::default(), &lookup(&[&venv, &other], None), &p, &mut ProbeCache::off(), "py").unwrap();
        assert_eq!(e.argv, [venv_python.into_os_string()]);
        // In one folder the names keep their order: python3 first.
        let both = root.join("both");
        std::fs::create_dir_all(&both).unwrap();
        let both3 = exe(&both, "python3");
        exe(&both, "python");
        let e = resolve_with(&Config::default(), &lookup(&[&both], None), &p, &mut ProbeCache::off(), "py").unwrap();
        assert_eq!(e.argv, [both3.into_os_string()]);
    }

    #[test]
    fn the_same_file_twice_on_path_is_probed_once() {
        // Ubuntu's /bin is a link to /usr/bin, so one old node is met twice (7.6).
        let root = temp("samefile");
        let real = root.join("usr-bin");
        std::fs::create_dir_all(&real).unwrap();
        exe(&real, "node");
        #[cfg(unix)]
        let other = {
            let link = root.join("bin");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            link
        };
        // The same folder: Windows file names are case-insensitive.
        #[cfg(windows)]
        let other = root.join("USR-BIN");
        let p = fake(|_| says("v18.19.1"));
        let nf = resolve_with(&Config::default(), &lookup(&[&real, &other], None), &p, &mut ProbeCache::off(), "js").unwrap_err();
        assert_eq!(nf.skipped.len(), 1, "{:?}", nf.skipped);
        assert_eq!(nf.skipped[0].0, real.join(if cfg!(windows) { "node.exe" } else { "node" }));
        assert_eq!(p.calls.get(), 1);
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
        // The first line says what was found and what V2 needs, not "not found" (7.8).
        assert!(nf.message.starts_with("engine 'py' (Python) is too old: found 3.7.9, V2 needs 3.10+\ntried: "), "{}", nf.message);
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
            // What the program said: a node built for another system, missing a library.
            (
                ProbeOutcome::Exited { code: Some(127), output: "\nnode: error while loading shared libraries: libatomic.so.1: cannot open shared object file\n".into() },
                "version probe failed (exit code 127): node: error while loading shared libraries: libatomic.so.1: cannot open shared object file",
            ),
            (says("node, but no version"), "no Node.js version in its --version output (it printed: node, but no version)"),
        ] {
            let p = fake(move |_| outcome.clone());
            let nf = resolve_with(&Config::default(), &lookup(&[&d], None), &p, &mut ProbeCache::off(), "js").unwrap_err();
            // Installed but not running is broken, not missing (7.8).
            assert_eq!(nf.status, Status::Broken);
            assert!(nf.message.starts_with("engine 'js' (JavaScript) was found but doesn't run (see why below)\n"), "{}", nf.message);
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
    fn a_configured_command_is_probed_as_long_as_it_may_start() {
        // 7.7: a command the user configured with a startup_timeout above 10 s isn't judged
        // unusable at 10 s; what auto-detection finds still is.
        let d = temp("slowstart");
        let node = exe(&d, "node");
        let l = lookup(&[&d], None);
        let mut cfg = with_command("js", &[&node.to_string_lossy()], project_source());
        cfg.engines.get_mut("js").unwrap().startup_timeout = Some(Sourced { value: 60.0, from: project_source() });
        let p = fake(|_| says("v24.11.1"));
        let limits = p.limits.clone();
        resolve_with(&cfg, &l, &p, &mut ProbeCache::off(), "js").unwrap();
        resolve_with(&Config::default(), &l, &p, &mut ProbeCache::off(), "js").unwrap();
        assert_eq!(*limits.borrow(), [Some(Duration::from_secs(60)), Some(PROBE_LIMIT)]);
        // Unset, it is the 30 s default; 0 is no limit.
        limits.borrow_mut().clear();
        cfg.engines.get_mut("js").unwrap().startup_timeout = None;
        resolve_with(&cfg, &l, &p, &mut ProbeCache::off(), "js").unwrap();
        cfg.engines.get_mut("js").unwrap().startup_timeout = Some(Sourced { value: 0.0, from: project_source() });
        resolve_with(&cfg, &l, &p, &mut ProbeCache::off(), "js").unwrap();
        // Too big for a Duration is no limit, not a panic.
        cfg.engines.get_mut("js").unwrap().startup_timeout = Some(Sourced { value: 1e300, from: project_source() });
        resolve_with(&cfg, &l, &p, &mut ProbeCache::off(), "js").unwrap();
        assert_eq!(*limits.borrow(), [Some(Duration::from_secs(30)), None, None]);
    }

    #[test]
    fn runs_at_the_same_time_keep_each_others_cache_entries() {
        // 7.7: a save lays this run's changes over the file as it is then.
        let home = temp("cache-race");
        let d = temp("cache-race-bin");
        let (a, b) = (exe(&d, "node"), exe(&d, "nodejs"));
        let p = fake(|_| says("v24.11.1"));
        let spec = spec("js");
        let mut first = ProbeCache::open(Some(&home), CacheMode::ReadWrite);
        let mut second = ProbeCache::open(Some(&home), CacheMode::ReadWrite);
        for (cache, exe) in [(&mut first, &a), (&mut second, &b)] {
            let mut r = Resolver { cfg: &Config::default(), lookup: &lookup(&[], None), prober: &p, cache };
            assert!(matches!(r.probe(&spec, &[exe.clone().into_os_string()], exe, Some(PROBE_LIMIT)), Judged::Usable(_)));
        }
        first.save();
        second.save();
        let file = home.join(".v2").join("cache").join("toolchains.json");
        let saved: Json = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        assert_eq!(saved["entries"].as_object().unwrap().len(), 2, "{saved}");
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
    fn messages_say_what_to_do() {
        // A missing shared library names its package (7.8).
        let node = Path::new("/usr/local/bin/node");
        let reason = "version probe failed (exit code 127): node: error while loading shared libraries: libatomic.so.1: cannot open shared object file";
        let (family, sudo) = crate::engines::spec::this_linux();
        let line = crate::engines::spec::package_line(family, sudo, "libatomic1", "libatomic", Some(()));
        assert_eq!(library_fix(node, reason), Some(format!("/usr/local/bin/node needs libatomic.so.1: {line}")));
        assert!(library_fix(node, "version probe failed (exit code 127): x: error while loading shared libraries: libfoo.so.2: nope").unwrap().ends_with("needs libfoo.so.2, which isn't installed: install it with your system's package manager"));
        assert_eq!(library_fix(node, "version probe failed (exit code 1)"), None);
        // V2_ENGINE_PY holds one program (18.4): a command with arguments when its program is
        // on PATH, a folder as a folder, anything else as not found.
        let d = temp("onecmd");
        exe(&d, "py");
        let cfg = with_command("py", &["py -3"], ConfigSource::Env("V2_ENGINE_PY".into()));
        let nf = resolve_with(&cfg, &lookup(&[&d], None), &fake(|_| says("Python 3.12.1")), &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with("; V2_ENGINE_PY names one program, without arguments: for a command with arguments, write [engines.py] command = [\"py\", \"-3\"] in v2.toml"), "{}", nf.message);
        // A folder: the program in it, if there is one, or a virtual environment's [python] venv
        // (7.8), from a variable or from v2.toml.
        // A program whose path has a space, then arguments.
        let spaced = d.join("My Python");
        std::fs::create_dir_all(&spaced).unwrap();
        exe(&spaced, "python");
        let program = first_program(&spaced.join("python").to_string_lossy(), &lookup(&[], None)).unwrap();
        let cfg = with_command("py", &[&format!("{} -X utf8", program.display())], ConfigSource::Env("V2_ENGINE_PY".into()));
        let nf = resolve_with(&cfg, &lookup(&[&d], None), &fake(|_| says("Python 3.12.1")), &mut ProbeCache::off(), "py").unwrap_err();
        let shown = if program.display().to_string().contains('\\') { format!("'{}'", program.display()) } else { format!("\"{}\"", program.display()) };
        assert!(nf.message.ends_with(&format!("for a command with arguments, write [engines.py] command = [{shown}, \"-X\", \"utf8\"] in v2.toml")), "{}", nf.message);
        let folder = d.join("Python Folder");
        std::fs::create_dir_all(&folder).unwrap();
        let env = ConfigSource::Env("V2_ENGINE_PY".into());
        let says_for = |source: &ConfigSource, dir: &Path| {
            let cfg = with_command("py", &[&dir.to_string_lossy()], source.clone());
            resolve_with(&cfg, &lookup(&[&d], None), &fake(|_| says("Python 3.12.1")), &mut ProbeCache::off(), "py").unwrap_err().message
        };
        let m = says_for(&env, &folder);
        assert!(m.ends_with("; V2_ENGINE_PY names a folder: point it at the program, not at its folder"), "{m}");
        let inside = folder.join(if cfg!(windows) { "python.exe" } else { "python3" });
        std::fs::write(&inside, "").unwrap();
        let m = says_for(&env, &folder);
        assert!(m.ends_with(&format!("; V2_ENGINE_PY names a folder: point it at the program in it, {}", inside.display())), "{m}");
        let venv = d.join(".venv");
        let python = venv.join(if cfg!(windows) { "Scripts\\python.exe" } else { "bin/python3" });
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = x\n").unwrap();
        std::fs::write(&python, "").unwrap();
        let shown = crate::config::toml_string(&venv.display().to_string());
        let m = says_for(&env, &venv);
        assert!(m.ends_with(&format!("; V2_ENGINE_PY names a virtual environment: write [python] venv = {shown} in v2.toml, or point V2_ENGINE_PY at its python, {}", python.display())), "{m}");
        let m = says_for(&ConfigSource::Project(d.join("v2.toml")), &venv);
        assert!(m.ends_with(&format!("; [engines.py] command names a virtual environment: write [python] venv = {shown} in v2.toml, or point [engines.py] command at its python, {}", python.display())), "{m}");
        let cfg = with_command("py", &["no such python"], ConfigSource::Env("V2_ENGINE_PY".into()));
        let nf = resolve_with(&cfg, &lookup(&[&d], None), &fake(|_| says("Python 3.12.1")), &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with("not found (set by V2_ENGINE_PY)"), "{}", nf.message);
        // A variable V2 doesn't expand, as PowerShell's single quotes keep it (18.4).
        let unexpanded = d.join("%ProgramFiles%").join("Python311").join("python.exe");
        let cfg = with_command("py", &[&unexpanded.to_string_lossy()], ConfigSource::Env("V2_ENGINE_PY".into()));
        let nf = resolve_with(&cfg, &lookup(&[&d], None), &fake(|_| says("Python 3.12.1")), &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with("not found (set by V2_ENGINE_PY); V2 doesn't expand %ProgramFiles%: write the path out"), "{}", nf.message);
        // A py launcher with no Python behind it: Python is missing, not broken (7.8).
        let launcher = temp("noruntime");
        exe(&launcher, if cfg!(windows) { "py" } else { "python3" });
        let p = fake(|_| ProbeOutcome::Exited { code: Some(103), output: "No suitable Python runtime found\nPass --list (-0) to see all detected environments on your machine\n".into() });
        let nf = resolve_with(&Config::default(), &lookup(&[&launcher], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.status, Status::Missing, "{}", nf.message);
        assert!(nf.message.contains("(a py launcher with no Python installed)"), "{}", nf.message);
        // A configured one that is too old is worded as one found on PATH, with what to do (7.8).
        let cfg = with_command("py", &["py"], ConfigSource::Env("V2_ENGINE_PY".into()));
        let nf = resolve_with(&cfg, &lookup(&[&d], None), &fake(|_| says("Python 3.9.6")), &mut ProbeCache::off(), "py").unwrap_err();
        let next = format!("; point it at a newer Python, or install one: {}", install_hint("py"));
        assert!(nf.message.contains(&format!("(set by V2_ENGINE_PY) is too old: found 3.9.6, V2 needs 3.10+{next}")), "{}", nf.message);
        assert_eq!(nf.status, Status::Old(Version(3, 9, 6)), "{}", nf.message);
    }

    #[test]
    fn venv_interpreter_and_precedence() {
        let d = temp("venv");
        let venv = d.join(".venv");
        let missing = Config { python_venv: Some(Sourced { value: venv.clone(), from: project_source() }), ..Config::default() };
        let p = fake(|_| says("Python 3.12.1"));
        let nf = resolve_with(&missing, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        let want = if cfg!(windows) { r"Scripts\python.exe" } else { "bin/python" };
        let python = if cfg!(windows) { "py" } else { "python3" };
        assert!(nf.message.starts_with(&format!("[python] venv '{}' has no {want} (create it: {python} -m venv ", venv.display())), "{}", nf.message);
        // A folder with a space is quoted, so the command can be pasted (7.6).
        let spaced = d.join("my env");
        let cfg = Config { python_venv: Some(Sourced { value: spaced.clone(), from: project_source() }), ..Config::default() };
        let nf = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with(&format!("(create it: {python} -m venv \"{}\")", spaced.display())), "{}", nf.message);
        let bin = venv.join(if cfg!(windows) { "Scripts" } else { "bin" });
        std::fs::create_dir_all(&bin).unwrap();
        let python = exe(&bin, "python");
        let e = resolve_with(&missing, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap();
        assert_eq!((e.argv, e.origin), (vec![python.clone().into_os_string()], Origin::Venv));
        // The interpreter instead of its folder, and a venv under another name (7.6).
        std::fs::write(venv.join("pyvenv.cfg"), "home = x\n").unwrap();
        let toml = d.join("v2.toml");
        let at = |p: &Path| Config { python_venv: Some(Sourced { value: p.to_path_buf(), from: ConfigSource::Project(toml.clone()) }), ..Config::default() };
        let nf = resolve_with(&at(&python), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.message, format!("[python] venv '{}' is a file: venv names the virtual environment's folder: write venv = \".venv\"", python.display()));
        let nf = resolve_with(&at(&d.join("venv")), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with(&format!("; there is a virtual environment in {}: did you mean venv = \".venv\"?", venv.display())), "{}", nf.message);
        // Its bin (Scripts) folder, or a python in one that isn't there: the folder above (7.6).
        let nf = resolve_with(&at(&bin), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        let inside = "is inside a virtual environment, and venv names the environment's folder: write";
        assert_eq!(nf.message, format!("[python] venv '{}' {inside} venv = \".venv\"", bin.display()));
        let none = d.join("none").join("bin").join("python");
        let nf = resolve_with(&at(&none), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.message, format!("[python] venv '{}' {inside} venv = \"none\" (create it: {} -m venv {})", none.display(), if cfg!(windows) { "py" } else { "python3" }, d.join("none").display()));
        // A variable V2 doesn't expand: no venv to create there (18.1).
        let var = d.join("%USERPROFILE%").join(".venv");
        let nf = resolve_with(&at(&var), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with("V2 doesn't expand %USERPROFILE%: write the path out (a ~ at its start is your home folder)") && !nf.message.contains("-m venv"), "{}", nf.message);
        // A configured command wins over the venv, from v2.toml or V2_ENGINE_PY (7.6).
        let other = exe(&d, "python3");
        for command_from in [project_source(), ConfigSource::Env("V2_ENGINE_PY".into())] {
            let mut cfg = with_command("py", &[&other.to_string_lossy()], command_from);
            cfg.python_venv = Some(Sourced { value: venv.clone(), from: project_source() });
            let e = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap();
            assert_ne!(e.origin, Origin::Venv);
        }
    }

    #[test]
    fn a_venv_that_isnt_one_never_gets_venv_run_in_it() {
        // 7.6: a Python installation, a conda environment, or a folder of other things named as
        // the venv: what to write instead, and never `-m venv` into it, which would make a venv
        // of another Python over it.
        let d = temp("notvenv");
        let p = fake(|_| says("Python 3.12.1"));
        let at = |v: &Path| Config { python_venv: Some(Sourced { value: v.to_path_buf(), from: project_source() }), ..Config::default() };
        let (python, created) = if cfg!(windows) { ("py", "\"") } else { ("python3", "'") };
        let root = d.join("Python311");
        let bin = if cfg!(windows) { root.clone() } else { root.join("bin") };
        std::fs::create_dir_all(&bin).unwrap();
        let py = exe(&bin, if cfg!(windows) { "python" } else { "python3" });
        // Windows: the folder itself (elsewhere its bin/python3 is simply used); everywhere: its
        // bin folder, Scripts on Windows.
        let mut named = vec![(root.join(if cfg!(windows) { "Scripts" } else { "bin" }), "a folder of a Python installation")];
        if cfg!(windows) {
            named.push((root.clone(), "a Python installation"));
        }
        for (v, what) in named {
            let nf = resolve_with(&at(&v), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
            let head = format!("[python] venv '{}' is {what}, not a virtual environment: name its python instead, [engines.py] command = ", v.display());
            assert!(nf.message.starts_with(&head) && nf.message.ends_with(" -m venv .venv"), "{}", nf.message);
            assert!(nf.message.contains(&py.display().to_string()) && !nf.message.contains(&format!("-m venv {created}")), "{}", nf.message);
        }
        #[cfg(windows)]
        {
            let env = d.join("miniconda3").join("envs").join("sales");
            std::fs::create_dir_all(env.join("conda-meta")).unwrap();
            let conda_py = exe(&env, "python");
            let nf = resolve_with(&at(&env), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
            let want = format!(
                "[python] venv '{}' is a conda environment, not a virtual environment: run v2 where it is activated (conda activate sales) and leave venv out, or name its python instead: [engines.py] command = ['{}']",
                env.display(),
                conda_py.display()
            );
            assert_eq!(nf.message, want);
        }
        // The python itself, not in a venv: name it as the command.
        let nf = resolve_with(&at(&py), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.contains("is a file: venv names a virtual environment's folder; for a Python that isn't in one, write [engines.py] command = "), "{}", nf.message);
        // A folder of other things: nothing to create in it.
        let src = d.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("main.py"), "").unwrap();
        let nf = resolve_with(&at(&src), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with(&format!("and it isn't a virtual environment (it has no pyvenv.cfg): write the folder of one, or make one: {python} -m venv .venv")), "{}", nf.message);
        // A venv without the python this system needs (made on another one): make it again.
        let other = d.join("wslenv");
        std::fs::create_dir_all(other.join(if cfg!(windows) { "bin" } else { "Scripts" })).unwrap();
        std::fs::write(other.join("pyvenv.cfg"), "home = x\n").unwrap();
        let nf = resolve_with(&at(&other), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with(&format!(", so it was made on another system or is damaged: make it again here, {python} -m venv --clear {}", other.display())), "{}", nf.message);
        // A venv whose python is there but doesn't run: the Python it was made with is gone.
        let old = d.join("oldvenv");
        let bin = old.join(if cfg!(windows) { "Scripts" } else { "bin" });
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(old.join("pyvenv.cfg"), "home = C:\\Python39\n").unwrap();
        exe(&bin, "python");
        let gone = fake(|_| ProbeOutcome::Exited { code: Some(103), output: "No Python at '\"C:\\Python39\\python.exe'".into() });
        let nf = resolve_with(&at(&old), &lookup(&[], None), &gone, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with(&format!("; make the venv again, {python} -m venv --clear {}, and install its packages again", old.display())), "{}", nf.message);
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
        let head = "engine 'ts' (TypeScript) can't run — @ts blocks run on the js engine, and engine 'js' (JavaScript) is too old: found 22.6.0, V2 needs 22.13+, not 23.0 or 23.1\n";
        assert!(nf.message.starts_with(head), "{}", nf.message);
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
        let j = judge(&spec("py"), false, &ProbeOutcome::Killed(9), Some(PROBE_LIMIT));
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
        assert_eq!(nf.message, format!("engine 'ts' (TypeScript) can't run — @ts blocks run on the js engine, and {}", js.message));
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
