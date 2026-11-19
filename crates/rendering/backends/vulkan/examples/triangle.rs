//! A spinning triangle: Oxreal's first draw call.
//!
//! ```text
//! cargo xtask slang install                                          # once: the pinned shader compiler
//! cargo run -p oxreal-rhi-vulkan --example triangle                  # until the window is closed
//! cargo run -p oxreal-rhi-vulkan --example triangle -- --frames 300  # exit after 300 frames (CI)
//! ```
//!
//! Compiles `examples/shaders/Triangle.slang` with the pinned Slang, creates a pipeline for the
//! backbuffer's format (recreated if the format changes), and draws three vertices without a
//! vertex buffer. The rotation and aspect ratio are push constants. Like `clear`, it fails on any
//! validation error or if frames stop arriving before `--timeout-secs` (default 60).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oxreal_diagnostics::{info, warn};
use oxreal_math::LinearRgba;
use oxreal_rhi::{
    ColorAttachment, CullMode, Device as _, DeviceDesc, LoadOp, PipelineHandle, PresentMode,
    RenderPassDesc, RenderPipelineDesc, ShaderCode, ShaderEntry, StoreOp, SurfaceDesc,
    SurfaceHandle, TextureFormat, Validation,
};
use oxreal_rhi_vulkan::VulkanDevice;
use oxreal_shader_compiler::{CompileRequest, Optimization, SlangCompiler, validate_spirv};
use oxreal_window::{App, Context, EventLoop, Window, WindowDesc, WindowEvent, WindowId};

/// Radians per second.
const SPIN_SPEED: f32 = 0.8;

struct Options {
    frames: Option<u64>,
    timeout: Duration,
}

struct TriangleApp {
    device: VulkanDevice,
    options: Options,
    spirv: Vec<u32>,
    /// The pipeline and the backbuffer format it was created for.
    pipeline: Option<(PipelineHandle, TextureFormat)>,
    window: Option<Arc<Window>>,
    surface: Option<SurfaceHandle>,
    size: (u32, u32),
    frames: u64,
    started: Instant,
    error: Option<String>,
}

impl TriangleApp {
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
        // with `spirv-val` in `compile_shader` when it's installed (always in CI).
        let code = unsafe { ShaderCode::from_spirv_unchecked(&self.spirv) };
        let pipeline = self
            .device
            .create_render_pipeline(&RenderPipelineDesc {
                label: "triangle",
                vertex: ShaderEntry {
                    code,
                    entry_point: "triangleVertex",
                },
                pixel: ShaderEntry {
                    code,
                    entry_point: "trianglePixel",
                },
                color_formats: &[format],
                push_constant_bytes: 8,
                cull: CullMode::Back,
            })
            .map_err(|e| e.to_string())?;
        info!(?format, "triangle pipeline created");
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
        let rotation = self.started.elapsed().as_secs_f32() * SPIN_SPEED;
        let aspect = self.size.0.max(1) as f32 / self.size.1.max(1) as f32;
        let mut constants = [0_u8; 8];
        constants[..4].copy_from_slice(&rotation.to_le_bytes());
        constants[4..].copy_from_slice(&aspect.to_le_bytes());

        let backbuffer = frame.backbuffer();
        {
            let attachments = [ColorAttachment {
                texture: backbuffer,
                load: LoadOp::Clear(LinearRgba::from_srgb(0.08, 0.09, 0.11, 1.0)),
                store: StoreOp::Store,
            }];
            let mut pass = frame.encoder().begin_render_pass(&RenderPassDesc {
                label: "triangle",
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

impl App for TriangleApp {
    fn resumed(&mut self, ctx: &Context<'_>) {
        if self.window.is_some() {
            return;
        }
        let window = match ctx.create_window(&WindowDesc {
            title: "Oxreal: triangle".to_owned(),
            ..WindowDesc::default()
        }) {
            Ok(window) => Arc::new(window),
            Err(e) => return self.fail(ctx, e.to_string()),
        };
        let size = window.inner_size();
        self.size = (size.width, size.height);
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
                self.size = (size.width, size.height);
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
                    "unknown argument {other:?}; usage: triangle [--frames N] [--timeout-secs S]"
                ));
            }
        }
    }
    Ok(options)
}

fn compile_shader() -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/shaders/Triangle.slang");
    let compiler = SlangCompiler::locate()?;
    let shader = compiler.compile(&CompileRequest {
        source: &source,
        entry_points: &["triangleVertex", "trianglePixel"],
        include_dirs: &[],
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

fn main() -> std::process::ExitCode {
    oxreal_diagnostics::run_application(run)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_options()?;
    let spirv = compile_shader()?;
    let device = oxreal_rhi_vulkan::create_device(&DeviceDesc {
        app_name: "oxreal triangle".to_owned(),
        validation: Validation::Enabled,
        ..DeviceDesc::default()
    })?;
    let app = TriangleApp {
        device,
        options,
        spirv,
        pipeline: None,
        window: None,
        surface: None,
        size: (1, 1),
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
        "triangle example finished"
    );
    Ok(())
}
