//! The tools that read and rewrite a server's whole mod list: adopting a group's
//! modpack from an r2modman profile code, minting a code that describes what a
//! server is running, and bumping every pinned version in the list at once.
//!
//! All three live here rather than in [`tools`](super::tools) because each is a
//! several-step conversation with a third party — fetch, classify, write; read,
//! compare, mint; read, look up, rewrite — and because the interesting decisions
//! in them are the *classifications* and the *rewrite*, which are pure and
//! tested next door in [`crate::thunderstore::modpack`] and
//! [`crate::thunderstore::updates`]. What is left in this file is the shell: the
//! cluster and registry calls, and the wording of the answers. They also share
//! the two things every mod-list tool needs first — the guard that confirms this
//! server's game speaks Thunderstore at all, and the read of one file off its
//! volume.
//!
//! **Export is read-only tier, and the cache is what makes that safe.** Minting a
//! code creates a permanent artifact on somebody else's service with no way to
//! delete it; without the cache, anyone who could ask a question could mint
//! unbounded numbers of them. With it, N friends asking produces one.
//!
//! **An update writes only when asked.** Every one of these tools is reached by
//! someone asking a question as often as by someone asking for a change, and
//! `update_mods` is the one where the two are hardest to tell apart — "are there
//! updates" and "update them" are the same sentence in most phrasings. So the
//! mode is an explicit argument with no default rather than a flag to forget:
//! checking reads the registry and touches nothing, applying rewrites the file.

use std::collections::BTreeMap;

use serenity::futures::StreamExt as _;
use tracing::{error, warn};

use grizzly_control_api::mods::{PROFILE_FILE, RESOLVED_FILE, ResolvedMods};
use grizzly_control_api::package::PackageId;
use poise::serenity_prelude as serenity;

use crate::agones::{
    FsOutcome, ModRegistry, ReadWindow, supervisor_read_file, supervisor_write_file,
};
use crate::prompts;
use crate::thunderstore::modpack::{
    CachedProfile, ImportPlan, Lookup, ModsDiff, diff_import, fingerprint, lookups_needed,
    plan_export, plan_import,
};
use crate::thunderstore::profile;
use crate::thunderstore::updates;
use crate::thunderstore::{create_profile, fetch_package, fetch_profile};

use super::tools::{ToolCtx, cluster_error, fs_result};

/// How many registry lookups an import runs at once. A profile may name up to
/// 256 packages and each needs one call to classify; sequential would take
/// minutes, and unbounded would point a burst at a public API that publishes no
/// rate limit and asks to be contacted rather than blocked.
const LOOKUP_CONCURRENCY: usize = 8;

/// Everything an r2modman-shaped server needs before any of these tools can
/// run: which Thunderstore catalog its mods come from, what its image already
/// supplies, and which file on its volume lists them.
pub(super) struct ThunderstoreTarget {
    pub(super) community: String,
    pub(super) provided: Vec<String>,
    pub(super) mods_file: String,
}

/// Resolve the server's game and confirm it is one whose mods come from a
/// Thunderstore community, or return the text to hand back instead.
pub(super) async fn target(ctx: &ToolCtx<'_>, server: &str) -> Result<ThunderstoreTarget, String> {
    let game = ctx.game_of(server).await?;
    let entry = game.as_deref().and_then(|game| ctx.data.catalog.get(game));
    let registry = entry.and_then(|entry| entry.mod_registry.as_ref());
    let mods_file = entry.and_then(|entry| entry.mods_file.clone());
    match (registry, mods_file) {
        (
            Some(ModRegistry::Thunderstore {
                community,
                provided,
            }),
            Some(mods_file),
        ) => Ok(ThunderstoreTarget {
            community: community.clone(),
            provided: provided.clone(),
            mods_file,
        }),
        _ => Err(prompts::ProfilesNotSupported {
            server,
            game: game.as_deref().unwrap_or("unknown game"),
        }
        .render()),
    }
}

/// Adopt a group's modpack from a profile code.
pub(crate) async fn exec_import_profile(ctx: &ToolCtx<'_>, server: &str, code: &str) -> String {
    let target = match target(ctx, server).await {
        Ok(target) => target,
        Err(refusal) => return refusal,
    };

    let payload = match fetch_profile(&ctx.data.http, code).await {
        Ok(Some(payload)) => payload,
        Ok(None) => return prompts::ProfileNotFound { code }.render(),
        Err(err) => {
            error!(error = ?err, %code, "agent: profile fetch failed");
            return prompts::ThunderstoreUnavailable::render();
        }
    };
    let mods = match profile::decode(&payload) {
        Ok(mods) => mods,
        Err(err) => {
            // A refusal, not a fault: the blob came from a third party and the
            // reason it can't be used is the useful half of the answer.
            warn!(error = ?err, %code, "agent: profile payload rejected");
            return prompts::ProfileUnreadable {
                code,
                reason: &format!("{err}"),
            }
            .render();
        }
    };

    let lookups = match classify(ctx, &target, &mods).await {
        Ok(lookups) => lookups,
        Err(err) => {
            error!(error = ?err, %code, "agent: profile classification failed");
            return prompts::ProfileRegistryUnavailable::render();
        }
    };
    let plan = plan_import(&mods, &target.provided, &lookups);

    // Writing an empty list here would uninstall everything the server runs on
    // the strength of a code whose entries this community has never heard of —
    // the one outcome nobody asking for a modpack could have wanted.
    if plan.listed() == 0 && !plan.missing.is_empty() {
        return prompts::ProfileNothingUsable {
            code,
            missing: &plan.missing.join(", "),
        }
        .render();
    }

    // Read before writing, because an import replaces the list rather than
    // adding to it: what it *removes* is the half nobody asking for a modpack
    // thinks to check, and after the write there is nothing left to compare
    // against. An absent list is an ordinary state and diffs as all-new; a read
    // that genuinely failed stops here, since the write about to follow would
    // fail the same way.
    let current = match read_text(ctx, server, &target.mods_file).await {
        Ok(text) => text.unwrap_or_default(),
        Err(problem) => return problem,
    };
    let diff = diff_import(&current, &plan);

    match supervisor_write_file(
        &ctx.data.kube_client,
        &ctx.data.control_http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        &target.mods_file,
        &plan.render(),
    )
    .await
    {
        Ok(outcome) => match fs_result(server, outcome) {
            Ok(result) => {
                if result.backed_up {
                    ctx.note_pending_change(server, &result.path);
                }
                format_import(server, &result.path, &plan, &diff)
            }
            Err(problem) => problem,
        },
        Err(err) => {
            error!(error = ?err, %server, "agent: import_profile write failed");
            cluster_error()
        }
    }
}

/// Bump every pinned version in a server's mod list to the newest the registry
/// reports, or just say what an update would move.
pub(crate) async fn exec_update_mods(ctx: &ToolCtx<'_>, server: &str, apply: bool) -> String {
    let target = match target(ctx, server).await {
        Ok(target) => target,
        Err(refusal) => return refusal,
    };

    let current = match read_text(ctx, server, &target.mods_file).await {
        Ok(Some(text)) => text,
        // No file, or an empty one: both mean there is nothing pinned to raise,
        // and saying so is a complete answer rather than a failure.
        Ok(None) => return prompts::ModsNoneListed { server }.render(),
        Err(problem) => return problem,
    };
    let wanted = updates::lookups_needed(&current);
    if wanted.is_empty() {
        return prompts::ModsNonePinned {
            server,
            path: &target.mods_file,
        }
        .render();
    }

    let latest = match latest_versions(ctx, &wanted).await {
        Ok(latest) => latest,
        Err(err) => {
            error!(error = ?err, %server, "agent: mod update lookups failed");
            return prompts::ThunderstoreUnavailable::render();
        }
    };
    let plan = updates::plan_update(&current, &latest);

    if plan.is_empty() {
        return prompts::ModsAlreadyCurrent {
            server,
            current: &plan.current.to_string(),
            floating: &list(&plan.floating),
            deprecated: &list(&plan.deprecated),
            unknown: &list(&plan.unknown),
        }
        .render();
    }
    if !apply {
        return format_update_check(server, &target.mods_file, &plan);
    }

    match supervisor_write_file(
        &ctx.data.kube_client,
        &ctx.data.control_http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        &target.mods_file,
        &plan.text,
    )
    .await
    {
        Ok(outcome) => match fs_result(server, outcome) {
            Ok(result) => {
                if result.backed_up {
                    ctx.note_pending_change(server, &result.path);
                }
                format_update_applied(server, &result.path, &plan)
            }
            Err(problem) => problem,
        },
        Err(err) => {
            error!(error = ?err, %server, "agent: update_mods write failed");
            cluster_error()
        }
    }
}

/// Ask the registry for the newest version of each pinned package.
///
/// A lookup that cannot be *reached* fails the whole update, for the same reason
/// an import's does: the call is cheap and idempotent, so retrying costs
/// nothing, whereas an update that silently skipped the packages it couldn't
/// check would report a list as up to date when it isn't — and that answer is
/// durable, since nobody asks twice.
async fn latest_versions(
    ctx: &ToolCtx<'_>,
    wanted: &[PackageId],
) -> anyhow::Result<BTreeMap<String, updates::Latest>> {
    let results: Vec<anyhow::Result<(String, updates::Latest)>> =
        serenity::futures::stream::iter(wanted.iter().map(ToString::to_string))
            .map(|full_name| async move {
                let Some((namespace, name)) = crate::thunderstore::split_full_name(&full_name)
                else {
                    return Ok((full_name.clone(), updates::Latest::Missing));
                };
                let found = fetch_package(&ctx.data.http, namespace, name).await?;
                let latest = found.map_or(updates::Latest::Missing, |package| {
                    package.latest.version_number.parse().map_or(
                        updates::Latest::Missing,
                        |version| updates::Latest::Known {
                            version,
                            deprecated: package.is_deprecated,
                        },
                    )
                });
                Ok((full_name, latest))
            })
            .buffer_unordered(LOOKUP_CONCURRENCY)
            .collect()
            .await;
    results.into_iter().collect()
}

/// What an update would move, with nothing written.
fn format_update_check(server: &str, path: &str, plan: &updates::UpdatePlan) -> String {
    prompts::ModsUpdatesAvailable {
        server,
        path,
        count: &plan.bumps().to_string(),
        bumped: &list(&plan.bumped),
        client_bumped: &list(&plan.client_bumped),
        current: &plan.current.to_string(),
        floating: &list(&plan.floating),
        deprecated: &list(&plan.deprecated),
        unknown: &list(&plan.unknown),
    }
    .render()
}

/// What an update wrote.
fn format_update_applied(server: &str, path: &str, plan: &updates::UpdatePlan) -> String {
    prompts::ModsUpdated {
        server,
        path,
        count: &plan.bumps().to_string(),
        bumped: &list(&plan.bumped),
        client_bumped: &list(&plan.client_bumped),
        current: &plan.current.to_string(),
        floating: &list(&plan.floating),
        deprecated: &list(&plan.deprecated),
        unknown: &list(&plan.unknown),
    }
    .render()
}

/// Ask the registry where each entry has to be installed.
///
/// A lookup that cannot be *reached* fails the whole import. Import is cheap and
/// idempotent, so retrying costs nothing — whereas a mod list written from
/// unclassified entries is durable, silently wrong, and looks like it worked.
async fn classify(
    ctx: &ToolCtx<'_>,
    target: &ThunderstoreTarget,
    mods: &[profile::ProfileMod],
) -> anyhow::Result<BTreeMap<String, Lookup>> {
    let names = lookups_needed(mods, &target.provided);
    let results: Vec<anyhow::Result<(String, Lookup)>> = serenity::futures::stream::iter(names)
        .map(|name| async move {
            let Some((namespace, package_name)) = crate::thunderstore::split_full_name(&name)
            else {
                return Ok((name.clone(), Lookup::Missing));
            };
            let found = fetch_package(&ctx.data.http, namespace, package_name).await?;
            let lookup = found.map_or(Lookup::Missing, |package| {
                Lookup::Known(package.side_support(&target.community))
            });
            Ok((name, lookup))
        })
        .buffer_unordered(LOOKUP_CONCURRENCY)
        .collect()
        .await;
    results.into_iter().collect()
}

/// Mint (or reuse) a code describing what this server's modpack is.
pub(crate) async fn exec_export_profile(ctx: &ToolCtx<'_>, server: &str) -> String {
    let target = match target(ctx, server).await {
        Ok(target) => target,
        Err(refusal) => return refusal,
    };
    let resolved_path = format!("{}/{RESOLVED_FILE}", work_dir());

    let resolved = match read_text(ctx, server, &resolved_path).await {
        Ok(Some(text)) => ResolvedMods::parse(&text),
        Ok(None) => return prompts::ProfileNotResolvedYet { server }.render(),
        Err(problem) => return problem,
    };
    let mods = plan_export(&resolved);
    if mods.is_empty() {
        return prompts::ProfileNothingToExport { server }.render();
    }

    let cache_path = format!("{}/{PROFILE_FILE}", work_dir());
    let cached = match read_text(ctx, server, &cache_path).await {
        Ok(Some(text)) => CachedProfile::parse(&text),
        // A cache that can't be read costs one extra mint, never the export.
        Ok(None) => None,
        Err(problem) => {
            warn!(%server, problem, "agent: could not read the cached profile code");
            None
        }
    };
    if let Some(cached) = &cached
        && cached.describes(&mods)
    {
        return format_export(server, &cached.code, &mods);
    }

    let payload = match profile::encode(server, &mods) {
        Ok(payload) => payload,
        Err(err) => {
            error!(error = ?err, %server, "agent: could not build a profile payload");
            return cluster_error();
        }
    };
    let code = match create_profile(&ctx.data.http, payload).await {
        Ok(code) => code,
        Err(err) => {
            error!(error = ?err, %server, "agent: profile mint failed");
            return prompts::ThunderstoreUnavailable::render();
        }
    };
    store_code(ctx, server, &cache_path, &code, &mods).await;
    // `target` is consumed only for its side of the guard above; naming it here
    // keeps the community in the log line that matters when a code looks wrong.
    tracing::info!(%server, community = %target.community, "agent: minted a profile code");
    format_export(server, &code, &mods)
}

/// The reconciler's working directory, as both halves agree it is.
fn work_dir() -> &'static str {
    grizzly_control_api::mods::DEFAULT_WORK_DIR
}

/// Read one file off the instance. `Ok(None)` means it isn't there yet, which is
/// an ordinary state for every file this module reads.
pub(super) async fn read_text(
    ctx: &ToolCtx<'_>,
    server: &str,
    path: &str,
) -> Result<Option<String>, String> {
    match supervisor_read_file(
        &ctx.data.kube_client,
        &ctx.data.control_http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        path,
        // Whole file, in bytes: this is parsed, not read, so a line window would
        // silently hand the parser a prefix of a long mod list.
        ReadWindow::Whole,
    )
    .await
    {
        // A rejection is how "no such file" arrives, and both files this module
        // reads are legitimately absent until the reconciler or an export has
        // run once. Logged all the same: a rejection for any *other* reason
        // reads to the caller as "not there yet", which would be a wrong answer
        // given confidently.
        Ok(FsOutcome::Rejected(reason)) => {
            tracing::debug!(%server, %path, %reason, "agent: profile file unavailable");
            Ok(None)
        }
        Ok(outcome) => fs_result(server, outcome).map(|file| Some(file.content)),
        Err(err) => {
            error!(error = ?err, %server, %path, "agent: profile file read failed");
            Err(cluster_error())
        }
    }
}

/// Record the minted code beside `resolved`, so the next asker gets this one
/// back instead of minting another permanent artifact.
async fn store_code(
    ctx: &ToolCtx<'_>,
    server: &str,
    path: &str,
    code: &str,
    mods: &[profile::ProfileMod],
) {
    let cached = CachedProfile {
        code: code.to_owned(),
        entries: fingerprint(mods),
    };
    match supervisor_write_file(
        &ctx.data.kube_client,
        &ctx.data.control_http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        path,
        &cached.render(),
    )
    .await
    {
        // Deliberately *not* noted as a pending change: this is the reconciler's
        // own working tree, not a config file, so a following restart has nothing
        // here to verify or roll back.
        Ok(FsOutcome::Ok(_)) => {}
        // The outcome's own text is the useful half — a rejected path or an
        // unreachable pod each mean something different for the next export.
        Ok(other) => warn!(
            %server,
            reason = fs_result(server, other).err().unwrap_or_default(),
            "agent: could not cache the profile code; the next export will mint another"
        ),
        Err(err) => error!(
            error = ?err,
            %server,
            "agent: could not cache the profile code; the next export will mint another"
        ),
    }
}

/// What an import changed, what it wrote, and what it left out and why.
fn format_import(server: &str, path: &str, plan: &ImportPlan, diff: &ModsDiff) -> String {
    prompts::ProfileImported {
        server,
        path,
        added: &list(&diff.added),
        removed: &list(&diff.removed),
        changed: &list(&diff.changed),
        moved: &list(&diff.moved),
        unchanged: &diff.unchanged.to_string(),
        // Said outright rather than left to be inferred from four empty lines,
        // because "the server already runs this pack" is a complete answer and
        // reading it out of absences invites reporting a change that didn't
        // happen.
        identical: if diff.is_empty() {
            "The list is unchanged — the server was already running exactly this pack."
        } else {
            ""
        },
        installs: &list(&plan.install),
        client_only: &list(&plan.client_only),
        disabled: &list(&plan.disabled),
        provided: &list(&plan.provided),
        missing: &list(&plan.missing),
    }
    .render()
}

/// The code, and exactly what it contains — said out loud because a code is a
/// snapshot of the pack at this moment and there is no way to revoke one.
fn format_export(server: &str, code: &str, mods: &[profile::ProfileMod]) -> String {
    prompts::ProfileExported {
        server,
        code,
        count: &mods.len().to_string(),
        mods: &list(&fingerprint(mods)),
    }
    .render()
}

/// Comma-joined, or the value-level `(none)` an empty list renders as.
fn list(entries: &[String]) -> String {
    crate::registry::list_or_none(entries)
}

#[cfg(test)]
#[path = "tests/mod_list.rs"]
mod tests;
