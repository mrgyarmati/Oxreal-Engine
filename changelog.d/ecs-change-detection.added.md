- `oxreal-ecs`: change detection (ADR-0014 §6). In a system, the `Added<T>` and `Changed<T>`
  query filters select entities whose `T` was added or changed since the system's previous
  completed run: every write since then, the system's own commands included, but not what it
  wrote itself during that run. On a system's first run, everything counts.
  `SystemResources::is_added` and `is_changed` do the same for resources.
  - What counts as a change: writing through `Mut`, `World::modify` when its edit writes, and
    replacing a component by `insert`. Fetching mutably without writing doesn't.
  - A schedule run on a different world starts its systems over, as on a first run.
- `oxreal-ecs`: `ComponentFilter`, the filters queries on the `World` itself accept (`With`,
  `Without` and tuples of them).
