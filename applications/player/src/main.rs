//! The Oxreal player: the game executable (ADR-0019 §3). Layer: **app-runtime**.
//!
//! ```text
//! cargo xtask slang install                      # once: the pinned shader compiler
//! cargo xtask cook-shaders                       # after shader changes: writes target/debug/cooked
//! cargo run -p oxreal-player                     # until the window is closed
//! cargo run -p oxreal-player -- --frames 300     # exit after 300 frames (CI)
//! cargo run -p oxreal-player -- --help
//! ```
//!
//! It owns what a game process owns, and nothing else does:
//!
//! - the platform event loop and the window (`oxreal-window`);
//! - the device: this is the only place the Vulkan backend is named. Everything else sees an
//!   `Arc<dyn Device>`;
//! - the surface and the frame loop. While the window is minimized (zero size) no frames can be
//!   shown, so the loop sleeps until the next event instead of spinning, except under
//!   `--frames`, whose timeout must keep running;
//! - input, fed into the scene's `InputState`, and cursor capture while looking around;
//! - frame timing;
//! - the ECS world and schedule (`scene.rs`), with `oxreal-hierarchy` and
//!   `oxreal-render-bridge`;
//! - the renderer, fed the extracted `RenderWorld` each frame.
//!
//! **No shader compiler is linked in.** Shaders come from the cooked directory
//! (`cooked.rs`), checked against the sources the renderer was built for. A missing or stale
//! cache is a clear startup error that says to run `cargo xtask cook-shaders`.
//!
//! Controls (`fly_camera.rs`): hold the **right mouse button** and move the mouse to look
//! around, use `W` `A` `S` `D` to move and `E` / `Q` to rise and sink, and turn the wheel to
//! change speed. Holding the button captures the cursor.
//!
//! It fails on any Vulkan validation error, and with `--frames`, if the frames don't arrive
//! before `--timeout-secs`.

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Instant;

use oxreal_diagnostics::{info, warn};
use oxreal_ecs::{Schedule, World};
use oxreal_input::InputState;
use oxreal_render_bridge::ExtractedWorld;
use oxreal_renderer::{CookedShaders, MeshData, RenderTarget, Renderer};
use oxreal_rhi::{Device, DeviceDesc, Frame, PresentMode, SurfaceDesc, SurfaceHandle};
use oxreal_window::{
    App, Context, DeviceEvent, EventLoop, Window, WindowDesc, WindowEvent, WindowId,
};

mod cooked;
mod error;
mod fly_camera;
mod options;
mod scene;

use cooked::{COOKED_DIR_ENV_VAR, resolve_cooked_root};
use error::PlayerError;
use fly_camera::{FlyCamera, LOOK_BUTTON};
use options::{Command, Options};
use scene::FrameTime;

/// The longest step the scene takes in one frame, so a stall (a breakpoint, a dragged window,
/// a minimized one) doesn't throw the camera across the scene.
const MAX_FRAME_SECONDS: f32 = 0.1;

struct Player {
    device: Arc<dyn Device>,
    /// `None` only after [`App::exiting`], which drops it before the surface.
    renderer: Option<Renderer>,
    options: Options,
    window: Option<Arc<Window>>,
    surface: Option<SurfaceHandle>,
    /// The scene: entities, the frame's input and time, and the extracted render world.
    world: World,
    /// The scene's systems, run once per frame.
    schedule: Schedule,
    /// Whether the cursor is captured for looking around.
    cursor_captured: bool,
    /// Whether the window has zero size (minimized), so no frame can be shown.
    minimized: bool,
    frames: u64,
    started: Instant,
    /// When the scene last updated, to scale movement by the frame's duration.
    last_update: Instant,
    error: Option<PlayerError>,
}

impl Player {
    /// Records the first error and ends the event loop.
    fn fail(&mut self, ctx: &Context<'_>, error: PlayerError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
        ctx.exit();
    }

    /// Ends `frame` without drawing, then fails: a frame must always be ended, or its swapchain
    /// image stays acquired.
    fn abandon_frame(&mut self, ctx: &Context<'_>, frame: Frame, error: PlayerError) {
        if let Err(end) = self.device.end_frame(frame) {
            warn!(error = %end, "ending the frame after an error failed");
        }
        self.fail(ctx, error);
    }

    /// Runs one frame of the scene: records the frame time, runs the schedule (gameplay,
    /// transform propagation, then extraction), captures the cursor while the look button is
    /// held, and ends the input frame.
    fn update(&mut self) -> Result<(), PlayerError> {
        let now = Instant::now();
        let seconds = (now - self.last_update)
            .as_secs_f32()
            .min(MAX_FRAME_SECONDS);
        self.last_update = now;
        self.world.insert_resource(FrameTime(seconds));
        self.schedule.run(&mut self.world)?;

        let Some(input) = self.world.resource::<InputState>() else {
            return Err(PlayerError::MissingResource("InputState"));
        };
        // Only while the button is still down: a press and a release (or focus loss) in the same
        // frame must not leave a capture behind.
        let capture = input.mouse_just_pressed(LOOK_BUTTON) && input.mouse_held(LOOK_BUTTON);
        // Also true when focus was lost while the button was held (ADR-0018 §2).
        let release = input.mouse_just_released(LOOK_BUTTON);
        let speed_changed = input.scroll_lines().y != 0.0 && input.mouse_held(LOOK_BUTTON);
        if capture {
            self.set_cursor_captured(true);
        }
        if release {
            self.set_cursor_captured(false);
        }
        if speed_changed && let Some(camera) = self.world.query::<&FlyCamera>().next() {
            info!(
                metres_per_second = format_args!("{:.2}", camera.speed()),
                "camera speed"
            );
        }
        if let Some(mut input) = self.world.resource_mut::<InputState>() {
            input.end_frame();
        }
        Ok(())
    }

    /// Captures or releases the cursor. Failing to capture isn't fatal: looking around uses raw
    /// mouse motion, which arrives either way, so the cursor just stays visible.
    fn set_cursor_captured(&mut self, captured: bool) {
        let Some(window) = &self.window else {
            return;
        };
        if captured == self.cursor_captured {
            return;
        }
        if captured {
            match window.capture_cursor() {
                Ok(capture) => {
                    info!(?capture, "cursor captured");
                    self.cursor_captured = true;
                }
                Err(e) => warn!(error = %e, "cursor not captured; looking around still works"),
            }
        } else {
            // Counts as released even on failure: the cursor is shown again either way.
            if let Err(e) = window.release_cursor() {
                warn!(error = %e, "cursor release failed");
            }
            self.cursor_captured = false;
        }
    }

    /// Notes whether the window is minimized. While it is, the loop sleeps until the next event
    /// (a restore arrives as a resize), unless `--frames` needs its timeout checked.
    fn set_minimized(&mut self, ctx: &Context<'_>, minimized: bool) {
        if minimized == self.minimized {
            return;
        }
        self.minimized = minimized;
        info!(minimized, "window visibility changed");
        ctx.set_continuous(!minimized || self.options.frames.is_some());
        if !minimized && let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn draw(&mut self, ctx: &Context<'_>) {
        let (Some(window), Some(surface)) = (self.window.clone(), self.surface) else {
            return;
        };
        // The renderer goes only in `exiting`, after which nothing is drawn.
        if self.renderer.is_none() {
            return;
        }
        // The scene updates even when the frame is skipped below, so input never piles up.
        if let Err(e) = self.update() {
            return self.fail(ctx, e);
        }
        let mut frame = match self.device.begin_frame(surface) {
            Ok(Some(frame)) => frame,
            Ok(None) => return, // Minimized or recreating: skip this frame.
            Err(e) => return self.fail(ctx, PlayerError::device("beginning a frame")(e)),
        };
        let target = RenderTarget::backbuffer(&frame);
        let rendered = match (
            self.renderer.as_mut(),
            self.world.resource::<ExtractedWorld>(),
        ) {
            // The renderer reads only the extracted render world, never the scene.
            (Some(renderer), Some(extracted)) => renderer
                .render(frame.encoder(), &target, extracted.render_world())
                .map(|_| ())
                .map_err(PlayerError::render("rendering the frame")),
            (_, None) => Err(PlayerError::MissingResource("ExtractedWorld")),
            (None, Some(_)) => unreachable!("checked at the start of `draw`"),
        };
        if let Err(e) = rendered {
            return self.abandon_frame(ctx, frame, e);
        }
        window.pre_present_notify();
        if let Err(e) = self.device.end_frame(frame) {
            return self.fail(ctx, PlayerError::device("presenting a frame")(e));
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

impl App for Player {
    fn resumed(&mut self, ctx: &Context<'_>) {
        if self.window.is_some() {
            return;
        }
        let window = match ctx.create_window(&WindowDesc {
            title: "Oxreal".to_owned(),
            ..WindowDesc::default()
        }) {
            Ok(window) => Arc::new(window),
            Err(e) => return self.fail(ctx, e.into()),
        };
        let size = window.inner_size();
        let desc = SurfaceDesc {
            width: size.width,
            height: size.height,
            present_mode: PresentMode::Fifo,
        };
        match self.device.create_surface(window.clone(), &desc) {
            Ok(surface) => self.surface = Some(surface),
            Err(e) => return self.fail(ctx, PlayerError::device("creating the surface")(e)),
        }
        info!(display_server = ?ctx.display_server(), ?size, "window ready");
        ctx.set_continuous(true);
        self.window = Some(window);
    }

    fn window_event(&mut self, ctx: &Context<'_>, _window: WindowId, event: WindowEvent) {
        if let Some(mut input) = self.world.resource_mut::<InputState>() {
            input.handle_window_event(&event);
        }
        match event {
            // Give the cursor back at once: no frame may come while the window is in the
            // background.
            WindowEvent::Focused(false) => self.set_cursor_captured(false),
            WindowEvent::CloseRequested => ctx.exit(),
            WindowEvent::Resized(size) => {
                self.set_minimized(ctx, size.width == 0 || size.height == 0);
                if let Some(surface) = self.surface
                    && let Err(e) = self.device.resize_surface(surface, size.width, size.height)
                {
                    self.fail(ctx, PlayerError::device("resizing the surface")(e));
                }
            }
            WindowEvent::RedrawRequested => self.draw(ctx),
            _ => {}
        }
    }

    fn device_event(&mut self, _ctx: &Context<'_>, event: DeviceEvent) {
        if let Some(mut input) = self.world.resource_mut::<InputState>() {
            input.handle_device_event(&event);
        }
    }

    fn about_to_wait(&mut self, ctx: &Context<'_>) {
        if self.options.frames.is_some() && self.started.elapsed() > self.options.timeout {
            let error = PlayerError::TimedOut {
                frames: self.frames,
                timeout: self.options.timeout,
            };
            return self.fail(ctx, error);
        }
        if !self.minimized
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn exiting(&mut self) {
        // The renderer's GPU objects go first; their destruction waits for recorded work.
        self.renderer = None;
        // The surface goes before the window it presents to.
        if let Some(surface) = self.surface.take()
            && let Err(e) = self.device.destroy_surface(surface)
        {
            warn!(error = %e, "destroying the surface failed");
        }
        self.window = None;
    }
}

fn main() -> std::process::ExitCode {
    oxreal_diagnostics::run_application(|| run().map_err(Into::into))
}

#[expect(
    clippy::print_stdout,
    reason = "usage text is the output a user asked for with --help"
)]
fn print_usage() {
    println!("{}", options::USAGE);
}

fn run() -> Result<(), PlayerError> {
    let options = match options::parse(std::env::args_os().skip(1)) {
        Ok(Command::Run(options)) => options,
        Ok(Command::Help) => {
            print_usage();
            return Ok(());
        }
        Err(source) => return Err(PlayerError::Options { source }),
    };

    let environment: Option<OsString> = std::env::var_os(COOKED_DIR_ENV_VAR);
    let (root, from) = resolve_cooked_root(
        options.cooked.as_deref(),
        environment.as_deref(),
        std::env::current_exe,
    )?;
    info!(root = %root.display(), %from, "cooked data");
    let shaders = match CookedShaders::load(&root) {
        Ok(shaders) => shaders,
        Err(source) => return Err(PlayerError::CookedShaders { root, from, source }),
    };

    let device: Arc<dyn Device> = Arc::new(
        oxreal_rhi_vulkan::create_device(&DeviceDesc {
            app_name: "oxreal player".to_owned(),
            ..DeviceDesc::default()
        })
        .map_err(PlayerError::device("creating the device"))?,
    );
    let mut renderer = Renderer::new(Arc::clone(&device), shaders)
        .map_err(PlayerError::render("creating the renderer"))?;
    let cube = renderer
        .register_mesh(&MeshData::cube())
        .map_err(PlayerError::render("registering the cube mesh"))?;
    let mut schedule = scene::schedule();
    // Check the systems' ordering now, so a mistake fails before a window opens.
    schedule.validate()?;
    let player = Player {
        device,
        renderer: Some(renderer),
        options,
        window: None,
        surface: None,
        world: scene::world(cube),
        schedule,
        cursor_captured: false,
        minimized: false,
        frames: 0,
        started: Instant::now(),
        last_update: Instant::now(),
        error: None,
    };

    let player = EventLoop::new()?.run(player)?;
    let elapsed = player.started.elapsed();
    let errors = player.device.validation_error_count();
    if let Some(error) = player.error {
        return Err(error);
    }
    if errors > 0 {
        return Err(PlayerError::Validation(errors));
    }
    if let Some(target) = player.options.frames
        && player.frames < target
    {
        return Err(PlayerError::ClosedEarly {
            frames: player.frames,
            target,
        });
    }
    info!(
        frames = player.frames,
        seconds = format_args!("{:.2}", elapsed.as_secs_f32()),
        validation_errors = errors,
        "player finished"
    );
    Ok(())
}
