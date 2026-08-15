//! Where a finished wallpaper frame goes.
//!
//! Behind a trait so the engine can be soak-tested for simulated weeks without
//! repainting the desktop of the machine running the tests.

/// Destination for rendered wallpaper frames.
pub trait WallpaperSink: Send + Sync {
    /// Resolution to render at.
    fn target_size(&self) -> Result<(u32, u32), String>;

    /// Take a finished RGBA8 frame and make it the wallpaper.
    fn publish(&self, pixels: &[u8], width: u32, height: u32) -> Result<(), String>;
}

/// The real desktop: save a PNG and hand it to the OS.
pub struct SystemWallpaper;

impl WallpaperSink for SystemWallpaper {
    #[cfg(windows)]
    fn target_size(&self) -> Result<(u32, u32), String> {
        crate::wallpaper::get_primary_monitor_resolution()
    }

    #[cfg(not(windows))]
    fn target_size(&self) -> Result<(u32, u32), String> {
        Err("wallpaper export is not supported on this platform".to_owned())
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
        Err("wallpaper export is not supported on this platform".to_owned())
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
        assert_eq!(sink.target_size().unwrap(), (4, 2));
        assert_eq!(sink.count(), 0);
        sink.publish(&[0u8; 4 * 2 * 4], 4, 2).unwrap();
        sink.publish(&[0u8; 4 * 2 * 4], 4, 2).unwrap();
        assert_eq!(sink.count(), 2);
    }
}
