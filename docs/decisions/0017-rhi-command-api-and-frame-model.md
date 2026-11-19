# ADR-0017: RHI command API, dispatch and frame model

- **Status:** Accepted
- **Date:** 2026-11-19
- **Related:** ADR-0003 (engine-owned RHI, future DirectX 12 and Metal), ADR-0016 (handles, bindless, two frames in flight), ADR-0015 (windows), TAD §5.3 and §5.8 (C ABI), D-06 (frame graph); prior art: [Cordite](../development/prior-art.md) (R7)

## Context

ADR-0016 fixed the RHI's resource and binding model. Before the first frame is drawn, the RHI's
**call interface** must be fixed too. Every renderer pass, the editor viewport, and later
plugins and the C ABI are written against it:

1. **Dispatch:** how renderer code reaches the backend (Vulkan now, DirectX 12 and Metal later).
2. **Recording:** how commands are recorded and when errors surface.
3. **Frames and presentation:** how a frame begins and ends, and what happens on resize,
   minimize and swapchain invalidation.
4. **Threading:** what may be called from where.

Bindless (ADR-0016) keeps calls per draw low: no descriptor binding, and per-draw data in push
constants. Per-call dispatch overhead is therefore small relative to GPU work, and recording
cost is dominated by the backend's own work.

Unreal's RHI uses virtual dispatch (`FDynamicRHI`, command lists). Cordite exposed a
third-party RHI's concrete types (R7), which made adding a backend a rewrite.

## Options considered

### Dispatch
- **A. Trait objects** (`dyn Device`, `dyn CommandEncoder`). The backend is chosen at runtime,
  which allows falling back from one backend to another at startup and selecting with a
  command-line flag. Renderer code is not generic, compiles once, and maps cleanly to a C ABI
  (handles plus function tables). Costs one indirect call per RHI call, and traits must be
  object-safe (no generic methods).
- **B. Static generics** (`Renderer<B: Backend>` with associated types). Zero dispatch cost.
  But backend generics spread through every renderer and editor type, the backend is fixed at
  compile time, compile times grow, and it's hostile to plugins and a C ABI.
- **C. Enum dispatch over known backends.** One `match` per call. It's a closed set, so plugin
  or out-of-tree backends are impossible, and every new backend edits every call site's enum.

### Recording errors
- **Fallible recording:** every call returns `Result`. Noisy, and most recording "errors" are
  programming errors anyway.
- **Infallible recording with validation:** recording calls return nothing. Misuse is caught by
  debug-build validation (the engine's own checks plus the API's validation layers), and real
  failures (device loss, out of memory) surface at **submit**.

## Decision

1. **Dispatch through trait objects (option A).** `oxreal-rhi` defines object-safe traits, and
   backends implement them. Renderer code holds `Arc<dyn Device>` and records through
   `&mut dyn CommandEncoder`. No generic `Backend` parameter appears in renderer, editor or
   gameplay types. If profiling ever shows dispatch overhead mattering, batching APIs (multi-draw,
   indirect) are added. Dispatch is not made static.
2. **Threading:**
   - `Device` is `Send + Sync`, and its methods take `&self`. Backends synchronize internally:
     queue submission is serialized, and pools are internally locked.
   - Command encoders are `Send` but **not** shared. Each is recorded by one thread at a time,
     which allows parallel recording later.
3. **Recording is infallible.** Encoder methods don't return `Result`. In debug builds, misuse
   such as an invalid handle or a render pass left open is caught by engine validation and
   reported with the call site. Device loss and out-of-memory surface as `RhiError` from
   **submit / end of frame** (ADR-0012 §7).
4. **Render passes are scoped.** Beginning a pass returns a guard borrowing the encoder, and
   the pass ends when the guard is dropped. Commands that only make sense inside a pass (draws)
   exist only on the guard, so "draw outside a pass" can't compile. Passes use dynamic
   rendering (ADR-0012) and describe attachments with load and store operations and clear
   values.
5. **Frame model:**
   - A window's presentation target is a **surface**, created from `oxreal-window`'s raw handles
     and owned by the device as a handle (ADR-0016 §1).
   - `begin_frame(surface)` returns **`Option<Frame>`**. It's `None` when nothing should be
     drawn: minimized (zero size), or the swapchain is being recreated. Callers skip the frame
     rather than handle an error.
   - A `Frame` carries the backbuffer's texture handle and the encoder for the frame.
     `end_frame(frame)` submits and presents.
   - **Swapchain recreation is internal:** on resize, out-of-date or suboptimal results the
     backend recreates the swapchain at the next `begin_frame`. Applications only report the new
     size from `WindowEvent::Resized`.
   - Two frames in flight, paced by a timeline semaphore (ADR-0016 §4). Per-frame resources are
     reused only after the GPU has finished that frame.
6. **Presentation settings:** present mode `Fifo` (vsync, the default and always available),
   `Mailbox` (low latency, falls back to `Fifo` if unsupported, with the fallback logged) or
   `Immediate`. The swapchain format is an **sRGB** 8-bit BGRA/RGBA format, so shaders write
   linear color and the hardware encodes (ADR-0011 §5).
7. **Clear values and colors are linear** (`LinearRgba` from `oxreal-math`).

### Implementation notes (2026-11-19, from the `unsafe` audit of the first draw call)

The audit found that several guarantees above held only for well-behaved callers. The
backend now enforces them, so safe code can't reach undefined behaviour:

- **Guards don't prove anything (§4).** `std::mem::forget` is safe, so a `RenderPass` guard can
  be leaked. Submission closes any open pass and reports it as misuse.
- **One open frame per surface (§5).** A second `begin_frame` and `destroy_surface` are refused
  while a frame is open. `end_frame` takes the surface, image and slot from the frame's own
  encoder, never from `Frame`'s public parts. A dropped frame is reported. The next
  `begin_frame` consumes its acquire semaphore with an empty submission and recreates the
  swapchain, which releases the image. Any failure after an image is acquired is handled the
  same way. Only the frame's encoder may use its backbuffer.
- **Encoders keep the device alive.** The device's state is shared (`Arc`) with every encoder,
  so an encoder that outlives the `Device` still records into live objects. Destroyed surfaces
  wait in the retire queue for frame encoders that still use them.
- **Resource state follows submission order.** An encoder claims every texture it records
  commands for. No other unsubmitted encoder may use a claimed texture. A dropped encoder
  restores the layouts it changed. Precise per-resource tracking arrives with the frame graph.
- **Draws are validated before recording:** pipeline and pass formats match, every
  push-constant byte is written, and attachments are unique and within device limits.
- **Shader code is a trust boundary.** `ShaderCode::from_spirv_unchecked` is `unsafe`: the caller
  promises valid SPIR-V (`spirv-val --target-env vulkan1.3`). The backend still checks the
  header, instruction framing, entry points and push-constant size before the driver sees it.

A second audit added:
- an encoder is refused by any device other than the one that created it;
- `vkDeviceWaitIdle` runs under the queue lock (Vulkan requires every queue to be externally
  synchronized during it);
- a frame is checked before it changes any state;
- a failed acquire release or swapchain creation leaves nothing signalled or retired.

**Known limitations,** to be addressed with the frame graph, which tracks per-resource usage:
- Destroying an object waits for *every* encoder open at that moment, not only those that use
  it. A long-lived or leaked (`mem::forget`) unsubmitted encoder therefore delays all later
  destruction until it is submitted or dropped, or until the device is dropped. This is
  correct but conservative.
- `begin_frame` holds the surface table across GPU waits (frame pacing, image acquisition,
  swapchain recreation), and `end_frame` holds the queue lock across `vkQueuePresentKHR`. There
  is no lock cycle, so this can't deadlock. While one surface waits, though, other surfaces and
  submissions on the device wait too.

Regression tests: `oxreal-rhi-vulkan` `tests/triangle.rs` and `examples/frame_protocol.rs`. CI
runs them with synchronization validation on (`OXREAL_VULKAN_SYNC_VALIDATION=1`, requested through
`VkValidationFeaturesEXT`; device creation fails if the layer can't provide it).

### Implementation notes (2026-11-19, depth attachments)

Render passes take an optional depth attachment (`RenderPassDesc::depth_attachment`), and
pipelines declare matching depth testing (`RenderPipelineDesc::depth`). The scene convention is
ADR-0011 §4's: `DepthState::SCENE`, cleared to `REVERSED_DEPTH_CLEAR`.

- **Formats and usage:** `TextureFormat::Depth32Float` with `TextureUsage::DEPTH_TARGET` only.
  Sampling or copying depth comes when a feature needs it (shadow maps, depth readback). The spec
  doesn't guarantee D32_SFLOAT depth attachments, so adapter selection requires them.
- **Matching:** a pipeline's depth format must equal the pass's (both `None`, or the same
  format), as dynamic rendering requires without `dynamicRenderingUnusedAttachments`. Mismatches,
  wrong usage and depth clears outside [0, 1] are misuse, reported at submit.
- **Depth-only passes** (no color attachments) and **depth-only pipelines** (no color formats)
  are allowed, for prepasses and later shadow maps. A pipeline still has a pixel stage. Use one
  without color outputs.
- **Layouts and barriers:** the attachment uses DEPTH_STENCIL_ATTACHMENT_OPTIMAL, valid for
  depth-only formats without `separateDepthStencilLayouts`. It stays in that layout after the
  pass. The begin barrier waits for all earlier work (ALL_COMMANDS) before the early and late
  fragment tests, like color attachments, and `Clear` or `DontCare` start from UNDEFINED.
- **The render area** is the smallest extent of all attachments, color and depth.
- **Frames** report the backbuffer's size (`Frame::backbuffer_size`), for sizing depth buffers
  and the camera's aspect ratio.

Tested by `tests/depth.rs`: occlusion in either draw order, camera placement, a depth prepass
loaded within one encoder and across submissions, and misuse. Deliberately broken versions
showed:
- switching the depth test off, or making the cube's winding inside out, fails the tests;
- starting a `Load` from UNDEFINED instead of the tracked layout doesn't. The validation layers
  accept it, and this GPU kept the contents anyway. The same gap exists for color `Load`.

*After the `unsafe` audit:*
- `write_texture` and `read_texture` refuse depth textures themselves, because their copies use
  the color aspect. Today the usage rules already keep depth textures out.
- Attachment textures are checked against `maxFramebufferWidth` and `maxFramebufferHeight`,
  which may be lower than `maxImageDimension2D`. Backbuffers take the surface's size.
- A pass changes tracked layouts only once all of its barriers are built.
- A test drops an encoder after a depth pass and then loads the depth in a new one. Breaking the
  layout revert for abandoned encoders makes the validation layers fail it.

## Consequences

- One renderer code path for every backend, runtime backend choice, and a straightforward C ABI
  later.
- Renderer code reads like Unreal's RHI usage: get a frame, record passes, end the frame.
- Recording stays ergonomic. Errors appear where they're actionable (submit), and validation
  catches misuse in development.
- Minimize, resize and swapchain invalidation are handled once in the backend, not in every
  application.
- Costs: one indirect call per RHI call, and object-safe traits only (generic conveniences
  become helper functions over the traits).
- Revisit triggers: profiling showing RHI dispatch as a measurable share of frame CPU time in
  the benchmark projects (add batching first), or parallel recording needing a different
  encoder ownership model.
