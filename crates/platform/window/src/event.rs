//! Engine-owned window events (ADR-0015 §3).

/// A size in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PhysicalSize {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

impl PhysicalSize {
    /// A size from width and height in physical pixels.
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// `true` when either dimension is zero, for example while a window is minimized. Renderers
    /// must skip presenting rather than create a zero-sized swapchain.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Something that happened to a window.
///
/// Keyboard, mouse and text input aren't here. They arrive with the input crate and are routed
/// through the ECS `Input` stage (ADR-0014).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum WindowEvent {
    /// The user or the system asked to close the window. The application decides whether to.
    CloseRequested,
    /// The drawable area changed size, in physical pixels. It may be zero while minimized.
    Resized(PhysicalSize),
    /// The window's scale factor changed, for example after moving it to a monitor with a
    /// different DPI. UI should re-layout. A `Resized` event follows when the physical size
    /// changes as a result.
    ScaleFactorChanged {
        /// New scale factor (physical pixels per logical pixel).
        scale_factor: f64,
    },
    /// The window gained (`true`) or lost (`false`) keyboard focus.
    Focused(bool),
    /// The window should be drawn now. Request this with [`crate::Window::request_redraw`].
    RedrawRequested,
}

/// Maps a winit event to an engine event. Events the engine doesn't model yet are dropped.
pub(crate) fn from_winit(event: &winit::event::WindowEvent) -> Option<WindowEvent> {
    use winit::event::WindowEvent as W;
    Some(match event {
        W::CloseRequested => WindowEvent::CloseRequested,
        W::Resized(size) => WindowEvent::Resized(PhysicalSize::new(size.width, size.height)),
        W::ScaleFactorChanged { scale_factor, .. } => WindowEvent::ScaleFactorChanged {
            scale_factor: *scale_factor,
        },
        W::Focused(focused) => WindowEvent::Focused(*focused),
        W::RedrawRequested => WindowEvent::RedrawRequested,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::WindowEvent as W;

    #[test]
    fn maps_modelled_events() {
        assert_eq!(
            from_winit(&W::CloseRequested),
            Some(WindowEvent::CloseRequested)
        );
        assert_eq!(
            from_winit(&W::Resized(winit::dpi::PhysicalSize::new(1920, 1080))),
            Some(WindowEvent::Resized(PhysicalSize::new(1920, 1080)))
        );
        assert_eq!(
            from_winit(&W::Focused(false)),
            Some(WindowEvent::Focused(false))
        );
        assert_eq!(
            from_winit(&W::RedrawRequested),
            Some(WindowEvent::RedrawRequested)
        );
    }

    #[test]
    fn drops_events_the_engine_does_not_model_yet() {
        assert_eq!(from_winit(&W::Destroyed), None);
        assert_eq!(from_winit(&W::Occluded(true)), None);
    }

    #[test]
    fn zero_sizes_are_detected() {
        assert!(PhysicalSize::new(0, 720).is_zero());
        assert!(PhysicalSize::new(1280, 0).is_zero());
        assert!(!PhysicalSize::new(1, 1).is_zero());
        assert!(PhysicalSize::default().is_zero());
    }
}
