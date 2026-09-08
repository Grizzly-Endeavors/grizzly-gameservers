use super::*;

fn package(json: &str) -> Package {
    serde_json::from_str(json).expect("the fixture parses")
}

/// Trimmed from a real `GET /api/experimental/package/ValheimModding/Jotunn/`
/// response — the fields this crate models, in the spelling the API uses.
const JOTUNN: &str = r#"{
  "namespace": "ValheimModding",
  "name": "Jotunn",
  "full_name": "ValheimModding-Jotunn",
  "is_deprecated": false,
  "latest": {
    "version_number": "2.28.0",
    "description": "Valheim library",
    "dependencies": ["denikson-BepInExPack_Valheim-5.4.2333"],
    "downloads": 1208923
  },
  "community_listings": [
    {"community": "valheim", "categories": ["Libraries", "Server-side", "Client-side"]},
    {"community": "other-game", "categories": ["Mods"]}
  ]
}"#;

#[test]
fn a_package_renders_the_name_and_the_string_to_pin() {
    let package = package(JOTUNN);
    assert_eq!(package.full_name(), "ValheimModding-Jotunn");
    assert_eq!(package.pinned_name(), "ValheimModding-Jotunn-2.28.0");
}

#[test]
fn side_support_reads_the_named_community_only() {
    let package = package(JOTUNN);
    assert_eq!(package.side_support("valheim"), SideSupport::Both);
    assert_eq!(package.side_support("other-game"), SideSupport::Unknown);
    assert_eq!(
        package.side_support("not-listed-here"),
        SideSupport::Unknown,
        "another community's categories describe a different game"
    );
}

#[test]
fn dependencies_are_shown_whole_when_they_fit() {
    let package = package(JOTUNN);
    assert_eq!(
        package.shown_dependencies(),
        vec!["denikson-BepInExPack_Valheim-5.4.2333"]
    );
    assert!(!package.dependencies_truncated());
}

#[test]
fn a_modpacks_dependency_list_is_cut_and_says_so() {
    let dependencies: Vec<String> = (0..20)
        .map(|index| format!("Author-Mod{index}-1.0.0"))
        .collect();
    let json = serde_json::json!({
        "namespace": "Author",
        "name": "Pack",
        "latest": {"version_number": "1.0.0", "dependencies": dependencies},
        "community_listings": []
    });
    let package: Package = serde_json::from_value(json).unwrap();
    assert_eq!(package.shown_dependencies().len(), DEPENDENCIES_SHOWN);
    assert!(package.dependencies_truncated());
}

#[test]
fn a_listing_pairs_its_namespace_and_name() {
    let listing: Listing = serde_json::from_str(
        r#"{"namespace":"Mydayyy","name":"ServerSideMap","description":"a map",
            "download_count":114094,
            "categories":[{"id":"29","name":"Server-side","slug":"server-side"}],
            "is_deprecated":false,"is_nsfw":false,
            "last_updated":"2025-03-12T00:31:53Z"}"#,
    )
    .unwrap();
    assert_eq!(listing.full_name(), "Mydayyy-ServerSideMap");
    assert_eq!(listing.side_support(), SideSupport::ServerOnly);
    assert_eq!(listing.updated_day(), "2025-03-12");
}

#[test]
fn a_missing_timestamp_reads_as_unknown_rather_than_blank() {
    let listing: Listing =
        serde_json::from_str(r#"{"namespace":"A","name":"B","description":"","download_count":0}"#)
            .unwrap();
    assert_eq!(listing.updated_day(), "unknown");
}

#[test]
fn a_full_name_splits_into_its_two_components() {
    assert_eq!(
        split_full_name("ValheimModding-Jotunn"),
        Some(("ValheimModding", "Jotunn"))
    );
    assert_eq!(
        split_full_name("denikson-BepInExPack_Valheim"),
        Some(("denikson", "BepInExPack_Valheim"))
    );
}

#[test]
fn a_modrinth_style_slug_is_rejected_rather_than_looked_up() {
    // A bare slug would become a 404 that reads as "no such mod" instead of
    // "that isn't how this registry names things".
    assert_eq!(split_full_name("terralith"), None);
    assert_eq!(split_full_name("-Jotunn"), None);
    assert_eq!(split_full_name("ValheimModding-"), None);
    assert_eq!(split_full_name("Author-Mod-1.0.0"), None);
}
