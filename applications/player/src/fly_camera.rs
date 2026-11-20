//! A fly camera with Unreal-style viewport controls (ADR-0018 §6).
//!
//! While the right mouse button is held, moving the mouse turns the view, `W` `A` `S` `D` move
//! forward, left, back and right, `E` and `Q` move up and down, and the wheel changes speed.
//! With the button up, the camera ignores input, so keys stay free for other uses.
//!
//! It lives in the player with the demo scene, and goes when the gameplay camera arrives with
//! the gameplay capabilities (ADR-0019 §3, ADR-0020). The `fly camera` system in `scene.rs`
//! copies its [`FlyCamera::transform`] into the entity's `LocalTransform`, which places the
//! entity's render-bridge `Camera`.

use oxreal_ecs::Component;
use oxreal_input::{InputState, KeyCode, MouseButton};
use oxreal_math::{Quat, Transform, Vec3, axes};

/// The button that has to be held for every fly control.
pub(crate) const LOOK_BUTTON: MouseButton = MouseButton::Right;

/// Radians turned per raw mouse unit. Raw units are roughly one per mouse count, so this is
/// about 0.14° per count.
const RADIANS_PER_MOUSE_UNIT: f32 = 0.0025;
/// How far the view can tilt up or down, short of straight up so yaw stays well defined.
const PITCH_LIMIT: f32 = 89.0_f32.to_radians();
/// Speed is multiplied by this per wheel notch up, and divided per notch down.
const SPEED_STEP: f32 = 1.25;
const MIN_SPEED: f32 = 0.25;
const MAX_SPEED: f32 = 100.0;
/// Touchpad pixels that count as one wheel notch: about one line of text.
const PIXELS_PER_NOTCH: f64 = 40.0;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FlyCamera {
    position: Vec3,
    /// Turn about world up, in radians. 0 looks along −Z (`axes::FORWARD`), positive turns left.
    yaw: f32,
    /// Tilt, in radians. Positive looks up.
    pitch: f32,
    /// Metres per second.
    speed: f32,
}

impl FlyCamera {
    /// A camera at `position` looking at `target`, moving at `speed` metres per second.
    pub(crate) fn looking_at(position: Vec3, target: Vec3, speed: f32) -> Self {
        let direction = (target - position).normalize_or(axes::FORWARD);
        Self {
            position,
            // Rotating −Z by `yaw` about +Y gives (−sin yaw, 0, −cos yaw).
            yaw: (-direction.x).atan2(-direction.z),
            pitch: direction
                .y
                .clamp(-1.0, 1.0)
                .asin()
                .clamp(-PITCH_LIMIT, PITCH_LIMIT),
            speed: speed.clamp(MIN_SPEED, MAX_SPEED),
        }
    }

    /// The camera's transform, for `projection::view_matrix`.
    pub(crate) fn transform(&self) -> Transform {
        Transform::from_translation(self.position).with_rotation(self.rotation())
    }

    /// Metres per second.
    pub(crate) fn speed(&self) -> f32 {
        self.speed
    }

    /// Applies one frame of input. `seconds` is the time since the previous frame.
    pub(crate) fn update(&mut self, input: &InputState, seconds: f32) {
        if !input.mouse_held(LOOK_BUTTON) {
            return;
        }
        // Mouse right turns right (negative yaw). Mouse toward the user looks down.
        let motion = input.mouse_motion().as_vec2() * RADIANS_PER_MOUSE_UNIT;
        self.yaw = (self.yaw - motion.x).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch - motion.y).clamp(-PITCH_LIMIT, PITCH_LIMIT);

        // Wheels scroll in notches. Touchpads scroll in pixels on Wayland (and in fractional
        // notches on Windows), so pixels count too. A frame's scroll is far inside f32's range.
        let notches = input.scroll_lines().y + (input.scroll_pixels().y / PIXELS_PER_NOTCH) as f32;
        if notches != 0.0 {
            self.speed = (self.speed * SPEED_STEP.powf(notches)).clamp(MIN_SPEED, MAX_SPEED);
        }

        let axis = |positive: KeyCode, negative: KeyCode| {
            f32::from(u8::from(input.key_held(positive)))
                - f32::from(u8::from(input.key_held(negative)))
        };
        let rotation = self.rotation();
        // Forward follows the view, including its tilt. Up and down follow the world.
        let direction = rotation * axes::FORWARD * axis(KeyCode::KeyW, KeyCode::KeyS)
            + rotation * axes::RIGHT * axis(KeyCode::KeyD, KeyCode::KeyA)
            + axes::UP * axis(KeyCode::KeyE, KeyCode::KeyQ);
        // Diagonals are no faster than straight lines.
        self.position += direction.normalize_or_zero() * self.speed * seconds;
    }

    fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch)
    }
}

impl Component for FlyCamera {}

#[cfg(test)]
mod tests {
    use oxreal_input::{ButtonState, KeyEvent, ScrollDelta};
    use oxreal_window::{DeviceEvent, WindowEvent};

    use super::*;

    fn camera() -> FlyCamera {
        FlyCamera::looking_at(Vec3::ZERO, axes::FORWARD, 4.0)
    }

    fn press(input: &mut InputState, key: KeyCode) {
        input.handle_window_event(&WindowEvent::Key(KeyEvent {
            key,
            state: ButtonState::Pressed,
            repeat: false,
        }));
    }

    fn hold_look_button(input: &mut InputState) {
        input.handle_window_event(&WindowEvent::MouseButton {
            button: LOOK_BUTTON,
            state: ButtonState::Pressed,
        });
    }

    fn assert_near(actual: Vec3, expected: Vec3) {
        assert!(
            actual.abs_diff_eq(expected, 1e-5),
            "{actual} is not {expected}"
        );
    }

    #[test]
    fn looking_at_faces_the_target() {
        for target in [
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(3.0, 0.0, 0.0),
            Vec3::new(-2.0, 1.0, 4.0),
        ] {
            let camera = FlyCamera::looking_at(Vec3::new(0.0, 0.0, 0.0), target, 4.0);
            assert_near(camera.transform().forward(), target.normalize());
        }
        let camera = FlyCamera::looking_at(Vec3::new(1.0, 2.0, 3.0), Vec3::ZERO, 4.0);
        assert_near(camera.transform().translation, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn nothing_happens_without_the_look_button() {
        let mut input = InputState::new();
        press(&mut input, KeyCode::KeyW);
        input.handle_device_event(&DeviceEvent::MouseMotion { dx: 100.0, dy: 0.0 });
        let mut moved = camera();
        moved.update(&input, 1.0);
        assert_eq!(moved, camera());
    }

    #[test]
    fn keys_move_relative_to_the_view_and_the_world() {
        let cases = [
            (KeyCode::KeyW, axes::FORWARD),
            (KeyCode::KeyS, axes::BACK),
            (KeyCode::KeyA, axes::LEFT),
            (KeyCode::KeyD, axes::RIGHT),
            (KeyCode::KeyE, axes::UP),
            (KeyCode::KeyQ, axes::DOWN),
        ];
        for (key, direction) in cases {
            let mut input = InputState::new();
            hold_look_button(&mut input);
            press(&mut input, key);
            let mut camera = camera();
            camera.update(&input, 0.5);
            // 4 m/s for half a second.
            assert_near(camera.transform().translation, direction * 2.0);
        }
    }

    #[test]
    fn diagonals_are_no_faster_and_opposites_cancel() {
        let mut input = InputState::new();
        hold_look_button(&mut input);
        press(&mut input, KeyCode::KeyW);
        press(&mut input, KeyCode::KeyD);
        let mut camera = camera();
        camera.update(&input, 1.0);
        assert!((camera.transform().translation.length() - 4.0).abs() < 1e-5);

        press(&mut input, KeyCode::KeyS);
        press(&mut input, KeyCode::KeyA);
        let before = camera.transform().translation;
        camera.update(&input, 1.0);
        assert_near(camera.transform().translation, before);
    }

    #[test]
    fn the_mouse_turns_right_and_looks_down() {
        let mut input = InputState::new();
        hold_look_button(&mut input);
        // A quarter turn right, and a little down.
        let quarter = std::f32::consts::FRAC_PI_2 / RADIANS_PER_MOUSE_UNIT;
        input.handle_device_event(&DeviceEvent::MouseMotion {
            dx: f64::from(quarter),
            dy: 40.0,
        });
        let mut camera = camera();
        camera.update(&input, 0.0);
        let forward = camera.transform().forward();
        assert!(forward.x > 0.99, "turned right toward +X: {forward}");
        assert!(forward.y < 0.0, "looks down: {forward}");
    }

    #[test]
    fn pitch_stops_short_of_straight_up_and_down() {
        for dy in [-1.0e6, 1.0e6] {
            let mut input = InputState::new();
            hold_look_button(&mut input);
            input.handle_device_event(&DeviceEvent::MouseMotion { dx: 0.0, dy });
            let mut camera = camera();
            camera.update(&input, 0.0);
            let forward = camera.transform().forward();
            assert!(
                forward.y.abs() < 0.9999 && forward.y.abs() > 0.999,
                "{forward}"
            );
            // Moving forward still moves, rather than collapsing to straight up or down.
            assert!(forward.x.abs() + forward.z.abs() > 0.0);
        }
    }

    #[test]
    fn the_wheel_changes_speed_within_limits() {
        let mut input = InputState::new();
        hold_look_button(&mut input);
        input.handle_window_event(&WindowEvent::Scroll(ScrollDelta::Lines { x: 0.0, y: 2.0 }));
        let mut camera = camera();
        camera.update(&input, 0.0);
        assert!((camera.speed() - 4.0 * SPEED_STEP * SPEED_STEP).abs() < 1e-4);

        // A touchpad: one notch's worth of pixels is one step.
        input.end_frame();
        let before = camera.speed();
        input.handle_window_event(&WindowEvent::Scroll(ScrollDelta::Pixels {
            x: 0.0,
            y: PIXELS_PER_NOTCH,
        }));
        camera.update(&input, 0.0);
        assert!((camera.speed() - before * SPEED_STEP).abs() < 1e-4);

        input.end_frame();
        input.handle_window_event(&WindowEvent::Scroll(ScrollDelta::Lines {
            x: 0.0,
            y: -1000.0,
        }));
        camera.update(&input, 0.0);
        assert_eq!(camera.speed(), MIN_SPEED);

        input.end_frame();
        input.handle_window_event(&WindowEvent::Scroll(ScrollDelta::Lines {
            x: 0.0,
            y: 1000.0,
        }));
        camera.update(&input, 0.0);
        assert_eq!(camera.speed(), MAX_SPEED);
    }
}
