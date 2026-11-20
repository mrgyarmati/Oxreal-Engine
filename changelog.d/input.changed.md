- `WindowEvent` has new variants for keyboard and mouse input. It's `#[non_exhaustive]`, so
  existing matches with a wildcard arm keep compiling.
- `WindowError` has a new `CursorCapture` variant.
