//! Builtins and the `Out` writer (INTERNALS 5.10): DOCS 3.11 without `value`, which needs
//! handles (roadmap Phase 1).

use std::io::{BufRead, BufWriter, IsTerminal, StdoutLock, Write};
use std::rc::Rc;

use super::call::ParamSpec;
use super::error::{fail, type_error, value_error, ErrorValue, Exec, Flow};
use super::interp::Interp;
use super::methods::int_arg;
use super::value::Value;
use crate::codec::json;
use crate::syntax::span::{os_message, Span};

pub type BuiltinFn = fn(&mut Interp, Vec<Value>, Span) -> Exec<Value>;

pub struct BuiltinDef {
    pub name: &'static str,
    pub params: &'static [ParamSpec],
    pub f: BuiltinFn,
}

pub static BUILTINS: &[BuiltinDef] = &[
    BuiltinDef {
        name: "print",
        params: &[ParamSpec::rest("values"), ParamSpec::opt("sep", || Value::str(" ")), ParamSpec::opt("end", || Value::str("\n"))],
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
    BuiltinDef { name: "write_file", params: &[ParamSpec::req("path"), ParamSpec::req("data")], f: write_file },
    BuiltinDef { name: "getenv", params: &[ParamSpec::req("name"), ParamSpec::opt("default", || Value::Null)], f: getenv },
    BuiltinDef {
        name: "args",
        params: &[],
        f: |it, _, _| Ok(Value::list(it.args.iter().map(|a| Value::str(a)).collect())),
    },
    BuiltinDef { name: "exit", params: &[ParamSpec::opt("code", || Value::Int(0))], f: exit },
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

/// `range(end)` / `range(start, end, step = 1)`: a list of ints, sized before it is built (5.10).
/// `range(end)` or `range(start, end, step = 1)`: one argument, by position or as `end:`, is the
/// end (5.10).
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
    let binary = match &args[1] {
        Value::Bool(b) => *b,
        other => return type_error(format!("read_file() argument 'binary' must be a bool, got {}", other.type_name())),
    };
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

/// `write_file(path, data)`: a str as UTF-8 or bytes as is; creates or truncates (5.10).
fn write_file(_it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let path = str_arg(&args[0], "write_file", "path")?;
    let r = match &args[1] {
        Value::Str(s) => std::fs::write(&*path, s.as_bytes()),
        Value::Bytes(b) => std::fs::write(&*path, b),
        other => return type_error(format!("write_file() argument 'data' must be a str or bytes, got {}", other.type_name())),
    };
    match r {
        Ok(()) => Ok(Value::Null),
        Err(e) => io_error(format!("cannot write '{path}': {}", os_message(&e))),
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

/// `print(...values, sep: " ", end: "\n")`: one write to `Out`.
fn print(it: &mut Interp, args: Vec<Value>, _at: Span) -> Exec<Value> {
    let sep = str_arg(&args[1], "print", "sep")?;
    let end = str_arg(&args[2], "print", "end")?;
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
    it.out.write_str(&text)?;
    Ok(Value::Null)
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
