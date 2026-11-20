# ADR-0020: Capability-oriented gameplay on the ECS

- **Status:** Accepted
- **Date:** 2026-11-20
- **Related:** PRD §5–§7, TAD §5.1, §5.6 and §5.8, ADR-0005 (ECS and hierarchy), ADR-0006 (overrides), ADR-0007 (clean room), ADR-0014 (ECS), D-02, D-07, D-12, D-20 (resolved here), D-21, D-22 (opened here). **Supersedes ADR-0008.**

## Context

ADR-0008 (Proposed) planned an Unreal-style gameplay framework on the ECS, with two API levels:
- plain **systems**;
- per-actor **Behaviours**, objects with `begin_play`, `tick` and `end_play` callbacks, which
  visual scripts and other languages would also implement.

Since then, the ECS has been built and accepted (ADR-0014), and it set the engine's direction:
- the ECS is the only runtime data store;
- logic lives in systems that declare what they read and write;
- structural changes go through deferred commands;
- relations such as the hierarchy are components kept consistent by hooks (the ADR-0005
  amendment).

The Behaviour level doesn't fit that direction:
- **A second update mechanism.** Ticking objects run next to scheduled systems, with their own
  order and lifecycle.
- **Hidden data access.** A callback can touch anything its context exposes, so the scheduler
  can't see what it reads and writes, and can't run it in parallel safely.
- **A second object model.** Behaviour state lives in trait objects instead of components, so
  serialization, reflection, the Details panel, replication and other languages (TAD §5.8) would
  each need a second path.

A proposal reviewed on 2026-11-20 argued for an **ECS-centric, capability-oriented** model
instead. Unreal's concepts stay available as vocabulary, without becoming runtime classes.

The need ADR-0008 was answering is real, and stays a requirement. Designers expect "this door
has its own logic", and Unreal developers expect familiar roles and terms (PRD §6). Pure systems
alone push every door and pickup into a dedicated system.

## Options considered

### Option A: ADR-0008's facade, with per-actor Behaviours as a runtime concept
- Pros: the most familiar shape for Unreal developers, and per-object logic is easy to write.
- Cons: the three problems above. The scheduler can't reason about callbacks, and every
  consumer of gameplay state (serialization, replication, the editor, the C ABI) needs a
  second path. A callback model is hard to remove once user code depends on it.

### Option B: Pure ECS, with no designer-level per-entity logic
- Pros: one model and one mechanism. The simplest engine.
- Cons: fails the PRD's designer and visual-scripting requirements. Every piece of bespoke
  logic needs a Rust system, and nothing in the model describes stateful, per-object behaviour.

### Option C: Capabilities on the ECS, with events and scheduled state machines and graphs (chosen)
- Pros:
  - one runtime data model and one scheduling model;
  - per-object logic still exists, but as data run by systems, with declared access;
  - Unreal's concepts survive as authoring vocabulary and composition patterns.
- Cons:
  - more design work before gameplay code: events (D-22) and the graph execution model (D-12)
    must exist before per-object logic does;
  - Unreal developers have to learn that a "Pawn" is a set of components, not a class.
- Evidence: this is the direction the accepted ECS (ADR-0014) and hierarchy (ADR-0005
  amendment) already took. The engines the proposal cites as references (Unity Entities' split
  between authoring and runtime data, Flecs' relations and observers) inform the shape, not the
  implementation. No benchmark supports or refutes it yet. §6 names the prototype that will.

## Decision

### 1. The ECS is the only runtime data model
- All runtime gameplay state lives in ECS components, resources and relations.
- No gameplay API, editor abstraction or language binding introduces a second object model, a
  second component registry, or state that duplicates components.
- **There is no runtime `Actor` type and no inheritance hierarchy.**

### 2. Gameplay is capabilities, relations and systems
- **A capability is data:** components that say what an entity can do or how it takes part in
  gameplay (`Health`, `Interactable`, `CharacterMovement`, `PlayerControlled`). Components
  carry no logic of their own.
- **Logic lives in systems** that declare their access and run in the schedule's stages
  (ADR-0014 §5). Bulk and continuous simulation always belong here.
- **Links between entities are relations**, with consistency kept by hooks, as for the
  hierarchy: possession, ownership, attachment.
- **Unreal's roles are compositions, not classes.**

  | Unreal concept | Oxreal |
  |---|---|
  | Actor | An authored object in a level (§4). At run time, an entity. |
  | Actor component | A component |
  | Scene component, attachment | A child entity in the hierarchy (ADR-0005) |
  | Pawn, Character | Capability sets: `Pawn` is the possessable capability, and a character adds movement, a collider and so on |
  | Controller, possession | A controller entity, linked to its pawn by a possession relation |
  | `Cast<T>`, interfaces | A component test (`World::has`) or a query filter |
  | GameMode | Rules as resources plus systems |
  | GameState, PlayerState | A state resource, or an entity per player |
  | Subsystem | A resource or service with a defined lifecycle scope |
  | Blueprint class | An actor template (§4), optionally with a graph (§3) |
  | Actor tick, tick groups | Scheduled systems and stages |
  | `BeginPlay`, `EndPlay`, overlap and similar | Events (§3) |

  The exact capability components, their names and their systems are designed when each
  gameplay feature is built. This table fixes their *kind*, not their contents.

### 3. Three execution mechanisms, one access model
- **Systems:** the primary mechanism, as above.
- **Events and observers** for reactions: something happened (spawned, despawned, damaged,
  opened), and systems that declared interest react. Their contract is decided in its own ADR
  (D-22) before any gameplay depends on them. That ADR must define:
  - ordering relative to stages and deferred commands;
  - lifetime and delivery guarantees;
  - how readers and writers are declared, so the scheduler sees them.

  ECS lifecycle hooks stay what ADR-0014 §4 made them: engine invariants such as relation
  inverses, never a gameplay event mechanism.
- **State machines and graphs** for stateful, designer-authored logic (a door, a boss's phases,
  a quest). An entity carries the graph's state and a reference to its asset as components.
  **Executor systems** run the graphs in a stage, with access declared to the scheduler: the
  union of what the graphs can touch. Visual scripting (D-12) must fit this contract.

**No per-object callbacks are a runtime concept.** Nothing ticks itself, and there's no
`begin_play`, `tick` or `end_play` trait. "This door has its own logic" is data (a graph and its
state) that a scheduled system runs.

### 4. Authored data stays separate from runtime state
- **"Actor" is an authoring term:** an object placed in a level, with a stable object ID, a
  transform and a place in the hierarchy, listed in the World Outliner. The editor may use it
  freely in its UI.
- **Identity:** stable object IDs identify authored objects in files, templates and
  references. `Entity` handles are runtime-only and never saved (ADR-0014 §3).
- **Actor templates** (prefabs, Unreal's Blueprint classes) are authoring assets: components,
  default values, children and optionally a graph. Instances store overrides (ADR-0006).
  Instantiating one creates ordinary entities. A template is never a runtime class.
- **Levels are instantiated into worlds:** loading resolves assets, spawns entities and links
  relations. Runtime changes never silently write back to authored data. Play in Editor runs
  in its own world, and stopping it restores the authored state. The file format and the
  editor's in-memory representation of a level are decided with D-07 and the editor. They must
  keep this separation, and the runtime hierarchy stays ECS data (the ADR-0005 amendment).
- Editor-only metadata (selection, locking, folders) stays out of cooked runtime data.

### 5. What carries over from ADR-0008
- **Unreal terminology in user-facing text** where it's the established term (PRD §6), with
  our own implementation (ADR-0007).
- **No Unreal-style type prefixes** (`A`, `U`, `F`, `T`, `E`, `I`) in identifiers. Write
  `Pawn`, not `APawn`. Module paths disambiguate. (This rule was ADR-0008 §4.)
- **The ECS requirements of ADR-0008 §3** (relations, hooks, deferred commands, stages,
  reflection-driven access, constant-time component tests) are already commitments of
  ADR-0014.
- **Stages** are Unreal's tick groups by another name (ADR-0014 §5).

### 6. Validated by a prototype before the gameplay framework grows
Before gameplay features beyond the M1 player are built (and before M4), a vertical slice
exercises the model end to end:
- a player-controlled character with movement and health, possessed through a relation;
- a door driven by a state machine;
- a thousand AI-controlled agents;
- projectiles and damage events;
- a level saved, reloaded and run in Play in Editor without losing authored changes.

It records its benchmarks: bulk iteration, structural changes, event dispatch, state-machine
execution, scheduler overhead and level instantiation. What it shows can amend this ADR. Until
then, these stay open:
- the event and observer contract (D-22);
- how graphs declare their access (D-12);
- which authoring relations, if any, stay in the level file rather than the world (D-07);
- when and which component types need sparse-set storage (ADR-0014).

### 7. Other languages
C++ and C# gameplay code (TAD §5.8, D-21) plugs into the same contracts as Rust: components,
systems with declared access, events, and graph nodes. There's no Behaviour contract for them
to implement.

## Consequences

- **Positive:**
  - One runtime model, one scheduler, one access model. Serialization, reflection, the Details
    panel, replication and the C ABI each have one path.
  - Everything a piece of logic touches is visible to the scheduler, so parallel execution stays
    possible.
  - Unreal developers keep familiar terms in the editor and the docs.
- **Negative / costs:**
  - Per-object logic waits for the events ADR (D-22) and the graph execution model (D-12).
  - Familiarity depends on documentation and the editor: "a Pawn is components" must be taught
    well, with examples.
  - Executor systems that run many different graphs declare broad access, which limits their
    parallelism. D-12 decides how much finer the declarations get.
- **Follow-up work:**
  - The events and observers ADR (D-22).
  - The visual scripting IR and execution ADR (D-12), against §3's contract.
  - D-07 (levels and their instantiation), against §4.
  - The prototype of §6.
- **Revisit triggers:**
  - The prototype or user testing shows designers can't express common per-object logic
    without writing Rust systems.
  - Graph executors become a serial bottleneck that finer access declarations can't fix.
  - Event dispatch costs dominate frame time in benchmark projects.
