//! Tag resolution (INTERNALS 7.1, 7.3). Custom engines (7.4, 17.1) join in roadmap Phase 3.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::resolve::{CacheMode, Lookup, NotFound, Origin, ProbeCache, Prober, RealProber, ResolvedEngine, Resolver, Status};
use super::spec::{builtins, custom_spec, CustomCfg, EngineSpec};
use crate::config::Config;
use crate::runtime::error::{did_you_mean, fail, Exec};

type Resolution = Result<Rc<ResolvedEngine>, Rc<NotFound>>;

/// The language of a tag that DOCS 5 lists but this build doesn't register yet: compiled blocks
/// (roadmap Phase 2). Each row goes when its engine joins the registry (7.3).
pub(crate) fn later_language(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "c" => "C",
        "cpp" | "cxx" => "C++",
        "rust" | "rs" => "Rust",
        _ => return None,
    })
}

/// The engines of this run. Resolution is lazy and memoized per id, failures included (7.1).
pub struct Registry {
    specs: Vec<Rc<EngineSpec>>,
    cfg: Rc<Config>,
    lookup: Lookup,
    prober: Box<dyn Prober>,
    cache: RefCell<ProbeCache>,
    resolved: RefCell<HashMap<Rc<str>, Resolution>>,
}

impl Registry {
    pub fn new(cfg: Rc<Config>, prober: Box<dyn Prober>, lookup: Lookup, cache: ProbeCache) -> Registry {
        let mut specs: Vec<Rc<EngineSpec>> = builtins().into_iter().map(Rc::new).collect();
        // Custom engines (17.2): every validated `[engines.<tag>]` table with a `kind`.
        specs.extend(cfg.custom_engines.iter().map(|(tag, c)| Rc::new(custom_spec(tag, c.clone()))));
        Registry { specs, cfg, lookup, prober, cache: RefCell::new(cache), resolved: RefCell::new(HashMap::new()) }
    }

    /// The registry of a run or of doctor: real probes, the process environment, and the probe
    /// cache in `mode` (off without a home folder, 18.1).
    pub fn real(cfg: Rc<Config>, mode: CacheMode) -> Registry {
        let home = crate::config::home_dir();
        let mode = if home.is_some() { mode } else { CacheMode::Off };
        let cache = ProbeCache::open(home.as_deref(), mode);
        Registry::new(cfg, Box::new(RealProber), Lookup::from_env(), cache)
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn specs(&self) -> &[Rc<EngineSpec>] {
        &self.specs
    }

    /// The id that a tag or alias names: `python` → `py`. Tags are case-sensitive (7.3).
    pub fn canonical(&self, tag: &str) -> Option<&Rc<EngineSpec>> {
        self.specs.iter().find(|s| &*s.id == tag || s.aliases.iter().any(|a| &**a == tag))
    }

    /// Resolve an engine by id, once per run.
    pub fn resolve(&self, spec: &Rc<EngineSpec>) -> Resolution {
        if let Some(r) = self.resolved.borrow().get(&spec.id) {
            return r.clone();
        }
        let r = match &spec.session_of {
            _ if spec.custom.is_some() => self.resolve_custom(spec),
            Some(host) => {
                let host_spec = self.specs.iter().find(|s| s.id == *host).cloned().expect("a built-in session engine");
                let js = self.resolve(&host_spec);
                let mut cache = self.cache.borrow_mut();
                let mut r = Resolver { cfg: &self.cfg, lookup: &self.lookup, prober: &*self.prober, cache: &mut cache };
                r.resolve_ts(spec, js.as_deref().map_err(|e| &**e))
            }
            None => {
                let mut cache = self.cache.borrow_mut();
                let mut r = Resolver { cfg: &self.cfg, lookup: &self.lookup, prober: &*self.prober, cache: &mut cache };
                let result = r.resolve(spec);
                cache.save();
                result
            }
        };
        let r: Resolution = r.map(Rc::new).map_err(Rc::new);
        self.resolved.borrow_mut().insert(spec.id.clone(), r.clone());
        r
    }

    /// A custom engine resolves only `command[0]` (17.1, 7.5): a path (already absolute, 18.1)
    /// must be a file; a bare name is looked up on `PATH`. Nothing is probed or started.
    fn resolve_custom(&self, spec: &Rc<EngineSpec>) -> Result<ResolvedEngine, NotFound> {
        let c = spec.custom.as_ref().expect("a custom spec");
        let program = &c.command[0];
        let from = || Some(c.origin.clone());
        let found = if program.contains(['/', '\\']) || std::path::Path::new(program).is_absolute() {
            let p = std::path::PathBuf::from(program);
            p.is_file().then_some(p)
        } else {
            super::resolve::first_program(program, &self.lookup)
        };
        match found {
            Some(path) => {
                // The program alone, as for a built-in run engine: `run_command` adds the rest.
                let argv = vec![path.into_os_string()];
                // Doctor's FROM: `v2.toml`, the variable's name, or the define_engine call site.
                let source = if c.origin.ends_with("v2.toml") {
                    crate::config::ConfigSource::Project(c.origin.clone().into())
                } else {
                    crate::config::ConfigSource::Env(c.origin.clone())
                };
                let origin = Origin::Config(source);
                Ok(ResolvedEngine { spec: spec.clone(), origin, argv, version: None, skipped: Vec::new() })
            }
            None => Err(NotFound {
                status: Status::Missing,
                // 7.4: `'perl' not found on PATH (engine declared in C:\proj\v2.toml)`, or `at
                // main.v2:3` for a define_engine call.
                message: format!(
                    "'{program}' not found{} (engine declared {} {})",
                    if program.contains(['/', '\\']) { "" } else { " on PATH" },
                    if c.origin.ends_with("v2.toml") { "in" } else { "at" },
                    c.origin
                ),
                command: Some(c.command.join(" ")),
                from: from(),
                skipped: Vec::new(),
            }),
        }
    }

    /// A run engine's whole argv, `{file}` still in it (17.3): the resolved program, then a
    /// configured command's other words, else 7.2's fixed args.
    pub fn run_command(&self, e: &ResolvedEngine) -> Vec<std::ffi::OsString> {
        let configured: Option<&[String]> = match &e.spec.custom {
            Some(c) => Some(&c.command),
            None => self.cfg.engines.get(&*e.spec.id).and_then(|c| c.command.as_ref()).map(|c| c.value.as_slice()),
        };
        let rest: Vec<std::ffi::OsString> = match configured {
            Some(words) => words[1..].iter().map(std::ffi::OsString::from).collect(),
            None => e.spec.fixed_args.iter().map(|a| std::ffi::OsString::from(&**a)).collect(),
        };
        e.argv.iter().cloned().chain(rest).collect()
    }

    /// `define_engine` (17.2): a custom engine registered at run time. `Err` is the reason it
    /// is refused, after `define_engine: `.
    pub fn declare(&mut self, tag: &str, cfg: CustomCfg) -> Result<(), String> {
        if let Some(why) = self.taken(tag) {
            return Err(why);
        }
        self.specs.push(Rc::new(custom_spec(tag, cfg)));
        Ok(())
    }

    /// Why `define_engine` can't declare `tag` (17.2): a built-in tag or alias, or a tag already
    /// declared. Checked before the other keys, so `define_engine("py")` says what is wrong.
    pub fn taken(&self, tag: &str) -> Option<String> {
        let existing = self.canonical(tag)?;
        Some(match &existing.custom {
            Some(c) => format!("engine '{tag}' is already declared ({})", c.origin),
            None => format!("'{tag}' is a built-in engine — configure it in [engines.{}] in v2.toml", existing.session_of.as_deref().unwrap_or(&existing.id)),
        })
    }

    /// Doctor: whether a resolved engine's program really starts, with the startup variables set
    /// (19.2; `resolve::start_failure`).
    pub fn start_failure(&self, e: &ResolvedEngine) -> Option<String> {
        super::resolve::start_failure(&*self.prober, &e.spec, &e.argv)
    }

    /// A block's tag or a selector's engine: `EngineUnknown` or `EngineNotFound`, a fresh error
    /// each time so every use gets its own location (7.3, 7.8).
    pub fn resolve_tag(&self, tag: &str) -> Exec<Rc<ResolvedEngine>> {
        let Some(spec) = self.canonical(tag).cloned() else {
            return fail("EngineUnknown", self.unknown_message(tag));
        };
        match self.resolve(&spec) {
            Ok(e) => Ok(e),
            Err(nf) => fail("EngineNotFound", nf.message.clone()),
        }
    }

    /// `unknown engine '@pyhton' (did you mean '@py'?)`, or the list of known engines (7.3). A tag
    /// of a later roadmap phase says so first, so `@rs` is not taken for a typo of `@ts`.
    pub fn unknown_message(&self, tag: &str) -> String {
        let ids: Vec<&str> = self.specs.iter().map(|s| &*s.id).collect();
        // Names of the tools rather than the languages (`@node`, `npm.lodash`, `pip.numpy`).
        let tool = match tag {
            "node" | "nodejs" | "npm" | "deno" | "bun" => Some(("js", "npm packages are imported with js.NAME, as in @import { chunk } from js.lodash")),
            "python3" | "py3" | "pip" | "pypi" => Some(("py", "Python modules are imported with py.NAME, as in @import { mean } from py.statistics")),
            _ => None,
        };
        if let Some((id, how)) = tool {
            return format!("unknown engine '@{tag}' (did you mean '@{id}'? {how})");
        }
        if let Some(language) = later_language(tag) {
            return format!("unknown engine '@{tag}': {language} blocks aren't in this version of V2 yet — known engines: {}", ids.join(", "));
        }
        let names = self.specs.iter().flat_map(|s| std::iter::once(&s.id).chain(&s.aliases)).map(|n| &**n);
        match did_you_mean(tag, names).and_then(|n| self.canonical(n)) {
            Some(spec) => format!("unknown engine '@{tag}' (did you mean '@{}'?)", spec.id),
            // A custom engine's tag (`@r`, DOCS 13): those come in a later version (7.3).
            None => format!("unknown engine '@{tag}' — known engines: {}; declare it in [engines.{tag}] in v2.toml, or with define_engine", ids.join(", ")),
        }
    }

    /// `engine_available(tag)`: false for an unknown tag, else whether resolution succeeds;
    /// it may probe but never starts a session (7.3).
    pub fn engine_available(&self, tag: &str) -> bool {
        self.canonical(tag).cloned().is_some_and(|spec| self.resolve(&spec).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::resolve::ProbeOutcome;
    use std::cell::Cell;
    use std::ffi::OsString;

    /// Answers every probe like a healthy Python 3.12 / Node 24, and counts the calls.
    struct Counting(Rc<Cell<usize>>);

    impl Prober for Counting {
        fn probe(&self, argv: &[OsString], _: &[(OsString, OsString)], _: Option<std::time::Duration>) -> ProbeOutcome {
            self.0.set(self.0.get() + 1);
            let name = argv[0].to_string_lossy().to_lowercase();
            let output = if name.contains("node") { "v24.11.1\n" } else { "Python 3.12.1\n" };
            ProbeOutcome::Exited { code: Some(0), output: output.into() }
        }
    }

    fn registry(path: Option<OsString>) -> (Registry, Rc<Cell<usize>>) {
        let calls = Rc::new(Cell::new(0));
        let lookup = Lookup { path, pathext: None, windows_apps: None, cwd: std::env::temp_dir(), xcrun_shims: Vec::new(), startup: Vec::new() };
        let reg = Registry::new(Rc::new(Config::default()), Box::new(Counting(calls.clone())), lookup, ProbeCache::off());
        (reg, calls)
    }

    #[test]
    fn aliases_canonicalize() {
        let (reg, _) = registry(None);
        for (tag, id) in [("py", "py"), ("python", "py"), ("js", "js"), ("javascript", "js"), ("ts", "ts"), ("typescript", "ts"), ("sh", "bash"), ("powershell", "pwsh"), ("csharp", "cs"), ("lua", "lua")] {
            assert_eq!(reg.canonical(tag).map(|s| &*s.id), Some(id), "{tag}");
        }
        for tag in ["PY", "c", "cpp", "rust", "rs", "cxx", "perl"] {
            assert!(reg.canonical(tag).is_none(), "{tag}");
        }
    }

    #[test]
    fn unknown_tag_messages() {
        let (reg, _) = registry(None);
        assert_eq!(reg.unknown_message("pyhton"), "unknown engine '@pyhton' (did you mean '@py'?)");
        assert_eq!(reg.unknown_message("javscript"), "unknown engine '@javscript' (did you mean '@js'?)");
        assert_eq!(reg.unknown_message("PY"), "unknown engine '@PY' (did you mean '@py'?)");
        assert_eq!(reg.unknown_message("perl"), "unknown engine '@perl' — known engines: py, js, ts, bash, pwsh, cmd, lua, ruby, go, java, zig, cs; declare it in [engines.perl] in v2.toml, or with define_engine");
        let known = "py, js, ts, bash, pwsh, cmd, lua, ruby, go, java, zig, cs";
        assert_eq!(reg.unknown_message("c"), format!("unknown engine '@c': C blocks aren't in this version of V2 yet — known engines: {known}"));
        // A later tag one edit away from a known one is not a typo of it.
        assert_eq!(reg.unknown_message("rs"), format!("unknown engine '@rs': Rust blocks aren't in this version of V2 yet — known engines: {known}"));
        assert_eq!(reg.unknown_message("rubby"), "unknown engine '@rubby' (did you mean '@ruby'?)");
        let Err(crate::runtime::error::Flow::Error(e)) = reg.resolve_tag("nosuch") else { panic!("an error") };
        assert_eq!(&*e.kind, "EngineUnknown");
    }

    #[test]
    fn failures_memoized_unknown_not() {
        // No PATH at all: py is missing, once.
        let (reg, calls) = registry(Some(OsString::new()));
        for _ in 0..3 {
            let Err(crate::runtime::error::Flow::Error(e)) = reg.resolve_tag("python") else { panic!("not found") };
            assert_eq!(&*e.kind, "EngineNotFound");
            assert!(e.message.starts_with("engine 'py' (Python) not found\ntried: python3, python"), "{}", e.message);
        }
        assert_eq!(reg.resolved.borrow().len(), 1);
        assert!(!reg.engine_available("nosuch"));
        assert!(!reg.resolved.borrow().contains_key("nosuch"));
        assert_eq!(calls.get(), 0, "nothing on PATH to probe");
    }

    #[test]
    fn a_failure_found_by_probing_is_memoized() {
        // 7.1: a too-old python on PATH is probed once, however often py is asked for.
        struct Old(Rc<Cell<usize>>);
        impl Prober for Old {
            fn probe(&self, _: &[OsString], _: &[(OsString, OsString)], _: Option<std::time::Duration>) -> ProbeOutcome {
                self.0.set(self.0.get() + 1);
                ProbeOutcome::Exited { code: Some(0), output: "Python 3.9.6\n".into() }
            }
        }
        let d = std::env::temp_dir().join(format!("v2reg-{}-old", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let python = d.join(if cfg!(windows) { "python.exe" } else { "python" });
        std::fs::write(&python, b"").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&python, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let calls = Rc::new(Cell::new(0));
        let lookup = Lookup { path: Some(d.into_os_string()), pathext: None, windows_apps: None, cwd: std::env::temp_dir(), xcrun_shims: Vec::new(), startup: Vec::new() };
        let reg = Registry::new(Rc::new(Config::default()), Box::new(Old(calls.clone())), lookup, ProbeCache::off());
        for _ in 0..3 {
            let Err(crate::runtime::error::Flow::Error(e)) = reg.resolve_tag("py") else { panic!("too old") };
            assert_eq!(&*e.kind, "EngineNotFound");
        }
        assert!(!reg.engine_available("python"));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn engine_available_never_errors() {
        let (reg, _) = registry(Some(OsString::new()));
        for tag in ["py", "js", "ts", "lua", "", "@py", "PY"] {
            assert!(!reg.engine_available(tag), "{tag}");
        }
    }

    #[test]
    fn successes_are_memoized_too() {
        let d = std::env::temp_dir().join(format!("v2reg-{}-memo", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let node = d.join(if cfg!(windows) { "node.exe" } else { "node" });
        std::fs::write(&node, b"").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&node, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let (reg, calls) = registry(Some(d.into_os_string()));
        for tag in ["js", "javascript", "ts", "js"] {
            assert!(reg.engine_available(tag), "{tag}");
        }
        assert!(reg.resolve_tag("js").is_ok());
        assert_eq!(calls.get(), 1, "resolution is memoized per id, successes too (7.1)");
    }
}
