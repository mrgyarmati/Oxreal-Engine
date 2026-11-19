//! A checkerboard texture, sampled through its bindless index (ADR-0016 §2).
//!
//! ```text
//! cargo xtask slang install                                          # once: the pinned shader compiler
//! cargo run -p oxreal-rhi-vulkan --example textured                  # until the window is closed
//! cargo run -p oxreal-rhi-vulkan --example textured -- --frames 300  # exit after 300 frames (CI)
//! ```
//!
//! Generates a checkerboard on the CPU, uploads it once with `write_texture`, and draws a
//! full-window quad that samples it by index with a nearest-filtering sampler. The shader is
//! `examples/shaders/TexturedQuad.slang`, which imports the RHI's `Bindless.slang`. The tiling
//! scale changes over time. Like `clear`, it fails on any validation error or if frames stop
//! arriving before `--timeout-secs` (default 60).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oxreal_diagnostics::{info, warn};
use oxreal_rhi::{
    AddressMode, ColorAttachment, CullMode, Device as _, DeviceDesc, Filter, LoadOp,
    PipelineHandle, PresentMode, RenderPassDesc, RenderPipelineDesc, RhiError, SamplerDesc,
    SamplerHandle, ShaderCode, ShaderEntry, StoreOp, SurfaceDesc, SurfaceHandle, TextureDesc,
    TextureFormat, TextureHandle, TextureUsage, Validation,
};
use oxreal_rhi_vulkan::VulkanDevice;
use oxreal_shader_compiler::{CompileRequest, Optimization, SlangCompiler, validate_spirv};
use oxreal_window::{App, Context, EventLoop, Window, WindowDesc, WindowEvent, WindowId};

/// Checkerboard size in texels, and squares per side.
const BOARD_TEXELS: u32 = 64;
const BOARD_SQUARES: u32 = 8;
/// Seconds per zoom cycle.
const ZOOM_PERIOD: f32 = 6.0;

struct Options {
    frames: Option<u64>,
    timeout: Duration,
}

/// The texture and sampler the quad samples, and their bindless indices.
struct Board {
    texture: TextureHandle,
    sampler: SamplerHandle,
    texture_index: u32,
    sampler_index: u32,
}

struct TexturedApp {
    device: VulkanDevice,
    options: Options,
    spirv: Vec<u32>,
    board: Board,
    /// The pipeline and the backbuffer format it was created for.
    pipeline: Option<(PipelineHandle, TextureFormat)>,
    window: Option<Arc<Window>>,
    surface: Option<SurfaceHandle>,
    frames: u64,
    started: Instant,
    error: Option<String>,
}

impl TexturedApp {
    fn fail(&mut self, ctx: &Context<'_>, message: String) {
        if self.error.is_none() {
            self.error = Some(message);
        }
        ctx.exit();
    }

    /// The pipeline for `format`, created (or recreated) as needed.
    fn pipeline_for(&mut self, format: TextureFormat) -> Result<PipelineHandle, String> {
        if let Some((pipeline, created_for)) = self.pipeline {
            if created_for == format {
                return Ok(pipeline);
            }
            // Destruction is deferred until the GPU is done with it.
            self.device
                .destroy_render_pipeline(pipeline)
                .map_err(|e| e.to_string())?;
            self.pipeline = None;
        }
        // SAFETY: produced by the pinned `slangc` through oxreal-shader-compiler and checked
        // with `spirv-val` in `compile_shader` when it's installed (always in CI). The only
        // indices it's given are the board's texture and sampler, which live until `exiting`,
        // after the last frame.
        let code = unsafe { ShaderCode::from_spirv_unchecked(&self.spirv) };
        let pipeline = self
            .device
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
                color_formats: &[format],
                push_constant_bytes: 12,
                cull: CullMode::None,
            })
            .map_err(|e| e.to_string())?;
        info!(?format, "textured pipeline created");
        self.pipeline = Some((pipeline, format));
        Ok(pipeline)
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
        let pipeline = match self.pipeline_for(frame.backbuffer_format()) {
            Ok(pipeline) => pipeline,
            Err(e) => {
                // The frame must still be ended, or its swapchain image stays acquired.
                if let Err(end) = self.device.end_frame(frame) {
                    warn!(error = %end, "ending the frame after a pipeline error failed");
                }
                return self.fail(ctx, e);
            }
        };
        // The board repeats 1 to 4 times across the window, back and forth.
        let phase = self.started.elapsed().as_secs_f32() / ZOOM_PERIOD * std::f32::consts::TAU;
        let scale = 2.5 - 1.5 * phase.cos();
        // QuadConstants: texture index, sampler index, scale.
        let mut constants = [0_u8; 12];
        constants[..4].copy_from_slice(&self.board.texture_index.to_le_bytes());
        constants[4..8].copy_from_slice(&self.board.sampler_index.to_le_bytes());
        constants[8..].copy_from_slice(&scale.to_le_bytes());

        let backbuffer = frame.backbuffer();
        {
            let attachments = [ColorAttachment {
                texture: backbuffer,
                load: LoadOp::DontCare, // The quad covers every pixel.
                store: StoreOp::Store,
            }];
            let mut pass = frame.encoder().begin_render_pass(&RenderPassDesc {
                label: "textured",
                color_attachments: &attachments,
            });
            pass.set_pipeline(pipeline);
            pass.set_push_constants(&constants);
            pass.draw(3, 1);
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

impl App for TexturedApp {
    fn resumed(&mut self, ctx: &Context<'_>) {
        if self.window.is_some() {
            return;
        }
        let window = match ctx.create_window(&WindowDesc {
            title: "Oxreal: textured".to_owned(),
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
                    "unknown argument {other:?}; usage: textured [--frames N] [--timeout-secs S]"
                ));
            }
        }
    }
    Ok(options)
}

fn compile_shader() -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = crate_dir.join("examples/shaders/TexturedQuad.slang");
    // Where `Bindless.slang`, the shader side of the bindless binding model, lives.
    let bindless = crate_dir.join("../../rhi/shaders");
    let compiler = SlangCompiler::locate()?;
    let shader = compiler.compile(&CompileRequest {
        source: &source,
        entry_points: &["quadVertex", "quadPixel"],
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
        mag_filter: Filter::Nearest,
        min_filter: Filter::Linear,
        address_mode: AddressMode::Repeat,
        max_anisotropy: None,
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
        app_name: "oxreal textured".to_owned(),
        validation: Validation::Enabled,
        ..DeviceDesc::default()
    })?;
    let board = create_board(&device)?;
    let app = TexturedApp {
        device,
        options,
        spirv,
        board,
        pipeline: None,
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
        "textured example finished"
    );
    Ok(())
}
