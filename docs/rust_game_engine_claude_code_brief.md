# Rust-Native Game Engine --- Project Brief for Claude Code

> **Working title:** TBD\
> **Status:** Product direction established; implementation architecture
> still requires validation\
> **Primary objective:** Build a new, independent, Rust-native game
> development platform with Unreal Engine-class ambitions and an editor
> experience familiar to Unreal developers.

This document is the initial source of truth for Claude Code when
exploring, scaffolding, implementing, and documenting the project. Treat
the requirements below as product constraints. Do not silently weaken
them or introduce an existing game engine as the foundation.

------------------------------------------------------------------------

## 1. Product vision

Build a complete, open-source, professional 3D game development
ecosystem written primarily in Rust.

The product should let a developer create, edit, test, profile, package,
and distribute games without depending on Unreal Engine, Unity, Godot,
or another engine underneath it.

The engine is a **game development product**, not a general-purpose 3D
content creation or visualization suite.

### Core product principles

1.  **Greenfield engine:** Own the engine architecture and core systems.
    Do not fork Bevy, Fyrox, Godot, or another engine.
2.  **Rust-native:** Rust is the primary implementation language for
    engine, editor, tools, and runtime.
3.  **Windows and Linux first-class:** Both platforms are required from
    the beginning, not treated as a later port.
4.  **Unreal-familiar UX:** Existing Unreal Engine developers should
    recognize the editor layout, terminology, interaction patterns, and
    common workflows.
5.  **Game-focused:** Prioritize real-time games, gameplay systems,
    characters, worlds, multiplayer, and game production workflows.
6.  **Modular architecture:** Subsystems have clear boundaries and
    replaceable implementations.
7.  **Editor/runtime separation:** The runtime must not depend on editor
    code.
8.  **Open and extensible:** Documented APIs, plugin interfaces,
    automation, and community contributions.
9.  **Production-minded:** Testing, profiling, diagnostics, reproducible
    builds, and asset cooking are foundational concerns.
10. **AI-automatable:** Important editor and project operations should
    eventually be exposed through structured commands and APIs.

### Product positioning

**Unreal-level ambition, Blender-level openness, Rust-level engineering
discipline.**

This is an ambition and long-term benchmark, not a claim of current
feature parity.

------------------------------------------------------------------------

## 2. Target users and scope

### Primary users

-   Unreal Engine developers considering an alternative.
-   Indie and AA game studios.
-   Gameplay programmers.
-   Technical artists.
-   Game designers.
-   Small teams building 3D games.
-   Developers who want a native Linux game development workflow.

### In scope

-   Real-time 3D game development.
-   First-person and third-person games.
-   3D platformers, action games, RPGs, shooters, simulation games, and
    multiplayer games.
-   Game worlds, levels, streaming, terrain, foliage, and instancing.
-   Character controllers, animation, physics, AI, navigation, audio,
    and VFX.
-   Gameplay programming in Rust and visual scripting.
-   Game UI and HUD authoring.
-   Cinematics and cutscenes when used in games.
-   Asset import, reimport, cooking, packaging, profiling, and
    deployment.
-   Dedicated server and multiplayer workflows.

### Explicitly not a product priority

Do not build the roadmap around:

-   Architectural visualization (ArchViz).
-   Film and television production.
-   Virtual production.
-   CAD or engineering visualization.
-   General-purpose 3D modelling.
-   Motion graphics.
-   A replacement for Blender, Maya, or other DCC applications.

Support features such as cameras, high-quality rendering, animation, and
timelines when they materially support games. Do not expand them into
separate film or visualization products.

------------------------------------------------------------------------

## 3. Platform requirements

### Day-one platforms

  -----------------------------------------------------------------------
  Platform                            Requirement
  ----------------------------------- -----------------------------------
  Windows 10/11 x64                   First-class editor, runtime, tools,
                                      builds, and tests

  Linux x86_64                        First-class editor, runtime, tools,
                                      builds, and tests
  -----------------------------------------------------------------------

Linux must not be a build-only target. The editor, asset pipeline,
debugging, profiling, packaging, and runtime must work on Linux.

### Linux distribution strategy

Initial validation targets:

-   Ubuntu LTS.
-   Fedora.
-   Arch Linux.

Additional distribution targets can follow, including Debian and
SteamOS.

Provide practical native distribution options. Evaluate portable
archives and Flatpak, but do not make sandboxed distribution mandatory
for developers who need access to SDKs, external tools, source assets,
and build systems.

### Graphics strategy

-   Design an engine-owned Rendering Hardware Interface (RHI).
-   Implement Vulkan as the first graphics backend.
-   Support Vulkan on Windows and Linux.
-   Keep the renderer independent from the platform windowing layer.
-   Design the RHI so future DirectX 12 and Metal backends are possible.
-   Do not assume Vulkan removes the need for platform-specific driver
    and hardware testing.

### Hardware validation

Plan real hardware testing across:

-   NVIDIA GPUs.
-   AMD GPUs.
-   Intel GPUs where supported.

CI should run Windows and Linux builds and automated tests on every
significant change. Hardware-backed rendering tests should be added as
infrastructure becomes available.

### Future platforms

macOS and console platforms are future considerations. Do not let them
block the first Windows/Linux vertical slice. Console support requires
platform-holder access, SDK agreements, and compliance work; do not
promise it without those dependencies.

------------------------------------------------------------------------

## 4. Unreal-familiar editor experience

Unreal familiarity is a product requirement, not a cosmetic preference.

An Unreal developer should be able to navigate the editor without
learning a completely new interaction model.

### Familiar terminology

Prefer established terms when appropriate:

  Our editor                              Familiar reference
  --------------------------------------- ---------------------
  Viewport                                Viewport
  World Outliner                          World Outliner
  Details Panel                           Details
  Content Browser                         Content Browser
  World Settings                          World Settings
  Project Settings                        Project Settings
  Play in Editor                          Play in Editor
  Blueprint Editor                        Blueprint Editor
  Material Editor                         Material Editor
  Animation Blueprint / Animation Graph   Animation Blueprint
  Output Log                              Output Log
  Sequencer                               Sequencer

These are UX references, not requirements to copy Unreal's internal
implementation or proprietary technology.

### Familiar interaction patterns

Plan for:

-   W/E/R translate, rotate, and scale shortcuts.
-   Familiar transform gizmos.
-   Right-mouse viewport navigation.
-   Perspective and orthographic views.
-   Drag-and-drop asset placement.
-   Double-click asset editing.
-   Context menus.
-   Multi-selection.
-   Hierarchy parenting.
-   Editable property fields.
-   Undo and redo.
-   Play, pause, and simulate controls.
-   Searchable commands and keyboard shortcuts.
-   Customizable editor layouts and key bindings.

### Default editor layout

The initial editor layout should include:

-   Main 3D viewport.
-   World Outliner.
-   Details panel.
-   Content Browser.
-   Toolbar with save, play, pause, and simulation controls.
-   Output Log.
-   Project and world settings access.

The editor UI implementation is replaceable. Do not let a temporary UI
library define engine architecture.

------------------------------------------------------------------------

## 5. High-level architecture

Use a modular monolith initially: one repository and a coherent Cargo
workspace, with independently testable crates and strict dependency
boundaries. Do not introduce microservices.

### Architectural layers

1.  **Editor application**
    -   Panels, viewport, outliner, inspector, content browser, graph
        editors, debugging UI.
2.  **Editor framework**
    -   Docking, commands, selection, undo/redo, gizmos, extension
        points, editor state.
3.  **Engine framework**
    -   Worlds, scenes, assets, components, materials, animation,
        gameplay, level management.
4.  **Runtime**
    -   ECS, scheduling, events, jobs, time, reflection, serialization,
        resource lifetime.
5.  **Rendering**
    -   RHI, frame graph, GPU resources, shader compilation, render
        scene, materials, lighting.
6.  **Simulation**
    -   Physics, character movement, animation, navigation, particles.
7.  **Game services**
    -   Input, audio, networking, UI, platform services.
8.  **Platform layer**
    -   Windows and Linux windowing, filesystems, threading, devices,
        packaging.

### Important separation rules

-   Runtime must never depend on editor crates.
-   Renderer must not own canonical scene data.
-   Asset database must be independent of the viewport.
-   Editor plugins must not require modifying engine source.
-   Player builds must exclude editor-only dependencies and
    functionality.
-   Platform-specific code must be isolated behind platform interfaces.
-   The renderer should consume extracted render-world data rather than
    directly owning the editable scene.
-   ECS runtime storage and authoring hierarchy are distinct concepts.
-   Do not create a monolithic `engine` crate that contains every
    subsystem.

### ECS and scene hierarchy

Build an engine-owned ECS API and architecture. Do not adopt Bevy ECS as
the engine's permanent public foundation.

The authoring hierarchy provides parent/child relationships and
transform parenting. ECS manages runtime entities, components, queries,
and systems. Define the synchronization and transform propagation rules
explicitly.

### Reflection

Runtime reflection is foundational. Design it early because the
following depend on it:

-   Details panel and property inspection.
-   Serialization.
-   Asset and component editing.
-   Visual scripting.
-   Runtime debugging.
-   Undo/redo.
-   Editor tooling.

Do not postpone reflection and then bolt it onto every subsystem later.

------------------------------------------------------------------------

## 6. Rendering architecture

Rendering is a major technical differentiator and a long-term
investment.

### Required conceptual subsystems

-   Rendering Hardware Interface (RHI).
-   GPU device and resource management.
-   Frame graph / render graph.
-   Shader compiler and shader reflection.
-   Render-world extraction.
-   Geometry and mesh processing.
-   Material system.
-   Lighting and shadows.
-   Visibility and occlusion.
-   Post-processing.
-   GPU profiling and diagnostics.

### Initial renderer goals

-   Vulkan backend.
-   Basic mesh rendering.
-   Camera and transforms.
-   PBR materials.
-   HDR pipeline and tone mapping.
-   Image-based lighting.
-   Directional, point, and spot lights.
-   Shadow mapping.
-   Basic post-processing.
-   Render diagnostics.

### Longer-term rendering roadmap

  Capability                    Target stage
  ----------------------------- --------------------------------------------
  PBR deferred rendering        Early usable engine
  HDR and tone mapping          Early usable engine
  Image-based lighting          Early usable engine
  Cascaded shadows              Early usable engine
  Temporal anti-aliasing        Later
  GPU-driven rendering          Later
  Clustered lighting            Later
  Virtualized geometry          Advanced
  Dynamic global illumination   Advanced
  Hardware ray tracing          Advanced
  Path tracing                  Low priority for this game-focused product

Do not attempt a Nanite or Lumen equivalent before the renderer has a
stable resource model, frame graph, shader pipeline, profiling, and
baseline performance.

The goal is to solve the underlying game-rendering problems, not to
reproduce Unreal feature names.

------------------------------------------------------------------------

## 7. Runtime and game systems

Game systems are core engine capabilities, not optional afterthoughts.

### Core runtime

-   ECS.
-   System scheduler.
-   Events and messaging.
-   Time and fixed timestep.
-   Job system.
-   Reflection.
-   Serialization.
-   Resource lifetime and memory management.
-   Input abstraction.
-   Runtime diagnostics.

### Gameplay framework

-   Rust gameplay modules.
-   Components and systems.
-   Gameplay events.
-   Input mapping.
-   Character framework.
-   First-person controller.
-   Third-person controller.
-   Camera framework.
-   Interaction framework.
-   Save/load support.
-   Game mode / game state concepts where useful.

### Physics and simulation

Start with a replaceable physics backend. Rapier is a candidate for the
initial integration; evaluate Jolt as an alternative against actual game
requirements.

Required eventual capabilities:

-   Rigid bodies.
-   Collision detection.
-   Character controllers.
-   Queries and traces.
-   Constraints and joints.
-   Ragdolls.
-   Vehicles.
-   Cloth and destruction where justified.

Do not create an engine-wide dependency on a specific physics vendor or
library.

### Character animation

-   Skeletal meshes.
-   Skeleton and skinning.
-   Animation clips.
-   State machines.
-   Blend spaces.
-   Animation graphs.
-   IK.
-   Retargeting.
-   Root motion.
-   Animation events.
-   Motion matching as a future research area.

### Game AI and navigation

-   Navigation meshes.
-   Pathfinding.
-   AI agents.
-   Perception.
-   State machines.
-   Behavior trees.
-   Utility AI.
-   Debug visualization.

### Networking

Design for multiplayer early, even if the complete multiplayer feature
set is delivered later.

Plan for:

-   Authority model.
-   Replication.
-   RPC.
-   Network identity.
-   Prediction and reconciliation.
-   State synchronization.
-   Dedicated servers.
-   Network profiling and debugging.

### Audio

-   Spatial audio.
-   Mixer and routing.
-   Audio events.
-   Attenuation.
-   Reverb and effects.
-   Audio graph as a later editor feature.

### VFX

-   CPU and GPU particles.
-   Emitters.
-   Trails.
-   Collision.
-   Gameplay-driven events.
-   Graph-based authoring.

### UI

-   Runtime UI system.
-   HUD and menus.
-   Layout and anchoring.
-   Input and focus.
-   Animation and transitions.
-   Visual UI designer.

------------------------------------------------------------------------

## 8. Asset pipeline and Blender interoperability

The asset pipeline is a core product feature.

### Initial formats

  Asset               Formats
  ------------------- ----------------------------------
  Models and scenes   glTF, GLB
  Models              FBX through a dedicated importer
  Scene interchange   USD as a later integration
  Textures            PNG, JPEG, TIFF, EXR, HDR, KTX2
  Audio               WAV, OGG, FLAC
  Fonts               TTF, OTF

Treat glTF/GLB as the first-class initial interchange format. FBX and
USD support must be evaluated and implemented through dedicated
importers; do not assume format support is trivial or complete.

### Asset pipeline capabilities

1.  Source asset discovery.
2.  Import and validation.
3.  Coordinate, scale, and axis conversion.
4.  Mesh and skeleton processing.
5.  Material translation.
6.  Texture processing.
7.  Stable asset identifiers.
8.  Dependency tracking.
9.  Derived data cache.
10. Reimport and change detection.
11. Hot reload.
12. Asset cooking and packaging.

### Non-destructive reimport

This is a high-priority requirement.

When a Blender source asset changes, reimport should update the imported
source data without destroying manually configured engine-side
properties.

Design stable asset IDs, source tracking, importer versions, dependency
records, and explicit override handling from the beginning.

### Blender workflow

The desired workflow:

1.  Create or edit an asset in Blender.
2.  Export or synchronize it using a predictable pipeline.
3.  Import it into the engine.
4.  Preserve relevant hierarchy, materials, skeleton, and animation
    information.
5.  Reimport after source changes.
6.  Retain engine-side references and overrides.

Do not build a full modelling application into the engine. Blender is a
content creation partner.

------------------------------------------------------------------------

## 9. Visual scripting and graph infrastructure

Support both:

-   Native Rust gameplay development.
-   Visual scripting for designers and developers who prefer graphs.

Plan a shared graph infrastructure where appropriate for:

-   Gameplay.
-   Materials.
-   Animation.
-   VFX.
-   Audio.

Graphs should have a defined data model, reflection integration,
serialization, validation, debugging, and execution model.

Visual scripting should not be a collection of editor-only callbacks.
Evaluate an intermediate representation (IR) that can be interpreted
initially and compiled or optimized later.

Do not start graph editor implementation until reflection,
serialization, type information, and runtime execution boundaries are
established.

------------------------------------------------------------------------

## 10. Proposed repository layout

Start with a Cargo workspace and clear crate ownership. Adjust names as
the architecture becomes concrete, but preserve the dependency
boundaries.

``` text
project-root/
├── Cargo.toml
├── rust-toolchain.toml
├── README.md
├── LICENSE
├── crates/
│   ├── foundation/
│   │   ├── math/
│   │   ├── memory/
│   │   ├── reflection/
│   │   ├── serialization/
│   │   └── diagnostics/
│   ├── runtime/
│   │   ├── ecs/
│   │   ├── scheduler/
│   │   ├── events/
│   │   ├── hierarchy/
│   │   ├── jobs/
│   │   └── runtime/
│   ├── rendering/
│   │   ├── rhi/
│   │   ├── renderer/
│   │   ├── frame-graph/
│   │   ├── shader-compiler/
│   │   ├── materials/
│   │   └── backends/
│   │       └── vulkan/
│   ├── simulation/
│   │   ├── physics/
│   │   ├── animation/
│   │   ├── navigation/
│   │   └── particles/
│   ├── systems/
│   │   ├── audio/
│   │   ├── input/
│   │   ├── networking/
│   │   └── ui/
│   ├── assets/
│   │   ├── database/
│   │   ├── importer/
│   │   ├── cooker/
│   │   ├── streaming/
│   │   └── formats/
│   ├── gameplay/
│   │   ├── framework/
│   │   └── visual-graph/
│   ├── editor/
│   │   ├── application/
│   │   ├── framework/
│   │   ├── viewport/
│   │   ├── inspector/
│   │   ├── outliner/
│   │   ├── content-browser/
│   │   └── graph-editor/
│   └── platform/
│       ├── windows/
│       └── linux/
├── applications/
│   ├── editor/
│   ├── player/
│   ├── asset-cooker/
│   └── shader-tool/
├── templates/
│   ├── empty/
│   ├── first-person/
│   └── third-person/
├── examples/
├── tests/
├── benchmarks/
├── docs/
│   ├── product/
│   ├── architecture/
│   ├── decisions/
│   ├── development/
│   └── user-guide/
└── tools/
```

Do not create every crate as an empty placeholder immediately. Create a
crate when it has a defined responsibility and a real dependency or
implementation need.

------------------------------------------------------------------------

## 11. Initial technology candidates

These are candidates, not unconditional commitments. Validate current
maintenance, licensing, platform coverage, performance, and API
suitability before adopting a dependency.

  -----------------------------------------------------------------------
  Area                    Candidate               Notes
  ----------------------- ----------------------- -----------------------
  Language                Rust                    Primary implementation
                                                  language

  Math                    glam                    Evaluate for engine
                                                  conventions

  Graphics API            Vulkan                  First backend

  Windowing               winit                   Hide behind platform
                                                  abstraction

  Physics                 Rapier / Jolt           Benchmark and abstract

  Audio                   Kira                    Evaluate production
                                                  requirements

  Editor UI               egui for prototype      Must remain replaceable

  Serialization           Serde + RON             Versioned project data

  Compression             zstd                    Evaluate asset pipeline
                                                  needs

  Parallelism             rayon + custom jobs     Avoid leaking
                                                  implementation details

  Diagnostics             tracing                 Structured diagnostics

  Asset interchange       glTF                    First-class initial
                                                  format
  -----------------------------------------------------------------------

Do not use a library's API as the public engine API unless that is an
explicit, documented decision.

Do not add dependencies without checking license compatibility and
maintenance health.

------------------------------------------------------------------------

## 12. Development roadmap

Build vertical slices. Every milestone should leave the repository in a
working, testable state.

### Milestone 0 --- Architecture and infrastructure

-   Create repository and Cargo workspace.
-   Establish formatting, linting, and test conventions.
-   Set up Windows and Linux CI.
-   Define error handling and logging.
-   Create architecture decision records.
-   Define crate dependency rules.
-   Audit dependency licensing.
-   Document the first runtime, scene, and RHI interfaces.

**Exit criteria:** Workspace builds and tests on Windows and Linux.
Architecture boundaries are documented.

### Milestone 1 --- First interactive runtime

-   Window and event handling.
-   Vulkan initialization on Windows and Linux.
-   Swapchain and frame presentation.
-   GPU resource lifecycle.
-   Basic mesh rendering.
-   Camera and transforms.
-   Minimal ECS and scheduler.
-   Input handling.

**Exit criteria:** A small application renders and moves around a 3D
scene on both platforms.

### Milestone 2 --- Editor foundation

-   Editor application.
-   Dockable panels.
-   Main viewport.
-   World Outliner.
-   Details panel.
-   Transform gizmos.
-   Selection and multi-selection.
-   Undo/redo foundation.
-   Scene save/load.

**Exit criteria:** Create, select, transform, save, close, reopen, and
continue editing a scene.

### Milestone 3 --- Asset workflow

-   Project format.
-   Asset database.
-   glTF/GLB importer.
-   Texture importer.
-   Material assignment.
-   Content Browser.
-   Stable asset identifiers.
-   Dependency tracking.
-   Reimport foundation.

**Exit criteria:** Import a Blender-exported environment, place it in a
scene, save the project, and reopen it without broken references.

### Milestone 4 --- First playable game

-   Physics integration.
-   Character controller.
-   Input mapping.
-   Gameplay components.
-   Rust gameplay modules.
-   Basic audio.
-   Skeletal animation playback.
-   Play in Editor.
-   Runtime debugging.

**Exit criteria:** Build and play a small third-person game inside the
editor.

### Milestone 5 --- Production rendering

-   Deferred rendering.
-   PBR.
-   Lighting and shadow improvements.
-   HDR and post-processing.
-   Shader diagnostics.
-   GPU profiling.
-   Material instances.
-   Temporal rendering.
-   Performance benchmarks.

**Exit criteria:** Produce a convincing, performant real-time game
environment on supported Windows and Linux hardware.

### Milestone 6 --- Professional game tools

-   Visual scripting.
-   Animation graphs.
-   VFX graphs.
-   Audio tools.
-   UI designer.
-   Terrain and foliage.
-   Navigation and AI.
-   Cinematic timeline.
-   Multiplayer framework.
-   Advanced profiling.

### Milestone 7 --- Production and distribution

-   Asset cooking.
-   Packaging.
-   Windows and Linux player builds.
-   Dedicated server.
-   Automated testing.
-   Plugin SDK.
-   Project templates.
-   Version control integration.
-   Documentation and release process.

------------------------------------------------------------------------

## 13. Initial 90-day plan

This is an aggressive prototype plan for an experienced developer or
small team, not a delivery guarantee.

  -----------------------------------------------------------------------
  Period                  Focus                   Deliverable
  ----------------------- ----------------------- -----------------------
  Days 1--15              Architecture and CI     Compiling workspace,
                                                  ADRs, platform CI

  Days 16--30             Vulkan and rendering    Window, swapchain,
                                                  basic mesh

  Days 31--45             Runtime                 ECS foundation,
                                                  transforms, input,
                                                  scene

  Days 46--60             Editor                  Viewport, Outliner,
                                                  Inspector, gizmos

  Days 61--75             Assets                  Project format, Content
                                                  Browser, glTF import

  Days 76--90             Vertical slice          Play-in-editor and
                                                  basic third-person
                                                  prototype
  -----------------------------------------------------------------------

If working solo, extend this schedule rather than cutting the platform
or architecture requirements.

------------------------------------------------------------------------

## 14. Quality and acceptance criteria

Avoid vague claims such as "fast," "AAA-ready," or "production-quality"
without measurable tests.

### Initial targets

-   60 FPS editor target on defined reference hardware.
-   60 FPS runtime target on defined reference hardware and test scene.
-   Small-project editor startup target under five seconds, measured on
    reference hardware.
-   Small-asset hot-reload target under one second, measured on
    reference hardware.
-   No scene or asset reference loss during normal save/reopen/reimport
    workflows.
-   Windows and Linux build and test coverage.
-   CPU and GPU profiling from early milestones.
-   Reproducible asset cooking and build inputs.

### Benchmark projects

Maintain three test projects:

1.  **Small:** Stylized platformer with 100--500 objects.
2.  **Medium:** Third-person environment with animation, physics, and
    lighting.
3.  **Large:** Streaming world with thousands of assets and large
    visibility ranges.

Use benchmarks to guide architecture. Do not optimize against arbitrary
object counts without defining hardware, content, and workload.

------------------------------------------------------------------------

## 15. Open-source and project governance

Proposed licensing: **MIT OR Apache-2.0**, subject to a dependency and
legal review.

-   Engine and editor should be open source.
-   Third-party dependencies retain their original licenses.
-   Documentation can use CC BY 4.0.
-   Community plugins can choose compatible licenses.
-   Publish contribution guidelines.
-   Maintain architecture decision records.
-   Maintain a public roadmap.
-   Require tests and documentation for meaningful subsystem changes.

Do not introduce proprietary account requirements or mandatory online
activation for basic editor and runtime use.

------------------------------------------------------------------------

## 16. Engineering rules for Claude Code

These rules apply to all implementation work.

### Before coding

1.  Inspect the repository and existing implementation.
2.  Read relevant architecture documents and ADRs.
3.  Identify the owning crate and its allowed dependencies.
4.  State the proposed change and its scope.
5.  Check whether an existing abstraction should be extended rather than
    duplicated.
6.  Check Windows and Linux implications.

### While coding

1.  Keep changes small and coherent.
2.  Prefer clear, explicit interfaces over premature abstraction.
3.  Do not create speculative frameworks without a real use case.
4.  Avoid unnecessary `unsafe`; document and minimize every required
    unsafe boundary.
5.  Do not use global mutable state as a shortcut.
6.  Do not let editor dependencies leak into runtime crates.
7.  Do not let platform-specific code leak into platform-independent
    crates.
8.  Do not silently add dependencies.
9.  Document important architectural decisions.
10. Keep errors actionable and diagnostics useful.

### After coding

1.  Run `cargo fmt --check`.
2.  Run relevant tests.
3.  Run `cargo clippy` for affected workspace targets where practical.
4.  Build on Windows and Linux CI.
5.  Add regression tests for fixed bugs.
6.  Update documentation and ADRs when architecture changes.
7.  Report what was implemented, what was tested, and what remains
    incomplete.

### Prohibited shortcuts

-   Do not fork or embed Bevy, Fyrox, Godot, or Unreal as the engine
    foundation.
-   Do not claim Linux support based only on successful compilation.
-   Do not build a general-purpose modelling or ArchViz product.
-   Do not implement a renderer feature before its required resource and
    diagnostic foundations exist.
-   Do not create a monolithic crate containing the entire engine.
-   Do not promise compatibility with Unreal project files.
-   Do not implement a large feature as an untested prototype and label
    it production-ready.
-   Do not invent APIs or assume a dependency supports a feature without
    verifying it.

------------------------------------------------------------------------

## 17. First task for Claude Code

Do not attempt to implement the entire engine in one pass.

Start by:

1.  Inspecting the repository.
2.  Determining whether it is empty or contains existing work.
3.  Creating or updating `README.md`.
4.  Creating `docs/product/PRD.md` from this product brief.
5.  Creating `docs/architecture/TAD.md` with proposed subsystem
    boundaries and dependency direction.
6.  Creating `docs/decisions/` and initial ADRs for:
    -   Greenfield engine architecture.
    -   Windows and Linux as first-class platforms.
    -   Vulkan as the first graphics backend.
    -   Runtime/editor separation.
    -   Engine-owned ECS and scene hierarchy.
    -   Asset identity and non-destructive reimport.
7.  Proposing a minimal Cargo workspace.
8.  Listing unresolved technical decisions and risks.

Do not scaffold every planned crate or begin a large renderer
implementation before the architecture has been reviewed.

If the repository already contains code, preserve it and explain how it
maps to this architecture. Never overwrite existing work without
inspecting it.

### First implementation milestone

Once the architecture is approved, implement the smallest cross-platform
vertical slice:

-   A Rust application.
-   A window on Windows and Linux.
-   Vulkan initialization.
-   A clear color and basic rendered geometry.
-   Camera movement.
-   A repeatable build and test process.

Keep the implementation minimal, measurable, and runnable.

------------------------------------------------------------------------

## 18. Unresolved decisions to research

Do not guess. Research and document trade-offs before locking these
decisions:

-   Custom ECS implementation details and archetype/storage strategy.
-   Reflection implementation and derive-macro approach.
-   Vulkan abstraction and memory allocation strategy.
-   Shader language and compiler pipeline.
-   Editor UI framework and long-term custom widget requirements.
-   Render graph design.
-   Scene and project file format/versioning.
-   Asset database storage and stable identifier strategy.
-   FBX importer licensing and implementation options.
-   Physics backend comparison.
-   Rust gameplay module hot reload strategy.
-   Visual scripting execution and compilation model.
-   Plugin ABI and compatibility strategy.
-   Linux packaging and graphics driver support matrix.

Each decision should have an ADR containing context, options,
trade-offs, decision, and consequences.

------------------------------------------------------------------------

## Final product test

The engine is moving in the right direction when an Unreal developer
can:

1.  Install it on Windows or Linux.
2.  Create a third-person project.
3.  Import a character and environment from Blender.
4.  Configure materials and lighting.
5.  Add gameplay in Rust.
6.  Set up character movement, animation, and collision.
7.  Press Play and iterate inside the editor.
8.  Profile the game.
9.  Package a playable Windows or Linux build.

That end-to-end workflow is the product. Individual engine subsystems
exist to make it excellent.
