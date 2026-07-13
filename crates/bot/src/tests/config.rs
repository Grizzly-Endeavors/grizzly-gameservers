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
fn valkey_is_none_without_a_password() {
    // Like DB, the password is the OpenBao-sourced part; its absence disables the
    // deferred-task queue rather than failing startup.
    let env = lookup_from(&[("DISCORD_BOT_TOKEN", "secret")]);
    assert!(
        BotConfig::from_env_with(&env).unwrap().valkey.is_none(),
        "no REDIS_PASSWORD should disable the deferred-task queue"
    );
}

#[test]
fn valkey_defaults_to_foundation_kv_cache_when_password_present() {
    let env = lookup_from(&[("DISCORD_BOT_TOKEN", "secret"), ("REDIS_PASSWORD", "pw")]);
    let valkey = BotConfig::from_env_with(&env).unwrap().valkey.unwrap();
    assert_eq!(valkey.host, "10.0.0.200");
    assert_eq!(valkey.port, 6379);
    assert_eq!(valkey.db, 2);
    assert_eq!(valkey.password, "pw");
    assert_eq!(valkey.url(), "redis://:pw@10.0.0.200:6379/2");
}

#[test]
fn blank_valkey_password_reads_as_absent() {
    let env = lookup_from(&[("DISCORD_BOT_TOKEN", "secret"), ("REDIS_PASSWORD", "")]);
    assert!(
        BotConfig::from_env_with(&env).unwrap().valkey.is_none(),
        "a blank password should read as unset, not an empty credential"
    );
}

#[test]
fn valkey_overrides_apply() {
    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("REDIS_PASSWORD", "pw"),
        ("REDIS_HOST", "127.0.0.1"),
        ("REDIS_PORT", "6400"),
        ("REDIS_DB", "5"),
    ]);
    let valkey = BotConfig::from_env_with(&env).unwrap().valkey.unwrap();
    assert_eq!(valkey.host, "127.0.0.1");
    assert_eq!(valkey.port, 6400);
    assert_eq!(valkey.db, 5);
}

#[test]
fn out_of_range_valkey_db_is_an_error() {
    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("REDIS_PASSWORD", "pw"),
        ("REDIS_DB", "16"),
    ]);
    let err = BotConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string().contains("REDIS_DB"),
        "an out-of-range Redis DB index should name the variable, got: {err}"
    );
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

#[test]
fn zero_ports_are_rejected() {
    for key in ["GAMESERVERS_CONTROL_PORT", "GAMESERVERS_AGENT_PORT"] {
        let pairs = [("DISCORD_BOT_TOKEN", "secret"), (key, "0")];
        let env = lookup_from(&pairs);
        let err = BotConfig::from_env_with(&env).unwrap_err();
        assert!(
            err.to_string().contains(key),
            "a zero {key} should be rejected and name the variable, got: {err}"
        );
    }

    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("DB_PASSWORD", "pw"),
        ("DB_PORT", "0"),
    ]);
    let err = BotConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string().contains("DB_PORT"),
        "a zero DB_PORT should be rejected, got: {err}"
    );
}

#[test]
fn zero_backup_interval_is_rejected() {
    // 0 would flow into tokio::time::interval, which panics on a zero period.
    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("GAMESERVERS_BACKUP_INTERVAL_HOURS", "0"),
    ]);
    let err = BotConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string()
            .contains("GAMESERVERS_BACKUP_INTERVAL_HOURS"),
        "a zero backup interval should be rejected, got: {err}"
    );
}

#[test]
fn zero_backup_retention_is_rejected() {
    // 0 would prune every key each cycle — backups run but nothing is kept.
    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("GAMESERVERS_BACKUP_RETENTION", "0"),
    ]);
    let err = BotConfig::from_env_with(&env).unwrap_err();
    assert!(
        err.to_string().contains("GAMESERVERS_BACKUP_RETENTION"),
        "a zero backup retention should be rejected, got: {err}"
    );
}

#[test]
fn positive_backup_settings_are_accepted() {
    let env = lookup_from(&[
        ("DISCORD_BOT_TOKEN", "secret"),
        ("GAMESERVERS_BACKUP_INTERVAL_HOURS", "6"),
        ("GAMESERVERS_BACKUP_RETENTION", "3"),
    ]);
    let config = BotConfig::from_env_with(&env).unwrap();
    assert_eq!(config.backup_interval, std::time::Duration::from_hours(6));
    assert_eq!(config.backup_retention, 3);
}

// ---- control-plane egress drift guard (issue #42) ----
//
// The control-plane host/port values are pinned in independent places Cilium
// can't cross-check: the `DEFAULT_*` consts above and the Cilium egress carve-outs
// under `cluster/guardrails/`. If a const drifts from its carve-out, Cilium
// silently *drops* the packets rather than erroring — a connect timeout that
// points nowhere near the NetworkPolicy. The cross-reference comments between
// these sites don't stop drift; these tests read the real YAML off disk and fail
// CI loudly the moment a const and its carve-out disagree. Mirrors the
// `real_satisfactory_manifest_is_on_the_advertise_path` pattern in
// `agones/tests/ports.rs`.
//
// The supervisor control port is no longer among the drifting pairs: the bot and
// supervisor both default to `grizzly_control_api::CONTROL_PORT`, so the two Rust
// sides are equal by construction — only that shared const vs. its
// `bot-to-supervisor-egress.yaml` carve-out still needs guarding below.

use std::path::PathBuf;

/// Minimal view of a `CiliumNetworkPolicy` egress carve-out — only the `port`
/// and `cidr` literals these guardrails pin. Deliberately partial so an unrelated
/// edit elsewhere in the manifest (selectors, metadata, comments) can't perturb
/// the drift check.
#[derive(serde::Deserialize)]
struct EgressPolicy {
    spec: EgressSpec,
}

#[derive(serde::Deserialize)]
struct EgressSpec {
    egress: Vec<EgressRule>,
}

#[derive(serde::Deserialize)]
struct EgressRule {
    #[serde(default, rename = "toCIDRSet")]
    to_cidr_set: Vec<CidrEntry>,
    #[serde(default, rename = "toPorts")]
    to_ports: Vec<ToPortRule>,
}

#[derive(serde::Deserialize)]
struct CidrEntry {
    cidr: String,
}

#[derive(serde::Deserialize)]
struct ToPortRule {
    ports: Vec<PortEntry>,
}

#[derive(serde::Deserialize)]
struct PortEntry {
    // Cilium serialises the port as a quoted string ("9359"), not an int.
    port: String,
}

impl EgressPolicy {
    /// The single TCP port this carve-out opens, parsed to a `u16`. Panics unless
    /// the policy declares exactly one — every guardrail here is single-port, and
    /// a second port sneaking in should fail the check, not be silently ignored.
    fn only_port(&self) -> u16 {
        let ports: Vec<u16> = self
            .spec
            .egress
            .iter()
            .flat_map(|rule| rule.to_ports.iter())
            .flat_map(|to_port| to_port.ports.iter())
            .map(|entry| {
                entry
                    .port
                    .parse()
                    .unwrap_or_else(|_| panic!("port {:?} is not a valid u16", entry.port))
            })
            .collect();
        let [port] = ports.as_slice() else {
            panic!("expected exactly one egress port, got {ports:?}");
        };
        *port
    }

    /// The single `/32` host this carve-out pins, with the mask stripped. Panics
    /// unless the policy declares exactly one host CIDR.
    fn only_cidr_host(&self) -> String {
        let cidrs: Vec<&str> = self
            .spec
            .egress
            .iter()
            .flat_map(|rule| rule.to_cidr_set.iter())
            .map(|entry| entry.cidr.as_str())
            .collect();
        let [cidr] = cidrs.as_slice() else {
            panic!("expected exactly one CIDR, got {cidrs:?}");
        };
        cidr.strip_suffix("/32")
            .unwrap_or_else(|| panic!("expected a /32 host CIDR, got {cidr:?}"))
            .to_owned()
    }
}

fn load_egress(file: &str) -> EgressPolicy {
    let path = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../cluster/guardrails"
    ))
    .join(file);
    let yaml =
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("reading {path:?}: {err}"));
    serde_yaml_ng::from_str(&yaml).unwrap_or_else(|err| panic!("parsing {path:?}: {err}"))
}

#[test]
fn supervisor_egress_port_matches_shared_control_port() {
    let yaml_port = load_egress("bot-to-supervisor-egress.yaml").only_port();
    assert_eq!(
        yaml_port, DEFAULT_CONTROL_PORT,
        "bot-to-supervisor-egress.yaml opens {yaml_port} but the shared control port \
         DEFAULT_CONTROL_PORT (= grizzly_control_api::CONTROL_PORT) is {DEFAULT_CONTROL_PORT}; \
         Cilium would silently drop the bot's control calls (issue #42)"
    );
}

#[test]
fn agent_egress_port_matches_agent_port_default() {
    let yaml_port = load_egress("game-to-bot-agent-egress.yaml").only_port();
    assert_eq!(
        yaml_port, DEFAULT_AGENT_PORT,
        "game-to-bot-agent-egress.yaml opens {yaml_port} but DEFAULT_AGENT_PORT is \
         {DEFAULT_AGENT_PORT}; Cilium would silently drop the in-game @Gary triggers (issue #42)"
    );
}

#[test]
fn postgres_egress_matches_db_host_and_port_defaults() {
    let policy = load_egress("bot-to-postgres-egress.yaml");
    assert_eq!(
        policy.only_cidr_host(),
        DEFAULT_DB_HOST,
        "bot-to-postgres-egress.yaml CIDR must match DEFAULT_DB_HOST (issue #42)"
    );
    assert_eq!(
        policy.only_port(),
        DEFAULT_DB_PORT,
        "bot-to-postgres-egress.yaml port must match DEFAULT_DB_PORT (issue #42)"
    );
}

#[test]
fn kv_cache_egress_matches_redis_host_and_port_defaults() {
    let policy = load_egress("bot-to-kv-cache-egress.yaml");
    assert_eq!(
        policy.only_cidr_host(),
        DEFAULT_REDIS_HOST,
        "bot-to-kv-cache-egress.yaml CIDR must match DEFAULT_REDIS_HOST (issue #42)"
    );
    assert_eq!(
        policy.only_port(),
        DEFAULT_REDIS_PORT,
        "bot-to-kv-cache-egress.yaml port must match DEFAULT_REDIS_PORT (issue #42)"
    );
}

#[test]
fn s3_egress_matches_s3_endpoint_host_and_port() {
    let policy = load_egress("bot-to-s3-egress.yaml");
    let endpoint = url::Url::parse(DEFAULT_S3_ENDPOINT).unwrap();
    let host = endpoint
        .host_str()
        .expect("DEFAULT_S3_ENDPOINT should carry a host");
    let port = endpoint
        .port()
        .expect("DEFAULT_S3_ENDPOINT should carry an explicit port");
    assert_eq!(
        policy.only_cidr_host(),
        host,
        "bot-to-s3-egress.yaml CIDR must match the host in DEFAULT_S3_ENDPOINT (issue #42)"
    );
    assert_eq!(
        policy.only_port(),
        port,
        "bot-to-s3-egress.yaml port must match the port in DEFAULT_S3_ENDPOINT (issue #42)"
    );
}

// ---- game manifest drift guards ----
//
// Two values in every `games/<game>/gameserver.yaml` must track a Rust source of
// truth that no YAML-level check can cross-verify. These read the real manifests
// off disk and fail CI the moment one drifts (mirroring the egress guards above
// and the control-port assertion in `agones/tests/instance.rs`):
//
//   * `spec.template.spec.terminationGracePeriodSeconds` — the window the kubelet
//     grants the pod after SIGTERM before it SIGKILLs. The in-pod supervisor is
//     PID 1 and catches SIGTERM to save the world within DEFAULT_GRACEFUL_TIMEOUT_SECS;
//     a shorter grace period truncates the save mid-write. Guarded as `>=` that window.
//   * the `control` containerPort — the port the bot dials the supervisor on. Both
//     sides default it to `grizzly_control_api::CONTROL_PORT`; the manifests
//     hardcode the same number and can't import the const.

/// The supervisor's SIGTERM world-save window, in seconds — the lower bound every
/// game pod's `terminationGracePeriodSeconds` must clear. Tracks
/// `DEFAULT_GRACEFUL_TIMEOUT_SECS` in `crates/supervisor/src/config.rs`, which is
/// private to that crate and not re-exported, so it can't be referenced here; this
/// literal mirrors it by hand. If the supervisor's graceful timeout ever rises
/// above 90, raise this and every manifest's `terminationGracePeriodSeconds` too.
const SUPERVISOR_GRACEFUL_TIMEOUT_SECS: u64 = 90;

/// Partial view of a game `GameServer` manifest — only the pod-spec fields these
/// drift guards pin, so an unrelated manifest edit can't perturb the check.
#[derive(serde::Deserialize)]
struct GameServerManifest {
    spec: ManifestSpec,
}

#[derive(serde::Deserialize)]
struct ManifestSpec {
    template: ManifestTemplate,
}

#[derive(serde::Deserialize)]
struct ManifestTemplate {
    spec: ManifestPodSpec,
}

#[derive(serde::Deserialize)]
struct ManifestPodSpec {
    // Optional so a manifest missing the field fails with the named assertion
    // below rather than serde's generic "missing field" error.
    #[serde(rename = "terminationGracePeriodSeconds")]
    termination_grace_period_seconds: Option<u64>,
    containers: Vec<ManifestContainer>,
}

#[derive(serde::Deserialize)]
struct ManifestContainer {
    #[serde(default)]
    ports: Vec<ManifestPort>,
}

#[derive(serde::Deserialize)]
struct ManifestPort {
    name: String,
    #[serde(rename = "containerPort")]
    container_port: u16,
}

impl ManifestPodSpec {
    /// The single `control` containerPort across the pod's containers. Panics
    /// unless exactly one is declared — a missing or duplicated control port is a
    /// drift the guard should catch, not skip over.
    fn control_port(&self) -> u16 {
        let ports: Vec<u16> = self
            .containers
            .iter()
            .flat_map(|container| container.ports.iter())
            .filter(|entry| entry.name == "control")
            .map(|entry| entry.container_port)
            .collect();
        let [port] = ports.as_slice() else {
            panic!("expected exactly one control port, got {ports:?}");
        };
        *port
    }
}

/// Every `games/<game>/gameserver.yaml` paired with its directory name.
/// Enumerated off disk rather than hardcoded so a newly-added game is guarded
/// automatically; includes `_template`, whose drift would propagate into every
/// game later copied from it.
fn load_game_manifests() -> Vec<(String, GameServerManifest)> {
    let games_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../games"));
    let entries =
        std::fs::read_dir(&games_dir).unwrap_or_else(|err| panic!("reading {games_dir:?}: {err}"));
    let mut manifests = Vec::new();
    for entry in entries {
        let dir = entry.unwrap_or_else(|err| panic!("reading a games/ entry: {err}"));
        let manifest_path = dir.path().join("gameserver.yaml");
        if !manifest_path.is_file() {
            continue;
        }
        let name = dir.file_name().to_string_lossy().into_owned();
        let yaml = std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|err| panic!("reading {manifest_path:?}: {err}"));
        let manifest = serde_yaml_ng::from_str(&yaml)
            .unwrap_or_else(|err| panic!("parsing {manifest_path:?}: {err}"));
        manifests.push((name, manifest));
    }
    assert!(
        !manifests.is_empty(),
        "no games/*/gameserver.yaml found under {games_dir:?}"
    );
    manifests
}

#[test]
fn game_manifests_grant_the_supervisor_its_full_shutdown_window() {
    for (game, manifest) in load_game_manifests() {
        let Some(grace) = manifest.spec.template.spec.termination_grace_period_seconds else {
            panic!(
                "games/{game}/gameserver.yaml has no \
                 spec.template.spec.terminationGracePeriodSeconds; the kubelet defaults it to \
                 30s and SIGKILLs the supervisor mid world-save"
            );
        };
        assert!(
            grace >= SUPERVISOR_GRACEFUL_TIMEOUT_SECS,
            "games/{game}/gameserver.yaml sets terminationGracePeriodSeconds={grace}, \
             below the supervisor's {SUPERVISOR_GRACEFUL_TIMEOUT_SECS}s save window \
             (DEFAULT_GRACEFUL_TIMEOUT_SECS); the world-save would be SIGKILLed mid-write"
        );
    }
}

#[test]
fn game_manifests_pin_the_shared_control_port() {
    for (game, manifest) in load_game_manifests() {
        assert_eq!(
            manifest.spec.template.spec.control_port(),
            grizzly_control_api::CONTROL_PORT,
            "games/{game}/gameserver.yaml declares a control containerPort that differs from \
             the shared grizzly_control_api::CONTROL_PORT; the bot would dial the wrong port"
        );
    }
}
