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
//!
//! Resources, the bindless model and command recording arrive in the following slices.
//!
//! [ADR-0003]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0003-vulkan-first-graphics-backend.md
//! [ADR-0016]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0016-rhi-resource-and-bind-model.md

mod adapter;
mod device;
mod error;

pub use adapter::{
    AdapterCandidate, AdapterInfo, AdapterType, ApiVersion, GPU_ENV_VAR, GpuSelection,
    select_adapter,
};
pub use device::{Device, DeviceDesc, Validation};
pub use error::{AdapterRejection, RhiError};
