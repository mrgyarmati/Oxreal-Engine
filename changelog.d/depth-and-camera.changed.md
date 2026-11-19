- `RenderPassDesc` has a `depth_attachment` field and `RenderPipelineDesc` a `depth` field. Set
  them to `None` where no depth is used.
- `RenderPipelineDesc::color_formats` may be empty when the pipeline has a depth state.
- GPU selection requires D32_SFLOAT depth-attachment support. The Vulkan spec doesn't guarantee it,
  but desktop GPUs and Lavapipe support it.
- Render-target and depth textures larger than the device's framebuffer limits are refused at
  creation, with a reason.
