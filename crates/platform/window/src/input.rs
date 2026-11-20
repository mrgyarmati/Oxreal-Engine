//! Engine-owned keyboard and mouse vocabulary, and its translation from winit (ADR-0018 §1).

/// Whether a key or button went down or came up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ButtonState {
    /// The key or button went down.
    Pressed,
    /// The key or button came up.
    Released,
}

impl ButtonState {
    /// `true` for [`ButtonState::Pressed`].
    #[must_use]
    pub const fn is_pressed(self) -> bool {
        matches!(self, Self::Pressed)
    }
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// The primary (usually left) button.
    Left,
    /// The secondary (usually right) button.
    Right,
    /// The middle button, often the wheel pressed down.
    Middle,
    /// The "back" side button.
    Back,
    /// The "forward" side button.
    Forward,
    /// Another button, by the platform's index. The same physical button can have different
    /// indices on different platforms, so bindings should prefer the named buttons.
    Other(u16),
}

/// A key press or release, by the key's **physical position** (ADR-0018 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    /// Which key, by position. Not affected by the keyboard layout.
    pub key: KeyCode,
    /// Whether it went down or came up.
    pub state: ButtonState,
    /// `true` for the repeated presses the system sends while a key is held. A repeat is not a
    /// new press.
    pub repeat: bool,
}

/// How far a wheel or touchpad scrolled.
///
/// Positive `y` scrolls **up** (a wheel turned away from the user). Positive `x` scrolls
/// **right**. Lines and pixels are kept apart: a wheel notch and a touchpad pixel aren't
/// comparable, and the consumer decides how to weigh them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollDelta {
    /// Wheel notches (lines). One notch of a typical wheel is `1.0`. High-resolution wheels
    /// report fractions.
    Lines {
        /// Horizontal notches, positive to the right.
        x: f32,
        /// Vertical notches, positive up.
        y: f32,
    },
    /// Pixels, from touchpads and other precise devices.
    Pixels {
        /// Horizontal physical pixels, positive to the right.
        x: f64,
        /// Vertical physical pixels, positive up.
        y: f64,
    },
}

/// An event from an input device rather than a window, delivered through
/// [`crate::App::device_event`] **only while one of the application's windows has focus**.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum DeviceEvent {
    /// Raw, unaccelerated mouse movement, for mouse-look. It keeps arriving while the cursor is
    /// captured or at the edge of the screen, unlike cursor positions.
    MouseMotion {
        /// Horizontal movement in device units (not pixels), positive to the right.
        dx: f64,
        /// Vertical movement in device units, positive **toward the user** (down the screen).
        dy: f64,
    },
}

/// Defines [`KeyCode`], its list of every variant, and its translations to and from winit. Every
/// variant has the same name as winit's, which is the W3C `KeyboardEvent.code` name.
macro_rules! key_codes {
    ($($name:ident => $label:literal,)+) => {
        /// A key, named by its **physical position** on a US keyboard (W3C
        /// `KeyboardEvent.code` names, ADR-0018 §1).
        ///
        /// [`KeyCode::KeyW`] is the key left of [`KeyCode::KeyE`] on every layout. On a French
        /// (AZERTY) keyboard it's labelled `Z`. So bindings stay in place when the layout changes.
        /// Typed text is a separate concern, handled with UI.
        ///
        /// The keys of standard 104-, 105- and 106-key keyboards are modelled. Media, browser and
        /// launcher keys aren't, because they're system controls rather than game input.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[non_exhaustive]
        pub enum KeyCode {
            $(
                #[doc = concat!("The key at the position of `", $label, "` on a US keyboard.")]
                $name,
            )+
        }

        impl KeyCode {
            /// Every key the engine models, in declaration order.
            pub const ALL: &'static [KeyCode] = &[$(KeyCode::$name,)+];
        }

        /// The engine key for a winit key, or `None` for keys the engine doesn't model.
        fn key_from_winit(code: winit::keyboard::KeyCode) -> Option<KeyCode> {
            Some(match code {
                $(winit::keyboard::KeyCode::$name => KeyCode::$name,)+
                _ => return None,
            })
        }

        #[cfg(test)]
        fn key_to_winit(key: KeyCode) -> winit::keyboard::KeyCode {
            match key {
                $(KeyCode::$name => winit::keyboard::KeyCode::$name,)+
            }
        }
    };
}

key_codes! {
    // Writing-system keys.
    Backquote => "`",
    Digit1 => "1", Digit2 => "2", Digit3 => "3", Digit4 => "4", Digit5 => "5",
    Digit6 => "6", Digit7 => "7", Digit8 => "8", Digit9 => "9", Digit0 => "0",
    Minus => "-",
    Equal => "=",
    KeyQ => "Q", KeyW => "W", KeyE => "E", KeyR => "R", KeyT => "T",
    KeyY => "Y", KeyU => "U", KeyI => "I", KeyO => "O", KeyP => "P",
    BracketLeft => "[",
    BracketRight => "]",
    Backslash => "\\",
    KeyA => "A", KeyS => "S", KeyD => "D", KeyF => "F", KeyG => "G",
    KeyH => "H", KeyJ => "J", KeyK => "K", KeyL => "L",
    Semicolon => ";",
    Quote => "'",
    KeyZ => "Z", KeyX => "X", KeyC => "C", KeyV => "V", KeyB => "B",
    KeyN => "N", KeyM => "M",
    Comma => ",",
    Period => ".",
    Slash => "/",
    IntlBackslash => "\\ (the extra key left of Z on 105-key keyboards)",
    IntlRo => "\\ (the extra key left of right Shift on Japanese keyboards)",
    IntlYen => "¥ (the extra key left of Backspace on Japanese keyboards)",
    // Functional keys.
    Escape => "Esc",
    Tab => "Tab",
    CapsLock => "Caps Lock",
    ShiftLeft => "left Shift",
    ShiftRight => "right Shift",
    ControlLeft => "left Ctrl",
    ControlRight => "right Ctrl",
    AltLeft => "left Alt",
    AltRight => "right Alt (AltGr on many layouts)",
    SuperLeft => "left Windows / Super",
    SuperRight => "right Windows / Super",
    ContextMenu => "Menu",
    Space => "Space",
    Enter => "Enter",
    Backspace => "Backspace",
    Convert => "変換 (Convert, Japanese keyboards)",
    NonConvert => "無変換 (Non-convert, Japanese keyboards)",
    KanaMode => "かな (Kana mode, Japanese keyboards)",
    // Navigation and editing.
    Insert => "Insert",
    Delete => "Delete",
    Home => "Home",
    End => "End",
    PageUp => "Page Up",
    PageDown => "Page Down",
    ArrowUp => "↑",
    ArrowDown => "↓",
    ArrowLeft => "←",
    ArrowRight => "→",
    // Numeric keypad.
    NumLock => "Num Lock",
    Numpad0 => "numpad 0", Numpad1 => "numpad 1", Numpad2 => "numpad 2",
    Numpad3 => "numpad 3", Numpad4 => "numpad 4", Numpad5 => "numpad 5",
    Numpad6 => "numpad 6", Numpad7 => "numpad 7", Numpad8 => "numpad 8",
    Numpad9 => "numpad 9",
    NumpadAdd => "numpad +",
    NumpadSubtract => "numpad -",
    NumpadMultiply => "numpad *",
    NumpadDivide => "numpad /",
    NumpadDecimal => "numpad .",
    NumpadEnter => "numpad Enter",
    // Function row and system keys.
    F1 => "F1", F2 => "F2", F3 => "F3", F4 => "F4", F5 => "F5", F6 => "F6",
    F7 => "F7", F8 => "F8", F9 => "F9", F10 => "F10", F11 => "F11", F12 => "F12",
    F13 => "F13", F14 => "F14", F15 => "F15", F16 => "F16", F17 => "F17", F18 => "F18",
    F19 => "F19", F20 => "F20", F21 => "F21", F22 => "F22", F23 => "F23", F24 => "F24",
    PrintScreen => "Print Screen",
    ScrollLock => "Scroll Lock",
    Pause => "Pause",
}

/// Translates a winit key event. `None` for synthetic events (ADR-0018 §1: only X11 and Windows
/// produce them, so they're dropped everywhere) and for keys the engine doesn't model.
pub(crate) fn key_event_from_winit(
    physical_key: winit::keyboard::PhysicalKey,
    state: winit::event::ElementState,
    repeat: bool,
    is_synthetic: bool,
) -> Option<KeyEvent> {
    if is_synthetic {
        return None;
    }
    let winit::keyboard::PhysicalKey::Code(code) = physical_key else {
        return None;
    };
    Some(KeyEvent {
        key: key_from_winit(code)?,
        state: button_state_from_winit(state),
        repeat,
    })
}

pub(crate) fn button_state_from_winit(state: winit::event::ElementState) -> ButtonState {
    match state {
        winit::event::ElementState::Pressed => ButtonState::Pressed,
        winit::event::ElementState::Released => ButtonState::Released,
    }
}

pub(crate) fn mouse_button_from_winit(button: winit::event::MouseButton) -> MouseButton {
    use winit::event::MouseButton as W;
    match button {
        W::Left => MouseButton::Left,
        W::Right => MouseButton::Right,
        W::Middle => MouseButton::Middle,
        W::Back => MouseButton::Back,
        W::Forward => MouseButton::Forward,
        W::Other(index) => MouseButton::Other(index),
    }
}

/// winit reports positive y for "content moves down" (which is scrolling up) and positive x for
/// "content moves right" (which is scrolling **left**). The engine's convention is positive =
/// up and right, so only x changes sign.
pub(crate) fn scroll_from_winit(delta: winit::event::MouseScrollDelta) -> ScrollDelta {
    use winit::event::MouseScrollDelta as W;
    match delta {
        W::LineDelta(x, y) => ScrollDelta::Lines { x: -x, y },
        W::PixelDelta(p) => ScrollDelta::Pixels { x: -p.x, y: p.y },
    }
}

pub(crate) fn device_event_from_winit(event: &winit::event::DeviceEvent) -> Option<DeviceEvent> {
    match event {
        winit::event::DeviceEvent::MouseMotion { delta: (dx, dy) } => {
            Some(DeviceEvent::MouseMotion { dx: *dx, dy: *dy })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_modelled_key_round_trips_through_winit() {
        let mut seen = HashSet::new();
        for &key in KeyCode::ALL {
            assert!(seen.insert(key), "{key:?} is listed twice");
            assert_eq!(key_from_winit(key_to_winit(key)), Some(key));
        }
        // 104-key ANSI layout, plus IntlBackslash (105), IntlRo, IntlYen, Convert, NonConvert and
        // KanaMode (106/109), plus F13-F24.
        assert_eq!(KeyCode::ALL.len(), 104 + 6 + 12);
    }

    #[test]
    fn keys_the_engine_does_not_model_are_dropped() {
        use winit::keyboard::KeyCode as W;
        for code in [
            W::MediaPlayPause,
            W::BrowserBack,
            W::AudioVolumeUp,
            W::Fn,
            W::F25,
        ] {
            assert_eq!(key_from_winit(code), None, "{code:?}");
        }
    }

    #[test]
    fn key_events_keep_position_state_and_repeat() {
        use winit::event::ElementState;
        use winit::keyboard::{KeyCode as W, PhysicalKey};
        assert_eq!(
            key_event_from_winit(
                PhysicalKey::Code(W::KeyW),
                ElementState::Pressed,
                true,
                false
            ),
            Some(KeyEvent {
                key: KeyCode::KeyW,
                state: ButtonState::Pressed,
                repeat: true,
            })
        );
        assert_eq!(
            key_event_from_winit(
                PhysicalKey::Code(W::ShiftLeft),
                ElementState::Released,
                false,
                false
            ),
            Some(KeyEvent {
                key: KeyCode::ShiftLeft,
                state: ButtonState::Released,
                repeat: false,
            })
        );
    }

    #[test]
    fn synthetic_unidentified_and_unmodelled_keys_are_dropped() {
        use winit::event::ElementState;
        use winit::keyboard::{KeyCode as W, NativeKeyCode, PhysicalKey};
        let w = PhysicalKey::Code(W::KeyW);
        // Synthetic presses and releases (X11 and Windows on focus changes) are dropped.
        assert_eq!(
            key_event_from_winit(w, ElementState::Pressed, false, true),
            None
        );
        assert_eq!(
            key_event_from_winit(w, ElementState::Released, false, true),
            None
        );
        assert_eq!(
            key_event_from_winit(
                PhysicalKey::Unidentified(NativeKeyCode::Xkb(0x1234)),
                ElementState::Pressed,
                false,
                false
            ),
            None
        );
        assert_eq!(
            key_event_from_winit(
                PhysicalKey::Code(W::MediaPlayPause),
                ElementState::Pressed,
                false,
                false
            ),
            None
        );
    }

    #[test]
    fn button_states_and_mouse_buttons_map() {
        use winit::event::{ElementState, MouseButton as W};
        assert_eq!(
            button_state_from_winit(ElementState::Pressed),
            ButtonState::Pressed
        );
        assert!(!button_state_from_winit(ElementState::Released).is_pressed());
        assert_eq!(mouse_button_from_winit(W::Left), MouseButton::Left);
        assert_eq!(mouse_button_from_winit(W::Right), MouseButton::Right);
        assert_eq!(mouse_button_from_winit(W::Middle), MouseButton::Middle);
        assert_eq!(mouse_button_from_winit(W::Back), MouseButton::Back);
        assert_eq!(mouse_button_from_winit(W::Forward), MouseButton::Forward);
        assert_eq!(mouse_button_from_winit(W::Other(9)), MouseButton::Other(9));
    }

    #[test]
    fn scrolling_is_positive_up_and_right() {
        use winit::dpi::PhysicalPosition;
        use winit::event::MouseScrollDelta as W;
        // A wheel turned away from the user: winit +y (content moves down).
        assert_eq!(
            scroll_from_winit(W::LineDelta(0.0, 1.0)),
            ScrollDelta::Lines { x: 0.0, y: 1.0 }
        );
        // A wheel tilted right: winit -x on Windows (content moves left).
        assert_eq!(
            scroll_from_winit(W::LineDelta(-1.0, 0.0)),
            ScrollDelta::Lines { x: 1.0, y: 0.0 }
        );
        assert_eq!(
            scroll_from_winit(W::PixelDelta(PhysicalPosition::new(-3.0, 4.0))),
            ScrollDelta::Pixels { x: 3.0, y: 4.0 }
        );
    }

    #[test]
    fn raw_mouse_motion_maps_and_other_device_events_drop() {
        use winit::event::DeviceEvent as W;
        assert_eq!(
            device_event_from_winit(&W::MouseMotion { delta: (2.5, -1.0) }),
            Some(DeviceEvent::MouseMotion { dx: 2.5, dy: -1.0 })
        );
        assert_eq!(device_event_from_winit(&W::Added), None);
    }
}
