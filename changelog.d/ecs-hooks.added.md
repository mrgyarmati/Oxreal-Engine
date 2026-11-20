- `oxreal-ecs` lifecycle hooks (ADR-0014 §4):
  - A component type declares `on_add`, `on_insert`, `on_replace` and `on_remove` hooks through
    `Component::HOOKS`, so they apply in every world.
  - Hooks read and write component data and resources through `HookWorld` and queue commands.
    Those commands run right after the change that triggered them, depth-first, both for direct
    `World` calls and at a schedule's sync points.
  - On despawn, every component's `on_replace` runs, then every `on_remove`, while the entity is
    still whole.
