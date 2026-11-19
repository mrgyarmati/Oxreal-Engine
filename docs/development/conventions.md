# Development Conventions

Engine-wide conventions for errors, logging, profiling, `unsafe`, platform code, testing, and
math and coordinates. Naming, API shape, documentation and shader style are in
[coding-standards.md](coding-standards.md), and terms are defined in [glossary.md](glossary.md).

## Workspace and crates

- One Cargo workspace. Crates live under `crates/<layer>/<name>`, executables under
  `applications/<name>`, and dev tooling under `tools/<name>`.
- Every crate has:
  - `[package.metadata.engine] layer = "<layer>"` (see TAD §2). `cargo xtask layers` enforces it.
  - `[lints] workspace = true`
  - `edition.workspace = true`, `rust-version.workspace = true`, `license.workspace = true`
  - A crate-level doc comment (`//!`) stating its responsibility and what it must not do.
- Shared third-party dependency versions go in `[workspace.dependencies]`.
- Don't create a crate without real code and a defined responsibility (use the `new-crate` skill).

## Errors

These rules follow [ADR-0010](../decisions/0010-errors-logging-and-profiling.md).

- Library crates return typed errors: one error enum per crate or subsystem, derived with
  `thiserror`, marked `#[non_exhaustive]` when public, with causes chained through `#[source]`.
- Messages must be actionable: lowercase, no trailing period, and naming the resource involved,
  for example `failed to load texture 'Content/rock.ktx2': unsupported format BC6H_SF16`.
- Applications and tools (`app-*`, `tool`) may use `anyhow` with `.context(...)` at the top
  level. Library APIs never return `anyhow::Error`.
- Panics are for bugs (violated invariants) only. No `unwrap()` on fallible runtime paths in
  library code. `expect("reason")` is acceptable for true invariants, and the reason must
  explain the invariant.
- Never panic across FFI boundaries.

## Logging and diagnostics

- Emit diagnostics with the logging macros re-exported by `oxreal-diagnostics` (backed by
  `tracing`), with structured fields such as `info!(window = %title, width, height, "swapchain created")`.
  Engine and gameplay code don't depend on `tracing` directly.
- No `println!` or `eprintln!` in library crates. Only applications install a log subscriber.
- Log levels: `error` means the user must act, `warn` means degraded, `info` is lifecycle,
  `debug` and `trace` are for development.

## Profiling

- Instrument with the `oxreal-diagnostics` macros (`profile_function!`, `profile_scope!`, frame
  marks). They compile to nothing unless the `profile-tracy` feature is enabled.
- Name GPU objects and profiling scopes from the start. GPU zones go through RHI helpers.
- Profiling builds are opt-in, and player builds never enable a profiler by default.

## `unsafe`

- Keep `unsafe` in the smallest possible module and expose a safe API.
- Every `unsafe` block has a `// SAFETY:` comment that names the invariants
  (`clippy::undocumented_unsafe_blocks` is denied).
- Every `unsafe fn` documents its contract in a `# Safety` doc section.
- Run new `unsafe` and FFI past the `unsafe-auditor` agent.
- **A safe API must stay sound under any safe caller,** not just well-behaved ones. In particular:
  - Soundness can't depend on a `Drop` guard running, because `std::mem::forget` is safe. Check
    again at the next point you control, for example submission.
  - It can't depend on public constructors being used "correctly": take the data you act on
    from your own state.
  - It can't depend on a handle's owner still being alive: share ownership (`Arc`) or tie
    the lifetimes.
  - It can't depend on two threads not interleaving: hold the lock across every step that
    must be atomic, and keep a documented lock order.
- When a safety property can't be checked at reasonable cost (for example the validity of
  shader code), the constructor that accepts the input is `unsafe` with a `# Safety` contract.
  The callee still checks what it can.

## State

- No `static mut`. No global mutable singletons for engine state. Pass contexts explicitly or
  store state in the World or resources.
- Process-wide services (logging subscriber, allocator) are initialized once by the application,
  never by a library.

## Platform

- Only `crates/platform/*` contains `cfg(target_os = ...)` / `cfg(windows)`. Anywhere else it
  needs a justification comment.
- Paths: use `std::path::Path` and never hard-code separators. Treat asset paths as
  case-sensitive (Linux), and warn when two paths differ only in case.
- Text files are checked in with LF (see `.gitattributes`).

## Testing

- Unit tests live next to the code (`#[cfg(test)]`). Cross-crate tests go in `tests/`.
- Every bug fix gets a regression test.
- Tests must pass on Windows and Linux. Mark GPU-requiring tests `#[ignore]` with a reason until
  GPU CI exists (D-19).
- Benchmarks go in `benchmarks/` and must name their hardware, content and workload.

## Math, coordinates and units

These rules follow [ADR-0011](../decisions/0011-math-library-and-coordinate-conventions.md).

- Import math types from `oxreal_math` (which re-exports `glam`), never from `glam` directly.
- **Right-handed, +Y up, +X right, forward is −Z.** Identity rotation and cameras face −Z.
- **SI units:** metres, kilograms, seconds. The editor may display other units, but data never
  stores them.
- Rotations are unit quaternions. Angles are **radians** in code and **degrees** in UI and
  human-edited files.
- Column-major matrices and column vectors, with `world = parent * local`.
- Clip space is **Y up, depth [0, 1]** on every backend. Vulkan flips with a negative viewport
  height.
- Front faces wind **counter-clockwise** as seen by the camera (glTF does the same).
  `CullMode::Back` removes clockwise triangles.
- Scene depth is **reversed** with an infinite far plane: `D32_SFLOAT`, cleared to 0, compared
  with `GREATER_OR_EQUAL`. Use the `oxreal_math` projection functions. Never build projection
  matrices by hand.
- Do all color math in linear RGB. Convert to or from sRGB only at storage and display boundaries.

## Commits and PRs

- Make small, coherent changes. Each PR states its scope, what was tested (and on which OS), and
  what remains incomplete.
- Architecture changes need an ADR in the same PR.
