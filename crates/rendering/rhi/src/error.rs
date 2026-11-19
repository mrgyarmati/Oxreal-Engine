//! RHI errors.

use std::fmt;

use crate::GpuSelection;

/// Why an adapter (GPU) was not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterRejection {
    /// The adapter's name, as reported by the driver.
    pub name: String,
    /// Every reason it was rejected, for example `missing feature descriptorIndexing`.
    pub reasons: Vec<String>,
}

impl fmt::Display for AdapterRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name, self.reasons.join("; "))
    }
}

/// An error from the RHI or one of its backends.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RhiError {
    /// The graphics API's runtime (for example the Vulkan loader) is not installed.
    #[error("{api} runtime not found: install or update the GPU driver ({details})")]
    RuntimeUnavailable {
        /// The graphics API, for example `Vulkan`.
        api: &'static str,
        /// What the loader reported.
        details: String,
    },
    /// The runtime is older than Oxreal's baseline.
    #[error(
        "{api} {found} is installed, but Oxreal requires {required} or newer: update the GPU driver"
    )]
    RuntimeTooOld {
        /// The graphics API.
        api: &'static str,
        /// The version found.
        found: String,
        /// The minimum version required.
        required: String,
    },
    /// No adapter meets the requirements.
    #[error(
        "no usable GPU found. Every adapter was rejected:\n  - {}\n\
         Update the GPU driver, or see docs/development/gpu-support.md",
        format_rejections(.rejected)
    )]
    NoSuitableAdapter {
        /// Every adapter considered, with its reasons.
        rejected: Vec<AdapterRejection>,
    },
    /// `OXREAL_GPU` (or the requested selection) matches no usable adapter.
    #[error(
        "the requested GPU ({selection}) matches no usable adapter; usable adapters: {available}"
    )]
    AdapterNotFound {
        /// What was requested.
        selection: GpuSelection,
        /// Names of the usable adapters.
        available: String,
    },
    /// An `OXREAL_*` environment variable holds an invalid value.
    #[error("invalid {variable} value {value:?}: expected {expected}")]
    InvalidEnvironment {
        /// The environment variable.
        variable: &'static str,
        /// The rejected value, lossily converted if it wasn't valid Unicode.
        value: String,
        /// What would be valid.
        expected: &'static str,
    },
    /// The GPU device was lost (driver reset, hang, removal). Every resource is invalid.
    #[error("GPU device lost during {operation}: the driver reset or the device was removed")]
    DeviceLost {
        /// The operation that observed the loss.
        operation: &'static str,
    },
    /// The GPU or the host ran out of memory.
    #[error("out of {kind} memory during {operation}")]
    OutOfMemory {
        /// `GPU` or `host`.
        kind: &'static str,
        /// The operation that failed.
        operation: &'static str,
    },
    /// Any other failure reported by the backend.
    #[error("{api} call {operation} failed: {details}")]
    Backend {
        /// The graphics API.
        api: &'static str,
        /// The operation that failed.
        operation: &'static str,
        /// What the backend reported.
        details: String,
    },
}

fn format_rejections(rejected: &[AdapterRejection]) -> String {
    if rejected.is_empty() {
        return "(no adapters reported by the driver)".to_owned();
    }
    rejected
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n  - ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_suitable_adapter_lists_every_rejection_with_reasons() {
        let err = RhiError::NoSuitableAdapter {
            rejected: vec![
                AdapterRejection {
                    name: "Old GPU".into(),
                    reasons: vec![
                        "Vulkan 1.2.0 < 1.3".into(),
                        "missing feature descriptorIndexing".into(),
                    ],
                },
                AdapterRejection {
                    name: "llvmpipe".into(),
                    reasons: vec!["missing feature samplerAnisotropy".into()],
                },
            ],
        };
        let message = err.to_string();
        assert!(
            message.contains("Old GPU: Vulkan 1.2.0 < 1.3; missing feature descriptorIndexing")
        );
        assert!(message.contains("llvmpipe: missing feature samplerAnisotropy"));
        assert!(message.contains("gpu-support.md"));
    }

    #[test]
    fn no_adapters_at_all_is_explained() {
        let err = RhiError::NoSuitableAdapter { rejected: vec![] };
        assert!(err.to_string().contains("no adapters reported"), "{err}");
    }
}
