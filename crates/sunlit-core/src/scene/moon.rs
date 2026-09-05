//! Where the Moon goes on screen, and how large.
//!
//! The Moon is a textured sphere at its true position, and everything about how
//! it lands on screen follows from measuring it from the eye rather than from
//! the geocenter: parallax is exact at every camera distance, including the
//! ones beyond the Moon's own orbit that the zoom control reaches, and so is
//! apparent size, which grows as the camera approaches it. Neither needs a
//! calculation of its own.
//!
//! Two things happen here and not in the shader. The model matrix is assembled,
//! so the vertex shader reads one transform and knows nothing about lunar
//! coordinates; and the disk is floored to the same
//! `sun_occlusion::MIN_BODY_DISK_RADIUS_PIXELS` the Sun takes, so the two
//! bodies that subtend the same half degree are the same size on screen.
//! The constant and the [`super::sun_occlusion::pixel_scale`] ramp are what the
//! two share; the flooring itself is not. `place_sun` clamps the projected
//! radius directly, which leaves a disc that is no longer the image of any
//! cone, while the Moon's true radius is inflated and re-imaged, so its disc
//! stays a real projection and its center moves a little with it.

use glam::{Mat3, Mat4, Vec2, Vec3};

use super::sun_occlusion::{MIN_BODY_DISK_RADIUS_PIXELS, ScreenCircle, pixel_scale, sky_lens_disc};

/// The Moon's radius in the scene's unit of length: 1737.4 km against the
/// Earth's equatorial 6378.137.
pub(crate) const MOON_RADIUS_EARTH_RADII: f32 = 0.2724;

/// The frame geometry [`place_moon`] needs.
#[derive(Clone, Copy, Debug)]
pub struct MoonPlacementInputs {
    /// Geocentric moon position in world space, in Earth radii.
    pub position: Vec3,
    /// Rotation from the Moon's body-fixed frame into world space.
    pub rotation: Mat3,
    /// The camera's position in world space.
    pub eye: Vec3,
    /// The camera's view matrix, shared by both lenses.
    pub view: Mat4,
    /// The `moon_size` parameter: a multiplier on the radius, and on nothing
    /// else.
    pub size: f32,
    pub sky_fov_deg: f32,
    /// Post-projection pan, in NDC, as the sky lens applies it.
    pub screen_offset: Vec2,
    pub viewport: Vec2,
}

/// Everything the uniform encoder needs to draw the Moon and to fade the Sun's
/// glare behind it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoonPlacement {
    /// Takes a mesh vertex to world space: scale, then the body's orientation,
    /// then its position.
    pub model: Mat4,
    /// The silhouette the Moon covers, at the floored radius.
    ///
    /// `None` where there is nothing to cover: the Moon at the antipode of the
    /// view axis, or an eye inside it.
    pub disc: Option<ScreenCircle>,
}

/// The mesh's local frame against a body-fixed one.
///
/// `geometry::sphere` puts the north pole on +Y and the prime meridian on +Z,
/// which is exactly where `scene::sky`'s world frame puts the Earth's, and the
/// flip and quarter shift in `assets::texture_loader::orient` line an
/// equirectangular map up with that. So a body-fixed point reaches the mesh
/// through the same permutation for either body, and one texture convention
/// serves the Earth's maps and the Moon's alike.
fn mesh_from_body() -> Mat3 {
    Mat3::from_cols(Vec3::Z, Vec3::X, Vec3::Y)
}

/// Place the Moon, with its disk no smaller than the Sun's floor.
pub fn place_moon(inputs: &MoonPlacementInputs) -> MoonPlacement {
    let to_moon = inputs.position - inputs.eye;
    let distance = to_moon.length();
    let view_direction = (inputs.view * to_moon.extend(0.0))
        .truncate()
        .normalize_or_zero();
    let disc_for = |radius: f32| {
        if radius <= 0.0 || radius >= distance {
            return None;
        }
        sky_lens_disc(
            view_direction,
            (radius / distance).asin(),
            inputs.sky_fov_deg,
            inputs.screen_offset,
            inputs.viewport,
        )
    };

    let true_radius = MOON_RADIUS_EARTH_RADII * inputs.size.max(0.0);
    let floor = MIN_BODY_DISK_RADIUS_PIXELS * pixel_scale(inputs.viewport.y);
    let (radius, disc) = match disc_for(true_radius) {
        // Inflating the radius is the same operation `moon_size` performs, so
        // direction, distance and parallax are all untouched by it. The disc is
        // taken again afterwards rather than scaled, because the lens is not
        // linear in the half-angle and this way there is no approximation to
        // hold. A disc that projected to exactly nothing is the one radius the
        // ratio cannot be taken against; any other, however small, gives a
        // factor that is finite or infinite, and `disc_for` refuses an inflated
        // radius that reaches the eye.
        Some(disc) if disc.radius < floor && disc.radius > 0.0 => {
            let inflated = true_radius * floor / disc.radius;
            (inflated, disc_for(inflated))
        }
        disc => (true_radius, disc),
    };

    MoonPlacement {
        model: Mat4::from_translation(inputs.position)
            * Mat4::from_mat3(inputs.rotation * mesh_from_body().transpose())
            * Mat4::from_scale(Vec3::splat(radius)),
        disc,
    }
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::*;
    use crate::scene::camera::OrbitalCamera;
    use crate::scene::sky::{SkyState, compute_sky_state_from_time};
    use crate::scene::sun::make_time;

    const VIEWPORT: Vec2 = Vec2::new(1920.0, 1080.0);

    fn sky() -> SkyState {
        compute_sky_state_from_time(make_time(2026, 6, 21, 12, 0, 0.0))
    }

    fn inputs(sky: &SkyState, camera: &OrbitalCamera, size: f32) -> MoonPlacementInputs {
        MoonPlacementInputs {
            position: sky.moon_position,
            rotation: sky.moon_rotation,
            eye: camera.eye_position(),
            view: camera.view_matrix(),
            size,
            sky_fov_deg: 140.0,
            screen_offset: Vec2::ZERO,
            viewport: VIEWPORT,
        }
    }

    /// A mesh vertex, in the sphere's own local frame.
    fn mesh_point(longitude_deg: f32, latitude_deg: f32) -> Vec3 {
        let (lon, lat) = (longitude_deg.to_radians(), latitude_deg.to_radians());
        // The inverse of the permutation the model matrix applies, spelled out
        // rather than borrowed, so a change to it fails here.
        Vec3::new(lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos())
    }

    /// The whole texture-orientation chain in one assertion: the point the
    /// oriented map puts longitude zero at, taken to world space by the model
    /// matrix, faces the Earth. Tidal lock says it must, to within the eight
    /// degrees of libration.
    #[test]
    fn the_meshs_prime_meridian_faces_the_earth() {
        let sky = sky();
        let camera = OrbitalCamera::new(0.0, 0.0, 8.0);
        let placement = place_moon(&inputs(&sky, &camera, 1.0));
        let surface = placement.model * mesh_point(0.0, 0.0).extend(1.0);
        let outward = (surface.truncate() - sky.moon_position).normalize();
        let toward_earth = (-sky.moon_position).normalize();
        let separation = outward.angle_between(toward_earth).to_degrees();
        assert!(separation < 8.5, "separation was {separation} degrees");
    }

    /// And the mesh's pole is the Moon's pole, which is what the north-up half
    /// of the orientation rests on.
    #[test]
    fn the_meshs_north_pole_is_the_moons_north_pole() {
        let sky = sky();
        let camera = OrbitalCamera::new(0.0, 0.0, 8.0);
        let placement = place_moon(&inputs(&sky, &camera, 1.0));
        let surface = placement.model * mesh_point(0.0, 90.0).extend(1.0);
        let outward = (surface.truncate() - sky.moon_position).normalize();
        let separation = outward.angle_between(sky.moon_rotation.z_axis).to_degrees();
        assert!(separation < 0.01, "separation was {separation} degrees");
    }

    /// The mesh is a unit sphere, so the model matrix's scale is the radius the
    /// Moon is drawn at, and `moon_size` multiplies it.
    #[test]
    fn the_size_parameter_multiplies_the_radius() {
        let sky = sky();
        let camera = OrbitalCamera::new(0.0, 0.0, 8.0);
        let radius_of = |size| {
            let placement = place_moon(&inputs(&sky, &camera, size));
            (placement.model * mesh_point(0.0, 0.0).extend(1.0)).truncate() - sky.moon_position
        };
        assert_relative_eq!(
            radius_of(4.0).length() / radius_of(1.0).length(),
            4.0,
            epsilon = 1e-4
        );
        assert_relative_eq!(
            radius_of(1.0).length(),
            MOON_RADIUS_EARTH_RADII,
            epsilon = 1e-4
        );
    }

    /// A camera that moves toward the Moon sees a larger one, with no
    /// calculation of its own: the direction and the distance both come from
    /// the eye.
    #[test]
    fn approaching_the_moon_makes_it_larger() {
        let sky = sky();
        let camera = OrbitalCamera::new(0.0, 0.0, 8.0);
        // Straight at the Moon, from two distances along the way to it.
        let eye_toward_moon = |distance: f32| {
            let mut i = inputs(&sky, &camera, 1.0);
            i.eye = sky.moon_position.normalize() * distance;
            i.view = Mat4::look_at_rh(i.eye, sky.moon_position, Vec3::Y);
            place_moon(&i).disc.expect("the moon is on screen").radius
        };
        let small = eye_toward_moon(2.0);
        let large = eye_toward_moon(sky.moon_position.length() / 3.0);
        assert!(
            large > small * 1.4,
            "radius went from {small} to {large} pixels"
        );
    }

    /// The Sun's floor, applied to the Moon: at the default field of view on a
    /// small frame the true disk is under two pixels across, and the model's
    /// scale is inflated by exactly the ratio that fixes it.
    #[test]
    fn a_sub_pixel_disk_is_floored_like_the_suns() {
        let sky = sky();
        let camera = OrbitalCamera::new(0.0, 0.0, 8.0);
        let mut small = inputs(&sky, &camera, 1.0);
        small.viewport = Vec2::new(512.0, 256.0);
        let placement = place_moon(&small);
        let disc = placement.disc.expect("the moon is on screen");
        assert_relative_eq!(disc.radius, MIN_BODY_DISK_RADIUS_PIXELS, epsilon = 1e-3);

        let radius =
            (placement.model * mesh_point(0.0, 0.0).extend(1.0)).truncate() - sky.moon_position;
        assert!(
            radius.length() > MOON_RADIUS_EARTH_RADII,
            "the floor did not inflate the model: radius {}",
            radius.length()
        );
    }

    /// A frame dense enough for the true disk to clear the floor keeps it: the
    /// floor is a minimum, not a size.
    #[test]
    fn a_disk_above_the_floor_is_left_alone() {
        let sky = sky();
        let camera = OrbitalCamera::new(0.0, 0.0, 8.0);
        let mut narrow = inputs(&sky, &camera, 4.0);
        narrow.sky_fov_deg = 60.0;
        let placement = place_moon(&narrow);
        let disc = placement.disc.expect("the moon is on screen");
        assert!(disc.radius > 4.0 * MIN_BODY_DISK_RADIUS_PIXELS, "{disc:?}");
        let radius =
            (placement.model * mesh_point(0.0, 0.0).extend(1.0)).truncate() - sky.moon_position;
        assert_relative_eq!(
            radius.length(),
            MOON_RADIUS_EARTH_RADII * 4.0,
            epsilon = 1e-4
        );
    }

    /// An eye inside the Moon has no silhouette to report, and nothing for the
    /// Sun's glare to hide behind.
    #[test]
    fn an_eye_inside_the_moon_reports_no_disc() {
        let sky = sky();
        let camera = OrbitalCamera::new(0.0, 0.0, 8.0);
        let mut inside = inputs(&sky, &camera, 1.0);
        inside.eye = sky.moon_position;
        assert_eq!(place_moon(&inside).disc, None);
    }
}
