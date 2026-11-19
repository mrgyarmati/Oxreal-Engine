//! Engine-owned shader reflection, parsed from `slangc -reflection-json` (ADR-0013 §2). The
//! JSON format never leaves this crate.

use serde_json::Value;

/// A pipeline stage.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ShaderStage {
    /// Vertex shader.
    Vertex,
    /// Pixel (fragment) shader.
    Pixel,
    /// Compute shader.
    Compute,
    /// Any other stage, by its Slang name.
    Other(String),
}

/// One entry point of a compiled module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryPoint {
    /// The entry point's name. It's also its name in SPIR-V (`-fvk-use-entrypoint-name`).
    pub name: String,
    /// Its stage.
    pub stage: ShaderStage,
}

/// A descriptor binding a shader uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceBinding {
    /// The shader parameter's name.
    pub name: String,
    /// Descriptor set.
    pub set: u32,
    /// Binding index within the set.
    pub binding: u32,
}

/// What a compiled module needs from the pipeline layout.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reflection {
    /// Entry points, in the order `slangc` reports them.
    pub entry_points: Vec<EntryPoint>,
    /// Descriptor bindings.
    pub bindings: Vec<ResourceBinding>,
    /// Whether any entry point uses push constants.
    pub uses_push_constants: bool,
}

/// Parses `slangc`'s reflection JSON.
pub(crate) fn parse(text: &str) -> Result<Reflection, String> {
    let json: Value =
        serde_json::from_str(text).map_err(|e| format!("invalid reflection JSON: {e}"))?;
    let mut reflection = Reflection::default();

    for entry in json["entryPoints"].as_array().into_iter().flatten() {
        let name = entry["name"]
            .as_str()
            .ok_or("reflection entry point without a name")?
            .to_owned();
        let stage = match entry["stage"].as_str().unwrap_or_default() {
            "vertex" => ShaderStage::Vertex,
            "fragment" | "pixel" => ShaderStage::Pixel,
            "compute" => ShaderStage::Compute,
            other => ShaderStage::Other(other.to_owned()),
        };
        reflection.entry_points.push(EntryPoint { name, stage });
    }

    for parameter in json["parameters"].as_array().into_iter().flatten() {
        let binding = &parameter["binding"];
        match binding["kind"].as_str() {
            Some("descriptorTableSlot") => {
                let index = |key: &str| -> Result<u32, String> {
                    binding[key]
                        .as_u64()
                        .map_or(Ok(0), |v| u32::try_from(v).map_err(|e| e.to_string()))
                };
                reflection.bindings.push(ResourceBinding {
                    name: parameter["name"].as_str().unwrap_or_default().to_owned(),
                    set: index("space")?,
                    binding: index("index")?,
                });
            }
            Some("pushConstantBuffer") => reflection.uses_push_constants = true,
            _ => {}
        }
    }
    Ok(reflection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entry_points_bindings_and_push_constants() {
        // Shape captured from slangc during the ADR-0013 spike.
        let json = r#"{
            "parameters": [
                {"name": "camera", "binding": {"kind": "descriptorTableSlot", "index": 0}},
                {"name": "textures", "binding": {"kind": "descriptorTableSlot", "space": 1, "index": 0}},
                {"name": "pc", "binding": {"kind": "pushConstantBuffer", "index": 0}}
            ],
            "entryPoints": [
                {"name": "vsMain", "stage": "vertex"},
                {"name": "fsMain", "stage": "fragment"}
            ]
        }"#;
        let r = parse(json).unwrap();
        assert_eq!(
            r.entry_points,
            vec![
                EntryPoint {
                    name: "vsMain".into(),
                    stage: ShaderStage::Vertex
                },
                EntryPoint {
                    name: "fsMain".into(),
                    stage: ShaderStage::Pixel
                },
            ]
        );
        assert_eq!(
            r.bindings,
            vec![
                ResourceBinding {
                    name: "camera".into(),
                    set: 0,
                    binding: 0
                },
                ResourceBinding {
                    name: "textures".into(),
                    set: 1,
                    binding: 0
                },
            ]
        );
        assert!(r.uses_push_constants);
    }

    #[test]
    fn empty_and_invalid_json() {
        assert_eq!(parse("{}").unwrap(), Reflection::default());
        assert!(parse("not json").is_err());
    }
}
