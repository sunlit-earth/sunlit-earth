#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera;
mod renderer;
mod sphere;
mod wgpu_init;

use clap::Parser;

slint::include_modules!();

/// Sunlit Earth: get a realistic 3D view of Earth as seen from space and set it as your wallpaper
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Force software rendering (CPU-based, no GPU required)
    #[arg(long)]
    software_rendering: bool,
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
    window.set_aa_default_index(aa_default);

    // Request a redraw whenever sliders or AA setting change
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

    renderer::setup_rendering_notifier(&window, aa_counts);

    window.run().expect("Failed to run window");
}
