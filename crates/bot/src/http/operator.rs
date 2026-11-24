use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::agent::commit_turn;
use crate::agent::{
    ChatMessage, EscalationReason, Generation, Role, SessionKey, SessionOutcome, TokenUsage,
    ToolCall, ToolDef, TurnEnding, TurnOrigin,
};
use crate::agones::ServerScope;
use crate::discord::gary::tools::{ConfirmMode, ToolCtx, available_tools, dispatch, known_tool};
use crate::discord::gary::{
    GaryTurn, build_system_prompt, framed_turn, game_catalog_list, replace_system_prompt,
    run_gary_turn,
};
use crate::discord::{AccessLevel, Data};
use crate::domain::{GuildId, InstanceName};
use crate::events::{Actor, Attribution, EventKind, ProductEvent, Surface, mapping};
use crate::http::gated;

const OPERATOR_CALL_ID: &str = "operator";

const OPERATOR_SPEAKER: &str = "the operator";

#[derive(Clone)]
pub(crate) struct OperatorDeps {
    data: Data,
}

impl OperatorDeps {
    pub(crate) fn from_data(data: &Data) -> Self {
        Self { data: data.clone() }
    }
}

pub(crate) fn router(deps: OperatorDeps, token: String) -> Router {
    gated(
        Router::new()
            .route("/operator/tool", post(run_tool))
            .route("/operator/gary", post(run_gary))
            .route("/operator/tools", get(list_tools))
            .with_state(deps),
        Some(token),
    )
}

#[derive(Deserialize)]
struct Caller {
    /// Discord channel id the reply and any confirmation card is posted to.
    /// A string because a snowflake exceeds the range JSON numbers survive
    /// intact in every client.
    channel: String,
    #[serde(default)]
    guild: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    access: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    confirm: bool,
}

#[derive(Deserialize)]
struct ToolRequest {
    tool: String,
    #[serde(default)]
    arguments: serde_json::Value,
    #[serde(flatten)]
    caller: Caller,
}

#[derive(Deserialize)]
struct GaryRequest {
    prompt: String,
    #[serde(default)]
    new_session: bool,
    #[serde(flatten)]
    caller: Caller,
}

#[derive(Deserialize)]
struct ToolsQuery {
    #[serde(default)]
    access: Option<String>,
}

#[derive(Serialize)]
struct ToolResponse {
    tool: String,
    result: String,
}

#[derive(Serialize)]
struct GaryResponse {
    reply: String,
    escalated: bool,
    escalation_kind: Option<&'static str>,
    escalation_reason: Option<String>,
    rounds: usize,
    generations: Vec<GenerationTrace>,
    usage: UsageTotals,
    latency_ms: u64,
    tool_calls: Vec<ToolTrace>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct GenerationTrace {
    model: String,
    prompt_tokens: Option<u32>,
    cached_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    total_tokens: Option<u32>,
    finish_reason: Option<String>,
    latency_ms: u64,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
struct UsageTotals {
    #[serde(rename = "prompt_tokens")]
    prompt: Option<u32>,
    #[serde(rename = "cached_tokens")]
    cached: Option<u32>,
    #[serde(rename = "completion_tokens")]
    completion: Option<u32>,
    #[serde(rename = "total_tokens")]
    total: Option<u32>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct ToolTrace {
    name: String,
    arguments: String,
    result: String,
}

#[derive(Debug)]
enum OperatorError {
    BadRequest(String),
    Unavailable(String),
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for OperatorError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            Self::BadRequest(error) => (StatusCode::BAD_REQUEST, error),
            Self::Unavailable(error) => (StatusCode::SERVICE_UNAVAILABLE, error),
        };
        (status, axum::Json(ErrorBody { error })).into_response()
    }
}

struct Acting {
    channel_id: serenity::ChannelId,
    guild: Option<u64>,
    author_id: serenity::UserId,
    access: AccessLevel,
    scope: ServerScope,
    confirmations: ConfirmMode,
}

async fn run_tool(
    State(deps): State<OperatorDeps>,
    axum::Json(request): axum::Json<ToolRequest>,
) -> Result<axum::Json<ToolResponse>, OperatorError> {
    let data = &deps.data;
    let serenity_ctx = live_gateway(data)?;
    if !known_tool(&request.tool) {
        return Err(OperatorError::BadRequest(format!(
            "unknown tool {:?} — GET /operator/tools lists the valid names",
            request.tool
        )));
    }
    let acting = resolve(data, &serenity_ctx, &request.caller).await?;
    let call = ToolCall {
        id: OPERATOR_CALL_ID.to_owned(),
        kind: "function".to_owned(),
        function: crate::agent::llm::FunctionCall {
            name: request.tool.clone(),
            arguments: request.arguments.to_string(),
        },
    };

    let tool_ctx = ToolCtx {
        data,
        serenity: &serenity_ctx,
        origin: TurnOrigin::Operator,
        channel_id: acting.channel_id,
        guild: acting.guild,
        author_id: acting.author_id,
        access: acting.access,
        scope: acting.scope,
        confirmations: acting.confirmations,
        pending_change: std::sync::Mutex::new(None),
        guardrail: std::sync::Mutex::new(None),
        escalation: std::sync::Mutex::new(None),
    };
    info!(
        tool = %request.tool,
        access = ?acting.access,
        "operator harness: running a tool directly"
    );
    let result = dispatch(&tool_ctx, &call).await;
    Ok(axum::Json(ToolResponse {
        tool: request.tool,
        result,
    }))
}

async fn run_gary(
    State(deps): State<OperatorDeps>,
    axum::Json(request): axum::Json<GaryRequest>,
) -> Result<axum::Json<GaryResponse>, OperatorError> {
    let data = &deps.data;
    if data.ollama.is_none() {
        return Err(OperatorError::Unavailable(
            "Gary has no model configured (OLLAMA_API_KEY unset), so there is no turn to run. \
             POST /operator/tool still works — it needs no model."
                .to_owned(),
        ));
    }
    if request.prompt.trim().is_empty() {
        return Err(OperatorError::BadRequest(
            "prompt is empty; send what you'd type to Gary".to_owned(),
        ));
    }
    let serenity_ctx = live_gateway(data)?;
    let acting = resolve(data, &serenity_ctx, &request.caller).await?;

    let key = SessionKey::Channel(acting.channel_id.get());
    let _turn_lock = data.sessions.turn_lock(key).await;
    if request.new_session {
        data.sessions.clear(key);
    }
    let games = game_catalog_list(data);
    let memories = data.memory.render_for_prompt().await;
    let system = build_system_prompt(acting.access, &games, &memories);
    let session = data
        .sessions
        .checkout(key, std::time::Instant::now(), || system.clone());
    let mut messages = session.messages;
    replace_system_prompt(&mut messages, system);
    messages.push(ChatMessage::user(framed_turn(
        OPERATOR_SPEAKER,
        acting.access,
        &request.prompt,
    )));
    let mark = messages.len();

    let turn = GaryTurn {
        ctx: &serenity_ctx,
        data,
        origin: TurnOrigin::Operator,
        session: Some(session.id.clone()),
        channel_id: acting.channel_id,
        guild: acting.guild,
        author_id: acting.author_id,
        access: acting.access,
        scope: acting.scope,
        confirmations: acting.confirmations,
    };
    info!(access = ?acting.access, "operator harness: running a gary turn");
    let outcome = run_gary_turn(&turn, &mut messages).await.map_err(|err| {
        error!(error = ?err, "operator harness: gary turn failed");
        OperatorError::Unavailable(format!(
            "the model call failed, so Gary couldn't answer: {err:#}"
        ))
    })?;

    let (actor, guild) = (acting.author_id, acting.guild);
    let committed = outcome.clone();
    let SessionOutcome {
        reply,
        ending,
        generations,
    } = outcome;
    let escalation = match &ending {
        TurnEnding::Escalated(reason) => Some(reason),
        TurnEnding::Answered | TurnEnding::Cancelled => None,
    };
    let escalation_kind = escalation.map(EscalationReason::as_str);
    let escalation_reason = match escalation {
        Some(EscalationReason::AgentRequested { reason, .. }) => Some(reason.clone()),
        Some(EscalationReason::RoundBudget) | None => None,
    };
    if let Some(reason) = escalation {
        record_harness_escalation(data, actor, guild, reason).await;
    }
    let turn_slice = messages.get(mark..).unwrap_or_default();
    let tool_calls = tool_trace(turn_slice);
    let rounds = rounds(turn_slice);
    let limits = data.settings.snapshot().transcript_limits();
    commit_turn(
        &data.sessions,
        key,
        session.id,
        messages,
        &committed,
        limits,
    );

    Ok(axum::Json(GaryResponse {
        reply,
        escalated: escalation.is_some(),
        escalation_kind,
        escalation_reason,
        rounds,
        usage: usage_totals(&generations),
        latency_ms: generations.iter().map(|g| millis(g.latency)).sum(),
        generations: generations.iter().map(generation_trace).collect(),
        tool_calls,
    }))
}

async fn record_harness_escalation(
    data: &Data,
    actor: serenity::UserId,
    guild: Option<u64>,
    reason: &EscalationReason,
) {
    let named = match reason {
        EscalationReason::AgentRequested { server, .. } => server.as_deref(),
        EscalationReason::RoundBudget => None,
    };
    let mut event = ProductEvent::new(
        EventKind::Escalation,
        &Attribution::new(Actor::Discord(actor.get()), Surface::Operator),
        reason.as_str(),
    )
    .guild(guild.map(|guild| GuildId::new(guild.to_string())))
    .details(mapping::escalation_details(named, None));
    if let Some(server) = named {
        event = event.instance(InstanceName::new(server));
    }
    data.events.record(event).await;
}

async fn list_tools(
    Query(query): Query<ToolsQuery>,
) -> Result<axum::Json<Vec<ToolDef>>, OperatorError> {
    let access = parse_access(query.access.as_deref())?;
    Ok(axum::Json(available_tools(access)))
}

fn live_gateway(data: &Data) -> Result<serenity::Context, OperatorError> {
    data.gateway.current().ok_or_else(|| {
        warn!("operator harness: request arrived before the discord gateway connected");
        OperatorError::Unavailable(
            "the discord gateway isn't connected yet — the bot is still starting up. Retry in a \
             few seconds."
                .to_owned(),
        )
    })
}

async fn resolve(
    data: &Data,
    serenity_ctx: &serenity::Context,
    caller: &Caller,
) -> Result<Acting, OperatorError> {
    let channel = parse_id(&caller.channel, "channel")?;
    let access = parse_access(caller.access.as_deref())?;
    let confirmations = parse_confirm(caller.confirm, access)?;
    let channel_id = serenity::ChannelId::new(channel);

    let guild = match caller.guild.as_deref() {
        Some(raw) => Some(parse_id(raw, "guild")?),
        None => guild_of_channel(serenity_ctx, channel_id).await,
    };
    let author_id = match caller.user.as_deref() {
        Some(raw) => parse_id(raw, "user")?,
        None => *data.operator_ids.first().ok_or_else(|| {
            OperatorError::BadRequest(
                "no user was given and there is no configured operator to default to; set \
                 GAMESERVERS_ADMIN_USER_IDS or pass \"user\""
                    .to_owned(),
            )
        })?,
    };

    let scope = parse_scope(caller.scope.as_deref(), guild)?;
    Ok(Acting {
        channel_id,
        guild,
        author_id: serenity::UserId::new(author_id),
        access,
        scope,
        confirmations,
    })
}

async fn guild_of_channel(
    serenity_ctx: &serenity::Context,
    channel_id: serenity::ChannelId,
) -> Option<u64> {
    match channel_id.to_channel(serenity_ctx).await {
        Ok(channel) => channel.guild().map(|guild| guild.guild_id.get()),
        Err(err) => {
            warn!(
                error = ?err,
                channel = channel_id.get(),
                "operator harness: couldn't read the channel to resolve its guild"
            );
            None
        }
    }
}

fn parse_id(raw: &str, field: &str) -> Result<u64, OperatorError> {
    raw.trim()
        .parse::<u64>()
        .ok()
        .filter(|&id| id != 0)
        .ok_or_else(|| {
            OperatorError::BadRequest(format!("{field} must be a discord id, got {raw:?}"))
        })
}

fn parse_access(raw: Option<&str>) -> Result<AccessLevel, OperatorError> {
    match raw.map(str::trim) {
        None | Some("admin") => Ok(AccessLevel::Admin),
        Some("manager") => Ok(AccessLevel::Manager),
        Some("read-only") => Ok(AccessLevel::ReadOnly),
        Some(other) => Err(OperatorError::BadRequest(format!(
            "access must be one of admin, manager, read-only; got {other:?}"
        ))),
    }
}

fn parse_scope(raw: Option<&str>, guild: Option<u64>) -> Result<ServerScope, OperatorError> {
    match raw.map(str::trim) {
        Some("all") => Ok(ServerScope::All),
        None | Some("guild") => guild
            .map(|id| ServerScope::Guild(id.to_string()))
            .ok_or_else(|| {
                OperatorError::BadRequest(
                "no guild to scope to: the channel's guild couldn't be read, so pass \"guild\" \
                 explicitly or use \"scope\": \"all\""
                    .to_owned(),
            )
            }),
        Some(other) => Err(OperatorError::BadRequest(format!(
            "scope must be guild or all; got {other:?}"
        ))),
    }
}

fn parse_confirm(confirm: bool, access: AccessLevel) -> Result<ConfirmMode, OperatorError> {
    if !confirm {
        return Ok(ConfirmMode::Interactive);
    }
    if access < AccessLevel::Admin {
        return Err(OperatorError::BadRequest(
            "confirm is admin-only: the destructive tools aren't offered below admin, so \
             pre-approving them at a lower tier can't do anything"
                .to_owned(),
        ));
    }
    Ok(ConfirmMode::Preapproved)
}

fn tool_trace(slice: &[ChatMessage]) -> Vec<ToolTrace> {
    slice
        .iter()
        .filter_map(ChatMessage::requested_tool_calls)
        .flatten()
        .map(|call| ToolTrace {
            name: call.function.name.clone(),
            arguments: call.function.arguments.clone(),
            result: result_for(slice, &call.id).unwrap_or_default(),
        })
        .collect()
}

fn result_for(slice: &[ChatMessage], call_id: &str) -> Option<String> {
    slice
        .iter()
        .find(|message| {
            message.role == Role::Tool && message.tool_call_id.as_deref() == Some(call_id)
        })
        .and_then(|message| message.content.clone())
}

fn generation_trace(generation: &Generation) -> GenerationTrace {
    let usage = generation.usage.as_ref();
    GenerationTrace {
        model: generation.model.clone(),
        prompt_tokens: usage.and_then(|usage| usage.prompt),
        cached_tokens: usage.and_then(TokenUsage::cached_prompt),
        completion_tokens: usage.and_then(|usage| usage.completion),
        total_tokens: usage.and_then(|usage| usage.total),
        finish_reason: generation.finish_reason.clone(),
        latency_ms: millis(generation.latency),
    }
}

fn usage_totals(generations: &[Generation]) -> UsageTotals {
    let mut totals = UsageTotals::default();
    for usage in generations
        .iter()
        .filter_map(|generation| generation.usage.as_ref())
    {
        accumulate(&mut totals.prompt, usage.prompt);
        accumulate(&mut totals.cached, usage.cached_prompt());
        accumulate(&mut totals.completion, usage.completion);
        accumulate(&mut totals.total, usage.total);
    }
    totals
}

fn accumulate(total: &mut Option<u32>, value: Option<u32>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or(0).saturating_add(value));
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn rounds(slice: &[ChatMessage]) -> usize {
    slice
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .count()
}

#[cfg(test)]
#[path = "tests/operator.rs"]
mod tests;
