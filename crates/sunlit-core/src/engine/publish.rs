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
use super::{Engine, EngineEvent, TILE_WAIT, join_notes, save_png, tile_output};
use crate::display::Monitor;
use crate::display::layout::{self, Framing, Rect};
use crate::params::SceneParams;

/// One render a wallpaper is made of: the framing and the size of a group of
/// screens, or of the canvas the screens span.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Shot {
    pub framing: Framing,
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
        reply: Sender<Result<(), String>>,
    },
    Pixels(Sender<Result<Vec<u8>, String>>),
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
        // The month is judged before the textures are: a publish can arrive
        // in a tick with nothing dirty, after the date crossed into the next
        // month, and would otherwise find the old month's floor ready.
        self.sync_month();
        let now = self.clock.elapsed();
        let cubes_pending = self.renderer.textures_pending(self.params.texture_index);
        if cubes_pending {
            self.tiles_awaited_since = None;
        }

        // A debt that already stands has had its support answer and its plan,
        // and the only thing left that can change is whether what it waits
        // for has landed. `tick` comes back here every 50 ms until it is
        // paid, and on Linux `check_supported` walks `PATH` with a stat per
        // directory.
        if self.wallpaper_owed
            && (cubes_pending || (!exiting && self.shots_pending() && !self.tiles_waited_out(now)))
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
        // Draws, and with it computes the wanted set with the plan's renders.
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
        let Some(surface) = &self.surface else {
            return false;
        };
        self.wallpaper_shots.iter().any(|shot| {
            let framed = shot.framing.applied_to(&self.params);
            let output = tile_output(&framed, shot.width, shot.height, true);
            surface
                .missing_for(&output, &self.renderer)
                .is_none_or(|missing| missing > 0)
        })
    }

    /// Whether a tile `export`'s render wants in view is on its way, or the
    /// wanted set in force was computed without it.
    fn export_pending(&self, export: &WaitingExport) -> bool {
        let Some(surface) = &self.surface else {
            return false;
        };
        let output = tile_output(&self.params, export.width, export.height, true);
        surface
            .missing_for(&output, &self.renderer)
            .is_none_or(|missing| missing > 0)
    }

    /// Report a finished publish attempt and settle the debt for it, and for
    /// a wallpaper to be made again.
    pub(super) fn report_wallpaper(&mut self, result: Result<String, String>) {
        self.wallpaper_owed = false;
        self.tiles_awaited_since = None;
        self.reexport = false;
        self.wallpaper_shots.clear();
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
            self.check_export_fits(group.width, group.height)?;
        }

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
            return Ok(WallpaperPlan {
                monitors,
                anchor: anchor.index,
                note,
                shots: vec![Shot {
                    framing: derived.framing,
                    width: bounds.width,
                    height: bounds.height,
                }],
                placement: Placement::Spanned(bounds),
            });
        }

        // One render per distinct size, shared by every screen of that size.
        // A screen with no group is one this mode does not paint, and the sink
        // leaves it alone where the desktop lets it.
        let shots = groups
            .iter()
            .map(|group| Shot {
                framing: layout::screen_framing(settings, group.width, group.height),
                width: group.width,
                height: group.height,
            })
            .collect();
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
        framing: &Framing,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        let framed = framing.applied_to(&self.params);
        self.export_capped(&framed, width, height)
    }

    /// Replay `params` at `width` by `height`, the page table held for this
    /// render alone to the cap of that output (plan departure 23): a render
    /// smaller than another the set was computed for would otherwise read
    /// that one's tiles past their coarser level.
    fn export_capped(
        &mut self,
        params: &SceneParams,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        let cap = self
            .surface
            .as_ref()
            .map(|surface| surface.cap_for(&tile_output(params, width, height, true)));
        self.renderer
            .export_image_capped(params, width, height, cap)
    }

    /// Make an export a caller waits on: now where the globe has no tiles or
    /// its tiles are resident, else once they are, or after [`TILE_WAIT`]
    /// with what is resident in their place. The floors are the caller's to
    /// wait for, through `TexturesReady`.
    pub(super) fn export(&mut self, width: u32, height: u32, reply: ExportReply) {
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

    /// Render an export from the frame just drawn and hand it over.
    pub(super) fn answer(&mut self, export: WaitingExport) {
        let params = self.params;
        let pixels = self.export_capped(&params, export.width, export.height);
        match export.reply {
            ExportReply::Pixels(reply) => {
                let _ = reply.send(pixels);
            }
            ExportReply::File { path, reply } => {
                let written =
                    pixels.and_then(|pixels| save_png(&path, export.width, export.height, &pixels));
                let _ = reply.send(written);
            }
        }
    }
}
