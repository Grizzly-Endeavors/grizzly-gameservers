use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use grizzly_agent::ToolDefinition;
use tracing::{debug, error, warn};

use super::IngameDeps;
use crate::agent::commit_turn;
use tokio_util::sync::CancellationToken;

use crate::agent::{
    ChatMessage, Completion, EscalationReason, SessionCallbacks, SessionEvent, SessionKey,
    ToolCall, ToolDef, TurnActor, TurnIdentity, TurnOrigin, TurnRecorder, run_session,
    send_chat_completion,
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

const MAX_REPLY_CHARS: usize = 600;

type CompleteFuture<'a> = Pin<Box<dyn Future<Output = Result<Completion>> + Send + 'a>>;
type DispatchFuture<'a> = Pin<Box<dyn Future<Output = String> + Send + 'a>>;
type ProgressFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

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

pub(crate) async fn handle_ingame_question(
    deps: &IngameDeps,
    server: &str,
    player: &str,
    question: &str,
) {
    let Some(connection) = deps.ollama_now() else {
        warn!(server, "in-game trigger arrived but Gary isn't configured");
        return;
    };
    let ollama = &connection;
    let max_rounds = deps.settings.snapshot().gary_max_rounds;

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
    let progress = move |_event: SessionEvent| Box::pin(async {}) as ProgressFuture<'_>;

    let never_escalates = || None;
    let never_cancelled = CancellationToken::new();
    let outcome = run_session(
        &mut messages,
        tool_defs,
        max_rounds,
        &recorder,
        &SessionCallbacks {
            complete: &complete,
            dispatch: &dispatch,
            progress: &progress,
            escalation_requested: &never_escalates,
            cancel: &never_cancelled,
        },
    )
    .await;

    let reply = match outcome {
        Ok(outcome) => {
            let reply = outcome.reply.clone();
            if let Some(reason) = outcome.escalation().cloned() {
                escalate(
                    deps,
                    &attribution,
                    &messages,
                    &reason,
                    Asked {
                        server,
                        player,
                        question,
                        guild: &guild,
                    },
                )
                .await;
            }
            let limits = deps.settings.snapshot().transcript_limits();
            commit_turn(&deps.sessions, key, session.id, messages, &outcome, limits);
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

struct Asked<'a> {
    server: &'a str,
    player: &'a str,
    question: &'a str,
    guild: &'a str,
}

async fn escalate(
    deps: &IngameDeps,
    attribution: &Attribution,
    messages: &[ChatMessage],
    reason: &EscalationReason,
    asked: Asked<'_>,
) {
    warn!(
        server = asked.server,
        player = asked.player,
        kind = reason.as_str(),
        "in-game session escalated"
    );
    let escalation = Escalation::from_turn(
        reason,
        EscalationContext::InGame {
            player: asked.player.to_owned(),
            server: asked.server.to_owned(),
            guild: asked.guild.to_owned(),
        },
        asked.question.to_owned(),
        summarize_attempts(messages),
    );
    deps.events
        .record(mapping::escalation_event(
            &escalation,
            attribution,
            Some(GuildId::new(asked.guild)),
        ))
        .await;
    deps.notifier.notify(&escalation).await;
}

fn framed_question(player: &str, question: &str) -> String {
    if question.trim().is_empty() {
        prompts::IngameNoQuestion { player }.render()
    } else {
        prompts::IngameQuestion {
            player,
            question: &crate::untrusted::scrub(question),
        }
        .render()
    }
}

fn session_key(guild: &str, player: &str) -> SessionKey {
    SessionKey::IngamePlayer {
        guild: guild.parse::<u64>().unwrap_or_else(|_| hash(guild)),
        player: hash(player),
    }
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

fn ingame_tools() -> Vec<ToolDef> {
    vec![
        IngameListServers::spec().into(),
        IngameAboutServer::spec().into(),
    ]
}

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

fn no_such(server: &str) -> String {
    prompts::ServerNotFound { server }.render()
}

fn cluster_error() -> String {
    prompts::ClusterUnreachable::render()
}

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
