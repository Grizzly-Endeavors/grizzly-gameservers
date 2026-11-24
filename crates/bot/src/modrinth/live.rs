//! Opt-in contract checks against the real Modrinth API.
//!
//! These exist because a facet grammar is only verified by what it returns. The
//! [`query`](super::query) unit tests pin the facet *string* — which is exactly
//! the assertion that stayed green while the datapack filter selected nearly
//! nothing. A query can be well-formed, well-tested, and still mean the wrong
//! thing; only a live call catches that.
//!
//! Ignored by default so the suite stays offline and CI never depends on a third
//! party. Run them when touching facet construction:
//!
//! ```text
//! cargo test --quiet --all-features modrinth::live -- --ignored
//! ```

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::modrinth::{Kind, SearchQuery};
    use crate::registry::Side;

    const DATAPACK_AND_MOD: &str = "terralith";

    async fn slugs(query: &SearchQuery) -> Vec<String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap();
        crate::modrinth::search_projects(&http, query)
            .await
            .expect("modrinth reachable")
            .into_iter()
            .map(|hit| hit.slug)
            .collect()
    }

    fn seeking(kind: Option<Kind>) -> SearchQuery {
        SearchQuery {
            text: DATAPACK_AND_MOD.to_owned(),
            kind,
            ..SearchQuery::default()
        }
    }

    #[tokio::test]
    #[ignore = "hits the live Modrinth API"]
    async fn the_datapack_filter_actually_returns_datapacks() {
        let found = slugs(&seeking(Some(Kind::Datapack))).await;
        assert!(
            found.iter().any(|slug| slug == DATAPACK_AND_MOD),
            "the datapack filter must find a datapack; got {found:?}"
        );
    }

    #[tokio::test]
    #[ignore = "hits the live Modrinth API"]
    async fn the_mod_filter_actually_returns_mods() {
        let found = slugs(&seeking(Some(Kind::Mod))).await;
        assert!(
            found.iter().any(|slug| slug == DATAPACK_AND_MOD),
            "the mod filter must find a project shipping a jar; got {found:?}"
        );
    }

    #[tokio::test]
    #[ignore = "hits the live Modrinth API"]
    async fn a_kind_filter_narrows_without_emptying_the_result() {
        let unfiltered = slugs(&seeking(None)).await.len();
        for kind in [Kind::Mod, Kind::Datapack] {
            let filtered = slugs(&seeking(Some(kind))).await.len();
            assert!(
                filtered * 2 >= unfiltered,
                "{kind:?} collapsed {unfiltered} hits to {filtered} — the terms are likely disjoint"
            );
        }
    }

    #[tokio::test]
    #[ignore = "hits the live Modrinth API"]
    async fn the_side_facets_narrow_without_emptying_the_result() {
        let unfiltered = slugs(&seeking(None)).await.len();
        for side in [Side::Server, Side::Client, Side::Both] {
            let filtered = slugs(&SearchQuery {
                side: Some(side),
                ..seeking(None)
            })
            .await
            .len();
            assert!(
                filtered > 0,
                "{side:?} collapsed {unfiltered} hits to none — the support vocabulary is likely wrong"
            );
        }
    }

    #[tokio::test]
    #[ignore = "hits the live Modrinth API"]
    async fn a_jar_only_mod_is_absent_from_a_datapack_search() {
        let found = slugs(&SearchQuery {
            text: "xaeros minimap".to_owned(),
            kind: Some(Kind::Datapack),
            ..SearchQuery::default()
        })
        .await;
        assert!(
            !found.iter().any(|slug| slug == "xaeros-minimap"),
            "a jar-only mod must not answer a datapack search; got {found:?}"
        );
    }
}
