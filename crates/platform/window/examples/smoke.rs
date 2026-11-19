//! Opens a window and runs a frame loop: the `oxreal-window` smoke test.
//!
//! ```text
//! cargo run -p oxreal-window --example smoke                  # until the window is closed
//! cargo run -p oxreal-window --example smoke -- --frames 120  # exit after 120 frames (CI)
//! ```
//!
//! With `--frames`, the run fails if the frames don't arrive within `--timeout-secs` (default
//! 30), so a stuck event loop fails CI instead of hanging it.

use std::time::{Duration, Instant};

use oxreal_diagnostics::{info, warn};
use oxreal_window::{App, Context, EventLoop, Window, WindowDesc, WindowEvent, WindowId};

struct Smoke {
    window: Option<Window>,
    frames: u64,
    target_frames: Option<u64>,
    deadline: Option<Instant>,
    started: Instant,
    error: Option<String>,
}

impl Smoke {
    fn fail(&mut self, ctx: &Context<'_>, message: String) {
        self.error = Some(message);
        ctx.exit();
    }
}

impl App for Smoke {
    fn resumed(&mut self, ctx: &Context<'_>) {
        if self.window.is_some() {
            return;
        }
        let desc = WindowDesc {
            title: "Oxreal window smoke test".to_owned(),
            ..WindowDesc::default()
        };
        match ctx.create_window(&desc) {
            Ok(window) => {
                info!(
                    display_server = ?ctx.display_server(),
                    size = ?window.inner_size(),
                    scale_factor = window.scale_factor(),
                    "window created"
                );
                ctx.set_continuous(true);
                self.window = Some(window);
            }
            Err(e) => self.fail(ctx, e.to_string()),
        }
    }

    fn window_event(&mut self, ctx: &Context<'_>, _window: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                info!("close requested");
                ctx.exit();
            }
            WindowEvent::Resized(size) => info!(?size, "resized"),
            WindowEvent::ScaleFactorChanged { scale_factor } => {
                info!(scale_factor, "scale factor changed");
            }
            WindowEvent::Focused(focused) => info!(focused, "focus changed"),
            WindowEvent::RedrawRequested => {
                if let Some(window) = &self.window {
                    window.pre_present_notify();
                }
                self.frames += 1;
                if self
                    .target_frames
                    .is_some_and(|target| self.frames >= target)
                {
                    ctx.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, ctx: &Context<'_>) {
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() > deadline)
        {
            let message = format!(
                "timed out after {} frame(s); expected {} (is the window being drawn?)",
                self.frames,
                self.target_frames.unwrap_or_default()
            );
            self.fail(ctx, message);
            return;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn exiting(&mut self) {
        // Close the window while the platform is still alive.
        self.window = None;
    }
}

fn parse_args() -> Result<(Option<u64>, Duration), String> {
    let mut frames = None;
    let mut timeout = Duration::from_secs(30);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or_else(|| format!("{name} needs a whole number"))
        };
        match arg.as_str() {
            "--frames" => frames = Some(value("--frames")?),
            "--timeout-secs" => timeout = Duration::from_secs(value("--timeout-secs")?),
            other => {
                return Err(format!(
                    "unknown argument {other:?}; usage: smoke [--frames N] [--timeout-secs S]"
                ));
            }
        }
    }
    Ok((frames, timeout))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    oxreal_diagnostics::init_application_logging()?;
    let (target_frames, timeout) = parse_args()?;
    let started = Instant::now();
    let app = Smoke {
        window: None,
        frames: 0,
        target_frames,
        deadline: target_frames.map(|_| started + timeout),
        started,
        error: None,
    };

    let app = EventLoop::new()?.run(app)?;
    let elapsed = app.started.elapsed();
    if let Some(error) = app.error {
        return Err(error.into());
    }
    if let Some(target) = app.target_frames
        && app.frames < target
    {
        warn!(frames = app.frames, target, "event loop ended early");
        return Err(format!("window closed after {} of {target} frames", app.frames).into());
    }
    info!(
        frames = app.frames,
        elapsed_ms = elapsed.as_millis(),
        "smoke test finished"
    );
    Ok(())
}
