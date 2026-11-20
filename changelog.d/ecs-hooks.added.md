- `oxreal-ecs` lifecycle hooks (ADR-0014 §4):
  - A component type declares `on_add`, `on_insert`, `on_replace` and `on_remove` hooks through
    `Component::HOOKS`, so they apply in every world. A hook receives a `HookWorld` and a
    `HookContext` (the entity, and whether it was spawned, inserted, removed or despawned).
  - Hooks read and write component data and resources, and queue commands. Those commands run
    right after the change that triggered them, depth-first and in constant stack, both for
    direct `World` calls and at a schedule's sync points.
  - Despawn is post-order: an entity's hooks run while it's whole, then the commands they
    queued (despawning dependents, for example), and only then is it removed. So children go
    before parents. A despawning entity refuses changes (`EcsError::Despawning`).
