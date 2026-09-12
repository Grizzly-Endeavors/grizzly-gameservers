use super::*;

/// A registry answer for each named package at the given version.
fn registry(entries: &[(&str, &str)]) -> BTreeMap<String, Latest> {
    entries
        .iter()
        .map(|(name, version)| {
            (
                (*name).to_owned(),
                Latest::Known {
                    version: version.parse().unwrap(),
                    deprecated: false,
                },
            )
        })
        .collect()
}

#[test]
fn only_pinned_entries_need_a_lookup() {
    // An unpinned line and a comment are decided on facts already in hand, so
    // spending a round trip on either would be spending it on nothing.
    let text = "# the group's pack\n\
                ValheimModding-Jotunn-2.30.0\n\
                Smoothbrain-Backpacks\n\
                client:Advize-PlantEasily-2.2.0\n\
                \n\
                not a package\n";

    let wanted: Vec<String> = lookups_needed(text)
        .iter()
        .map(ToString::to_string)
        .collect();

    assert_eq!(wanted, vec!["Advize-PlantEasily", "ValheimModding-Jotunn"]);
}

#[test]
fn a_package_listed_twice_is_looked_up_once() {
    let text = "ValheimModding-Jotunn-2.30.0\nValheimModding-Jotunn-2.29.0\n";
    assert_eq!(lookups_needed(text).len(), 1);
}

#[test]
fn a_pin_behind_the_registry_is_raised() {
    let plan = plan_update(
        "Smoothbrain-Backpacks-1.3.8\n",
        &registry(&[("Smoothbrain-Backpacks", "1.4.0")]),
    );

    assert_eq!(plan.bumped, vec!["Smoothbrain-Backpacks 1.3.8 -> 1.4.0"]);
    assert_eq!(plan.text, "Smoothbrain-Backpacks-1.4.0\n");
    assert_eq!(plan.bumps(), 1);
    assert!(!plan.is_empty());
}

#[test]
fn a_pin_already_newest_is_left_alone() {
    let plan = plan_update(
        "Smoothbrain-Backpacks-1.4.0\n",
        &registry(&[("Smoothbrain-Backpacks", "1.4.0")]),
    );

    assert!(plan.is_empty());
    assert_eq!(plan.current, 1);
    assert_eq!(plan.text, "Smoothbrain-Backpacks-1.4.0\n");
}

#[test]
fn a_pin_above_the_registry_is_never_rolled_back() {
    // "Latest" is the only claim the registry makes. Lowering a pin would take a
    // server off a version it is running, under the name of an update.
    let plan = plan_update(
        "Smoothbrain-Backpacks-1.5.0\n",
        &registry(&[("Smoothbrain-Backpacks", "1.4.0")]),
    );

    assert!(plan.is_empty());
    assert_eq!(plan.text, "Smoothbrain-Backpacks-1.5.0\n");
}

#[test]
fn versions_are_compared_numerically() {
    // A string comparison would read 1.10.0 as older than 1.9.0 and refuse the
    // bump — or, worse, offer it in reverse.
    let plan = plan_update(
        "Smoothbrain-Backpacks-1.9.0\n",
        &registry(&[("Smoothbrain-Backpacks", "1.10.0")]),
    );

    assert_eq!(plan.bumped, vec!["Smoothbrain-Backpacks 1.9.0 -> 1.10.0"]);
}

#[test]
fn an_unpinned_entry_is_reported_rather_than_pinned() {
    // It already installs the latest at every start. Pinning it would remove
    // that behaviour while claiming to have updated it.
    let plan = plan_update(
        "Smoothbrain-Backpacks\n",
        &registry(&[("Smoothbrain-Backpacks", "1.4.0")]),
    );

    assert!(plan.is_empty());
    assert_eq!(plan.floating, vec!["Smoothbrain-Backpacks"]);
    assert_eq!(plan.text, "Smoothbrain-Backpacks\n");
}

#[test]
fn a_players_machines_pin_is_bumped_on_its_own_line() {
    // Reported separately because it is the only part of an update that creates
    // work outside the server.
    let plan = plan_update(
        "client:Advize-PlantEasily-2.1.0\n",
        &registry(&[("Advize-PlantEasily", "2.2.0")]),
    );

    assert_eq!(
        plan.client_bumped,
        vec!["Advize-PlantEasily 2.1.0 -> 2.2.0"]
    );
    assert!(plan.bumped.is_empty());
    assert_eq!(plan.text, "client:Advize-PlantEasily-2.2.0\n");
}

#[test]
fn comments_blank_lines_and_order_survive_the_rewrite() {
    // The list is a file somebody authored. Re-rendering it from a parsed set
    // would silently discard all of this.
    let text = "# the group's pack, agreed 2026-09\n\
                \n\
                mtnewton-ItemStacks-1.2.0\n\
                Akoozie-ValheimTune-0.7.1\n\
                \n\
                # players install these themselves\n\
                client:Advize-PlantEasily-2.2.0\n";
    let plan = plan_update(
        text,
        &registry(&[
            ("mtnewton-ItemStacks", "1.3.0"),
            ("Akoozie-ValheimTune", "0.7.1"),
            ("Advize-PlantEasily", "2.2.0"),
        ]),
    );

    assert_eq!(
        plan.text,
        "# the group's pack, agreed 2026-09\n\
         \n\
         mtnewton-ItemStacks-1.3.0\n\
         Akoozie-ValheimTune-0.7.1\n\
         \n\
         # players install these themselves\n\
         client:Advize-PlantEasily-2.2.0\n"
    );
    assert_eq!(plan.bumps(), 1);
    assert_eq!(plan.current, 2);
}

#[test]
fn a_line_nothing_understands_is_carried_through_untouched() {
    let plan = plan_update(
        "definitely not a package\nSmoothbrain-Backpacks-1.3.8\nclient:no-version\n",
        &registry(&[("Smoothbrain-Backpacks", "1.4.0")]),
    );

    assert_eq!(
        plan.text,
        "definitely not a package\nSmoothbrain-Backpacks-1.4.0\nclient:no-version\n"
    );
}

#[test]
fn a_package_the_registry_does_not_have_keeps_its_line() {
    // The list may be older than a rename. Rewriting a line whose package can't
    // be confirmed is how an update breaks a launch.
    let mut latest = registry(&[("Smoothbrain-Backpacks", "1.4.0")]);
    latest.insert("Author-Renamed".to_owned(), Latest::Missing);
    let plan = plan_update(
        "Author-Renamed-1.0.0\nSmoothbrain-Backpacks-1.3.8\n",
        &latest,
    );

    assert_eq!(plan.unknown, vec!["Author-Renamed"]);
    assert!(plan.text.contains("Author-Renamed-1.0.0"));
    assert_eq!(plan.bumped, vec!["Smoothbrain-Backpacks 1.3.8 -> 1.4.0"]);
}

#[test]
fn a_package_with_no_lookup_at_all_is_treated_as_missing() {
    let plan = plan_update("Author-Unasked-1.0.0\n", &BTreeMap::new());

    assert_eq!(plan.unknown, vec!["Author-Unasked"]);
    assert_eq!(plan.text, "Author-Unasked-1.0.0\n");
}

#[test]
fn a_deprecated_package_is_reported_whether_or_not_its_pin_moves() {
    // "Its author has stopped maintaining this" is the same news at either
    // version, and this is the only turn that will mention it.
    let latest: BTreeMap<String, Latest> = [
        (
            "Author-Abandoned".to_owned(),
            Latest::Known {
                version: "2.0.0".parse().unwrap(),
                deprecated: true,
            },
        ),
        (
            "Author-AbandonedAtLatest".to_owned(),
            Latest::Known {
                version: "1.0.0".parse().unwrap(),
                deprecated: true,
            },
        ),
    ]
    .into_iter()
    .collect();
    let plan = plan_update(
        "Author-Abandoned-1.0.0\nAuthor-AbandonedAtLatest-1.0.0\n",
        &latest,
    );

    assert_eq!(
        plan.deprecated,
        vec!["Author-Abandoned", "Author-AbandonedAtLatest"]
    );
    assert_eq!(plan.bumps(), 1);
}

#[test]
fn an_empty_list_plans_nothing() {
    let plan = plan_update("", &BTreeMap::new());

    assert!(plan.is_empty());
    assert_eq!(plan.text, "");
    assert_eq!(plan.current, 0);
}

#[test]
fn a_list_with_no_trailing_newline_gains_one() {
    // The rewrite is line-based, so it normalises the file's last line ending.
    // Worth pinning: the supervisor tolerates either, and a diff that showed a
    // phantom change on the last line would read as a bug.
    let plan = plan_update(
        "Smoothbrain-Backpacks-1.3.8",
        &registry(&[("Smoothbrain-Backpacks", "1.4.0")]),
    );

    assert_eq!(plan.text, "Smoothbrain-Backpacks-1.4.0\n");
}
