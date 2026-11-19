//! Render pipelines (ADR-0016 §2, ADR-0017).

use crate::TextureFormat;

/// The largest push-constant block a pipeline may use: the minimum every conformant Vulkan device
/// guarantees (ADR-0016 §2).
pub const MAX_PUSH_CONSTANT_BYTES: u32 = 128;

/// Compiled shader code in a backend's format. Produced by `oxreal-shader-compiler` (tools) or
/// loaded from the cooked shader cache (games).
///
/// Creating one is `unsafe`. GPU drivers trust shader code: malformed SPIR-V can crash or corrupt
/// the driver, and no practical check fully rules that out. Backends still check what they can
/// (header, entry points, push-constant size) and report problems as errors. The caller
/// guarantees the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderCode<'a> {
    repr: Repr<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Repr<'a> {
    Spirv(&'a [u32]),
}

impl<'a> ShaderCode<'a> {
    /// SPIR-V words, for the Vulkan backend.
    ///
    /// # Safety
    ///
    /// - `words` must be a valid SPIR-V module for Vulkan 1.3 with the features Oxreal requires, as
    ///   `spirv-val --target-env vulkan1.3 --scalar-block-layout` defines it. Output of
    ///   `oxreal-shader-compiler` (the pinned `slangc`) meets this, and CI validates it with
    ///   `spirv-val`. Words from anywhere else must be validated before use.
    /// - It may only use what Oxreal's required device features guarantee. `spirv-val` doesn't
    ///   check this. Backends reject capabilities outside the allowlist (no `Int64`, so no 64-bit
    ///   address arithmetic). Vertex and pixel shaders must not write or use atomics through
    ///   buffer addresses (`vertexPipelineStoresAndAtomics` and `fragmentStoresAndAtomics`
    ///   aren't required).
    /// - Shaders that dereference buffer addresses ([`crate::Device::buffer_address`]) must only be
    ///   given addresses of live buffers, read in bounds and aligned to the type they read. A
    ///   buffer is live for work recorded by an encoder created before the buffer was destroyed
    ///   (until that encoder's work completes), and never for an encoder created after.
    /// - Shaders that sample through bindless indices ([`crate::Device::texture_index`],
    ///   [`crate::Device::sampler_index`]) must only be given indices those calls returned for
    ///   live objects. A texture index goes to the texture array and a sampler index to the
    ///   sampler array. "Live" means the same as for addresses: the object wasn't destroyed before
    ///   the encoder that uses the index was created. Indices that can differ between invocations
    ///   must be marked non-uniform (`NonUniformResourceIndex`; `Bindless.slang` does).
    ///   `spirv-val` doesn't check this.
    /// - A draw must not sample a texture that is a color attachment of the render pass it's in.
    ///   During the pass the texture is being written, and isn't in a shader-readable state.
    ///   Sample it in a later pass instead.
    /// - Addresses and indices are plain numbers: the device can't check what a shader does with
    ///   them, and push constants that carry them are set through safe calls. So these are
    ///   **program-wide** obligations: whoever creates `ShaderCode` for such a shader also vouches
    ///   that every address and index the program passes it is valid (ADR-0016 §2).
    #[must_use]
    pub const unsafe fn from_spirv_unchecked(words: &'a [u32]) -> Self {
        Self {
            repr: Repr::Spirv(words),
        }
    }

    /// The SPIR-V words, if this is SPIR-V code.
    #[must_use]
    pub const fn as_spirv(&self) -> Option<&'a [u32]> {
        match self.repr {
            Repr::Spirv(words) => Some(words),
        }
    }
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
