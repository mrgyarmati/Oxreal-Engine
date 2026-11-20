//! The render world: what to draw this frame, as plain data (brief rule 3, ADR-0019 §1).

use oxreal_math::projection::{ProjectionError, perspective_infinite_reversed};
use oxreal_math::{LinearRgba, Mat4, Transform};

use crate::MeshHandle;

/// How a camera projects the world onto the target.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Projection {
    /// Perspective with reversed, infinite depth (ADR-0011 §4): there's no far plane.
    Perspective {
        /// Vertical field of view, in radians. Must be in (0, π).
        vertical_fov: f32,
        /// Distance to the near plane, in metres. Must be positive.
        near: f32,
    },
}

impl Default for Projection {
    /// Perspective with a 60° field of view and a 5 cm near plane.
    fn default() -> Self {
        Self::Perspective {
            vertical_fov: 60_f32.to_radians(),
            near: 0.05,
        }
    }
}

impl Projection {
    /// View space to clip space, for a target with this width / height `aspect_ratio`.
    ///
    /// # Errors
    ///
    /// [`ProjectionError`] for settings that make no projection.
    pub fn clip_from_view(&self, aspect_ratio: f32) -> Result<Mat4, ProjectionError> {
        match *self {
            Self::Perspective { vertical_fov, near } => {
                perspective_infinite_reversed(vertical_fov, aspect_ratio, near)
            }
        }
    }
}

/// The camera a frame is seen through.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub struct Camera {
    /// The camera's placement in the world. It looks along its local forward axis, −Z
    /// (ADR-0011).
    pub transform: Transform,
    /// How it projects.
    pub projection: Projection,
}

impl Camera {
    /// A camera at `transform`, projecting with `projection`.
    #[must_use]
    pub const fn new(transform: Transform, projection: Projection) -> Self {
        Self {
            transform,
            projection,
        }
    }
}

/// One mesh to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct DrawItem {
    /// The mesh, from [`crate::Renderer::register_mesh`]. A handle that names no mesh any more
    /// is skipped and counted ([`crate::RenderStats::skipped`]), never drawn.
    pub mesh: MeshHandle,
    /// Object space to world space.
    pub object_to_world: Mat4,
}

impl DrawItem {
    /// Draws `mesh` placed by `object_to_world`.
    #[must_use]
    pub const fn new(mesh: MeshHandle, object_to_world: Mat4) -> Self {
        Self {
            mesh,
            object_to_world,
        }
    }
}

/// Everything a frame draws. The game fills it each frame (in its `Extract` stage); the
/// renderer only reads it. Start from [`RenderWorld::default`] and set the fields.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RenderWorld {
    /// The camera.
    pub camera: Camera,
    /// The color the target is cleared to, linear (ADR-0011 §5).
    pub background: LinearRgba,
    /// The meshes to draw.
    pub draws: Vec<DrawItem>,
}

impl Default for RenderWorld {
    /// A default camera, a near-black background, nothing to draw.
    fn default() -> Self {
        Self {
            camera: Camera::default(),
            background: LinearRgba::new(0.02, 0.02, 0.025, 1.0),
            draws: Vec::new(),
        }
    }
}
