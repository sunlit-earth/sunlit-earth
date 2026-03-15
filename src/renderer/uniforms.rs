/// GPU-side uniform buffer layout, matching the WGSL `Uniforms` struct.
///
/// Total: 96 bytes (must be a multiple of 16 for std140 alignment).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Uniforms {
    pub mvp: [f32; 16],           // 64 bytes
    pub sun_dir: [f32; 3],        // 12 bytes
    pub terminator_width: f32,    // 4 bytes
    pub flags: u32,               // 4 bytes
    pub diffuse_floor: f32,       // 4 bytes
    pub diffuse_ramp: f32,        // 4 bytes
    pub _pad: f32,                // 4 bytes
}

const _: () = assert!(std::mem::size_of::<Uniforms>() == 96);
