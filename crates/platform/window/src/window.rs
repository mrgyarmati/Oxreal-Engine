//! Windows.

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};

use crate::{PhysicalSize, WindowError};

/// Identifies a window in [`crate::App::window_event`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub(crate) winit::window::WindowId);

/// How to create a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowDesc {
    /// Title bar text.
    pub title: String,
    /// Requested drawable width in **logical** pixels. It's scaled by the monitor's scale
    /// factor, so the window looks the same size on any DPI.
    pub width: u32,
    /// Requested drawable height in logical pixels.
    pub height: u32,
    /// Whether the window is shown immediately.
    pub visible: bool,
}

impl Default for WindowDesc {
    fn default() -> Self {
        Self {
            title: "Oxreal".to_owned(),
            width: 1280,
            height: 720,
            visible: true,
        }
    }
}

/// How [`Window::capture_cursor`] holds the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CursorCapture {
    /// Held in place (Windows, Wayland). It reappears where it was captured. On Wayland this
    /// means the lock was **requested**: the compositor applies it once the pointer is over the
    /// window and the window has focus, and may postpone it.
    Locked,
    /// Kept inside the window, but free to move there while hidden (X11). It reappears where it
    /// drifted to.
    Confined,
}

/// An open window. It closes when dropped.
#[derive(Debug)]
pub struct Window {
    inner: winit::window::Window,
}

impl Window {
    pub(crate) fn new(inner: winit::window::Window) -> Self {
        Self { inner }
    }

    /// This window's identifier, as passed to [`crate::App::window_event`].
    #[must_use]
    pub fn id(&self) -> WindowId {
        WindowId(self.inner.id())
    }

    /// The drawable area in physical pixels. It may be zero while minimized.
    #[must_use]
    pub fn inner_size(&self) -> PhysicalSize {
        let size = self.inner.inner_size();
        PhysicalSize::new(size.width, size.height)
    }

    /// Physical pixels per logical pixel on the window's current monitor.
    #[must_use]
    pub fn scale_factor(&self) -> f64 {
        self.inner.scale_factor()
    }

    /// Asks for a [`crate::WindowEvent::RedrawRequested`] event for this window.
    pub fn request_redraw(&self) {
        self.inner.request_redraw();
    }

    /// Call just before presenting a frame. Some platforms (Wayland) use it to pace frames
    /// correctly.
    pub fn pre_present_notify(&self) {
        self.inner.pre_present_notify();
    }

    /// Changes the title bar text.
    pub fn set_title(&self, title: &str) {
        self.inner.set_title(title);
    }

    /// Captures the cursor for mouse-look (ADR-0018 §1): hides it and locks it in place, or, where
    /// locking isn't supported (X11), confines it to the window. Raw mouse motion
    /// ([`crate::DeviceEvent::MouseMotion`]) keeps arriving either way.
    ///
    /// Release it with [`Window::release_cursor`], and also when the window loses focus. Call it
    /// from the event-loop thread (inside [`crate::App`] callbacks): on Windows it waits for that
    /// thread, so calling it from a thread the event loop is waiting on would deadlock.
    ///
    /// # Errors
    ///
    /// [`WindowError::CursorCapture`] if the platform refuses both modes, for example because
    /// another application holds the pointer.
    pub fn capture_cursor(&self) -> Result<CursorCapture, WindowError> {
        use winit::window::CursorGrabMode;
        let capture = match self.inner.set_cursor_grab(CursorGrabMode::Locked) {
            Ok(()) => CursorCapture::Locked,
            Err(locked) => match self.inner.set_cursor_grab(CursorGrabMode::Confined) {
                Ok(()) => CursorCapture::Confined,
                Err(confined) => {
                    return Err(WindowError::CursorCapture {
                        reason: format!("locking failed ({locked}), confining failed ({confined})"),
                    });
                }
            },
        };
        self.inner.set_cursor_visible(false);
        Ok(capture)
    }

    /// Shows the cursor again and lets it leave the window. Releasing a cursor that isn't
    /// captured does nothing. Like [`Window::capture_cursor`], call it from the event-loop thread.
    ///
    /// On Wayland, releasing while the pointer is outside the window takes effect when the
    /// pointer next enters it.
    ///
    /// # Errors
    ///
    /// [`WindowError::CursorCapture`] if the platform refuses to release the grab. The cursor
    /// is shown again regardless.
    pub fn release_cursor(&self) -> Result<(), WindowError> {
        self.inner.set_cursor_visible(true);
        self.inner
            .set_cursor_grab(winit::window::CursorGrabMode::None)
            .map_err(|e| WindowError::CursorCapture {
                reason: format!("releasing failed ({e})"),
            })
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.inner.window_handle()
    }
}

impl HasDisplayHandle for Window {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.inner.display_handle()
    }
}
