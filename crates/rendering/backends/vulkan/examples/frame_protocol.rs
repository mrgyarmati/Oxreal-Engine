//! Checks that misusing the frame loop is reported or recovered, never undefined behaviour.
//!
//! ```text
//! cargo run -p oxreal-rhi-vulkan --example frame_protocol
//! ```
//!
//! These are regression checks from the `unsafe` audit of the first draw call (S4, C3). They need
//! a real window and swapchain, which the test harness can't create on its main thread, so they
//! run as an example. CI runs it on Lavapipe under Xvfb. In one window it checks:
//!
//! 1. A second `begin_frame` while a frame is open is refused.
//! 2. `destroy_surface` while a frame is open is refused.
//! 3. A standalone encoder can't render to the open frame's backbuffer.
//! 4. A `Frame` whose public parts were tampered with still presents its own backbuffer.
//! 5. A dropped frame is recovered by the next `begin_frame`.
//! 6. Normal frames still work afterwards.
//!
//! It fails if any check fails, or if Vulkan validation reports an error.

use std::sync::Arc;
use std::time::{Duration, Instant};

use oxreal_diagnostics::info;
use oxreal_math::LinearRgba;
use oxreal_rhi::{
    ColorAttachment, Device as _, DeviceDesc, Frame, LoadOp, PresentMode, RenderPassDesc, RhiError,
    StoreOp, SurfaceDesc, SurfaceHandle, TextureDesc, TextureFormat, TextureUsage, Validation,
};
use oxreal_rhi_vulkan::VulkanDevice;
use oxreal_window::{App, Context, EventLoop, Window, WindowDesc, WindowEvent, WindowId};

/// Normal frames rendered after the checks.
const NORMAL_FRAMES: u32 = 30;
const TIMEOUT: Duration = Duration::from_secs(60);

struct ProtocolApp {
    device: VulkanDevice,
    window: Option<Arc<Window>>,
    surface: Option<SurfaceHandle>,
    checks_done: bool,
    normal_frames: u32,
    started: Instant,
    error: Option<String>,
}

fn expect_invalid<T: std::fmt::Debug>(
    what: &str,
    result: Result<T, RhiError>,
    needle: &str,
) -> Result<(), String> {
    match result {
        Err(RhiError::InvalidUsage { details }) if details.contains(needle) => {
            info!(check = what, %details, "refused as expected");
            Ok(())
        }
        other => Err(format!(
            "{what}: expected InvalidUsage mentioning {needle:?}, got {other:?}"
        )),
    }
}

impl ProtocolApp {
    /// Begins a frame, retrying while the swapchain is being (re)created.
    fn frame(&self, surface: SurfaceHandle) -> Result<Frame, String> {
        for _ in 0..100 {
            match self.device.begin_frame(surface) {
                Ok(Some(frame)) => return Ok(frame),
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(format!("begin_frame: {e}")),
            }
        }
        Err("begin_frame kept returning None".to_owned())
    }

    fn clear(frame: &mut Frame, color: LinearRgba) {
        let attachments = [ColorAttachment {
            texture: frame.backbuffer(),
            load: LoadOp::Clear(color),
            store: StoreOp::Store,
        }];
        let _pass = frame.encoder().begin_render_pass(&RenderPassDesc {
            label: "clear",
            color_attachments: &attachments,
            depth_attachment: None,
        });
    }

    fn run_checks(&self, surface: SurfaceHandle) -> Result<(), String> {
        let device = &self.device;
        let mut frame = self.frame(surface)?;

        // 1 and 2: one open frame per surface, and it must be ended first.
        expect_invalid(
            "second begin_frame",
            device.begin_frame(surface),
            "already has an open frame",
        )?;
        expect_invalid(
            "destroy_surface with an open frame",
            device.destroy_surface(surface),
            "has an open frame",
        )?;

        // 3: the backbuffer belongs to the frame's encoder.
        let mut other = device
            .create_encoder("not the frame")
            .map_err(|e| e.to_string())?;
        {
            let attachments = [ColorAttachment {
                texture: frame.backbuffer(),
                load: LoadOp::Clear(LinearRgba::new(1.0, 0.0, 0.0, 1.0)),
                store: StoreOp::Store,
            }];
            let _pass = other.begin_render_pass(&RenderPassDesc {
                label: "steal the backbuffer",
                color_attachments: &attachments,
                depth_attachment: None,
            });
        }
        expect_invalid(
            "standalone encoder using the backbuffer",
            device.submit(other),
            "isn't acquired by this frame",
        )?;

        // 4: end_frame ignores tampered public parts and presents the frame's own image.
        Self::clear(&mut frame, LinearRgba::new(0.1, 0.2, 0.3, 1.0));
        let decoy = device
            .create_texture(&TextureDesc {
                label: "decoy",
                width: 16,
                height: 16,
                format: TextureFormat::Rgba8Unorm,
                usage: TextureUsage::RENDER_TARGET,
            })
            .map_err(|e| e.to_string())?;
        let (surface_of_frame, _, encoder) = frame.into_parts();
        let tampered = Frame::new(
            surface_of_frame,
            decoy,
            TextureFormat::Rgba8Unorm,
            (16, 16),
            encoder,
        );
        device
            .end_frame(tampered)
            .map_err(|e| format!("end_frame of a tampered frame: {e}"))?;
        device.destroy_texture(decoy).map_err(|e| e.to_string())?;

        // 5: a dropped frame is recovered.
        let mut dropped = self.frame(surface)?;
        Self::clear(&mut dropped, LinearRgba::new(0.0, 1.0, 0.0, 1.0));
        drop(dropped);
        let mut recovered = self.frame(surface)?;
        Self::clear(&mut recovered, LinearRgba::new(0.1, 0.2, 0.3, 1.0));
        device
            .end_frame(recovered)
            .map_err(|e| format!("end_frame after recovering: {e}"))?;
        info!("all frame protocol checks passed");
        Ok(())
    }

    fn fail(&mut self, ctx: &Context<'_>, message: String) {
        if self.error.is_none() {
            self.error = Some(message);
        }
        ctx.exit();
    }

    fn draw(&mut self, ctx: &Context<'_>) {
        let (Some(window), Some(surface)) = (self.window.clone(), self.surface) else {
            return;
        };
        if !self.checks_done {
            self.checks_done = true;
            if let Err(e) = self.run_checks(surface) {
                return self.fail(ctx, e);
            }
        }
        // 6: normal frames.
        let mut frame = match self.device.begin_frame(surface) {
            Ok(Some(frame)) => frame,
            Ok(None) => return,
            Err(e) => return self.fail(ctx, e.to_string()),
        };
        Self::clear(&mut frame, LinearRgba::new(0.1, 0.2, 0.3, 1.0));
        window.pre_present_notify();
        if let Err(e) = self.device.end_frame(frame) {
            return self.fail(ctx, e.to_string());
        }
        self.normal_frames += 1;
        if self.normal_frames >= NORMAL_FRAMES {
            ctx.exit();
        }
    }
}

impl App for ProtocolApp {
    fn resumed(&mut self, ctx: &Context<'_>) {
        if self.window.is_some() {
            return;
        }
        let window = match ctx.create_window(&WindowDesc {
            title: "Oxreal: frame protocol checks".to_owned(),
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
        if self.started.elapsed() > TIMEOUT {
            return self.fail(ctx, format!("timed out after {TIMEOUT:?}"));
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn exiting(&mut self) {
        if let Some(surface) = self.surface.take()
            && let Err(e) = self.device.destroy_surface(surface)
            && self.error.is_none()
        {
            self.error = Some(format!("destroy_surface: {e}"));
        }
        self.window = None;
    }
}

fn main() -> std::process::ExitCode {
    oxreal_diagnostics::run_application(run)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let device = oxreal_rhi_vulkan::create_device(&DeviceDesc {
        app_name: "oxreal frame protocol".to_owned(),
        validation: Validation::Enabled,
        ..DeviceDesc::default()
    })?;
    let app = ProtocolApp {
        device,
        window: None,
        surface: None,
        checks_done: false,
        normal_frames: 0,
        started: Instant::now(),
        error: None,
    };
    let app = EventLoop::new()?.run(app)?;
    if let Some(error) = app.error {
        return Err(error.into());
    }
    if !app.checks_done || app.normal_frames < NORMAL_FRAMES {
        return Err(format!(
            "window closed early: checks done = {}, {} of {NORMAL_FRAMES} normal frames",
            app.checks_done, app.normal_frames
        )
        .into());
    }
    app.device.wait_idle()?;
    let errors = app.device.validation_error_count();
    if errors > 0 {
        return Err(format!("Vulkan validation reported {errors} error(s); see the log").into());
    }
    info!(
        normal_frames = app.normal_frames,
        "frame protocol checks passed with no validation errors"
    );
    Ok(())
}
