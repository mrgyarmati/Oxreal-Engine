//! The platform event loop and the engine-owned [`App`] trait (ADR-0015 §3–§5).

use oxreal_diagnostics::info;
use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow};

use crate::{
    DeviceEvent, DisplayServer, Window, WindowDesc, WindowError, WindowEvent, WindowId, event,
    input,
};

/// An application driven by the platform event loop.
///
/// The callbacks can't fail. An application that hits an error stores it, calls
/// [`Context::exit`], and reports it after [`EventLoop::run`] returns.
pub trait App {
    /// The application may create windows now. Called at startup, and again after
    /// [`App::suspended`] on platforms that suspend.
    fn resumed(&mut self, ctx: &Context<'_>);

    /// Something happened to one of the application's windows.
    fn window_event(&mut self, ctx: &Context<'_>, window: WindowId, event: WindowEvent);

    /// Raw input from a device rather than a window, such as unaccelerated mouse motion for
    /// mouse-look (ADR-0018 §1). Delivered only while one of the application's windows has
    /// focus.
    fn device_event(&mut self, ctx: &Context<'_>, event: DeviceEvent) {
        let _ = (ctx, event);
    }

    /// All pending events have been delivered. A good place to drive the next frame, for example
    /// by calling [`Window::request_redraw`].
    fn about_to_wait(&mut self, ctx: &Context<'_>) {
        let _ = ctx;
    }

    /// The application must release GPU surfaces. They're recreated in [`App::resumed`]. Only
    /// some platforms suspend.
    fn suspended(&mut self, ctx: &Context<'_>) {
        let _ = ctx;
    }

    /// The event loop is ending. Last chance to clean up while the platform is still alive.
    fn exiting(&mut self) {}
}

/// Access to the running event loop, passed to every [`App`] callback.
#[derive(Debug)]
pub struct Context<'a> {
    event_loop: &'a ActiveEventLoop,
}

impl Context<'_> {
    /// Creates a window.
    ///
    /// # Errors
    ///
    /// [`WindowError::CreateWindow`] if the operating system refuses.
    pub fn create_window(&self, desc: &WindowDesc) -> Result<Window, WindowError> {
        let attributes = winit::window::Window::default_attributes()
            .with_title(desc.title.clone())
            .with_inner_size(winit::dpi::LogicalSize::new(desc.width, desc.height))
            .with_visible(desc.visible);
        self.event_loop
            .create_window(attributes)
            .map(Window::new)
            .map_err(|e| WindowError::CreateWindow {
                title: desc.title.clone(),
                source: Box::new(e),
            })
    }

    /// Ends the event loop after the current callback. [`App::exiting`] runs next.
    pub fn exit(&self) {
        self.event_loop.exit();
    }

    /// `true` to run continuously (games, editors with animation), or `false` to sleep until the
    /// next event (idle tools). The default is `false`.
    pub fn set_continuous(&self, continuous: bool) {
        self.event_loop.set_control_flow(if continuous {
            ControlFlow::Poll
        } else {
            ControlFlow::Wait
        });
    }

    /// The windowing system this event loop is connected to.
    #[must_use]
    pub fn display_server(&self) -> DisplayServer {
        active_display_server(self.event_loop)
    }
}

/// The platform event loop. Create it once, on the main thread.
#[derive(Debug)]
pub struct EventLoop {
    inner: winit::event_loop::EventLoop<()>,
}

impl EventLoop {
    /// Connects to the platform's windowing system.
    ///
    /// On Linux, `OXREAL_LINUX_DISPLAY=wayland|x11` forces a display server. Otherwise Wayland is
    /// used when available, and X11 when it isn't.
    ///
    /// # Errors
    ///
    /// - [`WindowError::NoDisplay`] (Linux) when no display server is configured.
    /// - [`WindowError::DisplayUnavailable`] or [`WindowError::InvalidDisplayOverride`] for a bad
    ///   override.
    /// - [`WindowError::EventLoop`] if the platform refuses, for example when called off the main
    ///   thread or twice.
    pub fn new() -> Result<Self, WindowError> {
        let inner = build_platform_event_loop()?;
        // ADR-0018 §1: device input (raw mouse motion) only while focused. This is winit's
        // default, stated so an upgrade can't change it silently. Wayland ignores it, so the
        // adapter also drops device events while no window has focus.
        inner.listen_device_events(winit::event_loop::DeviceEvents::WhenFocused);
        let event_loop = Self { inner };
        info!(display_server = ?event_loop.display_server(), "platform event loop created");
        Ok(event_loop)
    }

    /// The windowing system this event loop is connected to.
    #[must_use]
    pub fn display_server(&self) -> DisplayServer {
        loop_display_server(&self.inner)
    }

    /// Runs `app` until [`Context::exit`] is called. Returns the application so it can report
    /// what happened.
    ///
    /// # Errors
    ///
    /// [`WindowError::EventLoop`] if the platform event loop fails.
    pub fn run<A: App>(self, app: A) -> Result<A, WindowError> {
        let mut adapter = Adapter {
            app,
            input: input::InputFilter::default(),
        };
        self.inner
            .run_app(&mut adapter)
            .map_err(|e| WindowError::EventLoop {
                source: Box::new(e),
            })?;
        Ok(adapter.app)
    }
}

/// Bridges winit's handler to the engine's [`App`], translating events.
struct Adapter<A> {
    app: A,
    /// Evens out per-platform input differences (ADR-0018 §1).
    input: input::InputFilter<winit::window::WindowId>,
}

impl<A: App> ApplicationHandler for Adapter<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.app.resumed(&Context { event_loop });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        if let Some(event) = event::from_winit(&event, window_id, &mut self.input) {
            self.app
                .window_event(&Context { event_loop }, WindowId(window_id), event);
        }
    }

    fn device_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        // Wayland delivers raw motion whenever the pointer is over a window, focused or not.
        if !self.input.device_events_allowed() {
            return;
        }
        if let Some(event) = input::device_event_from_winit(&event) {
            self.app.device_event(&Context { event_loop }, event);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.app.about_to_wait(&Context { event_loop });
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.app.suspended(&Context { event_loop });
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.app.exiting();
    }
}

#[cfg(target_os = "linux")]
fn build_platform_event_loop() -> Result<winit::event_loop::EventLoop<()>, WindowError> {
    use crate::display::{LinuxBackendChoice, choose_linux_backend};
    use crate::{DISPLAY_OVERRIDE_ENV_VAR, display::DisplayServer as D};
    use winit::platform::wayland::EventLoopBuilderExtWayland as _;
    use winit::platform::x11::EventLoopBuilderExtX11 as _;

    let var = |name: &str| std::env::var(name).ok();
    let choice = choose_linux_backend(
        var(DISPLAY_OVERRIDE_ENV_VAR).as_deref(),
        var("WAYLAND_DISPLAY").as_deref(),
        var("DISPLAY").as_deref(),
    )?;
    let mut builder = winit::event_loop::EventLoop::builder();
    match choice {
        LinuxBackendChoice::Forced(D::Wayland) => {
            builder.with_wayland();
        }
        LinuxBackendChoice::Forced(D::X11) => {
            builder.with_x11();
        }
        LinuxBackendChoice::Forced(_) | LinuxBackendChoice::Automatic => {}
    }
    builder.build().map_err(|e| WindowError::EventLoop {
        source: Box::new(e),
    })
}

#[cfg(not(target_os = "linux"))]
fn build_platform_event_loop() -> Result<winit::event_loop::EventLoop<()>, WindowError> {
    winit::event_loop::EventLoop::new().map_err(|e| WindowError::EventLoop {
        source: Box::new(e),
    })
}

#[cfg(target_os = "linux")]
fn loop_display_server(event_loop: &winit::event_loop::EventLoop<()>) -> DisplayServer {
    use winit::platform::wayland::EventLoopExtWayland as _;
    if event_loop.is_wayland() {
        DisplayServer::Wayland
    } else {
        DisplayServer::X11
    }
}

#[cfg(target_os = "linux")]
fn active_display_server(event_loop: &ActiveEventLoop) -> DisplayServer {
    use winit::platform::wayland::ActiveEventLoopExtWayland as _;
    if event_loop.is_wayland() {
        DisplayServer::Wayland
    } else {
        DisplayServer::X11
    }
}

#[cfg(windows)]
fn loop_display_server(_event_loop: &winit::event_loop::EventLoop<()>) -> DisplayServer {
    DisplayServer::Windows
}

#[cfg(windows)]
fn active_display_server(_event_loop: &ActiveEventLoop) -> DisplayServer {
    DisplayServer::Windows
}

#[cfg(not(any(windows, target_os = "linux")))]
fn loop_display_server(_event_loop: &winit::event_loop::EventLoop<()>) -> DisplayServer {
    DisplayServer::Other
}

#[cfg(not(any(windows, target_os = "linux")))]
fn active_display_server(_event_loop: &ActiveEventLoop) -> DisplayServer {
    DisplayServer::Other
}
