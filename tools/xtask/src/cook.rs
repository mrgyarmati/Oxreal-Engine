//! `cargo xtask cook-shaders`: cooks the workspace's shaders (ADR-0019 §4).
//!
//! xtask doesn't depend on engine crates, so it can run even when they don't build. It works
//! out what to cook and where, then runs `oxreal-shader-tool cook`, which owns the cook logic:
//! - **recipes:** every workspace crate's `shaders/cook.json`;
//! - **output:** `<target dir>[/<triple>]/<profile dir>/cooked/shaders`, from `cargo metadata`,
//!   so `CARGO_TARGET_DIR` and `--target` are honoured. `--out <dir>` replaces the cooked root
//!   (the directory that contains `shaders/`);
//! - **optimization:** release for `--profile release`, debug for every other profile.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// What to cook, and how.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    /// The `cooked/shaders` directory.
    pub(crate) out_dir: PathBuf,
    /// `debug` or `release`.
    pub(crate) optimization: &'static str,
    /// The recipes found.
    pub(crate) recipes: Vec<PathBuf>,
}

/// The command's options.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Options {
    profile: Option<String>,
    target: Option<String>,
    out: Option<PathBuf>,
}

pub(crate) fn run(cargo: &str, root: &Path, args: &[String]) -> Result<(), String> {
    let options = parse_options(args)?;
    let metadata = crate::layers::cargo_metadata(cargo, root)?;
    let plan = plan(&metadata, &options, |path| path.is_file())?;
    if plan.recipes.is_empty() {
        println!("cook-shaders: no crate has a shaders/cook.json; writing an empty manifest");
    }
    // A developer's own compiler (OXREAL_SLANGC) replaces the pinned download. The cook still
    // requires it to report exactly the pinned version.
    if std::env::var_os("OXREAL_SLANGC").is_none() {
        crate::slang::ensure_installed(root)?;
    }

    let mut command = Command::new(cargo);
    command
        .args([
            "run",
            "--quiet",
            "--package",
            "oxreal-shader-tool",
            "--",
            "cook",
        ])
        .arg("--workspace")
        .arg(root)
        .arg("--out")
        .arg(&plan.out_dir)
        .args(["--optimization", plan.optimization])
        .args(&plan.recipes)
        .current_dir(root);
    println!(
        "\n==> cook {} recipe(s) into {} ({})",
        plan.recipes.len(),
        plan.out_dir.display(),
        plan.optimization
    );
    let status = command
        .status()
        .map_err(|e| format!("failed to run oxreal-shader-tool: {e}"))?;
    if status.success() {
        println!("cook-shaders: {}", plan.out_dir.display());
        Ok(())
    } else {
        Err(format!("cooking failed ({status}); see the messages above"))
    }
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        let slot_taken = match arg.as_str() {
            "--profile" => options.profile.replace(value()?).is_some(),
            "--target" => options.target.replace(value()?).is_some(),
            // The tool runs in the workspace root, so a relative --out is made absolute against
            // the directory the developer ran the command in.
            "--out" => options.out.replace(absolute(value()?.into())?).is_some(),
            other => {
                return Err(format!(
                    "cook-shaders: unknown option `{other}` (see `cargo xtask help`)"
                ));
            }
        };
        if slot_taken {
            return Err(format!("cook-shaders: {arg} is given twice"));
        }
    }
    Ok(options)
}

/// `path` made absolute against the current directory.
fn absolute(path: PathBuf) -> Result<PathBuf, String> {
    std::path::absolute(&path).map_err(|e| format!("cannot resolve {}: {e}", path.display()))
}

/// Works out the plan from `cargo metadata` output. `exists` says whether a file exists.
fn plan(
    metadata: &Value,
    options: &Options,
    exists: impl Fn(&Path) -> bool,
) -> Result<Plan, String> {
    let profile = options.profile.as_deref().unwrap_or("dev");
    let out_dir = match &options.out {
        Some(cooked_root) => cooked_root.join("shaders"),
        None => {
            let target_dir = metadata["target_directory"]
                .as_str()
                .ok_or("cargo metadata: missing `target_directory`")?;
            let mut dir = PathBuf::from(target_dir);
            if let Some(triple) = &options.target {
                dir.push(triple);
            }
            dir.join(profile_dir(profile))
                .join("cooked")
                .join("shaders")
        }
    };
    let packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata: missing `packages` array")?;
    let mut recipes: Vec<PathBuf> = packages
        .iter()
        .filter_map(|p| p["manifest_path"].as_str())
        .filter_map(|manifest| Path::new(manifest).parent())
        .map(|dir| dir.join("shaders").join("cook.json"))
        .filter(|recipe| exists(recipe))
        .collect();
    recipes.sort();
    Ok(Plan {
        out_dir,
        optimization: if profile == "release" {
            "release"
        } else {
            "debug"
        },
        recipes,
    })
}

/// The directory cargo builds a profile into: `dev` and `test` build into `debug`, `bench` into
/// `release`, and other profiles into their own name.
fn profile_dir(profile: &str) -> &str {
    match profile {
        "dev" | "test" => "debug",
        "bench" => "release",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn metadata() -> Value {
        json!({
            "target_directory": "/ws/target",
            "packages": [
                { "manifest_path": "/ws/crates/rendering/renderer/Cargo.toml" },
                { "manifest_path": "/ws/crates/runtime/ecs/Cargo.toml" },
                { "manifest_path": "/ws/crates/a/Cargo.toml" }
            ]
        })
    }

    fn has_recipe(path: &Path) -> bool {
        let text = path.to_string_lossy().replace('\\', "/");
        text.ends_with("renderer/shaders/cook.json") || text.ends_with("crates/a/shaders/cook.json")
    }

    fn options(words: &str) -> Options {
        let args: Vec<String> = words.split_whitespace().map(str::to_owned).collect();
        parse_options(&args).unwrap()
    }

    #[test]
    fn dev_cooks_debug_into_the_debug_directory() {
        let plan = plan(&metadata(), &options(""), has_recipe).unwrap();
        assert_eq!(plan.out_dir, Path::new("/ws/target/debug/cooked/shaders"));
        assert_eq!(plan.optimization, "debug");
        assert_eq!(
            plan.recipes,
            [
                Path::new("/ws/crates/a/shaders/cook.json"),
                Path::new("/ws/crates/rendering/renderer/shaders/cook.json"),
            ],
            "only crates with a recipe, sorted"
        );
    }

    #[test]
    fn release_and_custom_profiles_and_targets() {
        let release = plan(&metadata(), &options("--profile release"), has_recipe).unwrap();
        assert_eq!(
            release.out_dir,
            Path::new("/ws/target/release/cooked/shaders")
        );
        assert_eq!(release.optimization, "release");

        let custom = plan(
            &metadata(),
            &options("--profile profiling --target x86_64-unknown-linux-gnu"),
            has_recipe,
        )
        .unwrap();
        assert_eq!(
            custom.out_dir,
            Path::new("/ws/target/x86_64-unknown-linux-gnu/profiling/cooked/shaders")
        );
        assert_eq!(custom.optimization, "debug");

        let bench = plan(&metadata(), &options("--profile bench"), has_recipe).unwrap();
        assert_eq!(
            bench.out_dir,
            Path::new("/ws/target/release/cooked/shaders")
        );
        assert_eq!(
            bench.optimization, "debug",
            "only the release profile optimizes"
        );
    }

    fn plan_for(words: &str) -> Plan {
        plan(&metadata(), &options(words), has_recipe).unwrap()
    }

    #[test]
    fn out_replaces_the_cooked_root_and_is_made_absolute() {
        let plan = plan(&metadata(), &options("--out /tmp/game/cooked"), has_recipe).unwrap();
        let expected = absolute("/tmp/game/cooked".into()).unwrap().join("shaders");
        assert_eq!(plan.out_dir, expected);
        assert!(plan.out_dir.is_absolute());

        let relative = plan_for("--out cooked");
        assert_eq!(
            relative.out_dir,
            std::env::current_dir().unwrap().join("cooked/shaders")
        );
    }

    #[test]
    fn bad_options_are_rejected() {
        for (words, expected) in [
            ("--profile", "needs a value"),
            ("--out a --out b", "given twice"),
            ("--fast", "unknown option"),
        ] {
            let args: Vec<String> = words.split_whitespace().map(str::to_owned).collect();
            let err = parse_options(&args).unwrap_err();
            assert!(err.contains(expected), "`{words}`: {err}");
        }
    }
}
