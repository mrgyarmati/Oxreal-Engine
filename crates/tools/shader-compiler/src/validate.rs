//! Optional `spirv-val` check (ADR-0013 §2).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::ShaderError;

/// Validates `spirv` with `spirv-val --target-env vulkan1.3`, if `spirv-val` is available (from
/// the Vulkan SDK or `PATH`).
///
/// Returns `Ok(false)` when `spirv-val` isn't installed, so callers can log that validation was
/// skipped, and `Ok(true)` when the module is valid.
///
/// # Errors
///
/// [`ShaderError::Validation`] with the validator's messages if the module is invalid, or
/// [`ShaderError::Io`] if the check itself fails.
pub fn validate_spirv(spirv: &[u32], source_file: &Path) -> Result<bool, ShaderError> {
    let Some(validator) = find_spirv_val() else {
        return Ok(false);
    };
    let dir = crate::compiler::scratch_dir("spirv-val")?;
    let module = dir.join("module.spv");
    let bytes: Vec<u8> = spirv.iter().flat_map(|w| w.to_le_bytes()).collect();
    let result = std::fs::write(&module, bytes)
        .map_err(|source| ShaderError::Io {
            action: format!("writing {}", module.display()),
            source,
        })
        .and_then(|()| {
            Command::new(&validator)
                .args(["--target-env", "vulkan1.3"])
                .arg(&module)
                .output()
                .map_err(|source| ShaderError::Io {
                    action: format!("running {}", validator.display()),
                    source,
                })
        });
    let _ = std::fs::remove_dir_all(&dir);
    let output = result?;
    if output.status.success() {
        Ok(true)
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
