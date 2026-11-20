//! Cooking shaders into a directory a shipped game loads (ADR-0019 §4).
//!
//! [`cook`] compiles every shader the given recipes (`shaders/cook.json`, see [`recipe`]) list
//! and writes `<name>.spv` files and a [`manifest`]. The manifest is an attestation that
//! `ShaderCode::from_spirv_unchecked` relies on, so cooking is strict where compiling isn't:
//! - the compiler must be **exactly** the pinned Slang ([`SlangCompiler::ensure_pinned`]);
//! - every module must pass `spirv-val` ([`SpirvValidator`] is required, not optional);
//! - every workspace file the compiler reads must be a declared input. That makes the
//!   [`source key`](key) complete, so the renderer can tell a stale cache from a current one.
//!
//! Shaders whose source key, entry points and optimization level are unchanged, and whose
//! file is intact, are kept rather than recompiled. Files of shaders no recipe lists any more
//! are removed.
//!
//! Every shader is compiled and validated before any file changes, so a failing shader leaves
//! the directory as it was. Files are replaced atomically and the manifest is written last. A
//! cook interrupted while writing leaves the old manifest, whose hashes then fail for the
//! replaced files: the loader rejects that directory rather than load mismatched shaders.

pub mod key;
pub mod manifest;
pub mod paths;
pub mod recipe;

use std::path::{Path, PathBuf};

use oxreal_diagnostics::{debug, info};

pub use key::{SOURCE_KEY_FORMAT, SourceKey, TARGET, source_key};
pub use manifest::{MANIFEST_FILE, MANIFEST_FORMAT, Manifest, ManifestShader, optimization_name};
pub use paths::resolve_exact;
pub use recipe::{
    CookEntry, RECIPE_FORMAT, WorkspacePath, is_valid_output_name, parse_recipe, read_recipe,
};

use self::paths::{DependencyRoots, Location, normalize};
use crate::{
    CompileRequest, Optimization, ShaderError, SlangCompiler, SpirvValidator, pinned_version,
};

/// Why cooking failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CookError {
    /// A recipe is malformed.
    #[error("{recipe}: {details}")]
    Recipe {
        /// The recipe.
        recipe: PathBuf,
        /// What's wrong.
        details: String,
    },
    /// Two recipe entries have the same output name.
    #[error("two shaders are named `{name}`: in {first} and {second}. Output names must be unique")]
    DuplicateOutput {
        /// The name.
        name: String,
        /// The first recipe naming it.
        first: PathBuf,
        /// The second.
        second: PathBuf,
    },
    /// A path in a recipe doesn't exist.
    #[error("{recipe}: `{path}` does not exist")]
    MissingInput {
        /// The recipe.
        recipe: PathBuf,
        /// The path, as written.
        path: String,
    },
    /// A path in a recipe exists only with different case.
    #[error(
        "{recipe}: `{path}` exists only as `{actual}`. Paths must match case exactly, as they do \
         on Linux"
    )]
    CaseMismatch {
        /// The recipe.
        recipe: PathBuf,
        /// The path, as written.
        path: String,
        /// The part as it is on disk.
        actual: String,
    },
    /// The compiler read a workspace file the recipe doesn't declare.
    #[error(
        "{recipe}: shader `{output}` reads `{dependency}`, which its `inputs` don't list. Add it, \
         so changing it invalidates cooked shaders"
    )]
    UndeclaredDependency {
        /// The recipe.
        recipe: PathBuf,
        /// The shader.
        output: String,
        /// The file, workspace-relative.
        dependency: String,
    },
    /// The compiler read a file outside both the workspace and its own installation.
    #[error(
        "{recipe}: shader `{output}` reads {dependency}, outside the workspace. Cooked shaders \
         may only depend on workspace files and the pinned Slang"
    )]
    DependencyOutsideWorkspace {
        /// The recipe.
        recipe: PathBuf,
        /// The shader.
        output: String,
        /// The file.
        dependency: PathBuf,
    },
    /// The compiler or validator isn't usable for cooking.
    #[error(transparent)]
    Tool(ShaderError),
    /// A shader failed to compile or validate.
    #[error("cooking shader `{output}` failed")]
    Shader {
        /// The shader.
        output: String,
        /// Why.
        #[source]
        source: ShaderError,
    },
    /// Reading or writing a file failed.
    #[error("{action} failed: {source}")]
    Io {
        /// What was being done.
        action: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// What to cook, and where.
#[derive(Debug, Clone, Copy)]
pub struct CookRequest<'a> {
    /// The workspace root. Recipe paths are relative to it.
    pub workspace: &'a Path,
    /// The recipes, usually every workspace crate's `shaders/cook.json`.
    pub recipes: &'a [PathBuf],
    /// The cooked shader directory, for example `target/debug/cooked/shaders`. Created if
    /// needed.
    pub out_dir: &'a Path,
    /// How to optimize.
    pub optimization: Optimization,
}

/// What a cook did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CookReport {
    /// Shaders compiled.
    pub cooked: Vec<String>,
    /// Shaders kept, being unchanged.
    pub unchanged: Vec<String>,
    /// Files of shaders no recipe lists any more, removed.
    pub removed: Vec<String>,
}

/// Cooks the shaders `request` lists.
///
/// # Errors
///
/// A [`CookError`] naming the recipe, the shader and what to do. If any shader fails, nothing
/// in the output directory changes.
pub fn cook(
    compiler: &SlangCompiler,
    validator: &SpirvValidator,
    request: &CookRequest<'_>,
) -> Result<CookReport, CookError> {
    compiler.ensure_pinned().map_err(CookError::Tool)?;
    let slang_version = pinned_version();

    let mut entries = Vec::new();
    for recipe in request.recipes {
        entries.extend(read_recipe(recipe)?);
    }
    entries.sort_by(|a, b| a.output.cmp(&b.output));
    for pair in entries.windows(2) {
        if pair[0].output == pair[1].output {
            return Err(CookError::DuplicateOutput {
                name: pair[0].output.clone(),
                first: pair[0].recipe.clone(),
                second: pair[1].recipe.clone(),
            });
        }
    }

    let roots = DependencyRoots {
        workspace: normalize(request.workspace)?,
        compiler: normalize(compiler.install_root())?,
        // Windows file systems ignore case, so its paths compare without it. This is a
        // property of the host's file system, not engine platform code (CLAUDE.md rule 4).
        ignore_case: cfg!(windows),
    };
    std::fs::create_dir_all(request.out_dir).map_err(|source| CookError::Io {
        action: format!("creating {}", request.out_dir.display()),
        source,
    })?;
    let manifest_path = request.out_dir.join(MANIFEST_FILE);
    let previous = std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|text| Manifest::from_previous(&text))
        .filter(|m| m.slang_version == slang_version && m.spirv_val_version == validator.version());

    // Compile and validate everything first. Files change only once every shader has passed.
    let mut report = CookReport::default();
    let mut shaders = Vec::with_capacity(entries.len());
    let mut writes = Vec::new();
    for entry in &entries {
        let (shader, spirv) = cook_entry(
            compiler,
            validator,
            request,
            &roots,
            &slang_version,
            entry,
            previous.as_ref(),
        )?;
        match spirv {
            Some(bytes) => {
                report.cooked.push(shader.name.clone());
                writes.push((request.out_dir.join(&shader.file), bytes));
            }
            None => report.unchanged.push(shader.name.clone()),
        }
        shaders.push(shader);
    }
    for (path, bytes) in &writes {
        write_atomically(path, bytes)?;
    }

    if let Some(previous) = &previous {
        for old in &previous.shaders {
            let listed = shaders.iter().any(|s| s.name == old.name);
            // Only remove what this cook would have written, never an arbitrary path.
            if !listed && is_valid_output_name(&old.name) && old.file == spv_file(&old.name) {
                let path = request.out_dir.join(&old.file);
                match std::fs::remove_file(&path) {
                    Ok(()) => report.removed.push(old.name.clone()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(CookError::Io {
                            action: format!("removing {}", path.display()),
                            source,
                        });
                    }
                }
            }
        }
    }

    let manifest = Manifest {
        slang_version,
        spirv_val_version: validator.version().to_owned(),
        shaders,
    };
    write_atomically(&manifest_path, manifest.to_json().as_bytes())?;
    info!(
        cooked = report.cooked.len(),
        unchanged = report.unchanged.len(),
        removed = report.removed.len(),
        out_dir = %request.out_dir.display(),
        "cooked shaders"
    );
    Ok(report)
}

/// Cooks one entry: its manifest record, and its SPIR-V bytes to write, or `None` if the file
/// already there is unchanged.
fn cook_entry(
    compiler: &SlangCompiler,
    validator: &SpirvValidator,
    request: &CookRequest<'_>,
    roots: &DependencyRoots,
    slang_version: &str,
    entry: &CookEntry,
    previous: Option<&Manifest>,
) -> Result<(ManifestShader, Option<Vec<u8>>), CookError> {
    let workspace = request.workspace;
    let source = resolve_exact(workspace, &entry.source, &entry.recipe)?;
    let include_dirs = entry
        .include_dirs
        .iter()
        .map(|dir| resolve_exact(workspace, dir, &entry.recipe))
        .collect::<Result<Vec<_>, _>>()?;
    for input in &entry.inputs {
        resolve_exact(workspace, input, &entry.recipe)?;
    }
    let key = source_key(workspace, slang_version, entry)?.to_string();
    let file = spv_file(&entry.output);
    let path = request.out_dir.join(&file);

    let unchanged = previous
        .and_then(|m| m.shaders.iter().find(|s| s.name == entry.output))
        .filter(|old| {
            old.source_key == key
                && old.optimization == request.optimization
                && old.entry_points == entry.entry_points
                && old.file == file
        })
        .filter(|old| {
            std::fs::read(&path)
                .is_ok_and(|bytes| key::hex(&key::sha256(&bytes)) == old.spirv_sha256)
        });
    if let Some(old) = unchanged {
        debug!(shader = %entry.output, "unchanged");
        return Ok((old.clone(), None));
    }

    let entry_points: Vec<&str> = entry.entry_points.iter().map(String::as_str).collect();
    let failed = |source| CookError::Shader {
        output: entry.output.clone(),
        source,
    };
    let compiled = compiler
        .compile(&CompileRequest {
            source: &source,
            entry_points: &entry_points,
            include_dirs: &include_dirs,
            optimization: request.optimization,
        })
        .map_err(failed)?;
    check_dependencies(entry, &compiled.dependencies, roots)?;
    validator
        .validate(&compiled.spirv, &source)
        .map_err(failed)?;

    let bytes: Vec<u8> = compiled
        .spirv
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    debug!(shader = %entry.output, words = compiled.spirv.len(), "compiled and validated");
    let shader = ManifestShader {
        name: entry.output.clone(),
        file,
        entry_points: entry.entry_points.clone(),
        optimization: request.optimization,
        spirv_sha256: key::hex(&key::sha256(&bytes)),
        source_key: key,
    };
    Ok((shader, Some(bytes)))
}

/// Checks every file `slangc` read against the declared inputs (ADR-0019 §4).
fn check_dependencies(
    entry: &CookEntry,
    dependencies: &[PathBuf],
    roots: &DependencyRoots,
) -> Result<(), CookError> {
    for dependency in dependencies {
        match roots.locate(&normalize(dependency)?) {
            Location::Compiler => {}
            Location::Workspace(relative) => {
                if !entry.inputs.iter().any(|d| roots.matches(&relative, d)) {
                    return Err(CookError::UndeclaredDependency {
                        recipe: entry.recipe.clone(),
                        output: entry.output.clone(),
                        dependency: relative,
                    });
                }
            }
            Location::Outside => {
                return Err(CookError::DependencyOutsideWorkspace {
                    recipe: entry.recipe.clone(),
                    output: entry.output.clone(),
                    dependency: dependency.clone(),
                });
            }
        }
    }
    Ok(())
}

fn spv_file(name: &str) -> String {
    format!("{name}.spv")
}

/// Writes `path` through a temporary file and a rename, so a reader never sees half a file.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), CookError> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, bytes).map_err(|source| CookError::Io {
        action: format!("writing {}", temporary.display()),
        source,
    })?;
    std::fs::rename(&temporary, path).map_err(|source| CookError::Io {
        action: format!("replacing {}", path.display()),
        source,
    })
}
