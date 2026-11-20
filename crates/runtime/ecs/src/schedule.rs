//! Systems, stages and the schedule that runs them (ADR-0014 §5).

use std::any::{TypeId, type_name};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::fmt;
use std::sync::Arc;

use oxreal_diagnostics::warn;

use crate::change::Mut;
use crate::command::{Commands, Source};
use crate::component::{Component, Components};
use crate::entity::{Entities, Entity};
use crate::query::{self, AccessKind, QueryData, QueryFilter, QueryIter, ReadOnlyQueryData};
use crate::resource::{Resource, Resources};
use crate::storage::Archetypes;
use crate::world::{self, World};

/// When in a frame a system runs: Unreal's tick groups, preceded by input (ADR-0008 §1,
/// ADR-0014 §5). Each stage ends with a **sync point**, where the commands its systems recorded
/// are applied.
///
/// More stages may be added (for example a fixed-step sub-stage), so matches need a wildcard
/// arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Stage {
    /// Devices are read and input is routed, before gameplay sees the frame.
    Input,
    /// Ordinary gameplay. The default, as `TG_PrePhysics` is for Unreal actors.
    PrePhysics,
    /// Work that runs alongside physics. It runs once per [`Schedule::run`] until the fixed-step
    /// clock arrives with physics.
    DuringPhysics,
    /// Gameplay that reacts to the physics results.
    PostPhysics,
    /// Late updates, such as transform propagation and cameras.
    PostUpdate,
    /// Copies what the renderer needs out of the world.
    Extract,
}

/// How many stages there are. Private, so adding a stage changes no public type.
const STAGE_COUNT: usize = 6;

impl Stage {
    /// Every stage, in the order they run.
    pub const ALL: &'static [Stage] = &[
        Stage::Input,
        Stage::PrePhysics,
        Stage::DuringPhysics,
        Stage::PostPhysics,
        Stage::PostUpdate,
        Stage::Extract,
    ];

    const fn index(self) -> usize {
        self as usize
    }

    /// The stage's name, as written in docs and messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Stage::Input => "Input",
            Stage::PrePhysics => "PrePhysics",
            Stage::DuringPhysics => "DuringPhysics",
            Stage::PostPhysics => "PostPhysics",
            Stage::PostUpdate => "PostUpdate",
            Stage::Extract => "Extract",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

const _: () = assert!(
    Stage::ALL.len() == STAGE_COUNT,
    "STAGE_COUNT is out of date"
);

/// Type IDs and names a system declared, with how it may use each.
#[derive(Debug, Default, Clone)]
struct Declared {
    components: Vec<(TypeId, &'static str, AccessKind)>,
    resources: Vec<(TypeId, &'static str, AccessKind)>,
}

fn declare<T: 'static>(list: &mut Vec<(TypeId, &'static str, AccessKind)>, kind: AccessKind) {
    let id = TypeId::of::<T>();
    match list.iter_mut().find(|entry| entry.0 == id) {
        // Writing includes reading.
        Some(entry) => {
            if kind == AccessKind::Write {
                entry.2 = AccessKind::Write;
            }
        }
        None => list.push((id, type_name::<T>(), kind)),
    }
}

fn allows(list: &[(TypeId, &'static str, AccessKind)], id: TypeId, kind: AccessKind) -> bool {
    list.iter()
        .any(|entry| entry.0 == id && (kind == AccessKind::Read || entry.2 == AccessKind::Write))
}

#[cold]
#[track_caller]
fn undeclared(system: &str, what: &str, name: &str, kind: AccessKind) -> ! {
    let (verb, method) = match kind {
        AccessKind::Read => ("reads", "reads"),
        AccessKind::Write => ("writes", "writes"),
    };
    let suffix = if what == "resource" { "_resource" } else { "" };
    panic!(
        "system `{system}` {verb} {what} {name} without declaring it: add \
         `.{method}{suffix}::<{name}>()` to its `System`"
    );
}

/// The work a system does each frame, as stored by a [`System`].
type Run = Box<dyn FnMut(&mut SystemContext<'_>) + Send>;

/// A unit of per-frame work: a name, the data it may touch, ordering constraints, and a closure.
///
/// A system **declares** every component and resource it reads or writes (ADR-0014 §5). That's
/// what will let a parallel executor run non-conflicting systems together without an API
/// change. Touching anything undeclared panics, naming the missing declaration.
///
/// ```
/// use oxreal_ecs::{Component, Resource, System};
///
/// struct Position(f32);
/// impl Component for Position {}
/// struct Velocity(f32);
/// impl Component for Velocity {}
/// struct FrameTime(f32);
/// impl Resource for FrameTime {}
///
/// let movement = System::new("movement", |ctx| {
///     let dt = ctx.resources.get::<FrameTime>().map_or(0.0, |t| t.0);
///     for (mut position, velocity) in ctx.world.query_mut::<(&mut Position, &Velocity)>() {
///         position.0 += velocity.0 * dt;
///     }
/// })
/// .writes::<Position>()
/// .reads::<Velocity>()
/// .reads_resource::<FrameTime>();
/// # let _ = movement;
/// ```
pub struct System {
    name: Arc<str>,
    run: Run,
    declared: Declared,
    after: Vec<String>,
    before: Vec<String>,
}

impl fmt::Debug for System {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("System")
            .field("name", &self.name)
            .field("declared", &self.declared)
            .field("after", &self.after)
            .field("before", &self.before)
            .finish_non_exhaustive()
    }
}

impl System {
    /// A system named `name` (unique within its schedule) that runs `run` each frame. The
    /// closure may keep state between frames.
    pub fn new(
        name: impl Into<String>,
        run: impl FnMut(&mut SystemContext<'_>) + Send + 'static,
    ) -> Self {
        Self {
            name: Arc::from(name.into()),
            run: Box::new(run),
            declared: Declared::default(),
            after: Vec::new(),
            before: Vec::new(),
        }
    }

    /// The system's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Declares that the system reads component `T`.
    #[must_use]
    pub fn reads<T: Component>(mut self) -> Self {
        declare::<T>(&mut self.declared.components, AccessKind::Read);
        self
    }

    /// Declares that the system writes component `T` (and may read it).
    #[must_use]
    pub fn writes<T: Component>(mut self) -> Self {
        const { crate::component::assert_mutable::<T>() };
        declare::<T>(&mut self.declared.components, AccessKind::Write);
        self
    }

    /// Declares that the system reads resource `R`.
    #[must_use]
    pub fn reads_resource<R: Resource>(mut self) -> Self {
        declare::<R>(&mut self.declared.resources, AccessKind::Read);
        self
    }

    /// Declares that the system writes resource `R` (and may read it).
    #[must_use]
    pub fn writes_resource<R: Resource>(mut self) -> Self {
        declare::<R>(&mut self.declared.resources, AccessKind::Write);
        self
    }

    /// Runs this system after the system named `other`, which must be in the same stage.
    #[must_use]
    pub fn after(mut self, other: impl Into<String>) -> Self {
        self.after.push(other.into());
        self
    }

    /// Runs this system before the system named `other`, which must be in the same stage.
    #[must_use]
    pub fn before(mut self, other: impl Into<String>) -> Self {
        self.before.push(other.into());
        self
    }
}

/// What a running system can reach. The three fields borrow different parts of the world, so a
/// system can iterate a query, read a resource and record commands at the same time.
#[non_exhaustive]
#[derive(Debug)]
pub struct SystemContext<'a> {
    /// Entities and components: queries and lookups, limited to what the system declared.
    pub world: SystemWorld<'a>,
    /// Resources, limited to what the system declared.
    pub resources: SystemResources<'a>,
    /// Structural changes, applied at the end of the stage.
    pub commands: Commands<'a>,
}

/// A system's view of entities and components. Structure can't change here (use
/// [`Commands`]), and only declared components are reachable.
pub struct SystemWorld<'a> {
    archetypes: &'a mut Archetypes,
    components: &'a Components,
    entities: &'a Entities,
    declared: &'a Declared,
    system: &'a str,
}

impl fmt::Debug for SystemWorld<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemWorld")
            .field("system", &self.system)
            .finish_non_exhaustive()
    }
}

impl SystemWorld<'_> {
    #[track_caller]
    fn check_query<D: QueryData>(&self) {
        for (id, name, kind) in query::accesses::<D>() {
            if !allows(&self.declared.components, id, kind) {
                undeclared(self.system, "component", name, kind);
            }
        }
    }

    #[track_caller]
    fn check<T: 'static>(&self, kind: AccessKind) {
        if !allows(&self.declared.components, TypeId::of::<T>(), kind) {
            undeclared(self.system, "component", type_name::<T>(), kind);
        }
    }

    /// Iterates every entity matching `D`, reading only. See [`World::query`].
    ///
    /// # Panics
    ///
    /// If `D` aliases, or reads a component the system didn't declare.
    #[track_caller]
    pub fn query<D: ReadOnlyQueryData>(&self) -> QueryIter<'_, D> {
        self.check_query::<D>();
        QueryIter::shared(
            self.archetypes.iter(),
            query::prepare::<D, ()>(self.components),
        )
    }

    /// Like [`SystemWorld::query`], visiting only entities that pass filter `F`.
    ///
    /// # Panics
    ///
    /// If `D` aliases, or reads a component the system didn't declare.
    #[track_caller]
    pub fn query_filtered<D: ReadOnlyQueryData, F: QueryFilter>(&self) -> QueryIter<'_, D, F> {
        self.check_query::<D>();
        QueryIter::shared(
            self.archetypes.iter(),
            query::prepare::<D, F>(self.components),
        )
    }

    /// Iterates every entity matching `D`, which may write. See [`World::query_mut`].
    ///
    /// # Panics
    ///
    /// If `D` aliases, or uses a component in a way the system didn't declare.
    #[track_caller]
    pub fn query_mut<D: QueryData>(&mut self) -> QueryIter<'_, D> {
        self.check_query::<D>();
        let state = query::prepare::<D, ()>(self.components);
        QueryIter::unique(self.archetypes.iter_mut(), state)
    }

    /// Like [`SystemWorld::query_mut`], visiting only entities that pass filter `F`.
    ///
    /// # Panics
    ///
    /// If `D` aliases, or uses a component in a way the system didn't declare.
    #[track_caller]
    pub fn query_filtered_mut<D: QueryData, F: QueryFilter>(&mut self) -> QueryIter<'_, D, F> {
        self.check_query::<D>();
        let state = query::prepare::<D, F>(self.components);
        QueryIter::unique(self.archetypes.iter_mut(), state)
    }

    /// `entity`'s component `T`. See [`World::get`].
    ///
    /// # Panics
    ///
    /// If the system didn't declare reading or writing `T`.
    #[track_caller]
    #[must_use]
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        self.check::<T>(AccessKind::Read);
        world::get_component(self.entities, self.components, self.archetypes, entity)
    }

    /// `entity`'s component `T`, for writing. See [`World::get_mut`].
    ///
    /// # Panics
    ///
    /// If the system didn't declare writing `T`.
    #[track_caller]
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<Mut<'_, T>> {
        self.check::<T>(AccessKind::Write);
        world::get_component_mut(self.entities, self.components, self.archetypes, entity)
    }

    /// Whether `entity` is alive and has a `T`. Needs no declaration: it reads structure, not
    /// component data.
    #[must_use]
    pub fn has<T: Component>(&self, entity: Entity) -> bool {
        world::has_component::<T>(self.entities, self.components, self.archetypes, entity)
    }

    /// Whether `entity` is alive. Entities spawned by commands become alive at the sync point.
    #[must_use]
    pub fn contains(&self, entity: Entity) -> bool {
        self.entities.location(entity).is_some()
    }
}

/// A system's view of resources: only the declared ones are reachable.
pub struct SystemResources<'a> {
    resources: &'a mut Resources,
    declared: &'a Declared,
    system: &'a str,
}

impl fmt::Debug for SystemResources<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemResources")
            .field("system", &self.system)
            .finish_non_exhaustive()
    }
}

impl SystemResources<'_> {
    #[track_caller]
    fn check<R: 'static>(&self, kind: AccessKind) {
        if !allows(&self.declared.resources, TypeId::of::<R>(), kind) {
            undeclared(self.system, "resource", type_name::<R>(), kind);
        }
    }

    /// The resource of type `R`, if the world has one.
    ///
    /// # Panics
    ///
    /// If the system didn't declare reading or writing `R`.
    #[track_caller]
    #[must_use]
    pub fn get<R: Resource>(&self) -> Option<&R> {
        self.check::<R>(AccessKind::Read);
        self.resources.get()
    }

    /// The resource of type `R`, mutably.
    ///
    /// # Panics
    ///
    /// If the system didn't declare writing `R`.
    #[track_caller]
    pub fn get_mut<R: Resource>(&mut self) -> Option<Mut<'_, R>> {
        self.check::<R>(AccessKind::Write);
        self.resources.get_mut().map(Mut::new)
    }
}

/// Why a schedule can't run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ScheduleError {
    /// Two systems share a name. Names identify systems in ordering constraints and messages.
    #[error("two systems are named `{name}`; system names must be unique within a schedule")]
    DuplicateName {
        /// The repeated name.
        name: String,
    },
    /// A system is ordered relative to a name no system has.
    #[error("system `{system}` is ordered relative to `{other}`, which isn't in this schedule")]
    UnknownSystem {
        /// The system with the constraint.
        system: String,
        /// The name it refers to.
        other: String,
    },
    /// A system is ordered relative to a system in another stage.
    #[error(
        "system `{system}` ({stage}) is ordered relative to `{other}`, which runs in \
         {other_stage}; stages already run in order, so order systems only within a stage"
    )]
    AcrossStages {
        /// The system with the constraint.
        system: String,
        /// Its stage.
        stage: Stage,
        /// The system it refers to.
        other: String,
        /// That system's stage.
        other_stage: Stage,
    },
    /// Ordering constraints form a cycle, so no order satisfies them.
    #[error("the systems of {stage} are ordered in a cycle: {}", cycle.join(" → "))]
    Cycle {
        /// The stage.
        stage: Stage,
        /// The systems on the cycle, each running after the previous, back to the first.
        cycle: Vec<String>,
    },
}

/// Runs systems stage by stage, applying their commands at each stage's end (ADR-0014 §5).
///
/// Within a stage, systems run in an order that satisfies their `before`/`after` constraints.
/// Ties keep the order systems were added in, so a schedule always runs the same way. The
/// order is computed, and checked for unknown names and cycles, on the first run after systems
/// are added ([`Schedule::validate`] checks it earlier).
#[derive(Debug, Default)]
pub struct Schedule {
    stages: [Vec<System>; STAGE_COUNT],
    /// Per stage, indices into `stages` in run order. `None` until validated.
    order: Option<[Vec<usize>; STAGE_COUNT]>,
}

impl Schedule {
    /// An empty schedule.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `system` to `stage`.
    pub fn add(&mut self, stage: Stage, system: System) -> &mut Self {
        self.stages[stage.index()].push(system);
        self.order = None;
        self
    }

    /// Checks the systems' names and ordering constraints, and computes the run order.
    ///
    /// # Errors
    ///
    /// [`ScheduleError`] for duplicate names, unknown or cross-stage ordering, and cycles.
    pub fn validate(&mut self) -> Result<(), ScheduleError> {
        if self.order.is_none() {
            self.order = Some(compute_order(&self.stages)?);
        }
        Ok(())
    }

    /// The names of `stage`'s systems in the order they run.
    ///
    /// # Errors
    ///
    /// As for [`Schedule::validate`].
    pub fn run_order(&mut self, stage: Stage) -> Result<Vec<&str>, ScheduleError> {
        self.validate()?;
        let systems = &self.stages[stage.index()];
        Ok(self.order.as_ref().map_or_else(Vec::new, |order| {
            order[stage.index()]
                .iter()
                .map(|&i| &*systems[i].name)
                .collect()
        }))
    }

    /// Runs every stage once, in order: each stage's systems, then its sync point.
    ///
    /// Commands are applied per system in the stage's run order, and each system's commands in
    /// the order it recorded them (ADR-0014 §5).
    ///
    /// # Errors
    ///
    /// As for [`Schedule::validate`]. Nothing runs if the schedule is invalid.
    ///
    /// # Panics
    ///
    /// A panic in a system, or in a command at the sync point, propagates. For the stage it
    /// interrupts:
    /// - component values already written stay written;
    /// - none of the stage's remaining commands are applied: they're discarded, with a warning;
    /// - handles that discarded `spawn` commands reserved never become live, and their slots are
    ///   never reused, so they can't alias.
    ///
    /// The schedule and the world stay usable for the next run.
    pub fn run(&mut self, world: &mut World) -> Result<(), ScheduleError> {
        oxreal_diagnostics::profile_function!();
        self.validate()?;
        let Some(order) = &self.order else {
            return Ok(());
        };
        // Commands left in the world (only possible if a panic interrupted a direct world
        // operation) would otherwise be applied at the first sync point, out of context.
        if world.discard_commands() {
            warn!("discarding commands left over from an interrupted world operation");
        }
        let world = DiscardOnPanic(world);
        for &stage in Stage::ALL {
            oxreal_diagnostics::profile_scope!("ecs stage");
            let systems = &mut self.stages[stage.index()];
            for &index in &order[stage.index()] {
                let System {
                    name,
                    run,
                    declared,
                    ..
                } = &mut systems[index];
                let parts = world.0.split();
                let mut ctx = SystemContext {
                    world: SystemWorld {
                        archetypes: parts.archetypes,
                        components: parts.components,
                        entities: parts.entities,
                        declared,
                        system: name,
                    },
                    resources: SystemResources {
                        resources: parts.resources,
                        declared,
                        system: name,
                    },
                    commands: Commands::new(
                        parts.entities,
                        parts.queue,
                        Source::System(name.clone()),
                    ),
                };
                run(&mut ctx);
            }
            // The sync point.
            world.0.apply_commands();
        }
        Ok(())
    }
}

/// Empties the world's command queue if a panic unwinds through [`Schedule::run`], so an
/// abandoned stage's commands are never applied later.
struct DiscardOnPanic<'a>(&'a mut World);

impl Drop for DiscardOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() && self.0.discard_commands() {
            warn!("a panic interrupted a stage; discarding its commands");
        }
    }
}

/// Checks names and constraints and orders each stage's systems.
fn compute_order(
    stages: &[Vec<System>; STAGE_COUNT],
) -> Result<[Vec<usize>; STAGE_COUNT], ScheduleError> {
    let mut where_is: HashMap<&str, (Stage, usize)> = HashMap::new();
    for &stage in Stage::ALL {
        for (index, system) in stages[stage.index()].iter().enumerate() {
            if where_is.insert(&*system.name, (stage, index)).is_some() {
                return Err(ScheduleError::DuplicateName {
                    name: system.name.to_string(),
                });
            }
        }
    }
    let mut order: [Vec<usize>; STAGE_COUNT] = Default::default();
    for &stage in Stage::ALL {
        order[stage.index()] = order_stage(stage, &stages[stage.index()], &where_is)?;
    }
    Ok(order)
}

/// Orders one stage: a topological sort that always picks the earliest-added ready system.
fn order_stage(
    stage: Stage,
    systems: &[System],
    where_is: &HashMap<&str, (Stage, usize)>,
) -> Result<Vec<usize>, ScheduleError> {
    let resolve = |system: &System, other: &str| match where_is.get(other) {
        Some(&(other_stage, index)) if other_stage == stage => Ok(index),
        Some(&(other_stage, _)) => Err(ScheduleError::AcrossStages {
            system: system.name.to_string(),
            stage,
            other: other.to_owned(),
            other_stage,
        }),
        None => Err(ScheduleError::UnknownSystem {
            system: system.name.to_string(),
            other: other.to_owned(),
        }),
    };
    // successors[a] lists the systems that run after a.
    let mut successors = vec![Vec::new(); systems.len()];
    let mut predecessors = vec![Vec::new(); systems.len()];
    for (index, system) in systems.iter().enumerate() {
        for other in &system.after {
            let first = resolve(system, other)?;
            successors[first].push(index);
            predecessors[index].push(first);
        }
        for other in &system.before {
            let then = resolve(system, other)?;
            successors[index].push(then);
            predecessors[then].push(index);
        }
    }
    let mut waiting: Vec<usize> = predecessors.iter().map(Vec::len).collect();
    let mut ready: BinaryHeap<Reverse<usize>> = (0..systems.len())
        .filter(|&i| waiting[i] == 0)
        .map(Reverse)
        .collect();
    let mut sorted = Vec::with_capacity(systems.len());
    while let Some(Reverse(next)) = ready.pop() {
        sorted.push(next);
        for &after in &successors[next] {
            waiting[after] -= 1;
            if waiting[after] == 0 {
                ready.push(Reverse(after));
            }
        }
    }
    if sorted.len() == systems.len() {
        return Ok(sorted);
    }
    // Every system left over waits on another left-over system, so walking back through
    // predecessors must revisit one: that loop is a cycle.
    let Some(start) = (0..systems.len()).find(|&i| waiting[i] > 0) else {
        unreachable!("a stage failed to sort without a waiting system: a bug in oxreal-ecs");
    };
    let mut path = vec![start];
    let mut current = start;
    loop {
        let Some(&previous) = predecessors[current].iter().find(|&&p| waiting[p] > 0) else {
            unreachable!("a waiting system has no waiting predecessor: a bug in oxreal-ecs");
        };
        if let Some(position) = path.iter().position(|&p| p == previous) {
            let mut cycle: Vec<String> = path[position..]
                .iter()
                .rev()
                .map(|&i| systems[i].name.to_string())
                .collect();
            cycle.push(cycle[0].clone());
            return Err(ScheduleError::Cycle { stage, cycle });
        }
        path.push(previous);
        current = previous;
    }
}
