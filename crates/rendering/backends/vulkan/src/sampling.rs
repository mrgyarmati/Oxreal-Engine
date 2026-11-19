//! Texture uploads and samplers (ADR-0016 §2): what shaders sample through bindless indices.

use std::sync::Arc;

use ash::vk;
use oxreal_rhi::{AddressMode, Filter, RhiError, SamplerDesc, SamplerHandle, TextureHandle};

use crate::device::{Shared, lock};
use crate::encoder::Registration;
use crate::error::from_vk;
use crate::present::color_subresource;
use crate::resources::{Retired, Sampler, invalid};

fn vk_filter(filter: Filter) -> vk::Filter {
    match filter {
        Filter::Nearest => vk::Filter::NEAREST,
        Filter::Linear => vk::Filter::LINEAR,
    }
}

fn vk_address_mode(mode: AddressMode) -> vk::SamplerAddressMode {
    match mode {
        AddressMode::Repeat => vk::SamplerAddressMode::REPEAT,
        AddressMode::MirroredRepeat => vk::SamplerAddressMode::MIRRORED_REPEAT,
        AddressMode::ClampToEdge => vk::SamplerAddressMode::CLAMP_TO_EDGE,
    }
}

impl Shared {
    pub(crate) fn write_texture(
        shared: &Arc<Self>,
        texture: TextureHandle,
        data: &[u8],
    ) -> Result<(), RhiError> {
        // Registered before the lookup, so a concurrent destroy waits for this copy, and the
        // claim keeps the texture's layout tracking in submission order (ADR-0016 §1).
        let mut registration = Registration::new(shared);
        let (image, extent, resting) = {
            let mut textures = lock(&shared.textures);
            let t = textures
                .get_mut(texture.to_raw())
                .ok_or_else(|| invalid(format!("{texture:?} does not exist")))?;
            if t.allocation.is_none() {
                return Err(invalid(format!(
                    "write_texture: {texture:?} is a swapchain backbuffer"
                )));
            }
            if !t.usage.contains(vk::ImageUsageFlags::TRANSFER_DST) {
                return Err(invalid(format!(
                    "write_texture: {texture:?} was not created with TextureUsage::COPY_DESTINATION"
                )));
            }
            let expected = u64::from(t.extent.width)
                * u64::from(t.extent.height)
                * u64::from(t.format.bytes_per_pixel());
            if u64::try_from(data.len()) != Ok(expected) {
                return Err(invalid(format!(
                    "write_texture: {} bytes for a {}×{} {:?} texture, which needs exactly {expected}",
                    data.len(),
                    t.extent.width,
                    t.extent.height,
                    t.format
                )));
            }
            registration
                .claim(texture.to_raw(), t)
                .map_err(|problem| invalid(format!("write_texture: {problem}")))?;
            // A sampled texture goes back to its resting layout in the same command buffer.
            t.layout = t
                .resting_layout
                .unwrap_or(vk::ImageLayout::TRANSFER_DST_OPTIMAL);
            (t.image, t.extent, t.resting_layout)
        };
        let len =
            u64::try_from(data.len()).map_err(|_| invalid("write_texture: data too large"))?;

        let mut staging = shared.create_staging_buffer("write_texture staging", len)?;
        let Some(mapped) = staging.mapped_mut(data.len()) else {
            shared.destroy_staging_buffer(staging);
            return Err(invalid("write_texture: staging memory is not mapped"));
        };
        mapped.copy_from_slice(data);

        let (pool, cmd) = match shared.begin_one_time_commands("write_texture") {
            Ok(commands) => commands,
            Err(error) => {
                shared.destroy_staging_buffer(staging);
                return Err(error);
            }
        };
        let before_image = [vk::ImageMemoryBarrier2::default()
            // Earlier work may still read (sample) or write the image. The whole image is
            // overwritten, so its old contents are discarded (UNDEFINED).
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .image(image)
            .subresource_range(color_subresource())];
        // Belt and braces: the staging memory may be reused from an earlier staging buffer,
        // whose GPU use completed before it was freed.
        let before_buffer = [vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
            .buffer(staging.buffer())
            .size(vk::WHOLE_SIZE)];
        let region = [vk::BufferImageCopy::default()
            // Rows tightly packed (row length and image height 0), top row first.
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })];
        // Later work sees the pixels. A sampled texture also returns to its resting layout.
        let after: Vec<vk::ImageMemoryBarrier2<'_>> = resting
            .into_iter()
            .map(|layout| {
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::COPY)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                    .dst_access_mask(vk::AccessFlags2::MEMORY_READ)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(layout)
                    .image(image)
                    .subresource_range(color_subresource())
            })
            .collect();
        // SAFETY: `cmd` is recording. The image is alive until this submission completes
        // (`registration` was opened before the lookup, so a concurrent destroy waits for it),
        // and it's claimed, so its layout tracking is in submission order. The staging buffer is
        // retired only after this submission. The copy covers exactly the image (the data's size
        // was checked), and the barriers order it after earlier use and before later use. A
        // texture without a resting layout stays in TRANSFER_DST_OPTIMAL, as tracked, and the
        // next user's barrier (source ALL_COMMANDS / MEMORY_WRITE) makes the copy visible.
        unsafe {
            shared.raw().cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default()
                    .image_memory_barriers(&before_image)
                    .buffer_memory_barriers(&before_buffer),
            );
            shared.raw().cmd_copy_buffer_to_image(
                cmd,
                staging.buffer(),
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &region,
            );
            if !after.is_empty() {
                shared.raw().cmd_pipeline_barrier2(
                    cmd,
                    &vk::DependencyInfo::default().image_memory_barriers(&after),
                );
            }
        }
        match shared.submit_one_time_commands(pool, cmd, Some(&mut registration)) {
            Ok(value) => {
                lock(&shared.retired).retire_after_submission(staging.into_retired(), value);
                // The upload is submitted: from here on it succeeds.
                shared.collect_garbage_logged();
                Ok(())
            }
            Err(failure) => {
                // Freed now unless the copy may be pending (device lost). If nothing was
                // submitted, `registration` restores the texture's layout when it drops.
                failure.dispose(shared, staging.into_retired());
                Err(failure.error)
            }
        }
    }

    pub(crate) fn create_sampler(&self, desc: &SamplerDesc<'_>) -> Result<SamplerHandle, RhiError> {
        let anisotropy = match desc.max_anisotropy {
            None => None,
            Some(value) if value.is_finite() && (1.0..=16.0).contains(&value) => {
                Some(value.min(self.limits.max_sampler_anisotropy))
            }
            Some(value) => {
                return Err(invalid(format!(
                    "sampler {:?}: max_anisotropy {value} must be between 1 and 16",
                    desc.label
                )));
            }
        };
        // The index first: every sampler that exists holds one, which keeps the count within
        // maxSamplerAllocationCount (the array was sized under it).
        let index =
            lock(&self.bindless_slots)
                .samplers
                .allocate()
                .ok_or(RhiError::OutOfMemory {
                    kind: "bindless sampler slot",
                    operation: "creating a sampler",
                })?;
        let address = vk_address_mode(desc.address_mode);
        let info = vk::SamplerCreateInfo::default()
            .mag_filter(vk_filter(desc.mag_filter))
            .min_filter(vk_filter(desc.min_filter))
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(address)
            .address_mode_v(address)
            .address_mode_w(address)
            .anisotropy_enable(anisotropy.is_some())
            .max_anisotropy(anisotropy.unwrap_or(1.0))
            .min_lod(0.0)
            .max_lod(vk::LOD_CLAMP_NONE);
        // SAFETY: the device is alive. Anisotropy needs samplerAnisotropy (a required feature)
        // and stays within maxSamplerAnisotropy (clamped above). Fewer samplers than
        // maxSamplerAllocationCount exist: each holds a bindless index, and there are at most that
        // many (`bindless::capacities`).
        let sampler = match unsafe { self.raw().create_sampler(&info, None) } {
            Ok(sampler) => sampler,
            Err(e) => {
                lock(&self.bindless_slots).samplers.release(index);
                return Err(from_vk("vkCreateSampler", e));
            }
        };
        self.name_object(sampler, desc.label);
        {
            let _slots = lock(&self.bindless_slots);
            self.bindless.write_sampler(self.raw(), index, sampler);
        }
        let raw = lock(&self.samplers).insert(Sampler { sampler, index });
        Ok(SamplerHandle::from_raw(raw))
    }

    pub(crate) fn destroy_sampler(&self, sampler: SamplerHandle) -> Result<(), RhiError> {
        let removed = lock(&self.samplers)
            .remove(sampler.to_raw())
            .ok_or_else(|| invalid(format!("{sampler:?} does not exist")))?;
        self.retire(Retired::Sampler {
            sampler: removed.sampler,
            index: removed.index,
        });
        Ok(())
    }

    pub(crate) fn sampler_index(&self, sampler: SamplerHandle) -> Result<u32, RhiError> {
        lock(&self.samplers)
            .get(sampler.to_raw())
            .map(|s| s.index)
            .ok_or_else(|| invalid(format!("{sampler:?} does not exist")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampler_settings_map_to_vulkan() {
        assert_eq!(vk_filter(Filter::Nearest), vk::Filter::NEAREST);
        assert_eq!(vk_filter(Filter::Linear), vk::Filter::LINEAR);
        assert_eq!(
            vk_address_mode(AddressMode::ClampToEdge),
            vk::SamplerAddressMode::CLAMP_TO_EDGE
        );
        assert_eq!(
            vk_address_mode(AddressMode::MirroredRepeat),
            vk::SamplerAddressMode::MIRRORED_REPEAT
        );
    }
}
