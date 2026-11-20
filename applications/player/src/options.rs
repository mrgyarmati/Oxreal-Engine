//! The player's command line.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

/// How to use the player, printed by `--help` and with argument errors.
pub(crate) const USAGE: &str = "\
usage: oxreal-player [--cooked <dir>] [--frames <n>] [--timeout-secs <s>]

  --cooked <dir>       the cooked data's root, the directory holding `shaders/`
                       (default: $OXREAL_COOKED_DIR, else `cooked/` next to the player)
  --frames <n>         exit after n frames, failing if they don't arrive in time (for CI)
  --timeout-secs <s>   with --frames, the time allowed for them (default 60)
  --help               show this help";

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    /// `--cooked`.
    pub(crate) cooked: Option<PathBuf>,
    /// `--frames`: exit after this many frames.
    pub(crate) frames: Option<u64>,
    /// `--timeout-secs`: how long `--frames` may take.
    pub(crate) timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            cooked: None,
            frames: None,
            timeout: Duration::from_secs(60),
        }
    }
}

/// The result of reading the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    /// Run with these options.
    Run(Options),
    /// Print [`USAGE`] and exit successfully.
    Help,
}

/// The command line is wrong. The message says how; [`USAGE`] says what's right.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum OptionsError {
    /// An option needs a value and has none, or an empty one.
    #[error("{option} needs a value")]
    MissingValue {
        /// The option.
        option: &'static str,
    },
    /// A value isn't a whole number.
    #[error("{option} {value:?} is not a whole number")]
    NotANumber {
        /// The option.
        option: &'static str,
        /// What was given.
        value: String,
    },
    /// An argument isn't an option the player knows.
    #[error("unknown argument {argument:?}")]
    Unknown {
        /// The argument, lossily converted for the message.
        argument: String,
    },
}

/// Reads `args`, the command line without the program name.
///
/// # Errors
///
/// [`OptionsError`] for a missing or malformed value, or an unknown argument.
pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, OptionsError> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        // An empty value counts as missing, like an empty environment variable.
        let mut value = |option: &'static str| {
            args.next()
                .filter(|value| !value.is_empty())
                .ok_or(OptionsError::MissingValue { option })
        };
        match arg.to_str() {
            Some("--help" | "-h") => return Ok(Command::Help),
            // A path may be any OS string, so it isn't converted.
            Some("--cooked") => options.cooked = Some(PathBuf::from(value("--cooked")?)),
            Some("--frames") => options.frames = Some(number("--frames", &value("--frames")?)?),
            Some("--timeout-secs") => {
                let seconds = number("--timeout-secs", &value("--timeout-secs")?)?;
                options.timeout = Duration::from_secs(seconds);
            }
            _ => {
                return Err(OptionsError::Unknown {
                    argument: arg.to_string_lossy().into_owned(),
                });
            }
        }
    }
    Ok(Command::Run(options))
}

fn number(option: &'static str, value: &OsString) -> Result<u64, OptionsError> {
    value
        .to_str()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| OptionsError::NotANumber {
            option,
            value: value.to_string_lossy().into_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Result<Command, OptionsError> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn no_arguments_run_until_closed() {
        assert_eq!(parse_strs(&[]), Ok(Command::Run(Options::default())));
    }

    #[test]
    fn every_option_is_read() {
        let command = parse_strs(&[
            "--cooked",
            "data dir",
            "--frames",
            "120",
            "--timeout-secs",
            "5",
        ]);
        assert_eq!(
            command,
            Ok(Command::Run(Options {
                cooked: Some(PathBuf::from("data dir")),
                frames: Some(120),
                timeout: Duration::from_secs(5),
            }))
        );
    }

    #[test]
    fn help_wins() {
        assert_eq!(parse_strs(&["--frames", "3", "--help"]), Ok(Command::Help));
        assert_eq!(parse_strs(&["-h"]), Ok(Command::Help));
    }

    #[test]
    fn mistakes_are_named() {
        assert_eq!(
            parse_strs(&["--frames"]),
            Err(OptionsError::MissingValue { option: "--frames" })
        );
        assert_eq!(
            parse_strs(&["--cooked", ""]),
            Err(OptionsError::MissingValue { option: "--cooked" }),
            "an empty path would mean the current directory"
        );
        assert_eq!(
            parse_strs(&["--frames", "-1"]),
            Err(OptionsError::NotANumber {
                option: "--frames",
                value: "-1".to_owned()
            })
        );
        assert_eq!(
            parse_strs(&["--fullscreen"]),
            Err(OptionsError::Unknown {
                argument: "--fullscreen".to_owned()
            })
        );
    }
}
