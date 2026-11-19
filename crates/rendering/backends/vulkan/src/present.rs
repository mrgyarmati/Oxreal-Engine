//! Surfaces, swapchains and the frame loop (ADR-0016 §4, ADR-0017 §5–§6).
//!
//! - A surface owns its window (`Arc`), its `VkSurfaceKHR`, its swapchain and
//!   [`FRAMES_IN_FLIGHT`] frame slots. Each slot has a command pool, a command buffer and an
//!   acquire semaphore.
//! - Each swapchain image has its own render-finished semaphore. A present semaphore may only be
//!   reused once that image is acquired again, which per-image semaphores guarantee.
//! - Frame pacing uses the device's single timeline semaphore. Before a slot is reused, the CPU
//!   waits for the timeline value of that slot's last submission.
//! - Each surface has at most one open frame. `end_frame` takes everything it acts on from the
//!   frame's own encoder. A frame that is dropped, or fails after acquiring its image, has its
//!   acquire semaphore consumed by an empty submission, and the swapchain is recreated. Nothing
//!   is left signalled or acquired.
//! - Resize, out-of-date and suboptimal results only mark the swapchain for recreation. It's
//!   rebuilt at the next `begin_frame` (never while a frame is open), after the device is idle.
//! - Destroyed surfaces go through the retire queue, so a frame encoder that outlives its device
//!   never records into freed objects.

use std::sync::Arc;

use ash::vk;
use oxreal_diagnostics::{debug, info, warn};
use oxreal_rhi::{
    Frame, PresentMode, RhiError, SurfaceDesc, SurfaceHandle, SurfaceTarget, TextureFormat,
    TextureHandle,
};

use crate::device::{Shared, VulkanDevice, lock};
use crate::encoder::{FrameParts, FrameState, FrameToken, Registration, VulkanEncoder};
use crate::error::from_vk;
use crate::resources::{Retired, invalid, rhi_format};
use crate::retire::EncoderId;

/// Frames the CPU may record ahead of the GPU (ADR-0016 §4).
pub(crate) const FRAMES_IN_FLIGHT: usize = 2;

/// A texture known to the device: a swapchain backbuffer (image owned by the swapchain, no
/// allocation) or a texture from `create_texture` (owns its memory).
#[derive(Debug)]
pub(crate) struct Texture {
    pub(crate) image: vk::Image,
    pub(crate) view: vk::ImageView,
    pub(crate) extent: vk::Extent2D,
    /// The layout the image is in after every *submitted or claimed* command. Only the encoder
    /// in `user` changes it, so it follows submission order.
    pub(crate) layout: vk::ImageLayout,
    pub(crate) format: TextureFormat,
    pub(crate) usage: vk::ImageUsageFlags,
    /// `None` for swapchain backbuffers.
    pub(crate) allocation: Option<gpu_allocator::vulkan::Allocation>,
    /// The unsubmitted encoder that recorded commands using this texture, if any.
    pub(crate) user: Option<EncoderId>,
    /// For backbuffers: the frame encoder that acquired it. Only that encoder may use it.
    pub(crate) frame_owner: Option<EncoderId>,
}

/// A surface's Vulkan objects and window, destroyed together.
pub(crate) struct SurfaceObjects {
    /// Keeps the window alive while the `VkSurfaceKHR` refers to it.
    window: Arc<dyn SurfaceTarget>,
    surface: vk::SurfaceKHR,
    swapchain: Option<Swapchain>,
    frames: Vec<FrameSlot>,
}

impl std::fmt::Debug for SurfaceObjects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfaceObjects")
            .field("surface", &self.surface)
            .field("swapchain", &self.swapchain.as_ref().map(|s| s.handle))
            .finish_non_exhaustive()
    }
}

pub(crate) struct Surface {
    objects: SurfaceObjects,
    name: String,
    desired: vk::Extent2D,
    present_mode: PresentMode,
    needs_recreate: bool,
    frame_counter: u64,
    open_frame: Option<OpenFrame>,
}

/// The frame between `begin_frame` and `end_frame`.
struct OpenFrame {
    token: Arc<FrameToken>,
    slot_index: usize,
    backbuffer: u64,
}

struct Swapchain {
    handle: vk::SwapchainKHR,
    /// Raw texture handles of the backbuffers, by swapchain image index.
    images: Vec<u64>,
    /// Their views, destroyed with the swapchain.
    views: Vec<vk::ImageView>,
    /// Signalled by the frame's submission and waited on by its presentation, per image.
    render_finished: Vec<vk::Semaphore>,
}

struct FrameSlot {
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    /// Signalled when the acquired image is ready to be rendered to.
    acquire: vk::Semaphore,
    /// Timeline value of the last submission that waited on `acquire` (0: none yet).
    last_submission: u64,
    /// `acquire` still has a pending signal that `release_acquire` couldn't consume (its
    /// submission failed). Such a semaphore is never used or destroyed again: the next frame on
    /// the slot replaces it.
    acquire_pending: bool,
}

fn stale(kind: &str, handle: impl std::fmt::Debug) -> RhiError {
    RhiError::InvalidUsage {
        details: format!("{kind} {handle:?} does not exist (destroyed or never created)"),
    }
}

impl VulkanDevice {
    pub(crate) fn create_surface_impl(
        &self,
        window: Arc<dyn SurfaceTarget>,
        desc: &SurfaceDesc,
    ) -> Result<SurfaceHandle, RhiError> {
        let unsupported = |details: &str| RhiError::PresentationUnsupported {
            adapter: oxreal_rhi::Device::adapter_info(self).name.clone(),
            details: details.to_owned(),
        };
        let shared = &self.shared;
        let (Some(surface_loader), Some(_)) = (&shared.surface_loader, &shared.swapchain_loader)
        else {
            return Err(unsupported(
                "the VK_KHR_surface / VK_KHR_swapchain extensions are not available",
            ));
        };
        let handle_error = |e: raw_window_handle::HandleError| RhiError::Backend {
            api: "Vulkan",
            operation: "reading the window's native handles",
            details: e.to_string(),
        };
        let display = window.display_handle().map_err(handle_error)?.as_raw();
        let native = window.window_handle().map_err(handle_error)?.as_raw();

        // SAFETY: `display` and `native` come from `window`, which the surface keeps alive in an
        // `Arc` until the `VkSurfaceKHR` is destroyed. The instance enabled every platform
        // surface extension the loader offers. If the needed one is missing, the call fails with
        // ERROR_EXTENSION_NOT_PRESENT instead of misbehaving.
        let surface = unsafe {
            ash_window::create_surface(
                &shared.instance.entry,
                &shared.instance.instance,
                display,
                native,
                None,
            )
        }
        .map_err(|e| match e {
            vk::Result::ERROR_EXTENSION_NOT_PRESENT => {
                unsupported("Vulkan has no surface extension for this window system")
            }
            other => from_vk("vkCreate*SurfaceKHR", other),
        })?;

        // SAFETY: the surface was just created from this instance. The physical device and
        // queue family are this device's.
        let supported = unsafe {
            surface_loader.get_physical_device_surface_support(
                shared.physical_device,
                shared.queue_family,
                surface,
            )
        };
        let failure = match supported {
            Ok(true) => None,
            Ok(false) => Some(unsupported(
                "the graphics queue cannot present to this window",
            )),
            Err(e) => Some(from_vk("vkGetPhysicalDeviceSurfaceSupportKHR", e)),
        };
        if let Some(error) = failure {
            // SAFETY: the surface is unused (no swapchain) and destroyed exactly once.
            unsafe { surface_loader.destroy_surface(surface, None) };
            return Err(error);
        }

        let mut frames = Vec::with_capacity(FRAMES_IN_FLIGHT);
        for _ in 0..FRAMES_IN_FLIGHT {
            match self.create_frame_slot() {
                Ok(slot) => frames.push(slot),
                Err(error) => {
                    for slot in frames {
                        shared.destroy_frame_slot(slot);
                    }
                    // SAFETY: as above, unused and destroyed exactly once.
                    unsafe { surface_loader.destroy_surface(surface, None) };
                    return Err(error);
                }
            }
        }

        let mut surfaces = lock(&self.surfaces);
        let raw = surfaces.insert(Surface {
            objects: SurfaceObjects {
                window,
                surface,
                swapchain: None,
                frames,
            },
            name: String::new(),
            desired: vk::Extent2D {
                width: desc.width,
                height: desc.height,
            },
            present_mode: desc.present_mode,
            needs_recreate: true,
            frame_counter: 0,
            open_frame: None,
        });
        let handle = SurfaceHandle::from_raw(raw);
        if let Some(entry) = surfaces.get_mut(raw) {
            entry.name = format!("{handle:?}");
            shared.name_object(entry.objects.surface, &entry.name);
            for (i, slot) in entry.objects.frames.iter().enumerate() {
                let name = &entry.name;
                shared.name_object(slot.pool, &format!("{name} frame {i} command pool"));
                shared.name_object(slot.cmd, &format!("{name} frame {i} commands"));
                shared.name_object(slot.acquire, &format!("{name} frame {i} acquire"));
            }
        }
        info!(surface = ?handle, width = desc.width, height = desc.height, present_mode = ?desc.present_mode, "surface created");
        Ok(handle)
    }

    pub(crate) fn resize_surface_impl(
        &self,
        surface: SurfaceHandle,
        width: u32,
        height: u32,
    ) -> Result<(), RhiError> {
        let mut surfaces = lock(&self.surfaces);
        let entry = surfaces
            .get_mut(surface.to_raw())
            .ok_or_else(|| stale("surface", surface))?;
        entry.desired = vk::Extent2D { width, height };
        entry.needs_recreate = true;
        debug!(surface = ?surface, width, height, "surface resize requested");
        Ok(())
    }

    pub(crate) fn destroy_surface_impl(&self, surface: SurfaceHandle) -> Result<(), RhiError> {
        let entry = {
            let mut surfaces = lock(&self.surfaces);
            let entry = surfaces
                .get(surface.to_raw())
                .ok_or_else(|| stale("surface", surface))?;
            if entry
                .open_frame
                .as_ref()
                .is_some_and(|open| open.token.state() == FrameState::Open)
            {
                return Err(invalid(format!(
                    "{surface:?} has an open frame; pass it to end_frame before destroying the surface"
                )));
            }
            surfaces
                .remove(surface.to_raw())
                .ok_or_else(|| stale("surface", surface))?
        };
        self.retire_surface(entry);
        info!(surface = ?surface, "surface destroyed");
        Ok(())
    }

    /// Takes a surface out of service and queues its objects for destruction.
    pub(crate) fn retire_surface(&self, mut surface: Surface) {
        if let Some(open) = surface.open_frame.take()
            && let Some(slot) = surface.objects.frames.get_mut(open.slot_index)
        {
            // An abandoned frame, or (at device teardown) one still open: its acquire semaphore
            // has a pending signal that must be consumed before the semaphore is destroyed.
            self.release_acquire(slot);
        }
        // Backbuffers leave the texture table now, so no encoder can start using them. An
        // encoder that already did blocks the retirement below until it's done.
        if let Some(swapchain) = &surface.objects.swapchain {
            let mut textures = lock(&self.shared.textures);
            for &raw in &swapchain.images {
                textures.remove(raw);
            }
        }
        // Presentation may still use the swapchain and its semaphores, and no fence tracks
        // that. An idle device has finished presenting.
        if let Err(e) = self.shared.wait_idle() {
            warn!(error = %e, "waiting before destroying a surface failed");
        }
        if let Err(e) = self
            .shared
            .retire(Retired::Surface(Box::new(surface.objects)))
        {
            warn!(error = %e, "collecting destroyed objects failed");
        }
    }

    pub(crate) fn begin_frame_impl(
        &self,
        surface: SurfaceHandle,
    ) -> Result<Option<Frame>, RhiError> {
        // A frame loop that never destroys anything still frees what earlier destroys left
        // waiting for the GPU.
        self.shared.collect_garbage()?;
        let mut surfaces = lock(&self.surfaces);
        let entry = surfaces
            .get_mut(surface.to_raw())
            .ok_or_else(|| stale("surface", surface))?;

        if let Some(open) = &entry.open_frame {
            if open.token.state() == FrameState::Open {
                return Err(invalid(format!(
                    "{surface:?} already has an open frame; pass it to end_frame before beginning another"
                )));
            }
            self.recover_dropped_frame(entry);
        }
        if entry.desired.width == 0 || entry.desired.height == 0 {
            return Ok(None); // Minimized: nothing to present.
        }
        if entry.objects.swapchain.is_none() || entry.needs_recreate {
            self.recreate_swapchain(entry)?;
        }
        if entry.needs_recreate {
            return Ok(None);
        }
        let (Some(swapchain), Some(swapchain_loader)) =
            (&entry.objects.swapchain, &self.shared.swapchain_loader)
        else {
            return Ok(None); // The window currently has no drawable area.
        };
        let swapchain_handle = swapchain.handle;
        let images = swapchain.images.clone();

        let slot_index =
            usize::try_from(entry.frame_counter % FRAMES_IN_FLIGHT as u64).unwrap_or(0);
        let Some(slot) = entry.objects.frames.get_mut(slot_index) else {
            return Err(invalid(format!(
                "{surface:?} has no frame slot {slot_index}"
            )));
        };
        // The slot's command buffer and acquire semaphore are reusable once the GPU has finished
        // the slot's previous submission.
        self.shared.wait_timeline(slot.last_submission)?;
        if slot.acquire_pending {
            self.replace_acquire_semaphore(slot)?;
        }

        // SAFETY: the swapchain and semaphore are alive. The semaphore is unsignalled with no
        // pending operation: every signal from an earlier acquire was consumed by a later
        // submission (`end_frame`, or `release_acquire` for frames that failed or were dropped)
        // that completed (waited above). A signal that couldn't be consumed marked the slot
        // `acquire_pending`, and the semaphore was replaced just above.
        let acquired = unsafe {
            swapchain_loader.acquire_next_image(
                swapchain_handle,
                u64::MAX,
                slot.acquire,
                vk::Fence::null(),
            )
        };
        let image_index = match acquired {
            Ok((index, suboptimal)) => {
                if suboptimal {
                    entry.needs_recreate = true; // Still usable this frame.
                }
                index
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                entry.needs_recreate = true;
                return Ok(None);
            }
            Err(e) => return Err(from_vk("vkAcquireNextImageKHR", e)),
        };

        // From here on, an image is acquired and `slot.acquire` will be signalled. Any failure
        // must consume that signal and recreate the swapchain, which releases the image.
        let prepared = self.prepare_frame(slot, &images, image_index);
        let (registration, backbuffer, format) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.release_acquire(slot);
                entry.needs_recreate = true;
                return Err(error);
            }
        };
        let token = Arc::new(FrameToken::new());
        entry.open_frame = Some(OpenFrame {
            token: Arc::clone(&token),
            slot_index,
            backbuffer,
        });
        let encoder = VulkanEncoder::frame(
            Arc::clone(&self.shared),
            registration,
            slot.cmd,
            FrameParts {
                surface,
                image_index,
                slot_index,
                token,
            },
        );
        Ok(Some(Frame::new(
            surface,
            TextureHandle::from_raw(backbuffer),
            format,
            Box::new(encoder),
        )))
    }

    /// Resets the slot's command buffer for recording and makes the frame's encoder the owner of
    /// the acquired backbuffer.
    fn prepare_frame(
        &self,
        slot: &FrameSlot,
        images: &[u64],
        image_index: u32,
    ) -> Result<(Registration, u64, TextureFormat), RhiError> {
        let backbuffer =
            images
                .get(image_index as usize)
                .copied()
                .ok_or_else(|| RhiError::Backend {
                    api: "Vulkan",
                    operation: "vkAcquireNextImageKHR",
                    details: format!("returned image index {image_index} beyond the swapchain"),
                })?;
        // SAFETY: the pool's command buffer is not in use. The GPU finished this slot's previous
        // submission (waited in `begin_frame`). An encoder that recorded into it was ended or
        // dropped: a slot only gets a new frame once its previous one is no longer open.
        unsafe {
            self.raw()
                .reset_command_pool(slot.pool, vk::CommandPoolResetFlags::empty())
        }
        .map_err(|e| from_vk("vkResetCommandPool", e))?;
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: the command buffer was just reset and is not in use.
        unsafe { self.raw().begin_command_buffer(slot.cmd, &begin) }
            .map_err(|e| from_vk("vkBeginCommandBuffer", e))?;

        let registration = Registration::new(&self.shared);
        let mut textures = lock(&self.shared.textures);
        let texture = textures
            .get_mut(backbuffer)
            .ok_or_else(|| RhiError::Backend {
                api: "Vulkan",
                operation: "begin_frame",
                details: "the acquired backbuffer is missing from the texture table".to_owned(),
            })?;
        texture.frame_owner = Some(registration.id());
        Ok((registration, backbuffer, texture.format))
    }

    /// Cleans up after a frame whose encoder was dropped without `end_frame`.
    fn recover_dropped_frame(&self, entry: &mut Surface) {
        let Some(open) = entry.open_frame.take() else {
            return;
        };
        // The encoder is gone (its `Release` store is visible through `state()`), so nothing
        // records into the slot's command buffer any more.
        if let Some(slot) = entry.objects.frames.get_mut(open.slot_index) {
            self.release_acquire(slot);
        }
        if let Some(texture) = lock(&self.shared.textures).get_mut(open.backbuffer) {
            texture.frame_owner = None;
        }
        // The image stays acquired until the swapchain is recreated, which releases it.
        entry.needs_recreate = true;
        warn!(surface = %entry.name, "recovered a dropped frame; recreating the swapchain");
    }

    /// Consumes the pending signal of `slot.acquire` with an empty submission that waits on it,
    /// so the semaphore can be reused or destroyed.
    fn release_acquire(&self, slot: &mut FrameSlot) {
        let mut queue = lock(&self.shared.queue);
        let waits = [vk::SemaphoreSubmitInfo::default()
            .semaphore(slot.acquire)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        match self.shared.submit_locked(&mut queue, None, &waits, &[]) {
            Ok(value) => slot.last_submission = value,
            Err(e) => {
                // The signal stays pending: the semaphore can be neither reused nor destroyed.
                slot.acquire_pending = true;
                drop(queue);
                warn!(error = %e, "releasing a frame's acquire semaphore failed; it will be replaced");
            }
        }
    }

    /// Gives `slot` a new acquire semaphore in place of one whose signal is still pending. The
    /// old one is deliberately leaked: destroying a semaphore with a pending signal isn't
    /// allowed, and this only happens after a failed submission (out of memory or a lost
    /// device).
    fn replace_acquire_semaphore(&self, slot: &mut FrameSlot) -> Result<(), RhiError> {
        // SAFETY: the device is alive.
        let semaphore = unsafe {
            self.raw()
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
        }
        .map_err(|e| from_vk("vkCreateSemaphore", e))?;
        warn!("leaking an acquire semaphore whose signal could not be consumed");
        slot.acquire = semaphore;
        slot.acquire_pending = false;
        Ok(())
    }

    pub(crate) fn end_frame_impl(&self, frame: Frame) -> Result<(), RhiError> {
        // The surface and backbuffer come from the encoder, never from the `Frame`'s public
        // parts, so a mismatched frame can't make us present the wrong image.
        let (_, _, encoder) = frame.into_parts();
        let mut encoder = *encoder
            .into_any()
            .downcast::<VulkanEncoder>()
            .map_err(|_| invalid("frame was created by a different RHI backend"))?;
        if !encoder.belongs_to(&self.shared) {
            return Err(invalid("end_frame was given a frame of a different device"));
        }
        let Some(parts) = encoder.frame_parts() else {
            return Err(invalid(
                "end_frame was given an encoder from create_encoder; use submit",
            ));
        };
        let surface = parts.surface;
        let mut surfaces = lock(&self.surfaces);
        let entry = surfaces
            .get_mut(surface.to_raw())
            .ok_or_else(|| stale("surface", surface))?;
        // Checked before anything changes: a frame that isn't this surface's open frame must
        // leave the real open frame untouched.
        let is_open_frame = entry
            .open_frame
            .as_ref()
            .is_some_and(|open| Arc::ptr_eq(&open.token, &parts.token));
        if !is_open_frame {
            return Err(invalid(format!(
                "this frame is not {surface:?}'s open frame"
            )));
        }
        let Some(open) = entry.open_frame.take() else {
            return Err(invalid(format!("{surface:?} has no open frame")));
        };
        parts.token.end();

        encoder.finish_recording();
        encoder.transition_for_present(open.backbuffer);
        let result = self.submit_and_present(entry, &mut encoder, &parts, open.backbuffer);
        if let Some(texture) = lock(&self.shared.textures).get_mut(open.backbuffer) {
            texture.frame_owner = None;
        }
        let misuse = encoder.take_misuse();
        if let Err(error) = result {
            if !misuse.is_empty() {
                warn!(misuse = %misuse.join("; "), "misuse recorded in a frame that failed to submit");
            }
            return Err(error);
        }
        entry.frame_counter += 1;
        if misuse.is_empty() {
            Ok(())
        } else {
            Err(invalid(misuse.join("; ")))
        }
    }

    fn submit_and_present(
        &self,
        entry: &mut Surface,
        encoder: &mut VulkanEncoder,
        parts: &FrameParts,
        backbuffer: u64,
    ) -> Result<(), RhiError> {
        let surface = parts.surface;
        let objects = &mut entry.objects;
        // The slot index came from `begin_frame` on this surface, whose slots never change.
        let Some(slot) = objects.frames.get_mut(parts.slot_index) else {
            return Err(invalid(format!(
                "{surface:?} has no frame slot {}",
                parts.slot_index
            )));
        };
        let (Some(swapchain), Some(swapchain_loader)) =
            (&objects.swapchain, &self.shared.swapchain_loader)
        else {
            // Unreachable while the frame is open, but the acquire signal must still be consumed.
            self.release_acquire(slot);
            entry.needs_recreate = true;
            return Err(invalid(format!(
                "{surface:?} has no swapchain to present to"
            )));
        };
        let render_finished = match (
            swapchain.render_finished.get(parts.image_index as usize),
            swapchain.images.get(parts.image_index as usize),
        ) {
            (Some(&semaphore), Some(&image)) if image == backbuffer => semaphore,
            _ => {
                self.release_acquire(slot);
                entry.needs_recreate = true;
                return Err(invalid(format!(
                    "{surface:?}: the frame's image index {} doesn't match its swapchain",
                    parts.image_index
                )));
            }
        };

        let cmd = encoder.command_buffer();
        // SAFETY: recording into `cmd` began in `begin_frame`, and `finish_recording` closed any
        // open render pass.
        if let Err(e) = unsafe { self.raw().end_command_buffer(cmd) } {
            self.release_acquire(slot);
            entry.needs_recreate = true;
            return Err(from_vk("vkEndCommandBuffer", e));
        }

        let mut queue = lock(&self.shared.queue);
        let waits = [vk::SemaphoreSubmitInfo::default()
            .semaphore(slot.acquire)
            .stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)];
        let signals = [vk::SemaphoreSubmitInfo::default()
            .semaphore(render_finished)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        // The acquire semaphore has a pending signal from `begin_frame`'s acquire. This image's
        // render-finished semaphore is unsignalled: its previous present completed before the
        // image could be acquired again.
        let value = match self
            .shared
            .submit_locked(&mut queue, Some(cmd), &waits, &signals)
        {
            Ok(value) => value,
            Err(error) => {
                drop(queue);
                // Out of memory or a lost device. Try to consume the acquire signal, and rebuild
                // the swapchain either way.
                self.release_acquire(slot);
                entry.needs_recreate = true;
                return Err(error);
            }
        };
        slot.last_submission = value;
        // Under the queue lock (lock order queue → retired → textures).
        encoder.registration().submitted(value);

        let wait_present = [render_finished];
        let swapchains = [swapchain.handle];
        let indices = [parts.image_index];
        let present = vk::PresentInfoKHR::default()
            .wait_semaphores(&wait_present)
            .swapchains(&swapchains)
            .image_indices(&indices);
        // SAFETY: externally synchronized by the queue lock. The image was acquired in
        // `begin_frame` and is in PRESENT_SRC (`transition_for_present`). The wait semaphore is
        // signalled by the submission above.
        let presented = unsafe { swapchain_loader.queue_present(queue.queue, &present) };
        drop(queue);
        match presented {
            Ok(false) => Ok(()),
            Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                entry.needs_recreate = true;
                Ok(())
            }
            Err(e) => {
                // Surface lost, out of memory or device lost. Whether the present consumed its
                // semaphore wait is unspecified, so rebuild the swapchain and its semaphores.
                entry.needs_recreate = true;
                Err(from_vk("vkQueuePresentKHR", e))
            }
        }
    }

    fn create_frame_slot(&self) -> Result<FrameSlot, RhiError> {
        let pool_info =
            vk::CommandPoolCreateInfo::default().queue_family_index(self.shared.queue_family);
        // SAFETY: the device is alive, and the queue family is the device's.
        let pool = unsafe { self.raw().create_command_pool(&pool_info, None) }
            .map_err(|e| from_vk("vkCreateCommandPool", e))?;
        let alloc = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: `pool` was just created.
        let cmd = match unsafe { self.raw().allocate_command_buffers(&alloc) } {
            Ok(buffers) => buffers[0],
            Err(e) => {
                // SAFETY: the pool is unused and destroyed exactly once.
                unsafe { self.raw().destroy_command_pool(pool, None) };
                return Err(from_vk("vkAllocateCommandBuffers", e));
            }
        };
        // SAFETY: the device is alive.
        let acquire = match unsafe {
            self.raw()
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
        } {
            Ok(semaphore) => semaphore,
            Err(e) => {
                // SAFETY: as above. Destroying the pool frees its command buffer.
                unsafe { self.raw().destroy_command_pool(pool, None) };
                return Err(from_vk("vkCreateSemaphore", e));
            }
        };
        Ok(FrameSlot {
            pool,
            cmd,
            acquire,
            last_submission: 0,
            acquire_pending: false,
        })
    }

    /// Creates or replaces the surface's swapchain for its current size. Only called with no
    /// frame open. The device must be idle, which this function ensures.
    fn recreate_swapchain(&self, entry: &mut Surface) -> Result<(), RhiError> {
        let shared = &self.shared;
        let (Some(surface_loader), Some(swapchain_loader)) =
            (&shared.surface_loader, &shared.swapchain_loader)
        else {
            return Ok(());
        };
        // The old swapchain's images, views and semaphores may still be in use.
        shared.wait_idle()?;
        let surface = entry.objects.surface;

        // SAFETY: the surface belongs to this instance and the physical device is this device's.
        let caps = unsafe {
            surface_loader.get_physical_device_surface_capabilities(shared.physical_device, surface)
        }
        .map_err(|e| from_vk("vkGetPhysicalDeviceSurfaceCapabilitiesKHR", e))?;
        let extent = choose_extent(&caps, entry.desired);
        if extent.width == 0 || extent.height == 0 {
            entry.needs_recreate = true; // Minimized. Try again when the window has a size.
            return Ok(());
        }
        // SAFETY: as above.
        let formats = unsafe {
            surface_loader.get_physical_device_surface_formats(shared.physical_device, surface)
        }
        .map_err(|e| from_vk("vkGetPhysicalDeviceSurfaceFormatsKHR", e))?;
        // SAFETY: as above.
        let modes = unsafe {
            surface_loader
                .get_physical_device_surface_present_modes(shared.physical_device, surface)
        }
        .map_err(|e| from_vk("vkGetPhysicalDeviceSurfacePresentModesKHR", e))?;

        let format = choose_format(&formats).ok_or_else(|| RhiError::Backend {
            api: "Vulkan",
            operation: "choosing a swapchain format",
            details: "the surface offers no 8-bit BGRA or RGBA format in the sRGB color space"
                .to_owned(),
        })?;
        if !is_srgb(format.format) {
            warn!(format = ?format.format, "no sRGB swapchain format available; colors will be too dark");
        }
        let mode = choose_present_mode(entry.present_mode, &modes);
        if vk_present_mode(entry.present_mode) != mode {
            info!(requested = ?entry.present_mode, using = ?mode, "present mode unavailable; falling back");
        }
        let old = entry
            .objects
            .swapchain
            .as_ref()
            .map_or(vk::SwapchainKHR::null(), |s| s.handle);
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(choose_image_count(&caps))
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(choose_composite_alpha(caps.supported_composite_alpha))
            .present_mode(mode)
            .clipped(true)
            .old_swapchain(old);
        // SAFETY: every value comes from the surface's reported capabilities, formats and modes.
        // `old` is the surface's current swapchain (or null). It has not been retired yet: a
        // retired one is destroyed right below, whether creation succeeds or not.
        let created = unsafe { swapchain_loader.create_swapchain(&info, None) };
        // Passing `old` retires it even if creation fails, so it may never be passed again.
        // The device is idle (waited above) and no frame is open, so nothing uses it.
        if let Some(previous) = entry.objects.swapchain.take() {
            shared.forget_backbuffers(&previous);
            shared.destroy_swapchain(previous);
        }
        let handle = created.map_err(|e| from_vk("vkCreateSwapchainKHR", e))?;

        // Stored before anything else can fail or log, so the surface never has a live
        // swapchain it doesn't know about, even if a log subscriber panics.
        let swapchain = entry.objects.swapchain.insert(Swapchain {
            handle,
            images: Vec::new(),
            views: Vec::new(),
            render_finished: Vec::new(),
        });
        if let Err(error) = self.populate_swapchain(swapchain, &entry.name, format.format, extent) {
            if let Some(partial) = entry.objects.swapchain.take() {
                shared.forget_backbuffers(&partial);
                shared.destroy_swapchain(partial);
            }
            return Err(error);
        }
        let images = swapchain.images.len();
        entry.needs_recreate = false;
        info!(
            surface = %entry.name,
            width = extent.width,
            height = extent.height,
            format = ?format.format,
            present_mode = ?mode,
            images,
            "swapchain created"
        );
        Ok(())
    }

    /// Creates the views, texture entries and present semaphores of a new swapchain. On error the
    /// partially filled swapchain is still valid to destroy.
    fn populate_swapchain(
        &self,
        swapchain: &mut Swapchain,
        name: &str,
        format: vk::Format,
        extent: vk::Extent2D,
    ) -> Result<(), RhiError> {
        let shared = &self.shared;
        let Some(swapchain_loader) = &shared.swapchain_loader else {
            return Ok(());
        };
        // `choose_format` only picks formats the RHI can name.
        let texture_format = rhi_format(format).ok_or_else(|| RhiError::Backend {
            api: "Vulkan",
            operation: "creating a swapchain",
            details: format!("swapchain format {format:?} has no RHI equivalent"),
        })?;
        shared.name_object(swapchain.handle, &format!("{name} swapchain"));
        // SAFETY: the swapchain was just created.
        let images = unsafe { swapchain_loader.get_swapchain_images(swapchain.handle) }
            .map_err(|e| from_vk("vkGetSwapchainImagesKHR", e))?;
        for (i, image) in images.into_iter().enumerate() {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(color_subresource());
            // SAFETY: `image` belongs to the live swapchain, and the view matches its format.
            let view = unsafe { self.raw().create_image_view(&view_info, None) }
                .map_err(|e| from_vk("vkCreateImageView", e))?;
            swapchain.views.push(view);
            let raw = lock(&shared.textures).insert(Texture {
                image,
                view,
                extent,
                layout: vk::ImageLayout::UNDEFINED,
                format: texture_format,
                usage: vk::ImageUsageFlags::COLOR_ATTACHMENT,
                allocation: None,
                user: None,
                frame_owner: None,
            });
            swapchain.images.push(raw);
            shared.name_object(image, &format!("{name} backbuffer {i}"));
            shared.name_object(view, &format!("{name} backbuffer {i} view"));

            // SAFETY: the device is alive.
            let semaphore = unsafe {
                self.raw()
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }
            .map_err(|e| from_vk("vkCreateSemaphore", e))?;
            swapchain.render_finished.push(semaphore);
            shared.name_object(semaphore, &format!("{name} backbuffer {i} render finished"));
        }
        Ok(())
    }
}

impl Shared {
    /// Removes a swapchain's backbuffers from the texture table.
    fn forget_backbuffers(&self, swapchain: &Swapchain) {
        let mut textures = lock(&self.textures);
        for &raw in &swapchain.images {
            textures.remove(raw);
        }
    }

    /// Destroys a swapchain's views, semaphores and the swapchain. Nothing may use them: the
    /// device is idle and no encoder can reach them.
    fn destroy_swapchain(&self, swapchain: Swapchain) {
        for view in swapchain.views {
            // SAFETY: unused, per this function's contract. Destroyed exactly once.
            unsafe { self.raw().destroy_image_view(view, None) };
        }
        for semaphore in swapchain.render_finished {
            // SAFETY: as above. No submission or present still waits on it.
            unsafe { self.raw().destroy_semaphore(semaphore, None) };
        }
        if let Some(loader) = &self.swapchain_loader {
            // SAFETY: the views are destroyed and nothing uses the images (owned by the
            // swapchain and released with it).
            unsafe { loader.destroy_swapchain(swapchain.handle, None) };
        }
    }

    fn destroy_frame_slot(&self, slot: FrameSlot) {
        if !slot.acquire_pending {
            // SAFETY: nothing uses the semaphore (the device is idle or the slot's submissions
            // completed), and its last signal was consumed by `release_acquire` or an
            // `end_frame` submission. Destroyed exactly once.
            unsafe { self.raw().destroy_semaphore(slot.acquire, None) };
        }
        // SAFETY: no submission uses the pool's command buffer any more (as above), and no
        // encoder records into it. Destroyed exactly once, which frees its command buffer.
        unsafe { self.raw().destroy_command_pool(slot.pool, None) };
    }

    /// Destroys everything a retired surface owns, then releases its window.
    pub(crate) fn destroy_surface_objects(&self, objects: SurfaceObjects) {
        let SurfaceObjects {
            window,
            surface,
            swapchain,
            frames,
        } = objects;
        if let Some(swapchain) = swapchain {
            self.destroy_swapchain(swapchain);
        }
        for slot in frames {
            self.destroy_frame_slot(slot);
        }
        if let Some(loader) = &self.surface_loader {
            // SAFETY: the surface's swapchain was destroyed above, and the surface is destroyed
            // exactly once, while its window is still alive.
            unsafe { loader.destroy_surface(surface, None) };
        }
        drop(window);
    }
}

/// The subresource range of a single-mip, single-layer color image.
pub(crate) fn color_subresource() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

/// The swapchain size: the surface's current extent when the platform fixes it, otherwise the
/// requested size clamped to what the surface allows.
fn choose_extent(caps: &vk::SurfaceCapabilitiesKHR, desired: vk::Extent2D) -> vk::Extent2D {
    if caps.current_extent.width != u32::MAX {
        return caps.current_extent;
    }
    vk::Extent2D {
        width: desired
            .width
            .clamp(caps.min_image_extent.width, caps.max_image_extent.width),
        height: desired
            .height
            .clamp(caps.min_image_extent.height, caps.max_image_extent.height),
    }
}

/// Prefers 8-bit BGRA, then RGBA, in sRGB with the standard color space (ADR-0017 §6), then
/// their linear variants. Formats the RHI can't name are never chosen, so pipelines can always
/// be created for the backbuffer's format.
fn choose_format(formats: &[vk::SurfaceFormatKHR]) -> Option<vk::SurfaceFormatKHR> {
    let preferred = [
        vk::Format::B8G8R8A8_SRGB,
        vk::Format::R8G8B8A8_SRGB,
        vk::Format::B8G8R8A8_UNORM,
        vk::Format::R8G8B8A8_UNORM,
    ];
    preferred.iter().find_map(|&wanted| {
        formats
            .iter()
            .copied()
            .find(|f| f.format == wanted && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
    })
}

fn is_srgb(format: vk::Format) -> bool {
    matches!(
        format,
        vk::Format::B8G8R8A8_SRGB | vk::Format::R8G8B8A8_SRGB | vk::Format::A8B8G8R8_SRGB_PACK32
    )
}

fn vk_present_mode(mode: PresentMode) -> vk::PresentModeKHR {
    match mode {
        PresentMode::Fifo => vk::PresentModeKHR::FIFO,
        PresentMode::Mailbox => vk::PresentModeKHR::MAILBOX,
        PresentMode::Immediate => vk::PresentModeKHR::IMMEDIATE,
    }
}

/// The requested mode if supported, otherwise FIFO, which every Vulkan implementation supports.
fn choose_present_mode(
    requested: PresentMode,
    available: &[vk::PresentModeKHR],
) -> vk::PresentModeKHR {
    let wanted = vk_present_mode(requested);
    if available.contains(&wanted) {
        wanted
    } else {
        vk::PresentModeKHR::FIFO
    }
}

/// One more image than the minimum, so the CPU rarely waits for the presentation engine,
/// within the surface's maximum (0 means no maximum).
fn choose_image_count(caps: &vk::SurfaceCapabilitiesKHR) -> u32 {
    let wanted = caps.min_image_count + 1;
    if caps.max_image_count == 0 {
        wanted
    } else {
        wanted.min(caps.max_image_count)
    }
}

/// Opaque if possible. Otherwise the first supported mode, since some platforms (Wayland
/// compositors) only offer others.
fn choose_composite_alpha(supported: vk::CompositeAlphaFlagsKHR) -> vk::CompositeAlphaFlagsKHR {
    [
        vk::CompositeAlphaFlagsKHR::OPAQUE,
        vk::CompositeAlphaFlagsKHR::INHERIT,
        vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
    ]
    .into_iter()
    .find(|&mode| supported.contains(mode))
    .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(current: (u32, u32), min: (u32, u32), max: (u32, u32)) -> vk::SurfaceCapabilitiesKHR {
        vk::SurfaceCapabilitiesKHR {
            current_extent: vk::Extent2D {
                width: current.0,
                height: current.1,
            },
            min_image_extent: vk::Extent2D {
                width: min.0,
                height: min.1,
            },
            max_image_extent: vk::Extent2D {
                width: max.0,
                height: max.1,
            },
            min_image_count: 2,
            max_image_count: 3,
            ..Default::default()
        }
    }

    #[test]
    fn extent_follows_the_platform_or_clamps_the_request() {
        let desired = vk::Extent2D {
            width: 5000,
            height: 10,
        };
        let fixed = caps((1280, 720), (1, 1), (4096, 4096));
        assert_eq!(
            choose_extent(&fixed, desired),
            vk::Extent2D {
                width: 1280,
                height: 720
            }
        );
        let free = caps((u32::MAX, u32::MAX), (16, 16), (4096, 4096));
        assert_eq!(
            choose_extent(&free, desired),
            vk::Extent2D {
                width: 4096,
                height: 16
            }
        );
    }

    #[test]
    fn srgb_formats_are_preferred() {
        let f = |format, color_space| vk::SurfaceFormatKHR {
            format,
            color_space,
        };
        let nl = vk::ColorSpaceKHR::SRGB_NONLINEAR;
        let available = [
            f(vk::Format::B8G8R8A8_UNORM, nl),
            f(vk::Format::R8G8B8A8_SRGB, nl),
        ];
        assert_eq!(
            choose_format(&available).map(|x| x.format),
            Some(vk::Format::R8G8B8A8_SRGB)
        );
        let both = [
            f(vk::Format::R8G8B8A8_SRGB, nl),
            f(vk::Format::B8G8R8A8_SRGB, nl),
        ];
        assert_eq!(
            choose_format(&both).map(|x| x.format),
            Some(vk::Format::B8G8R8A8_SRGB)
        );
        let unorm_only = [f(vk::Format::B8G8R8A8_UNORM, nl)];
        assert_eq!(
            choose_format(&unorm_only).map(|x| x.format),
            Some(vk::Format::B8G8R8A8_UNORM)
        );
        assert!(!is_srgb(vk::Format::B8G8R8A8_UNORM));
        assert_eq!(choose_format(&[]), None);
        let exotic = [f(vk::Format::A2B10G10R10_UNORM_PACK32, nl)];
        assert_eq!(
            choose_format(&exotic),
            None,
            "formats the RHI cannot name are never chosen"
        );
    }

    #[test]
    fn unsupported_present_modes_fall_back_to_fifo() {
        let modes = [vk::PresentModeKHR::FIFO, vk::PresentModeKHR::IMMEDIATE];
        assert_eq!(
            choose_present_mode(PresentMode::Immediate, &modes),
            vk::PresentModeKHR::IMMEDIATE
        );
        assert_eq!(
            choose_present_mode(PresentMode::Mailbox, &modes),
            vk::PresentModeKHR::FIFO
        );
        assert_eq!(
            choose_present_mode(PresentMode::Fifo, &modes),
            vk::PresentModeKHR::FIFO
        );
    }

    #[test]
    fn image_count_is_one_above_minimum_within_maximum() {
        let mut c = caps((1, 1), (1, 1), (1, 1));
        assert_eq!(choose_image_count(&c), 3);
        c.max_image_count = 2;
        assert_eq!(choose_image_count(&c), 2);
        c.max_image_count = 0;
        assert_eq!(choose_image_count(&c), 3);
    }

    #[test]
    fn composite_alpha_prefers_opaque() {
        use vk::CompositeAlphaFlagsKHR as A;
        assert_eq!(
            choose_composite_alpha(A::OPAQUE | A::PRE_MULTIPLIED),
            A::OPAQUE
        );
        assert_eq!(choose_composite_alpha(A::PRE_MULTIPLIED), A::PRE_MULTIPLIED);
        assert_eq!(
            choose_composite_alpha(A::INHERIT | A::POST_MULTIPLIED),
            A::INHERIT
        );
    }
}
