//! Which mod registry a game's mods come from, declared on the game's
//! `GameServer` template annotations.
//!
//! The catalog holds each game's templates as raw strings, so this is the first
//! piece of *structured* per-game metadata the bot reads out of them; it follows
//! the precedent set by the port model in [`ports`](super::ports), which is
//! likewise declared as annotations and parsed rather than hardcoded per game.
//! Declaring it on the manifest rather than in Rust is what keeps onboarding a
//! new Thunderstore game a catalog edit instead of a bot release.
//!
//! Two faces, deliberately: [`parse_mod_registry`] is strict, because a
//! half-configured annotation pair is a mistake nobody wants silently rounded
//! down to "no mods"; [`mod_registry_or_warn`] wraps it leniently for the
//! catalog loader, because a typo in one game's manifest must not stop the bot
//! from starting and serving the other five.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use tracing::warn;

/// Annotation naming the registry a game's mods come from.
const MOD_REGISTRY_ANNOTATION: &str = "grizzly-gameservers.grizzly-endeavors.com/mod-registry";

/// Annotation naming the Thunderstore community to search. Required by, and
/// only meaningful for, the `thunderstore` registry.
const THUNDERSTORE_COMMUNITY_ANNOTATION: &str =
    "grizzly-gameservers.grizzly-endeavors.com/thunderstore-community";

/// The registry `search_mods` and `mod_info` talk to for one game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModRegistry {
    /// Modrinth, searched globally — Minecraft's registry.
    Modrinth,
    /// A Thunderstore community, named by its slug. Every r2modman-shaped game
    /// has its own community and packages never cross between them, so the slug
    /// is part of the registry's identity rather than a search filter.
    Thunderstore { community: String },
}

/// Read a game's registry off its `GameServer` annotations.
///
/// `Ok(None)` means the game declares no registry — the ordinary answer for a
/// game with no modding story, not a failure.
///
/// # Errors
///
/// Returns an error if the registry name is unknown, a `thunderstore` game names
/// no community, or a community is declared without the registry that uses it.
/// A half-configured pair must fail loudly rather than silently reading as "no
/// mods": the symptom of the silent version is Gary telling a friend their game
/// takes no mods, which is indistinguishable from the truth.
pub(crate) fn parse_mod_registry(
    annotations: Option<&BTreeMap<String, String>>,
) -> Result<Option<ModRegistry>> {
    let registry = annotations
        .and_then(|map| map.get(MOD_REGISTRY_ANNOTATION))
        .map(|value| value.trim());
    let community = annotations
        .and_then(|map| map.get(THUNDERSTORE_COMMUNITY_ANNOTATION))
        .map(|value| value.trim());

    match (registry, community) {
        (None, None) => Ok(None),
        (None, Some(_)) => bail!(
            "`{THUNDERSTORE_COMMUNITY_ANNOTATION}` is set without \
             `{MOD_REGISTRY_ANNOTATION}: thunderstore`"
        ),
        (Some("modrinth"), None) => Ok(Some(ModRegistry::Modrinth)),
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
        })),
        (Some(other), _) => bail!(
            "`{MOD_REGISTRY_ANNOTATION}` = `{other}` is not a registry this bot knows \
             (modrinth, thunderstore)"
        ),
    }
}

/// Read a game's registry straight off its raw `GameServer` template.
///
/// # Errors
///
/// Returns an error if the template is not parseable YAML, or its annotations
/// fail [`parse_mod_registry`].
pub(crate) fn parse_mod_registry_from_template(
    gameserver_yaml: &str,
) -> Result<Option<ModRegistry>> {
    let template: AnnotatedTemplate = serde_yaml_ng::from_str(gameserver_yaml)?;
    parse_mod_registry(template.metadata.annotations.as_ref())
}

/// [`parse_mod_registry_from_template`], degraded to "no registry" with a
/// warning. This is the face the catalog loader uses: a malformed annotation on
/// one game costs that game its mod tools, and must not cost every other guild
/// its bot.
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

/// The only part of a `GameServer` template this module reads. Deliberately
/// partial so an unrelated manifest edit can't perturb the parse.
#[derive(serde::Deserialize)]
struct AnnotatedTemplate {
    #[serde(default)]
    metadata: TemplateMetadata,
}

#[derive(Default, serde::Deserialize)]
struct TemplateMetadata {
    annotations: Option<BTreeMap<String, String>>,
}

#[cfg(test)]
#[path = "tests/mod_registry.rs"]
mod tests;
