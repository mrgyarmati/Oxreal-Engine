//! Draws the first triangle offscreen and checks its pixels.
//!
//! Validation layers prove that API usage is correct, but not that the picture is. These tests
//! pin down the conventions that go wrong silently (ADR-0011): clip-space +Y is **up**, and
//! counter-clockwise triangles are **front** faces.
//!
//! They need a Vulkan 1.3 device, the validation layers and the pinned Slang
//! (`cargo xtask slang install`). They're ignored by default. Run them with
//! `cargo test -p oxreal-rhi-vulkan -- --ignored`. CI runs them on Lavapipe.

use std::path::PathBuf;

use oxreal_math::LinearRgba;
use oxreal_rhi::{
    ColorAttachment, CommandEncoder, CullMode, Device as _, DeviceDesc, GpuSelection, LoadOp,
    RenderPassDesc, RenderPipelineDesc, RhiError, ShaderCode, ShaderEntry, StoreOp, TextureDesc,
    TextureFormat, TextureHandle, TextureUsage, Validation,
};
use oxreal_rhi_vulkan::VulkanDevice;
use oxreal_shader_compiler::{CompileRequest, Optimization, SlangCompiler};

const SIZE: u32 = 64;
const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

fn device() -> VulkanDevice {
    let device = oxreal_rhi_vulkan::create_device(&DeviceDesc {
        app_name: "oxreal triangle tests".to_owned(),
        validation: Validation::Enabled,
        gpu: GpuSelection::Automatic,
    })
    .expect("device creation");
    assert!(
        device.validation_enabled(),
        "validation layers must be installed for this test (Vulkan SDK or vulkan-validationlayers)"
    );
    device
}

fn triangle_spirv() -> Vec<u32> {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/shaders/Triangle.slang");
    let compiler = SlangCompiler::locate().expect("pinned Slang: run `cargo xtask slang install`");
    compiler
        .compile(&CompileRequest {
            source: &source,
            entry_points: &["triangleVertex", "trianglePixel"],
            include_dirs: &[],
            optimization: Optimization::Debug,
        })
        .expect("Triangle.slang compiles")
        .spirv
}

/// Push constants of `Triangle.slang`: rotation (radians) and aspect ratio.
fn constants(rotation: f32, aspect: f32) -> [u8; 8] {
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&rotation.to_le_bytes());
    bytes[4..].copy_from_slice(&aspect.to_le_bytes());
    bytes
}

/// A 64×64 render of the triangle, read back as RGBA rows, top row first.
struct Image(Vec<u8>);

impl Image {
    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * SIZE + x) * 4) as usize;
        [self.0[i], self.0[i + 1], self.0[i + 2], self.0[i + 3]]
    }

    fn is_background(&self, x: u32, y: u32) -> bool {
        let [r, g, b, _] = self.pixel(x, y);
        r == 0 && g == 0 && b == 0
    }

    /// The strongest channel at (x, y): 0 red, 1 green, 2 blue.
    fn dominant(&self, x: u32, y: u32) -> usize {
        let [r, g, b, _] = self.pixel(x, y);
        let channels = [r, g, b];
        (0..3).max_by_key(|&c| channels[c]).unwrap_or(0)
    }

    fn lit_pixels(&self) -> usize {
        (0..SIZE)
            .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
            .filter(|&(x, y)| !self.is_background(x, y))
            .count()
    }
}

fn render(device: &VulkanDevice, cull: CullMode) -> Image {
    let spirv = triangle_spirv();
    let pipeline = device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "triangle",
            vertex: ShaderEntry {
                code: ShaderCode::Spirv(&spirv),
                entry_point: "triangleVertex",
            },
            pixel: ShaderEntry {
                code: ShaderCode::Spirv(&spirv),
                entry_point: "trianglePixel",
            },
            color_formats: &[FORMAT],
            push_constant_bytes: 8,
            cull,
        })
        .expect("pipeline");
    let target = target(device);

    let mut encoder = device.create_encoder("triangle test").expect("encoder");
    {
        let attachments = [ColorAttachment {
            texture: target,
            load: LoadOp::Clear(LinearRgba::new(0.0, 0.0, 0.0, 1.0)),
            store: StoreOp::Store,
        }];
        let mut pass = encoder.begin_render_pass(&RenderPassDesc {
            label: "triangle",
            color_attachments: &attachments,
        });
        pass.set_pipeline(pipeline);
        pass.set_push_constants(&constants(0.0, 1.0));
        pass.draw(3, 1);
    }
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");
    let pixels = device.read_texture(target).expect("readback");
    assert_eq!(pixels.len(), (SIZE * SIZE * 4) as usize);

    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_texture(target).expect("destroy texture");
    Image(pixels)
}

fn target(device: &VulkanDevice) -> TextureHandle {
    device
        .create_texture(&TextureDesc {
            label: "triangle target",
            width: SIZE,
            height: SIZE,
            format: FORMAT,
            usage: TextureUsage::RENDER_TARGET | TextureUsage::COPY_SOURCE,
        })
        .expect("texture")
}

fn assert_no_validation_errors(device: &VulkanDevice) {
    device.wait_idle().expect("wait idle");
    assert_eq!(
        device.validation_error_count(),
        0,
        "validation reported errors (see log)"
    );
}

// Pixel positions below are derived from the corners in Triangle.slang: apex (0, 0.5),
// bottom-left (-0.5, -0.5), bottom-right (0.5, -0.5). On a 64-pixel target, clip-space y = 0.5
// is pixel row 16 from the top, y = -0.5 is row 48, and x = ±0.5 are columns 16 and 48.

#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang; run with --ignored"]
fn triangle_points_up_and_has_its_corner_colors() {
    let device = device();
    let image = render(&device, CullMode::Back);

    // Clip-space +Y is up: the apex is near the top, the base near the bottom.
    assert!(
        !image.is_background(32, 20),
        "just below the apex is inside"
    );
    assert!(image.is_background(32, 10), "above the apex is background");
    assert!(
        !image.is_background(32, 44),
        "just above the base is inside"
    );
    assert!(image.is_background(32, 54), "below the base is background");
    assert!(
        image.is_background(4, 44),
        "left of the triangle is background"
    );
    assert!(
        image.is_background(60, 44),
        "right of the triangle is background"
    );

    // Each corner keeps its color, so the triangle isn't mirrored either way.
    assert_eq!(image.dominant(32, 20), 0, "red apex at the top");
    assert_eq!(image.dominant(20, 46), 1, "green corner bottom-left");
    assert_eq!(image.dominant(43, 46), 2, "blue corner bottom-right");

    // Roughly half of a 32×32 bounding box. Catches a missing or doubled draw.
    let lit = image.lit_pixels();
    assert!((400..=624).contains(&lit), "{lit} lit pixels");

    assert_no_validation_errors(&device);
}

#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang; run with --ignored"]
fn counter_clockwise_triangles_are_front_faces() {
    let device = device();
    // The triangle winds counter-clockwise, so culling front faces removes it entirely, and
    // culling back faces (previous test) keeps it.
    let image = render(&device, CullMode::Front);
    assert_eq!(image.lit_pixels(), 0, "a front face was not culled");
    assert_no_validation_errors(&device);
}

#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang; run with --ignored"]
fn objects_destroyed_while_recording_outlive_the_submission() {
    let device = device();
    let spirv = triangle_spirv();
    let pipeline = device
        .create_render_pipeline(&RenderPipelineDesc {
            label: "short-lived",
            vertex: ShaderEntry {
                code: ShaderCode::Spirv(&spirv),
                entry_point: "triangleVertex",
            },
            pixel: ShaderEntry {
                code: ShaderCode::Spirv(&spirv),
                entry_point: "trianglePixel",
            },
            color_formats: &[FORMAT],
            push_constant_bytes: 8,
            cull: CullMode::None,
        })
        .expect("pipeline");
    let target = target(&device);

    let mut encoder: Box<dyn CommandEncoder> = device.create_encoder("deferred").expect("encoder");
    {
        let attachments = [ColorAttachment {
            texture: target,
            load: LoadOp::Clear(LinearRgba::new(0.0, 0.0, 0.0, 1.0)),
            store: StoreOp::Store,
        }];
        let mut pass = encoder.begin_render_pass(&RenderPassDesc {
            label: "deferred",
            color_attachments: &attachments,
        });
        pass.set_pipeline(pipeline);
        pass.set_push_constants(&constants(0.0, 1.0));
        pass.draw(3, 1);
    }
    // Destroyed while the encoder that uses them hasn't been submitted. Destruction must wait
    // for that submission. Validation would report destroying objects still in use.
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_texture(target).expect("destroy texture");
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");

    // The handles are invalid immediately.
    assert!(matches!(
        device.destroy_texture(target),
        Err(RhiError::InvalidUsage { .. })
    ));
    assert_no_validation_errors(&device);
}

#[test]
#[ignore = "requires a Vulkan 1.3 device and validation layers; run with --ignored"]
fn misuse_is_reported_at_submit_without_validation_errors() {
    let device = device();
    let target = target(&device);
    let mut encoder = device.create_encoder("misuse").expect("encoder");
    {
        let attachments = [ColorAttachment {
            texture: target,
            load: LoadOp::Clear(LinearRgba::new(0.0, 0.0, 0.0, 1.0)),
            store: StoreOp::Store,
        }];
        let mut pass = encoder.begin_render_pass(&RenderPassDesc {
            label: "misuse",
            color_attachments: &attachments,
        });
        pass.draw(3, 1); // No pipeline bound.
    }
    match device.submit(encoder) {
        Err(RhiError::InvalidUsage { details }) => {
            assert!(details.contains("draw before set_pipeline"), "{details}");
        }
        other => panic!("expected InvalidUsage, got {other:?}"),
    }
    // Abandoned encoders don't leak or break anything either.
    drop(device.create_encoder("abandoned").expect("encoder"));
    device.destroy_texture(target).expect("destroy texture");
    assert_no_validation_errors(&device);
}

#[test]
#[ignore = "requires a Vulkan 1.3 device and validation layers; run with --ignored"]
fn invalid_descriptions_are_rejected_with_reasons() {
    let device = device();
    let zero = device.create_texture(&TextureDesc {
        label: "empty",
        width: 0,
        height: 4,
        format: FORMAT,
        usage: TextureUsage::RENDER_TARGET,
    });
    assert!(
        matches!(&zero, Err(RhiError::InvalidUsage { details }) if details.contains("zero size")),
        "{zero:?}"
    );
    let words = [0x0723_0203_u32];
    let too_many = device.create_render_pipeline(&RenderPipelineDesc {
        label: "too many push constants",
        vertex: ShaderEntry {
            code: ShaderCode::Spirv(&words),
            entry_point: "main",
        },
        pixel: ShaderEntry {
            code: ShaderCode::Spirv(&words),
            entry_point: "main",
        },
        color_formats: &[FORMAT],
        push_constant_bytes: 132,
        cull: CullMode::None,
    });
    assert!(
        matches!(&too_many, Err(RhiError::InvalidUsage { details }) if details.contains("at most 128")),
        "{too_many:?}"
    );
    assert_no_validation_errors(&device);
}
