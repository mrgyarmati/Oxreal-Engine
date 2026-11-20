//! Tests of relations and transform propagation.

use std::sync::{Arc, Mutex};

use oxreal_ecs::{
    Component, ComponentHooks, EcsError, Entity, HookCause, Resource, Schedule, Stage, System,
    World,
};
use oxreal_math::{Affine3A, Quat, Transform, Vec3};

use crate::transform::world_placement;
use crate::{
    ChildOf, Children, HierarchyError, LocalTransform, OnParentDespawn, TransformRule,
    WorldTransform, add_systems, remove_parent, set_parent,
};

const KEEP_LOCAL: TransformRule = TransformRule::KeepLocal;
const KEEP_WORLD: TransformRule = TransformRule::KeepWorld;

fn children(world: &World, parent: Entity) -> Vec<Entity> {
    world
        .get::<Children>(parent)
        .map(|c| c.as_slice().to_vec())
        .unwrap_or_default()
}

fn parent(world: &World, child: Entity) -> Option<Entity> {
    world.get::<ChildOf>(child).map(ChildOf::parent)
}

fn at(x: f32, y: f32, z: f32) -> LocalTransform {
    LocalTransform(Transform::from_translation(Vec3::new(x, y, z)))
}

fn local(world: &World, entity: Entity) -> Option<Transform> {
    world.get::<LocalTransform>(entity).map(|l| l.0)
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 1e-4
}

#[test]
fn children_follow_child_of_in_order() {
    let mut world = World::new();
    let p = world.spawn(());
    let a = world.spawn(ChildOf::new(p));
    let b = world.spawn(ChildOf::new(p));
    let c = world.spawn(());
    world.insert(c, ChildOf::new(p)).expect("alive");
    assert_eq!(children(&world, p), [a, b, c]);
    assert_eq!(parent(&world, b), Some(p));
    assert_eq!(world.get::<Children>(p).map(Children::len), Some(3));
    assert_eq!(
        world.get::<ChildOf>(a).map(ChildOf::on_parent_despawn),
        Some(OnParentDespawn::Detach),
        "detaching is the default"
    );
}

#[test]
fn moving_and_detaching_keep_both_sides_consistent() {
    let mut world = World::new();
    let p = world.spawn(());
    let q = world.spawn(());
    let a = world.spawn(ChildOf::new(p));
    let b = world.spawn(ChildOf::new(p));

    set_parent(&mut world, a, q, KEEP_LOCAL).expect("valid");
    assert_eq!(children(&world, p), [b]);
    assert_eq!(children(&world, q), [a]);

    assert_eq!(remove_parent(&mut world, b, KEEP_LOCAL), Some(p));
    assert!(!world.has::<Children>(p), "an empty Children is removed");
    assert_eq!(parent(&world, b), None);
    assert_eq!(remove_parent(&mut world, b, KEEP_LOCAL), None);
}

#[test]
fn setting_the_same_parent_again_keeps_the_order() {
    let mut world = World::new();
    let p = world.spawn(());
    let a = world.spawn(ChildOf::new(p));
    let b = world.spawn(ChildOf::new(p));
    set_parent(&mut world, a, p, KEEP_WORLD).expect("valid");
    assert_eq!(children(&world, p), [a, b]);
}

#[test]
fn moving_keeps_the_links_despawn_policy() {
    let mut world = World::new();
    let p = world.spawn(());
    let q = world.spawn(());
    let a = world.spawn(ChildOf::new(p).despawn_with_parent());
    set_parent(&mut world, a, q, KEEP_LOCAL).expect("valid");
    assert_eq!(
        world.get::<ChildOf>(a).map(ChildOf::on_parent_despawn),
        Some(OnParentDespawn::Despawn)
    );
}

#[test]
fn set_parent_refuses_cycles_and_unusable_parents_and_explains() {
    let mut world = World::new();
    let root = world.spawn(());
    let mid = world.spawn(ChildOf::new(root));
    let leaf = world.spawn(ChildOf::new(mid));

    let cycle = set_parent(&mut world, root, leaf, KEEP_LOCAL);
    assert_eq!(
        cycle,
        Err(HierarchyError::Cycle {
            child: root,
            parent: leaf
        })
    );
    let message = cycle.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(message.contains("Detach it first"), "{message}");
    assert!(matches!(
        set_parent(&mut world, mid, mid, KEEP_LOCAL),
        Err(HierarchyError::Cycle { .. })
    ));

    let dead = world.spawn(());
    world.despawn(dead);
    let error = set_parent(&mut world, leaf, dead, KEEP_LOCAL).expect_err("dead parent");
    assert_eq!(
        error,
        HierarchyError::Ecs {
            child: leaf,
            parent: dead,
            source: EcsError::NoSuchEntity(dead)
        }
    );
    // The message says what failed, and the source says why, without repeating it.
    let shown = error.to_string();
    assert!(
        shown.contains("can't make") && shown.contains("Check that both are alive"),
        "{shown}"
    );
    assert!(
        !shown.contains("doesn't exist"),
        "the source isn't repeated: {shown}"
    );
    assert!(std::error::Error::source(&error).is_some());

    // Nothing changed.
    assert_eq!(children(&world, root), [mid]);
    assert_eq!(children(&world, mid), [leaf]);
    assert_eq!(parent(&world, root), None);
}

#[test]
fn hooks_refuse_cycles_and_dead_parents_inserted_directly() {
    let mut world = World::new();
    let root = world.spawn(());
    let leaf = world.spawn(ChildOf::new(root));
    world.insert(root, ChildOf::new(leaf)).expect("alive");
    assert_eq!(parent(&world, root), None, "refused rather than a cycle");
    assert_eq!(children(&world, leaf), Vec::<Entity>::new());
    assert_eq!(children(&world, root), [leaf]);

    let dead = world.spawn(());
    world.despawn(dead);
    let orphan = world.spawn(ChildOf::new(dead));
    assert_eq!(parent(&world, orphan), None);
}

#[test]
fn by_default_a_despawned_parents_children_detach_and_stay_put() {
    let mut world = World::new();
    let p = world.spawn(LocalTransform(
        Transform::from_translation(Vec3::new(10.0, 0.0, 0.0))
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
    ));
    let a = world.spawn((at(0.0, 0.0, -2.0), ChildOf::new(p)));
    let a1 = world.spawn((at(0.0, 1.0, 0.0), ChildOf::new(a)));
    let a_world = world_placement(&world, a).translation;
    let a1_world = world_placement(&world, a1).translation;

    assert!(world.despawn(p));
    assert!(world.contains(a) && world.contains(a1));
    assert_eq!(parent(&world, a), None, "a is a root now");
    assert_eq!(
        parent(&world, a1),
        Some(a),
        "its own children stay attached"
    );
    let a_local = local(&world, a).map(|t| t.translation).unwrap_or(Vec3::NAN);
    assert!(near(a_local, a_world.into()), "{a_local} != {a_world}");
    assert!(near(
        world_placement(&world, a1).translation.into(),
        a1_world.into()
    ));
}

/// Records, when dropped, the order in which entities were removed.
struct Removal {
    name: &'static str,
    order: Arc<Mutex<Vec<&'static str>>>,
}
impl Component for Removal {}
impl Drop for Removal {
    fn drop(&mut self) {
        if let Ok(mut order) = self.order.lock() {
            order.push(self.name);
        }
    }
}

#[test]
fn opted_in_children_are_despawned_first_and_others_detach() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let tag = |name| Removal {
        name,
        order: order.clone(),
    };
    let mut world = World::new();
    let root = world.spawn(tag("root"));
    let part = world.spawn((tag("part"), ChildOf::new(root).despawn_with_parent()));
    let sub = world.spawn((tag("sub"), ChildOf::new(part).despawn_with_parent()));
    let kept = world.spawn((tag("kept"), ChildOf::new(root)));
    let kept_child = world.spawn((tag("kept child"), ChildOf::new(kept).despawn_with_parent()));

    assert!(world.despawn(root));
    assert_eq!(*order.lock().expect("lock"), ["sub", "part", "root"]);
    assert!(!world.contains(part) && !world.contains(sub));
    assert!(world.contains(kept) && world.contains(kept_child));
    assert_eq!(parent(&world, kept), None);
    assert_eq!(parent(&world, kept_child), Some(kept));
}

#[test]
fn despawning_a_child_removes_it_from_its_parent() {
    let mut world = World::new();
    let p = world.spawn(());
    let a = world.spawn(ChildOf::new(p));
    let b = world.spawn(ChildOf::new(p));
    world.despawn(a);
    assert_eq!(children(&world, p), [b]);
    world.despawn(b);
    assert!(!world.has::<Children>(p));
    assert!(world.contains(p));
}

#[test]
fn removing_children_detaches_and_keeps_world_placement() {
    let mut world = World::new();
    let p = world.spawn(at(5.0, 0.0, 0.0));
    let a = world.spawn((at(0.0, 1.0, 0.0), ChildOf::new(p).despawn_with_parent()));
    assert!(world.remove::<Children>(p).is_some());
    assert!(world.contains(a), "removing Children never despawns");
    assert_eq!(parent(&world, a), None);
    assert!(near(
        local(&world, a).map(|t| t.translation).unwrap_or(Vec3::NAN),
        Vec3::new(5.0, 1.0, 0.0)
    ));
}

#[test]
fn a_children_list_moved_or_put_back_is_repaired() {
    let mut world = World::new();
    let p = world.spawn(());
    let q = world.spawn(());
    let a = world.spawn(ChildOf::new(p));
    let b = world.spawn(ChildOf::new(p));

    // Taken off `p` (which detaches a and b), then put on `q`: q has no such children.
    let stolen = world.remove::<Children>(p).expect("p had children");
    world.insert(q, stolen).expect("alive");
    assert!(!world.has::<Children>(q));

    // Taken off and put back on the same entity after its children were detached.
    world.insert(a, ChildOf::new(p)).expect("alive");
    let stale = world.remove::<Children>(p).expect("p had a child");
    world.insert(b, ChildOf::new(p)).expect("alive");
    world.insert(p, stale).expect("alive");
    assert_eq!(children(&world, p), [b], "only real links survive");

    // Taken off and put straight back: removal detached the children, so the list is stale
    // even though nothing else changed in between.
    let list = world.remove::<Children>(p).expect("p had a child");
    assert_eq!(parent(&world, b), None, "removing Children detached b");
    world.insert(p, list).expect("alive");
    assert!(
        !world.has::<Children>(p),
        "the stale list was rebuilt from links: none left"
    );
}

/// When its entity is despawned, queues commands that try to attach a new child to its dying
/// parent: the attach must be refused, leaving no link to a dead entity.
#[derive(Debug, Clone, Copy)]
struct Clinger {
    parent: Entity,
    newcomer: Entity,
}
impl Component for Clinger {
    const HOOKS: ComponentHooks = ComponentHooks::NONE.on_remove(|w, ctx| {
        if ctx.cause == HookCause::Despawn
            && let Some(c) = w.get::<Clinger>(ctx.entity).copied()
        {
            w.commands().run(move |world| {
                let result = set_parent(world, c.newcomer, c.parent, KEEP_LOCAL);
                assert!(
                    matches!(
                        result,
                        Err(HierarchyError::Ecs {
                            source: EcsError::Despawning(_),
                            ..
                        })
                    ),
                    "{result:?}"
                );
                // Inserted directly, the hook refuses it.
                world
                    .insert(c.newcomer, ChildOf::new(c.parent))
                    .expect("alive");
            });
        }
    });
}

#[test]
fn nothing_attaches_to_a_despawning_parent() {
    let mut world = World::new();
    let p = world.spawn(());
    let newcomer = world.spawn(());
    world.spawn((
        Clinger {
            parent: p,
            newcomer,
        },
        ChildOf::new(p).despawn_with_parent(),
    ));
    world.despawn(p);
    assert!(!world.contains(p));
    assert_eq!(parent(&world, newcomer), None, "no link to a dead parent");
}

/// When despawned, moves another child of the same parent elsewhere.
#[derive(Debug, Clone, Copy)]
struct Rescuer {
    sibling: Entity,
    shelter: Entity,
}
impl Component for Rescuer {
    const HOOKS: ComponentHooks = ComponentHooks::NONE.on_remove(|w, ctx| {
        if let Some(r) = w.get::<Rescuer>(ctx.entity).copied() {
            w.commands().run(move |world| {
                set_parent(world, r.sibling, r.shelter, KEEP_LOCAL).expect("valid");
            });
        }
    });
}

#[test]
fn a_child_moved_away_during_the_cascade_is_spared() {
    let mut world = World::new();
    let p = world.spawn(());
    let shelter = world.spawn(());
    let first = world.spawn(ChildOf::new(p).despawn_with_parent());
    let sibling = world.spawn(ChildOf::new(p).despawn_with_parent());
    world
        .insert(first, Rescuer { sibling, shelter })
        .expect("alive");
    world.despawn(p);
    assert!(!world.contains(first));
    assert!(world.contains(sibling), "moved out before its despawn ran");
    assert_eq!(parent(&world, sibling), Some(shelter));
}

#[test]
fn removing_local_transform_removes_the_world_transform() {
    let mut world = World::new();
    let e = world.spawn(at(1.0, 2.0, 3.0));
    assert!(world.has::<WorldTransform>(e));
    world.remove::<LocalTransform>(e);
    assert!(
        !world.has::<WorldTransform>(e),
        "nothing left to draw at a stale place"
    );
}

fn affine(t: &Transform) -> Affine3A {
    Affine3A::from_scale_rotation_translation(t.scale, t.rotation, t.translation)
}

fn run_post_update(world: &mut World) {
    let mut schedule = Schedule::new();
    add_systems(&mut schedule);
    schedule.run(world).expect("valid");
}

#[test]
fn keep_world_and_keep_local_attach_differently() {
    let mut world = World::new();
    let p = world.spawn(LocalTransform(
        Transform::from_translation(Vec3::new(10.0, 0.0, 0.0)).with_scale(Vec3::splat(2.0)),
    ));
    let a = world.spawn(at(1.0, 0.0, 0.0));
    let b = world.spawn(at(1.0, 0.0, 0.0));
    set_parent(&mut world, a, p, KEEP_WORLD).expect("valid");
    set_parent(&mut world, b, p, KEEP_LOCAL).expect("valid");
    run_post_update(&mut world);
    let a_world = world
        .get::<WorldTransform>(a)
        .map(WorldTransform::translation);
    let b_world = world
        .get::<WorldTransform>(b)
        .map(WorldTransform::translation);
    assert!(
        a_world.is_some_and(|t| near(t, Vec3::new(1.0, 0.0, 0.0))),
        "{a_world:?}"
    );
    assert!(
        b_world.is_some_and(|t| near(t, Vec3::new(12.0, 0.0, 0.0))),
        "{b_world:?}"
    );

    remove_parent(&mut world, b, KEEP_WORLD);
    run_post_update(&mut world);
    let b_world = world
        .get::<WorldTransform>(b)
        .map(WorldTransform::translation);
    assert!(
        b_world.is_some_and(|t| near(t, Vec3::new(12.0, 0.0, 0.0))),
        "{b_world:?}"
    );
}

#[test]
fn world_transforms_are_parent_times_local_down_the_tree() {
    let mut world = World::new();
    let t_root = Transform::from_translation(Vec3::new(5.0, 0.0, 0.0))
        .with_rotation(Quat::from_rotation_y(0.7));
    // Non-uniform scale under a rotated child: a Transform product would be wrong here.
    let t_mid =
        Transform::from_translation(Vec3::new(0.0, 1.0, 0.0)).with_scale(Vec3::new(2.0, 1.0, 0.5));
    let t_leaf = Transform::from_translation(Vec3::new(1.0, 0.0, 0.0))
        .with_rotation(Quat::from_rotation_z(0.4));
    let root = world.spawn(LocalTransform(t_root));
    let mid = world.spawn((LocalTransform(t_mid), ChildOf::new(root)));
    // No LocalTransform: passes its parent's world transform on.
    let pass = world.spawn(ChildOf::new(mid));
    let leaf = world.spawn((LocalTransform(t_leaf), ChildOf::new(pass)));
    assert!(
        world.has::<WorldTransform>(leaf),
        "added with LocalTransform"
    );
    assert!(!world.has::<WorldTransform>(pass));

    run_post_update(&mut world);
    let expected = affine(&t_root) * affine(&t_mid) * affine(&t_leaf);
    let got = world
        .get::<WorldTransform>(leaf)
        .map(WorldTransform::affine);
    assert!(
        got.is_some_and(|g| g.abs_diff_eq(expected, 1e-5)),
        "{got:?} != {expected:?}"
    );
    assert!(world_placement(&world, leaf).abs_diff_eq(expected, 1e-5));
}

/// A marker, to tell entities apart in the randomized tests.
#[derive(Debug, Clone, Copy)]
struct Node;
impl Component for Node {}

/// When despawned, meddles: re-parents or despawns another entity, chosen when it was made.
#[derive(Debug, Clone, Copy)]
struct Reactor {
    target: Entity,
    other: Entity,
    despawn: bool,
}
impl Component for Reactor {
    const HOOKS: ComponentHooks = ComponentHooks::NONE.on_remove(|w, ctx| {
        if let Some(r) = w.get::<Reactor>(ctx.entity).copied() {
            w.commands().run(move |world| {
                if r.despawn {
                    world.despawn(r.target);
                } else if world.contains(r.target) && world.contains(r.other) {
                    // A refusal (cycle, dead or despawning parent) is fine: it's checked
                    // against the invariants all the same.
                    let _ = set_parent(world, r.target, r.other, KEEP_WORLD);
                }
            });
        }
    });
}

/// Every hierarchy invariant, plus world transforms against a recursive reference.
fn check(world: &World, context: &str) {
    let nodes: Vec<Entity> = world.query::<Entity>().collect();
    for &e in &nodes {
        if let Some(p) = parent(world, e) {
            assert!(world.contains(p), "{context}: {e:?}'s parent {p:?} is dead");
            let siblings = children(world, p);
            assert_eq!(
                siblings.iter().filter(|&&s| s == e).count(),
                1,
                "{context}: {e:?} is listed once under {p:?}: {siblings:?}"
            );
            let mut current = e;
            for _ in 0..=nodes.len() {
                match parent(world, current) {
                    Some(up) => current = up,
                    None => break,
                }
            }
            assert_eq!(parent(world, current), None, "{context}: cycle above {e:?}");
        }
        if let Some(kids) = world.get::<Children>(e) {
            assert!(!kids.is_empty(), "{context}: {e:?} has an empty Children");
            for &kid in kids {
                assert_eq!(
                    parent(world, kid),
                    Some(e),
                    "{context}: {kid:?} under {e:?}"
                );
            }
        }
        if let Some(stored) = world.get::<WorldTransform>(e) {
            let reference = world_placement(world, e);
            assert!(
                stored.affine().abs_diff_eq(reference, 1e-3),
                "{context}: {e:?}'s world transform is stale"
            );
        }
    }
}

struct Rng(u64);
impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n as u64) as usize
    }
}

/// An insert into a live entity succeeds, unless a despawn earlier in the same command has
/// started on it.
fn expect_alive_or_despawning(result: Result<(), EcsError>) {
    assert!(
        matches!(result, Ok(()) | Err(EcsError::Despawning(_))),
        "{result:?}"
    );
}

/// One random operation, applied directly to `world`.
fn random_op(world: &mut World, rng: &mut Rng, live: &[Entity], step: usize) {
    let pick = |rng: &mut Rng| live[rng.below(live.len())];
    let placement = at(rng.below(7) as f32, rng.below(3) as f32, step as f32 * 0.01);
    match rng.below(10) {
        0 | 1 => {
            world.spawn((Node, placement));
        }
        _ if live.is_empty() => {}
        2 => {
            let link = if rng.below(2) == 0 {
                ChildOf::new(pick(rng))
            } else {
                ChildOf::new(pick(rng)).despawn_with_parent()
            };
            world.spawn((Node, placement, link));
        }
        3 => {
            let (c, p) = (pick(rng), pick(rng));
            let rule = if rng.below(2) == 0 {
                KEEP_LOCAL
            } else {
                KEEP_WORLD
            };
            if let Err(error) = set_parent(world, c, p, rule) {
                // Cycles are refused, and so are entities mid-despawn (a despawn earlier in the
                // same command finishes after it). Nothing else may fail.
                assert!(
                    matches!(
                        error,
                        HierarchyError::Cycle { .. }
                            | HierarchyError::Ecs {
                                source: EcsError::Despawning(_),
                                ..
                            }
                    ),
                    "step {step}: {error}"
                );
            }
        }
        4 => {
            // Directly, bypassing set_parent's checks: the hooks must cope.
            let (c, p) = (pick(rng), pick(rng));
            expect_alive_or_despawning(world.insert(c, ChildOf::new(p)));
        }
        5 => {
            remove_parent(world, pick(rng), KEEP_WORLD);
        }
        6 => {
            world.despawn(pick(rng));
        }
        7 => {
            world.remove::<Children>(pick(rng));
        }
        8 => {
            let reactor = Reactor {
                target: pick(rng),
                other: pick(rng),
                despawn: rng.below(2) == 0,
            };
            expect_alive_or_despawning(world.insert(pick(rng), reactor));
        }
        _ => {
            if let Some(mut local) = world.get_mut::<LocalTransform>(pick(rng)) {
                local.0.translation.y += 1.0;
            }
        }
    }
}

#[test]
fn random_direct_operations_keep_the_hierarchy_consistent() {
    let mut rng = Rng(0x0123_4567_89AB_CDEF);
    let mut world = World::new();
    for step in 0..3_000 {
        let live: Vec<Entity> = world.query::<Entity>().collect();
        random_op(&mut world, &mut rng, &live, step);
        run_post_update(&mut world);
        check(&world, &format!("step {step}"));
    }
    assert_eq!(
        repairs(),
        0,
        "random correct operations never need a repair"
    );
}

/// Lets a system's commands run random operations, several per sync point.
#[derive(Debug, Default)]
struct Script(u64);
impl Resource for Script {}

#[test]
fn random_operations_batched_as_system_commands_keep_the_hierarchy_consistent() {
    let mut world = World::new();
    world.insert_resource(Script(0x00C0_FFEE_0DD5_EED5));
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PrePhysics,
        System::new("random hierarchy edits", |ctx| {
            let seed = ctx.resources.get_mut::<Script>().map_or(1, |mut s| {
                s.0 = s.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                s.0
            });
            let live: Vec<Entity> = ctx.world.query::<Entity>().collect();
            // Reserved handles too: spawned now, used by later commands in the same batch.
            let mut fresh = Vec::new();
            for i in 0..8 {
                if i % 3 == 0 {
                    fresh.push(ctx.commands.spawn((Node, at(1.0, 0.0, 0.0))));
                }
                let pool: Vec<Entity> = live.iter().chain(&fresh).copied().collect();
                let step = i;
                let op_seed = seed ^ (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                ctx.commands.run(move |world| {
                    // Several changes in one command: where ordering and lost-update bugs live.
                    let mut rng = Rng(op_seed | 1);
                    for _ in 0..3 {
                        let live: Vec<Entity> = pool
                            .iter()
                            .copied()
                            .filter(|&e| world.contains(e))
                            .collect();
                        random_op(world, &mut rng, &live, step);
                    }
                });
            }
        })
        .writes_resource::<Script>(),
    );
    add_systems(&mut schedule);
    for frame in 0..400 {
        schedule.run(&mut world).expect("valid");
        check(&world, &format!("frame {frame}"));
    }
    assert_eq!(
        repairs(),
        0,
        "random correct operations never need a repair"
    );
}

/// Runs `edit` as one command from a system, so all its changes share one command.
fn in_one_command(world: &mut World, edit: impl FnOnce(&mut World) + Send + 'static) {
    let edit = Mutex::new(Some(edit));
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PrePhysics,
        System::new("one command", move |ctx| {
            if let Some(edit) = edit.lock().expect("lock").take() {
                ctx.commands.run(edit);
            }
        }),
    );
    schedule.run(world).expect("valid");
}

fn repairs() -> usize {
    crate::relation::REPAIRS.with(std::cell::Cell::get)
}

#[test]
fn several_attaches_in_one_command_keep_their_order() {
    let mut world = World::new();
    let q = world.spawn(());
    let kids: Vec<Entity> = (0..4).map(|_| world.spawn(())).collect();
    let order = kids.clone();
    in_one_command(&mut world, move |world| {
        for &kid in &order {
            world.insert(kid, ChildOf::new(q)).expect("alive");
        }
    });
    assert_eq!(children(&world, q), kids);
}

#[test]
fn moving_several_children_in_one_command_needs_no_repair() {
    let mut world = World::new();
    let p = world.spawn(());
    let q = world.spawn(());
    let a = world.spawn(ChildOf::new(p));
    let b = world.spawn(ChildOf::new(p));
    let c = world.spawn(ChildOf::new(p));
    let before = repairs();
    in_one_command(&mut world, move |world| {
        set_parent(world, b, q, KEEP_LOCAL).expect("valid");
        set_parent(world, a, q, KEEP_LOCAL).expect("valid");
    });
    assert_eq!(repairs(), before, "correct code never triggers a repair");
    assert_eq!(children(&world, q), [b, a]);
    assert_eq!(children(&world, p), [c]);
}

#[test]
fn a_child_attached_just_before_its_parent_despawns_follows_its_link() {
    let mut world = World::new();
    let p = world.spawn(at(5.0, 0.0, 0.0));
    let kept = world.spawn(at(0.0, 1.0, 0.0));
    let part = world.spawn(at(0.0, 2.0, 0.0));
    in_one_command(&mut world, move |world| {
        world.insert(kept, ChildOf::new(p)).expect("alive");
        world
            .insert(part, ChildOf::new(p).despawn_with_parent())
            .expect("alive");
        world.despawn(p);
    });
    assert!(!world.contains(p));
    assert_eq!(parent(&world, kept), None, "no link to a dead parent");
    assert!(near(
        local(&world, kept)
            .map(|t| t.translation)
            .unwrap_or(Vec3::NAN),
        Vec3::new(5.0, 1.0, 0.0)
    ));
    assert!(
        !world.contains(part),
        "despawned with its parent, as its link said"
    );
}

#[test]
fn keep_world_under_a_zero_scale_parent_is_refused_without_corruption() {
    let mut world = World::new();
    let hidden = world.spawn(LocalTransform(
        Transform::from_translation(Vec3::X).with_scale(Vec3::ZERO),
    ));
    let c = world.spawn(at(1.0, 2.0, 3.0));
    assert_eq!(
        set_parent(&mut world, c, hidden, KEEP_WORLD),
        Err(HierarchyError::CantKeepWorld {
            child: c,
            parent: hidden
        })
    );
    assert_eq!(parent(&world, c), None, "nothing changed");
    assert_eq!(
        local(&world, c).map(|t| t.translation),
        Some(Vec3::new(1.0, 2.0, 3.0))
    );

    // Keeping the local transform works, and detaching a zero-scale child writes no NaN.
    set_parent(&mut world, c, hidden, KEEP_LOCAL).expect("valid");
    remove_parent(&mut world, c, KEEP_WORLD);
    let after = local(&world, c).expect("has one");
    assert!(after.translation.is_finite() && after.rotation.is_finite() && after.scale.is_finite());
}
