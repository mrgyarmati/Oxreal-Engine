//! The device trait and its creation parameters.

use std::fmt;

use crate::{AdapterInfo, GpuSelection, RhiError};

/// Whether to enable the graphics API's validation layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Validation {
    /// On in debug builds when the validation layers are installed, otherwise off. Backends also
    /// honour their environment override (`OXREAL_VULKAN_VALIDATION=0|1` for Vulkan).
    #[default]
    Automatic,
    /// Always on. A warning is logged if the layers aren't installed.
    Enabled,
    /// Always off.
    Disabled,
}

/// How to create a [`Device`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDesc {
    /// The application's name, reported to drivers and tools.
    pub app_name: String,
    /// Validation layer policy.
    pub validation: Validation,
    /// Which GPU to use. [`GpuSelection::from_env_or`] reads `OXREAL_GPU`.
    pub gpu: GpuSelection,
}

impl Default for DeviceDesc {
    fn default() -> Self {
        Self {
            app_name: "Oxreal".to_owned(),
            validation: Validation::Automatic,
            gpu: GpuSelection::Automatic,
        }
    }
}

/// A GPU device. Every backend implements this.
///
/// Dropping a device waits for the GPU to finish and releases everything it owns.
pub trait Device: fmt::Debug + Send + Sync {
    /// The adapter this device runs on.
    fn adapter_info(&self) -> &AdapterInfo;

    /// Whether validation layers are active.
    fn validation_enabled(&self) -> bool;

    /// How many validation-layer **errors** have been reported since creation. Tests assert that
    /// this stays 0 (ADR-0012 §5).
    fn validation_error_count(&self) -> u64;

    /// Blocks until the GPU has finished all submitted work.
    ///
    /// # Errors
    ///
    /// [`RhiError::DeviceLost`] if the device was lost.
    fn wait_idle(&self) -> Result<(), RhiError>;
}
