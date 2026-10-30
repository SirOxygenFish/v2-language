//! Frames and environments (INTERNALS 5.6).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::value::{Callable, Value};
use crate::syntax::ast::Layout;

/// A frame and the frames it can see. Builtins frame ← module frame ← call frames ← block
/// frames.
pub type Env = Rc<Frame>;

pub struct Frame {
    /// The names of `slots`. Only `@import` in a test's hand-made frame adds to it.
    layout: RefCell<Layout>,
    /// `None`: not declared yet.
    slots: RefCell<Vec<Option<Slot>>>,
    pub parent: Option<Env>,
    /// Closures created over this frame; when non-zero, leaving the frame looks for the
    /// closure-in-own-frame cycle (5.6).
    pub closures: Cell<u32>,
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

/// Why a write by position didn't happen. `Empty` hands the value back: the name isn't declared
/// here yet, so it means an outer one (5.6).
pub enum WriteError {
    Const,
    Empty(Value),
}

thread_local! {
    /// Slot arrays of frames that are gone: a malloc and a free less per call.
    static SPARE: RefCell<Vec<Vec<Option<Slot>>>> = const { RefCell::new(Vec::new()) };
}

/// How many spare arrays to keep: about the depth of a program's usual recursion.
const MAX_SPARE: usize = 256;

pub fn new_frame(layout: &Layout, parent: Option<&Env>) -> Env {
    let mut slots = SPARE.with(|s| s.borrow_mut().pop()).unwrap_or_default();
    slots.resize_with(layout.len(), || None);
    Rc::new(Frame { layout: RefCell::new(layout.clone()), slots: RefCell::new(slots), parent: parent.cloned(), closures: Cell::new(0) })
}

impl Drop for Frame {
    fn drop(&mut self) {
        let mut slots = std::mem::take(self.slots.get_mut());
        slots.clear();
        // Gone at thread exit, when the array is simply freed.
        let _ = SPARE.try_with(|s| {
            let mut spare = s.borrow_mut();
            if spare.len() < MAX_SPARE {
                spare.push(slots);
            }
        });
    }
}

/// Declare the name at `index` of `env`'s layout.
#[inline]
pub fn declare_at(env: &Env, index: u32, value: Value, is_const: bool) {
    env.slots.borrow_mut()[index as usize] = Some(Slot { value, is_const });
}

/// Declare `name` in `env` itself, by name: what the resolver leaves to the statement (13.4).
/// Redeclaration is a static error (4.5), so this replaces.
pub fn declare(env: &Env, name: Rc<str>, value: Value, is_const: bool) {
    let found = env.layout.borrow().iter().position(|n| *n == name);
    let index = found.unwrap_or_else(|| {
        let mut layout = env.layout.borrow_mut();
        let extended: Layout = layout.iter().cloned().chain(std::iter::once(name)).collect();
        *layout = extended;
        env.slots.borrow_mut().push(None);
        layout.len() - 1
    });
    declare_at(env, index as u32, value, is_const);
}

/// The frame `hops` up from `env`.
#[inline]
fn frame_at(env: &Env, hops: u32) -> &Env {
    let mut f = env;
    for _ in 0..hops {
        f = f.parent.as_ref().expect("the resolver counted frames that exist");
    }
    f
}

/// The value at a position, or `None` if it isn't declared yet.
#[inline]
pub fn read(env: &Env, hops: u32, index: u32) -> Option<Value> {
    frame_at(env, hops).slots.borrow()[index as usize].as_ref().map(|slot| slot.value.clone())
}

/// Write the slot at a position.
#[inline]
pub fn write(env: &Env, hops: u32, index: u32, value: Value) -> Result<(), WriteError> {
    let old = match &mut frame_at(env, hops).slots.borrow_mut()[index as usize] {
        Some(slot) if slot.is_const => return Err(WriteError::Const),
        Some(slot) => std::mem::replace(&mut slot.value, value),
        None => return Err(WriteError::Empty(value)),
    };
    drop(old);
    Ok(())
}

/// Where `name` is declared by now in `f` itself.
fn position(f: &Frame, name: &str) -> Option<usize> {
    let index = f.layout.borrow().iter().position(|n| &**n == name)?;
    f.slots.borrow()[index].is_some().then_some(index)
}

/// The value of `name` in `env` itself.
pub fn own_value(env: &Env, name: &str) -> Option<Value> {
    position(env, name).and_then(|i| env.slots.borrow()[i].as_ref().map(|slot| slot.value.clone()))
}

/// By name, innermost frame first, skipping what isn't declared yet.
pub fn lookup(env: &Env, name: &str) -> Option<Value> {
    let mut cur = Some(env);
    while let Some(f) = cur {
        if let Some(v) = own_value(f, name) {
            return Some(v);
        }
        cur = f.parent.as_ref();
    }
    None
}

/// By name, as `lookup` finds it.
pub fn assign(env: &Env, name: &str, value: Value) -> Result<(), AssignError> {
    let mut cur = Some(env);
    while let Some(f) = cur {
        if let Some(index) = position(f, name) {
            if f.parent.is_none() {
                return Err(AssignError::Builtin);
            }
            return match write(f, 0, index as u32, value) {
                Ok(()) => Ok(()),
                Err(_) => Err(AssignError::Const),
            };
        }
        cur = f.parent.as_ref();
    }
    Err(AssignError::NotDeclared)
}

/// The frame's bindings in layout order, without what was never declared.
pub fn bindings(env: &Env) -> Vec<(Rc<str>, Value)> {
    let layout = env.layout.borrow();
    let slots = env.slots.borrow();
    layout.iter().zip(slots.iter()).filter_map(|(name, slot)| slot.as_ref().map(|s| (name.clone(), s.value.clone()))).collect()
}

/// Every visible name, innermost frame first, each frame in declaration order (the did-you-mean
/// tie-break, 5.9). A name not declared yet isn't visible.
pub fn visible_names(env: &Env) -> Vec<Rc<str>> {
    let mut names = Vec::new();
    let mut cur = Some(env);
    while let Some(f) = cur {
        let layout = f.layout.borrow();
        let slots = f.slots.borrow();
        names.extend(layout.iter().zip(slots.iter()).filter(|(_, slot)| slot.is_some()).map(|(name, _)| name.clone()));
        cur = f.parent.as_ref();
    }
    names
}

/// Take a frame's values out, to free them outside it.
pub fn take_values(env: &Env) -> Vec<Value> {
    match env.slots.try_borrow_mut() {
        Ok(mut slots) => std::mem::take(&mut *slots).into_iter().flatten().map(|slot| slot.value).collect(),
        Err(_) => Vec::new(),
    }
}

/// Called when `env` goes out of scope. A closure stored in the frame it captures (a local
/// `func`, `let f = func …`) keeps the frame alive through an `Rc` cycle; if nothing else
/// reaches the frame, clear its slots so the cycle is freed (5.6). Cycles through containers
/// are not trimmed.
pub fn leave(env: Env) {
    if env.closures.get() == 0 {
        return;
    }
    let own = env
        .slots
        .borrow()
        .iter()
        .flatten()
        .filter(|slot| match &slot.value {
            Value::Func(c) => {
                c.strong_count() == 1 && matches!(&**c, Callable::Closure { env: e, .. } if Rc::ptr_eq(e, &env))
            }
            _ => false,
        })
        .count();
    if Rc::strong_count(&env) == 1 + own {
        // Take the slots out first: dropping them may drop closures, which release `env`.
        let slots = std::mem::take(&mut *env.slots.borrow_mut());
        drop(slots);
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
        frame.closures.set(frame.closures.get() + 1);
        let f = Value::func(Callable::Closure { decl: decl.clone(), env: frame.clone(), name: "f".into() });
        declare(frame, "f".into(), f.clone(), false);
        f
    }

    fn frame() -> Env {
        new_frame(&Layout::from([]), None)
    }

    #[test]
    fn leaving_a_frame_frees_its_closure_cycle() {
        let frame = frame();
        let weak = Rc::downgrade(&frame);
        drop(closure_in(&frame));
        leave(frame);
        assert!(weak.upgrade().is_none(), "the frame was freed");
    }

    #[test]
    fn an_escaping_closure_keeps_its_frame() {
        let frame = frame();
        let weak = Rc::downgrade(&frame);
        let escaped = closure_in(&frame);
        leave(frame);
        let kept = weak.upgrade().expect("the closure still reaches its frame");
        assert!(lookup(&kept, "f").is_some(), "its slots were not cleared");
        drop(kept);
        drop(escaped);
    }

    #[test]
    fn an_empty_slot_means_an_outer_name() {
        let layout = Layout::from(["x".into()]);
        let outer = new_frame(&layout, None);
        let inner = new_frame(&layout, Some(&outer));
        declare_at(&outer, 0, Value::Int(1), false);
        assert!(read(&inner, 0, 0).is_none());
        assert!(matches!(lookup(&inner, "x"), Some(Value::Int(1))));
        assert!(matches!(write(&inner, 0, 0, Value::Int(2)), Err(WriteError::Empty(Value::Int(2)))));
        assert!(matches!(write(&inner, 1, 0, Value::Int(3)), Ok(())));
        assert!(matches!(read(&outer, 0, 0), Some(Value::Int(3))));
        assert_eq!(visible_names(&inner).len(), 1, "only what is declared is visible");
    }
}
