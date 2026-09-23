//! The X11 root window's background, set the way feh, Esetroot and xwallpaper
//! set it, for a session whose window manager draws no desktop of its own.
//!
//! One pixmap the size of the root, each monitor's picture drawn at its own
//! rectangle, which is every display mode at once: the view across screens is
//! the canvas placed where it spans. No PNG is written, since nothing reads one.

use crate::engine::wallpaper_sink::{Frame, JobImages, WallpaperJob};

/// How the X server wants a pixel laid out in an image it is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PixelFormat {
    pub(super) bits_per_pixel: u8,
    /// The multiple of bits every row is padded to.
    pub(super) scanline_pad: u8,
    pub(super) red_mask: u32,
    pub(super) green_mask: u32,
    pub(super) blue_mask: u32,
    /// Whether the least significant byte of a pixel comes first.
    pub(super) lsb_first: bool,
}

impl PixelFormat {
    fn bytes_per_pixel(self) -> usize {
        usize::from(self.bits_per_pixel.div_ceil(8))
    }

    fn stride(self, width: u32) -> usize {
        let bits = width as usize * usize::from(self.bits_per_pixel);
        let pad = usize::from(self.scanline_pad.max(8));
        bits.div_ceil(pad) * pad / 8
    }

    /// One pixel's value, from 8-bit channels scaled to each mask's width.
    fn encode(self, [red, green, blue]: [u8; 3]) -> u32 {
        channel(red, self.red_mask)
            | channel(green, self.green_mask)
            | channel(blue, self.blue_mask)
    }

    fn write(self, out: &mut [u8], pixel: u32) {
        let n = out.len();
        if self.lsb_first {
            out.copy_from_slice(&pixel.to_le_bytes()[..n]);
        } else {
            out.copy_from_slice(&pixel.to_be_bytes()[4 - n..]);
        }
    }
}

fn channel(value: u8, mask: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    let bits = mask.count_ones();
    let max = (1u64 << bits) - 1;
    let scaled = (u64::from(value) * max + 127) / 255;
    u32::try_from(scaled).unwrap_or(u32::MAX) << mask.trailing_zeros()
}

/// One picture and where its top left corner goes on the root.
pub(super) struct Piece<'a> {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) frame: &'a Frame,
}

/// What one publish puts on the root: the canvas where it spans, and every
/// monitor's own picture where it does not. A monitor with none is left out,
/// and shows the background the canvas starts with.
pub(super) fn pieces(job: &WallpaperJob) -> Vec<Piece<'_>> {
    match &job.images {
        JobImages::Spanned { canvas, bounds } => vec![Piece {
            x: bounds.x,
            y: bounds.y,
            frame: canvas,
        }],
        JobImages::PerMonitor(images) => job
            .monitors
            .iter()
            .zip(images)
            .filter_map(|(monitor, image)| {
                image.as_deref().map(|frame| Piece {
                    x: monitor.x,
                    y: monitor.y,
                    frame,
                })
            })
            .collect(),
    }
}

/// The root's whole image in the server's own pixel layout, black wherever no
/// piece reaches, and each piece clipped to the root.
pub(super) fn compose(
    width: u32,
    height: u32,
    pieces: &[Piece<'_>],
    format: PixelFormat,
) -> Vec<u8> {
    let stride = format.stride(width);
    let bpp = format.bytes_per_pixel();
    let mut out = vec![0u8; stride * height as usize];
    let black = format.encode([0, 0, 0]);
    if black != 0 {
        for row in out.chunks_exact_mut(stride) {
            for pixel in row[..width as usize * bpp].chunks_exact_mut(bpp) {
                format.write(pixel, black);
            }
        }
    }
    for piece in pieces {
        let frame = piece.frame;
        if !frame.is_well_formed() {
            continue;
        }
        for src_y in 0..frame.height {
            let Ok(dst_y) = u32::try_from(i64::from(piece.y) + i64::from(src_y)) else {
                continue;
            };
            if dst_y >= height {
                break;
            }
            for src_x in 0..frame.width {
                let Ok(dst_x) = u32::try_from(i64::from(piece.x) + i64::from(src_x)) else {
                    continue;
                };
                if dst_x >= width {
                    break;
                }
                let src = (src_y as usize * frame.width as usize + src_x as usize) * 4;
                let rgb = [
                    frame.pixels[src],
                    frame.pixels[src + 1],
                    frame.pixels[src + 2],
                ];
                let dst = dst_y as usize * stride + dst_x as usize * bpp;
                format.write(&mut out[dst..dst + bpp], format.encode(rgb));
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
pub(super) use live::set;

#[cfg(target_os = "linux")]
mod live {
    use x11rb::connection::Connection;
    use x11rb::image::{BitsPerPixel, Image, ImageOrder, ScanlinePad};
    use x11rb::protocol::xproto::{
        self, AtomEnum, ChangeWindowAttributesAux, CloseDown, ConnectionExt as _, CreateGCAux,
        Pixmap, PropMode, Window,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    use super::{PixelFormat, compose, pieces};
    use crate::engine::wallpaper_sink::WallpaperJob;

    const ROOT_ATOMS: [&str; 2] = ["_XROOTPMAP_ID", "ESETROOT_PMAP_ID"];

    fn fail(what: &str, e: &dyn std::fmt::Display) -> String {
        format!("the X root pixmap: {what}: {e}")
    }

    fn refused(e: &dyn std::fmt::Display) -> String {
        fail("the X server refused", e)
    }

    /// The root window and what an image of it has to look like.
    struct Root {
        window: Window,
        depth: u8,
        width: u16,
        height: u16,
        format: PixelFormat,
    }

    fn root_of(conn: &RustConnection, screen_num: usize) -> Result<Root, String> {
        let setup = conn.setup();
        let screen = setup
            .roots
            .get(screen_num)
            .ok_or_else(|| fail("the display has no such screen", &screen_num))?;
        let depth = screen.root_depth;
        let visual = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == depth)
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual_id == screen.root_visual)
            .ok_or_else(|| {
                fail(
                    "the root visual is not among the screen's",
                    &screen.root_visual,
                )
            })?;
        let pixmap_format = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == depth)
            .ok_or_else(|| fail("no pixmap format for the root depth", &depth))?;
        Ok(Root {
            window: screen.root,
            depth,
            width: screen.width_in_pixels,
            height: screen.height_in_pixels,
            format: PixelFormat {
                bits_per_pixel: pixmap_format.bits_per_pixel,
                scanline_pad: pixmap_format.scanline_pad,
                red_mask: visual.red_mask,
                green_mask: visual.green_mask,
                blue_mask: visual.blue_mask,
                lsb_first: setup.image_byte_order == xproto::ImageOrder::LSB_FIRST,
            },
        })
    }

    /// Paint the root with this publish, and free the pixmap the last setter
    /// left behind.
    ///
    /// The pixmap outlives this connection on purpose: `RetainPermanent` hands
    /// it to the server, which is what every root setter does, and the next
    /// publish finds it through the two atoms and frees it by its id.
    pub(in crate::wallpaper::linux) fn set(job: &WallpaperJob) -> Result<(), String> {
        let (conn, screen_num) =
            x11rb::connect(None).map_err(|e| fail("no X display to connect to", &e))?;
        let root = root_of(&conn, screen_num)?;
        let format = root.format;
        let data = compose(
            u32::from(root.width),
            u32::from(root.height),
            &pieces(job),
            format,
        );
        let image = Image::new(
            root.width,
            root.height,
            ScanlinePad::try_from(format.scanline_pad).map_err(|e| fail("scanline pad", &e))?,
            root.depth,
            BitsPerPixel::try_from(format.bits_per_pixel)
                .map_err(|e| fail("bits per pixel", &e))?,
            if format.lsb_first {
                ImageOrder::LsbFirst
            } else {
                ImageOrder::MsbFirst
            },
            std::borrow::Cow::Owned(data),
        )
        .map_err(|e| fail("the image does not fit its own size", &e))?;

        let pixmap = conn.generate_id().map_err(|e| refused(&e))?;
        conn.create_pixmap(root.depth, pixmap, root.window, root.width, root.height)
            .map_err(|e| refused(&e))?;
        let gc = conn.generate_id().map_err(|e| refused(&e))?;
        conn.create_gc(gc, pixmap, &CreateGCAux::new())
            .map_err(|e| refused(&e))?;
        image
            .put(&conn, pixmap, gc, 0, 0)
            .map_err(|e| refused(&e))?;
        drop(image);
        conn.free_gc(gc).map_err(|e| refused(&e))?;

        install(&conn, root.window, pixmap)?;
        tracing::info!(
            pixmap,
            width = root.width,
            height = root.height,
            "the X root pixmap is set"
        );
        Ok(())
    }

    /// Make `pixmap` the root's background, the way feh and Esetroot do.
    fn install(conn: &RustConnection, root: Window, pixmap: Pixmap) -> Result<(), String> {
        let mut atoms = [0; 2];
        for (atom, name) in atoms.iter_mut().zip(ROOT_ATOMS) {
            *atom = conn
                .intern_atom(false, name.as_bytes())
                .map_err(|e| refused(&e))?
                .reply()
                .map_err(|e| refused(&e))?
                .atom;
        }
        let previous: Vec<Option<u32>> = atoms
            .iter()
            .map(|&atom| {
                conn.get_property(false, root, atom, AtomEnum::PIXMAP, 0, 1)
                    .ok()
                    .and_then(|cookie| cookie.reply().ok())
                    .and_then(|reply| reply.value32().and_then(|mut v| v.next()))
            })
            .collect();
        if let [Some(old), Some(esetroot)] = previous[..]
            && old == esetroot
            && old != 0
        {
            // The previous setter's connection is gone, so its pixmap is the
            // only resource that id's client still holds. An id somebody
            // already freed is an error nobody needs to hear about.
            if let Ok(cookie) = conn.kill_client(old) {
                let _ = cookie.check();
            }
        }
        for atom in atoms {
            conn.change_property32(PropMode::REPLACE, root, atom, AtomEnum::PIXMAP, &[pixmap])
                .map_err(|e| refused(&e))?;
        }
        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().background_pixmap(pixmap),
        )
        .map_err(|e| refused(&e))?;
        conn.clear_area(false, root, 0, 0, 0, 0)
            .map_err(|e| refused(&e))?;
        conn.set_close_down_mode(CloseDown::RETAIN_PERMANENT)
            .map_err(|e| refused(&e))?;
        conn.get_input_focus()
            .map_err(|e| refused(&e))?
            .reply()
            .map_err(|e| refused(&e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGB32: PixelFormat = PixelFormat {
        bits_per_pixel: 32,
        scanline_pad: 32,
        red_mask: 0x00ff_0000,
        green_mask: 0x0000_ff00,
        blue_mask: 0x0000_00ff,
        lsb_first: true,
    };

    fn solid(width: u32, height: u32, rgb: [u8; 3]) -> Frame {
        let pixels = (0..width * height)
            .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
            .collect();
        Frame::new(pixels, width, height)
    }

    fn pixel_at(image: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [image[i], image[i + 1], image[i + 2], image[i + 3]]
    }

    #[test]
    fn each_monitor_lands_at_its_own_rectangle() {
        let left = solid(4, 2, [255, 0, 0]);
        let right = solid(2, 3, [0, 0, 255]);
        let pieces = [
            Piece {
                x: 0,
                y: 0,
                frame: &left,
            },
            Piece {
                x: 4,
                y: 0,
                frame: &right,
            },
        ];
        let image = compose(6, 3, &pieces, RGB32);
        assert_eq!(image.len(), 6 * 3 * 4);
        // Little-endian 0x00RRGGBB is B, G, R, 0 in memory.
        assert_eq!(pixel_at(&image, 6, 0, 0), [0, 0, 255, 0]);
        assert_eq!(pixel_at(&image, 6, 3, 1), [0, 0, 255, 0]);
        assert_eq!(pixel_at(&image, 6, 4, 0), [255, 0, 0, 0]);
        assert_eq!(pixel_at(&image, 6, 5, 2), [255, 0, 0, 0]);
        // Below the shorter left screen nothing was painted.
        assert_eq!(pixel_at(&image, 6, 0, 2), [0, 0, 0, 0]);
    }

    #[test]
    fn a_canvas_is_placed_where_it_spans_and_clipped_to_the_root() {
        let canvas = solid(8, 2, [0, 255, 0]);
        let pieces = [Piece {
            x: -2,
            y: 1,
            frame: &canvas,
        }];
        let image = compose(4, 4, &pieces, RGB32);
        assert_eq!(pixel_at(&image, 4, 0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel_at(&image, 4, 0, 1), [0, 255, 0, 0]);
        assert_eq!(pixel_at(&image, 4, 3, 2), [0, 255, 0, 0]);
        assert_eq!(pixel_at(&image, 4, 3, 3), [0, 0, 0, 0]);
    }

    #[test]
    fn a_monitor_left_alone_keeps_the_background() {
        use crate::display::Monitor;
        use crate::display::layout::DisplayMode;
        use crate::engine::wallpaper_sink::{JobImages, WallpaperJob};
        use std::sync::Arc;

        let monitor = |id: &str, x: i32| Monitor {
            id: id.to_owned(),
            label: id.to_owned(),
            x,
            y: 0,
            width: 2,
            height: 2,
            primary: x == 0,
        };
        let job = WallpaperJob {
            mode: DisplayMode::OneScreen,
            monitors: vec![monitor("A", 0), monitor("B", 2)],
            anchor: 0,
            images: JobImages::PerMonitor(vec![Some(Arc::new(solid(2, 2, [255, 255, 255]))), None]),
        };
        let image = compose(4, 2, &pieces(&job), RGB32);
        assert_eq!(pixel_at(&image, 4, 1, 1), [255, 255, 255, 0]);
        assert_eq!(pixel_at(&image, 4, 2, 0), [0, 0, 0, 0]);
        assert_eq!(pixel_at(&image, 4, 3, 1), [0, 0, 0, 0]);
    }

    #[test]
    fn the_byte_order_is_the_servers() {
        let msb = PixelFormat {
            lsb_first: false,
            ..RGB32
        };
        let red = solid(1, 1, [255, 0, 0]);
        let pieces = [Piece {
            x: 0,
            y: 0,
            frame: &red,
        }];
        assert_eq!(compose(1, 1, &pieces, msb), [0, 255, 0, 0]);
        assert_eq!(compose(1, 1, &pieces, RGB32), [0, 0, 255, 0]);
    }

    #[test]
    fn a_sixteen_bit_visual_gets_its_channels_scaled_and_its_rows_padded() {
        let rgb565 = PixelFormat {
            bits_per_pixel: 16,
            scanline_pad: 32,
            red_mask: 0xf800,
            green_mask: 0x07e0,
            blue_mask: 0x001f,
            lsb_first: true,
        };
        let frame = solid(3, 1, [255, 128, 0]);
        let pieces = [Piece {
            x: 0,
            y: 0,
            frame: &frame,
        }];
        let image = compose(3, 1, &pieces, rgb565);
        // Three 16-bit pixels are six bytes, padded to eight.
        assert_eq!(image.len(), 8);
        let pixel = u16::from_le_bytes([image[0], image[1]]);
        assert_eq!(pixel >> 11, 0x1f);
        assert_eq!((pixel >> 5) & 0x3f, 32);
        assert_eq!(pixel & 0x1f, 0);
        assert_eq!(&image[6..], [0, 0]);
    }
}
