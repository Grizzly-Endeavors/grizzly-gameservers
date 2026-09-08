use super::*;

fn ids(entries: &[ModsEntry]) -> Vec<String> {
    entries.iter().map(|entry| entry.id.to_string()).collect()
}

#[test]
fn an_empty_list_asks_for_nothing() {
    assert!(parse("").unwrap().is_empty());
    assert!(parse("\n\n   \n").unwrap().is_empty());
}

#[test]
fn comments_and_blank_lines_are_ignored() {
    let entries = parse("# my mods\n\nValheimModding-Jotunn\n  # trailing note\n").unwrap();
    assert_eq!(ids(&entries), vec!["ValheimModding-Jotunn"]);
    let [only] = entries.as_slice() else {
        panic!("expected one entry, got {entries:?}")
    };
    assert_eq!(only.line_number, 3, "the line number is the real one");
}

#[test]
fn a_bare_package_takes_the_latest_and_a_pinned_one_does_not() {
    let entries = parse("ValheimModding-Jotunn\nSmoothbrain-Sailing-1.1.8\n").unwrap();
    let [bare, pinned] = entries.as_slice() else {
        panic!("expected two entries, got {entries:?}")
    };
    assert_eq!(bare.pin, None);
    assert_eq!(pinned.pin.unwrap().to_string(), "1.1.8");
}

#[test]
fn surrounding_whitespace_is_forgiven() {
    let entries = parse("   Smoothbrain-Sailing-1.1.8   \n").unwrap();
    assert_eq!(ids(&entries), vec!["Smoothbrain-Sailing"]);
}

#[test]
fn a_malformed_line_names_its_line_and_the_shape_expected() {
    // Loud rather than skipped: silently dropping this line would answer "that
    // mod isn't installed" to somebody who asked for it, durably and wrongly.
    let err = parse("ValheimModding-Jotunn\njust some words\n").unwrap_err();
    let message = err.to_string();
    assert!(message.contains("line 2"), "got: {message}");
    assert!(message.contains("just some words"), "got: {message}");
    assert!(message.contains("Namespace-Name-1.2.3"), "got: {message}");
}

#[test]
fn a_modrinth_style_slug_is_refused() {
    assert!(parse("terralith\n").is_err());
}

#[test]
fn a_package_listed_twice_names_both_lines() {
    // Two entries for one package have no defined answer — the second would
    // silently win or silently lose depending on resolution order.
    let err = parse("ValheimModding-Jotunn\nValheimModding-Jotunn-2.28.0\n").unwrap_err();
    let message = err.to_string();
    assert!(message.contains("line 2"), "got: {message}");
    assert!(message.contains("line 1"), "got: {message}");
}

#[test]
fn a_version_that_is_not_three_numbers_is_malformed_not_a_bare_package() {
    // `Author-Mod-1.2` must not quietly read as a package literally named
    // "Mod-1.2" — the component grammar excludes the dash, so it can't.
    assert!(parse("Author-Mod-1.2\n").is_err());
}
