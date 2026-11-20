- `oxreal-ecs` immutable components: a component with `const MUTABLE: bool = false` can only be
  replaced with `insert` (so its hooks see every change), never written in place. `get_mut` and
  `&mut T` queries don't compile for it.
- `World::is_despawning` and `HookWorld::is_despawning` report an entity whose despawn has
  started.
