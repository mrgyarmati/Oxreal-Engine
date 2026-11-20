//! Parent/child relations: `ChildOf` and its maintained inverse, `Children` (ADR-0014 §3).

use std::fmt;

use oxreal_diagnostics::warn;
use oxreal_ecs::{
    Component, ComponentHooks, EcsError, Entity, HookCause, HookContext, HookWorld, World,
};

/// The most ancestors a cycle check walks before it gives up and treats the chain as broken.
/// Far deeper than any real hierarchy.
const MAX_DEPTH: usize = 1 << 16;

/// Makes an entity the child of `parent`. Insert it to attach, replace it to move, remove it to
/// detach. The parent's [`Children`] is kept in step automatically.
///
/// The parent can only be read: changing it means inserting a new `ChildOf`, which keeps
/// `Children` consistent. An entity can't become its own ancestor: such an insert is refused,
/// and so is a parent that isn't alive. The entity is then left without a parent, and a warning
/// says why. [`set_parent`] reports these cases as errors instead.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChildOf(Entity);

impl ChildOf {
    /// A link to `parent`.
    #[must_use]
    pub const fn new(parent: Entity) -> Self {
        Self(parent)
    }

    /// The parent.
    #[must_use]
    pub const fn parent(&self) -> Entity {
        self.0
    }
}

impl fmt::Debug for ChildOf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChildOf({:?})", self.0)
    }
}

impl Component for ChildOf {
    const HOOKS: ComponentHooks = ComponentHooks::NONE
        .on_insert(child_of_inserted)
        .on_replace(child_of_replaced);
}

/// An entity's children, in order. Maintained by [`ChildOf`]: it can be read, never built or
/// edited directly. It exists only while there is at least one child.
///
/// Despawning an entity despawns its children first, recursively (ADR-0014 §4). Removing
/// `Children` itself detaches the children instead, leaving them as roots.
#[derive(Debug, PartialEq, Eq)]
pub struct Children(Vec<Entity>);

impl Children {
    /// The children, in order.
    #[must_use]
    pub fn as_slice(&self) -> &[Entity] {
        &self.0
    }

    /// Iterates the children in order.
    pub fn iter(&self) -> std::slice::Iter<'_, Entity> {
        self.0.iter()
    }

    /// How many children there are. At least one.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Always `false` while the component exists. Provided for completeness.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'a> IntoIterator for &'a Children {
    type Item = &'a Entity;
    type IntoIter = std::slice::Iter<'a, Entity>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl Component for Children {
    const HOOKS: ComponentHooks = ComponentHooks::NONE.on_remove(children_removed);
}

/// Why a parent can't be set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HierarchyError {
    /// The child or the parent isn't alive, or is being despawned.
    Ecs(EcsError),
    /// The parent is the child itself, or one of its descendants: the link would form a cycle.
    Cycle {
        /// The entity that was to be moved.
        child: Entity,
        /// The parent that is already below it.
        parent: Entity,
    },
}

impl fmt::Display for HierarchyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HierarchyError::Ecs(error) => error.fmt(f),
            HierarchyError::Cycle { child, parent } => write!(
                f,
                "{parent:?} is {child:?} itself or one of its descendants, so it can't be its \
                 parent: that would form a cycle"
            ),
        }
    }
}

impl std::error::Error for HierarchyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HierarchyError::Ecs(error) => Some(error),
            HierarchyError::Cycle { .. } => None,
        }
    }
}

/// Makes `child` a child of `parent`, appended after its existing children, moving it from its
/// previous parent if it had one.
///
/// # Errors
///
/// - [`HierarchyError::Ecs`] if either entity isn't alive.
/// - [`HierarchyError::Cycle`] if `parent` is `child` or one of its descendants.
///
/// Nothing changes in either case.
pub fn set_parent(world: &mut World, child: Entity, parent: Entity) -> Result<(), HierarchyError> {
    if !world.contains(parent) {
        return Err(HierarchyError::Ecs(EcsError::NoSuchEntity(parent)));
    }
    if is_ancestor_or_self(
        |e| world.get::<ChildOf>(e).map(ChildOf::parent),
        child,
        parent,
    ) {
        return Err(HierarchyError::Cycle { child, parent });
    }
    world
        .insert(child, ChildOf::new(parent))
        .map_err(HierarchyError::Ecs)
}

/// Detaches `child` from its parent, making it a root. Returns the parent it had.
pub fn remove_parent(world: &mut World, child: Entity) -> Option<Entity> {
    world.remove::<ChildOf>(child).map(|link| link.parent())
}

/// Whether `ancestor` is `entity` itself or above it, walking up through `parent_of`.
fn is_ancestor_or_self(
    parent_of: impl Fn(Entity) -> Option<Entity>,
    ancestor: Entity,
    entity: Entity,
) -> bool {
    let mut current = entity;
    for _ in 0..MAX_DEPTH {
        if current == ancestor {
            return true;
        }
        match parent_of(current) {
            Some(parent) => current = parent,
            None => return false,
        }
    }
    // A chain this long is broken (or a cycle slipped in). Refusing is the safe answer.
    true
}

fn child_of_inserted(world: &mut HookWorld<'_>, context: HookContext) {
    let child = context.entity;
    let Some(parent) = world.get::<ChildOf>(child).map(ChildOf::parent) else {
        return;
    };
    let refused = if !world.contains(parent) {
        Some("the parent isn't alive")
    } else if is_ancestor_or_self(
        |e| world.get::<ChildOf>(e).map(ChildOf::parent),
        child,
        parent,
    ) {
        Some("the parent is the entity itself or one of its descendants")
    } else {
        None
    };
    if let Some(reason) = refused {
        warn!(
            ?child,
            ?parent,
            reason,
            "ChildOf refused; the entity stays without a parent"
        );
        world.commands().remove::<ChildOf>(child);
        return;
    }
    if let Some(mut children) = world.get_mut::<Children>(parent) {
        if !children.0.contains(&child) {
            children.0.push(child);
        }
    } else {
        // Inserting `Children` changes structure, so it waits for a command. Checked again then:
        // another child may have created it, or the link may have changed.
        world
            .commands()
            .run(move |world| add_child(world, parent, child));
    }
}

fn child_of_replaced(world: &mut HookWorld<'_>, context: HookContext) {
    let child = context.entity;
    let Some(parent) = world.get::<ChildOf>(child).map(ChildOf::parent) else {
        return;
    };
    let now_empty = match world.get_mut::<Children>(parent) {
        Some(mut children) => {
            children.0.retain(|&c| c != child);
            children.0.is_empty()
        }
        None => false,
    };
    if now_empty {
        world.commands().run(move |world| {
            if world
                .get::<Children>(parent)
                .is_some_and(Children::is_empty)
            {
                world.remove::<Children>(parent);
            }
        });
    }
}

fn children_removed(world: &mut HookWorld<'_>, context: HookContext) {
    let parent = context.entity;
    let children = world
        .get::<Children>(parent)
        .map(|c| c.0.clone())
        .unwrap_or_default();
    if context.cause == HookCause::Despawn {
        // The despawn finishes after these, so every child sees its parent whole (ADR-0014 §4).
        for child in children {
            world.commands().despawn(child);
        }
    } else {
        // `Children` was removed on its own: detach the children rather than despawn them.
        for child in children {
            world.commands().run(move |world| {
                if world.get::<ChildOf>(child).map(ChildOf::parent) == Some(parent) {
                    world.remove::<ChildOf>(child);
                }
            });
        }
    }
}

/// Adds `child` to `parent`'s `Children`, creating it, if `child` is still linked to `parent`.
fn add_child(world: &mut World, parent: Entity, child: Entity) {
    if world.get::<ChildOf>(child).map(ChildOf::parent) != Some(parent) {
        return;
    }
    if let Some(mut children) = world.get_mut::<Children>(parent) {
        if !children.0.contains(&child) {
            children.0.push(child);
        }
        return;
    }
    if let Err(error) = world.insert(parent, Children(vec![child])) {
        // The parent went away (or started despawning) before the link settled.
        warn!(?child, ?parent, %error, "ChildOf dropped; the entity stays without a parent");
        world.remove::<ChildOf>(child);
    }
}
