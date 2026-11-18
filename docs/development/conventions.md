# Development Conventions

Some conventions here are provisional and marked with the open decision that will settle them.

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

- Library crates return typed errors: one error enum per crate or subsystem, implementing
  `std::error::Error`. Messages must be actionable: say what failed, on which resource or path,
  and what to try.
- Applications and tools may wrap errors with context at the top level.
- No `unwrap()` or `expect()` on fallible runtime paths in library code. `expect("reason")` is
  acceptable for true invariants, and the reason must explain the invariant.
- Never panic across FFI boundaries.
- Crate choice (thiserror / anyhow) is pending D-18.

## Logging and diagnostics

- Use structured diagnostics via `tracing` (pending D-18). No `println!` or `eprintln!` in
  library crates.
- Name GPU objects and profiling scopes from the start.
- Log levels: `error` means the user must act, `warn` means degraded, `info` is lifecycle,
  `debug` and `trace` are for development.

## `unsafe`

- Keep `unsafe` in the smallest possible module and expose a safe API.
- Every `unsafe` block has a `// SAFETY:` comment that names the invariants
  (`clippy::undocumented_unsafe_blocks` is denied).
- Every `unsafe fn` documents its contract in a `# Safety` doc section.
- Run new `unsafe` and FFI past the `unsafe-auditor` agent.

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

## Coordinates and units (provisional, D-17)

To be decided in an ADR before M1 rendering lands. Until then, don't hard-code assumptions; route
them through `foundation/math` constants.

## Commits and PRs

- Make small, coherent changes. Each PR states its scope, what was tested (and on which OS), and
  what remains incomplete.
- Architecture changes need an ADR in the same PR.
