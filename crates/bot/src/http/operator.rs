//! The operator harness: an authenticated surface for driving the deployed bot
//! from a dev box, so a change can be exercised end-to-end without a human
//! typing into Discord.
//!
//! Three routes, all bearer-gated and reachable only over `kubectl port-forward`
//! (the Service is `ClusterIP`, never a `NodePort`):
//!
//! - `POST /operator/tool` runs one of Gary's tools directly. No model, so the
//!   result is deterministic and costs nothing — this is the ordinary path.
//! - `POST /operator/gary` runs a full model turn and returns the reply along
//!   with a trace of every tool the model called and what each returned, plus
//!   what the turn cost in tokens and latency.
//! - `GET /operator/tools` lists the tools a tier may call, with their schemas.
//!
//! Both POST routes take the access tier as a parameter. That is deliberate: it
//! makes "what does a read-only friend see?" a question answerable without
//! standing up a second Discord account.
//!
//! Replies still go to the named Discord channel exactly as a real turn would,
//! so the render path is genuinely exercised and every action the harness takes
//! leaves a visible trail where a human can read it.

use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::agent::{ChatMessage, Generation, Role, SessionOutcome, ToolCall, ToolDef, TurnOrigin};
use crate::agones::ServerScope;
use crate::discord::gary::tools::{ConfirmMode, ToolCtx, available_tools, dispatch, known_tool};
use crate::discord::gary::{GaryTurn, build_system_prompt, game_catalog_list, run_gary_turn};
use crate::discord::{AccessLevel, Data};
use crate::http::gated;

/// Synthetic id for a harness-issued tool call. The real dispatcher only echoes
/// it back into a transcript we don't keep, so it needs to be stable, not unique.
const OPERATOR_CALL_ID: &str = "operator";

/// Cloneable state the operator routes share with the rest of the bot. `Data`
/// already carries the gateway handle, so this is just the command state.
#[derive(Clone)]
pub(crate) struct OperatorDeps {
    data: Data,
}

impl OperatorDeps {
    pub(crate) fn from_data(data: &Data) -> Self {
        Self { data: data.clone() }
    }
}

/// The operator routes, gated on their own bearer token. The token is
/// non-optional by type: these routes reach Gary's full admin surface, so
/// "unconfigured" must mean unmounted rather than open, and that is enforced
/// here rather than trusted to a caller passing the right `Option`.
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

/// Fields every operator request carries: who the harness is acting as, where
/// the output lands, and what it is allowed to do.
#[derive(Deserialize)]
struct Caller {
    /// Discord channel id the reply and any confirmation card is posted to.
    /// A string because a snowflake exceeds the range JSON numbers survive
    /// intact in every client.
    channel: String,
    /// Guild to scope reads and actions to. Resolved from `channel` when absent,
    /// so a caller configures one id rather than two that can disagree.
    #[serde(default)]
    guild: Option<String>,
    /// Discord user the action is attributed to. Defaults to the first
    /// cross-guild operator, which is who would otherwise be doing this by hand.
    #[serde(default)]
    user: Option<String>,
    /// Access tier to act at: `read-only`, `manager`, or `admin` (the default).
    #[serde(default)]
    access: Option<String>,
    /// `guild` (the default) or `all` for the cross-guild operator view.
    #[serde(default)]
    scope: Option<String>,
    /// Pre-approve destructive confirmations. Admin-only, and every use posts a
    /// visible audit line into `channel`.
    #[serde(default)]
    confirm: bool,
}

#[derive(Deserialize)]
struct ToolRequest {
    /// Tool name, as listed by `GET /operator/tools`.
    tool: String,
    /// The tool's arguments as a JSON object. Omitted means no arguments.
    #[serde(default)]
    arguments: serde_json::Value,
    #[serde(flatten)]
    caller: Caller,
}

#[derive(Deserialize)]
struct GaryRequest {
    /// What to say to Gary, exactly as a friend would type it.
    prompt: String,
    /// Start a fresh conversation instead of continuing this channel/user's.
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
    /// The dispatcher's text, byte-identical to what the model would have read.
    result: String,
}

#[derive(Serialize)]
struct GaryResponse {
    reply: String,
    /// Whether Gary gave up and escalated rather than finishing the request.
    escalated: bool,
    /// Model turns this request took.
    rounds: usize,
    /// What each model call cost, in the order they ran.
    generations: Vec<GenerationTrace>,
    /// Token totals across the whole turn. Every count is `None` if the provider
    /// reported none — an unknown total, never a zero one.
    usage: UsageTotals,
    /// Model latency summed across the turn, which is most of its wall clock.
    latency_ms: u64,
    tool_calls: Vec<ToolTrace>,
}

/// One model call as the harness reports it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct GenerationTrace {
    model: String,
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    total_tokens: Option<u32>,
    finish_reason: Option<String>,
    latency_ms: u64,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
struct UsageTotals {
    #[serde(rename = "prompt_tokens")]
    prompt: Option<u32>,
    #[serde(rename = "completion_tokens")]
    completion: Option<u32>,
    #[serde(rename = "total_tokens")]
    total: Option<u32>,
}

/// One tool the model ran during a turn, as the model saw it.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct ToolTrace {
    name: String,
    /// The model's raw JSON-encoded argument object, verbatim off the wire — not
    /// re-parsed, so a malformed call stays visible instead of being smoothed over.
    arguments: String,
    /// The dispatcher's text result, byte-identical to what went back to the model.
    result: String,
}

/// Why an operator request couldn't be served. Each maps to a status the harness
/// can act on: fix the request, or wait and retry.
#[derive(Debug)]
enum OperatorError {
    /// The request itself is wrong — an unknown tool, a bad tier, a missing id.
    BadRequest(String),
    /// The bot isn't ready to serve this yet (gateway still connecting, or Gary
    /// isn't configured). Distinct from a 404 on purpose: a 404 here would be
    /// indistinguishable from "you're talking to an older image", which is
    /// exactly the confusion this harness exists to remove.
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

/// Everything a request resolved to, once its ids parsed and its tier was
/// checked against what it asked to do.
struct Acting {
    channel_id: serenity::ChannelId,
    guild: Option<u64>,
    author_id: serenity::UserId,
    access: AccessLevel,
    scope: ServerScope,
    confirmations: ConfirmMode,
}

/// Run one of Gary's tools directly, skipping the model. The dispatcher applies
/// the same tier and guild-scope gates it applies to a model-issued call, so a
/// refusal here is the real refusal a friend would hit.
async fn run_tool(
    State(deps): State<OperatorDeps>,
    axum::Json(request): axum::Json<ToolRequest>,
) -> Result<axum::Json<ToolResponse>, OperatorError> {
    let data = &deps.data;
    let serenity_ctx = live_gateway(data)?;
    // Reject an unknown name outright rather than letting it fall through to the
    // dispatcher's "I don't know that tool" text: a typo should be an error the
    // harness sees, not prose it has to read. A *known* tool above the caller's
    // tier still goes through, because that refusal is the thing under test.
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

/// Run a full model turn. The reply is posted to the channel exactly as a real
/// turn would post it, and also returned here alongside the tool-call trace.
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

    let key = (acting.channel_id.get(), acting.author_id.get());
    if request.new_session {
        data.sessions.clear(key);
    }
    let games = game_catalog_list(data);
    let memories = data.memory.render_for_prompt().await;
    let session = data.sessions.checkout(key, std::time::Instant::now(), || {
        build_system_prompt(acting.access, &games, &memories)
    });
    let mut messages = session.messages;
    messages.push(ChatMessage::user(request.prompt.clone()));
    // Everything already in the transcript is prior history; the trace is only
    // what this turn added.
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

    let SessionOutcome {
        reply,
        escalated,
        generations,
    } = outcome;
    let turn_slice = messages.get(mark..).unwrap_or_default();
    let tool_calls = tool_trace(turn_slice);
    let rounds = rounds(turn_slice);
    // Commit last: it moves the transcript the trace was read from.
    data.sessions
        .commit(key, session.id, messages, std::time::Instant::now());

    Ok(axum::Json(GaryResponse {
        reply,
        escalated,
        rounds,
        usage: usage_totals(&generations),
        latency_ms: generations.iter().map(|g| millis(g.latency)).sum(),
        generations: generations.iter().map(generation_trace).collect(),
        tool_calls,
    }))
}

/// The tools a tier may call, with their JSON Schemas — so a harness never has
/// to guess an argument name.
async fn list_tools(
    Query(query): Query<ToolsQuery>,
) -> Result<axum::Json<Vec<ToolDef>>, OperatorError> {
    let access = parse_access(query.access.as_deref())?;
    Ok(axum::Json(available_tools(access)))
}

/// The live gateway Context, or a 503 explaining that Discord isn't connected.
fn live_gateway(data: &Data) -> Result<serenity::Context, OperatorError> {
    data.gateway.current().ok_or_else(|| {
        // Not `debug`: transiently this is just a cold start, but persistently it
        // means a bad token or a gateway that can't connect — a real incident that
        // must not be visible only to whoever happens to be running the harness.
        warn!("operator harness: request arrived before the discord gateway connected");
        OperatorError::Unavailable(
            "the discord gateway isn't connected yet — the bot is still starting up. Retry in a \
             few seconds."
                .to_owned(),
        )
    })
}

/// Resolve a request's ids and tier, rejecting any combination that can't mean
/// what it says.
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

/// The guild a channel belongs to, or `None` for a DM or an unreadable channel.
/// A failure here isn't fatal — the caller can always name the guild explicitly —
/// but it is logged, because silently acting unscoped would be worse.
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

/// Parse a Discord snowflake, rejecting `0` — it parses fine and then silently
/// addresses nothing.
fn parse_id(raw: &str, field: &str) -> Result<u64, OperatorError> {
    raw.trim()
        .parse::<u64>()
        .ok()
        .filter(|&id| id != 0)
        .ok_or_else(|| {
            OperatorError::BadRequest(format!("{field} must be a discord id, got {raw:?}"))
        })
}

/// Parse the requested access tier. An unrecognized value is an error rather
/// than a fall back to the weakest tier: silently downgrading would make a
/// failed admin test read as a passing read-only one.
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

/// Parse the requested visibility scope. `all` is the cross-guild operator view;
/// `guild` needs a guild to scope to, so it's rejected without one rather than
/// quietly widening to everything.
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

/// Pre-approval is admin-only. A non-admin request asking for it is incoherent
/// (the destructive tools aren't even offered below admin), so it's a loud
/// rejection rather than a flag that quietly does nothing.
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

/// Every tool call in `slice` paired with the result that answered it, in
/// execution order. A call with no matching result is kept with an empty result
/// rather than dropped — a vanished call is an invisible failure.
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

/// The tool-result text answering `call_id`, if the loop recorded one.
fn result_for(slice: &[ChatMessage], call_id: &str) -> Option<String> {
    slice
        .iter()
        .find(|message| {
            message.role == Role::Tool && message.tool_call_id.as_deref() == Some(call_id)
        })
        .and_then(|message| message.content.clone())
}

/// One model call, flattened for the harness response.
fn generation_trace(generation: &Generation) -> GenerationTrace {
    let usage = generation.usage.as_ref();
    GenerationTrace {
        model: generation.model.clone(),
        prompt_tokens: usage.and_then(|usage| usage.prompt),
        completion_tokens: usage.and_then(|usage| usage.completion),
        total_tokens: usage.and_then(|usage| usage.total),
        finish_reason: generation.finish_reason.clone(),
        latency_ms: millis(generation.latency),
    }
}

/// A turn's token counts summed across its model calls. A count stays `None`
/// unless at least one call reported it, so "the provider didn't say" can never
/// be misread as "it cost nothing".
fn usage_totals(generations: &[Generation]) -> UsageTotals {
    let mut totals = UsageTotals::default();
    for usage in generations
        .iter()
        .filter_map(|generation| generation.usage.as_ref())
    {
        accumulate(&mut totals.prompt, usage.prompt);
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

/// Model turns this slice represents — one per assistant message the loop
/// appended, whether it carried tool calls or the final text.
fn rounds(slice: &[ChatMessage]) -> usize {
    slice
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .count()
}

#[cfg(test)]
#[path = "tests/operator.rs"]
mod tests;
