use super::*;
use crate::agent::llm::{PromptTokenDetails, TokenUsage};

/// The text of a rejection, so a test can assert the harness is *told* what to
/// fix rather than just handed a status code.
fn reason(err: OperatorError) -> String {
    match err {
        OperatorError::BadRequest(text) | OperatorError::Unavailable(text) => text,
    }
}

fn assistant_calling(calls: &[(&str, &str, &str)]) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: None,
        tool_calls: Some(
            calls
                .iter()
                .map(|(id, name, args)| ToolCall {
                    id: (*id).to_owned(),
                    kind: "function".to_owned(),
                    function: crate::agent::llm::FunctionCall {
                        name: (*name).to_owned(),
                        arguments: (*args).to_owned(),
                    },
                })
                .collect(),
        ),
        tool_call_id: None,
        reasoning_content: None,
    }
}

fn trace(name: &str, arguments: &str, result: &str) -> ToolTrace {
    ToolTrace {
        name: name.to_owned(),
        arguments: arguments.to_owned(),
        result: result.to_owned(),
    }
}

#[test]
fn access_tiers_parse_from_their_wire_names() {
    assert_eq!(parse_access(None).unwrap(), AccessLevel::Admin);
    assert_eq!(parse_access(Some("admin")).unwrap(), AccessLevel::Admin);
    assert_eq!(parse_access(Some("manager")).unwrap(), AccessLevel::Manager);
    assert_eq!(
        parse_access(Some("read-only")).unwrap(),
        AccessLevel::ReadOnly
    );
    assert_eq!(
        parse_access(Some("  admin  ")).unwrap(),
        AccessLevel::Admin,
        "surrounding whitespace shouldn't change the tier"
    );
}

#[test]
fn an_unrecognized_access_tier_is_an_error_not_a_downgrade() {
    // Falling back to ReadOnly would make a failed admin test look like a
    // passing read-only one — the harness would report success on a lie.
    let err = reason(parse_access(Some("readonly")).unwrap_err());
    assert!(
        err.contains("read-only"),
        "the error should name the accepted spellings, got: {err}"
    );
    assert!(parse_access(Some("Admin")).is_err(), "matching is exact");
    assert!(parse_access(Some("")).is_err());
}

#[test]
fn scope_defaults_to_the_channels_guild() {
    assert_eq!(
        parse_scope(None, Some(42)).unwrap(),
        ServerScope::Guild("42".to_owned())
    );
    assert_eq!(
        parse_scope(Some("guild"), Some(42)).unwrap(),
        ServerScope::Guild("42".to_owned())
    );
    assert_eq!(parse_scope(Some("all"), None).unwrap(), ServerScope::All);
}

#[test]
fn guild_scope_without_a_guild_is_refused_rather_than_widened() {
    let err = reason(parse_scope(None, None).unwrap_err());
    assert!(
        err.contains("guild"),
        "the error should say a guild is needed, got: {err}"
    );
    assert!(parse_scope(Some("everything"), Some(42)).is_err());
}

#[test]
fn snowflakes_must_be_real_ids() {
    assert_eq!(
        parse_id("123456789012345678", "channel").unwrap(),
        1_524_223_576_067_145_818
    );
    assert_eq!(
        parse_id(" 42 ", "channel").unwrap(),
        42,
        "ids are trimmed, since they arrive from shell interpolation"
    );

    let zero = reason(parse_id("0", "channel").unwrap_err());
    assert!(
        zero.contains("channel"),
        "the error should name the field, got: {zero}"
    );
    assert!(parse_id("", "guild").is_err());
    assert!(parse_id("not-an-id", "user").is_err());
    assert!(
        parse_id("-1", "user").is_err(),
        "a snowflake is unsigned; a negative must not wrap"
    );
}

#[test]
fn preapproval_is_admin_only() {
    assert_eq!(
        parse_confirm(false, AccessLevel::ReadOnly).unwrap(),
        ConfirmMode::Interactive,
        "not asking for pre-approval leaves the human click in place at every tier"
    );
    assert_eq!(
        parse_confirm(true, AccessLevel::Admin).unwrap(),
        ConfirmMode::Preapproved
    );

    // Silently ignoring the flag would let a test "pass" having never exercised
    // the path it asked for.
    let err = reason(parse_confirm(true, AccessLevel::Manager).unwrap_err());
    assert!(
        err.contains("admin"),
        "the error should say the flag is admin-only, got: {err}"
    );
    assert!(parse_confirm(true, AccessLevel::ReadOnly).is_err());
}

#[test]
fn a_turn_with_no_tools_traces_empty() {
    let slice = [
        ChatMessage::user("how are the servers?"),
        ChatMessage {
            role: Role::Assistant,
            content: Some("All three are up.".to_owned()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        },
    ];
    assert!(tool_trace(&slice).is_empty());
    assert_eq!(rounds(&slice), 1);
}

#[test]
fn a_single_call_traces_with_its_result() {
    let slice = [
        ChatMessage::user("status of mc?"),
        assistant_calling(&[("c1", "about_server", r#"{"name":"mc"}"#)]),
        ChatMessage::tool_result("c1", "mc — minecraft, Ready"),
        ChatMessage {
            role: Role::Assistant,
            content: Some("mc is up.".to_owned()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        },
    ];
    assert_eq!(
        tool_trace(&slice),
        vec![trace(
            "about_server",
            r#"{"name":"mc"}"#,
            "mc — minecraft, Ready"
        )]
    );
    assert_eq!(rounds(&slice), 2, "one tool round plus the final answer");
}

#[test]
fn several_calls_in_one_round_each_pair_with_their_own_result() {
    let slice = [
        assistant_calling(&[
            ("a", "about_server", r#"{"name":"mc"}"#),
            ("b", "about_server", r#"{"name":"valheim"}"#),
        ]),
        // Deliberately out of call order: pairing is by id, not position.
        ChatMessage::tool_result("b", "valheim — Ready"),
        ChatMessage::tool_result("a", "mc — Paused"),
    ];
    assert_eq!(
        tool_trace(&slice),
        vec![
            trace("about_server", r#"{"name":"mc"}"#, "mc — Paused"),
            trace("about_server", r#"{"name":"valheim"}"#, "valheim — Ready"),
        ],
        "the trace keeps the model's call order, not the results' arrival order"
    );
}

#[test]
fn rounds_are_traced_in_order() {
    let slice = [
        assistant_calling(&[("c1", "list_servers", "{}")]),
        ChatMessage::tool_result("c1", "mc — Ready"),
        assistant_calling(&[("c2", "read_logs", r#"{"name":"mc"}"#)]),
        ChatMessage::tool_result("c2", "[12:00] started"),
    ];
    let traced = tool_trace(&slice);
    assert_eq!(
        traced
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>(),
        vec!["list_servers", "read_logs"]
    );
    assert_eq!(rounds(&slice), 2);
}

#[test]
fn a_call_with_no_result_is_kept_with_an_empty_one() {
    // Dropping it would erase the evidence that a tool ran and never answered —
    // exactly the invisible failure the trace exists to surface.
    let slice = [assistant_calling(&[(
        "c1",
        "backup_server",
        r#"{"name":"mc"}"#,
    )])];
    assert_eq!(
        tool_trace(&slice),
        vec![trace("backup_server", r#"{"name":"mc"}"#, "")]
    );
}

#[test]
fn an_escalated_turn_reports_every_round_it_burned() {
    // The round budget runs out with no final text assistant message, so the
    // count has to come from the tool rounds themselves.
    let mut slice = Vec::new();
    for round in 0..crate::agent::DEFAULT_MAX_ROUNDS {
        let id = format!("c{round}");
        slice.push(assistant_calling(&[(&id, "read_logs", r#"{"name":"mc"}"#)]));
        slice.push(ChatMessage::tool_result(&id, "nothing useful"));
    }
    assert_eq!(rounds(&slice), crate::agent::DEFAULT_MAX_ROUNDS);
    assert_eq!(tool_trace(&slice).len(), crate::agent::DEFAULT_MAX_ROUNDS);
}

fn generation(model: &str, usage: Option<TokenUsage>, latency_ms: u64) -> Generation {
    Generation {
        model: model.to_owned(),
        usage,
        finish_reason: Some("stop".to_owned()),
        latency: Duration::from_millis(latency_ms),
    }
}

#[test]
fn each_model_call_is_reported_with_its_own_cost() {
    let generations = vec![
        generation(
            "glm-5.2",
            Some(TokenUsage {
                prompt: Some(900),
                completion: Some(40),
                total: Some(940),
                prompt_details: Some(PromptTokenDetails { cached: Some(700) }),
            }),
            1200,
        ),
        generation(
            "glm-5.2",
            Some(TokenUsage {
                prompt: Some(1100),
                completion: Some(12),
                total: Some(1112),
                prompt_details: Some(PromptTokenDetails { cached: Some(900) }),
            }),
            800,
        ),
    ];

    let traced: Vec<GenerationTrace> = generations.iter().map(generation_trace).collect();
    assert_eq!(
        traced.first().map(|trace| trace.prompt_tokens),
        Some(Some(900))
    );
    assert_eq!(
        traced.first().map(|trace| trace.cached_tokens),
        Some(Some(700)),
        "the cache split is what explains a turn costing less than its token count"
    );
    assert_eq!(traced.first().map(|trace| trace.latency_ms), Some(1200));
    assert_eq!(
        usage_totals(&generations),
        UsageTotals {
            prompt: Some(2000),
            cached: Some(1600),
            completion: Some(52),
            total: Some(2052),
        },
        "the turn's cost is the sum of its calls"
    );
}

#[test]
fn a_turn_the_provider_reported_nothing_for_totals_to_unknown() {
    let generations = vec![generation("glm-5.2", None, 700)];

    assert_eq!(
        usage_totals(&generations),
        UsageTotals::default(),
        "absent counts must stay absent — reporting zero would understate real spend"
    );
    assert_eq!(
        generations.iter().map(|g| millis(g.latency)).sum::<u64>(),
        700,
        "latency is measured here, so it's known even when the provider says nothing"
    );
}

#[test]
fn partial_reporting_sums_only_what_was_reported() {
    let generations = vec![
        generation("glm-5.2", None, 100),
        generation(
            "glm-5.2",
            Some(TokenUsage {
                prompt: Some(5),
                completion: None,
                total: None,
                prompt_details: None,
            }),
            100,
        ),
    ];

    assert_eq!(
        usage_totals(&generations),
        UsageTotals {
            prompt: Some(5),
            cached: None,
            completion: None,
            total: None,
        },
        "a count only one call reported is still a real count; the others stay unknown"
    );
}

#[test]
fn every_advertised_tool_is_reachable_by_name() {
    // The harness names tools by hand, so `known_tool` is what turns a typo into
    // an error instead of a confusing refusal from the dispatcher.
    for tool in available_tools(AccessLevel::Admin) {
        assert!(
            known_tool(&tool.function.name),
            "{} is advertised but wouldn't be accepted by name",
            tool.function.name
        );
    }
    assert!(!known_tool("list_serverz"), "a typo must not be accepted");
    assert!(!known_tool(""));
}

#[test]
fn tool_tiers_widen_with_access() {
    let names = |access| {
        available_tools(access)
            .into_iter()
            .map(|tool| tool.function.name)
            .collect::<Vec<_>>()
    };
    let read_only = names(AccessLevel::ReadOnly);
    let manager = names(AccessLevel::Manager);
    let admin = names(AccessLevel::Admin);

    assert!(
        read_only.iter().all(|tool| manager.contains(tool)),
        "a manager keeps every read-only tool"
    );
    assert!(
        manager.iter().all(|tool| admin.contains(tool)),
        "an admin keeps every manager tool"
    );
    assert!(
        !read_only.contains(&"destroy_server".to_owned()),
        "read-only must never be offered a destructive tool"
    );
    assert!(admin.contains(&"destroy_server".to_owned()));
}

fn profile(provider: &str, model: &str, max_tokens: Option<u32>) -> ModelProfile {
    ModelProfile {
        provider: provider.to_owned(),
        model: model.to_owned(),
        max_tokens,
    }
}

fn settings_connection() -> OllamaConfig {
    OllamaConfig {
        api_key: "ollama-key".to_owned(),
        base_url: "https://ollama.example/v1".to_owned(),
        model: "settings-model".to_owned(),
        max_tokens: None,
    }
}

#[test]
fn an_absent_model_profile_keeps_the_settings_model() {
    let resolved = resolve_connection(None, Some(&settings_connection()), Some("fw-key")).unwrap();
    assert!(resolved.is_none(), "no profile means the default path");
}

#[test]
fn fireworks_without_a_key_is_refused_naming_the_key() {
    let err = resolve_connection(Some(&profile("fireworks", "m", None)), None, None).unwrap_err();
    assert!(reason(err).contains("FIREWORKS_API_KEY"));
}

#[test]
fn fireworks_uses_its_endpoint_key_and_token_ceiling() {
    let resolved = resolve_connection(
        Some(&profile(
            "fireworks",
            "accounts/fireworks/models/x",
            Some(2048),
        )),
        None,
        Some("fw-key"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(resolved.api_key, "fw-key");
    assert_eq!(resolved.base_url, FIREWORKS_BASE_URL);
    assert_eq!(resolved.model, "accounts/fireworks/models/x");
    assert_eq!(resolved.max_tokens, Some(2048));
}

#[test]
fn ollama_profile_reuses_the_settings_key_and_endpoint_with_its_own_model() {
    let resolved = resolve_connection(
        Some(&profile("ollama", "other-model", None)),
        Some(&settings_connection()),
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(resolved.api_key, "ollama-key");
    assert_eq!(resolved.base_url, "https://ollama.example/v1");
    assert_eq!(resolved.model, "other-model");
}

#[test]
fn unknown_providers_and_empty_models_are_bad_requests() {
    let unknown = resolve_connection(Some(&profile("openai", "m", None)), None, Some("k"));
    assert!(reason(unknown.unwrap_err()).contains("openai"));
    let empty = resolve_connection(Some(&profile("fireworks", "  ", None)), None, Some("k"));
    assert!(reason(empty.unwrap_err()).contains("model"));
}

#[test]
fn gary_request_defaults_to_memory_on_and_no_profile() {
    let body = r#"{"prompt": "hi", "channel": "1"}"#;
    let request: GaryRequest = serde_json::from_str(body).unwrap();
    assert!(request.memory);
    assert!(request.model_profile.is_none());
}

#[test]
fn gary_request_parses_profile_and_memory_off() {
    let body = r#"{"prompt": "hi", "channel": "1", "memory": false,
        "model_profile": {"provider": "fireworks", "model": "m", "max_tokens": 512}}"#;
    let request: GaryRequest = serde_json::from_str(body).unwrap();
    assert!(!request.memory);
    let profile = request.model_profile.unwrap();
    assert_eq!(profile.provider, "fireworks");
    assert_eq!(profile.max_tokens, Some(512));
}

#[test]
fn turn_transcript_keeps_system_user_and_everything_after_in_order() {
    let mut reasoning_turn = assistant_calling(&[("c1", "list_servers", "{}")]);
    reasoning_turn.reasoning_content = Some("check the servers".to_owned());
    let mut closing = ChatMessage::assistant("mc is up.");
    closing.reasoning_content = Some("all good".to_owned());
    let messages = vec![
        ChatMessage::system("you are gary"),
        ChatMessage::user("earlier question"),
        ChatMessage::assistant("earlier answer"),
        ChatMessage::user("how are the servers?"),
        reasoning_turn,
        ChatMessage::tool_result("c1", "mc - Ready"),
        closing,
    ];

    let transcript = turn_transcript(&messages, 4);

    let roles: Vec<Role> = transcript.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [
            Role::System,
            Role::User,
            Role::Assistant,
            Role::Tool,
            Role::Assistant
        ],
        "prior-turn history is left out"
    );
    let wire = serde_json::to_value(&transcript).unwrap();
    assert_eq!(
        wire.pointer("/2/reasoning_content").unwrap(),
        "check the servers"
    );
    assert_eq!(
        wire.pointer("/2/tool_calls/0/function/name").unwrap(),
        "list_servers"
    );
    assert_eq!(wire.pointer("/3/tool_call_id").unwrap(), "c1");
    assert_eq!(wire.pointer("/3/role").unwrap(), "tool");
}
