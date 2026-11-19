//! Translation, rotation and scale (ADR-0011).

use std::ops::Mul;

use glam::{Mat3, Mat4, Quat, Vec3};

use crate::axes;

/// A placement in space: scale, then rotate, then translate.
///
/// The rotation is a unit quaternion. An identity transform faces [`axes::FORWARD`] (−Z) with
/// [`axes::UP`] (+Y) up.
///
/// # Composition
///
/// `parent * child` gives the child's placement in the parent's space (`world = parent * local`).
/// It's exact when the parent's scale is **uniform**. With non-uniform parent scale and a rotated
/// child, the true result contains shear, which a `Transform` can't represent. Use
/// [`Transform::to_matrix`] products when that matters. Unreal's `FTransform` has the same
/// property.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Position, in metres.
    pub translation: Vec3,
    /// Orientation, as a unit quaternion.
    pub rotation: Quat,
    /// Per-axis scale factors.
    pub scale: Vec3,
}

impl Transform {
    /// No translation, no rotation, unit scale.
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    /// A transform that only translates.
    #[must_use]
    pub const fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    /// A transform that only rotates.
    #[must_use]
    pub const fn from_rotation(rotation: Quat) -> Self {
        Self {
            rotation,
            ..Self::IDENTITY
        }
    }

    /// A transform that only scales.
    #[must_use]
    pub const fn from_scale(scale: Vec3) -> Self {
        Self {
            scale,
            ..Self::IDENTITY
        }
    }

    /// Decomposes an affine matrix into scale, rotation and translation.
    ///
    /// The matrix must not contain shear or a projection. Those can't be represented, and the
    /// result would be approximate.
    #[must_use]
    pub fn from_matrix(matrix: &Mat4) -> Self {
        let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
        Self {
            translation,
            rotation,
            scale,
        }
    }

    /// Returns this transform with a different translation.
    #[must_use]
    pub const fn with_translation(self, translation: Vec3) -> Self {
        Self {
            translation,
            ..self
        }
    }

    /// Returns this transform with a different rotation.
    #[must_use]
    pub const fn with_rotation(self, rotation: Quat) -> Self {
        Self { rotation, ..self }
    }

    /// Returns this transform with a different scale.
    #[must_use]
    pub const fn with_scale(self, scale: Vec3) -> Self {
        Self { scale, ..self }
    }

    /// The equivalent matrix (`translation * rotation * scale`, column-major).
    #[must_use]
    pub fn to_matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    /// Transforms a point: scale, rotate, then translate.
    #[must_use]
    pub fn transform_point(&self, point: Vec3) -> Vec3 {
        self.translation + self.rotation * (self.scale * point)
    }

    /// Transforms a direction or offset: scale and rotate, without translation.
    #[must_use]
    pub fn transform_vector(&self, vector: Vec3) -> Vec3 {
        self.rotation * (self.scale * vector)
    }

    /// The direction this transform faces: its local −Z, unit length.
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        self.rotation * axes::FORWARD
    }

    /// Its local +X, unit length.
    #[must_use]
    pub fn right(&self) -> Vec3 {
        self.rotation * axes::RIGHT
    }

    /// Its local +Y, unit length.
    #[must_use]
    pub fn up(&self) -> Vec3 {
        self.rotation * axes::UP
    }

    /// Returns this transform rotated to face `direction`, with its up as close to `up` as
    /// possible. Translation and scale are kept.
    ///
    /// Degenerate input never produces NaN:
    /// - a zero `direction` leaves the rotation unchanged
    /// - a zero `up` is treated as [`axes::UP`]
    /// - `direction` parallel to `up` picks an arbitrary perpendicular right axis
    #[must_use]
    pub fn looking_to(self, direction: Vec3, up: Vec3) -> Self {
        let Some(forward) = direction.try_normalize() else {
            return self;
        };
        let up = up.try_normalize().unwrap_or(axes::UP);
        let right = forward
            .cross(up)
            .try_normalize()
            .unwrap_or_else(|| forward.any_orthonormal_vector());
        let true_up = right.cross(forward);
        // Columns are where local +X, +Y and +Z go. Local −Z (forward) maps to `forward`.
        let rotation = Quat::from_mat3(&Mat3::from_cols(right, true_up, -forward)).normalize();
        self.with_rotation(rotation)
    }

    /// Returns this transform rotated to face `target`. See [`Transform::looking_to`].
    #[must_use]
    pub fn looking_at(self, target: Vec3, up: Vec3) -> Self {
        self.looking_to(target - self.translation, up)
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mul for Transform {
    type Output = Self;

    /// `parent * child`: the child's placement in the parent's space. See the type docs for when
    /// this is exact.
    fn mul(self, child: Self) -> Self {
        Self {
            translation: self.transform_point(child.translation),
            rotation: (self.rotation * child.rotation).normalize(),
            scale: self.scale * child.scale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    const EPS: f32 = 1e-5;

    fn assert_close(a: Vec3, b: Vec3) {
        assert!(a.abs_diff_eq(b, EPS), "{a} != {b}");
    }

    /// Deterministic sample transforms. Uniform scale only, where composition is exact.
    fn samples() -> Vec<Transform> {
        let mut out = vec![Transform::IDENTITY];
        for i in 0..8 {
            let f = i as f32;
            out.push(Transform {
                translation: Vec3::new(f - 3.0, 0.5 * f, 2.0 - f),
                rotation: Quat::from_euler(glam::EulerRot::YXZ, 0.3 * f, -0.2 * f, 0.1 * f),
                scale: Vec3::splat(0.5 + 0.25 * f),
            });
        }
        out
    }

    #[test]
    fn identity_faces_forward_with_up_up() {
        let t = Transform::IDENTITY;
        assert_eq!(t.forward(), axes::FORWARD);
        assert_eq!(t.up(), axes::UP);
        assert_eq!(t.right(), axes::RIGHT);
        assert_eq!(Transform::default(), t);
    }

    #[test]
    fn point_and_vector_transforms_match_the_matrix() {
        let p = Vec3::new(0.3, -1.2, 2.5);
        for t in samples() {
            let m = t.to_matrix();
            assert_close(t.transform_point(p), m.transform_point3(p));
            assert_close(t.transform_vector(p), m.transform_vector3(p));
        }
    }

    #[test]
    fn composition_matches_matrix_product_for_uniform_scale() {
        let p = Vec3::new(1.0, 2.0, -3.0);
        for parent in samples() {
            for child in samples() {
                let composed = parent * child;
                let product = parent.to_matrix() * child.to_matrix();
                assert_close(composed.transform_point(p), product.transform_point3(p));
            }
        }
    }

    #[test]
    fn matrix_round_trip() {
        for t in samples() {
            let back = Transform::from_matrix(&t.to_matrix());
            assert_close(back.translation, t.translation);
            assert_close(back.scale, t.scale);
            // q and −q are the same rotation, so compare their effect.
            assert_close(back.rotation * Vec3::X, t.rotation * Vec3::X);
            assert_close(back.rotation * Vec3::Y, t.rotation * Vec3::Y);
        }
    }

    #[test]
    fn looking_at_faces_the_target_and_keeps_up_upright() {
        let eye = Vec3::new(3.0, 4.0, 5.0);
        let target = Vec3::new(-1.0, 0.5, 0.0);
        let t = Transform::from_translation(eye).looking_at(target, axes::UP);
        assert_close(t.forward(), (target - eye).normalize());
        assert!(t.up().dot(axes::UP) > 0.0, "up flipped: {}", t.up());
        assert!(t.right().dot(axes::UP).abs() < EPS, "roll introduced");
        assert!((t.rotation.length() - 1.0).abs() < EPS);
        assert_eq!(t.translation, eye);
    }

    #[test]
    fn looking_to_a_side_axis_turns_as_expected() {
        // Facing +X from the default −Z is a −90° turn around +Y (right-handed).
        let t = Transform::IDENTITY.looking_to(Vec3::X, axes::UP);
        let expected = Quat::from_rotation_y(-FRAC_PI_2);
        assert_close(t.rotation * Vec3::Z, expected * Vec3::Z);
        assert_close(t.forward(), Vec3::X);
    }

    #[test]
    fn degenerate_look_directions_never_produce_nan() {
        let base = Transform::from_translation(Vec3::ONE);
        assert_eq!(base.looking_to(Vec3::ZERO, axes::UP), base);
        for (direction, up) in [
            (axes::UP, axes::UP),
            (axes::DOWN, axes::UP),
            (Vec3::X, Vec3::ZERO),
        ] {
            let t = base.looking_to(direction, up);
            assert!(t.rotation.is_finite(), "{direction} / {up}");
            assert_close(t.forward(), direction.normalize());
        }
    }
}
