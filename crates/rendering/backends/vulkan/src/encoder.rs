//! The Vulkan command encoder (ADR-0017 §3–§4).

use std::any::Any;
use std::ffi::CString;
use std::fmt;
use std::sync::{Arc, Mutex};

use ash::vk;
use oxreal_diagnostics::error;
use oxreal_rhi::{
    CommandEncoder, HandlePool, LoadOp, PipelineHandle, RenderPassDesc, StoreOp, SurfaceHandle,
    TextureHandle,
};

use crate::present::{Texture, color_subresource};
use crate::resources::{PUSH_CONSTANT_STAGES, Pipeline, Retired};
use crate::retire::{EncoderId, RetireQueue};

/// What an encoder needs from its device. Cloned into every encoder.
#[derive(Clone)]
pub(crate) struct EncoderContext {
    pub(crate) device: ash::Device,
    pub(crate) textures: Arc<Mutex<HandlePool<Texture>>>,
    pub(crate) pipelines: Arc<Mutex<HandlePool<Pipeline>>>,
    pub(crate) retired: Arc<Mutex<RetireQueue<Retired>>>,
    pub(crate) debug: Option<ash::ext::debug_utils::Device>,
    pub(crate) pipeline_layout: vk::PipelineLayout,
}

/// Where the encoder's commands go when recording ends.
#[derive(Debug)]
enum Kind {
    /// Part of a frame, submitted by `end_frame`.
    Frame {
        surface: SurfaceHandle,
        image_index: u32,
        slot_index: usize,
        ended: bool,
    },
    /// Created by `create_encoder`, submitted by `submit`. Owns its command pool until then.
    Standalone { pool: Option<vk::CommandPool> },
}

/// Records commands into a primary command buffer.
pub(crate) struct VulkanEncoder {
    ctx: EncoderContext,
    cmd: vk::CommandBuffer,
    kind: Kind,
    /// Registration in the retire queue, so objects destroyed while recording outlive it.
    id: Option<EncoderId>,
    pass: PassState,
    /// Misuse detected while recording, reported when the encoder is submitted (ADR-0017 §3).
    misuse: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PassState {
    Closed,
    /// A pass is open. `rendering` is false when it had no valid attachment, so
    /// `vkCmdBeginRendering` was skipped. `labeled` tracks the debug label.
    Open {
        rendering: bool,
        labeled: bool,
        extent: vk::Extent2D,
        /// Push-constant bytes of the bound pipeline, `None` before `set_pipeline`.
        pipeline_push_bytes: Option<u32>,
    },
}

impl fmt::Debug for VulkanEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VulkanEncoder")
            .field("kind", &self.kind)
            .field("pass", &self.pass)
            .field("misuse", &self.misuse)
            .finish_non_exhaustive()
    }
}

/// The viewport for Oxreal's Y-up clip space (ADR-0011): a negative height flips Vulkan's Y-down
/// clip space, so clip-space +Y is the top of the target.
pub(crate) fn y_up_viewport(extent: vk::Extent2D) -> vk::Viewport {
    // Render target sizes are far below 2^24, where f32 would lose precision.
    let (width, height) = (extent.width as f32, extent.height as f32);
    vk::Viewport {
        x: 0.0,
        y: height,
        width,
        height: -height,
        min_depth: 0.0,
        max_depth: 1.0,
    }
}

impl VulkanEncoder {
    fn new(ctx: EncoderContext, cmd: vk::CommandBuffer, kind: Kind) -> Self {
        let id = ctx.retired.lock().ok().map(|mut q| q.open_encoder());
        Self {
            ctx,
            cmd,
            kind,
            id,
            pass: PassState::Closed,
            misuse: Vec::new(),
        }
    }

    /// An encoder for a frame of `surface`.
    pub(crate) fn frame(
        ctx: EncoderContext,
        cmd: vk::CommandBuffer,
        surface: SurfaceHandle,
        image_index: u32,
        slot_index: usize,
    ) -> Self {
        Self::new(
            ctx,
            cmd,
            Kind::Frame {
                surface,
                image_index,
                slot_index,
                ended: false,
            },
        )
    }

    /// A standalone encoder that owns `pool`, whose command buffer `cmd` is recording.
    pub(crate) fn standalone(
        ctx: EncoderContext,
        cmd: vk::CommandBuffer,
        pool: vk::CommandPool,
    ) -> Self {
        Self::new(ctx, cmd, Kind::Standalone { pool: Some(pool) })
    }

    pub(crate) fn command_buffer(&self) -> vk::CommandBuffer {
        self.cmd
    }

    /// The swapchain image index and frame slot, for frame encoders.
    pub(crate) fn frame_info(&self) -> Option<(u32, usize)> {
        match self.kind {
            Kind::Frame {
                image_index,
                slot_index,
                ..
            } => Some((image_index, slot_index)),
            Kind::Standalone { .. } => None,
        }
    }

    /// For `submit`: takes the command pool and buffer. `None` for frame encoders.
    pub(crate) fn take_standalone(&mut self) -> Option<(vk::CommandPool, vk::CommandBuffer)> {
        match &mut self.kind {
            Kind::Standalone { pool } => pool.take().map(|pool| (pool, self.cmd)),
            Kind::Frame { .. } => None,
        }
    }

    /// Records that `end_frame` received this frame encoder, so dropping it is expected.
    pub(crate) fn mark_ended(&mut self) {
        if let Kind::Frame { ended, .. } = &mut self.kind {
            *ended = true;
        }
    }

    /// Records that the encoder's commands were submitted with timeline value `value`.
    pub(crate) fn mark_submitted(&mut self, value: u64) {
        if let Kind::Frame { ended, .. } = &mut self.kind {
            *ended = true;
        }
        if let Some(id) = self.id.take()
            && let Ok(mut queue) = self.ctx.retired.lock()
        {
            queue.encoder_submitted(id, value);
        }
    }

    pub(crate) fn take_misuse(&mut self) -> Vec<String> {
        std::mem::take(&mut self.misuse)
    }

    /// Records the backbuffer's transition to PRESENT_SRC at the end of the frame.
    pub(crate) fn transition_for_present(&mut self, backbuffer: TextureHandle) {
        let Ok(mut textures) = self.ctx.textures.lock() else {
            self.misuse.push("texture table lock poisoned".to_owned());
            return;
        };
        let Some(texture) = textures.get_mut(backbuffer.to_raw()) else {
            self.misuse.push(format!(
                "backbuffer {backbuffer:?} vanished before presentation"
            ));
            return;
        };
        let barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::NONE)
            .dst_access_mask(vk::AccessFlags2::NONE)
            .old_layout(texture.layout)
            .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .image(texture.image)
            .subresource_range(color_subresource());
        let barriers = [barrier];
        let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
        // SAFETY: `cmd` is in the recording state (begun in `begin_frame`), no render pass is
        // open, and the image belongs to the live swapchain.
        unsafe { self.ctx.device.cmd_pipeline_barrier2(self.cmd, &dependency) };
        texture.layout = vk::ImageLayout::PRESENT_SRC_KHR;
    }

    fn begin_label(&self, label: &str) -> bool {
        let (Some(debug), Ok(name)) = (&self.ctx.debug, CString::new(label.replace('\0', "")))
        else {
            return false;
        };
        let info = vk::DebugUtilsLabelEXT::default().label_name(&name);
        // SAFETY: `cmd` is recording. The label string is alive across the call.
        unsafe { debug.cmd_begin_debug_utils_label(self.cmd, &info) };
        true
    }

    /// The open pass's state if it is rendering, or records `what` as misuse.
    fn rendering_pass(&mut self, what: &str) -> Option<(vk::Extent2D, Option<u32>)> {
        match self.pass {
            PassState::Open {
                rendering: true,
                extent,
                pipeline_push_bytes,
                ..
            } => Some((extent, pipeline_push_bytes)),
            PassState::Open { .. } => None, // Already reported when the pass began.
            PassState::Closed => {
                self.misuse.push(format!("{what} outside a render pass"));
                None
            }
        }
    }
}

impl Drop for VulkanEncoder {
    fn drop(&mut self) {
        let Ok(mut queue) = self.ctx.retired.lock() else {
            return;
        };
        if let Some(id) = self.id.take() {
            queue.encoder_abandoned(id);
        }
        match &mut self.kind {
            // Never submitted: the device destroys the pool (and its command buffer) at its next
            // cleanup. The encoder doesn't call Vulkan itself, so it stays sound even if it
            // outlives its device.
            Kind::Standalone { pool } => {
                if let Some(pool) = pool.take() {
                    queue.retire_after_submission(Retired::CommandPool(pool), 0);
                }
            }
            Kind::Frame {
                surface,
                ended: false,
                ..
            } => {
                error!(
                    surface = ?surface,
                    "a frame was dropped without Device::end_frame; its swapchain image stays acquired"
                );
            }
            Kind::Frame { .. } => {}
        }
    }
}

impl CommandEncoder for VulkanEncoder {
    fn begin_render_pass_raw(&mut self, desc: &RenderPassDesc<'_>) {
        if self.pass != PassState::Closed {
            self.misuse.push(format!(
                "render pass {:?} begun while another pass is open",
                desc.label
            ));
            return;
        }
        let labeled = self.begin_label(desc.label);
        let not_rendering = PassState::Open {
            rendering: false,
            labeled,
            extent: vk::Extent2D::default(),
            pipeline_push_bytes: None,
        };

        let Ok(mut textures) = self.ctx.textures.lock() else {
            self.misuse.push("texture table lock poisoned".to_owned());
            self.pass = not_rendering;
            return;
        };
        let mut barriers = Vec::with_capacity(desc.color_attachments.len());
        let mut attachments = Vec::with_capacity(desc.color_attachments.len());
        let mut area: Option<vk::Extent2D> = None;
        for attachment in desc.color_attachments {
            let Some(texture) = textures.get_mut(attachment.texture.to_raw()) else {
                self.misuse.push(format!(
                    "render pass {:?} uses {:?}, which does not exist",
                    desc.label, attachment.texture
                ));
                continue;
            };
            if !texture
                .usage
                .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            {
                self.misuse.push(format!(
                    "render pass {:?} uses {:?}, which was not created with TextureUsage::RENDER_TARGET",
                    desc.label, attachment.texture
                ));
                continue;
            }
            // Contents are only preserved for `Load`. Otherwise starting from UNDEFINED lets the
            // driver skip the old contents.
            let old_layout = match attachment.load {
                LoadOp::Load => texture.layout,
                LoadOp::Clear(_) | LoadOp::DontCare => vk::ImageLayout::UNDEFINED,
            };
            barriers.push(
                vk::ImageMemoryBarrier2::default()
                    // Waits for any earlier use of the image: a previous pass, a copy, or the
                    // acquire semaphore (which signals at COLOR_ATTACHMENT_OUTPUT, included in
                    // ALL_COMMANDS). Precise per-resource tracking comes with the frame graph.
                    .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                    .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                    .dst_access_mask(
                        vk::AccessFlags2::COLOR_ATTACHMENT_WRITE
                            | vk::AccessFlags2::COLOR_ATTACHMENT_READ,
                    )
                    .old_layout(old_layout)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .image(texture.image)
                    .subresource_range(color_subresource()),
            );
            texture.layout = vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL;
            area = Some(area.map_or(texture.extent, |a| vk::Extent2D {
                width: a.width.min(texture.extent.width),
                height: a.height.min(texture.extent.height),
            }));

            let (load_op, clear) = match attachment.load {
                LoadOp::Clear(color) => (vk::AttachmentLoadOp::CLEAR, color.to_array()),
                LoadOp::Load => (vk::AttachmentLoadOp::LOAD, [0.0; 4]),
                LoadOp::DontCare => (vk::AttachmentLoadOp::DONT_CARE, [0.0; 4]),
            };
            let store_op = match attachment.store {
                StoreOp::Store => vk::AttachmentStoreOp::STORE,
                StoreOp::DontCare => vk::AttachmentStoreOp::DONT_CARE,
            };
            attachments.push(
                vk::RenderingAttachmentInfo::default()
                    .image_view(texture.view)
                    .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .load_op(load_op)
                    .store_op(store_op)
                    .clear_value(vk::ClearValue {
                        color: vk::ClearColorValue { float32: clear },
                    }),
            );
        }
        drop(textures);

        let Some(area) = area else {
            self.pass = not_rendering;
            return;
        };
        let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
        let rendering = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: area,
            })
            .layer_count(1)
            .color_attachments(&attachments);
        // SAFETY: `cmd` is recording and no pass is open. The barriers and attachments refer to
        // live images and views of this device, and dynamicRendering and synchronization2 are
        // enabled (required features). `barriers`, `attachments` and their infos are alive
        // across the calls.
        unsafe {
            self.ctx.device.cmd_pipeline_barrier2(self.cmd, &dependency);
            self.ctx.device.cmd_begin_rendering(self.cmd, &rendering);
        }
        self.pass = PassState::Open {
            rendering: true,
            labeled,
            extent: area,
            pipeline_push_bytes: None,
        };
    }

    fn end_render_pass_raw(&mut self) {
        let PassState::Open {
            rendering, labeled, ..
        } = self.pass
        else {
            self.misuse
                .push("render pass ended while none was open".to_owned());
            return;
        };
        if rendering {
            // SAFETY: `vkCmdBeginRendering` was recorded for this pass.
            unsafe { self.ctx.device.cmd_end_rendering(self.cmd) };
        }
        if labeled && let Some(debug) = &self.ctx.debug {
            // SAFETY: a label was begun for this pass on this command buffer.
            unsafe { debug.cmd_end_debug_utils_label(self.cmd) };
        }
        self.pass = PassState::Closed;
    }

    fn set_pipeline_raw(&mut self, pipeline: PipelineHandle) {
        let Some((extent, _)) = self.rendering_pass("set_pipeline") else {
            return;
        };
        let found = match self.ctx.pipelines.lock() {
            Ok(pipelines) => pipelines
                .get(pipeline.to_raw())
                .map(|p| (p.pipeline, p.push_constant_bytes)),
            Err(_) => {
                self.misuse.push("pipeline table lock poisoned".to_owned());
                return;
            }
        };
        let Some((raw, push_bytes)) = found else {
            self.misuse
                .push(format!("set_pipeline: {pipeline:?} does not exist"));
            return;
        };
        let viewports = [y_up_viewport(extent)];
        let scissors = [vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent,
        }];
        // SAFETY: `cmd` is recording inside a dynamic rendering pass. The pipeline is alive (it
        // is in the table, and destruction is deferred past this encoder's submission), and
        // viewport and scissor are its dynamic states.
        unsafe {
            self.ctx
                .device
                .cmd_bind_pipeline(self.cmd, vk::PipelineBindPoint::GRAPHICS, raw);
            self.ctx.device.cmd_set_viewport(self.cmd, 0, &viewports);
            self.ctx.device.cmd_set_scissor(self.cmd, 0, &scissors);
        }
        if let PassState::Open {
            pipeline_push_bytes,
            ..
        } = &mut self.pass
        {
            *pipeline_push_bytes = Some(push_bytes);
        }
    }

    fn set_push_constants_raw(&mut self, bytes: &[u8]) {
        let Some((_, bound)) = self.rendering_pass("set_push_constants") else {
            return;
        };
        let Some(allowed) = bound else {
            self.misuse
                .push("set_push_constants before set_pipeline".to_owned());
            return;
        };
        let fits = u32::try_from(bytes.len()).is_ok_and(|len| len <= allowed);
        if bytes.is_empty() || !bytes.len().is_multiple_of(4) || !fits {
            self.misuse.push(format!(
                "set_push_constants: {} bytes, but the pipeline declares {allowed} and the size must be a non-zero multiple of 4",
                bytes.len()
            ));
            return;
        }
        // SAFETY: `cmd` is recording. The range lies within the shared layout's push-constant
        // range (checked above against the pipeline's size, itself at most
        // MAX_PUSH_CONSTANT_BYTES), and the stage flags match that range exactly.
        unsafe {
            self.ctx.device.cmd_push_constants(
                self.cmd,
                self.ctx.pipeline_layout,
                PUSH_CONSTANT_STAGES,
                0,
                bytes,
            );
        }
    }

    fn draw_raw(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    ) {
        let Some((_, bound)) = self.rendering_pass("draw") else {
            return;
        };
        if bound.is_none() {
            self.misuse.push("draw before set_pipeline".to_owned());
            return;
        }
        // SAFETY: `cmd` is recording inside a rendering pass with a graphics pipeline bound and
        // its dynamic viewport and scissor set.
        unsafe {
            self.ctx.device.cmd_draw(
                self.cmd,
                vertex_count,
                instance_count,
                first_vertex,
                first_instance,
            );
        }
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any + Send> {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_flips_y_for_y_up_clip_space() {
        let v = y_up_viewport(vk::Extent2D {
            width: 640,
            height: 480,
        });
        assert_eq!((v.x, v.y, v.width, v.height), (0.0, 480.0, 640.0, -480.0));
        assert_eq!((v.min_depth, v.max_depth), (0.0, 1.0));
    }
}
