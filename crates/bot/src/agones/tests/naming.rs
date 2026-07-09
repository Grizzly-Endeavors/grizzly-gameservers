use super::*;

fn ports(values: &[i32]) -> BTreeSet<i32> {
    values.iter().copied().collect()
}

#[test]
fn pvc_name_appends_data_suffix() {
    assert_eq!(pvc_name("minecraft-ab12"), "minecraft-ab12-data");
}

#[test]
fn supplied_name_is_sanitized_and_used_without_game_prefix() {
    let name = build_instance_name("minecraft", Some("Bob's World!"), 0).unwrap();
    assert_eq!(name, "bob-s-world");
}

#[test]
fn generated_name_uses_game_prefix_and_fixed_length_id() {
    let name = build_instance_name("valheim", None, 123_456_789).unwrap();
    let id = name.strip_prefix("valheim-").unwrap();
    assert_eq!(id.len(), GENERATED_ID_LEN, "generated id has fixed length");
    assert!(
        id.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "id should be a clean label segment, got {id}"
    );
}

#[test]
fn generated_name_is_deterministic_for_a_given_entropy() {
    let first = build_instance_name("minecraft", None, 42).unwrap();
    let second = build_instance_name("minecraft", None, 42).unwrap();
    assert_eq!(first, second);
}

#[test]
fn name_with_no_alphanumerics_is_rejected() {
    let err = build_instance_name("minecraft", Some("!!!"), 0).unwrap_err();
    assert!(
        err.to_string().contains("at least one letter or number"),
        "should explain why the name is unusable, got: {err}"
    );
}

#[test]
fn overlong_name_is_rejected() {
    let long = "a".repeat(80);
    let err = build_instance_name("minecraft", Some(&long), 0).unwrap_err();
    assert!(
        err.to_string().contains("too long"),
        "should reject names past the length budget, got: {err}"
    );
}

#[test]
fn select_one_free_port_returns_lowest_available() {
    let used = ports(&[7000, 7001, 7003]);
    let port = select_free_ports(1, &used, &BTreeSet::new(), 7000..=7010);
    assert_eq!(port, Some(vec![7002]));
}

#[test]
fn select_free_ports_skips_excluded_ports() {
    let used = ports(&[7000]);
    let excluded = ports(&[7001, 7002]);
    let port = select_free_ports(1, &used, &excluded, 7000..=7010);
    assert_eq!(port, Some(vec![7003]));
}

#[test]
fn select_free_ports_returns_none_when_range_is_full() {
    let used: BTreeSet<i32> = (7000..=7010).collect();
    let port = select_free_ports(1, &used, &BTreeSet::new(), 7000..=7010);
    assert_eq!(port, None);
}

#[test]
fn select_free_ports_ignores_out_of_range_used_ports() {
    let used = ports(&[80, 443, 30000]);
    let port = select_free_ports(1, &used, &BTreeSet::new(), 7000..=7010);
    assert_eq!(port, Some(vec![7000]));
}

#[test]
fn select_two_free_ports_returns_the_lowest_available_pair() {
    let used = ports(&[7000, 7002]);
    let picked = select_free_ports(2, &used, &BTreeSet::new(), 7000..=7010);
    assert_eq!(picked, Some(vec![7001, 7003]));
}

#[test]
fn select_free_ports_returns_none_when_band_cannot_satisfy_count() {
    // Only one slot left (7010) but a two-port game needs two.
    let used: BTreeSet<i32> = (7000..=7009).collect();
    let picked = select_free_ports(2, &used, &BTreeSet::new(), 7000..=7010);
    assert_eq!(picked, None);
}
