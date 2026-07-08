use super::*;

fn lookup_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
    move |key| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| OsString::from(*v))
    }
}

#[test]
fn applies_defaults_with_empty_environment() {
    let env = lookup_from(&[]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    assert_eq!(config.child_command, "/start", "default child command");
    assert_eq!(config.game_port, 25565, "default game port");
    assert_eq!(config.control_port, 9359, "default control port");
    assert_eq!(
        config.sdk_base_url, "http://127.0.0.1:9358",
        "default SDK base url"
    );
    assert_eq!(
        config.health_interval,
        Duration::from_secs(5),
        "default health interval"
    );
    assert_eq!(
        config.graceful_timeout,
        Duration::from_secs(90),
        "default graceful timeout"
    );
    assert_eq!(config.crash_threshold, 5, "default crash threshold");
    assert_eq!(
        config.data_dir,
        std::path::Path::new("/data"),
        "default data dir"
    );
    assert_eq!(config.rcon_port, None, "rcon disabled by default");
    assert!(!config.rcon_minecraft, "minecraft quirks off by default");
    assert_eq!(
        config.rcon_password_env, "RCON_PASSWORD",
        "default rcon password env"
    );
    assert!(!config.start_paused, "starts unpaused by default");
}

#[test]
fn start_paused_reads_the_flag() {
    let env = lookup_from(&[("SUPERVISOR_START_PAUSED", "true")]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    assert!(
        config.start_paused,
        "SUPERVISOR_START_PAUSED should hold the game down"
    );
}

#[test]
fn overrides_from_environment() {
    let env = lookup_from(&[
        ("SUPERVISOR_CHILD_CMD", "/opt/run.sh"),
        ("SUPERVISOR_GAME_PORT", "25500"),
        ("SUPERVISOR_CONTROL_PORT", "9999"),
        ("AGONES_SDK_HTTP", "http://127.0.0.1:1234"),
        ("SUPERVISOR_HEALTH_INTERVAL_SECS", "3"),
        ("SUPERVISOR_GRACEFUL_TIMEOUT_SECS", "120"),
        ("SUPERVISOR_CRASH_WINDOW_SECS", "60"),
        ("SUPERVISOR_CRASH_THRESHOLD", "10"),
        ("SUPERVISOR_DATA_DIR", "/srv/world"),
        ("SUPERVISOR_RCON_PORT", "25575"),
        ("SUPERVISOR_RCON_MINECRAFT", "true"),
        ("SUPERVISOR_RCON_PASSWORD_ENV", "SOURCE_RCON_PW"),
    ]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    assert_eq!(
        config.child_command, "/opt/run.sh",
        "child command override"
    );
    assert_eq!(config.game_port, 25500, "game port override");
    assert_eq!(config.control_port, 9999, "control port override");
    assert_eq!(
        config.sdk_base_url, "http://127.0.0.1:1234",
        "sdk url override"
    );
    assert_eq!(
        config.health_interval,
        Duration::from_secs(3),
        "health interval override"
    );
    assert_eq!(
        config.graceful_timeout,
        Duration::from_mins(2),
        "graceful timeout override"
    );
    assert_eq!(
        config.crash_window,
        Duration::from_mins(1),
        "crash window override"
    );
    assert_eq!(config.crash_threshold, 10, "crash threshold override");
    assert_eq!(
        config.data_dir,
        std::path::Path::new("/srv/world"),
        "data dir override"
    );
    assert_eq!(config.rcon_port, Some(25575), "rcon port override");
    assert!(config.rcon_minecraft, "minecraft quirks enabled");
    assert_eq!(
        config.rcon_password_env, "SOURCE_RCON_PW",
        "rcon password env override"
    );
}

#[test]
fn rcon_flag_accepts_truthy_spellings_and_ignores_others() {
    for truthy in ["1", "true", "TRUE", "Yes", "on"] {
        let pairs = [("SUPERVISOR_RCON_MINECRAFT", truthy)];
        let env = lookup_from(&pairs);
        let config = SupervisorConfig::from_env_with(&env).unwrap();
        assert!(config.rcon_minecraft, "{truthy:?} should enable the flag");
    }
    for falsy in ["0", "false", "no", "", "maybe"] {
        let pairs = [("SUPERVISOR_RCON_MINECRAFT", falsy)];
        let env = lookup_from(&pairs);
        let config = SupervisorConfig::from_env_with(&env).unwrap();
        assert!(
            !config.rcon_minecraft,
            "{falsy:?} should leave the flag off"
        );
    }
}

#[test]
fn rejects_non_numeric_port() {
    let env = lookup_from(&[("SUPERVISOR_GAME_PORT", "twenty-five-thousand")]);
    let err = SupervisorConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string().contains("SUPERVISOR_GAME_PORT"),
        "error should name the offending key, got: {err}"
    );
}

#[test]
fn rejects_out_of_range_port() {
    let env = lookup_from(&[("SUPERVISOR_CONTROL_PORT", "70000")]);
    let err = SupervisorConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string().contains("SUPERVISOR_CONTROL_PORT"),
        "u16 overflow should be reported against the key, got: {err}"
    );
}
