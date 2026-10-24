//! Methods of str, bytes, list and dict (INTERNALS 5.11).

use std::fmt::Write as _;
use std::rc::Rc;

use super::builtins::str_arg;
use super::call::ParamSpec;
use super::error::{fail, type_error, value_error, Exec};
use super::interp::Interp;
use super::ops::{eq, sort_key_cmp};
use super::value::{Dict, List, Value};
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
    m("split", &[ParamSpec::req("sep")], str_split),
    m("lines", NONE, |_, r, _, _| Ok(Value::list(the_str(r).lines().map(Value::str).collect()))),
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
            Err(e) => value_error(format!("invalid UTF-8 at byte offset {}", e.valid_up_to())),
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

pub static LIST_METHODS: &[MethodDef] = &[
    m("push", &[ParamSpec::req("x")], |_, r, mut a, _| {
        the_list(r).borrow_mut().push(a.remove(0));
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
    m("sort", NONE, list_sort),
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
    m("join", &[ParamSpec::req("sep")], |it, r, a, _| {
        let sep = str_arg(&a[0], "list.join", "sep")?;
        let items = the_list(r).borrow().clone();
        let mut out = String::new();
        for (i, x) in items.iter().enumerate() {
            if i > 0 {
                out.push_str(&sep);
            }
            out.push_str(&it.str_of(x)?);
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
    m("keys", NONE, |_, r, _, _| Ok(Value::list(the_dict(r).borrow().keys().map(|k| Value::Str(k.clone())).collect()))),
    m("values", NONE, |_, r, _, _| Ok(Value::list(the_dict(r).borrow().values().cloned().collect()))),
    m("get", &[ParamSpec::req("k"), ParamSpec::opt("default", || Value::Null)], |_, r, mut a, _| {
        let k = str_arg(&a[0], "dict.get", "k")?;
        let found = the_dict(r).borrow().get(&k).cloned();
        Ok(found.unwrap_or_else(|| a.remove(1)))
    }),
    m("remove", &[ParamSpec::req("k")], |_, r, a, _| {
        let k = str_arg(&a[0], "dict.remove", "k")?;
        let removed = the_dict(r).borrow_mut().shift_remove(&k);
        match removed {
            Some(v) => Ok(v),
            None => key_error(&k, the_dict(r)),
        }
    }),
    m("copy", NONE, |_, r, _, _| Ok(Value::dict(the_dict(r).borrow().clone()))),
];

/// The method table of a receiver type.
pub fn table(v: &Value) -> Option<&'static [MethodDef]> {
    match v {
        Value::Str(_) => Some(STR_METHODS),
        Value::Bytes(_) => Some(BYTES_METHODS),
        Value::List(_) => Some(LIST_METHODS),
        Value::Dict(_) => Some(DICT_METHODS),
        _ => None,
    }
}

fn the_str(v: &Value) -> &Rc<str> {
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

fn str_split(_: &mut Interp, r: &Value, a: Vec<Value>, _: Span) -> Exec<Value> {
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

/// Stable, in place; all numbers (exact order, NaN last) or all strings (5.11).
fn list_sort(_: &mut Interp, r: &Value, _: Vec<Value>, _: Span) -> Exec<Value> {
    let l = the_list(r);
    let kind = |v: &Value| match v {
        Value::Int(_) | Value::Float(_) => Some(0),
        Value::Str(_) => Some(1),
        _ => None,
    };
    {
        let items = l.borrow();
        if let Some(bad) = items.iter().find(|v| kind(v).is_none()) {
            return type_error(format!("cannot sort a list containing {} (only numbers or strings)", bad.type_name()));
        }
        if let (Some(first), Some(other)) = (items.first(), items.iter().find(|v| kind(v) != kind(&items[0]))) {
            return type_error(format!("cannot sort a list mixing {} and {}", first.type_name(), other.type_name()));
        }
    }
    l.borrow_mut().sort_by(sort_key_cmp);
    Ok(Value::Null)
}
