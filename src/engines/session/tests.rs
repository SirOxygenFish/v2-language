//! Session tests (INTERNALS 22.3): a scripted worker drives the host's side of VEP/1, in
//! process over pipes, or as a real child process (this test binary re-run with `V2_WORKER`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::thread::JoinHandle;

use super::*;
use crate::config::Config;
use crate::engines::registry::Registry;
use crate::engines::resolve::{Lookup, Origin, ProbeCache, ProbeOutcome, Prober};
use crate::engines::spec::builtins;
use crate::runtime::builtins::{BuiltinDef, Out};
use crate::runtime::call::ParamSpec;
use crate::runtime::value::{Callable, Func};
use crate::syntax::span::{FileId, SourceMap};

const ALL_CAPS: [&str; 4] = ["modules", "handles", "kwargs", "reverse"];

/// The worker's end of a session.
struct Worker<R: Read, W: Write> {
    r: FrameReader<R>,
    w: W,
}

impl<R: Read, W: Write> Worker<R, W> {
    fn recv(&mut self) -> Json {
        let line = self.r.read_line().expect("a host frame").expect("not the end of input");
        serde_json::from_slice(&line).expect("a JSON frame")
    }

    fn recv_t(&mut self, t: &str) -> Json {
        let f = self.recv();
        assert_eq!(f["t"], t, "{f}");
        f
    }

    /// The end of input: the host closed its side.
    fn recv_end(&mut self) {
        assert!(matches!(self.r.read_line(), Ok(None)), "the host's side closed");
    }

    fn send(&mut self, j: Json) {
        let mut b = serde_json::to_vec(&j).unwrap();
        b.push(b'\n');
        self.raw(&b);
    }

    fn raw(&mut self, b: &[u8]) {
        self.w.write_all(b).unwrap();
        self.w.flush().unwrap();
    }

    fn hello(&mut self, version: &str) {
        self.send(json!({"t":"hello","vep":1,"version":version,"pid":4242,"caps":ALL_CAPS}));
    }

    fn start(&mut self) {
        self.hello("3.12.1");
        let init = self.recv_t("init");
        self.send(json!({"t":"ret","id":init["id"],"value":null}));
    }

    fn ret(&mut self, request: &Json, value: Json) {
        self.send(json!({"t":"ret","id":request["id"],"value":value}));
    }
}

struct NoProbe;

impl Prober for NoProbe {
    fn probe(&self, _: &[OsString], _: &[(OsString, OsString)], _: Option<std::time::Duration>) -> ProbeOutcome {
        ProbeOutcome::SpawnFailed("no probes in session tests".into())
    }
}

fn py() -> Rc<ResolvedEngine> {
    let spec = builtins().into_iter().next().map(Rc::new).unwrap();
    Rc::new(ResolvedEngine {
        spec,
        origin: Origin::Path,
        argv: vec!["python".into()],
        version: Some(Version(3, 12, 1)),
        skipped: Vec::new(),
    })
}

/// An interpreter with one source file, an empty config and captured stdout.
fn interp() -> (Interp, Rc<RefCell<Vec<u8>>>) {
    let cfg = Config { root: PathBuf::from("C:/proj"), ..Config::default() };
    let reg = Registry::new(Rc::new(cfg), Box::new(NoProbe), Lookup::default(), ProbeCache::off());
    let mut it = Interp::new(SourceMap::new(), HashMap::new(), 2 << 20, reg);
    it.sm.add(PathBuf::from("C:/proj/main.v2"), "main.v2".into(), "x\n");
    let (out, buf) = Out::capture();
    it.out = out;
    (it, buf)
}

fn at() -> Span {
    Span { file: FileId(0), lo: 0, hi: 1, line: 1, col: 1 }
}

fn no_timeouts() -> Timeouts {
    Timeouts { startup: None, call: None }
}

type PipeWorker = Worker<std::io::PipeReader, std::io::PipeWriter>;

/// A scripted step and what the host should then raise.
type Case = (fn(&mut PipeWorker), &'static str, &'static str);
type Violation = (fn(&mut PipeWorker, &Json), &'static str);

/// A session whose worker is `script`, on its own thread, over pipes.
fn scripted(it: &mut Interp, script: impl FnOnce(PipeWorker) + Send + 'static) -> (SessionId, JoinHandle<()>) {
    let (host_r, worker_w) = std::io::pipe().unwrap();
    let (worker_r, host_w) = std::io::pipe().unwrap();
    let t = std::thread::spawn(move || script(Worker { r: FrameReader::new(worker_r), w: worker_w }));
    let s = it.attach_session(py(), Box::new(host_r), Box::new(host_w), None, no_timeouts());
    (s, t)
}

fn call(name: &str) -> Json {
    json!({"t":"call","target":{"unit":"0:0","name":name},"args":[],"kwargs":{}})
}

fn err_of<T>(r: Exec<T>) -> Rc<ErrorValue> {
    match r {
        Err(Flow::Error(e)) => e,
        Err(other) => panic!("not an error: {other:?}"),
        Ok(_) => panic!("not an error"),
    }
}

#[test]
fn handshake_and_init() {
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.raw(b"preload noise from sitecustomize\n");
        w.hello("3.12.1");
        let init = w.recv_t("init");
        assert_eq!(init["project_root"], "C:/proj");
        assert!(init.get("paths").is_none(), "init carries only the project root (9.3)");
        w.ret(&init, Json::Null);
        w.recv_t("bye");
        w.recv_end();
    });
    it.handshake(s, at()).expect("ready");
    assert!(matches!(it.sessions[s].state, State::Ready));
    assert_eq!(it.sessions[s].hello.as_ref().map(|h| h.pid), Some(4242));
    it.shutdown_sessions();
    t.join().unwrap();
}

#[test]
fn handshake_failures() {
    let cases: Vec<Case> = vec![
        (|w| w.send(json!({"t":"ret","id":1,"value":null})), "ProtocolError", "engine 'py' sent 'ret' before hello"),
        // ch. 21: `out` too, the frame a worker might plausibly send early.
        (|w| w.send(json!({"t":"out","stream":"stdout","text":"x"})), "ProtocolError", "engine 'py' sent 'out' before hello"),
        (|w| w.send(json!({"t":"hello","vep":1})), "ProtocolError", "engine 'py' broke the protocol: 'hello' frame without 'caps'"),
        (
            |w| w.send(json!({"t":"hello","vep":2,"version":"1","pid":1,"caps":[]})),
            "ProtocolError",
            "engine 'py' speaks VEP/2; this V2 speaks VEP/1",
        ),
        (|w| w.hello("3.6.8"), "EngineNotFound", "Python 3.6.8 at python is too old (needs 3.10+)"),
        (
            |w| {
                w.hello("3.12.1");
                let init = w.recv_t("init");
                w.send(json!({"t":"err","id":init["id"],"error":{"type":"ImportError","message":"boom","trace":[]}}));
            },
            "EngineCrashed",
            "engine 'py' failed to start: boom",
        ),
        (|_| {}, "EngineCrashed", "engine 'py' exited during startup (an unknown exit status); see its error output above, if any"),
    ];
    for (script, kind, message) in cases {
        let (mut it, _) = interp();
        let (s, t) = scripted(&mut it, move |mut w| script(&mut w));
        let e = err_of(it.handshake(s, at()));
        assert_eq!((&*e.kind, e.message.split(" (got:").next().unwrap()), (kind, message));
        assert!(matches!(it.sessions[s].state, State::Dead(_)));
        t.join().unwrap();
        // Every later use fails without contacting the worker; a failed start re-raises.
        let again = err_of(it.request(s, call("f"), "a call to 'f'", at()));
        if kind == "EngineNotFound" {
            assert_eq!(&*again.message, message);
        } else {
            assert!(again.message.starts_with("engine 'py' is not running (stopped earlier: "), "{}", again.message);
        }
    }
}

#[test]
fn replies_echo_ids_and_out_comes_first() {
    let (mut it, stdout) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.start();
        let c = w.recv_t("call");
        assert_eq!(c["id"], 2, "init was request 1");
        w.send(json!({"t":"out","stream":"stdout","text":"hi\n"}));
        w.send(json!({"t":"out","stream":"stderr","text":"to stderr\n"}));
        w.ret(&c, json!({"$v2":"int","v":"9007199254740993"}));
        let c = w.recv_t("call");
        w.ret(&c, json!([1.0, "x"]));
        w.recv_t("bye");
        w.send(json!({"t":"out","stream":"stdout","text":"after bye\n"}));
    });
    it.handshake(s, at()).unwrap();
    let r = it.request(s, call("f"), "a call to 'f'", at()).unwrap();
    assert!(matches!(r.value, Value::Int(9007199254740993)));
    assert_eq!(String::from_utf8_lossy(&stdout.borrow()), "hi\n", "out before ret");
    let r = it.request(s, call("g"), "a call to 'g'", at()).unwrap();
    assert!(matches!(r.value, Value::List(_)));
    assert!(matches!(it.sessions[s].state, State::Ready));
    it.shutdown_sessions();
    assert!(String::from_utf8_lossy(&stdout.borrow()).ends_with("after bye\n"), "output after bye is drained");
    assert!(matches!(it.sessions[s].state, State::Closed));
    t.join().unwrap();
}

#[test]
fn protocol_violations_are_fatal_then_not_running() {
    let scripts: Vec<Violation> = vec![
        (|w, c| w.send(json!({"t":"ret","id":c["id"].as_u64().unwrap() + 7,"value":null})), "reply to request 9, which is not the innermost pending request"),
        (|w, _| w.raw(b"not a frame\n"), "not valid JSON (expected ident at line 1 column 2)"),
        (|w, _| w.raw(b"[1]\n"), "a line that is not a JSON object"),
        (|w, _| w.send(json!({"t":"load","id":1})), "a 'load' frame, which a worker may not send"),
        (|w, c| w.ret(c, json!({"$v2":"nope"})), "an unknown '$v2' tag 'nope'"),
    ];
    for (script, what) in scripts {
        let (mut it, _) = interp();
        let (s, t) = scripted(&mut it, move |mut w| {
            w.start();
            let c = w.recv_t("call");
            script(&mut w, &c);
            w.recv_end();
        });
        it.handshake(s, at()).unwrap();
        let e = err_of(it.request(s, call("f"), "a call to 'f'", at()));
        assert_eq!(&*e.kind, "ProtocolError");
        assert!(e.message.starts_with(&format!("engine 'py' broke the protocol: {what} (got: ")), "{}", e.message);
        let again = err_of(it.request(s, call("f"), "a call to 'f'", at()));
        assert_eq!(&*again.message, "engine 'py' is not running (stopped earlier: broke the protocol); engines are not restarted");
        t.join().unwrap();
    }
}

#[test]
fn eof_is_engine_crashed() {
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.start();
        w.recv_t("call");
    });
    it.handshake(s, at()).unwrap();
    let e = err_of(it.request(s, call("ratio"), "a call to 'ratio'", at()));
    assert_eq!(&*e.kind, "EngineCrashed");
    assert_eq!(&*e.message, "engine 'py' (pid 4242) exited during a call to 'ratio' (an unknown exit status); see its error output above, if any");
    let again = err_of(it.request(s, call("f"), "a call to 'f'", at()));
    assert_eq!(&*again.message, "engine 'py' is not running (stopped earlier: exited during a call to 'ratio' at main.v2:1); engines are not restarted");
    t.join().unwrap();
}

fn add(_: &mut Interp, args: Vec<Value>, _: Span) -> Exec<Value> {
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
        _ => fail("ValueError", "add() wants two ints"),
    }
}

static ADD: BuiltinDef = BuiltinDef { name: "add", params: &[ParamSpec::req("a"), ParamSpec::req("b")], f: add };

fn quit(_: &mut Interp, _: Vec<Value>, _: Span) -> Exec<Value> {
    Err(Flow::Exit(7))
}

static QUIT: BuiltinDef = BuiltinDef { name: "quit", params: &[], f: quit };

fn func(def: &'static BuiltinDef) -> Value {
    Value::Func(Func::new(Callable::Builtin(def)))
}

/// `call` with one V2 function as its argument, encoded for session `s`.
fn call_with(it: &mut Interp, s: SessionId, f: &Value) -> Json {
    let arg = it.wire(s).encode(f, "args[0]").unwrap();
    json!({"t":"call","target":{"unit":"0:0","name":"apply"},"args":[arg],"kwargs":{}})
}

#[test]
fn callbacks_into_v2() {
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.start();
        let c = w.recv_t("call");
        let f = c["args"][0].clone();
        assert_eq!((f["$v2"].as_str(), f["name"].as_str()), (Some("fn"), Some("add")));
        let id = f["id"].as_u64().unwrap();
        // A call with an unknown id gets an err; the session stays usable (9.5).
        w.send(json!({"t":"call","id":1,"target":{"fn":77},"args":[],"kwargs":{}}));
        let r = w.recv_t("err");
        assert_eq!((r["id"].as_u64(), r["error"]["type"].as_str(), r["error"]["message"].as_str()), (Some(1), Some("ProtocolError"), Some("unknown callable id 77")));
        // Positional and named arguments.
        w.send(json!({"t":"call","id":2,"target":{"fn":id},"args":[2],"kwargs":{"b":3}}));
        let r = w.recv_t("ret");
        assert_eq!((r["id"].as_u64(), r["value"].as_i64()), (Some(2), Some(5)));
        // A V2 error goes back with its tag (9.6) ...
        w.send(json!({"t":"call","id":3,"target":{"fn":id},"args":["x", 1],"kwargs":{}}));
        let r = w.recv_t("err");
        assert_eq!(r["error"]["type"], "ValueError");
        let tag = r["error"]["v2"].clone();
        assert_eq!(tag["$v2"], "error");
        // ... and comes back unchanged when the foreign code doesn't catch it.
        w.send(json!({"t":"err","id":c["id"],"error":{"type":"ValueError","message":"add() wants two ints","trace":[],"v2":tag}}));
        w.recv_t("bye");
    });
    it.handshake(s, at()).unwrap();
    let frame = call_with(&mut it, s, &func(&ADD));
    let e = err_of(it.request(s, frame, "a call to 'apply'", at()));
    assert_eq!((&*e.kind, &*e.message), ("ValueError", "add() wants two ints"));
    it.shutdown_sessions();
    t.join().unwrap();
}

#[test]
fn exit_in_a_callback_sends_no_reply() {
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.start();
        let c = w.recv_t("call");
        w.send(json!({"t":"call","id":1,"target":{"fn":c["args"][0]["id"]},"args":[],"kwargs":{}}));
        // No reply to the call: the next frame is bye (6.5, 9.7).
        w.recv_t("bye");
    });
    it.handshake(s, at()).unwrap();
    let frame = call_with(&mut it, s, &func(&QUIT));
    assert!(matches!(it.request(s, frame, "a call to 'apply'", at()), Err(Flow::Exit(7))));
    it.shutdown_sessions();
    t.join().unwrap();
}

#[test]
fn cross_language_depth_limit_both_ways() {
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.start();
        let c = w.recv_t("call");
        let id = c["args"][0]["id"].clone();
        // The host is at depth 64 while it waits: a call into V2 would be the 65th.
        w.send(json!({"t":"call","id":1,"target":{"fn":id},"args":[1, 2],"kwargs":{}}));
        let r = w.recv_t("err");
        assert_eq!((r["error"]["type"].as_str(), r["error"]["message"].as_str()), (Some("RecursionError"), Some("cross-language call depth limit (64) exceeded")));
        w.ret(&c, Json::Null);
        w.recv_t("bye");
    });
    it.handshake(s, at()).unwrap();
    let frame = call_with(&mut it, s, &func(&ADD));
    it.xdepth = MAX_XDEPTH - 1;
    it.request(s, frame, "a call to 'apply'", at()).expect("the 64th crossing is fine");
    it.xdepth = MAX_XDEPTH;
    let e = err_of(it.request(s, call("f"), "a call to 'f'", at()));
    assert_eq!((&*e.kind, &*e.message), ("RecursionError", "cross-language call depth limit (64) exceeded"));
    it.xdepth = 0;
    it.shutdown_sessions();
    t.join().unwrap();
}

#[test]
fn releases_ride_the_next_frame() {
    // 9.10, 11.2: dropped handles queue their ids and the next frame to that worker carries
    // them, however many; a statement boundary sends nothing on its own.
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.start();
        let c = w.recv_t("call");
        let handles: Vec<Json> = (1..=3).map(|i| json!({"$v2":"h","id":i,"type":"Thing"})).collect();
        w.ret(&c, Json::Array(handles));
        // The three dropped handles come before the next request, in one frame.
        let r = w.recv_t("release");
        assert_eq!(r["h"], json!([1, 2, 3]));
        let c = w.recv_t("call");
        let many: Vec<Json> = (10..266).map(|i| json!({"$v2":"h","id":i,"type":"Thing"})).collect();
        w.ret(&c, Json::Array(many));
        let r = w.recv_t("release");
        assert_eq!(r["h"].as_array().map(Vec::len), Some(256), "carried by the next request");
        let c = w.recv_t("call");
        w.ret(&c, Json::Null);
        w.recv_t("bye");
    });
    it.handshake(s, at()).unwrap();
    let r = it.request(s, call("f"), "a call to 'f'", at()).unwrap();
    drop(r);
    let r = it.request(s, call("g"), "a call to 'g'", at()).unwrap();
    drop(r);
    let prog = crate::syntax::parser::parse_str("let y = 1").unwrap();
    it.run(FileId(0), &prog).unwrap();
    assert_eq!(it.sessions[s].releases.borrow().len(), 256, "nothing sent at the statement boundary");
    it.request(s, call("h"), "a call to 'h'", at()).unwrap();
    it.shutdown_sessions();
    t.join().unwrap();
}

#[test]
fn worker_command_and_env() {
    let adapter = PathBuf::from("/run/adapters/py.py");
    let (argv, env) = worker_command(&py(), &adapter);
    assert_eq!(argv, [OsString::from("python"), "-B".into(), "-u".into(), adapter.clone().into_os_string()]);
    assert!(env.contains(&("PYTHONUTF8".into(), "1".into())) && env.contains(&("PYTHONIOENCODING".into(), "utf-8".into())));
    // js: Node finds packages itself; V2 sets no NODE_PATH (an inherited one stays as it is).
    let (argv, env) = worker_command(&js(), &adapter);
    assert_eq!(argv, [OsString::from("node"), adapter.into_os_string()]);
    assert!(!env.iter().any(|(k, _)| k == "NODE_PATH"), "{env:?}");
}

#[test]
fn exit_phrases() {
    assert_eq!(exit_phrase(Some(&ExitInfo { code: Some(1), signal: None })), "exited with code 1");
    assert_eq!(exit_phrase(None), "exited");
    assert_eq!(secs(Duration::from_secs(5)), "5 s");
    assert_eq!(secs(Duration::from_millis(300)), "0.3 s");
}

// ---------------------------------------------------------------------------------------------
// Real worker processes: this test binary re-run as the worker

/// argv and env that run this test binary as the worker named `script` (`worker_main`).
fn worker(script: &str) -> (Vec<OsString>, Vec<(OsString, OsString)>) {
    let exe = std::env::current_exe().expect("the test binary");
    let argv = ["--exact", "engines::session::tests::worker_main", "--nocapture", "--test-threads=1"];
    let argv = std::iter::once(exe.into_os_string()).chain(argv.into_iter().map(OsString::from)).collect();
    (argv, vec![("V2_WORKER".into(), script.into())])
}

/// The worker process: does nothing unless `V2_WORKER` names a script. libtest's
/// "running 1 test" line comes before `hello`: preload noise for the host (9.3).
#[test]
fn worker_main() {
    let Ok(script) = std::env::var("V2_WORKER") else { return };
    let mut w = Worker { r: FrameReader::new(std::io::stdin()), w: std::io::stdout() };
    // libtest has printed "test ...worker_main ... " without a line end: finish that line, so
    // it is noise and `hello` gets a line of its own.
    w.raw(b"\n");
    match script.as_str() {
        "serve" => {
            w.start();
            loop {
                let f = w.recv();
                match f["t"].as_str() {
                    Some("call") => w.ret(&f, json!(f["target"]["name"])),
                    Some("bye") => std::process::exit(0),
                    _ => std::process::exit(2),
                }
            }
        }
        "crash" => {
            w.start();
            w.recv_t("call");
            std::process::exit(3);
        }
        "slow" => {
            w.start();
            let f = w.recv_t("call");
            if f["target"]["name"] == "reverse" {
                // Call V2 back, which takes 600 ms: not this worker's time (8.5).
                w.send(json!({"t":"call","id":1,"target":{"fn":f["args"][0]["id"]},"args":[],"kwargs":{}}));
                w.recv_t("ret");
                w.ret(&f, json!("done"));
                w.recv_t("call");
            }
            std::thread::sleep(Duration::from_secs(20));
        }
        "late" => {
            std::thread::sleep(Duration::from_secs(20));
        }
        "ignore_bye" => {
            w.start();
            w.recv_t("bye");
            std::thread::sleep(Duration::from_secs(20));
        }
        "idle_exit" => {
            w.start();
            std::process::exit(5);
        }
        // Exit output larger than a pipe buffer (8.7).
        "bye_flood" => {
            w.start();
            w.recv_t("bye");
            let text = "x".repeat(16 << 10);
            for _ in 0..64 {
                w.send(json!({"t":"out","stream":"stdout","text":text}));
            }
            std::process::exit(0);
        }
        "hang_after_reverse" => {
            w.start();
            let f = w.recv_t("call");
            w.send(json!({"t":"call","id":1,"target":{"fn":f["args"][0]["id"]},"args":[],"kwargs":{}}));
            w.recv_t("ret");
            std::thread::sleep(Duration::from_secs(20));
        }
        "placeholder" => std::process::exit(9009),
        other => panic!("unknown worker script {other}"),
    }
}

fn start(it: &mut Interp, script: &str, timeouts: Timeouts) -> Exec<SessionId> {
    let (argv, env) = worker(script);
    let engine = ResolvedEngine { argv: argv.clone(), ..clone_engine(&py()) };
    it.start_session(Rc::new(engine), &argv, &env, timeouts, at())
}

fn clone_engine(e: &ResolvedEngine) -> ResolvedEngine {
    ResolvedEngine {
        spec: e.spec.clone(),
        origin: e.origin.clone(),
        argv: e.argv.clone(),
        version: e.version,
        skipped: e.skipped.clone(),
    }
}

#[test]
fn spawned_worker_lifecycle() {
    let _serial = crate::engines::process::tests::serial();
    let (mut it, _) = interp();
    let s = start(&mut it, "serve", no_timeouts()).expect("started");
    let r = it.request(s, call("hello"), "a call to 'hello'", at()).unwrap();
    assert!(matches!(r.value, Value::Str(v) if &*v == "hello"));
    it.shutdown_sessions();
    assert!(matches!(it.sessions[s].state, State::Closed));
}

#[test]
fn a_crash_during_a_call_names_the_exit_code() {
    let _serial = crate::engines::process::tests::serial();
    let (mut it, _) = interp();
    let s = start(&mut it, "crash", no_timeouts()).expect("started");
    let e = err_of(it.request(s, call("ratio"), "a call to 'ratio'", at()));
    assert_eq!(&*e.message, "engine 'py' (pid 4242) exited during a call to 'ratio' (exit code 3); see its error output above, if any");
    let again = err_of(it.request(s, call("f"), "a call to 'f'", at()));
    assert_eq!(&*again.message, "engine 'py' is not running (stopped earlier: exited with code 3 during a call to 'ratio' at main.v2:1); engines are not restarted");
}

static SLEEPY: BuiltinDef = BuiltinDef { name: "sleepy", params: &[], f: sleepy };

fn sleepy(_: &mut Interp, _: Vec<Value>, _: Span) -> Exec<Value> {
    std::thread::sleep(Duration::from_millis(600));
    Ok(Value::Null)
}

#[test]
fn call_timeout_counts_only_the_workers_time() {
    let _serial = crate::engines::process::tests::serial();
    let (mut it, _) = interp();
    let timeouts = Timeouts { startup: Some(Duration::from_secs(30)), call: Some(Duration::from_millis(300)) };
    let s = start(&mut it, "slow", timeouts).expect("started");
    // 600 ms in a V2 callback do not count against the 300 ms limit (8.5).
    let mut frame = call_with(&mut it, s, &func(&SLEEPY));
    frame["target"]["name"] = json!("reverse");
    let r = it.request(s, frame, "a call to 'reverse'", at()).expect("the timer was paused");
    assert!(matches!(r.value, Value::Str(v) if &*v == "done"));
    let started = Instant::now();
    let e = err_of(it.request(s, call("slow"), "a call to 'slow'", at()));
    assert_eq!((&*e.kind, &*e.message), ("EngineTimeout", "a call to 'slow' in engine 'py' exceeded call_timeout (0.3 s); the engine was stopped"));
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    let again = err_of(it.request(s, call("f"), "a call to 'f'", at()));
    assert!(again.message.contains("stopped earlier: exceeded call_timeout (0.3 s)"), "{}", again.message);
}

#[test]
fn startup_timeout() {
    let _serial = crate::engines::process::tests::serial();
    let (mut it, _) = interp();
    let started = Instant::now();
    let e = err_of(start(&mut it, "late", Timeouts { startup: Some(Duration::from_millis(300)), call: None }));
    assert_eq!(&*e.kind, "EngineTimeout");
    assert!(e.message.starts_with("engine 'py' did not start within 0.3 s ("), "{}", e.message);
    assert!(e.message.ends_with("); raise [engines.py] startup_timeout if it is only slow"), "{}", e.message);
    assert!(started.elapsed() < Duration::from_secs(5));
    // 21.2: killed and Dead; a later use says why (8.2).
    let s = it.sessions.len() - 1;
    let later = err_of(it.request(s, call("f"), "a call to 'f'", at()));
    assert_eq!(
        (&*later.kind, &*later.message),
        ("EngineCrashed", "engine 'py' is not running (stopped earlier: did not start within 0.3 s); engines are not restarted")
    );
}

#[test]
fn grace_kills_a_worker_that_ignores_bye() {
    let _serial = crate::engines::process::tests::serial();
    let (mut it, _) = interp();
    let s = start(&mut it, "ignore_bye", no_timeouts()).expect("started");
    let started = Instant::now();
    it.shutdown_sessions();
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(900) && took < Duration::from_secs(5), "{took:?}");
    assert!(matches!(it.sessions[s].state, State::Closed));
}

#[cfg(windows)]
#[test]
fn status_9009_before_hello_is_engine_not_found() {
    let _serial = crate::engines::process::tests::serial();
    let (mut it, _) = interp();
    let e = err_of(start(&mut it, "placeholder", no_timeouts()));
    assert_eq!(&*e.kind, "EngineNotFound");
    assert!(e.message.ends_with("is the Microsoft Store placeholder (exit code 9009)"), "{}", e.message);
    // 21.2: Dead (SpawnFailed); a later use raises this EngineNotFound again (8.2).
    let s = it.sessions.len() - 1;
    let later = err_of(it.request(s, call("f"), "a call to 'f'", at()));
    assert_eq!((&*later.kind, &*later.message), (&*e.kind, &*e.message));
}

#[test]
fn a_spawn_failure_is_engine_not_found_every_time() {
    let (mut it, _) = interp();
    let missing = std::env::temp_dir().join("v2-no-such-dir").join("python.exe");
    let argv = [missing.into_os_string()];
    let e = err_of(it.start_session(py(), &argv, &[], no_timeouts(), at()));
    assert_eq!(&*e.kind, "EngineNotFound");
    assert!(e.message.starts_with("could not start '"), "{}", e.message);
    let s = it.sessions.len() - 1;
    let again = err_of(it.request(s, call("f"), "a call to 'f'", at()));
    assert_eq!(again.message, e.message);
}

// ---------------------------------------------------------------------------------------------
// Mutation kills

fn interp_cfg(cfg: Config) -> (Interp, Rc<RefCell<Vec<u8>>>) {
    let reg = Registry::new(Rc::new(cfg), Box::new(NoProbe), Lookup::default(), ProbeCache::off());
    let mut it = Interp::new(SourceMap::new(), HashMap::new(), 2 << 20, reg);
    it.sm.add(PathBuf::from("C:/proj/main.v2"), "main.v2".into(), "x\n");
    let (out, buf) = Out::capture();
    it.out = out;
    (it, buf)
}

fn js() -> Rc<ResolvedEngine> {
    let spec = builtins().into_iter().nth(1).map(Rc::new).unwrap();
    Rc::new(ResolvedEngine { spec, origin: Origin::Path, argv: vec!["node".into()], version: Some(Version(24, 11, 1)), skipped: Vec::new() })
}

#[test]
fn a_session_a_ts_block_starts_is_held_to_js_minimum() {
    // 8.4: the ts spec has no minimum of its own, so hello is checked against js's.
    let (mut it, _) = interp();
    let ts = builtins().into_iter().nth(2).map(Rc::new).unwrap();
    let engine = Rc::new(ResolvedEngine { spec: ts, origin: Origin::Path, argv: vec!["node".into()], version: Some(Version(24, 11, 1)), skipped: Vec::new() });
    let (s, t) = scripted_engine(&mut it, engine, |mut w| {
        w.send(json!({"t":"hello","vep":1,"version":"23.1.0","pid":7,"caps":[]}));
    });
    let e = err_of(it.handshake(s, at()));
    assert_eq!((&*e.kind, &*e.message), ("EngineNotFound", "JavaScript 23.1.0 at node is too old (needs 22.13+, not 23.0 or 23.1)"));
    assert!(format!("{:?}", it.sessions[s].state).starts_with("Dead"));
    t.join().unwrap();
}

fn scripted_engine(it: &mut Interp, engine: Rc<ResolvedEngine>, script: impl FnOnce(PipeWorker) + Send + 'static) -> (SessionId, JoinHandle<()>) {
    let (host_r, worker_w) = std::io::pipe().unwrap();
    let (worker_r, host_w) = std::io::pipe().unwrap();
    let t = std::thread::spawn(move || script(Worker { r: FrameReader::new(worker_r), w: worker_w }));
    let s = it.attach_session(engine, Box::new(host_r), Box::new(host_w), None, no_timeouts());
    (s, t)
}

#[test]
fn init_is_the_project_root_and_the_minimums_hold() {
    let cfg = Config { root: PathBuf::from("C:/proj"), ..Config::default() };
    let (mut it, _) = interp_cfg(cfg);
    let (s, t) = scripted(&mut it, |mut w| {
        w.hello("3.10.0"); // the minimum itself is fine (8.4)
        let init = w.recv_t("init");
        let mut keys: Vec<&String> = init.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["caps", "id", "project_root", "t"], "init carries the project root and the host's caps (9.3)");
        assert_eq!(init["caps"], json!([]), "this host announces no optional caps yet");
        w.ret(&init, Json::Null);
        w.recv_t("bye");
    });
    let (j, tj) = scripted_engine(&mut it, js(), |mut w| {
        w.send(json!({"t":"hello","vep":1,"version":"22.13.0","pid":7,"caps":["modules","handles","reverse"]}));
        let init = w.recv_t("init");
        assert_eq!(init["project_root"], "C:/proj");
        w.ret(&init, Json::Null);
        w.recv_t("bye");
    });
    it.handshake(s, at()).expect("py ready");
    it.handshake(j, at()).expect("js ready");
    it.shutdown_sessions();
    t.join().unwrap();
    tj.join().unwrap();
}

/// The host's end of a pipe that also logs, per session, each `bye` written to it.
struct LogBye {
    name: &'static str,
    log: Rc<RefCell<Vec<&'static str>>>,
    w: std::io::PipeWriter,
}

impl Write for LogBye {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if b.windows(9).any(|x| x == br#""t":"bye""#) {
            self.log.borrow_mut().push(self.name);
        }
        self.w.write(b)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.w.flush()
    }
}

#[test]
fn shutdown_is_newest_first() {
    let (mut it, stdout) = interp();
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut threads = Vec::new();
    for (name, text) in [("older", "a\n"), ("newer", "b\n")] {
        let (host_r, worker_w) = std::io::pipe().unwrap();
        let (worker_r, host_w) = std::io::pipe().unwrap();
        threads.push(std::thread::spawn(move || {
            let mut w = Worker { r: FrameReader::new(worker_r), w: worker_w };
            w.start();
            w.recv_t("bye");
            w.send(json!({"t":"out","stream":"stdout","text":text}));
        }));
        let writer = LogBye { name, log: log.clone(), w: host_w };
        let s = it.attach_session(py(), Box::new(host_r), Box::new(writer), None, no_timeouts());
        it.handshake(s, at()).unwrap();
    }
    it.shutdown_sessions();
    assert_eq!(*log.borrow(), ["newer", "older"], "bye goes newest first (8.7)");
    let out = String::from_utf8_lossy(&stdout.borrow()).into_owned();
    assert!(out.len() == 4 && out.contains("a\n") && out.contains("b\n"), "both drained: {out:?}");
    for t in threads {
        t.join().unwrap();
    }
}

#[test]
fn exit_output_is_kept_next_to_a_worker_that_ignores_bye() {
    let _serial = crate::engines::process::tests::serial();
    // The drain reads every worker at once: one that ignores `bye` cannot block another's
    // exit output behind a full pipe (8.7).
    let (mut it, stdout) = interp();
    start(&mut it, "bye_flood", no_timeouts()).expect("started");
    start(&mut it, "ignore_bye", no_timeouts()).expect("started");
    it.shutdown_sessions();
    assert_eq!(stdout.borrow().len(), 64 * (16 << 10));
}

static REENTER: BuiltinDef = BuiltinDef { name: "reenter", params: &[], f: reenter };
static SWALLOW: BuiltinDef = BuiltinDef { name: "swallow", params: &[], f: swallow };

/// A V2 callback that calls the worker that called it, and lets its error through.
fn reenter(it: &mut Interp, _: Vec<Value>, at: Span) -> Exec<Value> {
    it.request(0, call("inner"), "a call to 'inner'", at).map(|r| r.value)
}

/// The same, but it catches the error.
fn swallow(it: &mut Interp, args: Vec<Value>, at: Span) -> Exec<Value> {
    let _ = reenter(it, args, at);
    Ok(Value::Null)
}

#[test]
fn a_failure_inside_a_callback_reaches_the_outer_call() {
    // The nested request to the same worker breaks the protocol and kills the session; the
    // outer call gets that error, or 8.2's "not running" when V2 code caught it (8.6).
    for (f, kind, message) in [
        (&REENTER, "ProtocolError", "engine 'py' broke the protocol: not valid JSON"),
        (&SWALLOW, "EngineCrashed", "engine 'py' is not running (stopped earlier: broke the protocol); engines are not restarted"),
    ] {
        let (mut it, _) = interp();
        let (s, t) = scripted(&mut it, |mut w| {
            w.start();
            let c = w.recv_t("call");
            w.send(json!({"t":"call","id":1,"target":{"fn":c["args"][0]["id"]},"args":[],"kwargs":{}}));
            w.recv_t("call");
            w.raw(b"garbage\n");
            let _ = w.r.read_line();
        });
        it.handshake(s, at()).unwrap();
        let frame = call_with(&mut it, s, &func(f));
        let e = err_of(it.request(s, frame, "a call to 'apply'", at()));
        assert_eq!((&*e.kind, e.message.split(" (expected").next().unwrap()), (kind, message));
        t.join().unwrap();
    }
}

static BIG_FN: BuiltinDef = BuiltinDef { name: "big_fn", params: &[], f: big_fn };

fn big_fn(_: &mut Interp, _: Vec<Value>, _: Span) -> Exec<Value> {
    Ok(Value::list(vec![func(&ADD), Value::str(&"z".repeat(vep::MAX_FRAME))]))
}

#[test]
fn a_reply_too_big_to_send_gives_its_callables_back() {
    // The `fn` tag in a reply that was never sent holds no count (10.1, 11.8).
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.start();
        let c = w.recv_t("call");
        w.send(json!({"t":"call","id":1,"target":{"fn":c["args"][0]["id"]},"args":[],"kwargs":{}}));
        assert_eq!(w.recv_t("err")["error"]["type"], "MarshalError");
        w.ret(&c, Json::Null);
        w.recv_t("bye");
    });
    it.handshake(s, at()).unwrap();
    let frame = call_with(&mut it, s, &func(&BIG_FN));
    it.request(s, frame, "a call to 'apply'", at()).unwrap();
    let table = &it.sessions[s].callables;
    assert_eq!((table.count(1), table.count(2)), (1, 0), "big_fn was sent, add never was");
    it.shutdown_sessions();
    t.join().unwrap();
}

#[test]
fn foreign_errors_keep_their_type_as_foreign_type() {
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.start();
        let c = w.recv_t("call");
        w.send(json!({"t":"err","id":c["id"],"error":{"type":"ZeroDivisionError","message":"division by zero","foreign_type":"ZeroDivisionError","trace":[]}}));
        let c = w.recv_t("call");
        w.send(json!({"t":"err","id":c["id"],"error":{"type":"KeyError","message":"'k'","foreign_type":"KeyError","trace":[]}}));
        w.recv_t("bye");
    });
    it.handshake(s, at()).unwrap();
    let e = err_of(it.request(s, call("ratio"), "a call to 'ratio'", at()));
    assert_eq!((&*e.kind, &*e.message, e.foreign_type.as_deref(), e.lang.as_deref()), ("ForeignError", "division by zero", Some("ZeroDivisionError"), Some("py")));
    let e = err_of(it.request(s, call("get"), "a call to 'get'", at()));
    assert_eq!((&*e.kind, e.foreign_type.as_deref(), e.lang.as_deref()), ("ForeignError", Some("KeyError"), Some("py")));
    it.shutdown_sessions();
    t.join().unwrap();
}

#[test]
fn a_worker_reports_only_9_6s_kinds() {
    // 9.6: an adapter's own kinds keep their type; a host-only kind is a foreign exception's
    // name, so a worker can't make V2 exit 3 or claim a build or FFI failure.
    let kinds = [
        ("ForeignError", "ForeignError"),
        ("ImportError", "ImportError"),
        ("NameError", "NameError"),
        ("ArityError", "ForeignError"),
        ("ValueError", "ValueError"),
        ("MarshalError", "MarshalError"),
        ("ProtocolError", "ProtocolError"),
        ("Error", "Error"),
        ("EngineNotFound", "ForeignError"),
        ("EngineCrashed", "ForeignError"),
        ("EngineTimeout", "ForeignError"),
        ("EngineUnknown", "ForeignError"),
        ("EngineNameConflict", "ForeignError"),
        ("BuildError", "ForeignError"),
        ("FFILoadError", "ForeignError"),
        ("FFISymbolError", "ForeignError"),
        ("FFIValueError", "ForeignError"),
        ("TypeError", "ForeignError"),
    ];
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, move |mut w| {
        w.start();
        for (sent, _) in kinds {
            let c = w.recv_t("call");
            w.send(json!({"t":"err","id":c["id"],"error":{"type":sent,"message":"m","foreign_type":null,"trace":[]}}));
        }
        w.recv_t("bye");
    });
    it.handshake(s, at()).unwrap();
    for (sent, kind) in kinds {
        let e = err_of(it.request(s, call("f"), "a call to 'f'", at()));
        let foreign_type = (kind == "ForeignError" && sent != "ForeignError").then_some(sent);
        assert_eq!((&*e.kind, e.foreign_type.as_deref()), (kind, foreign_type), "{sent}");
    }
    it.shutdown_sessions();
    t.join().unwrap();
}

#[test]
fn a_line_over_64_mib_after_hello() {
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.start();
        w.recv_t("call");
        let mut big = vec![b'x'; vep::MAX_FRAME + 1];
        big.push(b'\n');
        let _ = w.w.write_all(&big); // the host stops reading once the line passes the limit
    });
    it.handshake(s, at()).unwrap();
    let e = err_of(it.request(s, call("f"), "a call to 'f'", at()));
    assert_eq!((&*e.kind, &*e.message), ("ProtocolError", "frame from engine 'py' exceeds 64 MiB"));
    let again = err_of(it.request(s, call("f"), "a call to 'f'", at()));
    assert_eq!(&*again.message, "engine 'py' is not running (stopped earlier: broke the protocol); engines are not restarted");
    t.join().unwrap();
}

static BIG: BuiltinDef = BuiltinDef { name: "big", params: &[], f: big };

fn big(_: &mut Interp, _: Vec<Value>, _: Span) -> Exec<Value> {
    Ok(Value::str(&"z".repeat(vep::MAX_FRAME)))
}

#[test]
fn a_callback_result_too_big_is_an_err_to_the_worker() {
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.start();
        let c = w.recv_t("call");
        w.send(json!({"t":"call","id":1,"target":{"fn":c["args"][0]["id"]},"args":[],"kwargs":{}}));
        let r = w.recv_t("err");
        assert_eq!(r["error"]["type"], "MarshalError");
        assert_eq!(r["error"]["message"], "value too large to send to engine 'py' (65 MiB, limit 64 MiB)");
        w.ret(&c, json!("still usable"));
        w.recv_t("bye");
    });
    it.handshake(s, at()).unwrap();
    let frame = call_with(&mut it, s, &func(&BIG));
    let r = it.request(s, frame, "a call to 'apply'", at()).expect("the session survives");
    assert!(matches!(r.value, Value::Str(v) if &*v == "still usable"));
    it.shutdown_sessions();
    t.join().unwrap();
}

static NEST: BuiltinDef = BuiltinDef { name: "nest", params: &[], f: nest };

/// A V2 callback that calls back into the worker that called it.
fn nest(it: &mut Interp, _: Vec<Value>, at: Span) -> Exec<Value> {
    match it.request(0, call("inner"), "a call to 'inner'", at) {
        Err(Flow::Error(e)) => Ok(Value::str(&e.kind)),
        Ok(_) => Ok(Value::str("no limit")),
        Err(f) => Err(f),
    }
}

#[test]
fn a_served_call_counts_as_a_crossing() {
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.start();
        let c = w.recv_t("call");
        // Crossing 63 is the host's call, 64 this callback: its own call out would be the 65th.
        w.send(json!({"t":"call","id":1,"target":{"fn":c["args"][0]["id"]},"args":[],"kwargs":{}}));
        let r = w.recv_t("ret");
        assert_eq!(r["value"], "RecursionError");
        w.ret(&c, Json::Null);
        w.recv_t("bye");
    });
    it.handshake(s, at()).unwrap();
    let frame = call_with(&mut it, s, &func(&NEST));
    it.xdepth = MAX_XDEPTH - 2;
    it.request(s, frame, "a call to 'apply'", at()).unwrap();
    it.xdepth = 0;
    it.shutdown_sessions();
    t.join().unwrap();
}

#[test]
fn an_init_err_of_any_type_kills_the_worker() {
    // 8.4: `init` answered with `err` -> kill, EngineCrashed "failed to start", whatever its type.
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.hello("3.12.1");
        let init = w.recv_t("init");
        w.send(json!({"t":"err","id":init["id"],"error":{"type":"ProtocolError","message":"no init here","trace":[]}}));
        let _ = w.r.read_line();
    });
    let e = err_of(it.handshake(s, at()));
    let state = format!("{:?}", it.sessions[s].state);
    assert_eq!((&*e.kind, &*e.message, state.starts_with("Dead")), ("EngineCrashed", "engine 'py' failed to start: no init here", true), "{state}");
    t.join().unwrap();
}

#[test]
fn a_death_while_idle_is_noticed_at_the_next_use() {
    let _serial = crate::engines::process::tests::serial();
    let (mut it, _) = interp();
    let s = start(&mut it, "idle_exit", no_timeouts()).expect("started");
    let deadline = Instant::now() + Duration::from_secs(10);
    while it.sessions[s].child.as_ref().and_then(ChildProc::exited).is_none() {
        assert!(Instant::now() < deadline, "the worker did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }
    let e = err_of(it.request(s, call("f"), "a call to 'f'", at()));
    assert_eq!((&*e.kind, &*e.message), ("EngineCrashed", "engine 'py' exited while idle (exit code 5)"));
    let again = err_of(it.request(s, call("f"), "a call to 'f'", at()));
    assert_eq!(&*again.message, "engine 'py' is not running (stopped earlier: exited with code 5 at main.v2:1); engines are not restarted");
}

#[test]
fn the_timer_runs_again_after_a_reverse_call() {
    let _serial = crate::engines::process::tests::serial();
    let (mut it, _) = interp();
    let timeouts = Timeouts { startup: Some(Duration::from_secs(30)), call: Some(Duration::from_millis(300)) };
    let s = start(&mut it, "hang_after_reverse", timeouts).expect("started");
    let frame = call_with(&mut it, s, &func(&SLEEPY));
    let started = Instant::now();
    let e = err_of(it.request(s, frame, "a call to 'reverse'", at()));
    assert_eq!((&*e.kind, &*e.message), ("EngineTimeout", "a call to 'reverse' in engine 'py' exceeded call_timeout (0.3 s); the engine was stopped"));
    assert!(started.elapsed() < Duration::from_secs(8), "{:?}", started.elapsed());
}

#[test]
fn the_grace_is_shared() {
    let _serial = crate::engines::process::tests::serial();
    let (mut it, _) = interp();
    start(&mut it, "ignore_bye", no_timeouts()).expect("started");
    start(&mut it, "ignore_bye", no_timeouts()).expect("started");
    let started = Instant::now();
    it.shutdown_sessions();
    let took = started.elapsed();
    assert!(took < Duration::from_millis(1700), "one shared 1 s grace (8.7): {took:?}");
}

#[cfg(unix)]
#[test]
fn signal_phrase() {
    assert_eq!(exit_phrase(Some(&ExitInfo { code: None, signal: Some(9) })), "was killed by signal 9 (SIGKILL)");
}

/// A host process for `noise_before_hello_reaches_stderr`: does nothing unless `V2_HOST`.
#[test]
fn host_main() {
    if std::env::var_os("V2_HOST").is_none() {
        return;
    }
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.raw(b"preload noise 7b1e\n");
        w.raw(b"no newline 7b1e");
        w.start();
        w.recv_t("bye");
    });
    it.handshake(s, at()).unwrap();
    it.shutdown_sessions();
    t.join().unwrap();
}

#[test]
fn noise_before_hello_reaches_stderr() {
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe)
        .args(["--exact", "engines::session::tests::host_main", "--nocapture", "--test-threads=1"])
        .env("V2_HOST", "1")
        .output()
        .unwrap();
    assert!(out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("preload noise 7b1e\n") && err.contains("no newline 7b1e\n"), "{err}");
}

#[test]
fn a_handshake_at_depth_64_leaves_no_ready_session() {
    // The init request is the 65th crossing: RecursionError, but the worker never got init.
    let (mut it, _) = interp();
    let (s, _t) = scripted(&mut it, |mut w| {
        w.hello("3.12.1");
        let _ = w.r.read_line();
    });
    it.xdepth = MAX_XDEPTH;
    let e = err_of(it.handshake(s, at()));
    it.xdepth = 0;
    let state = format!("{:?}", it.sessions[s].state);
    assert_eq!((&*e.kind, state.starts_with("Dead")), ("RecursionError", true), "{state}");
}

#[test]
fn a_dict_key_holding_a_handle_wins_over_the_dict_methods() {
    // INTERNALS 5.5, DOCS 3.6 and 3.10: a module that re-exports a foreign function value
    // (`let keys = pymod.keys`) is called through its key, even when a dict method has that name:
    // the worker gets `call {h}` (11.3). Before the fix `d.keys()` silently ran the dict method.
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.start();
        let c = w.recv_t("call");
        assert_eq!(c["target"], json!({"h": 1}));
        w.ret(&c, json!("the foreign keys"));
        w.recv_t("bye");
    });
    it.handshake(s, at()).unwrap();
    let env = crate::runtime::env::new_frame(&crate::syntax::ast::Layout::from([]), None);
    let releases = it.sessions[s].releases.clone();
    // A function arrives callable (the `h` tag's `callable`, 11.1); other handles are data (DOCS 3.6).
    let h = Value::Handle(Rc::new(crate::engines::handles::HandleRef::new(s, "py".into(), 1, "function".into(), releases).with_callable(true)));
    let d = Value::dict([("keys".into(), h)].into_iter().collect());
    crate::runtime::env::declare(&env, "d".into(), d, false);
    let prog = crate::syntax::parser::parse_str("d.keys()").unwrap();
    let crate::syntax::ast::StmtKind::Expr(e) = &prog[0].kind else { panic!("expression") };
    let Ok(Value::Str(r)) = it.eval(e, &env) else { panic!("the dict method ran") };
    assert_eq!(&*r, "the foreign keys");
    it.shutdown_sessions();
    t.join().unwrap();
}

#[test]
fn a_worker_without_modules_is_sent_no_module_frame() {
    // 13.5, 17.5: the host raises before sending anything; the worker sees only init and bye.
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.send(json!({"t":"hello","vep":1,"version":"3.12.1","pid":1,"caps":["handles","kwargs","reverse"]}));
        let init = w.recv_t("init");
        w.send(json!({"t":"ret","id":init["id"],"value":null}));
        w.recv_t("bye");
    });
    it.handshake(s, at()).unwrap();
    let file = it.sm.add(PathBuf::from("C:/proj/mods.v2"), "mods.v2".into(), "@import { mean } from py.statistics\n");
    let toks = crate::syntax::lexer::lex(&it.sm, file).unwrap();
    let program = crate::syntax::parser::parse_program(&it.sm, toks).unwrap();
    let e = err_of(it.run(file, &program));
    assert_eq!((&*e.kind, &*e.message), ("ImportError", "engine 'py' cannot import packages"));
    it.shutdown_sessions();
    t.join().unwrap();
}

#[test]
fn a_session_that_failed_to_start_reports_its_own_error_first() {
    // 8.2: a dead session's caps say nothing; a module import or a call with a function arg
    // raises its stored EngineNotFound, not 'cannot import packages' or a reverse-cap MarshalError.
    let (mut it, _) = interp();
    let (s, t) = scripted(&mut it, |mut w| {
        w.send(json!({"t":"hello","vep":1,"version":"3.9.0","pid":7,"caps":ALL_CAPS}));
    });
    assert_eq!(&*err_of(it.handshake(s, at())).kind, "EngineNotFound");
    t.join().unwrap();
    let file = it.sm.add(PathBuf::from("C:/proj/mods.v2"), "mods.v2".into(), "@import { mean } from py.statistics\n");
    let toks = crate::syntax::lexer::lex(&it.sm, file).unwrap();
    let program = crate::syntax::parser::parse_program(&it.sm, toks).unwrap();
    let e = err_of(it.run(file, &program));
    assert_eq!((&*e.kind, e.message.contains("too old")), ("EngineNotFound", true), "{}", e.message);
    let proxy = Value::func(Callable::Proxy { session: s, unit: "0:0".into(), name: "f".into() });
    let e = err_of(it.call_value(&proxy, vec![func(&ADD)], Vec::new(), at()));
    assert_eq!(&*e.kind, "EngineNotFound", "{}", e.message);
}
