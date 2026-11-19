# ADR-0011: Math library and coordinate conventions

- **Status:** Accepted
- **Date:** 2026-11-19
- **Related:** TAD §7, ADR-0001 (no third-party API as engine API without a decision), ADR-0003 (no Vulkan-isms in engine conventions), ADR-0006 (glTF/Blender import), TAD §5.8 (C ABI), D-17; prior art: [Cordite](../development/prior-art.md)

## Context

Every engine subsystem, shader and asset importer depends on one set of math types and spatial
conventions. Getting them inconsistent produces the most expensive class of bugs: mirrored
meshes, inverted rotations, depth-precision artifacts, and per-backend projection fixes.

Oxreal renders through Vulkan first, but DirectX 12 and Metal must remain possible (ADR-0003). It
imports glTF/GLB first and works alongside Blender (ADR-0006). The PRD targets large streaming
worlds.

**Evidence from Cordite** (the maintainer's C++ engine, see prior-art.md) is that its conventions
were used in production code with pixel-asserting renderer tests across Vulkan and Direct3D 12:
- right-handed, +Y up, metres, identity facing −Z
- column-major matrices with `world = parent * local`
- Direct3D-style clip space (Y up, depth [0, 1]) on every backend
- reversed depth
- radians in code, degrees in UI and files
- linear color

## Options considered

### Math library
- **`glam` 0.34.3** (MIT OR Apache-2.0, 194M downloads, very active: 0.34.3 released
  2026-11-08, MSRV 1.85). SIMD `f32` types, an `f64` family (`DVec3`, `DMat4`), integer vectors,
  and a `camera` module whose right-handed projections were verified in its source. Optional
  `serde`, `bytemuck`, `mint` and `approx` features. It's 0.x, with roughly one breaking minor
  release a year.
- **`nalgebra` 0.35** (Apache-2.0). General linear algebra with generic dimensions. Heavier,
  slower to compile, and its generic-heavy API is a poor fit for gameplay code and a future C ABI.
- **`ultraviolet` 0.10** has "wide" SIMD types suited to batch math, but it's maintained less
  actively (last release 2025-04).
- **Our own math library.** Full control, but a large surface (vectors, matrices, quaternions,
  SIMD) to write, optimize and test before any engine work. No benefit over glam that justifies
  that.

### Exposing the types
- **A. Re-export glam's types from `oxreal-math`.** One set of types everywhere with no conversion
  cost, and the ecosystem (physics, egui and others) interoperates through glam or `mint`.
- **B. Newtype wrappers around glam.** Full API control, but hundreds of forwarding methods and
  friction at every library boundary.

### Up axis and handedness
- **Right-handed, +Y up.** Matches glTF (our first-class format, so no axis conversion on import),
  right-handed view math as in glam's `camera::rh`, Jolt's default gravity (−Y), and Cordite.
  Rapier doesn't assume an up axis.
- **Left-handed, +Z up (Unreal).** Familiar to Unreal developers, but every glTF and Blender
  import would need axis and handedness conversion, and most Rust math and graphics code assumes
  right-handed.
- **Right-handed, +Z up (Blender).** Matches the DCC tool, but not glTF or the physics defaults.

### Clip space
- **One engine convention: Y up, depth [0, 1]** (Direct3D/Metal style). The Vulkan backend flips
  Y with a negative viewport height (core since Vulkan 1.1). Matrices and shaders are identical
  for every backend.
- **Native Vulkan (Y down, depth [0, 1])** via glam's `camera::rh::proj::vulkan`. Simplest for
  Vulkan alone, but it bakes a Vulkan-ism into every matrix and shader, which breaks ADR-0003
  once DirectX 12 or Metal arrive.

## Decision

1. **`glam` is the math foundation, re-exported through `oxreal-math` (option A).** This is the
   explicit ADR-0001 decision to expose a library's types. Engine code and gameplay code import
   math from `oxreal_math`, never from `glam` directly, so the dependency stays in one place.
   - Upgrading glam to a new breaking release is a breaking Oxreal change (a minor bump before
     1.0, ADR-0009) with a `changed` changelog fragment.
   - `oxreal-math` **owns** the engine-specific pieces: `Transform` (translation, rotation, scale),
     projections (§4), `Aabb`, `Frustum`, color types and angle helpers.
   - Optional glam features (`serde`, `bytemuck`) are enabled by the decisions that need them
     (serialization, GPU upload).
2. **Spatial conventions** (identical to Cordite's):
   - **Right-handed, +Y up, +X right, forward is −Z.** An identity rotation faces −Z, and a
     camera looks down −Z. glTF assets face +Z by spec, so placing one with identity rotation
     makes it face the camera's default view, as glTF intends.
   - **Units:** 1 unit = 1 metre, plus kilograms and seconds (SI). The editor may *display*
     other units, but data and code are always SI. This deliberately differs from Unreal's
     centimetres.
   - **Rotations** are unit quaternions. Euler angles exist only at UI and file boundaries.
   - **Angles** are radians in code, and degrees in UI and human-edited files.
3. **Matrices:** column-major storage, column vectors (`v' = M * v`), and composition
   `world = parent * local`. This is glam's native layout and matches GPU buffer layout with no
   transposes.
4. **Clip space and depth:**
   - The engine convention is **Y up and depth [0, 1]** for all backends. The Vulkan backend
     applies a negative viewport height. glam's Vulkan-specific (Y-down) projections are not
     used.
   - **Reversed depth** (near = 1, far = 0) with an **infinite far plane** is the default
     perspective for scene rendering. Depth is `D32_SFLOAT`, cleared to 0, with a
     `GREATER_OR_EQUAL` comparison.
   - `oxreal-math` provides the projection functions (perspective, infinite reverse and
     orthographic), each with unit tests that check known points against the convention.
5. **Color:** all math happens in linear RGB. sRGB exists only at storage and display boundaries
   (texture formats, swapchain).
6. **Precision:** `f32` world coordinates for now. Large-world support (f64 positions via glam's
   `DVec3`, or origin rebasing) is decided when the streaming-world benchmark project (PRD §9)
   needs it. The authoring and serialization formats must not rule out f64 positions.
7. **Interop:**
   - The **glTF importer** needs no axis conversion: glTF is right-handed, +Y up, metres.
   - **FBX** and other importers convert their axis and unit conventions at import time.
   - Blender users export glTF as usual, since Blender's glTF exporter converts Z-up to Y-up.
   - The future **C ABI** (D-21) defines its own plain `#[repr(C)]` math structs instead of
     exposing glam's SIMD-aligned layouts.

## Consequences

- One convention from asset import to shader output, proven in Cordite. Future DirectX 12 and
  Metal backends need no matrix or shader changes, and Vulkan needs one viewport flip.
- Reversed infinite depth gives near-uniform depth precision at large view distances, at the cost
  of discipline: every depth clear, comparison and depth reconstruction must follow §4, and
  shared shader helpers enforce that.
- Unreal developers find metres and Y-up instead of centimetres and Z-up. The editor can show
  familiar units and gizmo axes without changing the data.
- glam's 0.x releases cause periodic breaking upgrades, kept to `oxreal-math`, and visible to
  users through the changelog.
- Revisit triggers: precision artifacts in the large-world benchmark (moves §6 forward), or glam
  maintenance stopping (we would fork or replace it behind the `oxreal-math` re-exports).
