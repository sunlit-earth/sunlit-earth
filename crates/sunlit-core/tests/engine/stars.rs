use crate::groups::{FRAME, plain};
use crate::harness::{gpu, test_params};

#[test]
fn zero_star_intensity_leaves_catalog_pixels_at_the_clear_color() {
    const CLEAR: [u8; 4] = [1, 1, 3, 255];

    // With the atmosphere off, the only thing outside the globe is stars, so
    // every pixel the two frames disagree about is one a star painted. That is
    // what lets this assert what its name says rather than the much weaker "one
    // pixel changed": at intensity zero each of those pixels is still exactly
    // the clear color, not a dimmed star.
    let gpu = gpu();
    let harness = plain(&gpu);
    let mut params = test_params();
    params.atmo_enabled = false;
    params.star_intensity = 0.0;
    let stars_off = harness.picture(&params, FRAME);
    params.star_intensity = 1.5;
    let stars_on = harness.picture(&params, FRAME);

    let mut star_pixels = 0_usize;
    for (index, (off, on)) in stars_off
        .chunks_exact(4)
        .zip(stars_on.chunks_exact(4))
        .enumerate()
    {
        if off != on {
            star_pixels += 1;
            assert_eq!(
                off, CLEAR,
                "pixel {index} carries {off:?} with stars disabled, not the clear color"
            );
        }
    }
    assert!(
        star_pixels > 0,
        "enabling stars should alter a clear background pixel"
    );
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
