use super::*;

fn memory(id: i64, scope: &str, content: &str) -> Memory {
    Memory {
        id,
        scope: scope.to_owned(),
        content: content.to_owned(),
    }
}

#[test]
fn render_empty_is_blank() {
    assert_eq!(render_memories(&[]), "");
}

#[test]
fn render_single_scope_includes_ids() {
    let memories = vec![
        memory(1, "palworld", "soft-stop before editing configs"),
        memory(2, "palworld", "settings live in PalWorldSettings.ini"),
    ];
    let rendered = render_memories(&memories);
    assert!(rendered.starts_with("palworld:\n"));
    assert!(rendered.contains("  - #1: soft-stop before editing configs"));
    assert!(rendered.contains("  - #2: settings live in PalWorldSettings.ini"));
}

#[test]
fn render_groups_scopes_in_name_order() {
    let memories = vec![
        memory(1, "palworld", "a palworld fact"),
        memory(2, "general", "a general fact"),
    ];
    let rendered = render_memories(&memories);
    let general_at = rendered.find("general:").expect("general group present");
    let palworld_at = rendered.find("palworld:").expect("palworld group present");
    // BTreeMap orders scopes alphabetically, so general precedes palworld.
    assert!(general_at < palworld_at);
}

#[test]
fn render_caps_at_most_recent() {
    // 45 facts in one scope; only the newest MAX_RENDERED (40) should render, so
    // ids 6..=45 survive and #5 falls off.
    let memories: Vec<Memory> = (1..=45)
        .map(|id| memory(id, "minecraft", &format!("fact {id}")))
        .collect();
    let rendered = render_memories(&memories);
    let line_count = rendered
        .lines()
        .filter(|line| line.trim_start().starts_with("- #"))
        .count();
    assert_eq!(line_count, MAX_RENDERED);
    assert!(rendered.contains("#45: fact 45"));
    assert!(rendered.contains("#6: fact 6"));
    assert!(!rendered.contains("#5: fact 5"));
}

#[test]
fn normalize_scope_accepts_known_game_and_general() {
    let ids = ["minecraft", "palworld"];
    assert_eq!(
        normalize_scope("palworld", &ids),
        Some("palworld".to_owned())
    );
    assert_eq!(
        normalize_scope(GENERAL_SCOPE, &ids),
        Some(GENERAL_SCOPE.to_owned())
    );
}

#[test]
fn normalize_scope_lowercases_and_trims() {
    let ids = ["palworld"];
    assert_eq!(
        normalize_scope("  PalWorld ", &ids),
        Some("palworld".to_owned())
    );
}

#[test]
fn normalize_scope_rejects_unknown() {
    let ids = ["minecraft"];
    assert_eq!(normalize_scope("valheim", &ids), None);
    assert_eq!(normalize_scope("", &ids), None);
}

#[tokio::test]
async fn an_over_long_note_is_refused_before_the_store_is_consulted() {
    // The cap answers the same way whether or not Postgres is up: an over-long
    // note is invalid on its own terms, and reporting it as "memory offline"
    // would send the model off retrying a save that will never be accepted.
    let memory = GaryMemory::disabled();
    let long = "x".repeat(MAX_NOTE_CHARS + 1);
    assert!(matches!(
        memory.remember("palworld", &long, None).await,
        Ok(RememberOutcome::TooLong { max }) if max == MAX_NOTE_CHARS
    ));
}

#[tokio::test]
async fn a_note_at_the_cap_is_not_refused_for_length() {
    // Exactly at the cap is allowed — the disabled store is what stops it here,
    // which is how we know length wasn't the reason.
    let memory = GaryMemory::disabled();
    let at_cap = "x".repeat(MAX_NOTE_CHARS);
    assert!(matches!(
        memory.remember("palworld", &at_cap, None).await,
        Ok(RememberOutcome::Unavailable)
    ));
}

#[tokio::test]
async fn the_cap_counts_characters_not_bytes() {
    // Counting bytes would reject an all-emoji note at roughly a third of the
    // documented limit, which the caller has no way to make sense of.
    let memory = GaryMemory::disabled();
    let wide = "🎮".repeat(MAX_NOTE_CHARS);
    assert!(matches!(
        memory.remember("palworld", &wide, None).await,
        Ok(RememberOutcome::Unavailable)
    ));
}

#[tokio::test]
async fn disabled_memory_saves_nothing_and_renders_blank() {
    let memory = GaryMemory::disabled();
    assert!(matches!(
        memory.remember("palworld", "a fact", None).await,
        Ok(RememberOutcome::Unavailable)
    ));
    assert!(matches!(
        memory.forget(1).await,
        Ok(ForgetOutcome::Unavailable)
    ));
    assert_eq!(memory.render_for_prompt().await, "");
    assert!(memory.all().await.is_empty());
}
