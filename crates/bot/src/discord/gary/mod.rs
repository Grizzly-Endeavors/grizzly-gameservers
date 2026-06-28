//! Gary, the Discord-facing ops agent. An `@mention` runs a fresh tool-calling
//! session against the configured model: anyone may ask, but only admins are
//! handed the mutating tools. This is the Discord shell — the model client and
//! loop live in `crate::agent`, the tool executors in [`tools`].

mod tools;

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use poise::serenity_prelude as serenity;
use serenity::{CreateMessage, EditMessage};
use tracing::{error, warn};

use super::auth::is_authorized;
use super::{Data, Error};
use crate::agent::{
    ChatMessage, DEFAULT_MAX_ROUNDS, SessionOutcome, ToolDef, run_session, send_chat_completion,
};

/// Discord's hard cap on message content. Replies are trimmed to fit under it.
const MAX_DISCORD_CONTENT: usize = 2000;

/// Shown while a session runs, then edited in place with the final reply.
const THINKING_PLACEHOLDER: &str = "🤔 thinking…";

type CompleteFuture<'a> = Pin<Box<dyn Future<Output = Result<ChatMessage>> + Send + 'a>>;
type DispatchFuture<'a> = Pin<Box<dyn Future<Output = String> + Send + 'a>>;

/// Poise event hook: route any message that mentions the bot to Gary, ignoring
/// everything else (and the bot's own messages, to avoid loops).
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
    if !new_message.mentions.iter().any(|user| user.id == bot_id) {
        return Ok(());
    }

    let prompt = extract_prompt(&new_message.content, bot_id);
    handle_mention(ctx, data, new_message, prompt).await;
    Ok(())
}

/// Run one agent session for a mention and report the result back in-channel.
/// Builds the model/tool callbacks, drives the loop, then edits the placeholder
/// with the final reply.
async fn handle_mention(
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

    let is_admin = caller_is_admin(data, message);
    let tool_defs = tools::available_tools(is_admin);
    let system_prompt = build_system_prompt(is_admin, &game_catalog_list(data));

    let Some(mut placeholder) = post_placeholder(ctx, message).await else {
        return;
    };

    let tool_ctx = tools::ToolCtx {
        data,
        serenity: ctx,
        channel_id: message.channel_id,
        author_id: message.author.id,
        is_admin,
    };
    // Capture only Copy references (`&`), so each closure stays `Fn` — the
    // session may call them across several rounds. A `move` closure that closed
    // over the non-Copy `tool_ctx` directly would be `FnOnce`.
    let http = &data.http;
    let tool_ctx = &tool_ctx;
    let complete = move |messages: Vec<ChatMessage>, defs: Vec<ToolDef>| {
        Box::pin(async move { send_chat_completion(http, ollama, &messages, &defs).await })
            as CompleteFuture<'_>
    };
    let dispatch = move |call| {
        Box::pin(async move { tools::dispatch(tool_ctx, &call).await }) as DispatchFuture<'_>
    };

    let final_text = match run_session(
        system_prompt,
        prompt,
        tool_defs,
        DEFAULT_MAX_ROUNDS,
        &complete,
        &dispatch,
    )
    .await
    {
        Ok(SessionOutcome { reply, escalated }) => {
            if escalated {
                warn!(user = %message.author.id, "agent escalated: round budget exhausted");
            }
            reply
        }
        Err(err) => {
            error!(error = ?err, user = %message.author.id, "agent: session failed");
            "Something went wrong while I was working on that. Try again in a moment.".to_owned()
        }
    };

    if let Err(err) = placeholder
        .edit(ctx, EditMessage::new().content(truncate(&final_text)))
        .await
    {
        error!(error = ?err, "agent: failed to edit reply with result");
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
/// slash commands enforce (explicit allowlist or the admin role).
fn caller_is_admin(data: &Data, message: &serenity::Message) -> bool {
    let role_ids: Vec<u64> = message
        .member
        .as_ref()
        .map(|member| member.roles.iter().map(|role| role.get()).collect())
        .unwrap_or_default();
    is_authorized(
        message.author.id.get(),
        &role_ids,
        data.admin_role_id,
        &data.admin_user_ids,
    )
}

fn game_catalog_list(data: &Data) -> String {
    data.catalog.game_ids().collect::<Vec<_>>().join(", ")
}

/// Gary's instructions. The admin variant mentions the mutating tools and the
/// confirm-before-destroy contract; the read-only variant scopes him to lookups.
fn build_system_prompt(is_admin: bool, games: &str) -> String {
    let mut prompt = String::from(
        "You are Gary, a friendly assistant who manages game servers for a group of friends on \
         Discord. The people talking to you are not technical, so keep replies short, plain, and \
         warm — no jargon, no stack traces, no internal IDs unless asked. Use the tools to find \
         out the real state of things; never guess a server's name or status — call list_servers \
         first if you're unsure. If a tool reports a problem, relay it plainly and suggest the \
         next step. If you can't accomplish what was asked, say so honestly rather than pretending.",
    );
    prompt.push_str("\n\nAvailable games to launch: ");
    prompt.push_str(if games.is_empty() { "(none)" } else { games });
    if is_admin {
        prompt.push_str(
            "\n\nThis person is an admin: you may create, stop, start, restart, and shut down \
             servers for them. Deleting a server (remove) destroys its world permanently and \
             always asks them to confirm with a button first — describe what you're about to \
             delete before you call it, and respect their answer.",
        );
    } else {
        prompt.push_str(
            "\n\nThis person is not an admin: you can look up servers and their status for them, \
             but you cannot create, change, or delete anything. If they ask for one of those, \
             explain warmly that an admin has to do it.",
        );
    }
    prompt
}

async fn post_placeholder(
    ctx: &serenity::Context,
    message: &serenity::Message,
) -> Option<serenity::Message> {
    match message
        .channel_id
        .send_message(
            ctx,
            CreateMessage::new()
                .content(THINKING_PLACEHOLDER)
                .reference_message(message),
        )
        .await
    {
        Ok(posted) => Some(posted),
        Err(err) => {
            error!(error = ?err, "agent: failed to post thinking placeholder");
            None
        }
    }
}

async fn reply(ctx: &serenity::Context, message: &serenity::Message, text: &str) {
    if let Err(err) = message
        .channel_id
        .send_message(
            ctx,
            CreateMessage::new()
                .content(text)
                .reference_message(message),
        )
        .await
    {
        error!(error = ?err, "agent: failed to send reply");
    }
}

/// Trim a reply to fit Discord's content cap, marking that it was cut.
fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_DISCORD_CONTENT {
        return text.to_owned();
    }
    let keep = MAX_DISCORD_CONTENT.saturating_sub(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('…');
    out
}

#[cfg(test)]
#[path = "tests/gary.rs"]
mod tests;
