- Every pipeline now shares a layout that includes the bindless descriptor set. Shaders that
  declare any other descriptor resource are rejected when the pipeline is created.
- `oxreal-shader-compiler` declares Slang's texture capability atoms, so sampling shaders no
  longer warn "profile implicitly upgraded". The SPIR-V is unchanged, and unsupported
  capabilities are still rejected by the allowlist.
- `ShaderCode::from_spirv_unchecked`'s safety contract now covers bindless indices: only live
  objects' indices, each in its own array, and never sampling a texture the current render
  pass draws into.
- Pipelines whose shaders declare fixed-size descriptor arrays, integer or depth textures, or
  combined image samplers are rejected with a reason.
- A GPU whose update-after-bind descriptor limits are too small is now rejected during GPU
  selection, with a reason, instead of failing at device creation.
