//! Tag resolution (INTERNALS 7.1, 7.3). Custom engines (7.4, 17.1) join in roadmap Phase 3.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::resolve::{CacheMode, Lookup, NotFound, ProbeCache, Prober, RealProber, ResolvedEngine, Resolver};
use super::spec::{builtins, EngineSpec};
use crate::config::Config;
use crate::runtime::error::{did_you_mean, fail, Exec};

type Resolution = Result<Rc<ResolvedEngine>, Rc<NotFound>>;

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
        let specs = builtins().into_iter().map(Rc::new).collect();
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

    /// `unknown engine '@pyhton' (did you mean '@py'?)`, or the list of known engines (7.3).
    pub fn unknown_message(&self, tag: &str) -> String {
        let names = self.specs.iter().flat_map(|s| std::iter::once(&s.id).chain(&s.aliases)).map(|n| &**n);
        match did_you_mean(tag, names).and_then(|n| self.canonical(n)) {
            Some(spec) => format!("unknown engine '@{tag}' (did you mean '@{}'?)", spec.id),
            None => {
                let ids: Vec<&str> = self.specs.iter().map(|s| &*s.id).collect();
                format!("unknown engine '@{tag}' — known engines: {}", ids.join(", "))
            }
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
        fn probe(&self, argv: &[OsString], _: &[(OsString, OsString)], _: std::time::Duration) -> ProbeOutcome {
            self.0.set(self.0.get() + 1);
            let name = argv[0].to_string_lossy().to_lowercase();
            let output = if name.contains("node") { "v24.11.1\n" } else { "Python 3.12.1\n" };
            ProbeOutcome::Exited { code: Some(0), output: output.into() }
        }
    }

    fn registry(path: Option<OsString>) -> (Registry, Rc<Cell<usize>>) {
        let calls = Rc::new(Cell::new(0));
        let lookup = Lookup { path, pathext: None, windows_apps: None, cwd: std::env::temp_dir(), xcrun_shims: Vec::new() };
        let reg = Registry::new(Rc::new(Config::default()), Box::new(Counting(calls.clone())), lookup, ProbeCache::off());
        (reg, calls)
    }

    #[test]
    fn aliases_canonicalize() {
        let (reg, _) = registry(None);
        for (tag, id) in [("py", "py"), ("python", "py"), ("js", "js"), ("javascript", "js"), ("ts", "ts"), ("typescript", "ts")] {
            assert_eq!(reg.canonical(tag).map(|s| &*s.id), Some(id), "{tag}");
        }
        for tag in ["PY", "c", "cpp", "rust", "rs", "cxx", "lua"] {
            assert!(reg.canonical(tag).is_none(), "{tag}");
        }
    }

    #[test]
    fn unknown_tag_messages() {
        let (reg, _) = registry(None);
        assert_eq!(reg.unknown_message("pyhton"), "unknown engine '@pyhton' (did you mean '@py'?)");
        assert_eq!(reg.unknown_message("javscript"), "unknown engine '@javscript' (did you mean '@js'?)");
        assert_eq!(reg.unknown_message("PY"), "unknown engine '@PY' (did you mean '@py'?)");
        assert_eq!(reg.unknown_message("lua"), "unknown engine '@lua' — known engines: py, js, ts");
        assert_eq!(reg.unknown_message("c"), "unknown engine '@c' — known engines: py, js, ts");
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
        assert!(!reg.engine_available("lua"));
        assert!(!reg.resolved.borrow().contains_key("lua"));
        assert_eq!(calls.get(), 0, "nothing on PATH to probe");
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
