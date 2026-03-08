slint::include_modules!();

fn main() {
    let window = MainWindow::new().expect("Failed to create window");
    window.run().expect("Failed to run window");
}
