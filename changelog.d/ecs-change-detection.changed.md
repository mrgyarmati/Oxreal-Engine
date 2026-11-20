- `oxreal-ecs`: mutable query iteration is about 2.7× faster (100k entities: 0.34 ms to 0.13
  ms), because query iterators now inline. Spawning and despawning are about 20% slower, since
  each component value now carries its change ticks.
