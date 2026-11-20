//! Tests of schedules, declared access and commands.

use std::sync::{Arc, Mutex};

use crate::{Component, Entity, Resource, Schedule, ScheduleError, Stage, System, World};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Health(u32);
impl Component for Health {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Armor(u32);
impl Component for Armor {}

#[derive(Debug, Default, PartialEq, Eq)]
struct Log(Vec<&'static str>);
impl Resource for Log {}

/// A system that appends `name` to the `Log` resource.
fn logger(name: &'static str) -> System {
    System::new(name, move |ctx| {
        if let Some(mut log) = ctx.resources.get_mut::<Log>() {
            log.0.push(name);
        }
    })
    .writes_resource::<Log>()
}

fn log_of(world: &World) -> Vec<&'static str> {
    world
        .resource::<Log>()
        .map(|log| log.0.clone())
        .unwrap_or_default()
}

#[test]
fn stages_run_in_order_and_systems_in_the_order_added() {
    let mut world = World::new();
    world.insert_resource(Log::default());
    let mut schedule = Schedule::new();
    schedule
        .add(Stage::Extract, logger("extract"))
        .add(Stage::PrePhysics, logger("gameplay 1"))
        .add(Stage::Input, logger("input"))
        .add(Stage::PrePhysics, logger("gameplay 2"))
        .add(Stage::PostUpdate, logger("late"));
    schedule.run(&mut world).expect("valid");
    assert_eq!(
        log_of(&world),
        ["input", "gameplay 1", "gameplay 2", "late", "extract"]
    );
}

#[test]
fn before_and_after_reorder_a_stage() {
    let mut schedule = Schedule::new();
    schedule
        .add(Stage::PrePhysics, logger("c").after("b"))
        .add(Stage::PrePhysics, logger("b"))
        .add(Stage::PrePhysics, logger("a").before("b"))
        .add(Stage::PrePhysics, logger("free"));
    assert_eq!(
        schedule.run_order(Stage::PrePhysics).expect("valid"),
        ["a", "b", "c", "free"]
    );

    let mut world = World::new();
    world.insert_resource(Log::default());
    schedule.run(&mut world).expect("valid");
    assert_eq!(log_of(&world), ["a", "b", "c", "free"]);
}

#[test]
fn systems_keep_their_state_between_runs() {
    let mut world = World::new();
    world.insert_resource(Log::default());
    let mut runs = 0;
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PrePhysics,
        System::new("counter", move |ctx| {
            runs += 1;
            if let Some(mut log) = ctx.resources.get_mut::<Log>() {
                log.0.push(if runs == 1 { "first" } else { "again" });
            }
        })
        .writes_resource::<Log>(),
    );
    schedule.run(&mut world).expect("valid");
    schedule.run(&mut world).expect("valid");
    assert_eq!(log_of(&world), ["first", "again"]);
}

#[test]
fn bad_schedules_are_explained() {
    let mut duplicate = Schedule::new();
    duplicate
        .add(Stage::Input, logger("x"))
        .add(Stage::Extract, logger("x"));
    assert_eq!(
        duplicate.validate(),
        Err(ScheduleError::DuplicateName { name: "x".into() })
    );

    let mut unknown = Schedule::new();
    unknown.add(Stage::Input, logger("x").after("missing"));
    assert_eq!(
        unknown.validate(),
        Err(ScheduleError::UnknownSystem {
            system: "x".into(),
            other: "missing".into()
        })
    );

    let mut across = Schedule::new();
    across
        .add(Stage::Input, logger("read"))
        .add(Stage::PrePhysics, logger("use").after("read"));
    let error = across.validate().expect_err("cross-stage ordering");
    assert!(
        error
            .to_string()
            .contains("order systems only within a stage")
    );

    let mut cycle = Schedule::new();
    cycle
        .add(Stage::PostUpdate, logger("ok"))
        .add(Stage::PostUpdate, logger("a").after("c"))
        .add(Stage::PostUpdate, logger("b").after("a"))
        .add(Stage::PostUpdate, logger("c").after("b"));
    match cycle.validate() {
        Err(ScheduleError::Cycle { stage, cycle }) => {
            assert_eq!(stage, Stage::PostUpdate);
            assert_eq!(cycle.len(), 4, "{cycle:?}");
            assert_eq!(cycle.first(), cycle.last());
            assert!(!cycle.contains(&"ok".to_owned()));
        }
        other => panic!("expected a cycle, got {other:?}"),
    }

    // Nothing runs from an invalid schedule.
    let mut world = World::new();
    world.insert_resource(Log::default());
    assert!(cycle.run(&mut world).is_err());
    assert!(log_of(&world).is_empty());
}

#[test]
#[should_panic(expected = "system `sneaky` reads component")]
fn reading_an_undeclared_component_panics() {
    let mut world = World::new();
    world.spawn(Health(1));
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PrePhysics,
        System::new("sneaky", |ctx| {
            let _ = ctx.world.query::<&Health>().count();
        }),
    );
    let _ = schedule.run(&mut world);
}

#[test]
#[should_panic(expected = "add `.writes::<")]
fn writing_a_component_declared_read_only_panics() {
    let mut world = World::new();
    world.spawn(Health(1));
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PrePhysics,
        System::new("reader", |ctx| {
            let _ = ctx.world.query_mut::<&mut Health>().count();
        })
        .reads::<Health>(),
    );
    let _ = schedule.run(&mut world);
}

#[test]
#[should_panic(expected = "add `.reads_resource::<")]
fn reading_an_undeclared_resource_panics() {
    let mut world = World::new();
    world.insert_resource(Log::default());
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PrePhysics,
        System::new("peeker", |ctx| {
            let _ = ctx.resources.get::<Log>();
        }),
    );
    let _ = schedule.run(&mut world);
}

#[test]
fn declaring_a_write_allows_reads_and_lookups() {
    let mut world = World::new();
    let e = world.spawn((Health(1), Armor(2)));
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PrePhysics,
        System::new("heal", move |ctx| {
            let armor = ctx.world.get::<Armor>(e).map_or(0, |a| a.0);
            assert!(ctx.world.has::<Armor>(e), "has needs no declaration");
            let total: u32 = ctx.world.query::<&Health>().map(|h| h.0).sum();
            for mut health in ctx.world.query_mut::<&mut Health>() {
                health.0 += total + armor;
            }
            if let Some(mut health) = ctx.world.get_mut::<Health>(e) {
                health.0 *= 10;
            }
        })
        .writes::<Health>()
        .reads::<Armor>(),
    );
    schedule.run(&mut world).expect("valid");
    assert_eq!(world.get::<Health>(e), Some(&Health(40)));
}

#[test]
fn commands_apply_at_the_end_of_the_stage() {
    let mut world = World::new();
    let spawned = Arc::new(Mutex::new(None::<Entity>));
    let seen_in_stage = Arc::new(Mutex::new(None::<bool>));
    let seen_next_stage = Arc::new(Mutex::new(None::<bool>));
    let mut schedule = Schedule::new();
    let (s1, s2) = (spawned.clone(), seen_in_stage.clone());
    schedule.add(
        Stage::PrePhysics,
        System::new("spawner", move |ctx| {
            let entity = ctx.commands.spawn(Health(5));
            *s2.lock().expect("lock") = Some(ctx.world.contains(entity));
            *s1.lock().expect("lock") = Some(entity);
        }),
    );
    let (s1, s3) = (spawned.clone(), seen_next_stage.clone());
    schedule.add(
        Stage::PostPhysics,
        System::new("checker", move |ctx| {
            let entity = s1.lock().expect("lock").expect("spawned");
            *s3.lock().expect("lock") = Some(ctx.world.get::<Health>(entity) == Some(&Health(5)));
        })
        .reads::<Health>(),
    );
    schedule.run(&mut world).expect("valid");
    assert_eq!(
        *seen_in_stage.lock().expect("lock"),
        Some(false),
        "not alive yet"
    );
    assert_eq!(
        *seen_next_stage.lock().expect("lock"),
        Some(true),
        "alive in the next stage"
    );
}

#[test]
fn commands_apply_in_recording_order() {
    let mut world = World::new();
    let target = world.spawn(Health(1));
    let gone = world.spawn(Health(2));
    let made = Arc::new(Mutex::new(None::<Entity>));
    let m = made.clone();
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PrePhysics,
        System::new("busy", move |ctx| {
            // Spawn, then change the new entity in the same stage, through its reserved handle.
            let new = ctx.commands.spawn(Health(10));
            ctx.commands.insert(new, Armor(3));
            ctx.commands.remove::<Health>(new);
            *m.lock().expect("lock") = Some(new);

            ctx.commands.insert(target, Armor(7));
            // Despawned first, so the insert after it is dropped with a warning, not a panic.
            ctx.commands.despawn(gone);
            ctx.commands.insert(gone, Armor(9));
            ctx.commands.despawn(gone);
            ctx.commands.run(|world| {
                world.spawn(Armor(100));
            });
        }),
    );
    schedule.run(&mut world).expect("valid");
    let new = made.lock().expect("lock").expect("spawned");
    assert_eq!(world.get::<Armor>(new), Some(&Armor(3)));
    assert!(!world.has::<Health>(new));
    assert_eq!(world.get::<Armor>(target), Some(&Armor(7)));
    assert!(!world.contains(gone));
    assert_eq!(world.query::<&Armor>().filter(|a| a.0 == 100).count(), 1);
    assert_eq!(world.len(), 3);
}

#[test]
fn command_spawns_reuse_despawned_slots() {
    let mut world = World::new();
    let old: Vec<Entity> = (0..5).map(|i| world.spawn(Health(i))).collect();
    for &entity in &old {
        world.despawn(entity);
    }
    let made = Arc::new(Mutex::new(Vec::new()));
    let m = made.clone();
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PrePhysics,
        System::new("spawner", move |ctx| {
            // Five reuse the free slots and three are fresh.
            let mut new: Vec<Entity> = (0..8).map(|i| ctx.commands.spawn(Health(i))).collect();
            // A direct spawn in between, from an exclusive command.
            ctx.commands.run(|world| {
                world.spawn(Armor(1));
            });
            new.push(ctx.commands.spawn(Health(99)));
            m.lock().expect("lock").extend(new);
        }),
    );
    schedule.run(&mut world).expect("valid");
    let made = made.lock().expect("lock").clone();
    let mut indices: Vec<u32> = made.iter().map(|e| e.index()).collect();
    indices.sort_unstable();
    indices.dedup();
    assert_eq!(indices.len(), made.len(), "every handle is distinct");
    for (i, &entity) in made.iter().enumerate() {
        assert!(world.contains(entity), "{entity:?}");
        let expected = if i == 8 { 99 } else { i as u32 };
        assert_eq!(world.get::<Health>(entity), Some(&Health(expected)));
    }
    for &entity in &old {
        assert!(!world.contains(entity), "old handles stay dead");
    }
    assert!(
        made.iter().take(5).all(|e| e.index() < 5),
        "the despawned slots were reused"
    );
    assert_eq!(world.len(), 10);
}

#[test]
#[should_panic(expected = "contains the component")]
fn a_duplicate_bundle_panics_when_recorded() {
    let mut world = World::new();
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PrePhysics,
        System::new("bad", |ctx| {
            ctx.commands.spawn((Health(1), Health(2)));
        }),
    );
    let _ = schedule.run(&mut world);
}

#[test]
fn a_panicking_system_discards_its_stage_commands_and_the_schedule_recovers() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicBool, Ordering};

    let mut world = World::new();
    let spawned = Arc::new(Mutex::new(Vec::<Entity>::new()));
    let fail = Arc::new(AtomicBool::new(true));
    let mut schedule = Schedule::new();
    let s = spawned.clone();
    schedule.add(
        Stage::PrePhysics,
        System::new("spawner", move |ctx| {
            let entity = ctx.commands.spawn(Health(1));
            s.lock().expect("lock").push(entity);
        }),
    );
    let f = fail.clone();
    schedule.add(
        Stage::PrePhysics,
        System::new("faulty", move |_| {
            assert!(!f.load(Ordering::Relaxed), "a deliberate test panic");
        })
        .after("spawner"),
    );

    let result = catch_unwind(AssertUnwindSafe(|| schedule.run(&mut world)));
    assert!(result.is_err(), "the system's panic propagates");
    let first = spawned.lock().expect("lock")[0];
    assert!(
        !world.contains(first),
        "the abandoned stage's spawn was discarded"
    );
    assert_eq!(world.len(), 0);

    fail.store(false, Ordering::Relaxed);
    schedule.run(&mut world).expect("the schedule recovers");
    let second = spawned.lock().expect("lock")[1];
    assert!(world.contains(second));
    assert!(
        !world.contains(first),
        "the discarded handle never becomes live"
    );
    assert_ne!(second.index(), first.index(), "its slot isn't reused");
    assert_eq!(world.len(), 1);
}
