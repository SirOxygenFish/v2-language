//! The `v2` binary: argument parsing, threads and exit codes (INTERNALS 1.2, 1.3, 6.5).

mod adapters;
mod build;
mod cli;
mod codec;
mod config;
mod doctor;
mod engines;
mod ffi;
mod runtime;
mod syntax;

use std::io::Write;
use std::process;

use cli::{Command, Invocation};

/// The interpreter thread's stack (INTERNALS 1.3, ch. 5).
const INTERP_STACK_BYTES: usize = 256 * 1024 * 1024;

/// Exit code for usage and configuration errors (INTERNALS 6.5).
const EXIT_USAGE: i32 = 2;
/// Exit code for an internal panic (INTERNALS 6.5).
const EXIT_PANIC: i32 = 101;

fn main() {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    let invocation = match cli::parse(&args) {
        Ok(inv) => inv,
        Err(cli::UsageError(msg)) => {
            eprintln!("v2: {msg}");
            eprintln!("run `v2 --help` for usage");
            exit(EXIT_USAGE);
        }
    };

    match invocation.command {
        Command::Help => {
            print!("{}", cli::HELP);
            exit(0);
        }
        Command::Version => {
            println!("{}", cli::version_line());
            exit(0);
        }
        _ => {}
    }

    // A panic anywhere is a V2 bug: one line on stderr, exit 101 (6.5).
    std::panic::set_hook(Box::new(|info| {
        eprintln!("v2: internal error: {}", panic_message(info));
    }));

    let watchdog = engines::session::Watchdog::start();
    let interpreter = std::thread::Builder::new()
        .name("v2-interp".into())
        .stack_size(INTERP_STACK_BYTES)
        .spawn(move || run(invocation));
    let code = match interpreter {
        Ok(handle) => handle.join().unwrap_or(EXIT_PANIC),
        Err(e) => {
            eprintln!("v2: internal error: cannot start the interpreter thread: {e}");
            EXIT_PANIC
        }
    };
    watchdog.stop();
    exit(code);
}

/// Runs on the interpreter thread and returns the process exit code.
fn run(invocation: Invocation) -> i32 {
    let what = match &invocation.command {
        Command::Script { .. } => "running a script",
        Command::RunProject { .. } => "`v2 run`",
        Command::Check { .. } => "`v2 check`",
        Command::Doctor { .. } => "`v2 doctor`",
        Command::CacheClean => "`v2 cache clean`",
        Command::Help | Command::Version => unreachable!("handled in main"),
    };
    eprintln!("v2: {what} is not implemented yet in this build");
    EXIT_USAGE
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    let text = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    };
    match info.location() {
        Some(loc) => format!("{text} ({}:{})", loc.file(), loc.line()),
        None => text,
    }
}

/// Flush the standard streams, then end the process with `code`.
fn exit(code: i32) -> ! {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    process::exit(code)
}
