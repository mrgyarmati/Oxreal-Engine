- `oxreal-reflection`: reading and editing values through their descriptions (ADR-0021 §4).
  - `ValueRef` and `ValueMut` views reach struct fields (by name or former name), array and list
    elements, `Option` payloads, enum variants and math components such as `translation.x`.
    Views of lists can push and remove elements; views of options can set or clear them.
  - `FieldPath` names a value inside another as text (`stats.health`, `waypoints[2].x`,
    `target?.armor`) and resolves to a `ResolvedPath` for fast access.
  - `OwnedValue` holds a value of any reflected type, for undo and the clipboard. Values read from
    bytes are validated first: a `bool` must be 0 or 1, and an enum's discriminant must name a
    variant.
  - `Patch` applies a list of edits whole or not at all, refuses overlapping edits, and returns
    its inverse for undo.
  - Every mistake in a path or value is an error naming the type, never a panic.
- `TypeRegistry::register_foreign` and `GroupBuilder::add_foreign` register types defined outside
  Rust (C#, C++, scripts) as plain, ABI-safe data, after validating every field's type, offset,
  alignment and overlap.
