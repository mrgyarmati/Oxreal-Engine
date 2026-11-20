//! GPU buffers (ADR-0016 §2): created in GPU memory, reached by shaders through their device
//! address, written and read back through staging copies on the queue.
//!
//! Writes go through the queue rather than mapped memory, so they take effect in submission
//! order and the CPU never writes memory a pending submission may be reading.

use std::sync::Arc;

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::Allocation;
use oxreal_rhi::{BufferDesc, BufferHandle, BufferUsage, RhiError};

use crate::device::{Shared, lock};
use crate::encoder::Registration;
use crate::error::from_vk;
use crate::resources::{Retired, invalid};

/// A buffer known to the device.
#[derive(Debug)]
pub(crate) struct Buffer {
    pub(crate) buffer: vk::Buffer,
    pub(crate) allocation: Allocation,
    pub(crate) size: u64,
    pub(crate) usage: BufferUsage,
    /// The device address, for buffers with `SHADER_ADDRESS` usage.
    pub(crate) address: Option<u64>,
}

fn vk_usage(usage: BufferUsage) -> vk::BufferUsageFlags {
    let mut flags = vk::BufferUsageFlags::empty();
    if usage.contains(BufferUsage::SHADER_ADDRESS) {
        // Shaders access it through a PhysicalStorageBuffer pointer (ADR-0016 §2).
        flags |= vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
    }
    if usage.contains(BufferUsage::COPY_DESTINATION) {
        flags |= vk::BufferUsageFlags::TRANSFER_DST;
    }
    if usage.contains(BufferUsage::COPY_SOURCE) {
        flags |= vk::BufferUsageFlags::TRANSFER_SRC;
    }
    if usage.contains(BufferUsage::INDEX) {
        flags |= vk::BufferUsageFlags::INDEX_BUFFER;
    }
    flags
}

/// A buffer with its memory, before it's registered anywhere. Destroyed by the caller on error.
struct RawBuffer {
    buffer: vk::Buffer,
    allocation: Allocation,
}

/// A CPU-writable buffer for uploads (`write_texture`). Retired after its submission, or
/// destroyed directly if it never reached the GPU.
pub(crate) struct StagingBuffer(RawBuffer);

impl StagingBuffer {
    pub(crate) fn buffer(&self) -> vk::Buffer {
        self.0.buffer
    }

    /// The first `len` bytes of its mapped memory. CpuToGpu memory is HOST_VISIBLE and
    /// HOST_COHERENT in gpu-allocator 0.28, so writes need no flush.
    pub(crate) fn mapped_mut(&mut self, len: usize) -> Option<&mut [u8]> {
        self.0
            .allocation
            .mapped_slice_mut()
            .and_then(|bytes| bytes.get_mut(..len))
    }

    pub(crate) fn into_retired(self) -> Retired {
        Retired::Buffer {
            buffer: self.0.buffer,
            allocation: self.0.allocation,
        }
    }
}

impl Shared {
    /// Creates a buffer and binds memory from `location`. On error nothing is left behind.
    fn create_raw_buffer(
        &self,
        label: &str,
        size: u64,
        usage: vk::BufferUsageFlags,
        location: MemoryLocation,
    ) -> Result<RawBuffer, RhiError> {
        let info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: the device is alive. The size is non-zero and at most maxBufferSize:
        // `create_buffer` checks both, and staging and readback buffers are as large as a checked
        // buffer or a part of one. The usage flags are supported (SHADER_DEVICE_ADDRESS needs
        // bufferDeviceAddress, a required feature), and exclusive sharing suits the device's
        // single queue family.
        let buffer = unsafe { self.raw().create_buffer(&info, None) }
            .map_err(|e| from_vk("vkCreateBuffer", e))?;
        // SAFETY: `buffer` was just created.
        let mut requirements = unsafe { self.raw().get_buffer_memory_requirements(buffer) };
        // Every buffer starts on a multiple of `BUFFER_ADDRESS_ALIGNMENT`, which
        // `Device::buffer_address` promises. Alignments are powers of two, so the larger one
        // still satisfies the driver's. This constrains the offset into the memory object; the
        // object's base meets the implementation's largest alignment requirement
        // (`vkAllocateMemory`), which the spec doesn't bound below, so `create_buffer` also checks
        // every address and refuses a misaligned one.
        requirements.alignment = requirements
            .alignment
            .max(oxreal_rhi::BUFFER_ADDRESS_ALIGNMENT);
        let allocation = match self.allocate(label, requirements, location, true) {
            Ok(allocation) => allocation,
            Err(error) => {
                // SAFETY: unused and destroyed exactly once.
                unsafe { self.raw().destroy_buffer(buffer, None) };
                return Err(error);
            }
        };
        // SAFETY: the allocation satisfies the buffer's requirements and is bound exactly once.
        // gpu-allocator allocates with DEVICE_ADDRESS when buffer_device_address is enabled
        // (it is, in `create_device`), which SHADER_DEVICE_ADDRESS buffers need.
        let bound = unsafe {
            self.raw()
                .bind_buffer_memory(buffer, allocation.memory(), allocation.offset())
        };
        if let Err(e) = bound {
            // SAFETY: never used. Destroyed exactly once, then its memory freed.
            unsafe { self.raw().destroy_buffer(buffer, None) };
            self.free(allocation);
            return Err(from_vk("vkBindBufferMemory", e));
        }
        self.name_object(buffer, label);
        Ok(RawBuffer { buffer, allocation })
    }

    /// A staging buffer of `size` bytes in CpuToGpu memory.
    pub(crate) fn create_staging_buffer(
        &self,
        label: &str,
        size: u64,
    ) -> Result<StagingBuffer, RhiError> {
        self.create_raw_buffer(
            label,
            size,
            vk::BufferUsageFlags::TRANSFER_SRC,
            MemoryLocation::CpuToGpu,
        )
        .map(StagingBuffer)
    }

    /// Destroys a staging buffer the GPU never used.
    pub(crate) fn destroy_staging_buffer(&self, staging: StagingBuffer) {
        self.destroy_raw_buffer(staging.0);
    }

    fn destroy_raw_buffer(&self, raw: RawBuffer) {
        // SAFETY: never used by the GPU (callers only do this before submitting). Destroyed
        // exactly once, then its memory freed.
        unsafe { self.raw().destroy_buffer(raw.buffer, None) };
        self.free(raw.allocation);
    }

    pub(crate) fn create_buffer(&self, desc: &BufferDesc<'_>) -> Result<BufferHandle, RhiError> {
        if desc.size == 0 {
            return Err(invalid(format!("buffer {:?} has zero size", desc.label)));
        }
        let max = self.limits.max_buffer_size;
        if desc.size > max {
            return Err(invalid(format!(
                "buffer {:?} is {} bytes, above this device's maxBufferSize of {max}",
                desc.label, desc.size
            )));
        }
        let usage = vk_usage(desc.usage);
        if usage.is_empty() {
            return Err(invalid(format!("buffer {:?} has no usage", desc.label)));
        }
        let raw = self.create_raw_buffer(desc.label, desc.size, usage, MemoryLocation::GpuOnly)?;
        let address = desc.usage.contains(BufferUsage::SHADER_ADDRESS).then(|| {
            let info = vk::BufferDeviceAddressInfo::default().buffer(raw.buffer);
            // SAFETY: the buffer is alive, bound to memory, and was created with
            // SHADER_DEVICE_ADDRESS usage; bufferDeviceAddress is enabled.
            unsafe { self.raw().get_buffer_device_address(&info) }
        });
        // `Device::buffer_address` promises the alignment; a driver that breaks it gets an error
        // here, never a misaligned address in a shader.
        if let Some(Err(error)) = address.map(check_address_alignment) {
            self.retire(Retired::Buffer {
                buffer: raw.buffer,
                allocation: raw.allocation,
            });
            return Err(error);
        }
        let handle = lock(&self.buffers).insert(Buffer {
            buffer: raw.buffer,
            allocation: raw.allocation,
            size: desc.size,
            usage: desc.usage,
            address,
        });
        Ok(BufferHandle::from_raw(handle))
    }

    pub(crate) fn destroy_buffer(&self, buffer: BufferHandle) -> Result<(), RhiError> {
        let removed = lock(&self.buffers)
            .remove(buffer.to_raw())
            .ok_or_else(|| invalid(format!("{buffer:?} does not exist")))?;
        self.retire(Retired::Buffer {
            buffer: removed.buffer,
            allocation: removed.allocation,
        });
        Ok(())
    }

    pub(crate) fn buffer_address(&self, buffer: BufferHandle) -> Result<u64, RhiError> {
        let buffers = lock(&self.buffers);
        let found = buffers
            .get(buffer.to_raw())
            .ok_or_else(|| invalid(format!("{buffer:?} does not exist")))?;
        found.address.ok_or_else(|| {
            invalid(format!(
                "{buffer:?} was not created with BufferUsage::SHADER_ADDRESS"
            ))
        })
    }

    /// Looks up a buffer for a copy, checking its usage and the byte range.
    fn copy_target(
        &self,
        buffer: BufferHandle,
        (needed, needed_name): (BufferUsage, &str),
        what: &str,
        range: Option<(u64, u64)>,
    ) -> Result<(vk::Buffer, u64), RhiError> {
        let buffers = lock(&self.buffers);
        let found = buffers
            .get(buffer.to_raw())
            .ok_or_else(|| invalid(format!("{buffer:?} does not exist")))?;
        if !found.usage.contains(needed) {
            return Err(invalid(format!(
                "{what}: {buffer:?} was not created with BufferUsage::{needed_name}"
            )));
        }
        if let Some((offset, len)) = range {
            let end = offset.checked_add(len);
            if len == 0 || end.is_none_or(|end| end > found.size) {
                return Err(invalid(format!(
                    "{what}: {len} bytes at offset {offset} don't fit {buffer:?} ({} bytes)",
                    found.size
                )));
            }
        }
        Ok((found.buffer, found.size))
    }

    pub(crate) fn write_buffer(
        shared: &Arc<Self>,
        buffer: BufferHandle,
        offset: u64,
        data: &[u8],
    ) -> Result<(), RhiError> {
        let len = u64::try_from(data.len()).map_err(|_| invalid("write_buffer: data too large"))?;
        // Registered like an encoder before the lookup, so destroying the buffer meanwhile
        // waits for this copy (ADR-0016 §1).
        let mut registration = Registration::new(shared);
        let (target, _) = shared.copy_target(
            buffer,
            (BufferUsage::COPY_DESTINATION, "COPY_DESTINATION"),
            "write_buffer",
            Some((offset, len)),
        )?;

        let mut staging = shared.create_raw_buffer(
            "write_buffer staging",
            len,
            vk::BufferUsageFlags::TRANSFER_SRC,
            MemoryLocation::CpuToGpu,
        )?;
        // CpuToGpu memory is HOST_VISIBLE and HOST_COHERENT in gpu-allocator 0.28 (required
        // flags), so the host write needs no flush, and the submission below makes it visible to
        // the copy. No GPU work can be using this memory: staging and readback allocations are
        // freed only after their submission completes, or at teardown once the device is idle.
        let Some(mapped) = staging
            .allocation
            .mapped_slice_mut()
            .and_then(|bytes| bytes.get_mut(..data.len()))
        else {
            shared.destroy_raw_buffer(staging);
            return Err(invalid("write_buffer: staging memory is not mapped"));
        };
        mapped.copy_from_slice(data);

        let recorded = shared.begin_one_time_commands("write_buffer");
        let (pool, cmd) = match recorded {
            Ok(commands) => commands,
            Err(error) => {
                shared.destroy_raw_buffer(staging);
                return Err(error);
            }
        };
        let before = [
            // Earlier GPU work may still read or write the destination range.
            vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .buffer(target)
                .offset(offset)
                .size(len),
            // Belt and braces: the staging memory may be reused from an earlier staging buffer,
            // whose GPU use completed before it was freed. Stated here so the ordering doesn't rest
            // on that alone (as the readback does).
            vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .buffer(staging.buffer)
                .size(vk::WHOLE_SIZE),
        ];
        let region = [vk::BufferCopy::default()
            .src_offset(0)
            .dst_offset(offset)
            .size(len)];
        // Later work, in any stage, sees the new data.
        let after = [vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
            .buffer(target)
            .offset(offset)
            .size(len)];
        // SAFETY: `cmd` is recording. Both buffers are alive. The target can't be destroyed before
        // this submission completes: `registration` was opened before `copy_target` looked the
        // buffer up, so a `destroy_buffer` that removes it afterwards is blocked by this
        // registration, and `submitted()` raises the wait to this copy's timeline value under the
        // queue lock. The staging buffer is retired only after this submission. The range was
        // checked against the target's size, and the barriers order the copy after earlier use
        // and before later use.
        unsafe {
            shared.raw().cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().buffer_memory_barriers(&before),
            );
            shared
                .raw()
                .cmd_copy_buffer(cmd, staging.buffer, target, &region);
            shared.raw().cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().buffer_memory_barriers(&after),
            );
        }
        match shared.submit_one_time_commands(pool, cmd, Some(&mut registration)) {
            Ok(value) => {
                lock(&shared.retired).retire_after_submission(
                    Retired::Buffer {
                        buffer: staging.buffer,
                        allocation: staging.allocation,
                    },
                    value,
                );
                // The write is submitted: from here on it succeeds, whatever cleanup reports.
                shared.collect_garbage_logged();
                Ok(())
            }
            Err(failure) => {
                // Freed now unless the copy may be pending (device lost).
                failure.dispose(
                    shared,
                    Retired::Buffer {
                        buffer: staging.buffer,
                        allocation: staging.allocation,
                    },
                );
                Err(failure.error)
            }
        }
    }

    pub(crate) fn read_buffer(
        shared: &Arc<Self>,
        buffer: BufferHandle,
    ) -> Result<Vec<u8>, RhiError> {
        let mut registration = Registration::new(shared);
        let (source, size) = shared.copy_target(
            buffer,
            (BufferUsage::COPY_SOURCE, "COPY_SOURCE"),
            "read_buffer",
            None,
        )?;
        let len = usize::try_from(size).map_err(|_| invalid("read_buffer: buffer too large"))?;
        // GpuToCpu memory is HOST_COHERENT in gpu-allocator 0.28, so no invalidation is needed.
        let readback = shared.create_raw_buffer(
            "read_buffer readback",
            size,
            vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuToCpu,
        )?;
        let (pool, cmd) = match shared.begin_one_time_commands("read_buffer") {
            Ok(commands) => commands,
            Err(error) => {
                shared.destroy_raw_buffer(readback);
                return Err(error);
            }
        };
        let before = [
            vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .buffer(source)
                .size(vk::WHOLE_SIZE),
            // Belt and braces: the readback memory may be reused from an earlier readback, whose
            // GPU use completed before it was freed.
            vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .buffer(readback.buffer)
                .size(vk::WHOLE_SIZE),
        ];
        let region = [vk::BufferCopy::default().size(size)];
        let after = [
            vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COPY)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                .dst_access_mask(vk::AccessFlags2::HOST_READ)
                .buffer(readback.buffer)
                .size(vk::WHOLE_SIZE),
            // Later work that writes the source (another thread may submit some before this
            // call's wait returns) must wait for the copy to finish reading it.
            // Write-after-read needs only an execution dependency.
            vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COPY)
                .src_access_mask(vk::AccessFlags2::NONE)
                .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .dst_access_mask(vk::AccessFlags2::NONE)
                .buffer(source)
                .size(vk::WHOLE_SIZE),
        ];
        // SAFETY: `cmd` is recording. The source can't be destroyed before this submission
        // completes (`registration`), and the readback buffer is this call's. The barriers order
        // the copy after earlier writes and make it visible to the host read after the wait.
        unsafe {
            shared.raw().cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().buffer_memory_barriers(&before),
            );
            shared
                .raw()
                .cmd_copy_buffer(cmd, source, readback.buffer, &region);
            shared.raw().cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().buffer_memory_barriers(&after),
            );
        }
        let value = match shared.submit_one_time_commands(pool, cmd, Some(&mut registration)) {
            Ok(value) => value,
            Err(failure) => {
                // Freed now unless the copy may be pending (device lost).
                failure.dispose(
                    shared,
                    Retired::Buffer {
                        buffer: readback.buffer,
                        allocation: readback.allocation,
                    },
                );
                return Err(failure.error);
            }
        };
        if let Err(error) = shared.wait_timeline(value) {
            lock(&shared.retired).retire_after_submission(
                Retired::Buffer {
                    buffer: readback.buffer,
                    allocation: readback.allocation,
                },
                value,
            );
            return Err(error);
        }
        let bytes = readback
            .allocation
            .mapped_slice()
            .and_then(|mapped| mapped.get(..len))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| invalid("read_buffer: readback memory is not mapped"));
        // The copy completed (its timeline value was reached), so the buffer is unused.
        shared.destroy_raw_buffer(readback);
        shared.collect_garbage_logged();
        bytes
    }
}

/// Refuses an address `Device::buffer_address` couldn't promise: not a multiple of
/// [`oxreal_rhi::BUFFER_ADDRESS_ALIGNMENT`].
fn check_address_alignment(address: u64) -> Result<(), RhiError> {
    if address.is_multiple_of(oxreal_rhi::BUFFER_ADDRESS_ALIGNMENT) {
        Ok(())
    } else {
        Err(RhiError::Backend {
            api: "Vulkan",
            operation: "vkGetBufferDeviceAddress",
            details: format!(
                "the driver placed a buffer at {address:#x}, which isn't {}-byte aligned",
                oxreal_rhi::BUFFER_ADDRESS_ALIGNMENT
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usages_map_to_vulkan_flags() {
        let address = vk_usage(BufferUsage::SHADER_ADDRESS);
        assert!(address.contains(
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
        ));
        let copies = vk_usage(BufferUsage::COPY_DESTINATION | BufferUsage::COPY_SOURCE);
        assert!(
            copies
                .contains(vk::BufferUsageFlags::TRANSFER_DST | vk::BufferUsageFlags::TRANSFER_SRC)
        );
        assert_eq!(
            vk_usage(BufferUsage::INDEX),
            vk::BufferUsageFlags::INDEX_BUFFER
        );
        assert!(vk_usage(BufferUsage::default()).is_empty());
    }

    #[test]
    fn misaligned_addresses_are_refused() {
        assert!(check_address_alignment(0x1000).is_ok());
        assert!(check_address_alignment(0x1010).is_ok());
        for address in [0x1001, 0x1004, 0x1008] {
            assert!(
                matches!(
                    check_address_alignment(address),
                    Err(RhiError::Backend { .. })
                ),
                "{address:#x}"
            );
        }
    }
}
