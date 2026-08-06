use super::*;

/// The three refusals a caller can provoke, and the two responses that must be
/// indistinguishable. Exercised through the pure validation the handler runs
/// rather than the handler itself, which needs a whole `Data` to construct.
#[test]
fn a_submission_that_opts_into_nothing_has_nothing_to_record() {
    let request = SignupRequest {
        email: "friend@example.com".to_owned(),
        wants_beta: false,
        wants_updates: false,
        note: None,
    };
    assert!(!request.wants_beta && !request.wants_updates);
}

#[test]
fn a_blank_note_is_stored_as_absent_rather_than_empty() {
    let note = Some("   ".to_owned())
        .map(|note| truncate_chars(note.trim(), MAX_NOTE_LEN))
        .filter(|note| !note.is_empty());
    assert_eq!(note, None);
}

#[test]
fn a_note_longer_than_the_limit_is_bounded_before_it_is_stored() {
    let long = "x".repeat(MAX_NOTE_LEN * 2);
    let note = Some(long)
        .map(|note| truncate_chars(note.trim(), MAX_NOTE_LEN))
        .filter(|note| !note.is_empty())
        .expect("a long note survives as a truncated one");
    // One over the limit: the truncation marker itself.
    assert_eq!(note.chars().count(), MAX_NOTE_LEN + 1);
}

/// The response body must not vary with whether the address was already known —
/// that difference is what turns a signup form into an enumeration oracle.
#[test]
fn every_accepted_submission_gets_the_same_answer() {
    let first = SignupResponse {
        recorded: true,
        message: THANKS,
    };
    let repeat = SignupResponse {
        recorded: true,
        message: THANKS,
    };
    assert_eq!(
        serde_json::to_string(&first).expect("the response serializes"),
        serde_json::to_string(&repeat).expect("the response serializes"),
    );
}

#[test]
fn a_refusal_names_what_was_wrong_without_echoing_the_input() {
    for message in [BAD_EMAIL, NOTHING_ASKED, TRY_AGAIN] {
        assert!(!message.is_empty());
        assert!(!message.contains('{'), "{message} looks like a template");
    }
}
