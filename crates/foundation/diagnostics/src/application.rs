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

/// Runs an application's main logic with logging set up and errors reported properly.
///
/// Call it as the whole body of `main`:
///
/// ```no_run
/// fn main() -> std::process::ExitCode {
///     oxreal_diagnostics::run_application(|| {
///         // ... the application ...
///         Ok(())
///     })
/// }
/// ```
///
/// - Initializes logging ([`init_application_logging`]).
/// - Runs `app`. If it returns an error, logs it as a readable message **with its whole cause
///   chain** (`a: b: c`), not as `Debug` output.
/// - Returns the process exit code: success, or failure.
pub fn run_application<F>(app: F) -> std::process::ExitCode
where
    F: FnOnce() -> Result<(), Box<dyn std::error::Error>>,
{
    if let Err(e) = init_application_logging() {
        report_without_logger(&e);
        return std::process::ExitCode::FAILURE;
    }
    match app() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{}", error_chain(e.as_ref()));
            std::process::ExitCode::FAILURE
        }
    }
}

/// Formats an error and its sources as `outer: cause: root cause`.
#[must_use]
pub fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        // Many errors already include their source's text. Don't repeat it.
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = cause.source();
    }
    message
}

#[expect(
    clippy::print_stderr,
    reason = "logging failed to initialize, so stderr is the only channel left to report on"
)]
fn report_without_logger(error: &InitError) {
    eprintln!("error: {}", error_chain(error));
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

#[cfg(test)]
mod chain_tests {
    use super::error_chain;
    use std::fmt;

    #[derive(Debug)]
    struct Layer {
        text: &'static str,
        source: Option<Box<Layer>>,
    }

    impl fmt::Display for Layer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.text)
        }
    }

    impl std::error::Error for Layer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.source.as_deref().map(|s| s as _)
        }
    }

    fn layer(text: &'static str, source: Option<Layer>) -> Layer {
        Layer {
            text,
            source: source.map(Box::new),
        }
    }

    #[test]
    fn joins_the_whole_cause_chain() {
        let error = layer(
            "failed to start",
            Some(layer("no usable GPU", Some(layer("driver too old", None)))),
        );
        assert_eq!(
            error_chain(&error),
            "failed to start: no usable GPU: driver too old"
        );
    }

    #[test]
    fn does_not_repeat_a_cause_already_in_the_message() {
        let error = layer(
            "platform event loop failed: os error 5",
            Some(layer("os error 5", None)),
        );
        assert_eq!(
            error_chain(&error),
            "platform event loop failed: os error 5"
        );
    }
}
