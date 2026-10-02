//! Foreign object handles and V2 callables inside workers (INTERNALS ch. 11). The host side of
//! both lives here; handle operations (`op` frames) arrive in roadmap 1.6.

#![allow(dead_code)] // sessions (roadmap 1.3) are the first callers

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::runtime::value::{Func, Value};

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
    releases: ReleaseQueue,
}

impl HandleRef {
    pub fn new(session: usize, tag: Rc<str>, id: u64, type_name: Rc<str>, releases: ReleaseQueue) -> HandleRef {
        HandleRef { session, tag, id, type_name, releases }
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

/// The V2 functions a session holds stubs for (11.8): one id per function, counted per `fn` tag
/// sent, dropped when the worker has released every count.
#[derive(Default)]
pub struct CallableTable {
    next: u64,
    entries: HashMap<u64, (Func, u32)>,
    by_addr: HashMap<usize, u64>,
}

impl CallableTable {
    /// The id for `f`; sending the same function again reuses its id and adds a count.
    pub fn register(&mut self, f: &Func) -> u64 {
        if let Some(&id) = self.by_addr.get(&f.addr()) {
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.1 += 1;
                return id;
            }
        }
        self.next += 1;
        let id = self.next;
        self.entries.insert(id, (f.clone(), 1));
        self.by_addr.insert(f.addr(), id);
        id
    }

    /// The function behind a `fn` tag the worker sends back; counts do not change.
    pub fn get(&self, id: u64) -> Option<Value> {
        self.entries.get(&id).map(|(f, _)| Value::Func(f.clone()))
    }

    /// One count less per occurrence of each id; an entry goes at 0 (11.8). Unknown ids are
    /// ignored (9.10).
    pub fn release(&mut self, ids: &[u64]) {
        for id in ids {
            let Some(entry) = self.entries.get_mut(id) else { continue };
            entry.1 -= 1;
            if entry.1 == 0 {
                if let Some((f, _)) = self.entries.remove(id) {
                    self.by_addr.remove(&f.addr());
                }
            }
        }
    }

    #[cfg(test)]
    pub fn count(&self, id: u64) -> u32 {
        self.entries.get(&id).map_or(0, |e| e.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::builtins::BUILTINS;
    use crate::runtime::value::Callable;

    fn builtin(name: &str) -> Func {
        Func::new(Callable::Builtin(BUILTINS.iter().find(|b| b.name == name).unwrap()))
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
        assert!(matches!(t.get(p), Some(Value::Func(f)) if Func::ptr_eq(&f, &print)));
        t.release(&[p, 99]);
        assert_eq!(t.count(p), 1);
        t.release(&[p, l]);
        assert!(t.get(p).is_none() && t.get(l).is_none());
        assert_ne!(t.register(&print), p, "a released function gets a new id");
    }
}
