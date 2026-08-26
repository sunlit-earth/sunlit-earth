//! Lazily created About window and the attributions displayed in it.

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::{AboutWindow, MainWindow};

/// Credits for data and libraries that contribute directly to the rendered image.
pub const ATTRIBUTIONS: [&str; 5] = [
    "HYG Database v4.4 by David Nash, CC BY-SA 4.0, codeberg.org/astronexus/hyg",
    "Astronomy Engine by Don Cross, MIT License",
    "NASA Blue Marble 2004 surface imagery",
    "NASA Black Marble 2016 nighttime imagery",
    "Live cloud composite provided by clouds.matteason.co.uk",
];

/// Shared handle that creates the About window on first use and reuses it.
#[derive(Clone, Default)]
pub struct AboutController {
    window: Rc<RefCell<Option<AboutWindow>>>,
}

impl AboutController {
    /// Register the settings window's About callback.
    pub fn register_settings_callback(&self, main_window: &MainWindow) {
        let about = self.clone();
        main_window.on_show_about(move || {
            let _ = about.show();
        });
    }

    /// Show the existing About window, creating and populating it if needed.
    pub fn show(&self) -> Result<(), slint::PlatformError> {
        let mut slot = self.window.borrow_mut();
        if slot.is_none() {
            let window = AboutWindow::new()?;
            window.set_version(env!("CARGO_PKG_VERSION").into());
            let attributions: Vec<slint::SharedString> = ATTRIBUTIONS
                .iter()
                .map(|text| slint::SharedString::from(*text))
                .collect();
            window.set_attributions(slint::ModelRc::new(slint::VecModel::from(attributions)));
            *slot = Some(window);
        }
        if let Some(window) = slot.as_ref() {
            window.show()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use slint::Model;

    use super::*;

    #[test]
    fn settings_callback_shows_version_and_hyg_attribution() {
        i_slint_backend_testing::init_no_event_loop();
        let main_window = MainWindow::new().expect("main window");
        let controller = AboutController::default();
        controller.register_settings_callback(&main_window);

        main_window.invoke_show_about();

        let slot = controller.window.borrow();
        let about = slot.as_ref().expect("About window was created");
        assert!(about.window().is_visible());
        assert_eq!(about.get_version(), env!("CARGO_PKG_VERSION"));
        assert!(
            about
                .get_attributions()
                .iter()
                .any(|text| text.contains("HYG Database v4.4") && text.contains("CC BY-SA 4.0"))
        );
    }
}
