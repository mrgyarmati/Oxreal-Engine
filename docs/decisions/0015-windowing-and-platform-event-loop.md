# ADR-0015: Windowing and the platform event loop

- **Status:** Accepted
- **Date:** 2026-11-19
- **Related:** ADR-0002 (Windows and Linux, Wayland and X11), ADR-0003 (renderer independent of windowing), ADR-0004 (platform code only in the platform layer), ADR-0010, ADR-0012 (surfaces from `raw-window-handle`), TAD §5.3 and §6; prior art: [Cordite](../development/prior-art.md) (R1, R4)

## Context

Milestone 1 needs windows and an event loop on Windows and on Linux under both **Wayland and
X11**. The renderer must create its Vulkan surface from the window's raw handles without
depending on the windowing library (ADR-0003, ADR-0012). Platform-specific code belongs only in
the platform layer (ADR-0004).

Cordite used GLFW. Its audit found two windowing problems:
- **R4:** DPI and framebuffer changes were never observed, so the UI scale went stale on another
  monitor.
- **R1:** input could not be consumed before gameplay saw it.

Facts checked on 2026-11-19:

- **`winit`** (Apache-2.0, 67.7M downloads, repository active as of 2026-11-13). The stable line
  is **0.30.13** (released 2026-03-02). **0.31 is in beta** (0.31.0-beta.4, 2026-10-24) and
  restructures the API (a `winit-core` split).
  - Default features: `rwh_06`, `x11`, `wayland`, `wayland-dlopen`, `wayland-csd-adwaita`.
  - With `wayland-dlopen` and `x11-dl`, libwayland and the X11 libraries are **loaded at
    runtime**: one Linux binary serves both display servers, and building needs no system
    development packages.
- **`raw-window-handle` 0.6.2** (MIT OR Apache-2.0 OR Zlib, 139M downloads) is the ecosystem's
  interop crate for passing native window handles to graphics APIs. `ash-window` 0.13 uses it.
- **SDL3** (`sdl3` 0.22.1, MIT, 0.3M downloads) and **GLFW** (`glfw` 0.63.0, Apache-2.0, 1.0M
  downloads) are Rust bindings over C libraries.

## Options considered

- **`winit` behind an engine-owned API.** Pure Rust, the de-facto standard, with Wayland, X11
  and Win32 in one crate, and raw handles through `raw-window-handle`. It ships breaking minor
  releases, which the wrapper contains.
- **SDL3.** Mature C library with windowing, input, gamepads and audio. The Rust bindings are
  young, it needs a C build or a shipped shared library, and it overlaps systems we plan to own.
- **GLFW.** Small C library (Cordite's choice), but a C dependency, weaker Wayland integration,
  and no gamepad rumble or IME depth.
- **Our own Win32, Wayland and X11 backends.** Full control, but three large platform
  integrations to write and maintain before the first triangle.

## Decision

1. **`winit` 0.30 (the current stable line)** provides windows and the event loop. It's wrapped
   by `oxreal-window` (layer `platform`). **No `winit` type appears in the crate's public API.**
   - Features: `rwh_06`, `x11`, `wayland`, `wayland-dlopen` and
     **`wayland-csd-adwaita-notitle`**. Linux gets runtime-loaded Wayland and X11, plus
     client-side decorations (buttons, borders, resizing) on compositors that need them, such as
     GNOME.
   - The default `wayland-csd-adwaita` feature is **not** used. It renders title text through
     `ab_glyph` → `ttf-parser`, which **RUSTSEC-2026-0192** (found by `cargo-deny` in this
     ADR's implementation PR) declares unmaintained, with no fixed version. We don't silence
     unmaintained-parser advisories.
   - Cost: on compositors that rely on client-side decorations, the title bar has no text. KDE,
     Sway and other compositors that draw their own decorations are unaffected. The editor
     is expected to draw its own window chrome later anyway.
   - Moving to 0.31 once it's stable is an upgrade inside `oxreal-window` only.
2. **`raw-window-handle` 0.6 is the one sanctioned interop type.** `oxreal-window`'s `Window`
   implements `HasWindowHandle` and `HasDisplayHandle`, and the Vulkan backend creates surfaces
   from them through `ash-window` (ADR-0012). This is the explicit ADR-0001 exception for this
   crate: the traits are the ecosystem standard, purpose-built for this boundary.
3. **Engine-owned event model:**
   - Applications implement an `oxreal-window` `App` trait with callbacks for lifecycle
     (`resumed`, `suspended`, `exiting`), window events and "about to wait" (end of an event
     batch, where the frame is driven).
   - Window events are an engine enum covering:
     - close requested
     - **resized in physical pixels**
     - **scale factor changed** (Cordite R4: observed from day one)
     - focus changed
     - redraw requested
   - Keyboard, mouse and text input are added with the input crate. They pass through engine
     input routing in the `Input` stage (ADR-0014), so UI can consume input before gameplay
     (Cordite R1).
4. **Event loop rules:**
   - The loop runs on the **main thread** (a requirement on several platforms).
   - Windows are created in `resumed`, following winit 0.30's lifecycle.
   - Sizes are **physical pixels** everywhere in the engine. The scale factor is reported
     separately for UI.
   - A zero-sized (minimized) window is a normal state that renderers must handle, not an error.
5. **Linux display server selection:** winit's default is Wayland when `WAYLAND_DISPLAY` is set,
   and X11 otherwise. `OXREAL_LINUX_DISPLAY=wayland|x11` forces one, so both paths can be tested
   on one machine. The chosen backend is logged at `info`. When neither is available, the error
   names both environment variables.
6. **Errors:** creating the event loop or a window returns a typed `oxreal-window` error with an
   actionable message. Failures never panic.
7. **Testing:**
   - Event mapping is unit-tested without a display.
   - A smoke example opens a window, processes events and exits after a set number of frames.
   - CI runs the smoke example on Linux under a virtual X server (`xvfb`), and on Windows if the
     hosted runner allows window creation (evaluated in the implementation PR).
   - Wayland is verified manually until a headless compositor job is added. `gpu-support.md` and
     setup docs record what was run.
8. **Runtime libraries on Linux** (loaded at runtime, not needed to build) are documented in
   `setup.md`: libwayland-client, libxkbcommon, and the X11 libraries winit loads (libX11,
   libXcursor, libXrandr, libXi, libxkbcommon-x11).

## Consequences

- One pure-Rust windowing dependency covering every target windowing system, with Linux binaries
  that run on Wayland and X11 alike and build without system packages.
- An engine-owned event model: replacing or upgrading winit (0.31, or our own backend later)
  touches only `oxreal-window`.
- DPI and resize events exist from the first commit, avoiding Cordite's R4.
- Costs:
  - `winit` brings a sizeable transitive tree on Linux (Wayland protocols, xkbcommon, X11
    bindings), audited in the implementation PR.
  - It's Apache-2.0 only, which is compatible.
  - Breaking minor releases need occasional upgrades.
- Revisit triggers: winit lacking something the editor needs (multi-window docking, IME,
  drag-and-drop) that can't be added upstream, or a stable 0.31 (upgrade).
