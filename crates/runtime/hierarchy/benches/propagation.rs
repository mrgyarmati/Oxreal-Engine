//! Transform propagation benchmark (ADR-0014 §10).
//!
//! ```text
//! cargo bench -p oxreal-hierarchy
//! ```
//!
//! 111,100 entities in trees four levels deep: 100 roots, each with 10 children, each with 10
//! children, each with 10 children. Reports the median of several runs after a warm-up. Record
//! results with the hardware they ran on (PRD §9).

#![expect(clippy::print_stdout, reason = "a benchmark reports on stdout")]

use std::hint::black_box;
use std::time::{Duration, Instant};

use oxreal_ecs::{Schedule, World};
use oxreal_hierarchy::{ChildOf, LocalTransform, WorldTransform, add_systems};
use oxreal_math::{Quat, Transform, Vec3};

const RUNS: usize = 11;

fn local(i: usize) -> LocalTransform {
    let f = i as f32;
    LocalTransform(
        Transform::from_translation(Vec3::new(f.sin(), 1.0, f.cos()))
            .with_rotation(Quat::from_rotation_y(f * 0.01)),
    )
}

fn build() -> World {
    let mut world = World::new();
    let mut level: Vec<_> = (0..100).map(|i| world.spawn(local(i))).collect();
    for _ in 0..3 {
        let mut next = Vec::with_capacity(level.len() * 10);
        for (i, &parent) in level.iter().enumerate() {
            for j in 0..10 {
                next.push(world.spawn((local(i * 10 + j), ChildOf::new(parent))));
            }
        }
        level = next;
    }
    world
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn main() {
    let start = Instant::now();
    let mut world = build();
    let built = start.elapsed();
    let mut schedule = Schedule::new();
    add_systems(&mut schedule);

    let mut samples = Vec::with_capacity(RUNS);
    for run in 0..=RUNS {
        let start = Instant::now();
        schedule.run(&mut world).expect("the schedule is valid");
        if run > 0 {
            samples.push(start.elapsed());
        }
    }
    let sum: f32 = world
        .query::<&WorldTransform>()
        .map(|t| t.translation().y)
        .sum();
    black_box(sum);

    println!("oxreal-hierarchy benchmarks: median of {RUNS} runs");
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
    println!(
        "{:<52} {:>10.3} ms",
        "build 111,100 entities, 4 levels (spawn + attach)",
        built.as_secs_f64() * 1e3
    );
    println!(
        "{:<52} {:>10.3} ms",
        "propagate transforms, 111,100 entities, 4 levels",
        median(samples).as_secs_f64() * 1e3
    );

    // Wide fan-out: one parent with many children. Each attach replaces the parent's immutable
    // `Children`, so the cost grows with the number of siblings (ADR-0014 notes).
    for wide in [1_000, 10_000] {
        let mut world = World::new();
        let parent = world.spawn(local(0));
        let kids: Vec<_> = (0..wide).map(|i| world.spawn(local(i))).collect();
        let start = Instant::now();
        for &kid in &kids {
            world
                .insert(kid, ChildOf::new(parent))
                .expect("the child is alive");
        }
        let attached = start.elapsed();
        let start = Instant::now();
        world.despawn(parent);
        let detached = start.elapsed();
        black_box(&world);
        println!(
            "{:<52} {:>10.3} ms",
            format!("attach {wide} children to one parent, one by one"),
            attached.as_secs_f64() * 1e3
        );
        println!(
            "{:<52} {:>10.3} ms",
            format!("despawn that parent (its {wide} children detach)"),
            detached.as_secs_f64() * 1e3
        );
    }
}
