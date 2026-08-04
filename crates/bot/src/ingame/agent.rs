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
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tracing::{debug, error, warn};

use super::IngameDeps;
use crate::agent::{
    ChatMessage, Completion, DEFAULT_MAX_ROUNDS, SessionEvent, SessionOutcome, ToolCall, ToolDef,
    TurnActor, TurnIdentity, TurnOrigin, TurnRecorder, run_session, send_chat_completion,
};
use crate::agones::{
    ServerScope, ServerSummary, list_active_servers, owner_of, supervisor_announce,
};
use crate::discord::AccessLevel;
use crate::domain::{GameId, GuildId};
use crate::events::{Actor, Attribution, Surface, mapping};
use crate::notify::{Escalation, EscalationContext, summarize_attempts};
use crate::prompts;
use crate::prompts::{IngameAboutServer, IngameListServers, NameParams};

/// Ceiling on the answer broadcast into chat. Game chat wraps long lines poorly,
/// and the prompt already asks for brevity — this is a defensive cap so a runaway
/// reply can't flood the world, not the primary length control.
const MAX_REPLY_CHARS: usize = 600;

type CompleteFuture<'a> = Pin<Box<dyn Future<Output = Result<Completion>> + Send + 'a>>;
type DispatchFuture<'a> = Pin<Box<dyn Future<Output = String> + Send + 'a>>;
type ProgressFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// The guild an in-game question is answered within, and the game it is about,
/// both read off the server's own labels — a player has no Discord identity to
/// derive either from.
///
/// `None` declines the question: without a guild there is no bound on what Gary
/// may see, and answering with an unscoped view would leak other guilds' servers
/// to whoever typed in chat. Every such case is logged, since the player just
/// sees silence.
async fn resolve_scope(deps: &IngameDeps, server: &str) -> Option<(String, Option<GameId>)> {
    let owner = match owner_of(&deps.client, &deps.namespace, server).await {
        Ok(Some(owner)) => owner,
        Ok(None) => {
            warn!(server, "in-game trigger for an unknown server; ignoring");
            return None;
        }
        Err(err) => {
            error!(error = ?err, server, "failed to resolve owner for in-game trigger");
            return None;
        }
    };
    let Some(guild) = owner.guild else {
        warn!(
            server,
            "in-game trigger for a server with no guild scope; ignoring"
        );
        return None;
    };
    Some((guild.as_str().to_owned(), owner.game))
}

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

    let Some((guild, game)) = resolve_scope(deps, server).await else {
        return;
    };
    let scope = ServerScope::Guild(guild.clone());

    let games = deps.catalog.game_ids().collect::<Vec<_>>().join(", ");
    let tool_defs = ingame_tools();

    let key = session_key(&guild, player);
    let session = deps
        .sessions
        .checkout(key, Instant::now(), || build_ingame_system_prompt(&games));
    let mut messages = session.messages;
    messages.push(ChatMessage::user(framed_question(player, question)));

    // The in-game surface has no Discord identity and no tier of its own: its
    // tool set is a fixed read-only subset, so that's what the turn runs at.
    let identity = TurnIdentity {
        origin: TurnOrigin::Ingame,
        deployment: deps.environment,
        guild: guild.parse::<u64>().ok(),
        channel: None,
        actor: TurnActor::Player(player.to_owned()),
        access: AccessLevel::ReadOnly,
        session: Some(session.id.clone()),
        game,
    };
    let attribution = Attribution::new(Actor::from_turn(&identity.actor), Surface::Agent);
    let recorder = TurnRecorder::new(identity, Arc::clone(&deps.telemetry));

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
        &recorder,
        &complete,
        &dispatch,
        &progress,
    )
    .await;

    let reply = match outcome {
        Ok(SessionOutcome {
            reply, escalated, ..
        }) => {
            if escalated {
                warn!(server, player, "in-game session hit the round budget");
                escalate(
                    deps,
                    &attribution,
                    &messages,
                    Asked {
                        server,
                        player,
                        question,
                        guild: &guild,
                    },
                )
                .await;
            }
            deps.sessions
                .commit(key, session.id, messages, Instant::now());
            reply
        }
        Err(err) => {
            error!(error = ?err, server, player, "in-game session failed");
            "Something went wrong answering that. Try again in a moment.".to_owned()
        }
    };

    supervisor_announce(
        &deps.client,
        &deps.control_http,
        &deps.namespace,
        server,
        deps.control_port,
        &format!("Gary: {}", truncate(&reply, MAX_REPLY_CHARS)),
    )
    .await;
}

/// Who asked what, and where — the locators an in-game escalation carries.
struct Asked<'a> {
    server: &'a str,
    player: &'a str,
    question: &'a str,
    guild: &'a str,
}

/// Record an in-game give-up and hand it to the operators. Recorded beside the
/// DM rather than inside it, so an escalation lands in the product record whether
/// or not there is an operator configured to hear about it.
async fn escalate(
    deps: &IngameDeps,
    attribution: &Attribution,
    messages: &[ChatMessage],
    asked: Asked<'_>,
) {
    let escalation = Escalation::RoundBudgetExhausted {
        context: EscalationContext::InGame {
            player: asked.player.to_owned(),
            server: asked.server.to_owned(),
            guild: asked.guild.to_owned(),
        },
        request: asked.question.to_owned(),
        attempts: summarize_attempts(messages),
        rounds: DEFAULT_MAX_ROUNDS,
    };
    deps.events
        .record(mapping::escalation_event(
            &escalation,
            attribution,
            Some(GuildId::new(asked.guild)),
        ))
        .await;
    deps.notifier.notify(&escalation).await;
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
        IngameListServers::spec().into(),
        IngameAboutServer::spec().into(),
    ]
}

/// Run one read-only tool call within `scope` and render a terse text result.
/// Unknown tools (and the mutating ones, which are never offered) get a plain
/// refusal rather than failing the loop.
async fn dispatch_ingame(deps: &IngameDeps, scope: &ServerScope, call: &ToolCall) -> String {
    match call.function.name.as_str() {
        IngameListServers::NAME => exec_list_servers(deps, scope).await,
        IngameAboutServer::NAME => {
            match serde_json::from_str::<NameParams>(call.function.arguments.as_str()) {
                Ok(arg) => exec_about_server(deps, scope, &arg.name).await,
                Err(err) => {
                    debug!(error = ?err, "ingame: about_server args failed to parse");
                    prompts::IngameServerUnclear::render()
                }
            }
        }
        _ => prompts::IngameLookupOnly::render(),
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

async fn exec_about_server(deps: &IngameDeps, scope: &ServerScope, name: &str) -> String {
    match list_active_servers(deps.client.clone(), &deps.namespace, &deps.domain, scope).await {
        Ok(summaries) => summaries
            .iter()
            .find(|summary| summary.name == name)
            .map_or_else(|| no_such(name), format_summary),
        Err(err) => {
            error!(error = ?err, "ingame: about_server failed");
            cluster_error()
        }
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
