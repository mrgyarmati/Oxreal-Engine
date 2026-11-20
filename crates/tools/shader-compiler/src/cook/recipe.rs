//! `shaders/cook.json`: what a crate cooks (ADR-0019 §4).
//!
//! ```json
//! {
//!   "format": 1,
//!   "shaders": [
//!     {
//!       "output": "Mesh",
//!       "source": "crates/rendering/renderer/shaders/Mesh.slang",
//!       "entry_points": ["meshVertex", "meshPixel"],
//!       "include_dirs": ["crates/rendering/rhi/shaders"],
//!       "inputs": [
//!         "crates/rendering/renderer/shaders/Mesh.slang",
//!         "crates/rendering/rhi/shaders/Bindless.slang"
//!       ]
//!     }
//!   ]
//! }
//! ```
//!
//! Paths are workspace-relative, `/`-separated and case-exact. They're used as written, never
//! derived from the file system, so a recipe means the same on every OS and in every checkout.
//! Unknown fields are errors, so a misspelt field can't be silently ignored.

use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::CookError;

/// The `cook.json` format this crate reads.
pub const RECIPE_FORMAT: u64 = 1;

/// A workspace-relative path as written in a recipe: `/`-separated, with no `.`, `..`, empty or
/// absolute parts, backslashes or drive letters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspacePath(String);

impl WorkspacePath {
    /// Checks `text` against the rules above.
    ///
    /// # Errors
    ///
    /// Why the path isn't acceptable.
    pub fn new(text: &str) -> Result<Self, String> {
        if text.is_empty() {
            return Err("a path is empty".to_owned());
        }
        if text.contains('\\') {
            return Err(format!("`{text}` uses `\\`: separate path parts with `/`"));
        }
        if text.contains(':') || text.contains('\0') {
            return Err(format!(
                "`{text}` contains `:` or NUL: paths are workspace-relative"
            ));
        }
        if text.starts_with('/') {
            return Err(format!(
                "`{text}` is absolute: paths are workspace-relative"
            ));
        }
        for part in text.split('/') {
            if part.is_empty() || part == "." || part == ".." {
                return Err(format!(
                    "`{text}` has an empty, `.` or `..` part: write the plain relative path"
                ));
            }
        }
        Ok(Self(text.to_owned()))
    }

    /// The path as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Its parts, in order.
    pub fn parts(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }
}

impl fmt::Display for WorkspacePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether `name` is a valid output name: 1 to 64 ASCII letters, digits, `_` or `-`, starting
/// with a letter or digit. The cooked file is `<name>.spv`.
#[must_use]
pub fn is_valid_output_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// One shader to cook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookEntry {
    /// The output name. The cooked file is `<output>.spv`.
    pub output: String,
    /// The Slang source.
    pub source: WorkspacePath,
    /// The entry points to compile, in order.
    pub entry_points: Vec<String>,
    /// Directories searched for imports, in order.
    pub include_dirs: Vec<WorkspacePath>,
    /// Every file the shader is built from, the source included, in order. Cooking fails if
    /// the compiler reads a workspace file that isn't listed.
    pub inputs: Vec<WorkspacePath>,
    /// The recipe it came from, for error messages.
    pub recipe: PathBuf,
}

/// Reads a recipe file.
///
/// # Errors
///
/// [`CookError::Io`] if it can't be read, or [`CookError::Recipe`] if it's malformed.
pub fn read_recipe(path: &Path) -> Result<Vec<CookEntry>, CookError> {
    let text = std::fs::read_to_string(path).map_err(|source| CookError::Io {
        action: format!("reading {}", path.display()),
        source,
    })?;
    parse_recipe(&text, path).map_err(|details| CookError::Recipe {
        recipe: path.to_owned(),
        details,
    })
}

/// Parses a recipe's text. `recipe` is recorded in each entry.
///
/// # Errors
///
/// What's wrong with it.
pub fn parse_recipe(text: &str, recipe: &Path) -> Result<Vec<CookEntry>, String> {
    let json: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let top = object(&json, "the recipe")?;
    only_fields(top, "the recipe", &["format", "shaders"])?;
    match top.get("format").and_then(Value::as_u64) {
        Some(RECIPE_FORMAT) => {}
        Some(other) => {
            return Err(format!(
                "format {other} is not supported; this build reads format {RECIPE_FORMAT}"
            ));
        }
        None => return Err("`format` must be a number".to_owned()),
    }
    let shaders = top
        .get("shaders")
        .and_then(Value::as_array)
        .ok_or("`shaders` must be an array")?;
    shaders
        .iter()
        .enumerate()
        .map(|(i, shader)| parse_entry(shader, recipe).map_err(|e| format!("shaders[{i}]: {e}")))
        .collect()
}

fn parse_entry(json: &Value, recipe: &Path) -> Result<CookEntry, String> {
    let fields = object(json, "a shader")?;
    only_fields(
        fields,
        "a shader",
        &["output", "source", "entry_points", "include_dirs", "inputs"],
    )?;
    let output = string(fields, "output")?;
    if !is_valid_output_name(&output) {
        return Err(format!(
            "output `{output}` must be 1 to 64 ASCII letters, digits, `_` or `-`, starting with a \
             letter or digit"
        ));
    }
    let source = WorkspacePath::new(&string(fields, "source")?)?;
    let entry_points = strings(fields, "entry_points")?;
    if entry_points.is_empty() {
        return Err("`entry_points` must not be empty".to_owned());
    }
    let include_dirs = paths(fields, "include_dirs")?;
    let inputs = paths(fields, "inputs")?;
    if !inputs.contains(&source) {
        return Err(format!("`inputs` must list the source, `{source}`"));
    }
    for (i, input) in inputs.iter().enumerate() {
        if inputs[..i].contains(input) {
            return Err(format!("`inputs` lists `{input}` twice"));
        }
    }
    Ok(CookEntry {
        output,
        source,
        entry_points,
        include_dirs,
        inputs,
        recipe: recipe.to_owned(),
    })
}

fn object<'a>(json: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    json.as_object()
        .ok_or_else(|| format!("{what} must be a JSON object"))
}

fn only_fields(fields: &Map<String, Value>, what: &str, allowed: &[&str]) -> Result<(), String> {
    match fields.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(unknown) => Err(format!(
            "{what} has an unknown field `{unknown}` (expected: {})",
            allowed.join(", ")
        )),
        None => Ok(()),
    }
}

fn string(fields: &Map<String, Value>, name: &str) -> Result<String, String> {
    fields
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("`{name}` must be a string"))
}

fn strings(fields: &Map<String, Value>, name: &str) -> Result<Vec<String>, String> {
    let error = || format!("`{name}` must be an array of non-empty strings");
    fields
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(error)?
        .iter()
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(error)
        })
        .collect()
}

fn paths(fields: &Map<String, Value>, name: &str) -> Result<Vec<WorkspacePath>, String> {
    strings(fields, name)?
        .iter()
        .map(|s| WorkspacePath::new(s).map_err(|e| format!("`{name}`: {e}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Vec<CookEntry>, String> {
        parse_recipe(text, Path::new("cook.json"))
    }

    const VALID: &str = r#"{
        "format": 1,
        "shaders": [{
            "output": "Mesh",
            "source": "a/Mesh.slang",
            "entry_points": ["meshVertex", "meshPixel"],
            "include_dirs": ["b"],
            "inputs": ["a/Mesh.slang", "b/Bindless.slang"]
        }]
    }"#;

    #[test]
    fn parses_a_valid_recipe() {
        let entries = parse(VALID).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.output, "Mesh");
        assert_eq!(entry.source.as_str(), "a/Mesh.slang");
        assert_eq!(entry.entry_points, ["meshVertex", "meshPixel"]);
        assert_eq!(entry.include_dirs[0].as_str(), "b");
        assert_eq!(entry.inputs.len(), 2);
    }

    #[test]
    fn rejects_paths_that_are_not_plain_workspace_relative() {
        for bad in [
            "",
            "/abs/x.slang",
            "C:/x.slang",
            "a\\b.slang",
            "a//b.slang",
            "a/./b.slang",
            "a/../b.slang",
            "..",
            "a/",
        ] {
            assert!(WorkspacePath::new(bad).is_err(), "`{bad}` was accepted");
        }
        assert!(WorkspacePath::new("crates/x/Shader.slang").is_ok());
    }

    #[test]
    fn checks_output_names() {
        for good in ["Mesh", "mesh_lit", "a-1", "9"] {
            assert!(is_valid_output_name(good), "{good}");
        }
        for bad in ["", "_x", "-x", "a.b", "a/b", "a b", "ä", &"x".repeat(65)] {
            assert!(!is_valid_output_name(bad), "{bad}");
        }
    }

    #[test]
    fn rejects_malformed_recipes_with_a_reason() {
        let cases = [
            ("[]", "JSON object"),
            (r#"{"format": 2, "shaders": []}"#, "format 2"),
            (r#"{"format": 1}"#, "`shaders`"),
            (
                r#"{"format": 1, "shaders": [], "extra": 0}"#,
                "unknown field `extra`",
            ),
            (
                &VALID.replace("\"inputs\": [\"a/Mesh.slang\", ", "\"inputs\": ["),
                "must list the source",
            ),
            (
                &VALID.replace("\"meshVertex\", \"meshPixel\"", ""),
                "must not be empty",
            ),
            (
                &VALID.replace("\"output\": \"Mesh\"", "\"output\": \"../x\""),
                "output",
            ),
            (
                &VALID.replace("\"include_dirs\"", "\"includes\""),
                "unknown field `includes`",
            ),
            (
                &VALID.replace("\"b/Bindless.slang\"", "\"a/Mesh.slang\""),
                "twice",
            ),
        ];
        for (text, expected) in cases {
            let err = parse(text).unwrap_err();
            assert!(
                err.contains(expected),
                "`{err}` should mention `{expected}`"
            );
        }
    }
}
