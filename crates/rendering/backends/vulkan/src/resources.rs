//! Textures, pipelines, standalone submissions, readback and deferred destruction (ADR-0016 §1–§2,
//! ADR-0017).

use std::ffi::CString;

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use oxreal_rhi::{
    CommandEncoder, CullMode, MAX_PUSH_CONSTANT_BYTES, PipelineHandle, RenderPipelineDesc,
    RhiError, ShaderCode, SubmissionId, TextureDesc, TextureFormat, TextureHandle, TextureUsage,
};

use crate::device::VulkanDevice;
use crate::encoder::VulkanEncoder;
use crate::error::from_vk;
use crate::present::{Texture, color_subresource, lock};

/// Stages that may read push constants. One range for every pipeline keeps all pipelines
/// layout-compatible (ADR-0016 §2).
pub(crate) const PUSH_CONSTANT_STAGES: vk::ShaderStageFlags = vk::ShaderStageFlags::from_raw(
    vk::ShaderStageFlags::ALL_GRAPHICS.as_raw() | vk::ShaderStageFlags::COMPUTE.as_raw(),
);

/// A render pipeline and what draws need to know about it.
#[derive(Debug)]
pub(crate) struct Pipeline {
    pub(crate) pipeline: vk::Pipeline,
    pub(crate) push_constant_bytes: u32,
}

/// An object whose destruction waits for the GPU (ADR-0016 §1).
#[derive(Debug)]
pub(crate) enum Retired {
    Texture {
        image: vk::Image,
        view: vk::ImageView,
        allocation: Allocation,
    },
    Pipeline(vk::Pipeline),
    CommandPool(vk::CommandPool),
}

/// The Vulkan format for `format`, or an error for formats this backend doesn't support yet.
pub(crate) fn vk_format(format: TextureFormat) -> Result<vk::Format, RhiError> {
    Ok(match format {
        TextureFormat::Bgra8UnormSrgb => vk::Format::B8G8R8A8_SRGB,
        TextureFormat::Rgba8UnormSrgb => vk::Format::R8G8B8A8_SRGB,
        TextureFormat::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        other => {
            return Err(invalid(format!(
                "{other:?} is not supported by the Vulkan backend yet"
            )));
        }
    })
}

pub(crate) fn rhi_format(format: vk::Format) -> Option<TextureFormat> {
    Some(match format {
        vk::Format::B8G8R8A8_SRGB => TextureFormat::Bgra8UnormSrgb,
        vk::Format::R8G8B8A8_SRGB => TextureFormat::Rgba8UnormSrgb,
        vk::Format::B8G8R8A8_UNORM => TextureFormat::Bgra8Unorm,
        vk::Format::R8G8B8A8_UNORM => TextureFormat::Rgba8Unorm,
        _ => return None,
    })
}

fn vk_usage(usage: TextureUsage) -> vk::ImageUsageFlags {
    let mut flags = vk::ImageUsageFlags::empty();
    if usage.contains(TextureUsage::RENDER_TARGET) {
        flags |= vk::ImageUsageFlags::COLOR_ATTACHMENT;
    }
    if usage.contains(TextureUsage::COPY_SOURCE) {
        flags |= vk::ImageUsageFlags::TRANSFER_SRC;
    }
    flags
}

fn invalid(details: impl Into<String>) -> RhiError {
    RhiError::InvalidUsage {
        details: details.into(),
    }
}

/// Creates the pipeline layout every pipeline shares: push constants up to
/// [`MAX_PUSH_CONSTANT_BYTES`]. The bindless set joins it with the first sampled textures.
pub(crate) fn create_pipeline_layout(device: &ash::Device) -> Result<vk::PipelineLayout, RhiError> {
    let ranges = [vk::PushConstantRange::default()
        .stage_flags(PUSH_CONSTANT_STAGES)
        .offset(0)
        .size(MAX_PUSH_CONSTANT_BYTES)];
    let info = vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&ranges);
    // SAFETY: the device is alive. Every conformant device supports 128 bytes of push constants.
    unsafe { device.create_pipeline_layout(&info, None) }
        .map_err(|e| from_vk("vkCreatePipelineLayout", e))
}

impl VulkanDevice {
    pub(crate) fn create_texture_impl(
        &self,
        desc: &TextureDesc<'_>,
    ) -> Result<TextureHandle, RhiError> {
        if desc.width == 0 || desc.height == 0 {
            return Err(invalid(format!("texture {:?} has zero size", desc.label)));
        }
        let usage = vk_usage(desc.usage);
        if usage.is_empty() {
            return Err(invalid(format!("texture {:?} has no usage", desc.label)));
        }
        let format = vk_format(desc.format)?;
        let extent = vk::Extent2D {
            width: desc.width,
            height: desc.height,
        };
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: desc.width,
                height: desc.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: the device is alive, and the parameters describe a valid 2D image of a format
        // every Vulkan 1.3 device supports for these usages.
        let image = unsafe { self.raw().create_image(&info, None) }
            .map_err(|e| from_vk("vkCreateImage", e))?;
        // SAFETY: `image` was just created.
        let requirements = unsafe { self.raw().get_image_memory_requirements(image) };
        let allocation =
            match self.allocate(desc.label, requirements, MemoryLocation::GpuOnly, false) {
                Ok(allocation) => allocation,
                Err(error) => {
                    // SAFETY: unused and destroyed exactly once.
                    unsafe { self.raw().destroy_image(image, None) };
                    return Err(error);
                }
            };
        // SAFETY: the allocation satisfies `requirements` and is bound exactly once.
        let bound = unsafe {
            self.raw()
                .bind_image_memory(image, allocation.memory(), allocation.offset())
        };
        let view = bound
            .map_err(|e| from_vk("vkBindImageMemory", e))
            .and_then(|()| {
                let view_info = vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(color_subresource());
                // SAFETY: `image` is bound to memory and the view matches its format.
                unsafe { self.raw().create_image_view(&view_info, None) }
                    .map_err(|e| from_vk("vkCreateImageView", e))
            });
        let view = match view {
            Ok(view) => view,
            Err(error) => {
                // SAFETY: never used by the GPU. Destroyed exactly once, then its memory freed.
                unsafe { self.raw().destroy_image(image, None) };
                self.free(allocation);
                return Err(error);
            }
        };
        self.name_object(image, desc.label);
        self.name_object(view, &format!("{} view", desc.label));
        let raw = lock(&self.textures, "texture table")?.insert(Texture {
            image,
            view,
            extent,
            layout: vk::ImageLayout::UNDEFINED,
            format: desc.format,
            usage,
            allocation: Some(allocation),
        });
        Ok(TextureHandle::from_raw(raw))
    }

    pub(crate) fn destroy_texture_impl(&self, texture: TextureHandle) -> Result<(), RhiError> {
        let mut textures = lock(&self.textures, "texture table")?;
        match textures.get(texture.to_raw()) {
            None => return Err(invalid(format!("{texture:?} does not exist"))),
            Some(t) if t.allocation.is_none() => {
                return Err(invalid(format!(
                    "{texture:?} is a swapchain backbuffer; it's destroyed with its surface"
                )));
            }
            Some(_) => {}
        }
        if let Some(Texture {
            image,
            view,
            allocation: Some(allocation),
            ..
        }) = textures.remove(texture.to_raw())
        {
            drop(textures);
            self.retire(Retired::Texture {
                image,
                view,
                allocation,
            })?;
        }
        Ok(())
    }

    pub(crate) fn create_render_pipeline_impl(
        &self,
        desc: &RenderPipelineDesc<'_>,
    ) -> Result<PipelineHandle, RhiError> {
        if desc.push_constant_bytes > MAX_PUSH_CONSTANT_BYTES
            || !desc.push_constant_bytes.is_multiple_of(4)
        {
            return Err(invalid(format!(
                "pipeline {:?}: push_constant_bytes {} must be a multiple of 4 and at most {MAX_PUSH_CONSTANT_BYTES}",
                desc.label, desc.push_constant_bytes
            )));
        }
        if desc.color_formats.is_empty() {
            return Err(invalid(format!(
                "pipeline {:?} has no color formats",
                desc.label
            )));
        }
        let vertex_module = self.shader_module(desc.label, desc.vertex.code)?;
        let pixel_module = match self.shader_module(desc.label, desc.pixel.code) {
            Ok(module) => module,
            Err(error) => {
                // SAFETY: the module is unused and destroyed exactly once.
                unsafe { self.raw().destroy_shader_module(vertex_module, None) };
                return Err(error);
            }
        };
        let result = self.build_pipeline(desc, vertex_module, pixel_module);
        // SAFETY: modules may be destroyed once pipeline creation has returned.
        unsafe {
            self.raw().destroy_shader_module(vertex_module, None);
            self.raw().destroy_shader_module(pixel_module, None);
        }
        let pipeline = result?;
        self.name_object(pipeline, desc.label);
        let raw = lock(&self.pipelines, "pipeline table")?.insert(Pipeline {
            pipeline,
            push_constant_bytes: desc.push_constant_bytes,
        });
        Ok(PipelineHandle::from_raw(raw))
    }

    fn shader_module(
        &self,
        label: &str,
        code: ShaderCode<'_>,
    ) -> Result<vk::ShaderModule, RhiError> {
        let ShaderCode::Spirv(words) = code else {
            return Err(invalid(format!(
                "pipeline {label:?}: the Vulkan backend needs SPIR-V shader code"
            )));
        };
        if words.is_empty() {
            return Err(invalid(format!("pipeline {label:?}: empty SPIR-V")));
        }
        let info = vk::ShaderModuleCreateInfo::default().code(words);
        // SAFETY: the device is alive and `words` is alive across the call. Malformed SPIR-V is
        // rejected by the driver (and reported by validation) rather than causing UB, because the
        // shader compiler validated it.
        unsafe { self.raw().create_shader_module(&info, None) }
            .map_err(|e| from_vk("vkCreateShaderModule", e))
    }

    fn build_pipeline(
        &self,
        desc: &RenderPipelineDesc<'_>,
        vertex: vk::ShaderModule,
        pixel: vk::ShaderModule,
    ) -> Result<vk::Pipeline, RhiError> {
        let name = |entry: &str| {
            CString::new(entry).map_err(|_| {
                invalid(format!(
                    "pipeline {:?}: entry point contains NUL",
                    desc.label
                ))
            })
        };
        let vertex_entry = name(desc.vertex.entry_point)?;
        let pixel_entry = name(desc.pixel.entry_point)?;
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vertex)
                .name(&vertex_entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(pixel)
                .name(&pixel_entry),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let cull = match desc.cull {
            CullMode::None => vk::CullModeFlags::NONE,
            CullMode::Back => vk::CullModeFlags::BACK,
            CullMode::Front => vk::CullModeFlags::FRONT,
        };
        // Front faces wind counter-clockwise in Oxreal's Y-up clip space (ADR-0011). The negative
        // viewport height that maps it onto Vulkan also mirrors winding, so Vulkan's
        // COUNTER_CLOCKWISE matches. The triangle readback test checks this.
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(cull)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend_attachments: Vec<vk::PipelineColorBlendAttachmentState> = desc
            .color_formats
            .iter()
            .map(|_| {
                vk::PipelineColorBlendAttachmentState::default()
                    .blend_enable(false)
                    .color_write_mask(vk::ColorComponentFlags::RGBA)
            })
            .collect();
        let blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let formats = desc
            .color_formats
            .iter()
            .map(|&f| vk_format(f))
            .collect::<Result<Vec<vk::Format>, RhiError>>()?;
        let mut rendering =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&formats);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(self.pipeline_layout)
            .push_next(&mut rendering);
        // SAFETY: every structure referenced by `info` is alive across the call. The modules and
        // layout belong to this device. Dynamic rendering is enabled (a required feature), so a
        // null render pass with PipelineRenderingCreateInfo is valid.
        let pipelines = unsafe {
            self.raw()
                .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
        }
        .map_err(|(_, e)| from_vk("vkCreateGraphicsPipelines", e))?;
        pipelines
            .into_iter()
            .next()
            .ok_or_else(|| invalid("vkCreateGraphicsPipelines returned no pipeline"))
    }

    pub(crate) fn destroy_render_pipeline_impl(
        &self,
        pipeline: PipelineHandle,
    ) -> Result<(), RhiError> {
        let removed = lock(&self.pipelines, "pipeline table")?
            .remove(pipeline.to_raw())
            .ok_or_else(|| invalid(format!("{pipeline:?} does not exist")))?;
        self.retire(Retired::Pipeline(removed.pipeline))
    }

    pub(crate) fn create_encoder_impl(
        &self,
        label: &str,
    ) -> Result<Box<dyn CommandEncoder>, RhiError> {
        self.collect_garbage()?;
        let (pool, cmd) = self.begin_one_time_commands(label)?;
        Ok(Box::new(VulkanEncoder::standalone(
            self.encoder_context(),
            cmd,
            pool,
        )))
    }

    pub(crate) fn submit_impl(
        &self,
        encoder: Box<dyn CommandEncoder>,
    ) -> Result<SubmissionId, RhiError> {
        let mut encoder = *encoder
            .into_any()
            .downcast::<VulkanEncoder>()
            .map_err(|_| invalid("encoder was created by a different RHI backend"))?;
        let Some((pool, cmd)) = encoder.take_standalone() else {
            return Err(invalid(
                "frame encoders are submitted by end_frame, not submit",
            ));
        };
        let result = self.submit_one_time_commands(pool, cmd);
        if let Ok(id) = result {
            encoder.mark_submitted(id.0);
        }
        let misuse = encoder.take_misuse();
        let id = result?;
        if misuse.is_empty() {
            Ok(id)
        } else {
            Err(invalid(misuse.join("; ")))
        }
    }

    pub(crate) fn read_texture_impl(&self, texture: TextureHandle) -> Result<Vec<u8>, RhiError> {
        let (image, extent, layout, format) = {
            let textures = lock(&self.textures, "texture table")?;
            let t = textures
                .get(texture.to_raw())
                .ok_or_else(|| invalid(format!("{texture:?} does not exist")))?;
            if !t.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC) {
                return Err(invalid(format!(
                    "{texture:?} was not created with TextureUsage::COPY_SOURCE"
                )));
            }
            (t.image, t.extent, t.layout, t.format)
        };
        let size = u64::from(extent.width)
            * u64::from(extent.height)
            * u64::from(format.bytes_per_pixel());

        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: the device is alive and the size is non-zero (textures can't be empty).
        let buffer = unsafe { self.raw().create_buffer(&buffer_info, None) }
            .map_err(|e| from_vk("vkCreateBuffer", e))?;
        // SAFETY: `buffer` was just created.
        let requirements = unsafe { self.raw().get_buffer_memory_requirements(buffer) };
        // GpuToCpu memory is always HOST_COHERENT in gpu-allocator, so no invalidation is needed.
        let allocation =
            match self.allocate("readback", requirements, MemoryLocation::GpuToCpu, true) {
                Ok(allocation) => allocation,
                Err(error) => {
                    // SAFETY: unused and destroyed exactly once.
                    unsafe { self.raw().destroy_buffer(buffer, None) };
                    return Err(error);
                }
            };

        let copied = self.copy_texture_to_buffer(image, extent, layout, buffer, &allocation);
        let pixels = copied.and_then(|()| {
            allocation
                .mapped_slice()
                .and_then(|bytes| bytes.get(..usize::try_from(size).ok()?))
                .map(<[u8]>::to_vec)
                .ok_or_else(|| invalid("readback memory is not mapped"))
        });
        // SAFETY: the copy has completed (waited on) or failed, so the buffer is unused.
        unsafe { self.raw().destroy_buffer(buffer, None) };
        self.free(allocation);
        if pixels.is_ok()
            && let Ok(mut textures) = self.textures.lock()
            && let Some(t) = textures.get_mut(texture.to_raw())
        {
            t.layout = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
        }
        pixels
    }

    fn copy_texture_to_buffer(
        &self,
        image: vk::Image,
        extent: vk::Extent2D,
        layout: vk::ImageLayout,
        buffer: vk::Buffer,
        allocation: &Allocation,
    ) -> Result<(), RhiError> {
        // SAFETY: the allocation satisfies the buffer's requirements and is bound exactly once.
        unsafe {
            self.raw()
                .bind_buffer_memory(buffer, allocation.memory(), allocation.offset())
        }
        .map_err(|e| from_vk("vkBindBufferMemory", e))?;

        let (pool, cmd) = self.begin_one_time_commands("readback")?;
        let image_barrier = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
            .old_layout(layout)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .image(image)
            .subresource_range(color_subresource())];
        let region = [vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })];
        let buffer_barrier = [vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)
            .buffer(buffer)
            .size(vk::WHOLE_SIZE)];
        // SAFETY: `cmd` is recording. The image and buffer are alive, and the barriers make the
        // previous writes visible to the copy and the copy visible to the host read after the
        // wait below.
        unsafe {
            self.raw().cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&image_barrier),
            );
            self.raw().cmd_copy_image_to_buffer(
                cmd,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &region,
            );
            self.raw().cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().buffer_memory_barriers(&buffer_barrier),
            );
        }
        let id = self.submit_one_time_commands(pool, cmd)?;
        self.wait_for_submission_impl(id)
    }

    pub(crate) fn wait_for_submission_impl(
        &self,
        submission: SubmissionId,
    ) -> Result<(), RhiError> {
        self.wait_timeline(submission.0)?;
        self.collect_garbage()
    }

    /// Creates a transient command pool with one command buffer in the recording state.
    fn begin_one_time_commands(
        &self,
        label: &str,
    ) -> Result<(vk::CommandPool, vk::CommandBuffer), RhiError> {
        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::TRANSIENT)
            .queue_family_index(self.queue_family);
        // SAFETY: the device is alive and the queue family is the device's.
        let pool = unsafe { self.raw().create_command_pool(&pool_info, None) }
            .map_err(|e| from_vk("vkCreateCommandPool", e))?;
        let alloc = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: `pool` was just created. The buffer is begun right after allocation.
        let cmd = unsafe { self.raw().allocate_command_buffers(&alloc) }
            .map_err(|e| from_vk("vkAllocateCommandBuffers", e))
            .and_then(|buffers| {
                let cmd = buffers[0];
                // SAFETY: a freshly allocated primary command buffer.
                unsafe { self.raw().begin_command_buffer(cmd, &begin) }
                    .map(|()| cmd)
                    .map_err(|e| from_vk("vkBeginCommandBuffer", e))
            });
        match cmd {
            Ok(cmd) => {
                self.name_object(pool, &format!("{label} command pool"));
                self.name_object(cmd, &format!("{label} commands"));
                Ok((pool, cmd))
            }
            Err(error) => {
                // SAFETY: never submitted. Destroying the pool frees its buffer.
                unsafe { self.raw().destroy_command_pool(pool, None) };
                Err(error)
            }
        }
    }

    /// Ends, submits and retires a one-time command buffer. The pool is destroyed once the GPU
    /// has finished.
    fn submit_one_time_commands(
        &self,
        pool: vk::CommandPool,
        cmd: vk::CommandBuffer,
    ) -> Result<SubmissionId, RhiError> {
        // SAFETY: `cmd` is recording and has no open render pass (the guard closes passes).
        if let Err(e) = unsafe { self.raw().end_command_buffer(cmd) } {
            // SAFETY: never submitted.
            unsafe { self.raw().destroy_command_pool(pool, None) };
            return Err(from_vk("vkEndCommandBuffer", e));
        }
        let mut queue = lock(&self.queue, "queue")?;
        let value = queue.timeline_value + 1;
        let command_buffers = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
        let signals = [vk::SemaphoreSubmitInfo::default()
            .semaphore(self.timeline)
            .value(value)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        let submit = vk::SubmitInfo2::default()
            .command_buffer_infos(&command_buffers)
            .signal_semaphore_infos(&signals);
        // SAFETY: the queue is externally synchronized by the lock. `cmd` is fully recorded. The
        // timeline value increases monotonically.
        let submitted = unsafe {
            self.raw()
                .queue_submit2(queue.queue, &[submit], vk::Fence::null())
        };
        if let Err(e) = submitted {
            drop(queue);
            // SAFETY: the submission failed, so the buffer is not pending.
            unsafe { self.raw().destroy_command_pool(pool, None) };
            return Err(from_vk("vkQueueSubmit2", e));
        }
        queue.timeline_value = value;
        drop(queue);
        lock(&self.retired, "retire queue")?
            .retire_after_submission(Retired::CommandPool(pool), value);
        Ok(SubmissionId(value))
    }

    /// Queues `object` for destruction after every submission made so far, and every encoder
    /// still recording, has finished with it (see [`crate::retire`]).
    pub(crate) fn retire(&self, object: Retired) -> Result<(), RhiError> {
        let after = lock(&self.queue, "queue")?.timeline_value;
        lock(&self.retired, "retire queue")?.retire(object, after);
        self.collect_garbage()
    }

    /// Destroys retired objects that nothing can use any more.
    pub(crate) fn collect_garbage(&self) -> Result<(), RhiError> {
        // SAFETY: the timeline semaphore is alive.
        let completed = unsafe { self.raw().get_semaphore_counter_value(self.timeline) }
            .map_err(|e| from_vk("vkGetSemaphoreCounterValue", e))?;
        let ready = lock(&self.retired, "retire queue")?.take_ready(completed);
        for object in ready {
            self.destroy_retired(object);
        }
        Ok(())
    }

    /// Destroys a retired object now. The GPU must have finished with it.
    pub(crate) fn destroy_retired(&self, object: Retired) {
        match object {
            Retired::Texture {
                image,
                view,
                allocation,
            } => {
                // SAFETY: the GPU has finished every submission that could use them (timeline
                // reached). Each is destroyed exactly once, and the memory is freed after.
                unsafe {
                    self.raw().destroy_image_view(view, None);
                    self.raw().destroy_image(image, None);
                }
                self.free(allocation);
            }
            // SAFETY: as above.
            Retired::Pipeline(pipeline) => unsafe { self.raw().destroy_pipeline(pipeline, None) },
            // SAFETY: as above. Destroying the pool frees its command buffers.
            Retired::CommandPool(pool) => unsafe { self.raw().destroy_command_pool(pool, None) },
        }
    }

    fn allocate(
        &self,
        name: &str,
        requirements: vk::MemoryRequirements,
        location: MemoryLocation,
        linear: bool,
    ) -> Result<Allocation, RhiError> {
        let mut allocator = lock(&self.allocator, "allocator")?;
        let allocator = allocator
            .as_mut()
            .ok_or_else(|| invalid("the allocator was already shut down"))?;
        allocator
            .allocate(&AllocationCreateDesc {
                name,
                requirements,
                location,
                linear,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| match e {
                gpu_allocator::AllocationError::OutOfMemory => RhiError::OutOfMemory {
                    kind: "GPU",
                    operation: "allocating memory",
                },
                other => RhiError::Backend {
                    api: "Vulkan",
                    operation: "gpu-allocator allocate",
                    details: other.to_string(),
                },
            })
    }

    pub(crate) fn free(&self, allocation: Allocation) {
        let freed = match self.allocator.lock() {
            Ok(mut guard) => guard.as_mut().map(|a| a.free(allocation)),
            Err(poisoned) => poisoned.into_inner().as_mut().map(|a| a.free(allocation)),
        };
        if let Some(Err(e)) = freed {
            oxreal_diagnostics::warn!(error = %e, "freeing GPU memory failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_round_trip() {
        for format in [
            TextureFormat::Bgra8UnormSrgb,
            TextureFormat::Rgba8UnormSrgb,
            TextureFormat::Bgra8Unorm,
            TextureFormat::Rgba8Unorm,
        ] {
            assert_eq!(rhi_format(vk_format(format).unwrap()), Some(format));
        }
        assert_eq!(rhi_format(vk::Format::A2B10G10R10_UNORM_PACK32), None);
    }

    #[test]
    fn usages_map_to_vulkan_flags() {
        let both = vk_usage(TextureUsage::RENDER_TARGET | TextureUsage::COPY_SOURCE);
        assert!(
            both.contains(
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC
            )
        );
        assert!(vk_usage(TextureUsage::default()).is_empty());
    }

    #[test]
    fn push_constant_stages_cover_graphics_and_compute() {
        assert!(PUSH_CONSTANT_STAGES.contains(vk::ShaderStageFlags::VERTEX));
        assert!(PUSH_CONSTANT_STAGES.contains(vk::ShaderStageFlags::FRAGMENT));
        assert!(PUSH_CONSTANT_STAGES.contains(vk::ShaderStageFlags::COMPUTE));
    }
}
