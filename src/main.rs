#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera;
mod grid_texture;
mod renderer;
mod sphere;
mod sun;
mod texture_loader;
mod wgpu_init;

#[cfg(test)]
mod shading;

use std::path::PathBuf;

use clap::Parser;

slint::include_modules!();

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
    // On Windows, Slint's SlintContext thread-local may be destroyed before
    // wgpu's internal LockTrace thread-local. When Slint drops the wgpu Queue,
    // Queue::drop tries to access the already-destroyed LockTrace, panicking.
    std::process::exit(0);
}
