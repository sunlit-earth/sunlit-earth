//! GPU integration tests for the WGSL blend/shading logic.
//!
//! Runs the actual `blend_fragment()` function from `shaders/blend.wgsl`
//! on the GPU via a compute shader, so these tests can never drift out of
//! sync with the production shader code.

mod common;

use std::sync::{LazyLock, Mutex, mpsc};

use wgpu::util::DeviceExt;

// ---------------------------------------------------------------------------
// Rust-side structs matching the WGSL storage buffer layout.
// vec3<f32> has 16-byte alignment in WGSL storage buffers.
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct TestCase {
    day: [f32; 3],
    _pad0: f32,
    night: [f32; 3],
    _pad1: f32,
    n_dot_l: f32,
    terminator_width: f32,
    diffuse_floor: f32,
    diffuse_ramp: f32,
    diffuse_enabled: u32,
    _pad2: u32,
    _pad3: u32,
    _pad4: u32,
}

const _: () = assert!(std::mem::size_of::<TestCase>() == 64);

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct TestResult {
    color: [f32; 3],
    blend: f32,
}

const _: () = assert!(std::mem::size_of::<TestResult>() == 16);

// ---------------------------------------------------------------------------
// Compute shader harness — concatenated after blend.wgsl at runtime.
// ---------------------------------------------------------------------------

const COMPUTE_HARNESS: &str = r"
struct TestCase {
    day: vec3<f32>,
    _pad0: f32,
    night: vec3<f32>,
    _pad1: f32,
    n_dot_l: f32,
    terminator_width: f32,
    diffuse_floor: f32,
    diffuse_ramp: f32,
    diffuse_enabled: u32,
    _pad2: u32,
    _pad3: u32,
    _pad4: u32,
}

struct TestResult {
    color: vec3<f32>,
    blend: f32,
}

@group(0) @binding(0) var<storage, read> inputs: array<TestCase>;
@group(0) @binding(1) var<storage, read_write> outputs: array<TestResult>;

@compute @workgroup_size(64)
fn test_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= arrayLength(&inputs) { return; }
    let tc = inputs[i];
    let result = blend_fragment(
        tc.day, tc.night, tc.n_dot_l, tc.terminator_width,
        tc.diffuse_enabled != 0u, tc.diffuse_floor, tc.diffuse_ramp,
    );
    outputs[i] = TestResult(result.color, result.blend);
}
";

// ---------------------------------------------------------------------------
// Shared GPU context — created once, reused by all tests.
// A Mutex serializes GPU submissions so parallel test threads don't crash.
// ---------------------------------------------------------------------------

struct BlendGpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
}

fn create_blend_context(force_software: bool) -> BlendGpuContext {
    let ctx = common::create_gpu_context(force_software);

    let wgsl_source = format!(
        "{}\n{}",
        include_str!("../shaders/blend.wgsl"),
        COMPUTE_HARNESS,
    );
    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("test_shader"),
            source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
        });

    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("test_pipeline"),
            layout: None,
            module: &shader,
            entry_point: Some("test_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

    BlendGpuContext {
        device: ctx.device,
        queue: ctx.queue,
        pipeline,
    }
}

static GPU: LazyLock<Mutex<BlendGpuContext>> =
    LazyLock::new(|| Mutex::new(create_blend_context(false)));

/// The second implementation `software_adapter_produces_correct_results`
/// cross-checks against.
///
/// A `LazyLock` rather than a context per call, because the project's
/// one-device rule is really a rule against creating devices per test: this is
/// the one place in the suite that holds a second device at all, and it exists
/// so a run on a discrete GPU still says something about what CI sees on a
/// software rasterizer. `None` where the platform has no software adapter,
/// which is macOS.
static SOFTWARE_GPU: LazyLock<Option<Mutex<BlendGpuContext>>> = LazyLock::new(|| {
    common::software_adapter_available().then(|| Mutex::new(create_blend_context(true)))
});

// ---------------------------------------------------------------------------
// GPU dispatch helper
// ---------------------------------------------------------------------------

fn run_on_gpu(cases: &[TestCase]) -> Vec<TestResult> {
    let gpu = GPU.lock().unwrap();
    dispatch(&gpu, cases)
}

fn dispatch(gpu: &BlendGpuContext, cases: &[TestCase]) -> Vec<TestResult> {
    assert!(!cases.is_empty(), "need at least one test case");

    let device = &gpu.device;
    let queue = &gpu.queue;
    let pipeline = &gpu.pipeline;

    let n = cases.len();
    let input_bytes = bytemuck::cast_slice(cases);
    let output_size = (n * std::mem::size_of::<TestResult>()) as u64;

    let input_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("input"),
        contents: input_bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });

    let output_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("output"),
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let staging_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging"),
        size: output_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output_buf.as_entire_binding(),
            },
        ],
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        #[allow(clippy::cast_possible_truncation)]
        let workgroups = (n as u32).div_ceil(64);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output_buf, 0, &staging_buf, 0, output_size);
    queue.submit(std::iter::once(encoder.finish()));

    let slice = staging_buf.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().expect("buffer mapping failed");

    let mapped = slice.get_mapped_range();
    let results: Vec<TestResult> = bytemuck::cast_slice(&mapped).to_vec();
    drop(mapped);
    staging_buf.unmap();

    results
}

// ---------------------------------------------------------------------------
// Test-case builders
// ---------------------------------------------------------------------------

fn make_case(
    day: [f32; 3],
    night: [f32; 3],
    n_dot_l: f32,
    terminator_width: f32,
    diffuse_enabled: bool,
    diffuse_floor: f32,
    diffuse_ramp: f32,
) -> TestCase {
    TestCase {
        day,
        _pad0: 0.0,
        night,
        _pad1: 0.0,
        n_dot_l,
        terminator_width,
        diffuse_floor,
        diffuse_ramp,
        diffuse_enabled: u32::from(diffuse_enabled),
        _pad2: 0,
        _pad3: 0,
        _pad4: 0,
    }
}

/// Build a sweep of `n_dot_l` from -1 to +1 in `steps` increments.
fn sweep(
    day: [f32; 3],
    night: [f32; 3],
    steps: usize,
    terminator_width: f32,
    diffuse_enabled: bool,
    diffuse_floor: f32,
    diffuse_ramp: f32,
) -> Vec<TestCase> {
    (0..=steps)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let n_dot_l = -1.0 + 2.0 * (i as f32 / steps as f32);
            make_case(
                day,
                night,
                n_dot_l,
                terminator_width,
                diffuse_enabled,
                diffuse_floor,
                diffuse_ramp,
            )
        })
        .collect()
}

fn luminance(c: &[f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

// ---------------------------------------------------------------------------
// Fixture parameters
// ---------------------------------------------------------------------------

const OCEAN_DAY: [f32; 3] = [0.05, 0.08, 0.30];
const OCEAN_NIGHT: [f32; 3] = [0.01, 0.01, 0.04];
const LAND_DAY: [f32; 3] = [0.30, 0.25, 0.15];
const LAND_NIGHT: [f32; 3] = [0.03, 0.02, 0.01];
const NYC_DAY: [f32; 3] = [0.28, 0.24, 0.20];
const NYC_NIGHT: [f32; 3] = [0.55, 0.50, 0.30];

const W: f32 = 0.15;
const FLOOR: f32 = 0.1;
const RAMP: f32 = 0.6;
const STEPS: usize = 1000;

// Tolerance: GPU f32 may differ from CPU by a few ULPs
const EPS: f32 = 1e-5;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The fixtures the sweeps run over: an ocean and a landmass, both darker at
/// night than by day.
const DARKENING_PAIRS: [([f32; 3], [f32; 3], &str); 2] = [
    (OCEAN_DAY, OCEAN_NIGHT, "ocean"),
    (LAND_DAY, LAND_NIGHT, "land"),
];

#[test]
fn luminance_rises_monotonically_through_the_terminator() {
    for (day, night, name) in DARKENING_PAIRS {
        let cases = sweep(day, night, STEPS, W, true, FLOOR, RAMP);
        let results = run_on_gpu(&cases);

        let lums: Vec<f32> = results.iter().map(|r| luminance(&r.color)).collect();
        for (i, w) in lums.windows(2).enumerate() {
            assert!(
                w[1] >= w[0] - EPS,
                "{name} monotonicity violated at step {i}: {:.6} -> {:.6}",
                w[0],
                w[1]
            );
        }
    }
}

#[test]
fn fully_lit_matches_day_color() {
    let pairs = [
        (OCEAN_DAY, OCEAN_NIGHT, "ocean"),
        (NYC_DAY, NYC_NIGHT, "NYC"),
    ];
    let cases: Vec<TestCase> = pairs
        .iter()
        .map(|&(day, night, _)| make_case(day, night, 1.0, W, true, FLOOR, RAMP))
        .collect();
    let results = run_on_gpu(&cases);

    for (result, (day, _, name)) in results.iter().zip(pairs) {
        for (ch, (&got, &expected)) in result.color.iter().zip(day.iter()).enumerate() {
            assert!(
                (got - expected).abs() < EPS,
                "{name} fully-lit ch={ch}: {got:.6} vs expected {expected:.6}",
            );
        }
    }
}

#[test]
fn fully_dark_matches_night_color() {
    let cases = vec![make_case(
        OCEAN_DAY,
        OCEAN_NIGHT,
        -1.0,
        W,
        true,
        FLOOR,
        RAMP,
    )];
    let results = run_on_gpu(&cases);

    for (ch, (&got, &expected)) in results[0].color.iter().zip(OCEAN_NIGHT.iter()).enumerate() {
        assert!(
            (got - expected).abs() < EPS,
            "Fully-dark ch={ch}: {got:.6} vs expected {expected:.6}",
        );
    }
}

#[test]
fn diffuse_disabled_matches_pure_blend() {
    // With diffuse disabled, result should be simple day/night mix.
    // We compute the expected value on the CPU using the standard
    // smoothstep formula — this is NOT a mirror of the blend logic,
    // just the well-known smoothstep(t) = t²(3−2t) applied to the
    // trivial mix case.
    let cases = sweep(OCEAN_DAY, OCEAN_NIGHT, 100, W, false, FLOOR, RAMP);
    let results = run_on_gpu(&cases);

    for (i, r) in results.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let n_dot_l = -1.0 + 2.0 * (i as f32 / 100.0);
        let t = ((n_dot_l + W) / (2.0 * W)).clamp(0.0, 1.0);
        let blend = t * t * (3.0 - 2.0 * t);
        for (ch, &got) in r.color.iter().enumerate() {
            let expected = OCEAN_NIGHT[ch] + (OCEAN_DAY[ch] - OCEAN_NIGHT[ch]) * blend;
            assert!(
                (got - expected).abs() < EPS,
                "Diffuse-disabled mismatch at step {i}, ch={ch}: \
                 {got:.6} vs {expected:.6}",
            );
        }
    }
}

#[test]
fn city_lights_hold_the_lit_side_at_the_day_color() {
    // NYC is the fixture whose night texture is brighter than its day one, so
    // the clamp to min(night, day) is what the diffuse ramp runs into: every
    // sample past the terminator is the day color exactly, ramp or no ramp.
    let lit = [0.15_f32, 0.3, 0.45, 0.6, 1.0];
    let cases: Vec<TestCase> = lit
        .iter()
        .map(|&ndl| make_case(NYC_DAY, NYC_NIGHT, ndl, W, true, FLOOR, RAMP))
        .chain(
            lit.iter()
                .map(|&ndl| make_case(OCEAN_DAY, OCEAN_NIGHT, ndl, W, true, FLOOR, RAMP)),
        )
        .collect();
    let results = run_on_gpu(&cases);

    for (result, &ndl) in results.iter().zip(&lit) {
        for (ch, (&got, &expected)) in result.color.iter().zip(NYC_DAY.iter()).enumerate() {
            assert!(
                (got - expected).abs() < EPS,
                "NYC at n_dot_l={ndl} ch={ch}: {got:.6} is not the day color {expected:.6}",
            );
        }
    }

    // The ocean's night texture is the darker one, so the same ramp does reach
    // it. Without this the equality above would also hold for a shader with no
    // diffuse term at all.
    let ramped = luminance(&results[lit.len()].color);
    assert!(
        ramped < luminance(&OCEAN_DAY) - EPS,
        "the diffuse ramp should darken the ocean just past the terminator, \
         got {ramped:.6} against a day luminance of {:.6}",
        luminance(&OCEAN_DAY)
    );
}

#[test]
fn never_below_min_across_color_range() {
    // The three named fixtures are in here as rows, so their own pairs are
    // among the combinations below.
    let days: &[[f32; 3]] = &[
        [0.02, 0.03, 0.20],
        OCEAN_DAY,
        [0.10, 0.10, 0.10],
        NYC_DAY,
        LAND_DAY,
        [0.80, 0.80, 0.80],
    ];
    let nights: &[[f32; 3]] = &[
        [0.00, 0.00, 0.00],
        OCEAN_NIGHT,
        LAND_NIGHT,
        [0.05, 0.04, 0.02],
        [0.30, 0.28, 0.15],
        NYC_NIGHT,
        [0.80, 0.75, 0.60],
    ];

    let mut cases = Vec::new();
    let mut meta = Vec::new(); // (day, night) per case for error messages
    for day in days {
        for night in nights {
            let batch = sweep(*day, *night, 500, W, true, FLOOR, RAMP);
            for _ in &batch {
                meta.push((*day, *night));
            }
            cases.extend(batch);
        }
    }

    let results = run_on_gpu(&cases);

    for (i, r) in results.iter().enumerate() {
        let (day, night) = meta[i];
        for ch in 0..3 {
            let floor = night[ch].min(day[ch]);
            assert!(
                r.color[ch] >= floor - EPS,
                "day={day:?} night={night:?} case={i} ch={ch}: \
                 {:.6} < floor {:.6}",
                r.color[ch],
                floor
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Software rendering — verifies the tests will pass on CI without a GPU.
// ---------------------------------------------------------------------------

#[test]
fn software_adapter_produces_correct_results() {
    // Metal exposes no software adapter (wgpu: `no_fallback_backends: METAL`),
    // so on macOS there is no second implementation to cross-check against and
    // this case has nothing to run. That is a property of the platform, not a
    // reason to stop checking the platforms that do have one, so the absence is
    // only tolerated on macOS: on Windows (WARP) and Linux (lavapipe) a missing
    // software adapter means the environment is broken and this fails.
    assert!(
        SOFTWARE_GPU.is_some() || cfg!(target_os = "macos"),
        "no software adapter: Windows has WARP and Linux has lavapipe, so this is a \
         broken environment rather than a platform without one"
    );
    let Some(software) = SOFTWARE_GPU.as_ref() else {
        eprintln!("no software adapter on this platform, skipping");
        return;
    };
    let gpu = software.lock().unwrap();

    let mut cases = sweep(OCEAN_DAY, OCEAN_NIGHT, 500, W, true, FLOOR, RAMP);
    cases.extend(sweep(NYC_DAY, NYC_NIGHT, 500, W, true, FLOOR, RAMP));
    let results = dispatch(&gpu, &cases);

    // Ocean sweep (first 501 results): monotonic, never below night
    let ocean = &results[..501];
    let ocean_lums: Vec<f32> = ocean.iter().map(|r| luminance(&r.color)).collect();
    let night_lum = luminance(&OCEAN_NIGHT);
    for (i, w) in ocean_lums.windows(2).enumerate() {
        assert!(
            w[1] >= w[0] - EPS,
            "Software: ocean monotonicity violated at step {i}: {:.6} -> {:.6}",
            w[0],
            w[1]
        );
    }
    let min_lum = ocean_lums.iter().copied().reduce(f32::min).unwrap();
    assert!(
        min_lum >= night_lum - EPS,
        "Software: ocean dipped below night: {min_lum:.6} < {night_lum:.6}"
    );

    // NYC sweep (next 501 results): day side not dominated by city lights
    let nyc = &results[501..];
    let day_lum = luminance(&NYC_DAY);
    let fully_lit = &nyc[500];
    for (ch, (&got, &expected)) in fully_lit.color.iter().zip(NYC_DAY.iter()).enumerate() {
        assert!(
            (got - expected).abs() < EPS,
            "Software: NYC fully-lit ch={ch}: got {got:.4}, expected {expected:.4}",
        );
    }
    for &step in &[375, 400, 425, 500] {
        let lum = luminance(&nyc[step].color);
        assert!(
            lum <= day_lum + EPS,
            "Software: NYC step {step} luminance {lum:.4} exceeds day {day_lum:.4}"
        );
    }
}
