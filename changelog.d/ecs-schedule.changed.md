- The `cube` example runs on the ECS: the camera, spinning cube, floor tiles and pillars are
  entities, gameplay runs as systems, and the renderer draws from a draw list extracted in the
  `Extract` stage.
- `World::resource_mut` returns `Mut<R>`, like `get_mut`. Bind it with `mut` to write through it.
