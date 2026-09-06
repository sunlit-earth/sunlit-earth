//! Embedded star catalog access.

use std::fmt;

const MAGIC: &[u8; 4] = b"SSTR";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 12;
pub const RECORD_SIZE: usize = 16;
const EMBEDDED: &[u8] = include_bytes!("stars/hyg_v4_4_mag7.bin");

/// A validated catalog whose payload is already in the GPU instance format.
#[derive(Clone, Copy, Debug)]
pub struct StarCatalog<'a> {
    count: u32,
    payload: &'a [u8],
}

impl StarCatalog<'_> {
    /// Number of star records in the catalog.
    #[cfg(test)]
    pub(crate) fn len(&self) -> u32 {
        self.count
    }

    /// The byte for byte wgpu vertex buffer payload.
    pub fn instance_bytes(&self) -> &[u8] {
        self.payload
    }

    /// Number of leading records at or brighter than `magnitude_limit`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the magnitude is clamped to the encoded byte range"
    )]
    pub fn visible_count(&self, magnitude_limit: f32) -> u32 {
        let encoded_limit =
            (((magnitude_limit.clamp(-2.0, 8.0) + 2.0) / 10.0) * 255.0).round() as u8;
        let mut low = 0_usize;
        let mut high =
            usize::try_from(self.count).map_or(self.payload.len() / RECORD_SIZE, |count| count);
        while low < high {
            let middle = low + (high - low) / 2;
            let magnitude = self.payload[middle * RECORD_SIZE + 15];
            if magnitude <= encoded_limit {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        u32::try_from(low).map_or(self.count, |count| count)
    }
}

/// Why an embedded star catalog could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StarCatalogError {
    HeaderTooShort,
    InvalidMagic,
    UnsupportedVersion(u32),
    InvalidLength,
}

impl fmt::Display for StarCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HeaderTooShort => formatter.write_str("star catalog header is truncated"),
            Self::InvalidMagic => formatter.write_str("star catalog magic is invalid"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "star catalog version {version} is not supported")
            }
            Self::InvalidLength => formatter.write_str("star catalog payload length is invalid"),
        }
    }
}

impl std::error::Error for StarCatalogError {}

/// Validate a catalog blob and borrow its instance payload.
pub(crate) fn parse_catalog(bytes: &[u8]) -> Result<StarCatalog<'_>, StarCatalogError> {
    if bytes.len() < HEADER_SIZE {
        return Err(StarCatalogError::HeaderTooShort);
    }
    if &bytes[..4] != MAGIC {
        return Err(StarCatalogError::InvalidMagic);
    }
    let version = u32::from_le_bytes(
        bytes[4..8]
            .try_into()
            .map_err(|_| StarCatalogError::HeaderTooShort)?,
    );
    if version != VERSION {
        return Err(StarCatalogError::UnsupportedVersion(version));
    }
    let count = u32::from_le_bytes(
        bytes[8..12]
            .try_into()
            .map_err(|_| StarCatalogError::HeaderTooShort)?,
    );
    let count_usize = usize::try_from(count).map_err(|_| StarCatalogError::InvalidLength)?;
    let expected = count_usize
        .checked_mul(RECORD_SIZE)
        .and_then(|payload| HEADER_SIZE.checked_add(payload))
        .ok_or(StarCatalogError::InvalidLength)?;
    if bytes.len() != expected {
        return Err(StarCatalogError::InvalidLength);
    }
    Ok(StarCatalog {
        count,
        payload: &bytes[HEADER_SIZE..],
    })
}

/// Return the validated HYG v4.4 catalog embedded in the executable.
pub fn embedded_catalog() -> StarCatalog<'static> {
    parse_catalog(EMBEDDED).expect("shipped star catalog is validated by tests")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/stars.bin");

    #[test]
    fn fixture_payload_is_returned_without_copying() {
        let catalog = parse_catalog(FIXTURE).expect("fixture catalog is valid");
        assert!(std::ptr::eq(
            catalog.instance_bytes().as_ptr(),
            FIXTURE[HEADER_SIZE..].as_ptr()
        ));
    }

    #[test]
    fn fixture_has_three_hand_checked_stars() {
        let catalog = parse_catalog(FIXTURE).expect("fixture catalog is valid");
        assert_eq!(catalog.len(), 3);
    }

    #[test]
    fn visible_count_uses_the_magnitude_sorted_prefix() {
        let catalog = parse_catalog(FIXTURE).expect("fixture catalog is valid");
        assert_eq!(catalog.visible_count(2.0), 2);
    }

    #[test]
    fn wrong_magic_is_rejected() {
        let mut bytes = FIXTURE.to_vec();
        bytes[0] = b'X';
        assert_eq!(
            parse_catalog(&bytes).unwrap_err(),
            StarCatalogError::InvalidMagic
        );
    }

    #[test]
    fn wrong_version_is_rejected() {
        let mut bytes = FIXTURE.to_vec();
        bytes[4..8].copy_from_slice(&2_u32.to_le_bytes());
        assert_eq!(
            parse_catalog(&bytes).unwrap_err(),
            StarCatalogError::UnsupportedVersion(2)
        );
    }

    #[test]
    fn mismatched_count_is_rejected() {
        let mut bytes = FIXTURE.to_vec();
        bytes[8..12].copy_from_slice(&4_u32.to_le_bytes());
        assert_eq!(
            parse_catalog(&bytes).unwrap_err(),
            StarCatalogError::InvalidLength
        );
    }

    /// The exact count is a property of the bake rather than of this code, and
    /// the xtask's bake-comparison test is what pins it. What matters here is
    /// that the blob the crate ships parses and holds a sky.
    #[test]
    fn the_shipped_catalog_holds_a_sky() {
        let count = embedded_catalog().len();
        assert!(count > 10_000, "only {count} stars in the shipped catalog");
    }

    #[test]
    fn shipped_directions_are_finite_units() {
        let catalog = embedded_catalog();
        let every_direction_is_valid =
            catalog
                .instance_bytes()
                .chunks_exact(RECORD_SIZE)
                .all(|record| {
                    let component = |offset| {
                        f32::from_le_bytes(
                            record[offset..offset + 4]
                                .try_into()
                                .expect("four-byte component"),
                        )
                    };
                    let direction = glam::Vec3::new(component(0), component(4), component(8));
                    direction.is_finite() && (direction.length() - 1.0).abs() < 1.0e-5
                });
        assert!(every_direction_is_valid);
    }

    /// The range a magnitude byte can decode to, which is the range
    /// `visible_count` clamps its limit into. A magnitude outside it would mean
    /// the encoding and the decoding disagree.
    const ENCODABLE_MAGNITUDES: std::ops::RangeInclusive<f32> = -2.0..=8.0;

    #[test]
    fn shipped_magnitudes_are_within_the_encodable_range() {
        let catalog = embedded_catalog();
        let every_magnitude_is_valid =
            catalog
                .instance_bytes()
                .chunks_exact(RECORD_SIZE)
                .all(|record| {
                    let magnitude = f32::from(record[15]) / 255.0 * 10.0 - 2.0;
                    ENCODABLE_MAGNITUDES.contains(&magnitude)
                });
        assert!(every_magnitude_is_valid);
    }

    #[test]
    fn shipped_magnitudes_are_sorted_brightest_first() {
        let catalog = embedded_catalog();
        assert!(
            catalog
                .instance_bytes()
                .chunks_exact(RECORD_SIZE)
                .map(|record| record[15])
                .is_sorted()
        );
    }
}
