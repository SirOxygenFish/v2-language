//! The VEP/1 conformance kit (INTERNALS 17.6, 22.3): checks a bridged adapter against the
//! protocol by talking to it directly, frame by frame, the way V2 does.
//!
//! ```text
//! vep_check KIT.json -- COMMAND [ARG ...]
//! cargo run --example vep_check -- examples/lua_adapter.kit.json -- lua examples/lua_adapter.lua
//! ```
//!
//! `KIT.json` holds snippets in the adapter's language (DOCS 13.2):
//!
//! * `definition` (required): a block that defines `echo(x)`, returning `x`; `fail(msg)`, raising
//!   an error whose message contains `msg`; `say(text)`, printing `text` and a line break; and a
//!   constant `answer` equal to `42`.
//! * `fail_line`: the body line (1-based) where `fail` raises; when given, the error's trace must
//!   point there.
//! * `action_value`: an action block whose value is `3`.
//! * `action_print`: an action block that prints `hi` and then `there`, each on its own line.
//! * `syntax_error` and `syntax_error_line`: a block with a syntax error on that body line.
//! * `module` (`{"name", "member"}`): checked when the adapter has the `modules` cap: a module of
//!   the language and a function in it.
//! * `apply` and `route`: checked when it has `reverse`: `apply` defines `apply(f, x)`, which
//!   returns `f(x)`; `route` defines `twice(x)`, which returns `host_double(x)` (an imported name).
//!
//! Each check prints `ok` or `FAIL` and why; the exit status is the number of failures (at most
//! 100), 0 when the adapter passes.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::Duration;

use serde_json::{json, Map, Value};

const FILE: &str = "/kit/kit.v2";
const LINE: u64 = 10;
const WAIT: Duration = Duration::from_secs(10);

/// An adapter process: frames go to its stdin; its stdout lines come through `lines`.
struct Adapter {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Option<Vec<u8>>>,
    next_id: u64,
}

impl Adapter {
    fn start(argv: &[String]) -> Result<Adapter, String> {
        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", argv[0]))?;
        let stdout = child.stdout.take().expect("piped");
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                match r.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(None);
                        return;
                    }
                    Ok(_) => {
                        if tx.send(Some(line)).is_err() {
                            return;
                        }
                    }
                }
            }
        });
        let stdin = child.stdin.take();
        Ok(Adapter { child, stdin, lines, next_id: 1 })
    }

    fn send(&mut self, frame: &Value) -> Result<(), String> {
        self.send_raw(&format!("{frame}\n"))
    }

    fn send_raw(&mut self, text: &str) -> Result<(), String> {
        let pipe = self.stdin.as_mut().ok_or("stdin is closed")?;
        pipe.write_all(text.as_bytes()).and_then(|_| pipe.flush()).map_err(|e| format!("the adapter stopped reading: {e}"))
    }

    /// The next frame, or why there is none.
    fn read(&mut self) -> Result<Value, String> {
        let line = match self.lines.recv_timeout(WAIT) {
            Ok(Some(line)) => line,
            Ok(None) | Err(RecvTimeoutError::Disconnected) => return Err("the adapter's stdout ended".into()),
            Err(RecvTimeoutError::Timeout) => return Err(format!("no frame within {}s", WAIT.as_secs())),
        };
        let text = std::str::from_utf8(&line).map_err(|_| format!("a line that isn't UTF-8: {}", shown(&line)))?;
        if !text.ends_with('\n') {
            return Err(format!("a frame without its line break: {}", shown(&line)));
        }
        let v: Value = serde_json::from_str(text).map_err(|e| format!("a line that isn't JSON ({e}): {}", shown(&line)))?;
        if !v.is_object() || !v["t"].is_string() {
            return Err(format!("a frame that isn't an object with a string t: {}", shown(&line)));
        }
        if !text.starts_with("{\"t\"") {
            return Err(format!("t isn't the frame's first member (9.1): {}", shown(&line)));
        }
        Ok(v)
    }

    /// A request: its frame sent with the next id, then frames read up to its reply. `out` frames
    /// are collected; a `release` is noted; a worker `call` goes to `serve`.
    fn request(&mut self, mut frame: Value, serve: &mut dyn FnMut(&Value) -> Value) -> Result<Reply, String> {
        let id = self.next_id;
        self.next_id += 1;
        frame["id"] = json!(id);
        self.send(&frame)?;
        let mut reply = Reply::default();
        loop {
            let f = self.read()?;
            match f["t"].as_str().unwrap_or_default() {
                "ret" | "err" if f["id"] == json!(id) => {
                    reply.frame = f;
                    return Ok(reply);
                }
                "ret" | "err" => return Err(format!("a reply to request {} while {id} is the innermost: {f}", f["id"])),
                "out" => {
                    let (Some(stream), Some(text)) = (f["stream"].as_str(), f["text"].as_str()) else { return Err(format!("a malformed out frame: {f}")) };
                    if stream != "stdout" && stream != "stderr" {
                        return Err(format!("an out frame for stream {stream:?}"));
                    }
                    reply.out.push((stream.to_string(), text.to_string()));
                }
                "release" => reply.released.extend(f["fn"].as_array().cloned().unwrap_or_default()),
                "call" => {
                    let answer = serve(&f);
                    reply.calls.push(f);
                    self.send(&answer)?;
                }
                other => return Err(format!("a frame an adapter may not send now: {other}: {f}")),
            }
        }
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// The exit status within `limit`, else `None` (the adapter is then killed).
    fn wait(&mut self, limit: Duration) -> Option<i32> {
        let end = std::time::Instant::now() + limit;
        while std::time::Instant::now() < end {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status.code().unwrap_or(-1));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        None
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct Reply {
    frame: Value,
    out: Vec<(String, String)>,
    calls: Vec<Value>,
    released: Vec<Value>,
}

impl Reply {
    fn ret(&self) -> Result<&Value, String> {
        match self.frame["t"].as_str() {
            Some("ret") => Ok(&self.frame["value"]),
            _ => Err(format!("err instead of ret: {}", self.frame["error"])),
        }
    }

    fn err(&self) -> Result<&Value, String> {
        match self.frame["t"].as_str() {
            Some("err") => Ok(&self.frame["error"]),
            _ => Err(format!("ret instead of err: {}", self.frame)),
        }
    }

    fn stdout(&self) -> String {
        self.out.iter().filter(|(s, _)| s == "stdout").map(|(_, t)| t.as_str()).collect()
    }
}

fn shown(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let s: String = s.chars().take(120).collect();
    format!("{s:?}")
}

/// An `int` tag as the integer it holds: a worker may send an integer within i64 either way (10.2).
fn untag(v: &Value) -> Option<Value> {
    if v.get("$v2")? != "int" {
        return None;
    }
    let n: i64 = v.get("v")?.as_str()?.parse().ok()?;
    Some(Value::from(n))
}

/// Values equal as V2 sees them: ints and floats apart, dict order aside.
fn same(a: &Value, b: &Value) -> bool {
    if let Some(a) = untag(a) {
        return same(&a, b);
    }
    if let Some(b) = untag(b) {
        return same(a, &b);
    }
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64(), x.is_f64(), y.is_f64()) {
            (Some(p), Some(q), true, true) => p == q && p.is_sign_negative() == q.is_sign_negative(),
            (_, _, false, false) => x.as_i64() == y.as_i64() && x.as_u64() == y.as_u64(),
            _ => false,
        },
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w))),
        _ => a == b,
    }
}

/// The values `echo` must give back unchanged (ch. 10), each with what it checks.
fn echo_cases() -> Vec<(&'static str, Value)> {
    let mut deep = json!(1);
    for _ in 0..50 {
        deep = json!([deep]);
    }
    vec![
        ("null", json!(null)),
        ("true", json!(true)),
        ("false", json!(false)),
        ("int 0", json!(0)),
        ("negative int", json!(-7)),
        ("int 2^53-1", json!(9007199254740991_i64)),
        ("int tag beyond 2^53", json!({"$v2": "int", "v": "9223372036854775807"})),
        ("float 1.5", json!(1.5)),
        ("float 1.0 stays a float", json!(1.0)),
        ("float -0.0", json!(-0.0)),
        ("nan tag", json!({"$v2": "float", "v": "nan"})),
        ("inf tag", json!({"$v2": "float", "v": "inf"})),
        ("-inf tag", json!({"$v2": "float", "v": "-inf"})),
        ("empty str", json!("")),
        ("unicode str", json!("héllo ✓ 𝄞 日本")),
        ("str with line breaks", json!("a\u{2028}b\r\nc\nd\u{0}e")),
        ("bytes tag", json!({"$v2": "bytes", "b64": "AAEC/w=="})),
        ("empty list", json!([])),
        ("nested list", json!([1, [2.5, ["x"]]])),
        ("list holding null", json!([1, null, 3])),
        ("dict", json!({"a": 1, "b": [true, null], "c": {"d": "e"}})),
        ("empty dict", json!({})),
        ("dict tag (a $v2 key)", json!({"$v2": "dict", "v": {"$v2": 1, "x": 2}})),
        ("50 nested lists", deep),
        ("1 MB str", json!("x".repeat(1 << 20))),
    ]
}

struct Kit {
    results: Vec<(String, Result<(), String>)>,
}

impl Kit {
    fn check(&mut self, name: &str, r: Result<(), String>) {
        match &r {
            Ok(()) => println!("ok    {name}"),
            Err(why) => println!("FAIL  {name}: {why}"),
        }
        self.results.push((name.to_string(), r));
    }
}

fn need(c: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if c {
        Ok(())
    } else {
        Err(why())
    }
}

fn no_calls(_: &Value) -> Value {
    json!({"t": "err", "id": 0, "error": {"type": "ProtocolError", "message": "the kit expected no call here", "foreign_type": null, "trace": []}})
}

fn load(kind: &str, unit: &str, source: &str, exports: &[&str], want_value: bool, imports: Value, args: Value) -> Value {
    json!({"t": "load", "unit": unit, "kind": kind, "lang": "kit", "file": FILE, "line": LINE, "source": source,
           "args": args, "imports": imports, "exports": exports, "want_value": want_value})
}

fn call(unit: &str, name: &str, args: Value) -> Value {
    json!({"t": "call", "target": {"unit": unit, "name": name}, "args": args, "kwargs": {}})
}

/// The checks of one adapter session; `Err` stops the session (the adapter is unusable).
fn session(kit: &mut Kit, argv: &[String], snippets: &Map<String, Value>) -> Result<(), String> {
    let snippet = |k: &str| snippets.get(k).and_then(Value::as_str);
    let mut a = Adapter::start(argv)?;

    // hello (9.3), with noise allowed before it.
    let hello = loop {
        let line = match a.lines.recv_timeout(WAIT) {
            Ok(Some(l)) => l,
            _ => return Err("no hello".into()),
        };
        let text = String::from_utf8_lossy(&line);
        if let Some(at) = text.find("{\"t\":\"hello\"") {
            break serde_json::from_str::<Value>(&text[at..]).map_err(|e| format!("hello isn't JSON: {e}"))?;
        }
        if text.trim_start().starts_with('{') {
            return Err(format!("a frame before hello: {text}"));
        }
    };
    let caps: Vec<String> = hello["caps"].as_array().map(|c| c.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    kit.check(
        "hello: vep 1, version, pid, caps",
        need(hello["vep"] == json!(1) && hello["version"].is_string() && hello["pid"].is_u64() && hello["caps"].as_array().is_some_and(|c| c.iter().all(Value::is_string)), || format!("{hello}")),
    );
    println!("      caps: {caps:?}");

    // init (9.3)
    let r = a.request(json!({"t": "init", "project_root": "/kit", "caps": [], "terminals": {"stdout": null, "stderr": null}}), &mut no_calls)?;
    kit.check("init answered with ret null", r.ret().and_then(|v| need(v.is_null(), || format!("value {v}"))));

    // An unknown t with an id (9.1).
    let r = a.request(json!({"t": "frobnicate"}), &mut no_calls)?;
    kit.check("an unknown t gets err ProtocolError", r.err().and_then(|e| need(e["type"] == "ProtocolError", || format!("{e}"))));

    // A definition (9.4, 12.2).
    let def = snippet("definition").ok_or("the kit file has no definition")?;
    // A name the block doesn't define goes to `missing`, and such a unit isn't kept (12.2).
    let r = a.request(load("definition", "0:0", def, &["echo", "nothere"], false, json!([]), json!({})), &mut no_calls)?;
    kit.check("definition: a name it lacks is missing", r.ret().and_then(|_| need(r.frame["missing"] == json!(["nothere"]), || format!("missing {}", r.frame["missing"]))));
    let r = a.request(load("definition", "0:1", def, &["echo", "fail", "say", "answer"], false, json!([]), json!({})), &mut no_calls)?;
    kit.check(
        "definition: exports, kinds and a constant's value",
        r.ret().and_then(|_| {
            let exports = r.frame["exports"].as_array().cloned().unwrap_or_default();
            let kind = |n: &str| exports.iter().find(|e| e["name"] == n).map(|e| e["kind"].clone());
            need(kind("echo") == Some(json!("func")) && kind("fail") == Some(json!("func")) && kind("say") == Some(json!("func")), || format!("exports {exports:?}"))?;
            let answer = exports.iter().find(|e| e["name"] == "answer");
            need(answer.is_some_and(|e| e["kind"] == "value" && e["value"] == json!(42)), || format!("answer: {answer:?}"))?;
            need(r.frame["missing"] == json!([]), || format!("missing {}", r.frame["missing"]))
        }),
    );

    // Values both ways (ch. 10).
    for (what, v) in echo_cases() {
        let r = a.request(call("0:1", "echo", json!([v.clone()])), &mut no_calls)?;
        kit.check(&format!("echo: {what}"), r.ret().and_then(|got| need(same(got, &v), || format!("sent {}, got {}", short(&v), short(got)))));
    }

    // A frame ended by \r\n (9.1).
    let id = a.next_id;
    a.next_id += 1;
    a.send_raw(&format!("{}\r\n", json!({"t": "call", "id": id, "target": {"unit": "0:1", "name": "echo"}, "args": [5], "kwargs": {}})))?;
    let f = a.read()?;
    kit.check("a frame ended by \\r\\n", need(f["t"] == "ret" && f["id"] == json!(id) && f["value"] == json!(5), || format!("{f}")));

    // Errors (9.6, 12.6).
    let r = a.request(call("0:1", "fail", json!(["boom"])), &mut no_calls)?;
    kit.check(
        "an error: ForeignError with the message",
        r.err().and_then(|e| need(e["type"] == "ForeignError" && e["message"].as_str().is_some_and(|m| m.contains("boom")) && e["trace"].is_array(), || format!("{e}"))),
    );
    if let Some(line) = snippets.get("fail_line").and_then(Value::as_u64) {
        let want = LINE + line - 1;
        kit.check(
            &format!("an error's trace points at {FILE}:{want}"),
            r.err().and_then(|e| {
                let frames = e["trace"].as_array().cloned().unwrap_or_default();
                need(frames.first().is_some_and(|f| f["file"] == FILE && f["line"] == json!(want)), || format!("trace {frames:?}"))
            }),
        );
    }
    let r = a.request(call("0:1", "nosuch", json!([])), &mut no_calls)?;
    kit.check("calling a name the unit lacks: err NameError", r.err().and_then(|e| need(e["type"] == "NameError", || format!("{e}"))));

    // Output (9.9).
    let r = a.request(call("0:1", "say", json!(["hi"])), &mut no_calls)?;
    kit.check("output: out frames before the reply", r.ret().and_then(|_| need(r.stdout() == "hi\n", || format!("stdout {:?}", r.stdout()))));

    // Actions (12.2).
    if let Some(src) = snippet("action_value") {
        let r = a.request(load("action", "0:2", src, &[], true, json!([]), json!({})), &mut no_calls)?;
        kit.check("an action's value", r.ret().and_then(|v| need(v == &json!(3), || format!("value {v}"))));
        let r = a.request(load("action", "0:2", src, &[], false, json!([]), json!({})), &mut no_calls)?;
        kit.check("an action without want_value gives null", r.ret().and_then(|v| need(v.is_null(), || format!("value {v}"))));
    }
    if let Some(src) = snippet("action_print") {
        let r = a.request(load("action", "0:3", src, &[], false, json!([]), json!({})), &mut no_calls)?;
        kit.check("an action's output, in order", r.ret().and_then(|_| need(r.stdout() == "hi\nthere\n", || format!("stdout {:?}", r.stdout()))));
    }
    if let (Some(src), Some(line)) = (snippet("syntax_error"), snippets.get("syntax_error_line").and_then(Value::as_u64)) {
        let want = LINE + line - 1;
        let r = a.request(load("action", "0:4", src, &[], false, json!([]), json!({})), &mut no_calls)?;
        kit.check(
            &format!("a syntax error: ForeignError SyntaxError at {FILE}:{want}"),
            r.err().and_then(|e| {
                need(e["type"] == "ForeignError" && e["foreign_type"] == "SyntaxError", || format!("{e}"))?;
                let frames = e["trace"].as_array().cloned().unwrap_or_default();
                need(frames.iter().any(|f| f["file"] == FILE && f["line"] == json!(want)), || format!("trace {frames:?}"))
            }),
        );
    }

    // modules (13.5)
    if caps.iter().any(|c| c == "modules") {
        match snippets.get("module") {
            Some(m) => {
                let (name, member) = (m["name"].as_str().unwrap_or_default(), m["member"].as_str().unwrap_or_default());
                let frame = json!({"t": "module", "unit": format!("kit:{name}"), "name": name, "from": FILE, "names": [member, "no_such_member"]});
                let r = a.request(frame, &mut no_calls)?;
                kit.check(
                    "module: exports and missing",
                    r.ret().and_then(|_| {
                        let exports = r.frame["exports"].as_array().cloned().unwrap_or_default();
                        need(exports.iter().any(|e| e["name"] == member && e["kind"] == "func"), || format!("exports {exports:?}"))?;
                        need(r.frame["missing"] == json!(["no_such_member"]), || format!("missing {}", r.frame["missing"]))
                    }),
                );
                let r = a.request(json!({"t": "module", "unit": "kit:v2_no_such_module_xyz", "name": "v2_no_such_module_xyz", "from": FILE, "names": ["x"]}), &mut no_calls)?;
                kit.check("a module that isn't there: err ImportError", r.err().and_then(|e| need(e["type"] == "ImportError", || format!("{e}"))));
            }
            None => kit.check("module", Err("the adapter has the modules cap, but the kit file has no module".into())),
        }
    }

    // reverse (9.5, 11.8, 13.7)
    if caps.iter().any(|c| c == "reverse") {
        match (snippet("apply"), snippet("route")) {
            (Some(apply), Some(route)) => {
                let r = a.request(load("definition", "0:5", apply, &["apply"], false, json!([]), json!({})), &mut no_calls)?;
                kit.check("reverse: apply loads", r.ret().map(|_| ()));
                let mut doubled = |c: &Value| {
                    let x = c["args"][0].as_i64().unwrap_or(0);
                    json!({"t": "ret", "id": c["id"], "value": x * 2})
                };
                let r = a.request(call("0:5", "apply", json!([{"$v2": "fn", "id": 1, "name": "double"}, 21])), &mut doubled)?;
                kit.check(
                    "reverse: a V2 function called back with call {fn}",
                    r.ret().and_then(|v| {
                        need(r.calls.len() == 1 && r.calls[0]["target"] == json!({"fn": 1}) && r.calls[0]["args"] == json!([21]) && r.calls[0]["kwargs"].is_object(), || format!("calls {:?}", r.calls))?;
                        need(v == &json!(42), || format!("value {v}"))
                    }),
                );
                let tag = json!({"$v2": "error", "type": "IOError", "message": "disk hiccup", "data": null, "lang": null, "foreign_type": null, "file": null, "line": null, "col": null, "trace": [], "cause": null, "ref": 7});
                let mut failing = |c: &Value| json!({"t": "err", "id": c["id"], "error": {"type": "IOError", "message": "disk hiccup", "foreign_type": null, "trace": [], "v2": tag.clone()}});
                let r = a.request(call("0:5", "apply", json!([{"$v2": "fn", "id": 2, "name": "flaky"}, 1])), &mut failing)?;
                kit.check(
                    "reverse: a V2 error goes back with its v2 tag",
                    r.err().and_then(|e| need(e["v2"]["type"] == "IOError" && e["v2"]["message"] == "disk hiccup", || format!("{e}"))),
                );
                let imports = json!([{"as": "host_double", "route": 3}]);
                let r = a.request(load("definition", "0:6", route, &["twice"], false, imports, json!({})), &mut no_calls)?;
                kit.check("reverse: a block with an imported route loads", r.ret().map(|_| ()));
                let r = a.request(call("0:6", "twice", json!([5])), &mut doubled)?;
                kit.check(
                    "reverse: an imported name calls call {route}",
                    r.ret().and_then(|v| {
                        need(r.calls.len() == 1 && r.calls[0]["target"] == json!({"route": 3}), || format!("calls {:?}", r.calls))?;
                        need(v == &json!(10), || format!("value {v}"))
                    }),
                );
                let r = a.request(json!({"t": "call", "target": {"route": 3}, "args": [], "kwargs": {}}), &mut no_calls)?;
                kit.check("a host call with a route target: err ProtocolError", r.err().and_then(|e| need(e["type"] == "ProtocolError", || format!("{e}"))));
            }
            _ => kit.check("reverse", Err("the adapter has the reverse cap, but the kit file lacks apply or route".into())),
        }
    }

    // bye (9.10)
    a.send(&json!({"t": "bye"}))?;
    let status = a.wait(Duration::from_secs(5));
    kit.check("bye: the adapter exits with 0", need(status == Some(0), || format!("status {status:?}")));
    Ok(())
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 100 {
        format!("{}… ({} bytes)", &s[..s.char_indices().take_while(|(i, _)| *i < 100).last().map_or(0, |(i, c)| i + c.len_utf8())], s.len())
    } else {
        s
    }
}

/// EOF on stdin ends the adapter quietly (9.1).
fn eof(kit: &mut Kit, argv: &[String]) -> Result<(), String> {
    let mut a = Adapter::start(argv)?;
    let _ = a.lines.recv_timeout(WAIT);
    a.close_stdin();
    let status = a.wait(Duration::from_secs(5));
    kit.check("EOF on stdin: the adapter exits with 0", need(status == Some(0), || format!("status {status:?}")));
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(sep) = args.iter().position(|a| a == "--") else {
        eprintln!("usage: vep_check KIT.json -- COMMAND [ARG ...]");
        std::process::exit(100);
    };
    let (kit_file, argv) = (&args[..sep], &args[sep + 1..]);
    if kit_file.len() != 1 || argv.is_empty() {
        eprintln!("usage: vep_check KIT.json -- COMMAND [ARG ...]");
        std::process::exit(100);
    }
    let snippets: Map<String, Value> = match std::fs::read_to_string(&kit_file[0]).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string())) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("vep_check: {}: {e}", kit_file[0]);
            std::process::exit(100);
        }
    };
    let mut kit = Kit { results: Vec::new() };
    if let Err(why) = session(&mut kit, argv, &snippets) {
        kit.check("the session", Err(why));
    }
    if let Err(why) = eof(&mut kit, argv) {
        kit.check("the EOF session", Err(why));
    }
    let failed = kit.results.iter().filter(|(_, r)| r.is_err()).count();
    println!("{} checks, {failed} failed", kit.results.len());
    std::process::exit(failed.min(100) as i32);
}
