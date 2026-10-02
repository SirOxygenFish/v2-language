//! The V2 test harness (INTERNALS 22.2), run with `cargo test --test run`.
//!
//! Every `tests/**/*.v2` file outside a `fixtures/` folder is a test. It runs as `v2 <file>` from
//! its own folder with a fresh temporary home, stdin from `<name>.in` (or an empty closed pipe),
//! and stdout/stderr captured. Leading `//` header comments set expectations:
//!
//! ```text
//! // exit: 1                  expected exit code (default 0)
//! // stderr: <text>           must appear in stderr; repeatable, matched in order
//! // args: a b                arguments passed after the file name
//! // platform: windows|unix   run only on that platform (otherwise SKIP)
//! // timeout: 60              seconds before the test is killed (default 30)
//! ```
//!
//! Stdout must equal `<name>.out` (CRLF converted to LF). A test with neither a `.out` file nor a
//! `stderr:` line is rejected. `// requires: py js` skips a test when an engine is missing: one
//! generated probe per run asks `engine_available` for every tag named; a skip is listed in the
//! summary and never counts as a pass, and with `V2_TEST_REQUIRE=py,js` it is a failure.
//!
//! The command-line cases (`--version`, `--help`, usage errors) are ordinary Rust tests below.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_v2");
const DEFAULT_TIMEOUT_SECS: u64 = 30;

// ---------------------------------------------------------------------------------------------
// Headers

#[derive(Debug, PartialEq)]
struct Header {
    exit: i32,
    stderr: Vec<String>,
    args: Vec<String>,
    platform: Option<String>,
    timeout: u64,
    requires: Vec<String>,
}

fn parse_header(src: &str) -> Result<Header, String> {
    let mut h = Header {
        exit: 0,
        stderr: Vec::new(),
        args: Vec::new(),
        platform: None,
        timeout: DEFAULT_TIMEOUT_SECS,
        requires: Vec::new(),
    };
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    for line in src.lines() {
        let Some(comment) = line.trim_end_matches('\r').strip_prefix("//") else {
            break;
        };
        let Some((key, value)) = comment.trim_start().split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "exit" => h.exit = value.parse().map_err(|_| format!("bad `exit:` header `{value}`"))?,
            "stderr" => h.stderr.push(value.to_string()),
            "args" => h.args = value.split_whitespace().map(String::from).collect(),
            "platform" => match value {
                "windows" | "unix" => h.platform = Some(value.to_string()),
                _ => return Err(format!("bad `platform:` header `{value}` (windows or unix)")),
            },
            "timeout" => {
                h.timeout = value.parse().map_err(|_| format!("bad `timeout:` header `{value}`"))?
            }
            "requires" => h.requires = value.split_whitespace().map(String::from).collect(),
            _ => {} // an ordinary comment that happens to contain a colon
        }
    }
    Ok(h)
}

// ---------------------------------------------------------------------------------------------
// Lint: tautological assertions such as `check("x", true, true)` (INTERNALS 22.2)

fn has_tautology(src: &str) -> bool {
    let mut rest = src;
    while let Some(pos) = rest.find("check(") {
        let after = &rest[pos + "check(".len()..];
        if let Some(comma) = after.find(',') {
            if comma > 0 {
                let tail = after[comma + 1..].trim_start();
                if let Some(t) = tail.strip_prefix("true") {
                    if let Some(t) = t.trim_start().strip_prefix(',') {
                        if let Some(t) = t.trim_start().strip_prefix("true") {
                            if t.trim_start().starts_with(')') {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        rest = after;
    }
    false
}

// ---------------------------------------------------------------------------------------------
// Running one test

enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// CRLF to LF on raw bytes, so expected output need not be valid UTF-8.
fn normalize_bytes(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    for (i, &c) in b.iter().enumerate() {
        if !(c == b'\r' && b.get(i + 1) == Some(&b'\n')) {
            out.push(c);
        }
    }
    out
}

fn this_platform() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else {
        "unix"
    }
}

/// A fresh, empty home directory per test (INTERNALS 22.2, isolation).
struct TempHome(PathBuf);

impl TempHome {
    fn new() -> TempHome {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "v2-test-home-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp home");
        TempHome(dir)
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `v2` with an isolated environment: fresh HOME/USERPROFILE, rustup/cargo homes kept, and no
/// inherited `V2_ENGINE_*` overrides.
fn v2_command(home: &Path) -> Command {
    let real_home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_default();
    let rustup = std::env::var_os("RUSTUP_HOME").map(PathBuf::from).unwrap_or(real_home.join(".rustup"));
    let cargo = std::env::var_os("CARGO_HOME").map(PathBuf::from).unwrap_or(real_home.join(".cargo"));
    let mut cmd = Command::new(BIN);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().to_ascii_uppercase().starts_with("V2_ENGINE_") {
            cmd.env_remove(&key);
        }
    }
    cmd.env("HOME", home).env("USERPROFILE", home).env("RUSTUP_HOME", rustup).env("CARGO_HOME", cargo);
    cmd
}

/// Wait for `child` with a deadline. Stdin is fed and stdout/stderr (those that are piped) are
/// drained on helper threads, so neither a child that never reads its input nor a chatty child
/// can block the harness; on the deadline the child is killed.
fn wait_with_timeout(mut child: Child, input: Vec<u8>, timeout: Duration) -> Result<Output, String> {
    let start = Instant::now();
    if let Some(mut stdin) = child.stdin.take() {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        }); // the pipe closes when the writer finishes
    }
    fn drain(stream: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut v = Vec::new();
            if let Some(mut s) = stream {
                let _ = s.read_to_end(&mut v);
            }
            v
        })
    }
    let out_t = drain(child.stdout.take());
    let err_t = drain(child.stderr.take());
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(format!("wait failed: {e}")),
        }
    };
    Ok(Output { status, stdout: out_t.join().unwrap_or_default(), stderr: err_t.join().unwrap_or_default() })
}

fn run_test(path: &Path, engines: &Engines) -> Outcome {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return Outcome::Fail(format!("cannot read test: {e}")),
    };
    let header = match parse_header(&src) {
        Ok(h) => h,
        Err(e) => return Outcome::Fail(e),
    };
    if has_tautology(&src) {
        return Outcome::Fail("contains a tautological check(..., true, true)".into());
    }
    if let Some(p) = &header.platform {
        if p != this_platform() {
            return Outcome::Skip(format!("platform {p} only"));
        }
    }
    match skip_reason(&header.requires, engines) {
        Ok(None) => {}
        Ok(Some(why)) => return Outcome::Skip(why),
        Err(why) => return Outcome::Fail(why),
    }
    let dir = path.parent().expect("test has a folder");
    let name = path.file_name().expect("test has a name").to_string_lossy().into_owned();
    let expected_out = match std::fs::read(path.with_extension("out")) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Outcome::Fail(format!("cannot read the .out file: {e}")),
    };
    if expected_out.is_none() && header.stderr.is_empty() {
        return Outcome::Fail("has neither a .out file nor a `stderr:` header".into());
    }
    let stdin_data = std::fs::read(path.with_extension("in")).unwrap_or_default();

    let home = TempHome::new();
    let mut cmd = v2_command(&home.0);
    cmd.arg(&name)
        .args(&header.args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Outcome::Fail(format!("cannot start v2: {e}")),
    };
    let output = match wait_with_timeout(child, stdin_data, Duration::from_secs(header.timeout)) {
        Ok(o) => o,
        Err(e) => return Outcome::Fail(e),
    };

    let stdout = normalize_bytes(&output.stdout);
    let stderr = normalize(&String::from_utf8_lossy(&output.stderr));
    let mut problems = Vec::new();
    let code = output.status.code();
    if code != Some(header.exit) {
        problems.push(format!("exit code {code:?}, expected {}", header.exit));
    }
    if let Some(expected) = expected_out {
        let expected = normalize_bytes(&expected);
        if stdout != expected {
            problems.push(format!(
                "stdout differs\n--- expected\n{}--- got\n{}---",
                String::from_utf8_lossy(&expected),
                String::from_utf8_lossy(&stdout)
            ));
        }
    }
    let mut at = 0;
    for needle in &header.stderr {
        match stderr[at..].find(needle.as_str()) {
            Some(i) => at += i + needle.len(),
            None => problems.push(format!("stderr lacks (in order): {needle}")),
        }
    }
    if problems.is_empty() {
        Outcome::Pass
    } else {
        Outcome::Fail(format!("{}\n--- stderr\n{stderr}---", problems.join("\n")))
    }
}

fn discover(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "fixtures") {
                continue;
            }
            discover(&path, found);
        } else if path.extension().is_some_and(|e| e == "v2") {
            found.push(path);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// `requires:` (INTERNALS 22.2)

/// Which engines this machine has, and which ones `V2_TEST_REQUIRE` insists on.
struct Engines {
    available: Result<HashMap<String, bool>, String>,
    required: Vec<String>,
}

/// One generated probe for every tag named in a `requires:` header (INTERNALS 22.2).
fn engine_availability(tags: &[String]) -> Result<HashMap<String, bool>, String> {
    if tags.is_empty() {
        return Ok(HashMap::new());
    }
    if let Some(bad) = tags.iter().find(|t| t.is_empty() || !t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')) {
        return Err(format!("bad `requires:` tag {bad:?}"));
    }
    let src: String = tags.iter().map(|t| format!("print(engine_available(\"{t}\"))\n")).collect();
    let home = TempHome::new();
    std::fs::write(home.0.join("probe.v2"), src).map_err(|e| e.to_string())?;
    let child = v2_command(&home.0)
        .arg("probe.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start v2: {e}"))?;
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(120))?;
    let stdout = normalize(&String::from_utf8_lossy(&out.stdout));
    let lines: Vec<&str> = stdout.lines().collect();
    if out.status.code() != Some(0) || lines.len() != tags.len() {
        return Err(format!("the engine_available probe failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(tags.iter().cloned().zip(lines.iter().map(|l| *l == "true")).collect())
}

/// `Ok(Some(why))`: skip; `Err(why)`: fail (a required engine is missing, or the probe failed).
fn skip_reason(requires: &[String], engines: &Engines) -> Result<Option<String>, String> {
    if requires.is_empty() {
        return Ok(None);
    }
    let available = engines.available.as_ref().map_err(|e| e.clone())?;
    let missing: Vec<&String> = requires.iter().filter(|t| !available.get(*t).copied().unwrap_or(false)).collect();
    // Any required tag that is missing fails the test, whatever the header's order.
    if let Some(tag) = missing.iter().find(|t| engines.required.contains(t)) {
        return Err(format!("engine {tag} not available, and V2_TEST_REQUIRE requires it"));
    }
    Ok(missing.first().map(|tag| format!("engine {tag} not available")))
}

#[test]
fn requires_skip_rules() {
    let engines = |avail: &[(&str, bool)], required: &[&str]| Engines {
        available: Ok(avail.iter().map(|(t, a)| (t.to_string(), *a)).collect()),
        required: required.iter().map(|t| t.to_string()).collect(),
    };
    let req = |tags: &[&str]| tags.iter().map(|t| t.to_string()).collect::<Vec<_>>();
    let e = engines(&[("py", true), ("js", false)], &[]);
    assert_eq!(skip_reason(&req(&[]), &e), Ok(None));
    assert_eq!(skip_reason(&req(&["py"]), &e), Ok(None));
    assert_eq!(skip_reason(&req(&["py", "js"]), &e), Ok(Some("engine js not available".into())));
    let e = engines(&[("py", true), ("js", false)], &["js"]);
    assert!(skip_reason(&req(&["js"]), &e).is_err(), "a required engine that is missing fails");
    let e = engines(&[("py", false), ("js", false)], &["js"]);
    assert!(skip_reason(&req(&["py", "js"]), &e).is_err(), "also after a missing tag that is not required");
    let broken = Engines { available: Err("probe failed".into()), required: Vec::new() };
    assert_eq!(skip_reason(&req(&["py"]), &broken), Err("probe failed".into()));
    assert_eq!(skip_reason(&req(&[]), &broken), Ok(None), "only tests that need engines care");
}

#[test]
fn v2_files() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut tests = Vec::new();
    discover(&root, &mut tests);
    let mut tags: Vec<String> = Vec::new();
    for path in &tests {
        let header = std::fs::read_to_string(path).ok().and_then(|src| parse_header(&src).ok());
        for tag in header.map(|h| h.requires).unwrap_or_default() {
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
    }
    let engines = Engines {
        available: engine_availability(&tags),
        required: std::env::var("V2_TEST_REQUIRE")
            .unwrap_or_default()
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
    };
    let (mut passed, mut failed, mut skipped) = (0, Vec::new(), Vec::new());
    for path in &tests {
        let shown = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        match run_test(path, &engines) {
            Outcome::Pass => passed += 1,
            Outcome::Fail(why) => failed.push(format!("FAIL {shown}: {why}")),
            Outcome::Skip(why) => skipped.push(format!("SKIP {shown} ({why})")),
        }
    }
    // Written to the real stderr, not via eprintln!, so libtest doesn't hide the summary and the
    // skip list on passing runs (INTERNALS 22.2: a skip must never look like a pass).
    let mut err = std::io::stderr().lock();
    for line in skipped.iter().chain(failed.iter()) {
        let _ = writeln!(err, "{line}");
    }
    let _ = writeln!(err, ".v2 tests: {passed} passed, {} failed, {} skipped", failed.len(), skipped.len());
    drop(err);
    assert!(failed.is_empty(), "{} .v2 test(s) failed", failed.len());
}

// ---------------------------------------------------------------------------------------------
// Command-line cases (INTERNALS 19.1)

fn v2(args: &[&str]) -> Output {
    let home = TempHome::new();
    let child = v2_command(&home.0)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).expect("v2 finished")
}

#[test]
fn version_prints_crate_version_and_protocol() {
    let out = v2(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        normalize(&String::from_utf8_lossy(&out.stdout)),
        format!("v2 {} (VEP/1)\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_lists_commands_and_exit_codes() {
    let out = v2(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    for needle in ["v2 [FLAGS] FILE", "doctor [TAG]", "cache clean", "--engine TAG=PATH", "130"] {
        assert!(text.contains(needle), "help lacks {needle:?}");
    }
}

#[test]
fn usage_errors_exit_2() {
    for (args, needle) in [
        (&[][..], "no script given"),
        (&["--frobnicate", "x.v2"][..], "unknown flag `--frobnicate`"),
        (&["--engine", "py", "x.v2"][..], "v2 doctor TAG"),
        (&["check"][..], "needs a FILE"),
        (&["no_such_script.v2"][..], "cannot read 'no_such_script.v2'"),
    ] {
        let out = v2(args);
        assert_eq!(out.status.code(), Some(2), "args {args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(needle), "args {args:?}: stderr {err:?} lacks {needle:?}");
    }
}

#[test]
fn invalid_utf8_is_a_syntax_error_at_its_line() {
    // LF, CRLF and lone-CR line endings all count lines the same way (INTERNALS 3.1).
    for bytes in [
        &b"print(1)\nprint(\"\xff\")\n"[..],
        &b"print(1)\r\nprint(\"\xff\")\r\n"[..],
        &b"print(1)\rprint(2)\r\xff\r"[..],
    ] {
        let home = TempHome::new();
        std::fs::write(home.0.join("bad.v2"), bytes).unwrap();
        let child = v2_command(&home.0)
            .arg("bad.v2")
            .current_dir(&home.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start v2");
        let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
        assert_eq!(out.status.code(), Some(1));
        let expected_line = if bytes.contains(&b'\n') { 2 } else { 3 };
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            format!("bad.v2:{expected_line}:1: SyntaxError: file is not valid UTF-8\n")
        );
        assert!(out.stdout.is_empty());
    }
}

/// Run generated source in a fresh temp home; returns (exit code, stdout, stderr).
fn run_generated(name: &str, src: &str) -> (Option<i32>, String, String) {
    let home = TempHome::new();
    std::fs::write(home.0.join(name), src).unwrap();
    let child = v2_command(&home.0)
        .arg(name)
        .current_dir(&home.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(120)).unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Write `files` (path, text) under a fresh temp home and run `v2 ARGS` in `cwd` (relative
/// to the home); returns (exit code, stdout, stderr).
fn run_in_project(files: &[(&str, &str)], cwd: &str, args: &[&str]) -> (Option<i32>, String, String) {
    let home = TempHome::new();
    for (path, text) in files {
        let p = home.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let dir = home.0.join(cwd);
    std::fs::create_dir_all(&dir).unwrap();
    let child = v2_command(&home.0)
        .args(args)
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(60)).unwrap();
    (out.status.code(), normalize(&String::from_utf8_lossy(&out.stdout)), normalize(&String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn check_reports_every_file_and_exits_1_on_errors() {
    // INTERNALS 19.3: the entry and every file it imports; diagnostics on stderr in file and
    // source order, import problems with their run-time text, the summary on stdout.
    let files = [
        ("v2.toml", "[engines.py]\ncomand = \"x\"\n"),
        ("app/main.v2", "import \"./lib/util.v2\" as util\nimport \"./lib/missing.v2\" as m\nlet a = 1\nlet a = 2\n"),
        ("app/lib/util.v2", "import \"../main.v2\" as main\nbreak\n"),
    ];
    let (code, stdout, stderr) = run_in_project(&files, ".", &["check", "app/main.v2"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert_eq!(stdout, "check app/main.v2: 2 files, 4 errors, 1 warning\n");
    let order = [
        "app/main.v2:2:1: ImportError: cannot find module './lib/missing.v2' (looked for app/lib/missing.v2)",
        "app/main.v2:4:5: SyntaxError: 'a' is already declared in this block",
        "app/lib/util.v2:1:1: ImportError: circular import: app/main.v2 -> app/lib/util.v2 -> app/main.v2",
        "app/lib/util.v2:2:1: SyntaxError: break outside a loop",
        "unknown key [engines.py] comand (did you mean 'command'?)",
    ];
    let mut at = 0;
    for needle in order {
        let i = stderr[at..].find(needle).unwrap_or_else(|| panic!("{needle:?} not in order in:\n{stderr}"));
        at += i + needle.len();
    }
    // A clean file: exit 0; check runs nothing.
    let clean = [("main.v2", "print(\"must not print\")\nimport \"./util.v2\" as u\n"), ("util.v2", "func f() { }\n")];
    let (code, stdout, stderr) = run_in_project(&clean, ".", &["check", "main.v2"]);
    assert_eq!((code, stdout.as_str(), stderr.as_str()), (Some(0), "check main.v2: 2 files, 0 errors, 0 warnings\n", ""));
}

#[test]
fn run_starts_the_project_entry() {
    // INTERNALS 19.1, 18.1: the root is the nearest folder with v2.toml, found from the cwd.
    let files = [("v2.toml", "[project]\nentry = \"app/main.v2\"\n"), ("app/main.v2", "print(args())\n")];
    let (code, stdout, stderr) = run_in_project(&files, "app/deeper", &["run", "a", "b c"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "[\"a\", \"b c\"]\n"), "{stderr}");
    let (code, _, stderr) = run_in_project(&[("v2.toml", "[project]\nname = \"x\"\n")], ".", &["run"]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("has no [project] entry"), "{stderr}");
    let (code, _, stderr) = run_in_project(&[], ".", &["run"]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("no v2.toml found in ") && stderr.contains(" or its parents"), "{stderr}");
}

#[test]
fn a_script_outside_the_project_uses_its_own_folders_config() {
    // INTERNALS 18.1: the root search starts at the script's folder, made absolute with `..`
    // applied, so `v2 ../other/main.v2` run inside a project never reads that project's v2.toml.
    let files = [
        ("proj/v2.toml", "[engines.py]\nstartup_timeout = \"x\"\n"),
        ("proj/sub/keep.txt", ""),
        ("other/main.v2", "print(\"ran\")\n"),
    ];
    for (cwd, args) in [
        ("proj", &["../other/main.v2"][..]),
        ("proj/sub", &["../../other/main.v2"][..]),
        ("proj", &["check", "../other/main.v2"][..]),
    ] {
        let (code, stdout, stderr) = run_in_project(&files, cwd, args);
        assert_eq!(code, Some(0), "{cwd} {args:?}: {stderr}");
        assert!(stdout == "ran\n" || stdout.starts_with("check "), "{cwd} {args:?}: {stdout}");
    }
}

#[test]
fn a_config_error_exits_2_before_anything_runs() {
    // INTERNALS 18.5 and R48: syntax and type errors name the file; nothing of the script runs.
    for (toml, needle) in [
        ("[engines.py\n", "v2.toml:1:"),
        ("[engines.py]\nstartup_timeout = \"30s\"\n", "config error: [engines.py] startup_timeout: expected a non-negative number, got string \"30s\""),
    ] {
        let files = [("v2.toml", toml), ("main.v2", "print(\"ran\")\n")];
        let (code, stdout, stderr) = run_in_project(&files, ".", &["main.v2"]);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stdout.is_empty(), "nothing runs: {stdout}");
        assert!(stderr.contains(needle) && stderr.contains("config error"), "{stderr}");
    }
    let (code, _, stderr) = run_in_project(&[("main.v2", "print(1)\n")], ".", &["--engine", "ts=x", "main.v2"]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("ts has no executable of its own"), "{stderr}");
}

#[test]
fn a_long_import_chain_is_a_recursion_error_not_a_crash() {
    // An import is a call for the depth limit and the stack guard (INTERNALS 5.8, 5.9); the
    // startup parse and `v2 check` walk the chain without recursing on the host stack.
    let mut files: Vec<(String, String)> = (0..12_000).map(|i| (format!("m{i}.v2"), format!("import \"./m{}.v2\" as n\n", i + 1))).collect();
    files.push(("m12000.v2".into(), "let done = 1\n".into()));
    files.push(("main.v2".into(), "try { import \"./m0.v2\" as m } catch (e) { print(e.type, e.message) }\n".into()));
    let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let (code, stdout, stderr) = run_in_project(&refs, ".", &["main.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "RecursionError maximum call depth (10000) exceeded\n"), "{stderr}");
    let (code, stdout, stderr) = run_in_project(&refs, ".", &["check", "main.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "check main.v2: 12002 files, 0 errors, 0 warnings\n"), "{stderr}");
}

#[test]
fn import_messages_show_paths_per_2_1() {
    let main = "func show(f) { try { f() } catch (e) { print(e.message) } }\n\
                show(lambda() { import \"./lib/../lib/nope.v2\" as a })\n\
                show(lambda() { import \"./dir\" as b })\n";
    let (code, stdout, stderr) = run_in_project(&[("main.v2", main), ("lib/x.v2", ""), ("dir/keep.v2", "")], ".", &["main.v2"]);
    assert_eq!(code, Some(0), "{stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "cannot find module './lib/../lib/nope.v2' (looked for lib/nope.v2)");
    assert!(lines[1].starts_with("cannot import './dir': cannot read 'dir': "), "{}", lines[1]);
    assert!(!lines[1].ends_with('.'), "no final period: {}", lines[1]);
    // A missing file outside the cwd shows its absolute path.
    let (_, stdout, _) = run_in_project(&[("app/main.v2", "try { import \"../../gone.v2\" as g } catch (e) { print(e.message) }\n")], "app", &["main.v2"]);
    let shown = stdout.trim_end().rsplit("(looked for ").next().unwrap().trim_end_matches(')');
    assert!(std::path::Path::new(shown).is_absolute() && !shown.contains(".."), "{stdout}");
}

#[cfg(windows)]
#[test]
fn a_drive_relative_import_is_an_import_error() {
    let src = "try { import \"C:m.v2\" as m } catch (e) { print(e.message) }\n";
    let (code, stdout, stderr) = run_in_project(&[("main.v2", src), ("m.v2", "let x = 1\n")], ".", &["main.v2"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stdout, "cannot find module 'C:m.v2' (a drive-relative path: write it relative to the file, or absolute)\n");
}

#[test]
fn writing_to_a_closed_stdout_is_an_io_error() {
    // INTERNALS 21 (Host) and 5.10: a failed write (closed pipe; Rust ignores SIGPIPE) raises
    // IOError at the print.
    let home = TempHome::new();
    std::fs::write(home.0.join("spam.v2"), "while true { print(\"x\" * 1000) }\n").unwrap();
    let mut child = v2_command(&home.0)
        .arg("spam.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    drop(child.stdout.take()); // close the read end
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("IOError: cannot write to stdout"), "{err}");
    assert!(err.contains("spam.v2:1:14"), "the caret is at the print: {err}");
}

/// Windows: put the child in its own process group, so Ctrl+Break can be sent to it alone. The
/// event travels through a console this process shares with the child; a CI runner may start
/// the tests without one, so allocate one then, keeping this process's own std handles.
fn interruptible(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Console::{AllocConsole, GetStdHandle, SetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
        static CONSOLE: std::sync::Once = std::sync::Once::new();
        // SAFETY: plain Win32 calls; AllocConsole fails harmlessly when a console is attached.
        CONSOLE.call_once(|| unsafe {
            let kinds = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
            let saved = kinds.map(|k| GetStdHandle(k));
            if AllocConsole() != 0 {
                for (k, h) in kinds.into_iter().zip(saved) {
                    SetStdHandle(k, h);
                }
            }
        });
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    let _ = cmd;
}

/// Interrupt `child` the way a terminal does: SIGINT, or Ctrl+Break to its process group.
fn interrupt(child: &Child) {
    #[cfg(unix)]
    // SAFETY: sending a signal to our own child.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }
    #[cfg(windows)]
    {
        // SAFETY: a plain Win32 call with the child's process group id.
        let sent = unsafe { windows_sys::Win32::System::Console::GenerateConsoleCtrlEvent(1, child.id()) };
        assert_ne!(sent, 0, "Ctrl+Break could not be sent: {}", std::io::Error::last_os_error());
    }
}

#[test]
fn an_interrupt_exits_130_without_running_finally() {
    // INTERNALS 6.5: the handler only sets INTERRUPTED; the watchdog ends the run with 130,
    // no catch or finally runs.
    let home = TempHome::new();
    std::fs::write(
        home.0.join("busy.v2"),
        "let n = 0\ntry {\n    while true { n += 1 }\n} catch (e) {\n    print(\"catch ran\")\n} finally {\n    print(\"finally ran\")\n}\n",
    )
    .unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg("busy.v2").current_dir(&home.0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    interruptible(&mut cmd);
    let child = cmd.spawn().expect("start v2");
    std::thread::sleep(Duration::from_millis(500));
    interrupt(&child);
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(10)).expect("v2 ended after the interrupt");
    assert_eq!(out.status.code(), Some(130));
    assert!(out.stdout.is_empty(), "catch and finally must not run");
    assert!(out.stderr.is_empty(), "no diagnostic after an interrupt");
}

#[cfg(unix)]
#[test]
fn an_interrupt_as_the_run_ends_still_exits_130() {
    // INTERNALS 6.5: once INTERRUPTED is set the interpreter runs no catch or finally and prints
    // no diagnostic, and a run that ends before the watchdog's next wake (up to 100 ms on Unix)
    // still exits 130. The watchdog wakes on a 100 ms grid from startup, so the signal goes
    // half a tick after v2 shows its prompt, and end of input follows 10 ms later: the run
    // ends long before the watchdog's next wake.
    let home = TempHome::new();
    let scripts = [
        // reaches catch and finally (exec_try)
        ("late_try.v2", "try {\n    let x = input(\"> \")\n    print(x.trim())\n} catch (e) {\n    print(\"catch ran\")\n} finally {\n    print(\"finally ran\")\n}\n"),
        // ends normally (main's check after the interpreter thread returns)
        ("late_end.v2", "let x = input(\"> \")\n"),
        // ends with an uncaught error (no diagnostic)
        ("late_diag.v2", "print(input(\"> \").trim())\n"),
    ];
    for (name, src) in scripts {
        std::fs::write(home.0.join(name), src).unwrap();
        for _ in 0..5 {
            let mut child = v2_command(&home.0)
                .arg(name)
                .current_dir(&home.0)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("start v2");
            let stdin = child.stdin.take();
            let mut out = child.stdout.take().unwrap();
            let mut prompt = [0u8; 2];
            out.read_exact(&mut prompt).expect("the prompt");
            child.stdout = Some(out);
            std::thread::sleep(Duration::from_millis(50));
            interrupt(&child);
            std::thread::sleep(Duration::from_millis(10));
            drop(stdin); // end of input: input() returns null and the run ends at once
            let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(10)).expect("v2 ended");
            let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert_eq!(out.status.code(), Some(130), "{name}: stdout {stdout:?} stderr {stderr:?}");
            assert!(stdout.is_empty() && stderr.is_empty(), "{name}: stdout {stdout:?} stderr {stderr:?}");
        }
    }
}

/// `v2 ARGS` in `dir` with stdout (or stderr) on a pipe whose reader is already gone; returns
/// the exit code and the other stream.
fn run_with_a_closed_stream(home: &Path, args: &[&str], close_stdout: bool) -> (Option<i32>, String) {
    let (reader, writer) = std::io::pipe().expect("a pipe");
    drop(reader);
    let mut cmd = v2_command(home);
    cmd.args(args).current_dir(home).stdin(Stdio::null());
    if close_stdout {
        cmd.stdout(writer).stderr(Stdio::piped());
    } else {
        cmd.stdout(Stdio::piped()).stderr(writer);
    }
    let child = cmd.spawn().expect("start v2");
    drop(cmd);
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    let other = if close_stdout { out.stderr } else { out.stdout };
    (out.status.code(), String::from_utf8_lossy(&other).into_owned())
}

#[test]
fn a_closed_stdout_or_stderr_never_panics() {
    // INTERNALS 6.5 and 19.1: 101 is a V2 bug and an abort is a crash. A diagnostic or summary
    // that can't be written is dropped; the exit code is the one the run would have had.
    let home = TempHome::new();
    let d = &home.0;
    std::fs::write(d.join("quiet.v2"), "let x = 1\n").unwrap();
    std::fs::write(d.join("bad.v2"), "break\n").unwrap();
    std::fs::write(d.join("throw.v2"), "throw \"boom\"\n").unwrap();
    std::fs::write(d.join("rec.v2"), "func f(n) { return f(n + 1) }\nf(0)\n").unwrap();
    std::fs::write(d.join("small.v2"), "print(\"hello\")\n").unwrap();
    std::fs::write(d.join("exit3.v2"), "print(\"hello\")\nexit(3)\n").unwrap();
    std::fs::create_dir_all(d.join("cfg")).unwrap();
    std::fs::write(d.join("cfg/v2.toml"), "[engines.py\n").unwrap();
    std::fs::write(d.join("cfg/main.v2"), "let x = 1\n").unwrap();
    for (args, code) in [
        (&["check", "quiet.v2"][..], 0),
        (&["check", "bad.v2"][..], 1),
        (&["--version"][..], 0),
        (&["--help"][..], 0),
    ] {
        let (got, stderr) = run_with_a_closed_stream(d, args, true);
        assert_eq!(got, Some(code), "stdout closed, args {args:?}: {stderr}");
        assert!(!stderr.contains("internal error") && !stderr.contains("panicked"), "args {args:?}: {stderr}");
    }
    // Program output that can't be written at the end of the run: reported, and exit 0 becomes
    // 1; another code is kept (6.5).
    let (got, stderr) = run_with_a_closed_stream(d, &["small.v2"], true);
    assert_eq!(got, Some(1), "{stderr}");
    assert!(stderr.starts_with("v2: cannot write to stdout: "), "{stderr}");
    let (got, stderr) = run_with_a_closed_stream(d, &["exit3.v2"], true);
    assert_eq!(got, Some(3), "{stderr}");
    for (args, code) in [
        (&["throw.v2"][..], 1),
        (&["bad.v2"][..], 1),
        (&["rec.v2"][..], 1),
        (&["missing.v2"][..], 2),
        (&["cfg/main.v2"][..], 2),
        (&["check", "bad.v2"][..], 1),
        (&["--bogus"][..], 2),
        (&[][..], 2),
    ] {
        let (got, _) = run_with_a_closed_stream(d, args, false);
        assert_eq!(got, Some(code), "stderr closed, args {args:?}");
    }
}

#[test]
fn check_keeps_file_order_and_warnings_for_files_that_are_not_utf8() {
    // INTERNALS 19.3 and 18.5: a file that isn't UTF-8 reports at the place it was met, and
    // config warnings are printed whenever the summary counts them.
    let home = TempHome::new();
    let d = &home.0;
    std::fs::write(d.join("v2.toml"), "[engines.py]\ncomand = \"x\"\n").unwrap();
    std::fs::write(d.join("main.v2"), "import \"./bad.v2\" as b\nimport \"./good.v2\" as g\nlet x = 1\n").unwrap();
    std::fs::write(d.join("bad.v2"), b"let a = \"\xff\"\n").unwrap();
    std::fs::write(d.join("good.v2"), "let y = 1\nlet y = 2\n").unwrap();
    let check = |file: &str| {
        let child = v2_command(d)
            .args(["check", file])
            .current_dir(d)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start v2");
        let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
        (out.status.code(), normalize(&String::from_utf8_lossy(&out.stdout)), normalize(&String::from_utf8_lossy(&out.stderr)))
    };
    let (code, stdout, stderr) = check("main.v2");
    assert_eq!((code, stdout.as_str()), (Some(1), "check main.v2: 3 files, 2 errors, 1 warning\n"), "{stderr}");
    let bad = stderr.find("bad.v2:1:1: SyntaxError: file is not valid UTF-8").expect(&stderr);
    let good = stderr.find("good.v2:2:5: SyntaxError: 'y' is already declared").expect(&stderr);
    let warning = stderr.find("unknown key [engines.py] comand").expect(&stderr);
    assert!(bad < good && good < warning, "{stderr}");
    let (code, stdout, stderr) = check("bad.v2");
    assert_eq!((code, stdout.as_str()), (Some(1), "check bad.v2: 1 file, 1 error, 1 warning\n"), "{stderr}");
    assert!(stderr.starts_with("bad.v2:1:1: SyntaxError: file is not valid UTF-8\nwarning: "), "{stderr}");
    assert!(stderr.contains("unknown key [engines.py] comand"), "{stderr}");
}

#[test]
fn check_warns_when_there_is_no_home_folder() {
    // INTERNALS 18.1: the user layer is skipped and check warns; a run says nothing.
    let home = TempHome::new();
    std::fs::write(home.0.join("main.v2"), "print(\"ran\")\n").unwrap();
    let run = |args: &[&str]| {
        let child = v2_command(&home.0)
            .env_remove(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .args(args)
            .current_dir(&home.0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start v2");
        let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
        (out.status.code(), normalize(&String::from_utf8_lossy(&out.stdout)), normalize(&String::from_utf8_lossy(&out.stderr)))
    };
    let (code, stdout, stderr) = run(&["check", "main.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "check main.v2: 1 file, 0 errors, 1 warning\n"), "{stderr}");
    assert!(stderr.starts_with("warning: no home folder ("), "{stderr}");
    assert_eq!(run(&["main.v2"]), (Some(0), "ran\n".to_string(), String::new()));
}

#[test]
fn block_head_and_close_errors() {
    // INTERNALS 3.7, 4.5 and the ch. 21 Syntax rows: each case is a SyntaxError (exit 1)
    // with these parts of the diagnostic, in order.
    let fence = "hint: if the block's code has braces V2 can't match, use the fence form";
    let no_args = "SyntaxError: compiled blocks take no args — @export a function and call it with arguments";
    let cases: &[(&str, &[&str])] = &[
        ("@py\n```\nx = 1\n```\n", &["b.v2:1:4: SyntaxError: expected { or a ``` fence after @py"]),
        (
            "@py(a,\n  b)\n{ x }\n",
            &[
                "b.v2:3:1: SyntaxError: the { that opens a block must be on the same line as @py",
                "note: the block starts here (b.v2:1:1)",
                "hint: move { to the end of line 2",
            ],
        ),
        (
            "@py {\n    x = 1 }\n",
            &["b.v2:1:1: SyntaxError: @py block closes at a } that does not start its line", "note: the } that closes it (b.v2:2:11)", fence],
        ),
        (
            "let y = @py { 1 } + 2\n",
            &["b.v2:1:17: SyntaxError: unexpected text after the } that closes the @py block", fence, "(@py { 1 }) + 2"],
        ),
        ("@cpp(x: 1) {\n}\n", &["b.v2:1:5: ", no_args]),
        ("@cxx(x: 1) {\n}\n", &["b.v2:1:5: ", no_args]),
        ("@rust(x: 1) {\n}\n", &["b.v2:1:6: ", no_args]),
        ("@rs(x: 1) {\n}\n", &["b.v2:1:4: ", no_args]),
    ];
    for (src, parts) in cases {
        let (code, stdout, stderr) = run_generated("b.v2", src);
        assert_eq!((code, stdout.as_str()), (Some(1), ""), "{src:?}: {stderr}");
        let mut at = 0;
        for part in *parts {
            let i = stderr[at..].find(part).unwrap_or_else(|| panic!("{src:?}: {part:?} not in order in:\n{stderr}"));
            at += i + part.len();
        }
    }
    // A closing } may be followed by ) ] } , ; or a comment (C2).
    let ok = "func f() {\n    let a = [@py {\n        1\n    }]\n    let b = {k: @py {\n        1\n    }}\n    print(@py {\n        1\n    }, 2); // c\n    let c = @py {\n        1\n    } /* c */\n}\nprint(\"ok\")\n";
    assert_eq!(run_generated("ok.v2", ok), (Some(0), "ok\n".to_string(), String::new()));
}

#[test]
#[cfg_attr(not(debug_assertions), ignore = "the V2_TEST_PANIC trigger exists in debug builds only")]
fn a_host_panic_exits_101() {
    // INTERNALS 6.5 and ch. 21 (Host panic): one line on stderr, exit 101, also when stderr is
    // closed. The trigger exists in debug builds only (22.2).
    let home = TempHome::new();
    std::fs::write(home.0.join("ok.v2"), "print(\"must not print\")\n").unwrap();
    let child = v2_command(&home.0)
        .env("V2_TEST_PANIC", "1")
        .arg("ok.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    let stderr = normalize(&String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(101), "{stderr}");
    assert!(out.stdout.is_empty());
    assert!(stderr.starts_with("v2: internal error: V2_TEST_PANIC is set (") && stderr.lines().count() == 1, "{stderr:?}");
    let (reader, writer) = std::io::pipe().expect("a pipe");
    drop(reader);
    let child = v2_command(&home.0)
        .env("V2_TEST_PANIC", "1")
        .arg("ok.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(writer)
        .spawn()
        .expect("start v2");
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    assert_eq!(out.status.code(), Some(101));
}

#[cfg(unix)]
#[test]
fn every_interrupt_signal_exits_130_and_ignored_ones_stay_ignored() {
    // INTERNALS 6.5: SIGINT, SIGTERM and SIGHUP all end the run with 130, with no catch,
    // finally or diagnostic; a signal ignored at startup (nohup) stays ignored.
    use std::os::unix::process::CommandExt;
    let home = TempHome::new();
    let src = "try {\n    while true { }\n} catch (e) {\n    print(\"catch ran\")\n} finally {\n    print(\"finally ran\")\n}\n";
    std::fs::write(home.0.join("busy.v2"), src).unwrap();
    let start = |ignore_hup: bool| {
        let mut cmd = v2_command(&home.0);
        cmd.arg("busy.v2").current_dir(&home.0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if ignore_hup {
            // SAFETY: only an async-signal-safe call between fork and exec.
            unsafe {
                cmd.pre_exec(|| {
                    libc::signal(libc::SIGHUP, libc::SIG_IGN);
                    Ok(())
                });
            }
        }
        let child = cmd.spawn().expect("start v2");
        std::thread::sleep(Duration::from_millis(300));
        child
    };
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        let child = start(false);
        // SAFETY: sending a signal to our own child.
        unsafe {
            libc::kill(child.id() as libc::pid_t, sig);
        }
        let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(10)).expect("v2 ended");
        assert_eq!(out.status.code(), Some(130), "signal {sig}");
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "signal {sig}: {out:?}");
    }
    let mut child = start(true);
    // SAFETY: sending signals to our own child.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGHUP);
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(child.try_wait().unwrap().is_none(), "an ignored SIGHUP must not end the run");
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(10)).expect("v2 ended");
    assert_eq!(out.status.code(), Some(130));
}

#[test]
fn without_a_watchdog_an_interrupt_still_ends_the_run() {
    // INTERNALS 6.5: if the watchdog thread can't start, no handler is installed, so the OS
    // default ends the process instead of the interrupt being swallowed. A huge RUST_MIN_STACK
    // makes the watchdog's spawn fail (the interpreter thread sets its own stack size).
    let home = TempHome::new();
    std::fs::write(home.0.join("busy.v2"), "while true { }\n").unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.env("RUST_MIN_STACK", (1u64 << 50).to_string())
        .arg("busy.v2")
        .current_dir(&home.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    interruptible(&mut cmd);
    let child = cmd.spawn().expect("start v2");
    std::thread::sleep(Duration::from_millis(500));
    interrupt(&child);
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(10)).expect("the interrupt ended v2");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(out.status.signal(), Some(libc::SIGINT), "{:?}", out.status);
    }
    #[cfg(windows)]
    assert_eq!(out.status.code(), Some(0xC000_013Au32 as i32), "STATUS_CONTROL_C_EXIT: {:?}", out.status);
}

/// `v2 ARGS` in a fresh home with `files` (path, text) written there; (code, stdout, stderr).
fn run_with_files(files: &[(&str, &str)], args: &[&str]) -> (Option<i32>, String, String, TempHome) {
    let home = TempHome::new();
    for (path, text) in files {
        let p = home.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let child = v2_command(&home.0)
        .args(args)
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(120)).unwrap();
    let text = |b: &[u8]| normalize(&String::from_utf8_lossy(b));
    (out.status.code(), text(&out.stdout), text(&out.stderr), home)
}

/// The rows of `v2 doctor`'s table by engine: (status, command, version, from).
fn doctor_rows(stdout: &str) -> HashMap<String, Vec<String>> {
    let mut lines = stdout.lines();
    let header = lines.next().unwrap_or_default();
    assert!(header.split_whitespace().eq(["ENGINE", "STATUS", "COMMAND", "VERSION", "FROM"]), "{stdout}");
    let starts: Vec<usize> = ["STATUS", "COMMAND", "VERSION", "FROM"].iter().map(|h| header.find(h).unwrap()).collect();
    let mut rows = HashMap::new();
    for line in lines.take_while(|l| !l.starts_with("notes:") && !l.starts_with("config files:")) {
        let cell = |i: usize| -> String {
            let from = starts[i];
            let to = starts.get(i + 1).copied().unwrap_or(line.len()).min(line.len());
            line.get(from..to).unwrap_or_default().trim().to_string()
        };
        let id = line[..starts[0]].trim().to_string();
        rows.insert(id, (0..4).map(cell).collect());
    }
    rows
}

#[test]
fn doctor_prints_the_engine_table() {
    // INTERNALS 19.2: one row per registered engine, in order; exit 0 even when some are missing.
    let probe = "print(engine_available(\"py\"), engine_available(\"js\"), engine_available(\"ts\"))\n";
    let (code, stdout, stderr, home) = run_with_files(&[("probe.v2", probe)], &["probe.v2"]);
    assert_eq!(code, Some(0), "{stderr}");
    let available: Vec<bool> = stdout.split_whitespace().map(|w| w == "true").collect();
    let (code, stdout, stderr, _) = run_with_files(&[], &["doctor"]);
    assert_eq!(code, Some(0), "{stderr}");
    let ids: Vec<&str> = stdout.lines().skip(1).take(3).map(|l| l.split_whitespace().next().unwrap_or_default()).collect();
    assert_eq!(ids, ["py", "js", "ts"], "{stdout}");
    let rows = doctor_rows(&stdout);
    for (id, avail) in ["py", "js", "ts"].iter().zip(available) {
        let row = &rows[*id];
        assert!(["ok", "missing", "old", "broken"].contains(&row[0].as_str()), "{stdout}");
        assert_eq!(row[0] == "ok", avail, "{id}: doctor and engine_available agree:\n{stdout}");
        if row[0] == "missing" {
            assert_eq!(&row[1..], ["-", "-", "-"], "{stdout}");
        }
    }
    assert!(stdout.contains("config files: none"), "{stdout}");
    drop(home);
}

#[test]
fn doctor_writes_the_probe_cache() {
    let (code, stdout, stderr, home) = run_with_files(&[], &["doctor"]);
    assert_eq!(code, Some(0), "{stderr}");
    let probed = doctor_rows(&stdout).values().any(|r| r[0] == "ok" || r[0] == "old");
    let cache = home.0.join(".v2").join("cache").join("toolchains.json");
    assert_eq!(cache.is_file(), probed, "{stdout}");
    if probed {
        let text = std::fs::read_to_string(&cache).unwrap();
        assert!(text.starts_with("{\"format\":1,\"entries\":{"), "{text}");
    }
}

#[test]
fn doctor_reports_configured_problems() {
    // A configured command never falls back to auto-detection (7.6): broken, FROM its source.
    let files = [("v2.toml", "[engines.py]\ncommand = \"missing/python.exe\"\n")];
    let (code, stdout, stderr, home) = run_with_files(&files, &["--engine", "js=missing/node.exe", "doctor"]);
    assert_eq!(code, Some(0), "{stderr}");
    let rows = doctor_rows(&stdout);
    assert_eq!((rows["py"][0].as_str(), rows["py"][3].as_str()), ("broken", "v2.toml"), "{stdout}");
    assert!(rows["py"][1].ends_with("python.exe"), "{stdout}");
    assert_eq!((rows["js"][0].as_str(), rows["js"][3].as_str()), ("broken", "--engine"), "{stdout}");
    assert_eq!(rows["ts"][0], "broken", "ts follows js:\n{stdout}");
    let toml = home.0.join("v2.toml");
    assert!(stdout.contains(&format!("py: engine 'py' command '{}' not found (set by {})", home.0.join("missing/python.exe").display(), toml.display())), "{stdout}");
    assert!(stdout.contains(&format!("config files:\n  {}", toml.display())), "{stdout}");
}

#[test]
fn an_uncaught_engine_not_found_exits_3() {
    // R48 (run half) and INTERNALS 6.5: no usable toolchain is exit 3; it is catchable.
    let script = "print(\"before\")\n@py {\n    x = 1\n}\n";
    let (code, stdout, stderr, _) = run_with_files(&[("main.v2", script)], &["--engine", "py=missing/python.exe", "main.v2"]);
    assert_eq!(code, Some(3), "{stderr}");
    assert_eq!(stdout, "before\n");
    assert!(stderr.starts_with("main.v2:2:1: EngineNotFound: engine 'py' command '") && stderr.contains("not found (set by --engine)"), "{stderr}");
    let files = [("v2.toml", "[engines.py]\ncommand = \"missing/python.exe\"\n"), ("main.v2", script)];
    let (code, _, stderr, _) = run_with_files(&files, &["main.v2"]);
    assert_eq!(code, Some(3), "{stderr}");
    let caught = "try {\n    @py {\n        x = 1\n    }\n} catch (e) {\n    print(e.type)\n}\n";
    let (code, stdout, stderr, _) = run_with_files(&[("main.v2", caught)], &["--engine", "py=missing/python.exe", "main.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "EngineNotFound\n"), "{stderr}");
}

#[test]
fn a_module_written_during_the_run_is_parsed_when_imported() {
    // late.v2 doesn't exist at startup, so the startup parse skips it; the import parses it
    // (INTERNALS 6.4 step 1). write_file is relative to the working directory, which
    // run_generated sets to the script's folder.
    // A missing module is not remembered: a later import retries (ch. 21 Host rows).
    let src = "write_file(\"late.v2\", \"let answer = 42\\n\")\n\
               import \"./late.v2\" as late\n\
               print(late.answer)\n\
               write_file(\"broken.v2\", \"let = 1\\n\")\n\
               try { import \"./broken.v2\" as b } catch (e) { print(e.message) }\n\
               try { import \"./later.v2\" as l } catch (e) { print(e.type) }\n\
               write_file(\"later.v2\", \"let n = 5\\n\")\n\
               import \"./later.v2\" as later\n\
               print(later.n)\n";
    let (code, stdout, stderr) = run_generated("main.v2", src);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stdout, "42\ncannot import './broken.v2': broken.v2:1:5: SyntaxError: expected a name after `let`, found `=`\nImportError\n5\n");
}

#[test]
fn deep_recursion_then_a_long_chain_is_a_recursion_error() {
    // The stack guard covers every expression node, so a 9,990-term chain evaluated near the
    // bottom of a deep recursion raises RecursionError instead of overflowing (INTERNALS 5.9).
    let chain = " + 1".repeat(9_990);
    for depth in [0, 500, 1000, 1500] {
        let src = format!(
            "func plain(m) {{ if m == 0 {{ return deep(100) }}; return plain(m - 1) }}\n\
             func deep(n) {{\n    if n == 0 {{ return 0 }}\n    return deep(n - 1){chain}\n}}\n\
             try {{ print(plain({depth})) }} catch (e) {{ print(e.type + \": \" + e.message) }}\n"
        );
        let (code, stdout, stderr) = run_generated("chain.v2", &src);
        assert_eq!(code, Some(0), "depth {depth}: {stderr}");
        let recursion = "RecursionError: maximum call depth (10000) exceeded\n";
        assert!(stdout == recursion || stdout.trim().parse::<i64>().is_ok(), "depth {depth}: {stdout:?}");
    }
}

#[test]
fn generated_input_never_overflows_the_stack() {
    // A million-term chain is a SyntaxError, not a crash (INTERNALS 4.1).
    for (name, src) in [
        ("plus.v2", format!("let x = 1{}\n", " + 1".repeat(1_000_000))),
        ("member.v2", format!("let a = 1\nlet x = a{}\n", ".b".repeat(1_000_000))),
        ("nested.v2", format!("let x = {}1{}\n", "(".repeat(100_000), ")".repeat(100_000))),
        // Groups of 9,999-link chains, each under the next one's chain.
        ("groups.v2", {
            let mut s = "a".to_string();
            for _ in 0..250 {
                s = format!("({s}{})", " + a".repeat(9_999));
            }
            format!("let x = {s}\n")
        }),
    ] {
        let home = TempHome::new();
        std::fs::write(home.0.join(name), src).unwrap();
        let child = v2_command(&home.0)
            .arg(name)
            .current_dir(&home.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start v2");
        let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(60)).unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{name}: {err}");
        let message = if name == "nested.v2" { "SyntaxError: nested too deeply" } else { "SyntaxError: expression too long" };
        assert!(err.contains(message), "{name}: {err}");
    }
}

// ---------------------------------------------------------------------------------------------
// The harness checks itself

#[test]
fn header_parsing() {
    let h = parse_header("// exit: 1\n// stderr: a\n// note: just a comment\n// stderr: b\n// args: x y\nprint(1)\n// exit: 9\n")
        .unwrap();
    assert_eq!(h.exit, 1);
    assert_eq!(h.stderr, vec!["a", "b"]);
    assert_eq!(h.args, vec!["x", "y"]);
    assert_eq!(h.timeout, DEFAULT_TIMEOUT_SECS);
    assert!(parse_header("// platform: beos\n").is_err());
    assert!(parse_header("// exit: one\n").is_err());
    assert_eq!(parse_header("\u{feff}// exit: 3\r\n").unwrap().exit, 3);
}

#[test]
fn tautology_lint() {
    assert!(has_tautology(r#"check("custom lang engines", true, true)"#));
    assert!(has_tautology("check(x,true ,  true )"));
    assert!(!has_tautology(r#"check("a", f(), true)"#));
    assert!(!has_tautology("check(, true, true)"));
}

// ---------------------------------------------------------------------------------------------
// Mutation kills

/// `v2 ARGS` in an existing home, with extra environment.
fn run_in_home(home: &Path, args: &[&str], env: &[(&str, &std::ffi::OsStr)]) -> (Option<i32>, String, String) {
    let mut cmd = v2_command(home);
    cmd.args(args).current_dir(home).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(120)).unwrap();
    let text = |b: &[u8]| normalize(&String::from_utf8_lossy(b));
    (out.status.code(), text(&out.stdout), text(&out.stderr))
}

#[test]
fn doctor_and_no_cache_probe_afresh() {
    let home = TempHome::new();
    std::fs::write(home.0.join("probe.v2"), "print(engine_available(\"py\"), engine_available(\"js\"))\n").unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["probe.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stdout, "true true\n", "this test needs py and js");
    let cache = home.0.join(".v2").join("cache").join("toolchains.json");
    // Every usable entry now claims 99.0.0, a version no real probe prints.
    let tamper = || {
        let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&cache).unwrap()).unwrap();
        for e in v["entries"].as_object_mut().unwrap().values_mut() {
            if e["ok"] == true {
                e["version"] = "99.0.0".into();
            }
        }
        std::fs::write(&cache, v.to_string()).unwrap();
    };
    let tampered = || std::fs::read_to_string(&cache).unwrap().contains("99.0.0");
    tamper();
    assert_eq!(run_in_home(&home.0, &["probe.v2"], &[]).1, "true true\n");
    assert!(tampered(), "a run reads the cache and keeps its hits (7.7)");
    assert_eq!(run_in_home(&home.0, &["--no-cache", "probe.v2"], &[]).1, "true true\n");
    assert!(!tampered(), "--no-cache probes afresh and overwrites (7.7, 19.4)");
    tamper();
    let (code, stdout, _) = run_in_home(&home.0, &["doctor"], &[]);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(!stdout.contains("99.0.0") && !tampered(), "doctor probes afresh (19.2):\n{stdout}");
}

#[test]
fn doctor_columns_are_longest_cell_plus_two() {
    let (code, stdout, stderr, _) = run_with_files(&[], &["doctor"]);
    assert_eq!(code, Some(0), "{stderr}");
    let lines: Vec<&str> = stdout.lines().take_while(|l| !l.starts_with("notes:") && !l.starts_with("config files:")).collect();
    let header = lines[0];
    let starts: Vec<usize> = ["ENGINE", "STATUS", "COMMAND", "VERSION", "FROM"].iter().map(|h| header.find(h).unwrap()).collect();
    for c in 0..4 {
        let longest = lines.iter().map(|l| l.get(starts[c]..starts[c + 1]).unwrap_or_default().trim_end().chars().count()).max().unwrap();
        assert_eq!(starts[c + 1] - starts[c], longest + 2, "column {c}:\n{stdout}");
    }
}

#[test]
fn doctor_without_engines_notes_install_hints() {
    let home = TempHome::new();
    let empty = home.0.join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["doctor"], &[("PATH", empty.as_os_str())]);
    assert_eq!(code, Some(0), "{stderr}");
    let rows = doctor_rows(&stdout);
    for id in ["py", "js", "ts"] {
        assert_eq!(rows[id], ["missing", "-", "-", "-"], "{stdout}");
        assert!(stdout.contains(&format!("\n  {id}: not found; install: ")), "{stdout}");
    }
}
