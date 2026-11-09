//! Reaching blocks: definition and action units, their sessions and `load` frames (INTERNALS
//! 6.2, 6.3, ch. 12).

use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Map, Value as Json};

use super::resolve::ResolvedEngine;
use super::session::{worker_command, SessionId, Timeouts};
use super::vep;
use crate::runtime::env::Env;
use crate::runtime::error::{ErrorValue, Exec, Flow};
use crate::runtime::interp::Interp;
use crate::runtime::value::Value;
use crate::syntax::ast::{EngineBlock, EngineImport, UnitId};
use crate::syntax::span::Span;

/// An export of a loaded unit (12.2, 13.2): a Proxy for a function, else the constant copied at
/// load.
#[derive(Clone)]
pub struct Export {
    pub name: Rc<str>,
    pub value: Value,
    pub func: bool,
}

/// A definition unit's state for the run (12.1); an action run leaves nothing behind but its
/// routes (13.6).
pub enum UnitState {
    /// In its language's line: reached, not run yet (12.7).
    Waiting,
    Loading,
    /// Its exports, which import resolution reads (13.3).
    Loaded(Rc<[Export]>),
    Failed(Rc<ErrorValue>),
}

/// What waits for its language: a definition block, or an `@import` with the blocks its statement
/// saw (12.7).
pub enum Waiting {
    Block(Rc<EngineBlock>),
    Import(Rc<EngineImport>, Rc<[UnitId]>),
}

/// One place in the line.
struct Place {
    seq: u64,
    key: Rc<str>,
    item: Waiting,
}

/// What waits, in program order, across languages (12.7). `running`: the place being run now.
#[derive(Default)]
pub struct Line {
    places: VecDeque<Place>,
    next: u64,
    running: Option<u64>,
    /// What needed the language, while a waiting block of the line loads: the V2 frames below
    /// its foreign ones are the need's (2.5, 12.7).
    need: Option<Span>,
}

/// `startup_timeout` when the config gives none, in seconds (DOCS 14).
pub const STARTUP_DEFAULT: f64 = 30.0;

impl Line {
    pub fn push(&mut self, key: Rc<str>, item: Waiting) {
        self.places.push_back(Place { seq: self.next, key, item });
        self.next += 1;
    }

    fn holds(&self, key: &str) -> bool {
        self.places.iter().any(|p| &*p.key == key)
    }
}

/// A block's wire unit id: `"<FileId>:<byte offset of @>"` (9.4).
pub fn wire_unit(id: UnitId) -> String {
    format!("{}:{}", id.file.0, id.offset)
}

fn error(kind: &str, message: String) -> Flow {
    Flow::Error(Rc::new(ErrorValue::new(kind, message)))
}

impl Interp {
    /// `block 'stats'`, or `@py block at main.v2:12` without a label; tags print canonical (12.1).
    pub fn block_name(&self, b: &EngineBlock) -> String {
        match &b.label {
            Some(label) => format!("block '{}'", label.text),
            None => {
                let tag = self.registry.canonical(&b.tag.text).map_or_else(|| b.tag.text.clone(), |s| s.id.clone());
                format!("@{tag} block at {}:{}", self.sm.file(b.span.file).display, b.span.line)
            }
        }
    }

    /// A block is reached (12.1): a definition block loads on its first reach, and later reaches
    /// do nothing or throw its stored load error; an action block runs on every reach.
    /// `want_value`: the block is in expression position (6.3).
    pub fn exec_block(&mut self, b: &Rc<EngineBlock>, env: &Env, want_value: bool) -> Exec<Value> {
        let definition = b.directives.exports.is_some();
        if definition {
            match self.units.get(&b.unit_id) {
                Some(UnitState::Loaded(_) | UnitState::Waiting) => return Ok(Value::Null),
                Some(UnitState::Failed(e)) => return Err(Flow::Error(e.clone())),
                Some(UnitState::Loading) => {
                    let msg = format!("{} is still loading (reached again during its own load)", self.block_name(b));
                    return Err(self.raise(error("ImportError", msg), b.tag.span));
                }
                None => {}
            }
            if let Some(key) = self.block_waits(b) {
                self.units.insert(b.unit_id, UnitState::Waiting);
                self.line.push(key, Waiting::Block(b.clone()));
                return Ok(Value::Null);
            }
        }
        // Before the unit counts as started, so an error from the line or the depth limit is not
        // stored as this block's (12.7).
        self.before_block(b)?;
        if definition {
            self.units.insert(b.unit_id, UnitState::Loading);
        }
        match self.run_unit(b, env, want_value && !definition) {
            Ok((value, exports)) => {
                if definition {
                    self.units.insert(b.unit_id, UnitState::Loaded(exports.into()));
                }
                Ok(value)
            }
            // Steps 2-7 fail the unit for good (12.1).
            Err(Flow::Error(e)) => {
                if definition {
                    self.units.insert(b.unit_id, UnitState::Failed(e.clone()));
                }
                Err(Flow::Error(e))
            }
            Err(other) => Err(other),
        }
    }

    /// Whether a session exists for `key`, a dead one included (12.7).
    pub(crate) fn running(&self, key: &str) -> bool {
        self.sessions.iter().any(|s| &*s.key == key)
    }

    /// The line `b` joins instead of running now (12.7).
    fn block_waits(&self, b: &EngineBlock) -> Option<Rc<str>> {
        if !self.files[&b.unit_id.file].block_may_wait(b.unit_id) {
            return None;
        }
        self.engine_key(&b.tag.text).filter(|key| !self.running(key))
    }

    /// A reached block is about to need its language: nothing starts at the depth limit, and what
    /// waits for the language runs first (12.7).
    fn before_block(&mut self, b: &EngineBlock) -> Exec<()> {
        let Some(key) = self.engine_key(&b.tag.text) else { return Ok(()) };
        if !self.running(&key) {
            self.check_depth(b.tag.span)?;
        }
        if self.line.holds(&key) {
            self.start_engine(&b.tag.text, b.tag.span)?;
        }
        Ok(())
    }

    /// Run the line (12.7): the first place, in program order, whose language is running, again and
    /// again. While a place runs, only places before it may go, because what comes after it comes
    /// after it in the program too. A failed place doesn't stop the rest; the first error is
    /// returned at the end. The depth limit belongs to the caller's chain, so each place starts
    /// from zero.
    fn run_line(&mut self, need: Span) -> Exec<()> {
        let mut outcome = Ok(());
        loop {
            let limit = self.line.running;
            let next = self.line.places.iter().position(|p| limit.is_none_or(|l| p.seq < l) && self.running(&p.key));
            let Some(place) = next.and_then(|i| self.line.places.remove(i)) else { return outcome };
            let outer = self.line.running.replace(place.seq);
            let depth = std::mem::replace(&mut self.xdepth, 0);
            let done = match &place.item {
                Waiting::Block(b) => {
                    self.units.remove(&b.unit_id);
                    let frame = self.files[&b.unit_id.file].frame.clone();
                    self.line.need = Some(need);
                    let done = self.exec_block(b, &frame, false).map(|_| ());
                    self.line.need = None;
                    done
                }
                Waiting::Import(imp, seen) => self.resolve_import(imp, Some(seen)).map(|_| ()),
            };
            self.line.running = outer;
            self.xdepth = depth;
            match done {
                Ok(()) => {}
                Err(Flow::Error(e)) => {
                    if outcome.is_ok() {
                        outcome = Err(Flow::Error(e));
                    }
                }
                Err(other) => return Err(other),
            }
        }
    }

    /// 12.1 steps 2-8 for one run of the unit.
    fn run_unit(&mut self, b: &Rc<EngineBlock>, env: &Env, want_value: bool) -> Exec<(Value, Vec<Export>)> {
        let at = b.tag.span;
        let need = self.line.need.take();
        let engine = self.registry.resolve_tag(&b.tag.text).map_err(|f| self.raise(f, at))?;
        let mut args = Vec::with_capacity(b.args.len());
        for (name, expr) in &b.args {
            args.push((name, self.eval(expr, env)?, expr.span));
        }
        let key = engine.spec.session_of.clone().unwrap_or_else(|| engine.spec.id.clone());
        let imports = self.inner_imports(b, &key)?;
        let s = self.session_for(&engine, at)?;
        self.check_alive(s, at)?;
        let mut encoded = Map::new();
        for (name, value, span) in args {
            match self.wire(s).encode(&value, &name.text) {
                Ok(j) => encoded.insert(name.text.to_string(), j),
                Err(flow) => {
                    // The frame is never sent: the args before this one give their counts back (10.1).
                    self.sessions[s].callables.release(&crate::codec::vep::fn_ids(&Json::Object(encoded)));
                    return Err(self.raise(flow, span));
                }
            };
        }
        let unit = wire_unit(b.unit_id);
        let file = self.sm.file(b.unit_id.file).path.to_string_lossy().into_owned();
        let exports: Vec<Json> = match &b.directives.exports {
            Some(ex) => ex.items.iter().map(|i| Json::from(&*i.text)).chain(ex.star.map(|_| Json::from("*"))).collect(),
            None => Vec::new(),
        };
        let frame = vep::load(vep::Load {
            unit: &unit,
            definition: b.directives.exports.is_some(),
            lang: &engine.spec.id,
            file: &file,
            line: b.loc.body_first_line,
            source: &b.body,
            args: encoded,
            imports,
            exports,
            want_value,
        });
        // For crash and timeout messages, which add the place themselves (8.5, 8.6).
        let during = match &b.label {
            Some(label) => format!("block '{}'", label.text),
            None => format!("a @{} block", engine.spec.id),
        };
        let reply = self.request(s, frame, &during, need.unwrap_or(at))?;
        // Only a definition's reply lists missing names (12.2); an action's are ignored.
        if let (Some(missing), Some(ex)) = (reply.missing.first(), b.directives.exports.as_ref()) {
            let span = ex.items.iter().find(|i| i.text == *missing).map_or(ex.at.unwrap_or(at), |i| i.span);
            let msg = format!("@export lists '{missing}' but {} does not define it", self.block_name(b));
            return Err(self.raise(error("ImportError", msg), span));
        }
        let unit: Rc<str> = unit.into();
        let exports = reply.exports.into_iter().map(|(name, v)| super::imports::export(s, &unit, name, v)).collect();
        Ok((if want_value { reply.value } else { Value::Null }, exports))
    }

    /// The session of `e`'s engine, started at its first use (8.2, 8.3); a ts block starts the
    /// js engine's. A session that failed to start stays, so each later use raises its error
    /// again (8.2).
    pub(crate) fn session_for(&mut self, e: &Rc<ResolvedEngine>, at: Span) -> Exec<SessionId> {
        let key = e.spec.session_of.clone().unwrap_or_else(|| e.spec.id.clone());
        let s = match self.sessions.iter().position(|s| s.key == key) {
            Some(s) => s,
            None => {
                // Before the adapter is written: a refused start must leave nothing behind.
                self.check_depth(at)?;
                let engine = match &e.spec.session_of {
                    Some(owner) => self.registry.resolve_tag(owner).map_err(|f| self.raise(f, at))?,
                    None => e.clone(),
                };
                let (file, text) = crate::adapters::for_session(&key).expect("a built-in bridged engine");
                let adapter = match crate::adapters::materialize(file, text) {
                    Ok(path) => path,
                    Err(msg) => return Err(self.raise(error("IOError", msg), at)),
                };
                let (argv, env) = worker_command(&engine, &adapter);
                let timeouts = self.timeouts(&key);
                match self.start_session(engine, &argv, &env, timeouts, at) {
                    Ok(s) => s,
                    Err(flow) => {
                        // A dead engine counts as running: what waited for it fails at this need,
                        // not at another language's (12.7). The start's error is the one raised.
                        let _ = self.run_line(at);
                        return Err(flow);
                    }
                }
            }
        };
        self.run_line(at).map_err(|f| self.raise(f, at))?;
        Ok(s)
    }

    /// The session of the engine that `tag` runs on, started if needed: its first need (12.7).
    pub(crate) fn start_engine(&mut self, tag: &str, at: Span) -> Exec<SessionId> {
        if let Some(s) = self.engine_key(tag).and_then(|key| self.sessions.iter().position(|s| s.key == key)) {
            self.run_line(at).map_err(|f| self.raise(f, at))?;
            return Ok(s);
        }
        let engine = self.registry.resolve_tag(tag).map_err(|f| self.raise(f, at))?;
        self.session_for(&engine, at)
    }

    /// `[engines.<key>] startup_timeout` (default 30 s) and `call_timeout` (default none); 0 is
    /// none (18.2).
    fn timeouts(&self, key: &str) -> Timeouts {
        let cfg = self.registry.config().engines.get(key);
        let limit = |secs: f64| Duration::try_from_secs_f64(secs).ok().filter(|d| !d.is_zero());
        Timeouts {
            startup: limit(cfg.and_then(|c| c.startup_timeout.as_ref()).map_or(STARTUP_DEFAULT, |t| t.value)),
            call: cfg.and_then(|c| c.call_timeout.as_ref()).and_then(|t| limit(t.value)),
        }
    }
}
