//! Engine sessions and the watchdog thread (INTERNALS 1.3, 6.5, ch. 8).
//!
//! Sessions arrive in roadmap 1.2. The watchdog thread already owns the interrupt path (6.5):
//! the signal or console handler only sets `INTERRUPTED`, and the watchdog ends the process
//! with 130. Call and startup deadlines (8.5) join it with the sessions.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

/// How long the watchdog sleeps between checks at most (INTERNALS 1.3).
const TICK: Duration = Duration::from_millis(100);

/// The exit code of an interrupted run (6.5).
pub const EXIT_INTERRUPTED: i32 = 130;

/// Set by the interrupt handler; nothing else happens in the handler (6.5).
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Whether an interrupt came. From then on the interpreter thread runs no `catch` or `finally`
/// and prints no diagnostic; the watchdog (or main, when the run ended first) exits 130 (6.5).
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// Where the interpreter thread would run a `catch` or `finally` or print a diagnostic: after an
/// interrupt it stops here, and the watchdog ends the process within one tick (6.5). The exit
/// after a second is only a backstop in case the watchdog thread is gone.
pub fn stop_if_interrupted() {
    if interrupted() {
        std::thread::sleep(Duration::from_secs(1));
        exit_interrupted();
    }
}

/// The end of an interrupted run (6.5): kill every registered child (none until roadmap 1.2),
/// remove the run dir (none yet), and end the process at once: no `bye`, `catch` or `finally`,
/// and on Unix no atexit handlers or C stdio flushing either (`_exit`; on Windows Rust's exit
/// is `ExitProcess`).
pub fn exit_interrupted() -> ! {
    #[cfg(unix)]
    // SAFETY: `_exit` ends the process; nothing here relies on cleanup.
    unsafe {
        libc::_exit(EXIT_INTERRUPTED)
    }
    #[cfg(not(unix))]
    std::process::exit(EXIT_INTERRUPTED)
}

#[derive(Default)]
struct WatchState {
    stop: bool,
}

type Shared = Arc<(Mutex<WatchState>, Condvar)>;

/// The running watchdog's state, so the Windows console handler can wake it.
static WATCHDOG: OnceLock<Shared> = OnceLock::new();

/// Handle to the running watchdog thread.
pub struct Watchdog {
    shared: Shared,
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    pub fn start() -> Watchdog {
        let shared: Shared = Arc::new((Mutex::new(WatchState::default()), Condvar::new()));
        let _ = WATCHDOG.set(Arc::clone(&shared));
        let theirs = Arc::clone(&shared);
        let thread = std::thread::Builder::new().name("v2-watchdog".into()).spawn(move || watch(&theirs)).ok();
        // Without a watchdog nothing would end the run, so the OS default stays (6.5).
        if thread.is_some() {
            install_interrupt_handler();
        }
        Watchdog { shared, thread }
    }

    /// Ask the thread to finish and wait for it.
    pub fn stop(mut self) {
        {
            let (lock, cvar) = &*self.shared;
            lock.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
            cvar.notify_all();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn watch(shared: &(Mutex<WatchState>, Condvar)) {
    let (lock, cvar) = shared;
    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
    while !state.stop {
        if interrupted() {
            exit_interrupted();
        }
        state = match cvar.wait_timeout(state, TICK) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
}

/// Unix: SIGINT, SIGTERM and SIGHUP set `INTERRUPTED`, an async-signal-safe store. A signal
/// ignored when v2 starts (`nohup`, background jobs of a shell without job control) stays
/// ignored (6.5).
#[cfg(unix)]
fn install_interrupt_handler() {
    extern "C" fn on_signal(_: libc::c_int) {
        INTERRUPTED.store(true, Ordering::SeqCst);
    }
    // SAFETY: the handler only stores to an atomic; `sigaction` gets zeroed, filled structs.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
        libc::sigemptyset(&mut action.sa_mask);
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let mut old: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(sig, std::ptr::null(), &mut old) == 0 && old.sa_sigaction == libc::SIG_IGN {
                continue;
            }
            libc::sigaction(sig, &action, std::ptr::null_mut());
        }
    }
}

/// Windows: Ctrl+C, Ctrl+Break and console close. The routine runs on its own thread: it sets
/// `INTERRUPTED`, wakes the watchdog, then blocks until the process ends, because returning
/// would let `CTRL_CLOSE_EVENT` end the process before the watchdog cleans up (6.5).
#[cfg(windows)]
fn install_interrupt_handler() {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    unsafe extern "system" fn on_ctrl(_kind: u32) -> windows_sys::core::BOOL {
        INTERRUPTED.store(true, Ordering::SeqCst);
        if let Some(shared) = WATCHDOG.get() {
            shared.1.notify_all();
        }
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    // SAFETY: registering a handler routine with the right signature.
    unsafe {
        SetConsoleCtrlHandler(Some(on_ctrl), 1);
    }
}
