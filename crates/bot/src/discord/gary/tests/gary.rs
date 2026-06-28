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
    let prompt = build_system_prompt(true, "minecraft, valheim");
    assert!(prompt.contains("admin"));
    assert!(
        prompt.contains("confirm"),
        "destructive guardrail must be stated"
    );
    assert!(prompt.contains("minecraft") && prompt.contains("valheim"));
}

#[test]
fn read_only_prompt_scopes_to_lookups() {
    let prompt = build_system_prompt(false, "minecraft");
    assert!(prompt.contains("cannot"), "non-admin can't mutate");
    assert!(prompt.contains("admin has to"));
}

#[test]
fn empty_catalog_renders_as_none() {
    let prompt = build_system_prompt(false, "");
    assert!(prompt.contains("(none)"));
}
