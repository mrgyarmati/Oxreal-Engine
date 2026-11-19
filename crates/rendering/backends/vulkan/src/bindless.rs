//! The global bindless descriptor set (ADR-0016 §2): one set that every pipeline binds, holding
//! every sampled texture and every sampler at a stable index.
//!
//! - Both bindings are `PARTIALLY_BOUND | UPDATE_AFTER_BIND | UPDATE_UNUSED_WHILE_PENDING`, so a
//!   new index can be written while command buffers that bound the set are pending: they never
//!   use that slot.
//! - An index is freed only when its object is destroyed, which the retire queue delays until
//!   nothing can use it. So a slot is never rewritten while the GPU may read it.
//! - Descriptor writes require the set to be externally synchronized, so they happen under the
//!   `bindless_slots` lock.

use ash::vk;
use oxreal_diagnostics::info;
use oxreal_rhi::{RhiError, SAMPLERS_BINDING, TEXTURES_BINDING};

use crate::error::from_vk;

/// The most textures and samplers the set holds. The device's limits can lower these. They stay
/// within the Roadmap 2022 guaranteed limits (ADR-0016 §2).
const MAX_TEXTURES: u32 = 65_536;
const MAX_SAMPLERS: u32 = 2_048;

/// The device's update-after-bind descriptor limits (Vulkan 1.2 properties).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DescriptorLimits {
    pub(crate) set_sampled_images: u32,
    pub(crate) stage_sampled_images: u32,
    pub(crate) set_samplers: u32,
    pub(crate) stage_samplers: u32,
    pub(crate) stage_resources: u32,
    pub(crate) all_pools: u32,
    /// `maxSamplerAllocationCount`: samplers that may exist at once.
    pub(crate) sampler_allocations: u32,
}

/// How many textures and samplers the set holds on a device with `limits`.
///
/// The sampler count is also kept within `maxSamplerAllocationCount`. Every existing sampler,
/// including one waiting to be destroyed, holds an index, so that limit can never be exceeded.
pub(crate) fn capacities(limits: &DescriptorLimits) -> Result<(u32, u32), String> {
    let samplers = MAX_SAMPLERS
        .min(limits.set_samplers)
        .min(limits.stage_samplers)
        .min(limits.sampler_allocations);
    let textures = MAX_TEXTURES
        .min(limits.set_sampled_images)
        .min(limits.stage_sampled_images)
        .min(limits.stage_resources.saturating_sub(samplers))
        .min(limits.all_pools.saturating_sub(samplers));
    if textures == 0 || samplers == 0 {
        return Err(format!(
            "the device allows {textures} bindless textures and {samplers} samplers; Oxreal needs at least one of each"
        ));
    }
    Ok((textures, samplers))
}

/// Hands out indices `0..capacity`, reusing freed ones.
#[derive(Debug)]
pub(crate) struct IndexAllocator {
    next: u32,
    free: Vec<u32>,
    capacity: u32,
}

impl IndexAllocator {
    pub(crate) fn new(capacity: u32) -> Self {
        Self {
            next: 0,
            free: Vec::new(),
            capacity,
        }
    }

    /// A free index, or `None` when all are taken.
    pub(crate) fn allocate(&mut self) -> Option<u32> {
        if let Some(index) = self.free.pop() {
            return Some(index);
        }
        (self.next < self.capacity).then(|| {
            self.next += 1;
            self.next - 1
        })
    }

    /// Returns `index` for reuse. Callers only free an index whose object nothing can use.
    pub(crate) fn release(&mut self, index: u32) {
        debug_assert!(index < self.next && !self.free.contains(&index));
        self.free.push(index);
    }
}

/// The bindless indices in use, guarded together with the descriptor writes.
#[derive(Debug)]
pub(crate) struct BindlessSlots {
    pub(crate) textures: IndexAllocator,
    pub(crate) samplers: IndexAllocator,
}

/// The descriptor set layout, pool and set.
#[derive(Debug)]
pub(crate) struct Bindless {
    pub(crate) set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    pub(crate) set: vk::DescriptorSet,
}

impl Bindless {
    /// Creates the layout, a pool and the one set. On error nothing is left behind.
    pub(crate) fn new(
        device: &ash::Device,
        limits: &DescriptorLimits,
    ) -> Result<(Self, BindlessSlots), RhiError> {
        let (textures, samplers) = capacities(limits).map_err(|details| RhiError::Backend {
            api: "Vulkan",
            operation: "sizing the bindless descriptor set",
            details,
        })?;
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(TEXTURES_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(textures)
                .stage_flags(vk::ShaderStageFlags::ALL),
            vk::DescriptorSetLayoutBinding::default()
                .binding(SAMPLERS_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(samplers)
                .stage_flags(vk::ShaderStageFlags::ALL),
        ];
        let binding_flags = [vk::DescriptorBindingFlags::PARTIALLY_BOUND
            | vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
            | vk::DescriptorBindingFlags::UPDATE_UNUSED_WHILE_PENDING;
            2];
        let mut flags_info =
            vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&binding_flags);
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default()
            .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
            .bindings(&bindings)
            .push_next(&mut flags_info);
        // SAFETY: the device is alive and every structure is alive across the call. The binding
        // flags need descriptorBindingPartiallyBound, descriptorBindingSampledImageUpdateAfterBind
        // and descriptorBindingUpdateUnusedWhilePending (required features). Sampler update after
        // bind is core in Vulkan 1.2. The counts are within the update-after-bind limits
        // (`capacities`).
        let set_layout = unsafe { device.create_descriptor_set_layout(&layout_info, None) }
            .map_err(|e| from_vk("vkCreateDescriptorSetLayout", e))?;

        let sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(textures),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLER)
                .descriptor_count(samplers),
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND)
            .max_sets(1)
            .pool_sizes(&sizes);
        // SAFETY: the device is alive. UPDATE_AFTER_BIND matches the layout's flag.
        let pool = match unsafe { device.create_descriptor_pool(&pool_info, None) } {
            Ok(pool) => pool,
            Err(e) => {
                // SAFETY: unused and destroyed exactly once.
                unsafe { device.destroy_descriptor_set_layout(set_layout, None) };
                return Err(from_vk("vkCreateDescriptorPool", e));
            }
        };
        let layouts = [set_layout];
        let alloc = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        // SAFETY: the pool has room for exactly this set (sizes match the layout's counts).
        let set = match unsafe { device.allocate_descriptor_sets(&alloc) } {
            Ok(sets) => sets[0],
            Err(e) => {
                // SAFETY: both unused and destroyed exactly once; the pool frees nothing else.
                unsafe {
                    device.destroy_descriptor_pool(pool, None);
                    device.destroy_descriptor_set_layout(set_layout, None);
                }
                return Err(from_vk("vkAllocateDescriptorSets", e));
            }
        };
        info!(textures, samplers, "bindless descriptor set created");
        Ok((
            Self {
                set_layout,
                pool,
                set,
            },
            BindlessSlots {
                textures: IndexAllocator::new(textures),
                samplers: IndexAllocator::new(samplers),
            },
        ))
    }

    /// Points texture slot `index` at `view`. Call with the `bindless_slots` lock held.
    pub(crate) fn write_texture(&self, device: &ash::Device, index: u32, view: vk::ImageView) {
        let images = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(TEXTURES_BINDING)
            .dst_array_element(index)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(&images);
        // SAFETY: the set is alive and externally synchronized by the `bindless_slots` lock the
        // caller holds. `index` came from the texture allocator, so it's below the binding's count.
        // No pending command buffer uses the slot: a freed index's old texture was destroyed only
        // after everything that could sample it completed. With UPDATE_AFTER_BIND and
        // UPDATE_UNUSED_WHILE_PENDING the set may be bound in pending command buffers meanwhile.
        // The view is alive, and its texture rests in SHADER_READ_ONLY_OPTIMAL whenever a shader
        // may sample it.
        unsafe { device.update_descriptor_sets(&[write], &[]) };
    }

    /// Points sampler slot `index` at `sampler`. Call with the `bindless_slots` lock held.
    pub(crate) fn write_sampler(&self, device: &ash::Device, index: u32, sampler: vk::Sampler) {
        let samplers = [vk::DescriptorImageInfo::default().sampler(sampler)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(SAMPLERS_BINDING)
            .dst_array_element(index)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .image_info(&samplers);
        // SAFETY: as in `write_texture`, for the sampler binding and allocator.
        unsafe { device.update_descriptor_sets(&[write], &[]) };
    }

    /// Destroys the pool (which frees the set) and the layout. The device must be idle and no
    /// pipeline layout may use the set layout any more.
    pub(crate) fn destroy(&self, device: &ash::Device) {
        // SAFETY: per this function's contract, nothing uses either object. Each is destroyed
        // exactly once (only from `Shared::drop`).
        unsafe {
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generous() -> DescriptorLimits {
        DescriptorLimits {
            set_sampled_images: 500_000,
            stage_sampled_images: 500_000,
            set_samplers: 500_000,
            stage_samplers: 500_000,
            stage_resources: 500_000,
            all_pools: 500_000,
            sampler_allocations: 4_000,
        }
    }

    #[test]
    fn capacities_use_the_defaults_on_generous_devices() {
        assert_eq!(capacities(&generous()), Ok((MAX_TEXTURES, MAX_SAMPLERS)));
    }

    #[test]
    fn capacities_respect_every_limit() {
        let tight = DescriptorLimits {
            stage_resources: 10_000,
            sampler_allocations: 1_000,
            ..generous()
        };
        // Samplers are capped by allocations, and textures share the per-stage budget.
        assert_eq!(capacities(&tight), Ok((9_000, 1_000)));
        let none = DescriptorLimits {
            set_samplers: 0,
            ..generous()
        };
        assert!(capacities(&none).unwrap_err().contains("at least one"));
    }

    #[test]
    fn indices_are_reused_only_after_release() {
        let mut indices = IndexAllocator::new(2);
        let a = indices.allocate().unwrap();
        let b = indices.allocate().unwrap();
        assert_ne!(a, b);
        assert_eq!(indices.allocate(), None, "full");
        indices.release(a);
        assert_eq!(indices.allocate(), Some(a));
        assert_eq!(indices.allocate(), None);
    }
}
