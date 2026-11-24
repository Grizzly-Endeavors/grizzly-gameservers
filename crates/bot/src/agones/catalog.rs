use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::env_overlay::{EnvOverlay, env_overlay_or_warn};
use super::mod_registry::{ModRegistry, mod_registry_or_warn, mods_file_or_warn};

#[derive(Clone, Debug)]
pub(crate) struct GameCatalogEntry {
    pub(crate) id: String,
    pub(crate) gameserver_yaml: String,
    pub(crate) service_yaml: String,
    pub(crate) pvc_yaml: String,
    pub(crate) mod_registry: Option<ModRegistry>,
    pub(crate) mods_file: Option<String>,
    pub(crate) env_overlay: Option<EnvOverlay>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct GameCatalog {
    entries: BTreeMap<String, GameCatalogEntry>,
}

impl GameCatalog {
    pub(crate) fn get(&self, id: &str) -> Option<&GameCatalogEntry> {
        self.entries.get(id)
    }

    pub(crate) fn game_ids(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

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
    let gameserver_yaml = read_template(dir, "gameserver.yaml").await?;
    let mod_registry = mod_registry_or_warn(id, &gameserver_yaml);
    let mods_file = mods_file_or_warn(id, &gameserver_yaml);
    let env_overlay = env_overlay_or_warn(id, &gameserver_yaml);
    Ok(GameCatalogEntry {
        id: id.to_owned(),
        gameserver_yaml,
        service_yaml: read_template(dir, "service.yaml").await?,
        pvc_yaml: read_template(dir, "pvc.yaml").await?,
        mod_registry,
        mods_file,
        env_overlay,
    })
}

async fn read_template(dir: &Path, file: &str) -> Result<String> {
    let path = dir.join(file);
    tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed to read catalog template {}", path.display()))
}

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
