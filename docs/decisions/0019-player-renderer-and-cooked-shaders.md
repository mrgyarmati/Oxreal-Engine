# ADR-0019: The player application, the M1 renderer and cooked shaders

- **Status:** Accepted
- **Date:** 2026-11-20
- **Related:** PRD §7 (M1), TAD §2–§4 and §7, brief rule 3 (the renderer consumes an extracted render world), ADR-0004 (layering), ADR-0005 (hierarchy), ADR-0008 (gameplay framework), ADR-0013 §4 (no shader compiler in shipped games), ADR-0014 (ECS), ADR-0016/0017 (RHI), ADR-0018 (input)

## Context

M1's exit criterion is "a small app renders and navigates a 3D scene on both platforms". Every
part exists, but the app is still `examples/cube` inside the Vulkan backend crate:
- it builds its scene in code;
- it drives the RHI directly (pipelines, depth buffer, push constants);
- it **compiles Slang at startup**.

The TAD plans `applications/player` (layer `app-runtime`) and `rendering/renderer`. Three
constraints shape them:

1. **ADR-0013 §4:** "No shader compiler at runtime in shipped games. Players load precompiled
   SPIR-V from the cooked shader cache". There's no cache or cook step yet. The layer rules
   forbid the player from depending on `oxreal-shader-compiler` (a `tool` crate), even as a
   build dependency.
2. **`ShaderCode::from_spirv_unchecked`** (ADR-0016) is `unsafe`. Its contract requires valid
   SPIR-V for Oxreal's feature set, and says "words from anywhere else must be validated before
   use". So shaders loaded from disk need an integrity story before they can soundly reach it.
3. **Brief rule 3:** the renderer doesn't own scene data, and consumes an extracted render
   world. The TAD places extraction glue in the `gameplay` layer.

## Options considered

### How a shipped player gets its shaders
- **A. Compile at startup** (what the example does). This breaks ADR-0013 §4, ships a compiler
  dependency, and slows startup.
- **B. Embed SPIR-V in the binary from a build script.** The build script would depend on the
  shader compiler, a `tool` crate, which the layer check forbids for `app-runtime`. It also
  makes every build of the game need `slangc`.
- **C. An offline cook step writes a cooked shader directory, and the player loads it.** The
  directory holds SPIR-V files plus a manifest with hashes, and the player verifies each file
  before use. This matches ADR-0013 §4 and the later asset pipeline. It needs a cook command
  and a small manifest format.

### Where rendering code lives
- **A. In the player** (as in the example). Every future app (the editor viewport, a dedicated
  test app) would copy it, and the backend calls would leak into application code.
- **B. A renderer crate that queries the ECS itself.** It couples rendering to scene storage,
  against rule 3, and makes the renderer impossible to test without a world.
- **C. A renderer crate with a plain-data input.** It takes a `RenderWorld` (camera plus draw
  items) filled by extraction glue. It's testable with hand-made input, and the editor and the
  player share it.

## Decision

### 1. `oxreal-renderer` (rendering layer)
- **It owns the M1 mesh pass:** the mesh pipeline, depth targets (recreated on resize), mesh
  buffers, a default checkerboard material, and the camera projection (ADR-0011's reversed
  depth).
- **It's generic over `oxreal_rhi::Device`.** Only the application names the backend.
- **Input is plain data:** each frame takes a `RenderWorld` (the camera's transform and
  projection settings, plus draw items made of a mesh handle and an object-to-world matrix).
  It doesn't depend on `oxreal-ecs`. Meshes are registered up front and drawn by handle.
- **It creates `ShaderCode` only from verified cooked SPIR-V (§3).** Its SAFETY arguments
  combine the cook pipeline's guarantees with its own obligations: buffer addresses, bindless
  indices and index ranges, the program-wide obligations of `from_spirv_unchecked`.
- **Its shaders** (`Mesh.slang`) move into the crate.

### 2. `oxreal-player` (`applications/player`, app-runtime layer)
- **The shipped executable for M1.** It owns:
  - the platform event loop and window;
  - Vulkan device creation, the only place the backend is named;
  - input fed into `InputState`;
  - cursor capture;
  - frame timing;
  - the ECS world and schedule (with `oxreal-hierarchy`'s propagation);
  - the renderer.
- **Extraction** is an `Extract`-stage system that fills the `RenderWorld` from `Camera`,
  `MeshInstance` and `WorldTransform` components. In M1 it lives in the player. It moves to a
  `gameplay`-layer glue crate when a second application needs it (the editor viewport, M2).
- **The demo scene** (the cube with its moons, the floor, the pillars, the fly camera) is built
  in code in the player, clearly marked as standing in for scene loading (D-07, M2). The fly
  camera stays with it until the gameplay camera arrives (ADR-0008).
- **The cube example retires.** The Vulkan backend keeps its narrow examples (`clear`,
  `triangle`, `textured`, `frame_protocol`, `device_info`). CI runs the player in its place,
  under the same job names, so the ruleset doesn't change.

### 3. The cooked shader directory (interim, until the asset pipeline)
- **`cargo xtask cook-shaders`** compiles every shader listed in each runtime crate's
  `shaders/cook.json`, with the pinned `slangc` through `oxreal-shader-compiler`, including its
  capability allowlist and `spirv-val`. It writes `cooked/shaders/` next to the build's
  executables (`target/<profile>/`), or into `--out <dir>`.
- **The output** is one `<name>.spv` per entry, plus `manifest.json`, which records:
  - a format version;
  - the Slang version and compile options;
  - for each entry: its file, its entry points, and the SHA-256 of its SPIR-V;
  - each entry's source key: the hash of the source and every file its depfile lists. Cooking
    skips entries whose key is unchanged.
- **The renderer's loader** opens the directory, checks the format version, and verifies each
  file's SHA-256 against the manifest before returning its words. Missing, mismatched or
  malformed files are typed errors that say what to run (`cargo xtask cook-shaders`). Where the
  player looks:
  - `--cooked <dir>`, if given;
  - otherwise `OXREAL_COOKED_DIR`;
  - otherwise `cooked/` next to the executable.
- **The trust argument** for `from_spirv_unchecked`:
  - The directory is part of the game's installation, like the executable itself: whoever can
    replace it can replace the executable.
  - The hash check catches corruption and stale or mismatched caches, not tampering.
  - Everything the manifest admits was produced by the pinned compiler with its validation.
- **Dependencies:** `sha2` and `serde_json`, already audited for `xtask`, are now also used by
  `oxreal-renderer` at run time. `dependencies.md` records the wider scope.
- **Deliberately interim.** The asset pipeline (D-08, M3) replaces the directory with the
  cooked asset cache and its stable IDs. The cache keys keep ADR-0013 §4's shape (source and
  dependency hashes, Slang version, options, target), so the move changes the container, not
  the rules.

### 4. Testing
- **Renderer:**
  - unit tests for the loader (valid cache, hash mismatch, missing file, wrong version) and
    for `RenderWorld` handling;
  - a GPU test that renders a frame from a hand-made `RenderWorld` and reads pixels back, run
    on Lavapipe in CI.
- **Player:**
  - CI cooks the shaders, then runs the player for a set number of frames under Xvfb, failing
    on any validation error;
  - the demo scene's systems keep their unit tests.

## Consequences

- **Positive:**
  - M1 has a real executable with no compiler inside, built from layered crates.
  - The editor viewport (M2) can reuse the renderer unchanged.
  - Shaders reach the GPU only from validated, hash-checked output.
- **Negative / costs:**
  - A cook step before running the player: `cargo xtask cook-shaders`, and in CI. A missing or
    stale cache is a clear startup error, never a silent mismatch.
  - `sha2` and `serde_json` ship in the player.
  - The demo scene and the fly camera live in the player until scenes and the gameplay camera
    exist.
- **Follow-up work:**
  - The implementation PR: renderer, player, the cook command, CI.
  - Extraction glue moving to the gameplay layer with the editor.
  - Scene loading (D-07).
  - The asset pipeline's cooked cache (D-08).
- **Revisit triggers:**
  - The cook step proving error-prone in daily work. The next step would then be automatic
    cooking on build in the editor or `xtask`, never a compiler in the game.
  - A second backend, whose shaders would need per-target cooked output.
