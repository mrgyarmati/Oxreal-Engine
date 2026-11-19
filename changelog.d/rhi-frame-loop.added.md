- Oxreal draws to windows. The Vulkan backend creates surfaces and swapchains (sRGB, vsync by
  default; mailbox and immediate optional) and runs a frame loop with two frames in flight.
  Resizing, minimizing and restoring a window recreate the swapchain automatically.
- `oxreal-rhi` gained surfaces, frames (`begin_frame` returns nothing to draw while minimized),
  command encoders, scoped render passes with clear, load and store operations, and typed
  generational handles.
- `clear` example: `cargo run -p oxreal-rhi-vulkan --example clear` opens a window and clears
  it to a slowly changing color.
