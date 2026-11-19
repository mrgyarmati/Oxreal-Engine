//! Engine-owned profiling macros (ADR-0010 §3).
//!
//! Each macro has two complete definitions, selected by **this** crate's `profile-tracy`
//! feature. A `#[cfg(feature = ...)]` written *inside* a `macro_rules!` body would be evaluated
//! in the calling crate, so it would test the caller's features instead of ours.

/// Times the rest of the enclosing block under `name`, which must be a string literal.
///
/// Use it in statement position. The measurement ends when the enclosing block ends. Without
/// the `profile-tracy` feature it compiles to nothing.
///
/// ```
/// fn build_frame_graph() {
///     oxreal_diagnostics::profile_scope!("build_frame_graph");
///     // ... measured ...
/// }
/// # build_frame_graph();
/// ```
#[cfg(feature = "profile-tracy")]
#[macro_export]
macro_rules! profile_scope {
    ($name:literal) => {
        let _oxreal_profile_scope =
            $crate::__private::scope($crate::__private::tracy_client::span_location!($name));
    };
}

/// Times the rest of the enclosing block under `name`, which must be a string literal.
///
/// Use it in statement position. The measurement ends when the enclosing block ends. Without
/// the `profile-tracy` feature it compiles to nothing.
///
/// ```
/// fn build_frame_graph() {
///     oxreal_diagnostics::profile_scope!("build_frame_graph");
///     // ... measured ...
/// }
/// # build_frame_graph();
/// ```
#[cfg(not(feature = "profile-tracy"))]
#[macro_export]
macro_rules! profile_scope {
    ($name:literal) => {};
}

/// Times the rest of the enclosing function, named after the function's path.
///
/// Use it as the first statement of the function. Without the `profile-tracy` feature it
/// compiles to nothing.
///
/// ```
/// fn propagate_transforms() {
///     oxreal_diagnostics::profile_function!();
///     // ... measured ...
/// }
/// # propagate_transforms();
/// ```
#[cfg(feature = "profile-tracy")]
#[macro_export]
macro_rules! profile_function {
    () => {
        let _oxreal_profile_function =
            $crate::__private::scope($crate::__private::tracy_client::span_location!());
    };
}

/// Times the rest of the enclosing function, named after the function's path.
///
/// Use it as the first statement of the function. Without the `profile-tracy` feature it
/// compiles to nothing.
///
/// ```
/// fn propagate_transforms() {
///     oxreal_diagnostics::profile_function!();
///     // ... measured ...
/// }
/// # propagate_transforms();
/// ```
#[cfg(not(feature = "profile-tracy"))]
#[macro_export]
macro_rules! profile_function {
    () => {};
}

/// Marks the end of a frame for the profiler. Call it once per presented frame.
///
/// Does nothing without the `profile-tracy` feature, or when the profiler client isn't running.
#[inline]
pub fn frame_mark() {
    #[cfg(feature = "profile-tracy")]
    tracy::frame_mark();
}

#[cfg(feature = "profile-tracy")]
pub(crate) mod tracy {
    use tracy_client::{Client, Span, SpanLocation};

    /// Begins a profiling zone if the Tracy client is running.
    ///
    /// Returns `None` instead of panicking when there is no client. `tracy_client::span!`
    /// panics in that case, and a library can't guarantee the application started one.
    #[inline]
    #[must_use = "the zone ends when the returned guard is dropped"]
    pub fn scope(location: &'static SpanLocation) -> Option<Span> {
        Client::running().map(|client| client.span(location, 0))
    }

    /// Ends a frame if the Tracy client is running. Never panics.
    #[inline]
    pub(crate) fn frame_mark() {
        if let Some(client) = Client::running() {
            client.frame_mark();
        }
    }
}

#[cfg(test)]
mod tests {
    // These tests run with and without `profile-tracy` (CI exercises both configurations):
    // the macros must compile in statement position, nest, and never panic.

    fn instrumented(depth: u32) -> u32 {
        crate::profile_function!();
        crate::profile_scope!("instrumented.body");
        if depth == 0 {
            0
        } else {
            crate::profile_scope!("instrumented.recurse");
            1 + instrumented(depth - 1)
        }
    }

    #[test]
    fn macros_compile_nest_and_do_not_change_behaviour() {
        assert_eq!(instrumented(3), 3);
    }

    #[test]
    fn frame_mark_never_panics() {
        for _ in 0..3 {
            crate::frame_mark();
        }
    }
}
