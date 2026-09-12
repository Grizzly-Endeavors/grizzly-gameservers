//! Thunderstore client, backing Gary's mod discovery for the r2modman-shaped
//! games (Valheim first).
//!
//! It mirrors [`modrinth`](crate::modrinth) in shape and exists for the same
//! reason — a package name recalled from training data is often wrong, and a
//! wrong entry in a server's mod list stops its next launch — but the registry
//! underneath is a different animal in three ways worth knowing before editing:
//!
//! - **Packages are per community.** `valheim` and `lethal-company` are separate
//!   catalogs; a package exists in one, not in "Thunderstore". Every call here
//!   therefore carries a community slug, taken from the game's catalog entry.
//! - **There is no server/client flag.** Communities express it with categories
//!   (`server-side`, `client-side`), which is why the side filter runs in
//!   [`query`] over category *slugs* rather than as a query parameter: category
//!   ids are per community and would have to be looked up to be sent.
//! - **Profile codes are a third surface.** `legacyprofile` mints and serves the
//!   payload behind an r2modman profile code — the format modded groups actually
//!   agree a modpack in. It takes no authentication in either direction, and it
//!   has no delete: every code ever minted is permanent, which is the whole
//!   reason [`profile`] exports are cached rather than minted per request.
//! - **Two APIs, deliberately.** Search uses the Cyberstorm listing endpoint
//!   (paged, filterable); a single package uses the experimental package
//!   endpoint, which is the one that reports the latest version string and its
//!   dependencies. The v1 package index is not an option at any size — it is
//!   162 MB for Valheim alone.

pub(crate) mod client;
mod live;
pub(crate) mod modpack;
pub(crate) mod profile;
pub(crate) mod query;
pub(crate) mod types;
pub(crate) mod updates;

pub(crate) use client::{create_profile, fetch_package, fetch_profile, search_packages};
pub(crate) use query::SearchQuery;
pub(crate) use types::{Listing, Package, split_full_name};
