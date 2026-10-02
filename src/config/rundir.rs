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

fn create() -> Result<PathBuf, String> {
    let base = std::env::temp_dir();
    let fail = |e: Option<&Error>| {
        let why = e.map_or_else(String::new, |e| format!(": {}", crate::syntax::span::os_message(e)));
        format!("cannot create a private temp folder in {}{why}", base.display())
    };
    for _ in 0..8 {
        let bytes = random16().map_err(|e| fail(Some(&e)))?;
        let name: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let dir = base.join(format!("v2-{}-{name}", std::process::id()));
        match make_private(&dir) {
            Ok(()) => {
                crate::engines::process::set_run_dir(dir.clone());
                return Ok(dir);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(fail(Some(&e))),
        }
    }
    Err(fail(None))
}

/// POSIX: `mkdir` with mode 0700, then `lstat` must show a real directory of the effective user
/// with exactly that mode (20.3).
#[cfg(unix)]
fn make_private(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    std::fs::DirBuilder::new().mode(0o700).create(dir)?;
    let m = std::fs::symlink_metadata(dir)?;
    // SAFETY: a plain query.
    let euid = unsafe { libc::geteuid() };
    if !m.is_dir() || m.uid() != euid || m.permissions().mode() & 0o777 != 0o700 {
        return Err(Error::other("the folder is not private"));
    }
    Ok(())
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
            let dacl = dacl_of(&dir);
            assert!(dacl.starts_with("D:P(A;OICI;FA;;;S-1-"), "{dacl}");
            assert_eq!(dacl.matches("(A;").count(), 1, "one entry, for this user only: {dacl}");
        }
        std::fs::write(dir.join("file"), "x").unwrap();
        remove();
        assert!(!dir.exists());
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
