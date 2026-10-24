//! Reaching blocks: definition and action units, their sessions and `load` frames (INTERNALS
//! 6.2, 6.3, ch. 12).

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
use crate::syntax::ast::EngineBlock;
use crate::syntax::span::Span;

/// An export of a loaded definition unit: its name and its constant, copied at load (`None` for
/// a function) (12.2).
pub type Export = (Rc<str>, Option<Value>);

/// A definition unit's state for the run (12.1); an action run leaves nothing behind.
pub enum UnitState {
    Loading,
    /// Its exports, which import resolution reads (roadmap 1.7).
    #[allow(dead_code)]
    Loaded(Rc<[Export]>),
    Failed(Rc<ErrorValue>),
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
    pub fn exec_block(&mut self, b: &EngineBlock, env: &Env, want_value: bool) -> Exec<Value> {
        let definition = b.directives.exports.is_some();
        if definition {
            match self.units.get(&b.unit_id) {
                Some(UnitState::Loaded(_)) => return Ok(Value::Null),
                Some(UnitState::Failed(e)) => return Err(Flow::Error(e.clone())),
                Some(UnitState::Loading) => {
                    let msg = format!("{} is still loading (reached again during its own load)", self.block_name(b));
                    return Err(self.raise(error("ImportError", msg), b.tag.span));
                }
                None => {}
            }
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

    /// 12.1 steps 2-8 for one run of the unit.
    fn run_unit(&mut self, b: &EngineBlock, env: &Env, want_value: bool) -> Exec<(Value, Vec<Export>)> {
        let at = b.tag.span;
        let engine = self.registry.resolve_tag(&b.tag.text).map_err(|f| self.raise(f, at))?;
        let mut args = Vec::with_capacity(b.args.len());
        for (name, expr) in &b.args {
            args.push((name, self.eval(expr, env)?, expr.span));
        }
        if let Some(imp) = b.directives.imports.first() {
            return Err(self.not_yet("@import inside blocks (roadmap 1.7)", imp.span));
        }
        let s = self.session_for(&engine, at)?;
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
        let unit = format!("{}:{}", b.unit_id.file.0, b.unit_id.offset);
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
            imports: Vec::new(),
            exports,
            want_value,
        });
        // For crash and timeout messages, which add the place themselves (8.5, 8.6).
        let during = match &b.label {
            Some(label) => format!("block '{}'", label.text),
            None => format!("a @{} block", engine.spec.id),
        };
        let reply = self.request(s, frame, &during, at)?;
        // Only a definition's reply lists missing names (12.2); an action's are ignored.
        if let (Some(missing), Some(ex)) = (reply.missing.first(), b.directives.exports.as_ref()) {
            let span = ex.items.iter().find(|i| i.text == *missing).map_or(ex.at.unwrap_or(at), |i| i.span);
            let msg = format!("@export lists '{missing}' but {} does not define it", self.block_name(b));
            return Err(self.raise(error("ImportError", msg), span));
        }
        Ok((if want_value { reply.value } else { Value::Null }, reply.exports))
    }

    /// The session of `e`'s engine, started at its first use (8.2, 8.3); a ts block starts the
    /// js engine's. A session that failed to start stays, so each later use raises its error
    /// again (8.2).
    fn session_for(&mut self, e: &Rc<ResolvedEngine>, at: Span) -> Exec<SessionId> {
        let key = e.spec.session_of.clone().unwrap_or_else(|| e.spec.id.clone());
        if let Some(s) = self.sessions.iter().position(|s| s.key == key) {
            return Ok(s);
        }
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
        self.start_session(engine, &argv, &env, timeouts, at)
    }

    /// `[engines.<key>] startup_timeout` (default 30 s) and `call_timeout` (default none); 0 is
    /// none (18.2).
    fn timeouts(&self, key: &str) -> Timeouts {
        let cfg = self.registry.config().engines.get(key);
        let limit = |secs: f64| Duration::try_from_secs_f64(secs).ok().filter(|d| !d.is_zero());
        Timeouts {
            startup: limit(cfg.and_then(|c| c.startup_timeout.as_ref()).map_or(30.0, |t| t.value)),
            call: cfg.and_then(|c| c.call_timeout.as_ref()).and_then(|t| limit(t.value)),
        }
    }
}
