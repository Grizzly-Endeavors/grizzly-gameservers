use super::*;

// Auth must degrade fail-closed: with no database, GuildConfig reports empty
// config for every guild and refuses every mutation, so the auth check falls
// back to the implicit admins (operators, guild owner) only — never opens up.

#[tokio::test]
async fn disabled_guild_config_is_unavailable() {
    let config = GuildConfig::connect(None).await;
    assert!(!config.is_available());
}

#[tokio::test]
async fn disabled_guild_config_reports_no_admins_or_managers() {
    let config = GuildConfig::connect(None).await;
    let admins = config.admins(42).await;
    assert!(admins.roles.is_empty());
    assert!(admins.users.is_empty());
    let managers = config.managers(42).await;
    assert!(managers.roles.is_empty());
    assert!(managers.users.is_empty());
}

#[tokio::test]
async fn disabled_guild_config_refuses_mutations() {
    let config = GuildConfig::connect(None).await;
    assert!(matches!(
        config.add_admin_user(42, 7).await.unwrap(),
        ConfigChange::Unavailable
    ));
    assert!(matches!(
        config.add_admin_role(42, 100).await.unwrap(),
        ConfigChange::Unavailable
    ));
    assert!(matches!(
        config.remove_admin_user(42, 7).await.unwrap(),
        ConfigChange::Unavailable
    ));
    assert!(matches!(
        config.remove_admin_role(42, 100).await.unwrap(),
        ConfigChange::Unavailable
    ));
    assert!(matches!(
        config.add_manager_user(42, 7).await.unwrap(),
        ConfigChange::Unavailable
    ));
    assert!(matches!(
        config.add_manager_role(42, 100).await.unwrap(),
        ConfigChange::Unavailable
    ));
    assert!(matches!(
        config.remove_manager_user(42, 7).await.unwrap(),
        ConfigChange::Unavailable
    ));
    assert!(matches!(
        config.remove_manager_role(42, 100).await.unwrap(),
        ConfigChange::Unavailable
    ));
}

#[tokio::test]
async fn disabled_home_channels_reports_non_home_and_refuses_toggle() {
    let home = HomeChannels::connect(None).await;
    assert!(!home.is_home(123).await);
    assert!(matches!(
        home.toggle(123, 42).await.unwrap(),
        HomeToggle::Unavailable
    ));
}

#[tokio::test]
async fn disabled_home_channels_has_no_guild_home() {
    let home = HomeChannels::connect(None).await;
    assert_eq!(home.home_for_guild(42).await, None);
}

#[test]
fn home_cache_resolves_oldest_home_per_guild() {
    let mut cache = HomeCache::default();
    cache.add(100, 1);
    cache.add(200, 2);
    cache.add(300, 1);
    assert_eq!(
        cache.home_for_guild(1),
        Some(100),
        "the oldest-registered channel is the guild's home"
    );
    assert_eq!(cache.home_for_guild(2), Some(200));
    assert_eq!(
        cache.home_for_guild(3),
        None,
        "an unknown guild has no home"
    );
}

#[test]
fn home_cache_removal_promotes_the_next_oldest() {
    let mut cache = HomeCache::default();
    cache.add(100, 1);
    cache.add(300, 1);
    cache.remove(100);
    assert_eq!(cache.home_for_guild(1), Some(300));
    cache.remove(300);
    assert_eq!(cache.home_for_guild(1), None);
}

#[test]
fn home_cache_readd_keeps_registration_order() {
    let mut cache = HomeCache::default();
    cache.add(100, 1);
    cache.add(300, 1);
    // A re-add only updates the guild (mirroring the store's upsert); it must
    // not move the channel to the back of the order.
    cache.add(100, 1);
    assert_eq!(cache.home_for_guild(1), Some(100));
}
