//! The device trait and its creation parameters.

use std::fmt;
use std::sync::Arc;

use crate::{
    AdapterInfo, CommandEncoder, Frame, GpuSelection, PipelineHandle, RenderPipelineDesc, RhiError,
    SubmissionId, SurfaceDesc, SurfaceHandle, SurfaceTarget, TextureDesc, TextureHandle,
};

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

    /// Creates a presentation surface for `window`. The surface keeps the window alive until
    /// [`Device::destroy_surface`] or the device is dropped.
    ///
    /// # Errors
    ///
    /// [`RhiError::PresentationUnsupported`] if this GPU can't present to the window, or another
    /// [`RhiError`] if creation fails.
    fn create_surface(
        &self,
        window: Arc<dyn SurfaceTarget>,
        desc: &SurfaceDesc,
    ) -> Result<SurfaceHandle, RhiError>;

    /// Reports a new drawable size, in physical pixels, for example from
    /// `WindowEvent::Resized`. The swapchain is recreated at the next [`Device::begin_frame`].
    /// A zero size (minimized) pauses presentation.
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle.
    fn resize_surface(
        &self,
        surface: SurfaceHandle,
        width: u32,
        height: u32,
    ) -> Result<(), RhiError>;

    /// Destroys a surface, after the GPU has finished with it, and releases its window.
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle, or [`RhiError::DeviceLost`].
    fn destroy_surface(&self, surface: SurfaceHandle) -> Result<(), RhiError>;

    /// Starts a frame on `surface`. Returns `Ok(None)` when nothing should be drawn this time,
    /// for example while the window is minimized or the swapchain is being recreated. Skip the
    /// frame and try again later (ADR-0017 §5).
    ///
    /// # Errors
    ///
    /// [`RhiError::DeviceLost`], [`RhiError::InvalidUsage`] for a stale handle, or another
    /// [`RhiError`] from the backend.
    fn begin_frame(&self, surface: SurfaceHandle) -> Result<Option<Frame>, RhiError>;

    /// Creates a texture in GPU memory. Its contents are undefined until rendered to.
    ///
    /// # Errors
    ///
    /// [`RhiError::OutOfMemory`], or [`RhiError::InvalidUsage`] for a zero size or an empty
    /// usage.
    fn create_texture(&self, desc: &TextureDesc<'_>) -> Result<TextureHandle, RhiError>;

    /// Destroys a texture once the GPU has finished every submission that may use it
    /// (deferred destruction, ADR-0016 §1). The handle is invalid immediately.
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle or a swapchain backbuffer.
    fn destroy_texture(&self, texture: TextureHandle) -> Result<(), RhiError>;

    /// Creates a render pipeline.
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for invalid descriptions (for example too many push-constant
    /// bytes), or [`RhiError::Backend`] if the driver rejects the shaders.
    fn create_render_pipeline(
        &self,
        desc: &RenderPipelineDesc<'_>,
    ) -> Result<PipelineHandle, RhiError>;

    /// Destroys a pipeline once the GPU has finished every submission that may use it.
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle.
    fn destroy_render_pipeline(&self, pipeline: PipelineHandle) -> Result<(), RhiError>;

    /// Starts recording commands outside the frame loop, for offscreen work such as tests,
    /// thumbnails and captures. Hand the encoder to [`Device::submit`].
    ///
    /// # Errors
    ///
    /// [`RhiError::OutOfMemory`] or another backend error.
    fn create_encoder(&self, label: &str) -> Result<Box<dyn CommandEncoder>, RhiError>;

    /// Submits commands recorded by an encoder from [`Device::create_encoder`].
    ///
    /// # Errors
    ///
    /// [`RhiError::DeviceLost`] / [`RhiError::OutOfMemory`], or [`RhiError::InvalidUsage`] if
    /// misuse was detected while recording (the commands are still submitted).
    fn submit(&self, encoder: Box<dyn CommandEncoder>) -> Result<SubmissionId, RhiError>;

    /// Blocks until the GPU has finished `submission`.
    ///
    /// # Errors
    ///
    /// [`RhiError::DeviceLost`].
    fn wait_for_submission(&self, submission: SubmissionId) -> Result<(), RhiError>;

    /// Copies a texture's pixels to the CPU: tightly packed rows, top row first, in the
    /// texture's format. Blocks until done. Meant for tests, tools and screenshots, not
    /// per-frame use. The texture needs [`crate::TextureUsage::COPY_SOURCE`].
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle or missing usage, or a device error.
    fn read_texture(&self, texture: TextureHandle) -> Result<Vec<u8>, RhiError>;

    /// Submits the frame's commands and presents its backbuffer.
    ///
    /// # Errors
    ///
    /// [`RhiError::DeviceLost`] / [`RhiError::OutOfMemory`] from submission or presentation, or
    /// [`RhiError::InvalidUsage`] if misuse was detected while recording. The frame is still
    /// submitted in that case, so presentation continues.
    fn end_frame(&self, frame: Frame) -> Result<(), RhiError>;
}
