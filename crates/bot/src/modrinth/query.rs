//! Search-query construction: the filters Gary may set, and the Modrinth facet
//! string they compile to. Pure — no IO, so the facet grammar is unit-testable.

use std::fmt;

use crate::registry::Side;

/// How many hits a search returns. Deliberately small: hit titles and
/// descriptions are third-party text entering the model's context, so the
/// surface is bounded the same way game logs are.
pub(crate) const SEARCH_LIMIT: usize = 5;

/// What kind of project the caller is after.
///
/// `project_type` means two different things in v2 and they must not be
/// conflated: as a *facet* its vocabulary includes `datapack`, but as a
/// *response field* it never returns one — a datapack always reads back as
/// `mod`. The index holds a separate document per project type, so a project
/// shipping both a datapack and a jar (Terralith does) is found under either,
/// which is the correct answer for both questions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A server-installed jar, needing a loader such as Fabric.
    Mod,
    /// A datapack: vanilla-compatible, no loader needed on the server.
    Datapack,
}

impl Kind {
    /// The `project_type` facet value that selects this kind.
    fn facet_value(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::Datapack => "datapack",
        }
    }
}

impl From<crate::prompts::SearchModsKind> for Kind {
    fn from(value: crate::prompts::SearchModsKind) -> Self {
        match value {
            crate::prompts::SearchModsKind::Mod => Self::Mod,
            crate::prompts::SearchModsKind::Datapack => Self::Datapack,
        }
    }
}

/// A mod loader, used both as a search filter and as the `TYPE` an instance
/// would need. Only the loaders Modrinth indexes as categories are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Loader {
    Fabric,
    Forge,
    Neoforge,
    Quilt,
}

impl From<crate::prompts::SearchModsLoader> for Loader {
    fn from(value: crate::prompts::SearchModsLoader) -> Self {
        match value {
            crate::prompts::SearchModsLoader::Fabric => Self::Fabric,
            crate::prompts::SearchModsLoader::Forge => Self::Forge,
            crate::prompts::SearchModsLoader::Neoforge => Self::Neoforge,
            crate::prompts::SearchModsLoader::Quilt => Self::Quilt,
        }
    }
}

impl fmt::Display for Loader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            Self::Fabric => "fabric",
            Self::Forge => "forge",
            Self::Neoforge => "neoforge",
            Self::Quilt => "quilt",
        };
        f.write_str(name)
    }
}

/// One search request: free text plus the optional narrowing filters.
#[derive(Clone, Debug, Default)]
pub(crate) struct SearchQuery {
    pub(crate) text: String,
    pub(crate) kind: Option<Kind>,
    pub(crate) loader: Option<Loader>,
    pub(crate) game_version: Option<String>,
    pub(crate) side: Option<Side>,
}

/// The facet values that mean "this project does something on that side".
///
/// Both `required` and `optional` count: `optional` means the project works
/// there, which is what a caller narrowing to a side is asking about. Only
/// `unsupported` — the excluded third value — means it does not.
const APPLIES: [&str; 2] = ["required", "optional"];

/// The facet group selecting projects that apply on one side.
fn side_group(field: &str) -> String {
    let terms: Vec<String> = APPLIES
        .iter()
        .map(|support| format!("{field}:{support}"))
        .collect();
    facet_group(&terms.iter().map(String::as_str).collect::<Vec<_>>())
}

impl SearchQuery {
    /// Compile the filters into Modrinth's facet syntax: a JSON array of arrays,
    /// where entries within an inner array are OR'd and the outer arrays are
    /// AND'd. Returns `None` when no filter is set, so the caller omits the
    /// parameter entirely rather than sending an empty one.
    pub(crate) fn facets(&self) -> Option<String> {
        let mut groups: Vec<String> = Vec::new();
        if let Some(kind) = self.kind {
            groups.push(facet_group(&[&format!(
                "project_type:{}",
                kind.facet_value()
            )]));
        }
        if let Some(loader) = self.loader {
            groups.push(facet_group(&[&format!("categories:{loader}")]));
        }
        if let Some(version) = self.game_version.as_deref() {
            groups.push(facet_group(&[&format!("versions:{version}")]));
        }
        match self.side {
            Some(Side::Server) => groups.push(side_group("server_side")),
            Some(Side::Client) => groups.push(side_group("client_side")),
            // Both sides are separate AND'd groups rather than one OR'd group:
            // a project needing installing in both places must satisfy each.
            Some(Side::Both) => {
                groups.push(side_group("server_side"));
                groups.push(side_group("client_side"));
            }
            None => {}
        }
        if groups.is_empty() {
            return None;
        }
        Some(format!("[{}]", groups.join(",")))
    }
}

/// Render one OR-group of facet terms as a JSON array of strings.
fn facet_group(terms: &[&str]) -> String {
    let quoted: Vec<String> = terms
        .iter()
        .map(|term| serde_json::Value::String((*term).to_owned()).to_string())
        .collect();
    format!("[{}]", quoted.join(","))
}

#[cfg(test)]
#[path = "tests/query.rs"]
mod tests;
