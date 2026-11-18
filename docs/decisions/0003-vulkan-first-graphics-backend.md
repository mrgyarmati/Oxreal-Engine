# ADR-0003: Vulkan as the first graphics backend behind an engine-owned RHI

- **Status:** Accepted
- **Date:** 2026-11-18
- **Related:** PRD §5, TAD §5.3, brief §3, §6; open decisions D-03, D-04, D-14

## Context

We need one modern explicit API that runs on both Windows and Linux. Future DX12 (Xbox, Windows
tooling) and Metal (macOS) backends must stay possible.

## Options considered

- **Vulkan directly behind our RHI.** One API covers both OSes, and we get explicit control and
  modern features (dynamic rendering, synchronization2, descriptor indexing, ray tracing later).
- **wgpu as the RHI.** It's portable and safe, but its API would become our RHI and constrain
  bindless and GPU-driven features. It also adds a WebGPU abstraction layer we don't need.
- **DX12 first.** Windows-only, which violates ADR-0002.

## Decision

- An engine-owned RHI (`rendering/rhi`) that exposes no backend types.
- Vulkan is the first backend (`rendering/backends/vulkan`), targeting Vulkan 1.3 core as the
  baseline (pending D-14 driver-matrix confirmation).
- Surfaces are created from raw window and display handles supplied by `platform`.
- Validation layers are enabled in debug builds when available. Every GPU object gets a debug name.
- Binding choices (ash or another crate), the allocator, the shader language and the bind model
  are left to follow-up ADRs (D-03, D-04).

## Consequences

- More backend code than with wgpu, and a significant `unsafe` surface that has to be audited
  (`unsafe-auditor`).
- The RHI design must be checked against DX12 and Metal semantics so it doesn't hard-code Vulkan-isms.
- Real driver and hardware testing is still required. Vulkan does not remove driver variance.
