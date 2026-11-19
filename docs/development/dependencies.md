# Third-Party Dependency Register

Every direct third-party dependency must be listed here before it's merged, with the audit result
(use the `dependency-auditor` agent). `deny.toml` enforces license and ban policy mechanically.
This file records the *why*.

Columns:
- **Crate** and **Version**
- **License**
- **Used by**: crates or tools that depend on it
- **Purpose**
- **Exposed in public API?**: should almost always be "no"
- **Maintenance**: last release, activity
- **Platforms**: Windows and Linux status
- **Audited**: date

| Crate | Version | License | Used by | Purpose | Exposed in public API? | Maintenance | Platforms | Audited |
|---|---|---|---|---|---|---|---|---|
| serde_json | 1.x | MIT OR Apache-2.0 | `tools/xtask` (dev tooling only) | Parse `cargo metadata` output for the layer check | No (not shipped) | Very active, de-facto standard | Win + Linux | 2026-11-18 |
| tracing | 0.1.44 (`std` only, no proc-macro attributes) | MIT | `oxreal-diagnostics` | Structured log events and spans (ADR-0010) | Macros re-exported through `oxreal-diagnostics`. Callers never name the crate. | Stable 0.1 line for years. Released 2025-12. ~1.07B downloads. | Win + Linux (pure Rust) | 2026-11-19 |
| tracing-subscriber | 0.3.23 (`std`, `fmt`, `ansi`, `env-filter`, `smallvec`, `tracing-log`) | MIT | `oxreal-diagnostics` (`application` feature only) | Global subscriber, `OXREAL_LOG` filter, forwards `log` records | No (types boxed inside `InitError`) | Released 2026-03. ~762M downloads. | Win + Linux (pure Rust) | 2026-11-19 |
| thiserror | 2.0.21 | MIT OR Apache-2.0 | `oxreal-diagnostics` (`application` feature) | Derives `Error`/`Display` for library error enums (ADR-0010) | No (generates standard trait impls only) | Released 2026-09. ~1.89B downloads. | Win + Linux (proc macro) | 2026-11-19 |
| tracy-client | 0.19.0 (`enable`, `ondemand`, `only-localhost`; no defaults) | MIT/Apache-2.0. Bundled Tracy C++ client (`tracy-client-sys` 0.30.1) is BSD-3-Clause. | `oxreal-diagnostics` (`profile-tracy` feature only) | Tracy profiler zones and frame marks | No (wrapped by `profile_*` macros) | Released 2026-08. ~14M downloads. | Win + Linux. Builds C++ with `cc` (MSVC / gcc / clang). | 2026-11-19 |

**Audit notes, 2026-11-19 (`oxreal-diagnostics`):**
- Transitive tree with all features: 33 crates on Windows and 29 on Linux, from `cargo metadata`
  with normal and build dependencies only. Every license is within `deny.toml`'s allow-list.
  - The only non-MIT/Apache licenses are BSD-3-Clause (the Tracy C++ client) and Unicode-3.0
    (`unicode-ident`).
  - `aho-corasick` and `memchr` are "Unlicense OR MIT", used under MIT.
- With **default features**, a library using `oxreal-diagnostics` pulls in only `tracing`,
  `tracing-core`, `once_cell` and `pin-project-lite`.
- `tracy-client`'s `span!` and `frame_mark()` **panic** if the client isn't running (verified in
  its source). Oxreal's macros therefore use `Client::running()` and never panic.
- `tracy-client` default features enable network `broadcast`, `system-tracing`, sampling and a
  crash handler. All are disabled deliberately (ADR-0010 §3).

## Candidates under evaluation (not yet adopted)

These come from brief §11 and later ADRs, and are not commitments until a crate adopts them:
- glam (ADR-0011)
- ash, ash-window, gpu-allocator and raw-window-handle (ADR-0012)
- winit
- Rapier and Jolt bindings
- Kira
- egui
- serde and RON
- zstd
- rayon
