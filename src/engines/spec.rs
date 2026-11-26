//! EngineSpec and the built-in engine table (INTERNALS 7.1, 7.2).
//!
//! Phase 1 registers py, js and ts; c, cpp and rust join with their toolchains in Phase 2
//! (ROADMAP rule 2). Scanning profiles are not here: the lexer takes them from the static table
//! in `syntax::scan` (3.8), which a test keeps in step with this one.

use std::fmt;
use std::rc::Rc;

/// A toolchain version, `major.minor.patch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u32, pub u32, pub u32);

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

impl Version {
    /// `3.11.6`, as the probe cache stores it.
    pub fn parse(s: &str) -> Option<Version> {
        three_numbers(s)
    }

    /// How a minimum is written in messages: `3.10`, `22.13`.
    pub fn short(&self) -> String {
        format!("{}.{}", self.0, self.1)
    }
}

/// How a probe's output names the version (7.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VersionPattern {
    /// `Python (\d+)\.(\d+)\.(\d+)` anywhere in the output.
    Python,
    /// `^v(\d+)\.(\d+)\.(\d+)` at the start of a line.
    Node,
    /// The run engines: the first `(\d+)\.(\d+)(?:\.(\d+))?` (`go version go1.22.1 linux/amd64`
    /// is 1.22.1); a missing part is 0 (7.7).
    Generic,
    /// java: `^\S+ (\d+)(?:\.(\d+))?(?:\.(\d+))?` on the first line (`openjdk 21 2023-09-19` is
    /// 21.0.0) (7.7).
    Java,
}

/// `(\d+)(?:\.(\d+))?(?:\.(\d+))?` at the start of `s`, a missing part 0; `need_dot`: the
/// minor part is required (Generic's pattern).
fn up_to_three(s: &str, need_dot: bool) -> Option<Version> {
    let mut nums = [0u32; 3];
    let mut rest = s;
    for (i, slot) in nums.iter_mut().enumerate() {
        if i > 0 {
            match rest.strip_prefix('.') {
                Some(r) if r.starts_with(|c: char| c.is_ascii_digit()) => rest = r,
                _ if i == 1 && need_dot => return None,
                _ => break,
            }
        }
        let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        *slot = rest[..end].parse().ok()?;
        rest = &rest[end..];
    }
    Some(Version(nums[0], nums[1], nums[2]))
}

/// `(\d+)\.(\d+)\.(\d+)` at the start of `s`.
fn three_numbers(s: &str) -> Option<Version> {
    let mut parts = s.splitn(3, '.');
    let mut num = |last: bool| -> Option<u32> {
        let p = parts.next()?;
        let digits: &str = if last { &p[..p.find(|c: char| !c.is_ascii_digit()).unwrap_or(p.len())] } else { p };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    };
    Some(Version(num(false)?, num(false)?, num(true)?))
}

impl VersionPattern {
    pub fn find(self, output: &str) -> Option<Version> {
        match self {
            VersionPattern::Python => output.match_indices("Python ").find_map(|(i, m)| three_numbers(&output[i + m.len()..])),
            VersionPattern::Node => output.lines().find_map(|l| three_numbers(l.strip_prefix('v')?)),
            // Each digit run that doesn't continue an earlier one (`go1.22.1`'s `1`).
            VersionPattern::Generic => output
                .char_indices()
                .filter(|&(i, c)| c.is_ascii_digit() && !output[..i].ends_with(|p: char| p.is_ascii_digit() || p == '.'))
                .find_map(|(i, _)| up_to_three(&output[i..], true)),
            VersionPattern::Java => {
                let first = output.lines().next()?;
                let (_, rest) = first.split_once(' ')?;
                up_to_three(rest, false)
            }
        }
    }

    /// Its name in the probe cache's spec hash (7.7).
    pub fn name(self) -> &'static str {
        match self {
            VersionPattern::Python => "python",
            VersionPattern::Node => "node",
            VersionPattern::Generic => "generic",
            VersionPattern::Java => "java",
        }
    }
}

/// How an engine runs its blocks (7.1): a worker over VEP, or a program per block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineKind {
    Bridged,
    Run,
}

/// A built-in engine (7.1; the fields Phase 1 uses).
#[derive(Debug)]
pub struct EngineSpec {
    pub id: Rc<str>,
    pub kind: EngineKind,
    /// A run engine's temp-file extension (7.2, 17.3).
    pub ext: Option<Rc<str>>,
    pub language: Rc<str>,
    pub aliases: Vec<Rc<str>>,
    /// The engine whose session runs this one's units: `js` for ts, which has no process.
    pub session_of: Option<Rc<str>>,
    /// argv prefixes tried in order; `[0]` is found on PATH (7.5).
    pub candidates: Vec<Vec<Rc<str>>>,
    /// Appended to a worker's argv; `{adapter}` is the adapter's path (8.3, 14.1).
    pub fixed_args: Vec<Rc<str>>,
    /// Added to the host's environment for probes and workers (7.7, 8.3).
    pub env: Vec<(Rc<str>, Rc<str>)>,
    pub min_version: Option<Version>,
    /// The probe's arguments and how to read its output (7.2, 7.7).
    pub probe: Option<(Vec<Rc<str>>, VersionPattern)>,
    /// Unix: the worker speaks the protocol on two fds of its own, named in `V2_VEP_FDS`, so its
    /// fds 0 and 1 can be the null device and stderr. For an adapter without `dup2` (9.9).
    pub protocol_fds: bool,
    /// A custom engine's settings (17.1); `None` for the built-in engines.
    pub custom: Option<Rc<CustomCfg>>,
}

fn strs(items: &[&str]) -> Vec<Rc<str>> {
    items.iter().map(|s| Rc::from(*s)).collect()
}

/// py, js and ts, per 7.2; `py -3` is a Windows candidate only.
pub fn builtins() -> Vec<EngineSpec> {
    let mut py_candidates = vec![strs(&["python3"]), strs(&["python"])];
    if cfg!(windows) {
        py_candidates.push(strs(&["py", "-3"]));
    }
    vec![
        EngineSpec {
            id: "py".into(),
            kind: EngineKind::Bridged,
            ext: None,
            language: "Python".into(),
            aliases: strs(&["python"]),
            session_of: None,
            candidates: py_candidates,
            fixed_args: strs(&["-B", "-u", "{adapter}"]),
            env: vec![("PYTHONUTF8".into(), "1".into()), ("PYTHONIOENCODING".into(), "utf-8".into())],
            min_version: Some(Version(3, 10, 0)),
            probe: Some((strs(&["--version"]), VersionPattern::Python)),
            protocol_fds: false,
            custom: None,
        },
        EngineSpec {
            id: "js".into(),
            kind: EngineKind::Bridged,
            ext: None,
            language: "JavaScript".into(),
            aliases: strs(&["javascript"]),
            session_of: None,
            candidates: vec![strs(&["node"])],
            // A .ts file that imports another, in a folder whose package.json has no "type", makes
            // Node advise "type": "module", which would break CommonJS files there (7.2).
            fixed_args: strs(&["--disable-warning=MODULE_TYPELESS_PACKAGE_JSON", "{adapter}"]),
            env: Vec::new(),
            min_version: Some(Version(22, 13, 0)),
            probe: Some((strs(&["--version"]), VersionPattern::Node)),
            protocol_fds: true,
            custom: None,
        },
        EngineSpec {
            id: "ts".into(),
            kind: EngineKind::Bridged,
            ext: None,
            language: "TypeScript".into(),
            aliases: strs(&["typescript"]),
            session_of: Some("js".into()),
            candidates: Vec::new(),
            fixed_args: Vec::new(),
            env: Vec::new(),
            min_version: None,
            probe: None,
            protocol_fds: false,
            custom: None,
        },
    ]
    .into_iter()
    .chain(run_builtins())
    .collect()
}

/// 7.2's Run rows: the built-in run engines.
fn run_builtins() -> Vec<EngineSpec> {
    let generic = |args: &[&str]| Some((strs(args), VersionPattern::Generic));
    let win = cfg!(windows);
    // id, language, aliases, candidates, fixed args, ext, probe, minimum
    let rows: Vec<(&str, &str, &[&str], Vec<Vec<Rc<str>>>, Vec<Rc<str>>, &str, Option<(Vec<Rc<str>>, VersionPattern)>, Option<Version>)> = vec![
        // Windows: Git Bash only, found by `resolve::git_bash`, never a PATH `bash` (7.6).
        ("bash", "shell", &["sh"], if win { Vec::new() } else { vec![strs(&["bash"])] }, strs(&["{file}"]), "sh", generic(&["--version"]), None),
        (
            "pwsh",
            "PowerShell",
            &["powershell"],
            if win { vec![strs(&["pwsh"]), strs(&["powershell"])] } else { vec![strs(&["pwsh"])] },
            if win { strs(&["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "{file}"]) } else { strs(&["-NoProfile", "-File", "{file}"]) },
            "ps1",
            generic(&["--version"]),
            None,
        ),
        // Not probed; outside Windows never found (7.6).
        ("cmd", "cmd", &[], if win { vec![strs(&["cmd"])] } else { Vec::new() }, strs(&["/d", "/q", "/c", "{file}"]), "bat", None, None),
        ("lua", "Lua", &[], vec![strs(&["lua"]), strs(&["lua5.4"])], strs(&["{file}"]), "lua", generic(&["-v"]), None),
        ("ruby", "Ruby", &[], vec![strs(&["ruby"])], strs(&["{file}"]), "rb", generic(&["--version"]), None),
        ("go", "Go", &[], vec![strs(&["go"])], strs(&["run", "{file}"]), "go", generic(&["version"]), None),
        ("java", "Java", &[], vec![strs(&["java"])], strs(&["{file}"]), "java", Some((strs(&["--version"]), VersionPattern::Java)), Some(Version(11, 0, 0))),
        ("zig", "Zig", &[], vec![strs(&["zig"])], strs(&["run", "{file}"]), "zig", generic(&["version"]), None),
        ("cs", "C#", &["csharp"], vec![strs(&["dotnet"])], strs(&["run", "{file}"]), "cs", generic(&["--version"]), Some(Version(10, 0, 0))),
    ];
    rows.into_iter()
        .map(|(id, language, aliases, candidates, fixed_args, ext, probe, min_version)| EngineSpec {
            id: id.into(),
            kind: EngineKind::Run,
            ext: Some(ext.into()),
            language: language.into(),
            aliases: strs(aliases),
            session_of: None,
            candidates,
            fixed_args,
            env: Vec::new(),
            min_version,
            probe,
            protocol_fds: false,
            custom: None,
        })
        .collect()
}

/// A custom engine's kind (17.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustomKind {
    Run,
    Bridged,
}

/// A custom engine's validated settings (17.1): from an `[engines.<tag>]` table in `v2.toml` or
/// a `define_engine` call. `command`'s paths are already rewritten against the declaring file's
/// folder (18.1), and `V2_ENGINE_<TAG>` has replaced `command[0]` (18.4).
#[derive(Clone, Debug, PartialEq)]
pub struct CustomCfg {
    pub kind: CustomKind,
    pub command: Vec<String>,
    pub ext: String,
    pub startup_timeout: Option<f64>,
    pub call_timeout: Option<f64>,
    /// Where it was declared, as messages and doctor name it: the `v2.toml`'s path, a
    /// `V2_ENGINE_<TAG>` variable's name, or a call site (`main.v2:3`).
    pub origin: String,
}

/// The keys a custom engine's table takes (17.1).
pub const CUSTOM_KEYS: &[&str] = &["kind", "command", "ext", "startup_timeout", "call_timeout"];

/// `[a-z][a-z0-9_]{0,31}`, and not one of V2's own words (17.1).
pub fn check_custom_tag(tag: &str) -> Result<(), String> {
    let mut chars = tag.chars();
    let ok = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && tag.len() <= 32;
    if !ok {
        return Err(format!("invalid engine tag '{tag}': a tag is a lowercase letter, then up to 31 lowercase letters, digits and _"));
    }
    if matches!(tag, "v2" | "export" | "import") {
        return Err(format!("'{tag}' is reserved"));
    }
    Ok(())
}

fn toml_type(v: &toml::Value) -> &'static str {
    match v {
        toml::Value::String(_) => "string",
        toml::Value::Integer(_) => "integer",
        toml::Value::Float(_) => "float",
        toml::Value::Boolean(_) => "boolean",
        toml::Value::Datetime(_) => "datetime",
        toml::Value::Array(_) => "array",
        toml::Value::Table(_) => "table",
    }
}

/// 17.1's validator, shared by `v2.toml` and `define_engine`: the table's keys checked and typed.
/// `command` comes back as written: the caller rewrites its paths (18.1). Errors are 17.1's texts,
/// which the caller places (a config error, or `define_engine: …`).
pub fn validate(tag: &str, table: &toml::Table, origin: String) -> Result<CustomCfg, String> {
    check_custom_tag(tag)?;
    for k in table.keys() {
        if !CUSTOM_KEYS.contains(&k.as_str()) {
            return Err(match crate::runtime::error::did_you_mean(k, CUSTOM_KEYS.iter().copied()) {
                Some(near) => format!("unknown key '{k}' (did you mean '{near}'?)"),
                None => format!("unknown key '{k}'"),
            });
        }
    }
    let kind = match table.get("kind") {
        Some(toml::Value::String(s)) if s == "run" => CustomKind::Run,
        Some(toml::Value::String(s)) if s == "bridged" => CustomKind::Bridged,
        Some(toml::Value::String(s)) => return Err(format!("'kind' must be \"run\" or \"bridged\", got \"{s}\"")),
        Some(other) => return Err(format!("'kind' must be \"run\" or \"bridged\", got {}", toml_type(other))),
        None => return Err("'kind' is required: \"run\" or \"bridged\"".into()),
    };
    let command: Vec<String> = match table.get("command") {
        Some(toml::Value::Array(items)) => {
            let list: Option<Vec<String>> = items.iter().map(|i| i.as_str().map(str::to_string)).collect();
            match list {
                Some(l) if !l.is_empty() && l.iter().all(|w| !w.is_empty()) => l,
                _ => return Err("'command' must be a non-empty array of non-empty strings".into()),
            }
        }
        Some(toml::Value::String(s)) => return Err(format!("'command' must be an array: write [\"{s}\"]")),
        Some(other) => return Err(format!("'command' must be an array of strings, got {}", toml_type(other))),
        None => return Err("'command' is required".into()),
    };
    let has_file = command.iter().any(|w| w.contains("{file}"));
    match kind {
        CustomKind::Run if !has_file => return Err("command must contain {file}".into()),
        CustomKind::Bridged if has_file => return Err("a bridged engine's command can't contain {file}: the adapter gets the code over the protocol".into()),
        _ => {}
    }
    let ext = match (table.get("ext"), kind) {
        (None, _) => tag.to_string(),
        (Some(_), CustomKind::Bridged) => return Err("'ext' does not apply to bridged engines".into()),
        (Some(toml::Value::String(s)), CustomKind::Run) => {
            let e = s.strip_prefix('.').unwrap_or(s);
            if e.is_empty() || !e.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(format!("'ext' must be letters, digits and _, got \"{s}\""));
            }
            e.to_string()
        }
        (Some(other), CustomKind::Run) => return Err(format!("'ext' must be a string, got {}", toml_type(other))),
    };
    let seconds = |key: &str| -> Result<Option<f64>, String> {
        let x = match table.get(key) {
            None => return Ok(None),
            Some(toml::Value::Integer(n)) => *n as f64,
            Some(toml::Value::Float(x)) => *x,
            Some(other) => return Err(format!("'{key}' must be a non-negative number of seconds, got {}", toml_type(other))),
        };
        if !x.is_finite() || x < 0.0 {
            return Err(format!("'{key}' must be a non-negative number of seconds, got {x}"));
        }
        Ok(Some(x))
    };
    let startup_timeout = seconds("startup_timeout")?;
    if startup_timeout.is_some() && kind == CustomKind::Run {
        return Err("'startup_timeout' does not apply to run engines".into());
    }
    let call_timeout = seconds("call_timeout")?;
    Ok(CustomCfg { kind, command, ext, startup_timeout, call_timeout, origin })
}

/// A custom engine's spec (17.1): no candidates, minimum or probe; its language is its tag.
pub fn custom_spec(tag: &str, cfg: CustomCfg) -> EngineSpec {
    EngineSpec {
        id: tag.into(),
        kind: if cfg.kind == CustomKind::Run { EngineKind::Run } else { EngineKind::Bridged },
        ext: (cfg.kind == CustomKind::Run).then(|| cfg.ext.as_str().into()),
        language: tag.into(),
        aliases: Vec::new(),
        session_of: None,
        candidates: Vec::new(),
        fixed_args: Vec::new(),
        env: Vec::new(),
        min_version: None,
        probe: None,
        protocol_fds: false,
        custom: Some(Rc::new(cfg)),
    }
}

impl EngineSpec {
    /// A custom engine of kind `bridged` (17.5).
    pub fn is_bridged_custom(&self) -> bool {
        self.custom.as_ref().is_some_and(|c| c.kind == CustomKind::Bridged)
    }

    /// Why version `v` can't be used (7.2), or `None` when it can: below the minimum, or a Node
    /// 23.0 or 23.1 (one Node rule for js and ts, since ts runs on the js engine).
    pub fn too_old(&self, v: Version) -> Option<String> {
        let min = self.min_version?;
        if v < min {
            return Some(format!("too old: {v} < {}", min.short()));
        }
        if self.is_node() && !node_supported(v) {
            return Some(format!("not supported: {v} has no type stripping; V2 needs {}", self.needs(min)));
        }
        None
    }

    /// The minimum as messages state it: `3.10+`, or `22.13+, not 23.0 or 23.1` for Node.
    pub fn needs(&self, min: Version) -> String {
        if self.is_node() {
            format!("{}+, not 23.0 or 23.1", min.short())
        } else {
            format!("{}+", min.short())
        }
    }

    fn is_node(&self) -> bool {
        matches!(self.probe, Some((_, VersionPattern::Node)))
    }
}

/// Whether a Node version has `module.stripTypeScriptTypes`: 22.13+ on the 22 line, or 23.2+
/// (7.2).
pub fn node_supported(node: Version) -> bool {
    match node.0 {
        22 => node >= Version(22, 13, 0),
        23 => node >= Version(23, 2, 0),
        major => major > 23,
    }
}

/// Whether a Node version loads `.ts`, `.mts` and `.cts` files (`@import … from ts."./x.ts"`) only
/// with `--experimental-strip-types`: before 22.18 on the 22 line, 23.6 on the 23 line (7.2).
pub fn node_needs_strip_flag(node: Version) -> bool {
    match node.0 {
        22 => node < Version(22, 18, 0),
        23 => node < Version(23, 6, 0),
        _ => false,
    }
}

/// The install line of an `EngineNotFound` message, for this system (7.8).
pub fn install_hint(id: &str) -> String {
    match (id, std::env::consts::OS) {
        ("py", "windows") => "winget install -e --id Python.Python.3.13 (or https://www.python.org/downloads/), then open a new terminal".into(),
        ("py", "macos") => "brew install python (or https://www.python.org/downloads/)".into(),
        ("py", _) => {
            let (family, sudo) = this_linux();
            format!("{} (or https://www.python.org/downloads/)", package_line(family, sudo, "python3", "python3", None))
        }
        (_, "windows") => "winget install -e --id OpenJS.NodeJS.LTS (or https://nodejs.org), then open a new terminal".into(),
        (_, "macos") => "brew install node (or https://nodejs.org)".into(),
        _ => "https://nodejs.org/en/download (distro packages may be older than 22.13)".into(),
    }
}

/// The variables an engine's program reads as it starts, set here: a stale `PYTHONHOME` or a
/// preload in `NODE_OPTIONS` stops it before V2 sees a version (8.4, 19.2). Values whole, as the
/// wrong folder is often the last, with their control characters escaped, as a path's (2.1).
pub fn startup_variables(id: &str) -> Vec<(&'static str, String)> {
    startup_values(id, |n| std::env::var_os(n))
}

/// `startup_variables` with the environment read through `get`.
fn startup_values(id: &str, get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Vec<(&'static str, String)> {
    startup_names(id).iter().filter_map(|n| get(n).filter(|v| !v.is_empty()).map(|v| (*n, crate::syntax::span::shown_path(&v.to_string_lossy())))).collect()
}

fn startup_names(id: &str) -> &'static [&'static str] {
    match id {
        "py" => &["PYTHONHOME", "PYTHONPATH"],
        "js" => &["NODE_OPTIONS"],
        _ => &[],
    }
}

/// Of those, the ones the version probe reads too: Node checks `NODE_OPTIONS` before it answers
/// `--version` (a bad option stops it), Python answers before it reads `PYTHONHOME` (7.8).
pub fn probe_variables(id: &str) -> &'static [&'static str] {
    match id {
        "js" => &["NODE_OPTIONS"],
        _ => &[],
    }
}

/// What doctor runs to see that the program really starts, when a startup variable or an
/// address-space limit is set (19.2): `--version` answers before Python reads `PYTHONHOME`, and
/// before Node runs a `--require` preload or reserves its memory (8.4). Python imports what V2's
/// worker imports as it starts (py.py, step 1), as a stale `PYTHONPATH`, another version's
/// standard library, starts Python but breaks `re`; without the folder `-c` puts first on
/// `sys.path` (the current one), as the worker, a script in V2's run folder, has none of it.
pub fn start_args(id: &str) -> &'static [&'static str] {
    match id {
        "py" => &["-c", PY_START_CHECK],
        "js" => &["-e", "0"],
        _ => &[],
    }
}

#[cfg(not(windows))]
const PY_START_CHECK: &str = "import sys; sys.path[:1] = [p for p in sys.path[:1] if p]; import os, io, json, ast, types, builtins, inspect, importlib, keyword, traceback, linecache, weakref, threading, signal, base64, codecs, collections, collections.abc, numbers, atexit, errno, warnings";
#[cfg(windows)]
const PY_START_CHECK: &str = "import sys; sys.path[:1] = [p for p in sys.path[:1] if p]; import os, io, json, ast, types, builtins, inspect, importlib, keyword, traceback, linecache, weakref, threading, signal, base64, codecs, collections, collections.abc, numbers, atexit, errno, warnings, msvcrt, ctypes";

/// How doctor names that check in its note (19.2): `-e 0 failed`, or for Python, whose check is
/// long, what it does.
pub fn start_check_failed(id: &str) -> String {
    match id {
        "py" => "can't import the standard modules V2's worker uses".into(),
        _ => format!("{} failed", start_args(id).join(" ")),
    }
}

/// `node didn't start: NODE_OPTIONS is set (--bogus), which it reads as it starts: unset it or
/// fix it` (7.8, 19.2), for the variables `set` (each `startup_variables`' pair).
pub fn startup_fix(id: &str, set: &[(&str, String)]) -> String {
    let program = if id == "py" { "python" } else { "node" };
    format!("{program} didn't start: {}", startup_vars_fix(set))
}

/// `NODE_OPTIONS is set (--bogus), which it reads as it starts: unset it or fix it`, the end of
/// a worker's startup crash (8.4) and of `startup_fix`.
pub fn startup_vars_fix(set: &[(&str, String)]) -> String {
    let vars: Vec<String> = set.iter().map(|(n, v)| format!("{n} is set ({v})")).collect();
    let it = if set.len() == 1 { "it" } else { "them" };
    format!("{}, which it reads as it starts: unset {it} or fix {it}", vars.join(" and "))
}

/// Unix, under an address-space limit (`ulimit -v`) below what the engine needs to start: `a
/// memory limit (ulimit -v) of 585 MB is set for this shell, and Node.js reserves about 1 GB as
/// it starts: raise it` (8.4, 19.2). A higher limit isn't why it failed: `None`.
pub fn limit_fix(id: &str) -> Option<String> {
    #[cfg(unix)]
    let limit = crate::address_limit();
    #[cfg(not(unix))]
    let limit: Option<u64> = None;
    limit_fix_at(id, limit)
}

/// `limit_fix` under a limit of `bytes`. Node.js 22 starts in about 700 MB of address space,
/// Python with V2's worker in about 30 MB (round 23): a limit under 2 GB, or 256 MB, may be why.
pub fn limit_fix_at(id: &str, bytes: Option<u64>) -> Option<String> {
    // ts runs on Node.js too (7.6).
    let node = matches!(id, "js" | "ts");
    let enough: u64 = if node { 2 << 30 } else { 256 << 20 };
    let needs = if node { "Node.js reserves about 1 GB" } else { "the engine may need more" };
    bytes.filter(|&b| b < enough).map(|b| format!("a memory limit (ulimit -v) of {} MB is set for this shell, and {needs} as it starts: raise it", b >> 20))
}

/// Which package manager a Linux uses, from `/etc/os-release`'s `ID` and `ID_LIKE` (7.8).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Family {
    Apt,
    Dnf,
    Unknown,
}

pub fn family_of(os_release: &str) -> Family {
    let ids: Vec<&str> = os_release
        .lines()
        .filter_map(|l| l.strip_prefix("ID=").or_else(|| l.strip_prefix("ID_LIKE=")))
        .flat_map(|v| v.trim_matches(['"', '\'']).split_whitespace())
        .collect();
    let has = |names: &[&str]| ids.iter().any(|i| names.contains(i));
    match (has(&["debian", "ubuntu"]), has(&["fedora", "rhel", "centos"])) {
        (true, false) => Family::Apt,
        (false, true) => Family::Dnf,
        _ => Family::Unknown,
    }
}

/// This system's family and `sudo ` (none for root: a container, a fresh WSL distro) (7.8).
pub fn this_linux() -> (Family, &'static str) {
    static IT: std::sync::OnceLock<(Family, &'static str)> = std::sync::OnceLock::new();
    *IT.get_or_init(|| {
        let family = family_of(&std::fs::read_to_string("/etc/os-release").unwrap_or_default());
        #[cfg(unix)]
        // SAFETY: geteuid has no preconditions.
        let root = unsafe { libc::geteuid() } == 0;
        #[cfg(not(unix))]
        let root = false;
        (family, if root { "" } else { "sudo " })
    })
}

/// The command that installs a package: one manager's when the family is known, else both, as
/// `apt … / dnf …`, or as `on Debian and Ubuntu, …; on Fedora, …` when `named` (7.8). For root
/// (a container, a fresh WSL distro, which have no package lists yet) apt's starts with
/// `apt update && `.
pub fn package_line(family: Family, sudo: &str, apt: &str, dnf: &str, named: Option<()>) -> String {
    let apt = if sudo.is_empty() { format!("apt update && apt install {apt}") } else { format!("{sudo}apt install {apt}") };
    match (family, named) {
        (Family::Apt, _) => apt,
        (Family::Dnf, _) => format!("{sudo}dnf install {dnf}"),
        (Family::Unknown, None) => format!("{apt} / {sudo}dnf install {dnf}"),
        (Family::Unknown, Some(())) => format!("on Debian and Ubuntu, {apt}; on Fedora, {sudo}dnf install {dnf}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_rows() {
        let specs = builtins();
        let ids: Vec<&str> = specs.iter().map(|s| &*s.id).collect();
        assert_eq!(ids, ["py", "js", "ts", "bash", "pwsh", "cmd", "lua", "ruby", "go", "java", "zig", "cs"]);
        // 7.2's Run rows: each one's temp-file extension and `{file}` in its fixed args.
        for s in specs.iter().filter(|s| s.kind == EngineKind::Run) {
            assert!(s.ext.is_some() && s.fixed_args.iter().any(|a| &**a == "{file}"), "{}", s.id);
        }
        let java = specs.iter().find(|s| &*s.id == "java").unwrap();
        assert_eq!(java.min_version, Some(Version(11, 0, 0)));
        let py = &specs[0];
        let first: Vec<&str> = py.candidates.iter().map(|c| &*c[0]).collect();
        assert_eq!(&first[..2], ["python3", "python"]);
        assert_eq!(py.candidates.len(), if cfg!(windows) { 3 } else { 2 }, "py -3 is Windows-only");
        assert_eq!(py.min_version, Some(Version(3, 10, 0)));
        assert_eq!(specs[1].min_version, Some(Version(22, 13, 0)));
        assert_eq!(specs[2].session_of.as_deref(), Some("js"));
        assert!(py.env.iter().any(|(k, v)| &**k == "PYTHONUTF8" && &**v == "1"));
    }

    #[test]
    fn doctors_python_check_imports_what_the_worker_imports_as_it_starts() {
        // Round 19, 19.2: a stale PYTHONPATH passes `python -c pass` but breaks the worker's
        // first imports (py.py, step 1, up to the protocol's fds), so the check imports them all.
        let check = start_args("py")[1];
        let mut imported: Vec<&str> = check.split("; import ").nth(1).unwrap().split(", ").collect();
        let step1 = crate::adapters::PY.split("IN = os.dup(0)").next().unwrap();
        let mut modules: Vec<&str> = Vec::new();
        for line in step1.lines().map(str::trim).filter(|l| l.starts_with("import ")) {
            let windows_only = matches!(line, "import msvcrt, ctypes");
            if windows_only && !cfg!(windows) {
                continue;
            }
            modules.extend(line["import ".len()..].split(", ").filter(|m| *m != "sys"));
        }
        assert!(modules.len() > 20, "{modules:?}");
        imported.sort_unstable();
        modules.sort_unstable();
        assert_eq!(imported, modules, "the worker's first imports and doctor's check");
        // Not the folder `-c` puts first on sys.path (the current one): the worker has none.
        assert!(check.starts_with("import sys; sys.path[:1] = [p for p in sys.path[:1] if p]; import "), "{check}");
        assert_eq!(start_args("js"), ["-e", "0"]);
        assert_eq!(start_check_failed("js"), "-e 0 failed");
    }

    #[test]
    fn a_startup_crash_names_the_variables_and_what_to_do() {
        // Round 19, 8.4: the run's message ends like doctor's fix line.
        let one = [("PYTHONPATH", "/old/lib".to_string())];
        assert_eq!(startup_vars_fix(&one), "PYTHONPATH is set (/old/lib), which it reads as it starts: unset it or fix it");
        assert_eq!(startup_fix("py", &one), "python didn't start: PYTHONPATH is set (/old/lib), which it reads as it starts: unset it or fix it");
        let two = [("PYTHONHOME", "/nope".to_string()), ("PYTHONPATH", "/old/lib".to_string())];
        assert_eq!(startup_vars_fix(&two), "PYTHONHOME is set (/nope) and PYTHONPATH is set (/old/lib), which it reads as it starts: unset them or fix them");
    }

    #[test]
    fn startup_variables_are_shown_whole_and_escaped() {
        // Round 22 (3): cut at 80 characters with no mark, a long PYTHONPATH read as another
        // folder that isn't there; the wrong folder is often its last entry. Whole, its control
        // characters escaped, as a printed path's (2.1); an empty one is unset.
        let long = format!("C:\\{};C:\\b\u{1b}[2J", "d".repeat(100));
        let env = |n: &str| match n {
            "PYTHONPATH" => Some(std::ffi::OsString::from(&long)),
            "PYTHONHOME" => Some(std::ffi::OsString::new()),
            "NODE_OPTIONS" => Some(std::ffi::OsString::from("--max-old-space-size=32")),
            _ => None,
        };
        let shown = format!("C:\\{};C:\\b\\u{{1b}}[2J", "d".repeat(100));
        assert_eq!(startup_values("py", env), [("PYTHONPATH", shown)]);
        assert_eq!(startup_values("js", env), [("NODE_OPTIONS", "--max-old-space-size=32".to_string())]);
        assert!(startup_values("ts", env).is_empty());
    }

    #[test]
    fn tables_agree_with_scan_and_config() {
        // The lexer's alias table and config's built-in tags must know every engine here.
        for spec in builtins() {
            assert_eq!(crate::syntax::scan::canonical_tag(&spec.id), &*spec.id);
            for alias in &spec.aliases {
                assert_eq!(crate::syntax::scan::canonical_tag(alias), &*spec.id, "{alias}");
            }
            assert!(crate::config::BUILTIN_TAGS.contains(&&*spec.id), "{}", spec.id);
        }
    }

    #[test]
    fn version_patterns() {
        let py = VersionPattern::Python;
        assert_eq!(py.find("Python 3.11.6\r\n"), Some(Version(3, 11, 6)));
        assert_eq!(py.find("Python 3.13.0rc1"), Some(Version(3, 13, 0)));
        assert_eq!(py.find("Python was not found; run without arguments to install"), None);
        assert_eq!(py.find("Python 3.12"), None);
        let node = VersionPattern::Node;
        assert_eq!(node.find("v24.11.1\n"), Some(Version(24, 11, 1)));
        assert_eq!(node.find("noise\nv18.19.1"), Some(Version(18, 19, 1)));
        assert_eq!(node.find("node v24.11.1"), None);
        assert_eq!(Version(3, 10, 0).short(), "3.10");
        assert_eq!(Version(3, 6, 8).to_string(), "3.6.8");
    }

    #[test]
    fn node_versions() {
        for (v, want) in [
            (Version(18, 19, 1), false),
            (Version(22, 12, 0), false),
            (Version(22, 13, 0), true),
            (Version(23, 1, 0), false),
            (Version(23, 2, 0), true),
            (Version(24, 11, 1), true),
        ] {
            assert_eq!(node_supported(v), want, "{v}");
        }
        // js and ts share the rule; Python has only its minimum.
        let specs = builtins();
        let (py, js) = (&specs[0], &specs[1]);
        assert_eq!(py.too_old(Version(3, 9, 6)).as_deref(), Some("too old: 3.9.6 < 3.10"));
        assert_eq!(py.too_old(Version(3, 10, 0)), None);
        assert_eq!(js.too_old(Version(20, 18, 0)).as_deref(), Some("too old: 20.18.0 < 22.13"));
        assert_eq!(js.too_old(Version(23, 1, 0)).as_deref(), Some("not supported: 23.1.0 has no type stripping; V2 needs 22.13+, not 23.0 or 23.1"));
        assert_eq!(js.too_old(Version(22, 13, 0)), None);
        assert_eq!(js.too_old(Version(24, 11, 1)), None);
    }

    #[test]
    fn install_hints_are_7_8s_table() {
        let (py, node) = match std::env::consts::OS {
            "windows" => (
                "winget install -e --id Python.Python.3.13 (or https://www.python.org/downloads/), then open a new terminal",
                "winget install -e --id OpenJS.NodeJS.LTS (or https://nodejs.org), then open a new terminal",
            ),
            "macos" => ("brew install python (or https://www.python.org/downloads/)", "brew install node (or https://nodejs.org)"),
            _ => (
                // This system's own line: its tests are below.
                &*format!("{} (or https://www.python.org/downloads/)", { let (f, s) = this_linux(); package_line(f, s, "python3", "python3", None) }),
                "https://nodejs.org/en/download (distro packages may be older than 22.13)",
            ),
        };
        assert_eq!((install_hint("py"), install_hint("js"), install_hint("ts")), (py.to_string(), node.to_string(), node.to_string()));
    }

    #[test]
    fn linux_install_lines_fit_the_system() {
        // 7.8: apt or dnf by /etc/os-release, and no sudo for root.
        assert_eq!(family_of("NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n"), Family::Apt);
        assert_eq!(family_of("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"), Family::Apt);
        assert_eq!(family_of("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n"), Family::Dnf);
        assert_eq!(family_of("ID=alpine\n"), Family::Unknown);
        assert_eq!(family_of(""), Family::Unknown);
        assert_eq!(package_line(Family::Apt, "", "python3", "python3", None), "apt update && apt install python3");
        assert_eq!(package_line(Family::Apt, "sudo ", "python3", "python3", None), "sudo apt install python3");
        assert_eq!(package_line(Family::Unknown, "", "python3", "python3", None), "apt update && apt install python3 / dnf install python3");
        assert_eq!(package_line(Family::Dnf, "sudo ", "python3", "python3", None), "sudo dnf install python3");
        assert_eq!(package_line(Family::Unknown, "sudo ", "python3", "python3", None), "sudo apt install python3 / sudo dnf install python3");
        assert_eq!(
            package_line(Family::Unknown, "sudo ", "libatomic1", "libatomic", Some(())),
            "on Debian and Ubuntu, sudo apt install libatomic1; on Fedora, sudo dnf install libatomic"
        );
    }

    #[test]
    fn only_a_limit_under_what_the_engine_needs_is_blamed() {
        // Round 23's review, 8.4: Node.js needs about 1 GB to start, Python little; a limit far
        // above that isn't why a program failed.
        let node = "a memory limit (ulimit -v) of 48 MB is set for this shell, and Node.js reserves about 1 GB as it starts: raise it";
        assert_eq!(limit_fix_at("js", Some(48 << 20)).as_deref(), Some(node));
        assert_eq!(limit_fix_at("ts", Some(48 << 20)).as_deref(), Some(node));
        assert!(limit_fix_at("js", Some((2 << 30) - 1)).is_some());
        assert_eq!(limit_fix_at("js", Some(2 << 30)), None);
        assert_eq!(limit_fix_at("js", Some(8_000_000 << 10)), None);
        assert_eq!(limit_fix_at("js", None), None);
        assert_eq!(limit_fix_at("py", Some(100 << 20)).as_deref(), Some("a memory limit (ulimit -v) of 100 MB is set for this shell, and the engine may need more as it starts: raise it"));
        assert_eq!(limit_fix_at("py", Some(1 << 30)), None);
    }

    #[test]
    fn custom_tables_are_validated() {
        // INTERNALS 17.1: kinds, commands, keys and tags.
        let t = |text: &str| -> toml::Table { text.parse().unwrap() };
        let ok = validate("lua", &t("kind = \"bridged\"\ncommand = [\"lua\", \"a.lua\"]\nstartup_timeout = 5"), "v2.toml".into()).unwrap();
        assert_eq!((ok.kind, ok.command.len(), ok.startup_timeout, ok.ext.as_str()), (CustomKind::Bridged, 2, Some(5.0), "lua"));
        let run = validate("r", &t("kind = \"run\"\ncommand = [\"Rscript\", \"{file}\"]\next = \".R\""), "v2.toml".into()).unwrap();
        assert_eq!(run.ext, "R");
        let err = |tag: &str, text: &str| validate(tag, &t(text), String::new()).unwrap_err();
        assert_eq!(err("r", "kind = \"run\"\ncommand = [\"Rscript\"]"), "command must contain {file}");
        assert!(err("x", "kind = \"bridged\"\ncommand = [\"x\", \"{file}\"]").starts_with("a bridged engine's command can't contain {file}"));
        assert_eq!(err("x", "command = [\"x\"]"), "'kind' is required: \"run\" or \"bridged\"");
        assert_eq!(err("x", "kind = \"bridged\"\ncommand = []"), "'command' must be a non-empty array of non-empty strings");
        assert_eq!(err("x", "kind = \"bridged\"\ncommand = [\"x\"]\next = \"y\""), "'ext' does not apply to bridged engines");
        assert_eq!(err("x", "kind = \"bridged\"\ncommand = [\"x\"]\ncall_timeout = -1"), "'call_timeout' must be a non-negative number of seconds, got -1");
        assert_eq!(err("x", "kind = \"bridged\"\ncomand = [\"x\"]"), "unknown key 'comand' (did you mean 'command'?)");
        assert_eq!(err("import", "kind = \"bridged\"\ncommand = [\"x\"]"), "'import' is reserved");
        assert!(err("9x", "kind = \"bridged\"\ncommand = [\"x\"]").starts_with("invalid engine tag '9x'"));
    }

    #[test]
    fn run_engine_version_patterns() {
        // 7.7: the first `\d+.\d+(.\d+)?` for run engines; java's first line.
        let g = VersionPattern::Generic;
        assert_eq!(g.find("go version go1.22.1 linux/amd64"), Some(Version(1, 22, 1)));
        assert_eq!(g.find("GNU bash, version 5.2.21(1)-release (x86_64-pc-linux-gnu)"), Some(Version(5, 2, 21)));
        assert_eq!(g.find("Lua 5.4.6  Copyright (C) 1994-2023 Lua.org, PUC-Rio"), Some(Version(5, 4, 6)));
        assert_eq!(g.find("ruby 3.3.0 (2023-12-25 revision 5124f9ac75) [x86_64-linux]"), Some(Version(3, 3, 0)));
        assert_eq!(g.find("0.14.1"), Some(Version(0, 14, 1)));
        assert_eq!(g.find("PowerShell 7.4"), Some(Version(7, 4, 0)));
        assert_eq!(g.find("no version here 7"), None);
        let j = VersionPattern::Java;
        assert_eq!(j.find("openjdk 21 2023-09-19\nOpenJDK Runtime"), Some(Version(21, 0, 0)));
        assert_eq!(j.find("java 17.0.2 2022-01-18 LTS"), Some(Version(17, 0, 2)));
        assert_eq!(j.find("openjdk 25.0.5 2025-10-21"), Some(Version(25, 0, 5)));
    }
}
