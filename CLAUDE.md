# CLAUDE.md

Guidance for Claude Code working in this repository.

## What this project is

A greenfield, Rust-native, open-source **3D game engine and editor** with Unreal Engine-class
ambitions and an editor that Unreal developers recognize. Windows and Linux are both first-class
from day one. Vulkan is the first graphics backend.

- Product brief (original source of truth): `docs/rust_game_engine_claude_code_brief.md`
- Product requirements: `docs/product/PRD.md`
- Architecture (layers, crates, dependency rules): `docs/architecture/TAD.md`
- Decisions: `docs/decisions/` (ADRs). Unresolved items: `docs/development/open-decisions.md`
- Conventions (errors, logging, unsafe, tests): `docs/development/conventions.md`
- Environment setup: `docs/development/setup.md`

Working title is **TBD**. Do not invent a product name or crate prefix. See open decision D-15.

## Current state

Milestone 0 (architecture and infrastructure). The workspace contains only `tools/xtask`.
Engine crates are created **only when they have a defined responsibility and real code**. Never
scaffold empty placeholder crates. Use the `new-crate` skill.

## Commands

```sh
cargo xtask ci          # everything CI runs: fmt check, clippy -D warnings, tests, layer check
cargo xtask layers      # enforce crate layer/dependency rules (docs/architecture/TAD.md §4)
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check        # licenses, advisories, banned crates (needs: cargo install cargo-deny)
```

Run `cargo xtask ci` before saying that work is done. Report what was tested and what wasn't.

## Non-negotiable rules

1. **No engine foundation from elsewhere.** Never fork, embed or depend on Bevy (incl. `bevy_ecs`),
   Fyrox, Godot or Unreal as a foundation. `deny.toml` bans them.
2. **Layering is enforced.** Every workspace crate declares
   `[package.metadata.engine] layer = "..."`, and `cargo xtask layers` rejects illegal edges.
   Runtime must never depend on editor, and player builds must contain no editor code.
3. **Renderer does not own scene data.** It consumes an extracted render world.
4. **Platform code stays behind platform interfaces.** No `cfg(windows)` or `cfg(target_os = "linux")`
   outside the platform layer, unless it's justified in a comment.
5. **Linux support is not "it compiles".** Claim it only when the code has been run and tested on Linux.
6. **No silent dependencies.** New third-party crates need a license, maintenance and platform check
   (use the `dependency-auditor` agent) and a line in `docs/development/dependencies.md`.
   Don't expose a dependency's types in a public engine API unless an ADR says so.
7. **`unsafe` is minimized and documented.** Every `unsafe` block gets a `// SAFETY:` comment
   (clippy enforces this). Prefer safe wrappers at the lowest layer.
8. **No global mutable state as a shortcut.** No `static mut`. Don't use lazy globals for engine state.
9. **Don't invent APIs.** Verify that a dependency supports something (docs or source) before relying on it.
10. **Foundations before features.** A renderer feature needs its resource, frame-graph and
    diagnostic foundations first. Visual scripting needs reflection, serialization and an execution
    model first.
11. **Architecture changes need an ADR.** Use the `new-adr` skill. Update `TAD.md` when boundaries move.
12. **Out of scope:** ArchViz, film or virtual production, CAD, general-purpose modelling, and Unreal
    project-file compatibility.

## Unreal Engine source reference

A local Unreal Engine source checkout path is recorded in `docs/UnrealEngineSource.txt`
(currently `H:\UnrealEngine\UnrealEngine`). Use it **only as a UX and terminology reference**:
editor layout, panel names, shortcuts, workflow behaviour. The UE source is under the Epic EULA,
and this project is MIT/Apache-2.0.

- **Never** copy, translate or closely paraphrase Unreal code, shaders, comments or data into this
  repository. Don't port algorithms line by line.
- Describe *observable behaviour* ("W/E/R switch gizmo mode; the Details panel groups properties
  by category with a search box") and design our own implementation.
- Delegate this research to the `unreal-ux-researcher` agent. Its output is behavioural notes, not code.

## Workflow (from brief §16)

Before coding: read the relevant ADRs and TAD section, identify the owning crate and its allowed
dependencies, state the scope, prefer extending existing abstractions, and consider both Windows and Linux.

While coding: make small, coherent changes. Use explicit interfaces and avoid speculative
frameworks. Errors must be actionable.

After coding: run `cargo xtask ci`, add regression tests for bug fixes, update docs and ADRs, and
report what was implemented, what was tested and what remains incomplete.

## Project agents (`.claude/agents/`)

| Agent | Use for |
|---|---|
| `architecture-reviewer` | Reviewing a change against TAD layering, ADRs and brief rules |
| `adr-author` | Researching an open decision and drafting an ADR with options and trade-offs |
| `dependency-auditor` | Vetting a crate before adoption (license, maintenance, platforms, unsafe) |
| `cross-platform-reviewer` | Windows/Linux portability of a change |
| `unsafe-auditor` | Reviewing `unsafe`, FFI and Vulkan code for soundness |
| `unreal-ux-researcher` | Studying Unreal editor UX in the local source (behaviour only, never code) |
| `test-engineer` | Writing unit, integration and regression tests and benchmarks |

Skills (`.claude/skills/`): `verify`, `new-adr`, `new-crate`.

## Code style quick reference

- Edition 2024, toolchain pinned in `rust-toolchain.toml`. Lints are configured in `[workspace.lints]`.
  Crates opt in with `[lints] workspace = true`.
- Libraries use typed errors (`thiserror`-style enums once adopted). Applications may use
  `anyhow`-style context. Never `unwrap()` in library code paths that can fail at runtime.
- Use `tracing` for diagnostics once adopted. No `println!` in library crates.
- Coordinate system, units and handedness: see `docs/development/conventions.md`.
  Don't assume them anywhere else.
