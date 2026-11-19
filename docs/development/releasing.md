# Releasing

Policy: [ADR-0009](../decisions/0009-versioning-and-release-policy.md). There are no releases
before **0.1.0**, which is cut when Milestone 1's exit criteria pass.

## Version rules (summary)

- One SemVer version for all workspace crates, set in the root `Cargo.toml`
  (`[workspace.package] version`).
- `main` always carries the next version with `-dev`, for example `0.2.0-dev`.
- Before 1.0, a minor bump (`0.x.0`) may break APIs. A patch bump (`0.x.y`) contains only
  compatible fixes.
- Data formats, the C ABI and the plugin API have their own versions (ADR-0009 §9).

## Release checklist (maintainers)

Each release lands as an ordinary pull request. `main` is protected.

1. **Prepare the branch.**
   ```sh
   git switch main && git pull
   git switch -c release/vX.Y.Z
   ```
2. **Set the version.** In the root `Cargo.toml`, change `version = "X.Y.Z-dev"` to
   `version = "X.Y.Z"`, then run `cargo check --workspace` so `Cargo.lock` is updated.
3. **Assemble the changelog.** Preview it first, then write it:
   ```sh
   cargo xtask changelog release X.Y.Z --dry-run   # prints the section, changes nothing
   cargo xtask changelog release X.Y.Z
   ```
   The second command writes the `## [X.Y.Z] - <date>` section into `CHANGELOG.md` and deletes
   the fragments. It always acts on the repository's own `CHANGELOG.md` and `changelog.d/`, not
   on the current directory. Proofread the section, because it becomes the release notes.
4. **Verify.** Run `cargo xtask ci` and `cargo deny check`.
5. **Open the PR** titled `Release vX.Y.Z`, wait for green CI, and merge it.
6. **Tag the merge commit on `main`** with an annotated tag:
   ```sh
   git switch main && git pull
   git tag -a vX.Y.Z -m "Oxreal vX.Y.Z"
   git push origin vX.Y.Z
   ```
7. **Create the GitHub Release** from the tag, pasting the `CHANGELOG.md` section as notes:
   ```sh
   gh release create vX.Y.Z --title "Oxreal vX.Y.Z" --notes-file <file with the section>
   ```
8. **Start the next cycle** with a PR that sets `version = "X.(Y+1).0-dev"`.

Steps 6–8 will be automated by a tag-triggered release workflow before 0.1.0 (ADR-0009 follow-up).

## Patch releases

Patch releases are cut from `main` as long as no breaking change has merged since the last
release. Release branches for backports are introduced only when there's a need, and this
document will be extended when that happens.
