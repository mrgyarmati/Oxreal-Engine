- `oxreal-diagnostics`, the first engine crate. It provides logging macros (`info!`, `warn!` and
  so on) for all engine and gameplay code, plus the `profile_function!`, `profile_scope!` and
  `frame_mark` profiling hooks, which cost nothing unless the `profile-tracy` feature is enabled.
- Applications set up logging with `init_application_logging()` (feature `application`),
  filtered by the `OXREAL_LOG` environment variable (`RUST_LOG` syntax).
