//! The derive works in crates that `forbid(unsafe_code)`, such as the component crates
//! (ADR-0021 §7): it emits an `unsafe impl` but never a lint attribute, which `forbid` would
//! reject. An integration test is its own crate, so this attribute covers only this file.
#![forbid(unsafe_code)]

use oxreal_reflection::{Reflect, TypeRegistry};

/// A component-like type, as a `forbid(unsafe_code)` crate would declare it.
#[derive(Reflect, Clone, Default)]
#[reflect(id = "f1e5d2c0-7b3a-4f8e-9d6c-0a2b4c6d8e1f", clone, default)]
struct Health {
    current: f32,
    max: f32,
}

#[derive(Reflect)]
#[reflect(transient)]
struct Generic<T> {
    value: T,
}

#[test]
fn derived_types_register_in_a_crate_that_forbids_unsafe_code() {
    let registry = TypeRegistry::new();
    let health = registry.register::<Health>().expect("valid");
    assert_eq!(
        registry.description(health).expect("registered").name(),
        "forbid_unsafe::Health"
    );
    registry.register::<Generic<Health>>().expect("valid");
    let value = Generic {
        value: Health {
            current: 1.0,
            max: 2.0,
        },
    };
    assert!(value.value.current < value.value.max);
}
