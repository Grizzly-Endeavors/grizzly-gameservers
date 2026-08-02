//! Modrinth response shapes and the bounding applied to them. Pure — only the
//! fields the tools render are modelled; everything else is ignored on
//! deserialize.

use serde::Deserialize;

/// Longest title kept. Titles are author-controlled third-party text, so they
/// are cut before entering the model's context.
const TITLE_LIMIT: usize = 80;

/// Longest description kept, same reasoning as [`TITLE_LIMIT`].
const DESCRIPTION_LIMIT: usize = 200;

/// How many supported game versions to render. The full list runs to dozens of
/// entries and only the recent ones bear on whether a mod fits a live server.
const VERSIONS_SHOWN: usize = 6;

/// The loader names Modrinth mixes into a search hit's `categories` alongside
/// genuine tags like `worldgen`. Splitting on this set is the only way to
/// recover a search hit's loaders, which — unlike a project lookup — it does not
/// report as a field of its own.
const KNOWN_LOADERS: [&str; 11] = [
    "bukkit", "datapack", "fabric", "folia", "forge", "neoforge", "paper", "purpur", "quilt",
    "spigot", "sponge",
];

/// The envelope `GET /v2/search` returns.
#[derive(Debug, Deserialize)]
pub(crate) struct SearchResponse {
    pub(crate) hits: Vec<SearchHit>,
}

/// One search hit.
///
/// Note the absent fields: a hit carries no `loaders` (they are folded into
/// `categories`) and its `latest_version` is a base62 *version id*, not a
/// Minecraft version, despite what upstream docs claim — so neither is modelled
/// and the version list comes from `versions`.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SearchHit {
    pub(crate) slug: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) downloads: u64,
    pub(crate) client_side: String,
    pub(crate) server_side: String,
    #[serde(default)]
    pub(crate) categories: Vec<String>,
    #[serde(default)]
    pub(crate) versions: Vec<String>,
}

impl SearchHit {
    /// The loaders this project supports, recovered from `categories`.
    pub(crate) fn loaders(&self) -> Vec<String> {
        self.categories
            .iter()
            .filter(|category| KNOWN_LOADERS.contains(&category.as_str()))
            .cloned()
            .collect()
    }
}

/// One project, from `GET /v2/project/{slug}`. Unlike a search hit this does
/// report `loaders` and `game_versions` as proper fields.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Project {
    pub(crate) slug: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) downloads: u64,
    pub(crate) client_side: String,
    pub(crate) server_side: String,
    #[serde(default)]
    pub(crate) loaders: Vec<String>,
    #[serde(default)]
    pub(crate) game_versions: Vec<String>,
}

/// Cut `text` to `limit` characters, marking it when something was dropped.
///
/// Counts characters rather than bytes: the input is arbitrary author-supplied
/// text and a byte cut would split a multi-byte character.
pub(crate) fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(limit).collect();
    cut.push('…');
    cut
}

/// A title, bounded.
pub(crate) fn short_title(title: &str) -> String {
    truncate(title, TITLE_LIMIT)
}

/// A description, bounded and flattened — newlines would break the one-line-per-
/// hit rendering the tools use.
pub(crate) fn short_description(description: &str) -> String {
    let flattened = description.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&flattened, DESCRIPTION_LIMIT)
}

/// The most recent supported game versions, newest last.
///
/// Modrinth returns the list oldest-first, so the tail is the recent end.
pub(crate) fn recent_versions(versions: &[String]) -> Vec<String> {
    let skip = versions.len().saturating_sub(VERSIONS_SHOWN);
    versions.iter().skip(skip).cloned().collect()
}

/// The versions to show for a hit, keeping `asked_for` visible when the search
/// filtered on one.
///
/// The newest few alone are misleading here: a project can support the version
/// the caller filtered on and still have six newer snapshot releases above it,
/// so the one version known to match would be the one version not displayed —
/// and a reader comparing the list against the server would rule the project
/// out for supporting exactly what they asked for.
pub(crate) fn versions_for_display(versions: &[String], asked_for: Option<&str>) -> Vec<String> {
    let recent = recent_versions(versions);
    let Some(asked_for) = asked_for else {
        return recent;
    };
    if recent.iter().any(|version| version == asked_for)
        || !versions.iter().any(|version| version == asked_for)
    {
        return recent;
    }
    let mut shown = vec![asked_for.to_owned()];
    shown.extend(recent.into_iter().take(VERSIONS_SHOWN - 1));
    shown
}

/// Join a list for display, or name it empty so a blank never reads as an
/// omission the model should fill in.
pub(crate) fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        return "none listed".to_owned();
    }
    items.join(", ")
}

#[cfg(test)]
#[path = "tests/types.rs"]
mod tests;
