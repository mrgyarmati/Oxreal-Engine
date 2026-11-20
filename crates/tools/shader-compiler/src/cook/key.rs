//! The source key (ADR-0019 §4): what a cooked shader was built from.
//!
//! `oxreal-renderer` computes the same key in its build script (`build/source_key.rs`) and
//! refuses a cache cooked from anything else. It can't depend on this crate (a tool), so it
//! reimplements this encoding; both are tested against the same golden value, a renderer test
//! checks its copies of the constants equal these, and its round-trip test cooks and loads its
//! shaders. **Any change here, or to the fixed `slangc` flags, is a format change:** bump
//! [`SOURCE_KEY_FORMAT`] and change both implementations together.
//!
//! The key is the SHA-256 of these records, in order. A *field* is its length as a
//! little-endian `u64`, then its bytes. A *count* is a little-endian `u64`.
//! 1. field: [`SOURCE_KEY_FORMAT`]
//! 2. field: the pinned Slang version from `tools/slang.json`
//! 3. field: [`TARGET`]
//! 4. field: the fixed `slangc` arguments, [`crate::compile_options`], which decide layouts
//! 5. the recipe entry: field output, field source, then count and fields of the entry points,
//!    the include directories, and the declared inputs, each in recipe order
//! 6. for each declared input, in recipe order: field path, field contents

use std::fmt;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::{CookEntry, CookError, WorkspacePath};

/// The source-key encoding version. Part of every key.
pub const SOURCE_KEY_FORMAT: &str = "oxreal-shader-source-key/2";

/// What cooked shaders target: SPIR-V 1.6 for Vulkan 1.3 (ADR-0013, ADR-0012). Part of every
/// key and every manifest.
pub const TARGET: &str = "spirv-1.6/vulkan-1.3";

/// A SHA-256 digest, shown as 64 lowercase hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceKey(pub [u8; 32]);

impl fmt::Display for SourceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

/// Computes `entry`'s source key, reading its declared inputs below `workspace`.
///
/// It reads the paths as written. Cooking checks first that each exists with exactly that case
/// (`resolve_exact`).
///
/// # Errors
///
/// [`CookError::Io`] if an input can't be read.
pub fn source_key(
    workspace: &Path,
    slang_version: &str,
    entry: &CookEntry,
) -> Result<SourceKey, CookError> {
    let mut contents = Vec::with_capacity(entry.inputs.len());
    for input in &entry.inputs {
        let path = workspace.join(input.as_str());
        let bytes = std::fs::read(&path).map_err(|source| CookError::Io {
            action: format!("reading {}", path.display()),
            source,
        })?;
        contents.push(bytes);
    }
    Ok(source_key_of(
        slang_version,
        &crate::compile_options(),
        entry,
        &contents,
    ))
}

/// The key of `entry` given its inputs' contents, in declared order.
fn source_key_of(
    slang_version: &str,
    compile_options: &str,
    entry: &CookEntry,
    contents: &[Vec<u8>],
) -> SourceKey {
    let mut hasher = Sha256::new();
    for field in [
        SOURCE_KEY_FORMAT,
        slang_version,
        TARGET,
        compile_options,
        &entry.output,
        entry.source.as_str(),
    ] {
        hasher.update(field_bytes(field.as_bytes()));
    }
    hasher.update(list(entry.entry_points.iter().map(String::as_str)));
    hasher.update(list(entry.include_dirs.iter().map(WorkspacePath::as_str)));
    hasher.update(list(entry.inputs.iter().map(WorkspacePath::as_str)));
    for (input, bytes) in entry.inputs.iter().zip(contents) {
        hasher.update(field_bytes(input.as_str().as_bytes()));
        hasher.update(field_bytes(bytes));
    }
    SourceKey(hasher.finalize().into())
}

/// A field: its length as a little-endian `u64`, then its bytes.
fn field_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = (bytes.len() as u64).to_le_bytes().to_vec();
    out.extend_from_slice(bytes);
    out
}

/// A count, then each item as a field.
fn list<'a>(items: impl ExactSizeIterator<Item = &'a str>) -> Vec<u8> {
    let mut out = (items.len() as u64).to_le_bytes().to_vec();
    for item in items {
        out.extend(field_bytes(item.as_bytes()));
    }
    out
}

/// The SHA-256 of `bytes`.
pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::read_recipe;
    use super::*;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cook")
    }

    /// The golden key of `tests/cook`'s only entry with Slang version `0.0`. The renderer's
    /// build script is tested against this same value. If you change the encoding on purpose,
    /// bump [`SOURCE_KEY_FORMAT`] and update both. It must not depend on the OS or the
    /// checkout: `.gitattributes` keeps the fixture's line endings LF everywhere. It was
    /// checked against an independent implementation of the encoding (`printf` and
    /// `sha256sum`) when it was set.
    const GOLDEN: &str = "a48e9c1c4f9726634f69e55fb5e33605a5688f1aafba879f1a0ef139bf90f05c";

    #[test]
    fn the_fixture_has_the_golden_key_on_every_platform() {
        let root = fixture();
        let entries = read_recipe(&root.join("shaders/cook.json")).unwrap();
        let key = source_key(&root, "0.0", &entries[0]).unwrap();
        assert_eq!(key.to_string(), GOLDEN);
    }

    #[test]
    fn every_part_of_the_entry_changes_the_key() {
        let root = fixture();
        let entry = read_recipe(&root.join("shaders/cook.json")).unwrap()[0].clone();
        let contents: Vec<Vec<u8>> = vec![b"a".to_vec(), b"b".to_vec()];
        let base = source_key_of("1", "opts", &entry, &contents);

        let mut variants = Vec::new();
        variants.push(source_key_of("2", "opts", &entry, &contents));
        // Another compiler flag set: layouts may differ.
        variants.push(source_key_of("1", "opts -fvk-other", &entry, &contents));
        variants.push(source_key_of(
            "1",
            "opts",
            &entry,
            &[b"a".to_vec(), b"c".to_vec()],
        ));
        let mut e = entry.clone();
        e.output = "Other".to_owned();
        variants.push(source_key_of("1", "opts", &e, &contents));
        let mut e = entry.clone();
        e.entry_points.reverse();
        variants.push(source_key_of("1", "opts", &e, &contents));
        let mut e = entry.clone();
        e.entry_points[0] = "renamedVertex".to_owned();
        variants.push(source_key_of("1", "opts", &e, &contents));
        let mut e = entry.clone();
        e.include_dirs.clear();
        variants.push(source_key_of("1", "opts", &e, &contents));
        let mut e = entry.clone();
        e.inputs.reverse();
        variants.push(source_key_of("1", "opts", &e, &contents));

        for (i, variant) in variants.iter().enumerate() {
            assert_ne!(*variant, base, "variant {i} has the base key");
            for other in &variants[..i] {
                assert_ne!(variant, other, "variant {i} collides");
            }
        }
    }

    #[test]
    fn length_prefixes_keep_neighbouring_fields_apart() {
        let root = fixture();
        let mut a = read_recipe(&root.join("shaders/cook.json")).unwrap()[0].clone();
        let mut b = a.clone();
        a.entry_points = vec!["ab".to_owned(), "c".to_owned()];
        b.entry_points = vec!["a".to_owned(), "bc".to_owned()];
        let contents = vec![Vec::new(), Vec::new()];
        assert_ne!(
            source_key_of("1", "opts", &a, &contents),
            source_key_of("1", "opts", &b, &contents)
        );
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xab]), "000fab");
    }
}
