//! The cooked-shader source key (ADR-0019 §4), as the build script computes it.
//!
//! `oxreal-shader-compiler` writes the cooked manifest and computes the same key
//! (`cook/key.rs`, which documents the encoding). This crate can't depend on it (a tool), so
//! the encoding is reimplemented here. Both are tested against the same golden value, and
//! `tests/round_trip.rs` cooks this crate's shaders and loads them, which fails if the two keys
//! differ. **Change both together,** with `SOURCE_KEY_FORMAT`.
//!
//! This file is compiled twice: into the build script, and into the crate's tests
//! (`src/lib.rs`), so the golden test checks the code the build actually runs.

use sha2::{Digest, Sha256};

/// The source-key encoding version, as in `oxreal-shader-compiler`.
pub(crate) const SOURCE_KEY_FORMAT: &str = "oxreal-shader-source-key/1";

/// What cooked shaders target, as in `oxreal-shader-compiler`.
pub(crate) const TARGET: &str = "spirv-1.6/vulkan-1.3";

/// One `cook.json` entry, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecipeShader {
    pub(crate) output: String,
    pub(crate) source: String,
    pub(crate) entry_points: Vec<String>,
    pub(crate) include_dirs: Vec<String>,
    pub(crate) inputs: Vec<String>,
}

/// Parses a recipe. The cook checks recipes fully; this only reads the fields the key needs,
/// and refuses input paths that could leave the workspace.
pub(crate) fn parse_recipe(text: &str) -> Result<Vec<RecipeShader>, String> {
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    if json["format"].as_u64() != Some(1) {
        return Err("`format` must be 1".to_owned());
    }
    let shaders = json["shaders"]
        .as_array()
        .ok_or("`shaders` must be an array")?;
    shaders
        .iter()
        .map(|shader| {
            let string = |name: &str| {
                shader[name]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("`{name}` must be a string"))
            };
            let strings = |name: &str| -> Result<Vec<String>, String> {
                shader[name]
                    .as_array()
                    .ok_or_else(|| format!("`{name}` must be an array"))?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| format!("`{name}` must hold strings"))
                    })
                    .collect()
            };
            let inputs = strings("inputs")?;
            for input in &inputs {
                let leaves = input.is_empty()
                    || input.starts_with('/')
                    || input.contains(['\\', ':'])
                    || input
                        .split('/')
                        .any(|p| p.is_empty() || p == "." || p == "..");
                if leaves {
                    return Err(format!(
                        "input `{input}` is not a plain workspace-relative path"
                    ));
                }
            }
            Ok(RecipeShader {
                output: string("output")?,
                source: string("source")?,
                entry_points: strings("entry_points")?,
                include_dirs: strings("include_dirs")?,
                inputs,
            })
        })
        .collect()
}

/// The source key of `shader`, given its inputs' contents in declared order.
pub(crate) fn source_key(
    slang_version: &str,
    shader: &RecipeShader,
    contents: &[Vec<u8>],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for field in [
        SOURCE_KEY_FORMAT,
        slang_version,
        TARGET,
        &shader.output,
        &shader.source,
    ] {
        hasher.update(field_bytes(field.as_bytes()));
    }
    for list in [&shader.entry_points, &shader.include_dirs, &shader.inputs] {
        hasher.update((list.len() as u64).to_le_bytes());
        for item in list {
            hasher.update(field_bytes(item.as_bytes()));
        }
    }
    for (input, bytes) in shader.inputs.iter().zip(contents) {
        hasher.update(field_bytes(input.as_bytes()));
        hasher.update(field_bytes(bytes));
    }
    hasher.finalize().into()
}

/// A field: its length as a little-endian `u64`, then its bytes.
fn field_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = (bytes.len() as u64).to_le_bytes().to_vec();
    out.extend_from_slice(bytes);
    out
}

/// Lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}
