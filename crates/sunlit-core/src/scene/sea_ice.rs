//! Which two monthly means of the sea ice the date falls between, the ice's
//! color for it, and where a point of the globe falls on the polar caps.
//!
//! Each monthly mean of the climatology stands for the middle of its month,
//! the instant halfway between its first and its last moment, and a date
//! between two such anchors is drawn as a linear mix of the two. This is not
//! the land's month in force (`scene::month`), which is cut at mid-month: the
//! ice follows its data, whose monthly mean is centered on the month.

use glam::Vec3;

use super::month::{CivilTime, SECONDS_PER_DAY, days_in_month, seconds_into_month};
use super::sun::DateTimeInput;

/// Half the side of a cap in sphere radii, `2 sin(27.5 degrees)`: a cap
/// reaches 35 degrees of latitude on its axes.
pub const RHO_MAX: f32 = 0.923_497;

/// The two monthly means a date is drawn between, January 0, and how far it
/// is from the first to the second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IceBlend {
    pub month_a: usize,
    pub month_b: usize,
    /// 0 at the middle of `month_a`, 1 at the middle of `month_b`.
    pub weight: f32,
}

impl IceBlend {
    /// The blend as the dirty check compares it: the weight to 1/256.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the weight is between 0 and 1, so the product fits a u16"
    )]
    pub fn quantized(self) -> [u16; 3] {
        [
            u16::try_from(self.month_a).unwrap_or(u16::MAX),
            u16::try_from(self.month_b).unwrap_or(u16::MAX),
            (self.weight.clamp(0.0, 1.0) * 256.0).round() as u16,
        ]
    }

    /// Both months in one word, `month_a` in the low 16 bits.
    #[must_use]
    pub fn packed_months(self) -> u32 {
        let low = u32::try_from(self.month_a).unwrap_or(0);
        let high = u32::try_from(self.month_b).unwrap_or(0);
        low | (high << 16)
    }
}

/// The blend for the scene's date: the custom date when it is in use, the live
/// clock otherwise.
#[must_use]
pub fn ice_blend(dt: &DateTimeInput, now_utc: time::OffsetDateTime) -> IceBlend {
    ice_blend_at(&CivilTime::of(dt, now_utc))
}

/// Half the length of calendar month `month` (1 to 12) of `year`, in seconds.
fn half_month(year: i32, month: u8) -> f64 {
    f64::from(days_in_month(year, month)) * SECONDS_PER_DAY / 2.0
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a weight between 0 and 1 loses nothing that matters in f32"
)]
pub(crate) fn ice_blend_at(at: &CivilTime) -> IceBlend {
    let month = at.month.clamp(1, 12);
    let into = seconds_into_month(at);
    let half = half_month(at.year, month);
    let this = usize::from(month - 1);
    let (previous_year, previous) = if month == 1 {
        (at.year - 1, 12)
    } else {
        (at.year, month - 1)
    };
    let (next_year, next) = if month == 12 {
        (at.year + 1, 1)
    } else {
        (at.year, month + 1)
    };
    let (month_a, since, span) = if into >= half {
        (this, into - half, half + half_month(next_year, next))
    } else {
        let before = half_month(previous_year, previous);
        ((this + 11) % 12, into + before, before + half)
    };
    IceBlend {
        month_a,
        month_b: (month_a + 1) % 12,
        weight: (since / span).clamp(0.0, 1.0) as f32,
    }
}

/// The color of the sea ice in each month, January first, before the gamma
/// and saturation the day surface gets: the north's and the south's.
///
/// A look, chosen by eye against true-color images of each month
/// (`docs/rendering.md`). The Arctic's goes toward a blue grey in summer, for
/// melt ponds and bare ice; the Antarctic's stays snow covered and hardly
/// changes.
const ICE_COLORS: [[[u8; 3]; 12]; 2] = [
    [
        [224, 229, 236],
        [224, 229, 236],
        [224, 229, 236],
        [224, 229, 236],
        [218, 223, 230],
        [206, 211, 218],
        [194, 199, 206],
        [192, 197, 204],
        [204, 209, 216],
        [216, 221, 228],
        [222, 227, 234],
        [224, 229, 236],
    ],
    [
        [210, 215, 220],
        [210, 215, 220],
        [216, 221, 226],
        [226, 231, 235],
        [226, 231, 235],
        [226, 231, 235],
        [226, 231, 235],
        [226, 231, 235],
        [226, 231, 235],
        [226, 231, 235],
        [222, 227, 231],
        [214, 219, 223],
    ],
];

/// The ice color of one hemisphere for a blend, RGBA8 with alpha 255:
/// `south` false the north's.
#[must_use]
pub fn ice_color(blend: IceBlend, south: bool) -> [u8; 4] {
    let table = &ICE_COLORS[usize::from(south)];
    let (a, b) = (table[blend.month_a % 12], table[blend.month_b % 12]);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a mix of two bytes, rounded, is a byte"
    )]
    let mix = |x: u8, y: u8| {
        let value = f32::from(x) + (f32::from(y) - f32::from(x)) * blend.weight;
        value.round().clamp(0.0, 255.0) as u8
    };
    [mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2]), 255]
}

/// Where a unit normal falls on the caps: the cap, 0 the north's and 1 the
/// south's, and its coordinates 0 to 1 across the cap, along a row and then
/// down a column, as `sea_ice_cap` in `sphere.wgsl` computes them.
#[must_use]
pub fn cap_coordinates(n: Vec3) -> (u32, [f32; 2]) {
    let south = n.y < 0.0;
    let k = (2.0 / (1.0 + n.y.abs())).sqrt();
    let x = k * n.x;
    let y = if south { k * n.z } else { -k * n.z };
    (
        u32::from(south),
        [0.5 + x / (2.0 * RHO_MAX), 0.5 - y / (2.0 * RHO_MAX)],
    )
}

#[cfg(test)]
mod tests {
    use approx::assert_relative_eq;

    use super::*;

    fn at(year: i32, month: u8, day: u8, hour: i32) -> CivilTime {
        CivilTime {
            year,
            month,
            day,
            hour,
            minute: 0,
            second: 0.0,
        }
    }

    /// The instant halfway through `month` of `year`.
    fn middle(year: i32, month: u8) -> CivilTime {
        let half = half_month(year, month);
        let day = (half / SECONDS_PER_DAY).floor();
        let hour = (half - day * SECONDS_PER_DAY) / 3600.0;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a day of a month and an hour of a day"
        )]
        at(year, month, day as u8 + 1, hour as i32)
    }

    #[test]
    fn each_mean_is_drawn_alone_at_the_middle_of_its_month() {
        for year in [2025, 2028] {
            for month in 1..=12_u8 {
                let blend = ice_blend_at(&middle(year, month));
                assert_eq!(blend.month_a, usize::from(month - 1), "{year}-{month}");
                assert_eq!(blend.month_b, usize::from(month % 12), "{year}-{month}");
                assert_relative_eq!(blend.weight, 0.0);
            }
        }
    }

    /// The first instant of a month is past the previous anchor by half the
    /// previous month, of a span of half of each.
    #[test]
    fn the_first_of_a_month_is_between_its_neighbours_anchors() {
        let blend = ice_blend_at(&at(2025, 3, 1, 0));
        assert_eq!((blend.month_a, blend.month_b), (1, 2));
        assert_relative_eq!(blend.weight, 14.0 / (14.0 + 15.5), epsilon = 1e-6);
    }

    /// The weight runs from 0 to 1 between two anchors without a step, which
    /// is what the turn of a month and of the year must not break: the hour
    /// before an anchor and the anchor itself draw the same picture.
    #[test]
    fn the_blend_is_continuous_through_every_month_and_the_turn_of_the_year() {
        for year in [2025, 2028] {
            let days = if crate::scene::datetime::is_leap_year(year) {
                366
            } else {
                365
            };
            let mut previous: Option<(usize, f32)> = None;
            for hour in 0..days * 24 {
                let (month, day) = crate::scene::datetime::day_of_year_to_month_day(
                    u16::try_from(hour / 24 + 1).expect("a day of the year"),
                    year,
                );
                let blend = ice_blend_at(&at(year, month, day, hour % 24));
                let drawn = |m: usize| {
                    if m == blend.month_a {
                        1.0 - blend.weight
                    } else if m == blend.month_b {
                        blend.weight
                    } else {
                        0.0
                    }
                };
                if let Some((m, share)) = previous {
                    assert!(
                        (drawn(m) - share).abs() < 0.003,
                        "{year}, hour {hour}: month {m} went from {share} to {}",
                        drawn(m)
                    );
                }
                previous = Some((blend.month_a, drawn(blend.month_a)));
            }
        }
        let december = ice_blend_at(&at(2025, 12, 31, 23));
        let january = ice_blend_at(&at(2026, 1, 1, 0));
        assert_eq!((december.month_a, december.month_b), (11, 0));
        assert_eq!((january.month_a, january.month_b), (11, 0));
        assert!((january.weight - december.weight).abs() < 0.003);
    }

    /// A leap year's February is a day longer, which moves its anchor half a
    /// day later and lengthens both spans around it.
    #[test]
    fn a_leap_february_moves_its_anchor() {
        let common = ice_blend_at(&at(2025, 2, 15, 0));
        let leap = ice_blend_at(&at(2028, 2, 15, 0));
        assert_eq!(common.month_a, 1, "the 15th is a common February's middle");
        assert_relative_eq!(common.weight, 0.0);
        assert_eq!(leap.month_a, 0, "and before a leap February's");
        assert_relative_eq!(leap.weight, 29.5 / 30.0, epsilon = 1e-6);
    }

    #[test]
    fn the_quantized_blend_tells_a_256th_apart() {
        let blend = |weight| IceBlend {
            month_a: 2,
            month_b: 3,
            weight,
        };
        assert_eq!(blend(0.5).quantized(), blend(0.5005).quantized());
        assert_ne!(blend(0.5).quantized(), blend(0.505).quantized());
        assert_eq!(blend(0.0).packed_months(), 2 | (3 << 16));
    }

    #[test]
    fn the_color_is_blended_between_the_months_like_the_concentration() {
        let at_a = IceBlend {
            month_a: 5,
            month_b: 6,
            weight: 0.0,
        };
        let at_b = IceBlend {
            weight: 1.0,
            ..at_a
        };
        let between = IceBlend {
            weight: 0.5,
            ..at_a
        };
        for south in [false, true] {
            let (a, b, m) = (
                ice_color(at_a, south),
                ice_color(at_b, south),
                ice_color(between, south),
            );
            for channel in 0..3 {
                let (low, high) = (a[channel].min(b[channel]), a[channel].max(b[channel]));
                assert!(low <= m[channel] && m[channel] <= high);
            }
            assert_eq!(m[3], 255);
        }
    }

    /// The points the texture pipeline pins its own projection with: a
    /// latitude and a longitude, and where the pipeline puts them on the caps.
    const POINTS: &str = include_str!("../../tests/fixtures/sea_ice_points.csv");

    #[test]
    fn the_projection_puts_the_pipelines_points_where_the_pipeline_does() {
        let mut checked = 0;
        for line in POINTS.lines().skip(1).filter(|line| !line.is_empty()) {
            let fields: Vec<f64> = line
                .split(',')
                .map(|field| field.trim().parse().expect("a number"))
                .collect();
            let [latitude, longitude, cap, u, v, ..] = fields[..] else {
                panic!("at least five fields in {line}");
            };
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the cap is written as 0 or 1"
            )]
            let cap = cap as u32;
            let (phi, lambda) = (latitude.to_radians(), longitude.to_radians());
            #[expect(clippy::cast_possible_truncation, reason = "a unit vector")]
            let n = Vec3::new(
                (phi.cos() * lambda.sin()) as f32,
                phi.sin() as f32,
                (phi.cos() * lambda.cos()) as f32,
            );
            let (found_cap, [found_u, found_v]) = cap_coordinates(n);
            assert_eq!(found_cap, cap, "{line}");
            assert_relative_eq!(f64::from(found_u), u, epsilon = 2e-6);
            assert_relative_eq!(f64::from(found_v), v, epsilon = 2e-6);
            checked += 1;
        }
        assert!(checked >= 8, "only {checked} points");
    }

    #[test]
    fn rho_max_is_the_cap_the_pipeline_bakes() {
        assert_relative_eq!(
            f64::from(RHO_MAX),
            2.0 * 27.5_f64.to_radians().sin(),
            epsilon = 1e-6
        );
    }
}
