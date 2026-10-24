//! What to draw for a set of monitors: how large, how framed, and which image
//! goes to which screen.
//!
//! Every function here is pure. A monitor list arrives as fabricated data in a
//! unit test exactly as it arrives from `EnumDisplayMonitors` or from xrandr, so
//! the whole model is exercised on a machine with no display attached, and the
//! only per-OS work left is filling the list in and handing the finished images
//! back out.
//!
//! The settings measure both fields of view across the picture's short edge
//! and the pan in halves of it, so that every output, whether the preview, a
//! screen or an export, is the same picture scaled until the short edges match
//! and differs from the others only in how far it reaches along its long edge.
//! The renderer takes each lens on its own axis instead:
//!
//! - The Earth lens is `Mat4::perspective_rh(fov_y, aspect, ..)`, so its scale in
//!   pixels per unit of tan-space is `height / (2 * tan(fov / 2))` and does not
//!   depend on the width at all.
//! - The sky lens is stereographic and anchored the other way: `sphere.wgsl`
//!   builds its NDC as `radial * r / edge * vec2(1, aspect)`, which puts the
//!   frame's *horizontal* edge at `tan(sky_fov / 4)`, so its scale is
//!   `width / (2 * tan(sky_fov / 4))` and does not depend on the height.
//! - The pan is in NDC, halves of the frame on each axis.
//!
//! [`output_framing`] is the one translation from the first set of units to
//! the second, and [`OutputParams`] is the proof that a render went through it.

use serde::{Deserialize, Serialize};

use super::Monitor;
use crate::params::{OutputParams, SceneParams};

/// How the monitors of one session relate to each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayMode {
    /// One screen gets the picture; the rest are left as they are.
    OneScreen,
    /// The same view on every screen, each at its own size and aspect ratio.
    ///
    /// The default, and identical to [`DisplayMode::OneScreen`] on a session
    /// with one monitor, which is why no migration is needed for one.
    #[default]
    EveryScreen,
    /// One continuous view over the whole virtual desktop, cut per monitor.
    AcrossScreens,
}

impl DisplayMode {
    /// Every mode, in the order the settings window offers them.
    pub const ALL: [Self; 3] = [Self::OneScreen, Self::EveryScreen, Self::AcrossScreens];

    /// What the settings window calls this.
    ///
    /// One or two words, because the combo is narrow enough to cut a sentence
    /// off: the row's hint is where the sentence lives. `Mirror` and `Extend`
    /// are what a display settings panel already calls these two arrangements,
    /// so they read as the terms they are rather than as abbreviations.
    pub fn label(self) -> &'static str {
        match self {
            Self::OneScreen => "One screen",
            Self::EveryScreen => "Mirror",
            Self::AcrossScreens => "Extend",
        }
    }

    /// This mode's position in [`DisplayMode::ALL`].
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|mode| *mode == self)
            .expect("every mode is in ALL")
    }

    /// The mode at a position in [`DisplayMode::ALL`], or the default.
    pub fn from_index(index: i32) -> Self {
        usize::try_from(index)
            .ok()
            .and_then(|index| Self::ALL.get(index).copied())
            .unwrap_or_default()
    }

    /// The name this takes in the config file.
    pub fn name(self) -> &'static str {
        match self {
            Self::OneScreen => "one-screen",
            Self::EveryScreen => "every-screen",
            Self::AcrossScreens => "across-screens",
        }
    }

    /// The mode a config file's name refers to, where it names one.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL.into_iter().find(|mode| mode.name() == name)
    }
}

impl Serialize for DisplayMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

/// A name nothing answers to is the default rather than a load failure.
///
/// The rest of the config is worth keeping: a file written by a newer build, or
/// one somebody edited by hand, must not cost a person every other setting they
/// have. The same reasoning `sanitize` applies to a number out of range.
impl<'de> Deserialize<'de> for DisplayMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(Self::from_name(&name).unwrap_or_else(|| {
            tracing::warn!(
                mode = %name,
                "the config names a display mode this build does not have; \
                 using the default"
            );
            Self::default()
        }))
    }
}

/// A rectangle in the virtual desktop, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /// The first column past this rectangle, wide enough not to wrap.
    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    /// The first row past this rectangle.
    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    /// Whether this rectangle has any pixels in it.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// This rectangle relative to another one's origin.
    #[must_use]
    pub(crate) fn relative_to(&self, origin: &Self) -> Self {
        Self {
            x: self.x - origin.x,
            y: self.y - origin.y,
            width: self.width,
            height: self.height,
        }
    }
}

/// The smallest rectangle containing every monitor that has pixels.
///
/// `None` for a list with nothing usable in it, which is a list with no
/// monitors or one where every monitor has a zero dimension. A monitor with a
/// zero dimension is dropped rather than allowed to pull the bounding box
/// toward its own origin, because a screen with no pixels is not somewhere a
/// wallpaper goes.
pub fn bounds_of(monitors: &[Monitor]) -> Option<Rect> {
    let mut rects = monitors
        .iter()
        .map(Monitor::rect)
        .filter(|rect| !rect.is_empty());
    let first = rects.next()?;
    let (mut left, mut top) = (i64::from(first.x), i64::from(first.y));
    let (mut right, mut bottom) = (first.right(), first.bottom());
    for rect in rects {
        left = left.min(i64::from(rect.x));
        top = top.min(i64::from(rect.y));
        right = right.max(rect.right());
        bottom = bottom.max(rect.bottom());
    }
    Some(Rect {
        x: clamp_to_i32(left),
        y: clamp_to_i32(top),
        width: clamp_to_u32(right - left),
        height: clamp_to_u32(bottom - top),
    })
}

fn clamp_to_i32(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value < 0 { i32::MIN } else { i32::MAX })
}

fn clamp_to_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(if value < 0 { 0 } else { u32::MAX })
}

/// Which monitor a plan is built around, and whether it is the one that was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    /// Its position in the monitor list.
    pub index: usize,
    /// A stored anchor named a monitor this session does not have.
    ///
    /// The plan is built around the system primary instead, and the status line
    /// says so: drawing on a different screen without a word is the failure this
    /// flag exists to prevent.
    pub fell_back: bool,
}

/// The monitor a plan is anchored to.
///
/// A stored id that is still here wins; anything else falls back to the system
/// primary, or to the first monitor where nothing is marked primary, which is
/// the fallback [`super::primary_of`] already makes for xrandr.
pub fn resolve_anchor(monitors: &[Monitor], stored: Option<&str>) -> Option<Anchor> {
    let asked = stored.filter(|id| !id.is_empty());
    if let Some(index) = asked.and_then(|id| monitors.iter().position(|m| m.id == id)) {
        return Some(Anchor {
            index,
            fell_back: false,
        });
    }
    let index = monitors
        .iter()
        .position(|monitor| monitor.primary)
        .or_else(|| (!monitors.is_empty()).then_some(0))?;
    Some(Anchor {
        index,
        fell_back: asked.is_some(),
    })
}

/// The framing values a render takes from the settings, before any output is
/// known.
///
/// Every one of them is measured on the picture's short edge, so that every
/// output is the same picture scaled until the short edges match: the two
/// fields of view span it, and the pan is in halves of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Framing {
    /// Field of view of the Earth lens across the short edge, in degrees.
    pub camera_fov: f32,
    /// Field of view of the sky lens across the short edge, in degrees.
    pub sky_fov: f32,
    /// Pan, in halves of the short edge, as `SceneParams` carries it.
    pub offset_x: f32,
    pub offset_y: f32,
}

impl From<&SceneParams> for Framing {
    /// The four values a render takes from the settings, read in one place.
    ///
    /// `sunlit-earth displays` prints the plan this leads to and the engine
    /// renders it. Two readings of `SceneParams` would let the printed plan and
    /// the drawn one drift the moment a fifth field is added here.
    fn from(params: &SceneParams) -> Self {
        Self {
            camera_fov: params.camera.fov_deg,
            sky_fov: params.sky_fov,
            offset_x: params.camera.offset_x,
            offset_y: params.camera.offset_y,
        }
    }
}

/// The framing one output is rendered with, in the units the shader takes,
/// and whether either lens could not be honored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputFraming {
    /// Vertical field of view of the Earth lens, in degrees.
    pub camera_fov: f32,
    /// Horizontal field of view of the sky lens, in degrees.
    pub sky_fov: f32,
    /// Pan, in the output's NDC.
    pub offset_x: f32,
    pub offset_y: f32,
    /// The derived Earth lens was wider than [`CAMERA_FOV_OUTPUT_MAX`].
    pub camera_clamped: bool,
    /// The derived sky lens was outside what the shader accepts, so the sky
    /// is drawn at another scale than the other outputs draw it.
    pub sky_clamped: bool,
}

impl OutputFraming {
    /// `params` with this framing in place of its own: what the render of one
    /// output draws, and what the wanted set of its tiles is computed from.
    ///
    /// The one way to an [`OutputParams`], which is what every render takes.
    #[must_use]
    pub fn applied_to(&self, params: &SceneParams) -> OutputParams {
        let mut framed = *params;
        framed.camera.fov_deg = self.camera_fov;
        framed.sky_fov = self.sky_fov;
        framed.camera.offset_x = self.offset_x;
        framed.camera.offset_y = self.offset_y;
        OutputParams::framed(framed)
    }

    /// Whether either lens was clamped.
    pub fn clamped(&self) -> bool {
        self.camera_clamped || self.sky_clamped
    }
}

/// The widest sky the shader will accept, and the narrowest.
///
/// `sphere.wgsl` clamps `sky_fov` to this range before taking the lens radius,
/// so a derived value outside it is a value the shader silently will not honor
/// and the derivation has to say so instead.
///
/// Neither end is the slider's. The slider means the short edge; this bounds
/// the horizontal value an output derives from it. A wide output or a span
/// derives a wider one, and what bounds that is the stereographic lens going
/// singular at 360 rather than anything happening at 180. What is linear in
/// output pixels is `tan(sky_fov / 4)`, so the derived angle self-limits as
/// the output widens, and 330 clears a seven-wide span at the widest slider
/// position. A narrow output derives a narrower one, down to 7.5 degrees for
/// the narrowest slider position on a 1:4 preview, so the lower end's only job
/// is keeping the tangent off zero. `docs/rendering.md` carries the measured
/// widths.
pub(crate) const SKY_FOV_MIN: f32 = 1.0;
pub(crate) const SKY_FOV_MAX: f32 = 330.0;

/// The widest Earth lens an output derives, in degrees.
///
/// Not the slider's 170: a portrait output widens the vertical lens past the
/// setting, and a span past the anchor's. The projection goes singular at 180,
/// and only a setting near the slider's top on a strongly portrait output comes
/// this close.
pub(crate) const CAMERA_FOV_OUTPUT_MAX: f32 = 179.0;

/// A pixel extent as a float.
///
/// Every extent in this module is a `u32` of pixels and every scale it takes
/// part in is an `f32`, so the narrowing happens here rather than once per
/// formula. Exact below 2^24, which is eight times the widest screen made.
#[expect(
    clippy::cast_precision_loss,
    reason = "the one place a pixel extent becomes a float"
)]
fn px(pixels: u32) -> f32 {
    pixels as f32
}

/// A pixel coordinate as a float, the same narrowing on the signed half of a
/// `Rect`.
#[expect(
    clippy::cast_precision_loss,
    reason = "the one place a pixel coordinate becomes a float"
)]
fn px_at(coordinate: i32) -> f32 {
    coordinate as f32
}

/// A perspective lens of `fov` degrees, widened or narrowed by `scale` in
/// tan-space: the lens that shows `scale` times as far out at the same
/// magnification.
fn scale_perspective(fov: f32, scale: f32) -> f32 {
    ((fov.to_radians() * 0.5).tan() * scale).atan().to_degrees() * 2.0
}

/// The stereographic counterpart of [`scale_perspective`], whose image plane
/// is linear in `tan(fov / 4)`.
fn scale_stereographic(fov: f32, scale: f32) -> f32 {
    ((fov.to_radians() * 0.25).tan() * scale)
        .atan()
        .to_degrees()
        * 4.0
}

/// The framing an output of `width` by `height` takes from the settings, with
/// `short_edge` the length in that output's pixels the settings are measured
/// on.
///
/// The Earth lens is vertical and the sky lens horizontal in the units the
/// shader takes, so each is scaled from the short edge to its own axis, and
/// the pan from halves of the short edge to the output's NDC. The principal
/// point stays at the output's center. An output with no pixels, or no short
/// edge, takes the settings as they are: there is nothing to draw it on.
pub fn output_framing(
    settings: Framing,
    width: u32,
    height: u32,
    short_edge: f32,
) -> OutputFraming {
    if width == 0 || height == 0 || short_edge <= 0.0 {
        return OutputFraming {
            camera_fov: settings.camera_fov,
            sky_fov: settings.sky_fov,
            offset_x: settings.offset_x,
            offset_y: settings.offset_y,
            camera_clamped: false,
            sky_clamped: false,
        };
    }
    let (width, height) = (px(width), px(height));
    let camera_fov = scale_perspective(settings.camera_fov, height / short_edge);
    let sky_fov = scale_stereographic(settings.sky_fov, width / short_edge);
    OutputFraming {
        camera_fov: camera_fov.min(CAMERA_FOV_OUTPUT_MAX),
        sky_fov: sky_fov.clamp(SKY_FOV_MIN, SKY_FOV_MAX),
        offset_x: settings.offset_x * short_edge / width,
        offset_y: settings.offset_y * short_edge / height,
        camera_clamped: camera_fov > CAMERA_FOV_OUTPUT_MAX,
        sky_clamped: !(SKY_FOV_MIN..=SKY_FOV_MAX).contains(&sky_fov),
    }
}

/// The framing one screen, or one export, is rendered with: its short edge is
/// the shorter of its two sides.
///
/// On a landscape output that is its height, so the Earth lens is the setting
/// itself. On a portrait one it is the width, which widens the Earth lens
/// until the horizontal extent is what the vertical one is at square: the
/// globe is contained rather than run off the sides.
pub fn screen_framing(settings: Framing, width: u32, height: u32) -> OutputFraming {
    output_framing(settings, width, height, px(width.min(height)))
}

/// The short edge the preview's frame is framed on, in frame pixels.
///
/// The anchor screen's rectangle is as tall as the frame: the frame's height
/// for a landscape screen, whatever the panel's shape, and the height times
/// the screen's aspect ratio for a portrait one, so its rectangle fits the
/// panel's height too. A panel narrower than the screen clips its sides
/// rather than shrinking the picture. With no screen known the frame's height
/// stands in.
pub fn preview_short_edge(frame_height: u32, screen: Option<(u32, u32)>) -> f32 {
    let height = px(frame_height);
    match screen {
        Some((width, screen_height)) if width > 0 && screen_height > 0 => {
            height * (px(width) / px(screen_height)).min(1.0)
        }
        _ => height,
    }
}

/// The framing that makes a canvas render, cropped to the anchor, the render the
/// anchor would have got alone.
///
/// A perspective projection has a constant scale in tan-space at the image
/// plane, so the identity reduces to keeping pixels per unit of tan-space equal
/// between the two renders and moving the principal point. Each lens is scaled
/// on the axis the shader takes it on: the Earth lens by the canvas's height
/// ratio, the sky lens by its width ratio.
///
/// The base is the anchor's *own* framing, so that what the crop reproduces is
/// the image that screen gets in every other mode.
pub fn canvas_framing(settings: Framing, anchor: Rect, canvas: Rect) -> OutputFraming {
    let base = screen_framing(settings, anchor.width, anchor.height);
    if anchor.is_empty() || canvas.is_empty() {
        return base;
    }
    let (canvas_width, canvas_height) = (px(canvas.width), px(canvas.height));
    let (anchor_width, anchor_height) = (px(anchor.width), px(anchor.height));

    let camera_fov = scale_perspective(base.camera_fov, canvas_height / anchor_height);
    let sky_fov = scale_stereographic(base.sky_fov, canvas_width / anchor_width);

    // The anchor's center in canvas pixels, then in canvas NDC, which is where
    // the principal point has to move to.
    let local = anchor.relative_to(&canvas);
    let center_x = px_at(local.x) + anchor_width * 0.5;
    let center_y = px_at(local.y) + anchor_height * 0.5;
    let ndc_x = 2.0 * center_x / canvas_width - 1.0;
    let ndc_y = 1.0 - 2.0 * center_y / canvas_height;

    OutputFraming {
        camera_fov: camera_fov.min(CAMERA_FOV_OUTPUT_MAX),
        sky_fov: sky_fov.clamp(SKY_FOV_MIN, SKY_FOV_MAX),
        // The second term carries the anchor's own pan, which is in the
        // anchor's NDC, into the canvas's.
        offset_x: -ndc_x + base.offset_x * anchor_width / canvas_width,
        offset_y: -ndc_y + base.offset_y * anchor_height / canvas_height,
        camera_clamped: base.camera_clamped || camera_fov > CAMERA_FOV_OUTPUT_MAX,
        sky_clamped: base.sky_clamped || !(SKY_FOV_MIN..=SKY_FOV_MAX).contains(&sky_fov),
    }
}

/// One export, and the monitors it is the image for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderGroup {
    pub width: u32,
    pub height: u32,
    /// Positions in the monitor list this one render covers.
    pub monitors: Vec<usize>,
}

/// Every export a mode needs, and which monitors each one is for.
///
/// "Distinct" is a distinct size, since the scene is otherwise identical across
/// screens: two monitors of the same resolution cost one render and two files,
/// and mirrored monitors, which are two entries with the same rectangle, come
/// out of this as one group for free.
///
/// [`DisplayMode::AcrossScreens`] answers with a single group at the canvas size
/// covering every monitor, because that mode is one export and a set of crops.
pub fn render_groups(monitors: &[Monitor], mode: DisplayMode, anchor: usize) -> Vec<RenderGroup> {
    let usable = |index: &usize| {
        monitors
            .get(*index)
            .is_some_and(|monitor| !monitor.rect().is_empty())
    };
    match mode {
        DisplayMode::OneScreen => (0..monitors.len())
            .filter(|index| *index == anchor)
            .filter(usable)
            .map(|index| RenderGroup {
                width: monitors[index].width,
                height: monitors[index].height,
                monitors: vec![index],
            })
            .collect(),
        DisplayMode::EveryScreen => {
            let mut groups: Vec<RenderGroup> = Vec::new();
            for index in (0..monitors.len()).filter(usable) {
                let (width, height) = (monitors[index].width, monitors[index].height);
                match groups
                    .iter_mut()
                    .find(|group| group.width == width && group.height == height)
                {
                    Some(group) => group.monitors.push(index),
                    None => groups.push(RenderGroup {
                        width,
                        height,
                        monitors: vec![index],
                    }),
                }
            }
            groups
        }
        DisplayMode::AcrossScreens => bounds_of(monitors)
            .into_iter()
            .map(|canvas| RenderGroup {
                width: canvas.width,
                height: canvas.height,
                monitors: (0..monitors.len()).filter(usable).collect(),
            })
            .collect(),
    }
}

/// Cut one rectangle out of an RGBA8 buffer.
///
/// `rect` is in the canvas's own coordinates, which is a monitor's rectangle
/// relative to the canvas origin. `None` rather than a partial copy where the
/// buffer is not the size it claims or the rectangle reaches outside it: a crop
/// that silently returns black is a wallpaper nobody can explain.
pub fn crop(pixels: &[u8], canvas_width: u32, canvas_height: u32, rect: Rect) -> Option<Vec<u8>> {
    let stride = (canvas_width as usize).checked_mul(4)?;
    if pixels.len() != stride.checked_mul(canvas_height as usize)? {
        return None;
    }
    if rect.is_empty()
        || rect.x < 0
        || rect.y < 0
        || rect.right() > i64::from(canvas_width)
        || rect.bottom() > i64::from(canvas_height)
    {
        return None;
    }
    let left = usize::try_from(rect.x).ok()? * 4;
    let top = usize::try_from(rect.y).ok()?;
    let row = (rect.width as usize) * 4;
    let mut out = Vec::with_capacity(row * rect.height as usize);
    for y in 0..rect.height as usize {
        let start = (top + y) * stride + left;
        out.extend_from_slice(&pixels[start..start + row]);
    }
    Some(out)
}

#[cfg(test)]
#[expect(
    clippy::cast_precision_loss,
    reason = "the fabricated screen sizes in these tests are small integers"
)]
mod tests {
    use approx::assert_relative_eq;

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

    /// Two 1920x1080 monitors side by side, the left one primary.
    fn side_by_side() -> Vec<Monitor> {
        vec![
            monitor("DP-1", 0, 0, 1920, 1080, true),
            monitor("DP-2", 1920, 0, 1920, 1080, false),
        ]
    }

    fn settings() -> Framing {
        Framing {
            camera_fov: 20.0,
            sky_fov: 86.0,
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }

    /// The canvas is the bounding box of the monitors, wherever a desk puts
    /// them. Windows gives negative coordinates freely, since the primary is
    /// the origin and everything left of or above it is negative.
    #[test]
    fn the_canvas_is_the_bounding_box_of_the_monitors() {
        for (layout, monitors, expected) in [
            (
                "side by side",
                side_by_side(),
                Rect {
                    x: 0,
                    y: 0,
                    width: 3840,
                    height: 1080,
                },
            ),
            (
                "a taller second monitor",
                vec![
                    monitor("DP-1", 0, 0, 1920, 1080, true),
                    monitor("DP-2", 1920, 0, 2560, 1440, false),
                ],
                Rect {
                    x: 0,
                    y: 0,
                    width: 4480,
                    height: 1440,
                },
            ),
            (
                "a stack",
                vec![
                    monitor("DP-1", 0, 0, 1920, 1080, true),
                    monitor("DP-2", 0, 1080, 1920, 1080, false),
                ],
                Rect {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 2160,
                },
            ),
            (
                "left of and above the primary",
                vec![
                    monitor("DISPLAY1", 0, 0, 1920, 1080, true),
                    monitor("DISPLAY2", -2560, -200, 2560, 1440, false),
                ],
                Rect {
                    x: -2560,
                    y: -200,
                    width: 4480,
                    height: 1440,
                },
            ),
            (
                "one monitor, which bounds exactly itself",
                vec![monitor("DP-1", 0, 0, 2560, 1440, true)],
                Rect {
                    x: 0,
                    y: 0,
                    width: 2560,
                    height: 1440,
                },
            ),
            (
                "two mirrored monitors, which are one screen's worth of canvas",
                vec![
                    monitor("DP-1", 0, 0, 1920, 1080, true),
                    monitor("HDMI-1", 0, 0, 1920, 1080, false),
                ],
                Rect {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
            ),
        ] {
            assert_eq!(bounds_of(&monitors), Some(expected), "{layout}");
        }
    }

    #[test]
    fn a_gap_between_two_monitors_is_inside_the_canvas() {
        // Pixels no monitor covers are rendered and discarded, which is what
        // makes the bounding box the canvas rather than the union.
        let monitors = vec![
            monitor("DP-1", 0, 0, 1920, 1080, true),
            monitor("DP-2", 3000, 0, 1920, 1080, false),
        ];
        let canvas = bounds_of(&monitors).expect("two monitors bound something");
        assert_eq!(canvas.width, 4920);
        let covered: u32 = monitors.iter().map(|m| m.width * m.height).sum();
        assert!(
            canvas.width * canvas.height > covered,
            "the gap is canvas nobody sees"
        );
    }

    #[test]
    fn a_monitor_with_no_pixels_does_not_pull_the_canvas_to_its_origin() {
        let monitors = vec![
            monitor("DP-1", 1000, 1000, 1920, 1080, true),
            monitor("DP-2", 0, 0, 0, 0, false),
        ];
        assert_eq!(
            bounds_of(&monitors),
            Some(Rect {
                x: 1000,
                y: 1000,
                width: 1920,
                height: 1080
            })
        );
        assert_eq!(bounds_of(&[]), None);
        assert_eq!(bounds_of(&monitors[1..]), None);
    }

    #[test]
    fn the_anchor_is_the_stored_monitor_when_the_session_still_has_it() {
        let monitors = side_by_side();
        assert_eq!(
            resolve_anchor(&monitors, Some("DP-2")),
            Some(Anchor {
                index: 1,
                fell_back: false
            })
        );
    }

    #[test]
    fn a_stored_anchor_that_is_gone_falls_back_to_the_primary_and_says_so() {
        let monitors = side_by_side();
        assert_eq!(
            resolve_anchor(&monitors, Some("HDMI-9")),
            Some(Anchor {
                index: 0,
                fell_back: true
            })
        );
        // Nothing stored is not a fallback: it is the ordinary default, and a
        // status line that complained about it would cry wolf on every session.
        assert_eq!(
            resolve_anchor(&monitors, None),
            Some(Anchor {
                index: 0,
                fell_back: false
            })
        );
        assert_eq!(resolve_anchor(&[], Some("DP-1")), None);
    }

    #[test]
    fn nothing_marked_primary_anchors_on_the_first_monitor() {
        let monitors = vec![
            monitor("DP-1", 0, 0, 1920, 1080, false),
            monitor("DP-2", 1920, 0, 1920, 1080, false),
        ];
        assert_eq!(resolve_anchor(&monitors, None).map(|a| a.index), Some(0));
    }

    #[test]
    fn a_landscape_output_keeps_the_earth_lens_it_was_given() {
        for (width, height) in [(1920, 1080), (3440, 1440), (1024, 768), (1080, 1080)] {
            let framing = screen_framing(settings(), width, height);
            assert_relative_eq!(framing.camera_fov, settings().camera_fov, epsilon = 1e-4);
            assert!(!framing.clamped(), "{width}x{height}: {framing:?}");
        }
    }

    #[test]
    fn a_portrait_output_gets_the_horizontal_extent_the_vertical_one_has_at_square() {
        let portrait = screen_framing(settings(), 1080, 1920);
        let square = screen_framing(settings(), 1080, 1080);
        let half = |fov: f32| (fov.to_radians() * 0.5).tan();
        let aspect = 1080.0 / 1920.0;
        assert_relative_eq!(
            half(portrait.camera_fov) * aspect,
            half(square.camera_fov),
            max_relative = 1e-5
        );
        assert!(
            portrait.camera_fov > settings().camera_fov,
            "containing a portrait screen widens the lens"
        );
    }

    #[test]
    fn the_contain_rule_is_monotonic_in_the_lens_it_is_given() {
        let lens = |camera_fov| {
            screen_framing(
                Framing {
                    camera_fov,
                    ..settings()
                },
                1080,
                1920,
            )
            .camera_fov
        };
        assert!(lens(15.0) < lens(40.0));
        // A degenerate output is left alone rather than divided by zero.
        assert_relative_eq!(screen_framing(settings(), 0, 1080).camera_fov, 20.0);
        assert_relative_eq!(screen_framing(settings(), 1080, 0).camera_fov, 20.0);
    }

    /// The preview's short edge: the anchor screen's rectangle as tall as the
    /// frame, whatever shape the panel is.
    #[test]
    fn the_previews_short_edge_puts_the_screen_at_the_frames_height() {
        // A landscape screen: the frame's height, in a wide panel and a narrow
        // one alike.
        assert_relative_eq!(preview_short_edge(600, Some((2560, 1440))), 600.0);
        // A portrait screen: its rectangle is the frame's height tall, so its
        // width, the short edge, is the height times its aspect ratio.
        assert_relative_eq!(
            preview_short_edge(600, Some((1080, 1920))),
            600.0 * 1080.0 / 1920.0
        );
        assert_relative_eq!(preview_short_edge(600, None), 600.0);
        assert_relative_eq!(preview_short_edge(600, Some((0, 1080))), 600.0);
    }

    /// Where the globe's center lands and how large it is, and where one sky
    /// direction lands, measured from the output's center in units of its
    /// short edge.
    fn positions_in_short_edges(
        settings: Framing,
        yaw_deg: f32,
        direction: glam::Vec3,
        width: u32,
        height: u32,
        short_edge: f32,
    ) -> [f32; 5] {
        use crate::scene::camera::{CameraParams, OrbitalCamera};
        use crate::scene::sky_lens::{globe_screen_circle, sky_lens_disc};

        let framing = output_framing(settings, width, height, short_edge);
        assert!(!framing.clamped(), "{width}x{height} on {short_edge}");
        let mut params = SceneParams::default();
        params.camera = CameraParams {
            yaw_deg,
            ..params.camera
        };
        let output = framing.applied_to(&params);
        let camera = OrbitalCamera::from_params(&output.camera);
        let viewport = glam::Vec2::new(width as f32, height as f32);
        let center = viewport * 0.5;
        let globe = globe_screen_circle(
            camera.mvp_matrix(viewport.x / viewport.y),
            camera.distance,
            1.0,
            camera.fov_deg,
            viewport,
        );
        let sky = sky_lens_disc(
            direction,
            0.0,
            output.sky_fov,
            glam::Vec2::new(-output.camera.offset_x, -output.camera.offset_y),
            viewport,
        )
        .expect("a direction in front of the lens has an image");
        let globe_at = (globe.center - center) / short_edge;
        let sky_at = (sky.center - center) / short_edge;
        [
            globe_at.x,
            globe_at.y,
            globe.radius / short_edge,
            sky_at.x,
            sky_at.y,
        ]
    }

    proptest::proptest! {
        /// Success criterion 1: any two outputs, the preview at any panel size
        /// for any screen, a screen or an export of any size, put a sky
        /// direction and the globe at the same place relative to their
        /// centers in units of their short edges.
        #[test]
        fn every_output_is_the_same_picture_in_short_edges(
            screen_height in 600_u32..2400,
            screen_eighths in 4_u32..24,
            panel_height in 200_u32..1500,
            panel_eighths in 2_u32..32,
            export_height in 100_u32..2000,
            export_eighths in 2_u32..32,
            sky_fov in 30.0_f32..120.0,
            camera_fov in 10.0_f32..60.0,
            offset_x in -1.0_f32..1.0,
            offset_y in -1.0_f32..1.0,
            yaw_deg in -20.0_f32..20.0,
            azimuth in 0.0_f32..std::f32::consts::TAU,
            off_axis in 0.0_f32..0.4,
        ) {
            // Widths in eighths of the height: screens from 1:2 to 3:1, panels
            // and exports from 1:4 to 4:1, which keeps every lens in range.
            let screen_width = screen_height * screen_eighths / 8;
            let panel_width = panel_height * panel_eighths / 8;
            let export_width = export_height * export_eighths / 8;
            let settings = Framing { camera_fov, sky_fov, offset_x, offset_y };
            let direction = glam::Vec3::new(
                off_axis.sin() * azimuth.cos(),
                off_axis.sin() * azimuth.sin(),
                -off_axis.cos(),
            );
            let place = |width: u32, height: u32, short_edge: f32| {
                positions_in_short_edges(settings, yaw_deg, direction, width, height, short_edge)
            };
            let screen = place(screen_width, screen_height, screen_width.min(screen_height) as f32);
            let export = place(export_width, export_height, export_width.min(export_height) as f32);
            let preview = place(
                panel_width,
                panel_height,
                preview_short_edge(panel_height, Some((screen_width, screen_height))),
            );
            for (index, ((screen, export), preview)) in
                screen.iter().zip(&export).zip(&preview).enumerate()
            {
                proptest::prop_assert!((screen - export).abs() < 1e-3, "{index}: {screen} {export}");
                proptest::prop_assert!((screen - preview).abs() < 1e-3, "{index}: {screen} {preview}");
            }
        }
    }

    #[test]
    fn a_pan_moves_the_scene_by_the_same_share_of_the_short_edge_everywhere() {
        let direction = glam::Vec3::new(0.2, -0.1, -0.97).normalize();
        let panned = Framing {
            offset_x: 0.6,
            offset_y: -0.3,
            ..settings()
        };
        for (width, height, short_edge) in [
            (1920, 1080, 1080.0),
            (1080, 1920, 1080.0),
            (3440, 1440, 1440.0),
            (400, 900, 900.0),
        ] {
            let still =
                positions_in_short_edges(settings(), 0.0, direction, width, height, short_edge);
            let moved = positions_in_short_edges(panned, 0.0, direction, width, height, short_edge);
            // NDC y runs up and pixel y down, and the pan moves the scene the
            // other way from its sign, as `mvp_matrix` applies it.
            for (still, moved, shift) in [
                (still[0], moved[0], -0.3),
                (still[1], moved[1], -0.15),
                (still[3], moved[3], -0.3),
                (still[4], moved[4], -0.15),
            ] {
                assert_relative_eq!(moved - still, shift, epsilon = 1e-4);
            }
        }
    }

    #[test]
    fn a_lens_the_output_cannot_take_says_it_was_clamped() {
        // The Earth lens: a setting at the slider's top on a strongly portrait
        // output.
        let tall = screen_framing(
            Framing {
                camera_fov: crate::scene::camera::CAMERA_FOV_MAX,
                ..settings()
            },
            100,
            2000,
        );
        assert!(tall.camera_clamped && !tall.sky_clamped, "{tall:?}");
        assert_relative_eq!(tall.camera_fov, CAMERA_FOV_OUTPUT_MAX);

        // The sky lens, at both ends: an output far wider than its short edge,
        // and one far narrower.
        let wide = output_framing(settings(), 20_000, 1000, 1000.0);
        assert!(wide.sky_clamped && !wide.camera_clamped, "{wide:?}");
        assert_relative_eq!(wide.sky_fov, SKY_FOV_MAX);
        let narrow = output_framing(settings(), 2, 1000, 1000.0);
        assert!(narrow.sky_clamped, "{narrow:?}");
        assert_relative_eq!(narrow.sky_fov, SKY_FOV_MIN);

        assert!(!screen_framing(settings(), 1920, 1080).clamped());
    }

    /// The invariant that gives the span mode its meaning.
    ///
    /// Asserted through the pixel scale each lens produces rather than through
    /// the angles, because the angles are only equal in the degenerate case and
    /// the scales are what the identity is actually about.
    #[test]
    fn the_anchors_framing_survives_being_put_on_a_canvas() {
        let monitors = side_by_side();
        let canvas = bounds_of(&monitors).unwrap();
        let anchor = monitors[0].rect();
        let own = screen_framing(settings(), anchor.width, anchor.height);
        // The Earth lens and the principal point. The sky lens has its own
        // case below, because the two are anchored to different axes and a
        // canvas that is wider rather than taller moves only one of them.
        let derived = canvas_framing(settings(), anchor, canvas);

        let camera_scale = |fov: f32, height: u32| height as f32 / (fov.to_radians() * 0.5).tan();
        assert_relative_eq!(
            camera_scale(derived.camera_fov, canvas.height),
            camera_scale(own.camera_fov, anchor.height),
            max_relative = 1e-5
        );
        // These two screens are the same height, so the canvas height cancels
        // and the identity above reduces to the lens being left alone outright.
        assert_relative_eq!(derived.camera_fov, own.camera_fov);

        // The anchor's center lands where its crop's center is.
        assert_relative_eq!(derived.offset_x, 0.5, epsilon = 1e-6);
        assert_relative_eq!(derived.offset_y, 0.0, epsilon = 1e-6);
    }

    /// The pixels the anchor's own render puts in one unit of lens radius.
    fn sky_scale(fov: f32, width: u32) -> f32 {
        width as f32 / (fov.to_radians() * 0.25).tan()
    }

    /// The horizontal sky a 1920x1080 anchor draws at these cases' settings.
    fn anchor_sky() -> f32 {
        screen_framing(settings(), 1920, 1080).sky_fov
    }

    /// How many times the anchor's width a canvas has to be before the sky
    /// saturates, at the framing these cases use.
    fn widths_the_sky_reaches() -> f32 {
        (SKY_FOV_MAX.to_radians() * 0.25).tan() / (anchor_sky().to_radians() * 0.25).tan()
    }

    #[test]
    fn a_span_of_ordinary_screens_keeps_the_sky_scale_too() {
        // Every width a desk holds, two screens to six. The canvas's pixels per
        // unit of lens radius equal the anchor's, which is the half of the span
        // identity the sky lens owns.
        for screens in 2..=6_i32 {
            let monitors: Vec<Monitor> = (0..screens)
                .map(|i| monitor(&format!("DP-{i}"), i * 1920, 0, 1920, 1080, i == 0))
                .collect();
            let canvas = bounds_of(&monitors).unwrap();
            let derived = canvas_framing(settings(), monitors[0].rect(), canvas);
            assert!(!derived.sky_clamped, "{screens} screens: {derived:?}");
            assert_relative_eq!(
                sky_scale(derived.sky_fov, canvas.width),
                sky_scale(anchor_sky(), monitors[0].width),
                max_relative = 1e-5
            );
            assert!(
                derived.sky_fov > 180.0,
                "a canvas {screens} times the anchor's width needs a sky past the \
                 slider's own maximum, and this one derived {}",
                derived.sky_fov
            );
        }
    }

    #[test]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a screen count a ceil already made whole"
    )]
    fn a_canvas_wider_than_the_sky_reaches_says_the_sky_was_clamped() {
        // One screen beyond the reach, which at the default sky is a wall of
        // eight. Nobody has this layout; what the case is for is that the
        // derivation reports the one number it could not honor instead of
        // handing back a crop that no longer matches.
        let screens = widths_the_sky_reaches().ceil() as i32 + 1;
        let monitors: Vec<Monitor> = (0..screens)
            .map(|i| monitor(&format!("DP-{i}"), i * 1920, 0, 1920, 1080, i == 0))
            .collect();
        let canvas = bounds_of(&monitors).unwrap();
        let derived = canvas_framing(settings(), monitors[0].rect(), canvas);
        assert!(derived.sky_clamped, "{screens} screens: {derived:?}");
        assert_relative_eq!(derived.sky_fov, SKY_FOV_MAX);
    }

    #[test]
    fn a_taller_canvas_widens_the_earth_lens() {
        let monitors = vec![
            monitor("DP-1", 0, 0, 1920, 1080, true),
            monitor("DP-2", 1920, 0, 1920, 2160, false),
        ];
        let canvas = bounds_of(&monitors).unwrap();
        let derived = canvas_framing(settings(), monitors[0].rect(), canvas);
        let scale = |fov: f32, height: u32| height as f32 / (fov.to_radians() * 0.5).tan();
        assert_relative_eq!(
            scale(derived.camera_fov, canvas.height),
            scale(settings().camera_fov, 1080),
            max_relative = 1e-5
        );
    }

    #[test]
    fn the_users_own_pan_survives_the_canvas() {
        let monitors = side_by_side();
        let canvas = bounds_of(&monitors).unwrap();
        let panned = Framing {
            offset_x: 0.4,
            offset_y: -0.2,
            ..settings()
        };
        let own = screen_framing(panned, 1920, 1080);
        let derived = canvas_framing(panned, monitors[1].rect(), canvas);
        // The anchor here is the right-hand screen, whose center sits at NDC
        // +0.5, and its own pan is carried in at the ratio of the two widths.
        assert_relative_eq!(derived.offset_x, -0.5 + own.offset_x * 0.5, epsilon = 1e-6);
        assert_relative_eq!(derived.offset_y, own.offset_y, epsilon = 1e-6);
    }

    #[test]
    fn a_span_over_one_monitor_is_that_monitors_own_framing() {
        let monitors = vec![monitor("DP-1", 0, 0, 2560, 1440, true)];
        let canvas = bounds_of(&monitors).unwrap();
        let own = screen_framing(settings(), 2560, 1440);
        let derived = canvas_framing(settings(), monitors[0].rect(), canvas);
        assert_relative_eq!(derived.camera_fov, own.camera_fov);
        assert_relative_eq!(derived.sky_fov, own.sky_fov);
        assert_relative_eq!(derived.offset_x, 0.0, epsilon = 1e-6);
        assert_relative_eq!(derived.offset_y, 0.0, epsilon = 1e-6);
    }

    #[test]
    fn one_screen_renders_only_the_anchor() {
        let groups = render_groups(&side_by_side(), DisplayMode::OneScreen, 1);
        assert_eq!(
            groups,
            vec![RenderGroup {
                width: 1920,
                height: 1080,
                monitors: vec![1]
            }]
        );
    }

    #[test]
    fn different_resolutions_are_one_render_each_at_their_own_size() {
        let monitors = vec![
            monitor("DP-1", 0, 0, 1920, 1080, true),
            monitor("DP-2", 1920, 0, 2560, 1440, false),
            monitor("DP-3", 4480, 0, 1920, 1080, false),
        ];
        let groups = render_groups(&monitors, DisplayMode::EveryScreen, 0);
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert_eq!(groups[0].monitors, vec![0, 2]);
        assert_eq!((groups[0].width, groups[0].height), (1920, 1080));
        assert_eq!(groups[1].monitors, vec![1]);
        assert_eq!((groups[1].width, groups[1].height), (2560, 1440));
    }

    #[test]
    fn a_span_is_one_render_at_the_canvas_size_for_every_monitor() {
        let groups = render_groups(&side_by_side(), DisplayMode::AcrossScreens, 0);
        assert_eq!(
            groups,
            vec![RenderGroup {
                width: 3840,
                height: 1080,
                monitors: vec![0, 1]
            }]
        );
    }

    #[test]
    fn a_screen_with_no_pixels_is_never_rendered_for() {
        let monitors = vec![
            monitor("DP-1", 0, 0, 1920, 1080, true),
            monitor("DP-2", 1920, 0, 0, 1080, false),
        ];
        for mode in DisplayMode::ALL {
            let groups = render_groups(&monitors, mode, 0);
            assert!(
                groups.iter().all(|group| group.monitors == vec![0]),
                "{mode:?} {groups:?}"
            );
        }
        assert!(render_groups(&[], DisplayMode::EveryScreen, 0).is_empty());
    }

    /// A canvas whose pixel values encode their own position, so a crop that is
    /// off by a row or a column is not a plausible image.
    fn canvas(width: u32, height: u32) -> Vec<u8> {
        (0..height)
            .flat_map(|y| {
                (0..width).flat_map(move |x| {
                    [
                        u8::try_from(x % 256).unwrap(),
                        u8::try_from(y % 256).unwrap(),
                        0,
                        255,
                    ]
                })
            })
            .collect()
    }

    #[test]
    fn the_crops_of_a_layout_tile_the_canvas() {
        let pixels = canvas(8, 4);
        let left = crop(
            &pixels,
            8,
            4,
            Rect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
        )
        .unwrap();
        let right = crop(
            &pixels,
            8,
            4,
            Rect {
                x: 4,
                y: 0,
                width: 4,
                height: 4,
            },
        )
        .unwrap();
        assert_eq!(left.len() + right.len(), pixels.len());
        // Their first pixels are the canvas's columns 0 and 4.
        assert_eq!(left[0], 0);
        assert_eq!(right[0], 4);

        // And a crop of the whole canvas is the canvas.
        assert_eq!(
            crop(
                &pixels,
                8,
                4,
                Rect {
                    x: 0,
                    y: 0,
                    width: 8,
                    height: 4
                }
            ),
            Some(pixels)
        );
    }

    #[test]
    fn a_crop_at_the_far_edge_is_not_off_by_a_row() {
        let pixels = canvas(8, 4);
        let corner = crop(
            &pixels,
            8,
            4,
            Rect {
                x: 6,
                y: 2,
                width: 2,
                height: 2,
            },
        )
        .unwrap();
        assert_eq!(&corner[..4], &[6, 2, 0, 255]);
        assert_eq!(&corner[corner.len() - 4..], &[7, 3, 0, 255]);
    }

    #[test]
    fn a_crop_that_reaches_outside_the_canvas_is_refused() {
        let pixels = canvas(4, 4);
        let outside = Rect {
            x: 2,
            y: 0,
            width: 4,
            height: 4,
        };
        assert_eq!(crop(&pixels, 4, 4, outside), None);
        assert_eq!(
            crop(
                &pixels,
                4,
                4,
                Rect {
                    x: -1,
                    y: 0,
                    width: 2,
                    height: 2
                }
            ),
            None
        );
        // A buffer that is not the size it claims is refused rather than read
        // past its end.
        assert_eq!(
            crop(
                &pixels,
                8,
                4,
                Rect {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2
                }
            ),
            None
        );
    }

    #[test]
    fn the_modes_round_trip_through_their_positions_and_their_names() {
        for mode in DisplayMode::ALL {
            assert_eq!(
                DisplayMode::from_index(i32::try_from(mode.index()).unwrap()),
                mode
            );
            let text = toml::to_string(&Wrapper { mode }).unwrap();
            assert!(text.contains(mode.name()), "{text}");
            assert_eq!(toml::from_str::<Wrapper>(&text).unwrap().mode, mode);
            assert_eq!(DisplayMode::from_name(mode.name()), Some(mode));
            assert!(!mode.label().is_empty());
        }
        // A name nothing answers to costs the default rather than the rest of
        // the config file.
        assert_eq!(DisplayMode::from_name("every-other-screen"), None);
        assert_eq!(
            toml::from_str::<Wrapper>("mode = \"every-other-screen\"")
                .unwrap()
                .mode,
            DisplayMode::default()
        );
        // Out of range is the default rather than a panic: the combo's index
        // arrives from the UI, and -1 is what an empty combo answers with.
        assert_eq!(DisplayMode::from_index(-1), DisplayMode::default());
        assert_eq!(DisplayMode::from_index(99), DisplayMode::default());
    }

    #[derive(Serialize, Deserialize)]
    struct Wrapper {
        mode: DisplayMode,
    }
}
