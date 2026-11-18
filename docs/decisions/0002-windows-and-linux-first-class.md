# ADR-0002: Windows and Linux as first-class platforms

- **Status:** Accepted
- **Date:** 2026-11-18
- **Related:** PRD §5, brief §3

## Context

Native Linux game development is a differentiator for target users. Platforms that are added late
tend to stay second-class: paths, case sensitivity, windowing (X11 and Wayland), drivers and
packaging all diverge.

## Decision

- Windows 10/11 x64 and Linux x86_64 are both required for the editor, runtime, tools, builds and
  tests from M0.
- CI runs fmt, clippy and tests on `windows-latest` and `ubuntu-latest` for every PR.
- Platform-specific code lives only in the `platform` layer behind traits. Other layers contain no
  `cfg(target_os)` unless it's justified.
- Linux windowing supports both Wayland and X11.
- "Supported on Linux" means run and tested on Linux, not just compiled. Validation distros are
  Ubuntu LTS, Fedora and Arch.
- Hardware validation across NVIDIA, AMD and Intel GPUs is planned. GPU tests will run on
  self-hosted runners when available.

## Consequences

- Every feature costs some portability work, which is cheaper now than retrofitting later.
- Developers on one OS depend on CI (and the `cross-platform-reviewer` agent) for the other.
- Hosted CI has no GPUs, so rendering tests need a headless or software path (for example lavapipe
  on Linux) plus later hardware runners. See D-19.
