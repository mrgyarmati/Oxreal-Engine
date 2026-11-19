- `ShaderCode` is now created with the `unsafe` `ShaderCode::from_spirv_unchecked`. Its caller
  promises valid SPIR-V, such as `oxreal-shader-compiler` output. GPU drivers trust shader code,
  so a safe constructor couldn't be sound.
- `Device::destroy_surface` refuses a surface that has an open frame, and `begin_frame` refuses
  a second open frame on the same surface.
