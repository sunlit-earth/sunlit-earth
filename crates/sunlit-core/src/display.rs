//! What the displays are, on the platforms that answer with a program rather
//! than with an API.
//!
//! Two questions need the same answer and used to have two placeholders for it:
//! how large to render a wallpaper, and whether a saved window position is still
//! somewhere a person can reach. On Windows both come from Win32 monitor
//! enumeration. On Linux the query is `xrandr --query`, whose output is parsed
//! here.
//!
//! xrandr rather than a windowing dependency. `sunlit-core` owns no window, and
//! Slint's public `Window` API reports the window's own size and nothing about
//! the display behind it, so there is no API here to ask. Under a Wayland
//! session the answer comes through `XWayland`, which is usable and has one known
//! distortion: display scaling can make the reported size differ from the
//! compositor's own idea of it. A native per-desktop D-Bus query is the fix and
//! is a roadmap item rather than phase work.
//!
//! The parser is separate from the process, so it is tested on every platform
//! against real xrandr output rather than only where xrandr exists.

pub mod layout;
pub mod watch;

/// One output, as xrandr describes a connected one with a mode assigned.
///
/// The geometry is post-rotation: xrandr reports a rotated output as the shape
/// it presents, so a portrait monitor is taller than it is wide here and nothing
/// downstream has to know about rotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub name: String,
    /// Whether xrandr marked this one primary. At most one output is.
    pub primary: bool,
    pub width: u32,
    pub height: u32,
    /// Position in the X screen's coordinate space, which is what makes a
    /// multi-monitor bounds check possible at all.
    pub x: i32,
    pub y: i32,
}

impl Output {
    /// Whether a rectangle overlaps this output at all.
    ///
    /// Half-open on the far edges, so a window whose left edge is exactly the
    /// output's right edge is on the next one and not on this.
    #[cfg(any(not(windows), test))]
    pub(crate) fn overlaps(&self, x: i32, y: i32, width: i32, height: i32) -> bool {
        let right = self
            .x
            .saturating_add(i32::try_from(self.width).unwrap_or(i32::MAX));
        let bottom = self
            .y
            .saturating_add(i32::try_from(self.height).unwrap_or(i32::MAX));
        x < right
            && x.saturating_add(width) > self.x
            && y < bottom
            && y.saturating_add(height) > self.y
    }
}

/// Read the connected outputs out of `xrandr --query` output.
///
/// One line per output, unindented, with `connected` as a whole word: the
/// disconnected ones say `disconnected`, which contains the same letters and is
/// why this compares tokens rather than searching the line. A connected output
/// with no mode assigned carries no geometry token and is skipped, because an
/// output nothing is displayed on is not somewhere to put a window.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_outputs(text: &str) -> Vec<Output> {
    let mut outputs = Vec::new();
    for line in text.lines() {
        // The mode list under each output is indented; the output's own line is
        // not, and neither is the `Screen 0:` header, which has no `connected`.
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let Some(name) = tokens.next() else { continue };
        let mut connected = false;
        let mut primary = false;
        let mut geometry = None;
        for token in tokens {
            match token {
                "connected" => connected = true,
                "primary" => primary = true,
                other => {
                    if geometry.is_none() {
                        geometry = parse_geometry(other);
                    }
                }
            }
        }
        if let (true, Some((width, height, x, y))) = (connected, geometry) {
            outputs.push(Output {
                name: name.to_owned(),
                primary,
                width,
                height,
                x,
                y,
            });
        }
    }
    outputs
}

/// `WIDTHxHEIGHT+X+Y`, with either sign on the offsets.
///
/// Rejecting anything else matters more than it looks: the rest of an output's
/// line is free text ("normal left inverted right x axis y axis", "0mm x 0mm"),
/// and the first token that happens to parse is taken as the geometry.
#[cfg(any(target_os = "linux", test))]
fn parse_geometry(token: &str) -> Option<(u32, u32, i32, i32)> {
    let (size, offsets) = token.split_once('+')?;
    let (width, height) = size.split_once('x')?;
    let width: u32 = width.parse().ok()?;
    let height: u32 = height.parse().ok()?;
    // The remainder is `X+Y` or `X-Y`, and X itself may be negative, so the
    // split is on the separator between them rather than on the first sign.
    let (x, y) = split_offsets(offsets)?;
    (width > 0 && height > 0).then_some((width, height, x, y))
}

/// The two offsets out of what follows the first `+`.
#[cfg(any(target_os = "linux", test))]
fn split_offsets(text: &str) -> Option<(i32, i32)> {
    let at = text
        .char_indices()
        .skip(1)
        .find(|&(_, c)| c == '+' || c == '-')?
        .0;
    let x: i32 = text[..at].parse().ok()?;
    // Kept with its sign: `+0-1080` is an output above the origin.
    let y: i32 = text[at..].trim_start_matches('+').parse().ok()?;
    Some((x, y))
}

/// The output a wallpaper is sized for: the primary one, or the first there is.
///
/// Falling back to the first rather than refusing, because a single-output
/// session is not always marked primary and that is the commonest case here.
pub fn primary_of(outputs: &[Output]) -> Option<&Output> {
    outputs
        .iter()
        .find(|output| output.primary)
        .or_else(|| outputs.first())
}

/// One monitor a wallpaper can be put on.
///
/// The same shape on every platform, which is the point: everything above this
/// is a pure function over a list of these, and the only per-OS work left is
/// filling the list in and handing the finished images back out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    /// Stable per connector, and the key a setter needs: the xrandr output name
    /// on Linux, the display device path on Windows.
    pub id: String,
    /// What the UI shows. Never used to address anything.
    pub label: String,
    /// Virtual-desktop coordinates in physical pixels, post-rotation.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

impl Monitor {
    /// This monitor's rectangle in the virtual desktop.
    pub fn rect(&self) -> layout::Rect {
        layout::Rect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}

impl From<Output> for Monitor {
    /// An xrandr output is already a monitor; the connector name is both the
    /// key a setter needs and the name a person recognizes.
    fn from(output: Output) -> Self {
        Self {
            label: output.name.clone(),
            id: output.name,
            x: output.x,
            y: output.y,
            width: output.width,
            height: output.height,
            primary: output.primary,
        }
    }
}

/// The monitor a wallpaper is anchored to by default: the primary, or the first.
///
/// The same fallback [`primary_of`] makes, and for the same reason: a session
/// that marks nothing primary is common and is not a session to refuse.
#[cfg(any(windows, test))]
pub(crate) fn primary_monitor_of(monitors: &[Monitor]) -> Option<&Monitor> {
    monitors
        .iter()
        .find(|monitor| monitor.primary)
        .or_else(|| monitors.first())
}

/// Every monitor this session has, in the order the platform lists them.
///
/// Three-valued the way [`outputs`] is, and load-bearing in the same way:
/// `None` where there is no way to ask, an empty list where the query answered
/// with nothing usable, and a list otherwise.
#[cfg(target_os = "linux")]
pub fn monitors() -> Option<Vec<Monitor>> {
    Some(outputs()?.into_iter().map(Monitor::from).collect())
}

#[cfg(windows)]
pub fn monitors() -> Option<Vec<Monitor>> {
    match crate::wallpaper::enumerate_monitors() {
        Ok(monitors) => Some(monitors),
        Err(e) => {
            tracing::warn!(error = %e, "cannot enumerate the monitors");
            None
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn monitors() -> Option<Vec<Monitor>> {
    None
}

/// Ask the display server what it has.
///
/// `None` where there is no way to ask, which is every platform but Linux, and
/// an empty list where xrandr answered with no usable output, which is what a
/// headless run looks like. The two are deliberately different: a caller that
/// cannot ask keeps whatever it did before, and a caller that asked and got
/// nothing knows there is no display.
#[cfg(target_os = "linux")]
pub fn outputs() -> Option<Vec<Output>> {
    // `DISPLAY` unset is the ordinary headless case: the `render` subcommand,
    // CI, the golden suite. Running xrandr there costs a process and a message
    // on stderr to learn what the missing variable already said.
    crate::env_override("DISPLAY")?;
    let out = std::process::Command::new("xrandr")
        .arg("--query")
        .output()
        .ok()?;
    if !out.status.success() {
        tracing::debug!(
            status = ?out.status.code(),
            "xrandr --query failed; falling back to the documented default size"
        );
        return None;
    }
    Some(parse_outputs(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(not(target_os = "linux"))]
pub fn outputs() -> Option<Vec<Output>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `xrandr --query` answers with in the Linux test guest: one
    /// virtio output, primary, with its mode list indented under it, and the
    /// second virtio head that guest has with nothing plugged into it.
    const GUEST: &str = "\
Screen 0: minimum 320 x 200, current 1920 x 1080, maximum 16384 x 16384
Virtual-1 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis) 0mm x 0mm
   1920x1080     59.96*+
   1280x800      59.81
Virtual-2 disconnected (normal left inverted right x axis y axis)
";

    /// Two monitors, the second above and to the left of the origin, and a
    /// connected output with no mode assigned.
    ///
    /// The primary is deliberately not the first output listed, and the
    /// disconnected `DP-3` deliberately still carries geometry: an output that
    /// was configured and then unplugged with its CRTC still assigned prints
    /// exactly that, and it is the one line the connected check alone keeps out.
    const DESK: &str = "\
Screen 0: minimum 8 x 8, current 5120 x 1440, maximum 32767 x 32767
DP-1 connected 3440x1440+1680+0 (normal left inverted right x axis y axis) 800mm x 335mm
   3440x1440    143.92*+
HDMI-1 connected primary 1680x1050+0-200 right (normal left inverted right x axis y axis) 474mm x 296mm
   1680x1050     59.95*+
DP-2 connected (normal left inverted right x axis y axis)
DP-3 disconnected 1920x1080+5120+0 (normal left inverted right x axis y axis) 0mm x 0mm
eDP-1 disconnected (normal left inverted right x axis y axis)
";

    /// The guest's one output, read whole.
    ///
    /// "0mm x 0mm" and "normal left inverted right x axis y axis" sit on the
    /// same line and both contain an `x`, so a parser that searched for one
    /// would find more geometry than there is.
    #[test]
    fn the_guests_single_output_is_read_with_its_mode() {
        let outputs = parse_outputs(GUEST);
        assert_eq!(
            outputs,
            vec![Output {
                name: "Virtual-1".to_owned(),
                primary: true,
                width: 1920,
                height: 1080,
                x: 0,
                y: 0,
            }]
        );
        assert_eq!(
            primary_of(&outputs).map(|o| (o.width, o.height)),
            Some((1920, 1080))
        );
    }

    /// The desk's five outputs come to exactly two, with their signs intact.
    ///
    /// Each of the three the parser drops is dropped for its own reason.
    /// "disconnected" contains "connected", which is why whole tokens are
    /// compared rather than the line searched. `DP-3` is disconnected and still
    /// holding a mode, which is what xrandr prints for an output that was
    /// configured and then unplugged, so the geometry filter alone would let it
    /// through. `DP-2` is connected with nothing displayed on it, so a window
    /// placed there would be on a black screen. `eDP-1` is neither.
    #[test]
    fn the_desk_parses_to_the_outputs_a_window_could_go_on() {
        assert_eq!(
            parse_outputs(DESK),
            vec![
                Output {
                    name: "DP-1".to_owned(),
                    primary: false,
                    width: 3440,
                    height: 1440,
                    x: 1680,
                    y: 0,
                },
                Output {
                    name: "HDMI-1".to_owned(),
                    primary: true,
                    width: 1680,
                    height: 1050,
                    x: 0,
                    y: -200,
                },
            ]
        );
    }

    #[test]
    fn the_primary_is_the_one_xrandr_marked_and_not_the_first_listed() {
        // The two are the same on most sessions, which is why this fixture makes
        // them differ: xrandr lists outputs in the X server's own order, and the
        // monitor a person is looking at is whichever one is marked.
        let outputs = parse_outputs(DESK);
        assert_eq!(outputs[0].name, "DP-1", "{outputs:?}");
        assert_eq!(
            primary_of(&outputs).map(|o| o.name.as_str()),
            Some("HDMI-1")
        );
        // The resolution too, because that is what taking the first costs: a
        // wallpaper rendered for the wrong monitor, which the desktop's own zoom
        // fill then crops.
        assert_eq!(
            primary_of(&outputs).map(|o| (o.width, o.height)),
            Some((1680, 1050))
        );
        // With nothing marked the first is the answer, which is the commonest
        // case here: XFCE marks no output at all, and neither does a
        // single-output session.
        let unmarked: Vec<Output> = outputs
            .iter()
            .map(|o| Output {
                primary: false,
                ..o.clone()
            })
            .collect();
        assert_eq!(primary_of(&unmarked).map(|o| o.name.as_str()), Some("DP-1"));
    }

    #[test]
    fn nothing_at_all_is_no_outputs_rather_than_a_guess() {
        assert!(parse_outputs("").is_empty());
        assert!(parse_outputs("xrandr: Can't open display\n").is_empty());
        assert!(primary_of(&[]).is_none());
    }

    #[test]
    fn an_output_becomes_a_monitor_with_its_connector_as_both_names() {
        // The connector name is the key a setter addresses and the name a
        // person recognizes, and on Linux those are the same string.
        let monitors: Vec<Monitor> = parse_outputs(DESK).into_iter().map(Monitor::from).collect();
        assert_eq!(monitors.len(), 2, "{monitors:?}");
        assert_eq!(monitors[0].id, "DP-1");
        assert_eq!(monitors[0].label, "DP-1");
        assert_eq!((monitors[0].width, monitors[0].height), (3440, 1440));
        assert_eq!((monitors[0].x, monitors[0].y), (1680, 0));
        assert!(!monitors[0].primary);
        assert_eq!((monitors[1].x, monitors[1].y), (0, -200));
        assert!(monitors[1].primary);
        assert_eq!(
            primary_monitor_of(&monitors).map(|m| m.id.as_str()),
            Some("HDMI-1"),
            "the marked one, not the first listed"
        );
        assert_eq!(
            monitors[0].rect(),
            layout::Rect {
                x: 1680,
                y: 0,
                width: 3440,
                height: 1440
            }
        );
    }

    /// Everything above this reads one list, so a platform that cannot ask has
    /// to be distinguishable from a session with nothing on it.
    #[test]
    fn a_platform_with_no_query_answers_that_it_cannot_ask() {
        #[cfg(not(any(windows, target_os = "linux")))]
        assert!(monitors().is_none());
        // Where there is a query, it either answered or said it could not, and
        // both are answers this run must not confuse for a monitor list.
        #[cfg(any(windows, target_os = "linux"))]
        if let Some(list) = monitors() {
            assert!(
                list.iter().all(|m| !m.id.is_empty()),
                "a monitor nothing can address is not one to plan around: {list:?}"
            );
        }
    }

    #[test]
    fn overlap_is_half_open_on_the_far_edges() {
        let output = Output {
            name: "DP-1".to_owned(),
            primary: true,
            width: 1920,
            height: 1080,
            x: 0,
            y: 0,
        };
        // A title bar inside it.
        assert!(output.overlaps(100, 100, 800, 30));
        // Its top left corner exactly.
        assert!(output.overlaps(0, 0, 1, 1));
        // One pixel past the right and bottom edges, which is the next monitor.
        assert!(!output.overlaps(1920, 0, 10, 10));
        assert!(!output.overlaps(0, 1080, 10, 10));
        // Straddling the left edge from outside still overlaps.
        assert!(output.overlaps(-5, 10, 10, 10));
        // Entirely off to the left does not.
        assert!(!output.overlaps(-100, 10, 50, 10));
    }

    #[test]
    fn an_output_far_from_the_origin_does_not_overflow_the_check() {
        let output = Output {
            name: "DP-1".to_owned(),
            primary: true,
            width: 1920,
            height: 1080,
            x: i32::MAX - 10,
            y: i32::MAX - 10,
        };
        assert!(!output.overlaps(0, 0, 100, 100));
        assert!(output.overlaps(i32::MAX - 5, i32::MAX - 5, i32::MAX, i32::MAX));
    }
}
