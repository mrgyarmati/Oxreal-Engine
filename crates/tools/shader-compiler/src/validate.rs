//! The `spirv-val` check (ADR-0013 §2): optional when compiling, mandatory when cooking
//! (ADR-0019 §4).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::ShaderError;

/// The official SPIR-V validator, `spirv-val`, from the Vulkan SDK or `PATH`.
///
/// It runs with `--target-env vulkan1.3 --scalar-block-layout`, which describes the device
/// Oxreal requires: Vulkan 1.3 with `scalarBlockLayout` (ADR-0016 §3). That lets buffer data use
/// the same C-like layout as Rust `#[repr(C)]` structs.
#[derive(Debug, Clone)]
pub struct SpirvValidator {
    path: PathBuf,
    version: String,
}

impl SpirvValidator {
    /// Finds `spirv-val` in `$VULKAN_SDK/Bin` (or `bin`), then on `PATH`, and records its
    /// version.
    ///
    /// # Errors
    ///
    /// [`ShaderError::ValidatorNotFound`] if it isn't installed, or [`ShaderError::Io`] if it
    /// can't be run.
    pub fn locate() -> Result<Self, ShaderError> {
        let path = find_spirv_val().ok_or(ShaderError::ValidatorNotFound)?;
        Self::new(path)
    }

    /// Uses the `spirv-val` at `path` and records its version.
    ///
    /// # Errors
    ///
    /// [`ShaderError::ValidatorNotFound`] if there's no file at `path`, or
    /// [`ShaderError::Io`] if it can't be run.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ShaderError> {
        let path = path.into();
        if !path.is_file() {
            return Err(ShaderError::ValidatorNotFound);
        }
        let output = Command::new(&path)
            .arg("--version")
            .output()
            .map_err(|source| ShaderError::Io {
                action: format!("running {} --version", path.display()),
                source,
            })?;
        // The first line names the release, for example `SPIRV-Tools v2026.4 v2026.4-0-g...`.
        // The rest lists every target it knows.
        let version = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        Ok(Self { path, version })
    }

    /// The validator's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The first line of `spirv-val --version`. Cooked manifests record it (ADR-0019 §4).
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Validates a SPIR-V module.
    ///
    /// # Errors
    ///
    /// [`ShaderError::Validation`] with the validator's messages if the module is invalid, or
    /// [`ShaderError::Io`] if the check itself fails.
    pub fn validate(&self, spirv: &[u32], source_file: &Path) -> Result<(), ShaderError> {
        let dir = crate::compiler::scratch_dir("spirv-val")?;
        let module = dir.join("module.spv");
        let bytes: Vec<u8> = spirv.iter().flat_map(|w| w.to_le_bytes()).collect();
        let result = std::fs::write(&module, bytes)
            .map_err(|source| ShaderError::Io {
                action: format!("writing {}", module.display()),
                source,
            })
            .and_then(|()| {
                Command::new(&self.path)
                    .args(["--target-env", "vulkan1.3", "--scalar-block-layout"])
                    .arg(&module)
                    .output()
                    .map_err(|source| ShaderError::Io {
                        action: format!("running {}", self.path.display()),
                        source,
                    })
            });
        let _ = std::fs::remove_dir_all(&dir);
        let output = result?;
        if output.status.success() {
            Ok(())
        } else {
            Err(ShaderError::Validation {
                source_file: source_file.to_owned(),
                diagnostics: format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                )
                .trim()
                .to_owned(),
            })
        }
    }
}

/// Validates `spirv` with [`SpirvValidator`], **if** `spirv-val` is installed.
///
/// Returns `Ok(false)` when it isn't, so callers can log that validation was skipped, and
/// `Ok(true)` when the module is valid. Cooking never uses this: it requires the validator
/// ([`SpirvValidator::locate`]).
///
/// # Errors
///
/// [`ShaderError::Validation`] with the validator's messages if the module is invalid, or
/// [`ShaderError::Io`] if the check itself fails.
pub fn validate_spirv(spirv: &[u32], source_file: &Path) -> Result<bool, ShaderError> {
    match SpirvValidator::locate() {
        Ok(validator) => validator.validate(spirv, source_file).map(|()| true),
        Err(ShaderError::ValidatorNotFound) => Ok(false),
        Err(other) => Err(other),
    }
}

fn find_spirv_val() -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "spirv-val.exe"
    } else {
        "spirv-val"
    };
    if let Some(sdk) = std::env::var_os("VULKAN_SDK") {
        for sub in ["Bin", "bin"] {
            let candidate = Path::new(&sdk).join(sub).join(exe);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(exe))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_validator_is_a_typed_error() {
        let err = SpirvValidator::new("definitely/not/spirv-val").unwrap_err();
        assert!(matches!(err, ShaderError::ValidatorNotFound));
        assert!(err.to_string().contains("Vulkan SDK"), "{err}");
    }
}
