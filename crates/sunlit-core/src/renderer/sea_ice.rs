//! The sea ice atlas on the GPU: its 24 caps as the layers of one array.
//!
//! The atlas is a JPEG XL of 12 columns by 2 rows of square caps, January to
//! December across, the north above the south (`textures/PROVENANCE.md`). The
//! array holds them as layer `2 * month + cap`, cap 0 the north's, so the
//! shader picks a layer by the date's month and the sign of the normal and a
//! change of month rebuilds nothing.

use wgpu::util::DeviceExt;

use crate::assets::texture_loader::{Channels, DecodedImage};

/// Months across the atlas.
const MONTHS: u32 = 12;

/// Caps per month, the north's and the south's.
const CAPS: u32 = 2;

/// Layers of the array.
pub(super) const LAYERS: u32 = MONTHS * CAPS;

/// The format the caps are held in: the concentration, 0 to 1.
pub(super) const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// The side of one cap of a decoded atlas, when the atlas has the layout of
/// one.
fn cap_side(image: &DecodedImage) -> Result<u32, String> {
    if image.channels != Channels::Red {
        return Err("the sea ice atlas is decoded to one channel".to_owned());
    }
    let side = image.width / MONTHS;
    if side == 0 || image.width != side * MONTHS || image.height != side * CAPS {
        return Err(format!(
            "a sea ice atlas of {}x{} is not {MONTHS} by {CAPS} square caps",
            image.width, image.height
        ));
    }
    Ok(side)
}

/// Create the array of caps from a decoded atlas and upload every layer.
///
/// The atlas is read in place, cap by cap, through the row stride of the whole
/// image, so nothing beside it is allocated for the upload.
pub(super) fn array_from_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    atlas: &DecodedImage,
) -> Result<wgpu::Texture, String> {
    let side = cap_side(atlas)?;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: LAYERS,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let stride = atlas.width as usize;
    for cap in 0..CAPS {
        for month in 0..MONTHS {
            let offset = (cap * side) as usize * stride + (month * side) as usize;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: month * CAPS + cap,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &atlas.pixels[offset..],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(atlas.width),
                    rows_per_image: Some(side),
                },
                wgpu::Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    Ok(texture)
}

/// The array every bind group holds until the atlas is resident: one texel
/// of no ice in each of the 24 layers.
pub(super) fn stand_in(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("dummy_sea_ice"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: LAYERS,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &[0; LAYERS as usize],
    );
    array_view(&texture)
}

/// The view the shader reads an array of caps through.
pub(super) fn array_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::texture_loader::Pixels;

    fn atlas(width: u32, height: u32, channels: Channels) -> DecodedImage {
        DecodedImage {
            pixels: Pixels::new(vec![0; (width * height) as usize * channels.count()]),
            width,
            height,
            channels,
        }
    }

    #[test]
    fn an_atlas_is_twelve_by_two_square_caps() {
        assert_eq!(cap_side(&atlas(12 * 64, 2 * 64, Channels::Red)), Ok(64));
        assert_eq!(cap_side(&atlas(12, 2, Channels::Red)), Ok(1));
        for (width, height) in [(12 * 64, 64), (12 * 64 + 1, 128), (64, 128), (0, 0)] {
            assert!(
                cap_side(&atlas(width, height, Channels::Red)).is_err(),
                "{width}x{height}"
            );
        }
        assert!(cap_side(&atlas(12, 2, Channels::Rgba)).is_err());
    }
}
