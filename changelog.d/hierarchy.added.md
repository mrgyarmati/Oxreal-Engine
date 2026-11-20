- `oxreal-hierarchy`: parent/child relations and transform propagation (ADR-0005, ADR-0014):
  - `ChildOf::new(parent)` attaches an entity. The parent's ordered `Children` is kept in step
    by hooks. Links to dead parents and links that would form cycles are refused, and
    `set_parent` reports them as errors.
  - Despawning a parent despawns its children first, recursively. Removing `Children` detaches
    them instead.
  - `LocalTransform` and `WorldTransform`: `propagate_transforms` (in `PostUpdate`, added by
    `add_systems`) computes `world = parent * local` down every tree.
  - A benchmark: `cargo bench -p oxreal-hierarchy`.
