//! Components and their registry.

use std::any::{TypeId, type_name};
use std::collections::HashMap;

use crate::storage::{AnyColumn, new_column};

/// Data attached to an entity.
///
/// Implement it for plain data types: `impl Component for Velocity {}`. A component has no
/// behaviour of its own. Systems and queries act on it (ADR-0008 §1).
///
/// A component's `Drop` must not panic. A panic while the world is dropping or moving
/// components leaves it in an inconsistent state (never memory-unsafe), and later operations
/// on it may panic.
pub trait Component: Send + Sync + 'static {}

/// A component type's index in one world's registry. Dense, assigned on first use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentId(u32);

impl ComponentId {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug)]
struct ComponentInfo {
    name: &'static str,
    new_column: fn() -> Box<dyn AnyColumn>,
}

/// The component types a world has seen.
#[derive(Debug, Default)]
pub struct Components {
    ids: HashMap<TypeId, ComponentId>,
    infos: Vec<ComponentInfo>,
}

impl Components {
    /// The ID of `T`, registering it on first use.
    pub(crate) fn register<T: Component>(&mut self) -> ComponentId {
        if let Some(&id) = self.ids.get(&TypeId::of::<T>()) {
            return id;
        }
        let Ok(index) = u32::try_from(self.infos.len()) else {
            panic!("component type limit reached: a world registers at most 2^32 types");
        };
        let id = ComponentId(index);
        self.infos.push(ComponentInfo {
            name: type_name::<T>(),
            new_column: new_column::<T>,
        });
        self.ids.insert(TypeId::of::<T>(), id);
        id
    }

    /// The ID of `T`, or `None` if no entity in this world has ever had one.
    pub(crate) fn id<T: 'static>(&self) -> Option<ComponentId> {
        self.ids.get(&TypeId::of::<T>()).copied()
    }

    /// The Rust type name of a component, for messages.
    pub(crate) fn name(&self, id: ComponentId) -> &'static str {
        self.infos[id.index()].name
    }

    /// An empty column for a component type.
    pub(crate) fn new_column(&self, id: ComponentId) -> Box<dyn AnyColumn> {
        (self.infos[id.index()].new_column)()
    }

    /// How many component types are registered.
    pub(crate) fn len(&self) -> usize {
        self.infos.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A;
    impl Component for A {}
    struct B;
    impl Component for B {}

    #[test]
    fn ids_are_dense_and_stable() {
        let mut components = Components::default();
        assert_eq!(components.id::<A>(), None);
        let a = components.register::<A>();
        let b = components.register::<B>();
        assert_eq!(components.register::<A>(), a);
        assert_eq!((a.index(), b.index()), (0, 1));
        assert_eq!(components.id::<B>(), Some(b));
        assert!(components.name(a).ends_with("::A"));
        assert_eq!(components.len(), 2);
    }
}
