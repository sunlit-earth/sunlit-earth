#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera;
mod renderer;
mod sphere;
mod wgpu_init;

slint::include_modules!();

fn main() {
    let wgpu_context = wgpu_init::init();

    slint::BackendSelector::new()
        .require_wgpu_28(wgpu_context.config)
        .select()
        .expect("Failed to select wgpu backend");

    let window = MainWindow::new().expect("Failed to create window");
    window.set_renderer_info(wgpu_context.adapter_info.into());

    // Request a redraw whenever sliders change
    let window_weak = window.as_weak();
    window.on_sliders_changed(move || {
        if let Some(win) = window_weak.upgrade() {
            win.window().request_redraw();
        }
    });

    renderer::setup_rendering_notifier(&window);

    window.run().expect("Failed to run window");
}
