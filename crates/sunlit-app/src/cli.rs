//! The command line, as clap parses it.
//!
//! The enums here mirror settings that live in `sunlit-core`, so clap owns the
//! parsing and core stays free of a clap dependency.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

use sunlit_core::config::QualityTier;

/// Sunlit Earth: get a realistic 3D view of Earth as seen from space and set it as your wallpaper
#[derive(Parser)]
#[command(version)]
pub(crate) struct Cli {
    /// Force software rendering (CPU-based, no GPU required)
    #[arg(long)]
    pub(crate) software_rendering: bool,

    /// Quality tier, overriding the saved config [possible values: low, medium, high]
    ///
    /// Debug builds default to low, release builds to high.
    #[arg(long, value_enum)]
    pub(crate) quality: Option<Quality>,

    /// Surface texture width, overriding the saved config [possible values: 8192, 4096, 2048]
    ///
    /// Lower widths are downscaled from the 8K sources once and cached.
    #[arg(long, value_enum)]
    pub(crate) texture_resolution: Option<TextureResolution>,

    /// Path to the textures directory
    #[arg(long)]
    pub(crate) textures_dir: Option<PathBuf>,

    /// Log level [possible values: error, warn, info, debug, trace]
    ///
    /// Takes precedence over the `RUST_LOG` environment variable.
    #[arg(long)]
    pub(crate) log_level: Option<String>,

    /// Startup mode: tray (default, minimize-to-tray on close) or window (close exits)
    #[arg(long, value_enum, default_value_t = Mode::Tray)]
    pub(crate) mode: Mode,

    /// Initial window visibility in tray mode: visible (default) or hidden
    #[arg(long, value_enum, default_value_t = TrayStart::Visible)]
    pub(crate) tray_start: TrayStart,

    /// Name of a local IPC socket to listen on for control commands (quit, show-window, hide-window)
    #[arg(long)]
    pub(crate) ipc_socket: Option<String>,

    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

/// CLI mirror of `QualityTier`, so clap owns the parsing and core stays free
/// of a clap dependency.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Quality {
    Low,
    Medium,
    High,
}

impl From<Quality> for QualityTier {
    fn from(value: Quality) -> Self {
        match value {
            Quality::Low => Self::Low,
            Quality::Medium => Self::Medium,
            Quality::High => Self::High,
        }
    }
}

/// CLI mirror of the texture resolution setting, so clap rejects a width that
/// is not on offer instead of the app correcting it after the fact.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum TextureResolution {
    #[value(name = "8192")]
    Full,
    #[value(name = "4096")]
    Half,
    #[value(name = "2048")]
    Quarter,
}

impl From<TextureResolution> for u32 {
    fn from(value: TextureResolution) -> Self {
        match value {
            TextureResolution::Full => 8192,
            TextureResolution::Half => 4096,
            TextureResolution::Quarter => 2048,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Mode {
    Tray,
    Window,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum TrayStart {
    Visible,
    Hidden,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Render the scene to a PNG file and exit.
    Render {
        /// Output file path
        #[arg(short, long, default_value = "render.png")]
        output: PathBuf,

        /// Image width in pixels
        #[arg(long, default_value_t = 1920)]
        width: u32,

        /// Image height in pixels
        #[arg(long, default_value_t = 1080)]
        height: u32,

        /// Path to a config file (TOML). If omitted, uses the saved user config.
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Print the monitors this session has and the wallpaper plan they come to.
    Displays {
        /// Write the plan's images into this directory instead of the desktop.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}
