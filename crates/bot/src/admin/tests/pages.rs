use super::*;

use crate::settings::SettingsSnapshot;

fn stored(value: &str) -> StoredOverride {
    StoredOverride {
        value: value.to_owned(),
        updated_at: "2026-08-05 12:00:00+00".to_owned(),
        updated_by: Some("bear".to_owned()),
    }
}

/// A restart-class override that the running process hasn't picked up must be
/// called out, or the settings page shows a number the bot isn't using with
/// nothing to say so.
#[test]
fn a_recorded_restart_setting_that_is_not_in_force_is_flagged() {
    let booted = SettingsSnapshot::for_test();
    let mut effective = booted.clone();
    effective.backup_retention = 14;
    let overrides = [(SettingKey::BackupRetention, stored("14"))]
        .into_iter()
        .collect();

    let rendered = pending_restart_notice(&effective, &booted, &overrides).into_string();
    assert!(rendered.contains("backup.retention"), "{rendered}");
    assert!(rendered.contains("restart"), "{rendered}");
}

/// A hot setting takes effect immediately, so it is never "waiting" — flagging
/// one would train the operator to ignore the warning.
#[test]
fn a_hot_setting_is_never_flagged_as_waiting() {
    let booted = SettingsSnapshot::for_test();
    let mut effective = booted.clone();
    effective.gary_model = "qwen3".to_owned();
    let overrides = [(SettingKey::GaryModel, stored("qwen3"))]
        .into_iter()
        .collect();

    assert!(
        pending_restart_notice(&effective, &booted, &overrides)
            .into_string()
            .is_empty()
    );
}

/// An override set to the value the process already booted with is in force
/// whatever its class, so there is nothing to wait for.
#[test]
fn a_restart_setting_matching_the_booted_value_is_not_flagged() {
    let booted = SettingsSnapshot::for_test();
    let overrides = [(
        SettingKey::BackupRetention,
        stored(&SettingKey::BackupRetention.render(&booted)),
    )]
    .into_iter()
    .collect();

    assert!(
        pending_restart_notice(&booted.clone(), &booted, &overrides)
            .into_string()
            .is_empty()
    );
}

#[test]
fn nothing_is_flagged_when_no_overrides_exist() {
    let booted = SettingsSnapshot::for_test();
    assert!(
        pending_restart_notice(&booted.clone(), &booted, &BTreeMap::new())
            .into_string()
            .is_empty()
    );
}

#[test]
fn blank_filter_fields_do_not_narrow_the_query() {
    let query = EventQuery {
        kind: Some(String::new()),
        guild: Some("   ".to_owned()),
        instance: Some("mc".to_owned()),
        surface: None,
        before: None,
    };
    let filter = query.filter();
    assert_eq!(filter.kind, None);
    assert_eq!(filter.guild, None);
    assert_eq!(filter.instance.as_deref(), Some("mc"));
    assert_eq!(filter.before_id, None);
}

/// Paging must not silently drop the filters the operator set, or "older" would
/// jump from a narrowed view back to the whole log.
#[test]
fn paging_preserves_the_active_filters() {
    let query = EventQuery {
        kind: Some("server_created".to_owned()),
        guild: None,
        instance: Some("mc-1".to_owned()),
        surface: Some(String::new()),
        before: None,
    };
    let next = query.next_page(42);
    assert!(next.starts_with("/events?before=42"), "{next}");
    assert!(next.contains("kind=server_created"), "{next}");
    assert!(next.contains("instance=mc-1"), "{next}");
    assert!(!next.contains("surface="), "{next}");
}

#[test]
fn filter_values_are_percent_encoded_into_the_page_link() {
    let query = EventQuery {
        kind: None,
        guild: None,
        instance: Some("a b&c=d".to_owned()),
        surface: None,
        before: None,
    };
    let next = query.next_page(7);
    assert!(next.contains("instance=a%20b%26c%3Dd"), "{next}");
}

#[test]
fn urlencode_leaves_the_unreserved_set_alone() {
    assert_eq!(urlencode("abcXYZ019-_.~"), "abcXYZ019-_.~");
    assert_eq!(urlencode("a/b?c"), "a%2Fb%3Fc");
}

#[test]
fn human_bytes_reads_the_way_a_person_would_say_it() {
    assert_eq!(human_bytes(48 * 1024 * 1024 * 1024), "48 GiB");
    assert_eq!(human_bytes(512 * 1024 * 1024), "512 MiB");
    assert_eq!(human_bytes(1536 * 1024 * 1024), "1.5 GiB");
    assert_eq!(human_bytes(900), "900 B");
}
