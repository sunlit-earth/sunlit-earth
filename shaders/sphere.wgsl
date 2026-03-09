struct Uniforms {
    mvp: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.mvp * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.normal = in.normal;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Convert UV to longitude/latitude in degrees
    let lon_deg = in.uv.x * 360.0;
    let lat_deg = in.uv.y * 180.0;

    // Grid lines every 15 degrees — use screen-space derivatives for
    // resolution-independent anti-aliased lines.
    let grid_spacing = 15.0;
    let line_width = 0.4; // in degrees

    // Distance to nearest grid line
    let lon_dist = abs(lon_deg % grid_spacing);
    let lon_line = min(lon_dist, grid_spacing - lon_dist);

    let lat_dist = abs(lat_deg % grid_spacing);
    let lat_line = min(lat_dist, grid_spacing - lat_dist);

    // Use fwidth for smooth anti-aliasing: measures how fast the value
    // changes across the pixel, giving us a 1-pixel blend width.
    let lon_fw = fwidth(lon_deg) * 0.5;
    let lat_fw = fwidth(lat_deg) * 0.5;

    // smoothstep produces a soft falloff instead of a hard edge
    let grid_lon = 1.0 - smoothstep(line_width - lon_fw, line_width + lon_fw, lon_line);
    let grid_lat = 1.0 - smoothstep(line_width - lat_fw, line_width + lat_fw, lat_line);
    let grid_alpha = max(grid_lon, grid_lat);

    // Thicker lines at equator and prime meridian
    let major_width = line_width * 1.5;
    let equator_dist = abs(lat_deg - 90.0);
    let prime_meridian_dist = min(lon_deg, abs(lon_deg - 360.0));

    let major_eq_fw = fwidth(lat_deg) * 0.5;
    let major_pm_fw = fwidth(lon_deg) * 0.5;
    let major_equator = 1.0 - smoothstep(major_width - major_eq_fw, major_width + major_eq_fw, equator_dist);
    let major_pm = 1.0 - smoothstep(major_width - major_pm_fw, major_width + major_pm_fw, prime_meridian_dist);
    let major_alpha = max(major_equator, major_pm);

    // Base color: ocean blue in southern hemisphere, green-ish in northern
    let latitude_factor = 1.0 - in.uv.y; // 1 at north pole, 0 at south
    let base_color = mix(
        vec3<f32>(0.15, 0.3, 0.6),  // ocean blue
        vec3<f32>(0.2, 0.5, 0.3),   // land green
        latitude_factor * 0.5 + 0.25
    );

    let grid_color = vec3<f32>(0.8, 0.8, 0.8);
    let major_color = vec3<f32>(1.0, 0.9, 0.3);

    // Blend: major lines on top, then grid lines, then base
    var color = mix(base_color, grid_color, grid_alpha);
    color = mix(color, major_color, major_alpha);

    // Simple diffuse lighting from camera direction (approximated by normal.z)
    let light = max(dot(in.normal, normalize(vec3<f32>(0.3, 0.5, 0.8))), 0.15);
    color = color * light;

    return vec4<f32>(color, 1.0);
}
