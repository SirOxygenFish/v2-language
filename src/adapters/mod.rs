//! The built-in engine adapters `py.py` and `js.js`, embedded with `include_str!` and written into
//! the run dir when their session first starts; never read from the project, `PATH` or `~/.v2`
//! (INTERNALS 14.1, 20.2).

use std::io::Write;
use std::path::PathBuf;

/// The Python adapter (14.3).
pub const PY: &str = include_str!("py.py");

/// `<run>/adapters/<file>`, created new (0600 on Unix) with `text` (14.1, 20.3). A session
/// starts once per run, so each adapter is written once.
pub fn materialize(file: &str, text: &str) -> Result<PathBuf, String> {
    let dir = crate::config::rundir::run_dir()?.join("adapters");
    let failed = |e: std::io::Error| format!("cannot write the engine adapter {file} into {}: {}", dir.display(), crate::syntax::span::os_message(&e));
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(failed(e)),
    }
    let path = dir.join(file);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(&path).and_then(|mut f| f.write_all(text.as_bytes())).map_err(failed)?;
    Ok(path)
}
