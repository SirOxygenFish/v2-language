//! Command-line parsing (INTERNALS 19.1).
//!
//! `v2 FILE [ARGS...]`, `v2 check FILE`, `v2 doctor`, `v2 cache clean`, `v2 --version`,
//! `v2 --help`. `--version` and `--help` are the only flags, and only as the first word. A first
//! word that is exactly `check`, `doctor` or `cache` is a subcommand; any other word is the
//! script, and everything after it goes to `args()` untouched.

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Version,
    /// `v2 FILE [ARGS...]`
    Script { file: String, args: Vec<String> },
    /// `v2 check FILE`
    Check { file: String },
    /// `v2 doctor`
    Doctor,
    /// `v2 cache clean`
    CacheClean,
}

/// A usage error: printed to stderr, exit code 2.
#[derive(Debug, PartialEq)]
pub struct UsageError(pub String);

pub const HELP: &str = "\
V2: one script, every language.

Usage:
  v2 FILE [ARGS...]   run a script; ARGS become args()
  v2 check FILE       validate a script without running anything
  v2 doctor           show every engine and where it was found
  v2 cache clean      delete V2's caches (~/.v2/cache)
  v2 --version        print the version
  v2 --help           print this help

Exit codes:
  0    success
  1    uncaught error or SyntaxError; v2 check found an error
  2    usage or configuration error
  3    a required engine is missing
  130  interrupted (Ctrl+C); finally blocks don't run, engines are stopped
  101  internal error in V2 itself (a bug, please report it)
";

pub fn version_line() -> String {
    format!("v2 {} (VEP/1)", env!("CARGO_PKG_VERSION"))
}

/// Parse the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Command, UsageError> {
    let Some(word) = args.first() else {
        return Err(UsageError("no script given".into()));
    };
    let rest = &args[1..];
    match word.as_str() {
        "--help" => Ok(Command::Help),
        "--version" => Ok(Command::Version),
        s if s.starts_with('-') => Err(UsageError(format!("unknown flag `{s}`"))),
        "check" => match rest {
            [file] => Ok(Command::Check { file: file.clone() }),
            [] => Err(UsageError("`v2 check` needs a FILE".into())),
            [_, extra, ..] => Err(UsageError(format!("unexpected argument `{extra}` after `v2 check FILE`"))),
        },
        "doctor" => match rest {
            [] => Ok(Command::Doctor),
            [first, ..] => Err(UsageError(format!("unexpected argument `{first}` after `v2 doctor`"))),
        },
        "cache" => match rest {
            [sub] if sub == "clean" => Ok(Command::CacheClean),
            [sub, extra, ..] if sub == "clean" => Err(UsageError(format!("unexpected argument `{extra}` after `v2 cache clean`"))),
            [] => Err(UsageError("`v2 cache` needs a subcommand: `v2 cache clean`".into())),
            [other, ..] => Err(UsageError(format!("unknown cache command `{other}` (the only one is `v2 cache clean`)"))),
        },
        file => Ok(Command::Script { file: file.to_string(), args: rest.to_vec() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, UsageError> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn cmd(args: &[&str]) -> Command {
        p(args).unwrap()
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
        assert_eq!(cmd(&["./doctor"]), Command::Script { file: "./doctor".into(), args: vec![] });
        assert_eq!(cmd(&["run", "a"]), Command::Script { file: "run".into(), args: vec!["a".into()] });
        assert_eq!(cmd(&["doctor"]), Command::Doctor);
        assert_eq!(cmd(&["check", "x.v2"]), Command::Check { file: "x.v2".into() });
        assert_eq!(cmd(&["cache", "clean"]), Command::CacheClean);
    }

    #[test]
    fn help_and_version() {
        assert_eq!(cmd(&["--help"]), Command::Help);
        assert_eq!(cmd(&["--version"]), Command::Version);
        assert_eq!(cmd(&["main.v2", "--help"]), Command::Script { file: "main.v2".into(), args: vec!["--help".into()] });
        // Only as the first word: after a subcommand they are ordinary arguments.
        assert_eq!(p(&["doctor", "--help"]).unwrap_err().0, "unexpected argument `--help` after `v2 doctor`");
        assert_eq!(cmd(&["check", "--version"]), Command::Check { file: "--version".into() });
    }

    #[test]
    fn usage_errors() {
        assert!(p(&[]).is_err());
        assert_eq!(p(&["--frobnicate", "x.v2"]).unwrap_err().0, "unknown flag `--frobnicate`");
        assert_eq!(p(&["--engine", "py=x", "x.v2"]).unwrap_err().0, "unknown flag `--engine`");
        assert_eq!(p(&["--no-cache", "x.v2"]).unwrap_err().0, "unknown flag `--no-cache`");
        assert_eq!(p(&["doctor", "py"]).unwrap_err().0, "unexpected argument `py` after `v2 doctor`");
        assert!(p(&["check"]).is_err());
        assert!(p(&["check", "a", "b"]).is_err());
        assert!(p(&["cache"]).is_err());
        assert!(p(&["cache", "nuke"]).is_err());
        assert_eq!(p(&["cache", "clean", "x"]).unwrap_err().0, "unexpected argument `x` after `v2 cache clean`");
        assert!(p(&["-h"]).is_err());
        assert!(p(&["-V"]).is_err());
        assert!(p(&["--", "x.v2"]).is_err());
        assert_eq!(p(&["-"]).unwrap_err().0, "unknown flag `-`");
    }
}
