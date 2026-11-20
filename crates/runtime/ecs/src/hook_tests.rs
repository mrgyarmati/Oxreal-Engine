//! Tests of lifecycle hooks (ADR-0014 §4).

use crate::{
    Component, ComponentHooks, EcsError, Entity, HookCause, HookContext, HookWorld, Resource,
    Schedule, Stage, System, World,
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

fn world() -> World {
    let mut world = World::new();
    world.insert_resource(Log::default());
    world
}

/// Logs every hook with the value it sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Watched(u32);
impl Component for Watched {
    const HOOKS: ComponentHooks = ComponentHooks::NONE
        .on_add(|w, ctx| {
            let value = w.get::<Watched>(ctx.entity).map(|v| v.0);
            log(w, format!("add {value:?}"));
        })
        .on_insert(|w, ctx| {
            let value = w.get::<Watched>(ctx.entity).map(|v| v.0);
            log(w, format!("insert {value:?}"));
        })
        .on_replace(|w, ctx| {
            let value = w.get::<Watched>(ctx.entity).map(|v| v.0);
            log(w, format!("replace {value:?}"));
        })
        .on_remove(|w, ctx| {
            let value = w.get::<Watched>(ctx.entity).map(|v| v.0);
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

/// Logs the cause it was called with.
#[derive(Debug, Clone, Copy)]
struct Causes;
impl Component for Causes {
    const HOOKS: ComponentHooks = ComponentHooks::NONE
        .on_add(|w, ctx| log(w, format!("add {:?}", ctx.cause)))
        .on_remove(|w, ctx| log(w, format!("remove {:?}", ctx.cause)));
}

#[test]
fn hooks_are_told_why_they_run() {
    let mut world = world();
    let a = world.spawn(Causes);
    world.remove::<Causes>(a);
    world.insert(a, Causes).expect("alive");
    world.despawn(a);
    assert_eq!(
        lines(&mut world),
        ["add Spawn", "remove Remove", "add Insert", "remove Despawn"]
    );
    // The context is plain data.
    let ctx = HookContext {
        entity: a,
        cause: HookCause::Despawn,
    };
    assert_eq!(ctx, ctx.clone());
}

/// A minimal parent/child relation, the way the hierarchy will build it: despawning a parent
/// despawns its children first.
#[derive(Debug, Clone, Default)]
struct Kids(Vec<Entity>);
impl Component for Kids {
    const HOOKS: ComponentHooks = ComponentHooks::NONE.on_remove(|w, ctx| {
        if ctx.cause == HookCause::Despawn {
            let kids = w
                .get::<Kids>(ctx.entity)
                .map(|k| k.0.clone())
                .unwrap_or_default();
            for kid in kids {
                w.commands().despawn(kid);
            }
        }
    });
}

/// A child, which checks on removal that its parent is still alive and whole.
#[derive(Debug, Clone, Copy)]
struct Kid {
    name: &'static str,
    parent: Entity,
}
impl Component for Kid {
    const HOOKS: ComponentHooks = ComponentHooks::NONE.on_remove(|w, ctx| {
        if let Some(kid) = w.get::<Kid>(ctx.entity).copied() {
            let parent_whole = w.contains(kid.parent) && w.has::<Kids>(kid.parent);
            log(
                w,
                format!("{} gone, parent whole: {parent_whole}", kid.name),
            );
        }
    });
}

/// Records, when dropped, the order in which entities were actually removed.
struct Removal {
    name: &'static str,
    order: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
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
fn despawn_removes_children_before_parents_while_the_parents_are_whole() {
    let order = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let removal = |name| Removal {
        name,
        order: order.clone(),
    };
    let mut world = world();
    let nobody = Entity::from_bits(u64::MAX).expect("valid bits");
    let root = world.spawn((
        Kids::default(),
        Kid {
            name: "root",
            parent: nobody,
        },
        removal("root"),
    ));
    let a = world.spawn((
        Kids::default(),
        Kid {
            name: "a",
            parent: root,
        },
        removal("a"),
    ));
    let b = world.spawn((
        Kid {
            name: "b",
            parent: root,
        },
        removal("b"),
    ));
    let a1 = world.spawn((
        Kid {
            name: "a1",
            parent: a,
        },
        removal("a1"),
    ));
    let a2 = world.spawn((
        Kid {
            name: "a2",
            parent: a,
        },
        removal("a2"),
    ));
    world.insert(root, Kids(vec![a, b])).expect("alive");
    world.insert(a, Kids(vec![a1, a2])).expect("alive");
    lines(&mut world);

    assert!(world.despawn(root));
    // Hooks run parent first (the parent's hook is what despawns the children), and every
    // child's hook sees its parent alive and whole. "root" has no living parent.
    assert_eq!(
        lines(&mut world),
        [
            "root gone, parent whole: false",
            "a gone, parent whole: true",
            "a1 gone, parent whole: true",
            "a2 gone, parent whole: true",
            "b gone, parent whole: true",
        ]
    );
    // Removal is post-order: every child before its parent.
    assert_eq!(*order.lock().expect("lock"), ["a1", "a2", "a", "b", "root"]);
    assert!(world.is_empty());
}

/// On removal, tries to change its own entity, which is being despawned.
#[derive(Debug, Clone, Copy)]
struct Stubborn;
impl Component for Stubborn {
    const HOOKS: ComponentHooks = ComponentHooks::NONE.on_remove(|w, ctx| {
        if ctx.cause == HookCause::Despawn {
            let entity = ctx.entity;
            w.commands().run(move |world| {
                let insert = world.insert(entity, Plain);
                let despawn_again = world.despawn(entity);
                let remove = world.remove::<Stubborn>(entity).is_some();
                if let Some(mut log) = world.resource_mut::<Log>() {
                    log.0.push(format!("{insert:?} {despawn_again} {remove}"));
                }
            });
        }
    });
}

#[test]
fn a_despawning_entity_refuses_structural_changes() {
    let mut world = world();
    let e = world.spawn(Stubborn);
    assert!(world.despawn(e));
    assert_eq!(
        lines(&mut world),
        [format!(
            "{:?} false false",
            Err::<(), _>(EcsError::Despawning(e))
        )]
    );
    assert!(!world.contains(e));
    let message = EcsError::Despawning(e).to_string();
    assert!(message.contains("being despawned"), "{message}");
}

/// Spawns a `Shadow` pointing back at its owner when added, and despawns it when removed.
#[derive(Debug, Clone, Copy)]
struct Owner {
    shadow: Option<Entity>,
}
impl Component for Owner {
    const HOOKS: ComponentHooks = ComponentHooks::NONE
        .on_add(|w, ctx| {
            let shadow = w.commands().spawn(Shadow(ctx.entity));
            if let Some(mut this) = w.get_mut::<Owner>(ctx.entity) {
                this.shadow = Some(shadow);
            }
            log(w, "owner added".into());
        })
        .on_remove(|w, ctx| {
            if let Some(shadow) = w.get::<Owner>(ctx.entity).and_then(|o| o.shadow) {
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
fn hook_commands_apply_before_a_direct_change_returns() {
    let mut world = world();
    let owner = world.spawn(Owner { shadow: None });
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

/// A chain: despawning a link despawns the next one, whose hook does the same.
#[derive(Debug, Clone, Copy)]
struct Link(Option<Entity>);
impl Component for Link {
    const HOOKS: ComponentHooks = ComponentHooks::NONE.on_remove(|w, ctx| {
        if let Some(next) = w.get::<Link>(ctx.entity).and_then(|l| l.0) {
            w.commands().despawn(next);
        }
    });
}

#[test]
fn a_deep_cascade_runs_in_constant_stack() {
    // Recursing once per link would overflow a test thread's 2 MiB stack long before this.
    const LINKS: usize = 100_000;
    let mut world = World::new();
    let mut next = None;
    for _ in 0..LINKS {
        next = Some(world.spawn(Link(next)));
    }
    world.spawn(Plain);
    assert!(world.despawn(next.expect("spawned")));
    assert_eq!(
        world.len(),
        1,
        "the whole chain went, the unrelated entity stayed"
    );
}

/// Queues a command, then panics.
#[derive(Debug, Clone, Copy)]
struct Faulty;
impl Component for Faulty {
    const HOOKS: ComponentHooks = ComponentHooks::NONE
        .on_add(|w, _| {
            w.commands().spawn(Plain);
        })
        .on_insert(|_, _| panic!("a deliberate test panic"));
}

#[test]
fn a_panicking_hook_leaves_no_stale_commands_behind() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let mut world = World::new();
    let result = catch_unwind(AssertUnwindSafe(|| world.spawn(Faulty)));
    assert!(result.is_err(), "the hook's panic propagates");
    assert_eq!(world.len(), 1, "the spawn itself happened");

    // The command the hook queued before panicking is discarded, not run by the next change.
    world.spawn(Watched(1));
    assert_eq!(world.len(), 2);
    assert_eq!(world.query::<&Plain>().count(), 0);
}

/// Counts live `Counted` components in a resource, through hooks only.
#[derive(Debug, Default, PartialEq, Eq)]
struct Census(i64);
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
fn hooks_fire_exactly_once_per_change_in_random_sequences() {
    // xorshift64*: deterministic, so a failure reproduces.
    let mut state: u64 = 0x005E_ED0F_C0FF_EE15;
    let mut next = |n: usize| {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        (state.wrapping_mul(0x2545_F491_4F6C_DD1D) % n as u64) as usize
    };
    let mut world = World::new();
    world.insert_resource(Census::default());
    let mut live: Vec<Entity> = Vec::new();
    for step in 0..10_000 {
        match next(6) {
            0 => live.push(world.spawn(Counted)),
            1 => live.push(world.spawn((Counted, Plain))),
            2 => live.push(world.spawn(Plain)),
            _ if live.is_empty() => {}
            3 => {
                let e = live.swap_remove(next(live.len()));
                world.despawn(e);
            }
            4 => {
                let e = live[next(live.len())];
                world.insert(e, Counted).expect("alive");
            }
            _ => {
                let e = live[next(live.len())];
                world.remove::<Counted>(e);
            }
        }
        if step % 50 == 0 {
            let counted = world.query::<&Counted>().count() as i64;
            assert_eq!(
                world.resource::<Census>(),
                Some(&Census(counted)),
                "step {step}"
            );
        }
    }
}

/// Immutable: only `insert` can change it, so its hooks see every change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Locked(u32);
impl Component for Locked {
    const MUTABLE: bool = false;
    const HOOKS: ComponentHooks = ComponentHooks::NONE
        .on_replace(|w, ctx| {
            let old = w.get::<Locked>(ctx.entity).map(|l| l.0);
            log(w, format!("replace {old:?}"));
        })
        .on_insert(|w, ctx| {
            let new = w.get::<Locked>(ctx.entity).map(|l| l.0);
            log(w, format!("insert {new:?}"));
        });
}

#[test]
fn immutable_components_change_only_through_insert_with_hooks() {
    let mut world = world();
    let e = world.spawn(Locked(1));
    assert_eq!(world.get::<Locked>(e), Some(&Locked(1)), "reading is fine");
    assert_eq!(world.query::<&Locked>().count(), 1);
    world.insert(e, Locked(2)).expect("alive");
    assert_eq!(
        lines(&mut world),
        ["insert Some(1)", "replace Some(1)", "insert Some(2)"]
    );
    assert_eq!(world.remove::<Locked>(e), Some(Locked(2)));
}

/// Records, from its `on_remove` hook, whether the world reports the entity as despawning.
#[derive(Debug, Clone, Copy)]
struct Witness;
impl Component for Witness {
    const HOOKS: ComponentHooks = ComponentHooks::NONE.on_remove(|w, ctx| {
        let entity = ctx.entity;
        let in_hook = w.is_despawning(entity);
        log(w, format!("{:?} in hook: {in_hook}", ctx.cause));
        w.commands().run(move |world| {
            let in_command = world.is_despawning(entity);
            let alive = world.contains(entity);
            if let Some(mut log) = world.resource_mut::<Log>() {
                log.0
                    .push(format!("in command: {in_command}, alive: {alive}"));
            }
        });
    });
}

#[test]
fn despawning_is_visible_to_hooks_and_their_commands() {
    let mut world = world();
    let e = world.spawn(Witness);
    world.remove::<Witness>(e);
    assert_eq!(
        lines(&mut world),
        ["Remove in hook: false", "in command: false, alive: true"]
    );
    world.insert(e, Witness).expect("alive");
    assert!(!world.is_despawning(e));
    world.despawn(e);
    assert_eq!(
        lines(&mut world),
        ["Despawn in hook: true", "in command: true, alive: true"]
    );
    assert!(!world.is_despawning(e), "not after the despawn finished");
}
