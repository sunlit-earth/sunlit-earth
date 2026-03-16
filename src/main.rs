#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use clap::Parser;
use slint::ComponentHandle;

use sunlit_earth::renderer;
use sunlit_earth::scene::camera::zoom_to_distance;
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

    // Set up AA options from supported sample counts
    let (aa_labels, aa_counts, aa_default) =
        renderer::build_aa_options(&wgpu_context.supported_sample_counts);
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

    // Defer setting indices so they apply after Slint processes the model changes
    #[allow(clippy::cast_possible_wrap)]
    let blend_mode_index = 3_i32;
    let window_weak = window.as_weak();
    slint::invoke_from_event_loop(move || {
        if let Some(win) = window_weak.upgrade() {
            win.set_aa_index(aa_default);
            win.set_texture_index(blend_mode_index);
            win.window().request_redraw();
        }
    })
    .ok();

    // Request a redraw whenever sliders, AA, or texture change
    let window_weak = window.as_weak();
    window.on_sliders_changed(move || {
        if let Some(win) = window_weak.upgrade() {
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
    });

    // Mouse scroll callback: zoom in/out
    let window_weak = window.as_weak();
    window.on_mouse_scroll(move |delta| {
        let Some(win) = window_weak.upgrade() else {
            return;
        };
        let scroll_sensitivity = 0.001;
        let current_zoom = win.get_camera_zoom();
        let new_zoom = (current_zoom - delta * scroll_sensitivity).clamp(0.0, 1.0);
        win.set_camera_zoom(new_zoom);
        win.window().request_redraw();
    });

    renderer::setup_rendering_notifier(&window, aa_counts, texture_paths);

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

    // Keep sun_timer alive until the event loop exits (prevent drop optimization)
    drop(sun_timer);

    // Exit immediately to avoid a panic from thread-local destruction ordering.
    std::process::exit(0);
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
