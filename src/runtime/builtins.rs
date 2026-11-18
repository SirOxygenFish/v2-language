//! Builtins and the `Out` writer (INTERNALS 5.10): DOCS 3.11 without `define_engine` (roadmap
//! Phase 3).

use std::cmp::Ordering;
use std::ffi::OsString;
use std::io::{BufRead, IsTerminal, StdoutLock, Write};
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::call::ParamSpec;
use super::error::{fail, type_error, value_error, ErrorValue, Exec, Flow};
use super::interp::Interp;
use super::methods::int_arg;
use super::ops::{binop, handle_hint, order};
use super::value::{float_str, DictMap, Str, Value};
use crate::codec::json;
use crate::engines::process::{self, stop_if_interrupted};
use crate::engines::resolve::{first_program, git_bash, is_drive_relative, Lookup};
use crate::syntax::ast::BinOp;
use crate::syntax::span::{os_message, shown_path, Span};

pub type BuiltinFn = fn(&mut Interp, Vec<Value>, Span) -> Exec<Value>;

pub struct BuiltinDef {
    pub name: &'static str,
    pub params: &'static [ParamSpec],
    pub f: BuiltinFn,
}

/// Builtins that take positional arguments only: `range(start: 5)` would read as an end (5.10).
pub const POSITIONAL_ONLY: &[&str] = &["range"];

pub static BUILTINS: &[BuiltinDef] = &[
    BuiltinDef {
        name: "print",
        params: &[
            ParamSpec::rest("values"),
            ParamSpec::opt("sep", || Value::str(" ")),
            ParamSpec::opt("end", || Value::str("\n")),
            ParamSpec::opt("stderr", || Value::Bool(false)),
        ],
        f: print,
    },
    BuiltinDef {
        name: "error",
        // `message` is checked in `error`, so `error("text")` (JavaScript's `new Error("text")`)
        // can say what the two arguments are.
        params: &[ParamSpec::req("type"), ParamSpec::opt("message", || Value::Null), ParamSpec::opt("data", || Value::Null)],
        f: error,
    },
    BuiltinDef {
        name: "input",
        params: &[ParamSpec::opt("prompt", || Value::str("")), ParamSpec::opt("echo", || Value::Bool(true))],
        f: input,
    },
    BuiltinDef { name: "len", params: &[ParamSpec::req("x")], f: len },
    BuiltinDef { name: "type", params: &[ParamSpec::req("x")], f: |_, a, _| Ok(Value::str(a[0].type_name())) },
    BuiltinDef { name: "str", params: &[ParamSpec::req("x")], f: |it, a, at| Ok(Value::Str(it.str_of(&a[0], at)?.into())) },
    BuiltinDef { name: "int", params: &[ParamSpec::req("x")], f: int },
    BuiltinDef { name: "float", params: &[ParamSpec::req("x")], f: float },
    BuiltinDef { name: "bool", params: &[ParamSpec::req("x")], f: |_, a, _| Ok(Value::Bool(a[0].truthy())) },
    BuiltinDef { name: "range", params: &[ParamSpec::rest("args")], f: range },
    BuiltinDef { name: "json_parse", params: &[ParamSpec::req("text")], f: json_parse },
    BuiltinDef {
        name: "json_stringify",
        params: &[ParamSpec::req("value"), ParamSpec::opt("indent", || Value::Null)],
        f: json_stringify,
    },
    BuiltinDef {
        name: "read_file",
        params: &[ParamSpec::req("path"), ParamSpec::opt("binary", || Value::Bool(false))],
        f: read_file,
    },
    BuiltinDef {
        name: "write_file",
        params: &[ParamSpec::req("path"), ParamSpec::req("data"), ParamSpec::opt("append", || Value::Bool(false))],
        f: write_file,
    },
    BuiltinDef { name: "exists", params: &[ParamSpec::req("path")], f: exists },
    BuiltinDef { name: "list_dir", params: &[ParamSpec::req("path")], f: list_dir },
    BuiltinDef { name: "make_dir", params: &[ParamSpec::req("path")], f: make_dir },
    BuiltinDef { name: "remove_file", params: &[ParamSpec::req("path")], f: remove_file },
    BuiltinDef { name: "script_dir", params: &[], f: script_dir },
    BuiltinDef { name: "exec", params: &[ParamSpec::req("argv"), ParamSpec::opt("input", || Value::Null)], f: exec },
    BuiltinDef { name: "sleep", params: &[ParamSpec::req("seconds")], f: sleep },
    BuiltinDef { name: "time", params: &[], f: time },
    BuiltinDef { name: "min", params: &[ParamSpec::rest("values")], f: |_, a, _| extreme(a, "min", Ordering::Less) },
    BuiltinDef { name: "max", params: &[ParamSpec::rest("values")], f: |_, a, _| extreme(a, "max", Ordering::Greater) },
    BuiltinDef { name: "sum", params: &[ParamSpec::req("xs")], f: sum },
    BuiltinDef { name: "abs", params: &[ParamSpec::req("x")], f: abs },
    BuiltinDef { name: "round", params: &[ParamSpec::req("x"), ParamSpec::opt("digits", || Value::Int(0))], f: round },
    BuiltinDef { name: "div", params: &[ParamSpec::req("a"), ParamSpec::req("b")], f: div },
    BuiltinDef { name: "getenv", params: &[ParamSpec::req("name"), ParamSpec::opt("default", || Value::Null)], f: getenv },
    BuiltinDef {
        name: "args",
        params: &[],
        f: |it, _, _| Ok(Value::list(it.args.iter().map(|a| Value::str(a)).collect())),
    },
    BuiltinDef { name: "exit", params: &[ParamSpec::opt("code", || Value::Int(0))], f: exit },
    BuiltinDef {
        name: "assert",
        params: &[ParamSpec::req("cond"), ParamSpec::opt("message", || Value::str("assertion failed"))],
        f: assert,
    },
    BuiltinDef { name: "to_v2", params: &[ParamSpec::req("h")], f: to_v2 },
    BuiltinDef { name: "engine_available", params: &[ParamSpec::req("tag")], f: engine_available },
];

/// What the program printed and stdout hasn't taken yet. Outside `Out`, so the interrupt path
/// can write it out (6.5).
static PENDING: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// What `Out` holds before it writes, when stdout isn't a terminal.
const OUT_BUFFER: usize = 64 * 1024;
/// How long `Out` holds output when stdout isn't a terminal: a file or a pipe gets a slow
/// program's output as it comes, and a reader that went away is noticed (5.10).
const OUT_HOLD: Duration = Duration::from_millis(100);

fn lock(m: &Mutex<Vec<u8>>) -> MutexGuard<'_, Vec<u8>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Write `buf` to `w`, keeping what it didn't take.
fn drain(w: &mut dyn Write, buf: &mut Vec<u8>) -> std::io::Result<()> {
    let mut done = 0;
    let r = loop {
        if done == buf.len() {
            break w.flush();
        }
        match w.write(&buf[done..]) {
            Ok(0) => break Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => done += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => break Err(e),
        }
    };
    if let Some(&b) = buf[..done].last() {
        LAST_BYTE.store(b, std::sync::atomic::Ordering::Relaxed);
    }
    buf.drain(..done);
    r
}

/// The last byte V2 wrote to stdout or stderr, and to stderr alone (0: none yet), so a report of
/// its own starts on a line of its own (2.4).
static LAST_BYTE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static LAST_ERR_BYTE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Note what a write to stderr ended with (`stderr!`, `print(stderr: true)`).
pub fn wrote_stderr(text: &[u8]) {
    if let Some(&b) = text.last() {
        LAST_BYTE.store(b, std::sync::atomic::Ordering::Relaxed);
        LAST_ERR_BYTE.store(b, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Before a report of V2's own on stderr (an uncaught error, 2.4): a line break when the program
/// left the line it goes on unfinished, as `print("x", end: "")` does. That line is stderr's, or
/// with stdout and stderr one terminal, pipe or file (`2>&1`) the line either wrote last. Call it
/// after `Out` is flushed.
pub fn start_line() {
    let unfinished = |b: u8| b != 0 && b != b'\n';
    let (any, err) = (LAST_BYTE.load(std::sync::atomic::Ordering::Relaxed), LAST_ERR_BYTE.load(std::sync::atomic::Ordering::Relaxed));
    if (unfinished(any) || unfinished(err)) && unfinished(if same_stream() { any } else { err }) {
        stderr!("\n");
    }
}

/// Whether stdout and stderr are the same terminal, pipe or file: on Unix the same device and
/// inode; on Windows both consoles, one kernel object (`2>&1`), or one file opened twice.
fn same_stream() -> bool {
    #[cfg(unix)]
    {
        let id = |fd| {
            // SAFETY: fstat fills a zeroed struct for any descriptor, open or not.
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            (unsafe { libc::fstat(fd, &mut st) } == 0).then_some((st.st_dev, st.st_ino))
        };
        id(1).is_some_and(|a| id(2) == Some(a))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::CompareObjectHandles;
        use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
        use windows_sys::Win32::System::Console::GetConsoleMode;
        let (out, err) = (std::io::stdout().as_raw_handle(), std::io::stderr().as_raw_handle());
        let mut mode = 0;
        // SAFETY: the standard handles stay open for the process; each call only reads them and
        // fills its out-parameter.
        unsafe {
            if (GetConsoleMode(out, &mut mode) != 0 && GetConsoleMode(err, &mut mode) != 0) || CompareObjectHandles(out, err) != 0 {
                return true;
            }
            let id = |h| {
                let mut i: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
                (GetFileInformationByHandle(h, &mut i) != 0).then_some((i.dwVolumeSerialNumber, i.nFileIndexHigh, i.nFileIndexLow))
            };
            id(out).is_some_and(|a| id(err) == Some(a))
        }
    }
}

/// The interrupt path (6.5): write what the program printed straight to the descriptor (the
/// interpreter thread holds `stdout`'s lock), from a thread of its own, so a reader that stopped
/// reading can't hold the exit longer than `wait`.
pub fn write_pending(wait: Duration) {
    let (done, finished) = std::sync::mpsc::channel();
    let writer = crate::engines::process::spawn_helper(std::thread::Builder::new(), move || {
        let mut pending = lock(&PENDING);
        if !pending.is_empty() {
            let _ = raw_stdout().write_all(&pending);
            pending.clear();
        }
        let _ = done.send(());
    });
    if writer.is_ok() {
        let _ = finished.recv_timeout(wait);
    }
}

/// The thread that writes what `Out` held once it is `OUT_HOLD` old, so a line printed before a
/// long computation reaches a file or a pipe while it runs (5.10). It writes under `PENDING`'s
/// lock, as `Out` does, so nothing changes order; a failed write leaves the bytes for `Out`'s next
/// write, which reports it. Started at the first held write.
fn flusher() -> Option<&'static std::thread::Thread> {
    static FLUSHER: std::sync::OnceLock<Option<std::thread::Thread>> = std::sync::OnceLock::new();
    FLUSHER
        .get_or_init(|| {
            let run = || loop {
                std::thread::park();
                std::thread::sleep(OUT_HOLD);
                let mut pending = lock(&PENDING);
                if !pending.is_empty() {
                    let _ = drain(&mut *raw_stdout(), &mut pending);
                }
            };
            crate::engines::process::spawn_helper(std::thread::Builder::new().name("v2-flush".into()), run).ok().map(|h| h.thread().clone())
        })
        .as_ref()
}

/// Stdout's descriptor as a file that is never closed.
fn raw_stdout() -> std::mem::ManuallyDrop<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        // SAFETY: fd 1 stays open for the process, and `ManuallyDrop` never closes it.
        std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(std::io::stdout().as_raw_fd()) })
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        // SAFETY: the standard handle stays open for the process, and `ManuallyDrop` never closes it.
        std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_handle(std::io::stdout().as_raw_handle()) })
    }
}

/// The only writer of the host's stdout: buffered, flushed after every write when stdout is a
/// terminal, otherwise by size and age (`OUT_HOLD`), and at the end of the run (5.10).
pub struct Out {
    w: Box<dyn Write>,
    tty: bool,
    pending: &'static Mutex<Vec<u8>>,
    /// When `pending` was last written out.
    flushed: std::time::Instant,
    /// Output that couldn't be written and whose `IOError` never reached the program (a block's
    /// own error won): the end of the run reports it (6.5).
    pub lost: Option<Rc<str>>,
}

impl Out {
    pub fn new() -> Out {
        let stdout = std::io::stdout();
        let tty = stdout.is_terminal();
        let lock: StdoutLock<'static> = stdout.lock();
        Out { w: Box::new(lock), tty, pending: &PENDING, flushed: std::time::Instant::now(), lost: None }
    }

    /// An `Out` that collects into a shared buffer (tests).
    #[cfg(test)]
    pub fn capture() -> (Out, std::rc::Rc<std::cell::RefCell<Vec<u8>>>) {
        struct Shared(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
        impl Write for Shared {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.borrow_mut().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let pending = Box::leak(Box::new(Mutex::new(Vec::new())));
        (Out { w: Box::new(Shared(buf.clone())), tty: true, pending, flushed: std::time::Instant::now(), lost: None }, buf)
    }

    pub fn write_str(&mut self, s: &str) -> Exec<()> {
        self.write_bytes(s.as_bytes())
    }

    /// Bytes as they are (`write_file("/dev/stdout", b"…")`, 5.10).
    pub fn write_bytes(&mut self, s: &[u8]) -> Exec<()> {
        let mut pending = lock(self.pending);
        pending.extend_from_slice(s);
        if !self.tty && pending.len() < OUT_BUFFER && self.flushed.elapsed() < OUT_HOLD {
            if let Some(t) = flusher() {
                t.unpark();
            }
            return Ok(());
        }
        let r = drain(&mut *self.w, &mut pending);
        drop(pending);
        self.flushed = std::time::Instant::now();
        self.raise(r)
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        let r = drain(&mut *self.w, &mut lock(self.pending));
        self.flushed = std::time::Instant::now();
        r
    }

    /// A flush on the way (before a wait, a child, stderr text or a frame to a worker): a reader
    /// that went away stops the run, as at a print; another failure waits for the next write or
    /// the end of the run (6.5).
    pub fn flush_or_stop(&mut self) {
        if let Err(e) = self.flush() {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                crate::engines::process::exit_broken_pipe();
            }
        }
    }

    /// Drop what is waiting: the rest of the output of a request whose write failed (9.9).
    pub fn discard(&mut self) {
        lock(self.pending).clear();
    }

    /// A failed write's `IOError`. What it couldn't write goes with it: a program that catches
    /// it has handled that output, and the end of the run doesn't count it as lost (6.5).
    fn raise(&mut self, r: std::io::Result<()>) -> Exec<()> {
        let Err(e) = r else { return Ok(()) };
        // Its reader went away (`| head`): the run stops quietly (6.5).
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            crate::engines::process::exit_broken_pipe();
        }
        lock(self.pending).clear();
        fail("IOError", format!("cannot write to stdout: {}", os_message(&e)))
    }
}

/// A child's output as text (5.10): UTF-8 as it is. On Windows, output that isn't UTF-8 is what
/// console programs (`cmd /c dir`) write in the console's code page (850, 437, …), and is decoded
/// with it; what is left invalid becomes U+FFFD.
fn child_text(bytes: &[u8]) -> String {
    if let Some(s) = utf16_text(bytes) {
        return s;
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    #[cfg(windows)]
    if let Some(s) = console_text(bytes) {
        return s;
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// A child's output in UTF-16 (`cmd /u`, `wsl --list`, a program that set its console to
/// Unicode): it starts with a byte order mark, or, on Windows, at least half of its odd bytes
/// are NUL and its even ones nearly never are, as Latin text in UTF-16LE is. Text in UTF-8 or a
/// code page has no NULs (5.10).
fn utf16_text(bytes: &[u8]) -> Option<String> {
    let decode = |b: &[u8], le: bool| {
        let (pairs, tail) = b.split_at(b.len() & !1);
        let units: Vec<u16> = pairs.chunks(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        let mut s = String::from_utf16_lossy(&units);
        if !tail.is_empty() {
            s.push('\u{FFFD}');
        }
        s
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return Some(decode(rest, true));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return Some(decode(rest, false));
    }
    if !cfg!(windows) || bytes.len() < 2 || !bytes.len().is_multiple_of(2) {
        return None;
    }
    let odd = bytes.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
    let even = bytes.iter().step_by(2).filter(|&&b| b == 0).count();
    (odd * 2 >= bytes.len() / 2 && even * 4 <= odd).then(|| decode(bytes, true))
}

/// `bytes` in the console's output code page, or the OEM code page with no console; `None` for
/// a UTF-8 console, whose programs write UTF-8.
#[cfg(windows)]
fn console_text(bytes: &[u8]) -> Option<String> {
    use windows_sys::Win32::Globalization::{GetOEMCP, MultiByteToWideChar};
    use windows_sys::Win32::System::Console::GetConsoleOutputCP;
    // SAFETY: neither takes arguments or has preconditions.
    let cp = match unsafe { GetConsoleOutputCP() } {
        0 => unsafe { GetOEMCP() },
        cp => cp,
    };
    let len = i32::try_from(bytes.len()).ok().filter(|_| cp != 65001)?;
    // SAFETY: the input is `len` readable bytes; a null output with size 0 asks for the length.
    let n = unsafe { MultiByteToWideChar(cp, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0) };
    let mut wide = vec![0u16; usize::try_from(n).ok().filter(|&n| n > 0)?];
    // SAFETY: `wide` holds the `n` units the first call asked for.
    let m = unsafe { MultiByteToWideChar(cp, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), n) };
    wide.truncate(usize::try_from(m).ok().filter(|&m| m > 0)?);
    Some(String::from_utf16_lossy(&wide))
}

/// `to_v2(h)`: a handle's object as plain data, converted inside its worker; anything else
/// unchanged (containers are not walked) (5.10, 11.5).
fn to_v2(it: &mut Interp, mut args: Vec<Value>, at: Span) -> Exec<Value> {
    match args.pop().unwrap_or(Value::Null) {
        Value::Handle(h) => it.handle_value(&h, at),
        other => Ok(other),
    }
}

/// `engine_available(tag)`: false for an unknown tag, else whether the engine resolves; it may
/// probe (cached) but never starts a session (7.3).
fn engine_available(it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let tag = str_arg(&args[0], "engine_available", "tag")?;
    Ok(Value::Bool(it.registry.engine_available(&tag)))
}

fn io_error<T>(message: String) -> Exec<T> {
    fail("IOError", message)
}

/// A file builtin's OS failure (5.10): `cannot read 'x': <why>`, a folder said as such, and for
/// a path holding a control character the hint that `\n` and `\t` in `"…"` are escapes.
fn file_error<T>(verb: &str, path: &str, e: &std::io::Error) -> Exec<T> {
    file_error_hint(verb, path, e, None)
}

/// `file_error`, with a hint of the caller's when the path itself needs none.
fn file_error_hint<T>(verb: &str, path: &str, e: &std::io::Error, hint: Option<String>) -> Exec<T> {
    // Windows: a pipe or a device (`\\.\pipe\x`): each look at the path below would open it
    // again, and each open of a pipe takes one of its server's instances (5.10).
    if device_path(path) {
        return root_error(verb, path, os_message(e));
    }
    // On Windows /dev/stdin and the others are Linux's names, whatever the current folder: on a
    // \\wsl.localhost share they are WSL's links, and their hint is Windows' way, below.
    if !(cfg!(windows) && matches!(path, "/dev/stdin" | "/dev/stdout" | "/dev/stderr")) {
        super::module::link_error(verb, path).map_or(Ok(()), |err| Err(Flow::Error(Rc::new(err))))?;
    }
    if let Some(why) = unreachable_root(path, e) {
        return root_error(verb, path, why);
    }
    // A name longer than Windows takes, which it says as a bad name or a folder that isn't there
    // (so a make_dir hint would fail too).
    if let Some(why) = long_name(path) {
        let err = ErrorValue::new("IOError", format!("cannot {verb} '{}': {why}", shown_path(path)));
        let _ = err.hint.set("shorten it".into());
        return Err(Flow::Error(Rc::new(err)));
    }
    let hint = hint.or_else(|| tmp_hint(path)).or_else(|| read_only_hint(verb, path, e));
    // A file where a folder goes, or the other way round: Windows says "Access is denied", "The
    // directory name is invalid" or "Cannot create a file when that file already exists".
    let p = std::path::Path::new(path);
    // A file where the path goes on (`notes.txt/sub`), or a folder that isn't there: the OS's
    // words for them ("The system cannot find the path specified") don't say which part.
    let in_the_way = p.ancestors().skip(1).find(|a| !a.as_os_str().is_empty() && a.exists()).filter(|a| a.is_file());
    let no_folder = p.parent().filter(|d| !d.as_os_str().is_empty() && !as_folder(d).exists() && e.kind() == std::io::ErrorKind::NotFound);
    // A link to a folder is no folder to remove, and shutil.rmtree refuses one (or, given
    // `link/`, empties the folder it points to): `link/` names the folder, and the link alone is
    // removed without the slash.
    let link = if verb == "remove" { folder_link(path) } else { None };
    let slash = super::module::trim_separators(path).len() < path.len();
    let why = match verb {
        "remove" if link.is_some() && slash => format!("it is a link to {}, a folder, and remove_file removes files only", link.as_ref().map(|to| shown_path(&to.to_string_lossy())).unwrap_or_default()),
        "remove" if link.is_some() => os_message(e),
        "remove" if p.is_dir() => "it is a folder, and remove_file removes files only".to_string(),
        "read" | "write" if p.is_dir() => "it is a folder".to_string(),
        "list" if p.is_file() => "it is a file, not a folder".to_string(),
        "create" if p.is_file() => "a file has that name".to_string(),
        _ if in_the_way.is_some() => format!("'{}' is a file, not a folder", in_the_way.map(|a| shown_path(&a.to_string_lossy())).unwrap_or_default()),
        _ if no_folder.is_some() => format!("there is no folder '{}'", no_folder.map(|d| shown_path(&d.to_string_lossy())).unwrap_or_default()),
        _ => os_message(e),
    };
    let err = ErrorValue::new("IOError", format!("cannot {verb} '{}': {why}", shown_path(path)));
    let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let tilde = path == "~" || path.starts_with("~/") || path.starts_with("~\\");
    if path.chars().any(char::is_control) {
        let _ = err
            .hint
            .set(r#"in "…", a backslash starts an escape (\n is a line break, \t a tab): write Windows paths with / or \\, as in "notes/new.txt""#.into());
    } else if let Some(h) = lossy_hint(path) {
        let _ = err.hint.set(h.into());
    } else if let Some(h) = hint {
        let _ = err.hint.set(h.into());
    } else if cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33 | 1224)) {
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION, ERROR_USER_MAPPED_FILE: the OS says only
        // that a process uses it.
        let _ = err.hint.set(busy_hint(verb).into());
    } else if cfg!(windows) && verb == "remove" && e.raw_os_error() == Some(5) && program_file(p) {
        // A running program's .exe (or a .dll it loaded) can't be removed; 5 also comes from a
        // folder's or file's permissions, hence "if".
        let _ = err.hint.set("if that program is running, stop it first".into());
    } else if link.is_some() && slash {
        let mut q = String::new();
        super::value::quoted(super::module::trim_separators(path), &mut q);
        let _ = err.hint.set(format!("to remove the link alone, leave out the slash at its end: remove_file({q})").into());
    } else if verb == "remove" && link.is_none() && p.is_dir() {
        let _ = err.hint.set("a @py block removes a folder with what is in it: @py(path) { import shutil; shutil.rmtree(path) }".into());
    } else if cfg!(windows) && matches!(path, "/dev/stdin" | "/dev/stdout" | "/dev/stderr") {
        // Linux's and macOS's names for the standard streams.
        let _ = err.hint.set("/dev/stdin, /dev/stdout and /dev/stderr are Linux's and macOS's: on Windows read standard input with input(), a line at a time, and write with print() and print(…, stderr: true)".into());
    } else if let Some(h) = msys_hint(path).filter(|_| e.kind() == std::io::ErrorKind::NotFound) {
        let _ = err.hint.set(h.into());
    } else if let Some(h) = linux_path_hint(path).filter(|_| e.kind() == std::io::ErrorKind::NotFound) {
        let _ = err.hint.set(h.into());
    } else if let Some(h) = unicode_twin_hint(path, e) {
        let _ = err.hint.set(h.into());
    } else if let Some(h) = stand_in_hint(verb, path) {
        let _ = err.hint.set(h.into());
    } else if bad_windows_name(path) {
        let _ = err.hint.set(format!("{BAD_NAME}: choose another name").into());
    } else if tilde && !p.exists() {
        // A shell expands them, V2 doesn't (5.10); `$env:X\…` fails as a bad name, not as missing.
        let write = if path == "~" { format!("getenv(\"{home}\")") } else { format!("getenv(\"{home}\") + \"{}\"", &path[1..]) };
        let _ = err.hint.set(format!("V2 doesn't expand ~ in paths: write {write}").into());
    } else if let Some(var) = crate::config::unexpanded(path).filter(|_| !p.exists()) {
        let name = var.trim_matches(['%', '$', '{', '}']).trim_start_matches("env:");
        let _ = err.hint.set(format!("V2 doesn't expand {var} in paths: write getenv(\"{}\") in its place", home_var(name)).into());
    } else if let Some(d) = no_folder.filter(|_| verb == "write") {
        let mut folder = String::new();
        super::value::quoted(&d.to_string_lossy(), &mut folder);
        let _ = err.hint.set(format!("make the folder first, with make_dir({folder})").into());
    } else if e.raw_os_error() == Some(27) && cfg!(unix) {
        // EFBIG: the file size limit (V2 ignores SIGXFSZ, so the write fails instead).
        let _ = err.hint.set("the file size limit (ulimit -f) stops it: raise the limit, or write less".into());
    }
    Err(Flow::Error(Rc::new(err)))
}

/// Windows: the hint for a file another process has open without sharing, locked or mapped
/// (5.10): V2's own blocks are processes too. Only a write can go to another name.
fn busy_hint(verb: &str) -> String {
    let close = "another program has it open (Excel keeps an open file locked; so does a block of this script that opened it): close it there";
    if verb == "write" {
        format!("{close}, or write to another file name")
    } else {
        close.to_string()
    }
}

/// Whether `p` names a program or a library by its extension (`.exe`, `.dll`), which Windows
/// keeps while it runs.
fn program_file(p: &std::path::Path) -> bool {
    p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe") || x.eq_ignore_ascii_case("dll"))
}

/// Where `path` (without the separators at its end) points when it is a link to a folder that is
/// there (a symlink, a junction on Windows), as messages show it (`module::link_target`) (5.10).
fn folder_link(path: &str) -> Option<std::path::PathBuf> {
    let link = std::path::Path::new(super::module::trim_separators(path));
    if link.as_os_str().is_empty() || !std::fs::symlink_metadata(link).ok()?.file_type().is_symlink() || !link.is_dir() {
        return None;
    }
    Some(super::module::link_target(link, &std::fs::read_link(link).ok()?))
}

/// Windows: what is wrong with a part of `path` longer than a Windows file or folder name can be,
/// 255 UTF-16 units (an emoji is 2): `the name 'nnnnnnnnnnnnnnnnnnnn…' is 256 characters long, and
/// a Windows file or folder name can have at most 255` (5.10).
fn long_name(path: &str) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let (part, units) = path.split(['/', '\\']).map(|p| (p, p.encode_utf16().count())).find(|&(_, n)| n > 255)?;
    let start: String = part.chars().take(20).collect();
    let counted = if units == part.chars().count() { "" } else { " as Windows counts them (an emoji is 2)" };
    Some(format!("the name '{}…' is {units} characters long{counted}, and a Windows file or folder name can have at most 255", shown_path(&start)))
}

/// Windows: a path in the device namespace that names no file: `\\.\pipe\x`, `\\.\COM1`,
/// `\\?\pipe\x` (either slash), not a drive's, a share's (`UNC`) or a volume's path after it
/// (5.10). Each look at such a path opens it, and each open of a named pipe takes an instance.
fn device_path(path: &str) -> bool {
    let b = path.as_bytes();
    let sep = |c: u8| c == b'\\' || c == b'/';
    if !cfg!(windows) || b.len() < 4 || !sep(b[0]) || !sep(b[1]) || !matches!(b[2], b'.' | b'?') || !sep(b[3]) {
        return false;
    }
    let first = path[4..].split(['/', '\\']).next().unwrap_or_default();
    let drive = first.len() == 2 && first.as_bytes()[0].is_ascii_alphabetic() && first.as_bytes()[1] == b':';
    !(drive || first.eq_ignore_ascii_case("UNC") || first.get(..7).is_some_and(|v| v.eq_ignore_ascii_case("Volume{")))
}

/// A path holding U+FFFD, which `args()` and `getenv()` put where bytes weren't UTF-8 (5.10): it
/// names no file, so what to do when an argument or variable of this run had such bytes.
fn lossy_hint(path: &str) -> Option<String> {
    if !path.contains('\u{FFFD}') {
        return None;
    }
    let what = if std::env::args_os().any(|a| a.to_str().is_none()) {
        "an argument"
    } else if std::env::vars_os().any(|(_, v)| v.to_str().is_none()) {
        "an environment variable"
    } else {
        return None;
    };
    Some(format!("the path holds U+FFFD where {what} wasn't UTF-8: rename the file to a UTF-8 name, or use a UTF-8 locale"))
}

/// A file that isn't there under its name, but is in another Unicode form (`daten_ü.csv` copied
/// from a Mac, its `ü` a `u` plus an accent), or a folder on its way that is: `module::unicode_twin`'s
/// hint (5.10), before the `make_dir` one, which would make a second folder that looks the same.
fn unicode_twin_hint(path: &str, e: &std::io::Error) -> Option<String> {
    let p = std::path::Path::new(path);
    let name = p.file_name()?.to_str().filter(|_| e.kind() == std::io::ErrorKind::NotFound)?;
    let folder = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    super::module::unicode_twin(folder, name).or_else(|| super::module::folder_twin(p)).map(|h| shown_path(&h))
}

/// Windows: `/c/Users/me/x.txt`, Git Bash's spelling of `C:/Users/me/x.txt`, or `/mnt/c/…`, WSL's
/// (`unix_drive`): what to write instead (5.10).
fn msys_hint(path: &str) -> Option<String> {
    let (written, windows, whose) = unix_drive(path)?;
    let mut write = String::new();
    super::value::quoted(&windows, &mut write);
    Some(format!("on Windows, {}/… is {whose} spelling of {}/…: write {write}", written.to_ascii_lowercase(), &windows[..2]))
}

/// Windows: a path in Git Bash's spelling, `/c/Users/me/x.txt` (one letter after the first `/`),
/// or WSL's, `/mnt/c/Users/me/x.txt`, of `C:/Users/me/x.txt`, where that drive exists and this
/// drive has no such folder (`\c`, `\mnt\c`): the part that names the drive as written, the
/// Windows spelling, and whose spelling it is (5.10, 19.1).
pub(crate) fn unix_drive(path: &str) -> Option<(&str, String, &'static str)> {
    let (at, whose) = if path.starts_with("/mnt/") { (5, "WSL's") } else { (1, "Git Bash's") };
    let b = path.as_bytes();
    if !cfg!(windows) || b.first() != Some(&b'/') || !b.get(at).is_some_and(u8::is_ascii_alphabetic) || !matches!(b.get(at + 1), None | Some(b'/')) {
        return None;
    }
    let drive = format!("{}:", (b[at] as char).to_ascii_uppercase());
    let written = &path[..at + 1];
    if !std::path::Path::new(&format!("{drive}/")).is_dir() || std::path::Path::new(written).exists() {
        return None;
    }
    Some((written, format!("{drive}{}", if path.len() > at + 1 { &path[at + 1..] } else { "/" }), whose))
}

/// Windows: a Linux path that isn't there, whose first folder is one of Linux's own (`/root`,
/// `/home`, `/usr`, `/etc`, `/opt`, `/srv`, `/var`) and not on this drive: where WSL's files are
/// from Windows (5.10, 19.1).
pub(crate) fn linux_path_hint(path: &str) -> Option<String> {
    let first = path.strip_prefix('/')?.split('/').next()?;
    if !cfg!(windows) || !matches!(first, "root" | "home" | "usr" | "etc" | "opt" | "srv" | "var") || std::path::Path::new(&format!("/{first}")).exists() {
        return None;
    }
    Some(r"a Linux path: from Windows, WSL's files are \\wsl.localhost\<distro>\…".to_string())
}

/// Windows: a part of `path` that Windows refuses as a file name: a device (`CON`, `PRN`, `AUX`,
/// `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`, when the path through that part is one: on Windows 11 a
/// bare name, and `NUL` in any folder; Windows 10 also in a folder or with an extension, as
/// `sub\aux` or `con.txt`), one that ends in a dot or a space, or one holding `< > " | ? *`
/// (5.10). A `\\?\` or `\\.\` path is taken as written.
fn bad_windows_name(path: &str) -> bool {
    if !cfg!(windows) || path.starts_with(r"\\?\") || path.starts_with(r"\\.\") {
        return false;
    }
    let after_drive = if path.as_bytes().get(1) == Some(&b':') { &path[2..] } else { path };
    // Where the part being looked at ends in `path`: the device check takes the path up to it.
    let mut end = path.len() - after_drive.len();
    after_drive.split(['/', '\\']).any(|part| {
        end += part.len();
        let through = &path[..end];
        let last = end >= path.len();
        end += 1;
        let stem = part.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
        let device_name = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && matches!(stem.as_bytes()[3], b'1'..=b'9'));
        // A bare `aux` is the device, but in `aux/x.txt` Windows 11 reads a folder `aux`, which
        // `make_dir("./aux")` can make: a folder there is no bad name.
        let device = device_name && windows_device(through) && (last || !as_folder(std::path::Path::new(through)).is_dir());
        device || (!matches!(part, "." | "..") && (part.ends_with('.') || part.ends_with(' '))) || part.contains(['<', '>', '"', '|', '?', '*'])
    })
}

/// A folder of a longer path as Windows reads it there: a relative `aux` with `.\` before it,
/// as Windows 11 takes a bare `aux` for the device but `aux\x.txt` for a file in a folder (5.10).
fn as_folder(d: &std::path::Path) -> std::borrow::Cow<'_, std::path::Path> {
    if cfg!(windows) && matches!(d.components().next(), Some(std::path::Component::Normal(_))) {
        std::path::Path::new(".").join(d).into()
    } else {
        d.into()
    }
}

/// Whether Windows takes the path `name` for a device, as its full path says (`\\.\CON`):
/// Windows 11 takes `con.txt` and `sub\aux` for a file.
#[cfg(windows)]
fn windows_device(name: &str) -> bool {
    use windows_sys::Win32::Storage::FileSystem::GetFullPathNameW;
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let mut buf = [0u16; 1024];
    // SAFETY: a NUL-terminated name and a buffer of the length given.
    let n = unsafe { GetFullPathNameW(wide.as_ptr(), buf.len() as u32, buf.as_mut_ptr(), std::ptr::null_mut()) } as usize;
    n > 0 && n < buf.len() && String::from_utf16_lossy(&buf[..n]).starts_with(r"\\.\")
}

#[cfg(not(windows))]
fn windows_device(_name: &str) -> bool {
    false
}

/// What Windows refuses as a file name, for the hints that say so (5.10).
const BAD_NAME: &str = r#"Windows doesn't allow this file name (CON, PRN, AUX, NUL, COM1 to COM9 and LPT1 to LPT9 are devices; a name can't end in a dot or a space, or hold < > " | ? *)"#;

/// Windows: a path that would reach something other than what it names, refused before the OS
/// sees it (5.10). A part (not `.` or `..`) that ends in a dot or a space, which Windows drops
/// (`remove_file("data.txt.")` would delete `data.txt`), and a path that Windows makes a device
/// (`GetFullPathNameW` gives `\\.\CON`: a bare `CON`, `AUX`, `PRN`, `COM1`-`COM9` or `LPT1`-`LPT9`
/// on Windows 11, `NUL` in any folder), where `read_file("CON")` would wait for typed input and
/// `write_file("nul")` would throw the data away. A `\\?\` or `\\.\` path is taken as written:
/// when such a name is really there (WSL and Macs make them; for `dir./new.txt`, the folder
/// `dir.`), the hint shows that way to reach it.
fn windows_name(verb: &str, path: &str) -> Exec<()> {
    if !cfg!(windows) || path.starts_with(r"\\?\") || path.starts_with(r"\\.\") {
        return Ok(());
    }
    // A device: what must be there for `\\?\` to reach it is its whole path.
    let (why, there) = match (dotted_name(path), device_name(path)) {
        (Some(dotted), _) => dotted,
        (None, Some(device)) => (format!("{} is the name of a device on Windows", shown_path(&device)), verbatim(path).filter(|v| std::fs::symlink_metadata(v).is_ok())),
        (None, None) => return Ok(()),
    };
    let err = ErrorValue::new("IOError", format!("cannot {verb} '{}': {why}", shown_path(path)));
    let builtin = builtin_name(verb);
    let hint = match there {
        Some(v) => {
            let mut q = String::new();
            super::value::quoted(&v, &mut q);
            format!(r"{BAD_NAME}, but one of that name is there (WSL and Macs can make one): reach it by its full path with \\?\ in front, as in {builtin}({q})")
        }
        None => format!("{BAD_NAME}: choose another name"),
    };
    let _ = err.hint.set(hint.into());
    Err(Flow::Error(Rc::new(err)))
}

/// Windows: a `\\?\` path with its `/` read as `\` and its `.` and `..` parts taken out, as
/// Windows reads any other path, though it passes a `\\?\` one on as written: no Windows name is
/// `.` or `..` or holds `/`, so `f"${script_dir()}/data.csv"` works in a script run by its `\\?\`
/// path (2.1, 5.10). Other paths come back as they are.
fn verbatim_parts(path: &str) -> std::borrow::Cow<'_, str> {
    let Some(rest) = path.strip_prefix(r"\\?\").filter(|_| cfg!(windows)) else { return path.into() };
    let parts: Vec<&str> = rest.split(['/', '\\']).collect();
    if !rest.contains('/') && !parts.iter().any(|p| matches!(*p, "." | "..")) {
        return path.into();
    }
    // The root: `C:`, `Volume{…}`, or `UNC\server\share`.
    let root = if parts[0].eq_ignore_ascii_case("UNC") { 3 } else { 1 };
    let mut kept: Vec<&str> = parts.iter().take(root).copied().collect();
    for part in parts.iter().skip(root) {
        match *part {
            "" | "." => {}
            ".." => {
                if kept.len() > root {
                    kept.pop();
                }
            }
            p => kept.push(p),
        }
    }
    // A path that ends at its root keeps the `\` after it (`\\?\C:\`).
    let tail = if kept.len() == root && root == 1 { r"\" } else { "" };
    format!(r"\\?\{}{tail}", kept.join(r"\")).into()
}

/// Windows: exec's arguments after the program that are `\\?\` paths, as `verbatim_parts` reads
/// them (`f"${script_dir()}/x.py"` in a script run by its `\\?\` path, which Python can't open
/// with its `/`); other arguments reach the program as given, and cmd's command line after `/c`
/// (`raw_tail`) stays as written (2.1, 5.10).
fn verbatim_arguments(argv: &mut [OsString], raw_tail: bool) {
    let last = argv.len().saturating_sub(1);
    for (i, arg) in argv.iter_mut().enumerate().skip(1) {
        if raw_tail && i == last {
            continue;
        }
        let read = arg.to_str().filter(|s| s.starts_with(r"\\?\")).and_then(|s| match verbatim_parts(s) {
            std::borrow::Cow::Owned(p) => Some(p),
            std::borrow::Cow::Borrowed(_) => None,
        });
        if let Some(p) = read {
            *arg = p.into();
        }
    }
}

/// The file builtin whose messages say `cannot {verb}`.
fn builtin_name(verb: &str) -> &'static str {
    match verb {
        "read" => "read_file",
        "write" => "write_file",
        "check" => "exists",
        "list" => "list_dir",
        "create" => "make_dir",
        _ => "remove_file",
    }
}

/// Windows: for a path with a part (not `.` or `..`) that ends in a dot or a space, which Windows
/// drops (`data.txt.` is `data.txt` to it), the reason it is refused, `a Windows file name can't
/// end in a dot or a space ('data.txt.' would be 'data.txt')`, and the `\\?\` path that reaches it
/// when one of that name is there (`verbatim`; WSL and Macs make them: the path through its last
/// part that ends so, `through_last_dotted`, so for `dir./new.txt` the folder `dir.`); for the file
/// builtins (`windows_name`), an import and FILE (5.10, 6.4, 19.1). A `\\?\` or `\\.\` path is
/// taken as written.
pub(crate) fn dotted_name(path: &str) -> Option<(String, Option<String>)> {
    if !cfg!(windows) || path.starts_with(r"\\?\") || path.starts_with(r"\\.\") {
        return None;
    }
    let after_drive = if path.as_bytes().get(1) == Some(&b':') { &path[2..] } else { path };
    let part = after_drive.split(['/', '\\']).find(|part| !matches!(*part, "." | "..") && (part.ends_with('.') || part.ends_with(' ')))?;
    let kept = part.trim_end_matches(['.', ' ']);
    let would = if kept.is_empty() { String::new() } else { format!(" ('{}' would be '{}')", shown_path(part), shown_path(kept)) };
    let there = verbatim(path).filter(|_| verbatim(through_last_dotted(path)).is_some_and(|t| std::fs::symlink_metadata(t).is_ok()));
    Some((format!("a Windows file name can't end in a dot or a space{would}"), there))
}

/// Windows: a path that names with `: * ? " < > |` what WSL made from a Linux name with them
/// (`module::stand_in_twin`: `mod:x.v2`, which Windows shows as `mod\u{f03a}x.v2`): how to reach
/// it, `WSL made one of that name, which Windows shows with \u{f03a} for ':': reach it as
/// read_file("…mod\u{f03a}x.v2"), or rename it in WSL` (5.10).
fn stand_in_hint(verb: &str, path: &str) -> Option<String> {
    let twin = super::module::stand_in_twin(path)?;
    let held = super::module::after_root(path)?.chars().find(|c| super::module::HELD.contains(c))?;
    let mut q = String::new();
    super::value::quoted(&twin, &mut q);
    Some(format!("WSL made one of that name, which Windows shows with \\u{{{:x}}} for '{held}': reach it as {}({q}), or rename it in WSL", 0xF000 + held as u32, builtin_name(verb)))
}

/// `path` up to the end of its last part (not `.` or `..`) that ends in a dot or a space:
/// `dir./x.` for `dir./x./new.txt` (5.10); the whole path when it has none.
fn through_last_dotted(path: &str) -> &str {
    let after_drive = if path.as_bytes().get(1) == Some(&b':') { &path[2..] } else { path };
    let mut end = path.len() - after_drive.len();
    let mut last = path.len();
    for part in after_drive.split(['/', '\\']) {
        end += part.len();
        if !matches!(part, "." | "..") && (part.ends_with('.') || part.ends_with(' ')) {
            last = end;
        }
        end += 1;
    }
    &path[..last]
}

/// Windows: the device a path names, as its full path says (`\\.\CON` gives `CON`) (5.10).
#[cfg(windows)]
fn device_name(path: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetFullPathNameW;
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let mut buf = [0u16; 1024];
    // SAFETY: a NUL-terminated name and a buffer of the length given.
    let n = unsafe { GetFullPathNameW(wide.as_ptr(), buf.len() as u32, buf.as_mut_ptr(), std::ptr::null_mut()) } as usize;
    // 0 is a failure, and a length that doesn't fit is the size it would need.
    let full = String::from_utf16_lossy(buf.get(..n).filter(|_| n > 0 && n < buf.len())?);
    full.strip_prefix(r"\\.\").map(str::to_string)
}

#[cfg(not(windows))]
fn device_name(_path: &str) -> Option<String> {
    None
}

/// Windows: `path` made absolute with `.` and `..` taken out by its text and `\\?\` in front, so
/// that nothing is dropped from its parts (`notes.txt ` keeps its space); `None` for a path
/// relative to another drive's folder (`D:notes.txt`) (5.10).
fn verbatim(path: &str) -> Option<String> {
    let given = path.replace('/', "\\");
    let b = given.as_bytes();
    let abs = if given.starts_with(r"\\") || (b.get(1) == Some(&b':') && b.get(2) == Some(&b'\\')) {
        given
    } else if b.get(1) == Some(&b':') {
        return None;
    } else {
        let cwd = std::env::current_dir().ok()?.to_str()?.to_string();
        if cwd.starts_with(r"\\?\") {
            return None;
        }
        // The top of the current drive, or of the share (`\\server\share`) the cwd is on.
        let top = match cwd.strip_prefix(r"\\") {
            Some(unc) => {
                let mut it = unc.splitn(3, '\\');
                format!(r"\\{}\{}", it.next()?, it.next()?)
            }
            None => cwd.get(..2)?.to_string(),
        };
        match given.strip_prefix('\\') {
            // `\x`: at the top of the current drive.
            Some(rest) => format!(r"{top}\{rest}"),
            None => format!(r"{}\{given}", cwd.trim_end_matches('\\')),
        }
    };
    let (root, rest) = match abs.strip_prefix(r"\\") {
        Some(unc) => {
            let mut it = unc.splitn(3, '\\');
            let (server, share) = (it.next()?, it.next()?);
            (format!(r"\\?\UNC\{server}\{share}"), it.next().unwrap_or_default().to_string())
        }
        None => (format!(r"\\?\{}", abs.get(..2)?), abs.get(3..).unwrap_or_default().to_string()),
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split('\\') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    Some(format!(r"{root}\{}", parts.join(r"\")))
}

/// Windows: a path on a drive that isn't there (an unplugged USB drive, `x:y.txt` with no `X:`)
/// or on a mapped drive or share that can't be reached: what to say instead of a folder to make
/// (5.10).
fn unreachable_root(path: &str, e: &std::io::Error) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let p = path.replace('/', "\\");
    let p = match (p.strip_prefix(r"\\?\UNC\"), p.strip_prefix(r"\\?\")) {
        (Some(unc), _) => format!(r"\\{unc}"),
        (None, Some(rest)) => rest.to_string(),
        (None, None) => p,
    };
    let b = p.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        let letter = (b[0] as char).to_ascii_uppercase();
        return drive_problem(letter, drive_type(letter), e);
    }
    let unc = p.strip_prefix(r"\\").filter(|r| !r.starts_with(['?', '.']))?;
    if let Some(why) = wsl_share(unc, e) {
        return Some(why);
    }
    let mut parts = unc.split('\\');
    let (server, share) = (parts.next().filter(|s| !s.is_empty())?, parts.next().filter(|s| !s.is_empty())?);
    network_error(e).then(|| format!(r"the share \\{}\{} can't be reached (is that computer on and connected, and is the share's name right?)", shown_path(server), shown_path(share)))
}

/// Windows: `wsl_share` for `path` as written (`/` or `\`, `\\?\UNC\` too), for the file builtins,
/// an import and FILE that failed with `e` (5.10, 6.4, 19.1).
pub(crate) fn wsl_root(path: &str, e: &std::io::Error) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let p = path.replace('/', "\\");
    let unc = p.strip_prefix(r"\\?\UNC\").or_else(|| p.strip_prefix(r"\\").filter(|r| !r.starts_with(['?', '.'])))?;
    wsl_share(unc, e)
}

/// A `\\wsl.localhost` or `\\wsl$` path (`unc`, what follows its `\\`, with `\`) that names no
/// WSL distribution, or one that isn't there, which Windows says as "The user name or password is
/// incorrect" (ERROR_LOGON_FAILURE) or as a network's error (`network_error`: ERROR_BAD_NET_NAME,
/// under load ERROR_NETNAME_DELETED too): what to say instead (5.10).
fn wsl_share(unc: &str, e: &std::io::Error) -> Option<String> {
    let mut parts = unc.split('\\');
    let server = parts.next().filter(|s| s.eq_ignore_ascii_case("wsl.localhost") || s.eq_ignore_ascii_case("wsl$"))?;
    match parts.next().filter(|d| !d.is_empty()) {
        None => Some(format!(r"\\{server} needs a WSL distribution's name after it, as in \\{server}\Ubuntu: wsl --list --quiet shows them")),
        Some(distro) if e.raw_os_error() == Some(1326) || network_error(e) => Some(format!("there is no WSL distribution named {}: wsl --list --quiet shows them", shown_path(distro))),
        Some(_) => None,
    }
}

/// Windows: an error of the network, not of the path (5.10): ERROR_BAD_NETPATH (53),
/// ERROR_BAD_NET_NAME (67), ERROR_DEV_NOT_EXIST (55), ERROR_CONNECTION_UNAVAIL (1201, a remembered
/// connection that isn't made) and the network's other failures.
fn network_error(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(51 | 53 | 55 | 59 | 64 | 67 | 1201 | 1203 | 1222 | 1231 | 1232))
}

/// `unreachable_root`'s IOError, and a device's (`device_path`): no hint of a folder to make,
/// but the escapes' one when the path holds a control character (`"Q:\temp"`).
fn root_error<T>(verb: &str, path: &str, why: String) -> Exec<T> {
    let err = ErrorValue::new("IOError", format!("cannot {verb} '{}': {why}", shown_path(path)));
    if path.chars().any(char::is_control) {
        let _ = err
            .hint
            .set(r#"in "…", a backslash starts an escape (\n is a line break, \t a tab): write Windows paths with / or \\, as in "notes/new.txt""#.into());
    }
    Err(Flow::Error(Rc::new(err)))
}

/// `GetDriveTypeW`'s `DRIVE_NO_ROOT_DIR` (no such drive) and `DRIVE_REMOTE` (a network drive).
const NO_DRIVE: u32 = 1;
const REMOTE_DRIVE: u32 = 4;

/// What is wrong with the drive `letter:` of a path the OS failed on with `e`, from its type
/// (`drive_type`): there is none, or it is a mapped network drive whose computer isn't reached
/// (`GetDriveTypeW` still knows its letter). A network error on a local drive comes from
/// something on the path (a link to a share), not the drive: `None` (the second review of round
/// 19).
fn drive_problem(letter: char, kind: u32, e: &std::io::Error) -> Option<String> {
    match kind {
        NO_DRIVE => Some(format!("there is no drive {letter}: (is it plugged in or connected?)")),
        REMOTE_DRIVE if network_error(e) => Some(format!("the drive {letter}: can't be reached (is it connected?)")),
        _ => None,
    }
}

/// Windows: the type of the drive `letter:` (`GetDriveTypeW`); it doesn't touch the drive, so a
/// network drive that is slow to answer doesn't hold it up.
#[cfg(windows)]
fn drive_type(letter: char) -> u32 {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    let root: Vec<u16> = format!(r"{letter}:\").encode_utf16().chain([0]).collect();
    // SAFETY: a NUL-terminated root path.
    unsafe { GetDriveTypeW(root.as_ptr()) }
}

/// Elsewhere every letter is a local drive (`DRIVE_FIXED`): nothing to say about it.
#[cfg(not(windows))]
fn drive_type(_letter: char) -> u32 {
    3
}

/// Windows: `/tmp/x.txt` or `/var/tmp/x.txt` where this drive has no such folder: Linux's and
/// macOS's temp folders, which `make_dir` would make at the top of the drive, where every user
/// can write: the temp folder to use instead (5.10).
fn tmp_hint(path: &str) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let (tmp, rest) = unix_tmp(path, &|tmp| std::path::Path::new(tmp).is_dir())?;
    Some(tmp_advice(tmp, rest))
}

/// The Linux temp folder `path` starts with (`/var/tmp` or `/tmp`) and the rest of it, when
/// `there(folder)` says this drive has no such folder (5.10). It asks nothing of the OS itself,
/// so a test can say what is there.
fn unix_tmp<'a>(path: &'a str, there: &dyn Fn(&str) -> bool) -> Option<(&'static str, &'a str)> {
    let (tmp, rest) = ["/var/tmp", "/tmp"].into_iter().find_map(|t| Some((t, path.strip_prefix(t).filter(|r| r.is_empty() || r.starts_with('/'))?)))?;
    (!there(tmp)).then_some((tmp, rest))
}

/// Windows: `make_dir`'s refusal of a `/tmp` or `/var/tmp` path where this drive has no such
/// folder (`unix_tmp`), which it would make at the top of the drive, where every user can write.
fn tmp_refusal(path: &str, there: &dyn Fn(&str) -> bool) -> Option<ErrorValue> {
    let (tmp, rest) = unix_tmp(path, there)?;
    let made = tmp.replace('/', "\\");
    let err = ErrorValue::new("IOError", format!("cannot create '{}': on Windows that makes {made} at the top of the drive, a folder other users can write to", shown_path(path)));
    let _ = err.hint.set(tmp_advice(tmp, rest).into());
    Some(err)
}

/// Windows: `make_dir`'s refusal of another Linux path whose top folder this drive doesn't have
/// (`linux_path_hint`: `/root/proj`), which it would make at the top of the drive, as `/tmp`'s is
/// (5.10).
fn linux_dir_refusal(path: &str) -> Option<ErrorValue> {
    let hint = linux_path_hint(path)?;
    let top = path[1..].split('/').next().unwrap_or_default();
    let err = ErrorValue::new("IOError", format!("cannot create '{}': on Windows that makes \\{top} at the top of the drive, a folder other users can write to", shown_path(path)));
    let _ = err.hint.set(hint.into());
    Some(err)
}

/// The `/tmp` hint: `getenv("TEMP")`, with the rest of the path after it.
fn tmp_advice(tmp: &str, rest: &str) -> String {
    let example = if rest.is_empty() {
        "getenv(\"TEMP\")".to_string()
    } else {
        let mut q = String::new();
        super::value::quoted(rest, &mut q);
        format!("getenv(\"TEMP\") + {q}")
    };
    format!("{tmp} is Linux's and macOS's temp folder: on Windows use getenv(\"TEMP\"), as in {example}")
}

/// A write refused because the file is read-only: how to make it writable (Windows: its
/// read-only attribute; elsewhere the owner's write permission, for a file of this user) (5.10).
fn read_only_hint(verb: &str, path: &str, e: &std::io::Error) -> Option<String> {
    if verb != "write" || e.kind() != std::io::ErrorKind::PermissionDenied {
        return None;
    }
    let meta = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file)?;
    #[cfg(windows)]
    {
        if !meta.permissions().readonly() {
            return None;
        }
        // attrib reads a `/` as the start of a switch, and a name that starts with `-` as an
        // attribute to clear: `.\` before it.
        let arg = path.replace('/', "\\");
        let arg = if arg.starts_with('-') { format!(".\\{arg}") } else { arg };
        let arg = if arg.chars().all(|c| c.is_alphanumeric() || "._-\\:".contains(c)) { arg } else { format!("\"{arg}\"") };
        Some(format!("'{}' is read-only: clear that with attrib -r {arg} (or in its Properties)", shown_path(path)))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: a plain query.
        if meta.mode() & 0o200 != 0 || meta.uid() != unsafe { libc::geteuid() } {
            return None;
        }
        // chmod reads a name that starts with `-` as an option: `./` before it.
        let path_arg = if path.starts_with('-') { format!("./{path}") } else { path.to_string() };
        let word = if path_arg.chars().all(|c| c.is_ascii_alphanumeric() || "._/-+".contains(c)) { path_arg } else { format!("'{}'", path_arg.replace('\'', "'\\''")) };
        Some(format!("'{}' is read-only: make it writable with chmod u+w {word}", shown_path(path)))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = meta;
        None
    }
}

/// The variable to name for `$HOME` in a path: on Windows, where `HOME` isn't set (PowerShell's
/// `$HOME` is its own, cmd has none), `USERPROFILE`, as for `~` (5.10).
fn home_var(name: &str) -> &str {
    if cfg!(windows) && name.eq_ignore_ascii_case("HOME") && std::env::var_os("HOME").is_none() {
        "USERPROFILE"
    } else {
        name
    }
}

/// A path whose first part a shell would have expanded (`~`, `~root`, `$HOME`, `%USERPROFILE%`):
/// V2 takes paths as written, so `make_dir("~/out")` would quietly make a folder named `~` here,
/// and a later `rm -rf ~` would take the home folder. Refused with what to write; `./~` is a
/// folder that is really named so (5.10).
fn shell_shorthand(verb: &str, path: &str) -> Exec<()> {
    // Every file builtin starts here: after an interrupt nothing is read or written (6.5).
    stop_if_interrupted();
    if path.is_empty() {
        return io_error(format!("cannot {verb} '': the path is empty"));
    }
    let mut first = path.split(['/', '\\']).next().unwrap_or_default();
    let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let user = other_home(path);
    let write = if first == "~" {
        match &path[1..] {
            "" => format!("getenv(\"{home}\")"),
            rest => format!("getenv(\"{home}\") + \"{rest}\""),
        }
    } else if let Some(user) = user {
        first = &path[..=user.len()];
        format!("the path of {user}'s home folder (what {first} is in a shell)")
    } else if crate::config::unexpanded(first) == Some(first) {
        let name = first.trim_matches(['%', '$', '{', '}']).trim_start_matches("env:");
        format!("getenv(\"{}\") in its place", home_var(name))
    } else {
        // `notes 12:30.txt` on Windows: a `:` after the drive, or after a share's
        // `\\server\share` (either slash), names a hidden stream of the file before it, so the
        // write would seem to work and leave that file empty (WSL's share takes it as a Linux
        // name's): refused on every path alike.
        if cfg!(windows) && super::module::after_root(path).is_some_and(|rest| rest.contains(':')) {
            let err = ErrorValue::new("IOError", format!("cannot {verb} '{}': a Windows file name can't have ':' in it", shown_path(path)));
            let hint = stand_in_hint(verb, path).unwrap_or_else(|| "use another character, as in 12-30 for a time".into());
            let _ = err.hint.set(hint.into());
            return Err(Flow::Error(Rc::new(err)));
        }
        return windows_name(verb, path);
    };
    let err = ErrorValue::new("IOError", format!("cannot {verb} '{}': V2 doesn't expand {first} in paths", shown_path(path)));
    let _ = err.hint.set(format!("write {write}, or ./{first} for a folder that is really named {first}").into());
    Err(Flow::Error(Rc::new(err)))
}

/// Unix: the user of a path that starts with `~user` (up to the first `/`) when there is such a
/// user, whose home folder a shell puts there (bash asks `getpwnam` too). A name no user has
/// (Word's `~WRL0001.tmp`, a folder `~old`) is a name; Windows shells expand no `~user` (5.10).
#[cfg(unix)]
fn other_home(path: &str) -> Option<&str> {
    let name = path.split('/').next()?.strip_prefix('~').filter(|n| !n.is_empty())?;
    let c = std::ffi::CString::new(name).ok()?;
    // SAFETY: a NUL-terminated name; only whether an entry came back is looked at.
    let found = unsafe { !libc::getpwnam(c.as_ptr()).is_null() };
    found.then_some(name)
}

#[cfg(not(unix))]
fn other_home(_path: &str) -> Option<&str> {
    None
}

/// A command of the shell (`dir`, `cd`), not a program on PATH: what to write instead (5.10).
fn shell_command_hint(name: &str, argv: &[Value]) -> Option<String> {
    const CMD: &[&str] = &[
        "assoc", "call", "cd", "chdir", "cls", "color", "copy", "date", "del", "dir", "echo", "erase", "ftype", "md", "mkdir", "mklink", "move", "path", "pause", "popd",
        "pushd", "rd", "ren", "rename", "rmdir", "set", "start", "time", "title", "type", "ver", "vol",
    ];
    const SH: &[&str] = &["alias", "cd", "eval", "export", "set", "source", "ulimit", "umask", "unalias", "unset"];
    let lower = name.to_ascii_lowercase();
    let (shell, list) = if cfg!(windows) { ("cmd.exe", CMD) } else { ("the shell", SH) };
    if !list.contains(&lower.as_str()) {
        return None;
    }
    let words: Vec<String> = argv.iter().map(|v| if let Value::Str(s) = v { s.to_string() } else { String::new() }).collect();
    let quoted = |w: &str| {
        let mut out = String::new();
        super::value::quoted(w, &mut out);
        out
    };
    // `cd` ends with its shell: the program to run there goes after it, in the same command.
    if lower == "cd" || lower == "chdir" {
        let dir = words.get(1).map_or("folder", String::as_str);
        let line = if cfg!(windows) { format!("cd /d {dir} && program") } else { format!("cd {dir} && program") };
        let (sh, flag) = if cfg!(windows) { ("cmd", "/c") } else { ("sh", "-c") };
        return Some(format!("'{name}' is a command of {shell}, not a program: to run a program in another folder, write exec([\"{sh}\", \"{flag}\", {}])", quoted(&line)));
    }
    let rest: Vec<String> = words.iter().map(|w| quoted(w)).collect();
    let call = if cfg!(windows) { format!("exec([\"cmd\", \"/c\", {}])", rest.join(", ")) } else { format!("exec([\"sh\", \"-c\", {}])", quoted(&words.join(" "))) };
    let also = if lower == "dir" { " (or list_dir(path) for the names in a folder)" } else { "" };
    Some(format!("'{name}' is a command of {shell}, not a program: write {call}{also}"))
}

/// Windows: a Linux program (`ls`, `grep`, `sed`, `awk`, `sh`) that isn't on PATH: run it in WSL,
/// `run Linux programs with exec(["wsl.exe", "-e", "ls", "-la"])`, with the arguments as given
/// (`-e`, not `--`, passes them on as they are) (5.10).
fn linux_program_hint(name: &str, argv: &[Value]) -> Option<String> {
    if !cfg!(windows) || !matches!(name, "ls" | "grep" | "sed" | "awk" | "sh") {
        return None;
    }
    let mut call = String::from(r#"exec(["wsl.exe", "-e""#);
    // exec has checked that every item is a str.
    for v in argv {
        if let Value::Str(s) = v {
            call.push_str(", ");
            super::value::quoted(s, &mut call);
        }
    }
    Some(format!("run Linux programs with {call}])"))
}

/// A script that doesn't start by itself (`./build.ps1`, `./tool.py` on Windows; one without
/// `#!` or the execute bit elsewhere): the program that runs it (5.10).
fn script_hint(name: &str) -> Option<String> {
    match script_call(name) {
        Some((ext, call)) => Some(format!("a .{ext} file runs through its program: write {call}")),
        // Windows without Git for Windows: no bash to name (7.6).
        None if cfg!(windows) && std::path::Path::new(name).extension().is_some_and(|e| e.eq_ignore_ascii_case("sh")) => {
            let mut path = String::new();
            super::value::quoted(name, &mut path);
            Some(format!("a .sh file needs a shell to run it, such as Git for Windows' bash (winget install -e --id Git.Git): then write exec([\"C:/Program Files/Git/bin/bash.exe\", {path}])"))
        }
        None => None,
    }
}

/// The exec call that runs a script through its program, and the script's extension. On
/// Windows a `.sh` runs through Git for Windows' bash, by its full path: a `bash` on PATH is
/// often WSL's, which runs Linux (7.6).
fn script_call(name: &str) -> Option<(String, String)> {
    let ext = std::path::Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    let mut path = String::new();
    super::value::quoted(name, &mut path);
    let call = match ext.as_str() {
        "ps1" if cfg!(windows) => format!("exec([\"powershell\", \"-NoProfile\", \"-ExecutionPolicy\", \"Bypass\", \"-File\", {path}])"),
        "ps1" => format!("exec([\"pwsh\", \"-File\", {path}])"),
        "py" => format!("exec([\"{}\", {path}]), or run its code in a @py block", if cfg!(windows) { "python" } else { "python3" }),
        "js" | "mjs" | "cjs" | "ts" | "mts" | "cts" => format!("exec([\"node\", {path}])"),
        "sh" if cfg!(windows) => {
            let program_files = std::env::var_os("ProgramFiles").map(std::path::PathBuf::from);
            let bash = git_bash(&Lookup::from_env(), program_files.as_deref())?;
            let mut shown = String::new();
            super::value::quoted(&bash.to_string_lossy().replace('\\', "/"), &mut shown);
            format!("exec([{shown}, {path}])")
        }
        "sh" => format!("exec([\"sh\", {path}])"),
        "v2" => format!("exec([\"v2\", {path}])"),
        _ => return None,
    };
    Some((ext, call))
}

/// Unix: a file with no x bit, which exec can run only through its program (5.10): what to
/// write, with the path as the program gave it (`bare`: given by its name alone).
fn unmarked_hint(file: &std::path::Path, shown: &str, bare: bool) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(file).ok()?;
        if !meta.is_file() || meta.permissions().mode() & 0o111 != 0 {
            return None;
        }
        // For the shell: quoted unless it is plain.
        let word = if shown.chars().all(|c| c.is_ascii_alphanumeric() || "._/-+".contains(c)) {
            shown.to_string()
        } else {
            format!("'{}'", shown.replace('\'', "'\\''"))
        };
        let mut q = String::new();
        super::value::quoted(shown, &mut q);
        let (start, mark) = if bare {
            ("V2 doesn't look in the current folder for a bare name, and ", format!("mark it once with chmod +x {word} and write exec([{q}])"))
        } else {
            ("", format!("mark it once with chmod +x {word}"))
        };
        Some(match script_call(shown) {
            Some((_, call)) => format!("{start}{shown} isn't marked as a program: write {call}, or {mark}"),
            None => format!("{start}{shown} isn't marked as a program: {mark}, or run it through its program, as in exec([\"sh\", {q}])"),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (file, shown, bare);
        None
    }
}

/// How a block reads a file in an old Windows code page: the one of the Windows that saved it,
/// this Windows' own first (`GetACP`: cp1254 on a Turkish one), then the common others, and
/// euc-jp, Linux's older Japanese (5.10).
fn code_page_way() -> String {
    #[cfg(windows)]
    // SAFETY: a plain query.
    let here = format!("cp{}", unsafe { windows_sys::Win32::Globalization::GetACP() });
    #[cfg(not(windows))]
    let here = "cp1252".to_string();
    // Said only where it isn't the usual one, so the hint reads the same on most machines.
    let own = if here != "cp1252" { format!(" (this Windows saves {here})") } else { String::new() };
    format!(
        "the code page of the Windows that saved it, as in @py(path) {{ open(path, encoding=\"{here}\").read() }}{own}: cp1252 for Western European text, cp1250 Central European, cp1251 Cyrillic, cp1253 Greek, cp1254 Turkish, cp1255 Hebrew, cp1256 Arabic, gbk Chinese, cp932 or euc-jp Japanese"
    )
}

/// What text that isn't UTF-8 probably is (5.10). `start`: `bytes` begin a file or stream, where
/// UTF-32's or UTF-16's byte order mark would be (UTF-32's first: little-endian, it starts with
/// UTF-16's).
fn not_utf8_kind(bytes: &[u8], start: bool) -> &'static str {
    if start && (bytes.starts_with(&[0xFF, 0xFE, 0, 0]) || bytes.starts_with(&[0, 0, 0xFE, 0xFF])) {
        "it is UTF-32 (what Out-File -Encoding utf32 writes)"
    } else if start && (bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF])) {
        "it is UTF-16 (what Windows PowerShell's > and Out-File write)"
    } else if bytes.iter().take(8192).any(|&b| b == 0) {
        "it looks like binary data"
    } else {
        "it may be in an older encoding, such as Windows-1252"
    }
}

/// Whether text that isn't UTF-8 (`e`, from `from_utf8`) is UTF-8 with its last character cut
/// (another program is still writing it, or cut it there): the bad bytes are an unfinished
/// character at the end (`error_len` is `None`), and the valid part before them (`good`, with a
/// UTF-8 byte-order mark when there was one) holds a byte past ASCII, so it isn't one byte of an
/// older code page at the end (`caf\xE9`) (5.10).
fn cut_utf8(good: &[u8], e: &std::str::Utf8Error) -> bool {
    e.error_len().is_none() && good.iter().any(|&b| b >= 0x80)
}

/// How many times in a row `input()` may give null before a loop that never checks is stopped.
const INPUT_NULLS: u32 = 1000;

/// The nulls `input()` gave since it last read a line.
static NULLS_IN_A_ROW: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// `input(prompt = "", echo: true)`: one line from stdin without `\n` and one `\r` before it;
/// `null` at end of input; the prompt is flushed first (5.10). With `echo: false` and stdin a
/// terminal, what is typed isn't shown (`EchoOff`), and a line break goes after the line, where
/// the prompt went, as getpass does. A menu loop that never checks for null would spin forever
/// once the input ends: the 1000th null in a row is an IOError instead.
fn input(it: &mut Interp, args: Vec<Value>, at: Span) -> Exec<Value> {
    use std::sync::atomic::Ordering;
    let echo = bool_arg(&args[1], "input", "echo")?;
    let prompt = it.str_of(&args[0], at)?;
    // Echo goes off before the prompt shows, so nothing typed after it is shown. A failed write
    // turns it back on before it is reported: a reader that went away ends the run at once.
    let quiet = if echo { None } else { EchoOff::start() };
    lock(it.out.pending).extend_from_slice(prompt.as_bytes());
    if let Err(e) = it.out.flush() {
        drop(quiet);
        // Always the IOError, if the run doesn't end here.
        return it.out.raise(Err(e)).map(|()| Value::Null);
    }
    let mut line = Vec::new();
    let read = std::io::stdin().lock().read_until(b'\n', &mut line);
    if let Some(quiet) = quiet {
        drop(quiet);
        // The terminal didn't show the Enter that ended the line.
        if matches!(read, Ok(n) if n > 0) {
            it.out.write_str("\n")?;
        }
    }
    match read {
        Ok(0) => {
            if NULLS_IN_A_ROW.fetch_add(1, Ordering::Relaxed) + 1 >= INPUT_NULLS {
                let err = ErrorValue::new("IOError", format!("the input has ended, and input() has given null {INPUT_NULLS} times in a row"));
                let _ = err.hint.set("check for null and stop, as in let line = input(); if line == null { break }".into());
                return Err(Flow::Error(Rc::new(err)));
            }
            return Ok(Value::Null);
        }
        Ok(_) => NULLS_IN_A_ROW.store(0, Ordering::Relaxed),
        Err(e) => return io_error(format!("cannot read from stdin: {}", os_message(&e))),
    }
    if line.last() == Some(&b'\n') {
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
    }
    // Windows PowerShell 5 starts text it pipes with a byte-order mark: the first line drops it,
    // as a script file's first line does (5.10).
    thread_local!(static LINES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) });
    let n = LINES.with(|l| {
        l.set(l.get() + 1);
        l.get()
    });
    // Two when `$OutputEncoding` is UTF-8 with its BOM, which PowerShell 5.1 adds once more.
    while n == 1 && line.starts_with(&[0xEF, 0xBB, 0xBF]) {
        line.drain(..3);
    }
    match String::from_utf8(line) {
        Ok(s) => Ok(Value::Str(s.into())),
        Err(e) => {
            let kind = not_utf8_kind(e.as_bytes(), n == 1);
            let good = &e.as_bytes()[..e.utf8_error().valid_up_to()];
            // A character cut in two (by `head -c`, or a writer stopped in the middle): no
            // code page's advice, which would garble the rest.
            if kind.ends_with("Windows-1252") && cut_utf8(good, &e.utf8_error()) {
                return io_error(format!("input line {n} is UTF-8 but ends inside a character: send whole characters"));
            }
            let err = ErrorValue::new("IOError", format!("input line {n} is not valid UTF-8: {kind}: send UTF-8 text"));
            if stdin_is_file() {
                // `v2 x.v2 < data.txt`: no shell setting changes a file's bytes.
                let _ = err.hint.set(format!("the file sent as input isn't UTF-8: save it as UTF-8, or read it in a @py block with {}", code_page_way()).into());
            } else if cfg!(windows) {
                let _ = err.hint.set(piped_input_hint(std::io::stderr().is_terminal()).into());
            }
            Err(Flow::Error(Rc::new(err)))
        }
    }
}

/// `input(echo: false)`: while it lives, the terminal on stdin doesn't show what is typed (5.10).
/// Unix: `ECHO` cleared with `tcsetattr`, line input (`ICANON`) kept, the settings before kept in
/// `ECHO_SAVED` for Ctrl+Z (`echo_around_stop`) and `TERMINAL_CHANGED` set, so Ctrl+C's path puts
/// the settings v2 started with back whatever stdout is (`restore_terminal`). Windows:
/// `ENABLE_ECHO_INPUT` cleared on the console input handle, the mode before kept in `ECHO_MODE` for
/// Ctrl+C's path (`echo_back`).
struct EchoOff {
    #[cfg(unix)]
    saved: libc::termios,
    #[cfg(windows)]
    saved: u32,
}

/// Unix: the settings the terminal had before the live `EchoOff` turned echo off (`None`: none is
/// live). Its lock is held while they change, so Ctrl+Z and the end of the line don't interleave.
#[cfg(unix)]
static ECHO_SAVED: Mutex<Option<libc::termios>> = Mutex::new(None);

/// Unix: whether another process group is the foreground job of the terminal on stdin (a terminal
/// that isn't v2's controlling one has no jobs, so v2 isn't in its background).
#[cfg(unix)]
fn background() -> bool {
    // SAFETY: plain queries of stdin's terminal and of this process.
    unsafe {
        let job = libc::tcgetpgrp(0);
        job != -1 && job != libc::getpgrp()
    }
}

/// Unix, Ctrl+Z (`process::suspend`, 6.5): `stop` stops v2 until it goes on. While `input(echo:
/// false)` waits, the terminal first gets back the settings it had before, so the shell shows what
/// is typed while v2 is stopped (dash leaves the terminal as it finds it); once v2 goes on as the
/// foreground job, echo goes off again (bash has put its own settings back meanwhile). Gone on in
/// the background (`bg`), v2 waits for the foreground first, stopped by SIGTTOU (`tcdrain`) as its
/// read would be by SIGTTIN.
#[cfg(unix)]
pub fn echo_around_stop(stop: impl FnOnce()) {
    let live = ECHO_SAVED.lock().unwrap_or_else(|e| e.into_inner());
    let Some(saved) = *live else {
        drop(live);
        return stop();
    };
    let mut quiet = saved;
    quiet.c_lflag &= !libc::ECHO;
    // SAFETY: plain calls on stdin with the settings tcgetattr gave.
    unsafe {
        if !background() {
            libc::tcsetattr(0, libc::TCSANOW, &saved);
        }
        stop();
        if background() {
            libc::tcdrain(0);
        }
        if !background() {
            libc::tcsetattr(0, libc::TCSANOW, &quiet);
        }
    }
}

/// Windows: the console input mode `input(echo: false)` changed, while it is changed
/// (`NO_MODE`: none).
#[cfg(windows)]
static ECHO_MODE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(NO_MODE);
#[cfg(windows)]
const NO_MODE: u32 = u32::MAX;

impl EchoOff {
    /// Echo off, or `None` when stdin isn't a terminal (a pipe or a file reads as usual) or its
    /// mode can't be set.
    fn start() -> Option<EchoOff> {
        #[cfg(unix)]
        // SAFETY: tcgetattr fills a zeroed struct for any descriptor, a terminal or not, and
        // tcsetattr gets a copy of what it gave.
        unsafe {
            use crate::engines::process::TERMINAL_CHANGED;
            use std::sync::atomic::Ordering::SeqCst;
            if libc::isatty(0) != 1 {
                return None;
            }
            // In the background (`v2 x.v2 &`, Ctrl+Z then bg) tcdrain stops v2 with SIGTTOU until it
            // is the foreground job, so the settings read below are the ones the shell gives it, not
            // its line editor's. Still in the background (SIGTTOU ignored, an orphaned job): read as
            // plain input, leaving the terminal alone.
            if background() {
                libc::tcdrain(0);
                if background() {
                    return None;
                }
            }
            let mut saved: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut saved) != 0 {
                return None;
            }
            let mut quiet = saved;
            quiet.c_lflag &= !libc::ECHO;
            let mut live = ECHO_SAVED.lock().unwrap_or_else(|e| e.into_inner());
            // Set first, so a Ctrl+C right after the change puts it back.
            TERMINAL_CHANGED.store(true, SeqCst);
            if libc::tcsetattr(0, libc::TCSANOW, &quiet) != 0 {
                TERMINAL_CHANGED.store(false, SeqCst);
                return None;
            }
            *live = Some(saved);
            Some(EchoOff { saved })
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use std::sync::atomic::Ordering;
            use windows_sys::Win32::System::Console::{GetConsoleMode, SetConsoleMode, ENABLE_ECHO_INPUT};
            let stdin = std::io::stdin().as_raw_handle();
            let mut saved = 0;
            // SAFETY: the standard input handle stays open for the process; GetConsoleMode fails
            // for one that isn't a console, and fills its out-parameter.
            if unsafe { GetConsoleMode(stdin, &mut saved) } == 0 {
                return None;
            }
            // Kept first, so a Ctrl+C right after the change puts it back.
            ECHO_MODE.store(saved, Ordering::SeqCst);
            // SAFETY: as above.
            if unsafe { SetConsoleMode(stdin, saved & !ENABLE_ECHO_INPUT) } == 0 {
                ECHO_MODE.store(NO_MODE, Ordering::SeqCst);
                return None;
            }
            Some(EchoOff { saved })
        }
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let mut live = ECHO_SAVED.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: stdin gets back the settings tcgetattr gave.
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.saved) };
            *live = None;
            crate::engines::process::TERMINAL_CHANGED.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            // SAFETY: the standard input handle stays open for the process.
            unsafe { windows_sys::Win32::System::Console::SetConsoleMode(std::io::stdin().as_raw_handle(), self.saved) };
            ECHO_MODE.store(NO_MODE, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

/// Windows, on the interrupt path (6.5): Ctrl+C during `input(echo: false)` gives the console its
/// echo back. Stdin's handle without its lock, which the interpreter thread holds while it reads.
#[cfg(windows)]
pub fn echo_back() {
    use std::os::windows::io::AsRawHandle;
    let mode = ECHO_MODE.load(std::sync::atomic::Ordering::SeqCst);
    if mode != NO_MODE {
        // SAFETY: the standard input handle stays open for the process.
        unsafe { windows_sys::Win32::System::Console::SetConsoleMode(std::io::stdin().as_raw_handle(), mode) };
    }
}

/// On Windows, `input()`'s hint for a line from a pipe or a console that isn't UTF-8 (5.10). With
/// a console on standard error the sender is a shell, which sends its console's old code page
/// unless told otherwise, or a program run there (Python's `input=` with `text=True` sends its
/// ANSI code page whatever chcp says); without one, a program runs v2 and reads what it says.
fn piped_input_hint(console: bool) -> &'static str {
    if console {
        "in cmd.exe, run chcp 65001 first; for Windows PowerShell 5.1, README.md's Troubleshooting has the line to run; a program that sends v2 its input must send UTF-8 (in Python, encoding=\"utf-8\")"
    } else {
        "the program that sends v2 its input must send UTF-8 (in Python, encoding=\"utf-8\")"
    }
}

/// Whether stdin is a file (`v2 x.v2 < data.txt`), not a terminal or a pipe.
fn stdin_is_file() -> bool {
    #[cfg(unix)]
    {
        std::os::fd::AsFd::as_fd(&std::io::stdin()).try_clone_to_owned().map(std::fs::File::from).and_then(|f| f.metadata()).is_ok_and(|m| m.is_file())
    }
    // A pipe's handle reports a file's metadata on Windows: its type says what it is.
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_DISK};
        // SAFETY: the standard input handle stays open for the process.
        unsafe { GetFileType(std::io::stdin().as_raw_handle()) == FILE_TYPE_DISK }
    }
}

/// `len(x)`: chars, bytes, items or entries, or a handle's `op len` (5.10, 11.3).
fn len(it: &mut Interp, args: Vec<Value>, at: Span) -> Exec<Value> {
    Ok(Value::Int(match &args[0] {
        Value::Handle(h) => it.handle_len(h, at)?,
        Value::Str(s) => it.chars.len(s) as i64,
        Value::Bytes(b) => b.len() as i64,
        Value::List(l) => l.borrow().len() as i64,
        Value::Dict(d) => d.borrow().len() as i64,
        other => return type_error(format!("len() needs a str, bytes, list or dict, got {}", other.type_name())),
    }))
}

/// `int(x)` (5.10): floats truncate toward zero; strings are `[+-]?[0-9]+` after trimming.
fn int(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    Ok(Value::Int(match &args[0] {
        Value::Int(n) => *n,
        Value::Bool(b) => *b as i64,
        Value::Float(x) => {
            let t = x.trunc();
            if !x.is_finite() || !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&t) {
                return value_error(format!("cannot convert {} to int", super::value::float_str(*x)));
            }
            t as i64
        }
        Value::Str(s) => {
            let t = s.trim();
            let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(number_text(format!("invalid int: '{s}'"), t, true));
            }
            match t.parse::<i64>() {
                Ok(n) => n,
                Err(_) => return value_error(format!("int out of range: '{s}'")),
            }
        }
        Value::Handle(h) => return number_error("int() can't convert handle", h, true),
        Value::Null => return Err(null_number("int")),
        other => return type_error(format!("int() can't convert {}", other.type_name())),
    }))
}

/// `int("13.5")`, `float("3,7")`, `int("13 Jahre")`: the ValueError, with what the text needs to
/// be (5.10).
fn number_text(message: String, t: &str, int: bool) -> Flow {
    let err = ErrorValue::new("ValueError", message);
    if let Some(h) = number_hint(t, int) {
        let _ = err.hint.set(h.into());
    }
    Flow::Error(Rc::new(err))
}

/// `number_text`'s hint for the trimmed text `t`, which `int()` (`int`) or `float()` can't read.
/// Each says what makes the text one they read, as the user's code can write it.
fn number_hint(t: &str, int: bool) -> Option<String> {
    let f = if int { "int" } else { "float" };
    let number = |x: &str| x.parse::<f64>().is_ok();
    let reads = |x: &str| if int { x.parse::<i64>().is_ok() } else { number(x) };
    // The number the text starts with: digits, a sign, a point or a comma.
    let lead: String = t.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | ',')).collect();
    let spaces = |c: char| matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '\u{2009}');
    if t.is_empty() {
        Some("the text is empty: an empty line from input() is \"\"".to_string())
    } else if t.starts_with("0x") || t.starts_with("0X") {
        Some(format!("{f}() reads decimal digits: for hex text, convert it in a block, as in @py(x) {{ int(x, 16) }}"))
    } else if t.contains('\u{2212}') {
        // `−5`: the minus sign U+2212, which JavaScript's Intl writes (sv-SE, fi-FI).
        let plain = t.replace('\u{2212}', "-");
        let head = "the text has a minus sign U+2212, as Intl writes it";
        Some(if reads(&plain) {
            format!("{head}: {f}(x.replace(\"\u{2212}\", \"-\"))")
        } else if let Some(then) = number_hint(&plain, int) {
            format!("{head}: x = x.replace(\"\u{2212}\", \"-\") changes it; then {then}")
        } else {
            format!("{head}: x.replace(\"\u{2212}\", \"-\") changes it")
        })
    } else if t.contains('_') && number(&t.replace('_', "")) {
        // `1_000`: as a V2 literal it works; as text the _ has to go.
        Some(format!("{f}() reads digits without _: {f}(x.replace(\"_\", \"\"))"))
    } else if t.contains(['\u{66b}', '\u{66c}']) && number(&arabic_marks(t).0) {
        // `12٫5`, `1٬234٫5`: U+066B, the Arabic decimal separator, and U+066C, its thousands one.
        let (plain, replaces) = arabic_marks(t);
        let g = if reads(&plain) { f } else { "float" };
        let has = match (t.contains('\u{66c}'), t.contains('\u{66b}')) {
            (true, true) => "U+066C and U+066B, the Arabic thousands and decimal separators",
            (true, false) => "U+066C, the Arabic thousands separator",
            _ => "U+066B, the Arabic decimal separator",
        };
        Some(format!("the text has {has}: {g}(x{replaces})"))
    } else if let Some(h) = grouped_hint(t, int) {
        Some(h)
    } else if t.chars().any(spaces) && number(&t.replace(spaces, "")) {
        let mut seen: Vec<char> = t.chars().filter(|&c| spaces(c)).collect();
        seen.sort_unstable();
        seen.dedup();
        let removes: String = seen.iter().map(|&c| format!(".replace(\"{}\", \"\")", char_text(c))).collect();
        Some(format!("{f}() reads digits without spaces between them: {f}(x{removes})"))
    } else if t.contains('.') && t.contains(',') && t.rfind(',') > t.rfind('.') && t.replace('.', "").replace(',', ".").parse::<f64>().is_ok() {
        // `1.234,56`: points between thousands and a decimal comma, as German and Turkish write.
        Some("the text has points between thousands and a decimal comma: float(x.replace(\".\", \"\").replace(\",\", \".\"))".to_string())
    } else if t.matches('.').count() > 1 && t.replace('.', "").parse::<i64>().is_ok() {
        Some(format!("the text has points between thousands: {f}(x.replace(\".\", \"\"))"))
    } else if let Some((mark, whole)) = thousands(t).filter(|_| int) {
        // `1.234`, `1,234`: a decimal to some, a thousand and more to others.
        let (what, decimal) = if mark == '.' {
            ("point", "use float(x), or int(float(x)) to drop what is after the point")
        } else {
            ("comma", "int(float(x.replace(\",\", \".\"))) drops what is after it")
        };
        Some(format!("if the {what} is between thousands, int(x.replace(\"{mark}\", \"\")) reads {whole}; if it is a decimal {what}, {decimal}"))
    } else if lead.contains(',') && lead.replace(',', ".").parse::<f64>().is_ok() {
        Some("write the decimal point as a point, as in 3.7: x.replace(\",\", \".\") changes it".to_string())
    } else if int && lead.parse::<i64>().is_err() && lead.parse::<f64>().is_ok() {
        Some(format!("int() reads whole numbers, as in 13: for {lead}, use float(x), or int(float(x)) to drop what is after the point"))
    } else if t.chars().any(|c| c.is_numeric() && !c.is_ascii_digit()) {
        // `١٢٣`, `１２`: digits of another script, which Python reads once what it refuses is out
        // (`other_digits`). For `int()`, U+066B makes the block `float`, and so does a point that
        // can't be between thousands (`between_thousands`): `१.२३४` is 1234 in de-DE-u-nu-deva, so
        // it stays `int`, which Python refuses instead of reading 1.234.
        let g = if t.contains('\u{66b}') || t.contains('.') && !between_thousands(t, '.') { "float" } else { f };
        let replaces = other_digits(t);
        Some(format!("{f}() reads the digits 0 to 9 only: for other digits, convert in a block, as in @py(x) {{ {g}(x{replaces}) }}"))
    } else if !lead.is_empty() && lead.len() < t.len() && lead.parse::<f64>().is_ok() && !t[lead.len()..].chars().any(char::is_numeric) {
        Some(format!("{f}() reads the number alone, without other text: '{lead}' would work"))
    } else if !t.chars().any(|c| c.is_ascii_digit()) {
        Some("the text has no digits: a number is written with digits, as in 13".to_string())
    } else {
        None
    }
}

/// `1 234,5`, `1’234.5`, `1,234.56`: digits in groups of three after the first one to three, one
/// separator between them (a space, U+00A0, U+202F, U+2009, `'`, U+2019, a comma or a point),
/// perhaps a decimal part, as a locale writes a number: what removes the separator (5.10). One
/// point or comma before three digits and no more (`1.234`) may be a decimal one, and a first
/// group that starts with 0 (`0.123`) is no locale's: `None`.
fn grouped_hint(t: &str, int: bool) -> Option<String> {
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    let first = body.bytes().take_while(u8::is_ascii_digit).count();
    let sep = body[first..].chars().next()?;
    if !(1..=3).contains(&first) || body.starts_with('0') || !matches!(sep, ' ' | '\u{a0}' | '\u{202f}' | '\u{2009}' | '\'' | '\u{2019}' | ',' | '.') {
        return None;
    }
    let mut rest = &body[first..];
    let mut groups = 0;
    while let Some(r) = rest.strip_prefix(sep) {
        let b = r.as_bytes();
        if b.len() < 3 || !b[..3].iter().all(u8::is_ascii_digit) || b.get(3).is_some_and(u8::is_ascii_digit) {
            return None;
        }
        rest = &r[3..];
        groups += 1;
    }
    let decimal = match rest.chars().next() {
        None => None,
        Some(d @ ('.' | ',')) if d != sep && rest.len() > 1 && rest[1..].bytes().all(|b| b.is_ascii_digit()) => Some(d),
        _ => return None,
    };
    if groups == 1 && decimal.is_none() && matches!(sep, '.' | ',') {
        return None;
    }
    let name = match sep {
        ' ' => "spaces",
        '\u{a0}' => "no-break spaces (U+00A0)",
        '\u{202f}' => "narrow no-break spaces (U+202F)",
        '\u{2009}' => "thin spaces (U+2009)",
        '\'' => "apostrophes",
        '\u{2019}' => "apostrophes (U+2019)",
        ',' => "commas",
        _ => "points",
    };
    let (and, swap) = if decimal == Some(',') { (" and a decimal comma", ".replace(\",\", \".\")") } else { ("", "") };
    let f = if int && decimal.is_none() { "int" } else { "float" };
    Some(format!("the text has {name} between thousands{and}: {f}(x.replace(\"{}\", \"\"){swap})", char_text(sep)))
}

/// `1.234`, `-1,234`: one point or comma before exactly three digits, and the text without it;
/// not after a 0 (`0.123`), which is no thousand.
fn thousands(t: &str) -> Option<(char, String)> {
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    let (whole, part) = body.split_once(['.', ','])?;
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let mark = body[whole.len()..].chars().next()?;
    ((1..=3).contains(&whole.len()) && digits(whole) && !whole.starts_with('0') && part.len() == 3 && digits(part)).then(|| (mark, t.replacen(mark, "", 1)))
}

/// `١٬٢٣٤٫٥` (ar-EG, fa-IR): the text with U+066C, the Arabic thousands separator, taken out and
/// U+066B, the decimal one, as a point, and the replaces a hint writes for that (5.10).
fn arabic_marks(t: &str) -> (String, String) {
    let mut replaces = String::new();
    if t.contains('\u{66c}') {
        replaces.push_str(".replace(\"\u{66c}\", \"\")");
    }
    if t.contains('\u{66b}') {
        replaces.push_str(".replace(\"\u{66b}\", \".\")");
    }
    (t.replace('\u{66c}', "").replace('\u{66b}', "."), replaces)
}

/// The replaces that make `t`, in digits of another script, text Python's `float()` reads (5.10):
/// the bidi marks Intl writes before a sign (U+061C ar-EG, U+200E fa-IR and ps-AF, U+200F ckb),
/// as Python escapes since they are invisible; ASCII commas between thousands (`১,২৩৪.৫` bn-BD;
/// `between_thousands`); and `arabic_marks`' U+066C and U+066B.
fn other_digits(t: &str) -> String {
    let mut replaces: String = ['\u{61c}', '\u{200e}', '\u{200f}'].into_iter().filter(|&c| t.contains(c)).map(|c| format!(".replace(\"\\u{:04x}\", \"\")", c as u32)).collect();
    if between_thousands(t, ',') {
        replaces.push_str(".replace(\",\", \"\")");
    }
    replaces + &arabic_marks(t).1
}

/// Whether the marks `mark` (a comma or a point) in `t`, in digits of another script, can be
/// between thousands, as `grouped_hint` reads ASCII ones: after the sign and bidi marks one to
/// three digits, the first not a zero (`zero_digit`), two or three digits to each next mark (the
/// Indian grouping), three after the last, none in the decimal part (after U+066B or the other of
/// `,` and `.`). So the commas bn, mr, ne and my write with their digits (`১,২৩৪.৫`,
/// `১২,৩৪,৫৬৭.৫`) are, and a decimal comma (`৩,৫`, `০,১২৩`, `১২৩৪,৫৬৭`) is not: taking it out
/// would turn 0.123 into 123.
fn between_thousands(t: &str, mark: char) -> bool {
    let (whole, part) = t.split_once(|c| c != mark && matches!(c, ',' | '.' | '\u{66b}')).unwrap_or((t, ""));
    let mut groups = whole.split(mark);
    let first = groups.next().unwrap_or_default().trim_start_matches(['+', '-', '\u{61c}', '\u{200e}', '\u{200f}']);
    let digits = |g: &str| if g.chars().all(char::is_numeric) { g.chars().count() } else { 0 };
    let rest: Vec<usize> = groups.map(digits).collect();
    (1..=3).contains(&digits(first)) && !first.starts_with(zero_digit) && !part.contains(mark) && rest.last() == Some(&3) && rest.iter().all(|n| (2..=3).contains(n))
}

/// Whether `c` is the zero of its script's digits (`0`, `०`, `০`, `໐`): Unicode encodes decimal
/// digits in runs of ten from zero, so it is a numeric character after numeric ones in tens or
/// none (the math digits run on, ten after ten; Brahmi's follow its twenty numbers).
fn zero_digit(c: char) -> bool {
    let before = (1..=c as u32).map_while(|n| char::from_u32(c as u32 - n).filter(|p| p.is_numeric())).count();
    c.is_numeric() && before % 10 == 0
}

/// A character as a V2 string writes it in a hint: an invisible space as its escape (`\u{a0}`).
fn char_text(c: char) -> String {
    if matches!(c, '\u{a0}' | '\u{202f}' | '\u{2009}') {
        format!("\\u{{{:x}}}", c as u32)
    } else {
        c.to_string()
    }
}

/// `int(input())` once the input has ended (5.10).
fn null_number(f: &str) -> Flow {
    let err = ErrorValue::new("TypeError", format!("{f}() can't convert null"));
    let _ = err.hint.set("input() gives null when the input has ended, and a function without return gives null: check for null first".into());
    Flow::Error(Rc::new(err))
}

/// A handle where V2 wants a number (Python's `Decimal` or `Fraction`, a numpy array): the
/// TypeError, with what converts it in its own language (5.10). For `int()` of an integer beyond
/// 64 bits that conversion gives the same handle back, so the hint says what can be done instead.
pub(crate) fn number_error<T>(message: &str, h: &crate::engines::handles::HandleRef, int: bool) -> Exec<T> {
    let tag = &*h.tag;
    let js = matches!(tag, "js" | "ts");
    let hint = if int && h.big_int() {
        let (digits, float) = if js { ("h.toString()", "Number(h)") } else { ("str(h)", "float(h)") };
        format!("this integer needs more than 64 bits, which V2's int doesn't have: keep the math in the block, or use @{tag}(h) {{ {digits} }} for its digits or @{tag}(h) {{ {float} }}")
    } else {
        let conv = match (js, int) {
            (true, true) => "Math.trunc(Number(h))",
            (true, false) => "Number(h)",
            (false, true) => "int(h)",
            (false, false) => "float(h)",
        };
        format!("convert it in its language, as in @{tag}(h) {{ {conv} }}")
    };
    let err = ErrorValue::new("TypeError", message);
    let _ = err.hint.set(hint.into());
    Err(Flow::Error(Rc::new(err)))
}

/// `float(x)` (5.10): strings via `f64::from_str` after trimming (`nan`, `inf`, `infinity`).
fn float(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    Ok(Value::Float(match &args[0] {
        Value::Float(x) => *x,
        Value::Int(n) => *n as f64,
        Value::Bool(b) => *b as i64 as f64,
        Value::Str(s) => match s.trim().parse::<f64>() {
            Ok(x) => x,
            Err(_) => return Err(number_text(format!("invalid float: '{s}'"), s.trim(), false)),
        },
        Value::Handle(h) => return number_error("float() can't convert handle", h, false),
        Value::Null => return Err(null_number("float")),
        other => return type_error(format!("float() can't convert {}", other.type_name())),
    }))
}

/// The most items one `range()` list or `*` repetition makes (5.3): past it, a machine's memory
/// would run out first.
pub(crate) const MAX_ITEMS: u64 = 100_000_000;

/// `range(end)` / `range(start, end, step = 1)`: a list of ints, sized before it is built. One
/// argument is the end; arguments are positional only (5.10).
fn range(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let Value::List(given) = &args[0] else { unreachable!("a rest parameter is a list") };
    let (start, step, count) = range_bounds(&given.borrow())?;
    if count > MAX_ITEMS {
        let err = ErrorValue::new("ValueError", format!("range too large: a list of {count} items ({MAX_ITEMS} at most)"));
        let _ = err.hint.set("for a long loop, write for i in 0..n, which makes no list".into());
        return Err(Flow::Error(Rc::new(err)));
    }
    let mut items = Vec::new();
    if usize::try_from(count).ok().and_then(|n| items.try_reserve_exact(n).ok()).is_none() {
        return value_error("range too large");
    }
    let mut i = start as i128;
    for _ in 0..count {
        items.push(Value::Int(i as i64));
        i += step as i128;
    }
    Ok(Value::list(items))
}

/// `range()`'s arguments (`given`, its rest parameter): the first item, the step and how many
/// items. A `for` over `range(…)` counts with them without making the list (5.8).
pub(crate) fn range_bounds(given: &[Value]) -> Exec<(i64, i64, u64)> {
    let (start, end, step) = match given {
        [] => return fail("ArityError", "range() missing required argument 'end'"),
        [end] => (&Value::Int(0), end, &Value::Int(1)),
        [start, end] => (start, end, &Value::Int(1)),
        [start, end, step] => (start, end, step),
        more => return fail("ArityError", format!("range() takes 3 positional arguments but {} were given", more.len())),
    };
    let (start, end, step) = (int_arg(start, "range", "start")?, int_arg(end, "range", "end")?, int_arg(step, "range", "step")?);
    if step == 0 {
        return value_error("range() step must not be zero");
    }
    // Number of items: ceil((end - start) / step) when that is positive.
    let span = end as i128 - start as i128;
    let count = if (span > 0) == (step > 0) && span != 0 { (span.abs() + step.unsigned_abs() as i128 - 1) / step.unsigned_abs() as i128 } else { 0 };
    Ok((start, step, count as u64))
}

fn json_parse(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let text = str_arg(&args[0], "json_parse", "text")?;
    json::parse(&text)
}

fn json_stringify(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let indent = match &args[1] {
        Value::Null => None,
        Value::Int(n) if *n >= 0 => Some(*n as usize),
        Value::Int(n) => return value_error(format!("json_stringify() argument 'indent' must be >= 0, got {n}")),
        other => {
            return type_error(format!("json_stringify() argument 'indent' must be an int or null, got {}", other.type_name()))
        }
    };
    Ok(Value::Str(json::stringify(&args[0], indent)?.into()))
}

/// `read_file(path, binary: false)`: relative to the working directory; text is strict UTF-8
/// without one leading BOM (Windows tools write one), no newline translation (5.10).
fn read_file(it: &mut Interp, args: Vec<Value>, at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "read_file", "path")?;
    shell_shorthand("read", &path)?;
    let path = verbatim_parts(&path);
    let binary = bool_arg(&args[1], "read_file", "binary")?;
    wsl_special("read", &path)?;
    let bytes = match read_path(&path) {
        Ok(Some(b)) => b,
        // More than a str or bytes holds: a huge file, or a device that never ends (/dev/zero).
        Ok(None) => {
            let err = ErrorValue::new("IOError", format!("cannot read '{}': it holds more than 1 GiB, the most V2 reads at once: read it in pieces in a block", shown_path(&path)));
            // Forms that give a block's value (a `with` statement gives none); the first works
            // on a device that never ends too.
            let _ = err
                .hint
                .set(r#"for its first MiB: @py(path) { open(path, "rb").read(1 << 20) }; for its last: @py(path) { f = open(path, "rb"); f.seek(-(1 << 20), 2); f.read() }"#.into());
            return Err(Flow::Error(Rc::new(err)));
        }
        Err(e) => {
            // A relative path starts in the folder v2 runs in; the file may be next to the script.
            let p = std::path::Path::new(&*path);
            let beside = it.sm.file(at.file).path.parent().filter(|dir| e.kind() == std::io::ErrorKind::NotFound && p.is_relative() && dir.join(p).is_file());
            let hint = beside.map(|_| {
                format!("relative paths start in the folder you ran v2 from; for the file next to the script: read_file(f\"${{script_dir()}}/{}\")", path.replace('\\', "/"))
            });
            return file_error_hint("read", &path, &e, hint);
        }
    };
    if binary {
        return Ok(Value::Bytes(bytes.into()));
    }
    let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    match std::str::from_utf8(text) {
        Ok(s) => Ok(Value::str(s)),
        Err(e) => {
            let kind = not_utf8_kind(&bytes, true);
            // A log another program is still writing: a code page's advice would garble it.
            if kind.ends_with("Windows-1252") && cut_utf8(&bytes[..bytes.len() - text.len() + e.valid_up_to()], &e) {
                let err = ErrorValue::new("IOError", format!("'{}' is UTF-8 but ends inside a character (a program may still be writing it)", shown_path(&path)));
                let _ = err
                    .hint
                    .set(r#"read it again once it is written, or without the cut character: @py(path) { open(path, encoding="utf-8", errors="ignore", newline="").read() }"#.into());
                return Err(Flow::Error(Rc::new(err)));
            }
            // Text in another encoding is read by a block that knows it (V2's strings are UTF-8).
            let fix = match kind {
                k if k.ends_with("binary data") => "read it with binary: true",
                k if k.starts_with("it is UTF-16") => "save it as UTF-8, or read it in a @py block: @py(path) { open(path, encoding=\"utf-16\").read() }",
                k if k.starts_with("it is UTF-32") => "save it as UTF-8, or read it in a @py block: @py(path) { open(path, encoding=\"utf-32\").read() }",
                _ => "",
            };
            let fix = if fix.is_empty() { format!("save it as UTF-8, or read it in a @py block with {}", code_page_way()) } else { fix.to_string() };
            let err = ErrorValue::new("IOError", format!("'{}' is not valid UTF-8: {kind}", shown_path(&path)));
            let _ = err.hint.set(fix.into());
            Err(Flow::Error(Rc::new(err)))
        }
    }
}

/// Windows: `read_file` or `write_file` (`verb` `read` or `write`) of a pipe or a device WSL made
/// (`module::WslFile::Special`, seen through \\wsl.localhost), refused before it is opened:
/// Windows takes it for a file, and opening a pipe would wait 10 s for its other end (leaving
/// WSL's file server waiting too), a read of a device read Linux's (1 GiB of `/dev/zero`) (5.10).
/// One look at the path's attributes (`wsl_file`), and only on a share (`on_a_share`).
fn wsl_special(verb: &str, path: &str) -> Exec<()> {
    if !on_a_share(path) || super::module::wsl_file(std::path::Path::new(path)) != Some(super::module::WslFile::Special) {
        return Ok(());
    }
    let err = ErrorValue::new("IOError", format!("cannot {verb} '{}': it is a pipe or a device, not a file", shown_path(path)));
    let how = if verb == "read" { "read" } else { "write to" };
    let _ = err.hint.set(format!("Windows can't {how} WSL's pipes and devices: run V2 inside WSL").into());
    Err(Flow::Error(Rc::new(err)))
}

/// Windows: whether `path` is on a `\\server\share` or a network drive (as WSL's files are, on
/// \\wsl.localhost or a drive mapped to it), by its text, or a relative one by the current
/// folder's: where `wsl_special` looks, as its look at a local file would make a small read a
/// quarter slower (5.10). A pipe or a device (`device_path`) is none: the look would open it.
fn on_a_share(path: &str) -> bool {
    let unc = |b: &[u8]| b.len() >= 2 && matches!(b[0], b'\\' | b'/') && matches!(b[1], b'\\' | b'/');
    let drive = |b: &[u8]| (b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic()).then(|| (b[0] as char).to_ascii_uppercase());
    if !cfg!(windows) || device_path(path) {
        return false;
    }
    if unc(path.as_bytes()) {
        return true;
    }
    if let Some(letter) = drive(path.as_bytes()) {
        return drive_type(letter) == REMOTE_DRIVE;
    }
    let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    unc(cwd.as_bytes()) || drive(cwd.as_bytes()).is_some_and(|l| drive_type(l) == REMOTE_DRIVE)
}

/// The bytes at `path`, or `None` past `process::read_cap()` (1 GiB), read no further, so a
/// device that never ends can't use up the memory (5.10). Unix: standard input by any of its
/// names is V2's own stream, as for input(): what input() holds comes first, then the rest from
/// where the descriptor is (after what the shell read, for a file). Opened again, a pipe would
/// lose the one and a file would start over.
fn read_path(path: &str) -> std::io::Result<Option<Vec<u8>>> {
    let cap = process::read_cap();
    let mut buf = Vec::new();
    if cfg!(unix) && matches!(path, "/dev/stdin" | "/dev/fd/0" | "/proc/self/fd/0") {
        return Ok(process::read_capped(std::io::stdin().lock(), cap, 0, &mut buf)?.then_some(buf));
    }
    let file = std::fs::File::open(path)?;
    // A regular file says its size: one too big is refused before it is read.
    let size = file.metadata().ok().filter(std::fs::Metadata::is_file).map_or(0, |m| m.len());
    if size > cap as u64 {
        return Ok(None);
    }
    Ok(process::read_capped(file, cap, size as usize, &mut buf)?.then_some(buf))
}

/// `write_file(path, data, append: false)`: a str as UTF-8 or bytes as is; creates or
/// truncates, or with `append` adds to the end, creating a missing file (5.10).
fn write_file(it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "write_file", "path")?;
    shell_shorthand("write", &path)?;
    let path = verbatim_parts(&path);
    let bytes: &[u8] = match &args[1] {
        Value::Str(s) => s.as_bytes(),
        Value::Bytes(b) => b,
        other => {
            let err = ErrorValue::new("TypeError", format!("write_file() argument 'data' must be a str or bytes, got {}", other.type_name()));
            let hint = match other {
                Value::List(_) => Some(r#"write one item per line with xs.join("\n") (read it back with read_file(f).lines()), or the whole list with json_stringify(xs)"#),
                Value::Dict(_) => Some("write it as JSON with json_stringify(d) (read it back with json_parse(read_file(f)))"),
                Value::Int(_) | Value::Float(_) | Value::Bool(_) => Some("write it as text with str(x)"),
                _ => None,
            };
            if let Some(h) = hint {
                let _ = err.hint.set(h.into());
            }
            return Err(Flow::Error(Rc::new(err)));
        }
    };
    let append = bool_arg(&args[2], "write_file", "append")?;
    // `write_file("/dev/stdout", data)`: V2's own stdout or stderr, written through V2's own
    // stream, so it comes after what print wrote and a redirected file isn't opened again at its
    // start and cut short (5.10).
    match path.as_ref() {
        "/dev/stdout" | "/dev/fd/1" | "/proc/self/fd/1" if cfg!(unix) => {
            it.out.write_bytes(bytes)?;
            return Ok(Value::Null);
        }
        "/dev/stderr" | "/dev/fd/2" | "/proc/self/fd/2" if cfg!(unix) => return to_stderr(it, bytes),
        _ => {}
    }
    wsl_special("write", &path)?;
    let r = if append {
        std::fs::OpenOptions::new().append(true).create(true).open(&*path).and_then(|mut f| f.write_all(bytes))
    } else {
        replace_file(&path, bytes)
    };
    match r {
        Ok(()) => Ok(Value::Null),
        Err(e) => file_error("write", &path, &e),
    }
}

/// `write_file`'s new contents for `path`, created if missing (5.10). Unix: `fs::write`, whose
/// locks are advisory. Windows: `fs::write` cuts a file before it writes, and another program's
/// lock of a part of it (a database's) doesn't stop the cut, only the write after it, so a failed
/// write left it empty. So the file is opened as it is, locked whole (`try_lock`: another
/// program's lock anywhere in it, past the end too, refuses it, as ERROR_LOCK_VIOLATION, before
/// anything changed; a file system without locks goes on without), cut (a file another program
/// has mapped refuses that, ERROR_USER_MAPPED_FILE, still whole), written and unlocked; never
/// written first and cut after, which would leave a mapped file part old, part new. What isn't a
/// disk file (a named pipe, a console) is only written: it can't be cut.
fn replace_file(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_DISK};
        // Opened as `OPEN_ALWAYS`, as `fs::write` does: a hidden or system file keeps working.
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(path)?;
        // SAFETY: a handle that `f` keeps open during the call.
        if unsafe { GetFileType(f.as_raw_handle()) } != FILE_TYPE_DISK {
            return f.write_all(bytes);
        }
        let locked = match f.try_lock() {
            Ok(()) => true,
            Err(std::fs::TryLockError::WouldBlock) => return Err(std::io::Error::from_raw_os_error(33)),
            Err(std::fs::TryLockError::Error(_)) => false,
        };
        let written = f.set_len(0).and_then(|()| f.write_all(bytes));
        if locked {
            let _ = f.unlock();
        }
        written
    }
    #[cfg(not(windows))]
    std::fs::write(path, bytes)
}

/// `exists(path)`: whether `fs::metadata` succeeds, symlinks followed; never an error (5.10).
fn exists(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "exists", "path")?;
    if path.is_empty() {
        return Ok(Value::Bool(false));
    }
    // `exists("~/.config/x")` would be false for a file that is there: refused as in the others.
    shell_shorthand("check", &path)?;
    let path = verbatim_parts(&path);
    Ok(Value::Bool(std::fs::metadata(&*path).is_ok()))
}

/// `list_dir(path)`: the entry names, sorted by code point (5.10).
fn list_dir(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "list_dir", "path")?;
    shell_shorthand("list", &path)?;
    let path = verbatim_parts(&path);
    let failed = |e: std::io::Error| file_error("list", &path, &e);
    let entries = match std::fs::read_dir(&*path) {
        Ok(rd) => rd,
        Err(e) => return failed(e),
    };
    let mut names = Vec::new();
    for entry in entries {
        let name = match entry {
            Ok(entry) => entry.file_name(),
            Err(e) => return failed(e),
        };
        match name.into_string() {
            Ok(name) => names.push(name),
            Err(bad) => {
                let shown = shown_path(&bad.to_string_lossy());
                // A name that can't be renamed (an upload, a share): a block lists the folder as bytes.
                let err = ErrorValue::new("IOError", format!("cannot list '{}': the name '{shown}' is not valid UTF-8 (\u{FFFD} marks what isn't), and V2's strings are: rename it", shown_path(&path)));
                let _ = err.hint.set("to list such names as they are, keep them as bytes in a @py block: @py(d) { import os; os.listdir(os.fsencode(d)) }, and open one there with os.path.join(os.fsencode(d), name)".into());
                return Err(Flow::Error(Rc::new(err)));
            }
        }
    }
    names.sort();
    Ok(Value::list(names.into_iter().map(|n| Value::Str(n.into())).collect()))
}

/// `make_dir(path)`: with any missing parents; an existing folder is fine (5.10).
fn make_dir(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "make_dir", "path")?;
    shell_shorthand("create", &path)?;
    let path = verbatim_parts(&path);
    // Windows: `/c/Users/me/out`, Git Bash's spelling, would make `\c\Users\me\out` on this
    // drive, where nothing else finds it, and WSL's `/mnt/c/…` `\mnt\c\…`: refused as the missing
    // folder it is (5.10).
    if let (Some((written, ..)), Some(h)) = (unix_drive(&path), msys_hint(&path)) {
        let err = ErrorValue::new("IOError", format!("cannot create '{}': there is no folder '{written}'", shown_path(&path)));
        let _ = err.hint.set(h.into());
        return Err(Flow::Error(Rc::new(err)));
    }
    // Windows: `/tmp/out` would make `\tmp` at the top of the drive, where every user can write.
    let refused = if cfg!(windows) { tmp_refusal(&path, &|tmp| std::path::Path::new(tmp).is_dir()) } else { None };
    if let Some(err) = refused.or_else(|| linux_dir_refusal(&path)) {
        return Err(Flow::Error(Rc::new(err)));
    }
    match std::fs::create_dir_all(&*path) {
        // Newer Rust's create_dir_all stops at a root it can't make without an error (`Z:/` on a
        // drive that isn't there): the folder must be there now, else the OS's reason for it.
        Ok(()) if path.is_empty() || std::path::Path::new(&*path).is_dir() => Ok(Value::Null),
        Ok(()) => {
            let e = std::fs::metadata(&*path).err().unwrap_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound));
            file_error("create", &path, &e)
        }
        // A root that can't be made (a drive or share that isn't there) is, in some Rust versions,
        // `create_dir_all`'s own "failed to create whole tree" (newer ones give the OS's reason):
        // the root's own reason instead (5.10), the last ancestor's. Other errors without an OS
        // code (a NUL in the path) are Rust's true reason, kept.
        Err(e) if whole_tree_error(&e) => {
            let e = match std::path::Path::new(&*path).ancestors().last().map(std::fs::metadata) {
                Some(Err(root)) => root,
                _ => e,
            };
            file_error("create", &path, &e)
        }
        Err(e) => file_error("create", &path, &e),
    }
}

/// `create_dir_all`'s own error for a root it can't make, `failed to create whole tree` (5.10):
/// it has no OS code, but neither has Rust's for a NUL in the path, which is the true reason.
fn whole_tree_error(e: &std::io::Error) -> bool {
    e.raw_os_error().is_none() && e.to_string() == "failed to create whole tree"
}

/// `remove_file(path)`: a file, never a folder; a link, not what it points to, as `rm` does
/// (5.10). Windows removes a link to a folder (a junction, a directory symlink) as a folder, so
/// one that leads to a folder goes with `remove_dir`, which takes the link alone; written with a
/// slash at its end (`link/`) it names the folder, refused as one, and one that leads nowhere
/// keeps `module::link_error`'s message.
fn remove_file(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "remove_file", "path")?;
    shell_shorthand("remove", &path)?;
    let path = verbatim_parts(&path);
    #[cfg(windows)]
    let dir_link = super::module::trim_separators(&path).len() == path.len()
        && std::fs::symlink_metadata(&*path).is_ok_and(|m| std::os::windows::fs::FileTypeExt::is_symlink_dir(&m.file_type()))
        && std::path::Path::new(&*path).is_dir();
    #[cfg(not(windows))]
    let dir_link = false;
    let removed = if dir_link { std::fs::remove_dir(&*path) } else { std::fs::remove_file(&*path) };
    match removed {
        Ok(()) => Ok(Value::Null),
        Err(e) => file_error("remove", &path, &e),
    }
}

/// `script_dir()`: the folder of the file containing the call, not the entry file's (5.10).
fn script_dir(it: &mut Interp, _args: Vec<Value>, at: Span) -> Exec<Value> {
    let path = &it.sm.file(at.file).path;
    Ok(Value::str(&path.parent().unwrap_or(path).to_string_lossy()))
}

/// `exec(argv, input = null)`: a program run without a shell, found like an engine command
/// (7.5) with the cwd as base; its outputs captured whole and decoded leniently, its tree
/// killed when it ends. Returns `{code, stdout, stderr}`; a non-zero code is not an error (5.10).
/// Whether `name` is a program in a node_modules/.bin of the current folder or one above it.
fn npm_tool(name: &str) -> bool {
    if name.contains(['/', '\\']) {
        return false;
    }
    let Ok(cwd) = std::env::current_dir() else { return false };
    cwd.ancestors().map(|d| d.join("node_modules").join(".bin")).any(|bin| bin.join(name).is_file() || (cfg!(windows) && bin.join(format!("{name}.cmd")).is_file()))
}

/// A named argument a builtin doesn't take, that people try: what to write instead.
pub(crate) fn named_hint(builtin: &str, name: &str) -> Option<String> {
    if builtin != "exec" || !matches!(name, "env" | "cwd" | "dir" | "environment") {
        return None;
    }
    Some(if cfg!(windows) {
        r#"exec takes argv and input only: for a variable or another folder, start a shell, as in exec(["cmd", "/c", "cd /d web && set \"NODE_ENV=production\" && npm run build"])"#.to_string()
    } else {
        r#"exec takes argv and input only: for a variable or another folder, start a shell, as in exec(["sh", "-c", "cd web && NODE_ENV=production npm run build"])"#.to_string()
    })
}

/// Windows: a working folder on a `\\server\share` (`\\wsl.localhost\Ubuntu\…`, also as
/// `\\?\UNC\…`), as cmd's `pushd` takes it; cmd.exe can't work in one (7.5).
/// A cmd.exe command line whose first command moves to an absolute folder, `pushd
/// "\\server\share\p"` or `cd /d C:\p` (`X:\`, `X:/` or `\\`, quoted or not): it works from the
/// Windows folder cmd starts in, where a relative one (`cd /d logs`) would be looked for (7.5).
fn moves_to_absolute(line: &str) -> bool {
    let line = line.trim_start();
    let lower = line.to_ascii_lowercase();
    let Some(rest) = ["pushd ", "cd /d ", "chdir /d "].iter().find(|c| lower.starts_with(*c)).map(|c| &line[c.len()..]) else { return false };
    let target = rest.trim_start().trim_start_matches('"');
    let b = target.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/')) || target.starts_with(r"\\")
}

/// Windows: exec's refusal of cmd.exe, or a `.bat` or `.cmd` file it would run (`npm.cmd`), in a
/// `\\server\share` working folder, where cmd says "UNC paths are not supported" and works in the
/// Windows folder instead, exit code 0 (7.5): what to run instead. Not for a cmd.exe whose
/// command line first moves to an absolute folder (`pushd "\\server\share\p" && …`).
fn cmd_in_share(argv: &[OsString], cwd: &std::path::Path) -> Option<String> {
    let folder = crate::engines::resolve::share_folder(cwd)?;
    let program = std::path::Path::new(argv.first()?);
    let file = program.file_name()?.to_string_lossy().into_owned();
    let ext = program.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let line = argv.iter().skip(1).position(|a| a.eq_ignore_ascii_case("/c") || a.eq_ignore_ascii_case("/k")).map(|i| {
        let rest: Vec<String> = argv[i + 2..].iter().map(|a| a.to_string_lossy().into_owned()).collect();
        rest.join(" ")
    });
    let who = if file.eq_ignore_ascii_case("cmd.exe") && !line.is_some_and(|l| moves_to_absolute(&l)) {
        "cmd.exe".to_string()
    } else if ext == "bat" || ext == "cmd" {
        format!("{file} runs in cmd.exe, which")
    } else {
        return None;
    };
    let mut line = String::new();
    super::value::quoted(&format!("pushd \"{folder}\" && …"), &mut line);
    Some(format!("exec: {who} {}: run the program itself, or exec([\"cmd\", \"/c\", {line}])", crate::engines::resolve::cmd_cant_work()))
}

fn exec(it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let items = match &args[0] {
        Value::List(l) => l.borrow().clone(),
        // `exec("git status")`: its words, as the list exec takes; a program's path with a space
        // stays whole.
        Value::Str(s) => {
            let words: Vec<&str> = if std::path::Path::new(&**s).is_file() { vec![&**s] } else { s.split_whitespace().collect() };
            if words.is_empty() {
                return type_error("exec() argument 'argv' must be a list, got str");
            }
            // `exec("dir")`: a command of the shell needs the shell, so the list alone won't do.
            let given: Vec<Value> = words.iter().map(|w| Value::str(w)).collect();
            if let Some(h) = shell_command_hint(words[0], &given) {
                let err = ErrorValue::new("TypeError", "exec() argument 'argv' must be a list, got str");
                let _ = err.hint.set(h.into());
                return Err(Flow::Error(Rc::new(err)));
            }
            let mut list = String::new();
            for (i, w) in words.iter().enumerate() {
                if i > 0 {
                    list.push_str(", ");
                }
                super::value::quoted(w, &mut list);
            }
            return type_error(format!("exec() argument 'argv' must be a list, got str — as in exec([{list}]): exec runs a program, without a shell"));
        }
        other => return type_error(format!("exec() argument 'argv' must be a list, got {}", other.type_name())),
    };
    let mut argv = Vec::with_capacity(items.len());
    for (i, v) in items.iter().enumerate() {
        match v {
            // Rust's "nul byte found in provided data" would name the program, not the item.
            Value::Str(s) if s.contains('\0') => {
                let mut q = String::new();
                super::value::quoted(s, &mut q);
                return value_error(format!("exec() argument 'argv' holds a NUL character at [{i}]: {q}: a program's name and arguments can't contain one"));
            }
            Value::Str(s) => argv.push(OsString::from(&**s)),
            other => return type_error(format!("exec() argument 'argv' must hold only str, got {} at [{i}]", other.type_name())),
        }
    }
    let Some(Value::Str(name)) = items.first() else {
        return value_error("exec() argument 'argv' must not be empty");
    };
    // Windows: `f"${script_dir()}/tool.exe"` in a script run by its `\\?\` path (2.1).
    let written = verbatim_parts(name);
    let name: &str = &written;
    let input = match &args[1] {
        Value::Null => None,
        Value::Str(s) => Some(s.as_bytes().to_vec()),
        Value::Bytes(b) => Some(b.to_vec()),
        other => return type_error(format!("exec() argument 'input' must be a str, bytes or null, got {}", other.type_name())),
    };
    let lookup = Lookup::from_env();
    argv[0] = match first_program(name, &lookup) {
        Some(p) => p.into_os_string(),
        None if is_drive_relative(std::path::Path::new(name)) => {
            return io_error(format!("exec: cannot start '{name}': a drive-relative path: write it relative to the working directory, or absolute"));
        }
        None if name.contains(['/', '\\']) => {
            let p: std::path::PathBuf = lookup.cwd.join(name).components().collect();
            if p.is_dir() {
                return io_error(format!("exec: cannot start '{name}': it is a folder, not a program"));
            }
            // Not there: a batch file's path would go to cmd.exe, which says so as its output.
            if !p.exists() {
                let err = ErrorValue::new("IOError", format!("exec: cannot start '{name}': there is no such file"));
                if let Some(h) = msys_hint(name).or_else(|| linux_path_hint(name)) {
                    let _ = err.hint.set(h.into());
                }
                return Err(Flow::Error(Rc::new(err)));
            }
            // A path that names nothing usable: starting it gives the OS's reason.
            p.into_os_string()
        }
        None => {
            // `env -i`, cron: nothing was searched (round 16).
            let unset = lookup.path.as_ref().is_none_or(|p| p.is_empty());
            let err = ErrorValue::new(
                "IOError",
                if unset { format!("exec: cannot start '{name}': not found, as PATH isn't set: set it, or give the program's full path") } else { format!("exec: cannot start '{name}': not found on PATH") },
            );
            // Ubuntu and macOS have python3 and no python.
            let python3 = (name == "python" && !unset).then(|| first_program("python3", &lookup)).flatten();
            // Windows' own shells run a program in the current folder by its bare name; V2
            // looks only on PATH, as a Unix shell does (5.10).
            let here = first_program(&format!("./{name}"), &lookup).and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
            if let Some(hint) = shell_command_hint(name, &items) {
                let _ = err.hint.set(hint.into());
            } else if python3.is_some() {
                let _ = err.hint.set("this system names it python3: write exec([\"python3\", …])".into());
            } else if let Some(file) = here {
                let shown = format!("./{file}");
                let mut path = String::new();
                super::value::quoted(&shown, &mut path);
                let hint = unmarked_hint(&lookup.cwd.join(&file), &shown, true)
                    .unwrap_or_else(|| format!("V2 doesn't look in the current folder for a bare name: write exec([{path}])"));
                let _ = err.hint.set(hint.into());
            } else if npm_tool(name) {
                // A tool an npm package installed for the project (prettier, tsc, eslint).
                let mut quoted = String::new();
                super::value::quoted(name, &mut quoted);
                let _ = err.hint.set(format!("{name} is in node_modules/.bin, where npm puts a package's programs: run it with npx, as in exec([\"npx\", {quoted}, …])").into());
            } else if let Some(hint) = linux_program_hint(name, &items) {
                let _ = err.hint.set(hint.into());
            }
            return Err(Flow::Error(Rc::new(err)));
        }
    };
    // Windows: cmd.exe in a \\server\share folder would run in C:\Windows instead (7.5).
    if let Some(refused) = cmd_in_share(&argv, &lookup.cwd) {
        return io_error(refused);
    }
    it.out.flush_or_stop();
    // A Python program writes a pipe in the locale's encoding (ANSI on Windows: café as cafÚ in
    // the console's code page, and CJK not at all): UTF-8 instead, unless the user chose (5.10).
    let utf8 = std::env::var_os("PYTHONIOENCODING").is_none().then(|| (OsString::from("PYTHONIOENCODING"), OsString::from("utf-8")));
    // `exec(["cmd", "/c", "cd /d \"my dir\" && dir"])`: the one string after /c is a command line
    // for cmd.exe, which reads it as typed; quoted for a program (`\"`), cmd would misread it.
    let is_cmd = std::path::Path::new(&argv[0]).file_name().is_some_and(|f| f.eq_ignore_ascii_case("cmd.exe"));
    let raw_tail = cfg!(windows) && is_cmd && argv.len() >= 3 && argv[argv.len() - 2].to_str().is_some_and(|s| s.eq_ignore_ascii_case("/c") || s.eq_ignore_ascii_case("/k"));
    verbatim_arguments(&mut argv, raw_tail);
    // On Unix the program sees the name as written (`ls`), as from a shell (7.5).
    let ran = process::run_captured_as(&argv, Some(std::ffi::OsStr::new(name)), utf8.as_slice(), None, input, raw_tail);
    // A signal that came while the program ran (its own `kill $PPID` too) ends the run here.
    stop_if_interrupted();
    let c = match ran {
        Ok(c) => c,
        Err(e) => {
            let err = ErrorValue::new("IOError", format!("exec: cannot start '{name}': {}", e.reason));
            let file = std::path::Path::new(&argv[0]);
            if let Some(h) = unmarked_hint(file, name, false).or_else(|| script_hint(name).filter(|_| file.is_file())) {
                let _ = err.hint.set(h.into());
            }
            return Err(Flow::Error(Rc::new(err)));
        }
    };
    // Past what a str holds: an endless program (`yes`), or a zip bomb unpacked to stdout.
    if c.too_much {
        return io_error(format!("exec: '{}' wrote more than 1 GiB of output: send it to a file in the command, or read it in pieces in a block", shown_path(name)));
    }
    let code = match (c.status.code, c.status.signal) {
        (_, Some(signal)) => 128 + signal as i64,
        (Some(code), None) if cfg!(windows) => code as u32 as i64,
        (Some(code), None) => code as i64,
        (None, None) => -1,
    };
    let mut result = DictMap::new();
    result.insert(Rc::from("code"), Value::Int(code));
    result.insert(Rc::from("stdout"), Value::str(&child_text(&c.stdout)));
    result.insert(Rc::from("stderr"), Value::str(&child_text(&c.stderr)));
    Ok(Value::dict(result))
}

/// `sleep(seconds)` on the interpreter thread; an interrupt still ends the run through the
/// watchdog (5.10, 6.5).
fn sleep(it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let (secs, shown) = match &args[0] {
        Value::Int(n) => (*n as f64, n.to_string()),
        Value::Float(x) => (*x, float_str(*x)),
        other => return type_error(format!("sleep() argument 'seconds' must be an int or float, got {}", other.type_name())),
    };
    if secs < 0.0 {
        return value_error(format!("sleep() argument 'seconds' must not be negative, got {shown}"));
    }
    match Duration::try_from_secs_f64(secs) {
        Ok(d) => {
            // What was printed before a wait is out during it, and survives an interrupt.
            it.out.flush_or_stop();
            std::thread::sleep(d)
        }
        Err(_) => return value_error(format!("sleep() argument 'seconds' is out of range: {shown}")),
    }
    Ok(Value::Null)
}

/// `time()`: seconds since 1970-01-01 UTC as a float (5.10).
fn time(_it: &mut Interp, _args: Vec<Value>, _at: Span) -> Exec<Value> {
    Ok(Value::Float(match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }))
}

/// `min`/`max`: the arguments, or a single list argument's items; all numbers or all strings.
/// An item replaces the best only when strictly smaller (larger), so the first wins ties (5.10).
fn extreme(args: Vec<Value>, name: &str, want: Ordering) -> Exec<Value> {
    let Value::List(values) = &args[0] else { unreachable!("the rest parameter is a list") };
    let mut items = values.borrow().clone();
    if let [Value::List(only)] = &items[..] {
        let inner = only.borrow().clone();
        items = inner;
    }
    let Some(first) = items.first() else { return value_error(format!("{name}() of an empty list")) };
    let kind = |v: &Value| match v {
        Value::Int(_) | Value::Float(_) => Some(0),
        Value::Str(_) => Some(1),
        _ => None,
    };
    if let Some(bad) = items.iter().find(|v| kind(v).is_none()) {
        let hint = handle_hint(bad, &format!("{name}(h)"), &format!("Math.{name}(...h)"));
        return type_error(format!("{name}() needs numbers or strings, got {}{hint}", bad.type_name()));
    }
    if let Some(other) = items.iter().find(|v| kind(v) != kind(first)) {
        return type_error(format!("{name}() can't compare {} and {}", first.type_name(), other.type_name()));
    }
    let mut best = first.clone();
    for x in &items[1..] {
        if order(x, &best, name)? == Some(want) {
            best = x.clone();
        }
    }
    Ok(best)
}

/// `sum(xs)`: a list of numbers folded from `0` with 5.3's `+` (5.10).
fn sum(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let items = match &args[0] {
        Value::List(l) => l.borrow().clone(),
        other => return type_error(format!("sum() argument 'xs' must be a list, got {}", other.type_name())),
    };
    let mut total = Value::Int(0);
    for (i, x) in items.iter().enumerate() {
        if !matches!(x, Value::Int(_) | Value::Float(_)) {
            return type_error(format!("sum() needs a list of numbers, got {} at [{i}]", x.type_name()));
        }
        total = binop(BinOp::Add, &total, x)?;
    }
    Ok(total)
}

/// `abs(x)` of an int (overflow is a ValueError) or a float (5.10).
fn abs(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    match &args[0] {
        Value::Int(n) => match n.checked_abs() {
            Some(a) => Ok(Value::Int(a)),
            None => value_error("integer overflow in abs()"),
        },
        Value::Float(x) => Ok(Value::Float(x.abs())),
        other => type_error(format!("abs() needs an int or float, got {}", other.type_name())),
    }
}

/// `round(x, digits = 0)`: a float printed as `${x:.Nf}` prints it (ties to even on the exact
/// binary value) and read back; `digits` 0 gives an int. An int comes back unchanged (5.10).
fn round(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let digits = int_arg(&args[1], "round", "digits")?;
    if digits < 0 {
        return value_error(format!("round() argument 'digits' must be >= 0, got {digits}"));
    }
    let x = match &args[0] {
        Value::Int(n) => return Ok(Value::Int(*n)),
        Value::Float(x) => *x,
        other => return type_error(format!("round() argument 'x' must be an int or float, got {}", other.type_name())),
    };
    if digits == 0 {
        // NaN, infinity and floats beyond i64 have no int, as in int() (5.10).
        return match format!("{x:.0}").parse::<i64>() {
            Ok(n) if x.is_finite() => Ok(Value::Int(n)),
            _ => value_error(format!("cannot convert {} to int", float_str(x))),
        };
    }
    // No f64 needs more than 323 decimals.
    if digits >= 324 || !x.is_finite() {
        return Ok(Value::Float(x));
    }
    let text = format!("{x:.prec$}", prec = digits as usize);
    Ok(Value::Float(text.parse().expect("a formatted finite float")))
}

/// `div(a, b)`: integer division rounded down, so `div(a, b) * b + a % b == a` (5.10).
fn div(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let a = int_arg(&args[0], "div", "a")?;
    let b = int_arg(&args[1], "div", "b")?;
    if b == 0 {
        return value_error("division by zero");
    }
    let Some(q) = a.checked_div(b) else { return value_error("integer overflow in div()") };
    Ok(Value::Int(if a % b != 0 && (a < 0) != (b < 0) { q - 1 } else { q }))
}

/// `assert(cond, message = "assertion failed")`: a falsy `cond` throws an AssertionError at
/// the call (5.10).
fn assert(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let message = str_arg(&args[1], "assert", "message")?;
    if args[0].truthy() {
        Ok(Value::Null)
    } else {
        fail("AssertionError", &message)
    }
}

/// `getenv(name, default = null)`, decoded lossily (5.10).
fn getenv(_it: &mut Interp, mut args: Vec<Value>, _at: Span) -> Exec<Value> {
    let name = str_arg(&args[0], "getenv", "name")?;
    if name.contains('\0') {
        let mut q = String::new();
        super::value::quoted(&name, &mut q);
        return value_error(format!("invalid environment variable name: {q}: a name can't contain a NUL character"));
    }
    if name.is_empty() || name.contains('=') {
        return value_error(format!("invalid environment variable name: '{name}'"));
    }
    Ok(match std::env::var_os(&*name) {
        Some(v) => Value::Str(v.to_string_lossy().into_owned().into()),
        None => args.pop().unwrap_or(Value::Null),
    })
}

/// `exit(code = 0)`: ends the run at once; no `catch` or `finally` runs (5.8, 5.10).
fn exit(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let code = int_arg(&args[0], "exit", "code")?;
    match exit_code(code) {
        Some(c) => Err(Flow::Exit(c)),
        None => {
            let err = ErrorValue::new("ValueError", format!("exit code {code} is out of range"));
            let range = if cfg!(windows) {
                "exit() takes a code from -2147483648 to 4294967295 on Windows"
            } else {
                "exit() takes a code from -2147483648 to 2147483647, and a shell sees only 0-255: use those for a code a shell checks"
            };
            let _ = err.hint.set(range.into());
            Err(Flow::Error(Rc::new(err)))
        }
    }
}

/// The code `exit(code)` ends the process with: an i32, and on Windows, whose exit codes are 32
/// unsigned bits (exec's `code` of a crashed program is 3221225477), 2147483648 to 4294967295 too,
/// passed on as those same bits (5.10).
fn exit_code(code: i64) -> Option<i32> {
    match i32::try_from(code) {
        Ok(c) => Some(c),
        Err(_) if cfg!(windows) => u32::try_from(code).ok().map(|c| c as i32),
        Err(_) => None,
    }
}

/// A `str` argument, or `TypeError: print() argument 'sep' must be a str, got int`.
pub fn str_arg(v: &Value, func: &str, param: &str) -> Exec<Str> {
    match v {
        Value::Str(s) => Ok(s.clone()),
        // A pathlib.Path or a JS URL: its text is str(h).
        Value::Handle(_) => {
            let err = ErrorValue::new("TypeError", format!("{func}() argument '{param}' must be a str, got handle"));
            let _ = err.hint.set(format!("pass its text, as in {func}(str(h))").into());
            Err(Flow::Error(Rc::new(err)))
        }
        other => type_error(format!("{func}() argument '{param}' must be a str, got {}", other.type_name())),
    }
}

/// A `bool` argument, or `TypeError: read_file() argument 'binary' must be a bool, got int`.
pub fn bool_arg(v: &Value, func: &str, param: &str) -> Exec<bool> {
    match v {
        Value::Bool(b) => Ok(*b),
        other => type_error(format!("{func}() argument '{param}' must be a bool, got {}", other.type_name())),
    }
}

/// `print(...values, sep: " ", end: "\n", stderr: false)`: one write to `Out`, or with
/// `stderr` one write to the host's stderr after flushing `Out` (5.10).
fn print(it: &mut Interp, args: Vec<Value>, at: Span) -> Exec<Value> {
    let sep = str_arg(&args[1], "print", "sep")?;
    let end = str_arg(&args[2], "print", "end")?;
    let on_stderr = bool_arg(&args[3], "print", "stderr")?;
    let Value::List(values) = &args[0] else { unreachable!("the rest parameter is a list") };
    let values = values.borrow().clone();
    let mut text = String::new();
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            text.push_str(&sep);
        }
        text.push_str(&it.str_of(v, at)?);
    }
    text.push_str(&end);
    if !on_stderr {
        it.out.write_str(&text)?;
        return Ok(Value::Null);
    }
    to_stderr(it, text.as_bytes())
}

/// `print(stderr: true)` and `write_file("/dev/stderr", …)`: one write to the host's stderr
/// after `Out` is flushed, so on one stream the text keeps program order (5.10).
fn to_stderr(it: &mut Interp, bytes: &[u8]) -> Exec<Value> {
    it.out.flush_or_stop();
    match write_stderr(bytes) {
        Ok(()) => Ok(Value::Null),
        Err(e) => io_error(format!("cannot write to stderr: {}", os_message(&e))),
    }
}

/// Program text for the host's stderr (`print(stderr: true)`, a block's stderr text, 9.9), written
/// at once. A reader that went away (`v2 x.v2 2>&1 | head`) stops the run quietly with 141, as on
/// stdout (6.5); another failure is returned.
pub fn write_stderr(bytes: &[u8]) -> std::io::Result<()> {
    stop_if_interrupted();
    let r = {
        let mut err = std::io::stderr().lock();
        err.write_all(bytes).and_then(|_| err.flush())
    };
    match r {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => process::exit_broken_pipe(),
        Err(e) => Err(e),
        Ok(()) => {
            wrote_stderr(bytes);
            Ok(())
        }
    }
}

/// `error(type, message, data = null)`: an error value with no location until raised (5.9).
fn error(_it: &mut Interp, mut args: Vec<Value>, _at: Span) -> Exec<Value> {
    let kind = str_arg(&args[0], "error", "type")?;
    if kind.is_empty() {
        return fail("ValueError", "error() argument 'type' must not be empty");
    }
    if matches!(args[1], Value::Null) {
        return fail("ArityError", format!("error() needs a type and a message, as in error(\"ValueError\", \"{kind}\"); throw \"{kind}\" throws a plain Error"));
    }
    let message = str_arg(&args[1], "error", "message")?;
    let data = args.pop().unwrap_or(Value::Null);
    Ok(Value::Error(Rc::new(ErrorValue::with_data((&kind).into(), (&message).into(), data))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16le(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn utf16_output_is_decoded() {
        // With a byte order mark, on every system; an odd last byte is U+FFFD.
        let mut bom = vec![0xFF, 0xFE];
        bom.extend(utf16le("héllo 日本"));
        assert_eq!(child_text(&bom), "héllo 日本");
        bom.push(b'x');
        assert_eq!(child_text(&bom), "héllo 日本\u{FFFD}");
        let be: Vec<u8> = [0xFE, 0xFF].into_iter().chain("ok".encode_utf16().flat_map(u16::to_be_bytes)).collect();
        assert_eq!(child_text(&be), "ok");
        // Without one (`cmd /u`, `wsl --list`): Windows only, where programs write it.
        let plain = utf16le("Ubuntu\r\ndocker-desktop\r\n");
        assert_eq!(child_text(&plain), if cfg!(windows) { "Ubuntu\r\ndocker-desktop\r\n".to_string() } else { String::from_utf8_lossy(&plain).into_owned() });
        // UTF-8, and binary data with NULs on both sides, stay as they were.
        assert_eq!(child_text("café\n".as_bytes()), "café\n");
        let ints = [1u8, 0, 0, 0, 2, 0, 0, 0];
        assert_eq!(child_text(&ints), String::from_utf8_lossy(&ints));
    }

    #[test]
    fn exec_refuses_cmd_in_a_share_folder_unless_it_moves() {
        // Round 21 (W1), 7.5: cmd.exe, and a batch file it runs, in a \\server\share working
        // folder (also as \\?\UNC\…) would run in the Windows folder; a cmd line that first moves
        // (pushd, cd /d) works. Drive folders and other programs are fine; elsewhere a path has
        // no share prefix at all.
        let argv = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
        let share = std::path::Path::new(r"\\wsl.localhost\Ubuntu\home\ann\p");
        let cmd = argv(&[r"C:\WINDOWS\system32\cmd.exe", "/c", "npm run build"]);
        let npm = argv(&[r"C:\Program Files\nodejs\npm.cmd", "run", "build"]);
        if cfg!(windows) {
            let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
            let rest = format!(
                "can't work in a \\\\server\\share folder and would run in {windows}: run the program itself, or exec([\"cmd\", \"/c\", \"pushd \\\"\\\\\\\\wsl.localhost\\\\Ubuntu\\\\home\\\\ann\\\\p\\\" && …\"])"
            );
            assert_eq!(cmd_in_share(&cmd, share), Some(format!("exec: cmd.exe {rest}")));
            assert_eq!(cmd_in_share(&npm, share), Some(format!("exec: npm.cmd runs in cmd.exe, which {rest}")));
            assert_eq!(cmd_in_share(&npm, std::path::Path::new(r"\\?\UNC\wsl.localhost\Ubuntu\home\ann\p")), Some(format!("exec: npm.cmd runs in cmd.exe, which {rest}")));
            assert_eq!(cmd_in_share(&argv(&["C:\\x\\BUILD.BAT"]), share).map(|m| m.starts_with("exec: BUILD.BAT runs in cmd.exe, ")), Some(true));
            // The review of round 21: a relative folder is looked for in the Windows folder cmd
            // starts in (`cd /d logs` is C:\Windows\Logs), so it moves nowhere V2 meant.
            for relative in [&["cmd.exe", "/c", "cd /d logs && cd"][..], &["cmd.exe", "/c", "pushd src && dir"], &["cmd.exe", "/c", "chdir /d \"build\" && make"], &["cmd.exe", "/c", "pushd", "system32"], &["cmd.exe", "/c", "cd /d build && make"]] {
                assert_eq!(cmd_in_share(&argv(relative), share), Some(format!("exec: cmd.exe {rest}")), "{relative:?}");
            }
        }
        let absolute: [&[&str]; 5] = [
            &["cmd.exe", "/C", "pushd \"\\\\wsl.localhost\\Ubuntu\\p\" && npm run build"],
            &["cmd.exe", "/k", " cd /d C:\\p && dir"],
            &["cmd.exe", "/c", "CD /D \"D:/work p\" && dir"],
            &["cmd.exe", "/c", "chdir /d C:/p&&dir"],
            &["cmd.exe", "/c", "pushd", "\\\\server\\share"],
        ];
        for moves in absolute {
            assert_eq!(cmd_in_share(&argv(moves), share), None, "{moves:?}");
        }
        assert_eq!(cmd_in_share(&argv(&[r"C:\Program Files\nodejs\node.exe", "x.js"]), share), None);
        assert_eq!(cmd_in_share(&cmd, std::path::Path::new(r"C:\Users\ann\p")), None);
        assert_eq!(cmd_in_share(&npm, std::path::Path::new(r"\\?\C:\Users\ann\p")), None);
    }

    #[test]
    fn input_from_a_console_names_the_shell_and_a_program() {
        // The second review of round 20 (INTERNALS 5.10): a program run in a console, with v2's
        // standard error left there, sends its ANSI code page whatever chcp says, so the shell's
        // advice gains the program's (tests/host/builtins/input_bad_program.v2 runs the other).
        assert_eq!(
            piped_input_hint(true),
            "in cmd.exe, run chcp 65001 first; for Windows PowerShell 5.1, README.md's Troubleshooting has the line to run; a program that sends v2 its input must send UTF-8 (in Python, encoding=\"utf-8\")"
        );
        assert_eq!(piped_input_hint(false), "the program that sends v2 its input must send UTF-8 (in Python, encoding=\"utf-8\")");
    }

    #[test]
    fn a_unix_temp_folder_is_refused_where_the_drive_has_none() {
        // Round 19 review (INTERNALS 5.10): the /tmp decision, told what the drive has, so no test
        // makes or needs \tmp at the top of a real drive; /var/tmp names \var\tmp, not \tmp.
        let none = |_: &str| false;
        assert_eq!(unix_tmp("/tmp/work", &none), Some(("/tmp", "/work")));
        assert_eq!(unix_tmp("/tmp", &none), Some(("/tmp", "")));
        assert_eq!(unix_tmp("/var/tmp/x", &none), Some(("/var/tmp", "/x")));
        assert_eq!(unix_tmp("/tmpfiles/x", &none), None);
        assert_eq!(unix_tmp("tmp/x", &none), None);
        // A folder the drive has is the user's own: nothing to say.
        assert_eq!(unix_tmp("/tmp/work", &|_: &str| true), None);
        assert_eq!(unix_tmp("/var/tmp/x", &|t: &str| t == "/tmp"), Some(("/var/tmp", "/x")));
        assert_eq!(unix_tmp("/tmp/x", &|t: &str| t == "/var/tmp"), Some(("/tmp", "/x")));
        assert!(tmp_refusal("/tmp/work", &|_: &str| true).is_none());
        let refused = |path: &str| tmp_refusal(path, &none).map(|e| (e.message.to_string(), e.hint.get().map(|h| h.to_string())));
        let temp = |tmp: &str, example: &str| Some(format!("{tmp} is Linux's and macOS's temp folder: on Windows use getenv(\"TEMP\"), as in {example}"));
        let makes = |path: &str, folder: &str| format!("cannot create '{path}': on Windows that makes {folder} at the top of the drive, a folder other users can write to");
        assert_eq!(refused("/tmp/work"), Some((makes("/tmp/work", r"\tmp"), temp("/tmp", "getenv(\"TEMP\") + \"/work\""))));
        assert_eq!(refused("/var/tmp/x"), Some((makes("/var/tmp/x", r"\var\tmp"), temp("/var/tmp", "getenv(\"TEMP\") + \"/x\""))));
        assert_eq!(refused("/tmp"), Some((makes("/tmp", r"\tmp"), temp("/tmp", "getenv(\"TEMP\")"))));
    }

    #[test]
    fn wsls_and_linuxs_paths_get_their_windows_way() {
        // INTERNALS 5.10 (round 21): /mnt/c/… is WSL's spelling of C:/…, as /c/… is Git Bash's;
        // a Linux path whose first folder this drive doesn't have is WSL's; ls not on PATH runs
        // in WSL. Each only where its folder isn't on the current drive (`\mnt`, `\root`).
        let cwd = std::env::current_dir().unwrap().to_string_lossy().into_owned();
        let letter = cwd[..1].to_ascii_lowercase();
        let here = |p: &str| std::path::Path::new(p).exists();
        if cfg!(windows) && !here("/mnt") && !here(&format!("/{letter}")) {
            let upper = letter.to_ascii_uppercase();
            assert_eq!(unix_drive(&format!("/mnt/{letter}/Users/me/x.txt")), Some((&*format!("/mnt/{letter}"), format!("{upper}:/Users/me/x.txt"), "WSL's")));
            assert_eq!(unix_drive(&format!("/mnt/{letter}")), Some((&*format!("/mnt/{letter}"), format!("{upper}:/"), "WSL's")));
            assert_eq!(unix_drive(&format!("/{letter}/x")), Some((&*format!("/{letter}"), format!("{upper}:/x"), "Git Bash's")));
            let hint = msys_hint(&format!("/mnt/{upper}/x/y.txt"));
            assert_eq!(hint.as_deref(), Some(&*format!("on Windows, /mnt/{letter}/… is WSL's spelling of {upper}:/…: write \"{upper}:/x/y.txt\"")));
        }
        for not in ["/mnt", "/mnt/", "/mnt/cc/x", "/mnty/c/x", "mnt/c/x", "/mnt//c"] {
            assert_eq!(unix_drive(not), None, "{not}");
        }
        if cfg!(windows) && !here("/root") {
            assert_eq!(linux_path_hint("/root/proj/main.v2").as_deref(), Some(r"a Linux path: from Windows, WSL's files are \\wsl.localhost\<distro>\…"));
        }
        for not in ["/rooty/x", "root/x", "/proc/self", "/dev/stdin", "C:/root/x"] {
            assert_eq!(linux_path_hint(not), None, "{not}");
        }
        let argv = [Value::str("ls"), Value::str("-la"), Value::str("my dir")];
        let want = cfg!(windows).then(|| r#"run Linux programs with exec(["wsl.exe", "-e", "ls", "-la", "my dir"])"#.to_string());
        assert_eq!(linux_program_hint("ls", &argv), want);
        assert_eq!(linux_program_hint("dir", &argv), None);
    }

    #[cfg(windows)]
    #[test]
    fn names_with_a_dot_or_space_at_the_end_are_read_as_written() {
        // INTERNALS 5.10, 6.4, 19.1 (round 21): an import and FILE take the file builtins' reason.
        let (why, there) = dotted_name("lib./util.v2").expect("a dotted folder");
        assert_eq!((why.as_str(), there), ("a Windows file name can't end in a dot or a space ('lib.' would be 'lib')", None));
        assert_eq!(dotted_name("run.v2 ").map(|d| d.0).as_deref(), Some("a Windows file name can't end in a dot or a space ('run.v2 ' would be 'run.v2')"));
        for fine in ["./run.v2", "../x.v2", r"\\?\C:\x\run.v2.", r"\\.\C:\x.", "C:x.v2"] {
            assert!(dotted_name(fine).is_none(), "{fine}");
        }
        // The review: such a script's script_dir() is a \\?\ path, where Windows reads no / or ..
        assert_eq!(verbatim_parts(r"\\?\C:\x\dots./data.csv"), r"\\?\C:\x\dots.\data.csv");
        assert_eq!(verbatim_parts(r"\\?\C:\x\dots.\./..\top.txt"), r"\\?\C:\x\top.txt");
        assert_eq!(verbatim_parts(r"\\?\UNC\srv\share\a/../b/"), r"\\?\UNC\srv\share\b");
        assert_eq!(verbatim_parts(r"\\?\C:\x\..\.."), r"\\?\C:\");
        for kept in [r"\\?\C:\x\notes.txt ", "C:/x/../y", r"\\.\C:/x", "./a/../b"] {
            assert!(matches!(verbatim_parts(kept), std::borrow::Cow::Borrowed(k) if k == kept), "{kept}");
        }
        // The second review: exec's arguments that are such paths too, but not cmd's command line
        // after /c, nor another argument.
        let args = |raw_tail: bool| {
            let mut argv: Vec<OsString> = ["cmd.exe", r"\\?\C:\x\dots./a.py", "./b/../c", "/c", r"\\?\C:/x & echo y"].map(OsString::from).to_vec();
            verbatim_arguments(&mut argv, raw_tail);
            argv.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>()
        };
        assert_eq!(args(true), ["cmd.exe", r"\\?\C:\x\dots.\a.py", "./b/../c", "/c", r"\\?\C:/x & echo y"]);
        assert_eq!(args(false)[4], r"\\?\C:\x & echo y");
    }

    #[test]
    fn only_create_dir_alls_own_error_is_taken_for_the_roots() {
        // Round 19 review (INTERNALS 5.10): Rust's error for a NUL in the path has no OS code
        // either, and is the true reason; create_dir_all's for a root it can't make is replaced.
        let nul = std::fs::create_dir_all("a\0b").unwrap_err();
        assert!(nul.raw_os_error().is_none() && !whole_tree_error(&nul), "{nul}");
        // Windows: a drive that isn't there (every letter is a local drive elsewhere). Rust 1.91's
        // create_dir_all gives its own error there, newer ones (CI's stable) the OS's: either way
        // what make_dir says is the drive's (unreachable_root), and nothing is left without a reason.
        if let Some(letter) = ('D'..='Z').rev().find(|&l| drive_type(l) == NO_DRIVE) {
            let path = format!(r"{letter}:\v2-r19-no-drive\x");
            let e = std::fs::create_dir_all(&path).unwrap_err();
            assert!(whole_tree_error(&e) || e.raw_os_error().is_some(), "{e}");
            let root = std::path::Path::new(&path).ancestors().last().map(std::fs::metadata);
            assert!(matches!(root, Some(Err(ref r)) if r.raw_os_error().is_some()), "{root:?}");
            assert_eq!(unreachable_root(&path, &e).as_deref(), Some(&*format!("there is no drive {letter}: (is it plugged in or connected?)")));
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_mapped_drive_that_cant_be_reached_is_named() {
        // Round 19 review (INTERNALS 5.10): a mapped network drive whose computer isn't reached
        // is a letter GetDriveTypeW knows, so it got "there is no folder 'Z:\reports'" and a
        // make_dir hint. A network error on a drive that is there names the drive, with no hint.
        // Only a network drive (DRIVE_REMOTE) is named so (the second review: a network error on
        // a local drive comes from a link to a share on the path, not from the drive).
        for code in [53, 55, 64, 67, 1201, 1231] {
            let e = std::io::Error::from_raw_os_error(code);
            assert_eq!(drive_problem('Z', REMOTE_DRIVE, &e).as_deref(), Some("the drive Z: can't be reached (is it connected?)"), "{code}");
            assert_eq!(drive_problem('C', 3, &e), None, "a local drive, {code}");
        }
        assert_eq!(drive_problem('Z', REMOTE_DRIVE, &std::io::Error::from_raw_os_error(3)), None, "a folder that isn't there");
        assert_eq!(drive_problem('Q', NO_DRIVE, &std::io::Error::from_raw_os_error(3)).as_deref(), Some("there is no drive Q: (is it plugged in or connected?)"));
        // Each way of writing a drive's path is read as that drive, \\?\ and a small letter too
        // (the third review): a letter that isn't there, and the message has no hint.
        if let Some(letter) = ('D'..='Z').rev().find(|&l| drive_type(l) == NO_DRIVE) {
            let none = format!("there is no drive {letter}: (is it plugged in or connected?)");
            let small = letter.to_ascii_lowercase();
            for path in [format!(r"{letter}:\v2-r19\out.csv"), format!(r"\\?\{letter}:\v2-r19\out.csv"), format!("{small}:/v2-r19/out.csv"), format!("{small}:out.csv")] {
                let e = std::io::Error::from_raw_os_error(3);
                assert_eq!(unreachable_root(&path, &e).as_deref(), Some(none.as_str()), "{path}");
                let Err(Flow::Error(err)) = file_error::<()>("write", &path, &e) else { panic!("{path}") };
                assert_eq!(&*err.message, format!("cannot write '{}': {none}", shown_path(&path)));
                assert!(err.hint.get().is_none(), "{:?}", err.hint.get());
            }
        }
        // The current drive is a local one: a network error on a path there isn't the drive's.
        let cwd = std::env::current_dir().unwrap().to_string_lossy().into_owned();
        assert_eq!(cwd.as_bytes().get(1), Some(&b':'), "{cwd}");
        let letter = cwd.chars().next().unwrap().to_ascii_uppercase();
        if drive_type(letter) != REMOTE_DRIVE {
            for path in [format!(r"{letter}:\v2-r19-no-such\out.csv"), format!(r"\\?\{letter}:\v2-r19-no-such\out.csv")] {
                assert_eq!(unreachable_root(&path, &std::io::Error::from_raw_os_error(1231)), None, "{path}");
            }
        }
    }

    #[test]
    fn a_wsl_distribution_that_isnt_there_is_named() {
        // Round 21 (INTERNALS 5.10): Windows says "The user name or password is incorrect"
        // (1326) for \\wsl.localhost\NAME and "The network name cannot be found" (67) for
        // \\wsl$\NAME when there is no such distribution; a bare \\wsl.localhost names none.
        let no = "there is no WSL distribution named no-such: wsl --list --quiet shows them";
        let err = std::io::Error::from_raw_os_error;
        assert_eq!(wsl_share(r"wsl.localhost\no-such\x.txt", &err(1326)).as_deref(), Some(no));
        assert_eq!(wsl_share(r"WSL$\no-such", &err(67)).as_deref(), Some(no));
        // Under load \\wsl$ says ERROR_NETNAME_DELETED instead (the review of round 21).
        assert_eq!(wsl_share(r"wsl$\no-such\x.txt", &err(64)).as_deref(), Some(no));
        let bare = r"\\wsl.localhost needs a WSL distribution's name after it, as in \\wsl.localhost\Ubuntu: wsl --list --quiet shows them";
        assert_eq!(wsl_share("wsl.localhost", &err(161)).as_deref(), Some(bare));
        assert_eq!(wsl_share(r"wsl.localhost\", &err(1326)).as_deref(), Some(bare));
        // Another failure in a distribution that is there, and another server, are not this.
        assert_eq!(wsl_share(r"wsl.localhost\Ubuntu\x.txt", &err(3)), None);
        assert_eq!(wsl_share(r"srv\docs\x.txt", &err(1326)), None);
        // read_file looks for WSL's pipes only on a share or a network drive (a look costs a
        // local read a quarter more): by the path, or a relative one by the current folder.
        assert_eq!(on_a_share(r"\\wsl.localhost\Ubuntu\fifo"), cfg!(windows));
        assert_eq!(on_a_share("//srv/docs/x.txt"), cfg!(windows));
        let cwd = std::env::current_dir().unwrap().to_string_lossy().into_owned();
        if cwd.as_bytes().get(1) == Some(&b':') && drive_type(cwd.chars().next().unwrap().to_ascii_uppercase()) != REMOTE_DRIVE {
            assert!(!on_a_share(&format!(r"{}\x.txt", &cwd[..2])) && !on_a_share("x.txt") && !on_a_share(r"\x.txt"));
        }
        if cfg!(windows) {
            assert_eq!(unreachable_root("//wsl.localhost/no-such/x.txt", &err(1326)).as_deref(), Some(no));
            assert_eq!(unreachable_root(r"\\?\UNC\wsl$\no-such\x.txt", &err(67)).as_deref(), Some(no));
            assert_eq!(unreachable_root("//WSL$/", &err(67)).as_deref(), Some(r"\\WSL$ needs a WSL distribution's name after it, as in \\WSL$\Ubuntu: wsl --list --quiet shows them"));
            // The whole path, as an import and FILE give it (review of round 21).
            assert_eq!(wsl_root("//wsl.localhost/no-such/m.v2", &err(1326)).as_deref(), Some(no));
            assert_eq!(wsl_root(r"\\?\UNC\wsl.localhost\no-such\m.v2", &err(1326)).as_deref(), Some(no));
            assert_eq!(wsl_root(r"\\wsl.localhost", &err(161)).as_deref(), Some(bare));
            assert_eq!(wsl_root(r"C:\no-such\m.v2", &err(1326)), None);
        } else {
            assert_eq!(wsl_root("//wsl.localhost/no-such/m.v2", &err(1326)), None, "a Linux path");
        }
    }
}
