- The Vulkan device no longer leaks its timeline semaphore if allocator creation fails.
- Swapchains only use formats the RHI can describe (8-bit BGRA/RGBA), instead of falling back
  to whatever format the surface lists first.
- Render passes now wait for any earlier use of their attachments, not only presentation.
