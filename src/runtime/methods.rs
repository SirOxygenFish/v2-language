//! Methods of str, bytes, list and dict (INTERNALS 5.11).

use std::fmt::Write as _;
use std::rc::Rc;

use super::builtins::{bool_arg, str_arg};
use super::call::ParamSpec;
use super::error::{fail, type_error, value_error, Exec, Flow};
use super::interp::Interp;
use super::ops::{eq, sort_key_cmp};
use super::value::{Dict, List, Str, Value};
use crate::syntax::span::Span;

pub type MethodFn = fn(&mut Interp, &Value, Vec<Value>, Span) -> Exec<Value>;

pub struct MethodDef {
    pub name: &'static str,
    pub params: &'static [ParamSpec],
    pub f: MethodFn,
}

const fn m(name: &'static str, params: &'static [ParamSpec], f: MethodFn) -> MethodDef {
    MethodDef { name, params, f }
}

const NONE: &[ParamSpec] = &[];

pub static STR_METHODS: &[MethodDef] = &[
    m("upper", NONE, |_, r, _, _| Ok(Value::Str(the_str(r).to_uppercase().into()))),
    m("lower", NONE, |_, r, _, _| Ok(Value::Str(the_str(r).to_lowercase().into()))),
    m("trim", NONE, |_, r, _, _| Ok(Value::str(the_str(r).trim()))),
    m("split", &[ParamSpec::opt("sep", || Value::Null)], str_split),
    m("lines", NONE, |_, r, _, _| Ok(Value::list(text_lines(the_str(r)).into_iter().map(Value::str).collect()))),
    m("replace", &[ParamSpec::req("old"), ParamSpec::req("new")], str_replace),
    m("starts_with", &[ParamSpec::req("p")], |_, r, a, _| {
        Ok(Value::Bool(the_str(r).starts_with(&*str_arg(&a[0], "str.starts_with", "p")?)))
    }),
    m("ends_with", &[ParamSpec::req("p")], |_, r, a, _| {
        Ok(Value::Bool(the_str(r).ends_with(&*str_arg(&a[0], "str.ends_with", "p")?)))
    }),
    m("index_of", &[ParamSpec::req("sub")], str_index_of),
    m("encode", NONE, |_, r, _, _| Ok(Value::Bytes(the_str(r).as_bytes().into()))),
];

pub static BYTES_METHODS: &[MethodDef] = &[
    m("decode", NONE, |_, r, _, _| {
        let b = the_bytes(r);
        match std::str::from_utf8(b) {
            Ok(s) => Ok(Value::str(s)),
            Err(e) => value_error(format!("invalid UTF-8 at byte offset {}", e.valid_up_to())).inspect_err(|f| utf8_only("bytes.decode", f, false)),
        }
    }),
    m("hex", NONE, |_, r, _, _| {
        let mut s = String::with_capacity(the_bytes(r).len() * 2);
        for b in the_bytes(r).iter() {
            let _ = write!(s, "{b:02x}");
        }
        Ok(Value::Str(s.into()))
    }),
];

/// `b.decode("latin-1")`, `s.encode(encoding: "cp1254")`, and bytes that aren't UTF-8: V2's own
/// read and write UTF-8 only, a block any encoding (5.4). The hint for `method`'s ArityError or
/// ValueError; `utf8` when the call named UTF-8 (`names_utf8`), which needs no argument.
pub(crate) fn utf8_only(method: &str, f: &Flow, utf8: bool) {
    let hint = match (method, utf8) {
        ("bytes.decode", true) => "V2's decode() reads UTF-8: write b.decode()",
        ("str.encode", true) => "V2's encode() writes UTF-8: write s.encode()",
        ("bytes.decode", _) => "V2's decode() reads UTF-8 only: for another encoding, decode in a block, as in @py(b) { b.decode(\"cp1254\") }",
        ("str.encode", _) => "V2's encode() writes UTF-8 only: for another encoding, encode in a block, as in @py(s) { s.encode(\"cp1254\") }",
        _ => return,
    };
    if let Flow::Error(e) = f {
        if matches!(&*e.kind, "ArityError" | "ValueError") {
            let _ = e.hint.set(hint.into());
        }
    }
}

/// Whether a call's one argument, by position or as `encoding:`, is `"utf-8"` or `"utf8"` in any
/// case, as in `b.decode("utf-8")` (`utf8_only`).
pub(crate) fn names_utf8(pos: &[Value], named: &crate::runtime::call::Named) -> bool {
    let only = match (pos, named.as_slice()) {
        ([v], []) => v,
        ([], [(k, v)]) if &**k == "encoding" => v,
        _ => return false,
    };
    matches!(only, Value::Str(s) if s.eq_ignore_ascii_case("utf-8") || s.eq_ignore_ascii_case("utf8"))
}

pub static LIST_METHODS: &[MethodDef] = &[
    m("push", &[ParamSpec::rest("items")], |_, r, a, _| {
        // The binder made the rest list, so `xs.push(...xs)` borrows two different lists.
        let Value::List(items) = &a[0] else { unreachable!("the rest parameter is a list") };
        let items = items.borrow().clone();
        the_list(r).borrow_mut().extend(items);
        Ok(Value::Null)
    }),
    m("pop", &[ParamSpec::opt("i", || Value::Int(-1))], |_, r, a, _| {
        let i = int_arg(&a[0], "list.pop", "i")?;
        let mut l = the_list(r).borrow_mut();
        if l.is_empty() {
            return fail("IndexError", "pop from empty list");
        }
        match normalize(i, l.len()) {
            Some(k) => Ok(l.remove(k)),
            None => index_error("list", i, l.len()),
        }
    }),
    m("get", &[ParamSpec::req("i"), ParamSpec::opt("default", || Value::Null)], |_, r, mut a, _| {
        let i = int_arg(&a[0], "list.get", "i")?;
        let l = the_list(r).borrow();
        Ok(match normalize(i, l.len()) {
            Some(k) => l[k].clone(),
            None => a.remove(1),
        })
    }),
    m("insert", &[ParamSpec::req("i"), ParamSpec::req("x")], |_, r, mut a, _| {
        let i = int_arg(&a[0], "list.insert", "i")?;
        let mut l = the_list(r).borrow_mut();
        let len = l.len() as i64;
        let k = if i < 0 { i.saturating_add(len) } else { i }.clamp(0, len);
        l.insert(k as usize, a.remove(1));
        Ok(Value::Null)
    }),
    m("sort", &[ParamSpec::opt("key", || Value::Null), ParamSpec::opt("reverse", || Value::Bool(false))], list_sort),
    m("reverse", NONE, |_, r, _, _| {
        the_list(r).borrow_mut().reverse();
        Ok(Value::Null)
    }),
    m("map", &[ParamSpec::req("f")], |it, r, a, at| {
        let items = the_list(r).borrow().clone();
        let mut out = Vec::with_capacity(items.len());
        for x in items {
            out.push(it.call_value(&a[0], vec![x], Vec::new(), at)?);
        }
        Ok(Value::list(out))
    }),
    m("filter", &[ParamSpec::req("f")], |it, r, a, at| {
        let items = the_list(r).borrow().clone();
        let mut out = Vec::new();
        for x in items {
            if it.call_value(&a[0], vec![x.clone()], Vec::new(), at)?.truthy() {
                out.push(x);
            }
        }
        Ok(Value::list(out))
    }),
    m("join", &[ParamSpec::req("sep")], |it, r, a, at| {
        let sep = str_arg(&a[0], "list.join", "sep")?;
        let items = the_list(r).borrow().clone();
        let mut out = String::new();
        for (i, x) in items.iter().enumerate() {
            if i > 0 {
                out.push_str(&sep);
            }
            out.push_str(&it.str_of(x, at)?);
        }
        Ok(Value::Str(out.into()))
    }),
    m("index_of", &[ParamSpec::req("x")], |_, r, a, _| {
        let items = the_list(r).borrow().clone();
        for (i, x) in items.iter().enumerate() {
            if eq(x, &a[0])? {
                return Ok(Value::Int(i as i64));
            }
        }
        Ok(Value::Int(-1))
    }),
    m("copy", NONE, |_, r, _, _| Ok(Value::list(the_list(r).borrow().clone()))),
];

pub static DICT_METHODS: &[MethodDef] = &[
    m("keys", NONE, |_, r, _, _| Ok(Value::list(the_dict(r).borrow().keys().map(|k| Value::Str(k.clone().into())).collect()))),
    m("values", NONE, |_, r, _, _| Ok(Value::list(the_dict(r).borrow().values().cloned().collect()))),
    m("get", &[ParamSpec::req("k"), ParamSpec::opt("default", || Value::Null)], |_, r, mut a, _| {
        let k = str_arg(&a[0], "dict.get", "k")?;
        let found = the_dict(r).borrow().get(&k).cloned();
        Ok(found.unwrap_or_else(|| a.remove(1)))
    }),
    m("remove", &[ParamSpec::req("k")], |_, r, a, _| {
        let k = str_arg(&a[0], "dict.remove", "k")?;
        let removed = the_dict(r).borrow_mut().remove(&k);
        match removed {
            Some(v) => Ok(v),
            None => key_error(&k, the_dict(r)),
        }
    }),
    m("copy", NONE, |_, r, _, _| Ok(Value::dict(the_dict(r).borrow().clone()))),
];

/// The method table of a receiver type.
/// Which methods table `table` gives `v`: 1 to 4, 0 for none (a method name's cache, 5.5).
pub fn table_kind(v: &Value) -> u32 {
    match v {
        Value::Str(_) => 1,
        Value::Bytes(_) => 2,
        Value::List(_) => 3,
        Value::Dict(_) => 4,
        _ => 0,
    }
}

pub fn table(v: &Value) -> Option<&'static [MethodDef]> {
    match v {
        Value::Str(_) => Some(STR_METHODS),
        Value::Bytes(_) => Some(BYTES_METHODS),
        Value::List(_) => Some(LIST_METHODS),
        Value::Dict(_) => Some(DICT_METHODS),
        _ => None,
    }
}

fn the_str(v: &Value) -> &Str {
    match v {
        Value::Str(s) => s,
        _ => unreachable!("str method on a str"),
    }
}

fn the_bytes(v: &Value) -> &Rc<[u8]> {
    match v {
        Value::Bytes(b) => b,
        _ => unreachable!("bytes method on bytes"),
    }
}

fn the_list(v: &Value) -> &List {
    match v {
        Value::List(l) => l,
        _ => unreachable!("list method on a list"),
    }
}

fn the_dict(v: &Value) -> &Dict {
    match v {
        Value::Dict(d) => d,
        _ => unreachable!("dict method on a dict"),
    }
}

pub fn int_arg(v: &Value, func: &str, param: &str) -> Exec<i64> {
    match v {
        Value::Int(n) => Ok(*n),
        other => type_error(format!("{func}() argument '{param}' must be an int, got {}", other.type_name())),
    }
}

/// A possibly negative index into a sequence of `len` items, if in range.
pub fn normalize(i: i64, len: usize) -> Option<usize> {
    let k = if i < 0 { i.checked_add(len as i64)? } else { i };
    (0..len as i64).contains(&k).then_some(k as usize)
}

/// `IndexError: list index 9 out of range (length 5)` (5.4).
pub fn index_error<T>(what: &str, i: i64, len: usize) -> Exec<T> {
    fail("IndexError", format!("{what} index {i} out of range (length {len})"))
}

/// `KeyError: key 'nmae' not found — did you mean 'name'?` (5.5, 5.9).
pub fn key_error<T>(key: &str, d: &Dict) -> Exec<T> {
    let keys: Vec<Rc<str>> = d.borrow().keys().cloned().collect();
    let msg = super::error::with_suggestion(format!("key '{key}' not found"), key, keys.iter().map(|k| &**k));
    fail("KeyError", msg)
}

/// `util._helper`, a member read: `key_error`, and for a name that starts with `_`, that an
/// imported .v2 file keeps such names to itself (6.4).
pub fn member_key_error<T>(key: &str, d: &Dict) -> Exec<T> {
    let Err(Flow::Error(e)) = key_error::<T>(key, d) else { unreachable!("key_error fails") };
    // `d.keys` without the call reads a key named keys.
    if DICT_METHODS.iter().any(|m| m.name == key) {
        let err = super::error::ErrorValue::new("KeyError", &*e.message);
        let _ = err.hint.set(format!("{key} is a method of dicts, so call it, as in d.{key}()").into());
        return Err(Flow::Error(Rc::new(err)));
    }
    if !key.starts_with('_') {
        return Err(Flow::Error(e));
    }
    fail("KeyError", format!("{} (an imported .v2 file keeps names that start with _ to itself)", e.message))
}

/// `lines()`: the text split at `\n`, `\r\n` and a lone `\r` (an old Mac file), without the
/// breaks; a break at the very end makes no empty last line (5.11).
pub fn text_lines(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let (mut out, mut start, mut i) = (Vec::new(), 0, 0);
    while i < b.len() {
        match b[i] {
            b'\n' | b'\r' => {
                out.push(&s[start..i]);
                i += if b[i] == b'\r' && b.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `split(sep = null)`: without `sep`, on runs of whitespace with no empty parts (5.11).
fn str_split(_: &mut Interp, r: &Value, a: Vec<Value>, _: Span) -> Exec<Value> {
    if let Value::Null = a[0] {
        return Ok(Value::list(the_str(r).split_whitespace().map(Value::str).collect()));
    }
    let sep = str_arg(&a[0], "str.split", "sep")?;
    if sep.is_empty() {
        return value_error("str.split() separator must not be empty");
    }
    Ok(Value::list(the_str(r).split(&*sep).map(Value::str).collect()))
}

fn str_replace(_: &mut Interp, r: &Value, a: Vec<Value>, _: Span) -> Exec<Value> {
    let old = str_arg(&a[0], "str.replace", "old")?;
    let new = str_arg(&a[1], "str.replace", "new")?;
    if old.is_empty() {
        return value_error("str.replace() argument 'old' must not be empty");
    }
    Ok(Value::Str(the_str(r).replace(&*old, &new).into()))
}

fn str_index_of(it: &mut Interp, r: &Value, a: Vec<Value>, _: Span) -> Exec<Value> {
    let sub = str_arg(&a[0], "str.index_of", "sub")?;
    let s = the_str(r);
    Ok(Value::Int(match s.find(&*sub) {
        Some(b) => it.chars.char_of(s, b) as i64,
        None => -1,
    }))
}

/// `sort(key = null, reverse = false)`: stable, in place. `key` is called once per item, in
/// order, on a snapshot; the keys (else the items) must be all numbers (exact order, NaN last)
/// or all strings. `reverse` reverses the comparator, so equal items keep their order (5.11).
fn list_sort(it: &mut Interp, r: &Value, a: Vec<Value>, at: Span) -> Exec<Value> {
    let l = the_list(r);
    let reverse = bool_arg(&a[1], "list.sort", "reverse")?;
    // No borrow is held across the key function, which may change this list (5.11).
    let items = l.borrow().clone();
    let keys = match &a[0] {
        Value::Null => items.clone(),
        f => {
            let mut keys = Vec::with_capacity(items.len());
            for x in &items {
                keys.push(it.call_value(f, vec![x.clone()], Vec::new(), at)?);
            }
            // Sorting would drop what the key function added or removed, as Python says (5.11).
            if l.borrow().len() != items.len() {
                return value_error("list modified during sort()");
            }
            keys
        }
    };
    let kind = |v: &Value| match v {
        Value::Int(_) | Value::Float(_) => Some(0),
        Value::Str(_) => Some(1),
        _ => None,
    };
    if let Some(bad) = keys.iter().find(|v| kind(v).is_none()) {
        let hint = super::ops::handle_hint(bad, "sorted(h)", "[...h].sort()");
        return type_error(format!("cannot sort a list containing {} (only numbers or strings){hint}", bad.type_name()));
    }
    if let (Some(first), Some(other)) = (keys.first(), keys.iter().find(|v| kind(v) != kind(&keys[0]))) {
        return type_error(format!("cannot sort a list mixing {} and {}", first.type_name(), other.type_name()));
    }
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&i, &j| {
        let o = sort_key_cmp(&keys[i], &keys[j]);
        if reverse {
            o.reverse()
        } else {
            o
        }
    });
    let sorted = order.into_iter().map(|i| items[i].clone()).collect();
    *l.borrow_mut() = sorted;
    Ok(Value::Null)
}
