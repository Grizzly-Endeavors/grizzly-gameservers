//! Gary's tool surface: the lifecycle operations exposed to the model, their
//! parameter schemas, the admin tiering, and the dispatcher that runs a call and
//! renders a compact text result for the model to relay. The results are plain
//! text on purpose — Gary composes the friendly Discord reply himself.

use poise::serenity_prelude as serenity;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serenity::{
    ButtonStyle, ComponentInteractionCollector, CreateActionRow, CreateButton,
    CreateInteractionResponse, CreateMessage, EditMessage,
};
use tracing::{error, info, warn};

use grizzly_control_api::{
    CommandResponse, DirEntry, EntryKind, ReadResponse, RestoreResponse, WriteResponse,
};

use super::recovery::{PendingChange, RecoveryStep, next_step};

use super::super::auth::AccessLevel;
use super::super::render::{
    archive_confirm_embed, archive_result_embed, destroy_confirm_embed, destroy_result_embed,
    error_embed, human_size, neutral_embed, restore_confirm_embed, restore_result_embed,
};
use super::super::{COMPONENT_TIMEOUT, Data, backup_ctx};
use crate::agent::{ToolCall, ToolDef};
use crate::agones::{
    DestroyOutcome, EditOutcome, FsOutcome, ProvisionOutcome, ReadyWait, Replacement, RuntimeState,
    ScopeVerdict, ServerScope, ServerSummary, ShutdownOutcome, StartBegin, SupervisorOutcome,
    begin_start, build_instance_name, destroy_instance, instance_runtime_state,
    list_active_servers, now_entropy, provision_instance, shutdown_instance, supervisor_announce,
    supervisor_edit_file, supervisor_list_files, supervisor_occupancy, supervisor_read_file,
    supervisor_read_logs, supervisor_restart, supervisor_restore_file, supervisor_send_command,
    supervisor_start, supervisor_stop, supervisor_write_file, verify_scope, wait_for_ready,
};
use crate::backup::{
    ArchiveOutcome, ArtifactSummary, BackupOutcome, BootState, RecoverOutcome, RestoreOutcome,
};
use crate::defer::{Condition, DeferredTask};
use crate::memory::{ForgetOutcome, RememberOutcome, normalize_scope};
use crate::notify::Escalation;
use crate::prompts::{
    self, ArchiveServer, BackupServer, BrowseFiles, CreateServer, CreateServerParams,
    DestroyServer, EditFile, EditFileParams, Forget, ForgetParams, ListArchives, ListBackups,
    ListServers, NameParams, PathParams, ReadFile, ReadLogs, ReadLogsParams, RecoverServer,
    Remember, RememberParams, RestartServer, RestoreFile, RestoreServer, RunWhen, RunWhenParams,
    SendCommand, SendCommandParams, ServerStatus, ShutdownServer, StartServer, StopServer,
    WriteFile, WriteFileParams,
};

/// Everything a tool executor needs: the shared bot state plus the Discord
/// handles the destructive-confirmation flow uses, and the caller's access tier
/// (so mutating tools can refuse at execution time as defense in depth).
pub(crate) struct ToolCtx<'a> {
    pub(crate) data: &'a Data,
    pub(crate) serenity: &'a serenity::Context,
    pub(crate) channel_id: serenity::ChannelId,
    /// The guild this conversation is in, stamped on a server Gary creates so it's
    /// owned by that guild. `None` in an operator's DM — a server created there is
    /// left unlabeled (operator-only), matching the pre-scoping convention.
    pub(crate) guild: Option<u64>,
    pub(crate) author_id: serenity::UserId,
    pub(crate) access: AccessLevel,
    /// The servers this caller may see and act on — every tool that targets an
    /// existing server by name is gated on it in [`dispatch`], and the listing
    /// tools query within it.
    pub(crate) scope: ServerScope,
    /// The last config edit that saved a snapshot and hasn't been verified by a
    /// restart yet, tracked for the duration of one Gary turn. When a restart
    /// applies this change, [`exec_restart`] watches it come back up and rolls it
    /// back on a crash — deterministic recovery that doesn't depend on the model.
    /// Guarded by a plain mutex because dispatch runs the tools one at a time and
    /// never holds the lock across an await.
    pub(crate) pending_change: std::sync::Mutex<Option<PendingChange>>,
}

impl ToolCtx<'_> {
    /// Record that `path` on `server` was just edited with a snapshot saved, so a
    /// following restart knows there's a change to verify and, if it crashes, undo.
    /// The last edit wins, mirroring `restore_file`'s own last-write-per-file model.
    fn note_pending_change(&self, server: &str, path: &str) {
        *self.pending_lock() = Some(PendingChange {
            server: server.to_owned(),
            path: path.to_owned(),
        });
    }

    /// Take the pending change if it targets `server`, leaving an edit awaiting a
    /// different server's restart in place. `Some` means this restart is the one
    /// that applies a tracked change and should be watched for a crash.
    fn take_pending_change(&self, server: &str) -> Option<PendingChange> {
        let mut guard = self.pending_lock();
        if guard.as_ref().is_some_and(|change| change.server == server) {
            guard.take()
        } else {
            None
        }
    }

    /// Drop any pending change for `path` on `server` — the model restored the file
    /// itself, so there's no longer an unverified edit for a later restart to undo.
    fn drop_pending_change(&self, server: &str, path: &str) {
        let mut guard = self.pending_lock();
        if guard
            .as_ref()
            .is_some_and(|change| change.server == server && change.path == path)
        {
            *guard = None;
        }
    }

    /// Lock the pending-change slot, recovering from a poisoned mutex rather than
    /// panicking — a poison here would only mean a prior panic while the lock was
    /// held, and the tracked state is advisory, so the last value is safe to reuse.
    fn pending_lock(&self) -> std::sync::MutexGuard<'_, Option<PendingChange>> {
        self.pending_change
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Just the `name` field, pulled from any targeted tool's arguments (they all
/// carry one) for the scope gate in [`dispatch`], ignoring the rest.
#[derive(Deserialize)]
struct TargetName {
    name: String,
}

/// The tools advertised to the model for a given caller. Everyone gets the
/// read-only set; managers additionally get the lifecycle and file-tuning tools;
/// admins additionally get the destructive tools and console commands.
pub(crate) fn available_tools(access: AccessLevel) -> Vec<ToolDef> {
    let mut tools: Vec<ToolDef> = vec![
        ListServers::spec().into(),
        ServerStatus::spec().into(),
        ListBackups::spec().into(),
        ListArchives::spec().into(),
    ];
    if access >= AccessLevel::Manager {
        tools.extend(manager_tools());
    }
    if access >= AccessLevel::Admin {
        tools.extend(admin_only_tools());
    }
    tools
}

/// The lifecycle and file-tuning tools offered to managers and admins — the
/// day-to-day operations, none of which permanently destroy a world.
fn manager_tools() -> Vec<ToolDef> {
    vec![
        CreateServer::spec().into(),
        StopServer::spec().into(),
        StartServer::spec().into(),
        RestartServer::spec().into(),
        ShutdownServer::spec().into(),
        BrowseFiles::spec().into(),
        ReadFile::spec().into(),
        ReadLogs::spec().into(),
        EditFile::spec().into(),
        WriteFile::spec().into(),
        RestoreFile::spec().into(),
        RunWhen::spec().into(),
        BackupServer::spec().into(),
        Remember::spec().into(),
        Forget::spec().into(),
    ]
}

/// The destructive and heavy-handed tools offered only to admin callers:
/// permanent deletion, world overwrites, archival, and live console commands.
/// The "do not confirm" steer on `destroy_server` and the confirmation phrasing
/// on `archive_server`/`restore_server` live in those tools' prompt files.
fn admin_only_tools() -> Vec<ToolDef> {
    vec![
        DestroyServer::spec().into(),
        ArchiveServer::spec().into(),
        RestoreServer::spec().into(),
        RecoverServer::spec().into(),
        SendCommand::spec().into(),
    ]
}

/// Run one tool call and return the text result to feed back to the model. Bad
/// arguments, unknown names, and non-admin attempts at mutating tools all return
/// an explanatory string rather than failing the loop.
///
/// Before dispatching, any tool that targets an existing server by name is
/// confined to the caller's [`ServerScope`](ToolCtx::scope): a server in another
/// guild reads as "no such server", so Gary can neither see nor touch another
/// group's servers. The tools not gated here each enforce tenancy on their own:
/// `list_servers` and `list_archives` scope-filter their own listing,
/// `create_server` stamps the new server with the current guild, and
/// `recover_server` resolves the archive within the caller's scope-filtered
/// listing (see `exec_recover`).
pub(crate) async fn dispatch(ctx: &ToolCtx<'_>, call: &ToolCall) -> String {
    let args = call.function.arguments.as_str();
    if targets_existing_server(call.function.name.as_str())
        && let Ok(TargetName { name }) = serde_json::from_str::<TargetName>(args)
        && let Some(refusal) = scope_refusal(ctx, &name).await
    {
        return refusal;
    }
    let name = call.function.name.as_str();
    match name {
        ListServers::NAME => exec_list_servers(ctx).await,
        ServerStatus::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_server_status(ctx, &params.name).await,
            Err(message) => message,
        },
        ListBackups::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_list_backups(ctx, &params.name).await,
            Err(message) => message,
        },
        ListArchives::NAME => exec_list_archives(ctx).await,
        // Memory tools carry no server name (memory is shared across guilds), so
        // they skip the scope gate above and dispatch on their own.
        Remember::NAME | Forget::NAME => dispatch_memory(ctx, name, args).await,
        _ => dispatch_mutating(ctx, name, args).await,
    }
}

/// Dispatch the memory tools. Manager-gated like the mutating set (defense in
/// depth — they aren't offered below manager either), but kept out of
/// [`dispatch_mutating`] because they target no server and take no scope gate.
async fn dispatch_memory(ctx: &ToolCtx<'_>, name: &str, args: &str) -> String {
    if ctx.access < AccessLevel::Manager {
        return prompts::NonManagerRefusal::render();
    }
    match name {
        Remember::NAME => match parse::<RememberParams>(args) {
            Ok(params) => exec_remember(ctx, &params.scope, &params.note).await,
            Err(message) => message,
        },
        Forget::NAME => match parse::<ForgetParams>(args) {
            Ok(params) => exec_forget(ctx, params.id).await,
            Err(message) => message,
        },
        other => prompts::UnknownTool { name: other }.render(),
    }
}

/// Dispatch the mutating tools — the manager-tier lifecycle/file set and the
/// admin-only destructive set — gating each on the caller's tier and rejecting
/// unknown names. Split out of [`dispatch`] so each stays under the line cap.
/// The tier guards mirror [`available_tools`]: a tool the caller couldn't be
/// offered falls through to the tier-appropriate refusal (defense in depth).
async fn dispatch_mutating(ctx: &ToolCtx<'_>, name: &str, args: &str) -> String {
    let manager = ctx.access >= AccessLevel::Manager;
    let admin = ctx.access >= AccessLevel::Admin;
    match name {
        CreateServer::NAME if manager => match parse::<CreateServerParams>(args) {
            Ok(params) => exec_create(ctx, &params.game, params.name.as_deref()).await,
            Err(message) => message,
        },
        StopServer::NAME if manager => match parse::<NameParams>(args) {
            Ok(params) => exec_stop(ctx, &params.name).await,
            Err(message) => message,
        },
        StartServer::NAME if manager => match parse::<NameParams>(args) {
            Ok(params) => exec_start(ctx, &params.name).await,
            Err(message) => message,
        },
        RestartServer::NAME if manager => match parse::<NameParams>(args) {
            Ok(params) => exec_restart(ctx, &params.name).await,
            Err(message) => message,
        },
        ShutdownServer::NAME if manager => match parse::<NameParams>(args) {
            Ok(params) => exec_shutdown(ctx, &params.name).await,
            Err(message) => message,
        },
        BrowseFiles::NAME if manager => match parse::<PathParams>(args) {
            Ok(params) => exec_browse_files(ctx, &params.name, &params.path).await,
            Err(message) => message,
        },
        ReadFile::NAME if manager => match parse::<PathParams>(args) {
            Ok(params) => exec_read_file(ctx, &params.name, &params.path).await,
            Err(message) => message,
        },
        ReadLogs::NAME if manager => match parse::<ReadLogsParams>(args) {
            Ok(params) => match narrow_lines(params.lines) {
                Ok(lines) => exec_read_logs(ctx, &params.name, lines).await,
                Err(message) => message,
            },
            Err(message) => message,
        },
        WriteFile::NAME if manager => match parse::<WriteFileParams>(args) {
            Ok(params) => exec_write_file(ctx, &params.name, &params.path, &params.content).await,
            Err(message) => message,
        },
        EditFile::NAME if manager => match parse::<EditFileParams>(args) {
            Ok(params) => {
                exec_edit_file(
                    ctx,
                    &params.name,
                    &params.path,
                    &params.old_text,
                    &params.new_text,
                )
                .await
            }
            Err(message) => message,
        },
        RestoreFile::NAME if manager => match parse::<PathParams>(args) {
            Ok(params) => exec_restore_file(ctx, &params.name, &params.path).await,
            Err(message) => message,
        },
        RunWhen::NAME if manager => match parse::<RunWhenParams>(args) {
            Ok(params) => {
                exec_run_when(ctx, &params.name, params.condition.into(), &params.task).await
            }
            Err(message) => message,
        },
        BackupServer::NAME if manager => match parse::<NameParams>(args) {
            Ok(params) => exec_backup(ctx, &params.name).await,
            Err(message) => message,
        },
        DestroyServer::NAME if admin => match parse::<NameParams>(args) {
            Ok(params) => exec_destroy(ctx, &params.name).await,
            Err(message) => message,
        },
        SendCommand::NAME if admin => match parse::<SendCommandParams>(args) {
            Ok(params) => exec_send_command(ctx, &params.name, &params.command).await,
            Err(message) => message,
        },
        ArchiveServer::NAME if admin => match parse::<NameParams>(args) {
            Ok(params) => exec_archive(ctx, &params.name).await,
            Err(message) => message,
        },
        RestoreServer::NAME if admin => match parse::<NameParams>(args) {
            Ok(params) => exec_restore(ctx, &params.name).await,
            Err(message) => message,
        },
        RecoverServer::NAME if admin => match parse::<NameParams>(args) {
            Ok(params) => exec_recover(ctx, &params.name).await,
            Err(message) => message,
        },
        // A known tool the caller's tier couldn't be offered (defense in depth),
        // or an unknown name — classified into the right refusal.
        other => tier_refusal(other),
    }
}

/// The response for a mutating tool that fell through every guarded arm: a
/// tier-appropriate refusal for a real-but-out-of-tier tool (defense in depth —
/// these aren't offered below their tier), or the unknown-tool message otherwise.
fn tier_refusal(name: &str) -> String {
    if matches!(
        name,
        DestroyServer::NAME
            | SendCommand::NAME
            | ArchiveServer::NAME
            | RestoreServer::NAME
            | RecoverServer::NAME
    ) {
        prompts::NonAdminRefusal::render()
    } else if matches!(
        name,
        CreateServer::NAME
            | StopServer::NAME
            | StartServer::NAME
            | RestartServer::NAME
            | ShutdownServer::NAME
            | BrowseFiles::NAME
            | ReadFile::NAME
            | ReadLogs::NAME
            | WriteFile::NAME
            | EditFile::NAME
            | RestoreFile::NAME
            | RunWhen::NAME
            | BackupServer::NAME
    ) {
        prompts::NonManagerRefusal::render()
    } else {
        prompts::UnknownTool { name }.render()
    }
}

fn parse<T: DeserializeOwned>(args: &str) -> Result<T, String> {
    serde_json::from_str(args).map_err(|err| {
        prompts::BadToolArguments {
            error: &err.to_string(),
        }
        .render()
    })
}

/// Narrow the model-supplied line count to the unsigned window `read_logs` wants.
/// The generated schema carries `lines` as a signed integer (the v1 tool vocab has
/// no unsigned type), so a negative value is possible on the wire; refuse it with a
/// message the model can act on rather than erroring the whole loop.
fn narrow_lines(lines: Option<i64>) -> Result<Option<usize>, String> {
    match lines {
        None => Ok(None),
        Some(count) => match usize::try_from(count) {
            Ok(narrowed) => Ok(Some(narrowed)),
            Err(_) => Err(prompts::NegativeLineCount::render()),
        },
    }
}

/// Whether a tool acts on an *existing* server named in its arguments — the set
/// the scope gate applies to. Excluded because they enforce tenancy themselves:
/// `list_servers` and `list_archives` (scope-filtered listings), `create_server`
/// (no existing target — stamps the current guild), and `recover_server` (resolves
/// the archive within the caller's scope). Keep this in sync with those tools.
fn targets_existing_server(tool: &str) -> bool {
    matches!(
        tool,
        ServerStatus::NAME
            | StopServer::NAME
            | StartServer::NAME
            | RestartServer::NAME
            | ShutdownServer::NAME
            | DestroyServer::NAME
            | BrowseFiles::NAME
            | ReadFile::NAME
            | ReadLogs::NAME
            | WriteFile::NAME
            | EditFile::NAME
            | RestoreFile::NAME
            | SendCommand::NAME
            | RunWhen::NAME
            | ListBackups::NAME
            | BackupServer::NAME
            | ArchiveServer::NAME
            | RestoreServer::NAME
    )
}

/// `None` if `server` is reachable in the caller's scope, else the text to hand
/// back to the model instead of running the tool. A foreign server is reported
/// as missing, identically to one that truly doesn't exist.
async fn scope_refusal(ctx: &ToolCtx<'_>, server: &str) -> Option<String> {
    match verify_scope(
        &ctx.data.kube_client,
        &ctx.data.namespace,
        server,
        &ctx.scope,
    )
    .await
    {
        Ok(ScopeVerdict::InScope) => None,
        Ok(ScopeVerdict::Foreign | ScopeVerdict::Absent) => Some(no_such(server)),
        Err(err) => {
            error!(error = ?err, %server, "agent: scope check failed");
            Some(cluster_error())
        }
    }
}

async fn exec_list_servers(ctx: &ToolCtx<'_>) -> String {
    match list_active_servers(
        ctx.data.kube_client.clone(),
        &ctx.data.namespace,
        &ctx.data.domain,
        &ctx.scope,
    )
    .await
    {
        Ok(summaries) => format_server_list(&summaries),
        Err(err) => {
            error!(error = ?err, "agent: list_servers failed");
            cluster_error()
        }
    }
}

async fn exec_server_status(ctx: &ToolCtx<'_>, server: &str) -> String {
    let summaries = match list_active_servers(
        ctx.data.kube_client.clone(),
        &ctx.data.namespace,
        &ctx.data.domain,
        &ctx.scope,
    )
    .await
    {
        Ok(summaries) => summaries,
        Err(err) => {
            error!(error = ?err, "agent: server_status failed");
            return cluster_error();
        }
    };
    let Some(summary) = summaries.iter().find(|summary| summary.name == server) else {
        return no_such(server);
    };
    let mut out = format_summary(summary);
    out.push_str(&occupancy_line(ctx, server).await);
    out
}

/// A "Players online" line appended to `server_status` so Gary always sees
/// occupancy before deciding whether a restart would kick anyone. The count is a
/// live RCON read; games with no RCON (or a console that isn't up yet) report
/// `unknown`, which means "can't confirm it's empty" — never treat it as empty.
async fn occupancy_line(ctx: &ToolCtx<'_>, server: &str) -> String {
    let reason = match supervisor_occupancy(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
    )
    .await
    {
        Ok(FsOutcome::Ok(Some(count))) => {
            let count = count.to_string();
            // The leading newline separates this from the summary line above; the
            // separator is owned here, so the prompt body carries no leading space.
            return format!("\n{}", prompts::OccupancyKnown { count: &count }.render());
        }
        Ok(FsOutcome::Ok(None)) => prompts::OccupancyReasonNoCount::render(),
        Ok(FsOutcome::PodNotReady) => prompts::OccupancyReasonNotUp::render(),
        Ok(_) => prompts::OccupancyReasonNoConsole::render(),
        Err(err) => {
            error!(error = ?err, %server, "agent: occupancy lookup failed");
            prompts::OccupancyReasonUnread::render()
        }
    };
    format!(
        "\n{}",
        prompts::OccupancyUnknown { reason: &reason }.render()
    )
}

async fn exec_create(ctx: &ToolCtx<'_>, game: &str, name: Option<&str>) -> String {
    let Some(entry) = ctx.data.catalog.get(game) else {
        return prompts::CreateUnknownGame {
            game,
            games: &game_ids(ctx),
        }
        .render();
    };
    let server = match build_instance_name(game, name, now_entropy()) {
        Ok(server) => server,
        Err(err) => {
            return prompts::CreateBadName {
                error: &err.to_string(),
            }
            .render();
        }
    };

    match provision_instance(
        &ctx.data.kube_client,
        &ctx.data.namespace,
        &ctx.data.domain,
        &ctx.data.provision_lock,
        entry,
        &server,
        &ctx.guild.map(|guild| guild.to_string()).unwrap_or_default(),
    )
    .await
    {
        // Don't block the loop on first-boot world generation (minutes). Report
        // the address now; the user can ask for status to see when it's ready.
        Ok(ProvisionOutcome::Provisioned { address }) => prompts::CreateCreated {
            server: &server,
            address: &address,
        }
        .render(),
        Ok(ProvisionOutcome::AlreadyExists) => {
            prompts::CreateNameTaken { server: &server }.render()
        }
        Ok(ProvisionOutcome::PortsExhausted) => prompts::CreatePortsExhausted::render(),
        Err(err) => {
            error!(error = ?err, game, server, "agent: create failed");
            cluster_error()
        }
    }
}

async fn exec_remember(ctx: &ToolCtx<'_>, scope: &str, note: &str) -> String {
    let note = note.trim();
    if note.is_empty() {
        return prompts::RememberNeedFact::render();
    }
    let ids: Vec<&str> = ctx.data.catalog.game_ids().collect();
    let Some(scope) = normalize_scope(scope, &ids) else {
        return prompts::RememberBadScope {
            games: &ids.join(", "),
        }
        .render();
    };
    let author = ctx.author_id.get().to_string();
    match ctx.data.memory.remember(&scope, note, Some(&author)).await {
        Ok(RememberOutcome::Saved(id)) => {
            let id = id.to_string();
            prompts::RememberSaved {
                scope: scope.as_str(),
                id: &id,
            }
            .render()
        }
        Ok(RememberOutcome::Unavailable) => prompts::RememberMemoryOffline::render(),
        Err(err) => {
            error!(error = ?err, %scope, "agent: remember failed");
            prompts::RememberSaveFailed::render()
        }
    }
}

async fn exec_forget(ctx: &ToolCtx<'_>, id: i64) -> String {
    match ctx.data.memory.forget(id).await {
        Ok(ForgetOutcome::Forgotten) => {
            let fact_id = id.to_string();
            prompts::ForgetForgot { id: &fact_id }.render()
        }
        Ok(ForgetOutcome::NotFound) => {
            let fact_id = id.to_string();
            prompts::ForgetNoSuchFact { id: &fact_id }.render()
        }
        Ok(ForgetOutcome::Unavailable) => prompts::ForgetMemoryOffline::render(),
        Err(err) => {
            error!(error = ?err, id, "agent: forget failed");
            prompts::ForgetFailed::render()
        }
    }
}

async fn exec_stop(ctx: &ToolCtx<'_>, server: &str) -> String {
    match supervisor_stop(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
    )
    .await
    {
        Ok(outcome) => format_supervisor(server, &outcome),
        Err(err) => {
            error!(error = ?err, %server, "agent: stop failed");
            cluster_error()
        }
    }
}

async fn exec_restart(ctx: &ToolCtx<'_>, server: &str) -> String {
    let outcome = match supervisor_restart(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(err) => {
            error!(error = ?err, %server, "agent: restart failed");
            return cluster_error();
        }
    };
    // Only a server that genuinely bounced applies a pending config change, so only
    // then is there something to verify. Any other outcome (already paused,
    // unreachable, gone) reports as before and leaves the pending change in place
    // for the restart that eventually applies it.
    let change = match &outcome {
        SupervisorOutcome::Restarted => ctx.take_pending_change(server),
        SupervisorOutcome::Paused
        | SupervisorOutcome::Resumed
        | SupervisorOutcome::AlreadyStopped
        | SupervisorOutcome::AlreadyRunning
        | SupervisorOutcome::PodNotReady
        | SupervisorOutcome::Unreachable
        | SupervisorOutcome::Failed(_)
        | SupervisorOutcome::NotFound
        | SupervisorOutcome::NotManaged => None,
    };
    // With a tracked change, the loop — not the model — watches it come back up and
    // rolls a crash back. A plain reboot reports immediately without blocking.
    match change {
        Some(change) => verify_change(ctx, change).await,
        None => format_supervisor(server, &outcome),
    }
}

/// Watch a restart that applied a config change and enforce the design's
/// snapshot→apply→restart→verify→auto-rollback guardrail deterministically: poll
/// until the server is up or crashed, and on a crash restore the pre-edit snapshot
/// and restart once more. Bounded to a single automatic rollback — a server still
/// crashing after that escalates rather than looping. Returns the system-generated
/// account of what happened for the model to relay; the model never has to reason
/// its way back to `restore_file` on its own.
async fn verify_change(ctx: &ToolCtx<'_>, change: PendingChange) -> String {
    let server = change.server.as_str();
    let mut rollback_spent = false;
    loop {
        let outcome = match wait_for_ready(
            &ctx.data.kube_client,
            &ctx.data.http,
            &ctx.data.namespace,
            server,
            ctx.data.control_port,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(err) => {
                error!(error = ?err, %server, "agent: post-restart readiness check failed");
                // Can't confirm health either way — leave the change in place and
                // tell the model plainly, rather than rolling back a maybe-fine server.
                return prompts::ChangeRestartedUnverified { server }.render();
            }
        };
        match next_step(&outcome, rollback_spent) {
            RecoveryStep::Healthy => {
                return if rollback_spent {
                    warn!(%server, path = %change.path, "agent: rolled a crashing config change back and the server recovered");
                    prompts::ChangeRolledBackHealthy {
                        path: change.path.as_str(),
                        server,
                    }
                    .render()
                } else {
                    info!(%server, path = %change.path, "agent: config change verified healthy after restart");
                    prompts::ChangeHealthy { server }.render()
                };
            }
            RecoveryStep::RollBack => match roll_back(ctx, &change).await {
                Ok(()) => {
                    rollback_spent = true;
                }
                Err(message) => return message,
            },
            RecoveryStep::Escalate => {
                error!(%server, path = %change.path, "agent: config change crashed the server and a rollback did not recover it — escalating");
                notify_crash_rollback(ctx, server, &change.path).await;
                let escalation = prompts::OperatorFlagged::render();
                return prompts::ChangeRollbackFailedFlagged {
                    path: change.path.as_str(),
                    server,
                    escalation: &escalation,
                }
                .render();
            }
            RecoveryStep::Inconclusive => return format_ready_wait(server, &outcome),
        }
    }
}

/// Restore the pre-edit snapshot and restart, the mechanical half of an automatic
/// rollback. `Ok` means the world is back on the old config and restarting, so the
/// caller re-polls readiness; `Err` carries the escalation text for a rollback that
/// couldn't even be issued (nothing left to do automatically).
async fn roll_back(ctx: &ToolCtx<'_>, change: &PendingChange) -> Result<(), String> {
    let server = change.server.as_str();
    let restore = supervisor_restore_file(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        &change.path,
    )
    .await;
    match restore {
        Ok(FsOutcome::Ok(_)) => {}
        Ok(
            FsOutcome::NotFound
            | FsOutcome::NotManaged
            | FsOutcome::PodNotReady
            | FsOutcome::Unreachable
            | FsOutcome::Rejected(_),
        ) => {
            warn!(%server, path = %change.path, "agent: auto-rollback restore could not be served");
            return Err(rollback_failed(ctx, server, &change.path).await);
        }
        Err(err) => {
            error!(error = ?err, %server, path = %change.path, "agent: auto-rollback restore failed");
            return Err(rollback_failed(ctx, server, &change.path).await);
        }
    }
    match supervisor_restart(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
    )
    .await
    {
        Ok(SupervisorOutcome::Restarted) => Ok(()),
        Ok(
            SupervisorOutcome::Paused
            | SupervisorOutcome::Resumed
            | SupervisorOutcome::AlreadyStopped
            | SupervisorOutcome::AlreadyRunning
            | SupervisorOutcome::PodNotReady
            | SupervisorOutcome::Unreachable
            | SupervisorOutcome::Failed(_)
            | SupervisorOutcome::NotFound
            | SupervisorOutcome::NotManaged,
        )
        | Err(_) => {
            warn!(%server, path = %change.path, "agent: restart after auto-rollback did not bounce the server");
            notify_crash_rollback(ctx, server, &change.path).await;
            let escalation = prompts::OperatorFlagged::render();
            Err(prompts::ChangeRollbackUnclean {
                path: change.path.as_str(),
                server,
                escalation: &escalation,
            }
            .render())
        }
    }
}

/// Escalation text when an automatic rollback couldn't even be issued — nothing
/// more the loop can do on its own. Also keeps the promise the text makes by
/// actually notifying the operators, not just logging it.
async fn rollback_failed(ctx: &ToolCtx<'_>, server: &str, path: &str) -> String {
    notify_crash_rollback(ctx, server, path).await;
    let escalation = prompts::OperatorFlagged::render();
    prompts::ChangeRollbackImpossible {
        path,
        server,
        escalation: &escalation,
    }
    .render()
}

/// DM the operators that an automatic crash rollback needs a human hand — the
/// shared notify step behind every "I've flagged this for an operator" promise
/// in [`verify_change`]/[`roll_back`].
async fn notify_crash_rollback(ctx: &ToolCtx<'_>, server: &str, path: &str) {
    ctx.data
        .notifier
        .notify(&Escalation::CrashRollback {
            server: server.to_owned(),
            path: path.to_owned(),
        })
        .await;
}

/// Mirrors the `/start` slash command's warm/cold routing: a live pod resumes in
/// place via the supervisor; a shut-down instance is rescheduled. Unlike the slash
/// command, the agent doesn't block waiting for readiness — it reports the
/// address and lets the user poll status.
async fn exec_start(ctx: &ToolCtx<'_>, server: &str) -> String {
    match instance_runtime_state(&ctx.data.kube_client, &ctx.data.namespace, server).await {
        Ok(RuntimeState::PodUp) => match supervisor_start(
            &ctx.data.kube_client,
            &ctx.data.http,
            &ctx.data.namespace,
            server,
            ctx.data.control_port,
        )
        .await
        {
            Ok(outcome) => format_supervisor(server, &outcome),
            Err(err) => {
                error!(error = ?err, %server, "agent: warm start failed");
                cluster_error()
            }
        },
        Ok(RuntimeState::Down) => exec_cold_start(ctx, server).await,
        Ok(RuntimeState::Absent) => no_such(server),
        Err(err) => {
            error!(error = ?err, %server, "agent: start state lookup failed");
            cluster_error()
        }
    }
}

async fn exec_cold_start(ctx: &ToolCtx<'_>, server: &str) -> String {
    match begin_start(
        &ctx.data.kube_client,
        &ctx.data.namespace,
        &ctx.data.domain,
        &ctx.data.catalog,
        server,
    )
    .await
    {
        Ok(StartBegin::Starting { address }) => prompts::ColdStartStarting {
            server,
            address: &address,
        }
        .render(),
        Ok(StartBegin::AlreadyRunning) => prompts::ServerAlreadyRunning { server }.render(),
        Ok(StartBegin::NotFound) => no_such(server),
        Ok(StartBegin::NotManaged) => not_managed(server),
        Ok(StartBegin::UnknownGame(game)) => prompts::ColdStartUnknownGame {
            server,
            game: &game,
        }
        .render(),
        Err(err) => {
            error!(error = ?err, %server, "agent: cold start failed");
            cluster_error()
        }
    }
}

async fn exec_shutdown(ctx: &ToolCtx<'_>, server: &str) -> String {
    match shutdown_instance(&ctx.data.kube_client, &ctx.data.namespace, server).await {
        Ok(ShutdownOutcome::Down) => prompts::ShutdownStopped { server }.render(),
        Ok(ShutdownOutcome::NotFound) => no_such(server),
        Ok(ShutdownOutcome::NotManaged) => not_managed(server),
        Err(err) => {
            error!(error = ?err, %server, "agent: shutdown failed");
            cluster_error()
        }
    }
}

/// Permanent deletion is gated behind an explicit Discord confirmation: the
/// model can request it, but a human must click through before any world is
/// destroyed. The returned text tells the model what the human decided.
async fn exec_destroy(ctx: &ToolCtx<'_>, server: &str) -> String {
    let buttons = CreateActionRow::Buttons(vec![
        CreateButton::new("gary_destroy_confirm")
            .label("Delete it")
            .style(ButtonStyle::Danger),
        CreateButton::new("gary_destroy_cancel")
            .label("Cancel")
            .style(ButtonStyle::Secondary),
    ]);
    let prompt = match ctx
        .channel_id
        .send_message(
            ctx.serenity,
            CreateMessage::new()
                .embed(destroy_confirm_embed(server))
                .components(vec![buttons]),
        )
        .await
    {
        Ok(message) => message,
        Err(err) => {
            error!(error = ?err, %server, "agent: failed to post destroy confirmation");
            return prompts::DestroyNoConfirmChannel::render();
        }
    };

    let decision = ComponentInteractionCollector::new(ctx.serenity)
        .author_id(ctx.author_id)
        .message_id(prompt.id)
        .timeout(COMPONENT_TIMEOUT)
        .await;

    finish_destroy(ctx, server, prompt, decision).await
}

async fn finish_destroy(
    ctx: &ToolCtx<'_>,
    server: &str,
    mut prompt: serenity::Message,
    decision: Option<serenity::ComponentInteraction>,
) -> String {
    let Some(interaction) = decision else {
        edit_prompt(
            ctx,
            &mut prompt,
            neutral_embed("Timed out", "Nothing was deleted."),
        )
        .await;
        let reason = prompts::AbortTimedOut::render();
        return prompts::ConfirmAborted {
            reason: &reason,
            server,
            verb: "deleted",
        }
        .render();
    };

    if let Err(err) = interaction
        .create_response(ctx.serenity, CreateInteractionResponse::Acknowledge)
        .await
    {
        warn!(error = ?err, "agent: failed to acknowledge destroy interaction");
    }

    if interaction.data.custom_id != "gary_destroy_confirm" {
        edit_prompt(
            ctx,
            &mut prompt,
            neutral_embed("Cancelled", "Nothing was deleted."),
        )
        .await;
        let reason = prompts::AbortCancelled::render();
        return prompts::ConfirmAborted {
            reason: &reason,
            server,
            verb: "deleted",
        }
        .render();
    }

    match destroy_instance(&ctx.data.kube_client, &ctx.data.namespace, server).await {
        Ok(outcome) => {
            edit_prompt(ctx, &mut prompt, destroy_result_embed(&outcome, server)).await;
            format_destroy(server, &outcome)
        }
        Err(err) => {
            error!(error = ?err, %server, "agent: destroy failed");
            let message = cluster_error();
            edit_prompt(ctx, &mut prompt, error_embed(&message)).await;
            message
        }
    }
}

async fn edit_prompt(
    ctx: &ToolCtx<'_>,
    prompt: &mut serenity::Message,
    embed: serenity::CreateEmbed,
) {
    if let Err(err) = prompt
        .edit(
            ctx.serenity,
            EditMessage::new().embed(embed).components(Vec::new()),
        )
        .await
    {
        warn!(error = ?err, "agent: failed to clear destroy confirmation prompt");
    }
}

async fn exec_browse_files(ctx: &ToolCtx<'_>, server: &str, path: &str) -> String {
    match supervisor_list_files(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        path,
    )
    .await
    {
        Ok(outcome) => match fs_result(server, outcome) {
            Ok(entries) => format_entries(path, &entries),
            Err(problem) => problem,
        },
        Err(err) => {
            error!(error = ?err, %server, "agent: browse_files failed");
            cluster_error()
        }
    }
}

async fn exec_read_file(ctx: &ToolCtx<'_>, server: &str, path: &str) -> String {
    match supervisor_read_file(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        path,
    )
    .await
    {
        Ok(outcome) => match fs_result(server, outcome) {
            Ok(file) => format_file(&file),
            Err(problem) => problem,
        },
        Err(err) => {
            error!(error = ?err, %server, "agent: read_file failed");
            cluster_error()
        }
    }
}

async fn exec_read_logs(ctx: &ToolCtx<'_>, server: &str, lines: Option<usize>) -> String {
    match supervisor_read_logs(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        lines,
    )
    .await
    {
        Ok(outcome) => match fs_result(server, outcome) {
            Ok(log_lines) => format_logs(server, &log_lines),
            Err(problem) => problem,
        },
        Err(err) => {
            error!(error = ?err, %server, "agent: read_logs failed");
            cluster_error()
        }
    }
}

async fn exec_write_file(ctx: &ToolCtx<'_>, server: &str, path: &str, content: &str) -> String {
    match supervisor_write_file(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        path,
        content,
    )
    .await
    {
        Ok(outcome) => match fs_result(server, outcome) {
            Ok(result) => {
                if result.backed_up {
                    ctx.note_pending_change(server, &result.path);
                }
                format_write(&result)
            }
            Err(problem) => problem,
        },
        Err(err) => {
            error!(error = ?err, %server, "agent: write_file failed");
            cluster_error()
        }
    }
}

async fn exec_edit_file(
    ctx: &ToolCtx<'_>,
    server: &str,
    path: &str,
    old_text: &str,
    new_text: &str,
) -> String {
    match supervisor_edit_file(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        path,
        Replacement {
            old: old_text,
            new: new_text,
        },
    )
    .await
    {
        Ok(outcome) => {
            if let EditOutcome::Edited(result) = &outcome
                && result.backed_up
            {
                ctx.note_pending_change(server, &result.path);
            }
            format_edit(server, path, outcome)
        }
        Err(err) => {
            error!(error = ?err, %server, "agent: edit_file failed");
            cluster_error()
        }
    }
}

async fn exec_restore_file(ctx: &ToolCtx<'_>, server: &str, path: &str) -> String {
    match supervisor_restore_file(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        path,
    )
    .await
    {
        Ok(outcome) => match fs_result(server, outcome) {
            Ok(result) => {
                ctx.drop_pending_change(server, &result.path);
                format_restore(&result)
            }
            Err(problem) => problem,
        },
        Err(err) => {
            error!(error = ?err, %server, "agent: restore_file failed");
            cluster_error()
        }
    }
}

async fn exec_send_command(ctx: &ToolCtx<'_>, server: &str, command: &str) -> String {
    match supervisor_send_command(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        command,
    )
    .await
    {
        Ok(outcome) => match fs_result(server, outcome) {
            Ok(result) => {
                announce_action(ctx, server, &format!("ran `{command}`")).await;
                format_command_output(server, command, &result)
            }
            Err(problem) => problem,
        },
        Err(err) => {
            error!(error = ?err, %server, "agent: send_command failed");
            cluster_error()
        }
    }
}

/// Queue a task to run once `server` reaches `condition`, returning a model-facing
/// note for Gary to relay in his own words. Non-blocking: the wait happens in the
/// background so this turn stays free. Refuses `empty`/`idle` for a game that
/// can't report a live player count (there'd be no way to tell when it's empty),
/// and reports plainly when the queue backend is unavailable — in both cases
/// nudging Gary to offer doing it now instead of leaving the ask dropped.
async fn exec_run_when(
    ctx: &ToolCtx<'_>,
    server: &str,
    condition: Condition,
    task: &str,
) -> String {
    if !ctx.data.defer.is_enabled() {
        return prompts::RunWhenQueueUnavailable::render();
    }
    if matches!(condition, Condition::Empty | Condition::Idle)
        && let Some(refusal) = empty_condition_feasibility(ctx, server).await
    {
        return refusal;
    }

    let record = DeferredTask::new(task, ctx.author_id.get(), ctx.channel_id.get(), ctx.guild);
    match ctx
        .data
        .defer
        .enqueue_and_watch(ctx.data, ctx.serenity, server, condition, &record)
        .await
    {
        Ok(()) => prompts::RunWhenScheduled {
            server,
            condition: condition.as_str(),
            task,
        }
        .render(),
        Err(err) => {
            error!(error = ?err, %server, "agent: failed to enqueue deferred task");
            prompts::RunWhenScheduleRejected::render()
        }
    }
}

/// `None` if `server` can report a live player count (so `empty`/`idle` are
/// watchable), else a model-facing refusal. A definite "no live count"
/// (`Ok(None)`) is a hard refusal; a transient not-ready/unreachable is allowed
/// through — the watcher polls until the count is readable.
async fn empty_condition_feasibility(ctx: &ToolCtx<'_>, server: &str) -> Option<String> {
    match supervisor_occupancy(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
    )
    .await
    {
        Ok(FsOutcome::Ok(None)) => Some(prompts::RunWhenCantWatchEmpty { server }.render()),
        // A real count, or a transient state — let it queue; the watcher handles it.
        Ok(
            FsOutcome::Ok(Some(_))
            | FsOutcome::NotFound
            | FsOutcome::NotManaged
            | FsOutcome::PodNotReady
            | FsOutcome::Unreachable
            | FsOutcome::Rejected(_),
        ) => None,
        Err(err) => {
            error!(error = ?err, %server, "agent: run_when feasibility probe failed");
            None
        }
    }
}

async fn exec_list_backups(ctx: &ToolCtx<'_>, server: &str) -> String {
    let Some(service) = ctx.data.backup.clone() else {
        return backups_not_configured();
    };
    match service.list_backups(server).await {
        Ok(list) => format_backup_list(server, &list),
        Err(err) => {
            error!(error = ?err, %server, "agent: list_backups failed");
            cluster_error()
        }
    }
}

async fn exec_list_archives(ctx: &ToolCtx<'_>) -> String {
    let Some(service) = ctx.data.backup.clone() else {
        return backups_not_configured();
    };
    if !service.archives_enabled() {
        return archives_unavailable_text();
    }
    match service.list_archives(&ctx.scope).await {
        Ok(list) => format_archive_list(&list),
        Err(err) => {
            error!(error = ?err, "agent: list_archives failed");
            cluster_error()
        }
    }
}

async fn exec_backup(ctx: &ToolCtx<'_>, server: &str) -> String {
    let Some(service) = ctx.data.backup.clone() else {
        return backups_not_configured();
    };
    match service
        .backup_instance(
            &backup_ctx(ctx.data),
            server,
            &ctx.author_id.get().to_string(),
        )
        .await
    {
        Ok(outcome) => {
            if let Some(reason) = outcome.reason() {
                warn!(reason, %server, "agent: backup did not succeed");
            }
            format_backup(server, &outcome)
        }
        Err(err) => {
            error!(error = ?err, %server, "agent: backup failed");
            cluster_error()
        }
    }
}

/// Archiving releases the server's storage, so — like destroy — the model can
/// request it but a human must click through before anything is released.
async fn exec_archive(ctx: &ToolCtx<'_>, server: &str) -> String {
    let Some(service) = ctx.data.backup.clone() else {
        return backups_not_configured();
    };
    if !service.archives_enabled() {
        return archives_unavailable_text();
    }
    let cancelled = prompts::AbortCancelled::render();
    let timed_out = prompts::AbortTimedOut::render();
    match confirm_destructive(
        ctx,
        archive_confirm_embed(server),
        "gary_archive_confirm",
        prompts::ConfirmAborted {
            reason: &cancelled,
            server,
            verb: "archived",
        }
        .render(),
        prompts::ConfirmAborted {
            reason: &timed_out,
            server,
            verb: "archived",
        }
        .render(),
    )
    .await
    {
        Confirm::Declined(text) => text,
        Confirm::Confirmed(mut prompt) => {
            match service
                .archive_instance(
                    &backup_ctx(ctx.data),
                    server,
                    &ctx.author_id.get().to_string(),
                )
                .await
            {
                Ok(outcome) => {
                    if let Some(reason) = outcome.reason() {
                        warn!(reason, %server, "agent: archive did not succeed");
                    }
                    edit_prompt(ctx, &mut prompt, archive_result_embed(&outcome)).await;
                    format_archive(server, &outcome)
                }
                Err(err) => {
                    error!(error = ?err, %server, "agent: archive failed");
                    let message = cluster_error();
                    edit_prompt(ctx, &mut prompt, error_embed(&message)).await;
                    message
                }
            }
        }
    }
}

/// Restoring overwrites the live world, so it too is gated behind a human click.
async fn exec_restore(ctx: &ToolCtx<'_>, server: &str) -> String {
    let Some(service) = ctx.data.backup.clone() else {
        return backups_not_configured();
    };
    let backups = match service.list_backups(server).await {
        Ok(backups) => backups,
        Err(err) => {
            error!(error = ?err, %server, "agent: restore listing failed");
            return cluster_error();
        }
    };
    let Some(latest) = backups.first() else {
        return prompts::RestoreServerNoBackups { server }.render();
    };
    let key = latest.key.clone();
    let label = latest.created_at.clone();
    let cancelled = prompts::AbortCancelled::render();
    let timed_out = prompts::AbortTimedOut::render();
    match confirm_destructive(
        ctx,
        restore_confirm_embed(server, &label),
        "gary_restore_confirm",
        prompts::ConfirmAborted {
            reason: &cancelled,
            server,
            verb: "restored",
        }
        .render(),
        prompts::ConfirmAborted {
            reason: &timed_out,
            server,
            verb: "restored",
        }
        .render(),
    )
    .await
    {
        Confirm::Declined(text) => text,
        Confirm::Confirmed(mut prompt) => {
            match service
                .restore_backup(&backup_ctx(ctx.data), server, &key)
                .await
            {
                Ok(outcome) => {
                    if let Some(reason) = outcome.reason() {
                        warn!(reason, %server, "agent: restore did not succeed");
                    }
                    edit_prompt(ctx, &mut prompt, restore_result_embed(&outcome, server)).await;
                    format_restore_outcome(server, &outcome)
                }
                Err(err) => {
                    error!(error = ?err, %server, "agent: restore failed");
                    let message = cluster_error();
                    edit_prompt(ctx, &mut prompt, error_embed(&message)).await;
                    message
                }
            }
        }
    }
}

async fn exec_recover(ctx: &ToolCtx<'_>, name: &str) -> String {
    let Some(service) = ctx.data.backup.clone() else {
        return backups_not_configured();
    };
    if !service.archives_enabled() {
        return archives_unavailable_text();
    }
    // Resolve the archive's owning guild from the caller's scope so recover
    // recreates it in its original tenant (and an operator can recover across
    // guilds). The scope-filtered listing also enforces tenancy: an archive in
    // another guild simply isn't found.
    let guild = match service.list_archives(&ctx.scope).await {
        Ok(archives) => match archives.iter().find(|archive| archive.name == name) {
            Some(archive) => archive.guild.clone(),
            None => {
                return prompts::RecoverNoArchiveHere { name }.render();
            }
        },
        Err(err) => {
            error!(error = ?err, %name, "agent: recover archive lookup failed");
            return cluster_error();
        }
    };
    match service
        .recover_archive(&backup_ctx(ctx.data), &guild, name)
        .await
    {
        Ok(outcome) => {
            if let Some(reason) = outcome.reason() {
                warn!(reason, %name, "agent: recover did not succeed");
            }
            format_recover(name, &outcome)
        }
        Err(err) => {
            error!(error = ?err, %name, "agent: recover failed");
            cluster_error()
        }
    }
}

/// A human's decision on a Gary destructive backup-action prompt.
enum Confirm {
    /// Approved; carries the prompt message so the caller edits it with the result.
    /// Boxed because a `Message` is far larger than the `Declined` string.
    Confirmed(Box<serenity::Message>),
    /// Declined or timed out; carries the text to report to the model.
    Declined(String),
}

/// Post a Danger/Cancel confirmation in-channel and wait for the invoking user's
/// click, mirroring the destroy-confirmation gate for archive/restore.
async fn confirm_destructive(
    ctx: &ToolCtx<'_>,
    prompt: serenity::CreateEmbed,
    confirm_id: &str,
    cancel_line: String,
    timeout_line: String,
) -> Confirm {
    let buttons = CreateActionRow::Buttons(vec![
        CreateButton::new(confirm_id)
            .label("Do it")
            .style(ButtonStyle::Danger),
        CreateButton::new("gary_backup_cancel")
            .label("Cancel")
            .style(ButtonStyle::Secondary),
    ]);
    let mut prompt_msg = match ctx
        .channel_id
        .send_message(
            ctx.serenity,
            CreateMessage::new().embed(prompt).components(vec![buttons]),
        )
        .await
    {
        Ok(message) => message,
        Err(err) => {
            error!(error = ?err, "agent: failed to post backup confirmation");
            return Confirm::Declined(
                "I couldn't post a confirmation prompt in this channel, so I didn't do anything."
                    .to_owned(),
            );
        }
    };
    let decision = ComponentInteractionCollector::new(ctx.serenity)
        .author_id(ctx.author_id)
        .message_id(prompt_msg.id)
        .timeout(COMPONENT_TIMEOUT)
        .await;
    let Some(interaction) = decision else {
        edit_prompt(
            ctx,
            &mut prompt_msg,
            neutral_embed("Timed out", "Nothing was changed."),
        )
        .await;
        return Confirm::Declined(timeout_line);
    };
    if let Err(err) = interaction
        .create_response(ctx.serenity, CreateInteractionResponse::Acknowledge)
        .await
    {
        warn!(error = ?err, "agent: failed to acknowledge backup interaction");
    }
    if interaction.data.custom_id != confirm_id {
        edit_prompt(
            ctx,
            &mut prompt_msg,
            neutral_embed("Cancelled", "Nothing was changed."),
        )
        .await;
        return Confirm::Declined(cancel_line);
    }
    Confirm::Confirmed(Box::new(prompt_msg))
}

/// Broadcast to everyone in-game that Gary ran a console command, as
/// `Gary: <phrase>`, best-effort. This is the attributed audit line that replaces
/// Minecraft's `[Rcon]` op-broadcast (disabled at the image level) so players know
/// when Gary is acting on the live server; delivery is fire-and-forget, so a
/// paused or console-less server just gets no message and the command is
/// unaffected.
async fn announce_action(ctx: &ToolCtx<'_>, server: &str, phrase: &str) {
    supervisor_announce(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        &format!("Gary: {phrase}"),
    )
    .await;
}

/// Collapse a filesystem outcome into either its payload or a plain-language
/// explanation of why the operation couldn't be served, for the model to relay.
fn fs_result<T>(server: &str, outcome: FsOutcome<T>) -> Result<T, String> {
    match outcome {
        FsOutcome::Ok(value) => Ok(value),
        FsOutcome::NotFound => Err(no_such(server)),
        FsOutcome::NotManaged => Err(not_managed(server)),
        FsOutcome::PodNotReady => Err(prompts::FsNotReady { server }.render()),
        FsOutcome::Unreachable => Err(prompts::FsUnreachable { server }.render()),
        FsOutcome::Rejected(message) => Err(prompts::FsRejected {
            message: message.as_str(),
        }
        .render()),
    }
}

/// One server rendered as a single labeled line for the model to read.
fn format_summary(server: &ServerSummary) -> String {
    let game = server.game.as_deref().unwrap_or("unknown game");
    let address = server.address.as_deref().unwrap_or("no address yet");
    prompts::ServerSummaryLine {
        name: server.name.as_str(),
        game,
        state: server.state.as_str(),
        address,
    }
    .render()
}

/// The active servers rendered as a newline-separated list.
fn format_server_list(servers: &[ServerSummary]) -> String {
    if servers.is_empty() {
        return prompts::ServerListEmpty::render();
    }
    servers
        .iter()
        .map(format_summary)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A tool result the model reads, so it carries the hint to re-list rather than
/// just reporting the server missing.
fn no_such(server: &str) -> String {
    prompts::ServerNotFound { server }.render()
}

fn cluster_error() -> String {
    prompts::ClusterUnreachable::render()
}

fn format_entries(path: &str, entries: &[DirEntry]) -> String {
    let location = if path.is_empty() {
        "the data directory".to_owned()
    } else {
        path.to_owned()
    };
    if entries.is_empty() {
        return prompts::BrowseEmpty {
            location: &location,
        }
        .render();
    }
    let listing = entries
        .iter()
        .map(|entry| match entry.kind {
            EntryKind::Dir => format!("{}/ (folder)", entry.name),
            EntryKind::File => format!("{} ({} bytes)", entry.name, entry.size),
            EntryKind::Other => format!("{} (other)", entry.name),
        })
        .collect::<Vec<_>>()
        .join("\n");
    prompts::BrowseListing {
        location: &location,
        listing: &listing,
    }
    .render()
}

fn format_file(file: &ReadResponse) -> String {
    // The parenthetical's leading space is a code-owned separator; the note
    // prompt itself carries none, so the empty case renders cleanly.
    let note = if file.truncated {
        format!(" {}", prompts::FileTruncatedNote::render())
    } else {
        String::new()
    };
    prompts::FileContents {
        path: file.path.as_str(),
        note: &note,
        content: file.content.as_str(),
    }
    .render()
}

fn format_logs(server: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        return prompts::LogsEmpty { server }.render();
    }
    let joined = lines.join("\n");
    prompts::LogsOutput {
        server,
        lines: &joined,
    }
    .render()
}

fn format_write(result: &WriteResponse) -> String {
    let saved = if result.backed_up {
        prompts::FileBackupSaved::render()
    } else {
        prompts::FileNoBackup::render()
    };
    prompts::FileWritten {
        path: result.path.as_str(),
        saved: &saved,
    }
    .render()
}

/// Render an [`EditOutcome`]. The soft-failure variants explain what to do next
/// (re-read and match exactly, disambiguate, or fall back to `write_file`) so Gary
/// can recover instead of reporting a dead end.
fn format_edit(server: &str, path: &str, outcome: EditOutcome) -> String {
    match outcome {
        EditOutcome::Edited(result) => {
            let saved = if result.backed_up {
                prompts::FileBackupSaved::render()
            } else {
                prompts::FileNoBackup::render()
            };
            prompts::FileEdited {
                path: result.path.as_str(),
                saved: &saved,
            }
            .render()
        }
        EditOutcome::NoMatch => prompts::EditNoMatch { path, server }.render(),
        EditOutcome::Ambiguous(count) => {
            let count = count.to_string();
            prompts::EditAmbiguous {
                count: &count,
                path,
            }
            .render()
        }
        EditOutcome::Unchanged => prompts::EditUnchanged { path }.render(),
        EditOutcome::TooLargeToEdit => prompts::EditTooLarge { path }.render(),
        EditOutcome::Unserved(problem) => match fs_result(server, problem) {
            // Unserved only ever carries a failure; the Ok arm is unreachable in
            // practice but is handled defensively rather than panicking.
            Ok(()) => cluster_error(),
            Err(message) => message,
        },
    }
}

fn format_restore(result: &RestoreResponse) -> String {
    prompts::RestoreFileDone {
        path: result.path.as_str(),
    }
    .render()
}

fn format_command_output(server: &str, command: &str, result: &CommandResponse) -> String {
    let output = result.output.trim();
    if output.is_empty() {
        prompts::CommandNoOutput { command, server }.render()
    } else {
        prompts::CommandOutput {
            command,
            server,
            output,
        }
        .render()
    }
}

fn format_supervisor(server: &str, outcome: &SupervisorOutcome) -> String {
    match outcome {
        SupervisorOutcome::Paused => prompts::LifecyclePaused { server }.render(),
        SupervisorOutcome::Resumed => prompts::LifecycleWaking { server }.render(),
        SupervisorOutcome::Restarted => prompts::LifecycleRestarted { server }.render(),
        SupervisorOutcome::AlreadyStopped => prompts::LifecycleAlreadyPaused { server }.render(),
        SupervisorOutcome::AlreadyRunning => prompts::ServerAlreadyRunning { server }.render(),
        SupervisorOutcome::PodNotReady => prompts::LifecycleControlNotReady { server }.render(),
        SupervisorOutcome::Unreachable => prompts::LifecycleControlUnreachable { server }.render(),
        SupervisorOutcome::Failed(message) => prompts::LifecycleControlRefused {
            server,
            message: message.as_str(),
        }
        .render(),
        SupervisorOutcome::NotFound => no_such(server),
        SupervisorOutcome::NotManaged => not_managed(server),
    }
}

fn format_ready_wait(server: &str, outcome: &ReadyWait) -> String {
    match outcome {
        ReadyWait::Ready => prompts::ReadyBackUp { server }.render(),
        ReadyWait::Crashed => prompts::ReadyCrashed { server }.render(),
        ReadyWait::Stopped => prompts::ReadyStopped { server }.render(),
        ReadyWait::TimedOut => prompts::ReadyTimedOut { server }.render(),
        ReadyWait::NotFound => no_such(server),
        ReadyWait::NotManaged => not_managed(server),
    }
}

fn format_destroy(server: &str, outcome: &DestroyOutcome) -> String {
    match outcome {
        DestroyOutcome::Destroyed => prompts::DestroyDeleted { server }.render(),
        DestroyOutcome::NotFound => no_such(server),
        DestroyOutcome::NotManaged => not_managed(server),
    }
}

fn format_backup(server: &str, outcome: &BackupOutcome) -> String {
    match outcome {
        BackupOutcome::BackedUp { size_bytes } => {
            let size = human_size(*size_bytes);
            prompts::BackupDone {
                server,
                size: &size,
            }
            .render()
        }
        BackupOutcome::NotRunning => prompts::BackupNotRunning { server }.render(),
        BackupOutcome::Unreachable(_) => prompts::BackupUnreachable { server }.render(),
        BackupOutcome::NotFound => no_such(server),
        BackupOutcome::NotManaged => not_managed(server),
    }
}

fn format_archive(server: &str, outcome: &ArchiveOutcome) -> String {
    match outcome {
        ArchiveOutcome::Archived { name, size_bytes } => {
            let size = human_size(*size_bytes);
            prompts::ArchiveDone {
                name: name.as_str(),
                size: &size,
            }
            .render()
        }
        ArchiveOutcome::Unavailable => archives_unavailable_text(),
        ArchiveOutcome::Failed(_) => prompts::ArchiveFailed { server }.render(),
        ArchiveOutcome::NotFound => no_such(server),
        ArchiveOutcome::NotManaged => not_managed(server),
    }
}

fn format_restore_outcome(server: &str, outcome: &RestoreOutcome) -> String {
    match outcome {
        RestoreOutcome::Restored {
            boot: BootState::Ready,
        } => prompts::RestoreServerReady { server }.render(),
        RestoreOutcome::Restored {
            boot: BootState::TimedOut,
        } => prompts::RestoreServerTimedOut { server }.render(),
        RestoreOutcome::Restored {
            boot: BootState::Crashed,
        } => prompts::RestoreServerCrashed { server }.render(),
        RestoreOutcome::Restored {
            boot: BootState::Stopped,
        } => prompts::RestoreServerStopped { server }.render(),
        RestoreOutcome::Failed(_) => prompts::RestoreServerFailed { server }.render(),
        RestoreOutcome::NotFound => no_such(server),
        RestoreOutcome::NotManaged => not_managed(server),
    }
}

fn format_recover(name: &str, outcome: &RecoverOutcome) -> String {
    match outcome {
        RecoverOutcome::Recovered {
            address,
            boot: BootState::Ready,
        } => prompts::RecoverReady {
            name,
            address: address.as_str(),
        }
        .render(),
        RecoverOutcome::Recovered {
            address,
            boot: BootState::TimedOut,
        } => prompts::RecoverTimedOut {
            name,
            address: address.as_str(),
        }
        .render(),
        RecoverOutcome::Recovered {
            address,
            boot: BootState::Crashed,
        } => prompts::RecoverCrashed {
            name,
            address: address.as_str(),
        }
        .render(),
        RecoverOutcome::Recovered {
            address,
            boot: BootState::Stopped,
        } => prompts::RecoverStopped {
            name,
            address: address.as_str(),
        }
        .render(),
        RecoverOutcome::NoSuchArchive => prompts::RecoverNoSuchArchive { name }.render(),
        RecoverOutcome::NameInUse => prompts::RecoverNameInUse { name }.render(),
        RecoverOutcome::Unavailable => archives_unavailable_text(),
        RecoverOutcome::UnknownGame(game) => prompts::RecoverUnknownGame {
            name,
            game: game.as_str(),
        }
        .render(),
        RecoverOutcome::PortsExhausted => prompts::RecoverPortsExhausted::render(),
        RecoverOutcome::Failed(_) => prompts::RecoverFailed { name }.render(),
    }
}

fn format_backup_list(server: &str, artifacts: &[ArtifactSummary]) -> String {
    if artifacts.is_empty() {
        return prompts::BackupListEmpty { server }.render();
    }
    let lines = artifacts
        .iter()
        .map(|artifact| {
            format!(
                "{} ({})",
                artifact.created_at,
                human_size(artifact.size_bytes)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    prompts::BackupListHeader {
        server,
        lines: &lines,
    }
    .render()
}

fn format_archive_list(artifacts: &[ArtifactSummary]) -> String {
    if artifacts.is_empty() {
        return prompts::ArchiveListEmpty::render();
    }
    let lines = artifacts
        .iter()
        .map(|artifact| {
            format!(
                "{} ({}, {})",
                artifact.name,
                human_size(artifact.size_bytes),
                artifact.created_at
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    prompts::ArchiveListHeader { lines: &lines }.render()
}

fn backups_not_configured() -> String {
    prompts::BackupsNotConfigured::render()
}

fn archives_unavailable_text() -> String {
    prompts::ArchivesUnavailable::render()
}

fn not_managed(server: &str) -> String {
    prompts::NotManaged { server }.render()
}

fn game_ids(ctx: &ToolCtx<'_>) -> String {
    ctx.data.catalog.game_ids().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
#[path = "tests/tools.rs"]
mod tests;
