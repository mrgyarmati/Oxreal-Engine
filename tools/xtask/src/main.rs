//! Development task runner for the workspace: `cargo xtask <command>`.
//!
//! Dev tooling only. It is never shipped and must not depend on engine crates.

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "xtask is a CLI tool: printing progress and results is its output"
)]

mod changelog;
mod cook;
mod layers;
mod slang;

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
Usage: cargo xtask <command>

Commands:
  ci                           Run everything CI runs: fmt check; clippy (-D warnings) and tests
                               with default and with all features; shader compiler and cook
                               tests with the pinned Slang (installed if needed); rustdoc; layer
                               check; changelog fragment check
  layers                       Check workspace crate layering rules (docs/architecture/TAD.md section 4)
  changelog check              Validate changelog fragments in changelog.d/ (ADR-0009)
  changelog release <X.Y.Z> [--dry-run]
                               Write fragments into CHANGELOG.md as release X.Y.Z and delete them
                               (--dry-run: print the section only, change nothing)
  slang install                Download, verify and unpack the pinned Slang compiler (ADR-0013)
  slang path                   Print the path of the pinned slangc
  cook-shaders [--profile <name>] [--target <triple>] [--out <dir>]
                               Cook every crate's shaders/cook.json with the pinned Slang
                               (installed if needed) and spirv-val (required; Vulkan SDK) into
                               <target dir>/<profile dir>/cooked/shaders (ADR-0019). The default
                               profile is dev. --out replaces the cooked root
  help                         Show this message";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("layers") => layers::run(&cargo(), &workspace_root()),
        Some("changelog") => changelog::run(&workspace_root(), &args[1..]),
        Some("slang") => slang::run(&workspace_root(), &args[1..]),
        Some("cook-shaders") => cook::run(&cargo(), &workspace_root(), &args[1..]),
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
    run_cargo_with_env(args, &[])
}

fn run_cargo_with_env(args: &[&str], envs: &[(&str, &str)]) -> Result<(), String> {
    println!("\n==> cargo {}", args.join(" "));
    let status = Command::new(cargo())
        .args(args)
        .envs(envs.iter().copied())
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
    // Optional features (application logging, profiling backends) must build and pass too,
    // exactly as in the CI matrix.
    run_cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--all-features",
        "--",
        "-D",
        "warnings",
    ])?;
    run_cargo(&["test", "--workspace", "--all-features"])?;
    // Shader compilation and cooking against the pinned Slang (ADR-0013, ADR-0019), installed
    // on demand. The cook tests skip without spirv-val unless OXREAL_REQUIRE_SPIRV_VAL=1, as CI
    // sets. This doesn't cook the workspace: `cargo xtask cook-shaders` does.
    println!("\n==> pinned Slang");
    slang::ensure_installed(&workspace_root())?;
    run_cargo(&[
        "test",
        "-p",
        "oxreal-shader-compiler",
        "--test",
        "compile",
        "--test",
        "cook",
        "--",
        "--ignored",
    ])?;
    run_cargo(&[
        "test",
        "-p",
        "oxreal-renderer",
        "--test",
        "round_trip",
        "--test",
        "shader_layout",
        "--",
        "--ignored",
    ])?;
    // Same flags as the CI `rustdoc` job: broken intra-doc links and doc warnings fail the build.
    run_cargo_with_env(
        &[
            "doc",
            "--workspace",
            "--no-deps",
            "--document-private-items",
            "--all-features",
        ],
        &[("RUSTDOCFLAGS", "-D warnings")],
    )?;
    println!("\n==> layer check");
    layers::run(&cargo(), &workspace_root())?;
    println!("\n==> changelog fragment check");
    changelog::run(&workspace_root(), &["check".to_owned()])?;
    println!("\nci: all checks passed");
    Ok(())
}
