use sunlit_core::params::SceneParams;

use crate::groups::{FRAME, plain};
use crate::harness::{Harness, gpu, test_params};

/// A night-side framing at a longitude chosen for where the Sun lands.
///
/// At noon on day 172 the subsolar point is near the prime meridian, so a
/// camera on the far side looks at the night side with the Sun somewhere
/// beyond the limb. Which side of the painted limb it lands on is what the
/// longitude picks: 160 stands a whole horizon zone and its adaptation reach
/// clear of the band, 168 puts the disk's lower edge exactly at the top of the
/// zone, 170.5 grazes the atmosphere band, 172 leaves the disk inside the band
/// with a hundredth of its light, and 176 puts it well inside the painted disc.
/// The two cases about a draw's own cull take the same family further round,
/// to 68 and 105, where the Sun is past a frame's corner rather than near the
/// limb. The atmosphere is off so that the only thing these cases can be
/// measuring is the Sun.
///
/// Those five numbers are for `FRAME`, the size these cases export at: at
/// another aspect ratio the painted silhouette is a different size and all five
/// move.
fn sun_params(longitude: f32) -> SceneParams {
    let mut params = test_params();
    params.datetime.custom_day_of_year = 172;
    params.camera.longitude = longitude;
    params.camera.latitude = 0.0;
    params.camera.zoom = 0.45;
    params.atmo_enabled = false;
    params
}

/// Render `params` with the Sun off and then on, and return both frames.
pub(crate) fn sun_off_and_on(harness: &Harness, longitude: f32) -> (Vec<u8>, Vec<u8>) {
    sun_off_and_on_framed(harness, sun_params(longitude))
}

/// The same for a framing the caller has already adjusted.
fn sun_off_and_on_framed(harness: &Harness, params: SceneParams) -> (Vec<u8>, Vec<u8>) {
    sun_off_and_on_at(harness, params, 1.5)
}

/// The same again at a glare strength of the caller's choosing, for a case
/// that needs the disk clipped white while the glare around it is not.
fn sun_off_and_on_at(harness: &Harness, params: SceneParams, glow: f32) -> (Vec<u8>, Vec<u8>) {
    let mut off = params;
    off.sun_glow = 0.0;
    let off = harness.picture(&off, FRAME);
    let mut on = params;
    on.sun_glow = glow;
    let on = harness.picture(&on, FRAME);
    (off, on)
}

/// Refraction is what could have taken this case away, and does not.
///
/// The lift is at most 0.46 of a horizon zone, which is 1.47 pixels here, and
/// it is spent long before this framing: the lifted disk's center sits 78.6
/// pixels from the globe's own center with a radius of 1.6 against a painted
/// limb at 81.35, so its whole image is inside. What the squash then does runs
/// the same way, since the flattened disk is compared against a limb moved out
/// by the same factor, which at the saturated squash here is 98.7 pixels.
#[test]
fn a_sun_behind_the_painted_globe_paints_nothing() {
    let gpu = gpu();
    let (off, on) = sun_off_and_on(plain(&gpu), 176.0);
    let differing = off
        .chunks_exact(4)
        .zip(on.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{differing} pixels changed with the Sun fully behind the globe"
    );
}

#[test]
fn zero_sun_glow_takes_the_sun_out_of_the_frame() {
    let gpu = gpu();
    let (off, on) = sun_off_and_on(plain(&gpu), 160.0);
    let mut painted = 0_usize;
    for (index, (dark, lit)) in off.chunks_exact(4).zip(on.chunks_exact(4)).enumerate() {
        if dark == lit {
            continue;
        }
        painted += 1;
        // The two sun draws are additive, so every pixel the Sun touches can
        // only have got brighter.
        assert!(
            (0..3).all(|c| lit[c] >= dark[c]),
            "pixel {index} went from {dark:?} to {lit:?}, which additive blending cannot do"
        );
    }
    assert!(
        painted > 100,
        "a Sun clear of the limb painted only {painted} pixels"
    );
}

#[test]
fn a_sun_grazing_the_limb_turns_the_glare_warm() {
    let gpu = gpu();
    let harness = plain(&gpu);
    // What each longitude adds to its own sun-off frame, summed per channel.
    // Comparing that against the other longitude's would compare two different
    // Earths; comparing each against its own leaves only the Sun.
    let warmth = |longitude: f32| {
        let (off, on) = sun_off_and_on(harness, longitude);
        let mut added = [0_u64; 3];
        for (dark, lit) in off.chunks_exact(4).zip(on.chunks_exact(4)) {
            for (channel, total) in added.iter_mut().enumerate() {
                *total += u64::from(lit[channel].saturating_sub(dark[channel]));
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let ratio = added[0] as f64 / added[2].max(1) as f64;
        ratio
    };
    let clear = warmth(160.0);
    let grazing = warmth(170.5);
    assert!(
        clear < 1.15,
        "a Sun clear of the atmosphere should glare near-white, red over blue was {clear:.2}"
    );
    assert!(
        grazing > clear * 1.3,
        "a Sun in the transit band should glare warmer than a clear one, \
         but red over blue was {grazing:.2} against {clear:.2}"
    );
}

/// Pixels the Sun added more than four levels to, and the whole of the light
/// it added.
///
/// The total rather than the brightest pixel: the disk is drawn clipped white
/// wherever it is drawn at all, so a framing that carries any of it at all has
/// a brightest gain of 254 whatever the exposure does, and the two framings
/// below would compare equal at every setting. The gain multiplies the glare's
/// amplitude, so what it moves is how much light there is.
fn sun_light(harness: &Harness, params: SceneParams) -> (usize, u64) {
    let (off, on) = sun_off_and_on_framed(harness, params);
    let mut painted = 0;
    let mut total = 0;
    for (dark, lit) in off.chunks_exact(4).zip(on.chunks_exact(4)) {
        let gained = [0, 1, 2].map(|c| lit[c].saturating_sub(dark[c]));
        if gained.iter().any(|&value| value > 4) {
            painted += 1;
        }
        total += gained.into_iter().map(u64::from).sum::<u64>();
    }
    (painted, total)
}

/// A disk the band has taken almost all of still glares.
///
/// At longitude 172 the disk is three quarters clear of the painted limb and
/// carries 1.1 percent of its light, which is what the compressive response is
/// for: the tenth of that the square root leaves is a glare a person sees, and
/// a linear one would be a frame with a red dot in it. Measured, it paints
/// 3527 pixels, and 558 of them with the exposure gain taken out.
#[test]
fn a_sliver_of_sun_over_the_limb_still_glares() {
    let gpu = gpu();
    let (painted, _) = sun_light(plain(&gpu), sun_params(172.0));
    assert!(
        painted > 2000,
        "a sliver of Sun in the band painted only {painted} pixels"
    );
}

/// The glare peaks as the disk stands clear of the horizon zone.
///
/// Both framings carry the whole disk and all of its light, so the flux, the
/// tint and the squash are the same at each and the exposure gain is the only
/// thing between them: 3 at 168, where the disk's lower edge is at the top of
/// the zone, and 1 at 160, where it is past the adaptation reach. Measured, the
/// first adds 2.74 times what the second does rather than the gain's own 3,
/// because the brightest of the glare is clipped at both.
#[test]
fn the_glare_peaks_as_the_disk_clears_the_horizon() {
    let gpu = gpu();
    let harness = plain(&gpu);
    let (_, at_the_zone) = sun_light(harness, sun_params(168.0));
    let (_, well_clear) = sun_light(harness, sun_params(160.0));
    assert!(
        at_the_zone > well_clear * 2,
        "the glare at the top of the zone added {at_the_zone} against \
         {well_clear} for a Sun well clear of it, which is no peak at all"
    );
}

/// At `sun_horizon_boost` 1 there is no peak, which is the physical answer.
///
/// The same two framings, so what is left when the gain is 1 at both is the
/// difference between two positions on the frame: the glare falls on a
/// different part of the globe's own brightness at each, and where it is
/// clipped is where the two cannot agree exactly. Measured, that is 0.971
/// against the 2.74 the case above gets at the default.
#[test]
fn the_physical_exposure_has_no_peak() {
    let gpu = gpu();
    let harness = plain(&gpu);
    let flat = |longitude: f32| {
        let mut params = sun_params(longitude);
        params.sun_horizon_boost = 1.0;
        sun_light(harness, params).1
    };
    let at_the_zone = flat(168.0);
    let well_clear = flat(160.0);
    #[allow(clippy::cast_precision_loss)]
    let ratio = at_the_zone as f64 / well_clear as f64;
    assert!(
        (0.9..1.1).contains(&ratio),
        "with the boost at 1 the two framings should glare alike, but the one \
         at the top of the zone added {at_the_zone} against {well_clear}, a \
         ratio of {ratio:.3}"
    );
}

/// The user's own framing: 3.7 Earth radii, 140 degrees of sky, the latitude
/// and the framing the golden suite's `horizon_camera` uses.
///
/// The atmosphere is on, unlike the sun cases above, because what these
/// measure is the band the shell draws. Three longitudes matter here, and all
/// three are geometry rather than taste: at 163.35 the true Sun stands on the
/// true limb, 15.68 degrees off the view axis, while its image through the sky
/// lens is 52 pixels from the frame's center and the painted limb is 204, so
/// there is no Sun to see anywhere near the limb; at 117.66 the image sits one
/// horizon zone inside the painted limb, which takes a true Sun 57.1 degrees
/// off the axis; and at 115.93 the disk's lower edge stands on the limb.
fn horizon_params(longitude: f32) -> SceneParams {
    let mut params = test_params();
    params.datetime.custom_day_of_year = 172;
    params.camera.longitude = longitude;
    params.camera.latitude = -23.44;
    params.camera.zoom = 0.227_047_34;
    params.sky_fov = 140.0;
    params
}

/// What the Sun and its band add to a frame, as red minus blue.
///
/// Against the same frame with both switched off, so the globe's own texture
/// and the shell's own blue cancel and what is left is the light this
/// amendment moved. Red minus blue because the band and the transmitted disk
/// are red by construction and everything else the frame holds is not.
fn sunrise_excess(harness: &Harness, longitude: f32) -> i64 {
    let params = horizon_params(longitude);
    let mut dark = params;
    dark.sun_glow = 0.0;
    dark.atmo_sunrise_glow = 0.0;
    let dark = harness.picture(&dark, FRAME);
    let lit = harness.picture(&params, FRAME);
    lit.chunks_exact(4)
        .zip(dark.chunks_exact(4))
        .map(|(lit, dark)| {
            i64::from(lit[0].saturating_sub(dark[0])) - i64::from(lit[2].saturating_sub(dark[2]))
        })
        .sum()
}

/// The glow arrives with the Sun's image rather than with the true Sun.
///
/// The lobe on the Rayleigh shell is measured through the sky lens, so it
/// fires where the Sun is drawn. Measured at 512 by 256 with the lobe in the
/// true scattering angle instead, the framing with no Sun to see adds 1287
/// against the 12222 of the framing whose image stands on the limb, a ninth of
/// it, which is a glow that arrives well before the Sun. Through the sky lens
/// the same two are 381 and 31221, an eighty-second.
#[test]
fn the_sunrise_band_arrives_with_the_suns_image() {
    let gpu = gpu();
    let harness = plain(&gpu);
    let no_sun_to_see = sunrise_excess(harness, 163.353_15);
    let image_at_the_limb = sunrise_excess(harness, 117.658_22);
    let disk_emerged = sunrise_excess(harness, 115.931_64);
    assert!(
        no_sun_to_see * 20 < image_at_the_limb,
        "a framing whose Sun is nowhere near the painted limb still reddened it \
         by {no_sun_to_see} against the {image_at_the_limb} of one whose image \
         stands on it"
    );
    assert!(
        disk_emerged > image_at_the_limb,
        "the emerged disk added {disk_emerged}, less than the {image_at_the_limb} \
         of the framing that has no disk in it at all"
    );
}

/// A Sun past the reach of an unpanned frame is drawn once a pan reaches it.
///
/// Both sun draws cull themselves against `sky_corner_angle`, the angle of the
/// frame's furthest corner widened by the pan. The glare's cone is 30 degrees
/// wide, so the widening decides anything only where the Sun is more than 30
/// degrees past an unpanned corner: nearer than that the glare draw clears its
/// own cull without the pan and paints the same pixels either way. At
/// longitude 68 the Sun sits 110.5 degrees off the view axis against a 76.1
/// degree corner, which is past both, and a pan of 0.9 brings the frame's
/// nearest pixel to 10.2 degrees from it.
#[test]
fn a_pan_past_the_frame_corner_still_draws_the_sun() {
    /// Pixels the Sun added more than a handful of levels to, and its most.
    fn added(harness: &Harness, params: SceneParams) -> (usize, u8) {
        let (off, on) = sun_off_and_on_framed(harness, params);
        let mut painted = 0;
        let mut brightest = 0;
        for (dark, lit) in off.chunks_exact(4).zip(on.chunks_exact(4)) {
            let gained = [0, 1, 2].map(|c| lit[c].saturating_sub(dark[c]));
            if gained.iter().any(|&value| value > 4) {
                painted += 1;
            }
            brightest = brightest.max(gained.into_iter().max().unwrap_or(0));
        }
        (painted, brightest)
    }

    let gpu = gpu();
    let harness = plain(&gpu);
    let mut framed = sun_params(68.0);
    assert_eq!(
        added(harness, framed),
        (0, 0),
        "no point of an unpanned frame is within the glare's cone here, so \
         the Sun may not touch a pixel of it"
    );

    framed.camera.offset_x = -0.9;
    let (painted, brightest) = added(harness, framed);
    assert!(
        painted > 4000 && brightest > 8,
        "the pan brings the frame's nearest pixel to 10.2 degrees from the \
         Sun, but only {painted} pixels gained more than four levels and the \
         brightest gained {brightest}"
    );
}

/// A disk the size slider has enlarged is drawn where its crescent reaches
/// the corner, which the true half degree alone would leave outside.
///
/// The disk draw culls itself on a cone of its own angular radius, and
/// `sun_size` multiplies that radius, so the cone the cull measures has to
/// carry the size the draw does. At longitude 105 the Sun stands 76.62
/// degrees off the view axis against a 76.11 degree corner: the true half
/// degree puts the whole disk outside the frame and eight times it puts a
/// crescent of it inside the top left one. The glare is the same either way,
/// and a quarter of the glow is where the core is already clipped white
/// (`SUN_CORE_GAIN` is 4) while the bloom around it is not, so a gain of more
/// than 200 levels is the disk and nothing else: measured, the crescent is 32
/// pixels and the glare without it gains at most 52.
#[test]
fn an_enlarged_disk_reaching_the_frame_corner_is_drawn() {
    let gpu = gpu();
    let mut framed = sun_params(105.0);
    framed.sun_size = 8.0;
    framed.sun_rays = 0.0;
    let (off, on) = sun_off_and_on_at(plain(&gpu), framed, 0.25);
    let clipped = off
        .chunks_exact(4)
        .zip(on.chunks_exact(4))
        .filter(|(dark, lit)| {
            [0, 1, 2]
                .iter()
                .any(|&c| lit[c].saturating_sub(dark[c]) > 200)
        })
        .count();
    assert!(
        clipped > 12,
        "the enlarged disk's crescent reaches inside the frame's corner, but \
         only {clipped} pixels there gained more than 200 levels"
    );
}

/// Panning the frame slides the whole composite across the framebuffer, so a
/// panned frame is the unpanned one moved by the pan and nothing else.
///
/// The pan reaches the Sun through four places that each carry a sign:
/// `scene::sun_occlusion::sky_lens_disc`, which is where the CPU decides what
/// the globe hides, and `sun_disc`, `sky_corner_angle` and `sky_lens_direction`
/// in the shader. Any one of them disagreeing with the pan the globe got leaves
/// the composite sheared rather than moved, which no other case in the suite
/// would see: every other frame in it is rendered with no pan at all.
#[test]
fn a_pan_slides_the_composite_without_shearing_it() {
    // An eighth of the frame's width, which is a whole number of pixels, so
    // the two frames compare without resampling either.
    const PAN: f32 = -0.25;
    const SHIFT: usize = 64;

    let panned_params = |offset_x: f32| {
        let mut params = sun_params(160.0);
        params.sun_glow = 1.5;
        params.camera.offset_x = offset_x;
        params
    };
    let gpu = gpu();
    let harness = plain(&gpu);
    let (width, height) = FRAME;
    let centered = harness.picture(&panned_params(0.0), FRAME);
    let panned = harness.picture(&panned_params(PAN), FRAME);

    let row = width as usize * 4;
    let compared = height as usize * (width as usize - SHIFT) * 3;
    let mut worst = 0_u8;
    let mut worst_at = (0_usize, 0_usize);
    let mut total = 0_u64;
    let mut outliers = 0_u64;
    let mut lit = 0_u64;
    for y in 0..height as usize {
        for x in 0..width as usize - SHIFT {
            let from = y * row + x * 4;
            let to = y * row + (x + SHIFT) * 4;
            if centered[from..from + 3].iter().any(|&c| c > 8) {
                lit += 1;
            }
            for channel in 0..3 {
                let difference = centered[from + channel].abs_diff(panned[to + channel]);
                total += u64::from(difference);
                if difference > 1 {
                    outliers += 1;
                }
                if difference > worst {
                    worst = difference;
                    worst_at = (x, y);
                }
            }
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let mean = total as f64 / compared as f64;
    assert!(
        lit > 2000,
        "only {lit} pixels of the compared region carry anything, so this \
         would pass on an empty frame"
    );
    // A step of the 8-bit output is the budget, because the glare dithers from
    // the framebuffer position and that does not travel with the pan, and
    // because a center is a pan plus a projection rather than a projection
    // shifted by whole pixels, so an antialiased edge can round the other way.
    // The handful of channels allowed past it are where an interpolated value
    // is steep enough that the same last bit of the vertex moves it further:
    // 0 of 344,064 on warp and 5 on lavapipe, against tens of thousands for
    // any of the signs being wrong.
    assert!(
        mean < 0.15 && outliers <= 64,
        "the panned frame is not the unpanned one moved by {SHIFT} pixels: \
         mean {mean:.4}, {outliers} of {compared} channels off by more than \
         one, worst {worst} at {worst_at:?}"
    );
}
