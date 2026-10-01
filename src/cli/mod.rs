//! Command-line parsing (INTERNALS 19.1).
//!
//! `v2 [FLAGS] FILE [ARGS...]`, `v2 [FLAGS] run [ARGS...]`, `v2 [FLAGS] check FILE`,
//! `v2 [FLAGS] doctor [TAG]`, `v2 cache clean`, `v2 --version`, `v2 --help`.
//! FLAGS are read from the first argument up to the first other word (`--` ends them).

/// Global flags (19.1, 18.4, 19.4).
#[derive(Debug, Default, PartialEq)]
pub struct Flags {
    /// `--engine TAG=PATH`, repeatable, in command-line order.
    pub engines: Vec<(String, String)>,
    /// `--no-cache`.
    pub no_cache: bool,
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Version,
    /// `v2 FILE [ARGS...]`
    Script { file: String, args: Vec<String> },
    /// `v2 run [ARGS...]`: run `[project] entry`.
    RunProject { args: Vec<String> },
    /// `v2 check FILE`
    Check { file: String },
    /// `v2 doctor [TAG]`
    Doctor { tag: Option<String> },
    /// `v2 cache clean`
    CacheClean,
}

#[derive(Debug, PartialEq)]
pub struct Invocation {
    pub flags: Flags,
    pub command: Command,
}

/// A usage error: printed to stderr, exit code 2.
#[derive(Debug, PartialEq)]
pub struct UsageError(pub String);

pub const HELP: &str = "\
V2: one script, every language.

Usage:
  v2 [FLAGS] FILE [ARGS...]   run a script; ARGS become args()
  v2 [FLAGS] run [ARGS...]    run the project's [project] entry (v2.toml)
  v2 [FLAGS] check FILE       validate a script without running anything
  v2 [FLAGS] doctor [TAG]     show every engine, or test one engine in depth
  v2 cache clean              delete compiled-block caches
  v2 --version                print the version
  v2 --help                   print this help

Flags:
  --engine TAG=PATH           use this executable for an engine (repeatable)
  --no-cache                  rebuild compiled blocks instead of using the cache
  --                          end of flags

Exit codes:
  0    success
  1    uncaught error or SyntaxError
  2    usage or configuration error
  3    a required engine is missing
  130  interrupted (Ctrl+C); finally blocks don't run, engines are stopped
  101  internal error in V2 itself (a bug, please report it)
";

pub fn version_line() -> String {
    format!("v2 {} (VEP/1)", env!("CARGO_PKG_VERSION"))
}

/// Parse the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Invocation, UsageError> {
    if args.is_empty() {
        return Err(UsageError("no script given".into()));
    }
    let mut flags = Flags::default();
    let mut meta = None;
    let i = read_flags(args, 0, &mut flags, &mut meta)?;
    if let Some(cmd) = meta {
        return Ok(Invocation { flags, command: cmd });
    }
    let Some(word) = args.get(i) else {
        return Err(UsageError("no script given".into()));
    };
    let rest = &args[i + 1..];
    let command = match word.as_str() {
        "run" => Command::RunProject { args: rest.to_vec() },
        "check" => {
            let j = read_flags(args, i + 1, &mut flags, &mut meta)?;
            if let Some(cmd) = meta {
                return Ok(Invocation { flags, command: cmd });
            }
            match &args[j..] {
                [file] => Command::Check { file: file.clone() },
                [] => return Err(UsageError("`v2 check` needs a FILE".into())),
                [_, extra, ..] => {
                    return Err(UsageError(format!("unexpected argument `{extra}` after `v2 check FILE`")))
                }
            }
        }
        "doctor" => {
            let j = read_flags(args, i + 1, &mut flags, &mut meta)?;
            if let Some(cmd) = meta {
                return Ok(Invocation { flags, command: cmd });
            }
            match &args[j..] {
                [] => Command::Doctor { tag: None },
                [tag] => Command::Doctor { tag: Some(tag.clone()) },
                [_, extra, ..] => {
                    return Err(UsageError(format!("unexpected argument `{extra}` after `v2 doctor TAG`")))
                }
            }
        }
        "cache" => match rest {
            [sub] if sub == "clean" => Command::CacheClean,
            [sub, extra, ..] if sub == "clean" => {
                return Err(UsageError(format!("unexpected argument `{extra}` after `v2 cache clean`")))
            }
            [] => return Err(UsageError("`v2 cache` needs a subcommand: `v2 cache clean`".into())),
            [other, ..] => {
                return Err(UsageError(format!("unknown cache command `{other}` (the only one is `v2 cache clean`)")))
            }
        },
        file => Command::Script { file: file.to_string(), args: rest.to_vec() },
    };
    Ok(Invocation { flags, command })
}

/// Read FLAGS starting at `start`; returns the index of the first non-flag word. `--help` and
/// `--version` among the flags (also after `check`/`doctor`) set `meta`; the first one wins.
fn read_flags(
    args: &[String],
    start: usize,
    flags: &mut Flags,
    meta: &mut Option<Command>,
) -> Result<usize, UsageError> {
    let mut i = start;
    while let Some(a) = args.get(i) {
        match a.as_str() {
            "--" => return Ok(i + 1),
            "--help" => {
                meta.get_or_insert(Command::Help);
                i += 1;
            }
            "--version" => {
                meta.get_or_insert(Command::Version);
                i += 1;
            }
            "--no-cache" => {
                flags.no_cache = true;
                i += 1;
            }
            "--engine" => {
                let spec = args.get(i + 1).ok_or_else(engine_usage)?;
                let (tag, path) = spec.split_once('=').ok_or_else(engine_usage)?;
                if tag.is_empty() || path.is_empty() {
                    return Err(engine_usage());
                }
                flags.engines.push((tag.to_string(), path.to_string()));
                i += 2;
            }
            s if s.starts_with('-') && s.len() > 1 => {
                return Err(UsageError(format!("unknown flag `{s}`")));
            }
            _ => return Ok(i),
        }
    }
    Ok(i)
}

fn engine_usage() -> UsageError {
    UsageError(
        "`--engine` needs TAG=PATH, e.g. `--engine py=C:/Python312/python.exe` \
         (to test one engine, use `v2 doctor TAG`)"
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Invocation, UsageError> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn cmd(args: &[&str]) -> Command {
        p(args).unwrap().command
    }

    #[test]
    fn script_and_args_untouched() {
        assert_eq!(
            cmd(&["main.v2", "--no-cache", "x"]),
            Command::Script { file: "main.v2".into(), args: vec!["--no-cache".into(), "x".into()] }
        );
    }

    #[test]
    fn subcommand_words_are_exact() {
        assert_eq!(cmd(&["run", "a"]), Command::RunProject { args: vec!["a".into()] });
        assert_eq!(cmd(&["./doctor"]), Command::Script { file: "./doctor".into(), args: vec![] });
        assert_eq!(cmd(&["doctor"]), Command::Doctor { tag: None });
        assert_eq!(cmd(&["doctor", "py"]), Command::Doctor { tag: Some("py".into()) });
        assert_eq!(cmd(&["check", "x.v2"]), Command::Check { file: "x.v2".into() });
        assert_eq!(cmd(&["cache", "clean"]), Command::CacheClean);
    }

    #[test]
    fn flags_before_command() {
        let inv = p(&["--engine", "py=C:/p.exe", "--no-cache", "main.v2"]).unwrap();
        assert_eq!(inv.flags.engines, vec![("py".to_string(), "C:/p.exe".to_string())]);
        assert!(inv.flags.no_cache);
        assert_eq!(inv.command, Command::Script { file: "main.v2".into(), args: vec![] });
    }

    #[test]
    fn flags_after_check_and_doctor() {
        let inv = p(&["doctor", "--engine", "py=x", "py"]).unwrap();
        assert_eq!(inv.flags.engines.len(), 1);
        assert_eq!(inv.command, Command::Doctor { tag: Some("py".into()) });
    }

    #[test]
    fn double_dash_ends_flags() {
        assert_eq!(cmd(&["--", "-weird.v2"]), Command::Script { file: "-weird.v2".into(), args: vec![] });
        assert_eq!(cmd(&["--", "run"]), Command::RunProject { args: vec![] });
    }

    #[test]
    fn help_and_version() {
        assert_eq!(cmd(&["--help"]), Command::Help);
        assert_eq!(cmd(&["--no-cache", "--version"]), Command::Version);
        assert_eq!(cmd(&["main.v2", "--help"]), Command::Script { file: "main.v2".into(), args: vec!["--help".into()] });
        assert_eq!(cmd(&["doctor", "--help"]), Command::Help);
        assert_eq!(cmd(&["check", "--version"]), Command::Version);
        assert_eq!(cmd(&["run", "--help"]), Command::RunProject { args: vec!["--help".into()] });
    }

    #[test]
    fn usage_errors() {
        assert!(p(&[]).is_err());
        assert!(p(&["--frobnicate", "x.v2"]).is_err());
        assert!(p(&["--engine"]).is_err());
        assert!(p(&["--engine", "py", "x.v2"]).is_err());
        assert!(p(&["--engine", "=x", "x.v2"]).is_err());
        assert!(p(&["check"]).is_err());
        assert!(p(&["check", "a", "b"]).is_err());
        assert!(p(&["cache"]).is_err());
        assert!(p(&["cache", "nuke"]).is_err());
        assert_eq!(p(&["cache", "clean", "x"]).unwrap_err().0, "unexpected argument `x` after `v2 cache clean`");
        assert!(p(&["-h"]).is_err());
        assert!(p(&["-V"]).is_err());
        assert!(p(&["--no-cache"]).is_err());
    }
}
