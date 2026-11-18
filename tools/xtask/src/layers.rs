//! Crate layering check (ADR-0004, TAD §4).
//!
//! Every workspace crate declares `[package.metadata.engine] layer = "<layer>"`. Each normal or
//! build dependency edge between workspace crates must be allowed by [`Layer::may_depend_on`].
//! Dev-dependencies are exempt, except that nothing below the editor may dev-depend on editor crates.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Layer {
    Foundation,
    Platform,
    Runtime,
    Rendering,
    Simulation,
    Systems,
    Assets,
    Gameplay,
    Editor,
    AppRuntime,
    AppEditor,
    Tool,
}

impl Layer {
    const ALL: [Layer; 12] = [
        Layer::Foundation,
        Layer::Platform,
        Layer::Runtime,
        Layer::Rendering,
        Layer::Simulation,
        Layer::Systems,
        Layer::Assets,
        Layer::Gameplay,
        Layer::Editor,
        Layer::AppRuntime,
        Layer::AppEditor,
        Layer::Tool,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Layer::Foundation => "foundation",
            Layer::Platform => "platform",
            Layer::Runtime => "runtime",
            Layer::Rendering => "rendering",
            Layer::Simulation => "simulation",
            Layer::Systems => "systems",
            Layer::Assets => "assets",
            Layer::Gameplay => "gameplay",
            Layer::Editor => "editor",
            Layer::AppRuntime => "app-runtime",
            Layer::AppEditor => "app-editor",
            Layer::Tool => "tool",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Layer> {
        Layer::ALL.into_iter().find(|l| l.name() == s)
    }

    fn is_editor(self) -> bool {
        matches!(self, Layer::Editor | Layer::AppEditor)
    }

    /// The allowed-dependency matrix. Keep in sync with docs/architecture/TAD.md §4.
    pub(crate) fn may_depend_on(self, dep: Layer) -> bool {
        use Layer::*;
        match self {
            Foundation => matches!(dep, Foundation),
            Platform => matches!(dep, Foundation | Platform),
            Runtime => matches!(dep, Foundation | Platform | Runtime),
            Rendering => matches!(dep, Foundation | Platform | Runtime | Rendering),
            Simulation => matches!(dep, Foundation | Runtime | Simulation),
            Systems => matches!(dep, Foundation | Platform | Runtime | Systems),
            Assets => matches!(dep, Foundation | Platform | Runtime | Assets),
            Gameplay => !matches!(dep, Editor | AppRuntime | AppEditor | Tool),
            Editor => !matches!(dep, AppRuntime | AppEditor | Tool),
            AppRuntime => !matches!(dep, Editor | AppRuntime | AppEditor | Tool),
            AppEditor => !matches!(dep, AppRuntime | AppEditor | Tool),
            Tool => !matches!(dep, Editor | AppRuntime | AppEditor),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DepKind {
    Normal,
    Build,
    Dev,
}

#[derive(Debug)]
pub(crate) struct Package {
    pub(crate) name: String,
    /// Raw `metadata.engine.layer` value, if declared.
    pub(crate) layer: Option<String>,
    /// Workspace-internal (path) dependencies only.
    pub(crate) deps: Vec<(String, DepKind)>,
}

/// Runs `cargo metadata` and checks the workspace. Prints a summary.
pub(crate) fn run(cargo: &str, root: &Path) -> Result<(), String> {
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let json: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("cannot parse cargo metadata output: {e}"))?;
    let packages = parse_packages(&json)?;
    let violations = check(&packages);
    if violations.is_empty() {
        println!("layers: {} workspace crate(s) OK", packages.len());
        Ok(())
    } else {
        Err(format!(
            "{} layering violation(s) (see docs/architecture/TAD.md §4):\n  - {}",
            violations.len(),
            violations.join("\n  - ")
        ))
    }
}

pub(crate) fn parse_packages(json: &Value) -> Result<Vec<Package>, String> {
    let pkgs = json["packages"]
        .as_array()
        .ok_or("cargo metadata: missing `packages` array")?;
    Ok(pkgs
        .iter()
        .map(|p| Package {
            name: p["name"].as_str().unwrap_or_default().to_owned(),
            layer: p["metadata"]["engine"]["layer"].as_str().map(str::to_owned),
            deps: p["dependencies"]
                .as_array()
                .map(|deps| {
                    deps.iter()
                        .filter(|d| d.get("path").is_some_and(|v| !v.is_null()))
                        .map(|d| {
                            let kind = match d["kind"].as_str() {
                                Some("dev") => DepKind::Dev,
                                Some("build") => DepKind::Build,
                                _ => DepKind::Normal,
                            };
                            (d["name"].as_str().unwrap_or_default().to_owned(), kind)
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect())
}

/// Returns human-readable violations. An empty result means the workspace is OK.
pub(crate) fn check(packages: &[Package]) -> Vec<String> {
    let mut violations = Vec::new();
    let mut layers = BTreeMap::new();
    for p in packages {
        match p.layer.as_deref().map(|s| (s, Layer::parse(s))) {
            None => violations.push(format!(
                "`{}` has no [package.metadata.engine] layer = \"...\"",
                p.name
            )),
            Some((raw, None)) => violations.push(format!(
                "`{}` declares unknown layer `{raw}` (valid: {})",
                p.name,
                Layer::ALL.map(Layer::name).join(", ")
            )),
            Some((_, Some(layer))) => {
                layers.insert(p.name.as_str(), layer);
            }
        }
    }
    for p in packages {
        let Some(&from) = layers.get(p.name.as_str()) else {
            continue;
        };
        for (dep, kind) in &p.deps {
            // Path deps outside the workspace or with an invalid layer are reported elsewhere.
            let Some(&to) = layers.get(dep.as_str()) else {
                continue;
            };
            let ok = match kind {
                DepKind::Dev => from.is_editor() || !to.is_editor(),
                DepKind::Normal | DepKind::Build => from.may_depend_on(to),
            };
            if !ok {
                violations.push(format!(
                    "`{}` ({}) must not depend on `{dep}` ({}) [{kind:?} dependency]",
                    p.name,
                    from.name(),
                    to.name()
                ));
            }
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, layer: Option<&str>, deps: &[(&str, DepKind)]) -> Package {
        Package {
            name: name.to_owned(),
            layer: layer.map(str::to_owned),
            deps: deps.iter().map(|(n, k)| ((*n).to_owned(), *k)).collect(),
        }
    }

    #[test]
    fn valid_stack_passes() {
        let ws = [
            pkg("math", Some("foundation"), &[]),
            pkg("ecs", Some("runtime"), &[("math", DepKind::Normal)]),
            pkg("rhi", Some("rendering"), &[("ecs", DepKind::Normal)]),
            pkg("editor-fw", Some("editor"), &[("rhi", DepKind::Normal)]),
            pkg(
                "editor",
                Some("app-editor"),
                &[("editor-fw", DepKind::Normal)],
            ),
            pkg("player", Some("app-runtime"), &[("rhi", DepKind::Normal)]),
        ];
        assert!(check(&ws).is_empty(), "{:?}", check(&ws));
    }

    #[test]
    fn runtime_must_not_depend_on_editor() {
        let ws = [
            pkg("editor-fw", Some("editor"), &[]),
            pkg(
                "player",
                Some("app-runtime"),
                &[("editor-fw", DepKind::Normal)],
            ),
        ];
        assert_eq!(check(&ws).len(), 1);
    }

    #[test]
    fn sibling_layers_are_isolated() {
        let ws = [
            pkg("rhi", Some("rendering"), &[]),
            pkg("physics", Some("simulation"), &[("rhi", DepKind::Normal)]),
        ];
        assert_eq!(check(&ws).len(), 1);
    }

    #[test]
    fn upward_dependency_rejected() {
        let ws = [
            pkg("ecs", Some("runtime"), &[]),
            pkg("math", Some("foundation"), &[("ecs", DepKind::Build)]),
        ];
        assert_eq!(check(&ws).len(), 1);
    }

    #[test]
    fn dev_deps_exempt_except_editor() {
        let ws = [
            pkg("ecs", Some("runtime"), &[]),
            pkg("editor-fw", Some("editor"), &[]),
            pkg(
                "math",
                Some("foundation"),
                &[("ecs", DepKind::Dev), ("editor-fw", DepKind::Dev)],
            ),
        ];
        let v = check(&ws);
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("editor-fw"));
    }

    #[test]
    fn missing_and_unknown_layers_reported() {
        let ws = [pkg("a", None, &[]), pkg("b", Some("engine"), &[])];
        assert_eq!(check(&ws).len(), 2);
    }

    #[test]
    fn parses_cargo_metadata_shape() {
        let json = serde_json::json!({
            "packages": [{
                "name": "ecs",
                "metadata": { "engine": { "layer": "runtime" } },
                "dependencies": [
                    { "name": "math", "kind": null, "path": "/ws/crates/foundation/math" },
                    { "name": "serde", "kind": null },
                    { "name": "helper", "kind": "dev", "path": "/ws/tools/helper" }
                ]
            }]
        });
        let pkgs = parse_packages(&json).unwrap();
        assert_eq!(pkgs[0].layer.as_deref(), Some("runtime"));
        assert_eq!(
            pkgs[0].deps,
            vec![
                ("math".to_owned(), DepKind::Normal),
                ("helper".to_owned(), DepKind::Dev)
            ]
        );
    }
}
