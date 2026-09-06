//! The Milky Way.

use std::sync::LazyLock;
use std::time::Duration;

use sunlit_core::assets::stars;
use sunlit_core::engine::EngineCommand;
use sunlit_core::params::SceneParams;

use crate::groups::{FIXTURES, FRAME, LANDMARK, LANDMARK_RADIUS_DEGREES, landmark_sky, sky};
use crate::harness::{Gpu, Harness, TIMEOUT, gpu};
use crate::moon::{camera_for, sky_for};
use crate::support;
use crate::textures::real_asset;

/// A camera that shows `eqj` in the sky, as far as it can be from both the
/// frame's edges and the painted globe, chosen by maximizing the smaller of
/// those two clearances over the camera's two angles.
///
/// By search, so nothing here has to know the world frame's own convention, and
/// through `sky_lens_disc`, which is the CPU's own spelling of the projection
/// rather than a new one. The frames these cases render have the Sun switched
/// off, so where it lands is not part of the choice.
///
/// Coarse then fine: four degrees over the whole sphere, then half a degree
/// over the eight degree neighborhood the coarse pass won. The clearance being
/// maximized varies over degrees rather than over half of one, so the answer is
/// the flat scan's for a fiftieth of its two hundred thousand cameras. A coarse
/// pass that landed somewhere wrong would not pass silently: every case that
/// calls this asserts that the direction it asked for really is on screen and
/// really is clear of the painted globe.
fn camera_showing(
    eqj: glam::Vec3,
    sky: &sunlit_core::scene::sky::SkyState,
    params: &SceneParams,
    viewport: glam::Vec2,
) -> (f32, f32) {
    let world = sky.world_from_eqj * eqj;
    let score = |longitude: f32, latitude: f32| -> Option<f32> {
        let camera = sunlit_core::scene::camera::OrbitalCamera::new(
            longitude,
            latitude,
            sunlit_core::scene::camera::zoom_to_distance(params.camera.zoom),
        );
        let view_direction = (camera.view_matrix() * world.extend(0.0)).truncate();
        let circle = sunlit_core::scene::sky_lens::sky_lens_disc(
            view_direction,
            0.0,
            params.sky_fov,
            glam::Vec2::ZERO,
            viewport,
        )?;
        let globe = sunlit_core::scene::sky_lens::globe_screen_circle(
            camera.mvp_matrix(viewport.x / viewport.y),
            camera.distance,
            1.0,
            camera.fov_deg,
            viewport,
        );
        let inset = circle
            .center
            .x
            .min(viewport.x - circle.center.x)
            .min(circle.center.y)
            .min(viewport.y - circle.center.y);
        Some(inset.min(circle.center.distance(globe.center) - globe.radius))
    };

    let sweep = |bounds: (f32, f32, f32, f32), step: f32, best: &mut (f32, (f32, f32))| {
        let (from_longitude, to_longitude, from_latitude, to_latitude) = bounds;
        let mut longitude = from_longitude;
        while longitude < to_longitude {
            let mut latitude = from_latitude;
            while latitude < to_latitude {
                if let Some(score) = score(longitude, latitude)
                    && score > best.0
                {
                    *best = (score, (longitude, latitude));
                }
                latitude += step;
            }
            longitude += step;
        }
    };

    // The coarse pass keeps several candidates rather than one, because the
    // score has plateaus: two framings eight degrees apart can differ by a
    // tenth of a pixel, and refining only the coarse winner would settle on
    // whichever side of the plateau the four degree grid happened to sample.
    let mut coarse: Vec<(f32, (f32, f32))> = Vec::new();
    let mut longitude = -180.0_f32;
    while longitude < 180.0 {
        let mut latitude = -85.0_f32;
        while latitude < 85.0 {
            if let Some(score) = score(longitude, latitude) {
                coarse.push((score, (longitude, latitude)));
            }
            latitude += 4.0;
        }
        longitude += 4.0;
    }
    coarse.sort_by(|a, b| b.0.total_cmp(&a.0));

    let mut best = (f32::MIN, (0.0_f32, 0.0_f32));
    for &(_, (longitude, latitude)) in coarse.iter().take(CANDIDATES) {
        sweep(
            (
                longitude - 8.0,
                longitude + 8.0,
                (latitude - 8.0).max(-85.0),
                (latitude + 8.0).min(85.0),
            ),
            0.5,
            &mut best,
        );
    }
    best.1
}

/// Coarse candidates refined at half a degree. Eight covers the plateau every
/// direction these cases ask about has, measured against the exhaustive scan
/// this replaced.
const CANDIDATES: usize = 8;

/// A unit vector in equatorial J2000 coordinates from right ascension and
/// declination, both in degrees.
fn eqj_direction(right_ascension: f32, declination: f32) -> glam::Vec3 {
    let (ra, dec) = (right_ascension.to_radians(), declination.to_radians());
    glam::Vec3::new(dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin())
}

/// Where the sky lens puts an equatorial J2000 direction, in pixels.
///
/// `scene::sky_lens::sky_lens_disc` of a zero-width cone, which is the
/// CPU's own spelling of the projection the shader inverts rather than a new
/// one.
fn eqj_screen_position(
    eqj: glam::Vec3,
    params: &SceneParams,
    viewport: glam::Vec2,
) -> Option<glam::Vec2> {
    let sky = sky_for(params);
    let view = camera_for(params).view_matrix();
    let view_direction = (view * (sky.world_from_eqj * eqj).extend(0.0)).truncate();
    sunlit_core::scene::sky_lens::sky_lens_disc(
        view_direction,
        0.0,
        params.sky_fov,
        glam::Vec2::ZERO,
        viewport,
    )
    .map(|circle| circle.center)
}

/// The painted globe's own circle, for the cases that have to ignore it.
fn globe_circle(
    params: &SceneParams,
    viewport: glam::Vec2,
) -> sunlit_core::scene::sky_lens::ScreenCircle {
    let camera = camera_for(params);
    sunlit_core::scene::sky_lens::globe_screen_circle(
        camera.mvp_matrix(viewport.x / viewport.y),
        camera.distance,
        1.0,
        camera.fov_deg,
        viewport,
    )
}

/// The frame the layer paints, and the same framing with it switched off,
/// which is what isolates the layer from the globe and the clear color.
fn panorama_pair(harness: &Harness, params: &SceneParams) -> (Vec<u8>, Vec<u8>) {
    let on = harness.picture(params, FRAME);
    let off = harness.picture(
        &SceneParams {
            milky_way_intensity: 0.0,
            ..*params
        },
        FRAME,
    );
    (on, off)
}

/// `FRAME` as the placement helpers want it.
pub(crate) fn viewport() -> glam::Vec2 {
    #[allow(clippy::cast_precision_loss)]
    glam::Vec2::new(FRAME.0 as f32, FRAME.1 as f32)
}

/// Parameters for the panorama cases: a pinned instant, a globe small enough to
/// leave sky around it, and everything else that puts light in the sky switched
/// off, so what is measured is the layer.
fn panorama_params() -> SceneParams {
    let mut params = SceneParams {
        texture_index: 0,
        sample_count: 1,
        star_intensity: 0.0,
        sun_glow: 0.0,
        moon_brightness: 0.0,
        cloud_opacity: 0.0,
        cloud_opacity_night: 0.0,
        atmo_enabled: false,
        milky_way_intensity: 1.0,
        camera: sunlit_core::scene::camera::CameraParams {
            zoom: 0.6,
            ..Default::default()
        },
        ..SceneParams::default()
    };
    params.datetime.use_custom = true;
    params.datetime.custom_hour = 2.0;
    params.datetime.custom_day_of_year = 172;
    params.datetime.custom_year = 2026;
    params
}

/// How much each pixel changed between two frames, as a sum over the channels.
fn channel_differences(on: &[u8], off: &[u8]) -> Vec<u32> {
    on.chunks_exact(4)
        .zip(off.chunks_exact(4))
        .map(|(a, b)| {
            u32::from(a[0].abs_diff(b[0]))
                + u32::from(a[1].abs_diff(b[1]))
                + u32::from(a[2].abs_diff(b[2]))
        })
        .collect()
}

/// The difference-weighted centroid of everything over `floor`, and how many
/// pixels that was.
fn difference_centroid(differences: &[u32], width: u32, floor: u32) -> (glam::Vec2, u32) {
    let mut sum = glam::Vec2::ZERO;
    let mut weight = 0.0_f32;
    let mut count = 0;
    for (index, &value) in differences.iter().enumerate() {
        if value < floor {
            continue;
        }
        #[allow(clippy::cast_possible_truncation)]
        let index = index as u32;
        #[allow(clippy::cast_precision_loss)]
        let position = glam::Vec2::new((index % width) as f32, (index / width) as f32);
        #[allow(clippy::cast_precision_loss)]
        let w = value as f32;
        sum += position * w;
        weight += w;
        count += 1;
    }
    (sum / weight.max(1.0), count)
}

/// The layer draws when it is switched on, and switching it off is the whole
/// sky's difference rather than a corner's.
#[test]
fn a_panorama_fills_the_sky_and_zero_intensity_empties_it() {
    let gpu = gpu();
    let params = panorama_params();
    let (on, off) = panorama_pair(sky(&gpu), &params);
    let differences = channel_differences(&on, &off);
    let changed = differences.iter().filter(|&&d| d > 0).count();
    let total = differences.len();
    let circle = globe_circle(&params, viewport());
    println!(
        "the panorama changes {changed} of {total} pixels, \
         with a globe {:.0} px across in the middle of them",
        circle.radius * 2.0
    );
    assert!(
        changed * 2 > total,
        "only {changed} of {total} pixels differ with the panorama on"
    );
}

/// The panorama's image of a sky position is where the star sprites put the
/// same position.
///
/// The strongest statement available about the direction-to-texel map, and the
/// one the round-trip probe cannot make: the probe holds the reconstruction to
/// the projection, and this holds the panorama's own texel layout to the
/// catalog path checked against ephemerides. A landmark painted at
/// Sirius's coordinates has to land on Sirius's sprite.
///
/// Sirius because it is the only catalog entry brighter than magnitude -1, so a
/// limit there leaves one sprite in the whole sky. Both measurements are taken
/// against the same frame with the layer or the stars switched off, so the
/// painted globe is subtracted out rather than reasoned about.
#[test]
fn the_panorama_puts_a_landmark_where_the_star_path_puts_the_same_direction() {
    let gpu = gpu();
    let harness = landmark_sky(&gpu);
    let mut params = panorama_params();
    let direction = eqj_direction(LANDMARK.0, LANDMARK.1);
    let sky = sky_for(&params);
    let viewport = viewport();
    let (longitude, latitude) = camera_showing(direction, &sky, &params, viewport);
    params.camera.longitude = longitude;
    params.camera.latitude = latitude;

    let expected = eqj_screen_position(direction, &params, viewport)
        .expect("Sirius is not at the view antipode in this framing");
    let circle = globe_circle(&params, viewport);
    assert!(
        expected.x > 0.0 && expected.x < viewport.x && expected.y > 0.0 && expected.y < viewport.y,
        "the framing puts Sirius at {expected:?}, which is off screen"
    );
    assert!(
        expected.distance(circle.center) > circle.radius + 30.0,
        "the framing puts Sirius {:.1} px from the center of a globe {:.1} px across",
        expected.distance(circle.center),
        circle.radius * 2.0
    );

    let (with_landmark, without) = panorama_pair(harness, &params);
    let (landmark, lit) =
        difference_centroid(&channel_differences(&with_landmark, &without), FRAME.0, 300);

    let starry = SceneParams {
        milky_way_intensity: 0.0,
        star_intensity: 4.0,
        star_mag_limit: -1.0,
        ..params
    };
    let with_star = harness.picture(&starry, FRAME);
    let (sprite, sprite_pixels) =
        difference_centroid(&channel_differences(&with_star, &without), FRAME.0, 60);

    println!(
        "the landmark's centroid is at {landmark:?} over {lit} pixels, \
         the sprite's at {sprite:?} over {sprite_pixels}, \
         and the lens puts the direction at {expected:?}"
    );
    // The floor tracks the fixture's area: the shared landmark is 5 degrees
    // where this case once painted its own at 4, and 1101 lit pixels were
    // measured here, so this keeps the fourteenfold headroom the 4 degree
    // disc had rather than inheriting a floor that got easier.
    assert!(lit > 78, "only {lit} pixels of the landmark are lit");
    assert!(
        (1..=400).contains(&sprite_pixels),
        "{sprite_pixels} pixels changed with one sprite in the sky"
    );
    assert!(
        landmark.distance(sprite) < 5.0,
        "the landmark is {:.1} px from the sprite for the same direction",
        landmark.distance(sprite)
    );
    assert!(
        landmark.distance(expected) < 5.0,
        "the landmark is {:.1} px from where the lens puts the direction",
        landmark.distance(expected)
    );
}

/// The wrap column is not a band of the coarsest mip.
///
/// `atan2`'s branch cut is a curve two pixels wide, a derivative being a
/// property of the fragment quad, which is a fraction of a percent of a golden
/// frame: inside its outlier allowance and absent from its mean, so a golden
/// passes with the seam in it and a second difference over the sky pixels is
/// what can see it. The fixture does not depend on right ascension at all, so a
/// pixel differing from its neighbors cannot be content, and its coarsest mip is
/// one texel holding the bands' own mean, which is far from the sky at most
/// declinations.
///
/// The painted globe is excluded, because its grid lines are features of exactly
/// the shape being measured.
#[test]
fn the_wrap_column_is_not_a_band_of_the_coarsest_mip() {
    /// The branch cut is the half plane where a direction's y is zero and its x
    /// is negative, which is right ascension 180 at every declination.
    const CUT_RIGHT_ASCENSION: f32 = 180.0;
    /// How far a sky pixel may sit from the mean of its neighbors.
    ///
    /// The clean frame reaches 4 on warp and 7 on lavapipe, which is the two
    /// rasterizers disagreeing about filtering and rounding rather than anything
    /// about the sky. The fault reaches 102 on warp and 18 on lavapipe, so the
    /// margin is wide on one adapter and narrow on the other and this sits
    /// between the two pairs; what the second adapter buys is that the case is
    /// not assumed to behave the same on both, which is the whole reason it runs
    /// on both.
    const SECOND_DIFFERENCE_TOLERANCE: i32 = 12;

    let gpu = gpu();
    let harness = sky(&gpu);
    let mut params = panorama_params();
    let state = sky_for(&params);
    let viewport = viewport();
    let (longitude, latitude) = camera_showing(
        eqj_direction(CUT_RIGHT_ASCENSION, 0.0),
        &state,
        &params,
        viewport,
    );
    params.camera.longitude = longitude;
    params.camera.latitude = latitude;

    // Vacuity guard: the case says nothing unless the cut crosses the frame.
    let mut on_screen = 0;
    for declination in [-60.0_f32, -30.0, 0.0, 30.0, 60.0] {
        let Some(position) = eqj_screen_position(
            eqj_direction(CUT_RIGHT_ASCENSION, declination),
            &params,
            viewport,
        ) else {
            continue;
        };
        if position.x >= 0.0
            && position.x < viewport.x
            && position.y >= 0.0
            && position.y < viewport.y
        {
            on_screen += 1;
        }
    }
    assert!(
        on_screen >= 2,
        "the branch cut crosses the frame at only {on_screen} of the five declinations sampled"
    );

    let pixels = harness.picture(&params, FRAME);
    let circle = globe_circle(&params, viewport);

    let width = FRAME.0 as usize;
    let height = FRAME.1 as usize;
    let value = |x: usize, y: usize| i32::from(pixels[(y * width + x) * 4]);
    let sky_pixel = |x: usize, y: usize| {
        #[allow(clippy::cast_precision_loss)]
        let position = glam::Vec2::new(x as f32, y as f32);
        position.distance(circle.center) > circle.radius + 2.0
    };
    let mut largest = 0;
    let mut worst_at = (0, 0);
    let mut anomalies = 0;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let neighbors = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)];
            if !sky_pixel(x, y) || !neighbors.iter().all(|&(nx, ny)| sky_pixel(nx, ny)) {
                continue;
            }
            // Second differences on both axes: the bands are smooth enough that
            // theirs is a fraction of a code value, while anything one or two
            // pixels wide has its own height in one of the two.
            let here = 2 * value(x, y);
            let across = (value(x - 1, y) + value(x + 1, y) - here).abs();
            let down = (value(x, y - 1) + value(x, y + 1) - here).abs();
            let curvature = across.max(down);
            if curvature > SECOND_DIFFERENCE_TOLERANCE {
                anomalies += 1;
            }
            if curvature > largest {
                largest = curvature;
                worst_at = (x, y);
            }
        }
    }
    println!(
        "the largest second difference among the sky pixels is {largest} at {worst_at:?}, \
         and {anomalies} of them are over {SECOND_DIFFERENCE_TOLERANCE}"
    );
    assert!(
        largest <= SECOND_DIFFERENCE_TOLERANCE,
        "the sky pixel at {worst_at:?} sits {largest} away from the mean of its neighbors, \
         and {anomalies} of them do: this panorama does not depend on right ascension and \
         its bands are tens of pixels wide, so nothing in it can turn over in one"
    );
}

/// The panorama rescales with the sky field of view and the painted globe does
/// not, which is what keeps the two lenses from drifting apart.
///
/// The landmark is a piece of sky of a fixed angular size, so halving the field
/// of view has to roughly quadruple its area; the globe has the camera's own 20
/// degree lens and cannot move at all.
#[test]
fn the_panorama_tracks_the_sky_field_of_view_and_the_globe_does_not() {
    let gpu = gpu();
    let harness = landmark_sky(&gpu);
    let mut params = panorama_params();
    // Further out than the other cases, so a landmark magnified by the narrow
    // end of the slider still has room beside the globe.
    params.camera.zoom = 0.8;
    let state = sky_for(&params);
    let viewport = viewport();
    // Chosen at the narrow end, where the layer is magnified most and the
    // landmark is hardest to keep in frame.
    let (longitude, latitude) = camera_showing(
        eqj_direction(LANDMARK.0, LANDMARK.1),
        &state,
        &SceneParams {
            sky_fov: 60.0,
            ..params
        },
        viewport,
    );
    params.camera.longitude = longitude;
    params.camera.latitude = latitude;

    let measure = |sky_fov: f32| {
        let framing = SceneParams { sky_fov, ..params };
        let sky = sky_for(&framing);
        let view_direction = (camera_for(&framing).view_matrix()
            * (sky.world_from_eqj * eqj_direction(LANDMARK.0, LANDMARK.1)).extend(0.0))
        .truncate();
        let disc = sunlit_core::scene::sky_lens::sky_lens_disc(
            view_direction,
            LANDMARK_RADIUS_DEGREES.to_radians(),
            sky_fov,
            glam::Vec2::ZERO,
            viewport,
        )
        .expect("in front of the lens");
        let circle = globe_circle(&framing, viewport);
        assert!(
            disc.center.distance(circle.center) > circle.radius + disc.radius + 3.0,
            "at {sky_fov} degrees of sky a landmark {:.1} px across sits {:.1} px from a \
             globe {:.1} px across",
            disc.radius * 2.0,
            disc.center.distance(circle.center),
            circle.radius * 2.0
        );
        let (on, off) = panorama_pair(harness, &framing);
        let landmark = channel_differences(&on, &off)
            .iter()
            .filter(|&&d| d > 300)
            .count();
        // On the frame with no panorama in it the sky is the clear color, whose
        // green channel is far below anything the grid texture paints.
        let globe = off.chunks_exact(4).filter(|px| px[1] >= 60).count();
        (landmark, globe)
    };

    let (wide_landmark, wide_globe) = measure(120.0);
    let (narrow_landmark, narrow_globe) = measure(60.0);
    #[allow(clippy::cast_precision_loss)]
    let landmark_ratio = narrow_landmark as f32 / wide_landmark as f32;
    #[allow(clippy::cast_precision_loss)]
    let globe_ratio = narrow_globe as f32 / wide_globe as f32;
    println!(
        "halving the sky field of view takes the landmark from {wide_landmark} px to \
         {narrow_landmark} ({landmark_ratio:.2}x) and the globe from {wide_globe} to \
         {narrow_globe} ({globe_ratio:.3}x)"
    );
    assert!(
        wide_landmark > 200,
        "the landmark is only {wide_landmark} px"
    );
    assert!(
        landmark_ratio > 3.0,
        "the landmark's area grew {landmark_ratio:.2}x, where halving the field of view \
         should be about four"
    );
    assert!(
        (globe_ratio - 1.0).abs() < 0.02,
        "the globe's area moved by {globe_ratio:.3}x, and the sky slider is not its lens"
    );
}

/// The engine whose panorama slot holds the shipped asset, where the checkout
/// has it.
///
/// `None` where `textures/**` is still Git LFS pointers, which is what makes
/// the two cases that need it skip rather than fail.
struct RealSky {
    harness: Harness,
    cache: std::path::PathBuf,
}

impl std::ops::Deref for RealSky {
    type Target = Harness;
    fn deref(&self) -> &Harness {
        &self.harness
    }
}

/// The width the real sky loads at, and the one the cap case reaches up to.
///
/// The narrow end is the default because the halving is what the cache holds:
/// the engine writes it once at startup and the case that reads it back costs
/// nothing, where starting wide would mean decoding the 4096 source three more
/// times.
const REAL_SKY_WIDTH: u32 = 2048;
const REAL_SKY_CAP: u32 = 8192;

static REAL_SKY: LazyLock<Option<RealSky>> = LazyLock::new(|| {
    let path = real_asset("milkyway_2020_4k.jxl").ok()?;
    let dir = FIXTURES.join("real_sky");
    std::fs::create_dir_all(&dir).expect("create the real sky cache directory");
    let cache = dir.clone();
    let harness = Harness::start(move |config| {
        config.preview_size = FRAME;
        config.params = panorama_params();
        config.texture_paths = vec![None, None, None, Some(path)];
        config.cache_dir = Some(dir);
        config.texture_resolution = REAL_SKY_WIDTH;
    });
    harness.wait_for_slot_texture("milky_way_texture");
    Some(RealSky { harness, cache })
});

/// The shipped panorama's engine, or `None` with a printed reason.
fn real_sky(_gpu: &Gpu) -> Option<&'static RealSky> {
    if let Err(why) = real_asset("milkyway_2020_4k.jxl") {
        println!("skipping: {why}; `git lfs pull` fetches the assets");
        return None;
    }
    let group = REAL_SKY.as_ref()?;
    // The layer has to be wanted before the width is restored, for the reason
    // `surface` puts the mode back first.
    group
        .harness
        .engine
        .send(EngineCommand::UpdateParams(Box::new(panorama_params())));
    if group.harness.restore_resolution() {
        wait_for_panorama_width(group, REAL_SKY_WIDTH);
    }
    group.harness.reset();
    Some(group)
}

/// Block until the panorama in the report is `width` wide, and answer with its
/// size.
fn wait_for_panorama_width(harness: &Harness, width: u32) -> (u32, u32) {
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        let report = harness
            .engine
            .memory_report()
            .expect("the engine should answer with a report");
        if let Some(texture) = report
            .expected
            .iter()
            .find(|texture| texture.label == "milky_way_texture")
            && texture.width == width
        {
            return (texture.width, texture.height);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no {width} wide panorama within {TIMEOUT:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The real panorama has the galactic plane where the plane is.
///
/// The fixture cases pin the map from a direction to a texel, but the fixture
/// and the shader are written from one reading of the asset's own layout, so
/// neither can catch that reading being wrong. This can: it samples the
/// rendered sky at the galactic center, at both galactic poles, and at two
/// stretches of the plane far from the center, and the ordering it asserts is
/// the one a mirrored or transposed reading gets backwards.
///
/// One framing per sample, each with the sample 40 degrees off the view axis,
/// because the five directions span the whole sky and no single frame holds
/// them.
///
/// Skips with a printed reason where `textures/**` is still Git LFS pointers.
#[test]
fn the_real_panorama_has_the_galactic_plane_where_the_plane_is() {
    let gpu = gpu();
    let Some(harness) = real_sky(&gpu) else {
        return;
    };

    let base = panorama_params();
    let state = sky_for(&base);
    let viewport = viewport();
    let sample = |name: &str, right_ascension: f32, declination: f32| {
        let direction = eqj_direction(right_ascension, declination);
        let (longitude, latitude) = camera_showing(direction, &state, &base, viewport);
        let mut params = base;
        params.camera.longitude = longitude;
        params.camera.latitude = latitude;
        let position = eqj_screen_position(direction, &params, viewport)
            .expect("in front of the lens at this framing");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (x, y) = (position.x.round() as u32, position.y.round() as u32);
        assert!(
            (5..FRAME.0 - 5).contains(&x) && (5..FRAME.1 - 5).contains(&y),
            "{name} is at ({x}, {y}), which is not a window inside the frame"
        );
        assert!(
            position.distance(globe_circle(&params, viewport).center)
                > globe_circle(&params, viewport).radius + 10.0,
            "{name} lands on the painted globe"
        );
        let pixels = harness.picture(&params, FRAME);
        // A window rather than a pixel, because the sky is Gaia photon noise
        // and one texel of it is not what is being compared.
        let mut total = 0_u32;
        let mut count = 0_u32;
        for wy in y.saturating_sub(4)..(y + 5).min(FRAME.1) {
            for wx in x.saturating_sub(4)..(x + 5).min(FRAME.0) {
                let index = ((wy * FRAME.0 + wx) * 4) as usize;
                total += u32::from(pixels[index])
                    + u32::from(pixels[index + 1])
                    + u32::from(pixels[index + 2]);
                count += 1;
            }
        }
        let mean = total / count.max(1);
        println!("  {name} at ({x}, {y}): mean {mean} of 765");
        mean
    };

    let bulge = sample("the galactic center", 266.42, -29.01);
    let north_pole = sample("the north galactic pole", 192.86, 27.13);
    let south_pole = sample("the south galactic pole", 12.86, -27.13);
    let cygnus = sample("the plane through Cygnus", 310.4, 45.3);
    let carina = sample("the plane through Carina", 160.0, -59.0);

    for (name, pole) in [
        ("the north galactic pole", north_pole),
        ("the south galactic pole", south_pole),
    ] {
        assert!(
            bulge > pole * 3,
            "the galactic center reads {bulge} and {name} {pole}, which is not this sky"
        );
        assert!(
            cygnus > pole,
            "the plane through Cygnus reads {cygnus} and {name} {pole}"
        );
        assert!(
            carina > pole,
            "the plane through Carina reads {carina} and {name} {pole}"
        );
    }
}

/// No star the sprite pipeline draws is baked into the real panorama.
///
/// The `milkyway_2020` layer is the SVS map with the Hipparcos and Tycho stars
/// taken out, which is what keeps a bright star from being drawn twice: once as
/// its own sprite and once as a blob under it. That is a property of the file
/// that shipped rather than of the description it came with, and it is a
/// property a re-bake from the source could lose without anything else moving.
///
/// The measure is the one the asset was checked with by hand: the mean of a 3x3
/// texel window at the star's own position against the mean of the 41x41 window
/// around it, on the file as it sits on disk. Every catalog record the star draw
/// submits inside `MAGNITUDE_LIMIT` is measured, rather than a hand-picked list,
/// so the set is the one the sprites come from.
///
/// The bound is what separates the two answers, and the numbers on both sides
/// of it are measured. Across the 21 records inside the limit the ratio runs
/// from 0.73 to 1.26, which is bright stars sitting in bright parts of the Milky
/// Way and nothing more. A star baked into the layer saturates the texels it
/// covers, and such a core reads 2.32 against the brightest surround in the set
/// and 5 or more against a typical one; the wrong SVS layer would do that to
/// most of the 21 at once. Two sits between the two.
///
/// What this window cannot see is a star confined to a single texel in the
/// brightest part of the plane, which stays inside the bound;
/// `docs/rendering.md` records why the peak texel does not fix that.
///
/// Skips with a printed reason where `textures/**` is still Git LFS pointers.
#[test]
fn no_bright_star_is_baked_into_the_real_panorama() {
    /// Bright enough to be a sprite nothing could hide under.
    const MAGNITUDE_LIMIT: f32 = 1.3;
    /// Half width of the core window, which is 3x3 texels.
    const CORE: i32 = 1;
    /// Half width of the surrounding window, which is 41x41.
    const SURROUND: i32 = 20;
    /// The core may be this much brighter than what surrounds it.
    const RATIO_BOUND: f64 = 2.0;

    let path = match real_asset("milkyway_2020_4k.jxl") {
        Ok(path) => path,
        Err(why) => {
            println!("skipping: {why}; `git lfs pull` fetches the assets");
            return;
        }
    };
    sunlit_core::assets::texture_loader::register_jxl_hook();
    let panorama = image::open(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
        .to_rgba8();
    let width = i32::try_from(panorama.width()).expect("a panorama of a sane width");
    let height = i32::try_from(panorama.height()).expect("a panorama of a sane height");

    // Wrapped in u and clamped in v, which is what the map itself does at its
    // seam and at its poles.
    let window_mean = |cx: i32, cy: i32, half: i32| -> f64 {
        let mut total = 0_u32;
        let mut count = 0_u32;
        for dy in -half..=half {
            let y = (cy + dy).clamp(0, height - 1);
            for dx in -half..=half {
                let x = (cx + dx).rem_euclid(width);
                let texel = panorama.get_pixel(
                    u32::try_from(x).expect("wrapped into the map"),
                    u32::try_from(y).expect("clamped into the map"),
                );
                total += u32::from(texel[0]) + u32::from(texel[1]) + u32::from(texel[2]);
                count += 1;
            }
        }
        f64::from(total) / f64::from(count.max(1))
    };

    let catalog = sunlit_core::assets::stars::embedded_catalog();
    let visible = usize::try_from(catalog.visible_count(MAGNITUDE_LIMIT)).expect("a small prefix");
    assert!(
        visible >= 15,
        "only {visible} catalog records are inside magnitude {MAGNITUDE_LIMIT}, \
         which is not the brightest sky"
    );
    let records = catalog.instance_bytes();

    let mut worst = (0.0_f64, 0.0_f32, 0.0_f32);
    for index in 0..visible {
        let record = &records[index * stars::RECORD_SIZE..(index + 1) * stars::RECORD_SIZE];
        let component = |offset: usize| {
            f32::from_le_bytes(
                record[offset..offset + 4]
                    .try_into()
                    .expect("four bytes of a direction"),
            )
        };
        let direction = glam::Vec3::new(component(0), component(4), component(8)).normalize();
        let right_ascension = direction
            .y
            .atan2(direction.x)
            .to_degrees()
            .rem_euclid(360.0);
        let declination = direction.z.clamp(-1.0, 1.0).asin().to_degrees();
        let magnitude = f32::from(record[15]) / 255.0 * 10.0 - 2.0;

        let (u, v) = support::panorama_texel(
            right_ascension,
            declination,
            panorama.width(),
            panorama.height(),
        );
        #[allow(clippy::cast_possible_truncation)]
        let (cx, cy) = (u.floor() as i32, v.floor() as i32);
        let core = window_mean(cx, cy, CORE);
        let surround = window_mean(cx, cy, SURROUND);
        let ratio = core / surround.max(1.0);
        println!(
            "  magnitude {magnitude:.2} at ra {right_ascension:.2} dec {declination:.2}: \
             core {core:.1} of 765, surround {surround:.1}, ratio {ratio:.2}"
        );
        if ratio > worst.0 {
            worst = (ratio, right_ascension, declination);
        }
    }

    println!(
        "the brightest core against its surroundings over {visible} stars is {:.2}, \
         at ra {:.2} dec {:.2}",
        worst.0, worst.1, worst.2
    );
    assert!(
        worst.0 < RATIO_BOUND,
        "a star at ra {:.2} dec {:.2} reads {:.2} times its surroundings, over {RATIO_BOUND}: \
         this panorama has bright stars in it and the sprites are drawing them again",
        worst.1,
        worst.2,
        worst.0
    );
}

/// The panorama follows the texture resolution cap, and the halving cache is
/// what serves the narrow end.
///
/// Its source is 4096 wide, which is between the widest and the narrowest of
/// the three widths the config offers, so it is the one texture where the
/// setting is a cap in both directions: 8192 loads it as it is and 2048 halves
/// it. That halving is what keeps the layer's 42.7 MiB from being the price of
/// choosing the low setting, so it is worth knowing the file is written and
/// read rather than assuming it.
///
/// Skips with a printed reason where `textures/**` is still Git LFS pointers.
#[test]
fn the_panorama_follows_the_texture_resolution_cap() {
    let gpu = gpu();
    let Some(group) = real_sky(&gpu) else {
        return;
    };

    // The engine loads at the narrow end, and the cache directory is this run's
    // own, so the file being there is the halving having been written.
    let narrow = wait_for_panorama_width(group, REAL_SKY_WIDTH);
    println!(
        "at the {REAL_SKY_WIDTH} setting the panorama loads at {}x{}",
        narrow.0, narrow.1
    );
    assert_eq!(narrow, (2048, 1024));
    let cached = group
        .cache
        .join("texture_cache")
        .join("milkyway_2020_4k.2048.png");
    assert!(
        cached.exists(),
        "the halving was not written to {}",
        cached.display()
    );

    let wide = wait_for_panorama_width_after(group, REAL_SKY_CAP, 4096);
    println!(
        "at the {REAL_SKY_CAP} setting the panorama loads at {}x{}",
        wide.0, wide.1
    );
    assert_eq!(
        wide,
        (4096, 2048),
        "the widest setting is a cap, and the file is 4096 wide"
    );

    // Back down, which is the load that reads what the first one wrote rather
    // than halving the source again. The width alone cannot show that; the file
    // having to be there for it to succeed can.
    assert_eq!(
        wait_for_panorama_width_after(group, REAL_SKY_WIDTH, REAL_SKY_WIDTH),
        narrow
    );
}

/// Reload the panorama at `resolution` and wait until it is `width` wide.
fn wait_for_panorama_width_after(group: &RealSky, resolution: u32, width: u32) -> (u32, u32) {
    group.set_texture_resolution(resolution);
    wait_for_panorama_width(group, width)
}
