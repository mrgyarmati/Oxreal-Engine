- `oxreal-rhi`: `CommandEncoder` is now an `unsafe trait`. Only backends implement it; a backend
  must be one encoder (its `as_any` and `into_any` return itself, and it records into itself).
- Cooked shader source keys now include the shader compiler's fixed flags (key format 2).
  Shaders cooked before this change are refused as stale: run `cargo xtask cook-shaders` again.
