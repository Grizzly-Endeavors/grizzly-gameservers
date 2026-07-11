use super::*;

fn task(text: &str) -> DeferredTask {
    DeferredTask::new(text, 1, 2, Some(3))
}

#[test]
fn wait_key_round_trips_for_every_condition() {
    for condition in [Condition::Startup, Condition::Empty, Condition::Idle] {
        let key = wait_key("minecraft-home-ab12", condition);
        assert_eq!(
            parse_wait_key(&key),
            Some(("minecraft-home-ab12".to_owned(), condition)),
            "key {key} should parse back to its server and condition"
        );
    }
}

#[test]
fn wait_key_uses_app_prefix() {
    // The shared kv-cache isolates by prefix; a bare `wait:` key would collide.
    assert!(wait_key("mc", Condition::Empty).starts_with("gameservers:wait:"));
}

#[test]
fn parse_wait_key_rejects_malformed() {
    assert_eq!(parse_wait_key("something:else"), None, "wrong prefix");
    assert_eq!(
        parse_wait_key("gameservers:wait:mc:bogus"),
        None,
        "unknown condition"
    );
    assert_eq!(
        parse_wait_key("gameservers:wait::empty"),
        None,
        "empty server"
    );
    assert_eq!(
        parse_wait_key("gameservers:wait:mc"),
        None,
        "no condition segment"
    );
}

#[test]
fn condition_serializes_lowercase() {
    assert_eq!(
        serde_json::to_string(&Condition::Startup).unwrap(),
        "\"startup\""
    );
    assert_eq!(
        serde_json::from_str::<Condition>("\"idle\"").unwrap(),
        Condition::Idle
    );
}

#[test]
fn empty_streak_starts_and_continues_while_empty() {
    // No streak yet + an empty reading starts one anchored at `now`.
    assert_eq!(next_empty_since(None, Some(0), 100_u64), Some(100));
    // An ongoing streak is preserved (anchored at its original start), not reset
    // to `now`, so the grace window measures from when it first went empty.
    assert_eq!(next_empty_since(Some(100_u64), Some(0), 200), Some(100));
}

#[test]
fn empty_streak_resets_on_players_or_unknown() {
    // A live player breaks the streak.
    assert_eq!(next_empty_since(Some(100_u64), Some(3), 200), None);
    // An unknown count also breaks it — we never treat "can't tell" as empty.
    assert_eq!(next_empty_since(Some(100_u64), None, 200), None);
    // And a fresh unknown never starts one.
    assert_eq!(next_empty_since(None, None, 200_u64), None);
}

#[test]
fn batch_prompt_names_server_and_numbers_tasks() {
    let tasks = [task("set difficulty to hard"), task("bump view distance")];
    let prompt = compose_batch_prompt("minecraft-home", "is now empty", &tasks);
    assert!(prompt.contains("minecraft-home"), "names the server");
    assert!(prompt.contains("is now empty"), "carries the trigger note");
    assert!(
        prompt.contains("1. set difficulty to hard"),
        "numbers the first task"
    );
    assert!(
        prompt.contains("2. bump view distance"),
        "numbers the second task"
    );
    assert!(
        prompt.contains("post a short"),
        "instructs Gary to report back to the channel"
    );
}
