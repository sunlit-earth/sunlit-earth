struct Uniforms {
    mvp: mat4x4<f32>,          // 64 bytes, offset 0
    sun_dir: vec3<f32>,        // 12 bytes, offset 64
    terminator_width: f32,     // 4 bytes, offset 76
    flags: u32,                // 4 bytes, offset 80 (bit 0: diffuse shading)
    diffuse_floor: f32,        // 4 bytes, offset 84
    diffuse_ramp: f32,         // 4 bytes, offset 88
    _pad: f32,                 // 4 bytes, offset 92
    eye_pos: vec3<f32>,        // 12 bytes, offset 96
    _pad2: f32,                // 4 bytes, offset 108
    spec_shininess: f32,       // 4 bytes, offset 112
    spec_intensity: f32,       // 4 bytes, offset 116
    _pad3: vec2<f32>,          // 8 bytes, offset 120
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

@group(0) @binding(1)
var sphere_texture: texture_2d<f32>;

@group(0) @binding(2)
var sphere_sampler: sampler;

@group(0) @binding(3)
var night_texture: texture_2d<f32>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_normal: vec3<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.mvp * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    // On a unit sphere, the vertex position IS the surface normal
    out.world_normal = in.position;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let day = textureSample(sphere_texture, sphere_sampler, in.uv);

    // If terminator_width is negative, we're in single-texture mode
    // (the night texture binding is a dummy placeholder)
    if uniforms.terminator_width < 0.0 {
        return vec4<f32>(day.rgb, 1.0);
    }

    let night_color = textureSample(night_texture, sphere_sampler, in.uv).rgb;
    let n = normalize(in.world_normal);
    let n_dot_l = dot(n, uniforms.sun_dir);

    let result = blend_fragment(
        day.rgb, night_color, n_dot_l,
        uniforms.terminator_width,
        (uniforms.flags & 1u) != 0u,
        uniforms.diffuse_floor,
        uniforms.diffuse_ramp,
    );

    var color = result.color;

    // Specular sun glint on water (Blinn-Phong).
    // Water mask encoded in upper alpha range: land=255, ocean=128.
    // Remap to [0, 1]: water = saturate((1 - alpha) * 2).
    if uniforms.spec_intensity > 0.0 {
        let water = saturate((1.0 - day.a) * 2.0);
        if water > 0.0 {
            let v = normalize(uniforms.eye_pos - in.world_normal);
            let h = normalize(uniforms.sun_dir + v);
            let n_dot_h = max(dot(n, h), 0.0);
            let spec = pow(n_dot_h, uniforms.spec_shininess);
            let glint = spec * max(n_dot_l, 0.0) * result.blend
                        * uniforms.spec_intensity * water;
            color += vec3<f32>(glint);
        }
    }

    return vec4<f32>(color, 1.0);
}
