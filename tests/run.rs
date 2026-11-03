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
    for needle in ["v2 FILE [ARGS...]", "v2 check FILE", "v2 doctor ", "v2 cache clean", "v2 --version", "130", "check found"] {
        assert!(text.contains(needle), "help lacks {needle:?}");
    }
    for gone in ["FLAGS", "--engine", "--no-cache", "v2 run", "TAG"] {
        assert!(!text.contains(gone), "help still has {gone:?}");
    }
}

#[test]
fn usage_errors_exit_2() {
    for (args, needle) in [
        (&[][..], "no script given"),
        (&["--frobnicate", "x.v2"][..], "unknown flag `--frobnicate`"),
        (&["--engine", "py=x", "x.v2"][..], "unknown flag `--engine`"),
        (&["--no-cache", "x.v2"][..], "unknown flag `--no-cache`"),
        (&["doctor", "py"][..], "unexpected argument `py` after `v2 doctor`"),
        (&["run"][..], "cannot read 'run'"),
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
fn a_url_in_a_string_gets_no_comment_hint() {
    // The `//` hint is for a comment that hid a bracket's close, not for one inside a string.
    let (code, _, stderr, _home) = run_with_files(&[("u.v2", "let u = print(\"http://x.com\"
")], &["u.v2"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("is never closed") && !stderr.contains("starts a comment"), "{stderr}");
}

#[test]
fn a_script_named_like_a_command_runs_by_its_path() {
    // DOCS 15: only the exact first word is a command, and what follows the script is its own.
    let (code, stdout, stderr, _home) = run_with_files(&[("doctor", "print(args())
")], &["./doctor", "check", "--help"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "[\"check\", \"--help\"]
"), "{stderr}");
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
        ("v2.toml", "[engines.py]\ncall_timeout = 5\n"),
        ("app/main.v2", "import \"./lib/util.v2\" as util\nimport \"./lib/missing.v2\" as m\nlet a = 1\nlet a = 2\n"),
        ("app/lib/util.v2", "import \"../main.v2\" as main\nbreak\n"),
    ];
    let (code, stdout, stderr) = run_in_project(&files, ".", &["check", "app/main.v2"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert_eq!(stdout, "check app/main.v2: 2 files, 4 errors\n");
    let order = [
        "app/main.v2:2:1: ImportError: cannot find module './lib/missing.v2' (looked for app/lib/missing.v2)",
        "app/main.v2:4:5: SyntaxError: 'a' is already declared in this block",
        "app/lib/util.v2:1:1: ImportError: circular import: app/main.v2 -> app/lib/util.v2 -> app/main.v2",
        "app/lib/util.v2:2:1: SyntaxError: break outside a loop",
    ];
    let mut at = 0;
    for needle in order {
        let i = stderr[at..].find(needle).unwrap_or_else(|| panic!("{needle:?} not in order in:\n{stderr}"));
        at += i + needle.len();
    }
    // A clean file: exit 0; check runs nothing.
    let clean = [("main.v2", "print(\"must not print\")\nimport \"./util.v2\" as u\n"), ("util.v2", "func f() { }\n")];
    let (code, stdout, stderr) = run_in_project(&clean, ".", &["check", "main.v2"]);
    assert_eq!((code, stdout.as_str(), stderr.as_str()), (Some(0), "check main.v2: 2 files, 0 errors\n", ""));
    // A bad v2.toml is a configuration error for check too (exit 2, nothing checked).
    let files = [("v2.toml", "[engines.py]\ncomand = \"x\"\n"), ("main.v2", "let x = 1\n")];
    let (code, stdout, stderr) = run_in_project(&files, ".", &["check", "main.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(2), ""), "{stderr}");
    assert!(stderr.contains("v2.toml: config error: unknown key [engines.py] comand (did you mean 'command'?)"), "{stderr}");
}

#[test]
fn a_project_in_a_folder_with_a_space_and_non_ascii_runs() {
    // INTERNALS 22.6: a path with a space and a non-ASCII character.
    let files = [
        ("v2 tést/v2.toml", "# project root\n"),
        ("v2 tést/main.v2", "import \"./lib.v2\" as lib\nprint(lib.greet())\n"),
        ("v2 tést/lib.v2", "func greet() { return \"hi\" }\n"),
    ];
    for (cwd, args, out) in [
        ("v2 tést", &["main.v2"][..], "hi\n"),
        ("", &["v2 tést/main.v2"][..], "hi\n"),
        ("", &["check", "v2 tést/main.v2"][..], "check v2 tést/main.v2: 2 files, 0 errors\n"),
    ] {
        let (code, stdout, stderr) = run_in_project(&files, cwd, args);
        assert_eq!((code, stdout.as_str()), (Some(0), out), "{cwd:?} {args:?}: {stderr}");
    }
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
    // INTERNALS 18.5 and R48: syntax and type errors, unknown sections and keys, and keys that
    // don't apply name the file; nothing of the script runs. The v2.toml is found from the
    // script's folder upward (18.1).
    for (toml, needle) in [
        ("[engines.py\n", "v2.toml:1:"),
        ("[engines.py]\nstartup_timeout = \"30s\"\n", "v2.toml: config error: [engines.py] startup_timeout: expected a non-negative number, got string \"30s\""),
        ("[engines.py]\ncomand = \"x\"\n", "v2.toml: config error: unknown key [engines.py] comand (did you mean 'command'?)"),
        ("[project]\nentry = \"app/main.v2\"\n", "v2.toml: config error: unknown section [project]"),
        ("[engines.js]\nflags = [\"-O2\"]\n", "v2.toml: config error: [engines.js] flags only applies to c, cpp and rust"),
        ("[engines.ts]\ncall_timeout = 5\n", "v2.toml: config error: [engines.ts]: ts runs on the js engine; configure [engines.js]"),
    ] {
        let files = [("v2.toml", toml), ("app/main.v2", "print(\"ran\")\n")];
        let (code, stdout, stderr) = run_in_project(&files, ".", &["app/main.v2"]);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stdout.is_empty(), "nothing runs: {stdout}");
        assert!(stderr.contains(needle), "{stderr}");
    }
    // V2_ENGINE_TS names the variable to use instead.
    let (code, stdout, stderr, _) = run_with_files_env(&[("main.v2", "print(1)\n")], &["main.v2"], &[("V2_ENGINE_TS", "x")]);
    assert_eq!((code, stdout.as_str()), (Some(2), ""), "{stderr}");
    assert_eq!(stderr, "config error: V2_ENGINE_TS: ts runs on the js engine; set V2_ENGINE_JS\n");
    // A variable for a tag nobody declares is ignored.
    let (code, stdout, stderr, _) = run_with_files_env(&[("main.v2", "print(1)\n")], &["main.v2"], &[("V2_ENGINE_LUA", "x")]);
    assert_eq!((code, stdout.as_str(), stderr.as_str()), (Some(0), "1\n", ""));
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
    assert_eq!((code, stdout.as_str()), (Some(0), "check main.v2: 12002 files, 0 errors\n"), "{stderr}");
}

#[test]
fn import_messages_show_paths_per_2_1() {
    let main = "func show(f) { try { f() } catch (e) { print(e.message) } }\n\
                show(func() { import \"./lib/../lib/nope.v2\" as a })\n\
                show(func() { import \"./dir\" as b })\n";
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
fn exec_runs_a_program_without_a_shell() {
    // DOCS 3.11, INTERNALS 5.10: the arguments arrive exactly as given; the code, stdout and
    // stderr come back; `input` is piped and then closed; a non-zero code is not an error. The
    // program is v2 itself, so the test is the same on every OS.
    let child = "print(json_stringify(args()))\nprint(\"to err\", stderr: true)\nlet line = input()\nwhile line != null {\n    print(\"got \" + line)\n    line = input()\n}\nexit(len(args()))\n";
    let main = r#"let v2 = args()[0]
let r = exec([v2, "child.v2", "a b", "", "c\"d", "%PATH%", "$HOME"])
print(r.code, json_stringify(r.stdout), json_stringify(r.stderr), r.keys())
r = exec([v2, "child.v2"], input: "one\r\ntwo")
print(r.code, json_stringify(r.stdout))
print(exec([v2, "child.v2"], input: b"three\n").stdout.trim())
print(exec([v2, "child.v2"], input: "").stdout.trim())
func show(f) {
    try {
        print(f())
    } catch (e) {
        print(e.type + ": " + e.message.split("': ")[0])
    }
}
show(func() => exec([]))
show(func() => exec(v2))
show(func() => exec([v2, 1]))
show(func() => exec([v2], input: 5))
try { exec(["v2_no_such_program_here"]) } catch (e) { print(e.type + ": " + e.message) }
show(func() => exec(["./no_such_dir/prog"]))
// A name with a separator is started as a path, so the reason is the OS's (7.5).
try { exec(["./no_such_dir/prog"]) } catch (e) { print(e.message.ends_with("not found on PATH"), len(e.message) > 45) }
"#;
    let (code, stdout, stderr, _home) = run_with_files(&[("child.v2", child), ("main.v2", main)], &["main.v2", BIN]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        stdout,
        concat!(
            r#"5 "[\"a b\",\"\",\"c\\\"d\",\"%PATH%\",\"$HOME\"]\n" "to err\n" ["code", "stdout", "stderr"]"#,
            "\n",
            r#"0 "[]\ngot one\ngot two\n""#,
            "\n",
            "[]\ngot three\n",
            "[]\n",
            "ValueError: exec() argument 'argv' must not be empty\n",
            "TypeError: exec() argument 'argv' must be a list, got str\n",
            "TypeError: exec() argument 'argv' must hold only str, got int at [1]\n",
            "TypeError: exec() argument 'input' must be a str, bytes or null, got int\n",
            "IOError: exec: cannot start 'v2_no_such_program_here': not found on PATH\n",
            "IOError: exec: cannot start './no_such_dir/prog\n",
            "false true\n",
        )
    );
}

#[test]
fn file_functions_and_exec_use_the_working_directory() {
    // DOCS 3.11, INTERNALS 5.10: relative paths in the file functions and exec's argv[0] are
    // relative to the cwd, not to the script; the program runs in V2's cwd and environment, and
    // with no `input` its stdin is empty even when V2's own stdin has data, which stays V2's.
    let home = TempHome::new();
    let main = r#"print(exists("data.txt"), read_file("data.txt").trim())
print("data.txt" in list_dir("."), "main.v2" in list_dir("."))
make_dir("made/deep")
write_file("made/deep/f.txt", "a")
write_file("made/deep/f.txt", "b", append: true)
print(read_file("made/deep/f.txt"), exists(script_dir() + "/made"))
remove_file("made/deep/f.txt")
print(list_dir("made/deep"))
let r = exec(["./prog", "child.v2"])
print(r.code, r.stdout.trim())
print(input())
"#;
    let files = [
        ("proj/sub/main.v2", main),
        ("proj/sub/data.txt", "script copy\n"),
        ("work/data.txt", "cwd copy\n"),
        ("work/child.v2", "print(read_file(\"data.txt\").trim(), getenv(\"V2_TEST_MARK\"), input())\n"),
    ];
    for (path, text) in files {
        let p = home.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let work = home.0.join("work");
    std::fs::copy(BIN, work.join(if cfg!(windows) { "prog.exe" } else { "prog" })).unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg(home.0.join("proj/sub/main.v2")).current_dir(&work).env("V2_TEST_MARK", "mark");
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), b"secret\n".to_vec(), Duration::from_secs(60)).unwrap();
    let (stdout, stderr) = (normalize(&String::from_utf8_lossy(&out.stdout)), String::from_utf8_lossy(&out.stderr).into_owned());
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "true cwd copy\ntrue false\nab false\n[]\n0 cwd copy mark null\nsecret\n");
}

#[cfg(windows)]
#[test]
fn exec_refuses_paths_cmd_would_rewrite() {
    // INTERNALS 7.5: `C:prog` is relative to drive C's own current folder, so it is never
    // found, even in the cwd; a batch file whose path holds `%` would be expanded by cmd.exe.
    let home = TempHome::new();
    std::fs::copy(BIN, home.0.join("prog.exe")).unwrap();
    std::fs::create_dir_all(home.0.join("p%OS%q")).unwrap();
    std::fs::write(home.0.join("p%OS%q").join("x.cmd"), "@echo ran\r\n").unwrap();
    let drive = home.0.to_string_lossy()[..2].to_string();
    let main = format!(
        "func show(f) {{\n    try {{\n        print(f())\n    }} catch (e) {{\n        print(e.type + \": \" + e.message)\n    }}\n}}\nshow(func() => exec([\"{drive}prog\", \"--version\"]))\nshow(func() => exec([r\".\\p%OS%q\\x.cmd\"]))\n"
    );
    std::fs::write(home.0.join("main.v2"), main).unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg("main.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
    let stdout = normalize(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        stdout,
        format!(
            "IOError: exec: cannot start '{drive}prog': a drive-relative path: write it relative to the working directory, or absolute\n\
             IOError: exec: cannot start '.\\p%OS%q\\x.cmd': a batch file whose path contains '%' can't be started safely\n"
        )
    );
}

#[cfg(target_os = "linux")]
#[test]
fn list_dir_refuses_names_that_are_not_utf8_and_exists_follows_symlinks() {
    // INTERNALS 5.10: a name that is not UTF-8 is an IOError; `exists` follows symlinks.
    use std::os::unix::ffi::OsStrExt;
    let home = TempHome::new();
    let d = home.0.join("names");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(std::ffi::OsStr::from_bytes(b"bad\xff")), "").unwrap();
    std::fs::write(d.join("data.txt"), "").unwrap();
    std::os::unix::fs::symlink("data.txt", d.join("link")).unwrap();
    std::os::unix::fs::symlink("missing", d.join("dangling")).unwrap();
    let main = "try { list_dir(\"names\") } catch (e) { print(e.type + \": \" + e.message) }\nprint(exists(\"names/link\"), exists(\"names/dangling\"), exists(\"names/data.txt\"))\n";
    std::fs::write(home.0.join("main.v2"), main).unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg("main.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "IOError: cannot list 'names': a name is not valid UTF-8\ntrue false true\n");
}

#[test]
fn print_to_stderr_flushes_stdout_first() {
    // INTERNALS 5.10: `print(stderr: true)` flushes `Out` and then writes stderr, so on one pipe
    // the lines keep program order; a failed write to stderr is an IOError at the print (ch. 21
    // Host).
    let home = TempHome::new();
    std::fs::write(home.0.join("order.v2"), "print(\"a\")\nprint(\"b\", stderr: true)\nprint(\"c\")\nprint(\"d\", \"e\", sep: \"-\", end: \"!\\n\", stderr: true)\n").unwrap();
    let (mut reader, writer) = std::io::pipe().expect("a pipe");
    let mut cmd = v2_command(&home.0);
    cmd.arg("order.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(writer.try_clone().unwrap()).stderr(writer);
    let child = cmd.spawn().expect("start v2");
    drop(cmd);
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text, "a\nb\nc\nd-e!\n");
    std::fs::write(home.0.join("closed.v2"), "try {\n    print(\"x\", stderr: true)\n} catch (e) {\n    print(e.type, e.message.split(\":\")[0], e.line)\n}\n").unwrap();
    let (code, stdout) = run_with_a_closed_stream(&home.0, &["closed.v2"], false);
    assert_eq!((code, stdout.as_str()), (Some(0), "IOError cannot write to stderr 2\n"));
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

#[test]
fn sleep_flushes_what_was_printed() {
    // Through a pipe `print` is buffered, but a wait shows what came before it (INTERNALS 5.10).
    let home = TempHome::new();
    std::fs::write(home.0.join("wait.v2"), "print(\"first\")\nsleep(20)\n").unwrap();
    let mut child = v2_command(&home.0)
        .arg("wait.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start v2");
    let start = Instant::now();
    let mut line = String::new();
    let got = std::io::BufRead::read_line(&mut std::io::BufReader::new(child.stdout.take().unwrap()), &mut line);
    let took = start.elapsed();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(got.is_ok() && line == "first\n", "{line:?}");
    assert!(took < Duration::from_secs(10), "{took:?}");
}

#[cfg(unix)]
#[test]
fn no_interpreter_thread_is_not_an_internal_error() {
    // A process limit (a container's pids limit) is the system's, not a V2 bug (INTERNALS 21).
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return; // root isn't held to RLIMIT_NPROC
    }
    let home = TempHome::new();
    std::fs::write(home.0.join("hi.v2"), "print(\"hi\")\n").unwrap();
    let v2 = v2_command(&home.0).get_program().to_os_string();
    let out = Command::new("bash").args(["-c", "ulimit -u 1; exec \"$0\" hi.v2"]).arg(&v2).current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("v2: cannot start the interpreter thread:") && !err.contains("internal error"), "{err}");
}

#[cfg(unix)]
#[test]
fn a_strict_umask_does_not_stop_a_worker() {
    // The run dir's insides have their modes set, not left to the umask (INTERNALS 20.3).
    if !needs("py", "a_strict_umask_does_not_stop_a_worker") {
        return;
    }
    let home = TempHome::new();
    std::fs::write(home.0.join("mask.v2"), "print(@py { 1 + 1 })\n").unwrap();
    for umask in ["0222", "0277", "0777"] {
        let v2 = v2_command(&home.0).get_program().to_os_string();
        let script = format!("umask {umask}; exec \"$0\" mask.v2");
        let out = Command::new("sh").args(["-c", &script]).arg(&v2).current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "2\n", "umask {umask}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

#[test]
fn a_run_that_used_fetch_ends_quietly() {
    // A libuv assertion from Node's teardown would land on stderr (INTERNALS 9.10).
    if !needs("js", "a_run_that_used_fetch_ends_quietly") {
        return;
    }
    let src = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/engines/js/fetch_exit.v2")).unwrap();
    let (code, stdout, stderr, _home) = run_with_files(&[("fetch.v2", &src)], &["fetch.v2"]);
    assert_eq!((code, stdout.as_str(), stderr.as_str()), (Some(0), "done\n", ""));
}

#[test]
fn a_js_error_with_no_call_to_attach_to_keeps_only_user_frames() {
    // INTERNALS 1.1, 12.6 and 14.4: printed with the frames an error reaching V2 keeps, by line,
    // and its causes; one from V2 says its type and where V2 raised it.
    if !needs("js", "a_js_error_with_no_call_to_attach_to_keeps_only_user_frames") {
        return;
    }
    let src = "\
func boom() {
    throw error(\"ValueError\", \"timer boom\")
}
@js J {
    @import { boom } from v2
    async function wait_on_timer() {
        return await new Promise(res => setTimeout(() => res(boom()), 5));
    }
    async function with_cause() {
        setTimeout(() => { throw new Error(\"outer\", { cause: new TypeError(\"inner\") }) }, 1);
        const g = new AggregateError([new RangeError(\"first\"), new SyntaxError(\"second\")], \"group\");
        g.errors.push(g);
        setTimeout(() => { throw g }, 2);
        const s = new Error(\"self\");
        s.cause = s;
        setTimeout(() => { throw s }, 3);
        const notGroup = new Error(\"not a group\");
        notGroup.errors = [\"member\"];
        setTimeout(() => { throw notGroup }, 4);
        const y = new Error(\"y\");
        const x = new Error(\"x\", { cause: y });
        setTimeout(() => { throw new AggregateError([x], \"agg\", { cause: x }) }, 5);
        class Odd extends Error {}
        Object.defineProperty(Odd.prototype, \"constructor\", { get() { throw new Error(\"no\") } });
        setTimeout(() => { throw new Odd(\"odd\", { cause: new Error(\"its cause\") }) }, 6);
        return await new Promise(res => setTimeout(() => res(1), 50));
    }
    @export { wait_on_timer, with_cause }
}
@import { wait_on_timer, with_cause } from J
try {
    wait_on_timer()
} catch (e) {
    print(\"caught\", e.type)
}
print(with_cause())
";
    let (code, stdout, stderr, _home) = run_with_files(&[("timer.v2", src)], &["timer.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "caught Error\n1\n"));
    // A block's own AggregateError (another realm) shows its members, and itself only once.
    assert!(stderr.contains("\n  RangeError: first\n") && stderr.contains("\n  SyntaxError: second\n"), "{stderr}");
    assert!(stderr.contains("\n  AggregateError: group (shown above)\n") && stderr.len() < 4000, "{stderr}");
    assert_eq!(stderr.matches("caused by: Error: self (shown above)").count(), 1, "{stderr}");
    assert!(stderr.contains("Error: not a group\n") && !stderr.contains("member"), "{stderr}");
    assert!(stderr.contains("caused by: Error: x (shown above)\ncaused by: Error: y\n"), "{stderr}");
    assert!(stderr.contains("caused by: Error: its cause\n"), "{stderr}");
    assert!(stderr.starts_with("V2Error: ValueError: timer boom\n    at boom (timer.v2:2)\n"), "{stderr}");
    // By line: a frame's column is the body's, not the file's.
    assert!(stderr.contains("timer.v2:7)\n"), "{stderr}");
    assert!(stderr.contains("Error: outer\n") && stderr.contains("caused by: TypeError: inner\n"), "{stderr}");
    assert!(!stderr.contains("js.js") && !stderr.contains("node:"), "{stderr}");
}

#[test]
fn a_python_thread_error_keeps_only_user_frames() {
    // INTERNALS 1.1, 12.6 and 14.3: the refusal a thread gets is printed without the adapter's
    // frames and without carets (Python would put them under the dedented line); a V2 error that
    // a `__del__` lets out says its V2 type.
    if !needs("py", "a_python_thread_error_keeps_only_user_frames") {
        return;
    }
    let src = "\
func boom() {
    return 1
}
func bad() {
    throw error(\"ValueError\", \"from v2\")
}
@py P {
    import threading
    @import { boom, bad } from v2
    class D:
        def __del__(self):
            bad()
    def go():
        t = threading.Thread(target=lambda: boom())
        t.start()
        t.join()
        D()
        return 2
    @export { go }
}
@import { go } from P
print(go())
";
    let (code, stdout, stderr, _home) = run_with_files(&[("thread.v2", src)], &["thread.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "2\n"));
    assert!(stderr.contains("line 14, in <lambda>"), "{stderr}");
    assert!(stderr.contains("V2Error: V2 function 'boom' called from another thread"), "{stderr}");
    assert!(stderr.contains("V2Error: ValueError: from v2"), "{stderr}");
    assert!(!stderr.contains("py.py") && !stderr.contains("^^"), "{stderr}");
}

#[test]
fn a_program_that_catches_a_closed_stdout_ends_well() {
    // INTERNALS 6.5: what a failed write couldn't write goes with its IOError, so a program that
    // catches it has handled that output: exit 0, nothing said at the end.
    let home = TempHome::new();
    let src = "try {\n    while true {\n        print(\"x\")\n    }\n} catch (e) {\n    print(\"caught\", e.type, stderr: true)\n}\n";
    std::fs::write(home.0.join("caught.v2"), src).unwrap();
    let mut child = v2_command(&home.0)
        .arg("caught.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    drop(child.stdout.take()); // close the read end
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!((out.status.code(), normalize(&err).as_str()), (Some(0), "caught IOError\n"));
}

#[test]
fn a_preload_that_logs_leaves_console_log_on_stdout() {
    // INTERNALS 14.4: Node's console binds at its first use, which a NODE_OPTIONS preload may
    // make; blocks get a console on the adapter's streams.
    if !needs("js", "a_preload_that_logs_leaves_console_log_on_stdout") {
        return;
    }
    let files = [
        ("preload.js", "const { log } = console\nconsole.log(\"preload noise\")\nglobalThis.preloadLog = log\n"),
        ("pre.v2", "print(\"v2 before\")\n@js {\n    console.log(\"from js\")\n    require(\"console\").log(\"required\")\n    preloadLog(\"kept\")\n}\nprint(\"v2 after\")\n"),
    ];
    let (code, stdout, stderr, _home) = run_with_files_env(&files, &["pre.v2"], &[("NODE_OPTIONS", "--require=./preload.js")]);
    assert_eq!((code, stdout.as_str()), (Some(0), "v2 before\nfrom js\nrequired\nkept\nv2 after\n"), "{stderr}");
}

#[test]
fn a_blocks_console_is_nodes_with_the_users_frames() {
    // INTERNALS 14.4: the console blocks get keeps Node's members, and console.trace shows the
    // block's frames, not the adapter's (12.6).
    if !needs("js", "a_blocks_console_is_nodes_with_the_users_frames") {
        return;
    }
    let src = "\
@js {
    console.log(typeof console.Console, typeof console.profile, typeof console.timeStamp)
    console.trace(\"here\", 1)
    console.group()
    console.trace(\"grouped\")
    console.groupEnd()
}
";
    let (code, stdout, stderr, _home) = run_with_files(&[("trace.v2", src)], &["trace.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "function function function\n"));
    assert!(stderr.starts_with("Trace: here 1\n    at ") && stderr.contains("trace.v2:3"), "{stderr}");
    // Through console.error, so inside a group it is indented like everything else.
    assert!(stderr.contains("\n  Trace: grouped\n      at "), "{stderr}");
    assert!(!stderr.contains("js.js") && !stderr.contains("node:"), "{stderr}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_child_that_left_its_process_group_ends_with_the_run() {
    // INTERNALS 8.3: kill_tree also ends what is left in the worker's session; only `setsid`
    // detaches (DOCS 16).
    if !needs("py", "a_child_that_left_its_process_group_ends_with_the_run") {
        return;
    }
    let src = "@py {\n    import subprocess, os\n    p = subprocess.Popen([\"sleep\", \"30\"], preexec_fn=os.setpgrp, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)\n    open(\"child.pid\", \"w\").write(str(p.pid))\n}\n";
    let start = Instant::now();
    let (code, _stdout, stderr, home) = run_with_files(&[("pgrp.v2", src)], &["pgrp.v2"]);
    assert!(start.elapsed() < Duration::from_secs(10), "the run waited for its child");
    assert_eq!(code, Some(0), "{stderr}");
    let pid: u32 = std::fs::read_to_string(home.0.join("child.pid")).unwrap().trim().parse().unwrap();
    let gone = Instant::now() + Duration::from_secs(2);
    while running(pid) {
        assert!(Instant::now() < gone, "the child {pid} outlived the run");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_blocks_failed_output_goes_with_its_io_error() {
    // INTERNALS 6.5, 9.9: what a block wrote after its first failed write goes too, so a caught
    // IOError ends well and an uncaught one is the only message.
    if !needs("py", "a_blocks_failed_output_goes_with_its_io_error") {
        return;
    }
    let block = "@py P {\n    def blk():\n        for i in range(1000):\n            print(\"p\" * 99)\n    @export { blk }\n}\n@import { blk } from P\n";
    for (end, want_code) in [("try {\n    blk()\n} catch (e) {\n    print(\"caught\", e.type, stderr: true)\n}\n", 0), ("blk()\n", 1)] {
        let home = TempHome::new();
        std::fs::write(home.0.join("lines.v2"), format!("{block}{end}")).unwrap();
        let mut child = v2_command(&home.0)
            .arg("lines.v2")
            .current_dir(&home.0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start v2");
        drop(child.stdout.take()); // close the read end
        let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(want_code), "{err}");
        // Caught: nothing said; uncaught: the IOError alone.
        assert_eq!(err.matches("cannot write to stdout").count(), want_code as usize, "{err}");
    }
}

#[test]
fn a_loss_that_a_blocks_error_hid_is_reported_at_the_end() {
    // INTERNALS 6.5, 9.9: the block's own error wins over its failed write, so the program is
    // never told; the end of the run says it.
    if !needs("py", "a_loss_that_a_blocks_error_hid_is_reported_at_the_end") {
        return;
    }
    let home = TempHome::new();
    let src = "let big = \"y\" * 100000\n@py(big) {\n    import sys\n    sys.stdout.write(big)\n    raise ValueError(\"block failed\")\n}\n";
    std::fs::write(home.0.join("hidden.v2"), src).unwrap();
    let mut child = v2_command(&home.0)
        .arg("hidden.v2")
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
    assert!(err.contains("v2: cannot write to stdout") && err.contains("ValueError: block failed"), "{err}");
}

#[test]
fn a_closed_stdout_is_reported_once() {
    // INTERNALS 6.5: the IOError that a write raised says it; the end of the run doesn't again.
    let home = TempHome::new();
    std::fs::write(home.0.join("flood.v2"), "while true {\n    print(\"x\")\n}\n").unwrap();
    let mut child = v2_command(&home.0)
        .arg("flood.v2")
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
    assert_eq!(err.matches("cannot write to stdout").count(), 1, "{err}");
}

#[cfg(unix)]
#[test]
fn js_blocks_cannot_reach_the_protocol_through_dev_stdout() {
    // INTERNALS 9.9: on Unix the js worker's protocol is on pipes of its own, so /dev/stdout is
    // the host's stderr and /dev/stdin the null device, as for Python.
    if !needs("js", "js_blocks_cannot_reach_the_protocol_through_dev_stdout") {
        return;
    }
    let src = "\
print(\"start\")
@js {
    require(\"fs\").writeFileSync(\"/dev/stdout\", \"hello\\n\")
}
print(@js { require(\"fs\").readFileSync(\"/dev/stdin\", \"utf8\").length })
print(\"end\")
";
    let (code, stdout, stderr, _home) = run_with_files(&[("fds.v2", src)], &["fds.v2"]);
    assert_eq!((code, stdout.as_str(), stderr.as_str()), (Some(0), "start\n0\nend\n", "hello\n"));
}

#[cfg(unix)]
#[test]
fn a_file_name_that_isnt_utf8_is_a_usage_error() {
    // INTERNALS 19.1: said so, exit 2, not "no such file".
    use std::os::unix::ffi::OsStrExt;
    let home = TempHome::new();
    let name = std::ffi::OsStr::from_bytes(b"caf\xe9.v2");
    std::fs::write(home.0.join(name), "print(1)\n").unwrap();
    for args in [vec![name], vec![std::ffi::OsStr::new("check"), name]] {
        let out = v2_command(&home.0).args(&args).current_dir(&home.0).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{err}");
        assert!(err.contains("is not valid UTF-8"), "{err}");
    }
}

#[test]
fn a_worker_writing_to_a_closed_stdout_is_an_io_error() {
    // INTERNALS 9.9: the write fails with no V2 expression waiting, so the IOError comes when the
    // block returns, catchable, and the worker is still there.
    if !needs("py", "a_worker_writing_to_a_closed_stdout_is_an_io_error") {
        return;
    }
    let src = "\
func flood() {
    @py {
        for i in range(2000):
            print(\"x\" * 1000)
    }
}
try {
    flood()
    print(\"not reached: nothing failed\", stderr: true)
} catch (e) {
    print(\"caught\", e.type, e.message.index_of(\"cannot write to stdout\") >= 0, stderr: true)
}
print(@py { 1 + 1 }, stderr: true)
";
    let home = TempHome::new();
    std::fs::write(home.0.join("flood.v2"), src).unwrap();
    let mut child = v2_command(&home.0)
        .arg("flood.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    drop(child.stdout.take()); // close the read end
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("caught IOError true\n2\n"), "{err}");
}

#[test]
fn a_failed_write_belongs_to_the_request_that_wrote_it() {
    // INTERNALS 9.9: not to a later request, and not to a nested one; an `err` wins over it.
    if !needs("py", "a_failed_write_belongs_to_the_request_that_wrote_it") || !needs("js", "a_failed_write_belongs_to_the_request_that_wrote_it") {
        return;
    }
    let src = "\
func first() {
    @py {
        print(\"x\" * 100000)
        raise ValueError(\"boom\")
    }
}
try {
    first()
} catch (e) {
    print(\"A\", e.type, stderr: true)
}
print(\"B\", @py { 40 + 2 }, stderr: true)
@py {
    def go(f):
        print(\"x\" * 100000)
        return f()
    @export { go }
}
@js {
    function quiet() { return 5 }
    @export { quiet }
}
@import { go } from @py
@import { quiet } from @js
try {
    go(func() {
        print(\"C\", quiet(), stderr: true)
    })
    print(\"not reached\", stderr: true)
} catch (e) {
    print(\"D\", e.type, stderr: true)
}
";
    let home = TempHome::new();
    std::fs::write(home.0.join("stale.v2"), src).unwrap();
    let mut child = v2_command(&home.0)
        .arg("stale.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    drop(child.stdout.take()); // close the read end
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("A ForeignError\nB 42\nC 5\nD IOError\n"), "{err}");
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

/// For a run.rs test that needs an engine: whether it is here. A missing engine skips the test,
/// or fails it when `V2_TEST_REQUIRE` names it (22.2).
fn needs(tag: &str, test: &str) -> bool {
    let available = engine_availability(&[tag.to_string()]).unwrap_or_else(|e| panic!("{e}"));
    if available[tag] {
        return true;
    }
    let required = std::env::var("V2_TEST_REQUIRE").unwrap_or_default();
    assert!(!required.split(',').any(|t| t.trim() == tag), "engine {tag} not available, and V2_TEST_REQUIRE requires it");
    // Straight to stderr: libtest would swallow eprintln! output of a test that passes.
    let _ = writeln!(std::io::stderr(), "SKIP {test} (engine {tag} not available)");
    false
}

/// A fresh temp folder for one run: what `temp_dir()` gives v2 and its children (20.3).
fn with_temp(cmd: &mut Command, tmp: &Path) {
    std::fs::create_dir_all(tmp).unwrap();
    cmd.env("TMP", tmp).env("TEMP", tmp).env("TMPDIR", tmp);
}

fn names_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
}

#[test]
fn python_finds_modules_next_to_the_script_then_in_the_project() {
    // DOCS 5 and 7, INTERNALS 9.3, 14.3: sys.path starts with the .v2 file's folder, then the
    // project root, also when PYTHONPATH lists the root after another folder; the adapter's own
    // folder is not on it; json.py and asyncio.py next to the script don't replace the adapter's
    // modules (an inherited PYTHONPATH is the user's own, 20.2); a function in a .py file next to
    // the script works with multiprocessing's spawn.
    if !needs("py", "python_finds_modules_next_to_the_script_then_in_the_project") {
        return;
    }
    let home = TempHome::new();
    let main = r#"@py {
    import os, sys, helper, rootmod
    here = os.path.dirname(__file__)
    root = os.path.dirname(here)
    print(helper.where(), rootmod.where())
    print(sys.path.index(here), sys.path.index(root), sys.path.index(os.environ["V2_OTHER"]) > 1)
    print(any(os.path.basename(p.rstrip("/\\")) == "adapters" for p in sys.path))
}
print(@py {
    async def f():
        return "awaited with the stdlib asyncio"
    f()
})
@py {
    import multiprocessing, helper
    with multiprocessing.get_context("spawn").Pool(2) as pool:
        print(pool.map(helper.square, [1, 2, 3]))
}
"#;
    let files = [
        ("proj/v2.toml", "# the project root
"),
        ("proj/rootmod.py", "def where():
    return 'root'
"),
        ("proj/sub/json.py", "raise ImportError('a project json.py replaced the adapter json')
"),
        ("proj/sub/asyncio.py", "raise ImportError('a project asyncio.py replaced the stdlib asyncio')
"),
        ("proj/sub/helper.py", "def where():
    return 'sub'

def square(x):
    return x * x
"),
        ("proj/sub/main.v2", main),
        ("other/placeholder.txt", ""),
    ];
    for (path, text) in files {
        let p = home.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let canon = |p: PathBuf| {
        let c = std::fs::canonicalize(p).unwrap().to_string_lossy().into_owned();
        c.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(c)
    };
    let (other, proj) = (canon(home.0.join("other")), canon(home.0.join("proj")));
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut cmd = v2_command(&home.0);
    cmd.arg(home.0.join("proj/sub/main.v2")).current_dir(&home.0).env("PYTHONPATH", format!("{other}{sep}{proj}")).env("V2_OTHER", &other);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(120)).unwrap();
    let stdout = normalize(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(stdout, "sub root
0 1 True
False
awaited with the stdlib asyncio
[1, 4, 9]
");
}

#[test]
fn every_exit_path_removes_the_run_dir() {
    // INTERNALS 20.3, ROADMAP 1.4: a run that started the py engine, whose adapter lives in the
    // run dir, removes it at a normal end, an uncaught error, exit() (also inside a callback
    // while Python waits), and after the worker crashed.
    if !needs("py", "every_exit_path_removes_the_run_dir") {
        return;
    }
    let home = TempHome::new();
    let scripts = [
        ("end.v2", "@py { print(1) }\n", 0),
        ("throw.v2", "@py { print(1) }\nthrow \"boom\"\n", 1),
        ("exit.v2", "@py { print(1) }\nexit(4)\n", 4),
        ("foreign.v2", "@py { 1 / 0 }\n", 1),
        ("crash.v2", "@py { import os; os._exit(3) }\n", 1),
        ("callback_exit.v2", "@py(f: func() => exit(5)) { f() }\nprint(\"not reached\")\n", 5),
    ];
    for (name, src, code) in scripts {
        std::fs::write(home.0.join(name), src).unwrap();
        let tmp = home.0.join(format!("tmp-{name}"));
        let mut cmd = v2_command(&home.0);
        cmd.arg(name).current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        with_temp(&mut cmd, &tmp);
        let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
        assert_eq!(out.status.code(), Some(code), "{name}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(!String::from_utf8_lossy(&out.stdout).contains("not reached"), "{name}");
        assert_eq!(names_in(&tmp), Vec::<String>::new(), "{name} left its run dir");
    }
}

/// Whether process `pid` still runs (not a zombie waiting for its new parent to reap it).
fn running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks that the process exists.
        if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return false;
        }
        // Where /proc exists, a zombie (state Z, after the parenthesized name) has ended.
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => !stat.rsplit(')').next().unwrap_or("").trim_start().starts_with('Z'),
            Err(_) => true,
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: plain Win32 calls on a handle this function owns.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code);
            CloseHandle(h);
            ok != 0 && code == 259 // STILL_ACTIVE
        }
    }
}

#[test]
fn an_interrupt_during_a_py_block_exits_130_and_cleans_up() {
    // R40 (INTERNALS 6.5, 8.3): SIGINT, and separately SIGTERM (Windows: Ctrl+Break), while a
    // @py block sleeps: exit 130 within 2 s, the worker is gone and the run dir is removed.
    if !needs("py", "an_interrupt_during_a_py_block_exits_130_and_cleans_up") {
        return;
    }
    #[cfg(unix)]
    let signals = [libc::SIGINT, libc::SIGTERM];
    #[cfg(windows)]
    let signals = [0];
    for signal in signals {
        let home = TempHome::new();
        let src = "@py {\n    import os, time\n    open(\"worker.pid\", \"w\").write(str(os.getpid()))\n    time.sleep(30)\n}\nprint(\"not reached\")\n";
        std::fs::write(home.0.join("sleepy.v2"), src).unwrap();
        let tmp = home.0.join("tmp");
        let mut cmd = v2_command(&home.0);
        cmd.arg("sleepy.v2").current_dir(&home.0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        with_temp(&mut cmd, &tmp);
        interruptible(&mut cmd);
        let child = cmd.spawn().expect("start v2");
        let pid_file = home.0.join("worker.pid");
        let deadline = Instant::now() + Duration::from_secs(30);
        let pid = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
                break pid;
            }
            assert!(Instant::now() < deadline, "the block never started");
            std::thread::sleep(Duration::from_millis(20));
        };
        let start = Instant::now();
        #[cfg(unix)]
        // SAFETY: sending a signal to our own child.
        unsafe {
            libc::kill(child.id() as libc::pid_t, signal);
        }
        #[cfg(windows)]
        {
            let _ = signal;
            interrupt(&child);
        }
        let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(10)).expect("v2 ended after the interrupt");
        assert_eq!(out.status.code(), Some(130), "signal {signal}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(start.elapsed() < Duration::from_secs(2), "signal {signal}: {:?}", start.elapsed());
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "signal {signal}: no output after an interrupt");
        let gone = Instant::now() + Duration::from_secs(2);
        while running(pid) {
            assert!(Instant::now() < gone, "signal {signal}: worker {pid} outlived the run");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(names_in(&tmp), Vec::<String>::new(), "signal {signal}: the run dir was left");
    }
}

#[test]
fn an_interrupt_still_writes_what_was_printed() {
    // INTERNALS 6.5: output into a pipe or a file waits in `Out`; an interrupt writes it out
    // before the run ends, the program's own and a block's.
    let cases = [
        ("print(\"before\")\nlet n = 0\nwhile true { n += 1 }\n", "before\n", None),
        (
            "print(\"before\")\n@py {\n    import time\n    print(\"from py\", flush=True)\n    open(\"ready\", \"w\").write(\"1\")\n    time.sleep(30)\n}\n",
            "before\nfrom py\n",
            Some("py"),
        ),
        // JavaScript has no flush: a line that doesn't end goes out at once (9.9).
        (
            "@js {\n    process.stdout.write(\"partial \")\n    require(\"fs\").writeFileSync(\"ready\", \"1\")\n    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 30000)\n}\n",
            "partial ",
            Some("js"),
        ),
    ];
    for (src, want, engine) in cases {
        if engine.is_some_and(|tag| !needs(tag, "an_interrupt_still_writes_what_was_printed")) {
            continue;
        }
        let home = TempHome::new();
        std::fs::write(home.0.join("printed.v2"), src).unwrap();
        let mut cmd = v2_command(&home.0);
        cmd.arg("printed.v2").current_dir(&home.0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        interruptible(&mut cmd);
        let child = cmd.spawn().expect("start v2");
        let deadline = Instant::now() + Duration::from_secs(30);
        while engine.is_some() && !home.0.join("ready").exists() {
            assert!(Instant::now() < deadline, "the block never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(500));
        interrupt(&child);
        let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(10)).expect("v2 ended after the interrupt");
        assert_eq!(out.status.code(), Some(130));
        assert_eq!(String::from_utf8_lossy(&out.stdout), want);
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
fn check_keeps_file_order_for_files_that_are_not_utf8() {
    // INTERNALS 19.3: a file that isn't UTF-8 reports at the place it was met.
    let home = TempHome::new();
    let d = &home.0;
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
    assert_eq!((code, stdout.as_str()), (Some(1), "check main.v2: 3 files, 2 errors\n"), "{stderr}");
    let bad = stderr.find("bad.v2:1:1: SyntaxError: file is not valid UTF-8").expect(&stderr);
    let good = stderr.find("good.v2:2:5: SyntaxError: 'y' is already declared").expect(&stderr);
    assert!(bad < good, "{stderr}");
    let (code, stdout, stderr) = check("bad.v2");
    assert_eq!((code, stdout.as_str()), (Some(1), "check bad.v2: 1 file, 1 error\n"), "{stderr}");
    assert_eq!(stderr, "bad.v2:1:1: SyntaxError: file is not valid UTF-8\n");
}

#[test]
fn no_home_folder_is_silent() {
    // INTERNALS 18.1: without a home folder the ~/.v2 caches are off; nothing says so.
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
    assert_eq!(run(&["check", "main.v2"]), (Some(0), "check main.v2: 1 file, 0 errors\n".to_string(), String::new()));
    assert_eq!(run(&["main.v2"]), (Some(0), "ran\n".to_string(), String::new()));
    let (code, _, stderr) = run(&["doctor"]);
    assert_eq!((code, stderr.as_str()), (Some(0), ""));
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
    // INTERNALS 6.5: SIGINT, SIGTERM, SIGHUP and SIGQUIT all end the run with 130, with no catch,
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
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
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
    run_with_files_env(files, args, &[])
}

/// `run_with_files` with these environment variables set (such as `V2_ENGINE_PY`).
fn run_with_files_env(files: &[(&str, &str)], args: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String, String, TempHome) {
    let home = TempHome::new();
    for (path, text) in files {
        let p = home.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let mut cmd = v2_command(&home.0);
    cmd.args(args).current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(120)).unwrap();
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
    for line in lines.take_while(|l| !l.starts_with("notes:") && !l.starts_with("config file:")) {
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
    assert!(stdout.contains("config file: none"), "{stdout}");
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
    let files = [("v2.toml", "[engines.py]\ncommand = [\"missing/python.exe\"]\n")];
    let (code, stdout, stderr, home) = run_with_files_env(&files, &["doctor"], &[("V2_ENGINE_JS", "missing/node.exe")]);
    assert_eq!(code, Some(0), "{stderr}");
    let rows = doctor_rows(&stdout);
    assert_eq!((rows["py"][0].as_str(), rows["py"][3].as_str()), ("broken", "v2.toml"), "{stdout}");
    assert!(rows["py"][1].ends_with("python.exe"), "{stdout}");
    assert_eq!((rows["js"][0].as_str(), rows["js"][3].as_str()), ("broken", "V2_ENGINE_JS"), "{stdout}");
    assert!(stdout.contains("js: engine 'js' command '") && stdout.contains("not found (set by V2_ENGINE_JS)"), "{stdout}");
    assert_eq!(rows["ts"][0], "broken", "ts follows js:\n{stdout}");
    let toml = home.0.join("v2.toml");
    assert!(stdout.contains(&format!("py: engine 'py' command '{}' not found (set by {})", home.0.join("missing/python.exe").display(), toml.display())), "{stdout}");
    assert!(stdout.contains(&format!("config file: {}", toml.display())), "{stdout}");
}

#[test]
fn an_uncaught_engine_not_found_exits_3() {
    // R48 (EngineNotFound part) and INTERNALS 19.1: no usable toolchain is exit 3; it is catchable.
    let script = "print(\"before\")\n@py {\n    x = 1\n}\n";
    let missing = [("V2_ENGINE_PY", "missing/python.exe")];
    let (code, stdout, stderr, _) = run_with_files_env(&[("main.v2", script)], &["main.v2"], &missing);
    assert_eq!(code, Some(3), "{stderr}");
    assert_eq!(stdout, "before\n");
    assert!(stderr.starts_with("main.v2:2:1: EngineNotFound: engine 'py' command '") && stderr.contains("not found (set by V2_ENGINE_PY)"), "{stderr}");
    let files = [("v2.toml", "[engines.py]\ncommand = [\"missing/python.exe\"]\n"), ("main.v2", script)];
    let (code, _, stderr, _) = run_with_files(&files, &["main.v2"]);
    assert_eq!(code, Some(3), "{stderr}");
    let caught = "try {\n    @py {\n        x = 1\n    }\n} catch (e) {\n    print(e.type)\n}\n";
    let (code, stdout, stderr, _) = run_with_files_env(&[("main.v2", caught)], &["main.v2"], &missing);
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

#[test]
fn nesting_stops_past_256_levels() {
    // A body is a level, and so is each expression, a statement's own included (INTERNALS 4.1).
    let parens = |n: usize| format!("let x = {}1{}\n", "(".repeat(n), ")".repeat(n));
    let ifs = |n: usize| format!("{}{}", "if true {\n".repeat(n), "}\n".repeat(n));
    for (src, fits) in [(parens(255), true), (parens(256), false), (ifs(256), true), (ifs(257), false)] {
        let (code, _, stderr, _home) = run_with_files(&[("deep.v2", &src)], &["deep.v2"]);
        assert_eq!(code == Some(0), fits, "{stderr}");
        assert_eq!(stderr.contains("SyntaxError: nested too deeply"), !fits, "{stderr}");
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
fn runs_use_the_probe_cache_and_doctor_probes_afresh() {
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
    let (code, stdout, _) = run_in_home(&home.0, &["doctor"], &[]);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(!stdout.contains("99.0.0") && !tampered(), "doctor probes afresh (19.2):\n{stdout}");
}

#[test]
fn doctor_columns_are_longest_cell_plus_two() {
    let (code, stdout, stderr, _) = run_with_files(&[], &["doctor"]);
    assert_eq!(code, Some(0), "{stderr}");
    let lines: Vec<&str> = stdout.lines().take_while(|l| !l.starts_with("notes:") && !l.starts_with("config file:")).collect();
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

#[test]
fn an_inner_import_that_fails_at_load_starts_no_worker() {
    // INTERNALS 12.1: a block's inner imports (step 5) are checked before its engine's worker
    // starts (step 6). A sitecustomize.py on PYTHONPATH marks every Python start; the probe
    // (`--version`) doesn't run it.
    if !needs("py", "an_inner_import_that_fails_at_load_starts_no_worker") {
        return;
    }
    let mark = "import os\nopen(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'started'), 'a').write('x')\n";
    let failing = "try {\n    @py {\n        @import { nope } from v2\n        pass\n    }\n} catch (e) {\n    print(e.type, e.message)\n}\n";
    let files = [("site/sitecustomize.py", mark), ("main.v2", failing), ("v2.toml", "")];
    let (code, out, err, home) = run_with_files_env(&files, &["main.v2"], &[("PYTHONPATH", "site")]);
    assert_eq!((code, out.as_str()), (Some(0), "ImportError 'nope' is not a top-level name in main.v2\n"), "{err}");
    assert!(!home.0.join("site/started").exists(), "no Python worker was started");
    // The control: a block that loads starts one.
    let files = [("site/sitecustomize.py", mark), ("main.v2", "@py { pass }\n"), ("v2.toml", "")];
    let (code, _, err, home) = run_with_files_env(&files, &["main.v2"], &[("PYTHONPATH", "site")]);
    assert_eq!(code, Some(0), "{err}");
    assert!(home.0.join("site/started").exists(), "the marker works");
}

#[test]
fn an_uncaught_foreign_error_prints_like_docs_10() {
    // R16 (uncaught): the header at the foreign frame's .v2 position, the excerpt and caret, then
    // the foreign frames before V2's (DOCS 10, INTERNALS 2.4, 2.5, 12.6). Python 3.11+ reports
    // columns; 3.10 would put the caret at the line's first character.
    let py = "@py risky {\n    def ratio(a, b):\n        return a / b\n    @export { ratio }\n}\n@import { ratio } from risky\n\n\nratio(1, 0)\n";
    let js = "@js calc {\n    function ratio(a, b) {\n        if (b === 0) throw new RangeError(\"division by zero\");\n        return a / b;\n    }\n    @export { ratio }\n}\n@import { ratio } from calc\nfunc go() {\n    return ratio(1, 0)\n}\ngo()\n";
    let cases = [
        ("py", py, "main.v2:3:16: ForeignError: ZeroDivisionError: division by zero\n    return a / b\n           ^\n  at ratio (main.v2:3, in @py risky)\n  at main.v2:9\n"),
        ("js", js, "main.v2:3:28: ForeignError: RangeError: division by zero\n    if (b === 0) throw new RangeError(\"division by zero\");\n                       ^\n  at ratio (main.v2:3, in @js calc)\n  at go (main.v2:10)\n  at main.v2:12\n"),
    ];
    for (tag, src, expected) in cases {
        if !needs(tag, "an_uncaught_foreign_error_prints_like_docs_10") {
            continue;
        }
        let (code, out, err, _home) = run_with_files(&[("main.v2", src), ("v2.toml", "")], &["main.v2"]);
        assert_eq!((code, out.as_str(), err.as_str()), (Some(1), "", expected), "{tag}");
    }
}

#[test]
fn a_parked_worker_keeps_everything() {
    // R65: state, handles and stored callbacks all survive a park (INTERNALS 8.8).
    if !needs("py", "a_parked_worker_keeps_everything") {
        return;
    }
    let src = r#"@py box {
    import datetime
    n = 0
    kept = []
    def bump():
        global n
        n += 1
        return n
    def keep(f):
        kept.append(f)
    def run():
        return kept[0]()
    def when():
        return datetime.date(2024, 2, 29)
    @export { bump, keep, run, when }
}
@import { bump, keep, run, when } from box
print(bump())
keep(func() => "called")
let d = when()
sleep(0.4)
print(bump())
print(d.year)
print(run())
"#;
    let (code, out, err, _home) = run_with_files_env(&[("main.v2", src), ("v2.toml", "")], &["main.v2"], &[("V2_TEST_PARK_MS", "50")]);
    assert_eq!((code, out.as_str()), (Some(0), "1\n2\n2024\ncalled\n"), "{err}");
}

/// A Python block that holds 30 MB and can read its own working set.
#[cfg(windows)]
const WORKING_SET: &str = r#"@py box {
    import ctypes, time
    class Mem(ctypes.Structure):
        _fields_ = [("cb", ctypes.c_uint32), ("faults", ctypes.c_uint32)] + [(n, ctypes.c_size_t) for n in "pqrstuvw"]
    k = ctypes.windll.kernel32
    k.GetCurrentProcess.restype = ctypes.c_void_p
    k.K32GetProcessMemoryInfo.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32]
    blob = bytearray(b"x" * 30000000)
    def ws():
        m = Mem()
        m.cb = ctypes.sizeof(m)
        k.K32GetProcessMemoryInfo(k.GetCurrentProcess(), ctypes.byref(m), m.cb)
        return m.q
    def busy():
        before = ws()
        time.sleep(0.4)
        return ws() > before / 2
    @export { ws, busy }
}
@import { ws, busy } from box
print(busy())
let before = ws()
sleep(0.4)
print(ws() < before / 2)
"#;

#[cfg(windows)]
#[test]
fn an_idle_worker_gives_back_its_ram() {
    // R65: idle, its working set goes; during a call it stays (INTERNALS 8.8).
    if !needs("py", "an_idle_worker_gives_back_its_ram") {
        return;
    }
    let (code, out, err, _home) = run_with_files_env(&[("main.v2", WORKING_SET), ("v2.toml", "")], &["main.v2"], &[("V2_TEST_PARK_MS", "50")]);
    assert_eq!((code, out.as_str()), (Some(0), "true\ntrue\n"), "{err}");
}

#[cfg(windows)]
#[test]
fn a_worker_with_a_call_timeout_keeps_its_ram() {
    // R65: coming back must never trip the timer (INTERNALS 8.8).
    if !needs("py", "a_worker_with_a_call_timeout_keeps_its_ram") {
        return;
    }
    let toml = "[engines.py]\ncall_timeout = 30\n";
    let (code, out, err, _home) = run_with_files_env(&[("main.v2", WORKING_SET), ("v2.toml", toml)], &["main.v2"], &[("V2_TEST_PARK_MS", "50")]);
    assert_eq!((code, out.as_str()), (Some(0), "true\nfalse\n"), "{err}");
}

// ---------------------------------------------------------------------------------------------
// The phase 0-1 audit (ROADMAP 1.14): promises that had no test

#[test]
fn a_syntax_error_is_the_first_thing_on_stderr() {
    // R07: stderr *starts* with the diagnostic (the harness finds header lines in order only).
    let src = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/host/syntax/unterminated.v2")).unwrap();
    let first = parse_header(&src).unwrap().stderr[0].clone();
    let (code, stdout, stderr) = run_generated("unterminated.v2", &src);
    assert_eq!((code, stdout.as_str()), (Some(1), ""), "{stderr}");
    assert!(normalize(&stderr).starts_with(&format!("{first}\n")), "{stderr}");
}

#[test]
fn exit_codes_past_8_bits() {
    // DOCS 3.11: on Linux and macOS only the low 8 bits of exit's code reach the shell.
    for (n, unix, windows) in [(257, 1, 257), (-1, 255, -1)] {
        let (code, _, stderr) = run_generated("exit.v2", &format!("exit({n})\n"));
        assert_eq!(code, Some(if cfg!(windows) { windows } else { unix }), "exit({n}): {stderr}");
    }
}

#[cfg(unix)]
#[test]
fn args_and_getenv_turn_invalid_utf8_into_replacement_characters() {
    // DOCS 3.11: they don't fail on bytes that aren't UTF-8.
    use std::os::unix::ffi::OsStrExt;
    let home = TempHome::new();
    std::fs::write(home.0.join("a.v2"), "print(args())\nprint(getenv(\"V2_BAD\"))\n").unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg("a.v2").arg(std::ffi::OsStr::from_bytes(b"x\xfey")).env("V2_BAD", std::ffi::OsStr::from_bytes(b"a\xffb"));
    cmd.current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "[\"x\u{FFFD}y\"]\na\u{FFFD}b\n");
}

#[test]
fn check_starts_no_engine_and_writes_nothing() {
    // DOCS 15, 16: `v2 check` only parses and checks. Blocks, package imports and imports from
    // blocks start nothing, run nothing and write nothing, not even the probe cache.
    let src = "@import { mean } from py.statistics\n@py stats {\n    def f():\n        return 1\n    @export { f }\n}\n@import { f } from stats\n@py { open(\"ran_py.txt\", \"w\").write(\"x\") }\n@js { require(\"fs\").writeFileSync(\"ran_js.txt\", \"x\") }\nprint(f(), mean([1]))\n";
    let (code, stdout, stderr, home) = run_with_files(&[("c.v2", src), ("v2.toml", "# root\n")], &["check", "c.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "check c.v2: 1 file, 0 errors\n"), "{stderr}");
    let mut left = names_in(&home.0);
    left.sort();
    assert_eq!(left, ["c.v2", "v2.toml"]);
}

#[test]
fn a_waiting_import_that_failed_is_tried_again_at_its_next_use() {
    // INTERNALS 12.7, ch. 21: a waiting @import that fails when its language starts leaves its
    // references waiting; each use tries again, and works once the module exists.
    if !needs("py", "a_waiting_import_that_failed_is_tried_again_at_its_next_use") {
        return;
    }
    let src = "@import { hi } from py.late_mod\nprint(\"waiting\")\ntry {\n    @py { print(\"not printed\") }\n} catch (e) {\n    print(\"first:\", e.type, e.message)\n}\ntry {\n    print(hi())\n} catch (e) {\n    print(\"second:\", e.type, e.message)\n}\nwrite_file(\"late_mod.py\", \"def hi():\\n    return 'hi'\\n\")\n@py {\n    import importlib\n    importlib.invalidate_caches()\n}\nprint(hi())\n";
    let (code, stdout, stderr, _home) = run_with_files(&[("retry.v2", src), ("v2.toml", "# root\n")], &["retry.v2"]);
    assert_eq!(code, Some(0), "{stderr}");
    let missing = "ImportError No module named 'late_mod'";
    assert_eq!(stdout, format!("waiting\nfirst: {missing}\nsecond: {missing}\nhi\n"));
}

#[test]
fn exit_inside_nested_foreign_calls_runs_no_finally() {
    // ch. 21 Host row, INTERNALS 9.10: exit() under V2 -> js -> py -> V2 ends the run with its
    // code at once; no `finally` runs, V2's or the workers'.
    if !needs("py", "exit_inside_nested_foreign_calls_runs_no_finally") || !needs("js", "exit_inside_nested_foreign_calls_runs_no_finally") {
        return;
    }
    let src = r#"@py w {
    def go(f):
        try:
            f()
        finally:
            open("py_finally.txt", "w").write("ran")
    @export { go }
}
@js j {
    function jgo(f) {
        try {
            return f()
        } finally {
            require("fs").writeFileSync("js_finally.txt", "ran")
        }
    }
    @export { jgo }
}
@import { go } from w
@import { jgo } from j
print("start")
try {
    jgo(func() => go(func() => exit(5)))
} finally {
    write_file("v2_finally.txt", "ran")
}
print("not reached")
"#;
    let (code, stdout, stderr, home) = run_with_files(&[("nested.v2", src)], &["nested.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(5), "start\n"), "{stderr}");
    std::thread::sleep(Duration::from_millis(300));
    let ran: Vec<String> = names_in(&home.0).into_iter().filter(|n| n.ends_with("_finally.txt")).collect();
    assert!(ran.is_empty(), "these finally blocks ran: {ran:?}");
}

#[test]
fn workers_run_in_v2s_working_directory_and_environment() {
    // DOCS 16, INTERNALS 8.3: workers inherit V2's working directory, not the script's folder,
    // and its environment.
    if !needs("py", "workers_run_in_v2s_working_directory_and_environment") || !needs("js", "workers_run_in_v2s_working_directory_and_environment") {
        return;
    }
    let home = TempHome::new();
    let main = "print(@py { import os; [os.path.basename(os.getcwd()), os.environ.get(\"V2_TEST_MARK\")] })\nprint(@js { [require(\"path\").basename(process.cwd()), process.env.V2_TEST_MARK] })\n";
    for (path, text) in [("proj/v2.toml", "# root\n"), ("proj/sub/main.v2", main)] {
        let p = home.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    std::fs::create_dir_all(home.0.join("work")).unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg(home.0.join("proj/sub/main.v2")).current_dir(home.0.join("work")).env("V2_TEST_MARK", "mark");
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(normalize(&String::from_utf8_lossy(&out.stdout)), "[\"work\", \"mark\"]\n[\"work\", \"mark\"]\n");
}

#[test]
fn js_packages_are_found_through_node_path() {
    // DOCS 7: after the node_modules folders from the script's folder up, Node looks in NODE_PATH.
    if !needs("js", "js_packages_are_found_through_node_path") {
        return;
    }
    let home = TempHome::new();
    let main = "@import { hi } from js.npmod\nprint(hi())\nprint(@js { require(\"npmod\").hi() })\n";
    for (path, text) in [("np/npmod/index.js", "exports.hi = () => \"from NODE_PATH\";\n"), ("proj/v2.toml", "# root\n"), ("proj/main.v2", main)] {
        let p = home.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let run = |node_path: Option<PathBuf>| {
        let mut cmd = v2_command(&home.0);
        cmd.arg("proj/main.v2").current_dir(&home.0).env_remove("NODE_PATH");
        if let Some(p) = node_path {
            cmd.env("NODE_PATH", p);
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
        (out.status.code(), normalize(&String::from_utf8_lossy(&out.stdout)), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    let (code, stdout, stderr) = run(Some(home.0.join("np")));
    assert_eq!((code, stdout.as_str()), (Some(0), "from NODE_PATH\nfrom NODE_PATH\n"), "{stderr}");
    let (code, _, stderr) = run(None);
    assert!(code == Some(1) && stderr.contains("Cannot find module 'npmod'"), "{stderr}");
}

/// The Python that blocks run on here (a block's `sys.executable`), for tests that make their own
/// venv or engine command.
fn block_python() -> String {
    let (code, stdout, stderr) = run_generated("which.v2", "print(@py { import sys; sys.executable })\n");
    assert_eq!(code, Some(0), "{stderr}");
    stdout.trim().to_string()
}

/// The first folder named `name` under `root`.
fn find_dir(root: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == name) {
                return Some(p);
            }
            if let Some(found) = find_dir(&p, name) {
                return Some(found);
            }
        }
    }
    None
}

#[test]
fn a_python_venv_from_v2_toml_is_used_unless_a_command_is_set() {
    // DOCS 5, 7, 14: `[python] venv`, relative to the v2.toml, gives blocks that venv's Python and
    // its packages; `[engines.py] command` wins over it.
    if !needs("py", "a_python_venv_from_v2_toml_is_used_unless_a_command_is_set") {
        return;
    }
    let python = block_python();
    let home = TempHome::new();
    let made = Command::new(&python).args(["-m", "venv", "--without-pip"]).arg(home.0.join(".venv")).output().expect("run python -m venv");
    assert!(made.status.success(), "python -m venv: {}", String::from_utf8_lossy(&made.stderr));
    let site = find_dir(&home.0.join(".venv"), "site-packages").expect("the venv's site-packages");
    std::fs::write(site.join("venvonly.py"), "def where():\n    return \"venv package\"\n").unwrap();
    std::fs::create_dir_all(home.0.join("sub")).unwrap();
    let main = "@import { where } from py.venvonly\nprint(where())\nprint(@py { import os, sys; os.path.basename(sys.prefix) })\n";
    std::fs::write(home.0.join("sub/main.v2"), main).unwrap();
    std::fs::write(home.0.join("v2.toml"), "[python]\nvenv = \".venv\"\n").unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["sub/main.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(0), "venv package\n.venv\n"), "{stderr}");
    std::fs::write(home.0.join("v2.toml"), format!("[python]\nvenv = \".venv\"\n\n[engines.py]\ncommand = ['{python}']\n")).unwrap();
    std::fs::write(home.0.join("sub/main.v2"), "print(@py { import os, sys; os.path.basename(sys.prefix) })\n").unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["sub/main.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_ne!(stdout, ".venv\n", "the configured command lost to the venv");
}

#[test]
fn startup_timeout_from_v2_toml_reaches_the_worker() {
    // DOCS 14, INTERNALS 8.4: a configured command that never says hello is stopped after the
    // project's startup_timeout, and the engine stays down for the rest of the run.
    if !needs("py", "startup_timeout_from_v2_toml_reaches_the_worker") {
        return;
    }
    let python = block_python();
    let slow = "import sys, time\nif sys.argv[1:] == [\"--version\"]:\n    print(\"Python %d.%d.%d\" % sys.version_info[:3])\n    sys.exit(0)\ntime.sleep(60)\n";
    let toml = format!("[engines.py]\ncommand = ['{python}', 'slow.py']\nstartup_timeout = 1\n");
    let main = "let t0 = time()\ntry {\n    @py { 1 }\n} catch (e) {\n    print(e.type, time() - t0 < 10)\n    print(e.message)\n}\ntry {\n    @py { 1 }\n} catch (e) {\n    print(e.type, e.message)\n}\n";
    let (code, stdout, stderr, _home) = run_with_files(&[("slow.py", slow), ("v2.toml", &toml), ("main.v2", main)], &["main.v2"]);
    assert_eq!(code, Some(0), "{stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.first(), Some(&"EngineTimeout true"), "{stdout}");
    assert!(lines[1].starts_with("engine 'py' did not start within 1 s"), "{stdout}");
    assert!(lines[2].starts_with("EngineCrashed engine 'py' is not running (stopped earlier: did not start within 1 s)"), "{stdout}");
}

/// Kills, by pid, the processes a test left running on purpose, however the test ends.
struct Reap(Vec<u32>);

impl Drop for Reap {
    fn drop(&mut self) {
        for &pid in self.0.iter().filter(|&&p| p != 0) {
            #[cfg(unix)]
            // SAFETY: a signal to a process this test started.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
            #[cfg(windows)]
            // SAFETY: plain Win32 calls on a handle this function owns.
            unsafe {
                use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
                let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
                if !h.is_null() {
                    TerminateProcess(h, 1);
                    windows_sys::Win32::Foundation::CloseHandle(h);
                }
            }
        }
    }
}

#[test]
fn children_of_blocks_end_with_the_run_unless_they_left() {
    // DOCS 16: what Python and JavaScript code started is stopped when V2 exits; on Linux and
    // macOS one that detached itself lives on, on Windows only one that broke away from V2's job.
    if !needs("py", "children_of_blocks_end_with_the_run_unless_they_left") || !needs("js", "children_of_blocks_end_with_the_run_unless_they_left") {
        return;
    }
    let src = r#"@py {
    import os, subprocess, sys
    quiet = dict(stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    away = dict(creationflags=0x01000000) if os.name == "nt" else dict(start_new_session=True)
    sleep = [sys.executable, "-c", "import time; time.sleep(60)"]
    plain = subprocess.Popen(sleep, **quiet)
    try:
        left = subprocess.Popen(sleep, **quiet, **away).pid
    except PermissionError:
        left = 0  # this machine's own job forbids breaking away
    open("py.pids", "w").write(str(plain.pid) + " " + str(left))
}
@js {
    const { spawn } = require("child_process")
    const sleep = ["-e", "setTimeout(() => {}, 60000)"]
    const plain = spawn(process.execPath, sleep, { stdio: "ignore" })
    const left = spawn(process.execPath, sleep, { stdio: "ignore", detached: true })
    plain.unref()
    left.unref()
    require("fs").writeFileSync("js.pids", plain.pid + " " + left.pid)
}
"#;
    let (code, _stdout, stderr, home) = run_with_files(&[("kids.v2", src)], &["kids.v2"]);
    let pids = |f: &str| -> Vec<u32> {
        std::fs::read_to_string(home.0.join(f)).unwrap_or_default().split_whitespace().map(|p| p.parse().unwrap()).collect()
    };
    let (py, js) = (pids("py.pids"), pids("js.pids"));
    let _reap = Reap(py.iter().chain(&js).copied().collect());
    assert_eq!((code, py.len(), js.len()), (Some(0), 2, 2), "{stderr}");
    let deadline = Instant::now() + Duration::from_secs(3);
    for pid in [py[0], js[0]] {
        while running(pid) {
            assert!(Instant::now() < deadline, "the child {pid} outlived the run");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    if py[1] == 0 {
        let _ = writeln!(std::io::stderr(), "SKIP the breakaway part of children_of_blocks_end_with_the_run_unless_they_left (not allowed here)");
    } else {
        assert!(running(py[1]), "the Python child that left V2 ended with it");
    }
    assert_eq!(running(js[1]), cfg!(unix), "a detached Node child lives on only on Linux and macOS");
}
