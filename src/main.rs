mod camera;
mod renderer;
mod sphere;

slint::include_modules!();

fn main() {
    // Ensure Slint uses the wgpu backend
    slint::BackendSelector::new()
        .require_wgpu_28(slint::wgpu_28::WGPUConfiguration::default())
        .select()
        .expect("Failed to select wgpu backend");

    let window = MainWindow::new().expect("Failed to create window");

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
