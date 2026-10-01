//! Statement and expression evaluation (INTERNALS 5.8, ch. 6). Roadmap 0.2: expression
//! statements, literals, builtin names, f-strings and calls.

use super::builtins::{self, Out, BUILTINS};
use super::error::{did_you_mean, ErrorValue};
use super::value::{float_str, to_str, Value};
use crate::syntax::ast::{Arg, EngineBlock, Expr, ExprKind, FPart, Stmt, StmtKind};
use crate::syntax::span::{SourceMap, Span};

pub struct Interp<'a> {
    pub sm: &'a SourceMap,
    pub out: Out,
}

impl<'a> Interp<'a> {
    pub fn new(sm: &'a SourceMap) -> Interp<'a> {
        Interp { sm, out: Out::new() }
    }

    pub fn run(&mut self, program: &[Stmt]) -> Result<(), ErrorValue> {
        for stmt in program {
            match &stmt.kind {
                StmtKind::Expr(e) => {
                    self.eval(e)?;
                }
                StmtKind::Block(b) => return Err(self.engine_unknown(b)),
                _ => return Err(self.not_yet(stmt.span)),
            }
        }
        Ok(())
    }

    /// Fill in the location and trace of an error the first time it is raised (5.9).
    fn raise(&self, mut e: ErrorValue, span: Span) -> ErrorValue {
        if e.span.is_none() {
            e.span = Some(span);
            let display = &self.sm.file(span.file).display;
            e.trace = vec![format!("{display}:{}", span.line)];
        }
        e
    }

    /// Temporary while roadmap 0.6/0.7 build the rest of the interpreter.
    fn not_yet(&self, span: Span) -> ErrorValue {
        self.raise(ErrorValue::new("Error", "the interpreter doesn't run this yet (roadmap 0.6/0.7)"), span)
    }

    /// Phase 0 has no engine registry, so reaching any block is `EngineUnknown` (INTERNALS 23).
    fn engine_unknown(&self, b: &EngineBlock) -> ErrorValue {
        let msg = format!("unknown engine '@{}' — this build has no engines yet", b.tag.text);
        self.raise(ErrorValue::new("EngineUnknown", msg), b.tag.span)
    }

    fn eval(&mut self, e: &Expr) -> Result<Value, ErrorValue> {
        Ok(match &e.kind {
            ExprKind::Null => Value::Null,
            ExprKind::Bool(b) => Value::Bool(*b),
            ExprKind::Int(n) => Value::Int(*n),
            ExprKind::Float(x) => Value::Float(*x),
            ExprKind::Str(s) => Value::Str(s.clone()),
            ExprKind::Bytes(b) => Value::Bytes(b.clone()),
            ExprKind::FStr(parts) => Value::Str(self.fstring(parts)?.into()),
            ExprKind::Name(n) => match builtins::lookup(n) {
                Some(def) => Value::Builtin(def),
                None => {
                    let mut msg = format!("'{n}' is not declared");
                    if let Some(s) = did_you_mean(n, BUILTINS.iter().map(|b| b.name)) {
                        msg.push_str(&format!(" — did you mean '{s}'?"));
                    }
                    return Err(self.raise(ErrorValue::new("NameError", msg), e.span));
                }
            },
            ExprKind::Block(b) => return Err(self.engine_unknown(b)),
            ExprKind::List(_)
            | ExprKind::Dict(_)
            | ExprKind::Unary(..)
            | ExprKind::Binary(..)
            | ExprKind::And(..)
            | ExprKind::Or(..)
            | ExprKind::Coalesce(..)
            | ExprKind::Ternary(..)
            | ExprKind::Index { .. }
            | ExprKind::Slice { .. }
            | ExprKind::Member { .. }
            | ExprKind::OptChain(_)
            | ExprKind::Lambda(_) => return Err(self.not_yet(e.span)),
            ExprKind::Call { callee, args, paren } => {
                let f = self.eval(callee)?;
                let mut pos = Vec::new();
                let mut named = Vec::new();
                for a in args {
                    match a {
                        Arg::Pos(x) => pos.push(self.eval(x)?),
                        Arg::Named(n, x) => named.push((n.clone(), self.eval(x)?)),
                        Arg::Spread(x) => return Err(self.not_yet(x.span)),
                    }
                }
                // The caret goes at the callee's name, else at the `(` (2.2).
                let at = if matches!(callee.kind, ExprKind::Name(_)) { callee.span } else { *paren };
                match f {
                    Value::Builtin(def) => (def.f)(self, pos, named).map_err(|err| self.raise(err, at))?,
                    other => {
                        let err = ErrorValue::new("TypeError", format!("'{}' is not callable", other.type_name()));
                        return Err(self.raise(err, at));
                    }
                }
            }
        })
    }

    fn fstring(&mut self, parts: &[FPart]) -> Result<String, ErrorValue> {
        let mut s = String::new();
        for p in parts {
            match p {
                FPart::Lit(text) => s.push_str(text),
                FPart::Expr { expr, fixed } => {
                    let v = self.eval(expr)?;
                    match fixed {
                        None => s.push_str(&to_str(&v)),
                        Some(n) => {
                            let n = *n as usize;
                            let text = match v {
                                Value::Int(i) => format!("{:.n$}", i as f64),
                                Value::Float(x) if x.is_finite() => format!("{x:.n$}"),
                                Value::Float(x) => float_str(x),
                                other => {
                                    let err = ErrorValue::new(
                                        "TypeError",
                                        format!("the :.{n}f format needs a number, got {}", other.type_name()),
                                    );
                                    return Err(self.raise(err, expr.span));
                                }
                            };
                            s.push_str(&text);
                        }
                    }
                }
            }
        }
        Ok(s)
    }
}
