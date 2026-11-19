//! Clears a window to a slowly changing color: the first pixels Oxreal draws.
//!
//! ```text
//! cargo run -p oxreal-rhi-vulkan --example clear                    # until the window is closed
//! cargo run -p oxreal-rhi-vulkan --example clear -- --frames 300    # exit after 300 frames (CI)
//! cargo run -p oxreal-rhi-vulkan --example clear -- --present-mode mailbox
//! ```
//!
//! Exercises the whole frame loop: surface, swapchain, acquire, a cleared render pass, submit,
//! present, resize and minimize. It fails if Vulkan validation reported any error, or if frames
//! stop arriving before `--timeout-secs` (default 60).

use std::sync::Arc;
use std::time::{Duration, Instant};

use oxreal_diagnostics::{info, warn};
use oxreal_math::LinearRgba;
use oxreal_rhi::{
    ColorAttachment, Device as _, DeviceDesc, LoadOp, PresentMode, RenderPassDesc, StoreOp,
    SurfaceDesc, SurfaceHandle, Validation,
};
use oxreal_rhi_vulkan::VulkanDevice;
use oxreal_window::{App, Context, EventLoop, Window, WindowDesc, WindowEvent, WindowId};

struct Options {
    frames: Option<u64>,
    timeout: Duration,
    present_mode: PresentMode,
}

struct ClearApp {
    device: VulkanDevice,
    options: Options,
    window: Option<Arc<Window>>,
    surface: Option<SurfaceHandle>,
    frames: u64,
    started: Instant,
    error: Option<String>,
}

impl ClearApp {
    fn fail(&mut self, ctx: &Context<'_>, message: String) {
        if self.error.is_none() {
            self.error = Some(message);
        }
        ctx.exit();
    }

    fn draw(&mut self, ctx: &Context<'_>) {
        let (Some(window), Some(surface)) = (&self.window, self.surface) else {
            return;
        };
        let mut frame = match self.device.begin_frame(surface) {
            Ok(Some(frame)) => frame,
            Ok(None) => return, // Minimized or recreating: skip this frame.
            Err(e) => return self.fail(ctx, e.to_string()),
        };
        let color = color_at(self.started.elapsed().as_secs_f32());
        let backbuffer = frame.backbuffer();
        {
            let attachments = [ColorAttachment {
                texture: backbuffer,
                load: LoadOp::Clear(color),
                store: StoreOp::Store,
            }];
            let _pass = frame.encoder().begin_render_pass(&RenderPassDesc {
                label: "clear",
                color_attachments: &attachments,
                depth_attachment: None,
            });
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

impl App for ClearApp {
    fn resumed(&mut self, ctx: &Context<'_>) {
        if self.window.is_some() {
            return;
        }
        let window = match ctx.create_window(&WindowDesc {
            title: "Oxreal: clear".to_owned(),
            ..WindowDesc::default()
        }) {
            Ok(window) => Arc::new(window),
            Err(e) => return self.fail(ctx, e.to_string()),
        };
        let size = window.inner_size();
        let desc = SurfaceDesc {
            width: size.width,
            height: size.height,
            present_mode: self.options.present_mode,
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
        // The surface goes before the window it presents to.
        if let Some(surface) = self.surface.take()
            && let Err(e) = self.device.destroy_surface(surface)
        {
            warn!(error = %e, "destroying the surface failed");
        }
        self.window = None;
    }
}

/// A slow cycle through hues. Picked in sRGB (as a person would) and converted to linear.
fn color_at(seconds: f32) -> LinearRgba {
    let hue = (seconds * 0.1).fract() * 6.0;
    let x = 1.0 - (hue % 2.0 - 1.0).abs();
    let (r, g, b) = match hue as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    // Keep it calm: 60 % saturation and value.
    let mix = |c: f32| 0.6 * (0.4 + 0.6 * c);
    LinearRgba::from_srgb(mix(r), mix(g), mix(b), 1.0)
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        frames: None,
        timeout: Duration::from_secs(60),
        present_mode: PresentMode::Fifo,
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
            "--present-mode" => {
                options.present_mode = match value("--present-mode")?.as_str() {
                    "fifo" => PresentMode::Fifo,
                    "mailbox" => PresentMode::Mailbox,
                    "immediate" => PresentMode::Immediate,
                    other => {
                        return Err(format!(
                            "unknown present mode {other:?}: fifo, mailbox or immediate"
                        ));
                    }
                };
            }
            other => {
                return Err(format!(
                    "unknown argument {other:?}; usage: clear [--frames N] [--timeout-secs S] [--present-mode fifo|mailbox|immediate]"
                ));
            }
        }
    }
    Ok(options)
}

fn main() -> std::process::ExitCode {
    oxreal_diagnostics::run_application(run)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_options()?;
    let device = oxreal_rhi_vulkan::create_device(&DeviceDesc {
        app_name: "oxreal clear".to_owned(),
        validation: Validation::Enabled,
        ..DeviceDesc::default()
    })?;
    let app = ClearApp {
        device,
        options,
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
        fps = format_args!(
            "{:.1}",
            app.frames as f32 / elapsed.as_secs_f32().max(f32::EPSILON)
        ),
        validation_errors = errors,
        "clear example finished"
    );
    Ok(())
}
