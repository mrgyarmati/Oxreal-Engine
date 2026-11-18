//! Development task runner for the workspace: `cargo xtask <command>`.
//!
//! Dev tooling only. It is never shipped and must not depend on engine crates.

// This is a CLI tool, so printing is its output.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod layers;

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
Usage: cargo xtask <command>

Commands:
  ci       Run everything CI runs: fmt check, clippy (-D warnings), tests, layer check
  layers   Check workspace crate layering rules (docs/architecture/TAD.md section 4)
  help     Show this message";

fn main() -> ExitCode {
    let result = match env::args().nth(1).as_deref() {
        Some("ci") => ci(),
        Some("layers") => layers::run(&cargo(), &workspace_root()),
        Some("help" | "-h" | "--help") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The cargo binary that invoked us (respects the pinned toolchain).
fn cargo() -> String {
    env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned())
}

/// `tools/xtask` lives two levels below the workspace root.
fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .nth(2)
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn run_cargo(args: &[&str]) -> Result<(), String> {
    println!("\n==> cargo {}", args.join(" "));
    let status = Command::new(cargo())
        .args(args)
        .current_dir(workspace_root())
        .status()
        .map_err(|e| format!("failed to launch cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`cargo {}` failed ({status})", args.join(" ")))
    }
}

fn ci() -> Result<(), String> {
    run_cargo(&["fmt", "--all", "--check"])?;
    run_cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ])?;
    run_cargo(&["test", "--workspace"])?;
    println!("\n==> layer check");
    layers::run(&cargo(), &workspace_root())?;
    println!("\nci: all checks passed");
    Ok(())
}
