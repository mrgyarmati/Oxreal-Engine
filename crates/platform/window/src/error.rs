//! Window and event-loop errors.

use crate::DisplayServer;

/// Why a window or the event loop could not be created or run.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WindowError {
    /// No display server is reachable (Linux): neither Wayland nor X11 is configured.
    #[error(
        "no display server found: neither WAYLAND_DISPLAY nor DISPLAY is set. Run from a \
         graphical session, or use a virtual display such as `xvfb-run` for headless runs"
    )]
    NoDisplay,
    /// A forced display server (`OXREAL_LINUX_DISPLAY`) is not available in this session.
    #[error(
        "OXREAL_LINUX_DISPLAY requests {requested:?}, but its environment variable \
         ({variable}) is not set in this session"
    )]
    DisplayUnavailable {
        /// The display server that was requested.
        requested: DisplayServer,
        /// The environment variable that would make it available.
        variable: &'static str,
    },
    /// `OXREAL_LINUX_DISPLAY` holds an unknown value.
    #[error("invalid OXREAL_LINUX_DISPLAY value {value:?}: expected `wayland` or `x11`")]
    InvalidDisplayOverride {
        /// The rejected value.
        value: String,
    },
    /// The platform event loop could not be created or failed while running.
    #[error("platform event loop failed: {source}")]
    EventLoop {
        /// The underlying platform error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The operating system refused to create a window.
    #[error("failed to create window {title:?}: {source}")]
    CreateWindow {
        /// Title of the window that could not be created.
        title: String,
        /// The underlying platform error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The cursor could not be captured or released (ADR-0018 §1).
    #[error("cursor capture failed: {reason}")]
    CursorCapture {
        /// What the platform refused, with its reasons.
        reason: String,
    },
}
