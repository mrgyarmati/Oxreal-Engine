//! Depth buffers and a 3D camera (ADR-0011 §4): reversed depth in `Depth32Float`, cleared to 0
//! and compared with *greater or equal*, with projections from `oxreal_math`.
//!
//! The pixel tests draw textured cubes (`examples/shaders/Cube.slang`) through a real camera.
//! Each cube samples a one-texel texture, so a pixel's dominant channel says which cube is
//! visible there. That checks the whole path: matrices in push constants, projection, depth
//! clear, depth test, and attachment layouts within and across submissions.
//!
//! They need a Vulkan 1.3 device, the validation layers and the pinned Slang
//! (`cargo xtask slang install`). They're ignored by default. Run them with
//! `cargo test -p oxreal-rhi-vulkan -- --ignored`. CI runs them on Lavapipe.

use std::path::PathBuf;
use std::sync::OnceLock;

use oxreal_math::projection::{REVERSED_DEPTH_CLEAR, perspective_infinite_reversed, view_matrix};
use oxreal_math::{LinearRgba, Mat4, Transform, Vec3};
use oxreal_rhi::{
    AddressMode, ColorAttachment, CommandEncoder, CompareOp, CullMode, DepthAttachment,
    DepthLoadOp, DepthState, Device as _, DeviceDesc, Filter, GpuSelection, LoadOp, PipelineHandle,
    RenderPassDesc, RenderPipelineDesc, RhiError, SamplerDesc, SamplerHandle, ShaderCode,
    ShaderEntry, StoreOp, SubmissionId, TextureDesc, TextureFormat, TextureHandle, TextureUsage,
    Validation,
};
use oxreal_rhi_vulkan::VulkanDevice;
use oxreal_shader_compiler::{CompileRequest, Optimization, SlangCompiler, validate_spirv};

const SIZE: u32 = 64;
const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
/// CubeConstants: a 4×4 matrix, then the texture and sampler indices.
const CONSTANT_BYTES: u32 = 72;

fn device() -> VulkanDevice {
    let _ = oxreal_diagnostics::init_application_logging();
    let device = oxreal_rhi_vulkan::create_device(&DeviceDesc {
        app_name: "oxreal depth tests".to_owned(),
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

fn misuse(result: Result<SubmissionId, RhiError>) -> String {
    invalid(result)
}

/// `Cube.slang`, compiled once per test binary and checked with `spirv-val` (mandatory in CI).
fn cube_spirv() -> &'static [u32] {
    static SPIRV: OnceLock<Vec<u32>> = OnceLock::new();
    SPIRV.get_or_init(|| {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source = crate_dir.join("examples/shaders/Cube.slang");
        let bindless = crate_dir.join("../../rhi/shaders");
        let compiler =
            SlangCompiler::locate().expect("pinned Slang: run `cargo xtask slang install`");
        let spirv = compiler
            .compile(&CompileRequest {
                source: &source,
                entry_points: &["cubeVertex", "cubePixel", "cubeDepthOnly"],
                include_dirs: &[bindless],
                optimization: Optimization::Debug,
            })
            .expect("Cube.slang compiles")
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

fn pipeline_desc<'a>(
    code: ShaderCode<'a>,
    color_formats: &'a [TextureFormat],
    depth: Option<DepthState>,
) -> RenderPipelineDesc<'a> {
    RenderPipelineDesc {
        label: "cube",
        vertex: ShaderEntry {
            code,
            entry_point: "cubeVertex",
        },
        pixel: ShaderEntry {
            code,
            // Depth-only pipelines write no color.
            entry_point: if color_formats.is_empty() {
                "cubeDepthOnly"
            } else {
                "cubePixel"
            },
        },
        color_formats,
        depth,
        push_constant_bytes: CONSTANT_BYTES,
        cull: CullMode::Back,
    }
}

fn cube_pipeline(
    device: &VulkanDevice,
    color_formats: &[TextureFormat],
    depth: Option<DepthState>,
) -> PipelineHandle {
    // SAFETY: produced by the pinned `slangc` and checked with spirv-val (mandatory in CI). The
    // tests only pass it indices of live textures and samplers, and never sample a texture
    // during a pass that renders into it.
    let code = unsafe { ShaderCode::from_spirv_unchecked(cube_spirv()) };
    device
        .create_render_pipeline(&pipeline_desc(code, color_formats, depth))
        .expect("pipeline")
}

/// A one-texel texture of `rgba`, sampled through its index.
fn solid(device: &VulkanDevice, rgba: [u8; 4]) -> TextureHandle {
    let texture = device
        .create_texture(&TextureDesc {
            label: "solid",
            width: 1,
            height: 1,
            format: FORMAT,
            usage: TextureUsage::SAMPLED | TextureUsage::COPY_DESTINATION,
        })
        .expect("texture");
    device.write_texture(texture, &rgba).expect("upload");
    texture
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
        .expect("target")
}

fn depth_buffer(device: &VulkanDevice) -> TextureHandle {
    device
        .create_texture(&TextureDesc {
            label: "depth",
            width: SIZE,
            height: SIZE,
            format: TextureFormat::Depth32Float,
            usage: TextureUsage::DEPTH_TARGET,
        })
        .expect("depth buffer")
}

/// What the cubes share: a sampler, and a camera's projection and view.
struct Scene {
    sampler: SamplerHandle,
    sampler_index: u32,
    clip_from_world: Mat4,
}

impl Scene {
    /// A 60° camera with a square aspect, placed by `camera`.
    fn new(device: &VulkanDevice, camera: &Transform) -> Self {
        let sampler = device
            .create_sampler(&SamplerDesc {
                label: "nearest",
                mag_filter: Filter::Nearest,
                min_filter: Filter::Nearest,
                address_mode: AddressMode::ClampToEdge,
                max_anisotropy: None,
            })
            .expect("sampler");
        let projection =
            perspective_infinite_reversed(60_f32.to_radians(), 1.0, 0.1).expect("projection");
        Self {
            sampler,
            sampler_index: device.sampler_index(sampler).expect("sampler index"),
            clip_from_world: projection * view_matrix(camera),
        }
    }

    /// Push constants that draw a cube of `size` at `position`, sampling `texture`.
    fn cube(
        &self,
        device: &VulkanDevice,
        texture: TextureHandle,
        position: Vec3,
        size: f32,
    ) -> [u8; CONSTANT_BYTES as usize] {
        let model = Transform::from_translation(position)
            .with_scale(Vec3::splat(size))
            .to_matrix();
        let mut bytes = [0; CONSTANT_BYTES as usize];
        let (words, _) = bytes.as_chunks_mut::<4>();
        for (chunk, value) in words
            .iter_mut()
            .zip((self.clip_from_world * model).to_cols_array())
        {
            *chunk = value.to_le_bytes();
        }
        let index = device.texture_index(texture).expect("texture index");
        bytes[64..68].copy_from_slice(&index.to_le_bytes());
        bytes[68..72].copy_from_slice(&self.sampler_index.to_le_bytes());
        bytes
    }

    fn destroy(self, device: &VulkanDevice) {
        device
            .destroy_sampler(self.sampler)
            .expect("destroy sampler");
    }
}

fn black() -> LoadOp {
    LoadOp::Clear(LinearRgba::new(0.0, 0.0, 0.0, 1.0))
}

fn cleared_depth(texture: TextureHandle, store: StoreOp) -> DepthAttachment {
    DepthAttachment {
        texture,
        load: DepthLoadOp::Clear(REVERSED_DEPTH_CLEAR),
        store,
    }
}

/// Records one pass on `target` and `depth` that draws each cube in order.
fn record_cubes(
    encoder: &mut dyn CommandEncoder,
    target: TextureHandle,
    depth: DepthAttachment,
    pipeline: PipelineHandle,
    cubes: &[[u8; CONSTANT_BYTES as usize]],
) {
    let attachments = [ColorAttachment {
        texture: target,
        load: black(),
        store: StoreOp::Store,
    }];
    let mut pass = encoder.begin_render_pass(&RenderPassDesc {
        label: "cubes",
        color_attachments: &attachments,
        depth_attachment: Some(depth),
    });
    pass.set_pipeline(pipeline);
    for cube in cubes {
        pass.set_push_constants(cube);
        pass.draw(36, 1);
    }
}

fn submit_and_read(
    device: &VulkanDevice,
    encoder: Box<dyn CommandEncoder>,
    target: TextureHandle,
) -> Vec<u8> {
    let submission = device.submit(encoder).expect("submit");
    device.wait_for_submission(submission).expect("wait");
    device.read_texture(target).expect("readback")
}

/// What's visible at a pixel, by its dominant channel.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    Red,
    Green,
    Background,
}

fn seen(pixels: &[u8], x: u32, y: u32) -> Seen {
    let i = ((y * SIZE + x) * 4) as usize;
    match (pixels[i], pixels[i + 1], pixels[i + 2]) {
        (r, g, _) if r > 100 && g < 20 => Seen::Red,
        (r, g, _) if g > 100 && r < 20 => Seen::Green,
        (0, 0, 0) => Seen::Background,
        other => panic!("unexpected color {other:?} at ({x}, {y})"),
    }
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

/// A small red cube in front of a large green one. Whatever the draw order, the nearer red cube
/// covers the middle, and the green cube shows around it.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn the_nearest_surface_wins_in_either_draw_order() {
    let device = device();
    let pipeline = cube_pipeline(&device, &[FORMAT], Some(DepthState::SCENE));
    let (red, green) = (solid(&device, RED), solid(&device, GREEN));
    let target = target(&device);
    let depth = depth_buffer(&device);
    let scene = Scene::new(&device, &Transform::IDENTITY);
    let near = scene.cube(&device, red, Vec3::new(0.0, 0.0, -3.0), 1.0);
    let far = scene.cube(&device, green, Vec3::new(0.0, 0.0, -6.0), 3.0);

    for (order, cubes) in [("near first", [near, far]), ("far first", [far, near])] {
        let mut encoder = device.create_encoder(order).expect("encoder");
        record_cubes(
            encoder.as_mut(),
            target,
            cleared_depth(depth, StoreOp::DontCare),
            pipeline,
            &cubes,
        );
        let pixels = submit_and_read(&device, encoder, target);
        assert_eq!(seen(&pixels, 32, 32), Seen::Red, "{order}: middle");
        // The camera faces the +Z side of the cube, whose shade in Cube.slang is
        // 0.6 + 0.4 · dot(+Z, normalize(0.3, 0.8, 0.5)) ≈ 0.802. The −Z side, which shows if the
        // winding were inside out and culling kept the far faces, is 0.6.
        let red_level = pixels[((32 * SIZE + 32) * 4) as usize];
        assert!(
            (200..=209).contains(&red_level),
            "{order}: the near face shows (red {red_level}, expected about 204)"
        );
        assert_eq!(seen(&pixels, 17, 32), Seen::Green, "{order}: beside");
        assert_eq!(seen(&pixels, 2, 2), Seen::Background, "{order}: corner");
    }

    scene.destroy(&device);
    for texture in [red, green, target, depth] {
        device.destroy_texture(texture).expect("destroy texture");
    }
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    assert_no_validation_errors(&device);
}

/// The camera's view and projection put a cube where the conventions say: up and to the right
/// of the view centre stays up and to the right (no flip of either axis, no transposed matrix).
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn cubes_appear_where_the_camera_projects_them() {
    let device = device();
    let pipeline = cube_pipeline(&device, &[FORMAT], Some(DepthState::SCENE));
    let red = solid(&device, RED);
    let target = target(&device);
    let depth = depth_buffer(&device);
    // The camera stands 5 m back on +Z, facing the origin (identity rotation faces −Z).
    let camera = Transform::from_translation(Vec3::new(0.0, 0.0, 5.0));
    let scene = Scene::new(&device, &camera);
    let position = Vec3::new(1.5, 1.0, 0.0);

    // Where the cube's centre must land, from the same matrices: top right.
    let clip = scene.clip_from_world * position.extend(1.0);
    let (ndc_x, ndc_y) = (clip.x / clip.w, clip.y / clip.w);
    assert!(ndc_x > 0.3 && ndc_y > 0.2, "({ndc_x}, {ndc_y})");
    let size = SIZE as f32;
    let x = ((ndc_x + 1.0) * 0.5 * size) as u32;
    let y = ((1.0 - ndc_y) * 0.5 * size) as u32; // Clip-space +Y is the top row.

    let mut encoder = device.create_encoder("placed").expect("encoder");
    let cube = scene.cube(&device, red, position, 1.0);
    record_cubes(
        encoder.as_mut(),
        target,
        cleared_depth(depth, StoreOp::DontCare),
        pipeline,
        &[cube],
    );
    let pixels = submit_and_read(&device, encoder, target);
    assert_eq!(seen(&pixels, x, y), Seen::Red, "at ({x}, {y})");
    let (mirror_x, mirror_y) = (SIZE - 1 - x, SIZE - 1 - y);
    assert_eq!(seen(&pixels, mirror_x, y), Seen::Background, "X mirrored");
    assert_eq!(seen(&pixels, x, mirror_y), Seen::Background, "Y mirrored");
    assert_eq!(seen(&pixels, 32, 32), Seen::Background, "centre");

    scene.destroy(&device);
    for texture in [red, target, depth] {
        device.destroy_texture(texture).expect("destroy texture");
    }
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    assert_no_validation_errors(&device);
}

/// A depth-only pass fills the depth buffer; a later pass loads it and tests against it without
/// writing. The far cube is hidden exactly where the prepass drew the near one. Run with both
/// passes in one encoder, and in two submissions (the depth layout is tracked across them).
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn a_depth_prepass_is_loaded_by_a_later_pass() {
    let device = device();
    let prepass = cube_pipeline(&device, &[], Some(DepthState::SCENE));
    let tested = cube_pipeline(
        &device,
        &[FORMAT],
        Some(DepthState {
            write: false,
            ..DepthState::SCENE
        }),
    );
    let (red, green) = (solid(&device, RED), solid(&device, GREEN));
    let target = target(&device);
    let depth = depth_buffer(&device);
    let scene = Scene::new(&device, &Transform::IDENTITY);
    let near = scene.cube(&device, red, Vec3::new(0.0, 0.0, -3.0), 1.0);
    let far = scene.cube(&device, green, Vec3::new(0.0, 0.0, -6.0), 3.0);

    for split in [false, true] {
        let mut encoder = device.create_encoder("prepass").expect("encoder");
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDesc {
                label: "depth prepass",
                color_attachments: &[],
                depth_attachment: Some(cleared_depth(depth, StoreOp::Store)),
            });
            pass.set_pipeline(prepass);
            pass.set_push_constants(&near);
            pass.draw(36, 1);
        }
        if split {
            let submission = device.submit(encoder).expect("submit prepass");
            device.wait_for_submission(submission).expect("wait");
            encoder = device.create_encoder("color").expect("encoder");
        }
        let loaded = DepthAttachment {
            texture: depth,
            load: DepthLoadOp::Load,
            store: StoreOp::DontCare,
        };
        record_cubes(encoder.as_mut(), target, loaded, tested, &[far]);
        let pixels = submit_and_read(&device, encoder, target);
        assert_eq!(
            seen(&pixels, 32, 32),
            Seen::Background,
            "split {split}: hidden by the prepass depth"
        );
        assert_eq!(seen(&pixels, 17, 32), Seen::Green, "split {split}: beside");
    }

    scene.destroy(&device);
    for texture in [red, green, target, depth] {
        device.destroy_texture(texture).expect("destroy texture");
    }
    for pipeline in [prepass, tested] {
        device
            .destroy_render_pipeline(pipeline)
            .expect("destroy pipeline");
    }
    assert_no_validation_errors(&device);
}

#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn depth_misuse_is_refused_with_reasons() {
    let device = device();
    let desc = |format, usage| TextureDesc {
        label: "misuse",
        width: 4,
        height: 4,
        format,
        usage,
    };
    let refused = invalid(device.create_texture(&desc(
        TextureFormat::Depth32Float,
        TextureUsage::DEPTH_TARGET | TextureUsage::SAMPLED,
    )));
    assert!(
        refused.contains("only TextureUsage::DEPTH_TARGET"),
        "{refused}"
    );
    let refused = invalid(device.create_texture(&desc(FORMAT, TextureUsage::DEPTH_TARGET)));
    assert!(refused.contains("needs a depth format"), "{refused}");

    // SAFETY: as in `cube_pipeline`; these pipelines are refused before any shader runs.
    let code = unsafe { ShaderCode::from_spirv_unchecked(cube_spirv()) };
    let depth_as_color = [TextureFormat::Depth32Float];
    for (formats, depth, expected) in [
        (&depth_as_color[..], None, "is a depth format"),
        (&[][..], None, "renders to nothing"),
        (
            &[FORMAT][..],
            Some(DepthState {
                format: FORMAT,
                compare: CompareOp::Always,
                write: true,
            }),
            "is not a depth format",
        ),
    ] {
        let refused = invalid(device.create_render_pipeline(&pipeline_desc(code, formats, depth)));
        assert!(refused.contains(expected), "{refused}");
    }

    let target = target(&device);
    let depth = depth_buffer(&device);
    let with_depth = cube_pipeline(&device, &[FORMAT], Some(DepthState::SCENE));
    let without_depth = cube_pipeline(&device, &[FORMAT], None);
    let mut encoder = device.create_encoder("misuse").expect("encoder");
    let color = [ColorAttachment {
        texture: target,
        load: black(),
        store: StoreOp::Store,
    }];
    // A color texture as the depth attachment.
    drop(encoder.begin_render_pass(&RenderPassDesc {
        label: "color as depth",
        color_attachments: &[],
        depth_attachment: Some(cleared_depth(target, StoreOp::DontCare)),
    }));
    // A depth clear outside [0, 1].
    drop(encoder.begin_render_pass(&RenderPassDesc {
        label: "bad clear",
        color_attachments: &color,
        depth_attachment: Some(DepthAttachment {
            texture: depth,
            load: DepthLoadOp::Clear(2.0),
            store: StoreOp::DontCare,
        }),
    }));
    // A depth-testing pipeline in a pass without depth.
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDesc {
            label: "no depth",
            color_attachments: &color,
            depth_attachment: None,
        });
        pass.set_pipeline(with_depth);
    }
    // And the reverse: a pipeline without depth in a pass with depth.
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDesc {
            label: "unexpected depth",
            color_attachments: &color,
            depth_attachment: Some(cleared_depth(depth, StoreOp::DontCare)),
        });
        pass.set_pipeline(without_depth);
    }
    let details = misuse(device.submit(encoder));
    for expected in [
        "was not created with TextureUsage::DEPTH_TARGET",
        "depth clears must be in [0, 1]",
        "has depth format Some(Depth32Float), but render pass \"no depth\" has None",
        "has depth format None, but render pass \"unexpected depth\" has Some(Depth32Float)",
    ] {
        assert!(details.contains(expected), "{expected:?} in {details}");
    }

    for texture in [target, depth] {
        device.destroy_texture(texture).expect("destroy texture");
    }
    for pipeline in [with_depth, without_depth] {
        device
            .destroy_render_pipeline(pipeline)
            .expect("destroy pipeline");
    }
    assert_no_validation_errors(&device);
}

/// An encoder dropped without submission leaves the depth buffer's tracked layout as it was,
/// so the next pass that loads it starts from the image's real layout. If tracking kept the
/// dropped pass's layout, the validation layers would report the mismatch at submit.
#[test]
#[ignore = "requires a Vulkan 1.3 device, validation layers and the pinned Slang"]
fn abandoned_depth_passes_leave_the_layout_unchanged() {
    let device = device();
    let pipeline = cube_pipeline(&device, &[FORMAT], Some(DepthState::SCENE));
    let red = solid(&device, RED);
    let target = target(&device);
    let depth = depth_buffer(&device);
    let scene = Scene::new(&device, &Transform::IDENTITY);
    let near = scene.cube(&device, red, Vec3::new(0.0, 0.0, -3.0), 1.0);

    let mut abandoned = device.create_encoder("abandoned").expect("encoder");
    record_cubes(
        abandoned.as_mut(),
        target,
        cleared_depth(depth, StoreOp::Store),
        pipeline,
        &[near],
    );
    drop(abandoned);

    let mut encoder = device.create_encoder("loads").expect("encoder");
    let loaded = DepthAttachment {
        texture: depth,
        load: DepthLoadOp::Load,
        store: StoreOp::DontCare,
    };
    record_cubes(encoder.as_mut(), target, loaded, pipeline, &[]);
    submit_and_read(&device, encoder, target);

    scene.destroy(&device);
    for texture in [red, target, depth] {
        device.destroy_texture(texture).expect("destroy texture");
    }
    device
        .destroy_render_pipeline(pipeline)
        .expect("destroy pipeline");
    assert_no_validation_errors(&device);
}
