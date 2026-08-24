//! Bake the HYG catalog into the renderer's static instance buffer.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

const MAGIC: &[u8; 4] = b"SSTR";
const VERSION: u32 = 1;
const RECORD_SIZE: usize = 16;
const CATALOG_EPOCH: f64 = 2000.0;
const BAKED_EPOCH: f64 = 2026.0;
const MAX_MAGNITUDE: f64 = 7.0;
const ATTRIBUTION: &str = "# Star catalog attribution\n\nThe embedded star catalog is derived from HYG Database v4.4 by David Nash, sourced from https://codeberg.org/astronexus/hyg, and is licensed under CC BY-SA 4.0: https://creativecommons.org/licenses/by-sa/4.0/.\n";

struct Columns {
    proper_name: usize,
    ra: usize,
    dec: usize,
    magnitude: usize,
    color_index: usize,
    proper_motion_ra: usize,
    proper_motion_dec: usize,
}

impl Columns {
    fn from_headers(headers: &csv::StringRecord) -> Result<Self, String> {
        let index = |name: &str| {
            headers
                .iter()
                .position(|header| header == name)
                .ok_or_else(|| format!("HYG CSV is missing the {name:?} column"))
        };
        Ok(Self {
            proper_name: index("proper")?,
            ra: index("ra")?,
            dec: index("dec")?,
            magnitude: index("mag")?,
            color_index: index("ci")?,
            proper_motion_ra: index("pmra")?,
            proper_motion_dec: index("pmdec")?,
        })
    }
}

struct StarRecord {
    direction: [f32; 3],
    color: [u8; 3],
    magnitude: u8,
}

/// Bake `input` and write the binary catalog plus its attribution sidecar.
pub fn run(input: &Path, output: &Path) -> Result<u8, String> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(input)
        .map_err(|error| format!("failed to open {}: {error}", input.display()))?;
    let columns = Columns::from_headers(
        reader
            .headers()
            .map_err(|error| format!("failed to read HYG headers: {error}"))?,
    )?;
    let mut stars = Vec::new();
    for (row_index, row) in reader.records().enumerate() {
        let row =
            row.map_err(|error| format!("failed to read HYG row {}: {error}", row_index + 2))?;
        if let Some(star) = parse_star(&row, &columns)
            .map_err(|error| format!("invalid HYG row {}: {error}", row_index + 2))?
        {
            stars.push(star);
        }
    }

    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    let star_count = u32::try_from(stars.len())
        .map_err(|_| format!("catalog has too many stars: {}", stars.len()))?;
    let file = File::create(output)
        .map_err(|error| format!("failed to create {}: {error}", output.display()))?;
    let mut writer = BufWriter::new(file);
    writer
        .write_all(MAGIC)
        .and_then(|()| writer.write_all(&VERSION.to_le_bytes()))
        .and_then(|()| writer.write_all(&star_count.to_le_bytes()))
        .map_err(|error| format!("failed to write {}: {error}", output.display()))?;
    for star in &stars {
        for component in star.direction {
            writer
                .write_all(&component.to_le_bytes())
                .map_err(|error| format!("failed to write {}: {error}", output.display()))?;
        }
        writer
            .write_all(&[star.color[0], star.color[1], star.color[2], star.magnitude])
            .map_err(|error| format!("failed to write {}: {error}", output.display()))?;
    }
    writer
        .flush()
        .map_err(|error| format!("failed to finish {}: {error}", output.display()))?;

    let attribution_path = parent.join("ATTRIBUTION.md");
    std::fs::write(&attribution_path, ATTRIBUTION).map_err(|error| {
        format!(
            "failed to write attribution {}: {error}",
            attribution_path.display()
        )
    })?;
    println!(
        "baked {} stars into {} bytes at {}",
        stars.len(),
        12 + stars.len() * RECORD_SIZE,
        output.display()
    );
    Ok(0)
}

fn parse_star(row: &csv::StringRecord, columns: &Columns) -> Result<Option<StarRecord>, String> {
    if field(row, columns.proper_name, "proper")? == "Sol" {
        return Ok(None);
    }
    let magnitude = parse_required(row, columns.magnitude, "mag")?;
    if magnitude > MAX_MAGNITUDE {
        return Ok(None);
    }
    let ra_hours = parse_required(row, columns.ra, "ra")?;
    let dec_degrees = parse_required(row, columns.dec, "dec")?;
    let proper_motion_ra = parse_optional(row, columns.proper_motion_ra, "pmra")?.unwrap_or(0.0);
    let proper_motion_dec = parse_optional(row, columns.proper_motion_dec, "pmdec")?.unwrap_or(0.0);
    let color_index = parse_optional(row, columns.color_index, "ci")?;

    Ok(Some(StarRecord {
        direction: propagated_direction(ra_hours, dec_degrees, proper_motion_ra, proper_motion_dec),
        color: color_index.map_or([255; 3], color_from_bv),
        magnitude: encode_magnitude(magnitude),
    }))
}

fn field<'a>(row: &'a csv::StringRecord, index: usize, name: &str) -> Result<&'a str, String> {
    row.get(index)
        .ok_or_else(|| format!("missing {name:?} field"))
}

fn parse_required(row: &csv::StringRecord, index: usize, name: &str) -> Result<f64, String> {
    let value = field(row, index, name)?;
    value
        .parse()
        .map_err(|error| format!("could not parse {name} value {value:?}: {error}"))
}

fn parse_optional(
    row: &csv::StringRecord,
    index: usize,
    name: &str,
) -> Result<Option<f64>, String> {
    let value = field(row, index, name)?;
    if value.trim().is_empty() {
        Ok(None)
    } else {
        value
            .parse()
            .map(Some)
            .map_err(|error| format!("could not parse {name} value {value:?}: {error}"))
    }
}

#[allow(clippy::cast_possible_truncation)]
fn propagated_direction(
    ra_hours: f64,
    dec_degrees: f64,
    proper_motion_ra: f64,
    proper_motion_dec: f64,
) -> [f32; 3] {
    let ra = (ra_hours * 15.0).to_radians();
    let dec = dec_degrees.to_radians();
    let direction = [dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin()];
    let east = [-ra.sin(), ra.cos(), 0.0];
    let north = [-dec.sin() * ra.cos(), -dec.sin() * ra.sin(), dec.cos()];
    let mas_to_radians = std::f64::consts::PI / (180.0 * 3_600_000.0);
    let years = BAKED_EPOCH - CATALOG_EPOCH;
    let moved = [
        direction[0]
            + years * mas_to_radians * (proper_motion_ra * east[0] + proper_motion_dec * north[0]),
        direction[1]
            + years * mas_to_radians * (proper_motion_ra * east[1] + proper_motion_dec * north[1]),
        direction[2]
            + years * mas_to_radians * (proper_motion_ra * east[2] + proper_motion_dec * north[2]),
    ];
    let length = (moved[0] * moved[0] + moved[1] * moved[1] + moved[2] * moved[2]).sqrt();
    [
        (moved[0] / length) as f32,
        (moved[1] / length) as f32,
        (moved[2] / length) as f32,
    ]
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn color_from_bv(color_index: f64) -> [u8; 3] {
    let bv = color_index.clamp(-0.4, 2.0);
    let temperature = 4600.0 * (1.0 / (0.92 * bv + 1.7) + 1.0 / (0.92 * bv + 0.62));
    let temperature = temperature / 100.0;
    let red = if temperature <= 66.0 {
        255.0
    } else {
        329.698_727_446 * (temperature - 60.0).powf(-0.133_204_759_2)
    };
    let green = if temperature <= 66.0 {
        99.470_802_586_1 * temperature.ln() - 161.119_568_166_1
    } else {
        288.122_169_528_3 * (temperature - 60.0).powf(-0.075_514_849_2)
    };
    let blue = if temperature >= 66.0 {
        255.0
    } else if temperature <= 19.0 {
        0.0
    } else {
        138.517_731_223_1 * (temperature - 10.0).ln() - 305.044_792_730_7
    };
    [red, green, blue].map(|channel| {
        let desaturated = channel.clamp(0.0, 255.0) * 0.65 + 255.0 * 0.35;
        desaturated.round() as u8
    })
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn encode_magnitude(magnitude: f64) -> u8 {
    (((magnitude.clamp(-2.0, 8.0) + 2.0) / 10.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_coordinates_point_along_eqj_x() {
        let direction = propagated_direction(0.0, 0.0, 0.0, 0.0);
        assert!(
            direction
                .into_iter()
                .zip([1.0, 0.0, 0.0])
                .all(|(actual, expected)| (actual - expected).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn north_pole_points_along_eqj_z() {
        let direction = propagated_direction(0.0, 90.0, 0.0, 0.0);
        assert!((direction[2] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn missing_color_is_white() {
        let headers =
            csv::StringRecord::from(vec!["proper", "ra", "dec", "mag", "ci", "pmra", "pmdec"]);
        let columns = Columns::from_headers(&headers).expect("fixture headers are valid");
        let row = csv::StringRecord::from(vec!["", "0", "0", "1", "", "", ""]);
        let star = parse_star(&row, &columns)
            .expect("fixture star is valid")
            .expect("fixture star is bright enough");
        assert_eq!(star.color, [255; 3]);
    }

    #[test]
    fn magnitude_seven_is_kept() {
        assert_eq!(encode_magnitude(7.0), 230);
    }

    #[test]
    fn sol_is_not_baked_as_a_catalog_star() {
        let headers =
            csv::StringRecord::from(vec!["proper", "ra", "dec", "mag", "ci", "pmra", "pmdec"]);
        let columns = Columns::from_headers(&headers).expect("fixture headers are valid");
        let row = csv::StringRecord::from(vec!["Sol", "0", "0", "-26.7", "0.65", "0", "0"]);
        assert!(
            parse_star(&row, &columns)
                .expect("Sol row parses")
                .is_none()
        );
    }
}
