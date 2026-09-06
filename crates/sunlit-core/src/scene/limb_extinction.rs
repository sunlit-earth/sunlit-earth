//! What the atmosphere takes out of light that grazes it, per channel.
//!
//! One exponential density model serves the whole band: an air mass that grows
//! toward the surface, a per-channel transmission along it, the hue that comes
//! out the far side, and how far the Sun's disk is still drawn where the path
//! carries almost nothing. `limb_transmission_km` and `limb_disk_amplitude` in
//! `sphere.wgsl` mirror the same two curves per fragment, and
//! `tests/render_pipeline.rs` is what keeps the two spellings agreeing.

use glam::Vec3;

/// Earth's radius in kilometers, which is what turns a shell radius in Earth
/// radii into the height of the band it stands for.
pub(crate) const EARTH_RADIUS_KM: f32 = 6371.0;

/// Scale height of the density the light path runs through, in kilometers.
///
/// One number serves the extinction and the refraction alike: `70 exp(-h / 7)`
/// reproduces Mallama's cumulative air masses to about a fifth over the whole
/// band, and Kipping's ray tracing fits 6.911 km to the lensing.
const ATMOSPHERE_SCALE_HEIGHT_KM: f32 = 7.0;

/// Air masses along a ray that grazes the surface, from Mallama's table.
const HORIZON_AIR_MASS: f32 = 70.0;

/// Transmission per air mass for red, green and blue.
///
/// Mallama states 90 percent for red and 73 percent for blue; green is fitted
/// to his own altitude and transmission pairs, and Kipping's lowtran7 figures
/// for a sea-level path give the same chromatic ratio from another direction.
const CHANNEL_TRANSMISSION: [f32; 3] = [0.90, 0.836, 0.73];

/// Cumulative air mass along a light path whose lowest point is `height_km`.
///
/// Below the surface the path is the surface-grazing one: nothing carries more
/// atmosphere than the whole of it, and the exponential would otherwise run
/// away where the disk sits behind the painted limb.
#[must_use]
pub(crate) fn limb_air_mass(height_km: f32, reddening: f32) -> f32 {
    HORIZON_AIR_MASS * (-height_km.max(0.0) / ATMOSPHERE_SCALE_HEIGHT_KM).exp() * reddening
}

/// What that path leaves of the light, per channel.
///
/// Mirrored by `limb_transmission_km` in `sphere.wgsl`, which is what colors
/// the Sun's disk per fragment and the Rayleigh shell's forward lobe; this
/// spelling is what the CPU integrates over the visible disk. `reddening` zero
/// is the white Sun that knows nothing about the band.
#[must_use]
pub fn limb_transmission(height_km: f32, reddening: f32) -> Vec3 {
    let air_mass = limb_air_mass(height_km, reddening);
    Vec3::new(
        CHANNEL_TRANSMISSION[0].powf(air_mass),
        CHANNEL_TRANSMISSION[1].powf(air_mass),
        CHANNEL_TRANSMISSION[2].powf(air_mass),
    )
}

/// The same light as a hue: divided by its largest channel, which red always
/// is. What the disk and the glare are tinted with, since both are clipped
/// bright long before the path stops carrying anything.
#[must_use]
#[cfg(test)]
pub(crate) fn limb_hue(height_km: f32, reddening: f32) -> Vec3 {
    let transmitted = limb_transmission(height_km, reddening);
    transmitted / transmitted.max_element().max(1e-30)
}

/// How far the disk is drawn where the path carries almost nothing.
///
/// A tenth of the Sun is still a blinding source, so the disk clips at full
/// brightness wherever the green channel carries more than 2.5 percent, about
/// nine kilometers, and fades out below that instead of being cut where the
/// painted globe begins. Mirrored by `limb_disk_amplitude` in `sphere.wgsl`.
#[must_use]
pub fn limb_disk_amplitude(green_transmission: f32) -> f32 {
    (green_transmission * DISK_FADE_GAIN).clamp(0.0, 1.0).sqrt()
}

/// Where the disk stops being clipped white, as a gain on the green channel.
const DISK_FADE_GAIN: f32 = 40.0;

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::*;

    /// Mallama's Table 3.2: the lowest altitude of the ray, the cumulative air
    /// mass along it, and what fraction of green light comes out the far side.
    const MALLAMA_TABLE: [(f32, f32, f32); 11] = [
        (32.0, 0.6, 0.90),
        (27.0, 1.3, 0.79),
        (22.0, 2.7, 0.62),
        (20.0, 3.8, 0.51),
        (18.0, 5.0, 0.41),
        (15.0, 8.5, 0.22),
        (13.0, 13.0, 0.10),
        (8.0, 24.0, 0.014),
        (5.0, 37.0, 0.0013),
        (2.6, 50.0, 0.0001),
        (0.8, 62.0, 0.00001),
    ];

    #[test]
    fn the_exponential_reproduces_the_measured_air_masses() {
        for (height, air_mass, _) in MALLAMA_TABLE {
            let modeled = limb_air_mass(height, 1.0);
            let ratio = modeled / air_mass;
            assert!(
                (0.79..=1.21).contains(&ratio),
                "at {height} km the table has {air_mass} air masses and the model {modeled}"
            );
        }
    }

    /// Transmission is exponential in air mass, so the fifth the fit is worth
    /// at the top of the band is a factor of one and a half at the bottom of
    /// it. Twenty percent holds where the air mass is small, and the 13 km row
    /// is where the fit gives up; `docs/rendering.md` has the measured
    /// disagreement there.
    #[test]
    fn the_transmitted_green_follows_the_table_where_the_air_mass_is_small() {
        for (height, _, transmitted) in MALLAMA_TABLE.iter().take(6) {
            let modeled = limb_transmission(*height, 1.0).y;
            let ratio = modeled / transmitted;
            assert!(
                (0.8..=1.2).contains(&ratio),
                "at {height} km the table transmits {transmitted} of the green and the model {modeled}"
            );
        }
        let deep = limb_transmission(13.0, 1.0).y;
        assert!(
            (deep / 0.10 - 1.412).abs() < 0.05,
            "the 13 km row is the one the exponential fit misses; got {deep}"
        );
    }

    #[test]
    fn the_band_reddens_downward_and_whitens_upward() {
        let mut previous = limb_hue(0.0, 1.0);
        assert!(
            previous.y < 0.02 && previous.z < 0.001,
            "a ray that grazes the surface is red, got {previous}"
        );
        for step in 1..=48 {
            #[allow(clippy::cast_precision_loss)]
            let hue = limb_hue(step as f32 * 2.0, 1.0);
            assert!(
                hue.y >= previous.y && hue.z >= previous.z,
                "the band has to warm downward and cool upward, but {hue} sits above {previous}"
            );
            previous = hue;
        }
        let high = limb_hue(50.0, 1.0);
        assert!(
            (high - Vec3::ONE).abs().max_element() < 0.02,
            "above the weather the path takes nothing out, got {high}"
        );
    }

    #[test]
    fn a_reddening_of_zero_is_the_sun_that_knows_nothing_about_the_band() {
        for height in [-40.0, 0.0, 9.0, 96.0] {
            assert_eq!(limb_transmission(height, 0.0), Vec3::ONE);
            assert_relative_eq!(limb_disk_amplitude(limb_transmission(height, 0.0).y), 1.0);
        }
    }

    /// A tenth of the Sun is still a blinding source, so the disk clips white
    /// down to about nine kilometers and only then starts to go.
    #[test]
    fn the_disk_clips_white_until_the_path_carries_almost_nothing() {
        assert_relative_eq!(limb_disk_amplitude(limb_transmission(12.0, 1.0).y), 1.0);
        assert_relative_eq!(limb_disk_amplitude(limb_transmission(9.0, 1.0).y), 1.0);
        assert!(limb_disk_amplitude(limb_transmission(7.0, 1.0).y) < 0.9);
        assert!(limb_disk_amplitude(limb_transmission(2.0, 1.0).y) < 0.1);
    }
}
