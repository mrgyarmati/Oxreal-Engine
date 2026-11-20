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
  LunarG code signature. Linux uses the distro's `spirv-tools`. The cook tests run on all three
  build jobs with `OXREAL_REQUIRE_SPIRV_VAL=1`, and every build job also runs
  `cargo xtask cook-shaders`.
- **Not yet:** no crate has a recipe until the renderer arrives (the next slice), so the
  workspace cook writes an empty manifest for now.
