//! Engine-owned window events (ADR-0015 §3, ADR-0018 §1).

use std::hash::Hash;

use crate::input::{self, ButtonState, InputFilter, KeyEvent, MouseButton, ScrollDelta};

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
/// Keyboard and mouse events are raw device input (ADR-0018 §1). Applications normally feed them
/// to `oxreal-input`, which keeps the frame's input state, rather than acting on them directly.
/// Text and IME input arrive later, with UI.
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
    /// A key went down, came up or repeated while this window had keyboard focus.
    Key(KeyEvent),
    /// A mouse button went down or came up over this window, or anywhere while it holds the
    /// pointer.
    MouseButton {
        /// Which button.
        button: MouseButton,
        /// Whether it went down or came up.
        state: ButtonState,
    },
    /// The cursor moved over the drawable area. Physical pixels from its **top-left** corner,
    /// y down (window coordinates, not world coordinates).
    CursorMoved {
        /// Pixels from the left edge.
        x: f64,
        /// Pixels from the top edge.
        y: f64,
    },
    /// The cursor left the drawable area.
    CursorLeft,
    /// A wheel or touchpad scrolled over this window.
    Scroll(ScrollDelta),
}

/// Maps a winit event for `window` to an engine event, through `filter` (ADR-0018 §1). Events the
/// engine doesn't model yet are dropped.
pub(crate) fn from_winit<Id: Eq + Hash>(
    event: &winit::event::WindowEvent,
    window: Id,
    filter: &mut InputFilter<Id>,
) -> Option<WindowEvent> {
    use winit::event::WindowEvent as W;
    Some(match event {
        W::CloseRequested => WindowEvent::CloseRequested,
        W::Resized(size) => WindowEvent::Resized(PhysicalSize::new(size.width, size.height)),
        W::ScaleFactorChanged { scale_factor, .. } => WindowEvent::ScaleFactorChanged {
            scale_factor: *scale_factor,
        },
        W::Focused(focused) => {
            filter.focus(window, *focused);
            WindowEvent::Focused(*focused)
        }
        W::Destroyed => {
            filter.destroyed(&window);
            return None;
        }
        W::RedrawRequested => WindowEvent::RedrawRequested,
        W::KeyboardInput {
            event,
            is_synthetic,
            ..
        } => WindowEvent::Key(filter.key(
            event.physical_key,
            event.state,
            event.repeat,
            *is_synthetic,
        )?),
        W::MouseInput { state, button, .. } => WindowEvent::MouseButton {
            button: input::mouse_button_from_winit(*button),
            state: input::button_state_from_winit(*state),
        },
        W::CursorMoved { position, .. } => WindowEvent::CursorMoved {
            x: position.x,
            y: position.y,
        },
        W::CursorLeft { .. } => WindowEvent::CursorLeft,
        W::MouseWheel { delta, .. } => WindowEvent::Scroll(input::scroll_from_winit(*delta)),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::WindowEvent as W;

    /// Maps one event for window 1 through a fresh filter.
    fn map(event: &W) -> Option<WindowEvent> {
        from_winit(event, 1_u32, &mut InputFilter::default())
    }

    #[test]
    fn maps_modelled_events() {
        assert_eq!(map(&W::CloseRequested), Some(WindowEvent::CloseRequested));
        assert_eq!(
            map(&W::Resized(winit::dpi::PhysicalSize::new(1920, 1080))),
            Some(WindowEvent::Resized(PhysicalSize::new(1920, 1080)))
        );
        assert_eq!(map(&W::Focused(false)), Some(WindowEvent::Focused(false)));
        assert_eq!(map(&W::RedrawRequested), Some(WindowEvent::RedrawRequested));
    }

    #[test]
    fn maps_mouse_events() {
        use winit::dpi::PhysicalPosition;
        use winit::event::{DeviceId, ElementState, MouseScrollDelta, TouchPhase};
        let device_id = DeviceId::dummy();
        assert_eq!(
            map(&W::MouseInput {
                device_id,
                state: ElementState::Pressed,
                button: winit::event::MouseButton::Right,
            }),
            Some(WindowEvent::MouseButton {
                button: MouseButton::Right,
                state: ButtonState::Pressed,
            })
        );
        assert_eq!(
            map(&W::CursorMoved {
                device_id,
                position: PhysicalPosition::new(12.5, 300.0),
            }),
            Some(WindowEvent::CursorMoved { x: 12.5, y: 300.0 })
        );
        assert_eq!(
            map(&W::CursorLeft { device_id }),
            Some(WindowEvent::CursorLeft)
        );
        assert_eq!(
            map(&W::MouseWheel {
                device_id,
                delta: MouseScrollDelta::LineDelta(0.0, -2.0),
                phase: TouchPhase::Moved,
            }),
            Some(WindowEvent::Scroll(ScrollDelta::Lines { x: 0.0, y: -2.0 }))
        );
    }

    #[test]
    fn drops_events_the_engine_does_not_model_yet() {
        assert_eq!(map(&W::Destroyed), None);
        assert_eq!(map(&W::Occluded(true)), None);
    }

    #[test]
    fn zero_sizes_are_detected() {
        assert!(PhysicalSize::new(0, 720).is_zero());
        assert!(PhysicalSize::new(1280, 0).is_zero());
        assert!(!PhysicalSize::new(1, 1).is_zero());
        assert!(PhysicalSize::default().is_zero());
    }
}
