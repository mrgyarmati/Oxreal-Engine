//! GPU selection, the logical device and the memory allocator.
//!
//! The device's state lives in [`Shared`], behind an `Arc` that every encoder also holds. The
//! `VkDevice`, its tables and its allocator therefore outlive any encoder still recording, even
//! after the [`VulkanDevice`] itself is dropped.

use std::ffi::{CStr, CString};
use std::sync::{Arc, Mutex, MutexGuard};

use ash::vk;
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use oxreal_diagnostics::{info, warn};
use oxreal_rhi::{
    AdapterCandidate, AdapterInfo, AdapterType, ApiVersion, BufferDesc, BufferHandle,
    CommandEncoder, DeviceDesc, Frame, GpuSelection, HandlePool, PipelineHandle,
    RenderPipelineDesc, RhiError, SubmissionId, SurfaceDesc, SurfaceHandle, SurfaceTarget,
    TextureDesc, TextureHandle, select_adapter,
};

use crate::buffers::Buffer;
use crate::error::from_vk;
use crate::features::{self, FeatureSet};
use crate::instance::{VulkanInstance, format_version};
use crate::present::{Surface, Texture};
use crate::resources::{Pipeline, Retired, create_pipeline_layout};
use crate::retire::RetireQueue;

/// Locks `mutex`, recovering it if a panic poisoned it.
///
/// This is sound because every critical section in this crate keeps its data consistent at each
/// point where it could panic:
/// - The only user code it can run is logging (the application's tracing subscriber). Every log
///   call inside a critical section comes after the guarded data is consistent again. For
///   example, a new swapchain is stored on its surface before anything is logged.
/// - Otherwise it only does bounded arithmetic and collection updates, and allocation failure
///   aborts instead of unwinding.
///
/// So a poisoned lock still guards consistent data. Turning poisoning into errors instead would
/// leak the Vulkan objects that were being registered or destroyed, and turn one panic into
/// failures everywhere. Keep this property when adding a critical section.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Creates a Vulkan device according to `desc`.
///
/// `OXREAL_GPU` and `OXREAL_VULKAN_VALIDATION` override the corresponding fields of `desc`.
///
/// # Errors
///
/// - [`RhiError::RuntimeUnavailable`] / [`RhiError::RuntimeTooOld`] without a Vulkan 1.3 loader.
/// - [`RhiError::NoSuitableAdapter`], listing every GPU and why it was rejected.
/// - [`RhiError::AdapterNotFound`] / [`RhiError::InvalidEnvironment`] for a bad selection.
/// - Other [`RhiError`]s if a Vulkan call fails.
pub fn create_device(desc: &DeviceDesc) -> Result<VulkanDevice, RhiError> {
    let instance = VulkanInstance::new(&desc.app_name, desc.validation)?;
    let selection = GpuSelection::from_env_or(desc.gpu.clone())?;

    // SAFETY: `instance.instance` is a live instance.
    let physical_devices = unsafe { instance.instance.enumerate_physical_devices() }
        .map_err(|e| from_vk("vkEnumeratePhysicalDevices", e))?;
    let probes: Vec<Probe> = physical_devices
        .iter()
        .map(|&pd| probe(&instance.instance, pd))
        .collect();
    let candidates: Vec<AdapterCandidate> = probes.iter().map(|p| p.candidate.clone()).collect();
    for (i, c) in candidates.iter().enumerate() {
        info!(
            index = i,
            name = %c.info.name,
            kind = ?c.info.adapter_type,
            vulkan = %c.info.api_version,
            usable = c.rejection_reasons.is_empty(),
            rejected_because = %c.rejection_reasons.join("; "),
            "GPU found"
        );
    }
    let chosen = select_adapter(&candidates, &selection)?;
    let probe = &probes[chosen];
    let Some(queue_family) = probe.queue_family else {
        // `select_adapter` only returns usable adapters, and a missing queue family is a
        // rejection reason, so this can't happen. Report it rather than panic.
        return Err(RhiError::Backend {
            api: "Vulkan",
            operation: "device selection",
            details: "selected adapter has no graphics+compute queue family".to_owned(),
        });
    };

    // Presentation needs VK_KHR_swapchain. A device without it still works headless, and
    // `create_surface` reports the limitation.
    // SAFETY: `probe.physical_device` belongs to the live instance.
    let device_extensions = unsafe {
        instance
            .instance
            .enumerate_device_extension_properties(probe.physical_device)
    }
    .map_err(|e| from_vk("vkEnumerateDeviceExtensionProperties", e))?;
    let swapchain_available = instance.surface_support()
        && device_extensions.iter().any(|e| {
            e.extension_name_as_c_str()
                .is_ok_and(|n| n == ash::khr::swapchain::NAME)
        });
    let enabled_extensions: Vec<&CStr> = if swapchain_available {
        vec![ash::khr::swapchain::NAME]
    } else {
        Vec::new()
    };

    let logical = create_logical_device(
        &instance.instance,
        probe.physical_device,
        queue_family,
        &enabled_extensions,
    )?;
    // SAFETY: queue family `queue_family`, index 0, was requested in `create_logical_device`.
    let queue = unsafe { logical.device.get_device_queue(queue_family, 0) };

    let allocator = Allocator::new(&AllocatorCreateDesc {
        instance: instance.instance.clone(),
        device: logical.device.clone(),
        physical_device: probe.physical_device,
        debug_settings: gpu_allocator::AllocatorDebugSettings::default(),
        buffer_device_address: true,
        allocation_sizes: gpu_allocator::AllocationSizes::default(),
    })
    .map_err(|e| RhiError::Backend {
        api: "Vulkan",
        operation: "gpu-allocator initialization",
        details: e.to_string(),
    })?;

    // Raw objects are created last, so an earlier failure leaks nothing (the allocator and the
    // logical device clean up on drop).
    // One timeline semaphore paces every submission on the queue (ADR-0016 §4).
    let mut timeline_type = vk::SemaphoreTypeCreateInfo::default()
        .semaphore_type(vk::SemaphoreType::TIMELINE)
        .initial_value(0);
    let timeline_info = vk::SemaphoreCreateInfo::default().push_next(&mut timeline_type);
    // SAFETY: the device is alive and timelineSemaphore is enabled (a required feature).
    let timeline = unsafe { logical.device.create_semaphore(&timeline_info, None) }
        .map_err(|e| from_vk("vkCreateSemaphore (timeline)", e))?;
    let pipeline_layout = match create_pipeline_layout(&logical.device) {
        Ok(layout) => layout,
        Err(error) => {
            // SAFETY: never used. Destroyed exactly once, before the device.
            unsafe { logical.device.destroy_semaphore(timeline, None) };
            return Err(error);
        }
    };

    let adapter_info = probe.candidate.info.clone();
    info!(
        name = %adapter_info.name,
        driver = %adapter_info.driver_name,
        driver_info = %adapter_info.driver_info,
        vulkan = %adapter_info.api_version,
        validation = instance.validation_enabled(),
        presentation = swapchain_available,
        "Vulkan device created"
    );

    let shared = Shared {
        textures: Mutex::new(HandlePool::new()),
        buffers: Mutex::new(HandlePool::new()),
        pipelines: Mutex::new(HandlePool::new()),
        retired: Mutex::new(RetireQueue::default()),
        queue: Mutex::new(QueueState {
            queue,
            timeline_value: 0,
        }),
        timeline,
        pipeline_layout,
        allocator: Mutex::new(Some(allocator)),
        limits: Limits {
            max_image_dimension_2d: probe.limits.max_image_dimension2_d,
            max_color_attachments: probe.limits.max_color_attachments,
            max_buffer_size: probe.max_buffer_size,
        },
        queue_family,
        physical_device: probe.physical_device,
        debug: instance
            .debug_utils()
            .then(|| ash::ext::debug_utils::Device::new(&instance.instance, &logical.device)),
        surface_loader: instance
            .surface_support()
            .then(|| ash::khr::surface::Instance::new(&instance.entry, &instance.instance)),
        swapchain_loader: swapchain_available
            .then(|| ash::khr::swapchain::Device::new(&instance.instance, &logical.device)),
        logical,
        instance,
    };
    shared.name_object(timeline, "Oxreal queue timeline");
    Ok(VulkanDevice {
        surfaces: Mutex::new(HandlePool::new()),
        shared: Arc::new(shared),
        adapter_info,
    })
}

/// The queue and its timeline. Vulkan requires external synchronization for queue access, so
/// both live behind one mutex. Submission order and timeline values stay consistent.
pub(crate) struct QueueState {
    pub(crate) queue: vk::Queue,
    /// The last timeline value signalled by a submission.
    pub(crate) timeline_value: u64,
}

/// Device limits the backend checks requests against.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) max_image_dimension_2d: u32,
    pub(crate) max_color_attachments: u32,
    /// Largest buffer `vkCreateBuffer` accepts (maintenance4 makes exceeding it invalid).
    pub(crate) max_buffer_size: u64,
}

/// The device state that encoders share with the device.
///
/// **Lock order:** `surfaces` (on [`VulkanDevice`]) → `queue` → `retired` → `textures` →
/// `buffers` → `pipelines` → `allocator`. A thread holding one of these locks only takes locks to
/// its right.
/// `queue` → `retired` is what makes deferred destruction exact: "read the timeline value and
/// queue an object" and "submit and record the submission" never interleave.
///
/// Teardown (`Drop`): wait for the GPU, then destroy retired objects (surfaces included), live
/// textures and pipelines, the pipeline layout and the timeline, and free the allocator. Then
/// the fields drop in declaration order: the logical device, then the instance.
pub(crate) struct Shared {
    pub(crate) textures: Mutex<HandlePool<Texture>>,
    pub(crate) buffers: Mutex<HandlePool<Buffer>>,
    pub(crate) pipelines: Mutex<HandlePool<Pipeline>>,
    pub(crate) retired: Mutex<RetireQueue<Retired>>,
    pub(crate) queue: Mutex<QueueState>,
    pub(crate) timeline: vk::Semaphore,
    /// The layout every pipeline shares (ADR-0016 §2).
    pub(crate) pipeline_layout: vk::PipelineLayout,
    pub(crate) allocator: Mutex<Option<Allocator>>,
    pub(crate) limits: Limits,
    pub(crate) queue_family: u32,
    pub(crate) physical_device: vk::PhysicalDevice,
    pub(crate) debug: Option<ash::ext::debug_utils::Device>,
    pub(crate) surface_loader: Option<ash::khr::surface::Instance>,
    pub(crate) swapchain_loader: Option<ash::khr::swapchain::Device>,
    logical: LogicalDevice,
    pub(crate) instance: VulkanInstance,
}

impl Shared {
    /// The logical device.
    pub(crate) fn raw(&self) -> &ash::Device {
        &self.logical.device
    }

    /// Gives a Vulkan object a name for validation messages and GPU debuggers (ADR-0003). Does
    /// nothing without `VK_EXT_debug_utils`.
    pub(crate) fn name_object<H: vk::Handle>(&self, handle: H, name: &str) {
        let Some(debug) = &self.debug else {
            return;
        };
        let Ok(name) = CString::new(name.replace('\0', "")) else {
            return;
        };
        let info = vk::DebugUtilsObjectNameInfoEXT::default()
            .object_handle(handle)
            .object_name(&name);
        // SAFETY: `handle` is a live object of this device, and `info` and `name` are alive
        // across the call. Naming has no other preconditions.
        if let Err(e) = unsafe { debug.set_debug_utils_object_name(&info) } {
            warn!(error = ?e, "failed to name a Vulkan object");
        }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // No encoder can exist any more: each holds an `Arc` to this. Wait before freeing
        // anything the GPU might still use.
        // SAFETY: the device is alive (it's destroyed later, by `LogicalDevice::drop`).
        if let Err(e) = unsafe { self.logical.device.device_wait_idle() } {
            warn!(error = ?e, "vkDeviceWaitIdle failed while destroying the device");
        }
        // Retired objects (surfaces included), then live ones the application didn't destroy.
        let retired = lock(&self.retired).drain();
        let textures = lock(&self.textures).drain();
        let buffers = lock(&self.buffers).drain();
        let pipelines = lock(&self.pipelines).drain();
        let leftovers = retired
            .into_iter()
            .chain(textures.into_iter().filter_map(|t| {
                // Backbuffers left the table when their surface was retired, so these are all
                // owned textures.
                t.allocation.map(|allocation| Retired::Texture {
                    image: t.image,
                    view: t.view,
                    allocation,
                })
            }))
            .chain(buffers.into_iter().map(|b| Retired::Buffer {
                buffer: b.buffer,
                allocation: b.allocation,
            }))
            .chain(pipelines.into_iter().map(|p| Retired::Pipeline(p.pipeline)));
        for object in leftovers {
            self.destroy_retired(object);
        }
        // SAFETY: the GPU is idle and every pipeline using the layout was destroyed above.
        unsafe {
            self.logical
                .device
                .destroy_pipeline_layout(self.pipeline_layout, None);
        }
        // SAFETY: the GPU is idle, so no submission still signals or waits on the timeline.
        unsafe { self.logical.device.destroy_semaphore(self.timeline, None) };
        // Free GPU memory before the logical device is destroyed (field order would do the
        // same; this states the requirement).
        drop(lock(&self.allocator).take());
    }
}

/// A Vulkan device: the selected GPU, a graphics+compute queue, the memory allocator, and the
/// surfaces, textures and pipelines created on it.
///
/// Dropping it retires its surfaces. The rest of the device is destroyed once no encoder
/// references it any more, which is immediately unless an encoder outlives the device.
pub struct VulkanDevice {
    pub(crate) surfaces: Mutex<HandlePool<Surface>>,
    pub(crate) shared: Arc<Shared>,
    adapter_info: AdapterInfo,
}

impl VulkanDevice {
    /// The logical device.
    pub(crate) fn raw(&self) -> &ash::Device {
        self.shared.raw()
    }
}

impl std::fmt::Debug for VulkanDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VulkanDevice")
            .field("adapter", &self.adapter_info.name)
            .field("queue_family", &self.shared.queue_family)
            .field("physical_device", &self.shared.physical_device)
            .field("presentation", &self.shared.swapchain_loader.is_some())
            .field("instance", &self.shared.instance)
            .finish_non_exhaustive()
    }
}

impl Drop for VulkanDevice {
    fn drop(&mut self) {
        // Surfaces the application didn't destroy. They wait in the retire queue for any frame
        // encoder still recording into them, and go with `Shared` at the latest.
        let surfaces = lock(&self.surfaces).drain();
        for surface in surfaces {
            self.retire_surface(surface);
        }
    }
}

impl oxreal_rhi::Device for VulkanDevice {
    fn adapter_info(&self) -> &AdapterInfo {
        &self.adapter_info
    }

    fn validation_enabled(&self) -> bool {
        self.shared.instance.validation_enabled()
    }

    fn validation_error_count(&self) -> u64 {
        self.shared.instance.validation_error_count()
    }

    fn wait_idle(&self) -> Result<(), RhiError> {
        self.shared.wait_idle()?;
        self.shared.collect_garbage()
    }

    fn create_surface(
        &self,
        window: Arc<dyn SurfaceTarget>,
        desc: &SurfaceDesc,
    ) -> Result<SurfaceHandle, RhiError> {
        self.create_surface_impl(window, desc)
    }

    fn resize_surface(
        &self,
        surface: SurfaceHandle,
        width: u32,
        height: u32,
    ) -> Result<(), RhiError> {
        self.resize_surface_impl(surface, width, height)
    }

    fn destroy_surface(&self, surface: SurfaceHandle) -> Result<(), RhiError> {
        self.destroy_surface_impl(surface)
    }

    fn begin_frame(&self, surface: SurfaceHandle) -> Result<Option<Frame>, RhiError> {
        self.begin_frame_impl(surface)
    }

    fn end_frame(&self, frame: Frame) -> Result<(), RhiError> {
        self.end_frame_impl(frame)
    }

    fn create_texture(&self, desc: &TextureDesc<'_>) -> Result<TextureHandle, RhiError> {
        self.shared.create_texture(desc)
    }

    fn destroy_texture(&self, texture: TextureHandle) -> Result<(), RhiError> {
        self.shared.destroy_texture(texture)
    }

    fn create_render_pipeline(
        &self,
        desc: &RenderPipelineDesc<'_>,
    ) -> Result<PipelineHandle, RhiError> {
        self.shared.create_render_pipeline(desc)
    }

    fn destroy_render_pipeline(&self, pipeline: PipelineHandle) -> Result<(), RhiError> {
        self.shared.destroy_render_pipeline(pipeline)
    }

    fn create_encoder(&self, label: &str) -> Result<Box<dyn CommandEncoder>, RhiError> {
        Shared::create_encoder(&self.shared, label)
    }

    fn submit(&self, encoder: Box<dyn CommandEncoder>) -> Result<SubmissionId, RhiError> {
        self.shared.submit(encoder)
    }

    fn wait_for_submission(&self, submission: SubmissionId) -> Result<(), RhiError> {
        self.shared.wait_for_submission(submission)
    }

    fn read_texture(&self, texture: TextureHandle) -> Result<Vec<u8>, RhiError> {
        Shared::read_texture(&self.shared, texture)
    }

    fn create_buffer(&self, desc: &BufferDesc<'_>) -> Result<BufferHandle, RhiError> {
        self.shared.create_buffer(desc)
    }

    fn destroy_buffer(&self, buffer: BufferHandle) -> Result<(), RhiError> {
        self.shared.destroy_buffer(buffer)
    }

    fn write_buffer(&self, buffer: BufferHandle, offset: u64, data: &[u8]) -> Result<(), RhiError> {
        Shared::write_buffer(&self.shared, buffer, offset, data)
    }

    fn read_buffer(&self, buffer: BufferHandle) -> Result<Vec<u8>, RhiError> {
        Shared::read_buffer(&self.shared, buffer)
    }

    fn buffer_address(&self, buffer: BufferHandle) -> Result<u64, RhiError> {
        self.shared.buffer_address(buffer)
    }
}

/// Owns the `VkDevice`, destroying it on drop.
struct LogicalDevice {
    device: ash::Device,
}

impl Drop for LogicalDevice {
    fn drop(&mut self) {
        // SAFETY: this only drops as a field of `Shared`, after `Shared::drop` waited for the GPU
        // and destroyed every object created from the device and freed its memory. `Shared`
        // outlives every encoder, so no other user of the device remains.
        unsafe { self.device.destroy_device(None) };
    }
}

/// What probing a physical device found out.
struct Probe {
    physical_device: vk::PhysicalDevice,
    candidate: AdapterCandidate,
    queue_family: Option<u32>,
    limits: vk::PhysicalDeviceLimits,
    /// `maxBufferSize` (Vulkan 1.3 core, from maintenance4).
    max_buffer_size: u64,
}

fn probe(instance: &ash::Instance, physical_device: vk::PhysicalDevice) -> Probe {
    // SAFETY: `physical_device` was enumerated from `instance`, which is alive.
    let props = unsafe { instance.get_physical_device_properties(physical_device) };
    let api = props.api_version;
    let mut reasons = Vec::new();

    let (driver_name, driver_info) = if api >= vk::API_VERSION_1_2 {
        let mut v12 = vk::PhysicalDeviceVulkan12Properties::default();
        let mut props2 = vk::PhysicalDeviceProperties2::default().push_next(&mut v12);
        // SAFETY: the device supports Vulkan 1.2, so Vulkan12Properties is a valid chain member.
        // Both structures are alive across the call.
        unsafe { instance.get_physical_device_properties2(physical_device, &mut props2) };
        (
            v12.driver_name_as_c_str()
                .map_or_else(|_| String::new(), |s| s.to_string_lossy().into_owned()),
            v12.driver_info_as_c_str()
                .map_or_else(|_| String::new(), |s| s.to_string_lossy().into_owned()),
        )
    } else {
        (String::new(), String::new())
    };

    // 0 for devices below Vulkan 1.3, which are rejected.
    let mut max_buffer_size = 0;
    if api < vk::API_VERSION_1_3 {
        reasons.push(format!(
            "supports Vulkan {} (1.3 required)",
            format_version(api)
        ));
    } else {
        let mut v13 = vk::PhysicalDeviceVulkan13Properties::default();
        let mut props2 = vk::PhysicalDeviceProperties2::default().push_next(&mut v13);
        // SAFETY: the device supports Vulkan 1.3, so Vulkan13Properties is a valid chain member.
        // Both structures are alive across the call.
        unsafe { instance.get_physical_device_properties2(physical_device, &mut props2) };
        max_buffer_size = v13.max_buffer_size;
        let available = query_features(instance, physical_device);
        reasons.extend(
            features::missing(&available)
                .into_iter()
                .map(|name| format!("missing feature {name}")),
        );
    }

    // SAFETY: `physical_device` belongs to the live `instance`.
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
    let wanted = vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE;
    let queue_family = families
        .iter()
        .position(|f| f.queue_flags.contains(wanted) && f.queue_count > 0)
        .and_then(|i| u32::try_from(i).ok());
    if queue_family.is_none() {
        reasons.push("no queue family supports both graphics and compute".to_owned());
    }

    // SAFETY: `physical_device` belongs to the live `instance`.
    let memory = unsafe { instance.get_physical_device_memory_properties(physical_device) };
    let device_local_bytes = memory
        .memory_heaps_as_slice()
        .iter()
        .filter(|h| h.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
        .map(|h| h.size)
        .max()
        .unwrap_or(0);

    let name = props.device_name_as_c_str().map_or_else(
        |_| "(unnamed GPU)".to_owned(),
        |s| s.to_string_lossy().into_owned(),
    );

    Probe {
        physical_device,
        candidate: AdapterCandidate {
            info: AdapterInfo {
                name,
                adapter_type: adapter_type(props.device_type),
                vendor_id: props.vendor_id,
                device_id: props.device_id,
                driver_name,
                driver_info,
                api_version: ApiVersion::new(
                    vk::api_version_major(api),
                    vk::api_version_minor(api),
                    vk::api_version_patch(api),
                ),
            },
            device_local_bytes,
            rejection_reasons: reasons,
        },
        queue_family,
        limits: props.limits,
        max_buffer_size,
    }
}

/// Reads the features Oxreal cares about. The device must support Vulkan 1.3.
fn query_features(instance: &ash::Instance, physical_device: vk::PhysicalDevice) -> FeatureSet {
    let mut v12 = vk::PhysicalDeviceVulkan12Features::default();
    let mut v13 = vk::PhysicalDeviceVulkan13Features::default();
    let core = {
        let mut features2 = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut v12)
            .push_next(&mut v13);
        // SAFETY: the caller checked Vulkan 1.3 support, so both chained structures are valid.
        // The chain's structures outlive the call.
        unsafe { instance.get_physical_device_features2(physical_device, &mut features2) };
        features2.features
    };
    // The chain no longer exists. Clear the dangling links before storing the structures.
    v12.p_next = std::ptr::null_mut();
    v13.p_next = std::ptr::null_mut();
    FeatureSet { core, v12, v13 }
}

fn create_logical_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family: u32,
    extensions: &[&CStr],
) -> Result<LogicalDevice, RhiError> {
    let extension_names: Vec<*const std::ffi::c_char> =
        extensions.iter().map(|name| name.as_ptr()).collect();
    let priorities = [1.0_f32];
    let queue_info = vk::DeviceQueueCreateInfo::default()
        .queue_family_index(queue_family)
        .queue_priorities(&priorities);
    let queue_infos = [queue_info];

    // Enable exactly the required features, which were verified during selection.
    let FeatureSet {
        core,
        mut v12,
        mut v13,
    } = features::required_set();
    let mut features2 = vk::PhysicalDeviceFeatures2::default()
        .features(core)
        .push_next(&mut v12)
        .push_next(&mut v13);
    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_infos)
        .enabled_extension_names(&extension_names)
        .push_next(&mut features2);

    // SAFETY: `physical_device` belongs to `instance`. Every enabled feature and extension was
    // verified as supported. All structures and name strings referenced by `create_info` are
    // alive across the call. `pEnabledFeatures` is null because PhysicalDeviceFeatures2 is
    // chained, as the spec requires.
    let device = unsafe { instance.create_device(physical_device, &create_info, None) }
        .map_err(|e| from_vk("vkCreateDevice", e))?;
    Ok(LogicalDevice { device })
}

fn adapter_type(t: vk::PhysicalDeviceType) -> AdapterType {
    match t {
        vk::PhysicalDeviceType::DISCRETE_GPU => AdapterType::Discrete,
        vk::PhysicalDeviceType::INTEGRATED_GPU => AdapterType::Integrated,
        vk::PhysicalDeviceType::VIRTUAL_GPU => AdapterType::Virtual,
        vk::PhysicalDeviceType::CPU => AdapterType::Cpu,
        _ => AdapterType::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_types_map_to_adapter_types() {
        assert_eq!(
            adapter_type(vk::PhysicalDeviceType::DISCRETE_GPU),
            AdapterType::Discrete
        );
        assert_eq!(adapter_type(vk::PhysicalDeviceType::CPU), AdapterType::Cpu);
        assert_eq!(
            adapter_type(vk::PhysicalDeviceType::OTHER),
            AdapterType::Other
        );
    }
}
