//! Fixtures shared by more than one test binary. Each includes the whole
//! module, so parts of it are unused in each.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Width of the moon fixture. Small enough to decode instantly, wide enough
/// that its landmarks survive the mip chain the uploader builds.
pub const MOON_FIXTURE_WIDTH: u32 = 256;

/// The base surface value, bright enough that a threshold well below it counts
/// every lit pixel of the disk and none of an unlit one.
const MOON_FIXTURE_BASE: u8 = 190;

/// Landmarks, as selenographic longitude and latitude in degrees, an angular
/// radius, and a value. Deliberately asymmetric in all three of the ways the
/// orientation can be wrong: east against west, north against south, and a
/// mark on the prime meridian that a quarter shift would move off center.
const MOON_FIXTURE_LANDMARKS: [(f32, f32, f32, u8); 4] = [
    (0.0, 0.0, 14.0, 255),
    (60.0, 5.0, 10.0, 70),
    (0.0, 55.0, 12.0, 255),
    (-55.0, -35.0, 16.0, 90),
];

/// Write an equirectangular moon map into `dir` and return its path.
///
/// Generated rather than committed, so that the bytes behind a golden reference
/// are the same on every machine that regenerates it and there is no binary in
/// the tree whose provenance is a past run of a tool. Its layout is the one
/// `assets::texture_loader::orient` expects of any source map: longitude zero at
/// the center, east to the right, north up.
pub fn write_moon_fixture(dir: &Path) -> PathBuf {
    let width = MOON_FIXTURE_WIDTH;
    let height = width / 2;
    let mut img = image::RgbaImage::from_pixel(
        width,
        height,
        image::Rgba([MOON_FIXTURE_BASE, MOON_FIXTURE_BASE, MOON_FIXTURE_BASE, 255]),
    );
    #[allow(clippy::cast_precision_loss)]
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let longitude = (x as f32 + 0.5) / width as f32 * 360.0 - 180.0;
        let latitude = 90.0 - (y as f32 + 0.5) / height as f32 * 180.0;
        for (mark_longitude, mark_latitude, radius, value) in MOON_FIXTURE_LANDMARKS {
            // Great-circle distance, so a landmark stays round near the poles
            // instead of smearing across them.
            let (lat0, lat1) = (latitude.to_radians(), mark_latitude.to_radians());
            let delta = (longitude - mark_longitude).to_radians();
            let cosine = lat0.sin() * lat1.sin() + lat0.cos() * lat1.cos() * delta.cos();
            if cosine.clamp(-1.0, 1.0).acos().to_degrees() <= radius {
                *pixel = image::Rgba([value, value, value, 255]);
            }
        }
    }
    let path = dir.join("moon-fixture.png");
    std::fs::create_dir_all(dir).expect("create the fixture directory");
    img.save(&path).expect("write the moon fixture");
    path
}

/// Width of the panorama fixtures. Wide enough that a disc at a named sky
/// position comes out round, narrow enough to decode in milliseconds.
pub const PANORAMA_FIXTURE_WIDTH: u32 = 512;

/// The two ends of the ramp fixture's declination gradient, north to south.
pub const PANORAMA_RAMP_ENDS: (u8, u8) = (16, 236);

/// Where a sky position sits in a panorama source, before the loader's own
/// orientation pass.
///
/// The layout `assets::texture_loader::orient` expects of a sky map, which is
/// the one the asset in `textures/` has and is not the one the Earth's and the
/// Moon's have: right ascension zero at the center, right ascension increasing
/// to the *left*, north up. A map of the sphere seen from inside runs the other
/// way round from a map of one seen from outside, and `textures/PROVENANCE.md`
/// records the measurement that this is the asset's layout.
fn panorama_texel(right_ascension: f32, declination: f32, width: u32, height: u32) -> (f32, f32) {
    #[allow(clippy::cast_precision_loss)]
    let (w, h) = (width as f32, height as f32);
    let u = (0.5 - right_ascension / 360.0).rem_euclid(1.0);
    let v = (90.0 - declination) / 180.0;
    (u * w, v * h)
}

/// Write a panorama whose value depends on declination alone, and return its
/// path.
///
/// Two properties, both of which a test needs. It does not depend on right
/// ascension at all, so it is perfectly smooth across the branch cut of the
/// shader's `atan2` and an artifact there cannot be mistaken for content. And
/// its coarsest mip is one texel holding the ramp's own mean, which is far from
/// the ramp's value at most declinations, so a sample that lands on that mip is
/// a visible band rather than a subtle one.
pub fn write_panorama_ramp_fixture(dir: &Path) -> PathBuf {
    let width = PANORAMA_FIXTURE_WIDTH;
    let height = width / 2;
    let mut img = image::RgbaImage::new(width, height);
    let (north, south) = PANORAMA_RAMP_ENDS;
    for (_, y, pixel) in img.enumerate_pixels_mut() {
        #[allow(clippy::cast_precision_loss)]
        let t = (y as f32 + 0.5) / height as f32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let value = (f32::from(north) + (f32::from(south) - f32::from(north)) * t).round() as u8;
        *pixel = image::Rgba([value, value, value, 255]);
    }
    let path = dir.join("panorama-ramp-fixture.png");
    std::fs::create_dir_all(dir).expect("create the fixture directory");
    img.save(&path).expect("write the panorama ramp fixture");
    path
}

/// Write a panorama that is black except for one white disc centered on
/// `right_ascension` and `declination`, both in degrees.
///
/// What this pins is where a direction lands, so the disc is the only thing in
/// it: the brightest pixel of a frame drawn from this panorama is the image of
/// one sky position and nothing else can be mistaken for it.
pub fn write_panorama_landmark_fixture(
    dir: &Path,
    file_name: &str,
    right_ascension: f32,
    declination: f32,
    radius_degrees: f32,
) -> PathBuf {
    let width = PANORAMA_FIXTURE_WIDTH;
    let height = width / 2;
    let mut img = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        #[allow(clippy::cast_precision_loss)]
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let (cx, cy) = panorama_texel(right_ascension, declination, width, height);
        // Wrapped in u, since a landmark near the map's own edge is otherwise
        // measured across the whole width.
        #[allow(clippy::cast_precision_loss)]
        let dx = {
            let raw = (px - cx).abs();
            raw.min(width as f32 - raw)
        };
        // Degrees per texel, which differ between the two axes only because the
        // map is twice as wide as it is tall.
        #[allow(clippy::cast_precision_loss)]
        let angular = glam::Vec2::new(
            dx * 360.0 / width as f32 * declination.to_radians().cos(),
            (py - cy) * 180.0 / height as f32,
        );
        if angular.length() <= radius_degrees {
            *pixel = image::Rgba([255, 255, 255, 255]);
        }
    }
    let path = dir.join(file_name);
    std::fs::create_dir_all(dir).expect("create the fixture directory");
    img.save(&path)
        .expect("write the panorama landmark fixture");
    path
}
