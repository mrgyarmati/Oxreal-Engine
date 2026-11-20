//! The world: every entity, component and resource of one simulation.

use std::any::TypeId;
use std::fmt;

use crate::bundle::{self, Bundle};
use crate::change::Mut;
use crate::component::{Component, Components};
use crate::entity::{Entities, Entity, Location};
use crate::query::{self, QueryData, QueryFilter, QueryIter, ReadOnlyQueryData};
use crate::resource::{Resource, Resources};
use crate::storage::{self, ArchetypeId, Archetypes};

/// Why an operation on a world failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EcsError {
    /// The entity isn't alive: it was despawned.
    #[error("{0:?} doesn't exist: it was despawned")]
    NoSuchEntity(Entity),
}

/// Every entity, component and resource of one simulation (ADR-0014).
///
/// Structural changes ([`World::spawn`], [`World::insert`], [`World::remove`],
/// [`World::despawn`]) take `&mut self`, as do mutable queries. Read-only queries and lookups
/// take `&self`.
pub struct World {
    entities: Entities,
    components: Components,
    archetypes: Archetypes,
    resources: Resources,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for World {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("World")
            .field("entities", &self.entities.len())
            .field("component_types", &self.components.len())
            .field("archetypes", &self.archetypes.len())
            .finish_non_exhaustive()
    }
}

impl World {
    /// An empty world.
    #[must_use]
    pub fn new() -> Self {
        let components = Components::default();
        Self {
            archetypes: Archetypes::new(&components),
            entities: Entities::default(),
            components,
            resources: Resources::default(),
        }
    }

    /// Creates an entity with the components of `bundle`.
    ///
    /// # Panics
    ///
    /// If `bundle` contains a component type twice, or the world already holds 2³² entities.
    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> Entity {
        let archetype = self.insert_target::<B>(ArchetypeId::EMPTY);
        let table = self.archetypes.get_mut(archetype);
        let row = table.len();
        let entity = self.entities.allocate(Location { archetype, row });
        table.push_entity(entity);
        bundle.write(table, row, &self.components);
        table.debug_check_rows();
        entity
    }

    /// Removes an entity and drops its components. Returns `false` if it wasn't alive.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        let Some(location) = self.entities.free(entity) else {
            return false;
        };
        let table = self.archetypes.get_mut(location.archetype);
        if let Some(moved) = table.swap_remove(location.row) {
            self.entities.set_location(moved, location);
        }
        true
    }

    /// Whether `entity` is alive in this world.
    #[must_use]
    pub fn contains(&self, entity: Entity) -> bool {
        self.entities.location(entity).is_some()
    }

    /// How many entities are alive.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Whether no entity is alive.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds the components of `bundle` to `entity`. Components it already has are replaced, and
    /// the old values dropped.
    ///
    /// # Errors
    ///
    /// [`EcsError::NoSuchEntity`] if `entity` isn't alive.
    ///
    /// # Panics
    ///
    /// If `bundle` contains a component type twice.
    pub fn insert<B: Bundle>(&mut self, entity: Entity, bundle: B) -> Result<(), EcsError> {
        let location = self
            .entities
            .location(entity)
            .ok_or(EcsError::NoSuchEntity(entity))?;
        let target = self.insert_target::<B>(location.archetype);
        let row = if target == location.archetype {
            location.row
        } else {
            let (from, to) = self.archetypes.pair_mut(location.archetype, target);
            let (row, moved) = storage::move_row(from, location.row, to, |id, _, _| {
                unreachable!("inserting dropped component {id:?}: a bug in oxreal-ecs")
            });
            self.relocate(entity, location, target, row, moved);
            row
        };
        let table = self.archetypes.get_mut(target);
        bundle.write(table, row, &self.components);
        table.debug_check_rows();
        Ok(())
    }

    /// Takes component `T` off `entity` and returns it. `None` if `entity` isn't alive or
    /// doesn't have a `T`.
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        let location = self.entities.location(entity)?;
        let id = self.components.id::<T>()?;
        if !self.archetypes.get(location.archetype).contains(id) {
            return None;
        }
        let target = self
            .archetypes
            .remove_target(location.archetype, id, &self.components);
        let (from, to) = self.archetypes.pair_mut(location.archetype, target);
        let mut taken = None;
        let (row, moved) = storage::move_row(from, location.row, to, |left, column, row| {
            if left == id {
                taken = Some(storage::typed_mut::<T>(column).swap_remove(row));
            } else {
                unreachable!("removing a component dropped {left:?} too: a bug in oxreal-ecs");
            }
        });
        self.relocate(entity, location, target, row, moved);
        taken
    }

    /// `entity`'s component `T`. `None` if `entity` isn't alive or doesn't have a `T`.
    #[must_use]
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        let location = self.entities.location(entity)?;
        let id = self.components.id::<T>()?;
        self.archetypes
            .get(location.archetype)
            .column::<T>(id)?
            .get(location.row)
    }

    /// `entity`'s component `T`, for writing through [`Mut`]. `None` if `entity` isn't alive or
    /// doesn't have a `T`.
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<Mut<'_, T>> {
        let location = self.entities.location(entity)?;
        let id = self.components.id::<T>()?;
        self.archetypes
            .get_mut(location.archetype)
            .column_mut::<T>(id)?
            .get_mut(location.row)
            .map(Mut::new)
    }

    /// Whether `entity` is alive and has a component `T`. Constant time: use it for role tests,
    /// the way Unreal code uses a cast (ADR-0008 §1).
    #[must_use]
    pub fn has<T: Component>(&self, entity: Entity) -> bool {
        let (Some(location), Some(id)) =
            (self.entities.location(entity), self.components.id::<T>())
        else {
            return false;
        };
        self.archetypes.get(location.archetype).contains(id)
    }

    /// Iterates every entity matching `D`, reading only.
    ///
    /// ```
    /// # use oxreal_ecs::{Component, World};
    /// struct Health(u32);
    /// impl Component for Health {}
    ///
    /// let mut world = World::new();
    /// world.spawn(Health(80));
    /// world.spawn(Health(100));
    /// let total: u32 = world.query::<&Health>().map(|health| health.0).sum();
    /// assert_eq!(total, 180);
    /// ```
    ///
    /// # Panics
    ///
    /// If `D` accesses a component mutably and another way too (see [`QueryData`]).
    pub fn query<D: ReadOnlyQueryData>(&self) -> QueryIter<'_, D> {
        QueryIter::shared(
            self.archetypes.iter(),
            query::prepare::<D, ()>(&self.components),
        )
    }

    /// Like [`World::query`], visiting only entities that pass filter `F`
    /// ([`crate::With`], [`crate::Without`]).
    ///
    /// # Panics
    ///
    /// If `D` accesses a component mutably and another way too (see [`QueryData`]).
    pub fn query_filtered<D: ReadOnlyQueryData, F: QueryFilter>(&self) -> QueryIter<'_, D, F> {
        QueryIter::shared(
            self.archetypes.iter(),
            query::prepare::<D, F>(&self.components),
        )
    }

    /// Iterates every entity matching `D`, which may write (`&mut T`).
    ///
    /// ```
    /// # use oxreal_ecs::{Component, World};
    /// struct Position(f32);
    /// impl Component for Position {}
    /// struct Velocity(f32);
    /// impl Component for Velocity {}
    ///
    /// let mut world = World::new();
    /// let ball = world.spawn((Position(0.0), Velocity(2.0)));
    /// for (mut position, velocity) in world.query_mut::<(&mut Position, &Velocity)>() {
    ///     position.0 += velocity.0;
    /// }
    /// assert_eq!(world.get::<Position>(ball).map(|p| p.0), Some(2.0));
    /// ```
    ///
    /// # Panics
    ///
    /// If `D` accesses a component mutably and another way too (see [`QueryData`]).
    pub fn query_mut<D: QueryData>(&mut self) -> QueryIter<'_, D> {
        let state = query::prepare::<D, ()>(&self.components);
        QueryIter::unique(self.archetypes.iter_mut(), state)
    }

    /// Like [`World::query_mut`], visiting only entities that pass filter `F`.
    ///
    /// # Panics
    ///
    /// If `D` accesses a component mutably and another way too (see [`QueryData`]).
    pub fn query_filtered_mut<D: QueryData, F: QueryFilter>(&mut self) -> QueryIter<'_, D, F> {
        let state = query::prepare::<D, F>(&self.components);
        QueryIter::unique(self.archetypes.iter_mut(), state)
    }

    /// Stores a resource, returning the one it replaces.
    pub fn insert_resource<R: Resource>(&mut self, value: R) -> Option<R> {
        self.resources.insert(value)
    }

    /// Takes a resource out of the world.
    pub fn remove_resource<R: Resource>(&mut self) -> Option<R> {
        self.resources.remove()
    }

    /// The resource of type `R`, if the world has one.
    #[must_use]
    pub fn resource<R: Resource>(&self) -> Option<&R> {
        self.resources.get()
    }

    /// The resource of type `R`, mutably.
    pub fn resource_mut<R: Resource>(&mut self) -> Option<&mut R> {
        self.resources.get_mut()
    }

    /// Where an entity in `from` goes when bundle `B` is inserted, using the cached edge when
    /// there is one.
    fn insert_target<B: Bundle>(&mut self, from: ArchetypeId) -> ArchetypeId {
        if let Some(to) = self
            .archetypes
            .cached_insert_target(from, TypeId::of::<B>())
        {
            return to;
        }
        let ids = bundle::component_ids::<B>(&mut self.components);
        self.archetypes
            .insert_target(from, TypeId::of::<B>(), &ids, &self.components)
    }

    /// Records the locations after `entity` moved from `old` to row `row` of `archetype`, with
    /// `moved` filling its old row.
    fn relocate(
        &mut self,
        entity: Entity,
        old: Location,
        archetype: ArchetypeId,
        row: usize,
        moved: Option<Entity>,
    ) {
        if let Some(moved) = moved {
            self.entities.set_location(moved, old);
        }
        self.entities
            .set_location(entity, Location { archetype, row });
    }
}
