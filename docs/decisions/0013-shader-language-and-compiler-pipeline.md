# ADR-0013: Shader language and compiler pipeline

- **Status:** Accepted
- **Date:** 2026-11-19
- **Related:** ADR-0003 (multiple backends), ADR-0011 (matrix and clip conventions), ADR-0012 (Vulkan 1.3), ADR-0009 (pinned tooling), ADR-0002, D-04; prior art: [Cordite](../development/prior-art.md)

## Context

The renderer, materials and later visual material graphs all compile to shaders. We need a
language, a compiler and a pipeline that:

- target SPIR-V for Vulkan 1.3 now, and DXIL (DirectX 12) and Metal later (ADR-0003)
- produce **reflection** (bindings, push constants, entry points) for the RHI and pipeline creation
- support **modules or imports** and abstractions for a growing material system
- feel familiar to Unreal developers, whose shader language is HLSL
- give reproducible builds, hot reload in the editor, and **no compiler inside shipped games**
- work identically on Windows and Linux

### Evidence gathered on 2026-11-19

- **Slang** (github.com/shader-slang/slang, Apache-2.0 WITH LLVM-exception, which `deny.toml`
  allows) is very active, with releases 2026.18.2 to **2026.22** between September and
  10 November. Official prebuilt archives exist for windows-x86_64 and linux-x86_64 (glibc
  2.27/2.28 builds), and the Vulkan SDK ships `slangc` (2026.17 in SDK 1.4.363.1).
- **Spike, run locally with that `slangc`.** A two-file shader using
  - `import` of a module
  - a constant buffer and push constants
  - a bindless `Texture2D[]` with `NonUniformResourceIndex`
  - `-matrix-layout-column-major`, target `spirv` with profile `spirv_1_6`

  compiled cleanly. Results:
  - `spirv-val --target-env vulkan1.3` reported the module **valid**.
  - `-reflection-json` listed every parameter with its set, binding, push-constant block and
    entry-point stage.
  - `-depfile` listed both source files.
  - Disassembly showed only the capabilities the code needs (`Shader`, `ShaderNonUniform`,
    `RuntimeDescriptorArray`, `SampledImageArrayNonUniformIndexing`). The compiler warned that it
    had widened the *profile*, but it did not declare the widened capabilities.
- **Rust integration of Slang:** the only binding crate, `shader-slang` 0.1.0, is immature
  (released July 2025, about 30K downloads). The `slangc` CLI is complete and stable.
- **Cordite** used Slang (2026.19) for SPIR-V and, through DXC, DXIL in production, with a
  precompiled shader cache so exported games don't depend on the compiler.
- **Alternatives on crates.io:** `hassle-rs` 0.12 (DXC bindings, MIT), `shaderc` 0.10.1
  (Apache-2.0, builds Google's C++ shaderc), `naga` 30.0.2 (pure Rust, part of wgpu, very
  active), and `spirv-builder` 0.11 (rust-gpu).

## Options considered

- **Slang.** HLSL-compatible syntax (familiar to Unreal developers), real modules, generics and
  interfaces (a strong fit for the material system), first-class reflection, and SPIR-V, DXIL
  (via DXC), Metal and GLSL targets from one source. Proven in Cordite. Costs: a large external
  binary toolchain, frequent releases to track, and no mature Rust bindings.
- **HLSL with DXC.** Familiar and industry-standard. DXC emits SPIR-V and DXIL. But it has no
  module system, the SPIR-V path is a secondary backend, and Metal needs a separate translator.
- **GLSL with glslang or shaderc.** Native to Vulkan's history, but no modules, unfamiliar to
  Unreal developers, a weak path to DirectX 12 and Metal, and `shaderc` builds C++ from source.
- **WGSL with naga.** Pure Rust, no external tools, great for wgpu. But WGSL's feature set tracks
  WebGPU (limited bindless and descriptor indexing, fewer advanced features), and it's
  unfamiliar to the target audience.
- **rust-gpu (shaders written in Rust).** An appealing Rust-native story, but it's SPIR-V only,
  needs a pinned nightly toolchain and has a small user base. Possible future addition, not the
  foundation.

### Integration options for Slang
- **Invoke the `slangc` CLI as a subprocess.** No `unsafe` FFI, no C++ build, crash isolation,
  trivially pinned, and identical on Windows and Linux. Costs process-spawn time per compile,
  which is mitigated by caching.
- **Link `libslang` through FFI** (immature crate, or our own bindings to its COM-style API).
  Lower latency and in-process reflection, but a large `unsafe` surface and ABI tracking for a
  library released every one to two weeks.

## Decision

1. **Slang is Oxreal's shader language** for engine shaders and, later, user and material
   shaders. Shaders use Slang modules (`import`). Shared engine code (conventions, BRDFs,
   bindless access) lives in engine Slang modules.
2. **The compiler is the `slangc` CLI, invoked as a subprocess** by a tool-layer crate
   (`oxreal-shader-compiler`, created with the first shaders). That crate:
   - builds the command line: `-target spirv -profile spirv_1_6 -matrix-layout-column-major
     -fvk-use-entrypoint-name`, plus `-g` and no optimization in debug builds
   - parses `-reflection-json` into engine-owned reflection types consumed by the RHI and
     pipeline layout code. `slangc`'s JSON format never leaks beyond that crate.
   - parses `-depfile` output for dependency tracking and hot reload
   - **checks every emitted SPIR-V module's declared capabilities against an allowlist** that
     matches the device features required by ADR-0012 and the bind-model ADR. An unexpected
     capability is a build error naming the shader, entry point and capability.
   - runs `spirv-val --target-env vulkan1.3` in debug and test builds when it's available
3. **Slang is pinned and provisioned reproducibly**, not taken from `PATH` or the Vulkan SDK:
   - The required version (initially **2026.22**) and the SHA-256 of each official release archive
     (windows-x86_64 `dd0c4a86…f17b`, linux-x86_64 `a877b75c…47e6`, as published by GitHub for the
     release assets) are recorded in the repository. The full hashes live in the tool manifest.
   - A `cargo xtask` command downloads, verifies and unpacks it into a git-ignored tools
     directory. CI does the same.
   - `OXREAL_SLANGC` may point to another `slangc` for experiments. Builds using it are flagged
     in logs.
   - Upgrading Slang is a reviewed PR that updates the version and hashes and passes the full
     shader test suite.
4. **No shader compiler at runtime in shipped games.** Players load precompiled SPIR-V (later
   DXIL and Metal) from the cooked shader cache. The editor and tools compile, including hot
   reload. Cache keys are (hash of the source and every dependency from the depfile, Slang
   version, compile options, target).
5. **Conventions:** matrices are column-major (ADR-0011). Shaders output the engine's Y-up clip
   space, and the Vulkan backend's negative viewport handles the flip (no `-fvk-invert-y`).
   Binding layout follows the bind-model ADR. Reflection is used to validate and create layouts,
   not to define the engine's conventions.
6. **Future backends:** DirectX 12 compiles the same Slang source to DXIL (Slang with DXC) and
   Metal compiles to Metal, both decided when those backends are built. No shader source forks
   per backend.
7. **Licensing:** `slangc` is a build tool and is not linked into Oxreal binaries. If the editor
   distribution later bundles it for user shader compilation, its license notices ship with it.

## Consequences

- One HLSL-like language with modules and generics, familiar to Unreal developers, and ready for
  DirectX 12 and Metal, with evidence from both the spike and Cordite.
- No FFI or C++ build for shader compilation, and compiler crashes can't take down the editor.
- Reproducible shader builds: a pinned version and hash-verified downloads on every machine
  and in CI.
- Costs: process spawn per compile (mitigated by caching and batching entry points), a toolchain
  download of 61 MB (Windows) or 79 MB (Linux) per developer and CI run (mitigated by caching),
  and regular Slang upgrade PRs.
- Capability allowlisting turns device-feature mismatches into build errors instead of
  driver-specific runtime failures.
- Follow-up work: the `cargo xtask` Slang provisioning command, `oxreal-shader-compiler`, and
  shader tests in CI (D-19).
- Revisit triggers: hot-reload latency becoming a measured problem (consider linking `libslang`),
  a mature Slang Rust binding appearing, or Slang development stalling (HLSL with DXC is the
  closest fallback, since the syntax is largely shared).
