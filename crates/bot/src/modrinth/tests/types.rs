use super::*;

/// A real search hit, trimmed to the modelled fields. Shaped after Terralith's
/// actual response, which is the case that proves datapacks arrive as
/// `project_type: "mod"` with their loaders folded into `categories`.
const TERRALITH_HIT: &str = r#"{
    "project_id": "8oi3bsk5",
    "slug": "terralith",
    "title": "Terralith",
    "description": "Terralith adds over 95 brand new biomes.",
    "downloads": 20538127,
    "client_side": "optional",
    "server_side": "required",
    "categories": ["datapack", "fabric", "forge", "neoforge", "quilt", "worldgen"],
    "versions": ["1.18", "1.19", "1.20", "26.1", "26.1.2", "26.2"],
    "latest_version": "EyCqftOK",
    "author": "Starmute"
}"#;

#[test]
fn a_search_hit_parses_and_ignores_unmodelled_fields() {
    let hit: SearchHit = serde_json::from_str(TERRALITH_HIT).unwrap();
    assert_eq!(hit.slug, "terralith");
    assert_eq!(hit.server_side, "required");
    assert_eq!(hit.client_side, "optional");
    assert_eq!(hit.downloads, 20_538_127);
}

#[test]
fn a_hits_loaders_are_recovered_from_its_categories() {
    // A search hit has no loaders field at all, so they have to be split back
    // out of categories — which also carry genuine tags like "worldgen".
    let hit: SearchHit = serde_json::from_str(TERRALITH_HIT).unwrap();
    assert_eq!(
        hit.loaders(),
        vec!["datapack", "fabric", "forge", "neoforge", "quilt"]
    );
    assert!(!hit.loaders().iter().any(|loader| loader == "worldgen"));
}

#[test]
fn missing_list_fields_default_to_empty_rather_than_failing() {
    let hit: SearchHit = serde_json::from_str(
        r#"{"slug":"x","title":"X","description":"d","downloads":0,
            "client_side":"unknown","server_side":"unknown"}"#,
    )
    .unwrap();
    assert!(hit.loaders().is_empty());
    assert!(recent_versions(&hit.versions).is_empty());
}

#[test]
fn a_project_lookup_reads_its_own_loader_and_version_fields() {
    let project: Project = serde_json::from_str(
        r#"{"slug":"chunky","title":"Chunky","description":"Pre-generates chunks.",
            "downloads":1000,"client_side":"unsupported","server_side":"required",
            "loaders":["fabric","neoforge"],"game_versions":["26.1","26.2"],
            "body":"long text","status":"approved"}"#,
    )
    .unwrap();
    assert_eq!(project.loaders, vec!["fabric", "neoforge"]);
    assert_eq!(project.game_versions, vec!["26.1", "26.2"]);
}

#[test]
fn recent_versions_takes_the_newest_end_of_the_list() {
    // Modrinth returns versions oldest-first, so "recent" is the tail.
    let versions: Vec<String> = (1..=20).map(|n| format!("26.{n}")).collect();
    assert_eq!(
        recent_versions(&versions),
        vec!["26.15", "26.16", "26.17", "26.18", "26.19", "26.20"]
    );
}

#[test]
fn a_short_version_list_is_returned_whole() {
    let versions = vec!["26.1".to_owned(), "26.2".to_owned()];
    assert_eq!(recent_versions(&versions), versions);
}

#[test]
fn an_unfiltered_search_just_shows_the_recent_versions() {
    let versions: Vec<String> = (1..=10).map(|n| format!("26.{n}")).collect();
    assert_eq!(
        versions_for_display(&versions, None),
        recent_versions(&versions)
    );
}

#[test]
fn the_version_searched_for_stays_visible_when_newer_ones_crowd_it_out() {
    // The real case: a project supports 26.2 and also six newer snapshots, so
    // the newest-six view hides the one version known to match — and a reader
    // comparing that list against the server rules the project out wrongly.
    let versions = vec![
        "26.2".to_owned(),
        "26.3-snapshot-1".to_owned(),
        "26.3-snapshot-2".to_owned(),
        "26.3-snapshot-3".to_owned(),
        "26.3-snapshot-4".to_owned(),
        "26.3-snapshot-5".to_owned(),
        "26.3-snapshot-6".to_owned(),
    ];
    assert!(
        !recent_versions(&versions).contains(&"26.2".to_owned()),
        "precondition: the plain recent view hides it"
    );
    let shown = versions_for_display(&versions, Some("26.2"));
    assert_eq!(shown.first().map(String::as_str), Some("26.2"));
    assert_eq!(shown.len(), 6, "the list stays bounded");
}

#[test]
fn a_version_already_in_the_recent_view_is_not_duplicated() {
    let versions = vec!["26.1".to_owned(), "26.2".to_owned()];
    assert_eq!(versions_for_display(&versions, Some("26.2")), versions);
}

#[test]
fn a_version_the_project_lacks_is_never_invented() {
    // Prepending an unsupported version would claim compatibility the registry
    // never reported — worse than hiding a supported one.
    let versions = vec!["26.1".to_owned(), "26.2".to_owned()];
    assert_eq!(versions_for_display(&versions, Some("1.7.10")), versions);
}
