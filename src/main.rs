#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use clap::Parser;
use slint::ComponentHandle;
use tracing::{debug, info};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

use sunlit_earth::config::{self, AppConfig};
use sunlit_earth::renderer::{self, gamma_slider_to_value, gamma_value_to_slider};
use sunlit_earth::scene::camera::{PRESETS, zoom_to_distance};
use sunlit_earth::scene::datetime;
use sunlit_earth::texture_loader;
#[cfg(windows)]
use sunlit_earth::wallpaper;
use sunlit_earth::wgpu_init;
use sunlit_earth::MainWindow;

/// Sunlit Earth: get a realistic 3D view of Earth as seen from space and set it as your wallpaper
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Force software rendering (CPU-based, no GPU required)
    #[arg(long)]
    software_rendering: bool,

    /// Path to the textures directory
    #[arg(long)]
    textures_dir: Option<PathBuf>,

    /// Log level [possible values: error, warn, info, debug, trace]
    ///
    /// Takes precedence over the `RUST_LOG` environment variable.
    #[arg(long)]
    log_level: Option<String>,
}

/// Initialize the global tracing subscriber.
///
/// `cli_level` is the default log level from the `--log-level` CLI flag.
/// It is ignored when the `RUST_LOG` environment variable is set.
///
/// Returns a `WorkerGuard` that must be kept alive for the duration of the
/// program so that buffered log lines are flushed before exit.
fn init_logging(cli_level: Option<&str>) -> tracing_appender::non_blocking::WorkerGuard {
    let (non_blocking, guard) = tracing_appender::non_blocking(std::io::stderr());

    let base_filter = match cli_level {
        Some(level) => EnvFilter::new(level),
        None => EnvFilter::from_default_env().add_directive("info".parse().expect("valid directive")),
    };

    let env_filter = base_filter
        .add_directive("wgpu_core=warn".parse().expect("valid directive"))
        .add_directive("wgpu_hal=error".parse().expect("valid directive"))
        .add_directive("naga=warn".parse().expect("valid directive"))
        .add_directive("winit=warn".parse().expect("valid directive"))
        .add_directive("ureq=warn".parse().expect("valid directive"))
        .add_directive("ureq_proto=warn".parse().expect("valid directive"))
        .add_directive("rustls=warn".parse().expect("valid directive"))
        .add_directive("jxl_render=warn".parse().expect("valid directive"))
        .add_directive("jxl_grid=warn".parse().expect("valid directive"))
        .add_directive("jxl_modular=warn".parse().expect("valid directive"))
        .add_directive("jxl_bitstream=warn".parse().expect("valid directive"))
        .add_directive("jxl_frame=warn".parse().expect("valid directive"))
        .add_directive("jxl_color=warn".parse().expect("valid directive"));

    let fmt_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_target(true)
        .with_thread_ids(true)
        .with_span_events(FmtSpan::CLOSE);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .init();

    guard
}

#[allow(clippy::too_many_lines)]
fn main() {
    let cli = Cli::parse();
    let _guard = init_logging(cli.log_level.as_deref());
    info!("sunlit earth v{}", env!("CARGO_PKG_VERSION"));
    debug!(
        software_rendering = cli.software_rendering,
        textures_dir = ?cli.textures_dir,
        log_level = ?cli.log_level,
        "parsed CLI arguments"
    );

    let wgpu_context = wgpu_init::init(cli.software_rendering);
    sunlit_earth::memory::log_memory_usage("after wgpu init");

    slint::BackendSelector::new()
        .require_wgpu_28(wgpu_context.config)
        .select()
        .expect("Failed to select wgpu backend");

    let window = MainWindow::new().expect("Failed to create window");
    window.set_renderer_info(wgpu_context.adapter_info.into());
    sunlit_earth::memory::log_memory_usage("after window creation");

    // Load persisted config (falls back to defaults if missing or corrupt)
    let config = config::load_config();
    apply_config_to_window(&window, &config);
    debug!("loaded config from disk");

    // Set up AA options from supported sample counts
    let (aa_labels, aa_counts, _aa_default) =
        renderer::build_aa_options(&wgpu_context.supported_sample_counts);
    let aa_model: slint::VecModel<slint::SharedString> = aa_labels.into();
    window.set_aa_options(slint::ModelRc::new(aa_model));

    // Register JXL decoding hook before any image loading
    texture_loader::register_jxl_hook();
    debug!("registered JXL decoding hook");

    // Resolve texture paths for JXL files (loaded lazily when selected)
    let textures_dir = texture_loader::resolve_textures_dir(cli.textures_dir.as_deref());
    let day_path = textures_dir
        .as_ref()
        .map(|d| d.join("world.topo.200405.jxl"))
        .filter(|p| p.exists());
    let night_path = textures_dir
        .as_ref()
        .map(|d| d.join("BlackMarble_2016.jxl"))
        .filter(|p| p.exists());
    let texture_paths = vec![day_path, night_path];
    info!(textures_dir = ?textures_dir, day = ?texture_paths[0], night = ?texture_paths[1], "resolved texture paths");

    // Set up texture options — always show all four
    let labels: Vec<slint::SharedString> = vec![
        "Grid".into(),
        "Day".into(),
        "Night".into(),
        "Day/Night Blend".into(),
    ];
    window.set_texture_options(slint::ModelRc::new(slint::VecModel::from(labels)));

    // Set up year ComboBox options (current year +/- 10)
    let (base_year, end_year) = datetime::year_range();
    let year_labels: Vec<slint::SharedString> =
        (base_year..=end_year).map(|y| slint::SharedString::from(y.to_string())).collect();
    window.set_year_options(slint::ModelRc::new(slint::VecModel::from(year_labels)));

    // Defer setting indices so they apply after Slint processes the model changes
    let config_aa_index = config::find_sample_count_index(&aa_counts, config.sample_count);
    let config_texture_index = config.texture_index;
    let window_weak = window.as_weak();
    slint::invoke_from_event_loop(move || {
        if let Some(win) = window_weak.upgrade() {
            win.set_aa_index(config_aa_index);
            win.set_texture_index(config_texture_index);
            win.window().request_redraw();
        }
    })
    .ok();

    // Request a redraw whenever sliders, AA, or texture change
    let window_weak = window.as_weak();
    window.on_sliders_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            update_datetime_labels(&win, base_year);
            win.window().request_redraw();
        }
    });

    // When "Override date/time" is toggled on, initialize sliders to current UTC time
    let window_weak = window.as_weak();
    window.on_datetime_override_toggled(move || {
        if let Some(win) = window_weak.upgrade() {
            if win.get_use_custom_datetime() {
                let now = time::OffsetDateTime::now_utc();
                let hour =
                    now.hour() as f32 + now.minute() as f32 / 60.0 + now.second() as f32 / 3600.0;
                win.set_custom_hour(hour);
                win.set_custom_day_of_year(now.ordinal() as f32);
                win.set_custom_year_index(now.year() - base_year);
            }
            update_datetime_labels(&win, base_year);
            win.window().request_redraw();
        }
    });

    let window_weak = window.as_weak();
    window.on_msaa_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            win.window().request_redraw();
        }
    });

    let window_weak = window.as_weak();
    window.on_texture_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            win.window().request_redraw();
        }
    });

    // "Set as Wallpaper" button: save config, then apply wallpaper.
    {
        let window_weak = window.as_weak();
        let aa_counts_for_save = aa_counts.clone();
        window.on_set_wallpaper(move || {
            let Some(win) = window_weak.upgrade() else {
                return;
            };
            config::save_config(&read_config_from_window(&win, &aa_counts_for_save));
            #[cfg(windows)]
            if let Err(e) = do_set_wallpaper() {
                tracing::error!("Failed to set wallpaper: {e}");
            }
            #[cfg(not(windows))]
            tracing::warn!("Wallpaper export is not supported on this platform");
        });
    }

    // Left-drag callback: rotate the globe (tilt-corrected)
    let window_weak = window.as_weak();
    window.on_mouse_drag_globe(move |dx, dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let tilt_rad = win.get_camera_tilt().to_radians();
        let cos_t = tilt_rad.cos();
        let sin_t = tilt_rad.sin();

        // Rotate the (dx, dy) vector by -tilt to undo the screen-space rotation
        let delta = [
            dx * cos_t + dy * sin_t,
            -dx * sin_t + dy * cos_t,
        ];

        // Scale sensitivity proportionally to camera distance
        let zoom = win.get_camera_zoom();
        let degrees_per_px = 0.3 * zoom_to_distance(zoom) / 8.0;

        let new_lon = win.get_camera_longitude() - delta[0] * degrees_per_px;
        let new_lat = win.get_camera_latitude() + delta[1] * degrees_per_px;

        // Wrap longitude to [-180, 180], clamp latitude to [-89, 89]
        let wrapped_lon = ((new_lon + 180.0) % 360.0 + 360.0) % 360.0 - 180.0;
        let clamped_lat = new_lat.clamp(-89.0, 89.0);

        win.set_camera_longitude(wrapped_lon);
        win.set_camera_latitude(clamped_lat);
        win.window().request_redraw();
    });

    // Right-drag callback: adjust framing (offset X/Y) with scrolling behavior
    let window_weak = window.as_weak();
    window.on_mouse_drag_frame(move |dx, dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let zoom = win.get_camera_zoom();
        let sensitivity = 0.002 * zoom_to_distance(zoom) / 8.0;

        let new_x = (win.get_camera_offset_x() - dx * sensitivity).clamp(-3.0, 3.0);
        let new_y = (win.get_camera_offset_y() + dy * sensitivity).clamp(-3.0, 3.0);

        win.set_camera_offset_x(new_x);
        win.set_camera_offset_y(new_y);
        win.window().request_redraw();
    });

    // Middle-drag callback: adjust pitch and yaw
    let window_weak = window.as_weak();
    window.on_mouse_drag_orient(move |dx, dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let degrees_per_px = 0.2;

        let new_yaw = (win.get_camera_yaw() + dx * degrees_per_px).clamp(-90.0, 90.0);
        let new_pitch = (win.get_camera_pitch() - dy * degrees_per_px).clamp(-90.0, 90.0);

        win.set_camera_yaw(new_yaw);
        win.set_camera_pitch(new_pitch);
        win.window().request_redraw();
    });

    // Left+right drag callback: adjust tilt (horizontal only)
    let window_weak = window.as_weak();
    window.on_mouse_drag_tilt(move |dx, _dy| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let degrees_per_px = 0.5;

        let new_tilt = win.get_camera_tilt() + dx * degrees_per_px;
        // Wrap to [-180, 180]
        let wrapped_tilt = ((new_tilt + 180.0) % 360.0 + 360.0) % 360.0 - 180.0;

        win.set_camera_tilt(wrapped_tilt);
        win.window().request_redraw();
    });

    // Mouse scroll callback: zoom in/out
    let window_weak = window.as_weak();
    window.on_mouse_scroll(move |delta| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let scroll_sensitivity = 0.0003;
        let current_zoom = win.get_camera_zoom();
        let new_zoom = (current_zoom - delta * scroll_sensitivity).clamp(0.0, 1.0);
        win.set_camera_zoom(new_zoom);
        win.window().request_redraw();
    });

    // Apply-preset callback: set camera parameters from the PRESETS array
    let window_weak = window.as_weak();
    #[allow(clippy::cast_sign_loss)]
    window.on_apply_preset(move |index| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let Some(preset) = PRESETS.get(index as usize) else {
            return;
        };
        win.set_camera_longitude(preset.longitude);
        win.set_camera_latitude(preset.latitude);
        win.set_camera_zoom(preset.zoom);
        win.set_camera_offset_x(preset.offset_x);
        win.set_camera_offset_y(preset.offset_y);
        win.set_camera_tilt(preset.tilt_deg);
        win.set_camera_yaw(preset.yaw_deg);
        win.set_camera_pitch(preset.pitch_deg);
        win.window().request_redraw();
    });

    // Load-defaults callback: restore all settings to AppConfig::default() without saving
    let window_weak = window.as_weak();
    let aa_counts_for_defaults = aa_counts.clone();
    window.on_load_defaults(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let defaults = AppConfig::default();
        apply_config_to_window(&win, &defaults);

        // Defer ComboBox index updates (same pattern as startup)
        let default_aa_index = config::find_sample_count_index(&aa_counts_for_defaults, defaults.sample_count);
        let default_texture_index = defaults.texture_index;
        let win_weak = win.as_weak();
        slint::invoke_from_event_loop(move || {
            if let Some(w) = win_weak.upgrade() {
                w.set_aa_index(default_aa_index);
                w.set_texture_index(default_texture_index);
                w.window().request_redraw();
            }
        })
        .ok();

        win.window().request_redraw();
    });

    // Reset callback: reload config from disk and restore UI to last-saved state
    let window_weak = window.as_weak();
    let aa_counts_for_reset = aa_counts.clone();
    window.on_reset(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let loaded = config::load_config();
        apply_config_to_window(&win, &loaded);

        // Defer ComboBox index updates (same pattern as startup)
        let loaded_aa_index = config::find_sample_count_index(&aa_counts_for_reset, loaded.sample_count);
        let loaded_texture_index = loaded.texture_index;
        let win_weak = win.as_weak();
        slint::invoke_from_event_loop(move || {
            if let Some(w) = win_weak.upgrade() {
                w.set_aa_index(loaded_aa_index);
                w.set_texture_index(loaded_texture_index);
                w.window().request_redraw();
            }
        })
        .ok();

        win.window().request_redraw();
    });

    // Create the texture channel in main.rs so both the renderer and the
    // cloud fetcher can share it (renderer gets both tx+rx, fetcher gets tx clone)
    let (texture_tx, texture_rx) =
        std::sync::mpsc::channel::<sunlit_earth::renderer::DecodedTextureMessage>();

    renderer::setup_rendering_notifier(
        &window,
        aa_counts,
        texture_paths,
        texture_tx.clone(),
        texture_rx,
    );

    // Spawn the background cloud fetcher (slot index 3 = CLOUDS_SLOT)
    sunlit_earth::cloud_fetcher::spawn_cloud_fetcher(
        texture_tx,
        window.as_weak(),
        3,
    );
    info!("spawned cloud fetcher background thread");

    // Periodic timer to update the sun position (every 2 minutes)
    let window_weak = window.as_weak();
    let sun_timer = slint::Timer::default();
    sun_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(120),
        move || {
            if let Some(win) = window_weak.upgrade() {
                sunlit_earth::memory::log_memory_usage("sun timer tick");
                win.window().request_redraw();
            }
        },
    );

    window.run().expect("Failed to run window");

    // Save window geometry on close (preserves all other config values on disk)
    let size = window.window().size();
    let pos = window.window().position();
    config::save_window_geometry(pos.x, pos.y, size.width, size.height);

    sunlit_earth::memory::log_memory_usage("before exit");

    // Keep timers alive until the event loop exits (prevent drop optimization)
    drop(sun_timer);

    // Exit immediately to avoid a panic from thread-local destruction ordering.
    debug!("exiting");
    std::process::exit(0);
}

/// Apply a loaded config to all window properties.
///
/// Called once at startup to restore persisted settings. The `texture_index`
/// and `aa_index` are set via a deferred `invoke_from_event_loop` instead,
/// so they are not set here.
fn apply_config_to_window(window: &MainWindow, config: &AppConfig) {
    window.set_camera_longitude(config.longitude);
    window.set_camera_latitude(config.latitude);
    window.set_camera_zoom(config.zoom);
    window.set_camera_offset_x(config.offset_x);
    window.set_camera_offset_y(config.offset_y);
    window.set_camera_tilt(config.tilt);
    window.set_camera_yaw(config.yaw);
    window.set_camera_pitch(config.pitch);
    window.set_terminator_width(config.terminator_width);
    window.set_diffuse_shading(config.diffuse_shading);
    window.set_diffuse_floor(config.diffuse_floor);
    window.set_diffuse_ramp(config.diffuse_ramp);
    window.set_spec_shininess(config.spec_shininess);
    window.set_spec_intensity(config.spec_intensity);
    window.set_fresnel_mix(config.fresnel_mix);
    window.set_fresnel_exp(config.fresnel_exp);
    window.set_cloud_opacity(config.cloud_opacity);
    window.set_cloud_floor(config.cloud_floor);
    window.set_cloud_gamma(config.cloud_gamma);
    window.set_atmo_enabled(config.atmo_enabled);
    window.set_rayleigh_intensity(config.rayleigh_intensity);
    window.set_rayleigh_sharpness(config.rayleigh_sharpness);
    window.set_rayleigh_haze(config.rayleigh_haze);
    window.set_nightglow_intensity(config.nightglow_intensity);
    window.set_nightglow_falloff(config.nightglow_falloff);
    window.set_nightglow_balance(config.nightglow_balance);
    window.set_day_gamma(gamma_value_to_slider(config.day_gamma));
    window.set_day_saturation(config.day_saturation);
    window.set_night_gamma(gamma_value_to_slider(config.night_gamma));
    window.set_night_saturation(config.night_saturation);

    // Custom datetime
    window.set_use_custom_datetime(config.use_custom_datetime);
    window.set_custom_hour(config.custom_hour);
    window.set_custom_day_of_year(config.custom_day_of_year);
    let base_year = datetime::base_year();
    let year_index = (config.custom_year - base_year).clamp(0, 20);
    window.set_custom_year_index(year_index);

    // Update display labels
    update_datetime_labels(window, base_year);

    if let Some((x, y, w, h)) = config::validated_window_geometry(config) {
        window.window().set_position(slint::PhysicalPosition::new(x, y));
        window.window().set_size(slint::PhysicalSize::new(w, h));
    }
}

/// Read all persisted settings from the window's current UI state.
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
fn read_config_from_window(window: &MainWindow, aa_counts: &[u32]) -> AppConfig {
    let aa_index = window.get_aa_index() as usize;
    let sample_count = aa_counts.get(aa_index).copied().unwrap_or(1);

    let pos = window.window().position();
    let size = window.window().size();

    AppConfig {
        longitude: window.get_camera_longitude(),
        latitude: window.get_camera_latitude(),
        zoom: window.get_camera_zoom(),
        tilt: window.get_camera_tilt(),
        yaw: window.get_camera_yaw(),
        pitch: window.get_camera_pitch(),
        offset_x: window.get_camera_offset_x(),
        offset_y: window.get_camera_offset_y(),
        texture_index: window.get_texture_index(),
        sample_count,
        terminator_width: window.get_terminator_width(),
        diffuse_shading: window.get_diffuse_shading(),
        diffuse_floor: window.get_diffuse_floor(),
        diffuse_ramp: window.get_diffuse_ramp(),
        spec_shininess: window.get_spec_shininess(),
        spec_intensity: window.get_spec_intensity(),
        fresnel_mix: window.get_fresnel_mix(),
        fresnel_exp: window.get_fresnel_exp(),
        cloud_opacity: window.get_cloud_opacity(),
        cloud_floor: window.get_cloud_floor(),
        cloud_gamma: window.get_cloud_gamma(),
        atmo_enabled: window.get_atmo_enabled(),
        rayleigh_intensity: window.get_rayleigh_intensity(),
        rayleigh_sharpness: window.get_rayleigh_sharpness(),
        rayleigh_haze: window.get_rayleigh_haze(),
        nightglow_intensity: window.get_nightglow_intensity(),
        nightglow_falloff: window.get_nightglow_falloff(),
        nightglow_balance: window.get_nightglow_balance(),
        day_gamma: gamma_slider_to_value(window.get_day_gamma()),
        day_saturation: window.get_day_saturation(),
        night_gamma: gamma_slider_to_value(window.get_night_gamma()),
        night_saturation: window.get_night_saturation(),
        use_custom_datetime: window.get_use_custom_datetime(),
        custom_hour: window.get_custom_hour(),
        custom_day_of_year: window.get_custom_day_of_year(),
        custom_year: window.get_custom_year_index() + datetime::base_year(),
        window_x: Some(pos.x),
        window_y: Some(pos.y),
        window_width: Some(size.width),
        window_height: Some(size.height),
    }
}

/// Update the hour label, day label, and max-day-of-year on the window
/// based on the current datetime slider values.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn update_datetime_labels(window: &MainWindow, base_year: i32) {
    let h = window.get_custom_hour();
    window.set_hour_label(datetime::hour_label(h).into());

    let year = window.get_custom_year_index() + base_year;
    let max_doy = datetime::days_in_year(year);
    window.set_max_day_of_year(i32::from(max_doy));

    // Clamp day-of-year if it exceeds the new max (e.g. leap -> non-leap)
    let doy = window.get_custom_day_of_year();
    if doy > f32::from(max_doy) {
        window.set_custom_day_of_year(f32::from(max_doy));
    }

    let doy = window.get_custom_day_of_year() as u16;
    let doy = doy.max(1);
    window.set_day_label(datetime::month_day_label(doy, year).into());
}

/// Render the current scene at the primary monitor's resolution, save as PNG,
/// and set it as the Windows desktop wallpaper.
#[cfg(windows)]
fn do_set_wallpaper() -> Result<(), String> {
    info!("starting wallpaper export");
    let (width, height) = wallpaper::get_primary_monitor_resolution()?;
    let pixels = renderer::export_wallpaper_image(width, height)?;
    let path = wallpaper::save_wallpaper_image(&pixels, width, height)?;
    wallpaper::set_wallpaper(&path)?;
    info!(path = %path.display(), "wallpaper set successfully");
    sunlit_earth::memory::log_memory_usage("after wallpaper set");
    Ok(())
}

