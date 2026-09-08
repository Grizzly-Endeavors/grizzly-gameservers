//! Search-query construction and the pure narrowing applied to a page of
//! listings. No IO, so the filtering rules are unit-testable without the API.

use crate::registry::{Side, SideSupport};

use super::types::Listing;

/// How many hits a search returns. Deliberately small, matching Modrinth's:
/// hit titles and descriptions are third-party text entering the model's
/// context, so the surface is bounded the same way game logs are.
pub(crate) const SEARCH_LIMIT: usize = 5;

/// How many listings to ask the API for before narrowing. Larger than
/// [`SEARCH_LIMIT`] because deprecated, NSFW, and wrong-side listings are
/// dropped here rather than by the API — asking for five would routinely leave
/// one or two after filtering.
pub(crate) const PAGE_SIZE: usize = 20;

/// Cyberstorm's ordering that puts the packages people actually run first.
/// Relevance alone surfaces abandoned near-name-matches ahead of the community's
/// standard answer, which is the wrong default when the asker is a friend who
/// described what they want rather than named it.
pub(crate) const ORDERING: &str = "most-downloaded";

/// One search request against one community.
#[derive(Clone, Debug)]
pub(crate) struct SearchQuery {
    pub(crate) community: String,
    pub(crate) text: String,
    pub(crate) side: Option<Side>,
}

/// Narrow a page of listings to what Gary should see: no deprecated packages, no
/// NSFW ones, only the requested side, and at most [`SEARCH_LIMIT`] of them.
///
/// Deprecation is a hard drop rather than a rendered flag on a search hit: the
/// author has said not to install it, and a listing Gary can see is a listing
/// Gary can pick. `mod_info` still reports deprecation, so a package named
/// directly is checked rather than hidden.
pub(crate) fn narrow(listings: Vec<Listing>, side: Option<Side>) -> Vec<Listing> {
    listings
        .into_iter()
        .filter(|listing| !listing.is_deprecated && !listing.is_nsfw)
        .filter(|listing| side.is_none_or(|side| listing.side_support().matches(side)))
        .take(SEARCH_LIMIT)
        .collect()
}

/// The category slug marking a package as installed on the server.
pub(crate) const SERVER_SIDE_SLUG: &str = "server-side";

/// The category slug marking a package as installed on players' machines.
pub(crate) const CLIENT_SIDE_SLUG: &str = "client-side";

/// Classify a package from the categories its community listing carries.
///
/// Matched case-insensitively because the two endpoints spell a category
/// differently: a Cyberstorm listing carries the slug (`server-side`) and a
/// package's community listing carries the display name (`Server-side`). They
/// differ only in case, so one comparison covers both without a slugifier.
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
