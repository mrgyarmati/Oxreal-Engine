//! Command-line arguments.

use std::path::PathBuf;

use oxreal_shader_compiler::Optimization;

pub(crate) const USAGE: &str = "\
Usage: oxreal-shader-tool <command>

Commands:
  cook --workspace <dir> --out <dir> [--optimization debug|release] <cook.json>...
      Compile every shader the recipes list with the pinned Slang, validate each with
      spirv-val, and write <out>/<name>.spv plus <out>/manifest.json (ADR-0019).
      Recipe paths are relative to --workspace. The default optimization is debug.
      Usually run as `cargo xtask cook-shaders`.
  help
      Show this message.";

/// What to do.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Help,
    Cook(CookArgs),
}

/// `cook`'s arguments.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CookArgs {
    pub(crate) workspace: PathBuf,
    pub(crate) out_dir: PathBuf,
    pub(crate) optimization: Optimization,
    pub(crate) recipes: Vec<PathBuf>,
}

/// The arguments were wrong.
#[derive(Debug, thiserror::Error)]
#[error("{0}\n\n{USAGE}")]
pub(crate) struct UsageError(String);

/// Parses the arguments after the program name.
pub(crate) fn parse(args: &[String]) -> Result<Command, UsageError> {
    match args.first().map(String::as_str) {
        None | Some("help" | "-h" | "--help") => Ok(Command::Help),
        Some("cook") => parse_cook(&args[1..]).map(Command::Cook),
        Some(other) => Err(UsageError(format!("unknown command `{other}`"))),
    }
}

fn parse_cook(args: &[String]) -> Result<CookArgs, UsageError> {
    let mut workspace = None;
    let mut out_dir = None;
    let mut optimization = None;
    let mut recipes = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .ok_or_else(|| UsageError(format!("{name} needs a value")))
        };
        match arg.as_str() {
            "--workspace" => set(&mut workspace, "--workspace", value("--workspace")?.into())?,
            "--out" => set(&mut out_dir, "--out", value("--out")?.into())?,
            "--optimization" => {
                let level = match value("--optimization")?.as_str() {
                    "debug" => Optimization::Debug,
                    "release" => Optimization::Release,
                    other => {
                        return Err(UsageError(format!(
                            "--optimization must be `debug` or `release`, not `{other}`"
                        )));
                    }
                };
                set(&mut optimization, "--optimization", level)?;
            }
            flag if flag.starts_with('-') => {
                return Err(UsageError(format!("unknown option `{flag}`")));
            }
            recipe => recipes.push(PathBuf::from(recipe)),
        }
    }
    Ok(CookArgs {
        workspace: workspace.ok_or_else(|| UsageError("cook needs --workspace".to_owned()))?,
        out_dir: out_dir.ok_or_else(|| UsageError("cook needs --out".to_owned()))?,
        optimization: optimization.unwrap_or_default(),
        recipes,
    })
}

fn set<T>(slot: &mut Option<T>, name: &str, value: T) -> Result<(), UsageError> {
    if slot.replace(value).is_some() {
        Err(UsageError(format!("{name} is given twice")))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(words: &str) -> Result<Command, UsageError> {
        let args: Vec<String> = words.split_whitespace().map(str::to_owned).collect();
        parse(&args)
    }

    #[test]
    fn parses_a_cook_command() {
        let command =
            parse_words("cook --workspace ws --out ws/out --optimization release a.json b.json")
                .unwrap();
        assert_eq!(
            command,
            Command::Cook(CookArgs {
                workspace: "ws".into(),
                out_dir: "ws/out".into(),
                optimization: Optimization::Release,
                recipes: vec!["a.json".into(), "b.json".into()],
            })
        );
    }

    #[test]
    fn optimization_defaults_to_debug_and_recipes_may_be_empty() {
        let Command::Cook(args) = parse_words("cook --out o --workspace w").unwrap() else {
            panic!("expected cook");
        };
        assert_eq!(args.optimization, Optimization::Debug);
        assert!(args.recipes.is_empty());
    }

    #[test]
    fn help_is_the_default() {
        assert_eq!(parse_words("").unwrap(), Command::Help);
        assert_eq!(parse_words("--help").unwrap(), Command::Help);
    }

    #[test]
    fn mistakes_are_reported_with_the_usage() {
        for (words, expected) in [
            ("frobnicate", "unknown command"),
            ("cook --out o", "needs --workspace"),
            ("cook --workspace w", "needs --out"),
            ("cook --workspace", "needs a value"),
            (
                "cook --workspace w --out o --optimization fast",
                "`debug` or `release`",
            ),
            ("cook --workspace w --workspace v --out o", "given twice"),
            ("cook --workspace w --out o --verbose", "unknown option"),
        ] {
            let err = parse_words(words).unwrap_err().to_string();
            assert!(err.contains(expected), "`{words}`: {err}");
            assert!(err.contains("Usage:"), "{err}");
        }
    }
}
