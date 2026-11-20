- ECS lookups by component, resource and bundle type are faster: `TypeId` maps no longer use
  SipHash. Spawning is about 20% faster and random `get` about 35% faster.
- The `cube` example's spinning cube has two moons, children that orbit with it through
  transform propagation.
