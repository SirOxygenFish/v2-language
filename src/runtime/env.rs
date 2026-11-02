//! Frames and environments (INTERNALS 5.6).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use super::error::ErrorValue;
use super::value::{release, Callable, Dict, Func, List, Value};
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
/// A name's value in this frame, if its slot is a constant one.
pub fn own_const(env: &Env, name: &str) -> Option<Value> {
    position(env, name).and_then(|i| env.slots.borrow()[i].as_ref().filter(|slot| slot.is_const).map(|slot| slot.value.clone()))
}

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

thread_local! {
    /// Frames left while something else still held them, and the count that sweeps them.
    static SUSPECTS: RefCell<(Vec<Weak<Frame>>, usize)> = const { RefCell::new((Vec::new(), FIRST_SWEEP)) };
}

const FIRST_SWEEP: usize = 256;

/// Called when `env` goes out of scope. A closure over the frame that the frame itself holds,
/// in a slot or in a list or dict in one, makes an `Rc` cycle. If only that cycle holds the
/// frame now, its slots are cleared so it goes; if something else still does (a closure that
/// was returned), the frame is checked again in later sweeps (5.6).
pub fn leave(env: Env) {
    // Nothing else holds it, so no closure does: it goes when `env` does.
    if env.closures.get() == 0 || Rc::strong_count(&env) == 1 {
        return;
    }
    if only_itself_holds(&env) {
        release(take_values(&env));
        return;
    }
    let due = SUSPECTS.with(|s| {
        let mut s = s.borrow_mut();
        s.0.push(Rc::downgrade(&env));
        s.0.len() >= s.1
    });
    drop(env);
    if due {
        sweep();
    }
}

/// Clear every remembered frame that only its own values hold now. The next sweep comes after
/// twice as many frames as were kept, so the work stays in proportion.
pub fn sweep() {
    let frames = SUSPECTS.with(|s| std::mem::take(&mut s.borrow_mut().0));
    let mut kept = Vec::new();
    for weak in frames {
        let Some(env) = weak.upgrade() else { continue };
        if only_itself_holds(&env) {
            release(take_values(&env));
        } else {
            kept.push(weak);
        }
    }
    SUSPECTS.with(|s| {
        let mut s = s.borrow_mut();
        s.1 = FIRST_SWEEP.max(2 * kept.len());
        s.0.extend(kept);
    });
}

/// A frame, list, dict, function or error reached from a frame's slots, held once here.
enum Node {
    Frame(Env),
    List(List),
    Dict(Dict),
    Func(Func),
    Error(Rc<ErrorValue>),
}

impl Node {
    fn of(v: &Value) -> Option<Node> {
        match v {
            Value::List(l) => Some(Node::List(l.clone())),
            Value::Dict(d) => Some(Node::Dict(d.clone())),
            Value::Func(f) => Some(Node::Func(f.clone())),
            Value::Error(e) => Some(Node::Error(e.clone())),
            _ => None,
        }
    }

    fn addr(&self) -> usize {
        match self {
            Node::Frame(f) => Rc::as_ptr(f) as usize,
            Node::List(l) => l.addr(),
            Node::Dict(d) => d.addr(),
            Node::Func(f) => f.addr(),
            Node::Error(e) => Rc::as_ptr(e) as usize,
        }
    }

    fn count(&self) -> usize {
        match self {
            Node::Frame(f) => Rc::strong_count(f),
            Node::List(l) => l.strong_count(),
            Node::Dict(d) => d.strong_count(),
            Node::Func(f) => f.strong_count(),
            Node::Error(e) => Rc::strong_count(e),
        }
    }

    /// What it holds, into `out`, spending `budget` per value; false if it is being changed
    /// right now or the budget is spent.
    fn holds(&self, out: &mut Vec<Node>, budget: &mut usize) -> bool {
        let mut take = |values: &mut dyn Iterator<Item = &Value>| {
            for v in values {
                if *budget == 0 {
                    return false;
                }
                *budget -= 1;
                out.extend(Node::of(v));
            }
            true
        };
        match self {
            Node::Frame(f) => frame_holds(f, out, budget),
            Node::List(l) => l.try_borrow().is_ok_and(|items| take(&mut items.iter())),
            Node::Dict(d) => d.try_borrow().is_ok_and(|map| take(&mut map.values())),
            Node::Func(f) => {
                match &**f {
                    Callable::Closure { env, .. } => out.push(Node::Frame(env.clone())),
                    Callable::Waiting { resolved, .. } => out.extend(resolved.get().and_then(Node::of)),
                    _ => {}
                }
                true
            }
            Node::Error(e) => take(&mut [&e.data, &e.cause].into_iter()),
        }
    }
}

fn has_ancestor(f: &Frame, addr: usize) -> bool {
    let mut p = f.parent.as_ref();
    while let Some(frame) = p {
        if Rc::as_ptr(frame) as usize == addr {
            return true;
        }
        p = frame.parent.as_ref();
    }
    false
}

fn frame_holds(f: &Frame, out: &mut Vec<Node>, budget: &mut usize) -> bool {
    let Ok(slots) = f.slots.try_borrow() else { return false };
    for slot in slots.iter().flatten() {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        out.extend(Node::of(&slot.value));
    }
    out.extend(f.parent.clone().map(Node::Frame));
    true
}

/// Whether nothing outside `env`'s own values still reaches it. Every frame, list, dict, function
/// and error reached from its slots is counted with the references to it from inside that group;
/// one with more is held from outside, so it lives, and so does whatever it reaches. `env` is
/// garbage if none of that reaches it. Only `env` and the frames below it are walked, not
/// another call's frame or one of an earlier pass of a loop. A reference this misses only makes
/// more look held, so a wrong answer can keep memory but never frees what is in use. A big group
/// isn't walked.
fn only_itself_holds(env: &Env) -> bool {
    const MAX_NODES: usize = 2_000;
    let me = Rc::as_ptr(env) as usize;
    // Each value reached, held once more here: its address, its references from inside, and the
    // value; `env` is first, with no value. `edges` are the references, as (holder, held) indexes.
    let mut nodes: Vec<(usize, usize, Option<Node>)> = vec![(me, 0, None)];
    let mut edges: Vec<(usize, usize)> = Vec::new();
    // Found by address in that list while it is short, then through a map.
    let mut by_addr: Option<HashMap<usize, usize>> = None;
    let mut budget = 100_000; // items looked at, plain ones included
    let mut out = Vec::new();
    if !frame_holds(env, &mut out, &mut budget) {
        return false;
    }
    let mut todo = Vec::new();
    let mut from = 0;
    loop {
        for node in out.drain(..) {
            let addr = node.addr();
            // The frames above `env` live on with the code that runs in them.
            if has_ancestor(env, addr) || matches!(&node, Node::Frame(f) if addr != me && !has_ancestor(f, me)) {
                continue;
            }
            let found = match &by_addr {
                Some(map) => map.get(&addr).copied(),
                None => nodes.iter().position(|n| n.0 == addr),
            };
            let k = match found {
                Some(k) => k,
                None => {
                    nodes.push((addr, 0, Some(node)));
                    let k = nodes.len() - 1;
                    match &mut by_addr {
                        Some(map) => {
                            map.insert(addr, k);
                        }
                        None if k >= 32 => by_addr = Some(nodes.iter().enumerate().map(|(k, n)| (n.0, k)).collect()),
                        None => {}
                    }
                    todo.push(k);
                    k
                }
            };
            nodes[k].1 += 1;
            edges.push((from, k));
        }
        let Some(k) = todo.pop() else { break };
        from = k;
        if nodes.len() > MAX_NODES || !nodes[k].2.as_ref().is_some_and(|node| node.holds(&mut out, &mut budget)) {
            return false;
        }
    }
    // What something outside holds lives, with all it reaches here.
    let held = |n: &(usize, usize, Option<Node>)| match &n.2 {
        Some(node) => node.count() > n.1 + 1,
        None => Rc::strong_count(env) > n.1 + 1, // more than the caller's
    };
    let mut live: Vec<usize> = (0..nodes.len()).filter(|&k| held(&nodes[k])).collect();
    if live.is_empty() {
        return true;
    }
    let mut seen = vec![false; nodes.len()];
    while let Some(k) = live.pop() {
        if k == 0 {
            return false;
        }
        if !std::mem::replace(&mut seen[k], true) {
            live.extend(edges.iter().filter(|e| e.0 == k).map(|e| e.1));
        }
    }
    true
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

    /// `frame` holding a closure over itself inside a list, and the closure.
    fn closure_in_list(frame: &Env) -> Value {
        let f = closure_in(frame);
        declare(frame, "box".into(), Value::list(vec![f.clone()]), false);
        // The slot `f` is cleared, so only the list holds the closure.
        let _ = write(frame, 0, 0, Value::Null);
        f
    }

    #[test]
    fn a_closure_in_a_list_in_its_frame_is_freed_with_it() {
        let frame = frame();
        let weak = Rc::downgrade(&frame);
        drop(closure_in_list(&frame));
        leave(frame);
        assert!(weak.upgrade().is_none(), "the frame was freed");
    }

    #[test]
    fn a_closure_in_a_slot_and_a_list_is_freed_with_its_frame() {
        let frame = frame();
        let weak = Rc::downgrade(&frame);
        let f = closure_in(&frame);
        declare(&frame, "box".into(), Value::list(vec![f.clone()]), false);
        drop(f);
        leave(frame);
        assert!(weak.upgrade().is_none(), "the frame was freed");
    }

    #[test]
    fn a_returned_closure_frees_its_frame_once_dropped() {
        let frame = frame();
        let weak = Rc::downgrade(&frame);
        let returned = closure_in_list(&frame);
        leave(frame);
        assert!(weak.upgrade().is_some(), "the returned closure still reaches it");
        drop(returned);
        sweep();
        assert!(weak.upgrade().is_none(), "the next sweep freed it");
    }

    #[test]
    fn a_value_shared_with_others_doesnt_keep_the_frame() {
        // `inc` passed in is held outside too, but nothing outside reaches the frame through it.
        let outside = Value::list(vec![Value::Int(1)]);
        let frame = frame();
        let weak = Rc::downgrade(&frame);
        declare(&frame, "shared".into(), outside.clone(), false);
        drop(closure_in(&frame));
        leave(frame);
        assert!(weak.upgrade().is_none(), "the frame was freed");
        drop(outside);
    }

    #[test]
    fn a_list_held_outside_keeps_the_frame() {
        let frame = frame();
        let weak = Rc::downgrade(&frame);
        drop(closure_in_list(&frame));
        let outside = lookup(&frame, "box").unwrap();
        leave(frame);
        sweep();
        let kept = weak.upgrade().expect("the list reaches the closure, which reaches the frame");
        assert!(lookup(&kept, "box").is_some(), "its slots were not cleared");
        drop(kept);
        drop(outside);
        sweep();
        assert!(weak.upgrade().is_none());
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
