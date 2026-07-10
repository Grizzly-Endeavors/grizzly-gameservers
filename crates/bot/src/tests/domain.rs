use super::*;

#[test]
fn new_accepts_both_borrowed_and_owned_strings() {
    assert_eq!(GuildId::new("998877").as_str(), "998877");
    assert_eq!(InstanceName::new("nether".to_owned()).as_str(), "nether");
    assert_eq!(GameId::new("minecraft").as_str(), "minecraft");
}

#[test]
fn into_string_returns_the_wrapped_value() {
    assert_eq!(GuildId::new("42").into_string(), "42".to_owned());
    assert_eq!(InstanceName::new("survival").into_string(), "survival");
}

#[test]
fn display_renders_the_bare_string() {
    assert_eq!(format!("{}", GameId::new("valheim")), "valheim");
    assert_eq!(InstanceName::new("nether").to_string(), "nether");
}

#[test]
fn serde_is_transparent_so_the_manifest_wire_format_is_unchanged() {
    let guild = GuildId::new("998877");
    let json = serde_json::to_string(&guild).unwrap();
    assert_eq!(
        json, "\"998877\"",
        "serializes as a bare string, not an object"
    );

    let parsed: GuildId = serde_json::from_str("\"998877\"").unwrap();
    assert_eq!(parsed, guild, "deserializes straight from a bare string");
}

#[test]
fn distinct_ids_are_separate_types() {
    // A GuildId and a GameId holding the same text are still different types —
    // this is what makes a transposition a compile error rather than a data bug.
    // (The compile-time guarantee is the point; this asserts the value semantics.)
    assert_eq!(GuildId::new("x").as_str(), GameId::new("x").as_str());
}
