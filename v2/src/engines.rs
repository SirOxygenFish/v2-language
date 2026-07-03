// Embedded language engines: run @py/@js/@bash/... blocks for real.
//
// Two execution modes:
//   * run-only blocks (no @export): the code executes at the statement
//     position in a subprocess with inherited stdio.
//   * exporting blocks: a persistent WORKER subprocess is started. The block
//     code runs once at startup, then the worker answers call requests over a
//     JSON line protocol on stdin/stdout (user prints are rerouted to stderr
//     so they still reach the console without corrupting the protocol).
//
// Values cross the boundary as JSON, so anything JSON-representable
// (numbers, strings, bools, null, lists, dicts) round-trips.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Canonical engine name for a block tag ("py"/"python" -> "python", ...).
pub fn normalize_lang(tag: &str) -> &'static str {
    match tag {
        "py" | "python" => "python",
        "js" | "javascript" => "node",
        "ts" | "typescript" => "ts",
        "lua" => "lua",
        "bash" => "bash",
        "sh" => "sh",
        "ps" | "powershell" => "powershell",
        "os" | "shell" => "shell",
        "go" => "go",
        "c" => "c",
        "cpp" | "cxx" => "cpp",
        "rust" | "rs" => "rust",
        "java" => "java",
        "cs" | "csharp" => "csharp",
        "asm" | "assembly" => "asm",
        "mal" | "malbolge" => "mal",
        "sql" => "sql",
        "wasm" => "wasm",
        _ => "unknown",
    }
}

/// Canonical short tag for selector registration ("python" -> "py").
pub fn canonical_tag(tag: &str) -> &'static str {
    match normalize_lang(tag) {
        "python" => "py",
        "node" => "js",
        other => match other {
            "ts" => "ts",
            "lua" => "lua",
            "bash" => "bash",
            "sh" => "sh",
            "powershell" => "ps",
            "shell" => "os",
            _ => "other",
        },
    }
}

/// Strip the common leading indentation from a block (embedded code is
/// indented to the V2 nesting level; Python cares).
pub fn dedent(code: &str) -> String {
    let mut min_indent = usize::MAX;
    for line in code.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        min_indent = min_indent.min(indent);
    }
    if min_indent == usize::MAX || min_indent == 0 {
        return code.to_string();
    }
    code.lines()
        .map(|l| if l.len() >= min_indent { &l[min_indent..] } else { l.trim_start() })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract V2 interop directives from raw embedded source.
/// Returns (code without directive lines, exported names, wildcard_export).
/// `@export { a, b }` lines are collected; `@import { ... } from ...` lines
/// inside blocks are stripped with a warning (not supported yet).
pub fn extract_directives(code: &str) -> (String, Vec<String>, bool) {
    let mut clean = String::new();
    let mut exports: Vec<String> = Vec::new();
    let mut wildcard = false;
    for line in code.lines() {
        let t = line.trim();
        if t.starts_with("@export") {
            if let (Some(o), Some(c)) = (t.find('{'), t.rfind('}')) {
                if o < c {
                    for part in t[o + 1..c].split(',') {
                        let name = part.trim();
                        if name == "*" {
                            wildcard = true;
                        } else if !name.is_empty() {
                            exports.push(name.to_string());
                        }
                    }
                }
            }
            clean.push('\n'); // keep line numbers stable
            continue;
        }
        if t.starts_with("@import") {
            eprintln!(
                "[warning] @import inside embedded blocks is not supported yet; line skipped"
            );
            clean.push('\n');
            continue;
        }
        clean.push_str(line);
        clean.push('\n');
    }
    (clean, exports, wildcard)
}

fn temp_source_path(ext: &str) -> std::path::PathBuf {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "v2_engine_{}_{}.{}",
        std::process::id(),
        n,
        ext
    ))
}

/// Interpreter command + source-file extension for run-only execution.
fn run_command(lang: &str, file: &str) -> Option<(String, Vec<String>)> {
    match lang {
        "python" => Some(("python".into(), vec![file.into()])),
        "node" => Some(("node".into(), vec![file.into()])),
        "lua" => Some(("lua".into(), vec![file.into()])),
        "bash" => Some(("bash".into(), vec![file.into()])),
        "sh" => Some(("sh".into(), vec![file.into()])),
        "powershell" => Some((
            "powershell".into(),
            vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                file.into(),
            ],
        )),
        "shell" => {
            if cfg!(target_os = "windows") {
                Some(("cmd".into(), vec!["/C".into(), file.into()]))
            } else {
                Some(("sh".into(), vec![file.into()]))
            }
        }
        _ => None,
    }
}

fn file_ext(lang: &str) -> &'static str {
    match lang {
        "python" => "py",
        "node" => "js",
        "lua" => "lua",
        "bash" | "sh" => "sh",
        "powershell" => "ps1",
        "shell" => {
            if cfg!(target_os = "windows") {
                "bat"
            } else {
                "sh"
            }
        }
        _ => "txt",
    }
}

/// Execute a run-only block at the statement position, stdio inherited.
pub fn run_block(lang: &str, code: &str) -> Result<(), String> {
    let (cmd, args, path) = prepare_run(lang, code)?;
    let mut command = Command::new(&cmd);
    command.args(&args);
    if lang == "python" {
        // UTF-8 everywhere — Windows consoles default to legacy code pages.
        command.env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");
    }
    let status = command
        .status()
        .map_err(|e| engine_missing_msg(lang, &cmd, &e.to_string()))?;
    let _ = std::fs::remove_file(&path);
    if !status.success() {
        return Err(format!(
            "@{} block exited with status {}",
            lang,
            status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

fn prepare_run(
    lang: &str,
    code: &str,
) -> Result<(String, Vec<String>, std::path::PathBuf), String> {
    let mut source = dedent(code);
    if lang == "python" {
        // Make local .py files importable from the project directory.
        source = format!(
            "import sys as _s, os as _o\n_s.path.insert(0, _o.getcwd())\n{}",
            source
        );
    }
    if lang == "shell" && cfg!(target_os = "windows") {
        // .bat files echo every command unless told not to.
        source = format!("@echo off\n{}", source);
    }
    let path = temp_source_path(file_ext(lang));
    std::fs::write(&path, &source).map_err(|e| format!("engine temp file: {}", e))?;
    let file = path.to_string_lossy().to_string();
    let (cmd, args) = run_command(lang, &file)
        .ok_or_else(|| format!("@{} blocks cannot be executed yet (no runtime bridge)", lang))?;
    Ok((cmd, args, path))
}

fn engine_missing_msg(lang: &str, cmd: &str, err: &str) -> String {
    format!(
        "@{} block needs '{}' on PATH but it could not be started ({})",
        lang, cmd, err
    )
}

const PY_WORKER_SUFFIX: &str = r#"
def _v2_list_exports():
    _out = []
    for _k, _v in list(globals().items()):
        if _k.startswith('_'):
            continue
        if callable(_v):
            _out.append(_k)
    return sorted(_out)
_proto.write(_json.dumps({"ready": True, "exports": _v2_list_exports()}, ensure_ascii=False) + "\n")
_proto.flush()
for _line in _sys.stdin:
    _line = _line.strip()
    if not _line:
        continue
    try:
        _req = _json.loads(_line)
        if _req.get("op") == "exit":
            break
        _fn = globals().get(_req["fn"])
        if _fn is None or not callable(_fn):
            raise Exception("no such function: " + str(_req["fn"]))
        _res = _fn(*_req.get("args", []))
        _proto.write(_json.dumps({"ok": _res}, ensure_ascii=False) + "\n")
    except Exception as _e:
        _proto.write(_json.dumps({"error": str(_e)}, ensure_ascii=False) + "\n")
    _proto.flush()
"#;

const JS_WORKER_SUFFIX: &str = r#"
_proto(JSON.stringify({ready: true, exports: []}) + '\n');
const _rl = require('readline').createInterface({ input: process.stdin, terminal: false });
_rl.on('line', (_line) => {
    _line = _line.trim();
    if (!_line) return;
    let _req;
    try { _req = JSON.parse(_line); } catch (_e) { _proto(JSON.stringify({error: String(_e)}) + '\n'); return; }
    if (_req.op === 'exit') { process.exit(0); }
    try {
        const _f = eval(_req.fn);
        if (typeof _f !== 'function') throw new Error('no such function: ' + _req.fn);
        const _res = _f(...(_req.args || []));
        _proto(JSON.stringify({ok: _res === undefined ? null : _res}) + '\n');
    } catch (_e) {
        _proto(JSON.stringify({error: String((_e && _e.message) || _e)}) + '\n');
    }
});
"#;

/// A persistent engine subprocess answering call requests over JSON lines.
pub struct EngineWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// Callable names the worker announced at startup (python enumerates its
    /// globals; node reports none and relies on explicit @export lists).
    pub announced_exports: Vec<String>,
    pub lang: String,
    source_path: std::path::PathBuf,
}

impl EngineWorker {
    /// Start a worker: block code runs once, then the protocol loop serves calls.
    pub fn start(lang: &str, user_code: &str) -> Result<EngineWorker, String> {
        let user_code = dedent(user_code);
        let (source, cmd, args_of): (String, String, fn(&str) -> Vec<String>) = match lang {
            "python" => (
                format!(
                    "import sys as _sys, json as _json, os as _os\n\
                     _proto = _sys.stdout\n\
                     _sys.stdout = _sys.stderr\n\
                     _sys.path.insert(0, _os.getcwd())\n\
                     {}\n{}",
                    user_code, PY_WORKER_SUFFIX
                ),
                "python".into(),
                |f| vec![f.to_string()],
            ),
            "node" => (
                format!(
                    "const _proto = process.stdout.write.bind(process.stdout);\n\
                     console.log = (...a) => process.stderr.write(a.map(x => (typeof x === 'object' && x !== null) ? JSON.stringify(x) : String(x)).join(' ') + '\\n');\n\
                     {}\n{}",
                    user_code, JS_WORKER_SUFFIX
                ),
                "node".into(),
                |f| vec![f.to_string()],
            ),
            other => {
                return Err(format!(
                    "@export is only supported in @py and @js blocks (got @{})",
                    other
                ))
            }
        };

        let path = temp_source_path(file_ext(lang));
        std::fs::write(&path, &source).map_err(|e| format!("engine temp file: {}", e))?;
        let file = path.to_string_lossy().to_string();

        let mut command = Command::new(&cmd);
        command
            .args(args_of(&file))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        // stderr inherited: user prints + tracebacks reach the console
        if lang == "python" {
            // UTF-8 everywhere — Windows defaults to legacy code pages.
            command.env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");
        }
        let mut child = command
            .spawn()
            .map_err(|e| engine_missing_msg(lang, &cmd, &e.to_string()))?;

        let stdin = child.stdin.take().ok_or("engine stdin unavailable")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("engine stdout unavailable")?);
        let mut worker = EngineWorker {
            child,
            stdin,
            stdout,
            announced_exports: Vec::new(),
            lang: lang.to_string(),
            source_path: path,
        };

        // The ready line arrives after the block's top-level code has run.
        let ready = worker.read_line()?;
        if let Some(list) = extract_json_string_array(&ready, "exports") {
            worker.announced_exports = list;
        }
        Ok(worker)
    }

    fn read_line(&mut self) -> Result<String, String> {
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .map_err(|e| format!("engine read failed: {}", e))?;
        if n == 0 {
            return Err(format!(
                "@{} engine exited unexpectedly (see output above)",
                self.lang
            ));
        }
        Ok(line.trim().to_string())
    }

    /// Call `fn_name` with a pre-serialized JSON args array; returns the raw
    /// JSON response line ({"ok": ...} or {"error": ...}).
    pub fn call_json(&mut self, fn_name: &str, args_json: &str) -> Result<String, String> {
        let request = format!(
            "{{\"fn\": {}, \"args\": {}}}\n",
            json_quote(fn_name),
            args_json
        );
        self.stdin
            .write_all(request.as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|e| format!("engine write failed: {}", e))?;
        self.read_line()
    }
}

impl Drop for EngineWorker {
    fn drop(&mut self) {
        let _ = self.stdin.write_all(b"{\"op\": \"exit\"}\n");
        let _ = self.stdin.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.source_path);
    }
}

fn json_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Pull `"key": ["a", "b", ...]` out of a one-line JSON object (just enough
/// for the ready handshake — full JSON parsing happens in the interpreter).
fn extract_json_string_array(json: &str, key: &str) -> Option<Vec<String>> {
    let key_pat = format!("\"{}\"", key);
    let start = json.find(&key_pat)?;
    let rest = &json[start + key_pat.len()..];
    let open = rest.find('[')?;
    let close = rest.find(']')?;
    if close < open {
        return None;
    }
    let inner = &rest[open + 1..close];
    let mut out = Vec::new();
    for part in inner.split(',') {
        let t = part.trim().trim_matches('"');
        if !t.is_empty() {
            out.push(t.to_string());
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dedent() {
        assert_eq!(dedent("    a\n    b"), "a\nb");
        assert_eq!(dedent("    a\n        b"), "a\n    b");
        assert_eq!(dedent("a\nb"), "a\nb");
    }

    #[test]
    fn test_extract_directives() {
        let (clean, exports, wild) =
            extract_directives("def f(x):\n    return x\n@export { f, g }\n");
        assert!(clean.contains("def f(x):"));
        assert!(!clean.contains("@export"));
        assert_eq!(exports, vec!["f", "g"]);
        assert!(!wild);
        let (_, e2, w2) = extract_directives("@export { * }");
        assert!(e2.is_empty());
        assert!(w2);
    }

    #[test]
    fn test_json_quote() {
        assert_eq!(json_quote("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn test_extract_json_string_array() {
        let v = extract_json_string_array(
            "{\"ready\": true, \"exports\": [\"a\", \"b\"]}",
            "exports",
        )
        .unwrap();
        assert_eq!(v, vec!["a", "b"]);
    }
}
