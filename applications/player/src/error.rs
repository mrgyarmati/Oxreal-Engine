//! The player's errors. Each keeps its cause as a `#[source]`, so the message printed at exit
//! (`oxreal_diagnostics::run_application`) shows the whole chain (ADR-0010).

use std::path::PathBuf;
use std::time::Duration;

use oxreal_ecs::ScheduleError;
use oxreal_renderer::{RenderError, ShaderCacheError};
use oxreal_rhi::RhiError;
use oxreal_window::WindowError;

use crate::cooked::{CookedRootError, CookedRootSource};
use crate::options::{OptionsError, USAGE};

/// Why the player stopped, or couldn't start.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PlayerError {
    /// The command line is wrong.
    #[error("{source}\n\n{USAGE}")]
    Options {
        /// What's wrong with it.
        #[source]
        source: OptionsError,
    },
    /// The cooked root couldn't be determined.
    #[error(transparent)]
    CookedRoot(#[from] CookedRootError),
    /// The cooked shaders couldn't be loaded.
    #[error("the cooked shaders in {} (from {from}) can't be used", root.display())]
    CookedShaders {
        /// The cooked root.
        root: PathBuf,
        /// Where the root came from.
        from: CookedRootSource,
        /// What's missing, stale or damaged.
        #[source]
        source: ShaderCacheError,
    },
    /// The device, a surface or a frame failed.
    #[error("{action} failed")]
    Device {
        /// What was being done.
        action: &'static str,
        /// The device's error.
        #[source]
        source: RhiError,
    },
    /// The renderer failed.
    #[error("{action} failed")]
    Render {
        /// What was being done.
        action: &'static str,
        /// The renderer's error.
        #[source]
        source: RenderError,
    },
    /// The scene's systems couldn't run.
    #[error("the scene's schedule failed")]
    Schedule(#[from] ScheduleError),
    /// The window or the event loop failed.
    #[error(transparent)]
    Window(#[from] WindowError),
    /// A resource the scene must have is missing: a bug in the scene's setup.
    #[error("the {0} resource is missing from the scene")]
    MissingResource(&'static str),
    /// `--frames` frames didn't arrive in time.
    #[error("timed out after {frames} frame(s) in {timeout:?}")]
    TimedOut {
        /// Frames that did arrive.
        frames: u64,
        /// The time allowed.
        timeout: Duration,
    },
    /// The window was closed before `--frames` frames.
    #[error("the window closed after {frames} of {target} frames")]
    ClosedEarly {
        /// Frames shown.
        frames: u64,
        /// Frames asked for.
        target: u64,
    },
    /// Vulkan validation reported errors (each is in the log).
    #[error("Vulkan validation reported {0} error(s); see the log")]
    Validation(u64),
}

impl PlayerError {
    /// A device error while doing `action`.
    pub(crate) fn device(action: &'static str) -> impl FnOnce(RhiError) -> Self {
        move |source| Self::Device { action, source }
    }

    /// A renderer error while doing `action`.
    pub(crate) fn render(action: &'static str) -> impl FnOnce(RenderError) -> Self {
        move |source| Self::Render { action, source }
    }
}
