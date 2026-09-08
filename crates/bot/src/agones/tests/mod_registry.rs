use super::*;

fn annotations(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn a_game_with_no_annotations_declares_no_registry() {
    assert_eq!(parse_mod_registry(None).unwrap(), None);
    assert_eq!(parse_mod_registry(Some(&annotations(&[]))).unwrap(), None);
}

#[test]
fn modrinth_needs_nothing_else() {
    let map = annotations(&[(MOD_REGISTRY_ANNOTATION, "modrinth")]);
    assert_eq!(
        parse_mod_registry(Some(&map)).unwrap(),
        Some(ModRegistry::Modrinth)
    );
}

#[test]
fn thunderstore_carries_its_community() {
    let map = annotations(&[
        (MOD_REGISTRY_ANNOTATION, "thunderstore"),
        (THUNDERSTORE_COMMUNITY_ANNOTATION, "valheim"),
    ]);
    assert_eq!(
        parse_mod_registry(Some(&map)).unwrap(),
        Some(ModRegistry::Thunderstore {
            community: "valheim".to_owned()
        })
    );
}

#[test]
fn an_unknown_registry_names_the_value_and_the_known_ones() {
    let map = annotations(&[(MOD_REGISTRY_ANNOTATION, "curseforge")]);
    let err = format!("{:#}", parse_mod_registry(Some(&map)).unwrap_err());
    assert!(err.contains("curseforge"), "got: {err}");
    assert!(err.contains("thunderstore"), "got: {err}");
}

#[test]
fn thunderstore_without_a_community_is_an_error() {
    let map = annotations(&[(MOD_REGISTRY_ANNOTATION, "thunderstore")]);
    let err = format!("{:#}", parse_mod_registry(Some(&map)).unwrap_err());
    assert!(
        err.contains(THUNDERSTORE_COMMUNITY_ANNOTATION),
        "got: {err}"
    );
}

#[test]
fn an_empty_community_is_an_error_rather_than_a_search_against_nothing() {
    let map = annotations(&[
        (MOD_REGISTRY_ANNOTATION, "thunderstore"),
        (THUNDERSTORE_COMMUNITY_ANNOTATION, "  "),
    ]);
    assert!(parse_mod_registry(Some(&map)).is_err());
}

#[test]
fn a_community_without_the_registry_is_an_error() {
    let map = annotations(&[(THUNDERSTORE_COMMUNITY_ANNOTATION, "valheim")]);
    let err = format!("{:#}", parse_mod_registry(Some(&map)).unwrap_err());
    assert!(err.contains(MOD_REGISTRY_ANNOTATION), "got: {err}");
}

#[test]
fn a_community_on_a_modrinth_game_is_an_error() {
    let map = annotations(&[
        (MOD_REGISTRY_ANNOTATION, "modrinth"),
        (THUNDERSTORE_COMMUNITY_ANNOTATION, "valheim"),
    ]);
    assert!(parse_mod_registry(Some(&map)).is_err());
}

#[test]
fn a_template_without_metadata_parses_as_no_registry() {
    assert_eq!(
        parse_mod_registry_from_template("apiVersion: agones.dev/v1\nkind: GameServer\n").unwrap(),
        None
    );
}

#[test]
fn a_template_annotation_pair_is_read_off_the_yaml() {
    let yaml = format!(
        "kind: GameServer\nmetadata:\n  annotations:\n    {MOD_REGISTRY_ANNOTATION}: thunderstore\n    {THUNDERSTORE_COMMUNITY_ANNOTATION}: valheim\n"
    );
    assert_eq!(
        parse_mod_registry_from_template(&yaml).unwrap(),
        Some(ModRegistry::Thunderstore {
            community: "valheim".to_owned()
        })
    );
}

#[test]
fn the_lenient_wrapper_degrades_a_bad_pair_to_no_registry() {
    let yaml = format!(
        "kind: GameServer\nmetadata:\n  annotations:\n    {MOD_REGISTRY_ANNOTATION}: thunderstore\n"
    );
    assert!(parse_mod_registry_from_template(&yaml).is_err());
    assert_eq!(mod_registry_or_warn("valheim", &yaml), None);
}

#[test]
fn unparseable_yaml_degrades_rather_than_stopping_the_catalog() {
    assert_eq!(mod_registry_or_warn("broken", "\tnot: yaml: at all"), None);
}
