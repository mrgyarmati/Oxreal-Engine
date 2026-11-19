//! Typed generational handles and the pool behind them (ADR-0016 §1).
//!
//! A handle is a 64-bit value: a 32-bit slot index and a 32-bit generation. Removing an object
//! bumps its slot's generation, so an old handle to that slot fails lookups instead of reaching
//! whatever object reuses the slot.

use std::fmt;

macro_rules! define_handle {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(transparent)]
        pub struct $name(u64);

        impl $name {
            /// Wraps a raw handle value. For backends: only values returned by a
            /// [`HandlePool`] are meaningful.
            #[must_use]
            pub const fn from_raw(raw: u64) -> Self {
                Self(raw)
            }

            /// The raw 64-bit value, for backends and the future C ABI.
            #[must_use]
            pub const fn to_raw(self) -> u64 {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    f,
                    "{}({}v{})",
                    stringify!($name),
                    self.0 & u64::from(u32::MAX),
                    self.0 >> 32
                )
            }
        }
    };
}

define_handle!(
    /// A presentation surface: a window the device can present to.
    SurfaceHandle
);
define_handle!(
    /// A texture, including swapchain backbuffers.
    TextureHandle
);
define_handle!(
    /// A render pipeline.
    PipelineHandle
);

/// Identifies a submission made with [`crate::Device::submit`]. Pass it to
/// [`crate::Device::wait_for_submission`] to wait for the GPU to finish it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubmissionId(pub u64);

/// Storage for backend objects, addressed by generational handles.
///
/// Raw handle `0` is never issued, because generations start at 1.
#[derive(Debug)]
pub struct HandlePool<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    len: usize,
}

#[derive(Debug)]
struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

impl<T> Default for HandlePool<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }
}

impl<T> HandlePool<T> {
    /// An empty pool.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores `value` and returns its raw handle.
    ///
    /// # Panics
    ///
    /// If more than `u32::MAX` slots would be needed. That's an engine bug, not a runtime
    /// condition.
    pub fn insert(&mut self, value: T) -> u64 {
        self.len += 1;
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.value = Some(value);
            return raw(index, slot.generation);
        }
        let index = u32::try_from(self.slots.len()).expect("handle pool exceeded u32::MAX slots");
        self.slots.push(Slot {
            generation: 1,
            value: Some(value),
        });
        raw(index, 1)
    }

    /// The object behind `handle`, if it's still alive.
    #[must_use]
    pub fn get(&self, handle: u64) -> Option<&T> {
        let (index, generation) = split(handle);
        self.slots
            .get(index as usize)
            .filter(|s| s.generation == generation)
            .and_then(|s| s.value.as_ref())
    }

    /// Mutable access to the object behind `handle`, if it's still alive.
    #[must_use]
    pub fn get_mut(&mut self, handle: u64) -> Option<&mut T> {
        let (index, generation) = split(handle);
        self.slots
            .get_mut(index as usize)
            .filter(|s| s.generation == generation)
            .and_then(|s| s.value.as_mut())
    }

    /// Removes and returns the object behind `handle`. Every copy of the handle becomes stale.
    pub fn remove(&mut self, handle: u64) -> Option<T> {
        let (index, generation) = split(handle);
        let slot = self.slots.get_mut(index as usize)?;
        if slot.generation != generation {
            return None;
        }
        let value = slot.value.take()?;
        self.len -= 1;
        // A slot whose generation would wrap is retired, so a stale handle can never match again.
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.free.push(index);
        }
        Some(value)
    }

    /// Number of live objects.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// `true` when no object is alive.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Removes and returns every live object, for teardown.
    pub fn drain(&mut self) -> Vec<T> {
        let mut out = Vec::with_capacity(self.len);
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if let Some(value) = slot.value.take() {
                out.push(value);
                if let Some(next) = slot.generation.checked_add(1) {
                    slot.generation = next;
                    if let Ok(index) = u32::try_from(index) {
                        self.free.push(index);
                    }
                }
            }
        }
        self.len = 0;
        out
    }
}

const fn raw(index: u32, generation: u32) -> u64 {
    ((generation as u64) << 32) | index as u64
}

const fn split(handle: u64) -> (u32, u32) {
    // Truncation is the point: the low and high 32 bits are the index and generation.
    (handle as u32, (handle >> 32) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove() {
        let mut pool = HandlePool::new();
        let a = pool.insert("a");
        let b = pool.insert("b");
        assert_ne!(a, 0, "raw 0 is never issued");
        assert_eq!(pool.get(a), Some(&"a"));
        assert_eq!(pool.len(), 2);
        assert_eq!(pool.remove(a), Some("a"));
        assert_eq!(pool.get(a), None);
        assert_eq!(pool.remove(a), None, "double remove is harmless");
        assert_eq!(pool.get(b), Some(&"b"));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn stale_handles_never_reach_a_reused_slot() {
        let mut pool = HandlePool::new();
        let old = pool.insert(1);
        pool.remove(old);
        let new = pool.insert(2);
        assert_eq!(split(old).0, split(new).0, "the slot is reused");
        assert_eq!(pool.get(old), None, "but the old handle is stale");
        assert_eq!(pool.get(new), Some(&2));
    }

    #[test]
    fn drain_empties_the_pool_and_invalidates_handles() {
        let mut pool = HandlePool::new();
        let handles: Vec<u64> = (0..3).map(|i| pool.insert(i)).collect();
        let mut drained = pool.drain();
        drained.sort_unstable();
        assert_eq!(drained, vec![0, 1, 2]);
        assert!(pool.is_empty());
        assert!(handles.iter().all(|&h| pool.get(h).is_none()));
    }

    #[test]
    fn exhausted_generations_retire_the_slot() {
        let mut pool = HandlePool::new();
        let h = pool.insert(());
        pool.slots[0].generation = u32::MAX;
        let maxed = raw(0, u32::MAX);
        assert!(pool.remove(maxed).is_some());
        assert!(
            pool.free.is_empty(),
            "a slot that can't get a fresh generation is not reused"
        );
        let _ = h;
    }

    #[test]
    fn handles_format_as_index_and_generation() {
        assert_eq!(
            format!("{:?}", TextureHandle::from_raw(raw(7, 2))),
            "TextureHandle(7v2)"
        );
    }
}
