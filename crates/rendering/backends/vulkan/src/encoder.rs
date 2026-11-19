//! The Vulkan command encoder (ADR-0017 §3–§4).
//!
//! Every encoder holds an `Arc` to the device's [`Shared`] state, so the `VkDevice` outlives it.
//! It also holds a [`Registration`], which:
//! - makes deferred destruction wait for it (ADR-0016 §1), and
//! - claims the textures it records commands for. While it's unsubmitted, no other encoder may
//!   use them, so each texture's tracked layout follows submission order.
//!
//! Misuse is never recorded as invalid Vulkan commands. It's remembered and reported when the
//! encoder is submitted (ADR-0017 §3).

use std::any::Any;
use std::ffi::CString;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use ash::vk;
use oxreal_diagnostics::error;
use oxreal_rhi::{
    CommandEncoder, HandlePool, LoadOp, PipelineHandle, RenderPassDesc, StoreOp, SurfaceHandle,
    TextureFormat,
};

use crate::device::{Shared, lock};
use crate::present::{Texture, color_subresource};
use crate::resources::{PUSH_CONSTANT_STAGES, Retired};
use crate::retire::EncoderId;

/// An encoder's entry in the device's bookkeeping.
pub(crate) struct Registration {
    shared: Arc<Shared>,
    id: EncoderId,
    /// Textures this encoder recorded commands for, with their layout before its first use.
    touched: Vec<(u64, vk::ImageLayout)>,
    open: bool,
}

impl Registration {
    pub(crate) fn new(shared: &Arc<Shared>) -> Self {
        let id = lock(&shared.retired).open_encoder();
        Self {
            shared: Arc::clone(shared),
            id,
            touched: Vec::new(),
            open: true,
        }
    }

    pub(crate) fn id(&self) -> EncoderId {
        self.id
    }

    /// Claims `texture` (raw handle `raw`) for this encoder. Call with the texture table
    /// locked, before recording commands that use it. Fails if another unsubmitted encoder uses
    /// it, or if it's a backbuffer that isn't this frame's.
    pub(crate) fn claim(&mut self, raw: u64, texture: &mut Texture) -> Result<(), String> {
        if texture.allocation.is_none() && texture.frame_owner != Some(self.id) {
            return Err(format!(
                "{} is a backbuffer that isn't acquired by this frame",
                handle_name(raw)
            ));
        }
        match texture.user {
            Some(user) if user == self.id => Ok(()),
            Some(_) => Err(format!(
                "{} is used by another encoder that hasn't been submitted yet",
                handle_name(raw)
            )),
            None => {
                texture.user = Some(self.id);
                self.touched.push((raw, texture.layout));
                Ok(())
            }
        }
    }

    /// Records that this encoder's commands were submitted with timeline value `value`. Call
    /// with the queue lock held, right after the submission (lock order queue → retired →
    /// textures).
    pub(crate) fn submitted(&mut self, value: u64) {
        if !self.open {
            return;
        }
        self.open = false;
        lock(&self.shared.retired).encoder_submitted(self.id, value);
        let mut textures = lock(&self.shared.textures);
        for (raw, _) in self.touched.drain(..) {
            if let Some(texture) = textures.get_mut(raw)
                && texture.user == Some(self.id)
            {
                texture.user = None;
            }
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if !self.open {
            return;
        }
        // Never submitted: none of its commands run. Objects it blocked may go, and textures it
        // touched are back in the layout they had before.
        lock(&self.shared.retired).encoder_abandoned(self.id);
        let mut textures = lock(&self.shared.textures);
        for (raw, layout) in self.touched.drain(..) {
            if let Some(texture) = textures.get_mut(raw)
                && texture.user == Some(self.id)
            {
                texture.user = None;
                texture.layout = layout;
            }
        }
    }
}

fn handle_name(raw: u64) -> String {
    format!("{:?}", oxreal_rhi::TextureHandle::from_raw(raw))
}

/// Whether a frame was ended, is still open, or was dropped. Shared by a frame encoder and its
/// surface, so `begin_frame` can tell a dropped frame (recover) from an open one (misuse).
#[derive(Debug)]
pub(crate) struct FrameToken(AtomicU8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameState {
    Open,
    Ended,
    Abandoned,
}

impl FrameToken {
    const OPEN: u8 = 0;
    const ENDED: u8 = 1;
    const ABANDONED: u8 = 2;

    pub(crate) fn new() -> Self {
        Self(AtomicU8::new(Self::OPEN))
    }

    pub(crate) fn state(&self) -> FrameState {
        match self.0.load(Ordering::Acquire) {
            Self::OPEN => FrameState::Open,
            Self::ENDED => FrameState::Ended,
            _ => FrameState::Abandoned,
        }
    }

    pub(crate) fn end(&self) {
        self.0.store(Self::ENDED, Ordering::Release);
    }

    /// Marks an open frame as dropped. The encoder no longer touches its command buffer, which
    /// `Release` publishes to the thread that recovers the frame.
    fn abandon(&self) {
        let _ = self.0.compare_exchange(
            Self::OPEN,
            Self::ABANDONED,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }
}

/// Where the encoder's commands go when recording ends.
#[derive(Debug)]
enum Kind {
    /// Part of a frame, submitted by `end_frame`.
    Frame(FrameParts),
    /// Created by `create_encoder`, submitted by `submit`. Owns its command pool until then.
    Standalone { pool: Option<vk::CommandPool> },
}

/// The frame data `end_frame` needs, taken from the encoder itself (never from the `Frame`'s
/// public fields).
#[derive(Debug, Clone)]
pub(crate) struct FrameParts {
    pub(crate) surface: SurfaceHandle,
    pub(crate) image_index: u32,
    pub(crate) slot_index: usize,
    pub(crate) token: Arc<FrameToken>,
}

/// An open render pass.
#[derive(Debug)]
struct Pass {
    label: String,
    /// False when the pass had an invalid attachment: nothing was begun and its commands are
    /// skipped (the problem was reported once).
    rendering: bool,
    labeled: bool,
    extent: vk::Extent2D,
    formats: Vec<TextureFormat>,
    /// Push-constant bytes of the pipeline bound in this pass.
    pipeline_push_bytes: Option<u32>,
}

/// Records commands into a primary command buffer.
pub(crate) struct VulkanEncoder {
    shared: Arc<Shared>,
    registration: Registration,
    cmd: vk::CommandBuffer,
    kind: Kind,
    pass: Option<Pass>,
    /// Push-constant bytes written so far (from offset 0). Push constants are command-buffer
    /// state, and all pipelines share one layout, so they persist across passes and binds.
    push_written: u32,
    /// Misuse detected while recording, reported when the encoder is submitted (ADR-0017 §3).
    misuse: Vec<String>,
}

impl fmt::Debug for VulkanEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VulkanEncoder")
            .field("id", &self.registration.id)
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
    /// An encoder for a frame, recording into the frame slot's `cmd`. `registration` is the one
    /// whose id owns the frame's backbuffer.
    pub(crate) fn frame(
        shared: Arc<Shared>,
        registration: Registration,
        cmd: vk::CommandBuffer,
        parts: FrameParts,
    ) -> Self {
        Self {
            shared,
            registration,
            cmd,
            kind: Kind::Frame(parts),
            pass: None,
            push_written: 0,
            misuse: Vec::new(),
        }
    }

    /// A standalone encoder that owns `pool`, whose command buffer `cmd` is recording.
    pub(crate) fn standalone(
        shared: Arc<Shared>,
        cmd: vk::CommandBuffer,
        pool: vk::CommandPool,
    ) -> Self {
        let registration = Registration::new(&shared);
        Self {
            shared,
            registration,
            cmd,
            kind: Kind::Standalone { pool: Some(pool) },
            pass: None,
            push_written: 0,
            misuse: Vec::new(),
        }
    }

    pub(crate) fn command_buffer(&self) -> vk::CommandBuffer {
        self.cmd
    }

    /// Whether this encoder was created by the device whose state is `shared`. Submitting an
    /// encoder to another device would use its Vulkan objects on the wrong device.
    pub(crate) fn belongs_to(&self, shared: &Shared) -> bool {
        std::ptr::eq(Arc::as_ptr(&self.shared), shared)
    }

    pub(crate) fn registration(&mut self) -> &mut Registration {
        &mut self.registration
    }

    /// The frame data of a frame encoder.
    pub(crate) fn frame_parts(&self) -> Option<FrameParts> {
        match &self.kind {
            Kind::Frame(parts) => Some(parts.clone()),
            Kind::Standalone { .. } => None,
        }
    }

    /// For `submit`: takes the command pool and buffer. `None` for frame encoders.
    pub(crate) fn take_standalone(&mut self) -> Option<(vk::CommandPool, vk::CommandBuffer)> {
        match &mut self.kind {
            Kind::Standalone { pool } => pool.take().map(|pool| (pool, self.cmd)),
            Kind::Frame(_) => None,
        }
    }

    /// Ends recording: closes a pass left open (a leaked `RenderPass` guard, which safe code
    /// can do with `mem::forget`) and reports it.
    pub(crate) fn finish_recording(&mut self) {
        if let Some(pass) = &self.pass {
            self.misuse.push(format!(
                "render pass {:?} was never ended (was its RenderPass guard leaked?); it was closed at submission",
                pass.label
            ));
            self.end_render_pass_raw();
        }
    }

    pub(crate) fn take_misuse(&mut self) -> Vec<String> {
        std::mem::take(&mut self.misuse)
    }

    /// Records the backbuffer's transition to PRESENT_SRC at the end of the frame. Call after
    /// [`Self::finish_recording`].
    pub(crate) fn transition_for_present(&mut self, backbuffer: u64) {
        let mut textures = lock(&self.shared.textures);
        let Some(texture) = textures.get_mut(backbuffer) else {
            self.misuse
                .push("the backbuffer vanished before presentation".to_owned());
            return;
        };
        if let Err(problem) = self.registration.claim(backbuffer, texture) {
            self.misuse.push(problem);
            return;
        }
        let barrier = vk::ImageMemoryBarrier2::default()
            // Chains with the acquire semaphore wait (COLOR_ATTACHMENT_OUTPUT) when the frame
            // never rendered to the image, and covers rendering when it did.
            .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
            // The render-finished semaphore is signalled at ALL_COMMANDS, so a destination of
            // ALL_COMMANDS orders the layout transition before that signal, and so before the
            // present that waits on it. (A destination of NONE leaves that ordering to how a
            // reader interprets the semaphore's scope; older validation layers report it.)
            // No access is needed: presentation makes the image visible itself.
            .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .dst_access_mask(vk::AccessFlags2::NONE)
            .old_layout(texture.layout)
            .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .image(texture.image)
            .subresource_range(color_subresource());
        let barriers = [barrier];
        let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
        // SAFETY: `cmd` is recording and no render pass is open (`finish_recording` ran first).
        // The image is this frame's acquired backbuffer (checked by `claim`). It stays alive
        // while the frame is open, because the swapchain is only recreated or destroyed with no
        // frame open.
        unsafe {
            self.shared
                .raw()
                .cmd_pipeline_barrier2(self.cmd, &dependency)
        };
        texture.layout = vk::ImageLayout::PRESENT_SRC_KHR;
    }

    fn begin_label(&self, label: &str) -> bool {
        let (Some(debug), Ok(name)) = (&self.shared.debug, CString::new(label.replace('\0', "")))
        else {
            return false;
        };
        let info = vk::DebugUtilsLabelEXT::default().label_name(&name);
        // SAFETY: `cmd` is recording: encoders record until submitted or dropped, and `Shared`
        // (the device) outlives them. The label string is alive across the call.
        unsafe { debug.cmd_begin_debug_utils_label(self.cmd, &info) };
        true
    }

    /// Checks every attachment of a pass before anything is recorded or claimed.
    fn check_attachments(
        &self,
        desc: &RenderPassDesc<'_>,
        textures: &HandlePool<Texture>,
    ) -> Vec<String> {
        let mut problems = Vec::new();
        let max = self.shared.limits.max_color_attachments as usize;
        if desc.color_attachments.is_empty() {
            problems.push(format!("render pass {:?} has no attachments", desc.label));
        }
        if desc.color_attachments.len() > max {
            problems.push(format!(
                "render pass {:?} has {} attachments; this device allows {max}",
                desc.label,
                desc.color_attachments.len()
            ));
        }
        for (i, attachment) in desc.color_attachments.iter().enumerate() {
            let raw = attachment.texture.to_raw();
            if desc.color_attachments[..i]
                .iter()
                .any(|a| a.texture.to_raw() == raw)
            {
                problems.push(format!(
                    "render pass {:?} uses {:?} twice",
                    desc.label, attachment.texture
                ));
                continue;
            }
            let Some(texture) = textures.get(raw) else {
                problems.push(format!(
                    "render pass {:?} uses {:?}, which does not exist",
                    desc.label, attachment.texture
                ));
                continue;
            };
            if !texture
                .usage
                .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            {
                problems.push(format!(
                    "render pass {:?} uses {:?}, which was not created with TextureUsage::RENDER_TARGET",
                    desc.label, attachment.texture
                ));
            }
            if texture.allocation.is_none() && texture.frame_owner != Some(self.registration.id) {
                problems.push(format!(
                    "render pass {:?} uses backbuffer {:?}, which isn't acquired by this frame",
                    desc.label, attachment.texture
                ));
            }
            if texture
                .user
                .is_some_and(|user| user != self.registration.id)
            {
                problems.push(format!(
                    "render pass {:?} uses {:?}, which another unsubmitted encoder uses",
                    desc.label, attachment.texture
                ));
            }
        }
        problems
    }

    /// The open pass, if it's rendering. Records `what` as misuse when no pass is open; a
    /// non-rendering pass already reported its problem.
    fn rendering_pass(&mut self, what: &str) -> Option<&mut Pass> {
        if self.pass.is_none() {
            self.misuse.push(format!("{what} outside a render pass"));
        }
        self.pass.as_mut().filter(|pass| pass.rendering)
    }
}

impl Drop for VulkanEncoder {
    fn drop(&mut self) {
        match &mut self.kind {
            // Never submitted: the pool (and its command buffer, recording or not) goes at the
            // next cleanup. This only queues it, so it's sound whenever the encoder drops.
            Kind::Standalone { pool } => {
                if let Some(pool) = pool.take() {
                    lock(&self.shared.retired)
                        .retire_after_submission(Retired::CommandPool(pool), 0);
                }
            }
            Kind::Frame(parts) => {
                if parts.token.state() == FrameState::Open {
                    // State first, then the log: a panicking log subscriber must not leave the
                    // frame marked open forever.
                    parts.token.abandon();
                    error!(
                        surface = ?parts.surface,
                        "a frame was dropped without Device::end_frame; the next begin_frame recreates the swapchain"
                    );
                }
            }
        }
        // `registration` drops after this; an unsubmitted encoder is abandoned there.
    }
}

impl CommandEncoder for VulkanEncoder {
    fn begin_render_pass_raw(&mut self, desc: &RenderPassDesc<'_>) {
        if self.pass.is_some() {
            self.misuse.push(format!(
                "render pass {:?} begun while another pass is open",
                desc.label
            ));
            return;
        }
        let labeled = self.begin_label(desc.label);
        let mut pass = Pass {
            label: desc.label.to_owned(),
            rendering: false,
            labeled,
            extent: vk::Extent2D::default(),
            formats: Vec::with_capacity(desc.color_attachments.len()),
            pipeline_push_bytes: None,
        };

        let shared = Arc::clone(&self.shared);
        let mut textures = lock(&shared.textures);
        let problems = self.check_attachments(desc, &textures);
        if !problems.is_empty() {
            drop(textures);
            self.misuse.extend(problems);
            self.pass = Some(pass);
            return;
        }

        // Claim every attachment before changing any layout or recording any barrier, so a
        // failure can't leave a layout updated without its barrier. `check_attachments` checked
        // the same conditions under this lock, so this only fails if they ever diverge.
        let mut claim_failed = false;
        for attachment in desc.color_attachments {
            let raw = attachment.texture.to_raw();
            let claimed = match textures.get_mut(raw) {
                Some(texture) => self.registration.claim(raw, texture),
                None => Err(format!("{:?} vanished", attachment.texture)),
            };
            if let Err(problem) = claimed {
                self.misuse.push(problem);
                claim_failed = true;
            }
        }
        if claim_failed {
            drop(textures);
            self.pass = Some(pass);
            return;
        }

        let mut barriers = Vec::with_capacity(desc.color_attachments.len());
        let mut attachments = Vec::with_capacity(desc.color_attachments.len());
        let mut area: Option<vk::Extent2D> = None;
        for attachment in desc.color_attachments {
            // Claimed just above, under the same lock.
            let Some(texture) = textures.get_mut(attachment.texture.to_raw()) else {
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
            pass.formats.push(texture.format);

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

        let complete = attachments.len() == desc.color_attachments.len();
        let Some(area) = area.filter(|_| complete) else {
            self.pass = Some(pass);
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
        // SAFETY: `cmd` is recording and no pass is open. Every attachment is a live texture of
        // this device with COLOR_ATTACHMENT usage, used once. It outlives this encoder's
        // submission because destroying it is deferred past every encoder open at that time (the
        // retire queue), and its tracked layout is current because this encoder claimed it.
        // There are at most maxColorAttachments of them. dynamicRendering and synchronization2
        // are enabled (required features). `barriers`, `attachments` and the infos are alive
        // across the calls.
        unsafe {
            shared.raw().cmd_pipeline_barrier2(self.cmd, &dependency);
            shared.raw().cmd_begin_rendering(self.cmd, &rendering);
        }
        pass.rendering = true;
        pass.extent = area;
        self.pass = Some(pass);
    }

    fn end_render_pass_raw(&mut self) {
        let Some(pass) = self.pass.take() else {
            self.misuse
                .push("render pass ended while none was open".to_owned());
            return;
        };
        if pass.rendering {
            // SAFETY: `vkCmdBeginRendering` was recorded for this pass on `cmd`.
            unsafe { self.shared.raw().cmd_end_rendering(self.cmd) };
        }
        if pass.labeled
            && let Some(debug) = &self.shared.debug
        {
            // SAFETY: a label was begun for this pass on this command buffer.
            unsafe { debug.cmd_end_debug_utils_label(self.cmd) };
        }
    }

    fn set_pipeline_raw(&mut self, pipeline: PipelineHandle) {
        let shared = Arc::clone(&self.shared);
        let cmd = self.cmd;
        let found = lock(&shared.pipelines)
            .get(pipeline.to_raw())
            .map(|p| (p.pipeline, p.push_constant_bytes, p.color_formats.clone()));
        let mut problem = None;
        if let Some(pass) = self.rendering_pass("set_pipeline") {
            pass.pipeline_push_bytes = None;
            match found {
                None => problem = Some(format!("set_pipeline: {pipeline:?} does not exist")),
                Some((_, _, formats)) if formats != pass.formats => {
                    problem = Some(format!(
                        "set_pipeline: {pipeline:?} renders to {formats:?}, but render pass {:?} has {:?}",
                        pass.label, pass.formats
                    ));
                }
                Some((raw, push_bytes, _)) => {
                    let viewports = [y_up_viewport(pass.extent)];
                    let scissors = [vk::Rect2D {
                        offset: vk::Offset2D { x: 0, y: 0 },
                        extent: pass.extent,
                    }];
                    // SAFETY: `cmd` is recording inside a dynamic rendering pass whose
                    // attachment formats match the pipeline's. The pipeline stays alive until
                    // this encoder is done, because destroying it is deferred past this encoder
                    // (ADR-0016 §1). Viewport and scissor are its dynamic states.
                    unsafe {
                        shared
                            .raw()
                            .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, raw);
                        shared.raw().cmd_set_viewport(cmd, 0, &viewports);
                        shared.raw().cmd_set_scissor(cmd, 0, &scissors);
                    }
                    pass.pipeline_push_bytes = Some(push_bytes);
                }
            }
        }
        self.misuse.extend(problem);
    }

    fn set_push_constants_raw(&mut self, bytes: &[u8]) {
        let Some(allowed) = self
            .rendering_pass("set_push_constants")
            .map(|pass| pass.pipeline_push_bytes)
        else {
            return;
        };
        let Some(allowed) = allowed else {
            self.misuse
                .push("set_push_constants before a successful set_pipeline".to_owned());
            return;
        };
        let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        if bytes.is_empty() || !len.is_multiple_of(4) || len > allowed {
            self.misuse.push(format!(
                "set_push_constants: {} bytes, but the pipeline declares {allowed} and the size must be a non-zero multiple of 4",
                bytes.len()
            ));
            return;
        }
        // SAFETY: `cmd` is recording. The range 0..len lies within the shared layout's
        // push-constant range (len ≤ the pipeline's size ≤ MAX_PUSH_CONSTANT_BYTES, the range's
        // size), and the stage flags are exactly that range's.
        unsafe {
            self.shared.raw().cmd_push_constants(
                self.cmd,
                self.shared.pipeline_layout,
                PUSH_CONSTANT_STAGES,
                0,
                bytes,
            );
        }
        self.push_written = self.push_written.max(len);
    }

    fn draw_raw(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    ) {
        let written = self.push_written;
        let Some(needed) = self
            .rendering_pass("draw")
            .map(|pass| pass.pipeline_push_bytes)
        else {
            return;
        };
        let Some(needed) = needed else {
            self.misuse
                .push("draw before a successful set_pipeline in this pass".to_owned());
            return;
        };
        if written < needed {
            self.misuse.push(format!(
                "draw: the pipeline reads {needed} push-constant bytes, but only {written} were written"
            ));
            return;
        }
        // SAFETY: `cmd` is recording inside a rendering pass with a compatible graphics pipeline
        // bound, its dynamic viewport and scissor set, and every push-constant byte it reads
        // written.
        unsafe {
            self.shared.raw().cmd_draw(
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

    #[test]
    fn frame_tokens_only_abandon_open_frames() {
        let token = FrameToken::new();
        assert_eq!(token.state(), FrameState::Open);
        token.abandon();
        assert_eq!(token.state(), FrameState::Abandoned);

        let ended = FrameToken::new();
        ended.end();
        ended.abandon();
        assert_eq!(
            ended.state(),
            FrameState::Ended,
            "an ended frame stays ended"
        );
    }
}
