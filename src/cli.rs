//! Command-line arguments.

use std::path::PathBuf;

pub const USAGE: &str = "\
herdr-inbox — one inbox for every coding agent you run in Herdr

Usage: herdr-inbox [--session NAME] [--herdr PATH]

Options:
  --session NAME   Use a named Herdr session instead of the default one
  --herdr PATH     The herdr binary to run (default: herdr on PATH)
  -h, --help       Show this help
  -V, --version    Show the version

Keys:
  Threads   j/k move · Enter open · x archive · Tab agent · q quit
  Agent     everything goes to the agent · Tab back to the threads
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub session: Option<String>,
    pub herdr: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run(Options),
    Help,
    Version,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut options = Options { session: None, herdr: PathBuf::from("herdr") };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag.to_string(), Some(value.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| args.next())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--session" => options.session = Some(value("--session")?),
            "--herdr" => options.herdr = PathBuf::from(value("--herdr")?),
            other => return Err(format!("unknown argument {other:?}; see herdr-inbox --help")),
        }
    }
    Ok(Command::Run(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(|a| a.to_string()))
    }

    #[test]
    fn no_arguments_runs_against_the_default_session() {
        assert_eq!(parse_strs(&[]), Ok(Command::Run(Options { session: None, herdr: "herdr".into() })));
    }

    #[test]
    fn session_and_herdr_accept_both_spellings() {
        let expected = Ok(Command::Run(Options { session: Some("work".into()), herdr: "/opt/herdr".into() }));
        assert_eq!(parse_strs(&["--session", "work", "--herdr", "/opt/herdr"]), expected);
        assert_eq!(parse_strs(&["--session=work", "--herdr=/opt/herdr"]), expected);
    }

    #[test]
    fn help_and_version_win_wherever_they_appear() {
        assert_eq!(parse_strs(&["--session", "x", "-h"]), Ok(Command::Help));
        assert_eq!(parse_strs(&["--version"]), Ok(Command::Version));
        assert_eq!(parse_strs(&["-V", "--bogus"]), Ok(Command::Version));
    }

    #[test]
    fn missing_values_and_unknown_flags_are_errors() {
        assert!(parse_strs(&["--session"]).unwrap_err().contains("--session needs a value"));
        assert!(parse_strs(&["--session="]).unwrap_err().contains("needs a value"));
        assert!(parse_strs(&["--nope"]).unwrap_err().contains("unknown argument"));
        assert!(parse_strs(&["stray"]).unwrap_err().contains("\"stray\""));
    }
}
