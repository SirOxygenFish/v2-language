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
//! `stderr:` line is rejected. `requires:` (engine-gated tests) arrives with `engine_available`
//! in roadmap 1.1; until then a test that uses it fails instead of silently skipping.
//!
//! The command-line cases (`--version`, `--help`, usage errors) are ordinary Rust tests below.

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

/// Wait for `child` with a deadline. Stdin is fed and stdout/stderr are drained on helper threads,
/// so neither a child that never reads its input nor a chatty child can block the harness.
fn wait_with_timeout(mut child: Child, input: Vec<u8>, timeout: Duration) -> Result<Output, String> {
    let start = Instant::now();
    if let Some(mut stdin) = child.stdin.take() {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        }); // the pipe closes when the writer finishes
    }
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    let out_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out.read_to_end(&mut v);
        v
    });
    let err_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err.read_to_end(&mut v);
        v
    });
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

fn run_test(path: &Path) -> Outcome {
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
    if !header.requires.is_empty() {
        return Outcome::Fail("`requires:` needs engine_available (roadmap 1.1)".into());
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

#[test]
fn v2_files() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut tests = Vec::new();
    discover(&root, &mut tests);
    let (mut passed, mut failed, mut skipped) = (0, Vec::new(), Vec::new());
    for path in &tests {
        let shown = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        match run_test(path) {
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
