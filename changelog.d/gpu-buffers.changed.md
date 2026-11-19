- `oxreal-shader-compiler` validates SPIR-V with `--scalar-block-layout`, matching the
  `scalarBlockLayout` feature Oxreal requires.
- `ShaderCode::from_spirv_unchecked`'s safety contract now also covers buffer addresses: shaders
  may only be given addresses of live buffers, and must stay in bounds.
