//! The Vulkan command encoder (ADR-0017 §3–§4).

use std::any::Any;
use std::ffi::CString;
use std::fmt;
use std::sync::{Arc, Mutex};

use ash::vk;
use oxreal_rhi::{
    CommandEncoder, HandlePool, LoadOp, RenderPassDesc, StoreOp, SurfaceHandle, TextureHandle,
};

use crate::present::{Texture, color_subresource};

/// Records one frame's commands into a primary command buffer.
pub(crate) struct VulkanEncoder {
    device: ash::Device,
    cmd: vk::CommandBuffer,
    textures: Arc<Mutex<HandlePool<Texture>>>,
    debug: Option<ash::ext::debug_utils::Device>,
    surface: SurfaceHandle,
    image_index: u32,
    slot_index: usize,
    pass: PassState,
    /// Misuse detected while recording, reported by `end_frame` (ADR-0017 §3).
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
    },
}

impl fmt::Debug for VulkanEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VulkanEncoder")
            .field("surface", &self.surface)
            .field("image_index", &self.image_index)
            .field("slot_index", &self.slot_index)
            .field("pass", &self.pass)
            .field("misuse", &self.misuse)
            .finish_non_exhaustive()
    }
}

impl VulkanEncoder {
    pub(crate) fn new(
        device: ash::Device,
        cmd: vk::CommandBuffer,
        textures: Arc<Mutex<HandlePool<Texture>>>,
        debug: Option<ash::ext::debug_utils::Device>,
        surface: SurfaceHandle,
        image_index: u32,
        slot_index: usize,
    ) -> Self {
        Self {
            device,
            cmd,
            textures,
            debug,
            surface,
            image_index,
            slot_index,
            pass: PassState::Closed,
            misuse: Vec::new(),
        }
    }

    pub(crate) fn command_buffer(&self) -> vk::CommandBuffer {
        self.cmd
    }

    pub(crate) fn image_index(&self) -> u32 {
        self.image_index
    }

    pub(crate) fn slot_index(&self) -> usize {
        self.slot_index
    }

    pub(crate) fn take_misuse(&mut self) -> Vec<String> {
        std::mem::take(&mut self.misuse)
    }

    /// Records the backbuffer's transition to PRESENT_SRC at the end of the frame.
    pub(crate) fn transition_for_present(&mut self, backbuffer: TextureHandle) {
        let Ok(mut textures) = self.textures.lock() else {
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
        unsafe { self.device.cmd_pipeline_barrier2(self.cmd, &dependency) };
        texture.layout = vk::ImageLayout::PRESENT_SRC_KHR;
    }

    fn begin_label(&self, label: &str) -> bool {
        let (Some(debug), Ok(name)) = (&self.debug, CString::new(label.replace('\0', ""))) else {
            return false;
        };
        let info = vk::DebugUtilsLabelEXT::default().label_name(&name);
        // SAFETY: `cmd` is recording. The label string is alive across the call.
        unsafe { debug.cmd_begin_debug_utils_label(self.cmd, &info) };
        true
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

        let Ok(mut textures) = self.textures.lock() else {
            self.misuse.push("texture table lock poisoned".to_owned());
            self.pass = PassState::Open {
                rendering: false,
                labeled,
            };
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
            // Contents are only preserved for `Load`. Otherwise starting from UNDEFINED lets the
            // driver skip the old contents.
            let old_layout = match attachment.load {
                LoadOp::Load => texture.layout,
                LoadOp::Clear(_) | LoadOp::DontCare => vk::ImageLayout::UNDEFINED,
            };
            barriers.push(
                vk::ImageMemoryBarrier2::default()
                    // Chains with the acquire semaphore wait at COLOR_ATTACHMENT_OUTPUT.
                    .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                    .src_access_mask(vk::AccessFlags2::NONE)
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
            self.pass = PassState::Open {
                rendering: false,
                labeled,
            };
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
            self.device.cmd_pipeline_barrier2(self.cmd, &dependency);
            self.device.cmd_begin_rendering(self.cmd, &rendering);
        }
        self.pass = PassState::Open {
            rendering: true,
            labeled,
        };
    }

    fn end_render_pass_raw(&mut self) {
        let PassState::Open { rendering, labeled } = self.pass else {
            self.misuse
                .push("render pass ended while none was open".to_owned());
            return;
        };
        if rendering {
            // SAFETY: `vkCmdBeginRendering` was recorded for this pass.
            unsafe { self.device.cmd_end_rendering(self.cmd) };
        }
        if labeled && let Some(debug) = &self.debug {
            // SAFETY: a label was begun for this pass on this command buffer.
            unsafe { debug.cmd_end_debug_utils_label(self.cmd) };
        }
        self.pass = PassState::Closed;
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any + Send> {
        self
    }
}
