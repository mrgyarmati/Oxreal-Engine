//! The device trait and its creation parameters.

use std::fmt;
use std::sync::Arc;

use crate::{
    AdapterInfo, BufferDesc, BufferHandle, CommandEncoder, Frame, GpuSelection, PipelineHandle,
    RenderPipelineDesc, RhiError, SamplerDesc, SamplerHandle, SubmissionId, SurfaceDesc,
    SurfaceHandle, SurfaceTarget, TextureDesc, TextureHandle,
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
    /// [`RhiError::InvalidUsage`] for a stale handle or while a frame of the surface is open
    /// (end it first), or [`RhiError::DeviceLost`].
    fn destroy_surface(&self, surface: SurfaceHandle) -> Result<(), RhiError>;

    /// Starts a frame on `surface`. Returns `Ok(None)` when nothing should be drawn this time,
    /// for example while the window is minimized or the swapchain is being recreated. Skip the
    /// frame and try again later (ADR-0017 §5).
    ///
    /// # Errors
    ///
    /// [`RhiError::DeviceLost`], [`RhiError::InvalidUsage`] for a stale handle or while another
    /// frame of the surface is still open (each surface has at most one), or another
    /// [`RhiError`] from the backend.
    fn begin_frame(&self, surface: SurfaceHandle) -> Result<Option<Frame>, RhiError>;

    /// Creates a texture in GPU memory. Its contents are undefined until rendered to.
    ///
    /// # Errors
    ///
    /// [`RhiError::OutOfMemory`], or [`RhiError::InvalidUsage`] for a zero size, a size above the
    /// device's limit, or an empty usage.
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
    /// [`RhiError::InvalidUsage`] for invalid descriptions: too many push-constant bytes or color
    /// formats, or shader code with a bad header, a missing entry point, or a push-constant block
    /// larger than declared; or [`RhiError::Backend`] if the driver rejects the shaders.
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
    /// [`RhiError::DeviceLost`], or [`RhiError::InvalidUsage`] for an id that was never returned by
    /// [`Device::submit`].
    fn wait_for_submission(&self, submission: SubmissionId) -> Result<(), RhiError>;

    /// Copies a texture's pixels to the CPU: tightly packed rows, top row first, in the
    /// texture's format. Blocks until done. Meant for tests, tools and screenshots, not
    /// per-frame use. The texture needs [`crate::TextureUsage::COPY_SOURCE`].
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle, missing usage, or a texture an unsubmitted
    /// encoder still uses; or a device error.
    fn read_texture(&self, texture: TextureHandle) -> Result<Vec<u8>, RhiError>;

    /// Creates a buffer in GPU memory. Its contents are undefined until written.
    ///
    /// # Errors
    ///
    /// [`RhiError::OutOfMemory`], or [`RhiError::InvalidUsage`] for a zero size, a size above the
    /// device's maximum, or an empty usage.
    fn create_buffer(&self, desc: &BufferDesc<'_>) -> Result<BufferHandle, RhiError>;

    /// Destroys a buffer once the GPU has finished every submission that may use it, and every
    /// encoder that was recording when it was destroyed (ADR-0016 §1). The handle is invalid
    /// immediately. Its address stays valid for encoders created before this call (destruction
    /// waits for them), and must not be used by encoders created after it.
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle.
    fn destroy_buffer(&self, buffer: BufferHandle) -> Result<(), RhiError>;

    /// Writes `data` into `buffer` at byte `offset`. The write is copied on the GPU queue, so it
    /// **takes effect in submission order**: work submitted after this call sees the new data,
    /// and work submitted before it sees the old. Doesn't wait for the GPU. The buffer needs
    /// [`crate::BufferUsage::COPY_DESTINATION`].
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle, missing usage, empty data or a range past
    /// the end; or a device error.
    fn write_buffer(&self, buffer: BufferHandle, offset: u64, data: &[u8]) -> Result<(), RhiError>;

    /// Copies a buffer's contents to the CPU, after all work submitted before this call. Blocks
    /// until done. Meant for tests and tools. The buffer needs
    /// [`crate::BufferUsage::COPY_SOURCE`].
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle, missing usage, or a buffer too large to fit
    /// in memory on this platform (`usize`); or a device error.
    fn read_buffer(&self, buffer: BufferHandle) -> Result<Vec<u8>, RhiError>;

    /// The buffer's 64-bit GPU address, for shaders to read and write it through (pass it in
    /// push constants or another buffer). It stays the same for the buffer's lifetime, and is a
    /// multiple of [`crate::BUFFER_ADDRESS_ALIGNMENT`]. The buffer needs
    /// [`crate::BufferUsage::SHADER_ADDRESS`].
    ///
    /// Shaders that use an address are bound by
    /// [`ShaderCode::from_spirv_unchecked`](crate::ShaderCode::from_spirv_unchecked)'s contract:
    /// only addresses of live buffers, only in bounds.
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle or missing usage.
    fn buffer_address(&self, buffer: BufferHandle) -> Result<u64, RhiError>;

    /// Uploads a texture's pixels: tightly packed rows, top row first, in the texture's format
    /// (the layout [`Device::read_texture`] returns). Like [`Device::write_buffer`], the upload
    /// is copied on the queue and **takes effect in submission order**. It doesn't wait for the
    /// GPU. The texture needs [`crate::TextureUsage::COPY_DESTINATION`].
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle, missing usage, a backbuffer, data whose size
    /// isn't exactly width × height × bytes per pixel, or a texture an unsubmitted encoder still
    /// uses; or a device error.
    fn write_texture(&self, texture: TextureHandle, data: &[u8]) -> Result<(), RhiError>;

    /// The texture's index in the bindless texture array ([`crate::TEXTURES_BINDING`]), for
    /// shaders to sample it by. It stays the same for the texture's lifetime. The texture needs
    /// [`crate::TextureUsage::SAMPLED`].
    ///
    /// Shaders that use an index are bound by
    /// [`ShaderCode::from_spirv_unchecked`](crate::ShaderCode::from_spirv_unchecked)'s contract:
    /// only indices of live textures.
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle or missing usage.
    fn texture_index(&self, texture: TextureHandle) -> Result<u32, RhiError>;

    /// Creates a sampler and gives it a bindless index.
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for an anisotropy outside 1 to 16, [`RhiError::OutOfMemory`]
    /// when the device's sampler limit or the bindless sampler array is full, or another
    /// backend error.
    fn create_sampler(&self, desc: &SamplerDesc<'_>) -> Result<SamplerHandle, RhiError>;

    /// Destroys a sampler once nothing can use it any more, like
    /// [`Device::destroy_texture`]. Its index is reused only after that.
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle.
    fn destroy_sampler(&self, sampler: SamplerHandle) -> Result<(), RhiError>;

    /// The sampler's index in the bindless sampler array ([`crate::SAMPLERS_BINDING`]).
    ///
    /// # Errors
    ///
    /// [`RhiError::InvalidUsage`] for a stale handle.
    fn sampler_index(&self, sampler: SamplerHandle) -> Result<u32, RhiError>;

    /// Submits the frame's commands and presents its backbuffer.
    ///
    /// # Errors
    ///
    /// [`RhiError::DeviceLost`] / [`RhiError::OutOfMemory`] from submission or presentation, or
    /// [`RhiError::InvalidUsage`] if misuse was detected while recording (the frame is still
    /// submitted, so presentation continues) or if the frame isn't the surface's open frame. If
    /// submission fails, the swapchain is recreated at the next [`Device::begin_frame`].
    fn end_frame(&self, frame: Frame) -> Result<(), RhiError>;

    /// Whether this device created `encoder`, through [`Device::create_encoder`] or
    /// [`Device::begin_frame`].
    ///
    /// Recording looks handles up in the encoder's own device, so code that records with its
    /// own pipelines, buffer addresses and bindless indices must check this first: in another
    /// device's encoder, its handles could name unrelated objects. `oxreal-renderer` does
    /// (ADR-0019 §1).
    fn owns_encoder(&self, encoder: &dyn CommandEncoder) -> bool;
}
