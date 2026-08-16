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

/// Render size used off Windows, where there is no monitor query yet.
///
/// Windows enumerates the real monitors (`wallpaper::get_primary_monitor_resolution`).
/// The obvious non-Windows equivalent would be to ask the windowing layer, but
/// Slint 1.17's public `Window` API reports the window's own size and scale
/// factor and nothing about the display it sits on, and adding a second
/// windowing dependency to serve a code path that currently ends in
/// "unsupported" would be the wrong trade. A common desktop resolution is
/// therefore the documented placeholder until the real Linux and macOS setters
/// land, at which point each of them brings its own native query.
pub const DEFAULT_TARGET_SIZE: (u32, u32) = (2560, 1440);

/// Message returned by the non-Windows `publish`.
///
/// Deliberately a plain error rather than a stub that writes a PNG somewhere
/// and reports success: the UI shows this string in the status line, and a
/// wallpaper that silently did not change is worse than one that says so.
#[cfg(not(windows))]
const UNSUPPORTED: &str = "setting the desktop wallpaper is not supported on this platform yet";

/// The real desktop: save a PNG and hand it to the OS.
pub struct SystemWallpaper;

impl WallpaperSink for SystemWallpaper {
    /// Off Windows there is nothing to publish to, and saying so here is what
    /// keeps the refusal cheap: `publish` alone would refuse only after a
    /// full-resolution render and readback had already happened.
    #[cfg(not(windows))]
    fn check_supported(&self) -> Result<(), String> {
        Err(UNSUPPORTED.to_owned())
    }

    #[cfg(windows)]
    fn target_size(&self) -> Result<(u32, u32), String> {
        crate::wallpaper::get_primary_monitor_resolution()
    }

    #[cfg(not(windows))]
    fn target_size(&self) -> Result<(u32, u32), String> {
        Ok(DEFAULT_TARGET_SIZE)
    }

    #[cfg(windows)]
    fn publish(&self, pixels: &[u8], width: u32, height: u32) -> Result<(), String> {
        let path = crate::wallpaper::save_wallpaper_image(pixels, width, height)?;
        crate::wallpaper::set_wallpaper(&path)?;
        tracing::info!(path = %path.display(), "wallpaper set successfully");
        crate::memory::log_memory_usage("after wallpaper set");
        Ok(())
    }

    #[cfg(not(windows))]
    fn publish(&self, _pixels: &[u8], _width: u32, _height: u32) -> Result<(), String> {
        Err(UNSUPPORTED.to_owned())
    }
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

    /// The desktop sink is available on Windows and nowhere else yet.
    ///
    /// Split by cfg rather than skipped, because both halves are assertions:
    /// the platform that has a setter must not report itself unsupported, and
    /// the platforms that do not must refuse before anything is rendered.
    #[test]
    #[cfg(windows)]
    fn system_wallpaper_accepts_frames_on_windows() {
        assert!(SystemWallpaper.check_supported().is_ok());
    }

    #[test]
    #[cfg(not(windows))]
    fn system_wallpaper_refuses_before_anything_is_rendered() {
        let refusal = SystemWallpaper
            .check_supported()
            .expect_err("there is no wallpaper setter off Windows yet");
        assert_eq!(refusal, UNSUPPORTED);
        // And it stays refused at the far end, so a caller that ignores the
        // check still cannot believe a wallpaper was set.
        assert_eq!(
            SystemWallpaper.publish(&[0u8; 4], 1, 1).unwrap_err(),
            UNSUPPORTED
        );
    }

    /// The documented placeholder resolution, which nothing else pins.
    #[test]
    #[cfg(not(windows))]
    fn system_wallpaper_target_size_is_the_documented_default() {
        let (width, height) = SystemWallpaper
            .target_size()
            .expect("the placeholder size is always available");
        assert_eq!((width, height), DEFAULT_TARGET_SIZE);
        assert!(
            width > 0 && height > 0,
            "a render size of zero renders nothing"
        );
    }
}
