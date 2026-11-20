# Development Setup

## Common

1. Install Rust via [rustup](https://rustup.rs). The toolchain version is pinned in
   `rust-toolchain.toml`, and rustup installs it automatically (with rustfmt and clippy).
2. Optional tools:
   ```sh
   cargo install cargo-deny   # license/advisory/ban checks (CI runs this)
   ```
3. Verify:
   ```sh
   cargo xtask ci
   ```

## Vulkan (needed from M1)

- The GPU driver provides the Vulkan loader and ICD. The **LunarG Vulkan SDK** provides
  validation layers, `vulkaninfo` and `spirv-val`.
- **Shaders are Slang** ([ADR-0013](../decisions/0013-shader-language-and-compiler-pipeline.md)).
  The build uses a pinned `slangc`, downloaded and hash-verified by `cargo xtask slang install` (run
  automatically by `cargo xtask ci`; needs `curl`), not the copy in the Vulkan SDK, which may be
  a different version. It lives in the git-ignored `.tools/` folder.
- **Cooked shaders** ([ADR-0019](../decisions/0019-player-renderer-and-cooked-shaders.md)). Games
  don't compile shaders. They load the directory that `cargo xtask cook-shaders` writes to
  `target/<profile>/cooked/shaders/` (`--profile release` for release builds). Cooking
  **requires `spirv-val`**: install the Vulkan SDK and set `VULKAN_SDK`, or install the distro's
  `spirv-tools` package. It also requires exactly the pinned Slang, which it installs if needed.
  Each crate lists its shaders in `shaders/cook.json`, including every file they import: the
  cook fails if a shader reads an undeclared file. `cargo xtask ci` runs the cook tests, which
  skip without `spirv-val`. CI requires it on every platform.
- Minimum: a Vulkan 1.3 driver with dynamic rendering, synchronization2 and maintenance4
  ([ADR-0012](../decisions/0012-vulkan-baseline-bindings-and-memory.md)). Tested hardware is listed
  in [gpu-support.md](gpu-support.md). Reference development setup: SDK 1.4.363.1.
- Debug builds enable `VK_LAYER_KHRONOS_validation` when it's present. Develop with it installed.

### Verify your setup

```sh
vulkaninfo --summary                      # your GPU should be listed with apiVersion >= 1.3
vulkaninfo | grep VK_LAYER_KHRONOS_validation   # must be found (PowerShell: | Select-String ...)
bash tools/ci/check-vulkan-features.sh             # every required feature is supported
cargo run -p oxreal-rhi-vulkan --example device_info   # Oxreal picks your GPU and creates a device
```

If the validation layer isn't listed, the SDK isn't registered. Reinstall the SDK through its
official installer rather than pointing environment variables at a copied folder.

## Windows 10/11

- Visual Studio Build Tools (2022 or later) with the "Desktop development with C++" workload
  (MSVC linker and Windows SDK).
- Toolchain target `x86_64-pc-windows-msvc`.
- Vulkan SDK: use the official installer from <https://vulkan.lunarg.com/sdk/home#windows>, or
  `winget install KhronosGroup.VulkanSDK`. It installs to `C:\VulkanSDK\<version>`, sets
  `VULKAN_SDK` machine-wide, adds the SDK to `PATH`, and registers the layers. Open a new terminal
  afterwards.

## Linux

### Building

Only a C toolchain is needed, for crates with small C parts (for example the optional Tracy
profiler). **Windowing needs no development packages**, because Oxreal loads the Wayland and X11
libraries at runtime ([ADR-0015](../decisions/0015-windowing-and-platform-event-loop.md)).

| Distro | Command |
|---|---|
| Ubuntu / Debian | `sudo apt install build-essential` |
| Fedora | `sudo dnf install gcc gcc-c++` |
| Arch | `sudo pacman -S base-devel` |

### Running

These libraries are loaded at runtime and are already present on any desktop install:
libwayland-client, libxkbcommon and libxkbcommon-x11, plus the X11 libraries libX11,
libX11-xcb, libXcursor, libXrandr and libXi. On a minimal or headless system, install them
explicitly:

| Distro | Command | Verified |
|---|---|---|
| Ubuntu / Debian | `sudo apt install libwayland-client0 libxkbcommon0 libxkbcommon-x11-0 libx11-6 libx11-xcb1 libxcursor1 libxrandr2 libxi6` | CI: Ubuntu 24.04 and 26.04, X11 path under Xvfb (Wayland not yet in CI) |
| Fedora | `sudo dnf install libwayland-client libxkbcommon libxkbcommon-x11 libX11 libX11-xcb libXcursor libXrandr libXi` | Not yet |
| Arch | `sudo pacman -S wayland libxkbcommon libxkbcommon-x11 libx11 libxcursor libxrandr libxi` | Not yet |

Vulkan runtime and tools (needed from the first rendering code):

| Distro | Command |
|---|---|
| Ubuntu / Debian | `sudo apt install libvulkan1 vulkan-tools vulkan-validationlayers mesa-vulkan-drivers` |
| Fedora | `sudo dnf install vulkan-loader vulkan-tools vulkan-validation-layers mesa-vulkan-drivers` |
| Arch | `sudo pacman -S vulkan-icd-loader vulkan-tools vulkan-validation-layers` |

### Choosing Wayland or X11

Oxreal uses Wayland when `WAYLAND_DISPLAY` is set, and X11 otherwise. Force either one with
`OXREAL_LINUX_DISPLAY=wayland` or `OXREAL_LINUX_DISPLAY=x11`. Test both before claiming
windowing support for a change:

```sh
OXREAL_LINUX_DISPLAY=wayland cargo run -p oxreal-window --example smoke -- --frames 120
OXREAL_LINUX_DISPLAY=x11     cargo run -p oxreal-window --example smoke -- --frames 120
xvfb-run -a cargo run -p oxreal-window --example smoke -- --frames 120   # headless (X11)
```

### Known upstream issues

- **Vulkan loader crash with several instances on parallel threads (Linux), loaders older
  than 1.4.350.** Reported upstream as
  [Vulkan-Loader #1863](https://github.com/KhronosGroup/Vulkan-Loader/issues/1863) and fixed by
  [#1866](https://github.com/KhronosGroup/Vulkan-Loader/pull/1866) (merged 2026-02-20, in SDK
  1.4.350 and later). Ubuntu 26.04 ships loader 1.4.341, which predates the fix.
  - Two mutexes guard one global structure, the list of instances and their drivers.
  - `vkEnumeratePhysicalDevices` unloads drivers that found no GPU, and frees their entries under
    `loader_lock`.
  - Meanwhile, `vkSetDebugUtilsObjectNameEXT` on *any other* instance walks those entries, in
    `loader_get_icd_and_device`, under `loader_global_instance_list_lock`.

  It needs driver manifests for GPUs that aren't present, which `mesa-vulkan-drivers` installs. It
  also needs more than one `VkInstance` in the process at once, which in Oxreal only the GPU test
  suites do: each test creates its own device. If `cargo test -p oxreal-rhi-vulkan -- --ignored`
  segfaults on such a loader, choose your driver explicitly, for example
  `VK_LOADER_DRIVERS_SELECT='*lvp*'` for Lavapipe, as CI does. Your GPU's manifest name is in
  `/usr/share/vulkan/icd.d/`. The setting can go from CI once every runner image ships loader
  1.4.350 or later. It still documents which driver the job tests.

## Unreal Engine source (UX reference, optional)

Read [ADR-0007](../decisions/0007-unreal-source-as-ux-reference-only.md) first: Unreal source is
a behaviour reference only, never a source of code. Access requires an Epic Games account
linked to GitHub, which is not needed to build or contribute to Oxreal.

To use it with Claude Code (the `unreal-ux-researcher` agent):

1. Write the absolute path of your checkout as a single line in `.claude/unreal-source.local.txt`.
   The file is git-ignored by the `.claude/*.local.*` pattern.
2. Allow Claude Code to read that directory by adding it to `permissions.additionalDirectories`
   in `.claude/settings.local.json`. Also add matching `Edit` and `Write` deny rules for it, so
   the checkout stays read-only.
