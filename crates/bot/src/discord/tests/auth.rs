use super::*;
use crate::store::GuildAdmins;

const OPERATOR: u64 = 7;
const OWNER: u64 = 50;
const ADMIN_ROLE: u64 = 100;
const ADMIN_USER: u64 = 60;
const GUILD: u64 = 999;

fn admins(roles: &[u64], users: &[u64]) -> GuildAdmins {
    GuildAdmins {
        roles: roles.iter().copied().collect(),
        users: users.iter().copied().collect(),
    }
}

fn check<'a>(
    user: u64,
    roles: &'a [u64],
    guild_owner: Option<u64>,
    operators: &'a [u64],
    guild_admins: &'a GuildAdmins,
) -> AdminCheck<'a> {
    AdminCheck {
        user,
        roles,
        guild_owner,
        operators,
        guild_admins,
    }
}

#[test]
fn operator_is_authorized_anywhere() {
    let empty = GuildAdmins::default();
    assert!(is_authorized(&check(
        OPERATOR,
        &[],
        None,
        &[OPERATOR],
        &empty
    )));
}

#[test]
fn guild_owner_is_authorized_in_their_guild() {
    let empty = GuildAdmins::default();
    assert!(is_authorized(&check(
        OWNER,
        &[],
        Some(OWNER),
        &[OPERATOR],
        &empty
    )));
}

#[test]
fn db_configured_admin_user_is_authorized() {
    let admins = admins(&[], &[ADMIN_USER]);
    assert!(is_authorized(&check(
        ADMIN_USER,
        &[],
        Some(OWNER),
        &[OPERATOR],
        &admins
    )));
}

#[test]
fn member_with_db_configured_admin_role_is_authorized() {
    let admins = admins(&[ADMIN_ROLE], &[]);
    assert!(is_authorized(&check(
        42,
        &[200, ADMIN_ROLE],
        Some(OWNER),
        &[OPERATOR],
        &admins
    )));
}

#[test]
fn non_admin_is_denied() {
    let admins = admins(&[ADMIN_ROLE], &[ADMIN_USER]);
    assert!(!is_authorized(&check(
        42,
        &[200, 300],
        Some(OWNER),
        &[OPERATOR],
        &admins
    )));
}

#[test]
fn fails_closed_to_implicit_admins_when_config_empty() {
    // Config DB down => guild_admins empty. Only operators + owner get in.
    let empty = GuildAdmins::default();
    assert!(!is_authorized(&check(
        ADMIN_USER,
        &[ADMIN_ROLE],
        Some(OWNER),
        &[OPERATOR],
        &empty
    )));
    assert!(is_authorized(&check(
        OWNER,
        &[],
        Some(OWNER),
        &[OPERATOR],
        &empty
    )));
    assert!(is_authorized(&check(
        OPERATOR,
        &[],
        Some(OWNER),
        &[OPERATOR],
        &empty
    )));
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
