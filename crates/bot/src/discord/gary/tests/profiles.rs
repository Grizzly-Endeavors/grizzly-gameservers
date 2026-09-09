use super::*;

use crate::thunderstore::profile::ProfileMod;

fn entry(full_name: &str, version: &str) -> ProfileMod {
    ProfileMod {
        full_name: full_name.to_owned(),
        version: version.to_owned(),
        enabled: true,
    }
}

#[test]
fn an_import_result_accounts_for_every_entry_including_the_dropped_ones() {
    // Each dropped category is a different thing to tell a friend, and a result
    // that hid them would produce "where's my mod" as the next message.
    let plan = ImportPlan {
        install: vec!["ValheimModding-Jotunn-2.29.2".to_owned()],
        client_only: vec!["RandyKnapp-EquipmentAndQuickSlots-3.0.2".to_owned()],
        disabled: vec!["Author-Off-1.0.0".to_owned()],
        provided: vec!["denikson-BepInExPack_Valheim-5.4.2333".to_owned()],
        missing: vec!["Author-Typo-1.0.0".to_owned()],
    };
    let text = format_import("valheim-abc123", ".grizzly/mods", &plan);
    for expected in [
        "valheim-abc123",
        ".grizzly/mods",
        "ValheimModding-Jotunn-2.29.2",
        "RandyKnapp-EquipmentAndQuickSlots-3.0.2",
        "Author-Off-1.0.0",
        "denikson-BepInExPack_Valheim-5.4.2333",
        "Author-Typo-1.0.0",
    ] {
        assert!(text.contains(expected), "{expected} missing from: {text}");
    }
}

#[test]
fn an_empty_category_reads_as_nothing_rather_than_as_a_gap() {
    let plan = ImportPlan {
        install: vec!["ValheimModding-Jotunn-2.29.2".to_owned()],
        ..ImportPlan::default()
    };
    let text = format_import("valheim-abc123", ".grizzly/mods", &plan);
    assert!(text.contains("none listed"), "{text}");
}

#[test]
fn an_export_result_carries_the_code_and_what_it_contains() {
    // Saying what a code contains is the whole guard against it going stale:
    // there is no way to revoke one once it is handed over.
    let mods = vec![entry("ValheimModding-Jotunn", "2.29.2")];
    let text = format_export(
        "valheim-abc123",
        "01a0838e-8316-f62c-48aa-7bdd59d3f8da",
        &mods,
    );
    assert!(
        text.contains("01a0838e-8316-f62c-48aa-7bdd59d3f8da"),
        "{text}"
    );
    assert!(text.contains("ValheimModding-Jotunn-2.29.2"), "{text}");
    assert!(text.contains("valheim-abc123"), "{text}");
}
