//! Engine sessions: one worker process per bridged engine for the whole run, its state machine,
//! the handshake, the re-entrant request loop and shutdown (INTERNALS ch. 8, 9.7-9.10). The
//! child-process rules and the watchdog are in `engines/process`; frames in `engines/vep`;
//! values in `codec/vep`.

#![allow(dead_code)] // blocks start sessions from roadmap 1.4 on

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
use crate::config::Config;
use crate::runtime::error::{fail, ErrorValue, Exec, Flow};
use crate::runtime::interp::Interp;
use crate::runtime::value::Value;
use crate::syntax::span::Span;

pub type SessionId = usize;

/// Cross-language crossings in progress at most, across all engines (9.7, DOCS 8.4).
pub const MAX_XDEPTH: u32 = 64;

/// Queued releases that make a standalone `release` at a statement boundary (9.10).
const RELEASE_BATCH: usize = 256;

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
    reader: Option<FrameReader<Box<dyn Read>>>,
    writer: Option<Box<dyn Write>>,
    pub hello: Option<Hello>,
    caps: Caps,
    next_id: u64,
    pending: Vec<Pending>,
    pub releases: ReleaseQueue,
    pub callables: CallableTable,
    timeouts: Timeouts,
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

/// A `ret` (9.6): its decoded value, and for `load`/`module` the export lists.
pub struct Reply {
    pub value: Value,
    pub exports: Option<Json>,
    pub missing: Option<Json>,
}

/// A worker's argv and environment (8.3, 14.1): the resolved argv, the spec's fixed arguments
/// with `{adapter}` filled in, the spec's environment, and for js `NODE_PATH` = `[node] modules`,
/// then `[node] paths`, then any inherited value (D54).
pub fn worker_command(e: &ResolvedEngine, cfg: &Config, adapter: &Path) -> (Vec<OsString>, Vec<(OsString, OsString)>) {
    let mut argv = e.argv.clone();
    for a in &e.spec.fixed_args {
        argv.push(if &**a == "{adapter}" { adapter.as_os_str().to_os_string() } else { OsString::from(&**a) });
    }
    let mut env: Vec<(OsString, OsString)> = e.spec.env.iter().map(|(k, v)| (OsString::from(&**k), OsString::from(&**v))).collect();
    if &*e.spec.id == "js" {
        let mut dirs = vec![cfg.node_modules.as_ref().map_or_else(|| cfg.root.join("node_modules"), |m| m.value.clone())];
        dirs.extend(cfg.node_paths.iter().flat_map(|p| p.value.iter().cloned()));
        if let Some(inherited) = std::env::var_os("NODE_PATH") {
            dirs.extend(std::env::split_paths(&inherited));
        }
        if let Ok(joined) = std::env::join_paths(dirs) {
            env.push(("NODE_PATH".into(), joined));
        }
    }
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

/// An error at no location yet: `raise` places it at the V2 expression.
fn error(kind: &str, message: String) -> Flow {
    match fail::<()>(kind, message) {
        Err(f) => f,
        Ok(()) => unreachable!(),
    }
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
        let key = engine.spec.session_of.clone().unwrap_or_else(|| engine.spec.id.clone());
        let spawned = ChildProc::spawn(&Spawn { argv, env, stdin: Pipe::Piped, stdout: Pipe::Piped, stderr: Pipe::Inherit });
        let mut child = match spawned {
            Ok(c) => c,
            Err(msg) => {
                let e = Rc::new(ErrorValue::new("EngineNotFound", msg));
                let s = self.new_session(key, engine, None, None, None, timeouts);
                self.sessions[s].state = State::Dead(DeadReason::SpawnFailed(e.clone()));
                return Err(self.raise(Flow::Error(e), at));
            }
        };
        let reader: Box<dyn Read> = Box::new(child.take_stdout().expect("piped stdout"));
        let writer: Box<dyn Write> = Box::new(child.take_stdin().expect("piped stdin"));
        let s = self.new_session(key, engine, Some(reader), Some(writer), Some(child), timeouts);
        self.handshake(s, at)?;
        Ok(s)
    }

    /// A session over any channel, before its handshake (the tests' scripted workers).
    pub fn attach_session(
        &mut self,
        engine: Rc<ResolvedEngine>,
        reader: Box<dyn Read>,
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
        reader: Option<Box<dyn Read>>,
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
            return error("EngineCrashed", format!("engine '{tag}' exited during startup ({how}); see its error output above"));
        }
        let pid = pid.map_or_else(String::new, |p| format!(" (pid {p})"));
        error("EngineCrashed", format!("engine '{tag}'{pid} exited during {} ({how}); see its error output above", pending.name))
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
                    let flow = self.protocol_violation(s, "a line over 64 MiB", b"");
                    return Err(self.raise(flow, at));
                }
                _ => {
                    let flow = self.worker_gone(s, &startup, at);
                    return Err(self.raise(flow, at));
                }
            };
            if vep::is_noise(&line) {
                let _ = self.out.flush();
                stderrln!("{}", String::from_utf8_lossy(&line));
                continue;
            }
            match vep::parse_hello(&line) {
                Ok(h) => break h,
                Err(what) => {
                    let t = serde_json::from_slice::<Json>(&line).ok().and_then(|j| j.get("t").and_then(Json::as_str).map(str::to_string));
                    let flow = match t {
                        Some(t) if t != "hello" => {
                            self.kill_session(s, DeadReason::Protocol);
                            error("ProtocolError", format!("engine '{tag}' sent '{t}' before hello"))
                        }
                        _ => self.protocol_violation(s, &what, &line),
                    };
                    return Err(self.raise(flow, at));
                }
            }
        };
        // A launcher or alias may run another interpreter than the one probed (8.4).
        let spec = self.sessions[s].engine.spec.clone();
        if let (Some(min), Some(v)) = (spec.min_version, Version::parse(&hello.version)) {
            if v < min {
                let path = self.sessions[s].engine.argv[0].to_string_lossy().into_owned();
                let e = Rc::new(ErrorValue::new(
                    "EngineNotFound",
                    format!("{} {v} at {path} is too old (needs {}+)", spec.language, min.short()),
                ));
                self.kill_session(s, DeadReason::SpawnFailed(e.clone()));
                return Err(self.raise(Flow::Error(e), at));
            }
        }
        self.sessions[s].caps = hello.caps;
        self.sessions[s].hello = Some(hello);
        self.sessions[s].state = State::Ready;
        let root = self.registry.config().root.to_string_lossy().into_owned();
        let mut paths: Vec<String> = Vec::new();
        if &*spec.id == "py" {
            paths.push(root.clone());
            for p in self.registry.config().python_paths.iter().flat_map(|p| p.value.iter()) {
                let p = p.to_string_lossy().into_owned();
                if !paths.contains(&p) {
                    paths.push(p);
                }
            }
        }
        let init = vep::init(0, &root, &paths);
        match self.exchange(s, init, startup, at) {
            Ok(_) => Ok(()),
            Err(Flow::Error(e)) if !matches!(&*e.kind, "EngineTimeout" | "EngineCrashed" | "ProtocolError" | "RecursionError") => {
                self.kill_session(s, DeadReason::Exited { exit: None, during: None, at: Some(at) });
                let flow = error("EngineCrashed", format!("engine '{tag}' failed to start: {}", e.message));
                Err(self.raise(flow, at))
            }
            Err(f) => Err(f),
        }
    }

    /// The use of a session that is not running (8.2, 8.6).
    fn check_alive(&mut self, s: SessionId, at: Span) -> Exec<()> {
        let tag = self.tag(s);
        let why = match &self.sessions[s].state {
            State::Ready | State::Busy(_) => None,
            State::Dead(DeadReason::SpawnFailed(e)) => {
                let e = Rc::new(ErrorValue::new(&e.kind, e.message.clone()));
                return Err(self.raise(Flow::Error(e), at));
            }
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
        if let Some(why) = why {
            let msg = format!("engine '{tag}' is not running (stopped earlier: {why}); engines are not restarted");
            return Err(self.raise(error("EngineCrashed", msg), at));
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

    /// Send one frame, the session's queued releases first (9.10). A write error is the
    /// worker going away (9.1).
    fn send(&mut self, s: SessionId, frame: &Json) -> Result<(), WriteErr> {
        let _ = self.out.flush();
        let session = &mut self.sessions[s];
        let Some(w) = session.writer.as_mut() else { return Err(WriteErr::Io(std::io::ErrorKind::BrokenPipe.into())) };
        if session.caps.handles {
            let ids = std::mem::take(&mut *session.releases.borrow_mut());
            if !ids.is_empty() {
                vep::write_frame(w.as_mut(), &vep::release(&ids))?;
            }
        }
        vep::write_frame(w.as_mut(), frame)
    }

    /// A request to the worker (`load`, `module`, `call`, `op`) and its reply, serving the
    /// worker's calls into V2 meanwhile (9.7). `name` describes it for messages.
    pub fn request(&mut self, s: SessionId, frame: Json, name: &str, at: Span) -> Exec<Reply> {
        self.check_alive(s, at)?;
        let timer = self.timer(s, self.sessions[s].timeouts.call);
        let pending = Pending { id: 0, name: name.into(), timer, startup: false };
        self.exchange(s, frame, pending, at)
    }

    fn exchange(&mut self, s: SessionId, mut frame: Json, mut pending: Pending, at: Span) -> Exec<Reply> {
        if self.xdepth >= MAX_XDEPTH {
            let flow = error("RecursionError", format!("cross-language call depth limit ({MAX_XDEPTH}) exceeded"));
            return Err(self.raise(flow, at));
        }
        let session = &mut self.sessions[s];
        session.next_id += 1;
        let id = session.next_id;
        frame["id"] = json!(id);
        pending.id = id;
        if let Err(e) = self.send(s, &frame) {
            let flow = match e {
                WriteErr::TooBig(n) => error("MarshalError", vep::too_big_message(&self.tag(s), n)),
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
        }
        result
    }

    fn wire(&mut self, s: SessionId) -> Wire<'_> {
        let session = &mut self.sessions[s];
        Wire {
            session: s,
            tag: session.key.clone(),
            caps: session.caps,
            releases: &session.releases,
            callables: &mut session.callables,
            sm: &self.sm,
            route: &|_| None,
        }
    }

    fn await_reply(&mut self, s: SessionId, id: u64, at: Span) -> Exec<Reply> {
        loop {
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
                Frame::Ret { value, exports, missing, .. } => {
                    return match self.wire(s).decode(&value) {
                        Ok(value) => Ok(Reply { value, exports, missing }),
                        Err(what) => {
                            let flow = self.protocol_violation(s, &what, &line);
                            Err(self.raise(flow, at))
                        }
                    };
                }
                Frame::Err { error: e, .. } => {
                    return match self.foreign_error(s, &e) {
                        Ok(err) => Err(self.raise(Flow::Error(Rc::new(err)), at)),
                        Err(what) => {
                            let flow = self.protocol_violation(s, &what, &line);
                            Err(self.raise(flow, at))
                        }
                    };
                }
                Frame::Call { id: c, target, args, kwargs } => {
                    self.serve_call(s, c, target, &args, &kwargs, &line, at)?;
                }
                Frame::Out { stream, text } => self.forward(stream, &text),
                Frame::Release { fns } => self.sessions[s].callables.release(&fns),
            }
        }
    }

    /// An `out` frame: stdout through `Out`, stderr after `Out` is flushed (9.9). It is no V2
    /// expression's error; a failed write shows at the end of the run (6.5).
    fn forward(&mut self, stream: Stream, text: &str) {
        match stream {
            Stream::Stdout => {
                let _ = self.out.write_str(text);
            }
            Stream::Stderr => {
                let _ = self.out.flush();
                stderr!("{text}");
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
            _ if self.xdepth >= MAX_XDEPTH => fail("RecursionError", format!("cross-language call depth limit ({MAX_XDEPTH}) exceeded")),
            Target::Fn(n) => match self.sessions[s].callables.get(n) {
                Some(f) => {
                    self.xdepth += 1;
                    let r = self.call_value(&f, pos, named, at);
                    self.xdepth -= 1;
                    r
                }
                None => fail("ProtocolError", format!("unknown callable id {n}")),
            },
            // Routes of inner `@import`s arrive in roadmap 1.7; until then no worker has one.
            Target::Route(n) => fail("ProtocolError", format!("unknown callable id {n}")),
        };
        let reply = match result {
            Err(Flow::Exit(code)) => return Err(Flow::Exit(code)),
            Ok(v) => match self.wire(s).encode(&v, "the result") {
                Ok(j) => vep::ret(c, j),
                Err(Flow::Error(e)) => vep::err(c, self.error_payload(s, &e)),
                Err(other) => return Err(other),
            },
            Err(Flow::Error(e)) => vep::err(c, self.error_payload(s, &e)),
            Err(other) => return Err(other),
        };
        if !matches!(self.sessions[s].state, State::Ready | State::Busy(_)) {
            return Ok(()); // the session died inside the callback; the read reports it
        }
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
            None => error("EngineCrashed", format!("engine '{}' exited", self.tag(s))),
        }
    }

    /// The `error` of an `err` answering a V2 error: its kind and message, and the error itself
    /// as a tag so that it comes back unchanged (9.6).
    fn error_payload(&mut self, s: SessionId, e: &Rc<ErrorValue>) -> Json {
        let tag = self.wire(s).encode(&Value::Error(e.clone()), "the error").unwrap_or(Json::Null);
        json!({"type": &*e.kind, "message": &*e.message, "foreign_type": null, "trace": [], "v2": tag})
    }

    /// An `err` from the worker as a V2 error (9.6). Its location and foreign frames are mapped
    /// in roadmap 1.8; until then it sits at the V2 call site.
    fn foreign_error(&mut self, s: SessionId, e: &Map<String, Json>) -> Result<ErrorValue, String> {
        if let Some(tag) = e.get("v2").filter(|t| !t.is_null()) {
            return match self.wire(s).decode(tag)? {
                Value::Error(err) => {
                    let mut restored = ErrorValue::with_data(err.kind.clone(), err.message.clone(), err.data.clone());
                    restored.lang = err.lang.clone();
                    restored.foreign_type = err.foreign_type.clone();
                    Ok(restored)
                }
                _ => Err("an 'err' whose 'v2' is not an error".into()),
            };
        }
        let kind = e.get("type").and_then(Json::as_str).ok_or("an 'err' without a string 'type'")?;
        let message = e.get("message").and_then(Json::as_str).ok_or("an 'err' without a string 'message'")?;
        const KINDS: [&str; 23] = [
            "Error", "TypeError", "ValueError", "KeyError", "IndexError", "NameError", "ArityError", "IOError",
            "RecursionError", "ImportError", "EngineUnknown", "EngineNotFound", "EngineNameConflict", "EngineCrashed",
            "EngineTimeout", "EngineExitError", "MarshalError", "BuildError", "ForeignError", "ProtocolError",
            "FFILoadError", "FFISymbolError", "FFIValueError",
        ];
        let mut err = if KINDS.contains(&kind) {
            let mut err = ErrorValue::new(kind, message);
            err.foreign_type = e.get("foreign_type").and_then(Json::as_str).map(Rc::from);
            err
        } else {
            let mut err = ErrorValue::new("ForeignError", message);
            err.foreign_type = Some(kind.into());
            err
        };
        err.lang = Some(self.tag(s));
        Ok(err)
    }

    /// At a statement boundary: a standalone `release` to each session holding 256 or more
    /// queued ids (9.10). A write error is left for the next use to report (8.6).
    pub fn flush_releases(&mut self) {
        for session in &mut self.sessions {
            if !session.caps.handles || !matches!(session.state, State::Ready | State::Busy(_)) {
                continue;
            }
            if session.releases.borrow().len() < RELEASE_BATCH {
                continue;
            }
            let ids = std::mem::take(&mut *session.releases.borrow_mut());
            if let Some(w) = session.writer.as_mut() {
                let _ = vep::write_frame(w.as_mut(), &vep::release(&ids));
            }
        }
    }

    /// The orderly end (8.7): `bye` to every running session, newest first, then one shared
    /// 1 s drain that forwards `out` frames, then the tree kill and reaping. After an interrupt
    /// the watchdog kills the workers instead (6.5).
    pub fn shutdown_sessions(&mut self) {
        process::stop_if_interrupted();
        let mut closing = Vec::new();
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
        }
        let deadline = Instant::now() + GRACE;
        for s in closing {
            let left = deadline.saturating_duration_since(Instant::now());
            let timer = self.timer(s, Some(left));
            while let Some(Ok(Some(line))) = self.sessions[s].reader.as_mut().map(FrameReader::read_line) {
                if let Ok(Frame::Out { stream, text }) = vep::parse_frame(&line, Caps::default()) {
                    self.forward(stream, &text);
                }
            }
            drop(timer);
            self.sessions[s].stop();
            self.sessions[s].state = State::Closed;
        }
        let _ = self.out.flush();
    }
}

#[cfg(test)]
mod tests;
