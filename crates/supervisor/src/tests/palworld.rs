use super::*;

#[test]
fn fresh_pvc_gets_a_minimal_valid_ini() {
    let out = ensure_rcon_settings(None, 25575, "abc123");
    assert!(
        out.contains(SECTION_HEADER),
        "section header present: {out}"
    );
    assert!(out.contains("RCONEnabled=True"), "rcon enabled: {out}");
    assert!(out.contains("RCONPort=25575"), "rcon port set: {out}");
    assert!(
        out.contains("AdminPassword=\"abc123\""),
        "admin password quoted: {out}"
    );
}

#[test]
fn preserves_join_password_while_enabling_rcon() {
    // The exact bug Gary hit: ServerPassword edited on the PVC must survive, while
    // the supervisor takes RCON from off/blank to on with the minted password.
    let existing = "[/Script/Pal.PalGameWorldSettings]\n\
         OptionSettings=(ServerPassword=\"fish\",AdminPassword=\"\",RCONEnabled=False,RCONPort=8888)\n";
    let out = ensure_rcon_settings(Some(existing), 25575, "secret");
    assert!(
        out.contains("ServerPassword=\"fish\""),
        "join password preserved: {out}"
    );
    assert!(out.contains("RCONEnabled=True"), "rcon enabled: {out}");
    assert!(out.contains("RCONPort=25575"), "rcon port updated: {out}");
    assert!(
        out.contains("AdminPassword=\"secret\""),
        "admin password set: {out}"
    );
    assert!(
        !out.contains("RCONEnabled=False"),
        "stale value gone: {out}"
    );
    assert!(!out.contains("RCONPort=8888"), "stale port gone: {out}");
    assert_eq!(
        out.matches("AdminPassword=").count(),
        1,
        "no duplicate AdminPassword key: {out}"
    );
}

#[test]
fn commas_inside_quoted_values_are_not_split() {
    let existing = "OptionSettings=(ServerDescription=\"hello, world\",AdminPassword=\"\")\n";
    let out = ensure_rcon_settings(Some(existing), 25575, "pw");
    assert!(
        out.contains("ServerDescription=\"hello, world\""),
        "a comma inside quotes must stay in its value: {out}"
    );
}

#[test]
fn non_option_settings_lines_are_untouched() {
    let existing = "[/Script/Pal.PalGameWorldSettings]\n\
         ; hand-written note\n\
         OptionSettings=(Foo=Bar)\n\
         [OtherSection]\n\
         Extra=1\n";
    let out = ensure_rcon_settings(Some(existing), 25575, "pw");
    assert!(out.contains("; hand-written note"), "comment kept: {out}");
    assert!(out.contains("[OtherSection]"), "other section kept: {out}");
    assert!(out.contains("Extra=1"), "other key kept: {out}");
    assert!(out.contains("Foo=Bar"), "existing option kept: {out}");
}

#[test]
fn existing_keys_are_updated_in_place_and_new_ones_appended() {
    let existing = "OptionSettings=(RCONPort=1,ServerName=\"x\")\n";
    let out = ensure_rcon_settings(Some(existing), 25575, "pw");
    let port_at = out.find("RCONPort=25575").expect("port present");
    let name_at = out.find("ServerName=").expect("name present");
    let enabled_at = out.find("RCONEnabled=True").expect("enabled present");
    assert!(
        port_at < name_at,
        "an existing key keeps its position: {out}"
    );
    assert!(
        enabled_at > name_at,
        "a new key is appended after existing ones: {out}"
    );
}

#[test]
fn seeding_twice_is_a_fixed_point() {
    let once = ensure_rcon_settings(Some("OptionSettings=(Foo=Bar)\n"), 25575, "pw");
    let twice = ensure_rcon_settings(Some(&once), 25575, "pw");
    assert_eq!(
        once, twice,
        "re-seeding an already-seeded ini changes nothing"
    );
}
