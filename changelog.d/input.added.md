- `oxreal-window` reports keyboard and mouse input (ADR-0018):
  - key events with physical key codes (W3C `KeyboardEvent.code` names), so bindings don't move
    with the keyboard layout;
  - mouse buttons, cursor movement and scrolling;
  - raw mouse motion through the new `App::device_event` callback;
  - `Window::capture_cursor` and `Window::release_cursor` for mouse-look.
- `oxreal-input`: `InputState` keeps held keys and buttons, what was pressed and released this
  frame, raw mouse motion, scroll and the cursor position. No tap is lost within a frame, and
  losing focus releases everything, the same on Windows, X11 and Wayland.
- The `cube` example is a small scene you can fly through: hold the right mouse button to look
  around, `W` `A` `S` `D` to move, `E` / `Q` to rise and sink, and the wheel to change speed.
