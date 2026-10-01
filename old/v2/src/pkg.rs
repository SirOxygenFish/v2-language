//! V2 package manager: `v2 init/add/remove/install/list/update`.
//!
//! Manifest is `v2.toml`. Dependencies may be a local path, a git URL, or a
//! registry version. Installed packages live under `v2_modules/<name>/` and are
//! resolved by the interpreter's import machinery. No hosted registry is
//! required — path and git sources work fully offline/decentralized; a version
//! string is resolved through an optional git-based registry index.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const MANIFEST: &str = "v2.toml";
pub const LOCKFILE: &str = "v2.lock";
pub const MODULES_DIR: &str = "v2_modules";

/// A dependency source parsed from the manifest.
#[derive(Debug, Clone)]
pub enum Source {
    /// Registry version constraint, e.g. "1.2.0".
    Version(String),
    /// Git repository, optional version/tag.
    Git { url: String, version: Option<String> },
    /// Local filesystem path.
    Path(String),
}

impl Source {
    fn describe(&self) -> String {
        match self {
            Source::Version(v) => format!("version {}", v),
            Source::Git { url, version } => match version {
                Some(v) => format!("git {} @ {}", url, v),
                None => format!("git {}", url),
            },
            Source::Path(p) => format!("path {}", p),
        }
    }
}

// ─────────────────────────── minimal TOML reader ───────────────────────────

/// A parsed TOML value we care about (string, or an inline table of strings).
#[derive(Debug, Clone)]
pub enum TomlVal {
    Str(String),
    Table(BTreeMap<String, String>),
}

/// Parse just enough TOML for our manifest: `[section]` headers, `key = "val"`,
/// and inline tables `key = { a = "x", b = "y" }`. Comments start with `#`.
pub fn parse_toml(src: &str) -> BTreeMap<String, BTreeMap<String, TomlVal>> {
    let mut sections: BTreeMap<String, BTreeMap<String, TomlVal>> = BTreeMap::new();
    let mut current = String::new();
    sections.entry(current.clone()).or_default();
    for raw in src.lines() {
        let line = strip_comment(raw).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current = line.trim_matches(|c| c == '[' || c == ']').trim().to_string();
            sections.entry(current.clone()).or_default();
            continue;
        }
        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim().trim_matches('"').to_string();
            let val = line[eq + 1..].trim();
            let parsed = if val.starts_with('{') {
                TomlVal::Table(parse_inline_table(val))
            } else {
                TomlVal::Str(unquote(val))
            };
            sections.entry(current.clone()).or_default().insert(key, parsed);
        }
    }
    sections
}

fn strip_comment(line: &str) -> String {
    let mut in_str = false;
    let mut out = String::new();
    for c in line.chars() {
        if c == '"' {
            in_str = !in_str;
        }
        if c == '#' && !in_str {
            break;
        }
        out.push(c);
    }
    out
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').trim_matches('\'').to_string()
}

fn parse_inline_table(s: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let inner = s.trim().trim_start_matches('{').trim_end_matches('}');
    for pair in split_top_level(inner, ',') {
        if let Some(eq) = pair.find('=') {
            let k = pair[..eq].trim().trim_matches('"').to_string();
            let v = unquote(&pair[eq + 1..]);
            if !k.is_empty() {
                map.insert(k, v);
            }
        }
    }
    map
}

fn split_top_level(s: &str, delim: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    for c in s.chars() {
        if c == '"' {
            in_str = !in_str;
        }
        if c == delim && !in_str {
            parts.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    parts
}

// ─────────────────────────── manifest access ───────────────────────────

pub fn read_dependencies(section: &str) -> BTreeMap<String, Source> {
    let mut deps = BTreeMap::new();
    let content = match fs::read_to_string(MANIFEST) {
        Ok(c) => c,
        Err(_) => return deps,
    };
    let toml = parse_toml(&content);
    if let Some(dep_section) = toml.get(section) {
        for (name, val) in dep_section {
            let src = match val {
                TomlVal::Str(v) => Source::Version(v.clone()),
                TomlVal::Table(t) => {
                    if let Some(p) = t.get("path") {
                        Source::Path(p.clone())
                    } else if let Some(u) = t.get("url").or_else(|| t.get("git")) {
                        Source::Git { url: u.clone(), version: t.get("version").cloned() }
                    } else if let Some(v) = t.get("version") {
                        Source::Version(v.clone())
                    } else {
                        continue;
                    }
                }
            };
            deps.insert(name.clone(), src);
        }
    }
    deps
}

// ─────────────────────────── commands ───────────────────────────

/// `v2 init [--lib]` — scaffold a new package.
pub fn init(is_lib: bool) -> Result<(), String> {
    if Path::new(MANIFEST).exists() {
        return Err(format!("{} already exists", MANIFEST));
    }
    let name = std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "myapp".into());
    let entry = if is_lib { "src/lib.v2" } else { "src/main.v2" };
    let manifest = format!(
        "[project]\nname    = \"{}\"\nversion = \"0.1.0\"\nentry   = \"{}\"\n\n[dependencies]\n\n[dev-dependencies]\n",
        name, entry
    );
    fs::write(MANIFEST, manifest).map_err(|e| format!("write {}: {}", MANIFEST, e))?;
    fs::create_dir_all("src").map_err(|e| format!("mkdir src: {}", e))?;
    if !Path::new(entry).exists() {
        let starter = if is_lib {
            "// Library entry point.\nfunc hello() {\n    return \"hello from {NAME}\"\n}\n".replace("{NAME}", &name)
        } else {
            "func main() {\n    print(\"Hello from {NAME}!\")\n}\n\nmain()\n".replace("{NAME}", &name)
        };
        fs::write(entry, starter).map_err(|e| format!("write {}: {}", entry, e))?;
    }
    println!("Created {} ({}) with entry {}", MANIFEST, name, entry);
    Ok(())
}

/// `v2 add <name> [--path P | --git URL | @version]` — add a dependency.
pub fn add(spec: &str, path: Option<&str>, git: Option<&str>) -> Result<(), String> {
    if !Path::new(MANIFEST).exists() {
        return Err(format!("no {} found — run `v2 init` first", MANIFEST));
    }
    // `name@version` splits into name + version.
    let (name, version) = match spec.split_once('@') {
        Some((n, v)) => (n.to_string(), Some(v.to_string())),
        None => (spec.to_string(), None),
    };
    let line = if let Some(p) = path {
        format!("{} = {{ path = \"{}\" }}", name, p)
    } else if let Some(u) = git {
        match &version {
            Some(v) => format!("{} = {{ git = \"{}\", version = \"{}\" }}", name, u, v),
            None => format!("{} = {{ git = \"{}\" }}", name, u),
        }
    } else {
        format!("{} = \"{}\"", name, version.clone().unwrap_or_else(|| "*".into()))
    };
    insert_into_dependencies(&name, &line)?;
    println!("Added dependency: {}", line);
    // Install the newly added dependency immediately.
    install(false)
}

/// Insert/replace a dependency line under `[dependencies]` in the manifest.
fn insert_into_dependencies(name: &str, line: &str) -> Result<(), String> {
    let content = fs::read_to_string(MANIFEST).map_err(|e| format!("read {}: {}", MANIFEST, e))?;
    let mut out = String::new();
    let mut in_deps = false;
    let mut inserted = false;
    let name_prefix = format!("{} ", name);
    let name_eq = format!("{}=", name);
    for l in content.lines() {
        let trimmed = l.trim();
        if trimmed.starts_with('[') {
            if in_deps && !inserted {
                out.push_str(line);
                out.push('\n');
                inserted = true;
            }
            in_deps = trimmed == "[dependencies]";
            out.push_str(l);
            out.push('\n');
            continue;
        }
        // Replace an existing entry for the same package.
        if in_deps && (trimmed.starts_with(&name_prefix) || trimmed.starts_with(&name_eq)) {
            continue;
        }
        out.push_str(l);
        out.push('\n');
    }
    if in_deps && !inserted {
        out.push_str(line);
        out.push('\n');
        inserted = true;
    }
    if !inserted {
        out.push_str("\n[dependencies]\n");
        out.push_str(line);
        out.push('\n');
    }
    fs::write(MANIFEST, out).map_err(|e| format!("write {}: {}", MANIFEST, e))?;
    Ok(())
}

/// `v2 remove <name>` — drop a dependency and its installed copy.
pub fn remove(name: &str) -> Result<(), String> {
    let content = fs::read_to_string(MANIFEST).map_err(|e| format!("read {}: {}", MANIFEST, e))?;
    let name_prefix = format!("{} ", name);
    let name_eq = format!("{}=", name);
    let mut in_deps = false;
    let mut out = String::new();
    for l in content.lines() {
        let trimmed = l.trim();
        if trimmed.starts_with('[') {
            in_deps = trimmed == "[dependencies]" || trimmed == "[dev-dependencies]";
        }
        if in_deps && (trimmed.starts_with(&name_prefix) || trimmed.starts_with(&name_eq)) {
            continue;
        }
        out.push_str(l);
        out.push('\n');
    }
    fs::write(MANIFEST, out).map_err(|e| format!("write {}: {}", MANIFEST, e))?;
    let installed = Path::new(MODULES_DIR).join(name);
    if installed.exists() {
        let _ = fs::remove_dir_all(&installed);
    }
    println!("Removed dependency: {}", name);
    Ok(())
}

/// `v2 install` — fetch all dependencies into `v2_modules/` and write the lockfile.
pub fn install(frozen: bool) -> Result<(), String> {
    let deps = read_dependencies("dependencies");
    if deps.is_empty() {
        println!("No dependencies to install.");
        return Ok(());
    }
    fs::create_dir_all(MODULES_DIR).map_err(|e| format!("mkdir {}: {}", MODULES_DIR, e))?;
    let mut lock_lines = Vec::new();
    for (name, src) in &deps {
        let dest = Path::new(MODULES_DIR).join(name);
        println!("Installing {} ({})", name, src.describe());
        let resolved = fetch(name, src, &dest, frozen)?;
        lock_lines.push(format!("{} = {{ source = \"{}\" }}", name, resolved));
    }
    let lock = format!("# v2.lock — generated by `v2 install`\n[dependencies]\n{}\n", lock_lines.join("\n"));
    fs::write(LOCKFILE, lock).map_err(|e| format!("write {}: {}", LOCKFILE, e))?;
    println!("Installed {} package(s).", deps.len());
    Ok(())
}

/// Fetch one dependency into `dest`, returning a resolved-source description.
fn fetch(name: &str, src: &Source, dest: &Path, frozen: bool) -> Result<String, String> {
    match src {
        Source::Path(p) => {
            let from = PathBuf::from(p);
            if !from.exists() {
                return Err(format!("path dependency '{}' not found: {}", name, p));
            }
            if dest.exists() {
                let _ = fs::remove_dir_all(dest);
            }
            copy_dir(&from, dest)?;
            Ok(format!("path+{}", p))
        }
        Source::Git { url, version } => {
            if dest.exists() {
                if frozen {
                    return Ok(format!("git+{}", url));
                }
                let _ = fs::remove_dir_all(dest);
            }
            let status = Command::new("git")
                .args(["clone", "--depth", "1", url, &dest.to_string_lossy()])
                .status()
                .map_err(|e| format!("git not available: {}", e))?;
            if !status.success() {
                return Err(format!("git clone failed for '{}'", name));
            }
            if let Some(v) = version {
                let _ = Command::new("git")
                    .args(["-C", &dest.to_string_lossy(), "checkout", v])
                    .status();
            }
            Ok(format!("git+{}", url))
        }
        Source::Version(v) => {
            // Resolve the package name through the registry index to a concrete
            // source (git URL or local path), then fetch that.
            match resolve_registry(name) {
                Some(src) => fetch(name, &src, dest, frozen),
                None => Err(format!(
                    "'{}' = \"{}\" not found in the registry. Publish it (`v2 publish --git <url>`), \
                     point $V2_REGISTRY at a registry index, or use a direct source \
                     (`v2 add {} --git <url>` / `--path <dir>`).",
                    name, v, name
                )),
            }
        }
    }
}

// ─────────────────────────── registry (git-based index) ───────────────────────────
//
// A registry is just a directory (typically a git repo hosted on GitHub) with an
// `index/<name>.toml` file per package mapping the name to a git URL:
//     git = "https://github.com/user/mathutils"
// It is located via $V2_REGISTRY (a local path or a git URL that gets cloned into
// ~/.v2/registry-cache), defaulting to ~/.v2/registry.

fn home_v2_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".v2")
}

/// Resolve the registry index directory, cloning/pulling it if $V2_REGISTRY is a URL.
fn registry_dir() -> Option<PathBuf> {
    let loc = std::env::var("V2_REGISTRY")
        .unwrap_or_else(|_| home_v2_dir().join("registry").to_string_lossy().to_string());
    if loc.starts_with("http") || loc.starts_with("git@") {
        let cache = home_v2_dir().join("registry-cache");
        if cache.join(".git").exists() {
            let _ = Command::new("git").args(["-C", &cache.to_string_lossy(), "pull", "-q"]).status();
        } else {
            let _ = fs::create_dir_all(cache.parent().unwrap());
            let ok = Command::new("git")
                .args(["clone", "--depth", "1", &loc, &cache.to_string_lossy()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                return None;
            }
        }
        Some(cache)
    } else {
        let p = PathBuf::from(loc);
        if p.exists() { Some(p) } else { None }
    }
}

/// Look up a package name in the registry index, returning a resolved Source
/// (git URL, or a path made absolute relative to the registry root).
fn resolve_registry(name: &str) -> Option<Source> {
    let dir = registry_dir()?;
    let idx = dir.join("index").join(format!("{}.toml", name));
    let content = fs::read_to_string(idx).ok()?;
    let toml = parse_toml(&content);
    // Top-level keys live under the "" section.
    let root = toml.get("")?;
    let version = match root.get("version") {
        Some(TomlVal::Str(v)) => Some(v.clone()),
        _ => None,
    };
    if let Some(TomlVal::Str(url)) = root.get("git").or_else(|| root.get("url")) {
        return Some(Source::Git { url: url.clone(), version });
    }
    if let Some(TomlVal::Str(p)) = root.get("path") {
        // Registry paths are relative to the registry root.
        let abs = dir.join(p);
        return Some(Source::Path(abs.to_string_lossy().to_string()));
    }
    None
}

/// `v2 publish [--git URL]` — register the current package in the local registry
/// index so others can `v2 add <name>`. Writes `index/<name>.toml`.
pub fn publish(git_url: Option<&str>) -> Result<(), String> {
    let content = fs::read_to_string(MANIFEST)
        .map_err(|_| format!("no {} found — run `v2 init` first", MANIFEST))?;
    let toml = parse_toml(&content);
    let project = toml.get("project").ok_or("manifest has no [project] section")?;
    let name = match project.get("name") {
        Some(TomlVal::Str(n)) => n.clone(),
        _ => return Err("manifest [project].name is required to publish".into()),
    };
    let version = match project.get("version") {
        Some(TomlVal::Str(v)) => v.clone(),
        _ => "0.1.0".to_string(),
    };
    let url = git_url
        .map(|s| s.to_string())
        .or_else(|| match project.get("repository") {
            Some(TomlVal::Str(u)) => Some(u.clone()),
            _ => None,
        })
        .ok_or("provide the package's git URL: `v2 publish --git <url>` (or set [project].repository)")?;

    let dir = std::env::var("V2_REGISTRY")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_v2_dir().join("registry"));
    if dir.to_string_lossy().starts_with("http") {
        return Err("cannot publish to a remote registry URL directly; clone it locally and set $V2_REGISTRY to that path".into());
    }
    let index = dir.join("index");
    fs::create_dir_all(&index).map_err(|e| format!("mkdir {}: {}", index.display(), e))?;
    let entry = format!(
        "# registry entry for {}\nname = \"{}\"\ngit = \"{}\"\nversion = \"{}\"\n",
        name, name, url, version
    );
    fs::write(index.join(format!("{}.toml", name)), entry)
        .map_err(|e| format!("write index entry: {}", e))?;
    println!("Published {} {} -> {} (registry: {})", name, version, url, dir.display());
    println!("Others can now install it with: v2 add {}", name);
    Ok(())
}

/// `v2 search <query>` — list registry packages whose name matches the query.
pub fn search(query: &str) -> Result<(), String> {
    let dir = registry_dir().ok_or("no registry found (set $V2_REGISTRY or run `v2 publish`)")?;
    let index = dir.join("index");
    let mut hits = Vec::new();
    if let Ok(entries) = fs::read_dir(&index) {
        for e in entries.flatten() {
            let fname = e.file_name().to_string_lossy().to_string();
            if let Some(name) = fname.strip_suffix(".toml") {
                if query.is_empty() || name.contains(query) {
                    hits.push(name.to_string());
                }
            }
        }
    }
    hits.sort();
    if hits.is_empty() {
        println!("No packages matching '{}' in the registry.", query);
    } else {
        println!("Registry packages matching '{}':", query);
        for h in hits {
            match resolve_registry(&h) {
                Some(src) => println!("  {} — {}", h, src.describe()),
                None => println!("  {}", h),
            }
        }
    }
    Ok(())
}

/// `v2 list` — show declared and installed dependencies.
pub fn list() -> Result<(), String> {
    let deps = read_dependencies("dependencies");
    if deps.is_empty() {
        println!("No dependencies declared.");
        return Ok(());
    }
    println!("Dependencies:");
    for (name, src) in &deps {
        let installed = Path::new(MODULES_DIR).join(name).exists();
        println!("  {} — {} [{}]", name, src.describe(), if installed { "installed" } else { "not installed" });
    }
    Ok(())
}

/// Recursively copy a directory tree (skips a nested `v2_modules` to avoid cycles).
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("mkdir {}: {}", to.display(), e))?;
    for entry in fs::read_dir(from).map_err(|e| format!("read {}: {}", from.display(), e))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if name == MODULES_DIR || name == ".git" {
            continue;
        }
        let dest = to.join(&name);
        if ty.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), &dest).map_err(|e| format!("copy: {}", e))?;
        }
    }
    Ok(())
}

/// Resolve the entry file for an installed package `name`, if any. Reads the
/// package's own `v2.toml` `entry`, else falls back to conventional locations.
pub fn resolve_installed_entry(name: &str) -> Option<PathBuf> {
    let base = Path::new(MODULES_DIR).join(name);
    if !base.exists() {
        return None;
    }
    // Honor the package's own manifest entry.
    if let Ok(content) = fs::read_to_string(base.join(MANIFEST)) {
        let toml = parse_toml(&content);
        if let Some(TomlVal::Str(entry)) = toml.get("project").and_then(|s| s.get("entry")) {
            let p = base.join(entry);
            if p.exists() {
                return Some(p);
            }
        }
    }
    for candidate in ["src/lib.v2", "src/main.v2", "lib.v2", "main.v2", &format!("{}.v2", name)] {
        let p = base.join(candidate);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// The project's entry file from `v2.toml` (`[project].entry`), if declared.
pub fn project_entry() -> Option<String> {
    let content = fs::read_to_string(MANIFEST).ok()?;
    let toml = parse_toml(&content);
    match toml.get("project").and_then(|s| s.get("entry")) {
        Some(TomlVal::Str(e)) => Some(e.clone()),
        _ => None,
    }
}

/// The set of CLI subcommand names handled by the package manager.
pub fn is_subcommand(s: &str) -> bool {
    matches!(
        s,
        "init" | "add" | "remove" | "install" | "update" | "list" | "lock"
            | "publish" | "search"
    )
}

/// Dispatch a package-manager subcommand. `args` is the full process argv.
pub fn dispatch(args: &[String]) -> Result<(), String> {
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let rest: Vec<&str> = args.iter().skip(2).map(|s| s.as_str()).collect();
    match cmd {
        "init" => init(rest.contains(&"--lib")),
        "add" => {
            let name = rest.iter().find(|a| !a.starts_with("--")).copied()
                .ok_or("usage: v2 add <name> [--path P | --git URL]")?;
            let path = flag_value(&rest, "--path");
            let git = flag_value(&rest, "--git");
            add(name, path, git)
        }
        "remove" => {
            let name = rest.iter().find(|a| !a.starts_with("--")).copied()
                .ok_or("usage: v2 remove <name>")?;
            remove(name)
        }
        "install" | "update" => install(rest.contains(&"--frozen")),
        "list" => list(),
        "lock" => install(false),
        "publish" => publish(flag_value(&rest, "--git").or_else(|| flag_value(&rest, "--url"))),
        "search" => {
            let query = rest.iter().find(|a| !a.starts_with("--")).copied().unwrap_or("");
            search(query)
        }
        _ => Err(format!("unknown subcommand: {}", cmd)),
    }
}

fn flag_value<'a>(args: &[&'a str], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| *a == flag).and_then(|i| args.get(i + 1).copied())
}
