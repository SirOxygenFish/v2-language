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
    /// The file can't be read (missing, unreadable): a startup error, exit 2; then what to do,
    /// when the OS's reason has an answer (`read_hint`).
    Io(String, Option<&'static str>),
    /// The bytes aren't UTF-8: a `SyntaxError` at the line and column of the first bad byte;
    /// `message` says so and how to fix it (2.1).
    NotUtf8 { display: Rc<str>, line: u32, col: u32, message: &'static str },
}

/// The message for a file that isn't UTF-8 (2.1). A UTF-16 file (a byte-order mark first) is
/// named as such: Windows PowerShell 5's `>` and `Out-File` write UTF-16.
pub fn not_utf8_message(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) || bomless_utf16(bytes) {
        "file is not valid UTF-8: it is UTF-16 (what Windows PowerShell's > and Out-File write); save it as UTF-8"
    } else if bytes.iter().take(8192).any(|&b| b == 0) {
        "file is not valid UTF-8: it looks like a binary file, not a V2 script"
    } else if cfg!(windows) {
        "file is not valid UTF-8: it may be in an older encoding, such as Windows-1252 (what Windows PowerShell's Set-Content writes): save it as UTF-8"
    } else {
        // On Linux and macOS such a file comes from an old system or a share.
        "file is not valid UTF-8: it may be in an older encoding, such as Windows-1252 or Latin-1: save it as UTF-8, as in iconv -f cp1252 -t utf-8 old.v2 > new.v2"
    }
}

/// UTF-16 without a byte order mark: in Latin text most pairs are an ASCII letter, digit,
/// space or line break and a NUL, the same way round (3 in 4 here). Some editors save UTF-16
/// that way; a binary file's NULs come in runs.
pub fn bomless_utf16(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8192) & !1];
    let pairs = sample.len() / 2;
    let text = |b: u8| matches!(b, b'\t' | b'\n' | b'\r' | 0x20..=0x7E);
    let count = |le: bool| sample.chunks(2).filter(|c| if le { text(c[0]) && c[1] == 0 } else { c[0] == 0 && text(c[1]) }).count();
    pairs >= 2 && (count(true) * 4 >= pairs * 3 || count(false) * 4 >= pairs * 3)
}

/// What to do about a file V2 can't read, for the script and the files it imports (19.1, 6.4):
/// on Windows one another program opened without sharing (`ERROR_SHARING_VIOLATION`,
/// `ERROR_LOCK_VIOLATION`), as the file builtins say too (5.10).
pub fn read_hint(e: &std::io::Error) -> Option<&'static str> {
    (cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33))).then_some("another program has it open (Excel keeps an open file locked): close it there")
}

/// A file's identity, the same however a path reaches it: the volume's serial number and the
/// file's index on Windows, the device and inode elsewhere. Two canonical paths can name one file
/// (a share of a local folder, `\\localhost\C$\…`, and its `C:\…`), so a module is looked up by
/// it too (6.4).
pub fn file_identity(path: &Path) -> Option<(u64, u64)> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS};
        // No access to the data: another program's lock (no sharing) doesn't stop it.
        let f = std::fs::OpenOptions::new().access_mode(0).custom_flags(FILE_FLAG_BACKUP_SEMANTICS).open(path).ok()?;
        // SAFETY: a zeroed struct for an open handle, which `f` keeps open during the call.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(f.as_raw_handle(), &mut info) } == 0 {
            return None;
        }
        Some((u64::from(info.dwVolumeSerialNumber), (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
    }
}

#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
    by_path: HashMap<PathBuf, FileId>,
    /// Loaded files by `file_identity`.
    by_identity: HashMap<(u64, u64), FileId>,
}

impl SourceMap {
    pub fn new() -> SourceMap {
        SourceMap::default()
    }

    /// Read, validate and normalize a file (3.1). A path that is already loaded returns its id.
    pub fn load(&mut self, path: &Path) -> Result<FileId, LoadError> {
        let canonical = std::fs::canonicalize(path)
            .map_err(|e| LoadError::Io(format!("cannot read '{}': {}", shown_path(&path.to_string_lossy()), os_message(&e)), read_hint(&e)))?;
        let canonical = strip_verbatim(&canonical);
        // `v2 x.v2` in a folder whose name isn't UTF-8: script_dir() and messages couldn't name
        // it, as main says for such a FILE (19.1, round 16).
        if canonical.to_str().is_none() {
            return Err(LoadError::Io(
                format!(
                    "the path '{}' is not valid UTF-8, which V2 needs: a folder on the way has a name in another encoding; rename it",
                    shown_path(&canonical.to_string_lossy())
                ),
                None,
            ));
        }
        if let Some(&id) = self.by_path.get(&canonical) {
            return Ok(id);
        }
        let bytes = std::fs::read(&canonical)
            .map_err(|e| LoadError::Io(format!("cannot read '{}': {}", display_path(&canonical), os_message(&e)), read_hint(&e)))?;
        let identity = file_identity(&canonical);
        let display = display_path(&canonical);
        // Latin text in UTF-16 is valid UTF-8 with a NUL between the letters.
        if bomless_utf16(&bytes) {
            return Err(LoadError::NotUtf8 { display, line: 1, col: 1, message: not_utf8_message(&bytes) });
        }
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
                // The bad byte's column: the characters before it on its line, plus one.
                let before = std::str::from_utf8(good).unwrap_or_default();
                let col = before.rsplit(['\n', '\r']).next().map_or(0, |l| l.chars().count()) as u32 + 1;
                return Err(LoadError::NotUtf8 { display, line, col, message: not_utf8_message(e.as_bytes()) });
            }
        };
        let id = self.add(canonical, display, &text);
        if let Some(key) = identity {
            self.by_identity.entry(key).or_insert(id);
        }
        Ok(id)
    }

    /// The loaded file that `path` names under another canonical path, if any (6.4).
    pub fn same_file(&self, path: &Path) -> Option<FileId> {
        if self.by_identity.is_empty() || self.by_path.contains_key(path) {
            return None;
        }
        file_identity(path).and_then(|key| self.by_identity.get(&key).copied())
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

/// Relative to the current directory with `/` separators when inside it, else absolute, with
/// control characters escaped (2.1).
pub fn display_path(canonical: &Path) -> Rc<str> {
    if let Ok(cwd) = std::env::current_dir().and_then(std::fs::canonicalize) {
        let cwd = strip_verbatim(&cwd);
        // Relative to a filesystem root (`cd /`), a path would only lose its first `/`.
        if cwd.parent().is_some() {
            if let Ok(rel) = canonical.strip_prefix(&cwd) {
                let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                if !parts.is_empty() {
                    return shown_path(&parts.join("/")).into();
                }
            }
        }
    }
    shown_path(&canonical.to_string_lossy()).into()
}

/// A path or a file name as messages show it: control characters escaped (`\n`, `\r`, `\t`, the
/// others as `\u{1b}`), so a name can't split a message or act on the terminal, and the line
/// break that `"C:\new"` holds shows as written (2.1, 5.10). Text with none comes back as it is.
pub fn shown_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
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
