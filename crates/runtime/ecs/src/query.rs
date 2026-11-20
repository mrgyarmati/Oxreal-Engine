//! Typed queries (ADR-0014 §7): iterate every entity that has a set of components.

use std::any::{TypeId, type_name};
use std::fmt;
use std::marker::PhantomData;

use crate::change::{ChangeWindow, Mut, Tick};
use crate::component::{Component, ComponentId, Components};
use crate::entity::Entity;
use crate::storage::{Archetype, ColumnTicks, RowTicks, TableView};

use self::sealed::RowTest as _;

/// What a query fetches for each entity:
///
/// | Query data | Item | Matches |
/// |---|---|---|
/// | `&T` | `&T` | entities with `T` |
/// | `&mut T` | [`Mut<T>`](crate::Mut) | entities with `T` (needs [`crate::World::query_mut`]) |
/// | `Option<&T>`, `Option<&mut T>` | `Option<&T>`, `Option<Mut<T>>` | every entity |
/// | [`Entity`] | the entity | every entity |
/// | a tuple of up to 12 of these | a tuple | entities matching every element |
///
/// A query may not access a component mutably together with any other access to it, for
/// example `(&mut Position, &Position)`. That's a bug in the caller, and creating the query
/// panics, naming the component.
pub trait QueryData: sealed::Fetch {}

/// Query data that only reads, usable through a shared `&World` ([`crate::World::query`]).
pub trait ReadOnlyQueryData: QueryData {}

/// Which entities a query visits, beyond what its data requires: [`With<T>`], [`Without<T>`],
/// [`Added<T>`], [`Changed<T>`], `()` (no filter), or a tuple of up to 12 filters (all must
/// hold).
pub trait QueryFilter: sealed::Filter {}

/// A [`QueryFilter`] on which components an entity has: [`With<T>`], [`Without<T>`], `()`, or a
/// tuple of these. Unlike [`Added`] and [`Changed`], it doesn't depend on a previous run, so
/// queries on the [`crate::World`] itself accept it.
pub trait ComponentFilter: QueryFilter {}

pub(crate) mod sealed {
    use super::{Access, Archetype, ChangeWindow, Components, TableView, TypeId};

    /// The engine's side of [`super::QueryData`]. Not implementable outside `oxreal-ecs`.
    pub trait Fetch {
        /// What [`Fetch::prepare`] resolves once per query: component IDs.
        type State: Copy;
        /// What the query yields per entity.
        type Item<'w>;
        /// Iterates one archetype's items.
        type Iter<'w>: Iterator<Item = Self::Item<'w>>;
        /// Records which components this reads and writes.
        fn declare(access: &mut Access);
        /// Resolves component IDs.
        fn prepare(components: &Components) -> Self::State;
        /// Whether an archetype's entities have what this needs.
        fn matches(state: &Self::State, archetype: &Archetype) -> bool;
        /// Iterates the items of a matching archetype: exactly one per row, in row order (row
        /// filters rely on it). Writes are stamped at `window.now`.
        fn iter<'w>(
            state: &Self::State,
            view: &mut TableView<'w>,
            window: ChangeWindow,
        ) -> Self::Iter<'w>;
    }

    /// The engine's side of [`super::QueryFilter`]. Not implementable outside `oxreal-ecs`.
    pub trait Filter {
        /// Component IDs, resolved once per query.
        type State: Copy;
        /// The row test for one archetype.
        type Rows<'w>: RowTest;
        /// Whether this filter tests rows ([`super::Added`], [`super::Changed`]), not just
        /// archetypes. When it doesn't, rows aren't tested at all.
        const BY_ROW: bool;
        /// Resolves component IDs.
        fn prepare(components: &Components) -> Self::State;
        /// Whether an archetype's entities may pass.
        fn matches(state: &Self::State, archetype: &Archetype) -> bool;
        /// The row test for a matching archetype.
        fn rows<'w>(
            state: &Self::State,
            view: &TableView<'w>,
            window: ChangeWindow,
        ) -> Self::Rows<'w>;
        /// Appends the components whose ticks this reads: a system must declare reading them.
        fn reads(reads: &mut Vec<(TypeId, &'static str)>);
    }

    /// Whether each row of one archetype passes a filter.
    pub trait RowTest {
        /// Whether row `row` passes.
        fn keep(&self, row: usize) -> bool;
    }
}

/// How a query uses a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccessKind {
    Read,
    Write,
}

/// The components a query reads and writes, checked for conflicting borrows.
///
/// Keyed by Rust type for now. The scheduler and dynamic queries will need it keyed by
/// component ID, with filter and resource access too (ADR-0014 implementation notes).
#[derive(Debug, Default)]
pub struct Access {
    entries: Vec<(TypeId, &'static str, AccessKind)>,
}

impl Access {
    fn add<T: 'static>(&mut self, kind: AccessKind) {
        let id = TypeId::of::<T>();
        if let Some(&(_, name, other)) = self.entries.iter().find(|entry| entry.0 == id)
            && (kind == AccessKind::Write || other == AccessKind::Write)
        {
            panic!(
                "a query accesses {name} mutably and also another way; that would alias. \
                 Read it once, or split the work into two queries"
            );
        }
        self.entries.push((id, type_name::<T>(), kind));
    }
}

/// Checks a query's accesses and resolves its IDs.
pub(crate) fn prepare<D: QueryData, F: QueryFilter>(
    components: &Components,
) -> (D::State, F::State) {
    D::declare(&mut Access::default());
    (D::prepare(components), F::prepare(components))
}

/// The components query data `D` reads and writes: `(type, type name, kind)`.
///
/// # Panics
///
/// If `D` aliases (see [`QueryData`]).
pub(crate) fn accesses<D: QueryData>() -> Vec<(TypeId, &'static str, AccessKind)> {
    let mut access = Access::default();
    D::declare(&mut access);
    access.entries
}

/// The components whose ticks filter `F` reads: `(type, type name)`.
pub(crate) fn filter_reads<F: QueryFilter>() -> Vec<(TypeId, &'static str)> {
    let mut reads = Vec::new();
    F::reads(&mut reads);
    reads
}

impl<T: Component> sealed::Fetch for &T {
    type State = Option<ComponentId>;
    type Item<'w> = &'w T;
    type Iter<'w> = std::slice::Iter<'w, T>;

    fn declare(access: &mut Access) {
        access.add::<T>(AccessKind::Read);
    }

    fn prepare(components: &Components) -> Self::State {
        components.id::<T>()
    }

    fn matches(state: &Self::State, archetype: &Archetype) -> bool {
        state.is_some_and(|id| archetype.contains(id))
    }

    fn iter<'w>(state: &Self::State, view: &mut TableView<'w>, _: ChangeWindow) -> Self::Iter<'w> {
        state
            .and_then(|id| view.take_slice::<T>(id))
            .unwrap_or_default()
            .iter()
    }
}

impl<T: Component> QueryData for &T {}
impl<T: Component> ReadOnlyQueryData for &T {}

/// One column for writing, with its changed ticks: the parts of a [`MutIter`].
fn take_for_writing<'w, T: Component>(
    state: Option<ComponentId>,
    view: &mut TableView<'w>,
    now: Tick,
) -> Option<MutIter<'w, T>> {
    let id = state?;
    let values = view.take_slice_mut::<T>(id)?;
    let Some(ticks) = view.ticks(id) else {
        unreachable!(
            "{} has values but no ticks: a bug in oxreal-ecs",
            type_name::<T>()
        );
    };
    Some(MutIter {
        rows: values.iter_mut().zip(ticks.rows()),
        now,
    })
}

impl<T: Component> sealed::Fetch for &mut T {
    type State = Option<ComponentId>;
    type Item<'w> = Mut<'w, T>;
    type Iter<'w> = MutIter<'w, T>;

    fn declare(access: &mut Access) {
        const { crate::component::assert_mutable::<T>() };
        access.add::<T>(AccessKind::Write);
    }

    fn prepare(components: &Components) -> Self::State {
        components.id::<T>()
    }

    fn matches(state: &Self::State, archetype: &Archetype) -> bool {
        state.is_some_and(|id| archetype.contains(id))
    }

    fn iter<'w>(
        state: &Self::State,
        view: &mut TableView<'w>,
        window: ChangeWindow,
    ) -> Self::Iter<'w> {
        take_for_writing(*state, view, window.now).unwrap_or_else(|| MutIter {
            rows: [].iter_mut().zip(&[]),
            now: window.now,
        })
    }
}

impl<T: Component> QueryData for &mut T {}

/// Iterates a column, wrapping each value in [`Mut`] with its changed tick. A named type rather
/// than a `Map` over a closure, so the wrapping inlines.
#[derive(Debug)]
pub struct MutIter<'w, T> {
    /// Values and their changed ticks, stepped together (`Zip` of two slices steps both with one
    /// index).
    rows: std::iter::Zip<std::slice::IterMut<'w, T>, std::slice::Iter<'w, RowTicks>>,
    now: Tick,
}

impl<'w, T> Iterator for MutIter<'w, T> {
    type Item = Mut<'w, T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let (value, ticks) = self.rows.next()?;
        Some(Mut::new(value, &ticks.changed, self.now))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.rows.size_hint()
    }
}

/// Yields `Some` items from an iterator, or `None` a fixed number of times when the archetype
/// lacks the component.
#[derive(Debug)]
pub struct Maybe<I> {
    inner: Option<I>,
    missing: usize,
}

impl<I: Iterator> Iterator for Maybe<I> {
    type Item = Option<I::Item>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            Some(inner) => inner.next().map(Some),
            None if self.missing > 0 => {
                self.missing -= 1;
                Some(None)
            }
            None => None,
        }
    }
}

impl<T: Component> sealed::Fetch for Option<&T> {
    type State = Option<ComponentId>;
    type Item<'w> = Option<&'w T>;
    type Iter<'w> = Maybe<std::slice::Iter<'w, T>>;

    fn declare(access: &mut Access) {
        access.add::<T>(AccessKind::Read);
    }

    fn prepare(components: &Components) -> Self::State {
        components.id::<T>()
    }

    fn matches(_: &Self::State, _: &Archetype) -> bool {
        true
    }

    fn iter<'w>(state: &Self::State, view: &mut TableView<'w>, _: ChangeWindow) -> Self::Iter<'w> {
        let rows = view.len();
        Maybe {
            inner: state
                .and_then(|id| view.take_slice::<T>(id))
                .map(<[T]>::iter),
            missing: rows,
        }
    }
}

impl<T: Component> QueryData for Option<&T> {}
impl<T: Component> ReadOnlyQueryData for Option<&T> {}

impl<T: Component> sealed::Fetch for Option<&mut T> {
    type State = Option<ComponentId>;
    type Item<'w> = Option<Mut<'w, T>>;
    type Iter<'w> = Maybe<MutIter<'w, T>>;

    fn declare(access: &mut Access) {
        const { crate::component::assert_mutable::<T>() };
        access.add::<T>(AccessKind::Write);
    }

    fn prepare(components: &Components) -> Self::State {
        components.id::<T>()
    }

    fn matches(_: &Self::State, _: &Archetype) -> bool {
        true
    }

    fn iter<'w>(
        state: &Self::State,
        view: &mut TableView<'w>,
        window: ChangeWindow,
    ) -> Self::Iter<'w> {
        let rows = view.len();
        Maybe {
            inner: take_for_writing(*state, view, window.now),
            missing: rows,
        }
    }
}

impl<T: Component> QueryData for Option<&mut T> {}

impl sealed::Fetch for Entity {
    type State = ();
    type Item<'w> = Entity;
    type Iter<'w> = std::iter::Copied<std::slice::Iter<'w, Entity>>;

    fn declare(_: &mut Access) {}

    fn prepare(_: &Components) -> Self::State {}

    fn matches((): &Self::State, _: &Archetype) -> bool {
        true
    }

    fn iter<'w>((): &Self::State, view: &mut TableView<'w>, _: ChangeWindow) -> Self::Iter<'w> {
        view.entities().iter().copied()
    }
}

impl QueryData for Entity {}
impl ReadOnlyQueryData for Entity {}

/// Steps several per-archetype iterators together, for tuple queries. Every element iterates
/// the same rows, so they end together.
#[derive(Debug)]
pub struct Zip<T>(T);

macro_rules! tuple_queries {
    ($($name:ident),+) => {
        #[allow(non_snake_case, reason = "the bindings reuse the type parameter names")]
        impl<$($name: Iterator),+> Iterator for Zip<($($name,)+)> {
            type Item = ($($name::Item,)+);

            #[inline]
            fn next(&mut self) -> Option<Self::Item> {
                let ($($name,)+) = &mut self.0;
                Some(($($name.next()?,)+))
            }
        }

        #[allow(non_snake_case, reason = "the bindings reuse the type parameter names")]
        impl<$($name: QueryData),+> sealed::Fetch for ($($name,)+) {
            type State = ($($name::State,)+);
            type Item<'w> = ($($name::Item<'w>,)+);
            type Iter<'w> = Zip<($($name::Iter<'w>,)+)>;

            fn declare(access: &mut Access) {
                $($name::declare(access);)+
            }

            fn prepare(components: &Components) -> Self::State {
                ($($name::prepare(components),)+)
            }

            fn matches(state: &Self::State, archetype: &Archetype) -> bool {
                let ($($name,)+) = state;
                $($name::matches($name, archetype))&&+
            }

            fn iter<'w>(
                state: &Self::State,
                view: &mut TableView<'w>,
                window: ChangeWindow,
            ) -> Self::Iter<'w> {
                let ($($name,)+) = state;
                Zip(($($name::iter($name, view, window),)+))
            }
        }

        impl<$($name: QueryData),+> QueryData for ($($name,)+) {}
        impl<$($name: ReadOnlyQueryData),+> ReadOnlyQueryData for ($($name,)+) {}

        #[allow(non_snake_case, reason = "the bindings reuse the type parameter names")]
        impl<$($name: sealed::RowTest),+> sealed::RowTest for ($($name,)+) {
            #[inline]
            fn keep(&self, row: usize) -> bool {
                let ($($name,)+) = self;
                $($name.keep(row))&&+
            }
        }

        #[allow(non_snake_case, reason = "the bindings reuse the type parameter names")]
        impl<$($name: QueryFilter),+> sealed::Filter for ($($name,)+) {
            type State = ($($name::State,)+);
            type Rows<'w> = ($($name::Rows<'w>,)+);
            const BY_ROW: bool = $($name::BY_ROW)||+;

            fn prepare(components: &Components) -> Self::State {
                ($($name::prepare(components),)+)
            }

            fn matches(state: &Self::State, archetype: &Archetype) -> bool {
                let ($($name,)+) = state;
                $($name::matches($name, archetype))&&+
            }

            fn rows<'w>(
                state: &Self::State,
                view: &TableView<'w>,
                window: ChangeWindow,
            ) -> Self::Rows<'w> {
                let ($($name,)+) = state;
                ($($name::rows($name, view, window),)+)
            }

            fn reads(reads: &mut Vec<(TypeId, &'static str)>) {
                $($name::reads(reads);)+
            }
        }

        impl<$($name: QueryFilter),+> QueryFilter for ($($name,)+) {}
        impl<$($name: ComponentFilter),+> ComponentFilter for ($($name,)+) {}
    };
}

tuple_queries!(A);
tuple_queries!(A, B);
tuple_queries!(A, B, C);
tuple_queries!(A, B, C, D);
tuple_queries!(A, B, C, D, E);
tuple_queries!(A, B, C, D, E, F);
tuple_queries!(A, B, C, D, E, F, G);
tuple_queries!(A, B, C, D, E, F, G, H);
tuple_queries!(A, B, C, D, E, F, G, H, I);
tuple_queries!(A, B, C, D, E, F, G, H, I, J);
tuple_queries!(A, B, C, D, E, F, G, H, I, J, K);
tuple_queries!(A, B, C, D, E, F, G, H, I, J, K, L);

/// The row test of filters that only look at archetypes: every row passes.
#[derive(Debug, Clone, Copy)]
pub struct AllRows;

impl sealed::RowTest for AllRows {
    #[inline]
    fn keep(&self, _: usize) -> bool {
        true
    }
}

impl sealed::Filter for () {
    type State = ();
    type Rows<'w> = AllRows;
    const BY_ROW: bool = false;

    fn prepare(_: &Components) -> Self::State {}

    fn matches((): &Self::State, _: &Archetype) -> bool {
        true
    }

    fn rows<'w>((): &Self::State, _: &TableView<'w>, _: ChangeWindow) -> Self::Rows<'w> {
        AllRows
    }

    fn reads(_: &mut Vec<(TypeId, &'static str)>) {}
}

impl QueryFilter for () {}
impl ComponentFilter for () {}

/// A filter: only entities that have component `T`. `T` isn't fetched.
pub struct With<T>(PhantomData<fn() -> T>);

/// A filter: only entities that don't have component `T`.
pub struct Without<T>(PhantomData<fn() -> T>);

/// A filter: only entities whose component `T` was **added** since the system last ran: spawned
/// with it, or given it by an insert. Replacing a `T` it already had isn't adding.
///
/// It compares with the system's previous run, so each addition is seen once per system (see
/// [`Changed`] for exactly what counts). On a system's first run, every `T` counts as added. A
/// system using it must declare reading or writing `T`. Only systems have a previous run, so
/// queries on the [`crate::World`] itself don't accept it.
pub struct Added<T>(PhantomData<fn() -> T>);

/// A filter: only entities whose component `T` was **added or changed** since the system last
/// ran. A value changes when it's written through [`crate::Mut`] or replaced by an insert;
/// fetching it mutably without writing isn't a change.
///
/// It compares with the system's previous run. The system sees every change made since: by other
/// systems, hooks, direct [`crate::World`] writes, and commands, its own included (they apply at
/// the sync point after it). It doesn't see what it wrote itself, through `ctx.world` or
/// `ctx.resources`, in that previous run. On a system's first run, every `T` counts as changed;
/// a run that panicked doesn't count, so its changes are seen again. A system using it must
/// declare reading or writing `T`. Only systems have a previous run, so queries on the
/// [`crate::World`] itself don't accept it.
///
/// ```
/// use oxreal_ecs::{Changed, Component, Resource, Schedule, Stage, System, World};
///
/// struct Health(u32);
/// impl Component for Health {}
/// /// How many health values changed since the system last ran.
/// struct HealthChanges(usize);
/// impl Resource for HealthChanges {}
///
/// let mut world = World::new();
/// let hero = world.spawn(Health(100));
/// world.spawn(Health(80));
/// world.insert_resource(HealthChanges(0));
///
/// let mut schedule = Schedule::new();
/// schedule.add(
///     Stage::PostUpdate,
///     System::new("count changes", |ctx| {
///         let changed = ctx.world.query_filtered::<&Health, Changed<Health>>().count();
///         if let Some(mut count) = ctx.resources.get_mut::<HealthChanges>() {
///             count.0 = changed;
///         }
///     })
///     .reads::<Health>()
///     .writes_resource::<HealthChanges>(),
/// );
///
/// let changes = |world: &World| world.resource::<HealthChanges>().map(|c| c.0);
/// schedule.run(&mut world).unwrap();
/// assert_eq!(changes(&world), Some(2), "on the first run, everything counts");
/// if let Some(mut health) = world.get_mut::<Health>(hero) {
///     health.0 -= 10;
/// }
/// schedule.run(&mut world).unwrap();
/// assert_eq!(changes(&world), Some(1), "only the hero's");
/// schedule.run(&mut world).unwrap();
/// assert_eq!(changes(&world), Some(0), "nothing since");
/// ```
pub struct Changed<T>(PhantomData<fn() -> T>);

macro_rules! filter_debug {
    ($($name:ident),+) => {$(
        impl<T> fmt::Debug for $name<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}<{}>", stringify!($name), type_name::<T>())
            }
        }
    )+};
}

filter_debug!(With, Without, Added, Changed);

impl<T: Component> sealed::Filter for With<T> {
    type State = Option<ComponentId>;
    type Rows<'w> = AllRows;
    const BY_ROW: bool = false;

    fn prepare(components: &Components) -> Self::State {
        components.id::<T>()
    }

    fn matches(state: &Self::State, archetype: &Archetype) -> bool {
        state.is_some_and(|id| archetype.contains(id))
    }

    fn rows<'w>(_: &Self::State, _: &TableView<'w>, _: ChangeWindow) -> Self::Rows<'w> {
        AllRows
    }

    fn reads(_: &mut Vec<(TypeId, &'static str)>) {}
}

impl<T: Component> QueryFilter for With<T> {}
impl<T: Component> ComponentFilter for With<T> {}

impl<T: Component> sealed::Filter for Without<T> {
    type State = Option<ComponentId>;
    type Rows<'w> = AllRows;
    const BY_ROW: bool = false;

    fn prepare(components: &Components) -> Self::State {
        components.id::<T>()
    }

    fn matches(state: &Self::State, archetype: &Archetype) -> bool {
        state.is_none_or(|id| !archetype.contains(id))
    }

    fn rows<'w>(_: &Self::State, _: &TableView<'w>, _: ChangeWindow) -> Self::Rows<'w> {
        AllRows
    }

    fn reads(_: &mut Vec<(TypeId, &'static str)>) {}
}

impl<T: Component> QueryFilter for Without<T> {}
impl<T: Component> ComponentFilter for Without<T> {}

/// The row test of [`Added`]: each row's added tick against the window.
#[derive(Debug)]
pub struct AddedRows<'w> {
    ticks: &'w [RowTicks],
    window: ChangeWindow,
}

impl sealed::RowTest for AddedRows<'_> {
    #[inline]
    fn keep(&self, row: usize) -> bool {
        self.ticks
            .get(row)
            .is_some_and(|ticks| self.window.contains(ticks.added))
    }
}

/// The row test of [`Changed`]: each row's changed tick against the window.
#[derive(Debug)]
pub struct ChangedRows<'w> {
    ticks: &'w [RowTicks],
    window: ChangeWindow,
}

impl sealed::RowTest for ChangedRows<'_> {
    #[inline]
    fn keep(&self, row: usize) -> bool {
        self.ticks
            .get(row)
            .is_some_and(|ticks| self.window.contains(ticks.changed.get()))
    }
}

impl<T: Component> sealed::Filter for Added<T> {
    type State = Option<ComponentId>;
    type Rows<'w> = AddedRows<'w>;
    const BY_ROW: bool = true;

    fn prepare(components: &Components) -> Self::State {
        components.id::<T>()
    }

    fn matches(state: &Self::State, archetype: &Archetype) -> bool {
        state.is_some_and(|id| archetype.contains(id))
    }

    fn rows<'w>(state: &Self::State, view: &TableView<'w>, window: ChangeWindow) -> Self::Rows<'w> {
        AddedRows {
            ticks: state
                .and_then(|id| view.ticks(id))
                .map_or(&[], ColumnTicks::rows),
            window,
        }
    }

    fn reads(reads: &mut Vec<(TypeId, &'static str)>) {
        reads.push((TypeId::of::<T>(), type_name::<T>()));
    }
}

impl<T: Component> QueryFilter for Added<T> {}

impl<T: Component> sealed::Filter for Changed<T> {
    type State = Option<ComponentId>;
    type Rows<'w> = ChangedRows<'w>;
    const BY_ROW: bool = true;

    fn prepare(components: &Components) -> Self::State {
        components.id::<T>()
    }

    fn matches(state: &Self::State, archetype: &Archetype) -> bool {
        state.is_some_and(|id| archetype.contains(id))
    }

    fn rows<'w>(state: &Self::State, view: &TableView<'w>, window: ChangeWindow) -> Self::Rows<'w> {
        ChangedRows {
            ticks: state
                .and_then(|id| view.ticks(id))
                .map_or(&[], ColumnTicks::rows),
            window,
        }
    }

    fn reads(reads: &mut Vec<(TypeId, &'static str)>) {
        reads.push((TypeId::of::<T>(), type_name::<T>()));
    }
}

impl<T: Component> QueryFilter for Changed<T> {}

/// The archetypes a query walks: shared for read-only queries, unique for the others.
enum Tables<'w> {
    Shared(std::slice::Iter<'w, Archetype>),
    Unique(std::slice::IterMut<'w, Archetype>),
}

/// Iterates a query's matches, archetype by archetype. Created by [`crate::World::query`] and
/// its variants.
pub struct QueryIter<'w, D: QueryData, F: QueryFilter = ()> {
    tables: Tables<'w>,
    data: D::State,
    filter: F::State,
    window: ChangeWindow,
    current: Option<D::Iter<'w>>,
    /// The current archetype's row test, the row the next item comes from, and how many rows it
    /// has.
    rows: Option<F::Rows<'w>>,
    row: usize,
    table_len: usize,
}

impl<'w, D: QueryData, F: QueryFilter> QueryIter<'w, D, F> {
    pub(crate) fn shared(
        archetypes: std::slice::Iter<'w, Archetype>,
        (data, filter): (D::State, F::State),
        window: ChangeWindow,
    ) -> Self {
        Self::new(Tables::Shared(archetypes), data, filter, window)
    }

    pub(crate) fn unique(
        archetypes: std::slice::IterMut<'w, Archetype>,
        (data, filter): (D::State, F::State),
        window: ChangeWindow,
    ) -> Self {
        Self::new(Tables::Unique(archetypes), data, filter, window)
    }

    fn new(tables: Tables<'w>, data: D::State, filter: F::State, window: ChangeWindow) -> Self {
        Self {
            tables,
            data,
            filter,
            window,
            current: None,
            rows: None,
            row: 0,
            table_len: 0,
        }
    }

    fn next_table(&mut self) -> Option<TableView<'w>> {
        let wanted = |archetype: &Archetype| {
            archetype.len() > 0
                && D::matches(&self.data, archetype)
                && F::matches(&self.filter, archetype)
        };
        match &mut self.tables {
            Tables::Shared(tables) => tables.find(|a| wanted(a)).map(TableView::shared),
            Tables::Unique(tables) => tables.find(|a| wanted(a)).map(TableView::unique),
        }
    }
}

impl<'w, D: QueryData, F: QueryFilter> Iterator for QueryIter<'w, D, F> {
    type Item = D::Item<'w>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(current) = self.current.as_mut() {
                if !F::BY_ROW {
                    // Archetype-only filters: every row of a matching archetype passes.
                    if let Some(item) = current.next() {
                        return Some(item);
                    }
                } else {
                    // An item filtered out is dropped unused: a `Mut` that isn't written through
                    // marks nothing.
                    for item in current.by_ref() {
                        let row = self.row;
                        self.row += 1;
                        if self.rows.as_ref().is_some_and(|rows| rows.keep(row)) {
                            return Some(item);
                        }
                    }
                    // Rows are matched to items by counting, so every query data must yield
                    // exactly one item per row (`sealed::Fetch::iter`).
                    debug_assert_eq!(
                        self.row, self.table_len,
                        "query data yielded a different number of items than its table has rows: \
                         a bug in oxreal-ecs"
                    );
                }
            }
            let mut view = self.next_table()?;
            self.rows = Some(F::rows(&self.filter, &view, self.window));
            self.row = 0;
            self.table_len = view.len();
            self.current = Some(D::iter(&self.data, &mut view, self.window));
        }
    }
}

impl<D: QueryData, F: QueryFilter> fmt::Debug for QueryIter<'_, D, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryIter")
            .field("data", &type_name::<D>())
            .field("filter", &type_name::<F>())
            .finish_non_exhaustive()
    }
}
