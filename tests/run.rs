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
/// inherited `V2_ENGINE_*` overrides, nor a developer's activated venv (`VIRTUAL_ENV`, which
/// doctor's notes and an ImportError's hint read): a test that wants one sets it.
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
    cmd.env_remove("VIRTUAL_ENV");
    // An activated venv's bin (Scripts) folder on the developer's PATH changes which Python the
    // runs find, and so their hints: left out (the second review of round 19), worked out once
    // per test process, as WSL's PATH has dozens of /mnt/c folders to look at. A test that wants
    // one sets PATH itself.
    static PATH_WITHOUT_VENVS: std::sync::OnceLock<Option<std::ffi::OsString>> = std::sync::OnceLock::new();
    let path = PATH_WITHOUT_VENVS.get_or_init(|| {
        let path = std::env::var_os("PATH")?;
        let all: Vec<PathBuf> = std::env::split_paths(&path).collect();
        let kept: Vec<&PathBuf> = all.iter().filter(|d| !d.parent().is_some_and(|p| p.join("pyvenv.cfg").is_file())).collect();
        if kept.len() < all.len() { std::env::join_paths(kept).ok() } else { None }
    });
    if let Some(path) = path {
        cmd.env("PATH", path);
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
        (&["run"][..], "there is no 'run' command: to run a script, type: v2 FILE"),
        (&["check"][..], "needs a FILE"),
        (&["no_such_script.v2"][..], "cannot find 'no_such_script.v2' in "),
    ] {
        let out = v2(args);
        assert_eq!(out.status.code(), Some(2), "args {args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(needle), "args {args:?}: stderr {err:?} lacks {needle:?}");
    }
}

#[test]
fn first_run_mistakes_say_what_to_do_next() {
    // INTERNALS 19.1: a FILE that is missing or is a folder ends with what to do next (exit 2),
    // and a JavaScript file given to V2 gets a note after its error.
    let files = [("hello.v2", "print(1)\n"), ("notes.v2.txt", "print(2)\n"), ("proj/main.v2", "print(3)\n"), ("app.js", "console.log(\"hi\")\n")];
    let sep = std::path::MAIN_SEPARATOR;
    let cases: Vec<(Vec<&str>, i32, String)> = vec![
        (vec!["hello"], 2, "did you mean 'hello.v2'?".into()),
        (vec!["notes.v2"], 2, "there is a 'notes.v2.txt': your editor added '.txt', so rename it to 'notes.v2'".into()),
        (vec!["helo.v2"], 2, "did you mean 'hello.v2'?".into()),
        (vec!["nothing.v2"], 2, "the .v2 files there: hello.v2".into()),
        (vec!["run", "hello.v2"], 2, "there is no 'run' command: to run hello.v2, type: v2 hello.v2".into()),
        (vec!["help"], 2, "for help, type: v2 --help".into()),
        (vec!["check", "hello"], 2, "did you mean 'hello.v2'?".into()),
        (vec!["proj"], 2, format!("'proj' is a folder, not a script\nrun one of its .v2 files, for example: v2 proj{sep}main.v2")),
        (
            vec!["app.js"],
            1,
            "NameError: 'console' is not declared".into(),
        ),
        (
            vec!["app.js"],
            1,
            "note: 'app.js' is a JavaScript file, and V2 runs .v2 scripts: put JavaScript code in a @js { } block in a .v2 file\n".into(),
        ),
    ];
    for (args, code, needle) in cases {
        let (got, stdout, stderr, _home) = run_with_files(&files, &args);
        assert_eq!(got, Some(code), "{args:?}: {stderr}");
        assert!(stdout.is_empty() && stderr.contains(&needle), "{args:?}: stderr {stderr:?} lacks {needle:?}");
    }
    // The first line names the folder it looked in.
    let (_, _, stderr, home) = run_with_files(&files, &["nothing.v2"]);
    let folder = std::fs::canonicalize(&home.0).unwrap();
    let folder = folder.to_string_lossy();
    let folder = folder.strip_prefix(r"\\?\").unwrap_or(&folder);
    assert!(stderr.starts_with(&format!("v2: cannot find 'nothing.v2' in {folder}\n")), "{stderr}");
    // In a folder that is there: the file's own name in it, not the path given once more.
    std::fs::create_dir_all(home.0.join("sub")).unwrap();
    let (_, _, stderr) = run_in_home(&home.0, &["sub/x.v2"], &[]);
    let sub = std::fs::canonicalize(home.0.join("sub")).unwrap();
    let sub = sub.to_string_lossy();
    let sub = sub.strip_prefix(r"\\?\").unwrap_or(&sub);
    assert!(stderr.starts_with(&format!("v2: cannot find 'x.v2' in {sub}\n")), "{stderr}");
    // A symbolic link to a file that isn't there says so (19.1).
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/nonexistent/real.v2", home.0.join("dangling.v2")).unwrap();
        let (code, _, stderr) = run_in_home(&home.0, &["dangling.v2"], &[]);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("\n'dangling.v2' is a link to /nonexistent/real.v2, which isn't there: remove the link (rm dangling.v2), or make /nonexistent/real.v2\n"), "{stderr}");
        // A relative target is from the link's own folder, shown absolute (the second review).
        std::os::unix::fs::symlink("gone.v2", home.0.join("sub").join("rel.v2")).unwrap();
        let (code, _, stderr) = run_in_home(&home.0, &["sub/rel.v2"], &[]);
        assert_eq!(code, Some(2), "{stderr}");
        let gone = format!("{sub}/gone.v2");
        assert!(stderr.contains(&format!("\n'rel.v2' is a link to {gone}, which isn't there: remove the link (rm sub/rel.v2), or make {gone}\n")), "{stderr}");
    }
}

#[test]
fn cross_language_mistakes_say_what_to_do() {
    // The hints of INTERNALS 2.4 that only an uncaught error prints, and the messages of 11.7 and
    // 13.3 for what a newcomer tries first.
    if !needs("py", "cross_language_mistakes_say_what_to_do") || !needs("js", "cross_language_mistakes_say_what_to_do") {
        return;
    }
    let cases = [
        ("var.v2", "let n = 10\nlet h = @js { n * 2 }\n", "  hint: 'n' is a V2 variable, and blocks don't see V2 variables: pass it in, @js(n) { ... }\n"),
        ("loop.v2", "for i in 0..2 {\n    print(@py { i * 100 })\n}\n", "  hint: 'i' is a V2 variable, and blocks don't see V2 variables: pass it in, @py(i) { ... }\n"),
        // Round 16: a V2 func, and a name the file imported, each get the way in that fits.
        ("func.v2", "func helper() {\n    return 1\n}\nprint(@py { helper() })\n", "  hint: 'helper' is a V2 function, and blocks don't see V2's names: import it in the block, @import { helper } from v2, or pass it in, @py(helper) { ... }\n"),
        ("imported.v2", "@import { mean } from py.statistics\nprint(@js { mean([1, 2]) })\n", "  hint: 'mean' is a name V2 imported, and blocks don't see V2's names: import it in the block, as its language does, or pass it in, @js(mean) { ... }\n"),
        (
            "nomod.v2",
            "@import { thing } from py.no_such_module_xyz\nprint(thing())\n",
            "  hint: if 'no_such_module_xyz' isn't misspelled, install it with pip for the Python that v2 doctor shows; README.md, \"Python and npm packages\", has the steps\n",
        ),
        ("nonpm.v2", "@import { chunk } from js.no_such_package_xyz\nprint(chunk([1], 1))\n", "  hint: if 'no_such_package_xyz' isn't misspelled, install it with npm install no_such_package_xyz in "),
        ("noexport.v2", "@py tools {\n    def a():\n        return 1\n}\n@import { a } from tools\nprint(a())\n", "ImportError: block 'tools' exports nothing: add @export { a } inside it\n"),
        (
            "static.v2",
            "@py k {\n    class C:\n        @staticmethod\n        def hello():\n            return 1\n    @export { C }\n}\n@import { C } from k\nprint(C.hello())\n",
            "an imported class is only a function here; @export a function from the block that uses C.hello\n",
        ),
        ("big.v2", "let b = @py { 2 ** 100 }\nprint(b + 1)\n", "this integer needs more than 64 bits, which V2's int doesn't have: keep the arithmetic in the block\n"),
    ];
    for (name, src, needle) in cases {
        let (code, _, stderr, _home) = run_with_files(&[(name, src)], &[name]);
        assert_eq!(code, Some(1), "{name}: {stderr}");
        assert!(stderr.contains(needle), "{name}: stderr {stderr:?} lacks {needle:?}");
    }
    // A caught error has no hint in its message.
    let src = "let n = 10\ntry {\n    @py { n }\n} catch (e) {\n    print(e.message)\n}\n";
    let (code, stdout, stderr, _home) = run_with_files(&[("caught.v2", src)], &["caught.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "name 'n' is not defined\n"), "{stderr}");
}

#[test]
fn a_closed_stdout_stops_the_run_quietly() {
    // INTERNALS 6.5: when stdout's reader goes away (`v2 x.v2 | head`), the run stops at once
    // with 141 and prints nothing more: V2's own loop, and a block that prints forever.
    let mut programs = vec![("loop.v2", "for i in 0..100000000 {\n    print(i)\n}\n")];
    if needs("py", "a_closed_stdout_stops_the_run_quietly") {
        programs.push(("pyloop.v2", "@py {\n    i = 0\n    while True:\n        print(i)\n        i += 1\n}\n"));
    }
    for (name, src) in programs {
        let home = TempHome::new();
        std::fs::write(home.0.join(name), src).unwrap();
        let mut child = v2_command(&home.0).arg(name).current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let mut first = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(child.stdout.take().unwrap()), &mut first).unwrap();
        assert_eq!(first, "0\n", "{name}");
        // The reader is dropped with the BufReader: the pipe is closed.
        let start = Instant::now();
        let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(20)).unwrap();
        assert_eq!(out.status.code(), Some(141), "{name}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(out.stderr.is_empty(), "{name}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(start.elapsed() < Duration::from_secs(10), "{name} stopped only after {:?}", start.elapsed());
    }
}

#[test]
fn a_utf16_file_says_to_save_it_as_utf8() {
    // Windows PowerShell 5's `>` writes UTF-16 with a byte-order mark (INTERNALS 2.1).
    let home = TempHome::new();
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend("print(1)\n".encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(home.0.join("u16.v2"), bytes).unwrap();
    let out = v2_command(&home.0).arg("u16.v2").current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "u16.v2:1:1: SyntaxError: file is not valid UTF-8: it is UTF-16 (what Windows PowerShell's > and Out-File write); save it as UTF-8\n"
    );
    // Without the byte-order mark, and big-endian: the same (some editors save it so).
    for le in [true, false] {
        let bytes: Vec<u8> = "print(1)
".encode_utf16().flat_map(|u| if le { u.to_le_bytes() } else { u.to_be_bytes() }).collect();
        std::fs::write(home.0.join("u16.v2"), bytes).unwrap();
        let out = v2_command(&home.0).arg("u16.v2").current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stderr).starts_with("u16.v2:1:1: SyntaxError: file is not valid UTF-8: it is UTF-16"), "{le}: {}", String::from_utf8_lossy(&out.stderr));
    }
    // A program or an image given to V2 is named a binary file (a NUL byte in its start).
    std::fs::write(home.0.join("app.v2"), b"MZ\x90\x00\x03\x00\xff\xff").unwrap();
    let out = v2_command(&home.0).arg("app.v2").current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    // At the first bad byte, after `MZ` (INTERNALS 3.1).
    assert_eq!(String::from_utf8_lossy(&out.stderr), "app.v2:1:3: SyntaxError: file is not valid UTF-8: it looks like a binary file, not a V2 script\n");
}

#[test]
fn a_utf32_file_is_named_utf32() {
    // Round 19: Out-File -Encoding utf32 writes UTF-32, whose little-endian byte-order mark
    // starts with UTF-16's: a script, v2 check, an import and a v2.toml say UTF-32 (INTERNALS
    // 2.1, 18.5).
    let utf32 = |text: &str, le: bool| -> Vec<u8> {
        let mut bytes = if le { vec![0xFF, 0xFE, 0, 0] } else { vec![0, 0, 0xFE, 0xFF] };
        bytes.extend(text.chars().flat_map(|c| if le { (c as u32).to_le_bytes() } else { (c as u32).to_be_bytes() }));
        bytes
    };
    let said = "SyntaxError: file is not valid UTF-8: it is UTF-32 (what Out-File -Encoding utf32 writes); save it as UTF-8\n";
    for le in [true, false] {
        let home = TempHome::new();
        std::fs::write(home.0.join("u32.v2"), utf32("print(\"héllo\")\n", le)).unwrap();
        for args in [&["u32.v2"][..], &["check", "u32.v2"]] {
            let (code, _, stderr) = run_in_home(&home.0, args, &[]);
            assert_eq!(code, Some(1), "{le} {args:?}: {stderr}");
            // At the first bad byte: the mark's 0xFF, or big-endian its 0xFE after two NULs.
            let at = if le { "u32.v2:1:1: " } else { "u32.v2:1:3: " };
            assert!(stderr.starts_with(&format!("{at}{said}")), "{le} {args:?}: {stderr}");
        }
        std::fs::write(home.0.join("main.v2"), "import \"./lib32.v2\" as l\n").unwrap();
        std::fs::write(home.0.join("lib32.v2"), utf32("let x = 1\n", le)).unwrap();
        let (code, _, stderr) = run_in_home(&home.0, &["main.v2"], &[]);
        assert_eq!(code, Some(1), "{le}: {stderr}");
        assert!(stderr.contains(said), "{le}: {stderr}");
        std::fs::write(home.0.join("v2.toml"), utf32("[python]\n", le)).unwrap();
        std::fs::write(home.0.join("ok.v2"), "print(1)\n").unwrap();
        let (code, _, stderr) = run_in_home(&home.0, &["ok.v2"], &[]);
        assert_eq!(code, Some(2), "{le}: {stderr}");
        assert!(stderr.contains("v2.toml: config error: the file is not valid UTF-8: it is UTF-32 (what Out-File -Encoding utf32 writes); save it as UTF-8"), "{le}: {stderr}");
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
fn an_npm_tool_not_on_path_says_to_run_it_with_npx() {
    // prettier installed for the project lives in node_modules/.bin, which PATH doesn't name.
    let tool = if cfg!(windows) { "node_modules/.bin/v2-test-tool.cmd" } else { "node_modules/.bin/v2-test-tool" };
    let (code, _, stderr, _home) = run_with_files(&[(tool, ""), ("t.v2", "exec([\"v2-test-tool\", \".\"])
")], &["t.v2"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("hint: v2-test-tool is in node_modules/.bin, where npm puts a package's programs: run it with npx, as in exec([\"npx\", \"v2-test-tool\", …])"), "{stderr}");
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
        // The bad byte's line and column: `print("` holds 7 characters.
        let (line, col) = if bytes.contains(&b'\n') { (2, 8) } else { (3, 1) };
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            format!("bad.v2:{line}:{col}: SyntaxError: {OLDER}\n")
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
    // A file whose parse stops at a syntax error: the summary says the rest wasn't checked.
    let broken = [("main.v2", "let x = (1\nlet y = ]\n")];
    let (code, stdout, stderr) = run_in_project(&broken, ".", &["check", "main.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(1), "check main.v2: 1 file, 1 error (a file with a syntax error shows only that error: fix it, then check again)\n"), "{stderr}");
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
    // A folder: one of its .v2 files is what to write (6.4).
    assert_eq!(lines[1], "cannot import './dir': it is a folder, and import loads a .v2 file, as in import \"./dir/keep.v2\" as keep");
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
show(func() => exec("git  status --short"))
// A program's path stays one item, even with a space in it.
try { exec(v2) } catch (e) { print(e.message.ends_with("as in exec([" + json_stringify(v2) + "]): exec runs a program, without a shell")) }
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
            "TypeError: exec() argument 'argv' must be a list, got str — as in exec([\"git\", \"status\", \"--short\"]): exec runs a program, without a shell\n",
            "true\n",
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
             IOError: exec: cannot start '.\\p%OS%q\\x.cmd': a batch file whose path contains '%' can't be started safely: move it to a folder whose path has no %, or start the program it runs directly\n"
        )
    );
}

#[cfg(windows)]
#[test]
fn exec_arguments_reach_msys_programs_and_c_runtimes_as_given() {
    // DOCS 3.11, INTERNALS 8.3 (round 18): Git Bash's MSYS programs (printf, sh) glob, expand
    // braces and ~, read @file and take ' as a quote in a word they see unquoted; v2 and Python
    // read their command line by the C runtime's rules. Each gets every argument as given. A
    // batch file keeps Rust's quoting for cmd.exe (*.v2 and @data.json unquoted).
    let cases = [
        "q\"uote", "it's", "*.v2", "@data.json", "{a,b}.v2", "~", r"a\b", r#"a\\"b"#, r"trailing\", "", "two words", "a\ttab", "Čeština", "%PATH%",
        r"\\server\share\My Docs", r"C:\some dir\", "-name", "*.json",
    ];
    let git = std::path::Path::new(r"C:\Program Files\Git\usr\bin");
    let main = r#"let cases = json_parse(read_file("cases.json"))
func same(name, argv) {
    try {
        let r = exec(argv + cases)
        let got = json_parse(r.stdout)
        print(name, got == cases ? "ok" : r.stdout.trim())
    } catch (e) {
        print(name, "skip", e.message)
    }
}
same("v2", [args()[0], "child.v2"])
same("python", ["python", "-c", "import sys, json; print(json.dumps(sys.argv[1:]))"])
let git = args()[1]
if exists(git + "/printf.exe") {
    let want = "[" + cases.join("][") + "]"
    let p = exec([git + "/printf.exe", "[%s]"] + cases).stdout
    print("printf", p == want ? "ok" : p)
    let s = exec([git + "/sh.exe", "-c", "printf '[%s]' \"$@\"", "sh"] + cases).stdout
    print("sh", s == want ? "ok" : s)
    let found = exec([git + "/find.exe", ".", "-maxdepth", "1", "-name", "*.v2"])
    print(found.code, len(found.stdout.trim().lines()))
}
print(exec(["./show.bat"] + cases).stdout.trim())
"#;
    let json = serde_json::to_string(&cases).unwrap();
    // The batch file hands its %* to v2, as npm.cmd hands it to node (said in ASCII: echo's code
    // page isn't UTF-8), then shows each argument as it got it (%1) and as %~1 reads it.
    let bat = format!("@echo off\r\n\"{BIN}\" same.v2 %*\r\n:next\r\nif [%1]==[] goto :eof\r\necho %1 [%~1]\r\nshift\r\ngoto next\r\n");
    let same = "print(args() == json_parse(read_file(\"cases.json\")) ? \"same\" : json_stringify(args()))\n";
    let files = [
        ("main.v2", main),
        ("child.v2", "print(json_stringify(args()))\n"),
        ("cases.json", json.as_str()),
        ("a.v2", ""),
        ("b.v2", ""),
        ("data.json", "{\"id\": 1}\n"),
        ("show.bat", bat.as_str()),
        ("same.v2", same),
    ];
    let (code, stdout, stderr, _home) = run_with_files(&files, &["main.v2", BIN, &git.to_string_lossy()]);
    assert_eq!(code, Some(0), "{stderr}");
    let mut lines = stdout.lines();
    assert_eq!(lines.next(), Some("v2 ok"), "{stdout}");
    let python = lines.next().unwrap_or_default();
    assert!(python == "python ok" || python.starts_with("python skip"), "{stdout}");
    if git.join("printf.exe").is_file() {
        assert_eq!(lines.next(), Some("printf ok"), "{stdout}");
        assert_eq!(lines.next(), Some("sh ok"), "{stdout}");
        // find's own pattern, not the shell's: a.v2, b.v2, child.v2, main.v2 and same.v2, not
        // "paths must precede expression".
        assert_eq!(lines.next(), Some("0 5"), "{stdout}");
    } else {
        let _ = writeln!(std::io::stderr(), "SKIP the MSYS part of exec_arguments_reach_msys_programs_and_c_runtimes_as_given (no Git for Windows)");
    }
    // The batch file: Rust's quoting for cmd.exe. A word of letters, digits and # $ * + - . / : ?
    // @ \ _ (or other than ASCII) goes as it is, any other in quotes with " as "", %PATH% stays
    // unexpanded, and the backslashes before a quote or the closing one are doubled.
    assert_eq!(lines.next(), Some("same"), "{stdout}");
    let shown = [
        r#""q""uote" [q""uote]"#,
        r#""it's" [it's]"#,
        "*.v2 [*.v2]",
        "@data.json [@data.json]",
        r#""{a,b}.v2" [{a,b}.v2]"#,
        r#""~" [~]"#,
        r"a\b [a\b]",
        r#""a\\\\""b" [a\\\\""b]"#,
        r#""trailing\\" [trailing\\]"#,
        r#""" []"#,
        r#""two words" [two words]"#,
        "\"a\ttab\" [a\ttab]",
        "Čeština [Čeština]",
        r#""%PATH%" [%PATH%]"#,
        r#""\\server\share\My Docs" [\\server\share\My Docs]"#,
        r#""C:\some dir\\" [C:\some dir\\]"#,
        "-name [-name]",
        "*.json [*.json]",
    ];
    let got: Vec<&str> = lines.collect();
    assert_eq!(got.len(), shown.len(), "{stdout}");
    for (got, want) in got.iter().zip(shown) {
        // echo writes in the console's code page, which may have no Č: one character for each.
        if want.is_ascii() {
            assert_eq!(*got, want, "{stdout}");
        } else {
            assert_eq!(got.chars().count(), want.chars().count(), "{stdout}");
        }
    }
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
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "IOError: cannot list 'names': the name 'bad\u{FFFD}' is not valid UTF-8 (\u{FFFD} marks what isn't), and V2's strings are: rename it\ntrue false true\n"
    );
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
fn writing_to_a_failing_stdout_is_an_io_error() {
    // INTERNALS 21 (Host) and 5.10: a failed write (closed pipe; Rust ignores SIGPIPE) raises
    // IOError at the print.
    let home = TempHome::new();
    let Some(stdout) = failing_stdout(&home.0) else { return };
    std::fs::write(home.0.join("spam.v2"), "while true { print(\"x\" * 1000) }\n").unwrap();
    let child = v2_command(&home.0)
        .arg("spam.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
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

#[test]
fn a_script_named_like_a_flag_is_shown_how_to_run() {
    // INTERNALS 19.1: `v2 -dash.v2`, also as PowerShell passes it (`-dash` `.v2`).
    let home = TempHome::new();
    std::fs::write(home.0.join("-dash.v2"), "print(1)\n").unwrap();
    for args in [&["-dash.v2"][..], &["-dash", ".v2"]] {
        let (code, _, stderr) = run_in_home(&home.0, args, &[]);
        assert_eq!(code, Some(2), "{stderr}");
        assert!(stderr.contains("\nto run the file -dash.v2, write: v2 ./-dash.v2\nrun `v2 --help` for usage\n"), "{args:?}: {stderr}");
    }
    let (code, stdout, stderr) = run_in_home(&home.0, &["./-dash.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(0), "1\n"), "{stderr}");
    // `v2 check -dash.v2` as PowerShell splits it.
    let (code, _, stderr) = run_in_home(&home.0, &["check", "-dash", ".v2"], &[]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("\nto check the file -dash.v2, write: v2 check ./-dash.v2\nrun `v2 --help` for usage\n"), "{stderr}");
}

#[test]
fn a_python_that_cant_start_names_pythonhome() {
    // INTERNALS 8.4, 19.2: --version runs, but Python can't start with a PYTHONHOME that isn't
    // there; the crash names the variable, and doctor, which sees `python -c pass` fail (round
    // 18), shows py broken with what Python said and the fix.
    if !needs("py", "a_python_that_cant_start_names_pythonhome") {
        return;
    }
    let home = TempHome::new();
    // Short, so no 80-character cut; relative, so there is no such folder from the cwd.
    let nowhere = std::ffi::OsStr::new("no-python-home");
    std::fs::write(home.0.join("start.v2"), "@py { 1 }\n").unwrap();
    let env = [("PYTHONHOME", nowhere)];
    let (code, _, stderr) = run_in_home(&home.0, &["start.v2"], &env);
    assert_eq!(code, Some(1), "{stderr}");
    // Round 19: what to do, as doctor's fix line says.
    let named = "; PYTHONHOME is set (no-python-home), which it reads as it starts: unset it or fix it\n";
    assert!(stderr.contains("EngineCrashed: engine 'py' exited during startup") && stderr.contains(named), "{stderr}");
    let (code, stdout, _) = run_in_home(&home.0, &["doctor"], &env);
    assert_eq!((code, doctor_rows(&stdout)["py"][0].as_str()), (Some(0), "broken"), "{stdout}");
    assert!(stdout.contains(" can't import the standard modules V2's worker uses (exit code ") && stdout.contains("): Fatal Python error: "), "{stdout}");
    let fix = "\n  py: fix: python didn't start: PYTHONHOME is set (no-python-home), which it reads as it starts: unset it or fix it\n";
    assert!(stdout.contains(fix) && !stdout.contains("which Python reads as it starts"), "{stdout}");
    assert!(stdout.lines().any(|l| l.starts_with("not ready: py")) && !stdout.lines().any(|l| l.starts_with("ready:") && l.contains("py")), "{stdout}");
    // One Python starts with: ok, and the note.
    let (_, stdout, _) = run_in_home(&home.0, &["doctor"], &[("PYTHONPATH", std::ffi::OsStr::new("."))]);
    assert_eq!(doctor_rows(&stdout)["py"][0], "ok", "{stdout}");
    assert!(stdout.contains("\n  py: PYTHONPATH is set (.), which Python reads as it starts\n"), "{stdout}");
}

#[test]
fn a_stale_pythonpath_that_breaks_the_workers_imports_is_broken_in_doctor() {
    // Round 19, 19.2: a PYTHONPATH left from another Python passes `python -c pass` but breaks
    // `re`, which the worker's json imports as it starts: doctor imports what the worker imports,
    // so py is broken, with Python's own line (not a source line of its traceback) and the fix.
    if !needs("py", "a_stale_pythonpath_that_breaks_the_workers_imports_is_broken_in_doctor") {
        return;
    }
    let home = TempHome::new();
    std::fs::create_dir_all(home.0.join("stale")).unwrap();
    std::fs::write(home.0.join("stale").join("re.py"), "raise ImportError('a re from another Python')\n").unwrap();
    std::fs::write(home.0.join("start.v2"), "@py { 1 }\n").unwrap();
    // Relative, as the run and doctor start Python in this folder: short, so no 80-character cut.
    let env = [("PYTHONPATH", std::ffi::OsStr::new("stale"))];
    let (code, stdout, _) = run_in_home(&home.0, &["doctor"], &env);
    assert_eq!((code, doctor_rows(&stdout)["py"][0].as_str()), (Some(0), "broken"), "{stdout}");
    assert!(stdout.contains(" can't import the standard modules V2's worker uses (exit code 1): ImportError: a re from another Python\n"), "{stdout}");
    assert!(stdout.contains("\n  py: fix: python didn't start: PYTHONPATH is set (stale), which it reads as it starts: unset it or fix it\n"), "{stdout}");
    let (code, _, stderr) = run_in_home(&home.0, &["start.v2"], &env);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("ImportError: a re from another Python"), "Python's own traceback comes first: {stderr}");
    assert!(stderr.contains("; PYTHONPATH is set (stale), which it reads as it starts: unset it or fix it\n"), "{stderr}");
}

#[test]
fn a_node_options_that_stops_node_is_named() {
    // Round 18, INTERNALS 7.8, 19.2: a bad option in NODE_OPTIONS stops `node --version`, and a
    // preload that isn't there stops `node -e 0`: doctor and the run name the variable, not a
    // reinstall of Node.js.
    if !needs("js", "a_node_options_that_stops_node_is_named") {
        return;
    }
    let home = TempHome::new();
    std::fs::write(home.0.join("j.v2"), "print(@js { 1 + 1 })\n").unwrap();
    let fix = "fix: node didn't start: NODE_OPTIONS is set (--bogus-flag), which it reads as it starts: unset it or fix it";
    let bogus = [("NODE_OPTIONS", std::ffi::OsStr::new("--bogus-flag"))];
    let (code, stdout, _) = run_in_home(&home.0, &["doctor"], &bogus);
    let rows = doctor_rows(&stdout);
    assert_eq!((code, rows["js"][0].as_str(), rows["ts"][0].as_str()), (Some(0), "broken", "broken"), "{stdout}");
    assert!(stdout.contains(&format!("\n  js: {fix}\n")) && !stdout.contains("repair that Node.js"), "{stdout}");
    assert!(stdout.contains("\n  ts: @ts blocks run on the js engine: see js above\n"), "{stdout}");
    // The run, with no probe cached: EngineNotFound, the fix before the install line.
    let (code, _, stderr) = run_in_home(&home.0, &["j.v2"], &bogus);
    assert_eq!(code, Some(3), "{stderr}");
    let at = stderr.find(&format!("\n    {fix}\n")).unwrap_or_else(|| panic!("{stderr}"));
    assert!(stderr[at..].contains("\n    install: "), "{stderr}");
    // A preload that isn't there: --version answers, `node -e 0` doesn't.
    let preload = [("NODE_OPTIONS", std::ffi::OsStr::new("--require ./missing.js"))];
    let (_, stdout, _) = run_in_home(&home.0, &["doctor"], &preload);
    let rows = doctor_rows(&stdout);
    assert_eq!((rows["js"][0].as_str(), rows["ts"][0].as_str()), ("broken", "broken"), "{stdout}");
    assert!(stdout.contains(" -e 0 failed (exit code 1): Error: Cannot find module"), "{stdout}");
    let named = "\n  js: fix: node didn't start: NODE_OPTIONS is set (--require ./missing.js), which it reads as it starts: unset it or fix it\n";
    assert!(stdout.contains(named), "{stdout}");
    // One that does no harm: ok, with the note.
    let (_, stdout, _) = run_in_home(&home.0, &["doctor"], &[("NODE_OPTIONS", std::ffi::OsStr::new("--max-old-space-size=512"))]);
    assert_eq!(doctor_rows(&stdout)["js"][0], "ok", "{stdout}");
    assert!(stdout.contains("\n  js: NODE_OPTIONS is set (--max-old-space-size=512), which Node.js reads as it starts\n"), "{stdout}");
}

#[test]
fn a_v2_module_of_another_project_is_told_which_project_the_run_uses() {
    // Round 18, INTERNALS 13.5, 18.1: one project serves a run, so a .v2 file of another project
    // (its own v2.toml) doesn't find that project's own Python modules; the hint says so, rather
    // than asking for the v2.toml that is already there.
    if !needs("py", "a_v2_module_of_another_project_is_told_which_project_the_run_uses") {
        return;
    }
    let files = [
        ("mono/v2.toml", ""),
        ("mono/zzroot_helper.py", "def root_help():\n    return 'mono root helper'\n"),
        ("mono/scripts/sub/rootlib.v2", "@import { root_help } from py.zzroot_helper\nfunc call_root() {\n    return root_help()\n}\n"),
        ("mono/scripts/rootuse.v2", "import \"./sub/rootlib.v2\" as r\nprint(r.call_root())\n"),
        ("glue/v2.toml", ""),
        ("glue/cross.v2", "import \"../mono/scripts/sub/rootlib.v2\" as r\nprint(r.call_root())\n"),
        // The review of round 18: a module that project lacks too, and a NAME.py above that no
        // project puts on Python's path.
        ("mono/lib/third.v2", "@import { get } from py.zz_not_installed_anywhere\nfunc call() {\n    return get()\n}\n"),
        ("glue/third.v2", "import \"../mono/lib/third.v2\" as t\nprint(t.call())\n"),
        ("mono/scripts/zzabove.py", "def up():\n    return 1\n"),
        ("mono/scripts/sub/above.v2", "@import { up } from py.zzabove\nfunc call() {\n    return up()\n}\n"),
        ("glue/above.v2", "import \"../mono/scripts/sub/above.v2\" as a\nprint(a.call())\n"),
    ];
    let (code, stdout, stderr, home) = run_with_files(&files, &["mono/scripts/rootuse.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "mono root helper\n"), "{stderr}");
    let (code, _, stderr) = run_in_home(&home.0, &["glue/cross.v2"], &[]);
    assert_eq!(code, Some(1), "{stderr}");
    let canon = |p: PathBuf| {
        let c = std::fs::canonicalize(p).unwrap().to_string_lossy().into_owned();
        c.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(c)
    };
    let hint = format!(
        "\n  hint: mono/scripts/sub/rootlib.v2 belongs to another project ({}, with its own v2.toml), but a run uses the entry script's project ({}): run it from that project, or put the module where this one finds it\n",
        canon(home.0.join("mono")),
        canon(home.0.join("glue"))
    );
    assert!(stderr.contains("ImportError: No module named 'zzroot_helper'") && stderr.contains(&hint), "{stderr}");
    // mono has no such module: the install line, as for any package.
    let (code, _, stderr) = run_in_home(&home.0, &["glue/third.v2"], &[]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("\n  hint: if 'zz_not_installed_anywhere' isn't misspelled, install it") && !stderr.contains("another project"), "{stderr}");
    // mono/scripts isn't on Python's path in either project: the file is named, not pip-installed.
    let (code, _, stderr) = run_in_home(&home.0, &["glue/above.v2"], &[]);
    assert_eq!(code, Some(1), "{stderr}");
    let above = format!(
        "\n  hint: there is a {} in a folder above, which isn't on Python's path: move it next to the .v2 file or into the project folder, or put its folder on sys.path in a @py block first\n",
        canon(home.0.join("mono").join("scripts").join("zzabove.py"))
    );
    assert!(stderr.contains(&above), "{stderr}");
}

#[cfg(unix)]
#[test]
fn a_python_module_linked_into_another_folder_is_the_module_python_has() {
    // Round 18, INTERNALS 13.5: a helpers.py that links to the module Python loaded is that
    // module, not another of the same name, whichever folder loads it first.
    if !needs("py", "a_python_module_linked_into_another_folder_is_the_module_python_has") {
        return;
    }
    let home = TempHome::new();
    let files = [
        ("real/helpers.py", "def helper_name():\n    return 'real helper'\n"),
        ("real/once.v2", "@import { helper_name } from py.helpers\nfunc who() {\n    return helper_name()\n}\n"),
        ("app/l.v2", "import \"../real/once.v2\" as a\n@import { helper_name } from py.helpers\nprint(a.who(), helper_name())\n"),
        ("app/r.v2", "@import { helper_name } from py.helpers\nprint(helper_name())\nimport \"../real/once.v2\" as a\nprint(a.who())\n"),
    ];
    for (path, text) in files {
        let p = home.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    std::os::unix::fs::symlink(home.0.join("real/helpers.py"), home.0.join("app/helpers.py")).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["app/l.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(0), "real helper real helper\n"), "{stderr}");
    let (code, stdout, stderr) = run_in_home(&home.0, &["app/r.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(0), "real helper\nreal helper\n"), "{stderr}");
}

#[cfg(windows)]
#[test]
fn exec_output_in_the_console_code_page_is_decoded() {
    // INTERNALS 5.10: on Windows, a child's output that isn't UTF-8 is in the console's code page,
    // the OEM one when there is no console (as here: v2 starts detached), not U+FFFD.
    use std::os::windows::process::CommandExt;
    if !needs("py", "exec_output_in_the_console_code_page_is_decoded") {
        return;
    }
    let python = block_python();
    let home = TempHome::new();
    let src = "let r = exec([args()[0], \"-c\", \"import sys; sys.stdout.buffer.write(bytes([0x9a]))\"])\nprint(len(r.stdout), r.stdout != \"\\u{fffd}\")\n";
    std::fs::write(home.0.join("oem.v2"), src).unwrap();
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let out = v2_command(&home.0).args(["oem.v2", &python]).current_dir(&home.0).stdin(Stdio::null()).creation_flags(DETACHED_PROCESS).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1 true\n", "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_python_program_run_with_exec_writes_utf8() {
    // INTERNALS 5.10: exec's children get PYTHONIOENCODING=utf-8 unless it is set, so a Python
    // program's text arrives whole (on Windows its pipes would be in the ANSI code page: café as
    // cafÚ, and CJK a UnicodeEncodeError); a value the user set is kept.
    if !needs("py", "a_python_program_run_with_exec_writes_utf8") {
        return;
    }
    let python = block_python();
    let home = TempHome::new();
    // Python's own escapes, so the program text is ASCII.
    let src = "let r = exec([args()[0], \"-c\", \"print('caf\\\\u00e9 \\\\u65e5\\\\u672c')\"])\nprint(r.code, r.stdout.trim())\n";
    std::fs::write(home.0.join("u.v2"), src).unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.env_remove("PYTHONIOENCODING").env_remove("PYTHONUTF8");
    let out = cmd.args(["u.v2", &python]).current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "0 caf\u{e9} \u{65e5}\u{672c}\n", "{}", String::from_utf8_lossy(&out.stderr));
    let out = v2_command(&home.0).args(["u.v2", &python]).env("PYTHONIOENCODING", "ascii:replace").current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "0 caf? ??\n", "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_reader_that_goes_away_stops_a_slow_printer() {
    // INTERNALS 5.10, 6.5: `v2 x.v2 | head -1` on a script that prints and sleeps stops at the
    // next flush that fails, not at the end, so the rest of the script never runs.
    let home = TempHome::new();
    let src = "for i in range(100) {\n    print(i)\n    sleep(0.05)\n}\nwrite_file(\"finished.txt\", \"x\")\n";
    std::fs::write(home.0.join("slow.v2"), src).unwrap();
    let mut child = v2_command(&home.0)
        .arg("slow.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    let start = Instant::now();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    std::io::BufRead::read_line(&mut stdout, &mut line).unwrap();
    drop(stdout); // the reader goes away, as `head -1` does
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    assert_eq!(line, "0\n");
    assert_eq!(out.status.code(), Some(141), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(start.elapsed() < Duration::from_secs(3), "it stopped after {:?}", start.elapsed());
    assert!(!home.0.join("finished.txt").exists(), "the rest of the script ran");
}

#[test]
fn a_blocks_output_reaches_a_pipe_as_it_comes() {
    // INTERNALS 9.9: what a block printed goes out before V2 waits for the worker again, so a
    // file or a pipe gets a long block's progress while it runs.
    if !needs("py", "a_blocks_output_reaches_a_pipe_as_it_comes") {
        return;
    }
    let home = TempHome::new();
    let src = "@py {\n    import time\n    print(\"started\")\n    time.sleep(4)\n    print(\"done\")\n}\n";
    std::fs::write(home.0.join("progress.v2"), src).unwrap();
    let mut child = v2_command(&home.0)
        .arg("progress.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    std::io::BufRead::read_line(&mut stdout, &mut line).unwrap();
    // Timed from the first line, not from the start: a cold CI runner may take seconds to start
    // Python (CI's Windows took 6 s once), but held output would come with the last line.
    let first = Instant::now();
    let mut rest = String::new();
    stdout.read_to_string(&mut rest).unwrap();
    let gap = first.elapsed();
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    assert_eq!((line.as_str(), rest.as_str()), ("started\n", "done\n"));
    assert!(gap > Duration::from_secs(2), "the first line came only {gap:?} before the end; the block sleeps 4 s between them");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_tilde_in_v2_toml_with_no_home_folder_is_an_error() {
    // 18.1: `env -i`, a service without HOME: not a folder named ~ next to v2.toml.
    let home = TempHome::new();
    std::fs::write(home.0.join("v2.toml"), "[python]\nvenv = \"~/venv-home\"\n").unwrap();
    std::fs::write(home.0.join("x.v2"), "print(1)\n").unwrap();
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let child = v2_command(&home.0).arg("x.v2").env_remove(var).current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("start v2");
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains(&format!("config error: [python] venv: '~/venv-home' starts with ~, but {var} isn't set, so there is no home folder for it: write the full path")), "{stderr}");
}

#[test]
fn input_from_a_file_that_isnt_utf8_says_to_convert_the_file() {
    // 5.10: `v2 x.v2 < data.txt` with a file in another encoding: no shell setting helps, so the
    // hint is about the file; and bytes FF FE after the first line are not UTF-16's mark.
    let home = TempHome::new();
    std::fs::write(home.0.join("r.v2"), "while true {\n    let l = input()\n    if l == null {\n        break\n    }\n    print(l)\n}\n").unwrap();
    std::fs::write(home.0.join("data.txt"), b"ok\n\xff\xfe bad\n").unwrap();
    let child = v2_command(&home.0)
        .arg("r.v2")
        .current_dir(&home.0)
        .stdin(std::fs::File::open(home.0.join("data.txt")).unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stdout).as_ref()), (Some(1), "ok\n"), "{stderr}");
    assert!(stderr.contains("IOError: input line 2 is not valid UTF-8: it may be in an older encoding, such as Windows-1252: send UTF-8 text"), "{stderr}");
    assert!(stderr.contains("\n  hint: the file sent as input isn't UTF-8: save it as UTF-8, or read it in a @py block with the code page of the Windows that saved it"), "{stderr}");
    // The same bytes through a pipe (a shell sent them): not the file's hint.
    let child = v2_command(&home.0).arg("r.v2").current_dir(&home.0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("start v2");
    let out = wait_with_timeout(child, b"ok\n\xff\xfe bad\n".to_vec(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("IOError: input line 2 is not valid UTF-8") && !stderr.contains("the file sent as input"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn write_file_to_dev_stdout_keeps_a_redirected_files_output() {
    // 5.10: opening /dev/stdout again would start a redirected file over, or write at its start;
    // through V2's own stdout nothing printed before is lost, and the order holds.
    let home = TempHome::new();
    std::fs::write(home.0.join("w.v2"), "print(\"1\")\nwrite_file(\"/dev/stdout\", \"2\\n\")\nprint(\"3\")\n").unwrap();
    let out_path = home.0.join("out.txt");
    let file = std::fs::File::create(&out_path).unwrap();
    let child = v2_command(&home.0).arg("w.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(file).stderr(Stdio::piped()).spawn().expect("start v2");
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(std::fs::read_to_string(&out_path).unwrap(), "1\n2\n3\n");
}

#[test]
fn a_line_printed_before_a_long_computation_reaches_a_pipe() {
    // INTERNALS 5.10: what `Out` holds for a file or a pipe goes out 100 ms later even when the
    // program prints nothing more, so a CI log or `| tee` shows "started" while V2 computes.
    let home = TempHome::new();
    let src = "print(\"started\")\nlet t = time()\nwhile time() - t < 4 {\n}\nprint(\"done\")\n";
    std::fs::write(home.0.join("busy.v2"), src).unwrap();
    let mut child = v2_command(&home.0)
        .arg("busy.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    let start = Instant::now();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    std::io::BufRead::read_line(&mut stdout, &mut line).unwrap();
    let took = start.elapsed();
    let mut rest = String::new();
    stdout.read_to_string(&mut rest).unwrap();
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    assert_eq!((line.as_str(), rest.as_str()), ("started\n", "done\n"));
    assert!(took < Duration::from_secs(3), "the first line came after {took:?}; the loop runs 4 s");
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
}

#[cfg(target_os = "linux")]
#[test]
fn no_interpreter_thread_is_not_an_internal_error() {
    // Linux only: there RLIMIT_NPROC also counts threads, so `ulimit -u 1` stops the thread.
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
    // The errors it printed turn exit code 0 into 1 (INTERNALS 6.5, 9.9).
    assert_eq!((code, stdout.as_str()), (Some(1), "caught Error\n1\n"));
    // A block's own AggregateError (another realm) shows its members, and itself only once.
    assert!(stderr.contains("\n  RangeError: first\n") && stderr.contains("\n  SyntaxError: second\n"), "{stderr}");
    assert!(stderr.contains("\n  AggregateError: group (shown above)\n") && stderr.len() < 4000, "{stderr}");
    assert_eq!(stderr.matches("caused by: Error: self (shown above)").count(), 1, "{stderr}");
    assert!(stderr.contains("Error: not a group\n") && !stderr.contains("member"), "{stderr}");
    assert!(stderr.contains("caused by: Error: x (shown above)\ncaused by: Error: y\n"), "{stderr}");
    assert!(stderr.contains("caused by: Error: its cause\n"), "{stderr}");
    assert!(stderr.starts_with("uncaught exception: V2Error: ValueError: timer boom\n    at boom (timer.v2:2)\n"), "{stderr}");
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
    // The thread's errors, reported, turn exit code 0 into 1 (INTERNALS 6.5, 9.9).
    assert_eq!((code, stdout.as_str()), (Some(1), "2\n"));
    assert!(stderr.contains("line 14, in <lambda>"), "{stderr}");
    assert!(stderr.contains("V2Error: V2 function 'boom' called from another thread"), "{stderr}");
    assert!(stderr.contains("V2Error: ValueError: from v2"), "{stderr}");
    assert!(!stderr.contains("py.py"), "{stderr}");
    // No caret under a .v2 line (Python 3.13+ underlines library lines, which stay as Python shows them).
    let lines: Vec<&str> = stderr.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if l.contains("thread.v2\", line") {
            assert!(!lines.get(i + 2).is_some_and(|n| n.trim_start().starts_with(['^', '~'])), "{stderr}");
        }
    }
}

#[test]
fn to_v2_of_pandas_and_numpy_dates_names_a_block_that_converts_them() {
    // INTERNALS 11.5, round 17: a datetime Series, a DataFrame and a datetime64 array fail on an
    // item, and the block the message names converts the whole object. The real pandas and numpy
    // (engines/py/to_v2_items.v2 has stand-ins), so it skips where they aren't installed.
    let test = "to_v2_of_pandas_and_numpy_dates_names_a_block_that_converts_them";
    if !needs("py", test) {
        return;
    }
    let src = r#"func show(f) {
    try {
        print(f())
    } catch (e) {
        print(e.type + ": " + e.message)
    }
}
let have = @py {
    import importlib.util
    all(importlib.util.find_spec(m) is not None for m in ("pandas", "numpy"))
}
if have {
    let dates = @py {
        import pandas as pd
        pd.to_datetime(pd.Series(["2026-01-01", "2026-01-02"]))
    }
    show(func() => to_v2(dates))
    show(func() => @py(h: dates) { h.astype(str).tolist() })
    let frame = @py(dates) {
        import pandas as pd
        pd.DataFrame({"d": dates, "n": [1.5, 2]})
    }
    show(func() => to_v2(frame))
    show(func() => @py(h: frame) { import json; json.loads(h.to_json(date_format="iso")) })
    let days = @py { __import__("numpy").array(["2026-01-01"], dtype="datetime64[D]") }
    show(func() => to_v2(days))
    show(func() => @py(h: days) { h.astype(str).tolist() })
} else {
    print("no pandas")
}
"#;
    let (code, stdout, stderr, _home) = run_with_files(&[("dates.v2", src)], &["dates.v2"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    if stdout == "no pandas\n" {
        let _ = writeln!(std::io::stderr(), "SKIP {test} (pandas or numpy not installed)");
        return;
    }
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 6, "{stdout}");
    // pandas names its Timestamp pandas.Timestamp from 3.0, pandas._libs.tslibs.timestamps.Timestamp before.
    let series = ": make plain data of it in a block first, as in @py(h) { h.astype(str).tolist() }";
    assert!(lines[0].starts_with("MarshalError: to_v2(): an item of h has the type pandas.") && lines[0].ends_with(&format!("Timestamp{series}")), "{stdout}");
    assert_eq!(lines[1], r#"["2026-01-01", "2026-01-02"]"#);
    let frame = r#"Timestamp: make plain data of it in a block first, as in @py(h) { import json; json.loads(h.to_json(date_format="iso")) }"#;
    assert!(lines[2].starts_with("MarshalError: to_v2(): an item of h has the type pandas.") && lines[2].ends_with(frame), "{stdout}");
    assert!(lines[3].starts_with(r#"{"d": {"0": "2026-01-01T00:00:00"#) && lines[3].ends_with(r#""n": {"0": 1.5, "1": 2.0}}"#), "{stdout}");
    assert_eq!(lines[4], format!("MarshalError: to_v2(): an item of h has the type datetime.date{series}"));
    assert_eq!(lines[5], r#"["2026-01-01"]"#);
}

#[test]
fn to_v2_of_pandas_indexed_by_dates_names_a_block_that_converts_them() {
    // INTERNALS 11.5, round 17 review: a Series or DataFrame indexed by dates (a time series) fails
    // on a key of what its to_dict() gave. The message names the object, not that dict, and a block
    // that converts it. The real pandas (engines/py/to_v2_items.v2 has stand-ins), so it skips
    // where pandas isn't installed.
    let test = "to_v2_of_pandas_indexed_by_dates_names_a_block_that_converts_them";
    if !needs("py", test) {
        return;
    }
    let src = r#"func show(f) {
    try {
        print(f())
    } catch (e) {
        print(e.type + ": " + e.message)
    }
}
let have = @py { __import__("importlib.util").util.find_spec("pandas") is not None }
if have {
    let series = @py {
        import pandas as pd
        pd.Series([1, 2], index=pd.to_datetime(["2026-01-01", "2026-01-02"]))
    }
    show(func() => to_v2(series))
    show(func() => @py(h: series) { __import__("json").loads(h.to_json(date_format="iso")) })
    let frame = @py(series) {
        import pandas as pd
        pd.DataFrame({"n": series})
    }
    show(func() => to_v2(frame))
    show(func() => @py(h: frame) { __import__("json").loads(h.to_json(date_format="iso")) })
} else {
    print("no pandas")
}
"#;
    let (code, stdout, stderr, _home) = run_with_files(&[("keys.v2", src)], &["keys.v2"]);
    assert_eq!(code, Some(0), "{stdout}{stderr}");
    if stdout == "no pandas\n" {
        let _ = writeln!(std::io::stderr(), "SKIP {test} (pandas not installed)");
        return;
    }
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 4, "{stdout}");
    // pandas names its Timestamp pandas.Timestamp from 3.0, pandas._libs.tslibs.timestamps.Timestamp before.
    let block = r#"Timestamp: make plain data of h in a block first, as in @py(h) { __import__("json").loads(h.to_json(date_format="iso")) }"#;
    assert!(lines[0].starts_with("MarshalError: to_v2(): a key of h has the type pandas.") && lines[0].ends_with(block), "{stdout}");
    assert_eq!(lines[1], r#"{"2026-01-01T00:00:00.000": 1, "2026-01-02T00:00:00.000": 2}"#);
    // A DataFrame's to_dict() is {column: {index: value}}: the key sits inside it.
    assert!(lines[2].starts_with("MarshalError: to_v2(): a key inside h has the type pandas.") && lines[2].ends_with(block), "{stdout}");
    assert_eq!(lines[3], r#"{"n": {"2026-01-01T00:00:00.000": 1, "2026-01-02T00:00:00.000": 2}}"#);
}

#[test]
fn a_program_that_catches_a_failing_stdout_ends_well() {
    // INTERNALS 6.5: what a failed write couldn't write goes with its IOError, so a program that
    // catches it has handled that output: exit 0, nothing said at the end.
    let home = TempHome::new();
    let Some(stdout) = failing_stdout(&home.0) else { return };
    let src = "try {\n    while true {\n        print(\"x\")\n    }\n} catch (e) {\n    print(\"caught\", e.type, stderr: true)\n}\n";
    std::fs::write(home.0.join("caught.v2"), src).unwrap();
    let child = v2_command(&home.0)
        .arg("caught.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
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
        let Some(stdout) = failing_stdout(&home.0) else { return };
        std::fs::write(home.0.join("lines.v2"), format!("{block}{end}")).unwrap();
        let child = v2_command(&home.0)
            .arg("lines.v2")
            .current_dir(&home.0)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(Stdio::piped())
            .spawn()
            .expect("start v2");
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
    let Some(stdout) = failing_stdout(&home.0) else { return };
    let src = "let big = \"y\" * 100000\n@py(big) {\n    import sys\n    sys.stdout.write(big)\n    raise ValueError(\"block failed\")\n}\n";
    std::fs::write(home.0.join("hidden.v2"), src).unwrap();
    let child = v2_command(&home.0)
        .arg("hidden.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("v2: cannot write to stdout") && err.contains("ValueError: block failed"), "{err}");
}

#[test]
fn a_failing_stdout_is_reported_once() {
    // INTERNALS 6.5: the IOError that a write raised says it; the end of the run doesn't again.
    let home = TempHome::new();
    let Some(stdout) = failing_stdout(&home.0) else { return };
    std::fs::write(home.0.join("flood.v2"), "while true {\n    print(\"x\")\n}\n").unwrap();
    let child = v2_command(&home.0)
        .arg("flood.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
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
    // macOS's APFS refuses the name itself; V2 refuses the argument before looking for a file.
    let _ = std::fs::write(home.0.join(name), "print(1)\n");
    for args in [vec![name], vec![std::ffi::OsStr::new("check"), name]] {
        let out = v2_command(&home.0).args(&args).current_dir(&home.0).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{err}");
        assert!(err.contains("is not valid UTF-8"), "{err}");
    }
}

#[test]
fn a_worker_writing_to_a_failing_stdout_is_an_io_error() {
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
    let Some(stdout) = failing_stdout(&home.0) else { return };
    std::fs::write(home.0.join("flood.v2"), src).unwrap();
    let child = v2_command(&home.0)
        .arg("flood.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
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
    let Some(stdout) = failing_stdout(&home.0) else { return };
    std::fs::write(home.0.join("stale.v2"), src).unwrap();
    let child = v2_command(&home.0)
        .arg("stale.v2")
        .current_dir(&home.0)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .expect("start v2");
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
/// A .v2 file in an older encoding: what V2 says, which on Linux and macOS names iconv (2.1).
const OLDER: &str = if cfg!(windows) {
    "file is not valid UTF-8: it may be in an older encoding, such as Windows-1252 (what Windows PowerShell's Set-Content writes): save it as UTF-8"
} else {
    "file is not valid UTF-8: it may be in an older encoding, such as Windows-1252 or Latin-1: save it as UTF-8, as in iconv -f cp1252 -t utf-8 old.v2 > new.v2"
};

/// The signal `interrupt` sends (Windows: Ctrl+Break, which has none).
#[cfg(unix)]
const SIGINT: i32 = libc::SIGINT;
#[cfg(windows)]
const SIGINT: i32 = 2;

/// An interrupted run (6.5): ended by the signal that came, so a shell stops a loop of v2 runs on
/// one Ctrl+C (shown as 128 + its number); on Windows, exit 130.
fn assert_interrupted(status: &std::process::ExitStatus, sig: i32, what: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!((status.signal(), status.code()), (Some(sig), None), "{what}");
    }
    #[cfg(windows)]
    {
        let _ = sig;
        assert_eq!(status.code(), Some(130), "{what}");
    }
}

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

#[cfg(unix)]
#[test]
fn the_adapters_folder_leaves_sys_path_under_a_symlinked_temp_folder() {
    // INTERNALS 9.3: Python resolves symlinks in sys.path[0], and macOS's temp folder is one; the
    // adapter's own folder still leaves sys.path, so a block's `import py` can't find the adapter.
    if !needs("py", "the_adapters_folder_leaves_sys_path_under_a_symlinked_temp_folder") {
        return;
    }
    let home = TempHome::new();
    std::fs::create_dir_all(home.0.join("real")).unwrap();
    std::os::unix::fs::symlink(home.0.join("real"), home.0.join("link")).unwrap();
    let src = "print(@py { import sys; any(p.rstrip('/').endswith('adapters') for p in sys.path) })\n";
    std::fs::write(home.0.join("main.v2"), src).unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg("main.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    with_temp(&mut cmd, &home.0.join("link"));
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stdout).as_ref()), (Some(0), "false\n"), "{stderr}");
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

#[cfg(unix)]
#[test]
fn ctrl_z_stops_the_workers_with_v2() {
    // INTERNALS 6.5: SIGTSTP (Ctrl+Z) stops v2 and its workers' process groups, as a shell's job
    // is stopped; SIGCONT (fg, bg) goes on with all of them.
    if !needs("py", "ctrl_z_stops_the_workers_with_v2") {
        return;
    }
    let home = TempHome::new();
    let src = "@py {\n    import os, time\n    open(\"worker.pid\", \"w\").write(str(os.getpid()))\n    with open(\"progress.txt\", \"w\") as f:\n        for i in range(40):\n            f.write(\"x\\n\")\n            f.flush()\n            time.sleep(0.05)\n}\nprint(\"done\")\n";
    std::fs::write(home.0.join("busy.v2"), src).unwrap();
    let child = v2_command(&home.0).arg("busy.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let v2 = child.id() as libc::pid_t;
    let deadline = Instant::now() + Duration::from_secs(30);
    let worker = loop {
        if let Some(pid) = std::fs::read_to_string(home.0.join("worker.pid")).ok().and_then(|s| s.trim().parse::<libc::pid_t>().ok()) {
            break pid;
        }
        assert!(Instant::now() < deadline, "the block never started");
        std::thread::sleep(Duration::from_millis(20));
    };
    let state = |pid: libc::pid_t| String::from_utf8_lossy(&Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().unwrap().stdout).trim().to_string();
    // SAFETY: signals to our own child.
    unsafe { libc::kill(v2, libc::SIGTSTP) };
    let deadline = Instant::now() + Duration::from_secs(5);
    while !(state(v2).starts_with('T') && state(worker).starts_with('T')) {
        assert!(Instant::now() < deadline, "v2 ({}) and its worker ({}) weren't both stopped", state(v2), state(worker));
        std::thread::sleep(Duration::from_millis(20));
    }
    let lines = || std::fs::read_to_string(home.0.join("progress.txt")).map(|s| s.lines().count()).unwrap_or(0);
    let before = lines();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(lines(), before, "the worker ran on while the job was stopped");
    // SAFETY: as above.
    unsafe { libc::kill(v2, libc::SIGCONT) };
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(30)).unwrap();
    assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned()), (Some(0), "done\n".to_string()), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(lines(), 40);
}

#[cfg(unix)]
#[test]
fn time_stopped_doesnt_count_toward_call_timeout() {
    // INTERNALS 8.5: v2 stopped (SIGSTOP, which it can't see) for longer than call_timeout,
    // during calls that are quicker than it: the clock skips the time stopped, so no call times
    // out and the engine isn't killed.
    if !needs("py", "time_stopped_doesnt_count_toward_call_timeout") {
        return;
    }
    let home = TempHome::new();
    std::fs::write(home.0.join("v2.toml"), "[engines.py]\ncall_timeout = 2\n").unwrap();
    let src = "@py t {\n    import time\n    def one():\n        time.sleep(1)\n        return \"ok\"\n    @export { one }\n}\n@import { one } from t\nfor i in 0..3 {\n    print(i, one())\n}\n";
    std::fs::write(home.0.join("tick.v2"), src).unwrap();
    let child = v2_command(&home.0).arg("tick.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let v2 = child.id() as libc::pid_t;
    std::thread::sleep(Duration::from_millis(1500));
    // SAFETY: signals to our own child.
    unsafe { libc::kill(v2, libc::SIGSTOP) };
    std::thread::sleep(Duration::from_secs(3));
    // SAFETY: as above.
    unsafe { libc::kill(v2, libc::SIGCONT) };
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(30)).unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "0 ok\n1 ok\n2 ok\n", "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(0));
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
        assert_interrupted(&out.status, signal, &String::from_utf8_lossy(&out.stderr));
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
        assert_interrupted(&out.status, SIGINT, "");
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
    assert_interrupted(&out.status, SIGINT, "");
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
            assert_interrupted(&out.status, SIGINT, &format!("{name}: stdout {stdout:?} stderr {stderr:?}"));
            assert!(stdout.is_empty() && stderr.is_empty(), "{name}: stdout {stdout:?} stderr {stderr:?}");
        }
    }
}

/// A stdout that fails every write with an error other than a closed pipe, which stops the run
/// quietly instead (6.5): the IOError rules of 6.5 and 9.9 for every other failure. A full device
/// on Linux, a read-only handle on Windows; `None` on macOS, which has neither.
#[allow(unused_variables, unreachable_code)]
fn failing_stdout(home: &Path) -> Option<Stdio> {
    #[cfg(target_os = "linux")]
    return Some(Stdio::from(std::fs::OpenOptions::new().write(true).open("/dev/full").expect("/dev/full")));
    #[cfg(windows)]
    {
        let path = home.join("read-only stdout");
        std::fs::write(&path, b"").unwrap();
        return Some(Stdio::from(std::fs::File::open(&path).unwrap()));
    }
    None
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
    // Program output whose reader went away by the end of the run: nothing said, and exit 0
    // becomes 141; another code is kept (6.5).
    let (got, stderr) = run_with_a_closed_stream(d, &["small.v2"], true);
    assert_eq!((got, stderr.as_str()), (Some(141), ""));
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
    let bad = stderr.find("bad.v2:1:10: SyntaxError: file is not valid UTF-8").expect(&stderr);
    let good = stderr.find("good.v2:2:5: SyntaxError: 'y' is already declared").expect(&stderr);
    assert!(bad < good, "{stderr}");
    let (code, stdout, stderr) = check("bad.v2");
    assert_eq!((code, stdout.as_str()), (Some(1), "check bad.v2: 1 file, 1 error\n"), "{stderr}");
    assert_eq!(stderr, format!("bad.v2:1:10: SyntaxError: {OLDER}\n"));
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
            &["b.v2:1:1: SyntaxError: @py block closes at a } that does not start its line", "note: the } that closes it (b.v2:2:11)", "hint: put the } that ends the block on a line of its own"],
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
fn every_interrupt_signal_ends_the_run_by_it_and_ignored_ones_stay_ignored() {
    // INTERNALS 6.5: SIGINT, SIGTERM, SIGHUP and SIGQUIT each end the run by that signal (a shell
    // shows 128 + its number), with no catch, finally or diagnostic; a signal ignored at startup
    // (nohup) stays ignored.
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
        assert_interrupted(&out.status, sig, &format!("signal {sig}"));
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
    assert_interrupted(&out.status, libc::SIGTERM, "");
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
    // 19.2: COMMAND is printed last; the cells come back as STATUS, COMMAND, VERSION, FROM.
    assert!(header.split_whitespace().eq(["ENGINE", "STATUS", "VERSION", "FROM", "COMMAND"]), "{stdout}");
    let starts: Vec<usize> = ["STATUS", "VERSION", "FROM", "COMMAND"].iter().map(|h| header.find(h).unwrap()).collect();
    let mut rows = HashMap::new();
    for line in lines.take_while(|l| !l.starts_with("notes:") && !l.starts_with("config file:")) {
        let cell = |i: usize| -> String {
            let from = starts[i];
            let to = starts.get(i + 1).copied().unwrap_or(line.len()).min(line.len());
            line.get(from..to).unwrap_or_default().trim().to_string()
        };
        let id = line[..starts[0]].trim().to_string();
        rows.insert(id, [0, 3, 1, 2].into_iter().map(cell).collect());
    }
    rows
}

#[test]
fn a_local_es_module_under_commonjs_shows_v2s_hint_without_nodes_warning() {
    // INTERNALS 13.5: Node's "Failed to load the ES module" warning says what V2's hint says
    // better, so a file of the user's gets only V2's.
    let files = [
        ("package.json", "{\"type\": \"commonjs\"}\n"),
        ("math.ts", "export function add(a: number, b: number): number {\n  return a + b\n}\n"),
        ("m.v2", "@import { add } from ts.\"./math.ts\"\nprint(add(1, 2))\n"),
    ];
    let (code, _, stderr, _home) = run_with_files(&files, &["m.v2"]);
    let required = std::env::var("V2_TEST_REQUIRE").unwrap_or_default().split([',', ' ']).any(|t| t == "ts");
    if stderr.contains("EngineNotFound") && !required {
        return;
    }
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("says \"type\": \"commonjs\""), "{stderr}");
    assert!(!stderr.contains("Failed to load the ES module"), "{stderr}");
}

#[test]
fn a_package_another_folder_has_its_own_copy_of_is_noted_once() {
    // DOCS 7, INTERNALS 13.5: a bare name is loaded once per run, from the first importer's
    // folder; a later importer whose folder resolves it to another file is told once.
    let legacy = |v: &str| format!("exports.version = '{v}'\nexports.v = () => '{v}'\n");
    let (old, new) = (legacy("0.9"), legacy("2.0"));
    let files = [
        ("node_modules/legacy/index.js", old.as_str()),
        ("tools/deep/node_modules/legacy/index.js", new.as_str()),
        ("scripts/lib2.v2", "@import js.legacy as legacy\nfunc lib_version() {\n    return legacy.version\n}\n"),
        // Same folder as two.v2: no second note. Its own folder resolves to the first copy: none.
        ("tools/deep/other.v2", "@import { v } from js.legacy\nfunc other() {\n    return v()\n}\n"),
        ("scripts/sub/same.v2", "@import { v } from js.legacy\nfunc same() {\n    return v()\n}\n"),
        (
            "tools/deep/two.v2",
            "import \"../../scripts/lib2.v2\" as lib\nimport \"./other.v2\" as other\nimport \"../../scripts/sub/same.v2\" as same\nprint(lib.lib_version())\n@import js.legacy as legacy\nprint(legacy.version)\nfunc again() {\n    @import { v } from js.legacy\n    return v()\n}\nprint(again(), again(), other.other(), same.same())\nprint(@js { require(\"legacy\").version })\n",
        ),
    ];
    let (code, stdout, stderr, _home) = run_with_files(&files, &["tools/deep/two.v2"]);
    let required = std::env::var("V2_TEST_REQUIRE").unwrap_or_default().split([',', ' ']).any(|t| t == "js");
    if stderr.contains("EngineNotFound") && !required {
        return;
    }
    assert_eq!(code, Some(0), "{stderr}");
    // @import gets the first copy everywhere; a block's require looks from its own folder.
    assert_eq!(stdout, "0.9\n0.9\n0.9 0.9 0.9 0.9\n2.0\n", "{stderr}");
    let notes: Vec<&str> = stderr.lines().filter(|l| l.starts_with("note: ")).collect();
    assert_eq!(notes.len(), 1, "{stderr}");
    let rest = notes[0].strip_prefix("note: js.legacy from ").expect(notes[0]);
    let (dir, rest) = rest.split_once(" resolves to ").expect(notes[0]);
    let (here, rest) = rest.split_once(", but this run already loaded ").expect(notes[0]);
    let first = rest.strip_suffix(": a package is loaded once per run (DOCS 7)").expect(notes[0]);
    assert!(Path::new(dir).ends_with("tools/deep"), "{dir}");
    assert!(Path::new(here).ends_with("tools/deep/node_modules/legacy/index.js"), "{here}");
    assert!(Path::new(first).ends_with("node_modules/legacy/index.js") && !first.contains("deep"), "{first}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_virtual_memory_limit_gets_a_smaller_interpreter_stack() {
    // main.rs: under ulimit -v 260000 (254 MiB) the 256 MiB stack can't be reserved, so the
    // interpreter starts on a smaller one, and the script runs.
    use std::os::unix::process::CommandExt;
    let home = TempHome::new();
    std::fs::write(home.0.join("m.v2"), "func f(n) {\n    return n == 0 ? 0 : 1 + f(n - 1)\n}\nprint(f(1000))\n").unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg("m.v2").current_dir(&home.0).stdin(Stdio::null());
    // SAFETY: setrlimit is async-signal-safe and touches only the child.
    unsafe {
        cmd.pre_exec(|| {
            let limit = libc::rlimit { rlim_cur: 260_000 * 1024, rlim_max: 260_000 * 1024 };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "1000\n");
    // Round 15: the stack is at most a quarter of the limit, so deep recursion under 400 MB ends
    // in V2's own error (or works), not in an abort ("memory allocation of 104 bytes failed",
    // exit 134) once the heap is used up; under 50 MB too, as glibc then keeps one malloc arena
    // (each holds 64 MiB of address space), and the error says why it came early.
    std::fs::write(home.0.join("d.v2"), "func f(n) {\n    if n == 0 { return 0 }\n    return 1 + f(n - 1)\n}\nprint(f(9000))\n").unwrap();
    for (mb, must_fail) in [(400u64, false), (50, true)] {
        let mut cmd = v2_command(&home.0);
        cmd.arg("d.v2").current_dir(&home.0).stdin(Stdio::null());
        // SAFETY: as above.
        unsafe {
            cmd.pre_exec(move || {
                let limit = libc::rlimit { rlim_cur: mb << 20, rlim_max: mb << 20 };
                if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let out = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        match out.status.code() {
            Some(0) if !must_fail => assert_eq!(String::from_utf8_lossy(&out.stdout), "9000\n"),
            Some(1) => assert!(stderr.contains("RecursionError: maximum call depth exceeded at ") && stderr.contains("(ulimit -v) left V2 a smaller stack"), "{mb} MB: {stderr}"),
            other => panic!("{mb} MB: {other:?} {stderr}"),
        }
    }
    // Round 16: Node.js can't start under 400 MB, and the error names the limit.
    if !needs("js", "a_virtual_memory_limit_gets_a_smaller_interpreter_stack") {
        return;
    }
    std::fs::write(home.0.join("j.v2"), "print(@js { 1 })\n").unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg("j.v2").current_dir(&home.0).stdin(Stdio::null());
    // SAFETY: as above.
    unsafe {
        cmd.pre_exec(|| {
            let limit = libc::rlimit { rlim_cur: 400 << 20, rlim_max: 400 << 20 };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let out = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("a memory limit (ulimit -v) of 400 MB is set for this shell, and Node.js reserves about 1 GB as it starts: raise it"), "{stderr}");
}

#[cfg(target_os = "linux")]
#[test]
fn doctor_starts_node_under_a_memory_limit() {
    // Round 19, 19.2: `node --version` answers under a limit too small for Node.js to start,
    // so doctor also runs `node -e 0` then: one that can't start is broken, with the run's words
    // about the limit. This node dies as Node.js does there (SIGTRAP), except for --version.
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    let home = TempHome::new();
    let bin = home.0.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let node = bin.join("node");
    std::fs::write(&node, "#!/bin/sh\n[ \"$1\" = --version ] && { echo v22.13.0; exit 0; }\nkill -TRAP $$\n").unwrap();
    std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
    let doctor = |limit: Option<libc::rlim_t>| {
        let mut cmd = v2_command(&home.0);
        cmd.arg("doctor").current_dir(&home.0).env("PATH", &bin).env_remove("NODE_OPTIONS").stdin(Stdio::null());
        if let Some(bytes) = limit {
            // SAFETY: setrlimit is async-signal-safe and touches only the child.
            unsafe {
                cmd.pre_exec(move || {
                    let limit = libc::rlimit { rlim_cur: bytes, rlim_max: bytes };
                    if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let out = cmd.output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    // No limit: --version is all doctor asks.
    let stdout = doctor(None);
    assert_eq!(doctor_rows(&stdout)["js"][0], "ok", "{stdout}");
    let stdout = doctor(Some(400 << 20));
    let rows = doctor_rows(&stdout);
    assert_eq!((rows["js"][0].as_str(), rows["ts"][0].as_str()), ("broken", "broken"), "{stdout}");
    let notes = format!(
        "\n  js: {} -e 0 failed (killed by signal 5 (SIGTRAP))\n  js: fix: a memory limit (ulimit -v) of 400 MB is set for this shell, and Node.js reserves about 1 GB as it starts: raise it\n",
        node.display()
    );
    assert!(stdout.contains(&notes), "{stdout}");
    assert!(stdout.contains("\n  ts: @ts blocks run on the js engine: see js above\n"), "{stdout}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_write_past_the_file_size_limit_is_an_ioerror() {
    // INTERNALS 8.3: V2 ignores SIGXFSZ, so a write past ulimit -f fails with EFBIG, an IOError
    // the script can catch, instead of killing v2.
    use std::os::unix::process::CommandExt;
    let home = TempHome::new();
    let src = "try {\n    write_file(\"big.out\", \"x\" * 1048576)\n} catch (e) {\n    print(e.type, \"|\", e.hint)\n}\n";
    std::fs::write(home.0.join("f.v2"), src).unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg("f.v2").current_dir(&home.0).stdin(Stdio::null());
    // SAFETY: setrlimit is async-signal-safe and touches only the child.
    unsafe {
        cmd.pre_exec(|| {
            let limit = libc::rlimit { rlim_cur: 100 * 1024, rlim_max: 100 * 1024 };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "IOError | the file size limit (ulimit -f) stops it: raise the limit, or write less\n");
}

#[cfg(target_os = "linux")]
#[test]
fn a_python_that_never_answers_is_noted_in_the_run() {
    // INTERNALS 7.6: a hit on PATH whose --version probe times out costs the run its 10 s, so the
    // run says so on stderr, and goes on with the next one.
    use std::os::unix::fs::PermissionsExt;
    let home = TempHome::new();
    let slow = home.0.join("slow");
    std::fs::create_dir_all(&slow).unwrap();
    std::fs::write(slow.join("python3"), "#!/bin/sh\nsleep 60\n").unwrap();
    std::fs::set_permissions(slow.join("python3"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(home.0.join("p.v2"), "print(@py { 1 + 1 })\n").unwrap();
    let path = format!("{}:{}", slow.display(), std::env::var("PATH").unwrap_or_default());
    let (code, stdout, stderr) = run_in_home(&home.0, &["p.v2"], &[("PATH", std::ffi::OsStr::new(&path))]);
    let required = std::env::var("V2_TEST_REQUIRE").unwrap_or_default().split([',', ' ']).any(|t| t == "py");
    if stderr.contains("EngineNotFound") && !required {
        return;
    }
    assert_eq!((code, stdout.as_str()), (Some(0), "2\n"), "{stderr}");
    let note = format!("note: {} didn't answer --version in 10 s, so V2 used ", slow.join("python3").display());
    assert!(stderr.starts_with(&note) && stderr.ends_with(" (v2 doctor lists what it skips)\n"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn a_deleted_current_folder_is_said() {
    // INTERNALS 19.1: a terminal still in a folder that was deleted.
    let home = TempHome::new();
    let script = format!("mkdir gone && cd gone && rmdir ../gone && exec \"{}\" hello.v2", BIN);
    let out = Command::new("sh").arg("-c").arg(script).current_dir(&home.0).env("HOME", &home.0).stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "v2: cannot find 'hello.v2': the current folder no longer exists (it was deleted or moved)\ncd to another folder, or give the script's full path\n"
    );
}

#[test]
fn pythoninspect_doesnt_hold_the_python_worker() {
    // INTERNALS 14.3: PYTHONINSPECT (python -i) would print the worker's SystemExit at the end and
    // wait for input; atexit still runs.
    let src = "print(@py { 1 + 1 })\n@py {\n    import atexit\n    atexit.register(lambda: print(\"atexit ran\"))\n}\n";
    let (code, stdout, stderr, _home) = run_with_files_env(&[("p.v2", src)], &["p.v2"], &[("PYTHONINSPECT", "1")]);
    let required = std::env::var("V2_TEST_REQUIRE").unwrap_or_default().split([',', ' ']).any(|t| t == "py");
    if stderr.contains("EngineNotFound") && !required {
        return;
    }
    assert_eq!((code, stdout.as_str(), stderr.as_str()), (Some(0), "2\natexit ran\n", ""));
}

#[cfg(target_os = "linux")]
#[test]
fn a_script_in_a_folder_whose_name_isnt_utf8_is_refused() {
    // Round 16: given by its own name from inside such a folder, as given by its full path, the
    // script is refused (script_dir() would name a folder that isn't there).
    use std::os::unix::ffi::OsStrExt;
    let home = TempHome::new();
    let dir = home.0.join(std::ffi::OsStr::from_bytes(b"dir\xff"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sd.v2"), "print(script_dir())\n").unwrap();
    let out = v2_command(&home.0).arg("sd.v2").current_dir(&dir).stdin(Stdio::null()).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("is not valid UTF-8, which V2 needs: a folder on the way has a name in another encoding; rename it"), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
}

#[cfg(unix)]
#[test]
fn exec_says_when_path_is_unset_or_python_is_python3() {
    // Round 16: with no PATH nothing was searched, so say that; and where only python3 is on
    // PATH (Ubuntu, macOS), exec(["python", …]) points at it.
    use std::os::unix::fs::PermissionsExt;
    let home = TempHome::new();
    let bin = home.0.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("python3"), "#!/bin/sh\necho 3\n").unwrap();
    std::fs::set_permissions(bin.join("python3"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let src = "for argv in [[\"ls\"], [\"python\", \"-V\"]] {\n    try { exec(argv) } catch (e) { print(e.message, \"|\", e.hint) }\n}\n";
    std::fs::write(home.0.join("t.v2"), src).unwrap();
    let out = v2_command(&home.0).arg("t.v2").current_dir(&home.0).env_remove("PATH").stdin(Stdio::null()).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "exec: cannot start 'ls': not found, as PATH isn't set: set it, or give the program's full path | null\nexec: cannot start 'python': not found, as PATH isn't set: set it, or give the program's full path | null\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = v2_command(&home.0).arg("t.v2").current_dir(&home.0).env("PATH", &bin).stdin(Stdio::null()).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "exec: cannot start 'ls': not found on PATH | null\nexec: cannot start 'python': not found on PATH | this system names it python3: write exec([\"python3\", …])\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(unix)]
#[test]
fn read_file_of_dev_stdin_gives_what_input_left() {
    // Round 16: after input(), read_file("/dev/stdin") reads on from there, for a pipe, a file
    // and a file the shell already read a line of (it reopened the path: a pipe lost input()'s
    // buffer, a file started over).
    let home = TempHome::new();
    std::fs::write(home.0.join("data.csv"), "header\nrow1\nrow2\n").unwrap();
    std::fs::write(home.0.join("rest.v2"), "let h = input()\nprint(h, json_stringify(read_file(\"/dev/stdin\")), input())\n").unwrap();
    std::fs::write(home.0.join("all.v2"), "print(json_stringify(read_file(\"/dev/fd/0\")))\n").unwrap();
    let file = || std::fs::File::open(home.0.join("data.csv")).unwrap();
    let out = v2_command(&home.0).arg("rest.v2").current_dir(&home.0).stdin(file()).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "header \"row1\\nrow2\\n\" null\n", "{}", String::from_utf8_lossy(&out.stderr));
    let child = v2_command(&home.0).arg("rest.v2").current_dir(&home.0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let out = wait_with_timeout(child, b"header\nrow1\nrow2\n".to_vec(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stdout).as_ref()), (Some(0), "header \"row1\\nrow2\\n\" null\n"), "{}", String::from_utf8_lossy(&out.stderr));
    let mut sh = Command::new("sh");
    sh.args(["-c", "{ read -r h; \"$0\" all.v2; } < data.csv"]).arg(BIN);
    for (k, v) in v2_command(&home.0).get_envs() {
        match v {
            Some(v) => sh.env(k, v),
            None => sh.env_remove(k),
        };
    }
    let out = sh.current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "\"row1\\nrow2\\n\"\n", "{}", String::from_utf8_lossy(&out.stderr));
}

#[cfg(unix)]
#[test]
fn exec_of_a_file_not_marked_as_a_program_says_what_works() {
    // Round 15: a script in the current folder without its x bit, by its bare name or its path,
    // gets a hint whose line works (the bare-name hint alone led to "Permission denied").
    use std::os::unix::fs::PermissionsExt;
    let home = TempHome::new();
    for f in ["job.sh", "tool"] {
        std::fs::write(home.0.join(f), "#!/bin/sh\necho hi\n").unwrap();
        std::fs::set_permissions(home.0.join(f), std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let src = "for argv in [[\"job.sh\"], [\"./job.sh\"], [\"tool\"], [\"./tool\"]] {\n    try { exec(argv) } catch (e) { print(e.hint) }\n}\nprint(exec([\"sh\", \"./job.sh\"]).stdout.trim())\n";
    std::fs::write(home.0.join("t.v2"), src).unwrap();
    let out = v2_command(&home.0).arg("t.v2").current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let expected = concat!(
        "V2 doesn't look in the current folder for a bare name, and ./job.sh isn't marked as a program: write exec([\"sh\", \"./job.sh\"]), or mark it once with chmod +x ./job.sh and write exec([\"./job.sh\"])\n",
        "./job.sh isn't marked as a program: write exec([\"sh\", \"./job.sh\"]), or mark it once with chmod +x ./job.sh\n",
        "V2 doesn't look in the current folder for a bare name, and ./tool isn't marked as a program: mark it once with chmod +x ./tool and write exec([\"./tool\"]), or run it through its program, as in exec([\"sh\", \"./tool\"])\n",
        "./tool isn't marked as a program: mark it once with chmod +x ./tool, or run it through its program, as in exec([\"sh\", \"./tool\"])\n",
        "hi\n",
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[cfg(unix)]
#[test]
fn a_signal_during_exec_stops_the_next_statement() {
    // INTERNALS 6.5: a SIGTERM that lands while exec runs (here the program's own `kill $PPID`)
    // ends the run as exec returns: the write after it never happens.
    let home = TempHome::new();
    // No sleep after the kill: exec returns before the watchdog's next tick (round 15).
    let src = "exec([\"sh\", \"-c\", \"kill -TERM $PPID\"])\nprint(\"after\")\nwrite_file(\"after.txt\", \"written after the signal\")\n";
    std::fs::write(home.0.join("t.v2"), src).unwrap();
    let out = v2_command(&home.0).arg("t.v2").current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
    assert_interrupted(&out.status, libc::SIGTERM, &String::from_utf8_lossy(&out.stderr));
    assert!(!home.0.join("after.txt").exists(), "the write ran after the signal");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    // The same from a worker that signals v2 while it answers a block (the session's request).
    let src = "if engine_available(\"py\") {\n    @py { import os, signal; os.kill(os.getppid(), signal.SIGTERM) }\n} else {\n    exec([\"sh\", \"-c\", \"kill -TERM $PPID\"])\n}\nprint(\"after\")\nwrite_file(\"after.txt\", \"written after the signal\")\n";
    std::fs::write(home.0.join("t.v2"), src).unwrap();
    let out = v2_command(&home.0).arg("t.v2").current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
    assert_interrupted(&out.status, libc::SIGTERM, &String::from_utf8_lossy(&out.stderr));
    assert!(!home.0.join("after.txt").exists(), "the write ran after the worker's signal");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
}

#[test]
fn doctor_notes_engine_tables_of_later_versions() {
    // INTERNALS 19.2: a v2.toml written for R or @sh says, before any block is written, that
    // this version doesn't run them.
    let toml = "[engines.r]\nkind = \"run\"\ncommand = [\"Rscript\", \"{file}\"]\n\n[engines.sh]\ncommand = [\"bash\"]\n";
    let (code, stdout, stderr, _home) = run_with_files(&[("v2.toml", toml)], &["doctor"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.contains("\n  r: [engines.r] in v2.toml is a custom engine, and custom engines aren't in this version of V2 yet: @r blocks won't run\n"), "{stdout}");
    assert!(stdout.contains("\n  sh: [engines.sh] in v2.toml configures @sh, which isn't in this version of V2 yet\n"), "{stdout}");
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
        assert!(["ok", "missing", "old", "unsupported", "broken"].contains(&row[0].as_str()), "{stdout}");
        assert_eq!(row[0] == "ok", avail, "{id}: doctor and engine_available agree:\n{stdout}");
        if row[0] == "missing" {
            assert_eq!(&row[1..], ["-", "-", "-"], "{stdout}");
        }
    }
    assert!(stdout.contains("config file: none"), "{stdout}");
    // The summary names the engines that are ready (19.2).
    let ok: Vec<&str> = ["py", "js", "ts"].into_iter().filter(|id| rows[*id][0] == "ok").collect();
    let ready = if ok.is_empty() { "none".to_string() } else { ok.join(", ") };
    assert!(stdout.contains(&format!("\nready: {ready}\n")), "{stdout}");
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
    // The working directory with its links followed, as a run's script folder is (18.1, round
    // 19): macOS's temp folder is a symlink, and Windows' may be an 8.3 short name.
    let real = PathBuf::from(real_path(&home.0));
    let toml = real.join("v2.toml");
    assert!(stdout.contains(&format!("py: engine 'py' command '{}' not found (set by {})", real.join("missing/python.exe").display(), toml.display())), "{stdout}");
    assert!(stdout.contains(&format!("config file: {}", toml.display())), "{stdout}");
}

#[test]
fn doctor_names_the_programs_it_could_not_use() {
    // 19.2: an old Python and a Node.js that doesn't run, found on PATH: their rows show the
    // program and FROM PATH, ts shows js's, and the notes say why.
    let home = TempHome::new();
    let bin = home.0.join("fake bin");
    std::fs::create_dir_all(&bin).unwrap();
    let ext = if cfg!(windows) { ".cmd" } else { "" };
    let (py, node) = (bin.join(format!("python3{ext}")), bin.join(format!("node{ext}")));
    if cfg!(windows) {
        std::fs::write(&py, "@echo Python 3.8.1\r\n").unwrap();
        std::fs::write(&node, "@echo node: error while loading shared libraries: libatomic.so.1 1>&2\r\n@exit /b 127\r\n").unwrap();
    } else {
        std::fs::write(&py, "#!/bin/sh\necho Python 3.8.1\n").unwrap();
        std::fs::write(&node, "#!/bin/sh\necho 'node: error while loading shared libraries: libatomic.so.1' >&2\nexit 127\n").unwrap();
    }
    #[cfg(unix)]
    for f in [&py, &node] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (code, stdout, stderr) = run_in_home(&home.0, &["doctor"], &[("PATH", bin.as_os_str())]);
    assert_eq!(code, Some(0), "{stderr}");
    let rows = doctor_rows(&stdout);
    let (py, node) = (py.display().to_string(), node.display().to_string());
    assert_eq!(rows["py"], ["old", py.as_str(), "3.8.1", "PATH"], "{stdout}");
    assert_eq!(rows["js"], ["broken", node.as_str(), "-", "PATH"], "{stdout}");
    assert_eq!(rows["ts"], ["broken", node.as_str(), "-", "PATH"], "{stdout}");
    assert!(stdout.contains("\n  py: too old: found 3.8.1, V2 needs 3.10+; install: "), "{stdout}");
    // The row and that note say it all: no skipped line repeats it.
    assert!(!stdout.contains(&format!("py: skipped {py}")), "{stdout}");
    // The package's line fits the system (apt or dnf, sudo unless root): INTERNALS 7.8.
    let fix = format!("\n  js: fix: {node} needs libatomic.so.1: ");
    let line = stdout.split(&*fix).nth(1).and_then(|r| r.lines().next()).unwrap_or_else(|| panic!("{stdout}"));
    assert!(line.contains("apt install libatomic1") || line.contains("dnf install libatomic"), "{stdout}");
    // The note, then the program it skipped and why, which "see why below" points at.
    let below = format!("\n  js: engine 'js' (JavaScript) was found but doesn't run (see why below)\n  js: skipped {node} (version probe failed (exit code 127): node: error while loading shared libraries: libatomic.so.1)\n");
    assert!(stdout.contains(&below), "{stdout}");
    // ts runs on that node: js's notes, fix included, are its own (19.2).
    assert!(stdout.contains("\n  ts: @ts blocks run on the js engine: see js above\n"), "{stdout}");
    // The only Python fails with no word: repair it, or install one, as for a missing one.
    if cfg!(windows) {
        std::fs::write(&py, "@exit /b 109\r\n").unwrap();
    } else {
        std::fs::write(&py, "#!/bin/sh\nexit 109\n").unwrap();
    }
    let (code, stdout, stderr) = run_in_home(&home.0, &["doctor"], &[("PATH", bin.as_os_str())]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(doctor_rows(&stdout)["py"][0], "broken", "{stdout}");
    let note = format!("\n  py: skipped {py} (version probe failed (exit code 109))\n  py: fix: repair that Python, or install one: ");
    assert!(stdout.contains(&note), "{stdout}");
}

#[test]
fn an_imported_files_syntax_error_shows_its_line_and_hint() {
    // 6.4: uncaught, the ImportError shows the problem as v2 check would: its line and its hint.
    let files = [("main.v2", "import \"./lib/util.v2\" as util\nprint(util.shout(\"x\"))\n"), ("lib/util.v2", "func shout(s) => s.upper()\n")];
    let (code, _out, err, _home) = run_with_files(&files, &["main.v2"]);
    assert_eq!(code, Some(1), "{err}");
    let shown = "lib/util.v2";
    let want = format!(
        "main.v2:1:1: ImportError: cannot import './lib/util.v2': {shown}:1:15: SyntaxError: expected {{, found `=>`\n    import \"./lib/util.v2\" as util\n    ^\n  in {shown}:\n    func shout(s) => s.upper()\n                  ^\n  hint: a named function has a {{ }} body: func shout(s) {{ return … }}, or write let shout = func(s) => …\n"
    );
    assert!(err.starts_with(&want), "{err}");
}

#[cfg(unix)]
#[test]
fn a_script_run_through_a_symlink_uses_its_own_folders_v2_toml() {
    // 18.1: the config is searched for from the real file's folder, as script_dir() sees it.
    let home = TempHome::new();
    std::fs::create_dir_all(home.0.join("proj")).unwrap();
    std::fs::create_dir_all(home.0.join("links")).unwrap();
    std::fs::write(home.0.join("proj/v2.toml"), "[engines.py]\ncommand = [\"missing/python\"]\n").unwrap();
    std::fs::write(home.0.join("proj/s.v2"), "print(engine_available(\"py\"))\n").unwrap();
    std::os::unix::fs::symlink(home.0.join("proj/s.v2"), home.0.join("links/s.v2")).unwrap();
    let (code, out, err) = run_in_home(&home.0, &["links/s.v2"], &[]);
    assert_eq!((code, out.as_str()), (Some(0), "false\n"), "{err}");
}

#[test]
fn a_temp_folder_that_doesnt_exist_is_named() {
    // 20.3 and 19.2: blocks need a temp folder; the message and doctor name the variable.
    let home = TempHome::new();
    let missing = home.0.join("no such temp");
    let var = if cfg!(windows) { "TMP" } else { "TMPDIR" };
    let env = [(var, missing.as_os_str())];
    let (code, out, _err) = run_in_home(&home.0, &["doctor"], &env);
    assert_eq!(code, Some(0));
    let problem = format!("{var} names {}, which doesn't exist: blocks can't run until it names a folder you can write to", missing.display());
    assert!(out.contains(&format!("\n  temp: {problem}\n")), "{out}");
    assert!(out.contains("not ready: ") && out.contains("blocks"), "{out}");
    if !needs("py", "a_temp_folder_that_doesnt_exist_is_named") {
        return;
    }
    std::fs::write(home.0.join("b.v2"), "print(@py { 1 })\n").unwrap();
    let (code, _out, err) = run_in_home(&home.0, &["b.v2"], &env);
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("IOError: cannot create a private temp folder in ") && err.contains(&format!("; {problem}")), "{err}");
}

#[test]
fn file_and_exec_errors_say_what_to_write() {
    // 5.10: what a shell would have done for the script (~, %VAR%, $env:VAR, its own commands)
    // and a folder given to remove_file get a hint with what to write instead.
    let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let mut cases: Vec<(&str, String)> = vec![
        ("read_file(\"~/no-such-v2-file.txt\")\n", format!("write getenv(\"{home}\") + \"/no-such-v2-file.txt\", or ./~ for a folder that is really named ~")),
        ("read_file(\"%NO_SUCH_V2_VAR%/x.txt\")\n", "write getenv(\"NO_SUCH_V2_VAR\") in its place, or ./%NO_SUCH_V2_VAR% for a folder that is really named %NO_SUCH_V2_VAR%".into()),
        ("write_file(\"$env:NO_SUCH_V2_VAR/x.txt\", \"x\")\n", "write getenv(\"NO_SUCH_V2_VAR\") in its place, or ./$env:NO_SUCH_V2_VAR for a folder that is really named $env:NO_SUCH_V2_VAR".into()),
        // Written as is, these would make a folder named ~ (or $HOME) here (5.10).
        ("make_dir(\"~/out\")\n", format!("write getenv(\"{home}\") + \"/out\", or ./~ for a folder that is really named ~")),
        ("write_file(\"$HOME/x.txt\", \"a\")\n", "write getenv(\"HOME\") in its place, or ./$HOME for a folder that is really named $HOME".into()),
        ("make_dir(\"fold\")\nremove_file(\"fold\")\n", "a @py block removes a folder with what is in it: @py(path) { import shutil; shutil.rmtree(path) }".into()),
    ];
    // macOS has /usr/bin/cd, a program.
    if cfg!(windows) {
        cases.push(("exec([\"cd\", \"build\"])\n", "'cd' is a command of cmd.exe, not a program: to run a program in another folder, write exec([\"cmd\", \"/c\", \"cd /d build && program\"])".into()));
        cases.push(("exec([\"copy\", \"a b\", \"c\"])\n", "'copy' is a command of cmd.exe, not a program: write exec([\"cmd\", \"/c\", \"copy\", \"a b\", \"c\"])".into()));
    } else if cfg!(target_os = "linux") {
        cases.push(("exec([\"cd\", \"build\"])\n", "'cd' is a command of the shell, not a program: to run a program in another folder, write exec([\"sh\", \"-c\", \"cd build && program\"])".into()));
        cases.push(("exec([\"export\", \"A=1\"])\n", "'export' is a command of the shell, not a program: write exec([\"sh\", \"-c\", \"export A=1\"])".into()));
    }
    for (src, hint) in cases {
        let (code, _, stderr) = run_generated("hint.v2", src);
        assert_eq!(code, Some(1), "{src}{stderr}");
        assert!(stderr.contains(&format!("\n  hint: {hint}\n")), "{src}{stderr}");
    }
}

#[test]
fn home_in_hints_fits_the_os() {
    // Round 19: in PowerShell and cmd HOME isn't set (PowerShell's $HOME is its own), so on
    // Windows $HOME's hint names USERPROFILE, as ~'s does, and getenv("HOME")'s null hint says
    // so too, with no Linux default; elsewhere getenv("USERPROFILE") names HOME (INTERNALS 5.3,
    // 5.10).
    let home = TempHome::new();
    let src = if cfg!(windows) {
        "func hint(f) {\n    try { f() } catch (e) { print(e.hint) }\n}\n\
         hint(func() => read_file(\"$HOME/Downloads/data.csv\"))\nhint(func() => read_file(\"${HOME}\\\\x.csv\"))\n\
         hint(func() => read_file(\"notes/%HOME%/x.csv\"))\nhint(func() => read_file(getenv(\"HOME\") + \"/x.csv\"))\n\
         let h = getenv(\"HOME\")\nhint(func() => read_file(h + \"/x.csv\"))\n"
    } else {
        "func hint(f) {\n    try { f() } catch (e) { print(e.hint) }\n}\n\
         hint(func() => getenv(\"USERPROFILE\") + \"/x.csv\")\nlet u = getenv(\"USERPROFILE\")\nhint(func() => u + \"/x.csv\")\n"
    };
    std::fs::write(home.0.join("h.v2"), src).unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.env_remove(if cfg!(windows) { "HOME" } else { "USERPROFILE" });
    cmd.arg("h.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
    let stdout = normalize(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let want = if cfg!(windows) {
        let windows = "Windows sets USERPROFILE for the home folder, not HOME (Git Bash sets both): write getenv(\"USERPROFILE\")";
        format!(
            "write getenv(\"USERPROFILE\") in its place, or ./$HOME for a folder that is really named $HOME\n\
             write getenv(\"USERPROFILE\") in its place, or ./${{HOME}} for a folder that is really named ${{HOME}}\n\
             V2 doesn't expand %HOME% in paths: write getenv(\"USERPROFILE\") in its place\n\
             getenv(\"HOME\") gave null: {windows}\n'h' is null: it was set from getenv(\"HOME\"), and {windows}\n"
        )
    } else {
        let unix = "USERPROFILE is Windows' name for the home folder: write getenv(\"HOME\")";
        format!("getenv(\"USERPROFILE\") gave null: {unix}\n'u' is null: it was set from getenv(\"USERPROFILE\"), and {unix}\n")
    };
    assert_eq!(stdout, want);
}

#[test]
fn a_read_only_file_says_how_to_make_it_writable() {
    // Round 19: a write to a read-only file said only "Access is denied" (Windows' read-only
    // attribute; elsewhere no write bit): the hint says how to clear it. remove_file removes such
    // a file on every OS, as rm does (DOCS 3.11, INTERNALS 5.10).
    let home = TempHome::new();
    std::fs::create_dir_all(home.0.join("sub")).unwrap();
    for name in ["ro.txt", "sub/my ro.txt", "-ro.txt"] {
        let p = home.0.join(name);
        std::fs::write(&p, "orig").unwrap();
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&p, perms).unwrap();
    }
    let src = "func hint(f) {\n    try {\n        f()\n        print(\"written\")\n    } catch (e) {\n        print(e.message)\n        print(e.hint)\n    }\n}\n\
               hint(func() => write_file(\"ro.txt\", \"changed\"))\nhint(func() => write_file(\"ro.txt\", \"more\", append: true))\n\
               hint(func() => write_file(\"sub/my ro.txt\", \"changed\"))\nhint(func() => write_file(\"-ro.txt\", \"changed\"))\n\
               remove_file(\"ro.txt\")\nremove_file(\"sub/my ro.txt\")\nprint(exists(\"ro.txt\"), exists(\"sub/my ro.txt\"))\n";
    std::fs::write(home.0.join("r.v2"), src).unwrap();
    // root may write any file: then there is nothing to say, but remove_file still removes them.
    let root = std::fs::OpenOptions::new().append(true).open(home.0.join("ro.txt")).is_ok();
    let (code, stdout, stderr) = run_in_home(&home.0, &["r.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.last(), Some(&"false false"), "{stdout}");
    if root {
        let _ = writeln!(std::io::stderr(), "SKIP the hint part of a_read_only_file_says_how_to_make_it_writable (root writes read-only files)");
        return;
    }
    let (ro, sub) = if cfg!(windows) {
        ("clear that with attrib -r ro.txt (or in its Properties)", "clear that with attrib -r \"sub\\my ro.txt\" (or in its Properties)")
    } else {
        ("make it writable with chmod u+w ro.txt", "make it writable with chmod u+w 'sub/my ro.txt'")
    };
    let denied = |name: &str| lines.iter().any(|l| l.starts_with(&format!("cannot write '{name}': ")));
    assert!(denied("ro.txt") && denied("sub/my ro.txt") && denied("-ro.txt"), "{stdout}");
    assert_eq!(lines[1], format!("'ro.txt' is read-only: {ro}"), "{stdout}");
    assert_eq!(lines[3], format!("'ro.txt' is read-only: {ro}"), "{stdout}");
    assert_eq!(lines[5], format!("'sub/my ro.txt' is read-only: {sub}"), "{stdout}");
    // Round 19 review: a name that starts with `-`, which chmod and attrib would read as an
    // option, gets ./ (.\ for attrib) before it; the command as shown makes it writable.
    let (dash, argv) = if cfg!(windows) {
        ("clear that with attrib -r .\\-ro.txt (or in its Properties)", ["attrib", "-r", ".\\-ro.txt"])
    } else {
        ("make it writable with chmod u+w ./-ro.txt", ["chmod", "u+w", "./-ro.txt"])
    };
    assert_eq!(lines[7], format!("'-ro.txt' is read-only: {dash}"), "{stdout}");
    let ran = Command::new(argv[0]).args(&argv[1..]).current_dir(&home.0).output().unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stdout));
    assert!(!std::fs::metadata(home.0.join("-ro.txt")).unwrap().permissions().readonly());
}

#[test]
fn exec_and_file_errors_name_the_part_that_is_wrong() {
    // 5.10: a program in the current folder by its bare name, a script the OS doesn't start by
    // itself, a folder, a batch file given a line break, a missing folder and a file in the way.
    let script = if cfg!(windows) { ("tool.cmd", "@echo off\r\necho hi\r\n") } else { ("tool.sh", "echo hi\n") };
    let bare = if cfg!(windows) { "tool" } else { "tool.sh" };
    let mut cases: Vec<(String, String, Option<String>)> = vec![
        (
            format!("exec([\"{bare}\"])\n"),
            format!("IOError: exec: cannot start '{bare}': not found on PATH"),
            // On Unix the file has no execute bit, so the hint says what works (round 15).
            Some(if cfg!(windows) {
                format!("V2 doesn't look in the current folder for a bare name: write exec([\"./{}\"])", script.0)
            } else {
                "V2 doesn't look in the current folder for a bare name, and ./tool.sh isn't marked as a program: write exec([\"sh\", \"./tool.sh\"]), or mark it once with chmod +x ./tool.sh and write exec([\"./tool.sh\"])".into()
            }),
        ),
        ("exec([\"./sub\"])\n".into(), "IOError: exec: cannot start './sub': it is a folder, not a program".into(), None),
        ("write_file(\"out/r.html\", \"x\")\n".into(), "IOError: cannot write 'out/r.html': there is no folder 'out'".into(), Some("make the folder first, with make_dir(\"out\")".into())),
        ("make_dir(\"notes.txt/sub\")\n".into(), "IOError: cannot create 'notes.txt/sub': 'notes.txt' is a file, not a folder".into(), None),
        ("read_file(\"notes.txt/a.txt\")\n".into(), "IOError: cannot read 'notes.txt/a.txt': 'notes.txt' is a file, not a folder".into(), None),
    ];
    if cfg!(windows) {
        cases.push((
            "exec([\"./run.ps1\"])\n".into(),
            "IOError: exec: cannot start './run.ps1': ".into(),
            Some("a .ps1 file runs through its program: write exec([\"powershell\", \"-NoProfile\", \"-ExecutionPolicy\", \"Bypass\", \"-File\", \"./run.ps1\"])".into()),
        ));
        cases.push((
            "exec([\"./tool.cmd\", \"a\\nb\"])\n".into(),
            "IOError: exec: cannot start './tool.cmd': a batch file can't take an argument with a line break: pass the text as input, as in exec(argv, input: text)".into(),
            None,
        ));
    } else {
        // Not executable: the OS says "Permission denied"; round 15's hint says what works.
        cases.push((
            "exec([\"./tool.sh\"])\n".into(),
            "IOError: exec: cannot start './tool.sh': ".into(),
            Some("./tool.sh isn't marked as a program: write exec([\"sh\", \"./tool.sh\"]), or mark it once with chmod +x ./tool.sh".into()),
        ));
    }
    for (src, message, hint) in cases {
        let files = [("x.v2", src.as_str()), script, ("run.ps1", "Write-Output hi\n"), ("sub/keep.txt", "k"), ("notes.txt", "n")];
        let (code, _, stderr, _home) = run_with_files(&files, &["x.v2"]);
        assert_eq!(code, Some(1), "{src}{stderr}");
        assert!(stderr.contains(&message), "{src}{stderr}");
        match hint {
            Some(h) => assert!(stderr.contains(&format!("\n  hint: {h}\n")), "{src}{stderr}"),
            None => assert!(!stderr.contains("hint:"), "{src}{stderr}"),
        }
    }
}

#[cfg(windows)]
#[test]
fn a_sh_script_on_windows_is_shown_git_bash_by_its_path() {
    // INTERNALS 5.10, 7.6 (round 18): the hint names Git for Windows' bash by its full path
    // (bin\bash.exe above git's folder, else in Program Files), never a bash on PATH, which is
    // often WSL's (Linux); with none, it says a shell is needed. The call it shows runs the script.
    let home = TempHome::new();
    for f in ["Git/cmd/git.exe", "Git/bin/bash.exe", "wsl/bash.exe"] {
        std::fs::create_dir_all(home.0.join(f).parent().unwrap()).unwrap();
        std::fs::write(home.0.join(f), "").unwrap();
    }
    std::fs::write(home.0.join("build.sh"), "echo got $1\nuname -s\n").unwrap();
    std::fs::write(home.0.join("x.v2"), "try { exec([\"./build.sh\", \"a\"]) } catch (e) { print(e.hint) }\n").unwrap();
    let (wsl, git) = (home.0.join("wsl"), home.0.join("Git").join("cmd"));
    // Windows sets a 64-bit program's ProgramFiles from ProgramW6432 as it starts.
    let none = home.0.join("no program files");
    let path = std::env::join_paths([&wsl, &git]).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["x.v2"], &[("PATH", &path), ("ProgramFiles", none.as_os_str()), ("ProgramW6432", none.as_os_str())]);
    assert_eq!(code, Some(0), "{stderr}");
    let bash = home.0.join("Git/bin/bash.exe").to_string_lossy().replace('\\', "/");
    assert_eq!(stdout, format!("a .sh file runs through its program: write exec([\"{bash}\", \"./build.sh\"])\n"));
    let (code, stdout, stderr) = run_in_home(&home.0, &["x.v2"], &[("PATH", wsl.as_os_str()), ("ProgramFiles", none.as_os_str()), ("ProgramW6432", none.as_os_str())]);
    assert_eq!(code, Some(0), "{stderr}");
    let want = "a .sh file needs a shell to run it, such as Git for Windows' bash (winget install -e --id Git.Git): then write exec([\"C:/Program Files/Git/bin/bash.exe\", \"./build.sh\"])\n";
    assert_eq!(stdout, want);
    let real = std::path::Path::new(r"C:\Program Files\Git\bin\bash.exe");
    if !real.is_file() {
        let _ = writeln!(std::io::stderr(), "SKIP the Git Bash run of a_sh_script_on_windows_is_shown_git_bash_by_its_path (no Git for Windows)");
        return;
    }
    std::fs::write(home.0.join("run.v2"), "let r = exec([\"C:/Program Files/Git/bin/bash.exe\", \"./build.sh\", \"Čeština\"])\nprint(r.code, r.stdout.trim())\n").unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["run.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.starts_with("0 got Čeština\nMINGW") || stdout.starts_with("0 got Čeština\nMSYS"), "{stdout}");
}

#[cfg(windows)]
#[test]
fn git_bash_paths_and_names_windows_refuses_get_a_hint() {
    // INTERNALS 5.10 (round 18): /c/Users/… is Git Bash's spelling of C:/Users/…, in every file
    // builtin's not-found error and for exec's program, and make_dir refuses one; a name Windows
    // doesn't allow says why.
    let home = TempHome::new();
    std::fs::write(home.0.join("data.txt"), "here").unwrap();
    let full = home.0.to_string_lossy().replace('\\', "/");
    let (drive, rest) = full.split_once(':').unwrap();
    let msys = format!("/{}{rest}", drive.to_ascii_lowercase());
    let windows = format!("{}:{rest}", drive.to_ascii_uppercase());
    let spelling = format!("on Windows, /{}/… is Git Bash's spelling of {}:/…: write ", drive.to_ascii_lowercase(), drive.to_ascii_uppercase());
    let q = |s: &str| serde_json::to_string(s).unwrap();
    let src = format!(
        "func hint(f) {{\n    try {{\n        f()\n        print(\"no error\")\n    }} catch (e) {{\n        print(e.hint)\n    }}\n}}\n\
         hint(func() => read_file({m}))\nhint(func() => write_file({mo}, \"x\"))\nhint(func() => list_dir({md}))\nhint(func() => remove_file({m}))\n\
         hint(func() => exec([{mp}]))\nhint(func() => make_dir({mn}))\nprint(exists({mr}))\nprint(read_file({w}))\n\
         hint(func() => write_file(\"what?.txt\", \"x\"))\nhint(func() => read_file(\"a|b.txt\"))\nhint(func() => make_dir(\"aux\"))\n\
         hint(func() => list_dir(\"nul\"))\nhint(func() => write_file(\"sub./x.txt\", \"x\"))\nhint(func() => read_file(\"missing.txt\"))\n\
         hint(func() => write_file(\"sub/aux/x.txt\", \"x\"))\nhint(func() => list_dir(\"./con\"))\n\
         hint(func() => make_dir(\"./aux\"))\nhint(func() => read_file(\"aux/missing.txt\"))\n\
         try {{\n    read_file(\"aux/missing.txt\")\n}} catch (e) {{\n    print(\"no folder\" in e.message)\n}}\n",
        m = q(&format!("{msys}/data.txt")),
        mo = q(&format!("{msys}/out.txt")),
        md = q(&msys),
        mp = q(&format!("{msys}/prog.exe")),
        mn = q(&format!("{msys}/new/dir")),
        mr = q(&msys[..2]),
        w = q(&format!("{windows}/data.txt")),
    );
    std::fs::write(home.0.join("h.v2"), src).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["h.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    let refused = "Windows doesn't allow this file name (CON, PRN, AUX, NUL, COM1 to COM9 and LPT1 to LPT9 are devices; a name can't end in a dot or a space, or hold < > \" | ? *): choose another name";
    // Round 18 review: a device name is one only where Windows makes the path a device: on
    // Windows 11 a bare `aux`, but `sub\aux` and `.\con` are folders (Windows 10: devices).
    let device = |p: &std::path::Path| {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetFullPathNameW;
        let wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
        let mut buf = [0u16; 1024];
        // SAFETY: a NUL-terminated path and a buffer of the length given.
        let n = unsafe { GetFullPathNameW(wide.as_ptr(), buf.len() as u32, buf.as_mut_ptr(), std::ptr::null_mut()) } as usize;
        String::from_utf16_lossy(&buf[..n.min(buf.len())]).starts_with(r"\\.\")
    };
    assert!(device(std::path::Path::new("aux")), "a bare aux is a device");
    let in_sub = if device(&home.0.join("sub").join("aux")) { refused.to_string() } else { "make the folder first, with make_dir(\"sub/aux\")".to_string() };
    let dot_con = if device(&home.0.join("con")) { refused } else { "null" };
    // Round 18 review: Windows 11 reads aux/x.txt as a file in a folder aux, which ./aux makes:
    // no device hint and no "there is no folder 'aux'" for a file missing from it.
    let dot_aux = device(&home.0.join("aux"));
    let want = [
        format!("{spelling}{}", q(&format!("{windows}/data.txt"))),
        format!("{spelling}{}", q(&format!("{windows}/out.txt"))),
        format!("{spelling}{}", q(&windows)),
        format!("{spelling}{}", q(&format!("{windows}/data.txt"))),
        format!("{spelling}{}", q(&format!("{windows}/prog.exe"))),
        // make_dir would make \c\Users\... on this drive: refused, and nothing is made.
        format!("{spelling}{}", q(&format!("{windows}/new/dir"))),
        "false".into(),
        "here".into(),
        refused.into(),
        refused.into(),
        refused.into(),
        refused.into(),
        refused.into(),
        "null".into(),
        in_sub,
        dot_con.into(),
        if dot_aux { refused } else { "no error" }.into(),
        if dot_aux { refused } else { "null" }.into(),
        "false".into(),
    ];
    let got: Vec<_> = stdout.lines().collect();
    // Where ./aux is the device too (Windows 10), what its message says isn't checked.
    let checked = if dot_aux { want.len() - 1 } else { want.len() };
    assert_eq!(got[..checked.min(got.len())], want[..checked], "{stdout}");
}

/// The script that prints each call's value, or its error's message and hint, one per line.
#[cfg(windows)]
const SHOW: &str = "func show(f) {\n    try {\n        print(\"ok\", f())\n    } catch (e) {\n        print(e.message)\n        print(e.hint)\n    }\n}\n";

#[cfg(windows)]
#[test]
fn names_windows_would_read_as_another_are_refused() {
    // INTERNALS 5.10 (round 19): Windows drops a trailing dot or space, and makes CON or NUL a
    // device, so remove_file("data.txt.") deleted data.txt, write_file("report.") wrote report and
    // read_file("CON") waited for typed input. Every file builtin refuses them before the OS sees
    // them; a \\?\ path is taken as written, and the hint shows that way to a file of such a name
    // that is really there (WSL and Macs make them).
    let home = TempHome::new();
    std::fs::write(home.0.join("data.txt"), "keep me").unwrap();
    std::fs::write(format!(r"\\?\{}\notes.txt ", home.0.display()), "space content").unwrap();
    let verbatim = serde_json::to_string(&format!(r"\\?\{}\notes.txt ", home.0.display())).unwrap();
    let src = format!(
        "{SHOW}show(func() => remove_file(\"data.txt.\"))\nshow(func() => read_file(\"CON\"))\nshow(func() => write_file(\"report.\", \"x\"))\n\
         show(func() => write_file(\"other.txt \", \"x\"))\nshow(func() => write_file(\"nul\", \"x\"))\nshow(func() => write_file(\"sub/NUL\", \"x\"))\n\
         show(func() => exists(\"CON\"))\nshow(func() => list_dir(\"com1\"))\nshow(func() => make_dir(\"out.\"))\nshow(func() => read_file(\"notes.txt \"))\n\
         show(func() => read_file({verbatim}))\nprint(list_dir(\".\"))\n"
    );
    std::fs::write(home.0.join("w.v2"), src).unwrap();
    let start = Instant::now();
    let (code, stdout, stderr) = run_in_home(&home.0, &["w.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(start.elapsed() < Duration::from_secs(30), "read_file(\"CON\") waited: {:?}", start.elapsed());
    let refused = "Windows doesn't allow this file name (CON, PRN, AUX, NUL, COM1 to COM9 and LPT1 to LPT9 are devices; a name can't end in a dot or a space, or hold < > \" | ? *): choose another name";
    let dot = |verb: &str, name: &str, kept: &str| format!("cannot {verb} '{name}': a Windows file name can't end in a dot or a space ('{name}' would be '{kept}')");
    let device = |verb: &str, path: &str, name: &str| format!("cannot {verb} '{path}': {name} is the name of a device on Windows");
    let lines: Vec<&str> = stdout.lines().collect();
    let want = [
        dot("remove", "data.txt.", "data.txt"),
        refused.into(),
        device("read", "CON", "CON"),
        refused.into(),
        dot("write", "report.", "report"),
        refused.into(),
        dot("write", "other.txt ", "other.txt"),
        refused.into(),
        device("write", "nul", "nul"),
        refused.into(),
        device("write", "sub/NUL", "NUL"),
        refused.into(),
        device("check", "CON", "CON"),
        refused.into(),
        device("list", "com1", "com1"),
        refused.into(),
        dot("create", "out.", "out"),
        refused.into(),
        dot("read", "notes.txt ", "notes.txt"),
    ];
    assert_eq!(lines[..want.len().min(lines.len())], want, "{stdout}");
    // The hint for the name that is there: its full path with \\?\ in front, which reads it.
    let there = lines[want.len()];
    let start = "Windows doesn't allow this file name (CON, PRN, AUX, NUL, COM1 to COM9 and LPT1 to LPT9 are devices; a name can't end in a dot or a space, or hold < > \" | ? *), but one of that name is there (WSL and Macs can make one): reach it by its full path with \\\\?\\ in front, as in read_file(";
    let literal = there.strip_prefix(start).and_then(|r| r.strip_suffix(')')).unwrap_or_else(|| panic!("{there}"));
    let shown: String = serde_json::from_str(literal).unwrap();
    assert!(shown.starts_with(r"\\?\") && shown.ends_with(r"\notes.txt "), "{shown}");
    assert_eq!(std::fs::read_to_string(&shown).unwrap(), "space content");
    assert_eq!(lines[want.len() + 1..], ["ok space content", "[\"data.txt\", \"notes.txt \", \"w.v2\"]"], "{stdout}");
    assert_eq!(std::fs::read_to_string(home.0.join("data.txt")).unwrap(), "keep me");
    std::fs::remove_file(format!(r"\\?\{}\notes.txt ", home.0.display())).unwrap();
    // Round 19 review: a write or make_dir into a folder whose name ends in a dot, which WSL
    // made, gets the \\?\ hint too (the folder through the refused part is there), and the
    // call it shows works; with no such folder, "choose another name".
    std::fs::create_dir(format!(r"\\?\{}\dir.", home.0.display())).unwrap();
    let src = format!("{SHOW}show(func() => write_file(\"dir./new.txt\", \"n\"))\nshow(func() => make_dir(\"dir./sub\"))\nshow(func() => write_file(\"report./new.txt\", \"n\"))\n");
    std::fs::write(home.0.join("d.v2"), src).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["d.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 6, "{stdout}");
    let dot_in = |verb: &str, path: &str, part: &str| format!("cannot {verb} '{path}': a Windows file name can't end in a dot or a space ('{part}.' would be '{part}')");
    let want = [dot_in("write", "dir./new.txt", "dir"), dot_in("create", "dir./sub", "dir"), dot_in("write", "report./new.txt", "report"), refused.to_string()];
    assert_eq!([lines[0], lines[2], lines[4], lines[5]], want.each_ref().map(String::as_str), "{stdout}");
    let call = |line: &str, builtin: &str, end: &str| {
        let start = start.replace("read_file(", &format!("{builtin}("));
        let literal = line.strip_prefix(&start).and_then(|r| r.strip_suffix(')')).unwrap_or_else(|| panic!("{line}")).to_string();
        let shown: String = serde_json::from_str(&literal).unwrap();
        assert!(shown.starts_with(r"\\?\") && shown.ends_with(end), "{shown}");
        literal
    };
    let (w, m) = (call(lines[1], "write_file", r"\dir.\new.txt"), call(lines[3], "make_dir", r"\dir.\sub"));
    std::fs::write(home.0.join("h.v2"), format!("write_file({w}, \"n\")\nmake_dir({m})\nprint(read_file({w}))\n")).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["h.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(0), "n\n"), "{stderr}");
    assert!(Path::new(&format!(r"\\?\{}\dir.\sub", home.0.display())).is_dir());
    std::fs::remove_dir_all(format!(r"\\?\{}\dir.", home.0.display())).unwrap();
}

#[cfg(windows)]
#[test]
fn a_drive_or_share_that_isnt_there_is_named() {
    // INTERNALS 5.10 (round 19): a drive that isn't there (an unplugged USB drive) got "there is
    // no folder 'Q:\reports'" and a make_dir hint, and make_dir then said Rust's "failed to
    // create whole tree". Now every file builtin names the drive, or the share it can't reach.
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    let absent = ('D'..='Z').rev().find(|l| {
        let root: Vec<u16> = format!("{l}:\\").encode_utf16().chain([0]).collect();
        // SAFETY: a NUL-terminated root path. 1 is DRIVE_NO_ROOT_DIR.
        unsafe { GetDriveTypeW(root.as_ptr()) == 1 }
    });
    let Some(q) = absent else {
        let _ = writeln!(std::io::stderr(), "SKIP a_drive_or_share_that_isnt_there_is_named (every drive letter is in use)");
        return;
    };
    let lower = q.to_ascii_lowercase();
    let share = r"\\localhost\v2-no-such-share-r19";
    let src = format!(
        "{SHOW}show(func() => write_file(\"{q}:\\\\reports\\\\out.csv\", \"x\"))\nshow(func() => make_dir(\"{q}:\\\\reports\"))\nshow(func() => make_dir(\"{q}:/\"))\n\
         show(func() => read_file(\"{q}:/data.csv\"))\nshow(func() => list_dir(\"{q}:/\"))\nshow(func() => remove_file(\"{q}:/x.txt\"))\n\
         show(func() => write_file(\"{lower}:y.txt\", \"x\"))\nprint(exists(\"{q}:/x\"))\n\
         show(func() => read_file(\"\\\\\\\\localhost\\\\v2-no-such-share-r19\\\\x.txt\"))\nshow(func() => make_dir(\"//localhost/v2-no-such-share-r19/out\"))\n"
    );
    let home = TempHome::new();
    std::fs::write(home.0.join("d.v2"), src).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["d.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    let none = format!("there is no drive {q}: (is it plugged in or connected?)");
    let mut want: Vec<String> = Vec::new();
    for (verb, path) in [
        ("write", format!("{q}:\\reports\\out.csv")),
        ("create", format!("{q}:\\reports")),
        ("create", format!("{q}:/")),
        ("read", format!("{q}:/data.csv")),
        ("list", format!("{q}:/")),
        ("remove", format!("{q}:/x.txt")),
        ("write", format!("{lower}:y.txt")),
    ] {
        want.push(format!("cannot {verb} '{path}': {none}"));
        want.push("null".into());
    }
    want.push("false".into());
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[..want.len().min(lines.len())], want, "{stdout}");
    assert!(!stdout.contains("failed to create whole tree") && !stdout.contains("make_dir"), "{stdout}");
    // A share that can't be reached, as Windows reports it here (the Server service answers
    // ERROR_BAD_NET_NAME, 67; without it, ERROR_BAD_NETPATH, 53).
    let os = std::fs::metadata(format!(r"{share}\x.txt")).unwrap_err().raw_os_error();
    if !matches!(os, Some(53 | 67)) {
        let _ = writeln!(std::io::stderr(), "SKIP the share part of a_drive_or_share_that_isnt_there_is_named (Windows says {os:?})");
        return;
    }
    let cant = format!("the share {share} can't be reached (is that computer on and connected, and is the share's name right?)");
    let rest = [format!("cannot read '{share}\\x.txt': {cant}"), "null".into(), format!("cannot create '//localhost/v2-no-such-share-r19/out': {cant}"), "null".into()];
    assert_eq!(lines[want.len()..], rest, "{stdout}");
}

/// Folders a test could make outside its temp folder if what it tests is broken (`\tmp` at the
/// top of the drive): when dropped, after a failed assertion too, each one that wasn't there
/// when it was made is removed with what is in it; nothing that was there is touched.
#[cfg(windows)]
struct Leftovers(Vec<PathBuf>);

#[cfg(windows)]
impl Leftovers {
    fn new(paths: &[PathBuf]) -> Leftovers {
        Leftovers(paths.iter().filter(|p| std::fs::symlink_metadata(p).is_err()).cloned().collect())
    }
}

#[cfg(windows)]
impl Drop for Leftovers {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

#[cfg(windows)]
#[test]
fn tmp_on_windows_points_to_temp() {
    // INTERNALS 5.10 (round 19): /tmp is Linux's and macOS's; make_dir("/tmp") would make \tmp
    // at the top of the drive, which every user can write to. The hint says getenv("TEMP").
    // The decision itself is a unit test (builtins.rs), which never looks at a real drive: this
    // one runs only where the drive has neither folder, and removes what a broken build makes.
    let home = TempHome::new();
    let drive = home.0.to_string_lossy()[..2].to_string();
    if Path::new(&format!(r"{drive}\tmp")).exists() || Path::new(&format!(r"{drive}\var\tmp")).exists() {
        let _ = writeln!(std::io::stderr(), "SKIP tmp_on_windows_points_to_temp ({drive}\\tmp or {drive}\\var\\tmp is there)");
        return;
    }
    let _left = Leftovers::new(&[format!(r"{drive}\tmp"), format!(r"{drive}\var\tmp"), format!(r"{drive}\var")].map(PathBuf::from));
    let src = format!(
        "{SHOW}show(func() => write_file(\"/tmp/x.txt\", \"x\"))\nshow(func() => read_file(\"/var/tmp/x.txt\"))\nshow(func() => list_dir(\"/tmp\"))\n\
         show(func() => make_dir(\"/tmp/work\"))\nshow(func() => make_dir(\"/var/tmp/x\"))\nprint(exists(\"/tmp\"), exists(\"/var/tmp\"))\n"
    );
    std::fs::write(home.0.join("t.v2"), src).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["t.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    let temp = |tmp: &str, rest: &str| format!("{tmp} is Linux's and macOS's temp folder: on Windows use getenv(\"TEMP\"), as in {rest}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "cannot write '/tmp/x.txt': there is no folder '/tmp'", "{stdout}");
    assert_eq!(lines[1], temp("/tmp", "getenv(\"TEMP\") + \"/x.txt\""), "{stdout}");
    assert_eq!(lines[3], temp("/var/tmp", "getenv(\"TEMP\") + \"/x.txt\""), "{stdout}");
    assert_eq!(lines[5], temp("/tmp", "getenv(\"TEMP\")"), "{stdout}");
    assert_eq!(lines[6], "cannot create '/tmp/work': on Windows that makes \\tmp at the top of the drive, a folder other users can write to", "{stdout}");
    assert_eq!(lines[7], temp("/tmp", "getenv(\"TEMP\") + \"/work\""), "{stdout}");
    // Round 19 review: /var/tmp names the folder it would make, \var\tmp.
    assert_eq!(lines[8], "cannot create '/var/tmp/x': on Windows that makes \\var\\tmp at the top of the drive, a folder other users can write to", "{stdout}");
    assert_eq!(lines[9..], [temp("/var/tmp", "getenv(\"TEMP\") + \"/x\""), "false false".into()], "{stdout}");
}

#[test]
fn make_dir_with_a_nul_gives_the_true_reason() {
    // Round 19 review (INTERNALS 5.10): make_dir took every error without an OS code for
    // create_dir_all's "failed to create whole tree", so a NUL in the path said "the drive or
    // share it is on isn't there" (an absolute path, on Linux too) or "The system cannot find
    // the path specified" (a relative one). It says Rust's reason, as write_file does.
    let home = TempHome::new();
    let rust = std::fs::create_dir_all("a\0b").unwrap_err();
    assert_eq!((rust.kind(), rust.raw_os_error()), (std::io::ErrorKind::InvalidInput, None), "{rust}");
    let reason = rust.to_string().trim_end_matches('.').to_string();
    let abs = home.0.to_string_lossy().replace('\\', "/");
    let src = format!(
        "func why(f) {{\n    try {{\n        f()\n        print(\"no error\")\n    }} catch (e) {{\n        print(e.message)\n    }}\n}}\n\
         why(func() => make_dir(\"a\\0b\"))\nwhy(func() => make_dir(\"{abs}/a\\0b\"))\nwhy(func() => make_dir(\"{abs}/new/a\\0b\"))\nwhy(func() => write_file(\"a\\0b\", \"x\"))\n"
    );
    std::fs::write(home.0.join("n.v2"), src).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["n.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    let want = [
        format!("cannot create 'a\\u{{0}}b': {reason}"),
        format!("cannot create '{abs}/a\\u{{0}}b': {reason}"),
        format!("cannot create '{abs}/new/a\\u{{0}}b': {reason}"),
        format!("cannot write 'a\\u{{0}}b': {reason}"),
    ];
    assert_eq!(stdout.lines().collect::<Vec<_>>(), want, "{stdout}");
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 1, "only n.v2 is there");
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn list_dir_names_the_name_that_isnt_utf8() {
    // 5.10: the name, with U+FFFD for what isn't UTF-8 (Linux: a byte; Windows: a lone
    // surrogate). macOS refuses such names.
    let home = TempHome::new();
    std::fs::create_dir_all(home.0.join("names")).unwrap();
    #[cfg(unix)]
    let bad = <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(b"bad\xff.txt").to_os_string();
    #[cfg(windows)]
    let bad = <std::ffi::OsString as std::os::windows::ffi::OsStringExt>::from_wide(&[0x62, 0x61, 0x64, 0xD800, 0x2E, 0x74, 0x78, 0x74]);
    std::fs::write(home.0.join("names").join(bad), "x").unwrap();
    std::fs::write(home.0.join("l.v2"), "print(list_dir(\"names\"))\n").unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &["l.v2"], &[]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("IOError: cannot list 'names': the name 'bad\u{FFFD}.txt' is not valid UTF-8 (\u{FFFD} marks what isn't), and V2's strings are: rename it"), "{stderr}");
    // The hint's code runs as written, and the bytes name it gives opens the file.
    let hint = "keep them as bytes in a @py block: @py(d) { import os; os.listdir(os.fsencode(d)) }, and open one there with os.path.join(os.fsencode(d), name)";
    assert!(stderr.contains(hint), "{stderr}");
    let src = "if engine_available(\"py\") {\n    let d = \"names\"\n    let name = (@py(d) { import os; os.listdir(os.fsencode(d)) })[0]\n    print(@py(d, name) { import os; open(os.path.join(os.fsencode(d), name), \"rb\").read() })\n} else {\n    print(\"no py\")\n}\n";
    std::fs::write(home.0.join("h.v2"), src).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["h.v2"], &[]);
    let required = std::env::var("V2_TEST_REQUIRE").unwrap_or_default().split([',', ' ']).any(|t| t == "py");
    if stdout != "no py\n" || required {
        assert_eq!((code, stdout.as_str()), (Some(0), "b\"x\"\n"), "{stderr}");
    }
}

#[test]
fn blocks_run_in_a_deep_temp_folder() {
    // 20.3: a temp folder whose path leaves too little room for the run dir's files under
    // Windows' 260 characters: the run dir is still made (a `\\?\` path) and used (with long
    // paths on, or by the folder's short name), so blocks run, and doctor sees no problem.
    if !needs("py", "blocks_run_in_a_deep_temp_folder") {
        return;
    }
    let home = TempHome::new();
    let mut tmp = home.0.join("tmp");
    while tmp.as_os_str().len() < 230 {
        tmp.push("a folder with a long name");
    }
    std::fs::write(home.0.join("deep.v2"), "print(@py { 6 * 7 })\n").unwrap();
    // A Windows with long paths off and no 8.3 names on that volume has no way to fit: then the
    // run and doctor say how long the folder is and what to do.
    let too_long = format!("TMP names {}, which is {} characters long: too long for the files blocks need there", tmp.display(), tmp.display().to_string().chars().count());
    let mut ran = None;
    for args in [&["deep.v2"][..], &["doctor"]] {
        let mut cmd = v2_command(&home.0);
        with_temp(&mut cmd, &tmp);
        let out = cmd.args(args).current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if args == ["deep.v2"] {
            ran = Some(stdout == "42\n");
            assert!(stdout == "42\n" || (cfg!(windows) && stderr.contains(&too_long)), "{stdout}{stderr}");
        } else if ran == Some(true) {
            assert!(!stdout.contains("temp: "), "{stdout}");
        } else {
            assert!(stdout.contains(&format!("temp: {too_long}")), "{stdout}");
        }
    }
    assert!(names_in(&tmp).is_empty(), "the run dir is removed");
}

#[test]
fn a_home_folder_that_doesnt_exist_gets_no_cache() {
    // 18.1: no folder is made for a HOME that isn't there.
    let home = TempHome::new();
    let missing = home.0.join("nonexistent");
    std::fs::write(home.0.join("p.v2"), "print(engine_available(\"py\"))\n").unwrap();
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let (code, _out, err) = run_in_home(&home.0, &["p.v2"], &[(var, missing.as_os_str())]);
    assert_eq!(code, Some(0), "{err}");
    assert!(!missing.exists(), "{} was created", missing.display());
}

#[test]
fn a_relative_path_next_to_the_script_gets_a_hint() {
    // 5.10: run from another folder, a relative path misses the file beside the script.
    let home = TempHome::new();
    std::fs::create_dir_all(home.0.join("proj")).unwrap();
    std::fs::write(home.0.join("proj/data.csv"), "a,b\n").unwrap();
    std::fs::write(home.0.join("proj/rel.v2"), "print(read_file(\"data.csv\"))\n").unwrap();
    let (code, _out, err) = run_in_home(&home.0, &["proj/rel.v2"], &[]);
    assert_eq!(code, Some(1), "{err}");
    let hint = "\n  hint: relative paths start in the folder you ran v2 from; for the file next to the script: read_file(f\"${script_dir()}/data.csv\")\n";
    assert!(err.contains("IOError: cannot read 'data.csv': ") && err.contains(hint), "{err}");
}

#[test]
fn a_path_with_a_control_character_gets_the_escape_hint() {
    // 5.10: "notes\new.txt" holds a line break: the message shows it escaped, with a hint.
    let (code, _out, err, _home) = run_with_files(&[("p.v2", "read_file(\"notes\\new.txt\")\n")], &["p.v2"]);
    assert_eq!(code, Some(1), "{err}");
    assert!(err.starts_with("p.v2:1:1: IOError: cannot read 'notes\\new.txt': "), "{err}");
    let hint = "\n  hint: in \"…\", a backslash starts an escape (\\n is a line break, \\t a tab): write Windows paths with / or \\\\, as in \"notes/new.txt\"\n";
    assert!(err.contains(hint), "{err}");
}

#[test]
fn doctor_calls_node_23_1_unsupported() {
    // 19.2: 23.0 and 23.1 are newer than the minimum but can't run ts: `unsupported`, not `old`.
    let home = TempHome::new();
    let bin = home.0.join("fake bin");
    std::fs::create_dir_all(&bin).unwrap();
    let node = bin.join(if cfg!(windows) { "node.cmd" } else { "node" });
    std::fs::write(&node, if cfg!(windows) { "@echo v23.1.0\r\n" } else { "#!/bin/sh\necho v23.1.0\n" }).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (code, stdout, stderr) = run_in_home(&home.0, &["doctor"], &[("PATH", bin.as_os_str())]);
    assert_eq!(code, Some(0), "{stderr}");
    let rows = doctor_rows(&stdout);
    let node = node.display().to_string();
    assert_eq!(rows["js"], ["unsupported", node.as_str(), "23.1.0", "PATH"], "{stdout}");
    assert!(stdout.contains("\n  js: not supported: found 23.1.0, which has no type stripping; V2 needs 22.13+, not 23.0 or 23.1; install: "), "{stdout}");
    assert!(!stdout.contains("js: skipped"), "{stdout}");
}

#[cfg(unix)]
#[test]
fn a_tilde_path_entry_is_named_by_doctor_and_engine_not_found() {
    // Round 19, 7.5: export PATH="~/.nvm/…/bin:$PATH" (quoted, so not expanded): only bash's own
    // lookup expands ~, so V2 skips the entry as relative, as execvp and dash do, and says so.
    use std::os::unix::fs::PermissionsExt;
    let home = TempHome::new();
    let (nvm, system) = (home.0.join(".nvm/v24/bin"), home.0.join("system"));
    for (dir, version) in [(&nvm, "v24.1.0"), (&system, "v22.13.0")] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("node"), format!("#!/bin/sh\necho {version}\n")).unwrap();
        std::fs::set_permissions(dir.join("node"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let why = "(its PATH entry starts with ~, which only bash expands: write $HOME/.nvm/v24/bin in your startup file)";
    let path = format!("~/.nvm/v24/bin:{}", system.display());
    let (code, stdout, stderr) = run_in_home(&home.0, &["doctor"], &[("PATH", std::ffi::OsStr::new(&path))]);
    assert_eq!(code, Some(0), "{stderr}");
    let used = system.join("node").display().to_string();
    assert_eq!(doctor_rows(&stdout)["js"], ["ok", used.as_str(), "22.13.0", "PATH"], "{stdout}");
    assert!(stdout.contains(&format!("\n  js: skipped ~/.nvm/v24/bin/node {why}\n")), "{stdout}");
    // After the one V2 uses, bash runs that one too: nothing to say.
    let after = format!("{}:~/.nvm/v24/bin", system.display());
    let (_, stdout, _) = run_in_home(&home.0, &["doctor"], &[("PATH", std::ffi::OsStr::new(&after))]);
    assert!(!stdout.contains("~/.nvm"), "{stdout}");
    // Nothing else on PATH: EngineNotFound's skipped line.
    std::fs::write(home.0.join("j.v2"), "@js { 1 }\n").unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &["j.v2"], &[("PATH", std::ffi::OsStr::new("~/.nvm/v24/bin"))]);
    assert_eq!(code, Some(3), "{stderr}");
    assert!(stderr.contains("EngineNotFound: engine 'js' (JavaScript) not found\n"), "{stderr}");
    assert!(stderr.contains(&format!("\n    tried: node on PATH\n    skipped: ~/.nvm/v24/bin/node {why}\n    install: ")), "{stderr}");
}

#[test]
fn a_conda_run_command_is_told_to_pass_input_and_output_through() {
    // Round 19, 8.4: `conda run` keeps the program's standard input and output unless given
    // --no-capture-output, so the worker ends with code 0 before its hello, with nothing above
    // the message. This conda answers --version as the environment's Python, then exits at once.
    let home = TempHome::new();
    let bin = home.0.join("conda").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let conda = bin.join(if cfg!(windows) { "conda.cmd" } else { "conda" });
    if cfg!(windows) {
        std::fs::write(&conda, "@echo off\r\nfor %%a in (%*) do if \"%%~a\"==\"--version\" (echo Python 3.12.1& exit /b 0)\r\nexit /b 0\r\n").unwrap();
    } else {
        std::fs::write(&conda, "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = --version ] && { echo Python 3.12.1; exit 0; }; done\nexit 0\n").unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&conda, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(home.0.join("s.v2"), "@py { 1 }\n").unwrap();
    let general = "EngineCrashed: engine 'py' exited during startup (exit code 0); see its error output above, if any; it ended without a word to V2, which talks to the program over its standard input and output: a command that runs it through another program must pass them through";
    let python = if cfg!(windows) { "envs\\sales\\python.exe" } else { "envs/sales/bin/python" };
    let conda_line = format!("; conda run passes them through only with --no-capture-output: add it after run, or name the environment's python ({python} in conda's folder) in [engines.py] command\n");
    for (words, named) in [("'run', '-n', 'sales', 'python'", true), ("'run', '--no-capture-output', '-n', 'sales', 'python'", false)] {
        std::fs::write(home.0.join("v2.toml"), format!("[engines.py]\ncommand = ['{}', {words}]\n", conda.display())).unwrap();
        let (code, _, stderr) = run_in_home(&home.0, &["s.v2"], &[]);
        assert_eq!(code, Some(1), "{stderr}");
        let want = if named { format!("{general}{conda_line}") } else { format!("{general}\n") };
        assert!(stderr.starts_with("s.v2:1:1: ") && stderr.contains(&want), "{words}: {stderr}");
    }
}

/// A `conda` in `home`/conda/bin that answers `--version` as the environment's program (Node.js
/// when the words have `node`, else Python), then exits at once, as `conda run` does to a worker
/// whose protocol it keeps or closes.
fn fake_conda(home: &Path) -> PathBuf {
    let bin = home.join("conda").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let conda = bin.join(if cfg!(windows) { "conda.cmd" } else { "conda" });
    if cfg!(windows) {
        let script = "@echo off\r\nset v2_node=\r\nfor %%a in (%*) do if \"%%~a\"==\"node\" set v2_node=1\r\nfor %%a in (%*) do if \"%%~a\"==\"--version\" goto version\r\nexit /b 0\r\n:version\r\nif defined v2_node (echo v22.14.0) else (echo Python 3.12.1)\r\nexit /b 0\r\n";
        std::fs::write(&conda, script).unwrap();
    } else {
        let script = "#!/bin/sh\nnode=\nfor a in \"$@\"; do [ \"$a\" = node ] && node=1; done\nfor a in \"$@\"; do\n  if [ \"$a\" = --version ]; then\n    if [ -n \"$node\" ]; then echo v22.14.0; else echo Python 3.12.1; fi\n    exit 0\n  fi\ndone\nexit 0\n";
        std::fs::write(&conda, script).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&conda, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    conda
}

#[test]
fn doctor_and_a_js_run_name_a_conda_run_that_cant_work() {
    // Review of round 19, 8.4 and 19.2: doctor gives the run's advice from the configured
    // command's words; js on Unix talks over two pipes of its own (V2_VEP_FDS), which conda run
    // closes even with --no-capture-output, so naming the environment's node is the fix there.
    let home = TempHome::new();
    let conda = fake_conda(&home.0);
    let toml = |py: &str, js: &str| format!("[engines.py]\ncommand = ['{0}', {py}]\n[engines.js]\ncommand = ['{0}', {js}]\n", conda.display());
    let (captured, passed) = ("'run', '-n', 'sales', 'python'", "'run', '--no-capture-output', '-n', 'sales', 'python'");
    let (node_captured, node_passed) = ("'run', '-n', 'web', 'node'", "'run', '--no-capture-output', '-n', 'web', 'node'");
    let python = if cfg!(windows) { "envs\\sales\\python.exe" } else { "envs/sales/bin/python" };
    let py_note = format!("\n  py: V2 talks to the program over its standard input and output, and conda run passes them through only with --no-capture-output: add it after run, or name the environment's python ({python} in conda's folder) in [engines.py] command\n");
    let pipes = "\n  js: V2 talks to the program over two pipes it inherits (named in V2_VEP_FDS), and conda run closes them, even with --no-capture-output: name the environment's node (envs/web/bin/node in conda's folder) in [engines.js] command\n";
    let node_io = "\n  js: V2 talks to the program over its standard input and output, and conda run passes them through only with --no-capture-output: add it after run, or name the environment's node in [engines.js] command\n";
    for (py, js, py_told, js_note) in [(captured, node_passed, true, cfg!(unix).then_some(pipes)), (passed, node_captured, false, Some(if cfg!(unix) { pipes } else { node_io }))] {
        std::fs::write(home.0.join("v2.toml"), toml(py, js)).unwrap();
        let (code, stdout, stderr) = run_in_home(&home.0, &["doctor"], &[]);
        assert_eq!(code, Some(0), "{stderr}");
        let rows = doctor_rows(&stdout);
        assert_eq!((&*rows["py"][0], &*rows["js"][0]), ("ok", "ok"), "{stdout}");
        assert_eq!(stdout.contains(&py_note), py_told, "{py}: {stdout}");
        match js_note {
            Some(note) => assert!(stdout.contains(note), "{js}: {stdout}"),
            None => assert!(!stdout.contains("js: V2 talks"), "{js}: {stdout}"),
        }
        assert!(!stdout.contains("ts: V2 talks"), "{stdout}");
    }
    // The run's words, js's channel on each OS.
    std::fs::write(home.0.join("v2.toml"), toml(captured, if cfg!(unix) { node_passed } else { node_captured })).unwrap();
    std::fs::write(home.0.join("j.v2"), "@js { 1 }\n").unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &["j.v2"], &[]);
    assert_eq!(code, Some(1), "{stderr}");
    let want = if cfg!(unix) {
        "EngineCrashed: engine 'js' exited during startup (exit code 0); see its error output above, if any; it ended without a word to V2, which talks to the program over two pipes it inherits (named in V2_VEP_FDS): a command that runs it through another program must leave them open; conda run closes them, even with --no-capture-output: name the environment's node (envs/web/bin/node in conda's folder) in [engines.js] command"
    } else {
        "EngineCrashed: engine 'js' exited during startup (exit code 0); see its error output above, if any; it ended without a word to V2, which talks to the program over its standard input and output: a command that runs it through another program must pass them through; conda run passes them through only with --no-capture-output: add it after run, or name the environment's node in [engines.js] command"
    };
    assert!(stderr.starts_with("j.v2:1:1: ") && stderr.contains(want), "{stderr}");
}

#[test]
fn doctor_quotes_a_configured_command_with_a_space() {
    // 19.2: an item with a space is quoted when there is more than one, also when it isn't found.
    let files = [("v2.toml", "[engines.py]\ncommand = [\"no such/python\", \"-X\", \"utf8\"]\n")];
    let (code, stdout, stderr, _home) = run_with_files(&files, &["doctor"]);
    assert_eq!(code, Some(0), "{stderr}");
    let command = &doctor_rows(&stdout)["py"][1];
    assert!(command.starts_with('"') && command.ends_with("python\" -X utf8"), "{stdout}");
}

#[cfg(windows)]
#[test]
fn doctor_never_names_a_store_placeholder_as_the_broken_program() {
    // 19.2 and 7.8: a broken row's COMMAND is the program that doesn't run, even when a Store
    // placeholder comes first on PATH.
    let home = TempHome::new();
    let apps = home.0.join("lad").join("Microsoft").join("WindowsApps");
    let bin = home.0.join("b");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(apps.join("python3.cmd"), "@echo Python was not found; run without arguments to install from the Microsoft Store\r\n@exit /b 9009\r\n").unwrap();
    std::fs::write(bin.join("python.cmd"), "@echo No global/local python version has been set yet.\r\n@exit /b 1\r\n").unwrap();
    let path = std::env::join_paths([&apps, &bin]).unwrap();
    let lad = home.0.join("lad");
    let (code, stdout, stderr) = run_in_home(&home.0, &["doctor"], &[("PATH", path.as_os_str()), ("LOCALAPPDATA", lad.as_os_str())]);
    assert_eq!(code, Some(0), "{stderr}");
    let rows = doctor_rows(&stdout);
    assert_eq!(rows["py"][..2], ["broken".to_string(), bin.join("python.cmd").display().to_string()], "{stdout}");
    assert!(stdout.contains("(Microsoft Store placeholder)"), "{stdout}");
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
    if !needs("py", "runs_use_the_probe_cache_and_doctor_probes_afresh") || !needs("js", "runs_use_the_probe_cache_and_doctor_probes_afresh") {
        return;
    }
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
    let starts: Vec<usize> = ["ENGINE", "STATUS", "VERSION", "FROM", "COMMAND"].iter().map(|h| header.find(h).unwrap()).collect();
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
    // With no PATH at all, installing wouldn't help: nothing was searched (19.2).
    let (_, unset, _) = run_in_home(&home.0, &["doctor"], &[("PATH", std::ffi::OsStr::new(""))]);
    assert!(unset.contains("\n  py: not found: PATH isn't set, so nothing was searched; set it, or name the program in v2.toml, [engines.py] command\n"), "{unset}");
    assert!(unset.contains("\n  ts: not found: PATH isn't set, so nothing was searched; set it, or name the program in v2.toml, [engines.js] command\n"), "{unset}");
    // Then the version line for bug reports and what it means for the user's scripts (19.2).
    let end = format!(
        "config file: none\nv2 {} (VEP/1), {} {}\nready: none\nnot ready: py, js, ts (see the notes); plain V2 scripts still run\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    assert!(stdout.ends_with(&end), "{stdout}");
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
    let has_cols = needs("py", "an_uncaught_foreign_error_prints_like_docs_10") && {
        let (code, out, err) = run_generated("cols.v2", "print(@py { import sys; sys.version_info >= (3, 11) })\n");
        assert_eq!(code, Some(0), "{err}");
        out == "true\n"
    };
    let py_expected = if has_cols {
        "main.v2:3:16: ForeignError: ZeroDivisionError: division by zero\n    return a / b\n           ^\n  at ratio (main.v2:3, in @py risky)\n  at main.v2:9\n"
    } else {
        "main.v2:3:9: ForeignError: ZeroDivisionError: division by zero\n    return a / b\n    ^\n  at ratio (main.v2:3, in @py risky)\n  at main.v2:9\n"
    };
    let cases = [
        ("py", py, py_expected),
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

#[test]
fn jsx_files_run_through_a_loader_from_node_options() {
    // DOCS 7, INTERNALS 13.5: a .jsx or .tsx file goes to Node like any other, so a loader that
    // NODE_OPTIONS registers (--require @babel/register) runs it, for an @import and a block's
    // require alike, and a syntax error it leaves gets no JSX hint. Without one: the hint.
    if !needs("js", "jsx_files_run_through_a_loader_from_node_options") {
        return;
    }
    let hook = r#"const fs = require('fs');
for (const ext of ['.jsx', '.tsx']) {
  require.extensions[ext] = (m, file) => m._compile(fs.readFileSync(file, 'utf8').replace(/<b>(\w+)<\/b>/g, '"$1"').replace(/: number/g, ''), file);
}
"#;
    let main = r#"print(@js { require("./comp.jsx")() })
@import js."./comp.jsx" as comp
print(comp())
@import { typed } from ts."./typed.tsx"
print(typed(2))
try {
    print(@js { require("./bad.jsx") })
} catch (e) {
    print(e.type, "|", e.hint)
}
"#;
    let home = TempHome::new();
    for (path, text) in [
        ("hook.cjs", hook),
        ("comp.jsx", "module.exports = () => <b>hi</b>;\n"),
        ("typed.tsx", "exports.typed = (n: number) => n * 2;\n"),
        ("bad.jsx", "module.exports = <i>x</i>;\n"),
        ("main.v2", main),
    ] {
        std::fs::write(home.0.join(path), text).unwrap();
    }
    let run = |node_options: Option<&str>| {
        let mut cmd = v2_command(&home.0);
        cmd.arg("main.v2").current_dir(&home.0).env_remove("NODE_OPTIONS");
        if let Some(o) = node_options {
            cmd.env("NODE_OPTIONS", o);
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
        (out.status.code(), normalize(&String::from_utf8_lossy(&out.stdout)), normalize(&String::from_utf8_lossy(&out.stderr)))
    };
    let (code, stdout, stderr) = run(Some("--require ./hook.cjs"));
    assert_eq!((code, stdout.as_str()), (Some(0), "hi\nhi\n4\nForeignError | null\n"), "{stderr}");
    let (code, stdout, stderr) = run(None);
    let hint = "hint: Node has no JSX, so it can't run .jsx or .tsx files: compile comp.jsx first";
    assert!(code == Some(1) && stdout.is_empty() && stderr.contains(hint), "{stdout}{stderr}");
}

#[test]
fn a_nested_typescript_file_gets_its_own_module_form() {
    // INTERNALS 12.5 (round 18 review): import = and export = are written as the file that
    // failed takes them: an .mts or .cts file that a .ts file imports gets its own form where
    // Node names that file (24); Node 22 names none, and the imported .ts file's form is left.
    if !needs("ts", "a_nested_typescript_file_gets_its_own_module_form") {
        return;
    }
    let stripped = "Node runs TypeScript by removing its types, and ";
    let cases = [
        ("outer_m.ts", "export { f } from \"./inner.mts\";\n", "inner.mts", "import fs = require(\"fs\");\nexport const f = 1;\n", "import x = require(…) is code: write import x from \"…\""),
        ("outer_c.ts", "import inner from \"./inner.cts\";\nexport const f = inner.g;\n", "inner.cts", "const g = 2;\nexport = { g };\n", "export = is code: write module.exports = x"),
    ];
    for (outer, outer_src, inner, inner_src, form) in cases {
        let main = format!("@import {{ f }} from ts.\"./{outer}\"\nprint(f)\n");
        let (code, stdout, stderr, _home) = run_with_files(&[("main.v2", &*main), (outer, outer_src), (inner, inner_src)], &["main.v2"]);
        assert_eq!(code, Some(1), "{stdout}{stderr}");
        let hint = stderr.lines().find_map(|l| l.trim().strip_prefix("hint: ")).unwrap_or_else(|| panic!("{stderr}"));
        // Where a frame names the inner file, the hint is that file's; else the .ts file's.
        if stderr.contains(&format!("{inner}:")) {
            assert_eq!(hint, format!("{stripped}{form}"), "{stderr}");
        } else {
            assert!(hint.starts_with(stripped) && hint.contains("in CommonJS, or"), "{stderr}");
        }
    }
}

#[test]
fn ts_module_keyword_gets_the_hint_of_its_message() {
    // INTERNALS 12.5: Node 24 refuses `module M` for its keyword, and Node 22.13 as a namespace,
    // as it does a namespace with only types, which Node 24 runs. Each message gets its own hint.
    if !needs("ts", "ts_module_keyword_gets_the_hint_of_its_message") {
        return;
    }
    let main = r#"func show(f) {
    try {
        print(f())
    } catch (e) {
        print(e.message, "=>", e.hint)
    }
}
show(func() {
    return @ts {
        module M { export type T = number }
        1
    }
})
show(func() {
    @import { x } from ts."./mk.ts"
    return x
})
show(func() {
    return @ts {
        module V { export const x = 1 }
        V.x
    }
})
show(func() {
    return @ts {
        namespace N { export type T = number }
        2
    }
})
"#;
    let mk = "module M { export type T = number }\nexport const x = 1;\n";
    let (code, stdout, stderr, _home) = run_with_files(&[("main.v2", main), ("mk.ts", mk)], &["main.v2"]);
    assert_eq!(code, Some(0), "{stderr}");
    let module = "write namespace M for module M: Node's type stripping takes only that spelling, and only for a namespace of types (one with values is code: use a module of its own (a file) or an object, as in const NS = { x: 1 })";
    let namespace = "Node runs TypeScript by removing its types, and a namespace with values is code (an older Node, such as 22.13, refuses even one with only types): use a module of its own (a file) or an object, as in const NS = { x: 1 }";
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 4, "{stdout}");
    // Round 18 review: a module with values is refused either way, and the hint says so first.
    for line in &lines[..3] {
        let (message, hint) = line.split_once(" => ").unwrap_or_else(|| panic!("{stdout}"));
        if message.contains("`module` keyword is not supported") {
            assert_eq!(hint, module, "{stdout}");
        } else {
            assert!(message.contains("namespace declaration is not supported"), "{stdout}");
            assert_eq!(hint, namespace, "{stdout}");
        }
    }
    let refused = lines[3].contains("namespace declaration is not supported") && lines[3].ends_with(&format!(" => {namespace}"));
    assert!(lines[3] == "2" || refused, "{stdout}");
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
fn a_python_venv_from_v2_toml_is_used_unless_v2_engine_py_is_set() {
    // DOCS 5, 7, 14: `[python] venv`, relative to the v2.toml, gives blocks that venv's Python and
    // its packages; `V2_ENGINE_PY` wins over it, and `[engines.py] command` beside it in v2.toml
    // is a configuration error (INTERNALS 18.3).
    if !needs("py", "a_python_venv_from_v2_toml_is_used_unless_v2_engine_py_is_set") {
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
    let (code, _, stderr) = run_in_home(&home.0, &["sub/main.v2"], &[]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("config error: [python] venv and [engines.py] command both choose the Python: keep one"), "{stderr}");
    std::fs::write(home.0.join("v2.toml"), "[python]\nvenv = \".venv\"\n").unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["sub/main.v2"], &[("V2_ENGINE_PY", std::ffi::OsStr::new(&python))]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_ne!(stdout, ".venv\n", "V2_ENGINE_PY lost to the venv");
}

#[test]
fn a_venv_made_with_another_python_is_broken_and_doctor_says_venv() {
    // Round 19 (L0, L9): a venv whose pyvenv.cfg names another Python than its python now runs
    // (an unversioned python3 an upgrade repointed) is EngineNotFound, exit 3, and doctor's row
    // is broken, FROM venv, as it is for a venv that isn't there (INTERNALS 7.6, 19.2).
    let home = TempHome::new();
    std::fs::write(home.0.join("v2.toml"), "[python]\nvenv = \".nope\"\n").unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["doctor"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    let py = &doctor_rows(&stdout)["py"];
    assert_eq!((py[0].as_str(), py[3].as_str()), ("broken", "venv"), "{stdout}");
    if !needs("py", "a_venv_made_with_another_python_is_broken_and_doctor_says_venv") {
        return;
    }
    let python = block_python();
    let made = Command::new(&python).args(["-m", "venv", "--without-pip"]).arg(home.0.join(".venv")).output().expect("run python -m venv");
    assert!(made.status.success(), "python -m venv: {}", String::from_utf8_lossy(&made.stderr));
    // Python 3.1 made it, says its pyvenv.cfg.
    let cfg = home.0.join(".venv").join("pyvenv.cfg");
    let text = std::fs::read_to_string(&cfg).unwrap();
    let older: String = text
        .lines()
        .map(|l| match l.split_once('=') {
            Some((key, _)) if key.trim().starts_with("version") => format!("{} = 3.1.0\n", key.trim()),
            _ => format!("{l}\n"),
        })
        .collect();
    assert!(older.contains("version = 3.1.0"), "{text}");
    std::fs::write(&cfg, older).unwrap();
    std::fs::write(home.0.join("v2.toml"), "[python]\nvenv = \".venv\"\n").unwrap();
    std::fs::write(home.0.join("m.v2"), "print(@py { 1 + 1 })\n").unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["m.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(3), ""), "{stderr}");
    assert!(stderr.contains("EngineNotFound: [python] venv '") && stderr.contains("' was made with Python 3.1 (its pyvenv.cfg says so), but its python now runs 3."), "{stderr}");
    assert!(stderr.contains(" (was Python upgraded?): make the venv again, ") && stderr.contains(" -m venv --clear "), "{stderr}");
    let (_, stdout, _) = run_in_home(&home.0, &["doctor"], &[]);
    let py = &doctor_rows(&stdout)["py"];
    assert_eq!((py[0].as_str(), py[3].as_str()), ("broken", "venv"), "{stdout}");
    assert!(stdout.contains("py: [python] venv '") && stdout.contains("' was made with Python 3.1 (its pyvenv.cfg says so)"), "{stdout}");
}

#[test]
fn an_activated_venv_whose_python_is_gone_is_named_by_doctor_and_the_hint() {
    // Round 19 (L4): VIRTUAL_ENV names a venv whose python links to an uninstalled Python (or is
    // missing), which the PATH search can't see: doctor's notes say so, and an ImportError's hint
    // says it instead of pip for the Python found on PATH (INTERNALS 13.5, 19.2).
    let home = TempHome::new();
    let venv = home.0.join("proj").join(".venv");
    std::fs::create_dir_all(venv.join(if cfg!(windows) { "Scripts" } else { "bin" })).unwrap();
    std::fs::write(venv.join("pyvenv.cfg"), "home = /nonexistent\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/nonexistent/python3.99", venv.join("bin").join("python3")).unwrap();
    let env = [("VIRTUAL_ENV", venv.as_os_str())];
    let what = if cfg!(windows) { r"has no Scripts\python.exe" } else { "has a python that links to /nonexistent/python3.99, which isn't there (was Python upgraded?)" };
    let problem = format!("the activated venv {} (VIRTUAL_ENV) {what}, so V2 can't use it: make the venv again, ", venv.display());
    let (code, stdout, stderr) = run_in_home(&home.0, &["doctor"], &env);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.contains(&format!("\n  py: {problem}")), "{stdout}");
    if !needs("py", "an_activated_venv_whose_python_is_gone_is_named_by_doctor_and_the_hint") {
        return;
    }
    std::fs::write(home.0.join("proj").join("m.v2"), "@import py.nosuch_r19_pkg as p\nprint(p.x)\n").unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &["proj/m.v2"], &env);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains(&format!("hint: {problem}")), "{stderr}");
    assert!(!stderr.contains("pip") && !stderr.contains("which V2 isn't using"), "{stderr}");
}

#[test]
fn an_activated_venv_gets_its_own_install_line_until_its_python_stops_running() {
    // Round 19 review (P1, P2; INTERNALS 7.6, 7.7, 13.5): an activated venv (VIRTUAL_ENV, its
    // bin first on PATH) is the Python V2 uses, so a missing package gets that venv's own install
    // line. Once its python no longer runs (on Windows, the copied launcher of a venv whose home
    // is gone says "No Python at", exit 103), the search finds another Python, and doctor and the
    // hint say what is wrong with the venv, though the first run cached its python as usable.
    if !needs("py", "an_activated_venv_gets_its_own_install_line_until_its_python_stops_running") {
        return;
    }
    let python = block_python();
    let home = TempHome::new();
    let proj = home.0.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let venv = proj.join(".venv");
    let made = Command::new(&python).args(["-m", "venv", "--without-pip"]).arg(&venv).output().expect("run python -m venv");
    assert!(made.status.success(), "python -m venv: {}", String::from_utf8_lossy(&made.stderr));
    let bin = venv.join(if cfg!(windows) { "Scripts" } else { "bin" });
    let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap();
    let env = [("VIRTUAL_ENV", venv.as_os_str()), ("PATH", path.as_os_str())];
    std::fs::write(proj.join("m.v2"), "@import py.nosuch_r19_pkg as p\nprint(p.x)\n").unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &["proj/m.v2"], &env);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("hint: if 'nosuch_r19_pkg' isn't misspelled, install it for the Python V2 uses") && stderr.contains(&bin.display().to_string()), "{stderr}");
    assert!(!stderr.contains("which V2 isn't using"), "{stderr}");
    // Its python stops running: on Windows, pyvenv.cfg's home names a Python that is gone;
    // elsewhere, a program that fails as that launcher does (the links to the system's Python
    // are removed first, never written through).
    if cfg!(windows) {
        let cfg = venv.join("pyvenv.cfg");
        let text = std::fs::read_to_string(&cfg).unwrap();
        let gone: String = text.lines().map(|l| if l.trim_start().starts_with("home") { "home = C:\\nonexistent_r19\\Python311\n".to_string() } else { format!("{l}\n") }).collect();
        std::fs::write(&cfg, gone).unwrap();
    } else {
        for name in ["python3", "python"] {
            let p = bin.join(name);
            let _ = std::fs::remove_file(&p);
            std::fs::write(&p, "#!/bin/sh\necho \"No Python at '/nonexistent_r19/python3'\" >&2\nexit 103\n").unwrap();
            #[cfg(unix)]
            std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
    }
    // The launcher's own words follow: Python 3.11's "No Python at '…'", 3.13's "did not find
    // executable at '…'" (CI's).
    let problem = format!("the activated venv {} (VIRTUAL_ENV) has a python that doesn't run (", venv.display());
    let (code, _, stderr) = run_in_home(&home.0, &["proj/m.v2"], &env);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains(&format!("hint: {problem}")) && stderr.contains("nonexistent_r19") && stderr.contains("so V2 can't use it: make the venv again, "), "{stderr}");
    assert!(!stderr.contains("pip") && !stderr.contains("which V2 isn't using"), "{stderr}");
    let (code, stdout, stderr) = run_in_home(&home.0, &["doctor"], &env);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.contains(&format!("\n  py: {problem}")), "{stdout}");
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

// ---------------------------------------------------------------------------------------------
// The docs' examples (ROADMAP release gate)

/// The ```v2 examples of a Markdown file, each keyed by its last numbered heading and its place
/// under it (`3.6-1`), or `prefix-N` when a prefix is given.
fn doc_examples(text: &str, prefix: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut section = "0".to_string();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut fence: Option<Vec<&str>> = None;
    for line in text.lines() {
        if fence.is_some() {
            if line.starts_with("```") {
                let body = fence.take().unwrap_or_default();
                let n = counts.entry(section.clone()).or_insert(0);
                *n += 1;
                let key = if prefix.is_empty() { format!("{section}-{n}") } else { format!("{prefix}-{n}") };
                out.push((key, body.join("\n") + "\n"));
            } else if let Some(body) = fence.as_mut() {
                body.push(line);
            }
            continue;
        }
        if let Some(h) = line.strip_prefix("### ").or_else(|| line.strip_prefix("## ")) {
            let num: String = h.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            let num = num.trim_end_matches('.');
            if num.starts_with(|c: char| c.is_ascii_digit()) {
                section = num.to_string();
            }
        }
        if line.starts_with("```v2") {
            fence = Some(Vec::new());
        }
    }
    out
}

#[test]
fn docs_examples_run_as_documented() {
    // ROADMAP release gate: every runnable example of DOCS.md and README.md runs and prints what
    // tests/docs/<key>.out says, written from the docs; tests/docs/skip.txt says why the others
    // don't. An example in neither, or a file there naming no example, fails, so they can't drift.
    // Next to `<key>.out` (or `<key>.<os>.out`): `.args` (one per line), `.before` (V2 code put
    // before the example), `.uses` (`file key`: another example written as that file), and
    // fixtures/<key>/ (files copied next to it).
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = root.join("tests").join("docs");
    let mut examples = doc_examples(&std::fs::read_to_string(root.join("DOCS.md")).unwrap(), "");
    examples.extend(doc_examples(&std::fs::read_to_string(root.join("README.md")).unwrap(), "readme"));
    let code_of: HashMap<&str, &str> = examples.iter().map(|(k, c)| (k.as_str(), c.as_str())).collect();
    let skip_text = std::fs::read_to_string(dir.join("skip.txt")).unwrap();
    let skips: Vec<&str> = skip_text.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_whitespace().next()).collect();
    let os = if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let available = engine_availability(&["py".to_string(), "js".to_string(), "ts".to_string()]).unwrap_or_else(|e| panic!("{e}"));
    let required = std::env::var("V2_TEST_REQUIRE").unwrap_or_default();
    let read = |name: String| std::fs::read_to_string(dir.join(name)).ok();
    let mut failures = Vec::new();
    let mut ran = 0;
    for (key, code) in &examples {
        if skips.contains(&key.as_str()) {
            continue;
        }
        let Some(want) = read(format!("{key}.{os}.out")).or_else(|| read(format!("{key}.out"))) else {
            failures.push(format!("{key}: no tests/docs/{key}.out (write it from the docs) and not in skip.txt"));
            continue;
        };
        let needs: Vec<&str> = ["py", "js", "ts"].into_iter().filter(|t| code.contains(&format!("@{t}")) || code.contains(&format!(" {t}."))).collect();
        if let Some(t) = needs.iter().find(|t| !available[**t]) {
            if required.split(',').any(|r| r.trim() == *t) {
                failures.push(format!("{key}: engine {t} not available, and V2_TEST_REQUIRE requires it"));
            } else {
                let _ = writeln!(std::io::stderr(), "SKIP docs example {key} (engine {t} not available)");
            }
            continue;
        }
        let home = TempHome::new();
        std::fs::write(home.0.join("main.v2"), read(format!("{key}.before")).unwrap_or_default() + code).unwrap();
        for line in read(format!("{key}.uses")).unwrap_or_default().lines() {
            let (file, other) = line.split_once(' ').expect("`file key` lines");
            std::fs::write(home.0.join(file), code_of[other.trim()]).unwrap();
        }
        for entry in std::fs::read_dir(dir.join("fixtures").join(key)).into_iter().flatten().flatten() {
            std::fs::copy(entry.path(), home.0.join(entry.file_name())).unwrap();
        }
        let args: Vec<String> = read(format!("{key}.args")).unwrap_or_default().lines().map(str::to_string).collect();
        let mut cmd = v2_command(&home.0);
        cmd.arg("main.v2").args(&args).current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(120)).unwrap();
        let stdout = normalize(&String::from_utf8_lossy(&out.stdout));
        if out.status.code() != Some(0) || stdout != normalize(&want) {
            let stderr = String::from_utf8_lossy(&out.stderr);
            failures.push(format!("{key}: exit {:?}\n--- want\n{want}--- got\n{stdout}--- stderr\n{stderr}", out.status.code()));
        }
        ran += 1;
    }
    let names_an_example = |key: &str| code_of.contains_key(key);
    for key in &skips {
        if !names_an_example(key) {
            failures.push(format!("tests/docs/skip.txt names no example: {key}"));
        }
    }
    let suffixes = [".windows.out", ".linux.out", ".macos.out", ".out", ".args", ".before", ".uses"];
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "skip.txt" || name == "fixtures" {
            continue;
        }
        if !suffixes.iter().find_map(|s| name.strip_suffix(s)).is_some_and(names_an_example) {
            failures.push(format!("tests/docs/{name} names no example"));
        }
    }
    for entry in std::fs::read_dir(dir.join("fixtures")).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !names_an_example(&name) {
            failures.push(format!("tests/docs/fixtures/{name} names no example"));
        }
    }
    assert!(failures.is_empty(), "{} of {ran} docs examples:\n{}", failures.len(), failures.join("\n"));
}

// ---------------------------------------------------------------------------------------------
// Paths, files and processes (round 17)

#[cfg(unix)]
fn run_capped(home: &Path, args: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String, String) {
    use std::os::unix::process::CommandExt;
    let mut cmd = v2_command(home);
    cmd.args(args).current_dir(home).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.envs(env.iter().copied());
    // A regression would read /dev/zero or `yes` until the memory runs out: 2 GiB ends it sooner.
    // SAFETY: setrlimit is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            let limit = libc::rlimit { rlim_cur: 2 << 30, rlim_max: 2 << 30 };
            libc::setrlimit(libc::RLIMIT_AS, &limit);
            Ok(())
        });
    }
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(30)).unwrap();
    let text = |b: &[u8]| normalize(&String::from_utf8_lossy(b));
    (out.status.code(), text(&out.stdout), text(&out.stderr))
}

#[cfg(unix)]
#[test]
fn imports_of_pipes_devices_and_broken_links_are_import_errors() {
    // 6.4 step 1: an imported path that is a pipe or a device (a FIFO, a link to /dev/zero) is
    // refused as the entry FILE is (19.1), in a run and in `v2 check`, instead of a read that
    // never ends; a link that can't be followed says why; a name that isn't there is still
    // "cannot find".
    use std::os::unix::ffi::OsStringExt;
    let home = TempHome::new();
    let fifo = std::ffi::CString::new(home.0.join("fifo.v2").into_os_string().into_vec()).unwrap();
    // SAFETY: a NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0, "mkfifo");
    std::os::unix::fs::symlink("/dev/zero", home.0.join("zero.v2")).unwrap();
    std::os::unix::fs::symlink("loop.v2", home.0.join("loop.v2")).unwrap();
    std::os::unix::fs::symlink("gone.v2", home.0.join("dangling.v2")).unwrap();
    let names = ["fifo", "zero", "loop", "dangling", "nope"];
    let main: String = names.iter().map(|n| format!("try {{ import \"./{n}.v2\" as m }} catch (e) {{ print(e.type + \": \" + e.message + \" | \" + str(e.hint)) }}\n")).collect();
    std::fs::write(home.0.join("main.v2"), main).unwrap();
    std::fs::write(home.0.join("uncaught.v2"), "import \"./zero.v2\" as z\n").unwrap();
    // A link that leads nowhere or in a loop gets what fixes it, as one on the way does; where a
    // link to nothing points is absolute (the second review of round 19).
    let real = std::fs::canonicalize(&home.0).unwrap().to_string_lossy().into_owned();
    let gone = format!("{real}/gone.v2");
    let want = [
        ("ImportError: cannot import './fifo.v2': it is a pipe or a device, not a file".to_string(), None),
        ("ImportError: cannot import './zero.v2': it is a pipe or a device, not a file".to_string(), None),
        ("ImportError: cannot import './loop.v2': it is a link in a loop".to_string(), Some("remove the link (rm loop.v2), or point it somewhere else".to_string())),
        (format!("ImportError: cannot import './dangling.v2': it is a link to {gone}, which isn't there"), Some(format!("remove the link (rm dangling.v2), or make {gone}"))),
        ("ImportError: cannot find module './nope.v2' (looked for nope.v2)".to_string(), None),
    ];
    let (code, stdout, stderr) = run_capped(&home.0, &["main.v2"], &[]);
    let lines: Vec<String> = want.iter().map(|(m, h)| format!("{m} | {}", h.as_deref().unwrap_or("null"))).collect();
    assert_eq!((code, stdout), (Some(0), lines.join("\n") + "\n"), "{stderr}");
    let (code, stdout, stderr) = run_capped(&home.0, &["check", "main.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(1), "check main.v2: 1 file, 5 errors\n"), "{stderr}");
    for (i, (w, hint)) in want.iter().enumerate() {
        assert!(stderr.contains(&format!("main.v2:{}:7: {w}\n", i + 1)), "{w}: {stderr}");
        if let Some(hint) = hint {
            assert!(stderr.contains(&format!("hint: {hint}\n")), "{hint}: {stderr}");
        }
    }
    let (code, _, stderr) = run_capped(&home.0, &["uncaught.v2"], &[]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.starts_with("uncaught.v2:1:1: ImportError: cannot import './zero.v2': it is a pipe or a device, not a file\n"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn an_import_of_standard_input_from_a_pipe_is_a_pipe() {
    // 6.4 step 1 (round 17 review): /dev/stdin from a pipe is a link canonicalize can't follow
    // (Linux's /proc/self/fd/0 names "pipe:[N]"), yet the pipe is there: it is refused as a pipe,
    // not called a link to a file that isn't there.
    let home = TempHome::new();
    let mut names = vec!["/dev/stdin"];
    if cfg!(target_os = "linux") {
        names.push("/proc/self/fd/0");
    }
    let main: String = names.iter().map(|n| format!("try {{ import \"{n}\" as m }} catch (e) {{ print(e.type + \": \" + e.message) }}\n")).collect();
    std::fs::write(home.0.join("main.v2"), main).unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg("main.v2").current_dir(&home.0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), b"hi\n".to_vec(), Duration::from_secs(30)).unwrap();
    let want: String = names.iter().map(|n| format!("ImportError: cannot import '{n}': it is a pipe or a device, not a file\n")).collect();
    let stdout = normalize(&String::from_utf8_lossy(&out.stdout));
    assert_eq!((out.status.code(), stdout), (Some(0), want), "{}", String::from_utf8_lossy(&out.stderr));
}

#[cfg(unix)]
#[test]
fn a_v2_toml_path_with_control_characters_is_shown_escaped() {
    // 2.1 (round 17 review): the v2.toml path in a config error, in an engine's "(set by …)" and
    // on doctor's `config file:` line shows control characters escaped, as a script's name does.
    let home = TempHome::new();
    let dir = home.0.join("p\u{1b}[2Jq");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("v2.toml"), "[pythn]\n").unwrap();
    std::fs::write(dir.join("m.v2"), "print(1)\n").unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["p\u{1b}[2Jq/m.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(2), ""), "{stderr:?}");
    assert!(stderr.contains("p\\u{1b}[2Jq/v2.toml: config error: unknown section [pythn]"), "{stderr:?}");
    assert!(!stderr.contains('\u{1b}'), "{stderr:?}");
    std::fs::write(dir.join("v2.toml"), "[engines.py]\ncommand = [\"nosuch-v2-test\"]\n").unwrap();
    let (code, stdout, stderr) = run_in_home(&dir, &["doctor"], &[]);
    assert_eq!(code, Some(0), "{stderr:?}");
    let toml = std::fs::canonicalize(&dir).unwrap().join("v2.toml").display().to_string().replace('\u{1b}', "\\u{1b}");
    assert!(stdout.contains(&format!("py: engine 'py' command 'nosuch-v2-test' not found (set by {toml})")), "{stdout:?}");
    assert!(stdout.contains(&format!("\nconfig file: {toml}\n")), "{stdout:?}");
    assert!(!stdout.contains('\u{1b}'), "{stdout:?}");
    // The same folder in a program's or a venv's path: the message, doctor's COMMAND column and
    // its notes (round 17's second review).
    let real = std::fs::canonicalize(&dir).unwrap().display().to_string().replace('\u{1b}', "\\u{1b}");
    for toml_text in ["[engines.py]\ncommand = [\"./nosuch-python\"]\n", "[python]\nvenv = \".venv\"\n"] {
        std::fs::write(dir.join("v2.toml"), toml_text).unwrap();
        let (code, _, stderr) = run_in_home(&home.0, &["p\u{1b}[2Jq/m.v2"], &[]);
        assert!(!stderr.contains('\u{1b}'), "{toml_text}: {code:?} {stderr:?}");
        let (_, stdout, stderr) = run_in_home(&dir, &["doctor"], &[]);
        assert!(!stdout.contains('\u{1b}') && !stderr.contains('\u{1b}'), "{toml_text}: {stdout:?} {stderr:?}");
        assert!(stdout.contains(&real), "{toml_text}: {stdout:?}");
    }
}

#[test]
fn an_absolute_import_of_a_data_file_hints_read_file_with_that_path() {
    // 6.4 (round 17's second review): the read_file hint for an absolute path is that path, not
    // script_dir() glued before it.
    let home = TempHome::new();
    std::fs::write(home.0.join("data.csv"), "a,b\n").unwrap();
    let abs = home.0.join("data.csv").display().to_string().replace('\\', "/");
    std::fs::write(home.0.join("t.v2"), format!("try {{ import \"{abs}\" as m }} catch (e) {{ print(e.message) }}\n")).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["t.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.trim_end().ends_with(&format!("import loads .v2 files; read a data file with read_file(\"{abs}\")")), "{stdout}");
}

#[cfg(unix)]
#[test]
fn control_characters_in_file_names_are_shown_escaped() {
    // 2.1, 19.1: a file name holding an escape sequence or a line break shows them escaped in the
    // listing of a folder's .v2 files, a did-you-mean, a diagnostic's header, `at` frames and
    // check's summary, so an unpacked archive's names can't drive the terminal or split a line.
    let home = TempHome::new();
    std::fs::write(home.0.join("a\u{1b}[2Jb.v2"), "").unwrap();
    std::fs::write(home.0.join("new\nline.v2"), "").unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &["nosuch.v2"], &[]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("\nthe .v2 files there: a\\u{1b}[2Jb.v2, new\\nline.v2\n"), "{stderr:?}");
    let (_, _, stderr) = run_in_home(&home.0, &["a\u{1b}[2Jc.v2"], &[]);
    assert!(stderr.starts_with("v2: cannot find 'a\\u{1b}[2Jc.v2' in ") && stderr.contains("\ndid you mean 'a\\u{1b}[2Jb.v2'?\n"), "{stderr:?}");
    let name = "z\u{1b}[2Jz.v2";
    std::fs::write(home.0.join(name), "func f() { throw \"boom\" }\nf()\n").unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &[name], &[]);
    assert_eq!(code, Some(1), "{stderr:?}");
    assert!(stderr.starts_with("z\\u{1b}[2Jz.v2:1:12: Error: boom\n"), "{stderr:?}");
    assert!(stderr.contains("\n  at f (z\\u{1b}[2Jz.v2:1)\n  at z\\u{1b}[2Jz.v2:2\n"), "{stderr:?}");
    let bad = "y\u{1b}[2Jy.v2";
    std::fs::write(home.0.join(bad), "let = 1\n").unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["check", bad], &[]);
    assert_eq!(code, Some(1), "{stderr:?}");
    assert!(stderr.starts_with("y\\u{1b}[2Jy.v2:1:5: SyntaxError: "), "{stderr:?}");
    assert!(stdout.starts_with("check y\\u{1b}[2Jy.v2: 1 file, 1 error"), "{stdout:?}");
    for text in [&stdout, &stderr] {
        assert!(!text.contains('\u{1b}'), "{text:?}");
    }
}

#[test]
#[cfg_attr(not(debug_assertions), ignore = "the V2_TEST_MAX_READ cap exists in debug builds only")]
fn read_file_and_exec_stop_past_what_a_str_holds() {
    // 5.10: past 1 GiB, the most text `*` makes (here the debug builds' V2_TEST_MAX_READ,
    // 1000 bytes), read_file and exec raise IOError instead of using up the memory, and exec
    // stops the program there; what fits is read whole.
    let main = "let v2 = args()[0]\n\
                for f in [\"fits.txt\", \"big.txt\"] {\n\
                \x20   try { print(len(read_file(f)), len(read_file(f, binary: true))) } catch (e) { print(e.type + \": \" + e.message) }\n\
                }\n\
                print(len(exec([v2, \"fits.v2\"]).stdout))\n\
                for f in [\"more.v2\", \"more_err.v2\"] {\n\
                \x20   try { exec([v2, f]) } catch (e) { print(e.type + \": \" + e.message.replace(v2, \"v2\")) }\n\
                }\n";
    let fits = "a".repeat(1000);
    let big = "a".repeat(1001);
    let files = [
        ("main.v2", main),
        ("fits.txt", fits.as_str()),
        ("big.txt", big.as_str()),
        ("fits.v2", "print(\"x\" * 999)\n"),
        ("more.v2", "for i in 0..100 { print(\"x\" * 99) }\n"),
        ("more_err.v2", "for i in 0..100 { print(\"x\" * 99, stderr: true) }\n"),
    ];
    let (code, stdout, stderr, _home) = run_with_files_env(&files, &["main.v2", BIN], &[("V2_TEST_MAX_READ", "1000")]);
    assert_eq!(code, Some(0), "{stderr}");
    let exec = "IOError: exec: 'v2' wrote more than 1 GiB of output: send it to a file in the command, or read it in pieces in a block";
    let want = format!("1000 1000\nIOError: cannot read 'big.txt': it holds more than 1 GiB, the most V2 reads at once: read it in pieces in a block\n1000\n{exec}\n{exec}\n");
    assert_eq!(stdout, want);
    // Sources that never end: a device, and a program that writes until it is stopped.
    #[cfg(unix)]
    {
        let home = TempHome::new();
        let src = "for f in [func() => read_file(\"/dev/zero\"), func() => read_file(\"/dev/zero\", binary: true), func() => exec([\"yes\"])] {\n    try { f() } catch (e) { print(e.message) }\n}\n";
        std::fs::write(home.0.join("endless.v2"), src).unwrap();
        let (code, stdout, stderr) = run_capped(&home.0, &["endless.v2"], &[("V2_TEST_MAX_READ", "100000")]);
        assert_eq!(code, Some(0), "{stderr}");
        let device = "cannot read '/dev/zero': it holds more than 1 GiB, the most V2 reads at once: read it in pieces in a block";
        assert_eq!(stdout, format!("{device}\n{device}\n{}\n", exec.trim_start_matches("IOError: ").replace("'v2'", "'yes'")));
    }
}

#[cfg(windows)]
#[test]
fn a_file_another_program_holds_open_says_to_close_it_there() {
    // 5.10: a file another program opened without sharing, as Excel does, is refused by Windows
    // with ERROR_SHARING_VIOLATION; each file builtin's IOError says what to do.
    use std::os::windows::fs::OpenOptionsExt;
    let home = TempHome::new();
    let csv = home.0.join("results.csv");
    std::fs::write(&csv, "a,b\n").unwrap();
    let held = std::fs::OpenOptions::new().read(true).write(true).share_mode(0).open(&csv).unwrap();
    let src = "let f = \"results.csv\"\n\
               for op in [func() => read_file(f), func() => write_file(f, \"x\"), func() => write_file(f, \"x\", append: true), func() => remove_file(f)] {\n\
               \x20   try { op() } catch (e) { print(e.message.split(\": \")[0] + \" | \" + e.hint) }\n\
               }\n";
    std::fs::write(home.0.join("locked.v2"), src).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["locked.v2"], &[]);
    drop(held);
    assert_eq!(code, Some(0), "{stderr}");
    let hint = "another program has it open (Excel keeps an open file locked): close it there, or use another file name";
    let want: String = ["read", "write", "write", "remove"].iter().map(|v| format!("cannot {v} 'results.csv' | {hint}\n")).collect();
    assert_eq!(stdout, want);
}

#[test]
fn an_uncaught_error_starts_on_a_line_of_its_own() {
    // 2.4: with stdout and stderr one pipe or file (`2>&1`), the report of an error after
    // print("x", end: "") starts on the next line; so it does after such a write to stderr. A
    // finished line gets no blank line, and with stdout apart nothing is added.
    let home = TempHome::new();
    std::fs::write(home.0.join("open.v2"), "print(\"9 stdout\", end: \"\")\nthrow \"10 uncaught\"\n").unwrap();
    std::fs::write(home.0.join("err.v2"), "print(\"w\", stderr: true, end: \"\")\nthrow \"e\"\n").unwrap();
    std::fs::write(home.0.join("ended.v2"), "print(\"line\")\nthrow \"e\"\n").unwrap();
    let one_pipe = |name: &str| {
        let (mut reader, writer) = std::io::pipe().expect("a pipe");
        let mut cmd = v2_command(&home.0);
        cmd.arg(name).current_dir(&home.0).stdin(Stdio::null()).stdout(writer.try_clone().unwrap()).stderr(writer);
        let child = cmd.spawn().expect("start v2");
        drop(cmd);
        let mut text = String::new();
        reader.read_to_string(&mut text).unwrap();
        let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
        assert_eq!(out.status.code(), Some(1), "{text}");
        normalize(&text)
    };
    let one_file = |name: &str| {
        let path = home.0.join("both.txt");
        let file = std::fs::File::create(&path).unwrap();
        let mut cmd = v2_command(&home.0);
        cmd.arg(name).current_dir(&home.0).stdin(Stdio::null()).stdout(file.try_clone().unwrap()).stderr(file);
        let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
        assert_eq!(out.status.code(), Some(1));
        normalize(&std::fs::read_to_string(&path).unwrap())
    };
    let joiners: [&dyn Fn(&str) -> String; 2] = [&one_pipe, &one_file];
    for joined in joiners {
        let text = joined("open.v2");
        assert!(text.starts_with("9 stdout\nopen.v2:2:1: Error: 10 uncaught\n"), "{text:?}");
        let text = joined("err.v2");
        assert!(text.starts_with("w\nerr.v2:2:1: Error: e\n"), "{text:?}");
        let text = joined("ended.v2");
        assert!(text.starts_with("line\nended.v2:2:1: Error: e\n"), "{text:?}");
    }
    let (code, stdout, stderr) = run_in_home(&home.0, &["open.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(1), "9 stdout"));
    assert!(stderr.starts_with("open.v2:2:1: Error: 10 uncaught\n"), "{stderr:?}");
    let (_, _, stderr) = run_in_home(&home.0, &["err.v2"], &[]);
    assert!(stderr.starts_with("w\nerr.v2:2:1: Error: e\n"), "{stderr:?}");
}

/// A link to a folder: a junction on Windows (which needs no administrator), else a symlink.
fn dir_link(link: &Path, target: &Path) {
    #[cfg(windows)]
    {
        let ok = Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(link).arg(target).stdout(Stdio::null()).status().unwrap().success();
        assert!(ok, "mklink /J {}", link.display());
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
}

/// A path as V2 shows a real one: canonical, without Windows' `\\?\`.
fn real_path(p: &Path) -> String {
    let real = std::fs::canonicalize(p).unwrap().to_string_lossy().into_owned();
    match real.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => real.strip_prefix(r"\\?\").unwrap_or(&real).to_string(),
    }
}

#[test]
fn links_on_the_way_that_lead_nowhere_or_in_a_loop_say_so() {
    // DOCS 3.10, 6.4, 19.1, 5.10 (round 19): a folder on a path that is a link (a junction on
    // Windows, the one link a non-administrator can make) to nothing, or in a loop, is named in a
    // run, in v2 check, for FILE and in the file builtins, with what fixes it (an import's hint
    // too): not "cannot find module", "there is no folder", Windows' words, or a make_dir hint
    // that would fail. The link is named as written, in the command the shell needs: on Windows
    // rmdir with \ (cmd takes sub/broken for a switch), in Git Bash (MSYSTEM is set) rm with /.
    let home = TempHome::new();
    let d = &home.0;
    std::fs::create_dir_all(d.join("sub").join("gone")).unwrap();
    dir_link(&d.join("sub").join("broken"), &d.join("sub").join("gone"));
    std::fs::remove_dir(d.join("sub").join("gone")).unwrap();
    dir_link(&d.join("loop1"), &d.join("loop2"));
    dir_link(&d.join("loop2"), &d.join("loop1"));
    let main = "try { import \"./sub/broken/lib.v2\" as a } catch (e) { print(e.message + \" | \" + e.hint) }\n\
                try { import \"./loop1/lib.v2\" as b } catch (e) { print(e.message + \" | \" + e.hint) }\n\
                for p in [\"sub/broken/new.txt\", \"loop1/x.txt\"] {\n    try { write_file(p, \"x\") } catch (e) { print(e.message + \" | \" + e.hint) }\n}\n\
                try { make_dir(\"sub/broken\") } catch (e) { print(e.message + \" | \" + e.hint) }\n";
    std::fs::write(d.join("main.v2"), main).unwrap();
    // With MSYSTEM set or not, whatever shell runs the tests: (code, stdout, stderr).
    let run = |args: &[&str], msystem: Option<&str>| {
        let mut cmd = v2_command(d);
        cmd.args(args).current_dir(d).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        match msystem {
            Some(v) => cmd.env("MSYSTEM", v),
            None => cmd.env_remove("MSYSTEM"),
        };
        let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
        let text = |b: &[u8]| normalize(&String::from_utf8_lossy(b));
        (out.status.code(), text(&out.stdout), text(&out.stderr))
    };
    let gone = d.join("sub").join("gone").display().to_string();
    let to_gone = format!("is a link to {gone}, which isn't there");
    for msystem in [None, Some("MINGW64")] {
        let (broken, loop1) = match (cfg!(windows), msystem) {
            (true, None) => (r"rmdir sub\broken", "rmdir loop1"),
            _ => ("rm sub/broken", "rm loop1"),
        };
        let fix = format!("remove the link ({broken}), or make {gone}");
        let in_loop = format!("'loop1' is a link in a loop | remove the link ({loop1}), or point it somewhere else");
        let want = [
            format!("cannot import './sub/broken/lib.v2': 'sub/broken' {to_gone} | {fix}"),
            format!("cannot import './loop1/lib.v2': {in_loop}"),
            format!("cannot write 'sub/broken/new.txt': 'sub/broken' {to_gone} | {fix}"),
            format!("cannot write 'loop1/x.txt': {in_loop}"),
            format!("cannot create 'sub/broken': it {to_gone} | {fix}"),
        ];
        let (code, stdout, stderr) = run(&["main.v2"], msystem);
        assert_eq!((code, stdout), (Some(0), want.join("\n") + "\n"), "{stderr}");
        let (code, stdout, stderr) = run(&["check", "main.v2"], msystem);
        assert_eq!((code, stdout.as_str()), (Some(1), "check main.v2: 1 file, 2 errors\n"), "{stderr}");
        for (line, w) in want[..2].iter().enumerate() {
            let (message, hint) = w.split_once(" | ").unwrap();
            assert!(stderr.contains(&format!("main.v2:{}:7: ImportError: {message}\n", line + 1)), "{stderr}");
            assert!(stderr.contains(&format!("\n  hint: {hint}\n")), "{stderr}");
        }
        let lines = [("sub/broken/main.v2", format!("'sub/broken' {to_gone}: {fix}")), ("loop1/main.v2", in_loop.replace(" |", ":"))];
        for (file, line) in lines {
            let (code, _, stderr) = run(&[file], msystem);
            assert_eq!((code, stderr), (Some(2), format!("v2: cannot find '{file}'\n{line}\n")));
        }
    }
    // A relative symbolic link's target is from the link's folder, not the current one.
    #[cfg(unix)]
    {
        std::fs::create_dir_all(d.join("rel")).unwrap();
        std::os::unix::fs::symlink("gone", d.join("rel").join("broken")).unwrap();
        let rel = "try { import \"./rel/broken/lib.v2\" as a } catch (e) { print(e.message + \" | \" + e.hint) }\n\
                   try { write_file(\"rel/broken/x.txt\", \"x\") } catch (e) { print(e.message + \" | \" + e.hint) }\n";
        std::fs::write(d.join("rel.v2"), rel).unwrap();
        let gone = format!("{}/gone", real_path(&d.join("rel")));
        let problem = format!("'rel/broken' is a link to {gone}, which isn't there");
        let fix = format!("remove the link (rm rel/broken), or make {gone}");
        let want = format!("cannot import './rel/broken/lib.v2': {problem} | {fix}\ncannot write 'rel/broken/x.txt': {problem} | {fix}\n");
        let (code, stdout, stderr) = run(&["rel.v2"], None);
        assert_eq!((code, stdout), (Some(0), want), "{stderr}");
        let (code, _, stderr) = run(&["rel/broken/main.v2"], None);
        assert_eq!((code, stderr), (Some(2), format!("v2: cannot find 'rel/broken/main.v2'\n{problem}: {fix}\n")));
    }
}

/// Takes away reading `dir` (Windows: a deny for this user that its files inherit; elsewhere its
/// mode), until the guard is dropped.
struct Unreadable(PathBuf);

impl Unreadable {
    fn new(dir: &Path, _mode: u32) -> Unreadable {
        #[cfg(windows)]
        {
            let user = std::env::var("USERNAME").unwrap();
            let ok = Command::new("icacls").arg(dir).arg("/deny").arg(format!("{user}:(OI)(CI)(R)")).stdout(Stdio::null()).status().unwrap().success();
            assert!(ok, "icacls /deny");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(_mode)).unwrap();
        }
        Unreadable(dir.to_path_buf())
    }
}

impl Drop for Unreadable {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            let user = std::env::var("USERNAME").unwrap_or_default();
            let _ = Command::new("icacls").arg(&self.0).arg("/remove:d").arg(user).stdout(Stdio::null()).status();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }
}

#[test]
fn a_folder_v2_may_not_read_says_so() {
    // 6.4, 19.1 (round 19): a .v2 file in a folder whose permissions deny reading (as a deny
    // inherited from the folder does) is "cannot import …: Access is denied", the OS's reason,
    // not "cannot find module", in a run and in v2 check; and a folder V2 can't list is not one
    // without .v2 files. Root reads every folder, so the test doesn't run as root.
    #[cfg(unix)]
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        let _ = writeln!(std::io::stderr(), "SKIP a_folder_v2_may_not_read_says_so (root reads every folder)");
        return;
    }
    let home = TempHome::new();
    let d = &home.0;
    for f in ["noread/lib.v2", "nolist/main.v2"] {
        std::fs::create_dir_all(d.join(f).parent().unwrap()).unwrap();
        std::fs::write(d.join(f), "let x = 1\n").unwrap();
    }
    std::fs::write(d.join("t.v2"), "import \"./noread/lib.v2\" as l\n").unwrap();
    // Off Windows a folder that can be looked into but not listed (--x) is the listing's case.
    let _a = Unreadable::new(&d.join("noread"), 0o000);
    let _b = Unreadable::new(&d.join("nolist"), 0o111);
    let why = if cfg!(windows) { "Access is denied" } else { "Permission denied" };
    let error = format!("t.v2:1:1: ImportError: cannot import './noread/lib.v2': {why}\n");
    let (code, _, stderr) = run_in_home(d, &["t.v2"], &[]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.starts_with(&error), "{stderr}");
    let (code, stdout, stderr) = run_in_home(d, &["check", "t.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(1), "check t.v2: 1 file, 1 error\n"), "{stderr}");
    assert!(stderr.starts_with(&error), "{stderr}");
    let (code, _, stderr) = run_in_home(d, &["nolist/nothere.v2"], &[]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.ends_with(&format!("\nV2 can't list that folder: {why} (check the name, or the folder's permissions)\n")), "{stderr}");
    let sep = std::path::MAIN_SEPARATOR;
    for (command, what) in [(&["nolist"][..], "run"), (&["check", "nolist"][..], "check")] {
        let (code, _, stderr) = run_in_home(d, command, &[]);
        let typed = command[..command.len() - 1].iter().map(|w| format!(" {w}")).collect::<String>();
        let want = format!("v2: 'nolist' is a folder, not a script, and V2 can't list it: {why}\n{what} a .v2 file in it by its name: v2{typed} nolist{sep}FILE.v2\n");
        assert_eq!((code, stderr), (Some(2), want));
    }
}

#[cfg(windows)]
#[test]
fn a_script_or_module_another_program_holds_open_says_to_close_it() {
    // 19.1, 6.4 (round 19): a file another program opened without sharing (as Excel keeps one)
    // gets the file builtins' hint (5.10) as the script, in v2 check and as an import.
    use std::os::windows::fs::OpenOptionsExt;
    let home = TempHome::new();
    let d = &home.0;
    std::fs::write(d.join("locked.v2"), "print(1)\n").unwrap();
    std::fs::write(d.join("lockedmod.v2"), "let x = 1\n").unwrap();
    std::fs::write(d.join("usemod.v2"), "import \"./lockedmod.v2\" as m\nprint(m.x)\n").unwrap();
    let hold = |p: &Path| std::fs::OpenOptions::new().read(true).share_mode(0).open(p).unwrap();
    let (_a, _b) = (hold(&d.join("locked.v2")), hold(&d.join("lockedmod.v2")));
    let busy = "The process cannot access the file because it is being used by another process";
    let hint = "another program has it open (Excel keeps an open file locked): close it there";
    for args in [&["locked.v2"][..], &["check", "locked.v2"]] {
        let (code, _, stderr) = run_in_home(d, args, &[]);
        assert_eq!((code, stderr), (Some(2), format!("v2: cannot read 'locked.v2': {busy}\n{hint}\n")));
    }
    for args in [&["usemod.v2"][..], &["check", "usemod.v2"]] {
        let (code, _, stderr) = run_in_home(d, args, &[]);
        assert_eq!(code, Some(1), "{stderr}");
        assert!(stderr.starts_with(&format!("usemod.v2:1:1: ImportError: cannot import './lockedmod.v2': cannot read 'lockedmod.v2': {busy}\n")), "{stderr}");
        assert!(stderr.contains(&format!("\n  hint: {hint}\n")), "{stderr}");
    }
}

/// `v2 ARGS` in `home` with `MSYSTEM` (Git Bash's) set to `msystem`, or unset: (code, stderr).
fn run_msystem(home: &Path, args: &[&str], msystem: Option<&str>) -> (Option<i32>, String) {
    let mut cmd = v2_command(home);
    cmd.args(args).current_dir(home).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    match msystem {
        Some(v) => cmd.env("MSYSTEM", v),
        None => cmd.env_remove("MSYSTEM"),
    };
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
    (out.status.code(), normalize(&String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn several_scripts_for_v2_check_get_the_readmes_loop() {
    // 19.1 (round 19): `v2 check *.v2` that a shell expanded (Git Bash, sh) gives several files:
    // v2 check takes one, and the README's loop checks them all, the shell's, or on Windows
    // outside Git Bash PowerShell's.
    let home = TempHome::new();
    let shell = "loop in the shell: ok=0; for f in *.v2; do v2 check \"$f\" || ok=1; done";
    let powershell = "loop in PowerShell: $ok = 0; Get-ChildItem *.v2 | ForEach-Object { v2 check $_.Name; if ($LASTEXITCODE -ne 0) { $ok = 1 } }";
    let want = |how: &str| (Some(2), format!("v2: unexpected argument `b.v2` after `v2 check FILE`: it checks one file; to check several, {how}\nrun `v2 --help` for usage\n"));
    assert_eq!(run_msystem(&home.0, &["check", "a.v2", "b.v2"], None), want(if cfg!(windows) { powershell } else { shell }));
    assert_eq!(run_msystem(&home.0, &["check", "a.v2", "b.v2"], Some("MINGW64")), want(shell));
    // Anything else after FILE is the plain usage error.
    let plain = (Some(2), "v2: unexpected argument `--all` after `v2 check FILE`\nrun `v2 --help` for usage\n".to_string());
    assert_eq!(run_msystem(&home.0, &["check", "a.v2", "--all"], None), plain);
}

#[cfg(windows)]
#[test]
fn a_git_bash_path_or_a_wildcard_for_file_says_what_to_type() {
    // 19.1 (round 19): Git Bash's /c/… pasted into cmd or PowerShell, which read it as \c\… on
    // this drive, gets the file builtins' hint (5.10); a * or ?, which they don't expand, says V2
    // takes one script (v2 check: the README's PowerShell loop). In Git Bash a * that reaches V2
    // matched nothing, so it is a file that isn't there.
    let home = TempHome::new();
    for f in ["hello.v2", "h2.v2"] {
        std::fs::write(home.0.join(f), "print(1)\n").unwrap();
    }
    let real = real_path(&home.0);
    let (drive, rest) = real.split_at(2);
    let letter = drive[..1].to_ascii_lowercase();
    if drive.ends_with(':') && !Path::new(&format!("{drive}\\{letter}")).exists() {
        let msys = format!("/{letter}{}/hello.v2", rest.replace('\\', "/"));
        let fixed = format!("{}:{}/hello.v2", letter.to_ascii_uppercase(), rest.replace('\\', "/"));
        let typed = if fixed.chars().all(|c| c.is_alphanumeric() || "._-+~\\/:".contains(c)) { fixed.clone() } else { format!("\"{fixed}\"") };
        for (args, command) in [(vec![msys.as_str()], "v2"), (vec!["check", msys.as_str()], "v2 check")] {
            let want = format!("v2: cannot find '{msys}'\non Windows, /{letter}/… is Git Bash's spelling of {}:/…: run {command} {typed}\n", letter.to_ascii_uppercase());
            assert_eq!(run_msystem(&home.0, &args, None), (Some(2), want));
        }
    }
    let loop_line = "to check several, loop in PowerShell: $ok = 0; Get-ChildItem *.v2 | ForEach-Object { v2 check $_.Name; if ($LASTEXITCODE -ne 0) { $ok = 1 } }";
    let want = format!("v2: cannot find '*.v2': cmd and PowerShell don't expand * and ?, and v2 check takes one file\n{loop_line}\n");
    assert_eq!(run_msystem(&home.0, &["check", "*.v2"], None), (Some(2), want));
    let want = "v2: cannot find 'h?.v2': cmd and PowerShell don't expand * and ?, and v2 runs one script\nthe .v2 files there: h2.v2, hello.v2\n";
    assert_eq!(run_msystem(&home.0, &["h?.v2"], None), (Some(2), want.to_string()));
    let (code, stderr) = run_msystem(&home.0, &["check", "*.v2"], Some("MINGW64"));
    assert_eq!(code, Some(2));
    assert!(stderr.starts_with("v2: cannot find '*.v2' in ") && stderr.ends_with("\nthe .v2 files there: h2.v2, hello.v2\n"), "{stderr}");
}

#[cfg(windows)]
#[test]
fn a_module_reached_through_a_share_and_its_drive_path_is_one_module() {
    // 6.4, 13.5 (round 19): a local folder shared as \\localhost\C$\… and its C:\… path
    // canonicalize apart, yet name one file: a V2 module runs once, and Python's module check
    // doesn't take one file for two. Skipped where the share isn't reachable.
    let home = TempHome::new();
    let real = real_path(&home.0);
    let unc = format!(r"\\localhost\{}$\{}", &real[..1], &real[3..]);
    if real.as_bytes().get(1) != Some(&b':') || !Path::new(&unc).is_dir() {
        let _ = writeln!(std::io::stderr(), "SKIP a_module_reached_through_a_share_and_its_drive_path_is_one_module (no \\\\localhost share)");
        return;
    }
    let c = real.replace('\\', "/");
    let files = [
        ("counter.v2", "print(\"counter.v2 loading\")\nlet n = 0\nfunc bump() {\n    n += 1\n    return n\n}\n".to_string()),
        ("main.v2", format!("import \"./counter.v2\" as a\nimport \"{c}/counter.v2\" as b\nprint(a.bump(), b.bump())\n")),
        ("helpers2.py", "def hi():\n    return 'hi'\n".to_string()),
        ("usehelp.v2", "@import { hi } from py.helpers2\nfunc call() { return hi() }\n".to_string()),
        ("usehelp2.v2", "@import { hi } from py.helpers2\nfunc call() { return hi() }\n".to_string()),
        ("pymix.v2", format!("import \"./usehelp2.v2\" as u2\nimport \"{c}/usehelp.v2\" as u1\nprint(u2.call(), u1.call())\n")),
    ];
    for (name, text) in &files {
        std::fs::write(home.0.join(name), text).unwrap();
    }
    let (code, stdout, stderr) = run_in_home(&home.0, &[&format!(r"{unc}\main.v2")], &[]);
    assert_eq!((code, stdout.as_str()), (Some(0), "counter.v2 loading\n1 2\n"), "{stderr}");
    let (code, stdout, stderr) = run_in_home(&home.0, &["check", &format!(r"{unc}\main.v2")], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.ends_with("main.v2: 2 files, 0 errors\n"), "{stdout}");
    if needs("py", "a_module_reached_through_a_share_and_its_drive_path_is_one_module") {
        let (code, stdout, stderr) = run_in_home(&home.0, &[&format!(r"{unc}\pymix.v2")], &[]);
        assert_eq!((code, stdout.as_str()), (Some(0), "hi hi\n"), "{stderr}");
    }
}

#[test]
fn a_new_file_that_got_a_deleted_modules_id_is_another_module() {
    // 6.4 (round 19): a module is looked up by its file's ID only while the loaded path still has
    // that ID. ext4 gives a deleted file's inode to the next new file, so two.v2, written after
    // one.v2 was imported and deleted, got one.v2's ID and its import returned one.v2's module.
    let home = TempHome::new();
    let main = "let d = script_dir() + \"/gen\"\nmake_dir(d)\n\
                write_file(d + \"/one.v2\", \"print(\\\"one.v2 loading\\\")\\nlet x = 1\\n\")\n\
                import \"./gen/one.v2\" as a\nprint(\"a.x =\", a.x)\nremove_file(d + \"/one.v2\")\n\
                write_file(d + \"/two.v2\", \"print(\\\"two.v2 loading\\\")\\nlet x = 2\\n\")\n\
                import \"./gen/two.v2\" as b\nprint(\"b.x =\", b.x)\n";
    std::fs::write(home.0.join("main.v2"), main).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["main.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(0), "one.v2 loading\na.x = 1\ntwo.v2 loading\nb.x = 2\n"), "{stderr}");
}

#[test]
fn an_import_spelled_in_another_case_gets_a_note_where_case_doesnt_matter() {
    // 6.4 (round 19): on a file system that ignores case (Windows', macOS's) an import whose
    // spelling differs from the file's name only in case works, and a note, once per import
    // site, says a case-sensitive one (Linux's) won't find it; v2 check says it too. Skipped on
    // a case-sensitive file system, where such an import is "cannot find module".
    let home = TempHome::new();
    std::fs::create_dir_all(home.0.join("real")).unwrap();
    std::fs::write(home.0.join("real/counter.v2"), "print(\"loading\")\nfunc f() { return 1 }\n").unwrap();
    if !home.0.join("REAL/Counter.V2").exists() {
        let _ = writeln!(std::io::stderr(), "SKIP an_import_spelled_in_another_case_gets_a_note_where_case_doesnt_matter (a case-sensitive file system)");
        return;
    }
    let main = "import \"./real/counter.v2\" as a\nimport \"./REAL/Counter.V2\" as c\nfunc g() {\n    import \"./REAL/Counter.V2\" as d\n    return d.f()\n}\nprint(g(), g(), c.f())\n";
    std::fs::write(home.0.join("spell.v2"), main).unwrap();
    let note = "note: './REAL/Counter.V2' is spelled './real/counter.v2' on disk: a case-sensitive file system, as Linux's, won't find it\n";
    let (code, stdout, stderr) = run_in_home(&home.0, &["spell.v2"], &[]);
    assert_eq!((code, stdout.as_str(), stderr), (Some(0), "loading\n1 1 1\n", note.repeat(2)));
    let (code, stdout, stderr) = run_in_home(&home.0, &["check", "spell.v2"], &[]);
    assert_eq!((code, stdout.as_str(), stderr), (Some(0), "check spell.v2: 2 files, 0 errors\n", note.repeat(2)));
}

#[test]
fn doctor_in_a_linked_folder_shows_the_v2_toml_a_run_there_uses() {
    // 18.1, DOCS 14 (round 19): doctor starts from the current folder with its links followed (a
    // junction on Windows, whose current folder keeps the link's path), as a run of a script there
    // starts from the script's real folder, so both name one v2.toml.
    let home = TempHome::new();
    let d = &home.0;
    std::fs::create_dir_all(d.join("A")).unwrap();
    std::fs::create_dir_all(d.join("B").join("sub")).unwrap();
    std::fs::write(d.join("A").join("v2.toml"), "[engines.js]\ncall_timeout = 1\n").unwrap();
    std::fs::write(d.join("B").join("v2.toml"), "[engines.js]\ncall_timeout = 2\n").unwrap();
    dir_link(&d.join("A").join("link"), &d.join("B").join("sub"));
    let mut cmd = v2_command(d);
    cmd.arg("doctor").current_dir(d.join("A").join("link")).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(120)).unwrap();
    let stdout = normalize(&String::from_utf8_lossy(&out.stdout));
    assert!(stdout.contains(&format!("\nconfig file: {}\n", real_path(&d.join("B").join("v2.toml")))), "{stdout}");
}
