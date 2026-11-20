- `oxreal-ecs`, the first slice of the engine ECS (ADR-0014):
  - a `World` of entities, components and resources;
  - generational `Entity` handles: a despawned entity's handle never refers to a later one;
  - archetype storage that keeps queries on dense arrays, with no `unsafe` code;
  - typed queries (`&T`, `&mut T` through a `Mut` smart pointer, `Option<..>`, `Entity`,
    tuples) with `With` / `Without` filters. A query that would alias a component, such as
    `(&mut A, &A)`, is refused with a message naming it.
- An ECS benchmark suite: `cargo bench -p oxreal-ecs`.
