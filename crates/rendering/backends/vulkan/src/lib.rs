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
//! - Surfaces and swapchains (recreated internally on resize, out-of-date and minimize) and
//!   the frame loop: two frames in flight paced by one timeline semaphore, per-image present
//!   semaphores, synchronization2 barriers and dynamic rendering (ADR-0017).
//!
//! - Textures in GPU memory, render pipelines sharing one push-constant layout, draws with a
//!   Y-up viewport (ADR-0011), standalone submissions and texture readback. Destruction is
//!   deferred until the GPU and every encoder still recording are done with the object.
//!
//! See `examples/clear.rs` for the frame loop, and `examples/triangle.rs` for a first draw.
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
mod encoder;
mod error;
mod features;
mod instance;
mod present;
mod resources;
mod retire;

pub use device::{VulkanDevice, create_device};
pub use instance::VALIDATION_ENV_VAR;
