//! Error values, control flow, did-you-mean and rendering (INTERNALS 2.4, 2.5, 5.8, 5.9).

use std::cell::OnceCell;
use std::rc::Rc;

use super::value::Value;
use crate::syntax::span::{SourceMap, Span};
use crate::syntax::Diag;

/// An error value (5.1, 5.9). `span` and `trace` are set by the first raise, so a rethrow keeps
/// them. `lang` and `foreign_type` are `None` for V2 errors; engines fill them (Phase 1).
#[derive(Debug)]
pub struct ErrorValue {
    /// The `type` field: `ValueError`, `KeyError`, …
    pub kind: Rc<str>,
    pub message: Rc<str>,
    pub data: Value,
    pub lang: Option<Rc<str>>,
    pub foreign_type: Option<Rc<str>>,
    pub span: OnceCell<Span>,
    /// Frame strings, innermost first (2.5).
    pub trace: OnceCell<Rc<[Rc<str>]>>,
    /// The error this one was raised from (a foreign exception's cause, 9.6), or `null`.
    pub cause: Value,
}

/// An error's `data` and `cause` are freed through the same queue as containers (5.1), so a
/// chain of errors (`e = error("E", "m", e)`, a million times) can't overflow the stack.
impl Drop for ErrorValue {
    fn drop(&mut self) {
        let data = std::mem::replace(&mut self.data, Value::Null);
        let cause = std::mem::replace(&mut self.cause, Value::Null);
        super::value::release(vec![data, cause]);
    }
}

impl ErrorValue {
    pub fn new(kind: &str, message: impl Into<Rc<str>>) -> ErrorValue {
        ErrorValue::with_data(kind.into(), message.into(), Value::Null)
    }

    pub fn with_data(kind: Rc<str>, message: Rc<str>, data: Value) -> ErrorValue {
        ErrorValue {
            kind,
            message,
            data,
            lang: None,
            foreign_type: None,
            span: OnceCell::new(),
            trace: OnceCell::new(),
            cause: Value::Null,
        }
    }
}

/// How a statement or expression ended other than normally (5.8).
#[derive(Debug)]
pub enum Flow {
    Error(Rc<ErrorValue>),
    Return(Value),
    Break,
    Continue,
    /// `exit(code)`: caught by nothing, and no `finally` runs (5.8).
    Exit(i32),
}

pub type Exec<T> = Result<T, Flow>;

/// An unraised error as a `Flow`; the interpreter adds the location (5.9).
pub fn fail<T>(kind: &str, message: impl Into<Rc<str>>) -> Exec<T> {
    Err(Flow::Error(Rc::new(ErrorValue::new(kind, message))))
}

pub fn type_error<T>(message: impl Into<Rc<str>>) -> Exec<T> {
    fail("TypeError", message)
}

pub fn value_error<T>(message: impl Into<Rc<str>>) -> Exec<T> {
    fail("ValueError", message)
}

/// `message — did you mean 'x'?` when a candidate is close enough (5.9).
pub fn with_suggestion<'a>(message: String, name: &str, candidates: impl IntoIterator<Item = &'a str>) -> String {
    match did_you_mean(name, candidates) {
        Some(s) => format!("{message} — did you mean '{s}'?"),
        None => message,
    }
}

// ---------------------------------------------------------------------------------------------
// Did-you-mean (5.9)

/// Optimal string alignment distance.
fn osa(a: &[char], b: &[char]) -> usize {
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

/// The closest candidate, compared lower-cased, if within `max(1, min(3, (len + 2) / 3))`.
pub fn did_you_mean<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let target: Vec<char> = name.to_lowercase().chars().collect();
    let limit = 1usize.max(3usize.min(target.len().div_ceil(3)));
    let mut best: Option<(usize, &str)> = None;
    for c in candidates {
        if c == name {
            continue;
        }
        let d = osa(&target, &c.to_lowercase().chars().collect::<Vec<_>>());
        if d <= limit && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| c)
}

// ---------------------------------------------------------------------------------------------
// Rendering (2.4)

/// The source line under `span` with a caret, indented 4. Leading whitespace is removed, tabs
/// print as one space, and lines over 120 chars are cut to a window around the caret.
fn excerpt(sm: &SourceMap, span: Span) -> String {
    let line = sm.line_text(span.file, span.line);
    let removed = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
    let chars: Vec<char> = line.chars().skip(removed).map(|c| if c == '\t' { ' ' } else { c }).collect();
    let mut caret = (span.col as usize).saturating_sub(1 + removed);
    let mut text: String = chars.iter().collect();
    if chars.len() > 120 {
        let start = caret.saturating_sub(60).min(chars.len() - 120);
        let end = start + 120;
        let mut s = String::new();
        if start > 0 {
            s.push_str("...");
        }
        s.extend(&chars[start..end]);
        if end < chars.len() {
            s.push_str("...");
        }
        caret = caret - start + if start > 0 { 3 } else { 0 };
        text = s;
    }
    format!("    {text}\n    {}^\n", " ".repeat(caret))
}

fn split_message(message: &str) -> (&str, Vec<&str>) {
    let mut lines = message.lines();
    let first = lines.next().unwrap_or("");
    (first, lines.collect())
}

/// `ValueError`, or `ForeignError: ZeroDivisionError` when the foreign class is known (2.4).
pub fn kind_text(e: &ErrorValue) -> String {
    match &e.foreign_type {
        Some(ft) => format!("{}: {ft}", e.kind),
        None => e.kind.to_string(),
    }
}

/// Most `caused by:` levels printed (2.4).
const MAX_CAUSES: usize = 8;

/// An uncaught error, as printed to stderr (2.4).
pub fn render_error(sm: &SourceMap, e: &ErrorValue) -> String {
    let (first, rest) = split_message(&e.message);
    let mut out = match e.span.get().copied() {
        Some(span) => {
            let display = &sm.file(span.file).display;
            let mut s = format!("{display}:{}:{}: {}: {first}\n", span.line, span.col, kind_text(e));
            s.push_str(&excerpt(sm, span));
            s
        }
        None => format!("v2: {}: {first}\n", kind_text(e)),
    };
    for line in rest {
        out.push_str(&format!("    {line}\n"));
    }
    if let Some(trace) = e.trace.get() {
        render_frames(&mut out, trace, "  ");
    }
    let mut cause = &e.cause;
    for _ in 0..MAX_CAUSES {
        let Value::Error(c) = cause else { break };
        out.push_str(&format!("  caused by: {}: {}\n", kind_text(c), split_message(&c.message).0));
        if let Some(trace) = c.trace.get() {
            render_frames(&mut out, trace, "    ");
        }
        cause = &c.cause;
    }
    out
}

/// More than 100 frames: the first 50, `... N frames omitted`, then the last 20 (2.4).
fn render_frames(out: &mut String, trace: &[Rc<str>], indent: &str) {
    if trace.len() > 100 {
        for f in &trace[..50] {
            out.push_str(&format!("{indent}at {f}\n"));
        }
        out.push_str(&format!("{indent}... {} frames omitted\n", trace.len() - 70));
        for f in &trace[trace.len() - 20..] {
            out.push_str(&format!("{indent}at {f}\n"));
        }
    } else {
        for f in trace {
            out.push_str(&format!("{indent}at {f}\n"));
        }
    }
}

/// A static diagnostic (`SyntaxError`), as printed to stderr.
pub fn render_diag(sm: &SourceMap, d: &Diag) -> String {
    let display = &sm.file(d.span.file).display;
    let (first, rest) = split_message(&d.message);
    let mut out = format!("{display}:{}:{}: SyntaxError: {first}\n", d.span.line, d.span.col);
    out.push_str(&excerpt(sm, d.span));
    for line in rest {
        out.push_str(&format!("    {line}\n"));
    }
    for (text, span) in &d.notes {
        let file = &sm.file(span.file).display;
        out.push_str(&format!("  note: {text} ({file}:{}:{})\n", span.line, span.col));
    }
    if let Some(h) = &d.hint {
        out.push_str(&format!("  hint: {h}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::span::FileId;
    use std::path::PathBuf;

    #[test]
    fn did_you_mean_thresholds() {
        let names = ["print", "input", "len"];
        assert_eq!(did_you_mean("pritn", names), Some("print"));
        assert_eq!(did_you_mean("Print", names), Some("print"));
        assert_eq!(did_you_mean("lne", names), Some("len"));
        assert_eq!(did_you_mean("xyz", names), None);
        assert_eq!(did_you_mean("print", names), None);
    }

    fn sm_with(text: &str) -> (SourceMap, FileId) {
        let mut sm = SourceMap::new();
        let id = sm.add(PathBuf::from("main.v2"), "main.v2".into(), text);
        (sm, id)
    }

    #[test]
    fn renders_runtime_error_with_excerpt_and_frames() {
        let (sm, id) = sm_with("print(\"a\")\n    nope(1)\n");
        let span = sm.span(id, 15, 19);
        let e = ErrorValue::new("NameError", "'nope' is not declared");
        e.span.set(span).unwrap();
        e.trace.set(vec![Rc::<str>::from("main.v2:2")].into()).unwrap();
        assert_eq!(
            render_error(&sm, &e),
            "main.v2:2:5: NameError: 'nope' is not declared\n    nope(1)\n    ^\n  at main.v2:2\n"
        );
    }

    #[test]
    fn renders_diag_with_notes_and_hint() {
        let (sm, id) = sm_with("f(1]\n");
        let d = Diag::new("found `]`", sm.span(id, 3, 4)).note("`(` opened here", sm.span(id, 1, 2)).hint("close it");
        assert_eq!(
            render_diag(&sm, &d),
            "main.v2:1:4: SyntaxError: found `]`\n    f(1]\n       ^\n  note: `(` opened here (main.v2:1:2)\n  hint: close it\n"
        );
    }

    #[test]
    fn long_lines_are_windowed() {
        let line = format!("{}X{}", "a".repeat(200), "b".repeat(200));
        let (sm, id) = sm_with(&line);
        let ex = excerpt(&sm, sm.span(id, 200, 201));
        let lines: Vec<&str> = ex.lines().collect();
        let caret = lines[1].find('^').unwrap();
        assert_eq!(&lines[0][caret..caret + 1], "X");
        assert!(lines[0].starts_with("    ...") && lines[0].ends_with("..."));
    }
}
