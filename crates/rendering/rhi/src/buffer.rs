//! GPU buffers (ADR-0016 §2): shaders reach them by device address.

use std::ops::BitOr;

/// Every buffer's device address ([`crate::Device::buffer_address`]) is a multiple of this many
/// bytes, so a shader may read any type up to 16 bytes, aligned to its size, at offset 0 and at
/// offsets that are multiples of 16.
pub const BUFFER_ADDRESS_ALIGNMENT: u64 = 16;

/// How a buffer may be used. Combine flags with `|`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BufferUsage(u32);

impl BufferUsage {
    /// Read by shaders through its device address ([`crate::Device::buffer_address`]). Vertex and
    /// pixel shaders may not write through addresses (Oxreal doesn't require the device features
    /// for that); shader writes come with compute.
    pub const SHADER_ADDRESS: Self = Self(1);
    /// Written by [`crate::Device::write_buffer`].
    pub const COPY_DESTINATION: Self = Self(1 << 1);
    /// Read back by [`crate::Device::read_buffer`].
    pub const COPY_SOURCE: Self = Self(1 << 2);
    /// Bound as the index buffer of indexed draws ([`crate::RenderPass::set_index_buffer`]).
    pub const INDEX: Self = Self(1 << 3);

    /// Whether every flag in `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for BufferUsage {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// The element type of an index buffer.
///
/// Every value is a valid index, including the largest of its type: primitive restart is off,
/// so `0xFFFF` and `0xFFFF_FFFF` are ordinary indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexFormat {
    /// 16-bit unsigned indices, for meshes with at most 65 536 vertices.
    Uint16,
    /// 32-bit unsigned indices.
    Uint32,
}

impl IndexFormat {
    /// Bytes per index.
    #[must_use]
    pub const fn size(self) -> u64 {
        match self {
            Self::Uint16 => 2,
            Self::Uint32 => 4,
        }
    }
}

/// How to create a buffer. Buffers live in GPU memory, and their contents are undefined until
/// written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferDesc<'a> {
    /// A name shown in validation messages and GPU debuggers.
    pub label: &'a str,
    /// Size in bytes. Not zero, and at most the device's maximum buffer size.
    pub size: u64,
    /// Allowed uses.
    pub usage: BufferUsage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_flags_combine() {
        let usage = BufferUsage::SHADER_ADDRESS | BufferUsage::COPY_DESTINATION;
        assert!(usage.contains(BufferUsage::SHADER_ADDRESS));
        assert!(usage.contains(BufferUsage::COPY_DESTINATION));
        assert!(!usage.contains(BufferUsage::COPY_SOURCE));
        assert!(!usage.contains(BufferUsage::INDEX));
        assert!((usage | BufferUsage::INDEX).contains(BufferUsage::INDEX));
    }

    #[test]
    fn index_sizes() {
        assert_eq!(IndexFormat::Uint16.size(), 2);
        assert_eq!(IndexFormat::Uint32.size(), 4);
    }
}
