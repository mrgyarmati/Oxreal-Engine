# ADR-0010: Errors, logging and profiling

- **Status:** Accepted
- **Date:** 2026-11-19
- **Related:** brief §14 ("CPU and GPU profiling from early milestones"), conventions.md, ADR-0003 (GPU object naming), D-18

## Context

Every engine crate needs the same answers from its first line of code: how errors are typed and
reported, how diagnostics are emitted, and how CPU and GPU time is measured. These choices touch
every public API, so changing them later is expensive. The constraints:

- Errors must be **actionable**: what failed, on which resource, and what to try.
- **Libraries never install global state** (logger, subscriber, profiler). Applications do.
- Gameplay code and plugins must not have to depend on a specific logging or profiling crate.
  Third-party types must not leak into engine APIs (ADR-0001).
- Profiling must cover CPU **and GPU** on Windows and Linux, and cost nothing when disabled.
- Shipped player builds must not open profiler network ports by default.

Versions below were checked on crates.io on 2026-11-19.

## Options considered

### Errors
- **Hand-written `Error` impls.** No dependency, but repetitive and easy to get inconsistent.
- **`thiserror` 2.0.21** (MIT OR Apache-2.0, very widely used). It derives `std::error::Error`
  and `Display` for our own enums. It generates standard trait impls only, so nothing from
  `thiserror` appears in our public API.
- **`snafu` / `error-stack`.** Richer context capture, but more ceremony, and error-stack's
  report type would become part of public signatures.
- **`anyhow` 1.0.104** (MIT OR Apache-2.0) adds type-erased errors with context. It suits
  applications and tools, not library APIs that callers need to match on.
- **`miette` 7.6** (Apache-2.0) gives rich diagnostic rendering with source spans. It's
  attractive for user-facing tool diagnostics such as shader and asset errors, but too heavy for
  core library error types.

### Logging and diagnostics
- **`log` facade.** Simple, but no spans and only limited structured fields.
- **`tracing` 0.1.44 + `tracing-subscriber` 0.3.23** (MIT). Structured events plus spans, a
  layered subscriber model (a custom layer can feed the editor's Output Log), and the de-facto
  standard in the Rust ecosystem.
- **Our own logger.** Full control, but it would reinvent a mature ecosystem with no benefit.

### Profiling
- **Tracy via `tracy-client` 0.19.0** (MIT/Apache-2.0; the bundled Tracy C++ client is
  BSD-3-Clause, which `deny.toml` allows). It covers CPU zones, **GPU zones** (`GpuContext`,
  `GpuSpan`, which suits Vulkan timestamp queries), frame marks, plots, memory and locks, on
  Windows and Linux. It's an industry-standard tool. Features include `ondemand` and
  `only-localhost`. It needs the Tracy viewer, whose protocol version must match the client's.
- **`puffin` 0.20.0** (MIT OR Apache-2.0). Pure Rust with an in-app egui viewer, so it's a good
  candidate for an in-editor overlay. It's CPU only.
- **`profiling` 1.0.18** (MIT OR Apache-2.0) is a facade over several profilers. It's close to
  what we need, but GPU zones and frame-mark semantics would stay outside our control.
- **`tracing` spans as profiling zones** (via `tracing-tracy`). Convenient, but span overhead in
  hot loops is higher than dedicated profiler zones, and it couples two concerns.

## Decision

### 1. Errors
- Each library crate (or subsystem) defines its own error enum(s) with **`thiserror`**, marked
  `#[non_exhaustive]` when they are public. Underlying causes are chained with `#[source]`.
  Messages are lowercase, have no trailing period, and name the resource involved, for example
  `failed to create swapchain for window 'Main' (2560x1440): surface lost`.
- **Applications and tools** (`app-*`, `tool` layers) may use **`anyhow`** with `.context(...)`
  at the top level. Library APIs never return `anyhow::Error`.
- **Panics are for bugs only** (violated invariants), never for recoverable conditions such as
  missing files, invalid assets or device loss. Panics never cross FFI boundaries; future C ABI
  entry points catch them (D-21). Builds keep `panic = "unwind"`.
- `miette` stays a candidate for user-facing diagnostics in tools and the editor (shader and
  asset errors), decided when the first such tool exists.

### 2. Logging and diagnostics
- All crates emit diagnostics through **`tracing`** (events and spans) with structured fields.
  Targets default to the crate path, and level semantics follow `conventions.md`.
- **Engine and gameplay code use logging macros re-exported by `oxreal-diagnostics`**
  (foundation layer), not `tracing` directly. Gameplay modules and plugins therefore depend only
  on an Oxreal crate, and the backend stays replaceable without touching call sites.
- **Only applications install a subscriber**, using `tracing-subscriber` with an environment
  filter (`OXREAL_LOG`, `RUST_LOG`-style syntax) and a console formatter. They do this through a
  helper in `oxreal-diagnostics` behind an application-only feature. The editor's Output Log will
  be a custom subscriber layer.

### 3. Profiling
- **Engine-owned profiling macros** in `oxreal-diagnostics`: `profile_scope!`,
  `profile_function!`, frame marks and plots. GPU zones are exposed through RHI-level helpers.
  Profiler types never appear in engine APIs.
- **Tracy is the first backend**, behind the Cargo feature `profile-tracy` (off by default).
  With the feature off, the macros compile to nothing.
- `tracy-client` is used with `default-features = false`, enabling `enable`, `ondemand` and
  `only-localhost`. That means **no network broadcast and no listening on external interfaces**,
  and data is collected only while a viewer is connected. Profiling builds are opt-in, and
  shipped player builds never enable the feature by default.
- The required Tracy viewer version is documented in `docs/development/setup.md` and updated
  whenever `tracy-client` is bumped.
- `puffin` is evaluated later as a second, in-editor backend behind the same macros.

### 4. Ownership
`oxreal-diagnostics` (layer `foundation`) is the first engine crate. It holds the re-exported
logging macros, the profiling macros and the application-side initialization helper. It is
created when the first crate needs it. Dependencies are registered in `dependencies.md` in that
crate's PR.

## Consequences

- Every crate has the same error style and diagnostics from day one, and the editor Output Log,
  log filtering and profiling all plug into one pipeline.
- Gameplay code and plugins depend on `oxreal-diagnostics` only, so swapping or adding backends
  is local.
- Profiling has zero cost unless it's built in, and profiling builds don't expose network
  services beyond localhost.
- Costs: four third-party dependencies (`thiserror`, `anyhow` for apps and tools, `tracing`,
  `tracing-subscriber`) plus optional `tracy-client`. A matching Tracy viewer version must be
  documented and maintained.
- Revisit triggers: measurable diagnostics overhead in hot paths (benchmarks), `tracing`
  reaching 0.2 with breaking changes, or a need for in-editor profiling that Tracy can't cover.
