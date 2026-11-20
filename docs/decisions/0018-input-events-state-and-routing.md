# ADR-0018: Input events, input state and routing

- **Status:** Accepted
- **Date:** 2026-11-20
- **Related:** PRD §5 (core runtime: input; gameplay: input mapping) and §7 (M1), TAD §2 and §7, ADR-0004 (platform code in the platform layer), ADR-0008 (gameplay framework), ADR-0011 (conventions), ADR-0014 §5 (`Input` stage), ADR-0015 §3 (input arrives with the input crate); prior art: [Cordite](../development/prior-art.md) (R1)

## Context

Milestone 1's exit criterion is an app that **renders and navigates** a 3D scene, so the engine
needs keyboard and mouse input. Three earlier decisions already constrain it:

- **ADR-0015 §3** keeps keyboard, mouse and text input out of the window event enum "until the
  input crate", and requires that input pass through engine routing.
- **ADR-0014 §5** runs an `Input` stage before gameplay, where devices are sampled and input is
  routed through **contexts with consumption**.
- **Cordite R1:** input could not be consumed, so UI couldn't stop gameplay from seeing a click.
  The lesson recorded in `prior-art.md` is that context stacks and consumption must be designed
  in from the start.

TAD §2 places input in the `systems` layer, above `platform` and `runtime`.

Facts checked in the `winit` 0.30.13 source on 2026-11-20 (the version ADR-0015 adopted):

- **Keys:** `WindowEvent::KeyboardInput` carries a `KeyEvent` with:
  - `physical_key`, either `PhysicalKey::Code(KeyCode)` or `Unidentified(NativeKeyCode)`;
  - a layout-dependent `logical_key`;
  - `state` and `repeat`.

  `KeyCode` follows the W3C `KeyboardEvent.code` table, which names keys by their **position**
  on a US keyboard. So `KeyW` is the key left of `KeyE` on every layout.
- **Synthetic key events.** The `is_synthetic` flag marks key presses that winit generates for
  keys already held when a window gains focus, and releases it generates when focus is lost.
  winit documents this as "only functional on X11 and Windows". Wayland never synthesizes them.
- **Raw mouse motion.** `ApplicationHandler::device_event` delivers `DeviceEvent::MouseMotion`
  with unaccelerated deltas "in unspecified units":
  - Windows: raw input.
  - X11: XInput2 raw events.
  - Wayland: the `zwp_relative_pointer_v1` protocol.

  Device events are delivered **only while a window is focused** by default
  (`DeviceEvents::WhenFocused`).
- **Cursor grab.** `Window::set_cursor_grab` takes `Confined` or `Locked`:
  - Windows and Wayland implement both.
  - X11 returns `NotSupported` for `Locked`.
  - `set_cursor_visible(false)` hides the cursor only inside the window.
- **Mouse.** `WindowEvent::MouseInput` reports button presses. `CursorMoved` reports the cursor
  position in physical pixels from the window's top-left corner. `MouseWheel` reports
  `MouseScrollDelta::LineDelta` (wheel notches) or `PixelDelta` (touchpads), and positive y
  scrolls content down.

## Options considered

### Where input state and routing live
- **A. Applications handle raw window events themselves** (the status quo of the examples).
  - Pros: nothing to build.
  - Cons: every app re-implements held-key tracking, focus loss and edge detection, and there
    is no common place for routing, so R1 repeats.
- **B. `oxreal-window` also keeps input state.**
  - Pros: one crate fewer.
  - Cons: mixes platform translation with engine policy (frame edges, consumption, later action
    mapping), and puts gameplay-facing state in the platform layer.
- **C. `oxreal-window` translates events; a new `oxreal-input` crate (`systems`) owns state and
  routing.**
  - Pros: platform code stays a thin translation (ADR-0004). State and routing are tested
    without a display, and the crate is where TAD §2 already puts input.
  - Cons: one more crate.

### Event-driven state or polled device state
- **Polling OS key state at frame time** (for example `GetAsyncKeyState`) needs per-OS code
  outside winit, and **loses presses shorter than a frame**: a tap between two samples never
  happened.
- **Event-driven state**, built from the events of the frame, keeps every press and release.

### Physical or logical keys for bindings
- **Logical keys** move with the keyboard layout: WASD on QWERTY becomes ZQSD on AZERTY.
- **Physical keys** keep bindings in the same place on every layout, which is what games and
  Unreal-style viewport controls expect. Text entry needs the logical side instead, through
  text and IME events, which belong with UI.

### Mouse-look source
- **Cursor deltas** are accelerated by the OS, stop at the screen or window edge, and need the
  cursor re-centred every frame (impossible on Wayland unless the pointer is locked).
- **Raw device motion** is unaccelerated, unaffected by edges and available on all three
  windowing systems.

## Decision

### 1. Device input events from the platform layer (`oxreal-window`)
`oxreal-window` translates winit input into engine-owned types. As in ADR-0015, no winit type
appears in its public API.

- **Window events** (new variants of the non-exhaustive `WindowEvent`):
  - **Key:** a physical `KeyCode`, a `ButtonState` (pressed or released) and a `repeat` flag.
  - **Mouse button:** a `MouseButton` (left, right, middle, back, forward, or other with a
    platform index) and a `ButtonState`.
  - **Cursor moved:** the position in **physical pixels**, origin at the top-left of the
    drawable area, as `f64`. Window coordinates are y-down; that convention stops at the window.
  - **Cursor left** the window.
  - **Scroll:** a `ScrollDelta` that keeps **lines** (wheel notches) and **pixels** (touchpads)
    apart. Positive x and y mean right and up (away from the user), so a wheel turned forward
    gives positive y. The platform layer doesn't convert one to the other.
- **Device events** arrive through a new `App::device_event` callback. The first is **raw mouse
  motion**: an unaccelerated `(dx, dy)` in device units, with positive y toward the user (down
  the screen). The platform layer requests device events **only while a window is focused**,
  and says so explicitly instead of relying on winit's default.
- **Keys are physical.** `KeyCode` uses the W3C `KeyboardEvent.code` names (`KeyW`, `Digit1`,
  `ShiftLeft`, `Numpad0` and so on). It covers the keys of standard 104-, 105- and 106-key
  keyboards (`IntlBackslash`, `IntlRo`, `IntlYen`). Media, browser and launcher keys are left
  out because they're OS controls rather than game input. Keys winit can't identify, and keys
  the engine doesn't model, are dropped: they can't be bound portably, and keeping a
  platform-specific code would leak per-OS behaviour into bindings.
- **Synthetic key events are dropped** on every platform, because only X11 and Windows produce
  them. §2 resets held state on focus loss instead, which gives the same result everywhere.
- **Cursor capture** for mouse-look is a `Window` operation. Capturing hides the cursor and
  **locks** it. Where locking isn't supported (X11), it confines the cursor to the window
  instead. The result tells the caller which of the two it got, and failure is a typed error.
  Raw motion keeps arriving in both modes, so mouse-look behaves the same.
- **Text and IME input** are separate events. They're added with runtime UI (D-05), not with
  key events.

### 2. Input state in `oxreal-input` (`systems` layer)
- `InputState` is built **from events**. It reports each key and mouse button as **pressed**
  (held now), **just pressed** and **just released** (edges since the last frame). It also
  accumulates **raw mouse motion** and **scroll** (lines and pixels apart) for the frame, and
  keeps the last **cursor position**, or none after the cursor left.
- **Frame boundary.** The application calls `end_frame()` once the frame has read its input.
  That clears the edges and the accumulators. A press and a release within one frame report
  both edges, and the key isn't held afterwards, so **no tap is lost**.
- **Key repeat** doesn't change state. A repeat is not a new press.
- **Focus loss releases everything.** Every held button reports a release, the accumulators are
  cleared, and the cursor position is forgotten. Keys already held when focus returns count only
  once they're pressed again. With §1, that's the same on Windows, X11 and Wayland, and no key
  is ever stuck down.
- `oxreal-input` **re-exports the input vocabulary** (`KeyCode`, `MouseButton`, `ButtonState`,
  `ScrollDelta`). Gameplay code depends on `oxreal-input`, never on `oxreal-window`.
- There is one `InputState` per application for now. Deciding which window's or viewport's
  events reach which consumer comes with the editor's multiple viewports.

### 3. Routing: designed now, built with the second consumer
In the ECS `Input` stage (ADR-0014 §5), the frame's events pass through an ordered **stack of
input contexts**, top first. For example: debug console, runtime UI, editor viewport, gameplay.

- A context may **consume** an event.
- A consumed **press** is hidden from every lower context **until its matching release**, and
  the release goes to the context that took the press. A button that UI captured never looks
  held to gameplay, and no context sees a release without its press (no stuck keys).
- Each context reads a **view** with the same queries as `InputState` (pressed, just pressed,
  motion and so on), so code written against `InputState` today moves to a context view
  without changing shape.
- Contexts are built when the second consumer exists: runtime UI, or the editor viewport. Until
  then each application has exactly one consumer, and a stack of one would be untested
  scaffolding.
- **Action mapping** builds on contexts: named actions and axes bound to keys, buttons and
  later gamepads, rebinding, and per-context maps. It's part of gameplay input mapping (PRD
  §5) and gets its own ADR.

### 4. Not decided here
- **Gamepads** need a backend choice (a third-party crate such as `gilrs`, or our own per-OS
  code). That's a separate ADR with a dependency audit.
- Touch and pen input.
- Input recording and replay. The event-driven design of §2 makes this possible later.

### 5. Testing
- **Event mapping:** unit tests without a display, covering every modelled key, button and
  scroll kind, and the dropping of synthetic and unidentified keys.
- **`InputState`:** unit tests for edges, sub-frame taps, repeat, focus loss and accumulation.
- **CI:** the examples run under Xvfb with no input, which proves that idle input changes
  nothing.
- **Manual checks** of cursor capture and mouse-look on Windows, X11 and Wayland until CI can
  inject input. Injecting synthetic X11 input (XTEST, for example `xdotool`) under Xvfb is the
  candidate. What was run is recorded in the implementation PR.

### 6. First consumer
The cube example gets a **fly camera** with Unreal-style viewport controls (observable
behaviour, ADR-0007):
- Holding the right mouse button captures the cursor and turns the view.
- `W` `A` `S` `D` move, and `E` and `Q` move up and down.
- The wheel changes speed while the button is held.

It stays **example code** until the gameplay camera arrives with the ECS (ADR-0008). Its math is
unit-tested in the example.

## Consequences

- Positive:
  - Keyboard and mouse input on all three windowing systems, without a winit type in any public
    API.
  - Layout-independent bindings from the first binding.
  - No lost taps, and no stuck keys after Alt-Tab, by construction and the same on every
    platform.
  - Routing and consumption are specified before the first UI exists, so R1 isn't repeated.
- Negative / costs:
  - The engine owns a `KeyCode` enum and its mapping, which needs updating if winit adds keys
    worth binding.
  - Dropping keys the engine doesn't model means an exotic key can't be bound until it's added.
  - On X11 the captured cursor is confined rather than locked. While it's hidden it can drift
    inside the window, and it reappears where it drifted to.
  - Mouse-look on Wayland needs the compositor to support the relative-pointer protocol. Mutter
    (GNOME), KWin (KDE) and wlroots compositors such as Sway do.
- Follow-up work:
  - The implementation PR: platform events, `oxreal-input`, and the fly camera.
  - Input contexts and consumption, with runtime UI or the editor viewport.
  - An action mapping ADR, and a gamepad ADR.
  - Text and IME input with UI (D-05).
  - Injected-input tests under Xvfb.
- Revisit triggers:
  - A platform where raw mouse motion is unavailable or unreliable.
  - winit 0.31 changing the input model.
  - Routing needs that a context stack can't express, such as simultaneous consumers for one
    event (for example a chorded editor shortcut that both UI and viewport must see).
