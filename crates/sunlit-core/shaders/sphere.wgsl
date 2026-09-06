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
    cloud_night: f32,              // 4 bytes, offset 196
    cloud_terminator: f32,         // 4 bytes, offset 200
    _pad5: f32,                    // 4 bytes, offset 204
    sky_view: mat4x4<f32>,            // 64 bytes, offset 208
    world_from_eqj: mat3x3<f32>,      // 48 bytes, offset 272
    viewport_size: vec2<f32>,         // 8 bytes, offset 320
    screen_offset: vec2<f32>,         // 8 bytes, offset 328
    star_intensity: f32,              // 4 bytes, offset 336
    star_mag_limit: f32,              // 4 bytes, offset 340
    star_size: f32,                   // 4 bytes, offset 344
    star_glow_strength: f32,          // 4 bytes, offset 348
    star_glow_radius: f32,            // 4 bytes, offset 352
    star_contrast: f32,               // 4 bytes, offset 356
    sky_fov: f32,                     // 4 bytes, offset 360
    sun_glow: f32,                    // 4 bytes, offset 364
    sun_rays: f32,                    // 4 bytes, offset 368
    sun_flare: f32,                   // 4 bytes, offset 372
    sun_visible: f32,                 // 4 bytes, offset 376
    sun_size: f32,                    // 4 bytes, offset 380
    sun_view_dir: vec3<f32>,          // 12 bytes, offset 384
    sun_disk_radius: f32,             // 4 bytes, offset 396
    moon_model: mat4x4<f32>,          // 64 bytes, offset 400
    moon_brightness: f32,             // 4 bytes, offset 464
    moon_earthshine: f32,             // 4 bytes, offset 468
    milky_way_intensity: f32,         // 4 bytes, offset 472
    cloud_opacity_night: f32,         // 4 bytes, offset 476
    sun_glare_tint: vec3<f32>,        // 12 bytes, offset 480
    sun_horizon_gain: f32,            // 4 bytes, offset 492
    sun_globe_center: vec2<f32>,      // 8 bytes, offset 496
    sun_globe_radius: f32,            // 4 bytes, offset 504
    sun_zone_width: f32,              // 4 bytes, offset 508
    sun_squash: f32,                  // 4 bytes, offset 512
    sun_halo_radius: f32,             // 4 bytes, offset 516
    sun_reddening: f32,               // 4 bytes, offset 520
    atmo_sunrise_glow: f32,           // 4 bytes, offset 524
    atmo_sunrise_g: f32,              // 4 bytes, offset 528
    sun_flux: f32,                    // 4 bytes, offset 532
    _pad7: f32,                       // 4 bytes, offset 536
    _pad8: f32,                       // 4 bytes, offset 540
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

const STAR_FAINT_CORE_RADIUS_PIXELS: f32 = 0.5;
const STAR_BRIGHT_CORE_RADIUS_PIXELS: f32 = 1.0;
const STAR_CORE_EDGE_PIXELS: f32 = 0.55;
const STAR_HALO_PEAK_AT_FULL_STRENGTH: f32 = 0.2;
/// Gaussian sigmas the halo spans before `star_glow_radius` ends it. The
/// truncation is subtracted rather than clipped (see `star_halo_profile`), so
/// this is the whole halo and not the part of it the sprite quad had room for.
const STAR_HALO_SIGMAS: f32 = 2.5;
const PI: f32 = 3.141592653589793;

/// Output-density ramp: 1.0 at 1080p and below, 2.0 at 4K and above.
///
/// Everything drawn at a size in pixels reads this, so a sprite does not
/// shrink to a quarter of its physical size as output density rises.
fn output_pixel_scale() -> f32 {
    return clamp(uniforms.viewport_size.y / 1080.0, 1.0, 2.0);
}

/// The horizontal half-extent of the sky lens in projected-plane units. A
/// direction `theta` from the view axis lands at `tan(theta / 2)`, and this is
/// what puts the frame's horizontal edge at NDC 1. Mirrored by
/// `sky_lens_edge_radius` in `scene::sky_lens`.
///
/// The upper end of the clamp is not the slider's 180. A spanned canvas derives
/// a sky wider than the anchor screen's own, and the lens only goes singular at
/// 360; `display::layout::SKY_FOV_MAX` is the same number and says where it
/// comes from.
fn sky_lens_edge_radius() -> f32 {
    return tan(clamp(uniforms.sky_fov, 60.0, 330.0) * PI / 720.0);
}

struct SkyLensPoint {
    /// Where the direction lands, in NDC, pan included.
    ndc: vec2<f32>,
    /// Its angle from the view axis, which is what says whether it is in front
    /// of the lens at all.
    theta: f32,
};

/// The sky lens's forward projection: where a view-space direction lands.
///
/// Stereographic, so a direction `theta` off the view axis lands at
/// `tan(theta / 2)` along its own radial direction, and one uniform scale takes
/// that to NDC. The inverse is `sky_lens_direction`, and the image of a cone
/// about a direction is `sun_disc`; this is the point mapping the star sprites
/// and the Moon's vertices both go through.
fn sky_lens_project(view_direction: vec3<f32>) -> SkyLensPoint {
    let theta = acos(clamp(-view_direction.z, -1.0, 1.0));
    let transverse_length = length(view_direction.xy);
    var radial_direction = vec2<f32>(0.0);
    if transverse_length > 0.000001 {
        radial_direction = view_direction.xy / transverse_length;
    }
    let projected_radius = tan(min(theta, PI - 0.001) * 0.5);
    let aspect = uniforms.viewport_size.x / uniforms.viewport_size.y;
    var out: SkyLensPoint;
    out.ndc = radial_direction * projected_radius / sky_lens_edge_radius()
        * vec2<f32>(1.0, aspect)
        + uniforms.screen_offset;
    out.theta = theta;
    return out;
}

/// The view-space direction a catalog direction points along.
///
/// Two rotations: the equatorial J2000 frame into the world, then the world
/// into the eye. Everything drawn from a celestial direction goes through this
/// one composition, and `milky_way_direction` is its inverse.
fn view_from_eqj(eqj_direction: vec3<f32>) -> vec3<f32> {
    let world_direction = uniforms.world_from_eqj * eqj_direction;
    return normalize((uniforms.sky_view * vec4<f32>(world_direction, 0.0)).xyz);
}

fn star_prominence(magnitude: f32) -> f32 {
    return 1.0 - smoothstep(0.0, 4.0, magnitude);
}

fn star_core_radius_pixels(magnitude: f32, pixel_scale: f32) -> f32 {
    let radius = mix(
        STAR_FAINT_CORE_RADIUS_PIXELS,
        STAR_BRIGHT_CORE_RADIUS_PIXELS,
        star_prominence(magnitude),
    );
    return radius * max(uniforms.star_size, 0.1) * pixel_scale;
}

fn star_sprite_radius_pixels(magnitude: f32, pixel_scale: f32) -> f32 {
    let core_extent = star_core_radius_pixels(magnitude, pixel_scale)
        + STAR_CORE_EDGE_PIXELS * pixel_scale;
    let has_halo = uniforms.star_glow_strength > 0.0
        && star_prominence(magnitude) > 0.0;
    let halo_extent = select(
        0.0,
        max(uniforms.star_glow_radius, 0.5) * pixel_scale,
        has_halo,
    );
    return max(core_extent, halo_extent);
}

/// Halo falloff, zero at `star_glow_radius` and normalized to peak at one.
///
/// A bare Gaussian still carries `exp(-STAR_HALO_SIGMAS^2 / 2)` of its peak
/// where the sprite quad ends, and at the top of the brightness, strength and
/// radius sliders together that residual is well above the visible threshold:
/// the quad's own edge then draws as a straight line and bright stars sit in
/// squares. Subtracting the value at the boundary is what makes the halo reach
/// zero there instead of being cut off, and dividing by what is left keeps the
/// center at full strength.
fn star_halo_profile(radius_squared: f32, pixel_scale: f32) -> f32 {
    let halo_extent = max(uniforms.star_glow_radius, 0.5) * pixel_scale;
    let halo_sigma = halo_extent / STAR_HALO_SIGMAS;
    let boundary = exp(-0.5 * STAR_HALO_SIGMAS * STAR_HALO_SIGMAS);
    let falloff = exp(-0.5 * radius_squared / (halo_sigma * halo_sigma));
    return max(falloff - boundary, 0.0) / (1.0 - boundary);
}

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
    let point = sky_lens_project(view_from_eqj(in.direction));
    let visible = point.theta < PI - 0.001 && magnitude <= uniforms.star_mag_limit;
    let pixel_scale = output_pixel_scale();
    let sprite_radius = star_sprite_radius_pixels(magnitude, pixel_scale);
    let sprite_offset = corner * (2.0 * sprite_radius / uniforms.viewport_size);
    let clip = vec4<f32>(point.ndc + sprite_offset, 1.0, 1.0);

    var out: StarOutput;
    out.clip_position = select(vec4<f32>(2.0, 2.0, 1.0, 1.0), clip, visible);
    out.local_position = corner;
    out.color = in.color_magnitude.rgb;
    out.magnitude = magnitude;
    return out;
}

@fragment
fn fs_star(in: StarOutput) -> @location(0) vec4<f32> {
    let pixel_scale = output_pixel_scale();
    let sprite_radius = star_sprite_radius_pixels(in.magnitude, pixel_scale);
    let position_pixels = in.local_position * sprite_radius;
    let radius = length(position_pixels);
    let radius_squared = dot(position_pixels, position_pixels);
    let prominence = star_prominence(in.magnitude);
    let core_radius = star_core_radius_pixels(in.magnitude, pixel_scale);
    let core_edge = STAR_CORE_EDGE_PIXELS * pixel_scale;
    let core = 1.0 - smoothstep(core_radius, core_radius + core_edge, radius);
    let halo = STAR_HALO_PEAK_AT_FULL_STRENGTH
        * clamp(uniforms.star_glow_strength, 0.0, 3.0)
        * prominence
        * star_halo_profile(radius_squared, pixel_scale);
    let contrast_exponent = 0.1 + 0.1 * clamp(uniforms.star_contrast, -1.0, 1.0);
    let compressed_flux = pow(10.0, -contrast_exponent * in.magnitude);
    let amplitude = uniforms.star_intensity * compressed_flux * (core + halo);
    return vec4<f32>(in.color * amplitude, amplitude);
}

// ---------------------------------------------------------------------------
// The Moon
//
// A textured sphere at its true position, distance and orientation, drawn with
// the sky. Its vertices reach the screen as directions from the eye through the
// same sky lens the stars use, which is what makes parallax and apparent size
// exact at every camera distance. Draw order and culling are in
// `docs/rendering.md`.
// ---------------------------------------------------------------------------

/// Width of the terminator, in units of n dot l.
///
/// The Moon has no atmosphere to scatter light past the shadow line, so this is
/// as narrow as it can be without aliasing rather than a lit-side gradient.
const MOON_TERMINATOR_WIDTH: f32 = 0.03;

struct MoonOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_normal: vec3<f32>,
};

@vertex
fn vs_moon(in: VertexInput) -> MoonOutput {
    let world_position = (uniforms.moon_model * vec4<f32>(in.position, 1.0)).xyz;
    let view_direction = normalize(
        (uniforms.sky_view * vec4<f32>(world_position - uniforms.eye_pos, 0.0)).xyz
    );
    var out: MoonOutput;
    // No offscreen guard here, unlike `vs_star`: the whole mesh is inside one
    // cone, and `scene::moon::place_moon` measures that cone before the draw is
    // submitted at all.
    out.clip_position = vec4<f32>(sky_lens_project(view_direction).ndc, 1.0, 1.0);
    out.uv = in.uv;
    // The model matrix carries a uniform scale and a rotation, so the position
    // is the normal here too, once it is normalized.
    out.world_normal = normalize((uniforms.moon_model * vec4<f32>(in.position, 0.0)).xyz);
    return out;
}

@fragment
fn fs_moon(in: MoonOutput) -> @location(0) vec4<f32> {
    let albedo = textureSample(sphere_texture, sphere_sampler, in.uv).rgb;
    let n_dot_l = dot(normalize(in.world_normal), normalize(uniforms.sun_dir));
    let sunlit = smoothstep(-MOON_TERMINATOR_WIDTH, MOON_TERMINATOR_WIDTH, n_dot_l);
    let shade = max(sunlit, clamp(uniforms.moon_earthshine, 0.0, 1.0));
    return vec4<f32>(albedo * shade * uniforms.moon_brightness, 1.0);
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

    var day_rgb = apply_gamma(day.rgb, uniforms.day_gamma);
    day_rgb = adjust_saturation(day_rgb, uniforms.day_saturation);

    // If terminator_width is negative, we're in single-texture mode
    // (the night texture binding is a dummy placeholder)
    if uniforms.terminator_width < 0.0 {
        return vec4<f32>(day_rgb, 1.0);
    }

    let night_color = textureSample(night_texture, sphere_sampler, in.uv).rgb;

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

/// A concentric shell around the globe: the same mesh, scaled to `radius`.
///
/// The four shells (clouds, Rayleigh, and the two nightglow layers) differ only
/// in which radius uniform they pass in. The normal is the unscaled unit-sphere
/// direction, which the scale leaves untouched.
fn shell_vertex(in: VertexInput, radius: f32) -> VertexOutput {
    var out: VertexOutput;
    let scaled = in.position * radius;
    out.clip_position = uniforms.mvp * vec4<f32>(scaled, 1.0);
    out.uv = in.uv;
    out.world_normal = in.position;
    return out;
}

@vertex
fn vs_cloud(in: VertexInput) -> VertexOutput {
    return shell_vertex(in, uniforms.cloud_sphere_radius);
}

// Smallest transmittance the night opacity slider can ask for, which is what
// its top end means: one over this is the largest optical depth, and at 32 a
// cloud the source gives a coverage of 0.1 still hides what is under it.
const NIGHT_OPACITY_MIN_TRANSMITTANCE: f32 = 0.03125;

@fragment
fn fs_cloud(in: VertexOutput) -> @location(0) vec4<f32> {
    let raw = textureSample(sphere_texture, sphere_sampler, in.uv).r;
    let floored = saturate((raw - uniforms.cloud_floor) / max(1.0 - uniforms.cloud_floor, 0.001));
    let cloud_density = pow(floored, 1.0 / max(uniforms.cloud_gamma, 0.01));
    let n = normalize(in.world_normal);
    let n_dot_l = dot(n, uniforms.sun_dir);
    // A cloud top at the shell's radius keeps the direct beam until the Sun is
    // sqrt(1 - 1/r^2) below its local horizontal, which is the same tangent
    // condition fs_rayleigh calls earth_limb_ndotv. So the ramp is centered
    // there rather than on the ground's terminator.
    let shell_shift = sqrt(1.0 - 1.0 / (uniforms.cloud_sphere_radius * uniforms.cloud_sphere_radius));
    let w = uniforms.cloud_terminator;
    let sunlit = smoothstep(-shell_shift - w, -shell_shift + w, n_dot_l);
    // cloud_night is a fraction of display white, not an irradiance: see the
    // field's doc comment in uniforms.rs.
    let brightness = mix(vec3<f32>(uniforms.cloud_night), vec3<f32>(1.0), sunlit);
    // The two hemispheres carry their own opacity, because one alpha does not
    // read the same on both: the day side keeps the multiply on coverage, and
    // the night side reads its slider as an optical depth, which is what puts
    // full cover at the top of the range and leaves an edge that still fades.
    // `docs/rendering.md` has the measurements that forced the split.
    let depth = uniforms.cloud_opacity_night
        / max(1.0 - uniforms.cloud_opacity_night, NIGHT_OPACITY_MIN_TRANSMITTANCE);
    // The base is held off zero because pow(0, 0) is a NaN through WGSL's
    // exp2(y * log2(x)), and a fully dense cloud at zero night opacity is
    // exactly that call.
    let night_alpha = saturate(1.0 - pow(max(1.0 - cloud_density, 1e-6), depth));
    let day_alpha = cloud_density * uniforms.cloud_opacity;
    return vec4<f32>(brightness, mix(night_alpha, day_alpha, sunlit));
}

// ---------------------------------------------------------------------------
// The light path through the band
//
// One model serves the Sun's disk, the glare's color and the Rayleigh shell's
// forward lobe: how much of the light survives a path whose lowest point is a
// given height above the surface, and what color it is by the time it arrives.
// Mirrored by `limb_air_mass`, `limb_transmission` and `limb_disk_amplitude`
// in `scene::limb_extinction`, which is what integrates it over the visible disk.
// It takes a height or a framebuffer position and nothing about the Sun, so
// the Moon can be given the same limb without moving anything here.
// ---------------------------------------------------------------------------

const EARTH_RADIUS_KM: f32 = 6371.0;
const ATMOSPHERE_SCALE_HEIGHT_KM: f32 = 7.0;
const HORIZON_AIR_MASS: f32 = 70.0;
/// Transmission per air mass for red, green and blue: Mallama's 90 and 73
/// percent with green fitted to his own table.
const CHANNEL_TRANSMISSION: vec3<f32> = vec3<f32>(0.90, 0.836, 0.73);
/// Where the disk stops being clipped white, as a gain on the green channel.
const DISK_FADE_GAIN: f32 = 40.0;

fn limb_air_mass(height_km: f32) -> f32 {
    return HORIZON_AIR_MASS
        * exp(-max(height_km, 0.0) / ATMOSPHERE_SCALE_HEIGHT_KM)
        * uniforms.sun_reddening;
}

fn limb_transmission_km(height_km: f32) -> vec3<f32> {
    return pow(CHANNEL_TRANSMISSION, vec3<f32>(limb_air_mass(height_km)));
}

/// The same light as a hue: divided by its largest channel, which red always
/// is, because the disk and the glare are clipped bright long before the path
/// stops carrying anything.
fn limb_hue_km(height_km: f32) -> vec3<f32> {
    let transmitted = limb_transmission_km(height_km);
    return transmitted / max(max(transmitted.r, transmitted.g), max(transmitted.b, 1e-30));
}

/// How far the disk is still drawn where the path carries almost nothing: a
/// tenth of the Sun is a blinding source, so it clips white down to about nine
/// kilometers and fades out below rather than being cut at the painted limb.
fn limb_disk_amplitude(green_transmission: f32) -> f32 {
    return sqrt(saturate(green_transmission * DISK_FADE_GAIN));
}

/// How high in the band a framebuffer position sits, in kilometers.
///
/// The painted annulus between the globe's silhouette and the atmosphere
/// shell's stands for the whole band, widened to `sun_zone_width` so the
/// gradient exists on a preview where the annulus is a pixel.
fn limb_band_height(position: vec2<f32>) -> f32 {
    let band_km = (uniforms.rayleigh_radius - 1.0) * EARTH_RADIUS_KM;
    let outside = length(position - uniforms.sun_globe_center) - uniforms.sun_globe_radius;
    return outside / max(uniforms.sun_zone_width, 0.0001) * band_km;
}

fn limb_transmission(position: vec2<f32>) -> vec3<f32> {
    return limb_transmission_km(limb_band_height(position));
}

// --- Rayleigh scattering shell (closest to surface, ~1.003 radius) ---

@vertex
fn vs_rayleigh(in: VertexInput) -> VertexOutput {
    return shell_vertex(in, uniforms.rayleigh_radius);
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

    // The forward-scattering lobe around the Sun's own azimuth. Its color is the
    // same light path the Sun's disk is drawn through, taken at this fragment's
    // own ray height, so the band is red where the ray grazes the surface and
    // white where it leaves the atmosphere.
    //
    // The scattering angle is the composite's, not the scene's: between the
    // fragment's own sky-lens ray and the Sun's image, which is the frame every
    // other term about the Sun is measured in. The globe is painted through the
    // camera lens and the Sun through this one, so a lobe in the true angle
    // peaks where the true Sun crosses the true limb, which at a close camera
    // is many degrees from where the disk is drawn.
    //
    // The floor is subtracted and the remainder renormalized, the way the star
    // halo has the value at its own edge taken off: Henyey-Greenstein keeps a
    // few percent at every backscattering angle, and left in that few percent
    // warms the whole limb of every frame whose Sun is behind the camera, which
    // is not a lobe around anything.
    let ray = sky_lens_direction(in.clip_position.xy);
    let cos_scatter = dot(ray, normalize(uniforms.sun_view_dir));
    let g = uniforms.atmo_sunrise_g;
    let forward = pow((1.0 - g) * (1.0 - g) / max(1.0 + g * g - 2.0 * g * cos_scatter, 1e-4), 1.5);
    let backward = pow((1.0 - g) / (1.0 + g), 3.0);
    let lobe = max(forward - backward, 0.0) / max(1.0 - backward, 1e-4);
    let ray_height = (r * sqrt(max(1.0 - n_dot_v * n_dot_v, 0.0)) - 1.0) * EARTH_RADIUS_KM;
    // The atmosphere at the top of the band sees the Sun some ten degrees past
    // the surface terminator, so the gate reaches a little into the night side.
    let sunlit = smoothstep(-0.2, 0.0, n_dot_l);
    let sunrise = limb_hue_km(ray_height) * lobe * sunlit * uniforms.atmo_sunrise_glow;

    // In-scattering: blue light scattered toward the viewer.
    // Extinction: original light blocked by the atmosphere (controlled by haze).
    // Both share the same spatial profile (rim) since they're the same interaction.
    let scatter = (color + sunrise) * rim * uniforms.rayleigh_intensity;
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
    return shell_vertex(in, uniforms.nightglow_orange_radius);
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
    return shell_vertex(in, uniforms.nightglow_green_radius);
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

// --- The Sun ---
//
// Two draws, because the Sun is two things at once: its body is a celestial
// object drawn with the sky, and its glare forms in the observer rather than in
// the scene and is a second quad drawn last over everything. The argument for
// splitting them is in `docs/rendering.md`.
//
// Every falloff is measured in degrees from the sun's center, reconstructed
// per fragment by inverting the sky lens, so the composition is correct at any
// sky field of view instead of being tied to a pixel radius.

/// Angular radius of the Sun's disk, matching `SUN_ANGULAR_RADIUS_DEGREES` in
/// `scene::sun_occlusion`.
const SUN_ANGULAR_RADIUS_DEGREES: f32 = 0.267;
/// How far from the center the glare quad reaches. Past this the composite is
/// below one 8-bit step at any usable glare strength.
const SUN_GLARE_REACH_DEGREES: f32 = 30.0;
/// Width of the core's antialiased boundary, in pixels at 1080p.
const SUN_CORE_EDGE_PIXELS: f32 = 0.8;
/// How fast the core saturates as the glare slider leaves zero. The core is
/// the one clipped white object in the scene, so it reaches full white well
/// before the slider does, and still fades out rather than popping when the
/// Sun is switched off.
const SUN_CORE_GAIN: f32 = 4.0;

const SUN_BLOOM_INNER_DEGREES: f32 = 0.8;
const SUN_BLOOM_INNER_POWER: f32 = 2.0;
const SUN_BLOOM_OUTER_DEGREES: f32 = 7.0;
const SUN_BLOOM_OUTER_POWER: f32 = 1.8;
const SUN_BLOOM_OUTER_WEIGHT: f32 = 0.09;
const SUN_BLOOM_GAIN: f32 = 1.0;

const SUN_CORONA_LOBES: f32 = 36.0;
const SUN_CORONA_TIP_DEGREES: f32 = 4.5;
/// Narrowest a needle's tip is allowed to be. Below about two pixels the lobe
/// pattern stops being a pattern and starts being adapter-dependent noise, so
/// the count comes down instead.
const SUN_CORONA_MIN_TIP_PIXELS: f32 = 2.0;
const SUN_CORONA_GAIN: f32 = 0.22;

/// The halo's reviewed radius, and what the width below is stated against: the
/// slider moves the ring and its width together, so the ratio is the constant.
const SUN_HALO_DEGREES: f32 = 3.0;
const SUN_HALO_WIDTH_DEGREES: f32 = 1.1;
const SUN_HALO_STRENGTH: f32 = 0.07;

const SUN_SPIKE_COUNT: f32 = 6.0;
const SUN_SPIKE_REACH_DEGREES: f32 = 10.0;
const SUN_SPIKE_SHARPNESS: f32 = 48.0;
const SUN_SPIKE_GAIN: f32 = 0.5;
const SUN_GHOST_GAIN: f32 = 0.1;

const SUN_GLARE_COLOR: vec3<f32> = vec3<f32>(1.0, 0.97, 0.92);

struct SunDisc {
    /// Center of the cone's image, in NDC.
    center: vec2<f32>,
    /// Its radius, in normalized projected-plane units.
    radius: f32,
    /// Whether any part of the cone is inside the frame.
    on_screen: bool,
    /// Whether the cone reaches the antipode, where its image is the exterior
    /// of a circle and the quad has to be the whole frame.
    unbounded: bool,
}

fn ndc_to_pixels(ndc: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        (ndc.x + 1.0) * 0.5 * uniforms.viewport_size.x,
        (1.0 - ndc.y) * 0.5 * uniforms.viewport_size.y,
    );
}

/// The angle the frame's furthest corner sits at, allowing for the pan.
fn sky_corner_angle() -> f32 {
    let aspect = uniforms.viewport_size.x / uniforms.viewport_size.y;
    let corner = vec2<f32>(1.0, 1.0) + abs(uniforms.screen_offset);
    let projected = length(vec2<f32>(corner.x, corner.y / aspect)) * sky_lens_edge_radius();
    return 2.0 * atan(projected);
}

/// The image of a cone of half-angle `half_angle` about the Sun.
///
/// The sky lens is conformal, so the cone images as a disc whose extremes
/// along the radial direction are the images of `theta - half_angle` and
/// `theta + half_angle`. Keeping the first signed is what lets a cone that
/// contains the view axis straddle the origin with no special case. Mirrors
/// `sky_lens_disc` in `scene::sky_lens`.
fn sun_disc(half_angle: f32) -> SunDisc {
    let direction = normalize(uniforms.sun_view_dir);
    let theta = acos(clamp(-direction.z, -1.0, 1.0));
    let transverse = length(direction.xy);
    var radial = vec2<f32>(1.0, 0.0);
    if transverse > 0.000001 {
        radial = direction.xy / transverse;
    }
    let edge = sky_lens_edge_radius();
    let aspect = uniforms.viewport_size.x / uniforms.viewport_size.y;
    let near = tan((theta - half_angle) * 0.5) / edge;
    let far = tan(min(theta + half_angle, PI - 0.001) * 0.5) / edge;

    var out: SunDisc;
    out.center = radial * ((near + far) * 0.5) * vec2<f32>(1.0, aspect) + uniforms.screen_offset;
    out.radius = (far - near) * 0.5;
    out.on_screen = theta - half_angle <= sky_corner_angle();
    out.unbounded = theta + half_angle >= PI - 0.001;
    return out;
}

/// The Sun's center in framebuffer pixels.
fn sun_screen_position() -> vec2<f32> {
    return ndc_to_pixels(sun_disc(0.0).center);
}

/// How many pixels one degree is worth where the Sun is.
///
/// The stereographic radius is `tan(theta / 2)`, so the radial scale is
/// `(1 + r^2) / 2` times the on-axis one: a degree near the frame's edge
/// covers several times the pixels it does on the view axis, and every size
/// the composite states in pixels has to know that.
fn sun_pixels_per_degree() -> f32 {
    let direction = normalize(uniforms.sun_view_dir);
    let theta = acos(clamp(-direction.z, -1.0, 1.0));
    let r = tan(min(theta, PI - 0.001) * 0.5);
    let per_radian = uniforms.viewport_size.x * 0.25 * (1.0 + r * r) / sky_lens_edge_radius();
    return per_radian * PI / 180.0;
}

/// The view-space direction a framebuffer position looks along, by inverting
/// the sky lens analytically.
fn sky_lens_direction(position: vec2<f32>) -> vec3<f32> {
    let ndc = vec2<f32>(
        position.x / uniforms.viewport_size.x * 2.0 - 1.0,
        1.0 - position.y / uniforms.viewport_size.y * 2.0,
    );
    let aspect = uniforms.viewport_size.x / uniforms.viewport_size.y;
    let projected = (ndc - uniforms.screen_offset) / vec2<f32>(1.0, aspect);
    let length_projected = length(projected);
    var radial = vec2<f32>(1.0, 0.0);
    if length_projected > 0.000001 {
        radial = projected / length_projected;
    }
    let theta = 2.0 * atan(length_projected * sky_lens_edge_radius());
    return vec3<f32>(radial * sin(theta), -cos(theta));
}

/// Veiling glare: a bright inner lobe over a much fainter one that carries the
/// tail out past ten degrees. Inverse power rather than Gaussian, because a
/// Gaussian that is bright close in has no tail and one with a tail is not
/// bright close in.
fn sun_bloom(degrees_out: f32) -> f32 {
    let d = max(degrees_out, 0.0001);
    // The point spread function is convolved with the source, so a larger disk
    // has a flat top out to its own edge and the same falloff beyond it. Only
    // this lobe follows the source; the outer one, the needles and the ring
    // belong to the eye and stay in absolute degrees.
    let inner_degrees = SUN_BLOOM_INNER_DEGREES
        + (uniforms.sun_size - 1.0) * SUN_ANGULAR_RADIUS_DEGREES;
    let inner = 1.0 / (1.0 + pow(d / inner_degrees, SUN_BLOOM_INNER_POWER));
    let outer = SUN_BLOOM_OUTER_WEIGHT
        / (1.0 + pow(d / SUN_BLOOM_OUTER_DEGREES, SUN_BLOOM_OUTER_POWER));
    return inner + outer;
}

/// Fade the whole sun-centered composite to zero before the quad ends.
///
/// A term still carrying anything where its quad stops draws that quad's own
/// edge as a straight line, which is what `star_halo_profile` exists to
/// prevent for the star sprites. Here the quad is the image of a cone, so one
/// angular window covers every term at once.
fn sun_glare_window(degrees_out: f32) -> f32 {
    return 1.0 - smoothstep(SUN_GLARE_REACH_DEGREES * 0.65, SUN_GLARE_REACH_DEGREES, degrees_out);
}

/// Integer hash, deliberately not the usual `fract(sin(...))` one: that feeds
/// a transcendental a large argument, where adapters disagree in the low bits,
/// and the needles are exactly the sort of high-frequency detail that turns
/// such a disagreement into a golden failure. This is bit-identical anywhere.
fn sun_hash(value: u32) -> f32 {
    var h = value * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return f32((h >> 22u) ^ h) / 4294967296.0;
}

/// The ciliary corona: dozens of thin needles of varying length, low contrast,
/// radiating from the core. Diffraction in the eye rather than in a lens,
/// which is why it belongs in the default look.
fn sun_corona(degrees_out: f32, azimuth: f32, pixels_per_degree: f32, scale: f32) -> f32 {
    let tip_pixels = SUN_CORONA_TIP_DEGREES * pixels_per_degree;
    let affordable = floor(PI * tip_pixels / (SUN_CORONA_MIN_TIP_PIXELS * scale));
    let lobes = clamp(affordable, 8.0, SUN_CORONA_LOBES);
    let position = (azimuth / (2.0 * PI) + 0.5) * lobes;
    let cell = floor(position);
    let index = u32(max(cell, 0.0));
    let reach = SUN_CORONA_TIP_DEGREES * (0.4 + 0.6 * sun_hash(index));
    let strength = 0.5 + 0.5 * sun_hash(index + 9871u);
    let across = pow(0.5 - 0.5 * cos(2.0 * PI * (position - cell)), 1.2);
    let along = clamp(1.0 - degrees_out / reach, 0.0, 1.0);
    return strength * across * along * along;
}

/// The lenticular halo: a faint ring near three degrees with a blue inner and
/// a red outer edge, from the lens fibers acting as a radial grating. Low
/// enough in alpha to be a detail rather than an object.
fn sun_halo(degrees_out: f32) -> vec3<f32> {
    let radius = uniforms.sun_halo_radius;
    let width = SUN_HALO_WIDTH_DEGREES * radius / SUN_HALO_DEGREES;
    let across = (degrees_out - radius) / width;
    let ring = exp(-across * across * 3.0);
    let color = mix(
        vec3<f32>(0.45, 0.62, 1.0),
        vec3<f32>(1.0, 0.55, 0.35),
        clamp(across * 0.5 + 0.5, 0.0, 1.0),
    );
    return color * ring * SUN_HALO_STRENGTH;
}

/// Aperture diffraction spikes. Screen-fixed on purpose: they belong to an
/// imaging device, and the tilt control rolls that device, so the pattern that
/// does not turn in frame is the correct one.
fn sun_spikes(degrees_out: f32, azimuth: f32) -> f32 {
    let lobes = pow(abs(cos(azimuth * SUN_SPIKE_COUNT * 0.5)), SUN_SPIKE_SHARPNESS);
    let along = clamp(1.0 - degrees_out / SUN_SPIKE_REACH_DEGREES, 0.0, 1.0);
    return lobes * along * along;
}

/// Internal-reflection ghosts, spaced along the axis from the Sun through the
/// frame's center, which is the geometry a real lens produces and the reason
/// this whole mode is off by default: in a still wallpaper they read to some
/// eyes as smudges on the display.
fn sun_ghosts(position: vec2<f32>, sun_pixels: vec2<f32>) -> vec3<f32> {
    var placements = array<f32, 3>(0.45, 0.95, 1.35);
    var sizes = array<f32, 3>(0.06, 0.095, 0.035);
    var tints = array<vec3<f32>, 3>(
        vec3<f32>(0.45, 0.75, 1.0),
        vec3<f32>(1.0, 0.72, 0.45),
        vec3<f32>(0.6, 1.0, 0.7),
    );
    let axis = uniforms.viewport_size * 0.5 - sun_pixels;
    let span = length(uniforms.viewport_size) * 0.5;
    var total = vec3<f32>(0.0);
    for (var i = 0; i < 3; i = i + 1) {
        let center = sun_pixels + axis * placements[i];
        let reach = max(sizes[i] * span, 1.0);
        let profile = clamp(1.0 - length(position - center) / reach, 0.0, 1.0);
        total = total + tints[i] * profile * profile * SUN_GHOST_GAIN;
    }
    return total;
}

/// The four corners of a screen-aligned quad, generated from the vertex index
/// so the three draws that use one need no vertex buffer.
fn sky_quad_corner(vertex_index: u32) -> vec2<f32> {
    let corners = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0,  1.0),
    );
    return corners[vertex_index];
}

const SUN_OFF_SCREEN: vec4<f32> = vec4<f32>(2.0, 2.0, 1.0, 1.0);

@vertex
fn vs_sun_disk(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    if uniforms.sun_glow <= 0.0 {
        return SUN_OFF_SCREEN;
    }
    // The cone the size slider asks for, not the true half degree: what this
    // decides is whether any of the disk that is drawn is inside the frame.
    let disc = sun_disc(radians(SUN_ANGULAR_RADIUS_DEGREES * uniforms.sun_size));
    if !disc.on_screen {
        return SUN_OFF_SCREEN;
    }
    // Sized from the radius the CPU floored rather than from the cone, so the
    // quad and the disk the fragment shader draws are the same circle.
    let extent = uniforms.sun_disk_radius + SUN_CORE_EDGE_PIXELS * output_pixel_scale();
    let offset = sky_quad_corner(vertex_index) * (2.0 * extent / uniforms.viewport_size);
    return vec4<f32>(disc.center + offset, 1.0, 1.0);
}

@fragment
fn fs_sun_disk(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let sun_pixels = sun_screen_position();
    let offset = position.xy - sun_pixels;
    // Refraction flattens the disk toward the horizon, which is an ellipse with
    // its short axis along the radius from the globe's center. The quad keeps
    // the unsquashed extent, so what changes is the distance this measures.
    var radius = length(offset);
    let outward = sun_pixels - uniforms.sun_globe_center;
    if uniforms.sun_squash < 1.0 && length(outward) > 0.0001 {
        let axis = normalize(outward);
        let along = dot(offset, axis);
        let across = length(offset - axis * along);
        radius = length(vec2<f32>(along / max(uniforms.sun_squash, 0.0001), across));
    }
    let edge = SUN_CORE_EDGE_PIXELS * output_pixel_scale();
    let core = 1.0 - smoothstep(
        uniforms.sun_disk_radius - edge,
        uniforms.sun_disk_radius + edge,
        radius,
    );
    // The gain scales the level, not the profile: multiplying the shape and
    // then clamping would eat the antialiased edge and leave a hard square of
    // white a few pixels across.
    let transmitted = limb_transmission(position.xy);
    let hue = transmitted / max(max(transmitted.r, transmitted.g), max(transmitted.b, 1e-30));
    let amplitude = core
        * saturate(uniforms.sun_glow * SUN_CORE_GAIN)
        * limb_disk_amplitude(transmitted.g);
    return vec4<f32>(hue * amplitude, amplitude);
}

@vertex
fn vs_sun_glare(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    if uniforms.sun_glow <= 0.0 || uniforms.sun_visible <= 0.0 {
        return SUN_OFF_SCREEN;
    }
    let corner = sky_quad_corner(vertex_index);
    let disc = sun_disc(radians(SUN_GLARE_REACH_DEGREES));
    if !disc.on_screen {
        return SUN_OFF_SCREEN;
    }
    // Camera mode puts ghosts on the far side of the frame's center, so the
    // cone no longer bounds what this draw touches.
    if disc.unbounded || uniforms.sun_flare > 0.0 {
        return vec4<f32>(corner, 1.0, 1.0);
    }
    let aspect = uniforms.viewport_size.x / uniforms.viewport_size.y;
    let offset = corner * disc.radius * vec2<f32>(1.0, aspect);
    return vec4<f32>(disc.center + offset, 1.0, 1.0);
}

@fragment
fn fs_sun_glare(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let sun_direction = normalize(uniforms.sun_view_dir);
    let ray = sky_lens_direction(position.xy);
    let degrees_out = degrees(acos(clamp(dot(ray, sun_direction), -1.0, 1.0)));
    let scale = output_pixel_scale();
    let sun_pixels = sun_screen_position();
    let offset = position.xy - sun_pixels;
    // Framebuffer azimuth with y flipped back up: both the corona and the
    // spikes are the observer's, so they are measured in the frame.
    let azimuth = atan2(-offset.y, offset.x);
    let pixels_per_degree = sun_pixels_per_degree();
    let core_degrees = uniforms.sun_disk_radius / max(pixels_per_degree, 0.0001);

    let bloom = sun_bloom(degrees_out) * SUN_BLOOM_GAIN;
    let corona = sun_corona(degrees_out, azimuth, pixels_per_degree, scale)
        * smoothstep(core_degrees, core_degrees * 2.5, degrees_out)
        * uniforms.sun_rays
        * SUN_CORONA_GAIN;
    // The mean color of the light the visible disk is sending, rather than one
    // fixed orange keyed on how much of the disk is in the band.
    let tint = SUN_GLARE_COLOR * uniforms.sun_glare_tint;

    var color = tint * (bloom + corona) + sun_halo(degrees_out);
    if uniforms.sun_flare > 0.0 {
        color = color + SUN_GLARE_COLOR * sun_spikes(degrees_out, azimuth)
            * uniforms.sun_flare * SUN_SPIKE_GAIN;
    }
    color = color * sun_glare_window(degrees_out);
    if uniforms.sun_flare > 0.0 {
        color = color + sun_ghosts(position.xy, sun_pixels) * uniforms.sun_flare;
    }
    // Veiling glare scales with the illuminance the source delivers, which is
    // the visible area times what the path transmits; the square root is
    // Stevens' exponent for a point source. The gain on top is the eye's own
    // adaptation lag.
    color = color * uniforms.sun_glow * uniforms.sun_horizon_gain * sqrt(uniforms.sun_flux);

    return vec4<f32>(color + dither(position), 0.0);
}

// ---------------------------------------------------------------------------
// The Milky Way
//
// The diffuse band as a panorama of the whole celestial sphere, sampled per
// pixel through the inverse of the lens the sprites are projected with. It is
// the first draw of the pass; its source position here is after the Sun only
// because `sky_lens_direction` has to be declared before it is called.
// ---------------------------------------------------------------------------

/// Where the loader's own orientation pass leaves right ascension zero.
///
/// The panorama in `textures/` is a standard astronomical all-sky map, centered
/// on right ascension zero with right ascension increasing to the left, and
/// `assets::texture_loader::orient` mirrors every equirectangular source it
/// loads and then shifts it a quarter width. The mirror is what turns right
/// ascension the right way round for this map and the shift is what moves its
/// zero to here. `textures/PROVENANCE.md` records the measurement that the
/// source's layout is the one this inverts.
const PANORAMA_RIGHT_ASCENSION_ZERO: f32 = 0.25;

/// The panorama's UV for a unit direction in equatorial J2000 coordinates.
fn milky_way_uv(direction: vec3<f32>) -> vec2<f32> {
    let u = atan2(direction.y, direction.x) / (2.0 * PI) + PANORAMA_RIGHT_ASCENSION_ZERO;
    let v = 0.5 - asin(clamp(direction.z, -1.0, 1.0)) / PI;
    return vec2<f32>(u, v);
}

/// The equatorial J2000 direction a framebuffer position looks along.
///
/// The inverse of `sky_lens_project` after `view_from_eqj`, which is what puts
/// the panorama at the same scale and orientation as the sprites drawn on top
/// of it.
fn milky_way_direction(position: vec2<f32>) -> vec3<f32> {
    let view_direction = sky_lens_direction(position);
    let world_direction = (transpose(uniforms.sky_view) * vec4<f32>(view_direction, 0.0)).xyz;
    return transpose(uniforms.world_from_eqj) * world_direction;
}

/// One screen-space derivative of `milky_way_uv`, taken from the direction's own
/// derivative rather than from the coordinate's.
///
/// `atan2` jumps a full turn across its branch cut, so a hardware derivative of
/// `u` there is a whole texture width wide and the sampler answers that one
/// pixel column with the coarsest mip: the average of the entire panorama,
/// drawn as a line from pole to pole. The direction is continuous across the
/// cut, so carrying its derivative through the map by the chain rule is what
/// keeps the gradient continuous too. Both denominators vanish at the celestial
/// poles, where the derivative really is unbounded and the coarse mip a pole
/// then selects is the right answer rather than an artifact; the floors are
/// there so it is a large number rather than a division by zero.
fn milky_way_uv_gradient(direction: vec3<f32>, derivative: vec3<f32>) -> vec2<f32> {
    let horizontal = max(dot(direction.xy, direction.xy), 1.0e-12);
    let du = (direction.x * derivative.y - direction.y * derivative.x)
        / (2.0 * PI * horizontal);
    let dv = -derivative.z / (PI * max(sqrt(1.0 - direction.z * direction.z), 1.0e-6));
    return vec2<f32>(du, dv);
}

/// The whole frame, always: there is no region the sky lens leaves undrawn and
/// nothing here to cull. Whether the layer is drawn at all is `MilkyWay::select`.
@vertex
fn vs_milky_way(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(sky_quad_corner(vertex_index), 1.0, 1.0);
}

@fragment
fn fs_milky_way(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let direction = normalize(milky_way_direction(position.xy));
    let uv = milky_way_uv(direction);
    let sky = textureSampleGrad(
        sphere_texture,
        sphere_sampler,
        uv,
        milky_way_uv_gradient(direction, dpdx(direction)),
        milky_way_uv_gradient(direction, dpdy(direction)),
    ).rgb;
    let color = sky * uniforms.milky_way_intensity;
    return vec4<f32>(color + dither(position), 0.0);
}
