//! BC7 sampling against the RGBA8 texture it was encoded from.
//!
//! The tile cache holds the surfaces in BC7 on every adapter. One that offers
//! block compression and is not a CPU samples the blocks themselves, so its BC7
//! sampler is part of every picture of the globe it draws, the Metal golden
//! references included; a CPU adapter samples them decoded to RGBA8 at upload,
//! so on warp and lavapipe the goldens pass through BC7's quality and not
//! through the adapter's decode. These cases pin the sampling itself on the
//! adapter the golden suite runs on: the software adapter where the platform
//! has one, the Metal device on macOS. One generated fixture is encoded with
//! `dds` at the preset the transcoder uses, and one pipeline samples it and the
//! original in two framings: one texel per pixel, and a quad whose footprint
//! runs from magnification through minification under anisotropy, through the
//! renderer's own surface sampler.
//! A last case uploads one tile through the renderer's own tile upload into
//! a BC7 array and reads both of its levels back, texel for texel.

use std::sync::{LazyLock, Mutex, MutexGuard};

use sunlit_core::assets::texture_loader::downsample_2x;

mod support;

use support::{MEAN_TOLERANCE, OUTLIER_FRACTION, OUTLIER_THRESHOLD, compare};

const FIXTURE_SIZE: u32 = 256;

/// Down to 4 x 4, the smallest level that holds one whole BC7 block.
const MIP_LEVELS: u32 = FIXTURE_SIZE.ilog2() - 1;

/// The largest channel difference between the adapter's BC7 decode and the
/// encoder's own decode of the same blocks, both sampled through one pipeline.
const DECODE_TOLERANCE: u8 = 2;

const SHADER: &str = r"
struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var surface: texture_2d<f32>;
@group(0) @binding(1) var surface_sampler: sampler;

// A full-frame quad as a triangle strip, its bottom edge at w = 1 and its top
// edge at w = far, with the texture repeated `u_span` times across. It is not
// the image of a plane: each of the two triangles interpolates a projective map
// of its own, and the two agree only along the diagonal they share. Between
// them the footprint still runs from magnified at the bottom to minified and
// anisotropic at the top, which is what the case is for.
fn corner(index: u32, far: f32, u_span: f32) -> Varyings {
    let right = (index & 1u) == 1u;
    let top = (index & 2u) == 2u;
    let w = select(1.0, far, top);
    var out: Varyings;
    out.position = vec4<f32>(select(-1.0, 1.0, right) * w, select(-1.0, 1.0, top) * w, 0.0, w);
    out.uv = vec2<f32>(select(0.0, u_span, right), select(1.0, 0.0, top));
    return out;
}

@vertex
fn vs_flat(@builtin(vertex_index) index: u32) -> Varyings {
    return corner(index, 1.0, 1.0);
}

@vertex
fn vs_receding(@builtin(vertex_index) index: u32) -> Varyings {
    return corner(index, 8.0, 8.0);
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    return textureSample(surface, surface_sampler, in.uv);
}
";

#[derive(Clone, Copy, Debug)]
enum Framing {
    /// One texel per pixel at texel centers through an isotropic sampler, so the
    /// frame is the level 0 texels themselves. An anisotropic sampler would not
    /// give that everywhere: lavapipe's anisotropic filter blurs even a
    /// footprint of one texel.
    Flat,
    /// Through the renderer's surface sampler, over the quad `corner` draws,
    /// from four times magnified at the bottom to eight texels per pixel at the
    /// top, over three mip levels and up to the sampler's full anisotropy.
    Receding,
}

impl Framing {
    const ALL: [Self; 2] = [Self::Flat, Self::Receding];

    fn size(self) -> (u32, u32) {
        match self {
            Self::Flat => (FIXTURE_SIZE, FIXTURE_SIZE),
            Self::Receding => (2 * FIXTURE_SIZE, FIXTURE_SIZE),
        }
    }
}

struct Bc7Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: String,
    layout: wgpu::BindGroupLayout,
    texel_sampler: wgpu::Sampler,
    surface_sampler: wgpu::Sampler,
    flat: wgpu::RenderPipeline,
    receding: wgpu::RenderPipeline,
    original: wgpu::Texture,
    /// `None` where the adapter does not offer `TEXTURE_COMPRESSION_BC`.
    compressed: Option<Compressed>,
}

struct Compressed {
    bc7: wgpu::Texture,
    /// The encoder's own decode of the same blocks, uploaded as RGBA8.
    decoded: wgpu::Texture,
}

static GPU: LazyLock<Mutex<Bc7Gpu>> = LazyLock::new(|| Mutex::new(Bc7Gpu::new()));

fn gpu() -> MutexGuard<'static, Bc7Gpu> {
    let gpu = GPU
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    println!(
        "bc7 adapter: {}, TEXTURE_COMPRESSION_BC {}",
        gpu.adapter,
        if gpu.compressed.is_some() {
            "reported"
        } else {
            "not reported"
        }
    );
    gpu
}

/// The golden suite's adapter, with BC requested where it is offered and the
/// limits the app requests, so nothing here passes on a device the renderer
/// would not get. Answers with the adapter's name, whether it offered BC, and
/// whether it is a CPU.
fn open_device() -> (wgpu::Device, wgpu::Queue, String, bool, bool) {
    let instance = sunlit_core::wgpu_init::instance();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        force_fallback_adapter: true,
        ..Default::default()
    }))
    .or_else(|_| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    })
    .expect("no wgpu adapter available (tried software and hardware)");
    let info = adapter.get_info();
    let bc = adapter
        .features()
        .contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("bc7"),
        required_features: if bc {
            wgpu::Features::TEXTURE_COMPRESSION_BC
        } else {
            wgpu::Features::empty()
        },
        required_limits:
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .expect("failed to create wgpu device");
    (
        device,
        queue,
        format!("{} ({:?})", info.name, info.backend),
        bc,
        info.device_type == wgpu::DeviceType::Cpu,
    )
}

/// The fixture as RGBA8, and as BC7 with its CPU decode where `bc` allows.
fn fixture_textures(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bc: bool,
) -> (wgpu::Texture, Option<Compressed>) {
    let levels = mip_chain(fixture());
    let original = upload(device, queue, wgpu::TextureFormat::Rgba8Unorm, &levels);
    let compressed = bc.then(|| {
        let blocks: Vec<(u32, Vec<u8>)> = levels
            .iter()
            .map(|(size, pixels)| (*size, encode(pixels, *size)))
            .collect();
        let decoded: Vec<(u32, Vec<u8>)> = blocks
            .iter()
            .map(|(size, bytes)| (*size, decode(bytes, *size)))
            .collect();
        Compressed {
            bc7: upload(device, queue, wgpu::TextureFormat::Bc7RgbaUnorm, &blocks),
            decoded: upload(device, queue, wgpu::TextureFormat::Rgba8Unorm, &decoded),
        }
    });
    (original, compressed)
}

impl Bc7Gpu {
    fn new() -> Self {
        let (device, queue, adapter, bc, cpu_adapter) = open_device();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bc7"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        // The renderer's own surface sampler, repeating across the quad as a
        // cube never needs to, and the same without anisotropy.
        let surface = wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            ..sunlit_core::renderer::surface_sampler_descriptor(cpu_adapter)
        };
        let texel_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            anisotropy_clamp: 1,
            ..surface.clone()
        });
        let surface_sampler = device.create_sampler(&surface);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bc7"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bc7"),
            bind_group_layouts: &[&layout],
            immediate_size: 0,
        });
        let pipeline = |vertex: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(vertex),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vertex),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let flat = pipeline("vs_flat");
        let receding = pipeline("vs_receding");

        let (original, compressed) = fixture_textures(&device, &queue, bc);

        Self {
            device,
            queue,
            adapter,
            layout,
            texel_sampler,
            surface_sampler,
            flat,
            receding,
            original,
            compressed,
        }
    }

    fn render(&self, texture: &wgpu::Texture, base_mip_level: u32, framing: Framing) -> Vec<u8> {
        let (width, height) = framing.size();
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bc7 target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            base_mip_level,
            ..Default::default()
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bc7"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(match framing {
                        Framing::Flat => &self.texel_sampler,
                        Framing::Receding => &self.surface_sampler,
                    }),
                },
            ],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bc7"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(match framing {
                Framing::Flat => &self.flat,
                Framing::Receding => &self.receding,
            });
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        sunlit_core::renderer::read_texture_rgba8(&self.device, &self.queue, &target, width, height)
            .expect("read the frame back")
    }
}

/// A 256 px stand-in for a surface tile: terrain over a smooth ocean, per-texel
/// grain on the land, hard coastlines, and a scatter of saturated points, so
/// every kind of block the real imagery gives the encoder is in it.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn fixture() -> Vec<u8> {
    const SEA_LEVEL: f32 = 0.5;
    let mut pixels = Vec::with_capacity((FIXTURE_SIZE * FIXTURE_SIZE * 4) as usize);
    for y in 0..FIXTURE_SIZE {
        for x in 0..FIXTURE_SIZE {
            let height = terrain(x, y);
            let grain = hash(x, y, 7) as f32 / u32::MAX as f32;
            let rgb = if grain > 0.997 {
                [255.0, 236.0, 190.0]
            } else if height < SEA_LEVEL {
                let depth = height / SEA_LEVEL;
                lerp([8.0, 22.0, 60.0], [24.0, 64.0, 110.0], depth)
            } else {
                let rise = (height - SEA_LEVEL) / (1.0 - SEA_LEVEL);
                let land = lerp([52.0, 96.0, 38.0], [168.0, 138.0, 96.0], rise);
                land.map(|c| c + (grain - 0.5) * 36.0)
            };
            pixels.extend(rgb.map(|c| c.clamp(0.0, 255.0).round() as u8));
            pixels.push(255);
        }
    }
    pixels
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// Value noise over five octaves, 64 px cells down to 4 px, in 0 to 1.
#[allow(clippy::cast_precision_loss)]
fn terrain(x: u32, y: u32) -> f32 {
    let mut total = 0.0;
    let mut weight = 0.0;
    for octave in 0..5 {
        let cell = 64 >> octave;
        let amplitude = 1.0 / (1 << octave) as f32;
        let (cx, cy) = (x / cell, y / cell);
        let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
        let fx = smooth((x % cell) as f32 / cell as f32);
        let fy = smooth((y % cell) as f32 / cell as f32);
        let lattice = |i: u32, j: u32| hash(i, j, octave) as f32 / u32::MAX as f32;
        let top = lattice(cx, cy) + (lattice(cx + 1, cy) - lattice(cx, cy)) * fx;
        let bottom = lattice(cx, cy + 1) + (lattice(cx + 1, cy + 1) - lattice(cx, cy + 1)) * fx;
        total += amplitude * (top + (bottom - top) * fy);
        weight += amplitude;
    }
    total / weight
}

fn hash(x: u32, y: u32, seed: u32) -> u32 {
    let mut h = x
        .wrapping_mul(0x27d4_eb2d)
        .wrapping_add(y.wrapping_mul(0x1656_67b1))
        .wrapping_add(seed.wrapping_mul(0x9e37_79b9));
    h = (h ^ (h >> 15)).wrapping_mul(0x85eb_ca6b);
    h = (h ^ (h >> 13)).wrapping_mul(0xc2b2_ae35);
    h ^ (h >> 16)
}

/// The fixture and its box-filtered levels, as the renderer builds its mips.
fn mip_chain(base: Vec<u8>) -> Vec<(u32, Vec<u8>)> {
    let mut levels = vec![(FIXTURE_SIZE, base)];
    for _ in 1..MIP_LEVELS {
        let (size, pixels) = levels.last().expect("level 0 is there");
        levels.push((size / 2, downsample_2x(pixels, *size, *size)));
    }
    levels
}

fn encode(pixels: &[u8], size: u32) -> Vec<u8> {
    let image = dds::ImageView::new(
        pixels,
        dds::Size::new(size, size),
        dds::ColorFormat::RGBA_U8,
    )
    .expect("a whole RGBA8 image");
    let mut options = dds::EncodeOptions::default();
    options.quality = dds::CompressionQuality::Fast;
    let mut blocks = Vec::new();
    dds::encode(&mut blocks, image, dds::Format::BC7_UNORM, None, &options)
        .expect("dds encodes BC7");
    blocks
}

fn decode(blocks: &[u8], size: u32) -> Vec<u8> {
    let mut pixels = vec![0; (size * size * 4) as usize];
    let image = dds::ImageViewMut::new(
        &mut pixels,
        dds::Size::new(size, size),
        dds::ColorFormat::RGBA_U8,
    )
    .expect("a whole RGBA8 image");
    dds::decode(
        &mut &blocks[..],
        image,
        dds::Format::BC7_UNORM,
        &dds::DecodeOptions::default(),
    )
    .expect("dds decodes its own BC7");
    pixels
}

fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    levels: &[(u32, Vec<u8>)],
) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bc7 fixture"),
        size: wgpu::Extent3d {
            width: FIXTURE_SIZE,
            height: FIXTURE_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: MIP_LEVELS,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let (block, block_bytes) = match format {
        wgpu::TextureFormat::Bc7RgbaUnorm => (4, 16),
        _ => (1, 4),
    };
    for (level, (size, bytes)) in (0..).zip(levels) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size / block * block_bytes),
                rows_per_image: Some(size / block),
            },
            wgpu::Extent3d {
                width: *size,
                height: *size,
                depth_or_array_layers: 1,
            },
        );
    }
    texture
}

fn worst_channel(a: &[u8], b: &[u8]) -> u8 {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .flat_map(|(x, y)| (0..3).map(move |c| x[c].abs_diff(y[c])))
        .max()
        .unwrap_or(0)
}

#[test]
fn bc7_sampling_stays_within_the_golden_tolerance_of_the_original() {
    let gpu = gpu();
    let Some(compressed) = &gpu.compressed else {
        eprintln!("the adapter offers no BC formats, so the RGBA8 path is the one it draws");
        return;
    };
    for framing in Framing::ALL {
        let original = gpu.render(&gpu.original, 0, framing);
        let bc7 = gpu.render(&compressed.bc7, 0, framing);
        let (mean, outliers) = compare(&original, &bc7);
        println!(
            "bc7 against the original, {framing:?}: mean {mean:.3}, {:.3}% of pixels over \
             {OUTLIER_THRESHOLD}, worst channel {}",
            outliers * 100.0,
            worst_channel(&original, &bc7)
        );
        assert!(
            mean <= MEAN_TOLERANCE && outliers <= OUTLIER_FRACTION,
            "{framing:?}: BC7 on {} is a mean channel difference of {mean:.3} from the \
             original with {:.3}% outliers, outside the golden tolerance",
            gpu.adapter,
            outliers * 100.0
        );
    }
}

#[test]
fn the_adapter_decodes_bc7_as_the_encoder_does() {
    let gpu = gpu();
    let Some(compressed) = &gpu.compressed else {
        eprintln!("the adapter offers no BC formats, so the RGBA8 path is the one it draws");
        return;
    };
    for framing in Framing::ALL {
        let decoded = gpu.render(&compressed.decoded, 0, framing);
        let bc7 = gpu.render(&compressed.bc7, 0, framing);
        let (mean, _) = compare(&decoded, &bc7);
        let worst = worst_channel(&decoded, &bc7);
        println!(
            "bc7 against the encoder's decode, {framing:?}: mean {mean:.4}, worst channel {worst}"
        );
        assert!(
            worst <= DECODE_TOLERANCE,
            "{framing:?}: {} samples BC7 up to {worst} away from what the encoder's own \
             decoder makes of the same blocks",
            gpu.adapter
        );
    }
}

#[test]
fn the_tolerance_sees_one_mip_level_of_lost_detail() {
    let gpu = gpu();
    let full = gpu.render(&gpu.original, 0, Framing::Flat);
    let texels = &mip_chain(fixture())[0].1;
    let worst = worst_channel(texels, &full);
    assert!(
        worst <= 1,
        "the flat framing should reproduce the level 0 texels, and {} is up to {worst} \
         away from them",
        gpu.adapter
    );
    let halved = gpu.render(&gpu.original, 1, Framing::Flat);
    let (mean, outliers) = compare(&full, &halved);
    println!(
        "level 1 against level 0, Flat: mean {mean:.3}, {:.3}% of pixels over {OUTLIER_THRESHOLD}",
        outliers * 100.0
    );
    assert!(
        mean > MEAN_TOLERANCE || outliers > OUTLIER_FRACTION,
        "the fixture has too little detail for the golden tolerance to tell its level 0 \
         from its level 1 (mean {mean:.3}, {:.3}% outliers)",
        outliers * 100.0
    );
}

/// Copies one level of layer 0 of a tile array to a frame of that level's
/// size, texel for pixel, with no sampler in the way. The device this target
/// opens has no compute stage.
const TILE_PROBE: &str = r"
@group(0) @binding(0) var tiles: texture_2d_array<f32>;

@vertex
fn vs_probe(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32(index & 1u), f32(index >> 1u)) * 2.0 - 1.0;
    return vec4<f32>(corner, 0.0, 1.0);
}

@fragment
fn fs_level_0(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(tiles, vec2<u32>(at.xy), 0, 0);
}

@fragment
fn fs_level_1(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(tiles, vec2<u32>(at.xy), 0, 1);
}
";

impl Bc7Gpu {
    /// Every texel of both levels of layer 0 of `array`, as RGBA8, the finest
    /// level first.
    fn probe_tile(&self, array: &wgpu::TextureView, size: u32) -> Vec<u8> {
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("bc7 tile probe"),
                source: wgpu::ShaderSource::Wgsl(TILE_PROBE.into()),
            });
        let mut texels = Vec::new();
        for (level, entry) in [(0, "fs_level_0"), (1, "fs_level_1")] {
            let pipeline = self
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("bc7 tile probe"),
                    layout: None,
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_probe"),
                        buffers: &[],
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(entry),
                        targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleStrip,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                });
            let width = size >> level;
            let target = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("bc7 tile probe"),
                size: wgpu::Extent3d {
                    width,
                    height: width,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(array),
                }],
            });
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("bc7 tile probe"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.draw(0..4, 0..1);
            }
            self.queue.submit(std::iter::once(encoder.finish()));
            texels.extend(
                sunlit_core::renderer::read_texture_rgba8(
                    &self.device,
                    &self.queue,
                    &target,
                    width,
                    width,
                )
                .expect("read the probe back"),
            );
        }
        texels
    }
}

/// A tile uploaded through the renderer's own upload into a BC7 array, which is
/// what an adapter that samples the blocks gets, holds its blocks at both of
/// its levels: the adapter decodes each texel of the layer as the encoder's own
/// decoder does.
#[test]
fn a_tile_uploaded_to_a_bc7_array_holds_its_blocks() {
    use sunlit_core::assets::tiles::{FIXTURE, PackKind, TileKey};
    use sunlit_core::renderer::tiles::{SurfaceTiles, TileId, TileUpload};

    let gpu = gpu();
    if gpu.compressed.is_none() {
        eprintln!("the adapter offers no BC formats, so its tiles are uploaded decoded");
        return;
    }
    let size = FIXTURE.layer();
    let source = fixture();
    let crop: Vec<u8> = (0..size)
        .flat_map(|row| {
            let at = (((row + 100) * FIXTURE_SIZE + 60) * 4) as usize;
            source[at..at + (size * 4) as usize].to_vec()
        })
        .collect();
    let half = downsample_2x(&crop, size, size);
    let blocks = [encode(&crop, size), encode(&half, size / 2)];
    let id = TileId {
        pack: PackKind::Night,
        key: TileKey {
            level: 4,
            face: 2,
            row: 1,
            col: 0,
        },
    };

    let mut tiles = SurfaceTiles::new(&gpu.device, FIXTURE, wgpu::TextureFormat::Bc7RgbaUnorm, 1);
    let failed = tiles.upload(
        &gpu.device,
        &gpu.queue,
        vec![TileUpload {
            id,
            blob: blocks.concat(),
            layer: None,
        }],
    );
    assert!(failed.is_empty(), "the tile uploads: {failed:?}");
    let probed = gpu.probe_tile(tiles.array_view().expect("the array"), size);
    let expected = [decode(&blocks[0], size), decode(&blocks[1], size / 2)].concat();
    let worst = worst_channel(&probed, &expected);
    println!("a BC7 tile layer against the encoder's decode: worst channel {worst}");
    assert!(
        worst <= DECODE_TOLERANCE,
        "{} holds the tile's layer up to {worst} away from its blocks decoded",
        gpu.adapter
    );
}
