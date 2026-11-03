//! Values (INTERNALS 5.1) and their canonical text (5.10 `str`).

use std::cell::{Cell, OnceCell, RefCell};
use std::fmt;
use std::ops::Deref;
use std::rc::Rc;

use indexmap::IndexMap;

use super::builtins::BuiltinDef;
use super::env::Env;
use super::error::ErrorValue;
use crate::engines::handles::HandleRef;
use crate::syntax::ast::{EngineImport, FuncDecl, UnitId};

/// A V2 value. Cloning is O(1): lists and dicts are shared, everything else is immutable (5.1).
#[derive(Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Bytes(Rc<[u8]>),
    List(List),
    Dict(Dict),
    Func(Func),
    /// An object that lives in a worker (ch. 11).
    Handle(Rc<HandleRef>),
    Error(Rc<ErrorValue>),
}

/// Something callable (5.1). Native functions arrive with Phase 2.
pub enum Callable {
    /// A `func` (named or anonymous) with the frame it closes over. `name` is the function's
    /// name, or `func@main.v2:12` (5.7).
    Closure { decl: Rc<FuncDecl>, env: Env, name: Rc<str> },
    Builtin(&'static BuiltinDef),
    /// A bridged export: `name` in the worker's unit `unit` (the wire unit id) (13.4).
    Proxy { session: usize, unit: Rc<str>, name: Rc<str> },
    /// An inner `@import`'s route stub passed back before its route resolved (11.8, 13.6).
    Route { route: usize, name: Rc<str> },
    /// A name bound by an `@import` that waited: `local` in `import`, the blocks reached by that
    /// statement, and what it turned out to be once its language started (12.7).
    Waiting { import: Rc<EngineImport>, local: Rc<str>, seen: Rc<[UnitId]>, resolved: OnceCell<Value> },
}

impl Callable {
    pub fn name(&self) -> &str {
        match self {
            Callable::Closure { name, .. } | Callable::Proxy { name, .. } | Callable::Route { name, .. } => name,
            Callable::Waiting { local, .. } => local,
            Callable::Builtin(def) => def.name,
        }
    }
}

// -- Shared containers ----------------------------------------------------------------------------
//
// Dropping the last reference to a list, dict or closure frees what it holds iteratively: a value
// nested a million levels deep (`for i in 0..1000000 { v = [v] }`) must not overflow the stack
// when it goes away.

macro_rules! shared {
    ($(#[$doc:meta])* $name:ident, $target:ty) => {
        $(#[$doc])*
        #[derive(Clone)]
        pub struct $name(Rc<$target>);

        #[allow(dead_code)] // the same API for each container; not every one uses all of it
        impl $name {
            pub fn new(v: $target) -> $name {
                $name(Rc::new(v))
            }

            pub fn ptr_eq(a: &$name, b: &$name) -> bool {
                Rc::ptr_eq(&a.0, &b.0)
            }

            /// Identity, for cycle detection.
            pub fn addr(&self) -> usize {
                Rc::as_ptr(&self.0) as *const u8 as usize
            }

            pub fn strong_count(&self) -> usize {
                Rc::strong_count(&self.0)
            }
        }

        impl Deref for $name {
            type Target = $target;
            fn deref(&self) -> &$target {
                &self.0
            }
        }
    };
}

shared!(
    /// A list: ordered, mutable, shared.
    List,
    RefCell<Vec<Value>>
);
shared!(
    /// A dict: string keys in insertion order, mutable, shared.
    Dict,
    RefCell<IndexMap<Rc<str>, Value>>
);
shared!(
    /// A function value.
    Func,
    Callable
);

impl Drop for List {
    fn drop(&mut self) {
        if Rc::strong_count(&self.0) == 1 {
            if let Ok(mut items) = self.0.try_borrow_mut() {
                let items = std::mem::take(&mut *items);
                release(items);
            }
        }
    }
}

impl Drop for Dict {
    fn drop(&mut self) {
        if Rc::strong_count(&self.0) == 1 {
            if let Ok(mut map) = self.0.try_borrow_mut() {
                let map = std::mem::take(&mut *map);
                release(map.into_values().collect());
            }
        }
    }
}

impl Drop for Func {
    fn drop(&mut self) {
        if let Callable::Closure { env, .. } = &*self.0 {
            // The last reference to a closure whose frame nothing else holds: free the frame's
            // values here, not through the frame's own drop.
            if Rc::strong_count(&self.0) == 1 && Rc::strong_count(env) == 1 {
                release(super::env::take_values(env));
            } else if Rc::strong_count(&self.0) <= 2 {
                // One holder will be left, which can be the frame's own: not a call's copy going.
                super::env::closure_dropped(env);
            }
        }
    }
}

thread_local! {
    static PENDING: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    static DRAINING: Cell<bool> = const { Cell::new(false) };
}

/// Drop `items`. Containers among them are queued and dropped one at a time by the outermost
/// call, so nested drops never recurse more than one level.
pub fn release(items: Vec<Value>) {
    if !items.iter().any(|v| matches!(v, Value::List(_) | Value::Dict(_) | Value::Func(_) | Value::Error(_))) {
        return;
    }
    PENDING.with(|p| p.borrow_mut().extend(items));
    if DRAINING.with(|d| d.replace(true)) {
        return;
    }
    while let Some(v) = PENDING.with(|p| p.borrow_mut().pop()) {
        drop(v);
    }
    DRAINING.with(|d| d.set(false));
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(s.into())
    }

    pub fn list(items: Vec<Value>) -> Value {
        super::env::made(items.len() + 1);
        Value::List(List::new(RefCell::new(items)))
    }

    pub fn dict(map: IndexMap<Rc<str>, Value>) -> Value {
        super::env::made(map.len() + 1);
        Value::Dict(Dict::new(RefCell::new(map)))
    }

    pub fn func(c: Callable) -> Value {
        Value::Func(Func::new(c))
    }

    /// The name `type()` returns (5.1).
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "str",
            Value::Bytes(_) => "bytes",
            Value::List(_) => "list",
            Value::Dict(_) => "dict",
            Value::Func(_) => "func",
            Value::Handle(_) => "handle",
            Value::Error(_) => "error",
        }
    }

    /// Falsy: `null`, `false`, `0`, `0.0`, `-0.0` and empty str, bytes, list, dict (5.2).
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(x) => *x != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::Bytes(b) => !b.is_empty(),
            Value::List(l) => !l.borrow().is_empty(),
            Value::Dict(d) => !d.borrow().is_empty(),
            Value::Func(_) | Value::Handle(_) | Value::Error(_) => true,
        }
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
            _ => s.push_str(&format!("\\x{c:02x}")),
        }
    }
    s.push('"');
    s
}

/// A string inside a container: double-quoted, with `\" \\ \n \r \t` and `\u{..}` for other
/// control chars (5.10).
pub fn quoted(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Scalars as `str()` would print them inside a container; containers only by size (tests and
/// `ErrorValue`'s `Debug`).
impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => write!(f, "null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(n) => write!(f, "{n}"),
            Value::Float(x) => write!(f, "{}", float_str(*x)),
            Value::Str(s) => {
                let mut out = String::new();
                quoted(s, &mut out);
                write!(f, "{out}")
            }
            Value::Bytes(b) => write!(f, "{}", bytes_repr(b)),
            Value::List(l) => write!(f, "<list of {}>", l.try_borrow().map_or(0, |l| l.len())),
            Value::Dict(d) => write!(f, "<dict of {}>", d.try_borrow().map_or(0, |d| d.len())),
            Value::Func(c) => write!(f, "<func {}>", c.name()),
            Value::Handle(h) => write!(f, "<{} {}>", h.tag, h.type_name),
            Value::Error(e) => write!(f, "<error {}: {}>", e.kind, e.message),
        }
    }
}
