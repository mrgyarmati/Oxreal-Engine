//! Where the player finds its cooked data (ADR-0019 §4).
//!
//! The cooked root is the directory that contains `shaders/` (what `cargo xtask cook-shaders`
//! writes). In order:
//!
//! 1. `--cooked <dir>`, if given;
//! 2. otherwise the [`COOKED_DIR_ENV_VAR`] environment variable, if set and not empty;
//! 3. otherwise `cooked/` next to the executable. `cargo xtask cook-shaders` writes
//!    `target/<profile>/cooked/`, beside `target/<profile>/oxreal-player`, so a cooked build
//!    runs as it is.
//!
//! The overrides work in every build. They sit at the trust level of the command line and the
//! environment, like the executable itself (ADR-0019 §4, "The trust argument").

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

/// Environment variable naming the cooked root, when `--cooked` isn't given.
pub(crate) const COOKED_DIR_ENV_VAR: &str = "OXREAL_COOKED_DIR";

/// The cooked root's name next to the executable.
const COOKED_DIR_NAME: &str = "cooked";

/// Where the cooked root came from, for messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CookedRootSource {
    /// `--cooked`.
    Argument,
    /// [`COOKED_DIR_ENV_VAR`].
    Environment,
    /// Next to the executable.
    Executable,
}

impl fmt::Display for CookedRootSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Argument => write!(f, "--cooked"),
            Self::Environment => write!(f, "{COOKED_DIR_ENV_VAR}"),
            Self::Executable => write!(f, "next to the executable"),
        }
    }
}

/// The cooked root couldn't be determined.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CookedRootError {
    /// The executable's own path is unknown, so `cooked/` can't be found next to it.
    #[error(
        "can't find the player's own path, to look for `{COOKED_DIR_NAME}/` next to it; \
         pass `--cooked <dir>` or set {COOKED_DIR_ENV_VAR}"
    )]
    ExecutableUnknown {
        /// Why `std::env::current_exe` failed.
        #[source]
        source: std::io::Error,
    },
    /// The executable's path has no parent directory.
    #[error(
        "the player's path {} has no parent directory to look for `{COOKED_DIR_NAME}/` in; \
         pass `--cooked <dir>` or set {COOKED_DIR_ENV_VAR}",
        path.display()
    )]
    ExecutableHasNoDirectory {
        /// The executable's path.
        path: PathBuf,
    },
}

/// Resolves the cooked root from the `--cooked` `argument`, the [`COOKED_DIR_ENV_VAR`] value in
/// `environment`, and `executable`, which returns the player's own path (normally
/// [`std::env::current_exe`]). `executable` is only called when it's needed.
///
/// An empty environment variable counts as unset, as for Oxreal's other variables. Relative
/// paths are kept relative, so they mean what they meant where the player was started.
///
/// # Errors
///
/// [`CookedRootError`] when the fallback next to the executable is needed and impossible.
pub(crate) fn resolve_cooked_root(
    argument: Option<&Path>,
    environment: Option<&OsStr>,
    executable: impl FnOnce() -> std::io::Result<PathBuf>,
) -> Result<(PathBuf, CookedRootSource), CookedRootError> {
    if let Some(dir) = argument {
        return Ok((dir.to_path_buf(), CookedRootSource::Argument));
    }
    if let Some(dir) = environment.filter(|value| !value.is_empty()) {
        return Ok((PathBuf::from(dir), CookedRootSource::Environment));
    }
    let path = executable().map_err(|source| CookedRootError::ExecutableUnknown { source })?;
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => {
            Ok((dir.join(COOKED_DIR_NAME), CookedRootSource::Executable))
        }
        _ => Err(CookedRootError::ExecutableHasNoDirectory { path }),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn never() -> std::io::Result<PathBuf> {
        panic!("the executable's path was looked up although it wasn't needed")
    }

    #[test]
    fn the_argument_wins_over_everything() {
        let env = OsString::from("from-env");
        let resolved = resolve_cooked_root(Some(Path::new("from-arg")), Some(&env), never).unwrap();
        assert_eq!(
            resolved,
            (PathBuf::from("from-arg"), CookedRootSource::Argument)
        );
    }

    #[test]
    fn the_environment_wins_over_the_executable() {
        let env = OsString::from("from-env");
        let resolved = resolve_cooked_root(None, Some(&env), never).unwrap();
        assert_eq!(
            resolved,
            (PathBuf::from("from-env"), CookedRootSource::Environment)
        );
    }

    #[test]
    fn an_empty_environment_variable_counts_as_unset() {
        let env = OsString::new();
        let exe = || Ok(Path::new("game").join("player"));
        let resolved = resolve_cooked_root(None, Some(&env), exe).unwrap();
        assert_eq!(
            resolved,
            (
                Path::new("game").join("cooked"),
                CookedRootSource::Executable
            )
        );
    }

    #[test]
    fn the_fallback_is_next_to_the_executable() {
        let exe = || Ok(Path::new("target").join("debug").join("oxreal-player"));
        let resolved = resolve_cooked_root(None, None, exe).unwrap();
        assert_eq!(
            resolved,
            (
                Path::new("target").join("debug").join("cooked"),
                CookedRootSource::Executable
            )
        );
    }

    #[test]
    fn an_unknown_executable_path_is_a_typed_error() {
        let exe = || Err(std::io::Error::other("no /proc"));
        let error = resolve_cooked_root(None, None, exe).unwrap_err();
        assert!(matches!(error, CookedRootError::ExecutableUnknown { .. }));
        assert!(error.to_string().contains("--cooked"), "{error}");
    }

    #[test]
    fn an_executable_without_a_directory_is_a_typed_error() {
        for path in ["", "oxreal-player"] {
            let exe = || Ok(PathBuf::from(path));
            let error = resolve_cooked_root(None, None, exe).unwrap_err();
            assert!(
                matches!(error, CookedRootError::ExecutableHasNoDirectory { .. }),
                "{path:?}: {error}"
            );
        }
    }

    // Path syntax is the platform's: these two tests check real executable paths of each OS on
    // that OS (ADR-0019 §5), so they're the only platform-specific code here.
    #[cfg(windows)]
    #[test]
    fn windows_paths() {
        for (exe, cooked) in [
            (
                r"C:\Games\Oxreal\oxreal-player.exe",
                r"C:\Games\Oxreal\cooked",
            ),
            (
                r"\\server\share\game\oxreal-player.exe",
                r"\\server\share\game\cooked",
            ),
            (r"\\?\C:\Games\oxreal-player.exe", r"\\?\C:\Games\cooked"),
            (r"D:\oxreal-player.exe", r"D:\cooked"),
        ] {
            let resolved = resolve_cooked_root(None, None, || Ok(PathBuf::from(exe))).unwrap();
            assert_eq!(resolved.0, PathBuf::from(cooked), "{exe}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_paths() {
        for (exe, cooked) in [
            ("/opt/oxreal/oxreal-player", "/opt/oxreal/cooked"),
            (
                "/home/dev/Oxreal Engine/target/debug/oxreal-player",
                "/home/dev/Oxreal Engine/target/debug/cooked",
            ),
            ("/oxreal-player", "/cooked"),
        ] {
            let resolved = resolve_cooked_root(None, None, || Ok(PathBuf::from(exe))).unwrap();
            assert_eq!(resolved.0, PathBuf::from(cooked), "{exe}");
        }
    }
}
