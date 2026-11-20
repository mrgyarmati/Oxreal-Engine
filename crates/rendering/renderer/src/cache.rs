//! Loading cooked shaders (ADR-0019 §4).
//!
//! `cargo xtask cook-shaders` writes `<cooked root>/shaders/`: one `<name>.spv` per shader and a
//! `manifest.json`. The loader treats the directory as untrusted input:
//! - **It opens only files it names itself** (`<name>.spv` for each shader this build needs),
//!   never a path from the manifest.
//! - **The source key must equal the one built into this crate** (`build.rs`). Otherwise the
//!   shader was cooked from other sources, and may not match the layouts this build packs:
//!   [`ShaderCacheError::Stale`].
//! - **Each file's SHA-256 must match the manifest**, which catches damage:
//!   [`ShaderCacheError::Corrupt`].
//!
//! Neither check defends against deliberate tampering, which is out of scope: whoever can
//! replace the cooked directory can replace the executable (ADR-0019 §4).

use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The manifest format this build reads.
const MANIFEST_FORMAT: u64 = 1;
/// The SPIR-V magic number, the first word of every module.
const SPIRV_MAGIC: u32 = 0x0723_0203;

/// A shader this build needs, with the source key computed when it was built.
#[derive(Debug)]
pub(crate) struct ExpectedShader {
    pub(crate) name: &'static str,
    pub(crate) source_key: &'static str,
    pub(crate) entry_points: &'static [&'static str],
}

include!(concat!(env!("OUT_DIR"), "/expected_shaders.rs"));

/// Why cooked shaders couldn't be loaded. Every case is fixed by running
/// `cargo xtask cook-shaders` for the build being run.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ShaderCacheError {
    /// The directory holds no cooked shaders.
    #[error(
        "no cooked shaders in {dir}: run `cargo xtask cook-shaders` (with `--profile release` \
         for release builds)"
    )]
    NotCooked {
        /// The `shaders` directory looked in.
        dir: PathBuf,
    },
    /// The manifest isn't one this build can read.
    #[error("{path} is not a cooked shader manifest this build reads: {details}")]
    Malformed {
        /// The manifest.
        path: PathBuf,
        /// What's wrong.
        details: String,
    },
    /// A shader was cooked from other sources than this build was made with.
    #[error(
        "cooked shader `{shader}` doesn't match this build ({details}): run \
         `cargo xtask cook-shaders`"
    )]
    Stale {
        /// The shader.
        shader: String,
        /// How it differs.
        details: String,
    },
    /// A cooked file is damaged.
    #[error("{path} is damaged ({details}): run `cargo xtask cook-shaders`")]
    Corrupt {
        /// The file.
        path: PathBuf,
        /// What's wrong.
        details: String,
    },
    /// Reading a file failed.
    #[error("{action} failed: {source}")]
    Io {
        /// What was being done.
        action: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// The cooked shaders this renderer needs, checked against the sources it was built with.
#[derive(Debug)]
pub struct CookedShaders {
    /// SPIR-V words, in [`EXPECTED_SHADERS`] order.
    spirv: Vec<Vec<u32>>,
}

impl CookedShaders {
    /// Loads the shaders from `cooked_root`, the directory that contains `shaders/` (for example
    /// `target/debug/cooked`).
    ///
    /// # Errors
    ///
    /// A [`ShaderCacheError`] saying what's missing, stale or damaged.
    pub fn load(cooked_root: &Path) -> Result<Self, ShaderCacheError> {
        let dir = cooked_root.join("shaders");
        let manifest_path = dir.join("manifest.json");
        let text = match std::fs::read_to_string(&manifest_path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ShaderCacheError::NotCooked { dir });
            }
            Err(source) => {
                return Err(ShaderCacheError::Io {
                    action: format!("reading {}", manifest_path.display()),
                    source,
                });
            }
        };
        let manifest = Manifest::parse(&text).map_err(|details| ShaderCacheError::Malformed {
            path: manifest_path.clone(),
            details,
        })?;
        let spirv = EXPECTED_SHADERS
            .iter()
            .map(|expected| load_shader(&dir, &manifest_path, &manifest, expected))
            .collect::<Result<_, _>>()?;
        Ok(Self { spirv })
    }

    /// The mesh shader's SPIR-V. `build.rs` guarantees this build expects it, and `load` loads
    /// every expected shader.
    pub(crate) fn mesh(&self) -> &[u32] {
        &self.spirv[MESH_SHADER]
    }
}

fn load_shader(
    dir: &Path,
    manifest_path: &Path,
    manifest: &Manifest,
    expected: &ExpectedShader,
) -> Result<Vec<u32>, ShaderCacheError> {
    let stale = |details: String| ShaderCacheError::Stale {
        shader: expected.name.to_owned(),
        details,
    };
    let entry = manifest
        .shaders
        .iter()
        .find(|s| s.name == expected.name)
        .ok_or_else(|| stale("it isn't in the manifest".to_owned()))?;
    if entry.source_key != expected.source_key {
        return Err(stale("its source key differs".to_owned()));
    }
    if entry.entry_points != expected.entry_points {
        return Err(stale(format!(
            "its entry points are {:?}, this build needs {:?}",
            entry.entry_points, expected.entry_points
        )));
    }
    // The file name is this build's own, never the manifest's: a manifest naming another file
    // for this shader is malformed, not followed.
    let file = format!("{}.spv", expected.name);
    if entry.file != file {
        return Err(ShaderCacheError::Malformed {
            path: manifest_path.to_owned(),
            details: format!(
                "shader `{}` names file `{}`, not `{file}`",
                expected.name, entry.file
            ),
        });
    }
    let path = dir.join(&file);
    let bytes = std::fs::read(&path).map_err(|source| ShaderCacheError::Io {
        action: format!("reading {}", path.display()),
        source,
    })?;
    let corrupt = |details: &str| ShaderCacheError::Corrupt {
        path: path.clone(),
        details: details.to_owned(),
    };
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    if hex(&digest) != entry.spirv_sha256 {
        return Err(corrupt("its SHA-256 differs from the manifest's"));
    }
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return Err(corrupt("its size is not a whole number of SPIR-V words"));
    }
    let (chunks, _) = bytes.as_chunks::<4>();
    let words: Vec<u32> = chunks.iter().map(|c| u32::from_le_bytes(*c)).collect();
    if words[0] != SPIRV_MAGIC {
        return Err(corrupt("it doesn't start with the SPIR-V magic number"));
    }
    Ok(words)
}

/// The parts of a manifest the loader uses.
#[derive(Debug)]
struct Manifest {
    shaders: Vec<ManifestShader>,
}

#[derive(Debug)]
struct ManifestShader {
    name: String,
    file: String,
    entry_points: Vec<String>,
    spirv_sha256: String,
    source_key: String,
}

impl Manifest {
    /// Parses a manifest strictly: every field the loader relies on must be present and well
    /// typed, and names may appear only once.
    fn parse(text: &str) -> Result<Self, String> {
        let json: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
        match json["format"].as_u64() {
            Some(MANIFEST_FORMAT) => {}
            Some(other) => {
                return Err(format!(
                    "format {other}; this build reads format {MANIFEST_FORMAT}"
                ));
            }
            None => return Err("`format` must be a number".to_owned()),
        }
        if json["target"] != TARGET {
            return Err(format!("`target` must be `{TARGET}`"));
        }
        let shaders = json["shaders"]
            .as_array()
            .ok_or("`shaders` must be an array")?
            .iter()
            .enumerate()
            .map(|(i, s)| parse_shader(s).map_err(|e| format!("shaders[{i}]: {e}")))
            .collect::<Result<Vec<_>, _>>()?;
        for (i, shader) in shaders.iter().enumerate() {
            if shaders[..i].iter().any(|s| s.name == shader.name) {
                return Err(format!("`{}` appears twice", shader.name));
            }
        }
        Ok(Self { shaders })
    }
}

fn parse_shader(json: &Value) -> Result<ManifestShader, String> {
    let string = |name: &str| {
        json[name]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("`{name}` must be a string"))
    };
    let entry_points = json["entry_points"]
        .as_array()
        .ok_or("`entry_points` must be an array")?
        .iter()
        .map(|e| {
            e.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "`entry_points` must hold strings".to_owned())
        })
        .collect::<Result<_, _>>()?;
    Ok(ManifestShader {
        name: string("name")?,
        file: string("file")?,
        entry_points,
        spirv_sha256: string("spirv_sha256")?,
        source_key: string("source_key")?,
    })
}

/// Lowercase hex.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// A cooked root in a fresh temporary directory, removed when dropped.
    struct Cooked(PathBuf);

    impl Cooked {
        /// A cache that matches this build: its own expected keys, with `spirv` as every
        /// shader's SPIR-V.
        fn valid(spirv: &[u32]) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!(
                "oxreal-renderer-cache-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let dir = root.join("shaders");
            std::fs::create_dir_all(&dir).unwrap();
            let bytes: Vec<u8> = spirv.iter().flat_map(|w| w.to_le_bytes()).collect();
            let digest: [u8; 32] = Sha256::digest(&bytes).into();
            let mut shaders = Vec::new();
            for expected in EXPECTED_SHADERS {
                std::fs::write(dir.join(format!("{}.spv", expected.name)), &bytes).unwrap();
                shaders.push(serde_json::json!({
                    "name": expected.name,
                    "file": format!("{}.spv", expected.name),
                    "entry_points": expected.entry_points,
                    "optimization": "debug",
                    "spirv_sha256": hex(&digest),
                    "source_key": expected.source_key,
                }));
            }
            let manifest = serde_json::json!({
                "format": 1,
                "slang_version": "test",
                "spirv_val_version": "test",
                "target": TARGET,
                "shaders": shaders,
            });
            std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
            Self(root)
        }

        fn manifest(&self) -> PathBuf {
            self.0.join("shaders/manifest.json")
        }

        fn edit_manifest(&self, from: &str, to: &str) {
            let text = std::fs::read_to_string(self.manifest()).unwrap();
            assert!(text.contains(from), "manifest lacks `{from}`: {text}");
            std::fs::write(self.manifest(), text.replacen(from, to, 1)).unwrap();
        }
    }

    impl Drop for Cooked {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const MODULE: [u32; 2] = [SPIRV_MAGIC, 0x0001_0600];

    fn mesh() -> &'static ExpectedShader {
        EXPECTED_SHADERS.iter().find(|e| e.name == "Mesh").unwrap()
    }

    #[test]
    fn the_build_expects_the_mesh_shader() {
        let mesh = mesh();
        assert_eq!(mesh.entry_points, ["meshVertex", "meshPixel"]);
        assert_eq!(mesh.source_key.len(), 64);
    }

    #[test]
    fn a_matching_cache_loads() {
        let cooked = Cooked::valid(&MODULE);
        let shaders = CookedShaders::load(&cooked.0).unwrap();
        assert_eq!(shaders.mesh(), &MODULE[..]);
    }

    #[test]
    fn a_missing_cache_says_how_to_cook() {
        let err = CookedShaders::load(Path::new("definitely/not/cooked")).unwrap_err();
        assert!(matches!(err, ShaderCacheError::NotCooked { .. }));
        assert!(
            err.to_string().contains("cargo xtask cook-shaders"),
            "{err}"
        );
    }

    #[test]
    fn a_cache_from_other_sources_is_stale() {
        let cooked = Cooked::valid(&MODULE);
        cooked.edit_manifest(mesh().source_key, &"0".repeat(64));
        match CookedShaders::load(&cooked.0).unwrap_err() {
            ShaderCacheError::Stale { shader, details } => {
                assert_eq!(shader, "Mesh");
                assert!(details.contains("source key"), "{details}");
            }
            other => panic!("expected Stale, got {other}"),
        }
    }

    #[test]
    fn a_cache_without_the_shader_is_stale() {
        let cooked = Cooked::valid(&MODULE);
        cooked.edit_manifest("\"name\":\"Mesh\"", "\"name\":\"Other\"");
        assert!(matches!(
            CookedShaders::load(&cooked.0).unwrap_err(),
            ShaderCacheError::Stale { .. }
        ));
    }

    #[test]
    fn a_damaged_file_is_corrupt() {
        let cooked = Cooked::valid(&MODULE);
        std::fs::write(cooked.0.join("shaders/Mesh.spv"), [0_u8; 8]).unwrap();
        assert!(matches!(
            CookedShaders::load(&cooked.0).unwrap_err(),
            ShaderCacheError::Corrupt { .. }
        ));
    }

    #[test]
    fn a_hashed_file_that_isnt_spirv_is_corrupt() {
        // Its hash matches (the manifest was written for it), but it isn't SPIR-V.
        let cooked = Cooked::valid(&[0xDEAD_BEEF, 1]);
        assert!(matches!(
            CookedShaders::load(&cooked.0).unwrap_err(),
            ShaderCacheError::Corrupt { .. }
        ));
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let cooked = Cooked::valid(&MODULE);
        std::fs::remove_file(cooked.0.join("shaders/Mesh.spv")).unwrap();
        assert!(matches!(
            CookedShaders::load(&cooked.0).unwrap_err(),
            ShaderCacheError::Io { .. }
        ));
    }

    #[test]
    fn the_manifest_never_chooses_the_file() {
        // A manifest pointing the shader at another path is refused, not followed.
        let cooked = Cooked::valid(&MODULE);
        cooked.edit_manifest("\"file\":\"Mesh.spv\"", "\"file\":\"../../elsewhere.spv\"");
        assert!(matches!(
            CookedShaders::load(&cooked.0).unwrap_err(),
            ShaderCacheError::Malformed { .. }
        ));
    }

    #[test]
    fn malformed_manifests_are_refused() {
        for (from, to) in [
            ("\"format\":1", "\"format\":2"),
            ("\"format\":1", "\"format\":\"1\""),
            (
                "\"target\":\"spirv-1.6/vulkan-1.3\"",
                "\"target\":\"spirv-1.5/vulkan-1.2\"",
            ),
            ("\"shaders\":[", "\"shaders\":[{\"name\":\"Mesh\"},"),
            ("\"source_key\":", "\"source_keys\":"),
        ] {
            let cooked = Cooked::valid(&MODULE);
            cooked.edit_manifest(from, to);
            let err = CookedShaders::load(&cooked.0).unwrap_err();
            assert!(
                matches!(err, ShaderCacheError::Malformed { .. }),
                "`{to}`: expected Malformed, got {err}"
            );
        }
        let cooked = Cooked::valid(&MODULE);
        std::fs::write(cooked.manifest(), "{").unwrap();
        assert!(matches!(
            CookedShaders::load(&cooked.0).unwrap_err(),
            ShaderCacheError::Malformed { .. }
        ));
    }
}
