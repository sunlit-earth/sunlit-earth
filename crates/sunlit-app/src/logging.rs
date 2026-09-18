//! The tracing subscriber this program installs, and how loud its
//! dependencies are allowed to be.

use std::io::IsTerminal as _;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, Layer, fmt};

/// What the log file is called, before the rotation's date suffix.
const LOG_FILE_PREFIX: &str = "sunlit-earth";

/// How many days of log files are kept.
const LOG_FILES_KEPT: usize = 7;

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
/// A log file is written too when stderr is not a terminal, which is where a
/// report would otherwise have nothing in it: an `.app` launched from Finder,
/// or a Windows binary started from a shortcut. It goes under the app data
/// directory as `sunlit-earth.<date>.log`, rotating daily and keeping a week,
/// and the startup line names the file rather than the directory.
///
/// Returns the `WorkerGuard`s that must be kept alive for the duration of the
/// program so that buffered log lines are flushed before exit. In sync mode no
/// guard is needed and the list is empty.
pub(crate) fn init_logging(cli_level: Option<&str>) -> Vec<WorkerGuard> {
    let sync_log = std::env::var("SUNLIT_EARTH_SYNC_LOG").is_ok();

    let base_filter = match cli_level {
        Some(level) => EnvFilter::new(level),
        None => EnvFilter::from_default_env().add_directive(LevelFilter::INFO.into()),
    };

    let env_filter = DEPENDENCY_LEVELS
        .iter()
        .filter_map(|directive| directive.parse().ok())
        .fold(base_filter, EnvFilter::add_directive);

    let mut guards = Vec::new();
    let (file_layer, file_path) = match file_writer() {
        Some((writer, path)) => {
            let (non_blocking, guard) = tracing_appender::non_blocking(writer);
            guards.push(guard);
            (
                Some(
                    fmt::layer()
                        .with_writer(non_blocking)
                        .with_ansi(false)
                        .with_target(true)
                        .with_thread_ids(true)
                        .with_span_events(FmtSpan::CLOSE)
                        .boxed(),
                ),
                Some(path),
            )
        }
        None => (None, None),
    };

    let stderr_layer = if sync_log {
        fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(true)
            .with_thread_ids(true)
            .with_span_events(FmtSpan::CLOSE)
            .boxed()
    } else {
        let (non_blocking, guard) = tracing_appender::non_blocking(std::io::stderr());
        guards.push(guard);
        fmt::layer()
            .with_writer(non_blocking)
            .with_target(true)
            .with_thread_ids(true)
            .with_span_events(FmtSpan::CLOSE)
            .boxed()
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();

    if let Some(path) = file_path {
        tracing::info!(file = %path.display(), "also logging to a file, because stderr is not a terminal");
    }
    guards
}

/// The rolling file appender, and the file it is writing to now.
///
/// `None` where stderr is a terminal, and `None` where the appender cannot be
/// built. Never a failure: logging that refuses to start because it could not
/// open a file is worse than logging to one place.
fn file_writer() -> Option<(
    tracing_appender::rolling::RollingFileAppender,
    std::path::PathBuf,
)> {
    if std::io::stderr().is_terminal() {
        return None;
    }
    let dir = sunlit_core::app_data_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let appender = build_appender(&dir)?;
    let today = time::OffsetDateTime::now_utc().date();
    Some((appender, dir.join(log_file_name(today))))
}

/// The daily appender, in `dir`.
fn build_appender(dir: &std::path::Path) -> Option<tracing_appender::rolling::RollingFileAppender> {
    tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix("log")
        .max_log_files(LOG_FILES_KEPT)
        .build(dir)
        .ok()
}

/// What the appender calls the file it writes on `date`.
///
/// The rotation puts its own date between the prefix and the suffix, so there
/// is no `sunlit-earth.log` to point a tester at and the startup line has to
/// name this instead. The test below writes through a real appender and checks
/// the file that appears against this, because the name is a contract with
/// `tracing-appender` rather than with us.
fn log_file_name(date: time::Date) -> String {
    format!(
        "{LOG_FILE_PREFIX}.{:04}-{:02}-{:02}.log",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

#[cfg(test)]
mod tests {
    use super::{build_appender, log_file_name};

    /// The startup line is only worth printing if it names the file that turns
    /// up, which is the half of it `tracing-appender` decides.
    #[test]
    fn the_file_named_at_startup_is_the_one_the_appender_writes() {
        let dir = std::env::temp_dir().join(format!("sunlit-earth-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");

        let mut appender = build_appender(&dir).expect("an appender");
        std::io::Write::write_all(&mut appender, b"a line\n").expect("a written line");
        std::io::Write::flush(&mut appender).expect("a flush");

        let written: Vec<String> = std::fs::read_dir(&dir)
            .expect("the directory")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        let expected = log_file_name(time::OffsetDateTime::now_utc().date());
        assert!(
            written.contains(&expected),
            "the appender wrote {written:?}, not {expected}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
