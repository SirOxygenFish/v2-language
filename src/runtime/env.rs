//! Frames and environments (INTERNALS 5.6).

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;

use super::value::{Callable, Value};

/// A frame and the frames it can see. Builtins frame ← module frame ← call frames ← block
/// frames.
pub type Env = Rc<RefCell<Frame>>;

pub struct Frame {
    pub vars: IndexMap<Rc<str>, Slot>,
    pub parent: Option<Env>,
    /// Closures created over this frame; when non-zero, leaving the frame looks for the
    /// closure-in-own-frame cycle (5.6).
    pub closures: u32,
}

pub struct Slot {
    pub value: Value,
    pub is_const: bool,
}

pub enum AssignError {
    NotDeclared,
    Const,
    /// A name found only in the builtins frame, which is read-only (5.6).
    Builtin,
}

pub fn new_frame(parent: Option<&Env>) -> Env {
    Rc::new(RefCell::new(Frame { vars: IndexMap::new(), parent: parent.cloned(), closures: 0 }))
}

/// Declare `name` in `env` itself. Redeclaration is a static error (4.5), so this replaces.
pub fn declare(env: &Env, name: Rc<str>, value: Value, is_const: bool) {
    env.borrow_mut().vars.insert(name, Slot { value, is_const });
}

pub fn lookup(env: &Env, name: &str) -> Option<Value> {
    let mut cur = env.clone();
    loop {
        let next = {
            let f = cur.borrow();
            if let Some(slot) = f.vars.get(name) {
                return Some(slot.value.clone());
            }
            f.parent.clone()
        };
        cur = next?;
    }
}

pub fn assign(env: &Env, name: &str, value: Value) -> Result<(), AssignError> {
    let mut cur = env.clone();
    loop {
        let next = {
            let mut f = cur.borrow_mut();
            let builtins = f.parent.is_none();
            if let Some(slot) = f.vars.get_mut(name) {
                if builtins {
                    return Err(AssignError::Builtin);
                }
                if slot.is_const {
                    return Err(AssignError::Const);
                }
                slot.value = value;
                return Ok(());
            }
            f.parent.clone()
        };
        cur = next.ok_or(AssignError::NotDeclared)?;
    }
}

/// Every visible name, innermost frame first, each frame in declaration order (the did-you-mean
/// tie-break, 5.9).
pub fn visible_names(env: &Env) -> Vec<Rc<str>> {
    let mut names = Vec::new();
    let mut cur = Some(env.clone());
    while let Some(e) = cur {
        let f = e.borrow();
        names.extend(f.vars.keys().cloned());
        cur = f.parent.clone();
    }
    names
}

/// Called when `env` goes out of scope. A closure stored in the frame it captures (a local
/// `func`, `let f = func …`) keeps the frame alive through an `Rc` cycle; if nothing else
/// reaches the frame, clear its slots so the cycle is freed (5.6). Cycles through containers
/// are not trimmed.
pub fn leave(env: Env) {
    if env.borrow().closures == 0 {
        return;
    }
    let own = {
        let f = env.borrow();
        f.vars
            .values()
            .filter(|slot| match &slot.value {
                Value::Func(c) => {
                    c.strong_count() == 1 && matches!(&**c, Callable::Closure { env: e, .. } if Rc::ptr_eq(e, &env))
                }
                _ => false,
            })
            .count()
    };
    if Rc::strong_count(&env) == 1 + own {
        // Take the slots out first: dropping them may drop closures, which release `env`.
        let vars = std::mem::take(&mut env.borrow_mut().vars);
        drop(vars);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::{ExprKind, StmtKind};
    use crate::syntax::parser::parse_str;

    /// A closure over `frame`, stored in `frame` (the cycle of a local `func`).
    fn closure_in(frame: &Env) -> Value {
        let prog = parse_str("let f = func() => 1").unwrap();
        let StmtKind::Let(_, Some(e)) = &prog[0].kind else { panic!("let") };
        let ExprKind::Func(decl) = &e.kind else { panic!("func") };
        frame.borrow_mut().closures += 1;
        let f = Value::func(Callable::Closure { decl: decl.clone(), env: frame.clone(), name: "f".into() });
        declare(frame, "f".into(), f.clone(), false);
        f
    }

    #[test]
    fn leaving_a_frame_frees_its_closure_cycle() {
        let frame = new_frame(None);
        let weak = Rc::downgrade(&frame);
        drop(closure_in(&frame));
        leave(frame);
        assert!(weak.upgrade().is_none(), "the frame was freed");
    }

    #[test]
    fn an_escaping_closure_keeps_its_frame() {
        let frame = new_frame(None);
        let weak = Rc::downgrade(&frame);
        let escaped = closure_in(&frame);
        leave(frame);
        let kept = weak.upgrade().expect("the closure still reaches its frame");
        assert!(lookup(&kept, "f").is_some(), "its slots were not cleared");
        drop(kept);
        drop(escaped);
    }
}
