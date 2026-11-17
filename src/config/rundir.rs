//! The private run directory `<run>` (INTERNALS 20.3): created on first need, owner-only, and
//! removed on every exit path (by main, or by the watchdog after an interrupt).

#![allow(dead_code)] // the adapters are its first content (roadmap 1.4)

use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

static RUN_DIR: OnceLock<Result<PathBuf, String>> = OnceLock::new();

/// The run dir, created on first need: `temp_dir()/v2-{pid}-{32 hex}`.
pub fn run_dir() -> Result<&'static Path, String> {
    match RUN_DIR.get_or_init(create) {
        Ok(p) => Ok(p),
        Err(e) => Err(e.clone()),
    }
}

/// Remove it, if this run made one (6.5's orderly end; the interrupt path removes it too).
pub fn remove() {
    if let Some(Ok(dir)) = RUN_DIR.get() {
        crate::engines::process::remove_run_dir(dir);
    }
}

/// The variable the temp folder comes from, as the OS reads them: `TMP`, then `TEMP` on Windows,
/// `TMPDIR` elsewhere; `None` when it is the system's default.
fn temp_variable() -> Option<&'static str> {
    let set = |v: &str| std::env::var_os(v).is_some_and(|x| !x.is_empty());
    if cfg!(windows) {
        ["TMP", "TEMP"].into_iter().find(|v| set(v))
    } else {
        set("TMPDIR").then_some("TMPDIR")
    }
}

/// What is wrong with the temp folder blocks need, and what to do (20.3): `None` when it is a
/// folder. Doctor says it too (19.2).
pub fn temp_problem() -> Option<String> {
    #[cfg(windows)]
    if let Some(problem) = unset_problem() {
        return Some(problem);
    }
    let base = std::env::temp_dir();
    if base.is_dir() {
        #[cfg(windows)]
        if let Err(problem) = fitting(base) {
            return Some(problem);
        }
        return None;
    }
    let what = if base.exists() { "isn't a folder" } else { "doesn't exist" };
    let shown = shown(&base);
    Some(match temp_variable() {
        Some(var) => format!("{var} names {shown}, which {what}: blocks can't run until it names a folder you can write to"),
        None => format!("the temp folder {shown} {what}: blocks can't run until it does"),
    })
}

/// Windows: with none of `TMP`, `TEMP` and `USERPROFILE` set, as a program that starts v2 with a
/// small environment of its own leaves them, Windows offers its own folder, where a user can't
/// write (20.3).
#[cfg(windows)]
fn unset_problem() -> Option<String> {
    let set = |v: &str| std::env::var_os(v).is_some_and(|x| !x.is_empty());
    (!["TMP", "TEMP", "USERPROFILE"].into_iter().any(set))
        .then(|| "TEMP and TMP aren't set: a program that starts v2 with an environment of its own must pass them on (in Python, env=dict(os.environ, …))".to_string())
}

/// The temp folder as messages show it: without the separator Windows ends it with.
fn shown(base: &Path) -> String {
    let text = base.display().to_string();
    match text.trim_end_matches(['\\', '/']) {
        "" => text,
        trimmed => trimmed.to_string(),
    }
}

/// Windows: the longest path `<run>` adds to the temp folder, `\v2-{pid}-{32 hex}\adapters\py.py`.
#[cfg(windows)]
const INSIDE: usize = "\\v2-4294967295-0123456789abcdef0123456789abcdef\\adapters\\py.py".len();

/// Windows paths stop at 259 characters unless long paths are on, and Python can't open its
/// adapter past that (20.3). So a temp folder that leaves too little room is used by its short
/// (8.3) name when that fits, and is a problem when nothing fits.
#[cfg(windows)]
fn fitting(base: PathBuf) -> Result<PathBuf, String> {
    choose(base, long_paths, short_name)
}

/// `fitting`, with what it asks Windows passed in (the tests' seam).
#[cfg(windows)]
fn choose(base: PathBuf, long_paths: impl Fn() -> bool, short_name: impl Fn(&Path) -> Option<PathBuf>) -> Result<PathBuf, String> {
    let len = |p: &Path| shown(p).encode_utf16().count();
    if len(&base) + INSIDE <= 259 || long_paths() {
        return Ok(base);
    }
    if let Some(short) = short_name(&base).filter(|s| len(s) + INSIDE <= 259) {
        return Ok(short);
    }
    let n = shown(&base).chars().count();
    let fix = "blocks can't run until TMP and TEMP name a shorter folder, such as C:\\Temp";
    Err(match temp_variable() {
        Some(var) => format!("{var} names {}, which is {n} characters long: too long for the files blocks need there, as Windows paths stop at 260 characters; {fix}", shown(&base)),
        None => format!("the temp folder {} is {n} characters long: too long for the files blocks need there, as Windows paths stop at 260 characters; {fix}", shown(&base)),
    })
}

/// `p` as a `\\?\` path, NUL-terminated, which Windows takes at any length.
#[cfg(windows)]
fn verbatim(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let wide: Vec<u16> = abs.as_os_str().encode_wide().collect();
    let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let mut out = if wide.starts_with(&w("\\\\?\\")) {
        wide
    } else if let Some(rest) = wide.strip_prefix(&w("\\\\")[..]) {
        [w("\\\\?\\UNC\\"), rest.to_vec()].concat()
    } else {
        [w("\\\\?\\"), wide].concat()
    };
    out.push(0);
    out
}

/// The 8.3 form of an existing folder (`C:\Users\SIROXY~1\…`), when the volume keeps them.
#[cfg(windows)]
fn short_name(p: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let long = verbatim(p);
    // SAFETY: a NUL-terminated path and a buffer of the size the first call asks for.
    let n = unsafe { GetShortPathNameW(long.as_ptr(), std::ptr::null_mut(), 0) };
    if n == 0 {
        return None;
    }
    let mut buf = vec![0u16; n as usize];
    // SAFETY: as above.
    let m = unsafe { GetShortPathNameW(long.as_ptr(), buf.as_mut_ptr(), n) };
    if m == 0 || m >= n {
        return None;
    }
    buf.truncate(m as usize);
    let text = std::ffi::OsString::from_wide(&buf).to_string_lossy().into_owned();
    let plain = match (text.strip_prefix("\\\\?\\UNC\\"), text.strip_prefix("\\\\?\\")) {
        (Some(unc), _) => format!("\\\\{unc}"),
        (None, Some(rest)) => rest.to_string(),
        _ => text,
    };
    Some(PathBuf::from(plain))
}

/// Whether Windows' long paths are on (`LongPathsEnabled`), so Python and Node.js open paths
/// past 259 characters.
#[cfg(windows)]
fn long_paths() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
    let key: Vec<u16> = "SYSTEM\\CurrentControlSet\\Control\\FileSystem".encode_utf16().chain([0]).collect();
    let name: Vec<u16> = "LongPathsEnabled".encode_utf16().chain([0]).collect();
    let (mut value, mut size) = (0u32, 4u32);
    // SAFETY: NUL-terminated strings and a DWORD buffer of the size given.
    let status = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut value as *mut u32).cast(), &mut size) };
    status == 0 && value != 0
}

fn create() -> Result<PathBuf, String> {
    let base = std::env::temp_dir();
    // Windows: in a temp folder too deep for the files inside, its short (8.3) form.
    #[cfg(windows)]
    let base = fitting(base.clone()).map_err(|problem| format!("cannot create a private temp folder in {}; {problem}", shown(&base)))?;
    let fail = |e: Option<&Error>| {
        let why = e.map_or_else(String::new, |e| format!(": {}", crate::syntax::span::os_message(e)));
        // A folder made but not private: its file system has no owner and mode to set.
        let shared = e.is_some_and(|e| e.to_string().contains(NOT_PRIVATE));
        let next = match (temp_problem(), temp_variable()) {
            (Some(problem), _) => format!("; {problem}"),
            (None, Some(var)) if shared => format!("; set {var} to a folder on the computer's own file system, such as /tmp"),
            (None, Some(var)) => format!("; {var} must name a folder you can write to"),
            (None, None) => String::new(),
        };
        format!("cannot create a private temp folder in {}{why}{next}", shown(&base))
    };
    for _ in 0..8 {
        let bytes = random16().map_err(|e| fail(Some(&e)))?;
        let name: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let dir = base.join(format!("v2-{}-{name}", std::process::id()));
        match make_private(&dir) {
            Ok(()) => {
                crate::engines::process::set_run_dir(dir.clone());
                sweep_stale(&base);
                return Ok(dir);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(fail(Some(&e))),
        }
    }
    Err(fail(None))
}

/// The run dirs that runs killed outright (SIGKILL, Task Manager) left in `base`: a real folder
/// named `v2-{pid}-{32 hex}` (on POSIX this user's), whose pid isn't running, is removed, best
/// effort (20.3). A live pid keeps its folder, except on Windows, which reuses pids soon: there a
/// process that started clearly after the folder was made isn't the run that made it. A folder
/// changed within `MARGIN` stays whatever its pid says: a run that is starting may have just made it.
fn sweep_stale(base: &Path) {
    let me = std::process::id();
    let now = SystemTime::now();
    for entry in std::fs::read_dir(base).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(rest) = name.to_str().and_then(|n| n.strip_prefix("v2-")) else { continue };
        let Some((pid, hex)) = rest.split_once('-') else { continue };
        let Ok(pid) = pid.parse::<u32>() else { continue };
        if pid == me || hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let path = entry.path();
        let Ok(m) = std::fs::symlink_metadata(&path) else { continue };
        if !m.is_dir() || !owned_by_me(&m) || young(m.modified().ok(), now) || owner_running(pid, &m) {
            continue;
        }
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// How far apart a folder's time and a process's start must be to say which came first, and how
/// long a folder stays untouched before the sweep may remove it (20.3). A run makes its folder a
/// few ms after it starts, and file times are coarser than that (NTFS takes the clock's last tick,
/// up to 16 ms old), so a run's folder can look older than the run.
const MARGIN: Duration = Duration::from_secs(5);

/// Whether a folder last changed at `changed` is too new to remove (a time to come is too).
fn young(changed: Option<SystemTime>, now: SystemTime) -> bool {
    changed.is_some_and(|t| !matches!(now.duration_since(t), Ok(age) if age >= MARGIN))
}

/// Whether the run that made the folder `m` describes may still be running (20.3).
#[cfg(unix)]
fn owner_running(pid: u32, _: &std::fs::Metadata) -> bool {
    running(pid)
}

/// Windows: a process with that pid that started clearly after the folder was made has the pid
/// again.
#[cfg(windows)]
fn owner_running(pid: u32, m: &std::fs::Metadata) -> bool {
    match (started(pid), m.created()) {
        (Some(started), Ok(made)) => !reused(started, made),
        _ => running(pid),
    }
}

/// Windows: whether a process that started at `started` can't be the run that made a folder at
/// `made`: it started more than `MARGIN` later.
#[cfg(windows)]
fn reused(started: SystemTime, made: SystemTime) -> bool {
    started.duration_since(made).is_ok_and(|after| after > MARGIN)
}

/// When the process with this pid started, if it is running and can be asked.
#[cfg(windows)]
fn started(pid: u32) -> Option<std::time::SystemTime> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut made, mut ended, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: a handle we close, and FILETIMEs for the call to fill.
    let ok = unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE as u32 && GetProcessTimes(h, &mut made, &mut ended, &mut kernel, &mut user) != 0;
        CloseHandle(h);
        ok
    };
    // 100 ns units since 1601; the Unix epoch is 11,644,473,600 s later.
    let ticks = ((made.dwHighDateTime as u64) << 32 | made.dwLowDateTime as u64).checked_sub(116_444_736_000_000_000)?;
    ok.then(|| std::time::UNIX_EPOCH + std::time::Duration::from_nanos(ticks.saturating_mul(100)))
}

#[cfg(unix)]
fn running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
    // SAFETY: signal 0 only checks; EPERM means it exists under another user.
    unsafe { libc::kill(pid, 0) == 0 || Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

#[cfg(windows)]
fn running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ACCESS_DENIED, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: a handle we close; a process we may not open is taken as running.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let alive = GetExitCodeProcess(h, &mut code) == 0 || code == STILL_ACTIVE as u32;
        CloseHandle(h);
        alive
    }
}

#[cfg(unix)]
fn owned_by_me(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: a plain query.
    m.uid() == unsafe { libc::geteuid() }
}

/// Windows: the folder's protected DACL already lets only its user delete it.
#[cfg(windows)]
fn owned_by_me(_: &std::fs::Metadata) -> bool {
    true
}

/// Why a folder that was made isn't private: what a Windows drive in WSL, a FAT drive or some
/// network shares do, which show every folder as everyone's.
const NOT_PRIVATE: &str = "its file system can't keep a folder private (a Windows drive in WSL, a FAT or network drive)";

/// POSIX: `mkdir` with mode 0700, `chmod` 0700 (the umask may have cleared owner bits), then
/// `lstat` must show a real directory of the effective user with exactly that mode (20.3). A
/// folder that fails is removed again.
#[cfg(unix)]
fn make_private(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    std::fs::DirBuilder::new().mode(0o700).create(dir)?;
    let check = || -> std::io::Result<()> {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let m = std::fs::symlink_metadata(dir)?;
        // SAFETY: a plain query.
        let euid = unsafe { libc::geteuid() };
        if !m.is_dir() || m.uid() != euid || m.permissions().mode() & 0o777 != 0o700 {
            return Err(Error::other(NOT_PRIVATE));
        }
        Ok(())
    };
    check().inspect_err(|_| {
        let _ = std::fs::remove_dir(dir);
    })
}

#[cfg(unix)]
fn random16() -> std::io::Result<[u8; 16]> {
    let mut b = [0u8; 16];
    // SAFETY: a 16-byte buffer.
    if unsafe { libc::getentropy(b.as_mut_ptr().cast(), b.len()) } != 0 {
        return Err(Error::last_os_error());
    }
    Ok(b)
}

/// Windows: `CreateDirectoryW` with a protected DACL that gives only the current user full
/// control: `D:P(A;OICI;FA;;;{sid})` (20.3).
#[cfg(windows)]
fn make_private(dir: &Path) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
    let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;{})", user_sid()?).encode_utf16().chain([0]).collect();
    // As a `\\?\` path, so a deep temp folder can't make it fail.
    let path = verbatim(dir);
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: NUL-terminated strings; the descriptor is freed with LocalFree.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) == 0 {
            return Err(Error::last_os_error());
        }
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
        let ok = CreateDirectoryW(path.as_ptr(), &sa);
        let err = Error::last_os_error();
        LocalFree(sd);
        if ok == 0 {
            return Err(err);
        }
    }
    Ok(())
}

/// The current user's SID as a string, from the process token.
#[cfg(windows)]
fn user_sid() -> std::io::Result<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY: a token we own and close; the buffer is sized by the first call and aligned
    // for TOKEN_USER (u64 storage).
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(Error::last_os_error());
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
        CloseHandle(token);
        if ok == 0 {
            return Err(Error::last_os_error());
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err(Error::last_os_error());
        }
        let n = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
        LocalFree(text.cast());
        Ok(sid)
    }
}

#[cfg(windows)]
fn random16() -> std::io::Result<[u8; 16]> {
    use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    let mut b = [0u8; 16];
    // SAFETY: a 16-byte buffer and the system RNG.
    let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), b.as_mut_ptr(), 16, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if status != 0 {
        return Err(Error::other(format!("BCryptGenRandom failed (0x{status:08X})")));
    }
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_dirs_of_killed_runs_are_swept() {
        // 20.3: what a run killed outright left is removed by the next run that makes a run dir;
        // a live pid's folder, one changed within MARGIN, and anything else, stay.
        let base = std::env::temp_dir().join(format!("v2sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // A pid that has ended: a child that ran and was waited for.
        let mut child = std::process::Command::new(std::env::current_exe().unwrap()).arg("--list").stdout(std::process::Stdio::null()).spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        let hex = "0123456789abcdef0123456789abcdef";
        let stale = base.join(format!("v2-{dead}-{hex}"));
        let fresh = base.join(format!("v2-{dead}-{}", "f".repeat(32)));
        let live = base.join(format!("v2-{}-{hex}", std::process::id()));
        let other = base.join(format!("v2-{dead}-not-hex"));
        for d in [&stale, &fresh, &live, &other] {
            std::fs::create_dir_all(d.join("adapters")).unwrap();
            std::fs::write(d.join("adapters").join("py.py"), "x").unwrap();
        }
        for d in [&stale, &live, &other] {
            backdate(d, false);
        }
        assert!(!running(dead) && running(std::process::id()));
        // Windows reuses pids: a folder made long before the process that has its pid now
        // started is a killed run's; one made 50 ms before it may be its own, as file times are
        // coarse and a run makes its folder a few ms after it starts.
        #[cfg(windows)]
        let (mut later, reused, close) = {
            let (made, just) = (base.join("made first"), base.join("made just before"));
            for d in [&made, &just] {
                std::fs::create_dir_all(d.join("adapters")).unwrap();
            }
            std::thread::sleep(Duration::from_millis(50));
            let child = std::process::Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
            let (reused, close) = (base.join(format!("v2-{}-{hex}", child.id())), base.join(format!("v2-{}-{}", child.id(), "e".repeat(32))));
            std::fs::rename(&made, &reused).unwrap();
            std::fs::rename(&just, &close).unwrap();
            backdate(&reused, true);
            backdate(&close, false);
            (child, reused, close)
        };
        sweep_stale(&base);
        assert!(!stale.exists(), "the killed run's folder goes");
        assert!(fresh.exists(), "a folder changed within MARGIN stays");
        assert!(live.exists() && other.exists(), "a running pid's folder and other names stay");
        #[cfg(windows)]
        {
            assert!(!reused.exists(), "a pid that a later process has now");
            assert!(close.exists(), "a folder made a few ms before its process started");
            let _ = later.kill();
            let _ = later.wait();
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_folder_made_as_its_run_starts_is_kept() {
        // 20.3 (round 20): runs started together removed each other's folders, as a folder made
        // a few ms after its run started can look older than the run. Only a process that started
        // more than MARGIN after the folder has its pid again; a folder is young within MARGIN.
        let now = SystemTime::now();
        #[cfg(windows)]
        {
            assert!(!reused(now, now - Duration::from_millis(3)), "a folder made a few ms before its process started");
            assert!(!reused(now, now + Duration::from_millis(3)), "a folder made after its process started");
            assert!(reused(now, now - Duration::from_secs(3600)), "a folder made long before");
        }
        assert!(young(Some(now - Duration::from_millis(3)), now) && young(Some(now + Duration::from_secs(1)), now));
        assert!(!young(Some(now - Duration::from_secs(60)), now) && !young(None, now));
    }

    /// Set a folder's last change an hour back, and on Windows with `created` its creation too.
    fn backdate(dir: &Path, created: bool) {
        let hour_ago = SystemTime::now() - Duration::from_secs(3600);
        let mut open = std::fs::OpenOptions::new();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_WRITE_ATTRIBUTES, with FILE_FLAG_BACKUP_SEMANTICS, which opens a folder.
            open.access_mode(0x100).custom_flags(0x0200_0000);
        }
        #[cfg(unix)]
        open.read(true);
        let times = std::fs::FileTimes::new().set_modified(hour_ago);
        #[cfg(windows)]
        let times = if created { std::os::windows::fs::FileTimesExt::set_created(times, hour_ago) } else { times };
        #[cfg(unix)]
        let _ = created;
        open.open(dir).unwrap().set_times(times).unwrap();
    }

    #[test]
    fn the_run_dir_is_private_named_and_removed() {
        let dir = run_dir().expect("a run dir").to_path_buf();
        assert_eq!(run_dir().unwrap(), dir, "created once");
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let prefix = format!("v2-{}-", std::process::id());
        assert!(name.starts_with(&prefix), "{name}");
        let hex = &name[prefix.len()..];
        assert!(hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()), "{name}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let m = std::fs::symlink_metadata(&dir).unwrap();
            assert_eq!(m.permissions().mode() & 0o777, 0o700);
            assert_eq!(m.uid(), unsafe { libc::geteuid() });
        }
        #[cfg(windows)]
        {
            // One entry, for this user only: a SID, or its two-letter alias (`LA` when the user
            // is the built-in Administrator, as on CI).
            let dacl = dacl_of(&dir);
            let who = dacl.strip_prefix("D:P(A;OICI;FA;;;").and_then(|r| r.strip_suffix(')')).unwrap_or_default();
            let alias = who.len() == 2 && who.bytes().all(|b| b.is_ascii_uppercase());
            assert!(who.starts_with("S-1-") || alias, "{dacl}");
            assert_eq!(dacl.matches("(A;").count(), 1, "{dacl}");
        }
        std::fs::write(dir.join("file"), "x").unwrap();
        remove();
        assert!(!dir.exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_deep_temp_folder_is_used_by_its_short_name() {
        // 20.3: a temp folder whose path leaves no room for the run dir's files (259 characters
        // with long paths off) is used by its 8.3 name; with long paths on, or a short name that
        // fits, as it is; with neither, the problem says how long it is and what to do.
        let deep = std::env::temp_dir().join(format!("v2deep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&deep);
        let mut dir = deep.clone();
        while dir.as_os_str().len() < 230 {
            dir.push("a folder with a long name");
        }
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(choose(dir.clone(), || true, |_| None), Ok(dir.clone()), "long paths on");
        let short = PathBuf::from(r"C:\Users\ME~1\AppData\Local\Temp\V2DEEP~1");
        assert_eq!(choose(dir.clone(), || false, |_| Some(short.clone())), Ok(short));
        let n = dir.display().to_string().chars().count();
        let problem = choose(dir.clone(), || false, |_| None).unwrap_err();
        assert!(problem.contains(&format!(", which is {n} characters long: too long for the files blocks need there")), "{problem}");
        assert!(problem.ends_with("blocks can't run until TMP and TEMP name a shorter folder, such as C:\\Temp"), "{problem}");
        // Windows' own short name, when the volume keeps 8.3 names, is the same folder.
        if let Some(s) = short_name(&dir) {
            assert!(s.as_os_str().len() < dir.as_os_str().len(), "{}", s.display());
            assert_eq!(std::fs::canonicalize(&s).unwrap(), std::fs::canonicalize(&dir).unwrap());
        }
        // The run dir itself is made at any depth (a `\\?\` path).
        let private = dir.join(format!("v2-{}-0123456789abcdef0123456789abcdef", std::process::id()));
        make_private(&private).expect("a private folder past 248 characters");
        std::fs::write(private.join("x"), "x").unwrap();
        let _ = std::fs::remove_dir_all(&deep);
    }

    #[cfg(unix)]
    #[test]
    fn a_umask_does_not_change_the_mode() {
        use std::os::unix::fs::PermissionsExt;
        // The umask is process-wide, so the test binary re-runs itself to set one.
        if std::env::var_os("V2_UMASK_CHILD").is_some() {
            // SAFETY: a plain call; this process runs only this test.
            unsafe { libc::umask(0o277) };
            let dir = run_dir().expect("a private run dir despite the umask");
            let mode = std::fs::symlink_metadata(dir).unwrap().permissions().mode() & 0o777;
            remove();
            println!("\nmode {mode:o}");
            std::process::exit(0);
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "config::rundir::tests::a_umask_does_not_change_the_mode", "--nocapture", "--test-threads=1"])
            .env("V2_UMASK_CHILD", "1")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("mode 700"), "{text}{}", String::from_utf8_lossy(&out.stderr));
    }

    /// The directory's DACL in SDDL.
    #[cfg(windows)]
    fn dacl_of(dir: &Path) -> String {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{ConvertSecurityDescriptorToStringSecurityDescriptorW, SDDL_REVISION_1};
        use windows_sys::Win32::Security::{GetFileSecurityW, DACL_SECURITY_INFORMATION};
        let path: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: sized buffers and NUL-terminated strings; the string is freed with LocalFree.
        unsafe {
            let mut len = 0u32;
            GetFileSecurityW(path.as_ptr(), DACL_SECURITY_INFORMATION, std::ptr::null_mut(), 0, &mut len);
            let mut sd = vec![0u64; (len as usize).div_ceil(8)];
            assert_ne!(GetFileSecurityW(path.as_ptr(), DACL_SECURITY_INFORMATION, sd.as_mut_ptr().cast(), len, &mut len), 0);
            let mut text: *mut u16 = std::ptr::null_mut();
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(sd.as_mut_ptr().cast(), SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut text, std::ptr::null_mut()),
                0
            );
            let n = (0..).take_while(|&i| *text.add(i) != 0).count();
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
            LocalFree(text.cast());
            s
        }
    }
}
