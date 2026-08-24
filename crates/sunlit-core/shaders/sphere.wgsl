struct Uniforms {
    mvp: mat4x4<f32>,              // 64 bytes, offset 0
    sun_dir: vec3<f32>,            // 12 bytes, offset 64
    terminator_width: f32,         // 4 bytes, offset 76
    flags: u32,                    // 4 bytes, offset 80 (bit 0: diffuse shading)
    diffuse_floor: f32,            // 4 bytes, offset 84
    diffuse_ramp: f32,             // 4 bytes, offset 88
    _pad: f32,                     // 4 bytes, offset 92
    eye_pos: vec3<f32>,            // 12 bytes, offset 96
    _pad2: f32,                    // 4 bytes, offset 108
    spec_shininess: f32,           // 4 bytes, offset 112
    spec_intensity: f32,           // 4 bytes, offset 116
    fresnel_mix: f32,              // 4 bytes, offset 120
    fresnel_exp: f32,              // 4 bytes, offset 124
    day_gamma: f32,                // 4 bytes, offset 128
    day_saturation: f32,           // 4 bytes, offset 132
    night_gamma: f32,              // 4 bytes, offset 136
    night_saturation: f32,         // 4 bytes, offset 140
    cloud_sphere_radius: f32,      // 4 bytes, offset 144
    cloud_opacity: f32,            // 4 bytes, offset 148
    cloud_floor: f32,              // 4 bytes, offset 152
    cloud_gamma: f32,              // 4 bytes, offset 156
    rayleigh_intensity: f32,       // 4 bytes, offset 160
    rayleigh_sharpness: f32,         // 4 bytes, offset 164
    nightglow_intensity: f32,      // 4 bytes, offset 168
    nightglow_falloff: f32,        // 4 bytes, offset 172
    nightglow_balance: f32,        // 4 bytes, offset 176
    rayleigh_radius: f32,          // 4 bytes, offset 180
    nightglow_orange_radius: f32,  // 4 bytes, offset 184
    nightglow_green_radius: f32,   // 4 bytes, offset 188
    rayleigh_haze: f32,            // 4 bytes, offset 192
    _pad3: f32,                    // 4 bytes, offset 196
    _pad4: f32,                    // 4 bytes, offset 200
    _pad5: f32,                    // 4 bytes, offset 204
    sky_view_projection: mat4x4<f32>, // 64 bytes, offset 208
    world_from_eqj: mat3x3<f32>,      // 48 bytes, offset 272
    viewport_size: vec2<f32>,         // 8 bytes, offset 320
    screen_offset: vec2<f32>,         // 8 bytes, offset 328
    star_intensity: f32,              // 4 bytes, offset 336
    star_mag_limit: f32,              // 4 bytes, offset 340
    _pad6: f32,                       // 4 bytes, offset 344
    _pad7: f32,                       // 4 bytes, offset 348
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

struct StarInput {
    @location(2) direction: vec3<f32>,
    @location(3) color_magnitude: vec4<f32>,
};

struct StarOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local_position: vec2<f32>,
    @location(1) color: vec3<f32>,
    @location(2) magnitude: f32,
};

@vertex
fn vs_star(in: StarInput, @builtin(vertex_index) vertex_index: u32) -> StarOutput {
    let corners = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0,  1.0),
    );
    let corner = corners[vertex_index];
    let magnitude = in.color_magnitude.a * 10.0 - 2.0;
    let world_direction = uniforms.world_from_eqj * in.direction;
    var clip = uniforms.sky_view_projection * vec4<f32>(world_direction, 0.0);
    let visible = clip.w > 0.0 && magnitude <= uniforms.star_mag_limit;
    let sprite_offset =
        (uniforms.screen_offset + corner * (12.0 / uniforms.viewport_size)) * clip.w;
    clip = vec4<f32>(clip.xy + sprite_offset, clip.w, clip.w);

    var out: StarOutput;
    out.clip_position = select(vec4<f32>(2.0, 2.0, 1.0, 1.0), clip, visible);
    out.local_position = corner;
    out.color = in.color_magnitude.rgb;
    out.magnitude = magnitude;
    return out;
}

@fragment
fn fs_star(in: StarOutput) -> @location(0) vec4<f32> {
    let radius_squared = dot(in.local_position, in.local_position);
    let gaussian = exp(-4.5 * radius_squared);
    let compressed_flux = pow(10.0, -0.1 * in.magnitude);
    let amplitude = uniforms.star_intensity * compressed_flux * gaussian;
    return vec4<f32>(in.color * amplitude, amplitude);
}

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

// --- Rayleigh scattering shell (closest to surface, ~1.003 radius) ---

@vertex
fn vs_rayleigh(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let scaled = in.position * uniforms.rayleigh_radius;
    out.clip_position = uniforms.mvp * vec4<f32>(scaled, 1.0);
    out.uv = in.uv;
    out.world_normal = in.position;
    return out;
}

@fragment
fn fs_rayleigh(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let v = normalize(uniforms.eye_pos - in.world_normal);
    let n_dot_v = clamp(dot(n, v), 0.0, 1.0);
    let n_dot_l = dot(n, uniforms.sun_dir);

    // Bidirectional falloff: Gaussian-like profile peaking where the view ray
    // grazes the Earth's surface (not at the shell edge). The shell is larger
    // than the physical atmosphere so fragments exist on both sides of the peak.
    // earth_limb_ndotv is the NdotV at which a ray is tangent to the unit sphere
    // from the atmosphere shell surface: cos(asin(1/R)) ≈ sqrt(1 - 1/R²).
    let r = uniforms.rayleigh_radius;
    let earth_limb_ndotv = sqrt(1.0 - 1.0 / (r * r));
    let dist_from_limb = (n_dot_v - earth_limb_ndotv) * uniforms.rayleigh_sharpness;
    let rim = exp(-dist_from_limb * dist_from_limb);

    // Day-side blue Rayleigh
    let day_t = smoothstep(-0.1, 0.2, n_dot_l);
    // Terminator orange (Gaussian peak at terminator)
    let term_t = exp(-n_dot_l * n_dot_l / 0.008);
    let day_color = vec3<f32>(0.1, 0.4, 0.9);  // deep blue, darker than ocean
    let term_color = vec3<f32>(1.0, 0.5, 0.2);  // orange spectral depletion
    // Night-side fadeout
    let night_fade = smoothstep(0.0, -0.3, n_dot_l);
    let color = mix(term_color * term_t * 0.5, day_color, day_t) * (1.0 - night_fade);

    // In-scattering: blue light scattered toward the viewer.
    // Extinction: original light blocked by the atmosphere (controlled by haze).
    // Both share the same spatial profile (rim) since they're the same interaction.
    let scatter = color * rim * uniforms.rayleigh_intensity;
    let extinction = rim * uniforms.rayleigh_intensity * uniforms.rayleigh_haze;
    return vec4<f32>(scatter, extinction);
}

// Screen-space dither to break up 8-bit color banding in faint gradients.
// Returns a value in [-0.5/255, +0.5/255] based on pixel position.
fn dither(pos: vec4<f32>) -> f32 {
    let p = pos.xy;
    return (fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453) - 0.5) / 255.0;
}

// --- Orange nightglow shell (sodium D + FeO, ~1.014 radius) ---

@vertex
fn vs_nightglow_orange(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let scaled = in.position * uniforms.nightglow_orange_radius;
    out.clip_position = uniforms.mvp * vec4<f32>(scaled, 1.0);
    out.uv = in.uv;
    out.world_normal = in.position;
    return out;
}

@fragment
fn fs_nightglow_orange(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let v = normalize(uniforms.eye_pos - in.world_normal);
    let n_dot_v = clamp(dot(n, v), 0.0, 1.0);
    let n_dot_l = dot(n, uniforms.sun_dir);

    // Rim falloff toward disk center, with soft edge at the shell silhouette
    // to avoid a hard color band where the sphere geometry ends.
    let rim = pow(1.0 - n_dot_v, uniforms.nightglow_falloff)
            * smoothstep(0.0, 0.05, n_dot_v);

    // Night-side mask: fully fades before reaching the day side
    let night_mask = smoothstep(-0.05, -0.2, n_dot_l);
    // Time-of-night: orange is STRONGER near the terminator, weaker at midnight
    let depth = clamp(-n_dot_l, 0.0, 1.0);
    let time_mod = 1.0 - 0.5 * depth;
    // Latitude modulation: enhanced near +/-23 degrees geomagnetic latitude
    let lat = asin(clamp(n.y, -1.0, 1.0));
    let lat_mod = 0.7 + 0.3 * exp(-pow((abs(lat) - 0.4) / 0.2, 2.0));

    let color = vec3<f32>(1.0, 0.7, 0.2);  // warm orange-yellow (sodium D + FeO)
    let intensity = uniforms.nightglow_intensity * (1.0 - uniforms.nightglow_balance);
    let rgb = color * rim * intensity * night_mask * time_mod * lat_mod;

    return vec4<f32>(rgb + dither(in.clip_position), 0.0);
}

// --- Green nightglow shell (OI 557.7nm, ~1.015 radius) ---

@vertex
fn vs_nightglow_green(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let scaled = in.position * uniforms.nightglow_green_radius;
    out.clip_position = uniforms.mvp * vec4<f32>(scaled, 1.0);
    out.uv = in.uv;
    out.world_normal = in.position;
    return out;
}

@fragment
fn fs_nightglow_green(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let v = normalize(uniforms.eye_pos - in.world_normal);
    let n_dot_v = clamp(dot(n, v), 0.0, 1.0);
    let n_dot_l = dot(n, uniforms.sun_dir);

    // Rim falloff toward disk center, with soft edge at the shell silhouette
    let rim = pow(1.0 - n_dot_v, uniforms.nightglow_falloff)
            * smoothstep(0.0, 0.05, n_dot_v);

    // Night-side mask: fully fades before reaching the day side
    let night_mask = smoothstep(-0.05, -0.2, n_dot_l);
    // Time-of-night: green is WEAKER near the terminator, STRONGER at midnight
    let depth = clamp(-n_dot_l, 0.0, 1.0);
    let time_mod = 0.5 + 0.5 * depth;
    // Latitude modulation: enhanced near +/-23 degrees geomagnetic latitude
    let lat = asin(clamp(n.y, -1.0, 1.0));
    let lat_mod = 0.7 + 0.3 * exp(-pow((abs(lat) - 0.4) / 0.2, 2.0));

    let color = vec3<f32>(0.2, 1.0, 0.3);  // green (OI 557.7nm)
    let intensity = uniforms.nightglow_intensity * uniforms.nightglow_balance;
    let rgb = color * rim * intensity * night_mask * time_mod * lat_mod;

    return vec4<f32>(rgb + dither(in.clip_position), 0.0);
}
