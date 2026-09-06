//! The Moon.

use sunlit_core::params::SceneParams;

use crate::groups::{FRAME, plain, sky};
use crate::harness::{gpu, test_params};
use crate::memory::expected_widths;
use crate::support;

/// A framing with the Moon in it, and nothing else that emits light.
///
/// The stars, the Sun and the atmosphere are all switched off, so the only
/// thing these cases can be measuring is the Moon; the camera looks at the
/// night side, where the sky lens has room to show it. The pinned instant is
/// the one the moon golden uses.
fn moon_params() -> SceneParams {
    let mut params = test_params();
    params.datetime.custom_day_of_year = 172;
    params.datetime.custom_hour = 21.0;
    params.camera.longitude = 160.0;
    params.camera.latitude = 0.0;
    params.camera.zoom = 0.45;
    params.atmo_enabled = false;
    params.star_intensity = 0.0;
    params.sun_glow = 0.0;
    params.sky_fov = 60.0;
    params.moon_size = 8.0;
    // Back on, since `test_params` switches every overlay off.
    params.moon_brightness = SceneParams::default().moon_brightness;
    // Earthshine well above the clear color, so the unlit face is part of what
    // "only adds light" is measured over rather than a wash against the sky.
    params.moon_earthshine = 0.2;
    params
}

/// The Moon adds light to a dark sky and takes none away.
///
/// With nothing else drawn, every pixel the Moon touches can only get brighter,
/// which is the shape `sun_off_and_on` uses: one engine, one `UpdateParams`, an
/// off frame against an on frame.
#[test]
fn a_moon_on_the_night_sky_only_adds_light() {
    let gpu = gpu();
    let harness = sky(&gpu);
    let params = moon_params();
    let on = harness.picture(&params, FRAME);

    let mut without = params;
    without.moon_brightness = 0.0;
    let off = harness.picture(&without, FRAME);

    let mut brighter = 0;
    for (before, after) in off.chunks_exact(4).zip(on.chunks_exact(4)) {
        let sum = |px: &[u8]| u32::from(px[0]) + u32::from(px[1]) + u32::from(px[2]);
        assert!(
            sum(after) >= sum(before),
            "a pixel went from {before:?} to {after:?} with nothing but the Moon drawn"
        );
        if sum(after) > sum(before) + 30 {
            brighter += 1;
        }
    }
    assert!(
        brighter > 500,
        "only {brighter} pixels got brighter with an eight times Moon in frame"
    );
}

/// A Moon switched off and a Moon with no texture behind it are the same
/// picture, which is what makes the missing asset a non-event.
#[test]
fn a_switched_off_moon_and_a_missing_texture_draw_the_same_frame() {
    let gpu = gpu();
    let params = moon_params();

    // The one case that holds two engines at once, and it needs them because
    // the difference it is about is a configuration rather than a parameter:
    // the shared sky engine has a Moon behind its slot and the plain one has
    // nothing behind any of them.
    let mut off = params;
    off.moon_brightness = 0.0;
    let with_texture = sky(&gpu).picture(&off, FRAME);
    let without_texture = plain(&gpu).picture(&params, FRAME);

    let differing = with_texture
        .chunks_exact(4)
        .zip(without_texture.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{differing} pixels differ between a switched-off Moon and a missing one"
    );
}

/// A Moon at the antipode of the view axis is not drawn at all.
///
/// The camera looks at the origin, so an eye on the line from the Earth to the
/// Moon, at any zoom short of the orbit, has the Moon exactly behind it. A cone
/// that reaches the lens's antipode has no finite image, which is what
/// `place_moon` reports by leaving the disc empty: the vertices would otherwise
/// land thousands of units out in every radial direction at once and the mesh's
/// triangles would sweep the frame. The globe drag reaches that camera, so the
/// frame it produces has to be the frame with no Moon in it.
#[test]
fn a_moon_at_the_view_antipode_draws_nothing() {
    let (width, height) = FRAME;

    let gpu = gpu();
    let harness = sky(&gpu);
    let mut params = moon_params();
    let direction = sky_for(&params).moon_position.normalize();
    params.camera.latitude = direction.y.asin().to_degrees();
    params.camera.longitude = direction.x.atan2(direction.z).to_degrees();

    #[allow(clippy::cast_precision_loss)]
    let viewport = glam::Vec2::new(width as f32, height as f32);
    assert_eq!(
        moon_placement(&params, viewport).disc,
        None,
        "this framing is the one where the disc is empty, or it measures nothing"
    );

    let behind = harness.picture(&params, FRAME);
    let mut without = params;
    without.moon_brightness = 0.0;
    let off = harness.picture(&without, FRAME);

    let differing = behind
        .chunks_exact(4)
        .zip(off.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{differing} pixels differ between a Moon behind the camera and no Moon at all"
    );
}

/// The Moon's texture lands in the slot the layout reserves for it, at the
/// width of the file behind it.
#[test]
fn the_moon_texture_lands_in_its_own_slot() {
    let gpu = gpu();
    let harness = sky(&gpu);
    let report = harness
        .engine
        .memory_report()
        .expect("the engine should answer with a report");
    assert_eq!(
        expected_widths(&report, "moon_texture"),
        [support::MOON_FIXTURE_WIDTH]
    );
}

/// The instant `doy` and `hour` name, in 2026.
fn moon_time(doy: u16, hour: i32) -> astronomy_engine_bindings::astro_time_t {
    let (month, day) = sunlit_core::scene::datetime::day_of_year_to_month_day(doy, 2026);
    sunlit_core::scene::sun::make_time(2026, i32::from(month), i32::from(day), hour, 0, 0.0)
}

/// The fraction of the Moon's disc an ephemeris says is lit, seen from the
/// geocenter. The independent answer these cases are measured against.
#[allow(clippy::cast_possible_truncation)]
fn illuminated_fraction(doy: u16, hour: i32) -> f32 {
    // SAFETY: Astronomy_Illumination is a pure C function taking and returning
    // value types.
    #[allow(unsafe_code)]
    let illumination = unsafe {
        astronomy_engine_bindings::Astronomy_Illumination(
            astronomy_engine_bindings::astro_body_t_BODY_MOON,
            moon_time(doy, hour),
        )
    };
    assert_eq!(
        illumination.status,
        astronomy_engine_bindings::astro_status_t_ASTRO_SUCCESS,
        "Astronomy_Illumination failed"
    );
    illumination.phase_fraction as f32
}

/// The sky state the engine computes for `params`.
///
/// Every framing here pins its datetime, so the clock a live one would consult
/// does not enter the answer, and the year and the fractional hour come from
/// the parameters instead of being assumed.
pub(crate) fn sky_for(params: &SceneParams) -> sunlit_core::scene::sky::SkyState {
    assert!(
        params.datetime.use_custom,
        "the placement helpers only answer for a pinned datetime"
    );
    sunlit_core::scene::sky::compute_sky_state(&params.datetime)
}

/// The camera `params` describes, as far as the placement needs it.
///
/// `write_uniforms` applies the pan and the orientation and the helpers below
/// pass none of them, so a case that set one would measure a disc away from
/// where the Moon is drawn; refused rather than answered wrong.
pub(crate) fn camera_for(params: &SceneParams) -> sunlit_core::scene::camera::OrbitalCamera {
    let cam = &params.camera;
    assert!(
        [
            cam.offset_x,
            cam.offset_y,
            cam.tilt_deg,
            cam.yaw_deg,
            cam.pitch_deg
        ]
        .iter()
        .all(|value| *value == 0.0),
        "the placement helpers carry no pan, tilt, yaw or pitch"
    );
    sunlit_core::scene::camera::OrbitalCamera::new(
        cam.longitude,
        cam.latitude,
        sunlit_core::scene::camera::zoom_to_distance(cam.zoom),
    )
}

/// Where the Moon lands for `params` at `viewport`, disc and all.
fn moon_placement(
    params: &SceneParams,
    viewport: glam::Vec2,
) -> sunlit_core::scene::moon::MoonPlacement {
    let sky = sky_for(params);
    let camera = camera_for(params);
    sunlit_core::scene::moon::place_moon(&sunlit_core::scene::moon::MoonPlacementInputs {
        position: sky.moon_position,
        rotation: sky.moon_rotation,
        eye: camera.eye_position(),
        view: camera.view_matrix(),
        size: params.moon_size,
        sky_fov_deg: params.sky_fov,
        screen_offset: glam::Vec2::ZERO,
        viewport,
    })
}

/// Where the Moon's disc lands, and how large, for `params` at `viewport`.
fn moon_disc(
    params: &SceneParams,
    viewport: glam::Vec2,
) -> sunlit_core::scene::sun_occlusion::ScreenCircle {
    moon_placement(params, viewport)
        .disc
        .expect("the moon is on screen at these framings")
}

/// The Sun's position on the same screen, for the cases that need it.
fn sun_screen_position(params: &SceneParams, viewport: glam::Vec2) -> glam::Vec2 {
    let sky = sky_for(params);
    let camera = camera_for(params);
    let view_direction = (camera.view_matrix() * sky.sun_direction.extend(0.0))
        .truncate()
        .normalize();
    sunlit_core::scene::sun_occlusion::sky_lens_disc(
        view_direction,
        0.0,
        params.sky_fov,
        glam::Vec2::ZERO,
        viewport,
    )
    .expect("the sun has an image at these framings")
    .center
}

/// Every pixel within a tenth of a radius of the disk, as (position, red
/// channel), from a render.
///
/// A circle rather than a box, and only a tenth wider than the disk, because
/// the painted globe reaches inside the box at some of these framings and its
/// grid is bright. The tenth is what lets a disk drawn larger than the circle
/// the CPU placed show up as too much lit area rather than being cropped out of
/// the measurement.
fn disc_pixels(
    pixels: &[u8],
    width: u32,
    disc: sunlit_core::scene::sun_occlusion::ScreenCircle,
) -> Vec<(glam::Vec2, u8)> {
    let reach = disc.radius * 1.1;
    let mut out = Vec::new();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    for y in (disc.center.y - reach).max(0.0) as u32..(disc.center.y + reach) as u32 {
        for x in (disc.center.x - reach).max(0.0) as u32..(disc.center.x + reach) as u32 {
            let position = glam::Vec2::new(x as f32, y as f32);
            if position.distance(disc.center) > reach {
                continue;
            }
            let index = ((y * width + x) * 4) as usize;
            out.push((position, pixels[index]));
        }
    }
    out
}

/// The lit fraction of the drawn disk against an ephemeris, at three phases.
///
/// The count comes from the GPU and the area from `scene::moon::place_moon`, so
/// a disk drawn at the wrong size misses this as surely as a phase computed the
/// wrong way round, and a terminator on the wrong side reports one minus the
/// answer. The tolerance covers three things: the camera's own parallax, which
/// is nine Earth radii against the Moon's sixty and moves the phase by up to
/// 0.02 at these framings; the disk's own edge, which is not antialiased and so
/// quantizes the area by about one part in the radius; and the terminator's
/// smoothstep, which is a band a pixel or so wide. Measured on warp at 48 pixels
/// of radius: 0.2089 against the ephemeris 0.2272, 0.4461 against 0.4439, and
/// 0.8619 against 0.8450, so the worst of the three is 0.018 against the 0.04
/// this allows.
#[test]
fn the_lit_fraction_tracks_the_ephemeris_at_three_phases() {
    const WIDTH: u32 = 1600;
    const HEIGHT: u32 = 800;
    /// Crescent, quarter and gibbous, all with the Moon clear of the painted
    /// globe and the camera's displacement nearly across the Sun's direction,
    /// which is what keeps the geocentric answer applicable.
    const INSTANTS: [(u16, i32); 3] = [(199, 16), (189, 8), (185, 4)];

    let gpu = gpu();
    let harness = sky(&gpu);
    let mut params = moon_params();
    // No earthshine, so the unlit face is black and the threshold is a
    // question about sunlight rather than about the floor.
    params.moon_earthshine = 0.0;

    #[allow(clippy::cast_precision_loss)]
    let viewport = glam::Vec2::new(WIDTH as f32, HEIGHT as f32);
    for (doy, hour) in INSTANTS {
        let mut at = params;
        at.datetime.custom_day_of_year = doy;
        at.datetime.custom_hour = f32::from(u16::try_from(hour).expect("a small hour"));
        let pixels = harness.picture(&at, (WIDTH, HEIGHT));

        let disc = moon_disc(&at, viewport);
        let lit = disc_pixels(&pixels, WIDTH, disc)
            .into_iter()
            .filter(|(_, red)| *red > 40)
            .count();
        #[allow(clippy::cast_precision_loss)]
        let fraction = lit as f32 / (std::f32::consts::PI * disc.radius * disc.radius);
        let expected = illuminated_fraction(doy, hour);
        println!(
            "day {doy} hour {hour}: disk radius {:.1} px, {lit} lit pixels,              fraction {fraction:.4} against the ephemeris {expected:.4}",
            disc.radius
        );
        assert!(
            (fraction - expected).abs() < 0.04,
            "day {doy} hour {hour}: the drawn fraction {fraction:.4} is not the              ephemeris {expected:.4}"
        );
    }
}

/// The lit limb faces the Sun.
///
/// The brightness centroid of the disk sits on the sunward side of its center,
/// and the direction from the center to the centroid is the direction of the
/// Sun on screen. That is the check a person makes by eye when they look at a
/// crescent, and it is the one thing the phase fraction cannot see: a
/// terminator rotated by ninety degrees leaves the fraction untouched.
#[test]
fn the_lit_limb_faces_the_sun() {
    const WIDTH: u32 = 1600;
    const HEIGHT: u32 = 800;

    let gpu = gpu();
    let harness = sky(&gpu);
    let mut params = moon_params();
    params.moon_earthshine = 0.0;
    params.datetime.custom_day_of_year = 199;
    params.datetime.custom_hour = 16.0;
    let pixels = harness.picture(&params, (WIDTH, HEIGHT));

    #[allow(clippy::cast_precision_loss)]
    let viewport = glam::Vec2::new(WIDTH as f32, HEIGHT as f32);
    let disc = moon_disc(&params, viewport);
    let mut weight = 0.0_f32;
    let mut centroid = glam::Vec2::ZERO;
    for (position, red) in disc_pixels(&pixels, WIDTH, disc) {
        let value = f32::from(red);
        weight += value;
        centroid += position * value;
    }
    assert!(weight > 0.0, "the disk painted nothing");
    centroid /= weight;

    let toward_light = (sun_screen_position(&params, viewport) - disc.center).normalize();
    let toward_centroid = (centroid - disc.center).normalize_or_zero();
    let separation = toward_centroid.angle_to(toward_light).abs().to_degrees();
    println!(
        "the lit centroid is {:.1} px from the disk's center, {separation:.1} degrees off          the direction of the Sun",
        centroid.distance(disc.center)
    );
    assert!(
        separation < 10.0,
        "the lit side points {separation:.1} degrees away from the Sun"
    );
}

/// The glare fades behind a Moon that covers the Sun.
///
/// The instant is the greatest eclipse of the 2024-04-08 total solar eclipse,
/// where `the_moon_covers_the_sun_at_the_2024_total_eclipse` measures the two
/// geocentric directions 0.347 degrees apart. The camera puts its view axis
/// seven degrees off the Sun, which is the window where two things are true at
/// once: the Sun's image clears the painted globe, so there is a glare to fade,
/// and the camera's own parallax leaves the Moon inside its own disc of the Sun.
/// The assertion is on pixels well outside the Moon's silhouette, because those
/// can only have changed through `sun_visible`: the Moon paints nothing there,
/// and dropping the Moon's disc on the way into `place_sun` leaves them
/// identical.
#[test]
fn a_moon_over_the_sun_fades_the_glare_around_it() {
    const WIDTH: u32 = 1024;
    const HEIGHT: u32 = 256;
    /// The view axis this far off the Sun, in degrees.
    const OFF_AXIS: f32 = 7.0;

    let gpu = gpu();
    let harness = sky(&gpu);
    let mut params = moon_params();
    params.sun_glow = 1.0;
    params.datetime.custom_year = 2024;
    params.datetime.custom_day_of_year = 99;
    params.datetime.custom_hour = 18.0 + 17.0 / 60.0;

    // The eye on the night side, swung `OFF_AXIS` out of the Earth-Sun line, so
    // the Sun sits that far from the view axis and the Moon almost with it.
    let sunward = sky_for(&params).sun_direction.normalize();
    let across = sunward.cross(glam::Vec3::Y).normalize();
    let radians = OFF_AXIS.to_radians();
    let eye = -sunward * radians.cos() + across * radians.sin();
    params.camera.latitude = eye.y.asin().to_degrees();
    params.camera.longitude = eye.x.atan2(eye.z).to_degrees();

    #[allow(clippy::cast_precision_loss)]
    let viewport = glam::Vec2::new(WIDTH as f32, HEIGHT as f32);
    let disc = moon_disc(&params, viewport);
    let sun = sun_screen_position(&params, viewport);
    println!(
        "the moon's disc is {:.1} px across at ({:.1}, {:.1}), the sun at ({:.1}, {:.1}), \
         {:.1} px apart",
        disc.radius * 2.0,
        disc.center.x,
        disc.center.y,
        sun.x,
        sun.y,
        disc.center.distance(sun)
    );
    assert!(
        disc.center.distance(sun) + 2.0 < disc.radius,
        "this framing is meant to put the Sun's disk inside the Moon's"
    );

    let eclipsed = harness.picture(&params, (WIDTH, HEIGHT));
    let mut without = params;
    without.moon_brightness = 0.0;
    let burning = harness.picture(&without, (WIDTH, HEIGHT));

    // Far enough out that the Moon's own mesh cannot reach, since the disc is
    // the image of the cone every one of its vertices is inside.
    let reach = disc.radius * 1.5;
    let mut dimmed = 0;
    for (index, (with, out)) in eclipsed
        .chunks_exact(4)
        .zip(burning.chunks_exact(4))
        .enumerate()
    {
        #[allow(clippy::cast_precision_loss)]
        let position = glam::Vec2::new(
            (index % WIDTH as usize) as f32,
            (index / WIDTH as usize) as f32,
        );
        if position.distance(disc.center) <= reach {
            continue;
        }
        let sum = |px: &[u8]| u32::from(px[0]) + u32::from(px[1]) + u32::from(px[2]);
        assert!(
            sum(with) <= sum(out),
            "a pixel {:.1} px from the Moon went from {out:?} to {with:?} with the Moon over \
             the Sun",
            position.distance(disc.center)
        );
        if sum(with) + 3 < sum(out) {
            dimmed += 1;
        }
    }
    println!("{dimmed} pixels outside the Moon's silhouette dimmed with the Sun covered");
    assert!(
        dimmed > 1000,
        "only {dimmed} pixels dimmed outside the Moon's silhouette"
    );
}

/// Earthshine lifts the unlit face and nothing else.
///
/// The golden cannot see this: the floor at its default of 0.05 changes the
/// window it compares by a mean of 0.64 against a tolerance of 2.00. So it is
/// pinned here, where a count of pixels needs no tolerance: the frames with and
/// without it differ only inside the disk, and only upward.
#[test]
fn earthshine_lifts_the_unlit_face_only() {
    let gpu = gpu();
    let harness = sky(&gpu);
    let mut params = moon_params();
    params.moon_earthshine = 0.0;
    let dark = harness.picture(&params, FRAME);

    let mut lifted = params;
    lifted.moon_earthshine = 0.3;
    let shone = harness.picture(&lifted, FRAME);

    #[allow(clippy::cast_precision_loss)]
    let disc = moon_disc(&params, glam::Vec2::new(FRAME.0 as f32, FRAME.1 as f32));
    let mut raised = 0;
    let width = FRAME.0 as usize;
    for (index, (before, after)) in dark.chunks_exact(4).zip(shone.chunks_exact(4)).enumerate() {
        if before == after {
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let position = glam::Vec2::new((index % width) as f32, (index / width) as f32);
        assert!(
            position.distance(disc.center) <= disc.radius + 1.5,
            "earthshine changed a pixel {:.1} px from the disk's center, which is {:.1} across",
            position.distance(disc.center),
            disc.radius * 2.0
        );
        assert!(
            after[0] >= before[0] && after[1] >= before[1] && after[2] >= before[2],
            "earthshine darkened a pixel from {before:?} to {after:?}"
        );
        raised += 1;
    }
    println!("earthshine raised {raised} pixels of the disk");
    assert!(
        raised > 100,
        "only {raised} pixels changed with the earthshine floor at 0.3"
    );
}
