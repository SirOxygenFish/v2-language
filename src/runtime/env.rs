//! Frames and environments (INTERNALS 5.6).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use super::error::ErrorValue;
use super::value::{release, Callable, Dict, Func, List, Str, Value};
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
    /// Left while something else still held it, so in `Gc::remembered` (5.1).
    pub remembered: Cell<bool>,
    /// Waiting in `Gc::recheck`.
    queued: Cell<bool>,
    /// What its last check looked at, and when (in frames left): what a recheck must wait for.
    weight: Cell<u32>,
    checked: Cell<u32>,
    /// Frames left before it was made: a walk from a younger frame doesn't go into it.
    born: u32,
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
    /// Frames left so far: their age, and what pays for rechecks (5.1).
    static TICKS: Cell<u32> = const { Cell::new(0) };
    /// List items, dict entries and frame slots made so far: what starts a collection (5.1).
    static MADE: Cell<usize> = const { Cell::new(0) };
    /// The strings, bytes and handles counted toward the next collection, by address: each
    /// counts once, however many frames reach it.
    static COUNTED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    /// Slot arrays of frames that are gone: a malloc and a free less per call.
    static SPARE: RefCell<Vec<Vec<Option<Slot>>>> = const { RefCell::new(Vec::new()) };
    /// Frames that were left with nothing else holding them, emptied, for `new_frame` to reuse:
    /// a call or a loop iteration makes no allocation (5.6).
    static POOL: RefCell<Vec<Env>> = const { RefCell::new(Vec::new()) };
}

/// How many spare arrays to keep: about the depth of a program's usual recursion.
const MAX_SPARE: usize = 256;

/// `n` more values made in lists, dicts or frames.
#[inline]
pub fn made(n: usize) {
    let _ = MADE.try_with(|m| m.set(m.get().wrapping_add(n)));
}

pub fn new_frame(layout: &Layout, parent: Option<&Env>) -> Env {
    made(layout.len() + 1);
    if let Some(mut env) = POOL.with(|p| p.borrow_mut().pop()) {
        let born = TICKS.with(Cell::get);
        let f = Rc::get_mut(&mut env).expect("a pooled frame has no other holder");
        if !Rc::ptr_eq(f.layout.get_mut(), layout) {
            *f.layout.get_mut() = layout.clone();
        }
        let slots = f.slots.get_mut();
        if slots.len() != layout.len() {
            slots.resize_with(layout.len(), || None);
        }
        f.parent = parent.cloned();
        f.born = born;
        f.weight.set(0);
        f.checked.set(0);
        return env;
    }
    let mut slots = SPARE.with(|s| s.borrow_mut().pop()).unwrap_or_default();
    slots.resize_with(layout.len(), || None);
    Rc::new(Frame {
        layout: RefCell::new(layout.clone()),
        slots: RefCell::new(slots),
        parent: parent.cloned(),
        closures: Cell::new(0),
        remembered: Cell::new(false),
        queued: Cell::new(false),
        weight: Cell::new(0),
        checked: Cell::new(0),
        born: TICKS.with(Cell::get),
    })
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

/// The int at a position, without a copy of the value: `None` if it isn't declared yet or holds
/// something else.
#[inline]
pub fn read_int(env: &Env, hops: u32, index: u32) -> Option<i64> {
    match frame_at(env, hops).slots.borrow()[index as usize] {
        Some(Slot { value: Value::Int(n), .. }) => Some(n),
        _ => None,
    }
}

/// `x = f(x)` for the int `x` at a position, in place: whether it was done (the slot holds an
/// int, isn't a constant, and `f` gave one). Otherwise nothing changed.
#[inline]
pub fn update_int(env: &Env, hops: u32, index: u32, f: impl FnOnce(i64) -> Option<i64>) -> bool {
    match &mut frame_at(env, hops).slots.borrow_mut()[index as usize] {
        Some(Slot { value: Value::Int(n), is_const: false }) => match f(*n) {
            Some(r) => {
                *n = r;
                true
            }
            None => false,
        },
        _ => false,
    }
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

/// `s += piece` on the variable at a position, in place: see `grow`. `old` comes back when the
/// slot is a constant or empty, for the usual path and its errors.
pub fn append_at(env: &Env, hops: u32, index: u32, old: Str, piece: &str) -> Result<(), Str> {
    match &mut frame_at(env, hops).slots.borrow_mut()[index as usize] {
        Some(slot) if !slot.is_const => grow(&mut slot.value, old, piece),
        _ => Err(old),
    }
}

/// `x += piece` where `x` was `old` before `piece` was evaluated (5.3): when `target` still holds
/// that string, `old` goes first, so the string grows in place if `target` was its only other
/// holder. Otherwise `old` comes back.
pub fn grow(target: &mut Value, old: Str, piece: &str) -> Result<(), Str> {
    match target {
        Value::Str(s) if Str::ptr_eq(s, &old) => {
            drop(old);
            s.append(piece);
            Ok(())
        }
        _ => Err(old),
    }
}

/// Whether `f`'s scope declares `name`, by now or later (a `let` that hasn't run yet, 5.9).
pub fn scope_declares(f: &Frame, name: &str) -> bool {
    f.layout.borrow().iter().any(|n| &**n == name)
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

/// Items a check at a scope exit, or a recheck, looks at before it leaves the frame to the next
/// collection; values it holds at most (5.1).
const CHECK_BUDGET: usize = if cfg!(test) { 1_000 } else { 10_000_000 };
const MAX_NODES: usize = if cfg!(test) { 1_000 } else { 1_000_000 };
/// A remembered frame is checked again only once the frames left since its last check, times
/// this, reach what that check looked at: a big group costs each call a few items, not itself.
const RECHECK_RATE: usize = 8;
/// Values the program must make before the first collection.
const FIRST_COLLECTION: usize = 100_000;
/// A remembered frame whose last check looked at more is not walked again from another
/// frame's check: rechecks and collections see to it (5.1).
const SMALL_GROUP: u32 = 64;

/// The cycle collector of 5.1.
struct Gc {
    /// Frames left while something else still held them.
    remembered: Vec<Weak<Frame>>,
    /// How many of those were alive at the last collection or tidying.
    kept: usize,
    /// Remembered frames that a closure over them, or below them, has let go of since.
    recheck: Vec<Weak<Frame>>,
    /// `MADE` at the last collection, and how much more starts the next.
    since: usize,
    due: usize,
}

thread_local! {
    static GC: RefCell<Gc> = const {
        RefCell::new(Gc { remembered: Vec::new(), kept: 0, recheck: Vec::new(), since: 0, due: FIRST_COLLECTION })
    };
    /// A check is dropping the copies it held, which are no one letting go.
    static CHECKING: Cell<bool> = const { Cell::new(false) };
    /// The collector has had something to recheck or keep since the run began: until then,
    /// leaving a frame needn't look at its lists.
    static GC_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// A closure over `env` is going and leaves at most one holder: each remembered frame from `env`
/// up may be garbage now, so it is checked again at the next scope exit.
pub fn closure_dropped(env: &Env) {
    if CHECKING.try_with(Cell::get).unwrap_or(true) {
        return;
    }
    let mut frame = Some(env);
    while let Some(f) = frame {
        if f.remembered.get() && !f.queued.get() {
            let queued = GC.try_with(|g| g.try_borrow_mut().map(|mut g| g.recheck.push(Rc::downgrade(f))).is_ok());
            let _ = GC_ACTIVE.try_with(|a| a.set(true));
            f.queued.set(queued.unwrap_or(false));
        }
        frame = f.parent.as_ref();
    }
}

/// Called when `env` goes out of scope. A closure over the frame that the frame itself holds,
/// in a slot or in a list or dict in one, makes an `Rc` cycle. If only that cycle holds the
/// frame now, its slots are cleared so it goes; if something else still does (a closure that
/// was returned), it is remembered, for rechecks and collections (5.1).
pub fn leave(env: Env) {
    let ticks = TICKS.with(|t| {
        let n = t.get().wrapping_add(1);
        t.set(n);
        n
    });
    let (pending, due) = if GC_ACTIVE.with(Cell::get) {
        let so_far = MADE.with(Cell::get);
        let Ok(state) = GC.try_with(|g| {
            let g = g.borrow();
            (!g.recheck.is_empty(), !g.remembered.is_empty() && so_far.wrapping_sub(g.since) >= g.due)
        }) else {
            return;
        };
        state
    } else {
        (false, false)
    };
    if pending {
        recheck(ticks);
    }
    // Nothing else holds it, so no closure does: it goes when `env` does.
    if env.closures.get() == 0 || Rc::strong_count(&env) == 1 {
        pool(env);
        if due {
            collect();
        }
        return;
    }
    let weight = match walk(std::slice::from_ref(&env), true, CHECK_BUDGET, MAX_NODES) {
        Some(found) if !found.dead.is_empty() => return free(found.frames),
        Some(found) => {
            // What it holds now waits for a collection, as if made now: strings, bytes and
            // handles too, which `made` doesn't see.
            made(found.held);
            found.work
        }
        None => CHECK_BUDGET,
    };
    env.weight.set(u32::try_from(weight).unwrap_or(u32::MAX));
    env.checked.set(ticks);
    env.remembered.set(true);
    let _ = GC_ACTIVE.try_with(|a| a.set(true));
    let _ = GC.try_with(|g| {
        let mut g = g.borrow_mut();
        g.remembered.push(Rc::downgrade(&env));
        // A dead entry keeps its frame's memory: tidy once they could outnumber the rest.
        if g.remembered.len() >= 2 * g.kept.max(512) {
            g.remembered.retain(|w| w.strong_count() > 0);
            g.kept = g.remembered.len();
        }
    });
    drop(env);
    if due {
        collect();
    }
}

/// A frame being left: kept emptied for `new_frame` when nothing else holds it or can find it (no
/// weak handle: the collector's lists), no closure was ever made over it, and the collector isn't
/// keeping it; else dropped. Its values go first, then its parent, as when it is dropped.
fn pool(mut env: Env) {
    if env.closures.get() != 0 || env.remembered.get() || env.queued.get() || Rc::weak_count(&env) != 0 {
        return;
    }
    let Some(f) = Rc::get_mut(&mut env) else { return };
    // Emptied in place: the next frame of the same layout needn't size the array again.
    for slot in f.slots.get_mut().iter_mut() {
        *slot = None;
    }
    f.parent = None;
    let _ = POOL.try_with(|p| {
        let mut pool = p.borrow_mut();
        if pool.len() < MAX_SPARE {
            pool.push(env);
        }
    });
}

/// A frame that a loop's iterations share (`Body.reusable`), after one: its values go, as a fresh
/// frame's would on leaving it, and the next iteration starts with nothing declared (5.6). Each
/// value goes outside the borrow, so its drop may look at frames.
pub fn reset(env: &Env) {
    let n = env.slots.borrow().len();
    for i in 0..n {
        let old = env.slots.borrow_mut()[i].take();
        drop(old);
    }
}

/// What leaving a frame does for the collector, for an iteration that shares its frame: the
/// count of frames left that paces it, its rechecks and its collections (5.1).
pub fn tick() {
    let ticks = TICKS.with(|t| {
        let n = t.get().wrapping_add(1);
        t.set(n);
        n
    });
    if !GC_ACTIVE.with(Cell::get) {
        return;
    }
    let so_far = MADE.with(Cell::get);
    let Ok((pending, due)) = GC.try_with(|g| {
        let g = g.borrow();
        (!g.recheck.is_empty(), !g.remembered.is_empty() && so_far.wrapping_sub(g.since) >= g.due)
    }) else {
        return;
    };
    if pending {
        recheck(ticks);
    }
    if due {
        collect();
    }
}

/// Check the remembered frames a closure has let go of, each once the frames left since its
/// last check pay for it; the rest wait for a collection.
fn recheck(ticks: u32) {
    let Ok(frames) = GC.try_with(|g| std::mem::take(&mut g.borrow_mut().recheck)) else { return };
    // Outermost first: freeing it frees what is below it too.
    for env in frames.iter().rev().filter_map(Weak::upgrade) {
        env.queued.set(false);
        let paid = (ticks.wrapping_sub(env.checked.get()) as usize + 1).saturating_mul(RECHECK_RATE);
        if !env.remembered.get() || paid < env.weight.get() as usize {
            continue;
        }
        match walk(std::slice::from_ref(&env), true, CHECK_BUDGET, MAX_NODES) {
            Some(found) if !found.dead.is_empty() => free(found.frames),
            found => {
                let weight = found.map_or(CHECK_BUDGET, |f| f.work);
                env.weight.set(u32::try_from(weight).unwrap_or(u32::MAX));
                env.checked.set(ticks);
            }
        }
    }
}

/// Check every remembered frame together, with no budget: this frees a cycle through several of
/// them, one made after its frame was left, one a dropped list or dict let go of, and one too big
/// for a check at a scope exit. The next comes once the program has made as many values as this
/// one found alive, so the work stays in proportion to the program's and garbage to what lives.
pub fn collect() {
    let Ok(weak) = GC.try_with(|g| std::mem::take(&mut g.borrow_mut().remembered)) else { return };
    let seeds: Vec<Env> = weak.iter().filter_map(Weak::upgrade).filter(|f| f.remembered.get()).collect();
    drop(weak);
    let found = walk(&seeds, false, usize::MAX, usize::MAX);
    let mut dead = vec![false; seeds.len()];
    for &k in found.as_ref().map_or(&[][..], |f| &f.dead) {
        dead[k] = true;
    }
    let kept: Vec<Weak<Frame>> = seeds.iter().zip(dead).filter(|(_, d)| !d).map(|(f, _)| Rc::downgrade(f)).collect();
    let live = found.as_ref().map_or(0, |f| f.live);
    let _ = GC.try_with(|g| {
        let mut g = g.borrow_mut();
        g.remembered.extend(kept);
        g.kept = g.remembered.len();
        g.since = MADE.with(Cell::get);
        COUNTED.with(|c| c.borrow_mut().clear());
        g.due = FIRST_COLLECTION.max(live);
    });
    drop(seeds);
    if let Some(found) = found {
        free(found.frames);
    }
}

/// Clear frames nothing outside reaches, so their cycles go.
fn free(frames: Vec<Env>) {
    let mut values = Vec::new();
    for f in &frames {
        f.remembered.set(false);
        values.extend(take_values(f));
    }
    drop(frames);
    release(values);
}

/// Set while a check runs (`CHECKING`).
struct Checking;

impl Checking {
    fn new() -> Checking {
        CHECKING.with(|c| c.set(true));
        Checking
    }
}

impl Drop for Checking {
    fn drop(&mut self) {
        let _ = CHECKING.try_with(|c| c.set(false));
    }
}

/// A frame, list, dict, function or error that a walk reached, held once here.
#[derive(Clone)]
enum Node {
    Frame(Env),
    List(List),
    Dict(Dict),
    Func(Func),
    Error(Rc<ErrorValue>),
}

impl Node {
    fn count(&self) -> usize {
        match self {
            Node::Frame(f) => Rc::strong_count(f),
            Node::List(l) => l.strong_count(),
            Node::Dict(d) => d.strong_count(),
            Node::Func(f) => f.strong_count(),
            Node::Error(e) => Rc::strong_count(e),
        }
    }

    /// Calls `f` on each reference it holds while `f` returns true. One being changed right now
    /// is taken to hold nothing, which only makes more look held.
    fn each(&self, f: &mut dyn FnMut(Ref<'_>) -> bool) -> bool {
        match self {
            Node::Frame(env) => {
                if let Ok(slots) = env.slots.try_borrow() {
                    for slot in slots.iter().flatten() {
                        if !f(Ref::Value(&slot.value)) {
                            return false;
                        }
                    }
                }
                env.parent.as_ref().is_none_or(|p| f(Ref::Frame(p)))
            }
            Node::List(l) => l.try_borrow().map_or(true, |items| items.iter().all(|v| f(Ref::Value(v)))),
            Node::Dict(d) => d.try_borrow().map_or(true, |map| map.values().all(|v| f(Ref::Value(v)))),
            Node::Func(func) => match &**func {
                Callable::Closure { env, .. } => f(Ref::Frame(env)),
                Callable::Waiting { resolved, .. } => resolved.get().is_none_or(|v| f(Ref::Value(v))),
                _ => true,
            },
            Node::Error(e) => f(Ref::Value(&e.data)) && f(Ref::Value(&e.cause)),
        }
    }
}

/// A reference a walk follows.
enum Ref<'a> {
    Value(&'a Value),
    Frame(&'a Env),
}

impl Ref<'_> {
    /// The address of what it points at and that value's holders; `None` for a value that holds
    /// nothing a walk follows (an int, a string, a handle).
    fn target(&self) -> Option<(usize, usize)> {
        Some(match self {
            Ref::Frame(f) => (Rc::as_ptr(f) as usize, Rc::strong_count(f)),
            Ref::Value(Value::List(l)) => (l.addr(), l.strong_count()),
            Ref::Value(Value::Dict(d)) => (d.addr(), d.strong_count()),
            Ref::Value(Value::Func(f)) => (f.addr(), f.strong_count()),
            Ref::Value(Value::Error(e)) => (Rc::as_ptr(e) as usize, Rc::strong_count(e)),
            Ref::Value(_) => return None,
        })
    }

    fn node(&self) -> Option<Node> {
        Some(match self {
            Ref::Frame(f) => Node::Frame(Rc::clone(f)),
            Ref::Value(Value::List(l)) => Node::List(l.clone()),
            Ref::Value(Value::Dict(d)) => Node::Dict(d.clone()),
            Ref::Value(Value::Func(f)) => Node::Func(f.clone()),
            Ref::Value(Value::Error(e)) => Node::Error(e.clone()),
            Ref::Value(_) => return None,
        })
    }
}

/// A value a walk reached: the value, held once here, its address, its references from walked
/// values, where its own references start and end in the walk's list of them, how many items
/// it has and what its strings, bytes and handles weigh, whether it is walked, and whether it
/// may be.
struct Entry {
    node: Node,
    addr: usize,
    refs: usize,
    edges: (usize, usize),
    items: usize,
    held: usize,
    walked: bool,
    closed: bool,
}

/// What a walk found.
struct Found {
    /// The seeds nothing outside reaches, by position.
    dead: Vec<usize>,
    /// Every frame nothing outside reaches, the seeds among them.
    frames: Vec<Env>,
    /// Items looked at, and those of them in values that live, with what their strings, bytes and
    /// handles weigh.
    work: usize,
    live: usize,
    /// What the strings, bytes and handles looked at weigh, in items (5.1).
    held: usize,
}

/// A handle's weight in items: its object, in another process, could be big.
const HANDLE_WEIGHT: usize = 1_000;

/// What a value that holds nothing a walk follows weighs, in items of 16 bytes, and its address.
/// Who else holds it doesn't matter: a value shared when its frame is left may be the garbage's
/// alone later.
fn weight(v: &Value) -> Option<(usize, usize)> {
    match v {
        Value::Str(s) if s.len() >= 16 => Some((s.addr(), s.len() / 16)),
        Value::Bytes(b) if b.len() >= 16 => Some((Rc::as_ptr(b) as *const u8 as usize, b.len() / 16)),
        Value::Handle(h) => Some((Rc::as_ptr(h) as usize, HANDLE_WEIGHT)),
        _ => None,
    }
}

/// Whether `f` is a frame below `seed`'s: its own nested calls and bodies.
fn below(f: &Frame, seed: usize) -> bool {
    let mut p = f.parent.as_ref();
    while let Some(frame) = p {
        if Rc::as_ptr(frame) as usize == seed {
            return true;
        }
        p = frame.parent.as_ref();
    }
    false
}

/// A walk's lists, kept for the next one so a small walk allocates nothing.
#[derive(Default)]
struct Scratch {
    nodes: Vec<Entry>,
    edges: Vec<usize>,
    todo: Vec<usize>,
    live: Vec<bool>,
}

thread_local! {
    static SCRATCH: Cell<Option<Scratch>> = const { Cell::new(None) };
}

/// Whether anything outside the values reached from `seeds` still reaches each of them. Every
/// frame, list, dict, function and error reached is counted with its references from the walked
/// ones; one with more holders than that is held from outside, so it lives, with all it reaches
/// (followed along each value's own references, so linear). A reference the walk misses only
/// makes more look held, so a wrong answer can keep memory but never frees what is in use.
/// `lazy`: a value is walked only once all its holders are found among walked ones, so what the
/// rest of the program shares (data passed in, a running frame, another call's function) costs
/// nothing. Nor is a frame older than the seeds, or one checked before whose group was big: it was
/// checked when it was left, and a chain of closures, each over the frame before (built by a loop
/// or by recursion), would make every walk go back to the first one. `None`: more than `budget`
/// items or `max_nodes` values.
fn walk(seeds: &[Env], lazy: bool, mut budget: usize, max_nodes: usize) -> Option<Found> {
    // Declared first, so it ends last: the copies below are dropped while it is set.
    let _checking = Checking::new();
    let mut scratch = SCRATCH.try_with(Cell::take).ok().flatten().unwrap_or_default();
    let Scratch { nodes, edges, todo, live } = &mut scratch;
    let n = seeds.len();
    nodes.extend(seeds.iter().map(|s| Entry { node: Node::Frame(s.clone()), addr: Rc::as_ptr(s) as usize, refs: 0, edges: (0, 0), items: 0, held: 0, walked: true, closed: false }));
    let youngest = seeds.iter().map(|s| s.born).max().unwrap_or(0);
    let checked = seeds.first().map_or(0, |s| Rc::as_ptr(s) as usize);
    let mut seen = HashSet::new();
    // Found by address in that list while it is short, then through a map.
    let index = |nodes: &[Entry]| nodes.iter().enumerate().map(|(k, e)| (e.addr, k)).collect::<HashMap<usize, usize>>();
    let mut by_addr = (n >= 32).then(|| index(nodes));
    todo.extend((0..n).rev());
    // Holders that aren't references from walked values: this list's copy, a seed's owner.
    let ours = |k: usize| 1 + usize::from(k < n);
    while let Some(k) = todo.pop() {
        // Held once more while it is walked.
        let node = nodes[k].node.clone();
        let first = edges.len();
        let mut items = 0;
        let mut held = 0;
        let mut over = false;
        node.each(&mut |r| {
            if budget == 0 {
                over = true;
                return false;
            }
            budget -= 1;
            items += 1;
            let Some((addr, count)) = r.target() else {
                if let Ref::Value(v) = r {
                    if let Some((at, w)) = weight(v) {
                        // Once per walk, and in a check at a scope exit once until the next
                        // collection: a value many frames reach counts once.
                        if seen.insert(at) && (!lazy || COUNTED.with(|c| c.borrow_mut().insert(at))) {
                            held += w;
                        }
                    }
                }
                return true;
            };
            let found = match &by_addr {
                Some(map) => map.get(&addr).copied(),
                None => nodes.iter().position(|e| e.addr == addr),
            };
            let j = match found {
                Some(j) => {
                    let e = &mut nodes[j];
                    e.refs += 1;
                    // Every holder found: walk it too.
                    if !e.walked && !e.closed && e.refs + ours(j) + usize::from(j == k) == count {
                        e.walked = true;
                        todo.push(j);
                    }
                    j
                }
                None => {
                    let Some(node) = r.node() else { return true };
                    let j = nodes.len();
                    // Made before the frame checked (wrapping like the ticks), or checked before
                    // with a big group and not the checked frame's own nested call: a chain built
                    // by recursion would be walked at every level.
                    let closed = lazy
                        && matches!(&node, Node::Frame(f) if (youngest.wrapping_sub(f.born) as i32) > 0
                            || (f.remembered.get() && f.weight.get() > SMALL_GROUP && !below(f, checked)));
                    // `count` has no copy of ours yet.
                    let walked = !lazy || (count == 1 && !closed);
                    nodes.push(Entry { node, addr, refs: 1, edges: (0, 0), items: 0, held: 0, walked, closed });
                    match &mut by_addr {
                        Some(map) => {
                            map.insert(addr, j);
                        }
                        None if j >= 32 => by_addr = Some(index(nodes)),
                        None => {}
                    }
                    if walked {
                        todo.push(j);
                    }
                    j
                }
            };
            edges.push(j);
            true
        });
        if over || nodes.len() > max_nodes {
            return None;
        }
        nodes[k].edges = (first, edges.len());
        nodes[k].items = items + 1;
        nodes[k].held = held;
    }
    // What something outside holds lives, with all it reaches.
    live.resize(nodes.len(), false);
    todo.extend((0..nodes.len()).filter(|&k| nodes[k].refs + ours(k) < nodes[k].node.count()));
    while let Some(k) = todo.pop() {
        if !std::mem::replace(&mut live[k], true) {
            let (a, b) = nodes[k].edges;
            todo.extend_from_slice(&edges[a..b]);
        }
    }
    let mut found = Found { dead: Vec::new(), frames: Vec::new(), work: 0, live: 0, held: 0 };
    for (k, (e, &l)) in nodes.iter().zip(live.iter()).enumerate() {
        found.work += e.items;
        found.held += e.held;
        if l {
            found.live += e.items + e.held;
            continue;
        }
        if k < n {
            found.dead.push(k);
        }
        if let Node::Frame(f) = &e.node {
            found.frames.push(f.clone());
        }
    }
    nodes.clear();
    edges.clear();
    live.clear();
    // A big walk's lists go, not kept for the rest of the run.
    if nodes.capacity() <= 4096 && edges.capacity() <= 16384 {
        let _ = SCRATCH.try_with(|s| s.set(Some(scratch)));
    }
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::{ExprKind, StmtKind};
    use crate::syntax::parser::parse_str;

    /// A closure over `frame`.
    fn closure_over(frame: &Env) -> Value {
        let prog = parse_str("let f = func() => 1").unwrap();
        let StmtKind::Let(_, Some(e)) = &prog[0].kind else { panic!("let") };
        let ExprKind::Func(decl) = &e.kind else { panic!("func") };
        frame.closures.set(frame.closures.get() + 1);
        Value::func(Callable::Closure { decl: decl.clone(), env: frame.clone(), name: "f".into() })
    }

    /// A closure over `frame`, stored in `frame` (the cycle of a local `func`).
    fn closure_in(frame: &Env) -> Value {
        let f = closure_over(frame);
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
        collect();
        assert!(weak.upgrade().is_none(), "the next collection freed it");
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
        collect();
        let kept = weak.upgrade().expect("the list reaches the closure, which reaches the frame");
        assert!(lookup(&kept, "box").is_some(), "its slots were not cleared");
        drop(kept);
        drop(outside);
        collect();
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn a_cycle_made_after_leaving_is_freed() {
        // No cycle when it is left, but the frame is remembered: the closure that escaped can
        // still store itself there.
        let frame = frame();
        let weak = Rc::downgrade(&frame);
        let escaped = closure_over(&frame);
        leave(frame);
        let kept = weak.upgrade().expect("the closure still reaches its frame");
        declare(&kept, "me".into(), escaped.clone(), false);
        drop(kept);
        drop(escaped);
        leave(self::frame());
        assert!(weak.upgrade().is_none(), "the recheck at the next scope exit freed it");
    }

    #[test]
    fn a_group_too_big_to_check_is_collected() {
        let frame = frame();
        let weak = Rc::downgrade(&frame);
        drop(closure_in(&frame));
        declare(&frame, "big".into(), Value::list(vec![Value::Int(0); CHECK_BUDGET]), false);
        leave(frame);
        assert!(weak.upgrade().is_some(), "too big to check when it was left");
        collect();
        assert!(weak.upgrade().is_none(), "a collection has no budget");
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
