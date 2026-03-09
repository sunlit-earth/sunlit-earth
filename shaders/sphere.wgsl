struct Uniforms {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

@group(0) @binding(1)
var sphere_texture: texture_2d<f32>;

@group(0) @binding(2)
var sphere_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
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
    // Transform normal by model matrix to get world-space normal
    out.world_normal = (uniforms.model * vec4<f32>(in.normal, 0.0)).xyz;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var color = textureSample(sphere_texture, sphere_sampler, in.uv).rgb;

    // Ambient + diffuse lighting in world space — light direction is fixed
    let light_dir = normalize(vec3<f32>(0.3, 0.5, 0.8));
    let diffuse = max(dot(normalize(in.world_normal), light_dir), 0.0);
    let ambient = 0.25;
    let light = ambient + (1.0 - ambient) * diffuse;
    color = color * light;

    return vec4<f32>(color, 1.0);
}
