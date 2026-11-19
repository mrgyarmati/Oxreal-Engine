# ADR-0012: Vulkan baseline, bindings and GPU memory allocation

- **Status:** Accepted
- **Date:** 2026-11-19
- **Related:** ADR-0003 (Vulkan behind an engine-owned RHI), ADR-0002 (Windows and Linux), ADR-0010 (diagnostics), ADR-0011 (clip space), D-03, D-14 (driver part), D-19; prior art: [Cordite](../development/prior-art.md)

## Context

Milestone 1 needs a Vulkan backend: instance and device creation, swapchain, GPU memory and
validation. Three choices underpin it: the **minimum Vulkan version and features** we require,
the **Rust bindings** we call Vulkan through, and the **GPU memory allocator**. All three are
implementation details of `oxreal-rhi-vulkan` and must never leak through the RHI (ADR-0003).

Facts checked on 2026-11-19:

- **`ash`** (MIT OR Apache-2.0, 53.6M downloads): the latest crates.io release is
  **0.38.0+1.3.281** (April 2024), with features `loaded` (runtime loading via `libloading`),
  `linked`, `debug` and `std`. The repository is active (commits in October 2026), and its
  unreleased main branch is at Vulkan **1.4.352**, but no newer version has been published.
- **`ash-window` 0.13.0** (MIT OR Apache-2.0) creates surfaces from `raw-window-handle` 0.6.
- **`gpu-allocator` 0.28.0** (MIT OR Apache-2.0, Traverse Research, 32.6M downloads, repository
  active as of 2026-11-18) is a pure-Rust allocator for **Vulkan, DirectX 12 and Metal**. It
  depends on `ash` 0.38 and logs through the `log` crate.
- **`vk-mem` 0.5.0** (MIT/Apache-2.0, 0.2M downloads) binds AMD's C++ Vulkan Memory Allocator
  (VMA).
- **`vulkanalia` 0.35.0** (Apache-2.0, 0.1M downloads, active) is an alternative generated
  binding. **`erupt`** was last released in 2022. **`vulkano` 0.35** is a safe, opinionated
  wrapper.
- **Driver support:** NVIDIA, AMD and Intel shipped conformant Vulkan 1.3 drivers at its launch
  (January 2022). AMD named Vega and RDNA. Intel's Arc conformance followed. Mesa's Lavapipe (the
  software rasterizer used in CI) is conformant to 1.3. Vulkan 1.4 (December 2024) is supported
  by current NVIDIA drivers (Maxwell and newer), Mesa 25.0+ (RADV gfx8+, NVK) and announced AMD
  and Intel drivers. Lavapipe exposes 1.4's extensions but had not been submitted for 1.4
  conformance as of early 2025.
- **Cordite** required Vulkan 1.3 and found it practical. It also had no device-loss handling
  (its risk R8).

## Options considered

### Baseline version
- **Vulkan 1.3 core.** Mature driver support on all three vendors and both operating systems,
  Lavapipe-conformant for CI, and it covers what a modern renderer needs (dynamic rendering,
  synchronization2, timeline semaphores, buffer device address, descriptor indexing). Supported
  by released `ash`.
- **Vulkan 1.4 core.** Push descriptors, dynamic-rendering local read and scalar block layout
  become mandatory. But older-driver coverage is thinner, Lavapipe 1.4 isn't conformance-tested,
  and **no released `ash` has 1.4 definitions**.
- **Vulkan 1.2.** Wider legacy coverage, but no dynamic rendering or synchronization2 in core.
  We would need render-pass objects and the legacy sync API in every code path.

### Bindings
- **`ash`**: thin, complete and the de-facto standard. It's `unsafe` by nature, which matches
  ADR-0003's plan to keep `unsafe` inside one backend crate.
- **`vulkanalia`**: comparable, but a far smaller community and Apache-2.0 only.
- **`vulkano`**: safe, but it's effectively a second RHI with its own resource and
  synchronization model, which duplicates and constrains ours.
- **`erupt`**: unmaintained since 2022.
- **A git dependency on `ash` main** (for 1.4): unreleased code. `deny.toml` forbids unknown git
  sources, and it can't be published.

### Memory allocation
- **`gpu-allocator`**: pure Rust, no C++ build step, actively maintained, and it also supports
  DirectX 12 and Metal, which matches ADR-0003's future backends.
- **`vk-mem` (VMA)**: an industry-standard allocator, but it needs a C++ toolchain at build time,
  the Rust bindings are small and lightly used, and it is Vulkan only.
- **Our own allocator.** Full control, eventually desirable for GPU-driven rendering, but a large
  correctness surface to write before the first triangle.

## Decision

1. **Baseline: Vulkan 1.3**, for both the instance API version and the device API version. The
   device must also support:
   - The features Vulkan 1.3 makes mandatory, which we rely on: **dynamic rendering**,
     **synchronization2** and **maintenance4**. No `VkRenderPass` or `VkFramebuffer` objects
     anywhere.
   - The remaining required features (timeline semaphores, buffer device address,
     descriptor-indexing subset) are **listed and finalized in the RHI bind-model ADR** before
     they are used, and checked against gpuinfo.org data at that point.
   - Device selection **rejects** a GPU that lacks a required feature with an actionable error
     naming the missing feature, the GPU and its driver version. It never crashes later.
   - **Vulkan 1.4 features** may be used *opportunistically* behind runtime feature checks, with
     a 1.3 fallback, once released `ash` exposes them. **Raising the baseline to 1.4 needs a new
     ADR**, once released bindings exist and driver coverage, including the CI software
     rasterizer, has been verified.
2. **Bindings: `ash` 0.38 from crates.io** with `default-features = false` and features
   `loaded`, `debug` and `std`. The Vulkan loader is loaded at runtime, so a missing Vulkan
   runtime becomes a clear error ("Vulkan runtime not found: install or update the GPU driver"),
   not a link or startup failure. All `ash` usage stays inside `oxreal-rhi-vulkan`.
3. **Surfaces:** `ash-window` 0.13 creates surfaces from the `raw-window-handle` 0.6 handles that
   the platform layer provides. That covers Win32, Wayland, Xlib and XCB, so Linux supports both
   Wayland and X11 (ADR-0002).
4. **Memory: `gpu-allocator` 0.28** with `default-features = false` and features `std` and
   `vulkan`.
   - It sits behind the RHI's own memory module, so it can be replaced or extended (for example
     with engine-owned sub-allocators for transient or GPU-driven resources).
   - Its `log` output is forwarded into our `tracing` pipeline (ADR-0010) by the application's
     subscriber setup.
5. **Validation and debugging:**
   - Debug builds enable `VK_LAYER_KHRONOS_validation` when it's installed and warn when it
     isn't. `OXREAL_VULKAN_VALIDATION=0/1` overrides this.
   - `VK_EXT_debug_utils` names every GPU object and labels command regions (ADR-0003). Its
     messages go to `tracing` with the matching level.
   - In tests, any validation-layer error **fails the test**.
6. **Device selection:** deterministic. Prefer discrete over integrated over software, then the
   largest device-local heap. It can be overridden with `OXREAL_GPU=<index|name substring>` and a
   command-line option, and the chosen device, driver and API version are logged at `info`.
7. **Device loss** (`VK_ERROR_DEVICE_LOST`, surface lost) is a typed RHI error, never a panic, so
   the application can save work and report it (Cordite R8). Full device recreation comes later.
8. **Clip space:** the backend sets a negative viewport height so the engine's Y-up, [0, 1] clip
   convention (ADR-0011) holds. Shaders and matrices never special-case Vulkan.
9. **Supported hardware** is documented in `docs/development/gpu-support.md`. It lists what has
   actually been run and tested (vendor, GPU, driver, OS), not what compiles. CI covers Lavapipe
   (D-19).

## Consequences

- One modern code path with dynamic rendering, synchronization2 and no legacy render passes,
  supported by every target vendor's current drivers and by CI's software rasterizer.
- GPUs whose drivers stop at Vulkan 1.2 are unsupported, as in Cordite. Users get a clear message
  instead of a crash. This probably includes NVIDIA Kepler on its final legacy driver and some
  older integrated GPUs on Windows. That is unverified, and `gpu-support.md` records it once
  confirmed.
- Dependencies: `ash`, `ash-window`, `gpu-allocator` and `raw-window-handle` (each audited in
  `dependencies.md` when `oxreal-rhi-vulkan` is created). `gpu-allocator` also brings `log` and
  `presser`.
- We depend on `ash` publishing releases. Mitigations: the repository is active, `vulkanalia` is
  a fallback, and all `ash` usage is contained in one crate.
- Revisit triggers: `ash` releasing 1.4 bindings (consider raising the baseline), allocator
  fragmentation or performance problems in benchmarks (consider engine-owned sub-allocators), or
  `ash` maintenance stopping (re-evaluate `vulkanalia`).
- D-14's remaining part, **Linux packaging**, stays open.
