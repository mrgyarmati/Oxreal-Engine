//! Tests of relations and transform propagation.

use oxreal_ecs::{Component, EcsError, Entity, Schedule, Stage, System, World};
use oxreal_math::{Affine3A, Quat, Transform, Vec3};

use crate::{
    ChildOf, Children, HierarchyError, LocalTransform, WorldTransform, add_systems, remove_parent,
    set_parent,
};

fn children(world: &World, parent: Entity) -> Vec<Entity> {
    world
        .get::<Children>(parent)
        .map(|c| c.as_slice().to_vec())
        .unwrap_or_default()
}

fn parent(world: &World, child: Entity) -> Option<Entity> {
    world.get::<ChildOf>(child).map(ChildOf::parent)
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
}

#[test]
fn moving_and_detaching_keep_both_sides_consistent() {
    let mut world = World::new();
    let p = world.spawn(());
    let q = world.spawn(());
    let a = world.spawn(ChildOf::new(p));
    let b = world.spawn(ChildOf::new(p));

    set_parent(&mut world, a, q).expect("valid");
    assert_eq!(children(&world, p), [b]);
    assert_eq!(children(&world, q), [a]);

    assert_eq!(remove_parent(&mut world, b), Some(p));
    assert!(!world.has::<Children>(p), "an empty Children is removed");
    assert_eq!(parent(&world, b), None);
    assert_eq!(remove_parent(&mut world, b), None);

    // Setting the same parent again changes nothing.
    set_parent(&mut world, a, q).expect("valid");
    assert_eq!(children(&world, q), [a]);
}

#[test]
fn set_parent_refuses_cycles_and_dead_parents() {
    let mut world = World::new();
    let root = world.spawn(());
    let mid = world.spawn(ChildOf::new(root));
    let leaf = world.spawn(ChildOf::new(mid));

    assert_eq!(
        set_parent(&mut world, root, leaf),
        Err(HierarchyError::Cycle {
            child: root,
            parent: leaf
        })
    );
    assert_eq!(
        set_parent(&mut world, mid, mid),
        Err(HierarchyError::Cycle {
            child: mid,
            parent: mid
        })
    );
    let dead = world.spawn(());
    world.despawn(dead);
    assert_eq!(
        set_parent(&mut world, leaf, dead),
        Err(HierarchyError::Ecs(EcsError::NoSuchEntity(dead)))
    );
    // Nothing changed.
    assert_eq!(children(&world, root), [mid]);
    assert_eq!(children(&world, mid), [leaf]);
    assert_eq!(parent(&world, root), None);
    let message = HierarchyError::Cycle {
        child: root,
        parent: leaf,
    }
    .to_string();
    assert!(message.contains("cycle"), "{message}");
}

#[test]
fn hooks_refuse_cycles_and_dead_parents_inserted_directly() {
    let mut world = World::new();
    let root = world.spawn(());
    let leaf = world.spawn(ChildOf::new(root));
    // Inserted directly, bypassing set_parent's checks: the hook refuses it and the entity
    // ends up without a parent, rather than in a cycle.
    world.insert(root, ChildOf::new(leaf)).expect("alive");
    assert_eq!(parent(&world, root), None);
    assert_eq!(children(&world, leaf), Vec::<Entity>::new());
    assert_eq!(children(&world, root), [leaf]);

    let dead = world.spawn(());
    world.despawn(dead);
    let orphan = world.spawn(ChildOf::new(dead));
    assert_eq!(parent(&world, orphan), None);
}

#[test]
fn despawning_a_parent_despawns_its_subtree() {
    let mut world = World::new();
    let root = world.spawn(());
    let a = world.spawn(ChildOf::new(root));
    let a1 = world.spawn(ChildOf::new(a));
    let b = world.spawn(ChildOf::new(root));
    let other = world.spawn(());
    assert!(world.despawn(root));
    for e in [root, a, a1, b] {
        assert!(!world.contains(e), "{e:?}");
    }
    assert!(world.contains(other));
    assert_eq!(world.len(), 1);
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
fn removing_children_detaches_rather_than_despawns() {
    let mut world = World::new();
    let p = world.spawn(());
    let a = world.spawn(ChildOf::new(p));
    let b = world.spawn(ChildOf::new(p));
    assert!(world.remove::<Children>(p).is_some());
    assert!(world.contains(a) && world.contains(b));
    assert_eq!(parent(&world, a), None);
    assert_eq!(parent(&world, b), None);
}

#[test]
fn many_children_attaching_in_one_flush_all_arrive_in_order() {
    let mut world = World::new();
    let p = world.spawn(());
    let made = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let m = made.clone();
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PrePhysics,
        System::new("spawn children", move |ctx| {
            let mut list = m.lock().expect("lock");
            for _ in 0..10 {
                list.push(ctx.commands.spawn(ChildOf::new(p)));
            }
        }),
    );
    schedule.run(&mut world).expect("valid");
    assert_eq!(children(&world, p), *made.lock().expect("lock"));
}

/// A marker, to tell entities apart in the randomized test.
#[derive(Debug, Clone, Copy)]
struct Node;
impl Component for Node {}

fn affine(t: &Transform) -> Affine3A {
    Affine3A::from_scale_rotation_translation(t.scale, t.rotation, t.translation)
}

fn run_post_update(world: &mut World) {
    let mut schedule = Schedule::new();
    add_systems(&mut schedule);
    schedule.run(world).expect("valid");
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
    assert_eq!(
        world
            .get::<WorldTransform>(root)
            .map(WorldTransform::affine),
        Some(affine(&t_root))
    );

    // Moving the root moves everything below it on the next propagation.
    if let Some(mut local) = world.get_mut::<LocalTransform>(root) {
        local.0.translation = Vec3::new(-5.0, 0.0, 0.0);
    }
    run_post_update(&mut world);
    let moved = world
        .get::<WorldTransform>(leaf)
        .map(WorldTransform::translation);
    assert!(moved.is_some_and(|t| t.x < 0.0), "{moved:?}");
}

#[test]
fn detached_entities_become_roots_for_propagation() {
    let mut world = World::new();
    let p = world.spawn(LocalTransform(Transform::from_translation(Vec3::X * 10.0)));
    let c = world.spawn((
        LocalTransform(Transform::from_translation(Vec3::Y)),
        ChildOf::new(p),
    ));
    run_post_update(&mut world);
    assert_eq!(
        world
            .get::<WorldTransform>(c)
            .map(WorldTransform::translation),
        Some(Vec3::new(10.0, 1.0, 0.0))
    );
    remove_parent(&mut world, c);
    run_post_update(&mut world);
    assert_eq!(
        world
            .get::<WorldTransform>(c)
            .map(WorldTransform::translation),
        Some(Vec3::Y)
    );
}

/// Checks every hierarchy invariant.
fn check(world: &World) {
    let nodes: Vec<Entity> = world.query::<Entity>().collect();
    for &e in &nodes {
        if let Some(p) = parent(world, e) {
            assert!(world.contains(p), "{e:?}'s parent {p:?} is dead");
            let siblings = children(world, p);
            assert_eq!(
                siblings.iter().filter(|&&s| s == e).count(),
                1,
                "{e:?} is listed once under {p:?}: {siblings:?}"
            );
            // No cycles: walking up ends.
            let mut current = e;
            for _ in 0..=nodes.len() {
                match parent(world, current) {
                    Some(up) => current = up,
                    None => break,
                }
            }
            assert_eq!(parent(world, current), None, "cycle above {e:?}");
        }
        if let Some(kids) = world.get::<Children>(e) {
            assert!(!kids.is_empty(), "{e:?} has an empty Children");
            for &kid in kids {
                assert_eq!(parent(world, kid), Some(e), "{kid:?} is listed under {e:?}");
            }
        }
    }
}

fn descendants(world: &World, root: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        out.push(e);
        stack.extend(children(world, e));
    }
    out
}

#[test]
fn random_operations_keep_the_hierarchy_consistent() {
    let mut state: u64 = 0x0123_4567_89AB_CDEF;
    let mut next = |n: usize| {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        (state.wrapping_mul(0x2545_F491_4F6C_DD1D) % n as u64) as usize
    };
    let mut world = World::new();
    let mut live: Vec<Entity> = Vec::new();
    for step in 0..5_000 {
        live.retain(|&e| world.contains(e));
        match next(7) {
            0 | 1 => live.push(world.spawn(Node)),
            _ if live.is_empty() => {}
            2 => {
                let p = live[next(live.len())];
                live.push(world.spawn((Node, ChildOf::new(p))));
            }
            3 => {
                let (c, p) = (live[next(live.len())], live[next(live.len())]);
                // Cycles are refused with an error; anything else must succeed.
                if let Err(error) = set_parent(&mut world, c, p) {
                    assert!(matches!(error, HierarchyError::Cycle { .. }), "{error}");
                }
            }
            4 => {
                let c = live[next(live.len())];
                remove_parent(&mut world, c);
            }
            5 => {
                let e = live[next(live.len())];
                let subtree = descendants(&world, e);
                assert!(world.despawn(e));
                for gone in subtree {
                    assert!(!world.contains(gone), "step {step}: {gone:?} survived");
                }
            }
            _ => {
                let e = live[next(live.len())];
                world.remove::<Children>(e);
            }
        }
        if step % 25 == 0 {
            check(&world);
        }
    }
    check(&world);
}
