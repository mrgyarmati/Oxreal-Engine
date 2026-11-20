//! Compiles Slang shaders to SPIR-V ([ADR-0013]).
//!
//! **Layer:** tool. Used by tools and the editor, **never** by shipped games, which load
//! precompiled shaders (ADR-0013 §4). The layer check enforces this.
//!
//! [`SlangCompiler`] runs the pinned `slangc` (installed by `cargo xtask slang install`) as a
//! subprocess, so there's no FFI or C++ build. Each compile:
//! - produces SPIR-V 1.6 with column-major matrices (ADR-0011)
//! - parses `slangc`'s reflection into [`Reflection`] (entry points, bindings, push constants)
//! - parses its dependency file, for caching and hot reload ([`CompiledShader::dependencies`])
//! - **checks every SPIR-V capability against an allowlist** derived from Oxreal's required
//!   GPU features (ADR-0016 §3). A shader that needs something the engine doesn't guarantee
//!   fails here, at build time, instead of on a player's GPU
//! - optionally runs `spirv-val` ([`validate_spirv`])
//!
//! [`cook`] builds on it to write the cooked shader directory that shipped games load
//! (ADR-0019 §4): exact pinned compiler, mandatory validation, and a source key per shader.
//!
//! ```no_run
//! use oxreal_shader_compiler::{CompileRequest, Optimization, SlangCompiler};
//! # fn main() -> Result<(), oxreal_shader_compiler::ShaderError> {
//! let compiler = SlangCompiler::locate()?;
//! let shader = compiler.compile(&CompileRequest {
//!     source: "shaders/passes/Tonemap.slang".as_ref(),
//!     entry_points: &["tonemapVertex", "tonemapPixel"],
//!     include_dirs: &["shaders".into()],
//!     optimization: Optimization::Debug,
//! })?;
//! println!("{} words of SPIR-V", shader.spirv.len());
//! # Ok(()) }
//! ```
//!
//! [ADR-0013]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0013-shader-language-and-compiler-pipeline.md

mod compiler;
pub mod cook;
mod depfile;
mod error;
mod reflection;
mod spirv;
mod validate;

pub use compiler::{
    CompileRequest, CompiledShader, Optimization, SLANGC_ENV_VAR, SlangCompiler, pinned_version,
};
pub use error::ShaderError;
pub use reflection::{EntryPoint, Reflection, ResourceBinding, ShaderStage};
pub use spirv::{ALLOWED_CAPABILITIES, Capability, capabilities};
pub use validate::{SpirvValidator, validate_spirv};
