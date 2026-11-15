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
use std::rc::Rc;

use crate::engines::registry::Registry;
use crate::engines::resolve::CacheMode;
use crate::syntax::span::{os_message, LoadError, SourceMap};
use crate::syntax::{check, lexer, parser};
use error::{render_diag, render_error, Flow};
use interp::Interp;

/// `v2 FILE [ARGS...]` on the interpreter thread; returns the exit code (6.1, 6.5).
pub fn run_script(path: &str, args: Vec<String>, cfg: Rc<crate::config::Config>) -> i32 {
    // A missing FILE or a folder: what to do next (19.1).
    if let Some(msg) = crate::cli::entry_error(path, args.first().map(String::as_str), "v2") {
        stderrln!("v2: {msg}");
        return 2;
    }
    // A Python or JS file given to V2 gets a note after its errors.
    let note = || {
        if let Some(note) = crate::cli::foreign_file_note(path) {
            stderrln!("{note}");
        }
    };
    let mut sm = SourceMap::new();
    let file = match sm.load(Path::new(path)) {
        Ok(id) => id,
        Err(LoadError::Io(msg)) => {
            stderrln!("v2: {msg}");
            return 2;
        }
        Err(LoadError::NotUtf8 { display, line, col, message }) => {
            stderrln!("{display}:{line}:{col}: SyntaxError: {message}");
            return 1;
        }
    };
    let mut program = match lexer::lex(&sm, file).and_then(|toks| parser::parse_program(&sm, toks)) {
        Ok(p) => p,
        Err(d) => {
            stderr!("{}", render_diag(&sm, &d));
            note();
            return 1;
        }
    };
    // Static checks report every violation, in source order (4.5).
    let diags = check::check(&program);
    if !diags.is_empty() {
        for d in &diags {
            stderr!("{}", render_diag(&sm, d));
        }
        note();
        return 1;
    }
    crate::syntax::resolve::resolve(&mut program);
    // Every file the import literals name, depth-first; their diagnostics wait for the
    // import that reaches them (6.1 step 2).
    let mut parsed = HashMap::new();
    module::parse_graph(&mut sm, file, &program, &mut parsed);
    let mut interp = Interp::new(sm, parsed, crate::INTERP_STACK_BYTES, Registry::real(cfg, CacheMode::ReadWrite));
    interp.args = args;
    let result = interp.run(file, &program);
    // Program output comes before the error that ended it (5.10). Output that can't be written
    // at the end is reported, and a run that would have exited 0 exits 1 (6.5).
    // Output that never reached stdout and that no `IOError` told the program about (6.5). A
    // reader that went away (`| head`) is no error to report: the run ends as it would, but a
    // run that would have exited 0 exits 141.
    let broken = std::cell::Cell::new(false);
    let lost = |out: &mut crate::runtime::builtins::Out| match out.flush() {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
            broken.set(true);
            None
        }
        Err(e) => Some(format!("cannot write to stdout: {}", os_message(&e))),
        Ok(()) => out.lost.take().map(|m| m.to_string()),
    };
    let flushed = lost(&mut interp.out);
    if let Some(m) = &flushed {
        stderrln!("v2: {m}");
    }
    let code = match result {
        Ok(()) => 0,
        Err(Flow::Error(e)) => {
            stderr!("{}", render_error(&interp.sm, &e));
            note();
            // No usable toolchain is exit 3 (6.5, 19.1).
            if &*e.kind == "EngineNotFound" {
                3
            } else {
                1
            }
        }
        Err(Flow::Exit(code)) => code,
        Err(Flow::Return(_) | Flow::Break | Flow::Continue) => {
            unreachable!("return, break and continue are placed statically (4.5)")
        }
    };
    // Then the engines stop: `bye`, one shared grace period that still forwards their output,
    // kill and reap; late output is flushed too (6.5, 8.7).
    interp.shutdown_sessions();
    let late = lost(&mut interp.out);
    if let (None, Some(m)) = (&flushed, &late) {
        stderrln!("v2: {m}");
    }
    match code {
        0 if broken.get() => crate::engines::process::EXIT_BROKEN_PIPE,
        0 if flushed.is_some() || late.is_some() => 1,
        code => code,
    }
}
