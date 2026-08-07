use super::*;

fn submission() -> Submission {
    Submission {
        email: "friend@example.com".to_owned(),
        display_email: "Friend@Example.com".to_owned(),
        wants_beta: true,
        wants_updates: false,
        note: None,
    }
}

/// The whole degradation contract in one place: with no database the queue is
/// inert but never panics and never claims to have stored anything.
#[tokio::test]
async fn a_disabled_queue_records_nothing_and_shows_nothing() {
    let signups = Signups::connect(None).await;

    assert!(!signups.is_enabled());
    assert_eq!(
        signups.submit(&submission()).await,
        SubmitOutcome::Unavailable
    );
    assert!(signups.pending_beta(10).await.is_empty());
    assert!(signups.recent_decisions(10).await.is_empty());
    assert_eq!(signups.updates_count().await, 0);
    assert_eq!(signups.submissions_last_24h().await, 0);
}

#[test]
fn an_address_is_stored_trimmed_and_lowercased() {
    assert_eq!(
        normalize_email("  Friend@Example.COM  ").as_deref(),
        Some("friend@example.com")
    );
}

/// Normalizing twice must equal normalizing once, or the primary key drifts from
/// what a later lookup computes.
#[test]
fn normalizing_is_idempotent() {
    let once = normalize_email(" Person@Mail.Example ").expect("a valid address");
    let twice = normalize_email(&once).expect("a normalized address is still valid");
    assert_eq!(once, twice);
}

#[test]
fn addresses_that_could_not_receive_mail_are_refused() {
    for raw in [
        "",
        "   ",
        "no-at-sign.example.com",
        "@example.com",
        "friend@",
        "friend@nodot",
        "two@at@example.com",
        "friend @example.com",
        "friend\u{7}@example.com",
        "friend\n@example.com",
    ] {
        assert!(
            normalize_email(raw).is_none(),
            "expected {raw:?} to be refused"
        );
    }
}

#[test]
fn an_address_longer_than_the_rfc_limit_is_refused() {
    let local = "a".repeat(MAX_EMAIL_LEN);
    assert!(normalize_email(&format!("{local}@example.com")).is_none());
}

#[test]
fn a_note_shorter_than_the_limit_is_left_alone() {
    assert_eq!(truncate_chars("hello", 10), "hello");
}

#[test]
fn a_note_exactly_at_the_limit_is_not_marked_truncated() {
    assert_eq!(truncate_chars("hello", 5), "hello");
}

#[test]
fn a_note_one_over_the_limit_is_cut_and_marked() {
    assert_eq!(truncate_chars("hello!", 5), "hello…");
}

/// Counting characters rather than bytes is the whole point — slicing this at
/// byte 5 would split a multi-byte character and panic.
#[test]
fn a_multi_byte_character_at_the_boundary_does_not_split() {
    assert_eq!(truncate_chars("aaaa🐻bbbb", 5), "aaaa🐻…");
    assert_eq!(truncate_chars("héllo wörld", 7), "héllo w…");
}

#[tokio::test]
async fn a_disabled_queue_cannot_decide_anything() {
    let signups = Signups::connect(None).await;
    assert_eq!(
        signups
            .decide("friend@example.com", SignupStatus::Approved, "bear")
            .await,
        SignupDecision::Unavailable
    );
    assert!(signups.approved_not_emailed(10).await.is_empty());
}

#[test]
fn signup_statuses_render_as_the_stored_vocabulary() {
    assert_eq!(SignupStatus::Approved.as_str(), "approved");
    assert_eq!(SignupStatus::Denied.as_str(), "denied");
}
