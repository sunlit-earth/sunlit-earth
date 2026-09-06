//! The CPU side of the shader's uniform block.
//!
//! Public so `tests/render_pipeline.rs` can read this struct's field offsets
//! back out of the shader that consumes them. A copy of it in the test would
//! only prove the copy right.

/// Declares the uniform block once, as the pairing it is: a Rust type and the
/// WGSL type `shaders/sphere.wgsl` has to declare in the same position.
///
/// The list generates the struct and `UNIFORM_FIELDS`, which the tests below
/// compare against the shader field for field. That is what stops a field from
/// being appended into the trailing padding on one side alone: the offsets can
/// all be unchanged and the sizes equal, and the names still disagree.
macro_rules! uniform_block {
    (
        $(
            $(#[$attr:meta])*
            $name:ident: $ty:ty as $wgsl:ident
        ),* $(,)?
    ) => {
        /// GPU-side uniform buffer layout, matching the WGSL `Uniforms` struct.
        ///
        /// Total: 544 bytes (must be a multiple of 16 for uniform alignment).
        #[repr(C)]
        #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
        #[expect(
            clippy::pub_underscore_fields,
            reason = "the padding fields carry the names the WGSL struct gives them"
        )]
        pub struct Uniforms {
            $(
                $(#[$attr])*
                pub $name: $ty,
            )*
        }

        /// Every field of the block in declaration order: its name, the WGSL
        /// type the shader has to give it, and the offset `#[repr(C)]` puts it
        /// at.
        #[cfg(test)]
        const UNIFORM_FIELDS: &[(&str, &str, usize)] = &[
            $((
                stringify!($name),
                stringify!($wgsl),
                std::mem::offset_of!(Uniforms, $name),
            ),)*
        ];
    };
}

uniform_block! {
    mvp: [f32; 16] as mat4x4,
    sun_dir: [f32; 3] as vec3,
    terminator_width: f32 as f32,
    /// Bit 0 is diffuse shading.
    flags: u32 as u32,
    diffuse_floor: f32 as f32,
    diffuse_ramp: f32 as f32,
    _pad: f32 as f32,
    eye_pos: [f32; 3] as vec3,
    _pad2: f32 as f32,
    spec_shininess: f32 as f32,
    spec_intensity: f32 as f32,
    fresnel_mix: f32 as f32,
    fresnel_exp: f32 as f32,
    day_gamma: f32 as f32,
    day_saturation: f32 as f32,
    night_gamma: f32 as f32,
    night_saturation: f32 as f32,
    cloud_sphere_radius: f32 as f32,
    cloud_opacity: f32 as f32,
    cloud_floor: f32 as f32,
    cloud_gamma: f32 as f32,
    rayleigh_intensity: f32 as f32,
    rayleigh_sharpness: f32 as f32,
    nightglow_intensity: f32 as f32,
    nightglow_falloff: f32 as f32,
    nightglow_balance: f32 as f32,
    rayleigh_radius: f32 as f32,
    nightglow_orange_radius: f32 as f32,
    nightglow_green_radius: f32 as f32,
    rayleigh_haze: f32 as f32,
    /// Brightness of a night-side cloud, as a fraction of display white.
    cloud_night: f32 as f32,
    /// Half-width of the cloud layer's terminator ramp. Never the globe's
    /// `terminator_width`, which carries a sentinel outside blend mode.
    cloud_terminator: f32 as f32,
    _pad5: f32 as f32,
    sky_view: [f32; 16] as mat4x4,
    world_from_eqj: [[f32; 4]; 3] as mat3x3,
    viewport_size: [f32; 2] as vec2,
    screen_offset: [f32; 2] as vec2,
    star_intensity: f32 as f32,
    star_mag_limit: f32 as f32,
    star_size: f32 as f32,
    star_glow_strength: f32 as f32,
    star_glow_radius: f32 as f32,
    star_contrast: f32 as f32,
    sky_fov: f32 as f32,
    sun_glow: f32 as f32,
    sun_rays: f32 as f32,
    sun_flare: f32 as f32,
    /// Fraction of the Sun's disk outside the globe's painted silhouette.
    sun_visible: f32 as f32,
    /// Multiplier on the Sun's radius above its true half degree. The disk is
    /// already sized with it; the shader needs it for the one term that follows
    /// the source rather than the eye, the bloom's inner lobe.
    sun_size: f32 as f32,
    /// Sun direction in view space; the shader rebuilds its screen position
    /// and every angular falloff from this one vector.
    sun_view_dir: [f32; 3] as vec3,
    /// The disk's radius in pixels, floor already applied.
    sun_disk_radius: f32 as f32,
    /// Takes a mesh vertex to the Moon's place in world space, with the pixel
    /// floor already in its scale.
    moon_model: [f32; 16] as mat4x4,
    moon_brightness: f32 as f32,
    moon_earthshine: f32 as f32,
    /// Brightness of the Milky Way panorama. Zero never reaches the shader,
    /// because the draw is skipped.
    milky_way_intensity: f32 as f32,
    /// Opacity of a night-side cloud, as an optical depth scale rather than a
    /// multiplier on coverage. See `fs_cloud`.
    cloud_opacity_night: f32 as f32,
    /// Mean transmitted color of the visible disk, normalized to its largest
    /// channel: what the light the glare is made of looks like after the path
    /// it took through the band.
    sun_glare_tint: [f32; 3] as vec3,
    /// Exposure gain, the eye's lag as the disk clears the band. One is the
    /// physical answer.
    sun_horizon_gain: f32 as f32,
    /// The painted globe's silhouette in pixels, which every horizon effect is
    /// measured outward from: the annulus the viewer can see, not the one an
    /// ephemeris has.
    sun_globe_center: [f32; 2] as vec2,
    sun_globe_radius: f32 as f32,
    /// Width of the horizon zone in pixels: the painted annulus, or the disk's
    /// diameter times `sun_horizon_depth`, whichever is wider.
    sun_zone_width: f32 as f32,
    /// Vertical magnification of the refracted disk, one where nothing bends.
    sun_squash: f32 as f32,
    sun_halo_radius: f32 as f32,
    sun_reddening: f32 as f32,
    atmo_sunrise_glow: f32 as f32,
    /// Henyey-Greenstein asymmetry for the forward lobe, derived on the CPU
    /// from the angle at which the lobe is to fall to half.
    atmo_sunrise_g: f32 as f32,
    /// Visible area times what the band transmits, which is what the glare's
    /// amplitude is a compressive function of.
    sun_flux: f32 as f32,
    _pad7: f32 as f32,
    _pad8: f32 as f32,
}

const _: () = assert!(std::mem::size_of::<Uniforms>() == 544);

#[cfg(test)]
mod tests {
    use super::{UNIFORM_FIELDS, Uniforms};

    const SPHERE_WGSL: &str = include_str!("../../shaders/sphere.wgsl");

    /// Size and alignment of a WGSL type in the uniform address space.
    fn layout_of(wgsl_type: &str) -> (usize, usize) {
        match wgsl_type {
            "f32" | "u32" => (4, 4),
            "vec2" => (8, 8),
            "vec3" => (12, 16),
            "mat3x3" => (48, 16),
            "mat4x4" => (64, 16),
            other => panic!("no uniform layout rule for {other}"),
        }
    }

    /// What a field of that type is spelled in the shader.
    fn spelling_of(wgsl_type: &str) -> String {
        match wgsl_type {
            "f32" | "u32" => wgsl_type.to_owned(),
            other => format!("{other}<f32>"),
        }
    }

    /// The shader's own `Uniforms` block: one entry per field, in declaration
    /// order, with the offset its trailing comment claims.
    fn shader_fields() -> Vec<(String, String, Option<usize>)> {
        let body = SPHERE_WGSL
            .split_once("struct Uniforms {")
            .expect("sphere.wgsl declares a Uniforms struct")
            .1
            .split_once("};")
            .expect("the Uniforms struct is closed")
            .0;
        body.lines()
            .filter_map(|line| {
                let (code, comment) = match line.split_once("//") {
                    Some((code, comment)) => (code, Some(comment)),
                    None => (line, None),
                };
                let (name, ty) = code.trim().trim_end_matches(',').split_once(':')?;
                let claimed = comment
                    .and_then(|c| c.split_once("offset "))
                    .and_then(|(_, rest)| rest.split_whitespace().next()?.parse().ok());
                Some((name.trim().to_owned(), ty.trim().to_owned(), claimed))
            })
            .collect()
    }

    /// The one check that a field appended into the trailing padding cannot
    /// pass: every offset can be unchanged and both structs the same size while
    /// one side calls a field padding and the other reads it.
    #[test]
    fn the_shader_declares_the_same_fields_in_the_same_order() {
        let shader = shader_fields();
        assert_eq!(
            shader.len(),
            UNIFORM_FIELDS.len(),
            "the shader declares {} fields and the Rust block {}",
            shader.len(),
            UNIFORM_FIELDS.len()
        );
        for (index, ((declared, declared_type, _), (name, wgsl_type, _))) in
            shader.iter().zip(UNIFORM_FIELDS).enumerate()
        {
            assert_eq!(
                (declared.as_str(), declared_type.as_str()),
                (*name, spelling_of(wgsl_type).as_str()),
                "field {index}: the shader and the Rust block disagree"
            );
        }
    }

    #[test]
    fn every_field_sits_where_both_sides_put_it() {
        let claimed: Vec<_> = shader_fields()
            .into_iter()
            .map(|(_, _, offset)| offset)
            .collect();
        let mut expected = 0usize;
        for ((name, wgsl_type, rust_offset), claimed) in UNIFORM_FIELDS.iter().zip(claimed) {
            let (size, align) = layout_of(wgsl_type);
            expected = expected.div_ceil(align) * align;
            assert_eq!(
                *rust_offset, expected,
                "{name} sits at a different offset in Rust than in the shader"
            );
            assert_eq!(
                claimed,
                Some(expected),
                "sphere.wgsl must put {name} at that offset and say so in its comment"
            );
            expected += size;
        }
        assert_eq!(
            size_of::<Uniforms>(),
            expected.div_ceil(16) * 16,
            "the block is not the size the shader's fields add up to"
        );
    }
}
