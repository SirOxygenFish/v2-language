//! JSON on serde_json (INTERNALS 10.1, 10.2). The plain mode, for `json_parse` and
//! `json_stringify` (5.10); the VEP mode with `$v2` tags (10.3) arrives with the engines.

use std::rc::Rc;

use serde_json::{Map, Number};

use crate::runtime::error::{type_error, value_error, Exec, Flow};
use crate::runtime::value::{float_str, quoted, DictMap, Value};

/// Past this many nested brackets the codec refuses input and values (10.1).
pub const MAX_JSON_DEPTH: usize = 1_000;

fn invalid<T>(line: usize, col: usize, what: &str) -> Exec<T> {
    value_error(format!("invalid JSON at line {line} column {col}: {what}"))
}

/// The 10.1 byte scan, outside strings: bracket nesting past 1,000 levels, and integer tokens
/// outside i64 (serde_json would already have made them floats). Columns count chars.
fn scan(text: &str) -> Exec<()> {
    let b = text.as_bytes();
    let (mut line, mut col) = (1usize, 0usize);
    let mut depth = 0usize;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c & 0xC0 != 0x80 {
            col += 1;
        }
        match c {
            b'\n' => {
                line += 1;
                col = 0;
            }
            b'"' => {
                // Skip the string; serde_json reports anything wrong inside it.
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    let step = if b[i] == b'\\' { 2 } else { 1 };
                    for &byte in &b[i..(i + step).min(b.len())] {
                        if byte == b'\n' {
                            line += 1;
                            col = 0;
                        } else if byte & 0xC0 != 0x80 {
                            col += 1;
                        }
                    }
                    i += step;
                }
                col += 1;
            }
            b'[' | b'{' => {
                depth += 1;
                if depth > MAX_JSON_DEPTH {
                    return invalid(line, col, &format!("nested deeper than {MAX_JSON_DEPTH} levels"));
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            b'-' | b'0'..=b'9' => {
                let (start, start_col) = (i, col);
                while i + 1 < b.len() && matches!(b[i + 1], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') {
                    i += 1;
                    col += 1;
                }
                let token = &text[start..=i];
                let digits = token.strip_prefix('-').unwrap_or(token);
                let integral = !digits.is_empty() && digits.bytes().all(|d| d.is_ascii_digit());
                if integral && token.parse::<i64>().is_err() {
                    // Valid JSON, but not V2 data: an id of 20 digits is common (10.2).
                    return value_error(format!("JSON at line {line} column {start_col} holds an integer beyond V2's 64-bit range (the JSON itself is fine): parse it in a @py block, as in @py(text) {{ import json; json.loads(text) }}, or have the number written as a string"));
                }
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

/// `json_parse(text)` (5.10): integers that fit i64 → int, anything with `.`/`e`/`E` → float,
/// duplicate keys → the last value at the first key's position; `$v2` keys are plain data. One
/// leading byte-order mark is skipped, as RFC 8259 allows (a file a block read with Python's
/// `open()` keeps the one Windows tools write; `read_file` drops it).
pub fn parse(text: &str) -> Exec<Value> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    scan(text)?;
    let mut de = serde_json::Deserializer::from_str(text);
    de.disable_recursion_limit();
    let mut values = de.into_iter::<serde_json::Value>();
    let parsed = match values.next() {
        Some(Ok(v)) => v,
        Some(Err(e)) => return serde_error(text, &e),
        // Nothing but whitespace (an empty file, a program that printed nothing).
        None => return value_error("invalid JSON: the text is empty"),
    };
    // Exactly one value: anything but JSON whitespace after it is an error.
    let end = values.byte_offset();
    let json_space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r');
    if let Some((k, _)) = text[end..].char_indices().find(|(_, c)| !json_space(*c)) {
        let before = &text[..end + k];
        let line = before.matches('\n').count() + 1;
        let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        return invalid(line, col, "trailing characters");
    }
    Ok(from_json(parsed))
}

/// serde_json counts columns in bytes; V2 counts chars (2.2).
fn serde_error<T>(text: &str, e: &serde_json::Error) -> Exec<T> {
    let full = e.to_string();
    let what = full.rsplit_once(" at line ").map_or(full.as_str(), |(w, _)| w);
    let start = text.split_inclusive('\n').take(e.line().saturating_sub(1)).map(str::len).sum::<usize>();
    let mut end = (start + e.column()).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    let col = text.get(start..end).map_or(e.column(), |s| s.chars().count());
    // `"\ud800"`: half of a surrogate pair, which serde_json words as a hex escape problem.
    let what = match what {
        "unexpected end of hex escape" | "lone leading surrogate in hex escape" => "a \\u escape of half a surrogate pair (\\ud800 to \\udfff) without its other half, which isn't text",
        w => w,
    };
    invalid(e.line(), col, what)
}

fn from_json(v: serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Value::Int(i),
            None => Value::Float(n.as_f64().unwrap_or(f64::NAN)),
        },
        serde_json::Value::String(s) => Value::Str(s.into()),
        serde_json::Value::Array(items) => Value::list(items.into_iter().map(from_json).collect()),
        serde_json::Value::Object(map) => {
            let mut out = DictMap::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.into(), from_json(v));
            }
            Value::dict(out)
        }
    }
}

/// `json_stringify(value, indent = null)` (5.10): compact, or pretty with `indent` spaces.
pub fn stringify(v: &Value, indent: Option<usize>) -> Exec<String> {
    let j = to_json(v, &mut Vec::new(), &mut Vec::new())?;
    Ok(match indent {
        None => j.to_string(),
        Some(n) => {
            let mut out = String::new();
            pretty(&j, n, 0, &mut out)?;
            out
        }
    })
}

/// A newline and `indent * level` spaces; the size is checked and reserved first, as for `*`
/// repetition (5.3).
fn pad(out: &mut String, indent: usize, level: usize) -> Exec<()> {
    let n = indent.checked_mul(level).and_then(|n| n.checked_add(1));
    match n {
        Some(n) if out.try_reserve(n).is_ok() => {
            out.push('\n');
            out.extend(std::iter::repeat_n(' ', n - 1));
            Ok(())
        }
        _ => value_error("json_stringify() result too large"),
    }
}

/// Like `JSON.stringify(v, null, n)` and Python's `json.dumps(v, indent=n)`: one item per line,
/// `": "` after keys, empty containers as `[]` / `{}`.
fn pretty(j: &serde_json::Value, indent: usize, level: usize, out: &mut String) -> Exec<()> {
    match j {
        serde_json::Value::Array(items) if !items.is_empty() => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(out, indent, level + 1)?;
                pretty(x, indent, level + 1, out)?;
            }
            pad(out, indent, level)?;
            out.push(']');
        }
        serde_json::Value::Object(map) if !map.is_empty() => {
            out.push('{');
            for (i, (k, x)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(out, indent, level + 1)?;
                out.push_str(&serde_json::Value::String(k.clone()).to_string());
                out.push_str(": ");
                pretty(x, indent, level + 1, out)?;
            }
            pad(out, indent, level)?;
            out.push('}');
        }
        other => out.push_str(&other.to_string()),
    }
    Ok(())
}

/// One step of the path to a value, for messages: `[2]`, `["name"]`.
enum Step {
    Index(usize),
    Key(Rc<str>),
}

/// A value JSON has no form for, with what to do instead as the error's hint (5.10).
#[cold]
#[inline(never)]
fn not_json<T>(v: &Value, path: &[Step]) -> Exec<T> {
    let what = match v {
        Value::Float(x) => format!("the float {}", float_str(*x)),
        other => other.type_name().to_string(),
    };
    let hint = match v {
        Value::Float(_) => "JSON has no NaN or infinity: store null instead (x != x is true for NaN), or write it with Python's json in a block, as in @py(v) { __import__(\"json\").dumps(v) }",
        Value::Bytes(_) => "JSON holds text, not bytes: store b.hex(), or b.decode() for UTF-8 text, or base64 from a block, as in @py(b) { __import__(\"base64\").b64encode(b).decode() }",
        Value::Func(_) => "a function isn't data: store what it returns, or leave it out",
        // An error value, the one other kind that comes here.
        _ => "an error isn't data: store its fields, as in {type: e.type, message: e.message}",
    };
    let err = not_json_hint::<T>(&what, path, "");
    if let Err(Flow::Error(e)) = &err {
        let _ = e.hint.set(hint.into());
    }
    err
}

/// A handle, which isn't JSON: `to_v2` first, or the block for a type it can't convert. Out of
/// line, so `to_json`'s frame stays small for deep values.
#[cold]
#[inline(never)]
fn handle_not_json<T>(h: &crate::engines::handles::HandleRef, path: &[Step]) -> Exec<T> {
    let how = h.plain_way().map_or_else(|| " (use to_v2(h) first)".to_string(), |w| format!(" ({w})"));
    not_json_hint("handle", path, &how)
}

fn not_json_hint<T>(what: &str, path: &[Step], hint: &str) -> Exec<T> {
    let mut at = String::new();
    for p in path {
        match p {
            Step::Index(i) => at.push_str(&format!("[{i}]")),
            Step::Key(k) => {
                at.push('[');
                quoted(k, &mut at);
                at.push(']');
            }
        }
    }
    let at = if at.is_empty() { at } else { format!(" at {at}") };
    type_error(format!("cannot convert {what} to JSON{at}{hint}"))
}

/// A container about to be encoded: a cycle or nesting past 1,000 levels is a ValueError.
fn enter(addr: usize, kind: &str, open: &mut Vec<usize>) -> Exec<()> {
    if open.contains(&addr) {
        return value_error(format!("cannot convert a cyclic {kind} to JSON"));
    }
    if open.len() >= MAX_JSON_DEPTH {
        return value_error(format!("cannot convert to JSON: nested deeper than {MAX_JSON_DEPTH} levels"));
    }
    open.push(addr);
    Ok(())
}

fn to_json(v: &Value, path: &mut Vec<Step>, open: &mut Vec<usize>) -> Exec<serde_json::Value> {
    Ok(match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(n) => serde_json::Value::Number((*n).into()),
        Value::Float(x) => match Number::from_f64(*x) {
            Some(n) => serde_json::Value::Number(n),
            None => return not_json(v, path),
        },
        Value::Str(s) => serde_json::Value::String(s.to_string()),
        Value::List(l) => {
            enter(l.addr(), "list", open)?;
            // Clone the items out: no borrow is held while encoding (5.11 borrow rule).
            let items = l.borrow().clone();
            let mut out = Vec::with_capacity(items.len());
            for (i, x) in items.iter().enumerate() {
                path.push(Step::Index(i));
                out.push(to_json(x, path, open)?);
                path.pop();
            }
            open.pop();
            serde_json::Value::Array(out)
        }
        Value::Dict(d) => {
            enter(d.addr(), "dict", open)?;
            let entries: Vec<(Rc<str>, Value)> = d.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let mut out = Map::with_capacity(entries.len());
            for (k, x) in entries {
                path.push(Step::Key(k.clone()));
                let j = to_json(&x, path, open)?;
                path.pop();
                out.insert(k.to_string(), j);
            }
            open.pop();
            serde_json::Value::Object(out)
        }
        Value::Bytes(_) | Value::Func(_) | Value::Error(_) => return not_json(v, path),
        Value::Handle(h) => return handle_not_json(h, path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::error::Flow;

    fn parsed(text: &str) -> String {
        match parse(text) {
            Ok(v) => format!("{v:?}"),
            Err(Flow::Error(e)) => e.message.to_string(),
            Err(_) => "flow".into(),
        }
    }

    fn json(v: &Value) -> String {
        stringify(v, None).unwrap_or_else(|f| match f {
            Flow::Error(e) => e.message.to_string(),
            _ => "flow".into(),
        })
    }

    #[test]
    fn numbers_keep_their_kind() {
        assert_eq!(parsed("1"), "1");
        assert_eq!(parsed("1.0"), "1.0");
        assert_eq!(parsed("1e2"), "100.0");
        assert_eq!(parsed("-0"), "-0.0");
        assert_eq!(parsed("9223372036854775807"), "9223372036854775807");
        assert_eq!(parsed("-9223372036854775808"), "-9223372036854775808");
        let big = "holds an integer beyond V2's 64-bit range (the JSON itself is fine): parse it in a @py block, as in @py(text) { import json; json.loads(text) }, or have the number written as a string";
        assert_eq!(parsed("9223372036854775808"), format!("JSON at line 1 column 1 {big}"));
        assert_eq!(parsed("[1,\n  -99999999999999999999]"), format!("JSON at line 2 column 3 {big}"));
        assert_eq!(json(&Value::Float(2.0)), "2.0");
        assert_eq!(json(&Value::Float(-0.0)), "-0.0");
        assert_eq!(json(&Value::Float(1e21)), "1e+21");
        assert_eq!(json(&Value::Int(i64::MIN)), "-9223372036854775808");
    }

    #[test]
    fn strings_objects_and_errors() {
        assert_eq!(json(&parse(r#""\r\n \u0000""#).ok().unwrap()), r#""\r\n \u0000""#);
        assert_eq!(parsed(r#""\uD800""#).split(':').next().unwrap(), "invalid JSON at line 1 column 8");
        let d = parse(r#"{"b": 1, "a": 2, "b": 3, "$v2": "plain"}"#).ok().unwrap();
        assert_eq!(json(&d), r#"{"b":3,"a":2,"$v2":"plain"}"#);
        assert_eq!(parsed("[1, 2"), "invalid JSON at line 1 column 5: EOF while parsing a list");
        assert_eq!(parsed("1 2"), "invalid JSON at line 1 column 3: trailing characters");
        assert_eq!(parsed(""), "invalid JSON: the text is empty");
        assert_eq!(parsed("\n\n   "), "invalid JSON: the text is empty");
        assert!(parsed(r#""\uD800""#).ends_with("without its other half, which isn't text"), "{}", parsed(r#""\uD800""#));
        // Columns count chars, also in serde_json's own errors.
        assert_eq!(parsed("{\"ключ\": tru}"), "invalid JSON at line 1 column 13: expected ident");
        assert_eq!(parsed("[\"é\""), "invalid JSON at line 1 column 4: EOF while parsing a list");
        // Only JSON whitespace may follow the value.
        for t in ["[1]\u{3000}", "[1]\u{b}", "\"s\"\u{a0}", "{}\u{85}"] {
            assert!(parsed(t).ends_with("trailing characters"), "{t:?}");
        }
        assert_eq!(parsed("[1]\n\t\r "), "<list of 1>");
        let huge = stringify(&Value::list(vec![Value::Int(1)]), Some(usize::MAX / 2));
        assert!(matches!(huge, Err(Flow::Error(e)) if &*e.message == "json_stringify() result too large"));
        // 1,000 levels are allowed; parsing them recurses, so use a stack like the interpreter's.
        let deep = std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(|| parse(&format!("{}{}", "[".repeat(1000), "]".repeat(1000))).is_ok())
            .unwrap()
            .join()
            .unwrap();
        assert!(deep);
        assert_eq!(parsed(&"[".repeat(1001)), "invalid JSON at line 1 column 1001: nested deeper than 1000 levels");
        assert_eq!(parsed(r#"["[[[[", "]]]"]"#), r#"<list of 2>"#);
    }

    #[test]
    fn stringify_rejects_what_json_cannot_hold() {
        let l = Value::list(vec![Value::Int(1), Value::dict(DictMap::from_iter([("k".into(), Value::Bytes(Rc::from(&b"x"[..])))]))]);
        assert_eq!(json(&l), r#"cannot convert bytes to JSON at [1]["k"]"#);
        assert_eq!(json(&Value::Float(f64::NAN)), "cannot convert the float nan to JSON");
        let cyc = Value::list(vec![]);
        if let Value::List(inner) = &cyc {
            inner.borrow_mut().push(cyc.clone());
        }
        assert_eq!(json(&cyc), "cannot convert a cyclic list to JSON");
        if let Value::List(inner) = &cyc {
            inner.borrow_mut().clear();
        }
        let pretty = stringify(&parse(r#"{"a": [1, {}], "b": []}"#).ok().unwrap(), Some(2)).ok().unwrap();
        assert_eq!(pretty, "{\n  \"a\": [\n    1,\n    {}\n  ],\n  \"b\": []\n}");
    }

    #[test]
    fn one_leading_byte_order_mark_is_skipped() {
        // RFC 8259 lets a parser ignore it, and read_file drops it (5.10); columns count after it.
        assert_eq!(parsed("\u{feff}[1]"), "<list of 1>");
        assert_eq!(parsed("\u{feff}[1, x]"), "invalid JSON at line 1 column 5: expected value");
        assert_eq!(parsed("\u{feff}\u{feff}[1]"), "invalid JSON at line 1 column 1: expected value");
        assert_eq!(parsed(" \u{feff}[1]"), "invalid JSON at line 1 column 2: expected value");
        assert_eq!(parsed("\u{feff}"), "invalid JSON: the text is empty");
    }

    #[test]
    fn what_json_cannot_hold_says_what_to_store() {
        // 5.10, round 17: each kind's hint, the message as it was.
        let hint = |v: Value| match stringify(&v, None) {
            Err(Flow::Error(e)) => (e.message.to_string(), e.hint.get().map(|h| h.to_string()).unwrap_or_default()),
            _ => panic!("no error"),
        };
        let float = "JSON has no NaN or infinity: store null instead (x != x is true for NaN), or write it with Python's json in a block, as in @py(v) { __import__(\"json\").dumps(v) }";
        for (x, shown) in [(f64::NAN, "nan"), (f64::INFINITY, "inf"), (f64::NEG_INFINITY, "-inf")] {
            assert_eq!(hint(Value::list(vec![Value::Float(x)])), (format!("cannot convert the float {shown} to JSON at [0]"), float.to_string()));
        }
        let (_, h) = hint(Value::Bytes(Rc::from(&b"x"[..])));
        assert!(h.starts_with("JSON holds text, not bytes: store b.hex(), or b.decode() for UTF-8 text"), "{h}");
        let (_, h) = hint(Value::Error(Rc::new(crate::runtime::error::ErrorValue::new("ValueError", "bad"))));
        assert_eq!(h, "an error isn't data: store its fields, as in {type: e.type, message: e.message}");
    }
}
