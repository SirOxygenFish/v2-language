//! What to do next when the script itself is the problem (INTERNALS 19.1): a FILE that isn't
//! there or is a folder, and a file in another language given to V2.

use std::path::{Path, PathBuf};

use crate::runtime::error::did_you_mean;
use crate::syntax::span::{os_message, shown_path};

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
/// anything else). `rest` is the arguments after FILE, for `v2 run hello.v2`; `command` is how the
/// suggestions start (`v2`, or `v2 check`). Each line has control characters escaped, so a file
/// name that holds one can't split it or act on the terminal (2.1).
pub fn entry_error(file: &str, rest: &[&str], command: &str) -> Option<String> {
    let lines = entry_lines(file, rest, command)?;
    Some(lines.iter().map(|l| shown_path(l)).collect::<Vec<_>>().join("\n"))
}

fn entry_lines(file: &str, rest: &[&str], command: &str) -> Option<Vec<String>> {
    // Windows's usual help switch; `?` can't be in a Windows file name.
    if file == "/?" {
        return Some(vec!["'/?' isn't a V2 command".into(), "for help, type: v2 --help".into()]);
    }
    let path = Path::new(file);
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => Some(folder_message(file, path, command)),
        Ok(m) if !m.is_file() => Some(vec![format!("'{file}' is a pipe or a device, not a file: V2 runs scripts saved as .v2 files")]),
        Ok(m) if m.len() > DATA_LIMIT && path.extension().is_some_and(|x| !x.eq_ignore_ascii_case("v2")) => Some(vec![
            format!("'{file}' is {} MB and isn't a .v2 script", m.len().div_ceil(1 << 20)),
            format!("to give it to a script, type: v2 SCRIPT.v2 {}", quoted(file)),
        ]),
        Ok(_) => None,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(missing_message(file, path, rest, command)),
        Err(e) => other_message(file, path, rest, command, &e),
    }
}

/// FILE that the OS can't even look for: through a link in a loop (Windows says "The name of the
/// file cannot be resolved by the system"), or, on Windows, a wildcard that cmd and PowerShell
/// pass on as it is ("The filename, directory name, or volume label syntax is incorrect"). Git
/// Bash expands one, so there it matched nothing. `None`: the loader says what the OS said.
fn other_message(file: &str, path: &Path, rest: &[&str], command: &str, e: &std::io::Error) -> Option<Vec<String>> {
    if let Some(link) = crate::runtime::module::link_problem(path) {
        return Some(vec![format!("cannot find '{file}'"), link_line(file, path, &link)]);
    }
    if !cfg!(windows) || !file.contains(['*', '?']) || e.kind() == std::io::ErrorKind::PermissionDenied {
        return None;
    }
    if std::env::var_os("MSYSTEM").is_some() {
        return Some(missing_message(file, path, rest, command));
    }
    let first = |what: &str| format!("cannot find '{file}': cmd and PowerShell don't expand * and ?, and {what}");
    if command == "v2" {
        let folder = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        return Some(vec![first("v2 runs one script"), files_there(folder)]);
    }
    // The README's loop, over FILE's pattern (in quotes only for what PowerShell would split).
    let name = if file.contains(['/', '\\']) { "FullName" } else { "Name" };
    let pattern = if file.chars().all(|c| c.is_alphanumeric() || "._-+~\\/:*?".contains(c)) { file.to_string() } else { quoted(file) };
    Some(vec![
        first("v2 check takes one file"),
        format!("to check several, loop in PowerShell: $ok = 0; Get-ChildItem {pattern} | ForEach-Object {{ v2 check $_.{name}; if ($LASTEXITCODE -ne 0) {{ $ok = 1 }} }}"),
    ])
}

/// The line for a link on FILE's way that leads nowhere or in a loop, and what fixes it.
fn link_line(file: &str, path: &Path, link: &crate::runtime::module::LinkProblem) -> String {
    let written = crate::runtime::module::written_prefix(file, &link.at);
    let name = if link.at == path { "it".to_string() } else { format!("'{written}'") };
    format!("{}: {}", link.problem(&name), link.fix(&written))
}

fn folder_message(file: &str, path: &Path, command: &str) -> Vec<String> {
    let scripts = match v2_files(path) {
        Ok(s) => s,
        // A folder V2 may not list (a deny on it): not one without .v2 files.
        Err(e) => {
            let sep = if file.ends_with('/') || file.ends_with(std::path::MAIN_SEPARATOR) { "" } else { std::path::MAIN_SEPARATOR_STR };
            let what = if command == "v2" { "run" } else { "check" };
            return vec![
                format!("'{file}' is a folder, not a script, and V2 can't list it: {}", os_message(&e)),
                format!("{what} a .v2 file in it by its name: {command} {}", quoted(&format!("{file}{sep}FILE.v2"))),
            ];
        }
    };
    let pick = scripts.iter().find(|n| n.eq_ignore_ascii_case("main.v2")).or(scripts.first());
    match pick {
        Some(name) => {
            let sep = if file.ends_with('/') || file.ends_with(std::path::MAIN_SEPARATOR) { "" } else { std::path::MAIN_SEPARATOR_STR };
            let what = if command == "v2" { "run" } else { "check" };
            vec![format!("'{file}' is a folder, not a script"), format!("{what} one of its .v2 files, for example: {command} {}", quoted(&format!("{file}{sep}{name}")))]
        }
        None => {
            // Notepad's `hallo.v2.txt`, as `v2 hallo.v2` inside that folder says too.
            let saved_txt = std::fs::read_dir(path).into_iter().flatten().flatten().filter_map(|e| e.file_name().into_string().ok()).find(|n| n.ends_with(".v2.txt"));
            let first = format!("'{file}' is a folder, not a script, and it has no .v2 files");
            match saved_txt {
                Some(t) => vec![first, format!("there is a '{t}': your editor added '.txt', so rename it to '{}'", t.trim_end_matches(".txt"))],
                None => vec![first],
            }
        }
    }
}

fn missing_message(file: &str, path: &Path, rest: &[&str], command: &str) -> Vec<String> {
    let folder = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    // The file's own name in its folder: `sub/x.v2` is `x.v2` in `…/sub`, not `sub/x.v2` there.
    let name = path.file_name().map_or_else(|| file.to_string(), |n| n.to_string_lossy().into_owned());
    // A terminal still in a folder that was deleted: nothing relative can be found there.
    if !path.is_absolute() && std::env::current_dir().is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return vec![
            format!("cannot find '{file}': the current folder no longer exists (it was deleted or moved)"),
            "cd to another folder, or give the script's full path".into(),
        ];
    }
    let first = if path.is_absolute() {
        format!("cannot find '{file}'")
    } else {
        let shown = std::env::current_dir().map(|cwd| shown_folder(&cwd.join(&folder))).unwrap_or_else(|_| folder.display().to_string());
        format!("cannot find '{name}' in {shown}")
    };
    // A link to a file that isn't there is listed in its folder, but can't be run: where it points
    // (absolute: a relative target is from the link's folder) and what fixes it (DOCS 3.10).
    if let Some(link) = crate::runtime::module::link_problem(path).filter(|l| l.at == path) {
        return vec![first, format!("{}: {}", link.problem(&format!("'{name}'")), link.fix(file))];
    }
    // A link on the way that leads nowhere (a junction whose folder was removed), which is listed
    // in its folder but is no folder to look in (DOCS 3.10).
    if let Some(link) = crate::runtime::module::link_problem(path) {
        return vec![format!("cannot find '{file}'"), link_line(file, path, &link)];
    }
    // Windows: Git Bash's `/c/Users/…` pasted into cmd or PowerShell, which read it as `\c\…` on
    // this drive; the file builtins say the same (5.10).
    if let Some(drive_path) = git_bash_path(file) {
        let letter = &file[1..2];
        return vec![
            format!("cannot find '{file}'"),
            format!("on Windows, /{}/… is Git Bash's spelling of {}:/…: run {command} {}", letter.to_ascii_lowercase(), letter.to_ascii_uppercase(), quoted(&drive_path)),
        ];
    }
    // A folder on the way that isn't there: say so, with the one it probably meant.
    if !folder.is_dir() {
        let mut missing = folder.clone();
        while let Some(parent) = missing.parent().filter(|p| !p.as_os_str().is_empty() && !p.is_dir()) {
            missing = parent.to_path_buf();
        }
        let shown = std::env::current_dir().map(|cwd| absolute_shown(&cwd.join(&missing))).unwrap_or_else(|_| missing.display().to_string());
        let name = missing.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let parent = missing.parent().filter(|p| !p.as_os_str().is_empty()).map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let folders: Vec<String> = std::fs::read_dir(&parent)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        // A Mac's `größe` for `größe` as typed comes first: a did-you-mean would look the same.
        let hint = match (crate::runtime::module::unicode_twin(&parent, &name), did_you_mean(&name, folders.iter().map(String::as_str))) {
            (Some(twin), _) => format!("there is no folder {shown}: {twin}"),
            (None, Some(close)) => format!("there is no folder {shown}: did you mean '{close}'?"),
            (None, None) => format!("there is no folder {shown}"),
        };
        return vec![format!("cannot find '{file}'"), hint];
    }
    vec![first, missing_hint(file, path, &folder, rest, command)]
}

/// A path as messages show it, made absolute against the current folder and with `.` and `..`
/// resolved where they can be, but not required to exist.
fn absolute_shown(p: &Path) -> String {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    crate::syntax::span::strip_verbatim(&out).display().to_string()
}

/// The one hint for a missing FILE: the command it looks like, the file it probably meant, or
/// what is there.
fn missing_hint(file: &str, path: &Path, folder: &Path, rest: &[&str], command: &str) -> String {
    let next = rest.first().copied();
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
    // `größe.v2` copied from a Mac, its `ö` an `o` plus an accent: it looks like the name typed
    // (or like NAME`.v2`, for `v2 größe`, which also lacks the .v2: the run that works then).
    if let Some(twin) = crate::runtime::module::unicode_twin(folder, &name) {
        return twin;
    }
    if path.extension().is_none() {
        if let Some(twin) = crate::runtime::module::unicode_twin(folder, &format!("{name}.v2")) {
            return format!("{twin}, and run {command} {}", quoted(&crate::runtime::module::composed_v2(file, &name)));
        }
    }
    // `v2 mein spiel.v2`, `v2 my mod [v1].v2 arg`: the shell split a name with spaces; the
    // longest run of the words after FILE that makes a file's name.
    if let Some(joined) = (1..=rest.len()).rev().map(|k| rest[..k].join(" ")).find(|n| folder.join(format!("{name} {n}")).is_file()) {
        return format!("the file's name has a space, so the shell split it: put it in quotes, as in v2 \"{file} {joined}\"");
    }
    if path.extension().is_none() && folder.join(format!("{name}.v2")).is_file() {
        return format!("did you mean '{file}.v2'?");
    }
    // `v2 doctr`, `v2 chek x.v2`: a command, misspelled; `v2 CHECK x.v2`, in another case.
    let commands = ["check", "doctor", "cache"];
    let cased = commands.into_iter().find(|c| *c == word);
    if let Some(command) = cased.or_else(|| did_you_mean(&word, commands)).filter(|_| plain) {
        let rest = match (command, next) {
            ("check", Some(script)) => format!(" {}", quoted(script)),
            ("check", None) => " FILE".to_string(),
            ("cache", _) => " clean".to_string(),
            _ => String::new(),
        };
        let case = if cased.is_some() { " (commands are lower case)" } else { "" };
        return format!("did you mean 'v2 {command}{rest}'?{case}");
    }
    // Notepad saves `hello` as hello.txt: whether FILE was hello or hello.v2, it becomes hello.v2.
    let stem = name.strip_suffix(".v2").unwrap_or(&name);
    if folder.join(format!("{stem}.txt")).is_file() && (stem != name || path.extension().is_none()) {
        return format!("there is a '{stem}.txt': your editor saved it as a .txt file, so rename it to '{stem}.v2'");
    }
    if folder.join(format!("{name}.txt")).is_file() {
        return format!("there is a '{name}.txt': your editor added '.txt', so rename it to '{name}'");
    }
    if path.extension().is_none() && folder.join(format!("{name}.v2.txt")).is_file() {
        return format!("there is a '{name}.v2.txt': your editor added '.txt', so rename it to '{name}.v2'");
    }
    let scripts = v2_files(folder).unwrap_or_default();
    // By the names without `.v2`, which every one shares: `nope.v2` isn't a typo of `tour.v2`.
    let stem = |s: &str| s.strip_suffix(".v2").unwrap_or(s).to_string();
    let stems: Vec<String> = scripts.iter().map(|s| stem(s)).collect();
    if let Some(close) = did_you_mean(&stem(&name), stems.iter().map(String::as_str)).map(|c| format!("{c}.v2")) {
        let suggestion = match file.rfind(['/', '\\']) {
            Some(i) => format!("{}{close}", &file[..=i]),
            None => close.to_string(),
        };
        if suggestion.contains(' ') {
            return format!("did you mean \"{suggestion}\"? a name with a space goes in quotes: v2 \"{suggestion}\"");
        }
        return format!("did you mean '{suggestion}'?");
    }
    files_there(folder)
}

/// The last hint for a missing FILE: the folder's `.v2` files, sorted, five at most, that it has
/// none, or that V2 can't list it (a deny on the folder), with the OS's reason (19.1).
fn files_there(folder: &Path) -> String {
    match v2_files(folder) {
        Err(e) => format!("V2 can't list that folder: {} (check the name, or the folder's permissions)", os_message(&e)),
        Ok(scripts) if scripts.is_empty() => "there are no .v2 files there: cd to the folder that holds your script, or give its full path".into(),
        Ok(scripts) if scripts.len() <= 5 => format!("the .v2 files there: {}", scripts.join(", ")),
        Ok(scripts) => format!("the .v2 files there: {}, …", scripts[..5].join(", ")),
    }
}

/// The `.v2` files directly in `folder`, sorted, or why the folder can't be listed.
fn v2_files(folder: &Path) -> std::io::Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(folder)?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| Path::new(n).extension().is_some_and(|x| x.eq_ignore_ascii_case("v2")))
        .collect();
    names.sort();
    Ok(names)
}

/// Windows: `/c/Users/me/x.v2`, Git Bash's spelling of `C:/Users/me/x.v2` (one letter after the
/// first `/`, a drive that exists, and no folder `\c` on this drive), as `C:/Users/me/x.v2`;
/// the file builtins' `msys_hint` reads it the same way (5.10).
fn git_bash_path(file: &str) -> Option<String> {
    let b = file.as_bytes();
    if !cfg!(windows) || b.first() != Some(&b'/') || !b.get(1).is_some_and(u8::is_ascii_alphabetic) || !matches!(b.get(2), None | Some(b'/')) {
        return None;
    }
    let drive = format!("{}:", (b[1] as char).to_ascii_uppercase());
    if !Path::new(&format!("{drive}/")).is_dir() || Path::new(&file[..2]).exists() {
        return None;
    }
    Some(format!("{drive}{}", if file.len() > 2 { &file[2..] } else { "/" }))
}

/// A folder as messages show it: absolute, without Windows's `\\?\`.
fn shown_folder(p: &Path) -> String {
    let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    crate::syntax::span::strip_verbatim(&p).display().to_string()
}

/// A word as the user would type it in a shell: in double quotes unless it holds only letters,
/// digits and `._-+~\/:`, so a pasted `R&D\app.v2` or `it's` works in cmd, PowerShell and sh
/// (19.1). Off Windows, one with a `$`, `` ` ``, `"`, `\` or `!`, which sh (or bash's history)
/// reads in double quotes too, goes in single quotes, a `'` in it as `'\''`.
pub fn shell_word(s: &str) -> String {
    if !cfg!(windows) && s.contains(['$', '`', '"', '\\', '!']) {
        return format!("'{}'", s.replace('\'', r"'\''"));
    }
    if !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || "._-+~\\/:".contains(c)) {
        s.to_string()
    } else {
        format!("\"{s}\"")
    }
}

fn quoted(s: &str) -> String {
    shell_word(s)
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
    let file = shown_path(file);
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

    #[test]
    fn suggested_commands_are_quoted_for_a_shell() {
        // 19.1: what cmd, PowerShell or sh would split or run (& ; ' and spaces) is quoted.
        assert_eq!(shell_word("R&D/app.v2"), "\"R&D/app.v2\"");
        assert_eq!(shell_word("semi;colon/app.v2"), "\"semi;colon/app.v2\"");
        assert_eq!(shell_word("it's/app.v2"), "\"it's/app.v2\"");
        assert_eq!(shell_word("sub dir/deep.v2"), "\"sub dir/deep.v2\"");
        assert_eq!(shell_word("../lib/x-1.2+b.v2"), "../lib/x-1.2+b.v2");
        if cfg!(windows) {
            assert_eq!(shell_word(r"R&D\app.v2"), r#""R&D\app.v2""#);
            assert_eq!(shell_word("C:\\Users\\ME~1\\donn\u{e9}es\\app.v2"), "C:\\Users\\ME~1\\donn\u{e9}es\\app.v2");
            assert_eq!(shell_word("a$b/v2.toml"), "\"a$b/v2.toml\"");
        } else {
            // Round 20: sh reads $, `, " and \ in double quotes, and bash's history a !.
            assert_eq!(shell_word("/w/a$b/v2.toml"), "'/w/a$b/v2.toml'");
            assert_eq!(shell_word("/w/x`y/v2.toml"), "'/w/x`y/v2.toml'");
            assert_eq!(shell_word("/w/b\\c/v2.toml"), "'/w/b\\c/v2.toml'");
            assert_eq!(shell_word("/w/q\"r/v2.toml"), "'/w/q\"r/v2.toml'");
            assert_eq!(shell_word("/w/wow!/v2.toml"), "'/w/wow!/v2.toml'");
            assert_eq!(shell_word("/w/it's $5/v2.toml"), "'/w/it'\\''s $5/v2.toml'");
        }
    }

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
            let m = entry_error(&at(f), next.as_slice(), "v2").expect("missing");
            m.lines().nth(1).unwrap().to_string()
        };
        assert!(entry_error(&at("hello.v2"), &[], "v2").is_none());
        // `at("")` is the folder with a separator at the end.
        let dir = at("");
        assert_eq!(hint("hello", None), format!("did you mean '{dir}hello.v2'?"));
        assert_eq!(hint("notes.v2", None), "there is a 'notes.v2.txt': your editor added '.txt', so rename it to 'notes.v2'");
        assert_eq!(hint("helo.v2", None), format!("did you mean '{dir}hello.v2'?"));
        assert_eq!(hint("zzzzzz.v2", None), "the .v2 files there: b.v2, hello.v2");
        // A name with a space, split by the shell or typed with _.
        std::fs::write(d.join("mein spiel.v2"), "print(1)\n").unwrap();
        assert_eq!(hint("mein", Some("spiel.v2")), format!("the file's name has a space, so the shell split it: put it in quotes, as in v2 \"{dir}mein spiel.v2\""));
        assert_eq!(hint("mein_spiel.v2", None), format!("did you mean \"{dir}mein spiel.v2\"? a name with a space goes in quotes: v2 \"{dir}mein spiel.v2\""));
        std::fs::remove_file(d.join("mein spiel.v2")).unwrap();
        // Round 16: several spaces, and a real argument after the name.
        std::fs::write(d.join("my mod — Ελλάδα [v1].v2"), "print(1)\n").unwrap();
        let m = entry_error(&at("my"), &["mod", "—", "Ελλάδα", "[v1].v2", "--fast"], "v2").unwrap();
        assert_eq!(m.lines().nth(1).unwrap(), format!("the file's name has a space, so the shell split it: put it in quotes, as in v2 \"{dir}my mod — Ελλάδα [v1].v2\""));
        std::fs::remove_file(d.join("my mod — Ελλάδα [v1].v2")).unwrap();
        // Notepad's hello.txt, run as hello.v2 or as hello.
        std::fs::write(d.join("greet.txt"), "print(1)\n").unwrap();
        assert_eq!(hint("greet.v2", None), "there is a 'greet.txt': your editor saved it as a .txt file, so rename it to 'greet.v2'");
        assert_eq!(hint("greet", None), "there is a 'greet.txt': your editor saved it as a .txt file, so rename it to 'greet.v2'");
        // Names are compared without .v2: nope.v2 is no typo of tour.v2.
        std::fs::write(d.join("tour.v2"), "print(1)\n").unwrap();
        assert_eq!(hint("nope.v2", None), "the .v2 files there: b.v2, hello.v2, tour.v2");
        // A folder on the way that isn't there, with the one it probably meant.
        std::fs::create_dir_all(d.join("examples")).unwrap();
        let m = entry_error(&at("exmaples/x.v2"), &[], "v2").unwrap();
        assert!(m.ends_with(&format!("there is no folder {}: did you mean 'examples'?", d.join("exmaples").display())), "{m}");
        // `v2 notes` when only Notepad's notes.v2.txt is there.
        assert_eq!(hint("notes", None), "there is a 'notes.v2.txt': your editor added '.txt', so rename it to 'notes.v2'");
        // Data given to V2 by mistake is refused before it is read.
        std::fs::write(d.join("data.csv"), vec![b'1'; (DATA_LIMIT + 1) as usize]).unwrap();
        let m = entry_error(&at("data.csv"), &[], "v2").unwrap();
        assert!(m.starts_with(&format!("'{}' is 2 MB and isn't a .v2 script\nto give it to a script, type: v2 SCRIPT.v2 ", at("data.csv"))), "{m}");
        let empty = temp("empty");
        let m = entry_error(&empty.join("x.v2").to_string_lossy(), &[], "v2").unwrap();
        assert!(m.ends_with("there are no .v2 files there: cd to the folder that holds your script, or give its full path"), "{m}");
    }

    #[test]
    fn command_words_point_to_the_command() {
        // Run from a folder that has none of these files (the test binary's cwd is the crate).
        let hint = |f: &str, next: Option<&str>| entry_error(f, next.as_slice(), "v2").map(|m| m.lines().nth(1).unwrap().to_string());
        assert_eq!(hint("run", Some("hello.v2")).unwrap(), "there is no 'run' command: to run hello.v2, type: v2 hello.v2");
        // A command, misspelled.
        assert_eq!(hint("doctr", None).unwrap(), "did you mean 'v2 doctor'?");
        assert_eq!(hint("chek", Some("x.v2")).unwrap(), "did you mean 'v2 check x.v2'?");
        assert_eq!(hint("cahce", None).unwrap(), "did you mean 'v2 cache clean'?");
        // A command in another case (round 20): commands are lower case.
        assert_eq!(hint("CHECK", Some("echo.v2")).unwrap(), "did you mean 'v2 check echo.v2'? (commands are lower case)");
        assert_eq!(hint("Doctor", None).unwrap(), "did you mean 'v2 doctor'? (commands are lower case)");
        assert_eq!(hint("CACHE", Some("clean")).unwrap(), "did you mean 'v2 cache clean'? (commands are lower case)");
        assert_eq!(hint("run", Some("my file.v2")).unwrap(), "there is no 'run' command: to run my file.v2, type: v2 \"my file.v2\"");
        assert_eq!(hint("help", None).unwrap(), "for help, type: v2 --help");
        assert_eq!(hint("version", None).unwrap(), "for the version, type: v2 --version");
        assert_eq!(hint("install", None).unwrap(), "'install' isn't a V2 command; the commands are check, doctor and cache clean (see v2 --help)");
    }

    #[test]
    fn a_name_in_another_unicode_form_is_named() {
        // 19.1 (round 20): `größe.v2` copied from a Mac (decomposed) for `größe.v2` as typed, and
        // a decomposed name given for a composed file. macOS's file system finds one by the other.
        if cfg!(target_os = "macos") {
            return;
        }
        let d = temp("unicode");
        std::fs::write(d.join("gro\u{308}\u{df}e.v2"), "").unwrap();
        std::fs::write(d.join("caf\u{e9}.v2"), "").unwrap();
        let hint = |f: &str| entry_error(&d.join(f).to_string_lossy(), &[], "v2").expect("missing").lines().nth(1).unwrap().to_string();
        let how = "decomposed Unicode (letters plus combining accents, as macOS writes names)";
        assert_eq!(hint("gr\u{f6}\u{df}e.v2"), format!("there is a 'gro\u{308}\u{df}e.v2' whose name looks the same but is written in {how}: rename it"));
        assert_eq!(hint("cafe\u{301}.v2"), format!("there is a 'caf\u{e9}.v2' whose name looks the same, but the name given is written in {how}: type it again"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_or_name_v2_in_another_unicode_form_is_named() {
        // 19.1 (round 20's review): a Mac's `größe` folder on FILE's way, and `v2 größe_çalış` for
        // a Mac's `größe_çalış.v2`, get the note, not a did-you-mean that looks like the name typed.
        if cfg!(target_os = "macos") {
            return;
        }
        let d = temp("unicode-folder");
        let (folder, folder_nfd) = ("gr\u{f6}\u{df}e", "gro\u{308}\u{df}e");
        let (file, file_nfd) = ("gr\u{f6}\u{df}e_\u{e7}al\u{131}\u{15f}", "gro\u{308}\u{df}e_c\u{327}al\u{131}s\u{327}");
        std::fs::create_dir_all(d.join(folder_nfd)).unwrap();
        std::fs::write(d.join(folder_nfd).join("x.v2"), "").unwrap();
        std::fs::write(d.join(format!("{file_nfd}.v2")), "").unwrap();
        let at = |f: &str| d.join(f).to_string_lossy().into_owned();
        let hint = |f: &str| entry_error(&at(f), &[], "v2").expect("missing").lines().nth(1).unwrap().to_string();
        let how = "decomposed Unicode (letters plus combining accents, as macOS writes names)";
        let x = format!("{folder}/x.v2");
        assert_eq!(hint(&x), format!("there is no folder {}: there is a '{folder_nfd}' whose name looks the same but is written in {how}: rename it", d.join(folder).display()));
        // FILE without its .v2 (third pass): the note ends with the run that works once renamed.
        let with_v2 = quoted(&format!("{}.v2", at(file)));
        assert_eq!(hint(file), format!("there is a '{file_nfd}.v2' whose name looks the same but is written in {how}: rename it, and run v2 {with_v2}"));
        let checked = entry_error(&at(file), &[], "v2 check").unwrap();
        assert!(checked.ends_with(&format!(": rename it, and run v2 check {with_v2}")), "{checked}");
        // A decomposed name given for a composed `naïve.v2`: the run, with the name composed.
        std::fs::write(d.join("na\u{ef}ve.v2"), "").unwrap();
        let composed = quoted(&format!("{}.v2", at("na\u{ef}ve")));
        assert_eq!(hint("nai\u{308}ve"), format!("there is a 'na\u{ef}ve.v2' whose name looks the same, but the name given is written in {how}: type it again, and run v2 {composed}"));
        assert!(entry_error(&at("na\u{ef}ve.v2"), &[], "v2").is_none());
        // Renamed as the notes say: found, and the run they name works.
        std::fs::rename(d.join(folder_nfd), d.join(folder)).unwrap();
        std::fs::rename(d.join(format!("{file_nfd}.v2")), d.join(format!("{file}.v2"))).unwrap();
        assert!(entry_error(&at(&x), &[], "v2").is_none());
        assert_eq!(hint(file), format!("did you mean '{}.v2'?", at(file)));
        assert!(entry_error(&at(&format!("{file}.v2")), &[], "v2").is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_names_a_script_in_it() {
        let d = temp("folder");
        let shown = d.to_string_lossy().into_owned();
        assert!(entry_error(&shown, &[], "v2").unwrap().ends_with("is a folder, not a script, and it has no .v2 files"));
        // Only Notepad's hallo.v2.txt there: what to rename.
        std::fs::write(d.join("hallo.v2.txt"), "").unwrap();
        assert!(entry_error(&shown, &[], "v2").unwrap().ends_with("it has no .v2 files
there is a 'hallo.v2.txt': your editor added '.txt', so rename it to 'hallo.v2'"));
        std::fs::remove_file(d.join("hallo.v2.txt")).unwrap();
        std::fs::write(d.join("util.v2"), "").unwrap();
        std::fs::write(d.join("main.v2"), "").unwrap();
        let m = entry_error(&shown, &[], "v2").unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        let want = format!("{shown}{sep}main.v2");
        assert!(m.ends_with(&format!("run one of its .v2 files, for example: v2 {}", quoted(&want))), "{m}");
        // `v2 check folder` suggests checking, not running.
        let m = entry_error(&shown, &[], "v2 check").unwrap();
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
