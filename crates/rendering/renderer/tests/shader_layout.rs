//! The memory layouts `shaders/Mesh.slang` compiles to match what the renderer packs.
//!
//! The renderer writes `Vertex` as 8 little-endian `f32`s (32 bytes) and the push constants as a
//! matrix, an address and two indices (80 bytes), and its `unsafe` shader contract relies on the
//! shader reading exactly that (`src/renderer.rs`). The layouts are read from the SPIR-V itself.
//!
//! It needs the pinned Slang but no GPU, and is ignored by default. Run it with
//! `cargo test -p oxreal-renderer -- --ignored`.

mod common;

use oxreal_shader_compiler::{CompileRequest, Optimization, SlangCompiler, SpirvLayouts};

#[test]
#[ignore = "requires the pinned Slang"]
fn mesh_shader_layouts_match_the_renderers_packing() {
    let workspace = common::workspace();
    let compiler = SlangCompiler::locate().expect("pinned Slang: run `cargo xtask slang install`");
    let spirv = compiler
        .compile(&CompileRequest {
            source: &workspace.join("crates/rendering/renderer/shaders/Mesh.slang"),
            entry_points: &["meshVertex", "meshPixel"],
            include_dirs: &[workspace.join("crates/rendering/rhi/shaders")],
            // Debug keeps the names this test looks types up by. Layout doesn't depend on it.
            optimization: Optimization::Debug,
        })
        .expect("Mesh.slang compiles")
        .spirv;
    let layouts = SpirvLayouts::parse(&spirv).expect("well-formed SPIR-V");

    let vertices = layouts.member_offsets("MeshVertex");
    assert!(!vertices.is_empty(), "no MeshVertex struct in the SPIR-V");
    for offsets in &vertices {
        assert_eq!(offsets, &[0, 12, 24], "position, normal, uv");
    }
    assert_eq!(
        layouts.pointer_strides("MeshVertex"),
        [32],
        "one pointer type to MeshVertex, with the renderer's 32-byte stride"
    );
    let constants = layouts.member_offsets("MeshConstants");
    assert!(
        !constants.is_empty(),
        "no MeshConstants struct in the SPIR-V"
    );
    for offsets in &constants {
        assert_eq!(
            offsets,
            &[0, 64, 72, 76],
            "matrix, vertex address, texture index, sampler index"
        );
    }
}
