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
        reasoning_content: None,
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
    assert!(notice.contains("he called no tools"));
}

#[test]
fn render_agent_requested_escalation_leads_with_garys_own_reason() {
    let notice = render_escalation(&Escalation::AgentRequested {
        context: EscalationContext::Discord {
            asker: "Alice (<@42>)".to_owned(),
            jump_link: "https://discord.com/channels/1/2/3".to_owned(),
            guild: Some(1),
        },
        request: "minecraft won't start".to_owned(),
        reason: "the world folder is missing and I can't tell which backup is right".to_owned(),
        server: Some("mc-summer".to_owned()),
        attempts: vec!["read_logs".to_owned(), "browse_files".to_owned()],
    });
    assert!(notice.contains("the world folder is missing"));
    assert!(notice.contains("Alice (<@42>)"));
    assert!(
        notice.contains("mc-summer"),
        "the server Gary named must be findable without reading the prose"
    );
    assert!(notice.contains("read_logs → browse_files"));
    // The whole point of this variant: it must not claim a budget was spent,
    // because Gary stopped while he still had rounds left.
    assert!(
        !notice.contains("rounds"),
        "a deliberate handoff didn't exhaust anything: {notice}"
    );
}

#[test]
fn an_escalation_about_no_particular_server_omits_the_server_line() {
    // Plenty of asks aren't about one server ("why is everything slow?"). A blank
    // Server line would read as a lookup that came back empty.
    let notice = render_escalation(&Escalation::AgentRequested {
        context: EscalationContext::Discord {
            asker: "Alice".to_owned(),
            jump_link: "https://discord.com/channels/1/2/3".to_owned(),
            guild: Some(1),
        },
        request: "why is everything slow".to_owned(),
        reason: "nothing in the cluster looks wrong from here".to_owned(),
        server: None,
        attempts: Vec::new(),
    });
    assert!(!notice.contains("Server:"));
}

#[test]
fn the_two_turn_escalations_are_built_from_the_loops_own_reason() {
    let context = || EscalationContext::Deferred {
        server: "mc-summer".to_owned(),
        condition: "empty".to_owned(),
        guild: Some(7),
    };
    let requested = Escalation::from_turn(
        &EscalationReason::AgentRequested {
            reason: "needs a human".to_owned(),
            server: None,
        },
        context(),
        "tune the difficulty".to_owned(),
        Vec::new(),
    );
    assert!(matches!(requested, Escalation::AgentRequested { .. }));

    let exhausted = Escalation::from_turn(
        &EscalationReason::RoundBudget,
        context(),
        "tune the difficulty".to_owned(),
        Vec::new(),
    );
    // The budget shape is the only one that may state a round count, and it must
    // be the real ceiling rather than something the caller happened to have.
    match exhausted {
        Escalation::RoundBudgetExhausted { rounds, .. } => {
            assert_eq!(rounds, crate::agent::DEFAULT_MAX_ROUNDS);
        }
        Escalation::AgentRequested { .. } | Escalation::CrashRollback { .. } => {
            panic!("the round-budget reason must build the round-budget notice")
        }
    }
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
        attempts: vec!["about_server".to_owned()],
        rounds: 16,
    });
    assert!(notice.contains("in-game chat"));
    assert!(notice.contains("server `mc-summer`"));
    assert!(notice.contains("guild `1234`"));
    assert!(notice.contains("player Steve"));
    assert!(notice.contains("about_server"));
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

#[test]
fn render_watchdog_escalation_names_the_server_signal_and_no_asker() {
    let notice = render_escalation(&Escalation::RoundBudgetExhausted {
        context: EscalationContext::Watchdog {
            server: "mc-summer".to_owned(),
            signal: "crash".to_owned(),
            guild: Some(42),
        },
        request: "investigate the crash loop".to_owned(),
        attempts: vec!["read_logs".to_owned()],
        rounds: 16,
    });
    assert!(notice.contains("mc-summer"));
    assert!(notice.contains("`crash`"));
    assert!(notice.contains("guild `42`"));
    assert!(
        notice.contains("nobody"),
        "the who-line must say no one asked: {notice}"
    );
    assert!(notice.contains("read_logs"));
}
