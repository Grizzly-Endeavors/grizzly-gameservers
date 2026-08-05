use super::*;

use axum::http::{HeaderName, HeaderValue};

const GROUP: &str = "grizzly-admins";

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
            HeaderValue::from_str(value).expect("valid header value"),
        );
    }
    headers
}

#[test]
fn a_member_of_the_required_group_is_admitted() {
    let identity = identify(
        &headers(&[
            (USERNAME_HEADER, "bear"),
            (GROUPS_HEADER, "grizzly-users|grizzly-admins"),
            (EMAIL_HEADER, "bear@example.com"),
        ]),
        GROUP,
    )
    .expect("a member is admitted");
    assert_eq!(identity.username, "bear");
    assert_eq!(identity.email.as_deref(), Some("bear@example.com"));
}

/// Authentik joins groups with `|`, but accepting a comma too means a change in
/// that formatting degrades to "some groups parse oddly" rather than locking
/// everyone out of the panel.
#[test]
fn either_separator_splits_the_group_header() {
    for raw in [
        "grizzly-admins",
        "a|grizzly-admins|b",
        "a, grizzly-admins ,b",
        " grizzly-admins ",
    ] {
        assert!(
            identify(
                &headers(&[(USERNAME_HEADER, "bear"), (GROUPS_HEADER, raw)]),
                GROUP,
            )
            .is_ok(),
            "{raw} should have parsed as membership"
        );
    }
}

#[test]
fn a_signed_in_non_member_is_told_which_group_they_lack() {
    let denial = identify(
        &headers(&[
            (USERNAME_HEADER, "friend"),
            (GROUPS_HEADER, "grizzly-users|household"),
        ]),
        GROUP,
    )
    .expect_err("a non-member is refused");
    match denial {
        Denial::NotAMember {
            username,
            required,
            held,
        } => {
            assert_eq!(username, "friend");
            assert_eq!(required, GROUP);
            assert_eq!(held, ["grizzly-users", "household"]);
        }
        Denial::Unidentified => panic!("a named user is not unidentified"),
    }
}

/// A near-miss must not pass: the check is exact-match per group, not a
/// substring test over the joined header.
#[test]
fn a_similarly_named_group_does_not_count() {
    for raw in [
        "grizzly-admins-readonly",
        "not-grizzly-admins",
        "grizzlyadmins",
    ] {
        let denial = identify(
            &headers(&[(USERNAME_HEADER, "friend"), (GROUPS_HEADER, raw)]),
            GROUP,
        );
        assert!(denial.is_err(), "{raw} must not be read as membership");
    }
}

/// No identity headers at all means the request reached the panel without going
/// through the outpost — the case the network policy exists to prevent. It must
/// be a refusal, never a default identity.
#[test]
fn a_request_without_identity_headers_is_refused() {
    assert_eq!(identify(&headers(&[]), GROUP), Err(Denial::Unidentified));
    assert_eq!(
        identify(&headers(&[(GROUPS_HEADER, "grizzly-admins")]), GROUP),
        Err(Denial::Unidentified),
        "groups alone must not stand in for a username"
    );
}

/// A header present but blank is the same as absent — an outpost that forwards
/// an empty username has not identified anybody.
#[test]
fn a_blank_username_is_not_an_identity() {
    assert_eq!(
        identify(
            &headers(&[(USERNAME_HEADER, "   "), (GROUPS_HEADER, GROUP)]),
            GROUP,
        ),
        Err(Denial::Unidentified)
    );
}

#[test]
fn a_member_with_no_email_is_still_admitted() {
    let identity = identify(
        &headers(&[(USERNAME_HEADER, "bear"), (GROUPS_HEADER, GROUP)]),
        GROUP,
    )
    .expect("email is optional");
    assert_eq!(identity.email, None);
}

#[test]
fn an_empty_group_header_holds_nothing() {
    let denial = identify(
        &headers(&[(USERNAME_HEADER, "friend"), (GROUPS_HEADER, "")]),
        GROUP,
    )
    .expect_err("no groups means no membership");
    match denial {
        Denial::NotAMember { held, .. } => assert!(held.is_empty()),
        Denial::Unidentified => panic!("a named user is not unidentified"),
    }
}
