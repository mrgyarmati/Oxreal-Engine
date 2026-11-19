//! A spinning textured cube seen through a 3D camera, with a depth buffer (ADR-0011 §4).
//!
//! ```text
//! cargo xtask slang install                                      # once: the pinned shader compiler
//! cargo run -p oxreal-rhi-vulkan --example cube                  # until the window is closed
//! cargo run -p oxreal-rhi-vulkan --example cube -- --frames 300  # exit after 300 frames (CI)
//! ```
//!
//! The camera uses `oxreal_math`'s view matrix and reversed-depth infinite perspective. The
//! depth buffer is `Depth32Float`, cleared to `REVERSED_DEPTH_CLEAR` every frame and recreated
//! when the backbuffer's size changes. The cube's vertices come from the vertex index
//! (`examples/shaders/Cube.slang`), and every face samples a checkerboard by bindless index.
//! Like `clear`, it fails on any validation error or if frames stop arriving before
//! `--timeout-secs` (default 60).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oxreal_diagnostics::{info, warn};
use oxreal_math::projection::{REVERSED_DEPTH_CLEAR, perspective_infinite_reversed, view_matrix};
use oxreal_math::{LinearRgba, Quat, Transform, Vec3};
use oxreal_rhi::{
    AddressMode, ColorAttachment, CullMode, DepthAttachment, DepthLoadOp, DepthState, Device as _,
    DeviceDesc, Filter, Frame, LoadOp, PipelineHandle, PresentMode, RenderPassDesc,
    RenderPipelineDesc, RhiError, SamplerDesc, SamplerHandle, ShaderCode, ShaderEntry, StoreOp,
    SurfaceDesc, SurfaceHandle, TextureDesc, TextureFormat, TextureHandle, TextureUsage,
    Validation,
};
use oxreal_rhi_vulkan::VulkanDevice;
use oxreal_shader_compiler::{CompileRequest, Optimization, SlangCompiler, validate_spirv};
use oxreal_window::{App, Context, EventLoop, Window, WindowDesc, WindowEvent, WindowId};

/// Checkerboard size in texels, and squares per side.
const BOARD_TEXELS: u32 = 64;
const BOARD_SQUARES: u32 = 4;
/// CubeConstants: a 4×4 matrix, then the texture and sampler indices.
const CONSTANT_BYTES: usize = 72;
/// Vertical field of view, and the near plane in metres.
const FIELD_OF_VIEW_DEGREES: f32 = 60.0;
const NEAR: f32 = 0.05;

struct Options {
    frames: Option<u64>,
    timeout: Duration,
}

/// The texture and sampler the cube samples, and their bindless indices.
struct Board {
    texture: TextureHandle,
    sampler: SamplerHandle,
    texture_index: u32,
    sampler_index: u32,
}

/// The depth buffer and the backbuffer size it was created for.
struct DepthBuffer {
    texture: TextureHandle,
    size: (u32, u32),
}

struct CubeApp {
    device: VulkanDevice,
    options: Options,
    spirv: Vec<u32>,
    board: Board,
    /// The pipeline and the backbuffer format it was created for.
    pipeline: Option<(PipelineHandle, TextureFormat)>,
    depth: Option<DepthBuffer>,
    window: Option<Arc<Window>>,
    surface: Option<SurfaceHandle>,
    frames: u64,
    started: Instant,
    error: Option<String>,
}

impl CubeApp {
    fn fail(&mut self, ctx: &Context<'_>, message: String) {
        if self.error.is_none() {
            self.error = Some(message);
        }
        ctx.exit();
    }

    /// Ends `frame` without drawing, then fails: a frame must always be ended, or its swapchain
    /// image stays acquired.
    fn abandon_frame(&mut self, ctx: &Context<'_>, frame: Frame, message: String) {
        if let Err(end) = self.device.end_frame(frame) {
            warn!(error = %end, "ending the frame after an error failed");
        }
        self.fail(ctx, message);
    }

    /// The pipeline for `format`, created (or recreated) as needed.
    fn pipeline_for(&mut self, format: TextureFormat) -> Result<PipelineHandle, RhiError> {
        if let Some((pipeline, created_for)) = self.pipeline {
            if created_for == format {
                return Ok(pipeline);
            }
            // Destruction is deferred until the GPU is done with it.
            self.device.destroy_render_pipeline(pipeline)?;
            self.pipeline = None;
        }
        // SAFETY: produced by the pinned `slangc` through oxreal-shader-compiler and checked
        // with `spirv-val` in `compile_shader` when it's installed (always in CI). The only
        // indices it's given are the board's texture and sampler, which live until `exiting`,
        // after the last frame, and the board is never rendered into.
        let code = unsafe { ShaderCode::from_spirv_unchecked(&self.spirv) };
        let pipeline = self.device.create_render_pipeline(&RenderPipelineDesc {
            label: "cube",
            vertex: ShaderEntry {
                code,
                entry_point: "cubeVertex",
            },
            pixel: ShaderEntry {
                code,
                entry_point: "cubePixel",
            },
            color_formats: &[format],
            depth: Some(DepthState::SCENE),
            push_constant_bytes: CONSTANT_BYTES as u32,
            cull: CullMode::Back,
        })?;
        info!(?format, "cube pipeline created");
        self.pipeline = Some((pipeline, format));
        Ok(pipeline)
    }

    /// The depth buffer for a backbuffer of `size`, recreated when the size changes.
    fn depth_for(&mut self, size: (u32, u32)) -> Result<TextureHandle, RhiError> {
        if let Some(depth) = &self.depth {
            if depth.size == size {
                return Ok(depth.texture);
            }
            // Destruction is deferred until the GPU is done with it.
            self.device.destroy_texture(depth.texture)?;
            self.depth = None;
        }
        let texture = self.device.create_texture(&TextureDesc {
            label: "depth",
            width: size.0,
            height: size.1,
            format: TextureFormat::Depth32Float,
            usage: TextureUsage::DEPTH_TARGET,
        })?;
        info!(width = size.0, height = size.1, "depth buffer created");
        self.depth = Some(DepthBuffer { texture, size });
        Ok(texture)
    }

    /// Push constants for this moment: the cube turns about two axes in front of a fixed camera.
    fn constants(&self, size: (u32, u32)) -> Result<[u8; CONSTANT_BYTES], String> {
        let seconds = self.started.elapsed().as_secs_f32();
        let model = Transform::from_rotation(
            Quat::from_rotation_y(seconds * 0.7) * Quat::from_rotation_x(seconds * 0.45),
        );
        // A little above the cube, looking down at it.
        let camera =
            Transform::from_translation(Vec3::new(0.0, 0.8, 2.2)).looking_at(Vec3::ZERO, Vec3::Y);
        // Sizes are far below 2^24, where f32 would lose precision.
        let aspect = size.0 as f32 / size.1 as f32;
        let projection =
            perspective_infinite_reversed(FIELD_OF_VIEW_DEGREES.to_radians(), aspect, NEAR)
                .map_err(|e| e.to_string())?;
        let clip_from_object = projection * view_matrix(&camera) * model.to_matrix();

        let mut bytes = [0; CONSTANT_BYTES];
        let (words, _) = bytes.as_chunks_mut::<4>();
        for (chunk, value) in words.iter_mut().zip(clip_from_object.to_cols_array()) {
            *chunk = value.to_le_bytes();
        }
        bytes[64..68].copy_from_slice(&self.board.texture_index.to_le_bytes());
        bytes[68..72].copy_from_slice(&self.board.sampler_index.to_le_bytes());
        Ok(bytes)
    }

    fn draw(&mut self, ctx: &Context<'_>) {
        let (Some(window), Some(surface)) = (self.window.clone(), self.surface) else {
            return;
        };
        let mut frame = match self.device.begin_frame(surface) {
            Ok(Some(frame)) => frame,
            Ok(None) => return, // Minimized or recreating: skip this frame.
            Err(e) => return self.fail(ctx, e.to_string()),
        };
        let size = frame.backbuffer_size();
        let prepared = self
            .pipeline_for(frame.backbuffer_format())
            .and_then(|pipeline| Ok((pipeline, self.depth_for(size)?)))
            .map_err(|e| e.to_string())
            .and_then(|(pipeline, depth)| Ok((pipeline, depth, self.constants(size)?)));
        let (pipeline, depth, constants) = match prepared {
            Ok(prepared) => prepared,
            Err(e) => return self.abandon_frame(ctx, frame, e),
        };

        let backbuffer = frame.backbuffer();
        {
            let attachments = [ColorAttachment {
                texture: backbuffer,
                load: LoadOp::Clear(LinearRgba::new(0.02, 0.02, 0.025, 1.0)),
                store: StoreOp::Store,
            }];
            let mut pass = frame.encoder().begin_render_pass(&RenderPassDesc {
                label: "cube",
                color_attachments: &attachments,
                depth_attachment: Some(DepthAttachment {
                    texture: depth,
                    load: DepthLoadOp::Clear(REVERSED_DEPTH_CLEAR),
                    store: StoreOp::DontCare, // Only this pass reads it.
                }),
            });
            pass.set_pipeline(pipeline);
            pass.set_push_constants(&constants);
            pass.draw(36, 1);
        } // The pass ends here.
        window.pre_present_notify();
        if let Err(e) = self.device.end_frame(frame) {
            return self.fail(ctx, e.to_string());
        }
        self.frames += 1;
        if self
            .options
            .frames
            .is_some_and(|target| self.frames >= target)
        {
            ctx.exit();
        }
    }
}

impl App for CubeApp {
    fn resumed(&mut self, ctx: &Context<'_>) {
        if self.window.is_some() {
            return;
        }
        let window = match ctx.create_window(&WindowDesc {
            title: "Oxreal: cube".to_owned(),
            ..WindowDesc::default()
        }) {
            Ok(window) => Arc::new(window),
            Err(e) => return self.fail(ctx, e.to_string()),
        };
        let size = window.inner_size();
        let desc = SurfaceDesc {
            width: size.width,
            height: size.height,
            present_mode: PresentMode::Fifo,
        };
        match self.device.create_surface(window.clone(), &desc) {
            Ok(surface) => self.surface = Some(surface),
            Err(e) => return self.fail(ctx, e.to_string()),
        }
        info!(display_server = ?ctx.display_server(), ?size, "window ready");
        ctx.set_continuous(true);
        self.window = Some(window);
    }

    fn window_event(&mut self, ctx: &Context<'_>, _window: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => ctx.exit(),
            WindowEvent::Resized(size) => {
                if let Some(surface) = self.surface
                    && let Err(e) = self.device.resize_surface(surface, size.width, size.height)
                {
                    self.fail(ctx, e.to_string());
                }
            }
            WindowEvent::RedrawRequested => self.draw(ctx),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, ctx: &Context<'_>) {
        if self.options.frames.is_some() && self.started.elapsed() > self.options.timeout {
            let message = format!(
                "timed out after {} frame(s) in {:?}",
                self.frames, self.options.timeout
            );
            return self.fail(ctx, message);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn exiting(&mut self) {
        if let Some((pipeline, _)) = self.pipeline.take()
            && let Err(e) = self.device.destroy_render_pipeline(pipeline)
        {
            warn!(error = %e, "destroying the pipeline failed");
        }
        if let Some(depth) = self.depth.take()
            && let Err(e) = self.device.destroy_texture(depth.texture)
        {
            warn!(error = %e, "destroying the depth buffer failed");
        }
        if let Err(e) = self.device.destroy_sampler(self.board.sampler) {
            warn!(error = %e, "destroying the sampler failed");
        }
        if let Err(e) = self.device.destroy_texture(self.board.texture) {
            warn!(error = %e, "destroying the texture failed");
        }
        // The surface goes before the window it presents to.
        if let Some(surface) = self.surface.take()
            && let Err(e) = self.device.destroy_surface(surface)
        {
            warn!(error = %e, "destroying the surface failed");
        }
        self.window = None;
    }
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        frames: None,
        timeout: Duration::from_secs(60),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--frames" => {
                let v = value("--frames")?;
                options.frames = Some(
                    v.parse()
                        .map_err(|_| format!("--frames {v:?} is not a whole number"))?,
                );
            }
            "--timeout-secs" => {
                let v = value("--timeout-secs")?;
                options.timeout = Duration::from_secs(
                    v.parse()
                        .map_err(|_| format!("--timeout-secs {v:?} is not a whole number"))?,
                );
            }
            other => {
                return Err(format!(
                    "unknown argument {other:?}; usage: cube [--frames N] [--timeout-secs S]"
                ));
            }
        }
    }
    Ok(options)
}

fn compile_shader() -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = crate_dir.join("examples/shaders/Cube.slang");
    // Where `Bindless.slang`, the shader side of the bindless binding model, lives.
    let bindless = crate_dir.join("../../rhi/shaders");
    let compiler = SlangCompiler::locate()?;
    let shader = compiler.compile(&CompileRequest {
        source: &source,
        entry_points: &["cubeVertex", "cubePixel"],
        include_dirs: &[bindless],
        optimization: Optimization::Release,
    })?;
    let validated = validate_spirv(&shader.spirv, &source)?;
    if !validated {
        warn!("spirv-val not found (Vulkan SDK or spirv-tools); shader not validated");
    }
    info!(
        slang = compiler.version(),
        words = shader.spirv.len(),
        validated,
        "shader compiled"
    );
    Ok(shader.spirv)
}

/// Uploads a checkerboard: Oxreal orange and a dark grey, picked in sRGB. The texture is sRGB too,
/// so sampling decodes it to linear color and the sRGB swapchain encodes it back (ADR-0011 §5).
fn create_board(device: &VulkanDevice) -> Result<Board, RhiError> {
    let square = BOARD_TEXELS / BOARD_SQUARES;
    let pixels: Vec<u8> = (0..BOARD_TEXELS)
        .flat_map(|y| (0..BOARD_TEXELS).map(move |x| (x, y)))
        .flat_map(|(x, y)| {
            if (x / square + y / square).is_multiple_of(2) {
                [0xE8, 0x6A, 0x17, 0xFF]
            } else {
                [0x24, 0x26, 0x2B, 0xFF]
            }
        })
        .collect();
    let texture = device.create_texture(&TextureDesc {
        label: "checkerboard",
        width: BOARD_TEXELS,
        height: BOARD_TEXELS,
        format: TextureFormat::Rgba8UnormSrgb,
        usage: TextureUsage::SAMPLED | TextureUsage::COPY_DESTINATION,
    })?;
    device.write_texture(texture, &pixels)?;
    let sampler = device.create_sampler(&SamplerDesc {
        label: "checkerboard sampler",
        mag_filter: Filter::Linear,
        min_filter: Filter::Linear,
        address_mode: AddressMode::ClampToEdge,
        // The faces are seen at steep angles as the cube turns.
        max_anisotropy: Some(8.0),
    })?;
    Ok(Board {
        texture,
        sampler,
        texture_index: device.texture_index(texture)?,
        sampler_index: device.sampler_index(sampler)?,
    })
}

fn main() -> std::process::ExitCode {
    oxreal_diagnostics::run_application(run)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_options()?;
    let spirv = compile_shader()?;
    let device = oxreal_rhi_vulkan::create_device(&DeviceDesc {
        app_name: "oxreal cube".to_owned(),
        validation: Validation::Enabled,
        ..DeviceDesc::default()
    })?;
    let board = create_board(&device)?;
    let app = CubeApp {
        device,
        options,
        spirv,
        board,
        pipeline: None,
        depth: None,
        window: None,
        surface: None,
        frames: 0,
        started: Instant::now(),
        error: None,
    };

    let app = EventLoop::new()?.run(app)?;
    let elapsed = app.started.elapsed();
    let errors = app.device.validation_error_count();
    if let Some(error) = app.error {
        return Err(error.into());
    }
    if errors > 0 {
        return Err(format!("Vulkan validation reported {errors} error(s); see the log").into());
    }
    if let Some(target) = app.options.frames
        && app.frames < target
    {
        return Err(format!("window closed after {} of {target} frames", app.frames).into());
    }
    info!(
        frames = app.frames,
        seconds = format_args!("{:.2}", elapsed.as_secs_f32()),
        validation_errors = errors,
        "cube example finished"
    );
    Ok(())
}
