//! Cooking this crate's shaders for tests, the way `cargo xtask cook-shaders` does.

#![allow(dead_code, reason = "each test file uses part of this module")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use oxreal_shader_compiler::cook::{CookRequest, cook};
use oxreal_shader_compiler::{Optimization, ShaderError, SlangCompiler, SpirvValidator};

/// The workspace root: this crate is at crates/rendering/renderer.
pub(crate) fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("the crate is inside the workspace")
        .to_owned()
}

/// A fresh temporary directory, removed when dropped.
pub(crate) struct TempDir(pub(crate) PathBuf);

impl TempDir {
    pub(crate) fn new(purpose: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "oxreal-renderer-{purpose}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("creating a temporary directory");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The validator, or `None` (with a note) where it isn't installed and isn't required.
#[expect(
    clippy::print_stderr,
    reason = "a skipped test says why, like the shader compiler's tests"
)]
pub(crate) fn validator() -> Option<SpirvValidator> {
    match SpirvValidator::locate() {
        Ok(validator) => Some(validator),
        Err(ShaderError::ValidatorNotFound) => {
            let required = std::env::var("OXREAL_REQUIRE_SPIRV_VAL").is_ok_and(|v| v == "1");
            assert!(
                !required,
                "OXREAL_REQUIRE_SPIRV_VAL=1 but spirv-val was not found (VULKAN_SDK or PATH)"
            );
            eprintln!("note: spirv-val not found; cooking needs it, so this test is skipped");
            None
        }
        Err(other) => panic!("spirv-val is unusable: {other}"),
    }
}

/// Cooks the renderer's recipe in `workspace` into a cooked root inside `out`, and returns that
/// root.
pub(crate) fn cook_renderer(validator: &SpirvValidator, workspace: &Path, out: &Path) -> PathBuf {
    let compiler = SlangCompiler::locate().expect("pinned Slang (cargo xtask slang install)");
    let recipes = [workspace.join("crates/rendering/renderer/shaders/cook.json")];
    let root = out.join("cooked");
    cook(
        &compiler,
        validator,
        &CookRequest {
            workspace,
            recipes: &recipes,
            out_dir: &root.join("shaders"),
            optimization: Optimization::Debug,
        },
    )
    .expect("the renderer's shaders cook");
    root
}
