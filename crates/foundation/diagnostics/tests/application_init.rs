//! Global-state behaviour of `init_application_logging`.
//!
//! This is a separate test binary, so it runs in its own process with no subscriber installed
//! by other tests.

#![cfg(feature = "application")]

#[test]
fn initializes_once_and_reports_a_second_attempt() {
    oxreal_diagnostics::init_application_logging().expect("first initialization succeeds");
    oxreal_diagnostics::info!(test = "application_init", "logging initialized");

    let second = oxreal_diagnostics::init_application_logging();
    let err = second.expect_err("a second initialization must fail, not replace the logger");
    assert!(
        matches!(
            err,
            oxreal_diagnostics::InitError::AlreadyInitialized { .. }
        ),
        "unexpected error: {err}"
    );
    assert!(err.to_string().contains("already initialized"), "{err}");
}
