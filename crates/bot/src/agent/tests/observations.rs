use std::time::Duration;

use super::*;
use crate::agent::store::SessionId;
use crate::agent::turn::{Environment, TurnOrigin};
use crate::discord::AccessLevel;
use crate::domain::GameId;

fn discord_identity() -> TurnIdentity {
    TurnIdentity {
        origin: TurnOrigin::Discord,
        deployment: Environment::Production,
        guild: Some(42),
        channel: Some(7),
        actor: TurnActor::Discord(1234),
        access: AccessLevel::Manager,
        session: Some(SessionId::from_raw("abc123")),
        game: None,
    }
}

fn assistant(text: &str) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: Some(text.to_owned()),
        tool_calls: None,
        tool_call_id: None,
    }
}

fn completion(usage: Option<TokenUsage>) -> Completion {
    Completion {
        message: assistant("all good"),
        model: "glm-5.2".to_owned(),
        usage,
        finish_reason: Some("stop".to_owned()),
        latency: Duration::from_millis(900),
    }
}

fn find<'a>(attributes: &'a [KeyValue], key: &str) -> Option<&'a Value> {
    attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| &kv.value)
}

fn text(attributes: &[KeyValue], key: &str) -> String {
    find(attributes, key)
        .unwrap_or_else(|| panic!("missing attribute {key}"))
        .as_str()
        .into_owned()
}

#[test]
fn a_discord_turn_carries_its_session_and_snowflakes() {
    let attributes = identity(&discord_identity(), "sha256:abc");
    assert_eq!(text(&attributes, langfuse::SESSION_ID), "abc123");
    assert_eq!(text(&attributes, langfuse::USER_ID), "1234");
    assert_eq!(text(&attributes, langfuse::RELEASE), "sha256:abc");
    assert_eq!(
        text(&attributes, "langfuse.trace.metadata.guild"),
        "42",
        "snowflakes go in as strings"
    );
}

/// A deferred batch is a one-shot conversation. An empty session id would show
/// up in Langfuse as a session literally named "", so the key must be absent.
#[test]
fn a_deferred_turn_has_no_session_attribute() {
    let identity_without_session = TurnIdentity {
        origin: TurnOrigin::Deferred,
        session: None,
        ..discord_identity()
    };
    let attributes = identity(&identity_without_session, "dev");
    assert!(find(&attributes, langfuse::SESSION_ID).is_none());
}

#[test]
fn an_ingame_player_is_namespaced_apart_from_discord_ids() {
    let ingame = TurnIdentity {
        origin: TurnOrigin::Ingame,
        actor: TurnActor::Player("1234".to_owned()),
        ..discord_identity()
    };
    assert_eq!(
        text(&identity(&ingame, "dev"), langfuse::USER_ID),
        "ingame:1234"
    );
}

/// Without this, cost and escalation rate can only be read for the whole fleet:
/// a Minecraft turn carrying a large modpack config costs materially more than a
/// Valheim one, and nothing in the trace says which is which.
#[test]
fn a_turn_that_knows_its_game_can_be_sliced_by_it() {
    let ingame = TurnIdentity {
        origin: TurnOrigin::Ingame,
        game: Some(GameId::new("minecraft")),
        ..discord_identity()
    };
    assert_eq!(
        text(&identity(&ingame, "dev"), "langfuse.trace.metadata.game"),
        "minecraft"
    );
}

/// A Discord turn often names no server, and a placeholder would show up in
/// Langfuse as a real game with real cost attributed to it. The key is absent so
/// the breakdown is honest about what it does not cover.
#[test]
fn a_turn_with_no_game_omits_the_attribute_rather_than_inventing_one() {
    let attributes = identity(&discord_identity(), "dev");
    assert!(find(&attributes, "langfuse.trace.metadata.game").is_none());
}

/// The whole point of the operator environment: harness traffic must never be
/// counted as product signal, even though the process itself is production.
#[test]
fn an_operator_turn_is_quarantined_from_production() {
    let operator = TurnIdentity {
        origin: TurnOrigin::Operator,
        deployment: Environment::Production,
        ..discord_identity()
    };
    assert_eq!(
        text(&identity(&operator, "dev"), langfuse::ENVIRONMENT),
        "operator"
    );
}

#[test]
fn the_trace_name_names_the_surface() {
    assert_eq!(
        text(&identity(&discord_identity(), "dev"), langfuse::TRACE_NAME),
        "gary.turn.discord"
    );
}

#[test]
fn a_generation_records_the_exact_transcript_it_sent() {
    let sent = vec![
        ChatMessage::system("be helpful"),
        ChatMessage::user("restart minecraft"),
    ];
    let attributes = generation(&sent, &completion(None), 1);
    let input = text(&attributes, langfuse::INPUT);
    let round_tripped = serde_json::from_str::<Vec<ChatMessage>>(&input).unwrap();
    assert_eq!(round_tripped.len(), 2);
    assert_eq!(
        round_tripped.last().and_then(|m| m.content.as_deref()),
        Some("restart minecraft")
    );
}

#[test]
fn a_generation_reports_the_counts_the_provider_sent() {
    let usage = TokenUsage {
        prompt: Some(120),
        completion: Some(30),
        total: Some(150),
    };
    let attributes = generation(&[], &completion(Some(usage)), 1);
    let details = text(&attributes, langfuse::USAGE_DETAILS);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&details).unwrap(),
        serde_json::json!({"input": 120, "output": 30, "total": 150})
    );
}

/// A count the provider didn't report is unknown, never zero — a zero would
/// claim the call was free and quietly understate cost.
#[test]
fn an_unreported_count_is_omitted_rather_than_zeroed() {
    let usage = TokenUsage {
        prompt: Some(120),
        completion: None,
        total: None,
    };
    let attributes = generation(&[], &completion(Some(usage)), 1);
    let details = text(&attributes, langfuse::USAGE_DETAILS);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&details).unwrap(),
        serde_json::json!({"input": 120})
    );
}

#[test]
fn a_generation_with_no_usage_at_all_carries_no_usage_attribute() {
    let all_absent = generation(&[], &completion(Some(TokenUsage::default())), 1);
    assert!(find(&all_absent, langfuse::USAGE_DETAILS).is_none());
    let no_usage_block = generation(&[], &completion(None), 1);
    assert!(find(&no_usage_block, langfuse::USAGE_DETAILS).is_none());
}

/// Arguments and result are already the strings the model wrote and read.
/// Re-encoding them would bury a config edit's diff under escaping.
#[test]
fn a_tool_observation_keeps_arguments_and_result_verbatim() {
    let arguments = r#"{"server":"minecraft","path":"server.properties"}"#;
    let attributes = tool(arguments, "restarted and healthy", 0);
    assert_eq!(text(&attributes, langfuse::INPUT), arguments);
    assert_eq!(text(&attributes, langfuse::OUTPUT), "restarted and healthy");
    assert_eq!(text(&attributes, langfuse::LEVEL), "DEFAULT");
}

#[test]
fn an_escalated_turn_is_flagged_at_warning() {
    let outcome = SessionOutcome {
        reply: "flagged for Bear".to_owned(),
        escalation: Some(EscalationReason::RoundBudget),
        generations: Vec::new(),
    };
    let attributes = root(Some("fix it"), &outcome);
    assert_eq!(text(&attributes, langfuse::LEVEL), "WARNING");
    assert_eq!(
        text(&attributes, "langfuse.observation.metadata.escalated"),
        "true"
    );
}

#[test]
fn a_plain_turn_records_its_reply_and_round_count() {
    let outcome = SessionOutcome {
        reply: "minecraft is up".to_owned(),
        escalation: None,
        generations: vec![(&completion(None)).into(), (&completion(None)).into()],
    };
    let attributes = root(Some("is it up?"), &outcome);
    assert_eq!(text(&attributes, langfuse::LEVEL), "DEFAULT");
    assert_eq!(text(&attributes, langfuse::OUTPUT), "minecraft is up");
    assert_eq!(text(&attributes, langfuse::INPUT), "is it up?");
    assert_eq!(
        text(&attributes, "langfuse.observation.metadata.rounds"),
        "2"
    );
}

#[test]
fn a_failed_turn_reports_the_error_at_error_level() {
    let attributes = failed_root(Some("hello"), "model endpoint unreachable");
    assert_eq!(text(&attributes, langfuse::LEVEL), "ERROR");
    assert_eq!(
        text(&attributes, langfuse::STATUS_MESSAGE),
        "model endpoint unreachable"
    );
}

#[test]
fn the_opening_question_is_the_last_user_message_of_the_first_round() {
    let sent = vec![
        ChatMessage::system("be helpful"),
        ChatMessage::user("older turn"),
        assistant("answered"),
        ChatMessage::user("restart minecraft"),
    ];
    assert_eq!(opening_input(&sent).as_deref(), Some("restart minecraft"));
}

#[test]
fn a_transcript_with_no_user_message_has_no_opening() {
    assert!(opening_input(&[ChatMessage::system("be helpful")]).is_none());
}
