struct Uniforms {
    mvp: mat4x4<f32>,          // 64 bytes, offset 0
    sun_dir: vec3<f32>,        // 12 bytes, offset 64
    terminator_width: f32,     // 4 bytes, offset 76
    flags: u32,                // 4 bytes, offset 80 (bit 0: diffuse shading)
    diffuse_floor: f32,        // 4 bytes, offset 84
    diffuse_ramp: f32,         // 4 bytes, offset 88
    _pad: f32,                 // 4 bytes, offset 92
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
    let day_color = textureSample(sphere_texture, sphere_sampler, in.uv).rgb;

    // If terminator_width is negative, we're in single-texture mode
    // (the night texture binding is a dummy placeholder)
    if uniforms.terminator_width < 0.0 {
        return vec4<f32>(day_color, 1.0);
    }

    let night_color = textureSample(night_texture, sphere_sampler, in.uv).rgb;
    let n = normalize(in.world_normal);
    let n_dot_l = dot(n, uniforms.sun_dir);
    let w = uniforms.terminator_width;
    let blend = smoothstep(-w, w, n_dot_l);

    // Diffuse shading: shade the day texture before blending so
    // the night-to-day transition stays monotonic. Shading the
    // blended result would double-darken the transition zone.
    var shaded_day = day_color;
    if (uniforms.flags & 1u) != 0u {
        let shading = mix(uniforms.diffuse_floor, 1.0,
                          smoothstep(0.0, uniforms.diffuse_ramp, n_dot_l));
        shaded_day = day_color * shading;
    }

    let color = mix(night_color, shaded_day, blend);

    return vec4<f32>(color, 1.0);
}
