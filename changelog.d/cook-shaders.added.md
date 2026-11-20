- `cargo xtask cook-shaders` cooks every crate's `shaders/cook.json` into the profile's target
  directory (`target/debug/cooked/shaders/` by default, `target/release/...` with
  `--profile release`): SPIR-V compiled with exactly the pinned Slang, validated by `spirv-val`
  (now required for cooking), plus a manifest recording each shader's source key and hash.
  Cooking fails, naming the file, when a shader imports a file its recipe doesn't declare.
- `oxreal-shader-tool`, the shader compiler's command-line tool, which runs the cook.
