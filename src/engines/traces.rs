//! Foreign positions to `.v2` positions (INTERNALS 2.3, 2.5, 12.6): the frames of a worker's
//! `err` as `e.trace` entries, and where the error points.

use std::path::Path;
use std::rc::Rc;

use serde_json::{Map, Value as Json};

use super::session::SessionId;
use crate::runtime::interp::Interp;
use crate::syntax::ast::EngineBlock;
use crate::syntax::span::{FileId, Span};

/// A worker error's frames, innermost first, and its innermost frame that maps into a block:
/// the position there and that block's tag (12.6).
pub struct Frames {
    pub frames: Vec<Rc<str>>,
    pub at: Option<(Span, Rc<str>)>,
}

/// The 1-based column of a line's first non-blank char (2.3).
fn first_text_col(line: &str) -> u32 {
    line.chars().position(|c| !c.is_whitespace()).map_or(1, |i| i as u32 + 1)
}

impl Interp {
    /// `@py risky`, or `@py block at main.v2:1` without a label (2.5).
    fn block_desc(&self, b: &EngineBlock) -> String {
        let tag = self.canonical_tag(b);
        match &b.label {
            Some(l) => format!("@{tag} {}", l.text),
            None => format!("@{tag} block at {}:{}", self.sm.file(b.span.file).display, b.span.line),
        }
    }

    fn canonical_tag(&self, b: &EngineBlock) -> Rc<str> {
        self.registry.canonical(&b.tag.text).map_or_else(|| b.tag.text.clone(), |s| s.id.clone())
    }

    /// The block of engine `key` whose body covers `line` of a file that has run; the first when
    /// one-line blocks share the line (2.3).
    fn block_at(&self, file: FileId, line: u32, key: &str) -> Option<&Rc<EngineBlock>> {
        let scope = self.files.get(&file)?;
        scope.bodies.iter().find(|b| {
            let first = b.loc.body_first_line;
            (first..first.saturating_add(b.loc.line_count.max(1))).contains(&line) && self.engine_key(&b.tag.text).as_deref() == Some(key)
        })
    }

    /// The frames of `e` (an `err.error`, 9.6) from session `s`: each `trace` entry printed as
    /// 2.5 says, or with no `trace` the lines of `text`.
    pub(crate) fn foreign_frames(&self, s: SessionId, e: &Map<String, Json>) -> Frames {
        let tag = self.sessions[s].key.clone();
        let mut out = Frames { frames: Vec::new(), at: None };
        let trace = e.get("trace").and_then(Json::as_array).filter(|t| !t.is_empty());
        let Some(trace) = trace else {
            if let Some(text) = e.get("text").and_then(Json::as_str) {
                out.frames = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(Rc::from).collect();
            }
            return out;
        };
        for f in trace {
            let (Some(file), Some(line)) = (f.get("file").and_then(Json::as_str), f.get("line").and_then(Json::as_u64)) else { continue };
            let line = u32::try_from(line).unwrap_or(u32::MAX);
            let func = f.get("func").and_then(Json::as_str);
            let fcol = f.get("col").and_then(Json::as_u64).and_then(|c| u32::try_from(c).ok()).filter(|&c| c > 0);
            let id = self.sm.id_of(Path::new(file));
            let (loc, mapped) = match id.and_then(|id| self.block_at(id, line, &tag).map(|b| (id, b))) {
                Some((id, b)) => {
                    let display = &self.sm.file(id).display;
                    (format!("{display}:{line}, in {}", self.block_desc(b)), Some((id, b)))
                }
                None => {
                    let shown = id.map_or_else(|| file.to_string(), |id| self.sm.file(id).display.to_string());
                    // A TypeScript file runs on the js engine, but is TypeScript (2.5).
                    let lower = file.to_ascii_lowercase();
                    let lang = if &*tag == "js" && [".ts", ".mts", ".cts"].iter().any(|e| lower.ends_with(e)) { "ts" } else { &*tag };
                    (format!("{shown}:{line}, in {lang}"), None)
                }
            };
            // V8 writes an anonymous function's frame as it writes the block's own top level,
            // so the same unnamed place twice in a row is printed once.
            if func.is_none() && out.frames.last().is_some_and(|l| **l == *loc) {
                continue;
            }
            out.frames.push(match func {
                Some(name) => format!("{name} ({loc})").into(),
                None => loc.into(),
            });
            if let (None, Some((id, b))) = (&out.at, mapped) {
                let text = self.sm.line_text(id, line);
                let col = fcol.and_then(|c| b.loc.col(line - b.loc.body_first_line + 1, c));
                let span = col.and_then(|c| self.sm.span_at(id, line, c)).or_else(|| self.sm.span_at(id, line, first_text_col(text)));
                if let Some(span) = span {
                    out.at = Some((span, self.canonical_tag(b)));
                }
            }
        }
        out
    }
}
