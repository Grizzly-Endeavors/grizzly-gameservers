use poise::serenity_prelude as serenity;

use super::*;

#[test]
fn strips_plain_mention_to_bare_prompt() {
    let bot = serenity::UserId::new(111);
    assert_eq!(
        extract_prompt("<@111> restart minecraft", bot),
        "restart minecraft"
    );
}

#[test]
fn strips_nickname_mention_form() {
    let bot = serenity::UserId::new(111);
    assert_eq!(
        extract_prompt("<@!111>   list the servers ", bot),
        "list the servers"
    );
}

#[test]
fn leaves_content_without_the_bot_mention_untouched() {
    let bot = serenity::UserId::new(111);
    // A mention of a different user is not stripped.
    assert_eq!(extract_prompt("<@222> hello", bot), "<@222> hello");
}

#[test]
fn empty_after_stripping_is_empty() {
    let bot = serenity::UserId::new(111);
    assert!(extract_prompt("<@111>", bot).is_empty());
}

#[test]
fn admin_prompt_describes_mutations_and_confirmation() {
    let prompt = build_system_prompt(AccessLevel::Admin, "minecraft, valheim");
    assert!(prompt.contains("admin"));
    assert!(
        prompt.contains("confirm"),
        "destructive guardrail must be stated"
    );
    assert!(
        prompt.contains("send_command"),
        "admins get the console-command tool"
    );
    assert!(prompt.contains("minecraft") && prompt.contains("valheim"));
}

#[test]
fn manager_prompt_grants_lifecycle_and_files_but_reserves_destruction() {
    let prompt = build_system_prompt(AccessLevel::Manager, "minecraft");
    assert!(prompt.contains("day-to-day"), "manager can operate servers");
    assert!(
        prompt.contains("edit_file"),
        "manager gets the file-editing troubleshooting tools"
    );
    assert!(
        prompt.contains("reserved for admins"),
        "manager must be told destructive actions need an admin"
    );
    assert!(
        !prompt.contains("send_command"),
        "manager must not be offered console commands"
    );
}

#[test]
fn read_only_prompt_scopes_to_lookups() {
    let prompt = build_system_prompt(AccessLevel::ReadOnly, "minecraft");
    assert!(prompt.contains("cannot"), "read-only caller can't mutate");
    assert!(prompt.contains("manager or admin has to"));
}

#[test]
fn empty_catalog_renders_as_none() {
    let prompt = build_system_prompt(AccessLevel::ReadOnly, "");
    assert!(prompt.contains("(none)"));
}

#[test]
fn auto_listen_answers_a_real_request() {
    assert!(is_auto_listen_prompt("restart minecraft"));
    assert!(is_auto_listen_prompt("  what servers are up?  "));
}

#[test]
fn auto_listen_ignores_blank_and_slash_command_lines() {
    assert!(!is_auto_listen_prompt(""));
    assert!(!is_auto_listen_prompt("   "));
    // A typed slash-command-style line — Gary must not answer it in a home channel.
    assert!(!is_auto_listen_prompt("/servers"));
    assert!(!is_auto_listen_prompt("  /gary-home"));
}
