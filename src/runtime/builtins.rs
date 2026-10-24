//! Builtins and the `Out` writer (INTERNALS 5.10): DOCS 3.11 without `to_v2`, which needs
//! handles (roadmap 1.6), and `define_engine` (roadmap Phase 3).

use std::cmp::Ordering;
use std::ffi::OsString;
use std::io::{BufRead, BufWriter, IsTerminal, StdoutLock, Write};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;

use super::call::ParamSpec;
use super::error::{fail, type_error, value_error, ErrorValue, Exec, Flow};
use super::interp::Interp;
use super::methods::int_arg;
use super::ops::{binop, order};
use super::value::{float_str, Value};
use crate::codec::json;
use crate::engines::process::{self, stop_if_interrupted};
use crate::engines::resolve::{find_program, is_drive_relative, Lookup};
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
        params: &[ParamSpec::req("type"), ParamSpec::req("message"), ParamSpec::opt("data", || Value::Null)],
        f: error,
    },
    BuiltinDef { name: "input", params: &[ParamSpec::opt("prompt", || Value::str(""))], f: input },
    BuiltinDef { name: "len", params: &[ParamSpec::req("x")], f: len },
    BuiltinDef { name: "type", params: &[ParamSpec::req("x")], f: |_, a, _| Ok(Value::str(a[0].type_name())) },
    BuiltinDef { name: "str", params: &[ParamSpec::req("x")], f: |it, a, _| Ok(Value::Str(it.str_of(&a[0])?.into())) },
    BuiltinDef { name: "int", params: &[ParamSpec::req("x")], f: int },
    BuiltinDef { name: "float", params: &[ParamSpec::req("x")], f: float },
    BuiltinDef { name: "bool", params: &[ParamSpec::req("x")], f: |_, a, _| Ok(Value::Bool(a[0].truthy())) },
    BuiltinDef {
        name: "range",
        params: &[
            ParamSpec::opt("start", || Value::Null),
            ParamSpec::opt("end", || Value::Null),
            ParamSpec::opt("step", || Value::Int(1)),
        ],
        f: range,
    },
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
    BuiltinDef { name: "engine_available", params: &[ParamSpec::req("tag")], f: engine_available },
];

/// The only writer of the host's stdout: buffered, flushed after every write when stdout is a
/// terminal, and at the end of the run (5.10).
pub struct Out {
    w: BufWriter<Box<dyn Write>>,
    tty: bool,
}

impl Out {
    pub fn new() -> Out {
        let stdout = std::io::stdout();
        let tty = stdout.is_terminal();
        let lock: StdoutLock<'static> = stdout.lock();
        Out { w: BufWriter::with_capacity(64 * 1024, Box::new(lock)), tty }
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
        (Out { w: BufWriter::new(Box::new(Shared(buf.clone()))), tty: true }, buf)
    }

    pub fn write_str(&mut self, s: &str) -> Exec<()> {
        let r = self.w.write_all(s.as_bytes()).and_then(|_| if self.tty { self.w.flush() } else { Ok(()) });
        match r {
            Ok(()) => Ok(()),
            Err(e) => fail("IOError", format!("cannot write to stdout: {}", os_message(&e))),
        }
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.w.flush()
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

/// `input(prompt = "")`: one line from stdin without `\n` and one `\r` before it; `null` at end
/// of input; the prompt is flushed first (5.10).
fn input(it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let prompt = it.str_of(&args[0])?;
    it.out.write_str(&prompt)?;
    if let Err(e) = it.out.flush() {
        return io_error(format!("cannot write to stdout: {}", os_message(&e)));
    }
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
    match String::from_utf8(line) {
        Ok(s) => Ok(Value::Str(s.into())),
        Err(_) => io_error("input is not valid UTF-8".into()),
    }
}

/// `len(x)`: chars, bytes, items or entries (5.10).
fn len(it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    Ok(Value::Int(match &args[0] {
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
    let (start, end) = match (&args[0], &args[1]) {
        (Value::Null, Value::Null) => return fail("ArityError", "range() missing required argument 'end'"),
        (Value::Null, end) | (end, Value::Null) => (0, int_arg(end, "range", "end")?),
        (start, end) => (int_arg(start, "range", "start")?, int_arg(end, "range", "end")?),
    };
    let step = int_arg(&args[2], "range", "step")?;
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

/// `read_file(path, binary: false)`: relative to the working directory; text is strict UTF-8,
/// BOM kept, no newline translation (5.10).
fn read_file(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "read_file", "path")?;
    let binary = bool_arg(&args[1], "read_file", "binary")?;
    let bytes = match std::fs::read(&*path) {
        Ok(b) => b,
        Err(e) => return io_error(format!("cannot read '{path}': {}", os_message(&e))),
    };
    if binary {
        return Ok(Value::Bytes(bytes.into()));
    }
    match String::from_utf8(bytes) {
        Ok(s) => Ok(Value::Str(s.into())),
        Err(_) => io_error(format!("'{path}' is not valid UTF-8 (use binary: true)")),
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
        Err(e) => io_error(format!("cannot write '{path}': {}", os_message(&e))),
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
    let failed = |e: std::io::Error| io_error(format!("cannot list '{path}': {}", os_message(&e)));
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
        Err(e) => io_error(format!("cannot create '{path}': {}", os_message(&e))),
    }
}

/// `remove_file(path)`: a file, never a folder (5.10).
fn remove_file(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "remove_file", "path")?;
    match std::fs::remove_file(&*path) {
        Ok(()) => Ok(Value::Null),
        Err(e) => io_error(format!("cannot remove '{path}': {}", os_message(&e))),
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
    argv[0] = match find_program(name, &lookup).into_iter().next() {
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
    let mut result = IndexMap::new();
    result.insert(Rc::from("code"), Value::Int(code));
    result.insert(Rc::from("stdout"), Value::str(&String::from_utf8_lossy(&c.stdout)));
    result.insert(Rc::from("stderr"), Value::str(&String::from_utf8_lossy(&c.stderr)));
    Ok(Value::dict(result))
}

/// `sleep(seconds)` on the interpreter thread; an interrupt still ends the run through the
/// watchdog (5.10, 6.5).
fn sleep(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let (secs, shown) = match &args[0] {
        Value::Int(n) => (*n as f64, n.to_string()),
        Value::Float(x) => (*x, float_str(*x)),
        other => return type_error(format!("sleep() argument 'seconds' must be an int or float, got {}", other.type_name())),
    };
    if secs < 0.0 {
        return value_error(format!("sleep() argument 'seconds' must not be negative, got {shown}"));
    }
    match Duration::try_from_secs_f64(secs) {
        Ok(d) => std::thread::sleep(d),
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
        return type_error(format!("{name}() needs numbers or strings, got {}", bad.type_name()));
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
        fail("AssertionError", message)
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
pub fn str_arg(v: &Value, func: &str, param: &str) -> Exec<Rc<str>> {
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
fn print(it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
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
        text.push_str(&it.str_of(v)?);
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
    let message = str_arg(&args[1], "error", "message")?;
    let data = args.pop().unwrap_or(Value::Null);
    Ok(Value::Error(Rc::new(ErrorValue::with_data(kind, message, data))))
}
