# ADR-0019: The player application, the M1 renderer and cooked shaders

- **Status:** Accepted
- **Date:** 2026-11-20
- **Related:** PRD §7 (M1), TAD §2–§4, §5.3 and §7, brief rule 3 (the renderer consumes an extracted render world), ADR-0004 (layering), ADR-0005 (hierarchy), ADR-0008 (gameplay framework), ADR-0013 §4 (no shader compiler in shipped games), ADR-0014 (ECS), ADR-0016 (RHI resources, `ShaderCode`), ADR-0017 §1 (renderer code holds `Arc<dyn Device>`), ADR-0018 (input), D-07, D-08

## Context

M1's exit criterion is "a small app renders and navigates a 3D scene on both platforms". Every
part exists, but the app is still `examples/cube` inside the Vulkan backend crate:
- it builds its scene in code;
- it drives the RHI directly;
- it **compiles Slang at startup**.

The TAD plans `applications/player` (layer `app-runtime`) and `rendering/renderer`. Four
constraints shape them:

1. **No compiler in the game.** ADR-0013 §4: "No shader compiler at runtime in shipped games.
   Players load precompiled SPIR-V from the cooked shader cache". There's no cache or cook step
   yet. The layer check forbids `app-runtime` → `tool`, so the player can't depend on
   `oxreal-shader-compiler`, even as a build dependency.
2. **`ShaderCode::from_spirv_unchecked` is `unsafe`** (ADR-0016). Its contract has two kinds of
   obligation:
   - The words must be a valid SPIR-V module within Oxreal's feature set: "words from anywhere
     else must be validated before use".
   - **Program-wide obligations:** every buffer address, bindless index and index range the
     program gives the shader must be valid. The shader must also match the layouts the Rust
     code writes, which `tests/shader_layout.rs` checks at build time today.
3. **Brief rule 3 and TAD §2:** the renderer doesn't own scene data, and consumes an extracted
   render world. Extraction glue belongs to the `gameplay` layer.
4. **ADR-0017 §1:** renderer code holds `Arc<dyn Device>`, records through
   `&mut dyn CommandEncoder`, and is never generic over a backend.

## Options considered

### How a shipped player gets its shaders
- **A. Compile at startup** (what the example does). This breaks ADR-0013 §4, ships a compiler
  dependency, and slows startup.
- **B. Embed SPIR-V from a build script that compiles it.** The build script depends on a
  `tool` crate, which the layer check forbids for `app-runtime`. It also makes every build of
  the game need `slangc`.
- **C. An offline cook step writes a cooked directory, and the player loads it.** The renderer
  checks it against what it was built for (§4). This matches ADR-0013 §4 and leads to the asset
  pipeline's cache (D-08).
- **D. Commit cooked SPIR-V to the repository and embed it with `include_bytes!`, with CI
  checking it's up to date.**
  - Pros: no I/O at run time, no stale-cache case, no hashing dependencies in the game.
  - Cons: binary files in git, reliance on byte-for-byte deterministic `slangc` output, a
    regeneration step in every shader change, and nothing that leads toward the asset cache.

  Rejected for those costs: C's checks (§4) close the same gaps.

### Where rendering code lives
- **A. In the player**, as in the example. Every future app would copy it, and backend calls
  would leak into application code.
- **B. A renderer crate that queries the ECS itself.** It couples rendering to scene storage,
  against rule 3, and makes the renderer impossible to test without a world.
- **C. A renderer crate with a plain-data input**, filled by extraction glue. It's testable
  with hand-made input, and the editor and the player share it.

## Decision

### 1. `oxreal-renderer` (rendering layer)
- **It owns the M1 mesh pass:** the mesh pipeline, depth targets (sized to the color target
  it's given), mesh buffers, a default checkerboard material, and the camera projection
  (ADR-0011's reversed depth). Its shaders (`Mesh.slang`) move into the crate.
- **It holds an `Arc<dyn oxreal_rhi::Device>` and records through `&mut dyn CommandEncoder`**
  (ADR-0017 §1). Only the application names the backend, when it creates the device.
- **Output target:** the **application** owns surfaces and the frame loop, including a
  minimized window, where `begin_frame` returns no frame. Each frame the renderer records into
  an encoder and a color target with its extent, both given by the caller. So an offscreen
  editor viewport can use it unchanged.
- **Input is plain data.** Each frame it takes a `RenderWorld`:
  - the camera's transform and projection settings;
  - draw items, each a mesh handle and an object-to-world matrix.

  It doesn't depend on `oxreal-ecs`.
- **It vouches for the program-wide obligations of `from_spirv_unchecked`, so nothing a caller
  passes can break them:**
  - Pipelines, `ShaderCode`, buffer addresses and bindless indices never appear in its public
    API.
  - Registering a mesh validates its data: every index must be below its vertex count, and
    every index plus the draw's vertex offset must fall in `0..=i32::MAX`. Anything else is a
    typed error, never a draw.
  - Mesh handles are generational. A stale handle in a `RenderWorld` is skipped and reported,
    never drawn.
  - Unregistering a mesh destroys its buffers through the RHI's deferred destruction, which
    keeps them alive for work already recorded (ADR-0016).
  - The shader's layout matches the Rust packing: the source key (§4) ties the cooked SPIR-V to
    exactly the sources whose layouts the renderer's layout test (today's
    `tests/shader_layout.rs`) checks.
- **It creates `ShaderCode` only from SPIR-V its loader has accepted** (§4).

### 2. Extraction: `oxreal-render-bridge` (gameplay layer)
- **It holds the components that make an entity visible:**
  - `Camera`: the projection settings;
  - `MeshInstance`: a renderer mesh handle.
- **It holds the `Extract`-stage system** that fills the renderer's `RenderWorld` from
  `Camera`, `MeshInstance` and `WorldTransform`.
- **It exists now, not later.** It holds real code, and TAD §2 puts extraction here. Creating
  it now gives these components their final identity before scene files (D-07) start naming
  them.

### 3. `oxreal-player` (`applications/player`, app-runtime layer)
- **The shipped executable for M1.** It owns:
  - the platform event loop and window;
  - device creation, the only place the Vulkan backend is named;
  - surfaces and the frame loop;
  - input, fed into `InputState`;
  - cursor capture;
  - frame timing;
  - the ECS world and schedule, with `oxreal-hierarchy` and `oxreal-render-bridge`;
  - the renderer.
- **The demo scene** (the cube with its moons, the floor, the pillars) is built in code in the
  player, marked as standing in for scene loading (D-07). The fly camera stays with it until
  the gameplay camera arrives (ADR-0008).
- **The cube example retires.** The Vulkan backend keeps its narrow examples.

### 4. The cooked shader directory (interim, until the asset pipeline)
- **The cook logic lives in `oxreal-shader-compiler`**, which owns the manifest's writer.
  `cargo xtask cook-shaders` runs it.
  - It cooks every workspace crate that has a `shaders/cook.json`.
  - Each entry names an output, a source file, its entry points, its include directories and
    its **declared inputs**: the source and every module it imports, across crates. For
    example, `Mesh.slang` imports `oxreal-rhi`'s `Bindless.slang`.
  - **Declared inputs must be complete.** Cooking fails if `slangc`'s depfile lists a file that
    isn't declared. Depfile entries are raw absolute paths, so the comparison follows one rule:
    - canonicalize both sides, with the `\\?\` prefix stripped on Windows;
    - compare case-insensitively on Windows, case-sensitively elsewhere;
    - map both to workspace-relative paths.

    A depfile entry inside the pinned Slang installation (its core modules) is covered by the
    Slang version and skipped. Any other entry outside the workspace is an error.
- **Validation is mandatory.** Cooking uses the pinned `slangc` and its capability allowlist,
  and **fails if `spirv-val` isn't found**. There's no skip flag: the manifest is the
  attestation that `from_spirv_unchecked` relies on.
- **The compiler is exactly the pinned one.** Cooking fails unless the compiler reports exactly
  the version pinned in `tools/slang.json`. `OXREAL_SLANGC` may name another binary, but only
  one that reports that exact version. (Compiling for tests and experiments keeps today's
  warning-only behaviour. Only cooking, which attests, is strict.)
- **Optimization:** `--profile release` cooks with Release optimization, other profiles with
  Debug. The level is recorded and is part of the cook cache key. The loader accepts either
  level, since both are validated.
- **Output** goes under the cargo target directory (from `cargo metadata`, so `--target` and
  `CARGO_TARGET_DIR` are honoured), in the profile's directory (`dev` maps to `debug`), as
  `cooked/shaders/`. `--out <dir>` overrides the root. The directory holds one `<name>.spv`
  per entry, plus `manifest.json`, which records:
  - a format version;
  - the Slang version, the `spirv-val` version, and the target (SPIR-V 1.6 for Vulkan 1.3);
  - for each entry: its file, its entry points, its optimization level, the SHA-256 of its
    SPIR-V, and its **source key**.

  Cooking skips entries whose full key (source key plus optimization level) is unchanged.
- **The source key** is a SHA-256 over length-prefixed records, in this order:
  - the key format version;
  - the pinned Slang version string from `tools/slang.json`;
  - the target;
  - the fixed `slangc` arguments every compile passes (`compile_options()`), which decide memory
    layouts. *(Amended 2026-11-20, key format `/2`; see the second slice's implementation
    notes.)*
  - the entry's normalized `cook.json` record: output name, source, entry points, include
    directories and declared inputs, in that order, so renaming an entry point or reordering
    include directories changes the key;
  - each declared input as a pair of its path and its bytes, in declared order.

  Paths are **workspace-relative, `/`-separated and case-exact, as written in `cook.json`**.
  They're never derived from the filesystem, so the key is the same in every checkout and on
  every OS. `.gitattributes` keeps sources' line endings identical across platforms
  (`eol=lf`).
- **The renderer's loader**, which owns the manifest's reader:
  - **The source key it expects is built into the renderer.** Its build script computes it
    with the same rules from the same `cook.json` and `tools/slang.json`. That needs no
    compiler, so the layer check passes. The build script:
    - emits `rerun-if-changed` for every declared input, for `shaders/cook.json` and for
      `tools/slang.json`;
    - fails the build if a declared input is missing;
    - ties the renderer to the repository layout, which `publish = false` already implies.

    The round-trip test (§5) guards that the build script's key and the cook's key stay
    identical.
  - **It refuses a cache cooked from other sources**, with a typed `StaleCache` error saying to
    run `cargo xtask cook-shaders`. This, not the file hash, is what keeps the shader's layouts
    matched to the Rust code whose layouts the renderer's tests check.
  - **It verifies each file's SHA-256 against the manifest**, which catches corruption.
  - **It treats the manifest as untrusted input.** File names must be plain names: no
    separators, no `..`, no absolute paths, and an exact case match on every platform.
  - Missing, mismatched and malformed files are typed errors, never panics.
- **Where the player looks** for the cooked root (which contains `shaders/`):
  - `--cooked <dir>`, if given;
  - otherwise `OXREAL_COOKED_DIR`;
  - otherwise `cooked/` next to the executable. If `current_exe()` fails, that's a typed error.
- **The trust argument.** The cooked directory is part of the game's installation, like the
  executable itself.
  - Everything a manifest admits was produced by the pinned compiler with mandatory
    validation, from the sources the renderer was built for.
  - The source key catches stale caches, and the file hashes catch corruption. **Neither
    defends against tampering**, which is out of scope: whoever can replace the directory can
    replace the executable.
  - The overrides (`--cooked`, `OXREAL_COOKED_DIR`) are honoured in every build, as developer
    and modding conveniences at the trust level of the process's command line and environment.
- **Dependencies:**
  - `sha2` and `serde_json` are already audited for `xtask`. They now also ship in the
    renderer, and `dependencies.md` records the wider scope.
  - The manifest is read as an untyped `serde_json::Value` by hand, so **no `serde` derive** is
    adopted. `serde` and the cooked scene format stay open (D-07).
- **Deliberately interim.** The asset pipeline (D-08, M3) replaces the directory with the
  cooked asset cache. The keys keep ADR-0013 §4's shape (source and dependency hashes, Slang
  version, options, target), so the move changes the container, not the rules.

### 5. Testing and CI
- **Renderer:**
  - unit tests for the loader: a valid cache, a stale source key, a hash mismatch, a missing
    file, a wrong format version, and unsafe file names;
  - **a round-trip test** that dev-depends on `oxreal-shader-compiler` (dev-dependencies are
    exempt from the layer check): cook into a temporary directory, then load it. It also checks
    that the build script's key equals the cook's. It needs `slangc` and `spirv-val`. Where
    they're missing it's skipped, unless `OXREAL_REQUIRE_SPIRV_VAL` is set, as the shader tests
    already behave. CI sets it, so it can't be skipped there.
  - **a golden test of the source key:** the key of a fixed fixture tree must equal a constant,
    on Windows and Linux alike;
  - **unit tests of the depfile normalization** with Windows-style and Unix-style paths;
  - a GPU test that renders a hand-made `RenderWorld` from a freshly cooked temporary cache and
    reads pixels back, run on Lavapipe in CI;
  - the layout test that `tests/shader_layout.rs` provides today, kept for the renderer's
    shaders.
- **Player:**
  - unit tests for resolving the cooked root on Windows-style and Unix-style paths;
  - the demo scene's systems keep their unit tests.
- **CI:** the existing `vulkan (lavapipe, …)` jobs gain two steps: cook the shaders, then run
  the player for a set number of frames under Xvfb, failing on any validation error. Job names
  don't change, so the ruleset doesn't either. `cargo xtask ci` doesn't cook, because that
  needs `slangc` and `spirv-val`.
- **Windows:**
  - CI's `clippy + test (windows-2025)` job installs `spirv-val` alongside the pinned Slang, and
    runs the cook and the round-trip test, so Windows path handling is exercised in CI.
  - That job has no GPU, so the player itself is run by hand on Windows with a real GPU, and the
    PR records it.

## Consequences

- **Positive:**
  - M1 has a real executable with no compiler inside, built from layered crates.
  - The editor viewport (M2) can reuse the renderer and the bridge unchanged.
  - Shaders reach the GPU only from validated output, cooked from the exact sources the
    renderer was built for.
- **Negative / costs:**
  - A cook step before running the player: `cargo xtask cook-shaders`, and in CI. A missing,
    stale or corrupted cache is a clear startup error, never a silent mismatch.
  - Developers need `spirv-val`, which comes with the Vulkan SDK, to cook. `setup.md` says so.
  - `sha2` and `serde_json` ship in the player.
  - The demo scene and the fly camera live in the player until scenes and the gameplay camera
    exist.
- **Follow-up work:**
  - The implementation PR: renderer, bridge, player, cooking, CI, docs.
  - Scene loading (D-07).
  - The asset pipeline's cooked cache (D-08).
- **Revisit triggers:**
  - The cook step proving error-prone in daily work. The next step would then be automatic
    cooking when building or running, never a compiler in the game.
  - A second backend, whose shaders need per-target cooked output.

## Implementation notes (2026-11-20, first slice: cooking)

- **`cargo xtask cook-shaders` runs a tool binary, `oxreal-shader-tool cook`** (the
  `applications/shader-tool` the TAD planned), instead of linking the cook into xtask. xtask
  has depended on no engine crate since M0, so `cargo xtask ci` still runs when engine crates
  don't build. xtask finds the recipes and the output directory from `cargo metadata`; the cook
  logic stays in `oxreal-shader-compiler` (`cook` module), as decided.
- **Recipe** (`shaders/cook.json`, format 1): a `shaders` array of `output`, `source`,
  `entry_points`, `include_dirs` and `inputs`. The source must be among the inputs. Unknown
  fields are errors. Every path is checked to exist with exactly its written case, so a recipe
  that would only work on Windows fails there too.
- **Source key:** the encoding is documented in `cook/key.rs`. A golden test pins the key of a
  fixture tree, and the value was confirmed by an independent shell implementation (`printf`
  and `sha256sum`) of the documented encoding. The renderer's build script will be tested
  against the same value.
- **Manifest** (format 1): `format`, `slang_version`, `spirv_val_version`, `target`, and
  `shaders` sorted by name, each with `name`, `file`, `entry_points`, `optimization`,
  `spirv_sha256` and `source_key`.
- **Atomicity:** every shader is compiled and validated before any file changes. Files are
  replaced through a temporary file and a rename, and the manifest is written last.
- **Windows CI** gets `spirv-val` from the LunarG Vulkan SDK 1.4.363.1 installer, pinned by
  SHA-256 and installed copy-only. The hash was computed from the download after checking its
  LunarG code signature. Linux uses the distro's `spirv-tools`; its Lavapipe jobs take only
  the validation layer from the matching Linux SDK archive (ADR-0012, implementation notes). The
  cook tests run on all three build jobs with `OXREAL_REQUIRE_SPIRV_VAL=1`, and every build job
  also runs `cargo xtask cook-shaders`.
- **Not yet:** no crate has a recipe until the renderer arrives (the next slice), so the
  workspace cook writes an empty manifest for now.
- **Rules added after review:**
  - Output names must differ in more than case, on every platform, since `Mesh.spv` and
    `mesh.spv` are one file on Windows. Windows' reserved device names (`CON`, `NUL`, `COM1`
    and so on) are refused everywhere too.
  - The compiler's own modules are exempt from the declared-input check only if its
    installation directory (symlinks resolved) can't be mistaken for workspace sources: it
    must not contain the workspace (`OXREAL_SLANGC=~/bin/slangc` would exempt every workspace
    file), and inside the workspace it must be under `.tools/`, where the pinned Slang lives.
  - Files of dropped shaders are removed before new files are written, so a rename that only
    changes case keeps the new file on Windows.
  - On Windows, replacing or removing a file retries for up to a second while another process
    briefly holds it (scanners, the indexer). A loader must not keep cooked files open or
    mapped while a cook may run.
- **`spirv-val` differs by platform:** CI validates with v2026.4 (Vulkan SDK, Windows), v2026.1
  (Ubuntu 26.04) and v2025.1 (Ubuntu 24.04). Every cook validates with whatever is installed,
  and the manifest records which, so a cache is only reused with the validator that produced
  it. A shader is checked by all three in CI.
- **Known limitation:** a shader is skipped as unchanged when its declared inputs are. A *new*,
  undeclared file that shadows an import earlier in the include path doesn't change the key,
  so it's only caught when the shader next recompiles and the dependency check runs. Keeping
  module names unique across include directories avoids it.

## Implementation notes (2026-11-20, second slice: the renderer)

- **`oxreal-renderer`** (rendering layer) provides:
  - `CookedShaders::load` (the strict loader);
  - `Renderer::{new, register_mesh, unregister_mesh, render}`;
  - plain-data `RenderWorld` / `Camera` / `DrawItem`;
  - `MeshData` with `Vertex` and 16- or 32-bit `Indices`, plus `MeshData::cube()`;
  - `RenderTarget`, with `RenderTarget::backbuffer(&Frame)`.

  Its public API has no pipeline, `ShaderCode`, address or bindless index.
- **The build script** (`build.rs`) computes the expected source keys from `shaders/cook.json`
  and `tools/slang.json`, and fails the build if a declared input is missing. Its key code
  (`build/source_key.rs`) is also compiled into the crate's tests:
  - the golden test checks it against the cook's golden value;
  - `tests/round_trip.rs` cooks this crate's real recipe and loads it, then cooks edited
    sources and expects `ShaderCacheError::Stale`.
- **The loader opens only `<name>.spv` for the shaders this build expects**, never a path from
  the manifest. It requires the expected source key and entry points, the SHA-256, and the
  SPIR-V magic number. Manifests are parsed strictly.
- **Encoder ownership:** `render` refuses an encoder of another device
  (`RenderError::ForeignEncoder`), through the new `Device::owns_encoder` (ADR-0017,
  implementation notes of 2026-11-20). Without it, a foreign encoder would resolve the
  renderer's handles in another device.
- **Meshes:** validated at registration (every index names a vertex, at most 2³¹ vertices,
  whole triangles), with generational handles. A slot whose generation would wrap is retired.
  Stale handles in a `RenderWorld` are skipped, counted (`RenderStats::skipped`) and logged.
- **Shared layout helper:** the SPIR-V layout reader moved from the backend's test into
  `oxreal_shader_compiler::SpirvLayouts`. Both the backend's and the renderer's
  `tests/shader_layout.rs` use it.
- **What stays for the third slice:** the cube example keeps compiling its own copy of
  `Mesh.slang` (`examples/shaders/`), which the backend's GPU tests also use. When the player
  replaces the example, that copy becomes a backend test shader.
- **Tests:**
  - unit tests of the loader (valid, missing, stale, without the shader, damaged, not SPIR-V,
    missing file, a manifest choosing another file, malformed manifests), the meshes and the
    handles;
  - GPU tests (`tests/gpu.rs`): a cube drawn and a stale handle skipped (pixels read back),
    another device's encoder refused with nothing recorded, three target sizes and 32-bit
    indices, invalid meshes, cameras and targets refused, all with validation and
    synchronization validation reporting nothing.

  The GPU tests ran on an AMD Radeon RX 9070 XT, and run on Lavapipe in CI. The round-trip and
  layout tests run on every CI build job.
- **After review** (the architecture review and the `unsafe` audit):
  - **The source key includes the fixed `slangc` flags** (`compile_options()`, built from the
    same list the compiler passes), because they decide memory layouts. Otherwise a cache cooked
    with other flags could pass for current. The key format is now
    `oxreal-shader-source-key/2`. Both golden values changed, and were confirmed again by the
    independent shell implementation. A renderer unit test checks that its copies of the format
    string, target and flags equal the cook's.
  - **The RHI changes:** `CommandEncoder` became an `unsafe trait`, and buffer addresses are
    16-byte aligned (ADR-0017, implementation notes of 2026-11-20).
  - **A camera is a transform plus a `#[non_exhaustive]` `Projection`.** The bridge's camera
    component can hold the renderer's `Projection` directly. `RenderWorld`, `Camera` and
    `DrawItem` are `#[non_exhaustive]`, with constructors.
  - **`build.rs` fails if the recipe lacks the `Mesh` shader and its entry points,** so a
    missing shader is a build error, not a run-time error variant.
  - **Other fixes:**
    - `render` refuses the renderer's own material texture as a target;
    - one pipeline is cached per color format;
    - skipped draws are logged only when their count changes;
    - mesh capacity is checked before uploading;
    - the push-constant packing is unit-tested at the shader's offsets;
    - `SpirvLayouts::parse` returns `ShaderError::MalformedSpirv`.
  - **Known limits, documented on `Renderer`:**
    - one depth target, recreated when the target size changes;
    - one unsubmitted encoder per renderer at a time;
    - mesh handles belong to the renderer that issued them.
  - **The safety argument's layout step holds for sources that pass
    `tests/shader_layout.rs`,** which CI and `cargo xtask ci` run. Editing the shader means
    running it.

## Implementation notes (2026-11-20, third slice: the bridge and the player)

- **`oxreal-render-bridge`** (gameplay layer) provides:
  - the `Camera` (projection settings) and `MeshInstance` (a mesh handle) components, both
    placed by the entity's `WorldTransform`;
  - the `ExtractedWorld` resource: the `RenderWorld` the application renders
    (`render_world()`) and the camera entity it's seen from (`camera_entity()`). Its fields are
    private, so the split of ownership is the type's: the application sets the background
    (`set_background`), and extraction rewrites the camera and the draws every frame. The
    resource has to be a wrapper anyway: the orphan rule keeps the bridge from making the
    renderer's `RenderWorld` a resource;
  - the `extract render world` system in `Extract`, added by `add_systems`.

  Choices it makes:
  - **Which camera:** a usable one (valid projection, and a world transform with an
    orientation, so not zero scale), the same one every frame while the set of cameras doesn't
    change. Only that is promised; `camera_entity()` reports which. The implementation takes the
    smallest `Entity`, which means nothing across runs. The camera's scale is dropped, since
    cameras are rigid. Choosing deliberately between cameras, which the editor viewport (M2)
    will also need, waits for the gameplay camera (ADR-0020).
  - **No usable camera, no draws:** the render world gets the default camera and no draws, so
    the target is only cleared, with a warning. Nothing from an earlier frame is left behind.
  - A change in the number of cameras or in the one chosen is logged once, not every frame.
  - The draw list is refilled in place, so its memory is reused from frame to frame.
- **`oxreal-player`** (`applications/player`, app-runtime layer) is the cube example's frame
  loop on top of the renderer and the bridge:
  - It holds the device only as `Arc<dyn Device>`, shared with the renderer. `create_device`
    in `run` is the only place the Vulkan backend is named.
  - It resolves the cooked root as §4 says (`cooked.rs`). An empty `OXREAL_COOKED_DIR` counts as
    unset, like Oxreal's other variables, and an empty `--cooked` is refused. "Next to the
    executable" follows `current_exe`: through a symbolic link, Linux reports the real file and
    Windows the link. `cargo xtask cook-shaders` now also honours `CARGO_BUILD_TARGET`, so the
    cook lands next to the binary cargo builds.
  - Its errors are a typed `PlayerError` that keeps each cause as a `#[source]` (ADR-0010), so
    the message at exit shows the whole chain, for example why a camera or a cooked file was
    refused.
  - While the window has zero size (minimized on Windows), no frame can be shown, so the event
    loop sleeps until the next event instead of spinning, except under `--frames`, whose
    timeout must keep running. A restore arrives as a resize.
  - The demo scene (`scene.rs`) and the fly camera (`fly_camera.rs`) moved from the example
    with their tests. The fly camera now writes its entity's `LocalTransform`, which places the
    entity's bridge `Camera`, rather than being read by a hand-written extraction. §3 names
    ADR-0008 as where the gameplay camera comes from; ADR-0020 has superseded it since.
  - Validation follows `Validation::Automatic` (on in debug builds). The player fails on any
    validation error, and with `--frames`, if frames stop arriving.
- **`MeshHandle::DANGLING`** was added to the renderer: a handle that never names a mesh,
  because the pool never issues its slot index. The scene's and the bridge's tests use it to
  run without a GPU, and draws with it are skipped like any stale handle. It isn't an "absent
  mesh" value; that's `Option<MeshHandle>`. The pool's index limit can be lowered in its unit
  tests, which check that "full" and "can't insert" agree at the limit, including after a slot
  is retired. Registering a mesh that doesn't fit now destroys its buffers instead of dropping
  them (a path the capacity check already made unreachable).
- **The cube example retired**, with the backend's test-only dependencies on the ECS, the
  hierarchy and input. **Correction to the second slice's note:** no backend GPU test used the
  example's copy of `Mesh.slang`. Only the backend's `tests/shader_layout.rs` read it, checking a
  shader nothing ran any more. Both were removed. The renderer's own layout test checks the
  shader that is drawn with, and `SpirvLayouts` keeps its unit tests.
- **CI:** the Lavapipe jobs cook the shaders, then run the player for 120 frames under Xvfb,
  instead of the cube example. Job names didn't change.
- **Tested:**
  - unit tests: the bridge's extraction (placement through parents, camera choice and its
    stability, no camera, losing the camera, unusable cameras, missing resource), the
    cooked-root resolution (including real Windows paths on Windows and Unix paths on Unix),
    the command line, the scene, the fly camera, the mesh pool at its limit, and
    `CARGO_BUILD_TARGET` in the cook;
  - on Windows 11 with an AMD Radeon RX 9070 XT: the player ran 300 frames with validation and
    synchronization validation on and reported no errors. A wrong `--cooked` gives the cook
    hint.
  - CI runs the player on Linux (X11 under Xvfb, Lavapipe). It never runs it on Windows (no GPU
    on the hosted runners) or on Wayland.
- **Follow-up, not verified:** on Wayland, a compositor stops sending frame callbacks to a
  hidden window, and Mesa's FIFO present can then block inside the driver, which would freeze
  the event loop (no close, no timeout). Wayland doesn't report minimizing, so the player can't
  avoid it. If a manual test on GNOME and KDE confirms it, the fix belongs in the backend or
  the window layer (for example a bounded acquire, or a present mode with frame pacing done by
  the application), decided in an ADR.
