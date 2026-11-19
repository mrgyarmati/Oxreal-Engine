- Pinned shader toolchain. `cargo xtask slang install` downloads Slang 2026.22, verifies its
  SHA-256 and unpacks it into `.tools/`. `cargo xtask ci` runs it automatically.
- `oxreal-shader-compiler` compiles Slang to SPIR-V. It reports `slangc`'s errors with file and
  line, reflects entry points, bindings and push constants, and tracks imported files. It
  rejects shaders that need GPU capabilities Oxreal doesn't guarantee, with a message naming
  them.
- Shader convention: use `SV_VulkanVertexID` / `SV_VulkanInstanceID`. `SV_VertexID` needs the
  `DrawParameters` capability and is rejected.
