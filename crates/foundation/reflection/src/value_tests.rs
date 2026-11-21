//! Tests of views, paths, patches, owned values, validation and foreign types (run under Miri).

use std::cell::Cell;
use std::mem::offset_of;

use oxreal_math::{Quat, Transform, Vec3};

use crate::{
    DefinitionProblem, FieldPath, ForeignTypeDefinition, OwnedValue, Patch, PatchError, Reflect,
    RegistrationError, StableTypeId, TypeRegistry, ValueError,
};

#[derive(Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(id = "1f5b3d7a-9c2e-4a6b-8d0f-4a6c8e0a2b5d", clone, default)]
#[repr(u8)]
enum Stance {
    #[default]
    Standing = 1,
    Crouching = 4,
    Prone = 9,
}

#[derive(Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(id = "2a6c4e8b-0d3f-4b7c-9e1a-5b7d9f1b3c6e", clone, default)]
enum Team {
    #[default]
    Red,
    #[reflect(alias = "Azure")]
    Blue,
}

#[derive(Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(id = "3b7d5f9c-1e4a-4c8d-8f2b-6c8e0a2c4d7f", clone, default)]
struct Stats {
    health: f32,
    #[reflect(alias = "armour")]
    armor: u16,
}

#[derive(Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(id = "4c8e6a0d-2f5b-4d9e-9a3c-7d9f1b3d5e8a", clone, default)]
struct Soldier {
    name: String,
    stats: Stats,
    stance: Stance,
    team: Team,
    placement: Transform,
    waypoints: Vec<Vec3>,
    target: Option<Stats>,
    scores: [i32; 3],
    #[reflect(opaque)]
    secret: u64,
}

fn soldier() -> Soldier {
    Soldier {
        name: "Ada".to_owned(),
        stats: Stats {
            health: 75.0,
            armor: 3,
        },
        stance: Stance::Crouching,
        team: Team::Blue,
        placement: Transform::from_translation(Vec3::new(1.0, 2.0, 3.0)),
        waypoints: vec![Vec3::X, Vec3::Y],
        target: Some(Stats {
            health: 10.0,
            armor: 0,
        }),
        scores: [5, -2, 7],
        secret: 42,
    }
}

fn path(text: &str) -> FieldPath {
    text.parse().expect("test paths are well-formed")
}

#[test]
fn shared_views_read_every_kind() {
    let registry = TypeRegistry::new();
    let value = soldier();
    let view = registry.view(&value).expect("registers");

    assert_eq!(
        view.field("name")
            .unwrap()
            .downcast_ref::<String>()
            .unwrap(),
        "Ada"
    );
    assert_eq!(
        *view
            .field("stats")
            .unwrap()
            .field("armour")
            .unwrap()
            .downcast_ref::<u16>()
            .unwrap(),
        3,
        "found by alias"
    );
    assert_eq!(view.field("stance").unwrap().variant_index().unwrap(), 1);
    assert_eq!(view.field("team").unwrap().variant_index().unwrap(), 1);
    let waypoints = view.field("waypoints").unwrap();
    assert_eq!(waypoints.len().unwrap(), 2);
    assert_eq!(
        *waypoints
            .element(1)
            .unwrap()
            .downcast_ref::<Vec3>()
            .unwrap(),
        Vec3::Y
    );
    let scores = view.field("scores").unwrap();
    assert_eq!(
        *scores.element(2).unwrap().downcast_ref::<i32>().unwrap(),
        7
    );
    let target = view.field("target").unwrap().some().unwrap().expect("Some");
    assert_eq!(
        *target
            .field("health")
            .unwrap()
            .downcast_ref::<f32>()
            .unwrap(),
        10.0
    );
    let y = view
        .field("placement")
        .unwrap()
        .component("translation")
        .unwrap();
    assert_eq!(*y.downcast_ref::<Vec3>().unwrap(), Vec3::new(1.0, 2.0, 3.0));
}

#[test]
fn view_mistakes_are_errors() {
    let registry = TypeRegistry::new();
    let value = soldier();
    let view = registry.view(&value).expect("registers");
    let soldier_name = view.description().name().to_owned();

    assert!(matches!(
        view.field("nope"),
        Err(ValueError::NoSuchField { field, .. }) if field == "nope"
    ));
    assert!(matches!(
        view.field("secret"),
        Err(ValueError::OpaqueField { field, .. }) if field == "secret"
    ));
    assert!(matches!(view.element(0), Err(ValueError::WrongKind { ty, .. }) if ty == soldier_name));
    assert!(matches!(
        view.field("scores").unwrap().element(3),
        Err(ValueError::IndexOutOfBounds {
            index: 3,
            len: 3,
            ..
        })
    ));
    assert!(matches!(
        view.field("waypoints").unwrap().element(2),
        Err(ValueError::IndexOutOfBounds {
            index: 2,
            len: 2,
            ..
        })
    ));
    let error = view
        .field("name")
        .unwrap()
        .downcast_ref::<u32>()
        .expect_err("not a u32");
    assert_eq!(
        error.to_string(),
        "expected a value of type 'u32', found 'String'"
    );
    assert!(matches!(
        view.variant_index(),
        Err(ValueError::WrongKind { .. })
    ));
    assert!(matches!(view.some(), Err(ValueError::WrongKind { .. })));
}

#[test]
fn exclusive_views_edit_every_kind() {
    let registry = TypeRegistry::new();
    let mut value = soldier();
    {
        let mut view = registry.view_mut(&mut value).expect("registers");
        *view
            .field_mut("stats")
            .unwrap()
            .field_mut("health")
            .unwrap()
            .downcast_mut::<f32>()
            .unwrap() = 1.0;
        view.field_mut("stance").unwrap().set_variant(2).unwrap();
        view.field_mut("team").unwrap().set_variant(0).unwrap();
        assert!(matches!(
            view.field_mut("team").unwrap().set_variant(2),
            Err(ValueError::NoSuchVariant { index: 2, .. })
        ));

        let mut waypoints = view.field_mut("waypoints").unwrap();
        waypoints
            .push(OwnedValue::new(&registry, Vec3::Z).unwrap())
            .unwrap();
        let removed = waypoints.remove(0).unwrap();
        assert_eq!(removed.into_value::<Vec3>().unwrap(), Vec3::X);
        assert!(matches!(
            waypoints.push(OwnedValue::new(&registry, 1.0_f32).unwrap()),
            Err(ValueError::TypeMismatch { .. })
        ));

        view.field_mut("target").unwrap().set_option(None).unwrap();
        let old = view
            .field_mut("name")
            .unwrap()
            .replace(OwnedValue::new(&registry, "Grace".to_owned()).unwrap())
            .unwrap();
        assert_eq!(old.into_value::<String>().unwrap(), "Ada");

        let mut placement = view.field_mut("placement").unwrap();
        let old = placement
            .set_component(
                "scale",
                OwnedValue::new(&registry, Vec3::splat(2.0)).unwrap(),
            )
            .unwrap();
        assert_eq!(old.into_value::<Vec3>().unwrap(), Vec3::ONE);
    }
    assert_eq!(value.stats.health, 1.0);
    assert_eq!(value.stance, Stance::Prone);
    assert_eq!(value.team, Team::Red);
    assert_eq!(value.waypoints, [Vec3::Y, Vec3::Z]);
    assert_eq!(value.target, None);
    assert_eq!(value.name, "Grace");
    assert_eq!(value.placement.scale, Vec3::splat(2.0));
    assert_eq!(value.secret, 42, "opaque fields are untouched");
}

#[test]
fn paths_read_and_replace_values_including_math_components() {
    let registry = TypeRegistry::new();
    let mut value = soldier();
    let view = registry.view(&value).expect("registers");
    assert_eq!(
        *view
            .path(&path("target?.armor"))
            .unwrap()
            .downcast_ref::<u16>()
            .unwrap(),
        0
    );
    assert_eq!(
        *view
            .path(&path("waypoints[1]"))
            .unwrap()
            .downcast_ref::<Vec3>()
            .unwrap(),
        Vec3::Y
    );
    let x = view.read_path(&path("placement.translation.x")).unwrap();
    assert_eq!(x.into_value::<f32>().unwrap(), 1.0);
    assert!(matches!(
        view.path(&path("placement.translation")),
        Err(ValueError::MathComponentBorrow { .. })
    ));

    let mut view = registry.view_mut(&mut value).expect("registers");
    let old = view
        .replace_path(
            &path("placement.rotation.w"),
            OwnedValue::new(&registry, 0.5_f32).unwrap(),
        )
        .unwrap();
    assert_eq!(old.into_value::<f32>().unwrap(), 1.0);
    let old = view
        .replace_path(
            &path("waypoints[0].z"),
            OwnedValue::new(&registry, 9.0_f32).unwrap(),
        )
        .unwrap();
    assert_eq!(old.into_value::<f32>().unwrap(), 0.0);
    assert!(matches!(
        view.replace_path(
            &path("scores[1]"),
            OwnedValue::new(&registry, 1.0_f32).unwrap()
        ),
        Err(ValueError::TypeMismatch { .. })
    ));
    assert_eq!(
        value.placement.rotation,
        Quat::from_xyzw(0.0, 0.0, 0.0, 0.5)
    );
    assert_eq!(value.waypoints[0], Vec3::new(1.0, 0.0, 9.0));
}

#[test]
fn path_resolution_and_use_report_what_went_wrong() {
    let registry = TypeRegistry::new();
    let key = registry.register::<Soldier>().unwrap();
    let resolve = |text: &str| path(text).resolve(&registry, key);
    assert!(matches!(
        resolve("stats.mana"),
        Err(ValueError::NoSuchField { .. })
    ));
    assert!(matches!(
        resolve("secret"),
        Err(ValueError::OpaqueField { .. })
    ));
    assert!(matches!(
        resolve("scores[3]"),
        Err(ValueError::IndexOutOfBounds { .. })
    ));
    assert!(matches!(
        resolve("stats[0]"),
        Err(ValueError::WrongKind { .. })
    ));
    assert!(matches!(
        resolve("name?"),
        Err(ValueError::WrongKind { .. })
    ));
    assert!(matches!(
        resolve("placement.translation[0]"),
        Err(ValueError::AfterMathComponent { .. })
    ));
    assert!(
        resolve("waypoints[99].x").is_ok(),
        "list bounds are checked on use"
    );

    let mut value = soldier();
    value.target = None;
    let view = registry.view(&value).unwrap();
    assert!(matches!(
        view.path(&path("target?.armor")),
        Err(ValueError::NoValue { .. })
    ));
    assert!(matches!(
        view.read_path(&path("waypoints[99].x")),
        Err(ValueError::IndexOutOfBounds { index: 99, .. })
    ));

    let other = TypeRegistry::new();
    let foreign_path = path("stats")
        .resolve(&other, other.register::<Soldier>().unwrap())
        .unwrap();
    assert!(matches!(
        view.at(&foreign_path),
        Err(ValueError::WrongRegistry)
    ));
    let stats_path = path("health")
        .resolve(&registry, registry.register::<Stats>().unwrap())
        .unwrap();
    let error = view.at(&stats_path).expect_err("resolved for Stats");
    assert!(
        matches!(&error, ValueError::PathForOtherType { path_root, .. } if path_root.ends_with("Stats")),
        "{error}"
    );
}

#[test]
fn patches_apply_whole_and_their_inverse_restores() {
    let registry = TypeRegistry::new();
    let original = soldier();
    let mut value = original.clone();
    let mut patch = Patch::new();
    patch
        .set(
            path("stats.health"),
            OwnedValue::new(&registry, 5.0_f32).unwrap(),
        )
        .set(
            path("name"),
            OwnedValue::new(&registry, "Lin".to_owned()).unwrap(),
        )
        .set(
            path("placement.translation.y"),
            OwnedValue::new(&registry, -1.0_f32).unwrap(),
        )
        .set(
            path("target?"),
            OwnedValue::new(&registry, Stats::default()).unwrap(),
        )
        .set(
            path("waypoints[1]"),
            OwnedValue::new(&registry, Vec3::NEG_Z).unwrap(),
        );
    patch
        .apply(&mut registry.view_mut(&mut value).unwrap())
        .expect("valid");
    assert_eq!(value.stats.health, 5.0);
    assert_eq!(value.name, "Lin");
    assert_eq!(value.placement.translation.y, -1.0);
    assert_eq!(value.target, Some(Stats::default()));
    assert_eq!(value.waypoints[1], Vec3::NEG_Z);

    patch
        .apply(&mut registry.view_mut(&mut value).unwrap())
        .expect("the patch, now the inverse, applies");
    assert_eq!(value, original);
    assert_eq!(patch.len(), 5);
}

#[test]
fn an_invalid_patch_changes_nothing() {
    let registry = TypeRegistry::new();
    let original = soldier();
    let mut value = original.clone();
    let apply =
        |value: &mut Soldier, mut patch: Patch| patch.apply(&mut registry.view_mut(value).unwrap());

    let mut patch = Patch::new();
    patch
        .set(
            path("stats.health"),
            OwnedValue::new(&registry, 5.0_f32).unwrap(),
        )
        .set(
            path("waypoints[7]"),
            OwnedValue::new(&registry, Vec3::ONE).unwrap(),
        );
    let error = apply(&mut value, patch).expect_err("index out of bounds");
    assert!(
        matches!(&error, PatchError::Entry { path, .. } if path == "waypoints[7]"),
        "{error}"
    );
    assert_eq!(
        value, original,
        "the valid first entry wasn't applied either"
    );

    let mut patch = Patch::new();
    patch
        .set(
            path("stats"),
            OwnedValue::new(&registry, Stats::default()).unwrap(),
        )
        .set(
            path("stats.armour"),
            OwnedValue::new(&registry, 1_u16).unwrap(),
        );
    assert_eq!(
        apply(&mut value, patch).expect_err("overlap"),
        PatchError::Overlap {
            first: "stats".to_owned(),
            second: "stats.armour".to_owned(),
        }
    );
    let mut patch = Patch::new();
    patch
        .set(
            path("stats.armor"),
            OwnedValue::new(&registry, 1_u16).unwrap(),
        )
        .set(
            path("stats.armour"),
            OwnedValue::new(&registry, 2_u16).unwrap(),
        );
    assert!(
        matches!(apply(&mut value, patch), Err(PatchError::Overlap { .. })),
        "an alias is the same field"
    );
    let mut patch = Patch::new();
    patch
        .set(
            path("placement.rotation"),
            OwnedValue::new(&registry, Quat::IDENTITY).unwrap(),
        )
        .set(
            path("placement.rotation.x"),
            OwnedValue::new(&registry, 1.0_f32).unwrap(),
        );
    assert!(matches!(
        apply(&mut value, patch),
        Err(PatchError::Overlap { .. })
    ));
    let mut patch = Patch::new();
    patch.set(path("name"), OwnedValue::new(&registry, 3_u8).unwrap());
    assert!(matches!(
        apply(&mut value, patch),
        Err(PatchError::Entry {
            source: ValueError::TypeMismatch { .. },
            ..
        })
    ));
    assert_eq!(value, original);
}

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
}

#[derive(Reflect, Debug, PartialEq)]
#[reflect(transient, clone)]
struct Counted(u32);

impl Counted {
    fn new(value: u32) -> Self {
        LIVE.set(LIVE.get() + 1);
        Self(value)
    }
}

impl Clone for Counted {
    fn clone(&self) -> Self {
        Self::new(self.0)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        LIVE.set(LIVE.get() - 1);
    }
}

#[test]
fn owned_values_drop_exactly_once() {
    let registry = TypeRegistry::new();
    let before = LIVE.get();
    {
        let owned = OwnedValue::new(&registry, Counted::new(1)).unwrap();
        let copy = owned.try_clone().unwrap();
        assert_eq!(LIVE.get() - before, 2);
        assert_eq!(copy.into_value::<Counted>().unwrap().0, 1);
        let owned = owned.into_value::<String>().expect_err("not a String");
        let mut list = vec![Counted::new(2)];
        let mut view = registry.view_mut(&mut list).unwrap();
        view.push(owned).unwrap();
        let removed = view.remove(0).unwrap();
        drop(removed);
        let mut option = Some(Counted::new(3));
        let mut view = registry.view_mut(&mut option).unwrap();
        view.set_option(Some(OwnedValue::new(&registry, Counted::new(4)).unwrap()))
            .unwrap();
        assert_eq!(option.as_ref().map(|c| c.0), Some(4));
        assert_eq!(list.iter().map(|c| c.0).collect::<Vec<_>>(), [1]);
        let snapshot = registry.view(&list).unwrap().to_owned_value().unwrap();
        drop(snapshot);
    }
    assert_eq!(LIVE.get(), before, "every value was dropped exactly once");

    let empty: [Counted; 0] = [];
    let zst = OwnedValue::new(&registry, empty).unwrap();
    drop(zst);
    assert_eq!(LIVE.get(), before);
}

#[test]
fn defaults_and_wrong_registries() {
    let registry = TypeRegistry::new();
    let key = registry.register::<Soldier>().unwrap();
    let default = OwnedValue::default_of(&registry, key).unwrap();
    assert_eq!(
        default.downcast_ref::<Soldier>().unwrap(),
        &Soldier::default()
    );
    let counted = registry.register::<Counted>().unwrap();
    assert!(matches!(
        OwnedValue::default_of(&registry, counted),
        Err(ValueError::Operation(_))
    ));

    let other = TypeRegistry::new();
    assert!(matches!(
        default.view(&other),
        Err(ValueError::WrongRegistry)
    ));
    let mut value = soldier();
    let foreign_value = OwnedValue::new(&other, "x".to_owned()).unwrap();
    assert!(matches!(
        registry
            .view_mut(&mut value)
            .unwrap()
            .field_mut("name")
            .unwrap()
            .replace(foreign_value),
        Err(ValueError::WrongRegistry)
    ));
}

#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(id = "5d9f7b1e-3a6c-4e0f-8b4d-8e0a2c4e6f9b", clone)]
struct Packet {
    alive: bool,
    stance: Stance,
    pad: [u16; 2],
    value: u32,
}

fn packet_bytes(alive: u8, stance: u8, value: u32) -> Vec<u8> {
    let mut bytes = vec![0_u8; size_of::<Packet>()];
    bytes[offset_of!(Packet, alive)] = alive;
    bytes[offset_of!(Packet, stance)] = stance;
    let at = offset_of!(Packet, value);
    bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
    bytes
}

#[test]
fn bytes_are_validated_before_they_become_a_value() {
    let registry = TypeRegistry::new();
    let key = registry.register::<Packet>().unwrap();

    let valid = OwnedValue::from_bytes(&registry, key, &packet_bytes(1, 9, 77)).unwrap();
    let packet = valid.into_value::<Packet>().unwrap();
    assert_eq!(
        packet,
        Packet {
            alive: true,
            stance: Stance::Prone,
            pad: [0; 2],
            value: 77
        }
    );

    let error = OwnedValue::from_bytes(&registry, key, &packet_bytes(2, 9, 0)).expect_err("bool");
    assert!(
        matches!(&error, ValueError::InvalidBytes { ty, offset, .. }
            if ty == "bool" && *offset == offset_of!(Packet, alive)),
        "{error}"
    );
    let error = OwnedValue::from_bytes(&registry, key, &packet_bytes(1, 2, 0)).expect_err("enum");
    assert!(matches!(error, ValueError::InvalidBytes { .. }), "{error}");
    assert!(matches!(
        OwnedValue::from_bytes(&registry, key, &[0; 3]),
        Err(ValueError::WrongSize { found: 3, .. })
    ));
    let string = registry.register::<String>().unwrap();
    assert!(matches!(
        OwnedValue::from_bytes(&registry, string, &[0; size_of::<String>()]),
        Err(ValueError::NotPlainData { .. })
    ));
    let team = registry.register::<Team>().unwrap();
    assert!(
        matches!(
            OwnedValue::from_bytes(&registry, team, &[0; size_of::<Team>()]),
            Err(ValueError::NotPlainData { .. })
        ),
        "an enum without a repr has no byte representation to validate"
    );

    let mut packet = packet;
    let mut view = registry.view_mut(&mut packet).unwrap();
    assert!(view.write_bytes(&packet_bytes(3, 1, 0)).is_err());
    view.write_bytes(&packet_bytes(0, 4, 5)).unwrap();
    assert_eq!(
        (packet.alive, packet.stance.clone(), packet.value),
        (false, Stance::Crouching, 5)
    );
}

fn health_type(registry: &TypeRegistry) -> ForeignTypeDefinition {
    let f32_type = registry.key_of::<f32>().unwrap();
    let u32_type = registry.key_of::<u32>().unwrap();
    ForeignTypeDefinition::new(
        StableTypeId::from_uuid("6e0a8c2f-4b7d-4f1a-9c5e-9f1b3d5f7a0c"),
        "Script.Health",
        layout(12, 4),
    )
    .field("current", f32_type, 0)
    .field("max", f32_type, 4)
    .field("flags", u32_type, 8)
}

#[test]
fn foreign_types_are_validated_plain_data() {
    let registry = TypeRegistry::new();
    let key = registry.register_foreign(health_type(&registry)).unwrap();
    let description = registry.description(key).unwrap();
    assert!(description.is_abi() && !description.operations().needs_drop());

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&50.0_f32.to_ne_bytes());
    bytes.extend_from_slice(&100.0_f32.to_ne_bytes());
    bytes.extend_from_slice(&3_u32.to_ne_bytes());
    let mut health = OwnedValue::from_bytes(&registry, key, &bytes).unwrap();
    let copy = health.try_clone().unwrap();
    {
        let mut view = health.view_mut(&registry).unwrap();
        *view
            .field_mut("current")
            .unwrap()
            .downcast_mut::<f32>()
            .unwrap() = 25.0;
    }
    let read = |value: &OwnedValue, field: &str| {
        *value
            .view(&registry)
            .unwrap()
            .field(field)
            .unwrap()
            .downcast_ref::<f32>()
            .unwrap()
    };
    assert_eq!(
        (read(&health, "current"), read(&copy, "current")),
        (25.0, 50.0)
    );
    let zero = OwnedValue::default_of(&registry, key).unwrap();
    assert_eq!(read(&zero, "max"), 0.0);
    assert!(
        matches!(
            zero.downcast_ref::<u32>(),
            Err(ValueError::TypeMismatch { .. })
        ),
        "a foreign type has no Rust type"
    );
}

#[test]
fn malformed_foreign_descriptions_are_refused() {
    let registry = TypeRegistry::new();
    let id = |n: u8| StableTypeId::from_bytes([n; 16]);
    let key = |registry: &TypeRegistry| registry.key_of::<u32>().unwrap();
    let u32_type = key(&registry);
    let bool_type = registry.key_of::<bool>().unwrap();
    let string_type = registry.key_of::<String>().unwrap();
    let problem = |definition| match registry.register_foreign(definition) {
        Err(RegistrationError::InvalidDefinition { problem, .. }) => problem,
        other => panic!("expected an invalid definition, got {other:?}"),
    };
    let base = |n| ForeignTypeDefinition::new(id(n), "Bad", layout(8, 4));

    assert_eq!(
        problem(ForeignTypeDefinition::new(
            id(1),
            "Bad",
            layout(ForeignTypeDefinition::MAX_SIZE + 4, 4)
        )),
        DefinitionProblem::TooLarge
    );
    assert_eq!(
        problem(ForeignTypeDefinition::new(id(2), "Bad", layout(6, 4))),
        DefinitionProblem::WrongLayout,
        "size not a multiple of the alignment"
    );
    assert_eq!(
        problem(base(3).field("flag", bool_type, 0)),
        DefinitionProblem::NonAbiField("flag".to_owned()),
        "no bool in foreign types"
    );
    assert_eq!(
        problem(base(4).field("name", string_type, 0)),
        DefinitionProblem::NonAbiField("name".to_owned())
    );
    assert_eq!(
        problem(base(5).field("a", u32_type, 6)),
        DefinitionProblem::FieldOutOfBounds("a".to_owned())
    );
    assert_eq!(
        problem(base(6).field("a", u32_type, 2)),
        DefinitionProblem::MisalignedField("a".to_owned())
    );
    assert_eq!(
        problem(base(7).field("a", u32_type, 0).field("b", u32_type, 2)),
        DefinitionProblem::MisalignedField("b".to_owned())
    );
    let u16_type = registry.key_of::<u16>().unwrap();
    assert_eq!(
        problem(base(8).field("a", u32_type, 0).field("b", u16_type, 2)),
        DefinitionProblem::OverlappingFields("b".to_owned())
    );
    assert_eq!(
        problem(base(9).field("a", u32_type, 0).field("a", u32_type, 4)),
        DefinitionProblem::DuplicateField("a".to_owned())
    );
    let other = TypeRegistry::new();
    let stranger = other
        .register_foreign(ForeignTypeDefinition::new(id(10), "X", layout(4, 4)))
        .unwrap();
    let unknown = crate::TypeKey(stranger.index() + 10_000);
    assert_eq!(
        problem(base(11).field("a", unknown, 0)),
        DefinitionProblem::UnknownFieldType("a".to_owned())
    );

    registry.register_foreign(health_type(&registry)).unwrap();
    assert!(matches!(
        registry.register_foreign(health_type(&registry)),
        Err(RegistrationError::DuplicateStableId { .. })
    ));
}

#[test]
fn foreign_types_load_and_unload_with_their_group() {
    let registry = TypeRegistry::new();
    let count = registry.len();
    let mut group = registry.group("script");
    group.add_foreign(health_type(&registry));
    let id = group.commit().unwrap();
    assert_eq!(registry.len(), count + 1);
    let health_id = StableTypeId::from_uuid("6e0a8c2f-4b7d-4f1a-9c5e-9f1b3d5f7a0c");
    assert!(registry.key_by_stable_id(health_id).is_some());
    registry.remove_group(id).unwrap();
    assert_eq!(registry.key_by_stable_id(health_id), None);
    registry
        .register_foreign(health_type(&registry))
        .expect("reloads under a new key");
}

/// A deterministic generator for the randomized tests (xorshift64*), so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).expect("below n")
    }
}

/// A random edit of `soldier`: its path, and the value, valid or not.
fn random_edit(rng: &mut Rng, registry: &TypeRegistry) -> (String, OwnedValue) {
    let f = |rng: &mut Rng| (rng.below(1000) as f32) / 10.0;
    match rng.below(9) {
        0 => (
            "stats.health".into(),
            OwnedValue::new(registry, f(rng)).unwrap(),
        ),
        1 => (
            "stats.armor".into(),
            OwnedValue::new(registry, rng.below(9) as u16).unwrap(),
        ),
        2 => (
            format!("scores[{}]", rng.below(3)),
            OwnedValue::new(registry, rng.below(9) as i32).unwrap(),
        ),
        3 => (
            format!("waypoints[{}].x", rng.below(3)),
            OwnedValue::new(registry, f(rng)).unwrap(),
        ),
        4 => (
            "placement.translation.z".into(),
            OwnedValue::new(registry, f(rng)).unwrap(),
        ),
        5 => (
            "target?.armor".into(),
            OwnedValue::new(registry, 7_u16).unwrap(),
        ),
        6 => (
            "name".into(),
            OwnedValue::new(registry, format!("n{}", rng.below(99))).unwrap(),
        ),
        7 => (
            "stance".into(),
            OwnedValue::new(registry, Stance::Prone).unwrap(),
        ),
        // Invalid: a value of the wrong type.
        _ => (
            "stats.health".into(),
            OwnedValue::new(registry, 1_u8).unwrap(),
        ),
    }
}

/// Applies `path = value` to a typed copy, the way the patch should.
fn apply_typed(value: &mut Soldier, path: &str, new: &OwnedValue) -> bool {
    let f = || *new.downcast_ref::<f32>().unwrap();
    match path {
        "stats.health" => match new.downcast_ref::<f32>() {
            Ok(v) => value.stats.health = *v,
            Err(_) => return false,
        },
        "stats.armor" => value.stats.armor = *new.downcast_ref::<u16>().unwrap(),
        "placement.translation.z" => value.placement.translation.z = f(),
        "target?.armor" => match &mut value.target {
            Some(target) => target.armor = 7,
            None => return false,
        },
        "name" => value.name = new.downcast_ref::<String>().unwrap().clone(),
        "stance" => value.stance = Stance::Prone,
        _ if path.starts_with("scores[") => {
            let index = usize::from(path.as_bytes()[7] - b'0');
            value.scores[index] = *new.downcast_ref::<i32>().unwrap();
        }
        _ if path.starts_with("waypoints[") => {
            let index = usize::from(path.as_bytes()[10] - b'0');
            match value.waypoints.get_mut(index) {
                Some(waypoint) => waypoint.x = f(),
                None => return false,
            }
        }
        _ => unreachable!("{path}"),
    }
    true
}

#[test]
fn random_patches_apply_all_or_nothing_and_agree_with_typed_edits() {
    let registry = TypeRegistry::new();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let rounds = if cfg!(miri) { 12 } else { 400 };
    for round in 0..rounds {
        let mut value = soldier();
        if rng.below(3) == 0 {
            value.target = None;
        }
        let mut expected = value.clone();
        let mut patch = Patch::new();
        let mut seen = Vec::new();
        let mut valid = true;
        for _ in 0..=rng.below(4) {
            let (text, new) = random_edit(&mut rng, &registry);
            if seen.contains(&text) {
                continue;
            }
            valid &= apply_typed(&mut expected, &text, &new);
            seen.push(text.clone());
            patch.set(path(&text), new);
        }
        let before = value.clone();
        let result = patch.apply(&mut registry.view_mut(&mut value).unwrap());
        if valid {
            result.unwrap_or_else(|e| panic!("round {round}: {e}"));
            assert_eq!(value, expected, "round {round}");
            patch
                .apply(&mut registry.view_mut(&mut value).unwrap())
                .unwrap();
            assert_eq!(value, before, "round {round}: the inverse restores");
        } else {
            assert!(
                result.is_err(),
                "round {round}: an invalid entry fails the patch"
            );
            assert_eq!(value, before, "round {round}: nothing changed");
            assert_eq!(patch.len(), seen.len(), "round {round}: the patch is kept");
        }
    }
}

#[test]
fn random_paths_read_what_typed_access_reads() {
    let registry = TypeRegistry::new();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let value = soldier();
    let view = registry.view(&value).unwrap();
    let rounds = if cfg!(miri) { 12 } else { 200 };
    for _ in 0..rounds {
        let (text, expected): (String, f32) = match rng.below(4) {
            0 => ("stats.health".into(), value.stats.health),
            1 => {
                let i = rng.below(2);
                (format!("waypoints[{i}].y"), value.waypoints[i].y)
            }
            2 => (
                "placement.translation.x".into(),
                value.placement.translation.x,
            ),
            _ => (
                "target?.health".into(),
                value.target.as_ref().unwrap().health,
            ),
        };
        let read = view.read_path(&path(&text)).unwrap();
        assert_eq!(read.into_value::<f32>().unwrap(), expected, "{text}");
    }
}

#[test]
fn views_and_owned_values_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<crate::ValueRef<'_>>();
    check::<crate::ValueMut<'_>>();
    check::<OwnedValue>();
}

fn layout(size: usize, align: usize) -> std::alloc::Layout {
    std::alloc::Layout::from_size_align(size, align).expect("test layouts are valid")
}

// --- Regression tests from the part 2 reviews ---

#[repr(u8)]
enum Small {
    X,
}

impl crate::UnitEnum for Small {
    fn variant_index(&self) -> usize {
        0
    }

    fn from_variant_index(index: usize) -> Option<Self> {
        (index == 0).then_some(Self::X)
    }
}

// SAFETY: deliberately wrong (its enum accessors are `Team`'s): registration must refuse it
// before any view could write a `Team` into a `Small`.
unsafe impl Reflect for Small {
    const TRANSIENT: bool = true;
    const ABI: bool = false;
    const DEFINITION: crate::TypeDefinition = crate::TypeDefinition::builder::<Self>(
        "Small",
        crate::Persistence::Transient,
        crate::KindDefinition::unit_enum::<Team>(None, &[crate::VariantDefinition::new("X")]),
    )
    .build();
}

#[test]
fn enum_accessors_of_another_type_are_refused() {
    let registry = TypeRegistry::new();
    let error = registry
        .register::<Small>()
        .expect_err("accessors for Team");
    assert!(
        matches!(
            &error,
            RegistrationError::InvalidDefinition {
                problem: DefinitionProblem::WrongType,
                ..
            }
        ),
        "{error}"
    );
    let Small::X = Small::X;
}

#[test]
fn a_failed_patch_is_kept_whole() {
    let registry = TypeRegistry::new();
    let mut value = soldier();
    let mut patch = Patch::new();
    patch
        .set(
            path("name"),
            OwnedValue::new(&registry, "Kept".to_owned()).unwrap(),
        )
        .set(
            path("waypoints[5]"),
            OwnedValue::new(&registry, Vec3::ONE).unwrap(),
        );
    assert!(
        patch
            .apply(&mut registry.view_mut(&mut value).unwrap())
            .is_err()
    );
    assert_eq!(patch.len(), 2, "nothing was lost");
    let (_, name) = patch.entries().next().unwrap();
    assert_eq!(name.downcast_ref::<String>().unwrap(), "Kept");

    // Once the target fits, the same patch applies.
    value.waypoints.extend([Vec3::ZERO; 4]);
    patch
        .apply(&mut registry.view_mut(&mut value).unwrap())
        .unwrap();
    assert_eq!(value.name, "Kept");
    assert_eq!(value.waypoints[5], Vec3::ONE);
}

#[derive(Reflect, Debug, PartialEq, Clone)]
#[reflect(id = "7f1b9d3a-5c8e-4a2b-8d6f-0a2c4e6a8b1d", clone)]
struct PluginData {
    values: Vec<Vec3>,
}

#[test]
fn a_resolved_path_still_works_after_its_group_is_removed() {
    // Applying a resolved path uses the descriptions it was resolved with, so a registry change
    // between validating and applying a patch can't make it fail halfway.
    let registry = TypeRegistry::new();
    let mut group = registry.group("plugin");
    group.add::<PluginData>();
    let id = group.commit().unwrap();
    let key = registry.key_of::<PluginData>().unwrap();
    let resolved = path("values[0].y").resolve(&registry, key).unwrap();

    let mut data = PluginData {
        values: vec![Vec3::ZERO],
    };
    let mut view = registry.view_mut(&mut data).unwrap();
    registry.remove_group(id).unwrap();
    let old = view
        .replace_at(&resolved, OwnedValue::new(&registry, 2.0_f32).unwrap())
        .unwrap();
    assert_eq!(old.into_value::<f32>().unwrap(), 0.0);
    assert_eq!(data.values[0].y, 2.0);
}

#[test]
fn errors_name_the_alternatives_and_remedies() {
    let registry = TypeRegistry::new();
    let value = soldier();
    let view = registry.view(&value).unwrap();
    let error = view.field("stats").unwrap().field("hp").expect_err("no hp");
    assert_eq!(
        error.to_string(),
        format!(
            "'{}::Stats' has no field 'hp'; its fields are: health, armor",
            module_path!()
        )
    );
    let error = path("placement.translation.x.y")
        .resolve(&registry, registry.key_of::<Soldier>().unwrap())
        .expect_err("x has no components");
    assert_eq!(
        error.to_string(),
        "after the math component 'x' (a 'f32'), a path can only name that value's own components"
    );
    let mut none = soldier();
    none.target = None;
    let error = registry
        .view(&none)
        .unwrap()
        .path(&path("target?.armor"))
        .expect_err("None");
    assert!(error.to_string().contains("set it to Some"), "{error}");

    let error = "a[01]".parse::<FieldPath>().expect_err("leading zero");
    assert_eq!(
        (error.position(), error.problem()),
        (2, "expected an index such as [0] or [12]")
    );
}

#[test]
fn foreign_field_names_must_be_path_names() {
    let registry = TypeRegistry::new();
    let u32_type = registry.key_of::<u32>().unwrap();
    for (n, name) in ["", "a.b", "hp[0]", "é"].into_iter().enumerate() {
        let definition = ForeignTypeDefinition::new(
            StableTypeId::from_bytes([u8::try_from(n).unwrap() + 100; 16]),
            "Named",
            layout(4, 4),
        )
        .field(name, u32_type, 0);
        assert_eq!(
            registry.register_foreign(definition),
            Err(RegistrationError::InvalidDefinition {
                ty: "Named".to_owned(),
                problem: DefinitionProblem::InvalidFieldName(name.to_owned()),
            }),
            "{name:?}"
        );
    }
}

#[test]
fn a_refused_write_of_bytes_leaves_the_value_unchanged() {
    let registry = TypeRegistry::new();
    let mut packet = Packet {
        alive: true,
        stance: Stance::Prone,
        pad: [1, 2],
        value: 9,
    };
    let before = packet.clone();
    let mut view = registry.view_mut(&mut packet).unwrap();
    assert!(view.write_bytes(&packet_bytes(7, 1, 0)).is_err());
    assert_eq!(packet, before);
}

/// Panics when dropped with the value 1.
#[derive(Reflect)]
#[reflect(transient)]
struct Boom(u32);

impl Drop for Boom {
    fn drop(&mut self) {
        assert!(self.0 != 1, "drop of Boom(1) panics");
    }
}

#[derive(Reflect, Clone, Default, Debug, PartialEq)]
#[reflect(transient, clone, default)]
struct Empty;

#[derive(Reflect)]
#[reflect(transient)]
struct Holder {
    lists: Vec<Vec<String>>,
    boom: Option<Boom>,
    empties: Vec<Empty>,
    array: [Empty; 3],
}

#[test]
fn zero_sized_values_nested_lists_and_panicking_drops() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let registry = TypeRegistry::new();

    // Zero-sized owned values.
    let empty = OwnedValue::new(&registry, Empty).unwrap();
    assert_eq!(
        empty.try_clone().unwrap().into_value::<Empty>().unwrap(),
        Empty
    );
    let default = OwnedValue::default_of(&registry, registry.key_of::<Empty>().unwrap()).unwrap();

    let mut holder = Holder {
        lists: vec![vec!["a".into()]],
        boom: Some(Boom(1)),
        empties: vec![Empty, Empty],
        array: [Empty, Empty, Empty],
    };
    {
        let mut view = registry.view_mut(&mut holder).unwrap();
        {
            let mut empties = view.field_mut("empties").unwrap();
            empties.push(default).unwrap();
            let old = empties.element_mut(2).unwrap().replace(empty).unwrap();
            drop(old);
            drop(empties.remove(0).unwrap());
            let mut array = view.field_mut("array").unwrap();
            array
                .element_mut(2)
                .unwrap()
                .downcast_mut::<Empty>()
                .unwrap();
        }
        {
            // A child of a list element, then pushes that reallocate the list, then the child
            // again.
            let mut lists = view.field_mut("lists").unwrap();
            {
                let mut inner = lists.element_mut(0).unwrap();
                inner
                    .push(OwnedValue::new(&registry, "b".to_owned()).unwrap())
                    .unwrap();
            }
            for _ in 0..8 {
                lists
                    .push(OwnedValue::new(&registry, vec!["x".to_owned()]).unwrap())
                    .unwrap();
            }
            let mut inner = lists.element_mut(0).unwrap();
            inner
                .element_mut(1)
                .unwrap()
                .downcast_mut::<String>()
                .unwrap()
                .push('!');
        }
        // Dropping the old payload panics: the new payload is in place, and nothing leaks.
        let panicked = catch_unwind(AssertUnwindSafe(|| {
            view.field_mut("boom")
                .unwrap()
                .set_option(Some(OwnedValue::new(&registry, Boom(2)).unwrap()))
                .unwrap();
        }))
        .is_err();
        assert!(panicked);
    }
    assert_eq!(holder.lists[0][1], "b!");
    assert_eq!(holder.boom.as_ref().map(|b| b.0), Some(2));

    // An owned value whose drop panics still frees its allocation.
    let boom = OwnedValue::new(&registry, Boom(1)).unwrap();
    assert!(catch_unwind(AssertUnwindSafe(|| drop(boom))).is_err());
    let mut list = vec![Boom(1), Boom(0)];
    let removed = registry.view_mut(&mut list).unwrap().remove(0).unwrap();
    assert!(catch_unwind(AssertUnwindSafe(|| drop(removed))).is_err());
}
