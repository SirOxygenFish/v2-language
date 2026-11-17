//! V2 modules: parsing the import graph at startup and loading files (INTERNALS 6.1, 6.4).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::syntax::ast::*;
use crate::syntax::span::{absolute_clean, display_path, is_stand_in, os_message, shown_path, FileId, LoadError, SourceMap, Span};
use crate::syntax::{check, lexer, parser, Diag};

/// A static diagnostic of a module file, kept until an `import` of it runs (6.1 step 2).
pub struct ModuleDiag {
    pub display: Rc<str>,
    pub line: u32,
    pub col: u32,
    pub message: String,
    /// The line and caret, and the hint, as `v2 check` prints them (2.4, 6.4).
    pub excerpt: String,
    pub hint: Option<String>,
}

/// A file read at startup or on its first import: its program, or its diagnostics.
pub enum Parsed {
    Ok { file: FileId, program: Rc<Vec<Stmt>> },
    Failed(Vec<ModuleDiag>),
}

fn module_diag(sm: &SourceMap, d: &Diag) -> ModuleDiag {
    ModuleDiag {
        display: sm.file(d.span.file).display.clone(),
        line: d.span.line,
        col: d.span.col,
        message: d.message.lines().next().unwrap_or("").to_string(),
        excerpt: crate::runtime::error::excerpt(sm, d.span),
        hint: d.hint.clone(),
    }
}

/// Load, lex, parse and check one file. `Err` holds what to print for the entry file;
/// `Parsed::Failed` holds a module's diagnostics.
pub fn parse_file(sm: &mut SourceMap, path: &Path) -> Result<Parsed, LoadError> {
    let file = match sm.load(path) {
        Ok(id) => id,
        Err(LoadError::NotUtf8 { display, line, col, message }) => {
            return Ok(Parsed::Failed(vec![ModuleDiag { display, line, col, message: message.into(), excerpt: String::new(), hint: None }]))
        }
        Err(e) => return Err(e),
    };
    let mut program = match lexer::lex(sm, file).and_then(|toks| parser::parse_program(sm, toks)) {
        Ok(p) => p,
        Err(d) => return Ok(Parsed::Failed(vec![module_diag(sm, &d)])),
    };
    let diags = check::check(&program);
    if !diags.is_empty() {
        return Ok(Parsed::Failed(diags.iter().map(|d| module_diag(sm, d)).collect()));
    }
    crate::syntax::resolve::resolve(&mut program);
    Ok(Parsed::Ok { file, program: Rc::new(program) })
}

/// Where `import "spec"` in `importer` points: relative to the importing file's folder,
/// absolute paths as is; no extension is added (6.4 step 1).
pub fn resolve(sm: &SourceMap, importer: FileId, spec: &str) -> PathBuf {
    let base = sm.file(importer).path.parent().map(Path::to_path_buf).unwrap_or_default();
    base.join(spec)
}

/// Why `import "spec"` loads no file: the `ImportError`'s message and its hint (6.4).
pub type ImportFailure = (String, Option<String>);

/// The file `import "spec"` in `importer` loads, canonical, or the `ImportError`'s message and
/// hint (6.4), with control characters escaped (2.1). A drive-relative Windows path (`C:m.v2`) is
/// neither relative to the file nor absolute.
pub fn locate(sm: &SourceMap, importer: FileId, spec: &str) -> Result<PathBuf, ImportFailure> {
    locate_noted(sm, importer, spec).map(|(path, _)| path)
}

/// `locate`, and the note for a spelling that differs from the file's name on disk only in case
/// (`case_note`). A file already loaded under another canonical path (`SourceMap::same_file`) is
/// that one.
pub fn locate_noted(sm: &SourceMap, importer: FileId, spec: &str) -> Result<(PathBuf, Option<String>), ImportFailure> {
    let found = locate_raw(sm, importer, spec).map_err(|(m, hint)| (shown_path(&m), hint.map(|h| shown_path(&h))))?;
    let note = case_note(&resolve(sm, importer, spec), spec, &found);
    match sm.same_file(&found) {
        Some(id) => Ok((sm.file(id).path.clone(), note)),
        None => Ok((found, note)),
    }
}

thread_local! {
    /// Import sites whose case note was printed: one note per site in a run (6.4).
    static NOTED: std::cell::RefCell<std::collections::HashSet<(u32, u32)>> = Default::default();
}

/// Whether the case note for the import at `at` is still to be printed (once per site).
pub fn first_note(at: Span) -> bool {
    NOTED.with(|n| n.borrow_mut().insert((at.file.0, at.lo)))
}

/// `import "./REAL/Counter.V2"` that found `real/counter.v2`, on a file system that ignores case
/// (Windows', macOS's): `note: './REAL/Counter.V2' is spelled './real/counter.v2' on disk: …`,
/// since a case-sensitive one won't find it. The written names at its end are compared with the
/// canonical path's last ones; a link among them (whose target has its own name) or any other
/// difference (an 8.3 short name) is no case problem (6.4).
pub fn case_note(target: &Path, spec: &str, found: &Path) -> Option<String> {
    // The written names at the end, each with where it starts in `spec`.
    let mut parts: Vec<(usize, &str)> = Vec::new();
    let mut start = 0;
    for (i, c) in spec.char_indices().chain(std::iter::once((spec.len(), '/'))) {
        if i == spec.len() || std::path::is_separator(c) {
            parts.push((start, &spec[start..i]));
            start = i + c.len_utf8();
        }
    }
    let plain = |p: &str| !p.is_empty() && p != "." && p != ".." && !(p.len() == 2 && p.ends_with(':'));
    let k = parts.iter().rev().take_while(|(_, p)| plain(p)).count();
    let written = &parts[parts.len() - k..];
    let real: Vec<String> = found.components().rev().take(k).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if k == 0 || real.len() < k {
        return None;
    }
    let real: Vec<&String> = real.iter().rev().collect();
    let same_but_case = written.iter().zip(&real).all(|((_, w), r)| w.to_lowercase() == r.to_lowercase());
    if !same_but_case || written.iter().zip(&real).all(|((_, w), r)| *w == r.as_str()) {
        return None;
    }
    let mut at = target.to_path_buf();
    for _ in 0..k {
        if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            return None;
        }
        at.pop();
    }
    let sep = if spec.contains('/') || !spec.contains('\\') { "/" } else { "\\" };
    let spelled = format!("{}{}", &spec[..written[0].0], real.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(sep));
    Some(shown_path(&format!("note: '{spec}' is spelled '{spelled}' on disk: a case-sensitive file system, as Linux's, won't find it")))
}

/// A link on the way to a path that can't be followed (DOCS 3.10): one to something that isn't
/// there, or one in a loop. Windows' junctions are links too (`FileType::is_symlink` says so).
pub struct LinkProblem {
    /// The path up to and including the link, spelled as the path was.
    pub at: PathBuf,
    /// What a link that leads nowhere names, absolute (`link_target`; `None` when it can't be
    /// read); `None` for a loop or a Linux symlink.
    target: Option<PathBuf>,
    looped: bool,
    /// A link to a folder (a junction, a directory symlink): Windows removes it with rmdir.
    dir: bool,
    /// A Linux symlink WSL made (`WslFile::Link`), which Windows can't follow wherever it points.
    linux: bool,
}

impl LinkProblem {
    /// `'brokenjunc' is a link to C:\x\gone, which isn't there`, or `… is a link in a loop`, with
    /// `name` (`'brokenjunc'`, or `it` for the path itself).
    pub fn problem(&self, name: &str) -> String {
        match (&self.target, self.looped) {
            _ if self.linux => format!("{name} is a Linux (WSL) symlink, which Windows can't follow"),
            (_, true) => format!("{name} is a link in a loop"),
            (Some(to), false) => format!("{name} is a link to {}, which isn't there", to.display()),
            (None, false) => format!("{name} is a link to something that isn't there"),
        }
    }

    /// What fixes it, for a message that ends with what to do: `remove the link (rmdir
    /// sub\brokenjunc), or make C:\x\gone`, with `link` the link as typed from the current folder
    /// (as written, or as messages show it; `remove_command`). A Linux symlink is no broken link:
    /// the path it points to, never its removal (DOCS 3.10); Windows can't read where it points.
    pub fn fix(&self, link: &str) -> String {
        if self.linux {
            return "use the path it points to (ls -l in WSL shows it), or run V2 inside WSL".to_string();
        }
        let remove = remove_command(link, self.dir, std::env::var_os("MSYSTEM").is_some());
        match (&self.target, self.looped) {
            (Some(to), false) => format!("remove the link ({remove}), or make {}", to.display()),
            _ => format!("remove the link ({remove}), or point it somewhere else"),
        }
    }
}

/// The command that removes the link `link`: on Windows `rmdir`, or `del` for a file's link, with
/// `\`, as cmd takes `/` for a switch; in Git Bash (`git_bash`: `MSYSTEM` is set), whose bash
/// drops a `\` and whose rmdir refuses a junction, `rm` with `/`; elsewhere `rm`.
fn remove_command(link: &str, dir: bool, git_bash: bool) -> String {
    let (remove, link) = match (cfg!(windows), git_bash, dir) {
        (true, false, true) => ("rmdir", link.replace('/', "\\")),
        (true, false, false) => ("del", link.replace('/', "\\")),
        (true, true, _) => ("rm", link.replace('\\', "/")),
        (false, ..) => ("rm", link.to_string()),
    };
    format!("{remove} {}", crate::cli::shell_word(&link))
}

/// Where the link `at` points, absolute: a relative target (`ln -s ../shared lib`) is relative to
/// the link's own folder, so `make gone` would make the wrong folder (DOCS 3.10).
fn link_target(at: &Path, to: &Path) -> PathBuf {
    if to.is_absolute() {
        // `\\?\C:\x` reads as `C:\x`; `\\?\Volume{…}\x` (a mount point's or a junction into a
        // volume's target) has no other name: as it is, never joined to the link's folder (the
        // second review of round 19).
        let plain = crate::syntax::span::strip_verbatim(to);
        return if plain.is_absolute() { plain } else { to.to_path_buf() };
    }
    let folder = at.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    absolute_clean(&canonical(folder).unwrap_or_else(|| folder.to_path_buf()).join(to))
}

/// The start of `path` as written that is the path `at`, one of its beginnings (`a/link` of
/// `a/link/x.txt`, where `at` would print `a\link` on Windows).
pub fn written_prefix<'a>(path: &'a str, at: &Path) -> std::borrow::Cow<'a, str> {
    let ends = path.char_indices().filter(|&(_, c)| std::path::is_separator(c)).map(|(i, _)| i).chain(std::iter::once(path.len()));
    match ends.map(|i| &path[..i]).find(|p| Path::new(p) == at) {
        Some(p) => p.into(),
        None => at.to_string_lossy().into_owned().into(),
    }
}

/// Whether `e` says that links lead in a loop: `ELOOP`, or Windows'
/// `ERROR_CANT_RESOLVE_FILENAME` (1921, "The name of the file cannot be resolved by the system").
pub fn is_link_loop(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    return e.raw_os_error() == Some(libc::ELOOP);
    #[cfg(windows)]
    return e.raw_os_error() == Some(1921);
}

/// What WSL made that Windows can't use as a file, by its reparse tag (DOCS 3.10, 3.11): a Linux
/// symlink (`IO_REPARSE_TAG_LX_SYMLINK`), which Windows can't follow, or a pipe, a socket or a
/// device (`LX_FIFO`, `AF_UNIX`, `LX_CHR`, `LX_BLK`), which Windows takes for a file: a read of a
/// pipe through \\wsl.localhost waits 10 s and leaves a thread of WSL's file server waiting for a
/// writer, and one of a `/dev/zero` node never ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WslFile {
    Link,
    Special,
}

/// The `WslFile` a reparse tag names.
pub fn wsl_tag(tag: u32) -> Option<WslFile> {
    match tag {
        0xA000_001D => Some(WslFile::Link),
        // AF_UNIX, LX_FIFO, LX_CHR, LX_BLK.
        0x8000_0023..=0x8000_0026 => Some(WslFile::Special),
        _ => None,
    }
}

/// Windows: the `WslFile` that `path` names itself (a link isn't followed). `None` elsewhere.
pub fn wsl_file(path: &Path) -> Option<WslFile> {
    reparse_tag(path).and_then(wsl_tag)
}

/// Windows: the reparse tag of `path` itself: its attributes, then, for a reparse point only, its
/// tag (`FileAttributeTagInfo`), so another file costs one call and nothing is opened for
/// reading; a link WSL made on a drive, which Windows may not even open
/// (`ERROR_CANT_ACCESS_FILE`), has it in its folder's listing (`listed_tag`). `None` elsewhere.
fn reparse_tag(path: &Path) -> Option<u32> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileAttributeTagInfo, GetFileAttributesW, GetFileInformationByHandleEx, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, INVALID_FILE_ATTRIBUTES,
        };
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.is_empty() || wide.contains(&0) {
            return None;
        }
        wide.push(0);
        // SAFETY: a NUL-terminated path.
        let attrs = unsafe { GetFileAttributesW(wide.as_ptr()) };
        if attrs == INVALID_FILE_ATTRIBUTES || attrs & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
            return None;
        }
        let f = match std::fs::OpenOptions::new().access_mode(0).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS).open(path) {
            Ok(f) => f,
            Err(e) if e.raw_os_error() == Some(1920) => return listed_tag(&wide),
            Err(_) => return None,
        };
        // SAFETY: a zeroed struct of the size given, for a handle that `f` keeps open during the
        // call.
        let mut info: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32;
        if unsafe { GetFileInformationByHandleEx(f.as_raw_handle(), FileAttributeTagInfo, (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(), size) } == 0 {
            return None;
        }
        Some(info.ReparseTag)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}

/// Windows: the reparse tag that the listing of `wide`'s folder gives it (`FindFirstFileW`'s
/// `dwReserved0`), `wide` a NUL-terminated path without wildcards.
#[cfg(windows)]
fn listed_tag(wide: &[u16]) -> Option<u32> {
    use windows_sys::Win32::Storage::FileSystem::{FindClose, FindFirstFileW, FILE_ATTRIBUTE_REPARSE_POINT, WIN32_FIND_DATAW};
    if wide.iter().any(|&c| c == u16::from(b'*') || c == u16::from(b'?')) {
        return None;
    }
    // SAFETY: a zeroed struct that FindFirstFileW fills, for a NUL-terminated path; the handle
    // is closed at once.
    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let h = unsafe { FindFirstFileW(wide.as_ptr(), &mut data) };
    if h == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return None;
    }
    unsafe { FindClose(h) };
    (data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0).then_some(data.dwReserved0)
}

/// Whether `m`, a file's metadata, is a reparse point's (Windows' `FILE_ATTRIBUTE_REPARSE_POINT`),
/// as WSL's pipes and devices are: only such a file needs `wsl_file`'s look.
pub fn reparse_point(m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    return std::os::windows::fs::MetadataExt::file_attributes(m) & 0x400 != 0;
    #[cfg(not(windows))]
    {
        let _ = m;
        false
    }
}

/// The first link on the way to `path`, or `path` itself, that leads nowhere or in a loop: each
/// part is looked at with `symlink_metadata`, then a link followed with `metadata` (DOCS 3.10).
/// `None` when every link leads somewhere, or a part can't be looked at (missing, unreadable).
pub fn link_problem(path: &Path) -> Option<LinkProblem> {
    let mut at = PathBuf::new();
    for c in path.components() {
        at.push(c);
        if !matches!(c, std::path::Component::Normal(_)) {
            continue;
        }
        let kind = match std::fs::symlink_metadata(&at) {
            Ok(m) => m.file_type(),
            // A link WSL made on a drive, which Windows may not even look at (1920).
            Err(_) if wsl_file(&at) == Some(WslFile::Link) => return Some(LinkProblem { at, target: None, looped: false, dir: false, linux: true }),
            Err(_) => return None,
        };
        if !kind.is_symlink() {
            continue;
        }
        #[cfg(windows)]
        let dir = std::os::windows::fs::FileTypeExt::is_symlink_dir(&kind);
        #[cfg(unix)]
        let dir = false;
        match std::fs::metadata(&at) {
            Ok(_) => {}
            // A Linux symlink seen through \\wsl.localhost (or made by WSL on a Windows drive),
            // which Windows can't follow wherever it points: never one that leads nowhere.
            Err(_) if wsl_file(&at) == Some(WslFile::Link) => return Some(LinkProblem { at, target: None, looped: false, dir, linux: true }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Some(LinkProblem { target: std::fs::read_link(&at).ok().map(|to| link_target(&at, &to)), at, looped: false, dir, linux: false });
            }
            Err(e) if is_link_loop(&e) => return Some(LinkProblem { at, target: None, looped: true, dir, linux: false }),
            Err(_) => return None,
        }
    }
    None
}

/// What to say of `link`, a Linux symlink (`WslFile::Link`) that another program (Windows' Node,
/// for a package in node_modules) couldn't follow: `'…\node_modules\pkg' is a Linux (WSL)
/// symlink, which Windows can't follow: use the path it points to (…), or run V2 inside WSL`.
pub fn wsl_link_line(link: &Path) -> String {
    let problem = LinkProblem { target: None, at: link.to_path_buf(), looped: false, dir: false, linux: true };
    shown_path(&format!("{}: {}", problem.problem(&format!("'{}'", link.display())), problem.fix("")))
}

/// A file builtin's failure on a path through a link that leads nowhere or in a loop (5.10):
/// `IOError: cannot write 'brokenjunc/new.txt': 'brokenjunc' is a link to C:\x\gone, which isn't
/// there`, `hint: remove the link (rmdir brokenjunc), or make C:\x\gone`, not a `make_dir` hint
/// that would fail, nor Windows' "Access is denied".
pub fn link_error(verb: &str, path: &str) -> Option<crate::runtime::error::ErrorValue> {
    let link = link_problem(Path::new(path))?;
    let written = written_prefix(path, &link.at);
    let name = if link.at == Path::new(path) { "it".to_string() } else { format!("'{written}'") };
    let err = crate::runtime::error::ErrorValue::new("IOError", shown_path(&format!("cannot {verb} '{path}': {}", link.problem(&name))));
    let _ = err.hint.set(shown_path(&link.fix(&written)).into());
    Some(err)
}

fn locate_raw(sm: &SourceMap, importer: FileId, spec: &str) -> Result<PathBuf, ImportFailure> {
    let p = Path::new(spec);
    if matches!(p.components().next(), Some(std::path::Component::Prefix(_))) && !p.has_root() {
        return Err((format!("cannot find module '{spec}' (a drive-relative path: write it relative to the file, or absolute)"), None));
    }
    let target = resolve(sm, importer, spec);
    // Windows: `./util.v2.` would load util.v2, as Windows drops the dot (5.10).
    if let Some(refused) = dotted_import(&target, spec) {
        return Err(refused);
    }
    let found = match crate::syntax::span::canonicalize(&target) {
        Ok(found) => crate::syntax::span::strip_verbatim(&found),
        // There, but it can't be followed: a link to a file that isn't there, a link in a loop
        // (ELOOP, Windows' 1921), or the OS's reason, as for FILE (19.1).
        // A link to an anonymous pipe (/dev/stdin from a pipe, /proc/self/fd/0, <(…)) is there
        // to the OS, which follows /proc's links, though canonicalize can't name its target.
        Err(e) if std::fs::symlink_metadata(&target).is_ok() || wsl_file(&target) == Some(WslFile::Link) => {
            if std::fs::metadata(&target).is_ok_and(|m| !m.is_dir() && !m.is_file()) {
                return Err((format!("cannot import '{spec}': it is a pipe or a device, not a file"), None));
            }
            // A link that leads nowhere (its target absolute) or in a loop: that, and what fixes it,
            // as for a folder on the way (DOCS 3.10; the second review of round 19).
            if let Some(link) = link_problem(&target).filter(|l| l.at == target) {
                return Err((format!("cannot import '{spec}': {}", link.problem("it")), Some(link.fix(&shown(&link.at)))));
            }
            let message = if is_link_loop(&e) { format!("cannot import '{spec}': it is a link in a loop") } else { format!("cannot import '{spec}': {}", os_message(&e)) };
            return Err((message, None));
        }
        Err(e) => {
            // \\wsl.localhost\NAME\… with no such WSL distribution (DOCS 3.10).
            if let Some(why) = crate::runtime::builtins::wsl_root(&target.to_string_lossy(), &e) {
                return Err((format!("cannot import '{spec}': {why}"), None));
            }
            // A link on the way that leads nowhere or in a loop, a junction too (DOCS 3.10).
            if let Some(link) = link_problem(&target) {
                let at = shown(&link.at);
                return Err((format!("cannot import '{spec}': {}", link.problem(&format!("'{at}'"))), Some(link.fix(&at))));
            }
            // There, in a folder V2 may not read (a deny inherited from it): the OS's reason.
            let lstat = std::fs::symlink_metadata(&target).err();
            if e.kind() == std::io::ErrorKind::PermissionDenied || lstat.is_some_and(|l| l.kind() == std::io::ErrorKind::PermissionDenied) {
                return Err((format!("cannot import '{spec}': {}", os_message(&e)), None));
            }
            // `import "./util" as util` for util.v2.
            let with = target.with_file_name(format!("{}.v2", target.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
            let hint = if spec.ends_with(".v2") || !with.is_file() { String::new() } else { format!(" — did you mean '{}.v2'?", trim_separators(spec)) };
            // Else a file of that folder it probably meant (`./util.v2` for Util.v2 on Linux).
            let (near, twin) = if hint.is_empty() { nearby_module(&target, spec) } else { (String::new(), None) };
            return Err((format!("cannot find module '{spec}' (looked for {}){hint}{near}", shown(&target)), twin));
        }
    };
    // A pipe or a device (a FIFO, a link to /dev/zero) would never end being read; the entry
    // FILE is refused the same way (19.1). So is one WSL made, which Windows takes for a file.
    if std::fs::metadata(&found).is_ok_and(|m| (!m.is_dir() && !m.is_file()) || (reparse_point(&m) && wsl_file(&found) == Some(WslFile::Special))) {
        return Err((format!("cannot import '{spec}': it is a pipe or a device, not a file"), None));
    }
    // A folder: one of its .v2 files is what to write (Windows would say "Access is denied").
    if found.is_dir() {
        let mut names: Vec<String> = std::fs::read_dir(&found)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.to_ascii_lowercase().ends_with(".v2"))
            .collect();
        names.sort();
        let file = names.first().map_or("FILE.v2", String::as_str);
        let stem = &file[..file.len() - 3];
        let plain = stem.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') && stem.chars().all(|c| c.is_alphanumeric() || c == '_');
        let alias = if plain && file != "FILE.v2" { stem } else { "m" };
        let joined = format!("{}/{file}", spec.trim_end_matches(['/', '\\']));
        return Err((format!("cannot import '{spec}': it is a folder, and import loads a .v2 file, as in import \"{joined}\" as {alias}"), None));
    }
    // `import` loads V2 files: another language's file, or data, is used another way.
    let ext = found.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    let Some(ext) = ext.filter(|e| e != "v2") else { return Ok(found) };
    let rel = spec.strip_prefix("./").or_else(|| spec.strip_prefix(".\\")).unwrap_or(spec);
    let next = match ext.as_str() {
        "py" if !rel.contains("..") && Path::new(rel).is_relative() => {
            let module = rel.trim_end_matches(".py").replace(['/', '\\'], ".");
            format!("a Python file is imported with @import {{ name }} from py.{module}")
        }
        "py" => "a Python file next to a .v2 file is imported with @import { name } from py.NAME".to_string(),
        "js" | "mjs" | "cjs" => format!("a JavaScript file is imported with @import {{ name }} from js.\"{spec}\""),
        "ts" | "mts" | "cts" => format!("a TypeScript file is imported with @import {{ name }} from ts.\"{spec}\""),
        // An absolute path is read as it is: script_dir() would come before it (round 17).
        _ if Path::new(rel).is_absolute() || Path::new(rel).has_root() => {
            let mut path = String::new();
            crate::runtime::value::quoted(&rel.replace('\\', "/"), &mut path);
            format!("read a data file with read_file({path})")
        }
        _ => format!("read a data file with read_file(f\"${{script_dir()}}/{}\")", rel.replace('\\', "/")),
    };
    Err((format!("cannot import '{spec}': import loads .v2 files; {next}"), None))
}

/// After a lookup of `name` in `folder` failed: a name there that is `name` in another Unicode
/// form (`größe.v2` as a Mac writes it, `o` plus a combining U+0308, for `größe.v2` typed with
/// `ö`), and what to do, as the CLI's line and the hint of an `ImportError` and a file builtin's
/// `IOError` say it (19.1, 6.4, 5.10).
pub fn unicode_twin(folder: &Path, name: &str) -> Option<String> {
    use unicode_normalization::UnicodeNormalization;
    let want: String = name.nfc().collect();
    let twin = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .find(|n| n != name && !(n.is_ascii() && name.is_ascii()) && n.nfc().eq(want.chars()))?;
    let how = "decomposed Unicode (letters plus combining accents, as macOS writes names)";
    Some(if twin.nfc().ne(twin.chars()) {
        format!("there is a '{twin}' whose name looks the same but is written in {how}: rename it")
    } else {
        format!("there is a '{twin}' whose name looks the same, but the name given is written in {how}: type it again")
    })
}

/// After a lookup of `path` failed because a folder on its way isn't there: `unicode_twin`'s hint
/// for the first folder that is missing (`größe/x.v2` when the folder is a Mac's `größe`), as the
/// CLI, an `ImportError` and a file builtin's `IOError` say it (19.1, 6.4, 5.10).
pub fn folder_twin(path: &Path) -> Option<String> {
    let mut missing = path.parent().filter(|p| !p.as_os_str().is_empty() && !p.is_dir())?;
    while let Some(parent) = missing.parent().filter(|p| !p.as_os_str().is_empty() && !p.is_dir()) {
        missing = parent;
    }
    let folder = missing.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    unicode_twin(folder, missing.file_name()?.to_str()?)
}

/// A path written without `.v2` whose last name, `name`, has a `unicode_twin` with `.v2`: as it
/// is written, its last name composed (NFC, as a keyboard types it and as the twin is once
/// renamed), plus `.v2`, for the CLI's `, and run v2 größe.v2` and an import's
/// `, and import './büyüt.v2'` (19.1, 6.4). A separator at its end is dropped (`./büyüt/`).
pub fn composed_v2(written: &str, name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let written = trim_separators(written);
    match written.strip_suffix(name) {
        Some(before) => format!("{before}{}.v2", name.nfc().collect::<String>()),
        None => format!("{written}.v2"),
    }
}

/// A path as written without the separators it ends with (`/`, and on Windows `\` too) and the
/// `.` parts after them, whose last name a hint changes: `hello/` and `hello/.` are `hello`, so
/// its `.v2` is `hello.v2`, not `hello/.v2`.
pub fn trim_separators(written: &str) -> &str {
    let sep = |c: char| c == '/' || (cfg!(windows) && c == '\\');
    let mut w = written.trim_end_matches(sep);
    while let Some(before) = w.strip_suffix('.').filter(|b| b.ends_with(sep)) {
        w = before.trim_end_matches(sep);
    }
    w
}

/// A relative `import` of a missing `.v2` file (6.4): ` — did you mean './Util.v2'?` (the path as
/// written, `trim_separators`, with the name found) for a `.v2` file of its folder within 5.9's
/// did-you-mean distance, the names compared without `.v2`, or one whose name differs only in
/// its `.v2`'s case; a case-sensitive file system's case is named. Before those, the hint for a
/// name in another Unicode form (`unicode_twin`): PATH's, PATH`.v2`'s when PATH doesn't end in
/// `.v2` (with the import that works then), or a missing folder's on the way (`folder_twin`).
fn nearby_module(target: &Path, spec: &str) -> (String, Option<String>) {
    // Windows: `./q?.v2` for a Linux `q?.v2` that WSL made, `q\u{f03f}.v2` to Windows: the import
    // that reaches it, and what its stand-in is.
    if let Some(written) = with_stand_ins(spec).filter(|_| stand_in_twin(&target.to_string_lossy()).is_some()) {
        return (format!(" — did you mean '{written}'?"), stand_in_note(&written));
    }
    let (Some(folder), Some(name)) = (target.parent(), target.file_name().and_then(|n| n.to_str())) else { return (String::new(), None) };
    let with_v2 = || unicode_twin(folder, &format!("{name}.v2")).map(|t| format!("{t}, and import '{}'", composed_v2(spec, name)));
    let twin = unicode_twin(folder, name).or_else(|| if spec.ends_with(".v2") { None } else { with_v2() });
    if let Some(twin) = twin.or_else(|| folder_twin(target)) {
        return (String::new(), Some(twin));
    }
    // `./util.v2/`: the file itself, which Windows 11 and macOS find with a separator after it.
    let trimmed = trim_separators(spec);
    if trimmed.len() < spec.len() && trimmed.ends_with(name) && folder.join(name).is_file() {
        return (format!(" — did you mean '{trimmed}'?"), None);
    }
    let stem = |s: &str| s.len().checked_sub(3).filter(|&i| s.is_char_boundary(i) && s[i..].eq_ignore_ascii_case(".v2")).map(|i| s[..i].to_string());
    let Some(want) = stem(name).filter(|_| Path::new(spec).is_relative()) else { return (String::new(), None) };
    let stems: Vec<(String, String)> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| stem(&n).map(|s| (s, n)))
        .collect();
    // `./lib/Util.V2` for Util.v2: the same stem, which did-you-mean leaves out as the name itself.
    let same = stems.iter().find(|(s, n)| *s == want && n != name).map(|(_, n)| n.as_str());
    let Some(found) = same.or_else(|| {
        let close = crate::runtime::error::did_you_mean(&want, stems.iter().map(|(s, _)| s.as_str()))?;
        Some(stems.iter().find(|(s, _)| s == close).map_or(close, |(_, n)| n.as_str()))
    }) else {
        return (String::new(), None);
    };
    let spec = trim_separators(spec);
    let written = match spec.rfind(['/', '\\']) {
        Some(i) => format!("{}{found}", &spec[..=i]),
        None => found.to_string(),
    };
    let case = if found.to_lowercase() == name.to_lowercase() {
        if cfg!(target_os = "linux") { " (Linux matches the case of file names)" } else { " (this file system matches the case of file names)" }
    } else {
        ""
    };
    (format!(" — did you mean '{written}'?{case}"), stand_in_note(found))
}

/// `fs::canonicalize` (`span::canonicalize`) without the `\\?\` prefix, or `None` when the file
/// is missing.
pub fn canonical(path: &Path) -> Option<PathBuf> {
    crate::syntax::span::canonicalize(path).ok().map(|p| crate::syntax::span::strip_verbatim(&p))
}

/// Windows: an import whose path has a part that ends in a dot or a space, which Windows drops
/// (`./util.v2.` would load util.v2), refused as the file builtins refuse it (5.10, 6.4): the
/// `\\?\` way to a file of that name when it is there (WSL and Macs make them; one whose own name
/// ends so isn't a `.v2` file), else the name Windows would read when that is there.
fn dotted_import(target: &Path, spec: &str) -> Option<ImportFailure> {
    let (why, there) = crate::runtime::builtins::dotted_name(target.to_str()?)?;
    let message = format!("cannot import '{spec}': {why}");
    let last = target.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(['.', ' ']));
    Some(match there {
        Some(_) if last => (message, Some("one of that name is there (WSL and Macs can make one), but a module's name ends in .v2: rename it".to_string())),
        Some(v) => {
            let mut q = String::new();
            crate::runtime::value::quoted(&v, &mut q);
            let stem = target.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            let plain = stem.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') && stem.chars().all(|c| c.is_alphanumeric() || c == '_');
            let alias = if plain { stem } else { "m" };
            (message, Some(format!(r"one of that name is there (WSL and Macs can make one): reach it by its full path with \\?\ in front, as in import {q} as {alias}")))
        }
        // `./util.v2.` for util.v2: the import without the dots and spaces.
        None if std::fs::metadata(target).is_ok() => (format!("{message} — did you mean '{}'?", without_dots(spec)), None),
        None => (message, None),
    })
}

/// `path` as written with the dots and spaces at the end of each part (not `.` or `..`) taken
/// off, as Windows reads it: `./lib./util.v2.` is `./lib/util.v2` (5.10, 6.4, 19.1).
pub fn without_dots(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut at = 0;
    for part in path.split(['/', '\\']) {
        out.push_str(if matches!(part, "." | "..") { part } else { part.trim_end_matches(['.', ' ']) });
        at += part.len();
        // The separator after it, as written.
        if let Some(sep) = path[at..].chars().next() {
            out.push(sep);
            at += sep.len_utf8();
        }
    }
    out
}

/// Windows: the part of `path` after its root, where a name can't hold `:`: after `X:`, after the
/// `\\server\share` of a UNC path (with either slash), or all of a relative path; `None` for a
/// `\\?\` or `\\.\` path (`//?/`, `//./` too), which is taken as written (5.10).
pub fn after_root(path: &str) -> Option<&str> {
    let b = path.as_bytes();
    let sep = |i: usize| matches!(b.get(i), Some(b'/' | b'\\'));
    if sep(0) && sep(1) {
        if matches!(b.get(2), Some(b'?' | b'.')) && (sep(3) || b.len() == 3) {
            return None;
        }
        let mut parts = path[2..].splitn(3, ['/', '\\']);
        let (_server, _share) = (parts.next(), parts.next());
        return Some(parts.next().unwrap_or_default());
    }
    Some(if b.get(1) == Some(&b':') { &path[2..] } else { path })
}

/// The characters a Windows name can't hold whose WSL stand-ins (`span::is_stand_in`) Windows
/// shows for a Linux name that has them; `/` and `\` are separators, and control characters are
/// the escapes' hint's (5.10).
pub const HELD: [char; 7] = [':', '*', '?', '"', '<', '>', '|'];

/// `path` with each of `HELD` in its parts after the root (`after_root`) as WSL's stand-in
/// (`mod:x.v2` as `mod\u{f03a}x.v2`); `None` when it has none.
pub fn with_stand_ins(path: &str) -> Option<String> {
    let rest = after_root(path).filter(|r| r.contains(HELD))?;
    let root = &path[..path.len() - rest.len()];
    Some(root.chars().chain(rest.chars().map(|c| if HELD.contains(&c) { char::from_u32(0xF000 + c as u32).unwrap_or(c) } else { c })).collect())
}

/// Windows: `with_stand_ins(path)` when something is there by that name: a Linux name WSL made,
/// which Windows reaches only so (5.10, 6.4, 19.1).
pub fn stand_in_twin(path: &str) -> Option<String> {
    with_stand_ins(path).filter(|twin| cfg!(windows) && std::fs::symlink_metadata(twin).is_ok())
}

/// Windows: for a name or path that holds a WSL stand-in, which a did-you-mean or a listing
/// offers (`q\u{f03f}.v2`, as Windows shows a Linux `q?.v2`): `\u{f03f} is how Windows shows the
/// '?' of the Linux name 'q?.v2': rename it in WSL` (the part that holds it; 6.4, 19.1, 5.10).
pub fn stand_in_note(text: &str) -> Option<String> {
    let part = text.split(['/', '\\']).find(|p| cfg!(windows) && p.chars().any(is_stand_in))?;
    let real = |c: char| if is_stand_in(c) { char::from_u32(c as u32 - 0xF000).unwrap_or(c) } else { c };
    let c = part.chars().find(|&c| is_stand_in(c))?;
    let linux: String = part.chars().map(real).collect();
    Some(format!("{} is how Windows shows the '{}' of the Linux name '{}': rename it in WSL", shown_path(&c.to_string()), shown_path(&real(c).to_string()), shown_path(&linux)))
}

/// 6.1 step 2: every file the `import` literals of `program` name, depth-first in source order.
/// Missing files are skipped (their `import` fails when it runs).
pub fn parse_graph(sm: &mut SourceMap, file: FileId, program: &[Stmt], parsed: &mut HashMap<PathBuf, Parsed>) {
    // Depth-first with an explicit stack: an import chain can be longer than the host stack.
    let entry = sm.file(file).path.clone();
    let mut stack = vec![(file, import_sites(program).into_iter())];
    while let Some((importer, sites)) = stack.last_mut() {
        let importer = *importer;
        let Some((spec, _)) = sites.next() else {
            stack.pop();
            continue;
        };
        let Ok(path) = locate(sm, importer, &spec) else { continue };
        if parsed.contains_key(&path) || path == entry {
            continue;
        }
        let Ok(result) = parse_file(sm, &path) else { continue };
        let next = match &result {
            Parsed::Ok { file, program } => Some((*file, import_sites(program).into_iter())),
            Parsed::Failed(_) => None,
        };
        parsed.insert(path, result);
        if let Some(frame) = next {
            stack.push(frame);
        }
    }
}

/// Every `import "…"` in a program with its statement's span, in source order, at any depth
/// (6.4: imports may appear in any body, including function bodies).
pub fn import_sites(program: &[Stmt]) -> Vec<(Rc<str>, Span)> {
    let mut out = Vec::new();
    collect_imports(program, &mut out);
    out
}

fn collect_imports(stmts: &[Stmt], out: &mut Vec<(Rc<str>, Span)>) {
    for s in stmts {
        stmt_imports(s, out);
    }
}

fn body_imports(b: &Body, out: &mut Vec<(Rc<str>, Span)>) {
    collect_imports(&b.stmts, out);
}

fn stmt_imports(s: &Stmt, out: &mut Vec<(Rc<str>, Span)>) {
    match &s.kind {
        StmtKind::Import { path, .. } => out.push((path.clone(), s.span)),
        StmtKind::Let(_, e) => e.iter().for_each(|e| expr_imports(e, out)),
        StmtKind::Const(_, e) | StmtKind::Expr(e) | StmtKind::Throw(e) => expr_imports(e, out),
        StmtKind::Return(e) => e.iter().for_each(|e| expr_imports(e, out)),
        StmtKind::Assign { target, value, .. } => {
            match target {
                Target::Name(..) => {}
                Target::Member(o, _) => expr_imports(o, out),
                Target::Index(o, i, _) => {
                    expr_imports(o, out);
                    expr_imports(i, out);
                }
            }
            expr_imports(value, out);
        }
        StmtKind::If { arms, else_ } => {
            for (c, b) in arms {
                expr_imports(c, out);
                body_imports(b, out);
            }
            else_.iter().for_each(|b| body_imports(b, out));
        }
        StmtKind::While { cond, body } => {
            expr_imports(cond, out);
            body_imports(body, out);
        }
        StmtKind::For { iter, body, .. } => {
            match iter {
                ForIter::Value(e) => expr_imports(e, out),
                ForIter::Range(a, b) => {
                    expr_imports(a, out);
                    expr_imports(b, out);
                }
            }
            body_imports(body, out);
        }
        StmtKind::Try { body, catch, finally } => {
            body_imports(body, out);
            catch.iter().for_each(|(_, b)| body_imports(b, out));
            finally.iter().for_each(|b| body_imports(b, out));
        }
        StmtKind::Func(f) => func_imports(f, out),
        StmtKind::Block(b) => b.args.iter().for_each(|(_, e)| expr_imports(e, out)),
        StmtKind::Break | StmtKind::Continue | StmtKind::EngineImport(_) | StmtKind::Extern(_) => {}
    }
}

fn func_imports(f: &FuncDecl, out: &mut Vec<(Rc<str>, Span)>) {
    for (_, d) in &f.params {
        d.iter().for_each(|d| expr_imports(d, out));
    }
    match &f.body {
        FuncBody::Block(b) => body_imports(b, out),
        FuncBody::Expr(e) => expr_imports(e, out),
    }
}

fn expr_imports(e: &Expr, out: &mut Vec<(Rc<str>, Span)>) {
    match &e.kind {
        ExprKind::Func(f) => func_imports(f, out),
        ExprKind::FStr(parts) => {
            for p in parts {
                if let FPart::Expr { expr, .. } = p {
                    expr_imports(expr, out);
                }
            }
        }
        ExprKind::List(items) => items.iter().for_each(|x| expr_imports(x, out)),
        ExprKind::Dict(entries) => {
            for (k, v) in entries {
                if let DictKey::Computed(k) = k {
                    expr_imports(k, out);
                }
                expr_imports(v, out);
            }
        }
        ExprKind::Unary(_, _, x) | ExprKind::OptChain(x) => expr_imports(x, out),
        ExprKind::Binary(_, _, a, b) | ExprKind::And(a, b) | ExprKind::Or(a, b) | ExprKind::Coalesce(a, b) => {
            expr_imports(a, out);
            expr_imports(b, out);
        }
        ExprKind::Ternary(c, a, b) => {
            expr_imports(c, out);
            expr_imports(a, out);
            expr_imports(b, out);
        }
        ExprKind::Call { callee, args, .. } => {
            expr_imports(callee, out);
            for a in args {
                match a {
                    Arg::Pos(x) | Arg::Named(_, x) | Arg::Spread(x) => expr_imports(x, out),
                }
            }
        }
        ExprKind::Index { obj, index, .. } => {
            expr_imports(obj, out);
            expr_imports(index, out);
        }
        ExprKind::Slice { obj, lo, hi, .. } => {
            expr_imports(obj, out);
            for x in [lo, hi].into_iter().flatten() {
                expr_imports(x, out);
            }
        }
        ExprKind::Member { obj, .. } => expr_imports(obj, out),
        ExprKind::Block(b) => b.args.iter().for_each(|(_, e)| expr_imports(e, out)),
        ExprKind::Null
        | ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bytes(_)
        | ExprKind::Name(..) => {}
    }
}

/// How a path that may not exist prints in messages (2.1): `.` and `..` resolved, and the
/// longest existing part canonicalized, so case and short names match the real folder.
pub fn shown(path: &Path) -> Rc<str> {
    let clean = absolute_clean(path);
    let mut existing = clean.clone();
    let mut rest = Vec::new();
    while canonical(&existing).is_none() {
        match existing.file_name() {
            Some(name) => {
                rest.push(name.to_os_string());
                existing.pop();
            }
            None => return display_path(&clean),
        }
    }
    let mut full = canonical(&existing).unwrap_or(existing);
    for name in rest.iter().rev() {
        full.push(name);
    }
    display_path(&full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spelling_that_differs_only_in_case_gets_a_note() {
        // 6.4 (round 19): the written names at the end against the canonical path's last ones.
        let base = std::env::temp_dir().join(format!("v2case-{}", std::process::id()));
        let note = |spec: &str, found: &str| case_note(&base.join(spec), spec, &base.join(found));
        assert_eq!(
            note("./REAL/Counter.V2", "real/counter.v2").as_deref(),
            Some("note: './REAL/Counter.V2' is spelled './real/counter.v2' on disk: a case-sensitive file system, as Linux's, won't find it")
        );
        // Only the parts written are compared, with the separator written.
        assert_eq!(
            note("../lib/X.v2", "lib/x.v2").as_deref(),
            Some("note: '../lib/X.v2' is spelled '../lib/x.v2' on disk: a case-sensitive file system, as Linux's, won't find it")
        );
        if cfg!(windows) {
            assert_eq!(
                note(r".\Lib\x.v2", "lib/x.v2").as_deref(),
                Some(r"note: '.\Lib\x.v2' is spelled '.\lib\x.v2' on disk: a case-sensitive file system, as Linux's, won't find it")
            );
            // An 8.3 short name is another name, not a case problem.
            assert_eq!(note("./REALDI~1/x.v2", "RealDirectory/x.v2"), None);
        }
        assert_eq!(note("./real/counter.v2", "real/counter.v2"), None);
        assert_eq!(note("./other.v2", "real.v2"), None);
    }

    #[test]
    fn the_parts_after_a_root_and_wsls_stand_ins_for_them() {
        // INTERNALS 5.10, 6.4 (round 21): a ':' after a share's \\server\share is a stream, as
        // after a drive, with either slash; WSL shows a Linux name's ':' as U+F03A.
        assert_eq!(after_root(r"\\srv\share\a:b.txt"), Some("a:b.txt"));
        assert_eq!(after_root("//srv/share/dir/a:b.txt"), Some("dir/a:b.txt"));
        assert_eq!(after_root(r"\\wsl$\Ubuntu/root\x:y"), Some(r"root\x:y"));
        assert_eq!(after_root(r"\\srv"), Some(""));
        assert_eq!(after_root("C:x:y"), Some("x:y"));
        assert_eq!(after_root("notes 12:30.txt"), Some("notes 12:30.txt"));
        for written in [r"\\?\C:\a:b", r"\\.\pipe\x", "//?/C:/a", "//./C:/a"] {
            assert_eq!(after_root(written), None, "{written}");
        }
        assert_eq!(with_stand_ins(r"\\wsl.localhost\d\root\q?.v2").as_deref(), Some("\\\\wsl.localhost\\d\\root\\q\u{f03f}.v2"));
        assert_eq!(with_stand_ins("case/mod:x|y.v2").as_deref(), Some("case/mod\u{f03a}x\u{f07c}y.v2"));
        assert_eq!(with_stand_ins("C:/plain.v2"), None);
        assert_eq!(without_dots("./lib./util.v2. "), "./lib/util.v2");
        assert_eq!(without_dots(r"..\a. \b"), r"..\a\b");
        let note = stand_in_note("case/q\u{f03f}.v2");
        let want = cfg!(windows).then(|| r"\u{f03f} is how Windows shows the '?' of the Linux name 'q?.v2': rename it in WSL".to_string());
        assert_eq!(note, want);
        assert_eq!(stand_in_note("case/plain.v2"), None);
        // The review of round 21: a Nerd Font icon of U+F000 to U+F0FF is no stand-in of WSL's.
        assert_eq!(stand_in_note("\u{f023}lock.v2"), None);
        let tab = cfg!(windows).then(|| r"\u{f009} is how Windows shows the '\t' of the Linux name 'a\tb.v2': rename it in WSL".to_string());
        assert_eq!(stand_in_note("a\u{f009}b.v2"), tab);
    }

    /// A link to a folder: a junction on Windows (which needs no administrator), else a symlink.
    pub(crate) fn dir_link(link: &Path, target: &Path) {
        #[cfg(windows)]
        {
            let ok = std::process::Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(link).arg(target).stdout(std::process::Stdio::null()).status().unwrap().success();
            assert!(ok, "mklink /J {}", link.display());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn links_that_lead_nowhere_or_in_a_loop_are_found() {
        // DOCS 3.10 (round 19): a folder part of the path, a junction on Windows.
        let d = std::env::temp_dir().join(format!("v2links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("gone")).unwrap();
        std::fs::create_dir_all(d.join("real")).unwrap();
        dir_link(&d.join("broken"), &d.join("gone"));
        std::fs::remove_dir(d.join("gone")).unwrap();
        dir_link(&d.join("loop1"), &d.join("loop2"));
        dir_link(&d.join("loop2"), &d.join("loop1"));
        dir_link(&d.join("fine"), &d.join("real"));
        let broken = link_problem(&d.join("broken").join("lib.v2")).expect("the broken link");
        assert_eq!(broken.at, d.join("broken"));
        assert_eq!(broken.problem("'broken'"), format!("'broken' is a link to {}, which isn't there", d.join("gone").display()));
        // The link as given, in the command the shell needs (`remove_command`).
        let remove = remove_command("broken", true, std::env::var_os("MSYSTEM").is_some());
        assert_eq!(broken.fix("broken"), format!("remove the link ({remove}), or make {}", d.join("gone").display()));
        let looped = link_problem(&d.join("loop1").join("x").join("y.v2")).expect("the loop");
        assert_eq!((looped.at.clone(), looped.problem("it")), (d.join("loop1"), "it is a link in a loop".to_string()));
        assert!(looped.fix("loop1").ends_with(" loop1), or point it somewhere else"), "{}", looped.fix("loop1"));
        // A link that leads somewhere, and a path that simply isn't there, have none.
        assert!(link_problem(&d.join("fine").join("nope.v2")).is_none());
        assert!(link_problem(&d.join("nope").join("x.v2")).is_none());
        assert_eq!(written_prefix("a/broken/x.txt", Path::new("a").join("broken").as_path()), "a/broken");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_command_that_removes_a_link_suits_the_shell() {
        // 19.1, 5.10 (round 19): cmd takes `sub/broken` for a switch ("Invalid switch"), Git
        // Bash's bash drops a `\` and its rmdir refuses a junction, where rm removes it.
        if cfg!(windows) {
            assert_eq!(remove_command("sub/broken", true, false), r"rmdir sub\broken");
            assert_eq!(remove_command("./sub/link.v2", false, false), r"del .\sub\link.v2");
            assert_eq!(remove_command(r"sub\broken", true, true), "rm sub/broken");
            assert_eq!(remove_command(r"C:\My Files\broken", true, false), r#"rmdir "C:\My Files\broken""#);
            assert_eq!(remove_command(r"C:\My Files\broken", false, true), r#"rm "C:/My Files/broken""#);
        } else {
            for git_bash in [false, true] {
                assert_eq!(remove_command("sub/broken", false, git_bash), "rm sub/broken");
            }
        }
    }

    #[test]
    fn wsls_reparse_tags_name_its_links_and_special_files() {
        // DOCS 3.10, 3.11 (round 21): what Windows shows of WSL's files on \\wsl.localhost.
        assert_eq!(wsl_tag(0xA000_001D), Some(WslFile::Link));
        for special in [0x8000_0023, 0x8000_0024, 0x8000_0025, 0x8000_0026] {
            assert_eq!(wsl_tag(special), Some(WslFile::Special), "{special:x}");
        }
        // Windows' own symlinks and junctions (mount points), and no tag, are none of WSL's.
        for other in [0xA000_000C, 0xA000_0003, 0x8000_0027, 0] {
            assert_eq!(wsl_tag(other), None, "{other:x}");
        }
        // A plain file is no reparse point, so FILE and an import don't look further (the review).
        let f = std::env::temp_dir().join(format!("v2-plain-{}.v2", std::process::id()));
        std::fs::write(&f, "").unwrap();
        assert!(!reparse_point(&std::fs::metadata(&f).unwrap()) && wsl_file(&f).is_none());
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn a_linux_symlink_is_named_as_one() {
        // DOCS 3.10 (round 21): Windows can't follow it wherever it points, so it is never a link
        // to something that isn't there, and the fix is never its removal.
        let lx = LinkProblem { at: PathBuf::from("links/rel.v2"), target: None, looped: false, dir: true, linux: true };
        assert_eq!(lx.problem("'links/rel.v2'"), "'links/rel.v2' is a Linux (WSL) symlink, which Windows can't follow");
        assert_eq!(lx.fix("links/rel.v2"), "use the path it points to (ls -l in WSL shows it), or run V2 inside WSL");
    }

    #[test]
    fn a_relative_link_target_is_from_the_links_folder() {
        // DOCS 3.10 (round 19): `ln -s ../gone sub/broken` points at gone next to sub, not in the
        // current folder; an absolute target is as it is. (No folder here exists.)
        let base = std::env::temp_dir().join(format!("v2-no-such-{}", std::process::id()));
        assert_eq!(link_target(&base.join("sub").join("broken"), Path::new("../gone")), absolute_clean(&base.join("gone")));
        assert_eq!(link_target(&base.join("sub").join("broken"), Path::new("gone")), absolute_clean(&base.join("sub").join("gone")));
        assert_eq!(link_target(&base.join("broken"), &base.join("abs")), base.join("abs"));
        #[cfg(windows)]
        {
            assert_eq!(link_target(&base.join("broken"), Path::new(r"\\?\C:\gone")), PathBuf::from(r"C:\gone"));
            let volume = Path::new(r"\\?\Volume{00000000-0000-0000-0000-000000000000}\gone");
            assert_eq!(link_target(&base.join("broken"), volume), volume.to_path_buf());
        }
        #[cfg(unix)]
        {
            // A real one: its folder as it really is.
            let d = std::env::temp_dir().join(format!("v2rellink-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(d.join("sub")).unwrap();
            std::os::unix::fs::symlink("gone", d.join("sub").join("broken")).unwrap();
            let link = link_problem(&d.join("sub").join("broken").join("main.v2")).expect("the broken link");
            let gone = canonical(&d.join("sub")).unwrap().join("gone");
            assert_eq!(link.problem("'sub/broken'"), format!("'sub/broken' is a link to {}, which isn't there", gone.display()));
            assert_eq!(link.fix("sub/broken"), format!("remove the link (rm sub/broken), or make {}", gone.display()));
            let _ = std::fs::remove_dir_all(&d);
        }
    }
}
