//! Resources: typed singletons stored in a world (ADR-0014 §8), with change detection like
//! components (§6).

use std::any::{Any, TypeId};

use crate::change::{ChangeWindow, Mut, Tick, ValueTicks};
use crate::type_map::TypeIdMap;

/// A value a world holds exactly one of, such as the frame's input or time.
///
/// Implement it explicitly: `impl Resource for GameSettings {}`.
pub trait Resource: Send + Sync + 'static {}

/// One stored resource and its ticks.
#[derive(Debug)]
struct Entry {
    value: Box<dyn Any + Send + Sync>,
    ticks: ValueTicks,
}

#[derive(Debug, Default)]
pub(crate) struct Resources {
    values: TypeIdMap<Entry>,
}

impl Resources {
    /// Stores `value` at `tick`. Replacing a resource changes it; storing one the world didn't
    /// have adds it.
    pub(crate) fn insert<R: Resource>(&mut self, value: R, tick: Tick) -> Option<R> {
        let id = TypeId::of::<R>();
        match self.values.get_mut(&id) {
            Some(entry) => {
                entry.ticks.changed.set(tick);
                let old = std::mem::replace(&mut entry.value, Box::new(value));
                Some(*downcast::<R>(old))
            }
            None => {
                self.values.insert(
                    id,
                    Entry {
                        value: Box::new(value),
                        ticks: ValueTicks::new(tick),
                    },
                );
                None
            }
        }
    }

    pub(crate) fn remove<R: Resource>(&mut self) -> Option<R> {
        self.values
            .remove(&TypeId::of::<R>())
            .map(|old| *downcast::<R>(old.value))
    }

    pub(crate) fn get<R: Resource>(&self) -> Option<&R> {
        self.values
            .get(&TypeId::of::<R>())
            .and_then(|entry| entry.value.downcast_ref())
    }

    /// The resource, for writing at `now`.
    pub(crate) fn get_mut<R: Resource>(&mut self, now: Tick) -> Option<Mut<'_, R>> {
        let Entry { value, ticks } = self.values.get_mut(&TypeId::of::<R>())?;
        let value = value.downcast_mut()?;
        Some(Mut::new(value, &ticks.changed, now))
    }

    /// Whether resource `R` was added within `window`. `false` if the world has none.
    pub(crate) fn is_added<R: Resource>(&self, window: ChangeWindow) -> bool {
        self.values
            .get(&TypeId::of::<R>())
            .is_some_and(|entry| window.contains(entry.ticks.added))
    }

    /// Whether resource `R` was added or changed within `window`. `false` if the world has none.
    pub(crate) fn is_changed<R: Resource>(&self, window: ChangeWindow) -> bool {
        self.values
            .get(&TypeId::of::<R>())
            .is_some_and(|entry| window.contains(entry.ticks.changed.get()))
    }

    /// Clamps every resource's ticks to at most `max_age` before `now`.
    pub(crate) fn clamp_ticks(&mut self, now: Tick, max_age: u32) {
        for entry in self.values.values_mut() {
            entry.ticks.added = entry.ticks.added.clamped(now, max_age);
            entry
                .ticks
                .changed
                .set(entry.ticks.changed.get().clamped(now, max_age));
        }
    }
}

fn downcast<R: Resource>(value: Box<dyn Any + Send + Sync>) -> Box<R> {
    match value.downcast() {
        Ok(value) => value,
        Err(_) => unreachable!(
            "a resource stored under {}'s type ID has another type: a bug in oxreal-ecs",
            std::any::type_name::<R>()
        ),
    }
}
