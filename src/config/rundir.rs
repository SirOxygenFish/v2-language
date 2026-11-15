//! The private run directory `<run>` (INTERNALS 20.3): created on first need, owner-only, and
//! removed on every exit path (by main, or by the watchdog after an interrupt).

#![allow(dead_code)] // the adapters are its first content (roadmap 1.4)

use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
    let base = std::env::temp_dir();
    if base.is_dir() {
        return None;
    }
    let what = if base.exists() { "isn't a folder" } else { "doesn't exist" };
    let shown = shown(&base);
    Some(match temp_variable() {
        Some(var) => format!("{var} names {shown}, which {what}: blocks can't run until it names a folder you can write to"),
        None => format!("the temp folder {shown} {what}: blocks can't run until it does"),
    })
}

/// The temp folder as messages show it: without the separator Windows ends it with.
fn shown(base: &Path) -> String {
    let text = base.display().to_string();
    match text.trim_end_matches(['\\', '/']) {
        "" => text,
        trimmed => trimmed.to_string(),
    }
}

fn create() -> Result<PathBuf, String> {
    let base = std::env::temp_dir();
    let fail = |e: Option<&Error>| {
        let why = e.map_or_else(String::new, |e| format!(": {}", crate::syntax::span::os_message(e)));
        let next = match (temp_problem(), temp_variable()) {
            (Some(problem), _) => format!("; {problem}"),
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
/// effort (20.3). A live pid, even one reused by another program, keeps its folder.
fn sweep_stale(base: &Path) {
    let me = std::process::id();
    for entry in std::fs::read_dir(base).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(rest) = name.to_str().and_then(|n| n.strip_prefix("v2-")) else { continue };
        let Some((pid, hex)) = rest.split_once('-') else { continue };
        let Ok(pid) = pid.parse::<u32>() else { continue };
        if pid == me || hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) || running(pid) {
            continue;
        }
        let path = entry.path();
        let Ok(m) = std::fs::symlink_metadata(&path) else { continue };
        if !m.is_dir() || !owned_by_me(&m) {
            continue;
        }
        let _ = std::fs::remove_dir_all(&path);
    }
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
            return Err(Error::other("the folder is not private"));
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
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
    let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;{})", user_sid()?).encode_utf16().chain([0]).collect();
    let path: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
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
        // a live pid's folder, and anything else, stay.
        let base = std::env::temp_dir().join(format!("v2sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // A pid that has ended: a child that ran and was waited for.
        let mut child = std::process::Command::new(std::env::current_exe().unwrap()).arg("--list").stdout(std::process::Stdio::null()).spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        let hex = "0123456789abcdef0123456789abcdef";
        let stale = base.join(format!("v2-{dead}-{hex}"));
        let live = base.join(format!("v2-{}-{hex}", std::process::id()));
        let other = base.join(format!("v2-{dead}-not-hex"));
        for d in [&stale, &live, &other] {
            std::fs::create_dir_all(d.join("adapters")).unwrap();
            std::fs::write(d.join("adapters").join("py.py"), "x").unwrap();
        }
        assert!(!running(dead) && running(std::process::id()));
        sweep_stale(&base);
        assert!(!stale.exists(), "the killed run's folder goes");
        assert!(live.exists() && other.exists(), "a running pid's folder and other names stay");
        let _ = std::fs::remove_dir_all(&base);
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
