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
    fresnel_mix: f32,          // 4 bytes, offset 120
    fresnel_exp: f32,          // 4 bytes, offset 124
    day_gamma: f32,            // 4 bytes, offset 128
    day_saturation: f32,       // 4 bytes, offset 132
    night_gamma: f32,          // 4 bytes, offset 136
    night_saturation: f32,     // 4 bytes, offset 140
    cloud_sphere_radius: f32,  // 4 bytes, offset 144
    cloud_opacity: f32,        // 4 bytes, offset 148
    cloud_floor: f32,          // 4 bytes, offset 152
    cloud_gamma: f32,          // 4 bytes, offset 156
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

/// Schlick-like Fresnel reflectance for water.
/// f0 = 0.02 is the normal-incidence reflectance of water.
/// The exponent controls the extent of the effect: 5.0 is the
/// physical Schlick approximation; lower values spread the
/// effect further from the limb, higher values concentrate it.
fn schlick_fresnel(n_dot_v: f32, exponent: f32) -> f32 {
    let f0 = 0.02;
    return f0 + (1.0 - f0) * pow(1.0 - n_dot_v, exponent);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let day = textureSample(sphere_texture, sphere_sampler, in.uv);

    // Apply day color correction (gamma first, then saturation)
    var day_rgb = apply_gamma(day.rgb, uniforms.day_gamma);
    day_rgb = adjust_saturation(day_rgb, uniforms.day_saturation);

    // If terminator_width is negative, we're in single-texture mode
    // (the night texture binding is a dummy placeholder)
    if uniforms.terminator_width < 0.0 {
        return vec4<f32>(day_rgb, 1.0);
    }

    let night_color = textureSample(night_texture, sphere_sampler, in.uv).rgb;

    // Apply night color correction (gamma first, then saturation)
    var night_rgb = apply_gamma(night_color, uniforms.night_gamma);
    night_rgb = adjust_saturation(night_rgb, uniforms.night_saturation);

    let n = normalize(in.world_normal);
    let n_dot_l = dot(n, uniforms.sun_dir);

    let result = blend_fragment(
        day_rgb, night_rgb, n_dot_l,
        uniforms.terminator_width,
        (uniforms.flags & 1u) != 0u,
        uniforms.diffuse_floor,
        uniforms.diffuse_ramp,
    );

    var color = result.color;

    // Water mask encoded in upper alpha range: land=255, ocean=128.
    // Remap to [0, 1]: water = saturate((1 - alpha) * 2).
    let water = saturate((1.0 - day.a) * 2.0);
    if water > 0.0 {
        let v = normalize(uniforms.eye_pos - in.world_normal);
        let n_dot_v = max(dot(n, v), 0.0);
        let fresnel = schlick_fresnel(n_dot_v, uniforms.fresnel_exp);

        // Specular sun glint on water (Blinn-Phong with Fresnel)
        if uniforms.spec_intensity > 0.0 {
            let h = normalize(uniforms.sun_dir + v);
            let n_dot_h = max(dot(n, h), 0.0);
            let spec = pow(n_dot_h, uniforms.spec_shininess);
            // Normalize Fresnel by F0 so specular stays at its pre-Fresnel
            // intensity at normal incidence and only grows at grazing angles.
            let glint = spec * (fresnel / 0.02) * max(n_dot_l, 0.0) * result.blend
                        * uniforms.spec_intensity * water;
            color += vec3<f32>(glint);
        }

        // Fresnel-driven diffuse color shift: at grazing angles, mix ocean
        // color toward a sky reflection color, simulating reduced
        // transmission and increased sky reflection on real water.
        if uniforms.fresnel_mix > 0.0 {
            let sky_color = vec3<f32>(0.5, 0.7, 0.9);
            color = mix(color, sky_color * result.blend, fresnel * uniforms.fresnel_mix * water);
        }
    }

    return vec4<f32>(color, 1.0);
}

@vertex
fn vs_cloud(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let scaled = in.position * uniforms.cloud_sphere_radius;
    out.clip_position = uniforms.mvp * vec4<f32>(scaled, 1.0);
    out.uv = in.uv;
    // Normal is the unscaled unit-sphere direction
    out.world_normal = in.position;
    return out;
}

@fragment
fn fs_cloud(in: VertexOutput) -> @location(0) vec4<f32> {
    let raw = textureSample(sphere_texture, sphere_sampler, in.uv).r;
    let floored = saturate((raw - uniforms.cloud_floor) / max(1.0 - uniforms.cloud_floor, 0.001));
    let cloud_density = pow(floored, 1.0 / max(uniforms.cloud_gamma, 0.01));
    let n = normalize(in.world_normal);
    let n_dot_l = dot(n, uniforms.sun_dir);
    let brightness = mix(0.05, 1.0, smoothstep(-uniforms.terminator_width, uniforms.terminator_width, n_dot_l));
    return vec4<f32>(brightness, brightness, brightness, cloud_density * uniforms.cloud_opacity);
}
