# ADR-0021: Reflection and the type registry

- **Status:** Accepted
- **Date:** 2026-11-21
- **Related:**
  - PRD §6, §7 and §8 (M2); brief §5 (Reflection)
  - TAD §2, §4, §5.4, §5.7 and §5.8
  - ADR-0001, ADR-0005, ADR-0006 (Proposed), ADR-0010, ADR-0011 (Decision item 7, C-ABI math)
  - ADR-0014 (§1, §3 amendment, §4, §6, §7, §9, §10 and its implementation notes)
  - ADR-0020 (§1, §4, §7)
  - Open decisions: D-02 (resolved here), D-07, D-11, D-12, D-13, D-21
  - Prior art: [Cordite](../development/prior-art.md)

## Context

Reflection underpins the Details panel, serialization, undo/redo, actor templates, visual
scripting, runtime debugging, replication and the future C ABI (brief §5, TAD §5.4). It must be
decided before M2, when the editor starts creating, editing, saving and reopening levels.

The constraints:

- **Engine-owned** (ADR-0001). `bevy_reflect` is banned in `deny.toml`. A dependency's
  reflection would become our public API.
- **Language-neutral metadata** (TAD §5.8, constraint 1): stable type IDs, field layouts,
  names, attributes and, later, function signatures, exportable to tools. Metadata must not
  exist only as Rust traits or generics.
- **Layering** (TAD §4): reflection is `foundation`, so it can't depend on `oxreal-ecs`
  (`runtime`). The ECS depends on reflection, not the reverse.
- **No global mutable state** (CLAUDE.md rule 8, conventions.md "State").
- **What ADR-0014 left for this decision:**
  - §7: dynamic queries by `ComponentId`, "using reflection metadata".
  - First slice notes: the dynamic-query slice brings **layout-erased columns** for components
    without a Rust type (C#, C++, scripts), with `unsafe`, Miri and an `unsafe-auditor`
    review, and a registry **keyed on something other than `TypeId`**. Component IDs are per
    world, in first-use order. PIE copies and C-ABI callers need either per-world resolution or
    a shared registry.
  - §3 amendment: paths that write by component ID **check the mutability flag** and change an
    immutable component only through `insert` or a by-ID `modify` that runs its hooks.
  - In-place modify notes: copy and serialize paths **skip the hierarchy's private stamp and
    `Children`**, and rebuild `Children` from `ChildOf` on load. The stamp must never be
    removed, reset or copied while its entity lives.
  - §3 amendment: the immutability check moves to a trait bound "once a `Component` derive
    exists (with reflection, D-02)". Custom bundles also wait for a derive.
  - §9: `World` stays `Sync`, so read-only systems can share `&World`.
- **One model** (ADR-0020 §1): no second component registry or object model in any binding or
  editor abstraction.
- **Cordite's lessons** (prior-art.md): its explicit per-type tables with ranges, units,
  descriptions and read-only flags worked, and parsing input into a validated patch before
  changing anything worked. Its reflection was too thin (risk R12): no methods, no lists of
  structs, no categories, no type registry. That blocked widgets, data assets and C#.

What the Rust language and our dependencies give us, verified in their docs:

- [`TypeId`](https://doc.rust-lang.org/std/any/struct.TypeId.html): "the hashes and ordering
  will vary between Rust releases". Its size and layout are unstable. It's an in-process key
  only, never a persistent one. `TypeId::of` is `const` since 1.91.
- [`type_name`](https://doc.rust-lang.org/std/any/fn.type_name.html) "is intended for
  diagnostic use", "must not be considered to be a unique identifier", and "may change between
  versions of the compiler".
- [`offset_of!`](https://doc.rust-lang.org/core/mem/macro.offset_of.html) is stable since 1.77
  for struct and nested fields. Enum variant fields need the unstable `offset_of_enum`.
- glam's SIMD types hide their components: [`Vec3A`](https://docs.rs/glam/0.34.3/glam/f32/struct.Vec3A.html)
  and [`Quat`](https://docs.rs/glam/0.34.3/glam/f32/struct.Quat.html) are declared with private
  fields, are "16 byte aligned", and reach `x`, `y`, `z` through `Deref`. `offset_of!` can't
  name those components. ADR-0011 (Decision item 7) already says the C ABI defines its own
  `#[repr(C)]` math structs instead of exposing glam's SIMD layouts.
- Built-in compile-time reflection (`#![feature(type_info)]`,
  [rust-lang/rust#146922](https://github.com/rust-lang/rust/issues/146922)) is a nightly
  experiment that still needs an RFC.

## Options considered

### 1. Who owns reflection

#### Option A: engine-owned reflection crates (chosen)
- Pros:
  - The metadata model, stable IDs, attributes and C-ABI export are designed for this engine:
    hooks, immutability, `Mut` change detection, foreign component types.
  - No third-party types in the public API. No breaking upstream churn.
- Cons, the cost of owning it:
  - Proc-macro crates (`syn`, `quote`, `proc-macro2`, all MIT OR Apache-2.0 and already in
    `Cargo.lock` as transitive dependencies, for example of `thiserror`, but new as direct
    dependencies, so they need an audit and a `dependencies.md` entry).
  - `unsafe` erased access, with a Miri job and audits (Decision §7).
  - Implementing every container, enum and attribute ourselves, and its tests. Estimated as
    several slices, comparable to the ECS (ADR-0014's notes).

#### Option B: adopt `facet` behind an engine wrapper
[`facet`](https://github.com/facet-rs/facet) is the most complete general-purpose runtime
reflection crate found. Checked 2026-11-20 and 2026-11-21:
- License MIT OR Apache-2.0. Latest stable `facet`/`facet-core`/`facet-reflect` 0.46.5
  (2026-05-26). The default branch is on `0.50.0-rc.7` (2026-08-20). First release 2025-04-06.
  crates.io lists at least 29 distinct `0.x` minor lines in its last 100 versions, each a
  breaking change under Cargo's rules.
- Its [`Shape`](https://docs.rs/facet-core/0.46.5/facet_core/struct.Shape.html) has layout,
  kinds, docs, attributes and type parameters. Its
  [`Field`](https://docs.rs/facet-core/0.46.5/facet_core/struct.Field.html) has name, byte
  offset (`offset_of!`), flags, extension attributes, defaults and docs.
- Identity is `ConstTypeId`, "a `TypeId` equivalent usable in const contexts", and `DeclId`,
  documented as "NOT stable across compilations, refactors, or reformatting". There's no
  persistent type identity.
- Its README says "Crates are in various states of progress, buyer beware!"

- Pros: a large part of the metadata model exists, with offsets and attributes.
- Cons:
  - We'd still need our own derive or attribute layer for stable IDs, editor attributes,
    mutability and persistence flags. That means two derives per type, or depending on
    facet's attribute extension staying stable.
  - Pre-1.0, with frequent breaking releases. Every bump would ripple through every reflected
    engine type.
  - Its shapes are `'static` data emitted for Rust types; it isn't designed for
    runtime-registered foreign types. C#/C++ components would need a second path, which
    ADR-0020 §1 forbids.
  - Its value access API (`facet-reflect`) would either leak into our API or be wrapped
    entirely, which leaves little reuse.

#### Option C: other crates (not viable)
- `bevy_reflect`: banned (ADR-0001). It has the closest feature set; its `TypePath` pins a
  stable *string* path with `#[type_path = "..."]`
  ([docs](https://docs.rs/bevy_reflect/latest/bevy_reflect/trait.TypePath.html)). Studied for
  ideas only.
- [`reflect`](https://github.com/dtolnay/reflect) (0.0.16): compile-time reflection *for
  writing proc macros*, a "proof of concept". Not runtime reflection.
- `type-uuid` (0.1.2, last release 2019-12): only a UUID trait. Unmaintained.

### 2. Stable type identity

- **Explicit 128-bit ID in the derive** (`#[reflect(id = "<uuid>")]`, chosen). Survives renames,
  module moves and crate moves. Language-neutral (C#/C++ declare the same value). Costs one
  generated UUID per type, and copy-paste duplicates must be caught. Fyrox does this
  (`TypeUuidProvider` with a `type_uuid` derive attribute); Cordite identifies assets and
  entities by UUIDs.
- **Hash of a qualified name** (`module_path!` + name). No annotation, but a rename or move is a
  new identity, so saved data breaks unless a redirect table maps old names, the way Unreal
  redirects renamed classes through configuration.
- **Registered string path** (`#[type_path = "oxreal.hierarchy.ChildOf"]`, Bevy's `TypePath`).
  Readable in text files and diffs. Renaming the *path* still needs redirects, and strings are
  slower to compare and to pass across the C ABI.

### 3. Component IDs: shared registry or per-world IDs

- **Per-world IDs in first-use order, with remapping (today's code).** No change now. Every
  world copy (PIE), every tool and every C-ABI caller must translate IDs per world, and every
  translation is a place to get it wrong.
- **One registry shared by the worlds of an application (chosen).** Edit and PIE worlds and
  the C ABI agree on every `ComponentId`. Costs a lock on registration, a per-world cache so
  hot paths don't lock, and an identity check so values never cross registries.

### 4. Must every component be reflected?

- **`Component: Reflect`, required (chosen).**
  - Every component can be edited, saved and seen by other languages: one model (ADR-0020 §1).
    Layout-erased storage gets layout and drop from the same description for Rust and foreign
    components.
  - Costs: the workspace has **74 hand-written `impl Component`** today, mostly in tests,
    benches and doc examples. Each needs `#[derive(Reflect, Component)]` plus an ID or
    `transient`. A hand-written impl stays possible but needs `unsafe impl Reflect` and the
    mutability associated type. The orphan rule means a component with a third-party field type
    needs an impl in `oxreal-reflection` or a field-level `#[reflect(opaque)]`.
- **Reflection opt-in per component.**
  - Cheaper today: existing impls and tests stay as they are.
  - Costs: unreflected components are invisible to the editor, saving and other languages.
    Every by-ID path, PIE copy and export needs a second case for them, and storage still needs
    their layout and drop from a separate source: two kinds of component, against ADR-0020
    §1's one-model intent.

### 5. Registration

Crate versions checked on crates.io on 2026-11-20.

- **Explicit (chosen):** each crate exposes a registration function, and the application calls
  it. Visible, ordered, works the same for Rust, foreign types, plugins and hot reload. Costs
  boilerplate, and a type left out is missing.
- **Life-before-main collection** ([`inventory`](https://github.com/dtolnay/inventory) 0.3.25,
  [`ctor`](https://github.com/mmastrac/linktime) 1.0.14, whose crates.io repository is the
  `linktime` workspace): registers on startup "as part of life-before-main", or when a dynamic
  library is loaded. Hidden process-wide state, filled before `main`. There's a history of
  registrations silently missing when a crate isn't otherwise referenced
  ([rust-lang/rust#47384](https://github.com/rust-lang/rust/issues/47384), linking
  dtolnay/inventory#7 and #9). It's closed as fixed, but we haven't verified it on our targets
  and linkers.
- **Link-section collection** ([`linkme`](https://github.com/dtolnay/linkme) 0.3.38): an
  immutable `static` slice built by the linker, with "no life-before-main or any other runtime
  initialization", supported on Windows and Linux. Not mutable state, but still
  linker-dependent, and it doesn't cover foreign or runtime-loaded types.

## Decision

### 1. Engine-owned reflection (option A)
- `crates/foundation/reflection` (`oxreal-reflection`): the metadata model, type keys and
  stable type IDs, the type registry, erased value access, field paths and patches.
- `crates/foundation/reflection-derive` (`oxreal-reflection-derive`): `#[derive(Reflect)]`.
- `crates/runtime/ecs-derive` (`oxreal-ecs-derive`, runtime layer): `#[derive(Component)]`,
  re-exported by `oxreal-ecs`. It emits only the `Component` impl (hooks, the mutability
  associated type, component flags). `Reflect` comes from `#[derive(Reflect)]` beside it.
  `#[derive(Bundle)]` comes later: it needs a hidden path through the sealed `Bundle` trait.
- `oxreal-reflection` depends on `oxreal-math`, so it can describe the glam types (orphan rule)
  and `Transform`. So `oxreal-math` must never depend on `oxreal-reflection`.
- No third-party types in its public API. `facet` isn't adopted.

### 2. Identity: in-process `TypeKey`, persistent `StableTypeId`
- **`TypeKey`** is a type's index in one `TypeRegistry`: the in-process identity. Descriptions
  refer to other types only by `TypeKey`, so field descriptions store their field type's key.
  For Rust types, the registry maps `TypeId` to the `TypeKey`. Foreign types have no `TypeId`.
  Nothing persists a `TypeKey` or a `TypeId`.
- **`StableTypeId`** (128 bits) is only the persistent and exported identity: files, overrides,
  migrations and the C ABI's persistent forms.
- **Every reflected type either carries an explicit ID or is declared transient.**
  `#[reflect(id = "6f1c…")]` is written as a UUID string. `#[reflect(transient)]` means "never
  persisted", and needs no ID (§3 has the full flag set). Choosing neither, or giving a
  malformed ID, is a build error whose message says how to generate one (a `cargo xtask`
  helper; its dependency gets audited when it's added).
- **A transient type can't hide inside a persisted one.** The derive rejects a field whose type
  is transient in a non-transient type, unless the field itself is `#[reflect(transient)]`.
- **Renames and moves keep the ID.** Nothing to migrate. Changing a type's ID is a breaking data
  change that needs a D-07 migration.
- **Retired IDs are never reused.** A checked-in list of retired IDs is part of the scan below,
  which rejects any of them appearing in code.
- **Display name:** every type has one, for the editor and diagnostics. By default it's
  `module_path!` plus the identifier, so it changes when the type moves. It's stable only when
  set with `#[reflect(name = "...")]`. The ID always wins.
- **Built-in types** (primitives, `String`, the math types, `Entity`, `AssetId`) have IDs fixed
  in `oxreal-reflection` or in the crate that owns them.
- **Generic instantiations** get an ID derived from their inputs:
  - The inputs are the generic definition's ID and every argument in order: each type
    argument's ID, and each const generic argument as its type's ID plus its value.
  - The function is **SHA-256** over a domain-separated encoding (a fixed version tag, then
    each input with a kind tag, IDs as their 16 big-endian bytes, const values in a fixed-width
    big-endian form), truncated to the first 128 bits. `sha2` is already an audited workspace
    dependency (`dependencies.md`, 0.11.1). Its "Used by" entry gains `oxreal-reflection`,
    which ships in players.
  - The encoding is fixed in slice 1, with checked-in test vectors, and never changes
    afterwards: a change would change every derived ID.
  - If any argument is transient, the instantiation is transient and gets no ID.
- **Collisions are errors.** Registering two different types with one ID fails, naming both. A
  `cargo xtask` scan reads every `#[reflect(id = ...)]` literal in the workspace and rejects
  duplicates and retired IDs, so a copy-pasted ID is caught even if the two types are never
  registered together.
- **Registration completeness:** the same scan lists each crate's ID literals, and a test per
  crate checks that its `register_types` registers every one of them. The scan skips test,
  benchmark and example code; test fixtures that are never registered use `transient`.
- **C-ABI form of 128-bit IDs** (`StableTypeId`, `AssetId`): `[u8; 16]` in big-endian byte
  order, which is RFC 9562's "network byte order" and the order of the UUID text, never a
  `u128` (no portable C type, and alignment differs between compilers). RFC 9562 notes that
  COM GUIDs store their first fields little-endian, so C# and C++ bindings convert explicitly
  rather than reinterpret bytes.

### 3. The metadata model
The registry holds **plain data**: no Rust generics or traits in the stored descriptions, so it
can be exported (to a file, a header generator, a C-ABI query) unchanged.
- For non-generic Rust types, the derive emits the description as an associated `const`. Its
  fields name their types through registration functions, which the registry resolves to
  `TypeKey`s when the type is registered.
- Descriptions of generic instantiations are built at registration.
- Foreign types are described at run time, through the same constructors, and validated (§7).

**A type description** has: its `TypeKey`, its `StableTypeId` (or none, when transient), its
display name, size and alignment, kind, attributes, operations and flags.
- **Operations:**
  - **drop** always;
  - **clone** only with `#[reflect(clone)]`, and **default construction** only with
    `#[reflect(default)]`. The derive checks them against the real `Clone` and `Default`
    impls. Nothing synthesizes them field by field.
  - A copy path (PIE, clipboard, template instantiation) clones a value when its type has
    `clone`. Otherwise it copies the value through its persisted form (save, then load), which
    every type with a `StableTypeId` has. Only a transient type without `clone` can't be
    copied: the copy reports a "not cloneable" error naming the type. So PIE never fails on an
    ordinary persisted component that lacks `#[reflect(clone)]`.
- **Kinds:**
  - **primitive:** `bool`, signed and unsigned integers to 64 bits, `f32`, `f64`;
  - **string;**
  - **struct**, with named or positional fields;
  - **enum:**
    - unit-only enums with an explicit `#[repr(u8/u16/u32)]` carry their discriminants;
    - unit-only enums without an explicit repr are allowed too, and saved and matched by
      variant name. `OnParentDespawn` (a field of `ChildOf`) migrates to this in slice 2;
    - data-carrying enums carry variants with fields;
  - **fixed array**, **list** (`Vec<T>`), **map** (ordered and hashed), **option;**
  - **handle:** a plain value that refers to something and must be remapped when copied or
    saved: `Entity` (to stable object IDs, D-07) and `AssetId`. The handle's domain is a
    `StableTypeId`, so reflection needs no dependency on the ECS;
  - **math leaf:** the `oxreal-math` types (`Vec3`, `Vec3A`, `Vec4`, `Quat`, `Mat4`,
    `Affine3A`, `Transform` and so on). Fixed layout, components reached through accessor
    functions, not offsets, because glam's SIMD types hide their fields. They aren't
    `abi`-safe: the C ABI uses its own `#[repr(C)]` math structs (ADR-0011, Decision item 7);
  - **opaque:** identity, layout and operations only. Not editable, and saved only through a
    codec that D-07 defines.
- **Fields** have a name, the field type's `TypeKey`, a byte offset (from `offset_of!`, for the
  current build), attributes and aliases (old names, for renamed fields in paths and files).
  - Rust's default layout is fixed within one build but unspecified across builds, so offsets
    are build-local data, never saved.
  - Data-carrying enum fields have no stable `offset_of!`. Their variant fields are reached
    through accessor functions that the derive generates (read the variant, project to a
    field), not offsets.
  - **`#[reflect(opaque)]` on a field:** the field's type needs no `Reflect`. The field isn't
    navigable or editable. Unless D-07 provides a codec for that field, saving the containing
    type returns an error naming the type and the field; mark it `transient` if it's never
    meant to be saved. It can still be copied in process if the type has `clone`.
  - **Enums and containers** are reached through accessor functions too, not offsets: unit
    enums without a `repr` are read and written by variant (the same derive-generated
    accessors as data-carrying enums), and `String`, `Vec`, `Option` and maps through
    operations specific to their kind.
- **The C ABI contract:**
  - A type marked `#[reflect(abi)]` is exported as a plain C struct in generated headers. The
    derive requires `#[repr(C)]` (or `#[repr(transparent)]`) and only ABI-safe field types:
    integers, floats, handles, fixed arrays and other `abi` types. **Generic types can't be
    `abi`.**
  - **No `bool` and no Rust enums in `abi` types or foreign-defined components.** They use
    `u8` or `u32` fields, with range-checked conversion where Rust code wants a `bool` or an
    enum. Rust code reads such fields of foreign components as integers.
  - Every other reflected type is reachable from other languages **by handle and field path**
    through the C ABI's accessors, not as a struct (D-21 designs those accessors).
  - **Foreign-defined component types** (C#, C++, scripts) are restricted to `abi`-style plain
    data until engine-owned string and list types exist for them.
- **Attributes** are a fixed, typed set, each with a documented meaning:
  - `display_name`, `category`, `tooltip` (defaulting to the first doc-comment line);
  - `range` (min, max, step for the UI), `units` (for example "degrees in the UI, radians
    stored", per ADR-0011);
  - `hidden`, `read_only`.

  Plus **custom attributes**: a name with a bool, integer, float or string value, for tools and
  plugins. Attributes never contain code.
- **Persistence and copy flags:**
  - **`transient`** (type or field): never persisted, so no `StableTypeId` is needed. A
    transient field gets its default on load, so its type must have `default`. In-process
    copies (PIE, templates) still clone every component that has `clone`, transient or not.
  - **`rebuilt`** (type): its owner rebuilds it, so copies and saves skip it. `rebuilt`
    implies `transient`. The hierarchy's `Children`, `ChildrenStamp` and `WorldTransform` are
    `rebuilt`. `Children` is rebuilt from `ChildOf` links on load and after a copy
    (ADR-0014, in-place modify notes).
- **Functions and methods are not in the first slices.** The description reserves a function
  table. When D-12 (graph nodes) or D-21 needs it, signatures are added as plain data (name,
  parameter names and type keys, return type key, whether it mutates), through a later
  amendment.
- **Generics:** a generic type may derive `Reflect` if all its type parameters are reflected.
  Each instantiation is a separate registered type (§2's derived ID). **No lifetime
  parameters** (`'static` only).

### 4. Value access and the ECS
- **`Reflect` is an `unsafe trait`.** Implementing it promises that the description matches the
  type's real layout and operations. The derive is the normal implementer and is safe to use.
  A hand-written impl takes on that proof obligation (conventions.md: a property that can't be
  checked cheaply makes its constructor `unsafe`).
- **Erased access** goes through borrowed, typed-at-run-time views: a read view and a write
  view (a description plus a pointer, tied to a lifetime).
  - Safe code builds them only from `&T` / `&mut T` of a `Reflect` type.
  - Engine code that holds raw storage builds them through a `pub unsafe fn` (pointer, type
    key, registry) with a `# Safety` contract. In `oxreal-ecs`, its only callers are in the
    storage module.
- **Field paths** have segments for field, list index, map key and enum variant. The persistent
  form uses names (for ADR-0006's overrides and D-07). The runtime form uses resolved indices.
  Leaf reads and writes are checked against the leaf's `TypeKey`. A mismatch is an error, not a
  panic, because paths and values come from files, the editor and the C ABI.
- **Value validity.** Every path that builds a value from bytes or foreign data validates it per
  kind before any Rust code sees it: owned values, leaf writes, by-ID inserts and patches.
  - `bool` must be 0 or 1; an enum discriminant must name a variant; future kinds add their
    rules (`NonZero*` non-zero, `char` a valid scalar value).
  - Integer and float leaves accept any bit pattern.
  - Writes from typed Rust values are valid by construction and skip the check.
- **Owned values and patches.** An owned erased value (a buffer laid out by its description,
  dropped through it) holds copies for undo and clipboard. **A patch** is a list of (field path,
  new value), **validated against the description before anything changes** and then applied as
  a whole (Cordite's validated choice). Undo stores the inverse patch.
- **Writes to components go through `oxreal-ecs`, never past it.** It provides the by-ID
  operations; reflection supplies only the views:
  - by-ID `get`, `insert`, `remove`, and dynamic queries (ADR-0014 §7);
  - **by-ID `modify`**, which runs `on_replace` and `on_insert` (`HookCause::Modify`) around the
    edit, exactly like `World::modify`;
  - a by-ID mutable view of a **mutable** component, which stamps the changed tick when it's
    written through, the erased equivalent of `Mut<T>` (ADR-0014 §6).
  - **Immutable components** are edited only through by-ID `modify` or `insert`. The registry's
    mutability flag is checked at run time, and a mutable view of an immutable component is
    refused with an error (ADR-0014, §3 amendment).
  - **`owner_only` components** (a flag the owning crate sets): by-ID insert, remove, `modify`
    and mutable views refuse them with an error. Reads are allowed. `Children` and
    `ChildrenStamp` are `owner_only`, so no data-driven path can remove or reset the stamp.
  - By-ID paths **return errors** for every caller mistake (unknown ID, wrong type, aliasing,
    refused flag), because they serve data-driven callers and FFI. Typed Rust paths keep their
    panics.
- **Dependency direction:** `oxreal-ecs` depends on `oxreal-reflection`. The ECS registers
  `Entity` as a handle type and owns `ComponentId`, the component registry, hooks, mutability
  and component flags. Reflection knows nothing about worlds.
- **Every component is reflected** (option 4): `Component` gains `Reflect` as a supertrait.
  - **Immutability becomes a trait bound:** `Component` gets a mutability associated type
    (mutable or immutable marker), which `#[derive(Component)]` supplies. The write paths bound
    on it, so the error is at the call site and visible to `cargo check` and rust-analyzer
    (settling ADR-0014's §3 amendment note). Hand-written `impl Component` stays possible; it
    names the associated type itself.
  - Layout-erased columns use the description's layout and drop, and its clone when copying,
    so Rust and foreign components share one storage path.
  - Resources are reflected when they need editor, serialization or C-ABI access. A
    `Resource: Reflect` bound isn't added now.

### 5. Registries: owned, shared, explicitly registered
- **`TypeRegistry`** (reflection) and **`ComponentRegistry`** (ECS) are ordinary values that the
  application creates and shares (`Arc`), never globals. Each keeps its data behind an internal
  `RwLock`, and its methods take `&self`.
  - `ComponentRegistry` holds an `Arc<TypeRegistry>`.
  - **Lock order: the component registry, then the type registry.** Code never takes them in
    the other order. This is documented on both types.
- **Explicit registration:** each engine crate with reflected types exposes
  `pub fn register_types(registry: &TypeRegistry)`. Applications and tools call the ones they
  link. Registering a type registers its field types.
- **Spawning registers the component** into the `ComponentRegistry` only, from `T`'s
  derive-emitted description. That registers `T`'s type into the `TypeRegistry`, under the lock
  order above. A forgotten `register_types` call therefore never breaks the runtime. What it can
  break is discovery: the editor's "Add Component" list and loading a file that names the type.
  Loading reports an unknown `StableTypeId` as an error naming the ID and the file.
- **No `inventory`, `ctor` or `linkme`.** Life-before-main registration is hidden process-wide
  state (rule 8) and has a history of silently missing registrations. `linkme` can't cover
  foreign or runtime-loaded types, so it would be a second mechanism.
- **Component IDs come from one shared registry** (option 3, settling ADR-0014's question).
  - `ComponentId` stays a dense `u32` used by archetype bitsets. It's **runtime-only and never
    saved**. Nothing may depend on its numeric order.
  - Worlds are created with a `ComponentRegistry`. `World::new()` still creates a private one,
    so existing code and tests don't change. Edit worlds, PIE worlds and the C ABI therefore
    agree on every `ComponentId`, and a C-ABI caller resolves `StableTypeId` to `ComponentId`
    once per registry.
  - **Per-world cache:** each world caches the registry entries it uses, so hot paths don't
    lock. The cache fills only under `&mut World`. A lookup through `&World` that misses the
    cache means the component is absent from that world, which is correct, because no entity
    in it has ever had that component. This keeps `World: Sync` (ADR-0014 §9).
  - **Registries have an internal identity**, from a process-wide counter like ADR-0014's
    `WorldId`. By-ID entry points and PIE copies check it. Copying between worlds with
    different registries is an error. A `Schedule` or query state resolved against one
    registry re-resolves when it runs against a world with another.
  - **Removal leaves a tombstone.** A removed `ComponentId` or `TypeKey` is never reused.
- **Plugins (D-13) and hot reload (D-11) use an explicit group transaction:** the types
  registered in a group become visible together when it commits, and are removed together.
  A group is removed only when no world holds a value of its types. The registry can't see
  worlds, so the caller that owns them (the hot-reload driver, D-11, or the plugin host, D-13)
  checks this before removing it. Reloading a module runs in
  this order: save the affected values, despawn them, remove the old group, register the new
  one, load the values. The new registration is therefore not a collision, and changed layouts
  migrate through the D-07 path. Code never swaps layouts in place under live values.

### 6. Seams to serialization, the editor and scripting
- **Serialization (D-07)** is written against reflection: it walks views and builds owned
  values and patches. D-07 still chooses the formats, versioning and migrations.
  - **serde policy:** engine types don't derive `serde::Serialize`/`Deserialize` as their
    persistence. That would be a second field list without IDs, attributes or persistence
    flags. D-07 may use serde-based format crates *inside* the serialization crate, driven by
    reflection. Tools keep using untyped `serde_json::Value` (ADR-0019 §4).
- **Details panel and undo/redo:** the panel reads descriptions and attributes. Edits become
  editor commands carrying patches (TAD §5.7), applied through the ECS by-ID operations, so
  hooks and change ticks behave as they would for gameplay code.
- **Actor templates and overrides:** overrides are keyed by (`StableTypeId`, persistent field
  path), reusing ADR-0006's model.
- **Visual scripting (D-12):** pins are typed by type (stable ID in assets). Get and set nodes
  come from field metadata; call nodes wait for the function table (§3).
- **Replication:** per-field replication attributes are added when networking is designed.
  Stable IDs and the persistence flags are already enough to build a schema hash from.

### 7. Unsafe
- **The full scope:**
  - `oxreal-reflection`'s erased pointer module: offset arithmetic, typed leaf reads and
    writes, drop and clone through descriptions, and the `pub unsafe fn` that builds a view
    from a raw pointer;
  - its hand-written `unsafe impl Reflect` for primitives, `String`, `Vec`, `Option`, maps and
    the math types, and `oxreal-ecs`'s for `Entity`;
  - the `unsafe impl Reflect` that `#[derive(Reflect)]` generates;
  - `oxreal-ecs`'s storage module: layout-erased columns (ADR-0014 §2 and §10).
    `oxreal-ecs` swaps `#![forbid(unsafe_code)]` (which no module can override) for a
    crate-wide deny with two scoped, reasoned exceptions: that module and the `Entity` impl;
  - later, the C-ABI layer (D-21).
- **How it's confined:** safe public APIs, a `// SAFETY:` comment per block, and `unsafe trait
  Reflect` for the one obligation callers can't check. The derive rejects `#[repr(packed)]`
  types, whose fields can't be referenced safely.
- **Crates that only use the derive keep `#![forbid(unsafe_code)]`.** A derive-generated
  `unsafe impl` compiles in such a crate (checked on rustc 1.100 with a scratch proc-macro),
  but an `#[allow(unsafe_code)]` the derive emitted would fail there (E0453), so the derive
  never emits one. `oxreal-hierarchy`, `oxreal-input` and other component crates stay
  `forbid`.
- **Foreign descriptions are validated at registration:** every field within the type's size,
  correctly aligned, non-overlapping, and only ABI-safe field kinds. With that and per-kind
  value validation (§4), safe APIs aren't meant to let foreign data create an invalid Rust
  value. That's a property the tests and Miri check, not a proof.
- **Checks:** a **Miri** CI job on a pinned nightly (ADR-0014 §10) lands **with slice 1**, in
  the same PR as the first `unsafe` block, and covers the reflection and ECS storage tests,
  validity tests included. The `unsafe-auditor` agent reviews each such PR before it's
  reported ready.

### 8. Delivery
1. **Slice 1, `oxreal-reflection` and its derive, before M2 editor work.**
   - `TypeKey`, `StableTypeId` with the generic-ID function and its test vectors,
     `TypeRegistry` with groups.
   - Descriptions for primitives, `String`, `Vec`, `Option`, fixed arrays, structs, tuple
     structs and unit enums, and the math leaves.
   - The attributes and flags above, views, field paths, patches, owned values and value
     validation.
   - The `cargo xtask` ID scan (duplicates, retired IDs, completeness) and the Miri job.
2. **Slice 2, ECS integration** (ADR-0014's dynamic-query slice).
   - `oxreal-ecs-derive` with `#[derive(Component)]`, `Component: Reflect`, the
     associated-type immutability bound.
   - The shared `ComponentRegistry`, layout-erased columns, by-ID get, insert, remove and
     `modify`, `owner_only`, dynamic queries.
   - Hierarchy types marked `rebuilt` and `owner_only`, `OnParentDespawn` reflected, the
     existing components migrated.
3. **Slice 3, with the M2 Details panel:** data-carrying enums, maps, custom attributes, and a
   description export (a plain-data file) that a tool can read.
   - Offsets in it are marked build- and target-specific, except in `abi` types.
   - Transient types appear with their `TypeKey` and display name only, no `StableTypeId`,
     marked not loadable.
4. **Later:** `#[derive(Bundle)]`, the function table (D-12, D-21), foreign type registration
   through the C ABI (D-21), replication attributes.

**Testing:**
- Offsets and sizes in descriptions equal `offset_of!` and `size_of` for every derived test
  type.
- Randomized tests: reading and writing random field paths through views agrees with typed
  access; patches apply all-or-nothing; drop accounting through owned values.
- **Validity tests:** a `bool` byte other than 0 or 1, an out-of-range discriminant and a
  malformed foreign description are each refused, through owned values, leaf writes, by-ID
  inserts and patches.
- `compile_fail` doc tests for derive errors: missing ID, a transient field type in a
  persisted type, `abi` without `#[repr(C)]`, `abi` with a `bool` field, `#[repr(packed)]`,
  `clone` without `Clone`, writing an immutable component.
- Registry tests: collisions, retired IDs, unknown IDs, tombstones, lock order under
  concurrent registration, copying between registries refused, generic-ID test vectors.
- ECS tests: by-ID writes run the same hooks and change ticks as typed writes (compared in one
  test); **removing `ChildrenStamp` by ID is refused** (a regression test for the `owner_only`
  rule); a PIE-style copy rebuilds `Children` and skips `rebuilt` components.
- Miri over all of the above.
- Benchmarks: by-ID `get` and dynamic iteration against typed queries, recorded with
  hardware details (PRD §9).

## Consequences

- **Positive:**
  - One description per type serves the editor, serialization, templates, scripting,
    replication and the C ABI, for Rust and foreign types alike (ADR-0020 §1).
  - Saved data survives renames and moves, and stable IDs are plain bytes any language can
    hold.
  - Settles ADR-0014's open items: the non-`TypeId` registry key, shared component IDs, the
    by-ID write rules, the derive for the immutability bound, and how copy and save paths
    treat the hierarchy's stamp and `Children`.
- **Negative / costs:**
  - Every persisted type needs a UUID attribute, which is noise in code and in reviews.
  - A forgotten `register_types` call shows up as a missing editor entry or a load error,
    unless the completeness test catches it first.
  - Owning reflection is a large, `unsafe`-bearing subsystem with a Miri job and audit load.
  - `Component: Reflect` changes all 74 existing `impl Component` (a mechanical migration in
    slice 2) and adds derive compile time.
  - Registration takes locks, and the per-world cache adds a little bookkeeping.
- **Constraints this places on open decisions:**
  - **D-07:** files identify types by `StableTypeId` and fields by persistent paths. Entities
    are written as stable object IDs (handles are remapped). Transient and rebuilt data is
    never written. Migrations are keyed by `StableTypeId`. Codecs for opaque types and fields
    are D-07's.
  - **D-11:** reloaded modules use group transactions, in the save, despawn, remove, register,
    load order. Layout changes migrate values through the D-07 path.
  - **D-13 and D-21:** the C ABI exports `StableTypeId`s (as big-endian `[u8; 16]`),
    descriptions and `abi` structs from the registry, uses its own math structs, and resolves
    `ComponentId`s once per registry. Foreign components use the same registry and storage,
    with integer fields in place of `bool` and enums.
  - **D-12:** pins and node signatures use stable IDs and the future function table.
- **Follow-up work:**
  - Audit `syn`, `quote` and `proc-macro2` as direct dependencies before slice 1, and add
    `oxreal-reflection` to `sha2`'s "Used by" entry.
  - TAD §5.4 and §7, and `open-decisions.md` (D-02), updated with this ADR.
  - The world-tag question from ADR-0014's first slice (handles carry no world identity) is
    still open. It becomes pressing once the editor holds edit and PIE worlds sharing one
    registry.
- **Revisit triggers:**
  - std's `type_info` (rust-lang/rust#146922) stabilizes and could generate descriptions
    without a derive.
  - Missing registrations cause repeated bugs despite the completeness test. Then reconsider
    link-section collection (`linkme`) for Rust types only.
  - By-ID access, dynamic queries or registry locking are a measured bottleneck for the Details
    panel, serialization or the C ABI.
  - Duplicate or lost IDs become a real problem in practice despite the xtask scan.
  - D-21's design needs a layout contract that `#[reflect(abi)]` can't express.

## Implementation notes (2026-11-21, slice 1, first part)

Slice 1 lands in two pull requests, so each stays reviewable. This first part brings the crates,
identity, the metadata model, the registry, the derive, erased drop, clone and default, the ID
scan and the Miri job. The second part brings erased views, field paths, patches, owned values,
value validation, the accessors of enums, containers and math types, and foreign (run-time)
type registration with its validation.

- **Definitions and descriptions.** `Reflect` provides a `TypeDefinition`: `Copy`, `'static`
  data with no destructor, built with `const fn`s, naming other types through `TypeRef`s (a
  `TypeId` and a function returning that type's definition). Registering it produces a
  `TypeDescription` that names types by `TypeKey`. The two forms are separate types because a
  constant can't hold owned data that foreign types will need, and a description can't hold
  keys before registration.
- **`Reflect` has two more constants, `TRANSIENT` and `ABI`,** besides `DEFINITION`. The derive's
  compile-time checks read them instead of field types' definitions, because evaluating
  definitions eagerly makes recursive types (`struct Node { children: Vec<Node> }`) a
  const-evaluation cycle. Registration refuses a definition that disagrees with them, and one
  built for a different Rust type.
- **The derive's checks are `.ensure(condition, message)` calls on the definition builder,**
  failing the build with the message when the constant is evaluated. They're expressions,
  because a statement in the constant's block stops its `&[...]` field lists from being promoted
  to `'static`. A non-generic type's definition is evaluated by a generated `const _`, so its
  checks fail the build even if nothing registers it; a generic type's are checked per
  instantiation. For generic types, `clone` and `default` add `Self: Clone` and
  `Self: Default` bounds to the impl.
- **Checked at registration, not at compile time:**
  - that a transient field's type is defaultable, unless the containing type is transient too
    (reading that needs the field type's definition);
  - the layout of primitives, arrays and `repr` enums;
  - field bounds, overlap and alignment. No field type may be more aligned than the containing
    type, so a hand-written impl of a `#[repr(packed)]` type is refused even if its offsets are
    honest;
  - duplicate field and variant names, discriminants, enums with no variants, and ranges;
  - the `abi` rules, including no `Drop`, and no `Drop` for handle types.

  An opaque transient field in a saved type needs the containing type to be `default`, since
  reflection can't construct the field.
- **`Reflect: Send + Sync + 'static`.** Descriptions and their erased operations are shared
  between threads, and layout-erased columns (slice 2) have only a description, no
  `T: Component` bound, so the thread-safety requirement sits on `Reflect` itself, matching
  `Component`.
- **The `Reflect` safety contract** names what registration checks and what stays the
  implementer's proof:
  - every field and every variant is listed (opaque if not reflected);
  - `abi` types are `#[repr(C)]` or `#[repr(transparent)]`, with padding bytes that readers
    copy but never interpret;
  - handles are plain values for which every bit pattern is valid.

  It also says that deriving exposes the fields: a type whose own `unsafe` code relies on
  invariants between fields marks them opaque. `oxreal-reflection` denies `unsafe_code`
  crate-wide, with a reasoned exception on each module that needs it. The derive crate forbids
  it.
- **Erased operations run through the description:** `TypeDescription::drop_value`,
  `clone_value` and `default_value` (`unsafe fn`s with the full pointer contract). Their
  `OperationError` names the type ("'Node' can't be cloned"), as §3 asks.
- **Built-in types:**
  - Short display names (`f32`, `String`, `Vec<f32>`, `[f32; 4]`).
  - Array lengths are const arguments encoded as `u64`, because `usize` isn't a reflected type.
    Arrays are ABI-safe when their element is.
  - `Transform` is a math type (§3 lists it) with the components `translation`, `rotation` and
    `scale`, so the editor edits it as one value, rotation shown in degrees.
  - `LinearRgba` is a struct with four `f32` fields, which the editor edits channel by channel.
  - `Vec`, `Option` and arrays clone and default element by element through the element's
    operations, which is what their std impls do. User types never get synthesized operations.
- **Units are an enum** (`Unit`), written `units = MetersPerSecond`, so the editor can convert
  angles without parsing strings. A `range` is in the stored units (radians for angles); the
  editor converts it the same way it converts the value. Custom attributes stay in slice 3.
- **Groups:**
  - A group can't be removed while a type outside it refers to one of its types. Removal names
    both types.
  - A built-in container (`Vec`, `Option`, array) belongs to its element type's group, not to
    the group that happened to register it first. So `Vec<f32>` is root's even if a plugin
    needed it first, and `Vec<PluginType>` leaves with the plugin.
  - A type added to a group that is already registered elsewhere is refused
    (`AlreadyInGroup`): removing the group couldn't remove it.
  - Group IDs carry their registry's identity, and another registry refuses them.
- **`register_types` returns `Result<(), RegistrationError>`,** a refinement of §5: two crates
  can declare the same ID, and the application must see that as an error rather than a panic.
  No crate has one yet; the first come with slice 2. Slice 2 also defines what spawning does
  when registering a component fails.
- **The generic-ID encoding** (§2), fixed here for every language:
  - **Input:** the ASCII tag `oxreal.reflection.generic-type-id.v1`, then the definition's 16
    ID bytes, then the argument count as a big-endian `u32`.
  - **Each type argument:** the byte `0x01` and its 16 ID bytes.
  - **Each const argument:** the byte `0x02`, its type's 16 ID bytes, and its value as a
    16-byte big-endian two's-complement integer (`bool` is 0 or 1).
  - `usize` arguments use `u64`'s ID.
  - **Output:** the first 16 bytes of SHA-256 over the input.

  Test vectors, computed independently with .NET's SHA-256 (`Vec`, `Option` and the array
  definition use the built-in IDs in `oxreal-reflection`'s `id.rs`):

  | Instantiation | Derived ID |
  |---|---|
  | `Vec<f32>` | `4fdf80ff-0bbd-6cde-61d4-18b31a945d77` |
  | `Option<String>` | `e86c420d-80de-4bcd-a4ff-4e38e53a4063` |
  | `[f32; 4]` | `f8c37ab2-39b9-653f-585b-3198a76711c8` |
  | `Vec<u8>` | `d74428bc-dbb8-7ad8-7bd2-6f0ea70fecdb` |
  | `Vec<Vec<u8>>` | `0aa52579-6b0f-9fb6-903a-5645e9f79c78` |
- **The ID scan** (`cargo xtask type-ids`, in `cargo xtask ci` and CI):
  - **What it reads:** every source file of the workspace, parsed with `syn`. It collects
    `#[reflect(id = "...")]` and `StableTypeId::from_uuid("...")`, the latter also inside macro
    invocations, so the built-in types' ID list (one `macro_rules!` list that also produces the
    test's `ALL`) is scanned. Hand-written IDs must use `from_uuid` with a literal.
  - **What it skips:** `tests/`, `benches/` and `examples/`, items marked exactly
    `#[cfg(test)]` or `#[test]`, and out-of-line `#[cfg(test)]` modules. It can't see items
    that macros generate. Symbolic links are skipped, because git checks them out differently
    on Windows and Linux.
  - **Retired IDs** are listed in `docs/development/retired-type-ids.txt`.
  - **Per-crate listings.** For each crate, it keeps `stable-type-ids.txt` at the crate root up
    to date (`--write` regenerates it). The listing holds the IDs that crate's `register_types`
    must register: non-generic types outside function bodies that derive an ID, or whose
    hand-written impl declares `Persistence::Persisted(StableTypeId::from_uuid("..."))`. The
    crate's test checks them with `TypeRegistry::unregistered_ids`.
  - **The built-in IDs** are checked by `oxreal-reflection`'s own test, since three of them
    (`Vec`, `Option`, arrays) are generic definitions rather than types.
  - `cargo xtask new-type-id` prints a new random (version 4) UUID.
- **Miri:**
  - A `miri` CI job on a pinned nightly (`nightly-2026-11-14`) runs `oxreal-reflection`'s tests
    with strict provenance and symbolic alignment checks. The tests cover clones, defaults and
    drops that panic partway through a container, zero-sized and over-aligned elements, and
    zero-length arrays.
  - The ECS storage tests join it in slice 2.
- **Planned for part 2 and slice 2:**
  - Unit enums without a `repr` need a derive-generated accessor (variant index to value and
    back) before views can read or write them.
  - Writes through reflection must enforce `read_only` where a type relies on it, or such
    fields stay opaque.
