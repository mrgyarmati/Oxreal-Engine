//! Erased accessors for kinds that offsets can't reach: lists, options, unit enums and math
//! components (ADR-0021 §3).
//!
//! Like [`Operations`](crate::Operations), each accessor is a function pointer monomorphized for
//! one type by this crate's `pub(crate)` constructors. Only [`crate::value`] calls them, under
//! the pointer contracts its views and owned values keep.

use std::any::TypeId;
use std::fmt;
use std::ptr;

/// A unit enum's variants by index: implemented by `#[derive(Reflect)]` for enums whose variants
/// carry no data, so reflection can read and write them without knowing their layout.
///
/// The index is the variant's position in declaration order, the order of the description's
/// variants. Implement it by hand only together with a hand-written `Reflect` impl.
pub trait UnitEnum: Sized + 'static {
    /// The index of this value's variant.
    fn variant_index(&self) -> usize;

    /// The value of the variant with this index, or `None` if there's no such variant.
    fn from_variant_index(index: usize) -> Option<Self>;
}

/// Reads and writes a unit enum by variant index.
#[derive(Clone, Copy)]
pub(crate) struct EnumAccess {
    /// The enum the accessors were made for; registration checks it's the described type.
    pub(crate) type_id: TypeId,
    /// # Safety: the pointer is to a valid, aligned value of the enum.
    pub(crate) index: unsafe fn(*const u8) -> usize,
    /// Replaces the value (dropping the old one) with the variant at the index. Returns `false`,
    /// changing nothing, if there's no such variant.
    ///
    /// # Safety: the pointer is to a valid, aligned value of the enum, writable and not aliased.
    pub(crate) set: unsafe fn(*mut u8, usize) -> bool,
}

impl EnumAccess {
    pub(crate) const fn of<T: UnitEnum>() -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            index: enum_index::<T>,
            set: enum_set::<T>,
        }
    }
}

/// # Safety
///
/// `value` points to a valid, aligned `T`.
unsafe fn enum_index<T: UnitEnum>(value: *const u8) -> usize {
    // SAFETY: the caller's contract.
    unsafe { &*value.cast::<T>() }.variant_index()
}

/// # Safety
///
/// `value` points to a valid, aligned `T` that nothing else accesses during the call.
unsafe fn enum_set<T: UnitEnum>(value: *mut u8, index: usize) -> bool {
    let Some(variant) = T::from_variant_index(index) else {
        return false;
    };
    // SAFETY: the caller's contract. Assigning drops the old value.
    unsafe { *value.cast::<T>() = variant };
    true
}

/// Reads and edits `Vec<T>` through erased pointers.
#[derive(Clone, Copy)]
pub(crate) struct ListAccess {
    /// # Safety: the pointer is to a valid, aligned `Vec<T>`.
    pub(crate) len: unsafe fn(*const u8) -> usize,
    /// The element at an index below `len`.
    ///
    /// # Safety: as `len`, and the index is in bounds.
    pub(crate) element: unsafe fn(*const u8, usize) -> *const u8,
    /// As `element`, for writing.
    ///
    /// # Safety: as `element`, and the list may be written and isn't aliased.
    pub(crate) element_mut: unsafe fn(*mut u8, usize) -> *mut u8,
    /// Moves the value at the second pointer to the end of the list.
    ///
    /// # Safety: the list may be written and isn't aliased; the second pointer is to a valid,
    /// owned `T` that the caller doesn't use or drop afterwards.
    pub(crate) push: unsafe fn(*mut u8, *mut u8),
    /// Removes the element at an index below `len`, moving it into the third pointer.
    ///
    /// # Safety: as `element_mut`; the third pointer is aligned and writable for a `T`.
    pub(crate) remove: unsafe fn(*mut u8, usize, *mut u8),
}

impl ListAccess {
    pub(crate) const fn of<T>() -> Self {
        Self {
            len: list_len::<T>,
            element: list_element::<T>,
            element_mut: list_element_mut::<T>,
            push: list_push::<T>,
            remove: list_remove::<T>,
        }
    }
}

/// # Safety
///
/// `list` points to a valid, aligned `Vec<T>`.
unsafe fn list_len<T>(list: *const u8) -> usize {
    // SAFETY: the caller's contract.
    unsafe { &*list.cast::<Vec<T>>() }.len()
}

/// # Safety
///
/// `list` points to a valid, aligned `Vec<T>`, and `index` is below its length.
unsafe fn list_element<T>(list: *const u8, index: usize) -> *const u8 {
    // SAFETY: the caller's contract.
    let list = unsafe { &*list.cast::<Vec<T>>() };
    debug_assert!(index < list.len(), "callers check the index");
    // SAFETY: `index` is in bounds (caller's contract), so the pointer is to an element.
    unsafe { list.as_ptr().add(index) }.cast()
}

/// # Safety
///
/// As [`list_element`], and nothing else accesses the list during the call.
unsafe fn list_element_mut<T>(list: *mut u8, index: usize) -> *mut u8 {
    // SAFETY: the caller's contract.
    let list = unsafe { &mut *list.cast::<Vec<T>>() };
    debug_assert!(index < list.len(), "callers check the index");
    // SAFETY: `index` is in bounds (caller's contract), so the pointer is to an element.
    unsafe { list.as_mut_ptr().add(index) }.cast()
}

/// # Safety
///
/// `list` points to a valid, aligned `Vec<T>` that nothing else accesses during the call.
/// `value` points to a valid, owned `T` that the caller doesn't use or drop afterwards.
unsafe fn list_push<T>(list: *mut u8, value: *mut u8) {
    // SAFETY: `value` is a valid `T` whose ownership moves here (caller's contract).
    let value = unsafe { value.cast::<T>().read() };
    // SAFETY: the caller's contract.
    unsafe { &mut *list.cast::<Vec<T>>() }.push(value);
}

/// # Safety
///
/// As [`list_element_mut`]; `out` is aligned and writable for a `T`.
unsafe fn list_remove<T>(list: *mut u8, index: usize, out: *mut u8) {
    // SAFETY: the caller's contract.
    let list = unsafe { &mut *list.cast::<Vec<T>>() };
    let removed = list.remove(index);
    // SAFETY: `out` is aligned and writable for a `T` (caller's contract).
    unsafe { out.cast::<T>().write(removed) };
}

/// Reads and edits `Option<T>` through erased pointers.
#[derive(Clone, Copy)]
pub(crate) struct OptionAccess {
    /// The payload, or null for `None`.
    ///
    /// # Safety: the pointer is to a valid, aligned `Option<T>`.
    pub(crate) get: unsafe fn(*const u8) -> *const u8,
    /// As `get`, for writing.
    ///
    /// # Safety: as `get`, and the option may be written and isn't aliased.
    pub(crate) get_mut: unsafe fn(*mut u8) -> *mut u8,
    /// Sets `Some` with the value at the second pointer (moved), or `None` for a null pointer,
    /// dropping the old payload.
    ///
    /// # Safety: as `get_mut`; a non-null second pointer is to a valid, owned `T` that the
    /// caller doesn't use or drop afterwards.
    pub(crate) set: unsafe fn(*mut u8, *mut u8),
}

impl OptionAccess {
    pub(crate) const fn of<T>() -> Self {
        Self {
            get: option_get::<T>,
            get_mut: option_get_mut::<T>,
            set: option_set::<T>,
        }
    }
}

/// # Safety
///
/// `option` points to a valid, aligned `Option<T>`.
unsafe fn option_get<T>(option: *const u8) -> *const u8 {
    // SAFETY: the caller's contract.
    match unsafe { &*option.cast::<Option<T>>() } {
        Some(value) => ptr::from_ref(value).cast(),
        None => ptr::null(),
    }
}

/// # Safety
///
/// As [`option_get`], and nothing else accesses the option during the call.
unsafe fn option_get_mut<T>(option: *mut u8) -> *mut u8 {
    // SAFETY: the caller's contract.
    match unsafe { &mut *option.cast::<Option<T>>() } {
        Some(value) => ptr::from_mut(value).cast(),
        None => ptr::null_mut(),
    }
}

/// # Safety
///
/// As [`option_get_mut`]; a non-null `value` points to a valid, owned `T` that the caller
/// doesn't use or drop afterwards.
unsafe fn option_set<T>(option: *mut u8, value: *mut u8) {
    let new = if value.is_null() {
        None
    } else {
        // SAFETY: `value` is a valid `T` whose ownership moves here (caller's contract).
        Some(unsafe { value.cast::<T>().read() })
    };
    // SAFETY: the caller's contract. Assigning drops the old payload.
    unsafe { *option.cast::<Option<T>>() = new };
}

/// Copies one math component out of its math value into the second pointer.
///
/// # Safety: the first pointer is to a valid, aligned math value; the second is aligned and
/// writable for the component's type.
pub(crate) type ComponentGet = unsafe fn(*const u8, *mut u8);

/// Copies the component value at the second pointer into the math value at the first.
/// The component type is `Copy`, so the source stays valid and its owner drops it normally.
///
/// # Safety: the first pointer is to a valid, aligned math value that may be written and isn't
/// aliased; the second is to a valid value of the component's type.
pub(crate) type ComponentSet = unsafe fn(*mut u8, *const u8);

/// The accessors of one registered type, by kind.
#[derive(Clone)]
pub(crate) enum Access {
    /// Reached through offsets (structs, arrays), or a leaf.
    Direct,
    List(ListAccess),
    Option(OptionAccess),
    Enum(EnumAccess),
    Math(Vec<(ComponentGet, ComponentSet)>),
}

impl fmt::Debug for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Direct => "Direct",
            Self::List(_) => "List",
            Self::Option(_) => "Option",
            Self::Enum(_) => "Enum",
            Self::Math(_) => "Math",
        })
    }
}
