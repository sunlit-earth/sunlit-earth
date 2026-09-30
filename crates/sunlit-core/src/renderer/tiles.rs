//! The tiles above the floor: one texture array whose layers hold tiles of the
//! day and of the night alike, and the page table the globe's fragment shader
//! reads once per fragment to learn what refines the floor there.
//!
//! The page table is an `R32Uint` array of six layers, one per face, with one
//! cell per tile of the finest level. The low 16 bits of a cell say what the
//! day draws there and the high 16 bits what the night draws, so blend mode
//! learns both from one load. Each half is a [`PageEntry`]: the floor, the
//! constant ocean color of the surface's pack, or a layer of the array and how
//! many levels coarser than the finest the tile in it is, which is the best
//! resident ancestor of the cell. The table is rewritten whole on the CPU
//! whenever what is resident changes and uploaded with one `write_texture`.
//! The queue carries out every write staged before a submit ahead of that
//! submit's commands, so the tiles a rewrite names and the rewrite itself reach
//! the GPU together, and a layer given to another tile is never read through
//! the mapping of the one it held before.
//!
//! Public, like `uniforms`, so the render pipeline's tests drive the upload
//! and the rewrite the renderer uses rather than a copy of them.

use std::collections::{HashMap, HashSet};

use crate::assets::tiles::{
    BlockFormat, Geometry, Pack, PackKind, TILE_LEVELS, TileKey, blob_bytes, decode_bc7, mip_levels,
};

use super::surface::{CubeLevel, write_cube_level};

/// Layers the tile array is created with where the device allows as many:
/// twice the tiles the largest frame needs at once, which research section 16
/// puts at about 435 for a 4K output.
pub const TILE_LAYER_BUDGET: u32 = 900;

const LAYER_BITS: u32 = 12;
const STEPS_SHIFT: u32 = 12;
const KIND_SHIFT: u32 = 14;
const KIND_OCEAN: u16 = 1;
const KIND_TILE: u16 = 2;

/// The most layers a page table entry can name.
pub const MAX_TILE_LAYERS: u32 = 1 << LAYER_BITS;

/// The most tiled levels a page table entry can name.
const MAX_STEPS: u32 = 1 << (KIND_SHIFT - STEPS_SHIFT);

/// What one surface draws from at one cell of the page table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageEntry {
    /// The floor cube.
    Floor,
    /// The constant ocean color of the surface's pack.
    Ocean,
    /// Layer `layer` of the tile array, which holds a tile `steps` levels
    /// coarser than the finest.
    Tile { layer: u32, steps: u32 },
}

impl PageEntry {
    /// The 16 bits the shader reads: the kind in bits 14 and 15, the steps in
    /// 12 and 13, the layer in 0 to 11.
    ///
    /// # Panics
    ///
    /// If the layer or the steps do not fit their bits, which the tile array's
    /// capacity and the geometry's check rule out.
    #[must_use]
    pub fn encode(self) -> u16 {
        match self {
            Self::Floor => 0,
            Self::Ocean => KIND_OCEAN << KIND_SHIFT,
            Self::Tile { layer, steps } => {
                assert!(
                    layer < MAX_TILE_LAYERS && steps < MAX_STEPS,
                    "layer {layer} {steps} levels up does not fit a page table entry"
                );
                let bits = u16::try_from(steps << STEPS_SHIFT | layer).expect("checked above");
                (KIND_TILE << KIND_SHIFT) | bits
            }
        }
    }

    /// The entry `bits` encode, or `None` for a kind no entry has.
    #[must_use]
    pub fn decode(bits: u16) -> Option<Self> {
        let layer = u32::from(bits) & (MAX_TILE_LAYERS - 1);
        let steps = (u32::from(bits) >> STEPS_SHIFT) & (MAX_STEPS - 1);
        match bits >> KIND_SHIFT {
            0 if bits == 0 => Some(Self::Floor),
            KIND_OCEAN if bits == KIND_OCEAN << KIND_SHIFT => Some(Self::Ocean),
            KIND_TILE => Some(Self::Tile { layer, steps }),
            _ => None,
        }
    }
}

/// A tile of one surface: the pack it comes from and where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileId {
    pub pack: PackKind,
    pub key: TileKey,
}

/// A tile to make resident: its blob as [`Pack::read`] returns it, and the
/// layer to put it in when the caller has chosen one.
#[derive(Debug)]
pub struct TileUpload {
    pub id: TileId,
    pub blob: Vec<u8>,
    /// A layer another tile holds is taken from that tile. `None` keeps the
    /// layer the tile already has, or takes the lowest free one.
    pub layer: Option<u32>,
}

/// The layers of the tile array and the tile each one holds.
#[derive(Debug)]
pub struct TileLayers {
    held: Vec<Option<TileId>>,
    resident: HashMap<TileId, u32>,
}

impl TileLayers {
    #[must_use]
    pub fn new(capacity: u32) -> Self {
        Self {
            held: vec![None; capacity as usize],
            resident: HashMap::new(),
        }
    }

    #[must_use]
    pub fn capacity(&self) -> u32 {
        u32::try_from(self.held.len()).expect("a capacity that came from a u32")
    }

    /// Layers that hold no tile.
    #[must_use]
    pub fn free(&self) -> u32 {
        self.capacity() - u32::try_from(self.resident.len()).expect("at most the capacity")
    }

    #[must_use]
    pub fn layer_of(&self, id: TileId) -> Option<u32> {
        self.resident.get(&id).copied()
    }

    #[must_use]
    pub fn holder(&self, layer: u32) -> Option<TileId> {
        self.held.get(layer as usize).copied().flatten()
    }

    /// Every resident tile and its layer.
    pub fn resident(&self) -> impl Iterator<Item = (TileId, u32)> {
        self.resident.iter().map(|(&id, &layer)| (id, layer))
    }

    /// Give `id` a layer: `layer` when the caller named one, else the one it
    /// already holds, else the lowest free one. Returns the layer, and the
    /// tile that had to give it up.
    pub fn claim(
        &mut self,
        id: TileId,
        layer: Option<u32>,
    ) -> Result<(u32, Option<TileId>), String> {
        let layer = match layer {
            Some(layer) if layer < self.capacity() => layer,
            Some(layer) => {
                return Err(format!(
                    "layer {layer} of a tile array of {}",
                    self.capacity()
                ));
            }
            None => match self.layer_of(id) {
                Some(layer) => layer,
                None => self
                    .held
                    .iter()
                    .position(Option::is_none)
                    .map(|at| u32::try_from(at).expect("an index below the capacity"))
                    .ok_or_else(|| "every layer of the tile array holds a tile".to_owned())?,
            },
        };
        if let Some(before) = self.resident.insert(id, layer)
            && before != layer
        {
            self.held[before as usize] = None;
        }
        let displaced = self.held[layer as usize]
            .replace(id)
            .filter(|&other| other != id);
        if let Some(other) = displaced {
            self.resident.remove(&other);
        }
        Ok((layer, displaced))
    }

    /// Free the layer `id` holds, and say which it was.
    pub fn release(&mut self, id: TileId) -> Option<u32> {
        let layer = self.resident.remove(&id)?;
        self.held[layer as usize] = None;
        Some(layer)
    }
}

/// One surface as the page table sees it: the pack its tiles come from, and
/// the tiles that pack flags constant ocean.
#[derive(Clone, Copy, Debug)]
pub struct PageSurface<'a> {
    pub pack: PackKind,
    pub ocean: &'a HashSet<TileKey>,
}

/// The page table on the CPU: what each surface draws at each cell of each
/// face, in the layout the texture takes, face after face and row after row.
#[derive(Debug, Clone)]
pub struct PageTable {
    geometry: Geometry,
    entries: Vec<u32>,
}

impl PageTable {
    /// A table that draws the floor everywhere.
    #[must_use]
    pub fn new(geometry: Geometry) -> Self {
        let cells = geometry.face / geometry.tile;
        Self {
            geometry,
            entries: vec![0; 6 * (cells * cells) as usize],
        }
    }

    /// Cells along each side of a face: the tiles of the finest level.
    #[must_use]
    pub fn cells(&self) -> u32 {
        self.geometry.face / self.geometry.tile
    }

    /// Every cell, as the texture holds it.
    #[must_use]
    pub fn entries(&self) -> &[u32] {
        &self.entries
    }

    fn index(&self, face: u8, row: u32, col: u32) -> usize {
        let cells = self.cells() as usize;
        (usize::from(face) * cells + row as usize) * cells + col as usize
    }

    /// What the day and the night draw at a cell. An entry of no known kind
    /// reads as `None`.
    #[must_use]
    pub fn at(&self, face: u8, row: u32, col: u32) -> [Option<PageEntry>; 2] {
        let bits = self.entries[self.index(face, row, col)];
        [bits & 0xFFFF, bits >> 16]
            .map(|half| PageEntry::decode(u16::try_from(half).expect("sixteen bits")))
    }

    /// Point every cell of both halves at the best that is resident for it:
    /// its own tile, else the nearest ancestor that is resident, a layer or
    /// constant ocean alike, else the floor. A half with no surface draws the
    /// floor.
    ///
    /// Checked in a debug build: every cell has to name what it resolves to.
    pub fn rewrite(&mut self, surfaces: [Option<PageSurface<'_>>; 2], layers: &TileLayers) {
        let cells = self.cells();
        for face in 0..6_u8 {
            for row in 0..cells {
                for col in 0..cells {
                    let [day, night] = surfaces.map(|surface| {
                        surface.map_or(PageEntry::Floor, |surface| {
                            self.resolve(surface, layers, face, row, col)
                        })
                    });
                    let at = self.index(face, row, col);
                    self.entries[at] = u32::from(day.encode()) | u32::from(night.encode()) << 16;
                }
            }
        }
        debug_assert_eq!(self.check(surfaces, layers), Ok(()));
    }

    /// The tile a cell of the finest level lies in, `steps` levels coarser.
    fn ancestor(&self, face: u8, row: u32, col: u32, steps: u32) -> TileKey {
        let finest = Geometry::level_of(self.geometry.face);
        let index = |i: u32| u16::try_from(i >> steps).expect("a tile index");
        TileKey {
            level: finest - u8::try_from(steps).expect("a few levels"),
            face,
            row: index(row),
            col: index(col),
        }
    }

    fn resolve(
        &self,
        surface: PageSurface<'_>,
        layers: &TileLayers,
        face: u8,
        row: u32,
        col: u32,
    ) -> PageEntry {
        for steps in 0..self.geometry.levels {
            let key = self.ancestor(face, row, col, steps);
            let id = TileId {
                pack: surface.pack,
                key,
            };
            if let Some(layer) = layers.layer_of(id) {
                return PageEntry::Tile { layer, steps };
            }
            if surface.ocean.contains(&key) {
                return PageEntry::Ocean;
            }
        }
        PageEntry::Floor
    }

    /// Whether every cell names something that is there: a layer that holds
    /// the cell's own ancestor at the level the entry says, of the surface's
    /// pack, or an ancestor the pack flags constant ocean, or the floor, which
    /// is resident whenever a surface is drawn at all.
    ///
    /// It reads the layers from the side the rewrite does not, which layer
    /// holds what rather than where a tile is, so a layer freed or given to
    /// another tile without a rewrite is caught here.
    pub fn check(
        &self,
        surfaces: [Option<PageSurface<'_>>; 2],
        layers: &TileLayers,
    ) -> Result<(), String> {
        let cells = self.cells();
        for face in 0..6_u8 {
            for row in 0..cells {
                for col in 0..cells {
                    for (half, (entry, surface)) in self
                        .at(face, row, col)
                        .into_iter()
                        .zip(surfaces)
                        .enumerate()
                    {
                        let cell = || format!("half {half} of cell ({row}, {col}) of face {face}");
                        match (entry, surface) {
                            (None, _) => return Err(format!("{} is no entry", cell())),
                            (Some(PageEntry::Floor), _) => {}
                            (Some(_), None) => {
                                return Err(format!("{} names a tile of no surface", cell()));
                            }
                            (Some(PageEntry::Ocean), Some(surface)) => {
                                let flagged = (0..self.geometry.levels).any(|steps| {
                                    surface
                                        .ocean
                                        .contains(&self.ancestor(face, row, col, steps))
                                });
                                if !flagged {
                                    return Err(format!(
                                        "{} is ocean where {:?} flags none of its tiles",
                                        cell(),
                                        surface.pack
                                    ));
                                }
                            }
                            (Some(PageEntry::Tile { layer, steps }), Some(surface)) => {
                                let wanted = TileId {
                                    pack: surface.pack,
                                    key: self.ancestor(face, row, col, steps),
                                };
                                if steps >= self.geometry.levels
                                    || layers.holder(layer) != Some(wanted)
                                {
                                    return Err(format!(
                                        "{} names layer {layer} for {wanted:?}, which holds {:?}",
                                        cell(),
                                        layers.holder(layer)
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// What a surface's pack says about its tiles without reading one: which are
/// constant ocean, and the color they stand for.
#[derive(Debug)]
struct OceanCells {
    color: [u8; 4],
    tiles: HashSet<TileKey>,
}

/// The tile array, the page table, and what each holds, for one device.
pub struct SurfaceTiles {
    geometry: Geometry,
    /// The format the array is created in: BC7 where the device samples the
    /// blocks, RGBA8 decoded from them where it does not.
    format: wgpu::TextureFormat,
    layers: TileLayers,
    /// Created with the first tile, so a device that never draws one never
    /// holds the array.
    array: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// The day packs and the night pack handed over so far.
    ocean: HashMap<PackKind, OceanCells>,
    /// The day pack whose tiles the day half names: the month whose floor is
    /// drawn.
    day: Option<PackKind>,
    table: PageTable,
    page: wgpu::Texture,
    page_view: wgpu::TextureView,
}

impl SurfaceTiles {
    /// A page table that draws the floor everywhere, and room for `capacity`
    /// layers once the first tile comes.
    ///
    /// # Panics
    ///
    /// If `geometry` cannot be cut, which a pack built under it would already
    /// have refused.
    #[must_use]
    pub fn new(
        device: &wgpu::Device,
        geometry: Geometry,
        format: wgpu::TextureFormat,
        capacity: u32,
    ) -> Self {
        geometry
            .check()
            .unwrap_or_else(|e| panic!("the tile geometry: {e}"));
        let table = PageTable::new(geometry);
        let page = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("page_table"),
            size: wgpu::Extent3d {
                width: table.cells(),
                height: table.cells(),
                depth_or_array_layers: 6,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let page_view = page.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Self {
            geometry,
            format,
            layers: TileLayers::new(capacity.min(MAX_TILE_LAYERS)),
            array: None,
            ocean: HashMap::new(),
            day: None,
            table,
            page,
            page_view,
        }
    }

    #[must_use]
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    #[must_use]
    pub fn layers(&self) -> &TileLayers {
        &self.layers
    }

    #[must_use]
    pub fn table(&self) -> &PageTable {
        &self.table
    }

    #[must_use]
    pub fn page_view(&self) -> &wgpu::TextureView {
        &self.page_view
    }

    /// The array's view, once a tile has been uploaded.
    #[must_use]
    pub fn array_view(&self) -> Option<&wgpu::TextureView> {
        self.array.as_ref().map(|(_, view)| view)
    }

    /// The textures held, with the labels they carry.
    pub fn textures(&self) -> impl Iterator<Item = (&'static str, &wgpu::Texture)> {
        std::iter::once(("page_table", &self.page)).chain(
            self.array
                .as_ref()
                .map(|(texture, _)| ("tile_array", texture)),
        )
    }

    /// The constant ocean colors of the day pack drawn and of the night, RGBA8,
    /// zero for a surface whose pack has not come.
    #[must_use]
    pub fn ocean_colors(&self) -> [[u8; 4]; 2] {
        let color = |kind: Option<PackKind>| {
            kind.and_then(|kind| self.ocean.get(&kind))
                .map_or([0; 4], |cells| cells.color)
        };
        [color(self.day), color(Some(PackKind::Night))]
    }

    /// Whether `pack`'s tiles are cut the way this table and array take them.
    pub fn accepts(&self, pack: &Pack) -> Result<(), String> {
        if matches!(pack.kind(), PackKind::Mask) || pack.format() != BlockFormat::Bc7 {
            return Err(format!(
                "a {:?} pack in {:?} has no tiles to draw",
                pack.kind(),
                pack.format()
            ));
        }
        let cut = (pack.tile(), pack.gutter(), pack.tile_levels());
        let drawn = (self.geometry.tile, self.geometry.gutter, TILE_LEVELS);
        if cut != drawn {
            return Err(format!(
                "the pack's tiles are {cut:?} (tile, gutter, levels), the renderer draws {drawn:?}"
            ));
        }
        Ok(())
    }

    /// Take what a day or night pack says about its tiles without reading
    /// any, and rewrite the table. A day pack is the month whose floor is
    /// drawn from now on, so the day half names its tiles, and the other
    /// months' ocean is let go of.
    pub fn add_pack(&mut self, queue: &wgpu::Queue, pack: &Pack) -> Result<(), String> {
        self.accepts(pack)?;
        let tiles = pack
            .entries()
            .iter()
            .filter(|entry| entry.ocean)
            .map(|entry| entry.key)
            .collect();
        let kind = pack.kind();
        if matches!(kind, PackKind::Day(_)) {
            self.ocean
                .retain(|other, _| !matches!(other, PackKind::Day(_)));
            self.day = Some(kind);
        }
        self.ocean.insert(
            kind,
            OceanCells {
                color: pack.ocean(),
                tiles,
            },
        );
        self.publish(queue);
        Ok(())
    }

    /// Make `tiles` resident and rewrite the table over them.
    ///
    /// A tile whose blob does not have the layout of the geometry's layers, or
    /// that cannot be placed, is left out and returned with the reason; the
    /// others are uploaded. On a device that does not sample BC7 the blocks
    /// are decoded to RGBA8 here.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tiles: Vec<TileUpload>,
    ) -> Vec<(TileId, String)> {
        let mut failed = Vec::new();
        for tile in tiles {
            if let Err(why) = self.place(device, queue, &tile) {
                failed.push((tile.id, why));
            }
        }
        self.publish(queue);
        failed
    }

    /// Give one tile a layer and stage the upload of its levels.
    fn place(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tile: &TileUpload,
    ) -> Result<(), String> {
        let levels = self.levels_of(&tile.blob)?;
        let (layer, _) = self.layers.claim(tile.id, tile.layer)?;
        if self.array.is_none() {
            self.array = Some(create_array(
                device,
                &self.geometry,
                self.format,
                self.layers.capacity(),
            ));
        }
        let (texture, _) = self.array.as_ref().expect("created above");
        for (level, (size, texels)) in (0..).zip(&levels) {
            write_cube_level(
                queue,
                texture,
                CubeLevel {
                    face: layer,
                    level,
                    size: *size,
                },
                texels,
            );
        }
        Ok(())
    }

    /// Free the layers `tiles` hold and rewrite the table without them.
    pub fn evict(&mut self, queue: &wgpu::Queue, tiles: &[TileId]) {
        for &id in tiles {
            self.layers.release(id);
        }
        self.publish(queue);
    }

    /// The texels or blocks of each level of a tile's layer, in the array's
    /// format, with the level's width.
    fn levels_of(&self, blob: &[u8]) -> Result<Vec<(u32, Vec<u8>)>, String> {
        let layer = self.geometry.layer();
        let expected = blob_bytes(BlockFormat::Bc7, layer, TILE_LEVELS);
        if blob.len() != expected {
            return Err(format!(
                "a blob of {} bytes, where a layer of {layer} px is {expected}",
                blob.len()
            ));
        }
        mip_levels(BlockFormat::Bc7, layer, TILE_LEVELS)
            .into_iter()
            .map(|mip| {
                let blocks = &blob[mip.offset..mip.offset + mip.len];
                let texels = if self.format == wgpu::TextureFormat::Rgba8Unorm {
                    decode_bc7(blocks, mip.size, mip.size)?
                } else {
                    blocks.to_vec()
                };
                Ok((mip.size, texels))
            })
            .collect()
    }

    /// Rewrite the table from what is resident and stage its upload.
    fn publish(&mut self, queue: &wgpu::Queue) {
        let surface = |kind: PackKind| {
            self.ocean.get(&kind).map(|cells| PageSurface {
                pack: kind,
                ocean: &cells.tiles,
            })
        };
        let surfaces = [self.day.and_then(surface), surface(PackKind::Night)];
        self.table.rewrite(surfaces, &self.layers);
        let cells = self.table.cells();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.page,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(self.table.entries()),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * cells),
                rows_per_image: Some(cells),
            },
            wgpu::Extent3d {
                width: cells,
                height: cells,
                depth_or_array_layers: 6,
            },
        );
    }
}

fn create_array(
    device: &wgpu::Device,
    geometry: &Geometry,
    format: wgpu::TextureFormat,
    layers: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("tile_array"),
        size: wgpu::Extent3d {
            width: geometry.layer(),
            height: geometry.layer(),
            depth_or_array_layers: layers,
        },
        mip_level_count: TILE_LEVELS,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    (texture, view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::tiles::FIXTURE;

    const DAY: PackKind = PackKind::Day(3);

    fn id(level: u8, face: u8, row: u16, col: u16) -> TileId {
        TileId {
            pack: DAY,
            key: TileKey {
                level,
                face,
                row,
                col,
            },
        }
    }

    /// The fixture geometry's two tiled levels: 16 px faces, 2 x 2 cells.
    const FINE: u8 = 4;
    const COARSE: u8 = 3;

    #[test]
    fn every_entry_survives_its_encoding() {
        for entry in [
            PageEntry::Floor,
            PageEntry::Ocean,
            PageEntry::Tile { layer: 0, steps: 0 },
            PageEntry::Tile {
                layer: MAX_TILE_LAYERS - 1,
                steps: 1,
            },
            PageEntry::Tile {
                layer: 899,
                steps: 3,
            },
        ] {
            assert_eq!(PageEntry::decode(entry.encode()), Some(entry));
        }
        assert_eq!(PageEntry::Floor.encode(), 0, "a zeroed table is all floor");
        assert_eq!(PageEntry::decode(3 << KIND_SHIFT), None);
        assert_eq!(PageEntry::decode(1), None, "a floor with a layer");
    }

    #[test]
    fn a_tile_takes_the_lowest_free_layer_and_keeps_it() {
        let mut layers = TileLayers::new(3);
        assert_eq!(layers.claim(id(FINE, 0, 0, 0), None), Ok((0, None)));
        assert_eq!(layers.claim(id(FINE, 0, 0, 1), None), Ok((1, None)));
        assert_eq!(layers.claim(id(FINE, 0, 0, 0), None), Ok((0, None)));
        assert_eq!(layers.release(id(FINE, 0, 0, 0)), Some(0));
        assert_eq!(layers.claim(id(FINE, 1, 0, 0), None), Ok((0, None)));
        assert_eq!(layers.free(), 1);
        assert_eq!(layers.claim(id(FINE, 2, 0, 0), None), Ok((2, None)));
        assert!(layers.claim(id(FINE, 3, 0, 0), None).is_err(), "full");
    }

    #[test]
    fn a_named_layer_is_taken_from_its_tile() {
        let mut layers = TileLayers::new(2);
        layers.claim(id(FINE, 0, 0, 0), None).unwrap();
        layers.claim(id(FINE, 0, 0, 1), None).unwrap();
        assert_eq!(
            layers.claim(id(FINE, 0, 1, 0), Some(1)),
            Ok((1, Some(id(FINE, 0, 0, 1))))
        );
        assert_eq!(layers.layer_of(id(FINE, 0, 0, 1)), None);
        assert_eq!(layers.holder(1), Some(id(FINE, 0, 1, 0)));

        assert_eq!(
            layers.claim(id(FINE, 0, 1, 0), Some(0)),
            Ok((0, Some(id(FINE, 0, 0, 0))))
        );
        assert_eq!(layers.holder(1), None, "a tile moved leaves its layer free");
        assert_eq!(layers.free(), 1);
        assert!(layers.claim(id(FINE, 0, 1, 1), Some(2)).is_err());
    }

    fn resolved(table: &PageTable, face: u8, row: u32, col: u32) -> PageEntry {
        table.at(face, row, col)[0].expect("an entry")
    }

    /// A cell without its own tile draws from the resident parent that covers
    /// it, else from the floor; a cell with its own tile draws that.
    #[test]
    fn a_cell_draws_its_own_tile_else_its_parent_else_the_floor() {
        let ocean = HashSet::new();
        let day = PageSurface {
            pack: DAY,
            ocean: &ocean,
        };
        let mut layers = TileLayers::new(8);
        let mut table = PageTable::new(FIXTURE);
        assert_eq!(table.cells(), 2);

        layers.claim(id(FINE, 4, 1, 0), None).unwrap();
        layers.claim(id(COARSE, 4, 0, 0), None).unwrap();
        layers.claim(id(FINE, 2, 0, 1), None).unwrap();
        // Another month's tile and the night's are nothing to the day half.
        layers
            .claim(
                TileId {
                    pack: PackKind::Day(4),
                    key: id(FINE, 0, 0, 0).key,
                },
                None,
            )
            .unwrap();
        table.rewrite([Some(day), None], &layers);

        assert_eq!(
            resolved(&table, 4, 1, 0),
            PageEntry::Tile { layer: 0, steps: 0 }
        );
        for (row, col) in [(0, 0), (0, 1), (1, 1)] {
            assert_eq!(
                resolved(&table, 4, row, col),
                PageEntry::Tile { layer: 1, steps: 1 },
                "({row}, {col}) of +Z falls back on the parent"
            );
        }
        assert_eq!(
            resolved(&table, 2, 0, 1),
            PageEntry::Tile { layer: 2, steps: 0 }
        );
        assert_eq!(
            resolved(&table, 2, 0, 0),
            PageEntry::Floor,
            "no parent is resident"
        );
        assert_eq!(
            resolved(&table, 0, 0, 0),
            PageEntry::Floor,
            "another month's tile"
        );
        assert!(
            table.entries().iter().all(|&bits| bits >> 16 == 0),
            "a half with no surface is all floor"
        );
    }

    /// Constant ocean is resident without a layer, at whichever level the pack
    /// flags it, and a resident tile of a finer level wins over it.
    #[test]
    fn a_flagged_tile_is_drawn_as_ocean_where_nothing_finer_is_resident() {
        let ocean: HashSet<TileKey> = [id(COARSE, 5, 0, 0).key, id(FINE, 1, 1, 1).key].into();
        let night = PageSurface {
            pack: PackKind::Night,
            ocean: &ocean,
        };
        let mut layers = TileLayers::new(4);
        let fine = TileId {
            pack: PackKind::Night,
            key: id(FINE, 5, 1, 1).key,
        };
        layers.claim(fine, None).unwrap();
        let mut table = PageTable::new(FIXTURE);
        table.rewrite([None, Some(night)], &layers);

        let night_at = |face, row, col| table.at(face, row, col)[1].expect("an entry");
        assert_eq!(night_at(5, 0, 0), PageEntry::Ocean);
        assert_eq!(night_at(5, 1, 1), PageEntry::Tile { layer: 0, steps: 0 });
        assert_eq!(night_at(1, 1, 1), PageEntry::Ocean);
        assert_eq!(night_at(1, 0, 0), PageEntry::Floor);
        assert_eq!(table.at(5, 0, 0)[0], Some(PageEntry::Floor));
    }

    /// The check reads which tile a layer holds, so a layer freed or handed on
    /// without a rewrite shows up as a cell naming something that is not there.
    #[test]
    fn the_check_catches_a_table_left_behind_by_its_layers() {
        let ocean = HashSet::new();
        let day = PageSurface {
            pack: DAY,
            ocean: &ocean,
        };
        let mut layers = TileLayers::new(2);
        layers.claim(id(COARSE, 3, 0, 0), None).unwrap();
        let mut table = PageTable::new(FIXTURE);
        table.rewrite([Some(day), None], &layers);
        assert_eq!(table.check([Some(day), None], &layers), Ok(()));

        layers.claim(id(FINE, 0, 0, 0), Some(0)).unwrap();
        let why = table
            .check([Some(day), None], &layers)
            .expect_err("layer 0 now holds another tile");
        assert!(why.contains("layer 0"), "{why}");

        layers.release(id(FINE, 0, 0, 0));
        assert!(
            table.check([Some(day), None], &layers).is_err(),
            "a freed layer"
        );
        assert!(
            table.check([None, None], &layers).is_err(),
            "a tile of no surface"
        );
        table.rewrite([Some(day), None], &layers);
        assert_eq!(table.check([Some(day), None], &layers), Ok(()));
    }

    /// The rewrite asserts its own result in a debug build, so a table whose
    /// layers disagree with it cannot be published from one.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "holds")]
    fn a_debug_build_refuses_to_publish_a_table_that_names_a_missing_tile() {
        let ocean = HashSet::new();
        let day = PageSurface {
            pack: DAY,
            ocean: &ocean,
        };
        let mut layers = TileLayers::new(2);
        layers.claim(id(FINE, 0, 0, 0), None).unwrap();
        // A layer map whose two sides disagree, which no claim or release
        // leaves behind: the tile says layer 1, layer 1 says nothing.
        layers.resident.insert(id(FINE, 0, 0, 0), 1);
        let mut table = PageTable::new(FIXTURE);
        table.rewrite([Some(day), None], &layers);
    }

    #[test]
    fn an_ocean_entry_with_no_flag_behind_it_is_caught() {
        let flagged: HashSet<TileKey> = [id(FINE, 0, 0, 0).key].into();
        let none = HashSet::new();
        let mut table = PageTable::new(FIXTURE);
        let layers = TileLayers::new(1);
        table.rewrite(
            [
                Some(PageSurface {
                    pack: DAY,
                    ocean: &flagged,
                }),
                None,
            ],
            &layers,
        );
        let why = table
            .check(
                [
                    Some(PageSurface {
                        pack: DAY,
                        ocean: &none,
                    }),
                    None,
                ],
                &layers,
            )
            .expect_err("the flag is gone");
        assert!(why.contains("ocean"), "{why}");
    }
}
