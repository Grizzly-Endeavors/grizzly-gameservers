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

    /// Terralith ships as both a datapack and a jar mod, so it is the one
    /// project that tells a correct kind filter from a plausible-looking one.
    const DATAPACK_AND_MOD: &str = "terralith";

    /// Run a real search through the same [`SearchQuery`] the tools use, so
    /// these check the shipped facet construction rather than a copy of it.
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
        // The regression this module exists for: the broken filter returned a
        // single unrelated hit where the unfiltered query returns a full page.
        // A kind filter is a narrowing, so it must keep a healthy share rather
        // than collapse — which is what two near-disjoint terms produce.
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
    async fn a_jar_only_mod_is_absent_from_a_datapack_search() {
        // The other direction: the filter must actually exclude, not just include.
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
