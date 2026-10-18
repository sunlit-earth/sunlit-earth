//! A color as the settings hold it: three 8 bit channels, written `#rrggbb`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A color of three 8 bit channels on the stored values a texture holds, as
/// the shader reads them: no surface format is sRGB, so the value chosen is
/// the value a texel takes.
///
/// Its text is `#rrggbb` in lower case, which is what the config file holds
/// and what the settings panel shows. It reads with or without the `#` and in
/// either case, and a value of any other shape fails the file like a value of
/// the wrong type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Rgb8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb8 {
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// The channels in the order red, green, blue.
    #[must_use]
    pub const fn channels(self) -> [u8; 3] {
        [self.r, self.g, self.b]
    }

    /// `#rrggbb`, in lower case.
    #[must_use]
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Read `#rrggbb` or `rrggbb`, in either case, with any whitespace around
    /// it, or `None` for anything else.
    #[must_use]
    pub fn parse_hex(text: &str) -> Option<Self> {
        let text = text.trim();
        let digits = text.strip_prefix('#').unwrap_or(text);
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
        Some(Self::new(channel(0)?, channel(2)?, channel(4)?))
    }
}

impl From<[u8; 3]> for Rgb8 {
    fn from([r, g, b]: [u8; 3]) -> Self {
        Self::new(r, g, b)
    }
}

impl fmt::Display for Rgb8 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl TryFrom<String> for Rgb8 {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse_hex(&text).ok_or_else(|| format!("`{text}` is not a color in the form #rrggbb"))
    }
}

impl From<Rgb8> for String {
    fn from(color: Rgb8) -> Self {
        color.to_hex()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_every_channel_value() {
        for value in 0..=u8::MAX {
            for color in [
                Rgb8::new(value, 0, 0),
                Rgb8::new(0, value, 0),
                Rgb8::new(0, 0, value),
                Rgb8::new(value, value.wrapping_mul(7), value.wrapping_add(128)),
            ] {
                assert_eq!(Rgb8::parse_hex(&color.to_hex()), Some(color), "{color:?}");
            }
        }
    }

    #[test]
    fn hex_is_written_in_lower_case_with_a_hash() {
        assert_eq!(Rgb8::new(10, 30, 60).to_hex(), "#0a1e3c");
        assert_eq!(Rgb8::new(255, 171, 0).to_string(), "#ffab00");
    }

    #[test]
    fn hex_reads_with_or_without_the_hash_in_either_case() {
        let expected = Some(Rgb8::new(0xab, 0xcd, 0xef));
        for text in [
            "#abcdef",
            "abcdef",
            "#ABCDEF",
            "ABCDEF",
            "#AbCdEf",
            "  #abcdef\n",
        ] {
            assert_eq!(Rgb8::parse_hex(text), expected, "{text:?}");
        }
    }

    #[test]
    fn hex_rejects_other_lengths_and_non_hex_digits() {
        for text in [
            "", "#", "#abc", "abc", "#abcde", "#abcdef0", "##abcdef", "#abcdeg", "#+abcde",
            "#-1bcde", "#ab cde", "0xabcd", "#éabcd",
        ] {
            assert_eq!(Rgb8::parse_hex(text), None, "{text:?}");
        }
    }

    #[test]
    fn serde_writes_and_reads_the_hex_text() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Holder {
            color: Rgb8,
        }
        let holder = Holder {
            color: Rgb8::new(5, 5, 15),
        };
        let text = toml::to_string(&holder).expect("serialize");
        assert_eq!(text.trim(), "color = \"#05050f\"");
        assert_eq!(
            toml::from_str::<Holder>(&text).expect("deserialize"),
            holder
        );
        assert!(toml::from_str::<Holder>("color = \"#05050\"").is_err());
        assert!(toml::from_str::<Holder>("color = 5").is_err());
    }
}
