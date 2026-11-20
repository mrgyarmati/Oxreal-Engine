//! `oxreal-shader-tool`: the command-line front end of `oxreal-shader-compiler`.
//!
//! **Layer:** tool. Never shipped in games (ADR-0013 §4). It holds no shader logic of its own:
//! it parses arguments, finds the compiler and validator, and calls
//! [`oxreal_shader_compiler::cook::cook`].
//!
//! Developers normally run it through `cargo xtask cook-shaders`, which finds the workspace's
//! recipes and the cargo target directory and passes them here (ADR-0019 §4).
//!
//! ```text
//! oxreal-shader-tool cook --workspace <dir> --out <dir> [--optimization debug|release] <cook.json>...
//! ```

mod args;

use std::process::ExitCode;

use oxreal_diagnostics::info;
use oxreal_shader_compiler::cook::{CookRequest, cook};
use oxreal_shader_compiler::{SlangCompiler, SpirvValidator};

use crate::args::Command;

fn main() -> ExitCode {
    oxreal_diagnostics::run_application(|| {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        match args::parse(&arguments)? {
            Command::Help => {
                print_usage();
                Ok(())
            }
            Command::Cook(cook_args) => {
                let compiler = SlangCompiler::locate()?;
                let validator = SpirvValidator::locate()?;
                info!(
                    slangc = %compiler.path().display(),
                    spirv_val = %validator.path().display(),
                    validator_version = validator.version(),
                    "cooking shaders"
                );
                let report = cook(
                    &compiler,
                    &validator,
                    &CookRequest {
                        workspace: &cook_args.workspace,
                        recipes: &cook_args.recipes,
                        out_dir: &cook_args.out_dir,
                        optimization: cook_args.optimization,
                    },
                )?;
                info!(
                    cooked = ?report.cooked,
                    unchanged = report.unchanged.len(),
                    removed = ?report.removed,
                    "done"
                );
                Ok(())
            }
        }
    })
}

#[expect(
    clippy::print_stdout,
    reason = "usage text is the output a user asked for with --help"
)]
fn print_usage() {
    println!("{}", args::USAGE);
}
