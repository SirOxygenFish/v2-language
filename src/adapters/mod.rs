//! The built-in engine adapters `py.py` and `js.js`, embedded with `include_str!` and written into
//! the run dir when their session first starts; never read from the project, `PATH` or `~/.v2`
//! (INTERNALS 14.1, 20.2).

use std::io::Write;
use std::path::PathBuf;

/// The Python adapter (14.3).
pub const PY: &str = include_str!("py.py");

/// The JavaScript adapter, which also runs ts blocks (14.4).
pub const JS: &str = include_str!("js.js");

/// The adapter a built-in bridged engine's session runs: its file name and text.
pub fn for_session(key: &str) -> Option<(&'static str, &'static str)> {
    match key {
        "py" => Some(("py.py", PY)),
        "js" => Some(("js.cjs", JS)),
        _ => None,
    }
}

/// `<run>/adapters/<file>`, created new (0600 on Unix) with `text` (14.1, 20.3). A session
/// starts once per run, so each adapter is written once.
pub fn materialize(file: &str, text: &str) -> Result<PathBuf, String> {
    let dir = crate::config::rundir::run_dir()?.join("adapters");
    let failed = |e: std::io::Error| {
        let lang = if file.starts_with("py") { "Python" } else { "Node.js" };
        let why = crate::syntax::span::os_message(&e);
        // ENOSPC (Linux, macOS) and ERROR_DISK_FULL / ERROR_HANDLE_DISK_FULL (Windows).
        let full = if cfg!(windows) { matches!(e.raw_os_error(), Some(112 | 39)) } else { e.raw_os_error() == Some(28) };
        if full {
            let temp = dir.parent().and_then(|d| d.parent()).unwrap_or(&dir);
            let var = if cfg!(windows) { "TEMP" } else { "TMPDIR" };
            return format!("cannot start {lang}: the temp folder {} is full ({why}): free some space there, or set {var} to a folder with room", temp.display());
        }
        format!("cannot start {lang}: V2 couldn't write its worker for it into {}: {why}", dir.display())
    };
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(failed(e)),
    }
    #[cfg(unix)]
    std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).map_err(failed)?;
    let path = dir.join(file);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let write = |mut f: std::fs::File| {
        #[cfg(unix)]
        f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        f.write_all(text.as_bytes())
    };
    options.open(&path).and_then(write).map_err(failed)?;
    Ok(path)
}
