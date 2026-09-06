//! How large a frame is drawn, and with how many samples.
//!
//! Policy the settings window and the engine ask for before any GPU object
//! exists: which anti-aliasing options an adapter and a quality tier leave on
//! offer, which sample count a request resolves to, and the granularity a
//! viewport is quantized to.

/// Render dimensions are rounded to this granularity to avoid
/// creating new GPU textures on every pixel change during resize.
const SIZE_GRANULARITY: u32 = 64;

/// Build the anti-aliasing option labels and find the default index
/// (preferring 8x MSAA).
///
/// `max_samples` is the quality tier's cap. Filtering here rather than
/// clamping inside the renderer keeps the combo box honest: it never offers a
/// setting the tier would silently ignore.
pub fn build_aa_options(supported: &[u32], max_samples: u32) -> (Vec<String>, Vec<u32>, i32) {
    let mut labels = vec!["None".to_owned()];
    let mut counts = vec![1];

    for &sc in supported {
        if sc > 1 && sc <= max_samples {
            labels.push(format!("MSAA {sc}\u{d7}"));
            counts.push(sc);
        }
    }

    // Default to 8x if available, otherwise the highest available option
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let default_index = counts
        .iter()
        .position(|&c| c == 8)
        .unwrap_or(counts.len() - 1) as i32;

    (labels, counts, default_index)
}

/// Resolve a requested MSAA sample count against what the adapter supports and
/// what the quality tier allows.
///
/// A sample count the adapter does not support is not a warning inside wgpu, it
/// is a validation error that kills whichever thread creates the texture, so
/// this has to happen before any render target is built. The rule mirrors the
/// combo box: take the highest supported count that is at most the requested
/// one, and if the request is below everything on offer, take the lowest thing
/// on offer instead. `1` is always a valid answer.
pub(crate) fn resolve_sample_count(requested: u32, supported: &[u32], max_samples: u32) -> u32 {
    let allowed: Vec<u32> = supported
        .iter()
        .copied()
        .filter(|&c| c >= 1 && c <= max_samples)
        .collect();
    if allowed.is_empty() {
        return 1;
    }
    allowed
        .iter()
        .copied()
        .filter(|&c| c <= requested)
        .max()
        .or_else(|| allowed.iter().copied().min())
        .unwrap_or(1)
}

/// Quantize width and height down to a multiple of `SIZE_GRANULARITY`, never
/// below one granularity unit in either dimension. Down rather than to the
/// nearest, so a render target above that floor is never larger than what was
/// asked for; below it, one granularity unit is the smallest target there is.
pub(crate) fn quantize_to_granularity(w: u32, h: u32) -> (u32, u32) {
    let qw = (w / SIZE_GRANULARITY).max(1) * SIZE_GRANULARITY;
    let qh = (h / SIZE_GRANULARITY).max(1) * SIZE_GRANULARITY;
    (qw, qh)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // build_aa_options
    // -----------------------------------------------------------------------

    #[test]
    fn aa_options_single_sample() {
        let (labels, counts, default) = build_aa_options(&[1], u32::MAX);
        assert_eq!(labels, ["None"]);
        assert_eq!(counts, [1]);
        assert_eq!(default, 0);
    }

    #[test]
    fn aa_options_full_range() {
        let (labels, counts, default) = build_aa_options(&[1, 2, 4, 8], u32::MAX);
        assert_eq!(
            labels,
            ["None", "MSAA 2\u{d7}", "MSAA 4\u{d7}", "MSAA 8\u{d7}"]
        );
        assert_eq!(counts, [1, 2, 4, 8]);
        assert_eq!(default, 3); // index of 8x
    }

    #[test]
    fn aa_options_no_8x_falls_back_to_highest() {
        let (labels, counts, default) = build_aa_options(&[1, 2, 4], u32::MAX);
        assert_eq!(labels, ["None", "MSAA 2\u{d7}", "MSAA 4\u{d7}"]);
        assert_eq!(counts, [1, 2, 4]);
        assert_eq!(default, 2); // last entry
    }

    #[test]
    fn aa_options_skip_intermediates() {
        let (labels, counts, default) = build_aa_options(&[1, 8], u32::MAX);
        assert_eq!(labels, ["None", "MSAA 8\u{d7}"]);
        assert_eq!(counts, [1, 8]);
        assert_eq!(default, 1); // index of 8x
    }

    #[test]
    fn aa_options_empty_input() {
        let (labels, counts, default) = build_aa_options(&[], u32::MAX);
        assert_eq!(labels, ["None"]);
        assert_eq!(counts, [1]);
        assert_eq!(default, 0);
    }

    #[test]
    fn aa_options_respect_the_tier_cap() {
        let (labels, counts, default) = build_aa_options(&[1, 2, 4, 8], 1);
        assert_eq!(labels, ["None"], "the low tier offers no multisampling");
        assert_eq!(counts, [1]);
        assert_eq!(default, 0);

        let (_, counts, _) = build_aa_options(&[1, 2, 4, 8], 4);
        assert_eq!(counts, [1, 2, 4], "the medium tier stops at 4x");
    }

    // -----------------------------------------------------------------------
    // resolve_sample_count
    // -----------------------------------------------------------------------

    /// What a typical desktop adapter reports for `Rgba8Unorm`.
    const FULL: [u32; 4] = [1, 2, 4, 8];

    #[test]
    fn supported_request_is_honored() {
        assert_eq!(resolve_sample_count(4, &FULL, u32::MAX), 4);
        assert_eq!(resolve_sample_count(8, &FULL, u32::MAX), 8);
        assert_eq!(resolve_sample_count(1, &FULL, u32::MAX), 1);
    }

    #[test]
    fn unsupported_request_falls_back_to_the_next_lower_option() {
        // The config asking for something absurd must not reach wgpu.
        assert_eq!(resolve_sample_count(64, &FULL, u32::MAX), 8);
        assert_eq!(resolve_sample_count(3, &FULL, u32::MAX), 2);
        assert_eq!(resolve_sample_count(0, &FULL, u32::MAX), 1);
    }

    #[test]
    fn adapter_without_8x_never_yields_8x() {
        assert_eq!(resolve_sample_count(8, &[1, 2, 4], u32::MAX), 4);
        assert_eq!(resolve_sample_count(8, &[1], u32::MAX), 1);
    }

    #[test]
    fn tier_cap_applies_on_top_of_adapter_support() {
        assert_eq!(resolve_sample_count(8, &FULL, 1), 1);
        assert_eq!(resolve_sample_count(8, &FULL, 4), 4);
        assert_eq!(resolve_sample_count(2, &FULL, 4), 2);
    }

    #[test]
    fn a_request_below_everything_offered_takes_the_lowest_option() {
        // An adapter that does not list 1x is not something we have seen, but
        // returning 0 or the request unchanged would be a validation error.
        assert_eq!(resolve_sample_count(1, &[4, 8], u32::MAX), 4);
    }

    #[test]
    fn an_empty_or_fully_filtered_list_still_yields_a_valid_count() {
        assert_eq!(resolve_sample_count(8, &[], u32::MAX), 1);
        assert_eq!(resolve_sample_count(8, &[4, 8], 2), 1);
    }

    #[test]
    fn every_resolution_is_actually_supported() {
        for supported in [vec![1], vec![1, 4], FULL.to_vec(), vec![1, 2, 4, 8, 16]] {
            for requested in [0, 1, 2, 3, 4, 7, 8, 16, 64, u32::MAX] {
                for cap in [1, 4, u32::MAX] {
                    let resolved = resolve_sample_count(requested, &supported, cap);
                    assert!(
                        supported.contains(&resolved) || resolved == 1,
                        "resolved {resolved} is not in {supported:?} \
                         (requested {requested}, cap {cap})"
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // quantize_to_granularity
    // -----------------------------------------------------------------------

    /// Each dimension comes back as the largest whole number of granules that
    /// fits in it, and never zero. Stated as the property rather than as
    /// numbers, because the granularity is a constant the renderer may change.
    #[test]
    fn quantization_floors_each_dimension_to_a_whole_granule() {
        let granule = SIZE_GRANULARITY;
        for (width, height) in [
            (0, 0),
            (granule - 1, granule - 1),
            (granule, granule),
            (granule + 1, granule + 1),
            (granule * 2, granule * 2),
            (granule * 2 + 1, granule * 3 + 8),
            (1920, 1080),
        ] {
            let (quantized_width, quantized_height) = quantize_to_granularity(width, height);
            for (requested, quantized) in [(width, quantized_width), (height, quantized_height)] {
                assert_eq!(quantized % granule, 0, "{requested} is not whole granules");
                assert!(quantized >= granule, "{requested} quantized to nothing");
                assert!(
                    quantized <= requested.max(granule),
                    "{requested} quantized up to {quantized}"
                );
                assert!(
                    quantized + granule > requested,
                    "{requested} quantized to {quantized}, a whole granule short"
                );
            }
        }
    }
}
