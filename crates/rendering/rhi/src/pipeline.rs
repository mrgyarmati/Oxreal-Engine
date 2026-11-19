//! Render pipelines (ADR-0016 §2, ADR-0017).

use crate::TextureFormat;

/// The largest push-constant block a pipeline may use: the minimum every conformant Vulkan device
/// guarantees (ADR-0016 §2).
pub const MAX_PUSH_CONSTANT_BYTES: u32 = 128;

/// Compiled shader code in a backend's format. Produced by `oxreal-shader-compiler` (tools) or
/// loaded from the cooked shader cache (games).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShaderCode<'a> {
    /// SPIR-V words, for the Vulkan backend.
    Spirv(&'a [u32]),
}

/// One shader stage of a pipeline: code and the entry point to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderEntry<'a> {
    /// The compiled module.
    pub code: ShaderCode<'a>,
    /// The entry point's name, as written in the shader.
    pub entry_point: &'a str,
}

/// Which triangles are discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CullMode {
    /// Draw both sides.
    #[default]
    None,
    /// Discard triangles facing away from the camera. Front faces wind **counter-clockwise** as
    /// seen by the camera, in Oxreal's right-handed, Y-up convention (ADR-0011).
    Back,
    /// Discard triangles facing the camera.
    Front,
}

/// How to create a render pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderPipelineDesc<'a> {
    /// A name shown in validation messages and GPU debuggers.
    pub label: &'a str,
    /// The vertex stage.
    pub vertex: ShaderEntry<'a>,
    /// The pixel stage.
    pub pixel: ShaderEntry<'a>,
    /// Formats of the color attachments it renders to, in order.
    pub color_formats: &'a [TextureFormat],
    /// Bytes of push constants the shaders read, at most [`MAX_PUSH_CONSTANT_BYTES`] and a
    /// multiple of 4. Zero for none.
    pub push_constant_bytes: u32,
    /// Face culling.
    pub cull: CullMode,
}
