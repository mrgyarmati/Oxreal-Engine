- `oxreal-reflection`: reading and editing values through their descriptions (ADR-0021 §4).
  - `ValueRef` and `ValueMut` views reach struct fields (by name or former name), array and list
    elements, `Option` payloads, enum variants and math components such as `translation.x`.
    Views of lists can push and remove elements; views of options can set or clear them.
  - `FieldPath` names a value inside another as text (`stats.health`, `waypoints[2].x`,
    `target?.armor`) and resolves to a `ResolvedPath` for fast access.
  - `OwnedValue` holds a value of any reflected type, for undo and the clipboard. Values read from
    bytes are validated first: a `bool` must be 0 or 1, and an enum's discriminant must name a
    variant.
  - `Patch` applies a list of edits whole or not at all and refuses overlapping edits. Once
    applied, the patch holds the old values, so applying it again undoes the edit; a patch that
    fails is left unchanged.
  - Every mistake in a path or value is an error, never a panic, naming the type involved and
    what to do.
- `TypeRegistry::register_foreign` and `GroupBuilder::add_foreign` register types defined outside
  Rust (C#, C++, scripts) as plain, ABI-safe data of at most 1 MiB, after validating every
  field's name, type, offset, alignment and overlap.
- `#[derive(Reflect)]` refuses enums without variants and field names other than ASCII letters,
  digits and underscores, which field paths can't name.
