//! The `v2` binary: argument parsing, threads and exit codes (INTERNALS 1.2, 1.3, 6.5, 19.1).

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

use cli::Command;

/// The interpreter thread's stack (INTERNALS 1.3, ch. 5).
const INTERP_STACK_BYTES: usize = 256 * 1024 * 1024;

/// Exit code for usage and configuration errors (INTERNALS 18.5, 19.1).
const EXIT_USAGE: i32 = 2;
/// Exit code for an internal panic (INTERNALS 6.5).
const EXIT_PANIC: i32 = 101;

fn main() {
    let raw: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // A file V2 couldn't name in its messages and paths: said so, not "no such file" (19.1).
    let file = raw.get(usize::from(raw.first().is_some_and(|w| w == "check")));
    if let Some(file) = file.filter(|f| f.to_str().is_none()) {
        stderrln!("v2: the file name '{}' is not valid UTF-8, which V2 needs", file.to_string_lossy());
        exit(EXIT_USAGE);
    }
    let args: Vec<String> = raw.iter().map(|a| a.to_string_lossy().into_owned()).collect();

    // A panic anywhere is a V2 bug: one line on stderr, exit 101 (6.5). The write can't fail
    // into a second panic.
    std::panic::set_hook(Box::new(|info| {
        let _ = writeln!(std::io::stderr().lock(), "v2: internal error: {}", panic_message(info));
    }));

    let command = match cli::parse(&args) {
        Ok(c) => c,
        Err(cli::UsageError(msg)) => {
            stderrln!("v2: {msg}");
            // `v2 -dash.v2`: a script whose name starts with -, which PowerShell even splits
            // before its dot (`-dash` `.v2`) (19.1).
            // `v2 check -dash.v2` the same way, split or not.
            let check = args.first().is_some_and(|a| a == "check") && args.get(1).is_some_and(|f| f.starts_with('-'));
            if msg.starts_with("unknown flag") || check {
                let at = usize::from(check);
                let first = &args[at];
                let split = args.get(at + 1).filter(|n| n.starts_with('.')).map(|n| format!("{first}{n}"));
                let named = [Some(first.clone()), split, Some(format!("{first}.v2"))].into_iter().flatten().find(|f| std::path::Path::new(f).is_file());
                if let Some(file) = named {
                    if check {
                        stderrln!("to check the file {file}, write: v2 check ./{file}");
                    } else {
                        stderrln!("to run the file {file}, write: v2 ./{file}");
                    }
                }
            }
            stderrln!("run `v2 --help` for usage");
            exit(EXIT_USAGE);
        }
    };

    match command {
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
        .spawn(move || run(command));
    let code = match interpreter {
        Ok(handle) => handle.join().unwrap_or(EXIT_PANIC),
        // The system's limit, not a bug in V2.
        Err(e) => {
            stderrln!("v2: cannot start the interpreter thread: {e}");
            1
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
fn run(command: Command) -> i32 {
    // Debug builds only: a deliberate panic for the exit-101 test (INTERNALS 22.2).
    #[cfg(debug_assertions)]
    if std::env::var_os("V2_TEST_PANIC").is_some() {
        panic!("V2_TEST_PANIC is set");
    }
    match command {
        // Config is loaded before the script is parsed (ch. 18); errors exit 2.
        Command::Script { file, args } => match load_config(&folder_of(&file)) {
            Ok(cfg) => runtime::run_script(&file, args, std::rc::Rc::new(cfg)),
            Err(code) => code,
        },
        Command::Check { file } => match load_config(&folder_of(&file)) {
            Ok(_) => doctor::check(&file),
            Err(code) => code,
        },
        Command::Doctor => {
            let cwd = std::env::current_dir().unwrap_or_default();
            match load_config(&cwd) {
                Ok(cfg) => doctor::doctor(std::rc::Rc::new(cfg)),
                Err(code) => code,
            }
        }
        Command::CacheClean => {
            stderrln!("v2: `v2 cache clean` is not implemented yet in this build");
            EXIT_USAGE
        }
        Command::Help | Command::Version => unreachable!("handled in main"),
    }
}

/// The folder a script lives in (its config's start folder, 18.1): the real file's, a symlink
/// followed, as `script_dir()` and imports see it.
fn folder_of(file: &str) -> std::path::PathBuf {
    let real = std::fs::canonicalize(file).map(|p| syntax::span::strip_verbatim(&p)).ok();
    let path = real.unwrap_or_else(|| std::path::PathBuf::from(file));
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    }
}

/// Load the configuration, or print the error and give exit code 2 (18.5).
fn load_config(start: &std::path::Path) -> Result<config::Config, i32> {
    config::load(start).map_err(|e| {
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
