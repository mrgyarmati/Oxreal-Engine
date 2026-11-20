//! Windows and the platform event loop ([ADR-0015]).
//!
//! **Layer:** platform. Wraps `winit` behind an engine-owned API. No `winit` type appears in
//! this crate's public interface. The one interop boundary is [`raw_window_handle`]:
//! [`Window`] implements `HasWindowHandle` and `HasDisplayHandle`, so the renderer can create a
//! surface without depending on this crate's backend.
//!
//! # Model
//!
//! - Implement [`App`] and pass it to [`EventLoop::run`] on the **main thread**.
//! - Create windows in [`App::resumed`] with [`Context::create_window`].
//! - Sizes are **physical pixels**. The scale factor is reported separately
//!   ([`WindowEvent::ScaleFactorChanged`]).
//! - A minimized window can be 0×0. That's a normal state, not an error ([`PhysicalSize::is_zero`]).
//!
//! # Input ([ADR-0018])
//!
//! - Keyboard and mouse arrive as [`WindowEvent`]s. Keys are [`KeyCode`]s, named by physical
//!   position, so bindings don't move with the keyboard layout.
//! - Raw mouse motion, for mouse-look, arrives through [`App::device_event`], only while a window
//!   has focus.
//! - [`Window::capture_cursor`] hides the cursor and locks it, or confines it on X11.
//! - Applications normally feed these events to `oxreal-input`, which keeps the frame's input
//!   state.
//!
//! On Linux, `OXREAL_LINUX_DISPLAY=wayland|x11` forces a display server. Otherwise Wayland is
//! used when `WAYLAND_DISPLAY` is set, and X11 when it isn't.
//!
//! [ADR-0015]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0015-windowing-and-platform-event-loop.md
//! [ADR-0018]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0018-input-events-state-and-routing.md

mod display;
mod error;
mod event;
mod event_loop;
mod input;
mod window;

pub use display::{DISPLAY_OVERRIDE_ENV_VAR, DisplayServer};
pub use error::WindowError;
pub use event::{PhysicalSize, WindowEvent};
pub use event_loop::{App, Context, EventLoop};
pub use input::{ButtonState, DeviceEvent, KeyCode, KeyEvent, MouseButton, ScrollDelta};
pub use window::{CursorCapture, Window, WindowDesc, WindowId};

/// Re-export of the interop crate whose traits [`Window`] implements.
pub use raw_window_handle;
