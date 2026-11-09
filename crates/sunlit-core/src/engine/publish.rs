//! What the engine does with a finished frame once the destination is the
//! desktop or a caller rather than the preview.
//!
//! The publish half of [`super::Engine`]: deciding when a wallpaper may go out,
//! saying how it went, noticing that the screens moved, and framing the export
//! the sink asked for; and the exports a caller waits on a reply for, which
//! wait for their tiles the way a wallpaper does.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Sender;
use tracing::{debug, error, info, warn};

use super::wallpaper_sink::{Frame, JobImages, WallpaperJob};
use super::{
    CLOUD_WAIT, Engine, EngineEvent, ExportError, ExportLimit, TILE_WAIT, join_notes, tile_output,
};
use crate::display::Monitor;
use crate::display::layout::{self, Framing, OutputFraming, Rect};
use crate::params::OutputParams;

/// One render a wallpaper is made of: the framing and the size of a group of
/// screens, or of the canvas the screens span.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Shot {
    pub framing: OutputFraming,
    pub width: u32,
    pub height: u32,
}

/// A wallpaper as planned for the monitors this session has now.
pub(super) struct WallpaperPlan {
    monitors: Vec<Monitor>,
    anchor: usize,
    note: String,
    shots: Vec<Shot>,
    placement: Placement,
}

/// Where the shots of a plan go.
enum Placement {
    /// Each shot on the screens at these positions of the monitor list.
    PerMonitor(Vec<Vec<usize>>),
    /// The one shot across the canvas the screens span.
    Spanned(Rect),
}

/// Who waits for an export made on request, and what they are handed.
pub(super) enum ExportReply {
    File {
        path: PathBuf,
        reply: Sender<Result<(), ExportError>>,
    },
    Pixels(Sender<Result<Vec<u8>, ExportError>>),
}

impl ExportReply {
    fn refuse(self, error: ExportError) {
        match self {
            Self::File { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            Self::Pixels(reply) => {
                let _ = reply.send(Err(error));
            }
        }
    }
}

/// A `RenderToFile` or `ExportPixels` request waiting for its tiles.
pub(super) struct WaitingExport {
    pub width: u32,
    pub height: u32,
    /// When it was asked for, or when the tile array was last purged, on the
    /// injected clock.
    since: Duration,
    reply: ExportReply,
}

impl Engine {
    /// Render at the sink's native resolution and hand the pixels over.
    ///
    /// Held back while a texture the current mode needs is on its way. The
    /// renderer falls back to the procedural grid while a slot or a floor is
    /// empty, which is fine for a preview and not fine for someone's desktop,
    /// and a resolution switch empties one for as long as the reload takes.
    /// Every caller that publishes arrives here, so this covers all of them.
    ///
    /// Held back too while a tile the wallpaper's renders want is on its way,
    /// since those cells would draw the floor, but for [`TILE_WAIT`] at the
    /// most, counted from when the cubes are resident: after that it goes out
    /// with what is resident, says so in the log, and is made again once the
    /// tiles are resident. The renders join the wanted set at its front from
    /// the moment they are planned, while a cube is still on its way too.
    ///
    /// Held back as well until the cloud layer has had its first answer, a
    /// map drawn or nothing to draw, but for [`CLOUD_WAIT`] at the most,
    /// counted from when the cubes are resident: after that it goes out
    /// without clouds and says so in the log. The wait is for the first
    /// answer only, so a later poll that is slow or failing holds nothing.
    ///
    /// One request is remembered, not a queue of them: two wallpaper updates
    /// asked for during one reload are the same wallpaper.
    pub(super) fn publish_wallpaper(&mut self) {
        self.publish(false);
    }

    /// Publish the wallpaper owed or asked for when the engine stops, with
    /// what is resident: there is no tick left to wait for tiles in. A cube
    /// still on its way holds it back as ever, since the globe would be the
    /// grid.
    pub(super) fn publish_wallpaper_at_exit(&mut self) {
        self.publish(true);
    }

    fn publish(&mut self, exiting: bool) {
        if self.held.is_some() && !exiting {
            self.publish_asked = true;
            return;
        }
        // The month is judged before the textures are: a publish can arrive
        // in a tick with nothing dirty, after the date crossed into the next
        // month, and would otherwise find the old month's floor ready.
        self.sync_month(self.setting_instant());
        let now = self.clock.elapsed();
        let cubes_pending = self.renderer.textures_pending(&self.params);
        if cubes_pending {
            self.tiles_awaited_since = None;
            self.clouds_awaited_since = None;
        }

        // A debt that already stands has had its support answer and its plan,
        // and the only thing left that can change is whether what it waits
        // for has landed. `tick` comes back here every 50 ms until it is
        // paid, and on Linux `check_supported` walks `PATH` with a stat per
        // directory.
        if self.wallpaper_owed
            && (cubes_pending
                || (!exiting && self.clouds_pending() && !self.clouds_waited_out(now))
                || (!exiting && self.shots_pending() && !self.tiles_waited_out(now)))
        {
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

        let plan = match self.plan_wallpaper() {
            Ok(plan) => plan,
            Err(e) => {
                self.report_wallpaper(Err(e));
                return;
            }
        };
        self.wallpaper_shots.clone_from(&plan.shots);
        if cubes_pending {
            if !self.wallpaper_owed {
                info!("wallpaper update deferred until the textures have loaded");
            }
            self.wallpaper_owed = true;
            self.want_now();
            return;
        }
        // Judged only with the cubes resident, so that a first run spending
        // minutes on its packs does not spend the cloud budget on them. The
        // tiles the renders want are read in the meantime, since the shots
        // are in the wanted set from the moment they are planned.
        if self.clouds_pending() && !exiting && !self.clouds_waited_out(now) {
            self.wallpaper_owed = true;
            self.want_now();
            return;
        }
        // Draws, and with it computes the wanted set with the plan's renders.
        // A map that landed since the hold was judged is drawn by it.
        self.prepare_export();
        let missing = self.shots_pending();
        if missing && exiting {
            warn!(
                "the tiles the wallpaper shows are not all resident as the engine stops; it \
                 goes out with what is resident in their place"
            );
        } else if missing {
            if !self.tiles_waited_out(now) {
                if !self.wallpaper_owed {
                    info!("wallpaper update deferred until the tiles it shows are resident");
                }
                self.wallpaper_owed = true;
                return;
            }
            warn!(
                waited_secs = TILE_WAIT.as_secs(),
                "the tiles the wallpaper shows did not all arrive in time; it goes out with \
                 what is resident in their place, and is made again once they are"
            );
        }

        if self.clouds_pending() {
            if exiting {
                warn!(
                    "the cloud layer has not settled as the engine stops; the wallpaper goes out \
                     without clouds"
                );
            } else {
                warn!(
                    waited_secs = CLOUD_WAIT.as_secs(),
                    "the cloud layer did not settle in time; the wallpaper goes out without \
                     clouds, and the next update has them"
                );
            }
        }

        let shots = plan.shots.clone();
        let result = self
            .render_plan(plan)
            .and_then(|(job, note)| Ok(join_notes(note, self.wallpaper.publish(&job)?)));
        let again = missing && result.is_ok();
        self.report_wallpaper(result);
        if again {
            self.reexport = true;
            self.wallpaper_shots = shots;
        }
    }

    /// Whether the cloud layer is still owed a first answer that a wallpaper
    /// should wait for: one is drawn, there is a source to draw it from, and
    /// `CloudsSettled` has not gone out. Without the first two the event never
    /// comes, and after it a slow, failing or in-flight poll holds nothing, so
    /// a session offline for hours still publishes on schedule.
    fn clouds_pending(&self) -> bool {
        self.cloud.is_some() && self.params.draws_clouds() && !self.clouds_settled
    }

    /// Whether the wallpaper owed has waited [`CLOUD_WAIT`] for the cloud layer
    /// by `now`. The wait starts the first time this asks with the cubes
    /// resident, and the log says the update is deferred at that moment, once
    /// per wait.
    fn clouds_waited_out(&mut self, now: Duration) -> bool {
        let since = *self.clouds_awaited_since.get_or_insert_with(|| {
            info!("wallpaper update deferred until the cloud layer has settled");
            now
        });
        now.saturating_sub(since) >= CLOUD_WAIT
    }

    /// Whether the wallpaper owed has waited [`TILE_WAIT`] for its tiles by
    /// `now`. The wait starts the first time this asks with the cubes
    /// resident, and starts over when the tiles it waits for could newly be
    /// read since: the month in force changed, its pack or the night's was
    /// opened, or the tile array was purged (plan departures 30 and 31).
    fn tiles_waited_out(&mut self, now: Duration) -> bool {
        let opened = self
            .surface
            .as_ref()
            .map_or(0, super::surface::SurfaceFeed::renewals);
        let since = match self.tiles_awaited_since {
            Some((since, at)) if at == opened => since,
            _ => {
                self.tiles_awaited_since = Some((now, opened));
                now
            }
        };
        now.saturating_sub(since) >= TILE_WAIT
    }

    /// Whether a tile one of the wallpaper's renders wants in view is on its
    /// way, or the wanted set in force was computed without them.
    pub(super) fn shots_pending(&self) -> bool {
        self.wallpaper_shots.iter().any(|shot| {
            let framed = shot.framing.applied_to(&self.params);
            self.output_pending(&framed, shot.width, shot.height)
        })
    }

    /// Whether a tile `export`'s render wants in view is on its way, or the
    /// wanted set in force was computed without it.
    fn export_pending(&self, export: &WaitingExport) -> bool {
        let framed = self
            .export_framing(export.width, export.height)
            .applied_to(&self.params);
        self.output_pending(&framed, export.width, export.height)
    }

    /// Whether a tile an export of `framed` at `width` by `height` wants in
    /// view is on its way, or the wanted set in force was computed without
    /// it. The output is resolved under the sky that set was computed under,
    /// which is the one it was computed for.
    fn output_pending(&self, framed: &OutputParams, width: u32, height: u32) -> bool {
        let Some(surface) = &self.surface else {
            return false;
        };
        self.wanted_sky.as_ref().is_none_or(|sky| {
            let output = tile_output(framed, sky, width, height, true);
            surface
                .missing_for(&output, &self.renderer)
                .is_none_or(|missing| missing > 0)
        })
    }

    /// Report a finished publish attempt and settle the debt for it, and for
    /// a wallpaper to be made again.
    pub(super) fn report_wallpaper(&mut self, result: Result<String, String>) {
        self.wallpaper_owed = false;
        self.tiles_awaited_since = None;
        self.clouds_awaited_since = None;
        self.reexport = false;
        self.wallpaper_shots.clear();
        if self.excursion.is_some() {
            // The wanted set took the setting's month and Sun while the shots
            // waited, and the preview's come back only with a draw.
            self.dirty = true;
        }
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
        self.see_monitors(&monitors);
        self.emit(EngineEvent::MonitorsChanged(monitors));
        if self.published {
            self.publish_wallpaper();
        }
    }

    /// Plan what this session's monitors need, and say what was odd.
    ///
    /// The monitor list is asked for on every publish rather than cached: the
    /// layout changes without telling anybody, and the auto-refresh means a
    /// stale one would be on the screen for as long as the interval.
    fn plan_wallpaper(&mut self) -> Result<WallpaperPlan, String> {
        let monitors = self.wallpaper.monitors()?;
        // The list a publish planned with is the one a later hint is compared
        // against, so a layout that kept moving is published again only if it
        // is different again.
        self.known_monitors = Some(monitors.clone());
        self.see_monitors(&monitors);
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

        let settings = Framing::from(&self.params);
        let groups = layout::render_groups(&monitors, self.display_mode, anchor.index);
        if groups.is_empty() {
            return Err("this session has no screen with any pixels on it".to_owned());
        }
        for group in &groups {
            self.check_export_fits(group.width, group.height)
                .map_err(wallpaper_limit)?;
        }

        if self.display_mode == layout::DisplayMode::AcrossScreens {
            let bounds = layout::bounds_of(&monitors)
                .ok_or_else(|| "this session has no screen with any pixels on it".to_owned())?;
            let derived = layout::canvas_framing(settings, monitors[anchor.index].rect(), bounds);
            note = join_notes(note, clamp_note(&derived));
            return Ok(WallpaperPlan {
                monitors,
                anchor: anchor.index,
                note,
                shots: vec![Shot {
                    framing: derived,
                    width: bounds.width,
                    height: bounds.height,
                }],
                placement: Placement::Spanned(bounds),
            });
        }

        // One render per distinct size, shared by every screen of that size.
        // A screen with no group is one this mode does not paint, and the sink
        // leaves it alone where the desktop lets it.
        let shots: Vec<Shot> = groups
            .iter()
            .map(|group| Shot {
                framing: layout::screen_framing(settings, group.width, group.height),
                width: group.width,
                height: group.height,
            })
            .collect();
        if let Some(clamped) = shots.iter().find(|shot| shot.framing.clamped()) {
            note = join_notes(note, clamp_note(&clamped.framing));
        }
        Ok(WallpaperPlan {
            monitors,
            anchor: anchor.index,
            note,
            shots,
            placement: Placement::PerMonitor(
                groups.into_iter().map(|group| group.monitors).collect(),
            ),
        })
    }

    /// Render every shot of `plan` from the frame just drawn, and put the
    /// job together.
    fn render_plan(&mut self, plan: WallpaperPlan) -> Result<(WallpaperJob, String), String> {
        let mut frames = Vec::with_capacity(plan.shots.len());
        for shot in &plan.shots {
            let pixels = self.export_framed(&shot.framing, shot.width, shot.height)?;
            frames.push(Arc::new(Frame::new(pixels, shot.width, shot.height)));
        }
        let images = match plan.placement {
            Placement::Spanned(bounds) => JobImages::Spanned {
                canvas: frames
                    .pop()
                    .ok_or_else(|| "a spanned plan with no render".to_owned())?,
                bounds,
            },
            Placement::PerMonitor(groups) => {
                let mut images: Vec<Option<Arc<Frame>>> = vec![None; plan.monitors.len()];
                for (frame, group) in frames.iter().zip(&groups) {
                    for index in group {
                        images[*index] = Some(Arc::clone(frame));
                    }
                }
                JobImages::PerMonitor(images)
            }
        };
        Ok((
            WallpaperJob {
                mode: self.display_mode,
                monitors: plan.monitors,
                anchor: plan.anchor,
                images,
            },
            plan.note,
        ))
    }

    /// Refuse a render this device cannot make, before it is attempted.
    ///
    /// Both limits are reachable in the span mode and by an export of a size
    /// typed in, and neither fails in a way anybody could read: past the
    /// texture dimension wgpu refuses the texture, and past the buffer size it
    /// panics in the readback.
    pub(super) fn check_export_fits(&self, width: u32, height: u32) -> Result<(), ExportLimit> {
        let (max_dimension, max_buffer) = self.renderer.export_limits();
        let fits = export_fits(width, height, max_dimension, max_buffer);
        if fits.is_ok() {
            debug!(width, height, "an export within the device's limits");
        }
        fits
    }

    /// Replay the current scene with one screen's framing.
    pub(super) fn export_framed(
        &mut self,
        framing: &OutputFraming,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        let framed = framing.applied_to(&self.params);
        self.export_capped(&framed, width, height)
    }

    /// Replay `params` at `width` by `height`, the page table held for this
    /// render alone to the cap of that output (plan departure 23): a render
    /// smaller than another the set was computed for would otherwise read
    /// that one's tiles past their coarser level. The output is resolved under
    /// the sky the renderer holds, which is the one the replay draws under.
    fn export_capped(
        &mut self,
        params: &OutputParams,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        let cap = self
            .surface
            .as_ref()
            .zip(self.renderer.sky_state())
            .map(|(surface, sky)| surface.cap_for(&tile_output(params, sky, width, height, true)));
        self.renderer
            .export_image_capped(params, width, height, cap)
    }

    /// Make an export a caller waits on: now where the globe has no tiles or
    /// its tiles are resident, else once they are, or after [`TILE_WAIT`]
    /// with what is resident in their place. The floors are the caller's to
    /// wait for, through `TexturesReady`.
    pub(super) fn export(&mut self, width: u32, height: u32, reply: ExportReply) {
        if let Err(limit) = self.check_export_fits(width, height) {
            warn!(width, height, %limit, "an export is past the device's limits");
            reply.refuse(ExportError::TooLarge(limit));
            return;
        }
        let export = WaitingExport {
            width,
            height,
            since: self.clock.elapsed(),
            reply,
        };
        if self.surface.is_none() {
            self.prepare_export();
            self.answer(export);
            return;
        }
        self.exports.push(export);
        self.settle_exports();
    }

    /// Start the tile wait of every waiting export over, for a purge of the
    /// tile array that let go of whatever of their tiles had landed.
    pub(super) fn restart_export_waits(&mut self) {
        let now = self.clock.elapsed();
        for export in &mut self.exports {
            export.since = now;
        }
    }

    /// Answer every waiting export whose own tiles are resident, or that has
    /// waited [`TILE_WAIT`] for them.
    pub(super) fn settle_exports(&mut self) {
        if self.exports.is_empty() {
            return;
        }
        // Draws, and with it computes the wanted set with every waiting
        // export's output.
        self.prepare_export();
        let now = self.clock.elapsed();
        let exports: Vec<_> = std::mem::take(&mut self.exports)
            .into_iter()
            .map(|export| {
                let pending = self.export_pending(&export);
                (export, pending)
            })
            .collect();
        for (export, pending) in exports {
            if pending && now.saturating_sub(export.since) < TILE_WAIT {
                self.exports.push(export);
                continue;
            }
            if pending {
                warn!(
                    width = export.width,
                    height = export.height,
                    waited_secs = TILE_WAIT.as_secs(),
                    "the tiles an export shows did not all arrive in time; it is made with \
                     what is resident in their place"
                );
            }
            self.answer(export);
        }
    }

    /// The framing an export of `width` by `height` takes: a screen's of
    /// that size, so a render made on request is the picture a wallpaper of
    /// that size would be.
    fn export_framing(&self, width: u32, height: u32) -> OutputFraming {
        layout::screen_framing(Framing::from(&self.params), width, height)
    }

    /// Render an export from the frame just drawn and hand it over.
    pub(super) fn answer(&mut self, export: WaitingExport) {
        let framing = self.export_framing(export.width, export.height);
        if framing.clamped() {
            warn!(
                width = export.width,
                height = export.height,
                camera_clamped = framing.camera_clamped,
                sky_clamped = framing.sky_clamped,
                "an export's derived lens is past what the renderer takes; it is drawn at \
                 the nearest one it does"
            );
        }
        let pixels = self
            .export_framed(&framing, export.width, export.height)
            .map_err(ExportError::Failed);
        match export.reply {
            ExportReply::Pixels(reply) => {
                let _ = reply.send(pixels);
            }
            ExportReply::File { path, reply } => {
                let written = pixels.and_then(|pixels| {
                    Ok(crate::image_file::write_png(
                        &path,
                        &pixels,
                        export.width,
                        export.height,
                    )?)
                });
                let _ = reply.send(written);
            }
        }
    }
}

/// Whether a render of `width` by `height` fits a device whose textures are at
/// most `max_side` on a side and whose buffers hold at most `max_buffer`
/// bytes.
///
/// The readback buffer is what is counted, not the image: `read_texture_rgba8`
/// pads every row out to 256 bytes, so a width that is not a multiple of 64
/// pixels costs more than four bytes each. The guard exists to turn an
/// oversized readback into a sentence instead of a panic, which it can only do
/// if it counts the same bytes the allocation does.
fn export_fits(width: u32, height: u32, max_side: u32, max_buffer: u64) -> Result<(), ExportLimit> {
    if width > max_side || height > max_side {
        return Err(ExportLimit::Side {
            width,
            height,
            max_side,
        });
    }
    let readback =
        |width: u32, height: u32| (u64::from(width) * 4).next_multiple_of(256) * u64::from(height);
    if readback(width, height) > max_buffer {
        let mut largest_square = max_side;
        while largest_square > 0 && readback(largest_square, largest_square) > max_buffer {
            largest_square -= 1;
        }
        return Err(ExportLimit::Readback {
            width,
            height,
            largest_square,
        });
    }
    Ok(())
}

/// What a wallpaper's status says about a render past the device's limits.
fn wallpaper_limit(limit: ExportLimit) -> String {
    match limit {
        ExportLimit::Side {
            width,
            height,
            max_side,
        } => format!(
            "a {width}x{height} wallpaper is larger than this GPU renders \
             ({max_side} pixels on a side); one screen at a time will still work"
        ),
        ExportLimit::Readback { width, height, .. } => format!(
            "a {width}x{height} wallpaper is more than this GPU reads back at once; \
             one screen at a time will still work"
        ),
    }
}

/// What a wallpaper's status note says about a lens one of its renders could
/// not honor, or nothing.
///
/// The globe and the sky are still drawn; what is lost is that the clamped
/// lens draws its half of the picture at another scale than the settings ask
/// for, so the screens or the span no longer match exactly.
fn clamp_note(framing: &OutputFraming) -> String {
    let note = match (framing.camera_clamped, framing.sky_clamped) {
        (false, false) => return String::new(),
        (true, false) => {
            "the globe's lens is as wide as it goes on this screen, so the globe is \
                          drawn larger than the other screens draw it"
        }
        (false, true) => {
            "the sky is as wide as it goes, so it does not continue across the \
                          screens as exactly as the globe does"
        }
        (true, true) => {
            "the globe's lens and the sky are both as wide as they go, so neither \
                         matches the other screens exactly"
        }
    };
    info!(
        note,
        camera_clamped = framing.camera_clamped,
        sky_clamped = framing.sky_clamped,
        "a derived lens hit the renderer's limit"
    );
    note.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIDE: u32 = 16_384;
    const BUFFER: u64 = 256 << 20;

    #[test]
    fn a_side_past_the_texture_limit_is_refused() {
        assert_eq!(
            export_fits(20_000, 1000, SIDE, BUFFER),
            Err(ExportLimit::Side {
                width: 20_000,
                height: 1000,
                max_side: SIDE
            })
        );
        assert!(matches!(
            export_fits(100, SIDE + 1, SIDE, BUFFER),
            Err(ExportLimit::Side { .. })
        ));
    }

    #[test]
    fn the_readback_counts_the_padded_rows() {
        let Err(ExportLimit::Readback { largest_square, .. }) =
            export_fits(12_000, 6_000, SIDE, BUFFER)
        else {
            panic!("12000 x 6000 is past 256 MiB");
        };
        assert!(export_fits(largest_square, largest_square, SIDE, BUFFER).is_ok());
        assert!(export_fits(largest_square + 1, largest_square + 1, SIDE, BUFFER).is_err());

        let (width, height) = (8190, 8193);
        assert!(
            u64::from(width) * 4 * u64::from(height) <= BUFFER,
            "the premise: four bytes a pixel would fit"
        );
        assert!(matches!(
            export_fits(width, height, SIDE, BUFFER),
            Err(ExportLimit::Readback { .. })
        ));
    }

    #[test]
    fn sizes_within_both_limits_pass() {
        assert_eq!(export_fits(3840, 2160, SIDE, BUFFER), Ok(()));
        assert_eq!(export_fits(8192, 8192, SIDE, BUFFER), Ok(()));
    }
}
