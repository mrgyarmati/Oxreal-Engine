# Contributing

Thanks for your interest! The project is in Milestone 0, so architecture review is the most
valuable contribution right now.

Everyone taking part in Oxreal spaces (issues, pull requests, Discussions) is expected to follow
the [Code of Conduct](CODE_OF_CONDUCT.md), which also explains how to report problems
confidentially.

## Before you start

1. Read the [PRD](docs/product/PRD.md), the [TAD](docs/architecture/TAD.md) and the
   [ADRs](docs/decisions/README.md).
2. Identify the owning crate and its layer. Check the allowed dependencies (TAD §4).
3. For anything architectural, start a thread in
   [Discussions → Ideas](https://github.com/mrgyarmati/Oxreal-Engine/discussions/categories/ideas)
   or open a proposed ADR first.

## Where to talk

| Channel | Use it for |
|---|---|
| [Discussions → Q&A](https://github.com/mrgyarmati/Oxreal-Engine/discussions/categories/q-a) | Questions about building, using or contributing |
| [Discussions → Ideas](https://github.com/mrgyarmati/Oxreal-Engine/discussions/categories/ideas) | Early ideas and design conversations before an issue or ADR |
| [Discussions → Show and tell](https://github.com/mrgyarmati/Oxreal-Engine/discussions/categories/show-and-tell) | Things you've built or experimented with |
| [Issues](https://github.com/mrgyarmati/Oxreal-Engine/issues/new/choose) | Confirmed bugs and concrete feature proposals (use the forms) |
| [Security advisories](https://github.com/mrgyarmati/Oxreal-Engine/security/advisories/new) | Vulnerabilities. Report them privately, never in public (see `SECURITY.md`) |

## Rules

- No Bevy, Fyrox, Godot or Unreal as a foundation (ADR-0001).
- **No Unreal Engine code, shaders or assets**, including paraphrased code (ADR-0007).
- Keep changes small and coherent, and consider both Windows and Linux.
- New third-party dependencies need an entry in `docs/development/dependencies.md`.
- `unsafe` requires `// SAFETY:` comments and a minimal surface.
- Bug fixes include a regression test.
- User-visible changes get an entry in `CHANGELOG.md` under `[Unreleased]`.

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
