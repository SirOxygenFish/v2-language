//! FileId, SourceMap and Span (INTERNALS 2.1, 2.2, 3.1).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Index into the SourceMap; the entry script is 0.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FileId(pub u32);

/// A byte range in a file's normalized text, plus the 1-based line and column of `lo`
/// (the column counts Unicode scalar values; a tab counts as 1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub file: FileId,
    pub lo: u32,
    pub hi: u32,
    pub line: u32,
    pub col: u32,
}

impl Span {
    /// From the start of `self` to the end of `other` (same file).
    pub fn to(self, other: Span) -> Span {
        Span { hi: other.hi.max(self.hi), ..self }
    }
}

/// Where a block body sits in its `.v2` file, for mapping foreign positions back (2.3).
#[derive(Clone, Debug, PartialEq)]
pub struct UnitLoc {
    pub file: FileId,
    /// `.v2` line of body line 1.
    pub body_first_line: u32,
    /// `.v2` column of body line 1, column 1.
    pub body_col: u32,
    /// Chars removed from each dedented line (4.4).
    pub dedent_width: u32,
    /// Lines in the processed body.
    pub line_count: u32,
    /// Body lines left undedented (4.4 step 3), sorted.
    pub kept: Rc<[u32]>,
    /// Body lines that lost one `@` of an `@@export`/`@@import` escape, and the column of the
    /// processed line where it was (4.4).
    pub escaped: Rc<[(u32, u32)]>,
}

impl UnitLoc {
    /// The `.v2` column of foreign column `fcol` on `body_line` (both 1-based), or `None` past
    /// any line; without a column, the caller uses the first non-blank column of the line (2.3).
    pub fn col(&self, body_line: u32, fcol: u32) -> Option<u32> {
        let escaped = self.escaped.iter().any(|&(line, at)| line == body_line && fcol >= at);
        let fcol = fcol.checked_add(escaped as u32)?;
        if body_line == 1 {
            self.body_col.checked_add(fcol)?.checked_sub(1)
        } else if self.kept.binary_search(&body_line).is_ok() {
            Some(fcol)
        } else {
            self.dedent_width.checked_add(fcol)
        }
    }
}

pub struct SourceFile {
    /// Canonical path without the `\\?\` prefix: the dedupe key.
    pub path: PathBuf,
    /// What messages and `e.file` print.
    pub display: Rc<str>,
    /// Normalized text (3.1); every offset refers to it.
    pub text: Rc<str>,
    /// Byte offset of each line start.
    pub line_starts: Vec<u32>,
}

/// Why a file could not be loaded.
pub enum LoadError {
    /// The file can't be read (missing, unreadable): a startup error, exit 2.
    Io(String),
    /// The bytes aren't UTF-8: a `SyntaxError` at the line of the first bad byte; `message` says
    /// so and how to fix it (2.1).
    NotUtf8 { display: Rc<str>, line: u32, message: &'static str },
}

/// The message for a file that isn't UTF-8 (2.1). A UTF-16 file (a byte-order mark first) is
/// named as such: Windows PowerShell 5's `>` and `Out-File` write UTF-16.
pub fn not_utf8_message(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        "file is not valid UTF-8: it is UTF-16 (what Windows PowerShell's > and Out-File write); save it as UTF-8"
    } else if bytes.iter().take(8192).any(|&b| b == 0) {
        "file is not valid UTF-8: it looks like a binary file, not a V2 script"
    } else {
        "file is not valid UTF-8: save it as UTF-8"
    }
}

#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
    by_path: HashMap<PathBuf, FileId>,
}

impl SourceMap {
    pub fn new() -> SourceMap {
        SourceMap::default()
    }

    /// Read, validate and normalize a file (3.1). A path that is already loaded returns its id.
    pub fn load(&mut self, path: &Path) -> Result<FileId, LoadError> {
        let canonical = std::fs::canonicalize(path)
            .map_err(|e| LoadError::Io(format!("cannot read '{}': {}", path.display(), os_message(&e))))?;
        let canonical = strip_verbatim(&canonical);
        if let Some(&id) = self.by_path.get(&canonical) {
            return Ok(id);
        }
        let bytes = std::fs::read(&canonical)
            .map_err(|e| LoadError::Io(format!("cannot read '{}': {}", display_path(&canonical), os_message(&e))))?;
        let display = display_path(&canonical);
        let text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(e) => {
                // Count line breaks the way `normalize` does: every LF, and every CR not
                // followed by LF (a CR right before the bad byte counts too).
                let good = &e.as_bytes()[..e.utf8_error().valid_up_to()];
                let breaks = good
                    .iter()
                    .enumerate()
                    .filter(|&(i, &b)| b == b'\n' || (b == b'\r' && good.get(i + 1) != Some(&b'\n')))
                    .count();
                let line = breaks as u32 + 1;
                return Err(LoadError::NotUtf8 { display, line, message: not_utf8_message(e.as_bytes()) });
            }
        };
        Ok(self.add(canonical, display, &text))
    }

    /// Add in-memory source (tests, and files already read). The text is normalized.
    pub fn add(&mut self, path: PathBuf, display: Rc<str>, text: &str) -> FileId {
        let text = normalize(text);
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        let id = FileId(self.files.len() as u32);
        self.by_path.insert(path.clone(), id);
        self.files.push(SourceFile { path, display, text: text.into(), line_starts });
        id
    }

    pub fn file(&self, id: FileId) -> &SourceFile {
        &self.files[id.0 as usize]
    }

    /// 1-based line and column (in chars) of a byte offset.
    pub fn line_col(&self, id: FileId, offset: u32) -> (u32, u32) {
        let f = self.file(id);
        let line_idx = match f.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let start = f.line_starts[line_idx] as usize;
        let col = f.text[start..offset as usize].chars().count() as u32 + 1;
        (line_idx as u32 + 1, col)
    }

    /// The file loaded from a canonical path, if any.
    pub fn id_of(&self, path: &Path) -> Option<FileId> {
        self.by_path.get(path).copied()
    }

    /// A one-character span at a 1-based line and column (in chars), if the file has them:
    /// where a foreign frame points (10.3, 12.6).
    pub fn span_at(&self, id: FileId, line: u32, col: u32) -> Option<Span> {
        let f = self.file(id);
        let start = *f.line_starts.get((line as usize).checked_sub(1)?)? as usize;
        let text = self.line_text(id, line);
        let (offset, c) = text.char_indices().nth((col as usize).checked_sub(1)?).unwrap_or((text.len(), ' '));
        if offset == text.len() && col as usize != text.chars().count() + 1 {
            return None;
        }
        let lo = (start + offset) as u32;
        let hi = if offset < text.len() { lo + c.len_utf8() as u32 } else { lo };
        Some(Span { file: id, lo, hi, line, col })
    }

    /// The text of a 1-based line, without its line break.
    pub fn line_text(&self, id: FileId, line: u32) -> &str {
        let f = self.file(id);
        let i = (line as usize).saturating_sub(1);
        let Some(&start) = f.line_starts.get(i) else { return "" };
        let end = f.line_starts.get(i + 1).map(|&e| e as usize - 1).unwrap_or(f.text.len());
        &f.text[start as usize..end.max(start as usize)]
    }

    /// A span over `lo..hi` with line and column computed.
    pub fn span(&self, id: FileId, lo: u32, hi: u32) -> Span {
        let (line, col) = self.line_col(id, lo);
        Span { file: id, lo, hi, line, col }
    }
}

/// Strip one leading BOM; turn CRLF and lone CR into LF (3.1).
pub fn normalize(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if !text.contains('\r') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// `path` made absolute with `.` dropped and `..` applied to the text, as Windows does; POSIX
/// `std::path::absolute` keeps `..`, so walking up `/a/../b` would visit `/a` (2.1, 18.1).
pub fn absolute_clean(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut clean = PathBuf::new();
    for c in absolute.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                clean.pop();
            }
            other => clean.push(other.as_os_str()),
        }
    }
    clean
}

pub fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p.to_path_buf()
    }
}

/// Relative to the current directory with `/` separators when inside it, else absolute (2.1).
pub fn display_path(canonical: &Path) -> Rc<str> {
    if let Ok(cwd) = std::env::current_dir().and_then(std::fs::canonicalize) {
        let cwd = strip_verbatim(&cwd);
        if let Ok(rel) = canonical.strip_prefix(&cwd) {
            let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            if !parts.is_empty() {
                return parts.join("/").into();
            }
        }
    }
    canonical.to_string_lossy().into_owned().into()
}

/// The OS error text without Rust's ` (os error N)` suffix.
pub fn os_message(e: &std::io::Error) -> String {
    let s = e.to_string();
    let s = match s.find(" (os error ") {
        Some(i) => &s[..i],
        None => &s[..],
    };
    // Windows ends its messages with a period; V2's messages don't (2.5).
    s.trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(text: &str) -> (SourceMap, FileId) {
        let mut m = SourceMap::new();
        let id = m.add(PathBuf::from("t.v2"), "t.v2".into(), text);
        (m, id)
    }

    #[test]
    fn normalizes_bom_and_line_endings() {
        assert_eq!(normalize("\u{feff}a\r\nb\rc\n"), "a\nb\nc\n");
        assert_eq!(normalize("x"), "x");
    }

    #[test]
    fn line_and_char_columns() {
        let (m, id) = map("ab\n\u{e9}x\n");
        assert_eq!(m.line_col(id, 0), (1, 1));
        assert_eq!(m.line_col(id, 3), (2, 1));
        // 'é' is two bytes; 'x' is the second char of line 2.
        assert_eq!(m.line_col(id, 5), (2, 2));
        assert_eq!(m.line_text(id, 2), "\u{e9}x");
        assert_eq!(m.line_text(id, 3), "");
    }
}
