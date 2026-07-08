use super::*;

#[test]
fn short_labels_pass_through_untouched() {
    let label = "survival · 2026-07-08 14:30";
    assert_eq!(truncate_option_label(label), label);
}

#[test]
fn over_long_labels_are_truncated_within_the_cap() {
    let label = "x".repeat(200);
    let truncated = truncate_option_label(&label);
    assert_eq!(
        truncated.chars().count(),
        MAX_SELECT_LABEL,
        "a truncated label should fill exactly the cap, ellipsis included"
    );
    assert!(
        truncated.ends_with('…'),
        "truncation should be marked with an ellipsis"
    );
}

#[test]
fn truncation_respects_char_boundaries() {
    // Multi-byte chars must not be split mid-codepoint.
    let label = "🐻".repeat(200);
    let truncated = truncate_option_label(&label);
    assert_eq!(truncated.chars().count(), MAX_SELECT_LABEL);
    assert!(truncated.ends_with('…'));
}

#[test]
fn a_label_exactly_at_the_cap_is_untouched() {
    let label = "a".repeat(MAX_SELECT_LABEL);
    assert_eq!(truncate_option_label(&label), label);
}
