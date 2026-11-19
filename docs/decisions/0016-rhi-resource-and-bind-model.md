# ADR-0016: RHI resource model, bindless binding and required GPU features

- **Status:** Accepted
- **Date:** 2026-11-19
- **Related:** ADR-0003 (engine-owned RHI), ADR-0012 (Vulkan 1.3 baseline; this ADR finalizes its required-feature list), ADR-0013 (Slang, capability allowlist), ADR-0011 (clip space), TAD §5.3, D-06 (frame graph), D-19 (GPU CI); prior art: [Cordite](../development/prior-art.md) (R7, R8)

## Context

The RHI is the boundary every renderer pass is written against. Cordite showed what happens
when a third-party RHI's types leak into ~31 renderer headers (its risk R7): a new backend
becomes a rewrite. ADR-0003 therefore requires an engine-owned RHI that also fits DirectX 12
and Metal later.

Three questions shape everything above the RHI:

1. **Resource model:** how passes refer to buffers, textures, samplers and pipelines, and when
   GPU objects are destroyed.
2. **Bind model:** how shaders find their resources. Per-draw descriptor sets or a global
   ("bindless") table indexed by integers.
3. **Required GPU features:** ADR-0012 deferred the final list to this ADR.

### Evidence (checked 2026-11-19)

- **Khronos `VP_KHR_roadmap_2022` profile**, read from the official definition in
  KhronosGroup/Vulkan-Headers, `registry/profiles/VP_KHR_roadmap.json`. It **requires**:
  - `descriptorIndexing`, `runtimeDescriptorArray`, `descriptorBindingPartiallyBound` and
    `descriptorBindingVariableDescriptorCount`
  - update-after-bind for **sampled images, storage images and storage buffers**
  - `descriptorBindingUpdateUnusedWhilePending`
  - non-uniform indexing of sampled image, storage image, storage buffer and uniform buffer
    arrays
  - `scalarBlockLayout` and `samplerAnisotropy`
  - limits of **≥ 500,000** update-after-bind sampled images per stage and per set

  It does **not** require update-after-bind for *uniform* buffers. `bufferDeviceAddress`
  (Vulkan 1.3) and `timelineSemaphore` (Vulkan 1.2) are core requirements.
- **Mesa ANV** (Intel, Linux) doesn't support `descriptorBindingUniformBufferUpdateAfterBind`
  (Mesa issue #5426). This matches the profile: bindless uniform buffers are not portable.
- **Development machine** (AMD RX 9070 XT, Adrenalin, Vulkan 1.4.359): every feature above is
  supported, plus `VK_EXT_descriptor_buffer` and `VK_KHR_push_descriptor`. Bindless limits are
  effectively unbounded, and `maxPushConstantsSize` is 256.
- **Lavapipe** (CI's software Vulkan): public sources didn't state its update-after-bind
  coverage, so this ADR's PR added a CI check (`tools/ci/check-vulkan-features.sh`) and
  **measured it**. **All required features are supported** on:

  | CI image | Lavapipe | Device Vulkan |
  |---|---|---|
  | Ubuntu 24.04 | Mesa 25.2.8, LLVM 20.1.2 | 1.4.318 |
  | Ubuntu 26.04 | Mesa 26.0.8, LLVM 21.1.8 | 1.4.335 |

  The check now runs on every CI run.
- **Cross-API fit:** DirectX 12 Shader Model 6.6 exposes a global `ResourceDescriptorHeap`
  indexed in shaders, and Metal exposes argument buffers. Bindless with integer indices maps
  onto both.
- **Vulkan guarantees** `maxPushConstantsSize ≥ 128` bytes on every conformant device.

## Options considered

### Resource model
- **Reference-counted objects (`Arc<Buffer>`)** are familiar, but destruction timing is implicit
  (the last drop can happen while the GPU still uses the object), and shared mutable state and
  FFI exposure are awkward.
- **Typed handles into device-owned pools** (`BufferHandle`, `TextureHandle`: index plus
  generation) with **deferred destruction** after the frames in flight that may use them.
  Destruction is explicit and safe, handles are `Copy` and C-ABI friendly, and stale handles are
  detectable.

### Bind model
- **Classic descriptor sets per draw or material.** Portable everywhere, but every pass manages
  layouts, pools and set allocation. Many binds per frame, and a poor fit for GPU-driven
  rendering later.
- **Bindless via descriptor indexing.** A few large descriptor arrays (textures, storage
  images, samplers) live in one global set, updated after bind. Shaders index them with
  integers carried in push constants or in buffers. Buffers are reached by **buffer device
  address**. Covered by Roadmap 2022, and the model DirectX 12 SM 6.6 and Metal argument buffers
  converge on.
- **`VK_EXT_descriptor_buffer`.** The most explicit and potentially fastest option, but an
  extension, not core and not part of Roadmap 2022. Possible later as an optimization behind the
  same RHI API.
- **Push descriptors** (core in 1.4, `VK_KHR_push_descriptor` before that). Convenient for small
  per-draw sets, but beyond our 1.3 baseline as a requirement.

## Decision

### 1. Resource model
- The RHI hands out **typed, generational handles** (`BufferHandle`, `TextureHandle`,
  `SamplerHandle`, `PipelineHandle`, ...): `Copy`, `#[repr(transparent)]` over a 64-bit value,
  and never containing backend types.
- The device owns the objects in pools. **Destroying is deferred** until the GPU has finished
  every frame that could still use the object, tracked with a timeline semaphore value per
  submission. Using a destroyed handle is a detected error in debug builds, not undefined
  behaviour.
  - *Implementation note (2026-11-19):* the timeline value alone isn't enough. An object can be
    destroyed while an encoder that uses it is still **recording**, before that submission has a
    timeline value. The Vulkan backend's retire queue therefore also waits for every encoder that
    was open at destruction time to be submitted (then for its value) or dropped. The
    bookkeeping is pure and unit-tested, and a GPU test destroys a pipeline and texture
    mid-recording under validation. The `unsafe` audit then found that queueing an object and
    recording a submission must not interleave. Both happen under the queue lock (lock order
    `queue` → `retired` → `textures`), and readback registers like an encoder.
- Every object takes a **debug name** at creation (ADR-0003, ADR-0010).

### 2. Bind model: bindless
- **One global bindless descriptor set** (set 0) holds runtime arrays, all
  `PARTIALLY_BOUND | UPDATE_AFTER_BIND`:
  - sampled textures (`Texture2D[]`, plus cube and array variants as needed)
  - storage images
  - samplers
- **Buffers are reached by `bufferDeviceAddress`** and passed to shaders as 64-bit addresses.
  There are no bindless *uniform* buffers, avoiding the non-portable feature the ANV and Roadmap
  evidence show.
- Per-draw and per-dispatch data travels in **push constants, kept within 128 bytes**, the
  minimum every conformant device guarantees. Larger data goes in a buffer whose address is in
  the push constants.
- *Implementation note (2026-11-19, buffers):*
  - Buffers live in GPU memory. `write_buffer` stages the data and copies it on the queue, with
    barriers before (earlier use) and after (any later access). Writes therefore take effect in
    **submission order**, and the CPU never writes memory a pending submission may read. A
    host-mapped streaming API can come later, with its own synchronization.
  - Buffer structs use **scalar block layout**, the same as Rust `#[repr(C)]`, which is why
    `scalarBlockLayout` is required. `spirv-val` runs with `--scalar-block-layout` to match.
  - An address is a plain number, so what a shader does with it can't be checked. Using only
    live addresses, in bounds and aligned, is part of `ShaderCode::from_spirv_unchecked`'s
    `unsafe` contract (the shader trust boundary). This is a **program-wide** obligation: push
    constants carrying an address are set through safe calls, so whoever creates `ShaderCode` for
    an address-using shader vouches for every address the program passes it. GPU-side memory
    safety of such shaders is outside what Rust's type system guarantees. "Live" means not
    destroyed before the encoder that uses it was created, matching the retire queue. An opaque
    `BufferAddress` type, or `unsafe` address-carrying push constants, can narrow this later.
  - Shaders may only use what the required features guarantee: no `Int64` (so no 64-bit address
    arithmetic), and no stores or atomics through addresses in vertex or pixel shaders
    (`vertexPipelineStoresAndAtomics` and `fragmentStoresAndAtomics` aren't required). The
    backend rejects SPIR-V capabilities outside the allowlist, the same list the shader compiler
    enforces (a test keeps them equal).
  - `write_buffer` doesn't wait, so a tight loop of large writes can outrun the GPU and grow
    staging memory. A bounded upload ring belongs to streaming work.
  - Synchronization validation can't see accesses through device addresses. The copy barriers
    follow the spec's rules and are checked by pixel and ordering tests, not by the tool.
- A texture or storage image created by the RHI gets a stable **bindless index** for its
  lifetime. Freed indices are reused only after the deferred-destruction point.
- Capacity is sized at device creation and must stay **within the Roadmap 2022 guaranteed
  limits** (≤ 500,000 sampled images). The chosen sizes are logged.
- *Implementation note (2026-11-19, bindless textures and samplers):*
  - Set 0 has two bindings: 0 holds `Texture2D[]` (sampled textures) and 1 holds
    `SamplerState[]`. Storage images join when compute needs them. `oxreal_rhi::BINDLESS_SET`
    and friends declare the layout, and `crates/rendering/rhi/shaders/Bindless.slang` declares
    it for shaders. The backend rejects shaders that declare any other descriptor resource.
  - Capacity is the smaller of 65,536 textures / 2,048 samplers and the device's
    update-after-bind limits. Samplers also stay within `maxSamplerAllocationCount`: every
    existing sampler holds an index, so that limit can't be exceeded.
  - Indices are released only when their object is really destroyed (after the retire queue),
    so a slot is never rewritten while the GPU may read it. A test proves this: releasing early
    makes it fail.
  - A sampled texture **rests** in `SHADER_READ_ONLY_OPTIMAL`. Creation, uploads, render passes
    and readback each return it there in the same command buffer, so shaders can sample it at
    any time. Index validity follows the address rule: a program-wide part of `ShaderCode`'s
    contract.
  - Not tool-verified: core validation skips layout checks for update-after-bind descriptors,
    and GPU-assisted validation (layer 1.4.363) didn't report a removed return-to-rest
    transition either. The transitions follow the spec, and the render-to-texture test checks
    the result.
- Slang shaders access resources through shared engine modules (`common/Bindless.slang`). The
  shader compiler's capability allowlist (ADR-0013) permits exactly the capabilities this model
  needs: `RuntimeDescriptorArray`, `ShaderNonUniform`, the non-uniform indexing capabilities and
  `PhysicalStorageBufferAddresses`.

### 3. Required device features (finalizes ADR-0012)
On top of Vulkan 1.3 core (dynamic rendering, synchronization2, maintenance4,
bufferDeviceAddress, timeline semaphores), Oxreal requires this **subset of Roadmap 2022**:

| Feature | Why |
|---|---|
| `descriptorIndexing`, `runtimeDescriptorArray`, `descriptorBindingPartiallyBound`, `descriptorBindingVariableDescriptorCount` | Global bindless arrays |
| `descriptorBindingSampledImageUpdateAfterBind`, `descriptorBindingStorageImageUpdateAfterBind`, `descriptorBindingUpdateUnusedWhilePending` | Updating the bindless table while frames are in flight |
| `shaderSampledImageArrayNonUniformIndexing`, `shaderStorageImageArrayNonUniformIndexing` | Indexing per pixel or per thread |
| `scalarBlockLayout` | Rust-matching (C-like) buffer layouts without std140 padding |
| `samplerAnisotropy` | Texture quality baseline |

Device selection (ADR-0012 §6) rejects a GPU that lacks any of them, naming the missing feature,
the GPU and its driver. **CI checks this exact list against Lavapipe on every run.**

### 4. Submission and synchronization
- **Two frames in flight**, paced with **one timeline semaphore per queue** (the CPU waits for
  value *N − 2* before reusing frame resources) instead of per-frame fence arrays.
- Barriers use **synchronization2**. In the first RHI version, passes declare resource usage and
  the RHI records the barriers. The frame graph (D-06) takes over barrier planning when it
  exists, without changing pass code.
- A single graphics+compute queue at first. Async compute and transfer queues come when
  profiling justifies them.

### 5. Device loss
`VK_ERROR_DEVICE_LOST` surfaces as a typed RHI error from submit and present (ADR-0012 §7,
Cordite R8). Pools and handles are invalid after it. Recreating the device without a restart
comes later.

## Consequences

- Passes refer to resources by small integers and addresses. Binding is mostly free per draw,
  material systems and GPU-driven rendering have a natural path, and DirectX 12 and Metal
  backends can implement the same model.
- Required features match what desktop drivers built to Roadmap 2022 provide. Older devices
  without descriptor indexing are rejected with a clear message.
- Costs:
  - Bindless shader bugs, such as an out-of-range or stale index, are harder to diagnose. Debug
    builds therefore validate indices on the CPU side, run with GPU-assisted validation
    available, and debug-name every resource.
  - Buffer device addresses need careful lifetime discipline, which the deferred destruction
    covers.
- Follow-ups: `common/Bindless.slang`, the RHI crate (`oxreal-rhi`) and the Vulkan backend
  (`oxreal-rhi-vulkan`), the capability allowlist contents, and GPU tests on Lavapipe (D-19).
- Revisit triggers: Lavapipe or a target driver lacking a listed feature (decided on CI
  evidence), descriptor-update CPU cost showing up in profiles (consider
  `VK_EXT_descriptor_buffer`), or the move to a Vulkan 1.4 baseline (push descriptors).
