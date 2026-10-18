//! The ocean colors over the Earth fixture: a new color reaches the water and
//! nothing else, and the grid takes none.

use sunlit_core::assets::cube_layout::CubeTextures;
use sunlit_core::assets::texture_loader;
use sunlit_core::color::Rgb8;
use sunlit_core::geometry::cube;
use sunlit_core::params::SceneParams;
use sunlit_core::scene::camera::OrbitalCamera;

use crate::groups::FRAME;
use crate::harness::{Harness, gpu};
use crate::test_support::{self, ScratchDir};
use crate::tiles::{EARTH, day_over};

/// A day color far from the fixture's fill in every channel.
const FAR: Rgb8 = Rgb8::new(200, 40, 120);

/// How far, in texels of the 256 px mask faces, a pixel's place has to be
/// from the other kind for it to count as land or as water: the 128 px mask
/// the pack holds is read bilinearly, and a coast's BC4 block can reach a
/// texel's neighbors, so the margin spans a filter footprint and a block.
const MARGIN: i64 = 10;

/// What the fixture's own mask faces say of a place.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Place {
    Land,
    Water,
    Coast,
}

/// The fixture's mask faces, decoded, in cube layer order.
fn mask_faces(textures: &std::path::Path) -> Vec<image::GrayImage> {
    texture_loader::register_jxl_hook();
    CubeTextures::resolve(textures)
        .mask
        .iter()
        .map(|path| {
            let path = path.as_ref().expect("a complete mask");
            let mut reader = image::ImageReader::open(path)
                .unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
            reader.no_limits();
            reader
                .decode()
                .unwrap_or_else(|e| panic!("decode {}: {e}", path.display()))
                .into_luma8()
        })
        .collect()
}

/// Land or water when every mask texel within [`MARGIN`] of the place under
/// `normal` agrees, and a coast otherwise, a face edge included.
fn place_of(faces: &[image::GrayImage], normal: glam::DVec3) -> Place {
    let (face, across, down) = cube::locate(normal);
    let mask = &faces[face];
    let size = mask.width();
    #[expect(clippy::cast_possible_truncation, reason = "a texel on a face")]
    let (column, row) = (
        cube::texel_coordinate(across, size).round() as i64,
        cube::texel_coordinate(down, size).round() as i64,
    );
    let inside = MARGIN..i64::from(size) - MARGIN;
    if !inside.contains(&column) || !inside.contains(&row) {
        return Place::Coast;
    }
    let mut seen = [false; 2];
    for dy in -MARGIN..=MARGIN {
        for dx in -MARGIN..=MARGIN {
            #[expect(
                clippy::cast_sign_loss,
                clippy::cast_possible_truncation,
                reason = "inside the face"
            )]
            let value = mask.get_pixel((column + dx) as u32, (row + dy) as u32).0[0];
            match value {
                0 => seen[0] = true,
                u8::MAX => seen[1] = true,
                _ => return Place::Coast,
            }
        }
    }
    match seen {
        [true, false] => Place::Land,
        [false, true] => Place::Water,
        _ => Place::Coast,
    }
}

/// The unit normal of the globe under each pixel of an export of `params` at
/// `FRAME`, `None` off the globe.
fn normals(params: &SceneParams) -> Vec<Option<glam::DVec3>> {
    let (width, height) = FRAME;
    let aspect = f64::from(width) / f64::from(height);
    #[expect(clippy::cast_possible_truncation, reason = "an aspect ratio")]
    let mvp = OrbitalCamera::from_params(&params.camera).mvp_matrix(aspect as f32);
    let inverse = mvp.as_dmat4().inverse();
    (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let ndc_x = (f64::from(x) + 0.5) / f64::from(width) * 2.0 - 1.0;
            let ndc_y = 1.0 - (f64::from(y) + 0.5) / f64::from(height) * 2.0;
            let near = inverse.project_point3(glam::DVec3::new(ndc_x, ndc_y, 0.0));
            let far = inverse.project_point3(glam::DVec3::new(ndc_x, ndc_y, 1.0));
            let ray = (far - near).normalize();
            let half_b = near.dot(ray);
            let discriminant = half_b * half_b - (near.length_squared() - 1.0);
            (discriminant >= 0.0)
                .then(|| (near + ray * (-half_b - discriminant.sqrt())).normalize())
        })
        .collect()
}

/// In Day mode a new day color moves every pixel where the fixture's mask is
/// open water and leaves every pixel where it is land exactly as it was. Over
/// the Gulf of Guinea, with Africa's interior and the Atlantic in the frame,
/// within 24 degrees of the equator, where no sea is frozen.
#[test]
fn a_day_color_moves_the_water_and_leaves_the_land() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_ocean_day");
    test_support::write_earth_fixture(&dir.join("textures"));
    let [fill, _] = test_support::EARTH_FIXTURE_OCEAN;
    let at_fill = SceneParams {
        day_ocean_color: Rgb8::from(fill),
        ..day_over(5.0, 0.0, 0.12)
    };
    let harness = Harness::start(|config| {
        config.cube_textures = CubeTextures::resolve(&dir.join("textures"));
        config.tile_geometry = EARTH;
        config.cache_dir = Some(dir.join("cache"));
        config.params = at_fill;
    });
    harness.wait_for_textures("the day floor and the mask");

    let before = harness.picture(&at_fill, FRAME);
    let after = harness.picture(
        &SceneParams {
            day_ocean_color: FAR,
            ..at_fill
        },
        FRAME,
    );
    let faces = mask_faces(&dir.join("textures"));
    let (mut land, mut water) = (0, 0);
    for ((n, b), a) in normals(&at_fill)
        .into_iter()
        .zip(before.chunks_exact(4))
        .zip(after.chunks_exact(4))
    {
        let Some(n) = n else { continue };
        if n.y.abs() > 0.4 {
            continue;
        }
        match place_of(&faces, n) {
            Place::Land => {
                land += 1;
                assert_eq!(a, b, "a pixel over land moved with the day's ocean color");
            }
            Place::Water => {
                water += 1;
                assert!(
                    a[0] >= b[0] + 100,
                    "a pixel over open water went from {b:?} to {a:?} for {FAR}"
                );
            }
            Place::Coast => {}
        }
    }
    assert!(
        land > 1_000 && water > 1_000,
        "{land} pixels of land and {water} of water in the frame"
    );
}

/// The grid's pixels do not depend on the two ocean colors: the grid's group
/// binds the dummy mask, which reads no water, so nothing is moved whatever
/// shift reaches the shader. That the shift is zero on the grid in the first
/// place is `surface::tests::a_frame_measures_its_ocean_colors_from_the_floors_it_draws`.
#[test]
fn the_grid_draws_the_same_pixels_with_any_ocean_colors() {
    let _gpu = gpu();
    let dir = ScratchDir::new("engine_ocean_grid");
    test_support::write_earth_fixture(&dir.join("textures"));
    let grid = SceneParams {
        texture_index: 0,
        ..day_over(5.0, 0.0, 0.12)
    };
    let harness = Harness::start(|config| {
        config.cube_textures = CubeTextures::resolve(&dir.join("textures"));
        config.tile_geometry = EARTH;
        config.cache_dir = Some(dir.join("cache"));
        config.params = grid;
    });
    harness.wait_for_textures("the grid");
    let before = harness.picture(&grid, FRAME);
    let after = harness.picture(
        &SceneParams {
            day_ocean_color: FAR,
            night_ocean_color: Rgb8::new(255, 255, 0),
            ..grid
        },
        FRAME,
    );
    assert_eq!(before, after);
}
