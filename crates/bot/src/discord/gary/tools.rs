//! Gary's tool surface: the lifecycle operations exposed to the model, their
//! parameter schemas, the admin tiering, and the dispatcher that runs a call and
//! renders a compact text result for the model to relay. The results are plain
//! text on purpose — Gary composes the friendly Discord reply himself.

use poise::serenity_prelude as serenity;
use schemars::{JsonSchema, SchemaGenerator};
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

use super::super::render::{destroy_confirm_embed, destroy_result_embed, neutral_embed};
use super::super::{COMPONENT_TIMEOUT, Data};
use crate::agent::{ToolCall, ToolDef};
use crate::agones::{
    DestroyOutcome, EditOutcome, FsOutcome, ProvisionOutcome, ReadyWait, Replacement, RuntimeState,
    ScopeVerdict, ServerScope, ServerSummary, ShutdownOutcome, StartBegin, SupervisorOutcome,
    begin_start, build_instance_name, destroy_instance, instance_runtime_state,
    list_active_servers, now_entropy, provision_instance, shutdown_instance, supervisor_announce,
    supervisor_edit_file, supervisor_list_files, supervisor_read_file, supervisor_read_logs,
    supervisor_restart, supervisor_restore_file, supervisor_send_command, supervisor_start,
    supervisor_stop, supervisor_write_file, verify_scope, wait_for_ready,
};

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

/// Returned to the model when a non-admin caller reaches a mutating tool. The
/// model is only offered mutating tools for admins, so this is defense in depth.
const NON_ADMIN_REFUSAL: &str =
    "that action needs an admin — I can only look things up for you here.";

/// Everything a tool executor needs: the shared bot state plus the Discord
/// handles the destructive-confirmation flow uses, and whether the caller is an
/// admin (so mutating tools can refuse at execution time as defense in depth).
pub(crate) struct ToolCtx<'a> {
    pub(crate) data: &'a Data,
    pub(crate) serenity: &'a serenity::Context,
    pub(crate) channel_id: serenity::ChannelId,
    pub(crate) author_id: serenity::UserId,
    pub(crate) is_admin: bool,
    /// The servers this caller may see and act on — every tool that targets an
    /// existing server by name is gated on it in [`dispatch`], and the listing
    /// tools query within it.
    pub(crate) scope: ServerScope,
}

#[derive(Deserialize, JsonSchema)]
struct NameParams {
    /// Exact server name, as shown by `list_servers`.
    name: String,
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
struct CommandParams {
    /// Exact server name, as shown by `list_servers`.
    name: String,
    /// The in-game console command to run, without a leading slash — e.g.
    /// `list`, `say hello everyone`, `weather clear`, `whitelist add steve`.
    command: String,
}

/// The tools advertised to the model for a given caller. Everyone gets the
/// read-only pair; admins additionally get the mutating set.
pub(crate) fn available_tools(is_admin: bool) -> Vec<ToolDef> {
    let mut tools = vec![
        ToolDef::function(
            LIST_SERVERS,
            "List every game server and its state and connection address.",
            empty_object_schema(),
        ),
        ToolDef::function(
            SERVER_STATUS,
            "Look up one server's current state and address by name.",
            params_schema::<NameParams>(),
        ),
    ];
    if is_admin {
        tools.extend([
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
                "Restart a running server in place — a quick reboot that keeps its address.",
                params_schema::<NameParams>(),
            ),
            ToolDef::function(
                SHUTDOWN_SERVER,
                "Fully shut a server down to free its slot, keeping the world so it can start later.",
                params_schema::<NameParams>(),
            ),
            ToolDef::function(
                DESTROY_SERVER,
                "Permanently delete a server and its world. Run this tool when asked, do not confirm.",
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
                SEND_COMMAND,
                "Run an in-game console command on a running server over RCON (e.g. list, say, weather, whitelist, op) and return the game's reply. Takes effect immediately — no restart needed. Only works on games that have RCON enabled.",
                params_schema::<CommandParams>(),
            ),
            ToolDef::function(
                WAIT_FOR_SERVER,
                "Wait for a starting or restarting server to actually come back up and start accepting players, up to a few minutes. Use this after start_server, restart_server, or a config change plus restart instead of repeatedly checking status or logs — it blocks until the server is ready, has crashed, or the wait runs out, then tells you which.",
                params_schema::<NameParams>(),
            ),
        ]);
    }
    tools
}

/// Run one tool call and return the text result to feed back to the model. Bad
/// arguments, unknown names, and non-admin attempts at mutating tools all return
/// an explanatory string rather than failing the loop.
///
/// Before dispatching, any tool that targets an existing server by name is
/// confined to the caller's [`ServerScope`](ToolCtx::scope): a server in another
/// channel reads as "no such server", so Gary can neither see nor touch another
/// group's servers. `list_servers` scopes itself in its query; `create_server`
/// makes a new server and stamps the current channel, so neither is gated here.
pub(crate) async fn dispatch(ctx: &ToolCtx<'_>, call: &ToolCall) -> String {
    let args = call.function.arguments.as_str();
    if targets_existing_server(call.function.name.as_str())
        && let Ok(TargetName { name }) = serde_json::from_str::<TargetName>(args)
        && let Some(refusal) = scope_refusal(ctx, &name).await
    {
        return refusal;
    }
    match call.function.name.as_str() {
        LIST_SERVERS => exec_list_servers(ctx).await,
        SERVER_STATUS => match parse::<NameParams>(args) {
            Ok(params) => exec_server_status(ctx, &params.name).await,
            Err(message) => message,
        },
        CREATE_SERVER if ctx.is_admin => match parse::<CreateParams>(args) {
            Ok(params) => exec_create(ctx, &params.game, params.name.as_deref()).await,
            Err(message) => message,
        },
        STOP_SERVER if ctx.is_admin => match parse::<NameParams>(args) {
            Ok(params) => exec_stop(ctx, &params.name).await,
            Err(message) => message,
        },
        START_SERVER if ctx.is_admin => match parse::<NameParams>(args) {
            Ok(params) => exec_start(ctx, &params.name).await,
            Err(message) => message,
        },
        RESTART_SERVER if ctx.is_admin => match parse::<NameParams>(args) {
            Ok(params) => exec_restart(ctx, &params.name).await,
            Err(message) => message,
        },
        SHUTDOWN_SERVER if ctx.is_admin => match parse::<NameParams>(args) {
            Ok(params) => exec_shutdown(ctx, &params.name).await,
            Err(message) => message,
        },
        DESTROY_SERVER if ctx.is_admin => match parse::<NameParams>(args) {
            Ok(params) => exec_destroy(ctx, &params.name).await,
            Err(message) => message,
        },
        BROWSE_FILES if ctx.is_admin => match parse::<PathParams>(args) {
            Ok(params) => exec_browse_files(ctx, &params.name, &params.path).await,
            Err(message) => message,
        },
        READ_FILE if ctx.is_admin => match parse::<PathParams>(args) {
            Ok(params) => exec_read_file(ctx, &params.name, &params.path).await,
            Err(message) => message,
        },
        READ_LOGS if ctx.is_admin => match parse::<LogsParams>(args) {
            Ok(params) => exec_read_logs(ctx, &params.name, params.lines).await,
            Err(message) => message,
        },
        WRITE_FILE if ctx.is_admin => match parse::<WriteParams>(args) {
            Ok(params) => exec_write_file(ctx, &params.name, &params.path, &params.content).await,
            Err(message) => message,
        },
        EDIT_FILE if ctx.is_admin => match parse::<EditParams>(args) {
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
        RESTORE_FILE if ctx.is_admin => match parse::<PathParams>(args) {
            Ok(params) => exec_restore_file(ctx, &params.name, &params.path).await,
            Err(message) => message,
        },
        SEND_COMMAND if ctx.is_admin => match parse::<CommandParams>(args) {
            Ok(params) => exec_send_command(ctx, &params.name, &params.command).await,
            Err(message) => message,
        },
        WAIT_FOR_SERVER if ctx.is_admin => match parse::<NameParams>(args) {
            Ok(params) => exec_wait_for_server(ctx, &params.name).await,
            Err(message) => message,
        },
        CREATE_SERVER | STOP_SERVER | START_SERVER | RESTART_SERVER | SHUTDOWN_SERVER
        | DESTROY_SERVER | BROWSE_FILES | READ_FILE | READ_LOGS | WRITE_FILE | EDIT_FILE
        | RESTORE_FILE | SEND_COMMAND | WAIT_FOR_SERVER => NON_ADMIN_REFUSAL.to_owned(),
        other => format!("'{other}' isn't a tool I have."),
    }
}

fn parse<T: DeserializeOwned>(args: &str) -> Result<T, String> {
    serde_json::from_str(args)
        .map_err(|err| format!("I couldn't read the arguments for that tool: {err}"))
}

/// Whether a tool acts on an *existing* server named in its arguments — the set
/// the scope gate applies to. `list_servers` (no target) and `create_server`
/// (makes a new one) are the only tools that don't, so they're excluded.
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

/// The parameter schema for a tool that takes no arguments.
fn empty_object_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object", "properties": {} })
}

/// JSON Schema for a tool's parameters, trimmed of the metadata keys some
/// providers reject (`$schema`, `title`).
fn params_schema<T: JsonSchema>() -> serde_json::Value {
    let mut value = SchemaGenerator::default()
        .into_root_schema_for::<T>()
        .to_value();
    if let Some(object) = value.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    value
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
    match list_active_servers(
        ctx.data.kube_client.clone(),
        &ctx.data.namespace,
        &ctx.data.domain,
        &ctx.scope,
    )
    .await
    {
        Ok(summaries) => summaries
            .iter()
            .find(|summary| summary.name == server)
            .map_or_else(|| no_such(server), format_summary),
        Err(err) => {
            error!(error = ?err, "agent: server_status failed");
            cluster_error()
        }
    }
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
        &ctx.channel_id.get().to_string(),
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
            cluster_error()
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

fn format_server_list(servers: &[ServerSummary]) -> String {
    if servers.is_empty() {
        return "no game servers exist right now".to_owned();
    }
    servers
        .iter()
        .map(format_summary)
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_summary(server: &ServerSummary) -> String {
    let game = server.game.as_deref().unwrap_or("unknown game");
    let address = server.address.as_deref().unwrap_or("no address yet");
    format!(
        "{} (game: {game}, state: {}, address: {address})",
        server.name, server.state
    )
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

fn no_such(server: &str) -> String {
    format!("there's no server named {server} — check list_servers for the current names")
}

fn not_managed(server: &str) -> String {
    format!("{server} is managed by the platform and can't be controlled from here")
}

fn cluster_error() -> String {
    "I couldn't reach the cluster just now — worth trying again in a moment".to_owned()
}

fn game_ids(ctx: &ToolCtx<'_>) -> String {
    ctx.data.catalog.game_ids().collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
#[path = "tests/tools.rs"]
mod tests;
