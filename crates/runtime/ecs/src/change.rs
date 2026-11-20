//! Mutable access to a component through a smart pointer (ADR-0014 §6).

use std::fmt;
use std::ops::{Deref, DerefMut};

/// Mutable access to one component value, from [`crate::World::get_mut`] or a `&mut T` query.
///
/// Use it like `&mut T`: fields and methods are reached through it, and `*value = new` replaces
/// the value. Bind it with `mut` to write through it:
///
/// ```
/// # use oxreal_ecs::{Component, World};
/// struct Health(u32);
/// impl Component for Health {}
///
/// let mut world = World::new();
/// let e = world.spawn(Health(10));
/// if let Some(mut health) = world.get_mut::<Health>(e) {
///     health.0 -= 3;
/// }
/// assert_eq!(world.get::<Health>(e).map(|h| h.0), Some(7));
/// ```
///
/// It exists so change detection can come without changing this API: a value will count as
/// changed only when it's actually written through ([`DerefMut`]), not merely fetched.
pub struct Mut<'w, T> {
    value: &'w mut T,
}

impl<'w, T> Mut<'w, T> {
    pub(crate) fn new(value: &'w mut T) -> Self {
        Self { value }
    }

    /// The plain `&mut T`, for passing to code that expects one. Taking it counts as a change.
    #[must_use]
    pub fn into_inner(self) -> &'w mut T {
        self.value
    }
}

impl<T> Deref for Mut<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value
    }
}

impl<T> DerefMut for Mut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.value
    }
}

impl<T> AsRef<T> for Mut<'_, T> {
    fn as_ref(&self) -> &T {
        self.value
    }
}

impl<T: fmt::Debug> fmt::Debug for Mut<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}
