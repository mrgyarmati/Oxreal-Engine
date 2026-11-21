- `oxreal-reflection` and `#[derive(Reflect)]` (ADR-0021): types can describe themselves at run
  time, as plain data the editor, saving and other languages will read.
  - Every reflected type has a stable type ID (`#[reflect(id = "...")]`), which survives renames
    and moves, or is declared `transient` (never saved) or `rebuilt`. Generic instantiations
    such as `Vec<f32>` get IDs derived from their arguments.
  - Descriptions cover primitives, `String`, `Vec`, `Option`, fixed arrays, structs, tuple
    structs, enums without data, and the `oxreal-math` types, with editor attributes (display
    name, category, tooltip from the doc comment, range, units, hidden, read-only), field aliases
    for renames, and drop, clone and default through erased pointers.
  - `TypeRegistry` holds the descriptions, shared by value (no global registry). Registration is
    all or nothing, duplicate IDs are refused naming both types, and groups of types can be
    registered and removed together, for plugins and hot reload.
  - The derive rejects, at compile time, a missing or malformed ID, a saved type with a field of
    a transient type, `abi` types without `#[repr(C)]` or with non-ABI-safe fields,
    `#[repr(packed)]`, and `clone` or `default` without the real impl.
- `cargo xtask new-type-id` prints a new stable type ID. `cargo xtask type-ids` (part of
  `cargo xtask ci` and CI) rejects IDs declared twice in the workspace and retired IDs
  (`docs/development/retired-type-ids.txt`).
