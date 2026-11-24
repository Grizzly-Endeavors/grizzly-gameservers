use std::collections::HashMap;
use std::str::FromStr;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use grizzly_control_api::IngameTriggerRequest;
use reqwest::header::AUTHORIZATION;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::config::ChatWatchConfig;

const PER_PLAYER_COOLDOWN: Duration = Duration::from_secs(5);

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatLine {
    pub player: String,
    pub body: String,
}

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

fn is_cooling_down(last_trigger: &mut HashMap<String, Instant>, player: &str) -> bool {
    let now = Instant::now();
    last_trigger.retain(|_, seen| now.duration_since(*seen) < PER_PLAYER_COOLDOWN);
    if last_trigger.contains_key(player) {
        return true;
    }
    last_trigger.insert(player.to_owned(), now);
    false
}

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

#[must_use]
pub fn parse_chat_line(format: ChatFormat, line: &str) -> Option<ChatLine> {
    match format {
        ChatFormat::Minecraft => parse_minecraft_chat(line),
    }
}

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
