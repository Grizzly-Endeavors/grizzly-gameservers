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
// The control-plane port/host values are pinned in five independent places: the
// `DEFAULT_*` consts above, the supervisor crate's own `DEFAULT_CONTROL_PORT`,
// and four Cilium egress carve-outs under `cluster/guardrails/`. If any one
// drifts from the others, Cilium silently *drops* the packets rather than
// erroring — a connect timeout that points nowhere near the NetworkPolicy. The
// cross-reference comments between these sites don't stop drift; these tests read
// the real YAML off disk and fail CI loudly the moment a const and its carve-out
// disagree. Mirrors the `real_satisfactory_manifest_is_on_the_advertise_path`
// pattern in `agones/tests/ports.rs`.

use std::path::{Path, PathBuf};

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

/// Reads a `const NAME: u16 = N;` literal out of a Rust source file by a targeted
/// line scan — enough to reach the supervisor crate's `DEFAULT_CONTROL_PORT`
/// without a build dependency on it, so its value is pinned against the shared
/// control port too. Deliberately a line scan, not a Rust parser: the const lines
/// are flat and this stays cheap to keep in sync with them.
fn rust_u16_const(source_path: &Path, name: &str) -> u16 {
    let source = std::fs::read_to_string(source_path)
        .unwrap_or_else(|err| panic!("reading {source_path:?}: {err}"));
    let prefix = format!("const {name}: u16 = ");
    let value = source
        .lines()
        .map(str::trim_start)
        .find_map(|line| line.strip_prefix(prefix.as_str()))
        .unwrap_or_else(|| panic!("{name} not found in {source_path:?}"));
    value
        .trim_end()
        .trim_end_matches(';')
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("{name} value {value:?} is not a valid u16"))
}

fn supervisor_config_path() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../supervisor/src/config.rs"
    ))
}

#[test]
fn supervisor_egress_port_matches_control_port_on_both_sides() {
    let yaml_port = load_egress("bot-to-supervisor-egress.yaml").only_port();
    assert_eq!(
        yaml_port, DEFAULT_CONTROL_PORT,
        "bot-to-supervisor-egress.yaml opens {yaml_port} but bot DEFAULT_CONTROL_PORT is \
         {DEFAULT_CONTROL_PORT}; Cilium would silently drop the bot's control calls (issue #42)"
    );
    let supervisor_control_port = rust_u16_const(&supervisor_config_path(), "DEFAULT_CONTROL_PORT");
    assert_eq!(
        supervisor_control_port, DEFAULT_CONTROL_PORT,
        "supervisor DEFAULT_CONTROL_PORT ({supervisor_control_port}) must equal the bot's \
         ({DEFAULT_CONTROL_PORT}) — they name the same in-pod control API port (issue #42)"
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
