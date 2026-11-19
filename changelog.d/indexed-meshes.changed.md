- The `cube` example draws a real mesh: 24 vertices and 36 indices in GPU buffers, with one
  indexed draw (`examples/shaders/Mesh.slang`).
- `CommandEncoder` has two more backend hooks, `set_index_buffer_raw` and `draw_indexed_raw`.
  Only backend implementations are affected.
- GPU selection requires the `fullDrawIndexUint32` feature, so every 32-bit index value is valid.
  Desktop GPUs and Lavapipe support it, and Vulkan 1.4 requires it.
