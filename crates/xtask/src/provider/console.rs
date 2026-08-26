//! The guest console's resolution, as far as both providers share it.
//!
//! One environment variable names it and one parser reads it, because the two
//! guests are looked at through different windows and the question a person asks
//! about them is the same one. What each provider does with the answer differs
//! and stays with the provider: a `Hyper-V` guest is seen through a basic
//! `vmconnect` session, whose window is the framebuffer, so an unset variable
//! there means "the largest mode that fits this host's screen"; a QEMU guest is
//! seen through a VNC viewer, which scales, so an unset variable there means the
//! documented default.

/// Overrides the guest console's resolution, as `WxH`. Read for both providers.
pub const RESOLUTION_ENV: &str = "SUNLIT_EARTH_VM_RESOLUTION";

/// What a resolution may be, at both ends.
///
/// The floor is below every mode either provider offers and the ceiling is
/// generous: the point is to catch a transposed or mistyped value, not to have
/// an opinion about a host with a very large screen.
const RESOLUTION_BOUNDS: (u32, u32) = (640, 7680);

/// Parse a `WxH` resolution, rejecting anything that is not one.
pub fn parse_resolution(value: &str) -> Option<(u32, u32)> {
    let (width, height) = value.trim().split_once(['x', 'X'])?;
    let width: u32 = width.trim().parse().ok()?;
    let height: u32 = height.trim().parse().ok()?;
    let (min, max) = RESOLUTION_BOUNDS;
    let plausible = (min..=max).contains(&width) && (min..=max).contains(&height);
    plausible.then_some((width, height))
}

/// The size [`RESOLUTION_ENV`] asks for, if it asks for one that parses.
///
/// `complain` belongs to the one call per boot that chooses a console size and
/// says so. The paths that re-derive the same answer later pass `false`: a value
/// nobody can parse has already been reported once, and reporting it again from
/// `vm view` would suggest something had changed since.
pub fn requested_resolution(complain: bool) -> Option<(u32, u32)> {
    let raw = crate::util::env_var(RESOLUTION_ENV)?;
    let parsed = parse_resolution(&raw);
    if parsed.is_none() && complain {
        println!(
            "warning: {RESOLUTION_ENV} is {raw:?}, which is not a size like \
             1920x1080; choosing one for this guest instead"
        );
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resolution_is_two_numbers_and_anything_else_is_not_one() {
        assert_eq!(parse_resolution("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_resolution(" 2560 X 1440 "), Some((2560, 1440)));
        for bad in [
            "",
            "1920",
            "1920x",
            "x1080",
            "1920*1080",
            "1920x1080x60",
            "huge",
            "-1920x1080",
            // Out of bounds at both ends: a typo rather than a screen, and
            // either would otherwise be handed to a hypervisor that accepts it.
            "320x240",
            "99999x1080",
        ] {
            assert_eq!(parse_resolution(bad), None, "{bad}");
        }
    }

    #[test]
    fn both_providers_read_the_same_variable() {
        // The name is a documented knob, and it is documented once for the two
        // guests, so it is one constant rather than one per provider.
        assert_eq!(RESOLUTION_ENV, "SUNLIT_EARTH_VM_RESOLUTION");
    }
}
