//! Command recording and frames (ADR-0017 §3–§5).

use std::any::Any;
use std::fmt;

use oxreal_math::LinearRgba;

use crate::{PipelineHandle, SurfaceHandle, TextureFormat, TextureHandle};

/// What happens to an attachment's contents when a render pass begins.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadOp {
    /// Fill with this **linear** color (ADR-0011 §5).
    Clear(LinearRgba),
    /// Keep the existing contents.
    Load,
    /// The contents are undefined. Use it when every pixel will be overwritten.
    DontCare,
}

/// What happens to an attachment's contents when a render pass ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOp {
    /// Keep the rendered result.
    Store,
    /// The result may be discarded.
    DontCare,
}

/// What happens to a depth attachment's contents when a render pass begins.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DepthLoadOp {
    /// Fill with this depth, in [0, 1]. Scene depth is reversed, so clear it to
    /// [`oxreal_math::projection::REVERSED_DEPTH_CLEAR`] (ADR-0011 §4).
    Clear(f32),
    /// Keep the existing contents, for example from a depth prepass.
    Load,
    /// The contents are undefined.
    DontCare,
}

/// The depth target of a render pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepthAttachment {
    /// A texture with a depth format and [`crate::TextureUsage::DEPTH_TARGET`].
    pub texture: TextureHandle,
    /// How to start.
    pub load: DepthLoadOp,
    /// How to finish. [`StoreOp::DontCare`] suits depth only this pass reads.
    pub store: StoreOp,
}

/// A color target of a render pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorAttachment {
    /// The texture to render into.
    pub texture: TextureHandle,
    /// How to start.
    pub load: LoadOp,
    /// How to finish.
    pub store: StoreOp,
}

/// Describes a render pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderPassDesc<'a> {
    /// A name shown in GPU debuggers and profilers.
    pub label: &'a str,
    /// Color targets, in shader output order. May be empty when there is a depth attachment.
    ///
    /// The pass renders to the area every attachment covers: the smallest width and height.
    pub color_attachments: &'a [ColorAttachment],
    /// The depth target, if any. Pipelines used in the pass must declare the same depth format
    /// ([`crate::RenderPipelineDesc::depth`]).
    pub depth_attachment: Option<DepthAttachment>,
}

/// Records GPU commands. Backends implement it. Renderer code records through
/// `&mut dyn CommandEncoder` (ADR-0017 §1).
///
/// Recording is infallible (ADR-0017 §3): misuse such as an invalid handle is remembered and
/// reported as an error when the frame ends.
///
/// Rules the backends enforce, reporting violations as misuse instead of recording invalid
/// commands:
/// - A texture used by one encoder can't be used by another until the first is submitted (or
///   dropped). This keeps resource state in submission order.
/// - A pass whose attachments aren't all valid records nothing. A pipeline must be set in every
///   pass, and must match the pass's attachment formats. Every push-constant byte the pipeline
///   declares must be written before a draw.
/// - A pass left open (for example a leaked [`RenderPass`] guard) is closed at submission and
///   reported.
///
/// The `*_raw` methods exist for backends to implement. Renderer code uses
/// [`begin_render_pass`](trait.CommandEncoder.html#method.begin_render_pass) on
/// `dyn CommandEncoder`, which ends the pass when its guard drops.
pub trait CommandEncoder: Send + fmt::Debug {
    /// Backend hook: begin a render pass. Called only by [`RenderPass`].
    fn begin_render_pass_raw(&mut self, desc: &RenderPassDesc<'_>);

    /// Backend hook: end the current render pass. Called only by [`RenderPass`].
    fn end_render_pass_raw(&mut self);

    /// Backend hook: bind a render pipeline. Called only by [`RenderPass::set_pipeline`].
    fn set_pipeline_raw(&mut self, pipeline: PipelineHandle);

    /// Backend hook: write push constants at offset 0. Called only by
    /// [`RenderPass::set_push_constants`].
    fn set_push_constants_raw(&mut self, bytes: &[u8]);

    /// Backend hook: issue a non-indexed draw. Called only by [`RenderPass::draw`].
    fn draw_raw(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    );

    /// Backend hook: converts the encoder back to its concrete type at the end of the frame.
    fn into_any(self: Box<Self>) -> Box<dyn Any + Send>;
}

impl<'e> dyn CommandEncoder + 'e {
    /// Begins a render pass. It ends when the returned guard is dropped, so a pass can't be left
    /// open.
    pub fn begin_render_pass<'p>(&'p mut self, desc: &RenderPassDesc<'_>) -> RenderPass<'p, 'e> {
        self.begin_render_pass_raw(desc);
        RenderPass { encoder: self }
    }
}

/// An open render pass. It ends on drop.
///
/// Draw commands will live here, so they can't be issued outside a pass (ADR-0017 §4).
#[derive(Debug)]
pub struct RenderPass<'p, 'e> {
    encoder: &'p mut (dyn CommandEncoder + 'e),
}

impl RenderPass<'_, '_> {
    /// Binds the pipeline used by the following draws. It must be created for this pass's
    /// attachment formats, and must be set again in every pass.
    pub fn set_pipeline(&mut self, pipeline: PipelineHandle) {
        self.encoder.set_pipeline_raw(pipeline);
    }

    /// Writes the bound pipeline's push constants, starting at offset 0. `bytes` must fit the
    /// pipeline's declared size (at most [`crate::MAX_PUSH_CONSTANT_BYTES`]) and be a multiple
    /// of 4. Misuse is reported when the frame ends.
    pub fn set_push_constants(&mut self, bytes: &[u8]) {
        self.encoder.set_push_constants_raw(bytes);
    }

    /// Draws `vertex_count` vertices, `instance_count` times, with the bound pipeline. Every
    /// push-constant byte the pipeline declares must have been written by this encoder.
    pub fn draw(&mut self, vertex_count: u32, instance_count: u32) {
        self.encoder.draw_raw(vertex_count, instance_count, 0, 0);
    }
}

impl Drop for RenderPass<'_, '_> {
    fn drop(&mut self) {
        self.encoder.end_render_pass_raw();
    }
}

/// One frame of a surface, returned by [`crate::Device::begin_frame`].
///
/// Record into [`Frame::encoder`] and render to [`Frame::backbuffer`], then pass the frame to
/// [`crate::Device::end_frame`]. **Every frame must be ended.** A dropped frame is reported as an
/// error, and the next `begin_frame` on its surface recovers by recreating the swapchain.
#[derive(Debug)]
#[must_use = "a frame must be passed to Device::end_frame"]
pub struct Frame {
    surface: SurfaceHandle,
    backbuffer: TextureHandle,
    backbuffer_format: TextureFormat,
    backbuffer_size: (u32, u32),
    encoder: Box<dyn CommandEncoder>,
}

impl Frame {
    /// For backends: assembles a frame.
    pub fn new(
        surface: SurfaceHandle,
        backbuffer: TextureHandle,
        backbuffer_format: TextureFormat,
        backbuffer_size: (u32, u32),
        encoder: Box<dyn CommandEncoder>,
    ) -> Self {
        Self {
            surface,
            backbuffer,
            backbuffer_format,
            backbuffer_size,
            encoder,
        }
    }

    /// The backbuffer's format. Pipelines that render to it must be created for this format,
    /// which can change when the swapchain is recreated.
    #[must_use]
    pub fn backbuffer_format(&self) -> TextureFormat {
        self.backbuffer_format
    }

    /// The backbuffer's width and height in pixels. It can differ from the window's size (the
    /// platform may fix the swapchain size) and change between frames, so size other render
    /// targets, such as a depth buffer, and the camera's aspect ratio from it.
    #[must_use]
    pub fn backbuffer_size(&self) -> (u32, u32) {
        self.backbuffer_size
    }

    /// The surface this frame presents to.
    #[must_use]
    pub fn surface(&self) -> SurfaceHandle {
        self.surface
    }

    /// The texture that is presented at the end of the frame.
    #[must_use]
    pub fn backbuffer(&self) -> TextureHandle {
        self.backbuffer
    }

    /// The frame's command encoder.
    pub fn encoder(&mut self) -> &mut (dyn CommandEncoder + 'static) {
        self.encoder.as_mut()
    }

    /// For backends: takes the frame apart at the end of the frame.
    #[must_use]
    pub fn into_parts(self) -> (SurfaceHandle, TextureHandle, Box<dyn CommandEncoder>) {
        (self.surface, self.backbuffer, self.encoder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records which hooks were called, to test the pass guard without a GPU.
    #[derive(Debug, Default)]
    struct Recorder {
        calls: Vec<&'static str>,
    }

    impl CommandEncoder for Recorder {
        fn begin_render_pass_raw(&mut self, _desc: &RenderPassDesc<'_>) {
            self.calls.push("begin");
        }
        fn end_render_pass_raw(&mut self) {
            self.calls.push("end");
        }
        fn set_pipeline_raw(&mut self, _pipeline: PipelineHandle) {
            self.calls.push("pipeline");
        }
        fn set_push_constants_raw(&mut self, _bytes: &[u8]) {
            self.calls.push("push");
        }
        fn draw_raw(&mut self, _: u32, _: u32, _: u32, _: u32) {
            self.calls.push("draw");
        }
        fn into_any(self: Box<Self>) -> Box<dyn Any + Send> {
            self
        }
    }

    #[test]
    fn draw_commands_go_through_the_pass_in_order() {
        let mut encoder: Box<dyn CommandEncoder> = Box::new(Recorder::default());
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDesc {
                label: "draws",
                color_attachments: &[],
                depth_attachment: None,
            });
            pass.set_pipeline(PipelineHandle::from_raw(1));
            pass.set_push_constants(&[0; 4]);
            pass.draw(3, 1);
        }
        let recorder = encoder.into_any().downcast::<Recorder>().unwrap();
        assert_eq!(
            recorder.calls,
            vec!["begin", "pipeline", "push", "draw", "end"]
        );
    }

    #[test]
    fn render_pass_guard_always_ends_the_pass() {
        let mut encoder: Box<dyn CommandEncoder> = Box::new(Recorder::default());
        let desc = RenderPassDesc {
            label: "test",
            color_attachments: &[],
            depth_attachment: None,
        };
        {
            let _pass = encoder.begin_render_pass(&desc);
        }
        {
            let _pass = encoder.begin_render_pass(&desc);
        }
        let recorder = encoder
            .into_any()
            .downcast::<Recorder>()
            .expect("the concrete type round-trips");
        assert_eq!(recorder.calls, vec!["begin", "end", "begin", "end"]);
    }
}
