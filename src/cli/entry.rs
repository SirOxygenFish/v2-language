//! What to do next when the script itself is the problem (INTERNALS 19.1): a FILE that isn't
//! there or is a folder, and a file in another language given to V2.

use std::path::{Path, PathBuf};

use crate::runtime::error::did_you_mean;

/// Words people type expecting a command V2 doesn't have; with no such file they get a hint.
const NOT_COMMANDS: &[&str] = &[
    "build", "compile", "format", "fmt", "init", "install", "lint", "new", "repl", "setup", "test", "uninstall", "update",
    "upgrade",
];

/// A file with another extension over this size is data given to V2 by mistake (`v2 data.csv`
/// for `v2 script.v2 data.csv`), refused before it is read whole.
const DATA_LIMIT: u64 = 1 << 20;

/// The message for an entry FILE that is missing, a folder, not a regular file, or a large file
/// that isn't a script, ending with what to do next; `None` when FILE is fine (the loader reports
/// anything else). `next` is the argument after FILE, for `v2 run hello.v2`; `command` is how the
/// suggestions start (`v2`, or `v2 check`).
pub fn entry_error(file: &str, next: Option<&str>, command: &str) -> Option<String> {
    // Windows's usual help switch; `?` can't be in a Windows file name.
    if file == "/?" {
        return Some("'/?' isn't a V2 command\nfor help, type: v2 --help".into());
    }
    let path = Path::new(file);
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => Some(folder_message(file, path, command)),
        Ok(m) if !m.is_file() => Some(format!("'{file}' is a pipe or a device, not a file: V2 runs scripts saved as .v2 files")),
        Ok(m) if m.len() > DATA_LIMIT && path.extension().is_some_and(|x| !x.eq_ignore_ascii_case("v2")) => Some(format!(
            "'{file}' is {} MB and isn't a .v2 script\nto give it to a script, type: v2 SCRIPT.v2 {}",
            m.len().div_ceil(1 << 20),
            quoted(file)
        )),
        Ok(_) => None,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(missing_message(file, path, next)),
        Err(_) => None,
    }
}

fn folder_message(file: &str, path: &Path, command: &str) -> String {
    let scripts = v2_files(path);
    let pick = scripts.iter().find(|n| n.eq_ignore_ascii_case("main.v2")).or(scripts.first());
    match pick {
        Some(name) => {
            let sep = if file.ends_with('/') || file.ends_with(std::path::MAIN_SEPARATOR) { "" } else { std::path::MAIN_SEPARATOR_STR };
            let what = if command == "v2" { "run" } else { "check" };
            format!("'{file}' is a folder, not a script\n{what} one of its .v2 files, for example: {command} {}", quoted(&format!("{file}{sep}{name}")))
        }
        None => format!("'{file}' is a folder, not a script, and it has no .v2 files"),
    }
}

fn missing_message(file: &str, path: &Path, next: Option<&str>) -> String {
    let folder = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let first = if path.is_absolute() {
        format!("cannot find '{file}'")
    } else {
        let shown = std::env::current_dir().map(|cwd| shown_folder(&cwd.join(&folder))).unwrap_or_else(|_| folder.display().to_string());
        format!("cannot find '{file}' in {shown}")
    };
    format!("{first}\n{}", missing_hint(file, path, &folder, next))
}

/// The one hint for a missing FILE: the command it looks like, the file it probably meant, or
/// what is there.
fn missing_hint(file: &str, path: &Path, folder: &Path, next: Option<&str>) -> String {
    let word = file.to_ascii_lowercase();
    let plain = !file.contains(['/', '\\']) && path.extension().is_none();
    if plain {
        match word.as_str() {
            "run" | "exec" | "start" => {
                return match next {
                    Some(script) => format!("there is no '{file}' command: to run {script}, type: v2 {}", quoted(script)),
                    None => format!("there is no '{file}' command: to run a script, type: v2 FILE (for example: v2 hello.v2)"),
                };
            }
            "help" => return "for help, type: v2 --help".into(),
            "version" => return "for the version, type: v2 --version".into(),
            w if NOT_COMMANDS.contains(&w) => {
                return format!("'{file}' isn't a V2 command; the commands are check, doctor and cache clean (see v2 --help)");
            }
            _ => {}
        }
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if path.extension().is_none() && folder.join(format!("{name}.v2")).is_file() {
        return format!("did you mean '{file}.v2'?");
    }
    if folder.join(format!("{name}.txt")).is_file() {
        return format!("there is a '{name}.txt': your editor added '.txt', so rename it to '{name}'");
    }
    if path.extension().is_none() && folder.join(format!("{name}.v2.txt")).is_file() {
        return format!("there is a '{name}.v2.txt': your editor added '.txt', so rename it to '{name}.v2'");
    }
    let scripts = v2_files(folder);
    if let Some(close) = did_you_mean(&name, scripts.iter().map(String::as_str)) {
        let suggestion = match file.rfind(['/', '\\']) {
            Some(i) => format!("{}{close}", &file[..=i]),
            None => close.to_string(),
        };
        return format!("did you mean '{suggestion}'?");
    }
    match scripts.len() {
        0 => "there are no .v2 files there: cd to the folder that holds your script, or give its full path".into(),
        n if n <= 5 => format!("the .v2 files there: {}", scripts.join(", ")),
        _ => format!("the .v2 files there: {}, …", scripts[..5].join(", ")),
    }
}

/// The `.v2` files directly in `folder`, sorted.
fn v2_files(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| Path::new(n).extension().is_some_and(|x| x.eq_ignore_ascii_case("v2")))
        .collect();
    names.sort();
    names
}

/// A folder as messages show it: absolute, without Windows's `\\?\`.
fn shown_folder(p: &Path) -> String {
    let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    crate::syntax::span::strip_verbatim(&p).display().to_string()
}

/// A path as the user would type it in a shell: quoted when it has a space.
fn quoted(s: &str) -> String {
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

/// A note for a file in another language given to V2, printed after its errors: `None` for
/// `.v2` files and extensions V2 doesn't know.
pub fn foreign_file_note(file: &str) -> Option<String> {
    let ext = Path::new(file).extension()?.to_string_lossy().to_ascii_lowercase();
    let (language, tag) = match ext.as_str() {
        "py" => ("Python", "py"),
        "js" | "mjs" | "cjs" => ("JavaScript", "js"),
        "ts" | "mts" | "cts" => ("TypeScript", "ts"),
        "c" | "h" => ("C", "c"),
        "cpp" | "cc" | "cxx" | "hpp" => ("C++", "cpp"),
        "rs" => ("Rust", "rust"),
        _ => return None,
    };
    Some(match tag {
        "py" | "js" | "ts" => format!(
            "note: '{file}' is a {language} file, and V2 runs .v2 scripts: put {language} code in a @{tag} {{ }} block in a .v2 file"
        ),
        _ => format!("note: '{file}' is a {language} file, and V2 runs .v2 scripts ({language} blocks aren't in this version of V2 yet)"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("v2entry-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_files_get_one_hint() {
        let d = temp("missing");
        for f in ["hello.v2", "notes.v2.txt", "b.v2"] {
            std::fs::write(d.join(f), "print(1)\n").unwrap();
        }
        let at = |f: &str| d.join(f).to_string_lossy().into_owned();
        let hint = |f: &str, next: Option<&str>| {
            let m = entry_error(&at(f), next, "v2").expect("missing");
            m.lines().nth(1).unwrap().to_string()
        };
        assert!(entry_error(&at("hello.v2"), None, "v2").is_none());
        // `at("")` is the folder with a separator at the end.
        let dir = at("");
        assert_eq!(hint("hello", None), format!("did you mean '{dir}hello.v2'?"));
        assert_eq!(hint("notes.v2", None), "there is a 'notes.v2.txt': your editor added '.txt', so rename it to 'notes.v2'");
        assert_eq!(hint("helo.v2", None), format!("did you mean '{dir}hello.v2'?"));
        assert_eq!(hint("zzzzzz.v2", None), "the .v2 files there: b.v2, hello.v2");
        // `v2 notes` when only Notepad's notes.v2.txt is there.
        assert_eq!(hint("notes", None), "there is a 'notes.v2.txt': your editor added '.txt', so rename it to 'notes.v2'");
        // Data given to V2 by mistake is refused before it is read.
        std::fs::write(d.join("data.csv"), vec![b'1'; (DATA_LIMIT + 1) as usize]).unwrap();
        let m = entry_error(&at("data.csv"), None, "v2").unwrap();
        assert!(m.starts_with(&format!("'{}' is 2 MB and isn't a .v2 script\nto give it to a script, type: v2 SCRIPT.v2 ", at("data.csv"))), "{m}");
        let empty = temp("empty");
        let m = entry_error(&empty.join("x.v2").to_string_lossy(), None, "v2").unwrap();
        assert!(m.ends_with("there are no .v2 files there: cd to the folder that holds your script, or give its full path"), "{m}");
    }

    #[test]
    fn command_words_point_to_the_command() {
        // Run from a folder that has none of these files (the test binary's cwd is the crate).
        let hint = |f: &str, next: Option<&str>| entry_error(f, next, "v2").map(|m| m.lines().nth(1).unwrap().to_string());
        assert_eq!(hint("run", Some("hello.v2")).unwrap(), "there is no 'run' command: to run hello.v2, type: v2 hello.v2");
        assert_eq!(hint("run", Some("my file.v2")).unwrap(), "there is no 'run' command: to run my file.v2, type: v2 \"my file.v2\"");
        assert_eq!(hint("help", None).unwrap(), "for help, type: v2 --help");
        assert_eq!(hint("version", None).unwrap(), "for the version, type: v2 --version");
        assert_eq!(hint("install", None).unwrap(), "'install' isn't a V2 command; the commands are check, doctor and cache clean (see v2 --help)");
    }

    #[test]
    fn a_folder_names_a_script_in_it() {
        let d = temp("folder");
        let shown = d.to_string_lossy().into_owned();
        assert!(entry_error(&shown, None, "v2").unwrap().ends_with("is a folder, not a script, and it has no .v2 files"));
        std::fs::write(d.join("util.v2"), "").unwrap();
        std::fs::write(d.join("main.v2"), "").unwrap();
        let m = entry_error(&shown, None, "v2").unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        let want = format!("{shown}{sep}main.v2");
        assert!(m.ends_with(&format!("run one of its .v2 files, for example: v2 {}", quoted(&want))), "{m}");
        // `v2 check folder` suggests checking, not running.
        let m = entry_error(&shown, None, "v2 check").unwrap();
        assert!(m.ends_with(&format!("check one of its .v2 files, for example: v2 check {}", quoted(&want))), "{m}");
    }

    #[test]
    fn foreign_files_get_a_note() {
        assert_eq!(
            foreign_file_note("app.py").unwrap(),
            "note: 'app.py' is a Python file, and V2 runs .v2 scripts: put Python code in a @py { } block in a .v2 file"
        );
        assert!(foreign_file_note("x.MJS").unwrap().contains("@js { }"));
        assert!(foreign_file_note("x.rs").unwrap().ends_with("(Rust blocks aren't in this version of V2 yet)"));
        assert!(foreign_file_note("x.v2").is_none() && foreign_file_note("x").is_none() && foreign_file_note("x.txt").is_none());
    }
}
