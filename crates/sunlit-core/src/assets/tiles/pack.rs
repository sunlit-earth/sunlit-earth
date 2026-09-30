//! The pack file: a header, a sorted index, and the blobs it points at.
//!
//! Everything is little endian. The fixed header is 48 bytes:
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 8 | magic, `SUNLTILE` |
//! | 8 | 4 | format version |
//! | 12 | 4 | where the blobs may start: the header, the key and the index |
//! | 16 | 4 | CRC-32 of every byte from offset 20 to the end of the index |
//! | 20 | 1 | what the pack holds: 0 a month of day, 1 the night, 2 the mask |
//! | 21 | 1 | the month, 1 to 12, for a day pack, else 0 |
//! | 22 | 1 | block format: 1 BC7, 2 BC4 |
//! | 23 | 1 | zero |
//! | 24 | 4 | tile width without its gutter |
//! | 28 | 4 | gutter width on each side |
//! | 32 | 4 | levels in a tile's blob |
//! | 36 | 4 | the constant ocean color, RGBA8 |
//! | 40 | 4 | index entries |
//! | 44 | 4 | key bytes |
//!
//! Then the cache key as UTF-8, zero padded to a multiple of 8, then the index
//! of 56-byte entries sorted by level, face, row and column:
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 1 | level: the base 2 logarithm of the face width at this level |
//! | 1 | 1 | face, in cube layer order |
//! | 2 | 1 | flags: 1 constant ocean with no blob, 2 a whole face |
//! | 3 | 1 | zero |
//! | 4 | 2 | tile row |
//! | 6 | 2 | tile column |
//! | 8 | 8 | blob offset from the start of the file |
//! | 16 | 4 | blob length |
//! | 20 | 4 | CRC-32 of the blob |
//! | 24 | 32 | SHA-256 of the blob |
//!
//! A tile's blob is its layer, the tile and its gutter, and the one mip below
//! it, each a level of blocks with rows first. A whole face's blob is the face
//! and every mip below it down to one texel. Blobs follow the index in its
//! order, so the coarse levels come first.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use super::PackKind;
use super::codec::{self, BlockFormat};

const MAGIC: [u8; 8] = *b"SUNLTILE";
pub(crate) const FORMAT_VERSION: u32 = 1;
const FIXED_HEADER: usize = 48;
const ENTRY_BYTES: usize = 56;
const CRC_START: usize = 20;
const FLAG_OCEAN: u8 = 1;
const FLAG_FACE: u8 = 2;

/// Where a tile is: its level, its face and its place on the face.
///
/// A level is named by the base 2 logarithm of the face width at that level,
/// so 11 is the 2048 level and a tile at `(row, col)` of level `l` covers the
/// four tiles `(2 row .. 2 row + 1, 2 col .. 2 col + 1)` of level `l + 1`. A
/// whole face sits at row and column 0 of its level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TileKey {
    pub level: u8,
    pub face: u8,
    pub row: u16,
    pub col: u16,
}

/// One index entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: TileKey,
    /// Constant ocean: nothing is stored, and the pack's ocean color stands in.
    pub ocean: bool,
    /// A whole face with its full mip chain rather than a tile with its mip.
    pub whole_face: bool,
    pub offset: u64,
    pub length: u32,
    pub crc32: u32,
    pub hash: [u8; 32],
}

/// What a pack says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Header {
    pub kind: PackKind,
    pub format: BlockFormat,
    pub tile: u32,
    pub gutter: u32,
    pub tile_levels: u32,
    pub ocean: [u8; 4],
    pub key: String,
}

/// Why a pack could not be opened or a blob not read.
#[derive(Debug)]
pub enum PackError {
    Io(io::Error),
    /// Not a pack this build can read: another format version, a torn or
    /// damaged header or index, or an index that contradicts itself.
    Invalid(String),
    /// A blob whose bytes do not match the checksum the index recorded.
    Corrupt(TileKey),
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Invalid(why) => write!(f, "not a usable pack: {why}"),
            Self::Corrupt(key) => write!(f, "the blob of {key:?} fails its checksum"),
        }
    }
}

impl std::error::Error for PackError {}

impl From<io::Error> for PackError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// The bytes before the first blob.
pub(crate) fn header_len(key: &str, entries: usize) -> usize {
    FIXED_HEADER + key.len().next_multiple_of(8) + entries * ENTRY_BYTES
}

/// The header and the index, ready to be written at the start of the file.
pub(crate) fn encode_header(header: &Header, entries: &[Entry]) -> Vec<u8> {
    let len = header_len(&header.key, entries.len());
    let mut out = Vec::with_capacity(len);
    out.extend(MAGIC);
    out.extend(FORMAT_VERSION.to_le_bytes());
    out.extend(u32_of(len).to_le_bytes());
    out.extend([0; 4]);
    let (kind, month) = match header.kind {
        PackKind::Day(month) => (0, u8::try_from(month + 1).expect("a month is a byte")),
        PackKind::Night => (1, 0),
        PackKind::Mask => (2, 0),
    };
    out.extend([kind, month, header.format.code(), 0]);
    out.extend(header.tile.to_le_bytes());
    out.extend(header.gutter.to_le_bytes());
    out.extend(header.tile_levels.to_le_bytes());
    out.extend(header.ocean);
    out.extend(u32_of(entries.len()).to_le_bytes());
    out.extend(u32_of(header.key.len()).to_le_bytes());
    out.extend(header.key.as_bytes());
    out.resize(FIXED_HEADER + header.key.len().next_multiple_of(8), 0);
    for entry in entries {
        let flags = (u8::from(entry.ocean) * FLAG_OCEAN) | (u8::from(entry.whole_face) * FLAG_FACE);
        out.extend([entry.key.level, entry.key.face, flags, 0]);
        out.extend(entry.key.row.to_le_bytes());
        out.extend(entry.key.col.to_le_bytes());
        out.extend(entry.offset.to_le_bytes());
        out.extend(entry.length.to_le_bytes());
        out.extend(entry.crc32.to_le_bytes());
        out.extend(entry.hash);
    }
    debug_assert_eq!(out.len(), len);
    let crc = crc32fast::hash(&out[CRC_START..]);
    out[16..20].copy_from_slice(&crc.to_le_bytes());
    out
}

fn u32_of(n: usize) -> u32 {
    u32::try_from(n).expect("a pack's header fits in 4 GiB")
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn le_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().expect("two bytes"))
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

/// The deepest level a tile key can name, which makes 2^15 the widest face.
const MAX_LEVEL: u8 = 15;

/// Whether a tile's sizes fit the format: a tile and gutter no wider than the
/// widest face, and no more levels in the blob than the layer has mips.
fn check_tile_layout(tile: u32, gutter: u32, tile_levels: u32) -> Result<(), String> {
    let widest = 1_u32 << MAX_LEVEL;
    if !(1..=widest).contains(&tile) || gutter > widest {
        return Err(format!("a tile of {tile} px with a gutter of {gutter} px"));
    }
    let layer = tile + 2 * gutter;
    let mips = codec::full_chain(layer);
    if !(1..=mips).contains(&tile_levels) {
        return Err(format!(
            "{tile_levels} levels in a layer of {layer} px, which has {mips}"
        ));
    }
    Ok(())
}

/// Parse and check a header and index read from a file of `file_len` bytes.
fn decode_header(bytes: &[u8], file_len: u64) -> Result<(Header, Vec<Entry>), PackError> {
    let invalid = |why: String| PackError::Invalid(why);
    let kind = match (bytes[20], bytes[21]) {
        (0, month @ 1..=12) => PackKind::Day(usize::from(month - 1)),
        (1, 0) => PackKind::Night,
        (2, 0) => PackKind::Mask,
        (kind, month) => return Err(invalid(format!("kind {kind} month {month}"))),
    };
    let format = BlockFormat::from_code(bytes[22])
        .ok_or_else(|| invalid(format!("block format {}", bytes[22])))?;
    let (tile, gutter, tile_levels) = (le_u32(bytes, 24), le_u32(bytes, 28), le_u32(bytes, 32));
    check_tile_layout(tile, gutter, tile_levels).map_err(invalid)?;
    let ocean = bytes[36..40].try_into().expect("four bytes");
    let count = le_u32(bytes, 40) as usize;
    let key_len = le_u32(bytes, 44) as usize;
    if header_len_checked(key_len, count) != Some(bytes.len()) {
        return Err(invalid(format!(
            "{count} entries and a key of {key_len} bytes do not fill {} bytes",
            bytes.len()
        )));
    }
    let key = std::str::from_utf8(&bytes[FIXED_HEADER..FIXED_HEADER + key_len])
        .map_err(|e| invalid(format!("the key is not UTF-8: {e}")))?
        .to_owned();
    let index = FIXED_HEADER + key_len.next_multiple_of(8);
    let tile_blob = codec::blob_bytes(format, tile + 2 * gutter, tile_levels);
    let mut entries: Vec<Entry> = Vec::with_capacity(count);
    for raw in bytes[index..].chunks_exact(ENTRY_BYTES) {
        let entry = Entry {
            key: TileKey {
                level: raw[0],
                face: raw[1],
                row: le_u16(raw, 4),
                col: le_u16(raw, 6),
            },
            ocean: raw[2] & FLAG_OCEAN != 0,
            whole_face: raw[2] & FLAG_FACE != 0,
            offset: le_u64(raw, 8),
            length: le_u32(raw, 16),
            crc32: le_u32(raw, 20),
            hash: raw[24..56].try_into().expect("32 bytes"),
        };
        let key = entry.key;
        if raw[2] & !(FLAG_OCEAN | FLAG_FACE) != 0 || usize::from(key.face) >= 6 || key.level > MAX_LEVEL {
            return Err(invalid(format!("entry {key:?} is malformed")));
        }
        if entries.last().is_some_and(|last| last.key >= key) {
            return Err(invalid(format!("entry {key:?} is out of order")));
        }
        let expected = if entry.ocean {
            0
        } else if entry.whole_face {
            let size = 1_u32 << key.level;
            codec::blob_bytes(format, size, codec::full_chain(size))
        } else {
            tile_blob
        };
        if entry.length as usize != expected {
            return Err(invalid(format!(
                "entry {key:?} holds {} bytes, its layout {expected}",
                entry.length
            )));
        }
        let end = entry.offset.checked_add(u64::from(entry.length));
        if !entry.ocean
            && (entry.offset < bytes.len() as u64 || end.is_none_or(|end| end > file_len))
        {
            return Err(invalid(format!("entry {key:?} points outside the blobs")));
        }
        entries.push(entry);
    }
    let header = Header {
        kind,
        format,
        tile,
        gutter,
        tile_levels,
        ocean,
        key,
    };
    Ok((header, entries))
}

fn header_len_checked(key_len: usize, count: usize) -> Option<usize> {
    FIXED_HEADER
        .checked_add(key_len.checked_next_multiple_of(8)?)?
        .checked_add(count.checked_mul(ENTRY_BYTES)?)
}

/// An open pack: its header and index in memory, its blobs a positional read
/// away. Reads take `&self`, so one pack serves any number of threads.
#[derive(Debug)]
pub struct Pack {
    file: File,
    header: Header,
    entries: Vec<Entry>,
}

impl Pack {
    /// Open the pack at `path` and check its header and index.
    ///
    /// The blobs are not read; each one is checked against its checksum when
    /// [`Pack::read`] reads it.
    pub fn open(path: &Path) -> Result<Self, PackError> {
        let mut file = File::open(path)?;
        let file_len = file.metadata()?.len();
        let mut fixed = [0_u8; FIXED_HEADER];
        file.read_exact(&mut fixed)?;
        if fixed[..8] != MAGIC {
            return Err(PackError::Invalid("no pack magic".to_owned()));
        }
        let version = le_u32(&fixed, 8);
        if version != FORMAT_VERSION {
            return Err(PackError::Invalid(format!(
                "format version {version}, this build reads {FORMAT_VERSION}"
            )));
        }
        let len = le_u32(&fixed, 12) as usize;
        if len < FIXED_HEADER || len as u64 > file_len {
            return Err(PackError::Invalid(format!(
                "a header of {len} bytes in a file of {file_len}"
            )));
        }
        let mut bytes = fixed.to_vec();
        bytes.resize(len, 0);
        file.read_exact(&mut bytes[FIXED_HEADER..])?;
        if crc32fast::hash(&bytes[CRC_START..]) != le_u32(&bytes, 16) {
            return Err(PackError::Invalid(
                "the header and index fail their checksum".to_owned(),
            ));
        }
        let (header, entries) = decode_header(&bytes, file_len)?;
        Ok(Self {
            file,
            header,
            entries,
        })
    }

    /// The cache key the pack was built under.
    pub fn key(&self) -> &str {
        &self.header.key
    }

    pub fn kind(&self) -> PackKind {
        self.header.kind
    }

    pub fn format(&self) -> BlockFormat {
        self.header.format
    }

    /// The width of a tile's layer, the tile and both gutters.
    pub fn layer(&self) -> u32 {
        self.header.tile + 2 * self.header.gutter
    }

    pub fn tile(&self) -> u32 {
        self.header.tile
    }

    pub fn gutter(&self) -> u32 {
        self.header.gutter
    }

    /// The levels in a tile's blob, the layer and its mips.
    pub fn tile_levels(&self) -> u32 {
        self.header.tile_levels
    }

    /// The color a constant-ocean tile stands for: the mean of the finest
    /// level's open water, which the bake flattened to one color.
    pub fn ocean(&self) -> [u8; 4] {
        self.header.ocean
    }

    /// Every entry, in index order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn find(&self, key: TileKey) -> Option<&Entry> {
        self.entries
            .binary_search_by(|entry| entry.key.cmp(&key))
            .ok()
            .map(|at| &self.entries[at])
    }

    /// The levels of `entry`'s blob, finest first.
    pub fn mips(&self, entry: &Entry) -> Vec<codec::MipLevel> {
        if entry.whole_face {
            let size = 1_u32 << entry.key.level;
            codec::mip_levels(self.header.format, size, codec::full_chain(size))
        } else {
            codec::mip_levels(self.header.format, self.layer(), self.header.tile_levels)
        }
    }

    /// Read `entry`'s blob and check it against its checksum. A constant
    /// ocean entry reads as nothing.
    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>, PackError> {
        let mut blob = vec![0_u8; entry.length as usize];
        read_exact_at(&self.file, &mut blob, entry.offset)?;
        if crc32fast::hash(&blob) != entry.crc32 {
            return Err(PackError::Corrupt(entry.key));
        }
        Ok(blob)
    }
}

/// Fill `buf` from `offset` of `file` without moving a shared cursor, so that
/// reads on several threads never interleave.
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
    }
    #[cfg(windows)]
    {
        let (mut buf, mut offset) = (buf, offset);
        while !buf.is_empty() {
            match std::os::windows::fs::FileExt::seek_read(file, buf, offset) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => {
                    buf = &mut buf[n..];
                    offset += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (file, buf, offset);
        Err(io::ErrorKind::Unsupported.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ScratchDir;
    use std::io::Write;

    fn header(key: &str) -> Header {
        Header {
            kind: PackKind::Day(4),
            format: BlockFormat::Bc7,
            tile: 8,
            gutter: 4,
            tile_levels: 2,
            ocean: [10, 30, 60, 255],
            key: key.to_owned(),
        }
    }

    /// A pack of two tiles and an ocean entry, with blobs of recognizable bytes.
    fn write_small_pack(path: &Path) -> (Vec<Entry>, Vec<Vec<u8>>) {
        let head = header("the key");
        let blob = codec::blob_bytes(BlockFormat::Bc7, 16, 2);
        let keys = [(3, 0, 0, 0), (4, 2, 1, 0), (4, 2, 1, 1)];
        let start = header_len(&head.key, keys.len()) as u64;
        let mut blobs = Vec::new();
        let mut entries = Vec::new();
        for (i, (level, face, row, col)) in keys.into_iter().enumerate() {
            let ocean = i == 1;
            let bytes: Vec<u8> = if ocean {
                Vec::new()
            } else {
                (0..blob)
                    .map(|b| u8::try_from((b + i * 17) % 251).unwrap())
                    .collect()
            };
            entries.push(Entry {
                key: TileKey {
                    level,
                    face,
                    row,
                    col,
                },
                ocean,
                whole_face: false,
                offset: if ocean {
                    0
                } else {
                    start + blobs.iter().map(|b: &Vec<u8>| b.len() as u64).sum::<u64>()
                },
                length: u32::try_from(bytes.len()).unwrap(),
                crc32: crc32fast::hash(&bytes),
                hash: [u8::try_from(i).unwrap(); 32],
            });
            if !ocean {
                blobs.push(bytes);
            }
        }
        let mut file = File::create(path).unwrap();
        file.write_all(&encode_header(&head, &entries)).unwrap();
        for blob in &blobs {
            file.write_all(blob).unwrap();
        }
        (entries, blobs)
    }

    #[test]
    fn a_pack_reads_back_what_was_written() {
        let dir = ScratchDir::new("tiles_pack_round_trip");
        let path = dir.join("p.pack");
        let (entries, blobs) = write_small_pack(&path);

        let pack = Pack::open(&path).expect("open");
        assert_eq!(pack.key(), "the key");
        assert_eq!(pack.kind(), PackKind::Day(4));
        assert_eq!(pack.ocean(), [10, 30, 60, 255]);
        assert_eq!((pack.layer(), pack.tile_levels()), (16, 2));
        assert_eq!(pack.entries(), entries.as_slice());
        let last = pack.find(entries[2].key).expect("found");
        assert_eq!(pack.read(last).expect("read"), blobs[1]);
        assert!(pack.read(&entries[1]).expect("an ocean entry").is_empty());
        assert!(
            pack.find(TileKey {
                level: 4,
                face: 2,
                row: 0,
                col: 0
            })
            .is_none()
        );
        let mips = pack.mips(last);
        assert_eq!(mips.iter().map(|m| m.size).collect::<Vec<_>>(), [16, 8]);
    }

    #[test]
    fn a_damaged_blob_fails_its_checksum() {
        let dir = ScratchDir::new("tiles_pack_crc");
        let path = dir.join("p.pack");
        let (entries, _) = write_small_pack(&path);
        let mut bytes = std::fs::read(&path).unwrap();
        let at = usize::try_from(entries[2].offset).unwrap() + 5;
        bytes[at] ^= 0x40;
        std::fs::write(&path, bytes).unwrap();

        let pack = Pack::open(&path).expect("the index is intact");
        assert!(
            matches!(pack.read(&entries[2]), Err(PackError::Corrupt(key)) if key == entries[2].key)
        );
        assert!(pack.read(&entries[0]).is_ok(), "the other blob is fine");
    }

    #[test]
    fn a_damaged_or_truncated_index_is_refused() {
        let dir = ScratchDir::new("tiles_pack_index");
        let path = dir.join("p.pack");
        let (entries, _) = write_small_pack(&path);
        let good = std::fs::read(&path).unwrap();

        let mut flipped = good.clone();
        flipped[FIXED_HEADER + 8 + 9] ^= 1;
        std::fs::write(&path, &flipped).unwrap();
        assert!(matches!(Pack::open(&path), Err(PackError::Invalid(_))));

        let end = usize::try_from(entries[2].offset).unwrap() + 3;
        std::fs::write(&path, &good[..end]).unwrap();
        assert!(
            matches!(Pack::open(&path), Err(PackError::Invalid(_))),
            "a blob past the end"
        );

        std::fs::write(&path, &good[..30]).unwrap();
        assert!(matches!(Pack::open(&path), Err(PackError::Io(_))));

        let mut other = good.clone();
        other[8] = 99;
        std::fs::write(&path, &other).unwrap();
        let Err(PackError::Invalid(why)) = Pack::open(&path) else {
            panic!("another format version must be refused");
        };
        assert!(why.contains("version 99"), "{why}");
    }

    /// The checksum covers the header too, so a contradiction written with a
    /// valid checksum is caught by the checks behind it.
    #[test]
    fn an_index_that_contradicts_itself_is_refused() {
        let dir = ScratchDir::new("tiles_pack_order");
        let path = dir.join("p.pack");
        let head = header("k");
        let entry = |col, length| Entry {
            key: TileKey {
                level: 4,
                face: 0,
                row: 0,
                col,
            },
            ocean: length == 0,
            whole_face: false,
            offset: 0,
            length,
            crc32: 0,
            hash: [0; 32],
        };
        for (entries, why) in [
            (vec![entry(1, 0), entry(0, 0)], "out of order"),
            (vec![entry(1, 0), entry(1, 0)], "out of order"),
            (vec![entry(0, 7)], "its layout"),
        ] {
            std::fs::write(&path, encode_header(&head, &entries)).unwrap();
            let Err(PackError::Invalid(found)) = Pack::open(&path) else {
                panic!("{why} must be refused");
            };
            assert!(found.contains(why), "{found}");
        }
    }

    #[test]
    fn a_header_with_sizes_the_format_cannot_hold_is_refused() {
        let dir = ScratchDir::new("tiles_pack_sizes");
        let path = dir.join("p.pack");
        for (tile, gutter, tile_levels) in [
            (0xFFFF_FFF0, 4, 2),
            (0, 4, 2),
            (65_536, 4, 2),
            (8, 0xFFFF_FFF0, 2),
            (8, 65_536, 2),
            (8, 4, 0),
            (8, 4, 6),
            (8, 4, 1_000_000),
            (8, 4, u32::MAX),
        ] {
            let head = Header {
                tile,
                gutter,
                tile_levels,
                ..header("k")
            };
            std::fs::write(&path, encode_header(&head, &[])).unwrap();
            assert!(
                matches!(Pack::open(&path), Err(PackError::Invalid(_))),
                "{tile} {gutter} {tile_levels}"
            );
        }
        let whole = Header {
            tile_levels: 5,
            ..header("k")
        };
        std::fs::write(&path, encode_header(&whole, &[])).unwrap();
        assert!(Pack::open(&path).is_ok(), "every mip of a layer is allowed");
    }

    #[test]
    fn positional_reads_do_not_depend_on_each_other() {
        let dir = ScratchDir::new("tiles_pack_read_at");
        let path = dir.join("bytes");
        let bytes: Vec<u8> = (0..=255).collect();
        std::fs::write(&path, &bytes).unwrap();
        let file = File::open(&path).unwrap();

        let mut later = [0_u8; 4];
        read_exact_at(&file, &mut later, 200).unwrap();
        let mut earlier = [0_u8; 4];
        read_exact_at(&file, &mut earlier, 3).unwrap();
        assert_eq!((later, earlier), ([200, 201, 202, 203], [3, 4, 5, 6]));

        let mut past = [0_u8; 8];
        let err = read_exact_at(&file, &mut past, 252).expect_err("four bytes short");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
