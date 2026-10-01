//! Error values, did-you-mean and rendering (INTERNALS 2.4, 2.5, 5.9).

use std::rc::Rc;

use crate::syntax::span::{SourceMap, Span};
use crate::syntax::Diag;

/// A raised (or not yet raised) V2 error. Roadmap 0.2 carries kind, message, location and
/// trace; `data`, `cause` and the foreign fields arrive with 0.7 and Phase 1.
#[derive(Debug, Clone)]
pub struct ErrorValue {
    pub kind: Rc<str>,
    pub message: String,
    /// Where it was first raised; `None` until then (5.9).
    pub span: Option<Span>,
    /// Frame strings, innermost first (2.5).
    pub trace: Vec<String>,
}

impl ErrorValue {
    pub fn new(kind: &str, message: impl Into<String>) -> ErrorValue {
        ErrorValue { kind: kind.into(), message: message.into(), span: None, trace: Vec::new() }
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
    for j in 0..=m {
        d[0][j] = j;
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
    let limit = 1usize.max(3usize.min((target.len() + 2) / 3));
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

/// An uncaught error, as printed to stderr.
pub fn render_error(sm: &SourceMap, e: &ErrorValue) -> String {
    let (first, rest) = split_message(&e.message);
    let mut out = match e.span {
        Some(span) => {
            let display = &sm.file(span.file).display;
            let mut s = format!("{display}:{}:{}: {}: {first}\n", span.line, span.col, e.kind);
            s.push_str(&excerpt(sm, span));
            s
        }
        None => format!("v2: {}: {first}\n", e.kind),
    };
    for line in rest {
        out.push_str(&format!("    {line}\n"));
    }
    render_frames(&mut out, &e.trace);
    out
}

/// More than 100 frames: the first 50, `... N frames omitted`, then the last 20 (2.4).
fn render_frames(out: &mut String, trace: &[String]) {
    if trace.len() > 100 {
        for f in &trace[..50] {
            out.push_str(&format!("  at {f}\n"));
        }
        out.push_str(&format!("  ... {} frames omitted\n", trace.len() - 70));
        for f in &trace[trace.len() - 20..] {
            out.push_str(&format!("  at {f}\n"));
        }
    } else {
        for f in trace {
            out.push_str(&format!("  at {f}\n"));
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
        let e = ErrorValue {
            kind: "NameError".into(),
            message: "'nope' is not declared".into(),
            span: Some(span),
            trace: vec!["main.v2:2".into()],
        };
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
