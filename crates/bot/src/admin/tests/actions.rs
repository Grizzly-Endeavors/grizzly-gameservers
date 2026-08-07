use super::*;

#[test]
fn every_action_round_trips_through_its_path_segment() {
    for action in [
        InstanceAction::Stop,
        InstanceAction::Start,
        InstanceAction::Restart,
        InstanceAction::Shutdown,
        InstanceAction::Destroy,
        InstanceAction::Backup,
    ] {
        assert_eq!(InstanceAction::parse(action.as_str()), Some(action));
    }
    assert_eq!(InstanceAction::parse("delete"), None);
    assert_eq!(InstanceAction::parse(""), None);
}

/// Destroy is the only action that loses a world; everything else is undone by
/// doing the opposite. Widening this set silently would put a typed
/// confirmation in front of routine work until people learned to ignore it.
#[test]
fn only_destroy_demands_a_typed_confirmation() {
    let destructive: Vec<&str> = [
        InstanceAction::Stop,
        InstanceAction::Start,
        InstanceAction::Restart,
        InstanceAction::Shutdown,
        InstanceAction::Destroy,
        InstanceAction::Backup,
    ]
    .into_iter()
    .filter(|action| action.is_destructive())
    .map(InstanceAction::as_str)
    .collect();
    assert_eq!(destructive, ["destroy"]);
}

#[test]
fn a_blank_cap_clears_the_override_back_to_the_global_default() {
    for field in ["servers", "cpu", "memory"] {
        let change = parse_limit_change(field, "", true).expect("a blank value clears");
        let cleared = matches!(
            change,
            LimitChange::Servers(None)
                | LimitChange::CpuLimitMillicores(None)
                | LimitChange::MemLimitBytes(None)
        );
        assert!(cleared, "{field} did not clear");
    }
}

#[test]
fn caps_accept_the_same_grammar_the_discord_command_does() {
    assert_eq!(
        parse_limit_change("servers", "8", false).expect("a count parses"),
        LimitChange::Servers(Some(8))
    );
    assert_eq!(
        parse_limit_change("cpu", "32", false).expect("bare cores parse"),
        LimitChange::CpuLimitMillicores(Some(32_000))
    );
    assert_eq!(
        parse_limit_change("cpu", "1500m", false).expect("millicores parse"),
        LimitChange::CpuLimitMillicores(Some(1_500))
    );
    assert_eq!(
        parse_limit_change("memory", "64Gi", false).expect("a binary suffix parses"),
        LimitChange::MemLimitBytes(Some(64 * 1024 * 1024 * 1024))
    );
}

#[test]
fn an_unusable_cap_is_refused_with_a_message_naming_the_value() {
    let count = parse_limit_change("servers", "lots", false).expect_err("refused");
    assert!(count.contains("lots"), "{count}");
    let cpu = parse_limit_change("cpu", "-4", false).expect_err("refused");
    assert!(cpu.contains("-4"), "{cpu}");
    // Zero would cap a guild at nothing, which reads as "broken" rather than
    // "restricted".
    assert!(parse_limit_change("servers", "0", false).is_err());
    assert!(parse_limit_change("memory", "0", false).is_err());
}

#[test]
fn an_unknown_cap_field_is_refused_rather_than_guessed_at() {
    let err = parse_limit_change("disk", "10Gi", false).expect_err("refused");
    assert!(err.contains("disk"), "{err}");
}

#[test]
fn signup_actions_round_trip_through_their_path_segment() {
    for action in [
        SignupAction::Approve,
        SignupAction::Deny,
        SignupAction::Resend,
    ] {
        assert_eq!(SignupAction::parse(action.as_str()), Some(action));
    }
}

#[test]
fn an_unknown_signup_verb_is_refused() {
    assert_eq!(SignupAction::parse("delete"), None);
    assert_eq!(SignupAction::parse(""), None);
}

/// Only deny is irreversible. Widening this set would train the operator to
/// click through confirmations, which is how the one that matters stops working.
#[test]
fn only_deny_demands_a_typed_confirmation() {
    assert!(SignupAction::Deny.is_destructive());
    assert!(!SignupAction::Approve.is_destructive());
    assert!(!SignupAction::Resend.is_destructive());
}
