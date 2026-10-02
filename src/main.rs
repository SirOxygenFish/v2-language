//! The `v2` binary: argument parsing, threads and exit codes (INTERNALS 1.2, 1.3, 6.5).

/// `print!`/`println!`/`eprint!`/`eprintln!` that never panic: a closed or broken stdout or
/// stderr must not turn an exit path into a panic (101) or an abort (6.5). Write errors are
/// ignored; the exit code says what happened. After an interrupt nothing more is printed to
/// stderr: the thread stops there and the run ends with 130 (6.5).
macro_rules! stdout {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        let _ = write!(std::io::stdout().lock(), $($t)*);
    }};
}
macro_rules! stdoutln {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout().lock(), $($t)*);
    }};
}
macro_rules! stderr {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        $crate::engines::process::stop_if_interrupted();
        let _ = write!(std::io::stderr().lock(), $($t)*);
    }};
}
macro_rules! stderrln {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        $crate::engines::process::stop_if_interrupted();
        let _ = writeln!(std::io::stderr().lock(), $($t)*);
    }};
}

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

    // A panic anywhere is a V2 bug: one line on stderr, exit 101 (6.5). The write can't fail
    // into a second panic.
    std::panic::set_hook(Box::new(|info| {
        let _ = writeln!(std::io::stderr().lock(), "v2: internal error: {}", panic_message(info));
    }));

    let invocation = match cli::parse(&args) {
        Ok(inv) => inv,
        Err(cli::UsageError(msg)) => {
            stderrln!("v2: {msg}");
            stderrln!("run `v2 --help` for usage");
            exit(EXIT_USAGE);
        }
    };

    match invocation.command {
        Command::Help => {
            stdout!("{}", cli::HELP);
            exit(0);
        }
        Command::Version => {
            stdoutln!("{}", cli::version_line());
            exit(0);
        }
        _ => {}
    }

    let watchdog = engines::process::Watchdog::start();
    let interpreter = std::thread::Builder::new()
        .name("v2-interp".into())
        .stack_size(INTERP_STACK_BYTES)
        .spawn(move || run(invocation));
    let code = match interpreter {
        Ok(handle) => handle.join().unwrap_or(EXIT_PANIC),
        Err(e) => {
            stderrln!("v2: internal error: cannot start the interpreter thread: {e}");
            EXIT_PANIC
        }
    };
    watchdog.stop();
    // The run dir goes on every exit path; the interrupt path removes it itself (6.5, 20.3).
    config::rundir::remove();
    // An interrupt that came while the run was ending still ends it with 130 (6.5).
    if engines::process::interrupted() {
        engines::process::exit_interrupted();
    }
    exit(code);
}

/// Runs on the interpreter thread and returns the process exit code.
fn run(invocation: Invocation) -> i32 {
    // Debug builds only: a deliberate panic for the exit-101 test (INTERNALS 22.2).
    #[cfg(debug_assertions)]
    if std::env::var_os("V2_TEST_PANIC").is_some() {
        panic!("V2_TEST_PANIC is set");
    }
    let flags = &invocation.flags;
    let what = match invocation.command {
        Command::Script { file, args } => {
            // Config is loaded before the script is parsed (ch. 18); errors exit 2.
            return match load_config(&folder_of(&file), flags) {
                Ok(cfg) => runtime::run_script(&file, args, std::rc::Rc::new(cfg)),
                Err(code) => code,
            };
        }
        Command::RunProject { args } => {
            let cwd = std::env::current_dir().unwrap_or_default();
            let Some(root) = config::find_root(&cwd) else {
                stderrln!("v2: no v2.toml found in {} or its parents", cwd.display());
                return EXIT_USAGE;
            };
            let cfg = match load_config(&cwd, flags) {
                Ok(c) => c,
                Err(code) => return code,
            };
            let Some(entry) = cfg.entry.clone() else {
                stderrln!("v2: {} has no [project] entry", root.join("v2.toml").display());
                return EXIT_USAGE;
            };
            return runtime::run_script(&entry.value.to_string_lossy(), args, std::rc::Rc::new(cfg));
        }
        Command::Check { file } => {
            return match load_config(&folder_of(&file), flags) {
                Ok(cfg) => doctor::check(&file, &cfg),
                Err(code) => code,
            };
        }
        // `v2 doctor TAG` arrives in roadmap 1.9.
        Command::Doctor { tag: None } => {
            let cwd = std::env::current_dir().unwrap_or_default();
            return match load_config(&cwd, flags) {
                Ok(cfg) => doctor::doctor(std::rc::Rc::new(cfg)),
                Err(code) => code,
            };
        }
        Command::Doctor { tag: Some(_) } => "`v2 doctor TAG`",
        Command::CacheClean => "`v2 cache clean`",
        Command::Help | Command::Version => unreachable!("handled in main"),
    };
    stderrln!("v2: {what} is not implemented yet in this build");
    EXIT_USAGE
}

/// The folder a script lives in (its config's start folder, 18.1).
fn folder_of(file: &str) -> std::path::PathBuf {
    match std::path::Path::new(file).parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    }
}

/// Load the configuration, or print the error and give exit code 2 (18.5).
fn load_config(start: &std::path::Path, flags: &cli::Flags) -> Result<config::Config, i32> {
    config::load(start, flags).map_err(|e| {
        stderrln!("{}", e.0);
        EXIT_USAGE
    })
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
