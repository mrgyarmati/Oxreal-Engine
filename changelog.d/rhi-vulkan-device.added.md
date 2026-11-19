- `oxreal-rhi`: the backend-agnostic GPU device API, with deterministic GPU selection that
  explains every rejected GPU.
- `oxreal-rhi-vulkan`: the Vulkan backend's device slice. It loads Vulkan at runtime (a missing
  driver is a clear error), creates a Vulkan 1.3 instance with validation layers in debug
  builds and Vulkan messages in the log, picks a GPU with every required feature, and creates
  the device, queue and GPU memory allocator.
- `OXREAL_GPU` (pick a GPU by index or name) and `OXREAL_VULKAN_VALIDATION` (`1` or `0`).
- `run_application` in `oxreal-diagnostics`: application entry point that reports errors as
  readable messages with their full cause chain.
