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
        }
    }

    /// Its name in the probe cache's spec hash (7.7).
    pub fn name(self) -> &'static str {
        match self {
            VersionPattern::Python => "python",
            VersionPattern::Node => "node",
        }
    }
}

/// A built-in engine (7.1; the fields Phase 1 uses).
#[derive(Debug)]
pub struct EngineSpec {
    pub id: Rc<str>,
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
            language: "Python".into(),
            aliases: strs(&["python"]),
            session_of: None,
            candidates: py_candidates,
            fixed_args: strs(&["-B", "-u", "{adapter}"]),
            env: vec![("PYTHONUTF8".into(), "1".into()), ("PYTHONIOENCODING".into(), "utf-8".into())],
            min_version: Some(Version(3, 10, 0)),
            probe: Some((strs(&["--version"]), VersionPattern::Python)),
            protocol_fds: false,
        },
        EngineSpec {
            id: "js".into(),
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
        },
        EngineSpec {
            id: "ts".into(),
            language: "TypeScript".into(),
            aliases: strs(&["typescript"]),
            session_of: Some("js".into()),
            candidates: Vec::new(),
            fixed_args: Vec::new(),
            env: Vec::new(),
            min_version: None,
            probe: None,
            protocol_fds: false,
        },
    ]
}

impl EngineSpec {
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
/// `apt … / dnf …`, or as `on Debian and Ubuntu, …; on Fedora, …` when `named` (7.8).
pub fn package_line(family: Family, sudo: &str, apt: &str, dnf: &str, named: Option<()>) -> String {
    match (family, named) {
        (Family::Apt, _) => format!("{sudo}apt install {apt}"),
        (Family::Dnf, _) => format!("{sudo}dnf install {dnf}"),
        (Family::Unknown, None) => format!("{sudo}apt install {apt} / {sudo}dnf install {dnf}"),
        (Family::Unknown, Some(())) => format!("on Debian and Ubuntu, {sudo}apt install {apt}; on Fedora, {sudo}dnf install {dnf}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_rows() {
        let specs = builtins();
        let ids: Vec<&str> = specs.iter().map(|s| &*s.id).collect();
        assert_eq!(ids, ["py", "js", "ts"]);
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
        assert_eq!(package_line(Family::Apt, "", "python3", "python3", None), "apt install python3");
        assert_eq!(package_line(Family::Dnf, "sudo ", "python3", "python3", None), "sudo dnf install python3");
        assert_eq!(package_line(Family::Unknown, "sudo ", "python3", "python3", None), "sudo apt install python3 / sudo dnf install python3");
        assert_eq!(
            package_line(Family::Unknown, "sudo ", "libatomic1", "libatomic", Some(())),
            "on Debian and Ubuntu, sudo apt install libatomic1; on Fedora, sudo dnf install libatomic"
        );
    }
}
