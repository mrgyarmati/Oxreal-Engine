//! Entities: generational handles and their allocator (ADR-0014 §3).

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};

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
}

/// Allocates entity handles and records where each live entity is stored.
#[derive(Debug, Default)]
pub(crate) struct Entities {
    slots: Vec<Slot>,
    /// Indices of dead slots ready for reuse, already at their next generation.
    free: Vec<u32>,
    live: usize,
}

impl Entities {
    /// A new handle. Its location must be set with [`Entities::set_location`] before it's used.
    ///
    /// # Panics
    ///
    /// When all 2³² indices are in use or retired: a world can't hold more entities.
    pub(crate) fn allocate(&mut self, location: Location) -> Entity {
        let index = match self.free.pop() {
            Some(index) => index,
            None => {
                let Ok(index) = u32::try_from(self.slots.len()) else {
                    panic!("entity limit reached: a world holds at most 2^32 entities");
                };
                self.slots.push(Slot {
                    generation: NonZeroU32::MIN,
                    location: None,
                });
                index
            }
        };
        let slot = &mut self.slots[index as usize];
        slot.location = Some(location);
        self.live += 1;
        Entity::new(index, slot.generation)
    }

    /// Frees a live entity's slot and returns where it was stored. `None` if `entity` isn't alive.
    pub(crate) fn free(&mut self, entity: Entity) -> Option<Location> {
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
        self.live -= 1;
        Some(location)
    }

    /// Where a live entity is stored. `None` for despawned or foreign handles.
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
        let entity = Entity::new(7, NonZeroU32::new(3).unwrap_or(NonZeroU32::MIN));
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
