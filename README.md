# Oxreal

An open-source, Rust-native 3D game development platform: engine, editor, asset pipeline and
tools. It has Unreal Engine-class ambitions and an editor that Unreal developers will find familiar.

> **Unreal-level ambition, Blender-level openness, Rust-level engineering discipline.**
> This describes long-term ambition. It is not a claim of current feature parity.

**Status:** Milestone 0 (architecture and infrastructure). No engine functionality exists yet.

## Principles

- **Greenfield.** The engine owns its architecture. It isn't built on Bevy, Fyrox, Godot or Unreal.
- **Windows and Linux first-class.** Editor, runtime, tools, builds and tests work on both platforms.
- **Vulkan first,** behind an engine-owned RHI so that DX12 and Metal remain possible.
- **Unreal-familiar UX.** Viewport, World Outliner, Details, Content Browser, Play in Editor.
- **Game-focused.** The engine targets real-time games. It is not ArchViz, film or a DCC replacement.
- **Modular.** A Cargo workspace with enforced layer boundaries. The runtime never depends on the editor.

## Documentation

| Document | Purpose |
|---|---|
| [Product requirements](docs/product/PRD.md) | What we're building and for whom |
| [Technical architecture](docs/architecture/TAD.md) | Layers, crates, dependency rules, core interfaces |
| [Decisions (ADRs)](docs/decisions/README.md) | Architecture decision records |
| [Open decisions & risks](docs/development/open-decisions.md) | What still needs research |
| [Development setup](docs/development/setup.md) | Toolchains, Vulkan SDK, Linux packages |
| [Coding standards](docs/development/coding-standards.md) | Naming, API design, documentation, Slang style, what CI enforces |
| [Glossary](docs/development/glossary.md) | One meaning per term: Actor vs Entity, Level vs Scene, and so on |
| [Conventions](docs/development/conventions.md) | Errors, logging, unsafe, testing, coordinates |
| [Prior art](docs/development/prior-art.md) | Lessons from earlier engines (Cordite) and how Oxreal applies them |
| [GPU support](docs/development/gpu-support.md) | Vulkan baseline and the hardware actually tested |
| [Contributing](CONTRIBUTING.md) | How to contribute |
| [Releasing](docs/development/releasing.md) | Versioning rules and the release checklist |
| [Code of Conduct](CODE_OF_CONDUCT.md) | Community standards and how to report problems |
| [Security policy](SECURITY.md) | How to report vulnerabilities privately |

## Building

```sh
cargo xtask ci     # fmt check + clippy + tests + rustdoc + layer check
```

See [setup](docs/development/setup.md) for platform prerequisites.

## License

Proposed: dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option,
pending dependency and legal review. Documentation is intended to be CC BY 4.0.
