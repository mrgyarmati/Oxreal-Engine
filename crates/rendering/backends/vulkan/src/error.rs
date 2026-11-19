//! Mapping Vulkan results to RHI errors.

use ash::vk;
use oxreal_rhi::RhiError;

/// Converts a failed Vulkan call into an [`RhiError`], keeping device loss and out-of-memory
/// distinguishable (ADR-0012 §7).
pub(crate) fn from_vk(operation: &'static str, result: vk::Result) -> RhiError {
    match result {
        vk::Result::ERROR_DEVICE_LOST => RhiError::DeviceLost { operation },
        vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => RhiError::OutOfMemory {
            kind: "GPU",
            operation,
        },
        vk::Result::ERROR_OUT_OF_HOST_MEMORY => RhiError::OutOfMemory {
            kind: "host",
            operation,
        },
        other => RhiError::Backend {
            api: "Vulkan",
            operation,
            details: format!("{other:?}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_loss_and_memory_errors_keep_their_meaning() {
        assert!(matches!(
            from_vk("vkQueueSubmit", vk::Result::ERROR_DEVICE_LOST),
            RhiError::DeviceLost {
                operation: "vkQueueSubmit"
            }
        ));
        assert!(matches!(
            from_vk("vkAllocateMemory", vk::Result::ERROR_OUT_OF_DEVICE_MEMORY),
            RhiError::OutOfMemory { kind: "GPU", .. }
        ));
        let other = from_vk("vkCreateDevice", vk::Result::ERROR_INITIALIZATION_FAILED);
        assert!(
            other.to_string().contains("ERROR_INITIALIZATION_FAILED"),
            "{other}"
        );
    }
}
