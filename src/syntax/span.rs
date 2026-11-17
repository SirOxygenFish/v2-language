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
/// named as such: Windows PowerShell 5's `>` and `Out-File` write UTF-16. UTF-32's marks are
/// looked for first, as the little-endian one starts with UTF-16's.
pub fn not_utf8_message(bytes: &[u8]) -> &'static str {
    // The locale doesn't change during a run.
    static OLDER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    not_utf8_named(bytes).unwrap_or_else(|| OLDER.get_or_init(|| format!("file is not valid UTF-8: {}", older_encoding("old.v2 > new.v2"))))
}

/// `not_utf8_message` for a file whose bytes say what it is: UTF-32, UTF-16 or binary.
pub fn not_utf8_named(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xFE, 0, 0]) || bytes.starts_with(&[0, 0, 0xFE, 0xFF]) {
        Some("file is not valid UTF-8: it is UTF-32 (what Out-File -Encoding utf32 writes); save it as UTF-8")
    } else if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) || bomless_utf16(bytes) {
        Some("file is not valid UTF-8: it is UTF-16 (what Windows PowerShell's > and Out-File write); save it as UTF-8")
    } else if bytes.iter().take(8192).any(|&b| b == 0) {
        Some("file is not valid UTF-8: it looks like a binary file, not a V2 script")
    } else {
        None
    }
}

/// What a file in an older encoding gets (2.1). On Linux and macOS such a file comes from an old
/// system or a share: iconv's line for `files` (`old.v2 > new.v2`), from the code page the locale
/// suggests (`locale_code_page`), then a look at the text, as iconv converts from a wrong code page
/// without an error.
pub fn older_encoding(files: &str) -> String {
    if cfg!(windows) {
        return "it may be in an older encoding, such as Windows-1252 (what Windows PowerShell's Set-Content writes): save it as UTF-8".to_string();
    }
    let (page, name) = locale_code_page();
    format!("it may be in an older encoding, such as {name}: save it as UTF-8, as in iconv -f {page} -t utf-8 {files}, then check the text")
}

/// The code page a file of the user's language was likely saved in, and its name, from the
/// locale's language (`LC_ALL`, else `LC_CTYPE`, else `LANG`): `tr_TR.UTF-8` → cp1254.
pub fn locale_code_page() -> (&'static str, &'static str) {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"].iter().filter_map(|v| std::env::var(v).ok()).find(|v| !v.is_empty()).unwrap_or_default();
    code_page_of(&locale)
}

/// `locale_code_page` for a locale's text: its language is what comes before `_`, `-`, `.` or `@`.
fn code_page_of(locale: &str) -> (&'static str, &'static str) {
    let lang = locale.split(['_', '.', '@', '-']).next().unwrap_or_default().to_ascii_lowercase();
    match lang.as_str() {
        "tr" => ("cp1254", "Windows-1254"),
        "ru" | "uk" | "bg" => ("cp1251", "Windows-1251"),
        "pl" | "cs" | "sk" | "hu" | "ro" | "hr" | "sl" => ("cp1250", "Windows-1250"),
        "el" => ("cp1253", "Windows-1253"),
        "he" => ("cp1255", "Windows-1255"),
        "ar" => ("cp1256", "Windows-1256"),
        "ja" => ("cp932", "Shift-JIS"),
        "zh" => ("gbk", "GBK"),
        "ko" => ("cp949", "EUC-KR"),
        _ => ("cp1252", "Windows-1252 or Latin-1"),
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
/// file's 128-bit ID on Windows (`FILE_ID_INFO`; the older 64-bit index isn't unique on ReFS, a
/// Dev Drive's file system), the device and inode elsewhere. Two canonical paths can name one
/// file (a share of a local folder, `\\localhost\C$\…`, and its `C:\…`), so a module is looked up
/// by it too (6.4). An ID of all zeros or all ones (some network redirectors give one) is none.
pub fn file_identity(path: &Path) -> Option<(u64, u128)> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{FileIdInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO};
        // No access to the data: another program's lock (no sharing) doesn't stop it.
        let f = std::fs::OpenOptions::new().access_mode(0).custom_flags(FILE_FLAG_BACKUP_SEMANTICS).open(path).ok()?;
        // SAFETY: a zeroed struct of the size given, for an open handle that `f` keeps open
        // during the call.
        let mut info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<FILE_ID_INFO>() as u32;
        if unsafe { GetFileInformationByHandleEx(f.as_raw_handle(), FileIdInfo, (&mut info as *mut FILE_ID_INFO).cast(), size) } == 0 {
            return None;
        }
        let id = u128::from_le_bytes(info.FileId.Identifier);
        (id != 0 && id != u128::MAX).then_some((info.VolumeSerialNumber, id))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| (m.dev(), u128::from(m.ino())))
    }
}

#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
    by_path: HashMap<PathBuf, FileId>,
    /// Loaded files by `file_identity`.
    by_identity: HashMap<(u64, u128), FileId>,
    /// Windows: loaded files on a `\\server\share` by `unc_key`.
    by_unc: HashMap<String, FileId>,
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
            // A file loaded before keeps the ID while its path still has it; a deleted one's
            // is this file's now.
            if !self.by_identity.get(&key).is_some_and(|&old| self.still_has(old, key)) {
                self.by_identity.insert(key, id);
            }
        }
        Ok(id)
    }

    /// The loaded file that `path` names under another canonical path, if any (6.4).
    pub fn same_file(&self, path: &Path) -> Option<FileId> {
        if self.by_path.contains_key(path) {
            return None;
        }
        // `\\wsl$\D\m.v2` or `\\WSL.LOCALHOST\d\m.v2` for a loaded `\\wsl.localhost\D\m.v2`: one
        // file, though WSL's share gives no file IDs.
        if let Some(&id) = path.to_str().filter(|_| cfg!(windows)).and_then(unc_key).and_then(|k| self.by_unc.get(&k)) {
            return Some(id);
        }
        if self.by_identity.is_empty() {
            return None;
        }
        let key = file_identity(path)?;
        self.by_identity.get(&key).copied().filter(|&id| self.still_has(id, key))
    }

    /// Whether the loaded file `id`'s own path still has the identity `key`. File systems give a
    /// deleted file's ID to the next new file (ext4 its inode at once, FAT the directory slot),
    /// so an ID alone could name another file (6.4).
    fn still_has(&self, id: FileId, key: (u64, u128)) -> bool {
        file_identity(&self.file(id).path) == Some(key)
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
        if let Some(key) = path.to_str().filter(|_| cfg!(windows)).and_then(unc_key) {
            self.by_unc.entry(key).or_insert(id);
        }
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

/// A `\\server\share\…` path (as `strip_verbatim` leaves one, with `\`) as module identity
/// compares it on Windows: the server and share in lower case, as Windows reads them in any case,
/// and `wsl$` as `wsl.localhost`, the same WSL share; the rest as it is (on WSL's share it is a
/// Linux path, where case matters) (6.4). `None` for another path.
pub fn unc_key(path: &str) -> Option<String> {
    let mut parts = path.strip_prefix(r"\\").filter(|r| !r.starts_with(['?', '.']))?.splitn(3, '\\');
    let server = parts.next().filter(|s| !s.is_empty())?.to_lowercase();
    let share = parts.next().filter(|s| !s.is_empty())?.to_lowercase();
    let server = if server == "wsl$" { "wsl.localhost".to_string() } else { server };
    Some(match parts.next() {
        Some(rest) => format!(r"\\{server}\{share}\{rest}"),
        None => format!(r"\\{server}\{share}"),
    })
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
/// others as `\u{1b}`), and the bidi formatting characters too (`\u{202e}`), so a name can't
/// split a message, act on the terminal or reorder the line, and the line break that `"C:\new"`
/// holds shows as written (2.1, 5.10). Text with none comes back as it is.
pub fn shown_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || is_bidi_format(c) => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The bidi embeddings, overrides and isolates (U+202A to U+202E, U+2066 to U+2069), which
/// reorder the text after them on screen (2.1, 2.4).
pub fn is_bidi_format(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
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
    fn shown_paths_escape_controls_and_bidi_formatting() {
        // 2.1 (round 20): embeddings, overrides and isolates would reorder a name on screen.
        assert_eq!(shown_path("rapor\u{202e}fdp.exe\u{1b}\n"), "rapor\\u{202e}fdp.exe\\u{1b}\\n");
        assert_eq!(shown_path("\u{202a}\u{202d}\u{2066}\u{2069}"), "\\u{202a}\\u{202d}\\u{2066}\\u{2069}");
        // Text in a right-to-left script and its marks stay as they are.
        assert_eq!(shown_path("\u{5e9}\u{5dc}\u{5d5}\u{5dd}\u{200f}.v2"), "\u{5e9}\u{5dc}\u{5d5}\u{5dd}\u{200f}.v2");
    }

    #[test]
    fn normalizes_bom_and_line_endings() {
        assert_eq!(normalize("\u{feff}a\r\nb\rc\n"), "a\nb\nc\n");
        assert_eq!(normalize("x"), "x");
    }

    #[test]
    fn the_code_page_comes_from_the_locales_language() {
        // 2.1 (round 20): iconv from cp1252 garbles a Turkish or Japanese file without an error.
        assert_eq!(code_page_of("tr_TR.UTF-8"), ("cp1254", "Windows-1254"));
        assert_eq!(code_page_of("ru_RU"), ("cp1251", "Windows-1251"));
        assert_eq!(code_page_of("uk_UA.UTF-8").0, "cp1251");
        assert_eq!(code_page_of("pl_PL.UTF-8").0, "cp1250");
        assert_eq!(code_page_of("el_GR.UTF-8").0, "cp1253");
        assert_eq!(code_page_of("he_IL.UTF-8").0, "cp1255");
        assert_eq!(code_page_of("ar_EG.UTF-8").0, "cp1256");
        assert_eq!(code_page_of("ja_JP.UTF-8"), ("cp932", "Shift-JIS"));
        assert_eq!(code_page_of("zh_CN.UTF-8").0, "gbk");
        assert_eq!(code_page_of("ko_KR.UTF-8").0, "cp949");
        for other in ["C.UTF-8", "POSIX", "", "de_DE.UTF-8", "en_US"] {
            assert_eq!(code_page_of(other), ("cp1252", "Windows-1252 or Latin-1"), "{other}");
        }
        // Those name a UTF-16 or UTF-32 mark only as a pair: 'ş' is 0xFE in cp1254.
        assert_eq!(not_utf8_named(b"\xfeehir\n"), None);
        assert_eq!(not_utf8_named(b"\xff\xfeh\x00"), Some("file is not valid UTF-8: it is UTF-16 (what Windows PowerShell's > and Out-File write); save it as UTF-8"));
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

    #[test]
    fn a_shares_name_is_one_however_it_is_spelled() {
        // 6.4 (round 21): Windows reads a server and a share in any case, and \\wsl$ is
        // \\wsl.localhost; WSL's share gives no file IDs, so a module there ran once per spelling.
        assert_eq!(unc_key(r"\\WSL$\Ubuntu\home\Me\m.v2").as_deref(), Some(r"\\wsl.localhost\ubuntu\home\Me\m.v2"));
        assert_eq!(unc_key(r"\\wsl.localhost\V2Win21").as_deref(), Some(r"\\wsl.localhost\v2win21"));
        assert_eq!(unc_key(r"\\Srv\Docs\A.v2").as_deref(), Some(r"\\srv\docs\A.v2"));
        for other in [r"C:\x\m.v2", r"\\?\C:\x", r"\\.\pipe\x", r"\\srv", r"\\", "m.v2"] {
            assert_eq!(unc_key(other), None, "{other}");
        }
        if cfg!(windows) {
            let mut m = SourceMap::new();
            let id = m.add(PathBuf::from(r"\\wsl.localhost\Ubuntu\root\m.v2"), "m.v2".into(), "");
            assert_eq!(m.same_file(Path::new(r"\\wsl$\ubuntu\root\m.v2")), Some(id));
            assert_eq!(m.same_file(Path::new(r"\\WSL.LOCALHOST\UBUNTU\root\m.v2")), Some(id));
            // The rest is a Linux path there, where case matters.
            assert_eq!(m.same_file(Path::new(r"\\wsl$\Ubuntu\root\M.v2")), None);
            assert_eq!(m.same_file(Path::new(r"\\wsl.localhost\Ubuntu\root\m.v2")), None, "the path itself is loaded");
        }
    }

    #[test]
    fn a_file_id_names_a_loaded_file_only_while_its_path_still_has_it() {
        // 6.4 (round 19): another path of a loaded file (a hard link here, a share in use) is that
        // file; but once the loaded path no longer has its ID (moved away here; deleted, as ext4
        // then gives the inode to the next new file), a path with that ID is another file.
        let d = std::env::temp_dir().join(format!("v2ident-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let canon = |p: &Path| strip_verbatim(&std::fs::canonicalize(p).unwrap());
        let (one, two, hard) = (d.join("one.v2"), d.join("two.v2"), d.join("hard.v2"));
        std::fs::write(&one, "let x = 1\n").unwrap();
        let mut m = SourceMap::new();
        let first = m.load(&one).ok().unwrap();
        std::fs::hard_link(&one, &hard).unwrap();
        assert_eq!(m.same_file(&canon(&hard)), Some(first));
        std::fs::remove_file(&hard).unwrap();
        let id = file_identity(&one);
        std::fs::rename(&one, &two).unwrap();
        assert!(id.is_some() && file_identity(&two) == id, "a moved file keeps its ID");
        assert_eq!(m.same_file(&canon(&two)), None);
        // Loaded, it holds that ID: a path of it is it.
        let second = m.load(&two).ok().unwrap();
        assert_ne!(second, first);
        std::fs::hard_link(&two, &hard).unwrap();
        assert_eq!(m.same_file(&canon(&hard)), Some(second));
        let _ = std::fs::remove_dir_all(&d);
    }
}
