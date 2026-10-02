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
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long the watchdog sleeps between checks at most (INTERNALS 1.3).
const TICK: Duration = Duration::from_millis(100);

/// How often the watchdog checks whether the top timer's child has exited (8.5).
const EXIT_CHECK: Duration = Duration::from_millis(250);

/// The exit code of an interrupted run (6.5).
pub const EXIT_INTERRUPTED: i32 = 130;

/// Set by the interrupt handler; nothing else happens in the handler (6.5).
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

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

fn die(w: &Watch) -> ! {
    for (_, kill) in &w.children {
        kill.kill_tree();
    }
    if let Some(dir) = &w.run_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
    hard_exit(EXIT_INTERRUPTED)
}

/// `_exit` on Unix (no atexit handlers, no C stdio flushing); on Windows Rust's exit is
/// `ExitProcess`.
fn hard_exit(code: i32) -> ! {
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

/// One deadline. Only the top of the stack runs: a request's timer pauses while V2 serves a
/// call from that worker, and a timer pushed on top of a running one pauses it until popped.
#[derive(Debug)]
struct Timer {
    id: u64,
    limit: Option<Duration>,
    used: Duration,
    running_since: Option<Instant>,
    /// Paused because another timer was pushed on top; resumed when that one is popped.
    auto_paused: bool,
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
    fn push(&mut self, id: u64, limit: Option<Duration>, now: Instant) {
        if let Some(top) = self.stack.last_mut() {
            if top.running_since.is_some() {
                top.used = top.used_at(now);
                top.running_since = None;
                top.auto_paused = true;
            }
        }
        self.stack.push(Timer { id, limit, used: Duration::ZERO, running_since: Some(now), auto_paused: false, fired: false });
    }

    fn pause(&mut self, id: u64, now: Instant) {
        if let Some(t) = self.stack.iter_mut().find(|t| t.id == id) {
            t.used = t.used_at(now);
            t.running_since = None;
            t.auto_paused = false;
        }
    }

    fn resume(&mut self, id: u64, now: Instant) {
        if let Some(t) = self.stack.iter_mut().find(|t| t.id == id) {
            if t.running_since.is_none() {
                t.running_since = Some(now);
            }
            t.auto_paused = false;
        }
    }

    fn pop(&mut self, id: u64, now: Instant) {
        self.stack.retain(|t| t.id != id);
        if let Some(top) = self.stack.last_mut() {
            if top.auto_paused {
                top.auto_paused = false;
                top.running_since = Some(now);
            }
        }
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
    run_dir: Option<PathBuf>,
    stop: bool,
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

fn watch() {
    let s = shared();
    let mut w = lock();
    let mut last_exit_check = Instant::now();
    loop {
        if w.stop {
            return;
        }
        if interrupted() {
            die(&w);
        }
        let now = Instant::now();
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
        let wait = w.timers.time_left(now).map_or(TICK, |left| left.min(TICK));
        w = match s.wake.wait_timeout(w, wait) {
            Ok((guard, _)) => guard,
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
}

fn wake() {
    shared().wake.notify_all();
}

/// Handle to the watchdog, held by main for the whole run.
pub struct Watchdog;

impl Watchdog {
    /// Start the thread and, if it runs, the interrupt handler: without a watchdog nothing
    /// would end the run, so the OS default stays (6.5).
    pub fn start() -> Watchdog {
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
            w.timers.pop(self.id, Instant::now());
            w.kills.remove(&self.id);
            w.fired.remove(&self.id);
        }
        wake();
    }
}

/// The private run directory, removed on the interrupt path too (20.3).
pub fn set_run_dir(dir: PathBuf) {
    lock().run_dir = Some(dir);
}

fn register(kill: KillHandle) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    lock().children.push((id, kill));
    id
}

fn unregister(id: u64) {
    lock().children.retain(|(c, _)| *c != id);
}

// ---------------------------------------------------------------------------------------------
// Child processes (8.3)

/// What a child's standard stream is connected to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pipe {
    Null,
    Piped,
    Inherit,
}

impl Pipe {
    fn stdio(self) -> Stdio {
        match self {
            Pipe::Null => Stdio::null(),
            Pipe::Piped => Stdio::piped(),
            Pipe::Inherit => Stdio::inherit(),
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
            (_, Some(sig)) => format!("killed by signal {sig} ({})", signal_name(sig)),
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

#[cfg(unix)]
fn signal_name(sig: i32) -> &'static str {
    match sig {
        libc::SIGHUP => "SIGHUP",
        libc::SIGINT => "SIGINT",
        libc::SIGQUIT => "SIGQUIT",
        libc::SIGILL => "SIGILL",
        libc::SIGTRAP => "SIGTRAP",
        libc::SIGABRT => "SIGABRT",
        libc::SIGBUS => "SIGBUS",
        libc::SIGFPE => "SIGFPE",
        libc::SIGKILL => "SIGKILL",
        libc::SIGUSR1 => "SIGUSR1",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGUSR2 => "SIGUSR2",
        libc::SIGPIPE => "SIGPIPE",
        libc::SIGALRM => "SIGALRM",
        libc::SIGTERM => "SIGTERM",
        libc::SIGXCPU => "SIGXCPU",
        libc::SIGXFSZ => "SIGXFSZ",
        libc::SIGSYS => "SIGSYS",
        _ => "an unknown signal",
    }
}

#[cfg(not(unix))]
fn signal_name(_: i32) -> &'static str {
    "an unknown signal"
}

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
    }

    /// Whether the child itself has exited (not reaped; 8.3).
    fn has_exited(&self) -> bool {
        self.exit_info(false).is_some()
    }

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
                return Some(if info.si_code == libc::CLD_EXITED {
                    ExitInfo { code: Some(status), signal: None }
                } else {
                    ExitInfo { code: None, signal: Some(status) }
                });
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

/// `could not start 'C:\...\python.exe': <OS message>` (8.3).
fn spawn_error(program: &OsString, e: &std::io::Error) -> String {
    let mut msg = format!("could not start '{}': {}", program.to_string_lossy(), crate::syntax::span::os_message(e));
    if cfg!(windows) && e.raw_os_error() == Some(193) {
        msg.push_str(" (not a valid program for this machine)");
    }
    msg
}

impl ChildProc {
    pub fn spawn(s: &Spawn) -> Result<ChildProc, String> {
        stop_if_interrupted();
        let program = &s.argv[0];
        let mut cmd = Command::new(program);
        cmd.args(&s.argv[1..]).envs(s.env.iter().map(|(k, v)| (k, v)));
        cmd.stdin(s.stdin.stdio()).stdout(s.stdout.stdio()).stderr(s.stderr.stdio());
        let (child, kill) = platform_spawn(&mut cmd).map_err(|e| spawn_error(program, &e))?;
        let reg = register(kill.clone());
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

    /// Unregister, kill what is left of the tree, then reap (8.3).
    pub fn reap(mut self) -> ExitInfo {
        self.finish()
    }

    fn finish(&mut self) -> ExitInfo {
        self.done = true;
        unregister(self.reg);
        self.kill.kill_tree();
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
fn platform_spawn(cmd: &mut Command) -> std::io::Result<(std::process::Child, KillHandle)> {
    use std::os::unix::process::CommandExt;
    #[cfg(target_os = "linux")]
    let host = std::process::id() as libc::pid_t;
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
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
fn platform_spawn(cmd: &mut Command) -> std::io::Result<(std::process::Child, KillHandle)> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::Foundation::{CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE};
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, TerminateProcess, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_SUSPENDED,
    };

    let mut flags = CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP;
    // SAFETY: a plain query.
    if unsafe { GetConsoleWindow() }.is_null() {
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
    if let Err(e) = resume_main_thread(child.id()) {
        kill.kill_tree();
        return Err(fail(&mut child, e));
    }
    Ok((child, kill))
}

/// The suspended child's only thread, found with a Toolhelp snapshot, resumed (8.3, 17.3).
#[cfg(windows)]
fn resume_main_thread(pid: u32) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    // SAFETY: a snapshot we own and close; the entry struct is sized before use.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut result = Err(std::io::Error::other("its thread was not found"));
        let mut more = Thread32First(snap, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                result = if thread.is_null() {
                    Err(std::io::Error::last_os_error())
                } else {
                    let r = ResumeThread(thread);
                    CloseHandle(thread);
                    if r == u32::MAX {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                };
                break;
            }
            more = Thread32Next(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        result
    }
}

/// A child's output after it ended: probes now, transpilers later (7.7, 12.5).
pub struct Captured {
    pub status: ExitInfo,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

fn drain<R: Read + Send + 'static>(stream: Option<R>) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(mut s) = stream {
            let _ = s.read_to_end(&mut out);
        }
        out
    })
}

/// Run `argv` to its end with stdout and stderr captured, `input` on stdin (else the null
/// device) and at most `limit`. Every pipe is serviced on its own thread; this thread waits for
/// the exit (or the timer's kill), kills the tree, joins the readers, then reaps (8.3).
pub fn run_captured(
    argv: &[OsString],
    env: &[(OsString, OsString)],
    input: Option<Vec<u8>>,
    limit: Option<Duration>,
) -> Result<Captured, String> {
    let stdin = if input.is_some() { Pipe::Piped } else { Pipe::Null };
    let mut child = ChildProc::spawn(&Spawn { argv, env, stdin, stdout: Pipe::Piped, stderr: Pipe::Piped })?;
    let writer = match (input, child.take_stdin()) {
        (Some(bytes), Some(mut w)) => Some(std::thread::spawn(move || {
            let _ = w.write_all(&bytes);
        })),
        _ => None,
    };
    let out = drain(child.take_stdout());
    let err = drain(child.take_stderr());
    let timer = limit.map(|l| push_timer(Some(l), child.kill_handle()));
    child.wait_exit();
    child.kill_tree();
    let timed_out = timer.as_ref().is_some_and(TimerGuard::fired);
    drop(timer);
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if let Some(w) = writer {
        let _ = w.join();
    }
    let status = child.reap();
    Ok(Captured { status, stdout, stderr, timed_out })
}

// ---------------------------------------------------------------------------------------------
// The interrupt handlers (6.5)

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
mod tests {
    use super::*;

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
            "cat" => {
                let mut input = Vec::new();
                std::io::stdin().read_to_end(&mut input).unwrap();
                out.write_all(b"<").unwrap();
                out.write_all(&input).unwrap();
                out.write_all(b">").unwrap();
            }
            "late" => {
                std::thread::sleep(Duration::from_millis(1500));
                std::fs::write(arg, "alive").unwrap();
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
            other => panic!("unknown helper role {other}"),
        }
        out.flush().unwrap();
        std::process::exit(0);
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
    }

    #[test]
    fn kill_tree_kills_grandchildren() {
        let marker = temp("tree").join("late.txt");
        let (argv, env) = helper(&format!("grandchild:{}", marker.display()));
        let mut child = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Piped, stderr: Pipe::Null })
            .expect("spawn");
        let mut out = child.take_stdout().unwrap();
        let mut seen = Vec::new();
        let mut buf = [0u8; 256];
        while !String::from_utf8_lossy(&seen).contains("ready") {
            let n = out.read(&mut buf).unwrap();
            assert!(n > 0, "the helper ended early: {}", String::from_utf8_lossy(&seen));
            seen.extend_from_slice(&buf[..n]);
        }
        assert_eq!(child.exited(), None);
        child.kill_tree();
        let status = child.reap();
        assert!(!status.success());
        std::thread::sleep(Duration::from_millis(2500));
        assert!(!marker.exists(), "the grandchild outlived the tree kill");
    }

    #[test]
    fn probe_limit_kills_tree_and_reports_timed_out() {
        let marker = temp("limit").join("late.txt");
        let (argv, env) = helper(&format!("grandchild:{}", marker.display()));
        let start = Instant::now();
        let c = run_captured(&argv, &env, None, Some(Duration::from_millis(500))).expect("spawn");
        assert!(c.timed_out, "{:?}", c.status);
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
        std::thread::sleep(Duration::from_millis(2500));
        assert!(!marker.exists(), "the grandchild outlived the timeout");
    }

    #[test]
    fn pipes_are_read_concurrently() {
        let (argv, env) = helper("chatty");
        let c = run_captured(&argv, &env, None, Some(Duration::from_secs(30))).expect("spawn");
        assert!(!c.timed_out && c.status.success(), "{:?}", c.status);
        assert!(c.stdout.len() >= 1 << 20 && c.stderr.len() >= 1 << 20, "{} {}", c.stdout.len(), c.stderr.len());
    }

    #[test]
    fn grandchild_holding_stdout_does_not_block() {
        let (argv, env) = helper("holder");
        let start = Instant::now();
        let c = run_captured(&argv, &env, None, Some(Duration::from_secs(30))).expect("spawn");
        assert!(!c.timed_out && c.status.success(), "{:?}", c.status);
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    }

    #[test]
    fn input_reaches_stdin() {
        let (argv, env) = helper("cat");
        let c = run_captured(&argv, &env, Some(b"hello".to_vec()), Some(Duration::from_secs(30))).expect("spawn");
        assert!(String::from_utf8_lossy(&c.stdout).contains("<hello>"), "{}", String::from_utf8_lossy(&c.stdout));
        // A child that never reads its input still ends: the input has its own thread.
        let (argv, env) = helper("exit:0");
        let c = run_captured(&argv, &env, Some(vec![b'x'; 1 << 20]), Some(Duration::from_secs(30))).expect("spawn");
        assert!(c.status.success() && !c.timed_out, "{:?}", c.status);
    }

    #[test]
    fn exit_status_text() {
        let (argv, env) = helper("exit:3");
        let c = run_captured(&argv, &env, None, None).expect("spawn");
        assert_eq!((c.status.code, c.status.describe()), (Some(3), "exit code 3".to_string()));
        let crash = ExitInfo { code: Some(0xC000_0005u32 as i32), signal: None };
        if cfg!(windows) {
            assert_eq!(crash.describe(), "exit code 3221225477 = 0xC0000005");
        }
        #[cfg(unix)]
        assert_eq!(ExitInfo { code: None, signal: Some(libc::SIGSEGV) }.describe(), format!("killed by signal {} (SIGSEGV)", libc::SIGSEGV));
    }

    #[test]
    fn spawn_error_text() {
        let missing = std::env::temp_dir().join("v2-no-such-dir").join("no-such-program.exe");
        let argv = [missing.clone().into_os_string()];
        let e = ChildProc::spawn(&Spawn { argv: &argv, env: &[], stdin: Pipe::Null, stdout: Pipe::Null, stderr: Pipe::Null })
            .err()
            .expect("an error");
        let expected = format!("could not start '{}': ", missing.display());
        assert!(e.starts_with(&expected) && e.len() > expected.len(), "{e}");
        assert!(!e.contains("(os error"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn exit_is_observed_without_reaping() {
        let (argv, env) = helper("exit:0");
        let child = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Null, stderr: Pipe::Null })
            .expect("spawn");
        let pid = child.id() as libc::pid_t;
        assert_eq!(child.wait_exit(), Some(ExitInfo { code: Some(0), signal: None }));
        assert_eq!(child.exited(), Some(ExitInfo { code: Some(0), signal: None }));
        // SAFETY: signal 0 only checks that the (zombie) process exists.
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "not reaped yet");
        child.reap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "reaped");
    }

    #[cfg(unix)]
    #[test]
    fn children_lead_their_own_session() {
        let (argv, env) = helper("session");
        let c = run_captured(&argv, &env, None, Some(Duration::from_secs(30))).expect("spawn");
        assert!(String::from_utf8_lossy(&c.stdout).contains("session-leader true"), "{}", String::from_utf8_lossy(&c.stdout));
    }

    #[cfg(windows)]
    #[test]
    fn children_are_in_their_own_job() {
        // In this child's job, not just in some job (the test runner may be in one already).
        let (argv, env) = helper("sleep:5000");
        let child = ChildProc::spawn(&Spawn { argv: &argv, env: &env, stdin: Pipe::Null, stdout: Pipe::Null, stderr: Pipe::Null })
            .expect("spawn");
        let kill = child.kill_handle();
        let mut in_job = 0;
        // SAFETY: a plain query with handles the KillHandle keeps open.
        unsafe { windows_sys::Win32::System::JobObjects::IsProcessInJob(kill.0.process, kill.0.job, &mut in_job) };
        assert_ne!(in_job, 0);
        assert_eq!(child.exited(), None, "still running");
        child.kill_tree();
        assert!(child.wait_exit().is_some_and(|e| e.code == Some(1)), "TerminateJobObject(job, 1)");
        child.reap();
    }
}
