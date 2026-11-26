//! Child processes and the watchdog thread (INTERNALS 1.3, 6.5, 8.3, 8.5).
//!
//! Every process V2 starts is a `ChildProc`: it gets its own session (Unix) or its own process
//! group and Job Object (Windows), it is killed as a tree, and its exit is observed before it
//! is reaped. The watchdog thread owns the interrupt path and the deadline timers: the
//! interrupt handler only sets `INTERRUPTED`, and an expired timer kills its child's tree, so
//! the read or wait that the interpreter thread is blocked in ends.

#![allow(dead_code)] // probes (roadmap 1.1) and sessions (1.3) are the first callers

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long the watchdog sleeps between checks at most (INTERNALS 6.5, 8.5).
const TICK: Duration = Duration::from_millis(100);

/// How often the watchdog checks whether the top timer's child has exited (8.5).
const EXIT_CHECK: Duration = Duration::from_millis(250);

/// How long a worker stays idle before it gives back its RAM (8.8).
const PARK_AFTER: Duration = Duration::from_secs(10);

/// The exit code of an interrupted run (6.5).
pub const EXIT_INTERRUPTED: i32 = 130;

/// Set by the interrupt handler; nothing else happens in the handler (6.5).
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Unix: the first interrupt signal that came, which v2 ends by once it has cleaned up (6.5).
#[cfg(unix)]
static SIGNALED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Unix: Ctrl+Z (SIGTSTP) came; the watchdog stops the children, then v2 itself (6.5).
#[cfg(unix)]
static SUSPEND: AtomicBool = AtomicBool::new(false);

/// A wait this much longer than asked means the process (or the computer) was suspended.
const SUSPENDED: Duration = Duration::from_secs(1);

/// Whether an interrupt came. From then on the interpreter thread runs no `catch` or `finally`,
/// prints no diagnostic and starts no process; the watchdog (or main, when the run ended
/// first) exits 130 (6.5).
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// Where the interpreter thread would run a `catch` or `finally`, print a diagnostic or start
/// a process: after an interrupt it stops here, and the watchdog ends the process within one
/// tick (6.5). The exit after a second is only a backstop in case the watchdog thread is gone.
pub fn stop_if_interrupted() {
    if interrupted() {
        std::thread::sleep(Duration::from_secs(1));
        exit_interrupted();
    }
}

/// The end of an interrupted run (6.5): kill every registered child as a tree, remove the run
/// dir (best effort), and end the process at once: no `bye`, `catch` or `finally`.
pub fn exit_interrupted() -> ! {
    die(&lock())
}

/// The exit code when stdout's or stderr's reader went away: 128 + SIGPIPE, what a shell shows for
/// a command a closed pipe stopped (6.5).
pub const EXIT_BROKEN_PIPE: i32 = 141;

/// stdout's or stderr's reader went away (`v2 x.v2 | head`, `2>&1 | head`): nothing more can be
/// shown, so the run ends at once and quietly, as for an interrupt but with no output left to
/// write: every child stopped as a tree, the run dir removed, exit 141 (6.5).
pub fn exit_broken_pipe() -> ! {
    let w = lock();
    for (_, kill) in &w.children {
        kill.kill_tree();
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    for (_, kill) in &w.children {
        kill.wait_stopped(deadline);
    }
    cursor_back();
    if let Some(dir) = &w.run_dir {
        remove_run_dir(dir);
    }
    hard_exit(EXIT_BROKEN_PIPE)
}

/// Whether V2's stdout is a pipe or socket whose reader has gone (6.5): what a child that wrote to
/// it straight, and died of SIGPIPE, found. Windows: `false`; V2's next write finds out there.
#[cfg(unix)]
pub fn stdout_gone() -> bool {
    let mut fd = libc::pollfd { fd: 1, events: libc::POLLOUT, revents: 0 };
    // SAFETY: one valid pollfd, no timeout.
    let n = unsafe { libc::poll(&mut fd, 1, 0) };
    n > 0 && fd.revents & (libc::POLLERR | libc::POLLHUP) != 0
}

#[cfg(not(unix))]
pub fn stdout_gone() -> bool {
    false
}

fn die(w: &Watch) -> ! {
    for (_, kill) in &w.children {
        kill.kill_tree();
    }
    // What the program printed still reaches a file or a pipe (6.5).
    crate::runtime::builtins::write_pending(Duration::from_millis(300));
    // A killed process releases its files only once it has stopped (Windows), so wait a little
    // for the trees before removing the run dir (20.3).
    let deadline = Instant::now() + Duration::from_millis(500);
    for (_, kill) in &w.children {
        kill.wait_stopped(deadline);
    }
    // Once nothing draws on it any more.
    restore_terminal();
    // Windows: the console shows what is typed again after Ctrl+C in `input(echo: false)`.
    #[cfg(windows)]
    crate::runtime::builtins::echo_back();
    if let Some(dir) = &w.run_dir {
        remove_run_dir(dir);
    }
    #[cfg(unix)]
    die_by_signal();
    hard_exit(EXIT_INTERRUPTED)
}

/// Unix: the settings of stdin, stdout and stderr that were terminals when v2 started (6.5).
#[cfg(unix)]
static TERMINAL: OnceLock<Vec<(libc::c_int, libc::termios)>> = OnceLock::new();

/// Unix, at startup: remember the terminal's settings (echo, line input), which the interrupt path
/// puts back (6.5).
#[cfg(unix)]
fn remember_terminal() {
    let saved = (0..3)
        .filter_map(|fd| {
            // SAFETY: tcgetattr fills a zeroed struct for any descriptor, a terminal or not.
            let mut t: libc::termios = unsafe { std::mem::zeroed() };
            (unsafe { libc::isatty(fd) == 1 && libc::tcgetattr(fd, &mut t) == 0 }).then_some((fd, t))
        })
        .collect();
    let _ = TERMINAL.set(saved);
}

/// Set while v2 itself has changed the terminal's settings and not yet put them back (input read
/// with echo off): the interrupt path then puts them back whatever stdout is (6.5).
pub(crate) static TERMINAL_CHANGED: AtomicBool = AtomicBool::new(false);

/// On the interrupt path: a block stopped on the spot never ran the code that undoes what it did to
/// the terminal (echo turned off, a live display's hidden cursor), so on Unix the terminal gets back
/// the settings it had when v2 started (only while v2 is its foreground job: a background one would
/// change the settings of the shell that now reads it), and the cursor is shown (`show_cursor`).
/// Only when v2's stdout is a terminal, as v2 is then what the screen shows (in `v2 x.v2 | less`
/// the pager has put the terminal in a mode of its own, and Ctrl+C reaches it too), or when
/// `TERMINAL_CHANGED` is set (6.5).
fn restore_terminal() {
    if terminal_size(1).is_none() && !TERMINAL_CHANGED.load(Ordering::SeqCst) {
        return;
    }
    #[cfg(unix)]
    if let Some(saved) = TERMINAL.get() {
        // SAFETY: plain calls on the standard descriptors with the settings tcgetattr gave; SIGTTOU
        // ignored, a race with the shell taking the terminal back can't stop the process; TCSANOW
        // doesn't wait for output.
        unsafe {
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            for (fd, t) in saved.iter().filter(|(fd, _)| libc::tcgetpgrp(*fd) == libc::getpgrp()) {
                libc::tcsetattr(*fd, libc::TCSANOW, t);
            }
        }
    }
    show_cursor();
}

/// Set once a worker has been told that V2's stdout or stderr is a terminal (9.3): from then on a
/// block's live display may have hidden the cursor.
pub static TOLD_TERMINAL: AtomicBool = AtomicBool::new(false);

/// An end other than the normal one (`exit(n)`, an uncaught error, a closed pipe): a live display a
/// block started may never have ended (plain Python's `sys.exit` would have run its cleanup), so
/// when v2's stdout is a terminal and a worker was told it has one, the cursor is shown (6.5).
pub fn cursor_back() {
    if TOLD_TERMINAL.load(Ordering::SeqCst) && terminal_size(1).is_some() {
        show_cursor();
    }
}

/// `ESC[?25h` (show the cursor), once to each terminal among stdout and stderr: on Unix one whose
/// foreground process group is v2's (SIGTTOU ignored), on Windows a console with VT processing on.
/// From a thread of its own, at most 100 ms, as a terminal whose output is stopped (Ctrl+S, a
/// console's selection) blocks the write (6.5).
fn show_cursor() {
    const SHOW: &[u8] = b"\x1b[?25h";
    let (done, finished) = std::sync::mpsc::channel();
    let writer = spawn_helper(std::thread::Builder::new(), move || {
        #[cfg(unix)]
        // SAFETY: plain calls on the standard descriptors.
        unsafe {
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            let device = |fd| {
                let mut st: libc::stat = std::mem::zeroed();
                (libc::fstat(fd, &mut st) == 0).then_some((st.st_dev, st.st_ino))
            };
            let mut shown = None;
            for fd in [1, 2] {
                if libc::isatty(fd) == 1 && libc::tcgetpgrp(fd) == libc::getpgrp() && (shown.is_none() || shown != device(fd)) {
                    libc::write(fd, SHOW.as_ptr().cast(), SHOW.len());
                    shown = device(fd);
                }
            }
        }
        #[cfg(windows)]
        // SAFETY: plain calls on the standard handles, which stay open for the process.
        unsafe {
            use windows_sys::Win32::Foundation::CompareObjectHandles;
            use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, WriteConsoleA, ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
            let mut shown = Vec::new();
            for kind in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
                let h = GetStdHandle(kind);
                let mut mode = 0;
                if GetConsoleMode(h, &mut mode) == 0 || mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING == 0 || shown.iter().any(|&o| CompareObjectHandles(o, h) != 0) {
                    continue;
                }
                let mut written = 0;
                WriteConsoleA(h, SHOW.as_ptr(), SHOW.len() as u32, &mut written, std::ptr::null());
                shown.push(h);
            }
        }
        let _ = done.send(());
    });
    if writer.is_ok() {
        let _ = finished.recv_timeout(Duration::from_millis(100));
    }
}

/// Whether V2's stdout (`fd` 1) or stderr (2) is a terminal, and then its size (columns, rows; 0
/// when the terminal doesn't say), which `init` passes on to the blocks (9.3, 9.9). Unix: isatty
/// and TIOCGWINSZ; Windows: a console, its window.
pub fn terminal_size(fd: i32) -> Option<(u16, u16)> {
    #[cfg(unix)]
    // SAFETY: isatty and TIOCGWINSZ only read the descriptor's state, into a zeroed struct.
    unsafe {
        if libc::isatty(fd) != 1 {
            return None;
        }
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws as *mut libc::winsize) != 0 {
            return Some((0, 0));
        }
        Some((ws.ws_col, ws.ws_row))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Console::{GetConsoleMode, GetConsoleScreenBufferInfo, CONSOLE_SCREEN_BUFFER_INFO};
        let h = if fd == 1 { std::io::stdout().as_raw_handle() } else { std::io::stderr().as_raw_handle() };
        // SAFETY: the standard handle stays open for the process; each call fills its out-parameter.
        unsafe {
            let mut mode = 0;
            if GetConsoleMode(h, &mut mode) == 0 {
                return None;
            }
            let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
            if GetConsoleScreenBufferInfo(h, &mut info) == 0 {
                return Some((0, 0));
            }
            let w = info.srWindow;
            let span = |a: i16, b: i16| u16::try_from(i32::from(b) - i32::from(a) + 1).unwrap_or(0);
            Some((span(w.Left, w.Right), span(w.Top, w.Bottom)))
        }
    }
}

/// Windows: what `console_vt_on` changed: a standard handle, the mode its console had, and the mode
/// v2 set, put back at every exit (6.5).
#[cfg(windows)]
static CONSOLE_MODES: OnceLock<Vec<(u32, u32, u32)>> = OnceLock::new();

/// Windows, at startup: stdout's and stderr's consoles get `ENABLE_VIRTUAL_TERMINAL_PROCESSING`, so
/// the escape sequences of colors and live displays render in conhost too (6.5, 9.9). A console
/// whose VT processing is on already (Windows Terminal, another v2 on it: `v2 a.v2 | v2 b.v2`) is
/// left as it is and not recorded.
#[cfg(windows)]
fn console_vt_on() {
    use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
    // Both modes first: stdout and stderr are often one console, which the first change alters.
    // SAFETY: plain calls on the standard handles; each fills its out-parameter.
    let found: Vec<(u32, u32)> = [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
        .into_iter()
        .filter_map(|kind| unsafe {
            let mut mode = 0;
            (GetConsoleMode(GetStdHandle(kind), &mut mode) != 0 && mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING == 0).then_some((kind, mode))
        })
        .collect();
    let mut changed = Vec::new();
    for (kind, mode) in found {
        let set = mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
        // SAFETY: as above; a Windows without VT processing refuses the flag, which changes nothing.
        if unsafe { SetConsoleMode(GetStdHandle(kind), set) } != 0 {
            changed.push((kind, mode, set));
        }
    }
    let _ = CONSOLE_MODES.set(changed);
}

/// Windows, at every exit (the end of the run, an interrupt, a closed pipe): a console v2 turned VT
/// processing on for gets back the mode it had, unless its mode has changed since (6.5).
#[cfg(windows)]
pub fn restore_console() {
    use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, SetConsoleMode};
    for &(kind, mode, set) in CONSOLE_MODES.get().into_iter().flatten() {
        // SAFETY: plain calls on the standard handles; GetConsoleMode fills its out-parameter.
        unsafe {
            let h = GetStdHandle(kind);
            let mut now = 0;
            if GetConsoleMode(h, &mut now) != 0 && now == set {
                SetConsoleMode(h, mode);
            }
        }
    }
}

/// Unix: end by the signal that came, as a program without a handler does, so the shell sees a
/// process killed by SIGINT (shown as 130, SIGTERM 143, SIGHUP 129) and stops a loop of v2 runs
/// on one Ctrl+C, which it doesn't for a plain exit 130 (6.5). SIGQUIT leaves no core file.
#[cfg(unix)]
fn die_by_signal() {
    let sig = SIGNALED.load(Ordering::SeqCst);
    if sig == 0 {
        return;
    }
    // SAFETY: plain libc calls on this process; `raise` with the default action ends it.
    unsafe {
        if sig == libc::SIGQUIT {
            let none = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            libc::setrlimit(libc::RLIMIT_CORE, &none);
        }
        libc::signal(sig, libc::SIG_DFL);
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, sig);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
        libc::raise(sig);
    }
}

/// Kill every process in the session `sid` leads (8.3). Linux, through /proc.
#[cfg(target_os = "linux")]
fn kill_session(sid: libc::pid_t) {
    let Ok(dir) = std::fs::read_dir("/proc") else { return };
    for entry in dir.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<libc::pid_t>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        // After the command in parentheses: state, ppid, pgrp, session.
        let session = stat.rfind(')').and_then(|i| stat[i + 1..].split_whitespace().nth(3)?.parse::<libc::pid_t>().ok());
        if session == Some(sid) {
            // SAFETY: a plain signal call; `sid` is a live (unreaped) leader's, so not reused.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

/// Remove the run dir; on Windows up to 3 more tries 50 ms apart while killed processes
/// release their locks; errors are ignored (6.5, 20.3).
pub fn remove_run_dir(dir: &std::path::Path) {
    for attempt in 0..4 {
        if std::fs::remove_dir_all(dir).is_ok() || !dir.exists() || !cfg!(windows) || attempt == 3 {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `_exit` on Unix (no atexit handlers, no C stdio flushing); on Windows Rust's exit is
/// `ExitProcess`, after the consoles get their modes back.
fn hard_exit(code: i32) -> ! {
    #[cfg(windows)]
    restore_console();
    #[cfg(unix)]
    // SAFETY: `_exit` ends the process; nothing here relies on cleanup.
    unsafe {
        libc::_exit(code)
    }
    #[cfg(not(unix))]
    std::process::exit(code)
}

// ---------------------------------------------------------------------------------------------
// Timers (8.5)

/// One deadline. Only the top of the stack runs, and not while it is paused: a request's timer
/// pauses while V2 serves a call from that worker (8.5).
#[derive(Debug)]
struct Timer {
    id: u64,
    limit: Option<Duration>,
    used: Duration,
    running_since: Option<Instant>,
    /// Paused by `pause` until `resume`, wherever it is in the stack.
    paused: bool,
    fired: bool,
}

impl Timer {
    fn used_at(&self, now: Instant) -> Duration {
        self.used + self.running_since.map_or(Duration::ZERO, |s| now.saturating_duration_since(s))
    }
}

#[derive(Debug, Default)]
struct TimerStack {
    stack: Vec<Timer>,
}

impl TimerStack {
    /// Start or stop each clock from the stack: only the top runs, and not while paused.
    fn sync(&mut self, now: Instant) {
        let top = self.stack.len().checked_sub(1);
        for (i, t) in self.stack.iter_mut().enumerate() {
            let run = Some(i) == top && !t.paused;
            match (run, t.running_since) {
                (true, None) => t.running_since = Some(now),
                (false, Some(_)) => {
                    t.used = t.used_at(now);
                    t.running_since = None;
                }
                _ => {}
            }
        }
    }

    fn push(&mut self, id: u64, limit: Option<Duration>, now: Instant) {
        self.stack.push(Timer { id, limit, used: Duration::ZERO, running_since: None, paused: false, fired: false });
        self.sync(now);
    }

    fn pause(&mut self, id: u64, now: Instant) {
        if let Some(t) = self.stack.iter_mut().find(|t| t.id == id) {
            t.paused = true;
        }
        self.sync(now);
    }

    fn resume(&mut self, id: u64, now: Instant) {
        if let Some(t) = self.stack.iter_mut().find(|t| t.id == id) {
            t.paused = false;
        }
        self.sync(now);
    }

    fn pop(&mut self, id: u64, now: Instant) {
        self.stack.retain(|t| t.id != id);
        self.sync(now);
    }

    fn top_id(&self) -> Option<u64> {
        self.stack.last().map(|t| t.id)
    }

    /// The top timer, if it is running and its limit is used up.
    fn expired(&self, now: Instant) -> Option<u64> {
        let t = self.stack.last()?;
        let limit = t.limit?;
        (t.running_since.is_some() && !t.fired && t.used_at(now) >= limit).then_some(t.id)
    }

    /// How long until the top timer expires, if it is running.
    fn time_left(&self, now: Instant) -> Option<Duration> {
        let t = self.stack.last()?;
        let limit = t.limit?;
        (t.running_since.is_some() && !t.fired).then(|| limit.saturating_sub(t.used_at(now)))
    }

    fn mark_fired(&mut self, id: u64) {
        if let Some(t) = self.stack.iter_mut().find(|t| t.id == id) {
            t.fired = true;
        }
    }

    /// Time the process spent suspended ran no call: the running clock doesn't count it (8.5).
    fn skip(&mut self, gap: Duration, now: Instant) {
        for t in &mut self.stack {
            if let Some(since) = t.running_since {
                t.running_since = Some((since + gap).min(now));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The watchdog's shared state

#[derive(Default)]
struct Watch {
    timers: TimerStack,
    /// What each timer kills when it expires.
    kills: HashMap<u64, KillHandle>,
    fired: HashSet<u64>,
    /// Every running child, killed on the interrupt path (6.5).
    children: Vec<(u64, KillHandle)>,
    /// The workers that may park (8.8).
    parkable: Vec<Parkable>,
    run_dir: Option<PathBuf>,
    stop: bool,
}

/// A worker that gives back its RAM when idle (8.8).
struct Parkable {
    kill: KillHandle,
    idle_since: Instant,
    parked: bool,
}

impl Watch {
    /// The workers idle for `after` that haven't parked since their last request (8.8).
    fn due_to_park(&mut self, now: Instant, after: Duration) -> Vec<KillHandle> {
        let kills = &self.kills;
        let busy = |p: &Parkable| kills.values().any(|k| std::sync::Arc::ptr_eq(&k.0, &p.kill.0));
        let mut due = Vec::new();
        for p in &mut self.parkable {
            if !p.parked && now.duration_since(p.idle_since) >= after && !busy(p) {
                p.parked = true;
                due.push(p.kill.clone());
            }
        }
        due
    }

    /// A request to `kill`'s child ended: its idle clock starts again.
    fn request_done(&mut self, kill: &KillHandle, now: Instant) {
        for p in self.parkable.iter_mut().filter(|p| std::sync::Arc::ptr_eq(&p.kill.0, &kill.0)) {
            p.idle_since = now;
            p.parked = false;
        }
    }
}

struct Shared {
    state: Mutex<Watch>,
    wake: Condvar,
    thread: Mutex<Option<JoinHandle<()>>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();
static THREAD_STARTED: OnceLock<bool> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn shared() -> &'static Shared {
    SHARED.get_or_init(|| Shared { state: Mutex::new(Watch::default()), wake: Condvar::new(), thread: Mutex::new(None) })
}

fn lock() -> MutexGuard<'static, Watch> {
    shared().state.lock().unwrap_or_else(|e| e.into_inner())
}

/// Start the watchdog thread once; whether it is running.
fn ensure_thread() -> bool {
    *THREAD_STARTED.get_or_init(|| {
        match std::thread::Builder::new().name("v2-watchdog".into()).spawn(watch) {
            Ok(handle) => {
                *shared().thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
                true
            }
            Err(_) => false,
        }
    })
}

/// How long a worker stays idle before it parks. Debug builds take `V2_TEST_PARK_MS` from the
/// tests, like `V2_TEST_PANIC` (22.2).
fn park_after() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("V2_TEST_PARK_MS").ok().and_then(|v| v.parse().ok()) {
        return Duration::from_millis(ms);
    }
    PARK_AFTER
}

fn watch() {
    let s = shared();
    let mut w = lock();
    let mut last_exit_check = Instant::now();
    let park = park_after();
    // When the watchdog last went to sleep, and for how long.
    let mut asked: Option<(Instant, Duration)> = None;
    loop {
        if w.stop {
            return;
        }
        if interrupted() {
            die(&w);
        }
        #[cfg(unix)]
        if SUSPEND.swap(false, Ordering::SeqCst) {
            suspend(&mut w);
        }
        let now = Instant::now();
        // Woken long after it asked (SIGSTOP, a computer that slept): no call ran meanwhile.
        if let Some((slept, wait)) = asked.take() {
            let late = now.saturating_duration_since(slept).saturating_sub(wait);
            if late >= SUSPENDED {
                w.timers.skip(late, now);
            }
        }
        if let Some(id) = w.timers.expired(now) {
            if let Some(kill) = w.kills.get(&id) {
                kill.kill_tree();
            }
            w.timers.mark_fired(id);
            w.fired.insert(id);
        }
        // A descendant holding the stdout pipe must not keep a read from reaching EOF (8.5).
        if now.duration_since(last_exit_check) >= EXIT_CHECK {
            last_exit_check = now;
            if let Some(kill) = w.timers.top_id().and_then(|id| w.kills.get(&id)) {
                if kill.has_exited() {
                    kill.kill_tree();
                }
            }
        }
        // On a thread of its own: a trim takes about 40 ms per GB, and a timer can't wait (8.8).
        let due = w.due_to_park(now, park);
        if !due.is_empty() {
            let _ = std::thread::Builder::new().name("v2-park".into()).spawn(move || due.iter().for_each(KillHandle::release_memory));
        }
        let mut wait = w.timers.time_left(now).map_or(TICK, |left| left.min(TICK));
        if w.timers.top_id().is_some() {
            wait = wait.min(EXIT_CHECK.saturating_sub(now.duration_since(last_exit_check)));
        }
        asked = Some((Instant::now(), wait));
        w = match s.wake.wait_timeout(w, wait) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
}

fn wake() {
    shared().wake.notify_all();
}

/// Ctrl+Z: every child's group stops, then v2 itself, as a shell's job would; when the job goes
/// on (fg, bg, SIGCONT), so do they, and the calls' clocks skip the time stopped (6.5, 8.5). Around
/// the stop, `input(echo: false)` gives the terminal its echo back (5.10).
#[cfg(unix)]
fn suspend(w: &mut Watch) {
    for (_, kill) in &w.children {
        kill.signal_group(libc::SIGSTOP);
    }
    let stopped = Instant::now();
    // SAFETY: a plain signal to this process; it stops every thread until SIGCONT.
    crate::runtime::builtins::echo_around_stop(|| unsafe {
        libc::raise(libc::SIGSTOP);
    });
    for (_, kill) in &w.children {
        kill.signal_group(libc::SIGCONT);
    }
    let now = Instant::now();
    w.timers.skip(now.saturating_duration_since(stopped), now);
}

/// Handle to the watchdog, held by main for the whole run.
pub struct Watchdog;

impl Watchdog {
    /// Start the thread and, if it runs, the interrupt handler: without a watchdog nothing
    /// would end the run, so the OS default stays (6.5).
    pub fn start() -> Watchdog {
        default_sigchld();
        #[cfg(unix)]
        remember_terminal();
        #[cfg(windows)]
        console_vt_on();
        if ensure_thread() {
            install_interrupt_handler();
        }
        Watchdog
    }

    /// Ask the thread to finish and wait for it.
    pub fn stop(self) {
        lock().stop = true;
        wake();
        let handle = shared().thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

/// A running deadline; dropping it removes the timer (8.5).
pub struct TimerGuard {
    id: u64,
}

/// Time the work of `kill`'s child: past `limit` (`None`: no limit) the watchdog kills its tree.
pub fn push_timer(limit: Option<Duration>, kill: KillHandle) -> TimerGuard {
    ensure_thread();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    {
        let mut w = lock();
        w.timers.push(id, limit, Instant::now());
        w.kills.insert(id, kill);
    }
    wake();
    TimerGuard { id }
}

impl TimerGuard {
    /// Stop counting, e.g. while V2 serves a call from the timed worker (8.5).
    pub fn pause(&self) {
        lock().timers.pause(self.id, Instant::now());
    }

    pub fn resume(&self) {
        lock().timers.resume(self.id, Instant::now());
        wake();
    }

    /// Whether the watchdog killed the child because this timer expired.
    pub fn fired(&self) -> bool {
        lock().fired.contains(&self.id)
    }
}

impl Drop for TimerGuard {
    fn drop(&mut self) {
        {
            let mut w = lock();
            let now = Instant::now();
            w.timers.pop(self.id, now);
            if let Some(kill) = w.kills.remove(&self.id) {
                w.request_done(&kill, now);
            }
            w.fired.remove(&self.id);
        }
        wake();
    }
}

/// Let `kill`'s child park when idle (8.8). It counts as idle from now.
pub fn allow_parking(kill: KillHandle) {
    ensure_thread();
    lock().parkable.push(Parkable { kill, idle_since: Instant::now(), parked: false });
}

/// The private run directory, removed on the interrupt path too (20.3).
pub fn set_run_dir(dir: PathBuf) {
    lock().run_dir = Some(dir);
}

/// Forget a child that is about to be reaped: its interrupt-path entry and every timer that
/// would kill it, even while a caller still holds the TimerGuard, so a reused pid is never
/// signalled or waited on (8.5).
fn unregister(id: u64, kill: &KillHandle) {
    let mut w = lock();
    w.children.retain(|(c, _)| *c != id);
    w.parkable.retain(|p| !std::sync::Arc::ptr_eq(&p.kill.0, &kill.0));
    let timers: Vec<u64> = w.kills.iter().filter(|(_, k)| std::sync::Arc::ptr_eq(&k.0, &kill.0)).map(|(t, _)| *t).collect();
    for t in timers {
        w.kills.remove(&t);
        w.timers.pop(t, Instant::now());
    }
}

// ---------------------------------------------------------------------------------------------
// Child processes (8.3)

/// What a child's standard stream is connected to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pipe {
    Null,
    Piped,
    Inherit,
    /// The host's stderr (for a child's stdout).
    Stderr,
}

impl Pipe {
    fn stdio(self) -> Stdio {
        match self {
            Pipe::Null => Stdio::null(),
            Pipe::Piped => Stdio::piped(),
            Pipe::Inherit => Stdio::inherit(),
            Pipe::Stderr => {
                #[cfg(unix)]
                let copy = std::os::fd::AsFd::as_fd(&std::io::stderr()).try_clone_to_owned().map(Stdio::from);
                #[cfg(windows)]
                let copy = std::os::windows::io::AsHandle::as_handle(&std::io::stderr()).try_clone_to_owned().map(Stdio::from);
                copy.unwrap_or_else(|_| Stdio::null())
            }
        }
    }
}

/// What to start: `argv[0]` is an absolute path (7.5); `env` is added to the host's.
pub struct Spawn<'a> {
    pub argv: &'a [OsString],
    pub env: &'a [(OsString, OsString)],
    pub stdin: Pipe,
    pub stdout: Pipe,
    pub stderr: Pipe,
    /// Inherited variables the child doesn't get (a run engine's `V2_ARG_*`, 17.4).
    pub remove: &'a [OsString],
    /// Unix: descriptors the child keeps, at their own numbers (a worker's protocol, 9.9).
    pub keep: &'a [i32],
    /// Windows: the last argument goes on the command line as written, not quoted, as cmd.exe
    /// reads the command after `/c` (`exec`, 5.10).
    pub raw_tail: bool,
}

/// How a child ended: an exit code, or (Unix) the signal that killed it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

impl ExitInfo {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// `exit code 3`, `exit code 3221225477 = 0xC0000005`, `killed by signal 11 (SIGSEGV)` (8.6).
    pub fn describe(&self) -> String {
        match (self.code, self.signal) {
            (_, Some(sig)) => match signal_name(sig) {
                Some(name) => format!("killed by signal {sig} ({name})"),
                None => format!("killed by signal {sig}"),
            },
            (Some(code), None) if cfg!(windows) && code as u32 >= 0x8000_0000 => {
                format!("exit code {} = 0x{:08X}", code as u32, code as u32)
            }
            (Some(code), None) => format!("exit code {code}"),
            (None, None) => "an unknown exit status".into(),
        }
    }

    fn from_status(status: std::process::ExitStatus) -> ExitInfo {
        #[cfg(unix)]
        let signal = std::os::unix::process::ExitStatusExt::signal(&status);
        #[cfg(not(unix))]
        let signal = None;
        ExitInfo { code: status.code(), signal }
    }
}

/// The name of a signal, as `kill -l` spells it (8.6).
#[cfg(unix)]
pub(crate) fn signal_name(sig: i32) -> Option<String> {
    let known = [
        (libc::SIGHUP, "SIGHUP"),
        (libc::SIGINT, "SIGINT"),
        (libc::SIGQUIT, "SIGQUIT"),
        (libc::SIGILL, "SIGILL"),
        (libc::SIGTRAP, "SIGTRAP"),
        (libc::SIGABRT, "SIGABRT"),
        (libc::SIGBUS, "SIGBUS"),
        (libc::SIGFPE, "SIGFPE"),
        (libc::SIGKILL, "SIGKILL"),
        (libc::SIGUSR1, "SIGUSR1"),
        (libc::SIGSEGV, "SIGSEGV"),
        (libc::SIGUSR2, "SIGUSR2"),
        (libc::SIGPIPE, "SIGPIPE"),
        (libc::SIGALRM, "SIGALRM"),
        (libc::SIGTERM, "SIGTERM"),
        (libc::SIGCHLD, "SIGCHLD"),
        (libc::SIGCONT, "SIGCONT"),
        (libc::SIGSTOP, "SIGSTOP"),
        (libc::SIGTSTP, "SIGTSTP"),
        (libc::SIGTTIN, "SIGTTIN"),
        (libc::SIGTTOU, "SIGTTOU"),
        (libc::SIGURG, "SIGURG"),
        (libc::SIGXCPU, "SIGXCPU"),
        (libc::SIGXFSZ, "SIGXFSZ"),
        (libc::SIGVTALRM, "SIGVTALRM"),
        (libc::SIGPROF, "SIGPROF"),
        (libc::SIGWINCH, "SIGWINCH"),
        (libc::SIGIO, "SIGIO"),
        (libc::SIGSYS, "SIGSYS"),
        #[cfg(target_os = "linux")]
        (libc::SIGSTKFLT, "SIGSTKFLT"),
        #[cfg(target_os = "linux")]
        (libc::SIGPWR, "SIGPWR"),
    ];
    if let Some((_, name)) = known.iter().find(|(n, _)| *n == sig) {
        return Some(name.to_string());
    }
    #[cfg(target_os = "linux")]
    {
        let (min, max) = (libc::SIGRTMIN(), libc::SIGRTMAX());
        if sig == min {
            return Some("SIGRTMIN".into());
        }
        if sig > min && sig <= max {
            return Some(format!("SIGRTMIN+{}", sig - min));
        }
    }
    None
}

#[cfg(not(unix))]
pub(crate) fn signal_name(_: i32) -> Option<String> {
    None
}

/// V2 needs its children's exit statuses, so an inherited `SIGCHLD = SIG_IGN` (which makes the
/// kernel reap them at once) is put back to the default (8.3, 8.6). And a write past the file
/// size limit (`ulimit -f`) fails with EFBIG, an IOError, instead of killing V2 with SIGXFSZ, as
/// Python does; children get the default back (`platform_spawn`).
#[cfg(unix)]
fn default_sigchld() {
    // SAFETY: a zeroed, filled struct for `sigaction`.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = libc::SIG_DFL;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGCHLD, &action, std::ptr::null_mut());
        action.sa_sigaction = libc::SIG_IGN;
        libc::sigaction(libc::SIGXFSZ, &action, std::ptr::null_mut());
    }
}

#[cfg(not(unix))]
fn default_sigchld() {}

/// What kills a child's tree, shared by the child and the watchdog (8.3, 8.5). Windows: the
/// child's Job Object and a duplicated process handle; Unix: the session leader's pid, which
/// is its process group id.
#[derive(Clone)]
pub struct KillHandle(std::sync::Arc<KillInner>);

#[cfg(windows)]
struct KillInner {
    job: windows_sys::Win32::Foundation::HANDLE,
    process: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: the handles are owned by this struct, used only through thread-safe Win32 calls and
// closed exactly once, on drop.
#[cfg(windows)]
unsafe impl Send for KillInner {}
#[cfg(windows)]
unsafe impl Sync for KillInner {}

#[cfg(windows)]
impl Drop for KillInner {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        // SAFETY: both handles are owned and still open; closing the job kills what is left
        // in it (KILL_ON_JOB_CLOSE).
        unsafe {
            CloseHandle(self.process);
            CloseHandle(self.job);
        }
    }
}

#[cfg(unix)]
struct KillInner {
    pid: libc::pid_t,
}

impl KillHandle {
    /// Unix: send `sig` to the child's process group, else to the child (6.5).
    #[cfg(unix)]
    fn signal_group(&self, sig: libc::c_int) {
        // SAFETY: plain signal calls; the leader is not reaped yet, so its pgid is not reused.
        unsafe {
            let pid = self.0.pid;
            if libc::kill(-pid, sig) == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                libc::kill(pid, sig);
            }
        }
    }

    /// Kill the child and everything it started that did not detach (8.3).
    pub fn kill_tree(&self) {
        #[cfg(windows)]
        // SAFETY: the job handle is open for as long as `self` lives.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0.job, 1);
        }
        #[cfg(unix)]
        // SAFETY: plain signal calls; the leader is not reaped yet, so its pgid is not reused.
        unsafe {
            let pid = self.0.pid;
            if libc::kill(-pid, libc::SIGKILL) == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        // One that only left the group (`setpgrp`, to keep Ctrl+C away) is still in the session.
        #[cfg(target_os = "linux")]
        kill_session(self.0.pid);
    }

    /// Wait until every process of the tree has stopped, or `deadline`. Windows only: a
    /// terminated job's processes take a moment to go and hold their files until then.
    fn wait_stopped(&self, deadline: Instant) {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::JobObjects::{
                JobObjectBasicAccountingInformation, QueryInformationJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
            };
            loop {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
                // SAFETY: the job handle is open; `info` is sized for this information class.
                let ok = unsafe {
                    QueryInformationJobObject(
                        self.0.job,
                        JobObjectBasicAccountingInformation,
                        &mut info as *mut _ as *mut core::ffi::c_void,
                        std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 || info.ActiveProcesses == 0 || Instant::now() >= deadline {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        #[cfg(not(windows))]
        let _ = deadline;
    }

    /// Whether the child itself has exited (not reaped; 8.3).
    fn has_exited(&self) -> bool {
        self.exit_info(false).is_some()
    }

    /// Move the pages of every process in the job out of RAM; they fault back in on use (8.8). A
    /// process may have gone, so errors are ignored.
    #[cfg(windows)]
    fn release_memory(&self) {
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_MORE_DATA};
        use windows_sys::Win32::System::JobObjects::{JobObjectBasicProcessIdList, QueryInformationJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST};
        use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_SET_QUOTA};
        // Room for far more processes than a worker has. A longer list comes back cut off, with
        // ERROR_MORE_DATA, which is fine: the worker itself is at its start.
        let mut buf = [0usize; 256];
        let size = std::mem::size_of_val(&buf) as u32;
        // SAFETY: the job handle is open; `buf` is usize-aligned and sized for this information class.
        let ok = unsafe { QueryInformationJobObject(self.0.job, JobObjectBasicProcessIdList, buf.as_mut_ptr().cast(), size, std::ptr::null_mut()) };
        // SAFETY: a plain query.
        if ok == 0 && unsafe { GetLastError() } != ERROR_MORE_DATA {
            return;
        }
        // SAFETY: the kernel filled `buf` as this struct: two u32 counts, then the ids.
        let list = unsafe { &*(buf.as_ptr() as *const JOBOBJECT_BASIC_PROCESS_ID_LIST) };
        let start = std::mem::offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList) / std::mem::size_of::<usize>();
        for &pid in buf[start..].iter().take(list.NumberOfProcessIdsInList as usize) {
            // SAFETY: a plain open of a process id; the handle is closed below.
            unsafe {
                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_QUERY_INFORMATION, 0, pid as u32);
                if !process.is_null() {
                    K32EmptyWorkingSet(process);
                    CloseHandle(process);
                }
            }
        }
    }

    /// Nothing to do: a normal user can't trim a child, and the kernel reclaims idle pages (8.8).
    #[cfg(not(windows))]
    fn release_memory(&self) {}

    /// The child's exit, waiting for it when `block`; never reaps it.
    fn exit_info(&self, block: bool) -> Option<ExitInfo> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
            use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
            // SAFETY: the process handle is open for as long as `self` lives.
            unsafe {
                if WaitForSingleObject(self.0.process, if block { INFINITE } else { 0 }) != WAIT_OBJECT_0 {
                    return None;
                }
                let mut code = 0u32;
                GetExitCodeProcess(self.0.process, &mut code);
                Some(ExitInfo { code: Some(code as i32), signal: None })
            }
        }
        #[cfg(unix)]
        {
            let flags = libc::WEXITED | libc::WNOWAIT | if block { 0 } else { libc::WNOHANG };
            loop {
                // SAFETY: `info` is a zeroed siginfo_t that waitid fills in.
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                let r = unsafe { libc::waitid(libc::P_PID, self.0.pid as libc::id_t, &mut info, flags) };
                if r == -1 {
                    if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                        continue;
                    }
                    return None;
                }
                let (pid, status) = siginfo_pid_status(&info);
                if pid == 0 {
                    return None;
                }
                return match info.si_code {
                    libc::CLD_EXITED => Some(ExitInfo { code: Some(status), signal: None }),
                    libc::CLD_KILLED | libc::CLD_DUMPED => Some(ExitInfo { code: None, signal: Some(status) }),
                    // macOS reports a stop or a continue (Ctrl+Z, fg) too, unasked: not an exit.
                    // Waiting, look again in a moment (WNOWAIT leaves the report there).
                    _ if block => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    _ => None,
                };
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn siginfo_pid_status(info: &libc::siginfo_t) -> (libc::pid_t, i32) {
    // SAFETY: waitid filled a SIGCHLD-shaped siginfo_t.
    unsafe { (info.si_pid(), info.si_status()) }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn siginfo_pid_status(info: &libc::siginfo_t) -> (libc::pid_t, i32) {
    (info.si_pid, info.si_status)
}

/// A child process started under 8.3's rules. Dropping it kills and reaps its tree.
pub struct ChildProc {
    child: std::process::Child,
    kill: KillHandle,
    reg: u64,
    done: bool,
}

/// A child that could not start.
#[derive(Debug)]
pub struct SpawnError {
    /// `could not start 'C:\...\python.exe': <reason>` (8.3).
    pub message: String,
    /// The OS message alone, which `exec` words its own way (5.10).
    pub reason: String,
    /// The OS's error code, when the OS refused the start.
    pub code: Option<i32>,
    /// Windows: the shorter path the program was started by, its own being too long
    /// (`short_program`).
    pub short: Option<OsString>,
}

/// Windows messages such as `%1 is not a valid Win32 application` come with their `%1`
/// unfilled. `program` is the path as given, also when it started by its `short` one.
fn spawn_error(program: &OsString, e: &std::io::Error, short: Option<OsString>) -> SpawnError {
    let mut reason = crate::syntax::span::os_message(e).replace("%1", "the program");
    if cfg!(windows) && e.raw_os_error() == Some(193) {
        reason.push_str(" (not a valid program for this machine)");
    }
    SpawnError { message: format!("could not start '{}': {reason}", program.to_string_lossy()), reason, code: e.raw_os_error(), short }
}

/// Windows: the length of `program`'s path in UTF-16 units, without a `\\?\` in front, when it
/// is over 259, the most V2 starts a program from: CreateProcessW takes more only from a program
/// marked long-path aware, which v2.exe isn't (5.10). A batch file's limit is 246: from 247 Rust
/// gives cmd.exe its path with `\\?\` in front, which cmd.exe can't run ("The system cannot find
/// the path specified").
#[cfg(windows)]
pub fn long_program(program: &std::ffi::OsStr) -> Option<usize> {
    let full = std::path::absolute(program).unwrap_or_else(|_| program.into());
    let shown = full.to_string_lossy();
    let plain = shown.strip_prefix(r"\\?\UNC\").map(|rest| format!(r"\\{rest}")).unwrap_or_else(|| shown.strip_prefix(r"\\?\").unwrap_or(&shown).to_string());
    let ext = full.extension().map(|e| e.to_ascii_lowercase());
    let limit = if ext.as_deref() == Some("bat".as_ref()) || ext.as_deref() == Some("cmd".as_ref()) { 246 } else { 259 };
    Some(plain.encode_utf16().count()).filter(|&n| n > limit)
}

/// Windows: a program whose path is too long to start from (`long_program`) by a shorter one
/// (5.10): its folder's 8.3 path and its own name, which a multi-call program (or clang's driver)
/// reads, when that is at most 259 units; else its whole 8.3 path, when that is.
#[cfg(windows)]
fn short_program(program: &std::ffi::OsStr) -> Option<OsString> {
    long_program(program)?;
    let full = std::path::absolute(program).ok()?;
    let fits = |p: OsString| long_program(&p).is_none().then_some(p);
    let named = short_path(full.parent()?.as_os_str()).map(|folder| std::path::Path::new(&folder).join(full.file_name().unwrap_or_default()).into_os_string());
    named.and_then(fits).or_else(|| short_path(full.as_os_str()).and_then(fits))
}

#[cfg(not(windows))]
fn short_program(_: &std::ffi::OsStr) -> Option<OsString> {
    None
}

/// Windows: `path` with each part by its 8.3 name where the volume has one (`GetShortPathNameW`,
/// which reaches a long path by its `\\?\` form), without a `\\?\` in front.
#[cfg(windows)]
fn short_path(path: &std::ffi::OsStr) -> Option<OsString> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let text = path.to_string_lossy();
    let verbatim = if text.starts_with(r"\\?\") {
        text.into_owned()
    } else if let Some(share) = text.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{share}")
    } else {
        format!(r"\\?\{text}")
    };
    let wide: Vec<u16> = std::ffi::OsStr::new(&verbatim).encode_wide().chain([0]).collect();
    // SAFETY: `wide` is NUL-terminated; a null buffer of size 0 asks for the size.
    let n = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    let mut short = vec![0u16; usize::try_from(n).ok().filter(|&n| n > 0)?];
    // SAFETY: `short` holds the `n` units the first call asked for.
    let m = unsafe { GetShortPathNameW(wide.as_ptr(), short.as_mut_ptr(), n) };
    short.truncate(usize::try_from(m).ok().filter(|&m| m > 0 && m < short.len())?);
    let short = String::from_utf16(&short).ok()?;
    Some(OsString::from(match short.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => short.strip_prefix(r"\\?\").unwrap_or(&short).to_string(),
    }))
}

/// Windows: the length in UTF-16 units of the command line `ChildProc::spawn_as` makes of
/// `program` (the path it starts) and `args` (8.3), of which CreateProcessW takes at most 32,766
/// (32,767 with its closing NUL).
#[cfg(any(windows, test))]
pub fn command_line_units(program: &std::ffi::OsStr, args: &[OsString], raw_tail: bool) -> usize {
    let units = |a: &std::ffi::OsStr| a.to_string_lossy().encode_utf16().collect::<Vec<u16>>();
    let path = std::path::Path::new(program);
    let ext = path.extension().map(|e| e.to_ascii_lowercase());
    let batch = ext.as_deref() == Some("bat".as_ref()) || ext.as_deref() == Some("cmd".as_ref());
    let cmd_exe = path.file_name().is_some_and(|f| f.eq_ignore_ascii_case("cmd.exe"));
    let is = |u: u16, c: u8| u == u16::from(c);
    // Rust's for cmd.exe: quotes around one with a space or a tab (or an empty one), a backslash
    // before each `"` and the backslashes before it, and before the closing quote.
    let rust = |a: &[u16]| {
        let quote = a.is_empty() || a.iter().any(|&u| is(u, b' ') || is(u, b'\t'));
        let (mut n, mut run) = (a.len() + if quote { 2 } else { 0 }, 0);
        for &u in a {
            if is(u, b'\\') {
                run += 1;
            } else {
                if is(u, b'"') {
                    n += run + 1;
                }
                run = 0;
            }
        }
        n + if quote { run } else { 0 }
    };
    // Rust's for a batch file: quotes around one with an ASCII symbol but `#$*+-./:?@\_`, a
    // control character, or none at all or a `\` at its end; `"` doubled after the backslashes
    // before it, `%` as `%%cd:~,%`, and the backslashes before the closing quote doubled.
    let bat = |a: &[u16]| {
        let symbol = |u: u16| u8::try_from(u).is_ok_and(|b| b.is_ascii() && !b.is_ascii_alphanumeric() && !b"#$*+-./:?@\\_".contains(&b));
        let control = |u: u16| char::from_u32(u32::from(u)).is_some_and(char::is_control);
        let quote = a.is_empty() || a.last().is_some_and(|&u| is(u, b'\\')) || a.iter().any(|&u| symbol(u) || control(u));
        let (mut n, mut run) = (a.len() + if quote { 2 } else { 0 }, 0);
        for &u in a {
            if is(u, b'\\') {
                run += 1;
            } else {
                if is(u, b'"') {
                    n += run + 1;
                } else if is(u, b'%') || is(u, b'\r') {
                    n += "%%cd:~,".len();
                }
                run = 0;
            }
        }
        n + if quote { run } else { 0 }
    };
    let last = args.len().saturating_sub(1);
    let line: usize = args
        .iter()
        .enumerate()
        .map(|(i, a)| {
            1 + if raw_tail && i == last && args.len() > 1 {
                units(a).len()
            } else if batch {
                bat(&units(a))
            } else if cmd_exe || raw_tail {
                rust(&units(a))
            } else {
                command_line_arg(&units(a)).len()
            }
        })
        .sum();
    // A batch file runs as `cmd.exe /e:ON /v:OFF /d /c ""script" args"`, its path as Rust's
    // `to_user_path` gives it (`batch_script_units`).
    let (shell, script) = if batch { ("cmd.exe /e:ON /v:OFF /d /c \"\"".len(), batch_script_units(&units(program))) } else { (0, units(program).len()) };
    shell + script + 2 + line
}

/// How long Rust makes a batch file's path on cmd.exe's command line (its `to_user_path`): one of
/// 247 to 259 units gets `\\?\` in front (`\\?\UNC\` for a share's), as Rust's `get_long_path`
/// adds it from 248 with the NUL; a `\\?\` one of at most 259 loses it; a longer one stays.
#[cfg(any(windows, test))]
fn batch_script_units(path: &[u16]) -> usize {
    let starts = |s: &str| s.encode_utf16().enumerate().all(|(i, u)| path.get(i) == Some(&u));
    match path.len() {
        n if n > 259 => n,
        n if starts(r"\\?\UNC\") => n - 6,
        n if starts(r"\\?\") => n - 4,
        n if n < 247 || starts(r"\\.\") => n,
        n if starts(r"\\") => n + 6,
        n => n + 4,
    }
}

/// Windows: one argument as the command line carries it, so that both kinds of program read it
/// back as given: a C runtime's (MSVCRT's rules, also Rust's, Python's, Node's) and Git Bash's
/// MSYS runtime (`find`, `grep`, `sh`), which globs, brace- and `~`-expands, reads `@file` and
/// takes `'` as a quote in a word it sees unquoted (8.3). A word with none of those characters
/// goes as it is; any other is quoted, `"` as `\"` and the backslashes before a quote, or before
/// the closing one, doubled. MSYS reads `\\` in quotes as one backslash, where the C runtime keeps
/// both, so such a run ends the quotes (doubled, as before a quote) and the word goes on unquoted
/// up to its next character that needs them. Only a run of two or more right before a space or
/// tab, a `'` or one of `* ? [ {` reaches an MSYS program halved.
#[cfg(any(windows, test))]
fn command_line_arg(arg: &[u16]) -> Vec<u16> {
    let (quote, backslash) = (u16::from(b'"'), u16::from(b'\\'));
    let ascii = |u: u16, set: &[u8]| u < 0x80 && set.contains(&(u as u8));
    // What has a meaning outside quotes: to MSYS, and spaces to both.
    let special = |u: u16| u <= 0x20 || u == 0x7f || ascii(u, b"\"'*?[{");
    // A word with these is globbed too, which would read its backslashes as escapes.
    if arg.first().is_some_and(|&u| !ascii(u, b"~@")) && arg.iter().all(|&u| !special(u) && !ascii(u, b"]}()")) {
        return arg.to_vec();
    }
    let mut out = vec![quote];
    let mut inside = true;
    let mut i = 0;
    while i < arg.len() {
        let run = arg[i..].iter().take_while(|&&u| u == backslash).count();
        let next = arg.get(i + run).copied();
        if !inside && (run > 0 || next.is_some_and(special)) {
            out.push(quote);
            inside = true;
        }
        match next {
            Some(q) if q == quote => out.extend(std::iter::repeat_n(backslash, 2 * run + 1)),
            None => out.extend(std::iter::repeat_n(backslash, 2 * run)),
            Some(c) if run >= 2 && !special(c) => {
                out.extend(std::iter::repeat_n(backslash, 2 * run));
                out.push(quote);
                inside = false;
            }
            Some(_) => out.extend(std::iter::repeat_n(backslash, run)),
        }
        out.extend(next);
        i += run + 1;
    }
    if inside {
        out.push(quote);
    }
    out
}

impl ChildProc {
    pub fn spawn(s: &Spawn) -> Result<ChildProc, SpawnError> {
        Self::spawn_as(s, None)
    }

    /// `spawn`, and on Unix with `arg0` as the program's `argv[0]`: it runs by its absolute path
    /// (7.5), and sees the name it was called by, as a shell passes it (`exec`, 5.10).
    pub fn spawn_as(s: &Spawn, arg0: Option<&std::ffi::OsStr>) -> Result<ChildProc, SpawnError> {
        stop_if_interrupted();
        let program = &s.argv[0];
        // V2 starts no program from a path over 259 characters: by a shorter one, when the
        // volume has 8.3 names that short (5.10). Messages name the path as given.
        let short = short_program(program);
        let started = short.as_ref().unwrap_or(program);
        // Windows runs a batch file through cmd.exe, which would expand `%NAME%` in its path
        // (Rust escapes only the arguments) and run something else (7.5).
        let ext = std::path::Path::new(program).extension().map(|e| e.to_ascii_lowercase());
        let batch = cfg!(windows) && (ext.as_deref() == Some("bat".as_ref()) || ext.as_deref() == Some("cmd".as_ref()));
        if batch && started.to_string_lossy().contains('%') {
            let reason = "a batch file whose path contains '%' can't be started safely: move it to a folder whose path has no %, or start the program it runs directly".to_string();
            return Err(SpawnError { message: format!("could not start '{}': {reason}", program.to_string_lossy()), reason, code: None, short: None });
        }
        // Rust refuses them ("batch file arguments are invalid"): cmd.exe would end the command
        // line there.
        if batch && s.argv[1..].iter().any(|a| a.as_encoded_bytes().iter().any(|b| matches!(b, b'\n' | b'\r'))) {
            let reason = "a batch file can't take an argument with a line break: pass the text as input, as in exec(argv, input: text)".to_string();
            return Err(SpawnError { message: format!("could not start '{}': {reason}", program.to_string_lossy()), reason, code: None, short: None });
        }
        let mut cmd = Command::new(started);
        // `ls: cannot access …` as from a shell, not `/usr/bin/ls: …`; Windows's command line
        // starts with the path either way.
        #[cfg(unix)]
        if let Some(name) = arg0 {
            std::os::unix::process::CommandExt::arg0(&mut cmd, name);
        }
        #[cfg(windows)]
        let _ = arg0;
        // Windows: a program parses its own command line. cmd.exe and batch files keep Rust's
        // quoting, which is cmd's; every other program gets `command_line_arg`'s (8.3).
        #[cfg(windows)]
        let cmd_exe = std::path::Path::new(program).file_name().is_some_and(|f| f.eq_ignore_ascii_case("cmd.exe"));
        match s.argv.split_last() {
            #[cfg(windows)]
            Some((last, rest)) if s.raw_tail && rest.len() > 1 => {
                std::os::windows::process::CommandExt::raw_arg(cmd.args(&rest[1..]), last);
            }
            #[cfg(windows)]
            Some(_) if !batch && !cmd_exe => {
                use std::os::windows::ffi::{OsStrExt, OsStringExt};
                for arg in &s.argv[1..] {
                    let wide: Vec<u16> = arg.encode_wide().collect();
                    std::os::windows::process::CommandExt::raw_arg(&mut cmd, OsString::from_wide(&command_line_arg(&wide)));
                }
            }
            _ => {
                cmd.args(&s.argv[1..]);
            }
        }
        for k in s.remove {
            cmd.env_remove(k);
        }
        cmd.envs(s.env.iter().map(|(k, v)| (k, v)));
        cmd.stdin(s.stdin.stdio()).stdout(s.stdout.stdio()).stderr(s.stderr.stdio());
        // Under the watchdog's lock: the interrupt path can't run between the spawn and the
        // registration, so it never misses a child (6.5).
        let mut w = lock();
        let (child, kill) = platform_spawn(&mut cmd, s.keep).map_err(|e| spawn_error(program, &e, short.clone()))?;
        let reg = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        w.children.push((reg, kill.clone()));
        drop(w);
        Ok(ChildProc { child, kill, reg, done: false })
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn kill_handle(&self) -> KillHandle {
        self.kill.clone()
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    /// The exit, if the child has exited; it is not reaped (8.3).
    pub fn exited(&self) -> Option<ExitInfo> {
        self.kill.exit_info(false)
    }

    /// Wait for the child to exit without reaping it.
    pub fn wait_exit(&self) -> Option<ExitInfo> {
        self.kill.exit_info(true)
    }

    pub fn kill_tree(&self) {
        self.kill.kill_tree();
    }

    /// Kill what is left of the tree, unregister it and its timers, then reap (8.3, 8.5).
    pub fn reap(mut self) -> ExitInfo {
        self.finish()
    }

    fn finish(&mut self) -> ExitInfo {
        self.done = true;
        self.kill.kill_tree();
        unregister(self.reg, &self.kill);
        match self.child.wait() {
            Ok(status) => ExitInfo::from_status(status),
            Err(_) => ExitInfo { code: None, signal: None },
        }
    }
}

impl Drop for ChildProc {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.finish();
        }
    }
}

/// Unix: the child leads a new session (no controlling terminal), and on Linux dies with the
/// spawning thread (8.3).
#[cfg(unix)]
fn platform_spawn(cmd: &mut Command, keep: &[i32]) -> std::io::Result<(std::process::Child, KillHandle)> {
    use std::os::unix::process::CommandExt;
    #[cfg(target_os = "linux")]
    let host = std::process::id() as libc::pid_t;
    let keep = keep.to_vec();
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            // V2 ignores SIGXFSZ; the programs it starts behave as they would anywhere.
            libc::signal(libc::SIGXFSZ, libc::SIG_DFL);
            for &fd in &keep {
                if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            #[cfg(target_os = "linux")]
            {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != host {
                    libc::_exit(1);
                }
            }
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    let pid = child.id() as libc::pid_t;
    Ok((child, KillHandle(std::sync::Arc::new(KillInner { pid }))))
}

/// Windows: created suspended in a new process group, put in its own kill-on-close Job Object,
/// then resumed, so every descendant is in the job (8.3).
#[cfg(windows)]
fn platform_spawn(cmd: &mut Command, _keep: &[i32]) -> std::io::Result<(std::process::Child, KillHandle)> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::Foundation::{CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE};
    use windows_sys::Win32::System::Console::GetConsoleCP;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, TerminateProcess, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED,
    };

    let mut flags = CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP;
    // Only a host with no console at all: children share any console the host has, even one
    // without a window (8.3). SAFETY: a plain query; 0 means there is no console.
    if unsafe { GetConsoleCP() } == 0 {
        flags |= CREATE_NO_WINDOW;
    }
    cmd.creation_flags(flags);
    let mut child = cmd.spawn()?;
    let fail = |child: &mut std::process::Child, e: std::io::Error| -> std::io::Error {
        // SAFETY: the child's own handle, still open.
        unsafe { TerminateProcess(child.as_raw_handle() as HANDLE, 1) };
        let _ = child.wait();
        e
    };
    // SAFETY: creating and configuring a job we own; the struct is zeroed then filled.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        let e = std::io::Error::last_os_error();
        return Err(fail(&mut child, e));
    }
    let close_job = |job: HANDLE| unsafe {
        CloseHandle(job);
    };
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    let ok = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) != 0
            && AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) != 0
    };
    if !ok {
        let e = std::io::Error::last_os_error();
        close_job(job);
        return Err(fail(&mut child, e));
    }
    let mut process: HANDLE = std::ptr::null_mut();
    // SAFETY: duplicating the child's handle into our own process.
    let dup = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            child.as_raw_handle() as HANDLE,
            GetCurrentProcess(),
            &mut process,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if dup == 0 {
        let e = std::io::Error::last_os_error();
        close_job(job);
        return Err(fail(&mut child, e));
    }
    let kill = KillHandle(std::sync::Arc::new(KillInner { job, process }));
    #[link(name = "ntdll")]
    extern "system" {
        fn NtResumeProcess(process: HANDLE) -> i32;
    }
    // SAFETY: the child's own handle, still open.
    let status = unsafe { NtResumeProcess(child.as_raw_handle() as HANDLE) };
    if status < 0 {
        let e = std::io::Error::other(format!("NtResumeProcess failed (0x{status:08X})"));
        kill.kill_tree();
        return Err(fail(&mut child, e));
    }
    Ok((child, kill))
}

/// A child's output after it ended: version probes (7.7), `exec` (5.10).
pub struct Captured {
    pub status: ExitInfo,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    /// Stdout or stderr went past `read_cap()`: the tree was stopped there, and that stream's
    /// bytes are dropped.
    pub too_much: bool,
}

/// 1 GiB, the most text repetition makes (5.3): what `read_file` reads, and what `run_captured`
/// keeps of each stream, at most (5.10).
pub const MAX_READ: usize = 1 << 30;

/// `MAX_READ`; in debug builds `V2_TEST_MAX_READ` (bytes) instead, so tests needn't fill 1 GiB.
pub fn read_cap() -> usize {
    #[cfg(debug_assertions)]
    if let Some(n) = std::env::var("V2_TEST_MAX_READ").ok().and_then(|v| v.parse().ok()) {
        return n;
    }
    MAX_READ
}

/// Read `r` to its end into `out`; `Ok(false)` once it holds more than `cap` bytes, which are not
/// kept, so memory stays bounded by `cap` (a device or a program that never ends). `size`: what a
/// regular file's metadata says, so its buffer is made once.
pub fn read_capped(mut r: impl Read, cap: usize, size: usize, out: &mut Vec<u8>) -> std::io::Result<bool> {
    let _ = out.try_reserve_exact(size.min(cap));
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => return Ok(true),
            Ok(n) if out.len() + n > cap => {
                out.clear();
                return Ok(false);
            }
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// A child's stdout that ends when the child does. A descendant that called `setsid` is out of
/// reach of the group kill and can hold the pipe open, and write to it, for as long as it lives
/// (8.5).
#[cfg(unix)]
pub struct UntilExit {
    pipe: std::fs::File,
    kill: KillHandle,
    /// Once the child has exited: what it left in the pipe, still to read. Anything after that
    /// is a descendant's.
    left: Option<usize>,
    checked: Instant,
}

#[cfg(unix)]
impl UntilExit {
    pub fn new(pipe: impl Into<std::os::fd::OwnedFd>, kill: KillHandle) -> UntilExit {
        UntilExit { pipe: std::fs::File::from(pipe.into()), kill, left: None, checked: Instant::now() }
    }
}

#[cfg(unix)]
impl Read for UntilExit {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let fd = std::os::fd::AsRawFd::as_raw_fd(&self.pipe);
        loop {
            // Checked even while data flows, so a chatty descendant can't hide the exit.
            if self.left.is_none() && self.checked.elapsed() >= EXIT_CHECK {
                self.checked = Instant::now();
                if self.kill.has_exited() {
                    let mut n: libc::c_int = 0;
                    // SAFETY: FIONREAD writes one int, the bytes waiting in the pipe.
                    unsafe { libc::ioctl(fd, libc::FIONREAD, &mut n) };
                    self.left = Some(n.max(0) as usize);
                }
            }
            if let Some(n) = self.left {
                let k = buf.len().min(n);
                let got = if k == 0 { 0 } else { self.pipe.read(&mut buf[..k])? };
                self.left = Some(n - got);
                return Ok(got);
            }
            let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            // SAFETY: one pollfd, alive for the call.
            match unsafe { libc::poll(&mut p, 1, EXIT_CHECK.as_millis() as i32) } {
                0 => {}
                -1 if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => {}
                _ => return self.pipe.read(buf),
            }
        }
    }
}

/// A pipe read to its end on a thread of its own, keeping `read_cap()` bytes at most: past that
/// it stops `kill`'s tree, so the wait for its exit ends, and gives `None` (5.10).
fn drain<R: Read + Send + 'static>(stream: Option<R>, kill: KillHandle) -> JoinHandle<Option<Vec<u8>>> {
    spawn_helper(std::thread::Builder::new(), move || {
        let mut out = Vec::new();
        if let Some(s) = stream {
            if let Ok(false) = read_capped(s, read_cap(), 0, &mut out) {
                kill.kill_tree();
                return None;
            }
        }
        Some(out)
    })
    .expect("start a reader thread")
}

/// Run `argv` to its end with stdout and stderr captured, and at most `limit`. Stdin is the null
/// device, or a pipe that a helper thread writes `input` into and then closes. Each output pipe
/// is read on its own thread, up to `read_cap()` bytes; this thread waits for the exit (or the
/// timer's kill, or a reader's past that size), kills the tree, joins the readers, then reaps
/// (8.3).
pub fn run_captured(
    argv: &[OsString],
    env: &[(OsString, OsString)],
    limit: Option<Duration>,
    input: Option<Vec<u8>>,
    raw_tail: bool,
) -> Result<Captured, SpawnError> {
    run_captured_as(argv, None, env, limit, input, raw_tail)
}

/// `run_captured`, the program seeing `arg0` as its `argv[0]` on Unix (`ChildProc::spawn_as`).
pub fn run_captured_as(
    argv: &[OsString],
    arg0: Option<&std::ffi::OsStr>,
    env: &[(OsString, OsString)],
    limit: Option<Duration>,
    input: Option<Vec<u8>>,
    raw_tail: bool,
) -> Result<Captured, SpawnError> {
    let stdin = if input.is_some() { Pipe::Piped } else { Pipe::Null };
    let mut child = ChildProc::spawn_as(&Spawn { argv, env, stdin, stdout: Pipe::Piped, stderr: Pipe::Piped, remove: &[], keep: &[], raw_tail }, arg0)?;
    // A child that exits without reading gets a write error here, which is not the caller's.
    let feed = input.zip(child.take_stdin()).map(|(bytes, mut pipe)| {
        spawn_helper(std::thread::Builder::new(), move || {
            let _ = std::io::Write::write_all(&mut pipe, &bytes);
        })
        .expect("start a writer thread")
    });
    let out = drain(child.take_stdout(), child.kill_handle());
    let err = drain(child.take_stderr(), child.kill_handle());
    let timer = limit.map(|l| push_timer(Some(l), child.kill_handle()));
    child.wait_exit();
    child.kill_tree();
    let timed_out = timer.as_ref().is_some_and(TimerGuard::fired);
    drop(timer);
    let stdout = out.join().unwrap_or(Some(Vec::new()));
    let stderr = err.join().unwrap_or(Some(Vec::new()));
    let too_much = stdout.is_none() || stderr.is_none();
    // Not joined: its writes fail now that the tree is dead, and a process that escaped the
    // tree could keep it blocked (8.3).
    drop(feed);
    let status = child.reap();
    Ok(Captured { status, stdout: stdout.unwrap_or_default(), stderr: stderr.unwrap_or_default(), timed_out, too_much })
}

// ---------------------------------------------------------------------------------------------
// The interrupt handlers (6.5)

/// Unix: the signals that interrupt a run.
#[cfg(unix)]
const INTERRUPTS: [libc::c_int; 4] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];

#[cfg(unix)]
fn interrupt_set() -> libc::sigset_t {
    // SAFETY: sigemptyset and sigaddset fill a zeroed set.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for sig in INTERRUPTS {
            libc::sigaddset(&mut set, sig);
        }
        set
    }
}

/// Unix: whether this thread takes the interrupt signals. Main blocks them before it starts any
/// thread, so only the interpreter thread, which takes them, gets them: the kernel runs the
/// handler there, ending the wait of an `exec` or a worker request, so a signal sent while one
/// runs (its program's `kill $PPID`) is seen as it returns, not a statement later (6.5).
#[cfg(unix)]
pub fn take_interrupts(take: bool) {
    let set = interrupt_set();
    // SAFETY: changes this thread's mask only.
    unsafe {
        libc::pthread_sigmask(if take { libc::SIG_UNBLOCK } else { libc::SIG_BLOCK }, &set, std::ptr::null_mut());
    }
}

/// A helper thread, started with the interrupt signals blocked, as it never takes them
/// (`take_interrupts`).
pub fn spawn_helper<F, T>(b: std::thread::Builder, f: F) -> std::io::Result<JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    #[cfg(unix)]
    {
        let set = interrupt_set();
        // SAFETY: blocks, then restores, this thread's mask around the spawn; the new thread
        // keeps the blocked one.
        let mut old: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old) };
        let r = b.spawn(f);
        unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut()) };
        r
    }
    #[cfg(not(unix))]
    b.spawn(f)
}

/// Unix: SIGINT, SIGTERM, SIGHUP and SIGQUIT set `INTERRUPTED`, an async-signal-safe store. A signal
/// ignored when v2 starts (`nohup`, background jobs of a shell without job control) stays
/// ignored (6.5).
#[cfg(unix)]
fn install_interrupt_handler() {
    extern "C" fn on_signal(sig: libc::c_int) {
        let _ = SIGNALED.compare_exchange(0, sig, Ordering::SeqCst, Ordering::SeqCst);
        INTERRUPTED.store(true, Ordering::SeqCst);
    }
    // SAFETY: the handler only stores to an atomic; `sigaction` gets zeroed, filled structs.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
        libc::sigemptyset(&mut action.sa_mask);
        for sig in INTERRUPTS {
            let mut old: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(sig, std::ptr::null(), &mut old) == 0 && old.sa_sigaction == libc::SIG_IGN {
                continue;
            }
            libc::sigaction(sig, &action, std::ptr::null_mut());
        }
    }
    // Ctrl+Z: the watchdog stops the children with v2 (`suspend`); one that is ignored stays so.
    extern "C" fn on_stop(_: libc::c_int) {
        SUSPEND.store(true, Ordering::SeqCst);
    }
    // SAFETY: as above.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_stop as extern "C" fn(libc::c_int) as usize;
        libc::sigemptyset(&mut action.sa_mask);
        let mut old: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(libc::SIGTSTP, std::ptr::null(), &mut old) == 0 && old.sa_sigaction != libc::SIG_IGN {
            libc::sigaction(libc::SIGTSTP, &action, std::ptr::null_mut());
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
        wake();
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    // SAFETY: registering a handler routine with the right signature.
    unsafe {
        SetConsoleCtrlHandler(Some(on_ctrl), 1);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// The watchdog is process-wide and only its top timer runs, as on the one interpreter
    /// thread of a run; tests that push timers take turns so they don't bury each other's.
    pub(crate) fn serial() -> MutexGuard<'static, ()> {
        static SERIAL: Mutex<()> = Mutex::new(());
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// argv and env that run this test binary as a helper child playing `role` (`helper_main`).
    fn helper(role: &str) -> (Vec<OsString>, Vec<(OsString, OsString)>) {
        let exe = std::env::current_exe().expect("the test binary");
        let argv = ["--exact", "engines::process::tests::helper_main", "--nocapture", "--test-threads=1"]
            .into_iter()
            .map(OsString::from);
        let argv = std::iter::once(exe.into_os_string()).chain(argv).collect();
        (argv, vec![("V2_HELPER".into(), role.into())])
    }

    /// A plain `Command` for a helper grandchild, the way user code would start one.
    fn plain_helper(role: &str) -> Command {
        let (argv, env) = helper(role);
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]).envs(env);
        cmd
    }

    /// Read a helper's stdout until it says `ready`; what it said.
    fn wait_ready(out: &mut impl Read) -> String {
        let mut seen = Vec::new();
        let mut buf = [0u8; 256];
        while !String::from_utf8_lossy(&seen).contains("ready") {
            let n = out.read(&mut buf).unwrap();
            assert!(n > 0, "the helper ended early: {}", String::from_utf8_lossy(&seen));
            seen.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&seen).into_owned()
    }

    #[test]
    fn a_worker_parks_once_per_idle_period() {
        // 8.8, on the bookkeeping alone: a real `Watch` is global and its clock is real.
        let (argv, env) = helper("sleep:20000");
        let child = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Null, stderr: Pipe::Null, remove: &[], keep: &[], raw_tail: false }).unwrap();
        let kill = child.kill_handle();
        let after = Duration::from_secs(10);
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut w = Watch::default();
        w.parkable.push(Parkable { kill: kill.clone(), idle_since: t0, parked: false });
        assert!(w.due_to_park(at(9), after).is_empty(), "not idle long enough");
        assert_eq!(w.due_to_park(at(10), after).len(), 1);
        assert!(w.due_to_park(at(30), after).is_empty(), "once per idle period");
        w.request_done(&kill, at(31));
        assert!(w.due_to_park(at(40), after).is_empty(), "a request restarts the clock");
        assert_eq!(w.due_to_park(at(41), after).len(), 1);
        w.request_done(&kill, at(41));
        w.kills.insert(1, kill.clone());
        assert!(w.due_to_park(at(60), after).is_empty(), "a request in flight is not idle");
        child.kill_tree();
        child.reap();
    }

    /// The file a `late` helper writes as soon as it starts.
    fn started(marker: &str) -> PathBuf {
        PathBuf::from(format!("{marker}.started"))
    }

    fn wait_for(path: &std::path::Path) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.exists() {
            assert!(Instant::now() < deadline, "{} never appeared", path.display());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The helper child: does nothing unless `V2_HELPER` names a role. Grandchildren are left
    /// running on purpose, like user code that outlives its parent.
    #[test]
    #[allow(clippy::zombie_processes)]
    fn helper_main() {
        let Ok(role) = std::env::var("V2_HELPER") else { return };
        let (role, arg) = role.split_once(':').unwrap_or((role.as_str(), ""));
        let mut out = std::io::stdout();
        match role {
            "sleep" => std::thread::sleep(Duration::from_millis(arg.parse().unwrap())),
            "exit" => std::process::exit(arg.parse().unwrap()),
            "chatty" => {
                let chunk = vec![b'x'; 64 * 1024];
                let mut err = std::io::stderr();
                for _ in 0..16 {
                    out.write_all(&chunk).unwrap();
                    err.write_all(&chunk).unwrap();
                }
            }
            // Start a grandchild that writes `arg` after 1.5 s unless killed, then wait.
            "grandchild" => {
                plain_helper(&format!("late:{arg}")).stdout(Stdio::null()).spawn().unwrap();
                writeln!(out, "ready").unwrap();
                out.flush().unwrap();
                std::thread::sleep(Duration::from_secs(30));
            }
            // 1 MiB to stdout and to stderr before reading stdin: every pipe must be serviced
            // at once (8.3).
            "chatty_cat" => {
                let chunk = vec![b'x'; 64 * 1024];
                let mut err = std::io::stderr();
                for _ in 0..16 {
                    out.write_all(&chunk).unwrap();
                    err.write_all(&chunk).unwrap();
                }
                let mut input = Vec::new();
                std::io::stdin().read_to_end(&mut input).unwrap();
                write!(out, "<read {}>", input.len()).unwrap();
            }
            // Start a grandchild that holds stdout and writes `arg` after 1.5 s, then exit.
            "orphan" => {
                plain_helper(&format!("late:{arg}")).stdin(Stdio::null()).spawn().unwrap();
                wait_for(&started(arg));
            }
            // The length of the command line this process was started with, as Windows keeps it.
            #[cfg(windows)]
            "cmdline" => {
                #[link(name = "kernel32")]
                extern "system" {
                    fn GetCommandLineW() -> *const u16;
                }
                // SAFETY: Windows keeps the process's NUL-terminated command line for its lifetime.
                let n = unsafe {
                    let line = GetCommandLineW();
                    (0..).take_while(|&i| *line.add(i) != 0).count()
                };
                writeln!(out, "\ncmdline {n}").unwrap();
            }
            "cat" => {
                let mut input = Vec::new();
                std::io::stdin().read_to_end(&mut input).unwrap();
                out.write_all(b"<").unwrap();
                out.write_all(&input).unwrap();
                out.write_all(b">").unwrap();
            }
            "late" => {
                std::fs::write(started(arg), "").unwrap();
                std::thread::sleep(Duration::from_millis(1500));
                std::fs::write(arg, "alive").unwrap();
            }
            // A host killed with no exit path: its children must die with it (6.5, 8.3).
            "owner" => {
                let (argv, env) = helper(&format!("late:{arg}"));
                let _child = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Null, stderr: Pipe::Null, remove: &[], keep: &[], raw_tail: false }).unwrap();
                wait_for(&started(arg));
                writeln!(out, "ready").unwrap();
                out.flush().unwrap();
                std::thread::sleep(Duration::from_secs(30));
            }
            // After an interrupt nothing more is started (6.5).
            "spawn_after_interrupt" => {
                INTERRUPTED.store(true, Ordering::SeqCst);
                let (argv, env) = helper(&format!("late:{arg}"));
                let _ = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Null, stderr: Pipe::Null, remove: &[], keep: &[], raw_tail: false });
                writeln!(out, "spawned").unwrap();
            }
            // Leave a grandchild holding stdout, then exit at once.
            "holder" => {
                plain_helper("sleep:20000").spawn().unwrap();
            }
            #[cfg(unix)]
            "session" => {
                // SAFETY: plain queries.
                let leader = unsafe { libc::getsid(0) == libc::getpid() };
                writeln!(out, "session-leader {leader}").unwrap();
            }
            // A host whose run dir is in use by a child tree when the interrupt comes; the
            // watchdog ends the run (6.5, 20.3).
            "interrupted" => {
                let (dir, marker) = arg.split_once('|').unwrap();
                let dir = PathBuf::from(dir);
                std::fs::create_dir_all(&dir).unwrap();
                std::env::set_current_dir(&dir).unwrap();
                set_run_dir(dir.clone());
                let (argv, env) = helper(&format!("grandchild:{marker}"));
                // The child's cwd is the run dir (as a compiler's would be); the host's is not.
                let mut child = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Piped, stderr: Pipe::Null, remove: &[], keep: &[], raw_tail: false }).unwrap();
                std::env::set_current_dir(std::env::temp_dir()).unwrap();
                wait_ready(&mut child.take_stdout().unwrap());
                Watchdog::start();
                INTERRUPTED.store(true, Ordering::SeqCst);
                wake();
                std::thread::sleep(Duration::from_secs(10));
            }
            // A run dir made by config::rundir is removed by the interrupt path (20.3).
            "rundir" => {
                let dir = crate::config::rundir::run_dir().unwrap().to_path_buf();
                writeln!(out, "\nRUNDIR={}", dir.display()).unwrap();
                out.flush().unwrap();
                INTERRUPTED.store(true, Ordering::SeqCst);
                exit_interrupted();
            }
            // A run started with SIGCHLD ignored still sees its children's statuses.
            "child_status" => {
                Watchdog::start();
                let (argv, env) = helper("exit:3");
                let c = run_captured(&argv, &env, Some(Duration::from_secs(30)), None, false).unwrap();
                writeln!(out, "status {:?}", c.status.code).unwrap();
            }
            // A host with a console but no window: its child must share that console (8.3).
            #[cfg(windows)]
            "console_host" => {
                let (argv, env) = helper("console_list");
                let c = run_captured(&argv, &env, Some(Duration::from_secs(30)), None, false).unwrap();
                out.write_all(&c.stdout).unwrap();
            }
            #[cfg(windows)]
            "console_list" => {
                let mut pids = [0u32; 16];
                // SAFETY: a buffer of 16 pids.
                let n = unsafe { windows_sys::Win32::System::Console::GetConsoleProcessList(pids.as_mut_ptr(), 16) };
                writeln!(out, "console processes {n}").unwrap();
            }
            other => panic!("unknown helper role {other}"),
        }
        out.flush().unwrap();
        std::process::exit(0);
    }

    /// One argument back from a command line, by the C runtime's rules (MSVCRT's since 2008,
    /// as Rust's own `args()` reads them): what `command_line_arg` must round-trip through.
    fn c_runtime_args(line: &str) -> Vec<String> {
        let chars: Vec<char> = line.chars().collect();
        let (mut args, mut cur, mut started, mut quoted, mut i) = (Vec::new(), String::new(), false, false, 0);
        while i < chars.len() {
            let c = chars[i];
            if (c == ' ' || c == '\t') && !quoted {
                if started {
                    args.push(std::mem::take(&mut cur));
                }
                started = false;
                i += 1;
                continue;
            }
            started = true;
            if c == '\\' {
                let n = chars[i..].iter().take_while(|&&c| c == '\\').count();
                i += n;
                if chars.get(i) == Some(&'"') {
                    cur.extend(std::iter::repeat_n('\\', n / 2));
                    if n % 2 == 1 {
                        cur.push('"');
                        i += 1;
                    }
                } else {
                    cur.extend(std::iter::repeat_n('\\', n));
                }
            } else if c == '"' {
                if quoted && chars.get(i + 1) == Some(&'"') {
                    cur.push('"');
                    i += 1;
                } else {
                    quoted = !quoted;
                }
                i += 1;
            } else {
                cur.push(c);
                i += 1;
            }
        }
        if started {
            args.push(cur);
        }
        args
    }

    fn encoded(arg: &str) -> String {
        String::from_utf16(&command_line_arg(&arg.encode_utf16().collect::<Vec<u16>>())).unwrap()
    }

    #[test]
    fn command_line_args_read_back_as_given() {
        // 8.3 (round 18): a plain word goes as it is; one MSYS would glob, expand or split is
        // quoted; a run of backslashes before a plain character ends the quotes, as MSYS halves
        // it in them.
        let cases = [
            ("--format=%s", "--format=%s"),
            (r"a\b", r"a\b"),
            (r"trailing\", r"trailing\"),
            ("%PATH%", "%PATH%"),
            ("Čeština", "Čeština"),
            ("", r#""""#),
            ("*.v2", r#""*.v2""#),
            ("it's", r#""it's""#),
            ("@data.json", r#""@data.json""#),
            ("~", r#""~""#),
            ("{a,b}.v2", r#""{a,b}.v2""#),
            ("f(x)", r#""f(x)""#),
            (r#"q"uote"#, r#""q\"uote""#),
            (r#"a\\"b"#, r#""a\\\\\"b""#),
            (r"two words\", r#""two words\\""#),
            (r"\\server\share\My Docs", r#""\\\\"server"\share\My Docs""#),
            (r"C:\a\\b c", r#""C:\a\\\\"b" c""#),
        ];
        for (arg, want) in cases {
            assert_eq!(encoded(arg), want, "{arg}");
        }
        // Every word of these characters up to four long reads back whole through the C runtime.
        let alphabet = ['a', '\\', '"', ' ', '\'', '*', '~', '@', '{', ')', '\t'];
        let (mut words, mut last) = (vec![String::new()], vec![String::new()]);
        for _ in 0..4 {
            last = last.iter().flat_map(|w| alphabet.iter().map(move |c| format!("{w}{c}"))).collect();
            words.extend(last.iter().cloned());
        }
        for w in &words {
            let line = format!("{} {} x", encoded(w), encoded("~"));
            assert_eq!(c_runtime_args(&line), [w.as_str(), "~", "x"], "{w:?} as {line}");
        }
    }

    #[test]
    fn command_line_units_counts_what_createprocess_gets() {
        // Round 23 (W5), 5.10: exec's error names the command line's length past 32,766. The
        // program in quotes, then each argument after a space, quoted as `command_line_arg` does.
        let argv = |a: &[&str]| a.iter().map(OsString::from).collect::<Vec<_>>();
        let units = |program: &str, args: &[&str], raw_tail| command_line_units(program.as_ref(), &argv(args), raw_tail);
        assert_eq!(units(r"C:\p\x.exe", &["a b", "c"], false), 12 + 6 + 2);
        // cmd.exe: Rust's quoting, and with /c the last argument as written.
        assert_eq!(units("cmd.exe", &["/c", "echo \"hi\""], true), 9 + 3 + 10);
        assert_eq!(units("cmd.exe", &["/c", "a b", "q\"x"], false), 9 + 3 + 6 + 5);
        // A batch file (review of round 23): `cmd.exe /e:ON /v:OFF /d /c ""C:\b.bat" …"`, an
        // argument with a symbol in quotes, `%` as `%%cd:~,%`, `"` doubled, a `\` at the end quoted.
        assert_eq!(units(r"C:\b.bat", &["x,yz", "%a%", "plain", "q\"x", "tail\\"], false), 29 + 10 + 7 + 20 + 6 + 7 + 9);
        // Its path as Rust gives it to cmd.exe: from 247 to 259 units with \\?\ in front (second
        // review of round 23), a share's with \\?\UNC\; a \\?\ one of at most 259 without.
        let path = |head: &str, n: usize| format!("{head}{}.bat", "a".repeat(n - head.len() - 4)).encode_utf16().collect::<Vec<u16>>();
        let lengths = [246, 247, 259, 260].map(|n| batch_script_units(&path(r"C:\", n)));
        assert_eq!(lengths, [246, 251, 263, 260]);
        assert_eq!((batch_script_units(&path(r"\\srv\share\", 250)), batch_script_units(&path(r"\\?\C:\", 200)), batch_script_units(&path(r"\\?\UNC\srv\s\", 200))), (256, 196, 194));
        assert_eq!(units(&String::from_utf16(&path(r"C:\", 250)).unwrap(), &["x"], false), 29 + 254 + 2 + 2);
        // On Windows, the count is the length the child sees.
        #[cfg(windows)]
        {
            let words = ["plain", "with space", "q\"uote", "trail\\", "a\\\\\"b", "", "*.v2", "{a,b}", "Čeština", "x,yz", "%a%"];
            let (mut argv, env) = helper("cmdline");
            argv.extend(words.map(OsString::from));
            let c = run_captured(&argv, &env, None, None, false).unwrap();
            let out = String::from_utf8_lossy(&c.stdout);
            let seen = out.lines().find_map(|l| l.strip_prefix("cmdline ")).unwrap_or_else(|| panic!("{out}"));
            assert_eq!(command_line_units(&argv[0], &argv[1..], false).to_string(), seen);
            // A batch file sees its command line as %CMDCMDLINE%.
            let d = temp("batchline");
            let bat = d.join("line.bat");
            std::fs::write(&bat, "@echo off\r\necho %CMDCMDLINE%\r\n").unwrap();
            let argv: Vec<OsString> = std::iter::once(bat.into_os_string()).chain(words.into_iter().filter(|w| !w.is_empty()).map(OsString::from)).collect();
            let c = run_captured(&argv, &[], None, None, false).unwrap();
            let line = String::from_utf8_lossy(&c.stdout);
            assert_eq!(line.trim_end_matches(['\r', '\n']).encode_utf16().count(), command_line_units(&argv[0], &argv[1..], false), "{line}");
            // At a path of 250 units, which Rust gives cmd.exe with \\?\ in front.
            let room = 250 - d.as_os_str().len() - r"\\line.bat".len();
            let folder = d.join("f".repeat(room));
            std::fs::create_dir_all(&folder).unwrap();
            let bat = folder.join("line.bat");
            assert_eq!(bat.as_os_str().len(), 250);
            std::fs::write(&bat, "@echo off\r\necho %CMDCMDLINE%\r\n").unwrap();
            // cmd.exe can't run it by that path, so it starts by its 8.3 one (long_program's 246
            // for a batch file), and the count is of the path it started by.
            let argv = [bat.into_os_string(), OsString::from("x,yz")];
            let c = run_captured(&argv, &[], None, None, false).unwrap();
            let line = String::from_utf8_lossy(&c.stdout);
            let started = short_program(&argv[0]).expect("an 8.3 path for the batch file");
            assert!(line.contains('~') && !line.contains(r"\\?\"), "{line} | {}", String::from_utf8_lossy(&c.stderr));
            assert_eq!(line.trim_end_matches(['\r', '\n']).encode_utf16().count(), command_line_units(&started, &argv[1..], false), "{line}");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_program_at_a_long_path_starts_by_a_shorter_one_but_is_named_as_given() {
        // Review of round 23 (W5), 5.10: the folder by its 8.3 path, the file by its own name
        // (multi-call programs read it); a start that fails names the path as given.
        let d = temp("longprogram");
        let mut folder = d.clone();
        for _ in 0..3 {
            folder.push("d".repeat(100));
        }
        std::fs::create_dir_all(&folder).unwrap();
        let junk = folder.join("clang-cl-like.exe");
        std::fs::write(&junk, "not a program").unwrap();
        let argv = [junk.clone().into_os_string()];
        let Err(e) = ChildProc::spawn(&Spawn { argv: &argv, env: &[], stdin: Pipe::Null, stdout: Pipe::Null, stderr: Pipe::Null, remove: &[], keep: &[], raw_tail: false }) else {
            panic!("a text file started")
        };
        assert!(e.message.starts_with(&format!("could not start '{}': ", junk.display())), "{}", e.message);
        // Where the volume has 8.3 names (not every CI drive does).
        if let Some(short) = &e.short {
            let short = std::path::Path::new(short);
            assert_eq!(short.file_name(), junk.file_name());
            assert!(short.to_string_lossy().contains('~') && long_program(short.as_os_str()).is_none(), "{}", short.display());
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("v2proc-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn timer_stack_runs_only_the_top() {
        let t0 = Instant::now();
        let mut s = TimerStack::default();
        s.push(1, Some(Duration::from_millis(100)), t0);
        s.push(2, Some(Duration::from_secs(1)), at(t0, 50));
        // Timer 1 has used 50 ms and is paused under timer 2.
        assert_eq!(s.expired(at(t0, 200)), None);
        assert_eq!(s.time_left(at(t0, 200)), Some(Duration::from_millis(850)));
        s.pop(2, at(t0, 300));
        assert_eq!(s.expired(at(t0, 349)), None);
        assert_eq!(s.expired(at(t0, 350)), Some(1));
        s.mark_fired(1);
        assert_eq!(s.expired(at(t0, 400)), None, "a fired timer fires once");
        s.push(3, None, t0);
        assert_eq!((s.expired(at(t0, 9_000)), s.time_left(t0)), (None, None), "no limit");
    }

    #[test]
    fn paused_time_does_not_count() {
        let t0 = Instant::now();
        let mut s = TimerStack::default();
        s.push(1, Some(Duration::from_millis(100)), t0);
        s.pause(1, at(t0, 60));
        assert_eq!(s.expired(at(t0, 500)), None);
        s.resume(1, at(t0, 500));
        assert_eq!(s.expired(at(t0, 539)), None);
        assert_eq!(s.expired(at(t0, 540)), Some(1));
        // An explicit pause outlasts a timer pushed and popped on top of it.
        let mut s = TimerStack::default();
        s.push(1, Some(Duration::from_millis(100)), t0);
        s.pause(1, at(t0, 10));
        s.push(2, Some(Duration::from_secs(5)), at(t0, 10));
        s.pop(2, at(t0, 20));
        assert_eq!(s.expired(at(t0, 1_000)), None);
        // A buried timer resumed under another one runs only once it is on top again.
        let mut s = TimerStack::default();
        s.push(1, Some(Duration::from_millis(100)), t0);
        s.pause(1, at(t0, 10));
        s.push(2, Some(Duration::from_secs(10)), at(t0, 10));
        s.resume(1, at(t0, 20));
        s.pop(2, at(t0, 1_000));
        assert_eq!(s.expired(at(t0, 1_089)), None, "10 ms before the pause, 89 ms on top since");
        assert_eq!(s.expired(at(t0, 1_090)), Some(1));
        // A buried timer paused and resumed still restarts when popped back to the top.
        let mut s = TimerStack::default();
        s.push(1, Some(Duration::from_millis(100)), t0);
        s.push(2, Some(Duration::from_secs(10)), at(t0, 50));
        s.pause(1, at(t0, 60));
        s.resume(1, at(t0, 70));
        s.pop(2, at(t0, 1_000));
        assert_eq!(s.expired(at(t0, 1_049)), None);
        assert_eq!(s.expired(at(t0, 1_050)), Some(1), "its deadline is still enforced");
    }

    #[test]
    fn time_suspended_doesnt_count() {
        // 8.5: a process stopped (Ctrl+Z, SIGSTOP) or a computer asleep runs no call, so the
        // running clock skips that time; a paused or buried timer has nothing to skip.
        let t0 = Instant::now();
        let mut s = TimerStack::default();
        s.push(1, Some(Duration::from_millis(100)), t0);
        s.skip(Duration::from_millis(5_000), at(t0, 5_050));
        assert_eq!(s.expired(at(t0, 5_099)), None, "50 ms ran before the stop, 49 after");
        assert_eq!(s.expired(at(t0, 5_100)), Some(1));
        // A gap can't make a clock start in the future.
        let mut s = TimerStack::default();
        s.push(1, Some(Duration::from_millis(100)), t0);
        s.skip(Duration::from_secs(60), at(t0, 10));
        assert_eq!(s.expired(at(t0, 109)), None);
        assert_eq!(s.expired(at(t0, 110)), Some(1));
    }

    #[test]
    fn exit_check_ends_a_read_a_grandchild_holds_open() {
        let _serial = crate::engines::process::tests::serial();
        // A session reads its worker's stdout to EOF (8.5): when the worker exits but a
        // grandchild still holds the pipe, the watchdog's exited check kills the tree.
        let (argv, env) = helper("holder");
        let mut child = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Piped, stderr: Pipe::Null, remove: &[], keep: &[], raw_tail: false })
            .expect("spawn");
        let timer = push_timer(None, child.kill_handle());
        let start = Instant::now();
        let mut out = child.take_stdout().unwrap();
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
        drop(timer);
        child.reap();
    }

    #[test]
    fn the_interrupt_path_kills_the_trees_and_removes_the_run_dir() {
        // 20.3: also when a killed child was using it (Windows releases its files late). The
        // grandchild is not covered by PDEATHSIG, so only the tree kill stops it.
        let mut markers = Vec::new();
        for round in 0..3 {
            let base = temp(&format!("rundir{round}"));
            let (dir, marker) = (base.join("run"), base.join("late.txt"));
            let (argv, env) = helper(&format!("interrupted:{}|{}", dir.display(), marker.display()));
            let status = Command::new(&argv[0]).args(&argv[1..]).envs(env).stdout(Stdio::null()).status().unwrap();
            assert_eq!(status.code(), Some(EXIT_INTERRUPTED));
            assert!(!dir.exists(), "round {round}: the run dir was left behind");
            markers.push(marker);
        }
        std::thread::sleep(Duration::from_millis(2500));
        assert!(markers.iter().all(|m| !m.exists()), "a grandchild outlived the interrupt");
    }

    #[test]
    fn nothing_is_spawned_after_an_interrupt() {
        let marker = temp("nospawn").join("late.txt");
        let (argv, env) = helper(&format!("spawn_after_interrupt:{}", marker.display()));
        let status = Command::new(&argv[0]).args(&argv[1..]).envs(env).stdout(Stdio::null()).status().unwrap();
        assert_eq!(status.code(), Some(EXIT_INTERRUPTED));
        std::thread::sleep(Duration::from_millis(500));
        assert!(!started(&marker.display().to_string()).exists(), "a process was started after the interrupt");
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn children_die_with_a_killed_host() {
        // PR_SET_PDEATHSIG (Linux) and kill-on-close Job Objects (Windows): no exit path runs.
        let marker = temp("hostdeath").join("late.txt");
        let mut cmd = plain_helper(&format!("owner:{}", marker.display()));
        let mut owner = cmd.stdout(Stdio::piped()).spawn().unwrap();
        wait_ready(owner.stdout.as_mut().unwrap());
        owner.kill().unwrap();
        owner.wait().unwrap();
        std::thread::sleep(Duration::from_millis(2500));
        assert!(!marker.exists(), "a child outlived its killed host");
    }

    #[test]
    fn reap_and_drop_kill_the_tree() {
        let dir = temp("reapdrop");
        let spawn = |name: &str| {
            let marker = dir.join(name);
            let (argv, env) = helper(&format!("grandchild:{}", marker.display()));
            let mut c = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Piped, stderr: Pipe::Null, remove: &[], keep: &[], raw_tail: false })
                .expect("spawn");
            wait_ready(&mut c.take_stdout().unwrap());
            (c, marker)
        };
        let (a, marker_a) = spawn("a.txt");
        let (b, marker_b) = spawn("b.txt");
        let reg = b.reg;
        let start = Instant::now();
        assert!(!a.reap().success());
        drop(b);
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
        assert!(!lock().children.iter().any(|(id, _)| *id == reg), "unregistered when dropped");
        std::thread::sleep(Duration::from_millis(2500));
        assert!(!marker_a.exists() && !marker_b.exists(), "a grandchild outlived reap or drop");
    }

    #[test]
    fn dropped_timers_and_reaped_children_leave_no_state() {
        let _serial = crate::engines::process::tests::serial();
        let (argv, env) = helper("sleep:20000");
        let child = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Null, stderr: Pipe::Null, remove: &[], keep: &[], raw_tail: false })
            .expect("spawn");
        let reg = child.reg;
        assert!(lock().children.iter().any(|(id, _)| *id == reg), "registered for the interrupt path");
        let timer = push_timer(Some(Duration::from_secs(60)), child.kill_handle());
        let id = timer.id;
        assert!(lock().kills.contains_key(&id));
        drop(timer);
        {
            let w = lock();
            assert!(!w.kills.contains_key(&id) && !w.fired.contains(&id), "its kill handle is gone");
            assert!(!w.timers.stack.iter().any(|t| t.id == id), "it is off the stack");
        }
        child.reap();
        assert!(!lock().children.iter().any(|(id, _)| *id == reg), "unregistered when reaped");
    }

    #[test]
    fn a_reaped_child_is_never_signalled() {
        let _serial = crate::engines::process::tests::serial();
        // A session reaps a dead worker while its request still holds the timer (8.5, 8.6).
        let (argv, env) = helper("exit:0");
        let child = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Null, stderr: Pipe::Null, remove: &[], keep: &[], raw_tail: false })
            .expect("spawn");
        child.wait_exit();
        let timer = push_timer(Some(Duration::from_millis(200)), child.kill_handle());
        child.reap();
        std::thread::sleep(Duration::from_millis(600));
        assert!(!timer.fired(), "the watchdog killed a reaped child");
        let w = lock();
        assert!(!w.kills.contains_key(&timer.id) && !w.timers.stack.iter().any(|t| t.id == timer.id));
    }

    #[test]
    fn a_paused_timer_fires_only_after_resume() {
        let _serial = crate::engines::process::tests::serial();
        let (argv, env) = helper("sleep:20000");
        let child = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Null, stderr: Pipe::Null, remove: &[], keep: &[], raw_tail: false })
            .expect("spawn");
        let timer = push_timer(Some(Duration::from_millis(300)), child.kill_handle());
        timer.pause();
        std::thread::sleep(Duration::from_millis(800));
        assert!(!timer.fired() && child.exited().is_none(), "paused time does not count");
        timer.resume();
        let start = Instant::now();
        assert!(child.wait_exit().is_some());
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
        assert!(timer.fired());
        assert!(lock().timers.stack.iter().any(|t| t.id == timer.id && t.fired), "marked, so it fires once");
        drop(timer);
        child.reap();
    }

    #[test]
    fn the_limit_ends_a_captured_run() {
        let _serial = crate::engines::process::tests::serial();
        let (argv, env) = helper("sleep:20000");
        let start = Instant::now();
        let c = run_captured(&argv, &env, Some(Duration::from_millis(500)), None, false).expect("spawn");
        assert!(c.timed_out, "{:?}", c.status);
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    }

    #[test]
    fn a_captured_run_gets_its_input_then_eof() {
        // `exec(argv, input: …)` (5.10): the bytes, then end of input; without input, the null
        // device. 1 MiB is more than a pipe buffer, so the writer must not block the reader.
        let (argv, env) = helper("cat");
        // libtest's own lines come before the helper's output.
        let echoed = |input: Option<Vec<u8>>| {
            let c = run_captured(&argv, &env, None, input, false).expect("spawn");
            let at = c.stdout.iter().position(|&b| b == b'<').expect("the helper's output");
            c.stdout[at..].to_vec()
        };
        let big = vec![b'x'; 1 << 20];
        assert_eq!(echoed(Some(big.clone())).len(), big.len() + 2);
        assert_eq!(echoed(Some(b"a\nb".to_vec())), b"<a\nb>");
        assert_eq!(echoed(None), b"<>");
    }

    #[test]
    fn a_captured_run_services_its_input_and_outputs_at_once() {
        // 8.3: input is written while both outputs are read. A child that fills stdout and
        // stderr before it reads stdin, and one that never reads its input, both finish.
        let _serial = crate::engines::process::tests::serial();
        let input = vec![b'y'; 1 << 20];
        let (argv, env) = helper("chatty_cat");
        let c = run_captured(&argv, &env, Some(Duration::from_secs(30)), Some(input.clone()), false).expect("spawn");
        assert!(!c.timed_out);
        assert!(c.stdout.ends_with(b"<read 1048576>"), "{}", String::from_utf8_lossy(&c.stdout[c.stdout.len().saturating_sub(40)..]));
        assert!(c.stdout.len() > 1 << 20 && c.stderr.len() >= 1 << 20, "{} {}", c.stdout.len(), c.stderr.len());
        let (argv, env) = helper("chatty");
        let c = run_captured(&argv, &env, Some(Duration::from_secs(30)), Some(input), false).expect("spawn");
        assert!(!c.timed_out && c.status.success(), "{:?}", c.status);
    }

    #[test]
    fn a_captured_run_stops_what_its_child_started() {
        // 8.3, DOCS 3.11 (`exec`): a grandchild left behind holding stdout is killed with the
        // tree when the child exits, so the run returns at once and the grandchild never writes.
        let dir = temp("orphan");
        let marker = dir.join("late");
        let (argv, env) = helper(&format!("orphan:{}", marker.display()));
        let start = Instant::now();
        let c = run_captured(&argv, &env, None, None, false).expect("spawn");
        assert!(c.status.success(), "{:?}", c.status);
        assert!(start.elapsed() < Duration::from_millis(1400), "{:?}", start.elapsed());
        std::thread::sleep(Duration::from_millis(2500));
        assert!(!marker.exists(), "the grandchild outlived the run");
    }

    #[cfg(unix)]
    #[test]
    fn a_killed_child_reports_its_signal() {
        let (argv, env) = helper("sleep:20000");
        let child = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Null, stderr: Pipe::Null, remove: &[], keep: &[], raw_tail: false })
            .expect("spawn");
        child.kill_tree();
        let killed = ExitInfo { code: None, signal: Some(libc::SIGKILL) };
        assert_eq!(child.wait_exit(), Some(killed));
        assert_eq!(child.reap(), killed);
        assert_eq!(killed.describe(), format!("killed by signal {} (SIGKILL)", libc::SIGKILL));
    }

    #[cfg(windows)]
    #[test]
    fn windows_spawn_and_exit_texts() {
        let bad = temp("bad").join("not-a-program.exe");
        std::fs::write(&bad, b"").unwrap();
        let argv = [bad.into_os_string()];
        let e = ChildProc::spawn(&Spawn { argv: &argv, env: &[], stdin: Pipe::Null, stdout: Pipe::Null, stderr: Pipe::Null, remove: &[], keep: &[], raw_tail: false })
            .err()
            .expect("an error");
        assert!(e.message.ends_with(" (not a valid program for this machine)") && !e.message.contains("%1"), "{e:?}");
        let describe = |c: u32| ExitInfo { code: Some(c as i32), signal: None }.describe();
        assert_eq!(describe(0x8000_0000), "exit code 2147483648 = 0x80000000");
        assert_eq!(describe(0x7FFF_FFFF), "exit code 2147483647");
    }

    #[cfg(windows)]
    #[test]
    fn children_are_in_their_own_job() {
        let _serial = crate::engines::process::tests::serial();
        // In this child's job, not just in some job (the test runner may be in one already).
        let (argv, env) = helper("sleep:5000");
        let child = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Null, stderr: Pipe::Null, remove: &[], keep: &[], raw_tail: false })
            .expect("spawn");
        let kill = child.kill_handle();
        let mut in_job = 0;
        // SAFETY: a plain query with handles the KillHandle keeps open.
        unsafe { windows_sys::Win32::System::JobObjects::IsProcessInJob(kill.0.process, kill.0.job, &mut in_job) };
        assert_ne!(in_job, 0);
        // Kill on close (a host that dies takes its children along) and breakaway allowed (8.3).
        use windows_sys::Win32::System::JobObjects::{
            JobObjectExtendedLimitInformation, QueryInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is sized for this information class.
        let ok = unsafe {
            QueryInformationJobObject(
                kill.0.job,
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(ok, 0);
        let flags = info.BasicLimitInformation.LimitFlags;
        assert_eq!(flags & (JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK), JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK);
        assert_eq!(child.exited(), None, "still running");
        child.kill_tree();
        assert!(child.wait_exit().is_some_and(|e| e.code == Some(1)), "TerminateJobObject(job, 1)");
        child.reap();
    }

    #[test]
    fn the_run_dir_is_registered_for_the_interrupt_path() {
        let (argv, env) = helper("rundir");
        let out = Command::new(&argv[0]).args(&argv[1..]).envs(env).stderr(Stdio::null()).output().unwrap();
        assert_eq!(out.status.code(), Some(EXIT_INTERRUPTED));
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let line = text.lines().find_map(|l| l.strip_prefix("RUNDIR=")).expect("the run dir's path").to_string();
        assert!(!std::path::Path::new(line.trim()).exists(), "left behind: {line}");
    }
}
