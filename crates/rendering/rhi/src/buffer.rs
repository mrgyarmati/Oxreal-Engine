//! GPU buffers (ADR-0016 §2): shaders reach them by device address.

use std::ops::BitOr;

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
    }
}
