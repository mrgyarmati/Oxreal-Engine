- `oxreal-renderer`: draws a plain-data `RenderWorld` (a camera and meshes) through the RHI with
  depth testing and a default checkerboard material, into a color target the application gives
  it. It loads shaders only from the cooked directory, and refuses one cooked from other
  sources or damaged, saying to run `cargo xtask cook-shaders`. Meshes are validated when
  registered, and draws naming an unregistered mesh are skipped.
- `oxreal-rhi`: `Device::owns_encoder`, which says whether a device created a command encoder,
  and `BUFFER_ADDRESS_ALIGNMENT`: every buffer address is a multiple of 16 bytes.
- `oxreal-shader-compiler`: `SpirvLayouts`, which reads struct offsets and pointer strides from
  SPIR-V, for tests that check a shader's memory layout against Rust; and `compile_options()`,
  the fixed `slangc` arguments every compile uses.
