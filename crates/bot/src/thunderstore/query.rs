//! Search-query construction and the pure narrowing applied to a page of
//! listings. No IO, so the filtering rules are unit-testable without the API.

use crate::registry::{Side, SideSupport};

use super::types::Listing;

pub(crate) const SEARCH_LIMIT: usize = 5;

/// How many listings to ask the API for before narrowing. Larger than
/// [`SEARCH_LIMIT`] because deprecated, NSFW, and wrong-side listings are
/// dropped here rather than by the API — asking for five would routinely leave
/// one or two after filtering.
pub(crate) const PAGE_SIZE: usize = 20;

pub(crate) const ORDERING: &str = "most-downloaded";

#[derive(Clone, Debug)]
pub(crate) struct SearchQuery {
    pub(crate) community: String,
    pub(crate) text: String,
    pub(crate) side: Option<Side>,
}

pub(crate) fn narrow(listings: Vec<Listing>, side: Option<Side>) -> Vec<Listing> {
    listings
        .into_iter()
        .filter(|listing| !listing.is_deprecated && !listing.is_nsfw)
        .filter(|listing| side.is_none_or(|side| listing.side_support().matches(side)))
        .take(SEARCH_LIMIT)
        .collect()
}

pub(crate) const SERVER_SIDE_SLUG: &str = "server-side";

pub(crate) const CLIENT_SIDE_SLUG: &str = "client-side";

pub(crate) fn side_support_from_categories<'a>(
    categories: impl Iterator<Item = &'a str>,
) -> SideSupport {
    let mut server = false;
    let mut client = false;
    for category in categories {
        if category.eq_ignore_ascii_case(SERVER_SIDE_SLUG) {
            server = true;
        } else if category.eq_ignore_ascii_case(CLIENT_SIDE_SLUG) {
            client = true;
        }
    }
    SideSupport::from_flags(server, client)
}

#[cfg(test)]
#[path = "tests/query.rs"]
mod tests;
