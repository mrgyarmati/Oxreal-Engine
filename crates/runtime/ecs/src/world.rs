//! The world: every entity, component and resource of one simulation.

use std::any::TypeId;
use std::collections::HashSet;
use std::fmt;
use std::sync::{Mutex, PoisonError};

use oxreal_diagnostics::warn;

use crate::bundle::{self, Bundle};
use crate::change::{CHECK_INTERVAL, ChangeWindow, MAX_AGE, Mut, Tick};
use crate::command::{CommandQueue, Source};
use crate::component::{Component, ComponentId, Components, HookCause, HookContext, HookKind};
use crate::entity::{Entities, Entity, Location};
use crate::hook::HookWorld;
use crate::query::{self, QueryData, QueryFilter, QueryIter, ReadOnlyQueryData};
use crate::resource::{Resource, Resources};
use crate::storage::{self, ArchetypeId, Archetypes};
use crate::type_map::TypeIdMap;

/// Why an operation on a world failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EcsError {
    /// The entity isn't alive: it was despawned.
    #[error("{0:?} doesn't exist: it was despawned")]
    NoSuchEntity(Entity),
    /// The entity is being despawned (its hooks' commands are running), so its components can't
    /// change any more.
    #[error("{0:?} is being despawned, so its components can't change")]
    Despawning(Entity),
}

/// Every entity, component and resource of one simulation (ADR-0014).
///
/// Structural changes ([`World::spawn`], [`World::insert`], [`World::remove`],
/// [`World::despawn`]) take `&mut self`, as do mutable queries. Read-only queries and lookups
/// take `&self`. A structural change returns after the hooks it triggered, and the commands
/// they queued, have run.
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
    /// `Sync`, and the mutex keeps `World` `Sync` without `unsafe`, so a future parallel
    /// executor can share `&World` between read-only systems.
    queue: Mutex<CommandQueue>,
    /// Whether [`World::apply_commands`] is running. Nested structural changes then hand their
    /// hook commands to that loop instead of starting another one, so cascades don't recurse.
    applying: bool,
    /// Entities whose despawn has started but not finished: their hooks' commands are running.
    despawning: HashSet<Entity>,
    bundles: Bundles,
    /// How many ticks the world has counted (ADR-0014 §6). Each system run takes the current
    /// count and moves the world to the next ([`World::begin_system_run`]). 64 bits never wrap
    /// in practice, so systems remember their previous run exactly.
    tick_count: u64,
    /// The low 32 bits of `tick_count`: what writes are stamped with now.
    change_tick: Tick,
    /// The count when stored ticks were last clamped.
    last_clamp: u64,
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
    index: TypeIdMap<usize>,
    list: Vec<BundleInfo>,
}

/// The parts of a world a system or hook may touch while it runs.
#[derive(Debug)]
pub(crate) struct WorldParts<'a> {
    /// Component storage, mutably, for queries and lookups.
    pub(crate) archetypes: &'a mut Archetypes,
    pub(crate) components: &'a Components,
    /// Read-only: structure doesn't change while a system or hook runs.
    pub(crate) entities: &'a Entities,
    pub(crate) resources: &'a mut Resources,
    pub(crate) queue: &'a mut CommandQueue,
    /// Entities whose despawn has started, for `HookWorld::is_despawning`.
    pub(crate) despawning: &'a HashSet<Entity>,
    /// The tick writes are stamped with.
    pub(crate) change_tick: Tick,
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
            applying: false,
            despawning: HashSet::new(),
            bundles: Bundles::default(),
            tick_count: 1,
            change_tick: Tick::from_count(1),
            last_clamp: 1,
        }
    }

    /// Creates an entity with the components of `bundle`. Its hooks run, then the commands they
    /// queued. A hook may despawn the new entity, in which case the returned handle is already
    /// dead.
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
        bundle.write(table, row, &self.components, self.change_tick);
        table.debug_check_rows();
        self.after_spawn(entity, info);
        self.apply_commands();
        entity
    }

    /// Removes an entity and drops its components. Returns `false` if it isn't alive, or its
    /// despawn has already started.
    ///
    /// Every component's `on_replace` hook runs, then every `on_remove`, while the entity still
    /// has all its components. The commands those hooks queued run next, **before** the entity
    /// is removed: a relation that despawns its dependents removes them first, depth-first
    /// (children before parents). Meanwhile the entity's components can't change
    /// ([`EcsError::Despawning`]).
    pub fn despawn(&mut self, entity: Entity) -> bool {
        let Some(location) = self.entities.location(entity) else {
            return false;
        };
        if self.is_despawning(entity) {
            return false;
        }
        let archetype = self.archetypes.get(location.archetype);
        if !archetype.hooked() {
            self.remove_entity(entity);
            self.apply_commands();
            return true;
        }
        let ids = archetype.components().to_vec();
        self.despawning.insert(entity);
        self.run_hooks(HookKind::Replace, entity, &ids, HookCause::Despawn);
        self.run_hooks(HookKind::Remove, entity, &ids, HookCause::Despawn);
        // Finish after everything the hooks queued, and everything that queues in turn.
        self.queue_mut().record(
            Source::Despawn,
            Box::new(move |world: &mut World, _: &Source| {
                world.despawning.remove(&entity);
                world.remove_entity(entity);
            }),
        );
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
    /// - [`EcsError::NoSuchEntity`] if `entity` isn't alive.
    /// - [`EcsError::Despawning`] if its despawn has started.
    ///
    /// # Panics
    ///
    /// If `bundle` contains a component type twice.
    pub fn insert<B: Bundle>(&mut self, entity: Entity, bundle: B) -> Result<(), EcsError> {
        let location = self
            .entities
            .location(entity)
            .ok_or(EcsError::NoSuchEntity(entity))?;
        if self.is_despawning(entity) {
            return Err(EcsError::Despawning(entity));
        }
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
            self.run_hooks(HookKind::Replace, entity, &replaced, HookCause::Insert);
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
        bundle.write(table, row, &self.components, self.change_tick);
        table.debug_check_rows();
        if self.bundles.list[info].hooked {
            let all = self.bundles.list[info].ids.to_vec();
            self.run_hooks(HookKind::Add, entity, &added, HookCause::Insert);
            self.run_hooks(HookKind::Insert, entity, &all, HookCause::Insert);
        }
        self.apply_commands();
        Ok(())
    }

    /// Edits `entity`'s component `T` in place, running its hooks: `on_replace` sees the value
    /// before `edit`, and `on_insert` the value after it (cause [`HookCause::Modify`]). Returns
    /// what `edit` returns, or `None` (without calling it) if `entity` isn't alive, doesn't have a
    /// `T`, or is being despawned.
    ///
    /// The commands the hooks queue run before this returns. When it's called from a command
    /// (inside `Commands::run`), they run right after that command instead, as for every
    /// structural change made there.
    ///
    /// Because every change still passes through the hooks, this works on **immutable**
    /// components too ([`Component::MUTABLE`]). It's the in-place counterpart of replacing one
    /// with `insert`, without moving or copying it. For a mutable component without hooks it's
    /// like [`World::get_mut`], except that it refuses an entity being despawned.
    ///
    /// `edit` gets the value as [`Mut`]: writing through it marks the value changed
    /// ([`crate::Changed`], ADR-0014 §6).
    ///
    /// ```
    /// use oxreal_ecs::{Component, World};
    ///
    /// struct Score(Vec<u32>);
    /// impl Component for Score {
    ///     const MUTABLE: bool = false;
    /// }
    ///
    /// let mut world = World::new();
    /// let e = world.spawn(Score(vec![1]));
    /// let len = world.modify::<Score, _>(e, |mut score| {
    ///     score.0.push(2);
    ///     score.0.len()
    /// });
    /// assert_eq!(len, Some(2));
    /// ```
    ///
    /// # Panics
    ///
    /// A panic in `edit` propagates. `on_insert` doesn't run, the value keeps whatever `edit` did
    /// to it, and the commands `on_replace` queued are discarded, so they never run out of
    /// context later.
    pub fn modify<T: Component, R>(
        &mut self,
        entity: Entity,
        edit: impl FnOnce(Mut<'_, T>) -> R,
    ) -> Option<R> {
        let location = self.entities.location(entity)?;
        let id = self.components.id::<T>()?;
        if !self.archetypes.get(location.archetype).contains(id) || self.is_despawning(entity) {
            return None;
        }
        let hooked = self.components.hooks(id).any();
        if hooked {
            self.run_hooks(HookKind::Replace, entity, &[id], HookCause::Modify);
        }
        // A panic in `edit` must not leave `on_replace`'s commands to run later, out of context.
        let guard = HookGuard(self);
        let now = guard.0.change_tick;
        // Hooks can't change structure, so the entity is still where it was. Reached through the
        // column directly: the in-place-write guard doesn't apply, because the hooks run.
        let Some((value, changed)) = guard
            .0
            .archetypes
            .get_mut(location.archetype)
            .column_mut::<T>(id)
            .and_then(|(column, ticks)| {
                Some((
                    column.get_mut(location.row)?,
                    ticks.changed().get(location.row)?,
                ))
            })
        else {
            unreachable!(
                "{entity:?} lost its {} while its on_replace hook ran: a bug in oxreal-ecs",
                std::any::type_name::<T>()
            );
        };
        let result = edit(Mut::new(value, changed, now));
        drop(guard);
        if hooked {
            self.run_hooks(HookKind::Insert, entity, &[id], HookCause::Modify);
        }
        self.apply_commands();
        Some(result)
    }

    /// Takes component `T` off `entity` and returns it. `None` if `entity` isn't alive, doesn't
    /// have a `T`, or is being despawned.
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        let location = self.entities.location(entity)?;
        let id = self.components.id::<T>()?;
        if !self.archetypes.get(location.archetype).contains(id) || self.is_despawning(entity) {
            return None;
        }
        if self.components.hooks(id).any() {
            self.run_hooks(HookKind::Replace, entity, &[id], HookCause::Remove);
            self.run_hooks(HookKind::Remove, entity, &[id], HookCause::Remove);
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
            self.change_tick,
        )
    }

    /// Whether `entity` is alive and has a component `T`. Constant time: use it for role tests,
    /// the way Unreal code uses a cast (ADR-0020 §2).
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
            ChangeWindow::everything(self.change_tick),
        )
    }

    /// Like [`World::query`], visiting only entities that pass filter `F` ([`crate::With`],
    /// [`crate::Without`], [`crate::Added`], [`crate::Changed`]). A query on the world has no
    /// previous run to compare with, so `Added` and `Changed` let every value through: track
    /// changes in a [`crate::System`].
    ///
    /// # Panics
    ///
    /// If `D` accesses a component mutably and another way too (see [`QueryData`]).
    pub fn query_filtered<D: ReadOnlyQueryData, F: QueryFilter>(&self) -> QueryIter<'_, D, F> {
        QueryIter::shared(
            self.archetypes.iter(),
            query::prepare::<D, F>(&self.components),
            ChangeWindow::everything(self.change_tick),
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
        QueryIter::unique(
            self.archetypes.iter_mut(),
            state,
            ChangeWindow::everything(self.change_tick),
        )
    }

    /// Like [`World::query_mut`], visiting only entities that pass filter `F` (see
    /// [`World::query_filtered`]).
    ///
    /// # Panics
    ///
    /// If `D` accesses a component mutably and another way too (see [`QueryData`]).
    pub fn query_filtered_mut<D: QueryData, F: QueryFilter>(&mut self) -> QueryIter<'_, D, F> {
        let state = query::prepare::<D, F>(&self.components);
        QueryIter::unique(
            self.archetypes.iter_mut(),
            state,
            ChangeWindow::everything(self.change_tick),
        )
    }

    /// Stores a resource, returning the one it replaces.
    pub fn insert_resource<R: Resource>(&mut self, value: R) -> Option<R> {
        self.resources.insert(value, self.change_tick)
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
        self.resources.get_mut(self.change_tick)
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
        bundle.write(table, row, &self.components, self.change_tick);
        table.debug_check_rows();
        self.after_spawn(entity, info);
        self.apply_commands();
        true
    }

    /// Applies recorded commands, depth-first, without recursion.
    ///
    /// Newly recorded commands (a system's at a sync point, or the hooks' of the change that
    /// just finished) go to the **front** of the pending list, in recording order. If a loop is
    /// already running further up the stack, it takes them from there next. Otherwise this call
    /// runs the loop until nothing is pending. So hook commands always run right after the change
    /// that caused them, and cascades of any depth use constant stack.
    pub(crate) fn apply_commands(&mut self) {
        let queue = self.queue_mut();
        if queue.is_empty() {
            return;
        }
        // Inside a running loop, leave the commands recorded: the loop promotes everything the
        // current command caused, in recording order, when that command returns. Promoting here
        // would put each nested change's commands in front of the previous one's.
        if self.applying {
            return;
        }
        if !self.queue_mut().promote_recorded() {
            return;
        }
        let guard = ApplyingGuard(self);
        guard.0.applying = true;
        loop {
            guard.0.entities.flush();
            let Some((source, command)) = guard.0.queue_mut().next_pending() else {
                break;
            };
            command(guard.0, &source);
            // Commands that didn't change structure (an exclusive closure, say) leave what they
            // recorded unpromoted.
            guard.0.queue_mut().promote_recorded();
        }
        guard.0.applying = false;
    }

    /// Discards queued commands without applying them, after a panic interrupted a stage.
    /// Returns whether there were any.
    pub(crate) fn discard_commands(&mut self) -> bool {
        let had = !self.queue_mut().is_empty() || !self.despawning.is_empty();
        self.queue_mut().clear();
        self.despawning.clear();
        self.applying = false;
        had
    }

    /// Starts a system run: returns the tick count its writes are stamped with, and moves the
    /// world to the next count, so every later write is newer than the run (ADR-0014 §6). Clamps
    /// stored ticks every [`CHECK_INTERVAL`] ticks, so wrapping comparisons stay right.
    pub(crate) fn begin_system_run(&mut self) -> u64 {
        let now = self.tick_count;
        self.tick_count += 1;
        self.change_tick = Tick::from_count(self.tick_count);
        if now - self.last_clamp >= u64::from(CHECK_INTERVAL) {
            let tick = Tick::from_count(now);
            self.archetypes.clamp_ticks(tick, MAX_AGE);
            self.resources.clamp_ticks(tick, MAX_AGE);
            self.last_clamp = now;
        }
        now
    }

    /// The tick count, for tests.
    #[cfg(test)]
    pub(crate) fn tick_count(&self) -> u64 {
        self.tick_count
    }

    /// Jumps the tick count forward, for tests of wraparound and clamping. The clamping pass
    /// stays where it was, so a jump past [`CHECK_INTERVAL`] makes the next system run clamp.
    #[cfg(test)]
    pub(crate) fn set_tick_count(&mut self, count: u64) {
        assert!(count >= self.tick_count, "ticks only move forward");
        self.tick_count = count;
        self.change_tick = Tick::from_count(count);
    }

    /// The parts a system or hook may touch while it runs.
    pub(crate) fn split(&mut self) -> WorldParts<'_> {
        WorldParts {
            archetypes: &mut self.archetypes,
            components: &self.components,
            entities: &self.entities,
            resources: &mut self.resources,
            queue: self.queue.get_mut().unwrap_or_else(PoisonError::into_inner),
            despawning: &self.despawning,
            change_tick: self.change_tick,
        }
    }

    /// The command queue. Never poisoned, because it's never locked.
    fn queue_mut(&mut self) -> &mut CommandQueue {
        self.queue.get_mut().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `entity`'s despawn has started but not finished: its hooks' commands are
    /// running. It still counts as alive ([`World::contains`]), and its components can be read but
    /// not changed. See [`crate::HookWorld::is_despawning`].
    #[must_use]
    pub fn is_despawning(&self, entity: Entity) -> bool {
        !self.despawning.is_empty() && self.despawning.contains(&entity)
    }

    /// Frees `entity` and drops its components, without hooks.
    fn remove_entity(&mut self, entity: Entity) {
        let Some(location) = self.entities.free(entity) else {
            unreachable!("{entity:?} died before its despawn finished: a bug in oxreal-ecs");
        };
        let table = self.archetypes.get_mut(location.archetype);
        if let Some(moved) = table.swap_remove(location.row) {
            self.entities.set_location(moved, location);
        }
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
    fn after_spawn(&mut self, entity: Entity, info: usize) {
        if self.bundles.list[info].hooked {
            let ids = self.bundles.list[info].ids.to_vec();
            self.run_hooks(HookKind::Add, entity, &ids, HookCause::Spawn);
            self.run_hooks(HookKind::Insert, entity, &ids, HookCause::Spawn);
        }
    }

    /// Runs hook `kind` of each component in `ids` that has one, in order.
    ///
    /// If a hook panics, the guard discards every queued command and despawn mark, so nothing is
    /// applied out of context later. The world's structure is consistent: hooks run before or
    /// after a change, never in the middle of one.
    fn run_hooks(&mut self, kind: HookKind, entity: Entity, ids: &[ComponentId], cause: HookCause) {
        let guard = HookGuard(self);
        for &id in ids {
            if let Some(hook) = guard.0.components.hooks(id).get(kind) {
                let source = Source::Hook {
                    component: guard.0.components.name(id),
                    hook: kind.name(),
                };
                hook(
                    &mut HookWorld::new(guard.0.split(), source),
                    HookContext { entity, cause },
                );
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

/// Discards queued commands and despawn marks if a hook, or a `modify` edit, panics.
struct HookGuard<'a>(&'a mut World);

impl Drop for HookGuard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() && self.0.discard_commands() {
            warn!(
                "a panic interrupted a hook or an edit; discarding the commands hooks had queued"
            );
        }
    }
}

/// Resets the command loop if a command panics, discarding what's pending so it's never applied
/// out of context.
struct ApplyingGuard<'a>(&'a mut World);

impl Drop for ApplyingGuard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() && self.0.discard_commands() {
            warn!("a panic interrupted the command loop; discarding the pending commands");
        }
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

/// `entity`'s component `T`, mutably, stamped `now` when written. Shared by [`World::get_mut`]
/// and the system and hook views.
pub(crate) fn get_component_mut<'a, T: Component>(
    entities: &Entities,
    components: &Components,
    archetypes: &'a mut Archetypes,
    entity: Entity,
    now: Tick,
) -> Option<Mut<'a, T>> {
    const { crate::component::assert_mutable::<T>() };
    let location = entities.location(entity)?;
    let id = components.id::<T>()?;
    let (column, ticks) = archetypes.get_mut(location.archetype).column_mut::<T>(id)?;
    Some(Mut::new(
        column.get_mut(location.row)?,
        ticks.changed().get(location.row)?,
        now,
    ))
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
