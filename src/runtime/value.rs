//! Values (INTERNALS 5.1) and their canonical text (5.10 `str`).

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::rc::{Rc, Weak};

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
    Str(Str),
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

// -- Strings and dict entries ---------------------------------------------------------------------

/// A str (5.1): immutable to V2 code, so a clone shares it. Most are an `Rc<str>`, one allocation;
/// one that `s += piece` grew keeps a `String`'s spare room instead, so the next append goes in
/// place when nothing else holds it (5.3).
#[derive(Clone)]
pub struct Str(Repr);

#[derive(Clone)]
enum Repr {
    Fixed(Rc<str>),
    Growing(Rc<String>),
}

/// What the char cache (5.4) holds of a string: it keeps the allocation, so the address can't be
/// reused while the entry lives, and it stops `append` from changing the text under the entry.
pub enum WeakStr {
    Fixed(Weak<str>),
    Growing(Weak<String>),
}

impl WeakStr {
    pub fn addr(&self) -> usize {
        match self {
            WeakStr::Fixed(w) => Weak::as_ptr(w) as *const u8 as usize,
            WeakStr::Growing(w) => Weak::as_ptr(w) as usize,
        }
    }

    pub fn alive(&self) -> bool {
        match self {
            WeakStr::Fixed(w) => w.strong_count() > 0,
            WeakStr::Growing(w) => w.strong_count() > 0,
        }
    }
}

impl Str {
    /// Identity: the char cache (5.4) and the collector's weights (5.1) key on it.
    pub fn addr(&self) -> usize {
        match &self.0 {
            Repr::Fixed(s) => Rc::as_ptr(s) as *const u8 as usize,
            Repr::Growing(s) => Rc::as_ptr(s) as usize,
        }
    }

    pub fn ptr_eq(a: &Str, b: &Str) -> bool {
        a.addr() == b.addr()
    }

    pub fn downgrade(&self) -> WeakStr {
        match &self.0 {
            Repr::Fixed(s) => WeakStr::Fixed(Rc::downgrade(s)),
            Repr::Growing(s) => WeakStr::Growing(Rc::downgrade(s)),
        }
    }

    /// `self + piece` into `self`: in place when this string grew before and nothing else holds
    /// it, not even the char cache; else into a new one, so other holders keep the old text.
    pub fn append(&mut self, piece: &str) {
        if let Repr::Growing(s) = &mut self.0 {
            if let Some(s) = Rc::get_mut(s) {
                s.push_str(piece);
                return;
            }
        }
        let mut s = String::with_capacity(self.len() + piece.len());
        s.push_str(self);
        s.push_str(piece);
        self.0 = Repr::Growing(Rc::new(s));
    }
}

impl Deref for Str {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        match &self.0 {
            Repr::Fixed(s) => s,
            Repr::Growing(s) => s,
        }
    }
}

impl PartialEq for Str {
    fn eq(&self, other: &Str) -> bool {
        **self == **other
    }
}

impl Eq for Str {}

impl PartialOrd for Str {
    fn partial_cmp(&self, other: &Str) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Str {
    fn cmp(&self, other: &Str) -> std::cmp::Ordering {
        (**self).cmp(&**other)
    }
}

impl From<Rc<str>> for Str {
    fn from(s: Rc<str>) -> Str {
        Str(Repr::Fixed(s))
    }
}

impl From<&str> for Str {
    fn from(s: &str) -> Str {
        Str(Repr::Fixed(s.into()))
    }
}

impl From<String> for Str {
    fn from(s: String) -> Str {
        Str(Repr::Fixed(s.into()))
    }
}

/// A dict key or an error's text: shared when it is an `Rc<str>` already.
impl From<&Str> for Rc<str> {
    fn from(s: &Str) -> Rc<str> {
        match &s.0 {
            Repr::Fixed(s) => s.clone(),
            Repr::Growing(s) => Rc::from(s.as_str()),
        }
    }
}

impl fmt::Display for Str {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl fmt::Debug for Str {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

/// A dict's entries: string keys in insertion order (5.1). `remove` leaves a hole instead of
/// moving every later entry, and the holes go once they outnumber the entries, so it is O(1)
/// amortized. A new key, or one removed and put back, goes at the end.
#[derive(Clone, Default)]
pub struct DictMap {
    /// The entries in order; `None` is a removed one.
    slots: Vec<Option<(Rc<str>, Value)>>,
    /// Where each key is in `slots` once there are more than `SMALL` of them; a small dict's keys
    /// are found by walking `slots`.
    index: HashMap<Rc<str>, usize>,
    len: usize,
}

/// Up to this many slots a dict has no `index`.
const SMALL: usize = 8;

impl DictMap {
    pub fn new() -> DictMap {
        DictMap::default()
    }

    pub fn with_capacity(n: usize) -> DictMap {
        DictMap { slots: Vec::with_capacity(n), ..DictMap::default() }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn find(&self, k: &str) -> Option<usize> {
        if self.slots.len() <= SMALL {
            self.slots.iter().position(|e| matches!(e, Some((key, _)) if **key == *k))
        } else {
            self.index.get(k).copied()
        }
    }

    pub fn contains_key(&self, k: &str) -> bool {
        self.find(k).is_some()
    }

    pub fn get(&self, k: &str) -> Option<&Value> {
        let i = self.find(k)?;
        self.slots[i].as_ref().map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, k: &str) -> Option<&mut Value> {
        let i = self.find(k)?;
        self.slots[i].as_mut().map(|(_, v)| v)
    }

    /// A key already there keeps its place and gets `v`; its old value comes back.
    pub fn insert(&mut self, k: Rc<str>, v: Value) -> Option<Value> {
        let end = self.slots.len();
        let i = if end > SMALL { *self.index.entry(k.clone()).or_insert(end) } else { self.find(&k).unwrap_or(end) };
        if let Some(Some((_, old))) = self.slots.get_mut(i) {
            return Some(std::mem::replace(old, v));
        }
        self.slots.push(Some((k, v)));
        self.len += 1;
        if self.slots.len() == SMALL + 1 {
            self.reindex();
        }
        None
    }

    pub fn remove(&mut self, k: &str) -> Option<Value> {
        let before = self.slots.len();
        let i = if before > SMALL { self.index.remove(k)? } else { self.find(k)? };
        let (_, v) = self.slots[i].take().expect("a key found has its entry");
        self.len -= 1;
        // Holes at the end go at once, the others when there are more of them than entries.
        while let Some(None) = self.slots.last() {
            self.slots.pop();
        }
        if self.slots.len() > 2 * self.len + SMALL {
            self.slots.retain(Option::is_some);
            self.reindex();
        } else if before > SMALL && self.slots.len() <= SMALL {
            self.reindex();
        }
        Some(v)
    }

    /// `index` from `slots`: every key when there are more than `SMALL` slots, else none.
    fn reindex(&mut self) {
        if self.slots.len() <= SMALL {
            self.index = HashMap::new();
            return;
        }
        self.index.clear();
        for (i, entry) in self.slots.iter().enumerate() {
            if let Some((k, _)) = entry {
                self.index.insert(k.clone(), i);
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Rc<str>, &Value)> {
        self.slots.iter().flatten().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &Rc<str>> {
        self.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.iter().map(|(_, v)| v)
    }

    pub fn into_values(self) -> impl Iterator<Item = Value> {
        self.slots.into_iter().flatten().map(|(_, v)| v)
    }
}

/// Later duplicates replace the value and keep the first one's place, as `insert` does.
impl FromIterator<(Rc<str>, Value)> for DictMap {
    fn from_iter<I: IntoIterator<Item = (Rc<str>, Value)>>(items: I) -> DictMap {
        let mut map = DictMap::new();
        for (k, v) in items {
            map.insert(k, v);
        }
        map
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
    RefCell<DictMap>
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

    pub fn dict(map: DictMap) -> Value {
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

    /// The type name as a message says it: `an int`, `a str`, `null`.
    pub fn a_type_name(&self) -> String {
        match self.type_name() {
            "null" => "null".to_string(),
            t @ ("int" | "error") => format!("an {t}"),
            t => format!("a {t}"),
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

/// A string inside a container: double-quoted, with `\" \\ \n \r \t`, and `\u{..}` for other
/// control characters and the invisible ones (a byte-order mark, zero-width and bidi
/// characters, a soft hyphen), so `["\u{feff}name"]` shows what `==` will see (5.10).
pub fn quoted(s: &str, out: &mut String) {
    out.push('"');
    let chars: Vec<char> = s.chars().collect();
    // A joiner between two letters or emoji (a family emoji, a Persian word) is part of how they
    // read; next to ASCII, a space or an end it is a stray invisible character.
    let joins = |i: usize| {
        let shown = |c: Option<&char>| c.is_some_and(|c| !c.is_ascii() && !c.is_whitespace());
        shown(i.checked_sub(1).and_then(|j| chars.get(j))) && shown(chars.get(i + 1))
    };
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{200C}' | '\u{200D}' if joins(i) => out.push(c),
            // Invisible characters, and spaces other than the plain one (a no-break space from a
            // web page or Excel looks like one and isn't equal to it); WSL's stand-ins for what a
            // Windows name can't hold (U+F03A for the `:` of a Linux name, `span::is_stand_in`).
            c if c.is_control()
                || crate::syntax::span::is_stand_in(c)
                || matches!(c as u32, 0x00A0 | 0x00AD | 0x1680 | 0x2000..=0x200F | 0x2028..=0x202F | 0x205F..=0x2064 | 0x3000 | 0xFEFF) =>
            {
                out.push_str(&format!("\\u{{{:x}}}", c as u32))
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_grows_in_place_only_when_nothing_else_holds_the_string() {
        // A str is an `Rc<str>` until it grows, and no bigger than one.
        assert_eq!(std::mem::size_of::<Str>(), std::mem::size_of::<Rc<str>>());
        assert_eq!(std::mem::size_of::<Value>(), 24);
        let literal: Rc<str> = Rc::from("ab");
        let mut s = Str::from(literal.clone());
        assert_eq!(s.addr(), Rc::as_ptr(&literal) as *const u8 as usize, "made from an Rc<str>: shared");
        s.append("c");
        assert_eq!((&*s, &*literal), ("abc", "ab"), "the first append copies");
        let addr = s.addr();
        s.append("d");
        assert_eq!((&*s, s.addr()), ("abcd", addr), "alone: in place");
        let other = s.clone();
        s.append("e");
        assert_eq!((&*s, &*other), ("abcde", "abcd"), "shared: the other holder keeps its text");
        assert_ne!(s.addr(), other.addr());
        drop(other);
        // The char cache holds a `WeakStr`: its entry must not see the text change under it.
        let weak = s.downgrade();
        let addr = s.addr();
        s.append("f");
        assert_eq!((&*s, weak.alive(), weak.addr()), ("abcdef", false, addr));
        assert_eq!(Rc::<str>::from(&s), Rc::from("abcdef"));
        // Grown or not, the text alone decides `==` and order.
        assert!(Str::from("abcdef") == s && Str::from("abcdeg") > s && Str::from("abc") < s);
    }

    /// Removals, keys put back and the compactions behind them keep a `DictMap` in step with a
    /// plain list of pairs, with no more holes than entries (plus a few).
    #[test]
    fn dict_map_keeps_insertion_order() {
        let pairs = |map: &DictMap| -> Vec<(String, i64)> {
            map.iter().map(|(k, v)| (k.to_string(), if let Value::Int(n) = v { *n } else { unreachable!("ints only") })).collect()
        };
        let mut map = DictMap::new();
        let mut model: Vec<(String, i64)> = Vec::new();
        let mut seed = 7u64;
        for step in 0..20_000i64 {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            // Few keys early (a small dict, no index; then one around the cut), so most steps hit
            // one, then more, so the map grows and shrinks.
            let keys = match step {
                0..3_000 => 6,
                3_000..6_000 => 14,
                6_000..10_000 => 40,
                _ => 2_000,
            };
            let key = format!("k{}", (seed >> 33) % keys);
            let found = model.iter().position(|(k, _)| *k == key);
            if seed & (1 << 20) == 0 {
                let removed = map.remove(&key).map(|v| matches!(v, Value::Int(_)));
                assert_eq!(removed, found.map(|i| model.remove(i)).map(|_| true), "remove {key}");
            } else {
                let old = map.insert(Rc::from(key.as_str()), Value::Int(step));
                assert_eq!(old.is_some(), found.is_some(), "insert {key}");
                match found {
                    Some(i) => model[i].1 = step,
                    None => model.push((key, step)),
                }
            }
            assert_eq!(map.len(), model.len());
            assert!(map.slots.len() <= 2 * map.len() + SMALL, "holes are bounded");
            assert_eq!(map.index.len(), if map.slots.len() > SMALL { map.len() } else { 0 }, "indexed only past SMALL");
            if step % 97 == 0 {
                assert_eq!(pairs(&map), model);
                assert!(model.iter().all(|(k, n)| matches!(map.get(k), Some(Value::Int(m)) if m == n) && map.contains_key(k)));
            }
        }
        assert_eq!(pairs(&map), model);
        assert_eq!(pairs(&map.clone()), model);
        let collected: DictMap = model.iter().map(|(k, n)| (Rc::from(k.as_str()), Value::Int(*n))).collect();
        assert_eq!(pairs(&collected), model);
    }
}
