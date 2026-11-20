# Glossary

One meaning per word, used the same way in code, docs and editor UI. Where Unreal has an
established term, Oxreal uses it with the same user-facing meaning (PRD §6). The implementation
is our own (ADR-0007).

## World and gameplay

| Term | Meaning | Not to be confused with |
|---|---|---|
| **Entity** | A runtime ECS identity: a 64-bit generational handle with components attached (ADR-0014). Runtime-only, never saved. | Actor |
| **Component** | Plain data attached to an entity. It carries no logic: systems act on it. | Unreal's ActorComponent, which also carries logic |
| **Capability** | A component (or a few) saying what an entity can do or how it takes part in gameplay: `Health`, `Interactable`, `PlayerControlled` (ADR-0020) | A class or interface |
| **System** | A function scheduled by the ECS that queries and updates components in a stage, with its access declared. Where gameplay logic lives. | |
| **Stage** | One step of the frame schedule: `Input → PrePhysics → DuringPhysics → PostPhysics → PostUpdate → Extract`. Unreal's *tick groups*. | |
| **World** | The ECS container for one simulation: entities, components and resources | Level |
| **Actor** | An authoring term: an object placed in a level, with a stable object ID, a transform and a place in the hierarchy. What the World Outliner lists. At run time it's an entity; there is no `Actor` type (ADR-0020). | Entity, which can be internal and not placed |
| **Pawn**, **Character**, **Controller**, **PlayerController**, **GameMode**, **GameState**, **PlayerState** | Unreal-equivalent gameplay roles, built as compositions of capabilities, relations and systems (ADR-0020) | Classes: they aren't |
| **Event** | A notification that something happened (spawned, damaged, opened), which systems that declared interest react to. Contract in D-22. | Lifecycle hook, which maintains engine invariants |
| **Graph** | A state machine or visual script that an entity carries as data, run by a scheduled executor system (ADR-0020, D-12). The home of per-object designer logic. | A per-object tick callback, which Oxreal doesn't have |
| **Actor template** | A reusable, overridable authored definition: components, defaults, children and optionally a graph. Unreal's Blueprint class, also known as a prefab. Instantiating one creates ordinary entities. | A runtime class |
| **Level** | A saved arrangement of actors that can be loaded into a world | Scene, World |
| **Scene** | Avoided as a technical term. In user-facing text it means "what's in the viewport". | |
| **Hierarchy** | Parent/child relationships between actors (`ChildOf`/`Children`), driving transform propagation | |
| **Transform** | Translation, rotation and scale (`oxreal-math`). `world = parent * local`. | Matrix |

## Rendering

| Term | Meaning |
|---|---|
| **RHI** | Rendering Hardware Interface: Oxreal's backend-agnostic GPU API (ADR-0003). Vulkan is its first backend. |
| **Backend** | One RHI implementation, for example `oxreal-rhi-vulkan` |
| **Render world** | The renderer's own snapshot of what to draw. The renderer never reads or mutates the game world directly (TAD §5.2). |
| **Extraction** | The `Extract` stage that copies render-relevant data from the world into the render world |
| **Frame graph** | The per-frame description of render passes and their resources (D-06) |
| **Reversed depth** | Depth 1 at the near plane, approaching 0 at infinity (ADR-0011) |
| **Physical / logical pixels** | Physical pixels are real device pixels. Logical pixels are scaled by the display's scale factor (ADR-0015). |

## Assets and tooling

| Term | Meaning |
|---|---|
| **Asset** | Anything a project stores and references by ID: mesh, texture, material, level, template |
| **AssetId** | A stable 128-bit identity, independent of path, that survives moves, renames and reimports (ADR-0006) |
| **Source asset** | The file a tool produced, for example a `.blend` export as glTF |
| **Import / reimport** | Converting a source asset into engine data. Reimport preserves engine-side overrides (ADR-0006). |
| **Cook** | Converting project data into the optimized, platform-specific form a shipped game loads |
| **Derived data cache (DDC)** | Cache of import and cook results, keyed by source hash, importer version, settings and platform |

## Project and process

| Term | Meaning |
|---|---|
| **Layer** | A crate's architectural tier (foundation → platform → runtime → … → applications). Dependencies only point down (ADR-0004, TAD §4). |
| **ADR** | Architecture Decision Record in `docs/decisions/`. Merging an Accepted ADR's PR makes it binding. |
| **Open decision (D-xx)** | A question still to be decided, tracked in `open-decisions.md` |
| **Changelog fragment** | A per-PR file in `changelog.d/` that is assembled into `CHANGELOG.md` at release (ADR-0009) |
| **Milestone (M0–M7)** | A roadmap stage with exit criteria (PRD §8). 0.1.0 is released at the M1 exit. |
