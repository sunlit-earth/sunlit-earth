//! The globe's surface on the equi-angular cube: the floor of the month in
//! force, the night floor and the water mask, made resident from the tile packs
//! one whole cube at a time, and the procedural grid that stands in for them.
//!
//! The set is one unit beside the texture slots rather than three of them: it
//! arrives from packs the transcoder builds, not from a decode thread and the
//! mailbox, and the engine hands each pack over on its own thread as it lands.

use crate::assets::texture_loader;
use crate::assets::tiles::{BlockFormat, Entry, Pack, PackKind, decode_bc4, decode_bc7};

use super::slots::TextureMode;
use super::tiles::SurfaceTiles;

/// Anisotropy of the surface sampler on an adapter that is not a CPU.
///
/// The 8 px gutter of a tile was sized for it, and the floors share the
/// sampler, since a second one costs a software adapter more than the whole
/// cube path does.
const SURFACE_ANISOTROPY: u16 = 8;

/// The one sampler the globe reads every cube through: the floors, the mask
/// and the grid.
///
/// A cube ignores the address mode. A CPU adapter gets no anisotropy, because
/// anisotropic filtering of a cube costs a software rasterizer seconds per
/// frame wherever the frame crosses a face edge (`docs/rendering.md`).
#[must_use]
pub fn surface_sampler_descriptor(cpu_adapter: bool) -> wgpu::SamplerDescriptor<'static> {
    wgpu::SamplerDescriptor {
        label: Some("surface_sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: if cpu_adapter { 1 } else { SURFACE_ANISOTROPY },
        ..Default::default()
    }
}

/// The formats the surface cubes are created in on one device.
///
/// The packs hold BC7 for the day and the night and BC4 for the mask on every
/// adapter. An adapter that can sample BC7 and is not a CPU samples the blocks
/// themselves; a CPU adapter samples BC7 slowly enough to matter, and an adapter
/// without block compression cannot create the texture, so both get the blocks
/// decoded to RGBA8 at upload. The mask stays BC4 wherever the adapter has
/// block compression, a CPU adapter included, and is decoded to R8 where it
/// does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceFormats {
    pub color: wgpu::TextureFormat,
    pub mask: wgpu::TextureFormat,
}

impl SurfaceFormats {
    pub(crate) fn for_adapter(block_compression: bool, cpu_adapter: bool) -> Self {
        Self {
            color: if block_compression && !cpu_adapter {
                wgpu::TextureFormat::Bc7RgbaUnorm
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            mask: if block_compression {
                wgpu::TextureFormat::Bc4RUnorm
            } else {
                wgpu::TextureFormat::R8Unorm
            },
        }
    }

    fn of(self, blocks: BlockFormat) -> wgpu::TextureFormat {
        match blocks {
            BlockFormat::Bc7 => self.color,
            BlockFormat::Bc4 => self.mask,
        }
    }
}

/// One cube of the surface set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceLayer {
    /// The day floor of a month, January 0.
    Day(usize),
    Night,
    Mask,
}

impl SurfaceLayer {
    /// The pack the cube is read from.
    pub(crate) fn pack(self) -> PackKind {
        match self {
            Self::Day(month) => PackKind::Day(month),
            Self::Night => PackKind::Night,
            Self::Mask => PackKind::Mask,
        }
    }

    /// Its GPU label, which is what the memory report names it by.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Day(_) => "day_floor",
            Self::Night => "night_floor",
            Self::Mask => "water_mask",
        }
    }

    fn blocks(self) -> BlockFormat {
        match self {
            Self::Day(_) | Self::Night => BlockFormat::Bc7,
            Self::Mask => BlockFormat::Bc4,
        }
    }
}

/// What the renderer can say about one cube a mode needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LayerState {
    Resident,
    /// Nothing is resident yet and its pack may still come.
    Waiting,
    /// Its pack failed in this run, so nothing further is coming.
    Failed,
}

/// The cubes `mode` draws the globe from while `month` is in force.
pub(super) fn needs(mode: TextureMode, month: usize) -> Vec<SurfaceLayer> {
    match mode {
        TextureMode::Grid => Vec::new(),
        TextureMode::Day => vec![SurfaceLayer::Day(month)],
        TextureMode::Night => vec![SurfaceLayer::Night],
        TextureMode::Blend => vec![
            SurfaceLayer::Day(month),
            SurfaceLayer::Night,
            SurfaceLayer::Mask,
        ],
    }
}

/// Whether every cube in `needs` is resident or failed, and whether any of
/// them is still on its way.
///
/// A cube whose pack failed counts as there, as a failed tile does (plan
/// decision 10): nothing further is coming for it, and the frame draws what
/// it would draw without it, so a mode that needs it is ready once the rest
/// is.
pub(super) fn readiness(
    needs: &[SurfaceLayer],
    state: impl Fn(SurfaceLayer) -> LayerState,
) -> (bool, bool) {
    let ready = needs
        .iter()
        .all(|&layer| state(layer) != LayerState::Waiting);
    let pending = needs
        .iter()
        .any(|&layer| state(layer) == LayerState::Waiting);
    (ready, pending)
}

/// A cube the renderer holds.
///
/// The texture is held beside its view for the memory report, which reads the
/// shape off it.
pub(super) struct ResidentCube {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

impl ResidentCube {
    pub(super) fn new(texture: wgpu::Texture) -> Self {
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        });
        Self { texture, view }
    }
}

/// Which of the surface's bind groups a frame draws the globe with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SurfaceGroup {
    /// The day floor alone.
    Day,
    /// The night floor alone.
    Night,
    /// The day and the night floors and the mask.
    Blend,
}

/// The surface set: what is resident, what failed, the tiles above the floors,
/// and the bind groups built from what is resident.
pub(super) struct SurfaceSet {
    pub formats: SurfaceFormats,
    /// The month in force, January 0.
    pub month: usize,
    /// The day floor of every month whose pack has landed, January first.
    /// They all stay resident (plan decision 4), so a date in any of them is
    /// drawn from its own floor the moment it names it.
    pub days: [Option<ResidentCube>; 12],
    /// The month whose day floor is drawn: the month in force once its floor
    /// is resident, and until then the one drawn before, which is a truer
    /// picture than the grid while the month in force's pack builds.
    pub drawn: Option<usize>,
    pub night: Option<ResidentCube>,
    pub mask: Option<ResidentCube>,
    failed: Vec<SurfaceLayer>,
    /// The tile array and the page table, which the renderer creates beside
    /// the set on its device.
    pub tiles: Option<SurfaceTiles>,
    /// How many times a day floor was made resident, one that was let go of
    /// and made resident again counting twice.
    pub floors_installed: u64,
    pub day_group: Option<wgpu::BindGroup>,
    pub night_group: Option<wgpu::BindGroup>,
    pub blend_group: Option<wgpu::BindGroup>,
}

impl SurfaceSet {
    pub(super) fn new(formats: SurfaceFormats, month: usize) -> Self {
        Self {
            formats,
            month,
            days: std::array::from_fn(|_| None),
            drawn: None,
            night: None,
            mask: None,
            failed: Vec::new(),
            tiles: None,
            floors_installed: 0,
            day_group: None,
            night_group: None,
            blend_group: None,
        }
    }

    /// Make `month` the month in force. Returns whether the day floor drawn
    /// changed, which it does at once when that month's floor is resident.
    pub(super) fn set_month(&mut self, month: usize) -> bool {
        self.month = month;
        self.draw_the_month_in_force()
    }

    /// Hold `cube` as the floor of `layer`. Returns whether the day floor
    /// drawn changed: the month in force's has come, or the first of any.
    pub(super) fn hold(&mut self, layer: SurfaceLayer, cube: ResidentCube) -> bool {
        match layer {
            SurfaceLayer::Day(month) => {
                self.days[month] = Some(cube);
                self.floors_installed += 1;
                let first = self.drawn.is_none();
                if first {
                    self.drawn = Some(month);
                }
                self.draw_the_month_in_force() || first
            }
            SurfaceLayer::Night => {
                self.night = Some(cube);
                false
            }
            SurfaceLayer::Mask => {
                self.mask = Some(cube);
                false
            }
        }
    }

    /// Let go of the day floor of `month`, unless it is the month in force's
    /// or the one drawn. Returns whether it was resident and is let go of.
    pub(super) fn release_day(&mut self, month: usize) -> bool {
        if month == self.month || self.drawn == Some(month) {
            return false;
        }
        self.days[month]
            .take()
            .map(|cube| cube.texture.destroy())
            .is_some()
    }

    fn draw_the_month_in_force(&mut self) -> bool {
        let changed = self.days[self.month].is_some() && self.drawn != Some(self.month);
        if changed {
            self.drawn = Some(self.month);
        }
        changed
    }

    /// The day floor drawn.
    pub(super) fn drawn_day(&self) -> Option<&ResidentCube> {
        self.days[self.drawn?].as_ref()
    }

    pub(super) fn state(&self, layer: SurfaceLayer) -> LayerState {
        let resident = match layer {
            SurfaceLayer::Day(month) => self.days.get(month).is_some_and(Option::is_some),
            SurfaceLayer::Night => self.night.is_some(),
            SurfaceLayer::Mask => self.mask.is_some(),
        };
        if resident {
            LayerState::Resident
        } else if self.failed.contains(&layer) {
            LayerState::Failed
        } else {
            LayerState::Waiting
        }
    }

    pub(super) fn mark_failed(&mut self, layer: SurfaceLayer) {
        if !self.failed.contains(&layer) {
            self.failed.push(layer);
        }
    }

    /// Whether the cubes `mode` needs are all resident, and whether one of them
    /// is still on its way.
    pub(super) fn readiness(&self, mode: TextureMode) -> (bool, bool) {
        readiness(&needs(mode, self.month), |layer| self.state(layer))
    }

    /// The group `mode` draws with and whether it blends, or `None` while
    /// nothing it can draw is resident.
    ///
    /// Blend falls back to the day floor alone until the night floor is there
    /// too.
    pub(super) fn route(&self, mode: TextureMode) -> Option<(SurfaceGroup, bool)> {
        match mode {
            TextureMode::Grid => None,
            TextureMode::Blend if self.blend_group.is_some() => Some((SurfaceGroup::Blend, true)),
            TextureMode::Day | TextureMode::Blend => {
                self.day_group.as_ref().map(|_| (SurfaceGroup::Day, false))
            }
            TextureMode::Night => self
                .night_group
                .as_ref()
                .map(|_| (SurfaceGroup::Night, false)),
        }
    }

    pub(super) fn group(&self, group: SurfaceGroup) -> Option<&wgpu::BindGroup> {
        match group {
            SurfaceGroup::Day => self.day_group.as_ref(),
            SurfaceGroup::Night => self.night_group.as_ref(),
            SurfaceGroup::Blend => self.blend_group.as_ref(),
        }
    }

    /// Every texture held, the cubes, the page table and the tile array, with
    /// the label it carries.
    pub(super) fn resident(&self) -> impl Iterator<Item = (&'static str, &wgpu::Texture)> {
        let days = self
            .days
            .iter()
            .enumerate()
            .filter_map(|(month, cube)| Some((SurfaceLayer::Day(month), cube.as_ref()?)));
        let night = self.night.as_ref().map(|cube| (SurfaceLayer::Night, cube));
        let mask = self.mask.as_ref().map(|cube| (SurfaceLayer::Mask, cube));
        days.chain([night, mask].into_iter().flatten())
            .map(|(layer, cube)| (layer.label(), &cube.texture))
            .chain(self.tiles.iter().flat_map(SurfaceTiles::textures))
    }
}

/// Create the cube a pack's whole faces make, in the format this device takes
/// it in, and upload every level of every face.
///
/// A face is read, uploaded and let go of before the next is read, decoded
/// first where the device does not sample the blocks.
pub(super) fn cube_from_pack(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layer: SurfaceLayer,
    pack: &Pack,
    formats: SurfaceFormats,
) -> Result<wgpu::Texture, String> {
    if pack.kind() != layer.pack() || pack.format() != layer.blocks() {
        return Err(format!(
            "a {:?} pack in {:?} cannot make the {layer:?} cube",
            pack.kind(),
            pack.format()
        ));
    }
    let faces = whole_faces(pack)?;
    let size = 1_u32 << faces[0].key.level;
    let levels = pack.mips(faces[0]);
    let format = formats.of(pack.format());
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(layer.label()),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: u32::try_from(levels.len()).map_err(|e| e.to_string())?,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (face, entry) in faces.iter().enumerate() {
        let blob = pack.read(entry).map_err(|e| e.to_string())?;
        for (level, mip) in pack.mips(entry).iter().enumerate() {
            let blocks = blob
                .get(mip.offset..mip.offset + mip.len)
                .ok_or_else(|| format!("face {face} is shorter than its level {level}"))?;
            let texels = match (pack.format(), format) {
                (BlockFormat::Bc7, wgpu::TextureFormat::Rgba8Unorm) => {
                    decode_bc7(blocks, mip.size, mip.size)?
                }
                (BlockFormat::Bc4, wgpu::TextureFormat::R8Unorm) => {
                    decode_bc4(blocks, mip.size, mip.size)?
                }
                _ => blocks.to_vec(),
            };
            write_cube_level(
                queue,
                &texture,
                CubeLevel {
                    face: u32::try_from(face).expect("six faces"),
                    level: u32::try_from(level).expect("a short chain"),
                    size: mip.size,
                },
                &texels,
            );
        }
    }
    Ok(texture)
}

/// The six whole-face entries of a pack, in face order, all of one size.
fn whole_faces(pack: &Pack) -> Result<[&Entry; 6], String> {
    let mut faces: [Option<&Entry>; 6] = [None; 6];
    for entry in pack.entries().iter().filter(|entry| entry.whole_face) {
        let slot = faces
            .get_mut(usize::from(entry.key.face))
            .ok_or_else(|| format!("a whole face numbered {}", entry.key.face))?;
        if slot.replace(entry).is_some() {
            return Err(format!("face {} is in the pack twice", entry.key.face));
        }
    }
    let faces = faces
        .into_iter()
        .enumerate()
        .map(|(face, entry)| entry.ok_or_else(|| format!("the pack has no face {face}")))
        .collect::<Result<Vec<_>, _>>()?;
    if faces
        .iter()
        .any(|entry| entry.key.level != faces[0].key.level)
    {
        return Err("the faces of the pack differ in size".to_owned());
    }
    Ok(faces.try_into().expect("six faces"))
}

/// Where one level of one face goes.
#[derive(Clone, Copy)]
pub(super) struct CubeLevel {
    pub face: u32,
    pub level: u32,
    /// The level's width in texels, which a block format rounds up to whole
    /// blocks.
    pub size: u32,
}

/// Write the texels or blocks of one level of one face.
pub(super) fn write_cube_level(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    at: CubeLevel,
    data: &[u8],
) {
    let format = texture.format();
    let (block_width, block_height) = format.block_dimensions();
    let block_bytes = format
        .block_copy_size(None)
        .expect("a surface format has a fixed block size");
    let (wide, high) = (
        at.size.div_ceil(block_width),
        at.size.div_ceil(block_height),
    );
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: at.level,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: at.face,
            },
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(wide * block_bytes),
            rows_per_image: Some(high),
        },
        wgpu::Extent3d {
            width: wide * block_width,
            height: high * block_height,
            depth_or_array_layers: 1,
        },
    );
}

/// An RGBA8 cube with a full mip chain, filled face by face from `face`, each
/// level box-filtered from the one above it.
pub(super) fn mipmapped_cube(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    size: u32,
    face: impl Fn(usize) -> Vec<u8>,
) -> wgpu::Texture {
    let levels = size.max(1).ilog2() + 1;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for index in 0..6 {
        let mut pixels = face(index);
        let mut width = size;
        for level in 0..levels {
            if level > 0 {
                pixels = texture_loader::downsample_2x(&pixels, width, width);
                width = (width / 2).max(1);
            }
            write_cube_level(
                queue,
                &texture,
                CubeLevel {
                    face: u32::try_from(index).expect("six faces"),
                    level,
                    size: width,
                },
                &pixels,
            );
        }
    }
    texture
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gpu_samples_the_blocks_and_a_cpu_samples_decoded_texels() {
        let gpu = SurfaceFormats::for_adapter(true, false);
        assert_eq!(gpu.color, wgpu::TextureFormat::Bc7RgbaUnorm);
        assert_eq!(gpu.mask, wgpu::TextureFormat::Bc4RUnorm);

        let cpu = SurfaceFormats::for_adapter(true, true);
        assert_eq!(cpu.color, wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(
            cpu.mask,
            wgpu::TextureFormat::Bc4RUnorm,
            "BC4 costs a CPU nothing"
        );

        for cpu_adapter in [false, true] {
            let without = SurfaceFormats::for_adapter(false, cpu_adapter);
            assert_eq!(without.color, wgpu::TextureFormat::Rgba8Unorm);
            assert_eq!(without.mask, wgpu::TextureFormat::R8Unorm);
        }
    }

    /// Nothing of the surface is sRGB: the shader works on the stored values,
    /// as it does for every flat texture.
    #[test]
    fn no_surface_format_is_srgb() {
        for block_compression in [false, true] {
            for cpu_adapter in [false, true] {
                let formats = SurfaceFormats::for_adapter(block_compression, cpu_adapter);
                assert!(!formats.color.is_srgb() && !formats.mask.is_srgb());
            }
        }
    }

    #[test]
    fn a_cpu_adapter_samples_without_anisotropy() {
        assert_eq!(surface_sampler_descriptor(true).anisotropy_clamp, 1);
        assert!(surface_sampler_descriptor(false).anisotropy_clamp > 1);
    }

    #[test]
    fn each_mode_needs_its_own_cubes() {
        assert!(needs(TextureMode::Grid, 3).is_empty());
        assert_eq!(needs(TextureMode::Day, 3), [SurfaceLayer::Day(3)]);
        assert_eq!(needs(TextureMode::Night, 3), [SurfaceLayer::Night]);
        assert_eq!(
            needs(TextureMode::Blend, 11),
            [
                SurfaceLayer::Day(11),
                SurfaceLayer::Night,
                SurfaceLayer::Mask
            ]
        );
    }

    #[test]
    fn a_mode_is_ready_when_all_it_needs_is_resident() {
        let blend = needs(TextureMode::Blend, 4);
        let all = |_| LayerState::Resident;
        assert_eq!(readiness(&blend, all), (true, false));

        let night_waits = |layer| match layer {
            SurfaceLayer::Night => LayerState::Waiting,
            _ => LayerState::Resident,
        };
        assert_eq!(readiness(&blend, night_waits), (false, true));
        assert_eq!(
            readiness(&needs(TextureMode::Day, 4), night_waits),
            (true, false),
            "the day alone does not wait for the night"
        );
    }

    /// A failed pack is terminal: the mode is ready and nothing is left to
    /// wait for, unless another cube it needs is still coming.
    #[test]
    fn a_failed_cube_is_not_waited_for() {
        let blend = needs(TextureMode::Blend, 0);
        let mask_failed = |layer| match layer {
            SurfaceLayer::Mask => LayerState::Failed,
            _ => LayerState::Resident,
        };
        assert_eq!(readiness(&blend, mask_failed), (true, false));
        assert_eq!(
            readiness(&blend, |_| LayerState::Failed),
            (true, false),
            "every cube failed"
        );

        let mask_failed_night_waits = |layer| match layer {
            SurfaceLayer::Mask => LayerState::Failed,
            SurfaceLayer::Night => LayerState::Waiting,
            SurfaceLayer::Day(_) => LayerState::Resident,
        };
        assert_eq!(readiness(&blend, mask_failed_night_waits), (false, true));
    }

    #[test]
    fn a_day_floor_of_another_month_is_not_the_month_in_force() {
        let mut set = SurfaceSet::new(SurfaceFormats::for_adapter(true, false), 5);
        assert_eq!(set.state(SurfaceLayer::Day(5)), LayerState::Waiting);
        set.mark_failed(SurfaceLayer::Day(5));
        assert_eq!(set.state(SurfaceLayer::Day(5)), LayerState::Failed);
        assert_eq!(set.state(SurfaceLayer::Day(6)), LayerState::Waiting);
        assert_eq!(set.readiness(TextureMode::Grid), (true, false));
        assert_eq!(set.readiness(TextureMode::Day), (true, false));
        set.month = 6;
        assert_eq!(set.readiness(TextureMode::Day), (false, true));
        assert_eq!(set.route(TextureMode::Blend), None, "nothing to draw yet");
    }
}
