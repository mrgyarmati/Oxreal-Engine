//! Type-erased operations: drop, clone and default construction through a description
//! (ADR-0021 §3).
//!
//! This is the `unsafe` core of the crate's first slice (ADR-0021 §7). Each operation is a plain
//! function pointer, monomorphized for one type, that works on untyped pointers. Safe code can't
//! call them: [`Operations`]' methods are `unsafe fn`s whose callers promise the pointer
//! contract, and the borrowed and owned views built on them later keep that promise for safe
//! callers.

use std::mem::{ManuallyDrop, MaybeUninit};
use std::ptr;

use crate::Reflect;

/// Drops the value at the pointer.
type DropFn = unsafe fn(*mut u8);
/// Writes a clone of the value at the first pointer into the uninitialized second one.
type CloneFn = unsafe fn(*const u8, *mut u8);
/// Writes the default value into the uninitialized pointer.
type DefaultFn = unsafe fn(*mut u8);

/// The operations a type supports through erased pointers (ADR-0021 §3).
///
/// - **Drop** is always supported. [`Operations::needs_drop`] says whether it does anything.
/// - **Clone** only when the type declares it: `#[reflect(clone)]` with a real [`Clone`] impl,
///   or a container whose elements are cloneable. Nothing is cloned field by field.
/// - **Default** only when the type declares it: `#[reflect(default)]` with a real [`Default`]
///   impl, or a container that has an empty value.
///
/// Every pointer passed to these methods must be aligned for the described type and must not
/// overlap another argument.
#[derive(Clone, Copy)]
pub struct Operations {
    drop: Option<DropFn>,
    clone: Option<CloneFn>,
    default: Option<DefaultFn>,
}

impl Operations {
    /// Drop only, for `T`.
    pub(crate) const fn of<T>() -> Self {
        Self {
            drop: if std::mem::needs_drop::<T>() {
                Some(drop_value::<T>)
            } else {
                None
            },
            clone: None,
            default: None,
        }
    }

    /// Adds clone through `T`'s [`Clone`] impl.
    pub(crate) const fn with_clone<T: Clone>(mut self) -> Self {
        self.clone = Some(clone_value::<T>);
        self
    }

    /// Adds default construction through `T`'s [`Default`] impl.
    pub(crate) const fn with_default<T: Default>(mut self) -> Self {
        self.default = Some(default_value::<T>);
        self
    }

    /// `Vec<T>`: cloneable when `T` is, and always defaultable (empty).
    pub(crate) const fn of_vec<T: Reflect>() -> Self {
        let mut ops = Self::of::<Vec<T>>();
        if T::DEFINITION.operations().clone.is_some() {
            ops.clone = Some(clone_vec::<T>);
        }
        ops.default = Some(default_value::<Vec<T>>);
        ops
    }

    /// `Option<T>`: cloneable when `T` is, and always defaultable (`None`).
    pub(crate) const fn of_option<T: Reflect>() -> Self {
        let mut ops = Self::of::<Option<T>>();
        if T::DEFINITION.operations().clone.is_some() {
            ops.clone = Some(clone_option::<T>);
        }
        ops.default = Some(default_value::<Option<T>>);
        ops
    }

    /// `[T; N]`: cloneable and defaultable when `T` is.
    pub(crate) const fn of_array<T: Reflect, const N: usize>() -> Self {
        let element = T::DEFINITION.operations();
        let mut ops = Self::of::<[T; N]>();
        if element.clone.is_some() {
            ops.clone = Some(clone_array::<T, N>);
        }
        if element.default.is_some() {
            ops.default = Some(default_array::<T, N>);
        }
        ops
    }

    /// Whether dropping a value does anything. When it doesn't, storage may skip
    /// [`Operations::drop_in_place`] entirely.
    #[must_use]
    pub const fn needs_drop(&self) -> bool {
        self.drop.is_some()
    }

    /// Whether values can be cloned.
    #[must_use]
    pub const fn is_cloneable(&self) -> bool {
        self.clone.is_some()
    }

    /// Whether a default value can be constructed.
    #[must_use]
    pub const fn is_defaultable(&self) -> bool {
        self.default.is_some()
    }

    /// Drops the value at `value`, which is uninitialized afterwards.
    ///
    /// # Safety
    ///
    /// `value` must point to a valid, aligned value of the type these operations describe,
    /// which the caller owns and doesn't use again (other than to overwrite it).
    pub unsafe fn drop_in_place(&self, value: *mut u8) {
        if let Some(drop) = self.drop {
            // SAFETY: the caller's contract is `drop`'s contract.
            unsafe { drop(value) }
        }
    }

    /// Writes a clone of the value at `source` into `target`.
    ///
    /// If the clone panics, `target` stays uninitialized and nothing leaks.
    ///
    /// # Errors
    ///
    /// [`UnsupportedOperation::Clone`] if the type can't be cloned. `target` is untouched.
    ///
    /// # Safety
    ///
    /// - `source` must point to a valid, aligned value of the described type, readable for the
    ///   duration of the call.
    /// - `target` must be aligned, writable for the type's size, not overlap `source`, and hold
    ///   no value that needs dropping (it is overwritten without being dropped).
    pub unsafe fn clone_into(
        &self,
        source: *const u8,
        target: *mut u8,
    ) -> Result<(), UnsupportedOperation> {
        let clone = self.clone.ok_or(UnsupportedOperation::Clone)?;
        // SAFETY: the caller's contract is `clone`'s contract.
        unsafe { clone(source, target) };
        Ok(())
    }

    /// Writes the default value into `target`.
    ///
    /// # Errors
    ///
    /// [`UnsupportedOperation::Default`] if the type has no default. `target` is untouched.
    ///
    /// # Safety
    ///
    /// `target` must be aligned, writable for the type's size, and hold no value that needs
    /// dropping (it is overwritten without being dropped).
    pub unsafe fn default_into(&self, target: *mut u8) -> Result<(), UnsupportedOperation> {
        let default = self.default.ok_or(UnsupportedOperation::Default)?;
        // SAFETY: the caller's contract is `default`'s contract.
        unsafe { default(target) };
        Ok(())
    }
}

impl std::fmt::Debug for Operations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Operations")
            .field("needs_drop", &self.needs_drop())
            .field("cloneable", &self.is_cloneable())
            .field("defaultable", &self.is_defaultable())
            .finish()
    }
}

/// An operation the type doesn't support was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UnsupportedOperation {
    /// The type isn't cloneable: it doesn't declare `#[reflect(clone)]`, or it's a container of
    /// a type that doesn't.
    #[error("the type can't be cloned: it doesn't declare #[reflect(clone)]")]
    Clone,
    /// The type has no default value: it doesn't declare `#[reflect(default)]`, or it's an
    /// array of a type that doesn't.
    #[error("the type has no default value: it doesn't declare #[reflect(default)]")]
    Default,
}

/// # Safety
///
/// `value` points to a valid, aligned, owned `T` that isn't used again.
unsafe fn drop_value<T>(value: *mut u8) {
    // SAFETY: the caller's contract.
    unsafe { ptr::drop_in_place(value.cast::<T>()) }
}

/// # Safety
///
/// `source` points to a valid, aligned `T`. `target` is aligned and writable for a `T`, doesn't
/// overlap `source`, and holds nothing that needs dropping.
unsafe fn clone_value<T: Clone>(source: *const u8, target: *mut u8) {
    // SAFETY: `source` is a valid `T` for the call (caller's contract). If `clone` panics,
    // nothing has been written.
    let clone = unsafe { &*source.cast::<T>() }.clone();
    // SAFETY: `target` is aligned and writable for a `T` (caller's contract).
    unsafe { target.cast::<T>().write(clone) }
}

/// # Safety
///
/// `target` is aligned and writable for a `T` and holds nothing that needs dropping.
unsafe fn default_value<T: Default>(target: *mut u8) {
    // SAFETY: the caller's contract.
    unsafe { target.cast::<T>().write(T::default()) }
}

/// The erased clone of `T`, which the container operations only install when it exists.
fn element_clone<T: Reflect>() -> CloneFn {
    T::DEFINITION
        .operations()
        .clone
        .expect("container clone is only installed when the element type is cloneable")
}

/// # Safety
///
/// As [`clone_value`], for `Vec<T>`. Only installed when `T` is cloneable.
unsafe fn clone_vec<T: Reflect>(source: *const u8, target: *mut u8) {
    let clone = element_clone::<T>();
    // SAFETY: `source` is a valid `Vec<T>` for the call (caller's contract).
    let source = unsafe { &*source.cast::<Vec<T>>() };
    let mut copy = Vec::<T>::with_capacity(source.len());
    for (i, element) in source.iter().enumerate() {
        // SAFETY: the capacity is `source.len()`, so slot `i` is in bounds, aligned and
        // uninitialized. `element` is a valid `T`. `clone` writes a valid `T` into the slot, and
        // only then does the length grow to cover it, so a panic in `clone` drops exactly the
        // elements cloned so far.
        unsafe {
            clone(
                ptr::from_ref(element).cast(),
                copy.as_mut_ptr().add(i).cast(),
            );
            copy.set_len(i + 1);
        }
    }
    // SAFETY: `target` is aligned and writable for a `Vec<T>` (caller's contract).
    unsafe { target.cast::<Vec<T>>().write(copy) }
}

/// # Safety
///
/// As [`clone_value`], for `Option<T>`. Only installed when `T` is cloneable.
unsafe fn clone_option<T: Reflect>(source: *const u8, target: *mut u8) {
    // SAFETY: `source` is a valid `Option<T>` for the call (caller's contract).
    let copy = match unsafe { &*source.cast::<Option<T>>() } {
        None => None,
        Some(value) => {
            let mut slot = MaybeUninit::<T>::uninit();
            // SAFETY: `value` is a valid `T`; `slot` is aligned, writable and uninitialized.
            unsafe { element_clone::<T>()(ptr::from_ref(value).cast(), slot.as_mut_ptr().cast()) };
            // SAFETY: the element clone initialized `slot`.
            Some(unsafe { slot.assume_init() })
        }
    };
    // SAFETY: `target` is aligned and writable for an `Option<T>` (caller's contract).
    unsafe { target.cast::<Option<T>>().write(copy) }
}

/// Drops the initialized prefix of an array being built if building it panics.
struct PartialArray<T> {
    start: *mut T,
    initialized: usize,
}

impl<T> Drop for PartialArray<T> {
    fn drop(&mut self) {
        // SAFETY: exactly the first `initialized` elements were written, and nothing else owns
        // them while the array is unfinished.
        unsafe {
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.start, self.initialized));
        }
    }
}

/// Fills `[T; N]` at `target` element by element with `write`, dropping the finished prefix if
/// `write` panics.
///
/// # Safety
///
/// `target` is aligned and writable for a `[T; N]` and holds nothing that needs dropping.
/// `write(i, slot)` must initialize `slot` with a valid `T` (or panic, leaving it uninitialized).
unsafe fn fill_array<T, const N: usize>(target: *mut u8, mut write: impl FnMut(usize, *mut u8)) {
    let start = target.cast::<T>();
    let mut partial = PartialArray::<T> {
        start,
        initialized: 0,
    };
    for i in 0..N {
        // SAFETY: `i < N`, so the slot is inside the array at `target` (caller's contract).
        write(i, unsafe { start.add(i) }.cast());
        partial.initialized = i + 1;
    }
    // All `N` elements are initialized: the array is complete and owned by `target` now, so the
    // guard must not drop them.
    let _complete = ManuallyDrop::new(partial);
}

/// # Safety
///
/// As [`clone_value`], for `[T; N]`. Only installed when `T` is cloneable.
unsafe fn clone_array<T: Reflect, const N: usize>(source: *const u8, target: *mut u8) {
    let clone = element_clone::<T>();
    let source = source.cast::<T>();
    // SAFETY: `target` is as `fill_array` requires (caller's contract). Each element `i < N` of
    // `source` is a valid `T`, and `clone` initializes the slot it is given.
    unsafe {
        fill_array::<T, N>(target, |i, slot| clone(source.add(i).cast(), slot));
    }
}

/// # Safety
///
/// As [`default_value`], for `[T; N]`. Only installed when `T` is defaultable.
unsafe fn default_array<T: Reflect, const N: usize>(target: *mut u8) {
    let default = T::DEFINITION
        .operations()
        .default
        .expect("array default is only installed when the element type is defaultable");
    // SAFETY: `target` is as `fill_array` requires (caller's contract), and `default`
    // initializes the slot it is given.
    unsafe { fill_array::<T, N>(target, |_, slot| default(slot)) };
}
