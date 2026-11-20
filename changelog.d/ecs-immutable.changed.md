- ECS lookups by component, resource and bundle type are faster, because `TypeId` maps no longer
  use SipHash. In the bundled benchmark, spawning got about 20% faster and random `get` about 35%
  faster.
