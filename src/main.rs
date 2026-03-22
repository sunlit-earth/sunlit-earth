#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::rc::Rc;

use clap::Parser;
use slint::ComponentHandle;

use sunlit_earth::config::{self, AppConfig};
use sunlit_earth::renderer::{self, gamma_slider_to_value, gamma_value_to_slider};
use sunlit_earth::scene::camera::{CameraParams, zoom_to_distance};
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
}

#[allow(clippy::too_many_lines)]
fn main() {
    let cli = Cli::parse();

    let wgpu_context = wgpu_init::init(cli.software_rendering);

    slint::BackendSelector::new()
        .require_wgpu_28(wgpu_context.config)
        .select()
        .expect("Failed to select wgpu backend");

    let window = MainWindow::new().expect("Failed to create window");
    window.set_renderer_info(wgpu_context.adapter_info.into());

    // Load persisted config (falls back to defaults if missing or corrupt)
    let config = config::load_config();
    apply_config_to_window(&window, &config);

    // Set up AA options from supported sample counts
    let (aa_labels, aa_counts, _aa_default) =
        renderer::build_aa_options(&wgpu_context.supported_sample_counts);
    let aa_counts_for_exit = aa_counts.clone();
    let aa_model: slint::VecModel<slint::SharedString> = aa_labels.into();
    window.set_aa_options(slint::ModelRc::new(aa_model));

    // Register JXL decoding hook before any image loading
    texture_loader::register_jxl_hook();

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

    // Debounced config save timer — restarts on every UI change, fires 1s after last change
    let config_timer = Rc::new(slint::Timer::default());
    {
        let window_weak = window.as_weak();
        let aa_counts_for_save = aa_counts.clone();
        config_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_secs(1), move || {
            if let Some(win) = window_weak.upgrade() {
                config::save_config(&read_config_from_window(&win, &aa_counts_for_save));
            }
        });
        // Stop the timer immediately — it will be restarted by callbacks
        config_timer.stop();
    }

    // Request a redraw whenever sliders, AA, or texture change
    let window_weak = window.as_weak();
    let config_timer_handle = Rc::clone(&config_timer);
    window.on_sliders_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            update_datetime_labels(&win, base_year);
            win.window().request_redraw();
        }
        config_timer_handle.restart();
    });

    let window_weak = window.as_weak();
    let config_timer_handle = Rc::clone(&config_timer);
    window.on_msaa_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            win.window().request_redraw();
        }
        config_timer_handle.restart();
    });

    let window_weak = window.as_weak();
    let config_timer_handle = Rc::clone(&config_timer);
    window.on_texture_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            win.window().request_redraw();
        }
        config_timer_handle.restart();
    });

    // "Set as Wallpaper" button callback (Windows only)
    #[cfg(windows)]
    {
        let window_weak = window.as_weak();
        window.on_set_wallpaper(move || {
            let Some(win) = window_weak.upgrade() else {
                return;
            };
            match do_set_wallpaper() {
                Ok(()) => win.set_wallpaper_status("Wallpaper set successfully".into()),
                Err(e) => win.set_wallpaper_status(format!("Error: {e}").into()),
            }
        });
    }
    #[cfg(not(windows))]
    {
        let window_weak = window.as_weak();
        window.on_set_wallpaper(move || {
            let Some(win) = window_weak.upgrade() else {
                return;
            };
            win.set_wallpaper_status("Not supported on this platform".into());
        });
    }

    // Mouse drag callback: rotate the globe (tilt-corrected)
    let window_weak = window.as_weak();
    let config_timer_handle = Rc::clone(&config_timer);
    window.on_mouse_drag(move |dx, dy| {
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
        config_timer_handle.restart();
    });

    // Mouse scroll callback: zoom in/out
    let window_weak = window.as_weak();
    let config_timer_handle = Rc::clone(&config_timer);
    window.on_mouse_scroll(move |delta| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let scroll_sensitivity = 0.0003;
        let current_zoom = win.get_camera_zoom();
        let new_zoom = (current_zoom - delta * scroll_sensitivity).clamp(0.0, 1.0);
        win.set_camera_zoom(new_zoom);
        win.window().request_redraw();
        config_timer_handle.restart();
    });

    // Reset All button callback
    let window_weak = window.as_weak();
    let config_timer_handle = Rc::clone(&config_timer);
    window.on_reset_all(move || {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let defaults = CameraParams::default();
        win.set_camera_longitude(defaults.longitude);
        win.set_camera_latitude(defaults.latitude);
        win.set_camera_zoom(defaults.zoom);
        win.set_camera_offset_x(defaults.offset_x);
        win.set_camera_offset_y(defaults.offset_y);
        win.set_camera_tilt(defaults.tilt_deg);
        win.set_camera_yaw(defaults.yaw_deg);
        win.set_camera_pitch(defaults.pitch_deg);
        // Reset lighting
        let lighting = AppConfig::default();
        win.set_terminator_width(lighting.terminator_width);
        win.set_diffuse_shading(lighting.diffuse_shading);
        win.set_diffuse_floor(lighting.diffuse_floor);
        win.set_diffuse_ramp(lighting.diffuse_ramp);
        win.set_spec_shininess(lighting.spec_shininess);
        win.set_spec_intensity(lighting.spec_intensity);
        win.set_fresnel_mix(lighting.fresnel_mix);
        win.set_fresnel_exp(lighting.fresnel_exp);
        win.set_cloud_opacity(lighting.cloud_opacity);
        win.set_cloud_floor(lighting.cloud_floor);
        win.set_cloud_gamma(lighting.cloud_gamma);
        win.set_atmo_enabled(lighting.atmo_enabled);
        win.set_rayleigh_intensity(lighting.rayleigh_intensity);
        win.set_rayleigh_sharpness(lighting.rayleigh_sharpness);
        win.set_rayleigh_haze(lighting.rayleigh_haze);
        win.set_nightglow_intensity(lighting.nightglow_intensity);
        win.set_nightglow_falloff(lighting.nightglow_falloff);
        win.set_nightglow_balance(lighting.nightglow_balance);
        win.set_day_gamma(gamma_value_to_slider(lighting.day_gamma));
        win.set_day_saturation(lighting.day_saturation);
        win.set_night_gamma(gamma_value_to_slider(lighting.night_gamma));
        win.set_night_saturation(lighting.night_saturation);
        // Reset datetime
        win.set_use_custom_datetime(false);
        win.set_custom_hour(12.0);
        win.set_custom_day_of_year(1.0);
        win.set_custom_year_index(10); // center = current year
        update_datetime_labels(&win, base_year);
        win.window().request_redraw();
        config_timer_handle.restart();
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

    // Periodic timer to update the sun position (every 2 minutes)
    let window_weak = window.as_weak();
    let sun_timer = slint::Timer::default();
    sun_timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(120),
        move || {
            if let Some(win) = window_weak.upgrade() {
                win.window().request_redraw();
            }
        },
    );

    window.run().expect("Failed to run window");

    // Save config on exit as a backstop (catches any changes during the debounce window)
    config::save_config(&read_config_from_window(&window, &aa_counts_for_exit));

    // Keep timers alive until the event loop exits (prevent drop optimization)
    drop(sun_timer);
    drop(config_timer);

    // Exit immediately to avoid a panic from thread-local destruction ordering.
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
    let (width, height) = wallpaper::get_primary_monitor_resolution()?;
    let pixels = renderer::export_wallpaper_image(width, height)?;
    let path = wallpaper::save_wallpaper_image(&pixels, width, height)?;
    wallpaper::set_wallpaper(&path)?;
    Ok(())
}
