//! Erased access to values: borrowed views ([`ValueRef`], [`ValueMut`]) and owned values
//! ([`OwnedValue`]) (ADR-0021 §4).
//!
//! A view is a pointer, the description of its type and the registry that holds it, tied to a
//! borrow. Safe code makes views only from `&T` and `&mut T` ([`TypeRegistry::view`],
//! [`TypeRegistry::view_mut`]); engine code that holds raw storage uses the `unsafe`
//! constructors. Every navigation step checks the description before it touches memory, and
//! every caller mistake (an unknown field, an index out of bounds, a value of the wrong type) is
//! an error, not a panic: paths and values come from files, the editor and other languages.
//!
//! # Invariants
//!
//! - A view's pointer is non-null, aligned, and points to a valid value of its description's
//!   type that lives for the view's lifetime.
//! - Through a [`ValueRef`], the value is only read (or changed through `UnsafeCell`, as `&T`
//!   allows). Through a [`ValueMut`], nothing else accesses the value while the view lives.
//! - The description belongs to the view's registry.
//! - An [`OwnedValue`] owns its allocation and a valid value in it, and drops both.

use std::alloc::{self, Layout};
use std::any::TypeId;
use std::fmt;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ptr::{self, NonNull};
use std::sync::Arc;

use crate::Reflect;
use crate::access::Access;
use crate::description::{TypeDescription, TypeKey, TypeKind};
use crate::ops::OperationError;
use crate::path::{FieldPath, ResolvedPath, Step};
use crate::registry::{RegistrationError, RegistryId, TypeRegistry};

/// A shared view of a value through its description.
///
/// ```
/// use oxreal_reflection::{Reflect, TypeRegistry};
///
/// #[derive(Reflect)]
/// #[reflect(id = "4c2e8a1f-6b3d-4f7e-9a5c-1d3e5f7a9b2c")]
/// struct Door {
///     locked: bool,
///     hinges: Vec<f32>,
/// }
///
/// let registry = TypeRegistry::new();
/// let door = Door { locked: true, hinges: vec![0.5, 1.5] };
/// let view = registry.view(&door)?;
/// assert_eq!(view.field("locked")?.downcast::<bool>()?, &true);
/// assert_eq!(view.field("hinges")?.element(1)?.downcast::<f32>()?, &1.5);
/// assert_eq!(view.path(&"hinges[0]".parse()?)?.downcast::<f32>()?, &0.5);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct ValueRef<'a> {
    registry: &'a TypeRegistry,
    description: Arc<TypeDescription>,
    ptr: NonNull<u8>,
    borrow: PhantomData<&'a ()>,
}

/// An exclusive view of a value through its description.
///
/// Writes go through the value's own operations: replacing a value swaps it with an
/// [`OwnedValue`] of the same type and hands back the old one, so nothing is dropped twice and
/// undo has what it needs.
pub struct ValueMut<'a> {
    registry: &'a TypeRegistry,
    description: Arc<TypeDescription>,
    ptr: NonNull<u8>,
    borrow: PhantomData<&'a mut ()>,
}

/// A value of a reflected type, owned through its description: what undo, the clipboard and
/// patches hold (ADR-0021 §4).
pub struct OwnedValue {
    registry: RegistryId,
    description: Arc<TypeDescription>,
    ptr: NonNull<u8>,
}

// SAFETY: a `ValueRef` is a shared borrow of a value of a `Reflect` type, which is `Sync`, or of
// plain data defined outside Rust, which has no thread affinity. Like `&T` for `T: Sync`, it can
// be sent and shared between threads.
unsafe impl Send for ValueRef<'_> {}
// SAFETY: as for `Send`.
unsafe impl Sync for ValueRef<'_> {}
// SAFETY: a `ValueMut` is an exclusive borrow of a value of a `Send + Sync` type (or plain
// data), like `&mut T`, which is `Send` and `Sync` for such `T`.
unsafe impl Send for ValueMut<'_> {}
// SAFETY: as for `Send`; through `&ValueMut` only shared access is possible.
unsafe impl Sync for ValueMut<'_> {}
// SAFETY: an `OwnedValue` owns a value of a `Send + Sync` type (or plain data) and its
// allocation, like `Box<T>`.
unsafe impl Send for OwnedValue {}
// SAFETY: as for `Send`; through `&OwnedValue` only shared access is possible.
unsafe impl Sync for OwnedValue {}

impl TypeRegistry {
    /// A shared view of `value`, registering `T` if it isn't registered yet.
    ///
    /// # Errors
    ///
    /// [`ValueError::Registration`] if `T` can't be registered.
    pub fn view<'a, T: Reflect>(&'a self, value: &'a T) -> Result<ValueRef<'a>, ValueError> {
        let key = self.register::<T>()?;
        let description = self.live(key)?;
        Ok(ValueRef {
            registry: self,
            description,
            ptr: NonNull::from(value).cast(),
            borrow: PhantomData,
        })
    }

    /// An exclusive view of `value`, registering `T` if it isn't registered yet.
    ///
    /// # Errors
    ///
    /// [`ValueError::Registration`] if `T` can't be registered.
    pub fn view_mut<'a, T: Reflect>(
        &'a self,
        value: &'a mut T,
    ) -> Result<ValueMut<'a>, ValueError> {
        let key = self.register::<T>()?;
        let description = self.live(key)?;
        Ok(ValueMut {
            registry: self,
            description,
            ptr: NonNull::from(value).cast(),
            borrow: PhantomData,
        })
    }

    fn live(&self, key: TypeKey) -> Result<Arc<TypeDescription>, ValueError> {
        self.description(key).ok_or(ValueError::UnknownType { key })
    }
}

impl<'a> ValueRef<'a> {
    /// A view of the value of type `key` at `ptr`.
    ///
    /// # Errors
    ///
    /// [`ValueError::UnknownType`] if `key` isn't a live type of `registry`.
    ///
    /// # Safety
    ///
    /// `ptr` points to a valid, aligned value of the type `key` names in `registry`. For `'a`
    /// it stays valid and is only read (or changed through `UnsafeCell`).
    pub unsafe fn from_raw(
        registry: &'a TypeRegistry,
        key: TypeKey,
        ptr: NonNull<u8>,
    ) -> Result<Self, ValueError> {
        Ok(Self {
            registry,
            description: registry.live(key)?,
            ptr,
            borrow: PhantomData,
        })
    }

    /// The value's type.
    #[must_use]
    pub fn description(&self) -> &TypeDescription {
        &self.description
    }

    /// The registry the view resolves types in.
    #[must_use]
    pub fn registry(&self) -> &'a TypeRegistry {
        self.registry
    }

    /// The value as a `T`.
    ///
    /// # Errors
    ///
    /// [`ValueError::TypeMismatch`] if the value isn't a `T`.
    pub fn downcast<T: Reflect>(&self) -> Result<&'a T, ValueError> {
        check_type::<T>(&self.description)?;
        // SAFETY: the value is a `T` (checked above) that lives and is only read for `'a` (the
        // view's invariant).
        Ok(unsafe { self.ptr.cast::<T>().as_ref() })
    }

    /// The struct field `name` (or a field with `name` as a former name).
    ///
    /// # Errors
    ///
    /// If the value isn't a struct, has no such field, or the field is opaque.
    pub fn field(&self, name: &str) -> Result<ValueRef<'a>, ValueError> {
        let (offset, key) = field_of(&self.description, name)?;
        // SAFETY: the field lies inside the value at `offset` (registration checked its bounds
        // and alignment), and it's of type `key`, valid for as long as its container.
        unsafe { self.child(self.ptr.add(offset), key) }
    }

    /// The number of elements of an array or list.
    ///
    /// # Errors
    ///
    /// [`ValueError::WrongKind`] if the value isn't an array or list.
    pub fn len(&self) -> Result<usize, ValueError> {
        // SAFETY: the view's invariant.
        unsafe { len(&self.description, self.ptr) }
    }

    /// Whether an array or list has no elements.
    ///
    /// # Errors
    ///
    /// [`ValueError::WrongKind`] if the value isn't an array or list.
    pub fn is_empty(&self) -> Result<bool, ValueError> {
        self.len().map(|len| len == 0)
    }

    /// The element at `index` of an array or list.
    ///
    /// # Errors
    ///
    /// If the value isn't an array or list, or `index` is out of bounds.
    pub fn element(&self, index: usize) -> Result<ValueRef<'a>, ValueError> {
        // SAFETY: the view's invariant; `element` checks the kind and bounds.
        let (ptr, key) = unsafe { element(&self.description, self.ptr, index, false)? };
        // SAFETY: an element of the value, of type `key`, valid as long as the container.
        unsafe { self.child(ptr, key) }
    }

    /// The value inside an `Option`, or `None`.
    ///
    /// # Errors
    ///
    /// [`ValueError::WrongKind`] if the value isn't an option.
    pub fn some(&self) -> Result<Option<ValueRef<'a>>, ValueError> {
        // SAFETY: the view's invariant; `payload` checks the kind.
        match unsafe { payload(&self.description, self.ptr, false)? } {
            // SAFETY: the payload of the value, of type `key`, valid as long as the container.
            Some((ptr, key)) => unsafe { self.child(ptr, key) }.map(Some),
            None => Ok(None),
        }
    }

    /// The index of a unit enum's variant, in [`UnitEnumInfo::variants`] order.
    ///
    /// [`UnitEnumInfo::variants`]: crate::UnitEnumInfo::variants
    ///
    /// # Errors
    ///
    /// [`ValueError::WrongKind`] if the value isn't a unit enum.
    pub fn variant(&self) -> Result<usize, ValueError> {
        let Access::Enum(access) = &self.description.access else {
            return Err(wrong_kind(&self.description, "a unit enum"));
        };
        // SAFETY: the value is a valid enum of this description's type (the view's invariant),
        // and the accessor was made for that type.
        Ok(unsafe { (access.index)(self.ptr.as_ptr()) })
    }

    /// A copy of the math component `name`, such as `x` of a `Vec3`.
    ///
    /// # Errors
    ///
    /// If the value isn't a math type or has no such component.
    pub fn component(&self, name: &str) -> Result<OwnedValue, ValueError> {
        let index = component_index(&self.description, name)?;
        // SAFETY: the view's invariant; `index` is one of the type's components.
        unsafe { get_component(self.registry, &self.description, self.ptr, index) }
    }

    /// The value at `path`, which must not end inside a math value (use
    /// [`ValueRef::read_at`] for those).
    ///
    /// # Errors
    ///
    /// If the path belongs to another type or registry, a list index is out of bounds, an
    /// option on the way is `None`, or the path ends inside a math value.
    pub fn at(&self, path: &ResolvedPath) -> Result<ValueRef<'a>, ValueError> {
        if path.ends_in_math_component() {
            return Err(ValueError::MathComponentBorrow {
                ty: self.description.name().to_owned(),
            });
        }
        self.check_path(path)?;
        // SAFETY: the view's invariant; the path was resolved for this type and registry.
        let ptr = unsafe { navigate(self.registry, &self.description, self.ptr, path, false)? };
        // SAFETY: the path's target, of the path's leaf type, inside the value.
        unsafe { self.child(ptr, path.leaf) }
    }

    /// A copy of the value at `path`, which may end inside a math value.
    ///
    /// # Errors
    ///
    /// As [`ValueRef::at`] (except for math values), or [`ValueError::Operation`] if the value
    /// isn't cloneable.
    pub fn read_at(&self, path: &ResolvedPath) -> Result<OwnedValue, ValueError> {
        self.check_path(path)?;
        // SAFETY: the view's invariant; the path was resolved for this type and registry.
        let ptr = unsafe { navigate(self.registry, &self.description, self.ptr, path, false)? };
        let Some((&(last, _), outer)) = path.components.split_last() else {
            // SAFETY: the path's target, of the leaf type.
            return unsafe { self.child(ptr, path.leaf) }?.to_owned_value();
        };
        let owner = path.steps.last().map_or(self.description.key(), |s| s.ty);
        let mut math = self.registry.live(owner)?;
        let mut copy = None::<OwnedValue>;
        for &(index, ty) in outer {
            let source = copy.as_ref().map_or(ptr, |c| c.ptr);
            // SAFETY: `source` is a valid value of `math`'s type: the navigated target, or the
            // previous component copy.
            copy = Some(unsafe { get_component(self.registry, &math, source, index)? });
            math = self.registry.live(ty)?;
        }
        let source = copy.as_ref().map_or(ptr, |c| c.ptr);
        // SAFETY: as above.
        unsafe { get_component(self.registry, &math, source, last) }
    }

    /// The value at `path`, resolved against this value's type.
    ///
    /// # Errors
    ///
    /// As [`FieldPath::resolve`] and [`ValueRef::at`].
    pub fn path(&self, path: &FieldPath) -> Result<ValueRef<'a>, ValueError> {
        self.at(&path.resolve(self.registry, self.description.key())?)
    }

    /// A copy of the value at `path`, resolved against this value's type.
    ///
    /// # Errors
    ///
    /// As [`FieldPath::resolve`] and [`ValueRef::read_at`].
    pub fn read_path(&self, path: &FieldPath) -> Result<OwnedValue, ValueError> {
        self.read_at(&path.resolve(self.registry, self.description.key())?)
    }

    /// A clone of the value.
    ///
    /// # Errors
    ///
    /// [`ValueError::Operation`] if the type isn't cloneable.
    pub fn to_owned_value(&self) -> Result<OwnedValue, ValueError> {
        let slot = Slot::new(self.description.layout());
        // SAFETY: the value is valid and only read during the call (the view's invariant); the
        // slot is aligned, writable for the type and doesn't overlap it.
        unsafe {
            self.description
                .clone_value(self.ptr.as_ptr(), slot.ptr.as_ptr())?;
        }
        // SAFETY: `clone_value` succeeded, so the slot holds a valid value.
        Ok(unsafe { slot.into_owned(self.registry.id(), Arc::clone(&self.description)) })
    }

    /// Checks that `path` belongs to this value and can be followed through it now (list
    /// indices in bounds, options `Some`).
    pub(crate) fn check_reachable(&self, path: &ResolvedPath) -> Result<(), ValueError> {
        self.check_path(path)?;
        // SAFETY: the view's invariant; the path was resolved for this type and registry.
        unsafe { navigate(self.registry, &self.description, self.ptr, path, false) }.map(|_| ())
    }

    fn check_path(&self, path: &ResolvedPath) -> Result<(), ValueError> {
        if path.registry != self.registry.id() {
            return Err(ValueError::WrongRegistry);
        }
        if path.root != self.description.key() {
            return Err(type_mismatch(self.registry, path.root, &self.description));
        }
        Ok(())
    }

    /// # Safety
    ///
    /// `ptr` points to a valid value of type `key` inside this view's value.
    unsafe fn child(&self, ptr: NonNull<u8>, key: TypeKey) -> Result<ValueRef<'a>, ValueError> {
        Ok(ValueRef {
            registry: self.registry,
            description: self.registry.live(key)?,
            ptr,
            borrow: PhantomData,
        })
    }
}

impl<'a> ValueMut<'a> {
    /// An exclusive view of the value of type `key` at `ptr`.
    ///
    /// # Errors
    ///
    /// [`ValueError::UnknownType`] if `key` isn't a live type of `registry`.
    ///
    /// # Safety
    ///
    /// `ptr` points to a valid, aligned value of the type `key` names in `registry`, writable
    /// (with provenance that allows writing). For `'a` it stays valid and nothing else accesses
    /// it.
    pub unsafe fn from_raw(
        registry: &'a TypeRegistry,
        key: TypeKey,
        ptr: NonNull<u8>,
    ) -> Result<Self, ValueError> {
        Ok(Self {
            registry,
            description: registry.live(key)?,
            ptr,
            borrow: PhantomData,
        })
    }

    /// The value's type.
    #[must_use]
    pub fn description(&self) -> &TypeDescription {
        &self.description
    }

    /// A shared view of the value, for reading.
    #[must_use]
    pub fn as_ref(&self) -> ValueRef<'_> {
        ValueRef {
            registry: self.registry,
            description: Arc::clone(&self.description),
            ptr: self.ptr,
            borrow: PhantomData,
        }
    }

    /// A shorter exclusive view of the same value.
    pub fn reborrow(&mut self) -> ValueMut<'_> {
        ValueMut {
            registry: self.registry,
            description: Arc::clone(&self.description),
            ptr: self.ptr,
            borrow: PhantomData,
        }
    }

    /// The value as a `T`.
    ///
    /// # Errors
    ///
    /// [`ValueError::TypeMismatch`] if the value isn't a `T`.
    pub fn downcast_mut<T: Reflect>(&mut self) -> Result<&mut T, ValueError> {
        check_type::<T>(&self.description)?;
        // SAFETY: the value is a `T` (checked above), and this view has exclusive access to it
        // for as long as the returned borrow of `self` lives.
        Ok(unsafe { self.ptr.cast::<T>().as_mut() })
    }

    /// The struct field `name` (or a field with `name` as a former name).
    ///
    /// # Errors
    ///
    /// As [`ValueRef::field`].
    pub fn field_mut(&mut self, name: &str) -> Result<ValueMut<'_>, ValueError> {
        let (offset, key) = field_of(&self.description, name)?;
        // SAFETY: as in `ValueRef::field`; the exclusive borrow of `self` covers the field.
        unsafe { self.child(self.ptr.add(offset), key) }
    }

    /// The element at `index` of an array or list.
    ///
    /// # Errors
    ///
    /// As [`ValueRef::element`].
    pub fn element_mut(&mut self, index: usize) -> Result<ValueMut<'_>, ValueError> {
        // SAFETY: the view's invariant; `element` checks the kind and bounds.
        let (ptr, key) = unsafe { element(&self.description, self.ptr, index, true)? };
        // SAFETY: an element of the value, reached for writing.
        unsafe { self.child(ptr, key) }
    }

    /// The value inside an `Option`, or `None`.
    ///
    /// # Errors
    ///
    /// As [`ValueRef::some`].
    pub fn some_mut(&mut self) -> Result<Option<ValueMut<'_>>, ValueError> {
        // SAFETY: the view's invariant; `payload` checks the kind.
        match unsafe { payload(&self.description, self.ptr, true)? } {
            // SAFETY: the payload, reached for writing.
            Some((ptr, key)) => unsafe { self.child(ptr, key) }.map(Some),
            None => Ok(None),
        }
    }

    /// Sets a unit enum to the variant at `index`, in [`UnitEnumInfo::variants`] order.
    ///
    /// [`UnitEnumInfo::variants`]: crate::UnitEnumInfo::variants
    ///
    /// # Errors
    ///
    /// If the value isn't a unit enum or has no such variant.
    pub fn set_variant(&mut self, index: usize) -> Result<(), ValueError> {
        let Access::Enum(access) = &self.description.access else {
            return Err(wrong_kind(&self.description, "a unit enum"));
        };
        // SAFETY: the value is a valid, exclusively borrowed enum of this description's type,
        // and the accessor was made for that type.
        if unsafe { (access.set)(self.ptr.as_ptr(), index) } {
            Ok(())
        } else {
            Err(ValueError::NoSuchVariant {
                ty: self.description.name().to_owned(),
                index,
            })
        }
    }

    /// Replaces the math component `name` with `value`, returning the old component.
    ///
    /// # Errors
    ///
    /// If the value isn't a math type, has no such component, or `value` has the wrong type.
    pub fn set_component(
        &mut self,
        name: &str,
        value: OwnedValue,
    ) -> Result<OwnedValue, ValueError> {
        let index = component_index(&self.description, name)?;
        // SAFETY: the view's invariant; `index` is one of the type's components.
        unsafe { set_component(self.registry, &self.description, self.ptr, index, value) }
    }

    /// Replaces the value with `value`, returning the old one. Nothing is dropped.
    ///
    /// # Errors
    ///
    /// [`ValueError::TypeMismatch`] if `value` has another type, or [`ValueError::WrongRegistry`]
    /// if it belongs to another registry.
    pub fn replace(&mut self, value: OwnedValue) -> Result<OwnedValue, ValueError> {
        check_owned(self.registry, &self.description, &value)?;
        // SAFETY: both pointers hold valid values of the same type (checked above), in distinct
        // allocations: the view's value is borrowed, the owned value owns its own. Swapping their
        // bytes moves each value into the other place.
        unsafe {
            ptr::swap_nonoverlapping(
                self.ptr.as_ptr(),
                value.ptr.as_ptr(),
                self.description.layout().size(),
            );
        }
        Ok(value)
    }

    /// Appends `value` to a list.
    ///
    /// # Errors
    ///
    /// If the value isn't a list, or `value` isn't of its element type.
    pub fn push(&mut self, value: OwnedValue) -> Result<(), ValueError> {
        let (Access::List(access), TypeKind::List(info)) =
            (&self.description.access, self.description.kind())
        else {
            return Err(wrong_kind(&self.description, "a list"));
        };
        let element = self.registry.live(info.element())?;
        check_owned(self.registry, &element, &value)?;
        let (value, description) = value.into_raw_parts();
        // Freed after the value moves out, even if the push panics (which drops the value).
        let _allocation = Slot {
            ptr: value,
            layout: description.layout(),
        };
        // SAFETY: the list is valid and exclusively borrowed; `value` is a valid element (checked
        // above) whose ownership moves into the list.
        unsafe { (access.push)(self.ptr.as_ptr(), value.as_ptr()) };
        Ok(())
    }

    /// Removes and returns the element at `index` of a list.
    ///
    /// # Errors
    ///
    /// If the value isn't a list, or `index` is out of bounds.
    pub fn remove(&mut self, index: usize) -> Result<OwnedValue, ValueError> {
        let (Access::List(access), TypeKind::List(info)) =
            (&self.description.access, self.description.kind())
        else {
            return Err(wrong_kind(&self.description, "a list"));
        };
        // SAFETY: the view's invariant.
        let len = unsafe { (access.len)(self.ptr.as_ptr()) };
        if index >= len {
            return Err(out_of_bounds(&self.description, index, len));
        }
        let element = self.registry.live(info.element())?;
        let slot = Slot::new(element.layout());
        // SAFETY: the list is valid and exclusively borrowed, `index` is in bounds, and the slot
        // is aligned and writable for an element.
        unsafe { (access.remove)(self.ptr.as_ptr(), index, slot.ptr.as_ptr()) };
        // SAFETY: `remove` moved the element into the slot.
        Ok(unsafe { slot.into_owned(self.registry.id(), element) })
    }

    /// Sets an `Option` to `Some(value)` or `None`, dropping its old value.
    ///
    /// # Errors
    ///
    /// If the value isn't an option, or `value` isn't of its payload type.
    pub fn set_option(&mut self, value: Option<OwnedValue>) -> Result<(), ValueError> {
        let (Access::Option(access), TypeKind::Option(info)) =
            (&self.description.access, self.description.kind())
        else {
            return Err(wrong_kind(&self.description, "an option"));
        };
        let Some(value) = value else {
            // SAFETY: the option is valid and exclusively borrowed; null means `None`.
            unsafe { (access.set)(self.ptr.as_ptr(), ptr::null_mut()) };
            return Ok(());
        };
        let payload = self.registry.live(info.some())?;
        check_owned(self.registry, &payload, &value)?;
        let (value, description) = value.into_raw_parts();
        // Freed after the value moves out, even if dropping the old payload panics.
        let _allocation = Slot {
            ptr: value,
            layout: description.layout(),
        };
        // SAFETY: the option is valid and exclusively borrowed; `value` is a valid payload
        // (checked above) whose ownership moves into the option.
        unsafe { (access.set)(self.ptr.as_ptr(), value.as_ptr()) };
        Ok(())
    }

    /// The value at `path`, which must not end inside a math value.
    ///
    /// # Errors
    ///
    /// As [`ValueRef::at`].
    pub fn at_mut(&mut self, path: &ResolvedPath) -> Result<ValueMut<'_>, ValueError> {
        if path.ends_in_math_component() {
            return Err(ValueError::MathComponentBorrow {
                ty: self.description.name().to_owned(),
            });
        }
        self.as_ref().check_path(path)?;
        // SAFETY: the view's invariant; the path was resolved for this type and registry.
        let ptr = unsafe { navigate(self.registry, &self.description, self.ptr, path, true)? };
        // SAFETY: the path's target, reached for writing.
        unsafe { self.child(ptr, path.leaf) }
    }

    /// Replaces the value at `path` with `value`, returning the old one. A path that ends inside
    /// a math value copies the math value out, changes the component and copies it back.
    ///
    /// # Errors
    ///
    /// As [`ValueRef::read_at`], or [`ValueError::TypeMismatch`] if `value` isn't of the path's
    /// leaf type. Nothing changes then.
    pub fn replace_at(
        &mut self,
        path: &ResolvedPath,
        value: OwnedValue,
    ) -> Result<OwnedValue, ValueError> {
        self.as_ref().check_path(path)?;
        let leaf = self.registry.live(path.leaf)?;
        check_owned(self.registry, &leaf, &value)?;
        // SAFETY: the view's invariant; the path was resolved for this type and registry.
        let ptr = unsafe { navigate(self.registry, &self.description, self.ptr, path, true)? };
        if path.components.is_empty() {
            // SAFETY: the path's target, of the leaf type, reached for writing.
            return unsafe { self.child(ptr, path.leaf) }?.replace(value);
        }
        let owner = path.steps.last().map_or(self.description.key(), |s| s.ty);
        let math = self.registry.live(owner)?;
        // SAFETY: `ptr` is a valid, exclusively borrowed value of `math`'s type, and the
        // components were resolved from it.
        unsafe { replace_in_math(self.registry, &math, ptr, &path.components, value) }
    }

    /// Replaces the value at `path`, resolved against this value's type, returning the old one.
    ///
    /// # Errors
    ///
    /// As [`FieldPath::resolve`] and [`ValueMut::replace_at`].
    pub fn replace_path(
        &mut self,
        path: &FieldPath,
        value: OwnedValue,
    ) -> Result<OwnedValue, ValueError> {
        let resolved = path.resolve(self.registry, self.description.key())?;
        self.replace_at(&resolved, value)
    }

    /// Replaces the value with one read from `bytes`, returning the old one. Only plain data
    /// (see [`OwnedValue::from_bytes`]) can be written this way, and the bytes are validated
    /// first.
    ///
    /// # Errors
    ///
    /// As [`OwnedValue::from_bytes`].
    pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<OwnedValue, ValueError> {
        let value = OwnedValue::from_bytes(self.registry, self.description.key(), bytes)?;
        self.replace(value)
    }

    /// # Safety
    ///
    /// `ptr` points to a valid value of type `key` inside this view's value, reached with write
    /// provenance.
    unsafe fn child(&mut self, ptr: NonNull<u8>, key: TypeKey) -> Result<ValueMut<'_>, ValueError> {
        Ok(ValueMut {
            registry: self.registry,
            description: self.registry.live(key)?,
            ptr,
            borrow: PhantomData,
        })
    }
}

impl OwnedValue {
    /// `value`, owned through its description, registering `T` if it isn't registered yet.
    ///
    /// # Errors
    ///
    /// [`ValueError::Registration`] if `T` can't be registered.
    pub fn new<T: Reflect>(registry: &TypeRegistry, value: T) -> Result<Self, ValueError> {
        let key = registry.register::<T>()?;
        let description = registry.live(key)?;
        let slot = Slot::new(description.layout());
        // SAFETY: the slot is aligned and writable for a `T` (its layout is `T`'s).
        unsafe { slot.ptr.cast::<T>().write(value) };
        // SAFETY: the slot now holds a valid `T`, the description's type.
        Ok(unsafe { slot.into_owned(registry.id(), description) })
    }

    /// The default value of the type `key`.
    ///
    /// # Errors
    ///
    /// If `key` isn't a live type of `registry`, or the type has no default.
    pub fn default_of(registry: &TypeRegistry, key: TypeKey) -> Result<Self, ValueError> {
        let description = registry.live(key)?;
        let slot = Slot::new(description.layout());
        // SAFETY: the slot is aligned and writable for the type.
        unsafe { description.default_value(slot.ptr.as_ptr())? };
        // SAFETY: `default_value` succeeded, so the slot holds a valid value.
        Ok(unsafe { slot.into_owned(registry.id(), description) })
    }

    /// A value of the type `key` read from `bytes`, its in-memory representation in this build.
    ///
    /// Only **plain data** can be read this way: primitives, unit enums with an explicit `repr`,
    /// handles, fixed arrays of plain data, and structs whose fields are all plain data. Every
    /// byte is validated before any Rust code sees the value (ADR-0021 §4): a `bool` must be 0
    /// or 1 and an enum's discriminant must name a variant. Padding bytes are ignored.
    ///
    /// # Errors
    ///
    /// If `key` isn't a live type, the type isn't plain data, `bytes` has the wrong length, or
    /// a byte is invalid ([`ValueError::InvalidBytes`], naming the type and offset).
    pub fn from_bytes(
        registry: &TypeRegistry,
        key: TypeKey,
        bytes: &[u8],
    ) -> Result<Self, ValueError> {
        let description = registry.live(key)?;
        if bytes.len() != description.layout().size() {
            return Err(ValueError::WrongSize {
                ty: description.name().to_owned(),
                expected: description.layout().size(),
                found: bytes.len(),
            });
        }
        crate::validate::validate(registry, &description, bytes, 0)?;
        let slot = Slot::new(description.layout());
        // SAFETY: the slot is writable for the type's size, which is `bytes.len()`; the two
        // don't overlap.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), slot.ptr.as_ptr(), bytes.len()) };
        // SAFETY: the bytes form a valid value: the type is plain data with no `Drop`, whose
        // validity depends only on the bytes `validate` checked.
        Ok(unsafe { slot.into_owned(registry.id(), description) })
    }

    /// The value's type.
    #[must_use]
    pub fn description(&self) -> &TypeDescription {
        &self.description
    }

    /// A shared view of the value.
    ///
    /// # Errors
    ///
    /// [`ValueError::WrongRegistry`] if `registry` isn't the one the value belongs to.
    pub fn as_ref<'a>(&'a self, registry: &'a TypeRegistry) -> Result<ValueRef<'a>, ValueError> {
        if registry.id() != self.registry {
            return Err(ValueError::WrongRegistry);
        }
        Ok(ValueRef {
            registry,
            description: Arc::clone(&self.description),
            ptr: self.ptr,
            borrow: PhantomData,
        })
    }

    /// An exclusive view of the value.
    ///
    /// # Errors
    ///
    /// [`ValueError::WrongRegistry`] if `registry` isn't the one the value belongs to.
    pub fn as_mut<'a>(
        &'a mut self,
        registry: &'a TypeRegistry,
    ) -> Result<ValueMut<'a>, ValueError> {
        if registry.id() != self.registry {
            return Err(ValueError::WrongRegistry);
        }
        Ok(ValueMut {
            registry,
            description: Arc::clone(&self.description),
            ptr: self.ptr,
            borrow: PhantomData,
        })
    }

    /// The value as a `T`.
    ///
    /// # Errors
    ///
    /// [`ValueError::TypeMismatch`] if the value isn't a `T`.
    pub fn downcast_ref<T: Reflect>(&self) -> Result<&T, ValueError> {
        check_type::<T>(&self.description)?;
        // SAFETY: the value is a `T` (checked above), owned by `self`.
        Ok(unsafe { self.ptr.cast::<T>().as_ref() })
    }

    /// The value as a `T`, moved out.
    ///
    /// # Errors
    ///
    /// The value itself, unchanged, if it isn't a `T`.
    pub fn into_value<T: Reflect>(self) -> Result<T, Self> {
        if check_type::<T>(&self.description).is_err() {
            return Err(self);
        }
        let (ptr, description) = self.into_raw_parts();
        let _allocation = Slot {
            ptr,
            layout: description.layout(),
        };
        // SAFETY: the value is a `T` (checked above) whose ownership moves out here; only its
        // allocation remains, freed by `_allocation`.
        Ok(unsafe { ptr.cast::<T>().read() })
    }

    /// A clone of the value.
    ///
    /// # Errors
    ///
    /// [`ValueError::Operation`] if the type isn't cloneable.
    pub fn try_clone(&self) -> Result<Self, ValueError> {
        let slot = Slot::new(self.description.layout());
        // SAFETY: the value is valid and only read; the slot is aligned, writable for the type
        // and doesn't overlap it.
        unsafe {
            self.description
                .clone_value(self.ptr.as_ptr(), slot.ptr.as_ptr())?;
        }
        // SAFETY: `clone_value` succeeded, so the slot holds a valid value.
        Ok(unsafe { slot.into_owned(self.registry, Arc::clone(&self.description)) })
    }

    pub(crate) fn registry_id(&self) -> RegistryId {
        self.registry
    }

    /// The value's pointer and description, without dropping either: the caller now owns the
    /// value and its allocation (`Slot::new` with the description's layout).
    fn into_raw_parts(self) -> (NonNull<u8>, Arc<TypeDescription>) {
        let this = ManuallyDrop::new(self);
        // SAFETY: the description is read out exactly once, and `this` is never used or dropped
        // again, so the `Arc` isn't released twice.
        let description = unsafe { ptr::read(&this.description) };
        (this.ptr, description)
    }
}

impl Drop for OwnedValue {
    fn drop(&mut self) {
        // Free the allocation even if the value's drop panics.
        let allocation = Slot {
            ptr: self.ptr,
            layout: self.description.layout(),
        };
        // SAFETY: the value is valid and owned, and never used again.
        unsafe { self.description.drop_value(self.ptr.as_ptr()) };
        drop(allocation);
    }
}

impl fmt::Debug for OwnedValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("OwnedValue")
            .field(&self.description.name())
            .finish()
    }
}

impl fmt::Debug for ValueRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ValueRef")
            .field(&self.description.name())
            .finish()
    }
}

impl fmt::Debug for ValueMut<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ValueMut")
            .field(&self.description.name())
            .finish()
    }
}

/// An allocation for one value, freed when dropped unless it becomes an [`OwnedValue`], so a
/// failed or panicking construction never leaks it. Zero-sized types get a dangling, aligned
/// pointer and no allocation.
struct Slot {
    ptr: NonNull<u8>,
    layout: Layout,
}

impl Slot {
    fn new(layout: Layout) -> Self {
        let ptr = if layout.size() == 0 {
            // An aligned, non-null address without provenance: valid for zero-sized accesses.
            NonNull::new(ptr::without_provenance_mut(layout.align()))
                .expect("alignments are never zero")
        } else {
            // SAFETY: the size is non-zero.
            let ptr = unsafe { alloc::alloc(layout) };
            NonNull::new(ptr).unwrap_or_else(|| alloc::handle_alloc_error(layout))
        };
        Self { ptr, layout }
    }

    /// # Safety
    ///
    /// The slot holds a valid value of `description`'s type, whose layout is the slot's.
    unsafe fn into_owned(
        self,
        registry: RegistryId,
        description: Arc<TypeDescription>,
    ) -> OwnedValue {
        debug_assert_eq!(self.layout, description.layout());
        let slot = ManuallyDrop::new(self);
        OwnedValue {
            registry,
            description,
            ptr: slot.ptr,
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        // SAFETY: the slot was allocated by `Slot::new` with this layout, and nothing owns it.
        unsafe { deallocate(self.ptr, self.layout) };
    }
}

/// # Safety
///
/// `ptr` was returned by `Slot::new(layout)` and isn't used afterwards.
unsafe fn deallocate(ptr: NonNull<u8>, layout: Layout) {
    if layout.size() != 0 {
        // SAFETY: allocated by the global allocator with this layout (caller's contract).
        unsafe { alloc::dealloc(ptr.as_ptr(), layout) };
    }
}

fn check_type<T: Reflect>(description: &TypeDescription) -> Result<(), ValueError> {
    if description.type_id == Some(TypeId::of::<T>()) {
        Ok(())
    } else {
        Err(ValueError::TypeMismatch {
            expected: T::DEFINITION.name().to_owned(),
            found: description.name().to_owned(),
        })
    }
}

fn check_owned(
    registry: &TypeRegistry,
    expected: &TypeDescription,
    value: &OwnedValue,
) -> Result<(), ValueError> {
    if value.registry_id() != registry.id() {
        return Err(ValueError::WrongRegistry);
    }
    if value.description.key() != expected.key() {
        return Err(ValueError::TypeMismatch {
            expected: expected.name().to_owned(),
            found: value.description.name().to_owned(),
        });
    }
    Ok(())
}

fn type_mismatch(
    registry: &TypeRegistry,
    expected: TypeKey,
    found: &TypeDescription,
) -> ValueError {
    ValueError::TypeMismatch {
        expected: registry
            .description(expected)
            .map_or_else(|| expected.to_string(), |d| d.name().to_owned()),
        found: found.name().to_owned(),
    }
}

fn wrong_kind(description: &TypeDescription, expected: &'static str) -> ValueError {
    ValueError::WrongKind {
        ty: description.name().to_owned(),
        expected,
    }
}

fn out_of_bounds(description: &TypeDescription, index: usize, len: usize) -> ValueError {
    ValueError::IndexOutOfBounds {
        ty: description.name().to_owned(),
        index,
        len,
    }
}

/// The offset and type of the struct field `name`.
fn field_of(description: &TypeDescription, name: &str) -> Result<(usize, TypeKey), ValueError> {
    let TypeKind::Struct(info) = description.kind() else {
        return Err(wrong_kind(description, "a struct"));
    };
    let (_, field) = info.field(name).ok_or_else(|| ValueError::NoSuchField {
        ty: description.name().to_owned(),
        field: name.to_owned(),
    })?;
    let key = field.ty().ok_or_else(|| ValueError::OpaqueField {
        ty: description.name().to_owned(),
        field: field.name().to_owned(),
    })?;
    Ok((field.offset(), key))
}

fn component_index(description: &TypeDescription, name: &str) -> Result<usize, ValueError> {
    let TypeKind::Math(info) = description.kind() else {
        return Err(wrong_kind(description, "a math type"));
    };
    info.components()
        .iter()
        .position(|c| c.name() == name)
        .ok_or_else(|| ValueError::NoSuchField {
            ty: description.name().to_owned(),
            field: name.to_owned(),
        })
}

/// # Safety
///
/// `ptr` points to a valid value of `description`'s type.
unsafe fn len(description: &TypeDescription, ptr: NonNull<u8>) -> Result<usize, ValueError> {
    match (description.kind(), &description.access) {
        (TypeKind::Array(info), _) => Ok(info.len()),
        // SAFETY: the caller's contract; the accessor belongs to this list type.
        (TypeKind::List(_), Access::List(access)) => Ok(unsafe { (access.len)(ptr.as_ptr()) }),
        _ => Err(wrong_kind(description, "an array or list")),
    }
}

/// The element at `index` and its type.
///
/// # Safety
///
/// `ptr` points to a valid value of `description`'s type; when `mutable`, it's exclusively
/// borrowed with write provenance.
unsafe fn element(
    description: &TypeDescription,
    ptr: NonNull<u8>,
    index: usize,
    mutable: bool,
) -> Result<(NonNull<u8>, TypeKey), ValueError> {
    // SAFETY: the caller's contract.
    let len = unsafe { len(description, ptr)? };
    if index >= len {
        return Err(out_of_bounds(description, index, len));
    }
    match (description.kind(), &description.access) {
        (TypeKind::Array(info), _) => {
            let stride = description.layout().size() / info.len();
            // SAFETY: `index < len`, so the element lies inside the array.
            Ok((unsafe { ptr.add(index * stride) }, info.element()))
        }
        (TypeKind::List(info), Access::List(access)) => {
            let element = if mutable {
                // SAFETY: the caller's contract; `index` is in bounds.
                unsafe { (access.element_mut)(ptr.as_ptr(), index) }
            } else {
                // SAFETY: the caller's contract; `index` is in bounds.
                unsafe { (access.element)(ptr.as_ptr(), index) }.cast_mut()
            };
            let element = NonNull::new(element).expect("list elements are never null");
            Ok((element, info.element()))
        }
        _ => Err(wrong_kind(description, "an array or list")),
    }
}

/// The payload of an `Option` and its type, or `None`.
///
/// # Safety
///
/// As [`element`].
unsafe fn payload(
    description: &TypeDescription,
    ptr: NonNull<u8>,
    mutable: bool,
) -> Result<Option<(NonNull<u8>, TypeKey)>, ValueError> {
    let (TypeKind::Option(info), Access::Option(access)) =
        (description.kind(), &description.access)
    else {
        return Err(wrong_kind(description, "an option"));
    };
    let payload = if mutable {
        // SAFETY: the caller's contract.
        unsafe { (access.get_mut)(ptr.as_ptr()) }
    } else {
        // SAFETY: the caller's contract.
        unsafe { (access.get)(ptr.as_ptr()) }.cast_mut()
    };
    Ok(NonNull::new(payload).map(|payload| (payload, info.some())))
}

/// Follows `path`'s steps (not its math components) from `ptr`.
///
/// # Safety
///
/// As [`element`], for `root`'s type; `path` was resolved for that type in `registry`.
unsafe fn navigate(
    registry: &TypeRegistry,
    root: &TypeDescription,
    mut ptr: NonNull<u8>,
    path: &ResolvedPath,
    mutable: bool,
) -> Result<NonNull<u8>, ValueError> {
    let mut container = None::<Arc<TypeDescription>>;
    for step in &path.steps {
        let description = container.as_deref().unwrap_or(root);
        debug_assert_eq!(description.key(), step.container);
        ptr = match step.step {
            // SAFETY: resolution read the offset from this type's description, so the inner
            // value lies inside the current one.
            Step::Field { offset, .. } | Step::ArrayElement { offset } => unsafe {
                ptr.add(offset)
            },
            Step::ListElement { index } => {
                // SAFETY: the caller's contract, for the current value.
                unsafe { element(description, ptr, index, mutable) }?.0
            }
            // SAFETY: the caller's contract, for the current value.
            Step::Some => match unsafe { payload(description, ptr, mutable) }? {
                Some((payload, _)) => payload,
                None => {
                    return Err(ValueError::NoValue {
                        ty: description.name().to_owned(),
                    });
                }
            },
        };
        container = Some(registry.live(step.ty)?);
    }
    Ok(ptr)
}

/// A copy of component `index` of the math value at `ptr`.
///
/// # Safety
///
/// `ptr` points to a valid value of `math`'s type, a math type with that component.
unsafe fn get_component(
    registry: &TypeRegistry,
    math: &TypeDescription,
    ptr: NonNull<u8>,
    index: usize,
) -> Result<OwnedValue, ValueError> {
    let (TypeKind::Math(info), Access::Math(accessors)) = (math.kind(), &math.access) else {
        return Err(wrong_kind(math, "a math type"));
    };
    let component = registry.live(info.components()[index].ty())?;
    let slot = Slot::new(component.layout());
    // SAFETY: `ptr` is a valid math value (caller's contract); the slot is aligned and
    // writable for the component's type; the getter belongs to this math type.
    unsafe { (accessors[index].0)(ptr.as_ptr(), slot.ptr.as_ptr()) };
    // SAFETY: the getter wrote a valid component value into the slot.
    Ok(unsafe { slot.into_owned(registry.id(), component) })
}

/// Sets component `index` of the math value at `ptr` to `value`, returning the old component.
///
/// # Safety
///
/// `ptr` points to a valid, exclusively borrowed value of `math`'s type, a math type with
/// that component, with write provenance.
unsafe fn set_component(
    registry: &TypeRegistry,
    math: &TypeDescription,
    ptr: NonNull<u8>,
    index: usize,
    value: OwnedValue,
) -> Result<OwnedValue, ValueError> {
    let (TypeKind::Math(info), Access::Math(accessors)) = (math.kind(), &math.access) else {
        return Err(wrong_kind(math, "a math type"));
    };
    let component = registry.live(info.components()[index].ty())?;
    check_owned(registry, &component, &value)?;
    // SAFETY: the caller's contract.
    let old = unsafe { get_component(registry, math, ptr, index)? };
    // SAFETY: the caller's contract; `value` is a valid component value (checked above), which
    // the setter copies. Components are plain values without `Drop`, so `value` is dropped
    // normally afterwards.
    unsafe { (accessors[index].1)(ptr.as_ptr(), value.ptr.as_ptr()) };
    Ok(old)
}

/// Replaces the value at the end of a chain of math components, copying each enclosing math
/// value out and back in, and returns the old value.
///
/// # Safety
///
/// As [`set_component`], for the first component of `components`; each later component
/// belongs to the type of the one before it.
unsafe fn replace_in_math(
    registry: &TypeRegistry,
    math: &TypeDescription,
    ptr: NonNull<u8>,
    components: &[(usize, TypeKey)],
    value: OwnedValue,
) -> Result<OwnedValue, ValueError> {
    let Some((&(index, ty), rest)) = components.split_first() else {
        unreachable!("callers pass at least one component");
    };
    if rest.is_empty() {
        // SAFETY: the caller's contract.
        return unsafe { set_component(registry, math, ptr, index, value) };
    }
    // SAFETY: the caller's contract.
    let inner = unsafe { get_component(registry, math, ptr, index)? };
    let inner_math = registry.live(ty)?;
    // SAFETY: `inner` is an owned, valid value of `inner_math`'s type, and `rest` was resolved
    // from it.
    let old = unsafe { replace_in_math(registry, &inner_math, inner.ptr, rest, value)? };
    // SAFETY: `inner` is a valid component value of the right type; the setter copies it, and
    // `inner` (plain math data) is then dropped normally.
    unsafe { (math_setter(math, index))(ptr.as_ptr(), inner.ptr.as_ptr()) };
    Ok(old)
}

fn math_setter(math: &TypeDescription, index: usize) -> crate::access::ComponentSet {
    match &math.access {
        Access::Math(accessors) => accessors[index].1,
        _ => unreachable!("resolved math components belong to math types"),
    }
}

/// Reading or writing a value through reflection failed (ADR-0021 §4).
///
/// Every caller mistake is an error, because paths and values come from files, the editor and
/// other languages.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ValueError {
    /// The value's kind doesn't support the request, such as a field of a list.
    #[error("'{ty}' isn't {expected}")]
    WrongKind {
        /// The value's type.
        ty: String,
        /// What the request needs, such as "a struct".
        expected: &'static str,
    },
    /// The struct or math type has no such field or component.
    #[error("'{ty}' has no field '{field}'")]
    NoSuchField {
        /// The type.
        ty: String,
        /// The requested name.
        field: String,
    },
    /// The field is opaque (`#[reflect(opaque)]`), so it can't be reached.
    #[error("field '{field}' of '{ty}' is opaque and can't be read or written through reflection")]
    OpaqueField {
        /// The type.
        ty: String,
        /// The field.
        field: String,
    },
    /// An array or list index is out of bounds.
    #[error("index {index} is out of bounds for '{ty}' of length {len}")]
    IndexOutOfBounds {
        /// The array or list type.
        ty: String,
        /// The requested index.
        index: usize,
        /// The length.
        len: usize,
    },
    /// An option on the way is `None`.
    #[error("'{ty}' is None")]
    NoValue {
        /// The option type.
        ty: String,
    },
    /// A value or path of another type was given.
    #[error("expected a value of type '{expected}', found '{found}'")]
    TypeMismatch {
        /// The type required.
        expected: String,
        /// The type given.
        found: String,
    },
    /// The unit enum has no variant with this index.
    #[error("'{ty}' has no variant {index}")]
    NoSuchVariant {
        /// The enum type.
        ty: String,
        /// The requested index.
        index: usize,
    },
    /// A path continues after a math component with something other than another component.
    #[error("inside the math value '{ty}', a path can only name further components")]
    AfterMathComponent {
        /// The math component's type.
        ty: String,
    },
    /// A path that ends inside a math value was used to borrow, but math components can only be
    /// copied (`read_at`, `replace_at`), because glam's SIMD types hide their fields.
    #[error(
        "the path into '{ty}' ends inside a math value, which can only be copied: use read_at or \
         replace_at"
    )]
    MathComponentBorrow {
        /// The type the path starts at.
        ty: String,
    },
    /// The type key isn't a live type of the registry.
    #[error("type {key} isn't registered (or was removed)")]
    UnknownType {
        /// The key.
        key: TypeKey,
    },
    /// A value or path from another registry was used.
    #[error("the value or path belongs to another type registry")]
    WrongRegistry,
    /// Only plain data can be read from bytes.
    #[error("'{ty}' isn't plain data, so it can't be read from bytes")]
    NotPlainData {
        /// The type, or the field type that isn't plain data.
        ty: String,
    },
    /// The byte slice has the wrong length.
    #[error("'{ty}' is {expected} bytes, but {found} were given")]
    WrongSize {
        /// The type.
        ty: String,
        /// Its size.
        expected: usize,
        /// The bytes given.
        found: usize,
    },
    /// A byte isn't valid for the type, such as a `bool` other than 0 or 1.
    #[error("invalid bytes for '{ty}' at offset {offset}: {problem}")]
    InvalidBytes {
        /// The type of the invalid part.
        ty: String,
        /// Its offset in the bytes.
        offset: usize,
        /// What's wrong.
        problem: String,
    },
    /// The type doesn't support a needed operation.
    #[error(transparent)]
    Operation(#[from] OperationError),
    /// The type couldn't be registered.
    #[error(transparent)]
    Registration(#[from] RegistrationError),
}
