# Prior art

Projects whose experience informs Oxreal's decisions. We reuse **knowledge and validated design
choices**, not code. Every Oxreal implementation is written fresh, in idiomatic Rust, under
MIT OR Apache-2.0.

## Cordite: the maintainer's C++ engine

- **What it is:** a C++20 3D engine with an editor. Vulkan through NVIDIA's NVRHI, plus offscreen
  Direct3D 12. Slang shaders. An EnTT-based ECS with a hand-written reflection table. Jolt
  physics, Lua scripting, miniaudio, cgltf/ufbx import, deferred rendering, Play-in-Editor,
  undoable editor commands, prefabs, and game export.
- **Where:** github.com/mrgyarmati/CorditeEngine (original: Bitrose-Studio/GameEngineExperiment).
  The maintainer has a local copy. Its Phase 0 audit (`docs/architecture/CORDITE_AUDIT.md`,
  `TECHNICAL_RISKS.md`, 2026-11-12) is the main source of the lessons below.
- **Code reuse policy:** Cordite is the maintainer's own work, so its design is free to learn
  from. Porting code (rewritten in Rust) is the copyright holder's decision, made case by case.
  Third-party code inside Cordite keeps its own licenses and is never copied.

### Validated choices Oxreal adopts

| Area | Cordite's choice | Oxreal |
|---|---|---|
| Spatial conventions | Right-handed, +Y up, metres, −Z forward. Column-major. Y-up clip space with depth [0, 1] on all backends. Reversed depth. | ADR-0011 |
| Shaders | Slang to SPIR-V (and to DXIL through DXC) | ADR-0013 |
| ECS storage | Sparse sets (EnTT), wrapped by a `World` that adds permanent IDs, names, an ordered hierarchy and world transforms | Input to D-01 and ADR-0005 |
| Reflection | Explicit per-type tables with ranges, units, descriptions and read-only and runtime flags. Input is parsed into a validated patch before anything changes. | Input to D-02 |
| Editor architecture | Context → Actions (validated, undoable) → Commands (by name, JSON) → panels. Every UI action is a command. | TAD §5.7 |
| Identity | Assets and entities identified by UUIDs, never paths | ADR-0006 |
| Testing | Renderer tests assert pixel values from the lighting model, reference images, layering and doc-link tests, performance budgets | Testing strategy (to adopt as the renderer lands) |

### Lessons: risks Cordite hit, and how Oxreal avoids them

| Cordite risk | What went wrong | Oxreal mitigation |
|---|---|---|
| R7: NVRHI as the de facto public RHI | 31 of 55 renderer headers exposed `nvrhi::` types, so adding OpenGL meant writing an NVRHI backend (~4,700+ lines) | Engine-owned RHI with no backend types in its API (ADR-0003, TAD §5.3) |
| R12: thin reflection | No methods, lists of structs, categories or type registry, which blocked widgets, data assets and C# | Decide reflection (D-02) early, with language-neutral metadata (TAD §5.8) |
| R11: Lua-bound script API | About 98 hand-bound `lua_State` functions | Gameplay API as handles plus plain data, and the C ABI as the single binding surface (TAD §5.8, D-21) |
| R13: formats without migrations | `Document` refused newer versions and had no upgrade path | Format versioning and migrations decided with D-07 |
| R1: input can't be consumed | No input contexts or consumption, so UI couldn't block gameplay input | Input design must include context stacks and consumption from the start |
| R3, R9: blocking editor operations and synchronous assets | Import, export and open froze the UI. The job system went unused. | Async asset pipeline and job system designed together (D-01/D-08) |
| R6: mandatory runtime parts | Every game linked Lua, physics, audio and ImGui | Crate layering plus optional subsystems (ADR-0004) |
| R8: device loss | No recovery, so the editor lost unsaved work | Device-loss handling is part of the RHI design (M1), with editor recovery saves (M2) |
| R5: GPU tests on one machine | Linux GPU tests never ran until late, and 17 cases failed on lavapipe | Linux in CI from day one (ADR-0002). GPU CI strategy (D-19). |
