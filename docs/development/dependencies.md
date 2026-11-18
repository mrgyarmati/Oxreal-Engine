# Third-Party Dependency Register

Every direct third-party dependency must be listed here before it's merged, with the audit result
(use the `dependency-auditor` agent). `deny.toml` enforces license and ban policy mechanically.
This file records the *why*.

Columns:
- **Crate** and **Version**
- **License**
- **Used by**: crates or tools that depend on it
- **Purpose**
- **Exposed in public API?**: should almost always be "no"
- **Maintenance**: last release, activity
- **Platforms**: Windows and Linux status
- **Audited**: date

| Crate | Version | License | Used by | Purpose | Exposed in public API? | Maintenance | Platforms | Audited |
|---|---|---|---|---|---|---|---|---|
| serde_json | 1.x | MIT OR Apache-2.0 | `tools/xtask` (dev tooling only) | Parse `cargo metadata` output for the layer check | No (not shipped) | Very active, de-facto standard | Win + Linux | 2026-11-18 |

## Candidates under evaluation (not yet adopted)

These come from brief §11 and are not commitments: glam, winit, ash, Rapier, Jolt bindings, Kira,
egui, serde and RON, zstd, rayon, and tracing.
