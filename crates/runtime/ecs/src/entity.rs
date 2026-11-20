//! Entities: generational handles and their allocator (ADR-0014 §3).

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::atomic::{AtomicI64, Ordering};

use crate::storage::ArchetypeId;

/// A handle to an entity in a [`crate::World`].
///
/// 64 bits: a 32-bit index and a 32-bit generation. When an entity is despawned its index is
/// reused with a new generation, so an old handle **never aliases** a later entity. Looking it up
/// just fails. Handles are runtime-only: they're not stable across runs, so files and network
/// messages refer to entities by a persistent ID instead (ADR-0005).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Entity(NonZeroU64);

impl Entity {
    fn new(index: u32, generation: NonZeroU32) -> Self {
        let bits = (u64::from(generation.get()) << 32) | u64::from(index);
        // The generation is non-zero, so the bits are too.
        match NonZeroU64::new(bits) {
            Some(bits) => Self(bits),
            None => unreachable!("an entity's generation is never zero"),
        }
    }

    /// The slot this entity occupies. Indices are reused after despawning.
    #[must_use]
    pub const fn index(self) -> u32 {
        // Truncation keeps the low 32 bits: the index.
        self.0.get() as u32
    }

    /// How many times the slot has been reused, starting at 1.
    #[must_use]
    pub const fn generation(self) -> u32 {
        (self.0.get() >> 32) as u32
    }

    /// The handle as 64 bits, for passing across an FFI boundary (TAD §5.8).
    #[must_use]
    pub const fn to_bits(self) -> u64 {
        self.0.get()
    }

    /// A handle from [`Entity::to_bits`]. `None` if the bits can't be a handle (generation zero).
    #[must_use]
    pub const fn from_bits(bits: u64) -> Option<Self> {
        if bits >> 32 == 0 {
            return None;
        }
        match NonZeroU64::new(bits) {
            Some(bits) => Some(Self(bits)),
            None => None,
        }
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Entity({}v{})", self.index(), self.generation())
    }
}

/// Where an entity's components live: its archetype and its row there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Location {
    pub(crate) archetype: ArchetypeId,
    pub(crate) row: usize,
}

#[derive(Debug)]
struct Slot {
    generation: NonZeroU32,
    /// `Some` while the entity with this generation is alive.
    location: Option<Location>,
    /// Handed out by [`Entities::reserve`] and not spawned yet.
    reserved: bool,
}

/// Allocates entity handles and records where each live entity is stored.
///
/// Handles can also be **reserved** through a shared reference ([`Entities::reserve`]), which
/// is how commands return a real handle for an entity they'll spawn later. Reservations take
/// indices from the end of the free list, then fresh indices past the end, by moving an atomic
/// cursor. The free list itself only changes through `&mut self`, after
/// [`Entities::flush`] has turned the reservations into reserved slots.
#[derive(Debug, Default)]
pub(crate) struct Entities {
    slots: Vec<Slot>,
    /// Indices of dead slots ready for reuse, already at their next generation.
    free: Vec<u32>,
    /// `free.len()` minus the reservations since the last flush. Below zero, reservations have
    /// used the whole free list and continue with fresh indices.
    cursor: AtomicI64,
    live: usize,
}

impl Entities {
    /// A new live handle, stored at `location`.
    ///
    /// # Panics
    ///
    /// When all 2³² indices are in use or retired: a world can't hold more entities.
    pub(crate) fn allocate(&mut self, location: Location) -> Entity {
        self.flush();
        let index = match self.free.pop() {
            Some(index) => index,
            None => self.push_slot(false),
        };
        self.sync_cursor();
        let slot = &mut self.slots[index as usize];
        slot.location = Some(location);
        self.live += 1;
        Entity::new(index, slot.generation)
    }

    /// Reserves a handle for an entity that [`Entities::place_reserved`] will make live later.
    /// Until then, looking the handle up fails.
    ///
    /// # Panics
    ///
    /// When all 2³² indices are in use or retired.
    pub(crate) fn reserve(&self) -> Entity {
        let before = self.cursor.fetch_sub(1, Ordering::Relaxed);
        if before > 0 {
            // `before` is at most `free.len()`, which fits in usize.
            let index = self.free[(before - 1) as usize];
            return Entity::new(index, self.slots[index as usize].generation);
        }
        // `before` is 0, -1, -2, ...: the 1st, 2nd, 3rd fresh index past the end.
        let fresh = self.slots.len() as u64 + before.unsigned_abs();
        let Ok(index) = u32::try_from(fresh) else {
            // Undo this reservation, so a later flush doesn't try to create the slot.
            self.cursor.fetch_add(1, Ordering::Relaxed);
            panic!("entity limit reached: a world holds at most 2^32 entities");
        };
        Entity::new(index, NonZeroU32::MIN)
    }

    /// Turns the reservations made since the last flush into reserved slots.
    pub(crate) fn flush(&mut self) {
        let cursor = *self.cursor.get_mut();
        if cursor >= 0 {
            // `cursor` is at most `free.len()`.
            for index in self.free.drain(cursor as usize..) {
                self.slots[index as usize].reserved = true;
            }
        } else {
            for index in self.free.drain(..) {
                self.slots[index as usize].reserved = true;
            }
            for _ in 0..cursor.unsigned_abs() {
                self.push_slot(true);
            }
        }
        self.sync_cursor();
    }

    /// Makes a reserved handle live at `location`. Returns `false` if `entity` isn't a reserved,
    /// unspawned handle.
    pub(crate) fn place_reserved(&mut self, entity: Entity, location: Location) -> bool {
        self.flush();
        match self.slot_mut(entity) {
            Some(slot) if slot.reserved => {
                slot.reserved = false;
                slot.location = Some(location);
                self.live += 1;
                true
            }
            _ => false,
        }
    }

    /// Frees a live entity's slot and returns where it was stored. `None` if `entity` isn't alive.
    pub(crate) fn free(&mut self, entity: Entity) -> Option<Location> {
        self.flush();
        let slot = self.slots.get_mut(entity.index() as usize)?;
        if slot.generation.get() != entity.generation() {
            return None;
        }
        let location = slot.location.take()?;
        // A slot whose generation would wrap is retired rather than reused, so old handles can
        // never alias a new entity.
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            self.free.push(entity.index());
        }
        self.sync_cursor();
        self.live -= 1;
        Some(location)
    }

    /// Where a live entity is stored. `None` for despawned and reserved-but-unspawned handles.
    /// A handle from another world isn't detected: it may name an unrelated entity here.
    pub(crate) fn location(&self, entity: Entity) -> Option<Location> {
        let slot = self.slots.get(entity.index() as usize)?;
        if slot.generation.get() == entity.generation() {
            slot.location
        } else {
            None
        }
    }

    /// Records where a live entity is now stored.
    pub(crate) fn set_location(&mut self, entity: Entity, location: Location) {
        if let Some(slot) = self.slot_mut(entity)
            && slot.location.is_some()
        {
            slot.location = Some(location);
        } else {
            unreachable!("set_location on {entity:?}, which isn't alive: a bug in oxreal-ecs");
        }
    }

    /// How many entities are alive.
    pub(crate) fn len(&self) -> usize {
        self.live
    }

    fn push_slot(&mut self, reserved: bool) -> u32 {
        let Ok(index) = u32::try_from(self.slots.len()) else {
            panic!("entity limit reached: a world holds at most 2^32 entities");
        };
        self.slots.push(Slot {
            generation: NonZeroU32::MIN,
            location: None,
            reserved,
        });
        index
    }

    fn sync_cursor(&mut self) {
        // A Vec's length is at most isize::MAX, which fits in i64.
        *self.cursor.get_mut() = self.free.len() as i64;
    }

    fn slot_mut(&mut self, entity: Entity) -> Option<&mut Slot> {
        let slot = self.slots.get_mut(entity.index() as usize)?;
        (slot.generation.get() == entity.generation()).then_some(slot)
    }

    #[cfg(test)]
    pub(crate) fn force_generation(&mut self, index: u32, generation: NonZeroU32) {
        self.slots[index as usize].generation = generation;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here() -> Location {
        Location {
            archetype: ArchetypeId::EMPTY,
            row: 0,
        }
    }

    #[test]
    fn handles_round_trip_through_bits() {
        let entity = Entity::new(7, NonZeroU32::new(3).expect("3 is not zero"));
        assert_eq!(entity.index(), 7);
        assert_eq!(entity.generation(), 3);
        assert_eq!(Entity::from_bits(entity.to_bits()), Some(entity));
        assert_eq!(
            Entity::from_bits(7),
            None,
            "generation zero is never a handle"
        );
        assert_eq!(Entity::from_bits(0), None);
        assert_eq!(format!("{entity:?}"), "Entity(7v3)");
        assert_eq!(
            size_of::<Option<Entity>>(),
            8,
            "Option<Entity> uses the niche"
        );
    }

    #[test]
    fn freed_indices_are_reused_with_a_new_generation() {
        let mut entities = Entities::default();
        let a = entities.allocate(here());
        assert!(entities.location(a).is_some());
        assert_eq!(entities.free(a), Some(here()));
        assert_eq!(entities.location(a), None);
        assert_eq!(entities.free(a), None, "freeing twice fails");

        let b = entities.allocate(here());
        assert_eq!(b.index(), a.index());
        assert_ne!(b, a);
        assert_eq!(
            entities.location(a),
            None,
            "the old handle doesn't alias the new one"
        );
        assert!(entities.location(b).is_some());
        assert_eq!(entities.len(), 1);
    }

    #[test]
    fn reservations_take_free_slots_from_the_end_then_fresh_ones() {
        let mut entities = Entities::default();
        let all: Vec<Entity> = (0..4).map(|_| entities.allocate(here())).collect();
        entities.free(all[1]);
        entities.free(all[3]);
        // The free list is [1, 3]: reservations take 3, then 1, then fresh 4 and 5.
        let reserved: Vec<Entity> = (0..4).map(|_| entities.reserve()).collect();
        let indices: Vec<u32> = reserved.iter().map(|e| e.index()).collect();
        assert_eq!(indices, [3, 1, 4, 5]);
        assert_eq!(
            reserved[0].generation(),
            2,
            "a reused slot has its next generation"
        );
        assert_eq!(reserved[2].generation(), 1);
        assert!(
            reserved.iter().all(|&e| entities.location(e).is_none()),
            "not alive yet"
        );

        for &entity in &reserved {
            assert!(entities.place_reserved(entity, here()));
        }
        assert!(!entities.place_reserved(reserved[0], here()), "only once");
        assert_eq!(entities.len(), 6);
        // Nothing is left to reuse: the next allocation is fresh.
        assert_eq!(entities.allocate(here()).index(), 6);
    }

    #[test]
    fn a_reservation_within_the_free_list_leaves_the_rest_reusable() {
        let mut entities = Entities::default();
        let all: Vec<Entity> = (0..3).map(|_| entities.allocate(here())).collect();
        for &entity in &all {
            entities.free(entity);
        }
        let reserved = entities.reserve();
        assert_eq!(reserved.index(), 2);
        entities.flush();
        // Indices 0 and 1 are still free; 2 is reserved and not handed out again.
        let a = entities.allocate(here());
        let b = entities.allocate(here());
        assert_eq!([a.index(), b.index()], [1, 0]);
        assert_eq!(entities.allocate(here()).index(), 3);
        assert!(entities.place_reserved(reserved, here()));
    }

    #[test]
    fn a_slot_at_the_last_generation_is_retired() {
        let mut entities = Entities::default();
        let a = entities.allocate(here());
        entities.force_generation(a.index(), NonZeroU32::MAX);
        let last = Entity::new(a.index(), NonZeroU32::MAX);
        assert!(entities.free(last).is_some());
        let b = entities.allocate(here());
        assert_ne!(b.index(), a.index(), "the exhausted slot isn't reused");
        assert_eq!(entities.location(last), None);
    }
}
