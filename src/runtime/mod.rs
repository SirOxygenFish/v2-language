//! The tree-walking interpreter (INTERNALS ch. 5, 6).

pub mod builtins;
pub mod call;
pub mod env;
pub mod error;
pub mod interp;
pub mod module;
pub mod value;

use std::path::Path;

use crate::syntax::span::{LoadError, SourceMap};
use crate::syntax::{lexer, parser};
use error::{render_diag, render_error};
use interp::Interp;

/// `v2 FILE [ARGS...]` on the interpreter thread; returns the exit code (6.1, 6.5).
pub fn run_script(path: &str, _args: Vec<String>) -> i32 {
    let mut sm = SourceMap::new();
    let file = match sm.load(Path::new(path)) {
        Ok(id) => id,
        Err(LoadError::Io(msg)) => {
            eprintln!("v2: {msg}");
            return 2;
        }
        Err(LoadError::NotUtf8 { display, line }) => {
            eprintln!("{display}:{line}:1: SyntaxError: file is not valid UTF-8");
            return 1;
        }
    };
    let program = match lexer::lex(&sm, file).and_then(|toks| parser::parse_program(&sm, toks)) {
        Ok(p) => p,
        Err(d) => {
            eprint!("{}", render_diag(&sm, &d));
            return 1;
        }
    };
    let mut interp = Interp::new(&sm);
    let result = interp.run(&program);
    interp.out.flush();
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprint!("{}", render_error(&sm, &e));
            1
        }
    }
}
