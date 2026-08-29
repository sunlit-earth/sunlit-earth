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

/// The base level and swing of the banded fixture, and how many bands it puts
/// across the sky.
///
/// Eight cycles over 180 degrees of declination, which is 32 texel rows of the
/// fixture. Mip level 5 of it is eight rows for those eight cycles, so
/// everything above level 4 has the bands averaged out of it and holds the base
/// level alone: that is what makes a sample that lands on a coarse mip a
/// visible band rather than a subtle one, and it is the property a linear ramp
/// does not have, since box-averaging and bilinear interpolation reproduce a
/// linear ramp exactly at every level.
pub const PANORAMA_BANDS: (f32, f32, f32) = (128.0, 100.0, 8.0);

/// Where a sky position sits in a panorama source, before the loader's own
/// orientation pass.
///
/// The layout `assets::texture_loader::orient` expects of a sky map, which is
/// the one the asset in `textures/` has and is not the one the Earth's and the
/// Moon's have: right ascension zero at the center, right ascension increasing
/// to the *left*, north up. A map of the sphere seen from inside runs the other
/// way round from a map of one seen from outside, and `textures/PROVENANCE.md`
/// records the measurement that this is the asset's layout.
pub fn panorama_texel(
    right_ascension: f32,
    declination: f32,
    width: u32,
    height: u32,
) -> (f32, f32) {
    #[allow(clippy::cast_precision_loss)]
    let (w, h) = (width as f32, height as f32);
    let u = (0.5 - right_ascension / 360.0).rem_euclid(1.0);
    let v = (90.0 - declination) / 180.0;
    (u * w, v * h)
}

/// Write a panorama whose value depends on declination alone, and return its
/// path.
///
/// Three properties, and each is what a test needs. It does not depend on right
/// ascension at all, so it is perfectly smooth across the branch cut of the
/// shader's `atan2` and an artifact there cannot be mistaken for content. Its
/// bands are slow enough on screen that neighboring pixels differ by a code
/// value or two, so a one-pixel or two-pixel anomaly stands out from them. And
/// every mip level above the fourth has the bands averaged out of it, so a
/// sample that lands on one reads the base level instead of the sky.
pub fn write_panorama_bands_fixture(dir: &Path) -> PathBuf {
    let width = PANORAMA_FIXTURE_WIDTH;
    let height = width / 2;
    let mut img = image::RgbaImage::new(width, height);
    let (base, swing, cycles) = PANORAMA_BANDS;
    for (_, y, pixel) in img.enumerate_pixels_mut() {
        #[allow(clippy::cast_precision_loss)]
        let t = (f32::from(u16::try_from(y).expect("a small row")) + 0.5) / height as f32;
        let level = base + swing * (t * cycles * std::f32::consts::TAU).sin();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let value = level.clamp(0.0, 255.0).round() as u8;
        *pixel = image::Rgba([value, value, value, 255]);
    }
    let path = dir.join("panorama-bands-fixture.png");
    std::fs::create_dir_all(dir).expect("create the fixture directory");
    img.save(&path).expect("write the panorama bands fixture");
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

/// Width of the day and night surface fixtures.
///
/// Exactly the 1024 texels `fs_cloud` measures its city-light blur against, so
/// the mip level it picks on these is zero because the width says so rather
/// than because the clamp under it caught a narrower map.
pub const SURFACE_FIXTURE_WIDTH: u32 = 1024;

/// The day map's one colour. Flat on purpose: every case that loads these is
/// about the cloud layer over the surface, and a lit side with structure of its
/// own would compete with it in a reference.
const DAY_FIXTURE_COLOR: [u8; 3] = [170, 175, 180];

/// The night map's unlit base: about 0.13 of display white, and blue, which is
/// what `BlackMarble_2016.jxl` reads over unlit land. A night map is not a black
/// image with lights on it, and a fixture that was one would make the ordering
/// between a night cloud and the ground under it true for free.
const NIGHT_FIXTURE_BASE: [u8; 3] = [34, 32, 60];

/// Where the one city on the night map sits, as longitude and latitude in
/// degrees. Inside the cloud fixture's equatorial band, so a deck covers it.
pub const NIGHT_FIXTURE_CITY: (f32, f32) = (150.0, 0.0);
/// Angular radius of the city's saturated core and of the cluster around it.
const NIGHT_FIXTURE_CORE_DEGREES: f32 = 9.0;
const NIGHT_FIXTURE_CLUSTER_DEGREES: f32 = 18.0;
const NIGHT_FIXTURE_CORE: [u8; 3] = [255, 250, 235];
const NIGHT_FIXTURE_CLUSTER: [u8; 3] = [150, 140, 120];

/// One day and night map pair, in the order the slots take them.
pub struct SurfaceFixtures {
    pub day: PathBuf,
    pub night: PathBuf,
}

impl SurfaceFixtures {
    /// The two paths as `texture_paths` wants them.
    pub fn paths(&self) -> Vec<Option<PathBuf>> {
        vec![Some(self.day.clone()), Some(self.night.clone())]
    }
}

/// Write a day map and a night map into `dir`, and return their paths.
///
/// The night map is the interesting one: an unlit base bright enough to be
/// measured against, and one city with a saturated core and a dimmer cluster
/// around it, which is the structure a uniform ambient term cannot represent
/// and the city-light coupling is about.
pub fn write_surface_fixtures(dir: &Path) -> SurfaceFixtures {
    let width = SURFACE_FIXTURE_WIDTH;
    let height = width / 2;
    let day = image::RgbaImage::from_pixel(width, height, rgba(DAY_FIXTURE_COLOR));
    let mut night = image::RgbaImage::from_pixel(width, height, rgba(NIGHT_FIXTURE_BASE));
    let (city_longitude, city_latitude) = NIGHT_FIXTURE_CITY;
    #[allow(clippy::cast_precision_loss)]
    for (x, y, pixel) in night.enumerate_pixels_mut() {
        let longitude = (x as f32 + 0.5) / width as f32 * 360.0 - 180.0;
        let latitude = 90.0 - (y as f32 + 0.5) / height as f32 * 180.0;
        let distance = great_circle_degrees(longitude, latitude, city_longitude, city_latitude);
        if distance <= NIGHT_FIXTURE_CORE_DEGREES {
            *pixel = rgba(NIGHT_FIXTURE_CORE);
        } else if distance <= NIGHT_FIXTURE_CLUSTER_DEGREES {
            *pixel = rgba(NIGHT_FIXTURE_CLUSTER);
        }
    }
    std::fs::create_dir_all(dir).expect("create the fixture directory");
    let paths = SurfaceFixtures {
        day: dir.join("day-fixture.png"),
        night: dir.join("night-city-fixture.png"),
    };
    day.save(&paths.day).expect("write the day fixture");
    night.save(&paths.night).expect("write the night fixture");
    paths
}

fn rgba(color: [u8; 3]) -> image::Rgba<u8> {
    image::Rgba([color[0], color[1], color[2], 255])
}

/// Great-circle distance in degrees, so a mark stays round near the poles
/// instead of smearing across them.
fn great_circle_degrees(
    longitude: f32,
    latitude: f32,
    mark_longitude: f32,
    mark_latitude: f32,
) -> f32 {
    let (lat0, lat1) = (latitude.to_radians(), mark_latitude.to_radians());
    let delta = (longitude - mark_longitude).to_radians();
    let cosine = lat0.sin() * lat1.sin() + lat0.cos() * lat1.cos() * delta.cos();
    cosine.clamp(-1.0, 1.0).acos().to_degrees()
}

/// Width of the cloud fixture. The overlay is sampled as one channel and its
/// bands are twenty degrees tall, so it needs no more resolution than this.
pub const CLOUD_FIXTURE_WIDTH: u32 = 512;

/// Height of one band of the cloud fixture, in degrees of latitude.
const CLOUD_FIXTURE_BAND_DEGREES: f32 = 20.0;

/// The latitude where the equatorial band ends, which is where a camera has the
/// deck and the bare surface in one frame.
pub const CLOUD_FIXTURE_BAND_EDGE: f32 = CLOUD_FIXTURE_BAND_DEGREES / 2.0;

/// A cloud map of hard latitude bands, dense over the equator.
///
/// Bands rather than blobs, and three things follow from the shape. The layer
/// reaches both hemispheres at every longitude, so a framing across the
/// terminator shows one band ramping from its night value to white along its
/// own length. The clear gaps keep the surface visible in the same frame, which
/// is where the ordering between a night cloud and the ground under it can be
/// seen. And a band is centered on the equator, where the night map's city is.
///
/// Latitude alone, so the fixture says nothing about the horizontal convention
/// the cloud decode applies: what a band is depends on neither the flip nor the
/// quarter shift.
fn cloud_bands_image() -> image::RgbaImage {
    let width = CLOUD_FIXTURE_WIDTH;
    let height = width / 2;
    let mut img = image::RgbaImage::new(width, height);
    #[allow(clippy::cast_precision_loss)]
    for (_, y, pixel) in img.enumerate_pixels_mut() {
        let latitude = 90.0 - (y as f32 + 0.5) / height as f32 * 180.0;
        #[allow(clippy::cast_possible_truncation)]
        let band = ((latitude + 90.0) / CLOUD_FIXTURE_BAND_DEGREES).floor() as i32;
        let value = if band % 2 == 0 { 255 } else { 0 };
        *pixel = rgba([value, value, value]);
    }
    img
}

/// The raw cloud value of the even bands' partial sibling.
///
/// The banded map is 255 or nothing, so every cloud pixel in it has a density of
/// one and any nonzero night opacity covers the ground completely. That is the
/// one thing the real source never is: its median texel is 0.79 and only half a
/// percent of it reaches 0.97. So the opacity cases use a map at one mid value
/// instead, where the alpha a mapping produces is a number rather than a
/// saturated one.
pub const CLOUD_FIXTURE_PARTIAL: u8 = 178;

/// A cloud map at one value everywhere.
fn cloud_uniform_image(value: u8) -> image::RgbaImage {
    let width = CLOUD_FIXTURE_WIDTH;
    let mut img = image::RgbaImage::new(width, width / 2);
    for (_, _, pixel) in img.enumerate_pixels_mut() {
        *pixel = rgba([value, value, value]);
    }
    img
}

/// A cloud source that serves one generated map and then reports it unchanged.
///
/// The cloud slot is fed by the fetcher rather than by `texture_paths`, so this
/// is the only way a test renders a cloud pixel at all. PNG rather than JPEG,
/// so the bands reach the shader with the edges they were drawn with.
pub struct FixtureClouds {
    bytes: Vec<u8>,
    etag: String,
}

impl FixtureClouds {
    /// The banded map, encoded once.
    pub fn bands() -> Self {
        Self::from_image(cloud_bands_image(), "cloud-bands-fixture")
    }

    /// One value everywhere, for a case that needs a density between the two
    /// the bands offer.
    pub fn uniform(value: u8) -> Self {
        Self::from_image(cloud_uniform_image(value), "cloud-uniform-fixture")
    }

    fn from_image(image: image::RgbaImage, etag: &str) -> Self {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode the cloud fixture");
        Self {
            bytes: bytes.into_inner(),
            etag: etag.to_owned(),
        }
    }
}

impl sunlit_core::assets::cloud_source::CloudSource for FixtureClouds {
    fn fetch_if_changed(
        &self,
        known_etag: Option<&str>,
    ) -> Result<Option<sunlit_core::assets::cloud_source::CloudImage>, String> {
        if known_etag == Some(self.etag.as_str()) {
            return Ok(None);
        }
        Ok(Some(sunlit_core::assets::cloud_source::CloudImage {
            bytes: self.bytes.clone(),
            etag: Some(self.etag.clone()),
            last_modified: None,
        }))
    }

    fn describe(&self) -> String {
        "cloud bands fixture".to_owned()
    }
}
