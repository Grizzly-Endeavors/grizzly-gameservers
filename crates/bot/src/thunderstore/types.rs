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

const DEPENDENCIES_SHOWN: usize = 8;

#[derive(Debug, Deserialize)]
pub(crate) struct ListingResponse {
    pub(crate) results: Vec<Listing>,
}

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

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ListingCategory {
    pub(crate) slug: String,
}

impl Listing {
    pub(crate) fn full_name(&self) -> String {
        format!("{}-{}", self.namespace, self.name)
    }

    pub(crate) fn side_support(&self) -> SideSupport {
        side_support_from_categories(
            self.categories
                .iter()
                .map(|category| category.slug.as_str()),
        )
    }

    pub(crate) fn updated_day(&self) -> &str {
        self.last_updated
            .split('T')
            .next()
            .filter(|day| day.len() == 10)
            .unwrap_or("unknown")
    }
}

pub(crate) fn split_full_name(full_name: &str) -> Option<(&str, &str)> {
    let (namespace, name) = full_name.split_once('-')?;
    let component_ok = |part: &str| {
        !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    (component_ok(namespace) && component_ok(name)).then_some((namespace, name))
}

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

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CommunityListing {
    pub(crate) community: String,
    #[serde(default)]
    pub(crate) categories: Vec<String>,
}

impl Package {
    pub(crate) fn full_name(&self) -> String {
        format!("{}-{}", self.namespace, self.name)
    }

    pub(crate) fn pinned_name(&self) -> String {
        format!("{}-{}", self.full_name(), self.latest.version_number)
    }

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

    pub(crate) fn shown_dependencies(&self) -> Vec<String> {
        self.latest
            .dependencies
            .iter()
            .take(DEPENDENCIES_SHOWN)
            .cloned()
            .collect()
    }

    pub(crate) fn dependencies_truncated(&self) -> bool {
        self.latest.dependencies.len() > DEPENDENCIES_SHOWN
    }
}

#[cfg(test)]
#[path = "tests/types.rs"]
mod tests;
