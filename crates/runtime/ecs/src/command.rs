//! Deferred structural changes (ADR-0014 §5).

use std::collections::VecDeque;
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
    /// The world itself: the step that finishes a despawn after its hooks' commands.
    Despawn,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::System(name) => write!(f, "system `{name}`"),
            Source::Hook { component, hook } => write!(f, "the {hook} hook of {component}"),
            Source::Despawn => f.write_str("a despawn"),
        }
    }
}

/// A recorded change, applied later with the source that recorded it.
pub(crate) type Command = Box<dyn FnOnce(&mut World, &Source) + Send>;

/// The world's commands.
///
/// - **Recorded** commands come from systems during a stage, and from hooks during a structural
///   change.
/// - **Pending** commands are waiting in the command loop. Recorded commands are promoted to the
///   *front* of the pending list, in recording order, so the commands a change caused run before
///   anything that was already waiting (depth-first) without recursion.
#[derive(Default)]
pub(crate) struct CommandQueue {
    recorded: Vec<(Source, Command)>,
    pending: VecDeque<(Source, Command)>,
}

impl fmt::Debug for CommandQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandQueue")
            .field("recorded", &self.recorded.len())
            .field("pending", &self.pending.len())
            .finish()
    }
}

impl CommandQueue {
    pub(crate) fn is_empty(&self) -> bool {
        self.recorded.is_empty() && self.pending.is_empty()
    }

    /// Records a command.
    pub(crate) fn record(&mut self, source: Source, command: Command) {
        self.recorded.push((source, command));
    }

    /// Moves the recorded commands to the front of the pending list, keeping their order.
    /// Returns whether anything is pending.
    pub(crate) fn promote_recorded(&mut self) -> bool {
        for command in self.recorded.drain(..).rev() {
            self.pending.push_front(command);
        }
        !self.pending.is_empty()
    }

    /// The next pending command.
    pub(crate) fn next_pending(&mut self) -> Option<(Source, Command)> {
        self.pending.pop_front()
    }

    /// Discards every command without applying it.
    pub(crate) fn clear(&mut self) {
        self.recorded.clear();
        self.pending.clear();
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
            .field("queue", &self.queue)
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
        self.queue.record(self.source.clone(), Box::new(command));
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
