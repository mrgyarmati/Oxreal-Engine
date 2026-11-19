//! Texture formats and descriptions.

use std::ops::BitOr;

/// Pixel formats the RHI supports. More are added as the renderer needs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextureFormat {
    /// 8-bit BGRA, sRGB-encoded. The usual swapchain format.
    Bgra8UnormSrgb,
    /// 8-bit RGBA, sRGB-encoded.
    Rgba8UnormSrgb,
    /// 8-bit BGRA, linear.
    Bgra8Unorm,
    /// 8-bit RGBA, linear.
    Rgba8Unorm,
    /// 32-bit float depth: Oxreal's scene depth format. Scene depth is **reversed** (near = 1,
    /// far = 0): clear it to [`oxreal_math::projection::REVERSED_DEPTH_CLEAR`] and compare with
    /// [`crate::CompareOp::GreaterOrEqual`] ([`crate::DepthState::SCENE`], ADR-0011 §4). Only
    /// [`TextureUsage::DEPTH_TARGET`] is supported for it so far.
    Depth32Float,
}

impl TextureFormat {
    /// Bytes per pixel.
    #[must_use]
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Bgra8UnormSrgb
            | Self::Rgba8UnormSrgb
            | Self::Bgra8Unorm
            | Self::Rgba8Unorm
            | Self::Depth32Float => 4,
        }
    }

    /// Whether the hardware sRGB-encodes writes and decodes reads (ADR-0011 §5).
    #[must_use]
    pub const fn is_srgb(self) -> bool {
        matches!(self, Self::Bgra8UnormSrgb | Self::Rgba8UnormSrgb)
    }

    /// Whether this is a depth format, used with [`TextureUsage::DEPTH_TARGET`] rather than as a
    /// color.
    #[must_use]
    pub const fn is_depth(self) -> bool {
        matches!(self, Self::Depth32Float)
    }
}

/// How a texture may be used. Combine flags with `|`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TextureUsage(u32);

impl TextureUsage {
    /// Rendered into as a color attachment.
    pub const RENDER_TARGET: Self = Self(1);
    /// Copied from, for example by [`crate::Device::read_texture`].
    pub const COPY_SOURCE: Self = Self(1 << 1);
    /// Sampled by shaders through its bindless index ([`crate::Device::texture_index`]). Between
    /// uses the texture rests in a shader-readable state, so any later work can sample it. Not
    /// during a render pass that draws into it: see
    /// [`ShaderCode::from_spirv_unchecked`](crate::ShaderCode::from_spirv_unchecked).
    pub const SAMPLED: Self = Self(1 << 2);
    /// Written by [`crate::Device::write_texture`].
    pub const COPY_DESTINATION: Self = Self(1 << 3);
    /// Rendered into as the depth attachment of a render pass ([`crate::DepthAttachment`]). Only
    /// for depth formats, which support no other usage yet.
    pub const DEPTH_TARGET: Self = Self(1 << 4);

    /// Whether every flag in `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for TextureUsage {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// How to create a texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureDesc<'a> {
    /// A name shown in validation messages and GPU debuggers.
    pub label: &'a str,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: TextureFormat,
    /// Allowed uses.
    pub usage: TextureUsage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_flags_combine() {
        let usage = TextureUsage::RENDER_TARGET | TextureUsage::COPY_SOURCE;
        assert!(usage.contains(TextureUsage::RENDER_TARGET));
        assert!(usage.contains(TextureUsage::COPY_SOURCE));
        assert!(!TextureUsage::RENDER_TARGET.contains(TextureUsage::COPY_SOURCE));
        assert!(TextureUsage::default().contains(TextureUsage::default()));
    }

    #[test]
    fn format_properties() {
        assert_eq!(TextureFormat::Rgba8Unorm.bytes_per_pixel(), 4);
        assert!(TextureFormat::Bgra8UnormSrgb.is_srgb());
        assert!(!TextureFormat::Rgba8Unorm.is_srgb());
        assert!(TextureFormat::Depth32Float.is_depth());
        assert!(!TextureFormat::Depth32Float.is_srgb());
        assert!(!TextureFormat::Bgra8UnormSrgb.is_depth());
        assert_eq!(TextureFormat::Depth32Float.bytes_per_pixel(), 4);
    }
}
