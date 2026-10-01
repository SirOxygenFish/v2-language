//! Values (INTERNALS 5.1). Roadmap 0.2 has the scalar kinds and builtin functions; lists, dicts,
//! closures, handles and errors arrive with 0.6-0.7.

use std::rc::Rc;

use super::builtins::BuiltinDef;

#[derive(Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Bytes(Rc<[u8]>),
    Builtin(&'static BuiltinDef),
}

impl Value {
    /// The name `type()` returns (DOCS 3.2).
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "str",
            Value::Bytes(_) => "bytes",
            Value::Builtin(_) => "func",
        }
    }
}

/// `str(v)`: a str is returned unchanged; everything else gets its canonical form (5.10).
pub fn to_str(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(x) => float_str(*x),
        Value::Str(s) => s.to_string(),
        Value::Bytes(b) => bytes_repr(b),
        Value::Builtin(def) => format!("<builtin {}>", def.name),
    }
}

/// Floats print like Rust's `{:?}` (`2.0`, `0.1`, `1e20`), with `nan`, `inf`, `-inf`.
pub fn float_str(x: f64) -> String {
    if x.is_nan() {
        "nan".into()
    } else if x.is_infinite() {
        if x > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        format!("{x:?}")
    }
}

/// `b"…"` with `\" \\ \n \r \t` and `\xNN` for other non-printable bytes.
pub fn bytes_repr(b: &[u8]) -> String {
    let mut s = String::from("b\"");
    for &c in b {
        match c {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            b'\t' => s.push_str("\\t"),
            0x20..=0x7e => s.push(c as char),
            _ => s.push_str(&format!("\\x{c:02X}")),
        }
    }
    s.push('"');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_forms() {
        assert_eq!(to_str(&Value::Float(2.0)), "2.0");
        assert_eq!(to_str(&Value::Float(0.1)), "0.1");
        assert_eq!(to_str(&Value::Float(1e20)), "1e20");
        assert_eq!(to_str(&Value::Float(f64::NAN)), "nan");
        assert_eq!(to_str(&Value::Float(f64::NEG_INFINITY)), "-inf");
        assert_eq!(to_str(&Value::Bytes(b"\x00A\"".to_vec().into())), "b\"\\x00A\\\"\"");
        assert_eq!(to_str(&Value::Null), "null");
    }
}
