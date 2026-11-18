# ADR-0006: Stable asset identity and non-destructive reimport

- **Status:** Proposed. The storage format is pending D-08.
- **Date:** 2026-11-18
- **Related:** PRD §7, TAD §5.5, brief §8; open decisions D-07, D-08

## Context

Blender round-tripping is a core workflow. Reimporting a changed source must not break references
or wipe engine-side configuration such as material assignments, collision settings, LOD settings or
per-asset tweaks.

## Decision (proposed)

- **Identity:** every asset has a random 128-bit `AssetId`, created on first import and stored in a
  text sidecar next to the source (or in project metadata). It is never derived from paths, so moves
  and renames don't break references. Sub-assets (meshes and materials inside a glTF) get IDs
  derived deterministically from the parent ID plus a stable source-side key (node path or name),
  recorded so that renaming in the DCC can be remapped.
- **Import record:** source path, content hash, importer id and version, import settings, produced
  artifacts and dependencies.
- **Overrides:** engine-side edits are stored as explicit overrides, keyed by AssetId and property
  path and kept separate from imported data. After import the pipeline applies them in this order:
  import, then defaults, then overrides. Overrides that no longer resolve are reported, never
  silently dropped.
- **Derived data cache:** keyed by (source hash, importer version, settings hash, target platform).
- **References** in scenes and assets are always by AssetId (and sub-id), never by path.

## Consequences

- Sidecar files must be committed to version control alongside sources.
- An importer version bump invalidates the DDC entries for that importer.
- We need a remap UI for sub-assets that were renamed or removed in the source.
