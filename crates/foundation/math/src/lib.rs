//! Math for Oxreal ([ADR-0011]).
//!
//! **Layer:** foundation. All engine and gameplay code imports math from this crate, never from
//! `glam` directly, so the dependency stays in one place.
//!
//! # Conventions
//!
//! | Topic | Convention |
//! |---|---|
//! | Axes | Right-handed, **+Y up**, +X right, **forward is −Z** ([`axes`]) |
//! | Units | SI: metres, kilograms, seconds |
//! | Rotations / angles | Unit quaternions. Radians in code, degrees in UI and files. |
//! | Matrices | Column-major, column vectors (`v' = M * v`), `world = parent * local` |
//! | Clip space | Y up, depth in [0, 1], identical on every graphics backend |
//! | Depth | **Reversed** (near = 1, far = 0) with an infinite far plane ([`projection`]) |
//! | Color | Linear RGB in all math. sRGB only at storage and display ([`color`]). |
//!
//! # Example: a camera
//!
//! ```
//! use oxreal_math::{Transform, Vec3, axes, projection};
//!
//! // A camera two metres up and five metres back, looking at the origin.
//! let camera = Transform::from_translation(Vec3::new(0.0, 2.0, 5.0)).looking_at(Vec3::ZERO, axes::UP);
//!
//! let view = projection::view_matrix(&camera);
//! let proj = projection::perspective_infinite_reversed(60_f32.to_radians(), 16.0 / 9.0, 0.1)?;
//! let view_proj = proj * view;
//!
//! // The origin is straight ahead: centre of the screen, between the near plane (1) and infinity (0).
//! let clip = view_proj * Vec3::ZERO.extend(1.0);
//! let ndc = clip.truncate() / clip.w;
//! assert!(ndc.x.abs() < 1e-5 && ndc.y.abs() < 1e-5);
//! assert!(ndc.z > 0.0 && ndc.z < 1.0);
//! # Ok::<(), oxreal_math::projection::ProjectionError>(())
//! ```
//!
//! [ADR-0011]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0011-math-library-and-coordinate-conventions.md

pub use glam::{
    Affine3A, BVec2, BVec3, BVec4, DMat4, DQuat, DVec2, DVec3, DVec4, EulerRot, IVec2, IVec3,
    IVec4, Mat2, Mat3, Mat3A, Mat4, Quat, UVec2, UVec3, UVec4, Vec2, Vec3, Vec3A, Vec4, dvec3,
    ivec2, ivec3, uvec2, uvec3, vec2, vec3, vec4,
};

pub mod axes;
pub mod color;
pub mod projection;
mod transform;

pub use color::LinearRgba;
pub use transform::Transform;
