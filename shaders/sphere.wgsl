struct Uniforms {
    mvp: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

@group(0) @binding(1)
var grid_texture: texture_2d<f32>;

@group(0) @binding(2)
var grid_sampler: sampler;

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
    // Sample the grid texture using the sphere's UV coordinates
    var color = textureSample(grid_texture, grid_sampler, in.uv).rgb;

    // Simple diffuse lighting
    let light = max(dot(in.normal, normalize(vec3<f32>(0.3, 0.5, 0.8))), 0.15);
    color = color * light;

    return vec4<f32>(color, 1.0);
}
