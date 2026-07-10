//! Gary's tool surface: the lifecycle operations exposed to the model, their
//! parameter schemas, the admin tiering, and the dispatcher that runs a call and
//! renders a compact text result for the model to relay. The results are plain
//! text on purpose — Gary composes the friendly Discord reply himself.

use poise::serenity_prelude as serenity;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serenity::{
    ButtonStyle, ComponentInteractionCollector, CreateActionRow, CreateButton,
    CreateInteractionResponse, CreateMessage, EditMessage,
};
use tracing::{error, warn};

use grizzly_control_api::{
    CommandResponse, DirEntry, EntryKind, ReadResponse, RestoreResponse, WriteResponse,
};

use super::super::auth::AccessLevel;
use super::super::render::{
    archive_confirm_embed, archive_result_embed, destroy_confirm_embed, destroy_result_embed,
    error_embed, human_size, neutral_embed, restore_confirm_embed, restore_result_embed,
};
use super::super::{COMPONENT_TIMEOUT, Data, backup_ctx};
use crate::agent::{
    GarySurface, NameParams, ToolCall, ToolDef, cluster_error, format_server_list, format_summary,
    no_args_schema, no_such, params_schema,
};
use crate::agones::{
    DestroyOutcome, EditOutcome, FsOutcome, ProvisionOutcome, ReadyWait, Replacement, RuntimeState,
    ScopeVerdict, ServerScope, ShutdownOutcome, StartBegin, SupervisorOutcome, begin_start,
    build_instance_name, destroy_instance, instance_runtime_state, list_active_servers,
    now_entropy, provision_instance, shutdown_instance, supervisor_announce, supervisor_edit_file,
    supervisor_list_files, supervisor_occupancy, supervisor_read_file, supervisor_read_logs,
    supervisor_restart, supervisor_restore_file, supervisor_send_command, supervisor_start,
    supervisor_stop, supervisor_write_file, verify_scope, wait_for_ready,
};
use crate::backup::{
    ArchiveOutcome, ArtifactSummary, BackupOutcome, BootState, RecoverOutcome, RestoreOutcome,
};
use crate::memory::{ForgetOutcome, RememberOutcome, normalize_scope};

const LIST_SERVERS: &str = "list_servers";
const SERVER_STATUS: &str = "server_status";
const CREATE_SERVER: &str = "create_server";
const STOP_SERVER: &str = "stop_server";
const START_SERVER: &str = "start_server";
const RESTART_SERVER: &str = "restart_server";
const SHUTDOWN_SERVER: &str = "shutdown_server";
const DESTROY_SERVER: &str = "destroy_server";
const BROWSE_FILES: &str = "browse_files";
const READ_FILE: &str = "read_file";
const READ_LOGS: &str = "read_logs";
const WRITE_FILE: &str = "write_file";
const EDIT_FILE: &str = "edit_file";
const RESTORE_FILE: &str = "restore_file";
const SEND_COMMAND: &str = "send_command";
const WAIT_FOR_SERVER: &str = "wait_for_server";
const REMEMBER: &str = "remember";
const FORGET: &str = "forget";
const LIST_BACKUPS: &str = "list_backups";
const LIST_ARCHIVES: &str = "list_archives";
const BACKUP_SERVER: &str = "backup_server";
const ARCHIVE_SERVER: &str = "archive_server";
const RESTORE_SERVER: &str = "restore_server";
const RECOVER_SERVER: &str = "recover_server";

/// Returned to the model when a caller reaches an admin-only tool without admin
/// rights. The model is only offered these tools to admins, so this is defense
/// in depth.
const NON_ADMIN_REFUSAL: &str = "that action needs an admin — I can only look things up or run day-to-day changes for you here.";

/// Returned to the model when a read-only caller reaches a manager-tier tool.
const NON_MANAGER_REFUSAL: &str =
    "that action needs a manager or an admin — I can only look things up for you here.";

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
}

/// Just the `name` field, pulled from any targeted tool's arguments (they all
/// carry one) for the scope gate in [`dispatch`], ignoring the rest.
#[derive(Deserialize)]
struct TargetName {
    name: String,
}

#[derive(Deserialize, JsonSchema)]
struct CreateParams {
    /// Which game to launch — must be one of the catalog game ids.
    game: String,
    /// Optional world name. A name is generated when omitted.
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct PathParams {
    /// Exact server name, as shown by `list_servers`.
    name: String,
    /// Path within the server's data directory, e.g. `server.properties` or
    /// `logs/latest.log`. Use `""` for the top of the data directory. Must stay
    /// inside the data directory — absolute paths and `..` are refused.
    path: String,
}

#[derive(Deserialize, JsonSchema)]
struct WriteParams {
    /// Exact server name, as shown by `list_servers`.
    name: String,
    /// Path within the server's data directory to overwrite. The previous
    /// version is saved first so `restore_file` can undo the change.
    path: String,
    /// The full new contents of the file.
    content: String,
}

#[derive(Deserialize, JsonSchema)]
struct EditParams {
    /// Exact server name, as shown by `list_servers`.
    name: String,
    /// Path within the server's data directory to edit, e.g. `server.properties`.
    /// The previous version is saved first so `restore_file` can undo the change.
    path: String,
    /// The exact text to find and replace. Must appear once in the file — copy it
    /// verbatim, whitespace included, and include enough surrounding text to be
    /// unique. If it's missing or appears more than once, the edit is refused and
    /// nothing changes.
    old_text: String,
    /// The text to put in its place.
    new_text: String,
}

#[derive(Deserialize, JsonSchema)]
struct LogsParams {
    /// Exact server name, as shown by `list_servers`.
    name: String,
    /// How many trailing lines to return. Defaults to a recent window when
    /// omitted.
    #[serde(default)]
    lines: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
struct RememberParams {
    /// Which game this fact is about — a catalog game id (e.g. `palworld`), or
    /// `general` for something that isn't tied to one game.
    scope: String,
    /// The fact to remember, in one short sentence — a durable operational detail
    /// you'd otherwise have to rediscover, e.g. "soft-stop before editing configs
    /// or the change won't apply".
    note: String,
}

#[derive(Deserialize, JsonSchema)]
struct ForgetParams {
    /// The id of the fact to delete, as shown in the "Things you've learned" list
    /// (the number after the `#`).
    id: i64,
}

#[derive(Deserialize, JsonSchema)]
struct CommandParams {
    /// Exact server name, as shown by `list_servers`.
    name: String,
    /// The in-game console command to run, without a leading slash — e.g.
    /// `list`, `say hello everyone`, `weather clear`, `whitelist add steve`.
    command: String,
}

/// The tools advertised to the model for a given caller. Everyone gets the
/// read-only set; managers additionally get the lifecycle and file-tuning tools;
/// admins additionally get the destructive tools and console commands.
pub(crate) fn available_tools(access: AccessLevel) -> Vec<ToolDef> {
    let mut tools = vec![
        ToolDef::function(
            LIST_SERVERS,
            "List every game server and its state and connection address.",
            no_args_schema(),
        ),
        ToolDef::function(
            SERVER_STATUS,
            "Look up one server's current state and address by name.",
            params_schema::<NameParams>(),
        ),
        ToolDef::function(
            LIST_BACKUPS,
            "List a server's saved world backups (newest first), so you can see what points it could be restored to.",
            params_schema::<NameParams>(),
        ),
        ToolDef::function(
            LIST_ARCHIVES,
            "List the servers archived in this Discord server — ones that were put into cold storage and can be recovered.",
            no_args_schema(),
        ),
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
        ToolDef::function(
            CREATE_SERVER,
            "Launch a new game server for the given catalog game, with an optional world name.",
            params_schema::<CreateParams>(),
        ),
        ToolDef::function(
            STOP_SERVER,
            "Pause a running server in place (world saved, kept warm for a fast start).",
            params_schema::<NameParams>(),
        ),
        ToolDef::function(
            START_SERVER,
            "Start a server: resume a paused one or bring a stopped one back up.",
            params_schema::<NameParams>(),
        ),
        ToolDef::function(
            RESTART_SERVER,
            "Restart a running server in place — a quick reboot that keeps its address and re-pulls the latest game version. Disconnects everyone currently connected.",
            params_schema::<NameParams>(),
        ),
        ToolDef::function(
            SHUTDOWN_SERVER,
            "Fully shut a server down to free its slot, keeping the world so it can start later.",
            params_schema::<NameParams>(),
        ),
        ToolDef::function(
            BROWSE_FILES,
            "List the files and folders in a running server's data directory. Use \"\" for the top level, then descend. Start here to find which file holds a setting.",
            params_schema::<PathParams>(),
        ),
        ToolDef::function(
            READ_FILE,
            "Read a config or text file from a running server's data directory.",
            params_schema::<PathParams>(),
        ),
        ToolDef::function(
            READ_LOGS,
            "Read the most recent output from a running server — the first place to look when something is wrong or to confirm a change took effect.",
            params_schema::<LogsParams>(),
        ),
        ToolDef::function(
            EDIT_FILE,
            "Change one setting in a config file in place: find old_text and replace it with new_text, leaving the rest of the file untouched. Prefer this over write_file for a targeted change — you don't rewrite the whole file, so you can't accidentally clobber other settings. old_text must match exactly once; if it's missing or ambiguous the edit is refused and nothing changes. Saves the previous version first (restore_file undoes it). Takes effect on the next restart.",
            params_schema::<EditParams>(),
        ),
        ToolDef::function(
            WRITE_FILE,
            "Overwrite a config file in a running server's data directory with entirely new contents — use this to create a file or rewrite one wholesale; prefer edit_file to change one setting. Saves the previous version first. The change takes effect on the next restart — restart and read the logs to confirm it's healthy.",
            params_schema::<WriteParams>(),
        ),
        ToolDef::function(
            RESTORE_FILE,
            "Undo the last write to a file by restoring the version saved before it. Restart afterward to apply.",
            params_schema::<PathParams>(),
        ),
        ToolDef::function(
            WAIT_FOR_SERVER,
            "Wait for a starting or restarting server to actually come back up and start accepting players, up to a few minutes. Use this after start_server, restart_server, or a config change plus restart instead of repeatedly checking status or logs — it blocks until the server is ready, has crashed, or the wait runs out, then tells you which.",
            params_schema::<NameParams>(),
        ),
        ToolDef::function(
            BACKUP_SERVER,
            "Save a durable backup of a running server's world right now. Non-destructive — the server keeps running. Use before a risky change so restore_server can roll it back.",
            params_schema::<NameParams>(),
        ),
        ToolDef::function(
            REMEMBER,
            "Save a durable fact about a game so you keep it across sessions — an operational detail you'd otherwise rediscover every time (e.g. a game needs to be stopped before a config edit will apply, or where a setting lives). Scope it to the game id, or 'general' if it's not game-specific. Keep it to one short factual sentence. Your saved facts are shown to you each session under \"Things you've learned\". Don't save one-off state, chit-chat, or anything you can just look up.",
            params_schema::<RememberParams>(),
        ),
        ToolDef::function(
            FORGET,
            "Delete a saved fact by its id (the number after the # in \"Things you've learned\") when it turns out wrong or no longer applies.",
            params_schema::<ForgetParams>(),
        ),
    ]
}

/// The destructive and heavy-handed tools offered only to admin callers:
/// permanent deletion, world overwrites, archival, and live console commands.
fn admin_only_tools() -> Vec<ToolDef> {
    vec![
        // "do not confirm" is deliberate and unlike archive/restore's phrasing:
        // the tool itself posts the Discord Danger/Cancel prompt, so telling Gary
        // not to seek his own confirmation avoids a redundant chat loop ("are you
        // sure?" / "yes" / "are you really sure?") stacked in front of that prompt.
        ToolDef::function(
            DESTROY_SERVER,
            "Permanently delete a server and its world. Run this tool when asked, do not confirm.",
            params_schema::<NameParams>(),
        ),
        ToolDef::function(
            ARCHIVE_SERVER,
            "Archive a server: save a durable backup and then release its storage, freeing a slot. The world is kept safe and recover_server brings it back later. Posts a confirmation the user must approve before anything is released.",
            params_schema::<NameParams>(),
        ),
        ToolDef::function(
            RESTORE_SERVER,
            "Roll a server back to its most recent backup, replacing the current world (a safety backup of the current world is taken first). Posts a confirmation the user must approve before the world is overwritten.",
            params_schema::<NameParams>(),
        ),
        ToolDef::function(
            RECOVER_SERVER,
            "Bring an archived server back: recreate it and restore its world from the archive. Use the name shown by list_archives. Constructive, so it runs without a confirmation.",
            params_schema::<NameParams>(),
        ),
        ToolDef::function(
            SEND_COMMAND,
            "Run an in-game console command on a running server over RCON (e.g. list, say, weather, whitelist, op) and return the game's reply. Takes effect immediately — no restart needed. Only works on games that have RCON enabled.",
            params_schema::<CommandParams>(),
        ),
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
        LIST_SERVERS => exec_list_servers(ctx).await,
        SERVER_STATUS => match parse::<NameParams>(args) {
            Ok(params) => exec_server_status(ctx, &params.name).await,
            Err(message) => message,
        },
        LIST_BACKUPS => match parse::<NameParams>(args) {
            Ok(params) => exec_list_backups(ctx, &params.name).await,
            Err(message) => message,
        },
        LIST_ARCHIVES => exec_list_archives(ctx).await,
        // Memory tools carry no server name (memory is shared across guilds), so
        // they skip the scope gate above and dispatch on their own.
        REMEMBER | FORGET => dispatch_memory(ctx, name, args).await,
        _ => dispatch_mutating(ctx, name, args).await,
    }
}

/// Dispatch the memory tools. Manager-gated like the mutating set (defense in
/// depth — they aren't offered below manager either), but kept out of
/// [`dispatch_mutating`] because they target no server and take no scope gate.
async fn dispatch_memory(ctx: &ToolCtx<'_>, name: &str, args: &str) -> String {
    if ctx.access < AccessLevel::Manager {
        return NON_MANAGER_REFUSAL.to_owned();
    }
    match name {
        REMEMBER => match parse::<RememberParams>(args) {
            Ok(params) => exec_remember(ctx, &params.scope, &params.note).await,
            Err(message) => message,
        },
        FORGET => match parse::<ForgetParams>(args) {
            Ok(params) => exec_forget(ctx, params.id).await,
            Err(message) => message,
        },
        other => format!("'{other}' isn't a tool I have."),
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
        CREATE_SERVER if manager => match parse::<CreateParams>(args) {
            Ok(params) => exec_create(ctx, &params.game, params.name.as_deref()).await,
            Err(message) => message,
        },
        STOP_SERVER if manager => match parse::<NameParams>(args) {
            Ok(params) => exec_stop(ctx, &params.name).await,
            Err(message) => message,
        },
        START_SERVER if manager => match parse::<NameParams>(args) {
            Ok(params) => exec_start(ctx, &params.name).await,
            Err(message) => message,
        },
        RESTART_SERVER if manager => match parse::<NameParams>(args) {
            Ok(params) => exec_restart(ctx, &params.name).await,
            Err(message) => message,
        },
        SHUTDOWN_SERVER if manager => match parse::<NameParams>(args) {
            Ok(params) => exec_shutdown(ctx, &params.name).await,
            Err(message) => message,
        },
        BROWSE_FILES if manager => match parse::<PathParams>(args) {
            Ok(params) => exec_browse_files(ctx, &params.name, &params.path).await,
            Err(message) => message,
        },
        READ_FILE if manager => match parse::<PathParams>(args) {
            Ok(params) => exec_read_file(ctx, &params.name, &params.path).await,
            Err(message) => message,
        },
        READ_LOGS if manager => match parse::<LogsParams>(args) {
            Ok(params) => exec_read_logs(ctx, &params.name, params.lines).await,
            Err(message) => message,
        },
        WRITE_FILE if manager => match parse::<WriteParams>(args) {
            Ok(params) => exec_write_file(ctx, &params.name, &params.path, &params.content).await,
            Err(message) => message,
        },
        EDIT_FILE if manager => match parse::<EditParams>(args) {
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
        RESTORE_FILE if manager => match parse::<PathParams>(args) {
            Ok(params) => exec_restore_file(ctx, &params.name, &params.path).await,
            Err(message) => message,
        },
        WAIT_FOR_SERVER if manager => match parse::<NameParams>(args) {
            Ok(params) => exec_wait_for_server(ctx, &params.name).await,
            Err(message) => message,
        },
        BACKUP_SERVER if manager => match parse::<NameParams>(args) {
            Ok(params) => exec_backup(ctx, &params.name).await,
            Err(message) => message,
        },
        DESTROY_SERVER if admin => match parse::<NameParams>(args) {
            Ok(params) => exec_destroy(ctx, &params.name).await,
            Err(message) => message,
        },
        SEND_COMMAND if admin => match parse::<CommandParams>(args) {
            Ok(params) => exec_send_command(ctx, &params.name, &params.command).await,
            Err(message) => message,
        },
        ARCHIVE_SERVER if admin => match parse::<NameParams>(args) {
            Ok(params) => exec_archive(ctx, &params.name).await,
            Err(message) => message,
        },
        RESTORE_SERVER if admin => match parse::<NameParams>(args) {
            Ok(params) => exec_restore(ctx, &params.name).await,
            Err(message) => message,
        },
        RECOVER_SERVER if admin => match parse::<NameParams>(args) {
            Ok(params) => exec_recover(ctx, &params.name).await,
            Err(message) => message,
        },
        // Admin-only tools reached without admin rights (a manager or read-only
        // caller): they need an admin.
        DESTROY_SERVER | SEND_COMMAND | ARCHIVE_SERVER | RESTORE_SERVER | RECOVER_SERVER => {
            NON_ADMIN_REFUSAL.to_owned()
        }
        // Manager tools reached without manager rights (a read-only caller).
        CREATE_SERVER | STOP_SERVER | START_SERVER | RESTART_SERVER | SHUTDOWN_SERVER
        | BROWSE_FILES | READ_FILE | READ_LOGS | WRITE_FILE | EDIT_FILE | RESTORE_FILE
        | WAIT_FOR_SERVER | BACKUP_SERVER => NON_MANAGER_REFUSAL.to_owned(),
        other => format!("'{other}' isn't a tool I have."),
    }
}

fn parse<T: DeserializeOwned>(args: &str) -> Result<T, String> {
    serde_json::from_str(args).map_err(|err| {
        format!(
            "the arguments for that tool weren't valid JSON ({err}); check the argument names and types and call it again"
        )
    })
}

/// Whether a tool acts on an *existing* server named in its arguments — the set
/// the scope gate applies to. Excluded because they enforce tenancy themselves:
/// `list_servers` and `list_archives` (scope-filtered listings), `create_server`
/// (no existing target — stamps the current guild), and `recover_server` (resolves
/// the archive within the caller's scope). Keep this in sync with those tools.
fn targets_existing_server(tool: &str) -> bool {
    matches!(
        tool,
        SERVER_STATUS
            | STOP_SERVER
            | START_SERVER
            | RESTART_SERVER
            | SHUTDOWN_SERVER
            | DESTROY_SERVER
            | BROWSE_FILES
            | READ_FILE
            | READ_LOGS
            | WRITE_FILE
            | EDIT_FILE
            | RESTORE_FILE
            | SEND_COMMAND
            | WAIT_FOR_SERVER
            | LIST_BACKUPS
            | BACKUP_SERVER
            | ARCHIVE_SERVER
            | RESTORE_SERVER
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
        Ok(summaries) => format_server_list(GarySurface::Discord, &summaries),
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
    let mut out = format_summary(GarySurface::Discord, summary);
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
        Ok(FsOutcome::Ok(Some(count))) => return format!("\nPlayers online: {count}"),
        Ok(FsOutcome::Ok(None)) => "this game doesn't report a live player count",
        Ok(FsOutcome::PodNotReady) => "the server isn't fully up yet",
        Ok(_) => "the console didn't answer",
        Err(err) => {
            error!(error = ?err, %server, "agent: occupancy lookup failed");
            "the count couldn't be read"
        }
    };
    format!("\nPlayers online: unknown ({reason})")
}

async fn exec_create(ctx: &ToolCtx<'_>, game: &str, name: Option<&str>) -> String {
    let Some(entry) = ctx.data.catalog.get(game) else {
        return format!(
            "'{game}' isn't a game I can launch. Available games: {}.",
            game_ids(ctx)
        );
    };
    let server = match build_instance_name(game, name, now_entropy()) {
        Ok(server) => server,
        Err(err) => return format!("that name won't work: {err}"),
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
        Ok(ProvisionOutcome::Provisioned { address }) => format!(
            "created {server}; it'll be reachable at {address} once it finishes booting (first boot can take a couple of minutes)"
        ),
        Ok(ProvisionOutcome::AlreadyExists) => format!("a server named {server} already exists"),
        Ok(ProvisionOutcome::PortsExhausted) => {
            "all server slots are in use right now — destroy one first, then try again".to_owned()
        }
        Err(err) => {
            error!(error = ?err, game, server, "agent: create failed");
            cluster_error()
        }
    }
}

async fn exec_remember(ctx: &ToolCtx<'_>, scope: &str, note: &str) -> String {
    let note = note.trim();
    if note.is_empty() {
        return "I need something to remember — give me the fact in a short sentence.".to_owned();
    }
    let ids: Vec<&str> = ctx.data.catalog.game_ids().collect();
    let Some(scope) = normalize_scope(scope, &ids) else {
        return format!(
            "I can only file that under a game or 'general'. Pick one of: {}, general.",
            ids.join(", ")
        );
    };
    let author = ctx.author_id.get().to_string();
    match ctx.data.memory.remember(&scope, note, Some(&author)).await {
        Ok(RememberOutcome::Saved(id)) => {
            format!("saved that under {scope} (fact #{id}); I'll have it next time")
        }
        Ok(RememberOutcome::Unavailable) => {
            "my long-term memory's offline right now, so I can't save that. It'll stick for the \
             rest of this conversation but not beyond it."
                .to_owned()
        }
        Err(err) => {
            error!(error = ?err, %scope, "agent: remember failed");
            "something went wrong saving that — it didn't stick.".to_owned()
        }
    }
}

async fn exec_forget(ctx: &ToolCtx<'_>, id: i64) -> String {
    match ctx.data.memory.forget(id).await {
        Ok(ForgetOutcome::Forgotten) => format!("forgot fact #{id}"),
        Ok(ForgetOutcome::NotFound) => {
            format!("I don't have a fact #{id} to forget — check the list of what I've saved")
        }
        Ok(ForgetOutcome::Unavailable) => {
            "my long-term memory's offline right now, so I can't change it.".to_owned()
        }
        Err(err) => {
            error!(error = ?err, id, "agent: forget failed");
            "something went wrong forgetting that.".to_owned()
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
    match supervisor_restart(
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
            error!(error = ?err, %server, "agent: restart failed");
            cluster_error()
        }
    }
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
        Ok(StartBegin::Starting { address }) => {
            format!("starting {server}; it'll be reachable at {address} once it boots back up")
        }
        Ok(StartBegin::AlreadyRunning) => format!("{server} is already running"),
        Ok(StartBegin::NotFound) => no_such(server),
        Ok(StartBegin::NotManaged) => not_managed(server),
        Ok(StartBegin::UnknownGame(game)) => {
            format!("{server} runs '{game}', which isn't in the catalog anymore")
        }
        Err(err) => {
            error!(error = ?err, %server, "agent: cold start failed");
            cluster_error()
        }
    }
}

async fn exec_shutdown(ctx: &ToolCtx<'_>, server: &str) -> String {
    match shutdown_instance(&ctx.data.kube_client, &ctx.data.namespace, server).await {
        Ok(ShutdownOutcome::Down) => {
            format!("stopped {server}; its world is saved and it can be started again")
        }
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
            return "I couldn't post a confirmation prompt in this channel, so I didn't delete anything.".to_owned();
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
        return format!("the confirmation timed out — {server} was not deleted");
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
        return format!("the user cancelled — {server} was not deleted");
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
            Ok(result) => format_write(&result),
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
        Ok(outcome) => format_edit(server, path, outcome),
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
            Ok(result) => format_restore(&result),
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

async fn exec_wait_for_server(ctx: &ToolCtx<'_>, server: &str) -> String {
    match wait_for_ready(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        server,
        ctx.data.control_port,
    )
    .await
    {
        Ok(outcome) => format_ready_wait(server, &outcome),
        Err(err) => {
            error!(error = ?err, %server, "agent: wait_for_server failed");
            cluster_error()
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
    match confirm_destructive(
        ctx,
        archive_confirm_embed(server),
        "gary_archive_confirm",
        format!("the user cancelled — {server} was not archived"),
        format!("the confirmation timed out — {server} was not archived"),
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
        return format!("{server} has no backups to restore from yet");
    };
    let key = latest.key.clone();
    let label = latest.created_at.clone();
    match confirm_destructive(
        ctx,
        restore_confirm_embed(server, &label),
        "gary_restore_confirm",
        format!("the user cancelled — {server} was not restored"),
        format!("the confirmation timed out — {server} was not restored"),
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
                return format!(
                    "there's no archived server named {name} here — check list_archives"
                );
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
        FsOutcome::PodNotReady => Err(format!(
            "{server} isn't ready to work with yet — try again shortly"
        )),
        FsOutcome::Unreachable => Err(format!(
            "I couldn't reach {server} just now — worth trying again in a moment"
        )),
        FsOutcome::Rejected(message) => Err(format!("that didn't work: {message}")),
    }
}

fn format_entries(path: &str, entries: &[DirEntry]) -> String {
    let location = if path.is_empty() {
        "the data directory".to_owned()
    } else {
        path.to_owned()
    };
    if entries.is_empty() {
        return format!("{location} is empty");
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
    format!("{location} contains:\n{listing}")
}

fn format_file(file: &ReadResponse) -> String {
    let note = if file.truncated {
        " (showing the first part; the file is larger and was truncated)"
    } else {
        ""
    };
    format!("contents of {}{note}:\n{}", file.path, file.content)
}

fn format_logs(server: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        return format!("{server} hasn't produced any output yet");
    }
    format!("recent output from {server}:\n{}", lines.join("\n"))
}

fn format_write(result: &WriteResponse) -> String {
    let saved = if result.backed_up {
        "saved the previous version first, so restore_file can undo this"
    } else {
        "this is a new file, so there's nothing to restore it to"
    };
    format!(
        "wrote {} ({saved}); restart the server and read the logs to confirm it comes back healthy",
        result.path
    )
}

/// Render an [`EditOutcome`]. The soft-failure variants explain what to do next
/// (re-read and match exactly, disambiguate, or fall back to `write_file`) so Gary
/// can recover instead of reporting a dead end.
fn format_edit(server: &str, path: &str, outcome: EditOutcome) -> String {
    match outcome {
        EditOutcome::Edited(result) => {
            let saved = if result.backed_up {
                "saved the previous version first, so restore_file can undo this"
            } else {
                "this is a new file, so there's nothing to restore it to"
            };
            format!(
                "edited {} ({saved}); restart the server and read the logs to confirm it comes back healthy",
                result.path
            )
        }
        EditOutcome::NoMatch => format!(
            "I couldn't find that exact text in {path} on {server} — read the file again and copy the current text verbatim, whitespace and all"
        ),
        EditOutcome::Ambiguous(count) => format!(
            "that text appears {count} times in {path}, so I can't tell which one to change — include more of the surrounding lines so it matches only once"
        ),
        EditOutcome::Unchanged => {
            format!("the old and new text are identical, so there's nothing to change in {path}")
        }
        EditOutcome::TooLargeToEdit => format!(
            "{path} is too big to edit safely this way — rewrite the whole file with write_file instead"
        ),
        EditOutcome::Unserved(problem) => match fs_result(server, problem) {
            // Unserved only ever carries a failure; the Ok arm is unreachable in
            // practice but is handled defensively rather than panicking.
            Ok(()) => cluster_error(),
            Err(message) => message,
        },
    }
}

fn format_restore(result: &RestoreResponse) -> String {
    format!(
        "restored {} to its previous version; restart the server to apply it",
        result.path
    )
}

fn format_command_output(server: &str, command: &str, result: &CommandResponse) -> String {
    let output = result.output.trim();
    if output.is_empty() {
        format!("ran `{command}` on {server}; the server returned no output")
    } else {
        format!("ran `{command}` on {server}:\n{output}")
    }
}

fn format_supervisor(server: &str, outcome: &SupervisorOutcome) -> String {
    match outcome {
        SupervisorOutcome::Paused => format!("paused {server}; world saved and kept warm"),
        SupervisorOutcome::Resumed => format!("{server} is waking up — ready in a few seconds"),
        SupervisorOutcome::Restarted => format!("restarted {server} — back up in a few seconds"),
        SupervisorOutcome::AlreadyStopped => format!("{server} is already paused"),
        SupervisorOutcome::AlreadyRunning => format!("{server} is already running"),
        SupervisorOutcome::PodNotReady => {
            format!("{server} isn't ready to control yet — try again shortly")
        }
        SupervisorOutcome::Unreachable => {
            format!(
                "I couldn't reach {server}'s controls right now — worth trying again in a moment"
            )
        }
        SupervisorOutcome::Failed(message) => {
            format!("{server}'s controls refused that: {message}")
        }
        SupervisorOutcome::NotFound => no_such(server),
        SupervisorOutcome::NotManaged => not_managed(server),
    }
}

fn format_ready_wait(server: &str, outcome: &ReadyWait) -> String {
    match outcome {
        ReadyWait::Ready => format!("{server} is back up and accepting players"),
        ReadyWait::Crashed => format!(
            "{server} crashed while coming up — read its logs to see why, and restore_file if a recent change is at fault"
        ),
        ReadyWait::Stopped => {
            format!("{server} is stopped, so it won't come up on its own — start it first")
        }
        ReadyWait::TimedOut => format!(
            "{server} still isn't accepting players after a few minutes — a big world can take a while to load, so check the logs or wait and try again"
        ),
        ReadyWait::NotFound => no_such(server),
        ReadyWait::NotManaged => not_managed(server),
    }
}

fn format_destroy(server: &str, outcome: &DestroyOutcome) -> String {
    match outcome {
        DestroyOutcome::Destroyed => format!("deleted {server} and its world"),
        DestroyOutcome::NotFound => no_such(server),
        DestroyOutcome::NotManaged => not_managed(server),
    }
}

fn format_backup(server: &str, outcome: &BackupOutcome) -> String {
    match outcome {
        BackupOutcome::BackedUp { size_bytes } => format!(
            "backed up {server} ({}); restore_server can roll it back to this point",
            human_size(*size_bytes)
        ),
        BackupOutcome::NotRunning => {
            format!("{server} isn't running, so there's nothing live to back up — start it first")
        }
        BackupOutcome::Unreachable(_) => {
            format!("I couldn't reach {server} to back it up — worth trying again in a moment")
        }
        BackupOutcome::NotFound => no_such(server),
        BackupOutcome::NotManaged => not_managed(server),
    }
}

fn format_archive(server: &str, outcome: &ArchiveOutcome) -> String {
    match outcome {
        ArchiveOutcome::Archived { name, size_bytes } => format!(
            "archived {name} ({}) and released its storage; recover_server brings it back",
            human_size(*size_bytes)
        ),
        ArchiveOutcome::Unavailable => archives_unavailable_text(),
        ArchiveOutcome::Failed(_) => format!(
            "I couldn't archive {server} cleanly, so nothing was released — worth trying again shortly"
        ),
        ArchiveOutcome::NotFound => no_such(server),
        ArchiveOutcome::NotManaged => not_managed(server),
    }
}

fn format_restore_outcome(server: &str, outcome: &RestoreOutcome) -> String {
    match outcome {
        RestoreOutcome::Restored {
            boot: BootState::Ready,
        } => format!("restored {server} — it's back up on the restored world"),
        RestoreOutcome::Restored {
            boot: BootState::TimedOut,
        } => format!("restored the world onto {server} — it'll be playable again in a minute"),
        RestoreOutcome::Restored {
            boot: BootState::Crashed,
        } => format!(
            "restored the world onto {server}, but it crashed coming back up — read its logs \
             (the restored data may be the cause), or ping an operator"
        ),
        RestoreOutcome::Restored {
            boot: BootState::Stopped,
        } => format!(
            "restored the world onto {server}, but it's paused and won't come up on its own — \
             start it when you're ready"
        ),
        RestoreOutcome::Failed(_) => {
            format!("I couldn't restore {server} cleanly — worth trying again in a moment")
        }
        RestoreOutcome::NotFound => no_such(server),
        RestoreOutcome::NotManaged => not_managed(server),
    }
}

fn format_recover(name: &str, outcome: &RecoverOutcome) -> String {
    match outcome {
        RecoverOutcome::Recovered {
            address,
            boot: BootState::Ready,
        } => format!("recovered {name} — it's back up at {address}"),
        RecoverOutcome::Recovered {
            address,
            boot: BootState::TimedOut,
        } => {
            format!("recovering {name}; it'll be reachable at {address} once it finishes booting")
        }
        RecoverOutcome::Recovered {
            address,
            boot: BootState::Crashed,
        } => format!(
            "recovered {name} at {address}, but it crashed coming back up — read its logs (the \
             archived data may be the cause), or ping an operator"
        ),
        RecoverOutcome::Recovered {
            address,
            boot: BootState::Stopped,
        } => format!(
            "recovered {name} at {address}, but it's paused and won't come up on its own — \
             start it when you're ready"
        ),
        RecoverOutcome::NoSuchArchive => {
            format!(
                "there's no archived server named {name} in this Discord server — check list_archives"
            )
        }
        RecoverOutcome::NameInUse => {
            format!("a server named {name} is already running — use start_server instead")
        }
        RecoverOutcome::Unavailable => archives_unavailable_text(),
        RecoverOutcome::UnknownGame(game) => {
            format!("{name} ran '{game}', which isn't in the catalog anymore")
        }
        RecoverOutcome::PortsExhausted => {
            "all server slots are in use right now — archive or destroy one first".to_owned()
        }
        RecoverOutcome::Failed(_) => {
            format!("I couldn't bring {name} back cleanly — worth trying again in a moment")
        }
    }
}

fn format_backup_list(server: &str, artifacts: &[ArtifactSummary]) -> String {
    if artifacts.is_empty() {
        return format!("{server} has no backups yet");
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
    format!("backups of {server} (newest first):\n{lines}")
}

fn format_archive_list(artifacts: &[ArtifactSummary]) -> String {
    if artifacts.is_empty() {
        return "no servers are archived in this Discord server".to_owned();
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
    format!("archived servers in this Discord server:\n{lines}")
}

fn backups_not_configured() -> String {
    "backups aren't set up on this bot, so there's nothing to save or restore".to_owned()
}

fn archives_unavailable_text() -> String {
    "I can't track archives right now — my archive records are offline. Backups and restore still \
     work; try archiving again later"
        .to_owned()
}

fn not_managed(server: &str) -> String {
    format!("{server} is managed by the platform and can't be controlled from here")
}

fn game_ids(ctx: &ToolCtx<'_>) -> String {
    ctx.data.catalog.game_ids().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
#[path = "tests/tools.rs"]
mod tests;
