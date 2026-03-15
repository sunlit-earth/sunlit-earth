/// Pure-Rust mirror of the fragment shader's day/night blend + diffuse
/// shading logic, used to test properties like monotonicity that are
/// hard to verify visually on-GPU.

/// GLSL/WGSL `smoothstep` equivalent.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Parameters for the day/night blend computation.
struct BlendParams {
    day: [f32; 3],
    night: [f32; 3],
    terminator_width: f32,
    diffuse_enabled: bool,
    diffuse_floor: f32,
    diffuse_ramp: f32,
}

/// Compute the blended fragment color for a given `n_dot_l` (dot product
/// of surface normal and sun direction). Mirrors `fs_main` in `sphere.wgsl`.
fn blend_color(p: &BlendParams, n_dot_l: f32) -> [f32; 3] {
    let w = p.terminator_width;
    let blend = smoothstep(-w, w, n_dot_l);

    let mut shaded_day = p.day;
    if p.diffuse_enabled {
        let shading = p.diffuse_floor
            + (1.0 - p.diffuse_floor) * smoothstep(0.0, p.diffuse_ramp, n_dot_l);
        for (i, sd) in shaded_day.iter_mut().enumerate() {
            *sd = (p.day[i] * shading).max(p.night[i].min(p.day[i]));
        }
    }

    [
        p.night[0] + (shaded_day[0] - p.night[0]) * blend,
        p.night[1] + (shaded_day[1] - p.night[1]) * blend,
        p.night[2] + (shaded_day[2] - p.night[2]) * blend,
    ]
}

/// Same computation but WITHOUT the clamp fix, to demonstrate the bug.
fn blend_color_buggy(p: &BlendParams, n_dot_l: f32) -> [f32; 3] {
    let w = p.terminator_width;
    let blend = smoothstep(-w, w, n_dot_l);

    let mut shaded_day = p.day;
    if p.diffuse_enabled {
        let shading = p.diffuse_floor
            + (1.0 - p.diffuse_floor) * smoothstep(0.0, p.diffuse_ramp, n_dot_l);
        for (i, sd) in shaded_day.iter_mut().enumerate() {
            *sd = p.day[i] * shading;
        }
    }

    [
        p.night[0] + (shaded_day[0] - p.night[0]) * blend,
        p.night[1] + (shaded_day[1] - p.night[1]) * blend,
        p.night[2] + (shaded_day[2] - p.night[2]) * blend,
    ]
}

fn luminance(c: &[f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dark ocean texel: the problematic case where day is not much
    /// brighter than night, so diffuse_floor can push it below night.
    fn ocean_params() -> BlendParams {
        BlendParams {
            day: [0.05, 0.08, 0.30],
            night: [0.01, 0.01, 0.04],
            terminator_width: 0.15,
            diffuse_enabled: true,
            diffuse_floor: 0.1,
            diffuse_ramp: 0.6,
        }
    }

    /// Brighter land texel (less likely to exhibit the bug, but still
    /// should be monotonic).
    fn land_params() -> BlendParams {
        BlendParams {
            day: [0.30, 0.25, 0.15],
            night: [0.03, 0.02, 0.01],
            terminator_width: 0.15,
            diffuse_enabled: true,
            diffuse_floor: 0.1,
            diffuse_ramp: 0.6,
        }
    }

    /// New York City: bright city lights in the night texture, moderate
    /// urban surface during the day. Represents the regression case where
    /// max(shaded_day, night_color) would leak city lights onto the day side.
    fn nyc_params() -> BlendParams {
        BlendParams {
            day: [0.28, 0.24, 0.20],
            night: [0.55, 0.50, 0.30],
            terminator_width: 0.15,
            diffuse_enabled: true,
            diffuse_floor: 0.1,
            diffuse_ramp: 0.6,
        }
    }

    /// Sweep n_dot_l from -1 to +1 and collect luminance values.
    fn luminance_sweep(
        f: fn(&BlendParams, f32) -> [f32; 3],
        p: &BlendParams,
        steps: usize,
    ) -> Vec<f32> {
        (0..=steps)
            .map(|i| {
                let n_dot_l = -1.0 + 2.0 * (i as f32 / steps as f32);
                luminance(&f(p, n_dot_l))
            })
            .collect()
    }

    // ---- Tests that demonstrate the bug in the old code ----

    #[test]
    fn buggy_ocean_dips_below_night() {
        let p = ocean_params();
        let night_lum = luminance(&p.night);
        let sweep = luminance_sweep(blend_color_buggy, &p, 1000);

        let min_lum = sweep.iter().copied().reduce(f32::min).unwrap();
        // The bug: minimum luminance in the sweep is darker than the
        // night side, creating a visible dark band.
        assert!(
            min_lum < night_lum,
            "Expected buggy code to dip below night luminance \
             ({min_lum:.6} should be < {night_lum:.6})"
        );
    }

    #[test]
    fn buggy_ocean_not_monotonic() {
        let p = ocean_params();
        let sweep = luminance_sweep(blend_color_buggy, &p, 1000);

        let is_monotonic = sweep.windows(2).all(|w| w[1] >= w[0] - 1e-6);
        assert!(
            !is_monotonic,
            "Expected buggy code to violate monotonicity for ocean texels"
        );
    }

    // ---- Tests that verify the fix ----

    #[test]
    fn fixed_ocean_never_below_night() {
        let p = ocean_params();
        let night_lum = luminance(&p.night);
        let sweep = luminance_sweep(blend_color, &p, 1000);

        let min_lum = sweep.iter().copied().reduce(f32::min).unwrap();
        assert!(
            min_lum >= night_lum - 1e-6,
            "Fixed code should never dip below night luminance \
             ({min_lum:.6} should be >= {night_lum:.6})"
        );
    }

    #[test]
    fn fixed_ocean_monotonic() {
        let p = ocean_params();
        let sweep = luminance_sweep(blend_color, &p, 1000);

        for (i, w) in sweep.windows(2).enumerate() {
            assert!(
                w[1] >= w[0] - 1e-6,
                "Monotonicity violated at step {i}: {:.6} -> {:.6}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn fixed_land_monotonic() {
        let p = land_params();
        let sweep = luminance_sweep(blend_color, &p, 1000);

        for (i, w) in sweep.windows(2).enumerate() {
            assert!(
                w[1] >= w[0] - 1e-6,
                "Monotonicity violated at step {i}: {:.6} -> {:.6}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn fixed_per_channel_never_below_night() {
        let p = ocean_params();
        for i in 0..=1000 {
            let n_dot_l = -1.0 + 2.0 * (i as f32 / 1000.0);
            let color = blend_color(&p, n_dot_l);
            for ch in 0..3 {
                assert!(
                    color[ch] >= p.night[ch] - 1e-6,
                    "Channel {ch} dipped below night at n_dot_l={n_dot_l:.3}: \
                     {:.6} < {:.6}",
                    color[ch],
                    p.night[ch]
                );
            }
        }
    }

    #[test]
    fn fixed_matches_unshaded_when_diffuse_disabled() {
        let mut p = ocean_params();
        p.diffuse_enabled = false;

        for i in 0..=100 {
            let n_dot_l = -1.0 + 2.0 * (i as f32 / 100.0);
            let color = blend_color(&p, n_dot_l);
            let blend = smoothstep(-p.terminator_width, p.terminator_width, n_dot_l);
            for ch in 0..3 {
                let expected = p.night[ch] + (p.day[ch] - p.night[ch]) * blend;
                assert!(
                    (color[ch] - expected).abs() < 1e-6,
                    "Diffuse-disabled mismatch at n_dot_l={n_dot_l:.2}, ch={ch}"
                );
            }
        }
    }

    #[test]
    fn fixed_fully_lit_matches_day_color() {
        let p = ocean_params();
        let color = blend_color(&p, 1.0);
        for ch in 0..3 {
            assert!(
                (color[ch] - p.day[ch]).abs() < 1e-6,
                "Fully-lit should match day color, ch={ch}: {:.6} vs {:.6}",
                color[ch],
                p.day[ch]
            );
        }
    }

    #[test]
    fn fixed_fully_dark_matches_night_color() {
        let p = ocean_params();
        let color = blend_color(&p, -1.0);
        for ch in 0..3 {
            assert!(
                (color[ch] - p.night[ch]).abs() < 1e-6,
                "Fully-dark should match night color, ch={ch}: {:.6} vs {:.6}",
                color[ch],
                p.night[ch]
            );
        }
    }

    // ---- City light regression tests ----

    /// At full daylight (n_dot_l = 1.0), New York should show day_color,
    /// NOT the brighter night city lights.
    #[test]
    fn nyc_fully_lit_matches_day_color() {
        let p = nyc_params();
        let color = blend_color(&p, 1.0);
        for ch in 0..3 {
            assert!(
                (color[ch] - p.day[ch]).abs() < 1e-6,
                "NYC fully-lit ch={ch}: got {:.4}, expected day {:.4}",
                color[ch],
                p.day[ch]
            );
        }
    }

    /// On the day side of NYC (n_dot_l well above terminator), the
    /// luminance must stay close to the shaded day texture — NOT be
    /// inflated toward the bright night texture.
    #[test]
    fn nyc_day_side_not_dominated_by_city_lights() {
        let p = nyc_params();
        let night_lum = luminance(&p.night);
        let day_lum = luminance(&p.day);

        // Sample several points well into the day side
        for &n_dot_l in &[0.3, 0.5, 0.7, 1.0] {
            let color = blend_color(&p, n_dot_l);
            let lum = luminance(&color);
            assert!(
                lum <= day_lum + 1e-6,
                "NYC at n_dot_l={n_dot_l}: luminance {lum:.4} exceeds \
                 day luminance {day_lum:.4} — city lights leaking through"
            );
            assert!(
                lum < night_lum,
                "NYC at n_dot_l={n_dot_l}: luminance {lum:.4} should be \
                 well below night luminance {night_lum:.4}"
            );
        }
    }

    /// No channel of the result should ever dip below `min(night, day)`
    /// for any n_dot_l. This is the core invariant of the fix.
    #[test]
    fn never_below_min_of_night_and_day() {
        let params = [ocean_params(), land_params(), nyc_params()];
        for p in &params {
            for i in 0..=1000 {
                let n_dot_l = -1.0 + 2.0 * (i as f32 / 1000.0);
                let color = blend_color(p, n_dot_l);
                for ch in 0..3 {
                    let floor = p.night[ch].min(p.day[ch]);
                    assert!(
                        color[ch] >= floor - 1e-6,
                        "Channel {ch} below min(night, day) at n_dot_l={n_dot_l:.3}: \
                         {:.6} < {:.6}",
                        color[ch],
                        floor
                    );
                }
            }
        }
    }

    /// Stress test: sweep many different day/night color combinations
    /// (including city-light cases where night > day) and verify that
    /// no channel ever dips below min(night, day).
    #[test]
    fn never_below_min_across_color_range() {
        let days: &[[f32; 3]] = &[
            [0.02, 0.03, 0.20], // very dark ocean
            [0.05, 0.08, 0.30], // typical ocean
            [0.10, 0.10, 0.10], // grey
            [0.28, 0.24, 0.20], // urban (NYC-like)
            [0.30, 0.25, 0.15], // land
            [0.80, 0.80, 0.80], // bright surface
        ];
        let nights: &[[f32; 3]] = &[
            [0.00, 0.00, 0.00], // pitch black
            [0.01, 0.01, 0.04], // dim ocean
            [0.05, 0.04, 0.02], // faint glow
            [0.30, 0.28, 0.15], // moderate city
            [0.55, 0.50, 0.30], // bright city (NYC)
            [0.80, 0.75, 0.60], // very bright city
        ];

        for day in days {
            for night in nights {
                let p = BlendParams {
                    day: *day,
                    night: *night,
                    terminator_width: 0.15,
                    diffuse_enabled: true,
                    diffuse_floor: 0.1,
                    diffuse_ramp: 0.6,
                };
                for i in 0..=500 {
                    let n_dot_l = -1.0 + 2.0 * (i as f32 / 500.0);
                    let color = blend_color(&p, n_dot_l);
                    for ch in 0..3 {
                        let floor = p.night[ch].min(p.day[ch]);
                        assert!(
                            color[ch] >= floor - 1e-6,
                            "day={day:?} night={night:?} n_dot_l={n_dot_l:.3} \
                             ch={ch}: {:.6} < floor {:.6}",
                            color[ch],
                            floor
                        );
                    }
                }
            }
        }
    }
}
