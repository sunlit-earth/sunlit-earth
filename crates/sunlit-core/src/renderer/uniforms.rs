//! The CPU side of the shader's uniform block.
//!
//! Public so `tests/render_pipeline.rs` can read this struct's field offsets
//! back out of the shader that consumes them. A copy of it in the test would
//! only prove the copy right.

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
    pub mvp: [f32; 16],               // 64 bytes
    pub sun_dir: [f32; 3],            // 12 bytes
    pub terminator_width: f32,        // 4 bytes
    pub flags: u32,                   // 4 bytes
    pub diffuse_floor: f32,           // 4 bytes
    pub diffuse_ramp: f32,            // 4 bytes
    pub _pad: f32,                    // 4 bytes
    pub eye_pos: [f32; 3],            // 12 bytes
    pub _pad2: f32,                   // 4 bytes
    pub spec_shininess: f32,          // 4 bytes
    pub spec_intensity: f32,          // 4 bytes
    pub fresnel_mix: f32,             // 4 bytes
    pub fresnel_exp: f32,             // 4 bytes
    pub day_gamma: f32,               // 4 bytes
    pub day_saturation: f32,          // 4 bytes
    pub night_gamma: f32,             // 4 bytes
    pub night_saturation: f32,        // 4 bytes
    pub cloud_sphere_radius: f32,     // 4 bytes
    pub cloud_opacity: f32,           // 4 bytes
    pub cloud_floor: f32,             // 4 bytes
    pub cloud_gamma: f32,             // 4 bytes
    pub rayleigh_intensity: f32,      // 4 bytes
    pub rayleigh_sharpness: f32,      // 4 bytes
    pub nightglow_intensity: f32,     // 4 bytes
    pub nightglow_falloff: f32,       // 4 bytes
    pub nightglow_balance: f32,       // 4 bytes
    pub rayleigh_radius: f32,         // 4 bytes
    pub nightglow_orange_radius: f32, // 4 bytes
    pub nightglow_green_radius: f32,  // 4 bytes
    pub rayleigh_haze: f32,           // 4 bytes
    /// Brightness of a night-side cloud, as a fraction of display white.
    pub cloud_night: f32, // 4 bytes
    /// Half-width of the cloud layer's terminator ramp. Never the globe's
    /// `terminator_width`, which carries a sentinel outside blend mode.
    pub cloud_terminator: f32, // 4 bytes
    pub _pad5: f32,                   // 4 bytes
    pub sky_view: [f32; 16],          // 64 bytes
    pub world_from_eqj: [[f32; 4]; 3], // 48 bytes
    pub viewport_size: [f32; 2],      // 8 bytes
    pub screen_offset: [f32; 2],      // 8 bytes
    pub star_intensity: f32,          // 4 bytes
    pub star_mag_limit: f32,          // 4 bytes
    pub star_size: f32,               // 4 bytes
    pub star_glow_strength: f32,      // 4 bytes
    pub star_glow_radius: f32,        // 4 bytes
    pub star_contrast: f32,           // 4 bytes
    pub sky_fov: f32,                 // 4 bytes
    pub sun_glow: f32,                // 4 bytes
    pub sun_rays: f32,                // 4 bytes
    pub sun_flare: f32,               // 4 bytes
    /// Fraction of the Sun's disk outside the globe's painted silhouette.
    pub sun_visible: f32, // 4 bytes
    /// Multiplier on the Sun's radius above its true half degree. The disk is
    /// already sized with it; the shader needs it for the one term that follows
    /// the source rather than the eye, the bloom's inner lobe.
    pub sun_size: f32, // 4 bytes
    /// Sun direction in view space; the shader rebuilds its screen position
    /// and every angular falloff from this one vector.
    pub sun_view_dir: [f32; 3], // 12 bytes
    /// The disk's radius in pixels, floor already applied.
    pub sun_disk_radius: f32, // 4 bytes
    /// Takes a mesh vertex to the Moon's place in world space, with the pixel
    /// floor already in its scale.
    pub moon_model: [f32; 16], // 64 bytes
    pub moon_brightness: f32,         // 4 bytes
    pub moon_earthshine: f32,         // 4 bytes
    /// Brightness of the Milky Way panorama. Zero never reaches the shader,
    /// because the draw is skipped.
    pub milky_way_intensity: f32, // 4 bytes
    /// Opacity of a night-side cloud, as an optical depth scale rather than a
    /// multiplier on coverage. See `fs_cloud`.
    pub cloud_opacity_night: f32, // 4 bytes
    /// Mean transmitted color of the visible disk, normalized to its largest
    /// channel: what the light the glare is made of looks like after the path
    /// it took through the band.
    pub sun_glare_tint: [f32; 3], // 12 bytes
    /// Exposure gain, the eye's lag as the disk clears the band. One is the
    /// physical answer.
    pub sun_horizon_gain: f32, // 4 bytes
    /// The painted globe's silhouette in pixels, which every horizon effect
    /// is measured outward from. That is decision 2's rule applied to the
    /// band: the annulus the viewer can see, not the one an ephemeris has.
    pub sun_globe_center: [f32; 2], // 8 bytes
    pub sun_globe_radius: f32,        // 4 bytes
    /// Width of the horizon zone in pixels: the painted annulus, or the disk's
    /// diameter times `sun_horizon_depth`, whichever is wider.
    pub sun_zone_width: f32, // 4 bytes
    /// Vertical magnification of the refracted disk, one where nothing bends.
    pub sun_squash: f32, // 4 bytes
    pub sun_halo_radius: f32,         // 4 bytes
    pub sun_reddening: f32,           // 4 bytes
    pub atmo_sunrise_glow: f32,       // 4 bytes
    /// Henyey-Greenstein asymmetry for the forward lobe, derived on the CPU
    /// from the angle at which the lobe is to fall to half.
    pub atmo_sunrise_g: f32, // 4 bytes
    /// Visible area times what the band transmits, which is what the glare's
    /// amplitude is a compressive function of.
    pub sun_flux: f32, // 4 bytes
    pub _pad7: f32,                   // 4 bytes
    pub _pad8: f32,                   // 4 bytes
}

const _: () = assert!(std::mem::size_of::<Uniforms>() == 544);
