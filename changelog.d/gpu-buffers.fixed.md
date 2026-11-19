- A failed submission now frees the objects it referenced right away when nothing was
  submitted, and keeps them until teardown only after a lost device, as the Vulkan spec
  requires. Before, `read_texture` freed too early after a lost device, and command pools were
  kept too long after an out-of-memory failure.
- Destroying a texture, pipeline or buffer no longer reports an error just because a later
  cleanup step failed.
