//! Components and their registry.

use std::any::{TypeId, type_name};
use std::fmt;

use crate::entity::Entity;
use crate::hook::HookWorld;
use crate::storage::{AnyColumn, new_column};
use crate::type_map::TypeIdMap;

/// Data attached to an entity.
///
/// Implement it for plain data types: `impl Component for Velocity {}`. A component has no
/// behaviour of its own. Systems and queries act on it (ADR-0008 §1).
///
/// A component's `Drop` must not panic. A panic while the world is dropping or moving
/// components leaves it in an inconsistent state (never memory-unsafe), and later operations
/// on it may panic.
///
/// A component can declare lifecycle [hooks](ComponentHooks) through [`Component::HOOKS`], to keep
/// engine invariants such as relation inverses (ADR-0014 §4).
pub trait Component: Send + Sync + 'static {
    /// Functions the world calls when a component of this type is added, inserted, replaced or
    /// removed. None by default.
    const HOOKS: ComponentHooks = ComponentHooks::NONE;
}

/// A lifecycle hook: called with the change that triggered it (ADR-0014 §4).
///
/// A hook may read and write component data and resources, and queue [`crate::Commands`]. It
/// can't change structure directly. The commands it queues run right after the change that
/// triggered it, before anything else.
pub type Hook = for<'a, 'w> fn(&'a mut HookWorld<'w>, HookContext);

/// What triggered a hook. Plain data, so more can be added without breaking hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct HookContext {
    /// The entity whose component changed.
    pub entity: Entity,
    /// Why the component changed.
    pub cause: HookCause,
}

/// Why a component was added, replaced or removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HookCause {
    /// The entity was spawned with it.
    Spawn,
    /// It was inserted into a live entity, as a new component or a replacement.
    Insert,
    /// It was removed from an entity that stays alive.
    Remove,
    /// The entity is being despawned. Lets a relation tell "my owner is going away" from "the
    /// relation was removed".
    Despawn,
}

/// The lifecycle hooks of a component type. Build it from [`ComponentHooks::NONE`]:
///
/// ```
/// use oxreal_ecs::{Component, ComponentHooks, HookContext, HookWorld};
///
/// struct Light;
/// impl Component for Light {
///     const HOOKS: ComponentHooks = ComponentHooks::NONE.on_add(light_added);
/// }
///
/// fn light_added(world: &mut HookWorld<'_>, context: HookContext) {
///     // For example, register `context.entity` with a lighting system's resource.
///     let _ = (world, context);
/// }
/// ```
///
/// When they run, for one entity and component:
///
/// | Change | Hooks, in order |
/// |---|---|
/// | added (spawn or insert) | `on_add`, `on_insert`, with the value in place |
/// | replaced (insert over an existing value) | `on_replace` with the old value, then `on_insert` with the new one |
/// | removed (remove or despawn) | `on_replace`, `on_remove`, while the entity still has every component |
///
/// On despawn, every component's `on_replace` runs before any `on_remove`, and the commands they
/// queue (despawning children, for example) finish **before** the entity is removed. The order
/// of hooks across the components of one entity is unspecified.
#[derive(Clone, Copy)]
pub struct ComponentHooks {
    on_add: Option<Hook>,
    on_insert: Option<Hook>,
    on_replace: Option<Hook>,
    on_remove: Option<Hook>,
}

impl fmt::Debug for ComponentHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentHooks")
            .field("on_add", &self.on_add.is_some())
            .field("on_insert", &self.on_insert.is_some())
            .field("on_replace", &self.on_replace.is_some())
            .field("on_remove", &self.on_remove.is_some())
            .finish()
    }
}

impl ComponentHooks {
    /// No hooks.
    pub const NONE: Self = Self {
        on_add: None,
        on_insert: None,
        on_replace: None,
        on_remove: None,
    };

    /// Runs `hook` when the component is added to an entity that didn't have one.
    #[must_use]
    pub const fn on_add(mut self, hook: Hook) -> Self {
        self.on_add = Some(hook);
        self
    }

    /// Runs `hook` whenever a value is inserted: added, or replacing an old one.
    #[must_use]
    pub const fn on_insert(mut self, hook: Hook) -> Self {
        self.on_insert = Some(hook);
        self
    }

    /// Runs `hook` before a value is replaced or removed, while it's still there.
    #[must_use]
    pub const fn on_replace(mut self, hook: Hook) -> Self {
        self.on_replace = Some(hook);
        self
    }

    /// Runs `hook` before the component is removed from an entity, or the entity despawned.
    #[must_use]
    pub const fn on_remove(mut self, hook: Hook) -> Self {
        self.on_remove = Some(hook);
        self
    }

    pub(crate) const fn any(&self) -> bool {
        self.on_add.is_some()
            || self.on_insert.is_some()
            || self.on_replace.is_some()
            || self.on_remove.is_some()
    }

    pub(crate) const fn get(&self, kind: HookKind) -> Option<Hook> {
        match kind {
            HookKind::Add => self.on_add,
            HookKind::Insert => self.on_insert,
            HookKind::Replace => self.on_replace,
            HookKind::Remove => self.on_remove,
        }
    }
}

/// Which lifecycle hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookKind {
    Add,
    Insert,
    Replace,
    Remove,
}

impl HookKind {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            HookKind::Add => "on_add",
            HookKind::Insert => "on_insert",
            HookKind::Replace => "on_replace",
            HookKind::Remove => "on_remove",
        }
    }
}

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
    hooks: ComponentHooks,
}

/// The component types a world has seen.
#[derive(Debug, Default)]
pub struct Components {
    ids: TypeIdMap<ComponentId>,
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
            hooks: T::HOOKS,
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

    /// A component type's lifecycle hooks.
    pub(crate) fn hooks(&self, id: ComponentId) -> ComponentHooks {
        self.infos[id.index()].hooks
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
