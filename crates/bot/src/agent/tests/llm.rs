use super::*;

#[test]
fn serializes_user_message_without_optional_fields() {
    let value = serde_json::to_value(ChatMessage::user("hi")).unwrap();
    assert_eq!(
        value,
        serde_json::json!({ "role": "user", "content": "hi" }),
        "absent tool_calls / tool_call_id must be omitted, not serialized as null"
    );
}

#[test]
fn tool_result_message_round_trips() {
    let msg = ChatMessage::tool_result("call_1", "paused mc-abc12");
    let value = serde_json::to_value(&msg).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "role": "tool",
            "content": "paused mc-abc12",
            "tool_call_id": "call_1"
        })
    );

    let back: ChatMessage = serde_json::from_value(value).unwrap();
    assert_eq!(back.role, Role::Tool);
    assert_eq!(back.tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(back.content.as_deref(), Some("paused mc-abc12"));
}

#[test]
fn parses_assistant_message_with_tool_calls() {
    let raw = serde_json::json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "id": "call_abc",
            "type": "function",
            "function": { "name": "list_servers", "arguments": "{}" }
        }]
    });
    let msg: ChatMessage = serde_json::from_value(raw).unwrap();

    let calls = msg.requested_tool_calls().unwrap();
    assert_eq!(calls.len(), 1);
    let call = calls.first().unwrap();
    assert_eq!(call.id, "call_abc");
    assert_eq!(call.function.name, "list_servers");
    assert_eq!(call.function.arguments, "{}");
}

#[test]
fn empty_tool_calls_reads_as_no_tool_calls() {
    let msg = ChatMessage {
        role: Role::Assistant,
        content: Some("all set".to_owned()),
        tool_calls: Some(Vec::new()),
        tool_call_id: None,
    };
    assert!(
        msg.requested_tool_calls().is_none(),
        "an empty tool_calls array is a text turn, not a tool turn"
    );
}

#[test]
fn parses_the_usage_and_finish_reason_the_provider_sends() {
    let raw = serde_json::json!({
        "model": "glm-5.2-turbo",
        "choices": [{
            "message": { "role": "assistant", "content": "done" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1234, "completion_tokens": 56, "total_tokens": 1290 }
    });
    let parsed: ChatResponse = serde_json::from_value(raw).unwrap();

    assert_eq!(parsed.model.as_deref(), Some("glm-5.2-turbo"));
    assert_eq!(
        parsed.usage,
        Some(TokenUsage {
            prompt: Some(1234),
            completion: Some(56),
            total: Some(1290),
        }),
        "token usage is the cost signal; it must survive deserialization"
    );
    assert_eq!(
        parsed
            .choices
            .first()
            .and_then(|choice| choice.finish_reason.as_deref()),
        Some("stop")
    );
}

#[test]
fn a_provider_that_reports_nothing_reads_as_unknown_not_zero() {
    let raw = serde_json::json!({
        "choices": [{ "message": { "role": "assistant", "content": "done" } }]
    });
    let parsed: ChatResponse = serde_json::from_value(raw).unwrap();

    assert!(
        parsed.usage.is_none(),
        "absent usage must stay absent — a zero here would understate real cost"
    );
    assert!(
        parsed.model.is_none(),
        "the caller falls back to its config"
    );
    assert!(
        parsed
            .choices
            .first()
            .is_some_and(|choice| choice.finish_reason.is_none())
    );
}

#[test]
fn partial_usage_keeps_the_counts_it_was_given() {
    let raw = serde_json::json!({ "prompt_tokens": 40 });
    let usage: TokenUsage = serde_json::from_value(raw).unwrap();

    assert_eq!(usage.prompt, Some(40));
    assert_eq!(usage.completion, None);
    assert_eq!(usage.total, None);
}

#[test]
fn tool_def_serializes_as_openai_function() {
    let def = ToolDef::function(
        "stop_server",
        "pause a server",
        serde_json::json!({ "type": "object", "properties": {} }),
    );
    let value = serde_json::to_value(&def).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "stop_server",
                "description": "pause a server",
                "parameters": { "type": "object", "properties": {} }
            }
        })
    );
}
