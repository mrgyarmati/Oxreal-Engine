- `oxreal-ecs` immutable components: a component with `const MUTABLE: bool = false` can only be
  replaced with `insert`, so its hooks see every change. Writing it in place (`get_mut`,
  `&mut T` in a query, `System::writes`) fails the build with an error saying to use `insert`.
  `cargo check` and rust-analyzer don't report this error.
- `World::is_despawning` and `HookWorld::is_despawning` report an entity whose despawn has
  started.
