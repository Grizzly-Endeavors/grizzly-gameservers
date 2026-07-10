use super::*;
use crate::agent::{ChatMessage, ToolCall};

/// Build an assistant message that requests the named tools, mirroring the wire
/// shape `summarize_attempts` walks (an assistant turn carrying `tool_calls`).
fn assistant_calling(names: &[&str]) -> ChatMessage {
    let calls = names
        .iter()
        .enumerate()
        .map(|(index, name)| ToolCall {
            id: format!("call_{index}"),
            kind: "function".to_owned(),
            function: crate::agent::llm::FunctionCall {
                name: (*name).to_owned(),
                arguments: "{}".to_owned(),
            },
        })
        .collect();
    ChatMessage {
        role: Role::Assistant,
        content: None,
        tool_calls: Some(calls),
        tool_call_id: None,
    }
}

#[test]
fn summarize_attempts_lists_tool_calls_in_order() {
    let messages = vec![
        ChatMessage::system("prompt"),
        ChatMessage::user("restart minecraft"),
        assistant_calling(&["list_servers"]),
        ChatMessage::tool_result("call_0", "ok"),
        assistant_calling(&["read_logs", "restart_server"]),
        ChatMessage::tool_result("call_0", "ok"),
    ];
    assert_eq!(
        summarize_attempts(&messages),
        vec!["list_servers", "read_logs", "restart_server"]
    );
}

#[test]
fn summarize_attempts_scopes_to_the_latest_user_turn() {
    // A continued conversation: the prior turn's tool calls must not leak into
    // this ask's attempt summary — only calls after the last user message count.
    let messages = vec![
        ChatMessage::system("prompt"),
        ChatMessage::user("first ask"),
        assistant_calling(&["list_servers"]),
        ChatMessage::tool_result("call_0", "ok"),
        ChatMessage::user("second ask"),
        assistant_calling(&["read_file"]),
        ChatMessage::tool_result("call_0", "ok"),
    ];
    assert_eq!(summarize_attempts(&messages), vec!["read_file"]);
}

#[test]
fn summarize_attempts_is_empty_when_no_tools_were_called() {
    let messages = vec![
        ChatMessage::system("prompt"),
        ChatMessage::user("just chatting"),
    ];
    assert!(summarize_attempts(&messages).is_empty());
}

#[test]
fn render_discord_escalation_includes_link_asker_and_attempts() {
    let notice = render_escalation(&Escalation::RoundBudgetExhausted {
        context: EscalationContext::Discord {
            asker: "Alice (<@42>)".to_owned(),
            jump_link: "https://discord.com/channels/1/2/3".to_owned(),
            guild: Some(1),
        },
        request: "restart minecraft".to_owned(),
        attempts: vec!["list_servers".to_owned(), "restart_server".to_owned()],
        rounds: 16,
    });
    assert!(notice.contains("guild `1`"));
    assert!(notice.contains("https://discord.com/channels/1/2/3"));
    assert!(notice.contains("Alice (<@42>)"));
    assert!(notice.contains("restart minecraft"));
    assert!(notice.contains("list_servers → restart_server"));
    assert!(notice.contains("16 rounds"));
}

#[test]
fn render_discord_dm_escalation_names_the_dm_not_a_guild() {
    let notice = render_escalation(&Escalation::RoundBudgetExhausted {
        context: EscalationContext::Discord {
            asker: "Bob".to_owned(),
            jump_link: "https://discord.com/channels/@me/9/8".to_owned(),
            guild: None,
        },
        request: "help".to_owned(),
        attempts: Vec::new(),
        rounds: 16,
    });
    assert!(notice.contains("a direct message"));
    assert!(!notice.contains("guild `"));
    // No tools were called before the give-up — say so rather than an empty list.
    assert!(notice.contains("gave up before calling any tools"));
}

#[test]
fn render_ingame_escalation_names_the_server_and_player() {
    let notice = render_escalation(&Escalation::RoundBudgetExhausted {
        context: EscalationContext::InGame {
            player: "Steve".to_owned(),
            server: "mc-summer".to_owned(),
            guild: "1234".to_owned(),
        },
        request: "why does it keep crashing".to_owned(),
        attempts: vec!["server_status".to_owned()],
        rounds: 16,
    });
    assert!(notice.contains("in-game chat"));
    assert!(notice.contains("server `mc-summer`"));
    assert!(notice.contains("guild `1234`"));
    assert!(notice.contains("player Steve"));
    assert!(notice.contains("server_status"));
}

#[test]
fn render_crash_rollback_escalation_names_the_server_and_path() {
    let notice = render_escalation(&Escalation::CrashRollback {
        server: "mc-summer".to_owned(),
        path: "server.properties".to_owned(),
    });
    assert!(notice.contains("mc-summer"));
    assert!(notice.contains("server.properties"));
    // No asker/attempts/rounds exist for this variant — the wording must not
    // imply a user request or a round budget that was never spent.
    assert!(!notice.contains("rounds"));
    assert!(!notice.contains("asked"));
}
