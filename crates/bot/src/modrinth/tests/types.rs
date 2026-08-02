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
fn short_text_passes_through_untouched() {
    assert_eq!(truncate("Terralith", 80), "Terralith");
    assert_eq!(truncate("abc", 3), "abc");
}

#[test]
fn overlong_text_is_cut_and_marked() {
    assert_eq!(truncate("abcdef", 3), "abc…");
}

#[test]
fn truncation_counts_characters_not_bytes() {
    // A byte-wise cut would split these and panic; the limit is a character
    // count so multi-byte author text survives it.
    assert_eq!(truncate("日本語テキスト", 3), "日本語…");
    assert_eq!(truncate("émoji 🎮 pack", 7), "émoji 🎮…");
}

#[test]
fn a_description_is_flattened_to_one_line() {
    // Descriptions feed a one-line-per-hit rendering, so embedded newlines would
    // break the listing apart into what reads like extra results.
    assert_eq!(
        short_description("adds biomes\nand  caves\n\nand more"),
        "adds biomes and caves and more"
    );
}

#[test]
fn a_long_description_is_bounded() {
    let long = "x".repeat(500);
    let short = short_description(&long);
    assert_eq!(short.chars().count(), 201, "200 characters plus the marker");
    assert!(short.ends_with('…'));
}

#[test]
fn a_long_title_is_bounded() {
    let short = short_title(&"t".repeat(200));
    assert_eq!(short.chars().count(), 81, "80 characters plus the marker");
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
fn an_empty_list_renders_as_words_not_a_blank() {
    assert_eq!(list_or_none(&[]), "none listed");
    assert_eq!(
        list_or_none(&["fabric".to_owned(), "quilt".to_owned()]),
        "fabric, quilt"
    );
}
