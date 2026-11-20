//! Typed queries (ADR-0014 §7): iterate every entity that has a set of components.

use std::any::{TypeId, type_name};
use std::fmt;
use std::marker::PhantomData;

use crate::change::Mut;
use crate::component::{Component, ComponentId, Components};
use crate::entity::Entity;
use crate::storage::{Archetype, TableView};

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

/// Which entities a query visits, beyond what its data requires:
/// [`With<T>`], [`Without<T>`], `()` (no filter), or a tuple of up to 12 filters (all must hold).
pub trait QueryFilter: sealed::Filter {}

pub(crate) mod sealed {
    use super::{Access, Archetype, Components, TableView};

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
        /// Iterates the items of a matching archetype.
        fn iter<'w>(state: &Self::State, view: &mut TableView<'w>) -> Self::Iter<'w>;
    }

    /// The engine's side of [`super::QueryFilter`]. Not implementable outside `oxreal-ecs`.
    pub trait Filter {
        /// Component IDs, resolved once per query.
        type State: Copy;
        /// Resolves component IDs.
        fn prepare(components: &Components) -> Self::State;
        /// Whether an archetype's entities pass.
        fn matches(state: &Self::State, archetype: &Archetype) -> bool;
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

    fn iter<'w>(state: &Self::State, view: &mut TableView<'w>) -> Self::Iter<'w> {
        state
            .and_then(|id| view.take_slice::<T>(id))
            .unwrap_or_default()
            .iter()
    }
}

impl<T: Component> QueryData for &T {}
impl<T: Component> ReadOnlyQueryData for &T {}

impl<T: Component> sealed::Fetch for &mut T {
    type State = Option<ComponentId>;
    type Item<'w> = Mut<'w, T>;
    type Iter<'w> = MutIter<'w, T>;

    fn declare(access: &mut Access) {
        access.add::<T>(AccessKind::Write);
    }

    fn prepare(components: &Components) -> Self::State {
        components.id::<T>()
    }

    fn matches(state: &Self::State, archetype: &Archetype) -> bool {
        state.is_some_and(|id| archetype.contains(id))
    }

    fn iter<'w>(state: &Self::State, view: &mut TableView<'w>) -> Self::Iter<'w> {
        MutIter(
            state
                .and_then(|id| view.take_slice_mut::<T>(id))
                .unwrap_or_default()
                .iter_mut(),
        )
    }
}

impl<T: Component> QueryData for &mut T {}

/// Iterates a column, wrapping each value in [`Mut`]. A named type rather than a `Map` over a
/// function pointer, so the wrapping inlines.
#[derive(Debug)]
pub struct MutIter<'w, T>(std::slice::IterMut<'w, T>);

impl<'w, T> Iterator for MutIter<'w, T> {
    type Item = Mut<'w, T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(Mut::new)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
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

    fn iter<'w>(state: &Self::State, view: &mut TableView<'w>) -> Self::Iter<'w> {
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
        access.add::<T>(AccessKind::Write);
    }

    fn prepare(components: &Components) -> Self::State {
        components.id::<T>()
    }

    fn matches(_: &Self::State, _: &Archetype) -> bool {
        true
    }

    fn iter<'w>(state: &Self::State, view: &mut TableView<'w>) -> Self::Iter<'w> {
        let rows = view.len();
        Maybe {
            inner: state
                .and_then(|id| view.take_slice_mut::<T>(id))
                .map(|slice| MutIter(slice.iter_mut())),
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

    fn iter<'w>((): &Self::State, view: &mut TableView<'w>) -> Self::Iter<'w> {
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

            fn iter<'w>(state: &Self::State, view: &mut TableView<'w>) -> Self::Iter<'w> {
                let ($($name,)+) = state;
                Zip(($($name::iter($name, view),)+))
            }
        }

        impl<$($name: QueryData),+> QueryData for ($($name,)+) {}
        impl<$($name: ReadOnlyQueryData),+> ReadOnlyQueryData for ($($name,)+) {}

        #[allow(non_snake_case, reason = "the bindings reuse the type parameter names")]
        impl<$($name: QueryFilter),+> sealed::Filter for ($($name,)+) {
            type State = ($($name::State,)+);

            fn prepare(components: &Components) -> Self::State {
                ($($name::prepare(components),)+)
            }

            fn matches(state: &Self::State, archetype: &Archetype) -> bool {
                let ($($name,)+) = state;
                $($name::matches($name, archetype))&&+
            }
        }

        impl<$($name: QueryFilter),+> QueryFilter for ($($name,)+) {}
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

impl sealed::Filter for () {
    type State = ();

    fn prepare(_: &Components) -> Self::State {}

    fn matches((): &Self::State, _: &Archetype) -> bool {
        true
    }
}

impl QueryFilter for () {}

/// A filter: only entities that have component `T`. `T` isn't fetched.
pub struct With<T>(PhantomData<fn() -> T>);

/// A filter: only entities that don't have component `T`.
pub struct Without<T>(PhantomData<fn() -> T>);

impl<T> fmt::Debug for With<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "With<{}>", type_name::<T>())
    }
}

impl<T> fmt::Debug for Without<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Without<{}>", type_name::<T>())
    }
}

impl<T: Component> sealed::Filter for With<T> {
    type State = Option<ComponentId>;

    fn prepare(components: &Components) -> Self::State {
        components.id::<T>()
    }

    fn matches(state: &Self::State, archetype: &Archetype) -> bool {
        state.is_some_and(|id| archetype.contains(id))
    }
}

impl<T: Component> QueryFilter for With<T> {}

impl<T: Component> sealed::Filter for Without<T> {
    type State = Option<ComponentId>;

    fn prepare(components: &Components) -> Self::State {
        components.id::<T>()
    }

    fn matches(state: &Self::State, archetype: &Archetype) -> bool {
        state.is_none_or(|id| !archetype.contains(id))
    }
}

impl<T: Component> QueryFilter for Without<T> {}

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
    current: Option<D::Iter<'w>>,
}

impl<'w, D: QueryData, F: QueryFilter> QueryIter<'w, D, F> {
    pub(crate) fn shared(
        archetypes: std::slice::Iter<'w, Archetype>,
        (data, filter): (D::State, F::State),
    ) -> Self {
        Self {
            tables: Tables::Shared(archetypes),
            data,
            filter,
            current: None,
        }
    }

    pub(crate) fn unique(
        archetypes: std::slice::IterMut<'w, Archetype>,
        (data, filter): (D::State, F::State),
    ) -> Self {
        Self {
            tables: Tables::Unique(archetypes),
            data,
            filter,
            current: None,
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

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.current.as_mut().and_then(Iterator::next) {
                return Some(item);
            }
            let mut view = self.next_table()?;
            self.current = Some(D::iter(&self.data, &mut view));
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
