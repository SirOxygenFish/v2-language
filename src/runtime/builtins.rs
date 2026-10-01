//! Builtins and the `Out` writer (INTERNALS 5.10). Roadmap 0.2 has `print`; the rest of
//! DOCS 3.11 arrives in 0.9.

use std::io::{BufWriter, IsTerminal, StdoutLock, Write};

use super::error::{did_you_mean, ErrorValue};
use super::interp::Interp;
use super::value::{to_str, Value};
use crate::syntax::ast::Name;
use crate::syntax::span::os_message;

pub type BuiltinFn = fn(&mut Interp, Vec<Value>, Vec<(Name, Value)>) -> Result<Value, ErrorValue>;

pub struct BuiltinDef {
    pub name: &'static str,
    pub f: BuiltinFn,
}

pub static BUILTINS: &[BuiltinDef] = &[BuiltinDef { name: "print", f: print }];

pub fn lookup(name: &str) -> Option<&'static BuiltinDef> {
    BUILTINS.iter().find(|b| b.name == name)
}

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

    pub fn write_str(&mut self, s: &str) -> Result<(), ErrorValue> {
        let r = self.w.write_all(s.as_bytes()).and_then(|_| if self.tty { self.w.flush() } else { Ok(()) });
        r.map_err(|e| ErrorValue::new("IOError", format!("cannot write to stdout: {}", os_message(&e))))
    }

    pub fn flush(&mut self) {
        let _ = self.w.flush();
    }
}

/// `print(...values, sep: " ", end: "\n")`
fn print(it: &mut Interp, args: Vec<Value>, named: Vec<(Name, Value)>) -> Result<Value, ErrorValue> {
    let mut sep = String::from(" ");
    let mut end = String::from("\n");
    for (name, value) in named {
        let slot = match &*name.text {
            "sep" => &mut sep,
            "end" => &mut end,
            other => {
                let mut msg = format!("print() got an unexpected argument '{other}'");
                if let Some(s) = did_you_mean(other, ["sep", "end"]) {
                    msg.push_str(&format!(" — did you mean '{s}'?"));
                }
                return Err(ErrorValue::new("ArityError", msg));
            }
        };
        match value {
            Value::Str(s) => *slot = s.to_string(),
            other => {
                return Err(ErrorValue::new(
                    "TypeError",
                    format!("print() argument '{}' must be a str, got {}", name.text, other.type_name()),
                ))
            }
        }
    }
    let mut text = args.iter().map(to_str).collect::<Vec<_>>().join(&sep);
    text.push_str(&end);
    it.out.write_str(&text)?;
    Ok(Value::Null)
}
