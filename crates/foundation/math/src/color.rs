//! Color in linear RGB (ADR-0011 §5).
//!
//! All color math happens in **linear** RGB. sRGB-encoded values exist only at storage and
//! display boundaries: textures, swapchain formats, and values people type or pick. Convert at
//! those boundaries with [`LinearRgba::from_srgb`] or [`LinearRgba::to_srgb`].

/// A color in linear RGB with straight (not premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LinearRgba {
    /// Red, linear.
    pub r: f32,
    /// Green, linear.
    pub g: f32,
    /// Blue, linear.
    pub b: f32,
    /// Alpha (coverage). Never gamma-encoded.
    pub a: f32,
}

impl LinearRgba {
    /// Opaque black.
    pub const BLACK: Self = Self::new(0.0, 0.0, 0.0, 1.0);
    /// Opaque white.
    pub const WHITE: Self = Self::new(1.0, 1.0, 1.0, 1.0);
    /// Fully transparent black.
    pub const TRANSPARENT: Self = Self::new(0.0, 0.0, 0.0, 0.0);

    /// A color from linear components.
    #[must_use]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Converts sRGB-encoded components in `[0, 1]` (as people pick them) to linear. Alpha is
    /// copied unchanged.
    #[must_use]
    pub fn from_srgb(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self::new(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a)
    }

    /// Converts 8-bit sRGB components (for example `#RRGGBBAA` colors) to linear.
    #[must_use]
    pub fn from_srgb8(r: u8, g: u8, b: u8, a: u8) -> Self {
        let unit = |c: u8| f32::from(c) / 255.0;
        Self::from_srgb(unit(r), unit(g), unit(b), unit(a))
    }

    /// The sRGB-encoded components `[r, g, b, a]`, for display or storage. Alpha is unchanged.
    #[must_use]
    pub fn to_srgb(self) -> [f32; 4] {
        [
            linear_to_srgb(self.r),
            linear_to_srgb(self.g),
            linear_to_srgb(self.b),
            self.a,
        ]
    }

    /// The linear components as `[r, g, b, a]`, for example for GPU upload or clear values.
    #[must_use]
    pub const fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

/// sRGB electro-optical transfer function (IEC 61966-2-1): encoded value to linear light.
#[must_use]
pub fn srgb_to_linear(encoded: f32) -> f32 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// The inverse of [`srgb_to_linear`]: linear light to the sRGB-encoded value.
#[must_use]
pub fn linear_to_srgb(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_function_reference_values() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        // Mid-grey: sRGB 0.5 is about 21.4 % linear light (reference value 0.214041).
        assert!((srgb_to_linear(0.5) - 0.214_041_14).abs() < 1e-6);
        // The linear segment below the threshold.
        assert!((srgb_to_linear(0.04) - 0.04 / 12.92).abs() < 1e-9);
    }

    #[test]
    fn every_8_bit_value_round_trips() {
        for c in 0..=255_u8 {
            let encoded = f32::from(c) / 255.0;
            let back = linear_to_srgb(srgb_to_linear(encoded));
            assert!((back - encoded).abs() < 1e-5, "{c}: {encoded} -> {back}");
        }
    }

    #[test]
    fn conversion_is_monotonic() {
        let mut previous = -1.0;
        for c in 0..=255_u8 {
            let linear = srgb_to_linear(f32::from(c) / 255.0);
            assert!(linear > previous, "not increasing at {c}");
            previous = linear;
        }
    }

    #[test]
    fn alpha_is_never_gamma_converted() {
        let color = LinearRgba::from_srgb8(255, 128, 0, 128);
        assert!((color.a - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(color.to_srgb()[3], color.a);
        assert_eq!(LinearRgba::WHITE.to_array(), [1.0, 1.0, 1.0, 1.0]);
    }
}
