//! Scanning raw block bodies (INTERNALS 3.7, 3.8).
//!
//! A brace-form body runs from just after its `{` to the matching `}`. To find that `}`, the
//! scanner skips the foreign language's strings, comments and templates, using a profile picked
//! from the tag's text alone (never from config). It also records, for every body line, whether
//! the line starts in code or inside a multi-line string, raw string, template or comment; dedent
//! (4.4) leaves the latter alone.

/// The scanning profile of a tag (3.8).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    Python,
    CLike(Dialect),
    /// Every custom engine: braces are counted, nothing is skipped.
    Plain,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialect {
    Js,
    C,
    Cpp,
    Rust,
}

pub fn profile_for(tag: &str) -> Profile {
    match tag {
        "py" | "python" => Profile::Python,
        "js" | "javascript" | "ts" | "typescript" => Profile::CLike(Dialect::Js),
        "c" => Profile::CLike(Dialect::C),
        "cpp" | "cxx" => Profile::CLike(Dialect::Cpp),
        "rust" | "rs" => Profile::CLike(Dialect::Rust),
        _ => Profile::Plain,
    }
}

/// The canonical form of a built-in tag or alias (`python` → `py`); other tags as written.
/// Messages always print tags this way (2.5, 7.3).
pub fn canonical_tag(tag: &str) -> &str {
    match tag {
        "python" => "py",
        "javascript" => "js",
        "typescript" => "ts",
        "cxx" => "cpp",
        "rs" => "rust",
        other => other,
    }
}

/// A brace body that closed.
#[derive(Debug, PartialEq)]
pub struct Scanned {
    /// Byte offset (into the scanned text) of the closing `}`.
    pub close: usize,
    /// For each body line, whether it starts in code (index 0 = the line holding the `{`).
    pub starts_in_code: Vec<bool>,
}

/// A brace body that reached the end of the text.
#[derive(Debug, PartialEq)]
pub struct Unclosed {
    /// Offset of the innermost `{` inside the body that is still open, if any.
    pub open_brace: Option<usize>,
    /// Offset and name of a foreign string, comment or template that never closed, if any.
    pub open_construct: Option<(usize, &'static str)>,
}

/// What the scanner is inside of, below the code level.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Inner {
    LineComment,
    /// Rust block comments nest; the depth counts open `/*`.
    BlockComment { depth: u32, start: usize },
    Str { quote: char, triple: bool, multiline: bool, start: usize },
    Char { start: usize },
    RawCpp { delim_start: usize, delim_end: usize, start: usize },
    RawRust { hashes: usize, start: usize },
}

impl Inner {
    fn start_and_name(self) -> Option<(usize, &'static str)> {
        match self {
            Inner::LineComment => None,
            Inner::BlockComment { start, .. } => Some((start, "block comment")),
            Inner::Str { start, .. } => Some((start, "string")),
            Inner::Char { start } => Some((start, "character literal")),
            Inner::RawCpp { start, .. } | Inner::RawRust { start, .. } => Some((start, "raw string")),
        }
    }
}

/// A code context; JS templates push a template frame, and each `${` in it a new code frame.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Frame {
    Code { depth: u32 },
    Template { start: usize },
}

/// Whether a JS `/` after `before` starts a regex literal rather than dividing: what comes before
/// it can't end an operand, which is JS's own rule (3.8).
fn regex_may_start(before: &str) -> bool {
    let mut before = before.trim_end();
    // Comments are no tokens: look before them.
    loop {
        if let Some(open) = before.strip_suffix("*/").and_then(|b| b.rfind("/*")) {
            before = before[..open].trim_end();
            continue;
        }
        let line = before.rfind('\n').map_or(0, |n| n + 1);
        match before[line..].find("//") {
            Some(c) => before = before[..line + c].trim_end(),
            None => break,
        }
    }
    let Some(last) = before.chars().last() else { return true };
    if before.ends_with("++") || before.ends_with("--") {
        return false; // postfix: an operand ends there
    }
    match last {
        // TypeScript's `x!` ends an operand; `return!` is an operator after a keyword.
        '!' => !ends_operand(&before[..before.len() - 1]),
        c if is_word(c) => !ends_operand(before),
        c => !matches!(c, ')' | ']' | '.' | '"' | '\'' | '`'),
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Whether `s` ends with a name, a literal, `)` or `]`: not with a keyword that takes an operand
/// next, unless it is read as a property (`o.in`).
fn ends_operand(s: &str) -> bool {
    match s.chars().last() {
        Some(c) if is_word(c) => {
            let start = s.trim_end_matches(is_word).len();
            s[..start].trim_end().ends_with('.')
                || !matches!(&s[start..], "return" | "typeof" | "instanceof" | "in" | "new" | "delete" | "void" | "throw" | "case" | "do" | "else" | "yield" | "await")
        }
        Some(c) => c == ')' || c == ']',
        None => false,
    }
}

/// Just past the closing `/` of a regex literal at `i`, or `None` if its line ends first.
fn regex_end(text: &str, i: usize) -> Option<usize> {
    let b = text.as_bytes();
    let (mut j, mut class) = (i + 1, false);
    while j < b.len() {
        match b[j] {
            b'\n' => return None,
            b'\\' if b.get(j + 1) != Some(&b'\n') => j += 1,
            b'[' => class = true,
            b']' => class = false,
            b'/' if !class => return Some(j + 1),
            _ => {}
        }
        j += 1;
    }
    None
}

/// Scan a brace body. `text` holds the source, `start` is just after the opening `{`.
pub fn scan_braces(text: &str, start: usize, profile: Profile) -> Result<Scanned, Unclosed> {
    let mut s = Scanner::new(text, profile, true);
    s.run(start)
}

/// Line states of a fenced body (never scanned for its end).
pub fn line_states(text: &str, profile: Profile) -> Vec<bool> {
    let mut s = Scanner::new(text, profile, false);
    match s.run(0) {
        Ok(scanned) => scanned.starts_in_code,
        Err(_) => s.states,
    }
}

struct Scanner<'a> {
    text: &'a str,
    profile: Profile,
    /// Brace form: depth 0 ends the body. Fence form: braces never end it.
    find_end: bool,
    frames: Vec<Frame>,
    inner: Option<Inner>,
    /// Offsets of `{` opened inside the body and not yet closed.
    open_braces: Vec<usize>,
    states: Vec<bool>,
}

impl<'a> Scanner<'a> {
    fn new(text: &'a str, profile: Profile, find_end: bool) -> Scanner<'a> {
        Scanner {
            text,
            profile,
            find_end,
            frames: vec![Frame::Code { depth: 1 }],
            inner: None,
            open_braces: Vec::new(),
            states: vec![true],
        }
    }

    fn in_code(&self) -> bool {
        if let Some(Inner::LineComment) = self.inner {
            return true; // a line comment ends with its line
        }
        self.inner.is_none() && matches!(self.frames.last(), Some(Frame::Code { .. }))
    }

    fn run(&mut self, start: usize) -> Result<Scanned, Unclosed> {
        let bytes = self.text.as_bytes();
        let mut i = start;
        while i < self.text.len() {
            let c = self.text[i..].chars().next().unwrap_or('\0');
            let len = c.len_utf8();
            if c == '\n' {
                match self.inner {
                    Some(Inner::LineComment) | Some(Inner::Char { .. }) => self.inner = None,
                    Some(Inner::Str { multiline: false, .. }) => self.inner = None,
                    _ => {}
                }
                self.states.push(self.in_code());
                i += 1;
                continue;
            }
            if let Some(inner) = self.inner {
                i = self.inside(inner, i, c);
                continue;
            }
            match self.frames.last().copied() {
                Some(Frame::Template { .. }) => {
                    if c == '\\' {
                        i += len;
                        i = self.skip_escaped(i);
                    } else if c == '`' {
                        self.frames.pop();
                        i += 1;
                    } else if self.text[i..].starts_with("${") {
                        self.frames.push(Frame::Code { depth: 1 });
                        i += 2;
                    } else {
                        i += len;
                    }
                    continue;
                }
                Some(Frame::Code { .. }) | None => {}
            }
            // Code.
            match c {
                '{' => {
                    if let Some(Frame::Code { depth }) = self.frames.last_mut() {
                        *depth += 1;
                    }
                    self.open_braces.push(i);
                    i += 1;
                }
                '}' => {
                    let mut closed_frame = false;
                    if let Some(Frame::Code { depth }) = self.frames.last_mut() {
                        *depth -= 1;
                        closed_frame = *depth == 0;
                    }
                    if closed_frame {
                        if self.frames.len() == 1 {
                            if self.find_end {
                                return Ok(Scanned { close: i, starts_in_code: std::mem::take(&mut self.states) });
                            }
                            // Fence form: a stray `}` just resets the depth.
                            self.frames[0] = Frame::Code { depth: 1 };
                        } else {
                            self.frames.pop(); // back to the template
                        }
                    } else {
                        self.open_braces.pop();
                    }
                    i += 1;
                }
                _ => i = self.code_char(i, c, bytes),
            }
        }
        let open_construct = self
            .inner
            .and_then(Inner::start_and_name)
            .or_else(|| self.frames.iter().rev().find_map(|f| match f {
                Frame::Template { start } => Some((*start, "template literal")),
                _ => None,
            }));
        Err(Unclosed { open_brace: self.open_braces.last().copied(), open_construct })
    }

    /// One char of code that isn't a brace; returns the next index.
    fn code_char(&mut self, i: usize, c: char, bytes: &[u8]) -> usize {
        let rest = &self.text[i..];
        match self.profile {
            Profile::Plain => i + c.len_utf8(),
            Profile::Python => match c {
                '#' => {
                    self.inner = Some(Inner::LineComment);
                    i + 1
                }
                '"' | '\'' => {
                    let triple = rest.starts_with("\"\"\"") || rest.starts_with("'''");
                    self.inner = Some(Inner::Str { quote: c, triple, multiline: triple, start: i });
                    i + if triple { 3 } else { 1 }
                }
                _ => i + c.len_utf8(),
            },
            Profile::CLike(dialect) => {
                if rest.starts_with("//") {
                    self.inner = Some(Inner::LineComment);
                    return i + 2;
                }
                if rest.starts_with("/*") {
                    self.inner = Some(Inner::BlockComment { depth: 1, start: i });
                    return i + 2;
                }
                match c {
                    '"' => {
                        self.inner = Some(Inner::Str { quote: '"', triple: false, multiline: dialect == Dialect::Rust, start: i });
                        i + 1
                    }
                    '\'' => match dialect {
                        Dialect::Js => {
                            self.inner = Some(Inner::Str { quote: '\'', triple: false, multiline: false, start: i });
                            i + 1
                        }
                        Dialect::Rust => self.rust_quote(i),
                        Dialect::C | Dialect::Cpp => {
                            self.inner = Some(Inner::Char { start: i });
                            i + 1
                        }
                    },
                    '`' if dialect == Dialect::Js => {
                        self.frames.push(Frame::Template { start: i });
                        i + 1
                    }
                    '/' if dialect == Dialect::Js && regex_may_start(&self.text[..i]) => regex_end(self.text, i).unwrap_or(i + 1),
                    c if c.is_ascii_digit() && matches!(dialect, Dialect::C | Dialect::Cpp) => {
                        // A number; `'` inside it is a digit separator (1'000'000).
                        let mut j = i;
                        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'.' || bytes[j] == b'\'') {
                            j += 1;
                        }
                        j
                    }
                    c if c == '_' || c.is_alphanumeric() => self.word(i, dialect),
                    _ => i + c.len_utf8(),
                }
            }
        }
    }

    /// An identifier in C-like code; detects raw-string prefixes (C++ `R"`, Rust `r#"`).
    fn word(&mut self, i: usize, dialect: Dialect) -> usize {
        let rest = &self.text[i..];
        let len: usize = rest.chars().take_while(|c| *c == '_' || c.is_alphanumeric()).map(char::len_utf8).sum();
        let w = &rest[..len];
        let after = &rest[len..];
        match dialect {
            Dialect::Cpp if matches!(w, "R" | "u8R" | "uR" | "UR" | "LR") && after.starts_with('"') => {
                // R"delim( … )delim"
                let open = i + len + 1;
                let delim_len = self.text[open..].find('(').filter(|&d| d <= 16);
                match delim_len {
                    Some(d) => {
                        self.inner = Some(Inner::RawCpp { delim_start: open, delim_end: open + d, start: i });
                        open + d + 1
                    }
                    None => i + len,
                }
            }
            Dialect::Rust if matches!(w, "r" | "br" | "cr") => {
                let hashes = after.chars().take_while(|c| *c == '#').count();
                if after[hashes..].starts_with('"') {
                    self.inner = Some(Inner::RawRust { hashes, start: i });
                    i + len + hashes + 1
                } else {
                    i + len
                }
            }
            _ => i + len,
        }
    }

    /// Rust `'`: a char literal (`'x'`, `'\n'`, `'\u{7D}'`) or a lifetime/label (`'a`).
    fn rust_quote(&mut self, i: usize) -> usize {
        let mut chars = self.text[i + 1..].chars();
        match (chars.next(), chars.next()) {
            (Some('\\'), _) => {
                self.inner = Some(Inner::Char { start: i });
                i + 1
            }
            (Some(x), Some('\'')) => i + 1 + x.len_utf8() + 1,
            _ => i + 1, // a lifetime or label: only the quote is consumed
        }
    }

    /// After a `\`: skip the escaped char, which may be a line break.
    fn skip_escaped(&mut self, i: usize) -> usize {
        match self.text[i..].chars().next() {
            Some('\n') => {
                self.states.push(self.in_code());
                i + 1
            }
            Some(c) => i + c.len_utf8(),
            None => i,
        }
    }

    /// One char inside a string, char literal, raw string or comment; returns the next index.
    fn inside(&mut self, inner: Inner, i: usize, c: char) -> usize {
        let rest = &self.text[i..];
        match inner {
            Inner::LineComment => i + c.len_utf8(),
            Inner::BlockComment { depth, start } => {
                if rest.starts_with("*/") {
                    self.inner = if depth > 1 { Some(Inner::BlockComment { depth: depth - 1, start }) } else { None };
                    i + 2
                } else if rest.starts_with("/*") && self.profile == Profile::CLike(Dialect::Rust) {
                    self.inner = Some(Inner::BlockComment { depth: depth + 1, start });
                    i + 2
                } else {
                    i + c.len_utf8()
                }
            }
            Inner::Str { quote, triple, .. } => {
                if c == '\\' {
                    self.skip_escaped(i + 1)
                } else if triple {
                    let close = if quote == '"' { "\"\"\"" } else { "'''" };
                    if rest.starts_with(close) {
                        self.inner = None;
                        i + 3
                    } else {
                        i + c.len_utf8()
                    }
                } else if c == quote {
                    self.inner = None;
                    i + 1
                } else {
                    i + c.len_utf8()
                }
            }
            Inner::Char { .. } => {
                if c == '\\' {
                    self.skip_escaped(i + 1)
                } else if c == '\'' {
                    self.inner = None;
                    i + 1
                } else {
                    i + c.len_utf8()
                }
            }
            Inner::RawCpp { delim_start, delim_end, .. } => {
                let delim = &self.text[delim_start..delim_end];
                if rest.starts_with(')') && rest[1..].starts_with(delim) && rest[1 + delim.len()..].starts_with('"') {
                    self.inner = None;
                    i + 2 + delim.len()
                } else {
                    i + c.len_utf8()
                }
            }
            Inner::RawRust { hashes, .. } => {
                if c == '"' && rest[1..].chars().take(hashes).filter(|h| *h == '#').count() == hashes {
                    self.inner = None;
                    i + 1 + hashes
                } else {
                    i + c.len_utf8()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The body of `{ … }` in `src` (which starts right after the `{`), or the error.
    fn body(src: &str, profile: Profile) -> Result<&str, Unclosed> {
        scan_braces(src, 0, profile).map(|s| &src[..s.close])
    }

    #[test]
    fn profiles_from_tags() {
        assert_eq!(profile_for("python"), Profile::Python);
        assert_eq!(profile_for("ts"), Profile::CLike(Dialect::Js));
        assert_eq!(profile_for("cxx"), Profile::CLike(Dialect::Cpp));
        assert_eq!(profile_for("rs"), Profile::CLike(Dialect::Rust));
        assert_eq!(profile_for("lua"), Profile::Plain);
        assert_eq!(profile_for("PY"), Profile::Plain);
    }

    #[test]
    fn python() {
        let py = Profile::Python;
        assert_eq!(body(r#" print("}") } x"#, py).unwrap(), r#" print("}") "#);
        assert_eq!(body(" x = '{' } ", py).unwrap(), " x = '{' ");
        assert_eq!(body(" s = \"\"\"\n}\n\"\"\" } ", py).unwrap(), " s = \"\"\"\n}\n\"\"\" ");
        assert_eq!(body(" # }\n d = {1: 2}\n}", py).unwrap(), " # }\n d = {1: 2}\n");
        assert_eq!(body(" f\"{x}\" }", py).unwrap(), " f\"{x}\" ");
        assert_eq!(body(r#" r"\"}" }"#, py).unwrap(), r#" r"\"}" "#);
        // A stray quote damages one line only.
        assert_eq!(body(" x = 'oops\n}", py).unwrap(), " x = 'oops\n");
    }

    #[test]
    fn c_and_cpp() {
        let c = Profile::CLike(Dialect::C);
        assert_eq!(body(" char c = '{'; }", c).unwrap(), " char c = '{'; ");
        assert_eq!(body(" puts(\"}\"); // }\n /* } */ }", c).unwrap(), " puts(\"}\"); // }\n /* } */ ");
        assert_eq!(body(" int n = 1'000'000; }", Profile::CLike(Dialect::Cpp)).unwrap(), " int n = 1'000'000; ");
        let cpp = Profile::CLike(Dialect::Cpp);
        assert_eq!(body(r#" auto s = R"x(})")x"; }"#, cpp).unwrap(), r#" auto s = R"x(})")x"; "#);
    }

    #[test]
    fn rust() {
        let rs = Profile::CLike(Dialect::Rust);
        assert_eq!(body(r##" let s = r#"}"#; }"##, rs).unwrap(), r##" let s = r#"}"#; "##);
        assert_eq!(body(" fn f<'a>(x: &'a str) -> char { '}' } }", rs).unwrap(), " fn f<'a>(x: &'a str) -> char { '}' } ");
        assert_eq!(body(r" let c = '\u{7D}'; }", rs).unwrap(), r" let c = '\u{7D}'; ");
        assert_eq!(body(" /* /* } */ } */ }", rs).unwrap(), " /* /* } */ } */ ");
        assert_eq!(body(" let s = \"a\n}\"; }", rs).unwrap(), " let s = \"a\n}\"; ");
    }

    #[test]
    fn js_regexes() {
        let js = Profile::CLike(Dialect::Js);
        let src = " s.replace(/`([^`]+)`/g, \"x\"); r = /}/; c = /[/}]/; return /a\\/}/ } tail";
        assert_eq!(body(src, js).unwrap(), " s.replace(/`([^`]+)`/g, \"x\"); r = /}/; c = /[/}]/; return /a\\/}/ ");
        // Division: after an operand, a `/` is no regex.
        assert_eq!(body(" x = a / b / c } d", js).unwrap(), " x = a / b / c ");
        assert_eq!(body(" x = f(1) / 2 } d", js).unwrap(), " x = f(1) / 2 ");
        // After a postfix operator, a property named like a keyword, `x!` or a comment: division.
        for src in [" b = a++ / 2; s = \"/\" } t", " b = a-- / 2; s = \"/\" } t", " b = o.in / 2; s = \"/\" } t", " b = m! / 2; s = \"/\" } t", " b = 4 /* c */ / 2; s = \"/\" } t", " b = of / 2; s = \"/\" } t"] {
            assert_eq!(body(src, js).unwrap(), &src[..src.len() - 3], "{src}");
        }
        assert_eq!(body(" if (!/}/.test(s)) x() } t", js).unwrap(), " if (!/}/.test(s)) x() ");
        // A regex after a line comment, or after `return!`.
        assert_eq!(body(" const t = [\n // a brace.\n /^[{]/,\n] } t", js).unwrap(), " const t = [\n // a brace.\n /^[{]/,\n] ");
        assert_eq!(body(" return!/}/.test(s) } t", js).unwrap(), " return!/}/.test(s) ");
        // A regex can't span lines, so a lone `/` there is a division after all.
        assert_eq!(body(" x = (1\n/ 2) } d", js).unwrap(), " x = (1\n/ 2) ");
    }

    #[test]
    fn js_templates() {
        let js = Profile::CLike(Dialect::Js);
        assert_eq!(body(r#" `${"}"}` }"#, js).unwrap(), r#" `${"}"}` "#);
        assert_eq!(body(" `a ${ {b: 1}.b } ${`n ${2}`}` }", js).unwrap(), " `a ${ {b: 1}.b } ${`n ${2}`}` ");
        assert_eq!(body(" `}\n}` }", js).unwrap(), " `}\n}` ");
    }

    #[test]
    fn plain_counts_braces() {
        // Plain knows no strings: a `}` inside quotes closes the body (the fence form is the fix).
        assert_eq!(body(" print(\"}\") ", Profile::Plain).unwrap(), " print(\"");
        assert_eq!(body(" a { b } c } d", Profile::Plain).unwrap(), " a { b } c ");
    }

    #[test]
    fn unclosed_reports() {
        let e = body(" if x {\n y", Profile::CLike(Dialect::C)).unwrap_err();
        assert_eq!(e.open_brace, Some(6));
        let e = body(" s = \"\"\"\n never", Profile::Python).unwrap_err();
        assert_eq!(e.open_construct, Some((5, "string")));
        let e = body(" `abc", Profile::CLike(Dialect::Js)).unwrap_err();
        assert_eq!(e.open_construct, Some((1, "template literal")));
    }

    #[test]
    fn line_states_follow_multiline_constructs() {
        let s = scan_braces("\n x = \"\"\"\n text\n\"\"\"\n y\n}", 0, Profile::Python).unwrap();
        // lines: "" (after {), x = """, text, """, y, }
        assert_eq!(s.starts_in_code, vec![true, true, false, false, true, true]);
        let fence = line_states("a = 1\n/* c\n c */\nb\n", Profile::CLike(Dialect::C));
        assert_eq!(fence, vec![true, true, false, true, true]);
        assert_eq!(line_states("a\n'''\nb\n", Profile::Plain), vec![true, true, true, true]);
    }
}
