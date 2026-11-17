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
        let text = format!($($t)*);
        let _ = std::io::stderr().lock().write_all(text.as_bytes());
        $crate::runtime::builtins::wrote_stderr(text.as_bytes());
    }};
}
macro_rules! stderrln {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        $crate::engines::process::stop_if_interrupted();
        let _ = writeln!(std::io::stderr().lock(), $($t)*);
        $crate::runtime::builtins::wrote_stderr(b"\n");
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

/// The interpreter thread's stack (INTERNALS 1.3, ch. 5), and the smaller ones tried when the
/// system won't reserve it (a virtual memory limit, `ulimit -v`): recursion then stops sooner.
const INTERP_STACKS: [usize; 3] = [256 << 20, 64 << 20, 16 << 20];

/// The stack the interpreter thread got, for its recursion guard.
static INTERP_STACK_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(256 << 20);

pub fn interp_stack_bytes() -> usize {
    INTERP_STACK_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Unix: the address-space limit (`ulimit -v`), if one is set.
#[cfg(unix)]
fn address_limit() -> Option<libc::rlim_t> {
    // SAFETY: getrlimit fills a zeroed struct.
    let mut r: libc::rlimit = unsafe { std::mem::zeroed() };
    (unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut r) } == 0 && r.rlim_cur != libc::RLIM_INFINITY).then_some(r.rlim_cur)
}

/// Unix: under an address-space limit, a stack bigger than a quarter of it leaves too little for
/// the heap, and deep recursion ends in an abort ("memory allocation of 104 bytes failed")
/// instead of V2's stack error.
fn stack_cap() -> usize {
    #[cfg(unix)]
    if let Some(limit) = address_limit() {
        return usize::try_from(limit / 4).unwrap_or(usize::MAX);
    }
    usize::MAX
}

/// Exit code for usage and configuration errors (INTERNALS 18.5, 19.1).
const EXIT_USAGE: i32 = 2;
/// Exit code for an internal panic (INTERNALS 6.5).
const EXIT_PANIC: i32 = 101;

fn main() {
    // Before any thread starts: only the interpreter thread takes SIGINT and the others (6.5).
    #[cfg(unix)]
    engines::process::take_interrupts(false);
    // glibc gives each thread that allocates an arena of its own, each holding 64 MiB of address
    // space: under `ulimit -v` they, not the program, would use it up. One arena then.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if address_limit().is_some() {
        // SAFETY: a malloc setting, made before any other thread runs.
        unsafe {
            libc::mallopt(libc::M_ARENA_MAX, 1);
        }
    }
    let raw: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // A file V2 couldn't name in its messages and paths: said so, not "no such file" (19.1).
    let file = raw.get(usize::from(raw.first().is_some_and(|w| w == "check")));
    if let Some(file) = file.filter(|f| f.to_str().is_none()) {
        stderrln!("v2: the file name '{}' is not valid UTF-8, which V2 needs: the name is in another encoding; rename it", syntax::span::shown_path(&file.to_string_lossy()));
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
            stderrln!("v2: {}", syntax::span::shown_path(&msg));
            // `v2 -dash.v2`: a script whose name starts with -, which PowerShell even splits
            // before its dot (`-dash` `.v2`) (19.1).
            // `v2 check -dash.v2` the same way, split or not.
            let check = args.first().is_some_and(|a| a == "check") && args.get(1).is_some_and(|f| f.starts_with('-'));
            if msg.starts_with("unknown flag") || check {
                let at = usize::from(check);
                let first = &args[at];
                let split = args.get(at + 1).filter(|n| n.starts_with('.')).map(|n| format!("{first}{n}"));
                let named = [Some(first.clone()), split, Some(format!("{first}.v2"))].into_iter().flatten().find(|f| std::path::Path::new(f).is_file());
                if let Some(file) = named.map(|f| syntax::span::shown_path(&f)) {
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
    let mut command = Some(command);
    let mut interpreter = None;
    let mut refused = None;
    let cap = stack_cap();
    let smallest = INTERP_STACKS[INTERP_STACKS.len() - 1];
    for size in INTERP_STACKS.into_iter().filter(|&s| s <= cap || s == smallest) {
        INTERP_STACK_BYTES.store(size, std::sync::atomic::Ordering::Relaxed);
        // The closure gets the command only when the thread starts.
        let (tx, rx) = std::sync::mpsc::channel::<Command>();
        match std::thread::Builder::new().name("v2-interp".into()).stack_size(size).spawn(move || {
            #[cfg(unix)]
            engines::process::take_interrupts(true);
            match rx.recv() {
                Ok(c) => run(c),
                Err(_) => 0,
            }
        }) {
            Ok(handle) => {
                let _ = tx.send(command.take().expect("sent once"));
                interpreter = Some(handle);
                break;
            }
            Err(e) => refused = Some(e),
        }
    }
    let code = match interpreter {
        Some(handle) => {
            let code = handle.join().unwrap_or(EXIT_PANIC);
            // A signal that came as the run ended is handled here, then checked below.
            #[cfg(unix)]
            engines::process::take_interrupts(true);
            code
        }
        // The system's limit, not a bug in V2.
        None => {
            let why = refused.map(|e| e.to_string()).unwrap_or_default();
            // The same error for a memory limit and a process limit: both are named.
            stderrln!("v2: cannot start the interpreter thread: {why}");
            stderrln!("a limit set for this shell may be too low: on memory (ulimit -v; V2 needs about 50 MB, deep recursion about 300 MB) or on processes and threads (ulimit -u)");
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
            // The cwd with its links followed (a junction, a symlink), as a run of a script there
            // starts from the script's real folder (18.1).
            let cwd = std::env::current_dir().unwrap_or_default();
            let cwd = std::fs::canonicalize(&cwd).map(|p| syntax::span::strip_verbatim(&p)).unwrap_or(cwd);
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
