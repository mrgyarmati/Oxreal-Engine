# Product Requirements Document

> Derived from `docs/rust_game_engine_claude_code_brief.md`. If the two documents conflict, the
> brief wins until this PRD is formally revised.
> **Status:** Draft v0.1 · **Name:** Oxreal (Oxreal Engine / Oxreal Editor / Oxreal Cook)

## 1. Vision

This is a complete, open-source, professional 3D **game development** ecosystem written primarily
in Rust. Developers can create, edit, test, profile, package and distribute games without
Unreal, Unity, Godot or any other engine underneath.

Positioning: *Unreal-level ambition, Blender-level openness, Rust-level engineering discipline.*
This describes ambition, not current parity.

## 2. Principles (product constraints)

1. Greenfield engine. We own the core architecture and fork no existing engine.
2. Rust-native engine, editor, tools and runtime.
3. Windows 10/11 x64 and Linux x86_64 are first-class from day one.
4. Unreal-familiar editor UX: terminology, layout, interaction patterns.
5. Game-focused scope.
6. Modular architecture with replaceable subsystem implementations.
7. Strict editor/runtime separation.
8. Open and extensible: documented APIs, plugins, automation.
9. Production-minded: testing, profiling, diagnostics, reproducible builds, cooking.
10. AI-automatable: important editor and project operations are exposed as structured commands and APIs.

## 3. Users

Unreal developers evaluating an alternative, indie and AA studios, gameplay programmers,
technical artists, game designers, small 3D teams, and developers who want a native Linux workflow.

## 4. Scope

**In scope:** real-time 3D games (first and third person, platformers, action, RPG, shooter,
simulation, multiplayer). Also in scope: worlds, levels, streaming, terrain and foliage,
instancing, characters, animation, physics, AI and navigation, audio, VFX, Rust gameplay, visual
scripting, game UI and HUD, in-game cinematics, asset import, reimport, cooking and packaging,
profiling, and dedicated servers.

**Not a priority:** ArchViz, film/TV, virtual production, CAD, general-purpose modelling, motion
graphics, and replacing a DCC application. Supporting features (cameras, timelines, high-quality
rendering) are built only as far as games need them.

## 5. Platform requirements

| Platform | Requirement |
|---|---|
| Windows 10/11 x64 | Editor, runtime, tools, builds and tests are first-class |
| Linux x86_64 | Editor, runtime, tools, builds and tests are first-class |

- Linux validation targets are Ubuntu LTS, Fedora and Arch. Debian and SteamOS follow later.
- Linux distribution uses portable archives, with Flatpak under evaluation. Sandboxing is never mandatory for developers.
- Graphics use an engine-owned RHI. Vulkan comes first, and DX12 and Metal must remain possible.
- Hardware validation covers NVIDIA, AMD and Intel GPUs. CI builds and tests both OSes on every significant change.
- macOS and consoles come later and must not block the first vertical slice.

## 6. Editor experience requirements

- The default layout has the Viewport, World Outliner, Details, Content Browser, a toolbar
  (save, play, pause, simulate), the Output Log, and access to Project and World Settings.
- Interaction includes W/E/R gizmo modes, right-mouse fly navigation, perspective and ortho
  views, drag-and-drop placement, double-click to edit, context menus, multi-select, parenting,
  editable properties, undo/redo, play/pause/simulate, a command palette, and customizable
  layouts and keybindings.
- Terminology covers Viewport, World Outliner, Details, Content Browser, World Settings, Project
  Settings, Play in Editor, Blueprint Editor, Material Editor, Animation Blueprint, Output Log and
  Sequencer. These terms are a UX reference only. We don't reuse Unreal's implementation or
  proprietary technology.
- The UI toolkit is replaceable (egui is the prototype candidate) and must not shape engine architecture.

## 7. Functional areas

| Area | Key capabilities |
|---|---|
| Core runtime | ECS, scheduler, events, fixed timestep, jobs, reflection, serialization, resource lifetime, input, diagnostics |
| Gameplay | Rust modules, components and systems, events, input mapping, character, FP and TP controllers, camera, interaction, save/load, game mode and state |
| Rendering | RHI, frame graph, shaders, render-world extraction, PBR, HDR, IBL, lights, shadows, post-processing, GPU profiling |
| Physics | Replaceable backend (Rapier and Jolt are candidates), rigid bodies, queries, character controller, joints, ragdolls, vehicles |
| Animation | Skeletal meshes, skinning, clips, state machines, blend spaces, graphs, IK, retargeting, root motion, events |
| AI | Navmesh, pathfinding, agents, perception, behavior trees, utility AI, debug visualization |
| Networking | Authority, replication, RPC, identity, prediction and reconciliation, dedicated servers, net profiling |
| Audio | Spatial audio, mixer, events, attenuation, effects. Audio graph comes later |
| VFX | CPU and GPU particles, emitters, trails, collision, graph authoring |
| UI | Runtime UI, HUD, layout and anchoring, focus, transitions, visual designer |
| Assets | glTF/GLB first, then FBX through a dedicated importer and USD later. Also images, audio and fonts. Stable IDs, dependencies, DDC, non-destructive reimport, hot reload, cooking |
| Graphs | Shared graph infrastructure (gameplay, material, animation, VFX, audio) with an IR that is interpreted first and compiled later |
| Languages | Rust is the first-class language for engine and gameplay. Visual scripting is for designers. **Planned later:** C++ and C# for gameplay and tooling through a stable, versioned C ABI and generated bindings (D-21) |

### Additional programming languages (future)

Rust stays the engine's implementation language and the first-class gameplay language. C++ and C#
are planned as additional **gameplay and tooling** languages so that Unreal (C++) and Unity/.NET
(C#) developers can bring their skills over. Delivery comes after the Rust gameplay API has
stabilized, and no milestone is committed yet.

The architecture must not rule this out in the meantime. See TAD §5.8 for the design constraints
that apply now.

### Non-destructive reimport (high priority)

Reimporting a changed Blender source must update the imported data and preserve engine-side
overrides and references. This needs stable asset IDs, source tracking, importer versions,
dependency records and explicit overrides from the start (ADR-0006).

## 8. Milestones

| # | Name | Exit criteria |
|---|---|---|
| M0 | Architecture & infrastructure | Workspace builds and tests on Windows and Linux. Boundaries are documented |
| M1 | First interactive runtime | Small app renders and navigates a 3D scene on both platforms (window, Vulkan, swapchain, mesh, camera, minimal ECS, input) |
| M2 | Editor foundation | Create, select, transform, save, close, reopen and continue editing a scene |
| M3 | Asset workflow | Import a Blender-exported environment, place it, save, and reopen with no broken references |
| M4 | First playable game | Build and play a small third-person game in the editor (PIE) |
| M5 | Production rendering | Convincing, performant environment on reference hardware for both OSes |
| M6 | Professional game tools | Visual scripting, anim and VFX graphs, UI designer, terrain, nav/AI, timeline, multiplayer |
| M7 | Production & distribution | Cooking, packaging, player and server builds, plugin SDK, templates, VCS integration |

The 90-day prototype plan is in brief §13. If the work is done solo, extend the schedule rather
than cutting platform or architecture requirements.

## 9. Quality targets

These must be measured on documented reference hardware (to be defined; see D-16):

- 60 FPS editor and 60 FPS runtime on the reference test scene
- Small-project editor startup under 5 s
- Small-asset hot reload under 1 s
- No reference loss across save, reopen and reimport
- CPU and GPU profiling from early milestones
- Reproducible cooking and build inputs

Benchmark projects: **Small** (stylized platformer with 100–500 objects), **Medium** (third-person
environment with animation, physics and lighting), and **Large** (streaming world with thousands of assets).

## 10. Governance

MIT OR Apache-2.0 (pending review). Documentation is CC BY 4.0. We keep ADRs, a public roadmap
and contribution guidelines. Meaningful changes need tests and documentation. No account or
online activation is required for basic use.

## 11. Final product test

An Unreal developer can install the engine on Windows or Linux and create a third-person
project. They can import a character and environment from Blender and set up materials and
lighting. They can add gameplay in Rust, set up movement, animation and collision, and then
Play and iterate in the editor. Finally, they can profile the game and package a playable
Windows or Linux build.
