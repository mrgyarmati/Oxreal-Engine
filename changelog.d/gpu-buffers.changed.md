- `oxreal-shader-compiler` validates SPIR-V with `--scalar-block-layout`, matching the
  `scalarBlockLayout` feature Oxreal requires.
- `ShaderCode::from_spirv_unchecked`'s safety contract now also covers buffer addresses: shaders
  may only be given addresses of live buffers, and must stay in bounds.
- Shader code that declares a SPIR-V capability Oxreal's required device features don't
  guarantee (for example `Int64`) is rejected when the pipeline is created, not only by the
  shader compiler.
