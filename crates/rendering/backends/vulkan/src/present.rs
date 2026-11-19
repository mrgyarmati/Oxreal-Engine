//! Surfaces, swapchains and the frame loop (ADR-0016 §4, ADR-0017 §5–§6).
//!
//! - A surface owns its window (`Arc`), its `VkSurfaceKHR`, its swapchain and
//!   [`FRAMES_IN_FLIGHT`] frame slots. Each slot has a command pool, a command buffer and an
//!   acquire semaphore.
//! - Each swapchain image has its own render-finished semaphore. A present semaphore may only be
//!   reused once that image is acquired again, which per-image semaphores guarantee.
//! - Frame pacing uses the device's single timeline semaphore. Before a slot is reused, the CPU
//!   waits for the timeline value of that slot's last submission.
//! - Resize, out-of-date and suboptimal results only mark the swapchain for recreation. It's
//!   rebuilt at the next `begin_frame`, after the device is idle.

use std::sync::{Arc, Mutex, MutexGuard};

use ash::vk;
use oxreal_diagnostics::{debug, info, warn};
use oxreal_rhi::{
    Frame, PresentMode, RhiError, SurfaceDesc, SurfaceHandle, SurfaceTarget, TextureFormat,
    TextureHandle,
};

use crate::device::VulkanDevice;
use crate::encoder::VulkanEncoder;
use crate::error::from_vk;
use crate::resources::rhi_format;

/// Frames the CPU may record ahead of the GPU (ADR-0016 §4).
pub(crate) const FRAMES_IN_FLIGHT: usize = 2;

/// A texture known to the device: a swapchain backbuffer (image owned by the swapchain, no
/// allocation) or a texture from `create_texture` (owns its memory).
#[derive(Debug)]
pub(crate) struct Texture {
    pub(crate) image: vk::Image,
    pub(crate) view: vk::ImageView,
    pub(crate) extent: vk::Extent2D,
    /// The layout the image is in after all recorded commands so far.
    pub(crate) layout: vk::ImageLayout,
    pub(crate) format: TextureFormat,
    pub(crate) usage: vk::ImageUsageFlags,
    /// `None` for swapchain backbuffers.
    pub(crate) allocation: Option<gpu_allocator::vulkan::Allocation>,
}

pub(crate) struct Surface {
    /// Keeps the window alive while the `VkSurfaceKHR` refers to it.
    _target: Arc<dyn SurfaceTarget>,
    surface: vk::SurfaceKHR,
    name: String,
    desired: vk::Extent2D,
    present_mode: PresentMode,
    swapchain: Option<Swapchain>,
    needs_recreate: bool,
    frames: Vec<FrameSlot>,
    frame_counter: u64,
}

struct Swapchain {
    handle: vk::SwapchainKHR,
    /// Raw texture handles of the backbuffers, by swapchain image index.
    images: Vec<u64>,
    /// Signalled by the frame's submission and waited on by its presentation, per image.
    render_finished: Vec<vk::Semaphore>,
}

struct FrameSlot {
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    /// Signalled when the acquired image is ready to be rendered to.
    acquire: vk::Semaphore,
    /// Timeline value of this slot's last submission (0: never submitted).
    last_submission: u64,
}

pub(crate) fn lock<'a, T>(
    mutex: &'a Mutex<T>,
    what: &'static str,
) -> Result<MutexGuard<'a, T>, RhiError> {
    mutex.lock().map_err(|_| RhiError::Backend {
        api: "Vulkan",
        operation: what,
        details: "a previous panic poisoned this lock; the device should be recreated".to_owned(),
    })
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
        let (Some(surface_loader), Some(_)) = (&self.loaders.surface, &self.loaders.swapchain)
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
                &self.instance.entry,
                &self.instance.instance,
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
                self.physical_device,
                self.queue_family,
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
                        self.destroy_frame_slot(slot);
                    }
                    // SAFETY: as above, unused and destroyed exactly once.
                    unsafe { surface_loader.destroy_surface(surface, None) };
                    return Err(error);
                }
            }
        }

        let mut surfaces = lock(&self.surfaces, "surface table")?;
        let raw = surfaces.insert(Surface {
            _target: window,
            surface,
            name: String::new(),
            desired: vk::Extent2D {
                width: desc.width,
                height: desc.height,
            },
            present_mode: desc.present_mode,
            swapchain: None,
            needs_recreate: true,
            frames,
            frame_counter: 0,
        });
        let handle = SurfaceHandle::from_raw(raw);
        if let Some(entry) = surfaces.get_mut(raw) {
            entry.name = format!("{handle:?}");
            self.name_object(entry.surface, &entry.name);
            for (i, slot) in entry.frames.iter().enumerate() {
                self.name_object(slot.pool, &format!("{} frame {i} command pool", entry.name));
                self.name_object(slot.cmd, &format!("{} frame {i} commands", entry.name));
                self.name_object(slot.acquire, &format!("{} frame {i} acquire", entry.name));
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
        let mut surfaces = lock(&self.surfaces, "surface table")?;
        let entry = surfaces
            .get_mut(surface.to_raw())
            .ok_or_else(|| stale("surface", surface))?;
        entry.desired = vk::Extent2D { width, height };
        entry.needs_recreate = true;
        debug!(surface = ?surface, width, height, "surface resize requested");
        Ok(())
    }

    pub(crate) fn destroy_surface_impl(&self, surface: SurfaceHandle) -> Result<(), RhiError> {
        let entry = lock(&self.surfaces, "surface table")?
            .remove(surface.to_raw())
            .ok_or_else(|| stale("surface", surface))?;
        // SAFETY: the device is alive. Waiting has no other preconditions.
        unsafe { self.raw().device_wait_idle() }.map_err(|e| from_vk("vkDeviceWaitIdle", e))?;
        self.destroy_surface_objects(entry);
        info!(surface = ?surface, "surface destroyed");
        Ok(())
    }

    pub(crate) fn begin_frame_impl(
        &self,
        surface: SurfaceHandle,
    ) -> Result<Option<Frame>, RhiError> {
        let mut surfaces = lock(&self.surfaces, "surface table")?;
        let entry = surfaces
            .get_mut(surface.to_raw())
            .ok_or_else(|| stale("surface", surface))?;

        if entry.desired.width == 0 || entry.desired.height == 0 {
            return Ok(None); // Minimized: nothing to present.
        }
        if entry.swapchain.is_none() || entry.needs_recreate {
            self.recreate_swapchain(entry)?;
        }
        let Some(swapchain) = &entry.swapchain else {
            return Ok(None); // The window currently has no drawable area.
        };
        if entry.needs_recreate {
            return Ok(None);
        }
        let Some(swapchain_loader) = &self.loaders.swapchain else {
            return Ok(None);
        };

        let slot_index =
            usize::try_from(entry.frame_counter % FRAMES_IN_FLIGHT as u64).unwrap_or(0);
        let slot = &entry.frames[slot_index];
        // The slot's command buffer and acquire semaphore are reusable once the GPU has finished
        // the slot's previous frame.
        self.wait_timeline(slot.last_submission)?;

        // SAFETY: the swapchain and semaphore are alive. The semaphore is unsignalled and has no
        // pending operations: its last use was waited on by the submission we just waited for.
        let acquired = unsafe {
            swapchain_loader.acquire_next_image(
                swapchain.handle,
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
        let backbuffer = swapchain
            .images
            .get(image_index as usize)
            .copied()
            .ok_or_else(|| RhiError::Backend {
                api: "Vulkan",
                operation: "vkAcquireNextImageKHR",
                details: format!("returned image index {image_index} beyond the swapchain"),
            })?;

        // SAFETY: the pool's command buffer is not in use: the GPU finished this slot's previous
        // submission (waited above).
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

        let format = lock(&self.textures, "texture table")?
            .get(backbuffer)
            .map(|t| t.format)
            .ok_or_else(|| RhiError::Backend {
                api: "Vulkan",
                operation: "begin_frame",
                details: "the acquired backbuffer is missing from the texture table".to_owned(),
            })?;
        let encoder = VulkanEncoder::frame(
            self.encoder_context(),
            slot.cmd,
            surface,
            image_index,
            slot_index,
        );
        Ok(Some(Frame::new(
            surface,
            TextureHandle::from_raw(backbuffer),
            format,
            Box::new(encoder),
        )))
    }

    pub(crate) fn end_frame_impl(&self, frame: Frame) -> Result<(), RhiError> {
        let (surface, backbuffer, encoder) = frame.into_parts();
        let encoder = encoder
            .into_any()
            .downcast::<VulkanEncoder>()
            .map_err(|_| RhiError::InvalidUsage {
                details: "frame was created by a different RHI backend".to_owned(),
            })?;
        let mut encoder = *encoder;
        encoder.mark_ended();
        let Some((image_index, slot_index)) = encoder.frame_info() else {
            return Err(RhiError::InvalidUsage {
                details: "end_frame was given an encoder from create_encoder; use submit"
                    .to_owned(),
            });
        };
        let mut surfaces = lock(&self.surfaces, "surface table")?;
        let entry = surfaces
            .get_mut(surface.to_raw())
            .ok_or_else(|| stale("surface", surface))?;
        let (Some(swapchain), Some(swapchain_loader)) = (&entry.swapchain, &self.loaders.swapchain)
        else {
            return Err(RhiError::InvalidUsage {
                details: format!("{surface:?} has no swapchain to present to"),
            });
        };

        // The backbuffer must be in PRESENT_SRC for presentation.
        encoder.transition_for_present(backbuffer);
        let cmd = encoder.command_buffer();
        // SAFETY: recording into `cmd` started in `begin_frame` and no render pass is open (the
        // render-pass guard closes passes on drop).
        unsafe { self.raw().end_command_buffer(cmd) }
            .map_err(|e| from_vk("vkEndCommandBuffer", e))?;

        let slot = &mut entry.frames[slot_index];
        let render_finished = swapchain.render_finished[image_index as usize];
        let mut queue = lock(&self.queue, "queue")?;
        let value = queue.timeline_value + 1;
        let waits = [vk::SemaphoreSubmitInfo::default()
            .semaphore(slot.acquire)
            .stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)];
        let command_buffers = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
        let signals = [
            vk::SemaphoreSubmitInfo::default()
                .semaphore(render_finished)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
            vk::SemaphoreSubmitInfo::default()
                .semaphore(self.timeline)
                .value(value)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
        ];
        let submit = vk::SubmitInfo2::default()
            .wait_semaphore_infos(&waits)
            .command_buffer_infos(&command_buffers)
            .signal_semaphore_infos(&signals);
        // SAFETY: the queue is externally synchronized by the `queue` lock. `cmd` is fully
        // recorded. The acquire semaphore has a pending signal from `vkAcquireNextImageKHR`. The
        // render-finished semaphore of this image is unsignalled (its previous present
        // completed before the image could be acquired again). The timeline value increases
        // monotonically.
        unsafe {
            self.raw()
                .queue_submit2(queue.queue, &[submit], vk::Fence::null())
        }
        .map_err(|e| from_vk("vkQueueSubmit2", e))?;
        queue.timeline_value = value;
        slot.last_submission = value;
        encoder.mark_submitted(value);

        let wait_present = [render_finished];
        let swapchains = [swapchain.handle];
        let indices = [image_index];
        let present = vk::PresentInfoKHR::default()
            .wait_semaphores(&wait_present)
            .swapchains(&swapchains)
            .image_indices(&indices);
        // SAFETY: externally synchronized by the `queue` lock. The image was acquired in
        // `begin_frame`, and the wait semaphore is signalled by the submission above.
        let presented = unsafe { swapchain_loader.queue_present(queue.queue, &present) };
        drop(queue);
        match presented {
            Ok(false) => {}
            Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => entry.needs_recreate = true,
            Err(e) => return Err(from_vk("vkQueuePresentKHR", e)),
        }
        entry.frame_counter += 1;

        let misuse = encoder.take_misuse();
        if misuse.is_empty() {
            Ok(())
        } else {
            Err(RhiError::InvalidUsage {
                details: misuse.join("; "),
            })
        }
    }

    /// Destroys everything a surface owns. The device must be idle.
    pub(crate) fn destroy_surface_objects(&self, surface: Surface) {
        if let Some(swapchain) = surface.swapchain {
            self.destroy_swapchain(swapchain);
        }
        for slot in surface.frames {
            self.destroy_frame_slot(slot);
        }
        if let Some(loader) = &self.loaders.surface {
            // SAFETY: the surface's swapchain was destroyed above, and the surface is destroyed
            // exactly once. Its window is released after this (when `surface` drops).
            unsafe { loader.destroy_surface(surface.surface, None) };
        }
    }

    pub(crate) fn wait_timeline(&self, value: u64) -> Result<(), RhiError> {
        if value == 0 {
            return Ok(());
        }
        let semaphores = [self.timeline];
        let values = [value];
        let info = vk::SemaphoreWaitInfo::default()
            .semaphores(&semaphores)
            .values(&values);
        // SAFETY: the timeline semaphore is alive. `value` was signalled by a past submission,
        // so the wait terminates unless the device is lost, which the result reports.
        unsafe { self.raw().wait_semaphores(&info, u64::MAX) }
            .map_err(|e| from_vk("vkWaitSemaphores", e))
    }

    fn create_frame_slot(&self) -> Result<FrameSlot, RhiError> {
        let pool_info = vk::CommandPoolCreateInfo::default().queue_family_index(self.queue_family);
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
        })
    }

    fn destroy_frame_slot(&self, slot: FrameSlot) {
        // SAFETY: the device is idle (callers wait first), so neither object is in use. Each is
        // destroyed exactly once, and destroying the pool frees its command buffer.
        unsafe {
            self.raw().destroy_semaphore(slot.acquire, None);
            self.raw().destroy_command_pool(slot.pool, None);
        }
    }

    /// Creates or replaces the surface's swapchain for its current size. The device must be
    /// idle, which this function ensures.
    fn recreate_swapchain(&self, entry: &mut Surface) -> Result<(), RhiError> {
        let (Some(surface_loader), Some(swapchain_loader)) =
            (&self.loaders.surface, &self.loaders.swapchain)
        else {
            return Ok(());
        };
        // The old swapchain's images, views and semaphores may still be in use.
        // SAFETY: the device is alive.
        unsafe { self.raw().device_wait_idle() }.map_err(|e| from_vk("vkDeviceWaitIdle", e))?;

        // SAFETY: the surface belongs to this instance and the physical device is this device's.
        let caps = unsafe {
            surface_loader
                .get_physical_device_surface_capabilities(self.physical_device, entry.surface)
        }
        .map_err(|e| from_vk("vkGetPhysicalDeviceSurfaceCapabilitiesKHR", e))?;
        let extent = choose_extent(&caps, entry.desired);
        if extent.width == 0 || extent.height == 0 {
            entry.needs_recreate = true; // Minimized. Try again when the window has a size.
            return Ok(());
        }
        // SAFETY: as above.
        let formats = unsafe {
            surface_loader.get_physical_device_surface_formats(self.physical_device, entry.surface)
        }
        .map_err(|e| from_vk("vkGetPhysicalDeviceSurfaceFormatsKHR", e))?;
        // SAFETY: as above.
        let modes = unsafe {
            surface_loader
                .get_physical_device_surface_present_modes(self.physical_device, entry.surface)
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
            .swapchain
            .as_ref()
            .map_or(vk::SwapchainKHR::null(), |s| s.handle);
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(entry.surface)
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
        // `old` is the surface's current swapchain (or null), and is retired below.
        let handle = unsafe { swapchain_loader.create_swapchain(&info, None) }
            .map_err(|e| from_vk("vkCreateSwapchainKHR", e))?;
        if let Some(previous) = entry.swapchain.take() {
            self.destroy_swapchain(previous); // The device is idle (waited above).
        }

        let mut swapchain = Swapchain {
            handle,
            images: Vec::new(),
            render_finished: Vec::new(),
        };
        if let Err(error) =
            self.populate_swapchain(&mut swapchain, &entry.name, format.format, extent)
        {
            self.destroy_swapchain(swapchain);
            return Err(error);
        }
        info!(
            surface = %entry.name,
            width = extent.width,
            height = extent.height,
            format = ?format.format,
            present_mode = ?mode,
            images = swapchain.images.len(),
            "swapchain created"
        );
        entry.swapchain = Some(swapchain);
        entry.needs_recreate = false;
        Ok(())
    }

    /// Creates the views, texture entries and present semaphores of a new swapchain. On error the
    /// partially filled swapchain is still valid to pass to `destroy_swapchain`.
    fn populate_swapchain(
        &self,
        swapchain: &mut Swapchain,
        name: &str,
        format: vk::Format,
        extent: vk::Extent2D,
    ) -> Result<(), RhiError> {
        let Some(swapchain_loader) = &self.loaders.swapchain else {
            return Ok(());
        };
        // `choose_format` only picks formats the RHI can name.
        let texture_format = rhi_format(format).ok_or_else(|| RhiError::Backend {
            api: "Vulkan",
            operation: "creating a swapchain",
            details: format!("swapchain format {format:?} has no RHI equivalent"),
        })?;
        self.name_object(swapchain.handle, &format!("{name} swapchain"));
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
            let raw = lock(&self.textures, "texture table")?.insert(Texture {
                image,
                view,
                extent,
                layout: vk::ImageLayout::UNDEFINED,
                format: texture_format,
                usage: vk::ImageUsageFlags::COLOR_ATTACHMENT,
                allocation: None,
            });
            swapchain.images.push(raw);
            self.name_object(image, &format!("{name} backbuffer {i}"));
            self.name_object(view, &format!("{name} backbuffer {i} view"));

            // SAFETY: the device is alive.
            let semaphore = unsafe {
                self.raw()
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            }
            .map_err(|e| from_vk("vkCreateSemaphore", e))?;
            swapchain.render_finished.push(semaphore);
            self.name_object(semaphore, &format!("{name} backbuffer {i} render finished"));
        }
        Ok(())
    }

    /// Destroys a swapchain and everything created for it. The device must be idle.
    fn destroy_swapchain(&self, swapchain: Swapchain) {
        if let Ok(mut textures) = self.textures.lock() {
            for raw in swapchain.images {
                if let Some(texture) = textures.remove(raw) {
                    // SAFETY: the device is idle, so the view is unused. Destroyed exactly once
                    // (removed from the table first).
                    unsafe { self.raw().destroy_image_view(texture.view, None) };
                }
            }
        }
        for semaphore in swapchain.render_finished {
            // SAFETY: the device is idle. Each semaphore is destroyed exactly once.
            unsafe { self.raw().destroy_semaphore(semaphore, None) };
        }
        if let Some(loader) = &self.loaders.swapchain {
            // SAFETY: the device is idle and the swapchain's views are destroyed. Its images are
            // owned by the swapchain and released with it.
            unsafe { loader.destroy_swapchain(swapchain.handle, None) };
        }
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
