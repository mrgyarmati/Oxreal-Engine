//! Deferred structural changes (ADR-0014 §5).

use std::fmt;
use std::sync::Arc;

use oxreal_diagnostics::warn;

use crate::bundle::{self, Bundle};
use crate::component::Component;
use crate::entity::{Entities, Entity};
use crate::world::World;

/// Who recorded a command, for diagnostics.
#[derive(Debug, Clone)]
pub(crate) enum Source {
    /// A system, by name.
    System(Arc<str>),
    /// A component's lifecycle hook.
    Hook {
        component: &'static str,
        hook: &'static str,
    },
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::System(name) => write!(f, "system `{name}`"),
            Source::Hook { component, hook } => write!(f, "the {hook} hook of {component}"),
        }
    }
}

/// A recorded change, applied later with the source that recorded it.
type Command = Box<dyn FnOnce(&mut World, &Source) + Send>;

/// Recorded commands, in order. The world owns one: systems fill it during a stage, and hooks
/// during a structural change.
#[derive(Default)]
pub(crate) struct CommandQueue {
    commands: Vec<(Source, Command)>,
}

impl fmt::Debug for CommandQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandQueue")
            .field("commands", &self.commands.len())
            .finish()
    }
}

impl CommandQueue {
    pub(crate) fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Takes every queued command out, leaving the queue empty.
    pub(crate) fn take(&mut self) -> Vec<(Source, Command)> {
        std::mem::take(&mut self.commands)
    }

    /// Discards every command without applying it.
    pub(crate) fn clear(&mut self) {
        self.commands.clear();
    }
}

/// Records structural changes for the next sync point: the end of the current stage.
///
/// A system can't change the world's structure while it runs, because queries hold its
/// storage. Instead it records commands here, and they're applied in order, after every system
/// of the stage has run (ADR-0014 §5).
///
/// ```
/// use oxreal_ecs::{Component, Entity, Schedule, Stage, System, World};
///
/// struct Health(u32);
/// impl Component for Health {}
///
/// let mut world = World::new();
/// let doomed = world.spawn(Health(0));
/// let mut schedule = Schedule::new();
/// schedule.add(
///     Stage::PrePhysics,
///     System::new("remove the dead", |ctx| {
///         for (entity, health) in ctx.world.query::<(Entity, &Health)>() {
///             if health.0 == 0 {
///                 ctx.commands.despawn(entity);
///             }
///         }
///     })
///     .reads::<Health>(),
/// );
/// schedule.run(&mut world).expect("the schedule is valid");
/// assert!(!world.contains(doomed));
/// ```
pub struct Commands<'a> {
    entities: &'a Entities,
    queue: &'a mut CommandQueue,
    source: Source,
}

impl fmt::Debug for Commands<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Commands")
            .field("queued", &self.queue.commands.len())
            .finish_non_exhaustive()
    }
}

impl<'a> Commands<'a> {
    pub(crate) fn new(entities: &'a Entities, queue: &'a mut CommandQueue, source: Source) -> Self {
        Self {
            entities,
            queue,
            source,
        }
    }

    fn push(&mut self, command: impl FnOnce(&mut World, &Source) + Send + 'static) {
        self.queue
            .commands
            .push((self.source.clone(), Box::new(command)));
    }

    /// Spawns an entity with `bundle` at the sync point. The handle is valid **now**: it can be
    /// stored in components or passed to other commands. Until the sync point, looking it up
    /// finds nothing.
    ///
    /// # Panics
    ///
    /// If `bundle` contains a component type twice, or the world already holds 2³² entities.
    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> Entity {
        bundle::check_duplicates::<B>();
        let entity = self.entities.reserve();
        self.push(move |world, source| {
            if !world.spawn_reserved(entity, bundle) {
                unreachable!(
                    "{entity:?}, reserved by {source}, couldn't be spawned: a bug in oxreal-ecs"
                );
            }
        });
        entity
    }

    /// Despawns `entity` at the sync point. Nothing happens if it's already gone by then.
    pub fn despawn(&mut self, entity: Entity) {
        self.push(move |world, _| {
            world.despawn(entity);
        });
    }

    /// Adds `bundle` to `entity` at the sync point, replacing components it already has. If the
    /// entity is gone by then, the bundle is dropped and a warning names the system or hook that
    /// recorded it.
    ///
    /// # Panics
    ///
    /// If `bundle` contains a component type twice.
    pub fn insert<B: Bundle>(&mut self, entity: Entity, bundle: B) {
        bundle::check_duplicates::<B>();
        self.push(move |world, source| {
            if let Err(error) = world.insert(entity, bundle) {
                warn!(
                    source = %source,
                    %error,
                    bundle = std::any::type_name::<B>(),
                    "a command's insert was dropped"
                );
            }
        });
    }

    /// Removes component `T` from `entity` at the sync point, dropping it. Nothing happens if
    /// the entity or the component is gone by then.
    pub fn remove<T: Component>(&mut self, entity: Entity) {
        self.push(move |world, _| {
            drop(world.remove::<T>(entity));
        });
    }

    /// Runs `f` with exclusive access to the world at the sync point, in order with the other
    /// commands. For changes the other commands can't express.
    pub fn run(&mut self, f: impl FnOnce(&mut World) + Send + 'static) {
        self.push(move |world, _| f(world));
    }
}
