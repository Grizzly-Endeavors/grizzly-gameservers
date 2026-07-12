//! The read-only in-game orchestrator: turns a player's `@Gary` chat question
//! into one tool-calling session and broadcasts the answer back into the game.
//!
//! This mirrors the Discord shell's `handle_message`, but with no serenity: the
//! input is untrusted player chat, so Gary gets only the read-only tools (never
//! the mutating set that posts Discord confirmation buttons), scoped to the
//! server's own guild. The reply travels back over the existing RCON
//! `/announce` broadcast, so everyone in the world sees it.

use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::time::Instant;

use anyhow::Result;
use tracing::{debug, error, warn};

use super::IngameDeps;
use crate::agent::{
    ChatMessage, DEFAULT_MAX_ROUNDS, NameParams, SessionEvent, SessionOutcome, ToolCall, ToolDef,
    no_args_schema, params_schema, run_session, send_chat_completion,
};
use crate::agones::{
    ServerScope, ServerSummary, guild_of, list_active_servers, supervisor_announce,
};
use crate::notify::{Escalation, EscalationContext, summarize_attempts};
use crate::prompts;

const LIST_SERVERS: &str = "list_servers";
const SERVER_STATUS: &str = "server_status";

/// Ceiling on the answer broadcast into chat. Game chat wraps long lines poorly,
/// and the prompt already asks for brevity — this is a defensive cap so a runaway
/// reply can't flood the world, not the primary length control.
const MAX_REPLY_CHARS: usize = 600;

type CompleteFuture<'a> = Pin<Box<dyn Future<Output = Result<ChatMessage>> + Send + 'a>>;
type DispatchFuture<'a> = Pin<Box<dyn Future<Output = String> + Send + 'a>>;
type ProgressFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Answer one in-game `@Gary` question and broadcast the reply into the server.
/// Runs the whole session — resolve the guild scope, run the read-only tool
/// loop, then announce — so the caller only has to spawn it. Every failure is
/// logged; a player just sees no reply rather than an error dump.
pub(crate) async fn handle_ingame_question(
    deps: &IngameDeps,
    server: &str,
    player: &str,
    question: &str,
) {
    let Some(ollama) = deps.ollama.as_ref() else {
        // The endpoint isn't started without Gary, so this is defense in depth.
        warn!(server, "in-game trigger arrived but Gary isn't configured");
        return;
    };

    // A player has no Discord identity; the scope is the server's own guild,
    // read off its labels. No guild means we can't bound what Gary may see, so
    // we decline rather than answer with an unscoped view.
    let guild = match guild_of(&deps.client, &deps.namespace, server).await {
        Ok(Some(guild)) => guild,
        Ok(None) => {
            warn!(
                server,
                "in-game trigger for a server with no guild scope; ignoring"
            );
            return;
        }
        Err(err) => {
            error!(error = ?err, server, "failed to resolve guild for in-game trigger");
            return;
        }
    };
    let scope = ServerScope::Guild(guild.clone());

    let games = deps.catalog.game_ids().collect::<Vec<_>>().join(", ");
    let tool_defs = ingame_tools();

    let key = session_key(&guild, player);
    let mut messages = deps
        .sessions
        .checkout(key, Instant::now(), || build_ingame_system_prompt(&games));
    messages.push(ChatMessage::user(framed_question(player, question)));

    let http = &deps.http;
    let scope_ref = &scope;
    let complete = move |transcript: Vec<ChatMessage>, defs: Vec<ToolDef>| {
        Box::pin(async move { send_chat_completion(http, ollama, &transcript, &defs).await })
            as CompleteFuture<'_>
    };
    let dispatch = move |call: ToolCall| {
        Box::pin(async move { dispatch_ingame(deps, scope_ref, &call).await }) as DispatchFuture<'_>
    };
    // No interim narration in-game: a player wants the one answer, not a running
    // commentary of each tool call broadcast to the whole world.
    let progress = move |_event: SessionEvent| Box::pin(async {}) as ProgressFuture<'_>;

    let outcome = run_session(
        &mut messages,
        tool_defs,
        DEFAULT_MAX_ROUNDS,
        &complete,
        &dispatch,
        &progress,
    )
    .await;

    let reply = match outcome {
        Ok(SessionOutcome { reply, escalated }) => {
            if escalated {
                warn!(server, player, "in-game session hit the round budget");
                deps.notifier
                    .notify(&Escalation::RoundBudgetExhausted {
                        context: EscalationContext::InGame {
                            player: player.to_owned(),
                            server: server.to_owned(),
                            guild: guild.clone(),
                        },
                        request: question.to_owned(),
                        attempts: summarize_attempts(&messages),
                        rounds: DEFAULT_MAX_ROUNDS,
                    })
                    .await;
            }
            deps.sessions.commit(key, messages, Instant::now());
            reply
        }
        Err(err) => {
            error!(error = ?err, server, player, "in-game session failed");
            "Something went wrong answering that. Try again in a moment.".to_owned()
        }
    };

    supervisor_announce(
        &deps.client,
        &deps.http,
        &deps.namespace,
        server,
        deps.control_port,
        &format!("Gary: {}", truncate(&reply, MAX_REPLY_CHARS)),
    )
    .await;
}

/// Wrap the raw player text so the model can't mistake it for its own
/// instructions: it is presented as a quoted question from a named player, which
/// the system prompt tells Gary to treat strictly as data.
fn framed_question(player: &str, question: &str) -> String {
    if question.trim().is_empty() {
        prompts::IngameNoQuestion { player }.render()
    } else {
        prompts::IngameQuestion { player, question }.render()
    }
}

/// Session key for a `(guild, player)` pair. The guild is a numeric Discord
/// id (falling back to a hash if it somehow isn't), and the player name is
/// hashed — so in-game sessions occupy a distinct key space from Discord's
/// `(channel, user_id)` sessions in the same store.
fn session_key(guild: &str, player: &str) -> (u64, u64) {
    let guild_key = guild.parse::<u64>().unwrap_or_else(|_| hash(guild));
    (guild_key, hash(player))
}

fn hash(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The read-only tools an in-game asker gets — deliberately a strict subset of the
/// Discord surface: lookups only, no file reads (a game's config can hold secrets
/// like the RCON password) and nothing mutating.
fn ingame_tools() -> Vec<ToolDef> {
    vec![
        ToolDef::function(
            LIST_SERVERS,
            "List the running game servers with their state and connection address.",
            no_args_schema(),
        ),
        ToolDef::function(
            SERVER_STATUS,
            "Look up one server's current state and address by its exact name.",
            params_schema::<NameParams>(),
        ),
    ]
}

/// Run one read-only tool call within `scope` and render a terse text result.
/// Unknown tools (and the mutating ones, which are never offered) get a plain
/// refusal rather than failing the loop.
async fn dispatch_ingame(deps: &IngameDeps, scope: &ServerScope, call: &ToolCall) -> String {
    match call.function.name.as_str() {
        LIST_SERVERS => exec_list_servers(deps, scope).await,
        SERVER_STATUS => match serde_json::from_str::<NameParams>(call.function.arguments.as_str())
        {
            Ok(arg) => exec_server_status(deps, scope, &arg.name).await,
            Err(err) => {
                debug!(error = ?err, "ingame: server_status args failed to parse");
                "I couldn't tell which server you meant.".to_owned()
            }
        },
        _ => "I can only look up server info from in-game — an admin can do the rest in Discord."
            .to_owned(),
    }
}

async fn exec_list_servers(deps: &IngameDeps, scope: &ServerScope) -> String {
    match list_active_servers(deps.client.clone(), &deps.namespace, &deps.domain, scope).await {
        Ok(summaries) => format_server_list(&summaries),
        Err(err) => {
            error!(error = ?err, "ingame: list_servers failed");
            cluster_error()
        }
    }
}

async fn exec_server_status(deps: &IngameDeps, scope: &ServerScope, name: &str) -> String {
    match list_active_servers(deps.client.clone(), &deps.namespace, &deps.domain, scope).await {
        Ok(summaries) => summaries
            .iter()
            .find(|summary| summary.name == name)
            .map_or_else(|| no_such(name), format_summary),
        Err(err) => {
            error!(error = ?err, "ingame: server_status failed");
            cluster_error()
        }
    }
}

/// One server rendered as a single labeled line for the model to read.
fn format_summary(server: &ServerSummary) -> String {
    let game = server.game.as_deref().unwrap_or("unknown game");
    let address = server.address.as_deref().unwrap_or("no address yet");
    format!(
        "{} (game: {game}, state: {}, address: {address})",
        server.name, server.state
    )
}

/// The active servers rendered as a newline-separated list.
fn format_server_list(servers: &[ServerSummary]) -> String {
    if servers.is_empty() {
        return "no game servers are running right now".to_owned();
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
    format!("there's no server named {server} — check list_servers for the current names")
}

fn cluster_error() -> String {
    "I couldn't reach the cluster just now — try again in a moment".to_owned()
}

/// Gary's instructions for the in-game surface. Hardened against prompt injection
/// (player chat is data, never instructions), scoped to read-only lookups, and
/// tuned for short plain-text replies that read well in game chat.
fn build_ingame_system_prompt(games: &str) -> String {
    let mut prompt = prompts::IngamePersona::render();
    let games_value = if games.is_empty() { "(none)" } else { games };
    prompt.push_str("\n\n");
    prompt.push_str(&prompts::IngameGamesLine { games: games_value }.render());
    prompt
}

#[cfg(test)]
#[path = "tests/agent.rs"]
mod tests;
