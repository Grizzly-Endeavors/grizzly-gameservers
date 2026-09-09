//! Gary's tool surface: the lifecycle operations exposed to the model, their
//! parameter schemas, the admin tiering, and the dispatcher that runs a call and
//! renders a compact text result for the model to relay. The results are plain
//! text on purpose — Gary composes the friendly Discord reply himself.

use poise::serenity_prelude as serenity;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serenity::futures::StreamExt as _;
use serenity::{
    ButtonStyle, ComponentInteractionCollector, CreateActionRow, CreateButton,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, EditMessage,
};
use tracing::{error, info, warn};

use grizzly_control_api::DEFAULT_READ_LINES;
use grizzly_control_api::{
    CommandResponse, DirEntry, EntryKind, ReadResponse, RestoreResponse, ShellResponse,
    WriteResponse,
};

use super::guide::{GuideSubject, render_guide};
use super::profiles::{exec_export_profile, exec_import_profile};
use super::recovery::{PendingChange, RecoveryStep, next_step};

use super::super::auth::{AccessLevel, Speaker, access_level_for};
use super::super::render::{
    archive_confirm_embed, archive_result_embed, delete_archive_confirm_embed,
    delete_archive_result_embed, destroy_confirm_embed, destroy_result_embed, error_embed,
    neutral_embed, restore_confirm_embed, restore_result_embed,
};
use super::super::{COMPONENT_TIMEOUT, Data, backup_ctx};
use crate::agent::recorder::GuardrailVerdict;
use crate::agent::{EscalationRequest, ToolCall, ToolDef, TurnOrigin};
use crate::agones::{
    DestroyOutcome, EditOutcome, FsOutcome, ProvisionOutcome, ProvisionSpec, ReadWindow, ReadyWait,
    Replacement, ResizeOutcome, RuntimeState, ScopeVerdict, ServerScope, ServerSummary,
    ShutdownOutcome, StartBegin, StatusPoll, SupervisorOutcome, begin_start, build_instance_name,
    catalog_sizing, destroy_instance, instance_runtime_state, list_active_servers, now_entropy,
    provision_instance, resize_instance, shutdown_instance, supervisor_announce,
    supervisor_edit_file, supervisor_list_files, supervisor_occupancy, supervisor_read_file,
    supervisor_read_logs, supervisor_restart, supervisor_restore_file, supervisor_run_shell,
    supervisor_send_command, supervisor_start, supervisor_status, supervisor_stop,
    supervisor_write_file, verify_scope, wait_for_instance_ready, wait_for_ready,
};
use crate::agones::{InstanceOwner, ModRegistry, guild_usage, owner_of};
use crate::backup::{
    ArchiveDeleteOutcome, ArchiveOutcome, ArtifactSummary, BackupOutcome, BootState,
    RecoverOutcome, RestoreOutcome,
};
use crate::capacity::CapacityRefusal;
use crate::defer::{Condition, DeferredTask};
use crate::domain::{GameId, GuildId, InstanceName};
use crate::events::{
    Actor, Attribution, EventKind, ProductEvent, ResourceSample, Surface, mapping, parse_bytes,
    parse_cpu_millicores, sample_instance,
};
use crate::limits::{LimitRefusal, check_added_load, format_cores, human_size};
use crate::memory::{ForgetOutcome, RememberOutcome, normalize_scope};
use crate::modrinth::{
    Kind, Loader, Project, SearchHit, SearchQuery, fetch_project, search_projects,
    versions_for_display,
};
use crate::notify::Escalation;
use crate::prompts::{
    self, AboutServer, ArchiveServer, BackupServer, BrowseFiles, CreateServer, CreateServerParams,
    DeleteArchive, DeleteArchiveDone, DeleteArchiveFailed, DeleteArchiveNotFound,
    DeleteArchiveUnavailable, DestroyServer, EditFile, EditFileParams, Escalate, EscalateParams,
    ExportProfile, Forget, ForgetParams, GameGuide, GameGuideParams, ImportProfile,
    ImportProfileParams, ListArchives, ListBackups, ListServers, ModInfo, ModInfoParams,
    NameParams, PathParams, ReadFile, ReadFileParams, ReadLogs, ReadLogsParams, RecoverServer,
    Remember, RememberParams, RestartServer, RestoreFile, RestoreServer, RunShell, RunShellParams,
    RunWhen, RunWhenParams, SearchMods, SearchModsParams, SendCommand, SendCommandParams,
    ServerResources, SetResources, SetResourcesParams, ShutdownServer, StartServer, StopServer,
    WriteFile, WriteFileParams,
};
use crate::registry::{Side, list_or_none, short_description, short_title};
use crate::sizing::{Sizing, SizingBounds, SizingRejection};
use crate::thunderstore::{
    Listing, Package, SearchQuery as ThunderstoreQuery, fetch_package, search_packages,
    split_full_name,
};

/// How a destructive tool obtains the human approval it requires.
///
/// [`Interactive`](ConfirmMode::Interactive) is the default so that any new
/// surface must state its posture out loud; a defaulted field is exactly how a
/// future caller would inherit pre-approval without anyone deciding it should.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ConfirmMode {
    /// Post Danger/Cancel buttons in the channel and block on the caller's click.
    #[default]
    Interactive,
    /// The caller approved this action out of band, before the request was made.
    /// Only the operator harness selects this, per request — no Discord client can
    /// click a button, so a headless caller would otherwise only ever time out. The
    /// action still posts a visible audit line to the channel, and refuses to
    /// proceed if that line can't be posted.
    Preapproved,
}

/// Everything a tool executor needs: the shared bot state plus the Discord
/// handles the destructive-confirmation flow uses, and the caller's access tier
/// (so mutating tools can refuse at execution time as defense in depth).
pub(crate) struct ToolCtx<'a> {
    pub(crate) data: &'a Data,
    pub(crate) serenity: &'a serenity::Context,
    /// Which entry point these tools are running under. Carried so the audit
    /// trail names the real surface instead of deducing one from `confirmations`.
    pub(crate) origin: TurnOrigin,
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
    /// How the destructive tools obtain their confirmation. Every Discord-driven
    /// surface sets [`ConfirmMode::Interactive`]; only the operator harness may
    /// pre-approve, and only per request.
    pub(crate) confirmations: ConfirmMode,
    /// The last config edit that saved a snapshot and hasn't been verified by a
    /// restart yet, tracked for the duration of one Gary turn. When a restart
    /// applies this change, [`exec_restart`] watches it come back up and rolls it
    /// back on a crash — deterministic recovery that doesn't depend on the model.
    /// Guarded by a plain mutex because dispatch runs the tools one at a time and
    /// never holds the lock across an await.
    pub(crate) pending_change: std::sync::Mutex<Option<PendingChange>>,
    /// How this turn's config change came out, once a restart verified it. Read
    /// by the shell after the loop returns, because the verdict is reached inside
    /// a tool and the loop never sees it as anything but result prose. Last write
    /// wins: a turn with several edit-and-restart cycles is scored on the final
    /// verdict, and the per-cycle history is already in its tool observations.
    pub(crate) guardrail: std::sync::Mutex<Option<GuardrailVerdict>>,
    /// Set when the model calls `escalate`. The loop polls it after each round
    /// of tool calls and stops there, which is what makes the escalation exit an
    /// exit rather than a notification the model can talk past. First write wins:
    /// if a round somehow escalates twice, the reply the user sees is the one
    /// that decided to hand off, not a second thought about it.
    pub(crate) escalation: std::sync::Mutex<Option<EscalationRequest>>,
}

impl ToolCtx<'_> {
    /// Record that `path` on `server` was just edited with a snapshot saved, so a
    /// following restart knows there's a change to verify and, if it crashes, undo.
    /// The last edit wins, mirroring `restore_file`'s own last-write-per-file model.
    pub(crate) fn note_pending_change(&self, server: &str, path: &str) {
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

    /// Record how a verified config change came out, for the turn's guardrail
    /// score.
    fn note_guardrail_verdict(&self, verdict: GuardrailVerdict) {
        *self
            .guardrail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(verdict);
    }

    /// The verdict this turn's config change reached, if it reached one.
    pub(crate) fn guardrail_verdict(&self) -> Option<GuardrailVerdict> {
        *self
            .guardrail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Record the model's decision to hand this turn to a human, keeping any
    /// earlier request in place. Poison-tolerant for the same reason as the
    /// pending-change lock.
    fn note_escalation(&self, request: EscalationRequest) {
        let mut guard = self
            .escalation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_none() {
            *guard = Some(request);
        }
    }

    /// The handoff this turn asked for, if the model asked for one. Cloned rather
    /// than taken, because both the loop (to stop) and the shell (to notify) read
    /// it.
    pub(crate) fn escalation_request(&self) -> Option<EscalationRequest> {
        self.escalation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Who this tool is acting for, for the product event log. The surface comes
    /// from the turn's origin, so a `run_when` batch and an operator-harness turn
    /// are separable from a friend's live conversation without any extra
    /// threading — the executors already carry both.
    /// The catalog id of the game `server` runs, `Ok(None)` when the instance
    /// carries no game label, or the text to hand back instead of running.
    ///
    /// Read off the live listing rather than off the instance name: a name is
    /// only conventionally prefixed with its game, and treating that convention
    /// as a fact would quietly answer the wrong game's question.
    pub(crate) async fn game_of(&self, server: &str) -> Result<Option<String>, String> {
        let summaries = match list_active_servers(
            self.data.kube_client.clone(),
            &self.data.namespace,
            &self.data.domain,
            &self.scope,
        )
        .await
        {
            Ok(summaries) => summaries,
            Err(err) => {
                error!(error = ?err, %server, "agent: could not resolve a server's game");
                return Err(cluster_error());
            }
        };
        summaries
            .into_iter()
            .find(|summary| summary.name == server)
            .map(|summary| summary.game)
            .ok_or_else(|| no_such(server))
    }

    pub(crate) fn attribution(&self) -> Attribution {
        Attribution::new(
            Actor::Discord(self.author_id.get()),
            Surface::from_origin(self.origin),
        )
    }

    /// Record a lifecycle action the model just took, against the server it acted
    /// on. Emitted after the action resolves, never before.
    async fn record_action(&self, kind: EventKind, server: &str, outcome: &'static str) {
        self.record_event(
            ProductEvent::new(kind, &self.attribution(), outcome)
                .instance(InstanceName::new(server)),
        )
        .await;
    }

    /// Record an already-built event, filling in the guild the turn ran in.
    async fn record_event(&self, event: ProductEvent) {
        self.data
            .events
            .record_lifecycle(
                &self.data.kube_client,
                &self.data.namespace,
                event.guild(self.guild.map(|guild| GuildId::new(guild.to_string()))),
            )
            .await;
    }
}

/// Just the `name` field, pulled from any targeted tool's arguments (they all
/// carry one) for the scope gate in [`dispatch`], ignoring the rest.
#[derive(Deserialize)]
struct TargetName {
    name: String,
}

/// Every Discord tool paired with the lowest tier that may call it — the single
/// home the offer list, the dispatch gate, and the below-tier refusal all read,
/// so a tool's tier is stated exactly once.
///
/// The tiers, in the order they appear: read-only lookups anyone may run —
/// plus `escalate`, which every caller needs precisely because the ones with the
/// fewest tools have the fewest other ways forward; manager lifecycle,
/// file-tuning, mod-discovery, memory, and resizing tools — the day-to-day
/// operations, none of which permanently destroy a world; and the admin-only
/// destructive set (permanent deletion, world overwrites, archival, live console
/// commands). The "do not confirm" steer on `destroy_server` and the
/// confirmation phrasing on `archive_server`/`restore_server` live in those
/// tools' prompt files.
///
/// `set_resources` is manager tier so the proactive watchdog — which runs its
/// unattended turns at that tier — can act on the memory-pressure incident it
/// exists to detect, rather than only reporting it. What stops that becoming
/// runaway allocation is the clamp in `crate::sizing`, not the tier.
fn tool_table() -> Vec<(AccessLevel, ToolDef)> {
    vec![
        (AccessLevel::ReadOnly, ListServers::spec().into()),
        (AccessLevel::ReadOnly, AboutServer::spec().into()),
        (AccessLevel::ReadOnly, ListBackups::spec().into()),
        (AccessLevel::ReadOnly, ListArchives::spec().into()),
        (AccessLevel::ReadOnly, ServerResources::spec().into()),
        (AccessLevel::ReadOnly, ExportProfile::spec().into()),
        (AccessLevel::ReadOnly, Escalate::spec().into()),
        (AccessLevel::Manager, CreateServer::spec().into()),
        (AccessLevel::Manager, StopServer::spec().into()),
        (AccessLevel::Manager, StartServer::spec().into()),
        (AccessLevel::Manager, RestartServer::spec().into()),
        (AccessLevel::Manager, ShutdownServer::spec().into()),
        (AccessLevel::Manager, BrowseFiles::spec().into()),
        (AccessLevel::Manager, ReadFile::spec().into()),
        (AccessLevel::Manager, ReadLogs::spec().into()),
        (AccessLevel::Manager, RunShell::spec().into()),
        (AccessLevel::Manager, EditFile::spec().into()),
        (AccessLevel::Manager, WriteFile::spec().into()),
        (AccessLevel::Manager, RestoreFile::spec().into()),
        (AccessLevel::Manager, RunWhen::spec().into()),
        (AccessLevel::Manager, BackupServer::spec().into()),
        (AccessLevel::Manager, GameGuide::spec().into()),
        (AccessLevel::Manager, SearchMods::spec().into()),
        (AccessLevel::Manager, ModInfo::spec().into()),
        (AccessLevel::Manager, ImportProfile::spec().into()),
        (AccessLevel::Manager, Remember::spec().into()),
        (AccessLevel::Manager, Forget::spec().into()),
        (AccessLevel::Manager, SetResources::spec().into()),
        (AccessLevel::Admin, DestroyServer::spec().into()),
        (AccessLevel::Admin, DeleteArchive::spec().into()),
        (AccessLevel::Admin, ArchiveServer::spec().into()),
        (AccessLevel::Admin, RestoreServer::spec().into()),
        (AccessLevel::Admin, RecoverServer::spec().into()),
        (AccessLevel::Admin, SendCommand::spec().into()),
    ]
}

/// The tools advertised to the model for a given caller: everything at or below
/// the caller's tier.
pub(crate) fn available_tools(access: AccessLevel) -> Vec<ToolDef> {
    tool_table()
        .into_iter()
        .filter(|(tier, _)| access >= *tier)
        .map(|(_, tool)| tool)
        .collect()
}

/// The lowest tier that may call `name`, or `None` if it isn't a tool at all.
fn tier_of(name: &str) -> Option<AccessLevel> {
    tool_table()
        .into_iter()
        .find(|(_, tool)| tool.function.name == name)
        .map(|(tier, _)| tier)
}

/// Whether `name` is a tool at all, at any tier. Distinguishes a typo (which a
/// caller naming tools by hand should be told about) from a real tool the caller
/// isn't allowed to use — the latter still dispatches, so it can produce the tier
/// refusal rather than a "no such tool" that hides the actual reason.
pub(crate) fn known_tool(name: &str) -> bool {
    tier_of(name).is_some()
}

/// Run one tool call and return the text result to feed back to the model. Bad
/// arguments, unknown names, and below-tier attempts all return an explanatory
/// string rather than failing the loop.
///
/// The tier gate runs first, once, against [`tool_table`] — defense in depth,
/// since a below-tier tool was never offered to this caller in the first place.
/// Because it is checked here, the arms below carry no tier guards of their own.
///
/// Before dispatching, any tool that targets an existing server by name is
/// confined to the caller's [`ServerScope`](ToolCtx::scope): a server in another
/// guild reads as "no such server", so Gary can neither see nor touch another
/// group's servers. The tools not gated here each enforce tenancy on their own:
/// `list_servers` and `list_archives` scope-filter their own listing,
/// `create_server` stamps the new server with the current guild,
/// `recover_server` resolves the archive within the caller's scope-filtered
/// listing (see `exec_recover`), and `escalate` reaches no server at all.
pub(crate) async fn dispatch(ctx: &ToolCtx<'_>, call: &ToolCall) -> String {
    let args = call.function.arguments.as_str();
    match tier_of(call.function.name.as_str()) {
        Some(required) if ctx.access < required => {
            return tier_refusal(call.function.name.as_str());
        }
        Some(_) => {}
        None => {
            return prompts::UnknownTool {
                name: &call.function.name,
            }
            .render();
        }
    }
    if targets_existing_server(call.function.name.as_str())
        && let Ok(TargetName { name }) = serde_json::from_str::<TargetName>(args)
        && let Some(refusal) = scope_refusal(ctx, &name).await
    {
        return refusal;
    }
    let name = call.function.name.as_str();
    match name {
        ListServers::NAME => exec_list_servers(ctx).await,
        AboutServer::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_about_server(ctx, &params.name).await,
            Err(message) => message,
        },
        ListBackups::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_list_backups(ctx, &params.name).await,
            Err(message) => message,
        },
        ListArchives::NAME => exec_list_archives(ctx).await,
        // The two r2modman profile tools sit at opposite tiers but share a
        // dispatcher, because they are one feature and their arms would
        // otherwise be pages apart.
        ExportProfile::NAME | ImportProfile::NAME => dispatch_profiles(ctx, name, args).await,
        ServerResources::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_server_resources(ctx, &params.name).await,
            Err(message) => message,
        },
        // The mod tools target no server at all — a guide is text and a registry
        // lookup reads a public catalog, not the cluster — so they take neither
        // the scope gate nor a server argument.
        GameGuide::NAME => match parse::<GameGuideParams>(args) {
            Ok(params) => exec_game_guide(ctx, &params.game),
            Err(message) => message,
        },
        SearchMods::NAME => match parse::<SearchModsParams>(args) {
            Ok(params) => exec_search_mods(ctx, params).await,
            Err(message) => message,
        },
        ModInfo::NAME => match parse::<ModInfoParams>(args) {
            Ok(params) => exec_mod_info(ctx, &params.game, &params.slug).await,
            Err(message) => message,
        },
        // Memory tools carry no server name (memory is shared across guilds), so
        // they skip the scope gate above and dispatch on their own.
        Remember::NAME | Forget::NAME => dispatch_memory(ctx, name, args).await,
        // Escalation names a server only as a hint for the operator notice, and
        // is deliberately outside the scope gate: a handoff about a server this
        // caller can't see still has to reach a human rather than being refused
        // as "no such server".
        Escalate::NAME => match parse::<EscalateParams>(args) {
            Ok(params) => exec_escalate(ctx, &params),
            Err(message) => message,
        },
        _ => dispatch_mutating(ctx, name, args).await,
    }
}

/// Dispatch the r2modman profile tools. Both target a server by name, so both
/// have already passed the scope gate in [`dispatch`]; their tiers — read-only
/// for the export, manager for the import — are settled there too.
///
/// Export is read-only despite handing back a shareable identifier: it changes
/// nothing a friend can see, and the cached code is what keeps a read-only
/// caller from minting permanent artifacts on a third party's service one
/// question at a time.
async fn dispatch_profiles(ctx: &ToolCtx<'_>, name: &str, args: &str) -> String {
    match name {
        ExportProfile::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_export_profile(ctx, &params.name).await,
            Err(message) => message,
        },
        ImportProfile::NAME => match parse::<ImportProfileParams>(args) {
            Ok(params) => exec_import_profile(ctx, &params.name, &params.code).await,
            Err(message) => message,
        },
        other => prompts::UnknownTool { name: other }.render(),
    }
}

/// Dispatch the tools that read or change one server's files — the inspection
/// set (browse/read/logs/shell) and the editing set (write/edit/restore). All
/// seven name a server and so have already passed the scope gate in
/// [`dispatch`]; their manager tier is settled there too.
///
/// Inspection and editing stay together because they are two halves of one
/// motion: the shell is how a change is *diagnosed*, and the typed writes are
/// the only path that snapshots the file first, which is what arms the
/// auto-rollback in [`verify_change`].
async fn dispatch_files(ctx: &ToolCtx<'_>, name: &str, args: &str) -> String {
    match name {
        BrowseFiles::NAME => match parse::<PathParams>(args) {
            Ok(params) => exec_browse_files(ctx, &params.name, &params.path).await,
            Err(message) => message,
        },
        ReadFile::NAME => match parse::<ReadFileParams>(args) {
            Ok(params) => match narrow_window(params.start_line, params.line_count) {
                Ok(window) => exec_read_file(ctx, &params.name, &params.path, window).await,
                Err(message) => message,
            },
            Err(message) => message,
        },
        ReadLogs::NAME => match parse::<ReadLogsParams>(args) {
            Ok(params) => match narrow_lines(params.lines) {
                Ok(lines) => exec_read_logs(ctx, &params.name, lines).await,
                Err(message) => message,
            },
            Err(message) => message,
        },
        RunShell::NAME => match parse::<RunShellParams>(args) {
            Ok(params) => exec_run_shell(ctx, &params.name, &params.command).await,
            Err(message) => message,
        },
        WriteFile::NAME => match parse::<WriteFileParams>(args) {
            Ok(params) => exec_write_file(ctx, &params.name, &params.path, &params.content).await,
            Err(message) => message,
        },
        EditFile::NAME => match parse::<EditFileParams>(args) {
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
        RestoreFile::NAME => match parse::<PathParams>(args) {
            Ok(params) => exec_restore_file(ctx, &params.name, &params.path).await,
            Err(message) => message,
        },
        other => prompts::UnknownTool { name: other }.render(),
    }
}

/// Dispatch the memory tools, kept out of [`dispatch_mutating`] because they
/// target no server and take no scope gate. Their manager tier is enforced by
/// the gate in [`dispatch`], like every other tool's.
async fn dispatch_memory(ctx: &ToolCtx<'_>, name: &str, args: &str) -> String {
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
/// admin-only destructive set. Split out of [`dispatch`] so each stays under the
/// line cap. Tiering is already settled by the gate in [`dispatch`], so the arms
/// here are plain name matches.
async fn dispatch_mutating(ctx: &ToolCtx<'_>, name: &str, args: &str) -> String {
    match name {
        CreateServer::NAME => match parse::<CreateServerParams>(args) {
            Ok(params) => exec_create(ctx, &params.game, params.name.as_deref()).await,
            Err(message) => message,
        },
        StopServer::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_stop(ctx, &params.name).await,
            Err(message) => message,
        },
        StartServer::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_start(ctx, &params.name).await,
            Err(message) => message,
        },
        RestartServer::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_restart(ctx, &params.name).await,
            Err(message) => message,
        },
        ShutdownServer::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_shutdown(ctx, &params.name).await,
            Err(message) => message,
        },
        BrowseFiles::NAME
        | ReadFile::NAME
        | ReadLogs::NAME
        | RunShell::NAME
        | WriteFile::NAME
        | EditFile::NAME
        | RestoreFile::NAME => dispatch_files(ctx, name, args).await,
        RunWhen::NAME => match parse::<RunWhenParams>(args) {
            Ok(params) => {
                exec_run_when(ctx, &params.name, params.condition.into(), &params.task).await
            }
            Err(message) => message,
        },
        BackupServer::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_backup(ctx, &params.name).await,
            Err(message) => message,
        },
        DestroyServer::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_destroy(ctx, &params.name).await,
            Err(message) => message,
        },
        DeleteArchive::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_delete_archive(ctx, &params.name).await,
            Err(message) => message,
        },
        SetResources::NAME => match parse::<SetResourcesParams>(args) {
            Ok(params) => exec_set_resources(ctx, &params).await,
            Err(message) => message,
        },
        SendCommand::NAME => match parse::<SendCommandParams>(args) {
            Ok(params) => exec_send_command(ctx, &params.name, &params.command).await,
            Err(message) => message,
        },
        ArchiveServer::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_archive(ctx, &params.name).await,
            Err(message) => message,
        },
        RestoreServer::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_restore(ctx, &params.name).await,
            Err(message) => message,
        },
        RecoverServer::NAME => match parse::<NameParams>(args) {
            Ok(params) => exec_recover(ctx, &params.name).await,
            Err(message) => message,
        },
        other => prompts::UnknownTool { name: other }.render(),
    }
}

/// The refusal for a tool the caller's tier may not use, naming the tier that
/// could. Reads the same [`tool_table`] the offer list does, so the ceiling it
/// relays can't drift from the one actually enforced.
fn tier_refusal(name: &str) -> String {
    match tier_of(name) {
        Some(AccessLevel::Admin) => prompts::NonAdminRefusal::render(),
        Some(AccessLevel::Manager) => prompts::NonManagerRefusal::render(),
        // A read-only tool is below every caller's tier, so reaching here means
        // the name isn't a tool at all.
        Some(AccessLevel::ReadOnly) | None => prompts::UnknownTool { name }.render(),
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

/// Narrow the model-supplied line window to the unsigned form `read_file` wants,
/// refusing a negative rather than silently reading somewhere else.
///
/// The generated schema carries both as signed integers (the v1 tool vocab has no
/// unsigned type), so a negative value is possible on the wire. An omitted half
/// takes its default rather than widening the request: a caller that named only
/// a start wants a window there, not the whole file.
fn narrow_window(start: Option<i64>, count: Option<i64>) -> Result<ReadWindow, String> {
    let start = match start {
        None => 1,
        Some(value) => match usize::try_from(value) {
            Ok(line) => line.max(1),
            Err(_) => return Err(prompts::NegativeLineNumber::render()),
        },
    };
    let count = match count {
        None => DEFAULT_READ_LINES,
        Some(value) => match usize::try_from(value) {
            Ok(lines) => lines,
            Err(_) => return Err(prompts::NegativeLineCount::render()),
        },
    };
    Ok(ReadWindow::Lines { start, count })
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
/// (no existing target — stamps the current guild), `delete_archive` (targets an
/// archive, which has no live server, and resolves it within the caller's scope),
/// and `recover_server` (resolves
/// the archive within the caller's scope). Also excluded because they reach no
/// server at all: `remember`/`forget` (memory is cross-guild) and
/// `search_mods`/`mod_info` (they read a public registry, not the cluster, so
/// there is no tenant to confine) and `game_guide` (it returns text). Keep this
/// in sync with those tools.
fn targets_existing_server(tool: &str) -> bool {
    matches!(
        tool,
        AboutServer::NAME
            | StopServer::NAME
            | StartServer::NAME
            | RestartServer::NAME
            | ShutdownServer::NAME
            | DestroyServer::NAME
            | BrowseFiles::NAME
            | ReadFile::NAME
            | ReadLogs::NAME
            | RunShell::NAME
            | WriteFile::NAME
            | EditFile::NAME
            | RestoreFile::NAME
            | SendCommand::NAME
            | RunWhen::NAME
            | ListBackups::NAME
            | BackupServer::NAME
            | ArchiveServer::NAME
            | RestoreServer::NAME
            | ServerResources::NAME
            | SetResources::NAME
            | ImportProfile::NAME
            | ExportProfile::NAME
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

async fn exec_about_server(ctx: &ToolCtx<'_>, server: &str) -> String {
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
            error!(error = ?err, "agent: about_server failed");
            return cluster_error();
        }
    };
    let Some(summary) = summaries.iter().find(|summary| summary.name == server) else {
        return no_such(server);
    };
    let mut out = format_summary(summary);
    out.push_str(&occupancy_line(ctx, server).await);
    out.push_str(&process_line(ctx, server).await);
    out.push_str(&history_lines(ctx, server).await);
    out
}

/// Lifecycle kinds that count as "something was done to this server". Excludes
/// `ServerCreated` — creation is not a *change*, and reporting it as the last one
/// on a server nobody has touched since would read as recent activity.
const LAST_ACTION_KINDS: &[EventKind] = &[
    EventKind::ServerStarted,
    EventKind::ServerStopped,
    EventKind::ServerRestarted,
    EventKind::ServerShutdown,
    EventKind::ServerRestored,
    EventKind::ServerRecovered,
    // A resize leaves no other trace anyone would notice — the pod isn't
    // recreated and the process never blinks — so this is the only place someone
    // asking "what happened to this server?" would find out that its headroom
    // changed under it.
    EventKind::ResourcesChanged,
];

/// Uptime and crash count, read live from the supervisor. Neither is derivable
/// from the Agones state the listing shows: a server crash-looping every few
/// minutes is `Ready` in every listing between relaunches.
async fn process_line(ctx: &ToolCtx<'_>, server: &str) -> String {
    let status = supervisor_status(
        &ctx.data.kube_client,
        &ctx.data.control_http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
    )
    .await;
    let rendered = match status {
        Ok(StatusPoll::Ok(status)) => prompts::AboutServerProcess {
            uptime: &format_uptime(status.uptime_seconds),
            restarts: &status.restarts.to_string(),
        }
        .render(),
        Ok(_) => prompts::AboutServerProcessUnknown::render(),
        Err(err) => {
            error!(error = ?err, %server, "agent: about_server status lookup failed");
            prompts::AboutServerProcessUnknown::render()
        }
    };
    format!("\n{rendered}")
}

/// When this server was last backed up and last acted on, from the product event
/// log. Both degrade to an explicit "no record" line rather than being dropped —
/// the log is optional, and a missing line would let the model infer the more
/// alarming of the two readings ("never backed up") from what may just be an
/// absent database.
async fn history_lines(ctx: &ToolCtx<'_>, server: &str) -> String {
    let mut kinds = vec![EventKind::BackupTaken];
    kinds.extend_from_slice(LAST_ACTION_KINDS);
    let events = ctx.data.events.latest_per_kind(server, &kinds).await;

    let backup = events
        .iter()
        .find(|event| event.kind == EventKind::BackupTaken.as_str())
        .map_or_else(prompts::AboutServerNoBackup::render, |event| {
            prompts::AboutServerLastBackup {
                when: &event.created_at,
            }
            .render()
        });

    // `latest_per_kind` returns one row per kind, so the most recent *action* is
    // the newest among them — the query can't order across kinds for us.
    let action = events
        .iter()
        .filter(|event| event.kind != EventKind::BackupTaken.as_str())
        .max_by(|a, b| a.created_at.cmp(&b.created_at))
        .map_or_else(prompts::AboutServerNoAction::render, |event| {
            prompts::AboutServerLastAction {
                action: past_tense(&event.kind),
                actor: &event.actor,
                when: &event.created_at,
            }
            .render()
        });

    format!("\n{backup}\n{action}")
}

/// An event kind as the model should read it back: `server_restarted` is a column
/// value, "restarted" is what happened. Kinds outside the `server_` family are
/// spelled out where the raw column value wouldn't read as English.
fn past_tense(kind: &str) -> &str {
    if kind == EventKind::ResourcesChanged.as_str() {
        return "had its resources changed";
    }
    kind.strip_prefix("server_").unwrap_or(kind)
}

/// Seconds as a coarse human duration. Deliberately lossy — the reader wants
/// "about a week" or "ten minutes", and a precise second count invites the model
/// to quote it back as though it were meaningful.
fn format_uptime(seconds: u64) -> String {
    let (days, hours, minutes) = (
        seconds / 86_400,
        (seconds % 86_400) / 3600,
        (seconds % 3600) / 60,
    );
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "just started".to_owned()
    }
}

/// A "Players online" line appended to `about_server` so Gary always sees
/// occupancy before deciding whether a restart would kick anyone. The count is a
/// live RCON read; games with no RCON (or a console that isn't up yet) report
/// `unknown`, which means "can't confirm it's empty" — never treat it as empty.
async fn occupancy_line(ctx: &ToolCtx<'_>, server: &str) -> String {
    let reason = match supervisor_occupancy(
        &ctx.data.kube_client,
        &ctx.data.control_http,
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

    let guild_key = ctx.guild.map(|guild| guild.to_string()).unwrap_or_default();
    let caps = if guild_key.is_empty() {
        None
    } else {
        Some(ctx.data.guild_limits.caps_for(&guild_key).await)
    };
    let provisioned = provision_instance(
        &ctx.data.kube_client,
        &ctx.data.namespace,
        &ctx.data.domain,
        &ctx.data.provision_lock,
        ProvisionSpec {
            entry,
            instance: &server,
            guild: &guild_key,
            caps: caps.as_ref(),
        },
    )
    .await;
    create_reply(ctx, game, &server, provisioned).await
}

/// Turn a provision result into Gary's reply, recording the terminal outcome of
/// everything except a successful provision — that one resolves later in
/// [`watch_boot`], once the boot it started has actually finished.
async fn create_reply(
    ctx: &ToolCtx<'_>,
    game: &str,
    server: &str,
    provisioned: anyhow::Result<ProvisionOutcome>,
) -> String {
    match provisioned {
        // Don't block the loop on first-boot world generation (minutes). Report
        // the address now; the user can ask for status to see when it's ready.
        Ok(ProvisionOutcome::Provisioned { address }) => {
            watch_boot(ctx, EventKind::ServerCreated, server, &address);
            prompts::CreateCreated {
                server,
                address: &address,
            }
            .render()
        }
        Ok(
            refused @ (ProvisionOutcome::AlreadyExists
            | ProvisionOutcome::NameUnavailable
            | ProvisionOutcome::PortsExhausted
            | ProvisionOutcome::GuildLimitReached(_)
            | ProvisionOutcome::ClusterFull(_)),
        ) => {
            let reply = refusal_reply(server, &refused).unwrap_or_else(|| {
                error!(server, "agent: create refusal had no reply to render");
                cluster_error()
            });
            ctx.record_action(
                EventKind::ServerCreated,
                server,
                mapping::provision(&refused),
            )
            .await;
            reply
        }
        Err(err) => {
            error!(error = ?err, game, server, "agent: create failed");
            ctx.record_action(EventKind::ServerCreated, server, mapping::FAILED)
                .await;
            cluster_error()
        }
    }
}

/// The reply explaining a refused create. `None` for a *successful* provision,
/// which has an address to report and is rendered by the caller instead — so a
/// caller that reaches here with one gets a generic fault rather than a confident
/// wrong answer.
fn refusal_reply(server: &str, outcome: &ProvisionOutcome) -> Option<String> {
    Some(match outcome {
        ProvisionOutcome::Provisioned { .. } => return None,
        ProvisionOutcome::AlreadyExists => prompts::CreateNameTaken { server }.render(),
        ProvisionOutcome::NameUnavailable => prompts::CreateNameUnavailable { server }.render(),
        ProvisionOutcome::PortsExhausted => prompts::CreatePortsExhausted::render(),
        ProvisionOutcome::GuildLimitReached(LimitRefusal::ServerCount { current, cap }) => {
            prompts::CreateGuildLimitServers {
                current: &current.to_string(),
                cap: &cap.to_string(),
            }
            .render()
        }
        ProvisionOutcome::GuildLimitReached(
            refusal @ (LimitRefusal::AggregateCpu { .. } | LimitRefusal::AggregateMemory { .. }),
        ) => prompts::CreateGuildLimitResources {
            detail: &describe_limit_refusal(*refusal),
        }
        .render(),
        ProvisionOutcome::ClusterFull(refusal) => prompts::CreateClusterFull {
            detail: &describe_capacity_refusal(*refusal),
        }
        .render(),
    })
}

/// Resolve a create or cold start's readiness in the background and record its
/// terminal outcome.
///
/// The tool itself must not wait — first-boot world generation runs for minutes
/// and the model would stall mid-turn — but the event still has to be terminal,
/// and time-to-ready can only be measured by whoever waits. So the wait moves off
/// the turn instead of off the record. The task is tracked, so the shutdown drain
/// knows about it, and races the shutdown token so a redeploy isn't held for the
/// full readiness budget.
fn watch_boot(ctx: &ToolCtx<'_>, kind: EventKind, server: &str, address: &str) {
    let (client, namespace) = (ctx.data.kube_client.clone(), ctx.data.namespace.clone());
    let events = std::sync::Arc::clone(&ctx.data.events);
    let shutdown = ctx.data.shutdown.clone();
    let attribution = ctx.attribution();
    let guild = ctx.guild.map(|guild| GuildId::new(guild.to_string()));
    let (server, address) = (server.to_owned(), address.to_owned());

    ctx.data.tasks.spawn(async move {
        let started = std::time::Instant::now();
        let ready = tokio::select! {
            resolved = wait_for_instance_ready(&client, &namespace, &server) => match resolved {
                Ok(ready) => ready,
                Err(err) => {
                    error!(error = ?err, %server, "agent: failed to wait for readiness");
                    false
                }
            },
            () = shutdown.cancelled() => return,
        };
        let elapsed = ready.then(|| started.elapsed());
        events
            .record_lifecycle(
                &client,
                &namespace,
                ProductEvent::new(kind, &attribution, mapping::readiness(ready))
                    .instance(InstanceName::new(server))
                    .guild(guild)
                    .details(mapping::boot_details(Some(&address), elapsed)),
            )
            .await;
    });
}

/// Report what one server is allowed to use and what it is actually using.
async fn exec_server_resources(ctx: &ToolCtx<'_>, server: &str) -> String {
    let sample = match sample_instance(&ctx.data.kube_client, &ctx.data.namespace, server).await {
        Ok(Some(sample)) => sample,
        Ok(None) => return prompts::ServerResourcesNotFound { server }.render(),
        Err(err) => {
            error!(error = ?err, %server, "agent: resource read failed");
            return cluster_error();
        }
    };
    let origin = if ctx.data.sizing.get(server).await.is_some() {
        prompts::ServerResourcesOverridden {
            defaults: &catalog_defaults(ctx, sample.game.as_ref()),
        }
        .render()
    } else {
        prompts::ServerResourcesCatalog::render()
    };
    format_resources(server, &sample, &origin)
}

/// This game's catalog sizing, worded as `set_resources` words a change — the
/// figures a resized server would go back to, since clearing an override means
/// setting them again by hand.
fn catalog_defaults(ctx: &ToolCtx<'_>, game: Option<&GameId>) -> String {
    catalog_defaults_summary(
        game.and_then(|game| ctx.data.catalog.get(game.as_str()))
            .and_then(catalog_sizing),
    )
}

/// "unknown" rather than a partial reading when the game has no catalog entry or
/// its template doesn't state all four figures: the sentence exists to be copied
/// into a resize, and three right figures beside one invented one is worse than
/// none at all.
fn catalog_defaults_summary(sizing: Option<Sizing>) -> String {
    sizing.map_or_else(|| "unknown".to_owned(), format_sizing_summary)
}

fn format_resources(server: &str, sample: &ResourceSample, origin: &str) -> String {
    prompts::ServerResourcesReport {
        server,
        cpu: &format_cpu_line(sample),
        memory: &format_memory_line(sample),
        disk: &sample.pvc_bytes.map_or_else(
            || "unknown".to_owned(),
            |bytes| human_size(bytes.unsigned_abs()),
        ),
        origin,
    }
    .render()
}

/// A null figure renders as "unknown", never as zero: metrics-server being down
/// is not the same as a server using no CPU, and the difference decides whether
/// a resize is warranted.
fn format_cpu_line(sample: &ResourceSample) -> String {
    format!(
        "{} reserved, {} maximum, {} in use now",
        millicores(sample.cpu_request_millicores),
        millicores(sample.cpu_limit_millicores),
        millicores(sample.cpu_millicores),
    )
}

fn format_memory_line(sample: &ResourceSample) -> String {
    format!(
        "{} reserved, {} maximum, {} in use now",
        bytes_or_unknown(sample.mem_request_bytes),
        bytes_or_unknown(sample.mem_limit_bytes),
        bytes_or_unknown(sample.mem_bytes),
    )
}

fn millicores(value: Option<i64>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |value| format!("{value}m"))
}

fn bytes_or_unknown(value: Option<i64>) -> String {
    value.map_or_else(
        || "unknown".to_owned(),
        |bytes| human_size(bytes.unsigned_abs()),
    )
}

/// Change what a server is allowed to use: record the override, then apply it to
/// the running container in place.
///
/// Recorded before it is applied, and refused outright when it can't be
/// recorded: a resize that lands on the pod but not in the database would be
/// undone by the next start, which is a worse outcome than not resizing.
async fn exec_set_resources(ctx: &ToolCtx<'_>, params: &SetResourcesParams) -> String {
    let sizing = match parse_sizing(params) {
        Ok(sizing) => sizing,
        Err(detail) => return prompts::SetResourcesOutOfRange { detail: &detail }.render(),
    };
    let bounds = ctx.data.settings.snapshot().sizing_bounds;
    if let Err(rejection) = sizing.validate(bounds) {
        return prompts::SetResourcesOutOfRange {
            detail: &describe_rejection(rejection, bounds),
        }
        .render();
    }
    // The aggregate cap binds the server's *owning* guild, read off its Service
    // label rather than the channel the request came from — so an operator DM
    // and the watchdog's unattended turns hit the same bound as a friend in the
    // guild (ADR-022). Usage excludes the target so the proposal replaces its
    // current figures instead of stacking onto them.
    match owner_of(&ctx.data.kube_client, &ctx.data.namespace, &params.name).await {
        Ok(Some(InstanceOwner {
            guild: Some(guild), ..
        })) => {
            let caps = ctx.data.guild_limits.caps_for(guild.as_str()).await;
            let usage = match guild_usage(
                &ctx.data.kube_client,
                &ctx.data.namespace,
                guild.as_str(),
                Some(&params.name),
            )
            .await
            {
                Ok(usage) => usage,
                Err(err) => {
                    error!(error = ?err, server = %params.name, "agent: guild usage read failed for resize");
                    return cluster_error();
                }
            };
            if let Err(refusal) = check_added_load(&usage, sizing, &caps) {
                ctx.record_action(
                    EventKind::ResourcesChanged,
                    &params.name,
                    mapping::limit_refusal(refusal),
                )
                .await;
                return prompts::SetResourcesGuildLimit {
                    detail: &describe_limit_refusal(refusal),
                }
                .render();
            }
        }
        // No Service or no guild label: an operator's unscoped instance, which
        // the caps don't bind. A missing instance is caught by the scope gate
        // before this tool runs.
        Ok(_) => {}
        Err(err) => {
            error!(error = ?err, server = %params.name, "agent: owner lookup failed for resize");
            return cluster_error();
        }
    }
    if !ctx.data.sizing.is_available() {
        return prompts::SetResourcesUnavailable::render();
    }
    let instance = InstanceName::new(&params.name);
    let actor = ctx.author_id.get().to_string();
    if let Err(err) = ctx.data.sizing.set(&instance, sizing, &actor).await {
        error!(error = ?err, server = %params.name, "agent: failed to record a resize");
        return prompts::SetResourcesUnavailable::render();
    }

    let outcome = match resize_instance(
        &ctx.data.kube_client,
        &ctx.data.namespace,
        &params.name,
        sizing,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(err) => {
            error!(error = ?err, server = %params.name, "agent: in-place resize failed");
            return cluster_error();
        }
    };
    ctx.record_action(
        EventKind::ResourcesChanged,
        &params.name,
        mapping::resize(&outcome),
    )
    .await;
    format_resize(&params.name, sizing, &outcome)
}

fn format_resize(server: &str, sizing: Sizing, outcome: &ResizeOutcome) -> String {
    let summary = format_sizing_summary(sizing);
    match outcome {
        ResizeOutcome::Applied => prompts::SetResourcesApplied {
            server,
            summary: &summary,
        }
        .render(),
        ResizeOutcome::InProgress => prompts::SetResourcesInProgress {
            server,
            summary: &summary,
        }
        .render(),
        // A destroyed or never-created instance reads the same as a stopped one
        // here: the override is saved either way, and a name that doesn't exist
        // is caught by the scope gate long before this.
        ResizeOutcome::NoPod | ResizeOutcome::NotFound | ResizeOutcome::NotManaged => {
            prompts::SetResourcesRecorded {
                server,
                summary: &summary,
            }
            .render()
        }
        ResizeOutcome::Infeasible(detail) => {
            prompts::SetResourcesRefused { server, detail }.render()
        }
    }
}

fn format_sizing_summary(sizing: Sizing) -> String {
    format!(
        "{} cpu / {} memory reserved, up to {} cpu / {} memory",
        millicores(Some(sizing.cpu_request_millicores)),
        human_size(sizing.mem_request_bytes.unsigned_abs()),
        millicores(Some(sizing.cpu_limit_millicores)),
        human_size(sizing.mem_limit_bytes.unsigned_abs()),
    )
}

/// Parse the model's four Kubernetes quantity strings, naming whichever one
/// didn't parse rather than reporting a generic failure.
fn parse_sizing(params: &SetResourcesParams) -> Result<Sizing, String> {
    let cpu = |field: &str, raw: &str| {
        parse_cpu_millicores(raw).ok_or_else(|| {
            format!("{field} isn't a CPU amount I understand: \"{raw}\". Use something like \"500m\" or \"2\".")
        })
    };
    let memory = |field: &str, raw: &str| {
        parse_bytes(raw).ok_or_else(|| {
            format!("{field} isn't a memory amount I understand: \"{raw}\". Use something like \"2Gi\" or \"512Mi\".")
        })
    };
    Ok(Sizing {
        cpu_request_millicores: cpu("cpu_request", &params.cpu_request)?,
        mem_request_bytes: memory("memory_request", &params.memory_request)?,
        cpu_limit_millicores: cpu("cpu_limit", &params.cpu_limit)?,
        mem_limit_bytes: memory("memory_limit", &params.memory_limit)?,
    })
}

/// Turn a validation failure into a sentence naming the figure and its bound, so
/// the model can propose a value that would work instead of retrying blindly.
/// The bounds are passed in rather than read from the constants, so the number
/// quoted is the one that actually refused the value.
fn describe_rejection(rejection: SizingRejection, bounds: SizingBounds) -> String {
    match rejection {
        SizingRejection::CpuBelowFloor => format!(
            "the CPU reservation is too small — {}m is the least a server can have",
            bounds.min_cpu_millicores
        ),
        SizingRejection::MemoryBelowFloor => format!(
            "the memory reservation is too small — {} is the least a server can have",
            human_size(bounds.min_mem_bytes.unsigned_abs())
        ),
        SizingRejection::CpuAboveCeiling => format!(
            "that's more CPU than any machine here has — {}m is the most a server can be given",
            bounds.max_cpu_millicores
        ),
        SizingRejection::MemoryAboveCeiling => format!(
            "that's more memory than any machine here has — {} is the most a server can be given",
            human_size(bounds.max_mem_bytes.unsigned_abs())
        ),
        SizingRejection::CpuRequestExceedsLimit => {
            "the CPU reservation is above its own maximum; the maximum has to be at least the reservation".to_owned()
        }
        SizingRejection::MemoryRequestExceedsLimit => {
            "the memory reservation is above its own maximum; the maximum has to be at least the reservation".to_owned()
        }
    }
}

/// Turn a guild-cap refusal into a sentence naming the figures, so the model
/// can explain the bound and offer a way out instead of retrying blindly.
fn describe_limit_refusal(refusal: LimitRefusal) -> String {
    match refusal {
        LimitRefusal::ServerCount { current, cap } => {
            format!("this community is at its server limit ({current} of {cap} in use)")
        }
        LimitRefusal::AggregateCpu {
            current_millicores,
            adding_millicores,
            cap_millicores,
        } => format!(
            "this needs {} of CPU on top of the {} the community's servers already hold, \
             but the community's allowance is {}",
            format_cores(adding_millicores),
            format_cores(current_millicores),
            format_cores(cap_millicores),
        ),
        LimitRefusal::AggregateMemory {
            current_bytes,
            adding_bytes,
            cap_bytes,
        } => format!(
            "this needs {} of memory on top of the {} the community's servers already hold, \
             but the community's allowance is {}",
            human_size(adding_bytes.unsigned_abs()),
            human_size(current_bytes.unsigned_abs()),
            human_size(cap_bytes.unsigned_abs()),
        ),
    }
}

/// The namespace-ceiling refusal as one sentence for a tool result. Says "the
/// service" rather than "the community", which is the whole distinction from
/// [`describe_limit_refusal`]: this one isn't about the caller's allowance and
/// the model must not offer to raise it.
fn describe_capacity_refusal(refusal: CapacityRefusal) -> String {
    match refusal {
        CapacityRefusal::Servers { current, ceiling } => format!(
            "the service is at its ceiling of {ceiling} servers ({current} in use across every \
             community)"
        ),
        CapacityRefusal::Storage {
            free_bytes,
            adding_bytes,
        } => format!(
            "the service has {} of storage left and this needs {}",
            human_size(free_bytes.unsigned_abs()),
            human_size(adding_bytes.unsigned_abs()),
        ),
        CapacityRefusal::Cpu {
            free_millicores,
            adding_millicores,
            ..
        } => format!(
            "the service has {} of CPU left and this needs {}",
            format_cores(free_millicores),
            format_cores(adding_millicores),
        ),
        CapacityRefusal::Memory {
            free_bytes,
            adding_bytes,
            ..
        } => format!(
            "the service has {} of memory left and this needs {}",
            human_size(free_bytes.unsigned_abs()),
            human_size(adding_bytes.unsigned_abs()),
        ),
        CapacityRefusal::Unspecified => "the service is out of room on the hardware".to_owned(),
    }
}

/// Record the model's decision to hand this turn to a human. Nothing is
/// delivered here: the loop stops on its next check and the shell — which is the
/// only layer that knows the surface's locators (a jump link, a player name, an
/// incident signal) — builds and sends the operator notice. Doing it that way
/// keeps one escalation path for both the model-invoked and budget-exhausted
/// exits instead of two that can drift.
fn exec_escalate(ctx: &ToolCtx<'_>, params: &EscalateParams) -> String {
    let message = params.message.trim();
    let reason = params.reason.trim();
    // Both are load-bearing — one is the only reply the user gets, the other the
    // only thing the operator is told — so an empty value is refused rather than
    // silently ending the turn on a blank message.
    if message.is_empty() || reason.is_empty() {
        return prompts::EscalateNeedBoth::render();
    }
    warn!(
        server = params.server.as_deref().unwrap_or("-"),
        origin = ?ctx.origin,
        user = %ctx.author_id,
        reason,
        "agent escalated: handed off to a human on its own"
    );
    ctx.note_escalation(EscalationRequest {
        reply: message.to_owned(),
        reason: reason.to_owned(),
        server: params
            .server
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
    });
    prompts::EscalateHandedOff::render()
}

async fn exec_remember(ctx: &ToolCtx<'_>, scope: &str, note: &str) -> String {
    let note = note.trim();
    if note.is_empty() {
        return prompts::RememberNeedFact::render();
    }
    let ids: Vec<&str> = ctx.data.catalog.game_ids().collect();
    let Some(scope) = normalize_scope(scope, &ids) else {
        return prompts::RememberBadScope {
            games: &game_ids(ctx),
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
        Ok(RememberOutcome::TooLong { max }) => prompts::RememberTooLong {
            max: &max.to_string(),
        }
        .render(),
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
        &ctx.data.control_http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
    )
    .await
    {
        Ok(outcome) => {
            ctx.record_action(
                EventKind::ServerStopped,
                server,
                mapping::supervisor(&outcome),
            )
            .await;
            format_supervisor(server, &outcome)
        }
        Err(err) => {
            error!(error = ?err, %server, "agent: stop failed");
            ctx.record_action(EventKind::ServerStopped, server, mapping::FAILED)
                .await;
            cluster_error()
        }
    }
}

async fn exec_restart(ctx: &ToolCtx<'_>, server: &str) -> String {
    let outcome = match supervisor_restart(
        &ctx.data.kube_client,
        &ctx.data.control_http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(err) => {
            error!(error = ?err, %server, "agent: restart failed");
            ctx.record_action(EventKind::ServerRestarted, server, mapping::FAILED)
                .await;
            return cluster_error();
        }
    };
    ctx.record_action(
        EventKind::ServerRestarted,
        server,
        mapping::supervisor(&outcome),
    )
    .await;
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
            &ctx.data.control_http,
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
                ctx.note_guardrail_verdict(GuardrailVerdict::Unverified);
                return prompts::ChangeRestartedUnverified { server }.render();
            }
        };
        match next_step(&outcome, rollback_spent) {
            RecoveryStep::Healthy => {
                ctx.note_guardrail_verdict(if rollback_spent {
                    GuardrailVerdict::RolledBack
                } else {
                    GuardrailVerdict::Healthy
                });
                return if rollback_spent {
                    warn!(%server, path = %change.path, "agent: rolled a crashing config change back and the server recovered");
                    let mut rendered = prompts::ChangeRolledBackHealthy {
                        path: change.path.as_str(),
                        server,
                    }
                    .render();
                    rendered.push_str(&crash_evidence_note());
                    rendered
                } else {
                    info!(%server, path = %change.path, "agent: config change verified healthy after restart");
                    prompts::ChangeHealthy { server }.render()
                };
            }
            RecoveryStep::RollBack => match roll_back(ctx, &change).await {
                Ok(()) => {
                    rollback_spent = true;
                }
                Err(message) => {
                    ctx.note_guardrail_verdict(GuardrailVerdict::RollbackFailed);
                    return message;
                }
            },
            RecoveryStep::Escalate => {
                ctx.note_guardrail_verdict(GuardrailVerdict::RollbackFailed);
                error!(%server, path = %change.path, "agent: config change crashed the server and a rollback did not recover it — escalating");
                notify_crash_rollback(ctx, server, &change.path).await;
                let escalation = prompts::OperatorFlagged::render();
                let mut rendered = prompts::ChangeRollbackFailedFlagged {
                    path: change.path.as_str(),
                    server,
                    escalation: &escalation,
                }
                .render();
                rendered.push_str(&crash_evidence_note());
                return rendered;
            }
            RecoveryStep::Inconclusive => {
                ctx.note_guardrail_verdict(GuardrailVerdict::Inconclusive);
                return format_ready_wait(server, &outcome);
            }
        }
    }
}

/// The note pointing the model at the crashed launch's preserved output.
///
/// Appended to every rollback outcome rather than only the unhappy ones: a
/// rollback that *worked* still leaves the question of what the change broke,
/// and that answer is now on the volume instead of gone.
fn crash_evidence_note() -> String {
    format!("\n\n{}", prompts::CrashOutputPreserved::render())
}

/// Restore the pre-edit snapshot and restart, the mechanical half of an automatic
/// rollback. `Ok` means the world is back on the old config and restarting, so the
/// caller re-polls readiness; `Err` carries the escalation text for a rollback that
/// couldn't even be issued (nothing left to do automatically).
async fn roll_back(ctx: &ToolCtx<'_>, change: &PendingChange) -> Result<(), String> {
    let server = change.server.as_str();
    let restore = supervisor_restore_file(
        &ctx.data.kube_client,
        &ctx.data.control_http,
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
        &ctx.data.control_http,
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
    let escalation = Escalation::CrashRollback {
        server: server.to_owned(),
        path: path.to_owned(),
    };
    ctx.record_event(mapping::escalation_event(
        &escalation,
        &ctx.attribution(),
        ctx.guild.map(|guild| GuildId::new(guild.to_string())),
    ))
    .await;
    ctx.data.notifier.notify(&escalation).await;
}

/// Mirrors the `/start` slash command's warm/cold routing: a live pod resumes in
/// place via the supervisor; a shut-down instance is rescheduled. Unlike the slash
/// command, the agent doesn't block waiting for readiness — it reports the
/// address and lets the user poll status.
async fn exec_start(ctx: &ToolCtx<'_>, server: &str) -> String {
    match instance_runtime_state(&ctx.data.kube_client, &ctx.data.namespace, server).await {
        Ok(RuntimeState::PodUp) => match supervisor_start(
            &ctx.data.kube_client,
            &ctx.data.control_http,
            &ctx.data.namespace,
            server,
            ctx.data.control_port,
        )
        .await
        {
            Ok(outcome) => {
                ctx.record_action(
                    EventKind::ServerStarted,
                    server,
                    mapping::supervisor(&outcome),
                )
                .await;
                format_supervisor(server, &outcome)
            }
            Err(err) => {
                error!(error = ?err, %server, "agent: warm start failed");
                ctx.record_action(EventKind::ServerStarted, server, mapping::FAILED)
                    .await;
                cluster_error()
            }
        },
        Ok(RuntimeState::Down) => exec_cold_start(ctx, server).await,
        Ok(RuntimeState::Terminating) => prompts::StartStillStopping { server }.render(),
        Ok(RuntimeState::Absent) => no_such(server),
        Err(err) => {
            error!(error = ?err, %server, "agent: start state lookup failed");
            cluster_error()
        }
    }
}

async fn exec_cold_start(ctx: &ToolCtx<'_>, server: &str) -> String {
    // One hold of the provision lock covers the cap gate and the GameServer
    // create, so a concurrent create can't add load in between (ADR-022). The
    // returns inside the block release it before any long wait.
    let begin = {
        let _guard = ctx.data.provision_lock.lock().await;
        match crate::discord::cold_start_refusal(ctx.data, server).await {
            Ok(None) => {}
            Ok(Some(refusal)) => {
                ctx.record_action(
                    EventKind::ServerStarted,
                    server,
                    mapping::limit_refusal(refusal),
                )
                .await;
                return prompts::ColdStartGuildLimit {
                    server,
                    detail: &describe_limit_refusal(refusal),
                }
                .render();
            }
            Err(err) => {
                error!(error = ?err, %server, "agent: cold start cap check failed");
                ctx.record_action(EventKind::ServerStarted, server, mapping::FAILED)
                    .await;
                return cluster_error();
            }
        }
        match begin_start(
            &ctx.data.kube_client,
            &ctx.data.namespace,
            &ctx.data.domain,
            &ctx.data.catalog,
            server,
            ctx.data.sizing.get(server).await,
        )
        .await
        {
            Ok(begin) => begin,
            Err(err) => {
                error!(error = ?err, %server, "agent: cold start failed");
                ctx.record_action(EventKind::ServerStarted, server, mapping::FAILED)
                    .await;
                return cluster_error();
            }
        }
    };

    // A start that's underway isn't terminal yet — the background watcher records
    // it once readiness resolves. Everything else is already its own ending.
    if let StartBegin::Starting { address } = &begin {
        watch_boot(ctx, EventKind::ServerStarted, server, address);
    } else {
        ctx.record_action(
            EventKind::ServerStarted,
            server,
            mapping::start_begin(&begin),
        )
        .await;
    }

    match begin {
        StartBegin::Starting { address } => prompts::ColdStartStarting {
            server,
            address: &address,
        }
        .render(),
        StartBegin::AlreadyRunning => prompts::ServerAlreadyRunning { server }.render(),
        StartBegin::NotFound => no_such(server),
        StartBegin::NotManaged => not_managed(server),
        StartBegin::UnknownGame(game) => prompts::ColdStartUnknownGame {
            server,
            game: &game,
        }
        .render(),
        StartBegin::ClusterFull(refusal) => prompts::ColdStartClusterFull {
            server,
            detail: &describe_capacity_refusal(refusal),
        }
        .render(),
    }
}

async fn exec_shutdown(ctx: &ToolCtx<'_>, server: &str) -> String {
    let outcome = match shutdown_instance(&ctx.data.kube_client, &ctx.data.namespace, server).await
    {
        Ok(outcome) => outcome,
        Err(err) => {
            error!(error = ?err, %server, "agent: shutdown failed");
            ctx.record_action(EventKind::ServerShutdown, server, mapping::FAILED)
                .await;
            return cluster_error();
        }
    };
    ctx.record_action(
        EventKind::ServerShutdown,
        server,
        mapping::shutdown(&outcome),
    )
    .await;
    match outcome {
        ShutdownOutcome::Down => prompts::ShutdownStopped { server }.render(),
        ShutdownOutcome::Stranded => prompts::ShutdownStranded { server }.render(),
        ShutdownOutcome::NotFound => no_such(server),
        ShutdownOutcome::NotManaged => not_managed(server),
    }
}

/// Permanent deletion is gated behind an explicit confirmation: the model can
/// request it, but a human must approve before any world is destroyed. The
/// returned text tells the model what was decided.
/// Delete a server's archives, behind the same confirmation gate as destroy.
///
/// Gated harder than it looks: `destroy_server` removes a running server while
/// deliberately leaving its archives standing, so this is the only path that
/// makes a world unrecoverable.
async fn exec_delete_archive(ctx: &ToolCtx<'_>, name: &str) -> String {
    let Some(service) = ctx.data.backup.clone() else {
        return backups_not_configured();
    };
    if !service.archives_enabled() {
        return DeleteArchiveUnavailable::render();
    }
    // Priced before the confirmation so the card can say how many copies and how
    // much storage actually go — "delete the smp archive" usually means more
    // generations than the listing showed, and a confirmation that understates
    // what it is approving isn't one.
    let (generations, freed) = match service.archive_footprint(&ctx.scope, name).await {
        Ok(Some(footprint)) => footprint,
        Ok(None) => return DeleteArchiveNotFound { name }.render(),
        Err(err) => {
            error!(error = ?err, %name, "agent: archive footprint lookup failed");
            return cluster_error();
        }
    };
    let cancelled = prompts::AbortCancelled::render();
    let timed_out = prompts::AbortTimedOut::render();
    match confirm_destructive(
        ctx,
        name,
        "delete the archive of",
        delete_archive_confirm_embed(name, generations, &human_size(freed)),
        "gary_delete_archive_confirm",
        prompts::ConfirmAborted {
            reason: &cancelled,
            server: name,
            verb: "deleted",
        }
        .render(),
        prompts::ConfirmAborted {
            reason: &timed_out,
            server: name,
            verb: "deleted",
        }
        .render(),
    )
    .await
    {
        Confirm::Declined(text) => text,
        Confirm::Confirmed(mut prompt) => {
            match service
                .delete_archives(&backup_ctx(ctx.data), &ctx.scope, name, &ctx.attribution())
                .await
            {
                Ok(outcome) => {
                    if let Some(reason) = outcome.reason() {
                        error!(reason, %name, "agent: delete_archive did not complete");
                    }
                    edit_prompt(ctx, &mut prompt, delete_archive_result_embed(&outcome)).await;
                    format_delete_archive(name, &outcome)
                }
                Err(err) => {
                    error!(error = ?err, %name, "agent: delete_archive failed");
                    let message = cluster_error();
                    edit_prompt(ctx, &mut prompt, error_embed(&message)).await;
                    message
                }
            }
        }
    }
}

fn format_delete_archive(name: &str, outcome: &ArchiveDeleteOutcome) -> String {
    match outcome {
        ArchiveDeleteOutcome::Deleted {
            generations,
            freed_bytes,
            ..
        } => DeleteArchiveDone {
            name,
            generations: &generations.to_string(),
            size: &human_size(*freed_bytes),
        }
        .render(),
        ArchiveDeleteOutcome::NotFound => DeleteArchiveNotFound { name }.render(),
        ArchiveDeleteOutcome::Unavailable => DeleteArchiveUnavailable::render(),
        ArchiveDeleteOutcome::Failed(_) => DeleteArchiveFailed { name }.render(),
    }
}

async fn exec_destroy(ctx: &ToolCtx<'_>, server: &str) -> String {
    match ctx.confirmations {
        ConfirmMode::Interactive => confirm_destroy_by_click(ctx, server).await,
        ConfirmMode::Preapproved => {
            let Some(mut prompt) =
                post_preapproval_audit(ctx, server, "destroy", destroy_confirm_embed(server)).await
            else {
                return prompts::DestroyNoConfirmChannel::render();
            };
            apply_destroy(ctx, server, &mut prompt).await
        }
    }
}

/// Wait for a click from someone entitled to approve, ignoring anyone who isn't.
///
/// The conversation belongs to the channel, so the person who asked for a
/// destructive action and the person allowed to approve it are routinely
/// different people — an admin signing off on a friend's request is the case
/// this exists for. Any click at `required` or above counts; anyone below is
/// told why, ephemerally, and the prompt stays live for someone who can. The
/// timeout is the total wait, not a per-click one, so a stream of unqualified
/// clicks can't hold a destructive prompt open indefinitely.
///
/// Returns the approving interaction **unacknowledged** — the caller answers it,
/// because only the caller knows whether it approved or cancelled.
async fn await_approval(
    ctx: &ToolCtx<'_>,
    message_id: serenity::MessageId,
    required: AccessLevel,
) -> Option<serenity::ComponentInteraction> {
    let mut clicks = ComponentInteractionCollector::new(ctx.serenity)
        .message_id(message_id)
        .timeout(COMPONENT_TIMEOUT)
        .stream();
    while let Some(interaction) = clicks.next().await {
        let clicker = Speaker {
            user: interaction.user.id.get(),
            guild: interaction.guild_id,
            roles: interaction
                .member
                .as_ref()
                .map(|member| member.roles.iter().map(|role| role.get()).collect())
                .unwrap_or_default(),
        };
        if access_level_for(ctx.serenity, ctx.data, &clicker).await >= required {
            return Some(interaction);
        }
        refuse_approval(ctx, &interaction, required).await;
    }
    None
}

/// Tell an unqualified clicker why nothing happened, only to them. Silence would
/// read as the button being broken — which is what it did before this gate
/// existed, since a non-matching click was simply dropped.
async fn refuse_approval(
    ctx: &ToolCtx<'_>,
    interaction: &serenity::ComponentInteraction,
    required: AccessLevel,
) {
    let notice = format!(
        "You need {} access to answer this one. Ask {} in this channel to confirm it.",
        required.as_str(),
        match required {
            AccessLevel::Admin => "an admin",
            AccessLevel::Manager | AccessLevel::ReadOnly => "a manager or admin",
        }
    );
    if let Err(err) = interaction
        .create_response(
            ctx.serenity,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(notice)
                    .ephemeral(true),
            ),
        )
        .await
    {
        warn!(error = ?err, user = %interaction.user.id, "agent: failed to refuse a confirmation click");
    }
}

/// Post the Danger/Cancel buttons and wait for a click from anyone who may
/// approve a destroy.
async fn confirm_destroy_by_click(ctx: &ToolCtx<'_>, server: &str) -> String {
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

    // destroy_server is admin-only, so an admin is who may sign it off.
    let decision = await_approval(ctx, prompt.id, AccessLevel::Admin).await;

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

    apply_destroy(ctx, server, &mut prompt).await
}

/// Delete the instance and swap the confirmation card for the result, once
/// approval has been established. `prompt` is whichever message carries that
/// card — the button prompt a human clicked, or the audit line a pre-approved
/// request posted — so the channel tells the same before/after story either way.
async fn apply_destroy(ctx: &ToolCtx<'_>, server: &str, prompt: &mut serenity::Message) -> String {
    match destroy_instance(&ctx.data.kube_client, &ctx.data.namespace, server).await {
        Ok(outcome) => {
            // Names are reused, so a resize left behind here would silently size
            // an unrelated future world. Logged rather than surfaced: the world
            // is already gone and a stale row is not the friend's problem.
            if let Err(err) = ctx.data.sizing.forget(server).await {
                error!(error = ?err, %server, "failed to clear the destroyed server's resource override");
            }
            ctx.record_action(
                EventKind::ServerDestroyed,
                server,
                mapping::destroy(&outcome),
            )
            .await;
            edit_prompt(ctx, prompt, destroy_result_embed(&outcome, server)).await;
            format_destroy(server, &outcome)
        }
        Err(err) => {
            error!(error = ?err, %server, "agent: destroy failed");
            ctx.record_action(EventKind::ServerDestroyed, server, mapping::FAILED)
                .await;
            let message = cluster_error();
            edit_prompt(ctx, prompt, error_embed(&message)).await;
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
        &ctx.data.control_http,
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

async fn exec_read_file(ctx: &ToolCtx<'_>, server: &str, path: &str, window: ReadWindow) -> String {
    match supervisor_read_file(
        &ctx.data.kube_client,
        &ctx.data.control_http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        path,
        window,
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

/// Run one read-only shell command in the server's container and hand back what
/// it printed. The sandbox lives in the supervisor, so there is nothing to police
/// here: a command that tries to write is refused by the kernel, and the refusal
/// arrives as ordinary stderr the model can read and respond to.
async fn exec_run_shell(ctx: &ToolCtx<'_>, server: &str, command: &str) -> String {
    match supervisor_run_shell(
        &ctx.data.kube_client,
        &ctx.data.control_http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        command,
    )
    .await
    {
        Ok(outcome) => match fs_result(server, outcome) {
            Ok(Some(result)) => format_shell(server, command, &result),
            Ok(None) => prompts::ShellUnsupported::render(),
            Err(problem) => problem,
        },
        Err(err) => {
            error!(error = ?err, %server, "agent: run_shell failed");
            cluster_error()
        }
    }
}

/// The platform's own account of how the command ended, kept outside the fenced
/// output so nothing the command printed can impersonate it.
fn format_shell_status(result: &ShellResponse) -> String {
    if result.timed_out {
        return prompts::ShellTimedOut::render();
    }
    match result.exit_code {
        Some(code) => prompts::ShellExited {
            code: &code.to_string(),
        }
        .render(),
        None => prompts::ShellSignalled::render(),
    }
}

fn format_shell(server: &str, command: &str, result: &ShellResponse) -> String {
    let status = format_shell_status(result);
    let mut body = result.stdout.clone();
    if !result.stderr.is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&result.stderr);
    }
    if body.trim().is_empty() {
        return prompts::ShellNoOutput {
            server,
            command,
            status: &status,
        }
        .render();
    }
    let scrubbed = crate::untrusted::scrub(&body);
    let mut rendered = prompts::ShellOutput {
        server,
        command,
        status: &status,
        output: &scrubbed,
    }
    .render();
    if result.truncated {
        rendered.push_str("\n\n");
        rendered.push_str(&prompts::ShellTruncated::render());
    }
    rendered
}

async fn exec_read_logs(ctx: &ToolCtx<'_>, server: &str, lines: Option<usize>) -> String {
    match supervisor_read_logs(
        &ctx.data.kube_client,
        &ctx.data.control_http,
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
        &ctx.data.control_http,
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
        &ctx.data.control_http,
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
        &ctx.data.control_http,
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
        &ctx.data.control_http,
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
        &ctx.data.control_http,
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
    match service.list_backups(&backup_ctx(ctx.data), server).await {
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

/// Load one game's mod workflow. Reaches nothing at all — the catalog is
/// already in memory — so this dispatches ahead of the scope gate and costs
/// nothing when the answer is "this game takes no mods".
fn exec_game_guide(ctx: &ToolCtx<'_>, game: &str) -> String {
    let subject = match ctx.data.catalog.get(game) {
        None => GuideSubject::UnknownGame,
        Some(entry) => match entry.mod_registry.as_ref() {
            Some(registry) => GuideSubject::Registry(registry),
            None => GuideSubject::NoRegistry,
        },
    };
    let ids = game_ids(ctx);
    render_guide(game, &subject, if ids.is_empty() { "(none)" } else { &ids })
}

/// The registry a catalog game declares, or the text to hand back instead of
/// searching: an unknown game is a typo to correct, a registry-less game is a
/// final answer. Both mod tools resolve their `game` argument through here, so
/// the two refusals are worded once.
fn registry_for_game(ctx: &ToolCtx<'_>, game: &str) -> Result<ModRegistry, String> {
    let Some(entry) = ctx.data.catalog.get(game) else {
        return Err(mod_game_unknown(ctx, game));
    };
    entry
        .mod_registry
        .clone()
        .ok_or_else(|| mods_not_supported(game))
}

/// Search a game's mod registry. Reaches no server, so it needs nothing from
/// the cluster — only the catalog and the shared HTTP client.
async fn exec_search_mods(ctx: &ToolCtx<'_>, params: SearchModsParams) -> String {
    let registry = match registry_for_game(ctx, &params.game) {
        Ok(registry) => registry,
        Err(refusal) => return refusal,
    };
    // Computed before anything consumes `params`, so the "filters ignored"
    // notice describes the call the model actually made.
    let ignored = minecraft_only_filters(&params);
    let side = params.side.map(Side::from);
    match registry {
        ModRegistry::Modrinth => {
            let query = SearchQuery {
                text: params.query,
                kind: params.kind.map(Kind::from),
                loader: params.loader.map(Loader::from),
                game_version: params.game_version,
                side,
            };
            match search_projects(&ctx.data.http, &query).await {
                Ok(hits) => format_mod_search(&query.text, query.game_version.as_deref(), &hits),
                Err(err) => {
                    error!(error = ?err, query = %query.text, "agent: modrinth search failed");
                    modrinth_unavailable()
                }
            }
        }
        ModRegistry::Thunderstore { community, .. } => {
            let query = ThunderstoreQuery {
                community,
                text: params.query,
                side,
            };
            let listing = match search_packages(&ctx.data.http, &query).await {
                Ok(hits) => format_thunderstore_search(&params.game, &query.text, &hits),
                Err(err) => {
                    error!(
                        error = ?err,
                        query = %query.text,
                        community = %query.community,
                        "agent: thunderstore search failed"
                    );
                    thunderstore_unavailable()
                }
            };
            // The Minecraft-only filters are reported as dropped rather than
            // silently ignored: a narrowing that never happened reads as a real
            // one, and every result would then be miscategorised downstream.
            if ignored.is_empty() {
                listing
            } else {
                format!(
                    "{}\n{listing}",
                    prompts::ModrinthFiltersIgnored {
                        game: &params.game,
                        filters: &ignored.join(", "),
                    }
                    .render()
                )
            }
        }
    }
}

/// The Minecraft-only search parameters this call set, in schema order. Empty
/// when the call is portable across registries.
fn minecraft_only_filters(params: &SearchModsParams) -> Vec<&'static str> {
    let mut set = Vec::new();
    if params.kind.is_some() {
        set.push("kind");
    }
    if params.loader.is_some() {
        set.push("loader");
    }
    if params.game_version.is_some() {
        set.push("game_version");
    }
    set
}

/// Look one mod up by the identifier its registry uses. An identifier the
/// registry doesn't know is an ordinary answer — the exact case this tool exists
/// to catch — not an error.
async fn exec_mod_info(ctx: &ToolCtx<'_>, game: &str, slug: &str) -> String {
    let registry = match registry_for_game(ctx, game) {
        Ok(registry) => registry,
        Err(refusal) => return refusal,
    };
    match registry {
        ModRegistry::Modrinth => match fetch_project(&ctx.data.http, slug).await {
            Ok(Some(project)) => format_mod_info(&project),
            Ok(None) => prompts::ModNotFound { slug }.render(),
            Err(err) => {
                error!(error = ?err, %slug, "agent: modrinth project lookup failed");
                modrinth_unavailable()
            }
        },
        ModRegistry::Thunderstore { community, .. } => {
            // A bare slug is not a lookup that happens to miss — it is the wrong
            // shape of identifier, so it is answered without a call rather than
            // as a 404 that would read as "no such mod".
            let Some((namespace, name)) = split_full_name(slug) else {
                return prompts::ThunderstoreNotFound { game, slug }.render();
            };
            match fetch_package(&ctx.data.http, namespace, name).await {
                Ok(Some(package)) => format_thunderstore_info(&package, &community),
                Ok(None) => prompts::ThunderstoreNotFound { game, slug }.render(),
                Err(err) => {
                    error!(
                        error = ?err,
                        %slug,
                        %community,
                        "agent: thunderstore package lookup failed"
                    );
                    thunderstore_unavailable()
                }
            }
        }
    }
}

async fn exec_backup(ctx: &ToolCtx<'_>, server: &str) -> String {
    let Some(service) = ctx.data.backup.clone() else {
        return backups_not_configured();
    };
    match service
        .backup_instance(&backup_ctx(ctx.data), server, &ctx.attribution())
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
        server,
        "archive",
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
                .archive_instance(&backup_ctx(ctx.data), server, &ctx.attribution())
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
    let backups = match service.list_backups(&backup_ctx(ctx.data), server).await {
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
        server,
        "restore",
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
                .restore_backup(&backup_ctx(ctx.data), server, &key, &ctx.attribution())
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
        .recover_archive(&backup_ctx(ctx.data), &guild, name, &ctx.attribution())
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

/// Posted in-channel whenever a destructive action runs without a button click.
/// The visible trail is the entire reason pre-approval is acceptable, so an
/// action that can't post this is refused rather than run unrecorded.
const PREAPPROVED_AUDIT_LINE: &str = "⚠️ Pre-approved by the operator harness — this was approved \
                                      out of band, not by a button click here.";

/// Announce a pre-approved destructive action in-channel: the same card the
/// interactive prompt shows, with no buttons, above a line saying nobody clicked.
/// Returns the posted message so the caller can swap in the result card, or
/// `None` when it couldn't be delivered — in which case the action must not run.
async fn post_preapproval_audit(
    ctx: &ToolCtx<'_>,
    server: &str,
    action: &str,
    card: serenity::CreateEmbed,
) -> Option<serenity::Message> {
    warn!(
        %server,
        action,
        origin = ?ctx.origin,
        user = %ctx.author_id,
        channel = %ctx.channel_id,
        "destructive action pre-approved; no human clicked through"
    );
    match ctx
        .channel_id
        .send_message(
            ctx.serenity,
            CreateMessage::new()
                .content(PREAPPROVED_AUDIT_LINE)
                .embed(card),
        )
        .await
    {
        Ok(message) => Some(message),
        Err(err) => {
            error!(
                error = ?err, %server, action,
                "agent: couldn't post the pre-approval audit line; refusing the action"
            );
            None
        }
    }
}

/// Obtain approval for a destructive action, mirroring the destroy gate for
/// archive/restore. Interactively that means posting Danger/Cancel buttons and
/// waiting for the invoking user's click; pre-approved, it means posting the
/// audit line and proceeding.
async fn confirm_destructive(
    ctx: &ToolCtx<'_>,
    server: &str,
    action: &str,
    prompt: serenity::CreateEmbed,
    confirm_id: &str,
    cancel_line: String,
    timeout_line: String,
) -> Confirm {
    match ctx.confirmations {
        ConfirmMode::Interactive => {}
        ConfirmMode::Preapproved => {
            return post_preapproval_audit(ctx, server, action, prompt)
                .await
                .map_or_else(
                    || Confirm::Declined(prompts::ConfirmNoChannel::render()),
                    |message| Confirm::Confirmed(Box::new(message)),
                );
        }
    }
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
            return Confirm::Declined(prompts::ConfirmNoChannel::render());
        }
    };
    // Every tool routed through here is admin-tier (archive, restore, delete).
    let decision = await_approval(ctx, prompt_msg.id, AccessLevel::Admin).await;
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
        &ctx.data.control_http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
        &format!("Gary: {phrase}"),
    )
    .await;
}

/// Collapse a filesystem outcome into either its payload or a plain-language
/// explanation of why the operation couldn't be served, for the model to relay.
pub(crate) fn fs_result<T>(server: &str, outcome: FsOutcome<T>) -> Result<T, String> {
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

pub(crate) fn cluster_error() -> String {
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
            EntryKind::File => format!("{} ({} bytes)", entry.name, entry.size_bytes),
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

/// Render a file window for the model: every line numbered from its real
/// position in the file, under a note saying which slice this is.
///
/// Numbering is what makes the window addressable. Without it the model can see
/// the text but has no way to name a location in it — neither to ask for the
/// next window nor to say where a value it found actually sits.
fn format_file(file: &ReadResponse) -> String {
    // Each parenthetical's leading space is a code-owned separator; the note
    // prompts carry none, so the empty case renders cleanly.
    let mut notes = String::new();
    let shown = file.content.lines().count();
    let first = file.start_line.max(1);
    // A window shorter than the whole file gets a range; a file that fits
    // entirely in the window gets no note, because there is nothing the reader
    // needs to act on.
    if file.truncated || shown < file.total_lines {
        let last = first.saturating_add(shown.saturating_sub(1));
        notes.push(' ');
        notes.push_str(
            &prompts::FileWindowNote {
                first: &first.to_string(),
                last: &last.to_string(),
                total: &file.total_lines.to_string(),
            }
            .render(),
        );
    }
    if file.line_cut {
        notes.push(' ');
        notes.push_str(&prompts::FileLineCut::render());
    }
    let note = notes;
    prompts::FileContents {
        path: file.path.as_str(),
        note: &note,
        content: &number_lines(&crate::untrusted::scrub(&file.content), first),
    }
    .render()
}

/// Prefix each line with its 1-based position in the file, starting at `first`.
///
/// Runs *after* the untrusted scrub, so a line the scrub redacted still carries
/// its number and the numbering stays continuous. Order is safe either way — the
/// scrub matches its markers anywhere in a line, so a number in front wouldn't
/// hide one — but a redaction that kept its position is easier to ask about.
///
/// Content can't forge a number regardless: every number here is generated from
/// the window's own offset, never read out of the file.
fn number_lines(content: &str, first: usize) -> String {
    content
        .lines()
        .enumerate()
        .fold(String::new(), |mut acc, (index, line)| {
            let number = first.saturating_add(index);
            acc.push_str(&number.to_string());
            acc.push('\t');
            acc.push_str(line);
            acc.push('\n');
            acc
        })
}

fn format_logs(server: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        return prompts::LogsEmpty { server }.render();
    }
    let joined = crate::untrusted::scrub(&lines.join("\n"));
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
        DestroyOutcome::Blocked => prompts::DestroyBlocked { server }.render(),
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
        // Partial success (durable archive, old storage not freed). There is no
        // honest tool-result prompt for it yet: ArchiveDone slightly overstates it
        // (it claims the storage was released), but it preserves the load-bearing
        // truth — archived and recoverable — where ArchiveFailed would falsely tell
        // the model nothing was saved. A dedicated prompt is the real fix (see
        // "Open questions" in docs/design/02-backups-archive-restore.md); left
        // as-is here to avoid rewording prompt text.
        ArchiveOutcome::Archived { name, size_bytes }
        | ArchiveOutcome::ArchivedNotReleased { name, size_bytes } => {
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
            ..
        } => prompts::RestoreServerReady { server }.render(),
        RestoreOutcome::Restored {
            boot: BootState::TimedOut,
            ..
        } => prompts::RestoreServerTimedOut { server }.render(),
        RestoreOutcome::Restored {
            boot: BootState::Crashed,
            ..
        } => prompts::RestoreServerCrashed { server }.render(),
        RestoreOutcome::Restored {
            boot: BootState::Stopped,
            ..
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
        RecoverOutcome::GuildLimitReached(refusal) => prompts::RecoverGuildLimit {
            detail: &describe_limit_refusal(*refusal),
        }
        .render(),
        RecoverOutcome::ClusterFull(refusal) => prompts::RecoverClusterFull {
            name,
            detail: &describe_capacity_refusal(*refusal),
        }
        .render(),
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

fn modrinth_unavailable() -> String {
    prompts::ModrinthUnavailable::render()
}

fn thunderstore_unavailable() -> String {
    prompts::ThunderstoreUnavailable::render()
}

/// A `game` argument naming nothing in the catalog — a typo to correct, so the
/// result names the catalog.
fn mod_game_unknown(ctx: &ToolCtx<'_>, game: &str) -> String {
    let ids = game_ids(ctx);
    prompts::ModGameUnknown {
        game,
        games: if ids.is_empty() { "(none)" } else { &ids },
    }
    .render()
}

/// A real game that declares no mod registry. Final: no other game id answers
/// this question, so the result says to stop rather than to retry.
fn mods_not_supported(game: &str) -> String {
    prompts::ModsNotSupported { game }.render()
}

/// Render a `search_mods` listing: one bounded line per hit. `asked_for` is the
/// game version the search filtered on, if any, so each line keeps it visible.
fn format_mod_search(query: &str, asked_for: Option<&str>, hits: &[SearchHit]) -> String {
    if hits.is_empty() {
        return prompts::ModSearchEmpty { query }.render();
    }
    let results = hits
        .iter()
        .map(|hit| format_mod_line(hit, asked_for))
        .collect::<Vec<_>>()
        .join("\n");
    prompts::ModSearchHeader {
        query,
        results: &results,
    }
    .render()
}

/// One search hit as a single line. A hit reports no loaders of its own, so they
/// are recovered from its categories.
fn format_mod_line(hit: &SearchHit, asked_for: Option<&str>) -> String {
    let loaders = list_or_none(&hit.loaders());
    let versions = list_or_none(&versions_for_display(&hit.versions, asked_for));
    prompts::ModSummaryLine {
        slug: &hit.slug,
        title: &short_title(&hit.title),
        description: &short_description(&hit.description),
        downloads: &hit.downloads.to_string(),
        server_side: &hit.server_side,
        client_side: &hit.client_side,
        loaders: &loaders,
        versions: &versions,
    }
    .render()
}

/// Render a resolved `mod_info` lookup. Unlike a search hit, a project reports
/// its loaders and game versions as fields of its own.
fn format_mod_info(project: &Project) -> String {
    let loaders = list_or_none(&project.loaders);
    let versions = list_or_none(&versions_for_display(&project.game_versions, None));
    prompts::ModProjectDetail {
        slug: &project.slug,
        title: &short_title(&project.title),
        description: &short_description(&project.description),
        downloads: &project.downloads.to_string(),
        server_side: &project.server_side,
        client_side: &project.client_side,
        loaders: &loaders,
        versions: &versions,
    }
    .render()
}

/// Render a `search_mods` listing for a Thunderstore game: one bounded line per
/// hit. A hit carries no version, so the header sends the model to `mod_info`
/// for the string it would pin.
fn format_thunderstore_search(game: &str, query: &str, hits: &[Listing]) -> String {
    if hits.is_empty() {
        return prompts::ThunderstoreSearchEmpty { game, query }.render();
    }
    let results = hits
        .iter()
        .map(format_thunderstore_line)
        .collect::<Vec<_>>()
        .join("\n");
    prompts::ThunderstoreSearchHeader {
        game,
        query,
        results: &results,
    }
    .render()
}

/// One search hit as a single line.
fn format_thunderstore_line(hit: &Listing) -> String {
    prompts::ThunderstoreSummaryLine {
        package: &short_title(&hit.full_name()),
        description: &short_description(&hit.description),
        downloads: &hit.download_count.to_string(),
        side: hit.side_support().label(),
        updated: hit.updated_day(),
    }
    .render()
}

/// Render a resolved `mod_info` lookup for a Thunderstore package. The side
/// classification is read from the game's own community: a package listed in
/// several communities is categorised separately in each, and another game's
/// categories describe another game.
fn format_thunderstore_info(package: &Package, community: &str) -> String {
    let shown = package.shown_dependencies();
    let mut dependencies = list_or_none(&shown);
    if package.dependencies_truncated() {
        dependencies.push_str(", and more");
    }
    prompts::ThunderstorePackageDetail {
        package: &short_title(&package.full_name()),
        pinned: &package.pinned_name(),
        description: &short_description(&package.latest.description),
        downloads: &package.latest.downloads.to_string(),
        side: package.side_support(community).label(),
        status: if package.is_deprecated {
            "deprecated by its author — do not install it"
        } else {
            "active"
        },
        dependencies: &dependencies,
    }
    .render()
}

/// Delegates to [`super::game_catalog_list`] for the `&ToolCtx`-shaped call
/// sites so the comma-joined id list is computed in exactly one place.
fn game_ids(ctx: &ToolCtx<'_>) -> String {
    super::game_catalog_list(ctx.data)
}

#[cfg(test)]
#[path = "tests/tools.rs"]
mod tests;
