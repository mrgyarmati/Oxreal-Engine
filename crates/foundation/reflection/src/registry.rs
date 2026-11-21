//! The type registry: registered descriptions, looked up by key, Rust type or stable ID
//! (ADR-0021 §5).

use std::alloc::Layout;
use std::any::TypeId;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::Reflect;
use crate::access::Access;
use crate::attributes::AttributesDefinition;
use crate::definition::{
    FieldDefinition, GenericArgDefinition, KindData, MathComponentDefinition, Persistence,
    TypeDefinition, TypeRef, VariantDefinition,
};
use crate::description::{
    ArrayInfo, Field, GenericArgument, HandleInfo, Identity, ListInfo, MathComponent, MathInfo,
    OptionInfo, StructInfo, TypeDescription, TypeKey, TypeKind, UnitEnumInfo, Variant,
};
use crate::foreign::ForeignTypeDefinition;
use crate::id::{GenericIdInput, ParseStableTypeIdError, StableTypeId};
use crate::kind::{IntRepr, Primitive};
use crate::ops::Operations;

/// The registered types of an application (ADR-0021 §5).
///
/// An ordinary value, never a global: the application creates one and shares it (`Arc`) with
/// the worlds, tools and bindings that need it. Its data sits behind an internal lock, so every
/// method takes `&self` and the registry can be shared between threads.
///
/// - **Registering a type registers the types it refers to:** field, element, component and
///   generic argument types.
/// - **Registration is all or nothing.** If any type in a registration fails, for example
///   because two types declare the same stable ID, none of them becomes visible.
/// - **Removal leaves a tombstone:** a removed type's [`TypeKey`] is never reused.
/// - Types are registered into **groups**. The root group holds the built-in types and
///   everything registered with [`TypeRegistry::register`]. Plugins and hot reload use
///   their own groups ([`TypeRegistry::group`]), which become visible together and are removed
///   together.
///
/// # Lock order
///
/// When code needs both, it takes the ECS component registry's lock **before** this registry's,
/// never the reverse (ADR-0021 §5). This registry never calls out to other code while it holds
/// its lock.
pub struct TypeRegistry {
    id: RegistryId,
    inner: RwLock<Inner>,
}

/// A registry's identity, unique within the process (ADR-0021 §5).
///
/// Values and keys from one registry must never be used with another. Code that holds keys or
/// IDs resolved against a registry records its identity and checks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegistryId(u64);

impl RegistryId {
    fn unique() -> Self {
        // A process-wide counter, not shared state: it holds nothing about any registry, only
        // guarantees that no two registries get the same number (CLAUDE.md rule 8 forbids global
        // state as a shortcut; this is identity, like `TypeId`). 64 bits never run out.
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// A registration group: types that become visible together and are removed together
/// (ADR-0021 §5).
///
/// A group ID belongs to the registry that issued it; another registry refuses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupId {
    registry: RegistryId,
    index: u32,
}

impl GroupId {
    fn root(registry: RegistryId) -> Self {
        Self { registry, index: 0 }
    }

    fn is_root(self) -> bool {
        self.index == 0
    }
}

struct Inner {
    id: RegistryId,
    entries: Vec<Entry>,
    by_type_id: HashMap<TypeId, TypeKey>,
    by_stable_id: HashMap<StableTypeId, TypeKey>,
    groups: Vec<Group>,
}

enum Entry {
    /// Being registered: known well enough for the types that refer to it, which may include
    /// itself (`struct Node { children: Vec<Node> }`).
    Pending(Summary),
    Live(Arc<TypeDescription>),
    /// Removed with its group. The key is never reused.
    Removed,
}

/// What registering a type needs to know about the types it refers to.
#[derive(Clone)]
struct Summary {
    identity: Identity,
    name: Cow<'static, str>,
    layout: Layout,
    operations: Operations,
    abi: bool,
    group: GroupId,
}

struct Group {
    name: String,
    live: bool,
}

impl TypeRegistry {
    /// An empty registry with the built-in types registered: the primitives, `String` and the
    /// `oxreal-math` types.
    #[must_use]
    pub fn new() -> Self {
        let id = RegistryId::unique();
        let registry = Self {
            id,
            inner: RwLock::new(Inner {
                id,
                entries: Vec::new(),
                by_type_id: HashMap::new(),
                by_stable_id: HashMap::new(),
                groups: vec![Group {
                    name: "root".to_owned(),
                    live: true,
                }],
            }),
        };
        crate::builtin::register_types(&registry);
        registry
    }

    /// This registry's identity.
    #[must_use]
    pub fn id(&self) -> RegistryId {
        self.id
    }

    /// The root group: the built-in types and everything registered with
    /// [`TypeRegistry::register`]. It can't be removed.
    #[must_use]
    pub fn root_group(&self) -> GroupId {
        GroupId::root(self.id)
    }

    /// Registers `T`, and the types it refers to, in the root group. Registering a type that is
    /// already registered returns its key.
    ///
    /// # Errors
    ///
    /// If `T` or a type it refers to can't be registered: its stable ID is already used by
    /// another type, it breaks a persistence rule, or its definition is invalid. Nothing is
    /// registered then.
    pub fn register<T: Reflect>(&self) -> Result<TypeKey, RegistrationError> {
        if let Some(key) = self.key_of::<T>() {
            return Ok(key);
        }
        let mut inner = self.write();
        let root = self.root_group();
        inner.transaction(|inner| inner.register(TypeRef::of::<T>(), root))
    }

    /// The IDs in a crate's `stable-type-ids.txt` listing that aren't registered: what a crate's
    /// completeness test checks after calling its `register_types` (ADR-0021 §2).
    ///
    /// The listing is what `cargo xtask type-ids --write` generates: a UUID at the start of each
    /// line, then the type's name. Blank lines and lines starting with `#` are skipped.
    ///
    /// ```
    /// use oxreal_reflection::TypeRegistry;
    ///
    /// let registry = TypeRegistry::new();
    /// let listing = "# generated\nd760993b-9c97-4114-8a54-b191698217ad bool\n";
    /// assert_eq!(registry.unregistered_ids(listing), Ok(Vec::new()));
    /// ```
    ///
    /// # Errors
    ///
    /// If a line doesn't start with a well-formed UUID.
    pub fn unregistered_ids(
        &self,
        listing: &str,
    ) -> Result<Vec<StableTypeId>, ParseStableTypeIdError> {
        let inner = self.read();
        let mut missing = Vec::new();
        for line in listing.lines().map(str::trim) {
            let line = line.strip_prefix('\u{feff}').unwrap_or(line);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let id: StableTypeId = line.split_whitespace().next().unwrap_or(line).parse()?;
            if !inner.by_stable_id.contains_key(&id) {
                missing.push(id);
            }
        }
        Ok(missing)
    }

    /// Registers a type defined outside Rust (a C#, C++ or script component) in the root group,
    /// after validating its whole description (see [`ForeignTypeDefinition`]).
    ///
    /// # Errors
    ///
    /// If its stable ID is already used, or the description is invalid: a bad layout, an
    /// unregistered or non-ABI-safe field type, or a field outside the type, misaligned,
    /// overlapping another, or sharing its name. Nothing is registered then.
    pub fn register_foreign(
        &self,
        definition: ForeignTypeDefinition,
    ) -> Result<TypeKey, RegistrationError> {
        let mut inner = self.write();
        let root = self.root_group();
        inner.transaction(|inner| inner.register_foreign(&definition, root))
    }

    /// Starts a registration group: types added to it become visible together when it's
    /// committed, and are removed together (ADR-0021 §5, for plugins and hot reload).
    pub fn group(&self, name: impl Into<String>) -> GroupBuilder<'_> {
        GroupBuilder {
            registry: self,
            name: name.into(),
            types: Vec::new(),
        }
    }

    /// Removes a group's types. Their keys become tombstones and are never reused, and their
    /// stable IDs and Rust types can be registered again, as a reloaded module does.
    ///
    /// The registry can't see worlds: the caller (the hot-reload driver or plugin host) must
    /// first make sure no value of these types exists anywhere.
    ///
    /// # Errors
    ///
    /// - [`RegistrationError::RootGroup`] for [`TypeRegistry::root_group`].
    /// - [`RegistrationError::UnknownGroup`] if the group doesn't exist, was removed, or belongs
    ///   to another registry.
    /// - [`RegistrationError::GroupInUse`] if a type outside the group refers to one of its
    ///   types, for example a root type with a field whose type was first registered by the
    ///   group. Nothing is removed then.
    pub fn remove_group(&self, group: GroupId) -> Result<(), RegistrationError> {
        let mut inner = self.write();
        inner.remove_group(group)
    }

    /// The description of the type with this key, or `None` if the key was never issued by this
    /// registry or its type was removed.
    #[must_use]
    pub fn description(&self, key: TypeKey) -> Option<Arc<TypeDescription>> {
        self.read().live(key).cloned()
    }

    /// The key of `T`, if it's registered.
    #[must_use]
    pub fn key_of<T: Reflect>(&self) -> Option<TypeKey> {
        self.read().by_type_id.get(&TypeId::of::<T>()).copied()
    }

    /// The description of `T`, if it's registered.
    #[must_use]
    pub fn description_of<T: Reflect>(&self) -> Option<Arc<TypeDescription>> {
        let inner = self.read();
        let key = inner.by_type_id.get(&TypeId::of::<T>())?;
        inner.live(*key).cloned()
    }

    /// The key of the type with this stable ID, if one is registered.
    #[must_use]
    pub fn key_by_stable_id(&self, id: StableTypeId) -> Option<TypeKey> {
        self.read().by_stable_id.get(&id).copied()
    }

    /// The description of the type with this stable ID, if one is registered.
    #[must_use]
    pub fn description_by_stable_id(&self, id: StableTypeId) -> Option<Arc<TypeDescription>> {
        let inner = self.read();
        let key = inner.by_stable_id.get(&id)?;
        inner.live(*key).cloned()
    }

    /// Every registered type's description, in key order.
    #[must_use]
    pub fn descriptions(&self) -> Vec<Arc<TypeDescription>> {
        let inner = self.read();
        inner
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Live(description) => Some(Arc::clone(description)),
                Entry::Pending(_) | Entry::Removed => None,
            })
            .collect()
    }

    /// The number of registered types.
    #[must_use]
    pub fn len(&self) -> usize {
        let inner = self.read();
        inner
            .entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Live(_)))
            .count()
    }

    /// Whether no type is registered. Never true for a registry made with
    /// [`TypeRegistry::new`], which registers the built-in types.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read(&self) -> RwLockReadGuard<'_, Inner> {
        // Nothing panics while the lock is held (registration doesn't call out to other code, and
        // failures roll back), so a poisoned lock means a bug in this module.
        self.inner
            .read()
            .unwrap_or_else(|_: PoisonError<_>| panic!("{POISONED}"))
    }

    fn write(&self) -> RwLockWriteGuard<'_, Inner> {
        self.inner
            .write()
            .unwrap_or_else(|_: PoisonError<_>| panic!("{POISONED}"))
    }
}

const POISONED: &str = "the type registry's lock is poisoned: a registration panicked, which is \
                        a bug in oxreal-reflection";

impl Default for TypeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for TypeRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypeRegistry")
            .field("id", &self.id)
            .field("types", &self.len())
            .finish()
    }
}

/// Collects the types of a registration group. Returned by [`TypeRegistry::group`].
///
/// Nothing is registered until [`GroupBuilder::commit`]. Dropping the builder discards it.
#[must_use = "a group is registered only when it's committed"]
pub struct GroupBuilder<'a> {
    registry: &'a TypeRegistry,
    name: String,
    types: Vec<Pending>,
}

/// A type waiting in a [`GroupBuilder`].
#[derive(Debug)]
enum Pending {
    Rust(TypeRef),
    Foreign(ForeignTypeDefinition),
}

impl GroupBuilder<'_> {
    /// Adds `T` to the group. Committing fails if `T` is already registered in another group
    /// (for example in the root group, because something used it first): removing this group
    /// couldn't remove it then.
    ///
    /// A built-in container (`Vec<T>`, `Option<T>`, `[T; N]`) always joins its element type's
    /// group instead: `Vec<f32>` is shared and stays in the root group even when a plugin adds
    /// it, while `Vec<PluginType>` joins the plugin's group with `PluginType`.
    pub fn add<T: Reflect>(&mut self) -> &mut Self {
        self.types.push(Pending::Rust(TypeRef::of::<T>()));
        self
    }

    /// Adds a type defined outside Rust to the group (see [`TypeRegistry::register_foreign`]).
    pub fn add_foreign(&mut self, definition: ForeignTypeDefinition) -> &mut Self {
        self.types.push(Pending::Foreign(definition));
        self
    }

    /// Registers the group's types, and the types they refer to that aren't registered yet, all
    /// at once.
    ///
    /// # Errors
    ///
    /// As [`TypeRegistry::register`]. Nothing is registered then.
    ///
    /// # Panics
    ///
    /// After `u32::MAX` groups, which no process registers.
    pub fn commit(self) -> Result<GroupId, RegistrationError> {
        let mut inner = self.registry.write();
        let group = GroupId {
            registry: self.registry.id,
            index: u32::try_from(inner.groups.len())
                .expect("fewer than u32::MAX registration groups"),
        };
        inner.transaction(|inner| {
            for pending in &self.types {
                let ty = match pending {
                    Pending::Rust(ty) => ty,
                    Pending::Foreign(definition) => {
                        inner.register_foreign(definition, group)?;
                        continue;
                    }
                };
                if let Some(key) = inner.by_type_id.get(&ty.type_id())
                    && let existing = inner.summary(*key)
                    && existing.group != group
                {
                    return Err(RegistrationError::AlreadyInGroup {
                        ty: existing.name.into_owned(),
                        group: inner.groups[existing.group.index as usize].name.clone(),
                    });
                }
                inner.register(*ty, group)?;
            }
            Ok(())
        })?;
        inner.groups.push(Group {
            name: self.name,
            live: true,
        });
        Ok(group)
    }
}

impl fmt::Debug for GroupBuilder<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupBuilder")
            .field("name", &self.name)
            .field("types", &self.types)
            .finish()
    }
}

impl Inner {
    fn live(&self, key: TypeKey) -> Option<&Arc<TypeDescription>> {
        match self.entries.get(key.0 as usize)? {
            Entry::Live(description) => Some(description),
            Entry::Pending(_) | Entry::Removed => None,
        }
    }

    fn summary(&self, key: TypeKey) -> Summary {
        match &self.entries[key.0 as usize] {
            Entry::Pending(summary) => summary.clone(),
            Entry::Live(d) => Summary {
                identity: d.identity,
                name: d.name.clone(),
                layout: d.layout,
                operations: d.operations,
                abi: d.abi,
                group: d.group,
            },
            Entry::Removed => unreachable!("lookups never return removed keys"),
        }
    }

    /// Runs `register`, undoing everything it added if it fails, so a registration is all or
    /// nothing.
    fn transaction<R>(
        &mut self,
        register: impl FnOnce(&mut Self) -> Result<R, RegistrationError>,
    ) -> Result<R, RegistrationError> {
        let start = self.entries.len();
        let result = register(self);
        if result.is_err() {
            // Keys from `start` on were issued by this registration only, and never visible:
            // the write lock was held throughout. Reusing them is therefore safe.
            let added = |key: &TypeKey| key.0 as usize >= start;
            self.by_type_id.retain(|_, key| !added(key));
            self.by_stable_id.retain(|_, key| !added(key));
            self.entries.truncate(start);
        }
        result
    }

    fn register(&mut self, ty: TypeRef, group: GroupId) -> Result<TypeKey, RegistrationError> {
        if let Some(key) = self.by_type_id.get(&ty.type_id()) {
            return Ok(*key);
        }
        let definition = ty.definition();
        let invalid = |problem| RegistrationError::InvalidDefinition {
            ty: definition.name().to_owned(),
            problem,
        };
        if definition.type_id() != ty.type_id() {
            return Err(invalid(DefinitionProblem::WrongType));
        }

        // Generic arguments come first: the identity and name depend on them, and they can't
        // refer back to this type (that would be an infinitely large type).
        let mut generic_args = Vec::with_capacity(definition.generic_args().len());
        let mut id_inputs = Vec::with_capacity(definition.generic_args().len());
        let mut arg_names = Vec::with_capacity(definition.generic_args().len());
        let mut transient_arg = false;
        for arg in definition.generic_args() {
            match arg {
                GenericArgDefinition::Type(arg) => {
                    let key = self.register(*arg, group)?;
                    let summary = self.summary(key);
                    match summary.identity.stable_id() {
                        Some(id) => id_inputs.push(GenericIdInput::Type(id)),
                        None => transient_arg = true,
                    }
                    arg_names.push(summary.name.into_owned());
                    generic_args.push(GenericArgument::Type(key));
                }
                GenericArgDefinition::Const(constant) => {
                    id_inputs.push(GenericIdInput::Const(*constant));
                    arg_names.push(constant.to_string());
                    generic_args.push(GenericArgument::Const(*constant));
                }
            }
        }
        // Registering an argument may have registered this type already: registering `Vec<Node>`
        // registers `Node`, whose field registers `Vec<Node>`.
        if let Some(key) = self.by_type_id.get(&ty.type_id()) {
            return Ok(*key);
        }
        let identity = match definition.persistence() {
            Persistence::Persisted(_) if transient_arg => Identity::Transient,
            Persistence::Persisted(id) if generic_args.is_empty() => Identity::Persisted(id),
            Persistence::Persisted(id) => {
                Identity::Persisted(StableTypeId::for_instantiation(id, &id_inputs))
            }
            Persistence::Transient => Identity::Transient,
            Persistence::Rebuilt => Identity::Rebuilt,
        };
        if identity.is_transient() != ty.is_transient() || definition.is_abi() != ty.is_abi() {
            return Err(invalid(DefinitionProblem::FlagMismatch));
        }
        let name = display_name(&definition, &arg_names);
        // A built-in container belongs with its element type: `Vec<f32>` is shared infrastructure
        // in the root group even if a plugin needed it first, and `Vec<PluginType>` leaves with the
        // plugin. Any other type joins the group that registers it.
        let group = match (definition.kind().0, generic_args.first()) {
            (
                KindData::Array { .. } | KindData::List { .. } | KindData::Option { .. },
                Some(GenericArgument::Type(element)),
            ) => self.summary(*element).group,
            _ => group,
        };

        if let Some(id) = identity.stable_id()
            && let Some(existing) = self.by_stable_id.get(&id)
        {
            return Err(RegistrationError::DuplicateStableId {
                id,
                existing: self.summary(*existing).name.into_owned(),
                new: name.into_owned(),
            });
        }

        let key = TypeKey(
            u32::try_from(self.entries.len()).expect("fewer than u32::MAX registered types"),
        );
        let summary = Summary {
            identity,
            name: name.clone(),
            layout: definition.layout(),
            operations: definition.operations(),
            abi: definition.is_abi(),
            group,
        };
        self.entries.push(Entry::Pending(summary.clone()));
        self.by_type_id.insert(ty.type_id(), key);
        if let Some(id) = identity.stable_id() {
            self.by_stable_id.insert(id, key);
        }

        let (kind, access) = self.resolve_kind(&definition, &summary, group)?;
        let attributes = attributes(definition.attributes(), &summary.name, None)?;
        if definition.is_abi() {
            if definition.operations().needs_drop() {
                return Err(RegistrationError::InvalidDefinition {
                    ty: summary.name.into_owned(),
                    problem: DefinitionProblem::AbiWithDrop,
                });
            }
            self.check_abi(&summary.name, &kind, !generic_args.is_empty())?;
        }
        self.entries[key.0 as usize] = Entry::Live(Arc::new(TypeDescription {
            key,
            identity,
            name,
            generic_args,
            layout: definition.layout(),
            kind,
            attributes,
            operations: definition.operations(),
            abi: definition.is_abi(),
            group,
            access,
            type_id: Some(ty.type_id()),
        }));
        Ok(key)
    }

    /// Validates and registers a type defined outside Rust (ADR-0021 §7): plain, ABI-safe data
    /// whose every field is checked, so no foreign description can make safe code create an
    /// invalid value.
    fn register_foreign(
        &mut self,
        definition: &ForeignTypeDefinition,
        group: GroupId,
    ) -> Result<TypeKey, RegistrationError> {
        let invalid = |problem| RegistrationError::InvalidDefinition {
            ty: definition.name.clone(),
            problem,
        };
        let layout = definition.layout;
        if !layout.size().is_multiple_of(layout.align()) {
            return Err(invalid(DefinitionProblem::WrongLayout));
        }
        if layout.size() > ForeignTypeDefinition::MAX_SIZE {
            return Err(invalid(DefinitionProblem::TooLarge));
        }
        if let Some(field) = definition.fields.iter().find(|f| !is_path_name(&f.name)) {
            return Err(invalid(DefinitionProblem::InvalidFieldName(
                field.name.clone(),
            )));
        }
        let names = definition.fields.iter().map(|f| (f.name.as_str(), &[][..]));
        check_unique_names(names)
            .map_err(|name| invalid(DefinitionProblem::DuplicateField(name)))?;
        if let Some(existing) = self.by_stable_id.get(&definition.id) {
            return Err(RegistrationError::DuplicateStableId {
                id: definition.id,
                existing: self.summary(*existing).name.into_owned(),
                new: definition.name.clone(),
            });
        }

        let mut fields = Vec::with_capacity(definition.fields.len());
        let mut extents = Vec::with_capacity(definition.fields.len());
        for field in &definition.fields {
            if self.live(field.ty).is_none() {
                return Err(invalid(DefinitionProblem::UnknownFieldType(
                    field.name.clone(),
                )));
            }
            let field_type = self.summary(field.ty);
            if !field_type.abi {
                return Err(invalid(DefinitionProblem::NonAbiField(field.name.clone())));
            }
            let fits = field
                .offset
                .checked_add(field_type.layout.size())
                .is_some_and(|end| end <= layout.size());
            if !fits {
                return Err(invalid(DefinitionProblem::FieldOutOfBounds(
                    field.name.clone(),
                )));
            }
            let align = field_type.layout.align();
            if field.offset % align != 0 || align > layout.align() {
                return Err(invalid(DefinitionProblem::MisalignedField(
                    field.name.clone(),
                )));
            }
            extents.push((field.offset, field_type.layout.size(), field.name.as_str()));
            fields.push(Field {
                name: Cow::Owned(field.name.clone()),
                ty: Some(field.ty),
                offset: field.offset,
                attributes: crate::Attributes::default(),
                aliases: Vec::new(),
                transient: false,
            });
        }
        check_overlap(extents)
            .map_err(|name| invalid(DefinitionProblem::OverlappingFields(name)))?;

        let key = TypeKey(
            u32::try_from(self.entries.len()).expect("fewer than u32::MAX registered types"),
        );
        self.by_stable_id.insert(definition.id, key);
        self.entries.push(Entry::Live(Arc::new(TypeDescription {
            key,
            identity: Identity::Persisted(definition.id),
            name: Cow::Owned(definition.name.clone()),
            generic_args: Vec::new(),
            layout,
            kind: TypeKind::Struct(StructInfo {
                fields,
                tuple: false,
            }),
            attributes: crate::Attributes::default(),
            operations: Operations::plain(),
            abi: true,
            group,
            access: Access::Direct,
            type_id: None,
        })));
        Ok(key)
    }

    fn resolve_kind(
        &mut self,
        definition: &TypeDefinition,
        this: &Summary,
        group: GroupId,
    ) -> Result<(TypeKind, Access), RegistrationError> {
        let invalid = |problem| RegistrationError::InvalidDefinition {
            ty: this.name.clone().into_owned(),
            problem,
        };
        let layout = definition.layout();
        let access = match definition.kind().0 {
            KindData::UnitEnum { access, .. } => {
                // `unit_enum::<T>` is public: a hand-written definition could name another enum,
                // whose accessors would then write a `T` into this type's memory.
                if access.type_id != definition.type_id() {
                    return Err(invalid(DefinitionProblem::WrongType));
                }
                Access::Enum(access)
            }
            KindData::List { access, .. } => Access::List(access),
            KindData::Option { access, .. } => Access::Option(access),
            KindData::Math { components } => {
                Access::Math(components.iter().map(|c| (c.get, c.set)).collect())
            }
            _ => Access::Direct,
        };
        let kind = match definition.kind().0 {
            KindData::Primitive(primitive) => {
                if layout != primitive_layout(primitive) {
                    return Err(invalid(DefinitionProblem::WrongLayout));
                }
                TypeKind::Primitive(primitive)
            }
            KindData::String => TypeKind::String,
            KindData::Handle { domain } => {
                if definition.operations().needs_drop() {
                    return Err(invalid(DefinitionProblem::HandleWithDrop));
                }
                TypeKind::Handle(HandleInfo { domain })
            }
            KindData::Opaque => TypeKind::Opaque,
            KindData::Struct { fields, tuple } => {
                TypeKind::Struct(self.resolve_struct(fields, tuple, this, group)?)
            }
            KindData::UnitEnum { repr, variants, .. } => {
                TypeKind::UnitEnum(resolve_enum(repr, variants, layout, this)?)
            }
            KindData::Array { element, len } => {
                let element = self.register(element, group)?;
                let element_layout = self.summary(element).layout;
                if Some(layout.size()) != element_layout.size().checked_mul(len) {
                    return Err(invalid(DefinitionProblem::WrongLayout));
                }
                TypeKind::Array(ArrayInfo { element, len })
            }
            KindData::List { element, .. } => TypeKind::List(ListInfo {
                element: self.register(element, group)?,
            }),
            KindData::Option { some, .. } => TypeKind::Option(OptionInfo {
                some: self.register(some, group)?,
            }),
            KindData::Math { components } => {
                TypeKind::Math(self.resolve_math(components, this, group)?)
            }
        };
        Ok((kind, access))
    }

    fn resolve_struct(
        &mut self,
        fields: &[FieldDefinition],
        tuple: bool,
        this: &Summary,
        group: GroupId,
    ) -> Result<StructInfo, RegistrationError> {
        let ty = || this.name.clone().into_owned();
        let invalid = |problem| RegistrationError::InvalidDefinition { ty: ty(), problem };
        check_unique_names(fields.iter().map(|f| (f.name, f.aliases)))
            .map_err(|name| invalid(DefinitionProblem::DuplicateField(name)))?;

        let mut resolved = Vec::with_capacity(fields.len());
        // (offset, size) of every reflected field, for the overlap check.
        let mut extents = Vec::with_capacity(fields.len());
        for field in fields {
            let attributes = attributes(&field.attributes, &this.name, Some(field.name))?;
            let key = match field.ty {
                Some(field_ty) => Some(self.register(field_ty, group)?),
                None => None,
            };
            match key {
                Some(key) => {
                    let field_summary = self.summary(key);
                    let fits = field
                        .offset
                        .checked_add(field_summary.layout.size())
                        .is_some_and(|end| end <= this.layout.size());
                    if !fits {
                        return Err(invalid(DefinitionProblem::FieldOutOfBounds(
                            field.name.to_owned(),
                        )));
                    }
                    // Aligned within any aligned value of the type: its offset is a multiple of
                    // its alignment, which the type's own alignment is a multiple of. A packed
                    // type fails the second test.
                    let align = field_summary.layout.align();
                    if field.offset % align != 0 || align > this.layout.align() {
                        return Err(invalid(DefinitionProblem::MisalignedField(
                            field.name.to_owned(),
                        )));
                    }
                    extents.push((field.offset, field_summary.layout.size(), field.name));
                    if field_summary.identity.is_transient()
                        && !field.transient
                        && !this.identity.is_transient()
                    {
                        return Err(RegistrationError::TransientFieldType {
                            ty: ty(),
                            field: field.name.to_owned(),
                            field_ty: field_summary.name.into_owned(),
                        });
                    }
                    if field.transient
                        && !this.identity.is_transient()
                        && !field_summary.operations.is_defaultable()
                    {
                        return Err(RegistrationError::TransientFieldWithoutDefault {
                            ty: ty(),
                            field: field.name.to_owned(),
                            field_ty: field_summary.name.into_owned(),
                        });
                    }
                }
                None => {
                    // An opaque transient field can't get a default of its own type on load,
                    // which reflection can't construct: the containing type's default supplies
                    // it.
                    if field.transient
                        && !this.identity.is_transient()
                        && !this.operations.is_defaultable()
                    {
                        return Err(invalid(DefinitionProblem::OpaqueTransientField(
                            field.name.to_owned(),
                        )));
                    }
                }
            }
            resolved.push(Field {
                name: Cow::Borrowed(field.name),
                ty: key,
                offset: field.offset,
                attributes,
                aliases: field.aliases.iter().map(|a| Cow::Borrowed(*a)).collect(),
                transient: field.transient,
            });
        }

        check_overlap(extents)
            .map_err(|name| invalid(DefinitionProblem::OverlappingFields(name)))?;
        Ok(StructInfo {
            fields: resolved,
            tuple,
        })
    }

    fn resolve_math(
        &mut self,
        components: &[MathComponentDefinition],
        this: &Summary,
        group: GroupId,
    ) -> Result<MathInfo, RegistrationError> {
        check_unique_names(components.iter().map(|c| (c.name, &[][..]))).map_err(|name| {
            RegistrationError::InvalidDefinition {
                ty: this.name.clone().into_owned(),
                problem: DefinitionProblem::DuplicateField(name),
            }
        })?;
        let mut resolved = Vec::with_capacity(components.len());
        for component in components {
            resolved.push(MathComponent {
                name: Cow::Borrowed(component.name),
                ty: self.register(component.ty, group)?,
            });
        }
        Ok(MathInfo {
            components: resolved,
        })
    }

    /// An ABI-safe type is plain data whose every part is ABI-safe (ADR-0021 §3).
    fn check_abi(
        &self,
        name: &str,
        kind: &TypeKind,
        generic: bool,
    ) -> Result<(), RegistrationError> {
        let invalid = |problem| RegistrationError::InvalidDefinition {
            ty: name.to_owned(),
            problem,
        };
        let abi = |key: TypeKey| self.summary(key).abi;
        match kind {
            TypeKind::Primitive(primitive) => {
                if *primitive == Primitive::Bool {
                    return Err(invalid(DefinitionProblem::NotAbiSafe));
                }
            }
            TypeKind::Handle(_) => {}
            TypeKind::Array(info) => {
                if !abi(info.element) {
                    return Err(invalid(DefinitionProblem::NotAbiSafe));
                }
            }
            TypeKind::Struct(info) => {
                if generic {
                    return Err(invalid(DefinitionProblem::GenericAbi));
                }
                for field in &info.fields {
                    if !field.ty.is_some_and(abi) {
                        return Err(invalid(DefinitionProblem::NonAbiField(
                            field.name.clone().into_owned(),
                        )));
                    }
                }
            }
            TypeKind::String
            | TypeKind::UnitEnum(_)
            | TypeKind::List(_)
            | TypeKind::Option(_)
            | TypeKind::Math(_)
            | TypeKind::Opaque => return Err(invalid(DefinitionProblem::NotAbiSafe)),
        }
        Ok(())
    }

    fn remove_group(&mut self, group: GroupId) -> Result<(), RegistrationError> {
        if group.registry != self.id {
            return Err(RegistrationError::UnknownGroup { group });
        }
        if group.is_root() {
            return Err(RegistrationError::RootGroup);
        }
        let state = self
            .groups
            .get(group.index as usize)
            .filter(|g| g.live)
            .ok_or(RegistrationError::UnknownGroup { group })?;
        let members: HashSet<TypeKey> = self
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Live(d) if d.group == group => Some(d.key),
                _ => None,
            })
            .collect();
        for entry in &self.entries {
            let Entry::Live(user) = entry else { continue };
            if user.group == group {
                continue;
            }
            if let Some(used) = user.referenced_types().find(|key| members.contains(key)) {
                return Err(RegistrationError::GroupInUse {
                    group: state.name.clone(),
                    ty: self.summary(used).name.into_owned(),
                    used_by: user.name.clone().into_owned(),
                });
            }
        }
        for key in &members {
            self.entries[key.0 as usize] = Entry::Removed;
        }
        self.by_type_id.retain(|_, key| !members.contains(key));
        self.by_stable_id.retain(|_, key| !members.contains(key));
        self.groups[group.index as usize].live = false;
        Ok(())
    }
}

/// The display name of a type: its own name, plus its generic arguments.
fn display_name(definition: &TypeDefinition, args: &[String]) -> Cow<'static, str> {
    if args.is_empty() {
        return Cow::Borrowed(definition.name());
    }
    match definition.kind().0 {
        KindData::Array { len, .. } => Cow::Owned(format!("[{}; {len}]", args[0])),
        _ => Cow::Owned(format!("{}<{}>", definition.name(), args.join(", "))),
    }
}

fn primitive_layout(primitive: Primitive) -> Layout {
    match primitive {
        Primitive::Bool => Layout::new::<bool>(),
        Primitive::I8 => Layout::new::<i8>(),
        Primitive::I16 => Layout::new::<i16>(),
        Primitive::I32 => Layout::new::<i32>(),
        Primitive::I64 => Layout::new::<i64>(),
        Primitive::U8 => Layout::new::<u8>(),
        Primitive::U16 => Layout::new::<u16>(),
        Primitive::U32 => Layout::new::<u32>(),
        Primitive::U64 => Layout::new::<u64>(),
        Primitive::F32 => Layout::new::<f32>(),
        Primitive::F64 => Layout::new::<f64>(),
    }
}

/// Checks a type's or member's attributes and converts them.
fn attributes(
    definition: &AttributesDefinition,
    ty: &str,
    member: Option<&str>,
) -> Result<crate::Attributes, RegistrationError> {
    let attributes = definition.to_attributes();
    if let Some(problem) = attributes.range().and_then(|range| range.problem()) {
        return Err(RegistrationError::InvalidDefinition {
            ty: ty.to_owned(),
            problem: DefinitionProblem::InvalidRange {
                member: member.map(str::to_owned),
                problem,
            },
        });
    }
    Ok(attributes)
}

/// Whether field paths can name a field called `name`: ASCII letters, digits and underscores.
fn is_path_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Checks that no two fields' byte ranges overlap, returning the name of a field that overlaps
/// the one before it. Each extent is (offset, size, name).
fn check_overlap(mut extents: Vec<(usize, usize, &str)>) -> Result<(), String> {
    extents.sort_unstable_by_key(|&(offset, size, _)| (offset, size));
    for pair in extents.windows(2) {
        let ((offset, size, _), (next, _, name)) = (pair[0], pair[1]);
        if offset + size > next {
            return Err(name.to_owned());
        }
    }
    Ok(())
}

/// Checks that names and aliases are unique among each other, returning the first repeat.
fn check_unique_names<'a>(
    members: impl Iterator<Item = (&'a str, &'a [&'a str])>,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    for (name, aliases) in members {
        for candidate in std::iter::once(name).chain(aliases.iter().copied()) {
            if !seen.insert(candidate) {
                return Err(candidate.to_owned());
            }
        }
    }
    Ok(())
}

fn resolve_enum(
    repr: Option<IntRepr>,
    variants: &[VariantDefinition],
    layout: Layout,
    this: &Summary,
) -> Result<UnitEnumInfo, RegistrationError> {
    let invalid = |problem| RegistrationError::InvalidDefinition {
        ty: this.name.clone().into_owned(),
        problem,
    };
    if variants.is_empty() {
        return Err(invalid(DefinitionProblem::NoVariants));
    }
    check_unique_names(variants.iter().map(|v| (v.name, v.aliases)))
        .map_err(|name| invalid(DefinitionProblem::DuplicateVariant(name)))?;
    if let Some(repr) = repr {
        let expected = match repr {
            IntRepr::U8 => Layout::new::<u8>(),
            IntRepr::U16 => Layout::new::<u16>(),
            IntRepr::U32 => Layout::new::<u32>(),
        };
        if layout != expected {
            return Err(invalid(DefinitionProblem::WrongLayout));
        }
    }
    let mut discriminants = HashSet::new();
    let mut resolved = Vec::with_capacity(variants.len());
    for variant in variants {
        match (repr, variant.discriminant) {
            (Some(repr), Some(value)) => {
                if value > repr.max() || !discriminants.insert(value) {
                    return Err(invalid(DefinitionProblem::InvalidDiscriminant(
                        variant.name.to_owned(),
                    )));
                }
            }
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => {
                return Err(invalid(DefinitionProblem::InvalidDiscriminant(
                    variant.name.to_owned(),
                )));
            }
        }
        resolved.push(Variant {
            name: Cow::Borrowed(variant.name),
            discriminant: variant.discriminant,
            attributes: attributes(&variant.attributes, &this.name, Some(variant.name))?,
            aliases: variant.aliases.iter().map(|a| Cow::Borrowed(*a)).collect(),
        });
    }
    Ok(UnitEnumInfo {
        repr,
        variants: resolved,
    })
}

/// A type couldn't be registered (ADR-0021 §2 and §5).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum RegistrationError {
    /// Two different types declare the same stable ID, usually a copy-pasted
    /// `#[reflect(id = "...")]`.
    #[error(
        "stable type ID {id} is declared by both '{existing}' and '{new}'; give one of them a \
         new ID (`cargo xtask new-type-id` prints one)"
    )]
    DuplicateStableId {
        /// The shared ID.
        id: StableTypeId,
        /// The type registered first.
        existing: String,
        /// The type being registered.
        new: String,
    },
    /// A saved type has a field whose type is never saved.
    #[error(
        "'{ty}' is saved, but its field '{field}' has the transient type '{field_ty}'; mark the \
         field #[reflect(transient)], or make '{ty}' transient too"
    )]
    TransientFieldType {
        /// The containing type.
        ty: String,
        /// The field.
        field: String,
        /// The field's transient type.
        field_ty: String,
    },
    /// A transient field is reset to its type's default on load, but the type has none.
    #[error(
        "field '{field}' of '{ty}' is transient, so loading resets it to its type's default, but \
         '{field_ty}' has none; declare #[reflect(default)] on '{field_ty}'"
    )]
    TransientFieldWithoutDefault {
        /// The containing type.
        ty: String,
        /// The field.
        field: String,
        /// The field's type.
        field_ty: String,
    },
    /// A definition contradicts itself or its type. With the derive this is a bug in the derive;
    /// otherwise in a hand-written `Reflect` impl.
    #[error("invalid reflection definition of '{ty}': {problem}")]
    InvalidDefinition {
        /// The type.
        ty: String,
        /// What's wrong.
        problem: DefinitionProblem,
    },
    /// A type added to a group is already registered in another group, so removing this group
    /// couldn't remove it.
    #[error(
        "'{ty}' is already registered in group '{group}'; add it to its group before anything \
         else registers it"
    )]
    AlreadyInGroup {
        /// The type.
        ty: String,
        /// The group it's already in.
        group: String,
    },
    /// A type outside the group refers to one of the group's types.
    #[error("group '{group}' can't be removed: its type '{ty}' is still used by '{used_by}'")]
    GroupInUse {
        /// The group's name.
        group: String,
        /// The group's type that is in use.
        ty: String,
        /// The type outside the group that uses it.
        used_by: String,
    },
    /// The group doesn't exist in this registry, or was already removed.
    #[error("registration group {group:?} doesn't exist in this registry, or was already removed")]
    UnknownGroup {
        /// The group.
        group: GroupId,
    },
    /// The root group can't be removed.
    #[error("the root group holds the built-in and directly registered types and can't be removed")]
    RootGroup,
}

/// What's wrong with an invalid definition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DefinitionProblem {
    /// The definition was built for a different Rust type.
    #[error("the definition was built for a different Rust type")]
    WrongType,
    /// `Reflect::TRANSIENT` or `Reflect::ABI` disagrees with the definition.
    #[error("Reflect::TRANSIENT or Reflect::ABI disagrees with the definition")]
    FlagMismatch,
    /// The layout doesn't match the kind (a primitive's size, an array's length, an enum's
    /// representation).
    #[error("the layout doesn't match the kind")]
    WrongLayout,
    /// Two fields share a name or alias.
    #[error("two fields are named or aliased '{0}'")]
    DuplicateField(String),
    /// Two variants share a name or alias.
    #[error("two variants are named or aliased '{0}'")]
    DuplicateVariant(String),
    /// A field doesn't fit in the type.
    #[error("field '{0}' lies outside the type")]
    FieldOutOfBounds(String),
    /// A field's offset isn't a multiple of its alignment, or its type is more aligned than the
    /// containing type (a packed type).
    #[error("field '{0}' isn't aligned within the type (#[repr(packed)] types can't be reflected)")]
    MisalignedField(String),
    /// An enum has no variants, so no value of it exists.
    #[error("an enum without variants has no values to reflect")]
    NoVariants,
    /// A handle type needs dropping, but handles are plain values that are copied and remapped.
    #[error("handles are plain values: a handle type can't need dropping")]
    HandleWithDrop,
    /// An ABI-safe type needs dropping, which other languages copying it can't honor.
    #[error("ABI-safe types are plain data: an abi type can't need dropping")]
    AbiWithDrop,
    /// A field overlaps the one before it.
    #[error("field '{0}' overlaps another field")]
    OverlappingFields(String),
    /// A variant's discriminant is missing, unexpected, repeated or too large for the
    /// representation.
    #[error(
        "variant '{0}' has a missing, unexpected, repeated or out-of-range discriminant (enums \
         with an explicit repr need one per variant; enums without one have none)"
    )]
    InvalidDiscriminant(String),
    /// An opaque field is transient, but the containing type has no default to supply it on
    /// load.
    #[error(
        "the opaque field '{0}' is transient, so loading needs the type's default to supply it; \
         declare #[reflect(default)]"
    )]
    OpaqueTransientField(String),
    /// An attribute's range is invalid.
    #[error("{} has an invalid range: {problem}", member.as_ref().map_or_else(|| "the type".to_owned(), |m| format!("'{m}'")))]
    InvalidRange {
        /// The field or variant, or `None` for the type itself.
        member: Option<String>,
        /// What's wrong with it.
        problem: &'static str,
    },
    /// A foreign type is larger than [`ForeignTypeDefinition::MAX_SIZE`].
    #[error("it's larger than the 1 MiB limit for types defined outside Rust")]
    TooLarge,
    /// A foreign field's name isn't one field paths can name.
    #[error(
        "field name '{0}' must be ASCII letters, digits and underscores, so field paths can \
         name it"
    )]
    InvalidFieldName(String),
    /// A foreign type's field names a type key that isn't registered.
    #[error("the type of field '{0}' isn't registered (or was removed)")]
    UnknownFieldType(String),
    /// An ABI-safe type has a field that isn't ABI-safe (or is opaque).
    #[error(
        "it's declared #[reflect(abi)], but field '{0}' isn't ABI-safe (use integers, floats, \
         handles, fixed arrays and other abi types; u8 instead of bool, an integer instead of an \
         enum)"
    )]
    NonAbiField(String),
    /// A generic type is declared ABI-safe.
    #[error("generic types can't be #[reflect(abi)]")]
    GenericAbi,
    /// A kind that can't be ABI-safe is declared ABI-safe.
    #[error("its kind can't be ABI-safe")]
    NotAbiSafe,
}
