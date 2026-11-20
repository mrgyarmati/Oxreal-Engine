- `oxreal-hierarchy`: parent/child relations and transform propagation (ADR-0005, ADR-0014):
  - `ChildOf::new(parent)` attaches an entity. The parent's ordered `Children` is kept in step
    by hooks. Both components are immutable, so no edit skips those hooks.
  - Links to dead or despawning parents, links that would form cycles, and links deeper than
    `MAX_DEPTH` are refused. `set_parent` reports them as errors.
  - When a parent is despawned, each child follows its link: it detaches and keeps its place in
    the world (the default, as in Unreal), or it's despawned first
    (`ChildOf::new(parent).despawn_with_parent()`).
  - `set_parent` and `remove_parent` take a `TransformRule`: keep the local transform, or keep
    the place in the world.
  - `LocalTransform` and `WorldTransform`: `propagate_transforms` (in `PostUpdate`, added by
    `add_systems`) computes `world = parent * local` down every tree.
  - A benchmark: `cargo bench -p oxreal-hierarchy`.
