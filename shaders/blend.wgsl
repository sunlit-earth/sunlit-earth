struct BlendResult {
    color: vec3<f32>,
    blend: f32,
}

fn apply_gamma(color: vec3<f32>, gamma: f32) -> vec3<f32> {
    return pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / gamma));
}

fn adjust_saturation(color: vec3<f32>, saturation: f32) -> vec3<f32> {
    let luminance = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    return mix(vec3<f32>(luminance), color, saturation);
}

/// Blend day and night textures with optional diffuse shading.
///
/// Pure function — no access to uniforms or textures. Takes pre-sampled
/// colors and shading parameters, returns the final fragment RGB and the
/// day/night blend factor (0 = full night, 1 = full day).
///
/// When diffuse shading is enabled, the day color is darkened by a
/// smoothstep-based shading factor and then clamped per-channel to
/// min(night, day) so the blend never dips below either texture.
fn blend_fragment(
    day_color: vec3<f32>,
    night_color: vec3<f32>,
    n_dot_l: f32,
    terminator_width: f32,
    diffuse_enabled: bool,
    diffuse_floor: f32,
    diffuse_ramp: f32,
) -> BlendResult {
    let w = terminator_width;
    let blend = smoothstep(-w, w, n_dot_l);

    var shaded_day = day_color;
    if diffuse_enabled {
        let shading = mix(diffuse_floor, 1.0,
                          smoothstep(0.0, diffuse_ramp, n_dot_l));
        shaded_day = max(day_color * shading, min(night_color, day_color));
    }

    return BlendResult(mix(night_color, shaded_day, blend), blend);
}
