- The Vulkan device no longer leaks its timeline semaphore if allocator creation fails.
- Swapchains only use formats the RHI can describe (8-bit BGRA/RGBA), instead of falling back
  to whatever format the surface lists first.
- Render passes now wait for any earlier use of their attachments, not only presentation.
- `Device::wait_for_submission` with an id that was never submitted returns an error instead of
  waiting forever.
- `submit` and `end_frame` refuse encoders and frames of another device, and `wait_idle` no
  longer races with submissions on other threads.
