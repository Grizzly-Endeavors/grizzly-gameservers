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
    let env = lookup_from(&[("DISCORD_BOT_TOKEN", "secret")]);
    let config = BotConfig::from_env_with(&env).unwrap();

    assert_eq!(config.token, "secret", "token should come from env");
    assert_eq!(config.namespace, "game-servers", "namespace should default");
    assert_eq!(
        config.domain, "gameservers.grizzly-endeavors.com",
        "domain should default"
    );
    assert!(
        config.operator_ids.is_empty(),
        "operator seed defaults empty"
    );
    assert_eq!(config.control_port, 9359, "control port should default");
    assert_eq!(
        config.catalog_dir,
        std::path::PathBuf::from("/usr/local/share/grizzly-gameservers/games"),
        "catalog dir should default to the baked path"
    );
}

#[test]
fn parses_operator_allowlist() {
    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("GAMESERVERS_ADMIN_USER_IDS", "10, 20 ,30,"),
        ("GAMESERVERS_CATALOG_DIR", "/srv/games"),
    ]);
    let config = BotConfig::from_env_with(&env).unwrap();

    assert_eq!(
        config.operator_ids,
        vec![10, 20, 30],
        "operator seed should split on commas and tolerate spaces and a trailing comma"
    );
    assert_eq!(config.catalog_dir, std::path::PathBuf::from("/srv/games"));
}

#[test]
fn non_numeric_user_in_allowlist_is_an_error() {
    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("GAMESERVERS_ADMIN_USER_IDS", "10,nope,30"),
    ]);
    let err = BotConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string().contains("GAMESERVERS_ADMIN_USER_IDS"),
        "error should name the offending variable, got: {err}"
    );
}

#[test]
fn overrides_namespace_and_domain_when_set() {
    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
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
    let env = lookup_from(&[]);
    let err = BotConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string().contains("DISCORD_BOT_TOKEN"),
        "error should name the missing variable, got: {err}"
    );
}

#[test]
fn control_port_override_and_validation() {
    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("GAMESERVERS_CONTROL_PORT", "9400"),
    ]);
    let config = BotConfig::from_env_with(&env).unwrap();
    assert_eq!(
        config.control_port, 9400,
        "control port override should apply"
    );

    let bad = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("GAMESERVERS_CONTROL_PORT", "70000"),
    ]);
    let err = BotConfig::from_env_with(&bad).unwrap_err();
    assert!(
        err.to_string().contains("GAMESERVERS_CONTROL_PORT"),
        "an out-of-range port should name the variable, got: {err}"
    );
}

#[test]
fn ollama_defaults_when_unset_and_key_is_absent() {
    let env = lookup_from(&[("DISCORD_BOT_TOKEN", "secret")]);
    let config = BotConfig::from_env_with(&env).unwrap();

    assert_eq!(config.ollama_api_key, None, "agent key is optional");
    assert_eq!(
        config.ollama_base_url, "https://ollama.com/v1",
        "base url should default to ollama cloud"
    );
    assert_eq!(config.ollama_model, "glm-5.2", "model should default");
}

#[test]
fn ollama_overrides_apply_and_blank_key_reads_as_absent() {
    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("OLLAMA_API_KEY", ""),
        ("OLLAMA_BASE_URL", "http://localhost:11434/v1"),
        ("OLLAMA_MODEL", "qwen3"),
    ]);
    let config = BotConfig::from_env_with(&env).unwrap();

    assert_eq!(
        config.ollama_api_key, None,
        "a blank key should be treated as unset, not an empty bearer token"
    );
    assert_eq!(config.ollama_base_url, "http://localhost:11434/v1");
    assert_eq!(config.ollama_model, "qwen3");
}

#[test]
fn db_is_none_without_a_password() {
    // The password is the OpenBao-sourced part; its absence is the degrade signal.
    let env = lookup_from(&[("DISCORD_BOT_TOKEN", "secret")]);
    let config = BotConfig::from_env_with(&env).unwrap();
    assert!(
        config.db.is_none(),
        "no DB_PASSWORD should disable persistence"
    );
}

#[test]
fn db_defaults_to_foundation_postgres_when_password_present() {
    let env = lookup_from(&[("DISCORD_BOT_TOKEN", "secret"), ("DB_PASSWORD", "pw")]);
    let db = BotConfig::from_env_with(&env).unwrap().db.unwrap();
    assert_eq!(db.host, "10.0.0.200");
    assert_eq!(db.port, 5432);
    assert_eq!(db.database, "grizzly_gameservers");
    assert_eq!(db.user, "grizzly_gameservers");
    assert_eq!(db.password, "pw");
}

#[test]
fn blank_db_password_reads_as_absent() {
    let env = lookup_from(&[("DISCORD_BOT_TOKEN", "secret"), ("DB_PASSWORD", "")]);
    assert!(
        BotConfig::from_env_with(&env).unwrap().db.is_none(),
        "a blank password should read as unset, not an empty credential"
    );
}

#[test]
fn db_overrides_apply() {
    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("DB_PASSWORD", "pw"),
        ("DB_HOST", "127.0.0.1"),
        ("DB_PORT", "6000"),
        ("DB_NAME", "gg_dev"),
        ("DB_USER", "dev"),
    ]);
    let db = BotConfig::from_env_with(&env).unwrap().db.unwrap();
    assert_eq!(db.host, "127.0.0.1");
    assert_eq!(db.port, 6000);
    assert_eq!(db.database, "gg_dev");
    assert_eq!(db.user, "dev");
}

#[test]
fn invalid_db_port_is_an_error() {
    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("DB_PASSWORD", "pw"),
        ("DB_PORT", "99999"),
    ]);
    let err = BotConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string().contains("DB_PORT"),
        "an out-of-range DB port should name the variable, got: {err}"
    );
}
