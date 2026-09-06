/// Texture slot index for the day texture (JXL).
pub(super) const DAY_SLOT: usize = 1;
/// Texture slot index for the night texture (JXL).
pub(super) const NIGHT_SLOT: usize = 2;
/// Texture slot index for the Moon's surface (JXL).
const MOON_SLOT: usize = 3;
/// Texture slot index for the Milky Way panorama (JXL).
const MILKY_WAY_SLOT: usize = 4;

/// Display names of the texture modes, in combo box order. The index into this
/// array is `SceneParams::texture_index`.
pub const TEXTURE_LABELS: [&str; 4] = ["Grid", "Day", "Night", "Day/Night Blend"];

/// The texture mode a combo box index names.
///
/// A mode is not a slot. The first three modes each draw the globe from one
/// file-backed slot, `Blend` binds two of them together and has no slot of its
/// own, and the cloud overlay has a slot but no mode.
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
    /// is the answer because it is the one slot that is always loaded.
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
/// path, in the order the paths arrive; then the cloud overlay, which comes
/// from the fetcher rather than from a file and is therefore last. Deriving the
/// cloud slot from the number of paths rather than naming a constant is what
/// lets a file-backed texture be added without moving it.
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

    /// Whether `slot` holds one of the globe's own maps.
    ///
    /// This is the question `Renderer::textures_ready` answers, from the other
    /// side: the globe is what readiness is about, and the overlays are excluded
    /// from it, so a configuration whose only file is an overlay's has nothing to
    /// wait for. Slot 0 is the procedural grid, which needs no file at all.
    pub fn is_globe(self, slot: usize) -> bool {
        slot > 0
            && [TextureMode::Day, TextureMode::Night]
                .into_iter()
                .any(|mode| self.globe(mode) == slot)
    }

    /// An overlay's own file-backed slot, when this layout reaches that far.
    ///
    /// A configuration with fewer file-backed paths than production's is a
    /// configuration missing that overlay, which is a picture without it rather
    /// than anything to repair.
    fn overlay(self, slot: usize) -> Option<usize> {
        (self.file_backed >= slot).then_some(slot)
    }

    /// The slot the globe is drawn from in `mode`.
    ///
    /// `Blend` reports the day slot, which is both the fallback it renders from
    /// until the composite bind group exists and the first of the two slots its
    /// readiness depends on. Clamped to a slot this layout has a file for, so a
    /// configuration with fewer paths than production's cannot reach past its
    /// own file-backed range into the cloud slot.
    pub(super) fn globe(self, mode: TextureMode) -> usize {
        let slot = match mode {
            TextureMode::Grid => 0,
            TextureMode::Day | TextureMode::Blend => DAY_SLOT,
            TextureMode::Night => NIGHT_SLOT,
        };
        slot.min(self.file_backed)
    }
}

/// What each file-backed slot's GPU texture is called. The cloud overlay is
/// named by [`Renderer::slot_label`] instead, because its slot is wherever the
/// layout puts it.
///
/// The allocator report the memory report is built from names allocations by
/// their GPU label, so a row that reads `day_texture` is worth more than one
/// that reads `texture_slot_1`.
pub(super) const SLOT_LABELS: [&str; 5] = [
    "grid_texture",
    "day_texture",
    "night_texture",
    "moon_texture",
    "milky_way_texture",
];

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
            #[allow(clippy::cast_sign_loss)]
            let expected = TEXTURE_LABELS[index as usize];
            assert_eq!(mode.label(), expected, "index {index}");
        }
    }

    /// Production's layout: the grid, the day and night surfaces, the Moon, the
    /// Milky Way, and the cloud overlay last.
    #[test]
    fn the_production_layout_puts_the_clouds_after_the_overlays() {
        let layout = SlotLayout::new(4);
        assert_eq!(layout.count(), 6);
        assert_eq!(layout.clouds(), 5);
        assert_eq!(layout.moon(), Some(MOON_SLOT));
        assert_eq!(layout.milky_way(), Some(MILKY_WAY_SLOT));
        assert_eq!(layout.globe(TextureMode::Grid), 0);
        assert_eq!(layout.globe(TextureMode::Day), DAY_SLOT);
        assert_eq!(layout.globe(TextureMode::Night), NIGHT_SLOT);
        assert_eq!(layout.globe(TextureMode::Blend), DAY_SLOT);
        for slot in 0..layout.count() {
            assert_eq!(
                layout.is_globe(slot),
                slot == DAY_SLOT || slot == NIGHT_SLOT,
                "slot {slot}"
            );
        }
    }

    /// The cloud overlay is always the last slot, whatever comes before it, the
    /// mailbox has one slot per texture, and a layout too short for an overlay
    /// reports no slot for it rather than one that belongs to something else.
    #[test]
    fn a_layout_without_an_overlay_says_so() {
        assert_eq!(SlotLayout::new(3).milky_way(), None);
        assert!(
            !SlotLayout::new(1).is_globe(NIGHT_SLOT),
            "a layout with one path has no night map, and clamping is not a second globe slot"
        );
        assert_eq!(SlotLayout::new(3).moon(), Some(MOON_SLOT));
        assert_eq!(SlotLayout::new(2).moon(), None);
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

    /// No mode ever indexes past the file-backed slots.
    #[test]
    fn no_mode_reaches_the_cloud_slot() {
        for file_backed in 0..6 {
            let layout = SlotLayout::new(file_backed);
            for (index, mode) in MODES {
                let slot = layout.globe(mode);
                assert!(
                    slot < layout.clouds(),
                    "{file_backed} paths, index {index}: slot {slot} is the cloud slot {}",
                    layout.clouds()
                );
                assert!(
                    slot <= file_backed,
                    "{file_backed} paths, index {index}: slot {slot} has no file behind it"
                );
            }
        }
    }
}
