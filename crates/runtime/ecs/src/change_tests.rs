//! Tests of change detection (ADR-0014 §6): `Added`, `Changed`, `Mut` and resource ticks.

use std::sync::{Arc, Mutex};

use crate::change::CHECK_INTERVAL;
use crate::{
    Added, Changed, Commands, Component, Entity, Resource, Schedule, Stage, System, World,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Health(u32);
impl Component for Health {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Armor(u32);
impl Component for Armor {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tag;
impl Component for Tag {}

#[derive(Debug, Default, PartialEq, Eq)]
struct Score(u32);
impl Resource for Score {}

/// What a system saw on each run, shared with the test.
type Seen = Arc<Mutex<Vec<Vec<Entity>>>>;

fn seen() -> Seen {
    Arc::new(Mutex::new(Vec::new()))
}

/// The entities seen on the latest run, sorted.
fn last(seen: &Seen) -> Vec<Entity> {
    let runs = seen.lock().expect("not poisoned");
    let mut last = runs.last().cloned().expect("the system ran");
    last.sort();
    last
}

fn sorted(mut entities: Vec<Entity>) -> Vec<Entity> {
    entities.sort();
    entities
}

/// A system that records the entities whose `Health` changed (or was added) since its last run.
fn changed_health(name: &str, seen: &Seen) -> System {
    let seen = Arc::clone(seen);
    System::new(name, move |ctx| {
        let entities = ctx
            .world
            .query_filtered::<Entity, Changed<Health>>()
            .collect();
        seen.lock().expect("not poisoned").push(entities);
    })
    .reads::<Health>()
}

/// A system that records the entities whose `Health` was added since its last run.
fn added_health(name: &str, seen: &Seen) -> System {
    let seen = Arc::clone(seen);
    System::new(name, move |ctx| {
        let entities = ctx
            .world
            .query_filtered::<Entity, Added<Health>>()
            .collect();
        seen.lock().expect("not poisoned").push(entities);
    })
    .reads::<Health>()
}

fn run(schedule: &mut Schedule, world: &mut World) {
    schedule.run(world).expect("the schedule is valid");
}

#[test]
fn the_first_run_sees_everything_and_later_runs_only_what_is_new() {
    let mut world = World::new();
    let a = world.spawn(Health(1));
    let b = world.spawn((Health(2), Armor(1)));
    let (added, changed) = (seen(), seen());
    let mut schedule = Schedule::new();
    schedule
        .add(Stage::PostUpdate, added_health("added", &added))
        .add(Stage::PostUpdate, changed_health("changed", &changed));

    run(&mut schedule, &mut world);
    assert_eq!(last(&added), sorted(vec![a, b]));
    assert_eq!(last(&changed), sorted(vec![a, b]));

    run(&mut schedule, &mut world);
    assert!(last(&added).is_empty(), "each addition is seen once");
    assert!(last(&changed).is_empty());

    let c = world.spawn(Health(3));
    run(&mut schedule, &mut world);
    assert_eq!(last(&added), [c]);
    assert_eq!(last(&changed), [c], "an addition is a change too");
}

#[test]
fn only_writes_through_mut_count_as_changes() {
    let mut world = World::new();
    let read = world.spawn(Health(1));
    let written = world.spawn(Health(2));
    let changed = seen();
    let mut schedule = Schedule::new();
    schedule.add(Stage::PostUpdate, changed_health("changed", &changed));
    run(&mut schedule, &mut world);

    // Fetched mutably, but only read.
    if let Some(health) = world.get_mut::<Health>(read) {
        assert_eq!(health.0, 1);
    }
    for health in world.query_mut::<&mut Health>() {
        let _ = health.0;
    }
    if let Some(mut health) = world.get_mut::<Health>(written) {
        health.0 += 1;
    }
    run(&mut schedule, &mut world);
    assert_eq!(last(&changed), [written]);
}

#[test]
fn a_system_sees_other_systems_writes_but_never_its_own() {
    let mut world = World::new();
    let e = world.spawn(Health(10));
    let (before, after, writer_saw) = (seen(), seen(), seen());
    let writer_seen = Arc::clone(&writer_saw);
    let mut schedule = Schedule::new();
    schedule
        .add(Stage::PrePhysics, changed_health("before", &before))
        .add(
            Stage::PostPhysics,
            System::new("writer", move |ctx| {
                let entities = ctx
                    .world
                    .query_filtered::<Entity, Changed<Health>>()
                    .collect();
                writer_seen.lock().expect("not poisoned").push(entities);
                for mut health in ctx.world.query_mut::<&mut Health>() {
                    health.0 -= 1;
                }
            })
            .writes::<Health>(),
        )
        .add(Stage::PostUpdate, changed_health("after", &after));

    run(&mut schedule, &mut world);
    run(&mut schedule, &mut world);
    // Second run: the writer's write from the first run is seen by the others, not by itself.
    assert_eq!(
        last(&before),
        [e],
        "an earlier system sees it on its next run"
    );
    assert!(
        last(&writer_saw).is_empty(),
        "the writer doesn't see its own write"
    );
    assert_eq!(last(&after), [e], "a later system sees it in the same run");
}

#[test]
fn mut_queries_can_filter_on_their_own_component() {
    let mut world = World::new();
    let hurt = world.spawn(Health(5));
    world.spawn(Health(9));
    let mut schedule = Schedule::new();
    let healed = seen();
    let healed_seen = Arc::clone(&healed);
    schedule.add(
        Stage::PostUpdate,
        System::new("heal changed", move |ctx| {
            let mut entities = Vec::new();
            for (entity, mut health) in ctx
                .world
                .query_filtered_mut::<(Entity, &mut Health), Changed<Health>>()
            {
                health.0 += 100;
                entities.push(entity);
            }
            healed_seen.lock().expect("not poisoned").push(entities);
        })
        .writes::<Health>(),
    );
    run(&mut schedule, &mut world);
    assert_eq!(last(&healed).len(), 2, "first run: everything");
    run(&mut schedule, &mut world);
    assert!(last(&healed).is_empty(), "its own writes don't feed back");
    if let Some(mut health) = world.get_mut::<Health>(hurt) {
        health.0 = 1;
    }
    run(&mut schedule, &mut world);
    assert_eq!(last(&healed), [hurt]);
    assert_eq!(world.get::<Health>(hurt), Some(&Health(101)));
}

#[test]
fn replacing_a_component_is_a_change_not_an_addition() {
    let mut world = World::new();
    let e = world.spawn(Health(1));
    let (added, changed) = (seen(), seen());
    let mut schedule = Schedule::new();
    schedule
        .add(Stage::PostUpdate, added_health("added", &added))
        .add(Stage::PostUpdate, changed_health("changed", &changed));
    run(&mut schedule, &mut world);

    world.insert(e, Health(2)).expect("alive");
    run(&mut schedule, &mut world);
    assert!(last(&added).is_empty());
    assert_eq!(last(&changed), [e]);

    // Removed and given back: added again.
    world.remove::<Health>(e);
    world.insert(e, Health(3)).expect("alive");
    run(&mut schedule, &mut world);
    assert_eq!(last(&added), [e]);
}

#[test]
fn moving_between_archetypes_keeps_ticks() {
    let mut world = World::new();
    let moved = world.spawn((Health(1), Armor(1)));
    let other = world.spawn((Health(2), Armor(2)));
    let last_row = world.spawn((Health(3), Armor(3)));
    let changed = seen();
    let mut schedule = Schedule::new();
    schedule.add(Stage::PostUpdate, changed_health("changed", &changed));
    run(&mut schedule, &mut world);

    // A changed value keeps its tick through moves; unchanged ones stay unchanged.
    if let Some(mut health) = world.get_mut::<Health>(other) {
        health.0 = 20;
    }
    world.insert(moved, Tag).expect("alive"); // to another archetype
    world.remove::<Armor>(other); // it moves too, carrying its change
    assert!(world.despawn(moved)); // the last row fills its slot
    run(&mut schedule, &mut world);
    assert_eq!(last(&changed), [other]);
    assert!(world.contains(last_row));
}

#[test]
fn optional_mut_and_modify_stamp_writes() {
    let mut world = World::new();
    let a = world.spawn(Health(1));
    let b = world.spawn(Armor(1));
    let c = world.spawn(Health(3));
    let changed = seen();
    let mut schedule = Schedule::new();
    schedule.add(Stage::PostUpdate, changed_health("changed", &changed));
    run(&mut schedule, &mut world);

    for (entity, health) in world.query_mut::<(Entity, Option<&mut Health>)>() {
        if entity == a
            && let Some(mut health) = health
        {
            health.0 = 10;
        }
    }
    let _ = b;
    world.modify::<Health, _>(c, |mut health| health.0 = 30);
    run(&mut schedule, &mut world);
    assert_eq!(last(&changed), sorted(vec![a, c]));
}

#[test]
fn commands_applied_at_a_sync_point_are_seen_by_later_stages() {
    let mut world = World::new();
    let e = world.spawn(Armor(1));
    let added = seen();
    let mut schedule = Schedule::new();
    schedule
        .add(
            Stage::PrePhysics,
            System::new("give health", move |ctx| {
                let commands: &mut Commands<'_> = &mut ctx.commands;
                commands.insert(e, Health(5));
            }),
        )
        .add(Stage::PostUpdate, added_health("added", &added));
    run(&mut schedule, &mut world);
    assert_eq!(last(&added), [e]);
}

#[test]
fn queries_on_the_world_have_no_previous_run_so_everything_counts() {
    let mut world = World::new();
    let a = world.spawn(Health(1));
    let _ = world.spawn(Armor(1));
    assert_eq!(
        world
            .query_filtered::<Entity, Changed<Health>>()
            .collect::<Vec<_>>(),
        [a]
    );
    assert_eq!(
        world
            .query_filtered::<Entity, Added<Health>>()
            .collect::<Vec<_>>(),
        [a]
    );
    assert_eq!(
        world
            .query_filtered::<Entity, (Changed<Health>, Added<Armor>)>()
            .count(),
        0,
        "every filter of a tuple must hold"
    );
}

#[test]
#[should_panic(expected = "reads component")]
fn filtering_on_an_undeclared_component_panics() {
    let mut world = World::new();
    world.spawn(Health(1));
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PostUpdate,
        System::new("sneaky", |ctx| {
            let _ = ctx
                .world
                .query_filtered::<Entity, Changed<Health>>()
                .count();
        }),
    );
    run(&mut schedule, &mut world);
}

#[test]
fn resources_report_additions_and_changes() {
    let mut world = World::new();
    world.insert_resource(Score(0));
    let reports: Arc<Mutex<Vec<(bool, bool)>>> = Arc::default();
    let seen_reports = Arc::clone(&reports);
    let mut schedule = Schedule::new();
    schedule
        .add(
            Stage::PrePhysics,
            System::new("watch", move |ctx| {
                let report = (
                    ctx.resources.is_added::<Score>(),
                    ctx.resources.is_changed::<Score>(),
                );
                seen_reports.lock().expect("not poisoned").push(report);
            })
            .reads_resource::<Score>(),
        )
        .add(
            Stage::PostUpdate,
            System::new("own writes", |ctx| {
                // Its own write never shows to itself, but "watch" sees it next run.
                if !ctx.resources.is_changed::<Score>()
                    && let Some(mut score) = ctx.resources.get_mut::<Score>()
                {
                    score.0 += 1;
                }
            })
            .writes_resource::<Score>(),
        );
    let report = |reports: &Arc<Mutex<Vec<(bool, bool)>>>| {
        *reports.lock().expect("not poisoned").last().expect("ran")
    };

    run(&mut schedule, &mut world);
    assert_eq!(report(&reports), (true, true), "first run");
    run(&mut schedule, &mut world);
    assert_eq!(
        report(&reports),
        (false, false),
        "\"own writes\" didn't write: it saw a change"
    );
    run(&mut schedule, &mut world);
    assert_eq!(
        report(&reports),
        (false, true),
        "\"own writes\" wrote last run"
    );
    world.insert_resource(Score(7));
    run(&mut schedule, &mut world);
    assert_eq!(
        report(&reports),
        (false, true),
        "replacing is a change, not an addition"
    );
}

#[test]
fn change_detection_works_across_the_32_bit_wrap() {
    let mut world = World::new();
    world.set_tick_count(u64::from(u32::MAX) - 3);
    let e = world.spawn(Health(1));
    let changed = seen();
    let mut schedule = Schedule::new();
    schedule.add(Stage::PostUpdate, changed_health("changed", &changed));
    for frame in 0..8_u32 {
        if frame % 2 == 1
            && let Some(mut health) = world.get_mut::<Health>(e)
        {
            health.0 += 1;
        }
        run(&mut schedule, &mut world);
        let expect: &[Entity] = if frame == 0 || frame % 2 == 1 {
            &[e]
        } else {
            &[]
        };
        assert_eq!(last(&changed), expect, "frame {frame}");
    }
}

/// Moves the world's tick count past the 32-bit horizon in realistic steps: at most
/// [`CHECK_INTERVAL`] at a time, each followed by a system run of `other` (as if other schedules
/// kept running), which is where clamping happens. Before each step, `each` runs.
fn tick_past_the_horizon(
    world: &mut World,
    other: &mut Schedule,
    mut each: impl FnMut(&mut World),
) {
    let target = (1_u64 << 32) + 3 * u64::from(CHECK_INTERVAL);
    while world.tick_count() < target {
        each(world);
        let next = world.tick_count() + u64::from(CHECK_INTERVAL);
        world.set_tick_count(next);
        run(other, world);
    }
}

fn idle_schedule() -> Schedule {
    let mut other = Schedule::new();
    other.add(Stage::PostUpdate, System::new("elsewhere", |_| {}));
    other
}

#[test]
fn an_old_change_never_looks_new_again_across_the_32_bit_horizon() {
    let mut world = World::new();
    let e = world.spawn(Health(1));
    let changed = seen();
    let mut watcher = Schedule::new();
    watcher.add(Stage::PostUpdate, changed_health("changed", &changed));
    run(&mut watcher, &mut world);
    assert_eq!(last(&changed), [e]);

    // Without clamping, the value's 32-bit tick would wrap around to look recent again.
    let mut other = idle_schedule();
    tick_past_the_horizon(&mut world, &mut other, |world| {
        run(&mut watcher, world);
        assert!(last(&changed).is_empty(), "nothing changed");
    });
}

#[test]
fn a_system_idle_across_the_32_bit_horizon_still_sees_what_changed() {
    let mut world = World::new();
    let changed_since = world.spawn(Health(1));
    let changed = seen();
    let mut watcher = Schedule::new();
    watcher.add(Stage::PostUpdate, changed_health("changed", &changed));
    run(&mut watcher, &mut world);
    if let Some(mut health) = world.get_mut::<Health>(changed_since) {
        health.0 = 2;
    }

    // The watcher doesn't run for more than 2³² ticks. Its previous run is remembered as a
    // 64-bit count, so it isn't mistaken for a recent one.
    let mut other = idle_schedule();
    tick_past_the_horizon(&mut world, &mut other, |_| {});
    run(&mut watcher, &mut world);
    assert!(
        last(&changed).contains(&changed_since),
        "the change made while it was idle is seen"
    );

    // Once caught up, only real changes count again.
    run(&mut watcher, &mut world);
    assert!(last(&changed).is_empty());
    let fresh = world.spawn(Health(3));
    run(&mut watcher, &mut world);
    assert_eq!(last(&changed), [fresh]);
}
