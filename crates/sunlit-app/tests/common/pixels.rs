//! What a rendered frame is asked to look like.
//!
//! Colour assertions rather than pixel equality: an adapter is allowed to
//! disagree about a channel, and none of these cases is about that.

/// Extract the (R, G, B) channels of a pixel at the given coordinates.
pub(crate) fn rgb_at(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = img.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

/// Assert that a corner of the render is empty space rather than globe.
///
/// A patch and not a pixel, because the sky is drawn there: a single sample
/// asks whether one point happens to be free of stars, and a star default, the
/// catalog or the fixture's date could land one on that sample and report
/// "expected black" about a globe that is exactly where it should be. What the
/// case means is that the corner is mostly empty, which a patch can say and a
/// pixel cannot. Stars are small and sparse; a globe filling the corner is
/// neither.
const CORNER_PATCH: u32 = 16;

const CORNER_BLACK_FRACTION: f64 = 0.5;

pub(crate) fn assert_space_corner(img: &image::RgbaImage, right: bool, bottom: bool, label: &str) {
    let x0 = if right { img.width() - CORNER_PATCH } else { 0 };
    let y0 = if bottom {
        img.height() - CORNER_PATCH
    } else {
        0
    };
    let mut black = 0_u32;
    let mut brightest = ([0_u8; 3], 0_u32, (0_u32, 0_u32));
    for y in y0..y0 + CORNER_PATCH {
        for x in x0..x0 + CORNER_PATCH {
            let rgb = rgb_at(img, x, y);
            let sum = u32::from(rgb[0]) + u32::from(rgb[1]) + u32::from(rgb[2]);
            if sum < 40 {
                black += 1;
            }
            if sum > brightest.1 {
                brightest = (rgb, sum, (x, y));
            }
        }
    }
    let fraction = f64::from(black) / f64::from(CORNER_PATCH * CORNER_PATCH);
    assert!(
        fraction >= CORNER_BLACK_FRACTION,
        "{label}: expected mostly empty space, only {:.0}% of the \
         {CORNER_PATCH}x{CORNER_PATCH} patch at ({x0}, {y0}) is black \
         (limit {:.0}%). Brightest pixel {:?} sum {} at {:?}.",
        fraction * 100.0,
        CORNER_BLACK_FRACTION * 100.0,
        brightest.0,
        brightest.1,
        brightest.2
    );
}

/// Assert that a pixel is dark ocean on the night side (nearly black).
pub(crate) fn assert_night_ocean(rgb: [u8; 3], label: &str) {
    let sum = u32::from(rgb[0]) + u32::from(rgb[1]) + u32::from(rgb[2]);
    assert!(
        sum < 40,
        "{label}: expected dark ocean, got {rgb:?} (sum={sum})"
    );
}

/// Assert that a pixel is night-side land (dim, bluish from nightglow).
pub(crate) fn assert_night_land(rgb: [u8; 3], label: &str) {
    let [r, g, b] = rgb;
    let sum = u32::from(r) + u32::from(g) + u32::from(b);
    assert!(
        sum > 40 && sum < 200 && b >= r && b >= g,
        "{label}: expected dim bluish night land, got {rgb:?} (sum={sum})"
    );
}

/// Assert that a pixel is greenish (vegetation).
/// Green must clearly dominate both red and blue.
pub(crate) fn assert_greenish(rgb: [u8; 3], label: &str) {
    let [r, g, b] = rgb;
    assert!(
        g > r + 10 && g > b && g > 40,
        "{label}: expected greenish (G dominant), got {rgb:?}"
    );
}

/// Assert that a pixel is yellowish/sandy (desert).
/// Red and green both strong, blue clearly lower.
pub(crate) fn assert_yellowish(rgb: [u8; 3], label: &str) {
    let [r, g, b] = rgb;
    let warm = u32::from(r) + u32::from(g);
    assert!(
        warm > 3 * u32::from(b) && r > 80 && g > 80,
        "{label}: expected yellowish/sandy (R+G >> B), got {rgb:?}"
    );
}

/// Assert that a pixel is blue (ocean).
/// Blue must exceed the sum of red and green.
pub(crate) fn assert_blue(rgb: [u8; 3], label: &str) {
    let [r, g, b] = rgb;
    assert!(
        u32::from(b) > u32::from(r) + u32::from(g) && b > 30,
        "{label}: expected blue (B > R+G), got {rgb:?}"
    );
}

/// Assert that a pixel is bright/white (ice/snow).
/// High overall brightness with all channels close together.
pub(crate) fn assert_ice(rgb: [u8; 3], label: &str) {
    let [r, g, b] = rgb;
    let sum = u32::from(r) + u32::from(g) + u32::from(b);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let spread = u32::from(max) - u32::from(min);
    assert!(
        sum > 500 && spread < 30,
        "{label}: expected bright white ice (sum>500, spread<30), got {rgb:?} (sum={sum}, spread={spread})"
    );
}
