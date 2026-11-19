- `oxreal-window`: windows and the event loop on Windows and Linux (Wayland and X11 from one
  binary). Applications implement the `App` trait. Sizes are in physical pixels, with resize and
  scale-factor events. `OXREAL_LINUX_DISPLAY=wayland|x11` forces a Linux display server.
- A window smoke test: `cargo run -p oxreal-window --example smoke -- --frames 120`.
