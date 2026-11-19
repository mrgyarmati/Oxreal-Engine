//! The engine's axis conventions as named constants (ADR-0011 §2).
//!
//! Use these names instead of literal axes, so code says *what* a direction means.

use glam::Vec3;

/// World up: **+Y**.
pub const UP: Vec3 = Vec3::Y;
/// World down: −Y. Default gravity points this way.
pub const DOWN: Vec3 = Vec3::NEG_Y;
/// Right: **+X**.
pub const RIGHT: Vec3 = Vec3::X;
/// Left: −X.
pub const LEFT: Vec3 = Vec3::NEG_X;
/// Forward: **−Z**. An identity rotation, and every camera by default, faces this way.
pub const FORWARD: Vec3 = Vec3::NEG_Z;
/// Back: +Z. glTF assets face this way by specification.
pub const BACK: Vec3 = Vec3::Z;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_form_a_right_handed_basis() {
        // In a right-handed system X × Y = Z. With forward = −Z this gives
        // right × up = −forward.
        assert_eq!(RIGHT.cross(UP), -FORWARD);
        assert_eq!(Vec3::X.cross(Vec3::Y), Vec3::Z);
    }

    #[test]
    fn opposites_are_consistent() {
        assert_eq!(UP, -DOWN);
        assert_eq!(RIGHT, -LEFT);
        assert_eq!(FORWARD, -BACK);
    }
}
