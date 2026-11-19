//! Textures, pipelines, standalone submissions, readback and deferred destruction (ADR-0016 §1–§2,
//! ADR-0017).

use std::ffi::CString;
use std::sync::Arc;

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use oxreal_rhi::{
    CommandEncoder, CompareOp, CullMode, MAX_PUSH_CONSTANT_BYTES, PipelineHandle,
    RenderPipelineDesc, RhiError, ShaderEntry, SubmissionId, TextureDesc, TextureFormat,
    TextureHandle, TextureUsage,
};

use crate::device::{QueueState, Shared, lock};
use crate::encoder::{Registration, VulkanEncoder};
use crate::error::from_vk;
use crate::present::{SurfaceObjects, Texture, color_subresource};
use crate::spirv::{self, Stage};

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
    pub(crate) color_formats: Vec<TextureFormat>,
    /// The depth attachment format it renders with, if any.
    pub(crate) depth_format: Option<TextureFormat>,
}

/// A sampler and its bindless index.
#[derive(Debug)]
pub(crate) struct Sampler {
    pub(crate) sampler: vk::Sampler,
    pub(crate) index: u32,
}

/// An object whose destruction waits until nothing can use it any more (ADR-0016 §1). Bindless
/// indices are released with their object, so they're reused only after that point.
#[derive(Debug)]
pub(crate) enum Retired {
    Texture {
        image: vk::Image,
        view: vk::ImageView,
        allocation: Allocation,
        bindless_index: Option<u32>,
    },
    Sampler {
        sampler: vk::Sampler,
        index: u32,
    },
    Buffer {
        buffer: vk::Buffer,
        allocation: Allocation,
    },
    Pipeline(vk::Pipeline),
    CommandPool(vk::CommandPool),
    Surface(Box<SurfaceObjects>),
}

/// The texture a readback copies from, as it is when the copy is recorded.
#[derive(Debug, Clone, Copy)]
struct ReadbackSource {
    image: vk::Image,
    extent: vk::Extent2D,
    /// The layout before the copy, in submission order (the texture is claimed).
    old_layout: vk::ImageLayout,
    /// The layout to return to afterwards, for sampled textures.
    resting: Option<vk::ImageLayout>,
}

/// A one-time submission that failed, and whether its work may still be pending.
#[derive(Debug)]
pub(crate) struct SubmitFailure {
    pub(crate) error: RhiError,
    /// `vkQueueSubmit2` returned `VK_ERROR_DEVICE_LOST`. The spec counts that as success when
    /// deciding whether a command buffer is pending or a resource is in use ("Lost Device"), so
    /// what the submission references must outlive the device's idle point. Otherwise nothing
    /// was submitted: recording failed, or the submit ran out of memory, after which the spec
    /// guarantees referenced resources are unaffected.
    pub(crate) pending: bool,
}

impl SubmitFailure {
    pub(crate) fn not_submitted(error: RhiError) -> Self {
        Self {
            error,
            pending: false,
        }
    }

    fn from_submit(error: RhiError) -> Self {
        let pending = matches!(error, RhiError::DeviceLost { .. });
        Self { error, pending }
    }

    /// Disposes of an object the failed submission referenced: destroyed now if nothing can be
    /// pending, otherwise kept until device teardown (after the device is idle).
    pub(crate) fn dispose(&self, shared: &Shared, object: Retired) {
        if self.pending {
            lock(&shared.retired).retire_after_submission(object, u64::MAX);
        } else {
            // Not in use: never submitted, or a failed submit left it unaffected.
            shared.destroy_retired(object);
        }
    }
}

/// The Vulkan format for `format`, or an error for formats this backend doesn't support yet.
pub(crate) fn vk_format(format: TextureFormat) -> Result<vk::Format, RhiError> {
    Ok(match format {
        TextureFormat::Bgra8UnormSrgb => vk::Format::B8G8R8A8_SRGB,
        TextureFormat::Rgba8UnormSrgb => vk::Format::R8G8B8A8_SRGB,
        TextureFormat::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        TextureFormat::Depth32Float => vk::Format::D32_SFLOAT,
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
    if usage.contains(TextureUsage::SAMPLED) {
        flags |= vk::ImageUsageFlags::SAMPLED;
    }
    if usage.contains(TextureUsage::COPY_DESTINATION) {
        flags |= vk::ImageUsageFlags::TRANSFER_DST;
    }
    if usage.contains(TextureUsage::DEPTH_TARGET) {
        flags |= vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT;
    }
    flags
}

fn vk_compare_op(op: CompareOp) -> vk::CompareOp {
    match op {
        CompareOp::Never => vk::CompareOp::NEVER,
        CompareOp::Less => vk::CompareOp::LESS,
        CompareOp::Equal => vk::CompareOp::EQUAL,
        CompareOp::LessOrEqual => vk::CompareOp::LESS_OR_EQUAL,
        CompareOp::Greater => vk::CompareOp::GREATER,
        CompareOp::NotEqual => vk::CompareOp::NOT_EQUAL,
        CompareOp::GreaterOrEqual => vk::CompareOp::GREATER_OR_EQUAL,
        CompareOp::Always => vk::CompareOp::ALWAYS,
    }
}

/// Rejects usages a format doesn't support. Depth formats are only depth targets so far: they
/// can't be sampled (the bindless set holds color textures), copied or rendered as color.
///
/// Paths that use the color aspect rely on this: `make_sampleable`, `write_texture`
/// (`sampling.rs`) and `read_texture`. The two copies also refuse depth formats themselves.
fn check_format_usage(desc: &TextureDesc<'_>) -> Result<(), RhiError> {
    let (format, usage) = (desc.format, desc.usage);
    if format.is_depth() && usage != TextureUsage::DEPTH_TARGET {
        return Err(invalid(format!(
            "texture {:?}: {format:?} supports only TextureUsage::DEPTH_TARGET so far (no sampling, copies or color rendering)",
            desc.label
        )));
    }
    if !format.is_depth() && usage.contains(TextureUsage::DEPTH_TARGET) {
        return Err(invalid(format!(
            "texture {:?}: DEPTH_TARGET needs a depth format such as Depth32Float, not {format:?}",
            desc.label
        )));
    }
    Ok(())
}

/// The subresource of a texture in `format`: its depth aspect for depth formats, its color
/// aspect otherwise.
pub(crate) fn subresource_range(format: TextureFormat) -> vk::ImageSubresourceRange {
    if format.is_depth() {
        vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::DEPTH)
            .level_count(1)
            .layer_count(1)
    } else {
        color_subresource()
    }
}

pub(crate) fn invalid(details: impl Into<String>) -> RhiError {
    RhiError::InvalidUsage {
        details: details.into(),
    }
}

/// Creates the pipeline layout every pipeline shares (ADR-0016 §2): the bindless set as set 0
/// and push constants up to [`MAX_PUSH_CONSTANT_BYTES`].
pub(crate) fn create_pipeline_layout(
    device: &ash::Device,
    bindless_layout: vk::DescriptorSetLayout,
) -> Result<vk::PipelineLayout, RhiError> {
    let ranges = [vk::PushConstantRange::default()
        .stage_flags(PUSH_CONSTANT_STAGES)
        .offset(0)
        .size(MAX_PUSH_CONSTANT_BYTES)];
    let set_layouts = [bindless_layout];
    let info = vk::PipelineLayoutCreateInfo::default()
        .set_layouts(&set_layouts)
        .push_constant_ranges(&ranges);
    // SAFETY: the device and the bindless set layout are alive. 128 bytes is the minimum
    // maxPushConstantsSize every conformant device supports, and one update-after-bind set is
    // within every limit (the set's counts were sized to the device).
    unsafe { device.create_pipeline_layout(&info, None) }
        .map_err(|e| from_vk("vkCreatePipelineLayout", e))
}

impl Shared {
    pub(crate) fn create_texture(&self, desc: &TextureDesc<'_>) -> Result<TextureHandle, RhiError> {
        if desc.width == 0 || desc.height == 0 {
            return Err(invalid(format!("texture {:?} has zero size", desc.label)));
        }
        let max = self.limits.max_image_dimension_2d;
        if desc.width > max || desc.height > max {
            return Err(invalid(format!(
                "texture {:?} is {}×{}, above this device's limit of {max} pixels per side",
                desc.label, desc.width, desc.height
            )));
        }
        let usage = vk_usage(desc.usage);
        // A render pass's area must fit the framebuffer limits, which may be below
        // maxImageDimension2D (VUID-VkRenderingInfo-pNext-07815/07816).
        let attachment =
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT;
        let (max_width, max_height) = (
            self.limits.max_framebuffer_width,
            self.limits.max_framebuffer_height,
        );
        if usage.intersects(attachment) && (desc.width > max_width || desc.height > max_height) {
            return Err(invalid(format!(
                "texture {:?} is {}×{}, above this device's render target limit of {max_width}×{max_height}",
                desc.label, desc.width, desc.height
            )));
        }
        if usage.is_empty() {
            return Err(invalid(format!("texture {:?} has no usage", desc.label)));
        }
        check_format_usage(desc)?;
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
        // SAFETY: the device is alive. The size is non-zero and within maxImageDimension2D
        // (checked above). The usage suits the format (`check_format_usage`), which is either:
        // - one of the 8-bit RGBA/BGRA formats (UNORM or sRGB), for which the Vulkan spec makes
        //   these optimal-tiling features mandatory: COLOR_ATTACHMENT, TRANSFER_SRC and
        //   TRANSFER_DST, and SAMPLED_IMAGE with linear filtering; or
        // - D32_SFLOAT with only DEPTH_STENCIL_ATTACHMENT. The spec doesn't guarantee that, so
        //   adapter selection requires it (`device::probe`).
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
                    .subresource_range(subresource_range(desc.format));
                // SAFETY: `image` is bound to memory, and the view matches its format and aspect.
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

        // `make_sampleable` uses the color aspect and the bindless set holds color textures;
        // `check_format_usage` keeps depth formats from SAMPLED.
        let (layout, bindless_index, resting_layout) =
            if usage.contains(vk::ImageUsageFlags::SAMPLED) {
                match self.make_sampleable(image, view, desc.label) {
                    Ok(index) => (
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        Some(index),
                        Some(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
                    ),
                    Err(failure) => {
                        failure.dispose(
                            self,
                            Retired::Texture {
                                image,
                                view,
                                allocation,
                                bindless_index: None,
                            },
                        );
                        return Err(failure.error);
                    }
                }
            } else {
                (vk::ImageLayout::UNDEFINED, None, None)
            };
        let raw = lock(&self.textures).insert(Texture {
            image,
            view,
            extent,
            layout,
            format: desc.format,
            usage,
            allocation: Some(allocation),
            user: None,
            frame_owner: None,
            bindless_index,
            resting_layout,
        });
        Ok(TextureHandle::from_raw(raw))
    }

    /// Gives a new texture a bindless index and moves it into its resting layout,
    /// SHADER_READ_ONLY_OPTIMAL, so shaders may sample it from any later submission. Doesn't
    /// wait: the transition is submitted before anything that could sample the texture. On
    /// error the index is released, and the failure says whether the image may still be in use.
    fn make_sampleable(
        &self,
        image: vk::Image,
        view: vk::ImageView,
        label: &str,
    ) -> Result<u32, SubmitFailure> {
        let index = lock(&self.bindless_slots)
            .textures
            .allocate()
            .ok_or_else(|| {
                SubmitFailure::not_submitted(RhiError::OutOfMemory {
                    kind: "bindless texture slot",
                    operation: "creating a sampled texture",
                })
            })?;
        let release = |failure: SubmitFailure| {
            lock(&self.bindless_slots).textures.release(index);
            failure
        };
        let (pool, cmd) = self
            .begin_one_time_commands(label)
            .map_err(|error| release(SubmitFailure::not_submitted(error)))?;
        let barrier = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::NONE)
            .src_access_mask(vk::AccessFlags2::NONE)
            .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .dst_access_mask(vk::AccessFlags2::MEMORY_READ)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(image)
            .subresource_range(color_subresource())];
        // SAFETY: `cmd` is recording. The image was just created and nothing else knows it yet.
        // Its contents are undefined until written, so transitioning from UNDEFINED discards
        // nothing, and the destination scope covers every later sampling.
        unsafe {
            self.raw().cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&barrier),
            );
        }
        self.submit_one_time_commands(pool, cmd, None)
            .map_err(release)?;
        let slots = lock(&self.bindless_slots);
        self.bindless.write_texture(self.raw(), index, view);
        drop(slots);
        Ok(index)
    }

    pub(crate) fn texture_index(&self, texture: TextureHandle) -> Result<u32, RhiError> {
        let textures = lock(&self.textures);
        let found = textures
            .get(texture.to_raw())
            .ok_or_else(|| invalid(format!("{texture:?} does not exist")))?;
        found.bindless_index.ok_or_else(|| {
            invalid(format!(
                "{texture:?} was not created with TextureUsage::SAMPLED"
            ))
        })
    }

    pub(crate) fn destroy_texture(&self, texture: TextureHandle) -> Result<(), RhiError> {
        let mut textures = lock(&self.textures);
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
            bindless_index,
            ..
        }) = textures.remove(texture.to_raw())
        {
            drop(textures);
            self.retire(Retired::Texture {
                image,
                view,
                allocation,
                bindless_index,
            });
        }
        Ok(())
    }

    pub(crate) fn create_render_pipeline(
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
        let max_attachments = self.limits.max_color_attachments as usize;
        if desc.color_formats.len() > max_attachments {
            return Err(invalid(format!(
                "pipeline {:?} has {} color formats; this device allows {max_attachments}",
                desc.label,
                desc.color_formats.len()
            )));
        }
        if desc.color_formats.is_empty() && desc.depth.is_none() {
            return Err(invalid(format!(
                "pipeline {:?} renders to nothing: it needs a color format or a depth state",
                desc.label
            )));
        }
        if let Some(format) = desc.color_formats.iter().find(|f| f.is_depth()) {
            return Err(invalid(format!(
                "pipeline {:?}: {format:?} is a depth format; declare it in `depth`, not `color_formats`",
                desc.label
            )));
        }
        let depth_format = match desc.depth {
            Some(depth) if !depth.format.is_depth() => {
                return Err(invalid(format!(
                    "pipeline {:?}: depth format {:?} is not a depth format",
                    desc.label, depth.format
                )));
            }
            Some(depth) => Some(vk_format(depth.format)?),
            None => None,
        };
        let formats = desc
            .color_formats
            .iter()
            .map(|&f| vk_format(f))
            .collect::<Result<Vec<vk::Format>, RhiError>>()?;
        let vertex_module = self.shader_module(desc, &desc.vertex, Stage::Vertex)?;
        let pixel_module = match self.shader_module(desc, &desc.pixel, Stage::Fragment) {
            Ok(module) => module,
            Err(error) => {
                // SAFETY: the module is unused and destroyed exactly once.
                unsafe { self.raw().destroy_shader_module(vertex_module, None) };
                return Err(error);
            }
        };
        let result = self.build_pipeline(desc, &formats, depth_format, vertex_module, pixel_module);
        // SAFETY: modules may be destroyed once pipeline creation has returned.
        unsafe {
            self.raw().destroy_shader_module(vertex_module, None);
            self.raw().destroy_shader_module(pixel_module, None);
        }
        let pipeline = result?;
        self.name_object(pipeline, desc.label);
        let raw = lock(&self.pipelines).insert(Pipeline {
            pipeline,
            push_constant_bytes: desc.push_constant_bytes,
            color_formats: desc.color_formats.to_vec(),
            depth_format: desc.depth.map(|depth| depth.format),
        });
        Ok(PipelineHandle::from_raw(raw))
    }

    /// Checks one stage's SPIR-V (see [`spirv`]) and creates its module.
    fn shader_module(
        &self,
        desc: &RenderPipelineDesc<'_>,
        entry: &ShaderEntry<'_>,
        stage: Stage,
    ) -> Result<vk::ShaderModule, RhiError> {
        let label = desc.label;
        let Some(words) = entry.code.as_spirv() else {
            return Err(invalid(format!(
                "pipeline {label:?}: the Vulkan backend needs SPIR-V shader code"
            )));
        };
        let info = spirv::inspect(words)
            .map_err(|problem| invalid(format!("pipeline {label:?}: {problem}")))?;
        if !info.has_entry_point(stage, entry.entry_point) {
            let found: Vec<String> = info
                .entry_points
                .iter()
                .map(|(s, n)| format!("{n} ({s:?})"))
                .collect();
            return Err(invalid(format!(
                "pipeline {label:?}: no {stage:?} entry point {:?} in its shader code; it has: {}",
                entry.entry_point,
                found.join(", ")
            )));
        }
        if info.push_constant_bytes > desc.push_constant_bytes {
            return Err(invalid(format!(
                "pipeline {label:?}: its shaders read {} push-constant bytes, but it declares {}",
                info.push_constant_bytes, desc.push_constant_bytes
            )));
        }
        let create_info = vk::ShaderModuleCreateInfo::default().code(words);
        // SAFETY: the device is alive and `words` outlives the call. The words are valid SPIR-V
        // for Vulkan 1.3: `ShaderCode::from_spirv_unchecked` makes that the caller's promise,
        // and `spirv::inspect` checked the header and instruction framing.
        unsafe { self.raw().create_shader_module(&create_info, None) }
            .map_err(|e| from_vk("vkCreateShaderModule", e))
    }

    fn build_pipeline(
        &self,
        desc: &RenderPipelineDesc<'_>,
        formats: &[vk::Format],
        depth_format: Option<vk::Format>,
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
        let blend_attachments: Vec<vk::PipelineColorBlendAttachmentState> = formats
            .iter()
            .map(|_| {
                vk::PipelineColorBlendAttachmentState::default()
                    .blend_enable(false)
                    .color_write_mask(vk::ColorComponentFlags::RGBA)
            })
            .collect();
        let blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        // Without a depth state the pipeline has no depth attachment. The default struct is valid
        // anyway: every test is disabled and its ops are valid enum values.
        let depth =
            desc.depth
                .map_or_else(vk::PipelineDepthStencilStateCreateInfo::default, |depth| {
                    vk::PipelineDepthStencilStateCreateInfo::default()
                        .depth_test_enable(true)
                        .depth_write_enable(depth.write)
                        .depth_compare_op(vk_compare_op(depth.compare))
                });
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(formats)
            .depth_attachment_format(depth_format.unwrap_or(vk::Format::UNDEFINED));
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .depth_stencil_state(&depth)
            .dynamic_state(&dynamic)
            .layout(self.pipeline_layout)
            .push_next(&mut rendering);
        // SAFETY: every structure referenced by `info` (stages, entry-point names, states,
        // formats) is alive across the call. The modules and layout belong to this device, the
        // entry points exist for their stages, and the shaders' push constants fit the layout's
        // range (checked in `shader_module`). Dynamic rendering is enabled (a required feature),
        // so a null render pass with PipelineRenderingCreateInfo is valid. The color attachment
        // count is within maxColorAttachments. The depth format, if any, is D32_SFLOAT, which
        // adapter selection requires to support depth attachments. Stencil tests stay disabled.
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

    pub(crate) fn destroy_render_pipeline(&self, pipeline: PipelineHandle) -> Result<(), RhiError> {
        let removed = lock(&self.pipelines)
            .remove(pipeline.to_raw())
            .ok_or_else(|| invalid(format!("{pipeline:?} does not exist")))?;
        self.retire(Retired::Pipeline(removed.pipeline));
        Ok(())
    }

    pub(crate) fn create_encoder(
        shared: &Arc<Self>,
        label: &str,
    ) -> Result<Box<dyn CommandEncoder>, RhiError> {
        shared.collect_garbage()?;
        let (pool, cmd) = shared.begin_one_time_commands(label)?;
        Ok(Box::new(VulkanEncoder::standalone(
            Arc::clone(shared),
            cmd,
            pool,
        )))
    }

    pub(crate) fn submit(
        &self,
        encoder: Box<dyn CommandEncoder>,
    ) -> Result<SubmissionId, RhiError> {
        let mut encoder = *encoder
            .into_any()
            .downcast::<VulkanEncoder>()
            .map_err(|_| invalid("encoder was created by a different RHI backend"))?;
        // Before any state changes: an encoder of another device must never reach this queue.
        if !encoder.belongs_to(self) {
            return Err(invalid("submit was given an encoder of a different device"));
        }
        let Some((pool, cmd)) = encoder.take_standalone() else {
            return Err(invalid(
                "frame encoders are submitted by end_frame, not submit",
            ));
        };
        encoder.finish_recording();
        let value = self
            .submit_one_time_commands(pool, cmd, Some(encoder.registration()))
            .map_err(|failure| failure.error)?;
        let misuse = encoder.take_misuse();
        if misuse.is_empty() {
            Ok(SubmissionId(value))
        } else {
            Err(invalid(misuse.join("; ")))
        }
    }

    pub(crate) fn read_texture(
        shared: &Arc<Self>,
        texture: TextureHandle,
    ) -> Result<Vec<u8>, RhiError> {
        // Registered like an encoder, so destroying the texture meanwhile waits for this copy,
        // and claiming it keeps its layout tracking in submission order (ADR-0016 §1).
        let mut registration = Registration::new(shared);
        let (image, extent, old_layout, format, resting) = {
            let mut textures = lock(&shared.textures);
            let t = textures
                .get_mut(texture.to_raw())
                .ok_or_else(|| invalid(format!("{texture:?} does not exist")))?;
            if !t.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC) {
                return Err(invalid(format!(
                    "{texture:?} was not created with TextureUsage::COPY_SOURCE"
                )));
            }
            // The copy below uses the color aspect. Depth textures can't get COPY_SOURCE today
            // (`check_format_usage`); this keeps the copy valid if that changes.
            if t.format.is_depth() {
                return Err(invalid(format!(
                    "read_texture: {texture:?} is a depth texture; reading depth back isn't supported"
                )));
            }
            registration
                .claim(texture.to_raw(), t)
                .map_err(|problem| invalid(format!("read_texture: {problem}")))?;
            let old = t.layout;
            // A sampled texture goes back to its resting layout in the same command buffer.
            let resting = t.resting_layout;
            t.layout = resting.unwrap_or(vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
            (t.image, t.extent, old, t.format, resting)
        };
        let size = u64::from(extent.width)
            * u64::from(extent.height)
            * u64::from(format.bytes_per_pixel());

        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: the device is alive and the size is non-zero (textures can't be empty).
        let buffer = unsafe { shared.raw().create_buffer(&buffer_info, None) }
            .map_err(|e| from_vk("vkCreateBuffer", e))?;
        // SAFETY: `buffer` was just created.
        let requirements = unsafe { shared.raw().get_buffer_memory_requirements(buffer) };
        // GpuToCpu memory is always HOST_COHERENT in gpu-allocator 0.28 (preferred and required
        // flags), so no vkInvalidateMappedMemoryRanges is needed before reading.
        let allocation =
            match shared.allocate("readback", requirements, MemoryLocation::GpuToCpu, true) {
                Ok(allocation) => allocation,
                Err(error) => {
                    // SAFETY: unused and destroyed exactly once.
                    unsafe { shared.raw().destroy_buffer(buffer, None) };
                    return Err(error);
                }
            };

        let submitted = shared.copy_texture_to_buffer(
            ReadbackSource {
                image,
                extent,
                old_layout,
                resting,
            },
            buffer,
            &allocation,
            &mut registration,
        );
        let value = match submitted {
            Ok(value) => value,
            Err(failure) => {
                // Freed now unless the copy may be pending (device lost). If nothing was
                // submitted, `registration` restores the texture's layout when it drops.
                failure.dispose(shared, Retired::Buffer { buffer, allocation });
                return Err(failure.error);
            }
        };
        if let Err(error) = shared.wait_timeline(value) {
            // The copy may still be writing the buffer: it goes once the GPU is done.
            lock(&shared.retired)
                .retire_after_submission(Retired::Buffer { buffer, allocation }, value);
            return Err(error);
        }
        let pixels = allocation
            .mapped_slice()
            .and_then(|bytes| bytes.get(..usize::try_from(size).ok()?))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| invalid("readback memory is not mapped"));
        // SAFETY: the copy has completed (its timeline value was reached), so the buffer is
        // unused. Destroyed exactly once, then its memory freed.
        unsafe { shared.raw().destroy_buffer(buffer, None) };
        shared.free(allocation);
        shared.collect_garbage_logged();
        pixels
    }

    /// Records and submits the readback copy. Returns its timeline value.
    ///
    /// # Errors
    ///
    /// A [`SubmitFailure`], which says whether the copy may still be pending.
    fn copy_texture_to_buffer(
        &self,
        source: ReadbackSource,
        buffer: vk::Buffer,
        allocation: &Allocation,
        registration: &mut Registration,
    ) -> Result<u64, SubmitFailure> {
        let ReadbackSource {
            image,
            extent,
            old_layout,
            resting,
        } = source;
        // SAFETY: the allocation satisfies the buffer's requirements and is bound exactly once.
        unsafe {
            self.raw()
                .bind_buffer_memory(buffer, allocation.memory(), allocation.offset())
        }
        .map_err(|e| SubmitFailure::not_submitted(from_vk("vkBindBufferMemory", e)))?;

        let (pool, cmd) = self
            .begin_one_time_commands("readback")
            .map_err(SubmitFailure::not_submitted)?;
        // The buffer's memory may have been written by earlier GPU work: the allocator reuses the
        // memory of freed readback buffers. Those writes completed (the CPU waited for them), but
        // this states the ordering in the command buffer instead of relying on that wait.
        let buffer_reuse_barrier = [vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .buffer(buffer)
            .size(vk::WHOLE_SIZE)];
        let image_barrier = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
            .old_layout(old_layout)
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
        // A sampled texture returns to its resting layout, ready for any later sampling.
        let restore: Vec<vk::ImageMemoryBarrier2<'_>> = resting
            .into_iter()
            .map(|layout| {
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::COPY)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
                    .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                    .dst_access_mask(vk::AccessFlags2::MEMORY_READ)
                    .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .new_layout(layout)
                    .image(image)
                    .subresource_range(color_subresource())
            })
            .collect();
        // SAFETY: `cmd` is recording and the buffer is bound to its memory. The image can't be
        // destroyed before this submission completes (it's claimed by `registration`), and
        // `old_layout` is its layout at this point in submission order (no other unsubmitted
        // encoder may use it). The barriers order earlier writes (to the image and to the
        // buffer's reused memory) before the copy, and make the copy visible to the host read
        // after the timeline wait.
        unsafe {
            self.raw().cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default()
                    .image_memory_barriers(&image_barrier)
                    .buffer_memory_barriers(&buffer_reuse_barrier),
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
                &vk::DependencyInfo::default()
                    .buffer_memory_barriers(&buffer_barrier)
                    .image_memory_barriers(&restore),
            );
        }
        self.submit_one_time_commands(pool, cmd, Some(registration))
    }

    /// Waits until the device is idle. `vkDeviceWaitIdle` requires host access to every queue of
    /// the device to be externally synchronized, so this holds the queue lock (lock order:
    /// callers may hold `surfaces`).
    pub(crate) fn wait_idle(&self) -> Result<(), RhiError> {
        let _queue = lock(&self.queue);
        // SAFETY: the device is alive, and holding the queue lock synchronizes its only queue
        // against every submit and present, as vkDeviceWaitIdle requires.
        unsafe { self.raw().device_wait_idle() }.map_err(|e| from_vk("vkDeviceWaitIdle", e))
    }

    pub(crate) fn wait_for_submission(&self, submission: SubmissionId) -> Result<(), RhiError> {
        let last = lock(&self.queue).timeline_value;
        if submission.0 > last {
            return Err(invalid(format!(
                "{submission:?} was never submitted (the last submission is {last})"
            )));
        }
        self.wait_timeline(submission.0)?;
        self.collect_garbage()
    }

    /// Blocks until the timeline reaches `value`, which must have been submitted already.
    pub(crate) fn wait_timeline(&self, value: u64) -> Result<(), RhiError> {
        if value == 0 {
            return Ok(());
        }
        let semaphores = [self.timeline];
        let values = [value];
        let info = vk::SemaphoreWaitInfo::default()
            .semaphores(&semaphores)
            .values(&values);
        // SAFETY: the timeline semaphore is alive. Every caller passes a value that a successful
        // submission will signal (`wait_for_submission` checks it), so the wait ends unless the
        // device is lost, which the result reports.
        unsafe { self.raw().wait_semaphores(&info, u64::MAX) }
            .map_err(|e| from_vk("vkWaitSemaphores", e))
    }

    /// Creates a transient command pool with one command buffer in the recording state.
    pub(crate) fn begin_one_time_commands(
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

    /// Ends and submits a one-time command buffer, then retires its pool. Returns the
    /// submission's timeline value.
    ///
    /// # Errors
    ///
    /// A [`SubmitFailure`], which says whether the work may still be pending.
    pub(crate) fn submit_one_time_commands(
        &self,
        pool: vk::CommandPool,
        cmd: vk::CommandBuffer,
        registration: Option<&mut Registration>,
    ) -> Result<u64, SubmitFailure> {
        // SAFETY: `cmd` is recording and no render pass is open: encoders close any open pass in
        // `finish_recording` before this, and readback commands never open one.
        if let Err(e) = unsafe { self.raw().end_command_buffer(cmd) } {
            // SAFETY: never submitted. Destroying the pool frees its buffer.
            unsafe { self.raw().destroy_command_pool(pool, None) };
            return Err(SubmitFailure::not_submitted(from_vk(
                "vkEndCommandBuffer",
                e,
            )));
        }
        let mut queue = lock(&self.queue);
        match self.submit_locked(&mut queue, Some(cmd), &[], &[]) {
            Ok(value) => {
                // Still under the queue lock (lock order queue → retired), so no retirement can
                // slip between this submission and its bookkeeping.
                if let Some(registration) = registration {
                    registration.submitted(value);
                }
                lock(&self.retired).retire_after_submission(Retired::CommandPool(pool), value);
                Ok(value)
            }
            Err(error) => {
                let failure = SubmitFailure::from_submit(error);
                if failure.pending {
                    // Counts as submitted (see `SubmitFailure::pending`): everything it uses
                    // waits for device teardown, which first waits for the device to be idle.
                    if let Some(registration) = registration {
                        registration.submitted(u64::MAX);
                    }
                }
                drop(queue);
                failure.dispose(self, Retired::CommandPool(pool));
                Err(failure)
            }
        }
    }

    /// Submits `cmd` (if any) on the queue, waiting on `waits`, signalling `signals` and the
    /// next timeline value. `queue` is the locked queue state. Returns the new timeline value.
    pub(crate) fn submit_locked(
        &self,
        queue: &mut QueueState,
        cmd: Option<vk::CommandBuffer>,
        waits: &[vk::SemaphoreSubmitInfo<'_>],
        signals: &[vk::SemaphoreSubmitInfo<'_>],
    ) -> Result<u64, RhiError> {
        let value = queue.timeline_value + 1;
        let command_buffers: Vec<vk::CommandBufferSubmitInfo<'_>> = cmd
            .into_iter()
            .map(|cmd| vk::CommandBufferSubmitInfo::default().command_buffer(cmd))
            .collect();
        let mut all_signals = signals.to_vec();
        all_signals.push(
            vk::SemaphoreSubmitInfo::default()
                .semaphore(self.timeline)
                .value(value)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
        );
        let submit = vk::SubmitInfo2::default()
            .wait_semaphore_infos(waits)
            .command_buffer_infos(&command_buffers)
            .signal_semaphore_infos(&all_signals);
        // SAFETY: the queue is externally synchronized: the caller holds its lock. Command
        // buffers are fully recorded. The timeline value increases monotonically. Callers
        // guarantee the binary semaphores in `waits` have a pending signal and those in
        // `signals` are unsignalled.
        unsafe {
            self.raw()
                .queue_submit2(queue.queue, &[submit], vk::Fence::null())
        }
        .map_err(|e| from_vk("vkQueueSubmit2", e))?;
        queue.timeline_value = value;
        Ok(value)
    }

    /// Queues `object` for destruction after every submission made so far, and every encoder
    /// still recording, has finished with it (see [`crate::retire`]). Can't fail: the object is
    /// queued either way, and a failed opportunistic cleanup (a lost device) is only logged.
    pub(crate) fn retire(&self, object: Retired) {
        {
            // Hold the queue lock while queueing (lock order queue → retired), so no submission
            // can complete its bookkeeping between reading the value and queueing the object.
            let queue = lock(&self.queue);
            lock(&self.retired).retire(object, queue.timeline_value);
        }
        self.collect_garbage_logged();
    }

    /// [`Self::collect_garbage`] for paths whose real work already succeeded: a failure (a lost
    /// device) is logged instead of turning that success into an error.
    pub(crate) fn collect_garbage_logged(&self) {
        if let Err(e) = self.collect_garbage() {
            oxreal_diagnostics::warn!(error = %e, "collecting destroyed objects failed");
        }
    }

    /// Destroys retired objects that nothing can use any more.
    pub(crate) fn collect_garbage(&self) -> Result<(), RhiError> {
        // SAFETY: the timeline semaphore is alive.
        let completed = unsafe { self.raw().get_semaphore_counter_value(self.timeline) }
            .map_err(|e| from_vk("vkGetSemaphoreCounterValue", e))?;
        let ready = lock(&self.retired).take_ready(completed);
        for object in ready {
            self.destroy_retired(object);
        }
        Ok(())
    }

    /// Destroys a retired object now. Nothing may use it any more: its timeline value was
    /// reached and no encoder that could reference it is still recording, or the device is
    /// idle and being torn down.
    pub(crate) fn destroy_retired(&self, object: Retired) {
        match object {
            Retired::Texture {
                image,
                view,
                allocation,
                bindless_index,
            } => {
                // SAFETY: unused, per this function's contract. Each is destroyed exactly once,
                // and the memory is freed after the image.
                unsafe {
                    self.raw().destroy_image_view(view, None);
                    self.raw().destroy_image(image, None);
                }
                self.free(allocation);
                // Nothing can sample the slot any more, so it may point elsewhere from now on.
                if let Some(index) = bindless_index {
                    lock(&self.bindless_slots).textures.release(index);
                }
            }
            Retired::Sampler { sampler, index } => {
                // SAFETY: unused, per this function's contract. Destroyed exactly once.
                unsafe { self.raw().destroy_sampler(sampler, None) };
                lock(&self.bindless_slots).samplers.release(index);
            }
            Retired::Buffer { buffer, allocation } => {
                // SAFETY: as above.
                unsafe { self.raw().destroy_buffer(buffer, None) };
                self.free(allocation);
            }
            // SAFETY: as above.
            Retired::Pipeline(pipeline) => unsafe { self.raw().destroy_pipeline(pipeline, None) },
            // SAFETY: as above. Destroying the pool frees its command buffers, which may still be
            // in the recording state (dropped encoders); that is allowed.
            Retired::CommandPool(pool) => unsafe { self.raw().destroy_command_pool(pool, None) },
            Retired::Surface(objects) => self.destroy_surface_objects(*objects),
        }
    }

    pub(crate) fn allocate(
        &self,
        name: &str,
        requirements: vk::MemoryRequirements,
        location: MemoryLocation,
        linear: bool,
    ) -> Result<Allocation, RhiError> {
        let mut allocator = lock(&self.allocator);
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
        if let Some(Err(e)) = lock(&self.allocator).as_mut().map(|a| a.free(allocation)) {
            oxreal_diagnostics::warn!(error = %e, "freeing GPU memory failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // Depth32Float is left out on purpose: `rhi_format` maps swapchain formats only.
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
        assert_eq!(
            vk_usage(TextureUsage::DEPTH_TARGET),
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
        );
    }

    #[test]
    fn depth_formats_are_depth_targets_only() {
        let desc = |format, usage| TextureDesc {
            label: "t",
            width: 1,
            height: 1,
            format,
            usage,
        };
        let depth = TextureFormat::Depth32Float;
        assert!(check_format_usage(&desc(depth, TextureUsage::DEPTH_TARGET)).is_ok());
        for usage in [
            TextureUsage::SAMPLED,
            TextureUsage::DEPTH_TARGET | TextureUsage::COPY_SOURCE,
            TextureUsage::RENDER_TARGET,
        ] {
            assert!(
                check_format_usage(&desc(depth, usage)).is_err(),
                "{usage:?}"
            );
        }
        let color = TextureFormat::Rgba8Unorm;
        assert!(check_format_usage(&desc(color, TextureUsage::DEPTH_TARGET)).is_err());
        assert!(check_format_usage(&desc(color, TextureUsage::RENDER_TARGET)).is_ok());
        assert_eq!(
            subresource_range(depth).aspect_mask,
            vk::ImageAspectFlags::DEPTH
        );
        assert_eq!(
            subresource_range(color).aspect_mask,
            vk::ImageAspectFlags::COLOR
        );
    }

    #[test]
    fn push_constant_stages_cover_graphics_and_compute() {
        assert!(PUSH_CONSTANT_STAGES.contains(vk::ShaderStageFlags::VERTEX));
        assert!(PUSH_CONSTANT_STAGES.contains(vk::ShaderStageFlags::FRAGMENT));
        assert!(PUSH_CONSTANT_STAGES.contains(vk::ShaderStageFlags::COMPUTE));
    }
}
