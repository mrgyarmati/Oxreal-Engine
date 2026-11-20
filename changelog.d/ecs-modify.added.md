- `World::modify::<T, _>(entity, |mut value| ...)` edits a component in place while running its
  `on_replace` and `on_insert` hooks (with the new `HookCause::Modify`). It works on immutable
  components, because every change still passes through the hooks.
