//! Windows.

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};

use crate::PhysicalSize;

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
