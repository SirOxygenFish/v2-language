//! Engine sessions: one worker process per bridged engine for the whole run, its state machine,
//! the handshake, the re-entrant request loop and shutdown (INTERNALS ch. 8, 9.7-9.10). The
//! child-process rules and the watchdog are in `engines/process`; frames in `engines/vep`;
//! values in `codec/vep`.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value as Json};

use super::handles::{CallableTable, ReleaseQueue};
use super::process::{self, ChildProc, ExitInfo, Pipe, Spawn, TimerGuard};
use super::resolve::ResolvedEngine;
use super::spec::Version;
use super::vep::{self, Caps, Frame, FrameReader, Hello, ReadErr, Stream, Target, WriteErr};
use crate::codec::vep::Wire;
use crate::runtime::error::{fail, ErrorValue, Exec, Flow};
use crate::runtime::interp::Interp;
use crate::runtime::value::{Callable, Value};
use crate::syntax::span::Span;

pub type SessionId = usize;

/// Cross-language crossings in progress at most, across all engines (9.7, DOCS 8.4).
pub const MAX_XDEPTH: u32 = 64;

/// The drain after `bye`, shared by every session (8.7).
const GRACE: Duration = Duration::from_secs(1);

/// Why a session stopped (8.1).
#[derive(Debug)]
pub enum DeadReason {
    SpawnFailed(Rc<ErrorValue>),
    StartupTimeout(Duration),
    CallTimeout(Duration),
    Protocol,
    Exited { exit: Option<ExitInfo>, during: Option<Rc<str>>, at: Option<Span> },
}

/// 8.1's states after spawning (a session is created when its worker starts).
#[derive(Debug)]
pub enum State {
    Handshaking,
    Ready,
    Busy(u32),
    Closing,
    Closed,
    Dead(DeadReason),
}

pub struct Timeouts {
    pub startup: Option<Duration>,
    pub call: Option<Duration>,
}

/// A host request awaiting its reply (9.7).
struct Pending {
    id: u64,
    /// What the request does, for messages: `a call to 'ratio'`.
    name: Rc<str>,
    timer: Option<TimerGuard>,
    startup: bool,
}

pub struct Session {
    /// The engine's tag: `py`, `js` (which also runs ts).
    pub key: Rc<str>,
    pub engine: Rc<ResolvedEngine>,
    pub state: State,
    child: Option<ChildProc>,
    reader: Option<FrameReader<Box<dyn Read + Send>>>,
    writer: Option<Box<dyn Write>>,
    pub hello: Option<Hello>,
    pub(crate) caps: Caps,
    next_id: u64,
    pending: Vec<Pending>,
    pub releases: ReleaseQueue,
    pub callables: CallableTable,
    timeouts: Timeouts,
    /// The V2 errors sent into this worker during the outermost request, by their tag's `ref`,
    /// so one that comes back gets its own data and cause, not the worker's copy (9.6).
    sent_errors: Vec<(u64, Rc<ErrorValue>)>,
    next_ref: u64,
}

impl Session {
    /// Kill the worker's tree and reap it (8.3, 8.6).
    fn stop(&mut self) -> Option<ExitInfo> {
        self.reader = None;
        self.writer = None;
        self.pending.clear();
        self.child.take().map(|c| {
            c.kill_tree();
            c.reap()
        })
    }
}

/// A `ret` (9.6): its decoded value, and for `load`/`module` the export lists (9.4): each
/// export's name with its constant (`None` for a function), and the names it lacks.
pub struct Reply {
    pub value: Value,
    pub exports: Vec<(Rc<str>, Option<Value>)>,
    pub missing: Vec<Rc<str>>,
    /// A module's public names, when it lacks a name asked for: for the did-you-mean (9.4).
    pub names: Vec<Rc<str>>,
}

/// Most `cause` levels an `err` is read to (9.6).
const MAX_CAUSES: usize = 8;

/// A worker's argv and environment (8.3, 14.1): the resolved argv, the spec's fixed arguments
/// with `{adapter}` filled in, and the spec's environment on top of the host's (a `NODE_PATH`
/// the user has is inherited untouched).
pub fn worker_command(e: &ResolvedEngine, adapter: &Path) -> (Vec<OsString>, Vec<(OsString, OsString)>) {
    let mut argv = e.argv.clone();
    // A Node that strips types only when asked: the .ts files an @import names load too. Asked,
    // it warns at startup, before the adapter can drop that warning (7.2).
    if &*e.spec.id == "js" && e.version.is_some_and(super::spec::node_needs_strip_flag) {
        argv.push("--experimental-strip-types".into());
        argv.push("--disable-warning=ExperimentalWarning".into());
    }
    for a in &e.spec.fixed_args {
        argv.push(if &**a == "{adapter}" { adapter.as_os_str().to_os_string() } else { OsString::from(&**a) });
    }
    let env = e.spec.env.iter().map(|(k, v)| (OsString::from(&**k), OsString::from(&**v))).collect();
    (argv, env)
}

/// `5 s`, `0.3 s`.
fn secs(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s.fract() == 0.0 {
        format!("{s:.0} s")
    } else {
        format!("{s} s")
    }
}

/// `exited with code 1`, `was killed by signal 11 (SIGSEGV)` (8.2).
fn exit_phrase(exit: Option<&ExitInfo>) -> String {
    match exit {
        Some(e) if e.signal.is_some() => format!("was {}", e.describe()),
        Some(e) => format!("exited with {}", e.describe().trim_start_matches("exit ")),
        None => "exited".into(),
    }
}

/// What to do about a value that can't cross (10.1, 10.4): a MarshalError's hint, from its
/// message and the language it was in.
fn marshal_hint(message: &str, lang: &str) -> Option<&'static str> {
    if message.starts_with("value too large to send") {
        return Some("write it to a file and pass the file's path, or send it in parts");
    }
    // A pandas Series indexed by dates, a dict keyed by tuples: V2's keys are strings.
    if message.starts_with("to_v2(): can't convert") && message.contains("(key of type ") {
        return Some(match lang {
            "py" => "make its keys strings in the block, as in @py(h) { {str(k): v for k, v in h.items()} }",
            _ => "make its keys strings in the block, as in @js(h) { Object.fromEntries([...h].map(([k, v]) => [String(k), v])) }",
        });
    }
    let code = message.split("unpaired surrogate (U+").nth(1)?.get(..4)?;
    let escaped = u32::from_str_radix(code, 16).is_ok_and(|c| (0xDC80..=0xDCFF).contains(&c));
    Some(match lang {
        // What Python's surrogateescape makes of bytes that aren't UTF-8.
        "py" if escaped => {
            "the text holds bytes that aren't UTF-8, which Python keeps as U+DC80 to U+DCFF (file names, environment variables and program output can): send it as bytes, os.fsencode(s), or with those bytes replaced, s.encode(\"utf-8\", \"surrogateescape\").decode(\"utf-8\", \"replace\")"
        }
        "py" => "replace it in the block: s.encode(\"utf-8\", \"replace\").decode()",
        _ => "replace it in the block: s.toWellFormed()",
    })
}

/// What to do about a foreign exception that V2's setup causes (12.6): blocks run Python in its
/// UTF-8 mode (8.4), so `open()` reads UTF-8 where a plain Windows Python reads the ANSI code page.
fn foreign_hint(kind: &str, message: &str, lang: &str) -> Option<&'static str> {
    match (lang, kind) {
        ("py", "UnicodeDecodeError") if message.starts_with("'utf-8' codec can't decode") => {
            Some("blocks run Python in UTF-8 mode, so open() reads UTF-8: for a file in another encoding, name it, as in open(path, encoding=\"cp1252\")")
        }
        _ => None,
    }
}

/// JavaScript calls a callback with all it has (`xs.map(f)` passes the item, its index and the
/// array) and a JS function ignores the extras: so does a V2 function called from JavaScript,
/// passed in or imported with `from v2` (DOCS 8.2).
pub(crate) fn drop_js_extras(f: &Value, pos: &mut Vec<Value>) {
    if let Value::Func(func) = f {
        if let Callable::Closure { decl, .. } = &**func {
            if decl.rest.is_none() && pos.len() > decl.params.len() {
                pos.truncate(decl.params.len());
            }
        }
    }
}

/// An error at no location yet: `raise` places it at the V2 expression.
fn error(kind: &str, message: String) -> Flow {
    match fail::<()>(kind, message) {
        Err(f) => f,
        Ok(()) => unreachable!(),
    }
}

fn depth_error() -> Flow {
    error("RecursionError", format!("cross-language call depth limit ({MAX_XDEPTH}) exceeded"))
}

impl Interp {
    /// Start a worker with `argv`/`env` and shake hands with it (8.3, 8.4).
    pub fn start_session(
        &mut self,
        engine: Rc<ResolvedEngine>,
        argv: &[OsString],
        env: &[(OsString, OsString)],
        timeouts: Timeouts,
        at: Span,
    ) -> Exec<SessionId> {
        self.check_depth(at)?;
        let key = engine.spec.session_of.clone().unwrap_or_else(|| engine.spec.id.clone());
        // Unix, for an adapter without `dup2`: the protocol on two pipes of its own (9.9).
        let fds = (cfg!(unix) && engine.spec.protocol_fds).then(|| Ok::<_, std::io::Error>((std::io::pipe()?, std::io::pipe()?))).transpose();
        // No pipes is a worker that couldn't start.
        let (fds, no_pipes) = match fds {
            Ok(fds) => (fds, None),
            Err(e) => (None, Some(e)),
        };
        #[cfg(unix)]
        let keep: Vec<i32> = fds.iter().flat_map(|((to_worker, _), (_, from_worker))| {
            use std::os::fd::AsRawFd;
            [to_worker.as_raw_fd(), from_worker.as_raw_fd()]
        }).collect();
        #[cfg(not(unix))]
        let keep: Vec<i32> = Vec::new();
        let mut env = env.to_vec();
        if let [to_worker, from_worker] = keep[..] {
            env.push(("V2_VEP_FDS".into(), format!("{to_worker},{from_worker}").into()));
        }
        let (stdin, stdout) = if fds.is_some() { (Pipe::Null, Pipe::Stderr) } else { (Pipe::Piped, Pipe::Piped) };
        let spawned = match no_pipes {
            Some(e) => {
                let reason = crate::syntax::span::os_message(&e);
                Err(process::SpawnError { message: format!("could not start '{}': {reason}", argv[0].to_string_lossy()), reason })
            }
            None => ChildProc::spawn(&Spawn { argv, env: &env, stdin, stdout, stderr: Pipe::Inherit, keep: &keep, raw_tail: false }),
        };
        let mut child = match spawned {
            Ok(c) => c,
            Err(spawn) => {
                let e = ErrorValue::new("EngineNotFound", spawn.message);
                // EMFILE: the program is there; the open-files limit stops V2 from starting it.
                if spawn.reason.contains("Too many open files") {
                    let _ = e.hint.set("the open-files limit (ulimit -n) is too low for V2 and its workers: raise it, as in ulimit -n 1024".into());
                }
                let e = Rc::new(e);
                let s = self.new_session(key, engine, None, None, None, timeouts);
                self.sessions[s].state = State::Dead(DeadReason::SpawnFailed(e.clone()));
                return Err(self.raise(Flow::Error(e), at));
            }
        };
        let (reader, writer): (Box<dyn Read + Send>, Box<dyn Write>) = match fds {
            // The child's ends close here, so the host sees the worker's end of input.
            Some(((to_worker, writer), (reader, from_worker))) => {
                drop((to_worker, from_worker));
                #[cfg(unix)]
                let reader = crate::engines::process::UntilExit::new(reader, child.kill_handle());
                (Box::new(reader), Box::new(writer))
            }
            None => {
                let pipe = child.take_stdout().expect("piped stdout");
                #[cfg(unix)]
                let pipe = crate::engines::process::UntilExit::new(pipe, child.kill_handle());
                (Box::new(pipe), Box::new(child.take_stdin().expect("piped stdin")))
            }
        };
        let parkable = timeouts.call.is_none().then(|| child.kill_handle());
        let s = self.new_session(key, engine, Some(reader), Some(writer), Some(child), timeouts);
        self.handshake(s, at)?;
        // Not with a call_timeout: bringing the memory back would count against it (8.8).
        if let Some(kill) = parkable {
            process::allow_parking(kill);
        }
        Ok(s)
    }

    /// A worker isn't started at the depth limit, where `init` would be one crossing too many (8.4).
    pub(crate) fn check_depth(&self, at: Span) -> Exec<()> {
        if self.xdepth >= MAX_XDEPTH {
            return Err(self.raise(depth_error(), at));
        }
        Ok(())
    }

    /// A session over any channel, before its handshake (the tests' scripted workers).
    #[cfg(test)]
    pub fn attach_session(
        &mut self,
        engine: Rc<ResolvedEngine>,
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write>,
        child: Option<ChildProc>,
        timeouts: Timeouts,
    ) -> SessionId {
        let key = engine.spec.session_of.clone().unwrap_or_else(|| engine.spec.id.clone());
        self.new_session(key, engine, Some(reader), Some(writer), child, timeouts)
    }

    fn new_session(
        &mut self,
        key: Rc<str>,
        engine: Rc<ResolvedEngine>,
        reader: Option<Box<dyn Read + Send>>,
        writer: Option<Box<dyn Write>>,
        child: Option<ChildProc>,
        timeouts: Timeouts,
    ) -> SessionId {
        self.sessions.push(Session {
            key,
            engine,
            state: State::Handshaking,
            child,
            reader: reader.map(FrameReader::new),
            writer,
            hello: None,
            caps: Caps::default(),
            next_id: 0,
            pending: Vec::new(),
            releases: Rc::default(),
            callables: CallableTable::default(),
            timeouts,
            sent_errors: Vec::new(),
            next_ref: 0,
        });
        self.sessions.len() - 1
    }

    fn tag(&self, s: SessionId) -> Rc<str> {
        self.sessions[s].key.clone()
    }

    fn timer(&self, s: SessionId, limit: Option<Duration>) -> Option<TimerGuard> {
        self.sessions[s].child.as_ref().map(|c| process::push_timer(limit, c.kill_handle()))
    }

    /// Kill the worker and record why (9.11, 8.6).
    fn kill_session(&mut self, s: SessionId, reason: DeadReason) -> Option<ExitInfo> {
        let exit = self.sessions[s].stop();
        self.sessions[s].state = State::Dead(reason);
        exit
    }

    fn protocol_violation(&mut self, s: SessionId, what: &str, got: &[u8]) -> Flow {
        let msg = vep::protocol_error(&self.tag(s), what, got);
        self.kill_session(s, DeadReason::Protocol);
        error("ProtocolError", msg)
    }

    /// The worker's stdout ended, or a write to it failed: an interrupt (not reported), a timer
    /// that killed it, or a crash (8.5, 8.6).
    fn worker_gone(&mut self, s: SessionId, pending: &Pending, at: Span) -> Flow {
        process::stop_if_interrupted();
        let tag = self.tag(s);
        let fired = pending.timer.as_ref().is_some_and(TimerGuard::fired);
        let limit = if pending.startup { self.sessions[s].timeouts.startup } else { self.sessions[s].timeouts.call };
        if fired {
            let limit = limit.unwrap_or_default();
            if pending.startup {
                let path = self.sessions[s].engine.argv[0].to_string_lossy().into_owned();
                self.kill_session(s, DeadReason::StartupTimeout(limit));
                return error(
                    "EngineTimeout",
                    format!("engine '{tag}' did not start within {} ({path}); raise [engines.{tag}] startup_timeout if it is only slow", secs(limit)),
                );
            }
            self.kill_session(s, DeadReason::CallTimeout(limit));
            return error(
                "EngineTimeout",
                format!("{} in engine '{tag}' exceeded call_timeout ({}); the engine was stopped", pending.name, secs(limit)),
            );
        }
        let pid = self.sessions[s].hello.as_ref().map(|h| h.pid);
        let exit = self.sessions[s].stop();
        let during = (!pending.startup).then(|| pending.name.clone());
        self.sessions[s].state = State::Dead(DeadReason::Exited { exit, during, at: Some(at) });
        let how = exit.map_or_else(|| "an unknown exit status".to_string(), |e| e.describe());
        if pending.startup {
            if cfg!(windows) && exit.is_some_and(|e| e.code == Some(9009)) {
                let path = self.sessions[s].engine.argv[0].to_string_lossy().into_owned();
                let e = Rc::new(ErrorValue::new(
                    "EngineNotFound",
                    format!("engine '{tag}' could not start: {path} is the Microsoft Store placeholder (exit code 9009)"),
                ));
                self.sessions[s].state = State::Dead(DeadReason::SpawnFailed(e.clone()));
                return Flow::Error(e);
            }
            // A variable the program reads as it starts may be why (8.4).
            let id = self.sessions[s].engine.spec.id.clone();
            let vars: String = super::spec::startup_variables(&id).iter().map(|(n, v)| format!("; {n} is set ({v}), which it reads as it starts")).collect();
            // Node.js reserves about 1 GB of address space as it starts (round 16).
            #[cfg(unix)]
            let limit = crate::address_limit()
                .map(|l| format!("; a memory limit (ulimit -v) of {} MB is set for this shell, and {} as it starts: raise it", l >> 20, if &*id == "js" { "Node.js reserves about 1 GB" } else { "the engine may need more" }))
                .unwrap_or_default();
            #[cfg(not(unix))]
            let limit = "";
            return error("EngineCrashed", format!("engine '{tag}' exited during startup ({how}); see its error output above, if any{vars}{limit}"));
        }
        let pid = pid.map_or_else(String::new, |p| format!(" (pid {p})"));
        error("EngineCrashed", format!("engine '{tag}'{pid} exited during {} ({how}); see its error output above, if any", pending.name))
    }

    /// 8.4: preload noise to stderr, `hello` (VEP/1, caps, version), then `init`, all within
    /// `startup_timeout`.
    pub(crate) fn handshake(&mut self, s: SessionId, at: Span) -> Exec<()> {
        let tag = self.tag(s);
        let timer = self.timer(s, self.sessions[s].timeouts.startup);
        let startup = Pending { id: 0, name: "startup".into(), timer, startup: true };
        let hello = loop {
            let line = match self.sessions[s].reader.as_mut().map(FrameReader::read_line) {
                Some(Ok(Some(line))) => line,
                Some(Err(ReadErr::TooLong)) => {
                    self.kill_session(s, DeadReason::Protocol);
                    return Err(self.raise(error("ProtocolError", format!("frame from engine '{tag}' exceeds 64 MiB")), at));
                }
                _ => {
                    let flow = self.worker_gone(s, &startup, at);
                    return Err(self.raise(flow, at));
                }
            };
            let (noise, frame) = vep::split_noise(&line);
            if !noise.is_empty() {
                self.out.flush_or_stop();
                stderrln!("{}", String::from_utf8_lossy(noise));
            }
            if frame.is_empty() {
                continue;
            }
            match vep::parse_hello(frame) {
                Ok(h) => break h,
                Err(what) => {
                    let t = serde_json::from_slice::<Json>(frame).ok().and_then(|j| j.get("t").and_then(Json::as_str).map(str::to_string));
                    let flow = match t {
                        Some(t) if t != "hello" => {
                            self.kill_session(s, DeadReason::Protocol);
                            error("ProtocolError", format!("engine '{tag}' sent '{t}' before hello"))
                        }
                        // 9.3: `engine 'py' speaks VEP/2; this V2 speaks VEP/1`.
                        _ if what.starts_with("speaks VEP/") => {
                            self.kill_session(s, DeadReason::Protocol);
                            error("ProtocolError", format!("engine '{tag}' {what}"))
                        }
                        _ => self.protocol_violation(s, &what, frame),
                    };
                    return Err(self.raise(flow, at));
                }
            }
        };
        // A launcher or alias may run another interpreter than the one probed (8.4). A session a ts
        // block started is held to js's minimum.
        let spec = self.sessions[s].engine.spec.clone();
        let spec = spec.session_of.as_deref().and_then(|o| self.registry.canonical(o).cloned()).unwrap_or(spec);
        if let (Some(min), Some(v)) = (spec.min_version, Version::parse(&hello.version)) {
            if spec.too_old(v).is_some() {
                let path = self.sessions[s].engine.argv[0].to_string_lossy().into_owned();
                let e = Rc::new(ErrorValue::new(
                    "EngineNotFound",
                    format!("{} {v} at {path} is too old (needs {})", spec.language, spec.needs(min)),
                ));
                self.kill_session(s, DeadReason::SpawnFailed(e.clone()));
                return Err(self.raise(Flow::Error(e), at));
            }
        }
        self.sessions[s].caps = hello.caps;
        self.sessions[s].hello = Some(hello);
        // Canonical, like the `.v2` paths in `load` frames, so a worker sees one spelling of it (2.1, 9.3).
        let root = &self.registry.config().root;
        let root = std::fs::canonicalize(root).map_or_else(|_| root.clone(), |c| crate::syntax::span::strip_verbatim(&c));
        let root = root.to_string_lossy().into_owned();
        // The init request would be one crossing too many: the worker never gets going.
        if self.xdepth >= MAX_XDEPTH {
            drop(startup);
            self.kill_session(s, DeadReason::Exited { exit: None, during: None, at: Some(at) });
            return Err(self.raise(depth_error(), at));
        }
        let init = vep::init(0, &root);
        match self.exchange(s, init, startup, at) {
            Ok(_) => {
                self.sessions[s].state = State::Ready;
                Ok(())
            }
            // The host already stopped it (timeout, crash, protocol): its error stands.
            Err(f) if matches!(self.sessions[s].state, State::Dead(_)) => Err(f),
            // `init` answered with `err`, whatever its type (8.4).
            Err(Flow::Error(e)) => {
                self.kill_session(s, DeadReason::Exited { exit: None, during: None, at: Some(at) });
                let flow = error("EngineCrashed", format!("engine '{tag}' failed to start: {}", e.message));
                Err(self.raise(flow, at))
            }
            Err(f) => {
                self.kill_session(s, DeadReason::Exited { exit: None, during: None, at: Some(at) });
                Err(f)
            }
        }
    }

    /// 8.2's error for a session that has stopped; `None` while it runs.
    fn not_running(&self, s: SessionId) -> Option<Flow> {
        let tag = self.tag(s);
        let why = match &self.sessions[s].state {
            State::Ready | State::Busy(_) => return None,
            State::Dead(DeadReason::SpawnFailed(e)) => return Some(Flow::Error(Rc::new(ErrorValue::new(&e.kind, e.message.clone())))),
            State::Dead(DeadReason::Exited { exit, during, at: place }) => {
                let mut w = exit_phrase(exit.as_ref());
                if let Some(d) = during {
                    w.push_str(&format!(" during {d}"));
                }
                if let Some(a) = place {
                    w.push_str(&format!(" at {}:{}", self.sm.file(a.file).display, a.line));
                }
                Some(w)
            }
            State::Dead(DeadReason::CallTimeout(d)) => Some(format!("exceeded call_timeout ({})", secs(*d))),
            State::Dead(DeadReason::StartupTimeout(d)) => Some(format!("did not start within {}", secs(*d))),
            State::Dead(DeadReason::Protocol) => Some("broke the protocol".into()),
            State::Handshaking | State::Closing | State::Closed => Some("shut down".into()),
        };
        why.map(|why| error("EngineCrashed", format!("engine '{tag}' is not running (stopped earlier: {why}); engines are not restarted")))
    }

    /// The use of a session that is not running (8.2, 8.6).
    pub(crate) fn check_alive(&mut self, s: SessionId, at: Span) -> Exec<()> {
        let tag = self.tag(s);
        if let Some(flow) = self.not_running(s) {
            return Err(self.raise(flow, at));
        }
        // A death while idle is noticed at the next use (8.6).
        let idle_exit = if self.sessions[s].pending.is_empty() { self.sessions[s].child.as_ref().and_then(ChildProc::exited) } else { None };
        if let Some(exit) = idle_exit {
            process::stop_if_interrupted();
            self.kill_session(s, DeadReason::Exited { exit: Some(exit), during: None, at: Some(at) });
            let msg = format!("engine '{tag}' exited while idle ({})", exit.describe());
            return Err(self.raise(error("EngineCrashed", msg), at));
        }
        Ok(())
    }

    /// Send one frame, the session's queued releases first (9.10): written before it, they can't
    /// fill the pipe while the worker writes its answer. A release for a handle the frame names,
    /// which may be its last holder, waits for a later frame (11.2). A write error is the worker
    /// going away (9.1).
    fn send(&mut self, s: SessionId, frame: &Json) -> Result<(), WriteErr> {
        self.out.flush_or_stop();
        let session = &mut self.sessions[s];
        let Some(w) = session.writer.as_mut() else { return Err(WriteErr::Io(std::io::ErrorKind::BrokenPipe.into())) };
        if session.caps.handles {
            let queued = std::mem::take(&mut *session.releases.borrow_mut());
            if !queued.is_empty() {
                let named = crate::codec::vep::handle_ids(frame);
                let (later, now): (Vec<u64>, Vec<u64>) = queued.into_iter().partition(|id| named.contains(id));
                session.releases.borrow_mut().extend(later);
                if !now.is_empty() {
                    vep::write_frame(w.as_mut(), &vep::release(&now))?;
                }
            }
        }
        let sent = vep::write_frame(w.as_mut(), frame);
        if let Err(WriteErr::TooBig(_)) = sent {
            // Nothing went out, so the `fn` tags it carried were never sent (10.1, 11.8).
            session.callables.release(&crate::codec::vep::fn_ids(frame));
        }
        sent
    }

    /// A request to the worker (`load`, `module`, `call`, `op`) and its reply, serving the
    /// worker's calls into V2 meanwhile (9.7). `name` describes it for messages.
    pub fn request(&mut self, s: SessionId, frame: Json, name: &str, at: Span) -> Exec<Reply> {
        if let Err(flow) = self.check_alive(s, at) {
            self.sessions[s].callables.release(&crate::codec::vep::fn_ids(&frame));
            return Err(flow);
        }
        let timer = self.timer(s, self.sessions[s].timeouts.call);
        let pending = Pending { id: 0, name: name.into(), timer, startup: false };
        let reply = self.exchange(s, frame, pending, at);
        // A signal that came while the worker answered: the next statement doesn't run (6.5).
        process::stop_if_interrupted();
        reply
    }

    fn exchange(&mut self, s: SessionId, mut frame: Json, mut pending: Pending, at: Span) -> Exec<Reply> {
        if self.xdepth >= MAX_XDEPTH {
            self.sessions[s].callables.release(&crate::codec::vep::fn_ids(&frame));
            return Err(self.raise(depth_error(), at));
        }
        let session = &mut self.sessions[s];
        session.next_id += 1;
        let id = session.next_id;
        frame["id"] = json!(id);
        pending.id = id;
        if let Err(e) = self.send(s, &frame) {
            let flow = match e {
                WriteErr::TooBig(n) => {
                    let message = vep::too_big_message(&self.tag(s), n);
                    let flow = error("MarshalError", message.clone());
                    if let (Flow::Error(e), Some(hint)) = (&flow, marshal_hint(&message, &self.tag(s))) {
                        let _ = e.hint.set(hint.into());
                    }
                    flow
                }
                WriteErr::Io(_) => self.worker_gone(s, &pending, at),
            };
            return Err(self.raise(flow, at));
        }
        self.xdepth += 1;
        let session = &mut self.sessions[s];
        match session.state {
            State::Busy(n) => session.state = State::Busy(n + 1),
            State::Ready => session.state = State::Busy(1),
            _ => {}
        }
        session.pending.push(pending);
        let result = self.await_reply(s, id, at);
        self.xdepth -= 1;
        let session = &mut self.sessions[s];
        if let Some(i) = session.pending.iter().position(|p| p.id == id) {
            session.pending.truncate(i);
        }
        if let State::Busy(n) = session.state {
            session.state = if n <= 1 { State::Ready } else { State::Busy(n - 1) };
            if n <= 1 {
                session.sent_errors.clear();
            }
        }
        result
    }

    pub(crate) fn wire(&mut self, s: SessionId) -> Wire<'_> {
        let session = &mut self.sessions[s];
        Wire {
            session: s,
            tag: session.key.clone(),
            caps: session.caps,
            releases: &session.releases,
            callables: &mut session.callables,
            sm: &self.sm,
            routes: &self.routes,
        }
    }

    fn await_reply(&mut self, s: SessionId, id: u64, at: Span) -> Exec<Reply> {
        let mut unwritten = None; // this request's output that couldn't be written (9.9)
        loop {
            // Every frame sent so far is handled: what the worker printed goes out before V2
            // waits, so a file or a pipe gets it as it comes (9.9). A failure here may be an
            // earlier request's output, so it waits for the next write, as at other flushes.
            if !self.sessions[s].reader.as_ref().is_some_and(|r| r.has_buffered()) {
                self.out.flush_or_stop();
            }
            let read = match self.sessions[s].reader.as_mut() {
                Some(r) => r.read_line(),
                None => Ok(None),
            };
            let line = match read {
                Ok(Some(line)) => line,
                Err(ReadErr::TooLong) => {
                    let msg = format!("frame from engine '{}' exceeds 64 MiB", self.tag(s));
                    self.kill_session(s, DeadReason::Protocol);
                    return Err(self.raise(error("ProtocolError", msg), at));
                }
                Ok(None) | Err(ReadErr::Io(_)) => {
                    let flow = self.gone_now(s, at);
                    return Err(self.raise(flow, at));
                }
            };
            let caps = self.sessions[s].caps;
            let frame = match vep::parse_frame(&line, caps) {
                Ok(f) => f,
                Err(what) => {
                    let flow = self.protocol_violation(s, &what, &line);
                    return Err(self.raise(flow, at));
                }
            };
            let innermost = self.sessions[s].pending.last().map(|p| p.id);
            match frame {
                Frame::Ret { id: r, .. } | Frame::Err { id: r, .. } if Some(r) != innermost || r != id => {
                    let what = format!("reply to request {r}, which is not the innermost pending request");
                    let flow = self.protocol_violation(s, &what, &line);
                    return Err(self.raise(flow, at));
                }
                Frame::Ret { value, exports, missing, names, .. } => {
                    let reply = self.wire(s).decode(&value).and_then(|value| {
                        let (exports, missing) = self.export_lists(s, exports.as_ref(), missing.as_ref())?;
                        // Only for a message: anything but a list of strings counts as none.
                        let names = names.as_ref().and_then(Json::as_array).map(|l| l.iter().filter_map(Json::as_str).map(Rc::from).collect()).unwrap_or_default();
                        Ok(Reply { value, exports, missing, names })
                    });
                    return match (reply, unwritten) {
                        (Ok(_), Some(flow)) => {
                            // Its later output went nowhere either: it goes with the IOError.
                            self.out.discard();
                            Err(self.raise(flow, at))
                        }
                        (Ok(reply), None) => Ok(reply),
                        (Err(what), _) => {
                            let flow = self.protocol_violation(s, &what, &line);
                            Err(self.raise(flow, at))
                        }
                    };
                }
                Frame::Err { error: e, .. } => {
                    // Its error wins, so a failed write of the request's output is never raised.
                    if let Some(Flow::Error(lost)) = unwritten {
                        self.out.discard();
                        self.out.lost.get_or_insert(lost.message.clone());
                    }
                    return match self.foreign_error(s, &e, at) {
                        Ok(err) => Err(self.raise(Flow::Error(err), at)),
                        Err(what) => {
                            let flow = self.protocol_violation(s, &what, &line);
                            Err(self.raise(flow, at))
                        }
                    };
                }
                Frame::Call { id: c, target, args, kwargs } => {
                    self.serve_call(s, c, target, &args, &kwargs, &line, at)?;
                }
                Frame::Out { stream, text, error } => {
                    self.reported |= error;
                    if let Err(flow) = self.forward(stream, &text) {
                        unwritten.get_or_insert(flow);
                    }
                }
                Frame::Release { fns } => self.sessions[s].callables.release(&fns),
            }
        }
    }

    /// An `out` frame: stdout through `Out`, stderr after `Out` is flushed (9.9). A failed write
    /// is for the request that wrote it, or shows at the end of the run (6.5).
    fn forward(&mut self, stream: Stream, text: &str) -> Exec<()> {
        match stream {
            Stream::Stdout => self.out.write_str(text),
            Stream::Stderr => {
                self.out.flush_or_stop();
                stderr!("{text}");
                Ok(())
            }
        }
    }

    /// A worker's `call` into V2, answered with `ret` or `err`; the request's timer is paused
    /// meanwhile (8.5, 9.7). `exit()` inside it sends no reply (6.5).
    #[allow(clippy::too_many_arguments)]
    fn serve_call(&mut self, s: SessionId, c: u64, target: Target, args: &[Json], kwargs: &Map<String, Json>, line: &[u8], at: Span) -> Exec<()> {
        let mut pos = Vec::with_capacity(args.len());
        let mut named = Vec::with_capacity(kwargs.len());
        for a in args {
            match self.wire(s).decode(a) {
                Ok(v) => pos.push(v),
                Err(what) => {
                    let flow = self.protocol_violation(s, &what, line);
                    return Err(self.raise(flow, at));
                }
            }
        }
        for (k, v) in kwargs {
            match self.wire(s).decode(v) {
                Ok(v) => named.push((Rc::from(k.as_str()), v)),
                Err(what) => {
                    let flow = self.protocol_violation(s, &what, line);
                    return Err(self.raise(flow, at));
                }
            }
        }
        if let Some(t) = self.sessions[s].pending.last().and_then(|p| p.timer.as_ref()) {
            t.pause();
        }
        let result = match target {
            _ if self.xdepth >= MAX_XDEPTH => Err(depth_error()),
            Target::Fn(n) => match self.sessions[s].callables.get(n) {
                Some(f) => {
                    if &*self.sessions[s].key == "js" {
                        drop_js_extras(&f, &mut pos);
                    }
                    self.xdepth += 1;
                    let r = self.call_value(&f, pos, named, at);
                    self.xdepth -= 1;
                    r
                }
                None => fail("ProtocolError", format!("unknown callable id {n}")),
            },
            Target::Route(n) => {
                self.xdepth += 1;
                let js = &*self.sessions[s].key == "js";
                let r = match usize::try_from(n) {
                    Ok(n) => self.call_route(n, pos, named, at, js),
                    Err(_) => fail("ProtocolError", format!("unknown callable id {n}")),
                };
                self.xdepth -= 1;
                r
            }
        };
        if let Err(Flow::Exit(code)) = result {
            return Err(Flow::Exit(code));
        }
        // The session died inside the callback (a nested request to it failed): the callback's
        // error, or 8.2's "not running", goes on up (8.6).
        if matches!(self.sessions[s].state, State::Dead(_)) {
            result?;
            let flow = self.not_running(s).expect("a dead session");
            return Err(self.raise(flow, at));
        }
        let reply = match result {
            Ok(v) => match self.wire(s).encode(&v, "the result") {
                Ok(j) => vep::ret(c, j),
                Err(Flow::Error(e)) => vep::err(c, self.error_payload(s, &e)),
                Err(other) => return Err(other),
            },
            Err(Flow::Error(e)) => vep::err(c, self.error_payload(s, &e)),
            Err(other) => return Err(other),
        };
        if let Err(e) = self.send(s, &reply) {
            let flow = match e {
                WriteErr::TooBig(n) => {
                    let msg = vep::too_big_message(&self.tag(s), n);
                    let fallback = vep::err(c, json!({"type": "MarshalError", "message": msg, "foreign_type": null, "trace": []}));
                    match self.send(s, &fallback) {
                        Ok(()) => {
                            self.resume(s);
                            return Ok(());
                        }
                        Err(_) => self.gone_now(s, at),
                    }
                }
                WriteErr::Io(_) => self.gone_now(s, at),
            };
            return Err(self.raise(flow, at));
        }
        self.resume(s);
        Ok(())
    }

    fn resume(&mut self, s: SessionId) {
        if let Some(t) = self.sessions[s].pending.last().and_then(|p| p.timer.as_ref()) {
            t.resume();
        }
    }

    /// The innermost request's worker is gone.
    fn gone_now(&mut self, s: SessionId, at: Span) -> Flow {
        match self.sessions[s].pending.pop() {
            Some(pending) => self.worker_gone(s, &pending, at),
            None => self.not_running(s).unwrap_or_else(|| error("EngineCrashed", format!("engine '{}' exited", self.tag(s)))),
        }
    }

    /// The `error` of an `err` answering a V2 error: its kind and message, and the error itself
    /// as a tag with a `ref`, so that it comes back unchanged (9.6).
    fn error_payload(&mut self, s: SessionId, e: &Rc<ErrorValue>) -> Json {
        let mut tag = match self.wire(s).encode_passing(&Value::Error(e.clone()), "the error") {
            Ok(tag) => tag,
            // Data or a cause that can't cross is left out of what the worker gets; type, message,
            // location and trace still go, and the `ref` brings the whole error home.
            Err(_) => {
                let mut bare = ErrorValue::with_data(e.kind.clone(), e.message.clone(), Value::Null);
                bare.lang = e.lang.clone();
                bare.foreign_type = e.foreign_type.clone();
                if let Some(span) = e.span.get() {
                    let _ = bare.span.set(*span);
                }
                if let Some(trace) = e.trace.get() {
                    let _ = bare.trace.set(trace.clone());
                }
                self.wire(s).encode(&Value::Error(Rc::new(bare)), "the error").unwrap_or(Json::Null)
            }
        };
        if let Some(m) = tag.as_object_mut() {
            let session = &mut self.sessions[s];
            session.next_ref += 1;
            m.insert("ref".into(), json!(session.next_ref));
            session.sent_errors.push((session.next_ref, e.clone()));
        }
        json!({"type": &*e.kind, "message": &*e.message, "foreign_type": null, "trace": [], "v2": tag})
    }

/// A `ret`'s `exports` and `missing` (9.4): absent members are empty lists. `Err`: what is
    /// wrong, for a fatal ProtocolError (9.11).
    #[allow(clippy::type_complexity)]
    fn export_lists(&mut self, s: SessionId, exports: Option<&Json>, missing: Option<&Json>) -> Result<(Vec<(Rc<str>, Option<Value>)>, Vec<Rc<str>>), String> {
        let mut out = Vec::new();
        for e in exports.map(|e| e.as_array().ok_or("a 'ret' with a bad 'exports'")).transpose()?.into_iter().flatten() {
            let bad = || "a 'ret' with a bad 'exports' entry".to_string();
            let name = e.get("name").and_then(Json::as_str).ok_or_else(bad)?;
            let value = match e.get("kind").and_then(Json::as_str) {
                Some("func") => None,
                Some("value") => Some(self.wire(s).decode(e.get("value").ok_or_else(bad)?)?),
                _ => return Err(bad()),
            };
            out.push((Rc::from(name), value));
        }
        let mut names = Vec::new();
        for m in missing.map(|m| m.as_array().ok_or("a 'ret' with a bad 'missing'")).transpose()?.into_iter().flatten() {
            names.push(Rc::from(m.as_str().ok_or("a 'ret' with a bad 'missing'")?));
        }
        Ok((out, names))
    }

    /// An `err` from the worker as a V2 error (9.6), with its chained causes. A V2 error that
    /// went through the worker comes back as it was, location and trace included. Foreign
    /// frames are mapped in roadmap 1.8; until then a foreign error sits at the V2 call site.
    /// A worker's `err` as a V2 error raised at `at` (9.6): located at its innermost frame in a
    /// block, with the foreign frames before V2's (12.6).
    fn foreign_error(&mut self, s: SessionId, e: &Map<String, Json>, at: Span) -> Result<Rc<ErrorValue>, String> {
        self.foreign_error_at(s, e, 0, Some(at))
    }

    /// Whether the line at `span` starts with ( [ or ` and the code line above it ends as an
    /// expression can, so JavaScript, without a ; there, read the two as one (DOCS 4).
    fn joined_line(&self, span: Span) -> bool {
        let line = self.sm.line_text(span.file, span.line).trim_start();
        if !line.starts_with(['(', '[', '`']) {
            return false;
        }
        let above = (1..span.line).rev().map(|n| self.sm.line_text(span.file, n).trim_end()).find(|l| !l.trim().is_empty());
        above.and_then(|l| l.chars().last()).is_some_and(|c| c.is_alphanumeric() || "_$)]'\"`".contains(c))
    }

    /// `at`: the V2 call site, whose frames follow the foreign ones; `None` for a cause.
    fn foreign_error_at(&mut self, s: SessionId, e: &Map<String, Json>, depth: usize, at: Option<Span>) -> Result<Rc<ErrorValue>, String> {
        if let Some(tag) = e.get("v2").filter(|t| !t.is_null()) {
            let mut err = match self.wire(s).decode(tag)? {
                Value::Error(err) => err,
                _ => return Err("an 'err' whose 'v2' is not an error".into()),
            };
            // One V2 sent gets its own data and cause back: the worker's copy (decoded above, so
            // its handles are counted and released) has only what its language could keep.
            let sent = tag.get("ref").and_then(Json::as_u64).and_then(|r| self.sessions[s].sent_errors.iter().find(|(n, _)| *n == r));
            if let (Some((_, original)), Some(x)) = (sent, Rc::get_mut(&mut err)) {
                x.data = original.data.clone();
                x.cause = original.cause.clone();
            }
            // A V2 error that crossed foreign code keeps its own frames, with the foreign ones
            // between the callback's and those below the request (9.6).
            let foreign = self.foreign_frames(s, e).frames;
            if let (Some(x), false) = (Rc::get_mut(&mut err), foreign.is_empty()) {
                let mut old: Vec<Rc<str>> = x.trace.take().map(|t| t.to_vec()).unwrap_or_default();
                if old.is_empty() {
                    // Never raised in V2: this is its first raise (5.9).
                    old = at.map_or_else(Vec::new, |at| self.trace_at(at).to_vec());
                }
                let keep = old.len().saturating_sub(self.call_depth() + 1);
                let trace: Vec<Rc<str>> = old[..keep].iter().cloned().chain(foreign).chain(old[keep..].iter().cloned()).collect();
                let _ = x.trace.set(trace.into());
            }
            return Ok(err);
        }
        let kind = e.get("type").and_then(Json::as_str).ok_or("an 'err' without a string 'type'")?;
        let message = e.get("message").and_then(Json::as_str).ok_or("an 'err' without a string 'message'")?;
        // The kinds an adapter may report (9.6); any other `type` is a foreign exception's, so a
        // worker can't raise a host-only kind such as EngineNotFound.
        const KINDS: [&str; 7] = ["ForeignError", "ImportError", "NameError", "ValueError", "MarshalError", "ProtocolError", "Error"];
        let mut err = if KINDS.contains(&kind) {
            let mut err = ErrorValue::new(kind, message);
            err.foreign_type = e.get("foreign_type").and_then(Json::as_str).map(Rc::from);
            if let Some(hint) = (kind == "MarshalError").then(|| marshal_hint(message, &self.tag(s))).flatten() {
                let _ = err.hint.set(hint.into());
            }
            err
        } else {
            let mut err = ErrorValue::new("ForeignError", message);
            err.foreign_type = Some(kind.into());
            err
        };
        if &*err.kind == "ForeignError" {
            if let Some(hint) = err.foreign_type.as_deref().and_then(|t| foreign_hint(t, message, &self.tag(s))) {
                let _ = err.hint.set(hint.into());
            }
            // The adapter's own hint (a file of the user's that hides a standard module, 9.6).
            if let Some(hint) = e.get("hint").and_then(Json::as_str) {
                let _ = err.hint.set(hint.into());
            }
            // A foreign exception's own simple fields (Node's `code: "ENOENT"`, the n of
            // `sys.exit(n)` as `exit_code`) are its data (9.6).
            if let Some(data) = e.get("data").filter(|d| d.as_object().is_some_and(|o| !o.is_empty())) {
                err.data = self.wire(s).decode(data)?;
            }
        }
        let frames = self.foreign_frames(s, e);
        err.lang = Some(self.tag(s));
        if let Some((span, lang)) = frames.at {
            // `const a = [1, 2]` then `[3, 4].forEach(…)`: JavaScript joined the two lines.
            if &*err.kind == "ForeignError" && matches!(&*lang, "js" | "ts") && err.hint.get().is_none() && !message.contains("as part of the one above") && self.joined_line(span) {
                let _ = err.hint.set("JavaScript reads this line as part of the one above, as in a(…) or a[…]: end the line above with ;".into());
            }
            let _ = err.span.set(span);
            err.lang = Some(lang);
        }
        let mut trace = frames.frames;
        if let Some(at) = at {
            trace.extend(self.trace_at(at).iter().cloned());
        }
        let _ = err.trace.set(trace.into());
        if let Some(cause) = e.get("cause").filter(|c| !c.is_null() && depth < MAX_CAUSES) {
            let cause = cause.as_object().ok_or("an 'err' with a bad 'cause'")?;
            err.cause = Value::Error(self.foreign_error_at(s, cause, depth + 1, None)?);
        }
        Ok(Rc::new(err))
    }

    /// The orderly end (8.7): `bye` to every running session, newest first, then one shared
    /// 1 s drain that forwards `out` frames from all of them as they come, then the tree kill
    /// and reaping. After an interrupt the watchdog kills the workers instead (6.5).
    pub fn shutdown_sessions(&mut self) {
        process::stop_if_interrupted();
        let (lines, rx) = std::sync::mpsc::channel::<Option<Vec<u8>>>();
        let mut closing = Vec::new();
        let mut open = 0;
        for s in (0..self.sessions.len()).rev() {
            if !matches!(self.sessions[s].state, State::Ready | State::Busy(_)) {
                continue;
            }
            let session = &mut self.sessions[s];
            if let Some(w) = session.writer.as_mut() {
                let _ = vep::write_frame(w.as_mut(), &vep::bye());
            }
            session.writer = None; // close its stdin
            session.pending.clear();
            session.state = State::Closing;
            closing.push(s);
            // Each worker's stdout is read on its own thread, so one that ignores `bye` cannot
            // hold up another's exit output.
            if let Some(mut reader) = session.reader.take() {
                let lines = lines.clone();
                let drain = move || {
                    while let Ok(Some(line)) = reader.read_line() {
                        if lines.send(Some(line)).is_err() {
                            return;
                        }
                    }
                    let _ = lines.send(None);
                };
                if process::spawn_helper(std::thread::Builder::new().name("v2-drain".into()), drain).is_ok() {
                    open += 1;
                }
            }
        }
        let deadline = Instant::now() + GRACE;
        while open > 0 {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { break };
            match rx.recv_timeout(left) {
                Ok(Some(line)) => {
                    if let Ok(Frame::Out { stream, text, error }) = vep::parse_frame(&line, Caps::default()) {
                        self.reported |= error;
                        if let Err(Flow::Error(lost)) = self.forward(stream, &text) {
                            self.out.lost.get_or_insert(lost.message.clone());
                        }
                    }
                }
                Ok(None) => open -= 1,
                Err(_) => break,
            }
        }
        for s in closing {
            self.sessions[s].stop();
            self.sessions[s].state = State::Closed;
        }
        let _ = self.out.flush();
    }
}

#[cfg(test)]
mod tests;
