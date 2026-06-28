//! Gary's tool surface: the lifecycle operations exposed to the model, their
//! parameter schemas, the admin tiering, and the dispatcher that runs a call and
//! renders a compact text result for the model to relay. The results are plain
//! text on purpose — Gary composes the friendly Discord reply himself.

use std::time::Duration;

use poise::serenity_prelude as serenity;
use schemars::{JsonSchema, SchemaGenerator};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serenity::{
    ButtonStyle, ComponentInteractionCollector, CreateActionRow, CreateButton,
    CreateInteractionResponse, CreateMessage, EditMessage,
};
use tracing::{error, warn};

use super::super::Data;
use super::super::render::{neutral_embed, remove_confirm_embed, remove_result_embed};
use crate::agent::{ToolCall, ToolDef};
use crate::agones::{
    KillOutcome, ProvisionOutcome, RemoveOutcome, RuntimeState, ServerSummary, StartBegin,
    SupervisorOutcome, begin_start, build_instance_name, instance_runtime_state, kill_instance,
    list_active_servers, now_entropy, provision_instance, remove_instance, supervisor_restart,
    supervisor_start, supervisor_stop,
};

const LIST_SERVERS: &str = "list_servers";
const SERVER_STATUS: &str = "server_status";
const CREATE_SERVER: &str = "create_server";
const STOP_SERVER: &str = "stop_server";
const START_SERVER: &str = "start_server";
const RESTART_SERVER: &str = "restart_server";
const KILL_SERVER: &str = "kill_server";
const REMOVE_SERVER: &str = "remove_server";

/// Returned to the model when a non-admin caller reaches a mutating tool. The
/// model is only offered mutating tools for admins, so this is defense in depth.
const NON_ADMIN_REFUSAL: &str =
    "that action needs an admin — I can only look things up for you here.";

/// How long the remove-confirmation buttons stay live before the deletion is
/// abandoned — matched to the slash command's `/remove` timeout.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(120);

/// Everything a tool executor needs: the shared bot state plus the Discord
/// handles the destructive-confirmation flow uses, and whether the caller is an
/// admin (so mutating tools can refuse at execution time as defense in depth).
pub(crate) struct ToolCtx<'a> {
    pub(crate) data: &'a Data,
    pub(crate) serenity: &'a serenity::Context,
    pub(crate) channel_id: serenity::ChannelId,
    pub(crate) author_id: serenity::UserId,
    pub(crate) is_admin: bool,
}

#[derive(Deserialize, JsonSchema)]
struct NameParams {
    /// Exact server name, as shown by `list_servers`.
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
                KILL_SERVER,
                "Fully shut a server down to free its slot, keeping the world so it can start later.",
                params_schema::<NameParams>(),
            ),
            ToolDef::function(
                REMOVE_SERVER,
                "Permanently delete a server and its world. Run this tool when asked, do not confirm.",
                params_schema::<NameParams>(),
            ),
        ]);
    }
    tools
}

/// Run one tool call and return the text result to feed back to the model. Bad
/// arguments, unknown names, and non-admin attempts at mutating tools all return
/// an explanatory string rather than failing the loop.
pub(crate) async fn dispatch(ctx: &ToolCtx<'_>, call: &ToolCall) -> String {
    let args = call.function.arguments.as_str();
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
        KILL_SERVER if ctx.is_admin => match parse::<NameParams>(args) {
            Ok(params) => exec_kill(ctx, &params.name).await,
            Err(message) => message,
        },
        REMOVE_SERVER if ctx.is_admin => match parse::<NameParams>(args) {
            Ok(params) => exec_remove(ctx, &params.name).await,
            Err(message) => message,
        },
        CREATE_SERVER | STOP_SERVER | START_SERVER | RESTART_SERVER | KILL_SERVER
        | REMOVE_SERVER => NON_ADMIN_REFUSAL.to_owned(),
        other => format!("'{other}' isn't a tool I have."),
    }
}

fn parse<T: DeserializeOwned>(args: &str) -> Result<T, String> {
    serde_json::from_str(args)
        .map_err(|err| format!("I couldn't read the arguments for that tool: {err}"))
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

async fn exec_server_status(ctx: &ToolCtx<'_>, name: &str) -> String {
    match list_active_servers(
        ctx.data.kube_client.clone(),
        &ctx.data.namespace,
        &ctx.data.domain,
    )
    .await
    {
        Ok(summaries) => summaries
            .iter()
            .find(|summary| summary.name == name)
            .map_or_else(|| no_such(name), format_summary),
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
    let instance = match build_instance_name(game, name, now_entropy()) {
        Ok(instance) => instance,
        Err(err) => return format!("that name won't work: {err}"),
    };

    match provision_instance(
        &ctx.data.kube_client,
        &ctx.data.namespace,
        &ctx.data.domain,
        &ctx.data.provision_lock,
        entry,
        &instance,
    )
    .await
    {
        // Don't block the loop on first-boot world generation (minutes). Report
        // the address now; the user can ask for status to see when it's ready.
        Ok(ProvisionOutcome::Provisioned { address }) => format!(
            "created {instance}; it'll be reachable at {address} once it finishes booting (first boot can take a couple of minutes)"
        ),
        Ok(ProvisionOutcome::AlreadyExists) => format!("a server named {instance} already exists"),
        Ok(ProvisionOutcome::PortsExhausted) => {
            "all server slots are in use right now — remove one first, then try again".to_owned()
        }
        Err(err) => {
            error!(error = ?err, game, instance, "agent: create failed");
            cluster_error()
        }
    }
}

async fn exec_stop(ctx: &ToolCtx<'_>, name: &str) -> String {
    match supervisor_stop(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        name,
        ctx.data.control_port,
    )
    .await
    {
        Ok(outcome) => format_supervisor(name, &outcome),
        Err(err) => {
            error!(error = ?err, server = %name, "agent: stop failed");
            cluster_error()
        }
    }
}

async fn exec_restart(ctx: &ToolCtx<'_>, name: &str) -> String {
    match supervisor_restart(
        &ctx.data.kube_client,
        &ctx.data.http,
        &ctx.data.namespace,
        name,
        ctx.data.control_port,
    )
    .await
    {
        Ok(outcome) => format_supervisor(name, &outcome),
        Err(err) => {
            error!(error = ?err, server = %name, "agent: restart failed");
            cluster_error()
        }
    }
}

/// Mirrors the `/start` slash command's warm/cold routing: a live pod resumes in
/// place via the supervisor; a killed instance is rescheduled. Unlike the slash
/// command, the agent doesn't block waiting for readiness — it reports the
/// address and lets the user poll status.
async fn exec_start(ctx: &ToolCtx<'_>, name: &str) -> String {
    match instance_runtime_state(&ctx.data.kube_client, &ctx.data.namespace, name).await {
        Ok(RuntimeState::PodUp) => match supervisor_start(
            &ctx.data.kube_client,
            &ctx.data.http,
            &ctx.data.namespace,
            name,
            ctx.data.control_port,
        )
        .await
        {
            Ok(outcome) => format_supervisor(name, &outcome),
            Err(err) => {
                error!(error = ?err, server = %name, "agent: warm start failed");
                cluster_error()
            }
        },
        Ok(RuntimeState::Killed) => exec_cold_start(ctx, name).await,
        Ok(RuntimeState::Absent) => no_such(name),
        Err(err) => {
            error!(error = ?err, server = %name, "agent: start state lookup failed");
            cluster_error()
        }
    }
}

async fn exec_cold_start(ctx: &ToolCtx<'_>, name: &str) -> String {
    match begin_start(
        &ctx.data.kube_client,
        &ctx.data.namespace,
        &ctx.data.domain,
        &ctx.data.catalog,
        name,
    )
    .await
    {
        Ok(StartBegin::Starting { address }) => {
            format!("starting {name}; it'll be reachable at {address} once it boots back up")
        }
        Ok(StartBegin::AlreadyRunning) => format!("{name} is already running"),
        Ok(StartBegin::NotFound) => no_such(name),
        Ok(StartBegin::NotManaged) => not_managed(name),
        Ok(StartBegin::UnknownGame(game)) => {
            format!("{name} runs '{game}', which isn't in the catalog anymore")
        }
        Err(err) => {
            error!(error = ?err, server = %name, "agent: cold start failed");
            cluster_error()
        }
    }
}

async fn exec_kill(ctx: &ToolCtx<'_>, name: &str) -> String {
    match kill_instance(&ctx.data.kube_client, &ctx.data.namespace, name).await {
        Ok(KillOutcome::Killed) => {
            format!("stopped {name}; its world is saved and it can be started again")
        }
        Ok(KillOutcome::NotFound) => no_such(name),
        Ok(KillOutcome::NotManaged) => not_managed(name),
        Err(err) => {
            error!(error = ?err, server = %name, "agent: kill failed");
            cluster_error()
        }
    }
}

/// Permanent deletion is gated behind an explicit Discord confirmation: the
/// model can request it, but a human must click through before any world is
/// destroyed. The returned text tells the model what the human decided.
async fn exec_remove(ctx: &ToolCtx<'_>, name: &str) -> String {
    let buttons = CreateActionRow::Buttons(vec![
        CreateButton::new("gary_remove_confirm")
            .label("Delete it")
            .style(ButtonStyle::Danger),
        CreateButton::new("gary_remove_cancel")
            .label("Cancel")
            .style(ButtonStyle::Secondary),
    ]);
    let prompt = match ctx
        .channel_id
        .send_message(
            ctx.serenity,
            CreateMessage::new()
                .embed(remove_confirm_embed(name))
                .components(vec![buttons]),
        )
        .await
    {
        Ok(message) => message,
        Err(err) => {
            error!(error = ?err, server = %name, "agent: failed to post remove confirmation");
            return "I couldn't post a confirmation prompt in this channel, so I didn't delete anything.".to_owned();
        }
    };

    let decision = ComponentInteractionCollector::new(ctx.serenity)
        .author_id(ctx.author_id)
        .message_id(prompt.id)
        .timeout(CONFIRM_TIMEOUT)
        .await;

    finish_remove(ctx, name, prompt, decision).await
}

async fn finish_remove(
    ctx: &ToolCtx<'_>,
    name: &str,
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
        return format!("the confirmation timed out — {name} was not deleted");
    };

    if let Err(err) = interaction
        .create_response(ctx.serenity, CreateInteractionResponse::Acknowledge)
        .await
    {
        warn!(error = ?err, "agent: failed to acknowledge remove interaction");
    }

    if interaction.data.custom_id != "gary_remove_confirm" {
        edit_prompt(
            ctx,
            &mut prompt,
            neutral_embed("Cancelled", "Nothing was deleted."),
        )
        .await;
        return format!("the user cancelled — {name} was not deleted");
    }

    match remove_instance(&ctx.data.kube_client, &ctx.data.namespace, name).await {
        Ok(outcome) => {
            edit_prompt(ctx, &mut prompt, remove_result_embed(&outcome, name)).await;
            format_remove(name, &outcome)
        }
        Err(err) => {
            error!(error = ?err, server = %name, "agent: remove failed");
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
        warn!(error = ?err, "agent: failed to clear remove confirmation prompt");
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

fn format_supervisor(name: &str, outcome: &SupervisorOutcome) -> String {
    match outcome {
        SupervisorOutcome::Paused => format!("paused {name}; world saved and kept warm"),
        SupervisorOutcome::Resumed => format!("{name} is waking up — ready in a few seconds"),
        SupervisorOutcome::Restarted => format!("restarted {name} — back up in a few seconds"),
        SupervisorOutcome::AlreadyStopped => format!("{name} is already paused"),
        SupervisorOutcome::AlreadyRunning => format!("{name} is already running"),
        SupervisorOutcome::PodNotReady => {
            format!("{name} isn't ready to control yet — try again shortly")
        }
        SupervisorOutcome::Unreachable => format!("I couldn't reach {name}'s controls right now"),
        SupervisorOutcome::NotFound => no_such(name),
        SupervisorOutcome::NotManaged => not_managed(name),
    }
}

fn format_remove(name: &str, outcome: &RemoveOutcome) -> String {
    match outcome {
        RemoveOutcome::Removed => format!("deleted {name} and its world"),
        RemoveOutcome::NotFound => no_such(name),
        RemoveOutcome::NotManaged => not_managed(name),
    }
}

fn no_such(name: &str) -> String {
    format!("there's no server named {name}")
}

fn not_managed(name: &str) -> String {
    format!("{name} is managed by the platform and can't be controlled from here")
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
