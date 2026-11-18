# Contributing

Thanks for your interest! The project is in Milestone 0, so architecture review is the most
valuable contribution right now.

## Before you start

1. Read the [PRD](docs/product/PRD.md), the [TAD](docs/architecture/TAD.md) and the
   [ADRs](docs/decisions/README.md).
2. Identify the owning crate and its layer. Check the allowed dependencies (TAD §4).
3. For anything architectural, open a discussion or a proposed ADR first.

## Rules

- No Bevy, Fyrox, Godot or Unreal as a foundation (ADR-0001).
- **No Unreal Engine code, shaders or assets**, including paraphrased code (ADR-0007).
- Keep changes small and coherent, and consider both Windows and Linux.
- New third-party dependencies need an entry in `docs/development/dependencies.md`.
- `unsafe` requires `// SAFETY:` comments and a minimal surface.
- Bug fixes include a regression test.

## Checks

```sh
cargo xtask ci
cargo deny check
```

CI runs these on Windows and Linux.

## Pull requests

Describe the scope, what you tested (and on which OS and GPU), and what remains incomplete.
Include an ADR for architecture changes.

## Licensing

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this project, as defined in the Apache-2.0 license, shall be dual licensed as MIT OR Apache-2.0,
without any additional terms or conditions.
