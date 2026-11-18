# Open Decisions and Risks

Do not guess. Each item is resolved by an ADR that documents context, options, trade-offs,
decision and consequences (use the `adr-author` agent). Update the status here when an ADR lands.

## Open decisions

| ID | Decision | Needed by | Status | ADR |
|---|---|---|---|---|
| D-01 | ECS storage (archetype / sparse set / hybrid), change detection, parallel scheduling. Must meet ADR-0008 §3 (relations, hooks, deferred commands, stages) | M1 | Open | (ADR-0005 partial) |
| D-02 | Reflection design: derive macro, type registry, attributes, stable type IDs. Metadata must be language-neutral (TAD §5.8) | before M2 | Open | |
| D-03 | Vulkan bindings (ash?) and GPU memory allocation strategy | M1 | Open | |
| D-04 | Shader language and compiler pipeline (HLSL via DXC, Slang, WGSL via naga, GLSL), reflection, hot reload | M1 | Open | |
| D-05 | Editor UI framework (egui prototype) and long-term custom widget needs | M2 | Open | |
| D-06 | Render graph / frame graph design | M1 to M5 | Open | |
| D-07 | Scene and project file format and versioning (RON? binary for cooked) and migration | M2 | Open | |
| D-08 | Asset DB storage and stable ID strategy details (sidecars vs central DB) | M3 | Open | (ADR-0006 partial) |
| D-09 | FBX importer: licensing and implementation options (Autodesk SDK license vs ufbx vs custom) | M3+ | Open | |
| D-10 | Physics backend comparison: Rapier vs Jolt (bindings, determinism, character controller) | M4 | Open | |
| D-11 | Rust gameplay module hot reload (dylib + stable ABI subset, WASM, or restart-based). Must be compatible with the D-21 C ABI boundary | M4 | Open | |
| D-12 | Visual scripting IR and execution model (interpreter first, later compilation) | M6 | Open | |
| D-13 | Plugin ABI and compatibility strategy (Rust has no stable ABI). The C ABI is the stable boundary (TAD §5.8, D-21) | M7 | Open | |
| D-14 | Linux packaging (tarball, Flatpak, AppImage) and graphics driver support matrix (Vulkan baseline version) | M1 / M7 | Open | |
| D-15 | Working title and crate name prefix | before first engine crate | **Resolved 2026-11-18**: Oxreal, crate prefix `oxreal-`. Checked against crates.io, GitHub and the web. A formal trademark search is still pending before the public launch | n/a |
| D-16 | Reference hardware for performance targets | M1 | Open | |
| D-17 | Math library and engine conventions: glam vs own, coordinate system, handedness, units | M1 | Open | |
| D-18 | Error-handling crates (thiserror / anyhow) and diagnostics stack (tracing, profiling: tracy / puffin) | M0 | Open | |
| D-19 | GPU testing in CI (lavapipe / SwiftShader software Vulkan, self-hosted GPU runners) | M1 | Open | |
| D-20 | Gameplay framework model: Actor/Pawn/Controller/GameMode concepts on ECS, behaviours, templates | before M4 (shapes M1 ECS) | Proposed | [ADR-0008](../decisions/0008-unreal-style-gameplay-framework-on-ecs.md) |
| D-21 | Additional languages: C++ and C# via a stable C ABI and generated bindings (.NET hosting model, header generation, ownership and threading rules, layer placement). Constraints already apply to D-02, D-11 and D-13 (TAD §5.8) | constraints now, ADR after the Rust gameplay API stabilizes | Open | |

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
