use super::*;

#[test]
fn an_unconfigured_api_accepts_anything() {
    let auth = ControlAuth::new(None);
    assert!(!auth.is_enforced());
    assert!(auth.accepts(None));
    assert!(auth.accepts(Some("Bearer whatever")));
}

/// A secret that synced blank must not become a credential every caller in the
/// namespace can guess by sending nothing.
#[test]
fn a_blank_token_reads_as_unconfigured() {
    for blank in ["", "   ", "\n"] {
        let auth = ControlAuth::new(Some(blank));
        assert!(
            !auth.is_enforced(),
            "{blank:?} should not enable enforcement"
        );
    }
}

#[test]
fn a_configured_api_requires_the_exact_token() {
    let auth = ControlAuth::new(Some("s3cret"));
    assert!(auth.is_enforced());
    assert!(auth.accepts(Some("Bearer s3cret")));

    assert!(!auth.accepts(None), "a missing header must be rejected");
    assert!(!auth.accepts(Some("Bearer wrong")));
    assert!(
        !auth.accepts(Some("Bearer s3cret ")),
        "trailing whitespace is a different token, not a tolerable typo"
    );
    assert!(
        !auth.accepts(Some("s3cret")),
        "the Bearer scheme is required, not just the value"
    );
    assert!(
        !auth.accepts(Some("bearer s3cret")),
        "the scheme is matched exactly, so a mismatch fails closed"
    );
    assert!(
        !auth.accepts(Some("Bearer ")),
        "an empty presented token must never match a configured one"
    );
}

/// The token is trimmed on the way in, so a secret file with a trailing newline
/// — the usual shape out of a mounted secret — still authenticates.
#[test]
fn a_configured_token_is_trimmed() {
    let auth = ControlAuth::new(Some("s3cret\n"));
    assert!(auth.accepts(Some("Bearer s3cret")));
}

#[test]
fn comparison_rejects_a_prefix_and_a_suffix() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"ab", b"abc"));
    assert!(!constant_time_eq(b"abcd", b"abc"));
    assert!(!constant_time_eq(b"abd", b"abc"));
    assert!(constant_time_eq(b"", b""));
}
