use super::*;
use crate::thunderstore::types::{Listing, ListingCategory};

fn listing(name: &str, slugs: &[&str]) -> Listing {
    Listing {
        namespace: "Author".to_owned(),
        name: name.to_owned(),
        description: "does a thing".to_owned(),
        download_count: 10,
        categories: slugs
            .iter()
            .map(|slug| ListingCategory {
                slug: (*slug).to_owned(),
            })
            .collect(),
        is_deprecated: false,
        is_nsfw: false,
        last_updated: "2026-03-12T00:31:53Z".to_owned(),
    }
}

fn names(listings: Vec<Listing>) -> Vec<String> {
    listings.into_iter().map(|hit| hit.name).collect()
}

#[test]
fn an_unfiltered_page_keeps_its_order() {
    let page = vec![listing("First", &[]), listing("Second", &[])];
    assert_eq!(names(narrow(page, None)), vec!["First", "Second"]);
}

#[test]
fn deprecated_and_nsfw_listings_never_reach_the_model() {
    let mut deprecated = listing("Abandoned", &["mods"]);
    deprecated.is_deprecated = true;
    let mut nsfw = listing("Explicit", &["mods"]);
    nsfw.is_nsfw = true;
    let page = vec![deprecated, nsfw, listing("Fine", &["mods"])];
    assert_eq!(names(narrow(page, None)), vec!["Fine"]);
}

#[test]
fn a_server_filter_keeps_server_only_and_both_sided_packages() {
    let page = vec![
        listing("ServerOnly", &["server-side"]),
        listing("BothSides", &["server-side", "client-side"]),
        listing("ClientOnly", &["client-side"]),
        listing("Uncategorised", &["mods"]),
    ];
    assert_eq!(
        names(narrow(page, Some(Side::Server))),
        vec!["ServerOnly", "BothSides"]
    );
}

#[test]
fn a_both_filter_keeps_only_the_packages_players_also_need() {
    let page = vec![
        listing("ServerOnly", &["server-side"]),
        listing("BothSides", &["server-side", "client-side"]),
    ];
    assert_eq!(names(narrow(page, Some(Side::Both))), vec!["BothSides"]);
}

#[test]
fn the_page_is_cut_to_the_search_limit() {
    let page: Vec<Listing> = (0..PAGE_SIZE)
        .map(|index| listing(&format!("Mod{index}"), &[]))
        .collect();
    assert_eq!(narrow(page, None).len(), SEARCH_LIMIT);
}

#[test]
fn the_page_asked_for_is_larger_than_what_is_shown() {
    // Filtering happens after the fetch, so a page sized to the display limit
    // would routinely come back short of it.
    const { assert!(PAGE_SIZE > SEARCH_LIMIT) }
}

#[test]
fn categories_classify_case_insensitively_across_both_endpoints() {
    // Slugs from a listing, display names from a package detail.
    assert_eq!(
        side_support_from_categories(["server-side", "mods"].into_iter()),
        SideSupport::ServerOnly
    );
    assert_eq!(
        side_support_from_categories(["Server-side", "Client-side"].into_iter()),
        SideSupport::Both
    );
    assert_eq!(
        side_support_from_categories(["Mods", "Utility"].into_iter()),
        SideSupport::Unknown
    );
}
