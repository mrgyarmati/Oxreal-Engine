//! `manifest.json`: what a cooked shader directory holds (ADR-0019 §4).
//!
//! ```json
//! {
//!   "format": 1,
//!   "slang_version": "2026.22",
//!   "spirv_val_version": "SPIRV-Tools v2026.4 v2026.4-0-gef96ed76",
//!   "target": "spirv-1.6/vulkan-1.3",
//!   "shaders": [
//!     {
//!       "name": "Mesh",
//!       "file": "Mesh.spv",
//!       "entry_points": ["meshVertex", "meshPixel"],
//!       "optimization": "debug",
//!       "spirv_sha256": "<64 hex digits>",
//!       "source_key": "<64 hex digits>"
//!     }
//!   ]
//! }
//! ```
//!
//! This crate writes it. The renderer's loader reads it, strictly and as untrusted input. The
//! cook reads its own previous manifest only to skip unchanged shaders, and recooks anything it
//! doesn't fully recognize. Shaders are sorted by name, so equal inputs give equal files.

use serde_json::{Value, json};

use super::key::TARGET;
use crate::Optimization;

/// The manifest format this crate writes.
pub const MANIFEST_FORMAT: u64 = 1;

/// The manifest's file name in a cooked shader directory.
pub const MANIFEST_FILE: &str = "manifest.json";

/// One cooked shader, as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestShader {
    /// The output name.
    pub name: String,
    /// The SPIR-V file, `<name>.spv`.
    pub file: String,
    /// Its entry points.
    pub entry_points: Vec<String>,
    /// How it was optimized.
    pub optimization: Optimization,
    /// SHA-256 of the file, hex.
    pub spirv_sha256: String,
    /// The source key, hex.
    pub source_key: String,
}

/// A whole manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The pinned Slang version.
    pub slang_version: String,
    /// The validator's version line.
    pub spirv_val_version: String,
    /// The shaders, sorted by name.
    pub shaders: Vec<ManifestShader>,
}

impl Manifest {
    /// The JSON text, pretty-printed, with a trailing newline.
    #[must_use]
    pub fn to_json(&self) -> String {
        let shaders: Vec<Value> = self
            .shaders
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "file": s.file,
                    "entry_points": s.entry_points,
                    "optimization": optimization_name(s.optimization),
                    "spirv_sha256": s.spirv_sha256,
                    "source_key": s.source_key,
                })
            })
            .collect();
        let manifest = json!({
            "format": MANIFEST_FORMAT,
            "slang_version": self.slang_version,
            "spirv_val_version": self.spirv_val_version,
            "target": TARGET,
            "shaders": shaders,
        });
        // Serializing a `Value` built from strings and numbers can't fail.
        let mut text = serde_json::to_string_pretty(&manifest).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Reads a previous manifest written by this format, or `None` if it's anything else.
    #[must_use]
    pub fn from_previous(text: &str) -> Option<Self> {
        let json: Value = serde_json::from_str(text).ok()?;
        if json["format"].as_u64() != Some(MANIFEST_FORMAT) || json["target"] != TARGET {
            return None;
        }
        let shaders = json["shaders"]
            .as_array()?
            .iter()
            .map(|s| {
                Some(ManifestShader {
                    name: s["name"].as_str()?.to_owned(),
                    file: s["file"].as_str()?.to_owned(),
                    entry_points: s["entry_points"]
                        .as_array()?
                        .iter()
                        .map(|e| e.as_str().map(str::to_owned))
                        .collect::<Option<_>>()?,
                    optimization: parse_optimization(s["optimization"].as_str()?)?,
                    spirv_sha256: s["spirv_sha256"].as_str()?.to_owned(),
                    source_key: s["source_key"].as_str()?.to_owned(),
                })
            })
            .collect::<Option<_>>()?;
        Some(Self {
            slang_version: json["slang_version"].as_str()?.to_owned(),
            spirv_val_version: json["spirv_val_version"].as_str()?.to_owned(),
            shaders,
        })
    }
}

/// The manifest's name for an optimization level.
#[must_use]
pub fn optimization_name(optimization: Optimization) -> &'static str {
    match optimization {
        Optimization::Debug => "debug",
        Optimization::Release => "release",
    }
}

fn parse_optimization(name: &str) -> Option<Optimization> {
    match name {
        "debug" => Some(Optimization::Debug),
        "release" => Some(Optimization::Release),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        Manifest {
            slang_version: "2026.22".to_owned(),
            spirv_val_version: "SPIRV-Tools v2026.4".to_owned(),
            shaders: vec![ManifestShader {
                name: "Mesh".to_owned(),
                file: "Mesh.spv".to_owned(),
                entry_points: vec!["meshVertex".to_owned(), "meshPixel".to_owned()],
                optimization: Optimization::Release,
                spirv_sha256: "ab".repeat(32),
                source_key: "cd".repeat(32),
            }],
        }
    }

    #[test]
    fn round_trips_through_json() {
        let manifest = sample();
        let text = manifest.to_json();
        assert!(text.ends_with('\n'));
        assert!(text.contains("\"format\": 1"), "{text}");
        assert!(text.contains(TARGET), "{text}");
        assert_eq!(Manifest::from_previous(&text), Some(manifest));
    }

    #[test]
    fn anything_unrecognized_is_ignored_so_everything_is_recooked() {
        let text = sample().to_json();
        for broken in [
            String::new(),
            "{".to_owned(),
            text.replace("\"format\": 1", "\"format\": 2"),
            text.replace(TARGET, "spirv-1.5/vulkan-1.2"),
            text.replace("\"release\"", "\"fast\""),
            text.replace("\"name\": \"Mesh\",", ""),
        ] {
            assert_eq!(Manifest::from_previous(&broken), None, "{broken}");
        }
    }
}
