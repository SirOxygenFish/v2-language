//! Builtins and the `Out` writer (INTERNALS 5.10). So far `print` and `error`; the rest of
//! DOCS 3.11 arrives in roadmap 0.9.

use std::io::{BufWriter, IsTerminal, StdoutLock, Write};
use std::rc::Rc;

use super::call::ParamSpec;
use super::error::{fail, type_error, ErrorValue, Exec};
use super::interp::Interp;
use super::value::Value;
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
];

/// The only writer of the host's stdout: buffered, flushed after every write when stdout is a
/// terminal, and at the end of the run (5.10).
pub struct Out {
    w: BufWriter<StdoutLock<'static>>,
    tty: bool,
}

impl Out {
    pub fn new() -> Out {
        let stdout = std::io::stdout();
        let tty = stdout.is_terminal();
        Out { w: BufWriter::with_capacity(64 * 1024, stdout.lock()), tty }
    }

    pub fn write_str(&mut self, s: &str) -> Exec<()> {
        let r = self.w.write_all(s.as_bytes()).and_then(|_| if self.tty { self.w.flush() } else { Ok(()) });
        match r {
            Ok(()) => Ok(()),
            Err(e) => fail("IOError", format!("cannot write to stdout: {}", os_message(&e))),
        }
    }

    pub fn flush(&mut self) {
        let _ = self.w.flush();
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
