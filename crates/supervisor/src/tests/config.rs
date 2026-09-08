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
        config.readiness_timeout,
        Duration::from_mins(10),
        "default readiness timeout"
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
    assert_eq!(
        config.rcon_dialect,
        crate::rcon::RconDialect::Source,
        "source dialect by default"
    );
    assert_eq!(
        config.palworld_ini_path, None,
        "palworld ini seeding off by default"
    );
    assert_eq!(
        config.rcon_password_env, "RCON_PASSWORD",
        "default rcon password env"
    );
    assert_eq!(
        config.rcon_password_max_len, None,
        "full-length rcon password by default"
    );
    assert!(!config.start_paused, "starts unpaused by default");
    assert_eq!(config.chat_watch, None, "chat watching off by default");
    assert_eq!(
        config.child_env, None,
        "per-instance env overlay off by default"
    );
    assert_eq!(
        config.ready_log_pattern, None,
        "TCP connect probe by default (no log-pattern readiness)"
    );
}

#[test]
fn parses_readiness_timeout_when_set() {
    let env = lookup_from(&[("SUPERVISOR_READINESS_TIMEOUT_SECS", "1800")]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    assert_eq!(
        config.readiness_timeout,
        Duration::from_mins(30),
        "the readiness timeout is read from the environment"
    );
}

#[test]
fn parses_rcon_password_max_len_when_set() {
    let env = lookup_from(&[("SUPERVISOR_RCON_PASSWORD_MAX_LEN", "30")]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    assert_eq!(
        config.rcon_password_max_len,
        Some(30),
        "the password cap is read from the environment"
    );
}

#[test]
fn parses_ready_log_pattern_when_set() {
    let env = lookup_from(&[("SUPERVISOR_READY_LOG_PATTERN", "Game server connected")]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    assert_eq!(
        config.ready_log_pattern.as_deref(),
        Some("Game server connected"),
        "the readiness marker is read verbatim from the environment"
    );
}

#[test]
fn chat_watch_absent_without_format() {
    let env = lookup_from(&[
        ("SUPERVISOR_AGENT_URL", "http://bot:9360/ingame-trigger"),
        ("SUPERVISOR_GAMESERVER_NAME", "mc-abc"),
    ]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    assert_eq!(
        config.chat_watch, None,
        "without a format the watcher stays off even if the endpoint is set"
    );
}

#[test]
fn chat_watch_assembles_when_configured() {
    let env = lookup_from(&[
        ("SUPERVISOR_CHAT_FORMAT", "minecraft"),
        ("SUPERVISOR_AGENT_URL", "http://bot:9360/ingame-trigger"),
        ("SUPERVISOR_GAMESERVER_NAME", "mc-abc"),
        ("SUPERVISOR_AGENT_TOKEN", "s3cret"),
    ]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    let watch = config.chat_watch.expect("chat watch should be configured");
    assert_eq!(watch.format, crate::chat_watcher::ChatFormat::Minecraft);
    assert_eq!(watch.trigger, "@Gary", "trigger defaults when unset");
    assert_eq!(watch.agent_url, "http://bot:9360/ingame-trigger");
    assert_eq!(watch.agent_token.as_deref(), Some("s3cret"));
    assert_eq!(watch.server, "mc-abc");
}

#[test]
fn chat_watch_trigger_is_overridable() {
    let env = lookup_from(&[
        ("SUPERVISOR_CHAT_FORMAT", "minecraft"),
        ("SUPERVISOR_AGENT_URL", "http://bot:9360/ingame-trigger"),
        ("SUPERVISOR_GAMESERVER_NAME", "mc-abc"),
        ("SUPERVISOR_CHAT_TRIGGER", "!ask"),
    ]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    let watch = config.chat_watch.expect("chat watch should be configured");
    assert_eq!(watch.trigger, "!ask");
    assert_eq!(watch.agent_token, None, "token is optional");
}

#[test]
fn chat_watch_format_without_endpoint_is_an_error() {
    let env = lookup_from(&[("SUPERVISOR_CHAT_FORMAT", "minecraft")]);
    let err = SupervisorConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string().contains("SUPERVISOR_AGENT_URL"),
        "a format with no endpoint should name the missing key, got: {err}"
    );
}

#[test]
fn chat_watch_rejects_unknown_format() {
    let env = lookup_from(&[
        ("SUPERVISOR_CHAT_FORMAT", "quake"),
        ("SUPERVISOR_AGENT_URL", "http://bot:9360/ingame-trigger"),
        ("SUPERVISOR_GAMESERVER_NAME", "mc-abc"),
    ]);
    let err = SupervisorConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string().contains("SUPERVISOR_CHAT_FORMAT"),
        "an unknown format should name the key, got: {err}"
    );
}

#[test]
fn child_env_assembles_when_configured() {
    let env = lookup_from(&[
        ("SUPERVISOR_CHILD_ENV_FILE", ".grizzly/env"),
        ("SUPERVISOR_CHILD_ENV_ALLOWLIST", "TYPE,MODRINTH_PROJECTS"),
    ]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    let overlay = config.child_env.expect("overlay should be configured");
    assert_eq!(overlay.file, ".grizzly/env");
    assert_eq!(overlay.allowlist, vec!["TYPE", "MODRINTH_PROJECTS"]);
}

#[test]
fn child_env_allowlist_tolerates_spacing_and_stray_commas() {
    // YAML makes it easy to hand-wrap or trailing-comma this value; neither
    // should produce a key that can never match.
    let env = lookup_from(&[
        ("SUPERVISOR_CHILD_ENV_FILE", ".grizzly/env"),
        ("SUPERVISOR_CHILD_ENV_ALLOWLIST", " TYPE , ,VERSION ,"),
    ]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    let overlay = config.child_env.expect("overlay should be configured");
    assert_eq!(overlay.allowlist, vec!["TYPE", "VERSION"]);
}

#[test]
fn child_env_absent_without_a_file() {
    // An allowlist alone has nothing to read; the config warns and stays off
    // rather than half-arming the injection path.
    let env = lookup_from(&[("SUPERVISOR_CHILD_ENV_ALLOWLIST", "TYPE")]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    assert_eq!(config.child_env, None);
}

#[test]
fn child_env_without_an_allowlist_fails_closed() {
    let env = lookup_from(&[("SUPERVISOR_CHILD_ENV_FILE", ".grizzly/env")]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    let overlay = config.child_env.expect("overlay should be configured");
    assert!(
        overlay.allowlist.is_empty(),
        "no allowlist means the overlay injects nothing"
    );
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
        ("SUPERVISOR_RCON_DIALECT", "minecraft"),
        ("SUPERVISOR_RCON_PASSWORD_ENV", "SOURCE_RCON_PW"),
        (
            "SUPERVISOR_PALWORLD_INI",
            "/palworld/Pal/Saved/Config/LinuxServer/PalWorldSettings.ini",
        ),
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
    assert_eq!(
        config.rcon_dialect,
        crate::rcon::RconDialect::Minecraft,
        "dialect override"
    );
    assert_eq!(
        config.rcon_password_env, "SOURCE_RCON_PW",
        "rcon password env override"
    );
    assert_eq!(
        config.palworld_ini_path.as_deref(),
        Some(std::path::Path::new(
            "/palworld/Pal/Saved/Config/LinuxServer/PalWorldSettings.ini"
        )),
        "palworld ini path override"
    );
}

#[test]
fn rcon_dialect_parses_each_variant() {
    use crate::rcon::RconDialect;
    for (raw, expected) in [
        ("minecraft", RconDialect::Minecraft),
        ("source", RconDialect::Source),
        ("palworld", RconDialect::Palworld),
    ] {
        let pairs = [("SUPERVISOR_RCON_DIALECT", raw)];
        let env = lookup_from(&pairs);
        let config = SupervisorConfig::from_env_with(&env).unwrap();
        assert_eq!(config.rcon_dialect, expected, "{raw:?} should parse");
    }
}

#[test]
fn rejects_unknown_rcon_dialect() {
    let env = lookup_from(&[("SUPERVISOR_RCON_DIALECT", "halflife")]);
    let err = SupervisorConfig::from_env_with(&env).unwrap_err();
    assert!(
        format!("{err:#}").contains("SUPERVISOR_RCON_DIALECT"),
        "the error should name the offending variable, got {err:#}"
    );
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

// ---- thunderstore mod reconciler ----

#[test]
fn thunderstore_assembles_when_the_manifest_and_the_image_agree() {
    let env = lookup_from(&[
        ("SUPERVISOR_MODS_FILE", ".grizzly/mods"),
        ("SUPERVISOR_THUNDERSTORE_COMMUNITY", "valheim"),
        ("SUPERVISOR_THUNDERSTORE_LAYOUT", "bepinex"),
        (
            "SUPERVISOR_THUNDERSTORE_PROVIDED",
            "denikson-BepInExPack_Valheim",
        ),
    ]);
    let config = SupervisorConfig::from_env_with(&env).unwrap();
    let mods = config.thunderstore.expect("the reconciler is configured");
    assert_eq!(mods.file, ".grizzly/mods");
    assert_eq!(mods.community, "valheim");
    assert_eq!(mods.layout, PackageLayout::BepInEx);
    assert_eq!(
        mods.provided
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["denikson-BepInExPack_Valheim"]
    );
    assert_eq!(mods.work_dir, ".grizzly/thunderstore");
}

#[test]
fn thunderstore_is_off_for_a_game_that_declares_nothing() {
    // Every existing image sets none of these and must behave exactly as before.
    let config = SupervisorConfig::from_env_with(&lookup_from(&[])).unwrap();
    assert_eq!(config.thunderstore, None);
}

#[test]
fn a_mod_list_without_a_community_disables_the_reconciler() {
    // The two halves come from different places — the manifest and the image —
    // so a half-rolled deploy is a real state. It warns and stays off rather
    // than refusing to start a live server on its next restart.
    let env = lookup_from(&[("SUPERVISOR_MODS_FILE", ".grizzly/mods")]);
    assert_eq!(
        SupervisorConfig::from_env_with(&env).unwrap().thunderstore,
        None
    );
}

#[test]
fn a_community_without_a_mod_list_disables_the_reconciler() {
    let env = lookup_from(&[("SUPERVISOR_THUNDERSTORE_COMMUNITY", "valheim")]);
    assert_eq!(
        SupervisorConfig::from_env_with(&env).unwrap().thunderstore,
        None
    );
}

#[test]
fn an_unknown_layout_is_a_hard_error() {
    // Unlike a missing half, a wrong value would install every mod into the
    // wrong place — a failure that looks like the game being broken.
    let env = lookup_from(&[
        ("SUPERVISOR_MODS_FILE", ".grizzly/mods"),
        ("SUPERVISOR_THUNDERSTORE_COMMUNITY", "valheim"),
        ("SUPERVISOR_THUNDERSTORE_LAYOUT", "ue4ss"),
    ]);
    let err = SupervisorConfig::from_env_with(&env).unwrap_err();
    assert!(err.to_string().contains("bepinex"), "got: {err}");
}

#[test]
fn an_unparseable_provided_package_is_a_hard_error() {
    // Silently dropping it would install a second loader pack over the image's,
    // which is how a working loader stops loading.
    let env = lookup_from(&[
        ("SUPERVISOR_MODS_FILE", ".grizzly/mods"),
        ("SUPERVISOR_THUNDERSTORE_COMMUNITY", "valheim"),
        ("SUPERVISOR_THUNDERSTORE_PROVIDED", "BepInExPack"),
    ]);
    let err = SupervisorConfig::from_env_with(&env).unwrap_err();
    assert!(err.to_string().contains("BepInExPack"), "got: {err}");
}

#[test]
fn the_provided_list_tolerates_spacing_and_stray_commas() {
    let env = lookup_from(&[
        ("SUPERVISOR_MODS_FILE", ".grizzly/mods"),
        ("SUPERVISOR_THUNDERSTORE_COMMUNITY", "valheim"),
        (
            "SUPERVISOR_THUNDERSTORE_PROVIDED",
            " denikson-BepInExPack_Valheim , ,Other-Pack ,",
        ),
    ]);
    let mods = SupervisorConfig::from_env_with(&env)
        .unwrap()
        .thunderstore
        .expect("the reconciler is configured");
    assert_eq!(mods.provided.len(), 2);
}

#[test]
fn the_working_directory_can_be_moved_off_its_default() {
    let env = lookup_from(&[
        ("SUPERVISOR_MODS_FILE", ".grizzly/mods"),
        ("SUPERVISOR_THUNDERSTORE_COMMUNITY", "valheim"),
        ("SUPERVISOR_THUNDERSTORE_DIR", ".grizzly/ts"),
    ]);
    let mods = SupervisorConfig::from_env_with(&env)
        .unwrap()
        .thunderstore
        .expect("the reconciler is configured");
    assert_eq!(mods.work_dir, ".grizzly/ts");
}
