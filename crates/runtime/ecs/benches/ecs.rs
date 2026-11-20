//! The ECS benchmark suite (ADR-0014 §10).
//!
//! ```text
//! cargo bench -p oxreal-ecs
//! ```
//!
//! Each workload runs several times after a warm-up, and the median is reported. Use the results
//! as evidence for storage changes, and record them with the hardware they ran on (PRD §9).
//! Transform propagation through a hierarchy, the fifth ADR-0014 workload, arrives with
//! relations.

#![expect(clippy::print_stdout, reason = "a benchmark reports on stdout")]

use std::hint::black_box;
use std::time::{Duration, Instant};

use oxreal_ecs::{Component, Entity, World};

#[derive(Clone, Copy)]
struct Position([f32; 3]);
impl Component for Position {}

#[derive(Clone, Copy)]
struct Velocity([f32; 3]);
impl Component for Velocity {}

/// Tags that put entities into different archetypes.
struct Tag<const N: usize>;
impl<const N: usize> Component for Tag<N> {}

/// A tag toggled every frame in the churn workload.
struct Toggled;
impl Component for Toggled {}

const ENTITIES: usize = 100_000;
const RUNS: usize = 11;

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// Runs `setup` then `work` `RUNS` times (plus a warm-up) and prints the median of `work`.
fn bench<S>(name: &str, mut setup: impl FnMut() -> S, mut work: impl FnMut(&mut S)) {
    let mut samples = Vec::with_capacity(RUNS);
    for run in 0..=RUNS {
        let mut state = setup();
        let start = Instant::now();
        work(&mut state);
        let elapsed = start.elapsed();
        black_box(&state);
        if run > 0 {
            samples.push(elapsed);
        }
    }
    let median = median(samples);
    println!("{name:<52} {:>10.3} ms", median.as_secs_f64() * 1e3);
}

fn moving(i: usize) -> (Position, Velocity) {
    let f = i as f32;
    (Position([f, 0.0, 0.0]), Velocity([1.0, 0.5, f.sin()]))
}

/// A world of `ENTITIES` moving entities spread evenly over `archetypes` archetypes.
fn spread_world(archetypes: usize) -> World {
    let mut world = World::new();
    for i in 0..ENTITIES {
        let entity = world.spawn(moving(i));
        let mask = i % archetypes;
        // Seven tag bits give up to 128 archetypes.
        macro_rules! tag {
            ($($bit:literal),*) => {$(
                if mask & (1 << $bit) != 0 {
                    world.insert(entity, Tag::<$bit>).expect("the entity is alive");
                }
            )*};
        }
        tag!(0, 1, 2, 3, 4, 5, 6);
    }
    world
}

fn integrate(world: &mut World) {
    for (mut position, velocity) in world.query_mut::<(&mut Position, &Velocity)>() {
        for axis in 0..3 {
            position.0[axis] += velocity.0[axis] * (1.0 / 60.0);
        }
    }
}

fn main() {
    println!("oxreal-ecs benchmarks: median of {RUNS} runs, {ENTITIES} entities unless noted");
    println!(
        "{} {} build, {} logical CPUs",
        std::env::consts::OS,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        std::thread::available_parallelism().map_or(0, usize::from)
    );
    println!();

    bench("spawn 100k (Position, Velocity)", World::new, |world| {
        for i in 0..ENTITIES {
            world.spawn(moving(i));
        }
    });

    bench(
        "despawn 100k",
        || {
            let mut world = World::new();
            let entities: Vec<Entity> = (0..ENTITIES).map(|i| world.spawn(moving(i))).collect();
            (world, entities)
        },
        |(world, entities)| {
            for &entity in entities.iter() {
                world.despawn(entity);
            }
        },
    );

    for archetypes in [1, 10, 100] {
        bench(
            &format!("iterate (&mut Position, &Velocity), {archetypes} archetype(s)"),
            || spread_world(archetypes),
            integrate,
        );
    }

    bench(
        "tag churn: insert + remove a tag on 10k entities",
        || {
            let mut world = World::new();
            let entities: Vec<Entity> = (0..ENTITIES).map(|i| world.spawn(moving(i))).collect();
            (world, entities)
        },
        |(world, entities)| {
            for &entity in entities.iter().step_by(10) {
                world.insert(entity, Toggled).expect("the entity is alive");
            }
            for &entity in entities.iter().step_by(10) {
                black_box(world.remove::<Toggled>(entity));
            }
        },
    );

    bench(
        "random access: get::<Position> on 100k shuffled handles",
        || {
            let mut world = spread_world(10);
            let mut entities: Vec<Entity> = world.query::<Entity>().collect();
            // A fixed permutation, so every run touches memory in the same scattered order.
            let len = entities.len();
            for i in 0..len {
                entities.swap(i, (i * 7919 + 13) % len);
            }
            world.spawn(());
            (world, entities)
        },
        |(world, entities)| {
            let mut sum = 0.0;
            for &entity in entities.iter() {
                if let Some(position) = world.get::<Position>(entity) {
                    sum += position.0[0];
                }
            }
            black_box(sum);
        },
    );
}
