//! The memory layouts `examples/shaders/Mesh.slang` compiles to match what the Rust side packs.
//!
//! The cube example and the GPU tests write `MeshVertex` as 8 little-endian `f32`s (32 bytes)
//! and the push constants as a matrix, an address and two indices (80 bytes), and their `unsafe`
//! shader contracts rely on the shader reading exactly that. The layouts are read from the
//! SPIR-V itself ([`SpirvLayouts`]).
//!
//! It needs the pinned Slang (`cargo xtask slang install`) but no GPU. It's ignored by default
//! like the GPU tests. Run it with `cargo test -p oxreal-rhi-vulkan -- --ignored`.

use std::path::PathBuf;

use oxreal_shader_compiler::{CompileRequest, Optimization, SlangCompiler, SpirvLayouts};

#[test]
#[ignore = "requires the pinned Slang"]
fn mesh_shader_layouts_match_the_rust_packing() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let compiler = SlangCompiler::locate().expect("pinned Slang: run `cargo xtask slang install`");
    let spirv = compiler
        .compile(&CompileRequest {
            source: &crate_dir.join("examples/shaders/Mesh.slang"),
            entry_points: &["meshVertex", "meshPixel"],
            include_dirs: &[crate_dir.join("../../rhi/shaders")],
            // Debug keeps the names this test looks types up by. Layout doesn't depend on it.
            optimization: Optimization::Debug,
        })
        .expect("Mesh.slang compiles")
        .spirv;
    let layouts = SpirvLayouts::parse(&spirv).expect("well-formed SPIR-V");

    let vertices = layouts.member_offsets("MeshVertex");
    assert!(!vertices.is_empty(), "no MeshVertex struct in the SPIR-V");
    for offsets in &vertices {
        assert_eq!(
            offsets,
            &[0, 12, 24],
            "MeshVertex members: position, normal, uv"
        );
    }
    assert_eq!(
        layouts.pointer_strides("MeshVertex"),
        [32],
        "one pointer type to MeshVertex, with a 32-byte stride (the Rust packing)"
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
            "MeshConstants members: matrix, vertex address, texture, sampler"
        );
    }
}
