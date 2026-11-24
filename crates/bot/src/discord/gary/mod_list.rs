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

pub(super) struct ThunderstoreTarget {
    pub(super) community: String,
    pub(super) provided: Vec<String>,
    pub(super) mods_file: String,
}

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

    if plan.listed() == 0 && !plan.missing.is_empty() {
        return prompts::ProfileNothingUsable {
            code,
            missing: &plan.missing.join(", "),
        }
        .render();
    }

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

pub(crate) async fn exec_update_mods(ctx: &ToolCtx<'_>, server: &str, apply: bool) -> String {
    let target = match target(ctx, server).await {
        Ok(target) => target,
        Err(refusal) => return refusal,
    };

    let current = match read_text(ctx, server, &target.mods_file).await {
        Ok(Some(text)) => text,
        Ok(None) => String::new(),
        Err(problem) => return problem,
    };
    let wanted = updates::lookups_needed(&current);
    if wanted.is_empty() {
        if current.trim().is_empty() {
            return prompts::ModsNoneListed { server }.render();
        }
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
    tracing::info!(%server, community = %target.community, "agent: minted a profile code");
    format_export(server, &code, &mods)
}

fn work_dir() -> &'static str {
    grizzly_control_api::mods::DEFAULT_WORK_DIR
}

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
        ReadWindow::Whole,
    )
    .await
    {
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
        Ok(FsOutcome::Ok(_)) => {}
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

fn format_import(server: &str, path: &str, plan: &ImportPlan, diff: &ModsDiff) -> String {
    prompts::ProfileImported {
        server,
        path,
        added: &list(&diff.added),
        removed: &list(&diff.removed),
        changed: &list(&diff.changed),
        moved: &list(&diff.moved),
        unchanged: &diff.unchanged.to_string(),
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

fn format_export(server: &str, code: &str, mods: &[profile::ProfileMod]) -> String {
    prompts::ProfileExported {
        server,
        code,
        count: &mods.len().to_string(),
        mods: &list(&fingerprint(mods)),
    }
    .render()
}

fn list(entries: &[String]) -> String {
    crate::registry::list_or_none(entries)
}

#[cfg(test)]
#[path = "tests/mod_list.rs"]
mod tests;
