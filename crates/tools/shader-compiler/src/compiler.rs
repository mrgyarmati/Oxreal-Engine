//! Running `slangc` (ADR-0013 §2).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use oxreal_diagnostics::{debug, warn};

use crate::reflection::{self, Reflection};
use crate::spirv::{self, Capability};
use crate::{ShaderError, depfile};

/// Environment variable naming a different `slangc` to use instead of the pinned one, for
/// experiments. It's logged as a warning (ADR-0013 §3).
pub const SLANGC_ENV_VAR: &str = "OXREAL_SLANGC";

/// How much the compiler optimizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Optimization {
    /// No optimization, with debug information (`-g -O0`) for GPU debuggers. The default.
    #[default]
    Debug,
    /// Optimized (`-O2`), without debug information.
    Release,
}

/// What to compile.
#[derive(Debug, Clone, Copy)]
pub struct CompileRequest<'a> {
    /// The Slang source file.
    pub source: &'a Path,
    /// Entry points to compile. Each must have a `[shader("...")]` attribute.
    pub entry_points: &'a [&'a str],
    /// Directories searched for `import`ed modules, in addition to the source's own directory.
    pub include_dirs: &'a [PathBuf],
    /// Optimization level.
    pub optimization: Optimization,
}

/// A compiled module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledShader {
    /// SPIR-V 1.6 words containing every requested entry point.
    pub spirv: Vec<u32>,
    /// What the module needs from its pipeline layout.
    pub reflection: Reflection,
    /// Every source file the module was built from, including imports. Use it for caching and
    /// hot reload.
    pub dependencies: Vec<PathBuf>,
    /// The SPIR-V capabilities it declares, all within [`crate::ALLOWED_CAPABILITIES`].
    pub capabilities: Vec<Capability>,
    /// Warnings the compiler printed. They're also logged.
    pub warnings: String,
}

/// The `slangc` command-line compiler.
#[derive(Debug, Clone)]
pub struct SlangCompiler {
    slangc: PathBuf,
    version: String,
}

impl SlangCompiler {
    /// Uses the `slangc` at `path` and records its version.
    ///
    /// # Errors
    ///
    /// [`ShaderError::CompilerNotFound`] if there's no file at `path`, or [`ShaderError::Io`]
    /// if it can't be run.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ShaderError> {
        let slangc = path.into();
        if !slangc.is_file() {
            return Err(ShaderError::CompilerNotFound { path: slangc });
        }
        let output =
            Command::new(&slangc)
                .arg("-v")
                .output()
                .map_err(|source| ShaderError::Io {
                    action: format!("running {} -v", slangc.display()),
                    source,
                })?;
        // slangc prints its version on stderr.
        let version = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .trim()
        .to_owned();
        Ok(Self { slangc, version })
    }

    /// Finds the compiler for development use: `OXREAL_SLANGC` if set (with a warning),
    /// otherwise the pinned Slang that `cargo xtask slang install` puts in the workspace.
    ///
    /// Shipped tools pass an explicit path to [`SlangCompiler::new`] instead.
    ///
    /// # Errors
    ///
    /// [`ShaderError::CompilerNotFound`] if it isn't installed, or [`ShaderError::Io`].
    pub fn locate() -> Result<Self, ShaderError> {
        if let Some(path) = std::env::var_os(SLANGC_ENV_VAR) {
            let compiler = Self::new(path)?;
            warn!(
                slangc = %compiler.slangc.display(),
                version = %compiler.version,
                "using {SLANGC_ENV_VAR} instead of the pinned Slang; builds may not be reproducible"
            );
            return Ok(compiler);
        }
        let (path, pinned) = pinned_slangc();
        let compiler = Self::new(path)?;
        if !compiler.version.contains(&pinned) {
            warn!(
                expected = %pinned,
                found = %compiler.version,
                "installed Slang is not the pinned version; run `cargo xtask slang install`"
            );
        }
        Ok(compiler)
    }

    /// The compiler's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.slangc
    }

    /// The version `slangc -v` reports. Include it in cache keys (ADR-0013 §4).
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Compiles `request` to SPIR-V.
    ///
    /// # Errors
    ///
    /// - [`ShaderError::Compile`] with the compiler's messages for shader errors.
    /// - [`ShaderError::DisallowedCapabilities`] if the result needs GPU capabilities Oxreal
    ///   doesn't guarantee.
    /// - [`ShaderError::InvalidOutput`] or [`ShaderError::Io`] for tool-level failures.
    pub fn compile(&self, request: &CompileRequest<'_>) -> Result<CompiledShader, ShaderError> {
        let dir = scratch_dir("compile")?;
        let result = self.compile_in(&dir, request);
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    fn compile_in(
        &self,
        dir: &Path,
        request: &CompileRequest<'_>,
    ) -> Result<CompiledShader, ShaderError> {
        let source_file = request.source.to_owned();
        let invalid = |details: String| ShaderError::InvalidOutput {
            source_file: source_file.clone(),
            details,
        };
        let spv_path = dir.join("module.spv");
        let reflection_path = dir.join("reflection.json");
        let depfile_path = dir.join("module.d");

        let mut command = Command::new(&self.slangc);
        command.arg(request.source).args([
            "-target",
            "spirv",
            "-profile",
            "spirv_1_6",
            "-matrix-layout-column-major",
            "-fvk-use-entrypoint-name",
        ]);
        for entry in request.entry_points {
            command.args(["-entry", entry]);
        }
        for dir in request.include_dirs {
            command.arg("-I").arg(dir);
        }
        match request.optimization {
            Optimization::Debug => command.args(["-g", "-O0"]),
            Optimization::Release => command.arg("-O2"),
        };
        command
            .arg("-o")
            .arg(&spv_path)
            .arg("-reflection-json")
            .arg(&reflection_path)
            .arg("-depfile")
            .arg(&depfile_path);

        debug!(source = %request.source.display(), entry_points = ?request.entry_points, "compiling shader");
        let output = command.output().map_err(|source| ShaderError::Io {
            action: format!("running {}", self.slangc.display()),
            source,
        })?;
        let messages = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        )
        .trim()
        .to_owned();
        if !output.status.success() {
            return Err(ShaderError::Compile {
                source_file,
                diagnostics: messages,
            });
        }
        if !messages.is_empty() {
            warn!(source = %request.source.display(), "shader compiled with warnings:\n{messages}");
        }

        let bytes = read(&spv_path)?;
        if bytes.len() % 4 != 0 {
            return Err(invalid(format!(
                "SPIR-V size {} is not a multiple of 4",
                bytes.len()
            )));
        }
        // The length was checked to be a multiple of 4, so there's no remainder.
        let (words, _) = bytes.as_chunks::<4>();
        let spirv: Vec<u32> = words.iter().map(|w| u32::from_le_bytes(*w)).collect();

        let capabilities = spirv::capabilities(&spirv).map_err(invalid)?;
        let disallowed = spirv::disallowed(&capabilities);
        if !disallowed.is_empty() {
            return Err(ShaderError::DisallowedCapabilities {
                source_file,
                capabilities: disallowed,
            });
        }

        let reflection = reflection::parse(&String::from_utf8_lossy(&read(&reflection_path)?))
            .map_err(&invalid)?;
        let dependencies =
            depfile::parse(&String::from_utf8_lossy(&read(&depfile_path)?)).map_err(&invalid)?;

        Ok(CompiledShader {
            spirv,
            reflection,
            dependencies,
            capabilities,
            warnings: messages,
        })
    }
}

fn read(path: &Path) -> Result<Vec<u8>, ShaderError> {
    std::fs::read(path).map_err(|source| ShaderError::Io {
        action: format!("reading {}", path.display()),
        source,
    })
}

/// The pinned `slangc` location and version from `tools/slang.json` (development builds).
fn pinned_slangc() -> (PathBuf, String) {
    const MANIFEST: &str = include_str!("../../../../tools/slang.json");
    let version = serde_json::from_str::<serde_json::Value>(MANIFEST)
        .ok()
        .and_then(|json| json["version"].as_str().map(str::to_owned))
        .unwrap_or_default();
    // This crate lives at crates/tools/shader-compiler, three levels below the workspace root.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let exe = if cfg!(windows) {
        "slangc.exe"
    } else {
        "slangc"
    };
    let path = root
        .join(".tools")
        .join("slang")
        .join(&version)
        .join("bin")
        .join(exe);
    (path, version)
}

/// A fresh, unique scratch directory for one tool invocation.
pub(crate) fn scratch_dir(purpose: &str) -> Result<PathBuf, ShaderError> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "oxreal-shader-{purpose}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).map_err(|source| ShaderError::Io {
        action: format!("creating {}", dir.display()),
        source,
    })?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_location_follows_the_manifest() {
        let (path, version) = pinned_slangc();
        assert!(!version.is_empty(), "tools/slang.json has a version");
        let text = path.to_string_lossy();
        assert!(text.contains(".tools"), "{text}");
        assert!(text.contains(&version), "{text}");
    }

    #[test]
    fn a_missing_compiler_explains_how_to_install_it() {
        let err = SlangCompiler::new("definitely/not/slangc").unwrap_err();
        assert!(matches!(err, ShaderError::CompilerNotFound { .. }));
        assert!(
            err.to_string().contains("cargo xtask slang install"),
            "{err}"
        );
    }

    #[test]
    fn scratch_dirs_are_unique() {
        let a = scratch_dir("test").unwrap();
        let b = scratch_dir("test").unwrap();
        assert_ne!(a, b);
        let _ = std::fs::remove_dir_all(a);
        let _ = std::fs::remove_dir_all(b);
    }
}
