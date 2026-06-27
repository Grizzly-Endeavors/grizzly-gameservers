#![expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]

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
        Duration::from_secs(120),
        "graceful timeout override"
    );
    assert_eq!(
        config.crash_window,
        Duration::from_secs(60),
        "crash window override"
    );
    assert_eq!(config.crash_threshold, 10, "crash threshold override");
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
