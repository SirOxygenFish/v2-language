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
    /// The variables the engines' programs read as they start that are set (8.4), for a probe
    /// that fails (7.8).
    pub startup: Vec<(&'static str, String)>,
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
            startup: ["py", "js"].into_iter().flat_map(crate::engines::spec::startup_variables).collect(),
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

/// Windows: Git for Windows' bash, as 7.6 finds the bash engine's: `bin\bash.exe` in the folder
/// above the one holding `git`, else `<program_files>\Git\bin\bash.exe`. Never a `bash` on PATH,
/// which on many machines is WSL's launcher (`System32\bash.exe`), running Linux.
pub fn git_bash(l: &Lookup, program_files: Option<&Path>) -> Option<PathBuf> {
    let beside_git = first_program("git", l).and_then(|git| Some(git.parent()?.parent()?.join("bin").join("bash.exe")));
    beside_git.into_iter().chain(program_files.map(|d| d.join("Git").join("bin").join("bash.exe"))).find(|p| p.is_file())
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
        let its = inside.map(|p| format!(", or point {who} at its python, {}", slashed(&p))).unwrap_or_default();
        let venv = crate::config::toml_string(&dir.display().to_string());
        return format!("; {who} names a virtual environment: write [python] venv = {venv} in v2.toml{its}");
    }
    match inside {
        Some(p) => format!("; {who} names a folder: point it at the program in it, {}", slashed(&p)),
        None => format!("; {who} names a folder: point it at the program, not at its folder"),
    }
}

/// A path to paste into `v2.toml` or a variable: on Windows with `/`, which is the same there and
/// is no escape in a TOML `"…"`, as `\n` in `"C:/Program Files/nodejs\node.exe"` is (7.8).
fn slashed(p: &Path) -> String {
    let text = p.display().to_string();
    if cfg!(windows) { text.replace('\\', "/") } else { text }
}

/// The skip reason of a `py` launcher left with no Python (7.6): like the placeholder, no Python
/// is installed, so the engine is missing, not broken (7.8).
const NO_RUNTIME: &str = "a py launcher with no Python installed";

/// The skip reason of a `.bat` or `.cmd` hit (pyenv-win's `shims\python.bat`) while V2's working
/// folder is a `\\server\share`, where cmd.exe would start the worker in the Windows folder (7.6):
/// never probed, so never cached, as the verdict depends on the working folder.
const BATCH_IN_SHARE: &str = r"runs in cmd.exe, which can't work in a \\server\share folder";

/// Windows: a `.bat` or `.cmd` program while the working folder `cwd` is a `\\server\share`.
fn batch_in_share(program: &Path, cwd: &Path) -> bool {
    program.extension().is_some_and(|e| e.eq_ignore_ascii_case("bat") || e.eq_ignore_ascii_case("cmd")) && share_folder(cwd).is_some()
}

/// What to do when the search passed over a batch file for `BATCH_IN_SHARE` (7.8, 19.2), in place
/// of repairing the program, which is fine.
pub fn batch_fix(id: &str, skipped: &[(PathBuf, String)]) -> Option<String> {
    let (exe, var) = if id == "py" { ("python.exe", "V2_ENGINE_PY") } else { ("node.exe", "V2_ENGINE_JS") };
    skipped.iter().any(|(_, why)| why == BATCH_IN_SHARE).then(|| format!("name the program itself ({exe}) with {var}, or run v2 from a folder on a drive"))
}

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
    if &*spec.id == "py" {
        for part in venv_stamps(path) {
            field(&part);
        }
    }
    Some(hex(&h.finalize()))
}

/// For a python in a venv (a `pyvenv.cfg` in its folder's parent): the stat of that `pyvenv.cfg`,
/// and the path and stat of the program its `home` names (`python.exe` there on Windows, else
/// `python3`), a missing one as a value of its own (7.7). A Windows venv's python is a copied
/// launcher whose own file doesn't change when that Python is uninstalled or `home` is edited.
/// Nothing for another python.
fn venv_stamps(python: &Path) -> Vec<Vec<u8>> {
    let Some(venv) = venv_of(python) else { return Vec::new() };
    let stamp = |p: &Path| match std::fs::metadata(p) {
        Ok(m) => {
            let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            [&m.len().to_le_bytes()[..], &mtime.to_le_bytes()[..]].concat()
        }
        Err(_) => b"missing".to_vec(),
    };
    let base = pyvenv_value(&venv, "home").map(|home| PathBuf::from(home).join(if cfg!(windows) { "python.exe" } else { "python3" }));
    let (path, stat) = match base.as_deref() {
        Some(b) => (b.as_os_str().as_encoded_bytes().to_vec(), stamp(b)),
        None => (Vec::new(), b"no home".to_vec()),
    };
    vec![stamp(&venv.join("pyvenv.cfg")), path, stat]
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

/// A `V2_ENGINE_<TAG>` value that is `~` or starts with `~/` (`~\`), which config joined to the
/// cwd as any relative path (18.4): what to do (7.6). After `~/`, the variable set with the path
/// written out, as a shell expands it in double quotes: `V2_ENGINE_PY="$HOME/bin/python3"`, on
/// Windows PowerShell's `$env:V2_ENGINE_PY = "$env:USERPROFILE\py\python.exe"`; a bare `~` (or
/// `~/`, `~\`), the home folder and no program, gets no example.
fn env_tilde(var: &str, name: &str) -> Option<String> {
    let p = Path::new(name);
    let cwd = std::env::current_dir().unwrap_or_default();
    let written = p.strip_prefix(&cwd).unwrap_or(p).to_string_lossy().into_owned();
    let what = format!("V2 doesn't expand ~ in {var}: write the path out");
    let rest = match written.strip_prefix('~')? {
        "" => "",
        r if r.starts_with(['/', '\\']) => &r[1..],
        _ => return None,
    };
    if rest.is_empty() {
        return Some(what);
    }
    let (home, sep) = if cfg!(windows) { ("$env:USERPROFILE", "\\") } else { ("$HOME", "/") };
    let path = format!("{home}{sep}{}", rest.replace(['/', '\\'], sep));
    Some(if cfg!(windows) { format!("{what}, as in $env:{var} = \"{path}\"") } else { format!("{what}, as in {var}=\"{path}\"") })
}

/// Windows: a working folder on a `\\server\share` (`\\wsl.localhost\Ubuntu\…`, also as
/// `\\?\UNC\…`), as cmd's `pushd` takes it. cmd.exe can't work in one (7.5, 7.6).
pub fn share_folder(cwd: &Path) -> Option<String> {
    use std::path::{Component, Prefix};
    match cwd.components().next() {
        Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..)) => Some(crate::syntax::span::strip_verbatim(cwd).display().to_string()),
        _ => None,
    }
}

/// What cmd.exe does in a `\\server\share` working folder (it says "UNC paths are not supported"
/// and works in the Windows folder instead), after `cmd.exe` or `… runs in cmd.exe, which`.
pub fn cmd_cant_work() -> String {
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    format!(r"can't work in a \\server\share folder and would run in {windows}")
}

/// Windows: a WSL link on a drive (a Linux venv made from WSL under /mnt/c), which Windows can't
/// even stat (`ERROR_CANT_ACCESS_FILE`), but which its folder lists as one (`module::wsl_file`).
fn wsl_link_on_drive(p: &Path) -> bool {
    let cant = std::fs::symlink_metadata(p).err().is_some_and(|e| cfg!(windows) && e.raw_os_error() == Some(1920));
    cant && crate::runtime::module::wsl_file(p) == Some(crate::runtime::module::WslFile::Link)
}

/// A path on WSL's share, `\\wsl.localhost\<distro>\…` or `\\wsl$\<distro>\…` (Windows).
fn on_wsl_share(p: &Path) -> bool {
    use std::path::{Component, Prefix};
    match p.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::UNC(server, _) | Prefix::VerbatimUNC(server, _) => server.eq_ignore_ascii_case("wsl.localhost") || server.eq_ignore_ascii_case("wsl$"),
            _ => false,
        },
        _ => false,
    }
}

/// Windows: a configured program that is there but that Windows can't start (7.6), so never
/// "not found": `Some(true)` for a Linux one (an ELF file, a WSL link Windows can't follow, any
/// file on WSL's share), `Some(false)` for another (a script, a text file). `None` when nothing is
/// there, or for a link Windows follows to nothing.
fn not_windows_program(p: &Path) -> Option<bool> {
    if !cfg!(windows) {
        return None;
    }
    if wsl_link_on_drive(p) {
        return Some(true);
    }
    let meta = std::fs::symlink_metadata(p).ok().filter(|m| !m.is_dir())?;
    match std::fs::metadata(p) {
        // A link: Windows' own reads back (and leads nowhere); WSL's doesn't read at all.
        Err(_) => (meta.file_type().is_symlink() && std::fs::read_link(p).is_err()).then_some(true),
        Ok(m) if m.is_dir() => None,
        Ok(_) => {
            let mut magic = [0u8; 4];
            let elf = std::fs::File::open(p).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut magic)).is_ok() && magic == *b"\x7fELF";
            Some(elf || on_wsl_share(p))
        }
    }
}

/// A configured program's path as messages and doctor show it: on Windows one that mixes `/` and
/// `\` (a `/usr/bin/python3` or `../x/python` joined to `v2.toml`'s folder, 18.1) with `\` only.
fn one_separator(name: &str) -> String {
    if cfg!(windows) && name.contains('/') && name.contains('\\') {
        name.replace('/', "\\")
    } else {
        name.to_string()
    }
}

/// `one_separator`, its control characters escaped (2.1).
fn shown_program(name: &str) -> String {
    crate::syntax::span::shown_path(&one_separator(name))
}

/// Windows: the WSL launcher a command runs (`wsl.exe`, or WSL's `bash.exe` in System32, not Git
/// for Windows'), whose Linux interpreter can't read the worker's adapter, a Windows path: what is
/// wrong and what to do, for doctor's notes and a crash at startup (7.6, 8.4, 19.2).
pub fn linux_interpreter_note(id: &str, argv: &[OsString]) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let program = Path::new(argv.first()?);
    let stem = program.file_stem()?.to_string_lossy().to_ascii_lowercase();
    let system = program.parent().and_then(Path::file_name).is_some_and(|d| ["system32", "sysnative"].iter().any(|s| d.eq_ignore_ascii_case(s)));
    let launcher = match stem.as_str() {
        "wsl" => "wsl.exe",
        "bash" if system => "WSL's bash.exe",
        _ => return None,
    };
    let (language, exe) = if id == "py" { ("python", "python.exe") } else { ("node", "node.exe") };
    Some(format!("{launcher} runs a Linux {language}, which the Windows v2 can't use, as its adapter is a Windows path: name a {exe}, or run the Linux v2 inside WSL"))
}

/// What "(set by …)" names: the file's full path, or the variable (7.6).
fn set_by(from: &ConfigSource) -> String {
    match from {
        ConfigSource::Project(p) => crate::syntax::span::shown_path(&p.display().to_string()),
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

    /// A `[python] venv` that can't be used: FROM says `venv`, as for one that works (19.2).
    fn broken_venv(message: String, command: String) -> NotFound {
        NotFound { status: Status::Broken, message, command: Some(command), from: Some(Origin::Venv.label()), skipped: Vec::new() }
    }
}

// ---------------------------------------------------------------------------------------------
// Virtual environments (7.6)

/// A `key = value` line of a venv's `pyvenv.cfg` (the key in any case, as `site` reads it).
fn pyvenv_value(venv: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(venv.join("pyvenv.cfg")).ok()?;
    text.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim().eq_ignore_ascii_case(key)).map(|(_, v)| v.trim().to_string())
}

/// The virtual environment `python` is in: the folder above its `bin` (`Scripts`) folder, when
/// that holds a `pyvenv.cfg`.
pub fn venv_of(python: &Path) -> Option<PathBuf> {
    python.parent()?.parent().filter(|v| v.join("pyvenv.cfg").is_file()).map(Path::to_path_buf)
}

/// Whether uv made the venv (its `pyvenv.cfg` has a `uv` line): it has no pip unless seeded, and
/// uv, not `-m venv`, makes it again.
pub fn made_by_uv(venv: &Path) -> bool {
    pyvenv_value(venv, "uv").is_some()
}

/// The uv project whose environment the venv is: it is the `.venv` of a folder that holds a
/// `pyproject.toml` and a `uv.lock`, the one `uv add` and `uv sync` use.
pub fn uv_project(venv: &Path) -> Option<PathBuf> {
    let project = venv.parent()?;
    let project_venv = venv.file_name().is_some_and(|n| n == ".venv") && project.join("pyproject.toml").is_file() && project.join("uv.lock").is_file();
    project_venv.then(|| project.to_path_buf())
}

/// Whether the venv has pip: a `pip` folder in its site-packages (`lib/python3.X/site-packages`,
/// Windows `Lib\site-packages`). True when V2 can't tell (no site-packages found).
pub fn venv_has_pip(venv: &Path) -> bool {
    let unix = std::fs::read_dir(venv.join("lib")).into_iter().flatten().flatten().map(|e| e.path().join("site-packages"));
    let sites: Vec<PathBuf> = std::iter::once(venv.join("Lib").join("site-packages")).chain(unix).filter(|s| s.is_dir()).collect();
    sites.is_empty() || sites.iter().any(|s| s.join("pip").is_dir())
}

/// How to make a broken venv again: uv's way for one uv made (`uv sync` in its project, else `uv
/// venv --clear`), else `python3 -m venv --clear` (`py` on Windows), the path pasteable.
pub fn remake_venv(venv: &Path) -> String {
    let dir = crate::cli::shell_word(&venv.display().to_string());
    if made_by_uv(venv) {
        return match uv_project(venv) {
            Some(project) => format!("make the venv again and install its packages: run uv sync in {}", crate::cli::shell_word(&project.display().to_string())),
            None => format!("make the venv again, uv venv --clear {dir}, and install its packages again"),
        };
    }
    let python = if cfg!(windows) { "py" } else { "python3" };
    format!("make the venv again, {python} -m venv --clear {dir}, and install its packages again")
}

/// A venv the other system made, which that system may still use (one project folder used from
/// Windows and from WSL), so never made again here (7.6): on Windows, one with a `bin/python` (or
/// `python3`) and no `bin/python.exe` (MSYS2's has one), or whose `pyvenv.cfg`'s `home` is a POSIX
/// path; elsewhere, one with a `Scripts/python.exe`, or a `home` with a drive letter or `\\`.
/// The rest of the message after the venv's name: where it was made, and a venv for this system
/// beside it, named in `V2_ENGINE_PY`.
pub fn foreign_venv(venv: &Path) -> Option<String> {
    let home = pyvenv_value(venv, "home").unwrap_or_default();
    let there = |p: PathBuf| std::fs::symlink_metadata(&p).is_ok() || wsl_link_on_drive(&p);
    let made_on = if cfg!(windows) {
        let bin = venv.join("bin");
        let posix = (there(bin.join("python")) || there(bin.join("python3"))) && !there(bin.join("python.exe"));
        (home.starts_with('/') || posix).then_some("Linux or macOS")?
    } else {
        let h = home.as_bytes();
        let drive = h.len() >= 3 && h[0].is_ascii_alphabetic() && h[1] == b':' && matches!(h[2], b'\\' | b'/');
        (drive || home.starts_with(r"\\") || is_regular_file(&venv.join("Scripts").join("python.exe"))).then_some("Windows")?
    };
    let this = if cfg!(windows) { "win" } else if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let new = venv.parent().unwrap_or(venv).join(format!(".venv-{this}"));
    let word = |p: &Path| crate::cli::shell_word(&p.display().to_string());
    let (python, set) = if cfg!(windows) {
        ("py", format!("$env:V2_ENGINE_PY = \"{}\"", new.join("Scripts").join("python.exe").display()))
    } else {
        ("python3", format!("export V2_ENGINE_PY={}", word(&new.join("bin").join("python"))))
    };
    Some(format!(
        "was made on {made_on}: keep it, make one for this system beside it, {python} -m venv {}, and name its python in V2_ENGINE_PY, which wins over [python] venv ({set}), or run V2 on that system",
        word(&new)
    ))
}

/// Where a link that leads nowhere was meant to lead: its last link's target, a relative one read
/// from that link's folder (a uv venv's `python3` links to `python`, which links to uv's Python).
fn dangling_target(link: &Path) -> Option<PathBuf> {
    let mut at = link.to_path_buf();
    let mut target = None;
    for _ in 0..40 {
        let Ok(next) = std::fs::read_link(&at) else { break };
        let next = at.parent().map_or_else(|| next.clone(), |dir| dir.join(&next));
        target = Some(next.clone());
        at = next;
    }
    target.filter(|_| !link.exists())
}

/// The major.minor a venv was made with: its `pyvenv.cfg`'s `version_info` (uv, virtualenv:
/// `3.12.3.final.0`), else `version` (`python -m venv`).
fn made_with(venv: &Path) -> Option<(u32, u32)> {
    let major_minor = |v: String| -> Option<(u32, u32)> {
        let mut parts = v.split('.');
        let major = parts.next()?.trim().parse().ok()?;
        let minor = parts.next()?;
        let digits = &minor[..minor.find(|c: char| !c.is_ascii_digit()).unwrap_or(minor.len())];
        Some((major, digits.parse().ok()?))
    };
    ["version_info", "version"].iter().find_map(|k| pyvenv_value(venv, k).and_then(major_minor))
}

/// A venv whose python now runs another Python than the one it was made with: it links to an
/// unversioned `python3` that was repointed (`uv python install --default --force`, a release
/// upgrade, Homebrew), so the packages in its `lib/python3.13` are gone for it. `who` begins the
/// message.
fn venv_mismatch(venv: &Path, probed: Version, who: &str) -> Option<String> {
    let (major, minor) = made_with(venv)?;
    ((major, minor) != (probed.0, probed.1)).then(|| {
        format!("{who} was made with Python {major}.{minor} (its pyvenv.cfg says so), but its python now runs {} (was Python upgraded?): {}", probed.short(), remake_venv(venv))
    })
}

/// The activated venv (`VIRTUAL_ENV`), when the search can't use its python: one that links to
/// nothing or is missing, which the search never meets, or one that is there but whose version
/// probe failed (among `skipped`, the search's: Windows' copied launcher, once the Python it runs
/// is uninstalled, says `No Python at '…'`). What is wrong and what to do, for doctor's note
/// (19.2) and an `ImportError`'s hint (13.5); `None` when its python is there and wasn't skipped
/// for that.
pub fn activated_venv_problem(venv: &Path, skipped: &[(PathBuf, String)]) -> Option<String> {
    let names: &[&str] = if cfg!(windows) { &["Scripts\\python.exe"] } else { &["bin/python3", "bin/python"] };
    let pythons: Vec<PathBuf> = names.iter().map(|n| venv.join(n)).collect();
    let who = format!("the activated venv {} (VIRTUAL_ENV)", crate::syntax::span::shown_path(&venv.display().to_string()));
    if pythons.iter().any(|p| is_regular_file(p)) {
        return failed_probe_in(venv, skipped).map(|said| format!("{who} has a python that doesn't run ({said}), so V2 can't use it: {}", remake_venv(venv)));
    }
    Some(match pythons.iter().find_map(|p| dangling_target(p)) {
        Some(target) => format!("{who} has a python that links to {}, which isn't there (was Python upgraded?), so V2 can't use it: {}", target.display(), remake_venv(venv)),
        None if !venv.is_dir() => {
            let python = if cfg!(windows) { "py" } else { "python3" };
            format!("{who} isn't there, so V2 can't use it: run deactivate, or make it again, {python} -m venv {}", crate::cli::shell_word(&venv.display().to_string()))
        }
        None => format!("{who} has no {}, so V2 can't use it: {}", if cfg!(windows) { r"Scripts\python.exe" } else { "bin/python" }, remake_venv(venv)),
    })
}

/// What the first hit of `skipped` in `venv`'s bin (Scripts) folder whose version probe failed
/// printed (`No Python at '…'`), else that reason (`version probe failed (exit code 103)`).
fn failed_probe_in(venv: &Path, skipped: &[(PathBuf, String)]) -> Option<String> {
    let canonical = std::fs::canonicalize(venv).ok();
    let same = |dir: &Path| dir == venv || (canonical.is_some() && std::fs::canonicalize(dir).ok() == canonical);
    let (_, why) = skipped.iter().find(|(p, why)| why.starts_with("version probe failed (") && p.parent().and_then(Path::parent).is_some_and(same))?;
    Some(why.split_once("): ").map_or(why.as_str(), |(_, said)| said).to_string())
}

/// Whether the Python a venv was made with (its `pyvenv.cfg`'s `home`) has `ensurepip`, which
/// Debian and Ubuntu leave out without python3-venv: the folder in its standard library,
/// `<home>/../lib/python3.X` (Windows `<home>\Lib`). True when V2 can't tell (no `home`, or no
/// standard library there).
pub fn base_has_ensurepip(venv: &Path) -> bool {
    let Some(home) = pyvenv_value(venv, "home").map(PathBuf::from) else { return true };
    let libs: Vec<PathBuf> = if cfg!(windows) {
        vec![home.join("Lib")]
    } else {
        let lib = home.parent().unwrap_or(&home).join("lib");
        match made_with(venv) {
            Some((major, minor)) => vec![lib.join(format!("python{major}.{minor}"))],
            None => std::fs::read_dir(&lib).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("python"))).collect(),
        }
    };
    let stdlibs: Vec<&PathBuf> = libs.iter().filter(|l| l.join("os.py").is_file()).collect();
    stdlibs.is_empty() || stdlibs.iter().any(|l| l.join("ensurepip").is_dir())
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
                let hint = if reason.starts_with("version probe timed out") && start > 0.0 && start < PROBE_LIMIT.as_secs_f64() {
                    // `startup_timeout = 2` and a message saying 10 s: probes never wait less.
                    format!("; a probe waits at least {} s: raise [engines.{}] startup_timeout above that if the program is only slow", PROBE_LIMIT.as_secs(), spec.id)
                } else if reason.starts_with("version probe timed out") {
                    format!("; raise [engines.{}] startup_timeout if it is only slow", spec.id)
                } else if reason == PLACEHOLDER {
                    // `V2_ENGINE_PY=python3`, a Linux habit, where Windows' Python is `python`:
                    // the one the search finds, named the way this one was (7.6). By its name
                    // only when that name finds it: a WindowsApps folder first on PATH has a
                    // placeholder for `python` too (and the configured name found one), so
                    // otherwise by its path.
                    match matches!(origin, Origin::Config(_)).then(|| self.search(spec).ok()).flatten() {
                        Some(found) => {
                            let program = PathBuf::from(&found.argv[0]);
                            let stem = program.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| first_program(s, self.lookup).as_ref() == Some(&program));
                            let name = stem.unwrap_or_else(|| program.display().to_string());
                            let named = match from {
                                ConfigSource::Env(var) => format!("{var}={name}"),
                                ConfigSource::Project(_) => {
                                    let words: Vec<String> = std::iter::once(name).chain(found.argv[1..].iter().map(|a| a.to_string_lossy().into_owned())).map(|w| crate::config::toml_string(&w)).collect();
                                    format!("[engines.{}] command = [{}]", spec.id, words.join(", "))
                                }
                            };
                            format!("; {} is installed as {}: name that one, as in {named}", spec.language, program.display())
                        }
                        None => format!("; install {} and name its full path, or leave command out: {}", spec.language, install_hint(&spec.id)),
                    }
                } else if let Some(fix) = library_fix(&path, reason) {
                    format!("\nfix: {fix}")
                } else if let Some(fix) = probe_fix(&spec.id, &[(path.clone(), reason.to_string())], &self.lookup.startup) {
                    format!("; {fix}")
                } else if origin == Origin::Venv {
                    // A venv's python that doesn't run: the Python it was made with is gone (an
                    // upgrade that removed it: Windows says "No Python at …").
                    format!("; {}", remake_venv(path.parent().and_then(Path::parent).unwrap_or(&path)))
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
                if origin == Origin::Venv {
                    nf.from = Some(origin.label());
                }
                Err(nf)
            }
        }
    }

    fn configured(&mut self, spec: &Rc<EngineSpec>, command: &[String], from: &ConfigSource) -> Result<ResolvedEngine, NotFound> {
        let name = &command[0];
        let Some(path) = first_program(name, self.lookup) else {
            let command_words = join(&std::iter::once(OsString::from(one_separator(name))).chain(command[1..].iter().map(OsString::from)).collect::<Vec<_>>());
            // Windows: a file there that isn't a Windows program, as a Linux python on WSL's share.
            let file: PathBuf = self.lookup.cwd.join(name).components().collect();
            if let Some(linux) = name.contains(['/', '\\']).then(|| not_windows_program(&file)).flatten() {
                let exe = if &*spec.id == "py" { "python.exe" } else { "node.exe" };
                let what = if linux {
                    format!("isn't a Windows program (a Linux one): the Windows v2 runs Windows programs: name a {exe}, or run the Linux v2 inside WSL")
                } else {
                    format!("isn't a Windows program (an .exe, .com, .bat or .cmd file): name a {exe}")
                };
                return Err(NotFound::broken(format!("engine '{}' command '{}' (set by {}) {what}", spec.id, shown_program(name), set_by(from)), command_words, from));
            }
            // `V2_ENGINE_PY="C:\Python311"`: a folder; `V2_ENGINE_PY="py -3"`: the variable holds
            // one program, never split (18.4).
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
                    let array: Vec<String> = std::iter::once(words[..k].join(" ")).chain(words[k..].iter().map(|w| w.to_string())).map(|w| crate::config::toml_string(&w)).collect();
                    format!(
                        "; {var} names one program, without arguments: for a command with arguments, write [engines.{}] command = [{}] in v2.toml",
                        spec.id,
                        array.join(", ")
                    )
                }
                // `V2_ENGINE_PY="~/bin/python3"`: in quotes a shell keeps the ~.
                ConfigSource::Env(var) if env_tilde(var, name).is_some() => format!("; {}", env_tilde(var, name).unwrap_or_default()),
                // `%ProgramFiles%\…` from PowerShell's single quotes, or a REG_SZ variable.
                _ if crate::config::unexpanded(name).is_some() => {
                    format!("; V2 doesn't expand {}: write the path out", crate::config::unexpanded(name).unwrap_or_default())
                }
                _ => String::new(),
            };
            // `"C:/Program Files/nodejs\node.exe"`: `\n` became a line break (18.5).
            let hint = crate::config::configured_escape_hint(Path::new(name), from, &spec.id).map_or(hint, |h| format!("; {h}"));
            return Err(NotFound::broken(format!("engine '{}' command '{}' not found (set by {}){hint}", spec.id, shown_program(name), set_by(from)), command_words, from));
        };
        // Windows: a .bat or .cmd (pyenv-win's shim, conda.bat) runs in cmd.exe, which in a
        // \\server\share working folder would start the worker in the Windows folder.
        let batch = batch_in_share(&path, &self.lookup.cwd);
        let mut argv = vec![path.into_os_string()];
        argv.extend(command[1..].iter().map(OsString::from));
        if batch {
            let exe = if &*spec.id == "py" { "python.exe" } else { "node.exe" };
            let message = format!(
                "engine '{}' command '{}' (set by {}) runs in cmd.exe, which {}: name the program itself ({exe}), or run v2 from a folder on a drive",
                spec.id,
                shown_program(&argv[0].to_string_lossy()),
                set_by(from),
                cmd_cant_work()
            );
            return Err(NotFound::broken(message, join(&argv), from));
        }
        self.usable(spec, argv, Origin::Config(from.clone()), &format!("command '{}'", shown_program(name)), from)
    }

    fn venv(&mut self, spec: &Rc<EngineSpec>, venv: &Path, from: &ConfigSource) -> Result<ResolvedEngine, NotFound> {
        let candidates: Vec<PathBuf> = if cfg!(windows) {
            vec![venv.join("Scripts").join("python.exe")]
        } else {
            vec![venv.join("bin").join("python"), venv.join("bin").join("python3")]
        };
        // Only a virtual environment (a pyvenv.cfg) is used: a conda environment or a Python
        // installation (`/usr`, a pyenv version) has a bin/python too, and gets the messages below
        // on every system.
        let Some(exe) = candidates.iter().find(|p| is_regular_file(p)).filter(|_| venv.join("pyvenv.cfg").is_file()) else {
            let want = if cfg!(windows) { r"Scripts\python.exe" } else { "bin/python" };
            let shown = crate::syntax::span::shown_path(&venv.display().to_string());
            // `venv = "C:/work\new"`: `\n` became a line break, so no venv to make there (18.5).
            if let Some(h) = crate::config::configured_escape_hint(venv, from, "venv") {
                return Err(NotFound::broken(format!("[python] venv '{shown}' has no {want}: {h}"), candidates[0].display().to_string(), from));
            }
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
            // A venv's python (Windows: Scripts\python.exe, a launcher that can't run without its
            // pyvenv.cfg) or, elsewhere, a bin/activate, but no pyvenv.cfg: a damaged venv, to make
            // again, not a Python installation (python.exe at the root, bin/python3 without
            // activate) nor a conda environment, whose base has a bin/activate too.
            let damaged = |dir: &Path| {
                let venv_files = if cfg!(windows) {
                    is_regular_file(&dir.join("Scripts").join("python.exe")) && !is_regular_file(&dir.join("python.exe"))
                } else {
                    is_regular_file(&dir.join("bin").join("activate"))
                };
                venv_files && !is_venv(dir) && !dir.join("conda-meta").is_dir()
            };
            // A conda environment's or an installation's Python (`C:\Program Files\Python311`,
            // `/usr`): never a place to run `-m venv` in, which would turn it into a venv of
            // another Python. Nor is a folder that holds anything else.
            let installed = |dir: &Path| -> Option<PathBuf> {
                let names: &[&str] = if cfg!(windows) { &["python.exe"] } else { &["bin/python3", "bin/python"] };
                names.iter().map(|n| dir.join(n)).find(|p| is_regular_file(p)).filter(|_| !is_venv(dir) && !damaged(dir))
            };
            let free = |dir: &Path| std::fs::read_dir(dir).map_or(true, |mut d| d.next().is_none());
            let command = |p: &Path| {
                let p = p.display().to_string();
                if p.contains('\\') { format!("[engines.py] command = ['{p}']") } else { format!("[engines.py] command = [\"{p}\"]") }
            };
            let not_a_venv = |what: &str, py: &Path| {
                format!("[python] venv '{shown}' is {what}, not a virtual environment: name its python instead, {}, or make a virtual environment with it: {} -m venv .venv", command(py), pasted(py))
            };
            // The bin (Scripts) folder or the python of a damaged venv: that venv, made again.
            let in_damaged = |what: &str, folder: &Path| {
                format!("[python] venv '{shown}' is {what} a virtual environment that has no pyvenv.cfg, so it is damaged: make it again, {python} -m venv --clear {}, and write {}", pasted(folder), write(folder))
            };
            // Its python links to one that is gone, as after the system's Python was upgraded.
            let dangling = candidates.iter().find_map(|c| dangling_target(c));
            let message = if let Some(target) = dangling {
                format!("[python] venv '{shown}' has a python that links to {}, which isn't there (was Python upgraded?): {}", target.display(), remake_venv(venv))
            } else if is_regular_file(venv) {
                // `.venv/bin/python`, `.venv\Scripts\python.exe`: the interpreter, not its folder.
                let above = venv.parent().and_then(Path::parent);
                match above.filter(|f| is_venv(f)) {
                    Some(folder) => format!("[python] venv '{shown}' is a file: venv names the virtual environment's folder: write {}", write(folder)),
                    // Its launcher (Windows), which can't run without pyvenv.cfg: no command to name.
                    None if above.is_some_and(damaged) => in_damaged("a file in", above.unwrap_or(venv)),
                    // A Python's name (python3, python.exe, pypy3): that program may be meant.
                    None if venv.file_name().is_some_and(|f| f.to_string_lossy().to_ascii_lowercase().starts_with("py")) => {
                        format!("[python] venv '{shown}' is a file: venv names a virtual environment's folder; for a Python that isn't in one, write {} instead of venv", command(venv))
                    }
                    None => format!("[python] venv '{shown}' is a file: venv names a virtual environment's folder, as in venv = \".venv\""),
                }
            } else if let Some(folder) = inside.filter(|f| is_venv(f)) {
                format!("[python] venv '{shown}' is inside a virtual environment, and venv names the environment's folder: write {}", write(folder))
            } else if damaged(venv) {
                format!("[python] venv '{shown}' has no pyvenv.cfg, so it is damaged: make it again, {python} -m venv --clear {}", pasted(venv))
            } else if let Some(folder) = inside.filter(|f| damaged(f)) {
                in_damaged("inside", folder)
            } else if let Some(py) = installed(venv) {
                if venv.join("conda-meta").is_dir() {
                    // The base environment (Miniforge's own folder, with condabin) is `base`.
                    let name = if venv.join("condabin").is_dir() { "base".to_string() } else { venv.file_name().map_or_else(|| shown.clone(), |n| n.to_string_lossy().into_owned()) };
                    format!("[python] venv '{shown}' is a conda environment, not a virtual environment: run v2 where it is activated (conda activate {name}) and leave venv out, or name its python instead: {}", command(&py))
                } else {
                    not_a_venv("a Python installation", &py)
                }
            } else if let Some(py) = inside.and_then(installed) {
                not_a_venv("a folder of a Python installation", &py)
            } else if let Some(folder) = inside {
                let create = if !free(folder) { String::new() } else { format!(" (create it: {python} -m venv {})", pasted(folder)) };
                format!("[python] venv '{shown}' is inside a virtual environment, and venv names the environment's folder: write {}{create}", write(folder))
            } else if let Some(advice) = foreign_venv(venv).filter(|_| is_venv(venv)) {
                // Made on the other system, which may still use it: kept.
                format!("[python] venv '{shown}' {advice}")
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
            // Every path in it with its control characters escaped, the commands to paste too (2.1):
            // each of these messages is one line.
            return Err(NotFound::broken_venv(crate::syntax::span::shown_path(&message), shown_command));
        };
        let what = format!("[python] venv '{}'", venv.display());
        let e = self.usable(spec, vec![exe.clone().into_os_string()], Origin::Venv, &what, from)?;
        // Its python runs, but another Python than the venv's packages are for.
        let who = format!("[python] venv '{}'", crate::syntax::span::shown_path(&venv.display().to_string()));
        match e.version.and_then(|v| venv_mismatch(venv, v, &who)) {
            Some(message) => Err(NotFound::broken_venv(message, exe.display().to_string())),
            None => Ok(e),
        }
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
                // Windows: pyenv-win's shims\python.bat from a \\server\share folder, where cmd.exe
                // would start the worker in the Windows folder: installed, but not usable here.
                if batch_in_share(&hit, &self.lookup.cwd) {
                    failed = true;
                    skipped.push((hit, BATCH_IN_SHARE.into()));
                    continue;
                }
                let mut argv = vec![hit.clone().into_os_string()];
                argv.extend(candidate[1..].iter().map(|a| OsString::from(&**a)));
                match self.probe(spec, &argv, &hit, Some(PROBE_LIMIT)) {
                    Judged::Usable(v) => {
                        // An activated venv's python that now runs another Python than the venv
                        // was made with: broken, as a [python] venv would be, not passed over (a
                        // node in a venv's bin, from a pip package, has no say).
                        let who = |venv: &Path| format!("the venv {} on PATH", crate::syntax::span::shown_path(&venv.display().to_string()));
                        let venv = venv_of(&hit).filter(|_| &*spec.id == "py");
                        if let Some(message) = venv.and_then(|venv| venv_mismatch(&venv, v, &who(&venv))) {
                            return Err(NotFound { status: Status::Broken, message, command: Some(join(&argv)), from: Some(Origin::Path.label()), skipped });
                        }
                        // A program that didn't answer in time cost this run its wait, and will
                        // cost the next: said once, as doctor isn't what the user ran (7.6).
                        if self.cache.mode == CacheMode::ReadWrite {
                            for (slow, _) in skipped.iter().filter(|(_, why)| why.starts_with("version probe timed out")) {
                                stderrln!(
                                    "note: {} didn't answer --version in {} s, so V2 used {} (v2 doctor lists what it skips)",
                                    slow.display(),
                                    PROBE_LIMIT.as_secs(),
                                    hit.display()
                                );
                            }
                        }
                        tilde_skips(spec, &path, Some(dir.as_path()), home().as_deref(), &seen, &mut skipped);
                        return Ok(ResolvedEngine { spec: spec.clone(), origin: Origin::Path, argv, version: Some(v), skipped });
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
        tilde_skips(spec, &path, None, home().as_deref(), &seen, &mut skipped);
        let status = match newest_old {
            Some(v) => Status::Old(v),
            None if failed => Status::Broken,
            None => Status::Missing,
        };
        let mut message = not_found_message(spec, &skipped, newest_old, failed, &self.lookup.startup);
        // `env -i v2 x.v2`: nothing to search (7.5).
        if self.lookup.path.as_ref().is_none_or(|p| p.is_empty()) {
            message = message.replacen(" on PATH", " on PATH, which isn't set", 1);
            // It may well be installed (cron, `env -i`): what fixes it is a PATH or a name (round 16).
            let key = if &*spec.id == "ts" { "js" } else { &*spec.id };
            let fix = format!("fix: set PATH, or name the program with V2_ENGINE_{} or in v2.toml, [engines.{key}] command", key.to_ascii_uppercase());
            message = message
                .lines()
                .filter(|l| !l.starts_with("or set [engines."))
                .map(|l| if l.starts_with("install:") { fix.as_str() } else { l })
                .collect::<Vec<_>>()
                .join("\n");
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

/// The home folder a PATH entry's `~` stands for in bash (7.5); `None` when `HOME` isn't set.
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// Unix: `PATH` entries that start with `~` (`~/.nvm/versions/node/v24/bin`, from a quoted
/// `export PATH="~/…:$PATH"`) are relative, so the walk skips them, as execvp, dash and Python do;
/// only bash's own lookup expands them, so the shell runs another program than V2 (7.5). One whose
/// folder, `~` expanded, holds a program the engine looks for is added to `skipped`, with what to
/// write instead: those before `upto`, the folder of the program V2 uses, or all when there is
/// none, unless the walk met that file already (`seen`).
fn tilde_skips(spec: &EngineSpec, path: &std::ffi::OsStr, upto: Option<&Path>, home: Option<&Path>, seen: &std::collections::HashSet<(PathBuf, Vec<String>)>, skipped: &mut Vec<(PathBuf, String)>) {
    if !cfg!(unix) {
        return;
    }
    let raw: Vec<PathBuf> = std::env::split_paths(path).collect();
    let end = upto.and_then(|d| raw.iter().position(|r| r == d)).unwrap_or(raw.len());
    for entry in &raw[..end] {
        let text = entry.to_string_lossy();
        let Some((folder, write)) = expand_tilde(&text, home) else { continue };
        let found = spec.candidates.iter().filter(|c| c.len() == 1).map(|c| (folder.join(&*c[0]), &c[0])).find(|(p, _)| is_regular_file(p) && executable(p));
        let Some((program, name)) = found else { continue };
        let file = std::fs::canonicalize(&program).unwrap_or(program);
        let shown = entry.join(&**name);
        if seen.contains(&(file, Vec::new())) || skipped.iter().any(|(p, _)| *p == shown) {
            continue;
        }
        skipped.push((shown, format!("its PATH entry starts with ~, which only bash expands: write {write} in your startup file")));
    }
}

/// `~/x` → (`$HOME/x` expanded, `$HOME/x`), `~` → `$HOME`, `~user/x` → that user's home joined
/// (written as the path) (7.5); `None` for an entry without `~`, or a home that isn't known.
fn expand_tilde(entry: &str, home: Option<&Path>) -> Option<(PathBuf, String)> {
    let rest = entry.strip_prefix('~')?;
    let (user, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let base = if user.is_empty() { home?.to_path_buf() } else { home_of(user)? };
    let written = if user.is_empty() { "$HOME".to_string() } else { base.display().to_string() };
    let tail = tail.trim_end_matches('/');
    Some(if tail.is_empty() { (base, written) } else { (base.join(tail), format!("{written}/{tail}")) })
}

/// The home folder of the user `name` (`~name`), from the user database.
#[cfg(unix)]
fn home_of(name: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(name).ok()?;
    // SAFETY: a zeroed passwd is a valid out-parameter.
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 16 * 1024];
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: getpwnam_r fills `pw`, its strings in `buf`, and sets `out` to `&pw` when found.
    let rc = unsafe { libc::getpwnam_r(name.as_ptr(), &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) };
    if rc != 0 || out.is_null() || pw.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir points to a NUL-terminated string in `buf`, which is still alive.
    let dir = unsafe { std::ffi::CStr::from_ptr(pw.pw_dir) };
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
}

#[cfg(not(unix))]
fn home_of(_: &str) -> Option<PathBuf> {
    None
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

/// When a version probe failed (the program exited with an error, or a signal stopped it) while
/// a variable that the probe reads is set (`NODE_OPTIONS`): that variable, not the program, is
/// the likely cause (7.8). `set`: the startup variables that are set.
pub fn probe_fix(id: &str, skipped: &[(PathBuf, String)], set: &[(&'static str, String)]) -> Option<String> {
    let reads = crate::engines::spec::probe_variables(id);
    let vars: Vec<(&str, String)> = set.iter().filter(|(n, _)| reads.contains(n)).cloned().collect();
    let failed = skipped.iter().any(|(p, why)| why.starts_with("version probe failed (") && library_fix(p, why).is_none());
    (failed && !vars.is_empty()).then(|| crate::engines::spec::startup_fix(id, &vars))
}

/// Doctor (19.2): with a startup variable or an address-space limit set, whether the program
/// really starts (Python importing what V2's worker imports, `node -e 0`), since `--version`
/// answers before Python reads `PYTHONHOME` or Node runs a preload (8.4). `None` when it starts,
/// or when the check can't tell (it timed out, or couldn't start at all); else what failed and
/// how it ended, `-e 0 failed (exit code 1): `, and the first line that names an error.
pub fn start_failure(prober: &dyn Prober, spec: &EngineSpec, argv: &[OsString]) -> Option<String> {
    let args = crate::engines::spec::start_args(&spec.id);
    if args.is_empty() {
        return None;
    }
    let mut full = argv.to_vec();
    full.extend(args.iter().map(OsString::from));
    let (code, signal, output) = match prober.probe(&full, &Resolver::env(spec), Some(PROBE_LIMIT)) {
        ProbeOutcome::Exited { code: Some(0), .. } | ProbeOutcome::TimedOut | ProbeOutcome::SpawnFailed(_) => return None,
        ProbeOutcome::Killed(sig) => (None, Some(sig), String::new()),
        ProbeOutcome::Exited { code, output } => (code, None, output),
    };
    // Python first prints its path configuration, Node the line of its loader that threw. A
    // traceback's exception is its first line after it that isn't indented (a frame, or the
    // source line of one, `raise ImportError(…)`, is).
    let lines = || output.lines().map(str::trim).filter(|l| !l.is_empty());
    let raised = output.lines().skip_while(|l| !l.starts_with("Traceback (most recent call last)")).skip(1).find(|l| !l.starts_with(char::is_whitespace) && !l.is_empty());
    let said = raised.map(str::trim).or_else(|| lines().find(|l| l.contains("rror"))).or_else(|| lines().next());
    let said = said.map(|l| format!(": {}", l.chars().take(160).collect::<String>())).unwrap_or_default();
    Some(format!("{} ({}){said}", crate::engines::spec::start_check_failed(&spec.id), ExitInfo { code, signal }.describe()))
}

/// 7.8's message: what was found (nothing, or only versions that are too old), what was tried,
/// what was skipped and why, how to install, how to configure. `set`: the startup variables that
/// are set (8.4).
fn not_found_message(spec: &EngineSpec, skipped: &[(PathBuf, String)], newest_old: Option<Version>, failed: bool, set: &[(&'static str, String)]) -> String {
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
        m.push_str(&format!("\nskipped: {} ({reason})", crate::syntax::span::shown_path(&path.display().to_string())));
    }
    for fix in skipped.iter().filter_map(|(path, reason)| library_fix(path, reason)) {
        m.push_str(&format!("\nfix: {fix}"));
    }
    if let Some(fix) = batch_fix(&spec.id, skipped) {
        m.push_str(&format!("\nfix: {fix}"));
    }
    // Before the install line: NODE_OPTIONS, not Node.js, is the likely cause.
    if let Some(fix) = probe_fix(&spec.id, skipped, set) {
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
            startup: Vec::new(),
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
        // Round 19 (L9): FROM says venv, as for a venv that works.
        assert_eq!((nf.status, nf.from.as_deref()), (Status::Broken, Some("venv")));
    }

    /// A venv `dir/name` with this pyvenv.cfg and a python where this system's venvs have one.
    fn fake_venv(dir: &Path, name: &str, cfg: &str) -> (PathBuf, PathBuf) {
        let venv = dir.join(name);
        let bin = venv.join(if cfg!(windows) { "Scripts" } else { "bin" });
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), cfg).unwrap();
        (venv, exe(&bin, if cfg!(windows) { "python" } else { "python3" }))
    }

    fn venv_config(venv: &Path) -> Config {
        Config { python_venv: Some(Sourced { value: venv.to_path_buf(), from: project_source() }), ..Config::default() }
    }

    #[test]
    fn a_venv_whose_python_now_runs_another_version_is_broken() {
        // Round 19 (L0): the venv's pyvenv.cfg says 3.13, but its python links to an unversioned
        // python3 that now runs 3.11, so its packages are gone for it: broken, not ok with a pip
        // line that can't work. FROM says venv (L9).
        let d = temp("mismatch");
        let (venv, python) = fake_venv(&d, ".venv", "home = /usr/bin\nversion = 3.13.18\n");
        let p = fake(|_| says("Python 3.11.18"));
        let nf = resolve_with(&venv_config(&venv), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        let py = if cfg!(windows) { "py" } else { "python3" };
        let made = |who: String| {
            format!(
                "{who} was made with Python 3.13 (its pyvenv.cfg says so), but its python now runs 3.11 (was Python upgraded?): make the venv again, {py} -m venv --clear {}, and install its packages again",
                venv.display()
            )
        };
        assert_eq!(nf.message, made(format!("[python] venv '{}'", venv.display())));
        assert_eq!((nf.status, nf.command.clone(), nf.from.as_deref()), (Status::Broken, Some(python.display().to_string()), Some("venv")));
        // Another patch release is the same Python.
        let e = resolve_with(&venv_config(&venv), &lookup(&[], None), &fake(|_| says("Python 3.13.2")), &mut ProbeCache::off(), "py").unwrap();
        assert_eq!(e.origin, Origin::Venv);
        // version_info (uv's, virtualenv's 3.11.18.final.0) is read before version; none: used.
        for cfg in ["version_info = 3.11.18.final.0\nversion = 3.13.18\n", "home = /usr/bin\n"] {
            std::fs::write(venv.join("pyvenv.cfg"), cfg).unwrap();
            assert!(resolve_with(&venv_config(&venv), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").is_ok(), "{cfg}");
        }
        // The same venv found on PATH (an activated one): named by its folder, FROM PATH.
        std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\nversion = 3.13.18\n").unwrap();
        let bin = python.parent().unwrap();
        let nf = resolve_with(&Config::default(), &lookup(&[bin], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.message, made(format!("the venv {} on PATH", venv.display())));
        assert_eq!((nf.status, nf.command, nf.from.as_deref()), (Status::Broken, Some(python.display().to_string()), Some("PATH")));
        // A node in a venv's bin (a pip package's) has no say.
        exe(bin, "node");
        assert!(resolve_with(&Config::default(), &lookup(&[bin], None), &fake(|_| says("v24.11.1")), &mut ProbeCache::off(), "js").is_ok());
    }

    #[test]
    fn a_venv_uv_made_is_made_again_with_uv() {
        // Round 19 (L1): uv's venvs (a `uv` line in pyvenv.cfg) are made again with uv: -m venv
        // would make one without the lockfile's packages.
        let d = temp("uvvenv");
        let (venv, _) = fake_venv(&d, ".venv", "uv = 0.13.8\nversion_info = 3.13.18\n");
        let p = fake(|_| says("Python 3.11.18"));
        let nf = resolve_with(&venv_config(&venv), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with(&format!("(was Python upgraded?): make the venv again, uv venv --clear {}, and install its packages again", venv.display())), "{}", nf.message);
        // A uv project's .venv (a pyproject.toml and uv.lock beside it): uv sync there.
        std::fs::write(d.join("pyproject.toml"), "").unwrap();
        std::fs::write(d.join("uv.lock"), "").unwrap();
        let sync = format!("make the venv again and install its packages: run uv sync in {}", d.display());
        let nf = resolve_with(&venv_config(&venv), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with(&format!("(was Python upgraded?): {sync}")), "{}", nf.message);
        // A python that doesn't run, the same way.
        let gone = fake(|_| ProbeOutcome::Exited { code: Some(103), output: "No Python at 'C:\\uv\\python.exe'".into() });
        let nf = resolve_with(&venv_config(&venv), &lookup(&[], None), &gone, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with(&format!("; {sync}")), "{}", nf.message);
        assert_eq!(nf.from.as_deref(), Some("venv"));
        // Another folder name than .venv: uv sync wouldn't make that one.
        let (other, _) = fake_venv(&d, "env2", "uv = 0.13.8\n");
        assert_eq!(remake_venv(&other), format!("make the venv again, uv venv --clear {}, and install its packages again", other.display()));
    }

    #[cfg(unix)]
    #[test]
    fn a_uv_venv_whose_python_is_gone_says_uv_venv() {
        // Round 19 (L1, L4): `uv python uninstall` leaves python linking to nothing, and python3
        // linking to python: the message names where the chain was meant to end.
        let d = temp("uvgone");
        let venv = d.join(".venv");
        let bin = venv.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "uv = 0.13.8\nversion_info = 3.13.18\n").unwrap();
        let gone = "/nonexistent/uv/cpython-3.13.99/bin/python3.13";
        std::os::unix::fs::symlink(gone, bin.join("python")).unwrap();
        std::os::unix::fs::symlink("python", bin.join("python3")).unwrap();
        let again = format!("make the venv again, uv venv --clear {}, and install its packages again", venv.display());
        let nf = resolve_with(&venv_config(&venv), &lookup(&[], None), &fake(|_| says("Python 3.13.1")), &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.message, format!("[python] venv '{}' has a python that links to {gone}, which isn't there (was Python upgraded?): {again}", venv.display()));
        // Activated (VIRTUAL_ENV), the search passes it over: doctor and the hint say so.
        assert_eq!(
            activated_venv_problem(&venv, &[]),
            Some(format!("the activated venv {} (VIRTUAL_ENV) has a python that links to {gone}, which isn't there (was Python upgraded?), so V2 can't use it: {again}", venv.display()))
        );
    }

    #[test]
    fn an_activated_venv_without_its_python_is_named() {
        // Round 19 (L4): VIRTUAL_ENV names a venv whose python is missing (or the venv itself).
        let d = temp("activated");
        let (venv, python) = fake_venv(&d, ".venv", "home = x\n");
        assert_eq!(activated_venv_problem(&venv, &[]), None, "its python is there");
        std::fs::remove_file(&python).unwrap();
        let (py, want) = if cfg!(windows) { ("py", r"Scripts\python.exe") } else { ("python3", "bin/python") };
        let who = format!("the activated venv {} (VIRTUAL_ENV)", venv.display());
        assert_eq!(
            activated_venv_problem(&venv, &[]),
            Some(format!("{who} has no {want}, so V2 can't use it: make the venv again, {py} -m venv --clear {}, and install its packages again", venv.display()))
        );
        std::fs::remove_dir_all(&venv).unwrap();
        assert_eq!(activated_venv_problem(&venv, &[]), Some(format!("{who} isn't there, so V2 can't use it: run deactivate, or make it again, {py} -m venv {}", venv.display())));
    }

    #[test]
    fn an_activated_venv_whose_python_doesnt_run_is_named() {
        // Round 19 review (P1): a Windows venv's python.exe is a copied launcher, there after the
        // Python it was made with is uninstalled, but its probe fails ("No Python at", exit 103),
        // so the search passes it over and finds another Python: the skipped list says so.
        let d = temp("activated-fails");
        let (venv, python) = fake_venv(&d, ".venv", "home = C:\\nonexistent\\Python311\n");
        let other = d.join("Python312");
        std::fs::create_dir_all(&other).unwrap();
        let found = exe(&other, "python");
        let said = "No Python at '\"C:\\nonexistent\\Python311\\python.exe'";
        let venv2 = venv.clone();
        let p = fake(move |prog| if prog.starts_with(&venv2) { ProbeOutcome::Exited { code: Some(103), output: format!("{said}\n") } } else { says("Python 3.12.1") });
        let e = resolve_with(&Config::default(), &lookup(&[python.parent().unwrap(), &other], None), &p, &mut ProbeCache::off(), "py").unwrap();
        assert_eq!((e.origin.clone(), e.argv.clone()), (Origin::Path, vec![found.into_os_string()]));
        assert_eq!(e.skipped, [(python.clone(), format!("version probe failed (exit code 103): {said}"))]);
        let py = if cfg!(windows) { "py" } else { "python3" };
        let again = format!("make the venv again, {py} -m venv --clear {}, and install its packages again", venv.display());
        let who = format!("the activated venv {} (VIRTUAL_ENV)", venv.display());
        assert_eq!(activated_venv_problem(&venv, &e.skipped), Some(format!("{who} has a python that doesn't run ({said}), so V2 can't use it: {again}")));
        // A failed probe that printed nothing: its reason.
        let quiet = [(python.clone(), "version probe failed (exit code 103)".to_string())];
        assert_eq!(activated_venv_problem(&venv, &quiet), Some(format!("{who} has a python that doesn't run (version probe failed (exit code 103)), so V2 can't use it: {again}")));
        // Not skipped, skipped for another reason (a slow probe may pass next time), or another
        // venv's python: nothing to say.
        assert_eq!(activated_venv_problem(&venv, &[]), None);
        assert_eq!(activated_venv_problem(&venv, &[(python.clone(), "version probe timed out (10 s)".to_string())]), None);
        let (elsewhere, elsewhere_py) = fake_venv(&d, "env2", "home = x\n");
        assert_eq!(activated_venv_problem(&venv, &[(elsewhere_py.clone(), format!("version probe failed (exit code 103): {said}"))]), None);
        assert!(activated_venv_problem(&elsewhere, &[(elsewhere_py, format!("version probe failed (exit code 103): {said}"))]).is_some());
    }

    #[test]
    fn a_venv_pythons_cache_entry_follows_its_pyvenv_cfg_and_base_python() {
        // Round 19 review (P1): a Windows venv's python is a copied launcher whose file doesn't
        // change when the Python its pyvenv.cfg's home names is uninstalled or home is edited, so
        // those key its cache entry too (7.7): a stale "usable" entry made the run's worker exit
        // with 103 (EngineCrashed) instead of a probe that says what is wrong.
        let home = temp("venvcache-home");
        let d = temp("venvcache");
        let base = d.join("Python311");
        std::fs::create_dir_all(&base).unwrap();
        let base_py = exe(&base, if cfg!(windows) { "python" } else { "python3" });
        let (venv, _) = fake_venv(&d, ".venv", &format!("home = {}\n", base.display()));
        let p = fake(|_| says("Python 3.11.6"));
        let run = || {
            let mut cache = ProbeCache::open(Some(&home), CacheMode::ReadWrite);
            resolve_with(&venv_config(&venv), &lookup(&[], None), &p, &mut cache, "py").unwrap();
            cache.save();
            p.calls.get()
        };
        assert_eq!(run(), 1);
        assert_eq!(run(), 1, "served from the cache");
        // The Python it was made with is uninstalled: probed again (and then cached as missing).
        std::fs::remove_file(&base_py).unwrap();
        assert_eq!(run(), 2);
        assert_eq!(run(), 2, "a missing home python is a value of its own");
        // pyvenv.cfg's home edited to another Python (the same length: the program's path, not
        // the file's mtime, which a coarse clock may leave as it was, tells).
        let other = d.join("Python312");
        std::fs::create_dir_all(&other).unwrap();
        exe(&other, if cfg!(windows) { "python" } else { "python3" });
        std::fs::write(venv.join("pyvenv.cfg"), format!("home = {}\n", other.display())).unwrap();
        assert_eq!(run(), 3);
        // Any other change to pyvenv.cfg.
        std::fs::write(venv.join("pyvenv.cfg"), format!("home = {}\nversion = 3.11.6\n", other.display())).unwrap();
        assert_eq!(run(), 4);
        assert_eq!(run(), 4);
        // A python that isn't in a venv keeps the key it had.
        assert!(venv_stamps(&base_py).is_empty());
    }

    #[test]
    fn an_unset_path_is_named() {
        // 7.8: `env -i v2 x.v2` has no PATH to search, and the message says so.
        let p = fake(|_| says("Python 3.12.1"));
        let nf = resolve_with(&Config::default(), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.contains(" on PATH, which isn't set\n"), "{}", nf.message);
        // Round 16: no install line for what may be installed, but how to point V2 at it.
        assert!(!nf.message.contains("install:"), "{}", nf.message);
        assert!(nf.message.ends_with("fix: set PATH, or name the program with V2_ENGINE_PY or in v2.toml, [engines.py] command"), "{}", nf.message);
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
        let l = Lookup { path: Some(path), pathext: None, windows_apps: None, cwd: std::env::current_dir().unwrap(), xcrun_shims: Vec::new(), startup: Vec::new() };
        assert_eq!(find_program("node", &l), [good]);
        let _ = std::fs::remove_dir_all(&rel);
    }

    #[test]
    fn the_first_program_stops_the_walk() {
        let (a, b) = (temp("first-a"), temp("first-b"));
        let (first, second) = (exe(&a, "tool"), exe(&b, "tool"));
        let path = std::env::join_paths([a, b]).unwrap();
        let l = Lookup { path: Some(path), pathext: None, windows_apps: None, cwd: std::env::current_dir().unwrap(), xcrun_shims: Vec::new(), startup: Vec::new() };
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

    #[test]
    fn a_probe_stopped_by_node_options_names_it() {
        // Round 18, 7.8: Node reads NODE_OPTIONS before it answers --version, so a failed probe
        // with it set is that variable's doing; Python answers before it reads PYTHONHOME.
        let d = temp("nodeopts");
        let node = exe(&d, "node");
        exe(&d, "python3");
        let bad = ProbeOutcome::Exited { code: Some(9), output: "node: --bogus-flag is not allowed in NODE_OPTIONS\n".into() };
        let p = fake(move |_| bad.clone());
        let set = vec![("NODE_OPTIONS", "--bogus-flag".to_string()), ("PYTHONHOME", "/nope".to_string())];
        let l = Lookup { startup: set.clone(), ..lookup(&[&d], None) };
        let fix = "\nfix: node didn't start: NODE_OPTIONS is set (--bogus-flag), which it reads as it starts: unset it or fix it\ninstall: ";
        let nf = resolve_with(&Config::default(), &l, &p, &mut ProbeCache::off(), "js").unwrap_err();
        assert!(nf.message.contains(fix), "{}", nf.message);
        // Unset, or another kind of failure: the install advice alone.
        let nf = resolve_with(&Config::default(), &lookup(&[&d], None), &p, &mut ProbeCache::off(), "js").unwrap_err();
        assert!(!nf.message.contains("fix:"), "{}", nf.message);
        let slow = fake(|_| ProbeOutcome::TimedOut);
        assert!(!resolve_with(&Config::default(), &l, &slow, &mut ProbeCache::off(), "js").unwrap_err().message.contains("fix:"));
        let nf = resolve_with(&Config::default(), &l, &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(!nf.message.contains("fix:"), "PYTHONHOME isn't read by --version: {}", nf.message);
        // A configured node: the same, on the message's first line, which doctor shows.
        let cfg = with_command("js", &[&node.to_string_lossy()], ConfigSource::Env("V2_ENGINE_JS".into()));
        let nf = resolve_with(&cfg, &l, &p, &mut ProbeCache::off(), "js").unwrap_err();
        assert!(nf.message.ends_with("NODE_OPTIONS; node didn't start: NODE_OPTIONS is set (--bogus-flag), which it reads as it starts: unset it or fix it"), "{}", nf.message);
        // Doctor's fix line for one found on PATH comes from the same rule.
        assert_eq!(probe_fix("js", &nf.skipped, &set), None, "nothing skipped");
        let skipped = [(node.clone(), "version probe failed (exit code 9): node: --bogus-flag is not allowed in NODE_OPTIONS".to_string())];
        assert!(probe_fix("js", &skipped, &set).is_some_and(|f| f.starts_with("node didn't start: NODE_OPTIONS is set (--bogus-flag)")));
        assert_eq!(probe_fix("js", &skipped, &set[1..]), None);
    }

    #[test]
    fn doctor_sees_whether_the_program_starts() {
        // Round 18, 19.2: `--version` answers before Python reads PYTHONHOME, so doctor starts
        // the program (Python importing what V2's worker imports, `node -e 0`) and reports the
        // first line that names an error.
        let python = PathBuf::from("/usr/bin/python3");
        let broken = "Python path configuration:\n  PYTHONHOME = '/nope'\nFatal Python error: init_fs_encoding: failed to get the Python codec of the filesystem encoding\nPython runtime state: core initialized\n";
        let p = fake(move |_| ProbeOutcome::Exited { code: Some(1), output: broken.into() });
        let argvs = p.argvs.clone();
        assert_eq!(
            start_failure(&p, &spec("py"), &[python.clone().into_os_string()]).as_deref(),
            Some("can't import the standard modules V2's worker uses (exit code 1): Fatal Python error: init_fs_encoding: failed to get the Python codec of the filesystem encoding")
        );
        let check = crate::engines::spec::start_args("py");
        assert_eq!(argvs.borrow()[0], [python.clone().into_os_string(), "-c".into(), check[1].into()]);
        // Round 19: a stale PYTHONPATH starts Python, then breaks a module the worker imports:
        // the traceback's own line, not a source line in it that names an error.
        let stale = "Traceback (most recent call last):\n  File \"<string>\", line 1, in <module>\n  File \"/old/re.py\", line 1, in <module>\n    raise ImportError('stale re')\nImportError: stale re\n";
        let p = fake(move |_| ProbeOutcome::Exited { code: Some(1), output: stale.into() });
        assert_eq!(
            start_failure(&p, &spec("py"), &[python.clone().into_os_string()]).as_deref(),
            Some("can't import the standard modules V2's worker uses (exit code 1): ImportError: stale re")
        );
        let node = PathBuf::from("/usr/bin/node");
        let preload = "node:internal/modules/cjs/loader:1247\n  throw err;\n  ^\n\nError: Cannot find module '/x/missing.js'\nRequire stack:\n";
        let p = fake(move |_| ProbeOutcome::Exited { code: Some(1), output: preload.into() });
        assert_eq!(start_failure(&p, &spec("js"), &[node.into_os_string()]).as_deref(), Some("-e 0 failed (exit code 1): Error: Cannot find module '/x/missing.js'"));
        // It starts, or the check can't tell: nothing to report.
        for outcome in [says(""), ProbeOutcome::TimedOut, ProbeOutcome::SpawnFailed("boom".into())] {
            let p = fake(move |_| outcome.clone());
            assert_eq!(start_failure(&p, &spec("py"), &[python.clone().into_os_string()]), None);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_tilde_path_entry_that_holds_the_program_is_named() {
        // Round 19, 7.5: `export PATH="~/.nvm/…/bin:$PATH"` (quoted, so the shell didn't expand
        // it): bash's own lookup expands it, execvp, dash, Python and V2 don't.
        let home = temp("tilde-home");
        let nvm = home.join(".nvm").join("v24").join("bin");
        std::fs::create_dir_all(&nvm).unwrap();
        exe(&nvm, "node");
        let system = temp("tilde-system");
        exe(&system, "node");
        let entry = PathBuf::from("~/.nvm/v24/bin");
        let path = std::env::join_paths([entry.clone(), system.clone()]).unwrap();
        let js = spec("js");
        let none = std::collections::HashSet::new();
        let why = "its PATH entry starts with ~, which only bash expands: write $HOME/.nvm/v24/bin in your startup file".to_string();
        let mut skipped = Vec::new();
        tilde_skips(&js, &path, Some(&system), Some(&home), &none, &mut skipped);
        assert_eq!(skipped, [(entry.join("node"), why.clone())], "before the one V2 uses");
        // After it: bash runs the same one. Without a home, or for another program: nothing.
        let after = std::env::join_paths([system.clone(), entry.clone()]).unwrap();
        for (path, home, id) in [(&after, Some(&*home), "js"), (&path, None, "js"), (&path, Some(&*home), "py")] {
            let mut skipped = Vec::new();
            tilde_skips(&spec(id), path, Some(&system), home, &none, &mut skipped);
            assert_eq!(skipped, [], "{id}");
        }
        // None usable: every such entry, after what the walk skipped.
        let mut skipped = vec![(system.join("node"), "too old: 18.19.1 < 22.13".to_string())];
        tilde_skips(&js, &path, None, Some(&home), &none, &mut skipped);
        assert_eq!(skipped[1], (entry.join("node"), why));
        // The same file met through an absolute entry: the shell and V2 agree.
        let seen = std::collections::HashSet::from([(std::fs::canonicalize(nvm.join("node")).unwrap(), Vec::new())]);
        let mut skipped = Vec::new();
        tilde_skips(&js, &path, None, Some(&home), &seen, &mut skipped);
        assert_eq!(skipped, []);
    }

    #[cfg(unix)]
    #[test]
    fn tilde_expansion() {
        let h = Path::new("/h");
        assert_eq!(expand_tilde("~", Some(h)), Some((PathBuf::from("/h"), "$HOME".to_string())));
        assert_eq!(expand_tilde("~/a/b/", Some(h)), Some((PathBuf::from("/h/a/b"), "$HOME/a/b".to_string())));
        assert_eq!(expand_tilde("~/a", None), None);
        assert_eq!(expand_tilde("/usr/bin", Some(h)), None);
        assert_eq!(expand_tilde("bin", Some(h)), None);
        // ~user: that user's home, written out.
        let root = home_of("root").expect("root has a home");
        assert_eq!(expand_tilde("~root/bin", Some(h)), Some((root.join("bin"), format!("{}/bin", root.display()))));
        assert_eq!(expand_tilde("~no-such-user-v2/bin", Some(h)), None);
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
        // Round 15: under 10 s the probe still waits 10 s, and the hint says so.
        let slow = fake(|_| ProbeOutcome::TimedOut);
        cfg.engines.get_mut("js").unwrap().startup_timeout = Some(Sourced { value: 2.0, from: project_source() });
        let nf = resolve_with(&cfg, &l, &slow, &mut ProbeCache::off(), "js").unwrap_err();
        assert!(nf.message.ends_with("version probe timed out (10 s); a probe waits at least 10 s: raise [engines.js] startup_timeout above that if the program is only slow"), "{}", nf.message);
        cfg.engines.get_mut("js").unwrap().startup_timeout = Some(Sourced { value: 20.0, from: project_source() });
        let nf = resolve_with(&cfg, &l, &slow, &mut ProbeCache::off(), "js").unwrap_err();
        assert!(nf.message.ends_with("version probe timed out (20 s); raise [engines.js] startup_timeout if it is only slow"), "{}", nf.message);
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
    fn git_bash_is_found_beside_git_then_in_program_files() {
        // 7.6 (round 18): bin\bash.exe above git's folder, else Program Files' Git; never a bash
        // on PATH (WSL's launcher, System32\bash.exe).
        let root = temp("gitbash");
        let (cmd, wsl, pf) = (root.join("Git").join("cmd"), root.join("System32"), root.join("Program Files"));
        for d in [&cmd, &wsl, &root.join("Git").join("bin"), &pf.join("Git").join("bin")] {
            std::fs::create_dir_all(d).unwrap();
        }
        exe(&cmd, "git");
        exe(&wsl, "bash");
        let beside = root.join("Git").join("bin").join("bash.exe");
        let installed = pf.join("Git").join("bin").join("bash.exe");
        std::fs::write(&beside, "").unwrap();
        std::fs::write(&installed, "").unwrap();
        assert_eq!(git_bash(&lookup(&[&wsl, &cmd], None), Some(&pf)), Some(beside));
        assert_eq!(git_bash(&lookup(&[&wsl], None), Some(&pf)), Some(installed));
        assert_eq!(git_bash(&lookup(&[&wsl], None), Some(&root.join("none"))), None);
    }

    #[test]
    fn a_configured_store_placeholder_names_the_python_on_path() {
        // 7.8 (round 18): V2_ENGINE_PY=python3 on Windows is the Store's placeholder while Python
        // is installed as python: the message names that one, the way the command was set.
        let root = temp("r18ph");
        let (apps, real) = (root.join("WindowsApps"), root.join("Python311"));
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        exe(&apps, "python3");
        let python = exe(&real, "python");
        let apps2 = apps.clone();
        let p = fake(move |prog| {
            if prog.starts_with(&apps2) {
                ProbeOutcome::Exited { code: Some(9009), output: "Python was not found; run without arguments to install from the Microsoft Store".into() }
            } else {
                says("Python 3.11.6")
            }
        });
        let start = "engine 'py' command 'python3' (set by";
        let l = lookup(&[&apps, &real], Some(&apps));
        let env = with_command("py", &["python3"], ConfigSource::Env("V2_ENGINE_PY".into()));
        let nf = resolve_with(&env, &l, &p, &mut ProbeCache::off(), "py").unwrap_err();
        let installed = format!("is not usable: Microsoft Store placeholder; Python is installed as {}: name that one, as in", python.display());
        assert_eq!(nf.message, format!("{start} V2_ENGINE_PY) {installed} V2_ENGINE_PY=python"));
        let toml = with_command("py", &["python3"], project_source());
        let nf = resolve_with(&toml, &l, &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.message, format!("{start} C:/proj/v2.toml) {installed} [engines.py] command = [\"python\"]"));
        // No Python on PATH: what to install.
        let nf = resolve_with(&env, &lookup(&[&apps], Some(&apps)), &p, &mut ProbeCache::off(), "py").unwrap_err();
        let install = format!("is not usable: Microsoft Store placeholder; install Python and name its full path, or leave command out: {}", install_hint("py"));
        assert_eq!(nf.message, format!("{start} V2_ENGINE_PY) {install}"));
        // Round 18 review: WindowsApps, first on PATH, has a python placeholder too, so python
        // would find it again: the message names Python by its path, never the configured name.
        exe(&apps, "python");
        let shown = python.display().to_string();
        let toml_word = if shown.contains('\\') { format!("'{shown}'") } else { format!("\"{shown}\"") };
        for configured in ["python3", "python"] {
            let start = format!("engine 'py' command '{configured}' (set by");
            let installed = format!("is not usable: Microsoft Store placeholder; Python is installed as {shown}: name that one, as in");
            let env = with_command("py", &[configured], ConfigSource::Env("V2_ENGINE_PY".into()));
            let nf = resolve_with(&env, &l, &p, &mut ProbeCache::off(), "py").unwrap_err();
            assert_eq!(nf.message, format!("{start} V2_ENGINE_PY) {installed} V2_ENGINE_PY={shown}"));
            let toml = with_command("py", &[configured], project_source());
            let nf = resolve_with(&toml, &l, &p, &mut ProbeCache::off(), "py").unwrap_err();
            assert_eq!(nf.message, format!("{start} C:/proj/v2.toml) {installed} [engines.py] command = [{toml_word}]"));
        }
        // Python first on PATH: its name finds it, past both placeholders.
        let nf = resolve_with(&env, &lookup(&[&real, &apps], Some(&apps)), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.message, format!("{start} V2_ENGINE_PY) {installed} V2_ENGINE_PY=python"));
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
        // With / on Windows, so it pastes into a TOML "…" too (round 19).
        let slashed = |p: &Path| p.display().to_string().replace('\\', if cfg!(windows) { "/" } else { "\\" });
        assert!(m.ends_with(&format!("; V2_ENGINE_PY names a folder: point it at the program in it, {}", slashed(&inside))), "{m}");
        let venv = d.join(".venv");
        let python = venv.join(if cfg!(windows) { "Scripts\\python.exe" } else { "bin/python3" });
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = x\n").unwrap();
        std::fs::write(&python, "").unwrap();
        let shown = crate::config::toml_string(&venv.display().to_string());
        let m = says_for(&env, &venv);
        assert!(m.ends_with(&format!("; V2_ENGINE_PY names a virtual environment: write [python] venv = {shown} in v2.toml, or point V2_ENGINE_PY at its python, {}", slashed(&python))), "{m}");
        let m = says_for(&ConfigSource::Project(d.join("v2.toml")), &venv);
        assert!(m.ends_with(&format!("; [engines.py] command names a virtual environment: write [python] venv = {shown} in v2.toml, or point [engines.py] command at its python, {}", slashed(&python))), "{m}");
        let cfg = with_command("py", &["no such python"], ConfigSource::Env("V2_ENGINE_PY".into()));
        let nf = resolve_with(&cfg, &lookup(&[&d], None), &fake(|_| says("Python 3.12.1")), &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with("not found (set by V2_ENGINE_PY)"), "{}", nf.message);
        // A variable V2 doesn't expand, as PowerShell's single quotes keep it (18.4).
        let unexpanded = d.join("%ProgramFiles%").join("Python311").join("python.exe");
        let cfg = with_command("py", &[&unexpanded.to_string_lossy()], ConfigSource::Env("V2_ENGINE_PY".into()));
        let nf = resolve_with(&cfg, &lookup(&[&d], None), &fake(|_| says("Python 3.12.1")), &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with("not found (set by V2_ENGINE_PY); V2 doesn't expand %ProgramFiles%: write the path out"), "{}", nf.message);
        // A ~ that quotes kept, joined to the cwd as config does (18.4, round 20). A bare ~ is the
        // home folder, no program: the hint gives no example (the second review of round 20).
        let cwd = std::env::current_dir().unwrap();
        for (value, path) in [("~/tools 1/bin/python3", "tools 1/bin/python3"), ("~", ""), ("~/", ""), ("~\\", "")] {
            let joined = if value == "~" { value.into() } else { cwd.join(value).to_string_lossy().into_owned() };
            let cfg = with_command("py", &[&joined], ConfigSource::Env("V2_ENGINE_PY".into()));
            let nf = resolve_with(&cfg, &lookup(&[&d], None), &fake(|_| says("Python 3.12.1")), &mut ProbeCache::off(), "py").unwrap_err();
            let want = if path.is_empty() {
                String::new()
            } else if cfg!(windows) {
                format!(", as in $env:V2_ENGINE_PY = \"$env:USERPROFILE\\{}\"", path.replace('/', "\\"))
            } else {
                format!(", as in V2_ENGINE_PY=\"$HOME/{path}\"")
            };
            assert!(nf.message.ends_with(&format!("not found (set by V2_ENGINE_PY); V2 doesn't expand ~ in V2_ENGINE_PY: write the path out{want}")), "{value}: {}", nf.message);
        }
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
    fn a_configured_path_holding_an_escape_says_so() {
        // 18.5, 7.6 (round 19): "C:/Program Files/nodejs\node.exe" in a TOML "…" holds a line
        // break, so the program isn't found: the message says why, for a command and a venv (whose
        // command to make it would make that name). Not for V2_ENGINE_*, which has no escapes, nor
        // for the name of v2.toml's own folder, which the user didn't write.
        let d = temp("escape");
        let toml = ConfigSource::Project(d.join("v2.toml"));
        let hint = |example: &str| format!(r#"in "…", a backslash starts an escape (\n is a line break, \t a tab): write the path with / or in single quotes, as in {example}"#);
        let js = |program: &Path, from: ConfigSource| {
            let cfg = with_command("js", &[&program.to_string_lossy()], from);
            resolve_with(&cfg, &lookup(&[], None), &fake(|_| says("v22.13.0")), &mut ProbeCache::off(), "js").unwrap_err().message
        };
        let program = d.join("nodejs\node.exe");
        let m = js(&program, toml.clone());
        let shown = crate::syntax::span::shown_path(&program.display().to_string());
        assert!(m.starts_with(&format!("engine 'js' command '{shown}' not found")), "{m}");
        assert!(m.ends_with(&format!("; {}", hint(r"command = ['C:\Program Files\nodejs\node.exe']"))), "{m}");
        assert!(!js(&program, ConfigSource::Env("V2_ENGINE_JS".into())).contains("backslash"));
        let cfg = with_command("py", &[&d.join("Python311\tpython.exe").to_string_lossy()], toml.clone());
        let nf = resolve_with(&cfg, &lookup(&[], None), &fake(|_| says("Python 3.12.1")), &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with(&format!("; {}", hint(r"command = ['C:\Program Files\Python311\python.exe']"))), "{}", nf.message);
        let want = if cfg!(windows) { r"Scripts\python.exe" } else { "bin/python" };
        let venv_says = |venv: &Path, from: ConfigSource| {
            let cfg = Config { python_venv: Some(Sourced { value: venv.to_path_buf(), from }), ..Config::default() };
            resolve_with(&cfg, &lookup(&[], None), &fake(|_| says("Python 3.12.1")), &mut ProbeCache::off(), "py").unwrap_err().message
        };
        let venv = d.join("work\new");
        let shown = crate::syntax::span::shown_path(&venv.display().to_string());
        assert_eq!(venv_says(&venv, toml.clone()), format!("[python] venv '{shown}' has no {want}: {}", hint(r"venv = 'C:\proj\.venv'")));
        // A folder named with a control character (Linux and macOS allow one): no such hint, and
        // the command to make the venv shows it escaped, as every printed path (2.1).
        let odd = d.join("a\u{1b}b");
        let m = venv_says(&odd.join(".venv"), ConfigSource::Project(odd.join("v2.toml")));
        assert!(m.contains("-m venv") && m.contains(r"a\u{1b}b") && !m.contains('\u{1b}') && !m.contains("backslash"), "{m}");
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
        // Round 19 (L9): FROM says venv for a missing one too.
        assert_eq!((nf.command.clone(), nf.from.as_deref()), (Some(venv.join(if cfg!(windows) { r"Scripts\python.exe" } else { "bin/python" }).display().to_string()), Some("venv")));
        // A folder with a space is quoted, so the command can be pasted (7.6).
        let spaced = d.join("my env");
        let cfg = Config { python_venv: Some(Sourced { value: spaced.clone(), from: project_source() }), ..Config::default() };
        let nf = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with(&format!("(create it: {python} -m venv \"{}\")", spaced.display())), "{}", nf.message);
        let bin = venv.join(if cfg!(windows) { "Scripts" } else { "bin" });
        std::fs::create_dir_all(&bin).unwrap();
        let python = exe(&bin, "python");
        std::fs::write(venv.join("pyvenv.cfg"), "home = x\n").unwrap();
        let e = resolve_with(&missing, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap();
        assert_eq!((e.argv, e.origin), (vec![python.clone().into_os_string()], Origin::Venv));
        // The interpreter instead of its folder, and a venv under another name (7.6).
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
        // The folder itself (round 19, L13: on Linux and macOS too, where its bin/python3 was
        // used), and its bin folder (Scripts); on Linux and macOS, a bin/python without a
        // bin/activate or pyvenv.cfg too.
        let bare = d.join("nocfg");
        let bare_py = {
            let bin = bare.join(if cfg!(windows) { "Scripts" } else { "bin" });
            std::fs::create_dir_all(&bin).unwrap();
            exe(&bin, "python")
        };
        let mut named = vec![(root.join(if cfg!(windows) { "Scripts" } else { "bin" }), "a folder of a Python installation", &py), (root.clone(), "a Python installation", &py)];
        if !cfg!(windows) {
            named.push((bare.clone(), "a Python installation", &bare_py));
        }
        for (v, what, py) in named {
            let nf = resolve_with(&at(&v), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
            let head = format!("[python] venv '{}' is {what}, not a virtual environment: name its python instead, [engines.py] command = ", v.display());
            assert!(nf.message.starts_with(&head) && nf.message.ends_with(" -m venv .venv"), "{}", nf.message);
            assert!(nf.message.contains(&py.display().to_string()) && !nf.message.contains(&format!("-m venv {created}")), "{}", nf.message);
            assert_eq!(nf.from.as_deref(), Some("venv"));
        }
        // Round 19 review (P4): a venv's Scripts\python.exe without pyvenv.cfg (a launcher that
        // can't run without it, "No pyvenv.cfg file"), or elsewhere a bin/activate without one, is
        // a damaged venv: made again, not named as a Python to run.
        #[cfg(unix)]
        std::fs::write(bare.join("bin").join("activate"), "").unwrap();
        let nf = resolve_with(&at(&bare), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.message, format!("[python] venv '{}' has no pyvenv.cfg, so it is damaged: make it again, {python} -m venv --clear {}", bare.display(), bare.display()));
        assert_eq!((nf.status, nf.from.as_deref()), (Status::Broken, Some("venv")));
        // Its Scripts (bin) folder, or its python (the launcher on Windows), named instead: the
        // same venv made again, never its launcher as a command (round 19, the second review).
        for (v, what) in [(bare_py.parent().unwrap().to_path_buf(), "inside"), (bare_py.clone(), "a file in")] {
            let nf = resolve_with(&at(&v), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
            let want = format!("[python] venv '{}' is {what} a virtual environment that has no pyvenv.cfg, so it is damaged: make it again, {python} -m venv --clear {}, and write venv = ", v.display(), bare.display());
            assert!(nf.message.starts_with(&want) && !nf.message.contains("[engines.py]"), "{}", nf.message);
        }
        // A conda environment (conda-meta), its python at its root on Windows, in bin elsewhere;
        // the base environment (with condabin) is activated as base.
        let quoted = |p: &Path| if cfg!(windows) { format!("['{}']", p.display()) } else { format!("[\"{}\"]", p.display()) };
        for (env, name) in [(d.join("miniconda3").join("envs").join("sales"), "sales"), (d.join("miniforge3"), "base")] {
            std::fs::create_dir_all(env.join("conda-meta")).unwrap();
            if name == "base" {
                std::fs::create_dir_all(env.join("condabin")).unwrap();
            }
            let conda_bin = if cfg!(windows) { env.clone() } else { env.join("bin") };
            std::fs::create_dir_all(&conda_bin).unwrap();
            let conda_py = exe(&conda_bin, if cfg!(windows) { "python" } else { "python3" });
            // Conda's base has a bin/activate too, and is no damaged venv (P4).
            if cfg!(unix) && name == "base" {
                std::fs::write(conda_bin.join("activate"), "").unwrap();
            }
            let nf = resolve_with(&at(&env), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
            let want = format!(
                "[python] venv '{}' is a conda environment, not a virtual environment: run v2 where it is activated (conda activate {name}) and leave venv out, or name its python instead: [engines.py] command = {}",
                env.display(),
                quoted(&conda_py)
            );
            assert_eq!(nf.message, want);
        }
        // The python itself, not in a venv: name it as the command.
        let nf = resolve_with(&at(&py), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.contains("is a file: venv names a virtual environment's folder; for a Python that isn't in one, write [engines.py] command = "), "{}", nf.message);
        // Another file (round 16: `venv = "v2.toml"`): not offered as a Python.
        let toml_file = d.join("v2.toml");
        std::fs::write(&toml_file, "").unwrap();
        let nf = resolve_with(&at(&toml_file), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.ends_with("is a file: venv names a virtual environment's folder, as in venv = \".venv\""), "{}", nf.message);
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

    #[test]
    fn a_venv_the_other_system_made_is_kept() {
        // Round 21 (W2, L8), 7.6: one project folder used from Windows and from WSL. The other
        // system's venv (on Windows a bin/python, or a POSIX home; elsewhere a Scripts\python.exe,
        // or a home with a drive letter) is never made again here with --clear, which on a
        // \\wsl.localhost path failed halfway and left neither system a venv: a venv for this
        // system beside it, named in V2_ENGINE_PY, which wins over [python] venv.
        let d = temp("othersystem");
        let p = fake(|_| says("Python 3.12.1"));
        let at = |v: &Path| Config { python_venv: Some(Sourced { value: v.to_path_buf(), from: project_source() }), ..Config::default() };
        let (other, bin, home) = if cfg!(windows) { ("Linux or macOS", "bin", "/usr/bin") } else { ("Windows", "Scripts", r"C:\Program Files\Python311") };
        let make = |name: &str, home: &str, python: bool| {
            let v = d.join(name);
            std::fs::create_dir_all(v.join(bin)).unwrap();
            std::fs::write(v.join("pyvenv.cfg"), format!("home = {home}\nversion = 3.12.3\n")).unwrap();
            if python {
                std::fs::write(v.join(bin).join(if cfg!(windows) { "python" } else { "python.exe" }), "").unwrap();
            }
            v
        };
        // Its python alone, its home alone, or both.
        for v in [make("both", home, true), make("python", "x", true), make("home", home, false)] {
            let nf = resolve_with(&at(&v), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
            let want = if cfg!(windows) {
                let new = d.join(".venv-win");
                format!(
                    "[python] venv '{}' was made on {other}: keep it, make one for this system beside it, py -m venv {}, and name its python in V2_ENGINE_PY, which wins over [python] venv ($env:V2_ENGINE_PY = \"{}\"), or run V2 on that system",
                    v.display(),
                    crate::cli::shell_word(&new.display().to_string()),
                    new.join("Scripts").join("python.exe").display()
                )
            } else {
                let new = d.join(if cfg!(target_os = "macos") { ".venv-macos" } else { ".venv-linux" });
                format!(
                    "[python] venv '{}' was made on {other}: keep it, make one for this system beside it, python3 -m venv {}, and name its python in V2_ENGINE_PY, which wins over [python] venv (export V2_ENGINE_PY={}), or run V2 on that system",
                    v.display(),
                    crate::cli::shell_word(&new.display().to_string()),
                    crate::cli::shell_word(&new.join("bin").join("python").display().to_string())
                )
            };
            assert_eq!(nf.message, want);
            assert!(!nf.message.contains("--clear"), "{}", nf.message);
            assert_eq!((nf.status, nf.from.as_deref()), (Status::Broken, Some("venv")));
        }
        // On Windows, an MSYS2 venv's bin has a python.exe: not Linux's, so the message for a venv
        // without this system's python stays.
        if cfg!(windows) {
            let msys = make("msys", "C:/msys64/ucrt64/bin", true);
            std::fs::write(msys.join("bin").join("python.exe"), "").unwrap();
            let nf = resolve_with(&at(&msys), &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
            assert!(nf.message.contains(", so it was made on another system or is damaged: "), "{}", nf.message);
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_configured_program_windows_cant_start_isnt_called_missing() {
        // Round 21 (W7), 7.6: a Linux python named in v2.toml (an ELF file, as /usr/bin/python3.12
        // on WSL's share) is there: it isn't a Windows program, never "not found". A path that
        // mixes separators, as `/usr/bin/x` joined to v2.toml's folder, is shown with `\` only.
        let d = temp("notwindows");
        let elf = d.join("python3.12");
        std::fs::write(&elf, b"\x7fELF\x02\x01\x01\0rest").unwrap();
        let text = d.join("pytext");
        std::fs::write(&text, "#!/usr/bin/env python3\n").unwrap();
        let p = fake(|_| says("Python 3.12.1"));
        let from = ConfigSource::Project(PathBuf::from(r"C:\proj\v2.toml"));
        let mixed = format!("{}/python3.12", d.display());
        for (id, exe) in [("py", "python.exe"), ("js", "node.exe")] {
            let cfg = with_command(id, &[&mixed, "-X", "utf8"], from.clone());
            let nf = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), id).unwrap_err();
            let want = format!(
                "engine '{id}' command '{}' (set by C:\\proj\\v2.toml) isn't a Windows program (a Linux one): the Windows v2 runs Windows programs: name a {exe}, or run the Linux v2 inside WSL",
                elf.display()
            );
            assert_eq!(nf.message, want);
            assert_eq!((nf.status, nf.command.as_deref()), (Status::Broken, Some(&*format!("{} -X utf8", crate::cli::shell_word(&elf.display().to_string())))));
        }
        let cfg = with_command("py", &[&text.display().to_string()], ConfigSource::Env("V2_ENGINE_PY".into()));
        let nf = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.message, format!("engine 'py' command '{}' (set by V2_ENGINE_PY) isn't a Windows program (an .exe, .com, .bat or .cmd file): name a python.exe", text.display()));
        // Nothing there: still not found, shown with one separator.
        let cfg = with_command("py", &[&format!("{}/missing/python", d.display())], from.clone());
        let nf = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.message, format!("engine 'py' command '{}' not found (set by C:\\proj\\v2.toml)", d.join("missing").join("python").display()));
        // On WSL's share (V2_TEST_WSL_DISTRO names a distro), a Linux program and a WSL link,
        // which Windows can't follow: /usr/bin/env and Debian's /usr/bin/sh, a link to dash.
        let Some(distro) = std::env::var("V2_TEST_WSL_DISTRO").ok().filter(|d| !d.is_empty()) else {
            let _ = std::io::Write::write_all(&mut std::io::stderr(), b"SKIP the WSL share part of a_configured_program_windows_cant_start_isnt_called_missing (V2_TEST_WSL_DISTRO not set)\n");
            return;
        };
        let root = PathBuf::from(format!(r"\\wsl.localhost\{distro}"));
        let mut programs = vec![root.join("usr").join("bin").join("env")];
        let sh = root.join("usr").join("bin").join("sh");
        if std::fs::symlink_metadata(&sh).is_ok_and(|m| m.file_type().is_symlink()) {
            programs.push(sh);
        }
        for program in programs {
            let cfg = with_command("py", &[&program.display().to_string()], from.clone());
            let nf = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
            assert!(nf.message.ends_with(" isn't a Windows program (a Linux one): the Windows v2 runs Windows programs: name a python.exe, or run the Linux v2 inside WSL"), "{}", nf.message);
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_batch_command_is_refused_in_a_share_folder() {
        // The review of round 21, 7.6: a configured .bat or .cmd (pyenv-win's shim, conda.bat)
        // runs in cmd.exe, which in a \\server\share working folder would start the worker in the
        // Windows folder: refused there before any probe; from a drive folder it is probed.
        let d = temp("batchshare");
        let shim = d.join("pyshim.bat");
        std::fs::write(&shim, "@echo off\r\n").unwrap();
        let p = fake(|_| says("Python 3.12.1"));
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        for (id, exe) in [("py", "python.exe"), ("js", "node.exe")] {
            let cfg = with_command(id, &[&shim.display().to_string(), "-X"], ConfigSource::Env(format!("V2_ENGINE_{}", id.to_uppercase())));
            for share in [r"\\wsl.localhost\Ubuntu\home\ann\p", r"\\?\UNC\server\share\p"] {
                let l = Lookup { cwd: PathBuf::from(share), ..lookup(&[], None) };
                let nf = resolve_with(&cfg, &l, &p, &mut ProbeCache::off(), id).unwrap_err();
                let want = format!(
                    "engine '{id}' command '{}' (set by V2_ENGINE_{}) runs in cmd.exe, which can't work in a \\\\server\\share folder and would run in {windows}: name the program itself ({exe}), or run v2 from a folder on a drive",
                    shim.display(),
                    id.to_uppercase()
                );
                assert_eq!(nf.message, want);
                assert_eq!((nf.status, nf.command.as_deref()), (Status::Broken, Some(&*format!("{} -X", crate::cli::shell_word(&shim.display().to_string())))));
            }
        }
        assert_eq!(p.calls.get(), 0, "no probe");
        let cfg = with_command("py", &[&shim.display().to_string()], ConfigSource::Env("V2_ENGINE_PY".into()));
        assert!(resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn a_batch_file_on_path_is_passed_over_in_a_share_folder() {
        // The review of round 21, 7.6: from a \\server\share working folder, pyenv-win's
        // shims\python.bat is skipped unprobed, like a Store placeholder, and the search goes on;
        // with nothing else, py doesn't run, and the fix is not to repair Python. From a drive
        // folder the shim is used.
        let d = temp("batchpath");
        let (shims, real) = (d.join("shims"), d.join("Python312"));
        std::fs::create_dir_all(&shims).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        let shim = shims.join("python.bat");
        std::fs::write(&shim, "@echo off\r\n").unwrap();
        let python = exe(&real, "python");
        let share = PathBuf::from(r"\\wsl.localhost\Ubuntu\home\ann\p");
        let p = fake(|_| says("Python 3.12.1"));
        let why = r"runs in cmd.exe, which can't work in a \\server\share folder";
        let l = Lookup { cwd: share.clone(), ..lookup(&[&shims, &real], None) };
        let e = resolve_with(&Config::default(), &l, &p, &mut ProbeCache::off(), "py").unwrap();
        assert_eq!(e.argv[0], python.as_os_str());
        assert_eq!(e.skipped, vec![(shim.clone(), why.to_string())]);
        assert!(p.argvs.borrow().iter().all(|a| a[0] != shim.as_os_str()), "the shim isn't probed: {:?}", p.argvs.borrow());
        let l = Lookup { cwd: share, ..lookup(&[&shims], None) };
        let nf = resolve_with(&Config::default(), &l, &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(nf.status, Status::Broken);
        assert!(nf.message.starts_with("engine 'py' (Python) was found but doesn't run (see why below)\n"), "{}", nf.message);
        let fix = "fix: name the program itself (python.exe) with V2_ENGINE_PY, or run v2 from a folder on a drive";
        assert!(nf.message.contains(&format!("\nskipped: {} ({why})\n{fix}\ninstall: ", shim.display())), "{}", nf.message);
        assert_eq!(batch_fix("js", &nf.skipped).as_deref(), Some("name the program itself (node.exe) with V2_ENGINE_JS, or run v2 from a folder on a drive"));
        let e = resolve_with(&Config::default(), &lookup(&[&shims], None), &p, &mut ProbeCache::off(), "py").unwrap();
        assert_eq!(e.argv[0], shim.as_os_str());
    }

    #[cfg(windows)]
    #[test]
    fn a_wsl_link_on_a_drive_is_a_linux_program() {
        // The review of round 21 (W7, W2): a link WSL makes on a drive (a Linux venv made from
        // WSL under /mnt/c) can't even be stat'ed from Windows (os error 1920), yet it is there:
        // a Linux program, never "not found", and a Linux venv's python. Needs a WSL distro
        // (V2_TEST_WSL_DISTRO), which makes the links in a temp folder.
        let Some(distro) = std::env::var("V2_TEST_WSL_DISTRO").ok().filter(|d| !d.is_empty()) else {
            let _ = std::io::Write::write_all(&mut std::io::stderr(), b"SKIP a_wsl_link_on_a_drive_is_a_linux_program (V2_TEST_WSL_DISTRO not set)\n");
            return;
        };
        /// Removes the folder through WSL, which can remove its links.
        struct Gone(String, PathBuf);
        impl Drop for Gone {
            fn drop(&mut self) {
                let script = "rm -rf \"$(wslpath -u \"$1\")\"";
                let _ = std::process::Command::new("wsl.exe").args(["-d", &self.0, "-u", "root", "-e", "sh", "-c", script, "sh"]).arg(&self.1).status();
                let _ = std::fs::remove_dir_all(&self.1);
            }
        }
        let d = crate::syntax::span::strip_verbatim(&std::fs::canonicalize(temp("wsllink")).unwrap());
        let _gone = Gone(distro.clone(), d.clone());
        let venv = d.join("lxvenv");
        std::fs::create_dir_all(venv.join("bin")).unwrap();
        std::fs::write(venv.join("pyvenv.cfg"), "home = x\n").unwrap();
        let link = |at: &Path| {
            let script = "ln -s /usr/bin/python3 \"$(wslpath -u \"$1\")\"";
            let made = std::process::Command::new("wsl.exe").args(["-d", &distro, "-u", "root", "-e", "sh", "-c", script, "sh"]).arg(at).status().unwrap();
            assert!(made.success(), "ln -s {}", at.display());
        };
        let python = d.join("python3");
        link(&python);
        link(&venv.join("bin").join("python"));
        assert_eq!(std::fs::symlink_metadata(&python).err().and_then(|e| e.raw_os_error()), Some(1920), "Windows stats a WSL link on a drive now");
        let p = fake(|_| says("Python 3.12.1"));
        let cfg = with_command("py", &[&python.display().to_string()], ConfigSource::Env("V2_ENGINE_PY".into()));
        let nf = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert_eq!(
            nf.message,
            format!("engine 'py' command '{}' (set by V2_ENGINE_PY) isn't a Windows program (a Linux one): the Windows v2 runs Windows programs: name a python.exe, or run the Linux v2 inside WSL", python.display())
        );
        let at = Config { python_venv: Some(Sourced { value: venv.clone(), from: project_source() }), ..Config::default() };
        let nf = resolve_with(&at, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap_err();
        assert!(nf.message.starts_with(&format!("[python] venv '{}' was made on Linux or macOS: keep it, ", venv.display())), "{}", nf.message);
    }

    #[test]
    fn a_command_through_wsl_exe_gets_a_note() {
        // Round 21 (W7), 7.6: wsl.exe, or WSL's bash.exe in System32, runs a Linux interpreter,
        // which can't read the adapter's Windows path. Git for Windows' bash, another program, and
        // any program elsewhere than Windows get no note.
        let note = |id: &str, p: &str| linux_interpreter_note(id, &[OsString::from(p), OsString::from("-e"), OsString::from("python3")]);
        let py = "runs a Linux python, which the Windows v2 can't use, as its adapter is a Windows path: name a python.exe, or run the Linux v2 inside WSL";
        if cfg!(windows) {
            assert_eq!(note("py", r"C:\WINDOWS\system32\wsl.exe").as_deref(), Some(&*format!("wsl.exe {py}")));
            assert_eq!(note("py", r"C:\Users\me\AppData\Local\Microsoft\WindowsApps\wsl.exe").as_deref(), Some(&*format!("wsl.exe {py}")));
            assert_eq!(note("py", r"C:\Windows\System32\bash.exe").as_deref(), Some(&*format!("WSL's bash.exe {py}")));
            assert_eq!(
                note("js", r"C:\Windows\Sysnative\bash.exe").as_deref(),
                Some("WSL's bash.exe runs a Linux node, which the Windows v2 can't use, as its adapter is a Windows path: name a node.exe, or run the Linux v2 inside WSL")
            );
        } else {
            assert_eq!(note("py", "/usr/bin/wsl.exe"), None);
        }
        assert_eq!(note("py", r"C:\Program Files\Git\bin\bash.exe"), None);
        assert_eq!(note("py", r"C:\Python311\python.exe"), None);
        assert_eq!(linux_interpreter_note("py", &[]), None);
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
        std::fs::write(d.join(".venv").join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        let cfg = Config { python_venv: Some(Sourced { value: d.join(".venv"), from: project_source() }), ..Config::default() };
        let p = fake(|_| says("Python 3.12.1"));
        let e = resolve_with(&cfg, &lookup(&[], None), &p, &mut ProbeCache::off(), "py").unwrap();
        assert_eq!(e.argv, [python.into_os_string()]);
    }
}
