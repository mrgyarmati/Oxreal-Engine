- `oxreal-ecs`: `World::query_filtered` and `query_filtered_mut` take a `ComponentFilter`.
  `Added` and `Changed` need a system's previous run, so they're refused there at compile time.
- `oxreal-ecs`: query iteration inlines, which made a mutable query over 100k entities 3.5×
  faster. Change detection costs part of that back: in the same benchmark, iteration is 0.34 ms
  before this release and 0.14 ms after. Spawning and despawning are about 15–20% slower, since
  each component value now carries its change ticks.
