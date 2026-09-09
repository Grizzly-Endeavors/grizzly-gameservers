use super::*;

fn ids(entries: &[ModsEntry]) -> Vec<String> {
    entries.iter().map(|entry| entry.id.to_string()).collect()
}

fn client_names(list: &ModsList) -> Vec<String> {
    list.client_only
        .iter()
        .map(|entry| entry.release.to_string())
        .collect()
}

fn warnings(list: &ModsList) -> Vec<String> {
    list.warnings.iter().map(ToString::to_string).collect()
}

#[test]
fn an_empty_list_asks_for_nothing() {
    assert_eq!(parse("").unwrap(), ModsList::default());
    assert_eq!(parse("\n\n   \n").unwrap(), ModsList::default());
}

#[test]
fn comments_and_blank_lines_are_ignored() {
    let list = parse("# my mods\n\nValheimModding-Jotunn\n  # trailing note\n").unwrap();
    assert_eq!(ids(&list.install), vec!["ValheimModding-Jotunn"]);
    let [only] = list.install.as_slice() else {
        panic!("expected one entry, got {list:?}")
    };
    assert_eq!(only.line_number, 3, "the line number is the real one");
}

#[test]
fn a_bare_package_takes_the_latest_and_a_pinned_one_does_not() {
    let list = parse("ValheimModding-Jotunn\nSmoothbrain-Sailing-1.1.8\n").unwrap();
    let [bare, pinned] = list.install.as_slice() else {
        panic!("expected two entries, got {list:?}")
    };
    assert_eq!(bare.pin, None);
    assert_eq!(pinned.pin.unwrap().to_string(), "1.1.8");
}

#[test]
fn surrounding_whitespace_is_forgiven() {
    let list = parse("   Smoothbrain-Sailing-1.1.8   \n").unwrap();
    assert_eq!(ids(&list.install), vec!["Smoothbrain-Sailing"]);
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

// ---- client entries ----

#[test]
fn a_client_line_is_recorded_and_never_installed() {
    let list = parse(
        "ValheimModding-Jotunn-2.29.2\n\
         client:RandyKnapp-EquipmentAndQuickSlots-3.0.2\n",
    )
    .unwrap();
    assert_eq!(ids(&list.install), vec!["ValheimModding-Jotunn"]);
    assert_eq!(
        client_names(&list),
        vec!["RandyKnapp-EquipmentAndQuickSlots-3.0.2"],
        "a client entry belongs to the modpack but not to this server's install set"
    );
    assert!(
        list.warnings.is_empty(),
        "a well-formed client line is clean"
    );
}

#[test]
fn a_client_line_tolerates_a_space_after_the_prefix() {
    let list = parse("client: Smoothbrain-Sailing-1.1.8\n").unwrap();
    assert_eq!(client_names(&list), vec!["Smoothbrain-Sailing-1.1.8"]);
}

#[test]
fn an_unpinned_client_line_is_skipped_rather_than_stopping_the_launch() {
    // Decision 3: a mod this server does not install has no business taking the
    // world down. Nothing downstream resolves a client entry, so an unpinned one
    // could never acquire a version — but that is a warning, not a crash.
    let list = parse("client:Smoothbrain-Sailing\n").unwrap();
    assert!(list.client_only.is_empty());
    let reported = warnings(&list);
    let [warning] = reported.as_slice() else {
        panic!("expected one warning, got {reported:?}")
    };
    assert!(warning.contains("no version"), "got: {warning}");
    assert!(warning.contains("line 1"), "got: {warning}");
}

#[test]
fn a_malformed_client_line_is_skipped_rather_than_stopping_the_launch() {
    let list = parse("ValheimModding-Jotunn\nclient:just some words\n").unwrap();
    assert_eq!(
        ids(&list.install),
        vec!["ValheimModding-Jotunn"],
        "the install half of the file is unaffected"
    );
    let reported = warnings(&list);
    let [warning] = reported.as_slice() else {
        panic!("expected one warning, got {reported:?}")
    };
    assert!(warning.contains("line 2"), "got: {warning}");
}

#[test]
fn a_package_may_appear_once_as_one_form_or_the_other() {
    // The install line is the offender here, and it is the one that would
    // actually install something, so it fails the launch like any other
    // ambiguous install line.
    let err = parse(
        "client:ValheimModding-Jotunn-2.29.2\n\
         ValheimModding-Jotunn-2.29.2\n",
    )
    .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("line 2"), "got: {message}");
    assert!(message.contains("line 1"), "got: {message}");
}

#[test]
fn a_client_line_duplicating_an_install_line_is_skipped_not_fatal() {
    let list = parse(
        "ValheimModding-Jotunn-2.29.2\n\
         client:ValheimModding-Jotunn-2.29.2\n",
    )
    .unwrap();
    assert_eq!(ids(&list.install), vec!["ValheimModding-Jotunn"]);
    assert!(
        list.client_only.is_empty(),
        "the package is already accounted for on the install side"
    );
    assert_eq!(warnings(&list).len(), 1, "{:?}", list.warnings);
}
