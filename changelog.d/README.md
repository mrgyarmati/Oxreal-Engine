# Changelog fragments

Unreleased changes live here as one small file per pull request, not in `CHANGELOG.md`.
Because each PR adds its own file, parallel PRs never conflict over the changelog (ADR-0009).

## Adding a fragment

Create `changelog.d/<short-slug>.<kind>.md`:

- `<short-slug>`: lowercase letters, digits and hyphens. Name it after the change, for example
  `vulkan-swapchain` or `fix-gizmo-snapping`.
- `<kind>`: one of `added`, `changed`, `deprecated`, `removed`, `fixed`, `security`
  ([Keep a Changelog](https://keepachangelog.com/en/1.1.0/) categories).

The content is one or more Markdown bullet points written for **users**: what changed for them,
not how it was implemented. Indented lines continue a bullet.

```markdown
- Vulkan swapchain is recreated automatically when the window is resized or minimized.
```

A PR may add several fragments, for example one `added` and one `fixed`. Internal-only changes
(refactors, CI tweaks, typo fixes) don't need a fragment.

## Checking and releasing

```sh
cargo xtask changelog check            # validates all fragments (part of `cargo xtask ci`)
cargo xtask changelog release 0.1.0    # maintainers only: see docs/development/releasing.md
```
