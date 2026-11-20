//! Resources: typed singletons stored in a world (ADR-0014 §8).

use crate::type_map::TypeIdMap;
use std::any::{Any, TypeId};

/// A value a world holds exactly one of, such as the frame's input or time.
///
/// Implement it explicitly: `impl Resource for GameSettings {}`.
pub trait Resource: Send + Sync + 'static {}

#[derive(Debug, Default)]
pub(crate) struct Resources {
    values: TypeIdMap<Box<dyn Any + Send + Sync>>,
}

impl Resources {
    pub(crate) fn insert<R: Resource>(&mut self, value: R) -> Option<R> {
        self.values
            .insert(TypeId::of::<R>(), Box::new(value))
            .map(|old| *downcast::<R>(old))
    }

    pub(crate) fn remove<R: Resource>(&mut self) -> Option<R> {
        self.values
            .remove(&TypeId::of::<R>())
            .map(|old| *downcast::<R>(old))
    }

    pub(crate) fn get<R: Resource>(&self) -> Option<&R> {
        self.values
            .get(&TypeId::of::<R>())
            .and_then(|value| value.downcast_ref())
    }

    pub(crate) fn get_mut<R: Resource>(&mut self) -> Option<&mut R> {
        self.values
            .get_mut(&TypeId::of::<R>())
            .and_then(|value| value.downcast_mut())
    }
}

fn downcast<R: Resource>(value: Box<dyn Any + Send + Sync>) -> Box<R> {
    match value.downcast() {
        Ok(value) => value,
        Err(_) => unreachable!(
            "a resource stored under {}'s type ID has another type: a bug in oxreal-ecs",
            std::any::type_name::<R>()
        ),
    }
}
