# GPU and driver support

Policy: [ADR-0012](../decisions/0012-vulkan-baseline-bindings-and-memory.md) and
[ADR-0016](../decisions/0016-rhi-resource-and-bind-model.md). Oxreal requires **Vulkan 1.3** plus
the bindless subset of Khronos's Roadmap 2022 profile. The exact list is
[`tools/ci/required-vulkan-features.txt`](../../tools/ci/required-vulkan-features.txt). A GPU
without them is rejected at startup with an error naming what's missing.

Check your own GPU with `tools/ci/check-vulkan-features.sh` (needs `vulkaninfo`).

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
| ubuntu-24.04, ubuntu-26.04 | Mesa Lavapipe (software, Vulkan 1.3 conformant) | Required-feature check on every run (`vulkan features` job). Rendering tests come with the first GPU code (D-19). |
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
