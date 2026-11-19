//! Vulkan backend of the Oxreal RHI ([ADR-0012], [ADR-0016]).
//!
//! **Layer:** rendering. This is the **only** crate that uses `ash` and Vulkan types. Everything
//! above it sees the [`oxreal_rhi`] API.
//!
//! What this slice provides:
//! - The Vulkan loader, loaded at runtime. A missing loader is a clear error, not a startup crash.
//! - The Vulkan 1.3 instance, with validation layers in debug builds and Vulkan messages routed to
//!   the log. Validation errors are counted, and tests assert zero.
//! - GPU selection against Oxreal's required features
//!   (`tools/ci/required-vulkan-features.txt`, kept in sync with this crate by a test).
//! - The logical device, a graphics+compute queue and the GPU memory allocator.
//!
//! Swapchains and the frame loop come in the next slice.
//!
//! # Environment variables
//!
//! - `OXREAL_GPU`: pick a GPU by index or name ([`oxreal_rhi::GpuSelection`]).
//! - `OXREAL_VULKAN_VALIDATION=0|1`: force validation layers off or on.
//!
//! [ADR-0012]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0012-vulkan-baseline-bindings-and-memory.md
//! [ADR-0016]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0016-rhi-resource-and-bind-model.md

mod debug;
mod device;
mod error;
mod features;
mod instance;

pub use device::{VulkanDevice, create_device};
pub use instance::VALIDATION_ENV_VAR;
