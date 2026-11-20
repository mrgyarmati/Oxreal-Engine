//! Behaviour tests of the public API, plus a randomized comparison against a reference model.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{Component, EcsError, Entity, Resource, With, Without, World};

#[derive(Debug, Clone, Copy, PartialEq)]
struct Position(f32);
impl Component for Position {}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Velocity(f32);
impl Component for Velocity {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Health(u32);
impl Component for Health {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Player;
impl Component for Player {}

/// Never added to any entity.
#[derive(Debug, PartialEq)]
struct Unused;
impl Component for Unused {}

#[test]
fn worlds_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<World>();
}

#[test]
fn spawned_entities_have_their_components() {
    let mut world = World::new();
    assert!(world.is_empty());
    let a = world.spawn((Position(1.0), Velocity(2.0)));
    let b = world.spawn(Position(3.0));
    let empty = world.spawn(());
    assert_eq!(world.len(), 3);
    assert_eq!(world.get::<Position>(a), Some(&Position(1.0)));
    assert_eq!(world.get::<Velocity>(a), Some(&Velocity(2.0)));
    assert_eq!(world.get::<Velocity>(b), None);
    assert!(world.has::<Position>(b) && !world.has::<Velocity>(b));
    assert!(world.contains(empty) && !world.has::<Position>(empty));
    assert!(!world.has::<Unused>(a));
}

#[test]
fn nested_bundles_flatten() {
    let mut world = World::new();
    let e = world.spawn(((Position(1.0), Velocity(2.0)), Health(3)));
    assert_eq!(world.get::<Health>(e), Some(&Health(3)));
    assert_eq!(world.get::<Velocity>(e), Some(&Velocity(2.0)));
}

#[test]
fn get_mut_changes_one_entity() {
    let mut world = World::new();
    let a = world.spawn(Health(10));
    let b = world.spawn(Health(20));
    if let Some(mut health) = world.get_mut::<Health>(a) {
        health.0 = 11;
    }
    assert_eq!(world.get::<Health>(a), Some(&Health(11)));
    assert_eq!(world.get::<Health>(b), Some(&Health(20)));
}

#[test]
fn insert_adds_new_components_and_replaces_existing_ones() {
    let mut world = World::new();
    let a = world.spawn(Position(1.0));
    let b = world.spawn(Position(2.0));
    world
        .insert(a, (Velocity(5.0), Position(9.0)))
        .expect("the entity is alive");
    assert_eq!(world.get::<Position>(a), Some(&Position(9.0)));
    assert_eq!(world.get::<Velocity>(a), Some(&Velocity(5.0)));
    // `b` was moved into `a`'s old row when `a` changed archetype.
    assert_eq!(world.get::<Position>(b), Some(&Position(2.0)));

    // Same archetype: replaced in place.
    world.insert(a, Velocity(6.0)).expect("the entity is alive");
    assert_eq!(world.get::<Velocity>(a), Some(&Velocity(6.0)));
}

#[test]
fn remove_returns_the_component_and_keeps_the_rest() {
    let mut world = World::new();
    let a = world.spawn((Position(1.0), Velocity(2.0), Health(3)));
    let b = world.spawn((Position(4.0), Velocity(5.0), Health(6)));
    assert_eq!(world.remove::<Velocity>(a), Some(Velocity(2.0)));
    assert_eq!(world.remove::<Velocity>(a), None);
    assert_eq!(world.remove::<Unused>(a), None);
    assert_eq!(world.get::<Position>(a), Some(&Position(1.0)));
    assert_eq!(world.get::<Health>(a), Some(&Health(3)));
    assert_eq!(world.get::<Velocity>(b), Some(&Velocity(5.0)));
}

#[test]
fn despawned_handles_fail_everywhere() {
    let mut world = World::new();
    let a = world.spawn(Health(1));
    let b = world.spawn(Health(2));
    assert!(world.despawn(a));
    assert!(!world.despawn(a));
    assert!(!world.contains(a));
    assert_eq!(world.get::<Health>(a), None);
    assert!(world.get_mut::<Health>(a).is_none());
    assert!(!world.has::<Health>(a));
    assert_eq!(world.remove::<Health>(a), None);
    assert_eq!(world.insert(a, Health(9)), Err(EcsError::NoSuchEntity(a)));
    assert_eq!(world.get::<Health>(b), Some(&Health(2)));

    // The slot is reused, but the old handle still doesn't reach the new entity.
    let c = world.spawn(Health(3));
    assert_eq!(c.index(), a.index());
    assert_eq!(world.get::<Health>(a), None);
    assert_eq!(world.get::<Health>(c), Some(&Health(3)));
    assert_eq!(world.len(), 2);
}

#[test]
fn errors_name_the_entity_and_the_likely_cause() {
    let mut world = World::new();
    let a = world.spawn(());
    world.despawn(a);
    let message = EcsError::NoSuchEntity(a).to_string();
    assert!(message.contains("Entity(0v1)") && message.contains("despawned"));
}

#[test]
fn queries_visit_every_matching_archetype() {
    let mut world = World::new();
    world.spawn(Position(1.0));
    world.spawn((Position(2.0), Velocity(1.0)));
    world.spawn((Position(3.0), Velocity(1.0), Player));
    world.spawn(Velocity(1.0));

    let mut positions: Vec<f32> = world.query::<&Position>().map(|p| p.0).collect();
    positions.sort_by(f32::total_cmp);
    assert_eq!(positions, [1.0, 2.0, 3.0]);
    assert_eq!(world.query::<(&Position, &Velocity)>().count(), 2);
    assert_eq!(world.query::<Entity>().count(), 4);
    assert_eq!(world.query::<&Unused>().count(), 0);
}

#[test]
fn mutable_queries_write_through() {
    let mut world = World::new();
    let a = world.spawn((Position(0.0), Velocity(2.0)));
    let b = world.spawn((Position(10.0), Velocity(-1.0), Player));
    let still = world.spawn(Position(5.0));
    for (mut position, velocity) in world.query_mut::<(&mut Position, &Velocity)>() {
        position.0 += velocity.0;
    }
    assert_eq!(world.get::<Position>(a), Some(&Position(2.0)));
    assert_eq!(world.get::<Position>(b), Some(&Position(9.0)));
    assert_eq!(world.get::<Position>(still), Some(&Position(5.0)));
}

#[test]
fn entities_and_components_come_out_together() {
    let mut world = World::new();
    let a = world.spawn(Health(1));
    let b = world.spawn((Health(2), Player));
    let mut seen: Vec<(Entity, u32)> = world
        .query::<(Entity, &Health)>()
        .map(|(e, h)| (e, h.0))
        .collect();
    seen.sort();
    assert_eq!(seen, [(a, 1), (b, 2)]);
}

#[test]
fn optional_components_match_every_entity() {
    let mut world = World::new();
    let a = world.spawn((Health(1), Velocity(1.0)));
    let b = world.spawn(Health(2));
    let mut seen: Vec<(Entity, Option<Velocity>)> = world
        .query::<(Entity, Option<&Velocity>)>()
        .map(|(e, v)| (e, v.copied()))
        .collect();
    seen.sort_by_key(|(e, _)| *e);
    assert_eq!(seen, [(a, Some(Velocity(1.0))), (b, None)]);

    for mut velocity in world.query_mut::<Option<&mut Velocity>>().flatten() {
        velocity.0 = 7.0;
    }
    assert_eq!(world.get::<Velocity>(a), Some(&Velocity(7.0)));
    assert_eq!(world.query::<Option<&Unused>>().count(), 2);
}

#[test]
fn filters_select_by_presence() {
    let mut world = World::new();
    world.spawn((Health(1), Player));
    world.spawn(Health(2));
    world.spawn(Player);
    let players: Vec<u32> = world
        .query_filtered::<&Health, With<Player>>()
        .map(|h| h.0)
        .collect();
    assert_eq!(players, [1]);
    let others: Vec<u32> = world
        .query_filtered::<&Health, Without<Player>>()
        .map(|h| h.0)
        .collect();
    assert_eq!(others, [2]);
    assert_eq!(world.query_filtered::<Entity, With<Unused>>().count(), 0);
    assert_eq!(world.query_filtered::<Entity, Without<Unused>>().count(), 3);
    assert_eq!(
        world
            .query_filtered::<Entity, (With<Player>, Without<Health>)>()
            .count(),
        1
    );
    for mut health in world.query_filtered_mut::<&mut Health, With<Player>>() {
        health.0 += 100;
    }
    let mut all: Vec<u32> = world.query::<&Health>().map(|h| h.0).collect();
    all.sort_unstable();
    assert_eq!(all, [2, 101]);
}

#[test]
fn reading_a_component_twice_is_allowed() {
    let mut world = World::new();
    world.spawn(Health(4));
    let pairs: Vec<(u32, u32)> = world
        .query::<(&Health, &Health)>()
        .map(|(a, b)| (a.0, b.0))
        .collect();
    assert_eq!(pairs, [(4, 4)]);
}

#[test]
#[should_panic(expected = "Health mutably and also another way")]
fn writing_and_reading_one_component_in_a_query_panics() {
    let mut world = World::new();
    world.spawn(Health(4));
    let _ = world.query_mut::<(&mut Health, &Health)>().count();
}

#[test]
#[should_panic(expected = "Health mutably and also another way")]
fn writing_one_component_twice_in_a_query_panics() {
    let mut world = World::new();
    let _ = world
        .query_mut::<(&mut Health, Option<&mut Health>)>()
        .count();
}

#[test]
#[should_panic(expected = "contains the component")]
fn a_bundle_with_a_duplicate_component_panics() {
    let mut world = World::new();
    world.spawn((Health(1), Health(2)));
}

#[test]
fn resources_are_typed_singletons() {
    #[derive(Debug, PartialEq)]
    struct Gravity(f32);
    impl Resource for Gravity {}

    let mut world = World::new();
    assert_eq!(world.resource::<Gravity>(), None);
    assert_eq!(world.insert_resource(Gravity(9.8)), None);
    assert_eq!(world.insert_resource(Gravity(1.6)), Some(Gravity(9.8)));
    if let Some(mut gravity) = world.resource_mut::<Gravity>() {
        gravity.0 *= 2.0;
    }
    assert_eq!(world.resource::<Gravity>(), Some(&Gravity(3.2)));
    assert_eq!(world.remove_resource::<Gravity>(), Some(Gravity(3.2)));
    assert_eq!(world.remove_resource::<Gravity>(), None);
}

/// Counts drops, to prove every component value is dropped exactly once.
struct Tracked(Arc<AtomicUsize>);
impl Component for Tracked {}
impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn every_component_value_is_dropped_exactly_once() {
    let drops = Arc::new(AtomicUsize::new(0));
    let tracked = || Tracked(drops.clone());
    let count = || drops.load(Ordering::Relaxed);
    {
        let mut world = World::new();
        let a = world.spawn((tracked(), Health(1)));
        let b = world.spawn(tracked());
        let c = world.spawn((tracked(), Player));

        world.despawn(a);
        assert_eq!(count(), 1, "despawn drops");
        world.insert(b, tracked()).expect("the entity is alive");
        assert_eq!(count(), 2, "replacing drops the old value");
        world.insert(b, Velocity(1.0)).expect("the entity is alive");
        assert_eq!(count(), 2, "moving between archetypes doesn't drop");
        let taken = world.remove::<Tracked>(c);
        assert_eq!(count(), 2, "remove hands the value over");
        drop(taken);
        assert_eq!(count(), 3);
        world.spawn(tracked());
    }
    assert_eq!(count(), 5, "dropping the world drops what's left");
}

/// xorshift64*: a deterministic generator, so a failure always reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        // n is small; the modulo bias is irrelevant for a test.
        (self.next() % n as u64) as usize
    }
}

/// What an entity should have: (Position, Velocity, Health, Player).
type Expected = (Option<Position>, Option<Velocity>, Option<Health>, bool);

fn check(world: &World, model: &HashMap<Entity, Expected>, dead: &[Entity]) {
    assert_eq!(world.len(), model.len());
    for (&entity, &(position, velocity, health, player)) in model {
        assert_eq!(
            world.get::<Position>(entity).copied(),
            position,
            "{entity:?}"
        );
        assert_eq!(
            world.get::<Velocity>(entity).copied(),
            velocity,
            "{entity:?}"
        );
        assert_eq!(world.get::<Health>(entity).copied(), health, "{entity:?}");
        assert_eq!(world.has::<Player>(entity), player, "{entity:?}");
    }
    for &entity in dead {
        assert!(!world.contains(entity), "{entity:?} should be dead");
    }
    let mut queried: Vec<(Entity, u32)> = world
        .query_filtered::<(Entity, &Health), With<Player>>()
        .map(|(e, h)| (e, h.0))
        .collect();
    queried.sort_unstable();
    let mut expected: Vec<(Entity, u32)> = model
        .iter()
        .filter_map(|(&e, &(_, _, health, player))| Some((e, health.filter(|_| player)?.0)))
        .collect();
    expected.sort_unstable();
    assert_eq!(queried, expected);
}

#[test]
fn random_operations_match_a_reference_model() {
    let mut rng = Rng(0x0DDB_1A5E_5BAD_5EED);
    let mut world = World::new();
    let mut model: HashMap<Entity, Expected> = HashMap::new();
    let mut live: Vec<Entity> = Vec::new();
    let mut dead: Vec<Entity> = Vec::new();

    for step in 0..20_000_u32 {
        let value = step as f32;
        match rng.below(8) {
            0 | 1 => {
                let e = match rng.below(3) {
                    0 => world.spawn(Position(value)),
                    1 => world.spawn((Position(value), Velocity(value))),
                    _ => world.spawn((Health(step), Player)),
                };
                let expected = match world.has::<Player>(e) {
                    true => (None, None, Some(Health(step)), true),
                    false if world.has::<Velocity>(e) => {
                        (Some(Position(value)), Some(Velocity(value)), None, false)
                    }
                    false => (Some(Position(value)), None, None, false),
                };
                model.insert(e, expected);
                live.push(e);
            }
            _ if live.is_empty() => {}
            2 => {
                let e = live.swap_remove(rng.below(live.len()));
                assert!(world.despawn(e));
                model.remove(&e);
                dead.push(e);
            }
            3 => {
                let e = live[rng.below(live.len())];
                world.insert(e, Health(step)).expect("the entity is alive");
                model.entry(e).and_modify(|m| m.2 = Some(Health(step)));
            }
            4 => {
                let e = live[rng.below(live.len())];
                world
                    .insert(e, (Velocity(value), Player))
                    .expect("the entity is alive");
                model.entry(e).and_modify(|m| {
                    m.1 = Some(Velocity(value));
                    m.3 = true;
                });
            }
            5 => {
                let e = live[rng.below(live.len())];
                let expected = model.get_mut(&e).and_then(|m| m.0.take());
                assert_eq!(world.remove::<Position>(e), expected);
            }
            6 => {
                let e = live[rng.below(live.len())];
                if world.remove::<Player>(e).is_some() {
                    model.entry(e).and_modify(|m| m.3 = false);
                }
            }
            _ => {
                for mut health in world.query_filtered_mut::<&mut Health, With<Player>>() {
                    health.0 += 1;
                }
                for m in model.values_mut() {
                    if m.3
                        && let Some(health) = &mut m.2
                    {
                        health.0 += 1;
                    }
                }
            }
        }
        if step % 97 == 0 {
            check(&world, &model, &dead);
        }
    }
    check(&world, &model, &dead);
}
