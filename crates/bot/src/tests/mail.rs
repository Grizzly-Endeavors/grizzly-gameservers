use super::*;

const INVITE: &str = "https://discord.com/oauth2/authorize?client_id=1&scope=bot";
const COMMUNITY: &str = "https://discord.gg/example";

#[tokio::test]
async fn an_unconfigured_mailer_never_dials() {
    let mailer = Mailer::connect(None);
    assert!(!mailer.is_enabled());
    assert_eq!(
        mailer.send("friend@example.com", "hi", "body").await,
        MailOutcome::Unavailable
    );
}

/// The whole point of the latch: after one refusal nothing may reach the wire
/// again, because a retried bad credential gets the household's address banned.
#[test]
fn the_gate_closes_permanently_once_the_credential_is_refused() {
    let gate = SendGate::default();
    assert!(gate.is_open());
    gate.close();
    assert!(!gate.is_open());
}

#[test]
fn closing_the_gate_twice_is_harmless() {
    let gate = SendGate::default();
    gate.close();
    gate.close();
    assert!(!gate.is_open());
}

#[test]
fn the_approval_email_carries_the_invite_link() {
    let body = approval_body(INVITE, None);
    assert!(body.contains(INVITE), "{body}");
    assert!(!body.is_empty());
}

/// An absent community link must drop the whole sentence, not render an empty
/// one — "say hello in the community server:" followed by nothing reads as a bug
/// to the person who was just told they were let in.
#[test]
fn an_absent_community_link_omits_its_sentence_entirely() {
    let body = approval_body(INVITE, None);
    assert!(!body.contains("community server"), "{body}");
}

#[test]
fn a_present_community_link_is_included() {
    let body = approval_body(INVITE, Some(COMMUNITY));
    assert!(body.contains(COMMUNITY), "{body}");
    assert!(body.contains("community server"), "{body}");
}

/// A blank setting is the same as an unset one — the settings catalog stores
/// strings, and "  " is what an operator clearing a field actually leaves.
#[test]
fn a_blank_community_link_is_treated_as_absent() {
    let body = approval_body(INVITE, Some("   "));
    assert!(!body.contains("community server"), "{body}");
}

#[test]
fn the_body_has_no_unsubstituted_placeholders() {
    let body = approval_body(INVITE, Some(COMMUNITY));
    assert!(!body.contains("{{"), "{body}");
    assert!(!body.contains("{}"), "{body}");
}

/// The email says adding the bot isn't the last step. Without it, someone adds
/// Gary, gets no reply because their guild isn't admitted yet, and concludes it
/// is broken.
#[test]
fn the_body_warns_that_access_is_not_immediate() {
    let body = approval_body(INVITE, None);
    assert!(body.contains("first half"), "{body}");
}

#[test]
fn every_outcome_describes_itself_for_the_operator() {
    for outcome in [
        MailOutcome::Sent,
        MailOutcome::Unavailable,
        MailOutcome::AuthRefused,
        MailOutcome::Rejected {
            reason: "550 no such user".to_owned(),
        },
        MailOutcome::Failed {
            reason: "timed out".to_owned(),
        },
    ] {
        assert!(!outcome.describe().is_empty(), "{outcome:?}");
    }
}

#[test]
fn only_a_sent_outcome_counts_as_sent() {
    assert!(MailOutcome::Sent.is_sent());
    assert!(!MailOutcome::Unavailable.is_sent());
    assert!(!MailOutcome::AuthRefused.is_sent());
}

/// Every mail outcome must map to an event outcome, so a send never lands in the
/// product log as an unexplained blank.
#[test]
fn every_mail_outcome_maps_to_an_event_outcome() {
    use crate::events::mapping;
    for outcome in [
        MailOutcome::Sent,
        MailOutcome::Unavailable,
        MailOutcome::AuthRefused,
        MailOutcome::Rejected {
            reason: "550".to_owned(),
        },
        MailOutcome::Failed {
            reason: "timeout".to_owned(),
        },
    ] {
        assert!(!mapping::mail(&outcome).is_empty(), "{outcome:?}");
    }
    assert_eq!(mapping::mail(&MailOutcome::Sent), mapping::SENT);
    assert_eq!(
        mapping::mail(&MailOutcome::AuthRefused),
        mapping::AUTH_REFUSED
    );
}
