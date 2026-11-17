//! Block bodies: lines, `@export`/`@import` directives and dedent (INTERNALS 4.4).
//!
//! A raw body becomes what the engine receives: the code as written, with directive lines blanked
//! (so foreign line numbers never shift) and the common indentation removed, plus a `UnitLoc` for
//! mapping foreign positions back to the `.v2` file and the parsed directives.

use std::rc::Rc;

use super::ast::{Directives, EngineImport, ExportItem, Exports, ImportItem, ImportItems, Name, Selector};
use super::lexer::LABEL_HINT;
use super::scan::{canonical_tag, profile_for, Profile};
use super::span::{FileId, SourceMap, Span, UnitLoc};
use super::token::{Delim, Kw, RawBody};
use super::Diag;

/// The processed body of one block.
pub struct Processed {
    pub body: Rc<str>,
    pub loc: UnitLoc,
    pub directives: Directives,
}

/// Tags whose `@export` items are function names only (4.4, 15.2).
const COMPILED_TAGS: &[&str] = &["c", "cpp", "cxx", "rust", "rs"];

struct Line {
    text: String,
    /// File offset of `text`'s first byte.
    offset: usize,
    /// Starts in code (not inside a multi-line foreign string or comment, 3.8).
    in_code: bool,
    /// Body line 1 taken from the header line: trimmed, never dedented.
    header_line: bool,
}

/// Turn a raw body into body text, location and directives (4.4 steps 1-3).
pub fn process(sm: &SourceMap, raw: &RawBody, tag: &str) -> Result<Processed, Diag> {
    let file = raw.text_span.file;
    let base = raw.text_span.lo as usize;
    let text: &str = &raw.text;
    let mut pieces: Vec<(usize, &str)> = Vec::new();
    let mut off = 0;
    for piece in text.split('\n') {
        pieces.push((off, piece));
        off += piece.len() + 1;
    }
    let state = |i: usize| raw.starts_in_code.get(i).copied().unwrap_or(true);
    let blank = |s: &str| s.chars().all(|c| c == ' ' || c == '\t');
    let lead_of = |s: &str| s.len() - s.trim_start_matches([' ', '\t']).len();

    // Step 1: lines.
    let mut lines: Vec<Line> = Vec::new();
    let first_line;
    let mut header_col = None;
    match raw.delim {
        Delim::Brace if !text.contains('\n') => {
            first_line = raw.open.line;
            let lead = lead_of(text);
            header_col = Some(raw.open.col + 1 + text[..lead].chars().count() as u32);
            let t = text.trim_matches([' ', '\t']);
            if !t.is_empty() {
                lines.push(Line { text: t.into(), offset: base + lead, in_code: true, header_line: true });
            }
        }
        Delim::Brace => {
            let (o0, l1) = pieces[0];
            if blank(l1) {
                first_line = raw.open.line + 1;
            } else {
                first_line = raw.open.line;
                let lead = lead_of(l1);
                header_col = Some(raw.open.col + 1 + l1[..lead].chars().count() as u32);
                let t = l1.trim_matches([' ', '\t']);
                lines.push(Line { text: t.into(), offset: base + o0 + lead, in_code: true, header_line: true });
            }
            // The last piece is the (blank, C1) text before the closing `}`: dropped.
            for (i, &(o, l)) in pieces.iter().enumerate().take(pieces.len() - 1).skip(1) {
                lines.push(Line { text: l.into(), offset: base + o, in_code: state(i), header_line: false });
            }
        }
        Delim::Fence(_) => {
            first_line = raw.open.line + 1;
            let n = if text.is_empty() || text.ends_with('\n') { pieces.len() - 1 } else { pieces.len() };
            for (i, &(o, l)) in pieces.iter().enumerate().take(n) {
                lines.push(Line { text: l.into(), offset: base + o, in_code: state(i), header_line: false });
            }
        }
    }

    // Step 2: directives.
    let compiled = COMPILED_TAGS.contains(&tag);
    let mut directives = Directives::default();
    let mut escapes = Vec::new(); // (line index, byte where the `@` was)
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].text.trim_start_matches([' ', '\t']);
        let lead = lines[i].text.len() - t.len();
        if is_directive(t, "@export") || is_directive(t, "@import") {
            let mut p = DirParser::new(sm, file, &lines, i, lead);
            // A line that doesn't parse may be the block's own code: a CSS @import in a template
            // literal or a docstring (4.4).
            let (word, example) = if is_directive(t, "@export") { ("@export", "") } else { ("@import", " (a CSS @import in a string)") };
            let escape = |d: Diag| match d.hint {
                Some(_) => d,
                None => d.hint(format!("a line that starts with {word} is V2's; for code of the block's own language that starts with {word}{example}, write @{word}")),
            };
            let last = p.directive(&mut directives, tag, compiled).map_err(escape)?;
            for line in &mut lines[i..=last] {
                line.text.clear();
            }
            i = last + 1;
            continue;
        }
        if is_directive(t, "@@export") || is_directive(t, "@@import") {
            lines[i].text.remove(lead); // the escape loses one `@`
            escapes.push((i, lead));
        } else if let Some((at, word)) = stray_directive(t).filter(|_| lines[i].in_code) {
            // Indented with no-break spaces (code copied from a web page): name that character,
            // as the lexer does (3.2), rather than the directive's place.
            let before = &t[..at];
            let blank = |c: char| c.is_whitespace() || matches!(c, '\u{200B}' | '\u{FEFF}');
            if let Some((k, c)) = before.char_indices().next().filter(|_| before.chars().all(blank)) {
                let lo = (lines[i].offset + lead + k) as u32;
                let (shown, hint) = super::lexer::shown_char(c);
                let d = Diag::new(format!("unexpected character {shown}"), sm.span(file, lo, lo + c.len_utf8() as u32));
                return Err(match hint {
                    Some(h) => d.hint(h),
                    None => d,
                });
            }
            let lo = (lines[i].offset + lead + at) as u32;
            return Err(Diag::new(format!("{word} goes on a line of its own"), sm.span(file, lo, lo + word.len() as u32)));
        }
        i += 1;
    }
    if compiled {
        compiled_directives(&directives, tag)?;
    }
    let mut bound: Vec<(&Rc<str>, Span)> = Vec::new();
    for import in &directives.imports {
        let ImportItems::Names(items) = &import.items else { continue };
        for item in items {
            let local = item.alias.as_ref().map_or(&item.name, |a| &a.text);
            if let Some((_, first)) = bound.iter().find(|(n, _)| *n == local) {
                return Err(Diag::new(format!("'{local}' is imported twice"), item.span).note("first imported here", *first));
            }
            bound.push((local, item.span));
        }
    }

    // A no-break space (or another Unicode space) pasted from a web page into a Python line's
    // indentation: Python rejects it, and may report another line (4.4). JavaScript takes it as
    // a space. Python's own indentation is spaces, tabs and form feeds.
    let python = matches!(profile_for(tag), Profile::Python);
    for line in lines.iter().filter(|l| python && !l.header_line && l.in_code) {
        let lead = line.text.find(|c: char| !c.is_whitespace()).unwrap_or(line.text.len());
        if let Some((i, c)) = line.text[..lead].char_indices().find(|&(_, c)| !matches!(c, ' ' | '\t' | '\x0c')) {
            let (name, hint) = super::lexer::shown_char(c);
            let at = line.offset as u32 + i as u32;
            let d = Diag::new(format!("unexpected character {name} in the block's indentation"), sm.span(file, at, at + c.len_utf8() as u32));
            return Err(match hint {
                Some(h) => d.hint(h),
                None => d,
            });
        }
    }

    // Step 3: dedent the code lines. A line that is only a comment doesn't set the indentation:
    // the language doesn't care where it starts.
    let profile = profile_for(tag);
    let comment = |t: &str| {
        let t = t.trim_start_matches([' ', '\t']);
        match profile {
            Profile::Python => t.starts_with('#'),
            Profile::CLike(_) => t.starts_with("//") || t.starts_with("/*"),
            Profile::Plain => false,
        }
    };
    let code: Vec<usize> = (0..lines.len())
        .filter(|&k| !lines[k].header_line && lines[k].in_code && !blank(&lines[k].text) && !comment(&lines[k].text))
        .collect();
    for line in lines.iter_mut() {
        if !line.header_line && line.in_code && blank(&line.text) {
            line.text.clear();
        }
    }
    let m = code.iter().map(|&k| lead_of(&lines[k].text)).min().unwrap_or(0);
    if let Some(&first) = code.first() {
        let prefix = lines[first].text[..m].to_string();
        for &k in &code[1..] {
            if lines[k].text[..m] != prefix {
                // The shown lines don't show their indentation: say what each starts with.
                let lead = |t: &str| indent_words(&t[..lead_of(t)]);
                let hint = format!(
                    "this line starts with {}, the block's first code line with {}: indent every line the same way",
                    lead(&lines[k].text),
                    lead(&lines[first].text)
                );
                return Err(Diag::new("block indentation mixes tabs and spaces", sm.span(file, lines[k].offset as u32, lines[k].offset as u32 + 1))
                    .note("the first code line of the block", sm.span(file, lines[first].offset as u32, lines[first].offset as u32 + 1))
                    .hint(hint));
            }
        }
        let mut kept = Vec::new();
        for (k, line) in lines.iter_mut().enumerate() {
            if line.header_line || line.text.is_empty() {
                continue;
            }
            if line.text.starts_with(&prefix) {
                line.text.drain(..m);
            } else {
                kept.push(k as u32 + 1);
            }
        }
        let escaped = escapes.iter().map(|&(k, lead)| (k as u32 + 1, if kept.contains(&(k as u32 + 1)) { lead + 1 } else { lead - m + 1 } as u32)).collect();
        return Ok(finish(file, lines, first_line, header_col.unwrap_or(m as u32 + 1), (m as u32, kept, escaped), directives));
    }
    let escaped = escapes.iter().map(|&(k, lead)| (k as u32 + 1, lead as u32 + 1)).collect();
    Ok(finish(file, lines, first_line, header_col.unwrap_or(1), (0, Vec::new(), escaped), directives))
}

fn finish(
    file: FileId,
    lines: Vec<Line>,
    first_line: u32,
    body_col: u32,
    (dedent_width, kept, escaped): (u32, Vec<u32>, Vec<(u32, u32)>),
    directives: Directives,
) -> Processed {
    let line_count = lines.len() as u32;
    let body: String = if lines.iter().all(|l| l.text.is_empty()) {
        String::new()
    } else {
        let mut s = lines.into_iter().map(|l| l.text).collect::<Vec<_>>().join("\n");
        s.push('\n');
        s
    };
    Processed {
        body: body.into(),
        loc: UnitLoc { file, body_first_line: first_line, body_col, dedent_width, line_count, kept: kept.into(), escaped: escaped.into() },
        directives,
    }
}

/// `@export {` or `@import {` after code on a line, where it would reach the other language as
/// code. Past a quote, or a `#` or `/` that can start a comment or a regex, the text may not be
/// code, so it is left alone (DOCS 6.1).
fn stray_directive(t: &str) -> Option<(usize, &'static str)> {
    let code = &t[..t.find(['"', '\'', '`', '#', '/']).unwrap_or(t.len())];
    ["@export", "@import"].into_iter().find_map(|word| {
        let (at, _) = code.match_indices(word).find(|&(i, _)| {
            i > 0 && code.as_bytes()[i - 1] != b'@' && code[i + word.len()..].trim_start().starts_with('{')
        })?;
        Some((at, word))
    })
}

/// `@export` (or `@import`, `@@export`, …) as an exact token: followed by a space, a tab, `{`
/// or the end of the line. `@exporter` and `@import_hook` are foreign code.
fn is_directive(t: &str, word: &str) -> bool {
    t.strip_prefix(word).is_some_and(|rest| matches!(rest.chars().next(), None | Some(' ' | '\t' | '{')))
}

// ---------------------------------------------------------------------------------------------
// Directive-mode lexing and parsing (4.4 step 2)

#[derive(Clone, Debug, PartialEq)]
enum DK {
    Export,
    Import,
    LBrace,
    RBrace,
    Comma,
    Star,
    Dot,
    At(Rc<str>),
    Name(Rc<str>),
    /// A V2 keyword (3.2); right after `.` it lexes as a `Name` instead.
    Kw(Rc<str>),
    Str(Rc<str>),
    /// A line break outside `{ }`: the directive ends.
    End,
}

struct DTok {
    kind: DK,
    span: Span,
    /// Index of the body line the token is on.
    line: usize,
}

struct DirParser<'a> {
    toks: Vec<DTok>,
    pos: usize,
    sm: &'a SourceMap,
    file: FileId,
    lex_error: Option<Diag>,
}

impl<'a> DirParser<'a> {
    /// Lex one directive starting at `lines[first]`, byte `start`. Inside `{ }` it continues over
    /// line breaks; outside, the first line break ends it. `#` and `//` start a comment.
    fn new(sm: &'a SourceMap, file: FileId, lines: &[Line], first: usize, start: usize) -> DirParser<'a> {
        let mut toks = Vec::new();
        let mut depth = 0i32;
        let mut lex_error = None;
        let mut li = first;
        let mut col = start;
        'lines: while li < lines.len() {
            let text = &lines[li].text;
            let base = lines[li].offset;
            let span = |lo: usize, hi: usize| sm.span(file, (base + lo) as u32, (base + hi) as u32);
            while col < text.len() {
                let rest = &text[col..];
                let c = rest.chars().next().unwrap_or(' ');
                let len = c.len_utf8();
                let (kind, n) = match c {
                    ' ' | '\t' => {
                        col += 1;
                        continue;
                    }
                    '#' => break,
                    '/' if rest.starts_with("//") => break,
                    '{' => {
                        depth += 1;
                        (DK::LBrace, 1)
                    }
                    '}' => {
                        depth -= 1;
                        (DK::RBrace, 1)
                    }
                    ',' => (DK::Comma, 1),
                    '*' => (DK::Star, 1),
                    '.' => (DK::Dot, 1),
                    '@' => {
                        // TAG is [A-Za-z_][A-Za-z0-9_]* (3.6).
                        let starts_ok = rest[1..].chars().next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic());
                        let word: String = if starts_ok {
                            rest[1..].chars().take_while(|c| *c == '_' || c.is_ascii_alphanumeric()).collect()
                        } else {
                            String::new()
                        };
                        match word.as_str() {
                            "export" => (DK::Export, 7),
                            "import" => (DK::Import, 7),
                            "" => {
                                lex_error.get_or_insert(Diag::new("expected an engine tag after @", span(col, col + 1)));
                                break 'lines;
                            }
                            w => (DK::At(w.into()), 1 + w.len()),
                        }
                    }
                    '"' | '\'' => match dir_string(rest, false) {
                        Ok((s, n)) => (DK::Str(s.into()), n),
                        Err((msg, at)) => {
                            lex_error.get_or_insert(Diag::new(msg, span(col + at, col + at + 1)));
                            break 'lines;
                        }
                    },
                    'r' if matches!(rest[1..].chars().next(), Some('"' | '\'')) => match dir_string(&rest[1..], true) {
                        Ok((s, n)) => (DK::Str(s.into()), n + 1),
                        Err((msg, at)) => {
                            lex_error.get_or_insert(Diag::new(msg, span(col + 1 + at, col + 2 + at)));
                            break 'lines;
                        }
                    },
                    c if c == '_' || c.is_alphabetic() => {
                        let n: usize = rest.chars().take_while(|c| *c == '_' || c.is_alphanumeric()).map(char::len_utf8).sum();
                        let w = &rest[..n];
                        if matches!(rest[n..].chars().next(), Some('"' | '\'')) {
                            lex_error.get_or_insert(
                                Diag::new(format!("unknown string prefix `{w}` in a directive"), span(col, col + n))
                                    .hint("directive names are plain \"…\" or raw r\"…\" strings"),
                            );
                            break 'lines;
                        }
                        let after_dot = matches!(toks.last(), Some(DTok { kind: DK::Dot, .. }));
                        let kind = if Kw::from_word(w).is_some() && !after_dot { DK::Kw(w.into()) } else { DK::Name(w.into()) };
                        (kind, n)
                    }
                    other => {
                        // `@export { f， g }`: once the directive's `{` is read, a space, a
                        // zero-width character or a quote is a slip, with its hint (3.2), and so
                        // is an input method's form whose ASCII fits where it stands; `；` in the
                        // list gets the comma. Any other character, or one before the `{` (a
                        // docstring's `@export – marks it`), gets the @@export one: the line may
                        // be the block's own text.
                        let opened = depth > 0 || toks.iter().any(|t| t.kind == DK::LBrace);
                        let after = &rest[len..];
                        let (shown, hint) = crate::syntax::lexer::shown_char(other);
                        let hint = match other {
                            '\u{FF1B}' if depth > 0 => Some("separate names with a comma: ,".to_string()),
                            // `@export ｛f， g｝`: a brace block's braces are ASCII ones, so both go.
                            '\u{FF5B}' if toks.len() == 1 && after.contains('\u{FF5D}') => Some("type { and } with the input method off".to_string()),
                            '\u{FF01}'..='\u{FF5E}' => hint.filter(|_| char::from_u32(other as u32 - 0xFEE0).is_some_and(|ascii| fits(ascii, &toks, depth, after))),
                            '\u{00A0}' | '\u{1680}' | '\u{2000}'..='\u{200D}' | '\u{202F}' | '\u{205F}' | '\u{2060}' | '\u{3000}' | '\u{FEFF}' | '\u{2018}'..='\u{201F}' | '\u{300C}'..='\u{300F}' => {
                                hint.filter(|_| opened)
                            }
                            _ => None,
                        };
                        let d = Diag::new(format!("unexpected character {shown} in a directive"), span(col, col + len));
                        let d = match hint {
                            Some(h) => d.hint(h),
                            None => d,
                        };
                        // JavaScript's `$x` in an `@import { }` or `@export { }` list goes in quotes.
                        let after_as = matches!(toks.last(), Some(DTok { kind: DK::Kw(w), .. }) if &**w == "as");
                        let d = match toks.first().map(|t| &t.kind) {
                            Some(DK::Import) if other == '$' && depth > 0 => d.hint(crate::syntax::lexer::quote_import_hint(rest, after_as, false)),
                            // `@export { show as $s }`: a rename, which @export can't do.
                            Some(DK::Export) if other == '$' && depth > 0 && after_as => d.hint(rename_hint(toks.len().checked_sub(2).map(|i| &toks[i].kind))),
                            Some(DK::Export) if other == '$' && depth > 0 => {
                                let name: String = rest.chars().take_while(|c| *c == '$' || *c == '_' || c.is_alphanumeric()).collect();
                                d.hint(format!("write a name that isn't a V2 name in quotes: @export {{ {name:?} }}"))
                            }
                            _ => d,
                        };
                        lex_error.get_or_insert(d);
                        break 'lines;
                    }
                };
                toks.push(DTok { kind, span: span(col, col + n), line: li });
                col += n;
            }
            if depth <= 0 {
                let end = span(text.len(), text.len());
                toks.push(DTok { kind: DK::End, span: end, line: li });
                break;
            }
            li += 1;
            col = 0;
        }
        if toks.last().is_none_or(|t| t.kind != DK::End) {
            let (line, span) = toks.last().map(|t| (t.line, t.span)).unwrap_or((first, sm.span(file, lines[first].offset as u32, lines[first].offset as u32)));
            toks.push(DTok { kind: DK::End, span: Span { lo: span.hi, ..span }, line });
        }
        DirParser { toks, pos: 0, sm, file, lex_error }
    }

    fn peek(&self) -> &DK {
        &self.toks[self.pos].kind
    }

    fn span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn bump(&mut self) -> &DTok {
        let i = self.pos;
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        &self.toks[i]
    }

    /// Parse one directive of a `tag` block; returns the index of its last body line.
    fn directive(&mut self, out: &mut Directives, tag: &str, compiled: bool) -> Result<usize, Diag> {
        if let Some(d) = self.lex_error.take() {
            return Err(d);
        }
        let start = self.span();
        match self.peek().clone() {
            DK::Export => {
                self.bump();
                let exports = out.exports.get_or_insert_with(Exports::default);
                exports.at.get_or_insert(start);
                self.export_list(exports, compiled)?;
            }
            _ => {
                self.bump();
                let import = self.import(start, tag)?;
                out.imports.push(import);
            }
        }
        if *self.peek() != DK::End {
            return Err(Diag::new("unexpected text after the directive", self.span())
                .hint("only a comment (# or //) may follow a directive on its line"));
        }
        let _ = (self.sm, self.file);
        Ok(self.toks[self.pos].line)
    }

    fn export_list(&mut self, exports: &mut Exports, compiled: bool) -> Result<(), Diag> {
        if *self.peek() != DK::LBrace {
            // A Python decorator named `export` (`@export.something`) is the language's own line.
            return Err(Diag::new("expected { after @export", self.span()).hint("a line that starts with @export is V2's; for code of the block's own language that starts with @export, write @@export"));
        }
        let open = self.bump().span;
        if *self.peek() == DK::RBrace {
            return Err(Diag::new("an @export list can't be empty", open));
        }
        loop {
            let t = self.bump();
            let span = t.span;
            let item = t.kind.clone();
            match item.clone() {
                DK::Star => {
                    exports.star.get_or_insert(span);
                }
                // Keywords are accepted bare here: they name foreign functions (4.4).
                DK::Name(n) | DK::Kw(n) => push_export(exports, n, span)?,
                // Compiled blocks export functions by name (15.2).
                DK::Str(_) if compiled => {
                    return Err(Diag::new("compiled blocks export functions by name", span)
                        .hint("export a wrapper function with a plain signature"))
                }
                DK::Str(s) => push_export(exports, s, span)?,
                DK::End => return Err(Diag::new("unterminated @export list", open)),
                other => return Err(Diag::new(format!("expected a name in the @export list, found {}", describe(&other)), span)),
            }
            match self.peek().clone() {
                DK::Comma => {
                    self.bump();
                    if *self.peek() == DK::RBrace {
                        self.bump();
                        return Ok(());
                    }
                }
                DK::RBrace => {
                    self.bump();
                    return Ok(());
                }
                DK::End => return Err(Diag::new("unterminated @export list", open)),
                // JavaScript's `export { show as pyshow }`.
                DK::Kw(k) if &*k == "as" => {
                    return Err(Diag::new("expected `,` or `}` in the @export list, found keyword `as`", self.span()).hint(rename_hint(Some(&item))))
                }
                other => return Err(Diag::new(format!("expected `,` or `}}` in the @export list, found {}", describe(&other)), self.span())),
            }
        }
    }

    fn import(&mut self, start: Span, tag: &str) -> Result<EngineImport, Diag> {
        if *self.peek() == DK::LBrace {
            let open = self.bump().span;
            if *self.peek() == DK::Star {
                return Err(Diag::new("@import needs a list of names", self.span()).hint("list the names: @import { mean } from …"));
            }
            let items = {
                let mut items = Vec::new();
                loop {
                    let t = self.bump();
                    let span = t.span;
                    let name = match t.kind.clone() {
                        DK::Name(n) | DK::Str(n) => n,
                        DK::Kw(k) => {
                            return Err(Diag::new(format!("expected a name in the @import list, found keyword `{k}`"), span)
                                .hint(format!("quote a name that is a V2 keyword: \"{k}\"")))
                        }
                        DK::End => return Err(Diag::new("unterminated @import list", open)),
                        other => return Err(Diag::new(format!("expected a name in the @import list, found {}", describe(&other)), span)),
                    };
                    let alias = if matches!(self.peek(), DK::Kw(w) if &**w == "as") {
                        self.bump();
                        Some(self.name("a name after `as`")?)
                    } else {
                        None
                    };
                    items.push(ImportItem { name, alias, span });
                    match self.peek().clone() {
                        DK::Comma => {
                            self.bump();
                            if *self.peek() == DK::RBrace {
                                break;
                            }
                        }
                        DK::RBrace => break,
                        DK::End => return Err(Diag::new("unterminated @import list", open)),
                        other => return Err(Diag::new(format!("expected `,` or `}}` in the @import list, found {}", describe(&other)), self.span())),
                    }
                }
                ImportItems::Names(items)
            };
            if *self.peek() != DK::RBrace {
                return Err(Diag::new("expected `}` to close the @import list", self.span()));
            }
            self.bump();
            if !matches!(self.peek(), DK::Kw(w) if &**w == "from") {
                return Err(Diag::new("expected `from` after the @import list", self.span()));
            }
            self.bump();
            let selector = self.selector()?;
            Ok(EngineImport { items, selector, span: start.to(self.toks[self.pos.saturating_sub(1)].span) })
        } else {
            // Inside a block a whole module comes from the language's own import (13.7).
            let hint = self.whole_module_hint(tag);
            Err(Diag::new("the whole-module form of @import is only valid in V2 code", start).hint(hint))
        }
    }

    /// For `@import SEL as NAME` in a block: the block language's own import of that module,
    /// else the names form.
    fn whole_module_hint(&mut self, tag: &str) -> String {
        let names = "import names instead: @import { … } from …".to_string();
        let Ok(Selector::Module { lang, path }) = self.selector() else { return names };
        if !matches!(self.peek(), DK::Kw(w) if &**w == "as") {
            return names;
        }
        self.bump();
        let Ok(alias) = self.name("") else { return names };
        let module = path.iter().map(|n| &*n.text).collect::<Vec<_>>().join(".");
        // ts runs in the js session, and `ts.pkg` is `js.pkg` (13.5).
        fn family(t: &str) -> &str {
            match canonical_tag(t) {
                "ts" => "js",
                other => other,
            }
        }
        let block = canonical_tag(tag);
        match (family(tag), family(&lang.text)) {
            ("py", "py") if module == *alias.text => format!("use import {module} inside @py blocks"),
            ("py", "py") => format!("use import {module} as {} inside @py blocks", alias.text),
            ("js", "js") => format!("use const {} = require({module:?}) inside @{block} blocks", alias.text),
            _ => names,
        }
    }

    /// `@tag`, `@tag.label`, `label`, `lang.seg.seg`, `v2` (4.1).
    fn selector(&mut self) -> Result<Selector, Diag> {
        let t = self.bump();
        let span = t.span;
        match t.kind.clone() {
            DK::At(tag) => {
                let tag = Name::new(tag, span);
                if *self.peek() == DK::Dot {
                    self.bump();
                    if let DK::Str(_) = self.peek() {
                        return Err(Diag::new("a block label can't be a string", self.span()).hint(LABEL_HINT));
                    }
                    let label = self.segment()?;
                    Ok(Selector::Exact { tag, label })
                } else {
                    Ok(Selector::Union { tag })
                }
            }
            DK::Name(n) => {
                if *self.peek() == DK::Dot {
                    let lang = Name::new(n, span);
                    let mut path = Vec::new();
                    while *self.peek() == DK::Dot {
                        self.bump();
                        path.push(self.segment()?);
                    }
                    Ok(Selector::Module { lang, path })
                } else if &*n == "v2" {
                    Ok(Selector::V2 { span })
                } else {
                    Ok(Selector::Label { label: Name::new(n, span) })
                }
            }
            DK::Str(_) => Err(Diag::new("a block label can't be a string", span).hint(LABEL_HINT)),
            other => Err(Diag::new(format!("expected a selector (@py, a block label, or py.module), found {}", describe(&other)), span)),
        }
    }

    fn segment(&mut self) -> Result<Name, Diag> {
        let t = self.bump();
        match t.kind.clone() {
            DK::Name(n) | DK::Str(n) => Ok(Name::new(n, t.span)),
            other => Err(Diag::new(format!("expected a name after `.`, found {}", describe(&other)), t.span)),
        }
    }

    fn name(&mut self, what: &str) -> Result<Name, Diag> {
        let t = self.bump();
        match t.kind.clone() {
            DK::Name(n) => Ok(Name::new(n, t.span)),
            other => Err(Diag::new(format!("expected {what}, found {}", describe(&other)), t.span)),
        }
    }
}

/// Whether `ascii`, an input method's form typed with the method off (3.2), fits where the
/// directive's lexer stopped, after `toks`, `depth` braces deep, with `after` left on the line:
/// so typing it gets the directive further. In the list a name's `_`, a quote, `,`, `*` and `}`;
/// the list's `{` right after the word; after an @import's list `from`'s `@`, then a selector's
/// `.`, quotes and `_`; `#` where the comment leaves the directive whole.
fn fits(ascii: char, toks: &[DTok], depth: i32, after: &str) -> bool {
    let last = toks.last().map(|t| &t.kind);
    let from = |k: &DK| matches!(k, DK::Kw(w) if &**w == "from");
    let selector = depth <= 0 && toks.first().is_some_and(|t| t.kind == DK::Import) && toks.iter().any(|t| from(&t.kind));
    let segment = matches!(last, Some(DK::Name(_) | DK::At(_) | DK::Str(_)));
    match ascii {
        '{' => toks.len() == 1,
        ',' | '*' | '}' => depth > 0,
        '"' | '\'' | '_' => depth > 0 || selector && (last.is_some_and(from) || last == Some(&DK::Dot)),
        '@' => selector && last.is_some_and(from),
        '.' => selector && segment,
        // In a list that goes on below, after an @export's list, after an @import's selector.
        '#' if depth > 0 => !after.contains(['}', '\u{FF5D}']),
        '#' => toks.iter().any(|t| t.kind == DK::RBrace) && (toks.first().is_some_and(|t| t.kind == DK::Export) || selector && segment),
        _ => false,
    }
}

/// JavaScript's `export { show as pyshow }`: the hint, with the item as each list writes it: a
/// quoted name keeps its quotes (`"$x"`), and a V2 keyword, bare in @export, is quoted in @import
/// (DOCS 6.1, 6.2).
fn rename_hint(item: Option<&DK>) -> String {
    let (exported, imported) = match item {
        Some(DK::Name(n)) => (n.to_string(), n.to_string()),
        Some(DK::Kw(n)) => (n.to_string(), format!("{:?}", &**n)),
        Some(DK::Str(n)) => (format!("{:?}", &**n), format!("{:?}", &**n)),
        _ => ("name".to_string(), "name".to_string()),
    };
    format!("@export can't rename: export {exported} and rename it where it is imported, as in @import {{ {imported} as other }} from …")
}

fn push_export(exports: &mut Exports, text: Rc<str>, span: Span) -> Result<(), Diag> {
    if let Some(prev) = exports.items.iter().find(|e| e.text == text) {
        return Err(Diag::new(format!("'{text}' is exported twice"), span).note("first exported here", prev.span));
    }
    exports.items.push(ExportItem { text, span });
    Ok(())
}

/// A one-line string literal at the start of `rest` (which begins with its quote), with the
/// escape rules of 3.5; `raw` strings have none. Returns (decoded text, byte length), or an
/// error message and the byte offset it points at.
fn dir_string(rest: &str, raw: bool) -> Result<(String, usize), (String, usize)> {
    let quote = rest.chars().next().unwrap_or('"');
    let mut out = String::new();
    let mut chars = rest.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        if c == quote {
            return Ok((out, i + 1));
        }
        if c != '\\' || raw {
            out.push(c);
            continue;
        }
        let bad = |what: String| Err((format!("unknown escape {what}"), i));
        match chars.next() {
            Some((_, 'n')) => out.push('\n'),
            Some((_, 't')) => out.push('\t'),
            Some((_, 'r')) => out.push('\r'),
            Some((_, '0')) => out.push('\0'),
            Some((_, e @ ('\\' | '"' | '\'' | '$'))) => out.push(e),
            Some((j, 'x')) => {
                let h = rest.get(j + 1..j + 3).unwrap_or("");
                match u8::from_str_radix(h, 16) {
                    Ok(b) if h.len() == 2 => {
                        out.push(char::from(b));
                        chars.next();
                        chars.next();
                    }
                    _ => return Err(("\\x takes exactly 2 hex digits".into(), i)),
                }
            }
            Some((j, 'u')) => {
                let tail = &rest[j + 1..];
                let close = tail.find('}');
                let hex = match (tail.starts_with('{'), close) {
                    (true, Some(k)) => &tail[1..k],
                    _ => "",
                };
                match u32::from_str_radix(hex, 16).ok().filter(|_| (1..=6).contains(&hex.len())).and_then(char::from_u32) {
                    Some(ch) => {
                        out.push(ch);
                        for _ in 0..hex.len() + 2 {
                            chars.next();
                        }
                    }
                    None => return Err(("\\u{…} must name a Unicode scalar value in 1 to 6 hex digits".into(), i)),
                }
            }
            Some((_, e)) => return bad(format!("\\{e}")),
            None => return Err(("unterminated string".into(), 0)),
        }
    }
    Err(("unterminated string".into(), 0))
}

fn describe(k: &DK) -> String {
    match k {
        DK::Export => "`@export`".into(),
        DK::Import => "`@import`".into(),
        DK::LBrace => "`{`".into(),
        DK::RBrace => "`}`".into(),
        DK::Comma => "`,`".into(),
        DK::Star => "`*`".into(),
        DK::Dot => "`.`".into(),
        DK::At(t) => format!("a selector tag `@{t}`"),
        DK::Name(n) => format!("name `{n}`"),
        DK::Kw(k) => format!("keyword `{k}`"),
        DK::Str(_) => "string".into(),
        DK::End => "end of line".into(),
    }
}

/// Compiled blocks' directive rules (4.5, 15.2), checked statically so `v2 check` finds them: a
/// block without `@export` runs as a program and can't import; imports name functions, never
/// `*`; every local name must be an identifier of the block's language, because V2 generates a
/// function with that name (15.4).
fn compiled_directives(d: &Directives, tag: &str) -> Result<(), Diag> {
    let lang = match tag {
        "c" => "C",
        "cpp" | "cxx" => "C++",
        _ => "Rust",
    };
    for import in &d.imports {
        if d.exports.is_none() {
            return Err(Diag::new("a compiled block without @export runs as a program and can't @import", import.span)
                .hint("export a function from this block, or call the other language from V2 code"));
        }
        match &import.items {
            ImportItems::Names(items) => {
                for item in items {
                    let (local, span) = item.alias.as_ref().map_or((&item.name, item.span), |a| (&a.text, a.span));
                    if !native_ident(lang, local) {
                        return Err(Diag::new(format!("'{local}' is not a valid {lang} identifier — rename it with 'as'"), span));
                    }
                }
            }
            ImportItems::Module(_) => {}
        }
    }
    if let Some(ex) = &d.exports {
        for item in &ex.items {
            if !native_ident(lang, &item.text) {
                return Err(Diag::new(format!("'{}' is not a valid {lang} identifier", item.text), item.span)
                    .hint("rename the function, or export a wrapper with a plain name"));
            }
        }
    }
    Ok(())
}

/// Indentation in words: `1 tab`, `4 spaces`, `1 tab and 2 spaces`.
fn indent_words(lead: &str) -> String {
    let count = |c: char, one: &str, many: &str| match lead.chars().filter(|&x| x == c).count() {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    };
    let parts: Vec<String> = [count('\t', "tab", "tabs"), count(' ', "space", "spaces")].into_iter().flatten().collect();
    if parts.is_empty() { "no indentation".to_string() } else { parts.join(" and ") }
}

/// An ASCII identifier that is not a keyword of `lang` (C11, C++17, Rust 2021 strict and
/// reserved keywords).
fn native_ident(lang: &str, name: &str) -> bool {
    const C: &[&str] = &[
        "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else", "enum", "extern",
        "float", "for", "goto", "if", "inline", "int", "long", "register", "restrict", "return", "short", "signed",
        "sizeof", "static", "struct", "switch", "typedef", "union", "unsigned", "void", "volatile", "while", "_Alignas",
        "_Alignof", "_Atomic", "_Bool", "_Complex", "_Generic", "_Imaginary", "_Noreturn", "_Static_assert",
        "_Thread_local",
    ];
    const CPP: &[&str] = &[
        "alignas", "alignof", "and", "and_eq", "asm", "bitand", "bitor", "bool", "catch", "char16_t", "char32_t",
        "char8_t", "class", "compl", "concept", "consteval", "constexpr", "constinit", "const_cast", "co_await",
        "co_return", "co_yield", "decltype", "delete", "dynamic_cast", "explicit", "export", "false", "friend",
        "mutable", "namespace", "new", "noexcept", "not", "not_eq", "nullptr", "operator", "or", "or_eq", "private",
        "protected", "public", "reinterpret_cast", "requires", "static_assert", "static_cast", "template", "this",
        "thread_local", "throw", "true", "try", "typeid", "typename", "using", "virtual", "wchar_t", "xor", "xor_eq",
    ];
    const RUST: &[&str] = &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn",
        "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self",
        "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "abstract", "become",
        "box", "do", "final", "macro", "override", "priv", "try", "typeof", "unsized", "virtual", "yield",
    ];
    let mut chars = name.chars();
    let ident = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    let keyword = match lang {
        "C" => C.contains(&name),
        "C++" => C.contains(&name) || CPP.contains(&name),
        _ => RUST.contains(&name),
    };
    ident && !keyword && name != "_"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::lexer::lex;
    use crate::syntax::token::Tok;
    use std::path::PathBuf;

    /// Lex `src`, take its first block (tag, raw body) and process it.
    fn run(src: &str) -> Result<(Processed, SourceMap), String> {
        let mut sm = SourceMap::new();
        let id = sm.add(PathBuf::from("t.v2"), "t.v2".into(), src);
        let toks = lex(&sm, id).map_err(|d| d.message)?;
        let tag = toks
            .iter()
            .find_map(|t| match &t.tok {
                Tok::BlockHead { tag, .. } => Some(tag.clone()),
                _ => None,
            })
            .expect("a block head");
        let raw = toks
            .iter()
            .find_map(|t| match &t.tok {
                Tok::BlockBody(b) => Some(b.clone()),
                _ => None,
            })
            .expect("a block body");
        let p = process(&sm, &raw, &tag).map_err(|d| d.message)?;
        Ok((p, sm))
    }

    fn err(src: &str) -> String {
        match run(src) {
            Ok(_) => panic!("expected an error for {src:?}"),
            Err(e) => e,
        }
    }

    #[test]
    fn docs_example_from_4_4() {
        let src = "\n\n\n\n\n\n\n\n\n@py stats {\n    def spread(xs):\n        return max(xs) - min(xs)\n    @export { spread }\n}\n";
        let (p, _) = run(src).unwrap();
        assert_eq!(&*p.body, "def spread(xs):\n    return max(xs) - min(xs)\n\n");
        assert_eq!(p.loc.body_first_line, 11);
        assert_eq!(p.loc.body_col, 5);
        assert_eq!(p.loc.dedent_width, 4);
        assert_eq!(p.loc.line_count, 3);
        assert!(p.loc.kept.is_empty());
        let ex = p.directives.exports.unwrap();
        assert_eq!(ex.items.len(), 1);
        assert_eq!(&*ex.items[0].text, "spread");
    }

    #[test]
    fn one_line_and_header_line_bodies() {
        let (p, _) = run("let x = @py(data) { sum(data) / len(data) }").unwrap();
        assert_eq!(&*p.body, "sum(data) / len(data)\n");
        assert_eq!((p.loc.body_first_line, p.loc.body_col), (1, 21));
        assert!(p.directives.exports.is_none());
        // A multi-line block's code starts on the line after its `{` (3.7); blanks may follow it.
        assert_eq!(run("@py { x = 1\n    y = 2\n}").err().as_deref(), Some("a multi-line block starts on the line after its {"));
        assert_eq!(&*run("@py {   \r\n    y = 2\n}").unwrap().0.body, "y = 2\n");
        let (p, _) = run("@py {}").unwrap();
        assert_eq!(&*p.body, "");
    }

    #[test]
    fn fence_bodies() {
        let (p, _) = run("@py report ```\n    template = \"{ unbalanced\"\n    @export { render }\n```\n").unwrap();
        assert_eq!(&*p.body, "template = \"{ unbalanced\"\n\n");
        assert_eq!(p.loc.body_first_line, 2);
        assert_eq!(p.directives.exports.unwrap().items[0].text.as_ref(), "render");
    }

    #[test]
    fn directives_are_exact_tokens_and_escapes_lose_one_at() {
        let src = "@py {\n    @exporter\n    @export_json\n    def f(): pass\n    s = '''\n@@export { x }\n@@import y\n'''\n}";
        let (p, _) = run(src).unwrap();
        assert!(p.directives.exports.is_none(), "decorators are foreign code");
        assert_eq!(&*p.body, "@exporter\n@export_json\ndef f(): pass\ns = '''\n@export { x }\n@import y\n'''\n");
        assert_eq!(p.loc.kept.as_ref(), &[5, 6, 7]);
    }

    #[test]
    fn multi_line_export_lists_keep_line_numbers() {
        let src = "@py {\n    def a(): pass\n    @export {\n        a,\n        b, # comment\n    }  // trailing\n    def b(): pass\n}";
        let (p, _) = run(src).unwrap();
        assert_eq!(&*p.body, "def a(): pass\n\n\n\n\ndef b(): pass\n");
        let ex = p.directives.exports.unwrap();
        let names: Vec<&str> = ex.items.iter().map(|e| &*e.text).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn imports_inside_blocks() {
        let src = "@js {\n  @import { square, \"odd-name\" as odd } from @py.calc\n  @import { mean } from py.statistics\n  @import { log } from v2\n  @import { format } from js.\"date-fns\"\n}";
        let (p, _) = run(src).unwrap();
        let imps = &p.directives.imports;
        assert_eq!(imps.len(), 4);
        assert!(matches!(&imps[0].selector, Selector::Exact { tag, label } if &*tag.text == "py" && &*label.text == "calc"));
        assert!(matches!(&imps[0].items, ImportItems::Names(v) if v.len() == 2 && v[1].alias.as_ref().unwrap().text.as_ref() == "odd"));
        assert!(matches!(&imps[1].selector, Selector::Module { lang, path } if &*lang.text == "py" && path.len() == 1));
        assert!(matches!(&imps[2].selector, Selector::V2 { .. }));
        assert!(matches!(&imps[3].selector, Selector::Module { path, .. } if &*path[0].text == "date-fns"), "quoted package segments stay");
        // Every line was a directive, so no line has text: the body is "" (4.4).
        assert_eq!(&*p.body, "");
        assert_eq!(p.loc.line_count, 4);
    }

    #[test]
    fn directive_errors() {
        assert_eq!(err("@py {\n @export {}\n}"), "an @export list can't be empty");
        assert_eq!(err("@py {\n @export { f, f }\n}"), "'f' is exported twice");
        // In a brace block, an unclosed `@export {` also leaves the block open; a fence shows the
        // directive's own error.
        assert_eq!(err("@py {\n @export { f\n}"), "@py block is never closed");
        assert_eq!(err("@py ```\n @export { f\n```\n"), "unterminated @export list");
        assert_eq!(err("@py {\n @export { f } g\n}"), "unexpected text after the directive");
        assert_eq!(err("@py {\n @import calc as c\n}"), "the whole-module form of @import is only valid in V2 code");
        assert_eq!(err("@py {\n @import { a }\n}"), "expected `from` after the @import list");
    }

    /// The exported names of the first block in `src`.
    fn exported(src: &str) -> Vec<String> {
        let (p, _) = run(src).unwrap();
        p.directives.exports.map(|e| e.items.iter().map(|i| i.text.to_string()).collect()).unwrap_or_default()
    }

    #[test]
    fn review_0_4_regressions() {
        // Quoted directive items follow the V2 string rules (3.5).
        assert_eq!(exported("@js {\n  @export { \"n\\n\", \"nn\" }\n}"), vec!["n\n", "nn"]);
        assert_eq!(err("@js {\n  @export { \"a\\x41\", \"aA\" }\n}"), "'aA' is exported twice");
        assert_eq!(err("@js {\n  @export { \"a\\d\" }\n}"), "unknown escape \\d");
        assert_eq!(exported("@js {\n  @export { \"\\u{24}helper\" }\n}"), vec!["$helper"]);
        assert_eq!(exported("@js {\n  @export { r\"a\\b\" }\n}"), vec!["a\\b"]);
        assert_eq!(err("@js {\n  @export { b\"x\" }\n}"), "unknown string prefix `b` in a directive");
        // Tags start with a letter or `_`.
        assert_eq!(err("@js {\n  @import { a } from @1py\n}"), "expected an engine tag after @");
        // Keywords: bare in @export lists only; quoted elsewhere; after `.` they are names.
        assert_eq!(exported("@py {\n  @export { for }\n}"), vec!["for"]);
        assert_eq!(err("@js {\n  @import { from } from @py\n}"), "expected a name in the @import list, found keyword `from`");
        assert_eq!(err("@js {\n  @import { a } from for\n}"), "expected a selector (@py, a block label, or py.module), found keyword `for`");
        assert_eq!(err("@js {\n  @import { a as for } from @py\n}"), "expected a name after `as`, found keyword `for`");
        assert!(run("@js {\n  @import { a } from py.import\n}").is_ok());
        assert!(run("@js {\n  @import { \"from\" } from @py\n}").is_ok());
        // The first @export's own span is recorded.
        let (p, sm) = run("@py {\n    def f(): pass\n    @export { f }\n}").unwrap();
        let at = p.directives.exports.unwrap().at.unwrap();
        assert_eq!(sm.line_col(at.file, at.lo), (3, 5));
    }

    /// The diagnostic for the first block in `src`.
    fn diag(src: &str) -> Diag {
        let mut sm = SourceMap::new();
        let id = sm.add(PathBuf::from("t.v2"), "t.v2".into(), src);
        let toks = lex(&sm, id).unwrap();
        let (tag, raw) = toks
            .iter()
            .find_map(|t| match &t.tok {
                Tok::BlockHead { tag, .. } => Some(tag.clone()),
                _ => None,
            })
            .zip(toks.iter().find_map(|t| match &t.tok {
                Tok::BlockBody(b) => Some(b.clone()),
                _ => None,
            }))
            .expect("a block");
        match process(&sm, &raw, &tag) {
            Ok(_) => panic!("expected an error for {src:?}"),
            Err(d) => d,
        }
    }

    #[test]
    fn a_whole_module_is_imported_by_the_blocks_own_language() {
        // 4.4, 4.5, 13.7: inside a block only `@import { names } from SEL`; the hint names the language's
        // own import when the module is the block's language.
        for (src, hint) in [
            ("@py {\n  @import py.numpy as np\n}", "use import numpy as np inside @py blocks"),
            ("@python {\n  @import py.os.path as p\n}", "use import os.path as p inside @py blocks"),
            ("@py {\n  @import py.os as os\n}", "use import os inside @py blocks"),
            ("@js {\n  @import js.\"date-fns\" as dfns\n}", "use const dfns = require(\"date-fns\") inside @js blocks"),
            ("@ts {\n  @import ts.lodash as _\n}", "use const _ = require(\"lodash\") inside @ts blocks"),
            ("@js {\n  @import py.numpy as np\n}", "import names instead: @import { … } from …"),
            ("@c {\n  @import py.numpy as np\n}", "import names instead: @import { … } from …"),
            ("@py {\n  @import calc as c\n}", "import names instead: @import { … } from …"),
            ("@py {\n  @import py.numpy\n}", "import names instead: @import { … } from …"),
        ] {
            let d = diag(src);
            assert_eq!((d.message.as_str(), d.hint.as_deref()), ("the whole-module form of @import is only valid in V2 code", Some(hint)), "{src}");
            assert_eq!((d.span.line, d.span.col), (2, 3), "at the @import: {src}");
        }
    }

    #[test]
    fn a_directive_line_that_doesnt_parse_says_how_to_escape_it() {
        // 4.4: a CSS @import in a template literal or a docstring is read as V2's line; the
        // error says how to write the block's own.
        let hint = "a line that starts with @import is V2's; for code of the block's own language that starts with @import (a CSS @import in a string), write @@import";
        for src in ["@js {\n  const css = `\n@import url(\"theme.css\");\n`\n}", "@py {\n  s = '''\n@import \"theme.css\";\n'''\n}"] {
            assert_eq!(diag(src).hint.as_deref(), Some(hint), "{src}");
        }
        assert_eq!(
            diag("@py {\n  def show(): pass\n  @export { show as pyshow }\n}").hint.as_deref(),
            Some("@export can't rename: export show and rename it where it is imported, as in @import { show as other } from …")
        );
        // The line escaped is the block's own.
        let (p, _) = run("@js {\n  const css = `\n@@import url(\"theme.css\");\n`\n}").unwrap();
        assert!(p.body.contains("\n@import url(\"theme.css\");\n"), "{:?}", p.body);
        // Round 20: an input method's comma or space in a list is its slip, with the lexer's hint
        // (3.2); a backtick keeps this one.
        let both = |src: &str| {
            let d = diag(src);
            (d.message, d.hint.unwrap_or_default())
        };
        assert_eq!(
            both("@py {\n    @export { f\u{FF0C} g }\n    def f(): pass\n}"),
            ("unexpected character '\u{FF0C}' (a full-width ',' from an input method) in a directive".into(), "type , with the input method off".into())
        );
        assert_eq!(both("@py {\n    @export { f\u{3000}}\n    def f(): pass\n}"), ("unexpected character U+3000 (an ideographic space) in a directive".into(), "type a plain space instead".into()));
        assert_eq!(both("@py {\n    @export { f\u{200B}}\n    def f(): pass\n}").1, "delete it");
        assert_eq!(both("@py {\n    @export { \u{201C}f\u{201D} }\n    def f(): pass\n}").1, "use a straight quote: \" or '");
        assert_eq!(both("@js {\n  @import { `a` } from @py\n}").1, hint);
        // Any other character, or one outside the list, keeps it: a docstring's line is the
        // block's own text, and `｀` or `～` typed as ASCII is no better.
        let export = "a line that starts with @export is V2's; for code of the block's own language that starts with @export, write @@export";
        let doc = "@py {\n    def f():\n        \"\"\"Shown.\n        @export \u{2013} marks it\n        \"\"\"\n}";
        assert_eq!(both(doc), ("unexpected character '\u{2013}' (a dash) in a directive".into(), export.into()));
        for c in ['\u{2013}', '\u{D7}', '\u{F7}', '\u{FF40}', '\u{FF5E}'] {
            assert_eq!(both(&format!("@py {{\n    @export {{ f{c} }}\n    def f(): pass\n}}")).1, export, "{c}");
        }
        // Round 20d: a slip after the list is one too (`from　@py`); an input method's form only
        // where its ASCII fits, and `；` in a list gets the comma. Each hint, followed, parses.
        let in_js = |line: &str| format!("@js {{\n  {line}\n}}");
        let in_py = |line: &str| format!("@py {{\n    def f(): pass\n    def g(): pass\n    def _g(): pass\n    {line}\n}}");
        let in_fence = |line: &str| format!("@py ```\n    def f(): pass\n    {line}\n```\n");
        let space = "type a plain space instead";
        let input = |c: char| format!("type {c} with the input method off");
        for (src, said, followed) in [
            (in_js("@import { f }\u{3000}from @py"), space.to_string(), in_js("@import { f } from @py")),
            (in_js("@import { f } from\u{3000}@py"), space.to_string(), in_js("@import { f } from @py")),
            (in_js("@import { f } from\u{A0}@py"), space.to_string(), in_js("@import { f } from @py")),
            (in_js("@import { f } from \u{FF20}py"), input('@'), in_js("@import { f } from @py")),
            (in_js("@import { f } from @py\u{200B}"), "delete it".to_string(), in_js("@import { f } from @py")),
            (in_js("@import { join } from py.\u{201C}os.path\u{201D}"), "use a straight quote: \" or '".to_string(), in_js("@import { join } from py.\"os.path\"")),
            (in_js("@import { join } from py\u{FF0E}os.path"), input('.'), in_js("@import { join } from py.os.path")),
            (in_js("@import { f } from @py \u{FF03} the function"), input('#'), in_js("@import { f } from @py # the function")),
            (in_py("@export { f } \u{FF03} x"), input('#'), in_py("@export { f } # x")),
            (in_py("@export {\n        f, \u{FF03} the function\n        g\n    }"), input('#'), in_py("@export {\n        f, # the function\n        g\n    }")),
            (in_py("@export { f\u{FF1B} g }"), "separate names with a comma: ,".to_string(), in_py("@export { f, g }")),
            (in_py("@export { f, \u{FF3F}g }"), input('_'), in_py("@export { f, _g }")),
            (in_py("@export { f\u{FF0C} g }"), input(','), in_py("@export { f, g }")),
            (in_py("@export {f\u{FF0C} g}"), input(','), in_py("@export {f, g}")),
            (in_fence("@export { \u{FF02}f\" }"), input('"'), in_fence("@export { \"f\" }")),
            (in_py("@export { \u{FF0A} }"), input('*'), in_py("@export { * }")),
            // A brace block's own braces are ASCII ones, so the list's two go together; a fence
            // doesn't count them.
            (in_py("@export \u{FF5B}f\u{FF0C} g\u{FF5D}"), "type { and } with the input method off".to_string(), in_py("@export {f\u{FF0C} g}")),
            (in_fence("@export \u{FF5B} f }"), input('{'), in_fence("@export { f }")),
            (in_fence("@export { f \u{FF5D}"), input('}'), in_fence("@export { f }")),
        ] {
            assert_eq!(both(&src).1, said, "{src:?}");
            if let Err(e) = run(&followed) {
                assert_eq!(said, "type { and } with the input method off", "{followed:?}: {e}");
            }
        }
        // A form whose ASCII doesn't fit where it stands (a `(`, a `.` or a digit in the list, a
        // `#` that would hide the `}`), and a slip before the `{`: the @@export hint.
        for src in [
            in_py("@export { f\u{FF08}\u{FF09} }"),
            in_py("@export { f\u{FF0E}g }"),
            in_py("@export { f, \u{FF11} }"),
            in_py("@export { f \u{FF03} x }"),
            in_py("@export { f } \u{FF20}py"),
            in_py("@export { f } \u{FF1B}"),
            in_py("@export \u{3000}{ f }"),
        ] {
            assert_eq!(both(&src).1, export, "{src:?}");
        }
        assert_eq!(both(&in_js("@import { f } \u{FF20}py")).1, hint);
        assert_eq!(both(&in_js("@import { f } from py.\u{FF03}")).1, hint);
    }

    #[test]
    fn names_that_arent_v2_names_are_quoted_in_hints() {
        // 4.4, round 17: the rename hint keeps the item as each list writes it, so its @import
        // works when copied, and a bare `$x` in a list gets the list's quoted form.
        let hint = |src: &str| diag(src).hint.unwrap_or_else(|| panic!("no hint for {src:?}"));
        let rename = |exported: &str, imported: &str| format!("@export can't rename: export {exported} and rename it where it is imported, as in @import {{ {imported} as other }} from …");
        assert_eq!(hint("@js d {\n  const $x = 1\n  @export { \"$x\" as dx }\n}"), rename("\"$x\"", "\"$x\""));
        assert_eq!(hint("@js d {\n  @export { from as f }\n}"), rename("from", "\"from\""));
        assert_eq!(hint("@js d {\n  @export { show as $s }\n}"), rename("show", "show"));
        assert_eq!(hint("@js d {\n  @export { $x }\n}"), "write a name that isn't a V2 name in quotes: @export { \"$x\" }");
        // A block's own list gets the alias form, which Python needs (it binds an imported name
        // only as a Python name): the alias written, else the name without its `$` when that is
        // a V2 and a Python name, else `other`. After `as` goes a plain name.
        let quoted = |item: &str| format!("write a name that isn't a V2 name in quotes, with a plain name after as: @import {{ {item} }} from …");
        for (tag, list, item) in [
            ("py", "$x", "\"$x\" as x"),
            ("js", "$x", "\"$x\" as x"),
            ("ts", "a, $el", "\"$el\" as el"),
            ("py", "$x as dx", "\"$x\" as dx"),
            ("py", "$class", "\"$class\" as other"),
            ("py", "$None", "\"$None\" as other"),
            ("js", "$lambda", "\"$lambda\" as other"),
            ("py", "$from", "\"$from\" as other"),
            // An alias every adapter takes (round 17's second review): not js.js's words or a dunder.
            ("js", "$this", "\"$this\" as other"),
            ("ts", "$delete", "\"$delete\" as other"),
            ("py", "$__x__", "\"$__x__\" as other"),
            ("py", "$", "\"$\" as other"),
            ("py", "$$", "\"$$\" as other"),
        ] {
            assert_eq!(hint(&format!("@{tag} u {{\n  @import {{ {list} }} from d\n}}")), quoted(item), "@{tag} {list}");
        }
        assert_eq!(hint("@py u {\n  @import { show as $s } from d\n}"), "after as goes a plain name, without $: as s");
        assert_eq!(hint("@py u {\n  @import { show as $class } from d\n}"), "after as goes a plain name, without $: as other");
    }

    #[test]
    fn labels_in_selectors_are_names() {
        // 3.7, 4.1: a string where a block label would be is an error; quoted package segments stay.
        for src in ["@js {\n  @import { f } from \"odd name\"\n}", "@js {\n  @import { f } from @py.\"odd name\"\n}"] {
            let d = diag(src);
            assert_eq!((d.message.as_str(), d.hint.as_deref()), ("a block label can't be a string", Some(LABEL_HINT)), "{src}");
        }
        assert!(run("@js {\n  @import { f } from py.\"my_mod\"\n}").is_ok());
        assert!(run("@js {\n  @import { f } from @py.calc\n}").is_ok());
    }

    #[test]
    fn compiled_directive_rules_are_static() {
        // 4.5, 15.2: v2 check finds these; they are no longer BuildErrors at load.
        let err = |src: &str| run(src).err().unwrap_or_default();
        assert_eq!(err("@c {\n    @import { mean } from py.statistics\n    int main(void) { return 0; }\n}"),
            "a compiled block without @export runs as a program and can't @import");
        assert_eq!(err("@rust {\n    @import { * } from stats\n    fn f() {}\n    @export { f }\n}"), "@import needs a list of names");
        assert_eq!(err("@cpp {\n    @import { delete } from stats\n    void f() {}\n    @export { f }\n}"),
            "'delete' is not a valid C++ identifier — rename it with 'as'");
        assert_eq!(err("@rust {\n    @import { mean as fn } from stats\n    fn f() {}\n    @export { f }\n}"),
            "'fn' is not a valid Rust identifier — rename it with 'as'");
        assert_eq!(err("@c {\n    @import { café } from stats\n    void f(void) {}\n    @export { f }\n}"),
            "'café' is not a valid C identifier — rename it with 'as'");
        assert_eq!(err("@c {\n    int größe(void) { return 1; }\n    @export { größe }\n}"), "'größe' is not a valid C identifier");
        // Fine: an alias fixes the name; delete is no keyword in C; py blocks take any name.
        assert!(run("@cpp {\n    @import { delete as remove } from stats\n    void f() {}\n    @export { f }\n}").is_ok());
        assert!(run("@c {\n    @import { delete } from stats\n    void f(void) {}\n    @export { f }\n}").is_ok());
        assert_eq!(err("@py {\n    @import { * } from stats\n}"), "@import needs a list of names");
    }

    #[test]
    fn compiled_exports_are_names() {
        // 4.4, 4.5: a quoted item in a compiled block is an error; in py/js/ts blocks it is a name.
        for tag in ["c", "cpp", "cxx", "rust", "rs"] {
            let d = diag(&format!("@{tag} {{\n    @export {{ f, \"int g(int a)\" }}\n}}"));
            assert_eq!(d.message, "compiled blocks export functions by name", "{tag}");
            assert_eq!(d.hint.as_deref(), Some("export a wrapper function with a plain signature"));
            assert_eq!((d.span.line, d.span.col), (2, 18), "at the string");
        }
        assert_eq!(exported("@c {\n    @export { f, g }\n}"), vec!["f", "g"]);
        assert_eq!(err("@c {\n    @export { f, f }\n}"), "'f' is exported twice");
        for tag in ["py", "js", "ts"] {
            assert_eq!(exported(&format!("@{tag} {{\n    @export {{ \"odd-name\" }}\n}}")), vec!["odd-name"], "{tag}");
        }
    }

    #[test]
    fn mixed_indentation_is_an_error_but_deeper_tabs_are_fine() {
        assert_eq!(err("@py {\n    a = 1\n\tb = 2\n}"), "block indentation mixes tabs and spaces");
        // A no-break space pasted into the indentation, reported at its own line (4.4).
        assert_eq!(err("@py {\n    a = 1\n\u{a0}   b = 2\n}"), "unexpected character U+00A0 (a no-break space) in the block's indentation");
        assert!(run("@py {\n\ta = 1\n\tb = 2\n}").is_ok(), "tabs alone are fine");
        assert!(run("@py {\na = 1\n\x0c\nb = 2\n}").is_ok(), "Python takes a form feed");
        assert_eq!(err("@py {\n    a = 1\n\u{a0}\n    b = 2\n}"), "unexpected character U+00A0 (a no-break space) in the block's indentation");
        let (p, _) = run("@py {\n\tif x:\n\t\ty = 1\n}").unwrap();
        assert_eq!(&*p.body, "if x:\n\ty = 1\n");
        // NBSP is content, not indentation, for a language that takes it, and doesn't panic.
        assert!(run("@js {\n  a\n  \u{a0}b\n}").is_ok());
        assert_eq!(err("@py {\n  a\n  \u{a0}b\n}"), "unexpected character U+00A0 (a no-break space) in the block's indentation");
    }

    #[test]
    fn unit_loc_columns() {
        let (p, _) = run("@py {\n    x = 1\n    y = 2\n}").unwrap();
        assert_eq!(p.loc.col(1, 1), Some(5));
        assert_eq!(p.loc.col(2, 3), Some(7));
        // A worker's column can be anything: past every line is no column, not an overflow.
        assert_eq!(p.loc.col(2, u32::MAX), None);
        assert_eq!(p.loc.col(1, u32::MAX), None);
        // An `@@export` escape loses one `@`; columns after it move back by one (4.4).
        let (p, _) = run("@py {\n    x = 1\n    @@export (1/0)\n}").unwrap();
        assert_eq!(&*p.body, "x = 1\n@export (1/0)\n");
        assert_eq!((p.loc.col(2, 1), p.loc.col(2, 2)), (Some(6), Some(7)), "the kept `@`, then `e`");
    }
}
