//! The star catalog.

use crate::groups::{FRAME, plain};
use crate::harness::{gpu, test_params};

/// How many disagreeing pixels the failure below lists before it stops.
const LISTED: usize = 8;

#[test]
fn zero_star_intensity_leaves_catalog_pixels_at_the_clear_color() {
    const CLEAR: [u8; 4] = [1, 1, 3, 255];
    /// A difference a person would see, which is the golden suite's own
    /// outlier threshold.
    const VISIBLE: u8 = 24;

    // With the atmosphere off, the only thing outside the globe is stars, so a
    // background pixel the two frames disagree about is one a star painted.
    // That is what lets this assert what its name says rather than the much
    // weaker "one pixel changed": at intensity zero each of those pixels is
    // still exactly the clear color, not a dimmed star.
    //
    // A pixel the globe covers is a different question and gets a different
    // rule. The globe is opaque and drawn after the stars, so nothing of a
    // star can survive there, and yet the paravirtual Metal device of a
    // `macos-latest` runner does not reproduce its limb pixels when the pass
    // changes: seven of them move by up to two of 255 when the star draw's ten
    // thousand sprites are added, some channels up and some down, and one pair
    // of neighbours exchanges values, which is a sampling difference at
    // extreme minification rather than anything additive. So a covered pixel
    // is held to the threshold the golden suite already uses for a difference
    // worth seeing, and the exact assertion stays where the property lives.
    let gpu = gpu();
    let harness = plain(&gpu);
    let mut params = test_params();
    params.atmo_enabled = false;
    params.star_intensity = 0.0;
    let stars_off = harness.picture(&params, FRAME);
    params.star_intensity = 1.5;
    let stars_on = harness.picture(&params, FRAME);

    let mut lit_background = 0_usize;
    let mut leaked = Vec::new();
    for (index, (off, on)) in stars_off
        .chunks_exact(4)
        .zip(stars_on.chunks_exact(4))
        .enumerate()
    {
        if off == on {
            continue;
        }
        if off == CLEAR {
            lit_background += 1;
        } else if off.iter().zip(on).any(|(a, b)| a.abs_diff(*b) > VISIBLE) {
            leaked.push(index);
        }
    }
    assert!(
        leaked.is_empty(),
        "{} covered pixel(s) changed visibly when the stars came on, \
         so a star reached the globe:\n{}",
        leaked.len(),
        report(&leaked, &stars_off, &stars_on)
    );
    assert!(
        lit_background > 0,
        "enabling stars should alter a clear background pixel"
    );
}

/// One pixel of an RGBA frame.
fn pixel(frame: &[u8], index: usize) -> [u8; 4] {
    let mut rgba = [0_u8; 4];
    rgba.copy_from_slice(&frame[index * 4..index * 4 + 4]);
    rgba
}

/// Where the listed pixels are and what the two renders made of them.
///
/// Enough for one CI run on an adapter nobody here has to answer what the case
/// cannot: whether the frame is the size the case assumed, where on the globe
/// the pixel sits, and which way each channel went.
fn report(indices: &[usize], off: &[u8], on: &[u8]) -> String {
    use std::fmt::Write;

    let width = FRAME.0 as usize;
    let mut lines = String::new();
    let _ = writeln!(
        lines,
        "  frame {}x{}, {} pixels",
        FRAME.0,
        FRAME.1,
        off.len() / 4
    );
    for &index in indices.iter().take(LISTED) {
        let _ = writeln!(
            lines,
            "  ({}, {}) off {:?} on {:?}",
            index % width,
            index / width,
            pixel(off, index),
            pixel(on, index)
        );
    }
    if indices.len() > LISTED {
        let _ = writeln!(lines, "  and {} more", indices.len() - LISTED);
    }
    lines
}

#[test]
fn larger_star_size_expands_crisp_cores_when_glow_is_disabled() {
    const CLEAR: [u8; 4] = [1, 1, 3, 255];

    let gpu = gpu();
    let harness = plain(&gpu);
    let mut params = test_params();
    params.star_intensity = 2.0;
    params.star_size = 0.5;
    params.star_glow_strength = 0.0;
    params.star_mag_limit = 4.0;
    let small_stars = harness.picture(&params, FRAME);
    params.star_size = 3.0;
    let large_stars = harness.picture(&params, FRAME);

    let expanded_core = small_stars
        .chunks_exact(4)
        .zip(large_stars.chunks_exact(4))
        .any(|(small, large)| small == CLEAR && large != CLEAR);
    assert!(
        expanded_core,
        "increasing star size should expand a core without relying on glow"
    );
}

#[test]
fn wider_sky_fov_reveals_more_catalog_directions() {
    const CLEAR: [u8; 4] = [1, 1, 3, 255];

    let gpu = gpu();
    let harness = plain(&gpu);
    let mut params = test_params();
    params.sky_fov = 60.0;
    let narrow_sky = harness.picture(&params, FRAME);
    params.sky_fov = 140.0;
    let wide_sky = harness.picture(&params, FRAME);

    let newly_visible_pixels = narrow_sky
        .chunks_exact(4)
        .zip(wide_sky.chunks_exact(4))
        .filter(|(narrow, wide)| *narrow == CLEAR && *wide != CLEAR)
        .count();
    assert!(
        newly_visible_pixels > 50,
        "wider sky FOV revealed only {newly_visible_pixels} background pixels"
    );
}
