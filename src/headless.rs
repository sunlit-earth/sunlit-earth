//! Headless renderer: produces wallpaper pixels without a Slint window.
//!
//! Creates its own wgpu device/queue, builds the full rendering pipeline,
//! renders one frame, reads back the pixels, and drops all GPU resources.
//! This enables background wallpaper export from the tray icon without
//! requiring a visible window or Slint rendering notifier.

use crate::cloud_fetcher;
use crate::config::AppConfig;
use crate::geometry::grid_texture;
use crate::renderer::gpu_setup::{
    GRID_TEX_HEIGHT, GRID_TEX_WIDTH, create_bind_group_layout, create_cloud_pipeline,
    create_dummy_texture, create_pipeline, create_pipeline_layout, create_render_textures,
    create_sampler, create_shader_module, create_sphere_buffers,
};
use crate::renderer::render_pass::{self, RenderTarget, ShadingParams};
use crate::renderer::textures::{create_bind_group, create_mipmapped_texture};
use crate::renderer::uniforms::Uniforms;
use crate::scene::camera::CameraParams;
use crate::scene::datetime;
use crate::scene::sun;
use crate::texture_loader;
use crate::wgpu_init;

/// Render the scene described by `config` at the given dimensions and return
/// raw RGBA8 pixel data.
///
/// This function is self-contained: it creates its own wgpu device, builds
/// the pipeline, loads textures synchronously, renders one frame, reads back
/// the pixels, and drops all GPU resources.
#[allow(clippy::too_many_lines)]
pub fn render_wallpaper_headless(
    config: &AppConfig,
    dimensions: (u32, u32),
    force_software: bool,
) -> Result<Vec<u8>, String> {
    let (width, height) = dimensions;
    if width == 0 || height == 0 {
        return Err(format!("Invalid dimensions: {width}x{height}"));
    }

    // 1. Create device/queue
    let ctx = wgpu_init::init_headless(force_software);
    let device = &ctx.device;
    let queue = &ctx.queue;

    // 2. Create shared GPU resources
    let (vertex_buffer, index_buffer, index_count) = create_sphere_buffers(device);
    let shader = create_shader_module(device);
    let bind_group_layout = create_bind_group_layout(device);
    let sampler = create_sampler(device);
    let dummy_texture_view = create_dummy_texture(device, queue);
    let pipeline_layout = create_pipeline_layout(device, &bind_group_layout);

    // Uniform buffer
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("headless_uniforms"),
        size: std::mem::size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // 3. Create grid texture (always available as slot 0)
    let grid_tex = create_mipmapped_texture(
        device,
        queue,
        "headless_grid_texture",
        GRID_TEX_WIDTH,
        GRID_TEX_HEIGHT,
        &grid_texture::generate(GRID_TEX_WIDTH, GRID_TEX_HEIGHT),
    );
    let grid_tex_view = grid_tex.create_view(&wgpu::TextureViewDescriptor::default());

    // 4. Determine which texture mode to use and load textures synchronously
    let texture_index = config.texture_index;
    let is_blend_mode = texture_index == 3;

    // Try to load day/night textures from disk if needed
    let textures_dir = texture_loader::resolve_textures_dir(None);
    let day_path = textures_dir
        .as_ref()
        .map(|d| d.join("world.topo.200405.jxl"))
        .filter(|p| p.exists());
    let night_path = textures_dir
        .as_ref()
        .map(|d| d.join("BlackMarble_2016.jxl"))
        .filter(|p| p.exists());

    texture_loader::register_jxl_hook();

    let mut day_tex_view: Option<wgpu::TextureView> = None;
    let mut night_tex_view: Option<wgpu::TextureView> = None;

    // Load day texture if needed (texture_index 1 or blend mode 3)
    if let Some(ref path) = day_path
        && (texture_index == 1 || is_blend_mode)
        && let Ok(img) = texture_loader::load(path)
    {
        let tex = create_mipmapped_texture(
            device, queue, "headless_day_texture",
            img.width, img.height, &img.pixels,
        );
        day_tex_view = Some(tex.create_view(&wgpu::TextureViewDescriptor::default()));
    }

    // Load night texture if needed (texture_index 2 or blend mode 3)
    if let Some(ref path) = night_path
        && (texture_index == 2 || is_blend_mode)
        && let Ok(img) = texture_loader::load(path)
    {
        let tex = create_mipmapped_texture(
            device, queue, "headless_night_texture",
            img.width, img.height, &img.pixels,
        );
        night_tex_view = Some(tex.create_view(&wgpu::TextureViewDescriptor::default()));
    }

    // 5. Load cloud texture from disk cache
    let mut cloud_tex_view: Option<wgpu::TextureView> = None;
    if let Some(cloud_path) = cloud_fetcher::cache_image_path()
        && cloud_path.exists()
        && let Ok(bytes) = std::fs::read(&cloud_path)
        && let Ok(img) = cloud_fetcher::decode_cloud_jpeg(&bytes)
    {
        let tex = create_mipmapped_texture(
            device, queue, "headless_cloud_texture",
            img.width, img.height, &img.pixels,
        );
        cloud_tex_view = Some(tex.create_view(&wgpu::TextureViewDescriptor::default()));
    }

    // 6. Create bind groups
    let (bind_group, use_blend) = resolve_bind_group(
        device,
        &bind_group_layout,
        &uniform_buffer,
        &sampler,
        &dummy_texture_view,
        &grid_tex_view,
        day_tex_view.as_ref(),
        night_tex_view.as_ref(),
        texture_index,
    );

    let cloud_bind_group = cloud_tex_view.as_ref().map(|view| {
        create_bind_group(
            device,
            &bind_group_layout,
            &uniform_buffer,
            view,
            &sampler,
            &dummy_texture_view,
            "headless_cloud_bind_group",
        )
    });

    // 7. Create pipeline and render textures
    let sample_count = 1; // Headless rendering uses no MSAA for simplicity
    let pipeline = create_pipeline(device, &pipeline_layout, &shader, sample_count);
    let cloud_pipeline = create_cloud_pipeline(device, &pipeline_layout, &shader, sample_count);

    let (render_texture, depth_texture, msaa_color_view, msaa_depth_view) =
        create_render_textures(
            device,
            width,
            height,
            sample_count,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );

    // 8. Compute sun direction
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let sun_dir = if config.use_custom_datetime {
        let doy = config.custom_day_of_year as u16;
        let year = config.custom_year;
        let (month, day) = datetime::day_of_year_to_month_day(doy.max(1), year);
        let (h, m, s) = datetime::hour_float_to_hms(config.custom_hour);
        sun::sun_direction_at(year, i32::from(month), i32::from(day), h, m, s)
    } else {
        sun::sun_direction_now()
    };

    // 9. Build shading params
    let shading = ShadingParams {
        sun_dir,
        use_blend,
        terminator_width: config.terminator_width,
        diffuse_shading: config.diffuse_shading,
        diffuse_floor: config.diffuse_floor,
        diffuse_ramp: config.diffuse_ramp,
        spec_shininess: config.spec_shininess,
        spec_intensity: config.spec_intensity,
        fresnel_mix: config.fresnel_mix,
        fresnel_exp: config.fresnel_exp,
        day_gamma: config.day_gamma,
        day_saturation: config.day_saturation,
        night_gamma: config.night_gamma,
        night_saturation: config.night_saturation,
        cloud_sphere_radius: 1.0015,
        cloud_opacity: config.cloud_opacity,
        cloud_floor: config.cloud_floor,
        cloud_gamma: config.cloud_gamma,
    };

    // 10. Write uniforms, encode, and submit
    #[allow(clippy::cast_precision_loss)]
    let aspect = width as f32 / height as f32;
    let camera = CameraParams {
        longitude: config.longitude,
        latitude: config.latitude,
        zoom: config.zoom,
        offset_x: config.offset_x,
        offset_y: config.offset_y,
        tilt_deg: config.tilt,
        yaw_deg: config.yaw,
        pitch_deg: config.pitch,
    };

    render_pass::write_uniforms(queue, &uniform_buffer, &camera, aspect, &shading);

    let resolve_view = render_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let target = RenderTarget::new(
        &resolve_view,
        msaa_color_view.as_ref(),
        &depth_texture,
        msaa_depth_view.as_ref(),
    );

    let (cloud_pipe, cloud_bg) =
        if shading.cloud_opacity > 0.0 && cloud_bind_group.is_some() {
            (Some(&cloud_pipeline), cloud_bind_group.as_ref())
        } else {
            (None, None)
        };

    render_pass::encode_and_submit(
        device,
        queue,
        &target,
        &pipeline,
        &bind_group,
        &vertex_buffer,
        &index_buffer,
        index_count,
        cloud_pipe,
        cloud_bg,
    );

    // 11. Read back pixels
    let pixels = render_pass::read_texture_rgba8(device, queue, &render_texture, width, height);

    Ok(pixels)
}

/// Determine which bind group to use based on the texture index and
/// available textures. Returns the bind group and whether blend uniforms
/// should be active.
#[allow(clippy::too_many_arguments)]
fn resolve_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buffer: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
    dummy_view: &wgpu::TextureView,
    grid_view: &wgpu::TextureView,
    day_view: Option<&wgpu::TextureView>,
    night_view: Option<&wgpu::TextureView>,
    texture_index: i32,
) -> (wgpu::BindGroup, bool) {
    match texture_index {
        // Grid texture
        0 => {
            let bg = create_bind_group(
                device, layout, uniform_buffer, grid_view, sampler, dummy_view,
                "headless_grid_bg",
            );
            (bg, false)
        }
        // Day texture
        1 => {
            let view = day_view.unwrap_or(grid_view);
            let bg = create_bind_group(
                device, layout, uniform_buffer, view, sampler, dummy_view,
                "headless_day_bg",
            );
            (bg, false)
        }
        // Night texture
        2 => {
            let view = night_view.unwrap_or(grid_view);
            let bg = create_bind_group(
                device, layout, uniform_buffer, view, sampler, dummy_view,
                "headless_night_bg",
            );
            (bg, false)
        }
        // Day/Night blend
        3 => {
            if let (Some(dv), Some(nv)) = (day_view, night_view) {
                let bg = create_bind_group(
                    device, layout, uniform_buffer, dv, sampler, nv,
                    "headless_blend_bg",
                );
                (bg, true)
            } else {
                // Fallback: use whichever is available, or grid
                let view = day_view.or(night_view).unwrap_or(grid_view);
                let bg = create_bind_group(
                    device, layout, uniform_buffer, view, sampler, dummy_view,
                    "headless_blend_fallback_bg",
                );
                (bg, false)
            }
        }
        // Unknown index: fall back to grid
        _ => {
            let bg = create_bind_group(
                device, layout, uniform_buffer, grid_view, sampler, dummy_view,
                "headless_fallback_bg",
            );
            (bg, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_render_returns_correct_pixel_count() {
        let config = AppConfig {
            texture_index: 0, // Grid texture only
            ..AppConfig::default()
        };
        let result = render_wallpaper_headless(&config, (64, 64), false);
        assert!(result.is_ok(), "headless render should succeed: {:?}", result.err());
        let pixels = result.unwrap();
        assert_eq!(pixels.len(), 64 * 64 * 4, "pixel buffer size mismatch");
    }

    #[test]
    fn headless_render_produces_nonzero_output() {
        let config = AppConfig {
            texture_index: 0,
            ..AppConfig::default()
        };
        let pixels = render_wallpaper_headless(&config, (64, 64), false)
            .expect("headless render should succeed");
        let nonzero = pixels.iter().any(|&b| b != 0);
        assert!(nonzero, "rendered pixels should not be all-zero");
    }

    #[test]
    fn headless_render_deterministic() {
        let config = AppConfig {
            texture_index: 0,
            use_custom_datetime: true,
            custom_hour: 12.0,
            custom_day_of_year: 100.0,
            custom_year: 2025,
            ..AppConfig::default()
        };
        let pixels1 = render_wallpaper_headless(&config, (64, 64), false)
            .expect("first render");
        let pixels2 = render_wallpaper_headless(&config, (64, 64), false)
            .expect("second render");
        assert_eq!(pixels1, pixels2, "identical config should produce identical output");
    }

    #[test]
    fn headless_render_rejects_zero_dimensions() {
        let config = AppConfig::default();
        let result = render_wallpaper_headless(&config, (0, 0), false);
        assert!(result.is_err(), "0x0 dimensions should be rejected");
    }

    /// Regression test: the headless renderer must complete on a thread with
    /// a 2 MB stack. This catches stack bloat that would overflow the Windows
    /// default main thread (1 MB) or spawned threads (2 MB). If this test
    /// fails with a stack overflow, some function is allocating too much data
    /// on the stack and needs to be refactored (Box large locals, split into
    /// smaller functions, etc.).
    #[test]
    fn headless_render_fits_in_2mb_stack() {
        let result = std::thread::Builder::new()
            .name("stack_budget_test".into())
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                let config = AppConfig {
                    texture_index: 0,
                    use_custom_datetime: true,
                    custom_hour: 12.0,
                    custom_day_of_year: 100.0,
                    custom_year: 2025,
                    ..AppConfig::default()
                };
                render_wallpaper_headless(&config, (64, 64), false)
            })
            .expect("failed to spawn thread")
            .join();

        match result {
            Ok(Ok(pixels)) => {
                assert_eq!(pixels.len(), 64 * 64 * 4);
            }
            Ok(Err(e)) => panic!("headless render failed: {e}"),
            Err(_) => panic!(
                "headless render overflowed a 2 MB stack — \
                 refactor to reduce stack usage"
            ),
        }
    }
}
