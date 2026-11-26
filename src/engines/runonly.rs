//! Run engines and compiled programs as child processes (INTERNALS 15.5, 17.3).
//!
//! A run-engine block is written to a temp file in the run dir and run by its engine's command,
//! once per reach: block args arrive as `V2_ARG_*` variables (17.4), output streams live, and in
//! expression position the block's value is what it printed.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::process::{self, ChildProc, Pipe, Spawn};
use super::resolve::ResolvedEngine;
use crate::runtime::error::{ErrorValue, Exec, Flow};
use crate::runtime::interp::Interp;
use crate::runtime::value::{DictMap, Value};
use crate::syntax::span::Span;

/// The last bytes of a run's stderr that `EngineExitError` keeps (17.3).
const STDERR_TAIL: usize = 4096;

/// Numbers the temp files of a run (`<run>/run/{tag}-{n}.{ext}`).
static NEXT_FILE: AtomicUsize = AtomicUsize::new(1);

fn error(kind: &str, message: String) -> Flow {
    Flow::Error(Rc::new(ErrorValue::new(kind, message)))
}

/// A value's type as 17.4's messages name it, when a run engine can't take it at any depth.
fn unsendable(v: &Value) -> Option<&'static str> {
    match v {
        Value::Bytes(_) => Some("bytes"),
        Value::Func(_) => Some("func"),
        Value::Handle(_) => Some("handle"),
        Value::Error(_) => Some("error"),
        Value::List(l) => l.borrow().iter().find_map(unsendable),
        Value::Dict(d) => d.borrow().values().find_map(unsendable),
        _ => None,
    }
}

fn non_finite(v: &Value) -> bool {
    match v {
        Value::Float(x) => !x.is_finite(),
        Value::List(l) => l.borrow().iter().any(non_finite),
        Value::Dict(d) => d.borrow().values().any(non_finite),
        _ => false,
    }
}

/// 17.4: `V2_ARG_<name>` for each arg; a `str` as is, anything else as compact JSON.
fn arg_vars(args: &[(Rc<str>, Value, Span)]) -> Result<Vec<(OsString, OsString)>, (Flow, Span)> {
    let mut vars = Vec::with_capacity(args.len());
    for (i, (name, value, span)) in args.iter().enumerate() {
        let ok_name = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ok_name {
            return Err((error("ValueError", format!("block arg '{name}' can't be used as an environment variable name")), *span));
        }
        // Windows matches variable names without case.
        if cfg!(windows) {
            if let Some((other, _, _)) = args[..i].iter().find(|(o, _, _)| o.eq_ignore_ascii_case(name)) {
                return Err((
                    error("ValueError", format!("block args '{other}' and '{name}' differ only in case; run engines pass args as environment variables, which are case-insensitive on Windows")),
                    *span,
                ));
            }
        }
        if let Some(kind) = unsendable(value) {
            return Err((error("MarshalError", format!("block arg '{name}': a {kind} can't be passed to a run engine")), *span));
        }
        if non_finite(value) {
            return Err((error("MarshalError", format!("block arg '{name}': NaN and infinity have no JSON form")), *span));
        }
        let text = match value {
            Value::Str(s) => s.to_string(),
            other => crate::codec::json::stringify(other, None).map_err(|f| (f, *span))?,
        };
        if text.contains('\0') {
            return Err((error("MarshalError", format!("block arg '{name}' contains a NUL character")), *span));
        }
        vars.push((OsString::from(format!("V2_ARG_{name}")), OsString::from(text)));
    }
    Ok(vars)
}

/// The inherited `V2_ARG_*` variables, which the child doesn't get (a nested `v2`'s, 17.4).
fn inherited_args() -> Vec<OsString> {
    std::env::vars_os()
        .map(|(k, _)| k)
        .filter(|k| {
            let k = k.to_string_lossy();
            if cfg!(windows) {
                k.len() >= 7 && k[..7].eq_ignore_ascii_case("V2_ARG_")
            } else {
                k.starts_with("V2_ARG_")
            }
        })
        .collect()
}

/// 17.3 step 1–2: the source written to a new file in the run dir.
fn write_source(tag: &str, ext: &str, body: &str) -> Result<PathBuf, String> {
    let dir = crate::config::rundir::run_dir()?.join("run");
    let failed = |e: std::io::Error| format!("cannot run @{tag}: V2 couldn't write the block into {}: {}", dir.display(), crate::syntax::span::os_message(&e));
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(failed(e)),
    }
    let path = dir.join(format!("{tag}-{}.{ext}", NEXT_FILE.fetch_add(1, Ordering::Relaxed)));
    let mut text = body.replace("\r\n", "\n");
    let lower = ext.to_ascii_lowercase();
    // cmd.exe's label search misbehaves in LF files; Windows PowerShell reads a BOM-less script
    // as ANSI. Neither moves a line.
    if lower == "bat" || lower == "cmd" {
        text = text.replace('\n', "\r\n");
    }
    if lower == "ps1" {
        text.insert(0, '\u{feff}');
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut f = options.open(&path).map_err(failed)?;
    f.write_all(text.as_bytes()).map_err(failed)?;
    Ok(path)
}

/// Removes the temp file on every path (17.3 step 6).
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// What a finished run left: its status, its stdout (expression position) and stderr's tail.
struct Finished {
    status: process::ExitInfo,
    stdout: Vec<u8>,
    stderr_tail: Vec<u8>,
    /// More than `STDERR_TAIL` bytes were written.
    stderr_cut: bool,
    timed_out: bool,
}

/// 17.3 steps 3–5: spawn, stream, time out, then kill what is left of the tree and reap.
fn run(argv: &[OsString], env: &[(OsString, OsString)], capture: bool, limit: Option<Duration>) -> Result<Finished, process::SpawnError> {
    let remove = inherited_args();
    let stdout = if capture { Pipe::Piped } else { Pipe::Inherit };
    let mut child = ChildProc::spawn(&Spawn { argv, env, stdin: Pipe::Null, stdout, stderr: Pipe::Piped, remove: &remove, keep: &[], raw_tail: false })?;
    // stderr: copied to V2's stderr as it comes, its last bytes kept.
    let err = child.take_stderr();
    let tee = process::spawn_helper(std::thread::Builder::new(), move || {
        let mut tail: Vec<u8> = Vec::new();
        let mut total = 0usize;
        if let Some(mut pipe) = err {
            let mut chunk = [0u8; 8192];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut host = std::io::stderr().lock();
                        let _ = host.write_all(&chunk[..n]);
                        let _ = host.flush();
                        total += n;
                        tail.extend_from_slice(&chunk[..n]);
                        if tail.len() > STDERR_TAIL {
                            tail.drain(..tail.len() - STDERR_TAIL);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        }
        (tail, total > STDERR_TAIL)
    })
    .expect("start a reader thread");
    let out = child.take_stdout().map(|mut pipe| {
        let kill = child.kill_handle();
        process::spawn_helper(std::thread::Builder::new(), move || {
            let mut bytes = Vec::new();
            if let Ok(false) = process::read_capped(&mut pipe, process::read_cap(), 0, &mut bytes) {
                kill.kill_tree();
            }
            bytes
        })
        .expect("start a reader thread")
    });
    let timer = limit.map(|l| process::push_timer(Some(l), child.kill_handle()));
    child.wait_exit();
    // Background processes the block started stop with it (DOCS 13.1).
    child.kill_tree();
    let timed_out = timer.as_ref().is_some_and(process::TimerGuard::fired);
    drop(timer);
    let stdout = out.map(|h| h.join().unwrap_or_default()).unwrap_or_default();
    let (stderr_tail, stderr_cut) = tee.join().unwrap_or_default();
    let status = child.reap();
    Ok(Finished { status, stdout, stderr_tail, stderr_cut, timed_out })
}

/// Bytes as text, invalid UTF-8 as U+FFFD.
fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

impl Interp {
    /// A run-engine block (17.3), reached: `args` already evaluated, the directive check done.
    pub(crate) fn run_engine_block(&mut self, engine: &Rc<ResolvedEngine>, body: &str, args: &[(Rc<str>, Value, Span)], want_value: bool, at: Span) -> Exec<Value> {
        let tag = engine.spec.id.clone();
        let env = arg_vars(args).map_err(|(f, span)| self.raise(f, span))?;
        let ext = engine.spec.ext.as_deref().unwrap_or(&tag).to_string();
        let path = write_source(&tag, &ext, body).map_err(|m| self.raise(error("IOError", m), at))?;
        let file = TempFile(path);
        let file_text = file.0.as_os_str().to_os_string();
        let argv: Vec<OsString> = self
            .registry
            .run_command(engine)
            .into_iter()
            .map(|a| match a.to_str() {
                Some(s) if s.contains("{file}") => OsString::from(s.replace("{file}", &file_text.to_string_lossy())),
                _ => a,
            })
            .collect();
        let limit = self.call_limit(&tag);
        // Program order: what V2 printed goes out before the block's own output (17.3 step 3).
        self.out.flush_or_stop();
        let ran = run(&argv, &env, want_value, limit);
        process::stop_if_interrupted();
        drop(file);
        let done = match ran {
            Ok(d) => d,
            Err(spawn) => return Err(self.raise(error("EngineNotFound", spawn.message), at)),
        };
        if done.timed_out {
            let secs = limit.map_or(0.0, |l| l.as_secs_f64());
            return Err(self.raise(error("EngineTimeout", format!("@{tag} block ran longer than call_timeout ({secs}s) and was stopped")), at));
        }
        if !done.status.success() {
            let message = match (done.status.code, done.status.signal) {
                (_, Some(_)) => format!("@{tag} block was {}", done.status.describe()),
                (Some(code), None) if cfg!(windows) && code as u32 >= 0x8000_0000 => format!("@{tag} block exited with status 0x{:08X}", code as u32),
                (Some(code), None) => format!("@{tag} block exited with status {code}"),
                (None, None) => format!("@{tag} block ended with an unknown status"),
            };
            // A tail cut out of a longer stderr starts after its first line break (17.3).
            let mut tail = done.stderr_tail.as_slice();
            if done.stderr_cut {
                if let Some(nl) = tail.iter().position(|&b| b == b'\n') {
                    tail = &tail[nl + 1..];
                }
            }
            let mut data = DictMap::new();
            data.insert(Rc::from("status"), done.status.code.map_or(Value::Null, |c| Value::Int(c as i64)));
            data.insert(Rc::from("signal"), done.status.signal.map_or(Value::Null, |s| Value::Int(s as i64)));
            data.insert(Rc::from("stderr"), Value::str(&lossy(tail)));
            let mut e = ErrorValue::new("EngineExitError", message);
            e.data = Value::dict(data);
            e.lang = Some(tag.clone());
            return Err(self.raise(Flow::Error(Rc::new(e)), at));
        }
        if !want_value {
            return Ok(Value::Null);
        }
        let mut text = lossy(&done.stdout);
        if text.ends_with("\r\n") {
            text.truncate(text.len() - 2);
        } else if text.ends_with('\n') {
            text.truncate(text.len() - 1);
        }
        Ok(Value::str(&text))
    }

    /// A run engine's `call_timeout` (18.2): per run, none by default.
    fn call_limit(&self, tag: &str) -> Option<Duration> {
        let secs = match self.registry.canonical(tag).and_then(|s| s.custom.clone()) {
            Some(c) => c.call_timeout,
            None => self.registry.config().engines.get(tag).and_then(|c| c.call_timeout.as_ref()).map(|t| t.value),
        };
        secs.and_then(|s| Duration::try_from_secs_f64(s).ok()).filter(|d| !d.is_zero())
    }
}
