//! How the cube files are named under the textures directory.
//!
//! The core resolves these names and the xtask bundles and checks them. The two
//! crates share no dependency, so the xtask compiles this file into itself
//! through a `#[path]` attribute, and neither can spell a name the other does
//! not. Nothing here may use anything but `std`.

/// The faces in cube layer order: +X, -X, +Y, -Y, +Z, -Z.
pub const FACES: [&str; 6] = ["px", "nx", "py", "ny", "pz", "nz"];

/// The day faces are shipped for the twelve months of 2004.
pub const MONTHS: usize = 12;

/// The year of the month directories, `day/2004MM`.
pub const YEAR: u32 = 2004;

pub const NIGHT_SET: &str = "night";

pub const MASK_SET: &str = "mask";

/// The directory of a month's day faces, January 0.
pub fn day_set(month: usize) -> String {
    format!("day/{YEAR}{:02}", month + 1)
}

/// A face's file within its set.
pub fn face_file(face: &str) -> String {
    format!("{face}.jxl")
}

/// Every cube file relative to the textures directory, with `/` between the
/// parts: the months in order, then the night and the mask, each set's faces
/// in layer order.
pub fn all_files() -> Vec<String> {
    (0..MONTHS)
        .map(day_set)
        .chain([NIGHT_SET.to_owned(), MASK_SET.to_owned()])
        .flat_map(|set| FACES.map(|face| format!("{set}/{}", face_file(face))))
        .collect()
}
