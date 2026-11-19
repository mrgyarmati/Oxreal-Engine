# Technical Architecture Document

> **Status:** Proposed v0.1. This needs review before engine crates are created.
> Related: [PRD](../product/PRD.md), [ADRs](../decisions/README.md), [open decisions](../development/open-decisions.md)

## 1. Shape

The project is a **modular monolith**: one repository, one Cargo workspace, and many small crates
that can be tested independently. Dependency boundaries are enforced mechanically
(`cargo xtask layers`). There are no microservices and no `engine` god-crate.

## 2. Layers

| Layer id (`metadata.engine.layer`) | Responsibility | Directory |
|---|---|---|
| `foundation` | math, memory, reflection, serialization, diagnostics, core types | `crates/foundation/*` |
| `platform` | windowing, filesystem, threads, devices, OS services (Windows/Linux impls behind traits) | `crates/platform/*` |
| `runtime` | ECS, scheduler, events, jobs, time, hierarchy, resource lifetime | `crates/runtime/*` |
| `rendering` | RHI, Vulkan backend, frame graph, shader compiler, renderer, materials | `crates/rendering/*` |
| `simulation` | physics abstraction and backends, animation, navigation, particles | `crates/simulation/*` |
| `systems` | input, audio, networking, runtime UI | `crates/systems/*` |
| `assets` | asset database, importers, formats, cooker, streaming | `crates/assets/*` |
| `gameplay` | gameplay framework, character, camera, visual-graph IR and VM, render-world extraction glue | `crates/gameplay/*` |
| `editor` | editor framework (docking, commands, selection, undo, gizmos) and panels | `crates/editor/*` |
| `app-runtime` | shipped executables without the editor: player, dedicated server | `applications/{player,server}` |
| `app-editor` | the editor executable | `applications/editor` |
| `tool` | offline tools, tool libraries and dev tooling: shader compiler, asset-cooker, shader-tool, xtask | `crates/tools/*`, `applications/{asset-cooker,shader-tool}`, `tools/*` |

## 3. Dependency direction

```text
 app-editor ─────► editor ─────────────┐
     │                                  ▼
 app-runtime ──────────────────────► gameplay
 tool ─────────────────────────────►    │
                     ┌────────┬─────────┼──────────┬──────────┐
                     ▼        ▼         ▼          ▼          ▼
                 rendering simulation systems    assets       │
                     │        │         │          │          │
                     └────────┴────┬────┴──────────┘          │
                                   ▼                          │
                                runtime ◄─────────────────────┘
                                   │
                                   ▼
                                platform
                                   │
                                   ▼
                               foundation
```

## 4. Allowed dependencies (enforced by `cargo xtask layers`)

| Layer | May depend on (workspace crates) |
|---|---|
| foundation | foundation |
| platform | foundation, platform |
| runtime | foundation, platform, runtime |
| rendering | foundation, platform, runtime, rendering |
| simulation | foundation, runtime, simulation |
| systems | foundation, platform, runtime, systems |
| assets | foundation, platform, runtime, assets |
| gameplay | all of the above, gameplay |
| editor | all of the above, editor |
| app-runtime | everything except editor and app-* |
| app-editor | everything except app-* |
| tool | everything except editor and app-* |

Rules this table encodes:

- **Runtime never depends on editor.** Player and server builds can't reach editor code.
- **Rendering, simulation, systems and assets are siblings.** They don't depend on each other.
  Cross-cutting glue such as render-world extraction (ECS to render scene), physics-to-transform
  sync and asset-to-GPU upload lives in `gameplay`. Otherwise it lives in a dedicated bridge crate
  at the gameplay layer.
- **Platform-specific code lives only in `platform`.** Other layers depend on platform *traits*.
- Dev-dependencies are exempt so tests can use helpers, but nothing may dev-depend on `editor`
  from below the editor layer.

Changing this table requires an ADR.

## 5. Key boundaries

### 5.1 ECS vs authoring hierarchy (ADR-0005, ADR-0014)

- The **ECS** (`runtime/ecs`) holds runtime entities, components, queries and systems. The public
  API exposes no storage types. Storage is archetype tables, with sparse sets as an
  evidence-gated opt-in. Entities are 64-bit generational handles, and relations are components
  with automatically maintained inverses. The ECS also provides lifecycle hooks, deferred
  commands applied at stage sync points, change detection, and static plus dynamic (by
  `ComponentId`) queries (ADR-0014).
- The **hierarchy** (`runtime/hierarchy`) holds authoring parent/child relationships (the ordered
  `ChildOf`/`Children` relation), local and world transforms, and propagation.
- Sync rules: the hierarchy is authoritative for parentage. Transform propagation runs as an
  explicit scheduled system at a defined stage (after gameplay and physics write-back, before
  render extraction). World transforms are derived data and are never authored directly.

### 5.2 Renderer does not own the scene

The frame flow is:
1. The simulation world updates.
2. **Extract**: a dedicated stage copies render-relevant data into a `RenderWorld` snapshot.
3. The renderer prepares, builds the frame graph, records and submits.

The renderer never mutates the game world. The editor viewport is just another consumer of the
same extraction path.

### 5.3 RHI

- `rendering/rhi` defines backend-agnostic traits and handles: device, queues, buffers, textures,
  samplers, pipelines, descriptor or bind model, command encoding, sync, and swapchain.
- `rendering/backends/vulkan` is the first implementation. No Vulkan types appear in the RHI API.
- The window surface is created from raw window and display handles that `platform` provides, so
  the renderer stays independent of the windowing layer.
- Vulkan 1.3 baseline, `ash` bindings and `gpu-allocator` are decided in ADR-0012, all confined to
  `oxreal-rhi-vulkan`. Shaders are Slang, compiled by the pinned `slangc` CLI (ADR-0013).
- **Resource and bind model (ADR-0016):**
  - typed generational handles into device pools, with deferred destruction
  - **bindless**: one global descriptor set of runtime arrays for textures, storage images and
    samplers; buffers by device address; per-draw data in at most 128 bytes of push constants
  - two frames in flight, paced by timeline semaphores
  - the required features are listed in `tools/ci/required-vulkan-features.txt`
- **Command API and frames (ADR-0017):**
  - trait-object dispatch (`Arc<dyn Device>`, `&mut dyn CommandEncoder`)
  - `Device` is `Send + Sync`, encoders are recorded by one thread at a time
  - recording is infallible, and errors surface at submit
  - render passes are scoped guards
  - `begin_frame(surface)` returns `Option<Frame>` (`None` when minimized); swapchain
    recreation happens inside the backend

### 5.4 Reflection (foundation, early)

Reflection underpins the Details panel, serialization, undo/redo, visual scripting and runtime
debugging. It lives in `foundation/reflection` with a derive macro (`foundation/reflection-derive`).
The design is open (D-02) but must be decided before M2.

### 5.5 Assets (ADR-0006)

- Every asset has a stable `AssetId` (128-bit, generated once and persisted in a sidecar or
  metadata). It is never derived from the path.
- Import records hold the source path and hash, importer id and version, settings, and output
  artifacts with their dependencies.
- User overrides are stored separately from imported data and applied after import, so reimport
  never destroys them.
- The derived data cache is keyed by (source hash, importer version, settings hash, platform).

### 5.6 Gameplay framework (ADR-0008, proposed)

- Unreal concepts (Actor, Pawn, Character, Controller, GameMode, GameState, PlayerState,
  Subsystems, Hud) are **roles built from ECS components**, not classes. There is no `A`/`U` prefix.
- Two API levels share one data model: plain **systems** for performance-critical code, and
  per-actor **Behaviours** (`begin_play`/`tick`/`end_play`) for Unreal-style logic. Visual
  scripts implement the same Behaviour contract.
- **Actor templates** (prefabs) play the role of Blueprint classes. Instance overrides reuse the
  ADR-0006 mechanism.
- Unreal tick groups map to schedule stages (ADR-0014 §5):
  `Input → PrePhysics → DuringPhysics → PostPhysics → PostUpdate → Extract`.
  Gameplay runs in `PrePhysics` by default, like Unreal's default actor tick group.

### 5.7 Editor

- The editor framework (commands, selection, undo/redo transactions, extension points) is
  UI-toolkit-agnostic. Panels sit behind a thin UI adapter, so egui is replaceable.
- All editor mutations go through **commands**. That gives undo/redo, scripting and automation
  (AI-automatable, PRD §2.10) the same entry point.

### 5.8 Additional language support: C++ and C# (future, D-21)

Implementation comes later, after the Rust gameplay API stabilizes. The constraints below apply
**now**, so that earlier decisions don't rule it out.

**Intended shape (to be decided in an ADR):**
- A versioned **stable C ABI** layer exposes engine services to other languages: opaque handles,
  plain-data structs, explicit ownership, error codes and no unwinding across the boundary. It is
  generated from reflection metadata where possible and never hand-maintained per language.
- **C++** gets generated headers with a thin RAII wrapper over the C ABI. There is no direct
  binding to Rust types.
- **C#** uses .NET hosting (embedding the runtime, with NativeAOT as an option) with generated
  P/Invoke bindings. Options are evaluated in the ADR.
- Foreign-language gameplay code plugs into the same contracts as Rust: Behaviours (ADR-0008),
  systems with declared component access, and events. No parallel gameplay framework.

**Constraints on decisions made before then:**
1. **Reflection (D-02)** must produce language-neutral type metadata: stable type IDs, field
   layouts, names, attributes and function signatures that tools can export. Metadata must not
   exist only as Rust trait magic or generics.
2. **Plugin ABI (D-13)** and **hot reload (D-11)** must treat the C ABI as the stable boundary,
   with Rust plugins as one client of it.
3. **Public engine services** (world access, spawning, events, assets, input) must be expressible
   as handles plus plain data. Gameplay-facing APIs must not *require* Rust-only constructs such
   as borrowed closures across frames or generic-only entry points.
4. **Memory and threading rules** at the boundary must be explicit: who owns what, which threads
   may call what, and how GC-managed C# objects reference entities (by handle, never by pointer).
5. A future interop layer must not compromise editor/runtime separation. Its placement in the
   layer matrix (TAD §4) is decided by the D-21 ADR.

## 6. Initial interfaces to document before M1 (sketch, non-binding)

```rust
// platform
pub trait Window { fn raw_handles(&self) -> RawHandles; fn inner_size(&self) -> (u32, u32); }
pub trait EventLoop { fn run(self, app: impl App) -> Result<(), PlatformError>; }

// rhi
pub trait Device {
    fn create_buffer(&self, desc: &BufferDesc) -> Result<BufferHandle, RhiError>;
    fn create_texture(&self, desc: &TextureDesc) -> Result<TextureHandle, RhiError>;
    fn create_swapchain(&self, surface: &RawHandles, desc: &SwapchainDesc) -> Result<SwapchainHandle, RhiError>;
    fn begin_frame(&self) -> Result<FrameContext, RhiError>;
    fn submit(&self, frame: FrameContext) -> Result<(), RhiError>;
}

// runtime
pub struct World { /* entities, components, resources */ }
pub trait System { fn run(&mut self, world: &mut World); }
pub struct Schedule { /* ordered stages (ADR-0014): Input, PrePhysics, DuringPhysics (fixed-step), PostPhysics, PostUpdate (transform propagation), Extract */ }
```

## 7. Minimal workspace proposal for M1

Create these only when M1 work starts, one at a time, each with real code:

| Crate | Layer | Purpose |
|---|---|---|
| `foundation/math` | foundation | **Created** (`oxreal-math`): re-exported glam types plus owned `Transform`, projections and color. Bounds and frustum come with culling. (ADR-0011) |
| `foundation/diagnostics` | foundation | **Created** (`oxreal-diagnostics`): re-exported log macros, `profile_*` macros, `init_application_logging` (ADR-0010) |
| `platform/window` | platform | **Created** (`oxreal-window`): windows and the event loop, winit 0.30 behind an engine-owned `App` trait and event enum. Raw handles via `raw-window-handle` 0.6 (ADR-0015). |
| `runtime/ecs` | runtime | Minimal ECS |
| `rendering/rhi` | rendering | **Created** (`oxreal-rhi`): `Device` trait, adapter selection, generational handles, surfaces, frames, command encoder and scoped render passes, textures, buffers (GPU addresses, ordered writes, readback), render pipelines (push constants), draws, standalone submission and readback, `RhiError`. Bindless textures come next. (ADR-0016, ADR-0017) |
| `rendering/backends/vulkan` | rendering | **Created** (`oxreal-rhi-vulkan`): instance, validation, GPU selection, device, allocator, surfaces and swapchains with internal recreation, timeline-paced frame loop, dynamic rendering, textures, buffers with device addresses and staged writes, pipelines with a Y-up viewport, deferred destruction, readback, SPIR-V checks, pixel tests. (ADR-0011, ADR-0012, ADR-0016, ADR-0017) |
| `rendering/renderer` | rendering | Basic mesh pass, camera |
| `applications/player` | app-runtime | The vertical-slice app |
| `tools/shader-compiler` | tool | **Created** (`oxreal-shader-compiler`): pinned `slangc`, reflection, depfile, SPIR-V capability allowlist, optional `spirv-val`. Never linked into games. (ADR-0013) |

Crate names use the `oxreal-` prefix, for example `oxreal-math` and `oxreal-rhi` (D-15).
