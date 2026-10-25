//! Foreign object handles and V2 callables inside workers (INTERNALS ch. 11): the host side of
//! both, and the operations V2 runs on handles (`op` and `call` frames, 9.5, 11.3, 11.4).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::{json, Map, Value as Json};

use crate::runtime::error::{fail, Exec};
use crate::runtime::interp::Interp;
use crate::runtime::value::Value;
use crate::syntax::span::Span;

/// A session's queue of handle ids to release, sent before its next frame (9.10, 11.2).
pub type ReleaseQueue = Rc<RefCell<Vec<u64>>>;

/// A V2 reference to an object that lives in a worker (11.2). Each decoded `h` tag is its own
/// `HandleRef`, matching one count on the worker side.
#[derive(Debug)]
pub struct HandleRef {
    pub session: usize,
    /// The owning engine's tag, for messages: `py`.
    pub tag: Rc<str>,
    pub id: u64,
    /// The foreign type, as diagnostics show it: `datetime.date`.
    pub type_name: Rc<str>,
    /// A function, method, class or callable object (the `h` tag's `callable`, 11.1, 11.6).
    pub callable: bool,
    releases: ReleaseQueue,
}

impl HandleRef {
    pub fn new(session: usize, tag: Rc<str>, id: u64, type_name: Rc<str>, releases: ReleaseQueue) -> HandleRef {
        HandleRef { session, tag, id, type_name, callable: false, releases }
    }

    pub fn with_callable(mut self, callable: bool) -> HandleRef {
        self.callable = callable;
        self
    }

    /// `<py datetime.date>`, as diagnostics show a handle (11.6).
    pub fn text(&self) -> String {
        format!("<{} {}>", self.tag, self.type_name)
    }

    /// Same session and id: the same foreign object (11.6).
    pub fn same(&self, other: &HandleRef) -> bool {
        self.session == other.session && self.id == other.id
    }
}

impl Drop for HandleRef {
    /// Only queues the id: never sends anything, so it is safe anywhere (11.2).
    fn drop(&mut self) {
        if let Ok(mut q) = self.releases.try_borrow_mut() {
            q.push(self.id);
        }
    }
}

/// The V2 functions (and other sessions' callable handles) a session holds stubs for (11.8): one
/// id per value, counted per `fn` tag sent, dropped when the worker has released every count.
#[derive(Default)]
pub struct CallableTable {
    next: u64,
    entries: HashMap<u64, (Value, u32)>,
    by_addr: HashMap<usize, u64>,
}

/// The `Rc` address that identifies a `Func` or a callable handle in the table.
fn addr(v: &Value) -> usize {
    match v {
        Value::Func(f) => f.addr(),
        Value::Handle(h) => Rc::as_ptr(h) as *const u8 as usize,
        _ => unreachable!("only functions and callable handles are registered"),
    }
}

impl CallableTable {
    /// The id for `v` (a `Func` or a callable handle); sending the same value again reuses its id
    /// and adds a count.
    pub fn register(&mut self, v: &Value) -> u64 {
        let key = addr(v);
        if let Some(&id) = self.by_addr.get(&key) {
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.1 += 1;
                return id;
            }
        }
        self.next += 1;
        let id = self.next;
        self.entries.insert(id, (v.clone(), 1));
        self.by_addr.insert(key, id);
        id
    }

    /// The value behind a `fn` tag the worker sends back; counts do not change.
    pub fn get(&self, id: u64) -> Option<Value> {
        self.entries.get(&id).map(|(v, _)| v.clone())
    }

    /// One count less per occurrence of each id; an entry goes at 0 (11.8). Unknown ids are
    /// ignored (9.10).
    pub fn release(&mut self, ids: &[u64]) {
        for id in ids {
            let Some(entry) = self.entries.get_mut(id) else { continue };
            entry.1 -= 1;
            if entry.1 == 0 {
                if let Some((v, _)) = self.entries.remove(id) {
                    self.by_addr.remove(&addr(&v));
                }
            }
        }
    }

    #[cfg(test)]
    pub fn count(&self, id: u64) -> u32 {
        self.entries.get(&id).map_or(0, |e| e.1)
    }
}

/// Items asked for per `next` (11.4).
const NEXT_BATCH: u64 = 64;

impl Interp {
    /// One `op` frame on `h` (9.5, 11.3) and the decoded reply; `what` names it in crash and
    /// timeout messages.
    fn handle_op(&mut self, h: &HandleRef, op: &str, members: Map<String, Json>, what: &str, at: Span) -> Exec<Value> {
        let mut frame = json!({"t": "op", "op": op, "h": h.id});
        for (k, v) in members {
            frame[k] = v;
        }
        Ok(self.request(h.session, frame, what, at)?.value)
    }

    /// `v` for `h`'s session, or the MarshalError naming `root` (10.5).
    fn encode_for(&mut self, h: &HandleRef, v: &Value, root: &str, at: Span) -> Exec<Json> {
        // A dead session's caps say nothing; its own error comes first (8.2).
        self.check_alive(h.session, at)?;
        self.wire(h.session).encode(v, root).map_err(|f| self.raise(f, at))
    }

    /// `h.name` (11.3).
    pub fn handle_getattr(&mut self, h: &HandleRef, name: &str, at: Span) -> Exec<Value> {
        let what = format!("reading '{name}' of {}", h.text());
        self.handle_op(h, "getattr", Map::from_iter([("name".to_string(), json!(name))]), &what, at)
    }

    /// `h.name = v` (11.3).
    pub fn handle_setattr(&mut self, h: &HandleRef, name: &str, v: &Value, at: Span) -> Exec<()> {
        let value = self.encode_for(h, v, "value", at)?;
        let what = format!("setting '{name}' of {}", h.text());
        self.handle_op(h, "setattr", Map::from_iter([("name".to_string(), json!(name)), ("value".to_string(), value)]), &what, at)?;
        Ok(())
    }

    /// `h[k]` (11.3).
    pub fn handle_getitem(&mut self, h: &HandleRef, k: &Value, at: Span) -> Exec<Value> {
        let key = self.encode_for(h, k, "key", at)?;
        let what = format!("indexing {}", h.text());
        self.handle_op(h, "getitem", Map::from_iter([("key".to_string(), key)]), &what, at)
    }

    /// `h[k] = v` (11.3).
    pub fn handle_setitem(&mut self, h: &HandleRef, k: &Value, v: &Value, at: Span) -> Exec<()> {
        let key = self.encode_for(h, k, "key", at)?;
        let value = match self.encode_for(h, v, "value", at) {
            Ok(j) => j,
            Err(flow) => {
                self.sessions[h.session].callables.release(&crate::codec::vep::fn_ids(&key));
                return Err(flow);
            }
        };
        let what = format!("setting an item of {}", h.text());
        self.handle_op(h, "setitem", Map::from_iter([("key".to_string(), key), ("value".to_string(), value)]), &what, at)?;
        Ok(())
    }

    /// `len(h)` (11.3).
    pub fn handle_len(&mut self, h: &HandleRef, at: Span) -> Exec<i64> {
        let what = format!("len() of {}", h.text());
        match self.handle_op(h, "len", Map::new(), &what, at)? {
            Value::Int(n) => Ok(n),
            other => fail("ProtocolError", format!("engine '{}' answered len() with {}", h.tag, other.type_name())),
        }
    }

    /// `str(h)`: the foreign text form (11.3).
    pub fn handle_str(&mut self, h: &HandleRef, at: Span) -> Exec<String> {
        let what = format!("str() of {}", h.text());
        match self.handle_op(h, "str", Map::new(), &what, at)? {
            Value::Str(s) => Ok(s.to_string()),
            other => fail("ProtocolError", format!("engine '{}' answered str() with {}", h.tag, other.type_name())),
        }
    }

    /// `to_v2(h)`: the object as plain data, converted inside the worker (11.5).
    pub fn handle_value(&mut self, h: &HandleRef, at: Span) -> Exec<Value> {
        let what = format!("to_v2() of {}", h.text());
        self.handle_op(h, "value", Map::new(), &what, at)
    }

    /// `h(args)`, or `h.method(args)` in one round trip (9.5, 11.3).
    pub fn handle_call(&mut self, h: &HandleRef, method: Option<&str>, pos: Vec<Value>, named: Vec<(Rc<str>, Value)>, at: Span) -> Exec<Value> {
        let target = match method {
            Some(m) => json!({"h": h.id, "method": m}),
            None => json!({"h": h.id}),
        };
        let what = match method {
            Some(m) => format!("a call to '{m}'"),
            None => format!("a call to {}", h.text()),
        };
        self.foreign_call(h.session, target, pos, named, &what, at)
    }

    /// A `call` frame to session `s` (9.5): a handle or a Proxy (13.4). Named args need the
    /// session's `kwargs` capability (10.8); `what` names the call in crash and timeout messages.
    pub fn foreign_call(&mut self, s: usize, target: Json, pos: Vec<Value>, named: Vec<(Rc<str>, Value)>, what: &str, at: Span) -> Exec<Value> {
        self.check_alive(s, at)?;
        if let Some((k, _)) = named.first() {
            if !self.sessions[s].caps.kwargs {
                let tag = self.sessions[s].key.clone();
                return fail("ArityError", format!("{tag} functions take positional arguments only (got '{k}')")).map_err(|f| self.raise(f, at));
            }
        }
        let mut args = Vec::with_capacity(pos.len());
        let mut kwargs = Map::new();
        let encoded = pos.iter().enumerate().map(|(i, v)| (format!("args[{i}]"), None, v)).chain(named.iter().map(|(k, v)| (format!("kwargs[\"{k}\"]"), Some(k), v)));
        for (root, key, v) in encoded {
            match self.wire(s).encode(v, &root) {
                Ok(j) => match key {
                    Some(k) => {
                        kwargs.insert(k.to_string(), j);
                    }
                    None => args.push(j),
                },
                Err(flow) => {
                    // The frame is never sent: what was encoded gives its counts back (10.1).
                    let sent = json!({"args": args, "kwargs": kwargs});
                    self.sessions[s].callables.release(&crate::codec::vep::fn_ids(&sent));
                    return Err(self.raise(flow, at));
                }
            }
        }
        let frame = json!({"t": "call", "target": target, "args": args, "kwargs": kwargs});
        Ok(self.request(s, frame, what, at)?.value)
    }

    /// `for x in h` starts here (11.4): the iterator's handle, and whether `h` is a mapping
    /// (whose items are `[key, value]` pairs).
    pub fn handle_iter(&mut self, h: &HandleRef, at: Span) -> Exec<(Rc<HandleRef>, bool)> {
        let what = format!("iterating {}", h.text());
        let reply = self.handle_op(h, "iter", Map::new(), &what, at)?;
        let parts = match &reply {
            Value::Dict(d) => {
                let d = d.borrow();
                (d.get("it").cloned(), d.get("mapping").cloned())
            }
            _ => (None, None),
        };
        match parts {
            (Some(Value::Handle(it)), Some(Value::Bool(mapping))) => Ok((it, mapping)),
            _ => fail("ProtocolError", format!("engine '{}' answered iter without an iterator handle", h.tag)).map_err(|f| self.raise(f, at)),
        }
    }

    /// The next items of an iterator handle, and whether it is done (11.4).
    pub fn handle_next(&mut self, it: &HandleRef, at: Span) -> Exec<(Vec<Value>, bool)> {
        let what = format!("iterating {}", it.text());
        let reply = self.handle_op(it, "next", Map::from_iter([("n".to_string(), json!(NEXT_BATCH))]), &what, at)?;
        let parts = match &reply {
            Value::Dict(d) => {
                let d = d.borrow();
                (d.get("items").cloned(), d.get("done").cloned())
            }
            _ => (None, None),
        };
        match parts {
            (Some(Value::List(items)), Some(Value::Bool(done))) => Ok((items.borrow().clone(), done)),
            _ => fail("ProtocolError", format!("engine '{}' answered next without items", it.tag)).map_err(|f| self.raise(f, at)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::builtins::BUILTINS;
    use crate::runtime::value::{Callable, Func};

    fn builtin(name: &str) -> Value {
        Value::Func(Func::new(Callable::Builtin(BUILTINS.iter().find(|b| b.name == name).unwrap())))
    }

    #[test]
    fn handle_drop_only_queues() {
        let q: ReleaseQueue = Rc::new(RefCell::new(Vec::new()));
        let a = HandleRef::new(0, "py".into(), 7, "datetime.date".into(), q.clone());
        let b = HandleRef::new(0, "py".into(), 7, "datetime.date".into(), q.clone());
        let c = HandleRef::new(1, "js".into(), 7, "Date".into(), q.clone());
        assert!(a.same(&b) && !a.same(&c));
        drop(a);
        drop(b);
        assert_eq!(*q.borrow(), [7, 7], "one id per reference");
        // A queue that is borrowed (it never is across calls) loses the id rather than panic.
        let held = q.borrow_mut();
        drop(c);
        drop(held);
        assert_eq!(q.borrow().len(), 2);
    }

    #[test]
    fn callables_reuse_ids_and_count() {
        let mut t = CallableTable::default();
        let (print, len) = (builtin("print"), builtin("len"));
        let p = t.register(&print);
        assert_eq!(t.register(&print.clone()), p, "same function, same id");
        let l = t.register(&len);
        assert_ne!(p, l);
        assert_eq!((t.count(p), t.count(l)), (2, 1));
        assert!(matches!((t.get(p), &print), (Some(Value::Func(f)), Value::Func(g)) if Func::ptr_eq(&f, g)));
        // A callable handle is keyed by its `HandleRef`, like a function by its `Rc`.
        let h = Value::Handle(Rc::new(HandleRef::new(1, "js".into(), 3, "function".into(), Rc::default()).with_callable(true)));
        let id = t.register(&h);
        assert_eq!(t.register(&h.clone()), id);
        assert!(matches!(t.get(id), Some(Value::Handle(r)) if r.id == 3));
        t.release(&[p, 99]);
        assert_eq!(t.count(p), 1);
        t.release(&[p, l]);
        assert!(t.get(p).is_none() && t.get(l).is_none());
        assert_ne!(t.register(&print), p, "a released function gets a new id");
    }
}
