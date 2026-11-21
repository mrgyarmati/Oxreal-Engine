//! Tests of the derive, the registry and the erased operations together.

use std::alloc::Layout;
use std::cell::Cell;
use std::mem::{MaybeUninit, offset_of, size_of};
use std::sync::Arc;

use oxreal_math::{Quat, Transform, Vec3};

use crate::id::builtin;
use crate::{
    AttributesDefinition, DefinitionProblem, FieldDefinition, GenericIdInput, GroupId, Identity,
    IntRepr, KindDefinition, Persistence, Primitive, Range, Reflect, RegistrationError,
    StableTypeId, TypeDefinition, TypeDescription, TypeKind, TypeRegistry, Unit,
    UnsupportedOperation,
};

fn described<T: Reflect>(registry: &TypeRegistry) -> Arc<TypeDescription> {
    let key = registry.register::<T>().expect("valid test type");
    registry.description(key).expect("just registered")
}

fn name_of(registry: &TypeRegistry, key: crate::TypeKey) -> String {
    registry
        .description(key)
        .expect("registered")
        .name()
        .to_owned()
}

/// A movement setting.
///
/// More text that isn't the tooltip.
#[derive(Reflect, Clone, Default, Debug, PartialEq)]
#[reflect(
    id = "c1a5e2f0-7d3b-4e8a-9c6f-0b2d4e6f8a1c",
    clone,
    default,
    category = "Gameplay"
)]
struct Movement {
    /// Top speed.
    #[reflect(range(min = 0, max = 20.5, step = 0.5), units = MetresPerSecond)]
    max_speed: f32,
    #[reflect(
        display_name = "Turn rate",
        alias = "turn",
        alias = "yaw_rate",
        read_only
    )]
    turn_rate: f64,
    name: String,
    #[reflect(transient)]
    current_speed: f32,
    waypoints: Vec<Vec3>,
    #[reflect(hidden, tooltip = "Overrides the doc")]
    /// Not this.
    flags: u8,
}

#[derive(Reflect)]
#[reflect(id = "d2b6f3a1-8e4c-4f9b-8d7a-1c3e5f7a9b2d", name = "test::Pair")]
struct Pair(u32, #[reflect(transient)] f32);

#[derive(Reflect)]
#[reflect(transient)]
struct Marker;

#[derive(Reflect, Clone, Copy, PartialEq, Eq, Debug)]
#[reflect(id = "e3c7a4b2-9f5d-4a0c-9e8b-2d4f6a8b0c3e")]
#[repr(u8)]
enum Mode {
    Walk = 3,
    #[reflect(alias = "Sprint")]
    Run,
    Fly = 200,
}

#[derive(Reflect)]
#[reflect(id = "f4d8b5c3-0a6e-4b1d-8f9c-3e5a7b9c1d4f")]
enum Weather {
    Clear,
    /// Rain.
    Rain,
}

#[derive(Reflect)]
#[reflect(id = "a5e9c6d4-1b7f-4c2e-9a0d-4f6b8c0d2e5a")]
struct Wrapper<T, const N: usize> {
    items: [T; N],
}

#[derive(Reflect)]
#[reflect(id = "b6f0d7e5-2c8a-4d3f-8b1e-5a7c9d1e3f6b")]
struct Node {
    value: i32,
    children: Vec<Node>,
}

#[derive(Reflect, Clone, Copy)]
#[reflect(id = "c7a1e8f6-3d9b-4e4a-9c2f-6b8d0e2f4a7c", abi)]
#[repr(C)]
struct Vertex {
    position: [f32; 3],
    color: u32,
}

#[derive(Reflect, Default)]
#[reflect(id = "d8b2f9a7-4e0c-4f5b-8d3a-7c9e1f3a5b8d", default)]
struct WithOpaque {
    id: u64,
    #[reflect(opaque, transient)]
    handle: ExternalHandle,
}

/// A type without `Reflect`, standing in for a third-party field type.
#[derive(Default)]
struct ExternalHandle(u64);

#[test]
fn derived_offsets_and_sizes_match_the_compiler() {
    let registry = TypeRegistry::new();
    let movement = described::<Movement>(&registry);
    assert_eq!(movement.layout(), Layout::new::<Movement>());
    let info = movement.as_struct().expect("a struct");
    let offsets: Vec<usize> = info.fields().iter().map(crate::Field::offset).collect();
    assert_eq!(
        offsets,
        [
            offset_of!(Movement, max_speed),
            offset_of!(Movement, turn_rate),
            offset_of!(Movement, name),
            offset_of!(Movement, current_speed),
            offset_of!(Movement, waypoints),
            offset_of!(Movement, flags),
        ]
    );

    let pair = described::<Pair>(&registry);
    let info = pair.as_struct().expect("a struct");
    assert!(info.is_tuple());
    assert_eq!(info.fields()[0].name(), "0");
    assert_eq!(info.fields()[1].offset(), offset_of!(Pair, 1));
    assert_eq!(pair.layout().size(), size_of::<Pair>());
}

#[test]
fn derived_names_attributes_and_aliases() {
    let registry = TypeRegistry::new();
    let movement = described::<Movement>(&registry);
    assert_eq!(movement.name(), concat!(module_path!(), "::Movement"));
    assert_eq!(movement.attributes().category(), Some("Gameplay"));
    assert_eq!(movement.attributes().tooltip(), Some("A movement setting."));

    let info = movement.as_struct().expect("a struct");
    let (_, speed) = info.field("max_speed").expect("declared");
    assert_eq!(speed.attributes().tooltip(), Some("Top speed."));
    assert_eq!(
        speed.attributes().range(),
        Some(Range::new(0.0, 20.5).with_step(0.5))
    );
    assert_eq!(speed.attributes().units(), Some(Unit::MetresPerSecond));
    assert_eq!(name_of(&registry, speed.ty().expect("not opaque")), "f32");

    let (index, turn) = info.field("yaw_rate").expect("found by alias");
    assert_eq!((index, turn.name()), (1, "turn_rate"));
    assert_eq!(turn.attributes().display_name(), Some("Turn rate"));
    assert!(turn.attributes().is_read_only());

    let (_, flags) = info.field("flags").expect("declared");
    assert!(flags.attributes().is_hidden());
    assert_eq!(flags.attributes().tooltip(), Some("Overrides the doc"));

    assert!(
        info.field("current_speed")
            .expect("declared")
            .1
            .is_transient()
    );
    assert_eq!(described::<Pair>(&registry).name(), "test::Pair");
}

#[test]
fn unit_enums_carry_discriminants_only_with_a_repr() {
    let registry = TypeRegistry::new();
    let mode = described::<Mode>(&registry);
    let TypeKind::UnitEnum(info) = mode.kind() else {
        panic!("{:?}", mode.kind());
    };
    assert_eq!(info.repr(), Some(IntRepr::U8));
    let discriminants: Vec<_> = info
        .variants()
        .iter()
        .map(crate::Variant::discriminant)
        .collect();
    assert_eq!(discriminants, [Some(3), Some(4), Some(200)]);
    assert_eq!(info.variant("Sprint").expect("an alias").1.name(), "Run");

    let weather = described::<Weather>(&registry);
    let TypeKind::UnitEnum(info) = weather.kind() else {
        panic!("{:?}", weather.kind());
    };
    assert_eq!(info.repr(), None);
    assert!(info.variants().iter().all(|v| v.discriminant().is_none()));
    assert_eq!(info.variants()[1].attributes().tooltip(), Some("Rain."));
}

#[test]
fn generic_instantiations_get_derived_ids_and_names() {
    let registry = TypeRegistry::new();
    let wrapper = described::<Wrapper<f32, 4>>(&registry);
    let definition = StableTypeId::from_uuid("a5e9c6d4-1b7f-4c2e-9a0d-4f6b8c0d2e5a");
    assert_eq!(
        wrapper.stable_id(),
        Some(StableTypeId::for_instantiation(
            definition,
            &[
                GenericIdInput::Type(builtin::F32),
                GenericIdInput::Const(crate::ConstArg::usize(4)),
            ],
        ))
    );
    assert_eq!(wrapper.name(), concat!(module_path!(), "::Wrapper<f32, 4>"));
    let items = wrapper.as_struct().expect("a struct").fields()[0]
        .ty()
        .expect("reflected");
    let array = registry
        .description(items)
        .expect("registered with its user");
    assert_eq!(array.name(), "[f32; 4]");
    assert!(array.is_abi(), "arrays of ABI-safe elements are ABI-safe");

    let vec_f32 = described::<Vec<f32>>(&registry);
    assert_eq!(vec_f32.name(), "Vec<f32>");
    assert_eq!(
        vec_f32.stable_id().map(|id| id.to_string()).as_deref(),
        Some("4fdf80ff-0bbd-6cde-61d4-18b31a945d77"),
        "the checked-in vector"
    );
    assert_eq!(
        described::<Option<String>>(&registry).name(),
        "Option<String>"
    );
}

#[test]
fn an_instantiation_with_a_transient_argument_is_transient() {
    let registry = TypeRegistry::new();
    let wrapper = described::<Wrapper<Marker, 2>>(&registry);
    assert_eq!(wrapper.identity(), Identity::Transient);
    const { assert!(<Wrapper<Marker, 2> as Reflect>::TRANSIENT) };
    assert_eq!(
        described::<Vec<Marker>>(&registry).identity(),
        Identity::Transient
    );
}

#[test]
fn a_recursive_type_registers_through_its_container_first() {
    // Registering `Vec<Node>` registers `Node`, whose field registers `Vec<Node>` again.
    let registry = TypeRegistry::new();
    let list = described::<Vec<Node>>(&registry);
    assert_eq!(registry.key_of::<Vec<Node>>(), Some(list.key()));
    assert!(registry.key_of::<Node>().is_some());
}

#[test]
fn recursive_types_register() {
    let registry = TypeRegistry::new();
    let node = described::<Node>(&registry);
    let children = node.as_struct().expect("a struct").fields()[1]
        .ty()
        .expect("reflected");
    let TypeKind::List(list) = registry
        .description(children)
        .expect("registered")
        .kind()
        .clone()
    else {
        panic!("Vec<Node> is a list");
    };
    assert_eq!(
        list.element(),
        node.key(),
        "the element type is Node itself"
    );
}

#[test]
fn abi_types_register_with_their_flag() {
    let registry = TypeRegistry::new();
    assert!(described::<Vertex>(&registry).is_abi());
    const {
        assert!(<Vertex as Reflect>::ABI);
        assert!(!<bool as Reflect>::ABI && <u32 as Reflect>::ABI && <f64 as Reflect>::ABI);
        assert!(
            !<Vec3 as Reflect>::ABI,
            "math types use the C ABI's own structs"
        );
    }
}

#[test]
fn opaque_fields_have_no_type() {
    let registry = TypeRegistry::new();
    let with_opaque = described::<WithOpaque>(&registry);
    let (_, handle) = with_opaque
        .as_struct()
        .expect("a struct")
        .field("handle")
        .expect("declared");
    assert!(handle.is_opaque());
    assert_eq!(handle.offset(), offset_of!(WithOpaque, handle));
    assert_eq!(WithOpaque::default().handle.0, 0);
}

#[test]
fn every_builtin_type_is_registered_with_its_id() {
    let registry = TypeRegistry::new();
    for id in crate::id::tests::builtin_ids() {
        let generic_definition = [builtin::VEC, builtin::OPTION, builtin::ARRAY].contains(&id);
        assert_eq!(
            registry.key_by_stable_id(id).is_some(),
            !generic_definition,
            "{id}: non-generic built-ins are registered, generic definitions aren't types"
        );
    }
    let transform = registry.description_of::<Transform>().expect("built in");
    let TypeKind::Math(info) = transform.kind() else {
        panic!("{:?}", transform.kind());
    };
    let names: Vec<_> = info
        .components()
        .iter()
        .map(crate::MathComponent::name)
        .collect();
    assert_eq!(names, ["translation", "rotation", "scale"]);
    assert_eq!(name_of(&registry, info.components()[1].ty()), "Quat");
    assert_eq!(
        registry.description_of::<f32>().expect("built in").kind(),
        &TypeKind::Primitive(Primitive::F32)
    );
}

#[test]
fn registering_twice_returns_the_same_key() {
    let registry = TypeRegistry::new();
    let first = registry.register::<Movement>().expect("valid");
    let count = registry.len();
    assert_eq!(registry.register::<Movement>(), Ok(first));
    assert_eq!(registry.len(), count);
    assert_eq!(registry.key_of::<Movement>(), Some(first));
}

#[test]
fn registries_have_distinct_identities() {
    assert_ne!(TypeRegistry::new().id(), TypeRegistry::new().id());
}

// Two types that copy-paste one ID, one of them needing new field types registered.
#[derive(Reflect)]
#[reflect(id = "e9c3a0b8-5f1d-4a6c-9e4b-8d0f2a4b6c9e")]
struct Original;

#[derive(Reflect)]
#[reflect(id = "e9c3a0b8-5f1d-4a6c-9e4b-8d0f2a4b6c9e")]
struct CopyPasted {
    values: Vec<i16>,
}

#[test]
fn a_duplicate_stable_id_is_refused_and_nothing_is_registered() {
    let registry = TypeRegistry::new();
    registry.register::<Original>().expect("first of the two");
    let count = registry.len();
    let error = registry.register::<CopyPasted>().expect_err("same ID");
    let RegistrationError::DuplicateStableId { existing, new, .. } = &error else {
        panic!("{error}");
    };
    assert!(
        existing.ends_with("Original") && new.ends_with("CopyPasted"),
        "{error}"
    );
    assert_eq!(
        registry.len(),
        count,
        "all or nothing: Vec<i16> wasn't kept either"
    );
    assert_eq!(registry.key_of::<Vec<i16>>(), None);
    assert_eq!(registry.key_of::<CopyPasted>(), None);
}

/// A definition the derive would refuse at compile time, written by hand to reach the
/// registry's own check.
struct SavedWithTransientField {
    marker: Marker,
}

// SAFETY: built with `builder::<Self>`; its one field is described with its real type and
// offset.
unsafe impl Reflect for SavedWithTransientField {
    const TRANSIENT: bool = false;
    const ABI: bool = false;
    const DEFINITION: TypeDefinition = TypeDefinition::builder::<Self>(
        "SavedWithTransientField",
        Persistence::Persisted(StableTypeId::from_uuid(
            "f0d4b1c9-6a2e-4b7d-8f5c-9e1a3b5c7d0f",
        )),
        KindDefinition::named_struct(&[FieldDefinition::new::<Marker>(
            "marker",
            offset_of!(SavedWithTransientField, marker),
        )]),
    )
    .build();
}

#[test]
fn a_saved_type_with_a_transient_field_type_is_refused() {
    let registry = TypeRegistry::new();
    let error = registry
        .register::<SavedWithTransientField>()
        .expect_err("transient field type");
    assert!(
        matches!(&error, RegistrationError::TransientFieldType { field, .. } if field == "marker"),
        "{error}"
    );
    let _ = SavedWithTransientField { marker: Marker }.marker;
}

#[derive(Reflect)]
#[reflect(id = "a1e5c2d0-7b3f-4c8e-9a6d-0f2b4c6d8e1a")]
struct NoDefault {
    value: u8,
}

#[derive(Reflect)]
#[reflect(id = "b2f6d3e1-8c4a-4d9f-8b7e-1a3c5d7e9f2b")]
struct TransientWithoutDefault {
    #[reflect(transient)]
    cached: NoDefault,
}

#[test]
fn a_transient_field_needs_a_defaultable_type() {
    let registry = TypeRegistry::new();
    let error = registry
        .register::<TransientWithoutDefault>()
        .expect_err("no default for the field");
    assert!(
        matches!(&error, RegistrationError::TransientFieldWithoutDefault { field, .. } if field == "cached"),
        "{error}"
    );
}

/// A hand-written impl whose `TRANSIENT` contradicts its definition.
struct Contradictory;

// SAFETY: built with `builder::<Self>`; a struct without fields. (`TRANSIENT` is deliberately
// wrong: registration must refuse it.)
unsafe impl Reflect for Contradictory {
    const TRANSIENT: bool = true;
    const ABI: bool = false;
    const DEFINITION: TypeDefinition = TypeDefinition::builder::<Self>(
        "Contradictory",
        Persistence::Persisted(StableTypeId::from_uuid(
            "c3a7e4f2-9d5b-4e0a-9c8f-2b4d6e8f0a3c",
        )),
        KindDefinition::named_struct(&[]),
    )
    .build();
}

/// A hand-written impl built for another type.
struct WrongType;

// SAFETY: deliberately wrong (built for `u32`): registration must refuse it before anything
// trusts it.
unsafe impl Reflect for WrongType {
    const TRANSIENT: bool = false;
    const ABI: bool = false;
    const DEFINITION: TypeDefinition = TypeDefinition::builder::<u32>(
        "WrongType",
        Persistence::Persisted(StableTypeId::from_uuid(
            "d4b8f5a3-0e6c-4f1b-8d9a-3c5e7f9a1b4d",
        )),
        KindDefinition::named_struct(&[]),
    )
    .build();
}

#[test]
fn contradictory_hand_written_definitions_are_refused() {
    let registry = TypeRegistry::new();
    let problem = |error| match error {
        RegistrationError::InvalidDefinition { problem, .. } => problem,
        other => panic!("{other}"),
    };
    assert_eq!(
        problem(
            registry
                .register::<Contradictory>()
                .expect_err("flag mismatch")
        ),
        DefinitionProblem::FlagMismatch
    );
    assert_eq!(
        problem(registry.register::<WrongType>().expect_err("wrong type")),
        DefinitionProblem::WrongType
    );
}

/// A hand-written enum definition with a repeated discriminant.
#[repr(u8)]
enum Repeated {
    A = 1,
}

// SAFETY: deliberately wrong (two variants claim discriminant 1): registration must refuse it.
unsafe impl Reflect for Repeated {
    const TRANSIENT: bool = false;
    const ABI: bool = false;
    const DEFINITION: TypeDefinition = TypeDefinition::builder::<Self>(
        "Repeated",
        Persistence::Persisted(StableTypeId::from_uuid(
            "e5c9a6b4-1f7d-4a2c-9e0b-4d6f8a0b2c5e",
        )),
        KindDefinition::unit_enum(
            Some(IntRepr::U8),
            &[
                crate::VariantDefinition::with_discriminant("A", Repeated::A as u32),
                crate::VariantDefinition::with_discriminant("B", 1),
            ],
        ),
    )
    .attributes(AttributesDefinition::new().range(Range::new(0.0, 1.0)))
    .build();
}

#[test]
fn invalid_enum_definitions_are_refused() {
    let registry = TypeRegistry::new();
    let error = registry
        .register::<Repeated>()
        .expect_err("repeated discriminant");
    assert!(
        matches!(
            &error,
            RegistrationError::InvalidDefinition {
                problem: DefinitionProblem::InvalidDiscriminant(variant),
                ..
            } if variant == "B"
        ),
        "{error}"
    );
}

#[test]
fn groups_become_visible_on_commit_and_leave_tombstones() {
    let registry = TypeRegistry::new();
    let mut group = registry.group("plugin");
    group.add::<Node>().add::<Weather>();
    assert_eq!(
        registry.key_of::<Node>(),
        None,
        "nothing is visible before commit"
    );
    let id = group.commit().expect("valid types");

    let node = registry.key_of::<Node>().expect("committed");
    let vec_node = registry
        .key_of::<Vec<Node>>()
        .expect("registered with Node");
    assert_eq!(registry.description(node).expect("live").group(), id);
    assert_eq!(registry.description(vec_node).expect("live").group(), id);
    let i32_key = registry.key_of::<i32>().expect("built in");
    assert_eq!(
        registry.description(i32_key).expect("live").group(),
        GroupId::ROOT,
        "already-registered field types stay where they are"
    );

    registry.remove_group(id).expect("nothing outside uses it");
    assert!(
        registry.description(node).is_none(),
        "the key is a tombstone"
    );
    assert_eq!(registry.key_of::<Node>(), None);
    assert_eq!(
        registry.key_by_stable_id(StableTypeId::from_uuid(
            "b6f0d7e5-2c8a-4d3f-8b1e-5a7c9d1e3f6b"
        )),
        None
    );
    assert_eq!(
        registry.remove_group(id),
        Err(RegistrationError::UnknownGroup { group: id })
    );

    // Reloading registers the same ID again, under a new key.
    let reloaded = registry.register::<Node>().expect("the ID is free again");
    assert!(reloaded > node, "removed keys are never reused");
}

#[test]
fn a_group_in_use_outside_it_isnt_removed() {
    let registry = TypeRegistry::new();
    let mut group = registry.group("plugin");
    group.add::<Vec<u16>>();
    let id = group.commit().expect("valid");
    registry
        .register::<Option<Vec<u16>>>()
        .expect("uses the group's type");

    let error = registry.remove_group(id).expect_err("in use");
    assert!(
        matches!(&error, RegistrationError::GroupInUse { ty, used_by, .. }
            if ty == "Vec<u16>" && used_by == "Option<Vec<u16>>"),
        "{error}"
    );
    assert!(
        registry.key_of::<Vec<u16>>().is_some(),
        "nothing was removed"
    );
    assert_eq!(
        registry.remove_group(GroupId::ROOT),
        Err(RegistrationError::RootGroup)
    );
}

#[test]
fn a_failed_group_commit_registers_nothing() {
    let registry = TypeRegistry::new();
    registry.register::<Original>().expect("first");
    let count = registry.len();
    let mut group = registry.group("broken");
    group.add::<Node>().add::<CopyPasted>();
    assert!(matches!(
        group.commit(),
        Err(RegistrationError::DuplicateStableId { .. })
    ));
    assert_eq!(registry.len(), count);
    assert_eq!(registry.key_of::<Node>(), None);
}

#[test]
fn concurrent_registration_agrees_on_keys() {
    let registry = TypeRegistry::new();
    let keys: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let registry = &registry;
                scope.spawn(move || {
                    // Half the threads register in the other order.
                    if i % 2 == 0 {
                        (registry.register::<Movement>(), registry.register::<Node>())
                    } else {
                        let node = registry.register::<Node>();
                        (registry.register::<Movement>(), node)
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("no panics"))
            .collect()
    });
    assert!(keys.windows(2).all(|pair| pair[0] == pair[1]), "{keys:?}");
    assert!(keys[0].0.is_ok() && keys[0].1.is_ok());
}

// --- Erased operations (run under Miri) ---

#[test]
fn erased_clone_default_and_drop_of_a_derived_type() {
    let registry = TypeRegistry::new();
    let movement = described::<Movement>(&registry);
    let ops = movement.operations();
    assert!(ops.needs_drop() && ops.is_cloneable() && ops.is_defaultable());

    let source = Movement {
        max_speed: 4.0,
        name: "walker".to_owned(),
        waypoints: vec![Vec3::X, Vec3::Y],
        ..Movement::default()
    };
    let mut copy = MaybeUninit::<Movement>::uninit();
    // SAFETY: `source` is a valid `Movement`; `copy` is aligned, writable and uninitialized.
    unsafe {
        ops.clone_into(std::ptr::from_ref(&source).cast(), copy.as_mut_ptr().cast())
            .expect("cloneable");
    }
    // SAFETY: `clone_into` initialized `copy`.
    let copy = unsafe { copy.assume_init() };
    assert_eq!(copy, source);

    let mut default = MaybeUninit::<Movement>::uninit();
    // SAFETY: `default` is aligned, writable and uninitialized.
    unsafe {
        ops.default_into(default.as_mut_ptr().cast())
            .expect("defaultable")
    };
    // SAFETY: `default_into` initialized `default`; it's dropped through the erased drop and
    // never used again.
    unsafe { ops.drop_in_place(default.as_mut_ptr().cast()) };
}

#[test]
fn unsupported_operations_are_errors() {
    let registry = TypeRegistry::new();
    let node = described::<Node>(&registry);
    let mut target = MaybeUninit::<Node>::uninit();
    let source = Node {
        value: 1,
        children: Vec::new(),
    };
    // SAFETY: valid source, aligned uninitialized target; nothing is written on error.
    let result = unsafe {
        node.operations().clone_into(
            std::ptr::from_ref(&source).cast(),
            target.as_mut_ptr().cast(),
        )
    };
    assert_eq!(result, Err(UnsupportedOperation::Clone));
    // SAFETY: as above.
    let result = unsafe { node.operations().default_into(target.as_mut_ptr().cast()) };
    assert_eq!(result, Err(UnsupportedOperation::Default));
    assert_eq!(source.value, 1);
}

#[test]
fn erased_container_clones_copy_every_element() {
    let registry = TypeRegistry::new();

    let list = described::<Vec<String>>(&registry);
    let source = vec!["a".to_owned(), "bc".to_owned(), String::new()];
    let mut copy = MaybeUninit::<Vec<String>>::uninit();
    // SAFETY: valid source; aligned, uninitialized target.
    unsafe {
        list.operations()
            .clone_into(std::ptr::from_ref(&source).cast(), copy.as_mut_ptr().cast())
            .expect("String is cloneable");
    }
    // SAFETY: initialized by `clone_into`.
    assert_eq!(unsafe { copy.assume_init() }, source);

    let option = described::<Option<Vec<String>>>(&registry);
    for source in [None, Some(vec!["x".to_owned()])] {
        let mut copy = MaybeUninit::<Option<Vec<String>>>::uninit();
        // SAFETY: valid source; aligned, uninitialized target.
        unsafe {
            option
                .operations()
                .clone_into(std::ptr::from_ref(&source).cast(), copy.as_mut_ptr().cast())
                .expect("cloneable");
        }
        // SAFETY: initialized by `clone_into`.
        assert_eq!(unsafe { copy.assume_init() }, source);
    }

    let array = described::<[String; 3]>(&registry);
    let source = ["1".to_owned(), "2".to_owned(), "3".to_owned()];
    let mut copy = MaybeUninit::<[String; 3]>::uninit();
    // SAFETY: valid source; aligned, uninitialized target.
    unsafe {
        array
            .operations()
            .clone_into(std::ptr::from_ref(&source).cast(), copy.as_mut_ptr().cast())
            .expect("cloneable");
    }
    // SAFETY: initialized by `clone_into`.
    assert_eq!(unsafe { copy.assume_init() }, source);

    let mut default = MaybeUninit::<[Vec<u8>; 2]>::uninit();
    let defaults = described::<[Vec<u8>; 2]>(&registry);
    // SAFETY: aligned, uninitialized target.
    unsafe {
        defaults
            .operations()
            .default_into(default.as_mut_ptr().cast())
            .expect("defaultable")
    };
    // SAFETY: initialized by `default_into`.
    let default = unsafe { default.assume_init() };
    assert_eq!(default, [Vec::<u8>::new(), Vec::new()]);

    assert!(
        !described::<Vec<Node>>(&registry)
            .operations()
            .is_cloneable(),
        "a list of uncloneable elements isn't cloneable"
    );
    assert!(
        described::<Vec<Node>>(&registry)
            .operations()
            .is_defaultable()
    );
    assert!(
        !described::<Option<Node>>(&registry)
            .operations()
            .is_cloneable()
    );
}

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static CLONES_LEFT: Cell<usize> = const { Cell::new(usize::MAX) };
}

/// Counts live values, and panics on clone once `CLONES_LEFT` runs out.
#[derive(Reflect, Debug)]
#[reflect(transient, clone, default)]
struct Tracked(u32);

impl Tracked {
    fn new(value: u32) -> Self {
        LIVE.set(LIVE.get() + 1);
        Self(value)
    }
}

impl Clone for Tracked {
    fn clone(&self) -> Self {
        let left = CLONES_LEFT.get();
        assert!(left > 0, "clone budget exhausted");
        CLONES_LEFT.set(left - 1);
        Self::new(self.0)
    }
}

impl Default for Tracked {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        LIVE.set(LIVE.get() - 1);
    }
}

/// Clones `source` through its erased clone with a budget of `budget` element clones, and
/// returns whether it panicked. A clone that succeeds is dropped again.
fn erased_clone_with_budget<T: Reflect>(
    registry: &TypeRegistry,
    source: &T,
    budget: usize,
) -> bool {
    let description = described::<T>(registry);
    CLONES_LEFT.set(budget);
    let mut copy = MaybeUninit::<T>::uninit();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: valid source; aligned, uninitialized target.
        unsafe {
            description
                .operations()
                .clone_into(std::ptr::from_ref(source).cast(), copy.as_mut_ptr().cast())
                .expect("cloneable");
        }
    }));
    CLONES_LEFT.set(usize::MAX);
    if result.is_ok() {
        // SAFETY: the clone finished, so `copy` is initialized; it's dropped once.
        unsafe {
            description
                .operations()
                .drop_in_place(copy.as_mut_ptr().cast())
        };
    }
    result.is_err()
}

#[test]
fn a_panicking_element_clone_leaks_and_double_drops_nothing() {
    let registry = TypeRegistry::new();
    let before = LIVE.get();
    {
        let list = vec![Tracked::new(1), Tracked::new(2), Tracked::new(3)];
        let array = [Tracked::new(4), Tracked::new(5), Tracked::new(6)];
        let option = Some(Tracked::new(7));
        for budget in 0..=3 {
            assert_eq!(
                erased_clone_with_budget(&registry, &list, budget),
                budget < 3
            );
            assert_eq!(
                erased_clone_with_budget(&registry, &array, budget),
                budget < 3
            );
            assert_eq!(
                LIVE.get() - before,
                7,
                "budget {budget}: partial clones were dropped"
            );
        }
        assert!(erased_clone_with_budget(&registry, &option, 0));
        assert!(!erased_clone_with_budget(&registry, &option, 1));
        assert_eq!(LIVE.get() - before, 7);
    }
    assert_eq!(LIVE.get(), before, "every value was dropped exactly once");
}

#[test]
fn erased_defaults_and_drops_balance() {
    let registry = TypeRegistry::new();
    let before = LIVE.get();
    let array = described::<[Tracked; 4]>(&registry);
    let mut value = MaybeUninit::<[Tracked; 4]>::uninit();
    // SAFETY: aligned, uninitialized target.
    unsafe {
        array
            .operations()
            .default_into(value.as_mut_ptr().cast())
            .expect("defaultable")
    };
    assert_eq!(LIVE.get() - before, 4);
    // SAFETY: initialized above; dropped once and not used again.
    unsafe { array.operations().drop_in_place(value.as_mut_ptr().cast()) };
    assert_eq!(LIVE.get(), before);
}

#[test]
fn math_types_clone_and_default_through_their_impls() {
    let registry = TypeRegistry::new();
    let transform = described::<Transform>(&registry);
    let source = Transform::from_rotation(Quat::from_rotation_y(1.0));
    let mut copy = MaybeUninit::<Transform>::uninit();
    // SAFETY: valid source; aligned, uninitialized target.
    unsafe {
        transform
            .operations()
            .clone_into(std::ptr::from_ref(&source).cast(), copy.as_mut_ptr().cast())
            .expect("cloneable");
    }
    // SAFETY: initialized by `clone_into`.
    assert_eq!(unsafe { copy.assume_init() }, source);
    assert!(!transform.operations().needs_drop());
}

#[test]
fn erased_operations_are_used_for_mode_values() {
    let registry = TypeRegistry::new();
    let mode = described::<Mode>(&registry);
    assert!(
        !mode.operations().is_cloneable(),
        "Mode doesn't declare clone"
    );
    assert_eq!(Mode::Walk as u8, 3);
    let _ = (Mode::Run, Mode::Fly, Weather::Clear, Weather::Rain, Marker);
}
