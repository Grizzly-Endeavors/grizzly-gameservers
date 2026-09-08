//! Thunderstore response shapes and the derivations over them. Pure — only the
//! fields the tools render are modelled; everything else is ignored on
//! deserialize.
//!
//! The two endpoints disagree about how a category is spelled: a Cyberstorm
//! listing carries category *objects* with a stable `slug`, while a package's
//! community listing carries display *names* ("Server-side"). Both funnel
//! through `side_support_from_categories`, which matches case-insensitively, so
//! the classification rule itself lives in exactly one place.

use serde::Deserialize;

use crate::registry::SideSupport;

use super::query::side_support_from_categories;

/// How many dependencies a package detail lists. Enough to show a loader pack
/// and the usual handful of libraries without a modpack's manifest flooding the
/// context.
const DEPENDENCIES_SHOWN: usize = 8;

/// The envelope the Cyberstorm listing endpoint returns.
#[derive(Debug, Deserialize)]
pub(crate) struct ListingResponse {
    pub(crate) results: Vec<Listing>,
}

/// One search hit. Note the absent field: a listing reports no version at all,
/// which is why `mod_info` is the step that produces the string Gary pins.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Listing {
    pub(crate) namespace: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) download_count: u64,
    #[serde(default)]
    pub(crate) categories: Vec<ListingCategory>,
    #[serde(default)]
    pub(crate) is_deprecated: bool,
    #[serde(default)]
    pub(crate) is_nsfw: bool,
    #[serde(default)]
    pub(crate) last_updated: String,
}

/// A category on a Cyberstorm listing. The `id` is deliberately unmodelled: ids
/// are per community, so filtering on them would mean a lookup call per search.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ListingCategory {
    pub(crate) slug: String,
}

impl Listing {
    /// The `Namespace-Name` pair that identifies this package everywhere —
    /// in `mod_info`, in a dependency string, and in a server's mod list.
    pub(crate) fn full_name(&self) -> String {
        format!("{}-{}", self.namespace, self.name)
    }

    /// Where this package has to be installed, per its community categories.
    pub(crate) fn side_support(&self) -> SideSupport {
        side_support_from_categories(
            self.categories
                .iter()
                .map(|category| category.slug.as_str()),
        )
    }

    /// The day this package was last updated, without the time of day. An
    /// invalid or absent timestamp renders as the literal `unknown`: staleness
    /// is a judgement signal, never a reason to fail a lookup.
    pub(crate) fn updated_day(&self) -> &str {
        self.last_updated
            .split('T')
            .next()
            .filter(|day| day.len() == 10)
            .unwrap_or("unknown")
    }
}

/// Split a `Namespace-Name` pair into its two halves.
///
/// `None` means the input isn't one — the shape Gary is told to use, and the
/// shape every dependency string and mod-list entry takes. Both components are
/// `[A-Za-z0-9_]+`, so the single separating dash is unambiguous and a slug with
/// none (a Modrinth habit leaking into a Thunderstore game) is rejected here
/// rather than becoming a 404 that reads as "no such mod".
pub(crate) fn split_full_name(full_name: &str) -> Option<(&str, &str)> {
    let (namespace, name) = full_name.split_once('-')?;
    let component_ok = |part: &str| {
        !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    (component_ok(namespace) && component_ok(name)).then_some((namespace, name))
}

/// One package, from the experimental package endpoint.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Package {
    pub(crate) namespace: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) is_deprecated: bool,
    pub(crate) latest: PackageVersion,
    #[serde(default)]
    pub(crate) community_listings: Vec<CommunityListing>,
}

/// The newest version of a package — the one Gary pins, and the one whose
/// dependency list is current.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct PackageVersion {
    pub(crate) version_number: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) dependencies: Vec<String>,
    #[serde(default)]
    pub(crate) downloads: u64,
}

/// How one community lists a package. A package can appear in several; the one
/// that matters is the community the game declares.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CommunityListing {
    pub(crate) community: String,
    /// Display names ("Server-side"), not slugs — see the module docs.
    #[serde(default)]
    pub(crate) categories: Vec<String>,
}

impl Package {
    /// The `Namespace-Name` pair, as the registry spells it.
    pub(crate) fn full_name(&self) -> String {
        format!("{}-{}", self.namespace, self.name)
    }

    /// The exact string a pinned mod-list entry uses.
    pub(crate) fn pinned_name(&self) -> String {
        format!("{}-{}", self.full_name(), self.latest.version_number)
    }

    /// Where this package has to be installed, per the named community's
    /// categories. A package absent from that community classifies as
    /// [`SideSupport::Unknown`] rather than borrowing another community's
    /// categories, which describe a different game.
    pub(crate) fn side_support(&self, community: &str) -> SideSupport {
        let Some(listing) = self
            .community_listings
            .iter()
            .find(|listing| listing.community == community)
        else {
            return SideSupport::Unknown;
        };
        side_support_from_categories(listing.categories.iter().map(String::as_str))
    }

    /// The dependencies to show, bounded. Loader packs and shared libraries make
    /// this list short for a mod and long for a modpack.
    pub(crate) fn shown_dependencies(&self) -> Vec<String> {
        self.latest
            .dependencies
            .iter()
            .take(DEPENDENCIES_SHOWN)
            .cloned()
            .collect()
    }

    /// Whether the dependency list was cut, so a result can say so rather than
    /// implying the shown ones are all of them.
    pub(crate) fn dependencies_truncated(&self) -> bool {
        self.latest.dependencies.len() > DEPENDENCIES_SHOWN
    }
}

#[cfg(test)]
#[path = "tests/types.rs"]
mod tests;
