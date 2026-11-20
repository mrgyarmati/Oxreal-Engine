- ECS lookups by component, resource and bundle type are faster, because `TypeId` maps no longer
  use SipHash. Spawning is about 20% faster, and random `get` about 35% faster.
