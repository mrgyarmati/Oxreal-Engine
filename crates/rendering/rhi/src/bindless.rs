//! The bindless binding model (ADR-0016 §2): shaders reach textures and samplers by index.
//!
//! Every pipeline sees one global descriptor set. Its layout is part of the RHI contract, and
//! `shaders/Bindless.slang` declares the same thing for shaders:
//!
//! | Set | Binding | Contents |
//! |---|---|---|
//! | [`BINDLESS_SET`] | [`TEXTURES_BINDING`] | `Texture2D[]`: every texture with [`crate::TextureUsage::SAMPLED`], at its [`crate::Device::texture_index`] |
//! | [`BINDLESS_SET`] | [`SAMPLERS_BINDING`] | `SamplerState[]`: every sampler, at its [`crate::Device::sampler_index`] |
//!
//! Indices travel to shaders in push constants or buffers. Like buffer addresses, they are plain
//! numbers: a shader that samples a destroyed texture's or sampler's index reads whatever now
//! occupies it (or nothing), so using only live indices is part of
//! [`ShaderCode::from_spirv_unchecked`](crate::ShaderCode::from_spirv_unchecked)'s contract. An
//! index is reused only after its object is really destroyed (deferred destruction), so it never
//! refers to two objects the GPU could see at once.

/// The descriptor set every pipeline's bindless resources live in.
pub const BINDLESS_SET: u32 = 0;

/// The binding of the sampled-texture array (`Texture2D[]`).
pub const TEXTURES_BINDING: u32 = 0;

/// The binding of the sampler array (`SamplerState[]`).
pub const SAMPLERS_BINDING: u32 = 1;

/// How a sampler filters texels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    /// The nearest texel. Crisp pixels.
    Nearest,
    /// Bilinear interpolation between the four nearest texels.
    #[default]
    Linear,
}

/// What a sampler returns for coordinates outside `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AddressMode {
    /// Tile the texture.
    #[default]
    Repeat,
    /// Tile, mirroring every other tile.
    MirroredRepeat,
    /// Repeat the edge texels.
    ClampToEdge,
}

/// How to create a sampler.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplerDesc<'a> {
    /// A name shown in validation messages and GPU debuggers.
    pub label: &'a str,
    /// Filtering when the texture is magnified (one texel covers several pixels).
    pub mag_filter: Filter,
    /// Filtering when the texture is minified (several texels per pixel).
    pub min_filter: Filter,
    /// Addressing along U (horizontal) and V (vertical).
    pub address_mode: AddressMode,
    /// Anisotropic filtering, as the maximum anisotropy (2 to 16, clamped to the device's limit).
    /// `None` for none.
    pub max_anisotropy: Option<f32>,
}

impl Default for SamplerDesc<'_> {
    fn default() -> Self {
        Self {
            label: "sampler",
            mag_filter: Filter::Linear,
            min_filter: Filter::Linear,
            address_mode: AddressMode::Repeat,
            max_anisotropy: None,
        }
    }
}
