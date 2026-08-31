//! What the settings window says about the screens a session has.
//!
//! Every function here is a pure function of a monitor list, so the combo
//! models, the anchor's round trip and the layout diagram are all decided where
//! a test can fabricate a layout no developer machine has. The `.slint` side
//! multiplies a tile by the board's size and does no arithmetic of its own.

use slint::Model;

use sunlit_core::display::Monitor;
use sunlit_core::display::layout::{self, DisplayMode, bounds_of};

use crate::{MainWindow, MonitorTile};

/// The first row of the screen combo: follow whatever the system calls primary.
pub const AUTOMATIC_SCREEN: &str = "Primary (automatic)";

/// The mode combo's rows, in the order the modes are declared.
pub fn mode_options() -> Vec<String> {
    DisplayMode::ALL
        .iter()
        .map(|mode| mode.label().to_owned())
        .collect()
}

/// One screen as the combo spells it.
///
/// `Monitor::label` is what a person recognises the screen by and is never used
/// to address it: the connector name on Linux, `Display 2` on Windows.
pub fn screen_label(monitor: &Monitor) -> String {
    let mut label = format!("{}  {}x{}", monitor.label, monitor.width, monitor.height);
    if monitor.primary {
        label.push_str("  primary");
    }
    label
}

/// The screen combo's rows, the automatic one first.
pub fn screen_options(monitors: &[Monitor]) -> Vec<String> {
    std::iter::once(AUTOMATIC_SCREEN.to_owned())
        .chain(monitors.iter().map(screen_label))
        .collect()
}

/// The id each row of the screen combo addresses, parallel to
/// [`screen_options`]. The automatic row addresses none, and its id is empty.
///
/// The window carries this list, which is what lets a save read an anchor id
/// out of a combo index without the monitor list being passed around with it.
pub fn screen_ids(monitors: &[Monitor]) -> Vec<String> {
    std::iter::once(String::new())
        .chain(monitors.iter().map(|monitor| monitor.id.clone()))
        .collect()
}

/// Where a stored anchor sits in the screen combo.
///
/// An id this session does not have is the automatic row rather than a row of
/// its own. The plan the engine builds falls back to the system primary for the
/// same reason, and a combo naming a screen that is not there would disagree
/// with the picture on the desk.
pub fn anchor_index(ids: &[String], stored: &str) -> i32 {
    let stored = stored.trim();
    if stored.is_empty() {
        return 0;
    }
    ids.iter()
        .position(|id| id == stored)
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(0)
}

/// The id the screen combo's current row addresses, empty for automatic.
pub fn anchor_at(ids: &[String], index: i32) -> String {
    usize::try_from(index)
        .ok()
        .and_then(|index| ids.get(index))
        .cloned()
        .unwrap_or_default()
}

/// The layout as the diagram draws it.
pub struct Diagram {
    /// One tile per monitor with pixels, normalised to the bounding box.
    pub tiles: Vec<MonitorTile>,
    /// Width over height of that bounding box, so the board keeps its shape
    /// instead of stretching a stacked layout into a wide one.
    pub aspect: f32,
}

/// The monitor rectangles, normalised to the bounding box of all of them.
///
/// `anchor` is a position in `monitors`, which is what `layout::resolve_anchor`
/// answers, and the tile it names is drawn highlighted. A label is the monitor's
/// own position counted from one, so the diagram and the screen combo above it
/// name the same screen the same way.
#[allow(clippy::cast_precision_loss)]
pub fn diagram(monitors: &[Monitor], anchor: Option<usize>) -> Diagram {
    let Some(bounds) = bounds_of(monitors) else {
        return Diagram {
            tiles: Vec::new(),
            aspect: 1.0,
        };
    };
    let (width, height) = (bounds.width as f32, bounds.height as f32);
    let tiles = monitors
        .iter()
        .enumerate()
        .filter(|(_, monitor)| !monitor.rect().is_empty())
        .map(|(index, monitor)| MonitorTile {
            label: (index + 1).to_string().into(),
            x: (monitor.x - bounds.x) as f32 / width,
            y: (monitor.y - bounds.y) as f32 / height,
            width: monitor.width as f32 / width,
            height: monitor.height as f32 / height,
            anchor: anchor == Some(index),
        })
        .collect();
    Diagram {
        tiles,
        aspect: width / height,
    }
}

/// Fill in the models the Displays group is built from.
///
/// Called once, at startup: the monitor list is re-queried on every publish but
/// the window is not a wallpaper, and a layout that changed while the settings
/// window is open is answered by the next publish rather than by a subscription
/// this feature deliberately does not take out.
pub fn apply_models_to_window(window: &MainWindow, monitors: &[Monitor]) {
    window.set_display_mode_options(shared(mode_options()));
    window.set_display_screen_options(shared(screen_options(monitors)));
    window.set_display_screen_ids(shared(screen_ids(monitors)));
}

/// Redraw the layout diagram for the anchor the window is currently on.
///
/// The highlighted tile is the *resolved* anchor, so the automatic row shows
/// which screen the system calls primary rather than nothing at all, and a
/// stored id this session lost highlights the screen that will actually be
/// painted.
pub fn apply_diagram_to_window(window: &MainWindow, monitors: &[Monitor], anchor: Option<&str>) {
    let resolved = layout::resolve_anchor(monitors, anchor).map(|anchor| anchor.index);
    let diagram = diagram(monitors, resolved);
    window.set_display_aspect(diagram.aspect);
    window.set_display_tiles(slint::ModelRc::new(slint::VecModel::from(diagram.tiles)));
}

/// The ids the screen combo's rows address, as the window holds them.
pub fn screen_ids_of_window(window: &MainWindow) -> Vec<String> {
    window
        .get_display_screen_ids()
        .iter()
        .map(|id| id.to_string())
        .collect()
}

/// The anchor id the screen combo is on, empty for the automatic row.
///
/// `None` where the window was never told which screens the session has, which
/// is a platform that cannot enumerate them and a test that filled no model in.
/// The stored id stands then, the same rule a one-run texture resolution
/// override follows.
pub fn anchor_from_window(window: &MainWindow) -> Option<String> {
    let ids = screen_ids_of_window(window);
    (!ids.is_empty()).then(|| anchor_at(&ids, window.get_display_anchor_index()))
}

/// The anchor id a save should write, given the row the window shows and the id
/// that is already stored.
///
/// The automatic row means somebody chose it only when the stored screen is one
/// this session has. A stored id the session lost shows as automatic too, and
/// forgetting it there would turn one unplugged cable into a lost setting.
pub fn anchor_to_store(window: &MainWindow, stored: &str) -> String {
    let Some(shown) = anchor_from_window(window) else {
        return stored.to_owned();
    };
    if !shown.is_empty() {
        return shown;
    }
    let ids = screen_ids_of_window(window);
    if ids.iter().any(|id| id == stored) {
        String::new()
    } else {
        stored.to_owned()
    }
}

/// The mode the mode combo is on.
pub fn mode_from_window(window: &MainWindow) -> DisplayMode {
    DisplayMode::from_index(window.get_display_mode_index())
}

fn shared(values: Vec<String>) -> slint::ModelRc<slint::SharedString> {
    let values: Vec<slint::SharedString> = values.into_iter().map(Into::into).collect();
    slint::ModelRc::new(slint::VecModel::from(values))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(id: &str, x: i32, y: i32, width: u32, height: u32, primary: bool) -> Monitor {
        Monitor {
            id: id.to_owned(),
            label: id.to_owned(),
            x,
            y,
            width,
            height,
            primary,
        }
    }

    fn side_by_side() -> Vec<Monitor> {
        vec![
            monitor("DP-1", 0, 0, 1920, 1080, true),
            monitor("DP-2", 1920, 0, 1920, 1080, false),
        ]
    }

    #[test]
    fn a_screen_reads_as_its_name_its_size_and_whether_it_is_the_primary() {
        let monitors = side_by_side();
        assert_eq!(screen_label(&monitors[0]), "DP-1  1920x1080  primary");
        assert_eq!(screen_label(&monitors[1]), "DP-2  1920x1080");
    }

    #[test]
    fn the_screen_combo_offers_the_automatic_row_before_any_screen() {
        let options = screen_options(&side_by_side());
        assert_eq!(options[0], AUTOMATIC_SCREEN);
        assert_eq!(options.len(), 3);
        assert_eq!(screen_ids(&side_by_side())[0], "");
    }

    #[test]
    fn an_anchor_round_trips_through_the_row_it_is_shown_on() {
        let ids = screen_ids(&side_by_side());
        for stored in ["", "DP-1", "DP-2"] {
            let index = anchor_index(&ids, stored);
            assert_eq!(anchor_at(&ids, index), stored, "stored {stored}");
        }
    }

    #[test]
    fn an_anchor_this_session_does_not_have_shows_as_automatic() {
        let ids = screen_ids(&side_by_side());
        assert_eq!(anchor_index(&ids, "HDMI-9"), 0);
        assert_eq!(anchor_at(&ids, anchor_index(&ids, "HDMI-9")), "");
    }

    #[test]
    fn a_combo_index_no_row_answers_to_is_the_automatic_anchor() {
        let ids = screen_ids(&side_by_side());
        assert_eq!(anchor_at(&ids, 7), "");
        assert_eq!(anchor_at(&ids, -1), "");
    }

    #[test]
    fn two_screens_side_by_side_are_two_halves_of_a_wide_board() {
        let diagram = diagram(&side_by_side(), Some(1));
        approx::assert_relative_eq!(diagram.aspect, 3840.0 / 1080.0);
        let tiles = &diagram.tiles;
        assert_eq!(tiles.len(), 2);
        approx::assert_relative_eq!(tiles[0].x, 0.0);
        approx::assert_relative_eq!(tiles[0].width, 0.5);
        approx::assert_relative_eq!(tiles[1].x, 0.5);
        approx::assert_relative_eq!(tiles[1].height, 1.0);
        assert!(!tiles[0].anchor && tiles[1].anchor);
        assert_eq!(tiles[0].label, "1");
        assert_eq!(tiles[1].label, "2");
    }

    #[test]
    fn a_monitor_left_of_the_primary_still_starts_at_the_boards_edge() {
        let monitors = vec![
            monitor("DP-1", 0, 0, 1920, 1080, true),
            monitor("DP-2", -1280, 0, 1280, 1024, false),
        ];
        let tiles = diagram(&monitors, Some(0)).tiles;
        approx::assert_relative_eq!(tiles[1].x, 0.0);
        approx::assert_relative_eq!(tiles[0].x, 1280.0 / 3200.0);
    }

    #[test]
    fn a_stacked_layout_keeps_its_shape() {
        let monitors = vec![
            monitor("DP-1", 0, 0, 1920, 1080, true),
            monitor("DP-2", 0, 1080, 1920, 1080, false),
        ];
        let diagram = diagram(&monitors, Some(0));
        approx::assert_relative_eq!(diagram.aspect, 1920.0 / 2160.0);
        approx::assert_relative_eq!(diagram.tiles[1].y, 0.5);
    }

    #[test]
    fn a_screen_with_no_pixels_is_not_drawn_and_does_not_renumber_the_rest() {
        let monitors = vec![
            monitor("DP-1", 0, 0, 1920, 1080, true),
            monitor("DP-2", 1920, 0, 0, 0, false),
            monitor("DP-3", 1920, 0, 1920, 1080, false),
        ];
        let tiles = diagram(&monitors, Some(0)).tiles;
        assert_eq!(tiles.len(), 2);
        assert_eq!(tiles[1].label, "3");
    }

    #[test]
    fn a_session_with_nothing_to_draw_has_no_tiles() {
        let diagram = diagram(&[], None);
        assert!(diagram.tiles.is_empty());
        approx::assert_relative_eq!(diagram.aspect, 1.0);
    }

    #[test]
    fn a_stored_anchor_the_session_still_has_is_the_row_it_names() {
        let ids = screen_ids(&side_by_side());
        assert_eq!(anchor_index(&ids, "DP-2"), 2);
        assert_eq!(anchor_index(&ids, "  DP-2  "), 2);
    }

    #[test]
    fn the_mode_combo_offers_every_mode_in_order() {
        let options = mode_options();
        assert_eq!(options.len(), DisplayMode::ALL.len());
        for mode in DisplayMode::ALL {
            assert_eq!(options[mode.index()], mode.label());
        }
    }
}
