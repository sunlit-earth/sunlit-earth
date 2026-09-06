//! The macOS half of the wallpaper path: write one PNG per screen, then hand
//! each to `NSWorkspace` from the main thread.
//!
//! The files are written on the thread that asked, through the shared
//! [`Publication::write_job`]. Handing them over is AppKit's, and `NSScreen` is
//! main-thread-only, so that half is submitted to the main dispatch queue and
//! waited on with a bound. `exec_async` and not `exec_sync`: the app joins the
//! engine thread from the main thread at shutdown, and a synchronous hop there
//! is a deadlock waiting for its moment.
//!
//! `docs/platforms.md` has the rest: why not `osascript`, and what a Space that
//! was not active keeps.
//!
//! [`Publication::write_job`]: super::Publication::write_job

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSImageScaling, NSScreen, NSWorkspace, NSWorkspaceDesktopImageAllowClippingKey,
    NSWorkspaceDesktopImageScalingKey,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL, ns_string};
use tracing::{debug, info, warn};

use crate::engine::wallpaper_sink::WallpaperJob;

/// How long the engine thread waits for the main thread to take the wallpaper.
///
/// Long enough that a main thread busy with a frame is not mistaken for a dead
/// one, short enough that a publish is never what a person waits on.
const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether this session has a display to paint, asked before anything is
/// rendered.
///
/// A login over SSH has none, and asking here refuses before a full-resolution
/// render rather than after.
pub(crate) fn check_supported() -> Result<(), String> {
    match crate::display::macos::active_displays() {
        Some(displays) if !displays.is_empty() => Ok(()),
        Some(_) => Err("this session has no display to paint".to_owned()),
        None => Err("macOS would not list this session's displays, so there is \
                     nothing to paint"
            .to_owned()),
    }
}

/// One screen's assignment, as it crosses to the main thread.
///
/// Strings and a path rather than anything of AppKit's, so the closure that
/// carries it is `Send`.
#[derive(Debug, Clone)]
struct Assignment {
    /// The key `display::macos::display_key` gives this monitor, which is what
    /// an `NSScreen` is matched back to.
    key: String,
    /// What the settings window calls it, for the notes.
    label: String,
    path: PathBuf,
}

/// Write the PNGs and hand each to `NSWorkspace`.
///
/// Every mode reaches here as one image per screen, because `image_for` has
/// already cut a spanning canvas into pieces. macOS has no spanning setter and
/// needs none.
pub(crate) fn set_wallpaper_job(job: &WallpaperJob) -> Result<String, String> {
    let mut publication = super::begin_publication()?;
    let written = publication.write_job(job)?;
    let assignments: Vec<Assignment> = job
        .monitors
        .iter()
        .zip(written.paths)
        .filter_map(|(monitor, path)| {
            path.map(|path| Assignment {
                key: monitor.id.clone(),
                label: monitor.label.clone(),
                path,
            })
        })
        .collect();
    let anchor = written.anchor.clone();
    publication.commit();

    if assignments.is_empty() {
        return Err("this publish has no image for any screen".to_owned());
    }

    let painted = on_main_thread(assignments, anchor)?;
    info!(screens = painted.painted, "set a wallpaper per screen");
    Ok(painted.note())
}

/// What the main thread did.
#[derive(Debug, Default)]
struct Painted {
    painted: usize,
    /// Screens the job named that AppKit has no `NSScreen` for.
    unmatched: Vec<String>,
    /// Screens AppKit refused, with the reason it gave.
    refused: Vec<String>,
    /// Whether every screen got the anchor's picture because none of them
    /// could be addressed by name.
    fell_back: bool,
}

impl Painted {
    /// The line the status area shows, empty where everything was painted.
    fn note(&self) -> String {
        if self.fell_back {
            return "macOS named no screen this publish knows about, so every screen \
                    got the anchor's picture the way they all did before"
                .to_owned();
        }
        let mut notes = Vec::new();
        if !self.unmatched.is_empty() {
            notes.push(format!(
                "macOS named no screen for {}, so those kept the wallpaper they had",
                self.unmatched.join(", ")
            ));
        }
        if !self.refused.is_empty() {
            notes.push(format!(
                "macOS would not take a wallpaper for {}",
                self.refused.join(", ")
            ));
        }
        notes.join("; ")
    }
}

/// Submit the AppKit half and wait for its answer.
///
/// The bound turns a main thread that never drains its queue into a refusal
/// rather than a hang. The files are already written and committed, so a
/// publish that times out leaves the desktop on the previous generation.
fn on_main_thread(
    assignments: Vec<Assignment>,
    anchor: Option<PathBuf>,
) -> Result<Painted, String> {
    let (tx, rx) = mpsc::channel();
    DispatchQueue::main().exec_async(move || {
        let _ = tx.send(paint(&assignments, anchor.as_deref()));
    });
    match rx.recv_timeout(MAIN_THREAD_TIMEOUT) {
        Ok(painted) => painted,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(timeout_refusal()),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("the main thread dropped the wallpaper without answering".to_owned())
        }
    }
}

/// What the status line says when the main thread never took the job.
fn timeout_refusal() -> String {
    format!(
        "the main thread did not take the wallpaper within {} s, so the desktop \
         still shows the previous one",
        MAIN_THREAD_TIMEOUT.as_secs()
    )
}

/// The AppKit half, which runs on the main thread and nowhere else.
///
/// `anchor` is the Windows setter's fallback for the same situation: where
/// AppKit names no screen this publish knows, every screen gets its picture
/// rather than nobody getting anything.
fn paint(assignments: &[Assignment], anchor: Option<&Path>) -> Result<Painted, String> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(
            "the wallpaper reached AppKit off the main thread, which cannot happen \
             through the main dispatch queue"
                .to_owned(),
        );
    };
    let workspace = NSWorkspace::sharedWorkspace();
    let options = fill_options();
    let mut painted = Painted::default();
    let mut matched = vec![false; assignments.len()];

    for screen in NSScreen::screens(mtm) {
        let Some(id) = screen_display_id(&screen) else {
            debug!("an NSScreen carries no NSScreenNumber, so nothing addresses it");
            continue;
        };
        let key = crate::display::macos::display_key(id);
        let Some(index) = assignments.iter().position(|a| a.key == key) else {
            // A screen this publish has no picture for: the display plan did
            // not name it, and it keeps what it had.
            continue;
        };
        matched[index] = true;
        let assignment = &assignments[index];
        let Some(url) = NSURL::from_file_path(&assignment.path) else {
            painted.refused.push(assignment.label.clone());
            continue;
        };
        // SAFETY: `options` holds the two documented keys of
        // `NSWorkspaceDesktopImageOptionKey` with the value types the
        // documentation gives them, an `NSNumber` each, which is the one thing
        // this method's safety comment asks of a caller.
        #[allow(unsafe_code)]
        let outcome = unsafe {
            workspace.setDesktopImageURL_forScreen_options_error(&url, &screen, &options)
        };
        match outcome {
            Ok(()) => {
                painted.painted += 1;
                // Logged, never asserted: reports on 14, 15 and 26 have
                // this answering with a directory or another screen's image.
                debug!(
                    screen = %assignment.label,
                    read_back = ?workspace.desktopImageURLForScreen(&screen),
                    "set this screen's wallpaper"
                );
            }
            Err(error) => {
                let reason = error.localizedDescription().to_string();
                warn!(
                    screen = %assignment.label,
                    error = %reason,
                    "AppKit would not take this screen's wallpaper"
                );
                painted.refused.push(assignment.label.clone());
            }
        }
    }

    for (assignment, matched) in assignments.iter().zip(matched) {
        if !matched {
            painted.unmatched.push(assignment.label.clone());
        }
    }
    if painted.painted == 0 {
        let Some(anchor) = anchor else {
            return Err(format!(
                "AppKit took no screen's wallpaper: {}",
                painted.note()
            ));
        };
        return every_screen(&workspace, mtm, &options, anchor);
    }
    Ok(painted)
}

/// Paint every screen with one picture, which is what a session macOS named no
/// screen of this publish for gets instead of nothing.
fn every_screen(
    workspace: &NSWorkspace,
    mtm: MainThreadMarker,
    options: &NSDictionary<NSString, AnyObject>,
    path: &Path,
) -> Result<Painted, String> {
    let Some(url) = NSURL::from_file_path(path) else {
        return Err(format!("{} is not a path AppKit takes", path.display()));
    };
    let mut painted = Painted::default();
    for screen in NSScreen::screens(mtm) {
        // SAFETY: as above; the options are the same dictionary.
        #[allow(unsafe_code)]
        let outcome =
            unsafe { workspace.setDesktopImageURL_forScreen_options_error(&url, &screen, options) };
        match outcome {
            Ok(()) => painted.painted += 1,
            Err(error) => {
                let reason = error.localizedDescription().to_string();
                warn!(error = %reason, "AppKit would not take the anchor's wallpaper either");
            }
        }
    }
    if painted.painted == 0 {
        return Err(
            "AppKit named no screen this publish knows about and would not \
                    take the anchor's picture for any screen either"
                .to_owned(),
        );
    }
    painted.fell_back = true;
    Ok(painted)
}

/// Fill without letterboxing, which is what every other platform's setter does.
///
/// `NSImageScaleProportionallyUpOrDown` with clipping allowed, which is also
/// the default when the keys are omitted and is spelled out so it stays so.
fn fill_options() -> objc2::rc::Retained<NSDictionary<NSString, AnyObject>> {
    // SAFETY: both are `NSString` constants exported by AppKit and are valid
    // for the life of the process; reading them is unsafe only because they are
    // extern statics.
    #[allow(unsafe_code)]
    let (scaling_key, clipping_key) = unsafe {
        (
            NSWorkspaceDesktopImageScalingKey,
            NSWorkspaceDesktopImageAllowClippingKey,
        )
    };
    let scaling =
        NSNumber::numberWithUnsignedInteger(NSImageScaling::ScaleProportionallyUpOrDown.0);
    let clipping = NSNumber::numberWithBool(true);
    let values: [&AnyObject; 2] = [&scaling, &clipping];
    NSDictionary::from_slices(&[scaling_key, clipping_key], &values)
}

/// The `CGDirectDisplayID` behind an `NSScreen`.
///
/// `deviceDescription`'s `NSScreenNumber`, which is the only bridge between the
/// two APIs.
fn screen_display_id(screen: &NSScreen) -> Option<u32> {
    let number = screen
        .deviceDescription()
        .objectForKey(ns_string!("NSScreenNumber"))?;
    number
        .downcast::<NSNumber>()
        .ok()
        .map(|number| number.unsignedIntValue())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal names the wait: "it did not work" and "the main thread has
    /// not answered in ten seconds" are different problems.
    #[test]
    fn the_timeout_refusal_says_how_long_it_waited_and_what_the_desktop_shows() {
        let refusal = timeout_refusal();
        assert!(refusal.contains("10 s"), "{refusal}");
        assert!(refusal.contains("previous"), "{refusal}");
    }

    /// A screen the setter could not match and one it could not paint are
    /// different things, and the note says which. Nothing painted at all is a
    /// refusal instead.
    #[test]
    fn the_note_tells_an_unmatched_screen_from_a_refused_one() {
        let painted = Painted {
            painted: 2,
            ..Painted::default()
        };
        assert_eq!(painted.note(), "");

        // Every screen got the same picture, so naming the unmatched ones
        // would be a list of all of them.
        let painted = Painted {
            painted: 2,
            fell_back: true,
            unmatched: vec!["Display 1".to_owned(), "Display 2".to_owned()],
            refused: Vec::new(),
        };
        assert!(
            painted.note().contains("the anchor's picture"),
            "{}",
            painted.note()
        );
        assert!(!painted.note().contains("Display 1"), "{}", painted.note());

        let painted = Painted {
            painted: 1,
            unmatched: vec!["Display 2".to_owned()],
            refused: Vec::new(),
            fell_back: false,
        };
        assert!(painted.note().contains("named no screen for Display 2"));
        assert!(painted.note().contains("kept the wallpaper they had"));

        let painted = Painted {
            painted: 1,
            unmatched: Vec::new(),
            refused: vec!["Display 1 (built-in)".to_owned()],
            fell_back: false,
        };
        assert!(
            painted
                .note()
                .contains("would not take a wallpaper for Display 1 (built-in)")
        );

        let painted = Painted {
            painted: 1,
            unmatched: vec!["Display 2".to_owned()],
            refused: vec!["Display 3".to_owned()],
            fell_back: false,
        };
        assert!(painted.note().contains("; "), "{}", painted.note());
    }

    /// Both are answers; neither is a panic.
    #[test]
    fn the_support_check_answers_for_whatever_session_this_is() {
        match check_supported() {
            Ok(()) => {
                let displays =
                    crate::display::macos::active_displays().expect("supported means a list");
                assert!(!displays.is_empty());
            }
            Err(refusal) => {
                assert!(
                    refusal.contains("no display to paint") || refusal.contains("would not list"),
                    "{refusal}"
                );
            }
        }
    }
}
