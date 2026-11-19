- GPU buffers: `Device::create_buffer`, `destroy_buffer` (deferred until the GPU is done),
  `write_buffer`, `read_buffer` and `buffer_address`.
  - Shaders read and write buffers through their 64-bit address, passed in push constants.
  - Writes are copied on the GPU queue, so they take effect in submission order.
  - Buffer structs use the same layout as Rust `#[repr(C)]` (scalar block layout).
- The `triangle` example now pulls its vertices from a GPU buffer.
