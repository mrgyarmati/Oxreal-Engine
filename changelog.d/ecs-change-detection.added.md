- `oxreal-ecs`: change detection (ADR-0014 §6). The `Added<T>` and `Changed<T>` query filters
  select entities whose `T` was added or changed since the system last ran: other systems'
  writes, never its own, and everything on a system's first run. Writing through `Mut`,
  `World::modify`, and replacing a component by `insert` count as changes; fetching mutably
  without writing doesn't. `SystemResources::is_added` and `is_changed` do the same for
  resources.
