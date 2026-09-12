use super::*;

fn entry(full_name: &str, version: &str, enabled: bool) -> ProfileMod {
    ProfileMod {
        full_name: full_name.to_owned(),
        version: version.to_owned(),
        enabled,
    }
}

fn lookups(pairs: &[(&str, Lookup)]) -> BTreeMap<String, Lookup> {
    pairs
        .iter()
        .map(|(name, lookup)| ((*name).to_owned(), *lookup))
        .collect()
}

/// The verified sample profile, which breaks in every interesting place: a
/// loader pack the image supplies, an untagged library, a client-only mod, a
/// both-sided mod, and one nobody tagged at all.
fn sample() -> Vec<ProfileMod> {
    vec![
        entry("denikson-BepInExPack_Valheim", "5.4.2333", true),
        entry("ValheimModding-Jotunn", "2.29.2", true),
        entry("RandyKnapp-EquipmentAndQuickSlots", "3.0.2", true),
        entry("Advize-PlantEverything", "1.20.0", true),
        entry("OdinPlus-OdinHorse", "1.6.6", true),
    ]
}

fn sample_lookups() -> BTreeMap<String, Lookup> {
    lookups(&[
        ("ValheimModding-Jotunn", Lookup::Known(SideSupport::Unknown)),
        (
            "RandyKnapp-EquipmentAndQuickSlots",
            Lookup::Known(SideSupport::ClientOnly),
        ),
        ("Advize-PlantEverything", Lookup::Known(SideSupport::Both)),
        ("OdinPlus-OdinHorse", Lookup::Known(SideSupport::Unknown)),
    ])
}

const PROVIDED: &str = "denikson-BepInExPack_Valheim";

#[test]
fn the_worked_sample_classifies_every_entry() {
    let plan = plan_import(&sample(), &[PROVIDED.to_owned()], &sample_lookups());
    assert_eq!(
        plan.install,
        vec![
            "Advize-PlantEverything-1.20.0".to_owned(),
            "OdinPlus-OdinHorse-1.6.6".to_owned(),
            "ValheimModding-Jotunn-2.29.2".to_owned(),
        ],
        "an untagged package installs — a missing install is invisible, an extra one is a download"
    );
    assert_eq!(
        plan.client_only,
        vec!["RandyKnapp-EquipmentAndQuickSlots-3.0.2".to_owned()]
    );
    assert_eq!(
        plan.provided,
        vec!["denikson-BepInExPack_Valheim-5.4.2333".to_owned()],
        "the image owns the loader pack"
    );
    assert!(plan.missing.is_empty() && plan.disabled.is_empty());
}

#[test]
fn the_rendered_file_marks_the_client_half_and_nothing_else() {
    let plan = plan_import(&sample(), &[PROVIDED.to_owned()], &sample_lookups());
    assert_eq!(
        plan.render(),
        "Advize-PlantEverything-1.20.0\n\
         OdinPlus-OdinHorse-1.6.6\n\
         ValheimModding-Jotunn-2.29.2\n\
         client:RandyKnapp-EquipmentAndQuickSlots-3.0.2\n"
    );
    assert_eq!(plan.listed(), 4);
}

#[test]
fn a_disabled_entry_is_dropped_before_anything_else_is_asked_about_it() {
    let mods = vec![entry("Author-Mod", "1.0.0", false)];
    let plan = plan_import(&mods, &[], &BTreeMap::new());
    assert_eq!(plan.disabled, vec!["Author-Mod-1.0.0".to_owned()]);
    assert!(
        plan.missing.is_empty(),
        "a mod the player switched off is not a mod the registry lost"
    );
}

#[test]
fn an_entry_the_community_has_no_record_of_is_left_out_and_named() {
    // Writing it anyway would stop the server on its next launch and be rolled
    // back — spending a restart to say what the lookup already said.
    let mods = vec![entry("Author-Typo", "1.0.0", true)];
    let plan = plan_import(&mods, &[], &lookups(&[("Author-Typo", Lookup::Missing)]));
    assert_eq!(plan.missing, vec!["Author-Typo-1.0.0".to_owned()]);
    assert_eq!(plan.render(), "");
}

#[test]
fn only_the_entries_a_classification_can_change_are_looked_up() {
    let needed = lookups_needed(
        &[
            entry("denikson-BepInExPack_Valheim", "5.4.2333", true),
            entry("Author-Off", "1.0.0", false),
            entry("Author-On", "1.0.0", true),
        ],
        &[PROVIDED.to_owned()],
    );
    assert_eq!(
        needed,
        vec!["Author-On".to_owned()],
        "a dropped entry is dropped on facts already in hand"
    );
}

#[test]
fn lookups_are_asked_for_once_per_package() {
    let needed = lookups_needed(
        &[
            entry("Author-Mod", "1.0.0", true),
            entry("Author-Mod", "1.0.0", true),
        ],
        &[],
    );
    assert_eq!(needed.len(), 1);
}

// ---- export ----

fn resolved() -> ResolvedMods {
    ResolvedMods {
        installed: vec![
            "ValheimModding-Jotunn-2.29.2".to_owned(),
            "Advize-PlantEverything-1.20.0".to_owned(),
        ],
        client_only: vec!["RandyKnapp-EquipmentAndQuickSlots-3.0.2".to_owned()],
        provided: vec!["denikson-BepInExPack_Valheim".to_owned()],
    }
}

#[test]
fn an_export_carries_the_whole_pack_without_the_loader() {
    let mods = plan_export(&resolved());
    let names: Vec<&str> = mods.iter().map(|m| m.full_name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "Advize-PlantEverything",
            "RandyKnapp-EquipmentAndQuickSlots",
            "ValheimModding-Jotunn",
        ],
        "the client half is what a player most needs, and the loader arrives on its own"
    );
    assert!(mods.iter().all(|entry| entry.enabled));
}

#[test]
fn an_unreadable_resolved_line_costs_one_entry_not_the_export() {
    let resolved = ResolvedMods {
        installed: vec![
            "ValheimModding-Jotunn-2.29.2".to_owned(),
            "not a release".to_owned(),
            "Author-Mod-1.2".to_owned(),
        ],
        ..ResolvedMods::default()
    };
    let mods = plan_export(&resolved);
    assert_eq!(mods.len(), 1, "{mods:?}");
}

// ---- the cached code ----

#[test]
fn the_cache_round_trips() {
    let cached = CachedProfile {
        code: "01a0838e-8316-f62c-48aa-7bdd59d3f8da".to_owned(),
        entries: fingerprint(&plan_export(&resolved())),
    };
    assert_eq!(CachedProfile::parse(&cached.render()), Some(cached));
}

#[test]
fn a_cached_code_is_reused_for_the_same_set_and_not_for_a_changed_one() {
    let mods = plan_export(&resolved());
    let cached = CachedProfile {
        code: "01a0838e-8316-f62c-48aa-7bdd59d3f8da".to_owned(),
        entries: fingerprint(&mods),
    };
    assert!(cached.describes(&mods));

    // A version moving is a different pack, and the old code still describes the
    // old one — which is exactly why it can't be handed out for this one.
    let mut moved = mods.clone();
    moved.first_mut().unwrap().version = "1.21.0".to_owned();
    assert!(!cached.describes(&moved));

    let mut shorter = mods.clone();
    shorter.pop();
    assert!(!cached.describes(&shorter), "a removal changes the set too");
}

#[test]
fn a_cache_file_that_isnt_one_reads_as_absent() {
    assert_eq!(CachedProfile::parse(""), None);
    assert_eq!(CachedProfile::parse("Author-Mod-1.0.0\n"), None);
    assert_eq!(CachedProfile::parse("code:\n"), None);
    assert_eq!(
        CachedProfile::parse("code: one\ncode: two\n"),
        None,
        "two codes in one file names no code at all"
    );
}

// ---- what an import changes about the list already there ----

/// A plan as an import builds one: pinned releases on both sides.
fn plan_of(install: &[&str], client_only: &[&str]) -> ImportPlan {
    ImportPlan {
        install: install.iter().map(|entry| (*entry).to_owned()).collect(),
        client_only: client_only
            .iter()
            .map(|entry| (*entry).to_owned())
            .collect(),
        ..ImportPlan::default()
    }
}

#[test]
fn a_server_with_no_mod_list_diffs_as_all_new() {
    let diff = diff_import("", &plan_of(&["ValheimModding-Jotunn-2.30.0"], &[]));

    assert_eq!(diff.added, vec!["ValheimModding-Jotunn-2.30.0"]);
    assert!(diff.removed.is_empty());
    assert_eq!(diff.unchanged, 0);
    assert!(!diff.is_empty());
}

#[test]
fn adopting_the_pack_a_server_already_runs_changes_nothing() {
    let current = "ValheimModding-Jotunn-2.30.0\nclient:Advize-PlantEasily-2.2.0\n";
    let diff = diff_import(
        current,
        &plan_of(
            &["ValheimModding-Jotunn-2.30.0"],
            &["Advize-PlantEasily-2.2.0"],
        ),
    );

    assert!(diff.is_empty(), "{diff:?}");
    assert_eq!(diff.unchanged, 2);
}

#[test]
fn a_mod_the_profile_does_not_name_is_reported_as_removed() {
    // The category an import produces by accident, since it replaces rather
    // than merges — and the one a friend never thinks to check.
    let current = "ValheimModding-Jotunn-2.30.0\nblacks7ar-OreMines-1.2.0\n";
    let diff = diff_import(current, &plan_of(&["ValheimModding-Jotunn-2.30.0"], &[]));

    assert_eq!(diff.removed, vec!["blacks7ar-OreMines-1.2.0"]);
    assert!(diff.added.is_empty());
    assert_eq!(diff.unchanged, 1);
}

#[test]
fn a_version_change_names_both_versions() {
    let diff = diff_import(
        "Smoothbrain-Backpacks-1.3.8\n",
        &plan_of(&["Smoothbrain-Backpacks-1.4.0"], &[]),
    );

    assert_eq!(diff.changed, vec!["Smoothbrain-Backpacks 1.3.8 -> 1.4.0"]);
    assert!(diff.added.is_empty() && diff.removed.is_empty());
}

#[test]
fn pinning_a_previously_floating_entry_is_a_change() {
    let diff = diff_import(
        "Smoothbrain-Backpacks\n",
        &plan_of(&["Smoothbrain-Backpacks-1.4.0"], &[]),
    );

    assert_eq!(
        diff.changed,
        vec!["Smoothbrain-Backpacks unpinned -> 1.4.0"]
    );
}

#[test]
fn changing_sides_is_neither_an_addition_nor_a_removal() {
    // Still in the pack, no longer running here. Reporting it as removed would
    // be a wrong answer to the only question the distinction affects.
    let diff = diff_import(
        "Advize-PlantEasily-2.2.0\n",
        &plan_of(&[], &["Advize-PlantEasily-2.2.0"]),
    );

    assert_eq!(
        diff.moved,
        vec!["Advize-PlantEasily: now players' machines only"]
    );
    assert!(diff.added.is_empty() && diff.removed.is_empty());

    let back = diff_import(
        "client:Advize-PlantEasily-2.2.0\n",
        &plan_of(&["Advize-PlantEasily-2.2.0"], &[]),
    );
    assert_eq!(
        back.moved,
        vec!["Advize-PlantEasily: now installed on the server"]
    );
}

#[test]
fn a_side_change_outranks_a_version_change_on_the_same_entry() {
    // Which side it goes on decides whether the server runs it at all; the
    // version is a detail of an entry that has moved.
    let diff = diff_import(
        "Advize-PlantEasily-2.1.0\n",
        &plan_of(&[], &["Advize-PlantEasily-2.2.0"]),
    );

    assert_eq!(diff.moved.len(), 1);
    assert!(diff.changed.is_empty());
}

#[test]
fn comments_and_junk_in_the_old_list_are_not_removals() {
    let current = "# the group's pack\n\nValheimModding-Jotunn-2.30.0\nnot a package\n";
    let diff = diff_import(current, &plan_of(&["ValheimModding-Jotunn-2.30.0"], &[]));

    assert!(diff.is_empty(), "{diff:?}");
}

#[test]
fn the_diff_is_ordered_by_package_rather_than_by_file_position() {
    // So the same change always reads the same way, whatever order the
    // exporting launcher listed the pack in.
    let diff = diff_import(
        "",
        &plan_of(
            &["mtnewton-ItemStacks-1.2.0", "Akoozie-ValheimTune-0.7.1"],
            &[],
        ),
    );

    assert_eq!(
        diff.added,
        vec!["Akoozie-ValheimTune-0.7.1", "mtnewton-ItemStacks-1.2.0"]
    );
}
