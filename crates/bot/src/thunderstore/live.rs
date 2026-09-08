//! Opt-in contract checks against the real Thunderstore API.
//!
//! These exist for the reason the Modrinth ones do: the response shapes and the
//! side-category convention here were established by reading live responses, not
//! documentation, and a unit test over a hand-written fixture only ever proves
//! the fixture is still the fixture. Two things in particular can drift without
//! any local signal — the Cyberstorm listing envelope, and whether the Valheim
//! community still expresses sidedness with `server-side`/`client-side`
//! categories at all.
//!
//! Ignored by default so the suite stays offline and CI never depends on a third
//! party. Run them when touching this module:
//!
//! ```text
//! cargo test --quiet --all-features thunderstore::live -- --ignored
//! ```

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::registry::{Side, SideSupport};
    use crate::thunderstore::{SearchQuery, fetch_package, search_packages};

    /// The community every fact in this module was verified against.
    const COMMUNITY: &str = "valheim";

    /// Valheim's modding framework: long-lived, never deprecated, and the one
    /// package guaranteed to depend on the `BepInEx` loader pack — so it proves
    /// both the package endpoint and its dependency strings at once. It is a
    /// library and carries no side categories at all, which is why the side
    /// assertion below uses a different package.
    const FRAMEWORK: (&str, &str) = ("ValheimModding", "Jotunn");

    /// A package the community has categorised on both sides — the one shape
    /// that proves a package detail's categories still classify, since the
    /// package endpoint spells them as display names rather than slugs.
    const BOTH_SIDED: (&str, &str) = ("Mydayyy", "ServerSideMap");

    fn http() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap()
    }

    fn seeking(text: &str, side: Option<Side>) -> SearchQuery {
        SearchQuery {
            community: COMMUNITY.to_owned(),
            text: text.to_owned(),
            side,
        }
    }

    #[tokio::test]
    #[ignore = "hits the live Thunderstore API"]
    async fn a_search_returns_packages_from_the_named_community() {
        let hits = search_packages(&http(), &seeking("map", None))
            .await
            .expect("thunderstore reachable");
        assert!(!hits.is_empty(), "a broad term must find something");
        for hit in &hits {
            assert!(!hit.namespace.is_empty() && !hit.name.is_empty());
        }
    }

    #[tokio::test]
    #[ignore = "hits the live Thunderstore API"]
    async fn the_side_categories_are_still_how_this_community_expresses_sidedness() {
        // The whole side filter rests on this convention, which is community
        // practice rather than a Thunderstore feature — if it ever stops being
        // used, every side-filtered search silently returns nothing.
        let hits = search_packages(&http(), &seeking("map", Some(Side::Server)))
            .await
            .expect("thunderstore reachable");
        assert!(
            !hits.is_empty(),
            "no server-side hits at all suggests the category convention changed"
        );
        for hit in &hits {
            assert!(
                hit.side_support().matches(Side::Server),
                "{} survived a server filter without a server-side category",
                hit.full_name()
            );
        }
    }

    #[tokio::test]
    #[ignore = "hits the live Thunderstore API"]
    async fn a_package_reports_a_version_and_its_loader_dependency() {
        let (namespace, name) = FRAMEWORK;
        let package = fetch_package(&http(), namespace, name)
            .await
            .expect("thunderstore reachable")
            .expect("the framework package exists");
        assert_eq!(package.full_name(), format!("{namespace}-{name}"));
        assert!(
            package.latest.version_number.split('.').count() == 3,
            "a pinned entry needs a three-part version, got {}",
            package.latest.version_number
        );
        assert!(
            package
                .latest
                .dependencies
                .iter()
                .any(|dependency| dependency.contains("BepInExPack")),
            "the framework must depend on the loader pack; got {:?}",
            package.latest.dependencies
        );
    }

    #[tokio::test]
    #[ignore = "hits the live Thunderstore API"]
    async fn a_package_detail_classifies_off_display_names() {
        // The package endpoint returns "Server-side", the listing endpoint
        // returns "server-side". A classifier written against one spelling
        // reports Unknown for every package looked up through the other.
        let (namespace, name) = BOTH_SIDED;
        let package = fetch_package(&http(), namespace, name)
            .await
            .expect("thunderstore reachable")
            .expect("the package exists");
        assert_eq!(package.side_support(COMMUNITY), SideSupport::Both);
    }

    #[tokio::test]
    #[ignore = "hits the live Thunderstore API"]
    async fn an_invented_package_is_absent_rather_than_an_error() {
        // The case the tool exists to catch: a name the model made up must come
        // back as "no such package", not as the registry being unreachable.
        let found = fetch_package(&http(), "Nobody", "DoesNotExistAtAll")
            .await
            .expect("thunderstore reachable");
        assert!(found.is_none());
    }
}
