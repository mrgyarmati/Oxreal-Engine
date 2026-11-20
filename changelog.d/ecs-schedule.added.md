- `oxreal-ecs` schedules (ADR-0014 §5):
  - `System`s declare the components and resources they read and write. Touching anything
    undeclared panics with the declaration to add.
  - A `Schedule` runs them stage by stage (`Input`, `PrePhysics`, `DuringPhysics`,
    `PostPhysics`, `PostUpdate`, `Extract`), honouring `before`/`after` constraints within a
    stage. Duplicate names, unknown or cross-stage constraints, and cycles are reported before
    anything runs, with the systems on the cycle named.
  - `Commands` record spawn, despawn, insert, remove and exclusive closures (`Commands::run`),
    applied in order at the end of each stage. `Commands::spawn` returns a real handle
    immediately.
- `oxreal_input::InputState` is an ECS resource.
