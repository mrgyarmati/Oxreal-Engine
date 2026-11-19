- Every pipeline now shares a layout that includes the bindless descriptor set. Shaders that
  declare any other descriptor resource are rejected when the pipeline is created.
- `oxreal-shader-compiler` declares Slang's texture capability atoms, so sampling shaders no
  longer warn "profile implicitly upgraded". The SPIR-V is unchanged, and unsupported
  capabilities are still rejected by the allowlist.
