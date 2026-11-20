//! The memory layouts `examples/shaders/Mesh.slang` compiles to match what the Rust side packs.
//!
//! The cube example writes `MeshVertex` as 8 little-endian `f32`s (32 bytes) and the push
//! constants as a matrix, an address and two indices (80 bytes), and its `unsafe` shader
//! contract relies on the shader reading exactly that. These layouts come from Slang's natural
//! (C-like) layout for pointer targets, which `scalarBlockLayout` permits. A Slang upgrade or a
//! compiler flag could change them, and neither `spirv-val` nor the validation layers would
//! notice the out-of-bounds reads that would follow. This test reads the layout decorations from
//! the SPIR-V itself.
//!
//! It needs the pinned Slang (`cargo xtask slang install`) but no GPU. It's ignored by default
//! like the GPU tests. Run it with `cargo test -p oxreal-rhi-vulkan -- --ignored`.

use std::collections::HashMap;
use std::path::PathBuf;

use oxreal_shader_compiler::{CompileRequest, Optimization, SlangCompiler};

const OP_NAME: u32 = 5;
const OP_TYPE_POINTER: u32 = 32;
const OP_DECORATE: u32 = 71;
const OP_MEMBER_DECORATE: u32 = 72;
const DECORATION_ARRAY_STRIDE: u32 = 6;
const DECORATION_OFFSET: u32 = 35;
const STORAGE_PHYSICAL_STORAGE_BUFFER: u32 = 5349;

/// The layout facts this test needs, gathered from a SPIR-V module.
#[derive(Default)]
struct Layouts {
    /// Debug names of result ids.
    names: HashMap<u32, String>,
    /// Member offsets of each struct, by member index.
    offsets: HashMap<u32, Vec<(u32, u32)>>,
    /// ArrayStride decorations, by id.
    strides: HashMap<u32, u32>,
    /// Physical-storage-buffer pointer types: (pointer id, pointee id).
    pointers: Vec<(u32, u32)>,
}

impl Layouts {
    fn parse(spirv: &[u32]) -> Self {
        let mut layouts = Self::default();
        let mut at = 5; // After the header.
        while at < spirv.len() {
            let count = (spirv[at] >> 16) as usize;
            let opcode = spirv[at] & 0xFFFF;
            assert!(count > 0 && at + count <= spirv.len(), "malformed SPIR-V");
            let operands = &spirv[at + 1..at + count];
            match opcode {
                OP_NAME => {
                    let bytes: Vec<u8> =
                        operands[1..].iter().flat_map(|w| w.to_le_bytes()).collect();
                    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                    let name = String::from_utf8_lossy(&bytes[..end]).into_owned();
                    layouts.names.insert(operands[0], name);
                }
                OP_MEMBER_DECORATE if operands[2] == DECORATION_OFFSET => {
                    layouts
                        .offsets
                        .entry(operands[0])
                        .or_default()
                        .push((operands[1], operands[3]));
                }
                OP_DECORATE if operands[1] == DECORATION_ARRAY_STRIDE => {
                    layouts.strides.insert(operands[0], operands[2]);
                }
                OP_TYPE_POINTER if operands[1] == STORAGE_PHYSICAL_STORAGE_BUFFER => {
                    layouts.pointers.push((operands[0], operands[2]));
                }
                _ => {}
            }
            at += count;
        }
        layouts
    }

    /// The ids of structs whose debug name starts with `prefix`.
    fn structs_named(&self, prefix: &str) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .names
            .iter()
            .filter(|(id, name)| name.starts_with(prefix) && self.offsets.contains_key(id))
            .map(|(&id, _)| id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Member offsets of struct `id`, in member order.
    fn member_offsets(&self, id: u32) -> Vec<u32> {
        let mut members = self.offsets[&id].clone();
        members.sort_unstable();
        members.into_iter().map(|(_, offset)| offset).collect()
    }
}

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
    let layouts = Layouts::parse(&spirv);

    // The vertex type the pointer reads, and the stride between vertices.
    let vertices = layouts.structs_named("MeshVertex");
    assert!(!vertices.is_empty(), "no MeshVertex struct in the SPIR-V");
    for &id in &vertices {
        assert_eq!(
            layouts.member_offsets(id),
            [0, 12, 24],
            "MeshVertex members: position, normal, uv"
        );
    }
    let strides: Vec<u32> = layouts
        .pointers
        .iter()
        .filter(|(_, pointee)| vertices.contains(pointee))
        .filter_map(|(pointer, _)| layouts.strides.get(pointer).copied())
        .collect();
    assert_eq!(
        strides,
        [32],
        "one pointer type to MeshVertex, with a 32-byte stride (the Rust packing)"
    );

    let constants = layouts.structs_named("MeshConstants");
    assert!(
        !constants.is_empty(),
        "no MeshConstants struct in the SPIR-V"
    );
    for &id in &constants {
        assert_eq!(
            layouts.member_offsets(id),
            [0, 64, 72, 76],
            "MeshConstants members: matrix, vertex address, texture, sampler"
        );
    }
}
