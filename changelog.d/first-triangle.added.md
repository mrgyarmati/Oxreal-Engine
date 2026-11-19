- First draw call. The RHI gains textures, render pipelines with push constants (up to 128
  bytes), `set_pipeline` / `set_push_constants` / `draw` on render passes, standalone command
  submission and `Device::read_texture`.
- The Vulkan backend destroys objects only once the GPU *and every encoder still recording* are
  done with them, so destroying a resource mid-frame is safe.
- `triangle` example: a spinning triangle compiled from Slang at startup.
- Pixel tests render offscreen and check the result: clip space is Y-up and counter-clockwise
  triangles are front faces (now written down in ADR-0011 and the conventions).
- Misuse is refused or recovered instead of reaching the driver:
  - a second open frame, a dropped frame, or a tampered `Frame`;
  - a leaked render-pass guard;
  - a texture used by two unsubmitted encoders;
  - a pipeline that doesn't match its pass, or unwritten push constants;
  - shader code with a bad header, a missing entry point or too many push-constant bytes.
- `examples/frame_protocol.rs` checks the frame-loop cases in a real window.
- `OXREAL_VULKAN_SYNC_VALIDATION=1` turns on Vulkan synchronization validation (missing or
  wrong barriers become errors). Device creation fails if it isn't available, so it can't be
  silently off. CI runs every GPU test and example with it.
