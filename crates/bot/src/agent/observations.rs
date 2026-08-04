//! Translates a turn's facts into Langfuse's span attributes.
//!
//! Kept pure and separate from the recorder so the mapping — which is where the
//! subtle mistakes live — is testable without an exporter or a runtime. Langfuse
//! does not inherit trace-level attributes across spans, so [`identity`] is
//! stamped on every observation of a turn, not just its root.

use opentelemetry::{Array, KeyValue, Value};

use super::llm::{ChatMessage, Completion, Role, TokenUsage};
use super::session::SessionOutcome;
use super::turn::{TurnActor, TurnIdentity};
use crate::telemetry::{Level, ObservationType, langfuse};

/// Prefix distinguishing an in-game player's name from a Discord snowflake, so
/// the two id spaces can't collide in Langfuse's user filter.
const PLAYER_PREFIX: &str = "ingame:";

/// The trace-level block, repeated on every span of the turn.
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
    // A deferred batch is a one-shot conversation with no session. Omitting the
    // key is what keeps it out of Langfuse's session view; an empty string would
    // read as a session literally named "".
    if let Some(session) = id.session.as_ref() {
        attributes.push(KeyValue::new(
            langfuse::SESSION_ID,
            session.as_str().to_owned(),
        ));
    }
    // Snowflakes go in as strings, matching how the bot's own tables store them.
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
    // Omitted rather than defaulted when the surface knows no game, for the same
    // reason as `session` above: a placeholder would show up in Langfuse as a real
    // game with real cost attributed to it. An absent key reads as "not broken
    // down", which is the truth.
    if let Some(game) = id.game.as_ref() {
        attributes.push(metadata(
            langfuse::TRACE_METADATA,
            "game",
            game.as_str().to_owned(),
        ));
    }
    attributes
}

/// The root observation: the turn as a whole.
pub(crate) fn root(input: Option<&str>, outcome: &SessionOutcome) -> Vec<KeyValue> {
    let level = if outcome.escalated {
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
            outcome.escalated.to_string(),
        ),
    ];
    if let Some(input) = input {
        attributes.push(KeyValue::new(langfuse::INPUT, input.to_owned()));
    }
    if outcome.escalated {
        attributes.push(KeyValue::new(
            langfuse::STATUS_MESSAGE,
            "the round budget was exhausted without reaching an answer",
        ));
    }
    attributes
}

/// The root observation for a turn that died on the model endpoint. No outcome
/// scores follow it — a provider outage isn't an agent result.
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

/// One model call. `sent` is the exact message array the request carried, which
/// is what makes a trace replayable rather than a summary of one.
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

/// One dispatched tool call. Arguments and result go in **verbatim**: they are
/// already the strings the model produced and read, and re-encoding them would
/// bury a config edit's old/new text under a layer of escaping.
pub(crate) fn tool(arguments: &str, result: &str, index: usize) -> Vec<KeyValue> {
    vec![
        KeyValue::new(langfuse::OBSERVATION_TYPE, ObservationType::Tool.as_str()),
        KeyValue::new(langfuse::INPUT, arguments.to_owned()),
        KeyValue::new(langfuse::OUTPUT, result.to_owned()),
        // Deliberately never ERROR: the loop feeds tool failures back to the
        // model as ordinary result text, so a failure here is the agent working,
        // not the tool breaking.
        KeyValue::new(langfuse::LEVEL, Level::Default.as_str()),
        metadata(langfuse::OBSERVATION_METADATA, "index", index.to_string()),
    ]
}

/// The turn's opening question, recovered from the transcript the first model
/// call sent. The recorder is never handed the prompt directly, but the last
/// user message of round one is exactly it.
pub(crate) fn opening_input(sent: &[ChatMessage]) -> Option<String> {
    sent.iter()
        .rev()
        .find(|message| matches!(message.role, Role::User))
        .and_then(|message| message.content.clone())
}

/// Langfuse's per-model cost table keys on these usage names. Built by
/// insertion so a count the provider didn't report stays absent — a zero would
/// claim the call was free.
fn usage_details(usage: &TokenUsage) -> Option<String> {
    let mut details = serde_json::Map::new();
    if let Some(count) = usage.prompt {
        details.insert("input".to_owned(), count.into());
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
