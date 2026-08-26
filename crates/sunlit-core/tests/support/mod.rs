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
