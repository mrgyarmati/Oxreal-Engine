---
name: new-crate
description: Add a new crate to the engine workspace in the correct layer directory with layer metadata, workspace lints, docs and an explicit workspace member entry. Use whenever a new engine, application or tool crate is needed - and only when it has a defined responsibility and real code to hold.
---

# Add a workspace crate

## 0. Justify it first
Empty placeholder crates are forbidden (brief §10). State the following:
- The crate's single responsibility, and what it must **not** do.
- Its layer (TAD §2) and its intended workspace dependencies, checked against the matrix in TAD §4.
- Why an existing crate can't own this.

Crate names use the `oxreal-` prefix (D-15), e.g. `oxreal-ecs`, `oxreal-rhi`. Dev tooling under `tools/` is
exempt (e.g. `xtask`).

## 1. Location
| Layer | Directory |
|---|---|
| foundation, platform, runtime, rendering, simulation, systems, assets, gameplay, editor | `crates/<layer>/<name>` (Vulkan backend: `crates/rendering/backends/vulkan`) |
| app-runtime, app-editor | `applications/<name>` |
| tool | `applications/<name>` (shipped tools) or `tools/<name>` (dev tooling) |

## 2. Cargo.toml
```toml
[package]
name = "oxreal-<name>"
description = "<one line: responsibility>"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
publish.workspace = true

[package.metadata.engine]
layer = "<layer>"

[dependencies]
# workspace crates: <name> = { path = "../../<layer>/<name>" }
# third-party: <dep>.workspace = true   (must be audited; see docs/development/dependencies.md)

[lints]
workspace = true
```

## 3. Source
Follow `docs/development/coding-standards.md` (naming, docs, API design; CI enforces most of it).
`src/lib.rs` (or `main.rs`) starts with a `//!` doc comment that states the responsibility, the
layer, and its boundaries. Add real code and at least one test.

## 4. Register
- Add the path to `members` in the root `Cargo.toml` (explicit list, alphabetical within a layer).
- Add it to the crate table in `docs/architecture/TAD.md` §7 (or the relevant section).
- Any new third-party dependency goes in `[workspace.dependencies]` and in
  `docs/development/dependencies.md` after a `dependency-auditor` review.

## 5. Verify
Run `cargo xtask ci`. The layer check must pass. Consider an `architecture-reviewer` pass.
