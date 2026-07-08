//! Watches the supervised game's captured output for players addressing the ops
//! agent in chat and forwards each hit to the bot's agent endpoint, closing the
//! in-game → agent reverse loop.
//!
//! The watcher is game-agnostic: the per-game template declares how that game
//! renders chat lines ([`ChatFormat`], via `SUPERVISOR_CHAT_FORMAT`) and the
//! trigger a player types (`SUPERVISOR_CHAT_TRIGGER`, default `@Gary`). Parsing
//! only matches genuine player-chat lines (`<player> message`), so the agent's
//! own `tellraw` replies — which never render in that shape — cannot re-trigger
//! it. The reply itself travels back over the existing RCON `/announce` bridge;
//! this module is only the inbound half.

use std::collections::HashMap;
use std::str::FromStr;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use grizzly_control_api::IngameTriggerRequest;
use reqwest::header::AUTHORIZATION;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::config::ChatWatchConfig;

/// Minimum spacing between triggers accepted from one player. Each trigger is an
/// LLM round-trip on the bot, so this keeps a single player from flooding it;
/// distinct players are unaffected.
const PER_PLAYER_COOLDOWN: Duration = Duration::from_secs(5);

/// How a given game renders chat lines in its stdout log stream. Selected per game
/// via `SUPERVISOR_CHAT_FORMAT` so the watcher itself stays game-agnostic — the
/// same split the outbound `send_command`/`announce` bridge already uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatFormat {
    Minecraft,
}

impl FromStr for ChatFormat {
    type Err = anyhow::Error;

    fn from_str(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "minecraft" => Ok(Self::Minecraft),
            other => Err(anyhow!("unknown chat format {other:?}")),
        }
    }
}

/// A parsed player chat line: who spoke and the full message body (trigger not yet
/// stripped).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatLine {
    pub player: String,
    pub body: String,
}

/// Consume captured output lines, forwarding any that address the agent. Runs
/// until the sender half (the log line pump) closes at supervisor shutdown.
pub async fn run(mut lines: mpsc::Receiver<String>, cfg: ChatWatchConfig, http: reqwest::Client) {
    if cfg.agent_token.is_none() {
        warn!(
            "in-game chat watcher running without an agent token; the bot endpoint is \
             unauthenticated (set SUPERVISOR_AGENT_TOKEN)"
        );
    }
    let mut last_trigger: HashMap<String, Instant> = HashMap::new();
    while let Some(line) = lines.recv().await {
        let Some(chat) = parse_chat_line(cfg.format, &line) else {
            continue;
        };
        let Some(question) = strip_trigger(&cfg.trigger, &chat.body) else {
            continue;
        };
        if is_cooling_down(&mut last_trigger, &chat.player) {
            debug!(player = %chat.player, "ignoring in-game trigger within cooldown");
            continue;
        }
        forward_trigger(&http, &cfg, &chat.player, &question).await;
    }
    debug!("chat watcher stopping; log stream closed");
}

/// Whether `player` triggered the agent too recently to accept another. Prunes
/// entries that have aged out first, so the map stays bounded to recently-active
/// players rather than every name ever seen.
fn is_cooling_down(last_trigger: &mut HashMap<String, Instant>, player: &str) -> bool {
    let now = Instant::now();
    last_trigger.retain(|_, seen| now.duration_since(*seen) < PER_PLAYER_COOLDOWN);
    if last_trigger.contains_key(player) {
        return true;
    }
    last_trigger.insert(player.to_owned(), now);
    false
}

/// POST one trigger to the bot's agent endpoint, best-effort: the reply comes back
/// asynchronously over RCON, so nothing here waits on Gary. A delivery failure is
/// logged and dropped — a chat question is not worth stalling log capture over.
async fn forward_trigger(
    http: &reqwest::Client,
    cfg: &ChatWatchConfig,
    player: &str,
    message: &str,
) {
    let mut request = http.post(&cfg.agent_url).json(&IngameTriggerRequest {
        server: cfg.server.clone(),
        player: player.to_owned(),
        message: message.to_owned(),
    });
    if let Some(token) = &cfg.agent_token {
        request = request.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    match request.send().await {
        Ok(reply) if reply.status().is_success() => {
            debug!(player, "forwarded in-game trigger to the agent");
        }
        Ok(reply) => {
            warn!(status = %reply.status(), player, "agent endpoint rejected in-game trigger");
        }
        Err(err) => {
            warn!(error = %err, url = %cfg.agent_url, "failed to reach the agent endpoint");
        }
    }
}

/// Parse one captured output line into a player chat message, or `None` if it is
/// not player chat. Only genuine `<player> message` chat matches — server output,
/// join/leave notices, RCON echoes, and the agent's own `tellraw` replies do not,
/// which is what keeps the reverse loop from feeding itself.
#[must_use]
pub fn parse_chat_line(format: ChatFormat, line: &str) -> Option<ChatLine> {
    match format {
        ChatFormat::Minecraft => parse_minecraft_chat(line),
    }
}

/// Minecraft renders player chat as `[HH:MM:SS] [Server thread/INFO]: <Player> msg`
/// (optionally prefixed `[Not Secure] ` when chat signing is off). We locate the
/// log-prefix terminator `]: `, then require the message to open with `<name>` —
/// the shape only player chat produces.
fn parse_minecraft_chat(line: &str) -> Option<ChatLine> {
    let after_prefix = line.split_once("]: ").map(|(_, rest)| rest)?;
    let message = after_prefix
        .strip_prefix("[Not Secure] ")
        .unwrap_or(after_prefix);
    let inner = message.strip_prefix('<')?;
    let (player, body) = inner.split_once('>')?;
    if player.is_empty() {
        return None;
    }
    Some(ChatLine {
        player: player.to_owned(),
        body: body.trim_start().to_owned(),
    })
}

/// If `body` addresses the agent (contains `trigger`, case-insensitively), return
/// the question with the trigger token removed and surrounding whitespace
/// trimmed; otherwise `None`. An empty remainder (a bare `@Gary`) is preserved so
/// the agent can prompt for what the player needs.
#[must_use]
pub fn strip_trigger(trigger: &str, body: &str) -> Option<String> {
    let start = body
        .to_ascii_lowercase()
        .find(&trigger.to_ascii_lowercase())?;
    let end = start + trigger.len();
    let before = body.get(..start).unwrap_or_default().trim();
    let after = body.get(end..).unwrap_or_default().trim();
    let question = match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (true, false) => after.to_owned(),
        (false, true) => before.to_owned(),
        (false, false) => format!("{before} {after}"),
    };
    Some(question)
}

#[cfg(test)]
#[path = "tests/chat_watcher.rs"]
mod tests;
