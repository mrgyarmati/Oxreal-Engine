- `oxreal-renderer`: draws a plain-data `RenderWorld` (a camera and meshes) through the RHI with
  depth testing and a default checkerboard material, into a color target the application gives
  it. It loads shaders only from the cooked directory, and refuses one cooked from other
  sources or damaged, saying to run `cargo xtask cook-shaders`. Meshes are validated when
  registered, and draws naming an unregistered mesh are skipped.
- `Device::owns_encoder`, which says whether a device created a command encoder.
- `oxreal_shader_compiler::SpirvLayouts`, which reads struct offsets and pointer strides from
  SPIR-V, for tests that check a shader's memory layout against Rust.
