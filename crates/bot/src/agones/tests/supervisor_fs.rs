use super::*;

fn replaced(content: &str, old: &str, new: &str) -> Option<String> {
    match apply_unique_edit(content, old, new) {
        EditApply::Replaced(text) => Some(text),
        EditApply::NoMatch | EditApply::Ambiguous(_) => None,
    }
}

#[test]
fn replaces_a_single_unique_occurrence() {
    let props = "difficulty=easy\nmax-players=20\n";
    assert_eq!(
        replaced(props, "difficulty=easy", "difficulty=hard").as_deref(),
        Some("difficulty=hard\nmax-players=20\n"),
        "the one matching line is rewritten and the rest is left intact"
    );
}

#[test]
fn missing_anchor_is_no_match() {
    assert!(
        matches!(
            apply_unique_edit(
                "difficulty=easy\n",
                "difficulty=peaceful",
                "difficulty=hard"
            ),
            EditApply::NoMatch
        ),
        "text that isn't present must not be silently appended or ignored"
    );
}

#[test]
fn repeated_anchor_is_ambiguous_with_its_count() {
    let content = "value=1\nother=x\nvalue=1\n";
    assert!(
        matches!(
            apply_unique_edit(content, "value=1", "value=2"),
            EditApply::Ambiguous(2)
        ),
        "an anchor that matches twice is refused, and the count is reported so the caller can disambiguate"
    );
}

#[test]
fn count_is_non_overlapping_left_to_right() {
    // "aa" occurs twice non-overlapping in "aaaa" (not three times), matching the
    // reader's mental model when they copy a snippet to anchor a change.
    assert!(matches!(
        apply_unique_edit("aaaa", "aa", "b"),
        EditApply::Ambiguous(2)
    ));
    assert_eq!(replaced("aaa", "aa", "b").as_deref(), Some("ba"));
}
