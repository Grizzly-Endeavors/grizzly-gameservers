use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// One game's raw, version-controlled templates as read from `games/<id>/`. The
/// strings are deserialized per instance by the renderer rather than at load
/// time, so a malformed template surfaces against a specific `/create` instead
/// of preventing the bot from starting.
#[derive(Clone, Debug)]
pub(crate) struct GameCatalogEntry {
    pub(crate) id: String,
    pub(crate) gameserver_yaml: String,
    pub(crate) service_yaml: String,
    pub(crate) pvc_yaml: String,
}

/// The per-game catalog: every game the shim can provision, keyed by id.
#[derive(Clone, Debug, Default)]
pub(crate) struct GameCatalog {
    entries: BTreeMap<String, GameCatalogEntry>,
}

impl GameCatalog {
    pub(crate) fn get(&self, id: &str) -> Option<&GameCatalogEntry> {
        self.entries.get(id)
    }

    /// Game ids in sorted order, for autocomplete and listings.
    pub(crate) fn game_ids(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

/// Load every game directory under `dir` into a [`GameCatalog`]. Directories
/// whose name starts with `_` (e.g. the `_template` skeleton) are skipped.
///
/// # Errors
///
/// Returns an error if `dir` cannot be read, a game id is not a valid RFC1123
/// label, a game directory is missing one of its three templates, or no games
/// are found at all.
pub(crate) async fn load_catalog(dir: &Path) -> Result<GameCatalog> {
    let mut read_dir = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("failed to read catalog directory {}", dir.display()))?;

    let mut entries = BTreeMap::new();
    while let Some(item) = read_dir
        .next_entry()
        .await
        .with_context(|| format!("failed to enumerate catalog directory {}", dir.display()))?
    {
        let path = item.path();
        let file_type = item
            .file_type()
            .await
            .with_context(|| format!("failed to stat catalog entry {}", path.display()))?;
        if !file_type.is_dir() {
            continue;
        }
        let Some(id) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if id.starts_with('_') {
            continue;
        }
        validate_game_id(id)?;
        let entry = load_entry(id, &path).await?;
        entries.insert(id.to_owned(), entry);
    }

    if entries.is_empty() {
        bail!("no games found in catalog directory {}", dir.display());
    }
    Ok(GameCatalog { entries })
}

async fn load_entry(id: &str, dir: &Path) -> Result<GameCatalogEntry> {
    Ok(GameCatalogEntry {
        id: id.to_owned(),
        gameserver_yaml: read_template(dir, "gameserver.yaml").await?,
        service_yaml: read_template(dir, "service.yaml").await?,
        pvc_yaml: read_template(dir, "pvc.yaml").await?,
    })
}

async fn read_template(dir: &Path, file: &str) -> Result<String> {
    let path = dir.join(file);
    tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed to read catalog template {}", path.display()))
}

/// A game id becomes the prefix of every instance name, so it must itself be a
/// valid RFC1123 label segment: lowercase alphanumerics and dashes, starting and
/// ending with an alphanumeric.
fn validate_game_id(id: &str) -> Result<()> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && id.ends_with(|c: char| c.is_ascii_alphanumeric());
    if !valid {
        bail!("game id '{id}' is not a valid lowercase RFC1123 name");
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/catalog.rs"]
mod tests;
