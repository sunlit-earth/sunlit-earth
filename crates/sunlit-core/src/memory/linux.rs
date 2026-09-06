//! The Linux arm of `snapshot` and the two `/proc` parsers behind it. The
//! counter table the three arms share is in the parent module's doc.
//!
//! The module is compiled on Linux and in every test build, so the parsing is
//! covered by the unit tests on the development machine rather than only on
//! the Linux runner. Only `snapshot` itself is gated to Linux.

#[cfg(target_os = "linux")]
use std::fs;

#[cfg(target_os = "linux")]
use tracing::warn;

#[cfg(target_os = "linux")]
use super::MemorySnapshot;

/// Linux: `/proc/self/status` for the resident sizes, `/proc/self/smaps_rollup`
/// for the private total. Both are plain text, so this needs no `unsafe` and no
/// binding crate.
#[cfg(target_os = "linux")]
pub fn snapshot() -> Option<MemorySnapshot> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let rss_bytes = parse_status_bytes(&status, "VmRSS")?;
    // VmHWM is the field most likely to be the one a restricted or unusual
    // /proc omits while still reporting VmRSS, and taking the whole snapshot
    // down over it would silence every assertion that reads the other two
    // fields. The current RSS is a true lower bound on the peak, so degrading
    // to it keeps the peak column honest in the direction that matters.
    let peak_rss_bytes = parse_status_bytes(&status, "VmHWM").unwrap_or(rss_bytes);

    // smaps_rollup arrived in Linux 4.14. Where it is missing (or unreadable
    // under a hardened kernel) RSS is the honest stand-in: it is an upper bound
    // on the private total, so the soak test's growth assertion stays valid,
    // it just also counts shared pages. Say so once, because a private column
    // that is quietly measuring something else is worth knowing about when
    // reading the metrics CSV afterwards.
    let private_bytes = fs::read_to_string("/proc/self/smaps_rollup")
        .ok()
        .and_then(|rollup| parse_private_bytes(&rollup))
        .unwrap_or_else(|| {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                warn!(
                    "/proc/self/smaps_rollup is unreadable; reporting VmRSS as private \
                     bytes, which also counts pages shared with other processes"
                );
            });
            rss_bytes
        });

    Some(MemorySnapshot {
        rss_bytes,
        peak_rss_bytes,
        private_bytes,
    })
}

/// Read one `Key:   1234 kB` line out of a `/proc` file and return it in bytes.
///
/// The unit is checked rather than assumed. Every field this reads is
/// documented in kibibytes, so the check should never fire; if a kernel ever
/// reported one of them in anything else, the alternative to failing here is
/// silently multiplying it by 1024.
fn parse_status_bytes(text: &str, key: &str) -> Option<u64> {
    for line in text.lines() {
        let Some(rest) = line.strip_prefix(key) else {
            continue;
        };
        // `VmRSS` must not match a hypothetical `VmRSSFoo`.
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let value: u64 = fields.next()?.parse().ok()?;
        if fields.next()? != "kB" {
            return None;
        }
        return value.checked_mul(1024);
    }
    None
}

/// Sum the private mappings reported by `/proc/self/smaps_rollup`, in bytes.
///
/// Clean plus dirty, which is what the rollup offers as "not shared with
/// anyone else". Swapped-out private pages are not included; a runner that is
/// swapping has bigger problems than this counter.
fn parse_private_bytes(rollup: &str) -> Option<u64> {
    let clean = parse_status_bytes(rollup, "Private_Clean")?;
    let dirty = parse_status_bytes(rollup, "Private_Dirty")?;
    clean.checked_add(dirty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parser_reads_kibibytes_as_bytes() {
        let status = "Name:\tsunlit-earth\nVmHWM:\t  204800 kB\nVmRSS:\t   102400 kB\n";
        assert_eq!(parse_status_bytes(status, "VmRSS"), Some(102_400 * 1024));
        assert_eq!(parse_status_bytes(status, "VmHWM"), Some(204_800 * 1024));
    }

    #[test]
    fn status_parser_returns_none_for_a_missing_key() {
        let status = "VmRSS:\t 100 kB\n";
        assert_eq!(parse_status_bytes(status, "VmSwap"), None);
    }

    #[test]
    fn status_parser_requires_the_whole_key() {
        let status = "VmRSSExtra:\t 999 kB\nVmRSS:\t 100 kB\n";
        assert_eq!(parse_status_bytes(status, "VmRSS"), Some(100 * 1024));
    }

    #[test]
    fn status_parser_requires_the_kilobyte_unit() {
        assert_eq!(parse_status_bytes("Threads:\t 8\n", "Threads"), None);
        assert_eq!(parse_status_bytes("VmRSS:\t 100 MB\n", "VmRSS"), None);
    }

    #[test]
    fn rollup_parser_sums_clean_and_dirty() {
        let rollup = "Rss:\t 4096 kB\nPrivate_Clean:\t 256 kB\nPrivate_Dirty:\t 1024 kB\n";
        assert_eq!(parse_private_bytes(rollup), Some((256 + 1024) * 1024));
    }

    #[test]
    fn rollup_parser_returns_none_when_a_field_is_absent() {
        assert_eq!(parse_private_bytes("Private_Clean:\t 256 kB\n"), None);
    }
}
