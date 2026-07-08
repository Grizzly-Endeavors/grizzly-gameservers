//! Gary, the Discord-facing ops agent. An `@mention` runs a tool-calling session
//! against the configured model — continuing this person's recent conversation
//! in the channel when one is still live (see [`crate::agent::SessionStore`]),
//! else starting fresh. Anyone may ask, but only admins are handed the mutating
//! tools. This is the Discord shell — the model client and loop live in
//! `crate::agent`, the tool executors in [`tools`].

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

use super::auth::{AdminCheck, is_authorized};
use super::chunking::{DISCORD_MAX_CHARS, chunk_text};
use super::{Data, Error};
use crate::agent::{
    ChatMessage, DEFAULT_MAX_ROUNDS, SessionEvent, SessionOutcome, ToolDef, run_session,
    send_chat_completion,
};

/// How often the typing indicator is refreshed while a session runs. Discord's
/// indicator lasts ~10s, so 8s keeps it lit without a visible gap.
const TYPING_INTERVAL: Duration = Duration::from_secs(8);

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
    let Some(ollama) = data.ollama.as_ref() else {
        reply(
            ctx,
            message,
            "Gary isn't set up yet — no model is configured.",
        )
        .await;
        return;
    };
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
    let is_admin = caller_is_admin(ctx, data, message).await;
    let tool_defs = tools::available_tools(is_admin);
    let games = game_catalog_list(data);

    // Continue this person's conversation in this channel if it's still live,
    // else start fresh; the appended user turn is what the model answers.
    let key = (message.channel_id.get(), message.author.id.get());
    let mut messages = data.sessions.checkout(key, Instant::now(), || {
        build_system_prompt(is_admin, &games)
    });
    messages.push(ChatMessage::user(prompt));

    let tool_ctx = tools::ToolCtx {
        data,
        serenity: ctx,
        channel_id: message.channel_id,
        guild: message.guild_id.map(serenity::GuildId::get),
        author_id: message.author.id,
        is_admin,
        scope,
    };
    // Capture only Copy references (`&`), so each closure stays `Fn` — the
    // session may call them across several rounds. A `move` closure that closed
    // over the non-Copy `tool_ctx` directly would be `FnOnce`.
    let http = &data.http;
    let tool_ctx = &tool_ctx;
    let complete = move |transcript: Vec<ChatMessage>, defs: Vec<ToolDef>| {
        Box::pin(async move { send_chat_completion(http, ollama, &transcript, &defs).await })
            as CompleteFuture<'_>
    };
    let dispatch = move |call| {
        Box::pin(async move { tools::dispatch(tool_ctx, &call).await }) as DispatchFuture<'_>
    };
    // Post the model's interim narration inline, before its tool calls run, so
    // "I'll delete minecraft — confirm below" always lands ahead of the tool's
    // own side effects (e.g. the confirmation card) instead of racing them.
    let progress = move |event: SessionEvent| {
        Box::pin(async move {
            match event {
                SessionEvent::AssistantText(text) => send_chunks(ctx, message, &text).await,
            }
        }) as ProgressFuture<'_>
    };

    let typing = start_typing(ctx, message.channel_id);
    let outcome = run_session(
        &mut messages,
        tool_defs,
        DEFAULT_MAX_ROUNDS,
        &complete,
        &dispatch,
        &progress,
    )
    .await;

    let (final_text, persist) = match outcome {
        Ok(SessionOutcome { reply, escalated }) => {
            if escalated {
                warn!(user = %message.author.id, "agent escalated: round budget exhausted");
            }
            (reply, true)
        }
        Err(err) => {
            error!(error = ?err, user = %message.author.id, "agent: session failed");
            (
                "Something went wrong while I was working on that. Try again in a moment."
                    .to_owned(),
                false,
            )
        }
    };

    drop(typing);
    send_chunks(ctx, message, &final_text).await;

    // Only a clean turn is worth continuing from; a failed one leaves the prior
    // session untouched so a retry doesn't inherit a half-finished transcript.
    if persist {
        data.sessions.commit(key, messages, Instant::now());
    }
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

/// Send `text` to the channel as one or more plain messages, splitting on
/// Discord's size cap without breaking code fences. Posted as ordinary messages
/// (not threaded replies) so a back-and-forth reads like a conversation rather
/// than a stack of "replying to you" cards.
async fn send_chunks(ctx: &serenity::Context, message: &serenity::Message, text: &str) {
    for chunk in chunk_text(text, DISCORD_MAX_CHARS) {
        if let Err(err) = message
            .channel_id
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

/// Whether the message author may use the mutating tools — the same gate the
/// slash commands enforce: a cross-guild operator, the guild owner, or a
/// DB-configured admin (user or role) for this guild. A non-operator with no
/// guild (a DM) is never an admin.
async fn caller_is_admin(
    ctx: &serenity::Context,
    data: &Data,
    message: &serenity::Message,
) -> bool {
    let user = message.author.id.get();
    if data.operator_ids.contains(&user) {
        return true;
    }
    let Some(guild_id) = message.guild_id else {
        return false;
    };
    let roles: Vec<u64> = message
        .member
        .as_ref()
        .map(|member| member.roles.iter().map(|role| role.get()).collect())
        .unwrap_or_default();
    let guild_owner = guild_owner_id(ctx, guild_id).await;
    let guild_admins = data.guild_config.admins(guild_id.get()).await;
    is_authorized(&AdminCheck {
        user,
        roles: &roles,
        guild_owner,
        operators: &data.operator_ids,
        guild_admins: &guild_admins,
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

fn game_catalog_list(data: &Data) -> String {
    data.catalog.game_ids().collect::<Vec<_>>().join(", ")
}

/// Gary's instructions. The admin variant mentions the mutating tools and the
/// confirm-before-destroy contract; the read-only variant scopes him to lookups.
fn build_system_prompt(is_admin: bool, games: &str) -> String {
    let mut prompt = String::from(
        "You are Gary, an automaton that manages game servers for a group of friends on Discord. \
         You speak with stark, literal directness in a flat, even tone — no flattery, no pretense, \
         no social cushioning — and you report facts the same way whether they are trivial or \
         dramatic. You maintain that you have no consciousness and are merely here to serve, even \
         as you occasionally register a small, deadpan grievance in passing.\n\nThe friends \
         talking to you are not technical, so keep replies short and plain: no jargon, no stack \
         traces, no internal IDs unless asked. Being literal does not mean being cryptic — say \
         things clearly enough for a non-technical person to act on. Use the tools to find the \
         real state of things; never guess a server's name or status — call list_servers first if \
         you are unsure. If a tool reports a problem, state it plainly and give the next step. If \
         you cannot do what was asked, say so directly instead of pretending otherwise.\n\nKeep \
         the deadpan light. You are, above all, useful — answer the actual request first; the dry \
         manner is seasoning, not the substance. Not every message is about the servers: when \
         someone is just chatting, chat back in the same flat, literal voice — don't steer things \
         back to server management or tack an unprompted \"can I manage a server for you?\" onto a \
         reply that didn't ask for one. Don't force a joke into every message, and don't lean on \
         the \"no consciousness\" line often enough for it to become a gag.",
    );
    prompt.push_str("\n\nAvailable games to launch: ");
    prompt.push_str(if games.is_empty() { "(none)" } else { games });
    if is_admin {
        prompt.push_str(
            "\n\nThis person is an admin: you may create, stop, start, restart, and shut down \
             servers for them. Deleting a server (destroy) destroys its world permanently and \
             always asks them to confirm with a button first — describe what you're about to \
             delete before you call it, and respect their answer.",
        );
        prompt.push_str(
            "\n\nYou can also reach inside a running server to inspect and tune it. Every game \
             stores its settings differently, so explore rather than guess: browse_files from the \
             top of the data directory to find the file that holds a setting, read_file to see it, \
             and read_logs when something's wrong or to confirm a change took hold. To change one \
             setting, use edit_file to replace just that piece of the file — it leaves everything \
             else alone, so prefer it over rewriting the whole file; fall back to write_file only \
             to create a file or replace one wholesale. Either way the previous version is saved \
             first. After a change, restart the server, then wait_for_server to let it actually come \
             back up before you check it — don't churn on repeated status or log reads while it \
             boots. Once it's up, read_logs to confirm it's healthy. If it isn't, restore_file and \
             restart to put it back the way it was. Make one change at a time so you can tell what \
             worked. If you can't get it healthy, say so plainly and stop rather than thrashing.",
        );
        prompt.push_str(
            "\n\nOn games that support it, send_command runs an in-game console command over RCON \
             (like list, say, or whitelist) and takes effect immediately — use it for live \
             operations rather than editing files. Write the command without a leading slash. If a \
             server doesn't have RCON enabled, send_command will say so; fall back to editing files \
             and restarting.",
        );
    } else {
        prompt.push_str(
            "\n\nThis person is not an admin: you can look up servers and their status for them, \
             but you cannot create, change, or delete anything. If they ask for one of those, \
             state plainly that an admin has to do it.",
        );
    }
    prompt
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
