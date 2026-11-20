- `oxreal-rhi-vulkan`: creating a Vulkan instance no longer fails with
  `vkEnumerateInstanceExtensionProperties` returning `VK_INCOMPLETE`. Some drivers (seen on AMD's
  Windows driver) return it when other threads create and destroy instances at the same time;
  the backend now asks again, up to four times, as the code means.
