//! Slint glue around the headless `sunlit_core::renderer::Renderer`.
//!
//! Everything GPU-related lives in core now; what remains here is the bridge
//! from Slint's rendering notifier to that renderer, plus the thread-local that
//! keeps it alive between callbacks. Step 5 replaces this with the engine.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use slint::{ComponentHandle, GraphicsAPI, RenderingState};
use tracing::{debug, error, info, trace};

use sunlit_core::assets::cloud_fetcher::NotifyFn;
use sunlit_core::assets::mailbox::TextureMailbox;
use sunlit_core::renderer::{RenderOutcome, Renderer, RendererConfig, quantize_to_granularity};
use sunlit_core::scene::camera::zoom_to_distance;
use sunlit_core::scene::sun;

use crate::MainWindow;

pub use sunlit_core::assets::mailbox::{DecodedTextureMessage, TextureMailbox as Mailbox};
pub use sunlit_core::renderer::{TEXTURE_LABELS, build_aa_options, read_texture_rgba8};

const DEFAULT_WIDTH: u32 = 800;
const DEFAULT_HEIGHT: u32 = 600;

// The renderer lives across rendering callbacks, and Slint calls those from the
// UI thread with an `FnMut`, so it has to be thread-local rather than owned by
// the caller.
thread_local! {
    static RENDERER: std::cell::RefCell<Option<Renderer>> = const { std::cell::RefCell::new(None) };
}

/// Register the rendering notifier on the given Slint window.
///
/// The `textures_ready` flag is set to `true` once all required texture slots
/// for the current mode are loaded (excluding clouds). This is used by the
/// `render` subcommand to know when the scene is fully rendered.
pub fn setup_rendering_notifier(
    window: &MainWindow,
    aa_counts: Vec<u32>,
    texture_paths: Vec<Option<PathBuf>>,
    texture_mailbox: TextureMailbox,
    textures_ready: Arc<AtomicBool>,
) {
    let window_weak = window.as_weak();

    window
        .window()
        .set_rendering_notifier(move |state, graphics_api| {
            rendering_callback(
                state,
                graphics_api,
                &window_weak,
                &aa_counts,
                &texture_paths,
                &texture_mailbox,
                &textures_ready,
            );
        })
        .expect("Failed to set rendering notifier — is the wgpu backend active?");
}

/// Upload decoded textures parked in the mailbox, independent of whether the
/// window is visible.
///
/// `BeforeRendering` stops firing once the window is hidden to the tray, so a
/// repeated timer on the event loop calls this to keep uploading decoded
/// textures to the GPU. Must run on the main thread, the same thread as the
/// renderer. A redraw is requested only when something was uploaded and the
/// window is visible; it is issued after the borrow is released so that a
/// synchronous repaint cannot re-enter the thread-local.
pub fn drain_texture_updates(window_weak: &slint::Weak<MainWindow>) {
    let uploaded = RENDERER.with(|r| {
        r.borrow_mut()
            .as_mut()
            .is_some_and(Renderer::drain_texture_updates)
    });

    if uploaded
        && let Some(win) = window_weak.upgrade()
        && win.window().is_visible()
    {
        win.window().request_redraw();
    }
}

/// Overwrite the sun direction used by the next wallpaper export.
///
/// Called by the scheduler before exporting, so the export uses a fresh sun
/// direction even when `BeforeRendering` hasn't fired (that is, when the window
/// is hidden to tray).
pub fn update_sun_direction(sun_dir: glam::Vec3) {
    RENDERER.with(|r| {
        if let Some(res) = r.borrow_mut().as_mut() {
            res.set_sun_direction(sun_dir);
        }
    });
}

/// Render the current scene at the given resolution and return raw RGBA8 pixels.
pub fn export_wallpaper_image(target_width: u32, target_height: u32) -> Result<Vec<u8>, String> {
    RENDERER.with(|r| {
        let borrow = r.borrow();
        let res = borrow.as_ref().ok_or("GPU not initialized")?;
        res.export_image(target_width, target_height)
    })
}

/// Look up the MSAA sample count from the AA combobox index.
fn lookup_sample_count(win: &MainWindow, aa_counts: &[u32]) -> u32 {
    let idx = usize::try_from(win.get_aa_index()).unwrap_or(0);
    aa_counts.get(idx).copied().unwrap_or(1)
}

/// Read the viewport size from the window, quantized for the renderer.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn quantized_viewport_size(win: &MainWindow) -> (u32, u32) {
    let w = (win.get_viewport_width() * win.window().scale_factor()) as u32;
    let h = (win.get_viewport_height() * win.window().scale_factor()) as u32;
    quantize_to_granularity(w, h)
}

#[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
fn rendering_callback(
    state: RenderingState,
    graphics_api: &GraphicsAPI,
    window_weak: &slint::Weak<MainWindow>,
    aa_counts: &[u32],
    texture_paths: &[Option<PathBuf>],
    texture_mailbox: &TextureMailbox,
    textures_ready: &Arc<AtomicBool>,
) {
    match state {
        RenderingState::RenderingSetup => {
            trace!("rendering setup");
            let GraphicsAPI::WGPU28 { device, queue, .. } = graphics_api else {
                error!("expected WGPU28 graphics API, got unsupported variant");
                return;
            };

            let (sample_count, width, height) =
                window_weak
                    .upgrade()
                    .map_or((4, DEFAULT_WIDTH, DEFAULT_HEIGHT), |win| {
                        let (w, h) = quantized_viewport_size(&win);
                        (lookup_sample_count(&win, aa_counts), w, h)
                    });

            let redraw_weak = window_weak.clone();
            let notify: NotifyFn = Arc::new(move || {
                let _ = redraw_weak.upgrade_in_event_loop(|win| {
                    win.window().request_redraw();
                });
            });

            let renderer = Renderer::new(
                device.clone(),
                queue.clone(),
                RendererConfig {
                    sample_count,
                    width,
                    height,
                    texture_paths: texture_paths.to_vec(),
                    mailbox: texture_mailbox.clone(),
                    notify,
                },
            );
            RENDERER.with(|r| {
                *r.borrow_mut() = Some(renderer);
            });
        }
        RenderingState::BeforeRendering => {
            let Some(win) = window_weak.upgrade() else {
                return;
            };

            let image = RENDERER.with(|r| {
                let mut borrow = r.borrow_mut();
                let res = borrow.as_mut()?;

                // Collect completed background texture decodes. The renderer's
                // dirty flag also covers uploads done by the drain timer.
                res.drain_texture_updates();

                let params = crate::ui_callbacks::read_params_from_window(&win, aa_counts);
                let (vw, vh) = quantized_viewport_size(&win);
                res.resize(vw, vh);

                let sun_dir = sun::compute_sun_direction(&params.datetime);
                win.set_zoom_display_distance(zoom_to_distance(params.camera.zoom));
                win.set_loading_text(res.loading_text(params.texture_index).into());

                let outcome = res.render(&params, sun_dir);

                if !textures_ready.load(Ordering::Relaxed) && res.textures_ready(params.texture_index)
                {
                    textures_ready.store(true, Ordering::Relaxed);
                    debug!("textures ready");
                }

                match outcome {
                    RenderOutcome::Skipped => None,
                    RenderOutcome::Rendered { first_frame } => {
                        let image = slint::Image::try_from(res.preview_texture().clone())
                            .expect("Failed to convert wgpu texture to Slint image");
                        Some((image, first_frame))
                    }
                }
            });

            if let Some((image, first_frame)) = image {
                win.set_rendered_image(image);
                // Emit "first frame rendered" exactly once
                if first_frame {
                    info!("first frame rendered");
                    println!("SIGNAL:first_frame_rendered");
                }
            }
        }
        RenderingState::RenderingTeardown => {
            trace!("rendering teardown");
            RENDERER.with(|r| {
                *r.borrow_mut() = None;
            });
        }
        _ => {}
    }
}
