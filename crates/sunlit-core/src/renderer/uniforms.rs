/// GPU-side uniform buffer layout, matching the WGSL `Uniforms` struct.
///
/// Total: 352 bytes (must be a multiple of 16 for uniform alignment).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Uniforms {
    pub mvp: [f32; 16],                 // 64 bytes
    pub sun_dir: [f32; 3],              // 12 bytes
    pub terminator_width: f32,          // 4 bytes
    pub flags: u32,                     // 4 bytes
    pub diffuse_floor: f32,             // 4 bytes
    pub diffuse_ramp: f32,              // 4 bytes
    pub _pad: f32,                      // 4 bytes
    pub eye_pos: [f32; 3],              // 12 bytes
    pub _pad2: f32,                     // 4 bytes
    pub spec_shininess: f32,            // 4 bytes
    pub spec_intensity: f32,            // 4 bytes
    pub fresnel_mix: f32,               // 4 bytes
    pub fresnel_exp: f32,               // 4 bytes
    pub day_gamma: f32,                 // 4 bytes
    pub day_saturation: f32,            // 4 bytes
    pub night_gamma: f32,               // 4 bytes
    pub night_saturation: f32,          // 4 bytes
    pub cloud_sphere_radius: f32,       // 4 bytes
    pub cloud_opacity: f32,             // 4 bytes
    pub cloud_floor: f32,               // 4 bytes
    pub cloud_gamma: f32,               // 4 bytes
    pub rayleigh_intensity: f32,        // 4 bytes
    pub rayleigh_sharpness: f32,        // 4 bytes
    pub nightglow_intensity: f32,       // 4 bytes
    pub nightglow_falloff: f32,         // 4 bytes
    pub nightglow_balance: f32,         // 4 bytes
    pub rayleigh_radius: f32,           // 4 bytes
    pub nightglow_orange_radius: f32,   // 4 bytes
    pub nightglow_green_radius: f32,    // 4 bytes
    pub rayleigh_haze: f32,             // 4 bytes
    pub _pad3: f32,                     // 4 bytes
    pub _pad4: f32,                     // 4 bytes
    pub _pad5: f32,                     // 4 bytes
    pub sky_view_projection: [f32; 16], // 64 bytes
    pub world_from_eqj: [[f32; 4]; 3],  // 48 bytes
    pub viewport_size: [f32; 2],        // 8 bytes
    pub screen_offset: [f32; 2],        // 8 bytes
    pub star_intensity: f32,            // 4 bytes
    pub star_mag_limit: f32,            // 4 bytes
    pub _pad6: f32,                     // 4 bytes
    pub _pad7: f32,                     // 4 bytes
}

const _: () = assert!(std::mem::size_of::<Uniforms>() == 352);
