# ADR-0009: Versioning and release policy

- **Status:** Accepted
- **Date:** 2026-11-19
- **Related:** PRD §8 (milestones), PRD §10 (governance), D-07 (file formats), D-13 (plugin ABI), D-21 (C ABI)

## Context

Oxreal is a multi-crate workspace that will ship an engine (crates), an editor and tools
(binaries), and later data formats and a C ABI. Contributors and users need to know what a version
number promises, when releases happen, and how to find out what changed.

There is also an immediate practical problem. One shared `## [Unreleased]` list in `CHANGELOG.md`
causes merge conflicts whenever two PRs add entries (PRs #1 and #3 conflicted on 2026-11-18).

## Options considered

### Version scheme
- **A. One unified version for all workspace crates (lockstep).** It's simple to reason about
  ("Oxreal 0.3"), matches how users pick an engine version, and needs no compatibility matrix
  between engine crates. The cost is that unchanged crates get version bumps.
- **B. Independent version per crate.** This is precise, but needs a compatibility matrix and is
  confusing for an engine whose crates are designed and released together.

### Changelog
- **A. One hand-edited `[Unreleased]` list.** This causes merge conflicts between parallel PRs.
- **B. Generate it from commit messages** (git-cliff 2.14, or release-plz 0.4 with Conventional
  Commits). That requires a strict commit-message format from every contributor, and commit
  messages describe implementation steps rather than user-visible changes.
- **C. Changelog fragments.** Each PR adds its own small file and a tool assembles them at
  release time (the pattern used by towncrier and others). There are no conflicts, entries stay
  human-written and user-facing, and review happens on the PR that makes the change.

### Release tooling
- **release-plz** (0.4.3, MIT/Apache-2.0, active) automates release PRs and crates.io publishing,
  and is designed around Conventional Commits and per-crate versions.
- **cargo-release** (1.1.7, MIT/Apache-2.0, active) automates version bumps, tags and publishing,
  driven by hand.
- **xtask** (in-repo) gives full control with no new dependency. Its scope is small: assemble
  fragments, set the version.

## Decision

1. **Semantic Versioning 2.0.0, one unified version** for every workspace crate, set once in
   `[workspace.package] version`. Every Oxreal crate inherits it with `version.workspace = true`.
2. **Before 1.0:** each minor release `0.MINOR.0` may contain breaking changes, and patch releases
   `0.MINOR.PATCH` contain only compatible fixes. **1.0.0** is declared when the PRD's final product
   test passes and the public API is ready for stability guarantees.
3. **Releases follow milestones.** **0.1.0** is cut when Milestone 1's exit criteria pass, and
   each later milestone produces at least one minor release. There are no releases before 0.1.0.
4. **Development versions:** `main` always carries the *next* version with a `-dev` pre-release
   suffix (now `0.1.0-dev`). A release commit drops the suffix, gets tagged, and the next commit
   bumps to the following `-dev` version.
5. **Tags and GitHub Releases:** one annotated tag per release, `vX.Y.Z`, on `main`. Its GitHub
   Release notes are the matching `CHANGELOG.md` section.
6. **Changelog fragments:** every PR with a user-visible change adds
   `changelog.d/<short-slug>.<kind>.md`, where `<kind>` is one of the
   [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) categories: `added`, `changed`,
   `deprecated`, `removed`, `fixed` or `security`. The file holds one or more Markdown bullet points.
   - `cargo xtask changelog check` validates the fragments and runs in CI.
   - `cargo xtask changelog release <version>` writes them into `CHANGELOG.md` under
     `## [<version>] - <date>`, grouped by kind, and deletes the fragments.
7. **Release tooling:** the in-repo `xtask` handles fragments and versions for now, so no new
   dependency. Re-evaluate release-plz or cargo-release when we start publishing to crates.io.
8. **crates.io:** crates stay `publish = false` until a release deliberately publishes them.
   We don't register placeholder crates; crates.io policy forbids name squatting.
9. **Versioned separately** (each has its own version and compatibility policy, defined in its
   own ADR): scene, asset and project file formats (D-07), the stable C ABI (D-21), and the
   plugin API (D-13). Their versions are not tied to the engine's SemVer.
10. **Toolchain and MSRV:** `rust-version` equals the pinned toolchain in `rust-toolchain.toml`.
    Raising it is a minor-level change before 1.0 and gets a `changed` changelog entry.

## Consequences

- Version numbers make one simple promise users can rely on, and milestones give releases a
  meaning.
- Parallel PRs no longer conflict on the changelog.
- Contributors must remember to add a fragment. The PR template asks for one.
- The release steps are documented in `docs/development/releasing.md` and partly automated by
  `xtask`. A tag-triggered release workflow is follow-up work for 0.1.0.
- Follow-ups: add API compatibility checks (cargo-semver-checks 0.51, MIT/Apache-2.0) once crates
  are published, and decide crates.io publishing before 0.1.0.
- Revisit triggers: crates needing independent release cadences, or contributors repeatedly
  forgetting fragments (then enforce them per PR in CI).
