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
    /// What to do about it, printed only when the error ends the program (2.4); never part of
    /// `e.message`, so a program that catches it sees the error as it was raised.
    pub hint: OnceCell<Rc<str>>,
    /// More lines printed with it then, before the hint: an imported file's problem, with its
    /// own line and caret (6.4).
    pub detail: OnceCell<Rc<str>>,
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
            hint: OnceCell::new(),
            detail: OnceCell::new(),
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

/// The closest candidate, compared lower-cased, if within 1 for a name of up to 5 chars, else
/// `min(3, (len + 2) / 3)`; of
/// candidates as close, the first that starts with the name, else the first.
pub fn did_you_mean<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let target: Vec<char> = name.to_lowercase().chars().collect();
    let limit = if target.len() <= 5 { 1 } else { 3usize.min(target.len().div_ceil(3)) };
    let mut best: Option<(usize, &str)> = None;
    for c in candidates {
        if c == name {
            continue;
        }
        let cand: Vec<char> = c.to_lowercase().chars().collect();
        let d = osa(&target, &cand);
        // A suggestion keeps some of the name: `y` isn't a typo of `f`.
        let prefix = cand.starts_with(&target);
        let better = |(bd, b): (usize, &str)| d < bd || (d == bd && prefix && !b.to_lowercase().starts_with(&*name.to_lowercase()));
        if d <= limit && d < target.len().max(cand.len()) && best.is_none_or(better) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| c)
}

// ---------------------------------------------------------------------------------------------
// Rendering (2.4)

/// The source line under `span` with a caret, indented 4. Leading whitespace is removed, tabs
/// print as one space, other control characters as their visible symbols, bidi formatting
/// characters escaped (`\u{202e}`), and lines over 120 chars are cut to a window around the caret.
pub fn excerpt(sm: &SourceMap, span: Span) -> String {
    let line = sm.line_text(span.file, span.line);
    let removed = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
    let col = (span.col as usize).saturating_sub(1 + removed);
    let (chars, mut caret) = visible_line(line.chars().skip(removed), col);
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
    // The caret goes under its character in a terminal, where wide characters take two columns;
    // an emoji joined to the one before (`👩‍💻`, after a zero-width joiner) shares its glyph.
    let mut prev = ' ';
    let pad: usize = text
        .chars()
        .take(caret)
        .map(|c| {
            let w = if prev == '\u{200D}' { 0 } else { columns(c) };
            prev = c;
            w
        })
        .sum();
    format!("    {text}\n    {}^\n", " ".repeat(pad))
}

/// The terminal columns a character takes (2.4): 2 for East Asian wide and fullwidth characters
/// and the common emoji, 0 for combining marks and zero-width characters, else 1.
fn columns(c: char) -> usize {
    match c as u32 {
        // Combining marks (Latin, Cyrillic, Hebrew, Arabic, Devanagari, Thai and the general
        // blocks), zero-width and bidi controls, variation selectors, skin tones.
        0x0300..=0x036F
        | 0x0483..=0x0489
        | 0x0591..=0x05BD
        | 0x05BF
        | 0x05C1..=0x05C2
        | 0x05C4..=0x05C5
        | 0x05C7
        | 0x0610..=0x061A
        | 0x064B..=0x065F
        | 0x0670
        | 0x06D6..=0x06DC
        | 0x06DF..=0x06E4
        | 0x06E7..=0x06E8
        | 0x06EA..=0x06ED
        | 0x0900..=0x0902
        | 0x093A
        | 0x093C
        | 0x0941..=0x0948
        | 0x094D
        | 0x0951..=0x0957
        | 0x0962..=0x0963
        | 0x0E31
        | 0x0E34..=0x0E3A
        | 0x0E47..=0x0E4E
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        | 0x200B..=0x200F
        | 0x202A..=0x202E
        | 0x2060..=0x2064
        | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F
        | 0xFE20..=0xFE2F
        | 0xFEFF
        | 0x1F3FB..=0x1F3FF
        | 0xE0100..=0xE01EF => 0,
        // East Asian wide and fullwidth: Hangul, CJK, kana, fullwidth forms, and the emoji that
        // terminals draw two columns wide.
        0x1100..=0x115F
        | 0x231A..=0x231B
        | 0x2329..=0x232A
        | 0x23E9..=0x23EC
        | 0x23F0
        | 0x23F3
        | 0x25FD..=0x25FE
        | 0x2614..=0x2615
        | 0x2648..=0x2653
        | 0x267F
        | 0x2693
        | 0x26A1
        | 0x26AA..=0x26AB
        | 0x26BD..=0x26BE
        | 0x26C4..=0x26C5
        | 0x26CE
        | 0x26D4
        | 0x26EA
        | 0x26F2..=0x26F3
        | 0x26F5
        | 0x26FA
        | 0x26FD
        | 0x2705
        | 0x270A..=0x270B
        | 0x2728
        | 0x274C
        | 0x274E
        | 0x2753..=0x2755
        | 0x2757
        | 0x2795..=0x2797
        | 0x27B0
        | 0x27BF
        | 0x2B1B..=0x2B1C
        | 0x2B50
        | 0x2B55
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x16FE0..=0x16FE4
        | 0x17000..=0x18AFF
        | 0x1B000..=0x1B2FF
        | 0x1F004
        | 0x1F0CF
        | 0x1F18E
        | 0x1F191..=0x1F19A
        | 0x1F200..=0x1F2FF
        | 0x1F300..=0x1F64F
        | 0x1F680..=0x1F6FF
        | 0x1F7E0..=0x1F7EB
        | 0x1F90C..=0x1F9FF
        | 0x1FA70..=0x1FAFF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

/// What an excerpt shows for a character: a tab as one space, other C0 controls and DEL as their
/// Control Pictures symbol (U+2400 on), C1 controls as U+FFFD, so a stray control character, an
/// escape sequence included, can't garble the terminal (2.4).
fn visible(c: char) -> char {
    match c {
        '\t' => ' ',
        '\u{0}'..='\u{1F}' => char::from_u32(0x2400 + c as u32).unwrap_or('\u{FFFD}'),
        '\u{7F}' => '\u{2421}',
        '\u{80}'..='\u{9F}' => '\u{FFFD}',
        c => c,
    }
}

/// An excerpt's characters as `visible` shows them, a bidi formatting character as `\u{202e}` so
/// it can't reorder the line (2.4), and where the char at index `col` of `line` went.
fn visible_line(line: impl Iterator<Item = char>, col: usize) -> (Vec<char>, usize) {
    let (mut chars, mut caret, mut count) = (Vec::new(), None, 0);
    for (k, c) in line.enumerate() {
        if k == col {
            caret = Some(chars.len());
        }
        if crate::syntax::span::is_bidi_format(c) {
            chars.extend(format!("\\u{{{:x}}}", c as u32).chars());
        } else {
            chars.push(visible(c));
        }
        count = k + 1;
    }
    let caret = caret.unwrap_or(chars.len() + col.saturating_sub(count));
    (chars, caret)
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
    if let Some(d) = e.detail.get() {
        out.push_str(d);
    }
    if let Some(h) = e.hint.get() {
        out.push_str(&format!("  hint: {h}\n"));
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

/// The frames as lines (2.4): a run of the same frame, or of the same 2 to 4 frames (a recursion
/// through other languages), that comes three or more times in a row prints once, then
/// `... the frame above repeated N more times`. Then, more than 100 lines: the first 50,
/// `... N frames omitted`, the last 20.
fn render_frames(out: &mut String, trace: &[Rc<str>], indent: &str) {
    // Each line with the number of frames it stands for.
    let mut lines: Vec<(String, usize)> = Vec::new();
    let mut i = 0;
    while i < trace.len() {
        let mut best: Option<(usize, usize)> = None;
        for period in 1..=4 {
            let unit = &trace[i..(i + period).min(trace.len())];
            if unit.len() < period {
                break;
            }
            let mut reps = 1;
            while trace.get(i + reps * period..i + (reps + 1) * period) == Some(unit) {
                reps += 1;
            }
            if reps >= 3 && best.is_none_or(|(p, r)| period * reps > p * r) {
                best = Some((period, reps));
            }
        }
        match best {
            Some((period, reps)) => {
                lines.extend(trace[i..i + period].iter().map(|f| (format!("at {f}"), 1)));
                let what = if period == 1 { "the frame above".to_string() } else { format!("the {period} frames above") };
                lines.push((format!("... {what} repeated {} more times", reps - 1), period * (reps - 1)));
                i += period * reps;
            }
            None => {
                lines.push((format!("at {}", trace[i]), 1));
                i += 1;
            }
        }
    }
    let mut put = |text: &str| out.push_str(&format!("{indent}{text}\n"));
    if lines.len() > 100 {
        let omitted: usize = lines[50..lines.len() - 20].iter().map(|(_, n)| n).sum();
        lines[..50].iter().for_each(|(t, _)| put(t));
        put(&format!("... {omitted} frames omitted"));
        lines[lines.len() - 20..].iter().for_each(|(t, _)| put(t));
    } else {
        lines.iter().for_each(|(t, _)| put(t));
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
        // One-letter names: every other one-letter name is one edit away, so none is suggested.
        assert_eq!(did_you_mean("y", ["f", "x"]), None);
        assert_eq!(did_you_mean("x", ["xs"]), Some("xs"));
        assert_eq!(did_you_mean("ab", ["ba"]), Some("ba"));
        // A tie goes to the one that starts with the name, whatever the order.
        assert_eq!(did_you_mean("stat", ["star", "stats", "stats2"]), Some("stats"));
        assert_eq!(did_you_mean("stat", ["start", "star"]), Some("start"));
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
    fn the_caret_counts_terminal_columns() {
        // 2.4: wide characters take two columns, so the caret still lands under the `+`.
        let (sm, id) = sm_with("let 名前 = \"测试\" + 1\n");
        // `+` is byte 22, character 15; before it: `let ` 4, 名前 4, ` = ` 3, `"` 1, 测试 4, `" ` 2.
        let d = Diag::new("x", sm.span(id, 22, 23));
        assert_eq!(render_diag(&sm, &d), format!("main.v2:1:15: SyntaxError: x\n    let 名前 = \"测试\" + 1\n    {}^\n", " ".repeat(18)));
        // An emoji two columns wide (✅, 3 bytes) and combining marks of no width (नमस्ते: 6 chars,
        // 4 columns), each before `nope`.
        let (sm, id) = sm_with("print(\"✅ done\", nope)\n");
        let d = Diag::new("x", sm.span(id, 18, 22));
        assert_eq!(render_diag(&sm, &d), format!("main.v2:1:17: SyntaxError: x\n    print(\"✅ done\", nope)\n    {}^\n", " ".repeat(17)));
        let (sm, id) = sm_with("print(\"नमस्ते\", nope)\n");
        let d = Diag::new("x", sm.span(id, 28, 32));
        assert_eq!(render_diag(&sm, &d), format!("main.v2:1:17: SyntaxError: x\n    print(\"नमस्ते\", nope)\n    {}^\n", " ".repeat(14)));
        // An emoji sequence joined with ZWJ (👩🏽‍💻: woman, skin tone, ZWJ, laptop) is one glyph,
        // two columns: before `nope`, `print("` 7, the glyph 2, `", ` 3.
        let text = "print(\"\u{1F469}\u{1F3FD}\u{200D}\u{1F4BB}\", nope)\n";
        let (sm, id) = sm_with(text);
        let at = text.find("nope").unwrap() as u32;
        let d = Diag::new("x", sm.span(id, at, at + 4));
        assert!(render_diag(&sm, &d).ends_with(&format!("\n    {}^\n", " ".repeat(12))), "{}", render_diag(&sm, &d));
    }

    #[test]
    fn repeated_frames_print_once() {
        // 2.4: a recursion's frames fold; so does a cycle through other languages.
        let frames = |list: &[&str]| -> Vec<Rc<str>> { list.iter().map(|f| Rc::<str>::from(*f)).collect() };
        let mut trace = vec!["f (r.v2:2)"; 10_000];
        trace.push("r.v2:4");
        let mut out = String::new();
        render_frames(&mut out, &frames(&trace), "  ");
        assert_eq!(out, "  at f (r.v2:2)\n  ... the frame above repeated 9999 more times\n  at r.v2:4\n");
        let mut trace = Vec::new();
        for _ in 0..30 {
            trace.extend(["pong (m.v2:9)", "relay (m.v2:2, in @js j)", "ping (m.v2:6)"]);
        }
        trace.push("m.v2:13");
        let mut out = String::new();
        render_frames(&mut out, &frames(&trace), "  ");
        assert_eq!(
            out,
            "  at pong (m.v2:9)\n  at relay (m.v2:2, in @js j)\n  at ping (m.v2:6)\n  ... the 3 frames above repeated 29 more times\n  at m.v2:13\n"
        );
        // Two in a row stay as they are; 150 different frames are cut to 50, a count, 20.
        let mut out = String::new();
        render_frames(&mut out, &frames(&["a", "a", "b"]), "  ");
        assert_eq!(out, "  at a\n  at a\n  at b\n");
        let many: Vec<String> = (0..150).map(|i| format!("f{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        let mut out = String::new();
        render_frames(&mut out, &frames(&many), "  ");
        assert!(out.lines().count() == 71 && out.contains("\n  ... 80 frames omitted\n  at f130\n"), "{out}");
    }

    #[test]
    fn excerpts_show_control_characters() {
        // A stray escape sequence in a file can't recolour the terminal; a tab is one space (2.4).
        let (sm, id) = sm_with("let a = 1\u{1B}[31m\t\u{0}\u{7F}\u{85}\n");
        let d = Diag::new("unexpected character U+001B", sm.span(id, 9, 10));
        assert_eq!(render_diag(&sm, &d), "main.v2:1:10: SyntaxError: unexpected character U+001B\n    let a = 1\u{241B}[31m \u{2400}\u{2421}\u{FFFD}\n             ^\n");
    }

    #[test]
    fn excerpts_escape_bidi_controls() {
        // A bidi override can't reorder the line shown (2.4, round 20): it prints as \u{202e}, and
        // the caret moves past what it became. Past the line's end the caret stays past it.
        let (sm, id) = sm_with("let a = \"\u{202E}x\u{2066}\" + nope\n");
        let at = "let a = \"\u{202E}x\u{2066}\" + ".len() as u32;
        let d = Diag::new("x", sm.span(id, at, at + 4));
        let shown = "let a = \"\\u{202e}x\\u{2066}\" + nope";
        assert_eq!(render_diag(&sm, &d), format!("main.v2:1:17: SyntaxError: x\n    {shown}\n    {}^\n", " ".repeat(shown.len() - 4)));
        let (sm, id) = sm_with("x\u{202E}\n");
        let d = Diag::new("x", sm.span(id, 4, 4));
        assert_eq!(render_diag(&sm, &d), format!("main.v2:1:3: SyntaxError: x\n    x\\u{{202e}}\n{}^\n", " ".repeat(4 + 9)));
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
