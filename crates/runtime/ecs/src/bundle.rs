//! Bundles: sets of components inserted together.

use std::any::type_name;

use crate::component::{Component, ComponentId, Components};
use crate::storage::Archetype;

/// One or more components, inserted together: a single component, or a tuple of up to 12
/// components (`(Position, Velocity)`), nested tuples included. `()` is the empty bundle.
///
/// A bundle may not contain the same component type twice.
pub trait Bundle: sealed::BundleImpl + Send + Sync + 'static {}

pub(crate) mod sealed {
    use super::{Archetype, ComponentId, Components};

    /// The engine's side of [`super::Bundle`]. Not implementable outside `oxreal-ecs`.
    pub trait BundleImpl {
        /// Registers the bundle's component types and appends their IDs, in bundle order.
        fn register(components: &mut Components, ids: &mut Vec<ComponentId>);
        /// Writes the components into row `row` of `archetype`, which has all of them.
        fn write(self, archetype: &mut Archetype, row: usize, components: &Components);
    }
}

impl<T: Component> sealed::BundleImpl for T {
    fn register(components: &mut Components, ids: &mut Vec<ComponentId>) {
        ids.push(components.register::<T>());
    }

    fn write(self, archetype: &mut Archetype, row: usize, components: &Components) {
        let column = components
            .id::<T>()
            .and_then(|id| archetype.column_mut::<T>(id));
        let Some(column) = column else {
            unreachable!(
                "an archetype lacks {} from the bundle written to it: a bug in oxreal-ecs",
                type_name::<T>()
            );
        };
        if let Some(slot) = column.get_mut(row) {
            // The entity already had this component: replace it, dropping the old value.
            *slot = self;
        } else {
            debug_assert_eq!(column.len(), row, "bundle rows must be written in order");
            column.push(self);
        }
    }
}

impl<T: Component> Bundle for T {}

macro_rules! tuple_bundles {
    ($($name:ident),*) => {
        impl<$($name: Bundle),*> sealed::BundleImpl for ($($name,)*) {
            #[allow(unused_variables, reason = "the empty tuple uses neither argument")]
            fn register(components: &mut Components, ids: &mut Vec<ComponentId>) {
                $($name::register(components, ids);)*
            }

            #[allow(unused_variables, reason = "the empty tuple uses no argument")]
            #[allow(non_snake_case, reason = "the bindings reuse the type parameter names")]
            fn write(self, archetype: &mut Archetype, row: usize, components: &Components) {
                let ($($name,)*) = self;
                $($name.write(archetype, row, components);)*
            }
        }

        impl<$($name: Bundle),*> Bundle for ($($name,)*) {}
    };
}

tuple_bundles!();
tuple_bundles!(A);
tuple_bundles!(A, B);
tuple_bundles!(A, B, C);
tuple_bundles!(A, B, C, D);
tuple_bundles!(A, B, C, D, E);
tuple_bundles!(A, B, C, D, E, F);
tuple_bundles!(A, B, C, D, E, F, G);
tuple_bundles!(A, B, C, D, E, F, G, H);
tuple_bundles!(A, B, C, D, E, F, G, H, I);
tuple_bundles!(A, B, C, D, E, F, G, H, I, J);
tuple_bundles!(A, B, C, D, E, F, G, H, I, J, K);
tuple_bundles!(A, B, C, D, E, F, G, H, I, J, K, L);

/// The bundle's component IDs, sorted.
///
/// # Panics
///
/// If the bundle contains a component type twice. That's a bug in the caller.
pub(crate) fn component_ids<B: Bundle>(components: &mut Components) -> Vec<ComponentId> {
    let mut ids = Vec::new();
    B::register(components, &mut ids);
    ids.sort_unstable();
    if let Some(pair) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
        panic!(
            "bundle {} contains the component {} twice",
            type_name::<B>(),
            components.name(pair[0])
        );
    }
    ids
}
