//! Oxreal's Rendering Hardware Interface ([ADR-0003], [ADR-0016]).
//!
//! **Layer:** rendering. This crate is backend-agnostic: it defines what a GPU device offers the
//! renderer, and contains **no backend types and no `unsafe`**. Backends such as
//! `oxreal-rhi-vulkan` implement it.
//!
//! What's here so far:
//! - [`Device`]: the trait every backend's device implements.
//! - [`DeviceDesc`], [`Validation`] and [`GpuSelection`]: how to create a device.
//! - [`AdapterInfo`] and [`select_adapter`]: deterministic GPU selection shared by every backend
//!   (ADR-0012 §6).
//! - [`RhiError`]: typed, actionable errors. Device loss is an error, never a panic.
//! - Surfaces, frames, command encoders and scoped render passes ([`Device::begin_frame`],
//!   [`RenderPass`]), as decided in ADR-0017.
//! - [`HandlePool`]: typed generational handles for backend objects (ADR-0016 §1).
//! - Textures, render pipelines (push constants up to [`MAX_PUSH_CONSTANT_BYTES`]), draws,
//!   standalone submissions and [`Device::read_texture`] for tests and tools.
//!
//! Buffers, pipelines and the bindless table arrive in the following slices.
//!
//! [ADR-0003]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0003-vulkan-first-graphics-backend.md
//! [ADR-0016]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0016-rhi-resource-and-bind-model.md

mod adapter;
mod command;
mod device;
mod error;
mod handle;
mod pipeline;
mod surface;
mod texture;

pub use adapter::{
    AdapterCandidate, AdapterInfo, AdapterType, ApiVersion, GPU_ENV_VAR, GpuSelection,
    select_adapter,
};
pub use command::{
    ColorAttachment, CommandEncoder, Frame, LoadOp, RenderPass, RenderPassDesc, StoreOp,
};
pub use device::{Device, DeviceDesc, Validation};
pub use error::{AdapterRejection, RhiError};
pub use handle::{HandlePool, PipelineHandle, SubmissionId, SurfaceHandle, TextureHandle};
pub use pipeline::{
    CullMode, MAX_PUSH_CONSTANT_BYTES, RenderPipelineDesc, ShaderCode, ShaderEntry,
};
pub use surface::{PresentMode, SurfaceDesc, SurfaceTarget};
pub use texture::{TextureDesc, TextureFormat, TextureUsage};

/// Re-export of the interop crate behind [`SurfaceTarget`].
pub use raw_window_handle;
