//! Tests of lifecycle hooks (ADR-0014 §4).

use crate::{
    Component, ComponentHooks, Entity, HookWorld, Resource, Schedule, Stage, System, World,
};

/// What hooks saw, in order.
#[derive(Debug, Default)]
struct Log(Vec<String>);
impl Resource for Log {}

fn log(world: &mut HookWorld<'_>, line: String) {
    if let Some(mut log) = world.resource_mut::<Log>() {
        log.0.push(line);
    }
}

fn lines(world: &mut World) -> Vec<String> {
    world
        .resource_mut::<Log>()
        .map(|mut log| std::mem::take(&mut log.0))
        .unwrap_or_default()
}

/// Logs every hook with the value it sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Watched(u32);
impl Component for Watched {
    const HOOKS: ComponentHooks = ComponentHooks::NONE
        .on_add(|w, e| {
            let value = w.get::<Watched>(e).map(|v| v.0);
            log(w, format!("add {value:?}"));
        })
        .on_insert(|w, e| {
            let value = w.get::<Watched>(e).map(|v| v.0);
            log(w, format!("insert {value:?}"));
        })
        .on_replace(|w, e| {
            let value = w.get::<Watched>(e).map(|v| v.0);
            log(w, format!("replace {value:?}"));
        })
        .on_remove(|w, e| {
            let value = w.get::<Watched>(e).map(|v| v.0);
            log(w, format!("remove {value:?}"));
        });
}

/// A second hooked component, to check ordering across components on despawn.
#[derive(Debug, Clone, Copy)]
struct Other;
impl Component for Other {
    const HOOKS: ComponentHooks = ComponentHooks::NONE
        .on_replace(|w, _| log(w, "replace Other".into()))
        .on_remove(|w, _| log(w, "remove Other".into()));
}

/// No hooks.
#[derive(Debug, Clone, Copy)]
struct Plain;
impl Component for Plain {}

fn world() -> World {
    let mut world = World::new();
    world.insert_resource(Log::default());
    world
}

#[test]
fn hooks_fire_in_order_and_see_the_right_values() {
    let mut world = world();
    let e = world.spawn(Watched(1));
    assert_eq!(lines(&mut world), ["add Some(1)", "insert Some(1)"]);

    world.insert(e, Watched(2)).expect("alive");
    assert_eq!(lines(&mut world), ["replace Some(1)", "insert Some(2)"]);

    // Adding another component doesn't touch Watched's hooks.
    world.insert(e, Plain).expect("alive");
    assert!(lines(&mut world).is_empty());

    assert_eq!(world.remove::<Watched>(e), Some(Watched(2)));
    assert_eq!(lines(&mut world), ["replace Some(2)", "remove Some(2)"]);

    world.insert(e, (Watched(3), Plain)).expect("alive");
    assert_eq!(
        lines(&mut world),
        ["add Some(3)", "insert Some(3)"],
        "a bundle mixing new and existing components fires add only for the new one"
    );
}

#[test]
fn despawn_runs_every_replace_before_any_remove_with_the_entity_whole() {
    let mut world = world();
    let e = world.spawn((Watched(7), Other, Plain));
    lines(&mut world);
    assert!(world.despawn(e));
    let log = lines(&mut world);
    assert_eq!(log.len(), 4, "{log:?}");
    assert!(log[..2].iter().all(|l| l.starts_with("replace")), "{log:?}");
    assert!(log[2..].iter().all(|l| l.starts_with("remove")), "{log:?}");
    assert!(
        log.contains(&"remove Some(7)".to_owned()),
        "values are still there"
    );
    assert!(!world.contains(e));
}

/// Spawns a `Shadow` pointing back at its owner when added, and despawns it when removed.
#[derive(Debug, Clone, Copy)]
struct Owner {
    shadow: Option<Entity>,
}
impl Component for Owner {
    const HOOKS: ComponentHooks = ComponentHooks::NONE
        .on_add(|w, owner| {
            let shadow = w.commands().spawn(Shadow(owner));
            if let Some(mut this) = w.get_mut::<Owner>(owner) {
                this.shadow = Some(shadow);
            }
            log(w, "owner added".into());
        })
        .on_remove(|w, owner| {
            if let Some(shadow) = w.get::<Owner>(owner).and_then(|o| o.shadow) {
                w.commands().despawn(shadow);
            }
        });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shadow(Entity);
impl Component for Shadow {
    const HOOKS: ComponentHooks = ComponentHooks::NONE.on_add(|w, _| log(w, "shadow added".into()));
}

#[test]
fn hook_commands_apply_right_after_a_direct_change() {
    let mut world = world();
    let owner = world.spawn(Owner { shadow: None });
    // The shadow exists as soon as spawn returns.
    let shadow = world
        .get::<Owner>(owner)
        .and_then(|o| o.shadow)
        .expect("the hook recorded it");
    assert_eq!(world.get::<Shadow>(shadow), Some(&Shadow(owner)));
    assert_eq!(lines(&mut world), ["owner added", "shadow added"]);

    assert!(world.despawn(owner));
    assert!(!world.contains(shadow), "the hook's despawn ran");
    assert!(world.is_empty());
}

#[test]
fn hook_commands_run_before_the_next_command_at_a_sync_point() {
    let mut world = world();
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PrePhysics,
        System::new("spawner", |ctx| {
            ctx.commands.spawn(Owner { shadow: None });
            ctx.commands.run(|world| {
                if let Some(mut log) = world.resource_mut::<Log>() {
                    log.0.push("next command".into());
                }
            });
        }),
    );
    schedule.run(&mut world).expect("valid");
    // Depth-first: the owner's hook spawns the shadow before the system's next command runs.
    assert_eq!(
        lines(&mut world),
        ["owner added", "shadow added", "next command"]
    );
    assert_eq!(world.len(), 2);
}

#[test]
fn hooks_on_command_spawns_run_at_the_sync_point() {
    let mut world = world();
    let mut schedule = Schedule::new();
    schedule.add(
        Stage::PrePhysics,
        System::new("spawner", |ctx| {
            ctx.commands.spawn(Watched(5));
        }),
    );
    schedule.run(&mut world).expect("valid");
    assert_eq!(lines(&mut world), ["add Some(5)", "insert Some(5)"]);
}

/// Counts live `Counted` components in a resource, through hooks only.
#[derive(Debug, Default, PartialEq, Eq)]
struct Census(u32);
impl Resource for Census {}

#[derive(Debug, Clone, Copy)]
struct Counted;
impl Component for Counted {
    const HOOKS: ComponentHooks = ComponentHooks::NONE
        .on_add(|w, _| {
            if let Some(mut census) = w.resource_mut::<Census>() {
                census.0 += 1;
            }
        })
        .on_remove(|w, _| {
            if let Some(mut census) = w.resource_mut::<Census>() {
                census.0 -= 1;
            }
        });
}

#[test]
fn hooks_keep_an_invariant_through_every_kind_of_change() {
    let mut world = World::new();
    world.insert_resource(Census::default());
    let a = world.spawn(Counted);
    let b = world.spawn((Counted, Plain));
    let c = world.spawn(Plain);
    world.insert(c, Counted).expect("alive");
    world.insert(c, Counted).expect("alive"); // A replacement: not a second add.
    assert_eq!(world.resource::<Census>(), Some(&Census(3)));
    world.remove::<Counted>(b);
    world.despawn(a);
    assert_eq!(world.resource::<Census>(), Some(&Census(1)));
    world.despawn(c);
    assert_eq!(world.resource::<Census>(), Some(&Census(0)));
}

/// A chain: despawning a link despawns the next one, whose hook does the same.
#[derive(Debug, Clone, Copy)]
struct Link(Option<Entity>);
impl Component for Link {
    const HOOKS: ComponentHooks = ComponentHooks::NONE.on_remove(|w, e| {
        if let Some(next) = w.get::<Link>(e).and_then(|l| l.0) {
            w.commands().despawn(next);
        }
    });
}

#[test]
fn hook_commands_cascade() {
    let mut world = World::new();
    let mut next = None;
    let mut first = None;
    for _ in 0..50 {
        let e = world.spawn(Link(next));
        next = Some(e);
        first = Some(e);
    }
    world.spawn(Plain);
    assert!(world.despawn(first.expect("spawned")));
    assert_eq!(
        world.len(),
        1,
        "the whole chain went, the unrelated entity stayed"
    );
}
