//! Bindless textures and samplers (ADR-0016 §2): uploads, sampling by index, render-to-texture,
//! and index lifetimes.
//!
//! The pixel tests draw a full-target quad that samples a texture through its bindless index
//! (`examples/shaders/TexturedQuad.slang`), so they check the whole path: upload, descriptor,
//! layout, sampler, and orientation.
//!
//! They need a Vulkan 1.3 device, the validation layers and the pinned Slang
//! (`cargo xtask slang install`). They're ignored by default. Run them with
//! `cargo test -p oxreal-rhi-vulkan -- --ignored`. CI runs them on Lavapipe.

use std::path::PathBuf;
use std::sync::OnceLock;

use oxreal_math::LinearRgba;
use oxreal_rhi::{
    AddressMode, ColorAttachment, CommandEncoder, CullMode, Device as _, DeviceDesc, Filter,
    GpuSelection, LoadOp, PipelineHandle, RenderPassDesc, RenderPipelineDesc, RhiError,
    SamplerDesc, SamplerHandle, ShaderCode, ShaderEntry, StoreOp, TextureDesc, TextureFormat,
    TextureHandle, TextureUsage, Validation,
};
use oxreal_rhi_vulkan::VulkanDevice;
use oxreal_shader_compiler::{CompileRequest, Optimization, SlangCompiler, validate_spirv};

const SIZE: u32 = 64;
const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

fn device() -> VulkanDevice {
    let _ = oxreal_diagnostics::init_application_logging();
    let device = oxreal_rhi_vulkan::create_device(&DeviceDesc {
        app_name: "oxreal texture tests".to_owned(),
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

/// `TexturedQuad.slang` (which imports the RHI's `Bindless.slang`), compiled once per test
/// binary and checked with `spirv-val` (mandatory in CI).
fn quad_spirv() -> &'static [u32] {
    static SPIRV: OnceLock<Vec<u32>> = OnceLock::new();
    SPIRV.get_or_init(|| {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source = crate_dir.join("examples/shaders/TexturedQuad.slang");
        let bindless = crate_dir.join("../../rhi/shaders");
        let compiler =
            SlangCompiler::locate().expect("pinned Slang: run `cargo xtask slang install`");
        let spirv = compiler
            .compile(&CompileRequest {
                source: &source,
                entry_points: &["quadVertex", "quadPixel"],
                include_dirs: &[bindless],
                optimization: Optimization::Debug,
            })
            .expect("TexturedQuad.slang compiles")
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

fn quad_pipeline(device: &VulkanDevice) -> PipelineHandle {
    // SAFETY: produced by the pinned `slangc` and checked with spirv-val (mandatory in CI). The
    // tests only pass it indices of live textures and samplers.
    let code = unsafe { ShaderCode::from_spirv_unchecked(quad_spirv()) };
    device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "textured quad",
            vertex: ShaderEntry {
                code,
                entry_point: "quadVertex",
            },
            pixel: ShaderEntry {
                code,
                entry_point: "quadPixel",
            },
            color_formats: &[FORMAT],
            depth: None,
            push_constant_bytes: 12,
            cull: CullMode::None,
        })
        .expect("pipeline")
}

/// QuadConstants: texture index, sampler index, scale.
fn constants(texture: u32, sampler: u32, scale: f32) -> [u8; 12] {
    let mut bytes = [0; 12];
    bytes[..4].copy_from_slice(&texture.to_le_bytes());
    bytes[4..8].copy_from_slice(&sampler.to_le_bytes());
    bytes[8..].copy_from_slice(&scale.to_le_bytes());
    bytes
}

fn texture(device: &VulkanDevice, size: u32, usage: TextureUsage) -> TextureHandle {
    device
        .create_texture(&TextureDesc {
            label: "texture",
            width: size,
            height: size,
            format: FORMAT,
            usage,
        })
        .expect("texture")
}

fn sampler(device: &VulkanDevice, filter: Filter, address_mode: AddressMode) -> SamplerHandle {
    device
        .create_sampler(&SamplerDesc {
            label: "sampler",
            mag_filter: filter,
            min_filter: filter,
            address_mode,
            max_anisotropy: None,
        })
        .expect("sampler")
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

/// A 2×2 texture: red and green on the top row, blue and white on the bottom row.
fn quadrants(device: &VulkanDevice) -> TextureHandle {
    let texture = texture(
        device,
        2,
        TextureUsage::SAMPLED | TextureUsage::COPY_DESTINATION,
    );
    let pixels: Vec<u8> = [RED, GREEN, BLUE, WHITE].concat();
    device.write_texture(texture, &pixels).expect("upload");
    texture
}

/// Records a pass on `target` that draws the quad sampling `texture` with `sampler`.
fn record_quad(
    device: &VulkanDevice,
    encoder: &mut dyn CommandEncoder,
    target: TextureHandle,
    pipeline: PipelineHandle,
    texture: TextureHandle,
    sampler: SamplerHandle,
    scale: f32,
) {
    let texture = device.texture_index(texture).expect("texture index");
    let sampler = device.sampler_index(sampler).expect("sampler index");
    let attachments = [ColorAttachment {
        texture: target,
        load: LoadOp::Clear(LinearRgba::new(0.0, 0.0, 0.0, 1.0)),
        store: StoreOp::Store,
    }];
    let mut pass = encoder.begin_render_pass(&RenderPassDesc {
        label: "quad",
        color_attachments: &attachments,
        depth_attachment: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_push_constants(&constants(texture, sampler, scale));
    pass.draw(3, 1);
}

/// Draws the quad into a fresh 64×64 target and reads it back.
fn draw(
    device: &VulkanDevice,
    pipeline: PipelineHandle,
    texture: TextureHandle,
    sampler: SamplerHandle,
    scale: f32,
) -> Vec<u8> {
    let target = self::texture(
        device,
        SIZE,
        TextureUsage::RENDER_TARGET | TextureUsage::COPY_SOURCE,
    );
    let mut encoder = device.create_encoder("quad").expect("encoder");
    record_quad(
        device,
        encoder.as_mut(),
        target,
        pipeline,
        texture,
        sampler,
        scale,
    );
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");
    let pixels = device.read_texture(target).expect("readback");
    device.destroy_texture(target).expect("destroy target");
    pixels
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// The texture's first row of data is at the top, and its left column on the left: no flip in
/// either direction between upload, sampling and the target.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn sampled_textures_appear_upright() {
    let device = device();
    let pipeline = quad_pipeline(&device);
    let texture = quadrants(&device);
    let nearest = sampler(&device, Filter::Nearest, AddressMode::ClampToEdge);
    let pixels = draw(&device, pipeline, texture, nearest, 1.0);
    assert_eq!(pixel(&pixels, 16, 16), RED, "top left");
    assert_eq!(pixel(&pixels, 48, 16), GREEN, "top right");
    assert_eq!(pixel(&pixels, 16, 48), BLUE, "bottom left");
    assert_eq!(pixel(&pixels, 48, 48), WHITE, "bottom right");

    device.destroy_sampler(nearest).expect("destroy sampler");
    device.destroy_texture(texture).expect("destroy texture");
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    assert_no_validation_errors(&device);
}

/// Sampler settings take effect: repeat tiles, and linear filtering blends.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn samplers_repeat_and_filter() {
    let device = device();
    let pipeline = quad_pipeline(&device);
    let texture = quadrants(&device);

    // Scale 2 with Repeat: the 2×2 pattern tiles twice across the target.
    let repeat = sampler(&device, Filter::Nearest, AddressMode::Repeat);
    let tiled = draw(&device, pipeline, texture, repeat, 2.0);
    assert_eq!(pixel(&tiled, 8, 8), RED);
    assert_eq!(pixel(&tiled, 24, 8), GREEN);
    assert_eq!(pixel(&tiled, 40, 8), RED, "the second tile starts again");
    assert_eq!(pixel(&tiled, 56, 56), WHITE);

    // Linear filtering at the centre blends all four texels.
    let linear = sampler(&device, Filter::Linear, AddressMode::ClampToEdge);
    let blended = draw(&device, pipeline, texture, linear, 1.0);
    let [r, g, b, _] = pixel(&blended, 32, 32);
    for (channel, value) in [("red", r), ("green", g), ("blue", b)] {
        assert!(
            (100..=156).contains(&value),
            "{channel} {value} is a blend of all four"
        );
    }

    for sampler in [repeat, linear] {
        device.destroy_sampler(sampler).expect("destroy sampler");
    }
    device.destroy_texture(texture).expect("destroy texture");
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    assert_no_validation_errors(&device);
}

/// A texture rendered to in one pass and sampled in the next, in one encoder: the render pass
/// returns it to its sampleable layout.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn render_to_texture_then_sample_it() {
    let device = device();
    let pipeline = quad_pipeline(&device);
    let canvas = texture(
        &device,
        8,
        TextureUsage::RENDER_TARGET | TextureUsage::SAMPLED,
    );
    let target = texture(
        &device,
        SIZE,
        TextureUsage::RENDER_TARGET | TextureUsage::COPY_SOURCE,
    );
    let nearest = sampler(&device, Filter::Nearest, AddressMode::ClampToEdge);

    let mut encoder = device
        .create_encoder("render then sample")
        .expect("encoder");
    {
        let attachments = [ColorAttachment {
            texture: canvas,
            load: LoadOp::Clear(LinearRgba::new(0.0, 1.0, 0.0, 1.0)),
            store: StoreOp::Store,
        }];
        let _pass = encoder.begin_render_pass(&RenderPassDesc {
            label: "fill the canvas",
            color_attachments: &attachments,
            depth_attachment: None,
        });
    }
    record_quad(
        &device,
        encoder.as_mut(),
        target,
        pipeline,
        canvas,
        nearest,
        1.0,
    );
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");
    let pixels = device.read_texture(target).expect("readback");
    assert_eq!(
        pixel(&pixels, 32, 32),
        GREEN,
        "the second pass sampled the first pass's result"
    );

    device.destroy_sampler(nearest).expect("destroy sampler");
    device.destroy_texture(canvas).expect("destroy canvas");
    device.destroy_texture(target).expect("destroy target");
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    assert_no_validation_errors(&device);
}

/// Uploads take effect in submission order, like buffer writes.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn uploads_take_effect_in_submission_order() {
    let device = device();
    let pipeline = quad_pipeline(&device);
    let texture = texture(
        &device,
        1,
        TextureUsage::SAMPLED | TextureUsage::COPY_DESTINATION,
    );
    let nearest = sampler(&device, Filter::Nearest, AddressMode::ClampToEdge);
    let target = self::texture(
        &device,
        SIZE,
        TextureUsage::RENDER_TARGET | TextureUsage::COPY_SOURCE,
    );

    device.write_texture(texture, &RED).expect("upload red");
    let mut encoder = device
        .create_encoder("recorded before the second upload")
        .expect("encoder");
    record_quad(
        &device,
        encoder.as_mut(),
        target,
        pipeline,
        texture,
        nearest,
        1.0,
    );
    device.write_texture(texture, &BLUE).expect("upload blue");
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");
    let pixels = device.read_texture(target).expect("readback");
    assert_eq!(
        pixel(&pixels, 32, 32),
        BLUE,
        "the draw was submitted after the second upload"
    );

    device.destroy_sampler(nearest).expect("destroy sampler");
    device.destroy_texture(texture).expect("destroy texture");
    device.destroy_texture(target).expect("destroy target");
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    assert_no_validation_errors(&device);
}

/// A destroyed texture's index isn't handed out again while an unsubmitted encoder may sample
/// it, and that encoder still sees the old texture.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn indices_outlive_every_encoder_that_may_use_them() {
    let device = device();
    let pipeline = quad_pipeline(&device);
    let nearest = sampler(&device, Filter::Nearest, AddressMode::ClampToEdge);
    let target = texture(
        &device,
        SIZE,
        TextureUsage::RENDER_TARGET | TextureUsage::COPY_SOURCE,
    );
    let old = quadrants(&device);
    let old_index = device.texture_index(old).expect("index");

    let mut encoder = device
        .create_encoder("samples the old texture")
        .expect("encoder");
    record_quad(
        &device,
        encoder.as_mut(),
        target,
        pipeline,
        old,
        nearest,
        1.0,
    );
    device.destroy_texture(old).expect("destroy");
    let new = texture(
        &device,
        2,
        TextureUsage::SAMPLED | TextureUsage::COPY_DESTINATION,
    );
    assert_ne!(
        device.texture_index(new).expect("index"),
        old_index,
        "the old index is still in use by the unsubmitted encoder"
    );
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");
    let pixels = device.read_texture(target).expect("readback");
    assert_eq!(
        pixel(&pixels, 16, 16),
        RED,
        "the encoder sampled the old texture"
    );

    device.destroy_texture(new).expect("destroy");
    device.destroy_sampler(nearest).expect("destroy sampler");
    device.destroy_texture(target).expect("destroy target");
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    assert_no_validation_errors(&device);
}

#[test]
#[ignore = "requires a Vulkan 1.3 device and validation layers"]
fn misuse_is_refused_with_reasons() {
    let device = device();
    let plain = texture(&device, 4, TextureUsage::RENDER_TARGET);
    assert!(invalid(device.texture_index(plain)).contains("SAMPLED"));
    assert!(invalid(device.write_texture(plain, &[0; 64])).contains("COPY_DESTINATION"));

    let uploadable = texture(
        &device,
        4,
        TextureUsage::SAMPLED | TextureUsage::COPY_DESTINATION,
    );
    assert!(invalid(device.write_texture(uploadable, &[0; 63])).contains("exactly 64"));

    for bad in [0.5, 17.0, f32::NAN] {
        let refused = invalid(device.create_sampler(&SamplerDesc {
            max_anisotropy: Some(bad),
            ..SamplerDesc::default()
        }));
        assert!(refused.contains("between 1 and 16"), "{refused}");
    }
    let anisotropic = device
        .create_sampler(&SamplerDesc {
            max_anisotropy: Some(16.0),
            ..SamplerDesc::default()
        })
        .expect("16x anisotropy is always supported");
    device.destroy_sampler(anisotropic).expect("destroy");
    assert!(invalid(device.destroy_sampler(anisotropic)).contains("does not exist"));
    assert!(invalid(device.sampler_index(anisotropic)).contains("does not exist"));

    device.destroy_texture(plain).expect("destroy");
    device.destroy_texture(uploadable).expect("destroy");
    assert_no_validation_errors(&device);
}
