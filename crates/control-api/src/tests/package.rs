use super::*;

#[test]
fn a_package_id_round_trips() {
    let id: PackageId = "ValheimModding-Jotunn".parse().unwrap();
    assert_eq!(id.namespace, "ValheimModding");
    assert_eq!(id.name, "Jotunn");
    assert_eq!(id.to_string(), "ValheimModding-Jotunn");
}

#[test]
fn underscores_are_part_of_a_name_and_dashes_are_not() {
    // The real loader pack's name carries an underscore; allowing a dash inside
    // a component would make every identifier ambiguous to split.
    assert!("denikson-BepInExPack_Valheim".parse::<PackageId>().is_ok());
    assert!("a-b-c".parse::<PackageId>().is_err());
    assert!("Jotunn".parse::<PackageId>().is_err());
    assert!("-Jotunn".parse::<PackageId>().is_err());
    assert!("ValheimModding-".parse::<PackageId>().is_err());
}

#[test]
fn versions_order_numerically_not_lexically() {
    let ten: PackageVersion = "1.10.0".parse().unwrap();
    let nine: PackageVersion = "1.9.0".parse().unwrap();
    assert!(
        ten > nine,
        "a string comparison would put 1.10.0 below 1.9.0"
    );
}

#[test]
fn a_version_is_exactly_three_numbers() {
    assert_eq!(
        "5.4.2333".parse::<PackageVersion>().unwrap(),
        PackageVersion {
            major: 5,
            minor: 4,
            patch: 2333
        }
    );
    for bad in ["1.2", "1.2.3.4", "1.2.x", "v1.2.3", "1.2.3-alpha", ""] {
        assert!(
            bad.parse::<PackageVersion>().is_err(),
            "{bad:?} should not parse"
        );
    }
}

#[test]
fn a_release_splits_the_version_off_the_right() {
    let release: PackageRelease = "denikson-BepInExPack_Valheim-5.4.2333".parse().unwrap();
    assert_eq!(release.id.namespace, "denikson");
    assert_eq!(release.id.name, "BepInExPack_Valheim");
    assert_eq!(release.version.to_string(), "5.4.2333");
    assert_eq!(release.to_string(), "denikson-BepInExPack_Valheim-5.4.2333");
}

#[test]
fn an_unversioned_package_is_not_a_release() {
    // The distinction the mod-list parser leans on to tell a pin from a bare
    // package without a second grammar.
    assert!("ValheimModding-Jotunn".parse::<PackageRelease>().is_err());
}

#[test]
fn parse_errors_say_what_shape_was_expected() {
    let err = "nonsense"
        .parse::<PackageRelease>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("Namespace-Name-1.2.3"), "got: {err}");
}
