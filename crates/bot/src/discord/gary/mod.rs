mod guide;
mod mod_list;
mod recovery;
pub(crate) mod tools;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use poise::serenity_prelude as serenity;
use serenity::CreateMessage;
use tokio::sync::watch;
use tracing::{error, trace, warn};

pub(crate) use self::tools::ConfirmMode;

use super::auth::{AccessLevel, Speaker, access_level_for};
use super::chunking::{DISCORD_MAX_CHARS, chunk_text};
use super::{Data, Error};
use crate::agent::commit_turn;
use crate::agent::{
    ChatMessage, Completion, EscalationReason, Role, SessionCallbacks, SessionEvent, SessionId,
    SessionKey, SessionOutcome, ToolDef, TurnActor, TurnEnding, TurnIdentity, TurnOrigin,
    TurnRecorder, run_session, send_chat_completion,
};
use crate::agones::ServerScope;
use crate::domain::GuildId;
use crate::events::{Actor, Attribution, Surface, mapping};
use crate::notify::{Escalation, EscalationContext, summarize_attempts};
use crate::prompts;
use crate::throttle::{TurnRefusal, TurnSubject};

/// How often the typing indicator is refreshed while a session runs. Discord's
/// indicator lasts ~10s, so 8s keeps it lit without a visible gap.
const TYPING_INTERVAL: Duration = Duration::from_secs(8);

const GARY_NOT_CONFIGURED_REPLY: &str = "Gary isn't set up yet — no model is configured.";

type CompleteFuture<'a> = Pin<Box<dyn Future<Output = Result<Completion>> + Send + 'a>>;
type DispatchFuture<'a> = Pin<Box<dyn Future<Output = String> + Send + 'a>>;
type ProgressFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

pub(crate) async fn on_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    framework: poise::FrameworkContext<'_, Data, Error>,
    data: &Data,
) -> Result<(), Error> {
    let serenity::FullEvent::Message { new_message } = event else {
        return Ok(());
    };
    if new_message.author.bot {
        return Ok(());
    }

    if let Some(guild) = new_message.guild_id
        && !super::guild_is_admitted(data, guild.get())
    {
        return Ok(());
    }

    let bot_id = framework.bot_id;
    let mentioned = new_message.mentions.iter().any(|user| user.id == bot_id);
    if mentioned {
        let prompt = extract_prompt(&new_message.content, bot_id);
        spawn_session(ctx, data, new_message, prompt);
        return Ok(());
    }

    let is_dm = new_message.guild_id.is_none();
    if !is_dm
        && !data
            .home_channels
            .is_home(new_message.channel_id.get())
            .await
    {
        return Ok(());
    }
    let prompt = new_message.content.trim();
    if !is_auto_listen_prompt(prompt) {
        return Ok(());
    }
    spawn_session(ctx, data, new_message, prompt.to_owned());
    Ok(())
}

fn spawn_session(
    ctx: &serenity::Context,
    data: &Data,
    message: &serenity::Message,
    prompt: String,
) {
    let ctx = ctx.clone();
    let data = data.clone();
    let message = message.clone();
    data.tasks.clone().spawn(async move {
        let subject = TurnSubject::Discord(message.author.id.get());
        match data.turn_throttle.claim(&subject).await {
            Ok(()) => {}
            Err(TurnRefusal::RateLimited { retry_after, first }) => {
                if first {
                    reply(&ctx, &message, &rate_limited_reply(retry_after)).await;
                }
                return;
            }
            Err(TurnRefusal::Busy) => {
                reply(&ctx, &message, BUSY_REPLY).await;
                return;
            }
        }
        handle_message(&ctx, &data, &message, prompt).await;
    });
}

fn rate_limited_reply(retry_after: std::time::Duration) -> String {
    let seconds = retry_after.as_secs().max(1);
    if seconds >= 60 {
        let minutes = seconds.div_ceil(60);
        format!(
            "I need a moment to catch up — you've asked for a lot at once. Try me again in about {minutes} minute{}.",
            if minutes == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "I need a moment to catch up — you've asked for a lot at once. Try me again in about {seconds} seconds."
        )
    }
}

const BUSY_REPLY: &str =
    "I'm working through a few requests right now — give me a minute and ask again.";

fn is_auto_listen_prompt(content: &str) -> bool {
    let trimmed = content.trim();
    !trimmed.is_empty() && !trimmed.starts_with('/')
}

async fn handle_message(
    ctx: &serenity::Context,
    data: &Data,
    message: &serenity::Message,
    prompt: String,
) {
    if data.ollama.is_none() {
        reply(ctx, message, GARY_NOT_CONFIGURED_REPLY).await;
        return;
    }
    if prompt.trim().is_empty() {
        reply(
            ctx,
            message,
            "Hi! Mention me with a request — like \"list the servers\" or \"restart minecraft\".",
        )
        .await;
        return;
    }

    let Some(scope) = super::auth::visibility_scope(
        message.author.id.get(),
        message.guild_id.map(serenity::GuildId::get),
        &data.operator_ids,
    ) else {
        reply(
            ctx,
            message,
            "I manage servers inside a Discord server — mention me in a channel of the server you want to manage.",
        )
        .await;
        return;
    };
    let access = caller_access_level(ctx, data, message).await;
    let games = game_catalog_list(data);
    let memories = data.memory.render_for_prompt().await;

    let key = SessionKey::Channel(message.channel_id.get());
    let _turn_lock = data.sessions.turn_lock(key).await;

    let Ok(_permit) = data.turn_throttle.admit_now() else {
        reply(ctx, message, BUSY_REPLY).await;
        return;
    };

    let system = build_system_prompt(access, &games, &memories);
    let session = data
        .sessions
        .checkout(key, Instant::now(), || system.clone());
    let mut messages = session.messages;
    replace_system_prompt(&mut messages, system);
    messages.push(ChatMessage::user(framed_turn(
        &message.author.name,
        access,
        &prompt,
    )));

    let turn = GaryTurn {
        ctx,
        data,
        origin: TurnOrigin::Discord,
        session: Some(session.id.clone()),
        channel_id: message.channel_id,
        guild: message.guild_id.map(serenity::GuildId::get),
        author_id: message.author.id,
        access,
        scope,
        confirmations: ConfirmMode::Interactive,
    };
    if let Ok(outcome) = run_gary_turn(&turn, &mut messages).await {
        if let Some(reason) = outcome.escalation() {
            warn!(
                user = %message.author.id,
                kind = reason.as_str(),
                "agent escalated"
            );
            notify_operators_escalated(data, message, &prompt, &messages, reason).await;
        }
        let limits = data.settings.snapshot().transcript_limits();
        commit_turn(&data.sessions, key, session.id, messages, &outcome, limits);
    }
}

pub(crate) struct GaryTurn<'a> {
    pub(crate) ctx: &'a serenity::Context,
    pub(crate) data: &'a Data,
    pub(crate) origin: TurnOrigin,
    pub(crate) session: Option<SessionId>,
    pub(crate) channel_id: serenity::ChannelId,
    pub(crate) guild: Option<u64>,
    pub(crate) author_id: serenity::UserId,
    pub(crate) access: AccessLevel,
    pub(crate) scope: ServerScope,
    pub(crate) confirmations: ConfirmMode,
}

impl GaryTurn<'_> {
    fn identity(&self) -> TurnIdentity {
        TurnIdentity {
            origin: self.origin,
            deployment: self.data.environment,
            guild: self.guild,
            channel: Some(self.channel_id.get()),
            actor: TurnActor::Discord(self.author_id.get()),
            access: self.access,
            session: self.session.clone(),
            game: None,
        }
    }
}

pub(crate) async fn run_gary_turn(
    turn: &GaryTurn<'_>,
    messages: &mut Vec<ChatMessage>,
) -> Result<SessionOutcome> {
    let Some(connection) = turn.data.ollama_now() else {
        send_chunks(turn.ctx, turn.channel_id, GARY_NOT_CONFIGURED_REPLY).await;
        return Ok(SessionOutcome {
            reply: String::new(),
            ending: TurnEnding::Answered,
            generations: Vec::new(),
        });
    };
    let ollama = &connection;
    let max_rounds = turn.data.settings.snapshot().gary_max_rounds;

    let tool_defs = tools::available_tools(turn.access);
    let tool_ctx = tools::ToolCtx {
        data: turn.data,
        serenity: turn.ctx,
        origin: turn.origin,
        channel_id: turn.channel_id,
        guild: turn.guild,
        author_id: turn.author_id,
        access: turn.access,
        scope: turn.scope.clone(),
        confirmations: turn.confirmations,
        pending_change: std::sync::Mutex::new(None),
        guardrail: std::sync::Mutex::new(None),
        escalation: std::sync::Mutex::new(None),
    };
    let http = &turn.data.http;
    let ctx = turn.ctx;
    let channel_id = turn.channel_id;
    let tool_ctx = &tool_ctx;
    let complete = move |transcript: Vec<ChatMessage>, defs: Vec<ToolDef>| {
        Box::pin(async move { send_chat_completion(http, ollama, &transcript, &defs).await })
            as CompleteFuture<'_>
    };
    let dispatch = move |call| {
        Box::pin(async move { tools::dispatch(tool_ctx, &call).await }) as DispatchFuture<'_>
    };
    let progress = move |event: SessionEvent| {
        Box::pin(async move {
            match event {
                SessionEvent::AssistantText(text) => send_chunks(ctx, channel_id, &text).await,
            }
        }) as ProgressFuture<'_>
    };

    let escalation_requested = move || tool_ctx.escalation_request();

    let recorder = TurnRecorder::new(turn.identity(), Arc::clone(&turn.data.telemetry));
    let (cancel, _interrupt) = turn.data.interrupts.register(turn.channel_id.get());
    let typing = start_typing(turn.ctx, turn.channel_id);
    let outcome = run_session(
        messages,
        tool_defs,
        max_rounds,
        &recorder,
        &SessionCallbacks {
            complete: &complete,
            dispatch: &dispatch,
            progress: &progress,
            escalation_requested: &escalation_requested,
            cancel: &cancel,
        },
    )
    .await;
    drop(typing);

    if let Some(verdict) = tool_ctx.guardrail_verdict() {
        recorder.record_guardrail(verdict);
    }

    match &outcome {
        Ok(SessionOutcome { reply, .. }) => send_chunks(turn.ctx, turn.channel_id, reply).await,
        Err(err) => {
            error!(error = ?err, "agent: session failed");
            send_chunks(
                turn.ctx,
                turn.channel_id,
                "Something went wrong while I was working on that. Try again in a moment.",
            )
            .await;
        }
    }
    outcome
}

async fn notify_operators_escalated(
    data: &Data,
    message: &serenity::Message,
    request: &str,
    messages: &[ChatMessage],
    reason: &EscalationReason,
) {
    let asker = format!("{} (<@{}>)", message.author.name, message.author.id.get());
    let guild = message.guild_id.map(serenity::GuildId::get);
    let escalation = Escalation::from_turn(
        reason,
        EscalationContext::Discord {
            asker,
            jump_link: message.link(),
            guild,
        },
        request.to_owned(),
        summarize_attempts(messages),
    );
    data.events
        .record(mapping::escalation_event(
            &escalation,
            &Attribution::new(Actor::Discord(message.author.id.get()), Surface::Agent),
            guild.map(|guild| GuildId::new(guild.to_string())),
        ))
        .await;
    data.notifier.notify(&escalation).await;
}

fn start_typing(ctx: &serenity::Context, channel_id: serenity::ChannelId) -> watch::Sender<bool> {
    let http = Arc::clone(&ctx.http);
    let (stop_tx, mut stop_rx) = watch::channel(false);
    tokio::spawn(async move {
        loop {
            if let Err(err) = channel_id.broadcast_typing(&http).await {
                trace!(error = %err, "agent: typing indicator refresh failed");
            }
            tokio::select! {
                () = tokio::time::sleep(TYPING_INTERVAL) => {}
                _ = stop_rx.changed() => break,
            }
        }
    });
    stop_tx
}

/// Send `text` to `channel_id` as one or more plain messages, splitting on
/// Discord's size cap without breaking code fences. Posted as ordinary messages
/// (not threaded replies) so a back-and-forth reads like a conversation rather
/// than a stack of "replying to you" cards. Takes a bare `ChannelId` so the
/// deferred-task path — which has no triggering message — can deliver too.
pub(crate) async fn send_chunks(
    ctx: &serenity::Context,
    channel_id: serenity::ChannelId,
    text: &str,
) {
    for chunk in chunk_text(text, DISCORD_MAX_CHARS) {
        if let Err(err) = channel_id
            .send_message(ctx, CreateMessage::new().content(chunk))
            .await
        {
            error!(error = ?err, "agent: failed to send reply chunk");
        }
    }
}

pub(crate) fn replace_system_prompt(messages: &mut Vec<ChatMessage>, system: String) {
    match messages.first_mut() {
        Some(first) if first.role == Role::System => first.content = Some(system),
        Some(_) | None => messages.insert(0, ChatMessage::system(system)),
    }
}

pub(crate) fn framed_turn(asker: &str, access: AccessLevel, prompt: &str) -> String {
    prompts::DiscordQuestion {
        asker,
        access: access.as_str(),
        question: &crate::untrusted::scrub(prompt),
    }
    .render()
}

fn extract_prompt(content: &str, bot_id: serenity::UserId) -> String {
    let id = bot_id.get();
    content
        .replace(&format!("<@{id}>"), "")
        .replace(&format!("<@!{id}>"), "")
        .trim()
        .to_owned()
}

async fn caller_access_level(
    ctx: &serenity::Context,
    data: &Data,
    message: &serenity::Message,
) -> AccessLevel {
    access_level_for(
        ctx,
        data,
        &Speaker {
            user: message.author.id.get(),
            guild: message.guild_id,
            roles: message
                .member
                .as_ref()
                .map(|member| member.roles.iter().map(|role| role.get()).collect())
                .unwrap_or_default(),
        },
    )
    .await
}

pub(crate) fn game_catalog_list(data: &Data) -> String {
    data.catalog.game_ids().collect::<Vec<_>>().join(", ")
}

pub(crate) fn build_system_prompt(access: AccessLevel, games: &str, memories: &str) -> String {
    let mut prompt = prompts::DiscordPersona::render();
    push_block(&mut prompt, &prompts::DiscordSharedChannel::render());
    push_block(&mut prompt, &prompts::DiscordReporting::render());
    push_block(&mut prompt, &prompts::DiscordEscalation::render());
    let games_value = if games.is_empty() { "(none)" } else { games };
    push_block(
        &mut prompt,
        &prompts::DiscordGamesLine { games: games_value }.render(),
    );
    if access >= AccessLevel::Manager {
        append_manager_guidance(&mut prompt, memories);
    }
    if access >= AccessLevel::Admin {
        push_block(&mut prompt, &prompts::DiscordAdminDestructive::render());
        push_block(&mut prompt, &prompts::DiscordAdminConsole::render());
    } else if access >= AccessLevel::Manager {
        push_block(&mut prompt, &prompts::DiscordManagerRestriction::render());
    } else {
        push_block(&mut prompt, &prompts::DiscordReadOnlyRestriction::render());
    }
    prompt
}

fn push_block(prompt: &mut String, block: &str) {
    prompt.push_str("\n\n");
    prompt.push_str(block);
}

fn append_manager_guidance(prompt: &mut String, memories: &str) {
    push_block(prompt, &prompts::DiscordManagerLifecycle::render());
    push_block(prompt, &prompts::DiscordManagerTuning::render());
    push_block(prompt, &prompts::DiscordManagerGameGuide::render());
    push_block(prompt, &prompts::DiscordManagerResources::render());
    push_block(prompt, &prompts::DiscordManagerOccupancy::render());
    push_block(prompt, &prompts::DiscordManagerScheduling::render());
    push_block(prompt, &prompts::DiscordMemoryHabit::render());
    if !memories.is_empty() {
        push_block(prompt, &prompts::DiscordSavedNotes { memories }.render());
    }
}

async fn reply(ctx: &serenity::Context, message: &serenity::Message, text: &str) {
    if let Err(err) = message
        .channel_id
        .send_message(ctx, CreateMessage::new().content(text))
        .await
    {
        error!(error = ?err, "agent: failed to send reply");
    }
}

#[cfg(test)]
#[path = "tests/gary.rs"]
mod tests;
