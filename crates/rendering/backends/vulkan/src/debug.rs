//! Vulkan debug messages into Oxreal's log (ADR-0010, ADR-0012 §5).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};

use ash::vk;
use oxreal_diagnostics::{debug, error, trace, warn};

/// State shared with the debug callback through its user-data pointer.
///
/// It's boxed so its address is stable, and it must outlive the messenger and the instance.
/// `VulkanInstance` guarantees that.
#[derive(Debug, Default)]
pub(crate) struct DebugState {
    error_count: AtomicU64,
}

impl DebugState {
    /// Validation-layer errors reported so far.
    pub(crate) fn error_count(&self) -> u64 {
        self.error_count.load(Ordering::Relaxed)
    }
}

/// Messenger settings: every severity and message type, routed to [`debug_callback`].
pub(crate) fn messenger_create_info(
    state: &DebugState,
) -> vk::DebugUtilsMessengerCreateInfoEXT<'static> {
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                | vk::DebugUtilsMessageSeverityFlagsEXT::INFO
                | vk::DebugUtilsMessageSeverityFlagsEXT::VERBOSE,
        )
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(debug_callback))
        .user_data(std::ptr::from_ref(state).cast_mut().cast::<c_void>())
}

/// The log level a Vulkan message is reported at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Error,
    Warn,
    Debug,
    Trace,
}

pub(crate) fn level_for(severity: vk::DebugUtilsMessageSeverityFlagsEXT) -> Level {
    use vk::DebugUtilsMessageSeverityFlagsEXT as S;
    if severity.contains(S::ERROR) {
        Level::Error
    } else if severity.contains(S::WARNING) {
        Level::Warn
    } else if severity.contains(S::INFO) {
        Level::Debug
    } else {
        Level::Trace
    }
}

/// Called by the Vulkan loader and layers, possibly from any thread.
///
/// Never unwinds into Vulkan: a panic while logging is caught and dropped.
unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user_data: *mut c_void,
) -> vk::Bool32 {
    let report = std::panic::AssertUnwindSafe(|| {
        // SAFETY: Vulkan passes a valid callback-data pointer (or null) for the duration of this
        // call. `as_ref` handles null.
        let data = unsafe { data.as_ref() };
        // SAFETY: the message and ID name are null or valid NUL-terminated strings owned by the
        // caller for the duration of this call.
        let message = data
            .and_then(|d| unsafe { d.message_as_c_str() })
            .map_or_else(|| "(no message)".into(), |m| m.to_string_lossy());
        // SAFETY: as above.
        let id = data
            .and_then(|d| unsafe { d.message_id_name_as_c_str() })
            .map_or_else(|| "".into(), |m| m.to_string_lossy());
        let kind = format!("{types:?}");

        let level = level_for(severity);
        if level == Level::Error {
            // SAFETY: `user_data` is the `DebugState` registered in `messenger_create_info`.
            // `VulkanInstance` keeps it alive until after the messenger and instance are
            // destroyed, so it's valid for every callback.
            if let Some(state) = unsafe { user_data.cast::<DebugState>().as_ref() } {
                state.error_count.fetch_add(1, Ordering::Relaxed);
            }
        }
        match level {
            Level::Error => error!(target: "vulkan", %kind, %id, "{message}"),
            Level::Warn => warn!(target: "vulkan", %kind, %id, "{message}"),
            Level::Debug => debug!(target: "vulkan", %kind, %id, "{message}"),
            Level::Trace => trace!(target: "vulkan", %kind, %id, "{message}"),
        }
    });
    // A panic must never cross into the C caller (conventions.md).
    let _ = std::panic::catch_unwind(report);
    // Returning FALSE tells the layer not to abort the Vulkan call that triggered the message.
    vk::FALSE
}

#[cfg(test)]
mod tests {
    use super::*;
    use vk::DebugUtilsMessageSeverityFlagsEXT as S;

    #[test]
    fn severities_map_to_log_levels() {
        assert_eq!(level_for(S::ERROR), Level::Error);
        assert_eq!(level_for(S::WARNING), Level::Warn);
        assert_eq!(level_for(S::INFO), Level::Debug);
        assert_eq!(level_for(S::VERBOSE), Level::Trace);
        // Combined flags report at the most severe level.
        assert_eq!(level_for(S::WARNING | S::ERROR), Level::Error);
    }

    #[test]
    fn callback_counts_errors_and_survives_null_data() {
        let state = DebugState::default();
        let user = std::ptr::from_ref(&state).cast_mut().cast::<c_void>();
        // SAFETY: `state` outlives both calls. A null data pointer is permitted by the callback.
        unsafe {
            debug_callback(
                S::ERROR,
                vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION,
                std::ptr::null(),
                user,
            );
            debug_callback(
                S::WARNING,
                vk::DebugUtilsMessageTypeFlagsEXT::GENERAL,
                std::ptr::null(),
                user,
            );
        }
        assert_eq!(state.error_count(), 1);
    }
}
