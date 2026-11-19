# GPU and driver support

Policy: [ADR-0012](../decisions/0012-vulkan-baseline-bindings-and-memory.md). Oxreal requires
**Vulkan 1.3** with dynamic rendering, synchronization2 and maintenance4. The full required
feature list is finalized in the RHI bind-model ADR. A GPU without them is rejected at startup
with an error naming what's missing.

**This page lists only what has actually been run and tested.** "It compiles" or "the vendor
says Vulkan 1.3" doesn't qualify (ADR-0002).

## Tested configurations

No Oxreal rendering code exists yet. Rows are added as Milestone 1 lands, by whoever runs the
tests on that hardware.

| Vendor | GPU | Driver | OS / windowing | Vulkan (device) | Oxreal commit | Result | Tested by / date |
|---|---|---|---|---|---|---|---|
| | | | | | | | |

## Continuous integration

| Runner | Device | Status |
|---|---|---|
| ubuntu-24.04, ubuntu-26.04 | Mesa Lavapipe (software, Vulkan 1.3 conformant) | Planned with the first GPU tests (D-19) |
| windows-2025 | none (no GPU) | Build and non-GPU tests only |

## Known unsupported

Configurations confirmed to lack the baseline. Add a row only with evidence (a `vulkaninfo`
summary or a test run).

| Vendor | GPU / driver | Reason | Evidence |
|---|---|---|---|
| | | | |

## Reporting

Ran Oxreal on hardware not listed here? Open an issue with the output of
`vulkaninfo --summary`, your OS and windowing system (Wayland or X11), and the test results.
