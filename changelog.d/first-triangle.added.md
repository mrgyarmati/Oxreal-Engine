- First draw call. The RHI gains textures, render pipelines with push constants (up to 128
  bytes), `set_pipeline` / `set_push_constants` / `draw` on render passes, standalone command
  submission and `Device::read_texture`.
- The Vulkan backend destroys objects only once the GPU *and every encoder still recording* are
  done with them, so destroying a resource mid-frame is safe.
- `triangle` example: a spinning triangle compiled from Slang at startup.
- Pixel tests render offscreen and check the result: clip space is Y-up and counter-clockwise
  triangles are front faces (now written down in ADR-0011 and the conventions).
