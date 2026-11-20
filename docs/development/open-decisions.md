# Open Decisions and Risks

Do not guess. Each item is resolved by an ADR that documents context, options, trade-offs,
decision and consequences (use the `adr-author` agent). Update the status here when an ADR lands.

## Open decisions

| ID | Decision | Needed by | Status | ADR |
|---|---|---|---|---|
| D-01 | ECS storage (archetype / sparse set / hybrid), change detection, parallel scheduling. Must meet ADR-0008 §3 (relations, hooks, deferred commands, stages; carried into ADR-0020 §5) | M1 | **Resolved**: private storage, archetype tables (sparse opt-in on evidence), relation components, hooks, commands, change ticks, serial-then-parallel scheduler | [ADR-0014](../decisions/0014-ecs-architecture-and-storage.md) |
| D-02 | Reflection design: derive macro, type registry, attributes, stable type IDs. Metadata must be language-neutral (TAD §5.8) | before M2 | Open | |
| D-03 | Vulkan bindings (ash?) and GPU memory allocation strategy | M1 | **Resolved**: `ash` 0.38 (runtime-loaded), `ash-window`, `gpu-allocator` 0.28 behind the RHI memory module | [ADR-0012](../decisions/0012-vulkan-baseline-bindings-and-memory.md) |
| D-04 | Shader language and compiler pipeline (HLSL via DXC, Slang, WGSL via naga, GLSL), reflection, hot reload | M1 | **Resolved**: Slang via the pinned `slangc` CLI, reflection JSON, capability allowlist, no compiler in shipped games | [ADR-0013](../decisions/0013-shader-language-and-compiler-pipeline.md) |
| D-05 | Editor UI framework (egui prototype) and long-term custom widget needs | M2 | Open | |
| D-06 | Render graph / frame graph design | M1 to M5 | Open | |
| D-07 | Scene and project file format and versioning (RON? binary for cooked) and migration, and instantiating levels into worlds (ADR-0020 §4) | M2 | Open | |
| D-08 | Asset DB storage and stable ID strategy details (sidecars vs central DB) | M3 | Open | (ADR-0006 partial) |
| D-09 | FBX importer: licensing and implementation options (Autodesk SDK license vs ufbx vs custom) | M3+ | Open | |
| D-10 | Physics backend comparison: Rapier vs Jolt (bindings, determinism, character controller) | M4 | Open | |
| D-11 | Rust gameplay module hot reload (dylib + stable ABI subset, WASM, or restart-based). Must be compatible with the D-21 C ABI boundary | M4 | Open | |
| D-12 | Visual scripting IR and execution model (interpreter first, later compilation), run by scheduled executor systems with declared access (ADR-0020 §3) | M6 | Open | |
| D-13 | Plugin ABI and compatibility strategy (Rust has no stable ABI). The C ABI is the stable boundary (TAD §5.8, D-21) | M7 | Open | |
| D-14 | Linux packaging (tarball, Flatpak, AppImage) and graphics driver support matrix (Vulkan baseline version) | M1 / M7 | **Driver part resolved**: Vulkan 1.3 baseline, tested matrix in `gpu-support.md`. Packaging still open | [ADR-0012](../decisions/0012-vulkan-baseline-bindings-and-memory.md) (driver part) |
| D-15 | Working title and crate name prefix | before first engine crate | **Resolved 2026-11-18**: Oxreal, crate prefix `oxreal-`. Checked against crates.io, GitHub and the web. A formal trademark search is still pending before the public launch | n/a |
| D-16 | Reference hardware for performance targets | M1 | Open | |
| D-17 | Math library and engine conventions: glam vs own, coordinate system, handedness, units | M1 | **Resolved**: glam re-exported by `oxreal-math`; right-handed +Y up, metres, −Z forward; Y-up [0, 1] clip space; reversed infinite depth | [ADR-0011](../decisions/0011-math-library-and-coordinate-conventions.md) |
| D-18 | Error-handling crates (thiserror / anyhow) and diagnostics stack (tracing, profiling: tracy / puffin) | M0 | **Resolved**: thiserror (libraries), anyhow (apps and tools), tracing through `oxreal-diagnostics`, Tracy behind `profile-tracy` | [ADR-0010](../decisions/0010-errors-logging-and-profiling.md) |
| D-19 | GPU testing in CI (lavapipe / SwiftShader software Vulkan, self-hosted GPU runners) | M1 | Open | |
| D-20 | Gameplay framework model: Actor/Pawn/Controller/GameMode concepts on ECS, per-object logic, templates | before M4 (shapes M1 ECS) | **Resolved**: capabilities, relations and systems on the ECS; events and scheduled state machines/graphs for per-object logic; "Actor" is an authoring term; no per-object callbacks. Supersedes ADR-0008 | [ADR-0020](../decisions/0020-capability-oriented-gameplay-on-the-ecs.md) |
| D-21 | Additional languages: C++ and C# via a stable C ABI and generated bindings (.NET hosting model, header generation, ownership and threading rules, layer placement). Constraints already apply to D-02, D-11 and D-13 (TAD §5.8) | constraints now, ADR after the Rust gameplay API stabilizes | Open | |
| D-22 | Gameplay events and observers: ordering relative to stages and deferred commands, lifetime, delivery guarantees, declared readers and writers (ADR-0020 §3) | before gameplay code relies on events (before M4) | Open | |

## Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Scope is enormous for a small team | Milestones slip, and quality drops if corners are cut | Vertical slices, strict milestone exit criteria, extend the schedule rather than cut platforms |
| Linux GPU and windowing variance (Wayland vs X11, Mesa vs proprietary drivers) | Bugs show up late | Linux CI from M0, real hardware testing on the 3 GPU vendors, validation layers on |
| `unsafe` surface in the Vulkan backend | UB, crashes, driver hangs | Isolate it in one crate, SAFETY comments, `unsafe-auditor` reviews, validation layers in CI |
| Reflection designed too late | Retrofitting across all subsystems | Decide D-02 before M2 and prototype it early |
| egui coupling leaks into the editor architecture | Expensive UI replacement | UI adapter boundary, commands/selection/undo kept toolkit-agnostic |
| Rust lacks a stable ABI | Plugins, hot reload and other languages are hard | Research D-11 and D-13 early, keep plugin surfaces narrow, use the C ABI as the single stable boundary (D-21) |
| Rust-only API design leaks into gameplay surfaces | C++ and C# support later needs a rewrite | Apply the TAD §5.8 constraints when deciding reflection, the plugin ABI and gameplay APIs |
| FBX licensing | Legal exposure or blocked feature | glTF first. FBX only after a licensing review (D-09) |
| Unreal-derived code contamination | License violation | ADR-0007 clean-room rules and review |
| Dependency churn or abandonment | Forced rewrites | Audit before adoption, keep dependencies behind engine interfaces |
| Hosted CI has no GPU | Rendering regressions are missed | Software Vulkan in CI plus self-hosted hardware runners (D-19) |
