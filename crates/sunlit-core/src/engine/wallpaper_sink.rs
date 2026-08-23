//! Where a finished wallpaper frame goes.
//!
//! Behind a trait so the engine can be soak-tested for simulated weeks without
//! repainting the desktop of the machine running the tests.

/// Destination for rendered wallpaper frames.
pub trait WallpaperSink: Send + Sync {
    /// Whether this sink can accept a frame at all.
    ///
    /// Checked before anything is rendered. Producing a wallpaper frame is the
    /// most expensive thing the engine does (a render at the display's native
    /// resolution, then a readback of that whole image: about 14 MB at 2560x1440,
    /// on a CPU rasterizer where there is no hardware to help), so a sink that
    /// is going to refuse the frame has to say so before that work happens
    /// rather than after. Sinks that always accept keep the default.
    fn check_supported(&self) -> Result<(), String> {
        Ok(())
    }

    /// Resolution to render at.
    fn target_size(&self) -> Result<(u32, u32), String>;

    /// Take a finished RGBA8 frame and make it the wallpaper.
    fn publish(&self, pixels: &[u8], width: u32, height: u32) -> Result<(), String>;
}

/// Render size used where there is no display to ask.
///
/// Windows enumerates the real monitors (`wallpaper::get_primary_monitor_resolution`)
/// and Linux parses `xrandr --query` (`display::outputs`). This is what is left
/// when neither answers: a headless run, a session with no output that has a mode
/// assigned, or a platform with no query at all. A common desktop resolution,
/// because the alternative is refusing to render a file somebody asked for.
///
/// Every use of it is logged where it happens, so a wallpaper at this size is
/// never silently a guess.
pub const DEFAULT_TARGET_SIZE: (u32, u32) = (2560, 1440);

/// Message returned where there is no wallpaper setter at all.
///
/// Deliberately a plain error rather than a stub that writes a PNG somewhere
/// and reports success: the UI shows this string in the status line, and a
/// wallpaper that silently did not change is worse than one that says so.
///
/// macOS only, now that Linux has a setter. It keeps the platform's name out of
/// it because what it says is true of any platform that reaches it.
#[cfg(not(any(windows, target_os = "linux")))]
const UNSUPPORTED: &str = "setting the desktop wallpaper is not supported on this platform yet";

/// The real desktop: save a PNG and hand it to the OS.
pub struct SystemWallpaper;

impl WallpaperSink for SystemWallpaper {
    /// Whether this desktop has a setter, asked before anything is rendered.
    ///
    /// On Linux both halves of the answer are cheap and both matter: which
    /// desktop this is, and whether its setter is installed. Finding out after a
    /// full-resolution render and readback is what this exists to avoid.
    #[cfg(target_os = "linux")]
    fn check_supported(&self) -> Result<(), String> {
        let backend = crate::desktop::detect_current().ok_or_else(|| {
            crate::desktop::no_backend_message(
                &crate::env_override(crate::desktop::DESKTOP_ENV).unwrap_or_default(),
            )
        })?;
        if which(backend.program).is_none() {
            return Err(format!(
                "this is {desktop}, whose wallpaper is set with `{program}`, and \
                 that program is not on PATH",
                desktop = backend.desktop,
                program = backend.program,
            ));
        }
        Ok(())
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    fn check_supported(&self) -> Result<(), String> {
        Err(UNSUPPORTED.to_owned())
    }

    #[cfg(windows)]
    fn target_size(&self) -> Result<(u32, u32), String> {
        crate::wallpaper::get_primary_monitor_resolution()
    }

    /// The primary output's current mode, or the documented default.
    ///
    /// Never an error: a wallpaper render at a plausible size is worth more than
    /// a refusal, and the two ways of not knowing are logged differently because
    /// they mean different things. No display to ask is ordinary; a display that
    /// answered with no usable output is not.
    #[cfg(not(windows))]
    fn target_size(&self) -> Result<(u32, u32), String> {
        let (default_width, default_height) = DEFAULT_TARGET_SIZE;
        match crate::display::outputs().as_deref() {
            Some(outputs) => match crate::display::primary_of(outputs) {
                Some(output) => Ok((output.width, output.height)),
                None => {
                    tracing::warn!(
                        width = default_width,
                        height = default_height,
                        "no display output has a mode assigned; rendering the \
                         wallpaper at the documented default size"
                    );
                    Ok(DEFAULT_TARGET_SIZE)
                }
            },
            None => {
                tracing::info!(
                    width = default_width,
                    height = default_height,
                    "no display to ask about its resolution; rendering the \
                     wallpaper at the documented default size"
                );
                Ok(DEFAULT_TARGET_SIZE)
            }
        }
    }

    #[cfg(windows)]
    fn publish(&self, pixels: &[u8], width: u32, height: u32) -> Result<(), String> {
        let path = crate::wallpaper::save_wallpaper_image(pixels, width, height)?;
        crate::wallpaper::set_wallpaper(&path)?;
        tracing::info!(path = %path.display(), "wallpaper set successfully");
        crate::memory::log_memory_usage("after wallpaper set");
        Ok(())
    }

    /// Write the PNG and run the desktop's own setter.
    ///
    /// The backend is looked up again rather than cached from `check_supported`:
    /// the sink outlives a session change, and running the previous desktop's
    /// setter would fail in a way that named the wrong desktop.
    #[cfg(target_os = "linux")]
    fn publish(&self, pixels: &[u8], width: u32, height: u32) -> Result<(), String> {
        let backend = crate::desktop::detect_current().ok_or_else(|| {
            crate::desktop::no_backend_message(
                &crate::env_override(crate::desktop::DESKTOP_ENV).unwrap_or_default(),
            )
        })?;
        let path = crate::wallpaper::save_wallpaper_image(pixels, width, height)?;

        let discovered = match backend.discovery() {
            Some(query) => run(&query)?,
            None => String::new(),
        };
        // The monitors' own names, which XFCE needs to build the property
        // xfdesktop actually reads. No display to ask is not a failure here: the
        // backend falls back to whatever its own listing offered.
        let monitors: Vec<String> = crate::display::outputs()
            .unwrap_or_default()
            .into_iter()
            .map(|output| output.name)
            .collect();
        let commands = backend.commands(&path, &discovered, &monitors);
        if commands.is_empty() {
            return Err(backend.nothing_to_run());
        }
        for command in &commands {
            run(command)?;
        }

        tracing::info!(
            path = %path.display(),
            desktop = backend.desktop,
            commands = commands.len(),
            "wallpaper set successfully"
        );
        crate::memory::log_memory_usage("after wallpaper set");
        Ok(())
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    fn publish(&self, _pixels: &[u8], _width: u32, _height: u32) -> Result<(), String> {
        Err(UNSUPPORTED.to_owned())
    }
}

/// Whether a program is on `PATH`, and where.
///
/// Written out rather than shelling out to `which`, which is one more program
/// that has to be installed for the check to work.
#[cfg(target_os = "linux")]
fn which(program: &str) -> Option<std::path::PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
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

/// A sink that renders at a fixed size and throws the pixels away, counting
/// how many frames it saw. Used by tests that care about the schedule rather
/// than the image.
pub struct CountingSink {
    size: (u32, u32),
    count: std::sync::atomic::AtomicUsize,
}

impl CountingSink {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            size: (width, height),
            count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Number of frames published so far.
    pub fn count(&self) -> usize {
        self.count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl WallpaperSink for CountingSink {
    fn target_size(&self) -> Result<(u32, u32), String> {
        Ok(self.size)
    }

    fn publish(&self, pixels: &[u8], width: u32, height: u32) -> Result<(), String> {
        assert_eq!(
            pixels.len(),
            (width as usize) * (height as usize) * 4,
            "sink received a malformed frame"
        );
        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counting_sink_starts_empty_and_counts_publishes() {
        let sink = CountingSink::new(4, 2);
        assert!(sink.check_supported().is_ok());
        assert_eq!(sink.target_size().unwrap(), (4, 2));
        assert_eq!(sink.count(), 0);
        sink.publish(&[0u8; 4 * 2 * 4], 4, 2).unwrap();
        sink.publish(&[0u8; 4 * 2 * 4], 4, 2).unwrap();
        assert_eq!(sink.count(), 2);
    }

    /// The desktop sink accepts frames on the platforms that can publish them.
    ///
    /// Split by cfg rather than skipped, because each half is an assertion: a
    /// platform with a setter must not report itself unsupported, and one
    /// without must refuse before anything is rendered.
    #[test]
    #[cfg(windows)]
    fn system_wallpaper_accepts_frames_on_windows() {
        assert!(SystemWallpaper.check_supported().is_ok());
    }

    /// On Linux the answer depends on the session, which a unit test does not
    /// have, so what is asserted is that the refusal explains itself. Both
    /// causes are refusals with something to act on: no desktop, and a desktop
    /// whose setter is not installed.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_linux_refusal_names_the_desktop_or_the_missing_program() {
        match SystemWallpaper.check_supported() {
            Ok(()) => {
                // A desktop with its setter present, which is what the guest
                // has: then the backend was found and probed.
                let backend =
                    crate::desktop::detect_current().expect("supported means a backend was found");
                assert!(which(backend.program).is_some());
            }
            Err(refusal) => {
                assert!(
                    refusal.contains(crate::desktop::DESKTOP_ENV)
                        || refusal.contains("not supported on")
                        || refusal.contains("not on PATH"),
                    "{refusal}"
                );
            }
        }
    }

    #[test]
    #[cfg(not(any(windows, target_os = "linux")))]
    fn system_wallpaper_refuses_before_anything_is_rendered() {
        let refusal = SystemWallpaper
            .check_supported()
            .expect_err("there is no wallpaper setter on this platform yet");
        assert_eq!(refusal, UNSUPPORTED);
        // And it stays refused at the far end, so a caller that ignores the
        // check still cannot believe a wallpaper was set.
        assert_eq!(
            SystemWallpaper.publish(&[0u8; 4], 1, 1).unwrap_err(),
            UNSUPPORTED
        );
    }

    /// Off Windows the render size is an answer or the documented default, and
    /// never an error: a wallpaper at a plausible size beats a refusal.
    #[test]
    #[cfg(not(windows))]
    fn the_render_size_is_the_display_or_the_documented_default() {
        let (width, height) = SystemWallpaper
            .target_size()
            .expect("a size is always available");
        assert!(
            width > 0 && height > 0,
            "a render size of zero renders nothing"
        );
        // With no display to ask, which is what a test run is, it is the one
        // documented constant rather than a guess of its own.
        if crate::display::outputs().is_none() {
            assert_eq!((width, height), DEFAULT_TARGET_SIZE);
        }
    }
}
