//! Archetype tables (ADR-0014 §2). Private: nothing here appears in the public API (§1).
//!
//! Entities with the same set of component types share an **archetype**: a table with one
//! column per component type and one row per entity. Columns are plain `Vec<T>`s behind the
//! [`AnyColumn`] trait. The trait is only used for whole-table operations (moving or dropping a
//! row, creating an empty column), so iterating a column is as fast as iterating a `Vec<T>`, and
//! the storage needs no `unsafe`.

use std::any::{Any, TypeId};
use std::collections::HashMap;

use crate::change::{ChangedTick, Tick};
use crate::component::{Component, ComponentId, Components};
use crate::entity::Entity;
use crate::type_map::TypeIdMap;

/// A type-erased column of one component type.
pub(crate) trait AnyColumn: Send + Sync {
    /// The column as `Any`, to recover its concrete type.
    fn as_any(&self) -> &dyn Any;
    /// The column as `Any`, mutably.
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Number of rows.
    fn len(&self) -> usize;
    /// Removes and drops a row, moving the last row into its place.
    fn swap_remove_drop(&mut self, row: usize);
    /// Removes a row, moving the last row into its place, and appends the value to `to`, which
    /// must be a column of the same type.
    fn swap_remove_into(&mut self, row: usize, to: &mut dyn AnyColumn);
}

/// The concrete column behind [`AnyColumn`].
struct Column<T>(Vec<T>);

impl<T: Component> AnyColumn for Column<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn swap_remove_drop(&mut self, row: usize) {
        drop(self.0.swap_remove(row));
    }

    fn swap_remove_into(&mut self, row: usize, to: &mut dyn AnyColumn) {
        let value = self.0.swap_remove(row);
        typed_mut::<T>(to).push(value);
    }
}

/// Creates an empty column for `T`. Stored as a function pointer in the component registry.
pub(crate) fn new_column<T: Component>() -> Box<dyn AnyColumn> {
    Box::new(Column::<T>(Vec::new()))
}

/// The `Vec<T>` inside a column.
pub(crate) fn typed<T: Component>(column: &dyn AnyColumn) -> &Vec<T> {
    match column.as_any().downcast_ref::<Column<T>>() {
        Some(column) => &column.0,
        None => column_type_mismatch::<T>(),
    }
}

/// The `Vec<T>` inside a column, mutably.
pub(crate) fn typed_mut<T: Component>(column: &mut dyn AnyColumn) -> &mut Vec<T> {
    match column.as_any_mut().downcast_mut::<Column<T>>() {
        Some(column) => &mut column.0,
        None => column_type_mismatch::<T>(),
    }
}

#[cold]
fn column_type_mismatch<T>() -> ! {
    unreachable!(
        "a column isn't a column of {}: a bug in oxreal-ecs",
        std::any::type_name::<T>()
    )
}

/// The added and changed ticks of one column's values, row for row (ADR-0014 §6). Kept beside
/// the type-erased column rather than in it, so a query can hold a column's values mutably and
/// its ticks shared at once.
#[derive(Debug, Default)]
pub(crate) struct ColumnTicks {
    rows: Vec<RowTicks>,
}

/// One value's ticks. Kept together, so adding, moving or removing a row is one operation.
#[derive(Debug)]
pub(crate) struct RowTicks {
    pub(crate) added: Tick,
    pub(crate) changed: ChangedTick,
}

impl ColumnTicks {
    fn len(&self) -> usize {
        self.rows.len()
    }

    /// Appends the ticks of a value added at `tick`.
    fn push(&mut self, tick: Tick) {
        self.rows.push(RowTicks {
            added: tick,
            changed: ChangedTick::new(tick),
        });
    }

    fn swap_remove(&mut self, row: usize) {
        self.rows.swap_remove(row);
    }

    /// Moves a row's ticks to the end of `to`, like [`AnyColumn::swap_remove_into`].
    fn swap_remove_into(&mut self, row: usize, to: &mut ColumnTicks) {
        to.rows.push(self.rows.swap_remove(row));
    }

    /// Each row's ticks.
    pub(crate) fn rows(&self) -> &[RowTicks] {
        &self.rows
    }

    /// Clamps every tick to at most `max_age` before `now`.
    fn clamp(&mut self, now: Tick, max_age: u32) {
        for row in &mut self.rows {
            row.added = row.added.clamped(now, max_age);
            row.changed.set(row.changed.get().clamped(now, max_age));
        }
    }
}

/// An archetype's index in [`Archetypes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ArchetypeId(u32);

impl ArchetypeId {
    /// The archetype of entities without components. Always present.
    pub(crate) const EMPTY: Self = Self(0);

    fn index(self) -> usize {
        self.0 as usize
    }
}

/// Entities that have exactly the same component types, stored in columns.
pub struct Archetype {
    /// Sorted, without duplicates. Parallel to `columns`.
    components: Box<[ComponentId]>,
    /// One bit per component ID, for constant-time membership tests (ADR-0014 §7).
    membership: Box<[u64]>,
    columns: Vec<Box<dyn AnyColumn>>,
    /// Each column's ticks. Parallel to `columns`.
    ticks: Vec<ColumnTicks>,
    entities: Vec<Entity>,
    /// Where inserting a bundle of a given type leads, filled in as it's used.
    insert_edges: TypeIdMap<ArchetypeId>,
    /// Whether any of its components has a lifecycle hook, so changes without hooks skip them.
    hooked: bool,
    /// Where removing a component leads, filled in as it's used.
    remove_edges: HashMap<ComponentId, ArchetypeId>,
}

impl std::fmt::Debug for Archetype {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Archetype")
            .field("components", &self.components)
            .field("entities", &self.entities.len())
            .finish_non_exhaustive()
    }
}

impl Archetype {
    fn new(components: Box<[ComponentId]>, registry: &Components) -> Self {
        let words = registry.len().div_ceil(64);
        let mut membership = vec![0_u64; words].into_boxed_slice();
        for id in &components {
            membership[id.index() / 64] |= 1 << (id.index() % 64);
        }
        let hooked = components.iter().any(|&id| registry.hooks(id).any());
        Self {
            columns: components
                .iter()
                .map(|&id| registry.new_column(id))
                .collect(),
            ticks: components.iter().map(|_| ColumnTicks::default()).collect(),
            components,
            membership,
            entities: Vec::new(),
            insert_edges: TypeIdMap::default(),
            hooked,
            remove_edges: HashMap::new(),
        }
    }

    /// The component types, sorted.
    pub(crate) fn components(&self) -> &[ComponentId] {
        &self.components
    }

    /// Whether any component type here has a lifecycle hook.
    pub(crate) fn hooked(&self) -> bool {
        self.hooked
    }

    /// Whether this archetype's entities have component `id`. Constant time.
    pub(crate) fn contains(&self, id: ComponentId) -> bool {
        self.membership
            .get(id.index() / 64)
            .is_some_and(|word| word & (1 << (id.index() % 64)) != 0)
    }

    pub(crate) fn len(&self) -> usize {
        self.entities.len()
    }

    fn column_index(&self, id: ComponentId) -> Option<usize> {
        self.components.binary_search(&id).ok()
    }

    /// The column of component `T`, if this archetype has it.
    pub(crate) fn column<T: Component>(&self, id: ComponentId) -> Option<&Vec<T>> {
        let index = self.column_index(id)?;
        Some(typed::<T>(&*self.columns[index]))
    }

    /// The column of component `T`, mutably, with its ticks.
    pub(crate) fn column_mut<T: Component>(
        &mut self,
        id: ComponentId,
    ) -> Option<(&mut Vec<T>, &ColumnTicks)> {
        let index = self.column_index(id)?;
        Some((
            typed_mut::<T>(&mut *self.columns[index]),
            &self.ticks[index],
        ))
    }

    /// Writes component `T`'s value in row `row` at `tick`: replacing the value already there
    /// (a change), or appending it as the next row (an addition).
    pub(crate) fn write<T: Component>(
        &mut self,
        id: ComponentId,
        row: usize,
        value: T,
        tick: Tick,
    ) {
        let Some(index) = self.column_index(id) else {
            unreachable!(
                "an archetype lacks {} from the bundle written to it: a bug in oxreal-ecs",
                std::any::type_name::<T>()
            );
        };
        let column = typed_mut::<T>(&mut *self.columns[index]);
        let ticks = &mut self.ticks[index];
        if let Some(slot) = column.get_mut(row) {
            // The entity already had this component: replace it, dropping the old value. Stamped
            // first, so a panic in the old value's `Drop` can't lose the change.
            ticks.rows[row].changed.set(tick);
            *slot = value;
        } else {
            debug_assert_eq!(column.len(), row, "bundle rows must be written in order");
            column.push(value);
            ticks.push(tick);
        }
    }

    /// Clamps every stored tick to at most `max_age` before `now`.
    pub(crate) fn clamp_ticks(&mut self, now: Tick, max_age: u32) {
        for ticks in &mut self.ticks {
            ticks.clamp(now, max_age);
        }
    }

    /// Appends a row for `entity`. Its components must be written right after, so every column
    /// has one value per row again.
    pub(crate) fn push_entity(&mut self, entity: Entity) -> usize {
        self.entities.push(entity);
        self.entities.len() - 1
    }

    /// Removes and drops row `row`. Returns the entity moved into the row from the end, if any.
    pub(crate) fn swap_remove(&mut self, row: usize) -> Option<Entity> {
        for column in &mut self.columns {
            column.swap_remove_drop(row);
        }
        for ticks in &mut self.ticks {
            ticks.swap_remove(row);
        }
        self.finish_swap_remove(row)
    }

    fn finish_swap_remove(&mut self, row: usize) -> Option<Entity> {
        self.entities.swap_remove(row);
        self.debug_check_rows();
        self.entities.get(row).copied()
    }

    /// Every column has one value per entity. Checked in debug builds after structural changes.
    pub(crate) fn debug_check_rows(&self) {
        debug_assert!(
            self.columns.iter().all(|c| c.len() == self.entities.len())
                && self.ticks.iter().all(|t| t.len() == self.entities.len()),
            "archetype {:?} has columns of unequal length: a bug in oxreal-ecs",
            self.components
        );
    }
}

/// Every archetype of a world, indexed by its component set.
#[derive(Debug)]
pub(crate) struct Archetypes {
    list: Vec<Archetype>,
    by_components: HashMap<Box<[ComponentId]>, ArchetypeId>,
}

impl Archetypes {
    pub(crate) fn new(components: &Components) -> Self {
        let empty: Box<[ComponentId]> = Box::new([]);
        Self {
            list: vec![Archetype::new(empty.clone(), components)],
            by_components: HashMap::from([(empty, ArchetypeId::EMPTY)]),
        }
    }

    pub(crate) fn get(&self, id: ArchetypeId) -> &Archetype {
        &self.list[id.index()]
    }

    pub(crate) fn get_mut(&mut self, id: ArchetypeId) -> &mut Archetype {
        &mut self.list[id.index()]
    }

    /// Both archetypes, mutably. They must differ.
    pub(crate) fn pair_mut(
        &mut self,
        a: ArchetypeId,
        b: ArchetypeId,
    ) -> (&mut Archetype, &mut Archetype) {
        match self.list.get_disjoint_mut([a.index(), b.index()]) {
            Ok([a, b]) => (a, b),
            Err(e) => unreachable!("archetypes {a:?} and {b:?}: {e}: a bug in oxreal-ecs"),
        }
    }

    pub(crate) fn iter(&self) -> std::slice::Iter<'_, Archetype> {
        self.list.iter()
    }

    pub(crate) fn iter_mut(&mut self) -> std::slice::IterMut<'_, Archetype> {
        self.list.iter_mut()
    }

    pub(crate) fn len(&self) -> usize {
        self.list.len()
    }

    /// Clamps every stored component tick to at most `max_age` before `now`.
    pub(crate) fn clamp_ticks(&mut self, now: Tick, max_age: u32) {
        for archetype in &mut self.list {
            archetype.clamp_ticks(now, max_age);
        }
    }

    /// The archetype with exactly these components (sorted, no duplicates), created if needed.
    pub(crate) fn get_or_create(
        &mut self,
        components: &[ComponentId],
        registry: &Components,
    ) -> ArchetypeId {
        if let Some(&id) = self.by_components.get(components) {
            return id;
        }
        let Ok(index) = u32::try_from(self.list.len()) else {
            panic!("archetype limit reached: a world holds at most 2^32 archetypes");
        };
        let id = ArchetypeId(index);
        let components: Box<[ComponentId]> = components.into();
        self.list.push(Archetype::new(components.clone(), registry));
        self.by_components.insert(components, id);
        id
    }

    /// The cached result of [`Archetypes::insert_target`], if this move happened before.
    pub(crate) fn cached_insert_target(
        &self,
        from: ArchetypeId,
        bundle: TypeId,
    ) -> Option<ArchetypeId> {
        self.get(from).insert_edges.get(&bundle).copied()
    }

    /// Where an entity in `from` goes when a bundle of type `bundle` with components
    /// `bundle_components` (sorted) is inserted.
    pub(crate) fn insert_target(
        &mut self,
        from: ArchetypeId,
        bundle: TypeId,
        bundle_components: &[ComponentId],
        registry: &Components,
    ) -> ArchetypeId {
        if let Some(&to) = self.get(from).insert_edges.get(&bundle) {
            return to;
        }
        let mut union: Vec<ComponentId> = self.get(from).components.to_vec();
        union.extend_from_slice(bundle_components);
        union.sort_unstable();
        union.dedup();
        let to = self.get_or_create(&union, registry);
        self.get_mut(from).insert_edges.insert(bundle, to);
        to
    }

    /// Where an entity in `from` goes when component `removed` is taken away.
    pub(crate) fn remove_target(
        &mut self,
        from: ArchetypeId,
        removed: ComponentId,
        registry: &Components,
    ) -> ArchetypeId {
        if let Some(&to) = self.get(from).remove_edges.get(&removed) {
            return to;
        }
        let rest: Vec<ComponentId> = self
            .get(from)
            .components
            .iter()
            .copied()
            .filter(|&id| id != removed)
            .collect();
        let to = self.get_or_create(&rest, registry);
        self.get_mut(from).remove_edges.insert(removed, to);
        to
    }
}

/// Moves row `row` of `from` to the end of `to`. Components `to` has are moved. Each component
/// `to` lacks is handed to `leftover` (column and row) to take or drop, and must be removed from
/// the column there. Returns the new row in `to`, and the entity moved into `row` in `from`.
pub(crate) fn move_row(
    from: &mut Archetype,
    row: usize,
    to: &mut Archetype,
    mut leftover: impl FnMut(ComponentId, &mut dyn AnyColumn, usize),
) -> (usize, Option<Entity>) {
    let entity = from.entities[row];
    let new_row = to.push_entity(entity);
    for (index, &id) in from.components.iter().enumerate() {
        let column = &mut *from.columns[index];
        let ticks = &mut from.ticks[index];
        match to.column_index(id) {
            // A moved value keeps its ticks: moving between archetypes isn't a change.
            Some(target) => {
                column.swap_remove_into(row, &mut *to.columns[target]);
                ticks.swap_remove_into(row, &mut to.ticks[target]);
            }
            None => {
                // Ticks first: `leftover` may drop the value, and its `Drop` may panic.
                ticks.swap_remove(row);
                leftover(id, column, row);
            }
        }
    }
    let moved = from.finish_swap_remove(row);
    (new_row, moved)
}

/// One column, borrowed for a query.
enum ColumnRef<'w> {
    Shared(&'w (dyn AnyColumn + 'static)),
    Unique(&'w mut (dyn AnyColumn + 'static)),
}

/// One archetype, borrowed for a query. Each column can be taken once, so a query can hold
/// several columns at once, mutably, without aliasing.
pub struct TableView<'w> {
    components: &'w [ComponentId],
    entities: &'w [Entity],
    columns: Vec<Option<ColumnRef<'w>>>,
    /// Every column's ticks, always shared: stamping goes through atomics.
    ticks: &'w [ColumnTicks],
}

impl std::fmt::Debug for TableView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TableView")
            .field("components", &self.components)
            .field("rows", &self.entities.len())
            .finish_non_exhaustive()
    }
}

impl<'w> TableView<'w> {
    pub(crate) fn shared(archetype: &'w Archetype) -> Self {
        Self {
            components: &archetype.components,
            entities: &archetype.entities,
            ticks: &archetype.ticks,
            columns: archetype
                .columns
                .iter()
                .map(|column| Some(ColumnRef::Shared(&**column)))
                .collect(),
        }
    }

    pub(crate) fn unique(archetype: &'w mut Archetype) -> Self {
        let Archetype {
            components,
            entities,
            columns,
            ticks,
            ..
        } = archetype;
        Self {
            components,
            entities,
            ticks,
            columns: columns
                .iter_mut()
                .map(|column| Some(ColumnRef::Unique(&mut **column)))
                .collect(),
        }
    }

    pub(crate) fn entities(&self) -> &'w [Entity] {
        self.entities
    }

    pub(crate) fn len(&self) -> usize {
        self.entities.len()
    }

    /// Component `id`'s ticks, row for row. `None` if the archetype doesn't have it. Ticks are
    /// always shared, so this works alongside any borrow of the values.
    pub(crate) fn ticks(&self, id: ComponentId) -> Option<&'w ColumnTicks> {
        let index = self.components.binary_search(&id).ok()?;
        Some(&self.ticks[index])
    }

    /// Borrows component `T`'s column for reading. `None` if the archetype doesn't have it.
    /// The column stays available to further reads (a query may read a component twice), but
    /// not to writes.
    pub(crate) fn take_slice<T: Component>(&mut self, id: ComponentId) -> Option<&'w [T]> {
        let index = self.components.binary_search(&id).ok()?;
        let column: &'w (dyn AnyColumn + 'static) = match self.take(id)? {
            ColumnRef::Shared(column) => column,
            ColumnRef::Unique(column) => column,
        };
        self.columns[index] = Some(ColumnRef::Shared(column));
        Some(typed::<T>(column))
    }

    /// Takes component `T`'s column for writing. `None` if the archetype doesn't have it.
    pub(crate) fn take_slice_mut<T: Component>(&mut self, id: ComponentId) -> Option<&'w mut [T]> {
        // Every query write goes through here, whatever path prepared the query.
        const { crate::component::assert_mutable::<T>() };
        match self.take(id)? {
            ColumnRef::Unique(column) => Some(typed_mut::<T>(column)),
            ColumnRef::Shared(_) => unreachable!(
                "a read-only query asked to write {}: a bug in oxreal-ecs",
                std::any::type_name::<T>()
            ),
        }
    }

    fn take(&mut self, id: ComponentId) -> Option<ColumnRef<'w>> {
        let index = self.components.binary_search(&id).ok()?;
        match self.columns[index].take() {
            Some(column) => Some(column),
            None => unreachable!(
                "a column was borrowed twice by one query; the access check should have \
                 rejected it: a bug in oxreal-ecs"
            ),
        }
    }
}
