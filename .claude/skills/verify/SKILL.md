---
name: verify
description: Run the full local verification pipeline for this Rust engine workspace (fmt, clippy -D warnings, tests, crate layer check, optional cargo-deny) and report results honestly. Use before declaring any implementation task done, before commits, or when the user asks to check/verify the build.
---

# Verify the workspace

1. Run the CI-equivalent pipeline from the workspace root:
   ```sh
   cargo xtask ci
   ```
   This runs `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, and `cargo xtask layers`.

2. If formatting fails, run `cargo fmt --all` and re-run. Formatting fixes are always safe.

3. If `cargo-deny` is installed (`cargo deny --version`), also run:
   ```sh
   cargo deny check
   ```
   If it isn't installed, say so. Don't install it without asking.

4. Fix clippy warnings properly. Use `#[allow(...)]` only with a comment explaining why, and
   never add crate-wide allows to silence warnings in bulk.

5. Report:
   - Each step's pass or fail status, with the relevant error output when something failed.
   - **Which OS this ran on.** Local runs cover one OS only. State that the other OS is verified by
     CI (`.github/workflows/ci.yml`) or not verified yet. Never claim Linux support from a Windows
     run (ADR-0002).
   - Tests that were skipped or ignored (for example GPU-gated tests) and why.
