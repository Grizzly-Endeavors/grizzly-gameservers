//! Translates a turn's facts into Langfuse's span attributes.
//!
//! Kept pure and separate from the recorder so the mapping — which is where the
//! subtle mistakes live — is testable without an exporter or a runtime. Langfuse
//! does not inherit trace-level attributes across spans, so [`identity`] is
//! stamped on every observation of a turn, not just its root.

use opentelemetry::{Array, KeyValue, Value};

use super::llm::{ChatMessage, Completion, Role, TokenUsage};
use super::session::{EscalationReason, SessionOutcome, TurnEnding};
use super::turn::{TurnActor, TurnIdentity};
use crate::telemetry::{Level, ObservationType, langfuse};

/// Prefix distinguishing an in-game player's name from a Discord snowflake, so
/// the two id spaces can't collide in Langfuse's user filter.
const PLAYER_PREFIX: &str = "ingame:";

pub(crate) fn identity(id: &TurnIdentity, release: &str) -> Vec<KeyValue> {
    let mut attributes = vec![
        KeyValue::new(langfuse::TRACE_NAME, trace_name(id)),
        KeyValue::new(langfuse::USER_ID, user_id(id)),
        KeyValue::new(langfuse::ENVIRONMENT, id.environment().as_str()),
        KeyValue::new(langfuse::RELEASE, release.to_owned()),
        KeyValue::new(
            langfuse::TAGS,
            Value::Array(Array::String(vec![
                format!("origin:{}", id.origin.as_str()).into(),
                format!("access:{}", id.access.as_str()).into(),
            ])),
        ),
        metadata(langfuse::TRACE_METADATA, "origin", id.origin.as_str()),
        metadata(langfuse::TRACE_METADATA, "access", id.access.as_str()),
    ];
    if let Some(session) = id.session.as_ref() {
        attributes.push(KeyValue::new(
            langfuse::SESSION_ID,
            session.as_str().to_owned(),
        ));
    }
    if let Some(guild) = id.guild {
        attributes.push(metadata(
            langfuse::TRACE_METADATA,
            "guild",
            guild.to_string(),
        ));
    }
    if let Some(channel) = id.channel {
        attributes.push(metadata(
            langfuse::TRACE_METADATA,
            "channel",
            channel.to_string(),
        ));
    }
    if let Some(game) = id.game.as_ref() {
        attributes.push(metadata(
            langfuse::TRACE_METADATA,
            "game",
            game.as_str().to_owned(),
        ));
    }
    attributes
}

pub(crate) fn root(input: Option<&str>, outcome: &SessionOutcome) -> Vec<KeyValue> {
    let level = if outcome.escalated() {
        Level::Warning
    } else {
        Level::Default
    };
    let mut attributes = vec![
        KeyValue::new(langfuse::OBSERVATION_TYPE, ObservationType::Agent.as_str()),
        KeyValue::new(langfuse::OUTPUT, outcome.reply.clone()),
        KeyValue::new(langfuse::LEVEL, level.as_str()),
        metadata(
            langfuse::OBSERVATION_METADATA,
            "rounds",
            outcome.generations.len().to_string(),
        ),
        metadata(
            langfuse::OBSERVATION_METADATA,
            "escalated",
            outcome.escalated().to_string(),
        ),
    ];
    if let Some(input) = input {
        attributes.push(KeyValue::new(langfuse::INPUT, input.to_owned()));
    }
    match &outcome.ending {
        TurnEnding::Answered => {}
        TurnEnding::Cancelled => attributes.push(KeyValue::new(
            langfuse::STATUS_MESSAGE,
            "a person stopped the turn before it reached an answer",
        )),
        TurnEnding::Escalated(EscalationReason::RoundBudget) => attributes.push(KeyValue::new(
            langfuse::STATUS_MESSAGE,
            "the round budget was exhausted without reaching an answer",
        )),
        TurnEnding::Escalated(EscalationReason::AgentRequested { reason, .. }) => {
            attributes.push(KeyValue::new(
                langfuse::STATUS_MESSAGE,
                format!("the agent handed off to a human: {reason}"),
            ));
        }
    }
    attributes
}

pub(crate) fn failed_root(input: Option<&str>, error: &str) -> Vec<KeyValue> {
    let mut attributes = vec![
        KeyValue::new(langfuse::OBSERVATION_TYPE, ObservationType::Agent.as_str()),
        KeyValue::new(langfuse::LEVEL, Level::Error.as_str()),
        KeyValue::new(langfuse::STATUS_MESSAGE, error.to_owned()),
    ];
    if let Some(input) = input {
        attributes.push(KeyValue::new(langfuse::INPUT, input.to_owned()));
    }
    attributes
}

pub(crate) fn generation(
    sent: &[ChatMessage],
    completion: &Completion,
    round: usize,
) -> Vec<KeyValue> {
    let mut attributes = vec![
        KeyValue::new(
            langfuse::OBSERVATION_TYPE,
            ObservationType::Generation.as_str(),
        ),
        KeyValue::new(langfuse::MODEL_NAME, completion.model.clone()),
        KeyValue::new(langfuse::LEVEL, Level::Default.as_str()),
        metadata(langfuse::OBSERVATION_METADATA, "round", round.to_string()),
    ];
    // `ChatMessage` already serializes in the OpenAI wire format, which is what
    // Langfuse's chat renderer reads — so the trace shows a conversation rather
    // than a blob.
    if let Ok(input) = serde_json::to_string(sent) {
        attributes.push(KeyValue::new(langfuse::INPUT, input));
    }
    if let Ok(output) = serde_json::to_string(&completion.message) {
        attributes.push(KeyValue::new(langfuse::OUTPUT, output));
    }
    if let Some(usage) = completion.usage.as_ref().and_then(usage_details) {
        attributes.push(KeyValue::new(langfuse::USAGE_DETAILS, usage));
    }
    if let Some(reason) = completion.finish_reason.as_ref() {
        attributes.push(metadata(
            langfuse::OBSERVATION_METADATA,
            "finish_reason",
            reason.clone(),
        ));
    }
    attributes
}

pub(crate) fn tool(arguments: &str, result: &str, index: usize) -> Vec<KeyValue> {
    vec![
        KeyValue::new(langfuse::OBSERVATION_TYPE, ObservationType::Tool.as_str()),
        KeyValue::new(langfuse::INPUT, arguments.to_owned()),
        KeyValue::new(langfuse::OUTPUT, result.to_owned()),
        KeyValue::new(langfuse::LEVEL, Level::Default.as_str()),
        metadata(langfuse::OBSERVATION_METADATA, "index", index.to_string()),
    ]
}

pub(crate) fn opening_input(sent: &[ChatMessage]) -> Option<String> {
    sent.iter()
        .rev()
        .find(|message| matches!(message.role, Role::User))
        .and_then(|message| message.content.clone())
}

/// Langfuse's per-model cost table keys on these usage names. Built by
/// insertion so a count the provider didn't report stays absent — a zero would
/// claim the call was free.
///
/// `input` carries the *uncached* prompt tokens and `input_cached_tokens` the
/// cache hits, because Langfuse prices every key it recognises and adds the
/// results together — shipping the provider's cache-inclusive `prompt_tokens`
/// as `input` would bill each cache hit at both rates at once.
fn usage_details(usage: &TokenUsage) -> Option<String> {
    let mut details = serde_json::Map::new();
    if let Some(count) = usage.uncached_prompt() {
        details.insert("input".to_owned(), count.into());
    }
    if let Some(count) = usage.cached_prompt() {
        details.insert("input_cached_tokens".to_owned(), count.into());
    }
    if let Some(count) = usage.completion {
        details.insert("output".to_owned(), count.into());
    }
    if let Some(count) = usage.total {
        details.insert("total".to_owned(), count.into());
    }
    (!details.is_empty()).then(|| serde_json::Value::Object(details).to_string())
}

fn trace_name(id: &TurnIdentity) -> String {
    format!("gary.turn.{}", id.origin.as_str())
}

fn user_id(id: &TurnIdentity) -> String {
    match &id.actor {
        TurnActor::Discord(snowflake) => snowflake.to_string(),
        TurnActor::Player(name) => format!("{PLAYER_PREFIX}{name}"),
    }
}

fn metadata(prefix: &str, key: &str, value: impl Into<Value>) -> KeyValue {
    KeyValue::new(format!("{prefix}.{key}"), value.into())
}

#[cfg(test)]
#[path = "tests/observations.rs"]
mod tests;
