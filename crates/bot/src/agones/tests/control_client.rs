use super::*;

#[test]
fn no_token_means_no_credential() {
    let client = ControlClient::new(reqwest::Client::new(), None);
    assert!(!client.is_authenticated());
}

/// Mirrors the supervisor's own parsing. If the two disagreed about what counts
/// as "no token", a blank secret would leave one side enforcing and the other
/// silent — every supervisor call failing with a 401 the bot couldn't explain.
#[test]
fn a_blank_token_reads_as_absent_on_this_side_too() {
    for blank in ["", "   ", "\n"] {
        let client = ControlClient::new(reqwest::Client::new(), Some(blank));
        assert!(
            !client.is_authenticated(),
            "{blank:?} should not authenticate"
        );
    }
}

#[test]
fn a_configured_token_is_presented() {
    let client = ControlClient::new(reqwest::Client::new(), Some("s3cret"));
    assert!(client.is_authenticated());
    let request = client.get("http://example.invalid/status").build().unwrap();
    assert_eq!(
        request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer s3cret")
    );
}

/// The supervisor trims what it compares against, so the bot must trim what it
/// sends — a mounted secret routinely arrives with a trailing newline.
#[test]
fn a_token_is_trimmed_before_it_is_sent() {
    let client = ControlClient::new(reqwest::Client::new(), Some(" s3cret\n"));
    let request = client.post("http://example.invalid/stop").build().unwrap();
    assert_eq!(
        request
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer s3cret")
    );
}
