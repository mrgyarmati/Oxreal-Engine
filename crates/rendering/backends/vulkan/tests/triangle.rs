//! Draws the first triangle offscreen and checks its pixels, and pins down how misuse and
//! concurrency are handled.
//!
//! Validation layers prove that API usage is correct, but not that the picture is. The pixel
//! tests pin down the conventions that go wrong silently (ADR-0011): clip-space +Y is **up**, and
//! counter-clockwise triangles are **front** faces. The others are regression tests for the
//! `unsafe` audit of the first draw call: each would hit undefined behaviour or a validation
//! error without its fix. CI runs them with synchronization validation on.
//!
//! They need a Vulkan 1.3 device, the validation layers and the pinned Slang
//! (`cargo xtask slang install`). They're ignored by default. Run them with
//! `cargo test -p oxreal-rhi-vulkan -- --ignored`. CI runs them on Lavapipe.

use std::path::PathBuf;
use std::sync::OnceLock;

use oxreal_math::LinearRgba;
use oxreal_rhi::{
    ColorAttachment, CommandEncoder, CullMode, Device as _, DeviceDesc, GpuSelection, LoadOp,
    PipelineHandle, RenderPassDesc, RenderPipelineDesc, RhiError, ShaderCode, ShaderEntry, StoreOp,
    SubmissionId, TextureDesc, TextureFormat, TextureHandle, TextureUsage, Validation,
};
use oxreal_rhi_vulkan::VulkanDevice;
use oxreal_shader_compiler::{CompileRequest, Optimization, SlangCompiler, validate_spirv};

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

/// `Triangle.slang`, compiled once per test binary and checked with `spirv-val` when it's
/// installed. CI sets `OXREAL_REQUIRE_SPIRV_VAL=1`, so there the check can't be skipped.
fn triangle_spirv() -> &'static [u32] {
    static SPIRV: OnceLock<Vec<u32>> = OnceLock::new();
    SPIRV.get_or_init(|| {
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/shaders/Triangle.slang");
        let compiler =
            SlangCompiler::locate().expect("pinned Slang: run `cargo xtask slang install`");
        let spirv = compiler
            .compile(&CompileRequest {
                source: &source,
                entry_points: &["triangleVertex", "trianglePixel"],
                include_dirs: &[],
                optimization: Optimization::Debug,
            })
            .expect("Triangle.slang compiles")
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

/// The triangle's shader code.
fn triangle_code() -> ShaderCode<'static> {
    // SAFETY: produced by the pinned `slangc` through oxreal-shader-compiler, and checked with
    // `spirv-val --target-env vulkan1.3` (mandatory in CI) in `triangle_spirv`.
    unsafe { ShaderCode::from_spirv_unchecked(triangle_spirv()) }
}

fn pipeline_desc<'a>(
    label: &'a str,
    formats: &'a [TextureFormat],
    cull: CullMode,
) -> RenderPipelineDesc<'a> {
    RenderPipelineDesc {
        label,
        vertex: ShaderEntry {
            code: triangle_code(),
            entry_point: "triangleVertex",
        },
        pixel: ShaderEntry {
            code: triangle_code(),
            entry_point: "trianglePixel",
        },
        color_formats: formats,
        push_constant_bytes: 8,
        cull,
    }
}

fn pipeline(device: &VulkanDevice, cull: CullMode) -> PipelineHandle {
    device
        .create_render_pipeline(&pipeline_desc("triangle", &[FORMAT], cull))
        .expect("pipeline")
}

/// Push constants of `Triangle.slang`: rotation (radians) and aspect ratio.
fn constants(rotation: f32, aspect: f32) -> [u8; 8] {
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&rotation.to_le_bytes());
    bytes[4..].copy_from_slice(&aspect.to_le_bytes());
    bytes
}

fn target_with_format(device: &VulkanDevice, format: TextureFormat) -> TextureHandle {
    device
        .create_texture(&TextureDesc {
            label: "triangle target",
            width: SIZE,
            height: SIZE,
            format,
            usage: TextureUsage::RENDER_TARGET | TextureUsage::COPY_SOURCE,
        })
        .expect("texture")
}

fn target(device: &VulkanDevice) -> TextureHandle {
    target_with_format(device, FORMAT)
}

fn black() -> LoadOp {
    LoadOp::Clear(LinearRgba::new(0.0, 0.0, 0.0, 1.0))
}

/// Records a pass on `texture` that draws the triangle with `pipeline`.
fn record_triangle(
    encoder: &mut dyn CommandEncoder,
    texture: TextureHandle,
    pipeline: PipelineHandle,
) {
    let attachments = [ColorAttachment {
        texture,
        load: black(),
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

fn submit_and_wait(device: &VulkanDevice, encoder: Box<dyn CommandEncoder>) {
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");
}

fn misuse(result: Result<SubmissionId, RhiError>) -> String {
    match result {
        Err(RhiError::InvalidUsage { details }) => details,
        other => panic!("expected InvalidUsage, got {other:?}"),
    }
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
    let pipeline = pipeline(device, cull);
    let target = target(device);
    let mut encoder = device.create_encoder("triangle test").expect("encoder");
    record_triangle(encoder.as_mut(), target, pipeline);
    submit_and_wait(device, encoder);
    let pixels = device.read_texture(target).expect("readback");
    assert_eq!(pixels.len(), (SIZE * SIZE * 4) as usize);
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_texture(target).expect("destroy texture");
    Image(pixels)
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
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
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
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn counter_clockwise_triangles_are_front_faces() {
    let device = device();
    // The triangle winds counter-clockwise, so culling front faces removes it entirely, and
    // culling back faces (previous test) keeps it.
    let image = render(&device, CullMode::Front);
    assert_eq!(image.lit_pixels(), 0, "a front face was not culled");
    assert_no_validation_errors(&device);
}

#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn objects_destroyed_while_recording_outlive_the_submission() {
    let device = device();
    let pipeline = pipeline(&device, CullMode::None);
    let target = target(&device);
    let mut encoder = device.create_encoder("deferred").expect("encoder");
    record_triangle(encoder.as_mut(), target, pipeline);
    // Destroyed while the encoder that uses them hasn't been submitted. Destruction must wait
    // for that submission. Validation would report destroying objects still in use.
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_texture(target).expect("destroy texture");
    submit_and_wait(&device, encoder);

    // The handles are invalid immediately.
    assert!(matches!(
        device.destroy_texture(target),
        Err(RhiError::InvalidUsage { .. })
    ));
    assert_no_validation_errors(&device);
}

#[test]
#[ignore = "requires a Vulkan 1.3 device and validation layers"]
fn misuse_is_reported_at_submit_without_validation_errors() {
    let device = device();
    let target = target(&device);
    let mut encoder = device.create_encoder("misuse").expect("encoder");
    {
        let attachments = [ColorAttachment {
            texture: target,
            load: black(),
            store: StoreOp::Store,
        }];
        let mut pass = encoder.begin_render_pass(&RenderPassDesc {
            label: "misuse",
            color_attachments: &attachments,
        });
        pass.draw(3, 1); // No pipeline bound.
    }
    let details = misuse(device.submit(encoder));
    assert!(
        details.contains("draw before a successful set_pipeline"),
        "{details}"
    );
    // Abandoned encoders don't leak or break anything either.
    drop(device.create_encoder("abandoned").expect("encoder"));
    device.destroy_texture(target).expect("destroy texture");
    assert_no_validation_errors(&device);
}

/// Audit S5: `mem::forget` on a pass guard is safe Rust, so a pass can be left open.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn a_leaked_render_pass_is_closed_at_submission() {
    let device = device();
    let pipeline = pipeline(&device, CullMode::None);
    let target = target(&device);
    let mut encoder = device.create_encoder("leaked pass").expect("encoder");
    let attachments = [ColorAttachment {
        texture: target,
        load: black(),
        store: StoreOp::Store,
    }];
    let mut pass = encoder.begin_render_pass(&RenderPassDesc {
        label: "never ended",
        color_attachments: &attachments,
    });
    pass.set_pipeline(pipeline);
    pass.set_push_constants(&constants(0.0, 1.0));
    pass.draw(3, 1);
    #[expect(
        clippy::mem_forget,
        reason = "the point of the test: safe code can leak a pass guard"
    )]
    std::mem::forget(pass);
    let details = misuse(device.submit(encoder));
    assert!(
        details.contains("\"never ended\" was never ended"),
        "{details}"
    );
    // The commands still ran: the pass was closed properly, so the triangle is there.
    let image = Image(device.read_texture(target).expect("readback"));
    assert!(!image.is_background(32, 32));
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_texture(target).expect("destroy texture");
    assert_no_validation_errors(&device);
}

/// Audit C1: a texture's layout is tracked in submission order, so two unsubmitted encoders
/// can't both use it, and a dropped encoder restores what it changed.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn textures_in_use_by_an_unsubmitted_encoder_are_refused() {
    let device = device();
    let pipeline = pipeline(&device, CullMode::None);
    let target = target(&device);

    let mut first = device.create_encoder("first").expect("encoder");
    record_triangle(first.as_mut(), target, pipeline);

    let mut second = device.create_encoder("second").expect("encoder");
    record_triangle(second.as_mut(), target, pipeline);
    let details = misuse(device.submit(second));
    assert!(details.contains("another unsubmitted encoder"), "{details}");
    match device.read_texture(target) {
        Err(RhiError::InvalidUsage { details }) => {
            assert!(details.contains("hasn't been submitted"), "{details}");
        }
        other => panic!("expected InvalidUsage, got {other:?}"),
    }

    submit_and_wait(&device, first);
    let image = Image(
        device
            .read_texture(target)
            .expect("readback after submission"),
    );
    assert!(!image.is_background(32, 32));

    // A dropped encoder's layout changes never happen on the GPU, so they're undone. The
    // readback after it starts from the layout the GPU really has (validation checks this).
    let mut dropped = device.create_encoder("dropped").expect("encoder");
    record_triangle(dropped.as_mut(), target, pipeline);
    drop(dropped);
    device
        .read_texture(target)
        .expect("readback after a dropped encoder");

    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_texture(target).expect("destroy texture");
    assert_no_validation_errors(&device);
}

/// Audit C2: pipelines must match the pass, and push constants must be written before a draw.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn draws_check_pipeline_formats_and_push_constants() {
    let device = device();
    let pipeline = pipeline(&device, CullMode::None);

    // A pipeline for RGBA drawn into a BGRA target.
    let bgra = target_with_format(&device, TextureFormat::Bgra8Unorm);
    let mut encoder = device.create_encoder("format mismatch").expect("encoder");
    record_triangle(encoder.as_mut(), bgra, pipeline);
    let details = misuse(device.submit(encoder));
    assert!(details.contains("renders to [Rgba8Unorm]"), "{details}");

    // The pipeline reads 8 push-constant bytes; nothing was written.
    let rgba = target(&device);
    let mut encoder = device.create_encoder("no push constants").expect("encoder");
    {
        let attachments = [ColorAttachment {
            texture: rgba,
            load: black(),
            store: StoreOp::Store,
        }];
        let mut pass = encoder.begin_render_pass(&RenderPassDesc {
            label: "no push constants",
            color_attachments: &attachments,
        });
        pass.set_pipeline(pipeline);
        pass.draw(3, 1);
    }
    let details = misuse(device.submit(encoder));
    assert!(details.contains("only 0 were written"), "{details}");

    // A pass naming the same texture twice records nothing.
    let mut encoder = device.create_encoder("duplicate").expect("encoder");
    {
        let attachment = ColorAttachment {
            texture: rgba,
            load: black(),
            store: StoreOp::Store,
        };
        let _pass = encoder.begin_render_pass(&RenderPassDesc {
            label: "duplicate",
            color_attachments: &[attachment, attachment],
        });
    }
    let details = misuse(device.submit(encoder));
    assert!(details.contains("twice"), "{details}");

    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_texture(bgra).expect("destroy texture");
    device.destroy_texture(rgba).expect("destroy texture");
    assert_no_validation_errors(&device);
}

/// Audit S6: shader code is checked before it reaches the driver.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn shader_code_is_checked_before_reaching_the_driver() {
    let device = device();
    let rejected = |desc: RenderPipelineDesc<'_>| match device.create_render_pipeline(&desc) {
        Err(RhiError::InvalidUsage { details }) => details,
        other => panic!("expected InvalidUsage, got {other:?}"),
    };

    let not_spirv = [0xdead_beef_u32, 0, 0, 1, 0];
    // SAFETY: not valid SPIR-V, on purpose. The backend must reject it before the driver sees
    // it, and this test checks that it does.
    let bad = unsafe { ShaderCode::from_spirv_unchecked(&not_spirv) };
    let mut desc = pipeline_desc("bad header", &[FORMAT], CullMode::None);
    desc.vertex.code = bad;
    assert!(rejected(desc).contains("not SPIR-V"));

    let mut desc = pipeline_desc("wrong entry", &[FORMAT], CullMode::None);
    desc.pixel.entry_point = "noSuchEntryPoint";
    let details = rejected(desc);
    assert!(
        details.contains("no Fragment entry point \"noSuchEntryPoint\""),
        "{details}"
    );

    let mut desc = pipeline_desc("wrong stage", &[FORMAT], CullMode::None);
    desc.vertex.entry_point = "trianglePixel"; // A pixel shader as the vertex stage.
    assert!(rejected(desc).contains("no Vertex entry point"));

    let mut desc = pipeline_desc("too few push bytes", &[FORMAT], CullMode::None);
    desc.push_constant_bytes = 4;
    let details = rejected(desc);
    assert!(
        details.contains("read 8 push-constant bytes, but it declares 4"),
        "{details}"
    );

    let mut desc = pipeline_desc("too many push bytes", &[FORMAT], CullMode::None);
    desc.push_constant_bytes = 132;
    assert!(rejected(desc).contains("at most 128"));

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
    let huge = device.create_texture(&TextureDesc {
        label: "huge",
        width: u32::MAX,
        height: 4,
        format: FORMAT,
        usage: TextureUsage::RENDER_TARGET,
    });
    assert!(
        matches!(&huge, Err(RhiError::InvalidUsage { details }) if details.contains("limit")),
        "{huge:?}"
    );
    assert_no_validation_errors(&device);
}

/// Audit R3: waiting for a submission that never happened would wait forever.
#[test]
#[ignore = "requires a Vulkan 1.3 device and validation layers"]
fn waiting_for_an_unknown_submission_is_an_error() {
    let device = device();
    match device.wait_for_submission(SubmissionId(u64::MAX)) {
        Err(RhiError::InvalidUsage { details }) => {
            assert!(details.contains("never submitted"), "{details}");
        }
        other => panic!("expected InvalidUsage, got {other:?}"),
    }
    assert_no_validation_errors(&device);
}

/// Audit S3: an encoder may outlive its device. It keeps the device's internals alive, so
/// recording into it and dropping it afterwards is sound.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn an_encoder_may_outlive_its_device() {
    let device = device();
    let pipeline = pipeline(&device, CullMode::None);
    let target = target(&device);
    let mut encoder = device
        .create_encoder("outlives the device")
        .expect("encoder");
    record_triangle(encoder.as_mut(), target, pipeline);
    drop(device);
    // Still recording into a live command buffer of a live (shared) VkDevice.
    record_triangle(encoder.as_mut(), target, pipeline);
    drop(encoder); // The last reference: the device is destroyed here.
}

/// Audit S1/S2: destroying a resource while another thread submits or reads it back. Each
/// iteration races the two; destruction must always wait for the GPU.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn destroying_while_another_thread_submits_is_safe() {
    let device = device();
    let pipeline = pipeline(&device, CullMode::None);
    for _ in 0..40 {
        let texture = target(&device);
        let mut encoder = device.create_encoder("race").expect("encoder");
        record_triangle(encoder.as_mut(), texture, pipeline);
        std::thread::scope(|scope| {
            scope.spawn(|| submit_and_wait(&device, encoder));
            scope.spawn(|| device.destroy_texture(texture).expect("destroy"));
        });

        let texture = target(&device);
        let mut encoder = device.create_encoder("race").expect("encoder");
        record_triangle(encoder.as_mut(), texture, pipeline);
        submit_and_wait(&device, encoder);
        std::thread::scope(|scope| {
            // Either the readback wins and succeeds, or the texture is already gone.
            scope.spawn(|| match device.read_texture(texture) {
                Ok(pixels) => assert_eq!(pixels.len(), (SIZE * SIZE * 4) as usize),
                Err(RhiError::InvalidUsage { details }) => {
                    assert!(details.contains("does not exist"), "{details}");
                }
                Err(other) => panic!("unexpected error: {other}"),
            });
            scope.spawn(|| device.destroy_texture(texture).expect("destroy"));
        });
    }
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    assert_no_validation_errors(&device);
}

/// A second pass with `LoadOp::Load` keeps what the first pass drew. Inside one command buffer,
/// this depends on the barrier between the passes (synchronization validation checks it in CI).
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn a_loading_pass_keeps_the_previous_passes_pixels() {
    let device = device();
    let pipeline = pipeline(&device, CullMode::None);
    let target = target(&device);
    let mut encoder = device.create_encoder("two passes").expect("encoder");
    record_triangle(encoder.as_mut(), target, pipeline);
    {
        let attachments = [ColorAttachment {
            texture: target,
            load: LoadOp::Load,
            store: StoreOp::Store,
        }];
        let mut pass = encoder.begin_render_pass(&RenderPassDesc {
            label: "second pass",
            color_attachments: &attachments,
        });
        // Draws the triangle again, rotated half a turn: the two overlap in the middle.
        pass.set_pipeline(pipeline);
        pass.set_push_constants(&constants(std::f32::consts::PI, 1.0));
        pass.draw(3, 1);
    }
    submit_and_wait(&device, encoder);
    let image = Image(device.read_texture(target).expect("readback"));
    assert!(
        !image.is_background(32, 20),
        "the first pass's apex survived"
    );
    assert!(
        !image.is_background(32, 44),
        "the second pass's apex is there too"
    );
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    device.destroy_texture(target).expect("destroy texture");
    assert_no_validation_errors(&device);
}
