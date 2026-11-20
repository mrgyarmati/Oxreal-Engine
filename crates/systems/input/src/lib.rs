//! Input state built from platform events ([ADR-0018]).
//!
//! **Layer:** systems. [`InputState`] turns the keyboard and mouse events of `oxreal-window`
//! into what game code asks about: which keys and buttons are held, which went down or up this
//! frame, how far the mouse moved and how far the wheel scrolled.
//!
//! This crate doesn't read devices or talk to the operating system. That's `oxreal-window`'s
//! job. It doesn't route input between consumers yet either: input contexts with consumption
//! (ADR-0018 §3) arrive with the second consumer, runtime UI or the editor viewport.
//!
//! # Use
//!
//! 1. Pass every [`WindowEvent`] and [`DeviceEvent`] to [`InputState::handle_window_event`] and
//!    [`InputState::handle_device_event`] as it arrives.
//! 2. Read the state while running the frame.
//! 3. Call [`InputState::end_frame`] once the frame has read its input.
//!
//! ```
//! use oxreal_input::{ButtonState, InputState, KeyCode, KeyEvent};
//! use oxreal_window::WindowEvent;
//!
//! let mut input = InputState::new();
//! let press = KeyEvent { key: KeyCode::KeyW, state: ButtonState::Pressed, repeat: false };
//! input.handle_window_event(&WindowEvent::Key(press));
//! assert!(input.key_held(KeyCode::KeyW));
//! assert!(input.key_just_pressed(KeyCode::KeyW));
//!
//! input.end_frame();
//! assert!(input.key_held(KeyCode::KeyW)); // Still held...
//! assert!(!input.key_just_pressed(KeyCode::KeyW)); // ...but no longer new.
//! ```
//!
//! # Guarantees
//!
//! - **No lost taps.** A key pressed and released within one frame reports both
//!   [`InputState::key_just_pressed`] and [`InputState::key_just_released`].
//! - **No stuck keys.** When the window loses focus, every held key and button is released (and
//!   reported as just released). Keys already held when focus returns count only once they're
//!   pressed again. That's the same on Windows, X11 and Wayland (ADR-0018 §2).
//! - **Key repeat** doesn't change the state.
//!
//! Keys are physical positions ([`KeyCode`]). Bind `KeyW` and it's the same key on QWERTY and
//! AZERTY keyboards.
//!
//! [ADR-0018]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0018-input-events-state-and-routing.md

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::hash::Hash;

use oxreal_math::{DVec2, Vec2};
use oxreal_window::{DeviceEvent, WindowEvent};

pub use oxreal_window::{ButtonState, KeyCode, KeyEvent, MouseButton, ScrollDelta};

/// The input of the current frame, built from events (ADR-0018 §2). See the
/// [crate documentation](crate) for how to drive it.
///
/// It's an ECS [`Resource`](oxreal_ecs::Resource): systems declare
/// `.reads_resource::<InputState>()` and read it from `ctx.resources` (ADR-0014 §8).
#[derive(Debug, Clone, Default)]
pub struct InputState {
    keys: Buttons<KeyCode>,
    mouse_buttons: Buttons<MouseButton>,
    mouse_motion: DVec2,
    scroll_lines: Vec2,
    scroll_pixels: DVec2,
    cursor: Option<DVec2>,
}

impl oxreal_ecs::Resource for InputState {}

impl InputState {
    /// No keys or buttons held, no motion, and no cursor position.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a window event. Events that aren't input, other than focus loss, are ignored.
    pub fn handle_window_event(&mut self, event: &WindowEvent) {
        match *event {
            WindowEvent::Key(KeyEvent {
                key,
                state,
                repeat: false,
            }) => self.keys.apply(key, state),
            WindowEvent::MouseButton { button, state } => self.mouse_buttons.apply(button, state),
            WindowEvent::CursorMoved { x, y } => self.cursor = Some(DVec2::new(x, y)),
            WindowEvent::CursorLeft => self.cursor = None,
            WindowEvent::Scroll(ScrollDelta::Lines { x, y }) => {
                self.scroll_lines += Vec2::new(x, y);
            }
            WindowEvent::Scroll(ScrollDelta::Pixels { x, y }) => {
                self.scroll_pixels += DVec2::new(x, y);
            }
            WindowEvent::Focused(false) => self.release_everything(),
            _ => {}
        }
    }

    /// Applies a device event, such as raw mouse motion.
    pub fn handle_device_event(&mut self, event: &DeviceEvent) {
        if let DeviceEvent::MouseMotion { dx, dy } = *event {
            self.mouse_motion += DVec2::new(dx, dy);
        }
    }

    /// Ends the frame: forgets this frame's presses and releases, motion and scroll. Call it
    /// once per frame, after everything that reads input has run.
    pub fn end_frame(&mut self) {
        self.keys.end_frame();
        self.mouse_buttons.end_frame();
        self.mouse_motion = DVec2::ZERO;
        self.scroll_lines = Vec2::ZERO;
        self.scroll_pixels = DVec2::ZERO;
    }

    /// `true` while `key` is down.
    #[must_use]
    pub fn key_held(&self, key: KeyCode) -> bool {
        self.keys.held.contains(&key)
    }

    /// `true` if `key` went down this frame, even if it has come up again since.
    #[must_use]
    pub fn key_just_pressed(&self, key: KeyCode) -> bool {
        self.keys.pressed.contains(&key)
    }

    /// `true` if `key` came up this frame, including when focus was lost while it was held.
    #[must_use]
    pub fn key_just_released(&self, key: KeyCode) -> bool {
        self.keys.released.contains(&key)
    }

    /// `true` while `button` is down.
    #[must_use]
    pub fn mouse_held(&self, button: MouseButton) -> bool {
        self.mouse_buttons.held.contains(&button)
    }

    /// `true` if `button` went down this frame, even if it has come up again since.
    #[must_use]
    pub fn mouse_just_pressed(&self, button: MouseButton) -> bool {
        self.mouse_buttons.pressed.contains(&button)
    }

    /// `true` if `button` came up this frame, including when focus was lost while it was held.
    #[must_use]
    pub fn mouse_just_released(&self, button: MouseButton) -> bool {
        self.mouse_buttons.released.contains(&button)
    }

    /// Raw, unaccelerated mouse movement this frame, in device units: x to the right, y
    /// **toward the user** (down the screen). Use it for mouse-look. It's independent of the
    /// cursor, so it keeps working while the cursor is captured. The units differ by platform
    /// (see `oxreal_window::DeviceEvent::MouseMotion`), so scale it by a sensitivity setting.
    #[must_use]
    pub fn mouse_motion(&self) -> DVec2 {
        self.mouse_motion
    }

    /// Wheel notches scrolled this frame: y up (wheel turned away from the user), x right.
    #[must_use]
    pub fn scroll_lines(&self) -> Vec2 {
        self.scroll_lines
    }

    /// Touchpad pixels scrolled this frame: y up, x right.
    #[must_use]
    pub fn scroll_pixels(&self) -> DVec2 {
        self.scroll_pixels
    }

    /// The cursor's last position over the window, in physical pixels from its top-left corner
    /// (y down). `None` once it has left the window or focus was lost. Right after focus
    /// returns on X11 it can lie outside the window, because X11 reports where the pointer was
    /// when focus arrived.
    #[must_use]
    pub fn cursor_position(&self) -> Option<DVec2> {
        self.cursor
    }

    fn release_everything(&mut self) {
        self.keys.release_all();
        self.mouse_buttons.release_all();
        self.mouse_motion = DVec2::ZERO;
        self.scroll_lines = Vec2::ZERO;
        self.scroll_pixels = DVec2::ZERO;
        self.cursor = None;
    }
}

/// Held buttons plus this frame's edges.
#[derive(Debug, Clone)]
struct Buttons<T> {
    held: HashSet<T>,
    pressed: HashSet<T>,
    released: HashSet<T>,
}

impl<T> Default for Buttons<T> {
    fn default() -> Self {
        Self {
            held: HashSet::new(),
            pressed: HashSet::new(),
            released: HashSet::new(),
        }
    }
}

impl<T: Copy + Eq + Hash> Buttons<T> {
    fn apply(&mut self, button: T, state: ButtonState) {
        match state {
            // A press of a held button (no release seen in between) isn't a new press.
            ButtonState::Pressed => {
                if self.held.insert(button) {
                    self.pressed.insert(button);
                }
            }
            // A release without a press seen (held since before focus arrived) is ignored.
            ButtonState::Released => {
                if self.held.remove(&button) {
                    self.released.insert(button);
                }
            }
        }
    }

    fn release_all(&mut self) {
        self.released.extend(self.held.drain());
    }

    fn end_frame(&mut self) {
        self.pressed.clear();
        self.released.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: KeyCode, state: ButtonState) -> WindowEvent {
        WindowEvent::Key(KeyEvent {
            key,
            state,
            repeat: false,
        })
    }

    fn mouse(button: MouseButton, state: ButtonState) -> WindowEvent {
        WindowEvent::MouseButton { button, state }
    }

    const W: KeyCode = KeyCode::KeyW;
    const DOWN: ButtonState = ButtonState::Pressed;
    const UP: ButtonState = ButtonState::Released;

    #[test]
    fn a_press_is_new_for_one_frame_and_held_until_released() {
        let mut input = InputState::new();
        input.handle_window_event(&key(W, DOWN));
        assert!(input.key_held(W) && input.key_just_pressed(W) && !input.key_just_released(W));

        input.end_frame();
        assert!(input.key_held(W) && !input.key_just_pressed(W));

        input.handle_window_event(&key(W, UP));
        assert!(!input.key_held(W) && input.key_just_released(W));

        input.end_frame();
        assert!(!input.key_held(W) && !input.key_just_released(W));
        assert!(!input.key_held(KeyCode::KeyS), "other keys are untouched");
    }

    #[test]
    fn a_tap_inside_one_frame_reports_both_edges() {
        let mut input = InputState::new();
        input.handle_window_event(&key(KeyCode::Space, DOWN));
        input.handle_window_event(&key(KeyCode::Space, UP));
        assert!(input.key_just_pressed(KeyCode::Space));
        assert!(input.key_just_released(KeyCode::Space));
        assert!(!input.key_held(KeyCode::Space));
    }

    #[test]
    fn repeats_and_duplicate_presses_are_not_new_presses() {
        let mut input = InputState::new();
        input.handle_window_event(&key(W, DOWN));
        input.end_frame();
        input.handle_window_event(&WindowEvent::Key(KeyEvent {
            key: W,
            state: DOWN,
            repeat: true,
        }));
        input.handle_window_event(&key(W, DOWN));
        assert!(input.key_held(W));
        assert!(!input.key_just_pressed(W));
    }

    #[test]
    fn a_release_without_a_seen_press_is_ignored() {
        // The key went down before the window had focus, so its press was never delivered.
        let mut input = InputState::new();
        input.handle_window_event(&key(W, UP));
        assert!(!input.key_held(W) && !input.key_just_released(W));
    }

    #[test]
    fn focus_loss_releases_everything_and_forgets_motion() {
        let mut input = InputState::new();
        input.handle_window_event(&key(W, DOWN));
        input.handle_window_event(&mouse(MouseButton::Right, DOWN));
        input.handle_window_event(&WindowEvent::CursorMoved { x: 10.0, y: 20.0 });
        input.handle_window_event(&WindowEvent::Scroll(ScrollDelta::Lines { x: 0.0, y: 1.0 }));
        input.handle_device_event(&DeviceEvent::MouseMotion { dx: 3.0, dy: 4.0 });
        input.end_frame();
        input.handle_device_event(&DeviceEvent::MouseMotion { dx: 5.0, dy: 0.0 });

        input.handle_window_event(&WindowEvent::Focused(false));
        assert!(!input.key_held(W) && input.key_just_released(W));
        assert!(!input.mouse_held(MouseButton::Right));
        assert!(input.mouse_just_released(MouseButton::Right));
        assert_eq!(input.mouse_motion(), DVec2::ZERO);
        assert_eq!(input.scroll_lines(), Vec2::ZERO);
        assert_eq!(input.cursor_position(), None);

        // The OS may still deliver the release afterwards. It must not count twice.
        input.end_frame();
        input.handle_window_event(&key(W, UP));
        assert!(!input.key_just_released(W));
    }

    #[test]
    fn gaining_focus_changes_nothing() {
        let mut input = InputState::new();
        input.handle_window_event(&key(W, DOWN));
        input.handle_window_event(&WindowEvent::Focused(true));
        assert!(input.key_held(W) && input.key_just_pressed(W));
    }

    #[test]
    fn mouse_buttons_follow_the_same_rules_as_keys() {
        let mut input = InputState::new();
        input.handle_window_event(&mouse(MouseButton::Left, DOWN));
        assert!(input.mouse_held(MouseButton::Left));
        assert!(input.mouse_just_pressed(MouseButton::Left));
        assert!(!input.mouse_held(MouseButton::Right));
        input.end_frame();
        input.handle_window_event(&mouse(MouseButton::Left, UP));
        assert!(input.mouse_just_released(MouseButton::Left));
        assert!(!input.mouse_held(MouseButton::Left));
        input.handle_window_event(&mouse(MouseButton::Other(7), DOWN));
        assert!(input.mouse_held(MouseButton::Other(7)));
    }

    #[test]
    fn motion_and_scroll_accumulate_over_a_frame_and_then_reset() {
        let mut input = InputState::new();
        input.handle_device_event(&DeviceEvent::MouseMotion { dx: 1.5, dy: -2.0 });
        input.handle_device_event(&DeviceEvent::MouseMotion { dx: 0.5, dy: 1.0 });
        input.handle_window_event(&WindowEvent::Scroll(ScrollDelta::Lines { x: 0.0, y: 1.0 }));
        input.handle_window_event(&WindowEvent::Scroll(ScrollDelta::Lines { x: -1.0, y: 2.0 }));
        input.handle_window_event(&WindowEvent::Scroll(ScrollDelta::Pixels {
            x: 4.0,
            y: -8.0,
        }));
        assert_eq!(input.mouse_motion(), DVec2::new(2.0, -1.0));
        assert_eq!(input.scroll_lines(), Vec2::new(-1.0, 3.0));
        assert_eq!(input.scroll_pixels(), DVec2::new(4.0, -8.0));

        input.end_frame();
        assert_eq!(input.mouse_motion(), DVec2::ZERO);
        assert_eq!(input.scroll_lines(), Vec2::ZERO);
        assert_eq!(input.scroll_pixels(), DVec2::ZERO);
    }

    #[test]
    fn the_cursor_position_survives_frames_until_the_cursor_leaves() {
        let mut input = InputState::new();
        assert_eq!(input.cursor_position(), None);
        input.handle_window_event(&WindowEvent::CursorMoved { x: 640.0, y: 360.5 });
        input.end_frame();
        assert_eq!(input.cursor_position(), Some(DVec2::new(640.0, 360.5)));
        input.handle_window_event(&WindowEvent::CursorLeft);
        assert_eq!(input.cursor_position(), None);
    }

    #[test]
    fn events_that_are_not_input_are_ignored() {
        let mut input = InputState::new();
        input.handle_window_event(&WindowEvent::RedrawRequested);
        input.handle_window_event(&WindowEvent::CloseRequested);
        assert!(KeyCode::ALL.iter().all(|&k| !input.key_held(k)));
    }
}
