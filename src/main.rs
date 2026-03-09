#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera;
mod earth_texture;
mod grid_texture;
mod renderer;
mod sphere;
mod wgpu_init;

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

    // Load earth texture if available
    let textures_dir = earth_texture::resolve_textures_dir(cli.textures_dir.as_deref());
    let earth_pixels = textures_dir.and_then(|dir| {
        let path = dir.join("earth_4k.jpg");
        match earth_texture::load(&path) {
            Ok(img) => {
                eprintln!("Loaded earth texture: {}×{}", img.width, img.height);
                Some(img)
            }
            Err(e) => {
                eprintln!("{e}");
                None
            }
        }
    });

    // Set up texture options — only show "Earth" if the texture loaded
    let has_earth = earth_pixels.is_some();
    if has_earth {
        let labels: Vec<slint::SharedString> = vec!["Grid".into(), "Earth".into()];
        window.set_texture_options(slint::ModelRc::new(slint::VecModel::from(labels)));
    }

    // Defer setting indices so they apply after Slint processes the model changes
    let window_weak = window.as_weak();
    slint::invoke_from_event_loop(move || {
        if let Some(win) = window_weak.upgrade() {
            win.set_aa_index(aa_default);
            if has_earth {
                win.set_texture_index(1);
            }
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

    renderer::setup_rendering_notifier(&window, aa_counts, earth_pixels);

    window.run().expect("Failed to run window");
}
