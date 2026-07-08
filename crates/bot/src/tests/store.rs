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
async fn disabled_guild_config_reports_no_admins() {
    let config = GuildConfig::connect(None).await;
    let admins = config.admins(42).await;
    assert!(admins.roles.is_empty());
    assert!(admins.users.is_empty());
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
