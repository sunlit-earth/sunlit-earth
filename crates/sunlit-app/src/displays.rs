//! What the app says about the screens a session has.
//!
//! Two readers, one model. The settings window's Displays group takes its combo
//! rows, its anchor and its layout diagram from here, and so do the `displays`
//! subcommand and the IPC command of the same name. Everything is a pure
//! function of a monitor list, so all of it is decided where a test can
//! fabricate a layout no developer machine has, and the `.slint` side multiplies
//! a tile by the board's size and does no arithmetic of its own.

use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Model};

use sunlit_core::display::Monitor;
use sunlit_core::display::layout::{self, DisplayMode, Framing, bounds_of};

use crate::{MainWindow, MonitorTile};

/// The one monitor list the window works from.
///
/// Shared by the callbacks that redraw the Displays group and by the engine
/// event that replaces it when the layout moves. A lock rather than a
/// `RefCell` because the event arrives on the engine thread and hands the list
/// across; every read is on the UI thread and none of them contend.
pub type SharedMonitors = Arc<Mutex<Vec<Monitor>>>;

/// An empty shared list, for a window that has not asked yet.
pub fn shared_monitors() -> SharedMonitors {
    Arc::new(Mutex::new(Vec::new()))
}

/// The list as it stands, copied out from under the lock.
pub fn monitors_of(screens: &SharedMonitors) -> Vec<Monitor> {
    lock(screens).clone()
}

/// Put the session's monitors in the shared list.
pub fn set_monitors(screens: &SharedMonitors, monitors: Vec<Monitor>) {
    *lock(screens) = monitors;
}

/// The list, whether or not a panic elsewhere left the lock poisoned.
///
/// A `Vec<Monitor>` is replaced whole and has no invariant a half-finished
/// write could break, so reading through the poison is better than turning one
/// panic into a second one on the UI thread.
fn lock(screens: &SharedMonitors) -> std::sync::MutexGuard<'_, Vec<Monitor>> {
    screens
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

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
/// `Monitor::label` is what a person recognizes the screen by and is never used
/// to address it: the connector name on Linux, `Display 2` on Windows.
fn screen_label(monitor: &Monitor) -> String {
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
fn anchor_at(ids: &[String], index: i32) -> String {
    usize::try_from(index)
        .ok()
        .and_then(|index| ids.get(index))
        .cloned()
        .unwrap_or_default()
}

/// The layout as the diagram draws it.
struct Diagram {
    /// One tile per monitor with pixels, normalized to the bounding box.
    pub tiles: Vec<MonitorTile>,
    /// Width over height of that bounding box, so the board keeps its shape
    /// instead of stretching a stacked layout into a wide one.
    pub aspect: f32,
}

/// The monitor rectangles, normalized to the bounding box of all of them.
///
/// `anchor` is a position in `monitors`, which is what `layout::resolve_anchor`
/// answers, and the tile it names is drawn highlighted. A label is the monitor's
/// own position counted from one, so the diagram and the screen combo above it
/// name the same screen the same way.
#[allow(clippy::cast_precision_loss)]
fn diagram(monitors: &[Monitor], anchor: Option<usize>) -> Diagram {
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
/// Called at startup and again from [`replace_monitors`] whenever the engine
/// reports that the layout moved.
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

/// Rebuild the Displays group around a layout that changed, and say which row
/// the screen combo was put back on.
///
/// The row is returned rather than only set, because setting it goes through
/// [`crate::ui_callbacks::defer_combobox_indices`], which lands after Slint has
/// processed the model change and therefore after this function returns. A test
/// with no event loop has nothing else to assert against.
///
/// `stored_anchor` is the id in the config file, which is written the moment the
/// plan changes and is therefore the anchor the engine is planning with. A
/// screen that comes back gets its row back; one that is gone shows as the
/// automatic row and the stored id is left alone.
///
/// Redrawing the diagram is also what hides the group when one screen is left
/// and shows it again when a second returns, since the group is bound to the
/// tile count.
pub fn replace_monitors(
    window: &MainWindow,
    screens: &SharedMonitors,
    monitors: Vec<Monitor>,
    stored_anchor: &str,
) -> i32 {
    // The rows first: replacing a combo's model is what can move its index, and
    // this one is putting four of them back where they already were.
    let mut indices = crate::ui_callbacks::ComboIndices::of_window(window);
    apply_models_to_window(window, &monitors);
    let row = anchor_index(&screen_ids(&monitors), stored_anchor);
    indices.display_anchor = row;
    crate::ui_callbacks::defer_combobox_indices(&window.as_weak(), indices);
    let stored = (!stored_anchor.trim().is_empty()).then_some(stored_anchor);
    apply_diagram_to_window(window, &monitors, stored);
    set_monitors(screens, monitors);
    row
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

/// The plan a mode and an anchor come to, as `sunlit-earth displays` prints it.
///
/// Every number a bug report needs and nothing that changes between two runs.
/// The renders are the exports the engine would make, in its own order, which is
/// what says at a glance that two identical screens cost one of them.
pub fn report(
    monitors: &[Monitor],
    mode: DisplayMode,
    stored: Option<&str>,
    settings: Framing,
) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let plural = if monitors.len() == 1 { "" } else { "s" };
    let _ = writeln!(
        out,
        "{} monitor{plural}, mode \"{}\"",
        monitors.len(),
        mode.label()
    );
    for (index, monitor) in monitors.iter().enumerate() {
        let _ = writeln!(
            out,
            "  {}  {}  {}x{} at ({}, {}){}",
            index + 1,
            monitor.label,
            monitor.width,
            monitor.height,
            monitor.x,
            monitor.y,
            if monitor.primary { "  primary" } else { "" }
        );
    }

    let Some(anchor) = layout::resolve_anchor(monitors, stored) else {
        let _ = writeln!(
            out,
            "no screen to draw on: this session enumerated no monitors"
        );
        return out;
    };
    let anchor_label = &monitors[anchor.index].label;
    if anchor.fell_back && stored.is_some_and(|id| !id.is_empty()) {
        let _ = writeln!(
            out,
            "anchor: {anchor_label} (the config asks for \"{}\", which this session does not have)",
            stored.unwrap_or_default()
        );
    } else {
        let _ = writeln!(out, "anchor: {anchor_label}");
    }

    if mode == DisplayMode::AcrossScreens
        && let Some(bounds) = bounds_of(monitors)
    {
        let derived = layout::canvas_framing(settings, monitors[anchor.index].rect(), bounds);
        let _ = writeln!(
            out,
            "canvas: {}x{} at ({}, {})",
            bounds.width, bounds.height, bounds.x, bounds.y
        );
        let _ = writeln!(
            out,
            "lens: camera {:.1} deg, sky {:.1} deg, pan ({:.3}, {:.3}){}",
            derived.framing.camera_fov,
            derived.framing.sky_fov,
            derived.framing.offset_x,
            derived.framing.offset_y,
            if derived.sky_clamped {
                "  the sky is as wide as it goes and does not continue exactly"
            } else {
                ""
            }
        );
    }

    let _ = writeln!(out, "renders:");
    for group in layout::render_groups(monitors, mode, anchor.index) {
        let screens: Vec<String> = group
            .monitors
            .iter()
            .map(|index| (index + 1).to_string())
            .collect();
        let _ = writeln!(
            out,
            "  {}x{}  screen{} {}",
            group.width,
            group.height,
            if group.monitors.len() == 1 { "" } else { "s" },
            screens.join(", ")
        );
    }
    out
}

/// The same plan on one line, for the IPC command the e2e suite parses.
///
/// Key=value pairs with no spaces in any value, in the shape `query-memory`
/// established. `rects` and `images` are the two lists a case wants to check:
/// the session's own geometry, and the exports the mode turns it into.
pub fn signal_line(monitors: &[Monitor], mode: DisplayMode, stored: Option<&str>) -> String {
    let rects: Vec<String> = monitors
        .iter()
        .map(|m| format!("{},{},{},{}", m.x, m.y, m.width, m.height))
        .collect();
    let anchor = layout::resolve_anchor(monitors, stored);
    let images: Vec<String> = anchor
        .map(|anchor| {
            layout::render_groups(monitors, mode, anchor.index)
                .iter()
                .map(|group| format!("{}x{}", group.width, group.height))
                .collect()
        })
        .unwrap_or_default();
    format!(
        "monitors={} mode={} anchor={} fell_back={} rects={} images={}",
        monitors.len(),
        mode.name(),
        anchor.map_or(-1, |anchor| i32::try_from(anchor.index).unwrap_or(-1)),
        i32::from(anchor.is_some_and(|anchor| anchor.fell_back)),
        rects.join(";"),
        images.join(";")
    )
}

/// A destination for `sunlit-earth displays --out`: files in a directory, and
/// nothing on the desktop.
///
/// It reports the session's real monitors, so the plan it is handed is the plan
/// the desktop would have got. What it does with that plan is write it down: the
/// canvas of a span first, then one file per screen, each already cut and sized
/// for the screen it is named after.
pub struct DirectorySink {
    dir: std::path::PathBuf,
    monitors: Vec<Monitor>,
}

impl DirectorySink {
    pub fn new(dir: std::path::PathBuf, monitors: Vec<Monitor>) -> Self {
        Self { dir, monitors }
    }
}

impl sunlit_core::engine::wallpaper_sink::WallpaperSink for DirectorySink {
    fn monitors(&self) -> Result<Vec<Monitor>, String> {
        Ok(self.monitors.clone())
    }

    fn publish(
        &self,
        job: &sunlit_core::engine::wallpaper_sink::WallpaperJob,
    ) -> Result<String, String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("{} could not be created: {e}", self.dir.display()))?;
        let mut written = Vec::new();
        if let Some(canvas) = job.canvas() {
            let path = self.dir.join("canvas.png");
            sunlit_core::engine::save_png(&path, canvas.width, canvas.height, &canvas.pixels)?;
            written.push(path);
        }
        for index in 0..job.monitors.len() {
            let Some(frame) = job.image_for(index)? else {
                continue;
            };
            let path = self.dir.join(format!("screen-{}.png", index + 1));
            sunlit_core::engine::save_png(&path, frame.width, frame.height, &frame.pixels)?;
            written.push(path);
        }
        for path in &written {
            println!("wrote {}", path.display());
        }
        Ok(String::new())
    }
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

    fn settings() -> Framing {
        Framing {
            camera_fov: 20.0,
            sky_fov: 140.0,
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }

    #[test]
    fn the_report_names_every_screen_and_the_renders_it_asks_for() {
        let report = report(&side_by_side(), DisplayMode::EveryScreen, None, settings());
        assert!(report.contains("2 monitors"), "{report}");
        assert!(
            report.contains("DP-1  1920x1080 at (0, 0)  primary"),
            "{report}"
        );
        assert!(report.contains("DP-2  1920x1080 at (1920, 0)"), "{report}");
        assert!(report.contains("anchor: DP-1"), "{report}");
        // Two screens of one size are one render, which is the whole point of
        // the grouping and the thing a person checks this output for.
        assert!(report.contains("1920x1080  screens 1, 2"), "{report}");
    }

    #[test]
    fn the_report_describes_the_canvas_a_span_would_render() {
        let report = report(
            &side_by_side(),
            DisplayMode::AcrossScreens,
            None,
            settings(),
        );
        assert!(report.contains("canvas: 3840x1080 at (0, 0)"), "{report}");
        assert!(report.contains("lens: camera 20.0 deg, sky 21"), "{report}");
        assert!(
            !report.contains("as wide as it goes"),
            "two screens are inside the sky's range now: {report}"
        );
        assert!(report.contains("3840x1080  screens 1, 2"), "{report}");
    }

    #[test]
    fn the_report_says_when_the_anchor_is_not_the_one_that_was_asked_for() {
        let report = report(
            &side_by_side(),
            DisplayMode::OneScreen,
            Some("HDMI-9"),
            settings(),
        );
        assert!(
            report.contains("anchor: DP-1 (the config asks for \"HDMI-9\""),
            "{report}"
        );
        // One screen is painted and the other is left alone.
        assert!(
            report.contains(
                "1920x1080  screen 1
"
            ),
            "{report}"
        );
    }

    #[test]
    fn the_signal_line_carries_the_rectangles_and_the_exports() {
        let line = signal_line(&side_by_side(), DisplayMode::AcrossScreens, Some("DP-2"));
        assert_eq!(
            line,
            "monitors=2 mode=across-screens anchor=1 fell_back=0 rects=0,0,1920,1080;1920,0,1920,1080 images=3840x1080"
        );
        assert!(
            !line.contains("  "),
            "every value has to be one space-free token: {line}"
        );
    }

    #[test]
    fn the_signal_line_reports_a_session_with_no_screens_rather_than_inventing_one() {
        let line = signal_line(&[], DisplayMode::EveryScreen, None);
        assert!(line.contains("monitors=0"), "{line}");
        assert!(line.contains("anchor=-1"), "{line}");
        assert!(line.ends_with("rects= images="), "{line}");
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
