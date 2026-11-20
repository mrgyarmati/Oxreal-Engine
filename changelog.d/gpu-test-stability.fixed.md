- `oxreal-rhi-vulkan`: creating a device no longer fails when a Vulkan enumeration
  (`vkEnumerateInstanceExtensionProperties` and similar) returns `VK_INCOMPLETE` from its count
  call. It was seen on Windows with AMD's driver installed while other threads created and
  destroyed instances. `VK_INCOMPLETE` is a success code, so the backend now asks again, up to
  four times.
