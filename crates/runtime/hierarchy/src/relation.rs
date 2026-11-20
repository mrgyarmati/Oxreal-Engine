//! Parent/child relations: `ChildOf` and its maintained inverse, `Children` (ADR-0014 §3).

use std::fmt;

use oxreal_diagnostics::warn;
use oxreal_ecs::{
    Component, ComponentHooks, EcsError, Entity, HookCause, HookContext, HookWorld, World,
};

use crate::transform::{TransformRule, keep_world_on_attach, keep_world_on_detach};

/// The longest chain a walk up the hierarchy follows: an entity and up to this many ancestors.
/// Far deeper than any real hierarchy. Attaching under a deeper chain is refused
/// ([`HierarchyError::TooDeep`]). The limit applies above the parent only, so a subtree being
/// attached adds its own depth: walks through deeper chains stop at the limit, and
/// `KeepWorld` placements then ignore the transforms beyond it.
pub const MAX_DEPTH: usize = 1 << 16;

/// What happens to a child when its parent is despawned (ADR-0014, §3 amendment, item 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum OnParentDespawn {
    /// The child is detached and keeps its place in the world. The default, as in Unreal.
    #[default]
    Detach,
    /// The child is despawned too, before its parent: for the parts of one object.
    Despawn,
}

/// Makes an entity the child of `parent`. Insert it to attach, insert another to move, remove it
/// to detach. The parent's [`Children`] is kept in step automatically.
///
/// It's **immutable** (ADR-0014 §3 amendment): changing the parent means inserting a new
/// `ChildOf`, which runs the hooks that keep `Children` consistent.
///
/// A link is refused, leaving the entity without a parent and logging a warning, when:
/// - the parent isn't alive, or is being despawned;
/// - the parent is the entity itself or one of its descendants;
/// - the chain above the parent is deeper than [`MAX_DEPTH`].
///
/// [`set_parent`] reports these cases as errors instead, before changing anything.
///
/// Replacing a link with an identical one, by `insert` or `World::modify`, still moves the child to
/// the end of its siblings: the hooks remove it and add it back. [`set_parent`] with the current
/// parent changes nothing.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChildOf {
    parent: Entity,
    on_parent_despawn: OnParentDespawn,
}

impl ChildOf {
    /// A link to `parent`. When the parent is despawned, the child is detached and keeps its
    /// place in the world.
    #[must_use]
    pub const fn new(parent: Entity) -> Self {
        Self {
            parent,
            on_parent_despawn: OnParentDespawn::Detach,
        }
    }

    /// The same link, but the child is despawned with its parent (before it), as a part of the
    /// same object, such as a character's weapon.
    #[must_use]
    pub const fn despawn_with_parent(mut self) -> Self {
        self.on_parent_despawn = OnParentDespawn::Despawn;
        self
    }

    /// The parent.
    #[must_use]
    pub const fn parent(&self) -> Entity {
        self.parent
    }

    /// What happens to the child when the parent is despawned.
    #[must_use]
    pub const fn on_parent_despawn(&self) -> OnParentDespawn {
        self.on_parent_despawn
    }
}

impl fmt::Debug for ChildOf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ChildOf({:?}, {:?})",
            self.parent, self.on_parent_despawn
        )
    }
}

impl Component for ChildOf {
    const MUTABLE: bool = false;
    const HOOKS: ComponentHooks = ComponentHooks::NONE
        .on_insert(child_of_inserted)
        .on_replace(child_of_replaced);
}

/// An entity's children, in order. Maintained by [`ChildOf`]: it can be read, never built or
/// edited directly. It exists only while there is at least one child.
///
/// It's immutable, and records which entity it belongs to and which version of that entity's
/// list it is. A `Children` taken off one entity (with `remove`) and inserted anywhere is
/// recognized in constant time and rebuilt from the actual `ChildOf` links, with a warning.
/// Repair keeps the order of the children it lists, and puts any others after them, in handle
/// order: their real order is lost.
///
/// Removing `Children` detaches the children, keeping their place in the world. Despawning the
/// entity treats each child by its link's [`OnParentDespawn`].
#[derive(Debug, PartialEq, Eq)]
pub struct Children {
    owner: Entity,
    /// Matches the owner's `ChildrenStamp` while this is its current list.
    stamp: u64,
    list: Vec<Entity>,
}

impl Children {
    /// The children, in order.
    #[must_use]
    pub fn as_slice(&self) -> &[Entity] {
        &self.list
    }

    /// Iterates the children in order.
    pub fn iter(&self) -> std::slice::Iter<'_, Entity> {
        self.list.iter()
    }

    /// How many children there are. At least one.
    #[must_use]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Always `false` while the component exists. Provided for completeness.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

impl<'a> IntoIterator for &'a Children {
    type Item = &'a Entity;
    type IntoIter = std::slice::Iter<'a, Entity>;

    fn into_iter(self) -> Self::IntoIter {
        self.list.iter()
    }
}

impl Component for Children {
    const MUTABLE: bool = false;
    const HOOKS: ComponentHooks = ComponentHooks::NONE
        .on_insert(children_inserted)
        .on_remove(children_removed);
}

/// Why a parent can't be set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HierarchyError {
    /// The child or the parent isn't alive, or is being despawned.
    #[error(
        "can't make {parent:?} the parent of {child:?}. Check that both are alive and not being \
         despawned before attaching"
    )]
    Ecs {
        /// The entity to attach.
        child: Entity,
        /// The intended parent.
        parent: Entity,
        /// Which of them isn't usable, and why.
        #[source]
        source: EcsError,
    },
    /// The parent is the child itself, or one of its descendants: the link would form a cycle.
    #[error(
        "can't make {parent:?} the parent of {child:?}: it's {child:?} itself or one of its \
         descendants, which would form a cycle. Detach it first, or choose a parent outside \
         {child:?}'s subtree"
    )]
    Cycle {
        /// The entity to attach.
        child: Entity,
        /// The intended parent, already at or below it.
        parent: Entity,
    },
    /// `TransformRule::KeepWorld` was asked for, but a world placement involved has zero scale,
    /// so the child's place can't be expressed relative to the parent.
    #[error(
        "can't keep {child:?}'s place in the world under {parent:?}: one of their world \
         placements has zero scale. Use `TransformRule::KeepLocal`, or give it a non-zero scale"
    )]
    CantKeepWorld {
        /// The entity to attach.
        child: Entity,
        /// The intended parent.
        parent: Entity,
    },
    /// The chain above the parent is deeper than [`MAX_DEPTH`].
    #[error(
        "can't make {parent:?} the parent of {child:?}: the hierarchy above it is deeper than \
         {limit} levels. Flatten the hierarchy"
    )]
    TooDeep {
        /// The entity to attach.
        child: Entity,
        /// The intended parent.
        parent: Entity,
        /// The deepest chain allowed.
        limit: usize,
    },
}

/// Makes `child` a child of `parent`, appended after its existing children, moving it from its
/// previous parent if it had one. `rule` says whether it keeps its local transform (and so
/// moves with its new parent) or its place in the world. There's deliberately no default.
///
/// Setting the parent it already has changes nothing, not even its place among its siblings.
///
/// # Errors
///
/// Nothing changes on error:
/// - [`HierarchyError::Ecs`] if either entity isn't alive, or is being despawned.
/// - [`HierarchyError::Cycle`] if `parent` is `child` or one of its descendants.
/// - [`HierarchyError::TooDeep`] if the chain above `parent` is deeper than [`MAX_DEPTH`].
pub fn set_parent(
    world: &mut World,
    child: Entity,
    parent: Entity,
    rule: TransformRule,
) -> Result<(), HierarchyError> {
    let ecs = |source| HierarchyError::Ecs {
        child,
        parent,
        source,
    };
    for entity in [child, parent] {
        if !world.contains(entity) {
            return Err(ecs(EcsError::NoSuchEntity(entity)));
        }
        if world.is_despawning(entity) {
            return Err(ecs(EcsError::Despawning(entity)));
        }
    }
    match ancestry(
        |e| world.get::<ChildOf>(e).map(ChildOf::parent),
        child,
        parent,
    ) {
        Ancestry::Unrelated => {}
        Ancestry::Ancestor => return Err(HierarchyError::Cycle { child, parent }),
        Ancestry::TooDeep => {
            return Err(HierarchyError::TooDeep {
                child,
                parent,
                limit: MAX_DEPTH,
            });
        }
    }
    let current = world.get::<ChildOf>(child).copied();
    if current.map(|link| link.parent()) == Some(parent) {
        return Ok(());
    }
    if rule == TransformRule::KeepWorld && keep_world_on_attach(world, child, parent).is_err() {
        return Err(HierarchyError::CantKeepWorld { child, parent });
    }
    // Keep the link's despawn policy when moving it.
    let link = ChildOf {
        parent,
        on_parent_despawn: current.map_or(OnParentDespawn::Detach, |c| c.on_parent_despawn),
    };
    world.insert(child, link).map_err(ecs)
}

/// Detaches `child` from its parent, making it a root. Returns the parent it had, or `None` if it
/// had none or is being despawned (its links are resolved by the despawn). `rule` says
/// whether it keeps its local transform (and so moves to where that places it in the world) or
/// its place in the world.
pub fn remove_parent(world: &mut World, child: Entity, rule: TransformRule) -> Option<Entity> {
    if !world.has::<ChildOf>(child) || world.is_despawning(child) {
        return None;
    }
    if rule == TransformRule::KeepWorld {
        keep_world_on_detach(world, child);
    }
    world.remove::<ChildOf>(child).map(|link| link.parent())
}

/// How `ancestor` relates to `entity`.
enum Ancestry {
    /// `ancestor` is neither `entity` nor above it.
    Unrelated,
    /// `ancestor` is `entity` itself or above it.
    Ancestor,
    /// The chain above `entity` is deeper than [`MAX_DEPTH`].
    TooDeep,
}

/// Whether `ancestor` is `entity` itself or above it, walking up through `parent_of`.
fn ancestry(
    parent_of: impl Fn(Entity) -> Option<Entity>,
    ancestor: Entity,
    entity: Entity,
) -> Ancestry {
    let mut current = entity;
    for _ in 0..MAX_DEPTH {
        if current == ancestor {
            return Ancestry::Ancestor;
        }
        match parent_of(current) {
            Some(parent) => current = parent,
            None => return Ancestry::Unrelated,
        }
    }
    Ancestry::TooDeep
}

fn child_of_inserted(world: &mut HookWorld<'_>, context: HookContext) {
    let child = context.entity;
    let Some(parent) = world.get::<ChildOf>(child).map(ChildOf::parent) else {
        return;
    };
    let refused = if !world.contains(parent) {
        Some("the parent isn't alive")
    } else if world.is_despawning(parent) {
        Some("the parent is being despawned")
    } else {
        match ancestry(
            |e| world.get::<ChildOf>(e).map(ChildOf::parent),
            child,
            parent,
        ) {
            Ancestry::Unrelated => None,
            Ancestry::Ancestor => Some("the parent is the entity itself or one of its descendants"),
            Ancestry::TooDeep => Some("the hierarchy above the parent is too deep"),
        }
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
    world.commands().run(move |world| {
        // Re-checked when it runs: the link may have changed since.
        let Some(link) = world.get::<ChildOf>(child).copied() else {
            return;
        };
        if link.parent() != parent {
            return;
        }
        if !world.contains(parent) || world.is_despawning(parent) {
            // The parent went (or started going) after the link was made, so its despawn didn't
            // see this child. Resolve the child now, as the despawn would have.
            resolve_orphan(world, child, link);
            return;
        }
        edit_children(world, parent, |list| {
            if !list.contains(&child) {
                list.push(child);
            }
        });
    });
}

/// Handles a child whose parent has gone, by its link's policy: despawn it, or detach it keeping
/// its place in the world (as far as the parent's transform can still be read).
fn resolve_orphan(world: &mut World, child: Entity, link: ChildOf) {
    if world.is_despawning(child) {
        return;
    }
    if link.on_parent_despawn() == OnParentDespawn::Despawn {
        world.despawn(child);
    } else {
        remove_parent(world, child, TransformRule::KeepWorld);
    }
}

fn child_of_replaced(world: &mut HookWorld<'_>, context: HookContext) {
    let child = context.entity;
    let Some(parent) = world.get::<ChildOf>(child).map(ChildOf::parent) else {
        return;
    };
    world
        .commands()
        .run(move |world| edit_children(world, parent, |list| list.retain(|&c| c != child)));
}

/// The stamp of an entity's current [`Children`]. Private to this crate, so no other code can
/// name, change or remove it.
///
/// Every legitimate edit of `Children` bumps it and stamps the list to match. Removing `Children`
/// bumps it too. So a list taken off an entity and put back later carries an old stamp, and a
/// list taken off another entity carries another owner: [`children_inserted`] detects both in
/// constant time, without looking at the children.
///
/// **Invariant:** it outlives `Children` and must never be removed, reset or copied while the
/// entity lives: it stays on former parents on purpose (8 bytes, and their own archetype). If it
/// were removed, numbering would restart and an old list could match again. Paths that copy or
/// remove components by ID (reflection, scene loading, entity cloning, ADR-0014 §3) must skip it
/// and `Children` both, and rebuild `Children` from the `ChildOf` links instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChildrenStamp(u64);

impl Component for ChildrenStamp {}

/// Bumps `parent`'s children stamp and returns the new value.
fn next_stamp(world: &mut World, parent: Entity) -> u64 {
    if let Some(mut stamp) = world.get_mut::<ChildrenStamp>(parent) {
        stamp.0 = stamp.0.wrapping_add(1);
        return stamp.0;
    }
    1
}

/// Applies `edit` to `parent`'s children: in place through [`World::modify`] when it has some
/// (so the hooks run, without copying the list), by inserting a new `Children` when it has none,
/// and by removing it when the list ends up empty. Does nothing to a dead or despawning parent,
/// whose children are being resolved by its despawn.
fn edit_children(world: &mut World, parent: Entity, edit: impl FnOnce(&mut Vec<Entity>)) {
    if !world.contains(parent) || world.is_despawning(parent) {
        return;
    }
    if let Some(children) = world.get::<Children>(parent) {
        if children.owner != parent {
            // A foreign list, already queued for a rebuild from the links: editing it would only
            // trigger a second rebuild. The rebuild sees this change's link anyway.
            return;
        }
        let stamp = next_stamp(world, parent);
        let empty = world.modify::<Children, _>(parent, |mut children| {
            edit(&mut children.list);
            children.stamp = stamp;
            children.list.is_empty()
        });
        if empty == Some(true) {
            world.remove::<Children>(parent);
        }
        return;
    }
    let mut list = Vec::new();
    edit(&mut list);
    if list.is_empty() {
        return;
    }
    let stamp = next_stamp(world, parent);
    // Can't fail: the parent is alive and not despawning. Stamp and list go in as one bundle,
    // and hooks run after the whole bundle is written, so the list's hook finds them matching.
    let _ = world.insert(
        parent,
        (
            ChildrenStamp(stamp),
            Children {
                owner: parent,
                stamp,
                list,
            },
        ),
    );
}

/// Checks, in constant time, that an inserted or edited `Children` is this crate's own current
/// list for this entity. A list taken off another entity (wrong owner), or put back after the
/// entity's list changed (old stamp), is rebuilt from the `ChildOf` links instead.
fn children_inserted(world: &mut HookWorld<'_>, context: HookContext) {
    let parent = context.entity;
    let Some(children) = world.get::<Children>(parent) else {
        return;
    };
    let current = world.get::<ChildrenStamp>(parent).map(|s| s.0);
    if children.owner == parent && current == Some(children.stamp) {
        return;
    }
    #[cfg(test)]
    REPAIRS.with(|repairs| repairs.set(repairs.get() + 1));
    warn!(
        ?parent,
        "a Children that isn't this entity's current list was inserted; rebuilding it from the \
         ChildOf links"
    );
    world.commands().run(move |world| {
        // Rebuilt from the links themselves, which are the truth. The inserted list may have
        // replaced a correct one, so filtering it alone could lose real children. This scans
        // the world, which is fine for a repair that correct code never needs.
        let links: std::collections::HashSet<Entity> = world
            .query::<(Entity, &ChildOf)>()
            .filter(|(_, link)| link.parent() == parent)
            .map(|(child, _)| child)
            .collect();
        let mut repaired: Vec<Entity> = Vec::with_capacity(links.len());
        // Listed children keep their order; the others follow in a stable order.
        if let Some(listed) = world.get::<Children>(parent) {
            for &kid in &listed.list {
                if links.contains(&kid) && !repaired.contains(&kid) {
                    repaired.push(kid);
                }
            }
        }
        let mut rest: Vec<Entity> = links
            .into_iter()
            .filter(|k| !repaired.contains(k))
            .collect();
        rest.sort_unstable();
        repaired.extend(rest);
        if repaired.is_empty() {
            world.remove::<Children>(parent);
        } else {
            let stamp = next_stamp(world, parent);
            let _ = world.insert(
                parent,
                (
                    ChildrenStamp(stamp),
                    Children {
                        owner: parent,
                        stamp,
                        list: repaired,
                    },
                ),
            );
        }
    });
}

#[cfg(test)]
thread_local! {
    /// How many repairs ran on this thread, so tests can check correct code never needs one.
    pub(crate) static REPAIRS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Resolves the children when `Children` goes: by each link's policy when the parent is
/// despawned, by detaching (keeping their place in the world) when `Children` is removed alone.
fn children_removed(world: &mut HookWorld<'_>, context: HookContext) {
    let parent = context.entity;
    let Some(children) = world.get::<Children>(parent) else {
        return;
    };
    if children.owner != parent {
        // A foreign list that's about to be repaired away: its children aren't ours.
        return;
    }
    let children = children.list.clone();
    // This list is now out of date: a copy put back later must be recognized as stale.
    if let Some(mut stamp) = world.get_mut::<ChildrenStamp>(parent) {
        stamp.0 = stamp.0.wrapping_add(1);
    }
    let despawning = context.cause == HookCause::Despawn;
    for child in children {
        world.commands().run(move |world| {
            // Re-checked when it runs: an earlier command may have moved the child.
            let Some(link) = world.get::<ChildOf>(child).copied() else {
                return;
            };
            if link.parent() != parent || world.is_despawning(child) {
                return;
            }
            if despawning && link.on_parent_despawn() == OnParentDespawn::Despawn {
                world.despawn(child);
            } else {
                remove_parent(world, child, TransformRule::KeepWorld);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(index: u32) -> Entity {
        Entity::from_bits((1 << 32) | u64::from(index)).expect("valid bits")
    }

    #[test]
    fn ancestry_finds_ancestors_and_stops_at_the_depth_limit() {
        // A chain 0 <- 1 <- 2 <- ... where each index's parent is the one before it.
        let parent_of = |e: Entity| e.index().checked_sub(1).map(entity);
        assert!(matches!(
            ancestry(parent_of, entity(0), entity(5)),
            Ancestry::Ancestor
        ));
        assert!(matches!(
            ancestry(parent_of, entity(5), entity(5)),
            Ancestry::Ancestor
        ));
        assert!(matches!(
            ancestry(parent_of, entity(9), entity(5)),
            Ancestry::Unrelated
        ));
        // An endless chain upward: the walk gives up rather than looping.
        let endless = |e: Entity| Some(entity(e.index() + 1));
        assert!(matches!(
            ancestry(endless, entity(0), entity(1)),
            Ancestry::TooDeep
        ));
        let message = HierarchyError::TooDeep {
            child: entity(1),
            parent: entity(2),
            limit: MAX_DEPTH,
        }
        .to_string();
        assert!(message.contains("Flatten"), "{message}");
    }
}
