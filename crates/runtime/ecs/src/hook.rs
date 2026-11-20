//! What a lifecycle hook can reach (ADR-0014 §4).

use std::fmt;

use crate::change::Mut;
use crate::command::{CommandQueue, Commands, Source};
use crate::component::{Component, Components};
use crate::entity::{Entities, Entity};
use crate::resource::{Resource, Resources};
use crate::storage::Archetypes;
use crate::world;

/// A lifecycle [hook](crate::Hook)'s view of the world.
///
/// It reads and writes component values and resources, and queues [`Commands`] for structural
/// changes. Those commands run right after the change that triggered the hook. The world's
/// structure can't change while a hook runs, because the hook is called in the middle of a
/// change.
pub struct HookWorld<'w> {
    archetypes: &'w mut Archetypes,
    components: &'w Components,
    entities: &'w Entities,
    resources: &'w mut Resources,
    commands: Commands<'w>,
}

impl fmt::Debug for HookWorld<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HookWorld").finish_non_exhaustive()
    }
}

impl<'w> HookWorld<'w> {
    pub(crate) fn new(
        (archetypes, components, entities, resources, queue): (
            &'w mut Archetypes,
            &'w Components,
            &'w Entities,
            &'w mut Resources,
            &'w mut CommandQueue,
        ),
        source: Source,
    ) -> Self {
        Self {
            archetypes,
            components,
            entities,
            resources,
            commands: Commands::new(entities, queue, source),
        }
    }

    /// `entity`'s component `T`. `None` if `entity` isn't alive or doesn't have a `T`.
    #[must_use]
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        world::get_component(self.entities, self.components, self.archetypes, entity)
    }

    /// `entity`'s component `T`, for writing.
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<Mut<'_, T>> {
        world::get_component_mut(self.entities, self.components, self.archetypes, entity)
    }

    /// Whether `entity` is alive and has a `T`.
    #[must_use]
    pub fn has<T: Component>(&self, entity: Entity) -> bool {
        world::has_component::<T>(self.entities, self.components, self.archetypes, entity)
    }

    /// Whether `entity` is alive.
    #[must_use]
    pub fn contains(&self, entity: Entity) -> bool {
        self.entities.location(entity).is_some()
    }

    /// The resource of type `R`, if the world has one.
    #[must_use]
    pub fn resource<R: Resource>(&self) -> Option<&R> {
        self.resources.get()
    }

    /// The resource of type `R`, for writing.
    pub fn resource_mut<R: Resource>(&mut self) -> Option<Mut<'_, R>> {
        self.resources.get_mut().map(Mut::new)
    }

    /// Commands for structural changes, applied right after the change that triggered the hook.
    pub fn commands(&mut self) -> &mut Commands<'w> {
        &mut self.commands
    }
}
