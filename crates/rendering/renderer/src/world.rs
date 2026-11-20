//! The render world: what to draw this frame, as plain data (brief rule 3, ADR-0019 §1).

use oxreal_math::{LinearRgba, Mat4, Transform};

use crate::MeshHandle;

/// The camera a frame is seen through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// The camera's placement in the world. It looks along its local forward axis (ADR-0011).
    pub transform: Transform,
    /// Vertical field of view, in radians. Must be in (0, π).
    pub vertical_fov: f32,
    /// Distance to the near plane, in metres. Must be positive. There's no far plane: depth is
    /// reversed and infinite (ADR-0011 §4).
    pub near: f32,
}

impl Default for Camera {
    /// At the origin, with a 60° field of view and a 5 cm near plane.
    fn default() -> Self {
        Self {
            transform: Transform::IDENTITY,
            vertical_fov: 60_f32.to_radians(),
            near: 0.05,
        }
    }
}

/// One mesh to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawItem {
    /// The mesh, from [`crate::Renderer::register_mesh`]. A handle that names no mesh any more
    /// is skipped and counted ([`crate::RenderStats::skipped`]), never drawn.
    pub mesh: MeshHandle,
    /// Object space to world space.
    pub object_to_world: Mat4,
}

/// Everything a frame draws. The game fills it each frame (in its `Extract` stage); the
/// renderer only reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderWorld {
    /// The camera.
    pub camera: Camera,
    /// The color the target is cleared to, linear (ADR-0011 §5).
    pub background: LinearRgba,
    /// The meshes to draw.
    pub draws: Vec<DrawItem>,
}

impl Default for RenderWorld {
    fn default() -> Self {
        Self {
            camera: Camera::default(),
            background: LinearRgba::new(0.02, 0.02, 0.025, 1.0),
            draws: Vec::new(),
        }
    }
}
