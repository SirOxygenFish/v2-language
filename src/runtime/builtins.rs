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
use super::ops::{binop, is_handle, order, HANDLE_HINT};
use super::value::{float_str, DictMap, Str, Value};
use crate::codec::json;
use crate::engines::process::{self, stop_if_interrupted};
use crate::engines::resolve::{first_program, is_drive_relative, Lookup};
use crate::syntax::ast::BinOp;
use crate::syntax::span::{os_message, Span};

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
    buf.drain(..done);
    r
}

/// The interrupt path (6.5): write what the program printed straight to the descriptor (the
/// interpreter thread holds `stdout`'s lock), from a thread of its own, so a reader that stopped
/// reading can't hold the exit longer than `wait`.
pub fn write_pending(wait: Duration) {
    let (done, finished) = std::sync::mpsc::channel();
    let writer = std::thread::Builder::new().spawn(move || {
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
/// terminal, and at the end of the run (5.10).
pub struct Out {
    w: Box<dyn Write>,
    tty: bool,
    pending: &'static Mutex<Vec<u8>>,
    /// Output that couldn't be written and whose `IOError` never reached the program (a block's
    /// own error won): the end of the run reports it (6.5).
    pub lost: Option<Rc<str>>,
}

impl Out {
    pub fn new() -> Out {
        let stdout = std::io::stdout();
        let tty = stdout.is_terminal();
        let lock: StdoutLock<'static> = stdout.lock();
        Out { w: Box::new(lock), tty, pending: &PENDING, lost: None }
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
        (Out { w: Box::new(Shared(buf.clone())), tty: true, pending, lost: None }, buf)
    }

    pub fn write_str(&mut self, s: &str) -> Exec<()> {
        let mut pending = lock(self.pending);
        pending.extend_from_slice(s.as_bytes());
        if !self.tty && pending.len() < OUT_BUFFER {
            return Ok(());
        }
        let r = drain(&mut *self.w, &mut pending);
        drop(pending);
        self.raise(r)
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        drain(&mut *self.w, &mut lock(self.pending))
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

/// A path as messages show it: control characters escaped, so the line break that `"C:\new"`
/// holds can't split the message (5.10).
fn shown_path(path: &str) -> String {
    let mut out = String::new();
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

/// A file builtin's OS failure (5.10): `cannot read 'x': <why>`, a folder said as such, and for
/// a path holding a control character the hint that `\n` and `\t` in `"…"` are escapes.
fn file_error<T>(verb: &str, path: &str, e: &std::io::Error) -> Exec<T> {
    file_error_hint(verb, path, e, None)
}

/// `file_error`, with a hint of the caller's when the path itself needs none.
fn file_error_hint<T>(verb: &str, path: &str, e: &std::io::Error, hint: Option<String>) -> Exec<T> {
    let why = if verb == "read" && std::path::Path::new(path).is_dir() { "it is a folder".to_string() } else { os_message(e) };
    let err = ErrorValue::new("IOError", format!("cannot {verb} '{}': {why}", shown_path(path)));
    if path.chars().any(char::is_control) {
        let _ = err
            .hint
            .set(r#"in "…", a backslash starts an escape (\n is a line break, \t a tab): write Windows paths with / or \\, as in "notes/new.txt""#.into());
    } else if let Some(h) = hint {
        let _ = err.hint.set(h.into());
    }
    Err(Flow::Error(Rc::new(err)))
}

/// What text that isn't UTF-8 probably is (5.10).
fn not_utf8_kind(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        "it is UTF-16 (what Windows PowerShell's > and Out-File write)"
    } else if bytes.iter().take(8192).any(|&b| b == 0) {
        "it looks like binary data"
    } else {
        "it may be in an older encoding, such as Windows-1252"
    }
}

/// `input(prompt = "")`: one line from stdin without `\n` and one `\r` before it; `null` at end
/// of input; the prompt is flushed first (5.10).
fn input(it: &mut Interp, args: Vec<Value>, at: Span) -> Exec<Value> {
    let prompt = it.str_of(&args[0], at)?;
    it.out.write_str(&prompt)?;
    it.out.flush_or_raise()?;
    let mut line = Vec::new();
    match std::io::stdin().lock().read_until(b'\n', &mut line) {
        Ok(0) => return Ok(Value::Null),
        Ok(_) => {}
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
    if n == 1 && line.starts_with(&[0xEF, 0xBB, 0xBF]) {
        line.drain(..3);
    }
    match String::from_utf8(line) {
        Ok(s) => Ok(Value::Str(s.into())),
        Err(e) => io_error(format!("input line {n} is not valid UTF-8: {}: send UTF-8 text, or read the file with read_file(path, binary: true)", not_utf8_kind(e.as_bytes()))),
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
                return value_error(format!("invalid int: '{s}'"));
            }
            match t.parse::<i64>() {
                Ok(n) => n,
                Err(_) => return value_error(format!("int out of range: '{s}'")),
            }
        }
        other => return type_error(format!("int() can't convert {}", other.type_name())),
    }))
}

/// `float(x)` (5.10): strings via `f64::from_str` after trimming (`nan`, `inf`, `infinity`).
fn float(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    Ok(Value::Float(match &args[0] {
        Value::Float(x) => *x,
        Value::Int(n) => *n as f64,
        Value::Bool(b) => *b as i64 as f64,
        Value::Str(s) => match s.trim().parse::<f64>() {
            Ok(x) => x,
            Err(_) => return value_error(format!("invalid float: '{s}'")),
        },
        other => return type_error(format!("float() can't convert {}", other.type_name())),
    }))
}

/// `range(end)` / `range(start, end, step = 1)`: a list of ints, sized before it is built. One
/// argument is the end; arguments are positional only (5.10).
fn range(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let Value::List(given) = &args[0] else { unreachable!("a rest parameter is a list") };
    let given = given.borrow();
    let (start, end, step) = match given.as_slice() {
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
    let binary = bool_arg(&args[1], "read_file", "binary")?;
    let bytes = match std::fs::read(&*path) {
        Ok(b) => b,
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
            let kind = not_utf8_kind(&bytes);
            let fix = if kind.ends_with("binary data") { "read it with binary: true" } else { "save it as UTF-8, or read it with binary: true" };
            io_error(format!("'{}' is not valid UTF-8: {kind}: {fix}", shown_path(&path)))
        }
    }
}

/// `write_file(path, data, append: false)`: a str as UTF-8 or bytes as is; creates or
/// truncates, or with `append` adds to the end, creating a missing file (5.10).
fn write_file(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "write_file", "path")?;
    let bytes: &[u8] = match &args[1] {
        Value::Str(s) => s.as_bytes(),
        Value::Bytes(b) => b,
        other => return type_error(format!("write_file() argument 'data' must be a str or bytes, got {}", other.type_name())),
    };
    let r = if bool_arg(&args[2], "write_file", "append")? {
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
    Ok(Value::Bool(std::fs::metadata(&*path).is_ok()))
}

/// `list_dir(path)`: the entry names, sorted by code point (5.10).
fn list_dir(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "list_dir", "path")?;
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
            Err(_) => return io_error(format!("cannot list '{path}': a name is not valid UTF-8")),
        }
    }
    names.sort();
    Ok(Value::list(names.into_iter().map(|n| Value::Str(n.into())).collect()))
}

/// `make_dir(path)`: with any missing parents; an existing folder is fine (5.10).
fn make_dir(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "make_dir", "path")?;
    match std::fs::create_dir_all(&*path) {
        Ok(()) => Ok(Value::Null),
        Err(e) => file_error("create", &path, &e),
    }
}

/// `remove_file(path)`: a file, never a folder (5.10).
fn remove_file(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "remove_file", "path")?;
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
        // A path that names nothing usable: starting it gives the OS's reason.
        None if name.contains(['/', '\\']) => lookup.cwd.join(&**name).into_os_string(),
        None => return io_error(format!("exec: cannot start '{name}': not found on PATH")),
    };
    let _ = it.out.flush();
    let c = match process::run_captured(&argv, &[], None, input) {
        Ok(c) => c,
        Err(e) => return io_error(format!("exec: cannot start '{name}': {}", e.reason)),
    };
    let code = match (c.status.code, c.status.signal) {
        (_, Some(signal)) => 128 + signal as i64,
        (Some(code), None) if cfg!(windows) => code as u32 as i64,
        (Some(code), None) => code as i64,
        (None, None) => -1,
    };
    let mut result = DictMap::new();
    result.insert(Rc::from("code"), Value::Int(code));
    result.insert(Rc::from("stdout"), Value::str(&String::from_utf8_lossy(&c.stdout)));
    result.insert(Rc::from("stderr"), Value::str(&String::from_utf8_lossy(&c.stderr)));
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
            let _ = it.out.flush();
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
        let hint = if is_handle(bad) { HANDLE_HINT } else { "" };
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
    if name.is_empty() || name.contains(['=', '\0']) {
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
    match i32::try_from(code) {
        Ok(c) => Err(Flow::Exit(c)),
        Err(_) => value_error(format!("exit code {code} is out of range")),
    }
}

/// A `str` argument, or `TypeError: print() argument 'sep' must be a str, got int`.
pub fn str_arg(v: &Value, func: &str, param: &str) -> Exec<Str> {
    match v {
        Value::Str(s) => Ok(s.clone()),
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
    let to_stderr = bool_arg(&args[3], "print", "stderr")?;
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
    if !to_stderr {
        it.out.write_str(&text)?;
        return Ok(Value::Null);
    }
    let _ = it.out.flush();
    stop_if_interrupted();
    let mut err = std::io::stderr().lock();
    match err.write_all(text.as_bytes()).and_then(|_| err.flush()) {
        Ok(()) => Ok(Value::Null),
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
