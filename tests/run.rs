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
fn a_closed_stderr_stops_the_run_quietly() {
    // DOCS 19.1, INTERNALS 6.5 (round 22, L8): stderr's reader going away (`v2 x.v2 2>&1 | head`)
    // stops the run as stdout's does, with 141 and nothing more printed: `print(stderr: true)`,
    // and a block's stderr text that V2 relays.
    let mut programs = vec![("errloop.v2", "for i in 0..100000000 {\n    print(i, stderr: true)\n}\n")];
    if needs("py", "a_closed_stderr_stops_the_run_quietly") {
        programs.push(("pyerrloop.v2", "@py {\n    import sys\n    i = 0\n    while True:\n        print(i, file=sys.stderr)\n        i += 1\n}\n"));
    }
    for (name, src) in programs {
        let home = TempHome::new();
        std::fs::write(home.0.join(name), src).unwrap();
        let mut child = v2_command(&home.0).arg(name).current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let mut first = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(child.stderr.take().unwrap()), &mut first).unwrap();
        assert_eq!(first, "0\n", "{name}");
        // The reader is dropped with the BufReader: the pipe is closed.
        let start = Instant::now();
        let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(20)).unwrap();
        assert_eq!(out.status.code(), Some(141), "{name}: {}", String::from_utf8_lossy(&out.stdout));
        assert!(out.stdout.is_empty(), "{name}: {}", String::from_utf8_lossy(&out.stdout));
        assert!(start.elapsed() < Duration::from_secs(10), "{name} stopped only after {:?}", start.elapsed());
    }
}

#[test]
fn colorama_passes_a_blocks_escape_codes_through_on_windows() {
    // INTERNALS 9.9 (round 22, the second review, issue 1): on Windows V2 has VT processing on for
    // its consoles, and colorama (whose init tqdm, click and others call) would turn a block's escape
    // codes into console calls on the worker's own handle, out of order with the text: there its
    // ansitowin32.winapi_test answers False once imported. A colorama next to the script stands for
    // the real one; on Linux and macOS it is left as it is.
    if !needs("py", "colorama_passes_a_blocks_escape_codes_through_on_windows") {
        return;
    }
    let home = TempHome::new();
    std::fs::create_dir_all(home.0.join("colorama")).unwrap();
    std::fs::write(home.0.join("colorama").join("__init__.py"), "").unwrap();
    std::fs::write(home.0.join("colorama").join("ansitowin32.py"), "def winapi_test():\n    return True\n").unwrap();
    std::fs::write(home.0.join("main.v2"), "@py {\n    from colorama import ansitowin32\n    print(ansitowin32.winapi_test())\n}\n").unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg("main.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
    let want = if cfg!(windows) { "False\n" } else { "True\n" };
    assert_eq!((out.status.code(), normalize(&String::from_utf8_lossy(&out.stdout)).as_str()), (Some(0), want), "{}", String::from_utf8_lossy(&out.stderr));
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
            .env("LC_ALL", "C.UTF-8")
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

#[cfg(unix)]
#[test]
fn iconvs_line_takes_its_code_page_from_the_locale() {
    // INTERNALS 3.1 (round 20): from cp1252, iconv garbled a Turkish file without an error. The
    // line V2 gives, followed, makes a file that prints the text as it was written.
    let home = TempHome::new();
    // print("Iğdır şehri") in cp1254. LC_ALL empty counts as unset: LC_CTYPE comes next.
    std::fs::write(home.0.join("old.v2"), b"print(\"I\xf0d\xfdr \xfeehri\")\n").unwrap();
    let out = v2_command(&home.0).env("LC_ALL", "").env("LC_CTYPE", "tr_TR.UTF-8").env("LANG", "C").arg("old.v2").current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "old.v2:1:9: SyntaxError: file is not valid UTF-8: it may be in an older encoding, such as Windows-1254: save it as UTF-8, as in iconv -f cp1254 -t utf-8 old.v2 > new.v2, then check the text\n"
    );
    let iconv = Command::new("iconv").args(["-f", "cp1254", "-t", "utf-8", "old.v2"]).current_dir(&home.0).output();
    let Some(iconv) = iconv.ok().filter(|o| o.status.success()) else {
        return; // no iconv with cp1254 here
    };
    std::fs::write(home.0.join("new.v2"), iconv.stdout).unwrap();
    let out = v2_command(&home.0).arg("new.v2").current_dir(&home.0).stdin(Stdio::null()).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Iğdır şehri\n", "{}", String::from_utf8_lossy(&out.stderr));
}

#[cfg(unix)]
#[test]
fn a_v2_tomls_iconv_line_names_the_file_it_is_about() {
    // INTERNALS 18.5 (round 20): v2.toml is found from the script's folder up (18.1), so from
    // another folder iconv's line names it by its path. Followed there, it converts that file
    // and leaves the folder's own v2.toml as it was.
    let home = TempHome::new();
    for d in ["proj/sub", "other"] {
        std::fs::create_dir_all(home.0.join(d)).unwrap();
    }
    // "# Çağrı" in cp1254; other/ has a v2.toml of its own, in UTF-8.
    std::fs::write(home.0.join("proj/v2.toml"), b"# \xc7a\xf0r\xfd\n").unwrap();
    std::fs::write(home.0.join("proj/a.v2"), "print(\"ok\")\n").unwrap();
    let mine = "# Çağrı'nın projesi\n";
    std::fs::write(home.0.join("other/v2.toml"), mine).unwrap();
    let run = |cwd: &str, file: &str| {
        let out = v2_command(&home.0).env("LC_ALL", "tr_TR.UTF-8").arg(file).current_dir(home.0.join(cwd)).stdin(Stdio::null()).output().unwrap();
        (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    let proj = std::fs::canonicalize(home.0.join("proj")).unwrap();
    let (file, new) = (proj.join("v2.toml").display().to_string(), proj.join("new.toml").display().to_string());
    let says = |files: &str| (Some(2), String::new(), format!("{file}: config error: the file is not valid UTF-8: it may be in an older encoding, such as Windows-1254: save it as UTF-8, as in iconv -f cp1254 -t utf-8 {files}, then check the text\n"));
    assert_eq!(run("proj", "a.v2"), says("v2.toml > new.toml && mv new.toml v2.toml"));
    let line = format!("{file} > {new} && mv {new} {file}");
    assert_eq!(run("other", "../proj/a.v2"), says(&line));
    assert_eq!(run("proj/sub", "../a.v2"), says(&line));
    let iconv = Command::new("sh").arg("-c").arg(format!("iconv -f cp1254 -t utf-8 {line}")).current_dir(home.0.join("other")).output().unwrap();
    if !iconv.status.success() {
        return; // no iconv with cp1254 here
    }
    assert_eq!(std::fs::read_to_string(home.0.join("other/v2.toml")).unwrap(), mine);
    assert_eq!(std::fs::read_to_string(proj.join("v2.toml")).unwrap(), "# Çağrı\n");
    assert_eq!(run("other", "../proj/a.v2"), (Some(0), "ok\n".to_string(), String::new()));
}

#[cfg(unix)]
#[test]
fn a_pasted_path_keeps_what_sh_expands_in_double_quotes() {
    // INTERNALS 19.1 (round 20): sh expands $, ` and \ in double quotes, so iconv's line for a
    // project in the folder a$b converted /…/a/v2.toml. In single quotes it converts that file.
    let home = TempHome::new();
    std::fs::create_dir_all(home.0.join("other")).unwrap();
    let iconv = Command::new("iconv").args(["-f", "cp1254", "-t", "utf-8", "/dev/null"]).output();
    let has_iconv = iconv.is_ok_and(|o| o.status.success());
    for name in ["a$b", "x`y", "b\\c"] {
        std::fs::create_dir_all(home.0.join(name)).unwrap();
        let proj = std::fs::canonicalize(home.0.join(name)).unwrap();
        std::fs::write(proj.join("v2.toml"), b"# \xc7a\xf0r\xfd\n").unwrap();
        std::fs::write(proj.join("a.v2"), "print(\"ok\")\n").unwrap();
        let script = format!("../{name}/a.v2");
        let run = || v2_command(&home.0).env("LC_ALL", "tr_TR.UTF-8").arg(&script).current_dir(home.0.join("other")).stdin(Stdio::null()).output().unwrap();
        let quoted = |file: &str| format!("'{}'", proj.join(file).display());
        let (file, new) = (quoted("v2.toml"), quoted("new.toml"));
        let line = format!("iconv -f cp1254 -t utf-8 {file} > {new} && mv {new} {file}");
        let err = String::from_utf8_lossy(&run().stderr).into_owned();
        assert!(err.ends_with(&format!(": save it as UTF-8, as in {line}, then check the text\n")), "{name}: {err}");
        if !has_iconv {
            continue; // no iconv with cp1254 here
        }
        let followed = Command::new("sh").arg("-c").arg(&line).current_dir(home.0.join("other")).output().unwrap();
        assert!(followed.status.success(), "{name}: {}", String::from_utf8_lossy(&followed.stderr));
        assert_eq!(std::fs::read_to_string(proj.join("v2.toml")).unwrap(), "# Çağrı\n", "{name}");
        let out = run();
        assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned()), (Some(0), "ok\n".to_string()), "{name}: {}", String::from_utf8_lossy(&out.stderr));
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
fn a_file_name_in_another_unicode_form_says_so() {
    // INTERNALS 19.1, 6.4, 5.10 (round 20): a file named as macOS writes names (decomposed, `o`
    // plus U+0308 for `ö`), asked for as a keyboard types it. macOS's file system finds one by the
    // other, so there they simply work.
    if cfg!(target_os = "macos") {
        return;
    }
    let (typed, nfd, csv) = ("gr\u{f6}\u{df}e.v2", "gro\u{308}\u{df}e.v2", "daten_u\u{308}.csv");
    let note = |n: &str| format!("there is a '{n}' whose name looks the same but is written in decomposed Unicode (letters plus combining accents, as macOS writes names): rename it");
    let files = [(nfd, "print(\"ran\")\n"), (csv, "a,b\n"), ("imp.v2", "import \"./gr\u{f6}\u{df}e.v2\" as g\n"), ("read.v2", "print(read_file(\"daten_\u{fc}.csv\"))\n")];
    let (code, _, stderr, home) = run_with_files(&files, &[typed]);
    assert_eq!(code, Some(2));
    assert!(stderr.ends_with(&format!("\n{}\n", note(nfd))), "{stderr}");
    for args in [&["imp.v2"][..], &["check", "imp.v2"]] {
        let (code, _, stderr) = run_in_home(&home.0, args, &[]);
        assert_eq!(code, Some(1));
        assert!(stderr.contains(&format!("ImportError: cannot find module './{typed}'")) && stderr.contains(&format!("\n  hint: {}\n", note(nfd))), "{stderr}");
    }
    let (code, _, stderr) = run_in_home(&home.0, &["read.v2"], &[]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains(&format!("\n  hint: {}\n", note(csv))), "{stderr}");
    // Renamed, as the hint says, the names are found.
    std::fs::rename(home.0.join(nfd), home.0.join(typed)).unwrap();
    std::fs::rename(home.0.join(csv), home.0.join("daten_\u{fc}.csv")).unwrap();
    assert_eq!(run_in_home(&home.0, &[typed], &[]).1, "ran\n");
    assert_eq!(run_in_home(&home.0, &["imp.v2"], &[]).1, "ran\n");
    assert_eq!(run_in_home(&home.0, &["read.v2"], &[]).1, "a,b\n\n");
}

#[test]
fn a_missing_module_names_the_file_it_probably_meant() {
    // INTERNALS 6.4 (round 20): a .v2 file of the import's folder within 5.9's did-you-mean
    // distance (accents and case too), as the path written with its name.
    let main = "func show(f) { try { f() } catch (e) { print(e.message) } }\n\
                show(func() { import \"./lib/utl.v2\" as a })\n\
                show(func() { import \"./lib/buyut.v2\" as b })\n\
                import \"./lib/Util.v2\" as u\n\
                print(u.x)\n\
                func lower() {\n    import \"./lib/util.v2\" as c\n    print(c.x)\n}\n\
                show(lower)\n";
    let files = [("main.v2", main), ("lib/Util.v2", "let x = 1\n"), ("lib/b\u{fc}y\u{fc}t.v2", "")];
    let (code, stdout, stderr, home) = run_with_files(&files, &["main.v2"]);
    assert_eq!(code, Some(0), "{stderr}");
    // Windows' and macOS's file systems find Util.v2 by util.v2, with 6.4's note.
    let case = if cfg!(target_os = "linux") {
        "cannot find module './lib/util.v2' (looked for lib/util.v2) — did you mean './lib/Util.v2'? (Linux matches the case of file names)\n"
    } else {
        "1\n"
    };
    assert_eq!(
        stdout,
        format!("cannot find module './lib/utl.v2' (looked for lib/utl.v2) — did you mean './lib/Util.v2'?\ncannot find module './lib/buyut.v2' (looked for lib/buyut.v2) — did you mean './lib/b\u{fc}y\u{fc}t.v2'?\n1\n{case}")
    );
    std::fs::write(home.0.join("typo.v2"), "import \"./lib/utl.v2\" as a\n").unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &["check", "typo.v2"], &[]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("ImportError: cannot find module './lib/utl.v2' (looked for lib/utl.v2) — did you mean './lib/Util.v2'?\n"), "{stderr}");
    // A separator at the path's end (round 20, fourth pass): the file, not './lib/Util/.v2' or
    // './lib/utl.v2/Util.v2'.
    let slash = "func show(f) { try { f() } catch (e) { print(e.message) } }\n\
                 show(func() { import \"./lib/Util/\" as a })\n\
                 show(func() { import \"./lib/utl.v2/\" as b })\n\
                 show(func() { import \"./lib/Util.v2/\" as c })\n";
    std::fs::write(home.0.join("slash.v2"), slash).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["slash.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    // The right file with a separator after it: Windows 11 and macOS import it; Linux and CI's
    // Windows Server don't, and name it (the fifth look at round 20). V2 asks the OS as this does.
    let found = std::fs::canonicalize(home.0.join("lib").join("Util.v2/")).is_ok();
    let itself = if found { "" } else { "cannot find module './lib/Util.v2/' (looked for lib/Util.v2) — did you mean './lib/Util.v2'?\n" };
    assert_eq!(
        stdout,
        format!("cannot find module './lib/Util/' (looked for lib/Util) — did you mean './lib/Util.v2'?\ncannot find module './lib/utl.v2/' (looked for lib/utl.v2) — did you mean './lib/Util.v2'?\n{itself}")
    );
}

#[test]
fn a_folder_in_another_unicode_form_says_so() {
    // INTERNALS 6.4, 5.10 (round 20's review): a folder on the way named as macOS writes names
    // (decomposed), and `import "./büyüt"` for a Mac's `büyüt.v2`, get the note: not nothing, nor
    // a make_dir hint that would make a second folder that looks the same. macOS finds them.
    if cfg!(target_os = "macos") {
        return;
    }
    let (folder, folder_nfd) = ("gr\u{f6}\u{df}e", "gro\u{308}\u{df}e");
    let (file, file_nfd) = ("b\u{fc}y\u{fc}t", "bu\u{308}yu\u{308}t");
    let (data, data_nfd) = ("\u{f6}rdner", "o\u{308}rdner");
    let note = |n: &str| format!("there is a '{n}' whose name looks the same but is written in decomposed Unicode (letters plus combining accents, as macOS writes names): rename it");
    let main = format!(
        "func show(f) {{ try {{ f()\n print(\"ok\") }} catch (e) {{ print(e.message, \"|\", e.hint) }} }}\n\
         show(func() {{ import \"./{folder}/u.v2\" as u }})\n\
         show(func() {{ import \"./{file}\" as b }})\n\
         show(func() => read_file(\"{data}/a.txt\"))\n\
         show(func() => write_file(\"{data}/new.txt\", \"x\"))\n"
    );
    let (u, b, a) = (format!("{folder_nfd}/u.v2"), format!("{file_nfd}.v2"), format!("{data_nfd}/a.txt"));
    let files = [("main.v2", main.as_str()), (u.as_str(), "let x = 1\n"), (b.as_str(), "let y = 2\n"), (a.as_str(), "a\n")];
    let (code, stdout, stderr, home) = run_with_files(&files, &["main.v2"]);
    assert_eq!(code, Some(0), "{stderr}");
    // Without its .v2, with the import that works once renamed (round 20, third pass).
    let with_v2 = format!("{}, and import './{file}.v2'", note(&b));
    assert_eq!(
        stdout,
        format!(
            "cannot find module './{folder}/u.v2' (looked for {folder}/u.v2) | {}\ncannot find module './{file}' (looked for {file}) | {with_v2}\ncannot read '{data}/a.txt': there is no folder '{data}' | {}\ncannot write '{data}/new.txt': there is no folder '{data}' | {}\n",
            note(folder_nfd),
            note(data_nfd),
            note(data_nfd)
        )
    );
    // With a separator at its end (fourth pass): the same import, not './büyüt/.v2'.
    let slash = format!("func show(f) {{ try {{ f() }} catch (e) {{ print(e.message, \"|\", e.hint) }} }}\nshow(func() {{ import \"./{file}/\" as b }})\n");
    std::fs::write(home.0.join("slash.v2"), slash).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["slash.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stdout, format!("cannot find module './{file}/' (looked for {file}) | {with_v2}\n"));
    // Renamed as the notes say: found, and `./büyüt` gets the .v2 it lacks (6.4); the import the
    // note names works (fixed.v2).
    std::fs::rename(home.0.join(folder_nfd), home.0.join(folder)).unwrap();
    std::fs::rename(home.0.join(&b), home.0.join(format!("{file}.v2"))).unwrap();
    std::fs::rename(home.0.join(data_nfd), home.0.join(data)).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["main.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stdout, format!("ok\ncannot find module './{file}' (looked for {file}) — did you mean './{file}.v2'? | null\nok\nok\n"));
    let (code, stdout, stderr) = run_in_home(&home.0, &["slash.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stdout, format!("cannot find module './{file}/' (looked for {file}) — did you mean './{file}.v2'? | null\n"));
    std::fs::write(home.0.join("fixed.v2"), format!("import \"./{file}.v2\" as b\nprint(b.y)\n")).unwrap();
    assert_eq!(run_in_home(&home.0, &["fixed.v2"], &[]).1, "2\n");
    // Written decomposed for a composed `naïve.v2`: the import, with the name composed.
    std::fs::write(home.0.join("na\u{ef}ve.v2"), "let z = 3\n").unwrap();
    std::fs::write(home.0.join("typed.v2"), "import \"./nai\u{308}ve\" as n\n").unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &["typed.v2"], &[]);
    assert_eq!(code, Some(1));
    let hint = "there is a 'na\u{ef}ve.v2' whose name looks the same, but the name given is written in decomposed Unicode (letters plus combining accents, as macOS writes names): type it again, and import './na\u{ef}ve.v2'";
    assert!(stderr.contains(&format!("\n  hint: {hint}\n")), "{stderr}");
    std::fs::write(home.0.join("typed.v2"), "import \"./na\u{ef}ve.v2\" as n\nprint(n.z)\n").unwrap();
    assert_eq!(run_in_home(&home.0, &["typed.v2"], &[]).1, "3\n");
}

#[test]
fn a_module_whose_v2_differs_in_case_is_named() {
    // INTERNALS 6.4 (round 20's review): `./lib/Util.V2` for lib/Util.v2, whose names without
    // `.v2` are the same, so 5.9's did-you-mean, which leaves out the name itself, can't name it.
    let files = [("main.v2", "import \"./lib/Util.V2\" as u\nprint(u.x)\n"), ("lib/Util.v2", "let x = 1\n")];
    let (code, stdout, stderr, _home) = run_with_files(&files, &["main.v2"]);
    if cfg!(target_os = "linux") {
        assert_eq!(code, Some(1));
        assert!(stderr.contains("ImportError: cannot find module './lib/Util.V2' (looked for lib/Util.V2) — did you mean './lib/Util.v2'? (Linux matches the case of file names)\n"), "{stderr}");
    } else {
        // Windows' and macOS's file systems find it, with 6.4's note.
        assert_eq!((code, stdout.as_str()), (Some(0), "1\n"), "{stderr}");
    }
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

#[cfg(unix)]
#[test]
fn exec_gives_the_program_its_name_as_written() {
    // INTERNALS 7.5 (round 20): it runs by its absolute path, but its argv[0] is the name as
    // written, as a shell passes it, so its messages start `ls:`, not `/usr/bin/ls:`.
    let src = "print(exec([\"sh\", \"-c\", \"echo $0\"]).stdout.trim())\nprint(exec([\"ls\", \"/no/such/v2-dir\"]).stderr.starts_with(\"ls: \"))\n";
    let (code, stdout, stderr) = run_generated("argv0.v2", src);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stdout, "sh\ntrue\n");
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
    // Host), but for a closed pipe, which stops the run quietly with 141, as on stdout (6.5;
    // round 22, L8).
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
    assert_eq!((code, stdout.as_str()), (Some(141), ""));
    let Some(stderr) = failing_stdout(&home.0) else { return };
    let child = v2_command(&home.0).arg("closed.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(stderr).spawn().expect("start v2");
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(DEFAULT_TIMEOUT_SECS)).unwrap();
    assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stdout).as_ref()), (Some(0), "IOError cannot write to stderr 2\n"));
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
fn an_embeddable_python_and_a_long_pythonpath_in_doctor_and_a_crash() {
    // Round 22 (1.3, 3, 4), INTERNALS 8.4, 19.2: Python's embeddable package (a python313._pth
    // beside its python) ignores PYTHONPATH, so doctor says what it reads instead and a crash
    // doesn't blame the variable; another Python's PYTHONPATH is shown whole (it was cut at 80
    // characters with no mark), its control characters escaped; V2_ENGINE_PY over the project's
    // venv is named. A fake Python that answers --version and doctor's -c check, and exits 1 as
    // a worker: no real Python needed.
    let home = TempHome::new();
    let (name, script) = if cfg!(windows) {
        ("python.bat", "@echo off\r\nif \"%~1\"==\"--version\" (\r\n  echo Python 3.13.9\r\n  exit /b 0\r\n)\r\nif \"%~1\"==\"-c\" exit /b 0\r\nexit /b 1\r\n")
    } else {
        ("python3", "#!/bin/sh\ncase \"$1\" in\n  --version) echo 'Python 3.13.9' ;;\n  -c) ;;\n  *) exit 1 ;;\nesac\n")
    };
    let program = |folder: &str| {
        let p = home.0.join(folder).join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, script).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        p
    };
    let (embed, full) = (program("embed"), program("full"));
    std::fs::write(home.0.join("embed").join("python313._pth"), "python313.zip\n.\n").unwrap();
    std::fs::write(home.0.join("start.v2"), "@py { 1 }\n").unwrap();
    std::fs::write(home.0.join("v2.toml"), "[python]\nvenv = \".venv\"\n").unwrap();
    // A venv this system could use (round 22 review, 3): its python and pyvenv.cfg.
    let venv_python = home.0.join(".venv").join(if cfg!(windows) { "Scripts\\python.exe" } else { "bin/python" });
    std::fs::create_dir_all(venv_python.parent().unwrap()).unwrap();
    std::fs::write(&venv_python, "").unwrap();
    std::fs::write(home.0.join(".venv").join("pyvenv.cfg"), "home = x\nversion = 3.12.3\n").unwrap();
    let long = format!("{}\u{1b}{};end", "d".repeat(100), "e".repeat(20));
    let shown = format!("{}\\u{{1b}}{};end", "d".repeat(100), "e".repeat(20));
    let env = |python: &Path| vec![("V2_ENGINE_PY", python.as_os_str().to_owned()), ("PYTHONPATH", std::ffi::OsString::from(&long))];
    let run = |python: &Path, args: &[&str]| {
        let env = env(python);
        let pairs: Vec<(&str, &std::ffi::OsStr)> = env.iter().map(|(k, v)| (*k, v.as_os_str())).collect();
        run_in_home(&home.0, args, &pairs)
    };
    let unset = if cfg!(windows) { "Remove-Item Env:V2_ENGINE_PY" } else { "unset V2_ENGINE_PY" };
    let unused = format!(".venv' (v2.toml) isn't used: to use it, {unset}\n");
    let (code, stdout, _) = run(&full, &["doctor"]);
    assert_eq!((code, doctor_rows(&stdout)["py"][0].as_str()), (Some(0), "ok"), "{stdout}");
    assert!(stdout.contains(&format!("\n  py: PYTHONPATH is set ({shown}), which Python reads as it starts\n")), "{stdout}");
    assert!(stdout.contains("\n  py: V2_ENGINE_PY is set, so [python] venv '") && stdout.contains(&unused), "{stdout}");
    assert!(!stdout.contains("embeddable"), "{stdout}");
    // Round 22 review (3): V2_ENGINE_PY naming that venv's own python leaves nothing unused.
    let (code, stdout, _) = run(&venv_python, &["doctor"]);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(!stdout.contains("isn't used"), "{stdout}");
    let (code, _, stderr) = run(&full, &["start.v2"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains(&format!("exited during startup (exit code 1); see its error output above, if any; PYTHONPATH is set ({shown}), which it reads as it starts: unset it or fix it\n")), "{stderr}");
    let (code, stdout, _) = run(&embed, &["doctor"]);
    assert_eq!((code, doctor_rows(&stdout)["py"][0].as_str()), (Some(0), "ok"), "{stdout}");
    assert!(stdout.contains("\n  py: this is Python's embeddable package: its python313._pth decides sys.path (PYTHONPATH is ignored), and it has no pip or venv\n"), "{stdout}");
    assert!(!stdout.contains("PYTHONPATH is set") && stdout.contains(&unused), "{stdout}");
    let (code, _, stderr) = run(&embed, &["start.v2"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("exited during startup (exit code 1); see its error output above, if any\n") && !stderr.contains("PYTHONPATH"), "{stderr}");
}

#[test]
fn a_real_embeddable_python_gets_what_works_there() {
    // Round 22 (1): with V2_TEST_EMBEDDABLE_PY naming the python.exe of python.org's embeddable
    // zip (CI has none, so it is skipped there): blocks run; a missing package's hint says it
    // has no pip, a missing standard module (tkinter) that pip can't add it, doctor what it reads
    // instead of PYTHONPATH, and [python] venv naming its folder offers no -m venv.
    let Some(python) = std::env::var_os("V2_TEST_EMBEDDABLE_PY").map(PathBuf::from) else {
        eprintln!("SKIP a_real_embeddable_python_gets_what_works_there: V2_TEST_EMBEDDABLE_PY isn't set");
        return;
    };
    let home = TempHome::new();
    std::fs::write(home.0.join("ok.v2"), "print(@py { 6 * 7 })\n").unwrap();
    std::fs::write(home.0.join("pkg.v2"), "@import { naturalsize } from py.zz_humanize_v2\nprint(naturalsize(1))\n").unwrap();
    std::fs::write(home.0.join("tk.v2"), "@import { Tk } from py.tkinter\nprint(Tk)\n").unwrap();
    let env = [("V2_ENGINE_PY", python.as_os_str()), ("PYTHONPATH", std::ffi::OsStr::new("nowhere"))];
    let (code, stdout, stderr) = run_in_home(&home.0, &["ok.v2"], &env);
    assert_eq!((code, stdout.as_str()), (Some(0), "42\n"), "{stderr}");
    let (code, _, stderr) = run_in_home(&home.0, &["pkg.v2"], &env);
    assert_eq!(code, Some(1), "{stderr}");
    let none = "hint: this is Python's embeddable package, which has no pip: for packages, use a full Python (python.org's installer, or the nuget package) and a venv\n";
    assert!(stderr.contains("ImportError: No module named 'zz_humanize_v2'") && stderr.contains(none), "{stderr}");
    let (code, _, stderr) = run_in_home(&home.0, &["tk.v2"], &env);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("hint: 'tkinter' is part of Python's standard library, which this Python lacks: pip can't add it; use a full Python\n"), "{stderr}");
    let (code, stdout, _) = run_in_home(&home.0, &["doctor"], &env);
    assert_eq!((code, doctor_rows(&stdout)["py"][0].as_str()), (Some(0), "ok"), "{stdout}");
    assert!(stdout.contains("\n  py: this is Python's embeddable package: its python") && stdout.contains("._pth decides sys.path (PYTHONPATH is ignored), and it has no pip or venv\n"), "{stdout}");
    assert!(!stdout.contains("PYTHONPATH is set"), "{stdout}");
    let folder = python.parent().unwrap().display().to_string();
    std::fs::write(home.0.join("v2.toml"), format!("[python]\nvenv = '{folder}'\n")).unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &["ok.v2"], &[]);
    assert_eq!(code, Some(3), "{stderr}");
    assert!(stderr.contains("is a Python installation, not a virtual environment: name its python instead, [engines.py] command = ") && !stderr.contains("-m venv"), "{stderr}");
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
fn a_js_crash_after_startup_names_node_options() {
    // Round 20, INTERNALS 8.6: NODE_OPTIONS can stop Node after startup too (a heap limit), so a
    // js session's crash during a block, or while idle, names it when it is set.
    if !needs("js", "a_js_crash_after_startup_names_node_options") {
        return;
    }
    let during = "print(@js { 1 })\ntry {\n    @js { process.kill(process.pid, \"SIGKILL\") }\n} catch (e) {\n    print(e.type, e.message)\n}\n";
    // The worker's event loop waits while it is idle: the run kills it.
    let kill = if cfg!(windows) { "[\"taskkill\", \"/F\", \"/PID\", str(pid)]" } else { "[\"kill\", \"-9\", str(pid)]" };
    let idle = format!("let pid = @js {{ process.pid }}\nlet r = exec({kill})\nsleep(1)\ntry {{\n    @js {{ 2 }}\n}} catch (e) {{\n    print(e.type, e.message)\n}}\n");
    let fix = "; NODE_OPTIONS is set (--max-old-space-size=4096), which it reads as it starts: unset it or fix it\n";
    let files = [("during.v2", during), ("idle.v2", idle.as_str())];
    let env = [("NODE_OPTIONS", "--max-old-space-size=4096")];
    let (code, stdout, stderr, _home) = run_with_files_env(&files, &["during.v2"], &env);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.starts_with("1\nEngineCrashed engine 'js' (pid ") && stdout.ends_with(&format!("; see its error output above, if any{fix}")), "{stdout}");
    let (code, stdout, stderr, _home) = run_with_files_env(&files, &["idle.v2"], &env);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.starts_with("EngineCrashed engine 'js' exited while idle (") && stdout.ends_with(fix), "{stdout}");
    // Not set: no such clause.
    if std::env::var_os("NODE_OPTIONS").is_none() {
        let (_, stdout, _, _home) = run_with_files_env(&files, &["during.v2"], &[]);
        assert!(stdout.ends_with("; see its error output above, if any\n"), "{stdout}");
    }
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

/// Windows: the test process has a console, which its children share: one made for it (its
/// standard handles kept) when it has none, as a CI runner may start the tests without one.
#[cfg(windows)]
fn attach_console() {
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
}

/// Windows: put the child in its own process group, so Ctrl+Break can be sent to it alone. The
/// event travels through a console this process shares with the child (`attach_console`).
fn interruptible(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        attach_console();
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    let _ = cmd;
}

/// Interrupt `child` the way a terminal does: SIGINT, or Ctrl+Break to its process group.
/// A .v2 file in an older encoding: what V2 says, which on Linux and macOS names iconv (2.1),
/// with the code page of the locale's language (`LC_ALL=C.UTF-8` here: cp1252).
const OLDER: &str = if cfg!(windows) {
    "file is not valid UTF-8: it may be in an older encoding, such as Windows-1252 (what Windows PowerShell's Set-Content writes): save it as UTF-8"
} else {
    "file is not valid UTF-8: it may be in an older encoding, such as Windows-1252 or Latin-1: save it as UTF-8, as in iconv -f cp1252 -t utf-8 old.v2 > new.v2, then check the text"
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

#[test]
fn runs_started_together_keep_their_run_dirs() {
    // Round 20, INTERNALS 20.3: a run's sweep of leftovers removed the folder another run had just
    // made, which could look older than that run; runs started together with one temp folder all
    // succeed, and leave nothing behind.
    if !needs("py", "runs_started_together_keep_their_run_dirs") {
        return;
    }
    let home = TempHome::new();
    std::fs::write(home.0.join("race.v2"), "print(@py { \"ok\" })\n").unwrap();
    let tmp = home.0.join("tmp");
    for round in 0..6 {
        let runs: Vec<Child> = (0..8)
            .map(|_| {
                let mut cmd = v2_command(&home.0);
                cmd.arg("race.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
                with_temp(&mut cmd, &tmp);
                cmd.spawn().expect("start v2")
            })
            .collect();
        for child in runs {
            let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(60)).unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert_eq!((out.status.code(), stdout.as_ref()), (Some(0), "ok\n"), "round {round}: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
    assert_eq!(names_in(&tmp), Vec::<String>::new(), "a run left its run dir");
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

/// A pseudo-terminal (Unix): v2 gets the slave as its stdin, stdout, stderr and controlling
/// terminal; the test reads what it draws from the master and types into it.
#[cfg(unix)]
struct Pty {
    master: std::fs::File,
    slave: std::fs::File,
}

#[cfg(unix)]
impl Pty {
    /// A terminal of `columns` x `rows`.
    fn open(columns: u16, rows: u16) -> Pty {
        use std::os::fd::FromRawFd;
        let (mut master, mut slave) = (-1, -1);
        let mut size = libc::winsize { ws_row: rows, ws_col: columns, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: openpty fills two descriptors; no name or settings are asked for.
        let r = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::addr_of_mut!(size)) };
        assert_eq!(r, 0, "openpty: {}", std::io::Error::last_os_error());
        for fd in [master, slave] {
            // SAFETY: a plain fcntl on a descriptor this function owns: other tests' children don't
            // inherit it.
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        // SAFETY: as above; reads of the master don't wait (macOS's poll doesn't take devices).
        unsafe { libc::fcntl(master, libc::F_SETFL, libc::fcntl(master, libc::F_GETFL) | libc::O_NONBLOCK) };
        // SAFETY: both descriptors are open, and the files own them from here on.
        unsafe { Pty { master: std::fs::File::from_raw_fd(master), slave: std::fs::File::from_raw_fd(slave) } }
    }

    /// `cmd` on the terminal, in a session of its own whose foreground job it is (as a shell starts
    /// a command), with SIGINT's default action, which a test runner may have set to ignored.
    fn attach(&self, cmd: &mut Command) {
        use std::os::unix::process::CommandExt;
        let end = || Stdio::from(self.slave.try_clone().expect("the terminal's slave"));
        cmd.stdin(end()).stdout(end()).stderr(end());
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                Ok(())
            });
        }
    }

    /// Read what is drawn into `seen` until `done` holds or `deadline` passes; whether it held.
    fn read_until(&mut self, seen: &mut Vec<u8>, deadline: Instant, done: impl Fn(&[u8]) -> bool) -> bool {
        let mut buf = [0u8; 4096];
        while !done(seen) {
            if Instant::now() > deadline {
                return false;
            }
            match self.master.read(&mut buf) {
                Ok(n) if n > 0 => seen.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                _ => return done(seen),
            }
        }
        true
    }

    /// Whatever is drawn now, into `seen`, without waiting.
    fn drain(&mut self, seen: &mut Vec<u8>) {
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = self.master.read(&mut buf) {
            seen.extend_from_slice(&buf[..n]);
        }
    }

    /// Wait for `child` until `deadline`, reading what is drawn into `seen` meanwhile: on macOS a
    /// session leader's exit waits until its terminal's output has been read (XNU's `ttywait`), so a
    /// test that stopped reading would wait for ever. Past the deadline it is killed and reaped
    /// (`reap`): `None`.
    fn wait(&mut self, child: &mut Child, seen: &mut Vec<u8>, deadline: Instant) -> Option<std::process::ExitStatus> {
        loop {
            self.drain(seen);
            if let Some(status) = child.try_wait().unwrap() {
                return Some(status);
            }
            if Instant::now() > deadline {
                self.reap(child, seen);
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Kill `child` and reap it, reading what is drawn meanwhile (`wait`), for at most 10 s.
    fn reap(&mut self, child: &mut Child, seen: &mut Vec<u8>) {
        let _ = child.kill();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            self.drain(seen);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The terminal's input, output, control and local flags, read through the master: once v2, the
    /// session leader, has ended, macOS has revoked the slave (its descriptors are no terminal any
    /// more), and Linux and macOS give a master the slave's settings.
    fn flags(&self) -> [libc::tcflag_t; 4] {
        use std::os::fd::AsRawFd;
        // SAFETY: tcgetattr fills a zeroed struct.
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::tcgetattr(self.master.as_raw_fd(), &mut t) }, 0, "tcgetattr: {}", std::io::Error::last_os_error());
        [t.c_iflag, t.c_oflag, t.c_cflag, t.c_lflag]
    }
}

#[cfg(unix)]
#[test]
fn an_interrupt_gives_the_terminal_back() {
    // DOCS 19.1, INTERNALS 6.5 (round 22, L5): Ctrl+C while a block has turned the terminal's echo
    // off, or while a live display hides the cursor, stops the block before its own cleanup runs;
    // the terminal gets back the settings it had when v2 started, and the cursor is shown.
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
    let mut cases = vec![("v2", "print(\"\\x1b[?25l\", end: \"\")\nprint(\"ready\")\nwhile true {\n    sleep(0.05)\n}\n")];
    if needs("py", "an_interrupt_gives_the_terminal_back") {
        let src = "@py {\n    import termios, time\n    a = termios.tcgetattr(2)\n    a[3] &= ~(termios.ECHO | termios.ICANON)\n    termios.tcsetattr(2, termios.TCSANOW, a)\n    print(\"\\x1b[?25l\", end=\"\")\n    print(\"ready\", flush=True)\n    time.sleep(30)\n}\n";
        cases.push(("py", src));
    }
    for (what, src) in cases {
        let home = TempHome::new();
        std::fs::write(home.0.join("tty.v2"), src).unwrap();
        let mut pty = Pty::open(80, 24);
        let before = pty.flags();
        let mut cmd = v2_command(&home.0);
        cmd.arg("tty.v2").current_dir(&home.0);
        pty.attach(&mut cmd);
        let mut child = cmd.spawn().expect("start v2");
        let mut seen = Vec::new();
        let ready = pty.read_until(&mut seen, Instant::now() + Duration::from_secs(30), |s| find(s, b"ready").is_some());
        let changed = pty.flags();
        if !ready || (what == "py" && changed[3] & libc::ECHO != 0) {
            pty.reap(&mut child, &mut seen);
            panic!("{what}: the program never got ready with echo off: {}", String::from_utf8_lossy(&seen).escape_debug());
        }
        // Ctrl+C, typed.
        pty.master.write_all(b"\x03").unwrap();
        let Some(status) = pty.wait(&mut child, &mut seen, Instant::now() + Duration::from_secs(10)) else {
            panic!("{what}: v2 didn't stop on Ctrl+C");
        };
        let at = find(&seen, b"ready").unwrap();
        pty.read_until(&mut seen, Instant::now() + Duration::from_secs(2), |s| find(&s[at..], b"\x1b[?25h").is_some());
        let shown = String::from_utf8_lossy(&seen).escape_debug().to_string();
        assert_interrupted(&status, SIGINT, &shown);
        assert!(find(&seen[at..], b"\x1b[?25h").is_some(), "{what}: the cursor wasn't shown again: {shown}");
        assert_eq!(pty.flags(), before, "{what}: the terminal's settings weren't put back: {shown}");
    }
}

#[cfg(unix)]
#[test]
fn ctrl_c_leaves_a_pagers_terminal_mode_alone() {
    // DOCS 19.1, INTERNALS 6.5 (round 22, the review of L5): in `v2 x.v2 | less` Ctrl+C reaches the
    // whole pipeline, and the pager, still running, has the terminal in a mode of its own (no echo,
    // no line input): v2, whose stdout is the pipe, leaves it as it is and shows no cursor. The
    // pager here is a Python reader of v2's output that sets that mode once v2 has started.
    if !needs("py", "ctrl_c_leaves_a_pagers_terminal_mode_alone") {
        return;
    }
    let home = TempHome::new();
    std::fs::write(home.0.join("loop.v2"), "let i = 0\nwhile true {\n    print(i)\n    i += 1\n    sleep(0.1)\n}\n").unwrap();
    let pager = r#"import os, signal, subprocess, sys, termios
signal.signal(signal.SIGINT, lambda *a: None)  # as less, which handles Ctrl+C itself
v2 = subprocess.Popen(sys.argv[1:], stdout=subprocess.PIPE)
v2.stdout.readline()  # v2 has started and seen the terminal as it was
saved = termios.tcgetattr(0)
raw = termios.tcgetattr(0)
raw[3] &= ~(termios.ECHO | termios.ICANON)
termios.tcsetattr(0, termios.TCSANOW, raw)
os.write(1, b"ready\n")
v2.stdout.read()
rc = v2.wait()
a = termios.tcgetattr(0)
os.write(1, ("mode echo=%s icanon=%s v2=%s\n" % (bool(a[3] & termios.ECHO), bool(a[3] & termios.ICANON), rc)).encode())
termios.tcsetattr(0, termios.TCSANOW, saved)
"#;
    std::fs::write(home.0.join("pager.py"), pager).unwrap();
    let mut pty = Pty::open(80, 24);
    let mut cmd = Command::new("python3");
    cmd.args(["pager.py", BIN, "loop.v2"]).current_dir(&home.0).env("HOME", &home.0).env_remove("VIRTUAL_ENV");
    pty.attach(&mut cmd);
    let mut child = cmd.spawn().expect("start python3");
    drop(cmd);
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
    let mut seen = Vec::new();
    if !pty.read_until(&mut seen, Instant::now() + Duration::from_secs(30), |s| find(s, b"ready").is_some()) {
        pty.reap(&mut child, &mut seen);
        panic!("the pager never got ready: {}", String::from_utf8_lossy(&seen).escape_debug());
    }
    // Ctrl+C, typed: to the pager and v2 alike.
    pty.master.write_all(b"\x03").unwrap();
    if pty.wait(&mut child, &mut seen, Instant::now() + Duration::from_secs(10)).is_none() {
        panic!("the pager didn't end after v2 did: {}", String::from_utf8_lossy(&seen).escape_debug());
    }
    pty.read_until(&mut seen, Instant::now() + Duration::from_secs(2), |s| find(s, b"v2=").is_some_and(|i| s[i..].contains(&b'\n')));
    let shown = String::from_utf8_lossy(&seen).escape_debug().to_string();
    assert!(shown.contains("mode echo=False icanon=False v2=-2"), "{shown}");
    assert!(find(&seen, b"\x1b[?25h").is_none(), "v2 wrote to the pager's terminal: {shown}");
}

#[cfg(unix)]
#[test]
fn ends_other_than_the_normal_one_show_the_cursor_again() {
    // DOCS 4.6, INTERNALS 6.5 (round 22, the second review, issue 3): a block that hid the cursor,
    // as a live display does, and never showed it again: after exit(n) from a callback, an uncaught
    // error, or a closed pipe on stderr, v2 (whose stdout is the terminal) shows it; after the
    // normal end it writes nothing of its own.
    if !needs("py", "ends_other_than_the_normal_one_show_the_cursor_again") {
        return;
    }
    let hide = "    print(\"\\x1b[?25l\", end=\"\", flush=True)\n";
    let cases = [
        ("exit(3)", format!("func stop() {{ exit(3) }}\n@py(stop) {{\n{hide}    stop()\n}}\nprint(\"not reached\")\n"), Some(3), false, true),
        ("an uncaught error", format!("@py {{\n{hide}    raise ValueError(\"boom\")\n}}\n"), Some(1), false, true),
        ("a closed stderr", format!("@py {{\n    import sys\n{hide}    while True:\n        print(\"x\", file=sys.stderr)\n}}\n"), Some(141), true, true),
        ("the normal end", format!("@py {{\n{hide}}}\nprint(\"end\")\n"), Some(0), false, false),
    ];
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
    for (what, src, code, closed_stderr, shown) in cases {
        let home = TempHome::new();
        std::fs::write(home.0.join("main.v2"), src).unwrap();
        let mut pty = Pty::open(80, 24);
        let mut cmd = v2_command(&home.0);
        cmd.arg("main.v2").current_dir(&home.0);
        pty.attach(&mut cmd);
        if closed_stderr {
            let (reader, writer) = std::io::pipe().expect("a pipe");
            drop(reader);
            cmd.stderr(writer);
        }
        let mut child = cmd.spawn().expect("start v2");
        drop(cmd);
        let mut seen = Vec::new();
        let Some(status) = pty.wait(&mut child, &mut seen, Instant::now() + Duration::from_secs(60)) else {
            panic!("{what}: v2 didn't end");
        };
        let wait = Duration::from_millis(if shown { 2000 } else { 300 });
        pty.read_until(&mut seen, Instant::now() + wait, |s| find(s, b"\x1b[?25l").is_some_and(|i| find(&s[i..], b"\x1b[?25h").is_some()));
        let screen = String::from_utf8_lossy(&seen).escape_debug().to_string();
        assert_eq!(status.code(), code, "{what}: {screen}");
        let at = find(&seen, b"\x1b[?25l").unwrap_or_else(|| panic!("{what}: the block didn't hide the cursor: {screen}"));
        assert_eq!(find(&seen[at..], b"\x1b[?25h").is_some(), shown, "{what}: {screen}");
    }
}

#[cfg(unix)]
#[test]
fn blocks_see_v2s_terminal() {
    // DOCS 4.6, INTERNALS 9.3 and 9.9 (round 22, L0-L2): a block's isatty answers, terminal sizes
    // and Node's tty methods are those of where V2 writes each stream: both on a 132 x 40 terminal;
    // stdout to a file (`> out.txt`: no escape codes in it); stderr to a file (`2> e.log`: the
    // colors stay on the screen). tests/cross/terminal_answers.v2 is the program, and its .out the
    // answers with neither stream a terminal.
    if !needs("py", "blocks_see_v2s_terminal") || !needs("js", "blocks_see_v2s_terminal") {
        return;
    }
    let src = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cross/terminal_answers.v2")).unwrap();
    let on_screen = |py: &str, js: &str| format!("{py}\n{js}\n\x1b[2K\x1b[1Gbar 132x40 8 true\n\x1b[32mok\x1b[39m\n{{ a: \x1b[33m1\x1b[39m }}\nend\n");
    let cases = [
        ("on the terminal", None, on_screen("py True True True True (132, 40) (132, 40) (132, 40)", "js true true true true 132 40 132 function")),
        ("stdout to a file", Some("out.txt"), "py False True False True (80, 24) None (132, 40)\njs false true false true undefined undefined 132 undefined\nok\n{ a: 1 }\nend\n".to_string()),
        ("stderr to a file", Some("err.txt"), on_screen("py True False True False (132, 40) (132, 40) None", "js true false true false 132 40 undefined function")),
    ];
    for (what, file, want) in cases {
        let home = TempHome::new();
        std::fs::write(home.0.join("main.v2"), &src).unwrap();
        let mut pty = Pty::open(132, 40);
        let mut cmd = v2_command(&home.0);
        cmd.arg("main.v2").current_dir(&home.0).env("TERM", "xterm-256color");
        // What else Node's color depth and shutil's size read.
        for var in ["COLUMNS", "LINES", "CI", "FORCE_COLOR", "NO_COLOR", "NODE_DISABLE_COLORS", "TMUX", "TERM_PROGRAM", "COLORTERM", "TEAMCITY_VERSION"] {
            cmd.env_remove(var);
        }
        pty.attach(&mut cmd);
        let redirected = file.map(|f| std::fs::File::create(home.0.join(f)).unwrap());
        match (file, &redirected) {
            (Some("out.txt"), Some(f)) => cmd.stdout(f.try_clone().unwrap()),
            (Some(_), Some(f)) => cmd.stderr(f.try_clone().unwrap()),
            _ => &mut cmd,
        };
        let mut child = cmd.spawn().expect("start v2");
        drop(cmd);
        let mut seen = Vec::new();
        let Some(status) = pty.wait(&mut child, &mut seen, Instant::now() + Duration::from_secs(60)) else {
            panic!("{what}: v2 didn't end");
        };
        let wait = Duration::from_millis(if file == Some("out.txt") { 200 } else { 2000 });
        pty.read_until(&mut seen, Instant::now() + wait, |s| s.ends_with(b"end\r\n"));
        let screen = String::from_utf8_lossy(&seen).replace("\r\n", "\n");
        let read = |f: &str| std::fs::read_to_string(home.0.join(f)).unwrap();
        let (got, other) = match file {
            Some("out.txt") => (read("out.txt"), screen.clone()),
            Some(f) => (screen.clone(), read(f)),
            None => (screen.clone(), String::new()),
        };
        assert_eq!(status.code(), Some(0), "{what}: {}", other.escape_debug());
        assert_eq!(got, want, "{what}: {}", got.escape_debug());
        assert!(!other.contains("Error"), "{what}: {}", other.escape_debug());
    }
}

/// Windows: the test process's console (`attach_console`, a hidden one where the tests run without
/// a window), one test at a time, with VT processing off and the cursor shown, as conhost starts;
/// put back as it was when the test ends.
#[cfg(windows)]
struct Console {
    file: std::fs::File,
    mode: u32,
    cursor: windows_sys::Win32::System::Console::CONSOLE_CURSOR_INFO,
    _turn: std::sync::MutexGuard<'static, ()>,
}

#[cfg(windows)]
impl Console {
    fn take() -> Console {
        use windows_sys::Win32::System::Console::{GetConsoleCursorInfo, SetConsoleCursorInfo, CONSOLE_CURSOR_INFO};
        static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        attach_console();
        let file = std::fs::OpenOptions::new().read(true).write(true).open("CONOUT$").expect("the console's screen buffer");
        let (mut mode, mut cursor) = (0, CONSOLE_CURSOR_INFO { dwSize: 25, bVisible: 1 });
        // SAFETY: plain calls on a handle this test owns; each fills its out-parameter.
        unsafe {
            assert_ne!(windows_sys::Win32::System::Console::GetConsoleMode(Self::raw(&file), &mut mode), 0, "{}", std::io::Error::last_os_error());
            GetConsoleCursorInfo(Self::raw(&file), &mut cursor);
        }
        let console = Console { file, mode, cursor, _turn: turn };
        console.set_mode(mode & !VT);
        let shown = CONSOLE_CURSOR_INFO { dwSize: cursor.dwSize, bVisible: 1 };
        // SAFETY: as above.
        unsafe { SetConsoleCursorInfo(console.handle(), &shown) };
        console
    }

    fn raw(file: &std::fs::File) -> windows_sys::Win32::Foundation::HANDLE {
        std::os::windows::io::AsRawHandle::as_raw_handle(file)
    }

    fn handle(&self) -> windows_sys::Win32::Foundation::HANDLE {
        Self::raw(&self.file)
    }

    fn mode(&self) -> u32 {
        let mut mode = 0;
        // SAFETY: fills its out-parameter.
        assert_ne!(unsafe { windows_sys::Win32::System::Console::GetConsoleMode(self.handle(), &mut mode) }, 0, "{}", std::io::Error::last_os_error());
        mode
    }

    fn set_mode(&self, mode: u32) {
        // SAFETY: a plain call on this test's handle.
        unsafe { windows_sys::Win32::System::Console::SetConsoleMode(self.handle(), mode) };
    }

    fn cursor_shown(&self) -> bool {
        let mut info = windows_sys::Win32::System::Console::CONSOLE_CURSOR_INFO { dwSize: 0, bVisible: 0 };
        // SAFETY: fills its out-parameter.
        unsafe { windows_sys::Win32::System::Console::GetConsoleCursorInfo(self.handle(), &mut info) };
        info.bVisible != 0
    }

    /// The console's window: columns and rows.
    fn size(&self) -> (i32, i32) {
        // SAFETY: fills a zeroed struct.
        let mut info: windows_sys::Win32::System::Console::CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
        assert_ne!(unsafe { windows_sys::Win32::System::Console::GetConsoleScreenBufferInfo(self.handle(), &mut info) }, 0);
        let w = info.srWindow;
        (i32::from(w.Right) - i32::from(w.Left) + 1, i32::from(w.Bottom) - i32::from(w.Top) + 1)
    }

    fn stdio(&self) -> Stdio {
        Stdio::from(self.file.try_clone().unwrap())
    }
}

#[cfg(windows)]
impl Drop for Console {
    fn drop(&mut self) {
        self.set_mode(self.mode);
        // SAFETY: a plain call on this test's handle.
        unsafe { windows_sys::Win32::System::Console::SetConsoleCursorInfo(self.handle(), &self.cursor) };
    }
}

#[cfg(windows)]
const VT: u32 = windows_sys::Win32::System::Console::ENABLE_VIRTUAL_TERMINAL_PROCESSING;

#[cfg(windows)]
#[test]
fn a_console_is_v2s_terminal_to_blocks_and_renders_escape_codes() {
    // DOCS 4.6, INTERNALS 6.5 and 9.9 (round 22, L0-L2): with stdout on a console and stderr on a
    // pipe, blocks see a terminal of the console's size on stdout only; while v2 runs the console
    // has VT processing on (so colors render in conhost), and afterwards its mode is as before.
    if !needs("py", "a_console_is_v2s_terminal_to_blocks_and_renders_escape_codes") || !needs("js", "a_console_is_v2s_terminal_to_blocks_and_renders_escape_codes") {
        return;
    }
    let console = Console::take();
    let mode = console.mode();
    let src = r#"let py = @py {
    import ctypes, os, shutil, sys
    from ctypes import wintypes
    k = ctypes.WinDLL("kernel32", use_last_error=True)
    k.CreateFileW.restype = wintypes.HANDLE
    h = k.CreateFileW("CONOUT$", 0xC0000000, 3, None, 3, 0, None)
    m = wintypes.DWORD()
    k.GetConsoleMode(wintypes.HANDLE(h), ctypes.byref(m))
    k.CloseHandle(wintypes.HANDLE(h))
    f"py {sys.stdout.isatty()} {sys.stderr.isatty()} {os.isatty(1)} {os.isatty(2)} {tuple(shutil.get_terminal_size())} vt {bool(m.value & 4)}"
}
let js = @js {
    const tty = require("tty");
    `js ${process.stdout.isTTY} ${process.stderr.isTTY} ${tty.isatty(1)} ${tty.isatty(2)} ${process.stdout.columns}x${process.stdout.rows}`
}
write_file("answers.txt", f"${py}\n${js}\n")
"#;
    let home = TempHome::new();
    std::fs::write(home.0.join("main.v2"), src).unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg("main.v2").current_dir(&home.0).env_remove("COLUMNS").env_remove("LINES");
    cmd.stdin(Stdio::null()).stdout(console.stdio()).stderr(Stdio::piped());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let (c, r) = console.size();
    let want = format!("py True False True False ({c}, {r}) vt True\njs true false true false {c}x{r}\n");
    assert_eq!(std::fs::read_to_string(home.0.join("answers.txt")).unwrap(), want, "{stderr}");
    assert_eq!(console.mode(), mode, "the console's mode is as it was before v2 ran");
}

#[cfg(windows)]
#[test]
fn a_console_gets_its_cursor_and_mode_back_however_v2_ends() {
    // INTERNALS 6.5 (round 22, the second review, issues 2, 3 and 8): v2's stdout on a console with
    // VT processing off. Ctrl+Break while the cursor is hidden (as a live display hides it), and
    // exit(3) from a callback while a block has it hidden: the cursor is shown again before the
    // console's mode is put back. A closed pipe on stdout, stderr on the console: the mode is put
    // back on that path too.
    let console = Console::take();
    let mode = console.mode();
    let home = TempHome::new();
    // Ctrl+Break, with V2 alone: its stdout is the console.
    std::fs::write(home.0.join("hide.v2"), "print(\"\\x1b[?25l\", end: \"\")\nwrite_file(\"ready\", \"1\")\nwhile true {\n    sleep(0.05)\n}\n").unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg("hide.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(console.stdio()).stderr(Stdio::piped());
    interruptible(&mut cmd);
    let child = cmd.spawn().expect("start v2");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !home.0.join("ready").exists() {
        assert!(Instant::now() < deadline, "hide.v2 never got ready");
        std::thread::sleep(Duration::from_millis(20));
    }
    let hidden = !console.cursor_shown();
    interrupt(&child);
    let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(20)).expect("v2 ended after Ctrl+Break");
    assert_interrupted(&out.status, SIGINT, &String::from_utf8_lossy(&out.stderr));
    assert!(hidden, "the cursor was hidden while hide.v2 ran (VT processing on)");
    assert!(console.cursor_shown(), "Ctrl+Break: the cursor is shown again");
    assert_eq!(console.mode(), mode, "Ctrl+Break: the console's mode is back");
    // exit(3) from a V2 callback a block called while it had the cursor hidden.
    if needs("py", "a_console_gets_its_cursor_and_mode_back_however_v2_ends") {
        let src = "func stop() { exit(3) }\n@py(stop) {\n    print(\"\\x1b[?25l\", end=\"\", flush=True)\n    stop()\n}\nprint(\"not reached\")\n";
        std::fs::write(home.0.join("exit.v2"), src).unwrap();
        let mut cmd = v2_command(&home.0);
        cmd.arg("exit.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(console.stdio()).stderr(Stdio::piped());
        let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(60)).unwrap();
        assert_eq!(out.status.code(), Some(3), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(console.cursor_shown(), "exit(3): the cursor is shown again");
        assert_eq!(console.mode(), mode, "exit(3): the console's mode is back");
    }
    // A closed pipe on stdout; stderr is the console.
    std::fs::write(home.0.join("loop.v2"), "for i in 0..100000000 {\n    print(i)\n}\n").unwrap();
    let (reader, writer) = std::io::pipe().expect("a pipe");
    drop(reader);
    let mut cmd = v2_command(&home.0);
    cmd.arg("loop.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(writer).stderr(console.stdio());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(30)).unwrap();
    assert_eq!(out.status.code(), Some(141));
    assert_eq!(console.mode(), mode, "a closed pipe: the console's mode is back");
}

#[cfg(windows)]
#[test]
fn a_v2_leaves_a_console_mode_it_didnt_set_alone() {
    // INTERNALS 6.5 (round 22, the second review, issue 4): two v2 runs on one console, as in
    // `v2 a.v2 | v2 b.v2`. A run that finds VT processing on already records nothing, so when the
    // first run, ending, turns it off, the second doesn't turn it on again at its end; and a run
    // puts back the mode it found only while the console still has the mode it set.
    let console = Console::take();
    let mode = console.mode();
    let home = TempHome::new();
    std::fs::write(home.0.join("wait.v2"), "write_file(\"ready\", \"1\")\nsleep(1)\n").unwrap();
    let run = |then: u32| {
        let _ = std::fs::remove_file(home.0.join("ready"));
        let mut cmd = v2_command(&home.0);
        cmd.arg("wait.v2").current_dir(&home.0).stdin(Stdio::null()).stdout(console.stdio()).stderr(Stdio::piped());
        let child = cmd.spawn().expect("start v2");
        let deadline = Instant::now() + Duration::from_secs(30);
        while !home.0.join("ready").exists() {
            assert!(Instant::now() < deadline, "wait.v2 never got ready");
            std::thread::sleep(Duration::from_millis(20));
        }
        // What another program on the console does while this v2 runs.
        console.set_mode(then);
        let out = wait_with_timeout(child, Vec::new(), Duration::from_secs(30)).unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        console.mode()
    };
    // VT on before v2 starts; the earlier run turns it off meanwhile.
    console.set_mode(mode | VT);
    assert_eq!(run(mode), mode, "a mode v2 didn't set is not put back");
    // VT off before v2 starts (v2 turns it on); meanwhile the console gets another mode.
    console.set_mode(mode);
    let other = mode | VT | windows_sys::Win32::System::Console::DISABLE_NEWLINE_AUTO_RETURN;
    assert_eq!(run(other), other, "a console whose mode changed after v2 set it keeps that mode");
}

#[cfg(unix)]
#[test]
fn input_without_echo_hides_what_is_typed() {
    // DOCS 3.11, INTERNALS 5.10 (round 22, L3): input(prompt, echo: false) on a terminal turns echo
    // off before the prompt and keeps line input; the line comes back as typed, and a line break
    // goes after it where the prompt went; the terminal's settings are back after Enter (the next
    // input() shows what is typed) and after Ctrl+C.
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
    let home = TempHome::new();
    std::fs::write(home.0.join("ask.v2"), "let pw = input(\"Password: \", echo: false)\nprint(f\"got [${pw}]\")\nprint(f\"name [${input(\"Name: \")}]\")\n").unwrap();
    std::fs::write(home.0.join("stop.v2"), "input(\"Password: \", echo: false)\nprint(\"not reached\")\n").unwrap();
    for (file, typed) in [("ask.v2", "s3cret\n"), ("stop.v2", "\x03")] {
        let mut pty = Pty::open(80, 24);
        let before = pty.flags();
        let mut cmd = v2_command(&home.0);
        cmd.arg(file).current_dir(&home.0);
        pty.attach(&mut cmd);
        let mut child = cmd.spawn().expect("start v2");
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        let stop = |pty: &mut Pty, child: &mut Child, why: &str, seen: &mut Vec<u8>| {
            pty.reap(child, seen);
            panic!("{file}: {why}: {}", String::from_utf8_lossy(seen).escape_debug());
        };
        if !pty.read_until(&mut seen, deadline, |s| find(s, b"Password: ").is_some()) {
            stop(&mut pty, &mut child, "no prompt", &mut seen);
        }
        let asking = pty.flags();
        if asking != [before[0], before[1], before[2], before[3] & !libc::ECHO] || asking[3] & libc::ICANON == 0 {
            stop(&mut pty, &mut child, &format!("echo isn't off alone while it waits ({before:?} became {asking:?})"), &mut seen);
        }
        pty.master.write_all(typed.as_bytes()).unwrap();
        let mut after = None;
        if file == "ask.v2" {
            if !pty.read_until(&mut seen, deadline, |s| find(s, b"Name: ").is_some()) {
                stop(&mut pty, &mut child, "no second prompt", &mut seen);
            }
            after = Some(pty.flags());
            pty.master.write_all(b"ada\n").unwrap();
        }
        let Some(status) = pty.wait(&mut child, &mut seen, deadline) else {
            panic!("{file}: v2 didn't end: {}", String::from_utf8_lossy(&seen).escape_debug());
        };
        // The last line, or after Ctrl+C the cursor shown again (6.5).
        let end: &[u8] = if file == "ask.v2" { b"name [ada]\r\n" } else { b"\x1b[?25h" };
        pty.read_until(&mut seen, Instant::now() + Duration::from_secs(2), |s| find(s, end).is_some());
        let shown = String::from_utf8_lossy(&seen).escape_debug().to_string();
        if file == "ask.v2" {
            assert!(status.success(), "{shown}");
            assert_eq!(after, Some(before), "the settings weren't back after Enter: {shown}");
            // Nothing typed shows but the second answer; v2's "\n" reaches the screen as "\r\n".
            assert_eq!(shown, "Password: \\r\\ngot [s3cret]\\r\\nName: ada\\r\\nname [ada]\\r\\n");
        } else {
            assert_interrupted(&status, SIGINT, &shown);
            assert!(find(&seen, b"not reached").is_none(), "{shown}");
        }
        assert_eq!(pty.flags(), before, "{file}: the terminal's settings weren't put back: {shown}");
    }
}

#[cfg(unix)]
#[test]
fn input_without_echo_gives_the_terminal_back_when_stdout_isnt_one() {
    // INTERNALS 5.10 and 6.5 (round 22, the review of L3): with stdin a terminal and stdout a file,
    // Ctrl+C at the password prompt puts the terminal's settings back, though v2's stdout isn't the
    // terminal (`TERMINAL_CHANGED`); with stdout a closed pipe the prompt can't be written: exit 141,
    // the settings put back first.
    let home = TempHome::new();
    std::fs::write(home.0.join("stop.v2"), "input(\"Password: \", echo: false)\nprint(\"not reached\")\n").unwrap();
    let out = home.0.join("out.txt");
    for case in ["stdout to a file", "stdout a closed pipe"] {
        let mut pty = Pty::open(80, 24);
        let before = pty.flags();
        let mut cmd = v2_command(&home.0);
        cmd.arg("stop.v2").current_dir(&home.0);
        pty.attach(&mut cmd);
        if case == "stdout to a file" {
            cmd.stdout(std::fs::File::create(&out).unwrap());
        } else {
            let (reader, writer) = std::io::pipe().unwrap();
            drop(reader);
            cmd.stdout(writer);
        }
        let mut child = cmd.spawn().expect("start v2");
        drop(cmd);
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut seen = Vec::new();
        let stop = |pty: &mut Pty, child: &mut Child, why: &str, seen: &mut Vec<u8>| {
            pty.reap(child, seen);
            panic!("{case}: {why}");
        };
        if case == "stdout to a file" {
            // The prompt is in the file and echo is off: v2 reads.
            while pty.flags()[3] & libc::ECHO != 0 || std::fs::read_to_string(&out).unwrap_or_default() != "Password: " {
                if Instant::now() > deadline {
                    stop(&mut pty, &mut child, "no prompt with echo off", &mut seen);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            pty.master.write_all(b"\x03").unwrap();
        }
        let Some(status) = pty.wait(&mut child, &mut seen, deadline) else {
            panic!("{case}: v2 didn't end");
        };
        if case == "stdout to a file" {
            assert_interrupted(&status, SIGINT, case);
        } else {
            assert_eq!(status.code(), Some(141), "{case}");
        }
        assert_eq!(pty.flags(), before, "{case}: the terminal's settings weren't put back");
    }
}

#[cfg(unix)]
#[test]
fn input_without_echo_and_job_control() {
    // DOCS 3.11, INTERNALS 5.10 and 6.5 (round 22, the review of L3): a Python stand-in for a shell
    // with job control runs v2 as a job. Ctrl+Z at the password prompt: echo is on while v2 is
    // stopped (dash leaves the terminal as it finds it), and off again once fg goes on with it (bash
    // puts its own settings back meanwhile), also after a bg in between. A prompt reached in the
    // background: v2 waits, stopped by SIGTTOU, before it reads the terminal's settings (it would
    // take the line editor's raw mode, and put it back at the end), and asks once in the foreground.
    if !needs("py", "input_without_echo_and_job_control") {
        return;
    }
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
    let home = TempHome::new();
    std::fs::write(home.0.join("ask.v2"), "let pw = input(\"Password: \", echo: false)\nprint(f\"got [${pw}]\")\nprint(f\"name [${input(\"Name: \")}]\")\n").unwrap();
    let shell = r#"import os, signal, sys, termios
case, argv = sys.argv[1], sys.argv[2:]
shell = termios.tcgetattr(0)
raw = termios.tcgetattr(0)
raw[3] &= ~(termios.ECHO | termios.ICANON)
signal.signal(signal.SIGTTOU, signal.SIG_IGN)  # a shell takes the terminal back without stopping


def say(text):
    os.write(1, (text + "\n").encode())


def flags():
    a = termios.tcgetattr(0)
    return "echo=%s icanon=%s" % (bool(a[3] & termios.ECHO), bool(a[3] & termios.ICANON))


def stopped():
    _, status = os.waitpid(pid, os.WUNTRACED)
    os.tcsetpgrp(0, os.getpgrp())
    if not os.WIFSTOPPED(status):
        say("not stopped: %s" % status)
        sys.exit(1)
    return signal.Signals(os.WSTOPSIG(status)).name


if case == "background":
    termios.tcsetattr(0, termios.TCSANOW, raw)  # the line editor's mode while a job runs in the background
pid = os.fork()
if pid == 0:
    os.setpgid(0, 0)
    if case != "background":
        os.tcsetpgrp(0, os.getpid())
    for s in (signal.SIGTTOU, signal.SIGTTIN, signal.SIGTSTP):
        signal.signal(s, signal.SIG_DFL)
    os.execv(argv[0], argv)
try:
    os.setpgid(pid, pid)
except OSError:
    pass
if case != "background":
    os.tcsetpgrp(0, pid)
say("stopped by %s, %s" % (stopped(), flags()))
if case == "ctrl_z then bg":
    termios.tcsetattr(0, termios.TCSANOW, raw)
    os.killpg(pid, signal.SIGCONT)  # bg: it goes on without the terminal
    stopped()
    say("stopped again")
termios.tcsetattr(0, termios.TCSANOW, shell)  # the settings bash gives a job it goes on with
say("fg")
os.tcsetpgrp(0, pid)
os.killpg(pid, signal.SIGCONT)
_, status = os.waitpid(pid, 0)
os.tcsetpgrp(0, os.getpgrp())
say("v2 ended %s, %s" % (os.waitstatus_to_exitcode(status), flags()))
"#;
    std::fs::write(home.0.join("shell.py"), shell).unwrap();
    let rest = "\r\ngot [s3cret]\r\nName: ada\r\nname [ada]\r\nv2 ended 0, echo=True icanon=True\r\n";
    let cases = [
        ("ctrl_z", format!("Password: stopped by SIGSTOP, echo=True icanon=True\r\nfg\r\n{rest}")),
        ("ctrl_z then bg", format!("Password: stopped by SIGSTOP, echo=True icanon=True\r\nstopped again\r\nfg\r\n{rest}")),
        ("background", format!("stopped by SIGTTOU, echo=False icanon=False\r\nfg\r\nPassword: {rest}")),
    ];
    for (case, want) in cases {
        let mut pty = Pty::open(80, 24);
        let before = pty.flags();
        let mut cmd = Command::new("python3");
        cmd.args(["shell.py", case, BIN, "ask.v2"]).current_dir(&home.0).env("HOME", &home.0).env_remove("VIRTUAL_ENV");
        pty.attach(&mut cmd);
        let mut child = cmd.spawn().expect("start python3");
        drop(cmd);
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        let stop = |pty: &mut Pty, child: &mut Child, why: &str, seen: &mut Vec<u8>| {
            pty.reap(child, seen);
            panic!("{case}: {why}: {}", String::from_utf8_lossy(seen).escape_debug());
        };
        if case != "background" {
            if !pty.read_until(&mut seen, deadline, |s| find(s, b"Password: ").is_some()) {
                stop(&mut pty, &mut child, "no prompt", &mut seen);
            }
            // Ctrl+Z, typed.
            pty.master.write_all(b"\x1a").unwrap();
        }
        if !pty.read_until(&mut seen, deadline, |s| find(s, b"fg\r\n").is_some()) {
            stop(&mut pty, &mut child, "v2 never stopped", &mut seen);
        }
        let fg = find(&seen, b"fg\r\n").unwrap() + 4;
        // After fg echo goes off again (Ctrl+Z), or off at last (the background).
        while pty.flags()[3] & libc::ECHO != 0 {
            if Instant::now() > deadline {
                stop(&mut pty, &mut child, "echo didn't go off after fg", &mut seen);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let asking = pty.flags();
        if asking != [before[0], before[1], before[2], before[3] & !libc::ECHO] {
            stop(&mut pty, &mut child, &format!("echo isn't off alone after fg ({before:?} became {asking:?})"), &mut seen);
        }
        if case == "background" && !pty.read_until(&mut seen, deadline, |s| find(&s[fg..], b"Password: ").is_some()) {
            stop(&mut pty, &mut child, "no prompt after fg", &mut seen);
        }
        pty.master.write_all(b"s3cret\n").unwrap();
        if !pty.read_until(&mut seen, deadline, |s| find(&s[fg..], b"Name: ").is_some()) {
            stop(&mut pty, &mut child, "no second prompt", &mut seen);
        }
        pty.master.write_all(b"ada\n").unwrap();
        let Some(status) = pty.wait(&mut child, &mut seen, deadline) else {
            panic!("{case}: the shell didn't end: {}", String::from_utf8_lossy(&seen).escape_debug());
        };
        pty.read_until(&mut seen, Instant::now() + Duration::from_secs(2), |s| s.ends_with(b"icanon=True\r\n"));
        let shown = String::from_utf8_lossy(&seen).escape_debug().to_string();
        assert!(status.success(), "{case}: {shown}");
        assert_eq!(shown, want.escape_debug().to_string(), "{case}");
        assert_eq!(pty.flags(), before, "{case}: the terminal's settings weren't put back: {shown}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn the_readmes_way_to_leave_a_job_running_returns_at_once() {
    // README (round 22, L4 and its review): its line for a job that outlives V2, run as written with
    // `job` a script that writes job.log 2 s later: exec returns at once (dash, Ubuntu's sh, held
    // exec's pipes while it waited for the job in the first line), and the job outlives V2 (under
    // load, `setsid -f` in a shell lost it: the shell ended, and exec's tree with it, before the
    // forked setsid had left that session; `setsid -w` waits until it has).
    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md")).unwrap();
    let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = flat.split_once("to leave a job running, `").and_then(|(_, rest)| rest.split_once('`')).expect("the README's line for a job that outlives V2").0;
    let path = std::env::var_os("PATH").unwrap_or_default();
    if !std::env::split_paths(&path).any(|d| d.join("setsid").is_file()) {
        let _ = writeln!(std::io::stderr(), "SKIP the_readmes_way_to_leave_a_job_running_returns_at_once (no setsid on PATH)");
        return;
    }
    let home = TempHome::new();
    let bin = home.0.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("job"), "#!/bin/sh\nsleep 2\necho alive\n").unwrap();
    let mut perms = std::fs::metadata(bin.join("job")).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(bin.join("job"), perms).unwrap();
    std::fs::write(home.0.join("main.v2"), format!("let t0 = time()\n{line}\nprint(time() - t0 < 1.5)\n")).unwrap();
    let mut cmd = v2_command(&home.0);
    let mut dirs = vec![bin];
    dirs.extend(std::env::split_paths(&path));
    cmd.arg("main.v2").current_dir(&home.0).env("PATH", std::env::join_paths(dirs).unwrap()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(30)).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stdout).as_ref()), (Some(0), "true\n"), "{line}: {stderr}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::fs::read_to_string(home.0.join("job.log")).unwrap_or_default() != "alive\n" {
        assert!(Instant::now() < deadline, "{line}: the job didn't outlive V2");
        std::thread::sleep(Duration::from_millis(50));
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
            .env("LC_ALL", "C.UTF-8")
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
fn names_that_arent_utf8_say_what_to_do() {
    // Round 20 (L12): FILE's name ends as a folder's does (19.1); a path that args() or getenv()
    // gave U+FFFD names no file, and the hint says why (5.10).
    use std::os::unix::ffi::OsStrExt;
    let home = TempHome::new();
    let run = |args: &[&std::ffi::OsStr], env: &[(&str, &std::ffi::OsStr)]| {
        let mut cmd = v2_command(&home.0);
        cmd.args(args).current_dir(&home.0).stdin(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    let latin = |b: &'static [u8]| std::ffi::OsStr::from_bytes(b);
    assert_eq!(
        run(&[latin(b"J\xfcrgen.v2")], &[]),
        (Some(2), "v2: the file name 'J\u{FFFD}rgen.v2' is not valid UTF-8, which V2 needs: the name is in another encoding; rename it\n".to_string())
    );
    std::fs::write(home.0.join("rf.v2"), "print(read_file(args().get(0) ?? getenv(\"DATA\")))\n").unwrap();
    let hint = |what: &str| format!("  hint: the path holds U+FFFD where {what} wasn't UTF-8: rename the file to a UTF-8 name, or use a UTF-8 locale\n");
    let (code, stderr) = run(&[latin(b"rf.v2"), latin(b"gr\xfc\xdfe.txt")], &[]);
    assert!(code == Some(1) && stderr.contains(&hint("an argument")), "{stderr}");
    let (code, stderr) = run(&[latin(b"rf.v2")], &[("DATA", latin(b"d\xfc.txt"))]);
    assert!(code == Some(1) && stderr.contains(&hint("an environment variable")), "{stderr}");
    // A U+FFFD written in the path itself gets no such hint.
    let (_, stderr) = run(&[latin(b"rf.v2"), latin("x\u{FFFD}.txt".as_bytes())], &[]);
    assert!(!stderr.contains("hint:"), "{stderr}");
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
    // On Windows with one separator, not the joined `…\missing/python.exe` (round 21, W7).
    assert!(stdout.contains(&format!("py: engine 'py' command '{}' not found (set by {})", real.join("missing").join("python.exe").display(), toml.display())), "{stdout}");
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

#[cfg(windows)]
#[test]
fn a_run_without_temp_variables_is_told_to_pass_them_on() {
    // Round 20, INTERNALS 20.3: a program that starts v2 with only PATH leaves out TMP, TEMP and
    // USERPROFILE, so Windows offers its own folder, where no block can run: doctor says so and
    // doesn't count blocks ready. USERPROFILE alone is a temp folder Windows uses.
    let home = TempHome::new();
    let doctor = |env: &[(&str, &Path)]| {
        let mut cmd = Command::new(BIN);
        cmd.arg("doctor").current_dir(&home.0).env_clear().env("PATH", std::env::var_os("PATH").unwrap_or_default());
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(120)).unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let note = "\n  temp: TEMP and TMP aren't set: a program that starts v2 with an environment of its own must pass them on (in Python, env=dict(os.environ, …))\n";
    let out = doctor(&[]);
    assert!(out.contains(note), "{out}");
    assert!(out.lines().any(|l| l.starts_with("not ready: ") && l.contains("blocks")), "{out}");
    let out = doctor(&[("USERPROFILE", &home.0)]);
    assert!(!out.contains("temp:"), "{out}");
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
         show(func() => write_file(\"{lower}:y.txt\", \"x\"))\nshow(func() => write_file(\"{q}:/\", \"x\"))\nprint(exists(\"{q}:/x\"))\n\
         show(func() => read_file(\"\\\\\\\\localhost\\\\v2-no-such-share-r19\\\\x.txt\"))\nshow(func() => make_dir(\"//localhost/v2-no-such-share-r19/out\"))\n\
         show(func() => write_file(\"//localhost/v2-no-such-share-r19/\", \"x\"))\n"
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
        // A root with its separator (the second review of round 23).
        ("write", format!("{q}:/")),
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
    let rest = [
        format!("cannot read '{share}\\x.txt': {cant}"),
        "null".into(),
        format!("cannot create '//localhost/v2-no-such-share-r19/out': {cant}"),
        "null".into(),
        format!("cannot write '//localhost/v2-no-such-share-r19/': {cant}"),
        "null".into(),
    ];
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

/// A V2 string literal of `s` as V2's hints write one: `\` doubled, `"` escaped, and WSL's
/// stand-ins (U+F000 to U+F0FF) as `\u{f03a}`.
#[cfg(windows)]
fn v2_literal(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{F000}'..='\u{F0FF}' => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(windows)]
#[test]
fn a_colon_is_refused_after_a_share_as_after_a_drive() {
    // INTERNALS 5.10 (round 21): after \\server\share\ a ':' wasn't refused, so write_file made an
    // empty 'log 10' with the data in a hidden stream, while //server/share/ was refused. Every
    // spelling is refused now, reads too; when WSL made a Linux name with ':' (mod\u{f03a}x.v2 to
    // Windows), the hint names it, and the call it shows reads it.
    let home = TempHome::new();
    std::fs::write(home.0.join("mod\u{f03a}x.v2"), "colon module").unwrap();
    let real = real_path(&home.0);
    let unc = format!(r"\\localhost\{}$\{}", &real[..1], &real[3..]);
    let share = real.as_bytes().get(1) == Some(&b':') && Path::new(&unc).is_dir();
    let mut paths = vec!["log 10:30.txt".to_string(), format!("{real}/sub:x.txt")];
    if share {
        paths.extend([format!(r"{unc}\log 10:30.txt"), format!("{unc}/log 10:30.txt"), format!("{}/log 10:30.txt", unc.replace('\\', "/"))]);
    } else {
        let _ = writeln!(std::io::stderr(), "SKIP the share part of a_colon_is_refused_after_a_share_as_after_a_drive (no \\\\localhost share)");
    }
    let mut src = SHOW.to_string();
    for p in &paths {
        src.push_str(&format!("show(func() => write_file({0}, \"important\"))\nshow(func() => read_file({0}))\n", v2_literal(p)));
    }
    let twins = if share { vec!["mod:x.v2".to_string(), format!(r"{unc}\mod:x.v2")] } else { vec!["mod:x.v2".to_string()] };
    for p in &twins {
        src.push_str(&format!("show(func() => read_file({}))\n", v2_literal(p)));
    }
    std::fs::write(home.0.join("c.v2"), src).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["c.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    let mut want = Vec::new();
    for p in &paths {
        for verb in ["write", "read"] {
            want.push(format!("cannot {verb} '{p}': a Windows file name can't have ':' in it"));
            want.push("use another character, as in 12-30 for a time".to_string());
        }
    }
    for p in &twins {
        want.push(format!("cannot read '{p}': a Windows file name can't have ':' in it"));
        let call = v2_literal(&p.replace("mod:x", "mod\u{f03a}x"));
        want.push(format!("WSL made one of that name, which Windows shows with \\u{{f03a}} for ':': reach it as read_file({call}), or rename it in WSL"));
    }
    assert_eq!(stdout.lines().collect::<Vec<_>>(), want, "{stdout}");
    // Nothing was written: no 'log 10' or 'sub' with a hidden stream.
    let mut names: Vec<String> = std::fs::read_dir(&home.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["c.v2", "mod\u{f03a}x.v2"]);
    // The call the hint shows reads the file.
    std::fs::write(home.0.join("r.v2"), "print(read_file(\"mod\\u{f03a}x.v2\"))\n").unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["r.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(0), "colon module\n"), "{stderr}");
}

#[cfg(windows)]
#[test]
fn names_wsl_made_with_stand_ins_are_offered_escaped() {
    // DOCS 3.11, INTERNALS 6.4, 19.1 (round 21): WSL shows a Linux q?.v2 to Windows as
    // q\u{f03f}.v2, whose stand-in printed as nothing: "did you mean './q.v2'?", which doesn't
    // work. An import or FILE of q?.v2 now gets the name escaped, with what its stand-in is, and
    // so does the listing of a folder's .v2 files; list_dir's names print escaped; an import by
    // the stand-in works.
    let note = "\\u{f03f} is how Windows shows the '?' of the Linux name 'q?.v2': rename it in WSL";
    let files = [
        ("q\u{f03f}.v2", "func who() { return \"q\" }\n"),
        ("imp.v2", "import \"./q?.v2\" as m\n"),
        ("ok.v2", "import \"./q\\u{f03f}.v2\" as m\nprint(m.who(), list_dir(\".\").filter(func(n) => n.starts_with(\"q\")))\n"),
        ("odd\u{f03a}dir/main.v2", "print(1)\n"),
    ];
    let (code, stdout, stderr, home) = run_with_files(&files, &["imp.v2"]);
    let message = "ImportError: cannot find module './q?.v2' (looked for q?.v2) — did you mean './q\\u{f03f}.v2'?";
    assert_eq!(code, Some(1), "{stdout}{stderr}");
    assert!(stderr.starts_with(&format!("imp.v2:1:1: {message}\n")) && stderr.contains(&format!("  hint: {note}\n")), "{stderr}");
    let (code, stdout, stderr) = run_in_home(&home.0, &["check", "imp.v2"], &[]);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stderr.starts_with(&format!("imp.v2:1:1: {message}\n")) && stderr.contains(&format!("  hint: {note}\n")), "{stderr}");
    let (code, stdout, stderr) = run_in_home(&home.0, &["ok.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(0), "q [\"q\\u{f03f}.v2\"]\n"), "{stderr}");
    let (code, _, stderr) = run_in_home(&home.0, &["q?.v2"], &[]);
    assert_eq!((code, stderr.as_str()), (Some(2), format!("v2: cannot find 'q?.v2'\ndid you mean 'q\\u{{f03f}}.v2'? {note}\n").as_str()));
    let (code, _, stderr) = run_in_home(&home.0, &["zzzzzz.v2"], &[]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.ends_with(&format!("\nthe .v2 files there: imp.v2, ok.v2, q\\u{{f03f}}.v2; {note}\n")), "{stderr}");
    // The review of round 21: the did-you-mean of a folder on the way that isn't there too.
    let (code, _, stderr) = run_in_home(&home.0, &[r"odd-dir\main.v2"], &[]);
    assert_eq!(code, Some(2), "{stderr}");
    let colon = "\\u{f03a} is how Windows shows the ':' of the Linux name 'odd:dir': rename it in WSL";
    assert!(stderr.starts_with("v2: cannot find 'odd-dir\\main.v2'\nthere is no folder "), "{stderr}");
    assert!(stderr.ends_with(&format!("odd-dir: did you mean 'odd\\u{{f03a}}dir'? {colon}\n")), "{stderr}");
}

#[cfg(windows)]
#[test]
fn a_script_run_by_its_verbatim_path_reaches_the_files_beside_it() {
    // The review of round 21 (DOCS 3.11, INTERNALS 2.1): a script in a folder whose name ends in a
    // dot, run by its \\?\ path as the refusal of `v2 dots./sd.v2` says, has a \\?\ script_dir(),
    // where Windows reads no / and no ..: read_file(f"${script_dir()}/data.csv") said "The
    // filename, directory name, or volume label syntax is incorrect". The file builtins and exec
    // read a \\?\ path's / as \ and take out its . and .. parts.
    let home = TempHome::new();
    let v = |name: &str| format!(r"\\?\{}\{name}", home.0.display());
    std::fs::create_dir(v("dots.")).unwrap();
    std::fs::write(v(r"dots.\data.csv"), "a,b").unwrap();
    std::fs::write(home.0.join("top.txt"), "top").unwrap();
    let exe = v2_literal(&format!(r"\\?\{}/v2.exe", Path::new(BIN).parent().unwrap().display()));
    let src = format!(
        "let d = script_dir()\nprint(d.starts_with(\"\\\\\\\\?\\\\\"), read_file(f\"${{d}}/data.csv\"), exists(f\"${{d}}/data.csv\"))\n\
         print(read_file(f\"${{d}}/./../top.txt\"), exists(f\"${{d}}/../top.txt\"))\nwrite_file(f\"${{d}}/out.txt\", \"w\")\nmake_dir(f\"${{d}}/sub/deeper\")\n\
         print(list_dir(f\"${{d}}/\"), list_dir(f\"${{d}}/sub\"))\nremove_file(f\"${{d}}/out.txt\")\nprint(exists(f\"${{d}}/out.txt\"))\n\
         let r = exec([{exe}, \"--version\"])\nprint(r.code, r.stdout.trim())\n\
         let h = exec([{exe}, f\"${{d}}/hello.v2\"])\nprint(h.code, h.stdout.trim())\n"
    );
    // The second review: an argument that is such a path too (python couldn't open
    // f"${script_dir()}/x.py"; builtins' unit test has cmd's command line, which stays as written).
    std::fs::write(v(r"dots.\hello.v2"), "print(\"hello from dots.\")\n").unwrap();
    std::fs::write(v(r"dots.\sd.v2"), src).unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &[r"dots.\sd.v2"], &[]);
    let start = r"one of that name is there (WSL and Macs can make one): run it by its full path with \\?\ in front: v2 ";
    let path = stderr.lines().nth(1).and_then(|l| l.strip_prefix(start)).map(|p| p.trim_matches('"').to_string()).unwrap_or_else(|| panic!("{stderr}"));
    assert_eq!(code, Some(2), "{stderr}");
    let (code, stdout, stderr) = run_in_home(&home.0, &[&path], &[]);
    let version = format!("0 v2 {} (VEP/1)", env!("CARGO_PKG_VERSION"));
    let want = ["true a,b true", "top true", "[\"data.csv\", \"hello.v2\", \"out.txt\", \"sd.v2\", \"sub\"] [\"deeper\"]", "false", &version, "0 hello from dots."];
    assert_eq!(stdout.lines().collect::<Vec<_>>(), want, "{stderr}");
    assert_eq!(code, Some(0), "{stderr}");
    std::fs::remove_dir_all(v("dots.")).unwrap();
}

#[cfg(windows)]
#[test]
fn a_script_or_import_whose_name_ends_in_a_dot_is_refused() {
    // INTERNALS 6.4, 19.1 (round 21): Windows drops a dot or a space at the end of a name, so
    // `v2 run.v2.` ran run.v2 and import "./dotm.v2." loaded dotm.v2, without a word. Both are
    // refused, as the file builtins refuse such a name; when a file of that name is there (WSL
    // and Macs make them), the run, check or import the message shows reaches it.
    let home = TempHome::new();
    let v = |name: &str| format!(r"\\?\{}\{name}", home.0.display());
    for (name, text) in [
        ("run.v2", "print(\"I am run.v2\")\n"),
        ("run.v2.", "print(\"I am run.v2.\")\n"),
        ("dotm.v2", "func who() { return \"plain\" }\n"),
        ("dotm.v2.", "func who() { return \"dotted\" }\n"),
        ("plain.v2", "func who() { return \"plain.v2\" }\n"),
    ] {
        std::fs::write(v(name), text).unwrap();
    }
    std::fs::create_dir(v("lib.")).unwrap();
    std::fs::write(v(r"lib.\m.v2"), "import \"./sib.v2\" as s\nimport \"../plain.v2\" as p\nfunc who() { return s.v() + \" \" + p.who() }\n").unwrap();
    std::fs::write(v(r"lib.\sib.v2"), "func v() { return \"sib\" }\n").unwrap();
    let why = |name: &str, kept: &str| format!("a Windows file name can't end in a dot or a space ('{name}' would be '{kept}')");
    // FILE: the \\?\ path of the file that is there, which runs and checks it.
    let shown_path = |line: &str, start: &str| -> String {
        let rest = line.strip_prefix(start).unwrap_or_else(|| panic!("{line}"));
        let path = rest.trim_matches('"').to_string();
        assert!(path.starts_with(r"\\?\") && path.ends_with(r"\run.v2."), "{path}");
        path
    };
    let (code, _, stderr) = run_in_home(&home.0, &["run.v2."], &[]);
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!((code, lines.len(), lines[0]), (Some(2), 2, format!("v2: cannot read 'run.v2.': {}", why("run.v2.", "run.v2")).as_str()), "{stderr}");
    let path = shown_path(lines[1], r"one of that name is there (WSL and Macs can make one): run it by its full path with \\?\ in front: v2 ");
    let (code, stdout, stderr) = run_in_home(&home.0, &[&path], &[]);
    assert_eq!((code, stdout.as_str()), (Some(0), "I am run.v2.\n"), "{stderr}");
    let (code, _, stderr) = run_in_home(&home.0, &["check", "run.v2."], &[]);
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!((code, lines.len()), (Some(2), 2), "{stderr}");
    let path = shown_path(lines[1], r"one of that name is there (WSL and Macs can make one): check it by its full path with \\?\ in front: v2 check ");
    let (code, stdout, stderr) = run_in_home(&home.0, &["check", &path], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.ends_with("run.v2.: 1 file, 0 errors\n"), "{stdout}");
    // Only Windows' reading of it there: that file; neither: the folder's .v2 files.
    let (code, _, stderr) = run_in_home(&home.0, &["plain.v2 "], &[]);
    assert_eq!((code, stderr.as_str()), (Some(2), format!("v2: cannot read 'plain.v2 ': {}\ndid you mean 'plain.v2'?\n", why("plain.v2 ", "plain.v2")).as_str()));
    let (code, _, stderr) = run_in_home(&home.0, &["gone.v2."], &[]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.ends_with("\nthe .v2 files there: dotm.v2, plain.v2, run.v2\n"), "{stderr}");
    // Imports: a module's own name that ends in a dot is no .v2 file; a folder's is reached by
    // the \\?\ path the hint shows, its own imports too.
    let src = format!(
        "{SHOW}show(func() {{\n    import \"./dotm.v2.\" as m\n    return m.who()\n}})\nshow(func() {{\n    import \"./plain.v2.\" as m\n    return m.who()\n}})\n\
         show(func() {{\n    import \"./lib./m.v2\" as m\n    return m.who()\n}})\n"
    );
    std::fs::write(home.0.join("imp.v2"), src).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["imp.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    let want = [
        format!("cannot import './dotm.v2.': {}", why("dotm.v2.", "dotm.v2")),
        "one of that name is there (WSL and Macs can make one), but a module's name ends in .v2: rename it".into(),
        format!("cannot import './plain.v2.': {} — did you mean './plain.v2'?", why("plain.v2.", "plain.v2")),
        "null".into(),
        format!("cannot import './lib./m.v2': {}", why("lib.", "lib")),
    ];
    assert_eq!(lines[..want.len().min(lines.len())], want, "{stdout}");
    let start = r"one of that name is there (WSL and Macs can make one): reach it by its full path with \\?\ in front, as in import ";
    let import = lines[want.len()].strip_prefix(start).and_then(|r| r.strip_suffix(" as m")).unwrap_or_else(|| panic!("{stdout}"));
    std::fs::write(home.0.join("lib.v2"), format!("import {import} as m\nprint(m.who())\n")).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["lib.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(0), "sib plain.v2\n"), "{stderr}");
    for name in ["run.v2.", "dotm.v2.", r"lib.\m.v2", r"lib.\sib.v2"] {
        std::fs::remove_file(v(name)).unwrap();
    }
    std::fs::remove_dir(v("lib.")).unwrap();
}

#[cfg(windows)]
#[test]
fn wsl_and_linux_paths_get_the_windows_way() {
    // DOCS 3.11, INTERNALS 5.10, 19.1 (round 21): /mnt/c/… pasted from WSL said only "there is no
    // folder C:\mnt", and make_dir would have made it at the top of the drive; /root/… and /home/…
    // said nothing of WSL; exec(["ls"]) only "not found on PATH". Runs only where the current
    // drive has none of those folders, and removes what a broken build makes.
    let home = TempHome::new();
    let real = real_path(&home.0);
    let drive = real[..2].to_string();
    let tops = ["mnt", "root", "home", "usr"].map(|t| PathBuf::from(format!(r"{drive}\{t}")));
    if real.as_bytes().get(1) != Some(&b':') || tops.iter().any(|t| t.exists()) {
        let _ = writeln!(std::io::stderr(), "SKIP wsl_and_linux_paths_get_the_windows_way ({drive} has \\mnt, \\root, \\home or \\usr)");
        return;
    }
    let _left = Leftovers::new(&tops);
    let (letter, upper) = (real[..1].to_ascii_lowercase(), real[..1].to_ascii_uppercase());
    let rest = real[3..].replace('\\', "/");
    let mnt = format!("/mnt/{letter}/{rest}");
    std::fs::write(home.0.join("data.txt"), "here").unwrap();
    std::fs::write(home.0.join("x.v2"), "print(\"x ran\")\n").unwrap();
    let q = |s: &str| serde_json::to_string(s).unwrap();
    // make_dir only once the hint shows the path is read as WSL's, or a probe that can make
    // nothing was refused (below), so a broken build makes nothing.
    let src = format!(
        "func hint(f) {{\n    try {{\n        f()\n        print(\"no error\")\n    }} catch (e) {{\n        print(e.hint)\n    }}\n}}\n\
         hint(func() => read_file({md}))\nhint(func() => exec([{mp}]))\nhint(func() => read_file(\"/root/proj/main.v2\"))\n\
         hint(func() => write_file(\"/home/me/x.txt\", \"x\"))\nhint(func() => exec([\"/usr/bin/python3\", \"x.py\"]))\nhint(func() => exec([\"ls\", \"-la\"]))\n\
         let h = null\ntry {{\n    read_file({mn})\n}} catch (e) {{\n    h = e.hint\n}}\nif h != null && \"WSL's\" in h {{\n    hint(func() => make_dir({mn}))\n}}\n\
         let m = null\ntry {{\n    make_dir(\"/root/v2\\u{{0}}probe\")\n}} catch (e) {{\n    m = e.message\n}}\nprint(m)\n\
         if m != null && \"top of the drive\" in m {{\n    hint(func() => make_dir(\"/root/proj/out\"))\n}}\n",
        md = q(&format!("{mnt}/data.txt")),
        mp = q(&format!("{mnt}/prog.exe")),
        mn = q(&format!("{mnt}/new/dir")),
    );
    std::fs::write(home.0.join("w.v2"), src).unwrap();
    // A PATH without Git's Linux tools.
    let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into())).join("System32");
    let (code, stdout, stderr) = run_in_home(&home.0, &["w.v2"], &[("PATH", system.as_os_str())]);
    assert_eq!(code, Some(0), "{stderr}");
    let spelling = |end: &str| format!("on Windows, /mnt/{letter}/… is WSL's spelling of {upper}:/…: write {}", q(&format!("{upper}:/{rest}/{end}")));
    let linux = r"a Linux path: from Windows, WSL's files are \\wsl.localhost\<distro>\…";
    let want = [
        spelling("data.txt"),
        spelling("prog.exe"),
        linux.into(),
        linux.into(),
        linux.into(),
        r#"run Linux programs with exec(["wsl.exe", "-e", "ls", "-la"])"#.into(),
        spelling("new/dir"),
        // The review of round 21: make_dir would make \root at the top of the drive. A NUL in the
        // path stops create_dir_all before it makes anything, so the real make_dir runs only once
        // this one was refused.
        r"cannot create '/root/v2\u{0}probe': on Windows that makes \root at the top of the drive, a folder other users can write to".into(),
        linux.into(),
    ];
    assert_eq!(stdout.lines().collect::<Vec<_>>(), want, "{stdout}");
    assert!(tops.iter().all(|t| !t.exists()), "nothing at the top of the drive");
    // FILE: the run it says works.
    let file = format!("{mnt}/x.v2");
    let (code, _, stderr) = run_in_home(&home.0, &[&file], &[]);
    let start = format!("v2: cannot find '{file}'\non Windows, /mnt/{letter}/… is WSL's spelling of {upper}:/…: run v2 ");
    assert_eq!(code, Some(2), "{stderr}");
    let run = stderr.strip_prefix(&start).map(|r| r.trim_end().trim_matches('"').to_string()).unwrap_or_else(|| panic!("{stderr}"));
    let (code, stdout, stderr) = run_in_home(&home.0, &[&run], &[]);
    assert_eq!((code, stdout.as_str()), (Some(0), "x ran\n"), "{stderr}");
    let (code, _, stderr) = run_in_home(&home.0, &["check", &file], &[]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("is WSL's spelling of") && stderr.contains(": run v2 check "), "{stderr}");
    let (code, _, stderr) = run_in_home(&home.0, &["/root/proj/main.v2"], &[]);
    assert_eq!((code, stderr.as_str()), (Some(2), format!("v2: cannot find '/root/proj/main.v2'\n{linux}\n").as_str()));
}

#[cfg(windows)]
#[test]
fn names_on_wsls_share_are_reached_as_read_file_reaches_them() {
    // Round 21 (DOCS 3.10, 3.11; INTERNALS 5.10, 6.4, 19.1) on WSL's share, which Windows CI
    // doesn't have: set V2_TEST_WSL_DISTRO to a distro's name (and V2_TEST_WSL_DIR to a folder
    // of it for the test's own folder, /tmp when unset). A Linux mod:x.v2 is mod\u{f03a}x.v2 to
    // Windows, whose name canonicalize couldn't normalize, so imports and FILE failed where
    // read_file read it; \\wsl.localhost\…\a:b was read and written, //wsl.localhost/…/a:b
    // refused; `v2 run.v2.` ran run.v2.
    let Some(distro) = std::env::var("V2_TEST_WSL_DISTRO").ok().filter(|d| !d.is_empty()) else {
        let _ = writeln!(std::io::stderr(), "SKIP names_on_wsls_share_are_reached_as_read_file_reaches_them (V2_TEST_WSL_DISTRO isn't set)");
        return;
    };
    let dir = format!("{}/v2-test-{}", std::env::var("V2_TEST_WSL_DIR").unwrap_or_else(|_| "/tmp".into()).trim_end_matches('/'), std::process::id());
    // A Linux path (Git Bash would turn /root into C:/Program Files/Git/root), from / in WSL.
    assert!(dir.starts_with('/') && !dir.contains(['\'', ':']), "V2_TEST_WSL_DIR must be a Linux folder: {dir}");
    let in_wsl = |script: &str| Command::new("wsl.exe").args(["-d", &distro, "--cd", "/", "-e", "sh", "-c", script]).stdout(Stdio::null()).status().unwrap().success();
    assert!(in_wsl(&format!("mkdir -p '{dir}/case' '{dir}/odd:dir'")), "mkdir in {distro}");
    // Removed in WSL at the end, after a failed assertion too.
    struct Gone<'a>(&'a dyn Fn(&str) -> bool, String);
    impl Drop for Gone<'_> {
        fn drop(&mut self) {
            (self.0)(&format!("rm -rf '{}'", self.1));
        }
    }
    let _gone = Gone(&in_wsl, dir.clone());
    let share = PathBuf::from(format!(r"\\wsl.localhost\{distro}{}", dir.replace('/', "\\")));
    let pua = |s: &str| s.replace(':', "\u{f03a}");
    for (name, text) in [
        ("case/mod:x.v2", "func who() { return \"colon\" }\n"),
        ("odd:dir/main.v2", "import \"./m.v2\" as m\nprint(m.v())\n"),
        ("odd:dir/m.v2", "func v() { return 7 }\n"),
        ("imp.v2", "import \"./case/mod\\u{f03a}x.v2\" as a\nimport \"./case/mod:x.v2\" as b\nprint(a.who(), b.who())\n"),
        ("run.v2", "print(\"I am run.v2\")\n"),
    ] {
        std::fs::write(share.join(pua(name)), text).unwrap();
    }
    std::fs::write(format!(r"\\?\UNC{}\run.v2.", &share.to_string_lossy()[1..]), "print(\"I am run.v2.\")\n").unwrap();
    assert!(in_wsl(&format!("test -f '{dir}/case/mod:x.v2' && test -f '{dir}/run.v2.'")), "the Linux names");
    let run = |args: &[&str]| {
        let tmp = TempHome::new();
        let mut cmd = v2_command(&tmp.0);
        cmd.args(args).current_dir(&share).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(120)).unwrap();
        (out.status.code(), normalize(&String::from_utf8_lossy(&out.stdout)), normalize(&String::from_utf8_lossy(&out.stderr)))
    };
    // Imports and FILE by the stand-in, and by ':', as read_file reads them.
    assert_eq!(run(&["imp.v2"]), (Some(0), "colon colon\n".into(), String::new()));
    for main in [pua("odd:dir\\main.v2"), "odd:dir\\main.v2".to_string()] {
        assert_eq!(run(&[&main]), (Some(0), "7\n".into(), String::new()), "{main}");
        let (code, stdout, stderr) = run(&["check", &main]);
        assert_eq!(code, Some(0), "{stderr}");
        assert!(stdout.ends_with(": 2 files, 0 errors\n"), "{stdout}");
    }
    // A ':' after the share is refused with either slash, and the hint's call reads the file.
    let at = share.to_string_lossy().into_owned();
    let src = format!(
        "{SHOW}show(func() => read_file({0}))\nshow(func() => read_file({1}))\nshow(func() => write_file({2}, \"x\"))\n",
        v2_literal(&format!(r"{at}\case\mod:x.v2")),
        v2_literal(&format!("{}/case/mod:x.v2", at.replace('\\', "/"))),
        v2_literal(&format!(r"{at}\new:x.txt")),
    );
    std::fs::write(share.join("rw.v2"), src).unwrap();
    let (code, stdout, stderr) = run(&["rw.v2"]);
    assert_eq!(code, Some(0), "{stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 6, "{stdout}");
    let hint = |path: &str| format!("WSL made one of that name, which Windows shows with \\u{{f03a}} for ':': reach it as read_file({}), or rename it in WSL", v2_literal(&pua(path)));
    assert_eq!(lines[1], hint(&format!(r"{at}\case\mod:x.v2")), "{stdout}");
    assert_eq!(lines[3], hint(&format!("{}/case/mod:x.v2", at.replace('\\', "/"))), "{stdout}");
    assert_eq!(lines[5], "use another character, as in 12-30 for a time", "{stdout}");
    assert!(!in_wsl(&format!("test -e '{dir}/new:x.txt'")), "nothing written");
    std::fs::write(share.join("rd.v2"), format!("print(read_file({}))\n", v2_literal(&pua(&format!(r"{at}\case\mod:x.v2"))))).unwrap();
    assert_eq!(run(&["rd.v2"]), (Some(0), "func who() { return \"colon\" }\n\n".into(), String::new()));
    // `v2 run.v2.`: refused, and the run it shows runs run.v2.
    let (code, _, stderr) = run(&["run.v2."]);
    let start = r"one of that name is there (WSL and Macs can make one): run it by its full path with \\?\ in front: v2 ";
    let path = stderr.lines().nth(1).and_then(|l| l.strip_prefix(start)).map(|p| p.trim_matches('"').to_string()).unwrap_or_else(|| panic!("{stderr}"));
    assert_eq!(code, Some(2), "{stderr}");
    assert!(path.starts_with(r"\\?\UNC\wsl.localhost\"), "{path}");
    assert_eq!(run(&[&path]), (Some(0), "I am run.v2.\n".into(), String::new()));
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
        // Round 21 (L1): no block can run, so the row is broken and the engine isn't ready (ts
        // runs on js), and DOCS 15's `grep -q '^ready:.* py'` fails as the scripts would.
        let status = |told: bool| if told { "broken" } else { "ok" };
        let js_told = js_note.is_some();
        assert_eq!((&*rows["py"][0], &*rows["js"][0], &*rows["ts"][0]), (status(py_told), status(js_told), status(js_told)), "{stdout}");
        let ready: Vec<&str> = [("py", py_told), ("js", js_told), ("ts", js_told)].iter().filter(|(_, told)| !told).map(|(id, _)| *id).collect();
        assert!(stdout.contains(&format!("\nready: {}\n", if ready.is_empty() { "none".to_string() } else { ready.join(", ") })), "{stdout}");
        assert_eq!(stdout.contains("\n  ts: @ts blocks run on the js engine: see js above\n"), js_told, "{stdout}");
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

#[cfg(windows)]
#[test]
fn a_command_through_wsl_exe_is_told_it_runs_a_linux_python() {
    // Round 21 (W7), 7.6, 8.4, 19.2: wsl.exe runs a Linux python, which answers --version but
    // can't open the worker's adapter, a Windows path (exit code 2): doctor's note, and the
    // run's. This wsl answers --version as Python, then exits as that python does.
    let home = TempHome::new();
    let bin = home.0.join("wsl-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let wsl = bin.join("wsl.cmd");
    std::fs::write(&wsl, "@echo off\r\nfor %%a in (%*) do if \"%%~a\"==\"--version\" (echo Python 3.12.3& exit /b 0)\r\necho python3: can't open file 1>&2\r\nexit /b 2\r\n").unwrap();
    std::fs::write(home.0.join("v2.toml"), format!("[engines.py]\ncommand = ['{}', '-e', 'python3']\n", wsl.display())).unwrap();
    std::fs::write(home.0.join("s.v2"), "@py { 1 }\n").unwrap();
    let note = "wsl.exe runs a Linux python, which the Windows v2 can't use, as its adapter is a Windows path: name a python.exe, or run the Linux v2 inside WSL";
    let (code, stdout, stderr) = run_in_home(&home.0, &["doctor"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(doctor_rows(&stdout)["py"][0], "ok", "{stdout}");
    assert!(stdout.contains(&format!("\n  py: {note}\n")), "{stdout}");
    let (code, _, stderr) = run_in_home(&home.0, &["s.v2"], &[]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains(&format!("EngineCrashed: engine 'py' exited during startup (exit code 2); see its error output above, if any; {note}\n")), "{stderr}");
}

#[cfg(windows)]
#[test]
fn exec_refuses_cmd_in_a_share_folder() {
    // Round 21 (W1), 7.5: cmd.exe can't use a \\server\share working folder (a project in WSL):
    // it says so on its stderr, works in the Windows folder instead and exits 0. So exec refuses
    // it, and a batch file it would run, before starting either; other programs run there.
    // Skipped where the \\localhost share isn't reachable.
    let home = TempHome::new();
    let real = real_path(&home.0);
    let unc = format!(r"\\localhost\{}$\{}", &real[..1], &real[3..]);
    if real.as_bytes().get(1) != Some(&b':') || !Path::new(&unc).is_dir() {
        let _ = writeln!(std::io::stderr(), "SKIP exec_refuses_cmd_in_a_share_folder (no \\\\localhost share)");
        return;
    }
    std::fs::write(home.0.join("build.bat"), "@echo off\r\necho building in %CD%\r\n").unwrap();
    // The command line to write, as a V2 string: pushd "<folder>" && …
    let pushd = |then: &str| format!("\"pushd \\\"{}\\\" && {then}\"", unc.replace('\\', r"\\"));
    // What the hint says, followed: build.bat runs in the folder (pushd maps a drive to the share).
    let script = format!(
        "func show(f) {{\n    try {{ f() }} catch (e) {{ print(e.type + \": \" + e.message) }}\n}}\n\
         show(func() => exec([\"cmd\", \"/c\", \"cd\"]))\nshow(func() => exec([\"./build.bat\"]))\nprint(exec([\"hostname\"]).code)\n\
         let r = exec([\"cmd\", \"/c\", {}])\nprint(r.code, r.stdout.trim().ends_with(\"{}\"))\n",
        pushd(r".\\build.bat"),
        real[2..].replace('\\', r"\\")
    );
    std::fs::write(home.0.join("w.v2"), script).unwrap();
    let mut cmd = v2_command(&home.0);
    cmd.arg("w.v2").current_dir(&unc).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(120)).unwrap();
    let (stdout, stderr) = (normalize(&String::from_utf8_lossy(&out.stdout)), normalize(&String::from_utf8_lossy(&out.stderr)));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let rest = format!("can't work in a \\\\server\\share folder and would run in {windows}: run the program itself, or exec([\"cmd\", \"/c\", {}])", pushd("…"));
    assert_eq!(stdout, format!("IOError: exec: cmd.exe {rest}\nIOError: exec: build.bat runs in cmd.exe, which {rest}\n0\n0 true\n"), "{stderr}");
}

#[cfg(windows)]
#[test]
fn an_engine_through_a_batch_file_is_refused_in_a_share_folder() {
    // The review of round 21, 7.6, 19.2: a configured .bat (pyenv-win's shim, conda.bat) runs in
    // cmd.exe, which from a \\server\share working folder would start the worker in the Windows
    // folder (os.getcwd() was C:\Windows): EngineNotFound, and doctor's row broken, out of
    // ready:. From the drive folder the same command is ok. Skipped where the \\localhost share
    // isn't reachable.
    let home = TempHome::new();
    let real = real_path(&home.0);
    let unc = format!(r"\\localhost\{}$\{}", &real[..1], &real[3..]);
    if real.as_bytes().get(1) != Some(&b':') || !Path::new(&unc).is_dir() {
        let _ = writeln!(std::io::stderr(), "SKIP an_engine_through_a_batch_file_is_refused_in_a_share_folder (no \\\\localhost share)");
        return;
    }
    let shim = home.0.join("pyshim.bat");
    std::fs::write(&shim, "@echo off\r\nfor %%a in (%*) do if \"%%~a\"==\"--version\" (echo Python 3.12.1& exit /b 0)\r\nexit /b 1\r\n").unwrap();
    std::fs::write(home.0.join("v2.toml"), format!("[engines.py]\ncommand = ['{}']\n", shim.display())).unwrap();
    std::fs::write(home.0.join("s.v2"), "@py { 1 }\n").unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["doctor"], &[]);
    assert_eq!((code, &*doctor_rows(&stdout)["py"][0]), (Some(0), "ok"), "{stdout}{stderr}");
    let run = |args: &[&str]| {
        let mut cmd = v2_command(&home.0);
        cmd.args(args).current_dir(&unc).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(120)).unwrap();
        (out.status.code(), normalize(&String::from_utf8_lossy(&out.stdout)), normalize(&String::from_utf8_lossy(&out.stderr)))
    };
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let head = format!("engine 'py' command '{}' (set by ", shim.display());
    let tail = format!("v2.toml) runs in cmd.exe, which can't work in a \\\\server\\share folder and would run in {windows}: name the program itself by its full path, as in V2_ENGINE_PY=C:\\…\\python.exe (for pyenv-win, pyenv which python shows it), or run v2 from a folder on a drive\n");
    let (code, stdout, stderr) = run(&["doctor"]);
    assert_eq!((code, &*doctor_rows(&stdout)["py"][0]), (Some(0), "broken"), "{stdout}{stderr}");
    let note = stdout.lines().find(|l| l.starts_with("  py: ")).unwrap_or_default();
    assert!(note.starts_with(&format!("  py: {head}")) && format!("{note}\n").ends_with(&tail), "{stdout}");
    assert!(stdout.lines().any(|l| l.starts_with("ready: ") && !l.contains("py")), "{stdout}");
    let (code, _, stderr) = run(&["s.v2"]);
    assert_eq!(code, Some(3), "{stderr}");
    assert!(stderr.contains(&format!("EngineNotFound: {head}")) && stderr.contains(&tail), "{stderr}");
}

#[cfg(windows)]
#[test]
fn a_batch_file_on_path_is_passed_over_in_a_share_folder() {
    // The review of round 21, 7.6, 19.2: from a \\server\share working folder, pyenv-win's
    // shims\python.bat on PATH is skipped like a Store placeholder and the search goes on; with
    // nothing else py doesn't run, and doctor says what to do instead of repairing Python. From
    // the drive folder the shim is used. Skipped where the \\localhost share isn't reachable.
    let home = TempHome::new();
    let real = real_path(&home.0);
    let unc = format!(r"\\localhost\{}$\{}", &real[..1], &real[3..]);
    if real.as_bytes().get(1) != Some(&b':') || !Path::new(&unc).is_dir() {
        let _ = writeln!(std::io::stderr(), "SKIP a_batch_file_on_path_is_passed_over_in_a_share_folder (no \\\\localhost share)");
        return;
    }
    let shims = home.0.join("shims");
    std::fs::create_dir_all(&shims).unwrap();
    let shim = shims.join("python.bat");
    std::fs::write(&shim, "@echo off\r\nfor %%a in (%*) do if \"%%~a\"==\"--version\" (echo Python 3.12.1& exit /b 0)\r\nexit /b 1\r\n").unwrap();
    std::fs::write(home.0.join("s.v2"), "@py { 1 }\n").unwrap();
    let run = |cwd: &str, path: &std::ffi::OsStr, args: &[&str]| {
        let mut cmd = v2_command(&home.0);
        cmd.args(args).current_dir(cwd).env("PATH", path).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(120)).unwrap();
        (out.status.code(), normalize(&String::from_utf8_lossy(&out.stdout)), normalize(&String::from_utf8_lossy(&out.stderr)))
    };
    let skipped = format!("skipped {} (runs in cmd.exe, which can't work in a \\\\server\\share folder)", shim.display());
    let fix = r"fix: name the program itself by its full path, as in V2_ENGINE_PY=C:\…\python.exe (for pyenv-win, pyenv which python shows it), or run v2 from a folder on a drive";
    // Only the shim on PATH: doctor's row is broken, with the shim, and the fix isn't a repair.
    let (code, stdout, stderr) = run(&unc, shims.as_os_str(), &["doctor"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(doctor_rows(&stdout)["py"][..2], ["broken".to_string(), shim.display().to_string()], "{stdout}");
    assert!(stdout.contains(&format!("\n  py: engine 'py' (Python) was found but doesn't run (see why below)\n  py: {skipped}\n  py: {fix}\n")), "{stdout}");
    assert!(!stdout.contains("repair that Python"), "{stdout}");
    let (code, _, stderr) = run(&unc, shims.as_os_str(), &["s.v2"]);
    assert_eq!(code, Some(3), "{stderr}");
    assert!(stderr.contains("EngineNotFound: engine 'py' (Python) was found but doesn't run (see why below)\n"), "{stderr}");
    // The second review of round 21: no install line, as Python is installed.
    assert!(stderr.contains(&format!("\n    skipped: {} (runs in cmd.exe, which can't work in a \\\\server\\share folder)\n    {fix}\n", shim.display())), "{stderr}");
    assert!(!stderr.contains("install: ") && !stderr.contains("or set [engines.py]"), "{stderr}");
    // The drive folder: the shim, which answers --version, is used.
    let (_, stdout, stderr) = run(&real, shims.as_os_str(), &["doctor"]);
    assert_eq!(doctor_rows(&stdout)["py"][..2], ["ok".to_string(), shim.display().to_string()], "{stdout}{stderr}");
    // With this machine's Python after the shim, the share folder's search goes on to it.
    if needs("py", "a_batch_file_on_path_is_passed_over_in_a_share_folder") {
        let path = std::env::join_paths(std::iter::once(shims.clone()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap();
        let (_, stdout, stderr) = run(&unc, &path, &["doctor"]);
        let rows = doctor_rows(&stdout);
        assert!(rows["py"][0] == "ok" && rows["py"][1] != shim.display().to_string(), "{stdout}{stderr}");
        assert!(stdout.contains(&format!("\n  py: {skipped}\n")), "{stdout}");
        // The fix followed with only the shim on PATH: that Python's python.exe by its full path.
        let python = rows["py"][1].trim_matches('"').to_string();
        if python.to_ascii_lowercase().ends_with("python.exe") {
            let mut cmd = v2_command(&home.0);
            cmd.arg("s.v2").current_dir(&unc).env("PATH", &shims).env("V2_ENGINE_PY", &python).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(120)).unwrap();
            assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        }
    }
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
    // Round 22 (4) and its review (6): the venv's own module, missing from the Python
    // V2_ENGINE_PY names: the hint says why, before the install line (INTERNALS 13.5).
    std::fs::write(home.0.join("sub/main.v2"), "@import { where } from py.venvonly\nprint(where())\n").unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &["sub/main.v2"], &[("V2_ENGINE_PY", std::ffi::OsStr::new(&python))]);
    assert_eq!(code, Some(1), "{stderr}");
    let unset = if cfg!(windows) { "Remove-Item Env:V2_ENGINE_PY" } else { "unset V2_ENGINE_PY" };
    let note = format!(".venv' (v2.toml) isn't used: to use it, {unset}; or if 'venvonly' isn't misspelled, install it for the Python V2 uses");
    assert!(stderr.contains("ImportError: No module named 'venvonly'") && stderr.contains("hint: V2_ENGINE_PY is set, so [python] venv '") && stderr.contains(&note), "{stderr}");
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
    // executable at '…'" (CI's). Round 22 (0): on Windows, the Python its home names instead.
    let problem = if cfg!(windows) {
        format!("the activated venv {} (VIRTUAL_ENV) has a python that doesn't run, so V2 can't use it: the Python this venv was made from, C:\\nonexistent_r19\\Python311\\python.exe, isn't there any more: put it back, or make the venv again", venv.display())
    } else {
        format!("the activated venv {} (VIRTUAL_ENV) has a python that doesn't run (", venv.display())
    };
    let (code, _, stderr) = run_in_home(&home.0, &["proj/m.v2"], &env);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains(&format!("hint: {problem}")) && stderr.contains("nonexistent_r19") && stderr.contains("so V2 can't use it: "), "{stderr}");
    assert!(cfg!(windows) || stderr.contains("so V2 can't use it: make the venv again, "), "{stderr}");
    assert!(!stderr.contains("pip") && !stderr.contains("which V2 isn't using"), "{stderr}");
    let (code, stdout, stderr) = run_in_home(&home.0, &["doctor"], &env);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.contains(&format!("\n  py: {problem}")), "{stdout}");
}

#[test]
fn a_package_imported_by_its_pip_name_gets_the_module_it_provides() {
    // Round 21 (L0, L9): a distribution the venv has, asked for by its pip name (normalized:
    // pyzzyaml for PyZzYAML, zz_opencv_python, or quoted with -), names its module before any
    // install line, as pip would only say it is installed; a my-helper.py of yours gets the rename.
    // Fake distributions like PyYAML's (top_level.txt) and opencv-python's (a RECORD only), and
    // each hint works when followed (INTERNALS 13.5). Review of round 21: like protobuf's, a
    // namespace package is named by its package (google.protobuf, never google); like
    // llama-cpp-python's, folders of data (bin, include, lib) are no modules; like
    // python-multipart's, the module closest to the distribution's name comes first; one with no
    // module gets no such hint; and py.myhelper is never offered my-helper.py. Second review: a
    // pip install -e of a flat layout (its RECORD lists only the .pth and setuptools' finder,
    // which resolves the module from the project), of a src layout (a .pth naming the folder) and
    // setup.py develop (an egg-info with SOURCES.txt, no RECORD) are named by their top_level.txt;
    // a namespace folder it names but has no file in (google-auth's scripts) is none, and an
    // editable install's own finder module is never named.
    if !needs("py", "a_package_imported_by_its_pip_name_gets_the_module_it_provides") {
        return;
    }
    let python = block_python();
    let home = TempHome::new();
    let venv = home.0.join(".venv");
    let made = Command::new(&python).args(["-m", "venv", "--without-pip"]).arg(&venv).output().expect("run python -m venv");
    assert!(made.status.success(), "python -m venv: {}", String::from_utf8_lossy(&made.stderr));
    let site = find_dir(&venv, "site-packages").expect("the venv's site-packages");
    for (rel, text) in [
        ("PyZzYAML-6.0.dist-info/METADATA", "Metadata-Version: 2.1\nName: PyZzYAML\nVersion: 6.0\n"),
        ("PyZzYAML-6.0.dist-info/top_level.txt", "_zzyaml\nzzyaml\n"),
        ("zzyaml/__init__.py", "x = \"zzyaml here\"\n"),
        ("zz_opencv_python-5.0.dist-info/METADATA", "Metadata-Version: 2.1\nName: zz-opencv-python\nVersion: 5.0\n"),
        ("zz_opencv_python-5.0.dist-info/RECORD", "zzcv2/__init__.py,,\nzz_opencv_python-5.0.dist-info/METADATA,,\nzz_opencv_python-5.0.dist-info/RECORD,,\n"),
        ("zzcv2/__init__.py", "y = 2\n"),
        ("zzprotobuf-4.0.dist-info/METADATA", "Metadata-Version: 2.1\nName: zzprotobuf\nVersion: 4.0\n"),
        ("zzprotobuf-4.0.dist-info/top_level.txt", "zzgoogle\nzzscripts\n"),
        ("zzscripts/notes.txt", ""),
        ("zzprotobuf-4.0.dist-info/RECORD", "zzgoogle/zzproto/__init__.py,,\nzzgoogle/zzproto/wire.py,,\nzzgoogle/_zzupb/_message.abi3.so,,\nzzprotobuf-4.0.dist-info/RECORD,,\n"),
        ("zzgoogle/zzproto/__init__.py", "z = 4\n"),
        ("zz_llama_python-0.3.dist-info/METADATA", "Metadata-Version: 2.1\nName: zz-llama-python\nVersion: 0.3\n"),
        ("zz_llama_python-0.3.dist-info/RECORD", "bin/zz-llama-cli,,\ninclude/zzllama.h,,\nlib/libzzllama.so,,\nzz_llama/__init__.py,,\nzz_llama/lib/libzzllama.so,,\nzz_llama_python-0.3.dist-info/RECORD,,\n"),
        ("zz_llama/__init__.py", "w = 5\n"),
        ("zz_python_multipart-0.0.20.dist-info/METADATA", "Metadata-Version: 2.1\nName: zz-python-multipart\nVersion: 0.0.20\n"),
        ("zz_python_multipart-0.0.20.dist-info/RECORD", "multizz/__init__.py,,\nzz_python_multipart/__init__.py,,\nzz_python_multipart-0.0.20.dist-info/RECORD,,\n"),
        ("zz_python_multipart/__init__.py", "v = 6\n"),
        ("zz_data_only-1.0.dist-info/METADATA", "Metadata-Version: 2.1\nName: zz-data-only\nVersion: 1.0\n"),
        ("zz_data_only-1.0.dist-info/RECORD", "share/zzdata/a.txt,,\nbin/zzdata,,\nzz_data_only-1.0.dist-info/RECORD,,\n"),
        // Python 3.12 and later list only the files of a RECORD that are there.
        ("multizz/__init__.py", ""),
        ("zzgoogle/zzproto/wire.py", ""),
        ("zzgoogle/_zzupb/_message.abi3.so", ""),
        ("bin/zz-llama-cli", ""),
        ("include/zzllama.h", ""),
        ("lib/libzzllama.so", ""),
        ("zz_llama/lib/libzzllama.so", ""),
        ("share/zzdata/a.txt", ""),
        ("bin/zzdata", ""),
    ] {
        let path = site.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let src = |dir: &str| home.0.join("editsrc").join(dir);
    let egg = src("eggproj");
    for (path, text) in [
        (src("flat").join("zzedithelper.py"), "e = 7\n"),
        (src("srcl").join("zzother_pkg").join("__init__.py"), "o = 8\n"),
        (egg.join("zz_egg.egg-info").join("PKG-INFO"), "Metadata-Version: 2.1\nName: zz-egg\nVersion: 1.0\n"),
        (egg.join("zz_egg.egg-info").join("top_level.txt"), "zzeggmod\n"),
        (egg.join("zz_egg.egg-info").join("SOURCES.txt"), "setup.py\nzzeggmod/__init__.py\ntests/__init__.py\nzz_egg.egg-info/PKG-INFO\n"),
        (egg.join("setup.py"), ""),
        (egg.join("zzeggmod").join("__init__.py"), "g = 9\n"),
        (egg.join("tests").join("__init__.py"), ""),
    ] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let finder = format!(
        "import sys\nfrom importlib.util import spec_from_file_location\n\nMAPPING = {{\"zzedithelper\": {:?}}}\n\n\nclass _EditableFinder:\n    @classmethod\n    \
         def find_spec(cls, fullname, path=None, target=None):\n        if fullname in MAPPING:\n            return spec_from_file_location(fullname, MAPPING[fullname])\n        \
         return None\n\n\ndef install():\n    if _EditableFinder not in sys.meta_path:\n        sys.meta_path.append(_EditableFinder)\n",
        src("flat").join("zzedithelper.py").display().to_string()
    );
    for (rel, text) in [
        ("__editable___zz_edit_helper_0_1_finder.py", finder),
        ("__editable__.zz_edit_helper-0.1.pth", "import __editable___zz_edit_helper_0_1_finder; __editable___zz_edit_helper_0_1_finder.install()\n".into()),
        ("zz_edit_helper-0.1.dist-info/METADATA", "Metadata-Version: 2.1\nName: zz-edit-helper\nVersion: 0.1\n".into()),
        ("zz_edit_helper-0.1.dist-info/top_level.txt", "zzedithelper\n".into()),
        ("zz_edit_helper-0.1.dist-info/RECORD", "__editable__.zz_edit_helper-0.1.pth,,\n__editable___zz_edit_helper_0_1_finder.py,,\nzz_edit_helper-0.1.dist-info/RECORD,,\n".into()),
        ("__editable__.zz_other_dist-0.1.pth", format!("{}\n", src("srcl").display())),
        ("zz_other_dist-0.1.dist-info/METADATA", "Metadata-Version: 2.1\nName: zz-other-dist\nVersion: 0.1\n".into()),
        ("zz_other_dist-0.1.dist-info/top_level.txt", "zzother_pkg\n".into()),
        ("zz_other_dist-0.1.dist-info/RECORD", "__editable__.zz_other_dist-0.1.pth,,\nzz_other_dist-0.1.dist-info/RECORD,,\n".into()),
        ("easy-install.pth", format!("{}\n", egg.display())),
        ("__editable__.zz_edit_bare-0.1.pth", "# its finder would be imported here\n".into()),
        ("__editable___zz_edit_bare_0_1_finder.py", "".into()),
        ("zz_edit_bare-0.1.dist-info/METADATA", "Metadata-Version: 2.1\nName: zz-edit-bare\nVersion: 0.1\n".into()),
        ("zz_edit_bare-0.1.dist-info/RECORD", "__editable__.zz_edit_bare-0.1.pth,,\n__editable___zz_edit_bare_0_1_finder.py,,\nzz_edit_bare-0.1.dist-info/RECORD,,\n".into()),
    ] {
        std::fs::create_dir_all(site.join(rel).parent().unwrap()).unwrap();
        std::fs::write(site.join(rel), text).unwrap();
    }
    std::fs::write(home.0.join("v2.toml"), "[python]\nvenv = \".venv\"\n").unwrap();
    std::fs::write(home.0.join("my-helper.py"), "def f():\n    return \"helper\"\n").unwrap();
    let imports = ["py.pyzzyaml as m", "py.PyZzYAML as m", "py.\"zz-opencv-python\" as m", "py.zz_opencv_python as m", "{ f } from py.\"my-helper\"", "{ f } from py.my_helper",
        "py.zzprotobuf as m", "py.zz_llama_python as m", "py.\"zz-python-multipart\" as m",
        "py.zz_edit_helper as m", "py.zz_other_dist as m", "py.zz_egg as m", "py.zz_data_only as m", "py.zz_edit_bare as m", "{ f } from py.myhelper"];
    let tries: String = imports.iter().map(|i| format!("try {{\n    @import {i}\n}} catch (e) {{\n    print(e.message, \"|\", e.hint)\n}}\n")).collect();
    std::fs::write(home.0.join("m.v2"), tries).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["m.v2"], &[]);
    let yaml = "PyZzYAML is installed, and its module is named zzyaml: @import py.zzyaml as zzyaml";
    let cv = "zz-opencv-python is installed, and its module is named zzcv2: @import py.zzcv2 as zzcv2";
    let rename = "Python module names use _: rename my-helper.py to my_helper.py and import py.my_helper";
    let want = [
        format!("No module named 'pyzzyaml' | {yaml}"),
        format!("No module named 'PyZzYAML' | {yaml}"),
        format!("'zz-opencv-python' is not a valid Python module name | {cv}"),
        format!("No module named 'zz_opencv_python' | {cv}"),
        format!("'my-helper' is not a valid Python module name | {rename}"),
        format!("No module named 'my_helper' | {rename}"),
        "No module named 'zzprotobuf' | zzprotobuf is installed, and its module is named zzgoogle.zzproto: @import py.zzgoogle.zzproto as zzproto".into(),
        "No module named 'zz_llama_python' | zz-llama-python is installed, and its module is named zz_llama: @import py.zz_llama as zz_llama".into(),
        "'zz-python-multipart' is not a valid Python module name | zz-python-multipart is installed, and its modules are named zz_python_multipart and multizz: @import py.zz_python_multipart as zz_python_multipart".into(),
        "No module named 'zz_edit_helper' | zz-edit-helper is installed, and its module is named zzedithelper: @import py.zzedithelper as zzedithelper".into(),
        "No module named 'zz_other_dist' | zz-other-dist is installed, and its module is named zzother_pkg: @import py.zzother_pkg as zzother_pkg".into(),
        "No module named 'zz_egg' | zz-egg is installed, and its module is named zzeggmod: @import py.zzeggmod as zzeggmod".into(),
    ];
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!((code, &lines[..lines.len().min(want.len())]), (Some(0), &want.iter().map(String::as_str).collect::<Vec<_>>()[..]), "{stderr}");
    // No module to name, and no my-helper.py offered for py.myhelper: the install line.
    let rest = &lines[want.len()..];
    assert_eq!(rest.len(), 3, "{stdout}");
    for (line, name) in rest.iter().zip(["zz_data_only", "zz_edit_bare", "myhelper"]) {
        assert!(line.starts_with(&format!("No module named '{name}' | if '{name}' isn't misspelled, install it for the Python V2 uses")) && !line.contains("is installed"), "{line}");
    }
    // Uncaught: the hint, and no install line.
    std::fs::write(home.0.join("m.v2"), "@import py.pyzzyaml as m\nprint(m.x)\n").unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &["m.v2"], &[]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("ImportError: No module named 'pyzzyaml'") && stderr.contains(&format!("\n  hint: {yaml}\n")) && !stderr.contains("pip"), "{stderr}");
    // Followed.
    std::fs::rename(home.0.join("my-helper.py"), home.0.join("my_helper.py")).unwrap();
    let follow = "@import py.zzyaml as zzyaml\n@import py.zzcv2 as zzcv2\n@import { f } from py.my_helper\n@import py.zzgoogle.zzproto as zzproto\n\
        @import py.zz_llama as zz_llama\n@import py.zz_python_multipart as zz_python_multipart\n\
        @import py.zzedithelper as zzedithelper\n@import py.zzother_pkg as zzother_pkg\n@import py.zzeggmod as zzeggmod\n\
        print(zzyaml.x, zzcv2.y, f(), zzproto.z, zz_llama.w, zz_python_multipart.v, zzedithelper.e, zzother_pkg.o, zzeggmod.g)\n";
    std::fs::write(home.0.join("m.v2"), follow).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["m.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(0), "zzyaml here 2 helper 4 5 6 7 8 9\n"), "{stderr}");
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
fn bidi_characters_in_file_names_are_shown_escaped() {
    // 2.1, 2.4, DOCS 3.9 (round 20): a bidi override in a file's name (`rapor\u{202e}v2.exe`, which
    // a terminal would show reversed) prints as \u{202e} in a diagnostic's header and source line,
    // the CLI's listing and did-you-mean, and e.file. Windows names can hold it too.
    let home = TempHome::new();
    let name = "rapor\u{202e}lmth.v2";
    std::fs::write(home.0.join(name), "import \"./m\u{2066}x.v2\" as m\nlet x = nope // \u{202e} note\n").unwrap();
    std::fs::write(home.0.join("m\u{2066}x.v2"), "try { throw \"boom\" } catch (e) { print(e.file) }\n").unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &[name], &[]);
    assert_eq!((code, stdout.as_str()), (Some(1), "m\\u{2066}x.v2\n"), "{stderr:?}");
    assert!(stderr.starts_with("rapor\\u{202e}lmth.v2:2:9: NameError: 'nope' is not declared\n    let x = nope // \\u{202e} note\n            ^\n"), "{stderr:?}");
    let (code, _, listing) = run_in_home(&home.0, &["rapor\u{202e}lmtx.v2"], &[]);
    assert_eq!(code, Some(2), "{listing:?}");
    assert!(listing.contains("\ndid you mean 'rapor\\u{202e}lmth.v2'?\n"), "{listing:?}");
    for text in [&stdout, &stderr, &listing] {
        assert!(!text.contains(['\u{202e}', '\u{2066}']), "{text:?}");
    }
}

#[test]
#[cfg_attr(not(debug_assertions), ignore = "the V2_TEST_MAX_READ cap exists in debug builds only")]
fn read_file_and_exec_stop_past_what_a_str_holds() {
    // 5.10: past 1 GiB, the most text `*` makes (here the debug builds' V2_TEST_MAX_READ,
    // 1000 bytes), read_file and exec raise IOError instead of using up the memory, and exec
    // stops the program there; what fits is read whole. read_file's hint shows blocks that read
    // a piece (round 23; tests/host/builtins/read_hints.v2 runs them).
    let main = "let v2 = args()[0]\n\
                for f in [\"fits.txt\", \"big.txt\"] {\n\
                \x20   try { print(len(read_file(f)), len(read_file(f, binary: true))) } catch (e) { print(e.type + \": \" + e.message); print(e.hint) }\n\
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
    let hint = r#"for its first MiB: @py(path) { open(path, "rb").read(1 << 20) }; for its last: @py(path) { f = open(path, "rb"); f.seek(-(1 << 20), 2); f.read() }"#;
    let want = format!("1000 1000\nIOError: cannot read 'big.txt': it holds more than 1 GiB, the most V2 reads at once: read it in pieces in a block\n{hint}\n1000\n{exec}\n{exec}\n");
    assert_eq!(stdout, want);
    // Sources that never end: a device, and a program that writes until it is stopped.
    #[cfg(unix)]
    {
        let home = TempHome::new();
        let src = "for f in [func() => read_file(\"/dev/zero\"), func() => read_file(\"/dev/zero\", binary: true), func() => exec([\"yes\"])] {\n    try { f() } catch (e) { print(e.message + \" | \" + str(e.hint)) }\n}\n";
        std::fs::write(home.0.join("endless.v2"), src).unwrap();
        let (code, stdout, stderr) = run_capped(&home.0, &["endless.v2"], &[("V2_TEST_MAX_READ", "100000")]);
        assert_eq!(code, Some(0), "{stderr}");
        // A device has no end to seek from: the first MiB's block only (the review of round 23).
        let device = r#"cannot read '/dev/zero': it holds more than 1 GiB, the most V2 reads at once: read it in pieces in a block | for its first MiB: @py(path) { open(path, "rb").read(1 << 20) }"#;
        assert_eq!(stdout, format!("{device}\n{device}\n{} | null\n", exec.trim_start_matches("IOError: ").replace("'v2'", "'yes'")));
    }
}

#[cfg(windows)]
#[test]
fn a_file_another_program_holds_open_says_to_close_it_there() {
    // 5.10: a file another program opened without sharing, as Excel does, is refused by Windows
    // with ERROR_SHARING_VIOLATION; each file builtin's IOError says what to do, a block of the
    // script being one such program, and only a write can go to another name (round 23).
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
    let close = "another program has it open (Excel keeps an open file locked; so does a block of this script that opened it): close it there";
    let write = format!("{close}, or write to another file name");
    let want: String = [("read", close), ("write", &write), ("write", &write), ("remove", close)].iter().map(|(v, hint)| format!("cannot {v} 'results.csv' | {hint}\n")).collect();
    assert_eq!(stdout, want);
}

#[cfg(windows)]
#[test]
fn write_file_leaves_a_locked_or_mapped_file_as_it_was() {
    // 5.10 (round 23): another program's lock of a part of a file (as a database keeps: bytes
    // 0-99, or 500-599 and 1,000,000-1,000,099, past the new data and past the end; a shared lock
    // too) or a mapping of it refuses write_file before anything changed: Rust's fs::write had cut
    // a locked file to nothing, then failed to write. A free file, a hidden and a system one, a
    // new one and empty data are written as before.
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY};
    use windows_sys::Win32::System::Memory::{CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_READ, PAGE_READONLY};
    let home = TempHome::new();
    let d = &home.0;
    let old = "D".repeat(1000);
    let held = ["lock0.db", "lock500.db", "far.db", "shared.db", "mapped.db"];
    for (name, attributes) in held.iter().map(|&n| (n, 0)).chain([("free.db", 0), ("hidden.db", 2), ("system.db", 4), ("empty.db", 0)]) {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).attributes(attributes).open(d.join(name)).unwrap();
        f.write_all(old.as_bytes()).unwrap();
    }
    // Held by this process, as another program would: closing a file ends its lock.
    let lock = |name: &str, at: u32, exclusive: bool| {
        let f = std::fs::OpenOptions::new().read(true).write(true).open(d.join(name)).unwrap();
        let how = if exclusive { LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY } else { LOCKFILE_FAIL_IMMEDIATELY };
        // SAFETY: a zeroed OVERLAPPED that says where the range starts, for a handle `f` keeps
        // open.
        let ok = unsafe {
            let mut at_offset: windows_sys::Win32::System::IO::OVERLAPPED = std::mem::zeroed();
            at_offset.Anonymous.Anonymous.Offset = at;
            LockFileEx(f.as_raw_handle(), how, 0, 100, 0, &mut at_offset)
        };
        assert_ne!(ok, 0, "LockFileEx {name}");
        f
    };
    let locks = [lock("lock0.db", 0, true), lock("lock500.db", 500, true), lock("far.db", 1_000_000, true), lock("shared.db", 0, false)];
    let mapped = std::fs::File::open(d.join("mapped.db")).unwrap();
    // SAFETY: a read-only mapping of a file `mapped` keeps open, and a view of all of it; both
    // are closed below.
    let (mapping, view) = unsafe {
        let mapping = CreateFileMappingW(mapped.as_raw_handle(), std::ptr::null(), PAGE_READONLY, 0, 0, std::ptr::null());
        assert!(!mapping.is_null(), "CreateFileMappingW");
        let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
        assert!(!view.Value.is_null(), "MapViewOfFile");
        (mapping, view)
    };
    let src = "for f in [\"lock0.db\", \"lock500.db\", \"far.db\", \"shared.db\", \"mapped.db\"] {\n\
               \x20   try { write_file(f, \"new contents\"); print(f, \"written\") } catch (e) { print(e.message + \" | \" + e.hint) }\n\
               }\n\
               write_file(\"free.db\", \"new contents\")\nwrite_file(\"hidden.db\", \"new contents\")\nwrite_file(\"system.db\", b\"new contents\")\n\
               write_file(\"new.db\", \"new contents\")\nwrite_file(\"empty.db\", \"\")\nprint(\"done\")\n";
    std::fs::write(d.join("held.v2"), src).unwrap();
    let (code, stdout, stderr) = run_in_home(d, &["held.v2"], &[]);
    // SAFETY: the view and the mapping made above, unmapped and closed once.
    unsafe {
        UnmapViewOfFile(view);
        CloseHandle(mapping);
    }
    drop((locks, mapped));
    assert_eq!(code, Some(0), "{stderr}");
    let hint = "another program has it open (Excel keeps an open file locked; so does a block of this script that opened it): close it there, or write to another file name";
    let locked = "The process cannot access the file because another process has locked a portion of the file";
    let want = format!(
        "cannot write 'lock0.db': {locked} | {hint}\ncannot write 'lock500.db': {locked} | {hint}\ncannot write 'far.db': {locked} | {hint}\n\
         cannot write 'shared.db': {locked} | {hint}\ncannot write 'mapped.db': The requested operation cannot be performed on a file with a user-mapped section open | {hint}\ndone\n"
    );
    assert_eq!(stdout, want);
    for name in held {
        assert_eq!(std::fs::read_to_string(d.join(name)).unwrap(), old, "{name}");
    }
    for name in ["free.db", "hidden.db", "system.db", "new.db"] {
        assert_eq!(std::fs::read_to_string(d.join(name)).unwrap(), "new contents", "{name}");
    }
    assert_eq!(std::fs::read(d.join("empty.db")).unwrap(), b"");
    assert_eq!(std::fs::metadata(d.join("hidden.db")).unwrap().file_attributes() & 2, 2, "still hidden");
    assert_eq!(std::fs::metadata(d.join("system.db")).unwrap().file_attributes() & 4, 4, "still a system file");
}

#[cfg(windows)]
#[test]
fn write_file_leaves_a_file_whole_when_another_program_locks_it_on_and_off() {
    // 5.10 (the second review of round 23): a program that locks and unlocks a part of a file
    // again and again (a database's short transactions, here 0.5 ms each way) could take its lock
    // between a look for one and the cut, and the write then failed on a cut file. write_file holds
    // its lock from before the cut to after the write: each write is refused with the file whole,
    // or done. (With a look alone, some of the 300 writes left the file cut.)
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{LockFileEx, UnlockFile, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY};
    let home = TempHome::new();
    let d = &home.0;
    std::fs::write(d.join("db.bin"), "D".repeat(4000)).unwrap();
    let src = "let data = \"N\" * 4000\nlet cut = 0\nfor i in 0..300 {\n\
               \x20   try { write_file(\"db.bin\", data) } catch (e) {\n\
               \x20       let now = -1\n\
               \x20       while now < 0 {\n            try { now = len(read_file(\"db.bin\", binary: true)) } catch (e2) { now = -1 }\n        }\n\
               \x20       if now != 4000 { cut += 1 }\n    }\n}\nprint(\"cut\", cut)\n";
    std::fs::write(d.join("w.v2"), src).unwrap();
    let held = std::fs::OpenOptions::new().read(true).write(true).open(d.join("db.bin")).unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let churn = {
        let (stop, h) = (stop.clone(), held.as_raw_handle() as usize);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                // SAFETY: a zeroed OVERLAPPED (the range starts at 0), for the handle `held` keeps
                // open until this thread has ended.
                unsafe {
                    let mut at: windows_sys::Win32::System::IO::OVERLAPPED = std::mem::zeroed();
                    if LockFileEx(h as _, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY, 0, 100, 0, &mut at) != 0 {
                        std::thread::sleep(Duration::from_micros(500));
                        UnlockFile(h as _, 0, 0, 100, 0);
                    }
                }
                std::thread::sleep(Duration::from_micros(500));
            }
        })
    };
    let (code, stdout, stderr) = run_in_home(d, &["w.v2"], &[]);
    stop.store(true, Ordering::Relaxed);
    churn.join().unwrap();
    drop(held);
    assert_eq!((code, stdout.as_str()), (Some(0), "cut 0\n"), "{stderr}");
}

#[cfg(windows)]
#[test]
fn write_file_to_a_named_pipe_opens_it_once() {
    // 5.10 (round 23): a named pipe is no disk file: write_file writes to it without the cut that
    // failed ("The parameter is incorrect"), and opens it once: each look at the path (for WSL's
    // pipes, then for the error's message) had taken another of its server's instances. With
    // append: true it is written too (the access that appends is a pipe's "create an instance",
    // which it refused).
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, PIPE_ACCESS_INBOUND};
    use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES};
    let home = TempHome::new();
    let name = format!(r"\\.\pipe\v2-test-pipe-{}", std::process::id());
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    // Four instances wait; each keeps what its client sent.
    let servers: Vec<_> = (0..4)
        .map(|_| {
            // SAFETY: a NUL-terminated name; the handle is closed by its thread.
            let h = unsafe { CreateNamedPipeW(wide.as_ptr(), PIPE_ACCESS_INBOUND, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, 4096, 4096, 0, std::ptr::null()) };
            assert_ne!(h, INVALID_HANDLE_VALUE, "CreateNamedPipeW");
            let (h, got) = (h as usize, got.clone());
            std::thread::spawn(move || {
                let h = h as HANDLE;
                let mut data = Vec::new();
                let mut buf = [0u8; 64];
                // SAFETY: the instance made above, read into `buf` until its client is gone.
                unsafe {
                    ConnectNamedPipe(h, std::ptr::null_mut());
                    let mut n = 0u32;
                    while ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut n, std::ptr::null_mut()) != 0 && n > 0 {
                        data.extend_from_slice(&buf[..n as usize]);
                    }
                    CloseHandle(h);
                }
                got.lock().unwrap().push(data);
            })
        })
        .collect();
    let lit = name.replace('\\', "\\\\");
    let src = format!("write_file(\"{lit}\", \"hello\")\nwrite_file(\"{lit}\", \"again\", append: true)\nprint(\"ok\")\n");
    std::fs::write(home.0.join("pipe.v2"), src).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["pipe.v2"], &[]);
    // The instances still waiting get a client of the test's own.
    for _ in 0..4 {
        if let Ok(mut client) = std::fs::OpenOptions::new().write(true).open(&name) {
            client.write_all(b"cleanup").unwrap();
        }
    }
    for s in servers {
        s.join().unwrap();
    }
    assert_eq!((code, stdout.as_str()), (Some(0), "ok\n"), "{stderr}");
    let mut from_v2: Vec<Vec<u8>> = got.lock().unwrap().iter().filter(|d| d.as_slice() != b"cleanup").cloned().collect();
    from_v2.sort();
    assert_eq!(from_v2, [b"again".to_vec(), b"hello".to_vec()]);
}

#[cfg(windows)]
#[test]
fn device_paths_of_a_disk_file_append_and_roots_keep_their_messages() {
    // 5.10 (the second review of round 23): a file reached by its volume's device path
    // (\\?\GLOBALROOT\Device\HarddiskVolumeN\…, \\?\HarddiskVolumeN\…) is appended to at its end
    // and made when missing (append had written it from its start, as for a pipe), and a slash
    // after its name is refused there too (it was dropped before the check). A root with its
    // separator is no name with a slash: exists() of \\?\C:\, \\.\C:\ and \\?\Volume{…}\ is true,
    // and write_file of \\?\C:\ says it is a folder.
    use windows_sys::Win32::Storage::FileSystem::{GetVolumeNameForVolumeMountPointW, QueryDosDeviceW};
    let home = TempHome::new();
    let real = real_path(&home.0);
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let mut buf = [0u16; 1024];
    // SAFETY: NUL-terminated names and a buffer of the length given.
    let n = unsafe { QueryDosDeviceW(wide(&real[..2]).as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    assert!(n > 0, "QueryDosDeviceW");
    let device = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap()]);
    // SAFETY: as above.
    let ok = unsafe { GetVolumeNameForVolumeMountPointW(wide(&real[..3]).as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    assert_ne!(ok, 0, "GetVolumeNameForVolumeMountPointW");
    let volume = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap()]);
    let lit = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\"));
    let mut src = String::from("func show(f) {\n    try { print(f()) } catch (e) { print(e.message + \" | \" + str(e.hint)) }\n}\n");
    for (name, at) in [("gr.txt", format!(r"\\?\GLOBALROOT{device}")), ("hv.txt", format!(r"\\?\{}", device.rsplit('\\').next().unwrap()))] {
        let file = format!(r"{at}{}\{name}", &real[2..]);
        src.push_str(&format!(
            "write_file({0}, \"data\")\nwrite_file({0}, \"XY\", append: true)\nprint(read_file({0}))\nwrite_file({1}, \"fresh\", append: true)\nprint(read_file({1}))\n",
            lit(&file),
            lit(&format!("{file}.new"))
        ));
        src.push_str(&format!("show(func() => read_file({}))\nprint(exists({}))\n", lit(&format!("{file}/")), lit(&format!("{file}/"))));
    }
    for root in [r"\\?\C:\", r"\\.\C:\", volume.as_str()] {
        src.push_str(&format!("print(exists({}))\n", lit(root)));
    }
    for root in [r"\\?\C:\", r"\\.\C:\"] {
        src.push_str(&format!("show(func() => write_file({}, \"x\"))\n", lit(root)));
    }
    std::fs::write(home.0.join("dev.v2"), src).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["dev.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    let mut want = String::new();
    for (name, at) in [("gr.txt", format!(r"\\?\GLOBALROOT{device}")), ("hv.txt", format!(r"\\?\{}", device.rsplit('\\').next().unwrap()))] {
        let file = format!(r"{at}{}\{name}", &real[2..]);
        let mut q = file.replace('\\', "\\\\");
        q = format!("\"{q}\"");
        want.push_str(&format!("dataXY\nfresh\ncannot read '{file}\\': it is a file, not a folder | leave out the slash at its end: read_file({q})\nfalse\n"));
    }
    want.push_str("true\ntrue\ntrue\ncannot write '\\\\?\\C:\\': it is a folder | null\ncannot write '\\\\.\\C:\\': it is a folder | null\n");
    assert_eq!(stdout, want);
}

#[cfg(windows)]
#[test]
fn write_file_to_a_mailslot_writes_it() {
    // 5.10 (the review of round 23): a mailslot's handle is FILE_TYPE_UNKNOWN to GetFileType,
    // and can't be cut ("The parameter is incorrect"): it is written as it is, appended to too.
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::Mailslots::{CreateMailslotW, GetMailslotInfo};
    let home = TempHome::new();
    let name = format!(r"\\.\mailslot\v2-test-{}", std::process::id());
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: a NUL-terminated name; the handle is closed below.
    let slot = unsafe { CreateMailslotW(wide.as_ptr(), 0, 0, std::ptr::null()) };
    assert_ne!(slot, INVALID_HANDLE_VALUE, "CreateMailslotW");
    let lit = name.replace('\\', "\\\\");
    std::fs::write(home.0.join("slot.v2"), format!("write_file(\"{lit}\", \"hi\")\nwrite_file(\"{lit}\", \"more\", append: true)\nprint(\"ok\")\n")).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["slot.v2"], &[]);
    let mut got = Vec::new();
    loop {
        let (mut next, mut count) = (0u32, 0u32);
        // SAFETY: the mailslot made above; each message is read into a buffer of its size.
        unsafe {
            GetMailslotInfo(slot, std::ptr::null_mut(), &mut next, &mut count, std::ptr::null_mut());
            if count == 0 {
                break;
            }
            let mut message = vec![0u8; next as usize];
            let mut n = 0u32;
            ReadFile(slot, message.as_mut_ptr(), next, &mut n, std::ptr::null_mut());
            message.truncate(n as usize);
            got.push(message);
        }
    }
    // SAFETY: the handle made above, closed once.
    unsafe { CloseHandle(slot) };
    assert_eq!((code, stdout.as_str()), (Some(0), "ok\n"), "{stderr}");
    assert_eq!(got, [b"hi".to_vec(), b"more".to_vec()]);
}

#[cfg(windows)]
#[test]
fn remove_file_of_a_running_program_says_to_stop_it() {
    // 5.10 (round 23): Windows refuses to remove a running program's .exe with "Access is
    // denied", which permissions give too: the hint says what to do if it is the program. A
    // folder named tool.exe is a folder (the review of round 23).
    let home = TempHome::new();
    let d = &home.0;
    std::fs::copy(BIN, d.join("busy.exe")).unwrap();
    std::fs::write(d.join("wait.v2"), "sleep(60)\n").unwrap();
    // The copy runs until the test ends it by its handle, however the test ends.
    struct Running(Child);
    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let running = Running(Command::new(d.join("busy.exe")).arg("wait.v2").current_dir(d).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    std::fs::create_dir(d.join("tool.exe")).unwrap();
    std::fs::write(d.join("rm.v2"), "for f in [\"busy.exe\", \"tool.exe\"] {\n    try { remove_file(f) } catch (e) { print(e.message + \" | \" + e.hint) }\n}\n").unwrap();
    let (code, stdout, stderr) = run_in_home(d, &["rm.v2"], &[]);
    drop(running);
    let want = "cannot remove 'busy.exe': Access is denied | if that program is running, stop it first\n\
                cannot remove 'tool.exe': it is a folder, and remove_file removes files only | a @py block removes a folder with what is in it: @py(path) { import shutil; shutil.rmtree(path) }\n";
    assert_eq!((code, stdout.as_str()), (Some(0), want), "{stderr}");
}

#[cfg(windows)]
#[test]
fn a_v2_toml_another_program_holds_open_says_to_close_it() {
    // 18.5, 19.1 (round 23): the project's v2.toml that another program opened without sharing
    // is a config error (exit 2, as before) with the hint FILE gets, in a run and in v2 check.
    use std::os::windows::fs::OpenOptionsExt;
    let home = TempHome::new();
    let d = &home.0;
    std::fs::write(d.join("v2.toml"), "# nothing\n").unwrap();
    std::fs::write(d.join("solo.v2"), "print(1)\n").unwrap();
    let toml = real_path(&d.join("v2.toml"));
    let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(d.join("v2.toml")).unwrap();
    let want = format!(
        "{toml}: config error: cannot read the file: The process cannot access the file because it is being used by another process\n  hint: another program has it open (Excel keeps an open file locked): close it there\n"
    );
    for args in [&["solo.v2"][..], &["check", "solo.v2"]] {
        let (code, _, stderr) = run_in_home(d, args, &[]);
        assert_eq!((code, stderr.as_str()), (Some(2), want.as_str()), "{args:?}");
    }
    drop(held);
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

#[test]
fn remove_file_removes_a_link_not_the_folder_it_points_to() {
    // DOCS 3.11, 5.10 (round 23): as rm does, remove_file of a link to a folder (a junction on
    // Windows, which said "it is a folder" and hinted shutil.rmtree, which refuses a link) takes
    // the link and leaves the folder; `live/` names the folder, refused as one, with no rmtree
    // hint (given `live/`, rmtree would empty the folder), on Windows `live\` and `live/.` too. A
    // link that leads nowhere goes too (the review of round 23: Windows had refused a junction's),
    // and so does a read-only junction (attrib +r j /L), as rm takes one; a directory symlink,
    // which needs the privilege to make, is removed as a junction is.
    let home = TempHome::new();
    let d = &home.0;
    std::fs::create_dir_all(d.join("target")).unwrap();
    std::fs::write(d.join("target").join("keep.txt"), "kept").unwrap();
    dir_link(&d.join("live"), &d.join("target"));
    std::fs::create_dir_all(d.join("gone")).unwrap();
    dir_link(&d.join("dead"), &d.join("gone"));
    std::fs::remove_dir(d.join("gone")).unwrap();
    #[cfg(windows)]
    {
        dir_link(&d.join("readonly"), &d.join("target"));
        let ok = Command::new("attrib").args(["+r", "readonly", "/L"]).current_dir(d).stdout(Stdio::null()).status().unwrap().success();
        assert!(ok, "attrib +r readonly /L");
    }
    #[cfg(windows)]
    let symlink = std::os::windows::fs::symlink_dir(d.join("target"), d.join("sym")).is_ok();
    #[cfg(unix)]
    let symlink = false;
    let src = "func show(f) {\n    try { f(); print(\"removed\") } catch (e) { print(e.message + \" | \" + e.hint) }\n}\n\
               show(func() => remove_file(\"live/\"))\nif exists(\"readonly\") {\n    show(func() => remove_file(\"live\\\\\"))\n    show(func() => remove_file(\"live/.\"))\n    show(func() => remove_file(\"readonly\"))\n}\n\
               show(func() => remove_file(\"live\"))\nshow(func() => remove_file(\"dead\"))\n\
               if exists(\"sym\") { show(func() => remove_file(\"sym\")) }\n";
    std::fs::write(d.join("links.v2"), src).unwrap();
    let (code, stdout, stderr) = run_in_home(d, &["links.v2"], &[]);
    assert_eq!(code, Some(0), "{stderr}");
    let target = d.join("target").display().to_string();
    let folder = |written: &str| format!("cannot remove '{written}': it is a link to {target}, a folder, and remove_file removes files only | to remove the link alone, leave out the slash at its end: remove_file(\"live\")\n");
    let mut want = folder("live/");
    if cfg!(windows) {
        want = format!("{want}{}{}removed\n", folder("live\\"), folder("live/."));
    }
    want.push_str("removed\nremoved\n");
    if symlink {
        want.push_str("removed\n");
    }
    assert_eq!(stdout, want);
    for link in ["live", "dead", "readonly", "sym"] {
        assert!(std::fs::symlink_metadata(d.join(link)).is_err(), "{link} is gone");
    }
    assert!(!d.join("gone").exists(), "a dead link's target isn't made");
    assert_eq!(std::fs::read_to_string(d.join("target").join("keep.txt")).unwrap(), "kept");
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

/// A folder in the WSL distribution that `V2_TEST_WSL_DISTRO` names, for the Windows v2 on its
/// \\wsl.localhost share (INTERNALS 22.2): made with wsl.exe under `V2_TEST_WSL_DIR` (else
/// /tmp), and removed, its pipes opened first, so no reader of one is left waiting. `None`, and
/// the test skips, when the variable isn't set: Windows CI has no distribution.
#[cfg(windows)]
struct WslDir {
    distro: String,
    linux: String,
    share: PathBuf,
}

/// The WSL distribution `V2_TEST_WSL_DISTRO` names, or `None` with the test's SKIP line.
#[cfg(windows)]
fn wsl_distro(test: &str) -> Option<String> {
    let distro = std::env::var("V2_TEST_WSL_DISTRO").ok().filter(|d| !d.is_empty());
    if distro.is_none() {
        let _ = writeln!(std::io::stderr(), "SKIP {test} (V2_TEST_WSL_DISTRO names no WSL distribution)");
    }
    distro
}

/// wsl.exe for `distro`, as root when `root`, in a console of its own: wsl.exe sets the console's
/// code page, which the tests running beside it read (exec_windows.v2's chcp).
#[cfg(windows)]
fn wsl_exe(distro: &str, root: bool) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = Command::new("wsl.exe");
    cmd.args(["-d", distro]).creation_flags(CREATE_NO_WINDOW);
    if root {
        cmd.args(["-u", "root"]);
    }
    cmd
}

#[cfg(windows)]
impl WslDir {
    fn new(test: &str) -> Option<WslDir> {
        let distro = wsl_distro(test)?;
        let base = std::env::var("V2_TEST_WSL_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let linux = format!("{}/v2-{test}-{}", base.trim_end_matches('/'), std::process::id());
        let share = PathBuf::from(format!(r"\\wsl.localhost\{distro}{}", linux.replace('/', "\\")));
        let dir = WslDir { distro, linux, share };
        dir.sh(&format!("rm -rf '{0}' && mkdir -p '{0}'", dir.linux), false);
        Some(dir)
    }

    /// `script` run by sh in the distribution, in the folder once it is there; as root when
    /// `root` (mknod needs it).
    fn sh(&self, script: &str, root: bool) {
        let script = format!("cd '{}' 2>/dev/null; {script}", self.linux);
        let out = self.wsl(&script, root).expect("run wsl.exe");
        assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// wsl.exe running `script` with sh.
    fn wsl(&self, script: &str, root: bool) -> std::io::Result<Output> {
        wsl_exe(&self.distro, root).args(["-e", "sh", "-c", script]).stdin(Stdio::null()).output()
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.share.join(rel.replace('/', "\\"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// `v2 ARGS` in the folder, with a fresh home on this computer.
    fn v2(&self, args: &[&str]) -> (Option<i32>, String, String) {
        let home = TempHome::new();
        let mut cmd = v2_command(&home.0);
        cmd.args(args).current_dir(&self.share).env_remove("MSYSTEM").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = wait_with_timeout(cmd.spawn().expect("start v2"), Vec::new(), Duration::from_secs(120)).unwrap();
        let text = |b: &[u8]| normalize(&String::from_utf8_lossy(b));
        (out.status.code(), text(&out.stdout), text(&out.stderr))
    }
}

#[cfg(windows)]
impl Drop for WslDir {
    fn drop(&mut self) {
        // A pipe opened for reading and writing at once never waits, and gives a reader waiting on
        // it (an open from Windows that the share passed on) its end.
        let script = format!("for p in $(find '{0}' -type p); do : <> \"$p\"; done; rm -rf '{0}'", self.linux);
        let _ = self.wsl(&script, true);
    }
}

#[cfg(windows)]
#[test]
fn linux_symlinks_on_wsls_share_are_named_as_such() {
    // DOCS 3.10, 3.11, 19.1 (round 21): through \\wsl.localhost a Linux symlink is a reparse point
    // Windows can't follow (IO_REPARSE_TAG_LX_SYMLINK) wherever it points, and the share doesn't
    // give its target: named so, with what works, never "a link to something that isn't there"
    // with del advice. /dev/stdin keeps its Windows hint there, and a package in node_modules
    // that is such a link says so, not "a folder of your own".
    let Some(wsl) = WslDir::new("linux_symlinks_on_wsls_share_are_named_as_such") else { return };
    wsl.write("links/target.v2", "print(\"target runs\")\n");
    wsl.write("links/realdir/f.txt", "f\n");
    wsl.sh("ln -s target.v2 links/rel.v2 && ln -s realdir links/dirlink && ln -s nowhere.v2 links/broken.v2", false);
    let fix = "use the path it points to (ls -l in WSL shows it), or run V2 inside WSL";
    let lx = "is a Linux (WSL) symlink, which Windows can't follow";
    wsl.write(
        "main.v2",
        "for p in [\"links/rel.v2\", \"links/broken.v2\", \"links/dirlink/f.txt\"] {\n    try { read_file(p) } catch (e) { print(e.message + \" | \" + e.hint) }\n}\n\
         try { list_dir(\"links/dirlink\") } catch (e) { print(e.message + \" | \" + e.hint) }\n\
         try { import \"./links/rel.v2\" as r } catch (e) { print(e.message + \" | \" + e.hint) }\n\
         for p in [\"/dev/stdin\", \"/dev/stdout\"] {\n    try { write_file(p, \"x\") } catch (e) { print(e.hint) }\n}\n",
    );
    let (code, stdout, stderr) = wsl.v2(&["main.v2"]);
    let dev = "/dev/stdin, /dev/stdout and /dev/stderr are Linux's and macOS's: on Windows read standard input with input(), a line at a time, and write with print() and print(…, stderr: true)";
    let want = format!(
        "cannot read 'links/rel.v2': it {lx} | {fix}\ncannot read 'links/broken.v2': it {lx} | {fix}\ncannot read 'links/dirlink/f.txt': 'links/dirlink' {lx} | {fix}\n\
         cannot list 'links/dirlink': it {lx} | {fix}\ncannot import './links/rel.v2': it {lx} | {fix}\n{dev}\n{dev}\n"
    );
    assert_eq!((code, stdout.as_str()), (Some(0), want.as_str()), "{stderr}");
    // FILE, and v2 check of an import of it.
    let (code, _, stderr) = wsl.v2(&[r"links\rel.v2"]);
    assert_eq!(code, Some(2));
    assert!(stderr.starts_with("v2: cannot find 'rel.v2' in ") && stderr.ends_with(&format!("\n'rel.v2' {lx}: {fix}\n")), "{stderr}");
    let (code, _, stderr) = wsl.v2(&[r"links\dirlink\x.v2"]);
    assert_eq!((code, stderr), (Some(2), format!("v2: cannot find 'links\\dirlink\\x.v2'\n'links\\dirlink' {lx}: {fix}\n")));
    wsl.write("imp.v2", "import \"./links/rel.v2\" as r\n");
    let (code, stdout, stderr) = wsl.v2(&["check", "imp.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(1), "check imp.v2: 1 file, 1 error\n"), "{stderr}");
    assert!(stderr.contains(&format!("ImportError: cannot import './links/rel.v2': it {lx}\n")) && stderr.contains(&format!("hint: {fix}\n")), "{stderr}");
    if needs("js", "linux_symlinks_on_wsls_share_are_named_as_such") {
        wsl.write("localpkg/index.js", "module.exports.hello = () => \"hi\"\n");
        wsl.sh("mkdir -p node_modules && ln -s ../localpkg node_modules/localpkg", false);
        wsl.write("npm.v2", "@import { hello } from js.localpkg\nprint(hello())\n");
        let (code, _, stderr) = wsl.v2(&["npm.v2"]);
        assert_eq!(code, Some(1), "{stderr}");
        assert!(stderr.contains(&format!("node_modules\\localpkg' {lx}: {fix}\n")), "{stderr}");
    }
}

#[cfg(windows)]
#[test]
fn a_module_on_wsls_share_runs_once_however_the_share_is_spelled() {
    // DOCS 3.10, 6.4 (round 21): \\wsl$ and \\wsl.localhost are one share, a server and a share
    // are read in any case, and the share gives no file IDs to find one file by.
    let Some(wsl) = WslDir::new("a_module_on_wsls_share_runs_once_however_the_share_is_spelled") else { return };
    wsl.write("lib/util.v2", "print(\"util runs\")\nfunc v() { return 1 }\n");
    let (dollar, upper) = (format!("//wsl$/{}{}", wsl.distro, wsl.linux), format!("//WSL.LOCALHOST/{}{}", wsl.distro.to_uppercase(), wsl.linux));
    wsl.write("main.v2", &format!("import \"./lib/util.v2\" as a\nimport \"{dollar}/lib/util.v2\" as b\nimport \"{upper}/lib/util.v2\" as c\nprint(a.v(), b.v(), c.v())\n"));
    let (code, stdout, stderr) = wsl.v2(&["main.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "util runs\n1 1 1\n"), "{stderr}");
    let (code, stdout, stderr) = wsl.v2(&["check", "main.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(0), "check main.v2: 2 files, 0 errors\n"), "{stderr}");
}

#[cfg(windows)]
#[test]
fn wsls_pipes_and_devices_on_its_share_are_refused_unread() {
    // DOCS 3.10, 3.11, 19.1 (round 21): Windows takes WSL's FIFO or device node for a file; a read
    // of the FIFO waited 10 s (and left a thread of WSL's file server waiting), and /dev/null's
    // node ran as an empty script. Their reparse tags say what they are, before anything is read.
    let Some(wsl) = WslDir::new("wsls_pipes_and_devices_on_its_share_are_refused_unread") else { return };
    wsl.sh("mkfifo fifo.v2 && mknod null.v2 c 1 3", true);
    let start = Instant::now();
    for file in ["fifo.v2", "null.v2"] {
        let (code, stdout, stderr) = wsl.v2(&[file]);
        assert_eq!((code, stdout.as_str(), stderr), (Some(2), "", format!("v2: '{file}' is a pipe or a device, not a file: V2 runs scripts saved as .v2 files\n")));
    }
    wsl.write("imp.v2", "import \"./fifo.v2\" as f\n");
    let (code, _, stderr) = wsl.v2(&["imp.v2"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.starts_with("imp.v2:1:1: ImportError: cannot import './fifo.v2': it is a pipe or a device, not a file\n"), "{stderr}");
    let (code, stdout, stderr) = wsl.v2(&["check", "imp.v2"]);
    assert_eq!((code, stdout.as_str()), (Some(1), "check imp.v2: 1 file, 1 error\n"), "{stderr}");
    wsl.write("read.v2", "for p in [\"fifo.v2\", \"null.v2\"] {\n    try { read_file(p) } catch (e) { print(e.message + \" | \" + e.hint) }\n}\n");
    let (code, stdout, stderr) = wsl.v2(&["read.v2"]);
    let hint = "Windows can't read WSL's pipes and devices: run V2 inside WSL";
    let want = format!("cannot read 'fifo.v2': it is a pipe or a device, not a file | {hint}\ncannot read 'null.v2': it is a pipe or a device, not a file | {hint}\n");
    assert_eq!((code, stdout.as_str()), (Some(0), want.as_str()), "{stderr}");
    // write_file too, appending or not (the review of round 21): a write's open waited as long.
    wsl.write(
        "write.v2",
        "for p in [\"fifo.v2\", \"null.v2\"] {\n    for add in [false, true] {\n        try { write_file(p, \"x\", append: add) } catch (e) { print(e.message + \" | \" + e.hint) }\n    }\n}\n",
    );
    let (code, stdout, stderr) = wsl.v2(&["write.v2"]);
    let hint = "Windows can't write to WSL's pipes and devices: run V2 inside WSL";
    let want = ["fifo.v2", "fifo.v2", "null.v2", "null.v2"].map(|f| format!("cannot write '{f}': it is a pipe or a device, not a file | {hint}\n")).concat();
    assert_eq!((code, stdout.as_str()), (Some(0), want.as_str()), "{stderr}");
    assert!(start.elapsed() < Duration::from_secs(9), "{:?}: a read or write of the pipe was tried", start.elapsed());
}

#[cfg(windows)]
#[test]
fn a_linux_symlink_wsl_made_on_a_drive_is_named_as_such() {
    // DOCS 3.10, 3.11, 19.1 (the review of round 21): WSL's ln -s under /mnt/c makes a Linux
    // symlink there too, which Windows can't follow, and on some folders can't even open (the temp
    // folder here: os error 1920), when its folder's listing still has its tag. In the temp folder
    // and in the build's, which have one kind each here.
    let Some(distro) = wsl_distro("a_linux_symlink_wsl_made_on_a_drive_is_named_as_such") else { return };
    /// The folder and the WSL link in it, the link removed through WSL (Windows may not open it).
    struct Made(String, PathBuf);
    impl Drop for Made {
        fn drop(&mut self) {
            wsl_sh(&self.0, "rm -f \"$(wslpath -u \"$1\")\"", &self.1.join("l.v2"));
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let home = TempHome::new();
    let build = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("v2-lxdrive-{}", std::process::id()));
    std::fs::create_dir_all(&build).unwrap();
    let fix = "use the path it points to (ls -l in WSL shows it), or run V2 inside WSL";
    let lx = "is a Linux (WSL) symlink, which Windows can't follow";
    let made = [home.0.clone(), build].map(|at| Made(distro.clone(), PathBuf::from(real_path(&at))));
    for Made(_, d) in &made {
        std::fs::write(d.join("t.v2"), "print(\"t runs\")\n").unwrap();
        std::fs::write(d.join("main.v2"), "try { read_file(\"l.v2\") } catch (e) { print(e.message + \" | \" + e.hint) }\ntry { import \"./l.v2\" as m } catch (e) { print(e.message + \" | \" + e.hint) }\n").unwrap();
        assert!(wsl_sh(&distro, "ln -s t.v2 \"$(wslpath -u \"$1\")\"", &d.join("l.v2")), "ln -s in {}", d.display());
        let (code, stdout, stderr) = run_in_home(d, &["main.v2"], &[]);
        let want = format!("cannot read 'l.v2': it {lx} | {fix}\ncannot import './l.v2': it {lx} | {fix}\n");
        assert_eq!((code, stdout.as_str()), (Some(0), want.as_str()), "{}: {stderr}", d.display());
        let (code, _, stderr) = run_in_home(d, &["l.v2"], &[]);
        assert_eq!((code, stderr), (Some(2), format!("v2: cannot find 'l.v2'\nit {lx}: {fix}\n")), "{}", d.display());
    }
}

/// `script` run by sh in the WSL distribution `distro` as root, with `arg` as its `$1`.
#[cfg(windows)]
fn wsl_sh(distro: &str, script: &str, arg: &Path) -> bool {
    wsl_exe(distro, true).args(["-e", "sh", "-c", script, "sh"]).arg(arg).stdin(Stdio::null()).output().is_ok_and(|o| o.status.success())
}

#[cfg(windows)]
#[test]
fn a_wsl_distribution_that_isnt_there_is_named() {
    // 5.10, 6.4, 19.1 (round 21 and its review): Windows says "The user name or password is
    // incorrect" for one through \\wsl.localhost, and a bare \\wsl.localhost names none: in the
    // file functions, an import, v2 FILE and v2 check FILE.
    if wsl_distro("a_wsl_distribution_that_isnt_there_is_named").is_none() {
        return;
    }
    let home = TempHome::new();
    let main = "for p in [\"//wsl.localhost/v2-no-such-distro/x.txt\", \"//wsl$/v2-no-such-distro/x.txt\", \"//wsl.localhost\"] {\n    try { read_file(p) } catch (e) { print(e.message) }\n}\n";
    std::fs::write(home.0.join("main.v2"), main).unwrap();
    let (code, stdout, stderr) = run_in_home(&home.0, &["main.v2"], &[]);
    let no = "there is no WSL distribution named v2-no-such-distro: wsl --list --quiet shows them";
    let want = format!(
        "cannot read '//wsl.localhost/v2-no-such-distro/x.txt': {no}\ncannot read '//wsl$/v2-no-such-distro/x.txt': {no}\n\
         cannot read '//wsl.localhost': \\\\wsl.localhost needs a WSL distribution's name after it, as in \\\\wsl.localhost\\Ubuntu: wsl --list --quiet shows them\n"
    );
    assert_eq!((code, stdout.as_str()), (Some(0), want.as_str()), "{stderr}");
    std::fs::write(home.0.join("imp.v2"), "import \"//wsl.localhost/v2-no-such-distro/m.v2\" as m\n").unwrap();
    let (code, _, stderr) = run_in_home(&home.0, &["imp.v2"], &[]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.starts_with(&format!("imp.v2:1:1: ImportError: cannot import '//wsl.localhost/v2-no-such-distro/m.v2': {no}\n")), "{stderr}");
    let (code, stdout, stderr) = run_in_home(&home.0, &["check", "imp.v2"], &[]);
    assert_eq!((code, stdout.as_str()), (Some(1), "check imp.v2: 1 file, 1 error\n"), "{stderr}");
    assert!(stderr.contains(&format!("ImportError: cannot import '//wsl.localhost/v2-no-such-distro/m.v2': {no}\n")), "{stderr}");
    let file = r"\\wsl.localhost\v2-no-such-distro\x.v2";
    for args in [vec![file], vec!["check", file], vec![r"\\wsl$\v2-no-such-distro\x.v2"]] {
        let (code, _, stderr) = run_in_home(&home.0, &args, &[]);
        let shown = args.last().unwrap();
        assert_eq!((code, stderr), (Some(2), format!("v2: cannot find '{shown}'\n{no}\n")), "{args:?}");
    }
    let (code, _, stderr) = run_in_home(&home.0, &[r"\\wsl.localhost"], &[]);
    let bare = r"\\wsl.localhost needs a WSL distribution's name after it, as in \\wsl.localhost\Ubuntu: wsl --list --quiet shows them";
    assert_eq!((code, stderr), (Some(2), format!("v2: cannot find '\\\\wsl.localhost'\n{bare}\n")));
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
