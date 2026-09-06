//! What the engine does with a finished frame once the destination is the
//! desktop rather than the preview.
//!
//! The publish half of [`super::Engine`]: deciding when a wallpaper may go out,
//! saying how it went, noticing that the screens moved, and framing the export
//! the sink asked for.

use std::sync::Arc;

use tracing::{debug, error, info, warn};

use super::wallpaper_sink::{Frame, JobImages, WallpaperJob};
use super::{Engine, EngineEvent, join_notes};

impl Engine {
    /// Render at the sink's native resolution and hand the pixels over.
    ///
    /// Held back while a texture the current mode needs is on its way. The
    /// renderer falls back to the procedural grid while a slot is empty, which
    /// is fine for a preview and not fine for someone's desktop, and a
    /// resolution switch empties one for as long as the reload takes. Every
    /// caller that publishes arrives here, so this covers all of them.
    /// `RenderToFile` and `ExportPixels` are deliberately not covered; they
    /// answer a caller holding a reply channel, which decides for itself what it
    /// will wait for.
    ///
    /// One request is remembered, not a queue of them: two wallpaper updates
    /// asked for during one reload are the same wallpaper.
    pub(super) fn publish_wallpaper(&mut self) {
        // A debt that already stands has had its support answer, and the only
        // thing left that can change is whether the textures have landed. `tick`
        // comes back here every 50 ms until it is paid, and on Linux
        // `check_supported` walks `PATH` with a stat per directory.
        if self.wallpaper_owed && self.renderer.textures_pending(self.params.texture_index) {
            return;
        }

        // Support first, before the size query, the render, and the wait below.
        // Off Windows this is the whole answer, and everything after it is
        // something to pay for on the way to a refusal that was already known:
        // a native-resolution render and its readback, or seconds of waiting for
        // textures that will not change it.
        if let Err(e) = self.wallpaper.check_supported() {
            self.report_wallpaper(Err(e));
            return;
        }

        if self.renderer.textures_pending(self.params.texture_index) {
            if !self.wallpaper_owed {
                info!("wallpaper update deferred until the textures have loaded");
            }
            self.wallpaper_owed = true;
            return;
        }

        let result = self
            .build_wallpaper_job()
            .and_then(|(job, note)| Ok(join_notes(note, self.wallpaper.publish(&job)?)));
        self.report_wallpaper(result);
    }

    /// Report a finished publish attempt and settle the debt for it.
    pub(super) fn report_wallpaper(&mut self, result: Result<String, String>) {
        self.wallpaper_owed = false;
        self.published |= result.is_ok();
        match &result {
            Err(e) => error!(error = %e, "wallpaper update failed"),
            Ok(note) if !note.is_empty() => {
                info!(note, "the wallpaper is not quite what was asked");
            }
            Ok(_) => {}
        }
        self.emit(EngineEvent::WallpaperSet(result));
    }

    /// Ask what the monitors are now, and act if they are not what they were.
    ///
    /// One query per settled burst, whatever the burst was made of: the hints
    /// this design pays for and discards on purpose are a resume from sleep, a
    /// scaling change and a color depth change, each of which costs one
    /// enumeration and an equal comparison.
    ///
    /// A republish follows only where `published` says the desk is holding a
    /// picture this process made; `docs/architecture.md` has the argument for
    /// both edges of that rule.
    pub(super) fn recheck_displays(&mut self) {
        let monitors = match self.wallpaper.monitors() {
            Ok(monitors) => monitors,
            Err(e) => {
                warn!(error = %e, "the monitors could not be listed after a display change");
                return;
            }
        };
        if self.known_monitors.as_ref() == Some(&monitors) {
            debug!(
                screens = monitors.len(),
                "the display layout is the one already known"
            );
            return;
        }
        info!(screens = monitors.len(), "the display layout changed");
        self.known_monitors = Some(monitors.clone());
        self.emit(EngineEvent::MonitorsChanged(monitors));
        if self.published {
            self.publish_wallpaper();
        }
    }

    /// Render everything this session's monitors need, and say what was odd.
    ///
    /// The monitor list is asked for on every publish rather than cached: the
    /// layout changes without telling anybody, and the auto-refresh means a
    /// stale one would be on the screen for as long as the interval.
    pub(super) fn build_wallpaper_job(&mut self) -> Result<(WallpaperJob, String), String> {
        use crate::display::layout;

        let monitors = self.wallpaper.monitors()?;
        // The list a publish planned with is the one a later hint is compared
        // against, so a layout that kept moving is published again only if it
        // is different again.
        self.known_monitors = Some(monitors.clone());
        let anchor = layout::resolve_anchor(&monitors, self.anchor_monitor.as_deref())
            .ok_or_else(|| "this session has no monitor to put a wallpaper on".to_owned())?;
        let mut note = String::new();
        if anchor.fell_back {
            note = format!(
                "the screen this was set to draw on is not connected, so {} is standing in for it",
                monitors[anchor.index].label
            );
            warn!(note, "the stored anchor monitor is gone");
        }

        let settings = layout::Framing {
            camera_fov: self.params.camera.fov_deg,
            sky_fov: self.params.sky_fov,
            offset_x: self.params.camera.offset_x,
            offset_y: self.params.camera.offset_y,
        };
        let groups = layout::render_groups(&monitors, self.display_mode, anchor.index);
        if groups.is_empty() {
            return Err("this session has no screen with any pixels on it".to_owned());
        }
        for group in &groups {
            self.check_export_fits(group.width, group.height)?;
        }
        self.prepare_export();

        if self.display_mode == layout::DisplayMode::AcrossScreens {
            let bounds = layout::bounds_of(&monitors)
                .ok_or_else(|| "this session has no screen with any pixels on it".to_owned())?;
            let derived = layout::canvas_framing(settings, monitors[anchor.index].rect(), bounds);
            if derived.sky_clamped {
                let clamped = "the sky is as wide as it goes, so it does not continue \
                               across the screens as exactly as the globe does";
                info!(clamped, "the derived sky lens hit the shader's limit");
                note = join_notes(note, clamped.to_owned());
            }
            let pixels = self.export_framed(&derived.framing, bounds.width, bounds.height)?;
            return Ok((
                WallpaperJob {
                    mode: self.display_mode,
                    monitors,
                    anchor: anchor.index,
                    images: JobImages::Spanned {
                        canvas: Arc::new(Frame::new(pixels, bounds.width, bounds.height)),
                        bounds,
                    },
                },
                note,
            ));
        }

        // One render per distinct size, shared by every screen of that size.
        // A screen with no group is one this mode does not paint, and the sink
        // leaves it alone where the desktop lets it.
        let mut images: Vec<Option<Arc<Frame>>> = vec![None; monitors.len()];
        for group in &groups {
            let framing = layout::screen_framing(settings, group.width, group.height);
            let pixels = self.export_framed(&framing, group.width, group.height)?;
            let frame = Arc::new(Frame::new(pixels, group.width, group.height));
            for index in &group.monitors {
                images[*index] = Some(Arc::clone(&frame));
            }
        }
        Ok((
            WallpaperJob {
                mode: self.display_mode,
                monitors,
                anchor: anchor.index,
                images: JobImages::PerMonitor(images),
            },
            note,
        ))
    }

    /// Refuse a render this device cannot make, before it is attempted.
    ///
    /// Both limits are reachable in the span mode and neither fails in a way
    /// anybody could read: past the texture dimension wgpu refuses the texture,
    /// and past the buffer size it panics in the readback.
    pub(super) fn check_export_fits(&self, width: u32, height: u32) -> Result<(), String> {
        let (max_dimension, max_buffer) = self.renderer.export_limits();
        if width > max_dimension || height > max_dimension {
            return Err(format!(
                "a {width}x{height} wallpaper is larger than this GPU renders \
                 ({max_dimension} pixels on a side); one screen at a time will still work"
            ));
        }
        // The readback buffer, not the image: `read_texture_rgba8` pads every
        // row out to 256 bytes, so a width that is not a multiple of 64 pixels
        // costs more than four bytes each. The guard exists to turn an
        // oversized readback into a sentence instead of a panic, which it can
        // only do if it counts the same bytes the allocation does.
        let bytes = (u64::from(width) * 4).next_multiple_of(256) * u64::from(height);
        if bytes > max_buffer {
            return Err(format!(
                "a {width}x{height} wallpaper reads back {bytes} bytes, and this GPU \
                 takes {max_buffer} at once"
            ));
        }
        debug!(width, height, bytes, "wallpaper export budget");
        Ok(())
    }

    /// Replay the current scene with one screen's framing.
    pub(super) fn export_framed(
        &mut self,
        framing: &crate::display::layout::Framing,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        let mut params = self.params;
        params.camera.fov_deg = framing.camera_fov;
        params.sky_fov = framing.sky_fov;
        params.camera.offset_x = framing.offset_x;
        params.camera.offset_y = framing.offset_y;
        self.renderer.export_image_with(&params, width, height)
    }
}
