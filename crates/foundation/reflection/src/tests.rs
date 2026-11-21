//! Tests of the derive, the registry and the erased operations together.

use std::alloc::Layout;
use std::cell::Cell;
use std::mem::{MaybeUninit, offset_of, size_of};
use std::sync::Arc;

use oxreal_math::{Affine3A, Mat4, Quat, Transform, Vec3, Vec3A};

use crate::id::builtin;
use crate::{
    AttributesDefinition, DefinitionProblem, FieldDefinition, GenericIdInput, Identity, IntRepr,
    KindDefinition, OperationError, Persistence, Primitive, Range, Reflect, RegistrationError,
    StableTypeId, TypeDefinition, TypeDescription, TypeKind, TypeRegistry, Unit,
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
    #[reflect(range(min = 0, max = 20.5, step = 0.5), units = MetersPerSecond)]
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
    assert_eq!(speed.attributes().units(), Some(Unit::MetersPerSecond));
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
    for &id in builtin::ALL {
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
        registry.root_group(),
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

/// A root type that refers to a plugin's type.
#[derive(Reflect)]
#[reflect(id = "c4b8e5f3-0a6d-4c1e-9f2b-3d5e7a9c1b4f")]
struct UsesWeather {
    weather: Weather,
}

#[test]
fn a_group_in_use_outside_it_isnt_removed() {
    let registry = TypeRegistry::new();
    let mut group = registry.group("plugin");
    group.add::<Weather>();
    let id = group.commit().expect("valid");
    registry
        .register::<UsesWeather>()
        .expect("uses the group's type");

    let error = registry.remove_group(id).expect_err("in use");
    assert!(
        matches!(&error, RegistrationError::GroupInUse { ty, used_by, .. }
            if ty.ends_with("Weather") && used_by.ends_with("UsesWeather")),
        "{error}"
    );
    assert!(
        registry.key_of::<Weather>().is_some(),
        "nothing was removed"
    );
    assert_eq!(
        registry.remove_group(registry.root_group()),
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

/// Clones `source` through its description, which must be cloneable.
fn erased_clone<T: Reflect>(description: &TypeDescription, source: &T) -> T {
    let mut copy = MaybeUninit::<T>::uninit();
    // SAFETY: `description` is `T`'s; `source` is a valid `T` that nothing writes during the
    // call; `copy` is aligned, writable and doesn't overlap it.
    unsafe {
        description
            .clone_value(std::ptr::from_ref(source).cast(), copy.as_mut_ptr().cast())
            .expect("cloneable");
    }
    // SAFETY: `clone_value` succeeded, so it initialized `copy`.
    unsafe { copy.assume_init() }
}

/// Builds a default `T` through its description, which must be defaultable.
fn erased_default<T: Reflect>(description: &TypeDescription) -> T {
    let mut value = MaybeUninit::<T>::uninit();
    // SAFETY: `description` is `T`'s; `value` is aligned and writable.
    unsafe {
        description
            .default_value(value.as_mut_ptr().cast())
            .expect("defaultable");
    }
    // SAFETY: `default_value` succeeded, so it initialized `value`.
    unsafe { value.assume_init() }
}

/// Drops `value` through its description.
fn erased_drop<T: Reflect>(description: &TypeDescription, value: T) {
    let mut value = std::mem::ManuallyDrop::new(value);
    // SAFETY: `description` is `T`'s; `value` is a valid, owned `T` that is never used again
    // (`ManuallyDrop` keeps it from being dropped twice).
    unsafe { description.drop_value(std::ptr::from_mut(&mut *value).cast()) };
}

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
    assert_eq!(erased_clone(&movement, &source), source);
    let default: Movement = erased_default(&movement);
    assert_eq!(default, Movement::default());
    erased_drop(&movement, default);
}

#[test]
fn unsupported_operations_are_errors_naming_the_type() {
    let registry = TypeRegistry::new();
    let node = described::<Node>(&registry);
    let mut target = MaybeUninit::<Node>::uninit();
    let source = Node {
        value: 1,
        children: Vec::new(),
    };
    // SAFETY: valid source; aligned, writable target; nothing is written on error.
    let result = unsafe {
        node.clone_value(
            std::ptr::from_ref(&source).cast(),
            target.as_mut_ptr().cast(),
        )
    };
    let error = result.expect_err("Node doesn't declare clone");
    assert!(
        matches!(&error, OperationError::NotCloneable { ty } if ty.ends_with("Node")),
        "{error}"
    );
    // SAFETY: as above.
    let result = unsafe { node.default_value(target.as_mut_ptr().cast()) };
    assert!(matches!(result, Err(OperationError::NoDefault { .. })));
    assert_eq!(source.value, 1);
}

#[test]
fn erased_container_clones_copy_every_element() {
    let registry = TypeRegistry::new();
    let source = vec!["a".to_owned(), "bc".to_owned(), String::new()];
    assert_eq!(
        erased_clone(&described::<Vec<String>>(&registry), &source),
        source
    );

    let option = described::<Option<Vec<String>>>(&registry);
    for source in [None, Some(vec!["x".to_owned()])] {
        assert_eq!(erased_clone(&option, &source), source);
    }

    let source = ["1".to_owned(), "2".to_owned(), "3".to_owned()];
    assert_eq!(
        erased_clone(&described::<[String; 3]>(&registry), &source),
        source
    );

    let defaults: [Vec<u8>; 2] = erased_default(&described::<[Vec<u8>; 2]>(&registry));
    assert_eq!(defaults, [Vec::<u8>::new(), Vec::new()]);

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

#[test]
fn zero_length_arrays_clone_and_default() {
    let registry = TypeRegistry::new();
    let empty = described::<[String; 0]>(&registry);
    let source: [String; 0] = [];
    assert_eq!(erased_clone(&empty, &source), source);
    let _: [String; 0] = erased_default(&empty);
}

#[test]
fn over_aligned_values_in_containers() {
    // Vec3A and the matrices are 16-byte aligned.
    let registry = TypeRegistry::new();
    let list = vec![Vec3A::X, Vec3A::Y, Vec3A::Z];
    assert_eq!(
        erased_clone(&described::<Vec<Vec3A>>(&registry), &list),
        list
    );
    let array = [
        Mat4::IDENTITY,
        Mat4::from_scale(Vec3::splat(2.0)),
        Mat4::ZERO,
    ];
    assert_eq!(
        erased_clone(&described::<[Mat4; 3]>(&registry), &array),
        array
    );
    let option = Some(Affine3A::from_translation(Vec3::ONE));
    assert_eq!(
        erased_clone(&described::<Option<Affine3A>>(&registry), &option),
        option
    );
    let defaults: [Mat4; 2] = erased_default(&described::<[Mat4; 2]>(&registry));
    assert_eq!(defaults, [Mat4::IDENTITY; 2]);
}

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static CLONES_LEFT: Cell<usize> = const { Cell::new(usize::MAX) };
    static DEFAULTS_LEFT: Cell<usize> = const { Cell::new(usize::MAX) };
    static PANIC_ON_DROP: Cell<Option<u32>> = const { Cell::new(None) };
}

/// Counts live values. Panics on clone or default once its budget runs out, and on drop when
/// its value is `PANIC_ON_DROP`.
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
        let left = DEFAULTS_LEFT.get();
        assert!(left > 0, "default budget exhausted");
        DEFAULTS_LEFT.set(left - 1);
        Self::new(0)
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        LIVE.set(LIVE.get() - 1);
        if PANIC_ON_DROP.get() == Some(self.0) {
            PANIC_ON_DROP.set(None);
            panic!("drop of Tracked({}) panics", self.0);
        }
    }
}

/// A zero-sized type that counts live values.
#[derive(Reflect, Debug)]
#[reflect(transient, clone, default)]
struct Zst;

impl Zst {
    fn new() -> Self {
        LIVE.set(LIVE.get() + 1);
        Self
    }
}

impl Clone for Zst {
    fn clone(&self) -> Self {
        Self::new()
    }
}

impl Default for Zst {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Zst {
    fn drop(&mut self) {
        LIVE.set(LIVE.get() - 1);
    }
}

/// Runs `operation` with a budget of `budget` clones or defaults (`cell`), returning whether it
/// panicked.
fn panics_with_budget(
    cell: &'static std::thread::LocalKey<Cell<usize>>,
    budget: usize,
    operation: impl FnOnce(),
) -> bool {
    cell.set(budget);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)).is_err();
    cell.set(usize::MAX);
    panicked
}

#[test]
fn a_panicking_element_clone_leaks_and_double_drops_nothing() {
    let registry = TypeRegistry::new();
    let before = LIVE.get();
    {
        let list = vec![Tracked::new(1), Tracked::new(2), Tracked::new(3)];
        let array = [Tracked::new(4), Tracked::new(5), Tracked::new(6)];
        let option = Some(Tracked::new(7));
        let list_type = described::<Vec<Tracked>>(&registry);
        let array_type = described::<[Tracked; 3]>(&registry);
        let option_type = described::<Option<Tracked>>(&registry);
        for budget in 0..=3 {
            let panicked = panics_with_budget(&CLONES_LEFT, budget, || {
                drop(erased_clone(&list_type, &list));
            });
            assert_eq!(panicked, budget < 3);
            let panicked = panics_with_budget(&CLONES_LEFT, budget, || {
                drop(erased_clone(&array_type, &array));
            });
            assert_eq!(panicked, budget < 3);
            assert_eq!(
                LIVE.get() - before,
                7,
                "budget {budget}: partial clones were dropped"
            );
        }
        assert!(panics_with_budget(&CLONES_LEFT, 0, || {
            drop(erased_clone(&option_type, &option));
        }));
        assert!(!panics_with_budget(&CLONES_LEFT, 1, || {
            drop(erased_clone(&option_type, &option));
        }));
        assert_eq!(LIVE.get() - before, 7);
    }
    assert_eq!(LIVE.get(), before, "every value was dropped exactly once");
}

#[test]
fn a_panicking_element_default_drops_the_finished_prefix() {
    let registry = TypeRegistry::new();
    let before = LIVE.get();
    let array = described::<[Tracked; 4]>(&registry);
    for budget in 0..=4 {
        let panicked = panics_with_budget(&DEFAULTS_LEFT, budget, || {
            let value: [Tracked; 4] = erased_default(&array);
            erased_drop(&array, value);
        });
        assert_eq!(panicked, budget < 4);
        assert_eq!(LIVE.get(), before, "budget {budget}");
    }
}

#[test]
fn erased_drops_of_containers_drop_every_element() {
    let registry = TypeRegistry::new();
    let before = LIVE.get();
    erased_drop(
        &described::<Vec<Tracked>>(&registry),
        vec![Tracked::new(1), Tracked::new(2)],
    );
    erased_drop(
        &described::<Option<Tracked>>(&registry),
        Some(Tracked::new(3)),
    );
    erased_drop(
        &described::<[Tracked; 2]>(&registry),
        [Tracked::new(4), Tracked::new(5)],
    );
    assert_eq!(LIVE.get(), before);
}

#[test]
fn a_panicking_drop_inside_a_list_drops_the_other_elements() {
    let registry = TypeRegistry::new();
    let before = LIVE.get();
    let list = described::<Vec<Tracked>>(&registry);
    PANIC_ON_DROP.set(Some(2));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        erased_drop(
            &list,
            vec![Tracked::new(1), Tracked::new(2), Tracked::new(3)],
        );
    }))
    .is_err();
    assert!(panicked);
    assert_eq!(
        LIVE.get(),
        before,
        "the elements after the panicking one were dropped too"
    );
}

#[test]
fn zero_sized_elements_balance() {
    let registry = TypeRegistry::new();
    let before = LIVE.get();
    {
        let list = vec![Zst::new(), Zst::new(), Zst::new()];
        let copy = erased_clone(&described::<Vec<Zst>>(&registry), &list);
        assert_eq!(copy.len(), 3);
        let array = [Zst::new(), Zst::new()];
        let copy_array = erased_clone(&described::<[Zst; 2]>(&registry), &array);
        let option = erased_clone(&described::<Option<Zst>>(&registry), &Some(Zst::new()));
        let defaults: [Zst; 3] = erased_default(&described::<[Zst; 3]>(&registry));
        erased_drop(&described::<Vec<Zst>>(&registry), copy);
        drop((list, array, copy_array, option, defaults));
    }
    assert_eq!(LIVE.get(), before);
}

#[test]
fn math_types_clone_and_default_through_their_impls() {
    let registry = TypeRegistry::new();
    let transform = described::<Transform>(&registry);
    let source = Transform::from_rotation(Quat::from_rotation_y(1.0));
    assert_eq!(erased_clone(&transform, &source), source);
    assert_eq!(erased_default::<Transform>(&transform), Transform::IDENTITY);
    assert!(!transform.operations().needs_drop());
}

#[test]
fn mode_doesnt_declare_clone() {
    let registry = TypeRegistry::new();
    let mode = described::<Mode>(&registry);
    assert!(!mode.operations().is_cloneable());
    assert_eq!(Mode::Walk as u8, 3);
    let _ = (Mode::Run, Mode::Fly, Weather::Clear, Weather::Rain, Marker);
}

// --- Registry rules added after review ---

#[test]
fn built_in_containers_belong_to_their_element_types_group() {
    let registry = TypeRegistry::new();
    let mut group = registry.group("plugin");
    group.add::<Vec<u16>>().add::<Option<Weather>>();
    let id = group.commit().expect("valid");
    let shared = registry.description_of::<Vec<u16>>().expect("registered");
    assert_eq!(
        shared.group(),
        registry.root_group(),
        "Vec<u16> is shared, so it's root's"
    );
    let own = registry
        .description_of::<Option<Weather>>()
        .expect("registered");
    assert_eq!(own.group(), id, "Option<Weather> leaves with Weather");

    // A root type using Vec<u16> doesn't pin the plugin.
    registry.register::<Option<Vec<u16>>>().expect("valid");
    registry
        .remove_group(id)
        .expect("nothing outside uses the plugin's types");
    assert!(registry.key_of::<Option<Weather>>().is_none());
    assert!(registry.key_of::<Vec<u16>>().is_some());
}

#[test]
fn a_type_already_in_another_group_cant_join_a_group() {
    let registry = TypeRegistry::new();
    registry.register::<Weather>().expect("valid");
    let mut group = registry.group("plugin");
    group.add::<Weather>();
    let error = group.commit().expect_err("Weather is root's");
    assert_eq!(
        error.to_string(),
        format!(
            "'{}::Weather' is already registered in group 'root'; add it to its group before \
             anything else registers it",
            module_path!()
        )
    );
    assert!(
        matches!(&error, RegistrationError::AlreadyInGroup { ty, group }
            if ty.ends_with("Weather") && group == "root"),
        "{error}"
    );

    // Adding a type twice, or one the group itself pulled in, is fine.
    let mut group = registry.group("plugin");
    group.add::<Node>().add::<Vec<Node>>().add::<Node>();
    group.commit().expect("all in this group");
}

#[test]
fn group_ids_belong_to_their_registry() {
    let first = TypeRegistry::new();
    let second = TypeRegistry::new();
    let mut group = first.group("plugin");
    group.add::<Weather>();
    let id = group.commit().expect("valid");
    assert_eq!(
        second.remove_group(id),
        Err(RegistrationError::UnknownGroup { group: id })
    );
    assert_eq!(
        second.remove_group(first.root_group()),
        Err(RegistrationError::UnknownGroup {
            group: first.root_group()
        })
    );
}

#[test]
fn unregistered_ids_lists_whats_missing_from_a_listing() {
    let registry = TypeRegistry::new();
    registry.register::<Weather>().expect("valid");
    let listing = "\u{feff}# generated\r\n\
                   f4d8b5c3-0a6e-4b1d-8f9c-3e5a7b9c1d4f Weather\r\n\
                   \r\n\
                   e3c7a4b2-9f5d-4a0c-9e8b-2d4f6a8b0c3e Mode\r\n";
    assert_eq!(
        registry.unregistered_ids(listing),
        Ok(vec![StableTypeId::from_uuid(
            "e3c7a4b2-9f5d-4a0c-9e8b-2d4f6a8b0c3e"
        )])
    );
    assert!(registry.unregistered_ids("not-a-uuid Thing").is_err());
}

/// An honest hand-written impl of a packed type: real field types, real offsets. Registration
/// must still refuse it, because its fields aren't aligned.
#[repr(C, packed(1))]
struct Packed {
    wide: u32,
    narrow: u8,
}

// SAFETY: deliberately breaks the "not packed" clause; registration must refuse it.
unsafe impl Reflect for Packed {
    const TRANSIENT: bool = true;
    const ABI: bool = false;
    const DEFINITION: TypeDefinition = TypeDefinition::builder::<Self>(
        "Packed",
        Persistence::Transient,
        KindDefinition::named_struct(&[
            FieldDefinition::new::<u32>("wide", offset_of!(Packed, wide)),
            FieldDefinition::new::<u8>("narrow", offset_of!(Packed, narrow)),
        ]),
    )
    .build();
}

/// A hand-written handle that needs dropping.
struct DroppingHandle(String);

// SAFETY: deliberately breaks the handle clause; registration must refuse it.
unsafe impl Reflect for DroppingHandle {
    const TRANSIENT: bool = true;
    const ABI: bool = false;
    const DEFINITION: TypeDefinition = TypeDefinition::builder::<Self>(
        "DroppingHandle",
        Persistence::Transient,
        KindDefinition::handle(StableTypeId::from_uuid(
            "a7c1e8f6-3d9b-4e4a-9c2f-6b8d0e2f4a7d",
        )),
    )
    .build();
}

/// A hand-written `abi` struct that needs dropping.
#[repr(C)]
struct DroppingAbi(u32);

impl Drop for DroppingAbi {
    fn drop(&mut self) {}
}

// SAFETY: deliberately breaks the ABI clause; registration must refuse it.
unsafe impl Reflect for DroppingAbi {
    const TRANSIENT: bool = true;
    const ABI: bool = true;
    const DEFINITION: TypeDefinition = TypeDefinition::builder::<Self>(
        "DroppingAbi",
        Persistence::Transient,
        KindDefinition::tuple_struct(&[FieldDefinition::new::<u32>(
            "0",
            offset_of!(DroppingAbi, 0),
        )]),
    )
    .abi()
    .build();
}

/// A hand-written enum without variants.
enum Never {}

// SAFETY: deliberately describes an empty enum; registration must refuse it.
unsafe impl Reflect for Never {
    const TRANSIENT: bool = true;
    const ABI: bool = false;
    const DEFINITION: TypeDefinition = TypeDefinition::builder::<Self>(
        "Never",
        Persistence::Transient,
        KindDefinition::unit_enum(None, &[]),
    )
    .build();
}

#[test]
fn unsound_hand_written_layouts_are_refused() {
    let registry = TypeRegistry::new();
    let problem = |error| match error {
        RegistrationError::InvalidDefinition { problem, .. } => problem,
        other => panic!("{other}"),
    };
    assert_eq!(
        problem(registry.register::<Packed>().expect_err("packed")),
        DefinitionProblem::MisalignedField("wide".to_owned())
    );
    assert_eq!(
        problem(
            registry
                .register::<DroppingHandle>()
                .expect_err("handle with Drop")
        ),
        DefinitionProblem::HandleWithDrop
    );
    assert_eq!(
        problem(
            registry
                .register::<DroppingAbi>()
                .expect_err("abi with Drop")
        ),
        DefinitionProblem::AbiWithDrop
    );
    assert_eq!(
        problem(registry.register::<Never>().expect_err("no variants")),
        DefinitionProblem::NoVariants
    );
    let packed = Packed { wide: 1, narrow: 2 };
    let _ = (
        DroppingHandle(String::new()).0,
        DroppingAbi(0).0,
        packed.narrow,
    );
}

#[derive(Reflect)]
#[reflect(transient)]
struct TransientCache {
    #[reflect(transient)]
    cached: NoDefault,
}

#[test]
fn a_transient_type_may_have_a_transient_field_without_default() {
    // It's never loaded, so nothing ever needs the field's default.
    let registry = TypeRegistry::new();
    registry.register::<TransientCache>().expect("valid");
    let cache = TransientCache {
        cached: NoDefault { value: 0 },
    };
    assert_eq!(cache.cached.value, 0);
}
