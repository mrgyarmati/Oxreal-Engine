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
| serde_json | 1.x | MIT OR Apache-2.0 | `tools/xtask`, `oxreal-shader-compiler` (tools); **`oxreal-renderer` (shipped in `oxreal-player`, and its build script)** | Parse `cargo metadata` output for the layer check; read `slangc` reflection, cook recipes and write cooked manifests; the renderer reads cooked manifests strictly as untrusted input. Always untyped `Value`s (no `serde` derive, ADR-0019 §4) | No | Very active, de-facto standard | Win + Linux | 2026-11-18 |
| tracing | 0.1.44 (`std` only, no proc-macro attributes) | MIT | `oxreal-diagnostics` | Structured log events and spans (ADR-0010) | Macros re-exported through `oxreal-diagnostics`. Callers never name the crate. | Stable 0.1 line for years. Released 2025-12. ~1.07B downloads. | Win + Linux (pure Rust) | 2026-11-19 |
| tracing-subscriber | 0.3.23 (`std`, `fmt`, `ansi`, `env-filter`, `smallvec`, `tracing-log`) | MIT | `oxreal-diagnostics` (`application` feature only) | Global subscriber, `OXREAL_LOG` filter, forwards `log` records | No (types boxed inside `InitError`) | Released 2026-03. ~762M downloads. | Win + Linux (pure Rust) | 2026-11-19 |
| thiserror | 2.0.21 | MIT OR Apache-2.0 | Every library crate with errors: `oxreal-diagnostics` (`application` feature), `-math`, `-window`, `-ecs`, `-hierarchy`, `-rhi`, `-renderer`, `-shader-compiler`, `-shader-tool` | Derives `Error`/`Display` for library error enums (ADR-0010) | No (generates standard trait impls only) | Released 2026-09. ~1.89B downloads. | Win + Linux (proc macro) | 2026-11-19 |
| glam | 0.34.3 (`std`, `f64`, `i32`, `u32`; no defaults) | MIT OR Apache-2.0 | `oxreal-math` | Vector, matrix and quaternion math (ADR-0011) | **Yes, deliberately** (ADR-0011 §1): types re-exported through `oxreal-math`. Callers never name `glam`. A breaking glam upgrade is a breaking Oxreal change. | Very active, 0.34.3 released 2026-11-08. ~194M downloads. | Win + Linux (pure Rust, no dependencies with these features) | 2026-11-19 |
| winit | 0.30.13 (`rwh_06`, `x11`, `wayland`, `wayland-dlopen`, `wayland-csd-adwaita-notitle`; no defaults) | Apache-2.0 | `oxreal-window` | Windows and event loop on Win32, Wayland and X11 (ADR-0015) | No (wrapped. Engine `App` trait and event enum.) | Very active, 0.30.13 stable (2026-03), 0.31 in beta. ~68M downloads. | Win + Linux. Linux libraries loaded at runtime (dlopen), no build-time system packages. | 2026-11-19 |
| raw-window-handle | 0.6.2 | MIT OR Apache-2.0 OR Zlib | `oxreal-window` (and the Vulkan backend later) | Native window and display handles for surface creation | **Yes, deliberately** (ADR-0015 §2): `Window` implements its traits | Stable 0.6 line since 2024-05. ~139M downloads. Ecosystem standard. | Win + Linux (pure Rust) | 2026-11-19 |
| sha2 | 0.11.1 | MIT OR Apache-2.0 | `tools/xtask`, `oxreal-shader-compiler` (tools); **`oxreal-renderer` (shipped in `oxreal-player`, and its build script)** | Verifying downloaded tool archives (Slang) against pinned SHA-256 (ADR-0013); cooked shader source keys and file hashes, computed by the cook and checked by the renderer (ADR-0019 §4) | No | RustCrypto. Released 2026-10. ~1.26B downloads. | Win + Linux (pure Rust) | 2026-11-19 |
| flate2 | 1.1.11 (default: pure-Rust `miniz_oxide`) | MIT OR Apache-2.0 | `tools/xtask` (dev tooling only) | Decompressing tool archives | No (not shipped) | Released 2026-11. ~871M downloads. | Win + Linux (pure Rust) | 2026-11-19 |
| tar | 0.4.46 | MIT OR Apache-2.0 | `tools/xtask` (dev tooling only) | Unpacking tool archives (refuses entries escaping the target directory) | No (not shipped) | Released 2026-05. ~296M downloads. | Win + Linux | 2026-11-19 |
| ash | 0.38.0+1.3.281 (`loaded`, `debug`, `std`; no defaults) | MIT OR Apache-2.0 | `oxreal-rhi-vulkan` | Vulkan bindings, with the loader loaded at runtime (ADR-0012) | No (confined to `oxreal-rhi-vulkan`) | Repository active (2026-11). Latest release 2024-04 (Vulkan 1.3.281). ~54M downloads. | Win + Linux. Runtime loading via `libloading` (ISC). | 2026-11-19 |
| ash-window | 0.13.0 | MIT OR Apache-2.0 | `oxreal-rhi-vulkan` | Vulkan surfaces from `raw-window-handle` (Win32, Wayland, Xlib, XCB) (ADR-0012) | No | Same maintainers as `ash`. Released 2024-04. Small, stable. | Win + Linux (pure Rust) | 2026-11-19 |
| gpu-allocator | 0.28.0 (`std`, `vulkan`; no defaults) | MIT OR Apache-2.0 | `oxreal-rhi-vulkan` | GPU memory sub-allocation (ADR-0012) | No (behind the RHI memory module) | Very active (repository 2026-11-18). ~33M downloads. Logs via `log`, forwarded by `tracing-log`. | Win + Linux (pure Rust) | 2026-11-19 |
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

**Audit notes, 2026-11-19 (`oxreal-window`):**
- Transitive tree (normal and build dependencies): **39 crates on Windows, 87 on Linux**. Linux
  adds the Wayland protocols, xkbcommon bindings, X11 bindings, and `sctk-adwaita` with
  `tiny-skia` for client-side decorations. All licenses are within `deny.toml`'s
  allow-list:
  - MIT, Apache-2.0, Zlib, ISC (`libloading`)
  - BSD-2-Clause (`arrayref`), BSD-3-Clause (`tiny-skia`)
  - Unicode-3.0 (`unicode-ident`)
- `rustix` and `linux-raw-sys` appear in two versions each on Linux (`cargo-deny` warns but does
  not fail). This is expected to resolve with upstream updates.
- Linux builds need no windowing development packages. CI builds before installing anything and
  then installs only the runtime libraries for the smoke test.
- **RUSTSEC-2026-0192** (`ttf-parser` unmaintained, no fixed version): winit's default
  `wayland-csd-adwaita` feature pulled it in through `ab_glyph`, and `cargo-deny` failed on it.
  It was resolved by using `wayland-csd-adwaita-notitle` (ADR-0015 §1), not by ignoring the
  advisory. The tree was rechecked: `ttf-parser`, `owned_ttf_parser` and both `ab_glyph` crates
  are gone.
- `deny.toml` now checks only the supported targets (`x86_64-pc-windows-msvc`,
  `x86_64-unknown-linux-gnu`), so crates that only macOS, Android or Redox would build no longer
  raise findings (ADR-0002).

## Candidates under evaluation (not yet adopted)

These come from brief §11 and later ADRs, and are not commitments until a crate adopts them:
- Rapier and Jolt bindings
- Kira
- egui
- serde and RON
- zstd
- rayon
