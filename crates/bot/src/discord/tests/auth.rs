use super::*;
use crate::store::GrantSet;

const OPERATOR: u64 = 7;
const OWNER: u64 = 50;
const ADMIN_ROLE: u64 = 100;
const ADMIN_USER: u64 = 60;
const MANAGER_ROLE: u64 = 110;
const MANAGER_USER: u64 = 70;
const GUILD: u64 = 999;

fn grants(roles: &[u64], users: &[u64]) -> GrantSet {
    GrantSet {
        roles: roles.iter().copied().collect(),
        users: users.iter().copied().collect(),
    }
}

fn check<'a>(
    user: u64,
    roles: &'a [u64],
    guild_owner: Option<u64>,
    operators: &'a [u64],
    guild_admins: &'a GrantSet,
    guild_managers: &'a GrantSet,
) -> AccessCheck<'a> {
    AccessCheck {
        user,
        roles,
        guild_owner,
        operators,
        guild_admins,
        guild_managers,
    }
}

#[test]
fn operator_is_admin_anywhere() {
    let empty = GrantSet::default();
    assert_eq!(
        access_level(&check(OPERATOR, &[], None, &[OPERATOR], &empty, &empty)),
        AccessLevel::Admin
    );
}

#[test]
fn guild_owner_is_admin_in_their_guild() {
    let empty = GrantSet::default();
    assert_eq!(
        access_level(&check(OWNER, &[], Some(OWNER), &[OPERATOR], &empty, &empty)),
        AccessLevel::Admin
    );
}

#[test]
fn db_configured_admin_user_is_admin() {
    let admins = grants(&[], &[ADMIN_USER]);
    let empty = GrantSet::default();
    assert_eq!(
        access_level(&check(
            ADMIN_USER,
            &[],
            Some(OWNER),
            &[OPERATOR],
            &admins,
            &empty
        )),
        AccessLevel::Admin
    );
}

#[test]
fn member_with_db_configured_admin_role_is_admin() {
    let admins = grants(&[ADMIN_ROLE], &[]);
    let empty = GrantSet::default();
    assert_eq!(
        access_level(&check(
            42,
            &[200, ADMIN_ROLE],
            Some(OWNER),
            &[OPERATOR],
            &admins,
            &empty
        )),
        AccessLevel::Admin
    );
}

#[test]
fn db_configured_manager_user_is_manager() {
    let empty = GrantSet::default();
    let managers = grants(&[], &[MANAGER_USER]);
    assert_eq!(
        access_level(&check(
            MANAGER_USER,
            &[],
            Some(OWNER),
            &[OPERATOR],
            &empty,
            &managers
        )),
        AccessLevel::Manager
    );
}

#[test]
fn member_with_db_configured_manager_role_is_manager() {
    let empty = GrantSet::default();
    let managers = grants(&[MANAGER_ROLE], &[]);
    assert_eq!(
        access_level(&check(
            42,
            &[200, MANAGER_ROLE],
            Some(OWNER),
            &[OPERATOR],
            &empty,
            &managers
        )),
        AccessLevel::Manager
    );
}

#[test]
fn admin_grant_outranks_manager_grant() {
    // Someone granted both tiers resolves to the higher one.
    let admins = grants(&[], &[ADMIN_USER]);
    let managers = grants(&[], &[ADMIN_USER]);
    assert_eq!(
        access_level(&check(
            ADMIN_USER,
            &[],
            Some(OWNER),
            &[OPERATOR],
            &admins,
            &managers
        )),
        AccessLevel::Admin
    );
}

#[test]
fn access_levels_are_ordered() {
    // The `>=` gates in the checks and tool dispatch rely on this ordering.
    assert!(AccessLevel::Admin > AccessLevel::Manager);
    assert!(AccessLevel::Manager > AccessLevel::ReadOnly);
}

#[test]
fn ungranted_member_is_read_only() {
    let admins = grants(&[ADMIN_ROLE], &[ADMIN_USER]);
    let managers = grants(&[MANAGER_ROLE], &[MANAGER_USER]);
    assert_eq!(
        access_level(&check(
            42,
            &[200, 300],
            Some(OWNER),
            &[OPERATOR],
            &admins,
            &managers
        )),
        AccessLevel::ReadOnly
    );
}

#[test]
fn fails_closed_when_config_empty() {
    // Config DB down => both grant sets empty. Only operators + owner get in
    // (as admins); a would-be manager falls back to read-only.
    let empty = GrantSet::default();
    assert_eq!(
        access_level(&check(
            MANAGER_USER,
            &[MANAGER_ROLE],
            Some(OWNER),
            &[OPERATOR],
            &empty,
            &empty
        )),
        AccessLevel::ReadOnly
    );
    assert_eq!(
        access_level(&check(OWNER, &[], Some(OWNER), &[OPERATOR], &empty, &empty)),
        AccessLevel::Admin
    );
    assert_eq!(
        access_level(&check(
            OPERATOR,
            &[],
            Some(OWNER),
            &[OPERATOR],
            &empty,
            &empty
        )),
        AccessLevel::Admin
    );
}

#[test]
fn operator_gets_the_all_guilds_view() {
    match visibility_scope(OPERATOR, Some(GUILD), &[OPERATOR]) {
        Some(ServerScope::All) => {}
        other => panic!("expected All, got {other:?}"),
    }
}

#[test]
fn operator_gets_all_scope_even_in_a_dm() {
    // The DM carve-out: an operator can manage every server from a DM.
    match visibility_scope(OPERATOR, None, &[OPERATOR]) {
        Some(ServerScope::All) => {}
        other => panic!("expected All, got {other:?}"),
    }
}

#[test]
fn non_operator_is_confined_to_their_guild() {
    match visibility_scope(42, Some(GUILD), &[OPERATOR]) {
        Some(ServerScope::Guild(id)) => assert_eq!(id, "999"),
        other => panic!("expected Guild, got {other:?}"),
    }
}

#[test]
fn non_operator_in_a_dm_has_no_scope() {
    assert!(visibility_scope(42, None, &[OPERATOR]).is_none());
}
