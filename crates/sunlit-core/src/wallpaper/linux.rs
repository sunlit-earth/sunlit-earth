//! The Linux half of the wallpaper path: cut and write the files one desktop
//! reach calls for, then run that desktop's own setter over them.
//!
//! Here rather than beside the sink so that both platforms delegate the same
//! way. The commands themselves are a table in [`crate::desktop`], because
//! there is no system call to make on Linux; what this module owns is the files
//! and the running.

mod portal;
mod root_pixmap;
mod swaybg;

#[cfg(target_os = "linux")]
use crate::display::layout::DisplayMode;
#[cfg(target_os = "linux")]
use crate::engine::wallpaper_sink::WallpaperJob;

/// The images of one publish in the order a person sees the screens, left to
/// right and then top to bottom.
///
/// Not the order the monitors arrived in: `xrandr` lists connectors, and where a
/// connector sits in that list says nothing about where its screen sits. KDE is
/// the row that depends on this, because Plasma addresses a screen by position
/// and never by name, so a session whose connectors are listed right to left
/// would otherwise have its two pictures swapped.
#[cfg(any(target_os = "linux", test))]
fn in_layout_order(
    mut placed: Vec<(i32, i32, Option<std::path::PathBuf>)>,
) -> Vec<Option<std::path::PathBuf>> {
    placed.sort_by_key(|&(x, y, _)| (x, y));
    placed.into_iter().map(|(_, _, path)| path).collect()
}

/// Write the files one desktop's reach calls for, and say where they went.
///
/// Cutting a canvas is only done for a desktop that addresses monitors
/// individually. A desktop that can span is handed the canvas whole, and one
/// that holds a single image is handed the anchor's own picture even in the span
/// mode, because a canvas zoomed onto every screen separately is not the view it
/// was cut to be.
#[cfg(target_os = "linux")]
fn write_placement(
    job: &WallpaperJob,
    reach: crate::desktop::Reach,
) -> Result<crate::desktop::Placement, String> {
    use crate::desktop::{Placement, Reach};

    let mut publication = crate::wallpaper::begin_publication()?;
    match reach {
        Reach::PerMonitor => {
            let written = publication.write_job(job)?;
            let mut per_monitor = Vec::with_capacity(job.monitors.len());
            let mut untouched = Vec::new();
            let mut placed: Vec<(i32, i32, Option<std::path::PathBuf>)> =
                Vec::with_capacity(job.monitors.len());
            for (monitor, path) in job.monitors.iter().zip(written.paths) {
                if let Some(path) = path {
                    placed.push((monitor.x, monitor.y, Some(path.clone())));
                    per_monitor.push((monitor.id.clone(), path));
                } else {
                    untouched.push(monitor.id.clone());
                    placed.push((monitor.x, monitor.y, None));
                }
            }
            let single = written
                .anchor
                .ok_or_else(|| "this publish has no image for its own anchor".to_owned())?;
            let by_position = in_layout_order(placed);
            publication.commit();
            Ok(Placement {
                per_monitor,
                untouched,
                by_position,
                single,
                spanned: false,
            })
        }
        Reach::Spanned if job.mode == DisplayMode::AcrossScreens => {
            let canvas = job.canvas().ok_or_else(|| {
                "a view across the screens was asked for without a canvas".to_owned()
            })?;
            let path = publication.write("canvas", &canvas.pixels, canvas.width, canvas.height)?;
            publication.commit();
            Ok(Placement {
                per_monitor: Vec::new(),
                untouched: Vec::new(),
                by_position: vec![Some(path.clone())],
                single: path,
                spanned: true,
            })
        }
        Reach::Spanned | Reach::OneImage => {
            let frame = job.anchor_image()?;
            let path = publication.write(
                &job.anchor.to_string(),
                &frame.pixels,
                frame.width,
                frame.height,
            )?;
            publication.commit();
            Ok(Placement::single(path))
        }
    }
}

/// Run one of the desktop's commands, and answer with its output.
///
/// A failure carries the program's own stderr, because the useful half of
/// "gsettings failed" is always what gsettings said.
#[cfg(target_os = "linux")]
fn run(command: &crate::desktop::Invocation) -> Result<String, String> {
    let out = std::process::Command::new(command.program)
        .args(&command.args)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", command.program))?;
    if !out.status.success() {
        return Err(format!(
            "{} {} failed: {}",
            command.program,
            command.args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether this session has a setter, asked before anything is rendered.
///
/// On Linux both halves of the answer are cheap and both matter: which
/// setter this session gets, and whether it is installed. Finding out after a
/// full-resolution render and readback is what this exists to avoid.
#[cfg(target_os = "linux")]
pub(crate) fn check_supported() -> Result<(), String> {
    crate::desktop::choose_current()
        .map(|_| ())
        .map_err(|refusal| refusal.to_string())
}

/// Write the PNGs and run the desktop's own setter.
///
/// The setter is chosen again rather than cached from `check_supported`: the
/// sink outlives a session change, and running the previous desktop's setter
/// would fail in a way that named the wrong desktop.
#[cfg(target_os = "linux")]
pub(crate) fn set_wallpaper_job(job: &WallpaperJob) -> Result<String, String> {
    use crate::desktop::Mechanism;

    let backend = crate::desktop::choose_current()
        .map_err(|refusal| refusal.to_string())?
        .backend;
    let done = || {
        backend
            .degradation(job.mode, job.monitors.len())
            .unwrap_or_default()
    };
    match backend.mechanism() {
        Mechanism::Commands => {}
        Mechanism::RootPixmap => {
            root_pixmap::set(job)?;
            return Ok(done());
        }
        Mechanism::Swaybg => {
            let placement = write_placement(job, backend.reach())?;
            let command = backend
                .commands(&placement, "")
                .into_iter()
                .next()
                .ok_or_else(|| backend.nothing_to_run())?;
            let image_dir = placement
                .single
                .parent()
                .ok_or_else(|| "the wallpaper was written to no directory".to_owned())?;
            swaybg::publish(&command, image_dir, &crate::wallpaper::wallpaper_dir()?)?;
            tracing::info!(
                path = %placement.single.display(),
                "wallpaper set through a swaybg of our own"
            );
            return Ok(done());
        }
        Mechanism::Portal => {
            let placement = write_placement(job, backend.reach())?;
            let waiting = portal::publish(&placement.single)?;
            tracing::info!(path = %placement.single.display(), "wallpaper handed to the portal");
            return Ok([waiting, done()]
                .into_iter()
                .filter(|note| !note.is_empty())
                .collect::<Vec<_>>()
                .join("; "));
        }
    }
    let placement = write_placement(job, backend.reach())?;

    let discovered = match backend.discovery() {
        Some(query) => run(&query)?,
        None => String::new(),
    };
    let commands = backend.commands(&placement, &discovered);
    if commands.is_empty() {
        return Err(backend.nothing_to_run());
    }
    for command in &commands {
        run(command)?;
    }

    tracing::info!(
        path = %placement.single.display(),
        desktop = backend.desktop,
        commands = commands.len(),
        screens = job.monitors.len(),
        "wallpaper set successfully"
    );
    Ok(done())
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use crate::engine::wallpaper_sink::{SystemWallpaper, WallpaperSink};

    use super::*;

    /// KDE addresses a screen by where it sits, so the order the connectors
    /// were listed in must not decide which screen gets which picture.
    #[test]
    fn the_images_come_out_in_the_order_the_screens_are_seen_in() {
        let path = |name: &str| Some(std::path::PathBuf::from(name));
        // Listed right to left, and above before below, which is a layout
        // xrandr will happily report and Plasma will not.
        let placed = vec![
            (1920, 0, path("/right.png")),
            (0, 1080, path("/below.png")),
            (0, 0, path("/left.png")),
        ];
        assert_eq!(
            in_layout_order(placed),
            vec![path("/left.png"), path("/below.png"), path("/right.png")]
        );
    }

    /// A screen the mode does not paint keeps its place, because dropping it
    /// would address every screen after it one position too early.
    #[test]
    fn a_screen_with_no_picture_still_holds_its_place() {
        let path = |name: &str| Some(std::path::PathBuf::from(name));
        let placed = vec![(1920, 0, path("/right.png")), (0, 0, None)];
        assert_eq!(in_layout_order(placed), vec![None, path("/right.png")]);
    }

    /// On Linux the answer depends on the session, which a unit test does not
    /// have, so what is asserted is that a refusal explains itself.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_linux_refusal_says_what_was_tried() {
        if let Err(refusal) = SystemWallpaper.check_supported() {
            assert!(refusal.contains("no way to set the wallpaper"), "{refusal}");
            assert!(refusal.contains(": "), "{refusal}");
        } else {
            assert!(crate::desktop::detect_current().is_some());
        }
    }
}
