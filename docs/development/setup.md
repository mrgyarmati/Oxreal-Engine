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
  The build uses a pinned `slangc`, downloaded and hash-verified by `cargo xtask` (coming with the
  first shaders), not the copy in the Vulkan SDK, which may be a different version.
- Minimum: a Vulkan 1.3 driver with dynamic rendering, synchronization2 and maintenance4
  ([ADR-0012](../decisions/0012-vulkan-baseline-bindings-and-memory.md)). Tested hardware is listed
  in [gpu-support.md](gpu-support.md). Reference development setup: SDK 1.4.363.1.
- Debug builds enable `VK_LAYER_KHRONOS_validation` when it's present. Develop with it installed.

### Verify your setup

```sh
vulkaninfo --summary                      # your GPU should be listed with apiVersion >= 1.3
vulkaninfo | grep VK_LAYER_KHRONOS_validation   # must be found (PowerShell: | Select-String ...)
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

Packages needed for windowing (Wayland and X11) and Vulkan:

**Ubuntu / Debian**
```sh
sudo apt install build-essential pkg-config libxkbcommon-dev libwayland-dev \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev \
  libvulkan-dev vulkan-tools vulkan-validationlayers mesa-vulkan-drivers
```

**Fedora**
```sh
sudo dnf install gcc pkgconf-pkg-config libxkbcommon-devel wayland-devel \
  libX11-devel libXcursor-devel libXrandr-devel libXi-devel \
  vulkan-loader-devel vulkan-tools vulkan-validation-layers mesa-vulkan-drivers
```

**Arch**
```sh
sudo pacman -S base-devel pkgconf libxkbcommon wayland libx11 libxcursor libxrandr libxi \
  vulkan-icd-loader vulkan-tools vulkan-validation-layers
```

Test under both a Wayland session and X11 (or Xwayland) before claiming windowing support.
This list will be finalized when the platform crates land.

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
