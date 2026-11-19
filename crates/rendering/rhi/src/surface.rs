//! Presentation surfaces (ADR-0017 §5–§6).

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

/// A window a device can present to.
///
/// Implemented automatically for anything with raw window and display handles that is
/// thread-safe, for example `oxreal_window::Window`. Surfaces hold the window in an `Arc`, so a
/// surface can never outlive its window.
pub trait SurfaceTarget: HasWindowHandle + HasDisplayHandle + Send + Sync {}

impl<T: HasWindowHandle + HasDisplayHandle + Send + Sync> SurfaceTarget for T {}

/// How frames are presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PresentMode {
    /// Wait for vertical blank. No tearing, always available. The default.
    #[default]
    Fifo,
    /// Low latency without tearing: the newest frame replaces a waiting one. Falls back to
    /// `Fifo` (logged) where unsupported.
    Mailbox,
    /// Present immediately. May tear. Falls back to `Fifo` (logged) where unsupported.
    Immediate,
}

/// How to create a surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceDesc {
    /// Drawable width in physical pixels. Usually the window's inner size.
    pub width: u32,
    /// Drawable height in physical pixels.
    pub height: u32,
    /// Presentation mode.
    pub present_mode: PresentMode,
}
