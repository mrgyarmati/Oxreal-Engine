//! Deferred structural changes (ADR-0014 §5).

use std::fmt;

use oxreal_diagnostics::warn;

use crate::bundle::{self, Bundle};
use crate::component::Component;
use crate::entity::{Entities, Entity};
use crate::world::World;

/// A recorded change, applied at a sync point. The `&str` is the name of the system that
/// recorded it, for diagnostics.
type Command = Box<dyn FnOnce(&mut World, &str) + Send>;

/// Commands recorded during a stage, in order, with the index of the system that recorded each.
#[derive(Default)]
pub(crate) struct CommandQueue {
    commands: Vec<(usize, Command)>,
}

impl fmt::Debug for CommandQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandQueue")
            .field("commands", &self.commands.len())
            .finish()
    }
}

impl CommandQueue {
    /// Applies every command in recording order, then empties the queue. `name` gives the name
    /// of the system with a given index.
    pub(crate) fn apply<'n>(&mut self, world: &mut World, name: impl Fn(usize) -> &'n str) {
        world.flush_reservations();
        for (system, command) in self.commands.drain(..) {
            command(world, name(system));
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.commands.is_empty()
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
    system: usize,
}

impl fmt::Debug for Commands<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Commands")
            .field("queued", &self.queue.commands.len())
            .finish_non_exhaustive()
    }
}

impl<'a> Commands<'a> {
    pub(crate) fn new(entities: &'a Entities, queue: &'a mut CommandQueue, system: usize) -> Self {
        Self {
            entities,
            queue,
            system,
        }
    }

    fn push(&mut self, command: impl FnOnce(&mut World, &str) + Send + 'static) {
        self.queue.commands.push((self.system, Box::new(command)));
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
        self.push(move |world, system| {
            if !world.spawn_reserved(entity, bundle) {
                unreachable!(
                    "{entity:?}, reserved by system `{system}`, couldn't be spawned: a bug in \
                     oxreal-ecs"
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
    /// entity is gone by then, the bundle is dropped and a warning names the system.
    ///
    /// # Panics
    ///
    /// If `bundle` contains a component type twice.
    pub fn insert<B: Bundle>(&mut self, entity: Entity, bundle: B) {
        bundle::check_duplicates::<B>();
        self.push(move |world, system| {
            if let Err(error) = world.insert(entity, bundle) {
                warn!(
                    system,
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
    pub fn run_exclusive(&mut self, f: impl FnOnce(&mut World) + Send + 'static) {
        self.push(move |world, _| f(world));
    }
}
