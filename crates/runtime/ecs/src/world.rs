//! The world: every entity, component and resource of one simulation.

use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, PoisonError};

use crate::bundle::{self, Bundle};
use crate::change::Mut;
use crate::command::{CommandQueue, Source};
use crate::component::{Component, ComponentId, Components, HookKind};
use crate::entity::{Entities, Entity, Location};
use crate::hook::HookWorld;
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
    /// Commands recorded by systems (applied at sync points) and by hooks (applied right after
    /// the change that triggered them).
    ///
    /// The `Mutex` is never locked: the queue is only reached through `&mut self`, with
    /// `Mutex::get_mut`, which costs nothing. It's there because commands are `Send` but not
    /// `Sync`, and the mutex keeps `World` `Sync` without `unsafe`.
    queue: Mutex<CommandQueue>,
    bundles: Bundles,
}

/// What the world knows about a bundle type.
#[derive(Debug)]
struct BundleInfo {
    /// Sorted component IDs.
    ids: Box<[ComponentId]>,
    /// Whether any of its components has a hook.
    hooked: bool,
    /// The archetype a new entity with exactly this bundle goes to, once known.
    spawn_archetype: Option<ArchetypeId>,
}

#[derive(Debug, Default)]
struct Bundles {
    index: HashMap<TypeId, usize>,
    list: Vec<BundleInfo>,
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
            queue: Mutex::new(CommandQueue::default()),
            bundles: Bundles::default(),
        }
    }

    /// Creates an entity with the components of `bundle`. Hooks run, then the commands they
    /// queued.
    ///
    /// # Panics
    ///
    /// If `bundle` contains a component type twice, or the world already holds 2³² entities.
    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> Entity {
        let info = self.bundle::<B>();
        let archetype = self.insert_target::<B>(ArchetypeId::EMPTY, info);
        let table = self.archetypes.get_mut(archetype);
        let row = table.len();
        let entity = self.entities.allocate(Location { archetype, row });
        table.push_entity(entity);
        bundle.write(table, row, &self.components);
        table.debug_check_rows();
        self.after_add(entity, info);
        self.apply_commands();
        entity
    }

    /// Removes an entity and drops its components. Returns `false` if it wasn't alive.
    ///
    /// Every component's `on_replace` hook runs, then every `on_remove`, while the entity still
    /// has all its components. Then it's removed, and the commands the hooks queued run.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        let Some(location) = self.entities.location(entity) else {
            return false;
        };
        let archetype = self.archetypes.get(location.archetype);
        if archetype.hooked() {
            let ids = archetype.components().to_vec();
            self.run_hooks(HookKind::Replace, entity, &ids);
            self.run_hooks(HookKind::Remove, entity, &ids);
        }
        // Hooks can't change structure, so the entity is still where it was.
        let Some(location) = self.entities.free(entity) else {
            unreachable!("{entity:?} died while its hooks ran: a bug in oxreal-ecs");
        };
        let table = self.archetypes.get_mut(location.archetype);
        if let Some(moved) = table.swap_remove(location.row) {
            self.entities.set_location(moved, location);
        }
        self.apply_commands();
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
        let info = self.bundle::<B>();
        let target = self.insert_target::<B>(location.archetype, info);
        // Components the entity already has are replaced: their `on_replace` hooks see the old
        // values. Hooks can't change structure, so `location` stays valid.
        let mut added = Vec::new();
        if self.bundles.list[info].hooked {
            let from = self.archetypes.get(location.archetype);
            let (replaced, new): (Vec<ComponentId>, Vec<ComponentId>) = self.bundles.list[info]
                .ids
                .iter()
                .partition(|&&id| from.contains(id));
            added = new;
            self.run_hooks(HookKind::Replace, entity, &replaced);
        }
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
        if self.bundles.list[info].hooked {
            let all = self.bundles.list[info].ids.to_vec();
            self.run_hooks(HookKind::Add, entity, &added);
            self.run_hooks(HookKind::Insert, entity, &all);
        }
        self.apply_commands();
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
        if self.components.hooks(id).any() {
            self.run_hooks(HookKind::Replace, entity, &[id]);
            self.run_hooks(HookKind::Remove, entity, &[id]);
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
        self.apply_commands();
        taken
    }

    /// `entity`'s component `T`. `None` if `entity` isn't alive or doesn't have a `T`.
    #[must_use]
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        get_component(&self.entities, &self.components, &self.archetypes, entity)
    }

    /// `entity`'s component `T`, for writing through [`Mut`]. `None` if `entity` isn't alive or
    /// doesn't have a `T`.
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<Mut<'_, T>> {
        get_component_mut(
            &self.entities,
            &self.components,
            &mut self.archetypes,
            entity,
        )
    }

    /// Whether `entity` is alive and has a component `T`. Constant time: use it for role tests,
    /// the way Unreal code uses a cast (ADR-0008 §1).
    #[must_use]
    pub fn has<T: Component>(&self, entity: Entity) -> bool {
        has_component::<T>(&self.entities, &self.components, &self.archetypes, entity)
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
    pub fn resource_mut<R: Resource>(&mut self) -> Option<Mut<'_, R>> {
        self.resources.get_mut().map(Mut::new)
    }

    /// Spawns `bundle` as `entity`, a handle reserved by [`crate::Commands::spawn`]. Returns
    /// `false`, dropping the bundle, if the handle isn't a reserved, unspawned one.
    pub(crate) fn spawn_reserved<B: Bundle>(&mut self, entity: Entity, bundle: B) -> bool {
        let info = self.bundle::<B>();
        let archetype = self.insert_target::<B>(ArchetypeId::EMPTY, info);
        let row = self.archetypes.get(archetype).len();
        if !self
            .entities
            .place_reserved(entity, Location { archetype, row })
        {
            return false;
        }
        let table = self.archetypes.get_mut(archetype);
        table.push_entity(entity);
        bundle.write(table, row, &self.components);
        table.debug_check_rows();
        self.after_add(entity, info);
        self.apply_commands();
        true
    }

    /// Applies queued commands in order: those systems recorded (at a sync point), or those
    /// hooks queued during the structural change that just finished.
    ///
    /// Each command's own structural changes apply the commands their hooks queue before the
    /// next command runs, so hook commands follow the change that caused them.
    pub(crate) fn apply_commands(&mut self) {
        if self.queue_mut().is_empty() {
            return;
        }
        self.entities.flush();
        for (source, command) in self.queue_mut().take() {
            command(self, &source);
        }
    }

    /// Discards queued commands without applying them, after a panic interrupted a stage.
    pub(crate) fn discard_commands(&mut self) -> bool {
        let queue = self.queue_mut();
        let had = !queue.is_empty();
        queue.clear();
        had
    }

    /// The parts a system or hook may touch while it runs: component storage (mutably, for
    /// queries), the registry and entities (read-only: structure doesn't change while it runs),
    /// resources, and the command queue.
    pub(crate) fn split(
        &mut self,
    ) -> (
        &mut Archetypes,
        &Components,
        &Entities,
        &mut Resources,
        &mut CommandQueue,
    ) {
        (
            &mut self.archetypes,
            &self.components,
            &self.entities,
            &mut self.resources,
            self.queue.get_mut().unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// The command queue. Never poisoned, because it's never locked.
    fn queue_mut(&mut self) -> &mut CommandQueue {
        self.queue.get_mut().unwrap_or_else(PoisonError::into_inner)
    }

    /// The index of bundle type `B`'s information, registered on first use.
    fn bundle<B: Bundle>(&mut self) -> usize {
        if let Some(&index) = self.bundles.index.get(&TypeId::of::<B>()) {
            return index;
        }
        let ids = bundle::component_ids::<B>(&mut self.components);
        let hooked = ids.iter().any(|&id| self.components.hooks(id).any());
        let index = self.bundles.list.len();
        self.bundles.list.push(BundleInfo {
            ids: ids.into(),
            hooked,
            spawn_archetype: None,
        });
        self.bundles.index.insert(TypeId::of::<B>(), index);
        index
    }

    /// Where an entity in `from` goes when bundle `B` (registered as `info`) is inserted, using
    /// the cached edge when there is one.
    fn insert_target<B: Bundle>(&mut self, from: ArchetypeId, info: usize) -> ArchetypeId {
        if from == ArchetypeId::EMPTY
            && let Some(archetype) = self.bundles.list[info].spawn_archetype
        {
            return archetype;
        }
        if let Some(to) = self
            .archetypes
            .cached_insert_target(from, TypeId::of::<B>())
        {
            return to;
        }
        let to = self.archetypes.insert_target(
            from,
            TypeId::of::<B>(),
            &self.bundles.list[info].ids,
            &self.components,
        );
        if from == ArchetypeId::EMPTY {
            self.bundles.list[info].spawn_archetype = Some(to);
        }
        to
    }

    /// Runs `on_add` and `on_insert` for every component of a newly spawned entity.
    fn after_add(&mut self, entity: Entity, info: usize) {
        if self.bundles.list[info].hooked {
            let ids = self.bundles.list[info].ids.to_vec();
            self.run_hooks(HookKind::Add, entity, &ids);
            self.run_hooks(HookKind::Insert, entity, &ids);
        }
    }

    /// Runs hook `kind` of each component in `ids` that has one, in order.
    fn run_hooks(&mut self, kind: HookKind, entity: Entity, ids: &[ComponentId]) {
        for &id in ids {
            if let Some(hook) = self.components.hooks(id).get(kind) {
                let source = Source::Hook {
                    component: self.components.name(id),
                    hook: kind.name(),
                };
                hook(&mut HookWorld::new(self.split(), source), entity);
            }
        }
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

/// `entity`'s component `T`. Shared by [`World::get`] and the system view.
pub(crate) fn get_component<'a, T: Component>(
    entities: &Entities,
    components: &Components,
    archetypes: &'a Archetypes,
    entity: Entity,
) -> Option<&'a T> {
    let location = entities.location(entity)?;
    let id = components.id::<T>()?;
    archetypes
        .get(location.archetype)
        .column::<T>(id)?
        .get(location.row)
}

/// `entity`'s component `T`, mutably. Shared by [`World::get_mut`] and the system view.
pub(crate) fn get_component_mut<'a, T: Component>(
    entities: &Entities,
    components: &Components,
    archetypes: &'a mut Archetypes,
    entity: Entity,
) -> Option<Mut<'a, T>> {
    let location = entities.location(entity)?;
    let id = components.id::<T>()?;
    archetypes
        .get_mut(location.archetype)
        .column_mut::<T>(id)?
        .get_mut(location.row)
        .map(Mut::new)
}

/// Whether `entity` is alive and has a `T`. Shared by [`World::has`] and the system view.
pub(crate) fn has_component<T: Component>(
    entities: &Entities,
    components: &Components,
    archetypes: &Archetypes,
    entity: Entity,
) -> bool {
    let (Some(location), Some(id)) = (entities.location(entity), components.id::<T>()) else {
        return false;
    };
    archetypes.get(location.archetype).contains(id)
}
