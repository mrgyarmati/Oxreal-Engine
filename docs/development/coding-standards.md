# Coding Standards

How Oxreal code is named, shaped and documented. These rules describe what the codebase already
does, so new code looks like existing code.

- **Engine conventions** (errors, logging, profiling, `unsafe`, platform code, math and
  coordinates) are in [conventions.md](conventions.md).
- **Terms** (Actor vs Entity, Level vs Scene, and so on) are in [glossary.md](glossary.md).
- The baseline is the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/). This
  document adds Oxreal's choices on top, and wins where they differ.

## What CI enforces

A rule that a machine can check is checked by CI, not by reviewers.

| Rule | Enforced by |
|---|---|
| Formatting (`rustfmt.toml`: edition 2024, 100 columns) | `cargo fmt --check` |
| Every public item documented | `missing_docs` |
| Public functions returning `Result` have `# Errors`; ones that can panic have `# Panics`; `unsafe fn` has `# Safety` | `clippy::missing_errors_doc`, `missing_panics_doc`, `missing_safety_doc` |
| Every `unsafe` block has a `// SAFETY:` comment | `clippy::undocumented_unsafe_blocks` |
| Lint suppressions state a reason | `clippy::allow_attributes_without_reason` |
| Public types implement `Debug` and nothing is needlessly `pub` | `missing_debug_implementations`, `unreachable_pub` |
| No `dbg!`, `todo!`, `unimplemented!`, `println!` (in libraries) or `unwrap()` (outside tests) | clippy lints in `[workspace.lints]` |
| Crate layering | `cargo xtask layers` |
| Changelog fragment format | `cargo xtask changelog check` |
| Licenses, advisories, banned crates | `cargo deny check` |

CI runs clippy with `-D warnings`, so every warning fails the build.

**Suppressing a lint:** use `#[expect(lint, reason = "...")]`, scoped as narrowly as possible.
`expect` fails the build when it no longer applies, so suppressions can't go stale. Use `allow`
only when `expect` can't work, for example when the lint fires in some configurations and not in
others. Always give a reason.

## Naming

### Rust identifiers

Follow the standard Rust casing ([RFC 430]) everywhere:

| Kind | Style | Examples |
|---|---|---|
| Crates | `oxreal-<noun>` (kebab-case) | `oxreal-math`, `oxreal-window`, `oxreal-rhi-vulkan` |
| Modules and files | `snake_case`, singular | `projection`, `event_loop.rs` |
| Types and traits | `UpperCamelCase` | `Transform`, `WindowEvent`, `App` |
| Functions, methods, variables | `snake_case` | `looking_at`, `inner_size` |
| Constants and statics | `SCREAMING_SNAKE_CASE` | `REVERSED_DEPTH_CLEAR`, `LOG_ENV_VAR` |
| Cargo features | `kebab-case`, describe a capability | `application`, `profile-tracy` |

[RFC 430]: https://rust-lang.github.io/rfcs/0430-finalizing-naming-conventions.html

Oxreal specifics:

- **No type prefixes.** No Unreal-style `A`, `U`, `F`, `T`, `E` or `I` (ADR-0008 §4). Write
  `Actor` and `Pawn`, not `AActor` or `APawn`. Module paths disambiguate.
- **Acronyms are words:** `Rhi`, `Gpu`, `Ui`, `Id`, `Srgb`, giving `RhiDevice`, `AssetId`,
  `LinearRgba`. Never `RHIDevice` or `AssetID`.
- **American English in identifiers** (`color`, `normalize`, `initialize`, `behavior`), matching
  Rust and its ecosystem. Prose in docs may use either variety, consistently within a file.
  - Note: ADR-0008 sketches a `Behaviour` trait. It is to be named `Behavior` when implemented.
- **Names say what, not how:** `inner_size`, not `get_winit_inner_size`. Implementation details
  stay out of names.

### Function and method names

| Pattern | Meaning | Examples |
|---|---|---|
| `new` | Primary constructor | `PhysicalSize::new` |
| `from_*` | Constructor from a specific input | `Transform::from_translation`, `LinearRgba::from_srgb8` |
| `with_*` | Returns a copy with one field changed (builder style, `#[must_use]`) | `Transform::with_rotation` |
| No `get_` prefix | Getters are named after the value | `window.inner_size()`, `transform.forward()` |
| `set_*` | Setters | `window.set_title(..)` |
| `is_*`, `has_*`, `can_*` | Boolean queries | `size.is_zero()`, `world.has::<T>()` |
| `try_*` | Fallible variant, **only** when an infallible one also exists | |
| `to_*`, `as_*`, `into_*` | Conversions: expensive copy, free borrow, consuming (API Guidelines C-CONV) | `to_matrix`, `to_srgb`, `to_array` |
| `init_*` | Process-wide setup, **applications only** | `init_application_logging` |

### Errors

- One error enum per crate or subsystem, named `<Area>Error`: `WindowError`, `ProjectionError`,
  `InitError`.
- Public error enums are `#[non_exhaustive]`.
- Variants name **what failed**: `NoDisplay`, `CreateWindow`, `InvalidFilter`. Avoid generic
  variants like `Other` or `Failed`.
- Messages are lowercase, have no trailing period, and name the resource and a remedy:
  `"failed to create window \"Main\": ..."`.
- Third-party errors are boxed into `#[source]` fields, so their types never appear in our API.

### Units and quantities

- Data and code use SI units (metres, kilograms, seconds) and **radians**, with no suffix needed
  (ADR-0011).
- Anything else carries its unit in the name: `elapsed_ms`, `timeout_secs`, `size_bytes`,
  `fov_degrees` (UI and files only).
- Pixels say which kind: `PhysicalSize` (physical pixels) vs logical pixels in `WindowDesc` docs.
  Never mix them silently.
- Colors are linear unless the name says otherwise: `from_srgb`, `to_srgb`.

### Environment variables

Prefix `OXREAL_`, `SCREAMING_SNAKE_CASE`. Each one is documented in the crate that reads it and
listed here.

| Variable | Read by | Purpose | Status |
|---|---|---|---|
| `OXREAL_LOG` | `oxreal-diagnostics` | Log filter, `RUST_LOG` syntax (falls back to `RUST_LOG`) | Implemented |
| `OXREAL_LINUX_DISPLAY` | `oxreal-window` | Force `wayland` or `x11` | Implemented |
| `OXREAL_GPU` | `oxreal-rhi` (all backends) | Choose a GPU by index or name (ADR-0012) | Implemented |
| `OXREAL_VULKAN_VALIDATION` | `oxreal-rhi-vulkan` | Force validation layers on (`1`) or off (`0`) (ADR-0012) | Implemented |
| `OXREAL_VULKAN_SYNC_VALIDATION` | `oxreal-rhi-vulkan` | `1` adds synchronization validation (needs validation on; device creation fails if unavailable). Set in CI | Implemented |
| `OXREAL_SLANGC` | `oxreal-shader-compiler` | Use a different `slangc` than the pinned one (logged as a warning) (ADR-0013) | Implemented |
| `OXREAL_REQUIRE_SPIRV_VAL` | shader-compiler tests | `1` makes a missing `spirv-val` a test failure (set in CI on Linux) | Implemented |

### Tests

- Unit tests live in a `#[cfg(test)] mod tests` at the bottom of the file. Cross-crate and
  global-state tests go in `tests/<topic>.rs`, which run as separate processes.
- **Test names are sentences** describing the behaviour:
  `degenerate_look_directions_never_produce_nan`, `oxreal_log_takes_precedence_over_rust_log`.
  Avoid names like `test_transform_2`.
- Assert with messages that show the values: `assert!(a.abs_diff_eq(b, EPS), "{a} != {b}")`.
- No dependence on wall-clock timing, test order, the current directory or OS path separators.

### Git

- Branches: `feat/…`, `fix/…`, `docs/…`, `ci/…`, `release/vX.Y.Z`, with a short kebab-case
  description.
- Commit titles: imperative mood, about 72 characters at most ("Add oxreal-math", not "Added" or
  "Adds"). The body explains **why** and what was verified.
- **Renaming a CI job changes its check name.** The branch ruleset requires checks by exact name
  (currently: `rustfmt`, `clippy + test (…)` ×3, `rustdoc`, `cargo-deny`, `vulkan (lavapipe, …)` ×2).
  Update the ruleset in the same change, or every PR waits forever for "Expected" checks.
- Avoid stacked PRs. If one is unavoidable, retarget it to `main` (or delete the base branch)
  **before** merging it. Otherwise it merges into the base branch, not `main`. This happened with
  #16 and needed a follow-up PR (#17).

## API design

- **Keep visibility minimal.** Default to private or `pub(crate)`. Everything `pub` is a promise.
- **No third-party types in public APIs** (ADR-0001), except the ones sanctioned by an ADR:
  - `glam` math types through `oxreal-math` (ADR-0011)
  - `raw-window-handle` traits on `oxreal-window::Window` (ADR-0015)
  - log macros re-exported by `oxreal-diagnostics` (ADR-0010)
- **`#[non_exhaustive]`** on public enums and structs that are expected to grow, such as errors
  and events. Not on plain data types like `PhysicalSize`.
- **`#[must_use]`** on functions whose only effect is their return value, and on `with_*`
  builders.
- **Validate, don't panic:** invalid input from users, files or the OS returns an error
  (`perspective_infinite_reversed` returns `ProjectionError`). `panic!` and `expect` are for
  violated internal invariants only. `expect` messages state the invariant.
- **Prefer types to flags.** Use an enum instead of a `bool` or string parameter when the
  meaning isn't obvious at the call site.
- **Derive what's meaningful:** `Debug` always, then `Clone`/`Copy`/`PartialEq`/`Eq`/`Hash` when
  they make sense for the type. `Default` only when there is an obvious default
  (`Transform::IDENTITY`).
- **No mutable global state.** Pass context explicitly (conventions.md).

## Documentation

- **Crate docs** (`//!` in `lib.rs`) state the crate's responsibility and **layer**, what it
  must not do, and link the ADR that governs it.
- **Every public item** has a doc comment. The first line is a one-sentence summary.
- Use the standard sections where they apply: `# Errors`, `# Panics`, `# Safety`, `# Examples`.
- **Examples compile and run** as doc tests. Prefer a short real example to prose.
- **Comments explain why, not what.** The code already says what. Reference ADRs and sections
  (`(ADR-0011 §4)`) when a line exists because of a decision.
- No commented-out code, and no `TODO` without an issue number.

## Shaders (Slang, ADR-0013)

These follow the conventions Cordite validated, adapted to Oxreal:

| Kind | Style | Examples |
|---|---|---|
| Files and modules | `UpperCamelCase.slang`, in `common/`, `passes/` or `materials/` | `common/Brdf.slang`, `passes/Tonemap.slang` |
| Types | `UpperCamelCase` | `SurfaceData`, `MeshVaryings` |
| Functions and variables | `lowerCamelCase` | `distributionGgx`, `viewDistance` |
| Constants | `k` + `UpperCamelCase` | `kPi`, `kMaxLights` |
| Entry points | `<purpose><Stage>`, with Stage one of `Vertex`, `Pixel` or `Compute` | `meshVertex`, `meshPixel`, `cullLightsCompute` |

- "Pixel", not "fragment", matching HLSL and Unreal terminology.
- **Use `SV_VulkanVertexID` and `SV_VulkanInstanceID`, not `SV_VertexID` / `SV_InstanceID`.** The
  HLSL semantics follow Direct3D rules (they exclude the base vertex and instance), so Slang
  subtracts Vulkan's base values. That needs the `DrawParameters` capability, which Oxreal does
  not require (Roadmap 2022 doesn't). The compiler's capability allowlist rejects such shaders.
- **Read buffers through pointers** (`Vertex* vertices` in the push constants) whose address
  comes from `Device::buffer_address`. Buffer data uses scalar block layout, so a Slang struct
  matches the Rust `#[repr(C)]` struct with the same fields in the same order. For example,
  `float2` + `float4` is 24 bytes with the `float4` at offset 8, with no std140/std430
  padding.
- Shared engine conventions (ADR-0011 depth and clip space, color space, bindless access) live
  in `common/` modules and are imported, never copied.
- Binding layout follows the RHI bind-model ADR.

## When a rule doesn't fit

Rules serve the code. If following one makes code worse, raise it in the PR. Change this
document in the same PR, so the rule and the code never disagree.
