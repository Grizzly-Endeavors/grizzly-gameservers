use super::*;

/// A baseline that doesn't depend on `BotConfig`, so the catalog can be tested
/// without building a whole environment.
fn baseline() -> SettingsSnapshot {
    SettingsSnapshot::for_test()
}

fn stored(value: &str) -> StoredOverride {
    StoredOverride {
        value: value.to_owned(),
        updated_at: "2026-08-05 00:00:00+00".to_owned(),
        updated_by: Some("tester".to_owned()),
    }
}

fn overrides(entries: &[(SettingKey, &str)]) -> BTreeMap<SettingKey, StoredOverride> {
    entries
        .iter()
        .map(|(key, value)| (*key, stored(value)))
        .collect()
}

#[test]
fn every_key_has_a_unique_database_key() {
    let mut seen = std::collections::HashSet::new();
    for key in SettingKey::ALL {
        assert!(
            seen.insert(key.as_str()),
            "duplicate database key: {}",
            key.as_str()
        );
    }
    assert_eq!(seen.len(), SettingKey::ALL.len());
}

#[test]
fn every_key_carries_a_grammar_hint() {
    for key in SettingKey::ALL {
        assert!(
            !key.expected().is_empty(),
            "{} has no grammar hint, so its rejection message would name nothing",
            key.as_str()
        );
    }
}

#[test]
fn every_key_resolves_back_from_its_database_key() {
    for key in SettingKey::ALL {
        assert_eq!(SettingKey::from_key(key.as_str()), Some(*key));
    }
    assert_eq!(SettingKey::from_key("nonsense.key"), None);
}

#[test]
fn an_override_beats_the_baseline() {
    let composed = compose(&baseline(), &overrides(&[(SettingKey::GaryModel, "qwen3")]));
    assert_eq!(composed.gary_model, "qwen3");
    // Untouched keys keep the deployed value.
    assert_eq!(composed.gary_base_url, "https://ollama.com/v1");
}

#[test]
fn clearing_an_override_returns_to_the_baseline() {
    let baseline = baseline();
    let with = compose(&baseline, &overrides(&[(SettingKey::ThrottleBurst, "42")]));
    assert_eq!(with.throttle.burst, 42);
    let without = compose(&baseline, &BTreeMap::new());
    assert_eq!(without.throttle.burst, baseline.throttle.burst);
}

#[test]
fn quantities_accept_the_kubernetes_grammar() {
    let mut snapshot = baseline();
    SettingKey::GuildMaxCpu
        .assign(&mut snapshot, "32")
        .expect("bare cores parse");
    assert_eq!(snapshot.guild_limits.cpu_limit_millicores, 32_000);
    SettingKey::GuildMaxCpu
        .assign(&mut snapshot, "1500m")
        .expect("millicores parse");
    assert_eq!(snapshot.guild_limits.cpu_limit_millicores, 1_500);
    SettingKey::GuildMaxMemory
        .assign(&mut snapshot, "64Gi")
        .expect("binary suffix parses");
    assert_eq!(
        snapshot.guild_limits.mem_limit_bytes,
        64 * 1024 * 1024 * 1024
    );
}

#[test]
fn the_allowlist_tolerates_whitespace_and_trailing_commas() {
    let mut snapshot = baseline();
    SettingKey::GuildAllowlist
        .assign(&mut snapshot, " 123 , 456, ")
        .expect("a padded list parses");
    assert_eq!(snapshot.guild_allowlist.as_ref(), [123, 456]);
}

#[test]
fn an_empty_allowlist_is_accepted_and_means_every_guild() {
    let mut snapshot = baseline();
    SettingKey::GuildAllowlist
        .assign(&mut snapshot, "789")
        .expect("a list parses");
    SettingKey::GuildAllowlist
        .assign(&mut snapshot, "")
        .expect("an empty list is a legitimate value, not a parse failure");
    assert!(snapshot.guild_allowlist.is_empty());
}

#[test]
fn a_non_integer_allowlist_entry_is_refused() {
    let mut snapshot = baseline();
    let err = SettingKey::GuildAllowlist
        .assign(&mut snapshot, "123,not-a-guild")
        .expect_err("a non-integer entry is refused");
    assert!(err.to_string().contains("not-a-guild"), "{err}");
}

#[test]
fn zero_is_refused_wherever_it_would_break_a_consumer() {
    for key in [
        SettingKey::ThrottleBurst,
        SettingKey::ThrottleRefillSeconds,
        SettingKey::ThrottleMaxConcurrent,
        SettingKey::GaryMaxRounds,
        SettingKey::BackupRetention,
        SettingKey::BackupIntervalHours,
        SettingKey::SampleIntervalMinutes,
    ] {
        let mut snapshot = baseline();
        assert!(
            key.assign(&mut snapshot, "0").is_err(),
            "{} accepted a zero",
            key.as_str()
        );
    }
}

#[test]
fn a_rejected_value_leaves_the_snapshot_untouched() {
    let baseline = baseline();
    let mut snapshot = baseline.clone();
    assert!(
        SettingKey::GuildMaxCpu
            .assign(&mut snapshot, "lots")
            .is_err()
    );
    assert_eq!(snapshot, baseline);
}

#[test]
fn the_watchdog_mode_vocabulary_is_closed() {
    let mut snapshot = baseline();
    for (raw, expected) in [
        ("on", WatchdogMode::On),
        ("SHADOW", WatchdogMode::Shadow),
        (" off ", WatchdogMode::Off),
    ] {
        SettingKey::WatchdogModeKey
            .assign(&mut snapshot, raw)
            .expect("a known mode parses");
        assert_eq!(snapshot.watchdog, expected);
    }
    assert!(
        SettingKey::WatchdogModeKey
            .assign(&mut snapshot, "shaddow")
            .is_err(),
        "a typo must not read as a mode"
    );
}

/// An unparseable stored row is a bug somewhere upstream, but it must degrade to
/// the deployed value rather than taking the process down or silently dropping
/// every *other* override with it.
#[test]
fn an_unusable_stored_row_falls_back_without_disturbing_its_neighbours() {
    let baseline = baseline();
    let composed = compose(
        &baseline,
        &overrides(&[
            (SettingKey::GuildMaxCpu, "not-a-quantity"),
            (SettingKey::GuildMaxServers, "9"),
        ]),
    );
    assert_eq!(
        composed.guild_limits.cpu_limit_millicores,
        baseline.guild_limits.cpu_limit_millicores
    );
    assert_eq!(composed.guild_limits.servers, 9);
}

#[test]
fn an_unrecognized_key_is_ignored_rather_than_guessed_at() {
    assert_eq!(SettingKey::from_key("gary.model "), None);
    assert_eq!(SettingKey::from_key("Gary.Model"), None);
}

#[test]
fn crossed_sizing_bounds_are_refused() {
    let mut snapshot = baseline();
    SettingKey::SizingMaxCpu
        .assign(&mut snapshot, "100m")
        .expect("the value itself parses");
    // Parsing succeeds in isolation; the cross-field rule is what catches it.
    assert!(snapshot.coherence().is_err());
}

#[test]
fn coherent_sizing_bounds_pass() {
    let mut snapshot = baseline();
    SettingKey::SizingMinCpu
        .assign(&mut snapshot, "500m")
        .expect("parses");
    SettingKey::SizingMaxCpu
        .assign(&mut snapshot, "8")
        .expect("parses");
    assert!(snapshot.coherence().is_ok());
}

#[test]
fn a_facade_without_persistence_serves_the_baseline() {
    let settings = RuntimeSettings::fixed(baseline());
    assert_eq!(settings.snapshot().gary_model, "glm-5.2");
    assert_eq!(settings.snapshot().backup_retention, 7);
}

#[tokio::test]
async fn a_facade_without_persistence_refuses_writes_rather_than_pretending() {
    let settings = RuntimeSettings::fixed(baseline());
    assert!(!settings.is_available());
    let err = settings
        .apply(SettingKey::GaryModel, Some("qwen3"), "tester")
        .await
        .expect_err("writes are refused without a database");
    assert!(err.to_string().contains("no database"), "{err}");
    // The refusal changed nothing, so the running configuration is still honest.
    assert_eq!(settings.snapshot().gary_model, "glm-5.2");
}

#[test]
fn the_booted_snapshot_is_what_the_process_started_with() {
    let settings = RuntimeSettings::fixed(baseline());
    assert_eq!(settings.booted().backup_retention, 7);
    assert_eq!(settings.baseline().backup_retention, 7);
}

/// Restart-class keys are the ones a consumer captures at startup. Getting this
/// list wrong is invisible until an operator changes one and it silently does
/// nothing, so it is pinned rather than left to the reader of `reload()`.
#[test]
fn only_the_cycle_cadences_wait_for_a_restart() {
    let restart: Vec<&str> = SettingKey::ALL
        .iter()
        .filter(|key| key.reload() == Reload::Restart)
        .map(|key| key.as_str())
        .collect();
    assert_eq!(
        restart,
        [
            "backup.interval_hours",
            "backup.retention",
            "archive.retention_days",
            "reconcile.interval_minutes",
            "reconcile.stranded_dwell_minutes",
            "sampling.interval_minutes",
        ]
    );
}

#[test]
fn every_key_carries_operator_facing_copy() {
    for key in SettingKey::ALL {
        assert!(!key.summary().is_empty(), "{} has no summary", key.as_str());
        assert!(!key.section().title().is_empty());
    }
}

/// The panel puts a rendered value straight into an editable field, so anything
/// that doesn't survive the round trip would silently rewrite a setting the
/// operator only opened to look at.
#[test]
fn rendering_a_value_round_trips_through_assign() {
    let baseline = baseline();
    for key in SettingKey::ALL {
        let rendered = key.render(&baseline);
        let mut probe = baseline.clone();
        key.assign(&mut probe, &rendered)
            .unwrap_or_else(|err| panic!("{} did not round trip: {err}", key.as_str()));
        assert_eq!(
            probe,
            baseline,
            "{} round tripped to a different value ({rendered})",
            key.as_str()
        );
    }
}

#[test]
fn bytes_render_in_the_largest_exact_binary_unit() {
    assert_eq!(exact_bytes(48 * 1024 * 1024 * 1024), "48Gi");
    assert_eq!(exact_bytes(512 * 1024 * 1024), "512Mi");
    assert_eq!(exact_bytes(1536), "1536");
    assert_eq!(exact_bytes(3 * 1024), "3Ki");
}

/// A bare number means *cores* to the CPU parser, so millicores must always be
/// rendered with their suffix or a save would multiply the value by a thousand.
#[test]
fn cpu_renders_with_an_explicit_millicore_suffix() {
    assert_eq!(exact_cpu(24_000), "24000m");
    assert_eq!(exact_cpu(250), "250m");
    assert_eq!(parse_cpu_millicores(&exact_cpu(250)), Some(250));
}
