//! The tracing subscriber this program installs, and how loud its
//! dependencies are allowed to be.

use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

/// How loud each dependency is allowed to be, whatever this app's level is.
///
/// The decoders and the graphics stack log per frame and per tile at their own
/// default, which buries everything this program has to say.
const DEPENDENCY_LEVELS: [&str; 13] = [
    "wgpu_core=warn",
    "wgpu_hal=error",
    "naga=warn",
    "winit=warn",
    "ureq=warn",
    "ureq_proto=warn",
    "rustls=warn",
    "jxl_render=warn",
    "jxl_grid=warn",
    "jxl_modular=warn",
    "jxl_bitstream=warn",
    "jxl_frame=warn",
    "jxl_color=warn",
];

/// Initialize the global tracing subscriber.
///
/// `cli_level` is the default log level from the `--log-level` CLI flag.
/// It is ignored when the `RUST_LOG` environment variable is set.
///
/// When `SUNLIT_EARTH_SYNC_LOG` is set in the environment, a synchronous
/// stderr writer is used instead of the non-blocking one. This eliminates
/// pipe buffer congestion and stderr lock contention that cause log messages
/// to arrive late (or not at all) in e2e tests. The trade-off is that log
/// writes block the calling thread, which is acceptable in test mode.
///
/// Returns a `WorkerGuard` that must be kept alive for the duration of the
/// program so that buffered log lines are flushed before exit. In sync mode,
/// no guard is needed and `None` is returned.
pub(crate) fn init_logging(
    cli_level: Option<&str>,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let sync_log = std::env::var("SUNLIT_EARTH_SYNC_LOG").is_ok();

    let base_filter = match cli_level {
        Some(level) => EnvFilter::new(level),
        None => EnvFilter::from_default_env().add_directive(LevelFilter::INFO.into()),
    };

    let env_filter = DEPENDENCY_LEVELS
        .iter()
        .filter_map(|directive| directive.parse().ok())
        .fold(base_filter, EnvFilter::add_directive);

    if sync_log {
        let fmt_layer = fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(true)
            .with_thread_ids(true)
            .with_span_events(FmtSpan::CLOSE);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .init();

        None
    } else {
        let (non_blocking, guard) = tracing_appender::non_blocking(std::io::stderr());

        let fmt_layer = fmt::layer()
            .with_writer(non_blocking)
            .with_target(true)
            .with_thread_ids(true)
            .with_span_events(FmtSpan::CLOSE);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .init();

        Some(guard)
    }
}
