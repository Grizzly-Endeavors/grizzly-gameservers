use super::*;

const ADMIN_ROLE: u64 = 100;
const ALLOWED_USER: u64 = 7;

#[test]
fn allowlisted_user_is_authorized() {
    assert!(is_authorized(
        ALLOWED_USER,
        &[],
        Some(ADMIN_ROLE),
        &[ALLOWED_USER]
    ));
}

#[test]
fn user_with_admin_role_is_authorized() {
    assert!(is_authorized(42, &[ADMIN_ROLE], Some(ADMIN_ROLE), &[]));
}

#[test]
fn user_without_role_or_allowlist_is_denied() {
    assert!(!is_authorized(42, &[200, 300], Some(ADMIN_ROLE), &[7]));
}

#[test]
fn no_admin_role_configured_falls_back_to_allowlist_only() {
    assert!(!is_authorized(42, &[ADMIN_ROLE], None, &[]));
    assert!(is_authorized(7, &[ADMIN_ROLE], None, &[7]));
}

#[test]
fn allowlisted_user_gets_the_cross_channel_view() {
    match visibility_scope(ALLOWED_USER, 999, &[ALLOWED_USER]) {
        ServerScope::All => {}
        ServerScope::Channel(id) => panic!("expected All, got Channel({id})"),
    }
}

#[test]
fn non_allowlisted_user_is_confined_to_their_channel() {
    // The admin *role* must not widen visibility — only the user-id allowlist
    // does — so this caller is scoped even though it holds the admin role.
    match visibility_scope(42, 999, &[ALLOWED_USER]) {
        ServerScope::Channel(id) => assert_eq!(id, "999"),
        ServerScope::All => panic!("expected Channel, got All"),
    }
}
