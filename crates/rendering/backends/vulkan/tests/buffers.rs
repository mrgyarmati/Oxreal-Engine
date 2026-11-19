//! GPU buffers: writes, readback, device addresses and vertex pulling.
//!
//! The pixel tests draw a triangle whose vertices come from a buffer, so they check the whole
//! path: write, address, shader read. They also check that writes take effect in submission
//! order (ADR-0016 §2).
//!
//! They need a Vulkan 1.3 device, the validation layers and the pinned Slang
//! (`cargo xtask slang install`). They're ignored by default. Run them with
//! `cargo test -p oxreal-rhi-vulkan -- --ignored`. CI runs them on Lavapipe.

use std::path::PathBuf;
use std::sync::OnceLock;

use oxreal_math::LinearRgba;
use oxreal_rhi::{
    BufferDesc, BufferHandle, BufferUsage, ColorAttachment, CommandEncoder, CullMode, Device as _,
    DeviceDesc, GpuSelection, LoadOp, PipelineHandle, RenderPassDesc, RenderPipelineDesc, RhiError,
    ShaderCode, ShaderEntry, StoreOp, TextureDesc, TextureFormat, TextureHandle, TextureUsage,
    Validation,
};
use oxreal_rhi_vulkan::VulkanDevice;
use oxreal_shader_compiler::{CompileRequest, Optimization, SlangCompiler, validate_spirv};

const SIZE: u32 = 64;
const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
/// `Vertex` in PulledTriangle.slang: position [f32; 2], color [f32; 4].
const VERTEX_BYTES: u64 = 24;

fn device() -> VulkanDevice {
    let _ = oxreal_diagnostics::init_application_logging();
    let device = oxreal_rhi_vulkan::create_device(&DeviceDesc {
        app_name: "oxreal buffer tests".to_owned(),
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

fn invalid<T: std::fmt::Debug>(result: Result<T, RhiError>) -> String {
    match result {
        Err(RhiError::InvalidUsage { details }) => details,
        other => panic!("expected InvalidUsage, got {other:?}"),
    }
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
    // tests only pass it addresses of live vertex buffers with three vertices.
    let code = unsafe { ShaderCode::from_spirv_unchecked(pulled_spirv()) };
    device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "pulled triangle",
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
            cull: CullMode::None,
        })
        .expect("pipeline")
}

/// Three vertices in `PulledTriangle.slang`'s layout, all with `color`.
fn vertices(points: [[f32; 2]; 3], color: [f32; 4]) -> Vec<u8> {
    points
        .iter()
        .flat_map(|p| p.iter().chain(color.iter()))
        .flat_map(|f| f.to_le_bytes())
        .collect()
}

/// Push constants: vertex buffer address, rotation, aspect.
fn constants(address: u64) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&address.to_le_bytes());
    bytes[8..12].copy_from_slice(&0.0_f32.to_le_bytes());
    bytes[12..].copy_from_slice(&1.0_f32.to_le_bytes());
    bytes
}

fn vertex_buffer(device: &VulkanDevice) -> BufferHandle {
    device
        .create_buffer(&BufferDesc {
            label: "vertices",
            size: 3 * VERTEX_BYTES,
            usage: BufferUsage::SHADER_ADDRESS | BufferUsage::COPY_DESTINATION,
        })
        .expect("buffer")
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

fn record_draw(
    encoder: &mut dyn CommandEncoder,
    texture: TextureHandle,
    pipeline: PipelineHandle,
    address: u64,
) {
    let attachments = [ColorAttachment {
        texture,
        load: LoadOp::Clear(LinearRgba::new(0.0, 0.0, 0.0, 1.0)),
        store: StoreOp::Store,
    }];
    let mut pass = encoder.begin_render_pass(&RenderPassDesc {
        label: "pulled",
        color_attachments: &attachments,
        depth_attachment: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_push_constants(&constants(address));
    pass.draw(3, 1);
}

/// The (red, green) of the pixel at (x, y), top row first.
fn pixel(pixels: &[u8], x: u32, y: u32) -> (u8, u8) {
    let i = ((y * SIZE + x) * 4) as usize;
    (pixels[i], pixels[i + 1])
}

/// A triangle in the left half (red) and one in the right half (green). Pixel columns 16 and 48
/// are their centres, row 32 the middle.
const LEFT: [[f32; 2]; 3] = [[-0.5, 0.5], [-0.9, -0.5], [-0.1, -0.5]];
const RIGHT: [[f32; 2]; 3] = [[0.5, 0.5], [0.1, -0.5], [0.9, -0.5]];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

#[test]
#[ignore = "requires a Vulkan 1.3 device and validation layers"]
fn writes_and_reads_round_trip() {
    let device = device();
    let buffer = device
        .create_buffer(&BufferDesc {
            label: "round trip",
            size: 256,
            usage: BufferUsage::COPY_DESTINATION | BufferUsage::COPY_SOURCE,
        })
        .expect("buffer");
    let first: Vec<u8> = (0..=255).collect();
    device.write_buffer(buffer, 0, &first).expect("write");
    device
        .write_buffer(buffer, 100, &[7; 20])
        .expect("write at offset");
    let read = device.read_buffer(buffer).expect("read");
    let mut expected = first;
    expected[100..120].fill(7);
    assert_eq!(read, expected, "later writes win, in submission order");
    device.destroy_buffer(buffer).expect("destroy");
    assert_no_validation_errors(&device);
}

#[test]
#[ignore = "requires a Vulkan 1.3 device and validation layers"]
fn misuse_is_refused_with_reasons() {
    let device = device();
    let desc = |size, usage| BufferDesc {
        label: "misuse",
        size,
        usage,
    };
    assert!(
        invalid(device.create_buffer(&desc(0, BufferUsage::COPY_SOURCE))).contains("zero size")
    );
    assert!(invalid(device.create_buffer(&desc(16, BufferUsage::default()))).contains("no usage"));
    // Past maxBufferSize, which maintenance4 makes invalid to pass to the driver.
    let huge = invalid(device.create_buffer(&desc(u64::MAX, BufferUsage::COPY_SOURCE)));
    assert!(huge.contains("maxBufferSize"), "{huge}");

    let write_only = device
        .create_buffer(&desc(16, BufferUsage::COPY_DESTINATION))
        .expect("buffer");
    assert!(invalid(device.write_buffer(write_only, 8, &[0; 16])).contains("don't fit"));
    assert!(invalid(device.write_buffer(write_only, u64::MAX, &[0; 4])).contains("don't fit"));
    assert!(invalid(device.write_buffer(write_only, 0, &[])).contains("don't fit"));
    assert!(invalid(device.read_buffer(write_only)).contains("COPY_SOURCE"));
    assert!(invalid(device.buffer_address(write_only)).contains("SHADER_ADDRESS"));

    let read_only = device
        .create_buffer(&desc(16, BufferUsage::COPY_SOURCE))
        .expect("buffer");
    assert!(invalid(device.write_buffer(read_only, 0, &[0; 4])).contains("COPY_DESTINATION"));

    device.destroy_buffer(write_only).expect("destroy");
    assert!(invalid(device.write_buffer(write_only, 0, &[0; 4])).contains("does not exist"));
    assert!(invalid(device.destroy_buffer(write_only)).contains("does not exist"));
    device.destroy_buffer(read_only).expect("destroy");
    assert_no_validation_errors(&device);
}

/// The vertex shader reads its vertices from a buffer by address, and a rewrite moves the
/// triangle.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn shaders_read_vertices_through_buffer_addresses() {
    let device = device();
    let pipeline = pulled_pipeline(&device);
    let buffer = vertex_buffer(&device);
    let address = device.buffer_address(buffer).expect("address");
    assert_ne!(address, 0);
    assert_eq!(
        device.buffer_address(buffer).expect("address"),
        address,
        "addresses are stable"
    );
    let texture = target(&device);

    let draw = || {
        let mut encoder = device.create_encoder("pulled").expect("encoder");
        record_draw(encoder.as_mut(), texture, pipeline, address);
        let submission = device.submit(encoder).expect("submit");
        device.wait_for_submission(submission).expect("wait");
        device.read_texture(texture).expect("readback")
    };

    device
        .write_buffer(buffer, 0, &vertices(LEFT, RED))
        .expect("write");
    let left = draw();
    assert_eq!(pixel(&left, 16, 32), (255, 0), "red triangle on the left");
    assert_eq!(pixel(&left, 48, 32), (0, 0), "nothing on the right");

    device
        .write_buffer(buffer, 0, &vertices(RIGHT, GREEN))
        .expect("rewrite");
    let right = draw();
    assert_eq!(
        pixel(&right, 16, 32),
        (0, 0),
        "nothing on the left any more"
    );
    assert_eq!(
        pixel(&right, 48, 32),
        (0, 255),
        "green triangle on the right"
    );

    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_buffer(buffer).expect("destroy buffer");
    device.destroy_texture(texture).expect("destroy texture");
    assert_no_validation_errors(&device);
}

/// Writes take effect in *submission* order: an encoder recorded before a write, but submitted
/// after it, sees the new data.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn writes_take_effect_in_submission_order() {
    let device = device();
    let pipeline = pulled_pipeline(&device);
    let buffer = vertex_buffer(&device);
    let address = device.buffer_address(buffer).expect("address");
    let texture = target(&device);
    device
        .write_buffer(buffer, 0, &vertices(LEFT, RED))
        .expect("write");

    let mut encoder = device
        .create_encoder("recorded before the rewrite")
        .expect("encoder");
    record_draw(encoder.as_mut(), texture, pipeline, address);
    device
        .write_buffer(buffer, 0, &vertices(RIGHT, GREEN))
        .expect("rewrite");
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");

    let pixels = device.read_texture(texture).expect("readback");
    assert_eq!(pixel(&pixels, 48, 32), (0, 255), "the draw saw the rewrite");
    assert_eq!(pixel(&pixels, 16, 32), (0, 0));

    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_buffer(buffer).expect("destroy buffer");
    device.destroy_texture(texture).expect("destroy texture");
    assert_no_validation_errors(&device);
}

/// Destroying a buffer an unsubmitted encoder uses (through its address) waits for that encoder.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn buffers_destroyed_while_recording_outlive_the_submission() {
    let device = device();
    let pipeline = pulled_pipeline(&device);
    let buffer = vertex_buffer(&device);
    let address = device.buffer_address(buffer).expect("address");
    let texture = target(&device);
    device
        .write_buffer(buffer, 0, &vertices(LEFT, RED))
        .expect("write");

    let mut encoder = device.create_encoder("uses the buffer").expect("encoder");
    record_draw(encoder.as_mut(), texture, pipeline, address);
    device.destroy_buffer(buffer).expect("destroy buffer");
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");
    let pixels = device.read_texture(texture).expect("readback");
    assert_eq!(
        pixel(&pixels, 16, 32),
        (255, 0),
        "the buffer was still there"
    );

    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_texture(texture).expect("destroy texture");
    assert_no_validation_errors(&device);
}

/// Many writes and reads from several threads at once: every write lands, in each thread's order.
#[test]
#[ignore = "requires a Vulkan 1.3 device and validation layers"]
fn concurrent_writes_and_reads_are_safe() {
    let device = device();
    let buffers: Vec<BufferHandle> = (0..4)
        .map(|_| {
            device
                .create_buffer(&BufferDesc {
                    label: "concurrent",
                    size: 64,
                    usage: BufferUsage::COPY_DESTINATION | BufferUsage::COPY_SOURCE,
                })
                .expect("buffer")
        })
        .collect();
    std::thread::scope(|scope| {
        for (n, &buffer) in buffers.iter().enumerate() {
            let device = &device;
            scope.spawn(move || {
                for round in 0..30_u8 {
                    let value = round
                        .wrapping_mul(7)
                        .wrapping_add(u8::try_from(n).unwrap_or(0));
                    device.write_buffer(buffer, 0, &[value; 64]).expect("write");
                    assert_eq!(device.read_buffer(buffer).expect("read"), vec![value; 64]);
                }
            });
        }
    });
    for buffer in buffers {
        device.destroy_buffer(buffer).expect("destroy");
    }
    assert_no_validation_errors(&device);
}
