//! Logging and profiling for Oxreal ([ADR-0010]).
//!
//! **Layer:** foundation. Every engine crate, gameplay module and plugin uses this crate for
//! diagnostics. They never depend on `tracing` or a profiler directly, so the backends can be
//! swapped without touching call sites.
//!
//! # Logging
//!
//! Use the re-exported macros with structured fields:
//!
//! ```
//! use oxreal_diagnostics::{info, warn};
//!
//! let (width, height) = (2560, 1440);
//! info!(width, height, "swapchain created");
//! warn!(frame = 42, "frame took longer than its budget");
//! ```
//!
//! Libraries only emit diagnostics. Applications install the subscriber once, with the
//! `application` feature (see `init_application_logging`).
//!
//! # Profiling
//!
//! [`profile_function!`] and [`profile_scope!`] time the rest of the enclosing block, and
//! [`frame_mark`] ends a frame. Without the `profile-tracy` feature they compile to nothing.
//!
//! ```
//! fn update_transforms() {
//!     oxreal_diagnostics::profile_function!();
//!     {
//!         oxreal_diagnostics::profile_scope!("propagate");
//!         // ... work measured as "propagate" ...
//!     }
//! }
//! # update_transforms();
//! oxreal_diagnostics::frame_mark();
//! ```
//!
//! # Features
//!
//! - `application`: installs the global log subscriber. **Only** `app-*` and `tool` crates may
//!   enable it.
//! - `profile-tracy`: enables the Tracy backend (on-demand, localhost only, no broadcast). It's
//!   opt-in for profiling builds and never on in shipped player builds by default.
//!
//! [ADR-0010]: https://github.com/mrgyarmati/Oxreal-Engine/blob/main/docs/decisions/0010-errors-logging-and-profiling.md

pub use tracing::{
    Level, debug, debug_span, error, error_span, info, info_span, trace, trace_span, warn,
    warn_span,
};

mod profiling;
pub use profiling::frame_mark;

#[cfg(feature = "application")]
mod application;
#[cfg(feature = "application")]
pub use application::{
    InitError, LOG_ENV_VAR, error_chain, init_application_logging, run_application,
};

/// Implementation details of the exported macros. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    #[cfg(feature = "profile-tracy")]
    pub use crate::profiling::tracy::scope;
    #[cfg(feature = "profile-tracy")]
    pub use tracy_client;
}
