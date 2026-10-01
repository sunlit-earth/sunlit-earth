//! Where each flat texture sits, and the texture modes the globe is drawn in.
//!
//! [`SlotLayout`] is where the order of the flat textures is decided. The
//! globe's surface is not in it: the floors, the mask and the tiles are one
//! unit beside the slots (`renderer::surface`).

/// Texture slot index for the Moon's surface (JXL).
const MOON_SLOT: usize = 1;
/// Texture slot index for the Milky Way panorama (JXL).
const MILKY_WAY_SLOT: usize = 2;

/// Display names of the texture modes, in combo box order. The index into this
/// array is `SceneParams::texture_index`.
pub const TEXTURE_LABELS: [&str; 4] = ["Grid", "Day", "Night", "Day/Night Blend"];

/// The texture mode a combo box index names.
///
/// A mode is not a slot: the grid is drawn from slot 0, the other three from
/// the cube surface, and the overlays have slots but no mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextureMode {
    Grid,
    Day,
    Night,
    Blend,
}

impl TextureMode {
    /// The mode a combo box index names, with anything outside the four the
    /// grid.
    ///
    /// A `texture_index` is a persisted integer that nothing repairs on load,
    /// so a hand-edited config can name a mode that does not exist. The grid
    /// is the answer because it is the one texture that is always loaded.
    pub(super) fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Day,
            2 => Self::Night,
            3 => Self::Blend,
            _ => Self::Grid,
        }
    }

    /// The combo box's own name for this mode, for the loading indicator.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Grid => TEXTURE_LABELS[0],
            Self::Day => TEXTURE_LABELS[1],
            Self::Night => TEXTURE_LABELS[2],
            Self::Blend => TEXTURE_LABELS[3],
        }
    }
}

/// Where each texture sits in `texture_slots`.
///
/// Slot 0 is the procedural grid, always loaded; then one slot per file-backed
/// path, in the order the paths arrive, the Moon and the Milky Way; then the
/// cloud overlay, which comes from the fetcher rather than from a file and is
/// therefore last. Deriving the cloud slot from the number of paths rather than
/// naming a constant is what lets a file-backed texture be added without moving
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotLayout {
    file_backed: usize,
}

impl SlotLayout {
    pub fn new(file_backed: usize) -> Self {
        Self { file_backed }
    }

    /// How many slots there are, which is also the mailbox's slot count.
    pub fn count(self) -> usize {
        self.file_backed + 2
    }

    /// The cloud overlay's slot.
    pub fn clouds(self) -> usize {
        self.file_backed + 1
    }

    /// The Moon's slot, in a layout that has one.
    pub fn moon(self) -> Option<usize> {
        self.overlay(MOON_SLOT)
    }

    /// The Milky Way panorama's slot, in a layout that has one.
    pub fn milky_way(self) -> Option<usize> {
        self.overlay(MILKY_WAY_SLOT)
    }

    /// An overlay's own file-backed slot, when this layout reaches that far.
    ///
    /// A configuration with fewer file-backed paths than production's is a
    /// configuration missing that overlay, which is a picture without it rather
    /// than anything to repair.
    fn overlay(self, slot: usize) -> Option<usize> {
        (self.file_backed >= slot).then_some(slot)
    }
}

/// What each file-backed slot's GPU texture is called. The cloud overlay is
/// named by [`super::Renderer::slot_label`] instead, because its slot is
/// wherever the layout puts it.
///
/// The allocator report the memory report is built from names allocations by
/// their GPU label, so a row that reads `moon_texture` is worth more than one
/// that reads `texture_slot_1`.
pub(super) const SLOT_LABELS: [&str; 3] = ["grid_texture", "moon_texture", "milky_way_texture"];

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // the slot layout
    // -----------------------------------------------------------------------

    /// Every mode the combo box can name, with the index it is named by.
    const MODES: [(i32, TextureMode); 4] = [
        (0, TextureMode::Grid),
        (1, TextureMode::Day),
        (2, TextureMode::Night),
        (3, TextureMode::Blend),
    ];

    #[test]
    fn every_combo_box_index_names_its_mode() {
        for (index, mode) in MODES {
            assert_eq!(TextureMode::from_index(index), mode, "index {index}");
        }
    }

    #[test]
    fn an_index_outside_the_modes_is_the_grid() {
        for index in [-2, -1, 4, 7, i32::MIN, i32::MAX] {
            assert_eq!(
                TextureMode::from_index(index),
                TextureMode::Grid,
                "index {index}"
            );
        }
    }

    #[test]
    fn every_mode_has_a_label() {
        for (index, mode) in MODES {
            #[expect(clippy::cast_sign_loss, reason = "the table's own indices")]
            let expected = TEXTURE_LABELS[index as usize];
            assert_eq!(mode.label(), expected, "index {index}");
        }
    }

    /// Production's layout: the grid, the Moon, the Milky Way, and the cloud
    /// overlay last.
    #[test]
    fn the_production_layout_puts_the_clouds_after_the_overlays() {
        let layout = SlotLayout::new(2);
        assert_eq!(layout.count(), 4);
        assert_eq!(layout.clouds(), 3);
        assert_eq!(layout.moon(), Some(MOON_SLOT));
        assert_eq!(layout.milky_way(), Some(MILKY_WAY_SLOT));
    }

    /// A layout too short for an overlay reports no slot for it rather than
    /// one that belongs to something else.
    #[test]
    fn a_layout_without_an_overlay_says_so() {
        assert_eq!(SlotLayout::new(1).milky_way(), None);
        assert_eq!(SlotLayout::new(1).moon(), Some(MOON_SLOT));
        assert_eq!(SlotLayout::new(0).moon(), None);
        for file_backed in 0..7 {
            let layout = SlotLayout::new(file_backed);
            for slot in [layout.moon(), layout.milky_way()].into_iter().flatten() {
                assert!(
                    slot < layout.clouds(),
                    "{file_backed} paths: overlay slot {slot} is the cloud slot"
                );
            }
        }
    }

    #[test]
    fn the_clouds_are_last_at_every_size() {
        for file_backed in 0..6 {
            let layout = SlotLayout::new(file_backed);
            assert_eq!(layout.count(), file_backed + 2, "{file_backed} paths");
            assert_eq!(layout.clouds(), layout.count() - 1, "{file_backed} paths");
        }
    }
}
