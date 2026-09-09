//! The per-game mod facts the bot reads off a game's `GameServer` template:
//! which registry its mods come from, which packages its image already supplies,
//! and which file an instance lists its mods in.
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

use crate::thunderstore::split_full_name;

/// Annotation naming the registry a game's mods come from.
const MOD_REGISTRY_ANNOTATION: &str = "grizzly-gameservers.grizzly-endeavors.com/mod-registry";

/// Annotation naming the Thunderstore community to search. Required by, and
/// only meaningful for, the `thunderstore` registry.
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

/// Container environment variable naming the instance's mod list, relative to
/// the data root. Read from the manifest rather than assumed, because the tools
/// that rewrite that file wholesale have to name the file the supervisor in that
/// pod will actually read.
const MODS_FILE_ENV: &str = "SUPERVISOR_MODS_FILE";

/// The registry `search_mods` and `mod_info` talk to for one game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModRegistry {
    /// Modrinth, searched globally — Minecraft's registry.
    Modrinth,
    /// A Thunderstore community, named by its slug. Every r2modman-shaped game
    /// has its own community and packages never cross between them, so the slug
    /// is part of the registry's identity rather than a search filter.
    Thunderstore {
        community: String,
        /// `Namespace-Name` packages the image supplies, which an instance must
        /// never list for itself. Empty for a game whose image supplies none.
        provided: Vec<String>,
    },
}

/// Read a game's registry off its `GameServer` annotations.
///
/// `Ok(None)` means the game declares no registry — the ordinary answer for a
/// game with no modding story, not a failure.
///
/// # Errors
///
/// Returns an error if the registry name is unknown, a `thunderstore` game names
/// no community, a community or provided list is declared without the registry
/// that uses it, or a provided entry isn't a `Namespace-Name` pair.
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

/// Split the provided-package annotation, rejecting anything that isn't the
/// `Namespace-Name` shape the reconciler matches on.
///
/// Strict rather than lenient: a misspelled entry here doesn't fail anything
/// visibly — it just stops matching the package it was meant to name, and the
/// loader pack quietly starts being written into mod lists again.
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

/// The mod list an instance of this game keeps its packages in, or `None` when
/// the game declares none (which is what turns the reconciler off).
///
/// # Errors
///
/// Returns an error if the template is not parseable YAML.
pub(crate) fn parse_mods_file_from_template(gameserver_yaml: &str) -> Result<Option<String>> {
    let template: AnnotatedTemplate = serde_yaml_ng::from_str(gameserver_yaml)?;
    Ok(template
        .spec
        .template
        .spec
        .containers
        .into_iter()
        .flat_map(|container| container.env)
        .find(|var| var.name == MODS_FILE_ENV)
        .and_then(|var| var.value)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty()))
}

/// [`parse_mods_file_from_template`], degraded to "no mod list" with a warning —
/// the same leniency the catalog loader applies to the annotations, and for the
/// same reason.
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

/// The only parts of a `GameServer` template this module reads. Deliberately
/// partial so an unrelated manifest edit can't perturb the parse.
#[derive(Default, serde::Deserialize)]
struct AnnotatedTemplate {
    #[serde(default)]
    metadata: TemplateMetadata,
    #[serde(default)]
    spec: TemplateSpec,
}

#[derive(Default, serde::Deserialize)]
struct TemplateMetadata {
    annotations: Option<BTreeMap<String, String>>,
}

#[derive(Default, serde::Deserialize)]
struct TemplateSpec {
    #[serde(default)]
    template: PodTemplate,
}

#[derive(Default, serde::Deserialize)]
struct PodTemplate {
    #[serde(default)]
    spec: PodSpec,
}

#[derive(Default, serde::Deserialize)]
struct PodSpec {
    #[serde(default)]
    containers: Vec<Container>,
}

#[derive(serde::Deserialize)]
struct Container {
    #[serde(default)]
    env: Vec<EnvVar>,
}

#[derive(serde::Deserialize)]
struct EnvVar {
    name: String,
    /// Absent for a `valueFrom` variable, which names a secret rather than a path.
    value: Option<String>,
}

#[cfg(test)]
#[path = "tests/mod_registry.rs"]
mod tests;
