//! Display-server selection (ADR-0015 §5).
//!
//! The decision logic is pure and platform-independent, so it's tested on every OS. Only
//! [`crate::EventLoop::new`] applies it, and only on Linux.

use crate::WindowError;

/// Environment variable that forces a Linux display server: `wayland` or `x11`.
pub const DISPLAY_OVERRIDE_ENV_VAR: &str = "OXREAL_LINUX_DISPLAY";

/// The windowing system an event loop is connected to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DisplayServer {
    /// Win32 on Windows.
    Windows,
    /// Wayland on Linux.
    Wayland,
    /// X11 on Linux (including XWayland).
    X11,
    /// Any other windowing system.
    Other,
}

/// What Linux event-loop creation should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    all(not(target_os = "linux"), not(test)),
    expect(
        dead_code,
        reason = "used only on Linux; kept platform-independent so its tests run on every OS"
    )
)]
pub(crate) enum LinuxBackendChoice {
    /// Let winit choose (Wayland when available, else X11).
    Automatic,
    /// Force this backend.
    Forced(DisplayServer),
}

/// Decides the Linux backend from the override and the session's environment.
///
/// Fails early with an actionable error instead of letting the platform layer fail obscurely.
#[cfg_attr(
    all(not(target_os = "linux"), not(test)),
    expect(
        dead_code,
        reason = "used only on Linux; kept platform-independent so its tests run on every OS"
    )
)]
pub(crate) fn choose_linux_backend(
    override_value: Option<&str>,
    wayland_display: Option<&str>,
    x11_display: Option<&str>,
) -> Result<LinuxBackendChoice, WindowError> {
    let set = |v: Option<&str>| v.is_some_and(|s| !s.trim().is_empty());
    let (has_wayland, has_x11) = (set(wayland_display), set(x11_display));

    match override_value.map(str::trim).filter(|v| !v.is_empty()) {
        None if has_wayland || has_x11 => Ok(LinuxBackendChoice::Automatic),
        None => Err(WindowError::NoDisplay),
        Some(v) if v.eq_ignore_ascii_case("wayland") => {
            if has_wayland {
                Ok(LinuxBackendChoice::Forced(DisplayServer::Wayland))
            } else {
                Err(WindowError::DisplayUnavailable {
                    requested: DisplayServer::Wayland,
                    variable: "WAYLAND_DISPLAY",
                })
            }
        }
        Some(v) if v.eq_ignore_ascii_case("x11") => {
            if has_x11 {
                Ok(LinuxBackendChoice::Forced(DisplayServer::X11))
            } else {
                Err(WindowError::DisplayUnavailable {
                    requested: DisplayServer::X11,
                    variable: "DISPLAY",
                })
            }
        }
        Some(v) => Err(WindowError::InvalidDisplayOverride {
            value: v.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_when_any_display_is_available() {
        for (wayland, x11) in [
            (Some("wayland-0"), None),
            (None, Some(":0")),
            (Some("w"), Some(":0")),
        ] {
            assert_eq!(
                choose_linux_backend(None, wayland, x11).unwrap(),
                LinuxBackendChoice::Automatic
            );
        }
    }

    #[test]
    fn no_display_is_an_actionable_error() {
        let err = choose_linux_backend(None, None, Some("  ")).unwrap_err();
        assert!(matches!(err, WindowError::NoDisplay));
        let message = err.to_string();
        assert!(
            message.contains("WAYLAND_DISPLAY") && message.contains("DISPLAY"),
            "{message}"
        );
    }

    #[test]
    fn override_forces_an_available_backend_case_insensitively() {
        assert_eq!(
            choose_linux_backend(Some("X11"), Some("wayland-0"), Some(":0")).unwrap(),
            LinuxBackendChoice::Forced(DisplayServer::X11)
        );
        assert_eq!(
            choose_linux_backend(Some(" wayland "), Some("wayland-0"), Some(":0")).unwrap(),
            LinuxBackendChoice::Forced(DisplayServer::Wayland)
        );
    }

    #[test]
    fn override_of_a_missing_backend_names_its_variable() {
        let err = choose_linux_backend(Some("wayland"), None, Some(":0")).unwrap_err();
        assert!(err.to_string().contains("WAYLAND_DISPLAY"), "{err}");
        let err = choose_linux_backend(Some("x11"), Some("wayland-0"), None).unwrap_err();
        assert!(matches!(
            err,
            WindowError::DisplayUnavailable {
                requested: DisplayServer::X11,
                ..
            }
        ));
    }

    #[test]
    fn unknown_override_is_rejected_and_empty_means_unset() {
        let err = choose_linux_backend(Some("mir"), Some("w"), Some(":0")).unwrap_err();
        assert!(err.to_string().contains("\"mir\""), "{err}");
        assert_eq!(
            choose_linux_backend(Some(""), None, Some(":0")).unwrap(),
            LinuxBackendChoice::Automatic
        );
    }
}
