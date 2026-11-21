//! The small enums shared by definitions and descriptions.

/// A primitive type (ADR-0021 §3).
///
/// `usize`, `isize`, `char` and 128-bit integers aren't primitives: their size depends on the
/// platform, or no file format and C-ABI caller needs them yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Primitive {
    /// `bool`. Its only valid bytes are 0 and 1, so it's never ABI-safe.
    Bool,
    /// `i8`.
    I8,
    /// `i16`.
    I16,
    /// `i32`.
    I32,
    /// `i64`.
    I64,
    /// `u8`.
    U8,
    /// `u16`.
    U16,
    /// `u32`.
    U32,
    /// `u64`.
    U64,
    /// `f32`.
    F32,
    /// `f64`.
    F64,
}

/// The explicit integer representation of a unit enum: `#[repr(u8)]`, `#[repr(u16)]` or
/// `#[repr(u32)]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntRepr {
    /// `#[repr(u8)]`.
    U8,
    /// `#[repr(u16)]`.
    U16,
    /// `#[repr(u32)]`.
    U32,
}

impl IntRepr {
    /// The largest discriminant the representation holds.
    #[must_use]
    pub const fn max(self) -> u32 {
        match self {
            Self::U8 => u8::MAX as u32,
            Self::U16 => u16::MAX as u32,
            Self::U32 => u32::MAX,
        }
    }
}
