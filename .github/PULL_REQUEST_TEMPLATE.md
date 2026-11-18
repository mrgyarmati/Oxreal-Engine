## Summary

<!-- What does this change do and why? Link the issue (Fixes #123) and any ADR. -->

## Scope

- Owning crate(s) and layer(s):
- Architecture impact: none / ADR added or updated (link) / TAD updated

## Testing

<!-- What did you run, and where? "Compiles on Linux" is not "tested on Linux" (ADR-0002). -->

- [ ] `cargo xtask ci` passes
- [ ] Tested on Windows (version, GPU/driver if relevant):
- [ ] Tested on Linux (distro, Wayland/X11, GPU/driver if relevant):
- [ ] Regression test added (bug fixes)

## Checklist

- [ ] No new third-party dependency, **or** it's listed in `docs/development/dependencies.md` with an audit
- [ ] Every `unsafe` block has a `// SAFETY:` comment
- [ ] Public API documented. User-facing docs updated where relevant
- [ ] `CHANGELOG.md` updated (user-visible changes)
- [ ] No code, shaders or assets derived from Unreal Engine source (ADR-0007)

## Remaining work / known limitations

<!-- What is intentionally not done in this PR? -->
