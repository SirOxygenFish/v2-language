//! A scripted VEP/1 adapter for `tests/custom` (INTERNALS 22.3): a real child process speaking
//! the protocol of chapter 9, built as a Cargo example and never shipped.
//!
//! Its "language" is one JSON object per block. In a definition block each key is an export,
//! a function whose behavior its value gives:
//!   * `"$N"`: returns its Nth argument (`"echo": "$0"`)
//!   * `"!text"`: raises a ForeignError with that message (`"fail": "!boom"`)
//!   * `"@out:text"`: prints `text` and a line break, returns null (`"say": "@out:hi"`)
//!   * anything else: returns that value (`"answer": 42`)
//! An action block does the same for each entry in order (printing, failing) and its value is
//! the `"value"` member, or null. A module (`fake.name`) exports each requested name as a
//! function returning its name.
//!
//! The first argument picks the mode (default `ok`): `ok`, `garbage` (a non-JSON line instead
//! of the first reply), `vep2` (`hello` with `vep: 2`), `nohello` (silent), `noise` (lines
//! before `hello`), `crash` (exits 3 at the first call), `slow` (each call sleeps 30 s) or
//! `nocaps` (`hello` with no caps; the others declare `modules` and `kwargs`).

use std::collections::HashMap;
use std::io::{BufRead, Write};

use serde_json::{json, Map, Value};

fn send(out: &mut impl Write, frame: Value) {
    let mut line = serde_json::to_vec(&frame).expect("a frame encodes");
    line.push(b'\n');
    if out.write_all(&line).and_then(|_| out.flush()).is_err() {
        std::process::exit(0); // the host is gone (9.1)
    }
}

fn foreign(message: &str) -> Value {
    json!({"type": "ForeignError", "message": message, "foreign_type": "FakeError", "trace": []})
}

/// Runs one behavior; `Ok(value)` or `Err(error)`. Output goes out as `out` frames first.
fn behave(out: &mut impl Write, how: &Value, args: &[Value]) -> Result<Value, Value> {
    if let Some(text) = how.as_str() {
        if let Some(n) = text.strip_prefix('$').and_then(|n| n.parse::<usize>().ok()) {
            return Ok(args.get(n).cloned().unwrap_or(Value::Null));
        }
        if let Some(msg) = text.strip_prefix('!') {
            return Err(foreign(msg));
        }
        if let Some(said) = text.strip_prefix("@out:") {
            send(out, json!({"t": "out", "stream": "stdout", "text": format!("{said}\n")}));
            return Ok(Value::Null);
        }
    }
    Ok(how.clone())
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "ok".into());
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    match mode.as_str() {
        "nohello" => {
            // Never a word: the host's startup_timeout ends it (8.4).
            std::thread::sleep(std::time::Duration::from_secs(3600));
            return;
        }
        "noise" => {
            let _ = out.write_all(b"preload noise before hello\nmore noise\n");
        }
        _ => {}
    }
    let vep = if mode == "vep2" { 2 } else { 1 };
    let caps: Vec<&str> = if mode == "nocaps" { Vec::new() } else { vec!["modules", "kwargs"] };
    send(&mut out, json!({"t": "hello", "vep": vep, "version": "1.0.0", "pid": std::process::id(), "caps": caps}));

    // unit id → its exports' behaviors
    let mut units: HashMap<String, Map<String, Value>> = HashMap::new();
    let mut replied = false;
    let mut line = Vec::new();
    loop {
        line.clear();
        match input.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return, // EOF: the host is gone (9.1)
            Ok(_) => {}
        }
        let Ok(m) = serde_json::from_slice::<Value>(&line) else {
            eprintln!("fake_adapter: a line it can't parse");
            std::process::exit(2);
        };
        let t = m["t"].as_str().unwrap_or_default().to_string();
        if t == "bye" {
            return;
        }
        if t == "release" {
            continue;
        }
        let id = m["id"].clone();
        if mode == "garbage" && replied {
            let _ = out.write_all(b"this is not a frame\n");
            let _ = out.flush();
            continue;
        }
        replied = true;
        let reply: Result<Value, Value> = match t.as_str() {
            "init" => Ok(json!({"value": null})),
            "load" => {
                let source = m["source"].as_str().unwrap_or_default();
                let script: Map<String, Value> = match serde_json::from_str::<Value>(source.trim()) {
                    Ok(Value::Object(o)) => o,
                    Ok(Value::Null) | Err(_) if source.trim().is_empty() => Map::new(),
                    _ => {
                        let e = json!({"type": "ForeignError", "message": "a fake block is one JSON object", "foreign_type": "SyntaxError",
                                       "trace": [{"file": m["file"], "line": m["line"], "col": null, "func": null}]});
                        send(&mut out, json!({"t": "err", "id": id, "error": e}));
                        continue;
                    }
                };
                if m["kind"] == "definition" {
                    let wanted: Vec<String> = m["exports"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
                    let star = wanted.iter().any(|n| n == "*");
                    let names: Vec<String> = if star { script.keys().cloned().collect() } else { wanted.clone() };
                    let missing: Vec<&String> = names.iter().filter(|n| !script.contains_key(*n)).collect();
                    let exports: Vec<Value> = names.iter().filter(|n| script.contains_key(*n)).map(|n| json!({"name": n, "kind": "func"})).collect();
                    let reply = json!({"value": null, "exports": exports, "missing": missing});
                    if missing.is_empty() {
                        units.insert(m["unit"].as_str().unwrap_or_default().to_string(), script);
                    }
                    Ok(reply)
                } else {
                    let mut failed = None;
                    for (key, how) in &script {
                        if key == "value" {
                            continue;
                        }
                        if let Err(e) = behave(&mut out, how, &[]) {
                            failed = Some(e);
                            break;
                        }
                    }
                    match failed {
                        Some(e) => Err(e),
                        None => {
                            let value = if m["want_value"] == true { script.get("value").cloned().unwrap_or(Value::Null) } else { Value::Null };
                            Ok(json!({"value": value, "exports": [], "missing": []}))
                        }
                    }
                }
            }
            "module" => {
                let names: Vec<Value> = m["names"].as_array().cloned().unwrap_or_default();
                let mut script = Map::new();
                for n in names.iter().filter_map(Value::as_str) {
                    script.insert(n.to_string(), json!(n));
                }
                let exports: Vec<Value> = script.keys().map(|n| json!({"name": n, "kind": "func"})).collect();
                units.insert(m["unit"].as_str().unwrap_or_default().to_string(), script);
                Ok(json!({"value": null, "exports": exports, "missing": []}))
            }
            "call" => {
                if mode == "crash" {
                    std::process::exit(3);
                }
                if mode == "slow" {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                }
                let unit = m["target"]["unit"].as_str().unwrap_or_default();
                let name = m["target"]["name"].as_str().unwrap_or_default();
                let mut args: Vec<Value> = m["args"].as_array().cloned().unwrap_or_default();
                // Named args come after the positional ones, in order.
                args.extend(m["kwargs"].as_object().into_iter().flat_map(|o| o.values().cloned()));
                match units.get(unit).and_then(|u| u.get(name)).cloned() {
                    Some(how) => behave(&mut out, &how, &args).map(|v| json!({"value": v})),
                    None => Err(json!({"type": "NameError", "message": format!("'{name}' is not defined"), "foreign_type": null, "trace": []})),
                }
            }
            other => Err(json!({"type": "ProtocolError", "message": format!("fake_adapter doesn't handle '{other}'"), "foreign_type": null, "trace": []})),
        };
        let frame = match reply {
            Ok(Value::Object(mut body)) => {
                body.insert("t".into(), json!("ret"));
                body.insert("id".into(), id);
                // `t` first, as writers put it (9.1).
                let mut f = Map::new();
                f.insert("t".into(), json!("ret"));
                f.extend(body);
                Value::Object(f)
            }
            Ok(v) => json!({"t": "ret", "id": id, "value": v}),
            Err(e) => json!({"t": "err", "id": id, "error": e}),
        };
        send(&mut out, frame);
    }
}
