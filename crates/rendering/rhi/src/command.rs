//! Command recording and frames (ADR-0017 §3–§5).

use std::any::Any;
use std::fmt;

use oxreal_math::LinearRgba;

use crate::{SurfaceHandle, TextureHandle};

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
    /// Color targets, in shader output order.
    pub color_attachments: &'a [ColorAttachment],
}

/// Records GPU commands. Backends implement it. Renderer code records through
/// `&mut dyn CommandEncoder` (ADR-0017 §1).
///
/// Recording is infallible (ADR-0017 §3): misuse such as an invalid handle is remembered and
/// reported as an error when the frame ends.
///
/// The `*_raw` methods exist for backends to implement. Renderer code uses
/// [`begin_render_pass`](trait.CommandEncoder.html#method.begin_render_pass) on
/// `dyn CommandEncoder`, which guarantees every pass is ended.
pub trait CommandEncoder: Send + fmt::Debug {
    /// Backend hook: begin a render pass. Called only by [`RenderPass`].
    fn begin_render_pass_raw(&mut self, desc: &RenderPassDesc<'_>);

    /// Backend hook: end the current render pass. Called only by [`RenderPass`].
    fn end_render_pass_raw(&mut self);

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

impl Drop for RenderPass<'_, '_> {
    fn drop(&mut self) {
        self.encoder.end_render_pass_raw();
    }
}

/// One frame of a surface, returned by [`crate::Device::begin_frame`].
///
/// Record into [`Frame::encoder`] and render to [`Frame::backbuffer`], then pass the frame to
/// [`crate::Device::end_frame`].
#[derive(Debug)]
pub struct Frame {
    surface: SurfaceHandle,
    backbuffer: TextureHandle,
    encoder: Box<dyn CommandEncoder>,
}

impl Frame {
    /// For backends: assembles a frame.
    #[must_use]
    pub fn new(
        surface: SurfaceHandle,
        backbuffer: TextureHandle,
        encoder: Box<dyn CommandEncoder>,
    ) -> Self {
        Self {
            surface,
            backbuffer,
            encoder,
        }
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
        fn into_any(self: Box<Self>) -> Box<dyn Any + Send> {
            self
        }
    }

    #[test]
    fn render_pass_guard_always_ends_the_pass() {
        let mut encoder: Box<dyn CommandEncoder> = Box::new(Recorder::default());
        let desc = RenderPassDesc {
            label: "test",
            color_attachments: &[],
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
