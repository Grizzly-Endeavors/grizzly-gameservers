#![expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]

use std::ffi::OsString;

use super::*;

fn lookup_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
    move |key| {
        pairs
            .iter()
            .find_map(|(k, v)| (*k == key).then(|| OsString::from(*v)))
    }
}

#[test]
fn parses_required_fields_and_applies_defaults() {
    let env = lookup_from(&[("DISCORD_BOT_TOKEN", "secret"), ("DISCORD_GUILD_ID", "42")]);
    let config = BotConfig::from_env_with(&env).unwrap();

    assert_eq!(config.token, "secret", "token should come from env");
    assert_eq!(config.guild_id, 42, "guild id should parse to integer");
    assert_eq!(config.namespace, "game-servers", "namespace should default");
    assert_eq!(
        config.domain, "gameservers.bearflinn.com",
        "domain should default"
    );
}

#[test]
fn overrides_namespace_and_domain_when_set() {
    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("DISCORD_GUILD_ID", "42"),
        ("GAMESERVERS_NAMESPACE", "other-ns"),
        ("GAMESERVERS_DOMAIN", "example.com"),
    ]);
    let config = BotConfig::from_env_with(&env).unwrap();

    assert_eq!(
        config.namespace, "other-ns",
        "namespace override should apply"
    );
    assert_eq!(config.domain, "example.com", "domain override should apply");
}

#[test]
fn missing_token_is_an_error() {
    let env = lookup_from(&[("DISCORD_GUILD_ID", "42")]);
    let err = BotConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string().contains("DISCORD_BOT_TOKEN"),
        "error should name the missing variable, got: {err}"
    );
}

#[test]
fn non_numeric_guild_id_is_an_error() {
    let env = lookup_from(&[("DISCORD_BOT_TOKEN", "secret"), ("DISCORD_GUILD_ID", "abc")]);
    let err = BotConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string().contains("DISCORD_GUILD_ID"),
        "error should name the offending variable, got: {err}"
    );
}

#[test]
fn zero_guild_id_is_rejected() {
    let env = lookup_from(&[("DISCORD_BOT_TOKEN", "secret"), ("DISCORD_GUILD_ID", "0")]);
    let err = BotConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string().contains("non-zero"),
        "error should reject zero guild id, got: {err}"
    );
}
