# Architecture Decision Records

Each significant architectural decision gets an ADR, written with the template in
[`0000-template.md`](0000-template.md). Number ADRs sequentially and never reuse a number. To
change a decision, write a new ADR that supersedes the old one. Don't rewrite history.

Status values: **Proposed** (under review), **Accepted**, **Superseded**, **Deprecated**.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-greenfield-engine-architecture.md) | Greenfield engine architecture | Accepted |
| [0002](0002-windows-and-linux-first-class.md) | Windows and Linux as first-class platforms | Accepted |
| [0003](0003-vulkan-first-graphics-backend.md) | Vulkan as the first graphics backend behind an engine-owned RHI | Accepted |
| [0004](0004-runtime-editor-separation.md) | Runtime/editor separation and enforced crate layering | Accepted |
| [0005](0005-engine-owned-ecs-and-scene-hierarchy.md) | Engine-owned ECS and separate authoring hierarchy | Proposed |
| [0006](0006-asset-identity-and-non-destructive-reimport.md) | Stable asset identity and non-destructive reimport | Proposed |
| [0007](0007-unreal-source-as-ux-reference-only.md) | Unreal Engine source is a UX reference only | Accepted |
| [0008](0008-unreal-style-gameplay-framework-on-ecs.md) | Unreal-style gameplay framework on top of the engine ECS | Proposed |

Open questions that still need ADRs are tracked in
[`../development/open-decisions.md`](../development/open-decisions.md).
