use super::*;

fn query(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.to_owned(),
        ..SearchQuery::default()
    }
}

#[test]
fn an_unfiltered_search_sends_no_facets() {
    assert_eq!(query("minimap").facets(), None);
}

#[test]
fn a_mod_search_subtracts_datapacks() {
    // Datapacks are project_type "mod" on Modrinth, so without the exclusion a
    // search for "a mod" answers with datapacks like Terralith.
    let facets = SearchQuery {
        kind: Some(Kind::Mod),
        ..query("chunky")
    }
    .facets()
    .expect("kind sets a facet");
    assert_eq!(facets, r#"[["project_type:mod"],["categories!=datapack"]]"#);
}

#[test]
fn a_datapack_search_selects_the_datapack_category() {
    let facets = SearchQuery {
        kind: Some(Kind::Datapack),
        ..query("terralith")
    }
    .facets()
    .expect("kind sets a facet");
    assert_eq!(facets, r#"[["project_type:mod"],["categories:datapack"]]"#);
}

#[test]
fn loader_and_game_version_are_and_ed_as_separate_groups() {
    let facets = SearchQuery {
        loader: Some(Loader::Fabric),
        game_version: Some("26.2".to_owned()),
        ..query("chunky")
    }
    .facets()
    .expect("filters set facets");
    assert_eq!(facets, r#"[["categories:fabric"],["versions:26.2"]]"#);
}

#[test]
fn every_filter_together_produces_one_group_each() {
    let facets = SearchQuery {
        text: "chunky".to_owned(),
        kind: Some(Kind::Mod),
        loader: Some(Loader::Neoforge),
        game_version: Some("26.2".to_owned()),
    }
    .facets()
    .expect("filters set facets");
    assert_eq!(
        facets,
        r#"[["project_type:mod"],["categories!=datapack"],["categories:neoforge"],["versions:26.2"]]"#
    );
}

#[test]
fn facet_values_are_json_escaped() {
    // A game version is model-supplied, so a quote in it must not break out of
    // the facet string and forge extra filters.
    let facets = SearchQuery {
        game_version: Some(r#"1.0"]] or ["#.to_owned()),
        ..query("chunky")
    }
    .facets()
    .expect("filter sets a facet");
    let parsed: serde_json::Value =
        serde_json::from_str(&facets).expect("facets stay valid json regardless of input");
    assert_eq!(parsed, serde_json::json!([[r#"versions:1.0"]] or ["#]]));
}

#[test]
fn loaders_render_as_modrinth_category_names() {
    assert_eq!(Loader::Fabric.to_string(), "fabric");
    assert_eq!(Loader::Neoforge.to_string(), "neoforge");
    assert_eq!(Loader::Quilt.to_string(), "quilt");
    assert_eq!(Loader::Forge.to_string(), "forge");
}
