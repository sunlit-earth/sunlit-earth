//! What crosses the engine's channel in each direction.
//!
//! One file for both halves because they are one contract: a client sends an
//! [`EngineCommand`] and hears back through an [`EngineEvent`], and neither is
//! useful without the other.

use std::path::PathBuf;
use std::time::Duration;

use crossbeam_channel::Sender;

use crate::memory_report::MemoryReport;
use crate::params::SceneParams;

/// Things a client asks the engine to do.
pub enum EngineCommand {
    /// Replace the scene parameters. Coalesced: only the newest survives a
    /// backlog, because intermediate drag positions are not worth rendering.
    UpdateParams(Box<SceneParams>),
    /// Resize the preview target. Quantized by the engine.
    SetPreviewSize(u32, u32),
    /// Turn preview frames on or off. Off saves the readback when nothing is
    /// looking (the window is hidden, or the client is the render subcommand).
    SetPreviewEnabled(bool),
    /// Render at the sink's native resolution and publish it now.
    RenderWallpaperNow,
    /// Render to a PNG file at an explicit size and report back.
    RenderToFile {
        path: PathBuf,
        width: u32,
        height: u32,
        reply: Sender<Result<(), String>>,
    },
    /// Render at an explicit size and hand the raw pixels back. Used by the
    /// e2e `export-test` probe, which checks that the GPU path still works
    /// while the window is hidden.
    ExportPixels {
        width: u32,
        height: u32,
        reply: Sender<Result<Vec<u8>, String>>,
    },
    /// Reload the file-backed textures at a different width.
    ///
    /// Not part of `UpdateParams`: the width does not describe what to draw, it
    /// decides which pixels to load, and acting on it means freeing GPU
    /// textures and re-reading files, which a parameter push cannot express.
    SetTextureResolution(u32),
    /// Assemble a memory report and hand it back.
    ///
    /// Answered on the engine thread because the device is owned there, in the
    /// same reply-channel shape as `ExportPixels`.
    ReportMemory { reply: Sender<Box<MemoryReport>> },
    /// Turn the unattended wallpaper refresh on or off.
    SetAutoRefresh { enabled: bool, interval: Duration },
    /// Replace the mode and the anchor a wallpaper is planned with.
    ///
    /// Not part of `UpdateParams` for the same reason `SetTextureResolution` is
    /// not: neither field describes what to draw, so neither belongs in
    /// `SceneParams` or in the digest that decides whether a frame is worth
    /// rendering. Both are read only when a wallpaper is published.
    SetDisplayPlan {
        mode: crate::display::layout::DisplayMode,
        anchor: Option<String>,
    },
    /// The display layout may have moved. A hint and nothing more: it carries
    /// no list, because the watcher that sends it has no opinion about what
    /// changed and the engine asks the sink itself once the burst has settled.
    DisplaysChanged,
    /// Re-evaluate the schedule now. Tests send this after advancing a mock
    /// clock; production uses it as a "something happened" nudge.
    Poke,
    /// Finish the current iteration and stop.
    Shutdown,
}

/// Things the engine tells its clients about.
pub enum EngineEvent {
    /// A freshly rendered preview frame, as tightly packed RGBA8.
    PreviewFrame {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// Every texture the current mode needs has finished loading. Fires once
    /// per set of textures, so again after a resolution change has reloaded
    /// them.
    TexturesReady,
    /// A wallpaper publish attempt finished.
    ///
    /// `Ok` carries what the desktop could not do, and is empty where it did
    /// exactly what the mode asked. A desktop with one wallpaper for every
    /// screen has not failed by giving them all the same image, but somebody
    /// looking at three identical screens deserves the sentence that says why.
    WallpaperSet(Result<String, String>),
    /// The monitors this session has, after a hint turned out to be a real
    /// change. Carries the list the engine will plan its next wallpaper with,
    /// so a client showing the layout does not have to query for itself.
    MonitorsChanged(Vec<crate::display::Monitor>),
    /// Loading-indicator text; empty when nothing is loading.
    Status(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The unbounded-channel justification at the `start` declaration rests on
    /// commands being small. This pin catches inline-size regressions such as
    /// un-boxing `SceneParams` (about 200 bytes). It cannot catch a variant
    /// that carries a heap buffer (`Vec<u8>` is 24 bytes inline), so a command
    /// that transported pixels would pass; the review guard for that is the
    /// justification comment itself, which any such variant must update.
    #[test]
    fn command_payload_is_small() {
        let size = std::mem::size_of::<EngineCommand>();
        assert!(
            size <= 64,
            "EngineCommand grew to {size} bytes inline; revisit the unbounded \
             channel justification in `start`"
        );
    }
}
