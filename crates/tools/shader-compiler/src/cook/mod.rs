//! Cooking shaders into a directory a shipped game loads (ADR-0019 §4).
//!
//! [`cook`] compiles every shader the given recipes (`shaders/cook.json`, see [`CookEntry`]) list
//! and writes `<name>.spv` files and a [`Manifest`]. The manifest is an attestation that
//! `ShaderCode::from_spirv_unchecked` relies on, so cooking is strict where compiling isn't:
//! - the compiler must be **exactly** the pinned Slang ([`SlangCompiler::ensure_pinned`]);
//! - every module must pass `spirv-val` ([`SpirvValidator`] is required, not optional);
//! - every workspace file the compiler reads must be a declared input. That makes the
//!   [source key](source_key) complete, so the renderer can tell a stale cache from a current one.
//!
//! Shaders whose source key, entry points and optimization level are unchanged, and whose
//! file is intact, are kept rather than recompiled. Files of shaders no recipe lists any more
//! are removed.
//!
//! Every shader is compiled and validated before any file changes, so a failing shader leaves
//! the directory as it was. Files are replaced atomically and the manifest is written last. A
//! cook interrupted while writing leaves the old manifest, whose hashes then fail for the
//! replaced files: the loader rejects that directory rather than load mismatched shaders.

mod key;
mod manifest;
mod paths;
mod recipe;

use std::path::{Path, PathBuf};

use oxreal_diagnostics::{debug, info};

pub use key::{SOURCE_KEY_FORMAT, SourceKey, TARGET, source_key};
pub use manifest::{MANIFEST_FILE, MANIFEST_FORMAT, Manifest, ManifestShader};
pub use recipe::{CookEntry, RECIPE_FORMAT, WorkspacePath, read_recipe};

use self::paths::{DependencyRoots, Location, normalize, resolve_exact};
use self::recipe::is_valid_output_name;
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
    /// The compiler's installation overlaps the workspace's sources: it contains the workspace,
    /// or lies inside it somewhere other than `.tools/`. Its own modules are exempt from the
    /// declared-input check, so workspace files would be too.
    #[error(
        "the Slang compiler's installation directory {} overlaps the workspace, so the cook \
         couldn't tell its modules from workspace files. Use the pinned Slang \
         (`cargo xtask slang install`), or point OXREAL_SLANGC at a slangc in its own \
         installation outside the workspace",
        compiler_root.display()
    )]
    CompilerOverlapsWorkspace {
        /// The compiler's installation directory, normalized (`/`-separated, symlinks
        /// resolved).
        compiler_root: PathBuf,
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
#[non_exhaustive]
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
    // Output names must differ in more than case: on Windows `Mesh.spv` and `mesh.spv` are
    // one file. The rule holds on every platform, so a recipe can't work only on Linux.
    entries.sort_by_key(|e| e.output.to_ascii_lowercase());
    for pair in entries.windows(2) {
        if pair[0].output.eq_ignore_ascii_case(&pair[1].output) {
            return Err(CookError::DuplicateOutput {
                name: pair[1].output.clone(),
                first: pair[0].recipe.clone(),
                second: pair[1].recipe.clone(),
            });
        }
    }

    let compiler_root = compiler.install_root().map_err(CookError::Tool)?;
    let roots = DependencyRoots {
        workspace: normalize(request.workspace)?,
        compiler: normalize(&compiler_root)?,
        // Windows file systems ignore case, so its paths compare without it. This is a
        // property of the host's file system, not engine platform code (CLAUDE.md rule 4).
        ignore_case: cfg!(windows),
    };
    roots.check()?;
    std::fs::create_dir_all(request.out_dir).map_err(|source| CookError::Io {
        action: format!("creating {}", request.out_dir.display()),
        source,
    })?;
    let manifest_path = request.out_dir.join(MANIFEST_FILE);
    let previous = std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|text| Manifest::from_previous(&text));
    // Shaders can be reused only from a manifest made by the same tools.
    let reusable = previous
        .as_ref()
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
            reusable,
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

    // Remove dropped shaders' files before writing, so a shader renamed only by case
    // (`Tint` to `tint`) loses its old file before the new one lands: on Windows they're one
    // file.
    if let Some(previous) = &previous {
        for old in &previous.shaders {
            let listed = shaders.iter().any(|s| s.name == old.name);
            // Only remove what this cook would have written, never an arbitrary path from the
            // previous manifest.
            if !listed && is_valid_output_name(&old.name) && old.file == spv_file(&old.name) {
                let path = request.out_dir.join(&old.file);
                match with_sharing_retries(|| std::fs::remove_file(&path)) {
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
    for (path, bytes) in &writes {
        write_atomically(path, bytes)?;
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
/// The temporary file is named after the process, so concurrent cooks don't share it, and it's
/// removed if the write fails.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), CookError> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let result = std::fs::write(&temporary, bytes)
        .map_err(|source| CookError::Io {
            action: format!("writing {}", temporary.display()),
            source,
        })
        .and_then(|()| {
            with_sharing_retries(|| std::fs::rename(&temporary, path)).map_err(|source| {
                CookError::Io {
                    action: format!(
                        "replacing {} (is a running program holding it open?)",
                        path.display()
                    ),
                    source,
                }
            })
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Runs `operation`, retrying for up to a second while it fails with `PermissionDenied`.
///
/// On Windows, replacing or deleting a file fails while another process has it open without
/// delete sharing, and virus scanners and the search indexer open new files briefly. Elsewhere
/// that error is permanent, so there are no retries. A file a program keeps open fails after
/// the retries, with the error naming it.
fn with_sharing_retries(mut operation: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    // A property of Windows file sharing, not engine platform code (CLAUDE.md rule 4).
    let retries = if cfg!(windows) { 20 } else { 0 };
    let mut attempt = 0;
    loop {
        match operation() {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && attempt < retries => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependencies_are_checked_against_the_declared_inputs() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cook");
        let entry = read_recipe(&workspace.join("shaders/cook.json")).unwrap()[0].clone();
        /// Removes the directory even if an assertion fails.
        struct TempDir(PathBuf);
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        // A file outside the workspace, and a separate "compiler installation".
        let temp = TempDir(
            std::env::temp_dir().join(format!("oxreal-cook-outside-{}", std::process::id())),
        );
        let outside = &temp.0;
        let compiler = outside.join("slang");
        std::fs::create_dir_all(compiler.join("lib")).unwrap();
        std::fs::write(outside.join("Stray.slang"), "").unwrap();
        std::fs::write(compiler.join("lib/core.slang"), "").unwrap();
        let roots = DependencyRoots {
            workspace: normalize(&workspace).unwrap(),
            compiler: normalize(&compiler).unwrap(),
            ignore_case: cfg!(windows),
        };

        let declared = [
            workspace.join("shaders/Tint.slang"),
            workspace.join("common/Palette.slang"),
        ];
        assert!(check_dependencies(&entry, &declared, &roots).is_ok());
        assert!(
            check_dependencies(&entry, &[compiler.join("lib/core.slang")], &roots).is_ok(),
            "the compiler's own modules are exempt"
        );
        assert!(matches!(
            check_dependencies(&entry, &[outside.join("Stray.slang")], &roots),
            Err(CookError::DependencyOutsideWorkspace { .. })
        ));
        match check_dependencies(&entry, &[workspace.join("shaders/cook.json")], &roots) {
            Err(CookError::UndeclaredDependency { dependency, .. }) => {
                assert_eq!(dependency, "shaders/cook.json");
            }
            other => panic!("expected UndeclaredDependency, got {other:?}"),
        }
    }

    #[test]
    fn sharing_retries_stop_at_success_or_a_permanent_error() {
        let mut calls = 0;
        let result = with_sharing_retries(|| {
            calls += 1;
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1, "only PermissionDenied is retried");

        let mut calls = 0;
        let result = with_sharing_retries(|| {
            calls += 1;
            if calls < 2 && cfg!(windows) {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            } else {
                Ok(())
            }
        });
        assert!(result.is_ok());
    }
}
