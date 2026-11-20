//! Shader compilation errors.

use std::path::PathBuf;

use crate::Capability;

/// Why a shader could not be compiled.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ShaderError {
    /// No usable `slangc` was found.
    #[error("slangc not found at {path}: run `cargo xtask slang install`, or set OXREAL_SLANGC")]
    CompilerNotFound {
        /// Where it was expected.
        path: PathBuf,
    },
    /// `slangc` could not be started or its output could not be read.
    #[error("{action} failed: {source}")]
    Io {
        /// What was being done.
        action: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The shader has errors. `diagnostics` is `slangc`'s own report, with file and line.
    #[error("compiling {source_file} failed:\n{diagnostics}")]
    Compile {
        /// The shader that failed.
        source_file: PathBuf,
        /// The compiler's messages.
        diagnostics: String,
    },
    /// The shader needs GPU capabilities Oxreal doesn't require devices to have (ADR-0013 §2,
    /// ADR-0016 §3).
    #[error(
        "{source_file} needs SPIR-V capabilities that Oxreal does not guarantee: {}. Rewrite the \
         shader to avoid them, or extend the required GPU features in an ADR first",
        list(.capabilities)
    )]
    DisallowedCapabilities {
        /// The shader.
        source_file: PathBuf,
        /// What it declared beyond the allowlist.
        capabilities: Vec<Capability>,
    },
    /// The compiler produced output Oxreal could not interpret.
    #[error("unexpected output from slangc for {source_file}: {details}")]
    InvalidOutput {
        /// The shader.
        source_file: PathBuf,
        /// What was wrong.
        details: String,
    },
    /// `spirv-val` isn't installed. Compiling can skip validation, but cooking can't
    /// (ADR-0019 §4).
    #[error(
        "spirv-val not found: install the Vulkan SDK (and set VULKAN_SDK) or the spirv-tools \
         package, so it's on PATH"
    )]
    ValidatorNotFound,
    /// The `spirv-val` named can't be used.
    #[error("{path} is not a usable spirv-val: {details}")]
    ValidatorUnusable {
        /// The path given.
        path: PathBuf,
        /// What's wrong.
        details: String,
    },
    /// The compiler isn't exactly the version pinned in `tools/slang.json`. Cooking requires it
    /// (ADR-0019 §4).
    #[error(
        "{path} reports Slang version `{found}`, but cooking requires exactly the pinned \
         `{expected}`: run `cargo xtask slang install`, or point OXREAL_SLANGC at that version"
    )]
    UnpinnedCompiler {
        /// The compiler.
        path: PathBuf,
        /// The version `tools/slang.json` pins.
        expected: String,
        /// The version it reported.
        found: String,
    },
    /// `spirv-val` rejected the generated SPIR-V.
    #[error("spirv-val rejected {source_file}:\n{diagnostics}")]
    Validation {
        /// The shader.
        source_file: PathBuf,
        /// The validator's messages.
        diagnostics: String,
    },
}

fn list(capabilities: &[Capability]) -> String {
    capabilities
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
