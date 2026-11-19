//! Application-side log setup (`application` feature, ADR-0010 §2).
//!
//! Only applications and tools call [`init_application_logging`]. Libraries never install a
//! global subscriber.

use std::io::IsTerminal as _;

use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::util::SubscriberInitExt as _;

/// Environment variable holding the log filter, in `RUST_LOG` syntax, for example
/// `info,oxreal_rhi_vulkan=debug`. If it isn't set, `RUST_LOG` is used, and then the default
/// level `info`.
pub const LOG_ENV_VAR: &str = "OXREAL_LOG";
const FALLBACK_ENV_VAR: &str = "RUST_LOG";

/// Why application logging could not be initialized.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum InitError {
    /// The filter in an environment variable isn't valid `RUST_LOG` syntax.
    #[error(
        "invalid log filter {variable}={value:?}: {source} \
         (expected RUST_LOG syntax, for example `info,oxreal_ecs=debug`)"
    )]
    InvalidFilter {
        /// The variable the filter was read from.
        variable: &'static str,
        /// The rejected filter text.
        value: String,
        /// The parser's explanation.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// A global subscriber or `log` logger is already installed in this process.
    #[error(
        "logging is already initialized in this process; call init_application_logging once, \
         at application startup: {source}"
    )]
    AlreadyInitialized {
        /// The underlying error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Installs Oxreal's global log subscriber. Call it once, early in `main`.
///
/// - Writes human-readable lines to **stderr**, colored when stderr is a terminal.
/// - Filters by [`LOG_ENV_VAR`], then `RUST_LOG`, then the default level `info`.
/// - Forwards records from the `log` crate (used by some dependencies) into the same output.
///
/// # Errors
///
/// [`InitError::InvalidFilter`] if a filter variable is set but malformed, and
/// [`InitError::AlreadyInitialized`] if logging was already set up in this process.
pub fn init_application_logging() -> Result<(), InitError> {
    let spec = select_filter_spec(
        std::env::var(LOG_ENV_VAR).ok(),
        std::env::var(FALLBACK_ENV_VAR).ok(),
    );
    let filter = build_filter(spec)?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_target(true)
        .finish()
        .try_init()
        .map_err(|e| InitError::AlreadyInitialized {
            source: Box::new(e),
        })?;

    #[cfg(feature = "profile-tracy")]
    tracing::info!("profiling enabled: Tracy client (on-demand, localhost only)");
    Ok(())
}

/// Picks the filter source: `OXREAL_LOG` wins over `RUST_LOG`. Empty values count as unset.
fn select_filter_spec(
    oxreal_log: Option<String>,
    rust_log: Option<String>,
) -> Option<(&'static str, String)> {
    let set = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    set(oxreal_log)
        .map(|v| (LOG_ENV_VAR, v))
        .or_else(|| set(rust_log).map(|v| (FALLBACK_ENV_VAR, v)))
}

fn build_filter(spec: Option<(&'static str, String)>) -> Result<EnvFilter, InitError> {
    let builder = EnvFilter::builder().with_default_directive(LevelFilter::INFO.into());
    match spec {
        None => Ok(builder.parse_lossy("")),
        Some((variable, value)) => builder.parse(&value).map_err(|e| InitError::InvalidFilter {
            variable,
            source: Box::new(e),
            value,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oxreal_log_takes_precedence_over_rust_log() {
        let picked = select_filter_spec(Some("debug".into()), Some("warn".into()));
        assert_eq!(picked, Some((LOG_ENV_VAR, "debug".to_owned())));
    }

    #[test]
    fn rust_log_is_the_fallback_and_empty_values_are_unset() {
        assert_eq!(
            select_filter_spec(None, Some("warn".into())),
            Some((FALLBACK_ENV_VAR, "warn".to_owned()))
        );
        assert_eq!(
            select_filter_spec(Some("  ".into()), Some("warn".into())),
            Some((FALLBACK_ENV_VAR, "warn".to_owned()))
        );
        assert_eq!(select_filter_spec(None, None), None);
    }

    #[test]
    fn valid_filters_build() {
        assert!(build_filter(None).is_ok());
        assert!(build_filter(Some((LOG_ENV_VAR, "info,oxreal_ecs=trace".into()))).is_ok());
    }

    #[test]
    fn invalid_filter_names_the_variable_and_value() {
        let err = build_filter(Some((LOG_ENV_VAR, "oxreal_ecs=loud".into())))
            .expect_err("`loud` is not a level");
        let message = err.to_string();
        assert!(message.contains("OXREAL_LOG"), "{message}");
        assert!(message.contains("oxreal_ecs=loud"), "{message}");
        assert!(std::error::Error::source(&err).is_some());
    }
}
