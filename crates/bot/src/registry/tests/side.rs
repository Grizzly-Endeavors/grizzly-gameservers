use super::*;

#[test]
fn flags_classify_all_four_combinations() {
    assert_eq!(SideSupport::from_flags(true, true), SideSupport::Both);
    assert_eq!(
        SideSupport::from_flags(true, false),
        SideSupport::ServerOnly
    );
    assert_eq!(
        SideSupport::from_flags(false, true),
        SideSupport::ClientOnly
    );
    assert_eq!(SideSupport::from_flags(false, false), SideSupport::Unknown);
}

#[test]
fn a_both_sided_mod_answers_a_server_search() {
    // The filter reads "applies to", not "applies only to" — a mod the players
    // also need still has to be installed on the server.
    assert!(SideSupport::Both.matches(Side::Server));
    assert!(SideSupport::Both.matches(Side::Client));
    assert!(SideSupport::Both.matches(Side::Both));
}

#[test]
fn a_one_sided_mod_answers_only_its_own_side() {
    assert!(SideSupport::ServerOnly.matches(Side::Server));
    assert!(!SideSupport::ServerOnly.matches(Side::Client));
    assert!(!SideSupport::ServerOnly.matches(Side::Both));
    assert!(SideSupport::ClientOnly.matches(Side::Client));
    assert!(!SideSupport::ClientOnly.matches(Side::Server));
}

#[test]
fn an_unclassified_mod_answers_no_side_filter() {
    // Surviving a `server` filter would read as the registry vouching for it.
    for side in [Side::Server, Side::Client, Side::Both] {
        assert!(!SideSupport::Unknown.matches(side));
    }
}

#[test]
fn every_label_is_distinct_so_a_line_never_reads_ambiguously() {
    let labels = [
        SideSupport::ServerOnly.label(),
        SideSupport::ClientOnly.label(),
        SideSupport::Both.label(),
        SideSupport::Unknown.label(),
    ];
    let mut sorted = labels.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), labels.len());
}
