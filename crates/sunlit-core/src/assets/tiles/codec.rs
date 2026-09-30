//! Block compression through the `dds` crate, and the layout of a mip chain of
//! blocks.

/// The version of `dds` whose output the packs hold. A test compares it with
/// the lockfile, because a preset's output may change between versions and the
/// cache key has to notice.
pub(crate) const DDS_VERSION: &str = "0.2.0";

/// The preset every block is encoded at, and its name in the cache key.
pub(crate) const PRESET: (dds::CompressionQuality, &str) = (dds::CompressionQuality::Fast, "fast");

/// How the texels of a pack are compressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockFormat {
    /// Four channels, the day and night surfaces, sampled as `Bc7RgbaUnorm`.
    Bc7,
    /// One channel, the water mask, sampled as `Bc4RUnorm`.
    Bc4,
}

impl BlockFormat {
    /// Bytes per 4 x 4 block.
    #[must_use]
    pub const fn block_bytes(self) -> usize {
        match self {
            Self::Bc7 => 16,
            Self::Bc4 => 8,
        }
    }

    /// Bytes per texel of the uncompressed input: RGBA8 or R8.
    #[must_use]
    pub const fn channels(self) -> usize {
        match self {
            Self::Bc7 => 4,
            Self::Bc4 => 1,
        }
    }

    /// Bytes of one square level `size` texels wide. A level narrower than a
    /// block still takes a whole one, as a GPU texture's does.
    #[must_use]
    pub fn level_bytes(self, size: u32) -> usize {
        let blocks = size.div_ceil(4) as usize;
        blocks * blocks * self.block_bytes()
    }

    pub(crate) const fn code(self) -> u8 {
        match self {
            Self::Bc7 => 1,
            Self::Bc4 => 2,
        }
    }

    pub(crate) const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Bc7),
            2 => Some(Self::Bc4),
            _ => None,
        }
    }

    const fn dds(self) -> (dds::Format, dds::ColorFormat) {
        match self {
            Self::Bc7 => (dds::Format::BC7_UNORM, dds::ColorFormat::RGBA_U8),
            Self::Bc4 => (dds::Format::BC4_UNORM, dds::ColorFormat::GRAYSCALE_U8),
        }
    }
}

/// One level of a blob: its width, and where its blocks are in the blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MipLevel {
    pub size: u32,
    pub offset: usize,
    pub len: usize,
}

/// The levels of a blob that holds `count` levels of a square image `size`
/// texels wide, finest first, each half the one before.
#[must_use]
pub fn mip_levels(format: BlockFormat, size: u32, count: u32) -> Vec<MipLevel> {
    let mut offset = 0;
    (0..count)
        .map(|level| {
            let size = (size >> level).max(1);
            let len = format.level_bytes(size);
            let mip = MipLevel { size, offset, len };
            offset += len;
            mip
        })
        .collect()
}

/// The number of levels in a full chain from `size` down to one texel.
#[must_use]
pub fn full_chain(size: u32) -> u32 {
    size.max(1).ilog2() + 1
}

/// The bytes of a blob of `count` levels from `size`.
#[must_use]
pub fn blob_bytes(format: BlockFormat, size: u32, count: u32) -> usize {
    mip_levels(format, size, count).iter().map(|m| m.len).sum()
}

/// Compress one square level `size` texels wide, appending its blocks to `out`.
///
/// `texels` is RGBA8 for [`BlockFormat::Bc7`] and R8 for [`BlockFormat::Bc4`].
/// With `parallel` the level is split across the current rayon pool, so a
/// caller that wants it on a pool of its own installs that pool around the
/// call. The blocks are the same either way.
pub(crate) fn encode(
    format: BlockFormat,
    texels: &[u8],
    size: u32,
    parallel: bool,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let (target, color) = format.dds();
    let image = dds::ImageView::new(texels, dds::Size::new(size, size), color)
        .ok_or_else(|| format!("{} bytes are not a {size} px level", texels.len()))?;
    let mut options = dds::EncodeOptions::default();
    options.quality = PRESET.0;
    options.parallel = parallel;
    dds::encode(out, image, target, None, &options).map_err(|e| format!("dds: {e}"))
}

/// Decompress one level of BC7 blocks `width` by `height` texels to RGBA8.
///
/// This is what an adapter that should not sample BC7 itself uploads instead:
/// a CPU adapter, or one without `TEXTURE_COMPRESSION_BC`.
pub fn decode_bc7(blocks: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let expected = width.div_ceil(4) as usize * height.div_ceil(4) as usize * 16;
    if blocks.len() != expected {
        return Err(format!(
            "{} bytes of BC7 for {width} x {height} texels, expected {expected}",
            blocks.len()
        ));
    }
    let mut pixels = vec![0_u8; width as usize * height as usize * 4];
    let image = dds::ImageViewMut::new(
        &mut pixels,
        dds::Size::new(width, height),
        dds::ColorFormat::RGBA_U8,
    )
    .ok_or_else(|| format!("no RGBA8 view of {width} x {height}"))?;
    dds::decode(
        &mut &blocks[..],
        image,
        dds::Format::BC7_UNORM,
        &dds::DecodeOptions::default(),
    )
    .map_err(|e| format!("dds: {e}"))?;
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(format: BlockFormat, texels: &[u8], size: u32) -> Vec<u8> {
        let mut out = Vec::new();
        encode(format, texels, size, true, &mut out).expect("encode");
        out
    }

    /// A smooth image with some detail, the kind the surfaces are.
    #[expect(clippy::cast_possible_truncation, reason = "the values stay bytes")]
    fn gradient(size: u32) -> Vec<u8> {
        let mut texels = Vec::new();
        for y in 0..size {
            for x in 0..size {
                let wobble = ((x * 7 + y * 3) % 5) as u8;
                texels.extend([
                    (x * 255 / size) as u8,
                    (y * 255 / size) as u8,
                    120 + wobble,
                    255,
                ]);
            }
        }
        texels
    }

    #[test]
    fn levels_are_laid_out_finest_first_and_a_small_one_takes_a_whole_block() {
        let levels = mip_levels(BlockFormat::Bc7, 144, 2);
        assert_eq!(
            levels,
            [
                MipLevel {
                    size: 144,
                    offset: 0,
                    len: 36 * 36 * 16
                },
                MipLevel {
                    size: 72,
                    offset: 36 * 36 * 16,
                    len: 18 * 18 * 16
                },
            ]
        );
        let chain = mip_levels(BlockFormat::Bc4, 8, full_chain(8));
        let sizes: Vec<_> = chain.iter().map(|m| (m.size, m.len)).collect();
        assert_eq!(sizes, [(8, 32), (4, 8), (2, 8), (1, 8)]);
        assert_eq!(blob_bytes(BlockFormat::Bc4, 8, 4), 56);
        assert_eq!(full_chain(512), 10);
        assert_eq!(full_chain(1), 1);
    }

    #[test]
    fn an_encode_fills_exactly_the_layout() {
        for size in [4, 16, 72, 144] {
            let texels = gradient(size);
            assert_eq!(
                encoded(BlockFormat::Bc7, &texels, size).len(),
                BlockFormat::Bc7.level_bytes(size)
            );
            let gray: Vec<u8> = texels.chunks(4).map(|t| t[0]).collect();
            assert_eq!(
                encoded(BlockFormat::Bc4, &gray, size).len(),
                BlockFormat::Bc4.level_bytes(size)
            );
        }
        for size in [1, 2] {
            let texels = gradient(size);
            assert_eq!(encoded(BlockFormat::Bc7, &texels, size).len(), 16);
        }
    }

    /// A block written by hand in mode 6, the one mode whose fields are simple
    /// enough to lay out here: both endpoints and every index are known, so the
    /// texels it decodes to are known without asking any encoder.
    #[test]
    fn a_hand_made_mode_6_block_decodes_to_its_endpoints() {
        // Mode 6: bit 6 set, then R0 R1 G0 G1 B0 B1 A0 A1 at 7 bits each, the
        // two p-bits, and 16 indices of 4 bits (the first one 3 bits).
        let fields: [(u32, u128); 12] = [
            (7, 0b100_0000),
            (7, 10),
            (7, 100),
            (7, 20),
            (7, 110),
            (7, 30),
            (7, 120),
            (7, 127),
            (7, 127),
            (1, 1),
            (1, 1),
            (63, 0),
        ];
        let mut bits: u128 = 0;
        let mut at = 0;
        for (width, value) in fields {
            bits |= value << at;
            at += width;
        }
        let block = bits.to_le_bytes();
        let pixels = decode_bc7(&block, 4, 4).expect("decode");
        // Endpoint 0 with its p-bit: (10 << 1 | 1) expanded from 8 bits is 21.
        for texel in pixels.chunks(4) {
            assert_eq!(texel, [21, 41, 61, 255]);
        }

        // Every index at its largest selects endpoint 1, except the first
        // texel's, whose index has one bit less: 7 is the weight 30 of 64.
        let block = (bits | (u128::MAX << 65)).to_le_bytes();
        let pixels = decode_bc7(&block, 4, 4).expect("decode");
        let blend = |e0: u32, e1: u32| (34 * e0 + 30 * e1 + 32) >> 6;
        assert_eq!(
            pixels[..4]
                .iter()
                .map(|&c| u32::from(c))
                .collect::<Vec<_>>(),
            [blend(21, 201), blend(41, 221), blend(61, 241), 255]
        );
        for texel in pixels.chunks(4).skip(1) {
            assert_eq!(texel, [201, 221, 241, 255]);
        }
    }

    #[test]
    fn a_decode_comes_back_within_bc7s_error() {
        let size = 144;
        let texels = gradient(size);
        let blocks = encoded(BlockFormat::Bc7, &texels, size);
        let decoded = decode_bc7(&blocks, size, size).expect("decode");
        assert_eq!(decoded.len(), texels.len());
        let worst = texels
            .iter()
            .zip(&decoded)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .expect("texels");
        assert!(worst <= 8, "worst channel error {worst}");
        assert!(
            decoded.chunks(4).all(|t| t[3] == 255),
            "opaque stays opaque"
        );
    }

    #[test]
    fn a_decode_refuses_a_length_that_is_not_the_level() {
        let err = decode_bc7(&[0; 16], 8, 8).expect_err("four blocks are needed");
        assert!(err.contains("expected 64"), "{err}");
    }
}
