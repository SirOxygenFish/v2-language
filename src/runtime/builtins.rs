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
    BuiltinDef { name: "input", params: &[ParamSpec::opt("prompt", || Value::str(""))], f: input },
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

    /// Flush, and raise a failure as 5.10's `IOError` (`input()`'s prompt).
    pub fn flush_or_raise(&mut self) -> Exec<()> {
        let r = self.flush();
        self.raise(r)
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
    super::module::link_error(verb, path).map_or(Ok(()), |err| Err(Flow::Error(Rc::new(err))))?;
    if let Some(why) = unreachable_root(path, e) {
        return root_error(verb, path, why);
    }
    let hint = hint.or_else(|| tmp_hint(path)).or_else(|| read_only_hint(verb, path, e));
    // A file where a folder goes, or the other way round: Windows says "Access is denied", "The
    // directory name is invalid" or "Cannot create a file when that file already exists".
    let p = std::path::Path::new(path);
    // A file where the path goes on (`notes.txt/sub`), or a folder that isn't there: the OS's
    // words for them ("The system cannot find the path specified") don't say which part.
    let in_the_way = p.ancestors().skip(1).find(|a| !a.as_os_str().is_empty() && a.exists()).filter(|a| a.is_file());
    let no_folder = p.parent().filter(|d| !d.as_os_str().is_empty() && !as_folder(d).exists() && e.kind() == std::io::ErrorKind::NotFound);
    let why = match verb {
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
    } else if cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33)) {
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION: the OS says only that a process uses it.
        let _ = err.hint.set("another program has it open (Excel keeps an open file locked): close it there, or use another file name".into());
    } else if verb == "remove" && p.is_dir() {
        let _ = err.hint.set("a @py block removes a folder with what is in it: @py(path) { import shutil; shutil.rmtree(path) }".into());
    } else if cfg!(windows) && matches!(path, "/dev/stdin" | "/dev/stdout" | "/dev/stderr") {
        // Linux's and macOS's names for the standard streams.
        let _ = err.hint.set("/dev/stdin, /dev/stdout and /dev/stderr are Linux's and macOS's: on Windows read standard input with input(), a line at a time, and write with print() and print(…, stderr: true)".into());
    } else if let Some(h) = msys_hint(path).filter(|_| e.kind() == std::io::ErrorKind::NotFound) {
        let _ = err.hint.set(h.into());
    } else if let Some(h) = unicode_twin_hint(path, e) {
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
/// from a Mac, its `ü` a `u` plus an accent): `module::unicode_twin`'s hint (5.10).
fn unicode_twin_hint(path: &str, e: &std::io::Error) -> Option<String> {
    let p = std::path::Path::new(path);
    let name = p.file_name()?.to_str().filter(|_| e.kind() == std::io::ErrorKind::NotFound)?;
    let folder = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    super::module::unicode_twin(folder, name).map(|h| shown_path(&h))
}

/// Windows: `/c/Users/me/x.txt`, Git Bash's spelling of `C:/Users/me/x.txt` (one letter after
/// the first `/`, a drive that exists, and no such folder on this drive: `\c`): what to write
/// instead (5.10).
fn msys_hint(path: &str) -> Option<String> {
    let b = path.as_bytes();
    if !cfg!(windows) || b.first() != Some(&b'/') || !b.get(1).is_some_and(u8::is_ascii_alphabetic) || !matches!(b.get(2), None | Some(b'/')) {
        return None;
    }
    let drive = format!("{}:", (b[1] as char).to_ascii_uppercase());
    if !std::path::Path::new(&format!("{drive}/")).is_dir() || std::path::Path::new(&path[..2]).exists() {
        return None;
    }
    let mut write = String::new();
    super::value::quoted(&format!("{drive}{}", if path.len() > 2 { &path[2..] } else { "/" }), &mut write);
    Some(format!("on Windows, /{}/… is Git Bash's spelling of {drive}/…: write {write}", (b[1] as char).to_ascii_lowercase()))
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
    let after_drive = if path.as_bytes().get(1) == Some(&b':') { &path[2..] } else { path };
    let trailing = after_drive.split(['/', '\\']).find(|part| !matches!(*part, "." | "..") && (part.ends_with('.') || part.ends_with(' ')));
    let why = match (trailing, device_name(path)) {
        (Some(part), _) => {
            let kept = part.trim_end_matches(['.', ' ']);
            let would = if kept.is_empty() { String::new() } else { format!(" ('{}' would be '{}')", shown_path(part), shown_path(kept)) };
            format!("a Windows file name can't end in a dot or a space{would}")
        }
        (None, Some(device)) => format!("{} is the name of a device on Windows", shown_path(&device)),
        (None, None) => return Ok(()),
    };
    let err = ErrorValue::new("IOError", format!("cannot {verb} '{}': {why}", shown_path(path)));
    let builtin = match verb {
        "read" => "read_file",
        "write" => "write_file",
        "check" => "exists",
        "list" => "list_dir",
        "create" => "make_dir",
        _ => "remove_file",
    };
    // What must be there for `\\?\` to reach it: the path through its last part that ends in a dot
    // or a space (the folder `dir.` of a write to `dir./new.txt`), else the device's whole path.
    let through = if trailing.is_some() { through_last_dotted(path) } else { path };
    let hint = match verbatim(path).filter(|_| verbatim(through).is_some_and(|t| std::fs::symlink_metadata(t).is_ok())) {
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
        if cwd.starts_with(r"\\") {
            return None;
        }
        match given.strip_prefix('\\') {
            // `\x`: at the top of the current drive.
            Some(rest) => format!(r"{}\{rest}", cwd.get(..2)?),
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
    let mut parts = unc.split('\\');
    let (server, share) = (parts.next().filter(|s| !s.is_empty())?, parts.next().filter(|s| !s.is_empty())?);
    network_error(e).then(|| format!(r"the share \\{}\{} can't be reached (is that computer on and connected, and is the share's name right?)", shown_path(server), shown_path(share)))
}

/// Windows: an error of the network, not of the path (5.10): ERROR_BAD_NETPATH (53),
/// ERROR_BAD_NET_NAME (67), ERROR_DEV_NOT_EXIST (55), ERROR_CONNECTION_UNAVAIL (1201, a remembered
/// connection that isn't made) and the network's other failures.
fn network_error(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(51 | 53 | 55 | 59 | 64 | 67 | 1201 | 1203 | 1222 | 1231 | 1232))
}

/// `unreachable_root`'s IOError: no hint of a folder to make, but the escapes' one when the
/// path holds a control character (`"Q:\temp"`).
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
        // `notes 12:30.txt` on Windows: a `:` after the drive names a hidden stream of the file
        // before it, so the write would seem to work and leave that file empty.
        let after_drive = if path.as_bytes().get(1) == Some(&b':') { &path[2..] } else { path };
        if cfg!(windows) && !path.starts_with(r"\\") && after_drive.contains(':') {
            let err = ErrorValue::new("IOError", format!("cannot {verb} '{}': a Windows file name can't have ':' in it", shown_path(path)));
            let _ = err.hint.set("use another character, as in 12-30 for a time".into());
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

/// How many times in a row `input()` may give null before a loop that never checks is stopped.
const INPUT_NULLS: u32 = 1000;

/// The nulls `input()` gave since it last read a line.
static NULLS_IN_A_ROW: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// `input(prompt = "")`: one line from stdin without `\n` and one `\r` before it; `null` at end
/// of input; the prompt is flushed first (5.10). A menu loop that never checks for null would
/// spin forever once the input ends: the 1000th null in a row is an IOError instead.
fn input(it: &mut Interp, args: Vec<Value>, at: Span) -> Exec<Value> {
    use std::sync::atomic::Ordering;
    let prompt = it.str_of(&args[0], at)?;
    it.out.write_str(&prompt)?;
    it.out.flush_or_raise()?;
    let mut line = Vec::new();
    match std::io::stdin().lock().read_until(b'\n', &mut line) {
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
            let err = ErrorValue::new("IOError", format!("input line {n} is not valid UTF-8: {}: send UTF-8 text", not_utf8_kind(e.as_bytes(), n == 1)));
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
        // `١٢٣`, `１２`: digits of another script, which Python reads; with U+066B, a point there,
        // and without U+066C (`١٬٢٣٤`, ar-EG's thousands), which Python refuses.
        let g = if t.contains('\u{66b}') { "float" } else { f };
        let replaces = arabic_marks(t).1;
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
    let binary = bool_arg(&args[1], "read_file", "binary")?;
    let bytes = match read_path(&path) {
        Ok(Some(b)) => b,
        // More than a str or bytes holds: a huge file, or a device that never ends (/dev/zero).
        Ok(None) => {
            return io_error(format!("cannot read '{}': it holds more than 1 GiB, the most V2 reads at once: read it in pieces in a block", shown_path(&path)));
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
        Err(_) => {
            let kind = not_utf8_kind(&bytes, true);
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
    let r = if append {
        std::fs::OpenOptions::new().append(true).create(true).open(&*path).and_then(|mut f| f.write_all(bytes))
    } else {
        std::fs::write(&*path, bytes)
    };
    match r {
        Ok(()) => Ok(Value::Null),
        Err(e) => file_error("write", &path, &e),
    }
}

/// `exists(path)`: whether `fs::metadata` succeeds, symlinks followed; never an error (5.10).
fn exists(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "exists", "path")?;
    if path.is_empty() {
        return Ok(Value::Bool(false));
    }
    // `exists("~/.config/x")` would be false for a file that is there: refused as in the others.
    shell_shorthand("check", &path)?;
    Ok(Value::Bool(std::fs::metadata(&*path).is_ok()))
}

/// `list_dir(path)`: the entry names, sorted by code point (5.10).
fn list_dir(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "list_dir", "path")?;
    shell_shorthand("list", &path)?;
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
    // Windows: `/c/Users/me/out`, Git Bash's spelling, would make `\c\Users\me\out` on this
    // drive, where nothing else finds it: refused as the missing folder it is (5.10).
    if let Some(h) = msys_hint(&path) {
        let err = ErrorValue::new("IOError", format!("cannot create '{}': there is no folder '{}'", shown_path(&path), &path[..2]));
        let _ = err.hint.set(h.into());
        return Err(Flow::Error(Rc::new(err)));
    }
    // Windows: `/tmp/out` would make `\tmp` at the top of the drive, where every user can write.
    let refused = if cfg!(windows) { tmp_refusal(&path, &|tmp| std::path::Path::new(tmp).is_dir()) } else { None };
    if let Some(err) = refused {
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

/// `remove_file(path)`: a file, never a folder (5.10).
fn remove_file(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "remove_file", "path")?;
    shell_shorthand("remove", &path)?;
    match std::fs::remove_file(&*path) {
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
    let input = match &args[1] {
        Value::Null => None,
        Value::Str(s) => Some(s.as_bytes().to_vec()),
        Value::Bytes(b) => Some(b.to_vec()),
        other => return type_error(format!("exec() argument 'input' must be a str, bytes or null, got {}", other.type_name())),
    };
    let lookup = Lookup::from_env();
    argv[0] = match first_program(name, &lookup) {
        Some(p) => p.into_os_string(),
        None if is_drive_relative(std::path::Path::new(&**name)) => {
            return io_error(format!("exec: cannot start '{name}': a drive-relative path: write it relative to the working directory, or absolute"));
        }
        None if name.contains(['/', '\\']) => {
            let p: std::path::PathBuf = lookup.cwd.join(&**name).components().collect();
            if p.is_dir() {
                return io_error(format!("exec: cannot start '{name}': it is a folder, not a program"));
            }
            // Not there: a batch file's path would go to cmd.exe, which says so as its output.
            if !p.exists() {
                let err = ErrorValue::new("IOError", format!("exec: cannot start '{name}': there is no such file"));
                if let Some(h) = msys_hint(name) {
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
            let python3 = (&**name == "python" && !unset).then(|| first_program("python3", &lookup)).flatten();
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
            }
            return Err(Flow::Error(Rc::new(err)));
        }
    };
    it.out.flush_or_stop();
    // A Python program writes a pipe in the locale's encoding (ANSI on Windows: café as cafÚ in
    // the console's code page, and CJK not at all): UTF-8 instead, unless the user chose (5.10).
    let utf8 = std::env::var_os("PYTHONIOENCODING").is_none().then(|| (OsString::from("PYTHONIOENCODING"), OsString::from("utf-8")));
    // `exec(["cmd", "/c", "cd /d \"my dir\" && dir"])`: the one string after /c is a command line
    // for cmd.exe, which reads it as typed; quoted for a program (`\"`), cmd would misread it.
    let is_cmd = std::path::Path::new(&argv[0]).file_name().is_some_and(|f| f.eq_ignore_ascii_case("cmd.exe"));
    let raw_tail = cfg!(windows) && is_cmd && argv.len() >= 3 && argv[argv.len() - 2].to_str().is_some_and(|s| s.eq_ignore_ascii_case("/c") || s.eq_ignore_ascii_case("/k"));
    // On Unix the program sees the name as written (`ls`), as from a shell (7.5).
    let ran = process::run_captured_as(&argv, Some(std::ffi::OsStr::new(&**name)), utf8.as_slice(), None, input, raw_tail);
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
    stop_if_interrupted();
    let mut err = std::io::stderr().lock();
    match err.write_all(bytes).and_then(|_| err.flush()) {
        Ok(()) => {
            wrote_stderr(bytes);
            Ok(Value::Null)
        }
        Err(e) => io_error(format!("cannot write to stderr: {}", os_message(&e))),
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
}
