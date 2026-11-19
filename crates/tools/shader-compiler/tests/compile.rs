//! Compiling real shaders with the pinned `slangc`.
//!
//! These tests need Slang, so they're ignored by default. `cargo xtask ci` installs Slang and
//! runs them, and so does CI. Run them by hand with
//! `cargo xtask slang install && cargo test -p oxreal-shader-compiler -- --ignored`.

use std::path::{Path, PathBuf};

use oxreal_shader_compiler::{
    Capability, CompileRequest, Optimization, ShaderError, ShaderStage, SlangCompiler,
    validate_spirv,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/shaders")
        .join(name)
}

fn compiler() -> SlangCompiler {
    SlangCompiler::locate().expect("Slang is installed (cargo xtask slang install)")
}

#[test]
#[ignore = "needs the pinned Slang: cargo xtask slang install, then --ignored"]
fn compiles_a_module_with_imports_and_reflects_it() {
    let source = fixture("Triangle.slang");
    let shader = compiler()
        .compile(&CompileRequest {
            source: &source,
            entry_points: &["triangleVertex", "trianglePixel"],
            include_dirs: &[],
            optimization: Optimization::Debug,
        })
        .expect("Triangle.slang compiles");

    // Both entry points, with their stages.
    let stages: Vec<(&str, &ShaderStage)> = shader
        .reflection
        .entry_points
        .iter()
        .map(|e| (e.name.as_str(), &e.stage))
        .collect();
    assert_eq!(
        stages,
        vec![
            ("triangleVertex", &ShaderStage::Vertex),
            ("trianglePixel", &ShaderStage::Pixel),
        ]
    );
    assert!(shader.reflection.uses_push_constants);
    assert!(shader.reflection.bindings.is_empty());

    // The import is tracked, so editing Common.slang can trigger a rebuild.
    let names: Vec<String> = shader
        .dependencies
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    assert!(names.contains(&"Triangle.slang".to_owned()), "{names:?}");
    assert!(names.contains(&"Common.slang".to_owned()), "{names:?}");

    // Only the plain Shader capabilities: nothing beyond the guaranteed feature set.
    assert!(shader.capabilities.contains(&Capability(1)));

    // The official validator agrees, when it's installed (Vulkan SDK or PATH).
    // CI sets OXREAL_REQUIRE_SPIRV_VAL=1 where spirv-val is installed, so validation can't be
    // skipped silently there.
    let validated = validate_spirv(&shader.spirv, &source).expect("valid SPIR-V");
    let required = std::env::var("OXREAL_REQUIRE_SPIRV_VAL").is_ok_and(|v| v == "1");
    assert!(
        validated || !required,
        "OXREAL_REQUIRE_SPIRV_VAL=1 but spirv-val was not found (VULKAN_SDK or PATH)"
    );
    if !validated {
        eprintln!("note: spirv-val not found; SPIR-V validation skipped");
    }
}

#[test]
#[ignore = "needs the pinned Slang: cargo xtask slang install, then --ignored"]
fn release_builds_are_smaller_than_debug_builds() {
    let source = fixture("Triangle.slang");
    let build = |optimization| {
        compiler()
            .compile(&CompileRequest {
                source: &source,
                entry_points: &["triangleVertex", "trianglePixel"],
                include_dirs: &[],
                optimization,
            })
            .expect("compiles")
            .spirv
            .len()
    };
    let (debug, release) = (build(Optimization::Debug), build(Optimization::Release));
    assert!(
        release < debug,
        "release {release} words vs debug {debug} words"
    );
}

#[test]
#[ignore = "needs the pinned Slang: cargo xtask slang install, then --ignored"]
fn compile_errors_carry_the_compilers_diagnostics() {
    let source = fixture("Broken.slang");
    let err = compiler()
        .compile(&CompileRequest {
            source: &source,
            entry_points: &["brokenPixel"],
            include_dirs: &[],
            optimization: Optimization::Debug,
        })
        .expect_err("Broken.slang must not compile");
    match &err {
        ShaderError::Compile { diagnostics, .. } => {
            assert!(diagnostics.contains("undefinedSymbol"), "{diagnostics}");
            assert!(diagnostics.contains("Broken.slang"), "{diagnostics}");
        }
        other => panic!("expected a compile error, got {other}"),
    }
}

#[test]
#[ignore = "needs the pinned Slang: cargo xtask slang install, then --ignored"]
fn shaders_needing_unguaranteed_capabilities_are_rejected() {
    let source = fixture("NeedsInt64.slang");
    let err = compiler()
        .compile(&CompileRequest {
            source: &source,
            entry_points: &["needsInt64Pixel"],
            include_dirs: &[],
            optimization: Optimization::Debug,
        })
        .expect_err("Int64 is not a guaranteed capability");
    match &err {
        ShaderError::DisallowedCapabilities { capabilities, .. } => {
            assert!(capabilities.contains(&Capability(11)), "{capabilities:?}");
            assert!(err.to_string().contains("Int64"), "{err}");
        }
        other => panic!("expected DisallowedCapabilities, got {other}"),
    }
}

#[test]
#[ignore = "needs the pinned Slang: cargo xtask slang install, then --ignored"]
fn sv_vertex_id_is_rejected_because_it_needs_draw_parameters() {
    let source = fixture("UsesSvVertexId.slang");
    let err = compiler()
        .compile(&CompileRequest {
            source: &source,
            entry_points: &["usesSvVertexIdVertex"],
            include_dirs: &[],
            optimization: Optimization::Debug,
        })
        .expect_err("SV_VertexID needs DrawParameters");
    match &err {
        ShaderError::DisallowedCapabilities { capabilities, .. } => {
            assert_eq!(capabilities, &vec![Capability(4427)]);
            assert!(err.to_string().contains("DrawParameters"), "{err}");
        }
        other => panic!("expected DisallowedCapabilities, got {other}"),
    }
}
