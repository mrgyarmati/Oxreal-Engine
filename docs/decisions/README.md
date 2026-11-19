# Architecture Decision Records

Each significant architectural decision gets an ADR, written with the template in
[`0000-template.md`](0000-template.md). Number ADRs sequentially and never reuse a number. To
change a decision, write a new ADR that supersedes the old one. Don't rewrite history.

Status values: **Proposed** (under review), **Accepted**, **Superseded**, **Deprecated**.

**How a decision is made.** An ADR arrives in a pull request.
- If the PR sets the status to **Accepted**, merging it accepts the decision, and the PR review
  is the decision discussion.
- A **Proposed** ADR may be merged to record a direction that still needs research. It is
  accepted later by a follow-up PR that changes only its status (and any refinements).
- Implementation that depends on a decision waits until its ADR is Accepted.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-greenfield-engine-architecture.md) | Greenfield engine architecture | Accepted |
| [0002](0002-windows-and-linux-first-class.md) | Windows and Linux as first-class platforms | Accepted |
| [0003](0003-vulkan-first-graphics-backend.md) | Vulkan as the first graphics backend behind an engine-owned RHI | Accepted |
| [0004](0004-runtime-editor-separation.md) | Runtime/editor separation and enforced crate layering | Accepted |
| [0005](0005-engine-owned-ecs-and-scene-hierarchy.md) | Engine-owned ECS and separate authoring hierarchy | Accepted |
| [0006](0006-asset-identity-and-non-destructive-reimport.md) | Stable asset identity and non-destructive reimport | Proposed |
| [0007](0007-unreal-source-as-ux-reference-only.md) | Unreal Engine source is a UX reference only | Accepted |
| [0008](0008-unreal-style-gameplay-framework-on-ecs.md) | Unreal-style gameplay framework on top of the engine ECS | Proposed |
| [0009](0009-versioning-and-release-policy.md) | Versioning and release policy | Accepted |
| [0010](0010-errors-logging-and-profiling.md) | Errors, logging and profiling | Accepted |
| [0011](0011-math-library-and-coordinate-conventions.md) | Math library and coordinate conventions | Accepted |
| [0012](0012-vulkan-baseline-bindings-and-memory.md) | Vulkan baseline, bindings and GPU memory allocation | Accepted |
| [0013](0013-shader-language-and-compiler-pipeline.md) | Shader language and compiler pipeline | Accepted |
| [0014](0014-ecs-architecture-and-storage.md) | ECS architecture and storage | Accepted |
| [0015](0015-windowing-and-platform-event-loop.md) | Windowing and the platform event loop | Accepted |

Open questions that still need ADRs are tracked in
[`../development/open-decisions.md`](../development/open-decisions.md).
