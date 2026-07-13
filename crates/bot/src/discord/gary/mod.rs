//! Gary, the Discord-facing ops agent. An `@mention` runs a tool-calling session
//! against the configured model — continuing this person's recent conversation
//! in the channel when one is still live (see [`crate::agent::SessionStore`]),
//! else starting fresh. Anyone may ask, but the tools handed to the model are
//! scoped to the caller's tier (see [`super::auth::AccessLevel`]): read-only
//! lookups for everyone, the day-to-day lifecycle and file-editing set for
//! managers, and the destructive tools plus console commands for admins. This is
//! the Discord shell — the model client and loop live in `crate::agent`, the
//! tool executors in [`tools`].

mod recovery;
mod tools;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use poise::serenity_prelude as serenity;
use serenity::CreateMessage;
use tokio::sync::watch;
use tracing::{error, trace, warn};

use super::auth::{AccessCheck, AccessLevel, access_level};
use super::chunking::{DISCORD_MAX_CHARS, chunk_text};
use super::{Data, Error};
use crate::agent::{
    ChatMessage, DEFAULT_MAX_ROUNDS, SessionEvent, SessionOutcome, ToolDef, run_session,
    send_chat_completion,
};
use crate::agones::ServerScope;
use crate::notify::{Escalation, EscalationContext, summarize_attempts};
use crate::prompts;

/// How often the typing indicator is refreshed while a session runs. Discord's
/// indicator lasts ~10s, so 8s keeps it lit without a visible gap.
const TYPING_INTERVAL: Duration = Duration::from_secs(8);

/// Shown when Gary has no model configured, from the `@mention` entry point
/// and, as defense in depth, inside [`run_gary_turn`] itself.
const GARY_NOT_CONFIGURED_REPLY: &str = "Gary isn't set up yet — no model is configured.";

type CompleteFuture<'a> = Pin<Box<dyn Future<Output = Result<ChatMessage>> + Send + 'a>>;
type DispatchFuture<'a> = Pin<Box<dyn Future<Output = String> + Send + 'a>>;
type ProgressFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Poise event hook: route a message to Gary when it's addressed to him — an
/// `@mention` anywhere, or *any* message in a DM or a registered home channel
/// (where a mention isn't required). The bot's own messages are ignored to avoid
/// loops.
///
/// In the no-mention (DM/home) path, empty messages and ones that look like a
/// slash command (leading `/`) are skipped silently, so Gary doesn't chime in on
/// every stray line or echo a mistyped command back. The `@mention` path keeps
/// its existing behavior, including the "mention me with a request" nudge.
///
/// # Errors
///
/// Propagates only fatal serenity errors; per-message failures are handled in
/// place by replying with a friendly message and logging the cause.
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

    let bot_id = framework.bot_id;
    let mentioned = new_message.mentions.iter().any(|user| user.id == bot_id);
    if mentioned {
        let prompt = extract_prompt(&new_message.content, bot_id);
        spawn_session(ctx, data, new_message, prompt);
        return Ok(());
    }

    // No mention: only listen in a DM or a registered home channel, and only to
    // an actual request — skip blanks and slash-command-style lines.
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

/// Run a Gary turn on the shared task tracker instead of inline in poise's event
/// dispatch, so the shutdown drain can await an in-flight turn (a mutating tool
/// call and its follow-up) rather than the gateway socket closing under it. The
/// spawned task owns cheap clones of the handler's borrowed inputs.
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
        handle_message(&ctx, &data, &message, prompt).await;
    });
}

/// Whether a no-mention message is something Gary should answer: non-empty and
/// not a slash-command-style line (leading `/`). Real slash commands arrive as
/// interactions, not messages, so this only guards typed `/foo` text — but it
/// keeps Gary from reacting to it, or to blank lines, in a home channel or DM.
fn is_auto_listen_prompt(content: &str) -> bool {
    let trimmed = content.trim();
    !trimmed.is_empty() && !trimmed.starts_with('/')
}

/// Run one agent turn for a message addressed to Gary and report back
/// in-channel. Builds the model/tool callbacks, keeps a typing indicator lit
/// while the loop runs, posts the model's interim narration as it arrives and
/// its final reply (chunked), then persists the transcript for the next turn.
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

    // Resolve the caller's tenant scope. A non-operator in a DM has no guild to
    // scope to, so Gary can't act on any server — guide them to a guild channel.
    // Operators keep the all-guilds view even in a DM (they can manage anything).
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
    // Rendered here (async) rather than inside the checkout closure (sync). A fact
    // Gary saves mid-conversation lands in the prompt on the next fresh session;
    // within this one it's already in his transcript.
    let memories = data.memory.render_for_prompt().await;

    // Continue this person's conversation in this channel if it's still live,
    // else start fresh; the appended user turn is what the model answers.
    let key = (message.channel_id.get(), message.author.id.get());
    let mut messages = data.sessions.checkout(key, Instant::now(), || {
        build_system_prompt(access, &games, &memories)
    });
    messages.push(ChatMessage::user(prompt.clone()));

    let turn = GaryTurn {
        ctx,
        data,
        channel_id: message.channel_id,
        guild: message.guild_id.map(serenity::GuildId::get),
        author_id: message.author.id,
        access,
        scope,
    };
    // On a hard error, run_gary_turn already logged and posted a friendly message;
    // the prior session is left untouched so a retry doesn't inherit a half-finished
    // transcript. Only a clean turn is committed for continuity.
    if let Ok(SessionOutcome { escalated, .. }) = run_gary_turn(&turn, &mut messages).await {
        if escalated {
            warn!(user = %message.author.id, "agent escalated: round budget exhausted");
            notify_operators_escalated(data, message, &prompt, &messages).await;
        }
        data.sessions.commit(key, messages, Instant::now());
    }
}

/// The identity and delivery target for one Gary turn, decoupled from any
/// `serenity::Message`. The `@mention` path fills it from the incoming message;
/// the deferred-task path fills it from a queued batch — so both drive the same
/// [`run_gary_turn`].
pub(crate) struct GaryTurn<'a> {
    pub(crate) ctx: &'a serenity::Context,
    pub(crate) data: &'a Data,
    /// Where the reply (and interim narration) is posted.
    pub(crate) channel_id: serenity::ChannelId,
    pub(crate) guild: Option<u64>,
    pub(crate) author_id: serenity::UserId,
    pub(crate) access: AccessLevel,
    pub(crate) scope: ServerScope,
}

/// Run the tool-calling loop for a seeded transcript, keeping a typing indicator
/// lit, posting the model's interim narration and final reply (chunked) to the
/// turn's channel. Mutates `messages` in place so the caller holds the full
/// transcript to persist. Owns all Discord delivery — including a friendly message
/// on a hard error — so callers only handle session bookkeeping and escalation.
///
/// # Errors
///
/// Returns an error only if the model call itself fails (the endpoint is
/// unreachable); tool failures are surfaced to the model as text, not propagated.
pub(crate) async fn run_gary_turn(
    turn: &GaryTurn<'_>,
    messages: &mut Vec<ChatMessage>,
) -> Result<SessionOutcome> {
    let Some(ollama) = turn.data.ollama.as_ref() else {
        // Callers gate on Gary being configured; this is defense in depth.
        send_chunks(turn.ctx, turn.channel_id, GARY_NOT_CONFIGURED_REPLY).await;
        return Ok(SessionOutcome {
            reply: String::new(),
            escalated: false,
        });
    };

    let tool_defs = tools::available_tools(turn.access);
    let tool_ctx = tools::ToolCtx {
        data: turn.data,
        serenity: turn.ctx,
        channel_id: turn.channel_id,
        guild: turn.guild,
        author_id: turn.author_id,
        access: turn.access,
        scope: turn.scope.clone(),
        pending_change: std::sync::Mutex::new(None),
    };
    // Capture only Copy references (`&`), so each closure stays `Fn` — the session
    // may call them across several rounds.
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
    // Post interim narration inline, before its tool calls run, so "I'll delete
    // minecraft — confirm below" lands ahead of the tool's own side effects.
    let progress = move |event: SessionEvent| {
        Box::pin(async move {
            match event {
                SessionEvent::AssistantText(text) => send_chunks(ctx, channel_id, &text).await,
            }
        }) as ProgressFuture<'_>
    };

    let typing = start_typing(turn.ctx, turn.channel_id);
    let outcome = run_session(
        messages,
        tool_defs,
        DEFAULT_MAX_ROUNDS,
        &complete,
        &dispatch,
        &progress,
    )
    .await;
    drop(typing);

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

/// DM the operators that Gary gave up on this request, with the jump link, the
/// asker, what they asked, and the tools he tried. Split out of [`handle_message`]
/// so its body stays under the line cap.
async fn notify_operators_escalated(
    data: &Data,
    message: &serenity::Message,
    request: &str,
    messages: &[ChatMessage],
) {
    let asker = format!("{} (<@{}>)", message.author.name, message.author.id.get());
    data.notifier
        .notify(&Escalation::RoundBudgetExhausted {
            context: EscalationContext::Discord {
                asker,
                jump_link: message.link(),
                guild: message.guild_id.map(serenity::GuildId::get),
            },
            request: request.to_owned(),
            attempts: summarize_attempts(messages),
            rounds: DEFAULT_MAX_ROUNDS,
        })
        .await;
}

/// Keep Discord's typing indicator lit for `channel_id` until the returned guard
/// is dropped. Dropping it closes the watch channel, which wakes the task out of
/// its sleep and ends it.
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

/// Strip the bot's mention markup (both `<@id>` and the legacy `<@!id>`) from the
/// message content, leaving the bare request.
fn extract_prompt(content: &str, bot_id: serenity::UserId) -> String {
    let id = bot_id.get();
    content
        .replace(&format!("<@{id}>"), "")
        .replace(&format!("<@!{id}>"), "")
        .trim()
        .to_owned()
}

/// The message author's access tier — the same policy the slash commands
/// enforce (see [`access_level`]). A cross-guild operator is admin everywhere; a
/// non-operator with no guild (a DM) is read-only.
async fn caller_access_level(
    ctx: &serenity::Context,
    data: &Data,
    message: &serenity::Message,
) -> AccessLevel {
    let user = message.author.id.get();
    if data.operator_ids.contains(&user) {
        return AccessLevel::Admin;
    }
    let Some(guild_id) = message.guild_id else {
        return AccessLevel::ReadOnly;
    };
    let roles: Vec<u64> = message
        .member
        .as_ref()
        .map(|member| member.roles.iter().map(|role| role.get()).collect())
        .unwrap_or_default();
    let guild_owner = guild_owner_id(ctx, guild_id).await;
    let guild_admins = data.guild_config.admins(guild_id.get()).await;
    let guild_managers = data.guild_config.managers(guild_id.get()).await;
    access_level(&AccessCheck {
        user,
        roles: &roles,
        guild_owner,
        operators: &data.operator_ids,
        guild_admins: &guild_admins,
        guild_managers: &guild_managers,
    })
}

/// The guild's owner id, cache-first (the guild is cached from its `GuildCreate`),
/// falling back to an HTTP fetch. `None` on a read failure — auth then fails
/// closed to operators only for that check.
async fn guild_owner_id(ctx: &serenity::Context, guild_id: serenity::GuildId) -> Option<u64> {
    if let Some(guild) = guild_id.to_guild_cached(&ctx.cache) {
        return Some(guild.owner_id.get());
    }
    match guild_id.to_partial_guild(&ctx.http).await {
        Ok(guild) => Some(guild.owner_id.get()),
        Err(err) => {
            error!(error = ?err, guild = guild_id.get(), "failed to read guild for owner check");
            None
        }
    }
}

pub(crate) fn game_catalog_list(data: &Data) -> String {
    data.catalog.game_ids().collect::<Vec<_>>().join(", ")
}

/// Gary's instructions, tailored to the caller's tier. Managers and admins both
/// get the lifecycle and file-tuning tools; admins additionally get the
/// destructive tools and console commands; read-only callers are scoped to
/// lookups.
pub(crate) fn build_system_prompt(access: AccessLevel, games: &str, memories: &str) -> String {
    let mut prompt = prompts::DiscordPersona::render();
    push_block(&mut prompt, &prompts::DiscordReporting::render());
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

/// Append a system-prompt block after the inter-block separator. Prompt files
/// carry no leading/trailing whitespace, so the `\n\n` glue between blocks is
/// owned here rather than baked into the files.
fn push_block(prompt: &mut String, block: &str) {
    prompt.push_str("\n\n");
    prompt.push_str(block);
}

/// Append the manager-and-above guidance to `prompt`: the lifecycle grant, the
/// inspect/tune-and-restart workflow (including that a restart self-guards a config
/// change it applies), the occupancy check before a restart, and the durable-memory
/// habit, plus any saved notes. Split out of [`build_system_prompt`] to keep the
/// tier gating readable.
fn append_manager_guidance(prompt: &mut String, memories: &str) {
    push_block(prompt, &prompts::DiscordManagerLifecycle::render());
    push_block(prompt, &prompts::DiscordManagerTuning::render());
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
