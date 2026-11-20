//! Cooking this crate's shaders and loading them (ADR-0019 §5).
//!
//! The cook (`oxreal-shader-compiler`) and this crate's build script compute the source key
//! separately. A cache cooked from this checkout loads only if both agree, and one cooked from
//! edited sources must be refused as stale.
//!
//! These need the pinned Slang and `spirv-val`, so they're ignored by default. CI runs them with
//! `OXREAL_REQUIRE_SPIRV_VAL=1`. Run them with `cargo test -p oxreal-renderer -- --ignored`.

mod common;

use oxreal_renderer::{CookedShaders, ShaderCacheError};

#[test]
#[ignore = "needs the pinned Slang and spirv-val: cargo xtask slang install, then --ignored"]
fn shaders_cooked_from_this_checkout_load() {
    let Some(validator) = common::validator() else {
        return;
    };
    let out = common::TempDir::new("round-trip");
    let cooked = common::cook_renderer(&validator, &common::workspace(), &out.0);
    CookedShaders::load(&cooked).expect("the build script's keys equal the cook's");
}

#[test]
#[ignore = "needs the pinned Slang and spirv-val: cargo xtask slang install, then --ignored"]
fn shaders_cooked_from_edited_sources_are_stale() {
    let Some(validator) = common::validator() else {
        return;
    };
    // A copy of the inputs, with the shader edited: still valid, but not what this build packs
    // for.
    let workspace = common::TempDir::new("edited-workspace");
    for file in [
        "crates/rendering/renderer/shaders/cook.json",
        "crates/rendering/renderer/shaders/Mesh.slang",
        "crates/rendering/rhi/shaders/Bindless.slang",
    ] {
        let to = workspace.0.join(file);
        std::fs::create_dir_all(to.parent().expect("a parent")).expect("creating directories");
        std::fs::copy(common::workspace().join(file), to).expect("copying an input");
    }
    let shader = workspace
        .0
        .join("crates/rendering/renderer/shaders/Mesh.slang");
    let text = std::fs::read_to_string(&shader).unwrap();
    std::fs::write(&shader, format!("{text}\n// An edit.\n")).unwrap();

    let out = common::TempDir::new("edited-cook");
    let cooked = common::cook_renderer(&validator, &workspace.0, &out.0);
    match CookedShaders::load(&cooked).unwrap_err() {
        ShaderCacheError::Stale { shader, details } => {
            assert_eq!(shader, "Mesh");
            assert!(details.contains("source key"), "{details}");
        }
        other => panic!("expected Stale, got {other}"),
    }
}
