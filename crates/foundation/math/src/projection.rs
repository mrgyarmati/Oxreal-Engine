//! Projection and view matrices for Oxreal's clip-space convention (ADR-0011 §4).
//!
//! - View space is right-handed and +Y up, and the camera looks down **−Z**.
//! - Clip space is **Y up** with depth in **[0, 1]** on every backend. The Vulkan backend flips Y
//!   with a negative viewport height, so these matrices are never adjusted per API.
//! - Scene depth is **reversed**: the near plane maps to 1 and distance approaches 0. Clear
//!   depth to [`REVERSED_DEPTH_CLEAR`] and compare with *greater or equal*.
//!
//! Always build projections with these functions, never by hand.

use glam::{Mat4, Vec4};

use crate::Transform;

/// Depth clear value for reversed depth: the far end of the range.
pub const REVERSED_DEPTH_CLEAR: f32 = 0.0;

/// Invalid parameters for a projection.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ProjectionError {
    /// The vertical field of view must be finite and in (0, π) radians.
    #[error("vertical field of view must be in (0, π) radians, got {0}")]
    FieldOfView(f32),
    /// The aspect ratio (width / height) must be finite and positive.
    #[error("aspect ratio (width / height) must be finite and positive, got {0}")]
    AspectRatio(f32),
    /// The near plane distance must be finite and positive.
    #[error("near plane distance must be finite and positive, got {0}")]
    Near(f32),
    /// An orthographic volume needs `left != right`, `bottom != top` and `near < far`, all finite.
    #[error(
        "orthographic volume is empty or not finite: left {left}, right {right}, \
         bottom {bottom}, top {top}, near {near}, far {far}"
    )]
    OrthographicVolume {
        /// Left plane.
        left: f32,
        /// Right plane.
        right: f32,
        /// Bottom plane.
        bottom: f32,
        /// Top plane.
        top: f32,
        /// Near plane distance.
        near: f32,
        /// Far plane distance.
        far: f32,
    },
}

/// Perspective projection with **reversed depth and an infinite far plane**. It's the default
/// for scene rendering.
///
/// `vertical_fov` is in radians and `aspect_ratio` is width / height. A point at distance `near`
/// in front of the camera maps to depth 1, and depth approaches 0 as distance grows.
///
/// # Errors
///
/// Returns [`ProjectionError`] if any parameter is out of range or not finite.
pub fn perspective_infinite_reversed(
    vertical_fov: f32,
    aspect_ratio: f32,
    near: f32,
) -> Result<Mat4, ProjectionError> {
    if !(vertical_fov.is_finite() && vertical_fov > 0.0 && vertical_fov < std::f32::consts::PI) {
        return Err(ProjectionError::FieldOfView(vertical_fov));
    }
    if !(aspect_ratio.is_finite() && aspect_ratio > 0.0) {
        return Err(ProjectionError::AspectRatio(aspect_ratio));
    }
    if !(near.is_finite() && near > 0.0) {
        return Err(ProjectionError::Near(near));
    }
    let f = 1.0 / (0.5 * vertical_fov).tan();
    // clip = (x·f/aspect, y·f, near, −z_view), so after division ndc.z = near / distance.
    Ok(Mat4::from_cols(
        Vec4::new(f / aspect_ratio, 0.0, 0.0, 0.0),
        Vec4::new(0.0, f, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, -1.0),
        Vec4::new(0.0, 0.0, near, 0.0),
    ))
}

/// Orthographic projection with **reversed depth**: distance `near` maps to depth 1 and `far`
/// maps to 0. The planes are in view space, where the camera looks down −Z, so `near` and `far`
/// are positive distances in front of the camera.
///
/// # Errors
///
/// Returns [`ProjectionError::OrthographicVolume`] for an empty or non-finite volume.
pub fn orthographic_reversed(
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
    near: f32,
    far: f32,
) -> Result<Mat4, ProjectionError> {
    let values = [left, right, bottom, top, near, far];
    if !values.iter().all(|v| v.is_finite()) || left == right || bottom == top || near >= far {
        return Err(ProjectionError::OrthographicVolume {
            left,
            right,
            bottom,
            top,
            near,
            far,
        });
    }
    let (rl, tb, fnr) = (right - left, top - bottom, far - near);
    // ndc.z = (far + z_view) / (far − near): distance `near` → 1, distance `far` → 0.
    Ok(Mat4::from_cols(
        Vec4::new(2.0 / rl, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / tb, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0 / fnr, 0.0),
        Vec4::new(-(right + left) / rl, -(top + bottom) / tb, far / fnr, 1.0),
    ))
}

/// View matrix (world → view) for a camera placed by `camera`.
///
/// The camera looks along its local −Z ([`Transform::forward`]). Scale is ignored: cameras are
/// rigid, so scaling a camera entity never distorts the view.
#[must_use]
pub fn view_matrix(camera: &Transform) -> Mat4 {
    Mat4::from_rotation_translation(camera.rotation, camera.translation).inverse()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Vec3, axes};
    use glam::Quat;
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_3, PI};

    const EPS: f32 = 1e-5;

    /// Projects a view-space point and returns normalized device coordinates.
    fn ndc(m: &Mat4, p: Vec3) -> Vec3 {
        let clip = *m * p.extend(1.0);
        clip.truncate() / clip.w
    }

    fn assert_close(a: Vec3, b: Vec3) {
        assert!(a.abs_diff_eq(b, EPS), "{a} != {b}");
    }

    #[test]
    fn perspective_maps_near_plane_to_one_and_infinity_towards_zero() {
        let near = 0.1;
        let m = perspective_infinite_reversed(FRAC_PI_3, 16.0 / 9.0, near).unwrap();
        assert!((ndc(&m, Vec3::new(0.0, 0.0, -near)).z - 1.0).abs() < EPS);
        assert!((ndc(&m, Vec3::new(0.0, 0.0, -1.0)).z - near).abs() < EPS);
        let far_z = ndc(&m, Vec3::new(0.0, 0.0, -1.0e6)).z;
        assert!(far_z > 0.0 && far_z < 1e-6, "{far_z}");
    }

    #[test]
    fn perspective_is_y_up_and_respects_fov_and_aspect() {
        let (fov, aspect) = (FRAC_PI_2, 2.0);
        let m = perspective_infinite_reversed(fov, aspect, 0.1).unwrap();
        let d = 10.0;
        let half = d * (0.5 * fov).tan();
        // The top edge of the field of view is +1 in NDC (Y up), the bottom is −1.
        assert!((ndc(&m, Vec3::new(0.0, half, -d)).y - 1.0).abs() < EPS);
        assert!((ndc(&m, Vec3::new(0.0, -half, -d)).y + 1.0).abs() < EPS);
        // The right edge is widened by the aspect ratio.
        assert!((ndc(&m, Vec3::new(half * aspect, 0.0, -d)).x - 1.0).abs() < EPS);
    }

    #[test]
    fn perspective_matches_glams_vulkan_projection_with_y_flipped() {
        // An independent cross-check: glam's Vulkan variant is the same projection with
        // Y down. Flipping Y must reproduce ours exactly.
        let (fov, aspect, near) = (1.1, 1.6, 0.05);
        let ours = perspective_infinite_reversed(fov, aspect, near).unwrap();
        let vulkan =
            glam::camera::rh::proj::vulkan::perspective_infinite_reverse(fov, aspect, near);
        let flip_y = Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0));
        assert!(ours.abs_diff_eq(flip_y * vulkan, EPS), "{ours} vs {vulkan}");
    }

    #[test]
    fn perspective_rejects_invalid_parameters() {
        assert_eq!(
            perspective_infinite_reversed(0.0, 1.0, 0.1),
            Err(ProjectionError::FieldOfView(0.0))
        );
        assert_eq!(
            perspective_infinite_reversed(PI, 1.0, 0.1),
            Err(ProjectionError::FieldOfView(PI))
        );
        assert!(perspective_infinite_reversed(f32::NAN, 1.0, 0.1).is_err());
        assert_eq!(
            perspective_infinite_reversed(1.0, 0.0, 0.1),
            Err(ProjectionError::AspectRatio(0.0))
        );
        assert_eq!(
            perspective_infinite_reversed(1.0, 1.0, -0.1),
            Err(ProjectionError::Near(-0.1))
        );
        assert!(perspective_infinite_reversed(1.0, 1.0, f32::INFINITY).is_err());
    }

    #[test]
    fn orthographic_maps_the_volume_to_the_unit_clip_box() {
        let m = orthographic_reversed(-4.0, 6.0, -1.0, 3.0, 0.5, 100.0).unwrap();
        assert_close(
            ndc(&m, Vec3::new(-4.0, -1.0, -0.5)),
            Vec3::new(-1.0, -1.0, 1.0),
        );
        assert_close(
            ndc(&m, Vec3::new(6.0, 3.0, -100.0)),
            Vec3::new(1.0, 1.0, 0.0),
        );
        assert_close(
            ndc(&m, Vec3::new(1.0, 1.0, -50.25)),
            Vec3::new(0.0, 0.0, 0.5),
        );
    }

    #[test]
    fn orthographic_rejects_empty_or_non_finite_volumes() {
        assert!(orthographic_reversed(1.0, 1.0, 0.0, 1.0, 0.1, 10.0).is_err());
        assert!(orthographic_reversed(0.0, 1.0, 2.0, 2.0, 0.1, 10.0).is_err());
        assert!(orthographic_reversed(0.0, 1.0, 0.0, 1.0, 10.0, 10.0).is_err());
        assert!(orthographic_reversed(0.0, 1.0, 0.0, 1.0, 10.0, 1.0).is_err());
        assert!(orthographic_reversed(0.0, f32::NAN, 0.0, 1.0, 0.1, 10.0).is_err());
    }

    #[test]
    fn view_matrix_moves_the_world_in_front_of_the_camera() {
        // A camera at (0, 2, 10) looking at the origin sees the origin straight ahead.
        let camera =
            Transform::from_translation(Vec3::new(0.0, 2.0, 10.0)).looking_at(Vec3::ZERO, axes::UP);
        let view = view_matrix(&camera);
        let origin_in_view = view.transform_point3(Vec3::ZERO);
        let expected_distance = Vec3::new(0.0, 2.0, 10.0).length();
        assert_close(origin_in_view, Vec3::new(0.0, 0.0, -expected_distance));
    }

    #[test]
    fn view_matrix_ignores_camera_scale() {
        let rigid = Transform::from_translation(Vec3::new(1.0, 2.0, 3.0))
            .with_rotation(Quat::from_rotation_y(0.7));
        let scaled = rigid.with_scale(Vec3::new(3.0, 0.5, 2.0));
        assert!(view_matrix(&rigid).abs_diff_eq(view_matrix(&scaled), EPS));
    }

    #[test]
    fn errors_are_actionable() {
        let message = perspective_infinite_reversed(1.0, 0.0, 0.1)
            .unwrap_err()
            .to_string();
        assert!(message.contains("aspect ratio"), "{message}");
        assert!(message.contains('0'), "{message}");
    }
}
