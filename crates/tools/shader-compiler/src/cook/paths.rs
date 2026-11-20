//! Resolving recipe paths, and normalizing `slangc`'s dependency list (ADR-0019 §4).

use std::path::{Path, PathBuf};

use super::{CookError, WorkspacePath};

/// Resolves `path` below `workspace`, requiring every part to exist with **exactly** this case.
///
/// Windows and macOS file systems ignore case, Linux's don't. Checking here makes a recipe that
/// would only work on Windows fail on Windows too.
///
/// # Errors
///
/// [`CookError::MissingInput`] if it doesn't exist, [`CookError::CaseMismatch`] if it exists
/// only with different case, or [`CookError::Io`] if a directory can't be listed.
pub(crate) fn resolve_exact(
    workspace: &Path,
    path: &WorkspacePath,
    recipe: &Path,
) -> Result<PathBuf, CookError> {
    let mut current = workspace.to_owned();
    for part in path.parts() {
        let entries = std::fs::read_dir(&current).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                missing(path, recipe)
            } else {
                CookError::Io {
                    action: format!("listing {}", current.display()),
                    source,
                }
            }
        })?;
        let mut differently_cased = None;
        let mut found = false;
        for entry in entries {
            let entry = entry.map_err(|source| CookError::Io {
                action: format!("listing {}", current.display()),
                source,
            })?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == part {
                found = true;
                break;
            }
            if name.to_lowercase() == part.to_lowercase() {
                differently_cased = Some(name.into_owned());
            }
        }
        if !found {
            return Err(match differently_cased {
                Some(actual) => CookError::CaseMismatch {
                    recipe: recipe.to_owned(),
                    path: path.to_string(),
                    actual,
                },
                None => missing(path, recipe),
            });
        }
        current.push(part);
    }
    Ok(current)
}

fn missing(path: &WorkspacePath, recipe: &Path) -> CookError {
    CookError::MissingInput {
        recipe: recipe.to_owned(),
        path: path.to_string(),
    }
}

/// An absolute path in a comparable form: canonical, without Windows' `\\?\` prefix, with `/`
/// separators.
///
/// # Errors
///
/// [`CookError::Io`] if it can't be canonicalized (it doesn't exist, for example).
pub(crate) fn normalize(path: &Path) -> Result<String, CookError> {
    let canonical = std::fs::canonicalize(path).map_err(|source| CookError::Io {
        action: format!("resolving {}", path.display()),
        source,
    })?;
    Ok(normalize_text(&canonical.to_string_lossy()))
}

/// [`normalize`]'s text rules, without touching the file system.
fn normalize_text(path: &str) -> String {
    let stripped = if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        path.to_owned()
    };
    let slashed = stripped.replace('\\', "/");
    slashed.trim_end_matches('/').to_owned()
}

/// Where a dependency `slangc` reported lies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Location {
    /// In the workspace, at this workspace-relative path.
    Workspace(String),
    /// In the compiler's own installation (its core modules), which the Slang version covers.
    Compiler,
    /// Anywhere else.
    Outside,
}

/// Classifies normalized paths against the normalized workspace and compiler roots.
#[derive(Debug, Clone)]
pub(crate) struct DependencyRoots {
    /// The workspace root, normalized.
    pub(crate) workspace: String,
    /// The compiler's install root, normalized.
    pub(crate) compiler: String,
    /// Whether paths compare without case, as on Windows.
    pub(crate) ignore_case: bool,
}

impl DependencyRoots {
    /// Classifies `path`, already normalized. The compiler's root is checked first: the pinned
    /// Slang is installed inside the workspace, in `.tools/`.
    #[must_use]
    pub(crate) fn locate(&self, path: &str) -> Location {
        if relative_to(&self.compiler, path, self.ignore_case).is_some() {
            Location::Compiler
        } else if let Some(relative) = relative_to(&self.workspace, path, self.ignore_case) {
            Location::Workspace(relative)
        } else {
            Location::Outside
        }
    }

    /// Checks that the compiler's installation can't be mistaken for workspace sources. Its
    /// files are exempt from the declared-input check, so an installation that contains the
    /// workspace (`OXREAL_SLANGC=~/bin/slangc`, installed in `~`) would exempt every workspace
    /// file. Inside the workspace it must be under `.tools/`, where the pinned Slang lives.
    ///
    /// # Errors
    ///
    /// [`CookError::CompilerOverlapsWorkspace`].
    pub(crate) fn check(&self) -> Result<(), CookError> {
        // A compiler installed inside the workspace may only be under `.tools/`, the
        // git-ignored tool directory that holds no sources. Anywhere else (for example
        // `OXREAL_SLANGC=<workspace>/crates/foo/bin/slangc`) would exempt that crate's shaders.
        let tools = format!("{}/.tools", self.workspace);
        let around = contains(&self.compiler, &self.workspace, self.ignore_case);
        let among_sources = contains(&self.workspace, &self.compiler, self.ignore_case)
            && !contains(&tools, &self.compiler, self.ignore_case);
        if around || among_sources {
            Err(CookError::CompilerOverlapsWorkspace {
                compiler_root: PathBuf::from(&self.compiler),
            })
        } else {
            Ok(())
        }
    }

    /// Whether `relative` (from [`Location::Workspace`]) is `declared`.
    #[must_use]
    pub(crate) fn matches(&self, relative: &str, declared: &WorkspacePath) -> bool {
        if self.ignore_case {
            relative.to_lowercase() == declared.as_str().to_lowercase()
        } else {
            relative == declared.as_str()
        }
    }
}

/// `path` relative to `root`, both normalized, or `None` if it isn't below it.
fn relative_to(root: &str, path: &str, ignore_case: bool) -> Option<String> {
    if root.is_empty() {
        return None;
    }
    // `get` returns `None` where `root.len()` falls inside a multi-byte character of `path`, so
    // non-ASCII paths can't panic here.
    let head = path.get(..root.len())?;
    let rest = path.get(root.len()..)?.strip_prefix('/')?;
    let same = if ignore_case {
        head.to_lowercase() == root.to_lowercase()
    } else {
        head == root
    };
    (same && !rest.is_empty()).then(|| rest.to_owned())
}

/// Whether `inner` is `outer` or below it, both normalized.
pub(crate) fn contains(outer: &str, inner: &str, ignore_case: bool) -> bool {
    let equal = if ignore_case {
        outer.to_lowercase() == inner.to_lowercase()
    } else {
        outer == inner
    };
    equal || relative_to(outer, inner, ignore_case).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_verbatim_prefixes_and_uses_forward_slashes() {
        assert_eq!(
            normalize_text(r"\\?\C:\Dev\Oxreal\crates\a.slang"),
            "C:/Dev/Oxreal/crates/a.slang"
        );
        assert_eq!(
            normalize_text(r"\\?\UNC\server\share\ws\a.slang"),
            "//server/share/ws/a.slang"
        );
        assert_eq!(normalize_text(r"C:\Dev\Oxreal\"), "C:/Dev/Oxreal");
        assert_eq!(
            normalize_text("/home/dev/oxreal/a.slang"),
            "/home/dev/oxreal/a.slang"
        );
    }

    fn windows() -> DependencyRoots {
        DependencyRoots {
            workspace: "E:/Development/Oxreal".to_owned(),
            compiler: "E:/Development/Oxreal/.tools/slang/2026.22".to_owned(),
            ignore_case: true,
        }
    }

    fn linux() -> DependencyRoots {
        DependencyRoots {
            workspace: "/home/dev/oxreal".to_owned(),
            compiler: "/opt/slang".to_owned(),
            ignore_case: false,
        }
    }

    #[test]
    fn windows_paths_compare_without_case() {
        let roots = windows();
        assert_eq!(
            roots.locate("e:/development/oxreal/crates/rhi/shaders/Bindless.slang"),
            Location::Workspace("crates/rhi/shaders/Bindless.slang".to_owned())
        );
        let declared = WorkspacePath::new("crates/rhi/shaders/Bindless.slang").unwrap();
        assert!(roots.matches("Crates/RHI/shaders/bindless.slang", &declared));
        assert_eq!(roots.locate("C:/Windows/x.slang"), Location::Outside);
        // A sibling directory that shares a prefix isn't inside.
        assert_eq!(
            roots.locate("E:/Development/OxrealOld/a.slang"),
            Location::Outside
        );
    }

    #[test]
    fn unix_paths_compare_with_case() {
        let roots = linux();
        assert_eq!(
            roots.locate("/home/dev/oxreal/crates/a.slang"),
            Location::Workspace("crates/a.slang".to_owned())
        );
        assert_eq!(
            roots.locate("/home/dev/Oxreal/crates/a.slang"),
            Location::Outside
        );
        let declared = WorkspacePath::new("crates/a.slang").unwrap();
        assert!(roots.matches("crates/a.slang", &declared));
        assert!(!roots.matches("crates/A.slang", &declared));
    }

    #[test]
    fn non_ascii_paths_never_panic() {
        // Byte 13 of the path falls inside `é`: comparing must not split it.
        assert_eq!(
            relative_to("/home/dev/abc", "/home/dev/éé/x.slang", false),
            None
        );
        assert_eq!(
            relative_to("C:/Users/Jürgen", "C:/Users/Jürgen/ws/a.slang", true),
            Some("ws/a.slang".to_owned())
        );
        assert_eq!(
            windows().locate("E:/Développement/a.slang"),
            Location::Outside
        );
    }

    #[test]
    fn a_compiler_overlapping_the_workspace_sources_is_refused() {
        // OXREAL_SLANGC=/home/dev/bin/slangc: the installation directory is /home/dev.
        let around = DependencyRoots {
            workspace: "/home/dev/oxreal".to_owned(),
            compiler: "/home/dev".to_owned(),
            ignore_case: false,
        };
        assert!(matches!(
            around.check(),
            Err(CookError::CompilerOverlapsWorkspace { .. })
        ));
        let same = DependencyRoots {
            compiler: "/home/dev/oxreal".to_owned(),
            ..around.clone()
        };
        assert!(same.check().is_err());
        // Inside the workspace, but among sources: a crate's shaders would be exempt.
        let inside = DependencyRoots {
            compiler: "/home/dev/oxreal/crates/foo".to_owned(),
            ..around.clone()
        };
        assert!(inside.check().is_err());
        let tools_lookalike = DependencyRoots {
            compiler: "/home/dev/oxreal/.toolsmith/slang".to_owned(),
            ..around.clone()
        };
        assert!(tools_lookalike.check().is_err());
        // The pinned Slang inside the workspace, and a separate installation, are fine.
        assert!(windows().check().is_ok());
        assert!(linux().check().is_ok());
    }

    #[test]
    fn containment_includes_the_directory_itself() {
        assert!(contains("/home/dev", "/home/dev", false));
        assert!(contains("/home/dev", "/home/dev/oxreal", false));
        assert!(!contains("/home/dev/oxreal", "/home/dev", false));
        assert!(contains("C:/Users/Dev", "c:/users/dev/oxreal", true));
        assert!(!contains("/home/dev", "/home/developer", false));
    }

    #[test]
    fn the_compiler_installation_is_recognized_even_inside_the_workspace() {
        // The pinned Slang is installed in the workspace's .tools directory. Its core modules
        // are covered by the Slang version, not by declared inputs.
        let core =
            "E:/Development/Oxreal/.tools/slang/2026.22/bin/slang-standard-module/core.slang";
        assert_eq!(windows().locate(core), Location::Compiler);
        assert_eq!(
            linux().locate("/opt/slang/lib/core.slang"),
            Location::Compiler
        );
    }

    #[test]
    fn resolving_requires_exact_case() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cook");
        let recipe = Path::new("cook.json");
        let exact = WorkspacePath::new("shaders/Tint.slang").unwrap();
        assert!(resolve_exact(&root, &exact, recipe).unwrap().is_file());

        let wrong = WorkspacePath::new("shaders/tint.slang").unwrap();
        match resolve_exact(&root, &wrong, recipe).unwrap_err() {
            CookError::CaseMismatch { actual, .. } => assert_eq!(actual, "Tint.slang"),
            other => panic!("expected CaseMismatch, got {other}"),
        }
        let absent = WorkspacePath::new("shaders/Missing.slang").unwrap();
        assert!(matches!(
            resolve_exact(&root, &absent, recipe).unwrap_err(),
            CookError::MissingInput { .. }
        ));
        let absent_dir = WorkspacePath::new("nowhere/x.slang").unwrap();
        assert!(matches!(
            resolve_exact(&root, &absent_dir, recipe).unwrap_err(),
            CookError::MissingInput { .. }
        ));
    }
}
