//! Indexed draws: index buffers, index offsets, vertex offsets and index misuse.
//!
//! The pixel tests draw quads from four vertices and six indices with
//! `examples/shaders/PulledTriangle.slang`, which reads each vertex from a buffer by its vertex
//! ID. Only a correct index path covers a whole quad: wrong indices draw a different triangle or
//! a degenerate one, and pixels near opposite corners of each quad catch either.
//!
//! They need a Vulkan 1.3 device, the validation layers and the pinned Slang
//! (`cargo xtask slang install`). They're ignored by default. Run them with
//! `cargo test -p oxreal-rhi-vulkan -- --ignored`. CI runs them on Lavapipe.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::OnceLock;

use oxreal_math::LinearRgba;
use oxreal_rhi::{
    BufferDesc, BufferHandle, BufferUsage, ColorAttachment, CommandEncoder, CullMode, Device as _,
    DeviceDesc, GpuSelection, IndexFormat, LoadOp, PipelineHandle, RenderPassDesc,
    RenderPipelineDesc, RhiError, ShaderCode, ShaderEntry, StoreOp, TextureDesc, TextureFormat,
    TextureHandle, TextureUsage, Validation,
};
use oxreal_rhi_vulkan::VulkanDevice;
use oxreal_shader_compiler::{CompileRequest, Optimization, SlangCompiler, validate_spirv};

const SIZE: u32 = 64;
const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
/// `Vertex` in PulledTriangle.slang: position [f32; 2], color [f32; 4].
const VERTEX_BYTES: usize = 24;

fn device() -> VulkanDevice {
    let _ = oxreal_diagnostics::init_application_logging();
    let device = oxreal_rhi_vulkan::create_device(&DeviceDesc {
        app_name: "oxreal indexed draw tests".to_owned(),
        validation: Validation::Enabled,
        gpu: GpuSelection::Automatic,
    })
    .expect("device creation");
    assert!(
        device.validation_enabled(),
        "validation layers must be installed"
    );
    device
}

fn assert_no_validation_errors(device: &VulkanDevice) {
    device.wait_idle().expect("wait idle");
    assert_eq!(
        device.validation_error_count(),
        0,
        "validation reported errors (see log)"
    );
}

/// `PulledTriangle.slang`, compiled once per test binary and checked with `spirv-val` (mandatory
/// in CI).
fn pulled_spirv() -> &'static [u32] {
    static SPIRV: OnceLock<Vec<u32>> = OnceLock::new();
    SPIRV.get_or_init(|| {
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/shaders/PulledTriangle.slang");
        let compiler =
            SlangCompiler::locate().expect("pinned Slang: run `cargo xtask slang install`");
        let spirv = compiler
            .compile(&CompileRequest {
                source: &source,
                entry_points: &["pulledVertex", "pulledPixel"],
                include_dirs: &[],
                optimization: Optimization::Debug,
            })
            .expect("PulledTriangle.slang compiles")
            .spirv;
        let validated = validate_spirv(&spirv, &source).expect("valid SPIR-V");
        let required = std::env::var("OXREAL_REQUIRE_SPIRV_VAL").is_ok_and(|v| v == "1");
        assert!(
            validated || !required,
            "OXREAL_REQUIRE_SPIRV_VAL=1 but spirv-val was not found"
        );
        spirv
    })
}

fn pulled_pipeline(device: &VulkanDevice) -> PipelineHandle {
    // SAFETY: produced by the pinned `slangc` and checked with spirv-val (mandatory in CI). The
    // tests only pass it addresses of live vertex buffers, and every index plus vertex offset
    // they draw with names a vertex inside that buffer.
    let code = unsafe { ShaderCode::from_spirv_unchecked(pulled_spirv()) };
    device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "pulled quads",
            vertex: ShaderEntry {
                code,
                entry_point: "pulledVertex",
            },
            pixel: ShaderEntry {
                code,
                entry_point: "pulledPixel",
            },
            color_formats: &[FORMAT],
            depth: None,
            push_constant_bytes: 16,
            // Culling on: a wrongly ordered index would also lose its triangle this way.
            cull: CullMode::Back,
        })
        .expect("pipeline")
}

/// Push constants: vertex buffer address, rotation, aspect.
fn constants(address: u64) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&address.to_le_bytes());
    bytes[8..12].copy_from_slice(&0.0_f32.to_le_bytes());
    bytes[12..].copy_from_slice(&1.0_f32.to_le_bytes());
    bytes
}

/// One vertex in `PulledTriangle.slang`'s layout.
fn vertex(position: [f32; 2], color: [f32; 4]) -> Vec<u8> {
    position
        .iter()
        .chain(color.iter())
        .flat_map(|f| f.to_le_bytes())
        .collect()
}

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

/// The corners of a quad spanning x in [left, right] and y in [-0.5, 0.5], counter-clockwise
/// from the top left: indices 0, 1, 2 and 0, 2, 3 cover it, both front-facing.
fn quad(left: f32, right: f32, color: [f32; 4]) -> Vec<u8> {
    [[left, 0.5], [left, -0.5], [right, -0.5], [right, 0.5]]
        .into_iter()
        .flat_map(|corner| vertex(corner, color))
        .collect()
}

/// Indices covering the quad whose first vertex is `base`.
fn quad_indices(base: u32) -> [u32; 6] {
    [base, base + 1, base + 2, base, base + 2, base + 3]
}

/// Eight vertices: a red quad on the left (0–3), then a green quad on the right (4–7).
fn two_quads() -> Vec<u8> {
    let mut bytes = quad(-0.9, -0.1, RED);
    bytes.extend(quad(0.1, 0.9, GREEN));
    bytes
}

fn u16_bytes(indices: &[u32]) -> Vec<u8> {
    indices
        .iter()
        .flat_map(|&i| u16::try_from(i).expect("fits u16").to_le_bytes())
        .collect()
}

fn u32_bytes(indices: &[u32]) -> Vec<u8> {
    indices.iter().flat_map(|i| i.to_le_bytes()).collect()
}

fn buffer(device: &VulkanDevice, label: &str, usage: BufferUsage, data: &[u8]) -> BufferHandle {
    let buffer = device
        .create_buffer(&BufferDesc {
            label,
            size: data.len() as u64,
            usage: usage | BufferUsage::COPY_DESTINATION,
        })
        .expect("buffer");
    device.write_buffer(buffer, 0, data).expect("write");
    buffer
}

fn target(device: &VulkanDevice) -> TextureHandle {
    device
        .create_texture(&TextureDesc {
            label: "target",
            width: SIZE,
            height: SIZE,
            format: FORMAT,
            usage: TextureUsage::RENDER_TARGET | TextureUsage::COPY_SOURCE,
        })
        .expect("texture")
}

/// How an index buffer is bound, and the draws made with it.
struct IndexedDraws<'a> {
    index_offset: u64,
    format: IndexFormat,
    /// Index range and vertex offset of each draw.
    draws: &'a [(Range<u32>, i32)],
}

/// Records a pass that clears to black, binds `indices` and makes `draws`.
fn record(
    encoder: &mut dyn CommandEncoder,
    texture: TextureHandle,
    pipeline: PipelineHandle,
    address: u64,
    indices: BufferHandle,
    draws: &IndexedDraws<'_>,
) {
    let attachments = [ColorAttachment {
        texture,
        load: LoadOp::Clear(LinearRgba::new(0.0, 0.0, 0.0, 1.0)),
        store: StoreOp::Store,
    }];
    let mut pass = encoder.begin_render_pass(&RenderPassDesc {
        label: "indexed",
        color_attachments: &attachments,
        depth_attachment: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_push_constants(&constants(address));
    pass.set_index_buffer(indices, draws.index_offset, draws.format);
    for (range, vertex_offset) in draws.draws {
        pass.draw_indexed(range.clone(), *vertex_offset, 1);
    }
}

/// The (red, green) of the pixel at (x, y), top row first.
fn pixel(pixels: &[u8], x: u32, y: u32) -> (u8, u8) {
    let i = ((y * SIZE + x) * 4) as usize;
    (pixels[i], pixels[i + 1])
}

/// Checks that both quads were drawn whole: pixels near the top-left and bottom-right corners
/// of each (one in each of its triangles), with black between and around them.
fn assert_two_quads(pixels: &[u8], case: &str) {
    // The left quad covers columns 3–28, the right one 35–60; both cover rows 16–47.
    for (x, y) in [(6, 19), (25, 44)] {
        assert_eq!(pixel(pixels, x, y), (255, 0), "{case}: red at ({x}, {y})");
    }
    for (x, y) in [(38, 19), (57, 44)] {
        assert_eq!(pixel(pixels, x, y), (0, 255), "{case}: green at ({x}, {y})");
    }
    for (x, y) in [(32, 32), (16, 4), (48, 60)] {
        assert_eq!(pixel(pixels, x, y), (0, 0), "{case}: black at ({x}, {y})");
    }
}

/// 16- and 32-bit indices, with index-buffer offsets, first indices and vertex offsets (positive
/// and negative), all draw the same two quads.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn indexed_draws_pull_vertices_through_indices() {
    let device = device();
    let pipeline = pulled_pipeline(&device);
    let vertices = buffer(
        &device,
        "two quads",
        BufferUsage::SHADER_ADDRESS,
        &two_quads(),
    );
    let address = device.buffer_address(vertices).expect("address");
    let texture = target(&device);

    let both = [quad_indices(0), quad_indices(4)].concat();
    // Two padding indices first, skipped by the bind offset.
    let padded_u16 = u16_bytes(&[[9, 9].as_slice(), &quad_indices(0)].concat());
    let cases: [(&str, Vec<u8>, IndexedDraws<'_>); 4] = [
        (
            "u16, one draw per quad",
            u16_bytes(&both),
            IndexedDraws {
                index_offset: 0,
                format: IndexFormat::Uint16,
                draws: &[(0..6, 0), (6..12, 0)],
            },
        ),
        (
            "u32, one draw for both",
            u32_bytes(&both),
            IndexedDraws {
                index_offset: 0,
                format: IndexFormat::Uint32,
                draws: &[(0..12, 0)],
            },
        ),
        (
            "u16 at a bind offset, shared indices with vertex offsets",
            padded_u16,
            IndexedDraws {
                index_offset: 4,
                format: IndexFormat::Uint16,
                draws: &[(0..6, 0), (0..6, 4)],
            },
        ),
        (
            "u32 with a negative vertex offset",
            u32_bytes(&quad_indices(4)),
            IndexedDraws {
                index_offset: 0,
                format: IndexFormat::Uint32,
                draws: &[(0..6, -4), (0..6, 0)],
            },
        ),
    ];
    for (case, index_bytes, draws) in &cases {
        let indices = buffer(&device, "indices", BufferUsage::INDEX, index_bytes);
        let mut encoder = device.create_encoder(case).expect("encoder");
        record(encoder.as_mut(), texture, pipeline, address, indices, draws);
        let submission = device.submit(encoder).expect("submit");
        device.wait_for_submission(submission).expect("wait");
        let pixels = device.read_texture(texture).expect("readback");
        assert_two_quads(&pixels, case);
        device.destroy_buffer(indices).expect("destroy indices");
    }

    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_buffer(vertices).expect("destroy vertices");
    device.destroy_texture(texture).expect("destroy texture");
    assert_no_validation_errors(&device);
}

/// Primitive restart is off: `0xFFFF` is an ordinary 16-bit index, here naming the last of
/// 65 536 vertices. With restart on, the quad would lose both triangles.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn the_largest_16_bit_index_is_an_ordinary_index() {
    let device = device();
    let pipeline = pulled_pipeline(&device);
    const LAST: u32 = 0xFFFF;
    // Vertices 0–3 are the red quad, except that its third corner lives at vertex 0xFFFF.
    // Everything else is zero (a degenerate point, never referenced).
    let red = quad(-0.9, -0.1, RED);
    let mut bytes = vec![0; (LAST as usize + 1) * VERTEX_BYTES];
    bytes[..red.len()].copy_from_slice(&red);
    let third = 2 * VERTEX_BYTES..3 * VERTEX_BYTES;
    bytes[LAST as usize * VERTEX_BYTES..].copy_from_slice(&red[third]);
    let green = quad(0.1, 0.9, GREEN);
    // The green quad as vertices 4–7.
    bytes[4 * VERTEX_BYTES..8 * VERTEX_BYTES].copy_from_slice(&green);

    let vertices = buffer(
        &device,
        "65 536 vertices",
        BufferUsage::SHADER_ADDRESS,
        &bytes,
    );
    let address = device.buffer_address(vertices).expect("address");
    let indices = buffer(
        &device,
        "indices with 0xFFFF",
        BufferUsage::INDEX,
        &u16_bytes(&[[0, 1, LAST, 0, LAST, 3].as_slice(), &quad_indices(4)].concat()),
    );
    let texture = target(&device);

    let mut encoder = device.create_encoder("0xFFFF").expect("encoder");
    record(
        encoder.as_mut(),
        texture,
        pipeline,
        address,
        indices,
        &IndexedDraws {
            index_offset: 0,
            format: IndexFormat::Uint16,
            draws: &[(0..12, 0)],
        },
    );
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");
    let pixels = device.read_texture(texture).expect("readback");
    assert_two_quads(&pixels, "0xFFFF");

    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_buffer(indices).expect("destroy indices");
    device.destroy_buffer(vertices).expect("destroy vertices");
    device.destroy_texture(texture).expect("destroy texture");
    assert_no_validation_errors(&device);
}

/// Every kind of index misuse is refused at submit with a reason, and nothing invalid reaches
/// the driver (the validation layers stay silent).
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn index_misuse_is_refused_with_reasons() {
    let device = device();
    let pipeline = pulled_pipeline(&device);
    let vertices = buffer(
        &device,
        "two quads",
        BufferUsage::SHADER_ADDRESS,
        &two_quads(),
    );
    let address = device.buffer_address(vertices).expect("address");
    // Six u16 indices: 12 bytes.
    let indices = buffer(
        &device,
        "indices",
        BufferUsage::INDEX,
        &u16_bytes(&quad_indices(0)),
    );
    let destroyed = buffer(&device, "destroyed", BufferUsage::INDEX, &[0; 4]);
    device.destroy_buffer(destroyed).expect("destroy");
    let texture = target(&device);

    let attachments = [ColorAttachment {
        texture,
        load: LoadOp::Clear(LinearRgba::new(0.0, 0.0, 0.0, 1.0)),
        store: StoreOp::Store,
    }];
    let desc = RenderPassDesc {
        label: "misuse",
        color_attachments: &attachments,
        depth_attachment: None,
    };
    let mut encoder = device.create_encoder("misuse").expect("encoder");
    {
        let mut pass = encoder.begin_render_pass(&desc);
        pass.set_pipeline(pipeline);
        pass.set_push_constants(&constants(address));
        pass.draw_indexed(0..6, 0, 1); // Nothing bound yet.
        pass.set_index_buffer(vertices, 0, IndexFormat::Uint16); // No INDEX usage.
        pass.set_index_buffer(indices, 1, IndexFormat::Uint16); // Misaligned.
        pass.set_index_buffer(indices, 12, IndexFormat::Uint16); // At the end.
        pass.set_index_buffer(destroyed, 0, IndexFormat::Uint16); // Destroyed.
        pass.set_index_buffer(indices, 0, IndexFormat::Uint32); // Three u32 indices fit.
        pass.draw_indexed(0..6, 0, 1); // Six don't.
        pass.set_index_buffer(indices, 4, IndexFormat::Uint16); // Four u16 indices from here.
        pass.draw_indexed(0..5, 0, 1); // Past the end.
        #[expect(
            clippy::reversed_empty_ranges,
            reason = "testing that misuse is refused"
        )]
        pass.draw_indexed(4..2, 0, 1);
        pass.draw_indexed(0..4, 0, 1); // Fine.
    }
    {
        let mut pass = encoder.begin_render_pass(&desc);
        pass.set_pipeline(pipeline);
        pass.set_push_constants(&constants(address));
        pass.draw_indexed(0..3, 0, 1); // Bindings don't carry over between passes.
    }
    {
        let mut pass = encoder.begin_render_pass(&desc);
        pass.set_index_buffer(indices, 0, IndexFormat::Uint16);
        pass.draw_indexed(0..6, 0, 1); // No pipeline.
    }
    let error = match device.submit(encoder) {
        Err(RhiError::InvalidUsage { details }) => details,
        other => panic!("expected InvalidUsage, got {other:?}"),
    };
    for expected in [
        "draw_indexed without a successful set_index_buffer in this pass",
        "was not created with BufferUsage::INDEX",
        "offset 1 is not a multiple of the Uint16 index size (2 bytes)",
        "offset 12 is not inside",
        "does not exist",
        "index range 0..6 reads past the end",
        "which holds 3 indices",
        "index range 0..5 reads past the end",
        "which holds 4 indices",
        "index range 4..2 ends before it starts",
        "draw_indexed before a successful set_pipeline in this pass",
    ] {
        assert!(error.contains(expected), "missing {expected:?} in: {error}");
    }
    assert_eq!(
        error
            .matches("draw_indexed without a successful set_index_buffer")
            .count(),
        2,
        "the first pass's first draw and the second pass's draw: {error}"
    );

    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_buffer(indices).expect("destroy indices");
    device.destroy_buffer(vertices).expect("destroy vertices");
    device.destroy_texture(texture).expect("destroy texture");
    assert_no_validation_errors(&device);
}

/// Destroying an index buffer that an unsubmitted encoder has bound waits for that encoder.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn index_buffers_destroyed_while_recording_outlive_the_submission() {
    let device = device();
    let pipeline = pulled_pipeline(&device);
    let vertices = buffer(
        &device,
        "two quads",
        BufferUsage::SHADER_ADDRESS,
        &two_quads(),
    );
    let address = device.buffer_address(vertices).expect("address");
    let indices = buffer(
        &device,
        "indices",
        BufferUsage::INDEX,
        &u32_bytes(&[quad_indices(0), quad_indices(4)].concat()),
    );
    let texture = target(&device);

    let mut encoder = device
        .create_encoder("uses the index buffer")
        .expect("encoder");
    record(
        encoder.as_mut(),
        texture,
        pipeline,
        address,
        indices,
        &IndexedDraws {
            index_offset: 0,
            format: IndexFormat::Uint32,
            draws: &[(0..12, 0)],
        },
    );
    device.destroy_buffer(indices).expect("destroy indices");
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");
    let pixels = device.read_texture(texture).expect("readback");
    assert_two_quads(&pixels, "destroyed while recording");

    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_buffer(vertices).expect("destroy vertices");
    device.destroy_texture(texture).expect("destroy texture");
    assert_no_validation_errors(&device);
}
