//! The codec's VEP mode (INTERNALS 10.1-10.5): V2 values to and from frame payloads, with the
//! `$v2` tags. The plain mode of `json_parse`/`json_stringify` is `codec/json`.

#![allow(dead_code)] // sessions (roadmap 1.3) are the first callers

use std::rc::Rc;

use indexmap::IndexMap;
use serde_json::{json, Map, Number, Value as Json};

use crate::engines::handles::{CallableTable, HandleRef, ReleaseQueue};
use crate::engines::imports::Route;
use crate::engines::vep::Caps;
use crate::runtime::error::{fail, ErrorValue, Exec};
use crate::runtime::value::{Callable, Value};
use crate::syntax::span::SourceMap;

/// Lists and dicts nest at most this deep on the wire (10.5).
pub const MAX_DEPTH: usize = 256;

/// Ints beyond ±(2^53−1) travel as `int` tags (10.2).
const MAX_SAFE_INT: i64 = (1 << 53) - 1;

/// One session's side of the codec: whose handles and callables are in play.
pub struct Wire<'a> {
    pub session: usize,
    pub tag: Rc<str>,
    pub caps: Caps,
    pub releases: &'a ReleaseQueue,
    pub callables: &'a mut CallableTable,
    pub sm: &'a SourceMap,
    /// The inner `@import` routes, which `fn` tags with a `route` name (11.8, 13.6).
    pub routes: &'a [Route],
}

/// A step of the path an error names: `args[0]["when"]` (10.5).
enum Step {
    Index(usize),
    Key(Rc<str>),
}

fn path_text(root: &str, path: &[Step]) -> String {
    let mut s = root.to_string();
    for step in path {
        match step {
            Step::Index(i) => s.push_str(&format!("[{i}]")),
            Step::Key(k) => s.push_str(&format!("[{}]", Json::String(k.to_string()))),
        }
    }
    s
}

struct Encoder<'w, 'a> {
    wire: &'w mut Wire<'a>,
    root: &'w str,
    path: Vec<Step>,
    /// Containers on the current path, by address (cycles, 10.5).
    open: Vec<usize>,
    /// Callable ids added by this encode, rolled back if it fails (10.1).
    added: Vec<u64>,
    /// An error on its way back through foreign code: any other session's handle in it goes as a
    /// function, which is what keeps its place until the error comes home (11.7).
    passing: bool,
}

impl Encoder<'_, '_> {
    fn fail<T>(&self, message: String) -> Exec<T> {
        let at = path_text(self.root, &self.path);
        if at.is_empty() {
            fail("MarshalError", message)
        } else {
            fail("MarshalError", format!("{message} (at {at})"))
        }
    }

    fn enter(&mut self, addr: usize, kind: &str) -> Exec<()> {
        if self.open.contains(&addr) {
            return self.fail(format!("cannot send {kind} that contains itself"));
        }
        if self.open.len() >= MAX_DEPTH {
            return self.fail(format!("value nested deeper than {MAX_DEPTH} levels"));
        }
        self.open.push(addr);
        Ok(())
    }

    fn value(&mut self, v: &Value) -> Exec<Json> {
        Ok(match v {
            Value::Null => Json::Null,
            Value::Bool(b) => Json::Bool(*b),
            Value::Int(n) if (-MAX_SAFE_INT..=MAX_SAFE_INT).contains(n) => json!(n),
            Value::Int(n) => json!({"$v2": "int", "v": n.to_string()}),
            Value::Float(x) => match Number::from_f64(*x) {
                Some(n) => Json::Number(n),
                None => json!({"$v2": "float", "v": if x.is_nan() { "nan" } else if *x > 0.0 { "inf" } else { "-inf" }}),
            },
            Value::Str(s) => Json::String(s.to_string()),
            Value::Bytes(b) => json!({"$v2": "bytes", "b64": base64_encode(b)}),
            Value::List(l) => {
                self.enter(l.addr(), "a list")?;
                let items = l.borrow().clone();
                let mut out = Vec::with_capacity(items.len());
                for (i, item) in items.iter().enumerate() {
                    self.path.push(Step::Index(i));
                    out.push(self.value(item)?);
                    self.path.pop();
                }
                self.open.pop();
                Json::Array(out)
            }
            Value::Dict(d) => {
                self.enter(d.addr(), "a dict")?;
                let entries: Vec<(Rc<str>, Value)> = d.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                let mut out = Map::with_capacity(entries.len());
                let mut tagged = false;
                for (k, item) in &entries {
                    tagged |= &**k == "$v2";
                    self.path.push(Step::Key(k.clone()));
                    out.insert(k.to_string(), self.value(item)?);
                    self.path.pop();
                }
                self.open.pop();
                if tagged {
                    json!({"$v2": "dict", "v": out})
                } else {
                    Json::Object(out)
                }
            }
            Value::Func(f) => match &**f {
                // Home, it is the function itself, not a way back through V2 (11.8).
                Callable::Proxy { session, unit, name } if *session == self.wire.session => {
                    json!({"$v2": "fn", "unit": &**unit, "name": &**name})
                }
                _ => self.callable(v, f.name())?,
            },
            Value::Handle(h) => {
                // Another session's callable handle goes as a function its own session calls (11.6).
                if h.session != self.wire.session && (h.callable || self.passing) {
                    return self.callable(v, &h.text());
                }
                if h.session != self.wire.session {
                    return self.fail(format!(
                        "this handle belongs to engine '{}' and can't be passed to '{}' — convert it with to_v2(h) first",
                        h.tag, self.wire.tag
                    ));
                }
                json!({"$v2": "h", "id": h.id})
            }
            Value::Error(e) => self.error(e)?,
        })
    }

    /// A `Func` or another session's callable handle, registered in the callable table (11.8).
    fn callable(&mut self, v: &Value, name: &str) -> Exec<Json> {
        if !self.wire.caps.reverse {
            return self.fail(format!("engine '{}' can't receive V2 functions (no 'reverse' capability)", self.wire.tag));
        }
        let id = self.wire.callables.register(v);
        self.added.push(id);
        Ok(json!({"$v2": "fn", "id": id, "name": name}))
    }

    /// A V2 error, with its location and frames (10.3). It is a nesting level, like a list or a
    /// dict: its data and cause are one level below it (10.5).
    fn error(&mut self, e: &ErrorValue) -> Exec<Json> {
        self.enter(e as *const ErrorValue as usize, "an error")?;
        let data = self.value(&e.data)?;
        let cause = self.value(&e.cause)?;
        self.open.pop();
        let (file, line, col) = match e.span.get() {
            Some(s) => (json!(self.wire.sm.file(s.file).path.to_string_lossy()), json!(s.line), json!(s.col)),
            None => (Json::Null, Json::Null, Json::Null),
        };
        let trace: Vec<Json> = e.trace.get().map_or_else(Vec::new, |t| t.iter().map(|f| json!(&**f)).collect());
        Ok(json!({
            "$v2": "error", "type": &*e.kind, "message": &*e.message, "data": data,
            "lang": e.lang.as_deref(), "foreign_type": e.foreign_type.as_deref(),
            "file": file, "line": line, "col": col, "trace": trace, "cause": cause,
        }))
    }
}

impl Wire<'_> {
    /// `value` for this session (10.1). A failure is a `MarshalError` naming `root` (`args[0]`)
    /// and the path inside it; the callable counts it added are rolled back.
    pub fn encode(&mut self, value: &Value, root: &str) -> Exec<Json> {
        self.encode_as(value, root, false)
    }

    /// `encode` for the error answering a worker's `call` (9.6).
    pub fn encode_passing(&mut self, value: &Value, root: &str) -> Exec<Json> {
        self.encode_as(value, root, true)
    }

    fn encode_as(&mut self, value: &Value, root: &str, passing: bool) -> Exec<Json> {
        let mut enc = Encoder { wire: self, root, path: Vec::new(), open: Vec::new(), added: Vec::new(), passing };
        let result = enc.value(value);
        let added = std::mem::take(&mut enc.added);
        if result.is_err() {
            self.callables.release(&added);
        }
        result
    }

    /// A payload from this session's worker (10.1-10.3). `Err` is what is wrong with it, for a
    /// fatal `ProtocolError` (9.11).
    pub fn decode(&mut self, j: &Json) -> Result<Value, String> {
        self.decode_at(j, 0)
    }

    fn decode_at(&mut self, j: &Json, depth: usize) -> Result<Value, String> {
        Ok(match j {
            Json::Null => Value::Null,
            Json::Bool(b) => Value::Bool(*b),
            // Integer tokens that fit i64 are ints; anything else is a float (10.2).
            Json::Number(n) => match n.as_i64() {
                Some(i) if !(i == 0 && n.as_f64().is_some_and(|x| x.is_sign_negative())) => Value::Int(i),
                _ => Value::Float(n.as_f64().unwrap_or(f64::NAN)),
            },
            Json::String(s) => Value::str(s),
            Json::Array(items) => {
                if depth >= MAX_DEPTH {
                    return Err(format!("value nested deeper than {MAX_DEPTH} levels"));
                }
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(self.decode_at(item, depth + 1)?);
                }
                Value::list(out)
            }
            Json::Object(m) => match m.get("$v2") {
                Some(tag) => self.tag(tag, m, depth)?,
                None => self.dict(m, depth)?,
            },
        })
    }

    fn dict(&mut self, m: &Map<String, Json>, depth: usize) -> Result<Value, String> {
        if depth >= MAX_DEPTH {
            return Err(format!("value nested deeper than {MAX_DEPTH} levels"));
        }
        let mut out = IndexMap::with_capacity(m.len());
        for (k, v) in m {
            out.insert(Rc::from(k.as_str()), self.decode_at(v, depth + 1)?);
        }
        Ok(Value::dict(out))
    }

    fn tag(&mut self, tag: &Json, m: &Map<String, Json>, depth: usize) -> Result<Value, String> {
        let name = tag.as_str().ok_or("a '$v2' tag that is not a string")?;
        let get = |member: &str| m.get(member).ok_or_else(|| format!("a '{name}' tag without '{member}'"));
        let text = |member: &str| -> Result<&str, String> { get(member)?.as_str().ok_or_else(|| format!("a '{name}' tag with a bad '{member}'")) };
        let uint = |member: &str| -> Result<u64, String> { get(member)?.as_u64().ok_or_else(|| format!("a '{name}' tag with a bad '{member}'")) };
        Ok(match name {
            "int" => Value::Int(text("v")?.parse().map_err(|_| "an 'int' tag outside the 64-bit range")?),
            "float" => Value::Float(match text("v")? {
                "nan" => f64::NAN,
                "inf" => f64::INFINITY,
                "-inf" => f64::NEG_INFINITY,
                _ => return Err("a 'float' tag with a bad 'v'".into()),
            }),
            "bytes" => Value::Bytes(base64_decode(text("b64")?).ok_or("a 'bytes' tag with bad base64")?.into()),
            "dict" => self.dict(get("v")?.as_object().ok_or("a 'dict' tag with a bad 'v'")?, depth)?,
            "h" => {
                if !self.caps.handles {
                    return Err("an 'h' tag from a worker without the 'handles' capability".into());
                }
                let callable = m.get("callable") == Some(&Json::Bool(true));
                let h = HandleRef::new(self.session, self.tag.clone(), uint("id")?, text("type")?.into(), self.releases.clone()).with_callable(callable);
                Value::Handle(Rc::new(h))
            }
            "fn" => match (m.get("id").and_then(Json::as_u64), m.get("route").and_then(Json::as_u64)) {
                (Some(id), None) => self.callables.get(id).ok_or_else(|| format!("an 'fn' tag with an unknown id {id}"))?,
                (None, Some(r)) => {
                    let n = usize::try_from(r).ok().filter(|&n| n < self.routes.len());
                    let n = n.ok_or_else(|| format!("an 'fn' tag with an unknown route {r}"))?;
                    self.routes[n].value(n)
                }
                _ => return Err("an 'fn' tag without 'id' or 'route'".into()),
            },
            "error" => Value::Error(Rc::new(self.error(m, depth)?)),
            other => return Err(format!("an unknown '$v2' tag '{other}'")),
        })
    }

    /// An `error` tag back to an `ErrorValue`: its location if the file is one of this run's (10.3).
    fn error(&mut self, m: &Map<String, Json>, depth: usize) -> Result<ErrorValue, String> {
        if depth >= MAX_DEPTH {
            return Err(format!("value nested deeper than {MAX_DEPTH} levels"));
        }
        let text = |member: &str| m.get(member).and_then(Json::as_str).ok_or_else(|| format!("an 'error' tag with a bad '{member}'"));
        let optional = |member: &str| m.get(member).and_then(Json::as_str).map(Rc::<str>::from);
        let data = match m.get("data") {
            Some(d) => self.decode_at(d, depth + 1)?,
            None => Value::Null,
        };
        let mut e = ErrorValue::with_data(text("type")?.into(), text("message")?.into(), data);
        e.lang = optional("lang");
        e.foreign_type = optional("foreign_type");
        if let Some(c) = m.get("cause").filter(|c| !c.is_null()) {
            e.cause = self.decode_at(c, depth + 1)?;
        }
        let file = optional("file").and_then(|f| self.sm.id_of(std::path::Path::new(&*f)));
        let line = m.get("line").and_then(Json::as_u64).and_then(|n| u32::try_from(n).ok());
        let col = m.get("col").and_then(Json::as_u64).and_then(|n| u32::try_from(n).ok());
        if let (Some(file), Some(line), Some(col)) = (file, line, col) {
            if let Some(span) = self.sm.span_at(file, line, col) {
                let _ = e.span.set(span);
            }
        }
        if let Some(frames) = m.get("trace").and_then(Json::as_array).filter(|f| !f.is_empty()) {
            let frames: Vec<Rc<str>> = frames.iter().filter_map(Json::as_str).map(Rc::from).collect();
            let _ = e.trace.set(frames.into());
        }
        Ok(e)
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The callable ids of every `fn` tag in an encoded frame, one per tag: what a frame that was
/// never sent must give back to the callable table (10.1, 11.8).
/// The handle ids a frame names: its `h` tags, and an `op`'s or a call target's `h` (11.2). A user's
/// dict key `h` counts too, which can only delay a release.
pub fn handle_ids(frame: &Json) -> std::collections::HashSet<u64> {
    fn walk(j: &Json, out: &mut std::collections::HashSet<u64>) {
        match j {
            Json::Array(items) => items.iter().for_each(|i| walk(i, out)),
            Json::Object(m) => match m.get("$v2").and_then(Json::as_str) {
                Some("h") => out.extend(m.get("id").and_then(Json::as_u64)),
                Some("dict") => m.get("v").and_then(Json::as_object).iter().flat_map(|v| v.values()).for_each(|v| walk(v, out)),
                Some("error") => ["data", "cause"].iter().filter_map(|k| m.get(*k)).for_each(|v| walk(v, out)),
                Some(_) => {}
                None => {
                    out.extend(m.get("h").and_then(Json::as_u64));
                    m.values().for_each(|v| walk(v, out));
                }
            },
            _ => {}
        }
    }
    let mut out = std::collections::HashSet::new();
    walk(frame, &mut out);
    out
}

pub fn fn_ids(frame: &Json) -> Vec<u64> {
    fn walk(j: &Json, out: &mut Vec<u64>) {
        match j {
            Json::Array(items) => items.iter().for_each(|i| walk(i, out)),
            Json::Object(m) => match m.get("$v2").and_then(Json::as_str) {
                Some("fn") => out.extend(m.get("id").and_then(Json::as_u64)),
                // A `dict` tag's `v` holds the user's own keys, which may include "$v2".
                Some("dict") => m.get("v").and_then(Json::as_object).iter().flat_map(|v| v.values()).for_each(|v| walk(v, out)),
                Some("error") => ["data", "cause"].iter().filter_map(|k| m.get(*k)).for_each(|v| walk(v, out)),
                Some(_) => {}
                None => m.values().for_each(|v| walk(v, out)),
            },
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(frame, &mut out);
    out
}

/// Standard, padded base64 (10.1).
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Strict decoding: padded, no whitespace, zero padding bits.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let b = text.as_bytes();
    if !b.len().is_multiple_of(4) {
        return None;
    }
    let value = |c: u8| B64.iter().position(|&x| x == c).map(|p| p as u32);
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for (q, chunk) in b.chunks(4).enumerate() {
        let last = q == b.len() / 4 - 1;
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &chunk[..4 - pad] {
            n = n << 6 | value(c)?;
        }
        n <<= 6 * pad as u32;
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        let keep = 3 - pad;
        if bytes[keep..].iter().any(|&x| x != 0) {
            return None; // non-zero padding bits
        }
        out.extend_from_slice(&bytes[..keep]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::builtins::BUILTINS;
    use crate::runtime::value::{Callable, Func};
    use crate::syntax::span::FileId;
    use std::cell::RefCell;

    struct Fixture {
        releases: ReleaseQueue,
        callables: CallableTable,
        sm: SourceMap,
        routes: Vec<Route>,
    }

    impl Fixture {
        fn new() -> Fixture {
            let routes = vec![Route::Target(Value::str("route zero")), Route::V2 { file: FileId(0), name: "log".into() }];
            Fixture { releases: Rc::new(RefCell::new(Vec::new())), callables: CallableTable::default(), sm: SourceMap::new(), routes }
        }

        fn wire(&mut self, caps: Caps) -> Wire<'_> {
            Wire {
                session: 0,
                tag: "py".into(),
                caps,
                releases: &self.releases,
                callables: &mut self.callables,
                sm: &self.sm,
                routes: &self.routes,
            }
        }
    }

    const ALL: Caps = Caps { modules: true, handles: true, kwargs: true, reverse: true };

    fn enc(v: &Value) -> String {
        Fixture::new().wire(ALL).encode(v, "value").map(|j| j.to_string()).unwrap_or_else(err_text)
    }

    fn err_text(f: crate::runtime::error::Flow) -> String {
        match f {
            crate::runtime::error::Flow::Error(e) => format!("{}: {}", e.kind, e.message),
            _ => "?".into(),
        }
    }

    fn dec(text: &str) -> Result<Value, String> {
        // As frames are parsed: serde_json's own recursion limit (128) is off (9.1, 10.5).
        let mut de = serde_json::Deserializer::from_str(text);
        de.disable_recursion_limit();
        let j: Json = de.into_iter().next().unwrap().unwrap();
        Fixture::new().wire(ALL).decode(&j)
    }

    #[test]
    fn numbers_keep_their_kind() {
        assert_eq!(enc(&Value::Float(1.0)), "1.0");
        assert_eq!(enc(&Value::Float(-0.0)), "-0.0");
        assert_eq!(enc(&Value::Int((1 << 53) - 1)), "9007199254740991");
        assert_eq!(enc(&Value::Int(1 << 53)), r#"{"$v2":"int","v":"9007199254740992"}"#);
        assert_eq!(enc(&Value::Int(i64::MIN)), r#"{"$v2":"int","v":"-9223372036854775808"}"#);
        assert_eq!(enc(&Value::Float(f64::NAN)), r#"{"$v2":"float","v":"nan"}"#);
        assert_eq!(enc(&Value::Float(f64::NEG_INFINITY)), r#"{"$v2":"float","v":"-inf"}"#);
        assert!(matches!(dec("1.0"), Ok(Value::Float(x)) if x == 1.0));
        assert!(matches!(dec("-0.0"), Ok(Value::Float(x)) if x == 0.0 && x.is_sign_negative()));
        assert!(matches!(dec("7"), Ok(Value::Int(7))));
        assert!(matches!(dec("9223372036854775807"), Ok(Value::Int(i64::MAX))));
        assert!(matches!(dec("9223372036854775808"), Ok(Value::Float(_))), "outside i64: the nearest float");
        assert!(matches!(dec(r#"{"$v2":"int","v":"-9223372036854775808"}"#), Ok(Value::Int(i64::MIN))));
        assert_eq!(dec(r#"{"$v2":"int","v":"9223372036854775808"}"#).err().unwrap(), "an 'int' tag outside the 64-bit range");
        assert!(matches!(dec(r#"{"$v2":"float","v":"inf"}"#), Ok(Value::Float(x)) if x == f64::INFINITY));
    }

    #[test]
    fn strings_bytes_and_dicts() {
        // Control characters are escaped; U+2028 may travel raw (9.1).
        assert_eq!(enc(&Value::str("\r\n\u{2028}\0\"")), "\"\\r\\n\u{2028}\\u0000\\\"\"");
        assert_eq!(enc(&Value::Bytes(b"\x00A\xff".to_vec().into())), r#"{"$v2":"bytes","b64":"AEH/"}"#);
        assert!(matches!(dec(r#"{"$v2":"bytes","b64":"AEH/"}"#), Ok(Value::Bytes(b)) if &*b == b"\x00A\xff"));
        let mut m = IndexMap::new();
        m.insert(Rc::from("$v2"), Value::Int(1));
        m.insert(Rc::from("b"), Value::Null);
        assert_eq!(enc(&Value::dict(m)), r#"{"$v2":"dict","v":{"$v2":1,"b":null}}"#);
        let d = dec(r#"{"$v2":"dict","v":{"$v2":"h","x":{"$v2":"int","v":"5"}}}"#).unwrap();
        let Value::Dict(d) = d else { panic!() };
        assert!(matches!(d.borrow().get("$v2"), Some(Value::Str(s)) if &**s == "h"), "keys of v are literal");
        assert!(matches!(d.borrow().get("x"), Some(Value::Int(5))), "values are decoded");
    }

    #[test]
    fn base64_is_strict() {
        for (raw, text) in [(&b""[..], ""), (b"f", "Zg=="), (b"fo", "Zm8="), (b"foo", "Zm9v"), (b"foob", "Zm9vYg==")] {
            assert_eq!(base64_encode(raw), text);
            assert_eq!(base64_decode(text).as_deref(), Some(raw));
        }
        for bad in ["Zg", "Zg=", "Z===", "Zh==", "Zm9v Yg==", "Zg==Zg==", "Zm9!"] {
            assert_eq!(base64_decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn depth_256_both_ways() {
        let nested = |n: usize| {
            let mut v = Value::Null;
            for _ in 0..n {
                v = Value::list(vec![v]);
            }
            v
        };
        assert!(Fixture::new().wire(ALL).encode(&nested(256), "x").is_ok());
        assert_eq!(enc(&nested(257)), "MarshalError: value nested deeper than 256 levels (at value".to_string() + &"[0]".repeat(256) + ")");
        let text = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(dec(&text(256)).is_ok());
        assert_eq!(dec(&text(257)).err().unwrap(), "value nested deeper than 256 levels");
    }

    #[test]
    fn cycles_and_paths() {
        let l = Value::list(vec![Value::Int(1)]);
        if let Value::List(inner) = &l {
            inner.borrow_mut().push(l.clone());
        }
        assert_eq!(enc(&l), "MarshalError: cannot send a list that contains itself (at value[1])");
        if let Value::List(inner) = &l {
            inner.borrow_mut().clear(); // break the cycle for the drop
        }
        // Shared (not cyclic) substructure is fine and copied per occurrence.
        let shared = Value::list(vec![Value::Int(2)]);
        assert_eq!(enc(&Value::list(vec![shared.clone(), shared])), "[[2],[2]]");
    }

    #[test]
    fn handles_belong_to_their_session() {
        let mut fx = Fixture::new();
        let h = Value::Handle(Rc::new(HandleRef::new(0, "py".into(), 9, "datetime.date".into(), fx.releases.clone())));
        let js = Value::Handle(Rc::new(HandleRef::new(1, "js".into(), 3, "Date".into(), fx.releases.clone())));
        let mut m = IndexMap::new();
        m.insert(Rc::from("when"), js);
        let args = Value::list(vec![Value::dict(m)]);
        assert_eq!(fx.wire(ALL).encode(&h, "args[0]").unwrap().to_string(), r#"{"$v2":"h","id":9}"#);
        let e = fx.wire(ALL).encode(&args, "args").err().map(err_text).unwrap();
        assert_eq!(e, "MarshalError: this handle belongs to engine 'js' and can't be passed to 'py' — convert it with to_v2(h) first (at args[0][\"when\"])");
        // Decoding: one HandleRef per tag; without the handles cap a tag is fatal.
        let tag: Json = serde_json::from_str(r#"{"$v2":"h","id":4,"type":"datetime.date"}"#).unwrap();
        let v = fx.wire(ALL).decode(&tag).unwrap();
        let Value::Handle(r) = &v else { panic!() };
        assert_eq!((r.id, &*r.type_name, &*r.tag), (4, "datetime.date", "py"));
        drop(v);
        assert!(fx.releases.borrow().contains(&4), "dropping it queues a release");
        let no_handles = Caps { handles: false, ..ALL };
        assert_eq!(fx.wire(no_handles).decode(&tag).err().unwrap(), "an 'h' tag from a worker without the 'handles' capability");
    }

    #[test]
    fn functions_need_reverse_reuse_ids_and_roll_back() {
        let mut fx = Fixture::new();
        let print = Value::Func(Func::new(Callable::Builtin(BUILTINS.iter().find(|b| b.name == "print").unwrap())));
        let j = fx.wire(ALL).encode(&Value::list(vec![print.clone(), print.clone()]), "args").unwrap();
        assert_eq!(j.to_string(), r#"[{"$v2":"fn","id":1,"name":"print"},{"$v2":"fn","id":1,"name":"print"}]"#);
        assert_eq!(fx.callables.count(1), 2);
        // Back from the worker: the same function, counts unchanged.
        let back: Json = serde_json::from_str(r#"{"$v2":"fn","id":1}"#).unwrap();
        assert!(matches!(fx.wire(ALL).decode(&back), Ok(Value::Func(_))));
        assert_eq!(fx.callables.count(1), 2);
        // A route stub: a resolved route's target, else a function that resolves it (11.8).
        let route = |r: u64| json!({"$v2": "fn", "route": r});
        assert!(matches!(fx.wire(ALL).decode(&route(0)), Ok(Value::Str(s)) if &*s == "route zero"));
        let Ok(Value::Func(f)) = fx.wire(ALL).decode(&route(1)) else { panic!("a function") };
        assert!(matches!(&*f, Callable::Route { route: 1, name } if &**name == "log"));
        assert_eq!(fx.wire(ALL).decode(&route(2)).err().unwrap(), "an 'fn' tag with an unknown route 2");
        // Another session's callable handle goes as a function; a handle of this session as itself.
        let other = Value::Handle(Rc::new(HandleRef::new(1, "js".into(), 4, "function".into(), fx.releases.clone()).with_callable(true)));
        let mine = Value::Handle(Rc::new(HandleRef::new(0, "py".into(), 4, "function".into(), fx.releases.clone()).with_callable(true)));
        let j = fx.wire(ALL).encode(&Value::list(vec![other, mine]), "args").unwrap();
        assert_eq!(j.to_string(), r#"[{"$v2":"fn","id":2,"name":"<js function>"},{"$v2":"h","id":4}]"#);
        assert_eq!(fx.wire(ALL).decode(&serde_json::from_str(r#"{"$v2":"fn","id":77}"#).unwrap()).err().unwrap(), "an 'fn' tag with an unknown id 77");
        // A failed encode rolls back the counts it added.
        let bad = Value::list(vec![print.clone(), Value::Handle(Rc::new(HandleRef::new(1, "js".into(), 1, "x".into(), fx.releases.clone())))]);
        assert!(fx.wire(ALL).encode(&bad, "args").is_err());
        assert_eq!(fx.callables.count(1), 2);
        let no_reverse = Caps { reverse: false, ..ALL };
        let e = fx.wire(no_reverse).encode(&print, "args[0]").err().map(err_text).unwrap();
        assert_eq!(e, "MarshalError: engine 'py' can't receive V2 functions (no 'reverse' capability) (at args[0])");
    }

    #[test]
    fn errors_round_trip() {
        let mut fx = Fixture::new();
        let file = fx.sm.add(std::path::PathBuf::from("C:/w/main.v2"), "main.v2".into(), "let x = 1\nthrow error(\"ValueError\", \"bad\")\n");
        let mut e = ErrorValue::with_data("ValueError".into(), "bad".into(), Value::Int(3));
        let _ = e.span.set(fx.sm.span_at(file, 2, 7).unwrap());
        let _ = e.trace.set(vec![Rc::from("main.v2:2")].into());
        e.cause = Value::Error(Rc::new(ErrorValue::new("KeyError", "k")));
        let j = fx.wire(ALL).encode(&Value::Error(Rc::new(e)), "value").unwrap();
        assert_eq!(j["$v2"], "error");
        assert_eq!((j["file"].as_str(), j["line"].as_u64(), j["col"].as_u64()), (Some("C:/w/main.v2"), Some(2), Some(7)));
        assert_eq!(j["cause"]["type"], "KeyError");
        let back = fx.wire(ALL).decode(&j).unwrap();
        let Value::Error(b) = back else { panic!() };
        assert_eq!((&*b.kind, &*b.message), ("ValueError", "bad"));
        assert!(matches!(b.data, Value::Int(3)));
        let span = *b.span.get().unwrap();
        assert_eq!((span.file, span.line, span.col), (file, 2, 7));
        assert_eq!(b.trace.get().unwrap().len(), 1);
        assert!(matches!(&b.cause, Value::Error(c) if &*c.kind == "KeyError"));
        // A file this run doesn't know leaves the error without a location.
        let mut other = j.clone();
        other["file"] = json!("C:/elsewhere.v2");
        let Value::Error(o) = fx.wire(ALL).decode(&other).unwrap() else { panic!() };
        assert!(o.span.get().is_none());
    }

    #[test]
    fn malformed_tags_are_fatal() {
        for (text, what) in [
            (r#"{"$v2":"nope"}"#, "an unknown '$v2' tag 'nope'"),
            (r#"{"$v2":1}"#, "a '$v2' tag that is not a string"),
            (r#"{"$v2":"int"}"#, "a 'int' tag without 'v'"),
            (r#"{"$v2":"float","v":"NaN"}"#, "a 'float' tag with a bad 'v'"),
            (r#"{"$v2":"bytes","b64":"!"}"#, "a 'bytes' tag with bad base64"),
            (r#"{"$v2":"dict","v":[]}"#, "a 'dict' tag with a bad 'v'"),
            (r#"{"$v2":"h","id":1}"#, "a 'h' tag without 'type'"),
            (r#"{"$v2":"fn"}"#, "an 'fn' tag without 'id' or 'route'"),
            (r#"{"$v2":"error","type":"E"}"#, "an 'error' tag with a bad 'message'"),
        ] {
            assert_eq!(dec(text).err().as_deref(), Some(what), "{text}");
        }
    }

    #[test]
    fn errors_are_a_level() {
        // 10.5: an error is a level like a list, so its data has 255 left, both ways.
        let deep = |lists: usize| {
            let mut data = Value::Null;
            for _ in 0..lists {
                data = Value::list(vec![data]);
            }
            Value::Error(Rc::new(ErrorValue::with_data("ValueError".into(), "deep".into(), data)))
        };
        let mut f = Fixture::new();
        let j = f.wire(ALL).encode(&deep(255), "value").expect("256 levels go out");
        assert!(f.wire(ALL).decode(&j).is_ok(), "and come back");
        assert_eq!(enc(&deep(256)).split(" (").next(), Some("MarshalError: value nested deeper than 256 levels"));
        let tag = json!({"$v2": "error", "type": "K", "message": "m", "data": j});
        assert_eq!(f.wire(ALL).decode(&tag).err().as_deref(), Some("value nested deeper than 256 levels"));
    }

    #[test]
    fn dict_tags_fit_the_frame_scan() {
        // 140 dicts keyed "$v2" are 140 levels but 280 brackets: frames allow 1,000 (10.1).
        let mut v = Value::Null;
        for _ in 0..140 {
            let mut m = IndexMap::new();
            m.insert(Rc::from("$v2"), v);
            v = Value::dict(m);
        }
        let mut f = Fixture::new();
        let j = f.wire(ALL).encode(&v, "value").unwrap();
        let frame = json!({"t":"ret","id":1,"value":j}).to_string();
        let parsed = std::thread::Builder::new().stack_size(64 << 20).spawn(move || crate::engines::vep::parse_frame(frame.as_bytes(), ALL).is_ok());
        assert!(parsed.unwrap().join().unwrap());
    }

    #[test]
    fn fn_ids_count_tags_not_user_keys() {
        let print = Value::Func(Func::new(Callable::Builtin(BUILTINS.iter().find(|b| b.name == "print").unwrap())));
        let mut lookalike = IndexMap::new();
        lookalike.insert(Rc::from("$v2"), Value::str("fn"));
        lookalike.insert(Rc::from("id"), Value::Int(99));
        lookalike.insert(Rc::from("f"), print.clone());
        let e = Value::Error(Rc::new(ErrorValue::with_data("ValueError".into(), "x".into(), print.clone())));
        let v = Value::list(vec![print.clone(), Value::dict(lookalike), e]);
        let mut f = Fixture::new();
        let j = f.wire(ALL).encode(&v, "value").unwrap();
        let ids = fn_ids(&json!({"t":"ret","id":1,"value":j}));
        assert_eq!(ids.len(), 3, "in the list, in the dict, in the error: {ids:?}");
        assert!(ids.iter().all(|&i| i == ids[0] && i != 99), "{ids:?}");
    }

    #[test]
    fn dicts_count_toward_depth_and_negative_safe_ints_are_plain() {
        let text = |n: usize| format!("{}null{}", "{\"a\":".repeat(n), "}".repeat(n));
        assert!(dec(&text(256)).is_ok());
        assert_eq!(dec(&text(257)).err().unwrap(), "value nested deeper than 256 levels");
        assert_eq!(enc(&Value::Int(-((1 << 53) - 1))), "-9007199254740991");
        assert_eq!(enc(&Value::Int(-(1 << 53))), r#"{"$v2":"int","v":"-9007199254740992"}"#);
        assert_eq!(enc(&Value::Int(-5)), "-5");
    }
}
