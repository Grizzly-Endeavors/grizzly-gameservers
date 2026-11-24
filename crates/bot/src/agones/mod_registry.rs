use std::collections::BTreeMap;

use anyhow::{Result, bail};
use tracing::warn;

use super::template::AnnotatedTemplate;
use crate::thunderstore::split_full_name;

const MOD_REGISTRY_ANNOTATION: &str = "grizzly-gameservers.grizzly-endeavors.com/mod-registry";

const THUNDERSTORE_COMMUNITY_ANNOTATION: &str =
    "grizzly-gameservers.grizzly-endeavors.com/thunderstore-community";

/// Annotation listing the packages the game image already supplies, comma
/// separated as `Namespace-Name`. Optional, and only meaningful for the
/// `thunderstore` registry.
///
/// The same fact the image states as `SUPERVISOR_THUNDERSTORE_PROVIDED`, which
/// the bot cannot see: a Dockerfile `ENV` is baked into the image and never
/// appears on the pod. It is restated here because the bot needs it at *write*
/// time — an imported profile lists the loader pack, and writing that into a mod
/// list would put a line in the file that this game's own guidance says never to
/// write. The two copies are held together by a drift guard in
/// `crates/bot/src/tests/config.rs`, the same arrangement the community slug
/// already lives under.
const THUNDERSTORE_PROVIDED_ANNOTATION: &str =
    "grizzly-gameservers.grizzly-endeavors.com/thunderstore-provided";

const MODS_FILE_ENV: &str = "SUPERVISOR_MODS_FILE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModRegistry {
    Modrinth,
    Thunderstore {
        community: String,
        provided: Vec<String>,
    },
}

pub(crate) fn parse_mod_registry(
    annotations: Option<&BTreeMap<String, String>>,
) -> Result<Option<ModRegistry>> {
    let registry = annotations
        .and_then(|map| map.get(MOD_REGISTRY_ANNOTATION))
        .map(|value| value.trim());
    let community = annotations
        .and_then(|map| map.get(THUNDERSTORE_COMMUNITY_ANNOTATION))
        .map(|value| value.trim());
    let provided = annotations
        .and_then(|map| map.get(THUNDERSTORE_PROVIDED_ANNOTATION))
        .map(|value| value.trim());

    match (registry, community) {
        (None, None) if provided.is_some() => bail!(
            "`{THUNDERSTORE_PROVIDED_ANNOTATION}` is set without \
             `{MOD_REGISTRY_ANNOTATION}: thunderstore`"
        ),
        (None, None) => Ok(None),
        (None, Some(_)) => bail!(
            "`{THUNDERSTORE_COMMUNITY_ANNOTATION}` is set without \
             `{MOD_REGISTRY_ANNOTATION}: thunderstore`"
        ),
        (Some("modrinth"), None) => {
            if provided.is_some() {
                bail!("`{THUNDERSTORE_PROVIDED_ANNOTATION}` is meaningless on a modrinth game");
            }
            Ok(Some(ModRegistry::Modrinth))
        }
        (Some("modrinth"), Some(_)) => {
            bail!("`{THUNDERSTORE_COMMUNITY_ANNOTATION}` is meaningless on a modrinth game")
        }
        (Some("thunderstore"), None) => bail!(
            "`{MOD_REGISTRY_ANNOTATION}: thunderstore` requires \
             `{THUNDERSTORE_COMMUNITY_ANNOTATION}`"
        ),
        (Some("thunderstore"), Some("")) => {
            bail!("`{THUNDERSTORE_COMMUNITY_ANNOTATION}` is set but empty")
        }
        (Some("thunderstore"), Some(community)) => Ok(Some(ModRegistry::Thunderstore {
            community: community.to_owned(),
            provided: parse_provided(provided)?,
        })),
        (Some(other), _) => bail!(
            "`{MOD_REGISTRY_ANNOTATION}` = `{other}` is not a registry this bot knows \
             (modrinth, thunderstore)"
        ),
    }
}

pub(crate) fn parse_mod_registry_from_template(
    gameserver_yaml: &str,
) -> Result<Option<ModRegistry>> {
    parse_mod_registry(AnnotatedTemplate::parse(gameserver_yaml)?.annotations())
}

pub(crate) fn mod_registry_or_warn(id: &str, gameserver_yaml: &str) -> Option<ModRegistry> {
    match parse_mod_registry_from_template(gameserver_yaml) {
        Ok(registry) => registry,
        Err(err) => {
            warn!(
                error = ?err,
                game = %id,
                "catalog: unusable mod-registry annotations; this game's mod tools are off"
            );
            None
        }
    }
}

fn parse_provided(raw: Option<&str>) -> Result<Vec<String>> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut provided = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        if split_full_name(entry).is_none() {
            bail!(
                "`{THUNDERSTORE_PROVIDED_ANNOTATION}` entry `{entry}` is not a \
                 Namespace-Name package"
            );
        }
        provided.push(entry.to_owned());
    }
    Ok(provided)
}

pub(crate) fn parse_mods_file_from_template(gameserver_yaml: &str) -> Result<Option<String>> {
    Ok(AnnotatedTemplate::parse(gameserver_yaml)?
        .env_value(MODS_FILE_ENV)
        .map(str::to_owned))
}

pub(crate) fn mods_file_or_warn(id: &str, gameserver_yaml: &str) -> Option<String> {
    match parse_mods_file_from_template(gameserver_yaml) {
        Ok(file) => file,
        Err(err) => {
            warn!(
                error = ?err,
                game = %id,
                "catalog: unreadable GameServer template; this game's mod list is unknown"
            );
            None
        }
    }
}

#[cfg(test)]
#[path = "tests/mod_registry.rs"]
mod tests;
