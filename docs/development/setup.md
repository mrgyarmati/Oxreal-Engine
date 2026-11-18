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

- The GPU driver provides the Vulkan loader. The **LunarG Vulkan SDK** provides validation layers,
  `vulkaninfo`, and shader tools (`glslc`/`dxc`/`slangc` depending on D-04).
- Debug builds enable `VK_LAYER_KHRONOS_validation` when it's present.
- Check with `vulkaninfo --summary`.

## Windows 10/11

- Visual Studio 2022 Build Tools with "Desktop development with C++" (MSVC linker and Windows SDK).
- Toolchain target `x86_64-pc-windows-msvc`.
- Vulkan SDK from LunarG (the installer sets `VULKAN_SDK`).

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

The local path is recorded in `docs/UnrealEngineSource.txt`. Read
[ADR-0007](../decisions/0007-unreal-source-as-ux-reference-only.md) before using it: behaviour
reference only, never code. To let Claude Code read it, add the path to
`permissions.additionalDirectories` in `.claude/settings.local.json` (machine-specific, not committed).
