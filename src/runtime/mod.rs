//! The tree-walking interpreter (INTERNALS ch. 5, 6).

pub mod builtins;
pub mod call;
pub mod env;
pub mod error;
pub mod interp;
pub mod methods;
pub mod module;
pub mod ops;
pub mod strings;
pub mod value;

use std::collections::HashMap;
use std::path::Path;

use crate::syntax::span::{LoadError, SourceMap};
use crate::syntax::{check, lexer, parser};
use error::{render_diag, render_error, Flow};
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
    // Static checks report every violation, in source order (4.5).
    let diags = check::check(&program);
    if !diags.is_empty() {
        for d in &diags {
            eprint!("{}", render_diag(&sm, d));
        }
        return 1;
    }
    // Every file the import literals name, depth-first; their diagnostics wait for the
    // import that reaches them (6.1 step 2).
    let mut parsed = HashMap::new();
    module::parse_graph(&mut sm, file, &program, &mut parsed);
    let mut interp = Interp::new(sm, parsed, crate::INTERP_STACK_BYTES);
    let result = interp.run(file, &program);
    // Program output comes before the error that ended it (5.10).
    interp.out.flush();
    match result {
        Ok(()) => 0,
        Err(Flow::Error(e)) => {
            eprint!("{}", render_error(&interp.sm, &e));
            1
        }
        Err(Flow::Return(_) | Flow::Break | Flow::Continue) => {
            unreachable!("return, break and continue are placed statically (4.5)")
        }
    }
}
