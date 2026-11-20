- `oxreal-player`: the game executable. It opens a window, creates the device, and runs the ECS
  world and schedule and the renderer every frame, with a demo scene and a fly camera.
  It loads shaders from the cooked directory (`--cooked`, `OXREAL_COOKED_DIR`, or `cooked/` next
  to the executable) and contains no shader compiler. Run `cargo xtask cook-shaders`, then
  `cargo run -p oxreal-player`.
- `oxreal-render-bridge`: the `Camera` and `MeshInstance` components, and the `Extract` system
  that fills the renderer's `RenderWorld` from them (the `ExtractedWorld` resource).
- `oxreal-renderer`: `MeshHandle::DANGLING`, a handle that never names a mesh.
